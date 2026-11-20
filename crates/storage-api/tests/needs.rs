//! A decision that needs many records is answered in a bounded number of loads.

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, query};

/// Loads one decision may spend however many references the record holds: the header, the first
/// load, the reload that adds every reference of the stored record, and one of slack.
const LOADS_PER_DECISION: usize = 4;

fn owner() -> Principal {
    user("owner")
}

fn run(store: &MemoryStore, who: Principal, now: i64, intent: Intent) -> Executed {
    execute(store, &intent, &ctx("acme", who, now)).expect("accepted")
}

fn found(store: &MemoryStore) {
    run(
        store,
        Principal::System,
        1,
        Intent::CreateTenant {
            name: "acme".into(),
            owner: principal_id("owner"),
        },
    );
}

fn define(store: &MemoryStore, now: i64, changes: Vec<SchemaChange>) {
    run(
        store,
        owner(),
        now,
        Intent::ChangeSchema {
            changes,
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
}

fn create(store: &MemoryStore, now: i64, object_name: &str, fields: &[(&str, Value)]) -> RecordId {
    let done = run(
        store,
        owner(),
        now,
        Intent::CreateRecord {
            object: object(object_name),
            fields: fields.iter().map(|(k, v)| (field(k), v.clone())).collect(),
        },
    );
    match done.transition.output {
        Output::Record { id, .. } => id,
        other => panic!("not a record: {other:?}"),
    }
}

fn reference(target: &str) -> FieldType {
    FieldType::Ref {
        target: object(target),
        on_delete: OnDelete::SetNull,
    }
}

#[test]
fn updating_one_field_of_a_record_with_a_hundred_references_succeeds_in_a_bounded_number_of_loads()
{
    let store = MemoryStore::new();
    found(&store);
    define(
        &store,
        2,
        vec![
            SchemaChange::DefineObject {
                id: object("badge"),
                label: "Badge".into(),
            },
            SchemaChange::DefineObject {
                id: object("profile"),
                label: "Profile".into(),
            },
            SchemaChange::DefineField {
                object: object("profile"),
                field: field_def("title", FieldType::Str, true),
            },
            SchemaChange::DefineField {
                object: object("profile"),
                field: field_def(
                    "badges",
                    FieldType::List(Box::new(reference("badge"))),
                    false,
                ),
            },
        ],
    );
    let badges: Vec<Value> = (0..100)
        .map(|i| Value::Ref(create(&store, 10 + i, "badge", &[])))
        .collect();
    let profile = create(
        &store,
        200,
        "profile",
        &[("title", text("before")), ("badges", Value::List(badges))],
    );

    let before = store.loads();
    let done = run(
        &store,
        owner(),
        300,
        Intent::UpdateRecord {
            object: object("profile"),
            id: profile,
            fields: BTreeMap::from([(field("title"), text("after"))]),
            expected_version: None,
        },
    );
    assert_eq!(done.transition.events.len(), 1);
    let loads = store.loads() - before;
    assert!(
        loads <= LOADS_PER_DECISION,
        "the update took {loads} loads for 100 references"
    );
}

#[test]
fn a_schema_change_that_revalidates_records_referring_to_forty_types_succeeds() {
    let store = MemoryStore::new();
    found(&store);
    let targets: Vec<String> = (0..40).map(|i| format!("kind{i}")).collect();
    let mut changes = vec![SchemaChange::DefineObject {
        id: object("hub"),
        label: "Hub".into(),
    }];
    for name in &targets {
        changes.push(SchemaChange::DefineObject {
            id: object(name),
            label: name.clone(),
        });
        changes.push(SchemaChange::DefineField {
            object: object("hub"),
            field: field_def(&format!("to_{name}"), reference(name), false),
        });
    }
    define(&store, 2, changes);
    let mut fields = Vec::new();
    for (i, name) in targets.iter().enumerate() {
        let id = create(&store, 10 + i as i64, name, &[]);
        fields.push((format!("to_{name}"), Value::Ref(id)));
    }
    let fields: Vec<(&str, Value)> = fields
        .iter()
        .map(|(k, v)| (k.as_str(), v.clone()))
        .collect();
    create(&store, 100, "hub", &fields);

    let before = store.loads();
    define(
        &store,
        200,
        vec![SchemaChange::DefineConstraint {
            object: object("hub"),
            constraint: ConstraintDef {
                id: ConstraintId::new("always").expect("id"),
                expr: eq(lit("a"), lit("a")),
                message: "always".into(),
            },
        }],
    );
    let loads = store.loads() - before;
    assert!(
        loads <= LOADS_PER_DECISION,
        "the schema change took {loads} loads for 40 referenced types"
    );
}

/// Verifies a query whose rule follows a reference is answered on a partial snapshot.
#[test]
fn a_query_filter_that_follows_a_reference_is_decided_on_a_partial_snapshot() {
    let store = MemoryStore::new();
    found(&store);
    define(
        &store,
        2,
        vec![
            SchemaChange::DefineObject {
                id: object("person"),
                label: "Person".into(),
            },
            SchemaChange::DefineField {
                object: object("person"),
                field: field_def("name", FieldType::Str, true),
            },
            SchemaChange::DefineObject {
                id: object("note"),
                label: "Note".into(),
            },
            SchemaChange::DefineField {
                object: object("note"),
                field: field_def("author", reference("person"), false),
            },
            SchemaChange::DefineQuery(QueryDef {
                id: QueryId::new("by_ada").expect("id"),
                object: object("note"),
                filter: Some(eq(
                    Expr::Through(Scope::Record, "author".into(), "name".into()),
                    lit("Ada"),
                )),
                params: BTreeSet::new(),
                order: vec![],
                limit: None,
            }),
        ],
    );
    let ada = create(&store, 10, "person", &[("name", text("Ada"))]);
    let bob = create(&store, 11, "person", &[("name", text("Bob"))]);
    let hers = create(&store, 12, "note", &[("author", Value::Ref(ada))]);
    create(&store, 13, "note", &[("author", Value::Ref(bob))]);
    create(&store, 14, "note", &[]);

    let result = query(
        &store,
        &ReadRequest::Named {
            query: QueryId::new("by_ada").expect("id"),
            params: BTreeMap::new(),
        },
        &ctx("acme", owner(), 20),
    )
    .expect("the read is answered");
    let ids: Vec<RecordId> = result.rows.iter().map(|row| row.id).collect();
    assert_eq!(ids, vec![hers]);
}
