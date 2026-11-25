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
    let text = refusal(define(
        "notes_note",
        presentation("note", Some("rank"), &[]),
    ));
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

// ---------------------------------------------------------------------------------------------
// Row actions (D-DS-32): a `row.*` argument is valid only on a row action, and `row.<field>` names
// a field of the object that the row's section reads.
// ---------------------------------------------------------------------------------------------

fn apply(world: &World, changes: Vec<SchemaChange>) -> Result<World, DomainError> {
    world
        .step(
            &Intent::ChangeSchema {
                changes,
                expected_version: None,
                existing: ExistingData::Reject,
            },
            &ctx("alpha", user("alpha:owner"), 9_000),
        )
        .map(|(world, _)| world)
}

/// The notes of alpha with a query, a view whose `notes` section reads them, a screen on that view
/// and an operation that deletes the note its `note` input names.
fn notes_screen() -> World {
    let mut world = notes_world();
    let steps = [
        vec![SchemaChange::DefineQuery(QueryDef {
            id: QueryId::new("all_notes").unwrap(),
            object: object("note"),
            filter: None,
            params: std::collections::BTreeSet::new(),
            order: vec![],
            limit: None,
            aggregates: None,
            declared_fields: None,
            tree: None,
        })],
        vec![SchemaChange::DefineView(ViewDef {
            id: ViewId::new("notes_view").unwrap(),
            route: "/notes".into(),
            title: "Notes".into(),
            sections: BTreeMap::from([(
                "notes".to_string(),
                ViewSection {
                    marks: None,
                    joins: None,
                    query: QueryId::new("all_notes").unwrap(),
                    args: BTreeMap::new(),
                    include: vec![],
                    markdown: vec![],
                    single: false,
                    aggregate: None,
                },
            )]),
        })],
        vec![SchemaChange::DefineOperation(TenantOperationDef {
            id: TenantOperationId::new("discard").unwrap(),
            label: "Discard".into(),
            inputs: vec![TenantOperationInput {
                name: "note".into(),
                ty: FieldType::Ref {
                    target: object("note"),
                    on_delete: OnDelete::Restrict,
                },
                required: true,
                label: None,
            }],
            invokers: TenantOperationInvokers {
                signed_in: true,
                roles: std::collections::BTreeSet::new(),
            },
            subject: None,
            conditions: vec![],
            steps: vec![TenantOperationStep::DeleteRecord {
                id: "gone".into(),
                object: object("note"),
                record: Expr::Param("note".into()),
            }],
            agent_policy: None,
        })],
        vec![SchemaChange::DefineUiElement {
            id: "notes_screen".into(),
            element: UiElement::Screen(UiScreen {
                route: "/notes".into(),
                view: ViewId::new("notes_view").unwrap(),
                title: "Notes".into(),
                kind: UiScreenKind::List,
                auth: UiAuth::Session,
                query: vec![],
            }),
        }],
    ];
    for changes in steps {
        world = apply(&world, changes).expect("the screen's parts are accepted");
    }
    world
}

/// A discard action on the notes screen whose `note` argument reads `source`, on the section
/// `row` (or on no row at all).
fn discard_on(id: &str, row: Option<&str>, source: &str) -> Vec<SchemaChange> {
    vec![SchemaChange::DefineUiElement {
        id: id.into(),
        element: UiElement::Action(UiAction {
            screen: "notes_screen".into(),
            label: "Discard".into(),
            operation: TenantOperationId::new("discard").unwrap(),
            form: None,
            row: row.map(str::to_string),
            arguments: BTreeMap::from([("note".to_string(), source.to_string())]),
            confirm: None,
            on_success: UiOutcome {
                toast: None,
                navigate: None,
            },
            on_error: UiErrorDisplay::Inline,
            when_refused: None,
        }),
    }]
}

/// A row action whose `note` argument is the row's id is accepted.
#[test]
fn a_row_action_reads_its_rows_id_and_fields() {
    let world = notes_screen();
    apply(&world, discard_on("discard_row", Some("notes"), "row.id")).expect("accepted");
    apply(
        &world,
        discard_on("discard_title", Some("notes"), "row.title"),
    )
    .expect("accepted");
}

/// A `row.*` argument on an action that names no row is refused: no row supplies it.
#[test]
fn a_row_argument_on_a_non_row_action_is_refused() {
    let world = notes_screen();
    let error = apply(&world, discard_on("loose", None, "row.id")).expect_err("refused");
    assert!(error.message.contains("not a row action"), "{error:?}");
}

/// A `row.<field>` argument naming a field the row's object does not have is refused.
#[test]
fn a_row_argument_naming_an_unknown_field_is_refused() {
    let world = notes_screen();
    let error = apply(&world, discard_on("typo", Some("notes"), "row.nope")).expect_err("refused");
    assert!(
        error.message.contains("not a field of the object of notes"),
        "{error:?}"
    );
}

/// A row that names a section the screen's view does not have is refused.
#[test]
fn a_row_naming_an_unknown_section_is_refused() {
    let world = notes_screen();
    let error =
        apply(&world, discard_on("missing", Some("missing"), "row.id")).expect_err("refused");
    assert!(error.message.contains("is not a section"), "{error:?}");
}
