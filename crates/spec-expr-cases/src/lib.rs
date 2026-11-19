//! The shared case table of expression and value semantics (WG-4902), run on the code
//! `strata-spec-expr` generates.
//!
//! `cases.json` is a byte-identical copy of Exegra's `tests/fixtures/expr-cases.json`, which Exegra
//! runs on its simulator's evaluators (the JavaScript evaluator and the Lean core). The build script
//! translates every case to Rust ([`run`]) and to JavaScript ([`JS_SCRIPT`], run with `node` by the
//! tests); `tests/rust_cases.rs` and `tests/js_cases.rs` check every outcome against the table, so
//! the simulator and both targets agree case by case. The rules are `docs/semantics.md`, "Values,
//! numbers and time", in Exegra.

use serde::{Deserialize, Serialize};

/// The case table, as shipped.
pub const TABLE: &str = include_str!("../cases.json");

/// The node script that runs every case's generated JavaScript and prints the outcomes as JSON,
/// `[{value} | {error}]` in table order.
pub const JS_SCRIPT: &str = include_str!(concat!(env!("OUT_DIR"), "/cases.mjs"));

/// The struct type `point` of the table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: i64,
    pub y: i64,
}

/// The error a translated expression returns when checked arithmetic fails or a divisor is zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepError;

/// What a case gave.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The expression's value.
    Value(serde_json::Value),
    /// The step error: overflow or division by zero.
    Overflow,
    /// The environment could not be decoded: a value that is not an option is missing.
    Absent,
    /// The translation refused the expression.
    Refused,
}

#[allow(unused_parens, unused_braces, clippy::all)]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/cases.rs"));
}

pub use generated::{run, CASES};

/// Whether `got` is the outcome `expect` (`{value}` or `{error}` of the table) asks for. Numbers
/// compare by value; `overflow` and `division-by-zero` are both the step error.
pub fn matches(expect: &serde_json::Value, got: &Outcome) -> bool {
    match (
        expect.get("value"),
        expect.get("error").and_then(|e| e.as_str()),
        got,
    ) {
        (Some(v), _, Outcome::Value(g)) => same_value(v, g),
        (_, Some("overflow" | "division-by-zero"), Outcome::Overflow) => true,
        (_, Some("absent"), Outcome::Absent) => true,
        (_, Some("refused"), Outcome::Refused) => true,
        _ => false,
    }
}

/// Structural equality of two JSON values with numbers by value.
pub fn same_value(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use serde_json::Value as J;
    match (a, b) {
        (J::Number(x), J::Number(y)) => x.as_f64() == y.as_f64(),
        (J::Array(x), J::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same_value(p, q))
        }
        (J::Object(x), J::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, p)| y.get(k).is_some_and(|q| same_value(p, q)))
        }
        _ => a == b,
    }
}
