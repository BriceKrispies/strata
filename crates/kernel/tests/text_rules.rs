//! Text in the tenant rule language and on write (WG-5597): the grapheme and emoji functions, the
//! length each of them counts, and normalisation on write, with a unique constraint over it.

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::expr::{self, Env};
use strata_kernel::normalise::normalised;
use strata_kernel::testkit::*;
use strata_kernel::*;

fn literal(text: &str) -> Expr {
    Expr::Lit(Value::Str(text.to_string()))
}

fn evaluate(expr: &Expr) -> Value {
    let params = BTreeMap::new();
    let roles = BTreeSet::new();
    let env = Env {
        record: None,
        old: None,
        new: None,
        params: &params,
        now: Timestamp(0),
        principal: &Principal::Anonymous,
        roles: &roles,
        lookup: None,
    };
    expr::eval(expr, &env).expect("the expression evaluates")
}

/// Each case: the text, whether it is exactly one emoji, and its extended grapheme count.
/// A text-default emoji character (U+00A9, U+2764) is an emoji only with VS16 or a skin tone
/// after it (the presentation a reader sees); the bare character is text, so it is not one.
const CASES: &[(&str, bool, i64)] = &[
    // emoji, one grapheme
    ("\u{1F600}", true, 1),
    ("\u{1F44D}", true, 1),
    ("\u{1F44D}\u{1F3FD}", true, 1),
    ("\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}", true, 1),
    ("\u{1F1EC}\u{1F1E7}", true, 1),
    ("1\u{FE0F}\u{20E3}", true, 1),
    ("1\u{20E3}", true, 1),
    ("*\u{FE0F}\u{20E3}", true, 1),
    ("#\u{FE0F}\u{20E3}", true, 1),
    ("\u{2764}\u{FE0F}", true, 1),
    ("\u{A9}\u{FE0F}", true, 1),
    ("\u{2122}\u{FE0F}", true, 1),
    ("\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}", true, 1),
    ("\u{1F469}\u{1F3FD}\u{200D}\u{1F4BB}", true, 1),
    (
        "\u{1F3F4}\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}",
        true,
        1,
    ),
    ("\u{1F9D1}\u{200D}\u{2695}\u{FE0F}", true, 1),
    ("\u{261D}\u{1F3FD}", true, 1),
    ("\u{1F3FD}", true, 1),
    // not one emoji: text, a text presentation, or a lone part
    ("\u{2764}", false, 1),
    ("\u{A9}", false, 1),
    ("\u{2122}", false, 1),
    ("\u{A9}\u{FE0E}", false, 1),
    ("\u{1F44D}\u{FE0E}", false, 1),
    ("\u{FE0F}", false, 1),
    ("\u{1F1EC}", false, 1),
    ("\u{1F1EC}\u{1F1E7}\u{1F1EB}\u{1F1F7}", false, 2),
    ("\u{1F600}\u{1F600}", false, 2),
    ("\u{1F600} ", false, 2),
    ("\u{1F600}\u{200D}", false, 1),
    ("a\u{1F44D}", false, 2),
    // letters, digits and symbols
    ("a", false, 1),
    ("A", false, 1),
    ("\u{E9}", false, 1),
    ("e\u{301}", false, 1),
    ("hello", false, 5),
    ("7", false, 1),
    ("42", false, 2),
    ("#", false, 1),
    ("*", false, 1),
    ("\u{D55C}", false, 1),
    ("\u{4E2D}", false, 1),
    (" ", false, 1),
    ("", false, 0),
];

#[test]
fn is_emoji_and_graphemes_hold_for_each_case_of_the_table() {
    assert!(CASES.len() >= 30, "the table has {} cases", CASES.len());
    for &(text, emoji, count) in CASES {
        assert_eq!(
            evaluate(&Expr::IsEmoji(Box::new(literal(text)))),
            Value::Bool(emoji),
            "is_emoji of {text:?}"
        );
        assert_eq!(
            evaluate(&Expr::Graphemes(Box::new(literal(text)))),
            Value::Int(count),
            "graphemes of {text:?}"
        );
    }
}

#[test]
fn len_still_counts_code_points_where_graphemes_counts_the_clusters() {
    let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
    assert_eq!(
        evaluate(&Expr::Len(Box::new(literal(family)))),
        Value::Int(5)
    );
    assert_eq!(
        evaluate(&Expr::Graphemes(Box::new(literal(family)))),
        Value::Int(1)
    );
    assert_eq!(
        evaluate(&Expr::Len(Box::new(literal("e\u{301}")))),
        Value::Int(2)
    );
    assert_eq!(
        evaluate(&Expr::Graphemes(Box::new(literal("e\u{301}")))),
        Value::Int(1)
    );
}

#[test]
fn an_emoji_field_stores_the_vs16_form_of_either_spelling() {
    let form = |text: &str| normalised(Normalisation::Emoji, text);
    assert_eq!(form("\u{2764}"), "\u{2764}\u{FE0F}");
    assert_eq!(form("\u{2764}\u{FE0F}"), "\u{2764}\u{FE0F}");
    assert_eq!(form("\u{A9}"), "\u{A9}\u{FE0F}");
    assert_eq!(form("1\u{20E3}"), "1\u{FE0F}\u{20E3}");
    // A skin tone modifier sequence is fully qualified without VS16 (UTS #51): both spellings of
    // it store the sequence without VS16.
    assert_eq!(form("\u{261D}\u{1F3FD}"), "\u{261D}\u{1F3FD}");
    assert_eq!(form("\u{261D}\u{FE0F}\u{1F3FD}"), "\u{261D}\u{1F3FD}");
    assert_eq!(form("\u{1F44D}\u{1F3FD}"), "\u{1F44D}\u{1F3FD}");
    // Text that is not one emoji is stored as written: no VS16 is added to it.
    assert_eq!(form("hello"), "hello");
    assert_eq!(form("1"), "1");
    assert_eq!(form("\u{2764}\u{2764}"), "\u{2764}\u{2764}");
}

#[test]
fn an_nfc_field_stores_composed_text() {
    assert_eq!(normalised(Normalisation::Nfc, "e\u{301}"), "\u{E9}");
    assert_eq!(normalised(Normalisation::Nfc, "\u{E9}"), "\u{E9}");
    assert_eq!(normalised(Normalisation::Nfc, "hello"), "hello");
}

fn step(
    world: &World,
    who: &str,
    now: i64,
    intent: Intent,
) -> Result<(World, Transition), DomainError> {
    world.step(&intent, &ctx("alpha", user(&format!("alpha:{who}")), now))
}

fn must(world: &World, who: &str, now: i64, intent: Intent) -> World {
    step(world, who, now, intent).expect("accepted").0
}

/// A reaction object whose `emoji` field is unique, and which normalises to the emoji form when
/// `normalise` is set.
fn reactions_with(normalise: Option<Normalisation>) -> World {
    let changes = vec![
        SchemaChange::DefineObject {
            id: object("reaction"),
            label: "Reaction".into(),
        },
        SchemaChange::DefineField {
            object: object("reaction"),
            field: FieldDef {
                unique: true,
                normalise,
                ..field_def("emoji", FieldType::Str, true)
            },
        },
    ];
    must(
        &notes_world(),
        "owner",
        8_000,
        Intent::ChangeSchema {
            changes,
            expected_version: None,
            existing: ExistingData::Reject,
        },
    )
}

fn react(emoji: &str) -> Intent {
    Intent::CreateRecord {
        object: object("reaction"),
        fields: BTreeMap::from([(field("emoji"), text(emoji))]),
    }
}

/// The stored values of the reactions of alpha, in record order.
fn stored(world: &World) -> Vec<Option<Value>> {
    world
        .state(&tenant_id("alpha"))
        .tenant
        .as_ref()
        .expect("alpha has a tenant")
        .records_of(&object("reaction"))
        .map(|record| record.fields.get(&field("emoji")).cloned())
        .collect()
}

#[test]
fn a_unique_emoji_field_stores_one_value_for_both_spellings_and_refuses_the_second() {
    let world = must(
        &reactions_with(Some(Normalisation::Emoji)),
        "owner",
        8_001,
        react("\u{2764}"),
    );
    assert_eq!(stored(&world), vec![Some(text("\u{2764}\u{FE0F}"))]);
    let error = step(&world, "owner", 8_002, react("\u{2764}\u{FE0F}"))
        .expect_err("the second spelling is refused");
    assert_eq!(error.kind, ErrorKind::UniqueViolated);
    // Another emoji is another value.
    let world = must(&world, "owner", 8_003, react("\u{1F44D}"));
    assert_eq!(stored(&world).len(), 2);
}

#[test]
fn without_normalisation_the_two_spellings_are_two_values() {
    let world = must(&reactions_with(None), "owner", 8_100, react("\u{2764}"));
    let world = must(&world, "owner", 8_101, react("\u{2764}\u{FE0F}"));
    assert_eq!(
        stored(&world),
        vec![Some(text("\u{2764}")), Some(text("\u{2764}\u{FE0F}"))]
    );
}
