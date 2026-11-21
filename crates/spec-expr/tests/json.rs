//! The `json` type: any JSON value, compared only with `==` and `!=` against `json` or `null`, in
//! both targets; no ordering, arithmetic, concatenation or boolean use, and no path into it.

mod common;

use common::{js_err, scope, ty_of_ok, type_err, TestScope};
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
