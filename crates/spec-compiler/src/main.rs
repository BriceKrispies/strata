//! The `specc` command.
//!
//! ```text
//! specc generate     --spec <file> --out <dir> [--crate-name <name>] [--runtime <dir>]
//! specc check        --spec <file> --out <dir> [--crate-name <name>] [--runtime <dir>]
//! specc deploy       --spec <file> --out <dir>
//! specc check-deploy --spec <file> --out <dir>
//! specc e2e          --spec <file> --out <dir>
//! specc check-e2e    --spec <file> --out <dir>
//! ```
//!
//! `deploy` and `check-deploy` do for the deployment files (a compose stack, Kubernetes
//! manifests and Prometheus alert rules; see `strata_spec_compiler::deploy`) what `generate`
//! and `check` do for the crate; their warnings go to standard error and do not change the exit
//! code. `generate` writes the crate into `<dir>` and removes generated files it no longer writes.
//! `check` exits 1 and lists the paths that differ when `<dir>` is not what `generate` would
//! leave. A specification that is not ready is refused with one `error[<code>] <entity>:
//! <message>` line per diagnostic and exit code 1. Usage errors exit with 2. The crate name
//! defaults to the last component of `<dir>`.
//!
//! A crate with implementation-level machines depends on `strata-runtime` by path: `--runtime`
//! names the runtime crate's directory, by default `crates/runtime` of the Strata checkout specc
//! was built from, and the manifest holds its path relative to `<dir>`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use strata_spec_compiler::emit::relative_path;
use strata_spec_compiler::{
    deploy, e2e, generate, output, GeneratedFile, Options, RuntimeDependency,
};

const USAGE: &str = "usage:
  specc generate     --spec <file> --out <dir> [--crate-name <name>] [--runtime <dir>]
  specc check        --spec <file> --out <dir> [--crate-name <name>] [--runtime <dir>]
  specc deploy       --spec <file> --out <dir>
  specc check-deploy --spec <file> --out <dir>
  specc e2e          --spec <file> --out <dir>
  specc check-e2e    --spec <file> --out <dir>";

/// The runtime crate of the checkout specc was built from.
const DEFAULT_RUNTIME: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../runtime");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Generate,
    Check,
    Deploy,
    CheckDeploy,
    E2e,
    CheckE2e,
}

struct Args {
    command: Command,
    spec: String,
    out: PathBuf,
    crate_name: String,
    runtime: PathBuf,
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let (command, rest) = match raw.split_first() {
        Some((c, rest)) if c == "generate" => (Command::Generate, rest),
        Some((c, rest)) if c == "check" => (Command::Check, rest),
        Some((c, rest)) if c == "deploy" => (Command::Deploy, rest),
        Some((c, rest)) if c == "check-deploy" => (Command::CheckDeploy, rest),
        Some((c, rest)) if c == "e2e" => (Command::E2e, rest),
        Some((c, rest)) if c == "check-e2e" => (Command::CheckE2e, rest),
        Some((c, _)) => return Err(format!("unknown command \"{c}\"")),
        None => return Err("no command given".to_string()),
    };
    let mut spec = None;
    let mut out = None;
    let mut crate_name = None;
    let mut runtime = None;
    let mut iter = rest.iter();
    while let Some(flag) = iter.next() {
        let slot = match flag.as_str() {
            "--spec" => &mut spec,
            "--out" => &mut out,
            "--crate-name" => &mut crate_name,
            "--runtime" => &mut runtime,
            other => return Err(format!("unknown argument \"{other}\"")),
        };
        let Some(value) = iter.next() else {
            return Err(format!("{flag} needs a value"));
        };
        if slot.replace(value.clone()).is_some() {
            return Err(format!("{flag} is given twice"));
        }
    }
    let spec = spec.ok_or("--spec is required")?;
    let out = PathBuf::from(out.ok_or("--out is required")?);
    let deploys = matches!(
        command,
        Command::Deploy | Command::CheckDeploy | Command::E2e | Command::CheckE2e
    );
    if deploys && crate_name.is_some() {
        return Err("--crate-name does not apply to deploy or e2e".to_string());
    }
    let crate_name = match crate_name {
        _ if deploys => String::new(),
        Some(name) => name,
        None => out
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or("--out has no last component to name the crate; give --crate-name")?,
    };
    Ok(Args {
        command,
        spec,
        out,
        crate_name,
        runtime: PathBuf::from(runtime.unwrap_or_else(|| DEFAULT_RUNTIME.to_string())),
    })
}

fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&raw) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("specc: {message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let text = match std::fs::read_to_string(&args.spec) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("specc: cannot read {}: {e}", args.spec);
            return ExitCode::from(1);
        }
    };
    if matches!(args.command, Command::Deploy | Command::CheckDeploy) {
        return run_deploy(&args, &text);
    }
    if matches!(args.command, Command::E2e | Command::CheckE2e) {
        return run_e2e(&args, &text);
    }
    let runtime = match runtime_path(&args.out, &args.runtime) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("specc: {message}");
            return ExitCode::from(1);
        }
    };
    let options = Options {
        crate_name: args.crate_name.clone(),
        runtime: RuntimeDependency::Path(runtime),
    };
    let files = match generate(&args.spec, &text, &options) {
        Ok(files) => files,
        Err(diagnostics) => {
            for d in &diagnostics {
                eprintln!("{d}");
            }
            eprintln!(
                "specc: refused {} with {} diagnostic(s); nothing was written",
                args.spec,
                diagnostics.len()
            );
            return ExitCode::from(1);
        }
    };
    match args.command {
        Command::Generate => run_generate(&args.out, &files),
        Command::Check => run_check(&args.out, &files),
        // Handled before generation.
        Command::Deploy | Command::CheckDeploy | Command::E2e | Command::CheckE2e => {
            ExitCode::from(2)
        }
    }
}

fn run_deploy(args: &Args, text: &str) -> ExitCode {
    let output = match deploy::generate(&args.spec, text) {
        Ok(output) => output,
        Err(diagnostics) => {
            for d in &diagnostics {
                eprintln!("{d}");
            }
            eprintln!(
                "specc: refused {} with {} diagnostic(s); nothing was written",
                args.spec,
                diagnostics.len()
            );
            return ExitCode::from(1);
        }
    };
    for warning in &output.warnings {
        eprintln!("{warning}");
    }
    match args.command {
        Command::CheckDeploy => run_check(&args.out, &output.files),
        _ => run_generate(&args.out, &output.files),
    }
}

fn run_e2e(args: &Args, text: &str) -> ExitCode {
    let output = match e2e::generate(&args.spec, text) {
        Ok(output) => output,
        Err(diagnostics) => {
            for d in &diagnostics {
                eprintln!("{d}");
            }
            eprintln!(
                "specc: refused {} with {} diagnostic(s); nothing was written",
                args.spec,
                diagnostics.len()
            );
            return ExitCode::from(1);
        }
    };
    match args.command {
        Command::CheckE2e => run_check(&args.out, &output.files),
        _ => run_generate(&args.out, &output.files),
    }
}

/// The runtime directory relative to the output directory, both made absolute and normalised
/// (the output directory need not exist yet).
fn runtime_path(out: &Path, runtime: &Path) -> Result<String, String> {
    let runtime = std::fs::canonicalize(runtime)
        .map_err(|e| format!("cannot find the runtime crate {}: {e}", runtime.display()))?;
    let out = absolute(out)?;
    // canonicalize gives verbatim paths on Windows (\\?\C:\...); compare like with like.
    let runtime = absolute(&strip_verbatim(&runtime))?;
    relative_path(&out, &runtime).ok_or_else(|| {
        format!(
            "{} and {} share no root, so the runtime cannot be named by a relative path; give --runtime on the same drive",
            out.display(),
            runtime.display()
        )
    })
}

/// `path` made absolute against the current directory, with `.` and `..` resolved by name.
fn absolute(path: &Path) -> Result<PathBuf, String> {
    let joined = std::env::current_dir()
        .map_err(|e| format!("cannot read the current directory: {e}"))?
        .join(path);
    let mut out = PathBuf::new();
    for part in joined.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

/// A Windows verbatim path (`\\?\C:\x`) as an ordinary one (`C:\x`); other paths unchanged.
fn strip_verbatim(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

fn run_generate(out: &Path, files: &[GeneratedFile]) -> ExitCode {
    match output::write(out, files) {
        Ok(removed) => {
            for path in &removed {
                println!("removed {path}");
            }
            println!("wrote {} files to {}", files.len(), out.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("specc: cannot write {}: {e}", out.display());
            ExitCode::from(1)
        }
    }
}

fn run_check(out: &Path, files: &[GeneratedFile]) -> ExitCode {
    match output::compare(out, files) {
        Ok(differences) if differences.is_empty() => {
            println!("{} is up to date", out.display());
            ExitCode::SUCCESS
        }
        Ok(differences) => {
            for path in &differences.differing {
                println!("differs: {path}");
            }
            for path in &differences.missing {
                println!("missing: {path}");
            }
            for path in &differences.extra {
                println!("extra: {path}");
            }
            println!(
                "{} is not what specc generate would write; run specc generate",
                out.display()
            );
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("specc: cannot read {}: {e}", out.display());
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn crate_name_defaults_to_the_out_directory() {
        let parsed = parse_args(&args(&[
            "generate",
            "--spec",
            "s.jsonc",
            "--out",
            "gen/my-crate",
        ]))
        .unwrap();
        assert_eq!(parsed.command, Command::Generate);
        assert_eq!(parsed.crate_name, "my-crate");
        assert_eq!(parsed.runtime, PathBuf::from(DEFAULT_RUNTIME));
    }

    #[test]
    fn the_runtime_is_named_relative_to_the_output() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let out = workspace.join("target/specc-out/generated");
        let path = runtime_path(&out, Path::new(DEFAULT_RUNTIME)).unwrap();
        assert_eq!(path, "../../../crates/runtime");
    }

    #[test]
    fn usage_errors_are_reported() {
        assert!(parse_args(&args(&[])).is_err());
        assert!(parse_args(&args(&["build"])).is_err());
        assert!(parse_args(&args(&["check", "--spec"])).is_err());
        assert!(parse_args(&args(&["check", "--out", "x"])).is_err());
        assert!(parse_args(&args(&[
            "check", "--spec", "a", "--spec", "b", "--out", "x"
        ]))
        .is_err());
    }
}
