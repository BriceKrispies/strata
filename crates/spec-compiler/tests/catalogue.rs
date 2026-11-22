//! The capability catalogue: a golden fixture (`tests/fixtures/catalogue/expected`, rewritten by
//! `SPECC_BLESS=1`), what the fixture's catalogue holds and leaves out, the refusals, and the
//! catalogue of Strata's own specification, which stays within its size budget, lists every public
//! contract and is the committed `specifications/catalogue.md`.

use std::fs;
use std::path::PathBuf;

use strata_spec_compiler::catalogue;
use strata_spec_compiler::{output, GeneratedFile};

const FIXTURE_SPEC: &str = "crates/spec-compiler/tests/fixtures/catalogue/spec.jsonc";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_text() -> String {
    fs::read_to_string(manifest_dir().join("tests/fixtures/catalogue/spec.jsonc")).unwrap()
}

fn fixture_files() -> Vec<GeneratedFile> {
    catalogue::generate(FIXTURE_SPEC, &fixture_text())
        .unwrap()
        .files
}

#[test]
fn golden_output_matches_the_expected_file() {
    let dir = manifest_dir().join("tests/fixtures/catalogue/expected");
    let files = fixture_files();
    if std::env::var("SPECC_BLESS").as_deref() == Ok("1") {
        output::write(&dir, &files).unwrap();
        return;
    }
    let differences = output::compare(&dir, &files).unwrap();
    assert!(
        differences.is_empty(),
        "tests/fixtures/catalogue/expected is not what specc catalogue generates: {differences:?}. \
         Run the test with SPECC_BLESS=1 to rewrite it, and review the change."
    );
}
