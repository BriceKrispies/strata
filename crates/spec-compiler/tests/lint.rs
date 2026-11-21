//! The requirement wording lint of specc (`strata_spec_compiler::lint`, WG-5368).
//!
//! `tests/fixtures/lint/<rule>/` holds, for every rule, an accepted specification (no finding at
//! all) and a refused one (exactly one finding, of that rule, on the entity the test names).

use std::fs;
use std::path::PathBuf;

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
