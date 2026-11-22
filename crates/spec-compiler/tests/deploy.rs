//! The deployment output: a golden fixture (`tests/fixtures/deploy/expected`, rewritten by
//! `SPECC_BLESS=1`), the exactness of the generated network policies against the declared
//! relationships, and the alert rules read from budget statements.

use std::fs;
use std::path::PathBuf;

use strata_spec_compiler::deploy;
use strata_spec_compiler::{output, GeneratedFile};

const FIXTURE_SPEC: &str = "crates/spec-compiler/tests/fixtures/deploy/spec.jsonc";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_text() -> String {
    fs::read_to_string(manifest_dir().join("tests/fixtures/deploy/spec.jsonc")).unwrap()
}

fn fixture_files() -> Vec<GeneratedFile> {
    deploy::generate(FIXTURE_SPEC, &fixture_text())
        .unwrap()
        .files
}

#[test]
fn golden_output_matches_the_expected_files() {
    let dir = manifest_dir().join("tests/fixtures/deploy/expected");
    let files = fixture_files();
    if std::env::var("SPECC_BLESS").as_deref() == Ok("1") {
        output::write(&dir, &files).unwrap();
        return;
    }
    let differences = output::compare(&dir, &files).unwrap();
    assert!(
        differences.is_empty(),
        "tests/fixtures/deploy/expected is not what specc deploy generates: {differences:?}. \
         Run the test with SPECC_BLESS=1 to rewrite it, and review the change."
    );
}
