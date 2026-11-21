//! The erasure of one data subject: what the kernel clears, keeps, removes and reports, and how
//! it works through many records. One rule per test.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use strata_kernel::testkit::*;
use strata_kernel::*;

// ---------------------------------------------------------------------------------------------
// Fixtures: the notes world, with record types that declare personal data. A schema change
// cannot declare retention, so the types are put into the schema of the tenant directly.
// ---------------------------------------------------------------------------------------------

fn personal(kind: PersonalKind, subject: DataSubject) -> Option<PersonalDeclaration> {
    Some(PersonalDeclaration {
        kind,
        subject,
        searchable: false,
    })
}

fn declared(
    id: &str,
    ty: FieldType,
    required: bool,
    kind: PersonalKind,
    subject: DataSubject,
) -> (FieldId, FieldDef) {
    let mut def = field_def(id, ty, required);
    def.personal = personal(kind, subject);
    (field(id), def)
}

fn plain(id: &str, ty: FieldType) -> (FieldId, FieldDef) {
    (field(id), field_def(id, ty, false))
}

fn record_type(id: &str, fields: Vec<(FieldId, FieldDef)>) -> ObjectTypeDef {
    ObjectTypeDef {
        id: object(id),
        label: id.to_string(),
        fields: fields.into_iter().collect(),
        constraints: BTreeMap::new(),
        retention: Some(RetentionRule {
            days: 30,
            from: RetentionStart::Created,
        }),
        publication: None,
        language_field: None,
        translation_group: None,
        unique: None,
        searchable: None,
    }
}

fn open_to_writers(schema: &mut TenantSchema, object_name: &str) {
    let id = PolicyId::new(format!("writers_use_{object_name}")).expect("policy id");
    schema.policies.insert(
        id.clone(),
        PolicyDef {
            id,
            role: role("writer"),
            resource: Resource::Object(object(object_name)),
            actions: BTreeSet::from([Action::Read, Action::Create]),
            condition: None,
        },
    );
}

/// Alpha gets the types:
/// - `profile`: `person` (plain text that names the subject), `name` (required, personal to the
///   person), `bio` and `city` (optional, personal to the person) and `plan` (not personal);
/// - `diary`: `entry` (required) and `mood` (optional), personal to the creator, and `topic`;
/// - `badge`: `who`, a required reference personal to the creator.
fn personal_world() -> World {
    let mut world = notes_world();
    let alpha = tenant_id("alpha");
    let state = TenantState::clone(&world.tenants[&alpha]);
    let mut schema = TenantSchema::clone(&state.schema);
    let by_person = || DataSubject::Field {
        field: field("person"),
    };
    schema.objects.insert(
        object("profile"),
        record_type(
            "profile",
            vec![
                plain("person", FieldType::Str),
                declared(
                    "name",
                    FieldType::Str,
                    true,
                    PersonalKind::Name,
                    by_person(),
                ),
                declared(
                    "bio",
                    FieldType::Str,
                    false,
                    PersonalKind::FreeText,
                    by_person(),
                ),
                declared(
                    "city",
                    FieldType::Str,
                    false,
                    PersonalKind::PostalAddress,
                    by_person(),
                ),
                plain("plan", FieldType::Str),
            ],
        ),
    );
    schema.objects.insert(
        object("diary"),
        record_type(
            "diary",
            vec![
                declared(
                    "entry",
                    FieldType::Str,
                    true,
                    PersonalKind::FreeText,
                    DataSubject::Creator,
                ),
                declared(
                    "mood",
                    FieldType::Str,
                    false,
                    PersonalKind::Other,
                    DataSubject::Creator,
                ),
                plain("topic", FieldType::Str),
            ],
        ),
    );
    schema.objects.insert(
        object("badge"),
        record_type(
            "badge",
            vec![declared(
                "who",
                FieldType::Ref {
                    target: object("person"),
                    on_delete: OnDelete::SetNull,
                },
                true,
                PersonalKind::Identifier,
                DataSubject::Creator,
            )],
        ),
    );
    for name in ["profile", "diary", "badge"] {
        open_to_writers(&mut schema, name);
    }
    assert!(schema.problems().is_empty(), "{:?}", schema.problems());
    let mut state = state;
    state.schema = Arc::new(schema);
    world.tenants.insert(alpha, Arc::new(state));
    world
}

fn at(
    world: &World,
    who: &str,
    now: i64,
    intent: Intent,
) -> Result<(World, Transition), DomainError> {
    world.step(&intent, &ctx("alpha", user(&format!("alpha:{who}")), now))
}

fn must(world: &World, who: &str, now: i64, intent: Intent) -> World {
    at(world, who, now, intent).expect("accepted").0
}

fn create(object_name: &str, fields: &[(&str, Value)]) -> Intent {
    Intent::CreateRecord {
        object: object(object_name),
        fields: fields.iter().map(|(k, v)| (field(k), v.clone())).collect(),
    }
}

fn profile(person: &str, name: &str) -> Intent {
    create(
        "profile",
        &[
            ("person", text(person)),
            ("name", text(name)),
            ("bio", text("a long personal biography")),
            ("city", text("Rue du Secret 7")),
            ("plan", text("free")),
        ],
    )
}

fn diary(entry: &str) -> Intent {
    create(
        "diary",
        &[
            ("entry", text(entry)),
            ("mood", text("private mood")),
            ("topic", text("weather")),
        ],
    )
}

fn erase_intent(subject: &str) -> Intent {
    Intent::EraseSubject {
        subject: principal_id(subject),
        resume_object: None,
        resume_after: None,
        held_objects: Vec::new(),
        addressed_invitations: Vec::new(),
    }
}

fn erased(outcome: &Transition) -> (&Vec<ErasedRecordOf>, bool) {
    match &outcome.output {
        Output::Erased { changed, more, .. } => (changed, *more),
        other => panic!("not an erasure: {other:?}"),
    }
}

type ErasedRecordOf = strata_kernel::erasure::ErasedRecord;

/// Run the erasure to its end, as the owner, following the continuation.
fn erase_all(mut world: World, subject: &str, mut now: i64) -> (World, Vec<Transition>) {
    let mut resume: (Option<ObjectTypeId>, Option<RecordId>) = (None, None);
    let mut all = Vec::new();
    loop {
        now += 1;
        let intent = Intent::EraseSubject {
            subject: principal_id(subject),
            resume_object: resume.0.clone(),
            resume_after: resume.1,
            held_objects: Vec::new(),
            addressed_invitations: Vec::new(),
        };
        let (next, outcome) = at(&world, "owner", now, intent).expect("accepted");
        world = next;
        let Output::Erased {
            more,
            next_object,
            next_after,
            ..
        } = outcome.output.clone()
        else {
            panic!("not an erasure");
        };
        all.push(outcome);
        if !more {
            return (world, all);
        }
        assert!(all.len() < 60, "the continuation does not end");
        resume = (next_object, next_after);
    }
}

// ---------------------------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------------------------

// Verifies: PRV-2461
#[test]
fn repeating_an_erasure_changes_nothing() {
    let world = personal_world();
    let world = must(&world, "owner", 100, profile("alpha:writer1", "Ada"));
    let world = must(&world, "writer1", 101, diary("my secret day"));
    let (world, _) = erase_all(world, "alpha:writer1", 200);
    let records = world.tenants[&tenant_id("alpha")].records.clone();
    let members = world.tenants[&tenant_id("alpha")].members.clone();
    let (again, outcome) =
        at(&world, "owner", 300, erase_intent("alpha:writer1")).expect("accepted");
    let (changed, more) = erased(&outcome);
    assert!(changed.is_empty() && !more);
    assert!(outcome.delta.ops.is_empty());
    assert!(outcome.events.is_empty());
    let state = &again.tenants[&tenant_id("alpha")];
    assert_eq!(state.records, records);
    assert_eq!(state.members, members);
}

// Verifies: PRV-1584
// Verifies: PRV-2745
#[test]
fn a_field_that_names_its_subject_by_a_reference_is_refused_at_declaration() {
    let mut schema = TenantSchema::clone(&personal_world().tenants[&tenant_id("alpha")].schema);
    schema.objects.insert(
        object("link"),
        record_type(
            "link",
            vec![
                plain(
                    "target",
                    FieldType::Ref {
                        target: object("profile"),
                        on_delete: OnDelete::SetNull,
                    },
                ),
                declared(
                    "remark",
                    FieldType::Str,
                    false,
                    PersonalKind::FreeText,
                    DataSubject::Field {
                        field: field("target"),
                    },
                ),
            ],
        ),
    );
    let problems = schema.problems();
    assert!(
        problems
            .iter()
            .any(|p| p.contains("link.remark") && p.contains("principal id held as text")),
        "{problems:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// Subject export, legal holds on record types, invitations addressed to the subject, retention.
// ---------------------------------------------------------------------------------------------
