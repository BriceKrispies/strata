//! The requirement wording lint of specc (`strata_spec_compiler::lint`, WG-5368).
//!
//! `tests/fixtures/lint/<rule>/` holds, for every rule, an accepted specification (no finding at
//! all) and a refused one (exactly one finding, of that rule, on the entity the test names).

use std::fs;
use std::path::PathBuf;

use serde_json::json;
use strata_spec_compiler::lint::{self, Finding};

fn fixture(rule: &str, kind: &str) -> (String, String) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/lint")
        .join(rule)
        .join(format!("{kind}.jsonc"));
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    (path.display().to_string(), text)
}

fn findings(rule: &str, kind: &str) -> Vec<Finding> {
    let (path, text) = fixture(rule, kind);
    lint::lint(&path, &text).unwrap_or_else(|d| panic!("{path} is refused by the loader: {d:?}"))
}

/// Every rule with the entity its refused fixture is expected to be reported on.
const EXPECTED: [(&str, &str); 8] = [
    (lint::ONE_SHALL, "r-1"),
    (lint::BOUNDARY, "r-1"),
    (lint::VAGUE_WORD, "r-1"),
    (lint::UNIVERSAL_METHOD, "r-1"),
    (lint::LIMIT_METHOD, "r-1"),
    (lint::BUDGET_ROW, "r-1"),
    (lint::UNTRACED, "g-2"),
    (lint::MILESTONE, "r-1"),
];

#[test]
fn every_rule_has_a_fixture_pair_and_the_list_is_complete() {
    let mut listed: Vec<&str> = EXPECTED.iter().map(|(rule, _)| *rule).collect();
    listed.sort_unstable();
    let mut all = lint::RULES.to_vec();
    all.sort_unstable();
    assert_eq!(listed, all);
}

#[test]
fn an_accepted_fixture_has_no_finding() {
    for (rule, _) in EXPECTED {
        let name = rule.trim_start_matches("lint.");
        assert_eq!(
            findings(name, "accepted"),
            vec![],
            "accepted fixture of {rule}"
        );
    }
}

#[test]
fn a_refused_fixture_has_exactly_the_finding_of_its_rule() {
    for (rule, entity) in EXPECTED {
        let name = rule.trim_start_matches("lint.");
        let found = findings(name, "refused");
        assert_eq!(found.len(), 1, "refused fixture of {rule}: {found:?}");
        assert_eq!(found[0].rule, rule);
        assert_eq!(found[0].entity, entity);
        assert!(found[0].line > 1, "{rule} names its line: {:?}", found[0]);
        let printed = found[0].to_string();
        assert!(
            printed.starts_with(&format!("error[{rule}] {entity}: ")),
            "{printed}"
        );
    }
}

#[test]
fn a_finding_is_on_the_line_of_its_entity() {
    let (path, text) = fixture("one-shall", "refused");
    let found = lint::lint(&path, &text).unwrap();
    let expected = text
        .lines()
        .position(|l| l.contains("\"id\":\"r-1\""))
        .unwrap()
        + 1;
    assert_eq!(found[0].line, expected);
}

#[test]
fn an_exemption_covers_its_finding_and_an_unmatched_one_is_stale() {
    let found = findings("one-shall", "refused");
    let exemptions = json!({ "lint": [ {
        "reason": "Known (WG-5368).",
        "items": ["lint.one-shall:r-1", "lint.one-shall:gone"]
    } ] });
    let outcome = lint::apply_exemptions(found, &exemptions);
    assert!(outcome.remaining.is_empty());
    assert_eq!(outcome.exempt, 1);
    assert_eq!(outcome.stale, vec!["lint.one-shall:gone".to_string()]);
    assert!(outcome.problems.is_empty());
}

#[test]
fn an_exemption_for_another_rule_does_not_cover_a_finding() {
    let found = findings("one-shall", "refused");
    let exemptions = json!({ "lint": [ { "reason": "r", "items": ["lint.boundary:r-1"] } ] });
    let outcome = lint::apply_exemptions(found, &exemptions);
    assert_eq!(outcome.remaining.len(), 1);
    assert_eq!(outcome.stale, vec!["lint.boundary:r-1".to_string()]);
}

#[test]
fn a_group_without_a_reason_or_an_item_listed_twice_is_a_problem_of_the_list() {
    let exemptions = json!({ "lint": [
        { "reason": " ", "items": ["lint.one-shall:a"] },
        { "reason": "r", "items": ["lint.one-shall:a"] }
    ] });
    let outcome = lint::apply_exemptions(Vec::new(), &exemptions);
    assert_eq!(
        outcome.problems,
        vec![
            "exemption group 0 has no reason".to_string(),
            "exemption lint.one-shall:a is listed twice".to_string()
        ]
    );
}

#[test]
fn a_retired_requirement_is_not_linted_and_does_not_trace() {
    let (path, text) = fixture("one-shall", "refused");
    let retired = text.replace("\"lifecycle\":\"active\"", "\"lifecycle\":\"retired\"");
    let found = lint::lint(&path, &retired).unwrap();
    assert!(found.iter().all(|f| f.rule == lint::UNTRACED), "{found:?}");
}
