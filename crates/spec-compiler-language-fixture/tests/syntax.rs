//! The generated parser and printer: the substrate syntax tests (crates/substrate/src/syntax.rs),
//! ported to the parser and printer specc generates from the rule language.

use strata_spec_compiler_language_fixture::rule_language::parse;
use strata_spec_compiler_language_fixture::types::{CmpOp, Expr, Scope, Value};

#[test]
fn parses_precedence_and_calls() {
    let expr =
        parse("len(new.title) <= 120 and not is_null(new.slug) or has_role(editor)").unwrap();
    assert_eq!(
        expr.to_string(),
        "(((len(new.title) <= 120) and not(is_null(new.slug))) or has_role(editor))"
    );
}

#[test]
fn printed_expressions_parse_back_to_themselves() {
    for source in [
        "record.status == 'published'",
        "new.kind in ['a', 'b\\'c', 'd\\\\e']",
        "if(is_null(old.rank), 0, old.rank) < -3",
        "contains(lower(record.title), lower(param.q)) or starts_with(record.cover, '/media/')",
        "record.$created_by == principal and now >= record.published_at",
        "new.when == time(1700000000000) and new.who != ref(7) and new.flag == true and is_null(null)",
        "is_slug(new.slug)",
    ] {
        let parsed = parse(source).unwrap();
        assert_eq!(parse(&parsed.to_string()).unwrap(), parsed, "{source}");
    }
}

#[test]
fn rejects_what_it_does_not_understand() {
    assert!(parse("record.title ==").is_err());
    assert!(parse("drop_table(record.x)").is_err());
    assert!(parse("'open").is_err());
    assert!(parse("record.a == 1 2").is_err());
    assert!(parse("[record.a]").is_err());
    // A comparison is non-associative, a record id is not negative.
    assert!(parse("record.a == 1 == 2").is_err());
    assert!(parse("record.a == ref(-1)").is_err());
}

#[test]
fn builds_the_declared_variants() {
    let boxed = |e: Expr| Box::new(e);
    assert_eq!(
        parse("new.author.principal").unwrap(),
        Expr::Through {
            scope: Scope::New,
            field: "author".to_string(),
            target: "principal".to_string(),
        }
    );
    assert_eq!(
        parse("old.$id >= ref(3)").unwrap(),
        Expr::Cmp {
            op: CmpOp::Ge,
            left: boxed(Expr::Var {
                scope: Scope::Old,
                name: "$id".to_string(),
            }),
            right: boxed(Expr::Lit(Value::Ref(3))),
        }
    );
    // A run of one n-ary operator is one list; a single operand is not wrapped.
    assert!(
        matches!(parse("true and true and true").unwrap(), Expr::And(items) if items.len() == 3)
    );
    assert_eq!(parse("(true)").unwrap(), Expr::Lit(Value::Bool(true)));
    assert_eq!(
        parse("[time(5), null, -2, [1]]").unwrap(),
        Expr::Lit(Value::List(vec![
            Value::Time(5),
            Value::Null,
            Value::Int(-2),
            Value::List(vec![Value::Int(1)]),
        ]))
    );
}

#[test]
fn prints_each_form_as_the_kernel_does() {
    // The empty n-ary forms print their identity literal; one operand keeps its parentheses.
    assert_eq!(Expr::And(vec![]).to_string(), "true");
    assert_eq!(Expr::Or(vec![]).to_string(), "false");
    assert_eq!(
        Expr::And(vec![Expr::Lit(Value::Null)]).to_string(),
        "(null)"
    );
    assert_eq!(
        Expr::Lit(Value::Str("it's \\ a\nline".to_string())).to_string(),
        "'it\\'s \\\\ a\\nline'"
    );
    assert_eq!(
        parse("if(has_role(editor), principal, param.who)")
            .unwrap()
            .to_string(),
        "if(has_role(editor), principal, param.who)"
    );
    assert_eq!(
        parse("not not record.a in [1, 2]").unwrap().to_string(),
        "not(not((record.a in [1, 2])))"
    );
}

/// Checked arithmetic and time arithmetic (D-6540, D-6541, D-6543): `*` binds tighter than `+`
/// and `-`, both bind tighter than comparison, and both group to the left. A `-` before a digit is
/// a subtraction after an operand and the sign of a literal anywhere else.
#[test]
fn parses_arithmetic_with_its_precedence_and_the_sign_of_literals() {
    for (source, printed) in [
        ("1 + 2 * 3", "(1 + (2 * 3))"),
        ("1 - 2 - 3", "((1 - 2) - 3)"),
        ("2 * 3 * 4 + 1", "(((2 * 3) * 4) + 1)"),
        ("record.n + 1 > 3", "((record.n + 1) > 3)"),
        (
            "now - days(7) < record.joined_at",
            "((now - days(7)) < record.joined_at)",
        ),
        ("record.n -1", "(record.n - 1)"),
        ("record.n-1", "(record.n - 1)"),
        ("(1)-1", "(1 - 1)"),
        ("-1", "-1"),
        ("record.n - -1", "(record.n - -1)"),
        ("record.n == -1", "(record.n == -1)"),
        ("record.n in [-1, 2]", "(record.n in [-1, 2])"),
        ("not -1 == 1", "not((-1 == 1))"),
        ("if(true, -1, 2)", "if(true, -1, 2)"),
        (
            "ms_between(record.a, now) >= hours(1) + minutes(30)",
            "(ms_between(record.a, now) >= (hours(1) + minutes(30)))",
        ),
        (
            "concat(principal, concat(':', text(param.badge)))",
            "concat(principal, concat(':', text(param.badge)))",
        ),
    ] {
        let parsed = parse(source).unwrap_or_else(|e| panic!("{source}: {e}"));
        assert_eq!(parsed.to_string(), printed, "{source}");
        assert_eq!(parse(printed).unwrap(), parsed, "{printed}");
    }
}

#[test]
fn builds_the_arithmetic_variants() {
    use strata_spec_compiler_language_fixture::types::{ArithOp, DurationUnit};
    let boxed = |e: Expr| Box::new(e);
    assert_eq!(
        parse("record.earned_at + days(30)").unwrap(),
        Expr::Arith {
            op: ArithOp::Add,
            left: boxed(Expr::Var {
                scope: Scope::Record,
                name: "earned_at".into(),
            }),
            right: boxed(Expr::Duration {
                unit: DurationUnit::Days,
                amount: boxed(Expr::Lit(Value::Int(30))),
            }),
        }
    );
    assert_eq!(
        parse("ms_between(now, text(1))").unwrap(),
        Expr::MsBetween {
            from: boxed(Expr::Now),
            to: boxed(Expr::Text(boxed(Expr::Lit(Value::Int(1))))),
        }
    );
    assert_eq!(
        parse("concat('a', 'b')").unwrap(),
        Expr::Concat {
            left: boxed(Expr::Lit(Value::Str("a".into()))),
            right: boxed(Expr::Lit(Value::Str("b".into()))),
        }
    );
}
