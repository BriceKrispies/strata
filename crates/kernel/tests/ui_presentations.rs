//! Presentations (D-8250): the display type of each field of an object type, held as an interface
//! element and checked against the object type when it is defined (`ui.binding.*`).

use std::collections::BTreeMap;
use strata_kernel::testkit::*;
use strata_kernel::*;

fn define(id: &str, presentation: UiPresentation) -> Intent {
    Intent::ChangeSchema {
        changes: vec![SchemaChange::DefineUiElement {
            id: id.to_string(),
            element: UiElement::Presentation(presentation),
        }],
        expected_version: None,
        existing: ExistingData::Reject,
    }
}

fn shown(display: UiDisplayType) -> UiFieldDisplay {
    UiFieldDisplay {
        display,
        props: None,
    }
}

fn presentation(
    object: &str,
    title: Option<&str>,
    fields: &[(&str, UiDisplayType)],
) -> UiPresentation {
    UiPresentation {
        object: self::object(object),
        title: title.map(field),
        fields: fields
            .iter()
            .map(|(name, display)| (field(name), shown(*display)))
            .collect::<BTreeMap<_, _>>(),
    }
}

fn run(intent: Intent) -> Result<World, DomainError> {
    notes_world()
        .step(&intent, &ctx("alpha", user("alpha:owner"), 9_000))
        .map(|(world, _)| world)
}

fn refusal(intent: Intent) -> String {
    format!("{:?}", run(intent).expect_err("refused"))
}

/// A presentation of an existing object type, naming its fields with display types that apply to
/// them, and the system attributes `created_by` and `created_at`, is accepted.
#[test]
fn a_presentation_that_fits_its_object_type_is_accepted() {
    let fitting = presentation(
        "note",
        Some("title"),
        &[
            ("title", UiDisplayType::Text),
            ("rank", UiDisplayType::Count),
            ("author", UiDisplayType::Reference),
            ("created_by", UiDisplayType::Person),
            ("created_at", UiDisplayType::Datetime),
        ],
    );
    run(define("notes_note", fitting)).expect("accepted");
}

/// A presentation of an object type the tenant does not have is refused.
#[test]
fn a_presentation_of_an_unknown_object_type_is_refused() {
    let text = refusal(define("notes_none", presentation("nothing", None, &[])));
    assert!(text.contains("ui.binding.unknown-object"), "{text}");
}

/// A field the object type does not have is refused.
#[test]
fn a_presentation_naming_an_unknown_field_is_refused() {
    let text = refusal(define(
        "notes_note",
        presentation("note", None, &[("nope", UiDisplayType::Text)]),
    ));
    assert!(text.contains("ui.binding.unknown-field"), "{text}");
    let text = refusal(define(
        "notes_note",
        presentation("note", Some("nope"), &[]),
    ));
    assert!(text.contains("ui.binding.unknown-field"), "{text}");
}

/// A display type that does not apply to the field's type is refused: a person is a string, a
/// count an integer, and `created_by` is a person and nothing else.
#[test]
fn a_display_type_that_does_not_fit_the_field_type_is_refused() {
    for (name, display) in [
        ("rank", UiDisplayType::Person),
        ("title", UiDisplayType::Count),
        ("created_by", UiDisplayType::Text),
    ] {
        let text = refusal(define(
            "notes_note",
            presentation("note", None, &[(name, display)]),
        ));
        assert!(text.contains("ui.binding.display-type"), "{name}: {text}");
    }
}

/// A title taken from a field that is not a string is refused.
#[test]
fn a_title_that_is_not_a_string_field_is_refused() {
    let text = refusal(define("notes_note", presentation("note", Some("rank"), &[])));
    assert!(text.contains("ui.binding.title"), "{text}");
}

/// An object type has one presentation.
#[test]
fn two_presentations_of_one_object_type_are_refused() {
    let world = run(define("notes_a", presentation("note", None, &[]))).unwrap();
    let second = define("notes_b", presentation("note", None, &[]));
    let error = world
        .step(&second, &ctx("alpha", user("alpha:owner"), 9_100))
        .expect_err("refused");
    assert!(format!("{error:?}").contains("ui.binding.duplicate"));
}
