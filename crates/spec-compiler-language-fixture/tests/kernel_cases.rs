//! The shared case table of the tenant rule language (Exegra's
//! `tests/fixtures/language/kernel-cases.json`, WG-4957, extended by WG-5064), run on the parser,
//! printer, evaluator and checker specc generates. `kernel-cases.json` here is a byte-identical
//! copy of Exegra's file, which Exegra runs on its own evaluator
//! (`tests/core/language-kernel-cases.test.js`), so the two evaluators are held to the same
//! values and the same error texts, including `integer-overflow` at both ends of the range of
//! ±(2^53-1) and `string-too-long` (CMS-1546).
//!
//! For each case: `source` parses to `expr`, `expr` prints as `source`, and evaluating or
//! checking it in the case's environment gives `result`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value as Json;
use strata_spec_compiler_language_fixture::rule_language::{
    check, eval, parse, Env, EvalError, Record, Shape, TypeEnv,
};
use strata_spec_compiler_language_fixture::types::{Expr, Scope, Value};

const TABLE: &str = include_str!("kernel-cases.json");

/// The kernel's field id rule (`is_name` in crates/kernel/src/ids.rs).
fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && s.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn field_id_error(name: &str) -> String {
    format!("invalid field identifier: {name:?}")
}

fn value(json: &Json) -> Value {
    serde_json::from_value(json.clone()).unwrap_or_else(|e| panic!("{json}: {e}"))
}

/// A record of an eval environment: its fields and system attributes by name.
struct Rec(BTreeMap<String, Value>);

impl Record for Rec {
    fn system_attribute(&self, name: &str) -> Value {
        self.0.get(name).cloned().unwrap_or(Value::Null)
    }

    fn field(&self, name: &str) -> Result<Value, String> {
        if !is_name(name) {
            return Err(field_id_error(name));
        }
        Ok(self.0.get(name).cloned().unwrap_or(Value::Null))
    }
}

/// The fixture's `rule-env`: record, old and new (a record's fields or null), param, now,
/// principal and the roles held. There is no lookup.
struct World {
    record: Option<Rec>,
    old: Option<Rec>,
    new: Option<Rec>,
    params: BTreeMap<String, Value>,
    now: i64,
    principal: Option<String>,
    roles: BTreeSet<String>,
}

fn rec(json: &Json) -> Option<Rec> {
    json.as_object()
        .map(|fields| Rec(fields.iter().map(|(k, v)| (k.clone(), value(v))).collect()))
}

fn world(env: &Json) -> World {
    World {
        record: rec(&env["record"]),
        old: rec(&env["old"]),
        new: rec(&env["new"]),
        params: env["param"]
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), value(v))).collect())
            .unwrap_or_default(),
        now: match env.get("now").map(value) {
            Some(Value::Time(t)) => t,
            _ => 0,
        },
        principal: match env.get("principal").map(value) {
            Some(Value::Str(s)) => Some(s),
            _ => None,
        },
        roles: env["has_role"]
            .as_array()
            .map(|r| {
                r.iter()
                    .filter_map(Json::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

impl Env for World {
    fn record(&self, scope: &Scope) -> Option<&dyn Record> {
        let record = match scope {
            Scope::Record => &self.record,
            Scope::Old => &self.old,
            Scope::New => &self.new,
        };
        record.as_ref().map(|r| r as &dyn Record)
    }

    fn follow(
        &self,
        _scope: &Scope,
        field: &str,
        _target: &u64,
    ) -> Result<Option<Box<dyn Record + '_>>, EvalError> {
        Err(EvalError(format!("{field} cannot be followed here")))
    }

    fn param(&self, name: &str) -> Option<Value> {
        self.params.get(name).cloned()
    }

    fn now(&self) -> i64 {
        self.now
    }

    fn principal(&self) -> Option<String> {
        self.principal.clone()
    }

    fn has_role(&self, role: &str) -> bool {
        self.roles.contains(role)
    }
}

/// The fixture's `check-env`: the shape of each field, the parameters and the roles declared.
/// A field of shape ref is a single reference to an object type that exists.
struct Site {
    fields: BTreeMap<String, Shape>,
    params: BTreeSet<String>,
    roles: BTreeSet<String>,
}

fn shape(name: &str) -> Shape {
    match name {
        "bool" => Shape::Bool,
        "int" => Shape::Int,
        "str" => Shape::Str,
        "time" => Shape::Time,
        "ref" => Shape::Ref,
        "list" => Shape::List,
        "any" => Shape::Any,
        other => panic!("unknown shape {other}"),
    }
}

fn strings(json: &Json) -> BTreeSet<String> {
    json.as_array()
        .map(|r| {
            r.iter()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn site(env: &Json) -> Site {
    Site {
        fields: env["fields"]
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), shape(v.as_str().unwrap_or_default())))
                    .collect()
            })
            .unwrap_or_default(),
        params: strings(&env["params"]),
        roles: strings(&env["roles"]),
    }
}

impl Site {
    fn shape_of(&self, name: &str) -> Result<Shape, String> {
        if !is_name(name) {
            return Err(field_id_error(name));
        }
        self.fields
            .get(name)
            .copied()
            .ok_or_else(|| format!("unknown field {name}"))
    }
}

impl TypeEnv for Site {
    fn field_shape(&self, through: Option<&str>, name: &str) -> Result<Shape, String> {
        match through {
            None => self.shape_of(name),
            // The target's fields are not declared here: any field id has the any shape.
            Some(_) if is_name(name) => Ok(Shape::Any),
            Some(_) => Err(field_id_error(name)),
        }
    }

    fn follow(&self, field: &str) -> Result<(), String> {
        if self.shape_of(field)? != Shape::Ref {
            return Err(format!("{field} is not a single reference"));
        }
        Ok(())
    }

    fn has_param(&self, name: &str) -> bool {
        self.params.contains(name)
    }

    fn has_role(&self, role: &str) -> bool {
        self.roles.contains(role)
    }
}

fn shape_name(s: Shape) -> String {
    format!("{s:?}").to_lowercase()
}

/// The expression of a case: `source` parses to it and it prints as `source`.
fn expression(c: &Json, failures: &mut Vec<String>) -> Expr {
    let name = c["name"].as_str().unwrap_or_default();
    let source = c["source"].as_str().unwrap_or_default();
    let expr: Expr = serde_json::from_value(c["expr"].clone())
        .unwrap_or_else(|e| panic!("{name}: the expr does not decode: {e}"));
    match parse(source) {
        Ok(parsed) if parsed == expr => {}
        Ok(parsed) => failures.push(format!("{name}: {source} parses to {parsed:?}")),
        Err(e) => failures.push(format!("{name}: {source} does not parse: {e}")),
    }
    let printed = expr.to_string();
    if printed != source {
        failures.push(format!("{name}: prints as {printed}, not {source}"));
    }
    expr
}

// Verifies: CMS-1546
#[test]
fn the_generated_evaluator_gives_every_eval_result_of_the_shared_table() {
    let table: Json = serde_json::from_str(TABLE).unwrap();
    let mut failures = Vec::new();
    for c in table["eval"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap_or_default();
        let expr = expression(c, &mut failures);
        let got = match eval(&expr, &world(&c["env"])) {
            Ok(v) => serde_json::json!({ "Ok": serde_json::to_value(&v).unwrap() }),
            Err(e) => serde_json::json!({ "Err": e.0 }),
        };
        if got != c["result"] {
            failures.push(format!("{name}: got {got}, the table says {}", c["result"]));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// Verifies: CMS-1545
#[test]
fn the_generated_checker_gives_every_check_result_of_the_shared_table() {
    let table: Json = serde_json::from_str(TABLE).unwrap();
    let mut failures = Vec::new();
    for c in table["check"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap_or_default();
        let expr = expression(c, &mut failures);
        let got = match check(&expr, &site(&c["env"])) {
            Ok(s) => serde_json::json!({ "Ok": shape_name(s) }),
            Err(e) => serde_json::json!({ "Err": e }),
        };
        if got != c["result"] {
            failures.push(format!("{name}: got {got}, the table says {}", c["result"]));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
