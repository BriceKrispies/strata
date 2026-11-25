//! A record type declared append-only: no update and no delete of a record of it, by any path,
//! for anyone (D-8422, OBS-3128). One rule per test.

use std::collections::BTreeMap;
use strata_kernel::testkit::*;
use strata_kernel::*;

/// The notes world with `note` declared append-only and note records 1 and 2 appended.
fn world() -> World {
    let world = notes_world();
    let owner = user("alpha:owner");
    let world = world
        .step(
            &Intent::ChangeSchema {
                changes: vec![SchemaChange::SetAppendOnly {
                    object: object("note"),
                }],
                expected_version: None,
                existing: ExistingData::Reject,
            },
            &ctx("alpha", owner.clone(), 2_000),
        )
        .expect("the declaration is accepted")
        .0;
    let world = world
        .step(
            &new_note("First", "first"),
            &ctx("alpha", owner.clone(), 2_001),
        )
        .expect("an append is accepted")
        .0;
    world
        .step(&new_note("Second", "second"), &ctx("alpha", owner, 2_002))
        .expect("an append is accepted")
        .0
}

fn refused(world: &World, who: Principal, intent: &Intent) -> DomainError {
    world
        .step(intent, &ctx("alpha", who, 3_000))
        .expect_err("the change is refused")
}

fn update(id: u64) -> Intent {
    Intent::UpdateRecord {
        object: object("note"),
        id: RecordId(id),
        fields: BTreeMap::from([(field("title"), text("Rewritten"))]),
        expected_version: None,
    }
}

fn delete(id: u64) -> Intent {
    Intent::DeleteRecord {
        object: object("note"),
        id: RecordId(id),
        expected_version: None,
    }
}

/// Verifies: OBS-3128
///
/// The owner and an editor alike are refused an update and a delete with `AppendOnly`, a record
/// is still appended, and a type that is not declared append-only is unchanged by the rule.
#[test]
fn nobody_updates_or_deletes_a_record_of_an_append_only_type() {
    let world = world();
    for who in [user("alpha:owner"), user("alpha:editor")] {
        for intent in [update(1), delete(1)] {
            let error = refused(&world, who.clone(), &intent);
            assert_eq!(error.kind, ErrorKind::AppendOnly, "{who:?} {intent:?}");
        }
    }
    let held = &world.tenants[&tenant_id("alpha")];
    assert_eq!(
        held.records[&object("note")].len(),
        2,
        "nothing was removed"
    );
    assert_eq!(
        held.records[&object("note")][&RecordId(1)]
            .get(&field("title"))
            .clone(),
        text("First"),
        "nothing was rewritten"
    );
    // A person is a type that is not append-only: it changes as before.
    world
        .step(
            &Intent::CreateRecord {
                object: object("person"),
                fields: BTreeMap::from([(field("name"), text("Ada"))]),
            },
            &ctx("alpha", user("alpha:editor"), 3_100),
        )
        .expect("an ordinary type is appended to")
        .0
        .step(
            &Intent::UpdateRecord {
                object: object("person"),
                id: RecordId(3),
                fields: BTreeMap::from([(field("name"), text("Ada L."))]),
                expected_version: None,
            },
            &ctx("alpha", user("alpha:editor"), 3_101),
        )
        .expect("an ordinary type is updated as before");
}

/// Verifies: OBS-3128
///
/// A path other than the plain intents is refused too: removing the type with its records is a
/// delete of each record, and the refusal leaves the type and its records as they were.
#[test]
fn removing_the_type_with_its_records_is_a_delete_and_is_refused() {
    let owner = user("alpha:owner");
    let world = notes_world()
        .step(
            &Intent::ChangeSchema {
                changes: vec![
                    SchemaChange::DefineObject {
                        id: object("entry"),
                        label: "Entry".into(),
                    },
                    SchemaChange::DefineField {
                        object: object("entry"),
                        field: field_def("label", FieldType::Str, true),
                    },
                    SchemaChange::SetAppendOnly {
                        object: object("entry"),
                    },
                ],
                expected_version: None,
                existing: ExistingData::Reject,
            },
            &ctx("alpha", owner.clone(), 2_000),
        )
        .expect("the type is declared")
        .0
        .step(
            &Intent::CreateRecord {
                object: object("entry"),
                fields: BTreeMap::from([(field("label"), text("one"))]),
            },
            &ctx("alpha", owner.clone(), 2_001),
        )
        .expect("an append is accepted")
        .0;
    let error = refused(
        &world,
        owner,
        &Intent::ChangeSchema {
            changes: vec![SchemaChange::RemoveObject {
                id: object("entry"),
                delete_records: true,
            }],
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    assert_eq!(error.kind, ErrorKind::AppendOnly, "{error:?}");
}
