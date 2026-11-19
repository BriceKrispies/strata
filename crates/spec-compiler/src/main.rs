//! The `specc` command.
//!
//! ```text
//! specc generate --spec <file> --out <dir> [--crate-name <name>]
//! specc check    --spec <file> --out <dir> [--crate-name <name>]
//! ```
//!
//! `generate` writes the crate into `<dir>` and removes generated files it no longer writes.
//! `check` exits 1 and lists the paths that differ when `<dir>` is not what `generate` would
//! leave. A specification that is not ready is refused with one `error[<code>] <entity>:
//! <message>` line per diagnostic and exit code 1. Usage errors exit with 2. The crate name
//! defaults to the last component of `<dir>`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use strata_spec_compiler::{generate, output, GeneratedFile, Options};

const USAGE: &str = "usage:
  specc generate --spec <file> --out <dir> [--crate-name <name>]
  specc check    --spec <file> --out <dir> [--crate-name <name>]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Generate,
    Check,
}

struct Args {
    command: Command,
    spec: String,
    out: PathBuf,
    crate_name: String,
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let (command, rest) = match raw.split_first() {
        Some((c, rest)) if c == "generate" => (Command::Generate, rest),
        Some((c, rest)) if c == "check" => (Command::Check, rest),
        Some((c, _)) => return Err(format!("unknown command \"{c}\"")),
        None => return Err("no command given".to_string()),
    };
    let mut spec = None;
    let mut out = None;
    let mut crate_name = None;
    let mut iter = rest.iter();
    while let Some(flag) = iter.next() {
        let slot = match flag.as_str() {
            "--spec" => &mut spec,
            "--out" => &mut out,
            "--crate-name" => &mut crate_name,
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
    let crate_name = match crate_name {
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
    let options = Options {
        crate_name: args.crate_name.clone(),
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
