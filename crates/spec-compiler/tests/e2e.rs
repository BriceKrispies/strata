//! The end-to-end output: a golden fixture (`tests/fixtures/e2e/expected`, rewritten by
//! `SPECC_BLESS=1`), a syntax check of every generated file with `node --check`, and the
//! refusals. Then the two committed outputs: Strata's own journeys (`tests/e2e/generated`) and the
//! step fixtures (`tests/e2e/step-checks`, from `tests/e2e/step-fixtures/spec.jsonc`), which hold a
//! journey that passes and one that must fail for every step kind; Playwright runs those against
//! the fixture pages of `tests/e2e/step-fixtures/server.mjs`.

use std::fs;
use std::path::PathBuf;

use strata_spec_compiler::e2e;
use strata_spec_compiler::{output, GeneratedFile};

const FIXTURE_SPEC: &str = "crates/spec-compiler/tests/fixtures/e2e/spec.jsonc";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_text() -> String {
    fs::read_to_string(manifest_dir().join("tests/fixtures/e2e/spec.jsonc")).unwrap()
}

fn fixture_files() -> Vec<GeneratedFile> {
    e2e::generate(FIXTURE_SPEC, &fixture_text()).unwrap().files
}

fn contents<'a>(files: &'a [GeneratedFile], path: &str) -> &'a str {
    &files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("no file {path}"))
        .contents
}

#[test]
fn golden_output_matches_the_expected_files() {
    let dir = manifest_dir().join("tests/fixtures/e2e/expected");
    let files = fixture_files();
    if std::env::var("SPECC_BLESS").as_deref() == Ok("1") {
        output::write(&dir, &files).unwrap();
        return;
    }
    let differences = output::compare(&dir, &files).unwrap();
    assert!(
        differences.is_empty(),
        "tests/fixtures/e2e/expected is not what specc e2e generates: {differences:?}. \
         Run the test with SPECC_BLESS=1 to rewrite it, and review the change."
    );
}

fn repo_root() -> PathBuf {
    manifest_dir().join("../..")
}

/// The files `specc e2e` writes from a specification of the repository, with its options.
fn generated(spec: &str, options: &e2e::Options) -> Vec<GeneratedFile> {
    let text = fs::read_to_string(repo_root().join(spec)).unwrap();
    e2e::generate_with(spec, &text, options)
        .unwrap_or_else(|d| panic!("specc e2e refuses {spec}: {d:#?}"))
        .files
}
