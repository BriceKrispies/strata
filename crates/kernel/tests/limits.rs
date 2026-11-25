//! The stated maximums (KRN-1280 to KRN-1288): what a tenant cannot make large, and the paging and
//! bounded loads of a named query read. Every number in a test is the one its requirement states.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use strata_kernel::testkit::*;
use strata_kernel::*;

fn owner() -> Principal {
    user("alpha:owner")
}

fn change(world: &World, now: i64, changes: Vec<SchemaChange>) -> Result<World, DomainError> {
    world
        .step(
            &Intent::ChangeSchema {
                changes,
                expected_version: None,
                existing: ExistingData::Reject,
            },
            &ctx("alpha", owner(), now),
        )
        .map(|(world, _)| world)
}

fn create_note(world: &World, fields: &[(&str, Value)]) -> Result<World, DomainError> {
    let mut all = BTreeMap::from([(field("title"), text("T")), (field("slug"), text("s"))]);
    for (name, value) in fields {
        all.insert(field(name), value.clone());
    }
    world
        .step(
            &Intent::CreateRecord {
                object: object("note"),
                fields: all,
            },
            &ctx("alpha", owner(), 5_000),
        )
        .map(|(world, _)| world)
}

fn query(id: &str, order: Vec<OrderKey>, filter: Option<Expr>, limit: Option<u32>) -> QueryDef {
    QueryDef {
        id: QueryId::new(id).unwrap(),
        object: object("note"),
        filter,
        params: BTreeSet::new(),
        order,
        limit,
        aggregates: None,
        declared_fields: None,
        tree: None,
    }
}

fn with_queries(queries: Vec<QueryDef>) -> World {
    change(
        &notes_world(),
        3_000,
        queries.into_iter().map(SchemaChange::DefineQuery).collect(),
    )
    .expect("queries are accepted")
}

fn id_order() -> Vec<OrderKey> {
    vec![OrderKey {
        field: "$id".into(),
        descending: false,
    }]
}

/// The state of alpha with `n` published notes added directly, ids from 1000.
fn crowded(world: &World, n: u64) -> State {
    let state = world.state(&tenant_id("alpha"));
    let mut tenant = TenantState::clone(state.tenant.as_ref().unwrap());
    let notes = tenant.records.entry(object("note")).or_default();
    for i in 1..=n {
        let id = RecordId(1_000 + i);
        notes.insert(
            id,
            Arc::new(Record {
                id,
                object: object("note"),
                version: 1,
                created_by: None,
                created_at: Timestamp(1),
                updated_at: Timestamp(1),
                fields: BTreeMap::from([
                    (field("title"), text("T")),
                    (field("slug"), text(&format!("s{i}"))),
                    (field("status"), text("published")),
                    (field("rank"), Value::Int(i as i64)),
                ]),
            }),
        );
    }
    State {
        tenant_id: state.tenant_id,
        tenant: Some(Arc::new(tenant)),
        coverage: Coverage::Complete,
    }
}

/// What a store hands the kernel for `needs`: only the records they name.
fn thin(full: &State, needs: &[Need]) -> State {
    let tenant = full.tenant.as_ref().unwrap();
    let mut records: BTreeMap<ObjectTypeId, BTreeMap<RecordId, Arc<Record>>> = BTreeMap::new();
    for need in needs {
        let Need::PageOf {
            object: kind,
            after,
            limit,
        } = need
        else {
            panic!("a named query asks for a window, not {need:?}");
        };
        for record in tenant
            .records_of(kind)
            .filter(|r| after.is_none_or(|a| r.id > a))
            .take(*limit as usize)
        {
            records
                .entry(kind.clone())
                .or_default()
                .insert(record.id, record.clone());
        }
    }
    State {
        tenant_id: full.tenant_id.clone(),
        tenant: Some(Arc::new(TenantState {
            records,
            ..TenantState::clone(tenant)
        })),
        coverage: Coverage::Partial(needs.iter().cloned().collect()),
    }
}

fn named(name: &str, page_size: Option<u32>, cursor: Option<PageCursor>) -> ReadRequest {
    ReadRequest::Named {
        query: QueryId::new(name).unwrap(),
        params: BTreeMap::new(),
        page_size,
        cursor,
    }
}

fn ids(result: &ReadResult) -> Vec<u64> {
    result.rows.iter().map(|r| r.id.0).collect()
}

// Verifies: KRN-1286
#[test]
fn a_query_with_a_limit_over_a_table_of_a_thousand_records_loads_a_bounded_window() {
    let world = with_queries(vec![query("all_notes", vec![], None, Some(20))]);
    // Five times the page: a read that loaded the table would load far more than the page.
    let full = crowded(&world, 1_000);
    let schema = &full.tenant.as_ref().unwrap().schema;
    let request = named("all_notes", None, None);
    let needs = read_needs(schema, &request);
    assert_eq!(
        needs,
        vec![Need::PageOf {
            object: object("note"),
            after: None,
            limit: 200
        }]
    );
    let state = thin(&full, &needs);
    let loaded: usize = state
        .tenant
        .as_ref()
        .unwrap()
        .records
        .values()
        .map(BTreeMap::len)
        .sum();
    assert!(loaded <= 200usize, "loaded {loaded}");

    let page = read(&state, &request, &ctx("alpha", owner(), 7_000)).unwrap();
    assert_eq!(ids(&page), (1_001..=1_020).collect::<Vec<_>>());
    assert_eq!(page.next, Some(PageCursor::After(RecordId(1_020))));

    // The cursor continues exactly after the last row.
    let request = named("all_notes", None, page.next);
    let needs = read_needs(schema, &request);
    let page = read(
        &thin(&full, &needs),
        &request,
        &ctx("alpha", owner(), 7_000),
    )
    .unwrap();
    assert_eq!(ids(&page), (1_021..=1_040).collect::<Vec<_>>());
}

// Verifies: KRN-1285
#[test]
fn a_cursor_of_the_other_kind_is_refused() {
    let world = with_queries(vec![
        query("all_notes", vec![], None, None),
        query(
            "by_rank",
            vec![OrderKey {
                field: "rank".into(),
                descending: true,
            }],
            None,
            None,
        ),
    ]);
    let state = world.state(&tenant_id("alpha"));
    let context = ctx("alpha", owner(), 7_000);
    let window = read(
        &state,
        &named("all_notes", None, Some(PageCursor::Offset(1))),
        &context,
    );
    assert_eq!(window.unwrap_err().kind, ErrorKind::InvalidIntent);
    let scan = read(
        &state,
        &named("by_rank", None, Some(PageCursor::After(RecordId(1)))),
        &context,
    );
    assert_eq!(scan.unwrap_err().kind, ErrorKind::InvalidIntent);
}

// Verifies: KRN-1287
#[test]
fn only_a_query_without_a_filter_ordered_by_nothing_or_the_id_is_a_window() {
    let filter = eq(var(Scope::Record, "status"), lit("published"));
    let world = with_queries(vec![
        query("plain", vec![], None, None),
        query("by_id", id_order(), None, None),
        query(
            "by_id_down",
            vec![OrderKey {
                field: "$id".into(),
                descending: true,
            }],
            None,
            None,
        ),
        query(
            "by_rank",
            vec![OrderKey {
                field: "rank".into(),
                descending: false,
            }],
            None,
            None,
        ),
        query("filtered", vec![], Some(filter), None),
    ]);
    let schema = world
        .state(&tenant_id("alpha"))
        .tenant
        .unwrap()
        .schema
        .clone();
    let limit_of = |name: &str| match read_needs(&schema, &named(name, None, None)).as_slice() {
        [Need::PageOf { limit, .. }] => *limit,
        other => panic!("{other:?}"),
    };
    assert_eq!(limit_of("plain"), 200);
    assert_eq!(limit_of("by_id"), 200);
    for scan in ["by_id_down", "by_rank"] {
        assert_eq!(limit_of(scan), 10_001, "{scan}");
    }
    // KRN-2520: an equality on a field is served by that field's index, a page at a time.
    match read_needs(&schema, &named("filtered", None, None)).as_slice() {
        [Need::IndexPage {
            field: f, limit, ..
        }] => {
            assert_eq!(f, &field("status"));
            assert_eq!(*limit, 200);
        }
        other => panic!("{other:?}"),
    }
}

// Verifies: KRN-1288
#[test]
fn a_scan_over_more_than_the_scan_bound_is_refused_after_loading_one_record_past_it() {
    let world = with_queries(vec![query(
        "by_rank",
        vec![OrderKey {
            field: "rank".into(),
            descending: true,
        }],
        None,
        Some(5),
    )]);
    let schema = world
        .state(&tenant_id("alpha"))
        .tenant
        .unwrap()
        .schema
        .clone();
    let request = named("by_rank", None, None);
    let context = ctx("alpha", owner(), 7_000);

    let at_bound = crowded(&world, 10_000);
    let state = thin(&at_bound, &read_needs(&schema, &request));
    let page = read(&state, &request, &context).unwrap();
    assert_eq!(page.rows.len(), 5);
    assert_eq!(page.next, Some(PageCursor::Offset(5)));

    let over = crowded(&world, 10_001);
    let state = thin(&over, &read_needs(&schema, &request));
    let loaded: usize = state
        .tenant
        .as_ref()
        .unwrap()
        .records
        .values()
        .map(BTreeMap::len)
        .sum();
    assert_eq!(loaded, 10_001);
    assert_eq!(
        read(&state, &request, &context).unwrap_err().kind,
        ErrorKind::LimitExceeded
    );
}

// The tests below take every number from the wording of the requirement they carry (the
// specification), never from `strata_kernel::limits`: a test that read the constant would move
// with it, and a changed limit would pass unseen.

/// The schema of alpha.
fn schema_of(world: &World) -> Arc<TenantSchema> {
    world
        .state(&tenant_id("alpha"))
        .tenant
        .unwrap()
        .schema
        .clone()
}

/// Add `count` definitions made by `define` (called with 0, 1, 2, ...), in intents of at most 200
/// changes (KRN-1283). The first refusal is returned.
fn grow(
    world: &World,
    count: usize,
    define: impl Fn(usize) -> SchemaChange,
) -> Result<World, DomainError> {
    let mut world = world.clone();
    let mut next = 0;
    while next < count {
        let end = (next + 200).min(count);
        world = change(
            &world,
            3_000 + next as i64,
            (next..end).map(&define).collect(),
        )?;
        next = end;
    }
    Ok(world)
}

fn define_object(i: usize) -> SchemaChange {
    SchemaChange::DefineObject {
        id: object(&format!("obj_{i}")),
        label: "O".into(),
    }
}

fn define_policy(i: usize) -> SchemaChange {
    SchemaChange::DefinePolicy(PolicyDef {
        id: PolicyId::new(format!("pol_{i}")).unwrap(),
        role: role("editor"),
        resource: Resource::Object(object("note")),
        actions: BTreeSet::from([Action::Read]),
        condition: None,
    })
}

fn define_query(i: usize) -> SchemaChange {
    SchemaChange::DefineQuery(query(&format!("qry_{i}"), vec![], None, None))
}

fn define_field(i: usize) -> SchemaChange {
    SchemaChange::DefineField {
        object: object("person"),
        field: field_def(&format!("f_{i}"), FieldType::Str, false),
    }
}

/// The notes world with a list field `tags` on a note.
fn tagged_world() -> World {
    change(
        &notes_world(),
        3_000,
        vec![SchemaChange::DefineField {
            object: object("note"),
            field: field_def("tags", FieldType::List(Box::new(FieldType::Str)), false),
        }],
    )
    .unwrap()
}

// Verifies: KRN-1280
#[test]
fn a_string_of_65536_bytes_is_stored_and_one_of_65537_is_refused() {
    let world = tagged_world();
    let at_most = "x".repeat(65_536);
    assert!(create_note(&world, &[("body", text(&at_most))]).is_ok());
    let over = "x".repeat(65_537);
    assert_eq!(
        create_note(&world, &[("body", text(&over))])
            .unwrap_err()
            .kind,
        ErrorKind::LimitExceeded
    );
}

// Verifies: KRN-1280
#[test]
fn a_list_of_1000_items_is_stored_and_one_of_1001_is_refused() {
    let world = tagged_world();
    let list = |n: usize| Value::List((0..n).map(|_| text("t")).collect());
    assert!(create_note(&world, &[("tags", list(1_000))]).is_ok());
    assert_eq!(
        create_note(&world, &[("tags", list(1_001))])
            .unwrap_err()
            .kind,
        ErrorKind::LimitExceeded
    );
}

/// The bytes of the JSON encoding of the one note of `world`.
fn stored_bytes(world: &World) -> usize {
    let state = world.state(&tenant_id("alpha"));
    let tenant = state.tenant.as_ref().unwrap();
    let record = tenant.records_of(&object("note")).next().unwrap();
    serde_json::to_vec(&**record).unwrap().len()
}

// Verifies: KRN-1280
#[test]
fn a_record_of_262144_bytes_is_stored_and_one_of_262145_is_refused() {
    let world = tagged_world();
    // Four strings in a list: none is near its own maximum of 65536 bytes, so only the size of
    // the record is in question. The padding is ASCII, so each byte of it is one byte of JSON.
    let padded = |pad: usize| {
        let items = (0..4)
            .map(|i| text(&"y".repeat(pad / 4 + usize::from(i < pad % 4))))
            .collect();
        create_note(&world, &[("tags", Value::List(items))])
    };
    // The size of a record with 4000 bytes of padding, measured, fixes how much padding makes the
    // record exactly the size the requirement names.
    let base = stored_bytes(&padded(4_000).unwrap());
    let pad = 4_000 + 262_144 - base;
    let at_most = padded(pad).unwrap();
    assert_eq!(stored_bytes(&at_most), 262_144);
    assert_eq!(padded(pad + 1).unwrap_err().kind, ErrorKind::LimitExceeded);
}

// Verifies: KRN-1281
#[test]
fn a_unique_value_encoding_to_200_bytes_is_stored_and_one_of_201_is_refused() {
    let world = notes_world();
    // The JSON encoding of a text wraps it in its tag and quotes; the length that counts is the
    // length of that encoding.
    let overhead = serde_json::to_string(&text("")).unwrap().len();
    let encoded = |slug: &str| serde_json::to_string(&text(slug)).unwrap().len();
    let fits = "u".repeat(200 - overhead);
    assert_eq!(encoded(&fits), 200);
    assert!(create_note(&world, &[("slug", text(&fits))]).is_ok());
    let over = "u".repeat(201 - overhead);
    assert_eq!(encoded(&over), 201);
    assert_eq!(
        create_note(&world, &[("slug", text(&over))])
            .unwrap_err()
            .kind,
        ErrorKind::LimitExceeded
    );
}

// Verifies: KRN-1282
#[test]
fn a_schema_of_200_object_types_is_accepted_and_one_of_201_is_refused() {
    let world = notes_world();
    let have = schema_of(&world).objects.len();
    let full = grow(&world, 200 - have, define_object).unwrap();
    assert_eq!(schema_of(&full).objects.len(), 200);
    assert_eq!(
        grow(&world, 201 - have, define_object).unwrap_err().kind,
        ErrorKind::InvalidSchema
    );
}

// Verifies: KRN-1282
#[test]
fn an_object_type_of_100_fields_is_accepted_and_one_of_101_is_refused() {
    let world = notes_world();
    let have = schema_of(&world).objects[&object("person")].fields.len();
    let full = grow(&world, 100 - have, define_field).unwrap();
    assert_eq!(
        schema_of(&full).objects[&object("person")].fields.len(),
        100
    );
    assert_eq!(
        grow(&world, 101 - have, define_field).unwrap_err().kind,
        ErrorKind::InvalidSchema
    );
}

// Verifies: KRN-1282
#[test]
fn a_schema_of_500_policies_is_accepted_and_one_of_501_is_refused() {
    let world = notes_world();
    let have = schema_of(&world).policies.len();
    let full = grow(&world, 500 - have, define_policy).unwrap();
    assert_eq!(schema_of(&full).policies.len(), 500);
    assert_eq!(
        grow(&world, 501 - have, define_policy).unwrap_err().kind,
        ErrorKind::InvalidSchema
    );
}

// Verifies: KRN-1282
#[test]
fn a_schema_of_500_named_queries_is_accepted_and_one_of_501_is_refused() {
    let world = notes_world();
    let have = schema_of(&world).queries.len();
    let full = grow(&world, 500 - have, define_query).unwrap();
    assert_eq!(schema_of(&full).queries.len(), 500);
    assert_eq!(
        grow(&world, 501 - have, define_query).unwrap_err().kind,
        ErrorKind::InvalidSchema
    );
}

// Verifies: KRN-1282
#[test]
fn a_named_query_limit_is_accepted_from_1_to_200_and_refused_outside_it() {
    let world = notes_world();
    let defined = |limit: u32| {
        change(
            &world,
            3_000,
            vec![SchemaChange::DefineQuery(query(
                "q",
                vec![],
                None,
                Some(limit),
            ))],
        )
    };
    for limit in [1, 200] {
        assert!(defined(limit).is_ok(), "a limit of {limit}");
    }
    for limit in [0, 201] {
        assert_eq!(
            defined(limit).unwrap_err().kind,
            ErrorKind::InvalidSchema,
            "a limit of {limit}"
        );
    }
}

// Verifies: KRN-1283
#[test]
fn a_schema_change_of_200_changes_is_accepted_and_one_of_201_is_refused() {
    let world = notes_world();
    // Queries, not objects: the schema may hold 200 object types, so that many new ones would
    // trip the object limit before the count of changes is the question.
    let defines = |n: usize| -> Vec<SchemaChange> { (0..n).map(define_query).collect() };
    assert!(change(&world, 3_000, defines(200)).is_ok());
    assert_eq!(
        change(&world, 3_000, defines(201)).unwrap_err().kind,
        ErrorKind::LimitExceeded
    );
}

// Verifies: KRN-1284
#[test]
fn a_records_read_naming_200_ids_is_answered_and_one_naming_201_is_refused() {
    let world = notes_world();
    let state = world.state(&tenant_id("alpha"));
    let request = |n: u64| ReadRequest::Records {
        object: object("note"),
        ids: (1..=n).map(RecordId).collect(),
    };
    let context = ctx("alpha", owner(), 7_000);
    assert!(read(&state, &request(200), &context).is_ok());
    assert_eq!(
        read(&state, &request(201), &context).unwrap_err().kind,
        ErrorKind::LimitExceeded
    );
    let schema = &state.tenant.as_ref().unwrap().schema;
    assert_eq!(read_needs(schema, &request(200)).len(), 200);
    assert!(read_needs(schema, &request(201)).is_empty());
}

// Verifies: KRN-1285
#[test]
fn a_page_is_the_size_the_read_names_else_the_limit_the_query_declares_else_50() {
    let world = with_queries(vec![
        query("no_limit", vec![], None, None),
        query("limit_20", vec![], None, Some(20)),
    ]);
    let full = crowded(&world, 300);
    let schema = &full.tenant.as_ref().unwrap().schema;
    let rows = |name: &str, page_size: Option<u32>| {
        let request = named(name, page_size, None);
        let state = thin(&full, &read_needs(schema, &request));
        read(&state, &request, &ctx("alpha", owner(), 7_000)).unwrap()
    };
    let default = rows("no_limit", None);
    assert_eq!(default.rows.len(), 50);
    assert_eq!(default.next, Some(PageCursor::After(RecordId(1_050))));
    assert_eq!(rows("no_limit", Some(7)).rows.len(), 7);
    assert_eq!(rows("no_limit", Some(200)).rows.len(), 200);
    assert_eq!(rows("limit_20", None).rows.len(), 20);
    assert_eq!(rows("limit_20", Some(7)).rows.len(), 7);
}

// Verifies: KRN-1285
#[test]
fn a_page_size_of_201_is_refused() {
    let world = with_queries(vec![query("no_limit", vec![], None, None)]);
    let state = crowded(&world, 3);
    let request = named("no_limit", Some(201), None);
    assert_eq!(
        read(&state, &request, &ctx("alpha", owner(), 7_000))
            .unwrap_err()
            .kind,
        ErrorKind::LimitExceeded
    );
}
