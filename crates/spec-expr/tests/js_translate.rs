//! The emitted JavaScript, compared as exact text, and the refusals shared with the Rust target.

mod common;

use common::{js, js_as, js_err, scope, TestScope};
use strata_spec_expr::{parse, to_js_as, to_rust_as, Ty, TypeErrorKind};

fn env() -> TestScope {
    scope(&[
        ("a", Ty::U32),
        ("b", Ty::U32),
        ("n", Ty::I64),
        ("signed", Ty::I32),
        ("f", Ty::F64),
        ("g", Ty::F64),
        ("p", Ty::Bool),
        ("q", Ty::Bool),
        ("s", Ty::String),
        ("t", Ty::String),
        ("o", Ty::Option(Box::new(Ty::U32))),
        ("ts1", Ty::Timestamp),
        ("ts2", Ty::Timestamp),
        ("event.token", Ty::U32),
        ("lease.count.jl-done", Ty::U64),
        ("token", Ty::U32),
    ])
}

const OV: &str = "__ov()";

/// A range-checked integer expression.
fn chk(lo: &str, hi: &str, expr: &str) -> String {
    format!("(($v) => $v >= {lo} && $v <= {hi} ? $v : {OV})({expr})")
}

fn u32c(expr: &str) -> String {
    chk("0", "4294967295", expr)
}

#[test]
fn references_are_bare() {
    let e = env();
    assert_eq!(js("a", &e), "a");
    assert_eq!(js("event.token", &e), "event_token");
    assert_eq!(js("lease.count.jl-done", &e), "lease_count_jl_done");
}

#[test]
fn literals() {
    let e = env();
    assert_eq!(js("true", &e), "true");
    assert_eq!(js("false", &e), "false");
    assert_eq!(js("'x'", &e), "\"x\"");
    assert_eq!(js("'a\"b\\c'", &e), "\"a\\\"b\\\\c\"");
    assert_eq!(js("\"line\u{2028}\"", &e), "\"line\\u2028\"");
    assert_eq!(js_as("7", &e, &Ty::U8), "7");
    assert_eq!(js_as("007", &e, &Ty::U16), "7");
    assert_eq!(js_as("-7", &e, &Ty::I32), "-7");
    assert_eq!(js_as("-0", &e, &Ty::I64), "0");
    assert_eq!(js_as("2", &e, &Ty::F64), "2");
    assert_eq!(js_as("1.5", &e, &Ty::F64), "1.5");
    assert_eq!(js_as("-1.5", &e, &Ty::F64), "-1.5");
    assert_eq!(js_as("9007199254740991", &e, &Ty::U64), "9007199254740991");
    assert_eq!(
        js_as("-9007199254740991", &e, &Ty::I64),
        "-9007199254740991"
    );
    assert_eq!(js_as("null", &e, &Ty::Option(Box::new(Ty::U32))), "null");
}

#[test]
fn checked_integer_arithmetic() {
    let e = env();
    assert_eq!(js("token + 1", &e), u32c("token + 1"));
    assert_eq!(js("a - b", &e), u32c("a - b"));
    assert_eq!(js("a * 3", &e), u32c("a * 3"));
    assert_eq!(
        js("a % 3", &e),
        format!("(($l, $r) => $r === 0 ? {OV} : $l % $r + 0)(a, 3)")
    );
    assert_eq!(
        js("signed % 3", &e),
        format!("(($l, $r) => $r === 0 ? {OV} : $l % $r + 0)(signed, 3)")
    );
    assert_eq!(
        js("-signed", &e),
        chk("-2147483648", "2147483647", "0 - signed")
    );
    assert_eq!(
        js("n + 1", &e),
        chk("-9007199254740991", "9007199254740991", "n + 1")
    );
    assert_eq!(
        js("lease.count.jl-done + 1", &e),
        chk("0", "9007199254740991", "lease_count_jl_done + 1")
    );
}

#[test]
fn nested_arithmetic_checks_every_step() {
    let e = env();
    assert_eq!(js("a + b + 1", &e), u32c(&format!("{} + 1", u32c("a + b"))));
    assert_eq!(
        js("a * (b + 1)", &e),
        u32c(&format!("a * {}", u32c("b + 1")))
    );
}

#[test]
fn float_arithmetic() {
    let e = env();
    assert_eq!(js("f + g * 2", &e), "f + g * 2");
    assert_eq!(js("(f + g) * 2", &e), "(f + g) * 2");
    assert_eq!(js("f - (g - 1)", &e), "f - (g - 1)");
    assert_eq!(js("f - g - 1", &e), "f - g - 1");
    assert_eq!(js("f - -1.5", &e), "f - -1.5");
    assert_eq!(js("-f", &e), "-f");
    assert_eq!(js("-(-f)", &e), "-(-f)");
    assert_eq!(js("-(f + g)", &e), "-(f + g)");
    assert_eq!(
        js("f / g", &e),
        format!("(($l, $r) => $r === 0 ? {OV} : $l / $r)(f, g)")
    );
    assert_eq!(
        js("f % 2", &e),
        format!("(($l, $r) => $r === 0 ? {OV} : $l % $r)(f, 2)")
    );
}

#[test]
fn comparisons_use_strict_equality() {
    let e = env();
    assert_eq!(js("a == b", &e), "a === b");
    assert_eq!(js("a != b", &e), "a !== b");
    assert_eq!(js("a < b", &e), "a < b");
    assert_eq!(js("a <= b", &e), "a <= b");
    assert_eq!(js("a > b", &e), "a > b");
    assert_eq!(js("a >= b", &e), "a >= b");
    assert_eq!(js("p == q", &e), "p === q");
    assert_eq!(js("(a < b) == p", &e), "(a < b) === p");
    assert_eq!(js("s == 't'", &e), "s === \"t\"");
    assert_eq!(js("o == null", &e), "o === null");
    assert_eq!(js("null != o", &e), "null !== o");
    assert_eq!(js("ts1 < ts2", &e), "ts1 < ts2");
    assert_eq!(js("ts1 == ts2", &e), "ts1 === ts2");
    assert_eq!(js("a + 1 == b", &e), format!("{} === b", u32c("a + 1")));
    assert_eq!(js("f + g < 1.5", &e), "f + g < 1.5");
}

#[test]
fn logic_short_circuits_and_implies_is_not_or() {
    let e = env();
    assert_eq!(js("p && q || p", &e), "p && q || p");
    assert_eq!(js("p && (q || p)", &e), "p && (q || p)");
    assert_eq!(js("p || q && p", &e), "p || q && p");
    assert_eq!(js("p => q", &e), "!p || q");
    assert_eq!(js("p => q => p", &e), "!p || !q || p");
    assert_eq!(js("(p => q) => p", &e), "!(!p || q) || p");
    assert_eq!(js("!p", &e), "!p");
    assert_eq!(js("!(p && q)", &e), "!(p && q)");
    assert_eq!(js("!(a == b)", &e), "!(a === b)");
    assert_eq!(js("!!p", &e), "!!p");
    // The right operand stays inside its own branch: no helper evaluates both sides.
    assert_eq!(
        js("p && a % 2 == 0", &e),
        format!("p && (($l, $r) => $r === 0 ? {OV} : $l % $r + 0)(a, 2) === 0")
    );
}

#[test]
fn string_concatenation() {
    let e = env();
    assert_eq!(js("s + t", &e), "s + t");
    assert_eq!(js("s + t + 'x'", &e), "s + t + \"x\"");
    assert_eq!(js("s + (t + s)", &e), "s + (t + s)");
    assert_eq!(js("s + t == 'ab'", &e), "s + t === \"ab\"");
}

#[test]
fn refusals_match_the_rust_target() {
    let e = env();
    let cases: &[(&str, TypeErrorKind)] = &[
        ("ts1 + ts2", TypeErrorKind::Unsupported),
        ("a == s", TypeErrorKind::Mismatch),
        ("a + f", TypeErrorKind::Mismatch),
        ("p + p", TypeErrorKind::BadOperand),
        ("-a", TypeErrorKind::BadOperand),
        ("-s", TypeErrorKind::BadOperand),
        ("s < t", TypeErrorKind::BadOperand),
        ("s - t", TypeErrorKind::BadOperand),
        ("missing", TypeErrorKind::UnknownReference),
        ("a && p", TypeErrorKind::NotBoolean),
        ("!a", TypeErrorKind::NotBoolean),
        ("1 + 2", TypeErrorKind::LiteralNeedsType),
        ("1 == 2", TypeErrorKind::LiteralNeedsType),
        ("null", TypeErrorKind::NullNeedsOption),
        ("null == null", TypeErrorKind::NullNeedsOption),
        ("a == null", TypeErrorKind::NullNeedsOption),
        ("a == 1.5", TypeErrorKind::LiteralDoesNotFit),
        ("a == 4294967296", TypeErrorKind::LiteralDoesNotFit),
        ("a == -1", TypeErrorKind::LiteralDoesNotFit),
        ("o == 1", TypeErrorKind::Mismatch),
    ];
    for (src, kind) in cases {
        let ex = parse(src).unwrap();
        let rust = strata_spec_expr::to_rust(&ex, &e).unwrap_err();
        assert_eq!(&js_err(src, &e).kind, kind, "{src}");
        assert_eq!(rust.kind, *kind, "{src} (rust)");
    }
}

#[test]
fn literal_range_is_enforced_per_type() {
    let e = env();
    let cases: &[(&str, Ty)] = &[
        ("256", Ty::U8),
        ("-1", Ty::U8),
        ("65536", Ty::U16),
        ("2147483648", Ty::I32),
        ("-2147483649", Ty::I32),
        ("1.5", Ty::U32),
        ("null", Ty::U32),
        ("5", Ty::Bool),
        ("5", Ty::String),
    ];
    for (src, ty) in cases {
        let ex = parse(src).unwrap();
        let j = to_js_as(&ex, &e, ty).unwrap_err();
        let r = to_rust_as(&ex, &e, ty).unwrap_err();
        assert_eq!(j.kind, r.kind, "{src} as {ty}");
    }
}

#[test]
fn sixty_four_bit_literals_stop_at_the_safe_integer_limit() {
    // The same bound as the Rust target and Exegra: a double cannot hold more.
    let e = env();
    let ex = parse("9007199254740992").unwrap();
    let err = to_js_as(&ex, &e, &Ty::U64).unwrap_err();
    assert_eq!(err.kind, TypeErrorKind::LiteralDoesNotFit);
    assert_eq!(
        to_rust_as(&ex, &e, &Ty::U64).unwrap_err().kind,
        TypeErrorKind::LiteralDoesNotFit
    );
    let ex = parse("-9007199254740992").unwrap();
    assert_eq!(
        to_js_as(&ex, &e, &Ty::I64).unwrap_err().kind,
        TypeErrorKind::LiteralDoesNotFit
    );
}

#[test]
fn effect_value_must_have_the_target_type() {
    let e = env();
    let ex = parse("a + b").unwrap();
    assert_eq!(
        to_js_as(&ex, &e, &Ty::U64).unwrap_err().kind,
        TypeErrorKind::Mismatch
    );
    assert!(to_js_as(&ex, &e, &Ty::U32).is_ok());
}
