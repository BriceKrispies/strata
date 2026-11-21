//! What the backend binary refuses to start without, and says so by name: the service token, the
//! factor key, the tenants directory and the mail transport outside development, and an operator
//! credential that is the service credential. Each case runs the real binary with a bare
//! environment and reads what it says as it exits.

use std::process::{Command, Output};

/// Run the backend with exactly these variables (and the few the operating system needs).
fn run(vars: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_strata-backend"));
    command.env_clear();
    for name in ["SystemRoot", "PATH", "TEMP", "TMP"] {
        if let Ok(value) = std::env::var(name) {
            command.env(name, value);
        }
    }
    for (name, value) in vars {
        command.env(name, value);
    }
    command.output().expect("the backend binary starts")
}

/// The binary exited with a failure and its message names `variable`.
fn refuses_naming(vars: &[(&str, &str)], variable: &str) {
    let output = run(vars);
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "it started: {said}");
    assert!(
        said.contains(variable),
        "it did not name {variable}: {said}"
    );
}

/// Verifies: RUN-1387
#[test]
fn a_start_without_the_service_token_is_refused_by_name() {
    refuses_naming(&[("STRATA_STORE", "memory")], "STRATA_SERVICE_TOKEN");
    refuses_naming(
        &[
            ("STRATA_STORE", "memory"),
            ("STRATA_SERVICE_TOKEN", "short"),
        ],
        "STRATA_SERVICE_TOKEN",
    );
}
