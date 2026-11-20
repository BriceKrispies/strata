//! The wire forms specc's unions, maps and constrained types serialise to: each value is written
//! to the exact JSON text serde produces for its encoding, and read back from that text to an
//! equal value. The types are generated from
//! `crates/spec-compiler/tests/fixtures/core/spec.jsonc`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;

use serde::de::DeserializeOwned;
use serde::Serialize;
use strata_spec_compiler_core_fixture::types::{
    AccountId, Chain, Change, Cmp, Cond, Expr, Owner, SeatCount, Seats, Slug, TenantRef, Token,
    TypeConstraintViolation, Value,
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
        tags: BTreeSet::new(),
        groups: BTreeMap::new(),
        nested: BTreeMap::new(),
    };
    let json = r#"{"by-slug":{"acme":5,"beta":2},"by-ref":{"9":3,"10":1},"labels":{"2":"two"},"tags":[],"groups":{},"nested":{}}"#;
    round_trip(seats.clone(), json);
    round_trip(
        Change::Moved(seats),
        &format!(r#"{{"type":"moved","data":{json}}}"#),
    );
}

#[test]
fn a_map_key_is_checked_against_its_type() {
    let bad = r#"{"by-slug":{"Acme":1},"by-ref":{},"labels":{},"tags":[],"groups":{},"nested":{}}"#;
    assert!(serde_json::from_str::<Seats>(bad).is_err());
    let bad = r#"{"by-slug":{},"by-ref":{"nine":1},"labels":{},"tags":[],"groups":{},"nested":{}}"#;
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
                marks: None,
            })),
            marks: None,
        },
        r#"{"label":"a","next":{"label":"b","next":null}}"#,
    );
}

// Sets (WG-4956): a JSON array of distinct items in ascending order; decoding refuses a repeat,
// also inside an option, a map and a tuple variant.

fn seats_with(tags: &[&str], groups: &[(&str, &[u32])]) -> Seats {
    Seats {
        by_slug: BTreeMap::new(),
        by_ref: BTreeMap::new(),
        labels: BTreeMap::new(),
        tags: tags.iter().map(|t| slug(t)).collect(),
        groups: groups
            .iter()
            .map(|(k, v)| (k.to_string(), v.iter().copied().collect()))
            .collect(),
        nested: BTreeMap::new(),
    }
}

#[test]
fn a_set_is_an_array_of_distinct_items_in_order() {
    round_trip(
        seats_with(&["beta", "acme"], &[("x", &[3, 1])]),
        r#"{"by-slug":{},"by-ref":{},"labels":{},"tags":["acme","beta"],"groups":{"x":[1,3]},"nested":{}}"#,
    );
    round_trip(
        Chain {
            label: "a".to_string(),
            next: None,
            marks: Some(BTreeSet::from(["m".to_string()])),
        },
        r#"{"label":"a","next":null,"marks":["m"]}"#,
    );
}

#[test]
fn a_set_that_repeats_an_item_is_refused() {
    let repeated =
        r#"{"by-slug":{},"by-ref":{},"labels":{},"tags":["acme","acme"],"groups":{},"nested":{}}"#;
    let error = serde_json::from_str::<Seats>(repeated)
        .unwrap_err()
        .to_string();
    assert!(error.contains("repeats"), "{error}");
    let in_a_map =
        r#"{"by-slug":{},"by-ref":{},"labels":{},"tags":[],"groups":{"x":[1,1]},"nested":{}}"#;
    assert!(serde_json::from_str::<Seats>(in_a_map).is_err());
    assert!(
        serde_json::from_str::<Chain>(r#"{"label":"a","next":null,"marks":["m","m"]}"#).is_err()
    );
    assert!(serde_json::from_str::<Chain>(r#"{"label":"a","next":null}"#).is_ok());
    assert!(serde_json::from_str::<Cond>(r#"{"AnyOf":[["acme","acme"],null]}"#).is_err());
}

// Tuple variants (WG-4956): {"Variant": [values]}, under the external and the adjacent tag.

#[test]
fn a_tuple_variant_is_an_array_of_its_values() {
    let leaf = |b: bool| Box::new(Cond::Leaf(b));
    round_trip(
        Cond::Cmp(Cmp::Eq, leaf(true), leaf(false)),
        r#"{"Cmp":["Eq",{"Leaf":true},{"Leaf":false}]}"#,
    );
    round_trip(
        Cond::AnyOf(BTreeSet::from([slug("b"), slug("a")]), None),
        r#"{"AnyOf":[["a","b"],null]}"#,
    );
    round_trip(
        Change::Swapped(slug("a"), slug("b")),
        r#"{"type":"swapped","data":["a","b"]}"#,
    );
}

#[test]
fn a_tuple_variant_with_the_wrong_number_of_values_is_refused() {
    assert!(serde_json::from_str::<Cond>(r#"{"Cmp":["Eq",{"Leaf":true}]}"#).is_err());
    assert!(serde_json::from_str::<Cond>(r#"{"Cmp":["Ne",{"Leaf":true},{"Leaf":true}]}"#).is_err());
    assert!(serde_json::from_str::<Change>(r#"{"type":"swapped","data":["a"]}"#).is_err());
}

// A map of maps (WG-4956).

#[test]
fn a_map_of_maps_nests_json_objects() {
    let mut seats = seats_with(&[], &[]);
    seats.nested = BTreeMap::from([(slug("acme"), BTreeMap::from([(TenantRef(2), SeatCount(4))]))]);
    round_trip(
        seats,
        r#"{"by-slug":{},"by-ref":{},"labels":{},"tags":[],"groups":{},"nested":{"acme":{"2":4}}}"#,
    );
}

// uuid ids are in the pure crate (WG-4956), as uuid::Uuid strings.

#[test]
fn a_uuid_id_is_its_hyphenated_text() {
    let text = "67e55044-10b1-426f-9247-bb680e5fe0c8";
    round_trip(
        Owner {
            account: AccountId(text.parse().unwrap()),
            slug: slug("acme"),
        },
        &format!(r#"{{"account":"{text}","slug":"acme"}}"#),
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
