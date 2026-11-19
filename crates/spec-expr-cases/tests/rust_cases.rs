//! Every case of the shared table on the Rust that `strata-spec-expr` generates gives the outcome
//! the table states, which is the outcome Exegra's simulator gives.

use serde_json::Value;
use strata_spec_expr_cases::{matches, run, CASES, TABLE};

#[test]
fn generated_rust_gives_every_outcome_of_the_shared_table() {
    let table: Value = serde_json::from_str(TABLE).unwrap();
    let cases = table["cases"].as_array().unwrap();
    assert_eq!(cases.len(), CASES);
    let mut failures = Vec::new();
    for (i, c) in cases.iter().enumerate() {
        let got = run(i, c["env"].clone());
        if !matches(&c["expect"], &got) {
            failures.push(format!(
                "{}: `{}` with {} expected {}, got {got:?}",
                c["name"], c["expr"], c["env"], c["expect"]
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
