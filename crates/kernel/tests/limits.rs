//! The stated maximums (KRN-1280 to KRN-1288): what a tenant cannot make large, and the paging and
//! bounded loads of a named query read.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use strata_kernel::limits;
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
fn a_query_with_a_limit_over_a_hundred_thousand_records_loads_a_bounded_window() {
    let world = with_queries(vec![query("all_notes", vec![], None, Some(20))]);
    let full = crowded(&world, 100_000);
    let schema = &full.tenant.as_ref().unwrap().schema;
    let request = named("all_notes", None, None);
    let needs = read_needs(schema, &request);
    assert_eq!(
        needs,
        vec![Need::PageOf {
            object: object("note"),
            after: None,
            limit: limits::MAX_PAGE_SIZE
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
    assert!(loaded <= limits::MAX_PAGE_SIZE as usize, "loaded {loaded}");

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
fn a_named_query_read_pages_through_every_record_and_ends() {
    let world = with_queries(vec![
        query("all_notes", id_order(), None, None),
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
    let full = crowded(&world, 120);
    let schema = &full.tenant.as_ref().unwrap().schema;
    for name in ["all_notes", "by_rank"] {
        let mut seen = Vec::new();
        let mut cursor = None;
        let mut pages = 0;
        loop {
            let request = named(name, Some(50), cursor);
            let state = thin(&full, &read_needs(schema, &request));
            let page = read(&state, &request, &ctx("alpha", owner(), 7_000)).unwrap();
            assert!(page.rows.len() <= 50);
            seen.extend(ids(&page));
            pages += 1;
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
            assert!(pages < 10, "the cursor does not advance");
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 120, "{name} returns each record once");
    }
}

// Verifies: KRN-1285
#[test]
fn a_query_with_no_limit_and_no_page_size_returns_the_default_page() {
    let world = with_queries(vec![query("all_notes", vec![], None, None)]);
    let full = crowded(&world, 120);
    let schema = &full.tenant.as_ref().unwrap().schema;
    let request = named("all_notes", None, None);
    let state = thin(&full, &read_needs(schema, &request));
    let page = read(&state, &request, &ctx("alpha", owner(), 7_000)).unwrap();
    assert_eq!(page.rows.len(), limits::DEFAULT_PAGE_SIZE as usize);
    assert!(page.next.is_some());
}

// Verifies: KRN-1285
#[test]
fn a_page_size_above_the_maximum_or_of_zero_is_refused() {
    let world = with_queries(vec![query("all_notes", vec![], None, None)]);
    let state = world.state(&tenant_id("alpha"));
    let read_as = |size| {
        read(
            &state,
            &named("all_notes", Some(size), None),
            &ctx("alpha", owner(), 7_000),
        )
    };
    assert_eq!(
        read_as(limits::MAX_PAGE_SIZE + 1).unwrap_err().kind,
        ErrorKind::LimitExceeded
    );
    assert_eq!(read_as(0).unwrap_err().kind, ErrorKind::InvalidIntent);
    assert!(read_as(limits::MAX_PAGE_SIZE).is_ok());
    let schema = &state.tenant.as_ref().unwrap().schema;
    assert!(read_needs(schema, &named("all_notes", Some(201), None)).is_empty());
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
    assert_eq!(limit_of("plain"), limits::MAX_PAGE_SIZE);
    assert_eq!(limit_of("by_id"), limits::MAX_PAGE_SIZE);
    for scan in ["by_id_down", "by_rank", "filtered"] {
        assert_eq!(limit_of(scan), limits::MAX_SCAN_RECORDS + 1, "{scan}");
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

    let at_bound = crowded(&world, limits::MAX_SCAN_RECORDS as u64);
    let state = thin(&at_bound, &read_needs(&schema, &request));
    let page = read(&state, &request, &context).unwrap();
    assert_eq!(page.rows.len(), 5);
    assert_eq!(page.next, Some(PageCursor::Offset(5)));

    let over = crowded(&world, limits::MAX_SCAN_RECORDS as u64 + 1);
    let state = thin(&over, &read_needs(&schema, &request));
    let loaded: usize = state
        .tenant
        .as_ref()
        .unwrap()
        .records
        .values()
        .map(BTreeMap::len)
        .sum();
    assert_eq!(loaded, limits::MAX_SCAN_RECORDS as usize + 1);
    assert_eq!(
        read(&state, &request, &context).unwrap_err().kind,
        ErrorKind::LimitExceeded
    );
}

// Verifies: KRN-1284
#[test]
fn a_records_read_naming_more_than_the_maximum_ids_is_refused() {
    let world = notes_world();
    let state = world.state(&tenant_id("alpha"));
    let request = |n: u64| ReadRequest::Records {
        object: object("note"),
        ids: (1..=n).map(RecordId).collect(),
    };
    let context = ctx("alpha", owner(), 7_000);
    let max = limits::MAX_READ_IDS as u64;
    assert!(read(&state, &request(max), &context).is_ok());
    assert_eq!(
        read(&state, &request(max + 1), &context).unwrap_err().kind,
        ErrorKind::LimitExceeded
    );
    let schema = &state.tenant.as_ref().unwrap().schema;
    assert!(read_needs(schema, &request(max + 1)).is_empty());
}

// Verifies: KRN-1280
#[test]
fn a_string_a_list_and_a_record_over_their_maximum_are_refused() {
    let world = change(
        &notes_world(),
        3_000,
        vec![SchemaChange::DefineField {
            object: object("note"),
            field: field_def("tags", FieldType::List(Box::new(FieldType::Str)), false),
        }],
    )
    .unwrap();
    let refused = |fields: &[(&str, Value)]| create_note(&world, fields).unwrap_err().kind;

    let long = "x".repeat(limits::MAX_STRING_BYTES + 1);
    assert_eq!(refused(&[("body", text(&long))]), ErrorKind::LimitExceeded);
    assert!(create_note(&world, &[("body", text(&long[1..]))]).is_ok());

    let many = Value::List((0..=limits::MAX_LIST_LEN).map(|_| text("t")).collect());
    assert_eq!(refused(&[("tags", many)]), ErrorKind::LimitExceeded);

    // Each item and the list are within their maximums; the record is not.
    let item = "y".repeat(limits::MAX_RECORD_BYTES / limits::MAX_LIST_LEN + 10);
    let heavy = Value::List((0..limits::MAX_LIST_LEN).map(|_| text(&item)).collect());
    assert_eq!(refused(&[("tags", heavy)]), ErrorKind::LimitExceeded);
}

// Verifies: KRN-1281
#[test]
fn a_value_in_a_unique_field_over_its_maximum_is_refused() {
    let world = notes_world();
    let long = "u".repeat(limits::MAX_UNIQUE_BYTES);
    assert_eq!(
        create_note(&world, &[("slug", text(&long))])
            .unwrap_err()
            .kind,
        ErrorKind::LimitExceeded
    );
    // The limit is on the encoded value, which wraps the text in its tag and quotes.
    let overhead = limits::encoded_len(&text(""));
    let fits = "u".repeat(limits::MAX_UNIQUE_BYTES - overhead);
    assert_eq!(limits::encoded_len(&text(&fits)), limits::MAX_UNIQUE_BYTES);
    assert!(create_note(&world, &[("slug", text(&fits))]).is_ok());
}

// Verifies: KRN-1282
#[test]
fn a_schema_over_its_maximum_objects_fields_policies_or_queries_is_refused() {
    let world = notes_world();
    let invalid = |result: Result<World, DomainError>| {
        let error = result.unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidSchema, "{}", error.message);
        assert!(error.message.starts_with("limit:"), "{}", error.message);
    };

    // 100 more fields on person, which has one.
    invalid(change(
        &world,
        3_000,
        (0..limits::MAX_FIELDS_PER_OBJECT)
            .map(|i| SchemaChange::DefineField {
                object: object("person"),
                field: field_def(&format!("f_{i}"), FieldType::Str, false),
            })
            .collect(),
    ));

    // More object types than the maximum, in intents of at most the changes maximum: the world
    // holds two, so 150 more fit and 60 more do not.
    let objects = |from: usize, to: usize| -> Vec<SchemaChange> {
        (from..to)
            .map(|i| SchemaChange::DefineObject {
                id: object(&format!("o_{i}")),
                label: "O".into(),
            })
            .collect()
    };
    let more = change(&world, 3_500, objects(0, 150)).expect("152 object types fit");
    invalid(change(&more, 3_501, objects(150, 210)));

    // Policies and queries: 200 at a time, the third batch passes the maximum.
    let batches = |build: &dyn Fn(usize) -> SchemaChange, maximum: usize| {
        let mut step = world.clone();
        let mut refused = None;
        for n in 0..3 {
            let changes = (n * 200..(n + 1) * 200).map(build).collect();
            match change(&step, 4_000 + n as i64, changes) {
                Ok(next) => step = next,
                Err(error) => {
                    refused = Some((n, error));
                    break;
                }
            }
        }
        let (n, error) = refused.unwrap_or_else(|| panic!("more than {maximum} were accepted"));
        assert_eq!(n, 2, "the maximum is {maximum}");
        invalid(Err(error));
    };
    batches(
        &|i| {
            SchemaChange::DefinePolicy(PolicyDef {
                id: PolicyId::new(&format!("p_{i}")).unwrap(),
                role: role("writer"),
                resource: Resource::Object(object("note")),
                actions: BTreeSet::from([Action::Read]),
                condition: None,
            })
        },
        limits::MAX_POLICIES,
    );
    batches(
        &|i| SchemaChange::DefineQuery(query(&format!("q_{i}"), vec![], None, None)),
        limits::MAX_QUERIES,
    );
}

// Verifies: KRN-1282
#[test]
fn a_query_that_declares_a_limit_above_the_maximum_page_size_is_refused() {
    let world = notes_world();
    for limit in [0, limits::MAX_PAGE_SIZE + 1] {
        let error = change(
            &world,
            3_000,
            vec![SchemaChange::DefineQuery(query(
                "q",
                vec![],
                None,
                Some(limit),
            ))],
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidSchema);
    }
    assert!(change(
        &world,
        3_000,
        vec![SchemaChange::DefineQuery(query(
            "q",
            vec![],
            None,
            Some(limits::MAX_PAGE_SIZE)
        ))],
    )
    .is_ok());
}

// Verifies: KRN-1283
#[test]
fn a_schema_change_with_more_than_the_maximum_changes_is_refused() {
    let world = notes_world();
    // Queries, not objects: the schema may hold 200 object types, so that many new ones would
    // trip the object limit before the count of changes is the question.
    let defines = |n: usize| -> Vec<SchemaChange> {
        (0..n)
            .map(|i| SchemaChange::DefineQuery(query(&format!("c_{i}"), vec![], None, None)))
            .collect()
    };
    assert!(change(&world, 3_000, defines(limits::MAX_SCHEMA_CHANGES)).is_ok());
    assert_eq!(
        change(&world, 3_000, defines(limits::MAX_SCHEMA_CHANGES + 1))
            .unwrap_err()
            .kind,
        ErrorKind::LimitExceeded
    );
}
