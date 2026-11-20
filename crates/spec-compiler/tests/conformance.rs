//! The contract conformance suite (`specc conformance`): the golden of the fixture specification
//! (`crates/bff/tests/conformance/fixtures/generated`, rewritten by `SPECC_BLESS=1`), which the
//! self-test `crates/bff/tests/contract_conformance_fixtures.rs` runs, and the suite of Strata's own
//! specification, which covers every contract that is shipped and names a `bffRoute`.

use std::fs;
use std::path::PathBuf;

use strata_spec_compiler::conformance::{self, Options};
use strata_spec_compiler::{jsonc, output};

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

#[test]
fn every_violating_fixture_must_fail_with_its_rule_and_the_rest_must_pass() {
    let out = fixture();
    let text = &out.files[0].contents;
    let shipped = [
        "conforms-read-note",
        "violates-reply-shape",
        "violates-no-session",
        "violates-declared-error",
        "conforms-write-note",
        "violates-csrf",
        "violates-capability",
        "violates-other-tenant-input",
        "violates-step-up",
        "conforms-delete-note",
        "conforms-ping",
        "violates-quota",
        "violates-read-only",
    ];
    assert_eq!(out.covered, shipped);
    for id in shipped {
        let rule = id.strip_prefix("violates-");
        let test = format!("async fn {}()", id.replace('-', "_"));
        let at = text
            .find(&test)
            .unwrap_or_else(|| panic!("no test for {id}"));
        let before = &text[..at];
        let attributes = &before[before.rfind("#[tokio::test]").unwrap()..];
        match rule {
            Some(rule) => assert!(
                attributes.contains(&format!("#[should_panic(expected = \"[{rule}]\")]")),
                "{id}: {attributes}"
            ),
            None => assert!(!attributes.contains("should_panic"), "{id}: {attributes}"),
        }
    }
    // The contract that is not shipped has no test, and every check can be made.
    assert!(!text.contains("unshipped"));
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
}

/// Every contract of the platform specification that is shipped and names a bffRoute has a test,
/// and the checks the suite cannot make are listed with a reason.
#[test]
fn the_platform_suite_covers_every_shipped_contract_with_a_route() {
    let path = repo_root().join("specifications/spec.jsonc");
    let text = fs::read_to_string(&path).unwrap();
    let out = conformance::generate("specifications/spec.jsonc", &text, &Options::default())
        .unwrap_or_else(|d| panic!("specc conformance refuses the specification: {d:?}"));
    let spec = jsonc::parse(&text).unwrap();
    let shipped: Vec<String> = spec["contracts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["deployment"] == "shipped" && c["bffRoute"].is_string())
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect();
    assert!(!shipped.is_empty());
    assert_eq!(out.covered, shipped);
    for skip in &out.skipped {
        assert!(shipped.contains(&skip.contract));
        assert!(!skip.reason.trim().is_empty());
    }
    // A success check needs an example or no required input; the ones without are counted.
    assert!(out.skipped.iter().any(|s| s.check == "success"));
}
