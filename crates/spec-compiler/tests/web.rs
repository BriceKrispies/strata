//! The web backend: client machines as JavaScript modules.
//!
//! - the golden files `tests/fixtures/machines-web/web/` are what `specc web` generates from the
//!   machines fixture (`SPECC_BLESS=1` rewrites them);
//! - every generated file passes `node --check` (skipped, with a message, without `node`);
//! - the generated `login-form` steps exactly as Exegra runs scenario `login-run`, driven by
//!   `tests/fixtures/machines-web/web-differential.mjs` (skipped without `EXEGRA_HOME` or `node`);
//! - a client machine is not generated as Rust, and a specification without one gives no files.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::{json, Value};
use strata_spec_compiler::{
    generate, jsonc, output, web, GeneratedFile, Options, RuntimeDependency,
};

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

fn node_available() -> bool {
    Command::new("node").arg("--version").output().is_ok()
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
fn generated_files_are_valid_javascript() {
    if !node_available() {
        eprintln!("skipped: node could not be run");
        return;
    }
    let dir = written("check");
    let files = web_files();
    assert!(files.iter().any(|f| f.path == "login-form.js"));
    assert!(files.iter().any(|f| f.path == web::RUNTIME_FILE));
    for file in &files {
        let output = Command::new("node")
            .arg("--check")
            .arg(dir.join(&file.path))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "node --check {}:\n{}",
            file.path,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn a_client_machine_is_not_generated_as_rust() {
    let options = Options {
        crate_name: "out".to_string(),
        runtime: RuntimeDependency::Workspace,
    };
    let files = generate(SPEC, &machines_text(), &options).unwrap();
    // The fixture's only machine runs in the browser, so the crate has no machines module.
    assert!(!files.iter().any(|f| f.path == "src/machines.rs"));
    assert!(!files.iter().any(|f| f.contents.contains("impl Machine")));
}

#[test]
fn an_omitted_option_payload_field_is_null() {
    let files = web_files();
    let js = &files
        .iter()
        .find(|f| f.path == "login-form.js")
        .unwrap()
        .contents;
    // `credentials-sent` has an `option<string>` field `hint` that the machine never gives.
    assert!(js.contains("attempt: next.attempts, hint: null }"), "{js}");
    assert!(js.contains("hint?: string | null"), "{js}");
}

#[test]
fn payload_members_of_an_event_that_crosses_a_contract_follow_its_wire_case() {
    // credentials-sent becomes the request of a contract in camelCase, with a field hint-text.
    let text = machines_text()
        .replace(
            r#"{ "id": "credentials-sent", "name": "Credentials sent", "delivery": "at-least-once","#,
            r#"{ "id": "credentials-sent", "name": "Credentials sent", "delivery": "at-least-once", "contract": "send-credentials", "role": "request","#,
        )
        .replace(
            r#"{ "name": "hint", "type": "option<string>" } ] },"#,
            r#"{ "name": "hint-text", "type": "option<string>" } ] },"#,
        )
        .replace(
            r#"  "events": ["#,
            r#"  "contracts": [ { "id": "send-credentials", "name": "Send credentials", "binding": { "case": "camelCase" },
      "inputs": [ { "name": "name", "type": "string" }, { "name": "attempt", "type": "u32" }, { "name": "hint-text", "type": "option<string>" } ] } ],
  "events": ["#,
        );
    let files = web::generate(SPEC, &text).unwrap();
    let js = &files
        .iter()
        .find(|f| f.path == "login-form.js")
        .unwrap()
        .contents;
    assert!(
        js.contains("attempt: next.attempts, hintText: null }"),
        "{js}"
    );
    assert!(js.contains("hintText?: string | null"), "{js}");
    assert!(!js.contains("hint-text"), "{js}");
}

#[test]
fn a_specification_without_client_machines_gives_no_files() {
    let text = fs::read_to_string(manifest_dir().join("tests/fixtures/full/spec.jsonc")).unwrap();
    assert_eq!(web::generate("spec.jsonc", &text), Ok(Vec::new()));
}

#[test]
fn a_client_machine_refuses_what_it_cannot_express() {
    let text = machines_text().replace(
        r#"{ "id": "accept", "from": "sending", "to": "signed-in", "on": "login-accepted", "otherwise": true,
          "effects": [ { "cancel": "timeout" } ] }"#,
        r#"{ "id": "accept", "from": "sending", "to": "signed-in", "on": "login-accepted", "otherwise": true,
          "effects": [ { "acquire": "slot" } ] }"#,
    );
    let diagnostics = web::generate(SPEC, &text).unwrap_err();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].code, "specc.resource-effect");
    assert_eq!(diagnostics[0].entity, MACHINE);
}

/// The Exegra CLI script, when `EXEGRA_HOME` names a checkout that has one.
fn exegra_cli() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("EXEGRA_HOME")?);
    let cli = home.join("cli").join("exegra.js");
    // EXEGRA_HOME is set, so a missing CLI is a broken setup, not a reason to skip.
    assert!(
        cli.is_file(),
        "EXEGRA_HOME is set but {} does not exist",
        cli.display()
    );
    Some(cli)
}

#[test]
fn the_generated_login_form_steps_as_exegra_runs_it() {
    let Some(cli) = exegra_cli() else {
        eprintln!(
            "skipped: set EXEGRA_HOME to an Exegra checkout (one with cli/exegra.js) to compare the generated login form with Exegra"
        );
        return;
    };
    if !node_available() {
        eprintln!("skipped: node could not be run");
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
