//! A read or a view of a type with many records loads a bounded number of them (KRN-2520,
//! KRN-2526, KRN-2920), through the memory store and the SQLite adapter. The bound is per page of
//! candidates, not per table: the type holds 600 records, 400 of them owned by one person, and a
//! page of that person's records loads at most 200 of them, as the requirement says.

use std::collections::BTreeMap;
use std::sync::Arc;
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_storage_api::covering::{load_covering, read_asking};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, query, Commit, Store};
use strata_storage_sqlite::SqliteStore;
use strata_substrate::project::section_request;
use strata_substrate::view_needs::{initial_needs, view_reads};

/// The most records a page of candidates a read loads holds (KRN-2520, KRN-2920).
const PAGE_OF_CANDIDATES: usize = 200;
/// The records of the type: more than two pages of candidates for the owner of most of them.
const RECORDS: u64 = 600;
/// The records owned by `x`: the ids that are not a multiple of three.
const OWNED_BY_X: usize = 400;

fn rec(name: &str) -> Expr {
    var(Scope::Record, name)
}

fn query_def(id: &str, filter: Expr, order: Vec<OrderKey>) -> QueryDef {
    QueryDef {
        id: QueryId::new(id).unwrap(),
        object: object("item"),
        filter: Some(filter),
        params: std::collections::BTreeSet::from(["who".to_string()]),
        order,
        limit: None,
        aggregates: None,
        declared_fields: None,
    }
}

/// The same filter inside a one-item `or`: no plan serves it, so it is answered by a scan.
fn scan_twin(def: &QueryDef) -> QueryDef {
    QueryDef {
        id: QueryId::new(format!("{}_scan", def.id)).unwrap(),
        filter: def.filter.clone().map(|f| Expr::Or(vec![f])),
        ..def.clone()
    }
}

fn schema() -> Vec<SchemaChange> {
    let mut changes = vec![SchemaChange::DefineObject {
        id: object("item"),
        label: "Item".into(),
    }];
    for (name, ty) in [
        ("owner", FieldType::Str),
        ("score", FieldType::Int),
        ("status", FieldType::Str),
    ] {
        changes.push(SchemaChange::DefineField {
            object: object("item"),
            field: field_def(name, ty, false),
        });
    }
    changes.push(SchemaChange::DefinePolicy(PolicyDef {
        id: PolicyId::new("public_reads_open").unwrap(),
        role: role("public"),
        resource: Resource::Object(object("item")),
        actions: std::collections::BTreeSet::from([Action::Read]),
        condition: Some(eq(rec("status"), lit("open"))),
    }));
    let by_owner = query_def(
        "by_owner",
        eq(rec("owner"), Expr::Param("who".into())),
        vec![],
    );
    // Posts by this owner, newest first: the pair index of the owner and the creation time.
    let newest = query_def(
        "owner_newest",
        eq(rec("owner"), Expr::Param("who".into())),
        vec![OrderKey {
            field: "$created_at".into(),
            descending: true,
        }],
    );
    for def in [by_owner, newest] {
        changes.push(SchemaChange::DefineQuery(scan_twin(&def)));
        changes.push(SchemaChange::DefineQuery(def));
    }
    changes
}

/// A record: its creation time repeats, so the order by it has ties.
fn item(id: u64) -> Arc<Record> {
    let owner = if id % 3 == 0 { "y" } else { "x" };
    Arc::new(Record {
        id: RecordId(id),
        object: object("item"),
        version: 1,
        created_by: None,
        created_at: Timestamp(((id * 7_919) % 97) as i64),
        updated_at: Timestamp(1),
        fields: BTreeMap::from([
            (field("owner"), text(owner)),
            (field("score"), Value::Int((id % 100) as i64)),
            (field("status"), text("open")),
        ]),
    })
}

/// A tenant holding `RECORDS` items, committed as one delta.
fn crowded(store: &dyn Store) {
    let tenant = "alpha";
    execute(
        store,
        &Intent::CreateTenant {
            name: tenant.into(),
            owner: principal_id("alpha:owner"),
        },
        &ctx(tenant, Principal::System, 10),
    )
    .unwrap();
    let change = Intent::ChangeSchema {
        changes: schema(),
        expected_version: None,
        existing: ExistingData::Reject,
    };
    execute(store, &change, &ctx(tenant, user("alpha:owner"), 11)).unwrap();
    let state = store.load(&tenant_id(tenant), &[]).unwrap().tenant.unwrap();
    let transition = Transition {
        delta: StateDelta {
            tenant: tenant_id(tenant),
            base_revision: Some(state.revision),
            new_revision: StateRevision(state.revision.0 + 1),
            next_record: RECORDS + 1,
            event_seq: state.event_seq,
            ops: (1..=RECORDS)
                .map(|id| DeltaOp::PutRecord(item(id)))
                .collect(),
        },
        events: Vec::new(),
        effects: Vec::new(),
        evidence: Vec::new(),
        output: Output::Tenant,
    };
    let intent = Intent::CreateRecord {
        object: object("item"),
        fields: BTreeMap::new(),
    };
    store
        .commit(&Commit::of(
            &intent,
            &ctx(tenant, Principal::System, 20),
            &transition,
            store.generation(&tenant_id(tenant)).unwrap(),
        ))
        .unwrap();
}

fn named(name: &str, size: u32, cursor: Option<PageCursor>) -> ReadRequest {
    ReadRequest::Named {
        query: QueryId::new(name).unwrap(),
        params: BTreeMap::from([("who".to_string(), text("x"))]),
        page_size: Some(size),
        cursor,
    }
}

fn stores() -> Vec<(&'static str, Box<dyn Store>)> {
    vec![
        ("memory", Box::new(MemoryStore::new()) as Box<dyn Store>),
        (
            "sqlite",
            Box::new(SqliteStore::open_in_memory(Timestamp(1)).unwrap()),
        ),
    ]
}

fn schema_of(store: &dyn Store) -> Arc<TenantSchema> {
    store
        .load(&tenant_id("alpha"), &[])
        .unwrap()
        .tenant
        .unwrap()
        .schema
        .clone()
}

/// Verifies: KRN-2520
/// Verifies: KRN-2920
///
/// Each page of an index read (an equality on a field) and of a pair read (an equality ordered
/// by the creation time) is one load of at most 200 records out of the 400 the owner has, and
/// the rows of the pair read, paged to the end, are the rows of a scan of the same query.
#[test]
fn a_page_of_a_planned_read_loads_at_most_two_hundred_candidates() {
    for (name, store) in stores() {
        let store = store.as_ref();
        crowded(store);
        let context = ctx("alpha", Principal::Anonymous, 9_000);
        let schema = schema_of(store);
        for query_name in ["by_owner", "owner_newest"] {
            let scan_name = format!("{query_name}_scan");
            let mut cursor = None;
            let mut scan_cursor = None;
            let mut rows = Vec::new();
            let mut scan_rows = Vec::new();
            for page_number in 0..16 {
                let request = named(query_name, 25, cursor.clone());
                let covered = load_covering(
                    store,
                    &context.tenant,
                    read_needs(&schema, &request),
                    |state, asked| read_asking(state, &request, &context, asked),
                )
                .unwrap();
                let page = covered.outcome.expect("the read is answered");
                assert_eq!(query(store, &request, &context).unwrap(), page);
                assert!(
                    covered.loads == 1 && covered.records <= PAGE_OF_CANDIDATES,
                    "{name} {query_name} page {page_number}: {} loads, {} records",
                    covered.loads,
                    covered.records
                );
                rows.extend(page.rows.iter().map(|r| r.id));
                cursor = page.next;
                let scan =
                    query(store, &named(&scan_name, 25, scan_cursor.clone()), &context).unwrap();
                scan_rows.extend(scan.rows.iter().map(|r| r.id));
                scan_cursor = scan.next;
                if cursor.is_none() {
                    break;
                }
            }
            assert!(cursor.is_none(), "{name} {query_name}: paged to the end");
            assert_eq!(
                rows.len(),
                OWNED_BY_X,
                "{name} {query_name}: every record of x once"
            );
            if query_name == "owner_newest" {
                assert_eq!(
                    rows, scan_rows,
                    "{name}: the pair read returns the rows of a scan, in the same order"
                );
            }
        }
    }
}

/// Verifies: KRN-2526
///
/// A view whose one section lists the owner's records loads that section's page, not the 600
/// records of the type: two loads at most, and at most two pages of candidates.
#[test]
fn a_view_loads_the_pages_its_sections_read_not_the_type() {
    for (name, store) in stores() {
        let store = store.as_ref();
        crowded(store);
        let context = ctx("alpha", Principal::Anonymous, 9_000);
        let view = ViewDef {
            id: ViewId::new("board").unwrap(),
            route: "/board".into(),
            title: "Board".into(),
            sections: BTreeMap::from([(
                "owned".to_string(),
                ViewSection {
                    marks: None,
                    joins: None,
                    query: QueryId::new("by_owner").unwrap(),
                    args: BTreeMap::from([("who".to_string(), ArgSource::Const(text("x")))]),
                    include: vec![],
                    markdown: vec![],
                    single: false,
                    aggregate: None,
                },
            )]),
        };
        let route = BTreeMap::new();
        let request = BTreeMap::new();
        let request_of = |s: &ViewSection, done: &BTreeMap<String, Option<Arc<Record>>>| {
            section_request(s, &route, &request, done)
        };
        let schema = schema_of(store);
        let initial = initial_needs(&schema, &view, &request_of);
        let covered = load_covering(store, &tenant_id("alpha"), initial, |state, asked| {
            view_reads(state, &context, &view, &request_of, asked)
        })
        .unwrap();
        assert!(covered.settled, "{name}: the view's reads are answered");
        assert!(covered.outcome.is_ok());
        assert!(
            covered.loads <= 2 && covered.records <= 2 * PAGE_OF_CANDIDATES,
            "{name}: the view loaded {} records in {} loads",
            covered.records,
            covered.loads
        );
    }
}
