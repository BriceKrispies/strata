//! The contract conformance suite (`specc conformance`): the golden of the fixture specification
//! (`crates/bff/tests/conformance/fixtures/generated`, rewritten by `SPECC_BLESS=1`), which the
//! self-test `crates/bff/tests/contract_conformance_fixtures.rs` runs, and the suite of Strata's own
//! specification, which covers every contract that is shipped and names a `bffRoute`.

use std::fs;
use std::path::PathBuf;

use strata_spec_compiler::conformance::{self, Options};
use strata_spec_compiler::output;

const FIXTURE_SPEC: &str = "crates/bff/tests/conformance/fixtures/spec.jsonc";
const FIXTURE_OUT: &str = "crates/bff/tests/conformance/fixtures/generated";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture() -> conformance::ConformanceOutput {
    let text = fs::read_to_string(repo_root().join(FIXTURE_SPEC)).unwrap();
    conformance::generate(
        FIXTURE_SPEC,
        &text,
        &Options {
            expect_fail_prefix: Some("violates-".to_string()),
        },
    )
    .unwrap_or_else(|d| panic!("the fixture specification is refused: {d:?}"))
}

#[test]
fn golden_output_matches_the_expected_file() {
    let dir = repo_root().join(FIXTURE_OUT);
    let files = fixture().files;
    if std::env::var("SPECC_BLESS").as_deref() == Ok("1") {
        output::write(&dir, &files).unwrap();
        return;
    }
    let differences = output::compare(&dir, &files).unwrap();
    assert!(
        differences.is_empty(),
        "{FIXTURE_OUT} is not what specc conformance generates: {differences:?}. Run the test with \
         SPECC_BLESS=1 to rewrite it, and review the change."
    );
}
