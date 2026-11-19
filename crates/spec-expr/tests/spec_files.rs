//! Every guard and effect expression of the real specifications must parse.

use serde_json::Value;
use std::path::Path;
use strata_spec_expr::parse;

/// Remove `//` and `/* */` comments and trailing commas, leaving strings untouched.
fn strip_jsonc(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            out.push(c);
            i += 1;
            while i < chars.len() {
                out.push(chars[i]);
                if chars[i] == '\\' && i + 1 < chars.len() {
                    out.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                i += 1;
                if chars[i - 1] == '"' {
                    break;
                }
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else {
            out.push(c);
            i += 1;
        }
    }
    remove_trailing_commas(&out)
}

fn remove_trailing_commas(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let mut in_string = false;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 1;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
            out.push(c);
        } else if c == ',' {
            let next = chars[i + 1..].iter().find(|c| !c.is_whitespace());
            if !matches!(next, Some('}' | ']')) {
                out.push(c);
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// Collect `(where, expression)` pairs: transition guards, `set` values and `emit` payload entries.
fn expressions(v: &Value, at: &str, out: &mut Vec<(String, String)>) {
    match v {
        Value::Object(map) => {
            let id = map.get("id").and_then(Value::as_str).unwrap_or("?");
            let here = format!("{at}/{id}");
            if map.contains_key("from") && map.contains_key("on") {
                if let Some(Value::String(g)) = map.get("guard") {
                    out.push((format!("{here} guard"), g.clone()));
                }
            }
            if map.contains_key("set") {
                if let Some(Value::String(t)) = map.get("to") {
                    out.push((format!("{at} set {}", map["set"]), t.clone()));
                }
            }
            if map.contains_key("emit") {
                if let Some(Value::Object(payload)) = map.get("payload") {
                    for (k, p) in payload {
                        if let Value::String(s) = p {
                            out.push((format!("{at} emit {} payload {k}", map["emit"]), s.clone()));
                        }
                    }
                }
            }
            for (k, child) in map {
                expressions(child, &format!("{here}.{k}"), out);
            }
        }
        Value::Array(items) => {
            for (n, child) in items.iter().enumerate() {
                expressions(child, &format!("{at}[{n}]"), out);
            }
        }
        _ => {}
    }
}

fn check_file(path: &Path) -> usize {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let json: Value = serde_json::from_str(&strip_jsonc(&text))
        .unwrap_or_else(|e| panic!("{} is not JSONC: {e}", path.display()));
    let mut found = Vec::new();
    expressions(&json, "", &mut found);
    let mut failures = Vec::new();
    for (place, src) in &found {
        if let Err(e) = parse(src) {
            failures.push(format!("{place}: `{src}`\n{e}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} expressions in {} do not parse:\n{}",
        failures.len(),
        found.len(),
        path.display(),
        failures.join("\n")
    );
    found.len()
}

#[test]
fn strata_specification_expressions_parse() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../specifications/spec.jsonc");
    let n = check_file(&path);
    assert!(n > 0, "no expressions found: the walker is broken");
}

#[test]
fn exegra_specification_expressions_parse() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../exegra/specs/exegra/spec.jsonc");
    if !path.exists() {
        eprintln!("skipped: {} is absent", path.display());
        return;
    }
    check_file(&path);
}

#[test]
fn jsonc_stripping_respects_strings() {
    let v: Value = serde_json::from_str(&strip_jsonc(
        "// c\n{\"a\": \"x // y, }\", /* z */ \"b\": [1,],}",
    ))
    .unwrap();
    assert_eq!(v["a"], "x // y, }");
    assert_eq!(v["b"][0], 1);
}
