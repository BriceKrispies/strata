//! What the differential tests need from the machine they run on: the Exegra CLI and `node`.
//!
//! A missing prerequisite is a failure, never a pass: cargo reports a test that returns early as
//! passed, so a machine without Exegra would otherwise report green for tests that compared
//! nothing. The only way to run without a prerequisite is an explicit opt-out in the
//! environment, which prints a banner on every test it skips:
//!
//! - `SPECC_SKIP_EXEGRA_TESTS=1` skips the tests that compare generated code with Exegra;
//! - `SPECC_SKIP_NODE_TESTS=1` skips the tests that run generated JavaScript.
//!
//! Exegra is found at `$EXEGRA_HOME` (the gate, `scripts/spec-gate.mjs`, passes it), else at the
//! `exegra` directory beside the repository.

#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;

/// The root of the repository the tests run in, from the crate's own path.
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn opted_out(variable: &str, what: &str) -> bool {
    if std::env::var(variable).as_deref() == Ok("1") {
        eprintln!("################################################################");
        eprintln!("# SKIPPED BY {variable}=1: {what} was NOT compared.");
        eprintln!("# This run does not check it. Unset the variable to run it.");
        eprintln!("################################################################");
        true
    } else {
        false
    }
}

/// The Exegra CLI script. Fails the test when there is none, unless `SPECC_SKIP_EXEGRA_TESTS=1`
/// opted out (then `None`, after a banner).
pub fn exegra_cli(what: &str) -> Option<PathBuf> {
    if opted_out("SPECC_SKIP_EXEGRA_TESTS", what) {
        return None;
    }
    let home = match std::env::var_os("EXEGRA_HOME") {
        Some(home) => PathBuf::from(home),
        None => repo_root().join("..").join("exegra"),
    };
    let cli = home.join("cli").join("exegra.js");
    assert!(
        cli.is_file(),
        "{what} needs Exegra, and {} does not exist: set EXEGRA_HOME to an Exegra checkout (one with cli/exegra.js), or set SPECC_SKIP_EXEGRA_TESTS=1 to skip these tests knowingly",
        cli.display()
    );
    Some(cli)
}

/// True when `node` runs. Fails the test when it does not, unless `SPECC_SKIP_NODE_TESTS=1`
/// opted out (then false, after a banner).
pub fn node_required(what: &str) -> bool {
    if opted_out("SPECC_SKIP_NODE_TESTS", what) {
        return false;
    }
    let run = Command::new("node").arg("--version").output();
    assert!(
        run.as_ref().is_ok_and(|o| o.status.success()),
        "{what} needs node on the PATH ({run:?}); install it, or set SPECC_SKIP_NODE_TESTS=1 to skip these tests knowingly"
    );
    true
}

/// A value with every whole number written as an integer, so that numbers compare by value:
/// an `f64` holding `1` equals `1.0`.
pub fn norm(value: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::Number(n) if n.is_f64() => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => Value::from(f as i64),
            _ => Value::Number(n),
        },
        Value::Array(items) => Value::Array(items.into_iter().map(norm).collect()),
        Value::Object(map) => Value::Object(map.into_iter().map(|(k, v)| (k, norm(v))).collect()),
        other => other,
    }
}
