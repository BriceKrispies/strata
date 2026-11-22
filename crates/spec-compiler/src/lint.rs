//! The requirement wording lint: `specc lint --spec <file>`.
//!
//! The lint reads the structured specification (`claims[]` entities of kind `requirement`, plus
//! `goals`, `constraints`, `boundaries` and the budget measurement case table) and reports, one
//! `error[lint.<rule>] <entity id>: <what to change> (<file>:<line>)` line per finding and entity.
//! The rules are the ones of the former requirement lint that apply to a structured requirement
//! (the lint of the spec-driven workflow, which read rendered Markdown); the rules about
//! Markdown, documents, rendering, section placement, code resolution, duplicate wording, ID
//! grammar and the white-paper layout are not ported, because a structured entity cannot have
//! those faults. A requirement whose `lifecycle` is `retired` is not linted, and does not count
//! as tracing a goal or constraint.
//!
//! | Rule | Reports |
//! |---|---|
//! | `lint.one-shall` | a statement with no SHALL/MUST, with more than one, with a semicolon, or with "and" joining two modal clauses |
//! | `lint.boundary` | a requirement that names no boundary, or one the specification does not define |
//! | `lint.vague-word` | a word that is not an outcome: correctly, properly, efficient(ly), robust(ly), appropriate(ly), as expected, user-friendly, fast, secure(ly) |
//! | `lint.universal-method` | "never", "always", "whenever", "for every/all/any/each" or "no ... ever" verified by `example` or `scenario` instead of `property`, `proof` or `conformance` |
//! | `lint.limit-method` | a performance bound (a percentile, a time an operation takes, a rate) in a requirement whose method is not `budget`; an enforced limit, retention period or expiry is shown by an example and is not one |
//! | `lint.budget-row` | a `budget` requirement with no row in the budget measurement case table (silent when the specification has no such table) |
//! | `lint.untraced` | a goal or constraint that no active requirement traces |
//! | `lint.milestone` | a milestone that is not a positive integer, or a seam that is not one or is later than the milestone |
//! | `lint.no-check` | a requirement of a started milestone that declares no `check`: neither the mechanical check that holds it nor `documentation` (only when the started milestones are given, from `gate.json`) |
//! | `lint.check-declaration` | a `check` that is not in the vocabulary, a `checkedBy` that does not fit its check (a test is `<path>::<function>`, a contract ref, an `m-` claim, a budget row), or a `checkedBy` with no `check`; any requirement |
//!
//! Findings are keyed `<rule>:<entity id>`. Any finding fails the lint.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;

use serde_json::Value;

use crate::diagnostic::{Diagnostic, SPEC_ENTITY};
use crate::jsonc;
use crate::lint_text::{find_no_ever, find_performance_bound, find_word, mask_code, tokens};

/// The rule of a statement that is not one SHALL.
pub const ONE_SHALL: &str = "lint.one-shall";
/// The rule of a requirement without a defined boundary.
pub const BOUNDARY: &str = "lint.boundary";
/// The rule of a statement with a word that is not an outcome.
pub const VAGUE_WORD: &str = "lint.vague-word";
/// The rule of a universal claim verified by an example or a scenario.
pub const UNIVERSAL_METHOD: &str = "lint.universal-method";
/// The rule of a limit in a requirement that is not a budget.
pub const LIMIT_METHOD: &str = "lint.limit-method";
/// The rule of a budget without a measurement row.
pub const BUDGET_ROW: &str = "lint.budget-row";
/// The rule of a goal or constraint no requirement traces.
pub const UNTRACED: &str = "lint.untraced";
/// The rule of a missing or inconsistent milestone.
pub const MILESTONE: &str = "lint.milestone";
/// The rule of a requirement of a started milestone that names no check and is not documentation.
pub const NO_CHECK: &str = "lint.no-check";
/// The rule of a check declaration that is not well formed.
pub const CHECK_DECLARATION: &str = "lint.check-declaration";
/// Every rule, in the order they are documented.
pub const RULES: [&str; 10] = [
    ONE_SHALL,
    BOUNDARY,
    VAGUE_WORD,
    UNIVERSAL_METHOD,
    LIMIT_METHOD,
    BUDGET_ROW,
    UNTRACED,
    MILESTONE,
    NO_CHECK,
    CHECK_DECLARATION,
];

/// The checks a requirement may declare (`check`), and `documentation`, which declares none.
pub const CHECKS: [&str; 6] = [
    "test",
    "conformance",
    "differential",
    "model-claim",
    "budget",
    "documentation",
];

/// Words that are not outcomes: a requirement states what a caller can observe.
const VAGUE_WORDS: [&str; 13] = [
    "correctly",
    "properly",
    "efficient",
    "efficiently",
    "robust",
    "robustly",
    "appropriate",
    "appropriately",
    "as expected",
    "user-friendly",
    "fast",
    "secure",
    "securely",
];

/// Phrases of a universal claim, which an example or a scenario cannot show.
const UNIVERSAL: [&str; 7] = [
    "never",
    "always",
    "whenever",
    "for every",
    "for all",
    "for any",
    "for each",
];

/// One finding: a rule broken by one entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The rule, such as `lint.one-shall`.
    pub rule: &'static str,
    /// The id of the requirement, goal or constraint.
    pub entity: String,
    /// The path of the specification file, for printing.
    pub path: String,
    /// The 1-based line of the entity in the file (1 when it cannot be found).
    pub line: usize,
    /// What is wrong and what to change.
    pub message: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "error[{}] {}: {} ({}:{})",
            self.rule, self.entity, self.message, self.path, self.line
        )
    }
}

/// The 1-based line of every entity id, by top-level section, found by reading the text.
fn entity_lines(text: &str) -> HashMap<String, HashMap<String, usize>> {
    let mut out: HashMap<String, HashMap<String, usize>> = HashMap::new();
    let mut section: Option<String> = None;
    for (index, line) in text.lines().enumerate() {
        if let Some(rest) = line.strip_prefix("  \"") {
            if let Some(end) = rest.find('"') {
                if rest[end + 1..].trim_start().starts_with(':') {
                    section = Some(rest[..end].to_string());
                }
            }
        }
        let Some(section) = &section else { continue };
        let Some(at) = line.find("\"id\"") else {
            continue;
        };
        let after = line[at + 4..].trim_start();
        let Some(after) = after.strip_prefix(':') else {
            continue;
        };
        let Some(after) = after.trim_start().strip_prefix('"') else {
            continue;
        };
        if let Some(end) = after.find('"') {
            out.entry(section.clone())
                .or_default()
                .entry(after[..end].to_string())
                .or_insert(index + 1);
        }
    }
    out
}

fn array<'a>(top: &'a Value, key: &str) -> &'a [Value] {
    top.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn str_of<'a>(entity: &'a Value, key: &str) -> Option<&'a str> {
    entity.get(key).and_then(Value::as_str)
}

fn is_active_requirement(claim: &Value) -> bool {
    str_of(claim, "kind") == Some("requirement") && str_of(claim, "lifecycle") != Some("retired")
}

/// The requirement ids a markdown case table lists in its `Requirement` column.
fn table_requirements(body: &str) -> BTreeSet<String> {
    let mut column: Option<usize> = None;
    let mut out = BTreeSet::new();
    for line in body.lines().map(str::trim).filter(|l| l.starts_with('|')) {
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        match column {
            None => {
                column = cells
                    .iter()
                    .position(|c| c.eq_ignore_ascii_case("requirement"));
            }
            Some(i) => {
                if let Some(cell) = cells.get(i) {
                    out.insert(cell.to_uppercase());
                }
            }
        }
    }
    out
}

/// The requirement ids of the budget measurement table, or `None` when the specification has none.
fn budget_rows(top: &Value) -> Option<BTreeSet<String>> {
    let mut found: Option<BTreeSet<String>> = None;
    for table in array(top, "caseTables") {
        let named = str_of(table, "id")
            .into_iter()
            .chain(str_of(table, "path"))
            .any(|s| s.contains("budget-measurement"));
        if named {
            found
                .get_or_insert_with(BTreeSet::new)
                .extend(table_requirements(str_of(table, "body").unwrap_or("")));
        }
    }
    found
}

struct Sink<'a> {
    path: &'a str,
    lines: &'a HashMap<String, HashMap<String, usize>>,
    out: Vec<Finding>,
}

impl Sink<'_> {
    fn add(&mut self, rule: &'static str, section: &str, entity: &str, message: String) {
        let line = self
            .lines
            .get(section)
            .and_then(|m| m.get(entity))
            .copied()
            .unwrap_or(1);
        self.out.push(Finding {
            rule,
            entity: entity.to_string(),
            path: self.path.to_string(),
            line,
            message,
        });
    }
}

fn one_shall(statement: &str) -> Option<String> {
    let s = mask_code(statement);
    let mut reasons: Vec<String> = Vec::new();
    let mut modals = find_word(&s, "shall");
    modals.extend(find_word(&s, "must"));
    if modals.is_empty() {
        reasons.push("it has no SHALL, SHALL NOT or MUST".to_string());
    } else if modals.len() > 1 {
        reasons.push(format!(
            "it has {} modal verbs (SHALL/MUST); split it into one requirement each",
            modals.len()
        ));
    }
    if s.contains(';') {
        reasons.push("a semicolon joins clauses; split them".to_string());
    }
    if modals.len() < 2 {
        let toks = tokens(&s);
        let joined = toks.windows(2).any(|w| {
            w[0].1 == "and" && matches!(w[1].1.as_str(), "never" | "always" | "will" | "should")
        });
        if joined {
            reasons.push("\"and\" joins two modal clauses; split them".to_string());
        }
    }
    (!reasons.is_empty()).then(|| format!("not atomic: {}", reasons.join("; ")))
}

fn vague_words(statement: &str) -> Option<String> {
    let s = mask_code(statement);
    let hits: Vec<&str> = VAGUE_WORDS
        .iter()
        .copied()
        .filter(|w| !find_word(&s, w).is_empty())
        .collect();
    (!hits.is_empty()).then(|| {
        format!(
            "{} not an outcome; state what a caller can observe (a budget for \"fast\", an adversary for \"secure\")",
            hits.iter()
                .map(|w| format!("\"{w}\""))
                .collect::<Vec<_>>()
                .join(", ")
                + if hits.len() == 1 { " is" } else { " are" }
        )
    })
}

fn universal_claim(statement: &str) -> Option<&'static str> {
    let s = mask_code(statement);
    let mut hit: Option<(usize, &'static str)> = None;
    let mut consider = |off: usize, phrase: &'static str| {
        if hit.is_none_or(|h| off < h.0) {
            hit = Some((off, phrase));
        }
    };
    for phrase in UNIVERSAL {
        if let Some(&off) = find_word(&s, phrase).first() {
            consider(off, phrase);
        }
    }
    if let Some(off) = find_no_ever(&s) {
        consider(off, "no ... ever");
    }
    hit.map(|h| h.1)
}

/// The refusal of one `check` and `checkedBy`, or none when they are well formed.
fn check_problem(claim: &Value) -> Option<String> {
    let check = claim.get("check");
    let names = claim.get("checkedBy");
    let Some(check) = check else {
        return names.map(|_| {
            "\"checkedBy\" is given with no \"check\"; name the check (test, conformance, differential, model-claim, budget)".to_string()
        });
    };
    let Some(kind) = check.as_str().filter(|k| CHECKS.contains(k)) else {
        return Some(format!(
            "\"check\": {check} is not one of {}",
            CHECKS.join(", ")
        ));
    };
    let names: Vec<&str> = match names {
        None => Vec::new(),
        Some(Value::Array(items)) => {
            let strings: Vec<&str> = items.iter().filter_map(Value::as_str).collect();
            if strings.len() != items.len() {
                return Some("\"checkedBy\" must list strings".to_string());
            }
            strings
        }
        Some(_) => return Some("\"checkedBy\" must be a list of strings".to_string()),
    };
    if kind == "documentation" {
        return (!names.is_empty()).then(|| {
            "a documentation statement is held by no check; remove \"checkedBy\", or name the check".to_string()
        });
    }
    if names.is_empty() {
        return Some(format!(
            "\"check\": \"{kind}\" names nothing; list in \"checkedBy\" what holds the requirement"
        ));
    }
    let bad = names.iter().find(|name| !name_fits(kind, name))?;
    Some(format!(
        "\"{bad}\" is not a name a {kind} check takes ({})",
        match kind {
            "test" | "differential" => "<path from the repository root>::<test function>",
            "conformance" => "a contract ref such as CT-004",
            "model-claim" => "the id of an m- claim",
            _ => "a ref of the budget measurement table",
        }
    ))
}

fn name_fits(kind: &str, name: &str) -> bool {
    match kind {
        "test" | "differential" => name.split_once("::").is_some_and(|(path, test)| {
            path.contains('/') && path.contains('.') && !test.is_empty()
        }),
        "conformance" => name.split_once('-').is_some_and(|(tag, number)| {
            !tag.is_empty()
                && tag.bytes().all(|b| b.is_ascii_uppercase())
                && !number.is_empty()
                && number.bytes().all(|b| b.is_ascii_digit())
        }),
        "model-claim" => name.starts_with("m-") && name.len() > 2,
        _ => !name.trim().is_empty(),
    }
}

fn check_declaration(claim: &Value, id: &str, started: bool, sink: &mut Sink<'_>) {
    if let Some(message) = check_problem(claim) {
        sink.add(CHECK_DECLARATION, "claims", id, message);
    } else if started && claim.get("check").is_none() {
        sink.add(
            NO_CHECK,
            "claims",
            id,
            "the requirement is of a started milestone and names no \"check\"; state the mechanical check that holds it (test, conformance, differential, model-claim or budget, with \"checkedBy\"), or mark the statement \"check\": \"documentation\" if no check can hold it".to_string(),
        );
    }
}

fn positive(value: &Value) -> Option<u64> {
    value.as_u64().filter(|n| *n >= 1)
}

fn milestone(claim: &Value, id: &str, sink: &mut Sink<'_>) {
    let milestone = claim.get("milestone");
    let parsed = milestone.and_then(positive);
    match milestone {
        None => sink.add(
            MILESTONE,
            "claims",
            id,
            "the requirement has no \"milestone\"; give the milestone at which it is first verified (a positive integer)".to_string(),
        ),
        Some(v) if parsed.is_none() => sink.add(
            MILESTONE,
            "claims",
            id,
            format!("\"milestone\": {v} is not a positive integer; change it to the milestone number"),
        ),
        Some(_) => {}
    }
    if let Some(seam) = claim.get("seam") {
        match positive(seam) {
            None => sink.add(
                MILESTONE,
                "claims",
                id,
                format!(
                    "\"seam\": {seam} is not a positive integer; change it to a milestone number"
                ),
            ),
            Some(n) => {
                if parsed.is_some_and(|m| n > m) {
                    sink.add(
                        MILESTONE,
                        "claims",
                        id,
                        format!(
                            "\"seam\": {n} is later than \"milestone\": {}; the seam is the milestone by which the contract must exist, so it cannot be later than the first verification",
                            parsed.unwrap_or_default()
                        ),
                    );
                }
            }
        }
    }
}

/// Applies every rule to the specification in `text`, named `path` in the findings. A text that is
/// not JSONC is refused with the diagnostic of the loader.
pub fn lint(path: &str, text: &str) -> Result<Vec<Finding>, Vec<Diagnostic>> {
    lint_started(path, text, &BTreeSet::new())
}

/// [`lint`], and `lint.no-check` for the requirements of the milestones in `started` (the
/// `startedMilestones` of `gate.json`). With no started milestone that rule has nothing to apply to.
pub fn lint_started(
    path: &str,
    text: &str,
    started: &BTreeSet<u64>,
) -> Result<Vec<Finding>, Vec<Diagnostic>> {
    let top = jsonc::parse(text).map_err(|e| {
        vec![Diagnostic::new(
            e.code(crate::load::SYNTAX),
            SPEC_ENTITY,
            format!("the specification is not valid JSONC: {e}"),
        )]
    })?;
    let lines = entity_lines(text);
    let mut sink = Sink {
        path,
        lines: &lines,
        out: Vec::new(),
    };
    let boundaries: HashSet<&str> = array(&top, "boundaries")
        .iter()
        .filter_map(|b| str_of(b, "id"))
        .collect();
    let budget_rows = budget_rows(&top);
    let mut traced: HashSet<String> = HashSet::new();
    for claim in array(&top, "claims")
        .iter()
        .filter(|c| is_active_requirement(c))
    {
        let Some(id) = str_of(claim, "id") else {
            continue;
        };
        let statement = str_of(claim, "statement").unwrap_or("");
        let method = claim
            .get("evaluate")
            .and_then(|e| str_of(e, "method"))
            .unwrap_or("");
        for t in claim
            .get("traces")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            traced.insert(t.to_lowercase());
        }
        if let Some(message) = one_shall(statement) {
            sink.add(ONE_SHALL, "claims", id, message);
        }
        match str_of(claim, "boundary").filter(|b| !b.is_empty()) {
            None => sink.add(
                BOUNDARY,
                "claims",
                id,
                "the requirement names no \"boundary\"; state it at the boundary a caller can see (a boundary id)".to_string(),
            ),
            Some(b) if !boundaries.contains(b) => sink.add(
                BOUNDARY,
                "claims",
                id,
                format!("\"boundary\": \"{b}\" is not a boundary of the specification; name a defined boundary"),
            ),
            Some(_) => {}
        }
        if let Some(message) = vague_words(statement) {
            sink.add(VAGUE_WORD, "claims", id, message);
        }
        if matches!(method, "example" | "scenario") {
            if let Some(phrase) = universal_claim(statement) {
                sink.add(
                    UNIVERSAL_METHOD,
                    "claims",
                    id,
                    format!(
                        "\"{phrase}\" cannot be shown by {method}; change the method to property, proof or conformance, or reword the claim"
                    ),
                );
            }
        }
        if method != "budget" {
            if let Some((_, phrase)) = find_performance_bound(&mask_code(statement)) {
                sink.add(
                    LIMIT_METHOD,
                    "claims",
                    id,
                    format!(
                        "the performance bound \"{phrase}\" needs method budget, not {}; change the method and add a budget measurement row",
                        if method.is_empty() { "none" } else { method }
                    ),
                );
            }
        }
        if method == "budget" {
            if let Some(rows) = &budget_rows {
                let key = str_of(claim, "ref").unwrap_or(id).to_uppercase();
                if !rows.contains(&key) && !rows.contains(&id.to_uppercase()) {
                    sink.add(
                        BUDGET_ROW,
                        "claims",
                        id,
                        format!("no row of the budget measurement table names {key}; add a row saying how the bound is measured"),
                    );
                }
            }
        }
        milestone(claim, id, &mut sink);
        let is_started = claim
            .get("milestone")
            .and_then(positive)
            .is_some_and(|m| started.contains(&m));
        check_declaration(claim, id, is_started, &mut sink);
    }
    for (section, what) in [("goals", "goal"), ("constraints", "constraint")] {
        for entity in array(&top, section) {
            let Some(id) = str_of(entity, "id") else {
                continue;
            };
            if !traced.contains(&id.to_lowercase()) {
                sink.add(
                    UNTRACED,
                    section,
                    id,
                    format!("the {what} has no requirement tracing to it; add it to the traces of the requirement that demonstrates it, or write that requirement"),
                );
            }
        }
    }
    Ok(sink.out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(rule: &'static str, entity: &str) -> Finding {
        Finding {
            rule,
            entity: entity.to_string(),
            path: "s.jsonc".to_string(),
            line: 3,
            message: "m".to_string(),
        }
    }

    #[test]
    fn a_finding_prints_its_rule_entity_line_and_message() {
        assert_eq!(
            finding(ONE_SHALL, "a-1").to_string(),
            "error[lint.one-shall] a-1: m (s.jsonc:3)"
        );
    }

    #[test]
    fn lines_are_read_from_the_section_of_the_entity() {
        let text = "{\n  \"goals\": [\n    {\"id\":\"g-1\"}\n  ],\n  \"claims\": [\n    {\"id\":\"g-1\"}\n  ]\n}";
        let lines = entity_lines(text);
        assert_eq!(lines["goals"]["g-1"], 3);
        assert_eq!(lines["claims"]["g-1"], 6);
    }
}
