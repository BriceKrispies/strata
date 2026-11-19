//! What the page decides must be what the server decides.
//!
//! - a validation's pattern, range and list of values give the same verdict in the generated
//!   JavaScript as in Rust, over the shared table `tests/fixtures/validation-cases.json`;
//! - a pattern whose meaning differs between the two engines is refused at generation;
//! - two screens whose routes tie are refused at generation.
//!
//! The comparison with the page needs `node` (see `support`).

mod support;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;
use strata_spec_compiler::{rules, web};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn table() -> Value {
    let text =
        fs::read_to_string(manifest_dir().join("tests/fixtures/validation-cases.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn strata_spec() -> String {
    fs::read_to_string(manifest_dir().join("../../specifications/spec.jsonc")).unwrap()
}

/// The server's reading of a range: the value must read as a finite number inside the bounds
/// (the generated Rust refuses `x < min` and `x > max`, and a value that is no number does not
/// decode).
fn server_in_range(value: &Value, min: Option<f64>, max: Option<f64>) -> bool {
    let number = match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    };
    number
        .filter(|n| n.is_finite())
        .is_some_and(|n| min.is_none_or(|m| n >= m) && max.is_none_or(|m| n <= m))
}

/// The server's reading of a list of values: the value, read as the list's kind, is one of them.
fn server_one_of(value: &Value, values: &[Value]) -> bool {
    values.iter().any(|v| match (v, value) {
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Number(a), Value::String(b)) => b.parse::<f64>().ok() == a.as_f64(),
        _ => false,
    })
}

#[test]
fn the_page_and_the_server_decide_a_validation_alike() {
    if !support::node_required("the run of the generated validations") {
        return;
    }
    let table = table();
    let mut script = String::from("import assert from 'node:assert/strict';\n");
    script.push_str(rules::NUMERIC_JS);
    let check = |script: &mut String, fails: &str, case: &Value, server: bool, what: &str| {
        let input = &case[0];
        assert_eq!(
            server,
            case[1].as_bool().unwrap(),
            "the server on {what} {input}"
        );
        let label = serde_json::to_string(&format!("the page on {what} {input}")).unwrap();
        script.push_str(&format!(
            "{{ const value = {input}; assert.equal(!({fails}), {server}, {label}); }}\n"
        ));
    };

    for entry in table["patterns"].as_array().unwrap() {
        let pattern = entry["pattern"].as_str().unwrap();
        assert_eq!(
            rules::to_js(pattern).unwrap(),
            entry["js"].as_str().unwrap(),
            "{pattern}"
        );
        let server = regex::Regex::new(&rules::anchored(pattern)).unwrap();
        let fails = rules::pattern_fails(pattern, "value").unwrap();
        for case in entry["cases"].as_array().unwrap() {
            let matched = server.is_match(case[0].as_str().unwrap());
            check(
                &mut script,
                &fails,
                case,
                matched,
                &format!("pattern {pattern}"),
            );
        }
    }
    for pattern in table["refused"].as_array().unwrap() {
        assert!(
            rules::to_js(pattern.as_str().unwrap()).is_err(),
            "{pattern} is not refused"
        );
    }
    for entry in table["ranges"].as_array().unwrap() {
        let (min, max) = (entry.get("min"), entry.get("max"));
        let fails = rules::range_fails(
            "value",
            min.and_then(Value::as_number),
            max.and_then(Value::as_number),
        )
        .unwrap();
        for case in entry["cases"].as_array().unwrap() {
            let inside = server_in_range(
                &case[0],
                min.and_then(Value::as_f64),
                max.and_then(Value::as_f64),
            );
            check(&mut script, &fails, case, inside, &format!("range {entry}"));
        }
    }
    for entry in table["oneOf"].as_array().unwrap() {
        let values = entry["values"].as_array().unwrap();
        let fails = rules::one_of_fails("value", values).unwrap();
        for case in entry["cases"].as_array().unwrap() {
            check(
                &mut script,
                &fails,
                case,
                server_one_of(&case[0], values),
                &format!("oneOf {entry}"),
            );
        }
    }

    let dir = std::env::temp_dir().join(format!("specc-web-rules-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rules.mjs");
    fs::write(&path, script).unwrap();
    let run = Command::new("node").arg(&path).output().unwrap();
    assert!(
        run.status.success(),
        "the page decides a validation differently from the server:\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn a_pattern_that_the_page_cannot_read_as_the_server_does_is_refused() {
    let text = strata_spec().replace(
        r#""pattern":"[\\s\\S]*\\S[\\s\\S]*""#,
        r#""pattern":"\\bsign\\b""#,
    );
    let diagnostics = web::generate("specifications/spec.jsonc", &text).unwrap_err();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].entity, "val-sign-in-login");
    assert!(
        diagnostics[0].message.contains("same meaning"),
        "{}",
        diagnostics[0].message
    );
}

#[test]
fn two_screens_whose_routes_tie_are_refused() {
    // `/authors/:slug` and `/topics/:id` answer the same paths with one literal segment each.
    let text = strata_spec().replace(r#""route":"/authors/:slug""#, r#""route":"/topics/:id""#);
    let diagnostics = web::generate("specifications/spec.jsonc", &text).unwrap_err();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].entity, "screen-author");
    assert!(
        diagnostics[0].message.contains("screen-topic"),
        "{}",
        diagnostics[0].message
    );

    // A route with more literal segments is the more specific one and is not a tie.
    assert!(web::generate("specifications/spec.jsonc", &strata_spec()).is_ok());
}

#[test]
fn a_record_constraint_pattern_is_matched_against_the_whole_value() {
    // `crates/model` is what specc generates; its record patterns are anchored as a validation's.
    let records = fs::read_to_string(manifest_dir().join("../model/src/records.rs")).unwrap();
    assert!(
        records.contains(r#"Regex::new("\\A(?:^[a-z][a-z0-9_]*$)\\z")"#),
        "a record pattern is not anchored"
    );
    assert!(!records.contains(r#"Regex::new("^[a-z][a-z0-9_]*$")"#));
}
