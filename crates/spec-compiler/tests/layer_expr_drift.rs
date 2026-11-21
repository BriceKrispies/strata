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

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use serde_json::{json, Map, Value};
use strata_spec_compiler::language::{Language, Operator};
use strata_spec_compiler::layer_expr::{self, ProductContext, Scope, EXPR_TYPE};
use strata_spec_compiler::load;
use strata_spec_compiler_language_fixture::rule_language as lang;

const PLATFORM: &str = "specifications/spec.jsonc";
const LANGUAGE_FIXTURE: &str = "crates/spec-compiler/tests/fixtures/language/spec.jsonc";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn language_of(spec: &str, id: &str) -> Language {
    let text = fs::read_to_string(root().join(spec)).unwrap();
    let loaded = load::load(spec, &text).unwrap_or_else(|diagnostics| {
        panic!(
            "{spec} did not load:\n{}",
            diagnostics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        )
    });
    loaded
        .languages
        .into_iter()
        .find(|l| l.id == id)
        .unwrap_or_else(|| panic!("{spec} declares no language {id}"))
}

/// The platform's rule language, loaded once for every test in this binary.
fn platform_language() -> &'static Language {
    static LANGUAGE: std::sync::OnceLock<Language> = std::sync::OnceLock::new();
    LANGUAGE.get_or_init(|| language_of(PLATFORM, "tenant-rule-language"))
}

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

/// `Bool`, `Int`, ...: how the checkers name a shape.
fn shape_word(shape: &str) -> String {
    let mut chars = shape.chars();
    let first = chars.next().unwrap().to_ascii_uppercase();
    format!("{first}{}", chars.as_str())
}

fn is_expression_form(form: &str) -> bool {
    matches!(form, "infix" | "keyword-infix" | "prefix-word" | "call")
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

/// The shape the declaration says an operator's result has when its operands are the first accepted
/// shapes: `operand:<n>` is the shape of the n-th operand (counting from 1).
fn declared_result(op: &Operator) -> Option<String> {
    let result = op.result.as_deref()?;
    Some(match result.strip_prefix("operand:") {
        Some(n) => {
            let at: usize = n.parse().unwrap();
            shape_word(&accepted(op, at - 1)[0])
        }
        None => shape_word(result),
    })
}

fn check(text: &str) -> Result<String, (&'static str, String)> {
    layer_expr::shape_name(text, &Scope::default(), &bare_context())
}

/// Every operator the entity declares is understood by `layer_expr.rs`: its syntax, and the shapes
/// its operands and result have.
#[test]
fn layer_expr_handles_every_operator_the_language_declares() {
    let language = platform_language();
    assert!(
        language.operators.len() > 30,
        "the language loaded with {} operators",
        language.operators.len()
    );
    let mut failures = Vec::new();
    for op in &language.operators {
        let name = &op.name;
        let form = op.form.as_deref().unwrap_or("");
        let probed: Result<(String, String, Option<String>), String> = match form {
            "literal" => {
                let token = op.token.as_deref().unwrap_or("");
                let (text, shape) = match token {
                    "null" => ("null", "Any"),
                    "bool" => ("true", "Bool"),
                    "int" => ("1", "Int"),
                    "string" => ("'a'", "Str"),
                    "time" => ("time(1)", "Time"),
                    "ref" => ("ref(1)", "Ref"),
                    "list" => ("[1, 'a']", "List"),
                    other => {
                        failures.push(format!(
                            "{name}: a literal token {other:?} this test and layer_expr.rs do not know"
                        ));
                        continue;
                    }
                };
                Ok((text.to_string(), text.to_string(), Some(shape.to_string())))
            }
            "scope" => {
                let (text, shape) = match name.as_str() {
                    "var" => ("record.f", None),
                    "through" => ("record.f.g", None),
                    "param" => ("param.p", Some("Any")),
                    "now" => ("now", Some("Time")),
                    "principal" => ("principal", Some("Str")),
                    "has-role" => ("has_role(member)", Some("Bool")),
                    other => {
                        failures.push(format!(
                            "{other}: a scope operator this test and layer_expr.rs do not know"
                        ));
                        continue;
                    }
                };
                Ok((
                    text.to_string(),
                    text.to_string(),
                    shape.map(str::to_string),
                ))
            }
            form if is_expression_form(form) => {
                let (text, printed) = probe(op, None);
                Ok((text, printed, declared_result(op)))
            }
            other => Err(format!(
                "{name}: the syntax form {other:?} is one this test and layer_expr.rs do not know"
            )),
        };
        let (text, printed, shape) = match probed {
            Ok(found) => found,
            Err(message) => {
                failures.push(message);
                continue;
            }
        };
        if !is_expression_form(form) {
            if let (Some(have), Some(declared)) = (&shape, &op.result) {
                if *have != shape_word(declared) {
                    failures.push(format!(
                        "{name}: the language declares the result {declared}, this test expects {have}"
                    ));
                }
            }
        }
        match layer_expr::print_text(&text) {
            Ok(got) if got == printed => {}
            other => failures.push(format!(
                "{name}: `{text}` should parse and print as `{printed}`, got {other:?}"
            )),
        }
        if let Some(shape) = shape {
            match check(&text) {
                Ok(got) if got == shape => {}
                other => failures.push(format!(
                    "{name}: `{text}` should check as {shape}, got {other:?}"
                )),
            }
        }
        // An operand of a shape the declaration does not accept is a type error.
        if is_expression_form(form) {
            let positions = if op.operands.len() == 1 && arity(op, form) > 1 {
                1
            } else {
                arity(op, form)
            };
            for at in 0..positions {
                for shape in &language.shapes {
                    let any = language.any_shape.as_deref() == Some(shape.as_str());
                    if any || accepted(op, at).contains(shape) {
                        continue;
                    }
                    let (bad, _) = probe(op, Some((at, shape)));
                    match check(&bad) {
                        Err((EXPR_TYPE, _)) => {}
                        other => failures.push(format!(
                            "{name}: operand {} is declared not to accept {shape}, but `{bad}` gives {other:?}, not a type error",
                            at + 1
                        )),
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The precedence and associativity the entity declares for each infix operator, against every other.
#[test]
fn layer_expr_groups_infix_operators_as_the_language_declares() {
    let language = platform_language();
    let infix: Vec<&Operator> = language
        .operators
        .iter()
        .filter(|o| matches!(o.form.as_deref(), Some("infix" | "keyword-infix")))
        .collect();
    assert!(infix.len() > 10, "{} infix operators", infix.len());
    let mut failures = Vec::new();
    for a in &infix {
        for b in &infix {
            let (ta, tb) = (a.token.as_deref().unwrap(), b.token.as_deref().unwrap());
            let (pa, pb) = (a.precedence.unwrap(), b.precedence.unwrap());
            let text = format!("1 {ta} 1 {tb} 1");
            let tight_right = format!("(1 {ta} (1 {tb} 1))");
            let tight_left = format!("((1 {ta} 1) {tb} 1)");
            let expected: Option<String> = if pa < pb {
                Some(tight_right)
            } else if pa > pb {
                Some(tight_left)
            } else {
                match a.assoc.as_deref().unwrap() {
                    "none" => None,
                    "left" => Some(tight_left),
                    "n-ary" if ta == tb => Some(format!("(1 {ta} 1 {tb} 1)")),
                    other => {
                        failures.push(format!(
                            "{ta} and {tb} share precedence {pa} with associativity {other:?}, which this test does not know"
                        ));
                        continue;
                    }
                }
            };
            let got = layer_expr::print_text(&text).ok();
            if got != expected {
                failures.push(format!(
                    "`{text}` should print as {expected:?}, layer_expr.rs gives {got:?}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The calls `layer_expr.rs` parses are exactly the calls the language declares.
#[test]
fn layer_expr_has_exactly_the_calls_of_the_language() {
    let declared: BTreeSet<String> = platform_language()
        .operators
        .iter()
        .filter(|o| o.form.as_deref() == Some("call"))
        .filter_map(|o| o.token.clone())
        .collect();
    let handled: BTreeSet<String> = layer_expr::call_names()
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(declared, handled);
}

/// The language the generated parser below comes from declares what the platform's language declares:
/// the same operators with the same syntax, operands and results.
#[test]
fn the_language_fixture_declares_the_operators_of_the_platform_language() {
    let platform = platform_language();
    let fixture = language_of(LANGUAGE_FIXTURE, "rule-language");
    let declared = |l: &Language| {
        l.operators
            .iter()
            .map(|o| {
                (
                    o.name.clone(),
                    o.form.clone(),
                    o.token.clone(),
                    o.precedence,
                    o.assoc.clone(),
                    o.operands.clone(),
                    o.result.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(declared(platform), declared(&fixture));
    assert_eq!(platform.shapes, fixture.shapes);
    assert_eq!(platform.any_shape, fixture.any_shape);
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
