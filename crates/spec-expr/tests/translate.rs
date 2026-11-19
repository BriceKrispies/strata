//! The emitted Rust: expectations are written as Rust source and compared as token text.

mod common;

use common::{rust, rust_as, scope, tok, TestScope};
use strata_spec_expr::Ty;

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

#[test]
fn references_become_parenthesised_expressions() {
    let e = env();
    assert_eq!(rust("a", &e), tok("(a)"));
    assert_eq!(rust("event.token", &e), tok("(event_token)"));
    assert_eq!(
        rust("lease.count.jl-done", &e),
        tok("(lease_count_jl_done)")
    );
}

#[test]
fn literals() {
    let e = env();
    assert_eq!(rust("true", &e), tok("(true)"));
    assert_eq!(rust("false", &e), tok("(false)"));
    assert_eq!(rust("'x'", &e), tok("(::std::string::String::from(\"x\"))"));
    assert_eq!(rust_as("7", &e, &Ty::U8), tok("(7u8)"));
    assert_eq!(rust_as("007", &e, &Ty::U16), tok("(7u16)"));
    assert_eq!(rust_as("-7", &e, &Ty::I32), tok("(-7i32)"));
    assert_eq!(rust_as("-0", &e, &Ty::I64), tok("(0i64)"));
    assert_eq!(rust_as("2", &e, &Ty::F64), tok("(2.0f64)"));
    assert_eq!(rust_as("1.5", &e, &Ty::F64), tok("(1.5f64)"));
    assert_eq!(rust_as("-1.5", &e, &Ty::F64), tok("(-1.5f64)"));
    assert_eq!(
        rust_as("18446744073709551615", &e, &Ty::U64),
        tok("(18446744073709551615u64)")
    );
    assert_eq!(
        rust_as("-9223372036854775808", &e, &Ty::I64),
        tok("(-9223372036854775808i64)")
    );
    assert_eq!(
        rust_as("null", &e, &Ty::Option(Box::new(Ty::U32))),
        tok("(::core::option::Option::None)")
    );
}

#[test]
fn checked_integer_arithmetic() {
    let e = env();
    assert_eq!(
        rust("token + 1", &e),
        tok("((token).checked_add(1u32).ok_or(StepError::Overflow)?)")
    );
    assert_eq!(
        rust("a - b", &e),
        tok("((a).checked_sub(b).ok_or(StepError::Overflow)?)")
    );
    assert_eq!(
        rust("a * 3", &e),
        tok("((a).checked_mul(3u32).ok_or(StepError::Overflow)?)")
    );
    assert_eq!(
        rust("n % 2", &e),
        tok("((n).checked_rem(2i64).ok_or(StepError::Overflow)?)")
    );
    assert_eq!(
        rust("1 + a", &e),
        tok("((1u32).checked_add(a).ok_or(StepError::Overflow)?)")
    );
}

#[test]
fn nested_arithmetic_keeps_grouping_and_every_step_is_checked() {
    let e = env();
    assert_eq!(
        rust("(a + 1) * 2", &e),
        tok("(((a).checked_add(1u32).ok_or(StepError::Overflow)?).checked_mul(2u32).ok_or(StepError::Overflow)?)")
    );
    assert_eq!(
        rust("a + 1 * 2", &e),
        tok("((a).checked_add((1u32).checked_mul(2u32).ok_or(StepError::Overflow)?).ok_or(StepError::Overflow)?)")
    );
}

#[test]
fn negation() {
    let e = env();
    assert_eq!(
        rust("-signed", &e),
        tok("((signed).checked_neg().ok_or(StepError::Overflow)?)")
    );
    assert_eq!(rust("-f", &e), tok("(-(f))"));
    assert_eq!(rust("signed == -5", &e), tok("((signed) == (-5i32))"));
}

#[test]
fn float_arithmetic_is_plain_and_division_by_zero_is_the_overflow_error() {
    let e = env();
    assert_eq!(rust("f + g", &e), tok("((f) + (g))"));
    assert_eq!(rust("f - 1", &e), tok("((f) - (1.0f64))"));
    assert_eq!(rust("f * 0.5", &e), tok("((f) * (0.5f64))"));
    assert_eq!(
        rust("f / g", &e),
        tok("({ let __l = (f); let __r = (g); if __r == 0.0f64 { return ::core::result::Result::Err(StepError::Overflow); } (__l / __r) })")
    );
    assert_eq!(
        rust("f % 2", &e),
        tok("({ let __l = (f); let __r = (2.0f64); if __r == 0.0f64 { return ::core::result::Result::Err(StepError::Overflow); } (__l % __r) })")
    );
}

#[test]
fn comparisons() {
    let e = env();
    assert_eq!(rust("a == b", &e), tok("((a) == (b))"));
    assert_eq!(rust("a != 1", &e), tok("((a) != (1u32))"));
    assert_eq!(rust("a < b", &e), tok("((a) < (b))"));
    assert_eq!(rust("a <= b", &e), tok("((a) <= (b))"));
    assert_eq!(rust("a > b", &e), tok("((a) > (b))"));
    assert_eq!(rust("a >= 2", &e), tok("((a) >= (2u32))"));
    assert_eq!(rust("f < 1.5", &e), tok("((f) < (1.5f64))"));
    assert_eq!(rust("ts1 < ts2", &e), tok("((ts1) < (ts2))"));
    assert_eq!(rust("p == q", &e), tok("((p) == (q))"));
    assert_eq!(
        rust("s == 'x'", &e),
        tok("((s) == (::std::string::String::from(\"x\")))")
    );
    assert_eq!(
        rust("o == null", &e),
        tok("((o) == (::core::option::Option::None))")
    );
    assert_eq!(
        rust("null != o", &e),
        tok("((::core::option::Option::None) != (o))")
    );
}

#[test]
fn logical_operators() {
    let e = env();
    assert_eq!(rust("p && q", &e), tok("((p) && (q))"));
    assert_eq!(rust("p || q", &e), tok("((p) || (q))"));
    assert_eq!(rust("!p", &e), tok("(!(p))"));
    assert_eq!(rust("p => q", &e), tok("((!(p)) || (q))"));
    assert_eq!(rust("p => q => p", &e), tok("((!(p)) || ((!(q)) || (p)))"));
    assert_eq!(rust("!(p && q)", &e), tok("(!((p) && (q)))"));
}

#[test]
fn logical_operators_short_circuit_around_checked_arithmetic() {
    let e = env();
    // The right operand, with its `?`, sits inside the `&&`/`||` expression, so it only runs when
    // the left operand does not decide.
    assert_eq!(
        rust("a > 0 && a - 1 > 0", &e),
        tok("(((a) > (0u32)) && (((a).checked_sub(1u32).ok_or(StepError::Overflow)?) > (0u32)))")
    );
    assert_eq!(
        rust("a == 0 || a - 1 > 0", &e),
        tok("(((a) == (0u32)) || (((a).checked_sub(1u32).ok_or(StepError::Overflow)?) > (0u32)))")
    );
    assert_eq!(
        rust("a == 0 => a - 1 > 0", &e),
        tok("((!((a) == (0u32))) || (((a).checked_sub(1u32).ok_or(StepError::Overflow)?) > (0u32)))")
    );
}

#[test]
fn string_concatenation() {
    let e = env();
    assert_eq!(
        rust("s + t", &e),
        tok("({ let mut __s = ::std::string::String::from(&*(s)); __s.push_str(&*(t)); __s })")
    );
}

#[test]
fn effect_values_take_the_variable_type() {
    let e = env();
    assert_eq!(rust_as("0", &e, &Ty::U32), tok("(0u32)"));
    assert_eq!(
        rust_as("token + 1", &e, &Ty::U32),
        tok("((token).checked_add(1u32).ok_or(StepError::Overflow)?)")
    );
    assert_eq!(
        rust_as("1 + 2", &e, &Ty::U8),
        tok("((1u8).checked_add(2u8).ok_or(StepError::Overflow)?)")
    );
    assert_eq!(rust_as("event.token", &e, &Ty::U32), tok("(event_token)"));
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
