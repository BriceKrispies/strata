//! Typing rules: what has which type, and each kind of type error.

mod common;

use common::{scope, ty_of_ok, type_err, TestScope};
use strata_spec_expr::{parse, to_rust_as, type_of, Ty, TypeErrorKind};

fn env() -> TestScope {
    scope(&[
        ("a", Ty::U32),
        ("b", Ty::U32),
        ("c", Ty::U64),
        ("small", Ty::U8),
        ("signed", Ty::I32),
        ("wide", Ty::I64),
        ("f", Ty::F64),
        ("g", Ty::F64),
        ("p", Ty::Bool),
        ("q", Ty::Bool),
        ("s", Ty::String),
        ("t", Ty::String),
        ("ts1", Ty::Timestamp),
        ("ts2", Ty::Timestamp),
        ("d1", Ty::Duration),
        ("tok", Ty::Named("session-token".into())),
        ("o", Ty::Option(Box::new(Ty::U32))),
        ("l", Ty::List(Box::new(Ty::U32))),
        ("m", Ty::List(Box::new(Ty::U32))),
        ("u", Ty::Uuid),
        ("event.token", Ty::U32),
    ])
}

#[test]
fn types_of_expressions() {
    let e = env();
    assert_eq!(ty_of_ok("a", &e), Ty::U32);
    assert_eq!(ty_of_ok("event.token", &e), Ty::U32);
    assert_eq!(ty_of_ok("a + 1", &e), Ty::U32);
    assert_eq!(ty_of_ok("1 + a", &e), Ty::U32);
    assert_eq!(ty_of_ok("a + b * 2", &e), Ty::U32);
    assert_eq!(ty_of_ok("c % 2", &e), Ty::U64);
    assert_eq!(ty_of_ok("f / g", &e), Ty::F64);
    assert_eq!(ty_of_ok("f + 1", &e), Ty::F64);
    assert_eq!(ty_of_ok("f * 1.5", &e), Ty::F64);
    assert_eq!(ty_of_ok("-signed", &e), Ty::I32);
    assert_eq!(ty_of_ok("-f", &e), Ty::F64);
    assert_eq!(ty_of_ok("s + 'x'", &e), Ty::String);
    assert_eq!(ty_of_ok("'x'", &e), Ty::String);
    assert_eq!(ty_of_ok("true", &e), Ty::Bool);
    assert_eq!(ty_of_ok("a + 1 == b", &e), Ty::Bool);
    assert_eq!(ty_of_ok("p && q => !p", &e), Ty::Bool);
    assert_eq!(ty_of_ok("s == t", &e), Ty::Bool);
    assert_eq!(ty_of_ok("s == 'x'", &e), Ty::Bool);
    assert_eq!(ty_of_ok("p == q", &e), Ty::Bool);
    assert_eq!(ty_of_ok("tok == tok", &e), Ty::Bool);
    assert_eq!(ty_of_ok("u != u", &e), Ty::Bool);
    assert_eq!(ty_of_ok("l == m", &e), Ty::Bool);
    assert_eq!(ty_of_ok("o == null", &e), Ty::Bool);
    assert_eq!(ty_of_ok("null != o", &e), Ty::Bool);
    assert_eq!(ty_of_ok("ts1 < ts2", &e), Ty::Bool);
    assert_eq!(ty_of_ok("d1 >= d1", &e), Ty::Bool);
    assert_eq!(ty_of_ok("f <= g", &e), Ty::Bool);
}

#[test]
fn literals_take_the_type_of_the_other_operand() {
    let e = env();
    assert_eq!(ty_of_ok("small + 255", &e), Ty::U8);
    assert_eq!(ty_of_ok("small == 0", &e), Ty::Bool);
    assert_eq!(ty_of_ok("signed - 2147483647", &e), Ty::I32);
    assert_eq!(ty_of_ok("signed == -2147483648", &e), Ty::Bool);
    assert_eq!(ty_of_ok("wide > -9007199254740991", &e), Ty::Bool);
    assert_eq!(ty_of_ok("c < 9007199254740991", &e), Ty::Bool);
    assert_eq!(ty_of_ok("a == -0", &e), Ty::Bool);
}

#[test]
fn literal_that_does_not_fit_is_an_error() {
    let e = env();
    for src in [
        "small + 256",
        "small == 1000",
        "a == 4294967296",
        "signed == 2147483648",
        "signed == -2147483649",
        "a == -1",
        "c == 18446744073709551616",
        "wide == 9223372036854775808",
        // u64 and i64 stop at +-(2^53 - 1) (docs/semantics.md, "Numbers").
        "c == 9007199254740992",
        "wide == -9007199254740992",
        "a + 1.5",
        "signed == 0.5",
    ] {
        let err = type_err(src, &e);
        assert_eq!(err.kind, TypeErrorKind::LiteralDoesNotFit, "{src}: {err}");
    }
    let err = type_err("small + 256", &e);
    assert!(
        err.message.contains("256") && err.message.contains("u8"),
        "{err}"
    );
}

#[test]
fn different_types_never_mix() {
    let e = env();
    for src in [
        "a + c",
        "a == c",
        "small < a",
        "a + signed",
        "a == f",
        "f + a",
        "s == a",
        "p == a",
        "o == a",
        "l == o",
        "ts1 == d1",
        "u == s",
        "tok == s",
        "a == 'x'",
        "s == 1",
        "p == 1",
    ] {
        let err = type_err(src, &e);
        assert_eq!(err.kind, TypeErrorKind::Mismatch, "{src}: {err}");
    }
}

#[test]
fn logical_operators_need_booleans() {
    let e = env();
    for src in [
        "a && p", "p || a", "a => p", "!a", "p && 1", "!'x'", "!null",
    ] {
        let err = type_err(src, &e);
        assert_eq!(err.kind, TypeErrorKind::NotBoolean, "{src}: {err}");
    }
}

#[test]
fn operators_must_apply_to_the_operand_type() {
    let e = env();
    for src in [
        "p + q",
        "p < q",
        "s < t",
        "s - t",
        "s * t",
        "tok < tok",
        "o < o",
        "l < m",
        "u < u",
        "-s",
        "-p",
        "-a",
        "-small",
        "-null",
        "o + 1 == a",
    ] {
        let err = type_err(src, &e);
        assert!(
            matches!(
                err.kind,
                TypeErrorKind::BadOperand | TypeErrorKind::Mismatch
            ),
            "{src}: {err}"
        );
    }
    assert_eq!(type_err("p + q", &e).kind, TypeErrorKind::BadOperand);
    assert_eq!(type_err("s < t", &e).kind, TypeErrorKind::BadOperand);
    assert_eq!(type_err("-a", &e).kind, TypeErrorKind::BadOperand);
    assert!(type_err("-a", &e).message.contains("unsigned"));
}

#[test]
fn unsupported_but_legal_exegra() {
    let e = env();
    for src in ["ts1 + ts2", "d1 * d1", "ts1 - ts2"] {
        let err = type_err(src, &e);
        assert_eq!(err.kind, TypeErrorKind::Unsupported, "{src}: {err}");
    }
    // Integer division truncates in Exegra too (WG-4902), so it is typed like the other operators.
    assert_eq!(ty_of_ok("a / b", &e), Ty::U32);
    assert_eq!(ty_of_ok("c / 2", &e), Ty::U64);
    assert_eq!(ty_of_ok("signed / 2", &e), Ty::I32);
}

#[test]
fn literals_alone_have_no_type() {
    let e = env();
    for src in ["1", "1.5", "1 + 2", "1 == 1", "-3", "-(1 + 2)", "1 < 2"] {
        let err = type_err(src, &e);
        assert_eq!(err.kind, TypeErrorKind::LiteralNeedsType, "{src}: {err}");
    }
    assert_eq!(type_err("null", &e).kind, TypeErrorKind::NullNeedsOption);
    assert_eq!(
        type_err("null == null", &e).kind,
        TypeErrorKind::NullNeedsOption
    );
}

#[test]
fn null_is_only_an_option() {
    let e = env();
    for src in ["a == null", "s != null", "null == p", "null + 1 == a"] {
        let err = type_err(src, &e);
        assert!(
            matches!(
                err.kind,
                TypeErrorKind::NullNeedsOption | TypeErrorKind::BadOperand
            ),
            "{src}: {err}"
        );
    }
    assert_eq!(
        type_err("a == null", &e).kind,
        TypeErrorKind::NullNeedsOption
    );
}

#[test]
fn unknown_references() {
    let e = env();
    let err = type_err("a + nope", &e);
    assert_eq!(err.kind, TypeErrorKind::UnknownReference);
    assert_eq!(err.message, "Unknown reference \"nope\"");
    assert_eq!(
        type_err("event.missing == 1", &e).kind,
        TypeErrorKind::UnknownReference
    );
    assert_eq!(
        type_err("event.missing == 1", &e).message,
        "Unknown reference \"event.missing\""
    );
}

#[test]
fn expected_type_is_checked_and_gives_literals_their_type() {
    let e = env();
    let parsed = |s: &str| parse(s).unwrap();
    assert!(to_rust_as(&parsed("0"), &e, &Ty::U64).is_ok());
    assert!(to_rust_as(&parsed("1 + 2"), &e, &Ty::U8).is_ok());
    assert!(to_rust_as(&parsed("1.5"), &e, &Ty::F64).is_ok());
    assert!(to_rust_as(&parsed("2"), &e, &Ty::F64).is_ok());
    assert!(to_rust_as(&parsed("'x'"), &e, &Ty::String).is_ok());
    assert!(to_rust_as(&parsed("null"), &e, &Ty::Option(Box::new(Ty::U8))).is_ok());
    assert!(to_rust_as(&parsed("a > 1"), &e, &Ty::Bool).is_ok());
    let kind = |s: &str, t: Ty| to_rust_as(&parsed(s), &e, &t).unwrap_err().kind;
    assert_eq!(kind("300", Ty::U8), TypeErrorKind::LiteralDoesNotFit);
    assert_eq!(kind("1.5", Ty::U32), TypeErrorKind::LiteralDoesNotFit);
    assert_eq!(kind("1", Ty::Bool), TypeErrorKind::Mismatch);
    assert_eq!(kind("1", Ty::String), TypeErrorKind::Mismatch);
    assert_eq!(kind("a", Ty::U64), TypeErrorKind::Mismatch);
    assert_eq!(kind("a + 1", Ty::I32), TypeErrorKind::Mismatch);
    assert_eq!(kind("'x'", Ty::U32), TypeErrorKind::Mismatch);
    assert_eq!(kind("null", Ty::U32), TypeErrorKind::NullNeedsOption);
    assert_eq!(kind("p", Ty::U32), TypeErrorKind::Mismatch);
}

#[test]
fn type_of_matches_translation() {
    let e = env();
    let parsed = parse("a + 1").unwrap();
    assert_eq!(type_of(&parsed, &e).unwrap(), Ty::U32);
}

#[test]
fn errors_carry_source_and_span() {
    let e = env();
    let err = type_err("a + c", &e);
    assert_eq!(err.source, "a + c");
    assert_eq!((err.span.start, err.span.end), (0, 5));
    let text = err.to_string();
    assert!(text.contains("u32") && text.contains("u64"), "{text}");
    assert!(text.contains("a + c") && text.contains("^^^^^"), "{text}");

    let err = type_err("a == 1 && 7 + small > nope", &e);
    assert_eq!(err.kind, TypeErrorKind::UnknownReference);
    assert_eq!(&err.source[err.span.start..err.span.end], "nope");
}

#[test]
fn type_names_display() {
    assert_eq!(
        Ty::List(Box::new(Ty::Option(Box::new(Ty::U8)))).to_string(),
        "list<option<u8>>"
    );
    assert_eq!(Ty::Named("x-y".into()).to_string(), "x-y");
    assert_eq!(Ty::Timestamp.to_string(), "timestamp");
}
