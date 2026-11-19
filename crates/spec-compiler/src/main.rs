//! The `specc` command.
//!
//! ```text
//! specc generate     --spec <file> --out <dir> [--spec-hash <hash>] [--crate-name <name>] [--runtime <dir>] [--core-out <dir> [--core-crate-name <name>]]
//! specc check        --spec <file> --out <dir> [--spec-hash <hash>] [--crate-name <name>] [--runtime <dir>] [--core-out <dir> [--core-crate-name <name>]]
//! specc deploy       --spec <file> --out <dir> [--spec-hash <hash>]
//! specc check-deploy --spec <file> --out <dir> [--spec-hash <hash>]
//! specc e2e          --spec <file> --out <dir> [--spec-hash <hash>] [--origin <url>] [--expect-fail-prefix <id prefix>]
//! specc check-e2e    --spec <file> --out <dir> [--spec-hash <hash>] [--origin <url>] [--expect-fail-prefix <id prefix>]
//! specc scenarios       --spec <file> --out <dir> [--spec-hash <hash>] [--crate-name <name>] [--exegra-project <id>] [--exegra-specs <dir>]
//! specc check-scenarios --spec <file> --out <dir> [--spec-hash <hash>] [--crate-name <name>] [--exegra-project <id>] [--exegra-specs <dir>]
//! specc web          --spec <file> --out <dir> [--spec-hash <hash>]
//! specc check-web    --spec <file> --out <dir> [--spec-hash <hash>]
//! ```
//!
//! `--spec-hash <hash>` is the specification's semantic hash as `exegra validate` reports it
//! (`semanticHash`). It is written into the header of every generated file, so a file generated
//! from another version of the specification differs and `check` and `check-web` fail on it.
//!
//! `web` and `check-web` generate the machines of client components as JavaScript modules for
//! the browser (`strata_spec_compiler::web`), conventionally into `web/src/generated`; `generate`
//! leaves those machines out of the Rust crate.
//!
//! `scenarios` writes `scenarios.rs` into `<dir>` (a generated crate's `tests` directory): one
//! test per scenario that runs only implementation machines, with the expectations Exegra
//! computes for it (see `strata_spec_compiler::scenarios`). It runs `node
//! $EXEGRA_HOME/cli/exegra.js trace`, and is refused (`specc.scenarios-no-exegra`) when
//! `EXEGRA_HOME` is not set. The crate name defaults to the name of `<dir>`'s parent; the Exegra
//! project to the name of the specification's directory and the specs directory to that
//! directory's parent.
//!
//! `e2e` and `check-e2e` write the Playwright journeys (`strata_spec_compiler::e2e`). `--origin`
//! is the origin the journeys' absolute addresses were written against, which the tests move to
//! `STRATA_E2E_BASE_URL` when that is set; journeys whose id starts with `--expect-fail-prefix`
//! are generated as tests that must fail (the fixtures that show each assertion can fail).
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
//! `--core-out` splits the generation in two crates (`strata_spec_compiler::model_core`): the pure
//! crate, written into the `--core-out` directory and named `--core-crate-name` (by default that
//! directory's last component), holds the representation types and every declared type (uuid ids
//! through the uuid crate without its v4 feature); the crate in `--out` depends on it by relative
//! path and re-exports them. `check` then
//! compares both directories.
//!
//! A crate with implementation-level machines depends on `strata-runtime` by path: `--runtime`
//! names the runtime crate's directory, by default `crates/runtime` of the Strata checkout specc
//! was built from, and the manifest holds its path relative to `<dir>`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use strata_spec_compiler::emit::relative_path;
use strata_spec_compiler::{
    deploy, e2e, generate, generate_with_core, model_core, output, scenarios, web, GeneratedFile,
    Options, RuntimeDependency,
};

const USAGE: &str = "usage:
  specc generate     --spec <file> --out <dir> [--spec-hash <hash>] [--crate-name <name>] [--runtime <dir>] [--core-out <dir> [--core-crate-name <name>]]
  specc check        --spec <file> --out <dir> [--spec-hash <hash>] [--crate-name <name>] [--runtime <dir>] [--core-out <dir> [--core-crate-name <name>]]
  specc deploy       --spec <file> --out <dir> [--spec-hash <hash>]
  specc check-deploy --spec <file> --out <dir> [--spec-hash <hash>]
  specc e2e          --spec <file> --out <dir> [--spec-hash <hash>] [--origin <url>] [--expect-fail-prefix <id prefix>]
  specc check-e2e    --spec <file> --out <dir> [--spec-hash <hash>] [--origin <url>] [--expect-fail-prefix <id prefix>]
  specc scenarios       --spec <file> --out <dir> [--spec-hash <hash>] [--crate-name <name>] [--exegra-project <id>] [--exegra-specs <dir>]
  specc check-scenarios --spec <file> --out <dir> [--spec-hash <hash>] [--crate-name <name>] [--exegra-project <id>] [--exegra-specs <dir>]
  specc web          --spec <file> --out <dir> [--spec-hash <hash>]
  specc check-web    --spec <file> --out <dir> [--spec-hash <hash>]";

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
    Scenarios,
    CheckScenarios,
    Web,
    CheckWeb,
}

struct Args {
    command: Command,
    spec: String,
    out: PathBuf,
    crate_name: String,
    runtime: PathBuf,
    /// The specification's semantic hash, written into every header when given.
    spec_hash: Option<String>,
    exegra_project: Option<String>,
    exegra_specs: Option<PathBuf>,
    e2e: e2e::Options,
    /// The pure crate's directory and package name, when the generation is split.
    core: Option<(PathBuf, String)>,
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let (command, rest) = match raw.split_first() {
        Some((c, rest)) if c == "generate" => (Command::Generate, rest),
        Some((c, rest)) if c == "check" => (Command::Check, rest),
        Some((c, rest)) if c == "deploy" => (Command::Deploy, rest),
        Some((c, rest)) if c == "check-deploy" => (Command::CheckDeploy, rest),
        Some((c, rest)) if c == "e2e" => (Command::E2e, rest),
        Some((c, rest)) if c == "check-e2e" => (Command::CheckE2e, rest),
        Some((c, rest)) if c == "scenarios" => (Command::Scenarios, rest),
        Some((c, rest)) if c == "check-scenarios" => (Command::CheckScenarios, rest),
        Some((c, rest)) if c == "web" => (Command::Web, rest),
        Some((c, rest)) if c == "check-web" => (Command::CheckWeb, rest),
        Some((c, _)) => return Err(format!("unknown command \"{c}\"")),
        None => return Err("no command given".to_string()),
    };
    let mut spec = None;
    let mut out = None;
    let mut crate_name = None;
    let mut runtime = None;
    let mut spec_hash = None;
    let mut exegra_project = None;
    let mut exegra_specs = None;
    let mut origin = None;
    let mut expect_fail_prefix = None;
    let mut core_out = None;
    let mut core_crate_name = None;
    let mut iter = rest.iter();
    while let Some(flag) = iter.next() {
        let slot = match flag.as_str() {
            "--spec" => &mut spec,
            "--out" => &mut out,
            "--crate-name" => &mut crate_name,
            "--runtime" => &mut runtime,
            "--spec-hash" => &mut spec_hash,
            "--exegra-project" => &mut exegra_project,
            "--exegra-specs" => &mut exegra_specs,
            "--origin" => &mut origin,
            "--expect-fail-prefix" => &mut expect_fail_prefix,
            "--core-out" => &mut core_out,
            "--core-crate-name" => &mut core_crate_name,
            other => return Err(format!("unknown argument \"{other}\"")),
        };
        let Some(value) = iter.next() else {
            return Err(format!("{flag} needs a value"));
        };
        if slot.replace(value.clone()).is_some() {
            return Err(format!("{flag} is given twice"));
        }
    }
    if let Some(hash) = &spec_hash {
        if hash.is_empty() || !hash.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(
                "--spec-hash must be letters and digits (Exegra's semanticHash)".to_string(),
            );
        }
    }
    let spec = spec.ok_or("--spec is required")?;
    let out = PathBuf::from(out.ok_or("--out is required")?);
    let deploys = matches!(
        command,
        Command::Deploy
            | Command::CheckDeploy
            | Command::E2e
            | Command::CheckE2e
            | Command::Web
            | Command::CheckWeb
    );
    if deploys && crate_name.is_some() {
        return Err("--crate-name does not apply to deploy or e2e".to_string());
    }
    let end_to_end = matches!(command, Command::E2e | Command::CheckE2e);
    if !end_to_end && (origin.is_some() || expect_fail_prefix.is_some()) {
        return Err("--origin and --expect-fail-prefix apply only to e2e".to_string());
    }
    let scenario = matches!(command, Command::Scenarios | Command::CheckScenarios);
    if !scenario && (exegra_project.is_some() || exegra_specs.is_some()) {
        return Err("--exegra-project and --exegra-specs apply only to scenarios".to_string());
    }
    let crate_command = matches!(command, Command::Generate | Command::Check);
    if !crate_command && (core_out.is_some() || core_crate_name.is_some()) {
        return Err(
            "--core-out and --core-crate-name apply only to generate and check".to_string(),
        );
    }
    if core_out.is_none() && core_crate_name.is_some() {
        return Err("--core-crate-name needs --core-out".to_string());
    }
    let core = match core_out {
        None => None,
        Some(dir) => {
            let dir = PathBuf::from(dir);
            let name = match core_crate_name {
                Some(name) => name,
                None => dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .ok_or(
                    "--core-out has no last component to name the crate; give --core-crate-name",
                )?,
            };
            Some((dir, name))
        }
    };
    let crate_name = match crate_name {
        _ if deploys => String::new(),
        Some(name) => name,
        None if scenario => absolute(&out)?
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or("--out has no parent to name the crate; give --crate-name")?,
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
        spec_hash,
        exegra_project,
        exegra_specs: exegra_specs.map(PathBuf::from),
        e2e: e2e::Options {
            origin,
            expect_fail_prefix,
        },
        core,
    })
}

impl Args {
    /// What the headers of generated files name as their source: the specification's path, and
    /// its semantic hash when `--spec-hash` gave one.
    fn header_source(&self) -> String {
        match &self.spec_hash {
            Some(hash) => format!("{} (semantic hash {hash})", self.spec),
            None => self.spec.clone(),
        }
    }
}

/// specc walks specifications recursively (types, expressions, functions); a large specification
/// nests deeper than the main thread's default stack allows in a debug build, so the work runs on a
/// thread with a stack of its own.
const STACK_BYTES: usize = 256 * 1024 * 1024;

fn main() -> ExitCode {
    std::thread::Builder::new()
        .name("specc".into())
        .stack_size(STACK_BYTES)
        .spawn(run)
        .expect("specc: cannot start its worker thread")
        .join()
        .unwrap_or(ExitCode::FAILURE)
}

fn run() -> ExitCode {
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
    if matches!(args.command, Command::Web | Command::CheckWeb) {
        return run_web(&args, &text);
    }
    if matches!(args.command, Command::Scenarios | Command::CheckScenarios) {
        return run_scenarios(&args, &text);
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
    if let Some((core_out, core_name)) = &args.core {
        return run_split(&args, &text, &options, core_out, core_name);
    }
    let files = match generate(&args.header_source(), &text, &options) {
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
        Command::Deploy
        | Command::CheckDeploy
        | Command::E2e
        | Command::CheckE2e
        | Command::Scenarios
        | Command::CheckScenarios
        | Command::Web
        | Command::CheckWeb => ExitCode::from(2),
    }
}

/// `generate` or `check` of a generation split into the model crate and the pure crate.
fn run_split(
    args: &Args,
    text: &str,
    options: &Options,
    core_out: &Path,
    core_name: &str,
) -> ExitCode {
    let path = match absolute(&args.out).and_then(|out| {
        let core = absolute(core_out)?;
        relative_path(&out, &core).ok_or_else(|| {
            format!(
                "{} and {} share no root, so the pure crate cannot be named by a relative path",
                out.display(),
                core.display()
            )
        })
    }) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("specc: {message}");
            return ExitCode::from(1);
        }
    };
    let core = model_core::CoreDependency {
        crate_name: core_name.to_string(),
        path,
    };
    let (model, pure) = match generate_with_core(&args.header_source(), text, options, &core) {
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
    let run = |out: &Path, files: &[GeneratedFile]| match args.command {
        Command::Check => run_check(out, files),
        _ => run_generate(out, files),
    };
    let pure_result = run(core_out, &pure);
    let model_result = run(&args.out, &model);
    let failed: Vec<String> = [(core_out, pure_result), (args.out.as_path(), model_result)]
        .iter()
        .filter(|(_, result)| *result != ExitCode::SUCCESS)
        .map(|(dir, _)| dir.display().to_string())
        .collect();
    if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        // The last line names every crate that failed, for callers that read only that line.
        println!("specc: {} failed", failed.join(" and "));
        ExitCode::from(1)
    }
}

fn run_deploy(args: &Args, text: &str) -> ExitCode {
    let output = match deploy::generate(&args.header_source(), text) {
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

fn run_web(args: &Args, text: &str) -> ExitCode {
    let files = match web::generate(&args.header_source(), text) {
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
        Command::CheckWeb => run_check(&args.out, &files),
        _ => run_generate(&args.out, &files),
    }
}

fn run_e2e(args: &Args, text: &str) -> ExitCode {
    let output = match e2e::generate_with(&args.header_source(), text, &args.e2e) {
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

fn run_scenarios(args: &Args, text: &str) -> ExitCode {
    let spec = absolute(Path::new(&args.spec)).unwrap_or_else(|_| PathBuf::from(&args.spec));
    let project = args.exegra_project.clone().or_else(|| {
        spec.parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
    });
    let specs = args
        .exegra_specs
        .clone()
        .or_else(|| spec.parent().and_then(Path::parent).map(Path::to_path_buf));
    let home = std::env::var_os("EXEGRA_HOME")
        .map(PathBuf::from)
        .filter(|h| h.join("cli").join("exegra.js").is_file());
    let source = match (home, project, specs) {
        (Some(home), Some(project), Some(specs)) => {
            Some(scenarios::exegra_cli(home, project, specs))
        }
        _ => None,
    };
    let result = scenarios::generate(
        &args.header_source(),
        text,
        &args.crate_name,
        source.as_ref().map(|s| s as scenarios::TraceSource),
    );
    let output = match result {
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
        Command::CheckScenarios => run_check(&args.out, &output.files),
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
        assert_eq!(parsed.core, None);
    }

    #[test]
    fn the_core_crate_is_named_after_its_directory() {
        let parsed = parse_args(&args(&[
            "generate",
            "--spec",
            "s.jsonc",
            "--out",
            "crates/model",
            "--core-out",
            "crates/model-core",
        ]))
        .unwrap();
        assert_eq!(
            parsed.core,
            Some((PathBuf::from("crates/model-core"), "model-core".to_string()))
        );
        assert!(parse_args(&args(&[
            "web",
            "--spec",
            "s",
            "--out",
            "o",
            "--core-out",
            "c"
        ]))
        .is_err());
        assert!(parse_args(&args(&[
            "generate",
            "--spec",
            "s",
            "--out",
            "o",
            "--core-crate-name",
            "c"
        ]))
        .is_err());
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
