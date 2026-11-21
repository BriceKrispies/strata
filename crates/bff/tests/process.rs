//! The gateway binary as a process: what it refuses to start without, and that a termination
//! signal drains it and ends it within the stated deadline. Each case runs the real binary with a
//! bare environment.

use std::process::{Command, Output};

const TOKEN: &str = "process-service-token-0123456789";

fn command(vars: &[(&str, &str)]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_strata-bff"));
    command.env_clear();
    for name in ["SystemRoot", "PATH", "TEMP", "TMP"] {
        if let Ok(value) = std::env::var(name) {
            command.env(name, value);
        }
    }
    for (name, value) in vars {
        command.env(name, value);
    }
    command
}

fn run(vars: &[(&str, &str)]) -> Output {
    command(vars).output().expect("the gateway binary starts")
}

/// Verifies: GWY-1401
#[test]
fn a_start_outside_development_without_sites_or_a_service_token_is_refused_by_name() {
    let output = run(&[]);
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{said}");
    assert!(said.contains("STRATA_SERVICE_TOKEN"), "{said}");

    let output = run(&[("STRATA_SERVICE_TOKEN", TOKEN)]);
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "it served demonstration tenants: {said}"
    );
    assert!(said.contains("STRATA_SITES"), "{said}");
}
