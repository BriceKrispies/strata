//! Visibility is decided on the server (D-8402, D-8404): what a reader is served of a record, a row,
//! a count and a view, and what a write that names a view is told.
//!
//! The tenant is the notes world of the test kit: the public reads published notes only, writers
//! read every note, and the tests add a field whose read rule cannot be evaluated, a query over
//! the notes with a count, and a view over that query.

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::testkit::*;
use strata_kernel::*;

fn int(n: i64) -> Value {
    Value::Int(n)
}

/// A note by the owner with a `secret` field whose read rule follows the note's author, which the
/// note does not have: the rule cannot be evaluated for anyone but the system.
fn world() -> World {
    let unevaluable = eq(
        Expr::Through(Scope::Record, "author".into(), "name".into()),
        lit("anyone"),
    );
    let count = AggregateDef {
        id: AggregateId::new("count_all").unwrap(),
        kind: AggregateKind::Count,
        field: None,
        group_by: None,
        group_by_second: None,
        n: None,
        descending: false,
        scan_bound: 100,
    };
    let (world, _) = notes_world()
        .step(
            &Intent::ChangeSchema {
                changes: vec![
                    SchemaChange::DefineField {
                        object: object("note"),
                        field: FieldDef {
                            read: Some(unevaluable),
                            ..field_def("secret", FieldType::Str, false)
                        },
                    },
                    SchemaChange::DefineQuery(QueryDef {
                        id: QueryId::new("all_notes").unwrap(),
                        object: object("note"),
                        filter: None,
                        params: BTreeSet::new(),
                        order: vec![],
                        limit: None,
                        aggregates: Some(vec![count]),
                        declared_fields: Some(vec![]),
                        tree: None,
                    }),
                    SchemaChange::DefineView(ViewDef {
                        id: ViewId::new("desk").unwrap(),
                        route: "/desk".into(),
                        title: "Desk".into(),
                        sections: BTreeMap::from([(
                            "notes".to_string(),
                            ViewSection {
                                query: QueryId::new("all_notes").unwrap(),
                                args: BTreeMap::new(),
                                include: vec![],
                                markdown: vec![],
                                single: false,
                                aggregate: None,
                                marks: None,
                                joins: None,
                            },
                        )]),
                    }),
                ],
                expected_version: None,
                existing: ExistingData::Reject,
            },
            &ctx("alpha", user("alpha:owner"), 3_000),
        )
        .expect("the schema is accepted");
    let (world, _) = world
        .step(
            &Intent::CreateRecord {
                object: object("note"),
                fields: BTreeMap::from([
                    (field("title"), text("Draft")),
                    (field("slug"), text("draft")),
                    (field("secret"), text("hidden")),
                ]),
            },
            &ctx("alpha", user("alpha:owner"), 5_000),
        )
        .expect("the note is created");
    world
}

fn read_as(world: &World, who: Principal, request: ReadRequest) -> Vec<std::sync::Arc<Record>> {
    let state = world.state(&tenant_id("alpha"));
    read(&state, &request, &ctx("alpha", who, 7_000))
        .expect("the read is answered")
        .rows
}

fn all_notes() -> ReadRequest {
    ReadRequest::Named {
        query: QueryId::new("all_notes").unwrap(),
        params: BTreeMap::new(),
        page_size: None,
        cursor: None,
    }
}

// Verifies: ADM-1609
#[test]
fn a_row_the_reader_may_not_read_and_a_field_that_cannot_be_decided_are_not_served() {
    let world = world();
    let state = world.state(&tenant_id("alpha"));
    let id = state
        .tenant
        .as_ref()
        .unwrap()
        .records_of(&object("note"))
        .next()
        .unwrap()
        .id;
    let by_id = ReadRequest::Records {
        object: object("note"),
        ids: vec![id],
    };

    // The public may not read a draft: no row, by the record read and by the named query, and the
    // count the reader is given does not include it either.
    assert!(read_as(&world, Principal::Anonymous, by_id.clone()).is_empty());
    assert!(read_as(&world, Principal::Anonymous, all_notes()).is_empty());
    let counted = aggregates::run(
        &state,
        &AggregateRequest {
            query: QueryId::new("all_notes").unwrap(),
            aggregate: AggregateId::new("count_all").unwrap(),
            params: BTreeMap::new(),
            filter: BTreeMap::new(),
        },
        &ctx("alpha", Principal::Anonymous, 7_000),
    )
    .expect("the count is answered");
    assert!(
        counted
            .groups
            .iter()
            .all(|group| group.value != Some(int(1))),
        "the public is not told of the draft by a count: {:?}",
        counted.groups
    );

    // A writer reads the note, and the field whose rule cannot be evaluated is withheld rather
    // than returned; the fields that are not ruled are served.
    for request in [by_id, all_notes()] {
        let rows = read_as(&world, user("alpha:writer1"), request);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get(&field("title")), &text("Draft"));
        assert!(
            !rows[0].fields.contains_key(&field("secret")),
            "a field whose read rule cannot be evaluated is withheld"
        );
    }
}

// Verifies: ADM-1612
#[test]
fn a_view_confers_no_read_and_cannot_be_written() {
    let world = world();
    // The section of the view reads the notes as the invoker: the public gets none of the draft, a
    // writer gets it. Nothing the view says widens that.
    assert!(read_as(&world, Principal::Anonymous, all_notes()).is_empty());
    assert_eq!(read_as(&world, user("alpha:writer1"), all_notes()).len(), 1);

    // A write that names the view is refused as a write to a view, for a caller who holds every
    // permission and for one who holds none, whatever the intent.
    for who in [
        user("alpha:owner"),
        user("alpha:writer1"),
        Principal::Anonymous,
    ] {
        for intent in [
            Intent::CreateRecord {
                object: object("desk"),
                fields: BTreeMap::new(),
            },
            Intent::UpdateRecord {
                object: object("desk"),
                id: RecordId(1),
                fields: BTreeMap::new(),
                expected_version: None,
            },
            Intent::DeleteRecord {
                object: object("desk"),
                id: RecordId(1),
                expected_version: None,
            },
        ] {
            let refused = world
                .step(&intent, &ctx("alpha", who.clone(), 8_000))
                .expect_err("a view cannot be written");
            assert_eq!(refused.kind, ErrorKind::ViewReadOnly, "{who:?}: {intent:?}");
        }
    }

    // A name that is neither an object type nor a view is still not found for a caller who may
    // act on it, and the refusal for lack of permission otherwise: the view refusal tells nothing
    // about other names.
    let missing = world
        .step(
            &Intent::CreateRecord {
                object: object("nothing"),
                fields: BTreeMap::new(),
            },
            &ctx("alpha", user("alpha:owner"), 8_000),
        )
        .expect_err("there is no such type");
    assert_eq!(missing.kind, ErrorKind::UnknownObject);
}
