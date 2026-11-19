//! The emitted Rust: expectations are written as Rust source and compared as token text without
//! whitespace. Parentheses appear only where Rust's precedence needs them.

mod common;

use common::{rust, rust_as, scope, tok, TestScope};
use strata_spec_expr::{parse, to_rust, Ty};

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

const OV: &str = "StepError::Overflow";

#[test]
fn references_are_bare() {
    let e = env();
    assert_eq!(rust("a", &e), tok("a"));
    assert_eq!(rust("event.token", &e), tok("event_token"));
    assert_eq!(rust("lease.count.jl-done", &e), tok("lease_count_jl_done"));
}

#[test]
fn literals() {
    let e = env();
    assert_eq!(rust("true", &e), tok("true"));
    assert_eq!(rust("false", &e), tok("false"));
    assert_eq!(rust("'x'", &e), tok("::std::string::String::from(\"x\")"));
    assert_eq!(rust_as("7", &e, &Ty::U8), tok("7u8"));
    assert_eq!(rust_as("007", &e, &Ty::U16), tok("7u16"));
    assert_eq!(rust_as("-7", &e, &Ty::I32), tok("-7i32"));
    assert_eq!(rust_as("-0", &e, &Ty::I64), tok("0i64"));
    assert_eq!(rust_as("2", &e, &Ty::F64), tok("2.0f64"));
    assert_eq!(rust_as("1.5", &e, &Ty::F64), tok("1.5f64"));
    assert_eq!(rust_as("-1.5", &e, &Ty::F64), tok("-1.5f64"));
    // u64 and i64 stop at +-(2^53 - 1), the integers every engine holds exactly.
    assert_eq!(
        rust_as("9007199254740991", &e, &Ty::U64),
        tok("9007199254740991u64")
    );
    assert_eq!(
        rust_as("-9007199254740991", &e, &Ty::I64),
        tok("-9007199254740991i64")
    );
    assert_eq!(
        rust_as("null", &e, &Ty::Option(Box::new(Ty::U32))),
        tok("::core::option::Option::None")
    );
}

#[test]
fn checked_integer_arithmetic() {
    let e = env();
    assert_eq!(
        rust("token + 1", &e),
        tok(&format!("token.checked_add(1u32).ok_or({OV})?"))
    );
    assert_eq!(
        rust("a - b", &e),
        tok(&format!("a.checked_sub(b).ok_or({OV})?"))
    );
    assert_eq!(
        rust("a * 3", &e),
        tok(&format!("a.checked_mul(3u32).ok_or({OV})?"))
    );
    assert_eq!(
        rust("n % 2", &e),
        tok(&format!(
            "{{ let __l = n; let __r = 2i64; if __r == 0 {{ return ::core::result::Result::Err({OV}); }} __l.wrapping_rem(__r) }}"
        ))
    );
    assert_eq!(
        rust("n + 1", &e),
        tok(&format!(
            "n.checked_add(1i64).filter(|__v| (-9007199254740991i64..=9007199254740991i64).contains(__v)).ok_or({OV})?"
        ))
    );
    assert_eq!(
        rust("lease.count.jl-done * 2", &e),
        tok(&format!(
            "lease_count_jl_done.checked_mul(2u64).filter(|__v| *__v <= 9007199254740991u64).ok_or({OV})?"
        ))
    );
    assert_eq!(
        rust("a / b", &e),
        tok(&format!("a.checked_div(b).ok_or({OV})?"))
    );
    assert_eq!(
        rust("1 + a", &e),
        tok(&format!("1u32.checked_add(a).ok_or({OV})?"))
    );
}

#[test]
fn nested_arithmetic_checks_every_step() {
    let e = env();
    assert_eq!(
        rust("(a + 1) * 2", &e),
        tok(&format!(
            "a.checked_add(1u32).ok_or({OV})?.checked_mul(2u32).ok_or({OV})?"
        ))
    );
    assert_eq!(
        rust("a + 1 * 2", &e),
        tok(&format!(
            "a.checked_add(1u32.checked_mul(2u32).ok_or({OV})?).ok_or({OV})?"
        ))
    );
}

#[test]
fn negation() {
    let e = env();
    assert_eq!(
        rust("-signed", &e),
        tok(&format!("signed.checked_neg().ok_or({OV})?"))
    );
    assert_eq!(rust("-f", &e), tok("-f"));
    assert_eq!(rust("signed == -5", &e), tok("signed == -5i32"));
    // A negative literal is a unary expression: it needs parentheses as a method receiver only.
    assert_eq!(
        rust("-1 + signed", &e),
        tok(&format!("(-1i32).checked_add(signed).ok_or({OV})?"))
    );
    assert_eq!(
        rust("-(signed + 1)", &e),
        tok(&format!(
            "signed.checked_add(1i32).ok_or({OV})?.checked_neg().ok_or({OV})?"
        ))
    );
}

#[test]
fn float_arithmetic_is_plain_and_division_by_zero_is_the_overflow_error() {
    let e = env();
    assert_eq!(rust("f + g", &e), tok("f + g"));
    assert_eq!(rust("f - 1", &e), tok("f - 1.0f64"));
    assert_eq!(rust("f * 0.5", &e), tok("f * 0.5f64"));
    assert_eq!(
        rust("f / g", &e),
        tok(&format!(
            "{{ let __l = f; let __r = g; if __r == 0.0f64 {{ return ::core::result::Result::Err({OV}); }} __l / __r }}"
        ))
    );
    assert_eq!(
        rust("f % 2", &e),
        tok(&format!(
            "{{ let __l = f; let __r = 2.0f64; if __r == 0.0f64 {{ return ::core::result::Result::Err({OV}); }} __l % __r }}"
        ))
    );
}

#[test]
fn float_precedence_adds_parentheses_only_where_needed() {
    let e = env();
    assert_eq!(rust("f + g * f", &e), tok("f + g * f"));
    assert_eq!(rust("(f + g) * f", &e), tok("(f + g) * f"));
    assert_eq!(rust("f - g - f", &e), tok("f - g - f"));
    assert_eq!(rust("f - (g - f)", &e), tok("f - (g - f)"));
    assert_eq!(rust("f * g * f", &e), tok("f * g * f"));
    assert_eq!(rust("f * (g * f)", &e), tok("f * (g * f)"));
    assert_eq!(rust("-(f + g)", &e), tok("-(f + g)"));
    assert_eq!(rust("-f * g", &e), tok("-f * g"));
    assert_eq!(rust("f * -g", &e), tok("f * -g"));
    assert_eq!(rust("f + g < f * g", &e), tok("f + g < f * g"));
}

#[test]
fn comparisons() {
    let e = env();
    assert_eq!(rust("a == b", &e), tok("a == b"));
    assert_eq!(rust("a != 1", &e), tok("a != 1u32"));
    assert_eq!(rust("a < b", &e), tok("a < b"));
    assert_eq!(rust("a <= b", &e), tok("a <= b"));
    assert_eq!(rust("a > b", &e), tok("a > b"));
    assert_eq!(rust("a >= 2", &e), tok("a >= 2u32"));
    assert_eq!(rust("f < 1.5", &e), tok("f < 1.5f64"));
    assert_eq!(rust("ts1 < ts2", &e), tok("ts1 < ts2"));
    assert_eq!(rust("p == q", &e), tok("p == q"));
    assert_eq!(rust("s == 'x'", &e), tok("s == \"x\""));
    assert_eq!(
        rust("o == null", &e),
        tok("o == ::core::option::Option::None")
    );
    assert_eq!(
        rust("null != o", &e),
        tok("::core::option::Option::None != o")
    );
    // Comparisons do not chain in Rust, so a comparison operand is parenthesised.
    assert_eq!(rust("(a < b) == p", &e), tok("(a < b) == p"));
    assert_eq!(
        rust("a + 1 == b", &e),
        tok(&format!("a.checked_add(1u32).ok_or({OV})? == b"))
    );
}

#[test]
fn logical_operators() {
    let e = env();
    assert_eq!(rust("p && q", &e), tok("p && q"));
    assert_eq!(rust("p || q", &e), tok("p || q"));
    assert_eq!(rust("!p", &e), tok("!p"));
    assert_eq!(rust("!!p", &e), tok("!!p"));
    assert_eq!(rust("p => q", &e), tok("!p || q"));
    assert_eq!(rust("p => q => p", &e), tok("!p || !q || p"));
    assert_eq!(rust("!(p && q)", &e), tok("!(p && q)"));
    assert_eq!(rust("!(a == b)", &e), tok("!(a == b)"));
    assert_eq!(rust("a == b && p", &e), tok("a == b && p"));
    assert_eq!(rust("p || q && p", &e), tok("p || q && p"));
    assert_eq!(rust("(p || q) && p", &e), tok("(p || q) && p"));
    assert_eq!(rust("(p || q) => p", &e), tok("!(p || q) || p"));
    assert_eq!(rust("(p && q) => p", &e), tok("!(p && q) || p"));
}

#[test]
fn logical_operators_short_circuit_around_checked_arithmetic() {
    let e = env();
    // The right operand, with its `?`, sits inside the `&&`/`||` expression, so it only runs when
    // the left operand does not decide.
    assert_eq!(
        rust("a > 0 && a - 1 > 0", &e),
        tok(&format!(
            "a > 0u32 && a.checked_sub(1u32).ok_or({OV})? > 0u32"
        ))
    );
    assert_eq!(
        rust("a == 0 || a - 1 > 0", &e),
        tok(&format!(
            "a == 0u32 || a.checked_sub(1u32).ok_or({OV})? > 0u32"
        ))
    );
    assert_eq!(
        rust("a == 0 => a - 1 > 0", &e),
        tok(&format!(
            "!(a == 0u32) || a.checked_sub(1u32).ok_or({OV})? > 0u32"
        ))
    );
}

#[test]
fn string_concatenation() {
    let e = env();
    assert_eq!(
        rust("s + t", &e),
        tok("{ let mut __s = ::std::string::String::from(&*s); __s.push_str(&*t); __s }")
    );
}

#[test]
fn effect_values_take_the_variable_type() {
    let e = env();
    assert_eq!(rust_as("0", &e, &Ty::U32), tok("0u32"));
    assert_eq!(
        rust_as("token + 1", &e, &Ty::U32),
        tok(&format!("token.checked_add(1u32).ok_or({OV})?"))
    );
    assert_eq!(
        rust_as("1 + 2", &e, &Ty::U8),
        tok(&format!("1u8.checked_add(2u8).ok_or({OV})?"))
    );
    assert_eq!(rust_as("event.token", &e, &Ty::U32), tok("event_token"));
}

#[test]
fn translation_enforces_literal_fitting() {
    let e = env();
    let small = scope(&[("small", Ty::U8)]);
    for src in ["small + 256", "small == 300"] {
        let parsed = parse(src).unwrap();
        assert!(to_rust(&parsed, &small).is_err(), "{src}");
    }
    assert!(strata_spec_expr::to_rust_as(&parse("300").unwrap(), &e, &Ty::U8).is_err());
    assert!(strata_spec_expr::to_rust_as(&parse("255").unwrap(), &e, &Ty::U8).is_ok());
}

#[test]
fn an_overflowing_sum_is_translated_to_the_error_path() {
    let e = env();
    let out = rust("a + b", &e);
    assert!(out.contains("checked_add"), "{out}");
    assert!(out.contains("ok_or"), "{out}");
    assert!(out.contains("StepError"), "{out}");
    assert!(out.contains('?'), "{out}");
}

/// The deepest accepted expressions go through parsing, typing and translation on a small stack
/// (half of a Windows main thread's), so the depth limit is proven to leave room.
#[test]
fn deepest_accepted_expressions_fit_a_small_stack() {
    let handle = std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(|| {
            let e = env();
            let chain = format!("a{}", " + 1".repeat(62));
            let parens = format!("{}a + 1{}", "(".repeat(62), ")".repeat(62));
            let nots = format!("{}p", "!".repeat(62));
            let negs = format!("{}signed", "-".repeat(62));
            let right = format!("{}p{}", "p && (".repeat(30), ")".repeat(30));
            for src in [chain, parens, nots, negs, right] {
                let parsed = parse(&src).unwrap_or_else(|err| panic!("{err}"));

                strata_spec_expr::type_of(&parsed, &e).unwrap();
                to_rust(&parsed, &e).unwrap();
            }
        })
        .unwrap();
    handle
        .join()
        .expect("deep expressions overflowed the stack");
}
