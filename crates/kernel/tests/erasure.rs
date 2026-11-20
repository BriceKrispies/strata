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
/// - `link`: `target` (a reference, so it cannot name a principal) and `remark`, personal to it;
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
        object("link"),
        record_type(
            "link",
            vec![
                plain(
                    "target",
                    FieldType::Ref {
                        target: object("person"),
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
    for name in ["profile", "diary", "link", "badge"] {
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

fn stored(world: &World, object_name: &str, id: u64) -> Arc<Record> {
    world.tenants[&tenant_id("alpha")]
        .record(&object(object_name), RecordId(id))
        .expect("the record exists")
        .clone()
}

fn erase_intent(subject: &str) -> Intent {
    Intent::EraseSubject {
        subject: principal_id(subject),
        resume_object: None,
        resume_after: None,
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

// Verifies: PRV-2460
#[test]
fn personal_fields_are_cleared_and_the_others_kept() {
    let world = personal_world();
    let world = must(
        &world,
        "owner",
        100,
        profile("alpha:writer1", "Ada Lovelace"),
    );
    let before = stored(&world, "profile", 1);
    let (world, outcomes) = erase_all(world, "alpha:writer1", 200);
    assert_eq!(outcomes.len(), 1);
    let (changed, more) = erased(&outcomes[0]);
    assert!(!more);
    assert_eq!(changed.len(), 1);
    let fields: BTreeSet<&str> = changed[0].fields.iter().map(|f| f.as_str()).collect();
    assert_eq!(fields, BTreeSet::from(["name", "bio", "city"]));

    let after = stored(&world, "profile", 1);
    // Optional personal fields are gone; the required one holds the empty value of its type.
    assert!(after.get(&field("bio")).is_null());
    assert!(after.get(&field("city")).is_null());
    assert_eq!(after.get(&field("name")), &text(""));
    // What no declaration names stays, the field that names the subject included.
    assert_eq!(after.get(&field("plan")), &text("free"));
    assert_eq!(after.get(&field("person")), &text("alpha:writer1"));
    assert_eq!(after.version, before.version + 1);
    assert_eq!(after.created_by, before.created_by);
}

// Verifies: PRV-2460
#[test]
fn a_record_of_another_person_is_untouched() {
    let world = personal_world();
    let world = must(&world, "owner", 100, profile("alpha:writer1", "Ada"));
    let world = must(&world, "owner", 101, profile("alpha:writer2", "Grace"));
    let theirs = stored(&world, "profile", 2);
    let (world, _) = erase_all(world, "alpha:writer1", 200);
    assert_eq!(stored(&world, "profile", 2), theirs);
    assert_eq!(
        stored(&world, "profile", 2).get(&field("name")),
        &text("Grace")
    );
}

// Verifies: PRV-2460
#[test]
fn what_a_person_created_is_cleared_and_unlinked_and_what_others_created_is_not() {
    let world = personal_world();
    let world = must(&world, "writer1", 100, diary("my secret day"));
    let world = must(&world, "writer2", 101, diary("another secret day"));
    let theirs = stored(&world, "diary", 2);
    let (world, _) = erase_all(world, "alpha:writer1", 200);
    let mine = stored(&world, "diary", 1);
    assert_eq!(mine.get(&field("entry")), &text(""));
    assert!(mine.get(&field("mood")).is_null());
    assert_eq!(mine.get(&field("topic")), &text("weather"));
    assert_eq!(mine.created_by, None);
    assert_eq!(stored(&world, "diary", 2), theirs);
}

// Verifies: PRV-2462
#[test]
fn the_events_of_an_erasure_carry_no_erased_value() {
    let world = personal_world();
    let world = must(
        &world,
        "owner",
        100,
        profile("alpha:writer1", "Ada Lovelace"),
    );
    let world = must(&world, "writer1", 101, diary("my secret day"));
    let (_, outcomes) = erase_all(world, "alpha:writer1", 200);
    let events: Vec<&DomainEvent> = outcomes.iter().flat_map(|o| &o.events).collect();
    assert!(events
        .iter()
        .any(|e| matches!(e.body, EventBody::RecordUpdated { .. })));
    let written = serde_json::to_string(&events).expect("events encode");
    for value in [
        "Ada Lovelace",
        "a long personal biography",
        "Rue du Secret 7",
        "my secret day",
        "private mood",
    ] {
        assert!(!written.contains(value), "an event carries {value:?}");
    }
    // What is not personal may be there: the plan stays in the images.
    assert!(written.contains("free"));
}

#[test]
fn roles_teams_and_invitations_of_the_person_are_removed_and_the_sessions_end() {
    let mut world = personal_world();
    let team = TeamId::new("crew").expect("team id");
    world = must(
        &world,
        "owner",
        100,
        Intent::DefineTeam {
            team: team.clone(),
            label: "Crew".into(),
        },
    );
    world = must(
        &world,
        "owner",
        101,
        Intent::AddTeamMember {
            team: team.clone(),
            principal: principal_id("alpha:writer1"),
        },
    );
    // A second owner issues an invitation and is then erased: the offer is withdrawn.
    world = must(
        &world,
        "owner",
        102,
        Intent::AssignRole {
            principal: principal_id("alpha:writer2"),
            role: owner_role(),
        },
    );
    let invitation = InvitationId::new("inv-1").expect("invitation id");
    world = must(
        &world,
        "writer2",
        103,
        Intent::CreateInvitation {
            invitation: invitation.clone(),
            address: "new@example.org".into(),
            roles: BTreeSet::from([role("writer")]),
        },
    );

    let (after, outcomes) = erase_all(world.clone(), "alpha:writer1", 200);
    let state = &after.tenants[&tenant_id("alpha")];
    assert!(!state.members.contains_key(&principal_id("alpha:writer1")));
    assert!(!state
        .schema
        .team(&team)
        .expect("team")
        .members
        .contains(&principal_id("alpha:writer1")));
    assert!(outcomes[0].effects.contains(&Effect::RevokeSessions {
        principal: principal_id("alpha:writer1")
    }));
    let Output::Erased {
        member_removed,
        teams,
        ..
    } = &outcomes[0].output
    else {
        panic!("not an erasure");
    };
    assert!(*member_removed);
    assert_eq!(teams, &vec![team]);

    let (after, outcomes) = erase_all(world, "alpha:writer2", 300);
    let state = &after.tenants[&tenant_id("alpha")];
    assert!(!state.members.contains_key(&principal_id("alpha:writer2")));
    let offer = state.schema.invitation(&invitation).expect("invitation");
    assert_eq!(offer.revoked_by, Some(principal_id("alpha:owner")));
    let Output::Erased { invitations, .. } = &outcomes[0].output else {
        panic!("not an erasure");
    };
    assert_eq!(invitations, &vec![invitation]);
}

#[test]
fn the_last_owner_is_not_erased() {
    let world = personal_world();
    let error = at(&world, "owner", 100, erase_intent("alpha:owner")).expect_err("refused");
    assert_eq!(error.kind, ErrorKind::InvalidIntent);
    assert!(error.evidence.iter().any(|c| !c.passed));
}

#[test]
fn only_an_owner_erases_a_subject() {
    let world = personal_world();
    let error = at(&world, "writer1", 100, erase_intent("alpha:writer2")).expect_err("refused");
    assert_eq!(error.kind, ErrorKind::Unauthorized);
    let error = world
        .step(
            &erase_intent("alpha:writer2"),
            &ctx("alpha", Principal::Anonymous, 101),
        )
        .expect_err("refused");
    assert_eq!(error.kind, ErrorKind::Unauthorized);
}

// Verifies: PRV-2461
#[test]
fn a_decision_changes_at_most_one_batch_and_says_where_to_continue() {
    let mut world = personal_world();
    let total = erasure::BATCH + 50;
    for i in 0..total {
        world = must(
            &world,
            "owner",
            100 + i as i64,
            profile("alpha:writer1", &format!("Person {i}")),
        );
    }
    let (world, outcomes) = erase_all(world, "alpha:writer1", 10_000);
    assert!(outcomes.len() >= 2);
    let mut seen = BTreeSet::new();
    for outcome in &outcomes {
        let (changed, _) = erased(outcome);
        assert!(changed.len() <= erasure::BATCH);
        for record in changed {
            assert!(seen.insert(record.id), "a record is changed twice");
        }
    }
    assert_eq!(seen.len(), total);
    assert!(erased(&outcomes[0]).1, "the first batch says more remain");
    assert!(!erased(outcomes.last().expect("a batch")).1);
    for id in 1..=total as u64 {
        assert_eq!(stored(&world, "profile", id).get(&field("name")), &text(""));
    }
}

// Verifies: PRV-2461
#[test]
fn the_records_a_person_created_are_found_by_a_page_scan_that_continues() {
    let mut world = personal_world();
    let total = erasure::BATCH * 2 + 30;
    for i in 0..total {
        let who = if i % 3 == 0 { "writer2" } else { "writer1" };
        world = must(&world, who, 100 + i as i64, diary(&format!("entry {i}")));
    }
    let mine = (0..total).filter(|i| i % 3 != 0).count();
    let (world, outcomes) = erase_all(world, "alpha:writer1", 10_000);
    assert!(outcomes.len() >= 3, "{} decisions", outcomes.len());
    let changed: usize = outcomes.iter().map(|o| erased(o).0.len()).sum();
    assert_eq!(changed, mine);
    for i in 0..total {
        let record = stored(&world, "diary", i as u64 + 1);
        if i % 3 == 0 {
            assert_eq!(
                record.get(&field("entry")),
                &text(format!("entry {i}").as_str())
            );
            assert!(record.created_by.is_some());
        } else {
            assert_eq!(record.get(&field("entry")), &text(""));
            assert!(record.created_by.is_none());
        }
    }
}

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

#[test]
fn another_tenant_is_untouched() {
    let world = personal_world();
    let world = must(&world, "owner", 100, profile("alpha:writer1", "Ada"));
    let beta = world.tenant_hash(&tenant_id("beta"));
    let (world, _) = erase_all(world, "alpha:writer1", 200);
    assert_eq!(world.tenant_hash(&tenant_id("beta")), beta);
    // The same id in the other tenant names another person, who is not a member there either.
    let (_, outcome) = world
        .step(
            &erase_intent("alpha:writer1"),
            &ctx("beta", user("beta:owner"), 300),
        )
        .expect("accepted");
    assert!(erased(&outcome).0.is_empty());
}

#[test]
fn what_cannot_be_erased_is_reported() {
    let world = personal_world();
    let world = must(
        &world,
        "owner",
        100,
        Intent::CreateRecord {
            object: object("person"),
            fields: BTreeMap::from([(field("name"), text("Ada"))]),
        },
    );
    let world = must(
        &world,
        "writer1",
        101,
        create("badge", &[("who", Value::Ref(RecordId(1)))]),
    );
    let (world, outcomes) = erase_all(world, "alpha:writer1", 200);
    let Output::Erased { not_erased, .. } = &outcomes[0].output else {
        panic!("not an erasure");
    };
    assert!(
        not_erased
            .iter()
            .any(|note| note.starts_with("held badge.who")),
        "{not_erased:?}"
    );
    assert!(
        not_erased
            .iter()
            .any(|note| note.starts_with("unresolved link.target")),
        "{not_erased:?}"
    );
    // The record that cannot be changed is left as it was.
    assert_eq!(
        stored(&world, "badge", 2).get(&field("who")),
        &Value::Ref(RecordId(1))
    );
}
