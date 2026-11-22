//! The web backend: client machines as JavaScript modules.
//!
//! - the golden files `tests/fixtures/machines-web/web/` are what `specc web` generates from the
//!   machines fixture (`SPECC_BLESS=1` rewrites them);
//! - every generated file passes `node --check`;
//! - the generated `login-form` steps exactly as Exegra runs scenario `login-run`, driven by
//!   `tests/fixtures/machines-web/web-differential.mjs`;
//! - the generated payload checks refuse what Rust's serde refuses.
//!
//! The tests that need `node` or Exegra fail without them (see `support`); only an explicit
//! `SPECC_SKIP_NODE_TESTS=1` or `SPECC_SKIP_EXEGRA_TESTS=1` skips them, with a banner.
//! - a client machine is not generated as Rust, and a specification without one gives no files.

use crate::support;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::{json, Value};
use strata_spec_compiler::{jsonc, output, web, GeneratedFile};

const SPEC: &str = "crates/spec-compiler/tests/fixtures/machines-web/spec.jsonc";
const MACHINE: &str = "login-form";
const SCENARIO: &str = "login-run";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn machines_text() -> String {
    fs::read_to_string(manifest_dir().join("tests/fixtures/machines-web/spec.jsonc")).unwrap()
}

fn web_files() -> Vec<GeneratedFile> {
    match web::generate(SPEC, &machines_text()) {
        Ok(files) => files,
        Err(diagnostics) => panic!(
            "the fixture was refused:\n{}",
            diagnostics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        ),
    }
}

/// A fresh, empty directory under the system temporary directory.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("specc-web-{name}-{}", std::process::id()));
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// The generated files written to a directory that node reads as ES modules.
fn written(name: &str) -> PathBuf {
    let dir = scratch_dir(name);
    output::write(&dir, &web_files()).unwrap();
    fs::write(dir.join("package.json"), "{ \"type\": \"module\" }\n").unwrap();
    dir
}

#[test]
fn golden_output_matches_the_fixture_directory() {
    let dir = manifest_dir().join("tests/fixtures/machines-web/web");
    let files = web_files();
    if std::env::var("SPECC_BLESS").as_deref() == Ok("1") {
        output::write(&dir, &files).unwrap();
        return;
    }
    let differences = output::compare(&dir, &files).unwrap();
    assert!(
        differences.is_empty(),
        "tests/fixtures/machines-web/web is not what specc web generates from the machines fixture: {differences:?}. \
         Run the test with SPECC_BLESS=1 to rewrite it, and review the change."
    );
}

#[test]
fn the_generated_login_form_steps_as_exegra_runs_it() {
    let what = "the comparison of the generated login form with Exegra";
    let Some(cli) = support::exegra_cli(what) else {
        return;
    };
    if !support::node_required(what) {
        return;
    }
    let specs = manifest_dir().join("tests").join("fixtures");
    let output = Command::new("node")
        .arg(&cli)
        .args(["trace", SCENARIO, "--project", "machines-web", "--specs"])
        .arg(&specs)
        .output()
        .unwrap();
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "the Exegra CLI did not print JSON ({e}):\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert_eq!(
        envelope["ok"],
        json!(true),
        "Exegra refused the fixture: {envelope:#}"
    );

    let dir = written("differential");
    let trace = dir.join("trace.json");
    let spec = dir.join("spec.json");
    fs::write(&trace, envelope["result"].to_string()).unwrap();
    fs::write(&spec, jsonc::parse(&machines_text()).unwrap().to_string()).unwrap();
    let driver = manifest_dir().join("tests/fixtures/machines-web/web-differential.mjs");
    let run = Command::new("node")
        .arg(&driver)
        .arg(&dir)
        .arg(MACHINE)
        .arg(&trace)
        .arg(&spec)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "the generated login form disagrees with Exegra:\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}
