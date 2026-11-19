//! The wire forms specc's unions, maps and constrained types serialise to: each value is written
//! to the exact JSON text serde produces for its encoding, and read back from that text to an
//! equal value. The types are generated from
//! `crates/spec-compiler/tests/fixtures/core/spec.jsonc`.

use std::collections::BTreeMap;
use std::fmt::Debug;

use serde::de::DeserializeOwned;
use serde::Serialize;
use strata_spec_compiler_core_fixture::types::{
    Chain, Change, Expr, SeatCount, Seats, Slug, TenantRef, Token, TypeConstraintViolation, Value,
};

/// `value` serialises to exactly `json`, and `json` deserialises to `value`.
fn round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: T, json: &str) {
    assert_eq!(serde_json::to_string(&value).unwrap(), json);
    assert_eq!(serde_json::from_str::<T>(json).unwrap(), value);
}

fn slug(text: &str) -> Slug {
    Slug::new(text.to_string()).unwrap()
}

// External tagging (the default): {"Variant": content}, a unit variant as the bare string, a
// newtype variant's content as the value itself; variant names as declared.

#[test]
fn an_external_unit_variant_is_its_name() {
    round_trip(Value::Null, r#""Null""#);
}

#[test]
fn an_external_newtype_variant_holds_its_value() {
    round_trip(Value::Bool(true), r#"{"Bool":true}"#);
    round_trip(Value::Int(-3), r#"{"Int":-3}"#);
    round_trip(Value::Text("hi".to_string()), r#"{"Text":"hi"}"#);
}

#[test]
fn a_union_recurses_through_a_list_and_a_map() {
    round_trip(
        Value::List(vec![Value::Int(1), Value::Null]),
        r#"{"List":[{"Int":1},"Null"]}"#,
    );
    let members = BTreeMap::from([
        ("b".to_string(), Value::Bool(false)),
        ("a".to_string(), Value::List(vec![])),
    ]);
    // A map is written in key order.
    round_trip(
        Value::Record(members),
        r#"{"Record":{"a":{"List":[]},"b":{"Bool":false}}}"#,
    );
}

// Internal tagging: one object {"op": "variant", ...fields}; variant names in snake_case.

#[test]
fn an_internal_struct_variant_carries_its_tag_among_its_fields() {
    round_trip(
        Expr::Literal {
            value: Value::Int(2),
        },
        r#"{"op":"literal","value":{"Int":2}}"#,
    );
}

#[test]
fn an_internal_unit_variant_is_its_tag_alone() {
    round_trip(Expr::Missing, r#"{"op":"missing"}"#);
}

#[test]
fn a_union_recurses_through_a_box() {
    round_trip(
        Expr::Not {
            operand: Box::new(Expr::Missing),
        },
        r#"{"op":"not","operand":{"op":"missing"}}"#,
    );
    round_trip(
        Expr::And {
            operands: vec![
                Expr::Missing,
                Expr::Not {
                    operand: Box::new(Expr::Missing),
                },
            ],
        },
        r#"{"op":"and","operands":[{"op":"missing"},{"op":"not","operand":{"op":"missing"}}]}"#,
    );
}

#[test]
fn an_optional_variant_field_is_left_out_when_absent() {
    round_trip(
        Expr::Pick {
            from: Box::new(Expr::Missing),
            default: None,
        },
        r#"{"op":"pick","from":{"op":"missing"}}"#,
    );
    round_trip(
        Expr::Pick {
            from: Box::new(Expr::Missing),
            default: Some(Box::new(Expr::Literal { value: Value::Null })),
        },
        r#"{"op":"pick","from":{"op":"missing"},"default":{"op":"literal","value":"Null"}}"#,
    );
}

// Adjacent tagging: {"type": "variant", "data": content}, content left out for a unit variant;
// variant names in kebab-case; struct fields keep the specification's names.

#[test]
fn an_adjacent_unit_variant_has_no_content() {
    round_trip(Change::Started, r#"{"type":"started"}"#);
}

#[test]
fn an_adjacent_struct_variant_holds_its_fields_as_content() {
    round_trip(
        Change::RenamedTo {
            old_name: slug("acme"),
            new_name: slug("acme-two"),
        },
        r#"{"type":"renamed-to","data":{"old-name":"acme","new-name":"acme-two"}}"#,
    );
}

#[test]
fn an_adjacent_newtype_variant_holds_its_value_as_content() {
    round_trip(Change::Scored(7), r#"{"type":"scored","data":7}"#);
}

// Maps: JSON objects in key order; integer keys, and ids over integers, are decimal strings.

#[test]
fn maps_are_keyed_by_strings_ids_and_integers() {
    let seats = Seats {
        by_slug: BTreeMap::from([(slug("beta"), SeatCount(2)), (slug("acme"), SeatCount(5))]),
        by_ref: BTreeMap::from([(TenantRef(10), SeatCount(1)), (TenantRef(9), SeatCount(3))]),
        labels: BTreeMap::from([(2, "two".to_string())]),
    };
    let json = r#"{"by-slug":{"acme":5,"beta":2},"by-ref":{"9":3,"10":1},"labels":{"2":"two"}}"#;
    round_trip(seats.clone(), json);
    round_trip(
        Change::Moved(seats),
        &format!(r#"{{"type":"moved","data":{json}}}"#),
    );
}

#[test]
fn a_map_key_is_checked_against_its_type() {
    let bad = r#"{"by-slug":{"Acme":1},"by-ref":{},"labels":{}}"#;
    assert!(serde_json::from_str::<Seats>(bad).is_err());
    let bad = r#"{"by-slug":{},"by-ref":{"nine":1},"labels":{}}"#;
    assert!(serde_json::from_str::<Seats>(bad).is_err());
}

// A struct recursive through an option holds a box.

#[test]
fn a_struct_recurses_through_an_option() {
    round_trip(
        Chain {
            label: "a".to_string(),
            next: Some(Box::new(Chain {
                label: "b".to_string(),
                next: None,
            })),
        },
        r#"{"label":"a","next":{"label":"b","next":null}}"#,
    );
}

// Constraints: checked by `new` and on decoding; the value itself is written bare.

#[test]
fn a_constrained_id_is_written_as_its_value() {
    round_trip(slug("acme-2"), r#""acme-2""#);
    round_trip(Token::new(vec![1, 2, 3, 4]).unwrap(), "[1,2,3,4]");
}

#[test]
fn constraints_refuse_values_that_break_them() {
    let broken = |text: &str| Slug::new(text.to_string()).unwrap_err().constraint;
    assert_eq!(broken(""), "minLength");
    assert_eq!(broken(&"a".repeat(33)), "maxLength");
    assert_eq!(broken("Acme"), "pattern");
    assert_eq!(
        Token::new(vec![1, 2, 3]).unwrap_err(),
        TypeConstraintViolation {
            type_id: "token",
            constraint: "minLength"
        }
    );
    assert!(serde_json::from_str::<Slug>(r#""9lives""#).is_err());
    assert!(serde_json::from_str::<Token>("[1,2,3,4,5]").is_err());
    assert!(Slug("Bad".to_string()).validate().is_err());
}
