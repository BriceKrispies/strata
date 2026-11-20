//! The `json` type: any JSON value, compared only with `==` and `!=` against `json` or `null`, in
//! both targets; no ordering, arithmetic, concatenation or boolean use, and no path into it.

mod common;

use common::{js, js_err, rust, scope, tok, ty_of_ok, type_err, TestScope};
use strata_spec_expr::{Ty, TypeErrorKind};

fn env() -> TestScope {
    scope(&[
        ("j", Ty::Json),
        ("k", Ty::Json),
        ("s", Ty::String),
        ("a", Ty::U32),
        ("o", Ty::Option(Box::new(Ty::Json))),
    ])
}

#[test]
fn json_compares_with_json_and_null() {
    let e = env();
    for src in ["j == k", "j != k", "j == null", "null != j", "o == null"] {
        assert_eq!(ty_of_ok(src, &e), Ty::Bool, "{src}");
    }
}

#[test]
fn json_equality_in_rust_is_structural_with_numbers_by_value() {
    let e = env();
    // serde_json's own == tells 1 from 1.0; the emitted comparison compares numbers by value.
    let eq = rust("j == k", &e);
    assert!(eq.contains("__json_eq") && eq.contains("as_f64"), "{eq}");
    let ne = rust("j != k", &e);
    assert!(ne.starts_with('!'), "{ne}");
    assert_eq!(rust("j != null", &e), tok("j != ::serde_json::Value::Null"));
}

#[test]
fn lists_and_structs_compare_structurally_in_javascript() {
    let e = strata_spec_expr_test_scope();
    let eq = js("l == m", &e);
    assert!(eq.contains("Object.keys") && eq.ends_with("(l, m)"), "{eq}");
}

fn strata_spec_expr_test_scope() -> TestScope {
    scope(&[
        ("l", Ty::List(Box::new(Ty::U32))),
        ("m", Ty::List(Box::new(Ty::U32))),
    ])
}

#[test]
fn json_equality_in_javascript_is_structural() {
    let e = env();
    let eq = js("j == k", &e);
    assert!(eq.ends_with("(j, k)"), "{eq}");
    assert!(eq.contains("Object.keys"), "{eq}");
    let ne = js("j != null", &e);
    // Nothing but `null` equals the `null` literal, so that comparison stays an identity check.
    assert_eq!(ne, "j !== null");
    let eq_null = js("j == null", &e);
    assert_eq!(eq_null, "j === null");
}

#[test]
fn json_has_no_ordering_arithmetic_text_or_boolean_use() {
    let e = env();
    for (src, kind) in [
        ("j < k", TypeErrorKind::BadOperand),
        ("j + k", TypeErrorKind::BadOperand),
        ("j - k", TypeErrorKind::BadOperand),
        ("j == s", TypeErrorKind::Mismatch),
        ("a != j", TypeErrorKind::Mismatch),
        ("j + s", TypeErrorKind::Mismatch),
        ("!j", TypeErrorKind::NotBoolean),
        ("j && true", TypeErrorKind::NotBoolean),
    ] {
        assert_eq!(type_err(src, &e).kind, kind, "rust: {src}");
        assert_eq!(js_err(src, &e).kind, kind, "js: {src}");
    }
}

#[test]
fn no_path_reaches_into_json() {
    let e = env();
    assert_eq!(
        type_err("j.page == k", &e).kind,
        TypeErrorKind::UnknownReference
    );
}
