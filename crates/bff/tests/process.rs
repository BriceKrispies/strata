//! The gateway binary as a process: what it refuses to start without, and that a termination
//! signal drains it and ends it within the stated deadline. Each case runs the real binary with a
//! bare environment.

use std::process::{Command, Output, Stdio};

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

/// Verifies: GWY-1682
///
/// SIGTERM, which is what an orchestrator sends, ends the gateway within its drain deadline even
/// while a client holds a connection open without finishing a request.
#[cfg(unix)]
#[test]
fn a_termination_signal_drains_the_gateway_within_the_deadline() {
    use std::io::Write;
    use std::time::{Duration, Instant};
    let port = {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap().port()
    };
    let addr = format!("127.0.0.1:{port}");
    let mut child = command(&[
        ("STRATA_SERVICE_TOKEN", TOKEN),
        ("STRATA_DEV", "1"),
        ("STRATA_BFF_ADDR", &addr),
        ("STRATA_DRAIN_TIMEOUT_SECS", "2"),
    ])
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
    .unwrap();
    let started = Instant::now();
    let mut held = loop {
        match std::net::TcpStream::connect(&addr) {
            Ok(stream) => break stream,
            Err(_) if started.elapsed() < Duration::from_secs(20) => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Err(error) => panic!("the gateway did not listen: {error}"),
        }
    };
    // A request that never finishes its headers.
    held.write_all(b"GET /api/session HTTP/1.1\r\nhost: 127.0.0.1\r\n")
        .unwrap();
    let terminated = Instant::now();
    let sent = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if terminated.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            panic!("the gateway did not end after SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "{status:?}");
    assert!(
        terminated.elapsed() < Duration::from_secs(8),
        "it took {:?} with a drain deadline of 2 s",
        terminated.elapsed()
    );
}
