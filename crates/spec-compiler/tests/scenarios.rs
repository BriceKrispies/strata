//! The scenario tests (`specc scenarios`): a golden over the machines fixture, built from a canned
//! Exegra trace (`tests/fixtures/machines/lease-run.trace.json`, written by
//! `node $EXEGRA_HOME/cli/exegra.js trace lease-run --project machines --specs tests/fixtures
//! --out ...`) so no Exegra is needed here; a check that every expected value in the generated
//! test comes from the trace, and so changes with it; the `Verifies:` lines; and the refusals.
//!
//! The golden, `tests/fixtures/machines/expected-scenarios.rs`, is rewritten by `SPECC_BLESS=1`.
//! It is also the file to copy into the fixture crate's `tests` directory (`specc scenarios
//! --spec ... --out crates/spec-compiler-machines-fixture/tests`), where it runs against the
//! generated lease.

use std::path::PathBuf;

use serde_json::{json, Value};
use strata_spec_compiler::scenarios;
use strata_spec_compiler::Diagnostic;

const SPEC_PATH: &str = "crates/spec-compiler/tests/fixtures/machines/spec.jsonc";
const CRATE: &str = "strata-spec-compiler-machines-fixture";

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/machines")
}

fn spec_text() -> String {
    std::fs::read_to_string(fixtures().join("spec.jsonc")).unwrap()
}

fn canned_trace() -> Value {
    let text = std::fs::read_to_string(fixtures().join("lease-run.trace.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// The text of `scenarios.rs` generated from `spec` and a trace.
fn generated(spec: &str, trace: &Value) -> Result<String, Vec<Diagnostic>> {
    let source = |_: &str| -> Result<Value, String> { Ok(trace.clone()) };
    scenarios::generate(SPEC_PATH, spec, CRATE, Some(&source)).map(|o| {
        assert_eq!(o.files.len(), 1);
        assert_eq!(o.files[0].path, "scenarios.rs");
        o.files[0].contents.clone()
    })
}

fn fixture_output() -> String {
    generated(&spec_text(), &canned_trace()).unwrap()
}

#[test]
fn golden_output_matches_the_expected_file() {
    let path = fixtures().join("expected-scenarios.rs");
    let text = fixture_output();
    if std::env::var("SPECC_BLESS").as_deref() == Ok("1") {
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!("tests/fixtures/machines/expected-scenarios.rs is missing. Run the test with SPECC_BLESS=1 to write it, and review it.")
    });
    assert_eq!(
        expected.replace("\r\n", "\n"),
        text,
        "tests/fixtures/machines/expected-scenarios.rs is not what specc scenarios generates. Run the test with SPECC_BLESS=1 to rewrite it, and review the change."
    );
}
