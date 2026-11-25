//! Planned named queries through the stores (KRN-2520, KRN-2521, KRN-2523, KRN-2525, KRN-2526,
//! KRN-2920 to KRN-2922): the SQLite adapter and the memory store answer every index page and
//! every pair page the same, a planned read through either returns the rows of a scan in the same
//! order across pages for every reader, a read or a view over 100,000 records loads a bounded
//! number of them, a cursor resumes exactly after commits insert and delete records, one tenant's
//! index never answers for another, a file whose order keys or pair rows are missing is rebuilt
//! when it is opened, the pair index follows record writes and query definitions, and a read whose
//! policy follows a reference loads only the records its rows name. Random data from a fixed seed.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use strata_kernel::query_plan::{self, IndexBound, IndexPosition};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_storage_api::covering::{load_covering, read_asking, Covered};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, query, Commit, Store};
use strata_storage_sqlite::SqliteStore;
use strata_substrate::project::section_request;
use strata_substrate::view_needs::view_reads;

fn owner(tenant: &str) -> Principal {
    user(&format!("{tenant}:owner"))
}

fn rec(name: &str) -> Expr {
    var(Scope::Record, name)
}

fn cmp(op: CmpOp, a: Expr, b: Expr) -> Expr {
    Expr::Cmp(op, Box::new(a), Box::new(b))
}

fn param(name: &str) -> Expr {
    Expr::Param(name.into())
}

fn order(field: &str, descending: bool) -> Vec<OrderKey> {
    vec![OrderKey {
        field: field.into(),
        descending,
    }]
}

fn query_def(id: &str, filter: Expr, params: &[&str], order: Vec<OrderKey>) -> QueryDef {
    QueryDef {
        id: QueryId::new(id).unwrap(),
        object: object("item"),
        filter: Some(filter),
        params: params.iter().map(|p| p.to_string()).collect(),
        order,
        limit: None,
        aggregates: None,
        declared_fields: None,
    }
}

fn planned_queries() -> Vec<QueryDef> {
    let range = || {
        Expr::And(vec![
            cmp(CmpOp::Ge, rec("score"), param("lo")),
            cmp(CmpOp::Lt, rec("score"), param("hi")),
        ])
    };
    vec![
        query_def("by_owner", eq(rec("owner"), param("who")), &["who"], vec![]),
        query_def("score_range", range(), &["lo", "hi"], order("score", false)),
        query_def(
            "score_range_down",
            range(),
            &["lo", "hi"],
            order("score", true),
        ),
        query_def(
            "groups_down",
            Expr::In(Box::new(rec("grp")), Box::new(param("gs"))),
            &["gs"],
            order("grp", true),
        ),
        query_def(
            "recent",
            cmp(CmpOp::Gt, rec("at"), param("t")),
            &["t"],
            order("at", false),
        ),
        query_def("mine", eq(rec("owner"), Expr::Principal), &[], vec![]),
        query_def(
            "open_above",
            Expr::And(vec![
                eq(rec("status"), lit("open")),
                cmp(CmpOp::Gt, rec("score"), param("lo")),
            ]),
            &["lo"],
            vec![],
        ),
        query_def(
            "group_ordered",
            eq(rec("grp"), param("g")),
            &["g"],
            order("grp", false),
        ),
        // Pair plans (KRN-2920, KRN-2921): an equality or a list of values on one field ordered
        // by another attribute.
        query_def(
            "owner_newest",
            eq(rec("owner"), param("who")),
            &["who"],
            order("$created_at", true),
        ),
        query_def(
            "owner_by_at",
            eq(rec("owner"), param("who")),
            &["who"],
            order("at", false),
        ),
        // Some scores are text or a list, which the pair index cannot order: the store answers
        // this pair from the records of the type.
        query_def(
            "groups_by_score",
            Expr::In(Box::new(rec("grp")), Box::new(param("gs"))),
            &["gs"],
            order("score", true),
        ),
        // Some notes are longer than the key limit: they have no row, and a probe for one is a
        // scan.
        query_def(
            "note_by_at",
            eq(rec("note"), param("n")),
            &["n"],
            order("at", true),
        ),
        query_def(
            "mine_newest",
            eq(rec("owner"), Expr::Principal),
            &[],
            order("$updated_at", true),
        ),
    ]
}

/// The same filter inside a one-item `or`: no plan serves it, and it holds when the filter holds.
fn scan_twin(def: &QueryDef) -> QueryDef {
    QueryDef {
        id: QueryId::new(format!("{}_scan", def.id)).unwrap(),
        filter: def.filter.clone().map(|f| Expr::Or(vec![f])),
        ..def.clone()
    }
}

fn policy(id: &str, role_name: &str, condition: Option<Expr>) -> SchemaChange {
    SchemaChange::DefinePolicy(PolicyDef {
        id: PolicyId::new(id).unwrap(),
        role: role(role_name),
        resource: Resource::Object(object("item")),
        actions: BTreeSet::from([Action::Read]),
        condition,
    })
}

fn item_schema() -> Vec<SchemaChange> {
    let mut changes = vec![
        SchemaChange::DefineRole(RoleDef {
            id: role("writer"),
            label: "Writer".into(),
        }),
        SchemaChange::DefineRole(RoleDef {
            id: role("editor"),
            label: "Editor".into(),
        }),
        SchemaChange::DefineRole(RoleDef {
            id: role("member"),
            label: "Member".into(),
        }),
        SchemaChange::DefineObject {
            id: object("team"),
            label: "Team".into(),
        },
        SchemaChange::DefineField {
            object: object("team"),
            field: field_def("open", FieldType::Bool, false),
        },
        SchemaChange::DefineObject {
            id: object("item"),
            label: "Item".into(),
        },
        SchemaChange::DefineField {
            object: object("item"),
            field: field_def(
                "team",
                FieldType::Ref {
                    target: object("team"),
                    on_delete: OnDelete::SetNull,
                },
                false,
            ),
        },
        // A member reads an item of an open team: a read policy that follows a reference
        // (KRN-2922).
        policy(
            "members_read_open_team",
            "member",
            Some(eq(
                Expr::Through(Scope::Record, "team".into(), "open".into()),
                Expr::Lit(Value::Bool(true)),
            )),
        ),
    ];
    for (name, ty) in [
        ("owner", FieldType::Str),
        ("grp", FieldType::Str),
        ("score", FieldType::Int),
        ("at", FieldType::Time),
        ("status", FieldType::Str),
        ("note", FieldType::Str),
    ] {
        changes.push(SchemaChange::DefineField {
            object: object("item"),
            field: field_def(name, ty, false),
        });
    }
    changes.push(policy(
        "public_reads_open",
        "public",
        Some(eq(rec("status"), lit("open"))),
    ));
    changes.push(policy(
        "writers_read_own",
        "writer",
        Some(eq(rec("owner"), Expr::Principal)),
    ));
    changes.push(policy("editors_read_items", "editor", None));
    for def in planned_queries() {
        changes.push(SchemaChange::DefineQuery(scan_twin(&def)));
        changes.push(SchemaChange::DefineQuery(def));
    }
    changes
}

/// A tenant with the item schema, a writer and an editor, and no items.
fn found(store: &dyn Store, tenant: &str) {
    execute(
        store,
        &Intent::CreateTenant {
            name: tenant.into(),
            owner: principal_id(&format!("{tenant}:owner")),
        },
        &ctx(tenant, Principal::System, 10),
    )
    .unwrap();
    let change = Intent::ChangeSchema {
        changes: item_schema(),
        expected_version: None,
        existing: ExistingData::Reject,
    };
    execute(store, &change, &ctx(tenant, owner(tenant), 11)).unwrap();
    for (who, role_name) in [
        ("writer1", "writer"),
        ("editor", "editor"),
        ("member1", "member"),
    ] {
        let assign = Intent::AssignRole {
            principal: principal_id(&format!("{tenant}:{who}")),
            role: role(role_name),
        };
        execute(store, &assign, &ctx(tenant, owner(tenant), 12)).unwrap();
    }
}

/// An item. Its creation and update times repeat, so the orders by them have ties.
fn item(id: u64, fields: &[(&str, Value)]) -> Arc<Record> {
    Arc::new(Record {
        id: RecordId(id),
        object: object("item"),
        version: 1,
        created_by: None,
        created_at: Timestamp(((id * 7_919) % 97) as i64),
        updated_at: Timestamp(((id * 31) % 13) as i64),
        fields: fields
            .iter()
            .filter(|(_, v)| *v != Value::Null)
            .map(|(name, value)| (field(name), value.clone()))
            .collect(),
    })
}

/// Commit record puts and deletions directly, as one delta: the way to give a store many records
/// without deciding an intent for each. Records are taken as they are, so a value of another
/// type than its field's (a grandfathered value) can be stored.
fn commit_ops(store: &dyn Store, tenant: &str, ops: Vec<DeltaOp>) {
    let header = store.load(&tenant_id(tenant), &[]).unwrap();
    let state = header.tenant.unwrap();
    let top = ops
        .iter()
        .filter_map(|op| match op {
            DeltaOp::PutRecord(r) => Some(r.id.0),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let transition = Transition {
        delta: StateDelta {
            tenant: tenant_id(tenant),
            base_revision: Some(state.revision),
            new_revision: StateRevision(state.revision.0 + 1),
            next_record: state.next_record.max(top + 1),
            event_seq: state.event_seq,
            ops,
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
    let context = ctx(tenant, Principal::System, 20);
    store
        .commit(&Commit::of(
            &intent,
            &context,
            &transition,
            store.generation(&tenant_id(tenant)).unwrap(),
        ))
        .unwrap();
}

fn put(store: &dyn Store, tenant: &str, items: Vec<Arc<Record>>) {
    for chunk in items.chunks(20_000) {
        commit_ops(
            store,
            tenant,
            chunk.iter().cloned().map(DeltaOp::PutRecord).collect(),
        );
    }
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

const OWNERS: [&str; 3] = ["alpha:writer1", "alpha:editor", "x"];
const GROUPS: [&str; 4] = ["g0", "g1", "g2", "g3"];

/// A random item: some values missing, some scores text or a list, some notes longer than the
/// index key limit.
fn random_item(rng: &mut Lcg, id: u64) -> Arc<Record> {
    let score = match rng.below(12) {
        0 => Value::Null,
        1 => text(&rng.below(4).to_string()),
        2 => Value::List(vec![Value::Int(1)]),
        _ => Value::Int(rng.below(30) as i64 - 15),
    };
    let note = match rng.below(4) {
        0 => text(&"n".repeat(300)),
        _ => text(&format!("n{}", rng.below(3))),
    };
    let team = match rng.below(4) {
        0 => Value::Null,
        _ => Value::Ref(RecordId(1 + rng.below(TEAMS))),
    };
    item(
        id,
        &[
            ("owner", text(rng.pick(&OWNERS))),
            ("grp", text(rng.pick(&GROUPS))),
            ("score", score),
            (
                "at",
                if rng.below(5) == 0 {
                    Value::Null
                } else {
                    Value::Time(rng.below(40) as i64)
                },
            ),
            (
                "status",
                text(if rng.below(3) == 0 { "closed" } else { "open" }),
            ),
            ("note", note),
            ("team", team),
        ],
    )
}

/// The teams of the random items; an even one is open.
const TEAMS: u64 = 5;

fn named(
    name: &str,
    params: &BTreeMap<String, Value>,
    size: u32,
    cursor: Option<PageCursor>,
) -> ReadRequest {
    ReadRequest::Named {
        query: QueryId::new(name).unwrap(),
        params: params.clone(),
        page_size: Some(size),
        cursor,
    }
}

/// A read through the store loop, with what it loaded: the loads and the records they returned.
fn traced(
    store: &dyn Store,
    request: &ReadRequest,
    context: &Context,
) -> (ReadResult, usize, usize) {
    let covered = covered_read(store, request, context);
    let result = covered.outcome.expect("the read is answered");
    assert_eq!(
        query(store, request, context).unwrap(),
        result,
        "the traced read is the read"
    );
    (result, covered.loads, covered.records)
}

/// A read through the store loop `query` runs, with everything it loaded.
fn covered_read(
    store: &dyn Store,
    request: &ReadRequest,
    context: &Context,
) -> Covered<ReadResult> {
    let schema = store
        .load(&context.tenant, &[])
        .unwrap()
        .tenant
        .unwrap()
        .schema
        .clone();
    load_covering(
        store,
        &context.tenant,
        read_needs(&schema, request),
        |state, asked| read_asking(state, request, context, asked),
    )
    .unwrap()
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

// Verifies: KRN-2525
#[test]
fn every_store_answers_an_index_page_alike() {
    let mut rng = Lcg(2525);
    let items: Vec<Arc<Record>> = (1..=300).map(|id| random_item(&mut rng, id)).collect();
    let made = stores();
    for (_, store) in &made {
        found(store.as_ref(), "alpha");
        put(store.as_ref(), "alpha", items.clone());
    }
    let complete = made[0].1.load_complete(&tenant_id("alpha")).unwrap();
    let all = complete.tenant.unwrap();
    let value = |rng: &mut Lcg| match rng.below(9) {
        0 => Value::Int(rng.below(30) as i64 - 15),
        1 => Value::Int(i64::MIN),
        2 => Value::Int(i64::MAX),
        3 => Value::Time(rng.below(40) as i64),
        4 => text(&format!("g{}", rng.below(4))),
        5 => text(&"n".repeat(300)),
        6 => text(&format!("n{}", rng.below(3))),
        7 => Value::Bool(rng.below(2) == 0),
        _ => Value::Null,
    };
    let fields = ["score", "at", "grp", "note", "owner"];
    for round in 0..600 {
        let field_name = *rng.pick(&fields);
        let low = value(&mut rng);
        let (lower, upper) = if rng.below(3) == 0 {
            let bound = IndexBound {
                value: low.clone(),
                inclusive: true,
            };
            (bound.clone(), bound)
        } else {
            let high = match (&low, rng.below(4)) {
                (Value::Int(_), 0..=2) => Value::Int(rng.below(30) as i64 - 10),
                (Value::Time(_), 0..=2) => Value::Time(rng.below(40) as i64),
                _ => value(&mut rng),
            };
            (
                IndexBound {
                    value: low,
                    inclusive: rng.below(2) == 0,
                },
                IndexBound {
                    value: high,
                    inclusive: rng.below(2) == 0,
                },
            )
        };
        let after = match rng.below(3) {
            0 => None,
            _ => Some(IndexPosition {
                value: value(&mut rng),
                id: RecordId(rng.below(320)),
            }),
        };
        let need = Need::IndexPage {
            object: object("item"),
            field: field(field_name),
            lower,
            upper,
            descending: rng.below(2) == 0,
            after,
            limit: 1 + rng.below(40) as u32,
        };
        let Need::IndexPage {
            field: f,
            lower,
            upper,
            descending,
            after,
            limit,
            ..
        } = &need
        else {
            unreachable!()
        };
        let expected: BTreeSet<u64> = query_plan::index_page(
            all.records_of(&object("item")),
            f,
            lower,
            upper,
            *descending,
            after.as_ref(),
            *limit,
        )
        .iter()
        .map(|r| r.id.0)
        .collect();
        for (name, store) in &made {
            let loaded = store
                .load(&tenant_id("alpha"), std::slice::from_ref(&need))
                .unwrap();
            let ids: BTreeSet<u64> = loaded
                .tenant
                .unwrap()
                .records_of(&object("item"))
                .map(|r| r.id.0)
                .collect();
            assert_eq!(ids, expected, "{name}, round {round}: {need:?}");
        }
    }
}

/// One hundred thousand items: a third owned by x, scores 0 to 999, times by id.
fn crowded(store: &dyn Store, tenant: &str) {
    found(store, tenant);
    put(
        store,
        tenant,
        (1..=100_000)
            .map(|id| {
                item(
                    id,
                    &[
                        ("owner", text(OWNERS[(id % 3) as usize])),
                        ("grp", text(GROUPS[(id % 4) as usize])),
                        ("score", Value::Int((id % 1_000) as i64)),
                        ("at", Value::Time(id as i64)),
                        ("status", text("open")),
                    ],
                )
            })
            .collect(),
    );
}

// Verifies: KRN-2520
// Verifies: KRN-2526
// Verifies: KRN-2920
#[test]
#[ignore = "scale: run with --ignored"]
fn reads_and_views_over_a_hundred_thousand_records_load_a_bounded_number() {
    for (name, store) in stores() {
        let store = store.as_ref();
        crowded(store, "alpha");
        let context = ctx("alpha", Principal::Anonymous, 9_000);
        // The most records a page of candidates holds (KRN-2520).
        let per_page = 200;
        // Posts by this author, newest first (WG-5181): one page of candidates per page of rows,
        // and the rows the kernel reads from the whole table.
        let complete = store.load_complete(&tenant_id("alpha")).unwrap();
        let who = BTreeMap::from([("who".to_string(), text("x"))]);
        for query_name in ["owner_newest", "owner_by_at"] {
            let mut cursor = None;
            for page_number in 0..3 {
                let request = named(query_name, &who, 25, cursor.clone());
                let (page, loads, records) = traced(store, &request, &context);
                assert_eq!(
                    page,
                    read(&complete, &request, &context).unwrap(),
                    "{name} {query_name} page {page_number}: the rows of the whole table"
                );
                assert_eq!(page.rows.len(), 25);
                assert!(
                    loads == 1 && records <= per_page,
                    "{name} {query_name} page {page_number}: {loads} loads, {records} records"
                );
                cursor = page.next;
            }
        }
        drop(complete);
        for (query_name, params) in [
            ("by_owner", BTreeMap::from([("who".to_string(), text("x"))])),
            (
                "score_range",
                BTreeMap::from([
                    ("lo".to_string(), Value::Int(10)),
                    ("hi".to_string(), Value::Int(990)),
                ]),
            ),
            (
                "score_range_down",
                BTreeMap::from([
                    ("lo".to_string(), Value::Int(10)),
                    ("hi".to_string(), Value::Int(990)),
                ]),
            ),
            (
                "recent",
                BTreeMap::from([("t".to_string(), Value::Time(50_000))]),
            ),
            (
                "groups_down",
                BTreeMap::from([("gs".to_string(), Value::List(vec![text("g1"), text("g3")]))]),
            ),
            // Posts by this author, newest first (WG-5181, KRN-2920).
            (
                "owner_newest",
                BTreeMap::from([("who".to_string(), text("x"))]),
            ),
            (
                "owner_by_at",
                BTreeMap::from([("who".to_string(), text("x"))]),
            ),
            (
                "groups_by_score",
                BTreeMap::from([("gs".to_string(), Value::List(vec![text("g1"), text("g3")]))]),
            ),
        ] {
            let mut cursor = None;
            for page_number in 0..3 {
                let request = named(query_name, &params, 20, cursor.clone());
                let (page, loads, records) = traced(store, &request, &context);
                assert_eq!(page.rows.len(), 20, "{name} {query_name}");
                assert!(loads <= 2, "{name} {query_name}: {loads} loads");
                assert!(
                    records <= 2 * per_page,
                    "{name} {query_name} page {page_number}: loaded {records} records"
                );
                cursor = page.next;
            }
        }
        // `principal` is known only once the reader is: one more load, still one page.
        let writer = ctx("alpha", user("alpha:writer1"), 9_000);
        let (page, loads, records) =
            traced(store, &named("mine", &BTreeMap::new(), 20, None), &writer);
        assert_eq!(page.rows.len(), 20);
        assert!(
            loads <= 2 && records <= per_page,
            "{name} mine: {loads} loads, {records} records"
        );

        // A view of two sections loads their pages, not the type.
        let section = |query: &str, args: BTreeMap<String, ArgSource>, single: bool| ViewSection {
            marks: None,
            joins: None,
            query: QueryId::new(query).unwrap(),
            args,
            include: vec![],
            markdown: vec![],
            single,
            aggregate: None,
        };
        let view = ViewDef {
            id: ViewId::new("board").unwrap(),
            route: "/board".into(),
            title: "Board".into(),
            sections: BTreeMap::from([
                (
                    "owned".to_string(),
                    section(
                        "by_owner",
                        BTreeMap::from([("who".to_string(), ArgSource::Const(text("x")))]),
                        false,
                    ),
                ),
                (
                    "group".to_string(),
                    section(
                        "group_ordered",
                        BTreeMap::from([("g".to_string(), ArgSource::Request("g".into()))]),
                        false,
                    ),
                ),
            ]),
        };
        let route = BTreeMap::new();
        let request = BTreeMap::from([("g".to_string(), "g2".to_string())]);
        let request_of = |s: &ViewSection, done: &BTreeMap<String, Option<Arc<Record>>>| {
            section_request(s, &route, &request, done)
        };
        let schema = store
            .load(&tenant_id("alpha"), &[])
            .unwrap()
            .tenant
            .unwrap()
            .schema
            .clone();
        let initial = strata_substrate::view_needs::initial_needs(&schema, &view, &request_of);
        let covered = load_covering(store, &tenant_id("alpha"), initial, |state, asked| {
            view_reads(state, &context, &view, &request_of, asked)
        })
        .unwrap();
        assert!(covered.settled, "{name}: the view's reads are answered");
        assert!(covered.outcome.is_ok());
        assert!(
            covered.records <= 2 * per_page * covered.loads && covered.loads <= 2,
            "{name}: the view loaded {} records in {} loads",
            covered.records,
            covered.loads
        );
        let rows =
            |query: &str, params| read(&covered.state, &named(query, params, 50, None), &context);
        let owned = rows(
            "by_owner",
            &BTreeMap::from([("who".to_string(), text("x"))]),
        )
        .unwrap();
        assert_eq!(
            owned.rows.len(),
            50,
            "the view's snapshot answers its section"
        );
    }
}
