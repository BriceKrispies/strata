//! Every case of the shared table on the JavaScript that `strata-spec-expr` generates, run with
//! `node`, gives the outcome the table states, which is the outcome Exegra's simulator gives.
//! Skipped with a message when node is not on PATH.

use serde_json::Value;
use std::process::Command;
use strata_spec_expr_cases::{same_value, JS_SCRIPT, TABLE};

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn generated_javascript_gives_every_outcome_of_the_shared_table() {
    if !node_available() {
        eprintln!("skipped: node is not on PATH");
        return;
    }
    let dir = std::env::temp_dir().join(format!("strata-spec-expr-cases-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("cases.mjs");
    std::fs::write(&file, JS_SCRIPT).unwrap();
    let output = Command::new("node").arg(&file).output().expect("node runs");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        output.status.success(),
        "node failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let got: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    let table: Value = serde_json::from_str(TABLE).unwrap();
    let cases = table["cases"].as_array().unwrap();
    assert_eq!(got.len(), cases.len());
    let mut failures = Vec::new();
    for (c, g) in cases.iter().zip(&got) {
        let expect = &c["expect"];
        let ok = match (
            expect.get("value"),
            expect.get("error").and_then(Value::as_str),
        ) {
            (Some(v), _) => g.get("value").is_some_and(|x| same_value(v, x)),
            (_, Some("overflow" | "division-by-zero")) => g["error"] == "overflow",
            (_, Some(kind)) => g["error"] == kind,
            _ => false,
        };
        if !ok {
            failures.push(format!(
                "{}: `{}` with {} expected {expect}, got {g}",
                c["name"], c["expr"], c["env"]
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
