//! `layer_expr.rs` against the tenant rule language it duplicates (WG-5060).
//!
//! specc checks the expressions of a product file with its own parser and checker
//! (`strata_spec_compiler::layer_expr`), because it may not depend on `strata-model-core`, where the
//! language is generated. Two copies of one language drift, so each way they can drift is a failing
//! test here:
//!
//! - an operator, a call or a syntax form added to the `tenant-rule-language` entity that
//!   `layer_expr.rs` does not handle: every operator the entity declares is probed against the parser
//!   (form, token, precedence, associativity) and the static check (accepted shapes, result);
//! - the entity's operators and the language fixture's (the language the generated parser below was
//!   generated from) being different, which would make the comparison below prove nothing;
//! - a text of the language that the generated parser and checker treat differently from
//!   `layer_expr.rs`: a corpus is parsed and checked by both, and the printed result, the shape and
//!   the text of each type error are compared.
//!
//! The probes are built from the declaration alone. An operator of a form this test does not know
//! fails it, so a new form cannot pass unprobed.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use strata_spec_compiler::language::Operator;
use strata_spec_compiler::layer_expr::{self, ProductContext, Scope, EXPR_TYPE};
use strata_spec_compiler_language_fixture::rule_language as lang;

/// The context probes are checked in: no object type, the role `member`.
fn bare_context() -> ProductContext {
    let ids: BTreeMap<String, String> = [("member".to_string(), "roles".to_string())].into();
    ProductContext::of(&ids, &Map::new())
}

/// An expression of each shape the language names.
fn sample(shape: &str) -> &'static str {
    match shape {
        "bool" => "true",
        "int" => "1",
        "str" => "'a'",
        "time" => "time(1)",
        "ref" => "ref(1)",
        "list" => "[1]",
        "any" => "null",
        other => panic!(
            "the language names a shape {other}, which this test has no sample expression of: add one"
        ),
    }
}

/// How many expression operands the probe of an operator has: a n-ary junction is probed with two.
fn arity(op: &Operator, form: &str) -> usize {
    match form {
        "infix" | "keyword-infix" => 2,
        "prefix-word" => 1,
        _ => op.operands.len(),
    }
}

/// The shapes operand `at` accepts; a n-ary operator declares one list for all its operands.
fn accepted(op: &Operator, at: usize) -> &[String] {
    let last = op
        .operands
        .len()
        .checked_sub(1)
        .unwrap_or_else(|| panic!("operator {} declares no operand", op.name));
    &op.operands[at.min(last)]
}

/// The text of a probe of `op`, and the text the printer is expected to give back for it. `replaced`
/// puts a sample of another shape at one operand.
fn probe(op: &Operator, replaced: Option<(usize, &str)>) -> (String, String) {
    let form = op.form.as_deref().unwrap();
    let token = op
        .token
        .as_deref()
        .unwrap_or_else(|| panic!("operator {} has the form {form} and no token", op.name));
    let args: Vec<&str> = (0..arity(op, form))
        .map(|i| match replaced {
            Some((at, shape)) if at == i => sample(shape),
            _ => sample(&accepted(op, i)[0]),
        })
        .collect();
    match form {
        "infix" | "keyword-infix" => (
            format!("{} {token} {}", args[0], args[1]),
            format!("({} {token} {})", args[0], args[1]),
        ),
        "prefix-word" => (
            format!("{token} {}", args[0]),
            format!("{token}({})", args[0]),
        ),
        _ => {
            let text = format!("{token}({})", args.join(", "));
            (text.clone(), text)
        }
    }
}

fn check(text: &str) -> Result<String, (&'static str, String)> {
    layer_expr::shape_name(text, &Scope::default(), &bare_context())
}

// ---------------------------------------------------------------------------------------------
// The corpus: the generated parser and checker against layer_expr.rs
// ---------------------------------------------------------------------------------------------

/// The fields of the object type `tier` and of `member`, the target of its reference `owner`.
const TIER: &[(&str, &str)] = &[
    ("name", "Str"),
    ("points", "Int"),
    ("at", "Time"),
    ("owner", "Ref"),
    ("tags", "List"),
    ("ok", "Bool"),
];
const MEMBER: &[(&str, &str)] = &[("email", "Str"), ("joined", "Time")];

fn object_json(id: &str, fields: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (name, ty) in fields {
        let ty = match *ty {
            "Ref" => json!({ "Ref": { "target": "member", "on-delete": "Restrict" } }),
            "List" => json!({ "List": "Str" }),
            other => json!(other),
        };
        map.insert((*name).to_string(), json!({ "ty": ty }));
    }
    json!({ "id": id, "fields": map })
}

/// The definition site of the generated check: the object type `tier` and the parameters `a` and `b`.
struct World;

impl World {
    fn reference(field: &str) -> Result<(), String> {
        match TIER.iter().find(|(name, _)| *name == field) {
            None => Err(format!("unknown field {field}")),
            Some((_, "Ref")) => Ok(()),
            Some(_) => Err(format!("{field} is not a single reference")),
        }
    }
}

impl lang::TypeEnv for World {
    fn field_shape(&self, through: Option<&str>, name: &str) -> Result<lang::Shape, String> {
        let table = match through {
            None => TIER,
            Some(field) => {
                World::reference(field)?;
                MEMBER
            }
        };
        match table.iter().find(|(field, _)| *field == name) {
            None => Err(format!("unknown field {name}")),
            Some((_, ty)) => Ok(match *ty {
                "Str" => lang::Shape::Str,
                "Int" => lang::Shape::Int,
                "Time" => lang::Shape::Time,
                "Ref" => lang::Shape::Ref,
                "List" => lang::Shape::List,
                "Bool" => lang::Shape::Bool,
                other => panic!("a field type {other}"),
            }),
        }
    }

    fn follow(&self, field: &str) -> Result<(), String> {
        World::reference(field)
    }

    fn has_param(&self, name: &str) -> bool {
        ["a", "b"].contains(&name)
    }

    fn has_role(&self, role: &str) -> bool {
        ["member", "boss"].contains(&role)
    }
}

/// Texts of the language: what both parse and what both refuse, at the syntax and at the check.
const CORPUS: &[&str] = &[
    // The language before the arithmetic operators.
    "record.name == 'gold' and record.points >= 10",
    "not record.points < 3 or true",
    "record.owner.email == 'a'",
    "record.$id == ref(3)",
    "now > time(5)",
    "principal == record.$created_by",
    "has_role(member) or has_role(boss)",
    "has_role(ghost)",
    "contains(record.tags, 'x')",
    "contains(record.name, 'x') and starts_with(lower(record.name), 'g')",
    "record.name in ['a', 'b']",
    "if(record.points > 1, 1, 2) == 1",
    "is_null(record.owner)",
    "is_slug(record.name) and len(record.tags) > 0",
    "record.points == null",
    "(true)",
    "param.a > 1",
    "param.c > 1",
    "record.nothing > 1",
    "record.owner.phone == null",
    "record.name.email == 'x'",
    "record.tags.email == 'x'",
    "old.points < new.points",
    "record.name == 1",
    "record.name > 1",
    "record.points and true",
    "not record.points",
    "len(record.points) > 0",
    "lower(record.points) == 'a'",
    "record.points in record.points",
    "if(record.points, 1, 2) == 1",
    "if(true, 1, 'a') == 1",
    "contains(record.name, 1)",
    "starts_with(1, 'a')",
    "[1, 'a'] == [1]",
    "[time(1), time(2)] == [time(1)]",
    "[]",
    // Literals and escapes.
    "'a\\nb' == record.name",
    "'it\\'s' == record.name",
    "'back\\\\slash' == record.name",
    "-9223372036854775808 < 0",
    "9223372036854775808 < 0",
    "time(-5) < now",
    "ref(3) == record.$id",
    "ref(-1) == record.$id",
    // Arithmetic, time arithmetic, durations, concat and text (D-6543).
    "record.points + 1 == 3",
    "record.points - 1 > 0",
    "record.points * 2 + 1 == 3",
    "(record.points + 1) * 2 >= 4",
    "1 + 2 * 3 == 7",
    "1 * 2 + 3 == 5",
    "1 - 2 - 3 == -4",
    "record.at + days(1) > now",
    "record.at - hours(2) < now",
    "now + minutes(30) <= record.at",
    "now + 1 > now",
    "ms_between(record.at, now) >= minutes(5)",
    "ms_between(now, now + days(90)) <= days(90)",
    "new.at > old.at and new.at <= old.at + days(90)",
    "concat('a', text(record.points)) == 'a1'",
    "concat(lower(record.name), 'x') == 'x'",
    "text(now) == 'x'",
    "text(record.owner) == 'x'",
    "text(record.$id) == 'x'",
    "text(true) == 'x'",
    "text('s') == 's'",
    "text(param.a) == 'x'",
    "text(record.tags) == 'x'",
    "param.a + 1 > 0",
    "param.a * param.b == 6",
    "now + param.a > now",
    "record.points + record.at > now",
    "now + now > now",
    "now - now > now",
    "'a' + 1 == 1",
    "record.points + 'a' == 1",
    "true - 1 == 1",
    "now * 2 == 1",
    "record.points * now == 1",
    "ms_between(1, now) > 0",
    "ms_between(now, 'a') > 0",
    "days('a') > 0",
    "hours(now) > 0",
    "minutes(true) > 0",
    "concat('a', 1) == 'a'",
    "concat(1, 'a') == 'a'",
    "now + 1 > 5",
    "record.points + 1 == 'a'",
    "record.points + 1",
    "now + days(1)",
    // The sign rule: a `-` is a sign only where no operand ends before it.
    "record.points-1 == 2",
    "record.points -1 == 2",
    "record.points - -1 == 2",
    "-1 + 2 == 1",
    "1 -1 == 0",
    "(1) -1 == 0",
    "2*-3 == -6",
    "1 in [-1, 2]",
    "contains(record.name, -1)",
    "days(-1) < 0",
    "not -1",
    "[1 -1]",
    "[1, -1]",
    "- 1",
    "--1",
    "1 - - 1",
    "not -1 == 1 -",
    // Syntax errors.
    "",
    "1 +",
    "+ 1",
    "* 2",
    "1 * * 2",
    "(1",
    "1)",
    "not",
    "days()",
    "days(1, 2)",
    "text()",
    "ms_between(now)",
    "concat('a')",
    "balance(1)",
    "'unterminated",
    "'bad\\q'",
    "\"double\"",
    "record.points >",
    "1 < 2 < 3",
    "1 == 2 == 3",
    "1 == 2 in [3]",
    "1 + 2 == 3 == 4",
    "time(1 + 2)",
    "time()",
    "has_role()",
    "1 2",
    "param.a.b",
    "record.owner.email.x",
    "[record.points]",
    "[1 + 2]",
];

/// Both parsers print the same tree for the same text, or both refuse it; both checkers give the same
/// shape, or both refuse it with the same words when layer_expr.rs calls the refusal a type error.
#[test]
fn layer_expr_and_the_generated_language_read_a_corpus_alike() {
    let top = json!({
        "objects": [object_json("tier", TIER), object_json("member", MEMBER)],
        "roles": [{ "id": "boss" }]
    });
    let ids: BTreeMap<String, String> = [("member".to_string(), "roles".to_string())].into();
    let ctx = ProductContext::of(&ids, top.as_object().unwrap());
    let scope = ctx.scope_of(
        "operations",
        &json!({
            "inputs": [{ "name": "a" }, { "name": "b" }],
            "subject": { "object": "tier", "input": "a" }
        }),
    );
    let mut failures = Vec::new();
    let (mut parsed, mut refused) = (0, 0);
    for text in CORPUS {
        let generated = lang::parse(text);
        let ours = layer_expr::print_text(text);
        let expr = match (&generated, &ours) {
            (Ok(expr), Ok(printed)) => {
                if &expr.to_string() != printed {
                    failures.push(format!(
                        "`{text}` prints as {expr} for the generated parser and {printed} for layer_expr.rs"
                    ));
                }
                parsed += 1;
                expr
            }
            (Err(_), Err(_)) => {
                refused += 1;
                continue;
            }
            (Ok(expr), Err(message)) => {
                failures.push(format!(
                    "`{text}` parses for the generated parser (as {expr}) and layer_expr.rs refuses it: {message}"
                ));
                continue;
            }
            (Err(message), Ok(printed)) => {
                failures.push(format!(
                    "`{text}` is refused by the generated parser ({message}) and layer_expr.rs reads it as {printed}"
                ));
                continue;
            }
        };
        let generated = lang::check(expr, &World).map(|shape| format!("{shape:?}"));
        let ours = layer_expr::shape_name(text, &scope, &ctx);
        match (&generated, &ours) {
            (Ok(a), Ok(b)) if a == b => {}
            (Err(_), Err((code, _))) if *code != EXPR_TYPE => {}
            (Err(a), Err((_, b))) if a == b => {}
            _ => failures.push(format!(
                "`{text}` checks as {generated:?} for the generated checker and {ours:?} for layer_expr.rs"
            )),
        }
    }
    assert!(
        parsed > 60 && refused > 15,
        "{parsed} parsed, {refused} refused"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
