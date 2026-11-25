//! Index plans of named queries (KRN-2520 to KRN-2524, KRN-2920 to KRN-2923): which queries are
//! planned, that a planned read returns exactly the rows a scan returns in the same order for
//! every reader across pages, that its loads are bounded whatever the size of the table, that its
//! cursor resumes exactly after records are inserted and deleted, that no field the reader may not
//! read shows through the rows, their order or the page boundaries, that an equality ordered by
//! another attribute is served by a pair index, that a read whose rows follow references loads
//! exactly the records they name, and what a scan's warning says would plan it.
//!
//! The snapshots are cut from a complete tenant by the needs the kernel names, as a store does,
//! and a read that finds a need missing is asked again with it and everything
//! `query_plan::read_missing` names loaded (the loop of `strata_storage_api::query`). The random
//! data is drawn from a fixed seed.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use strata_kernel::query_plan::{self, plan_query};
use strata_kernel::testkit::*;
use strata_kernel::*;

const TENANT: &str = "alpha";

fn owner() -> Principal {
    user("alpha:owner")
}

fn int(n: i64) -> Expr {
    Expr::Lit(Value::Int(n))
}

fn param(name: &str) -> Expr {
    Expr::Param(name.into())
}

fn cmp(op: CmpOp, a: Expr, b: Expr) -> Expr {
    Expr::Cmp(op, Box::new(a), Box::new(b))
}

fn rec(name: &str) -> Expr {
    var(Scope::Record, name)
}

fn order(field: &str, descending: bool) -> Vec<OrderKey> {
    vec![OrderKey {
        field: field.into(),
        descending,
    }]
}

fn query(id: &str, filter: Expr, params: &[&str], order: Vec<OrderKey>) -> QueryDef {
    QueryDef {
        id: QueryId::new(id).unwrap(),
        object: object("item"),
        filter: Some(filter),
        params: params.iter().map(|p| p.to_string()).collect(),
        order,
        limit: None,
        aggregates: None,
        declared_fields: None,
        tree: None,
    }
}

/// The planned queries of the property test, each beside its scan twin: the same filter inside
/// a one-item `or`, which no plan serves and which holds exactly when the filter holds.
fn planned_queries() -> Vec<QueryDef> {
    vec![
        query("by_owner", eq(rec("owner"), param("who")), &["who"], vec![]),
        query(
            "in_groups",
            Expr::In(
                Box::new(rec("grp")),
                Box::new(Expr::Lit(Value::List(vec![
                    text("g1"),
                    text("g3"),
                    Value::Null,
                ]))),
            ),
            &[],
            vec![],
        ),
        query(
            "score_range",
            Expr::And(vec![
                cmp(CmpOp::Ge, rec("score"), param("lo")),
                cmp(CmpOp::Lt, rec("score"), param("hi")),
            ]),
            &["lo", "hi"],
            order("score", false),
        ),
        query(
            "score_range_down",
            Expr::And(vec![
                cmp(CmpOp::Ge, rec("score"), param("lo")),
                cmp(CmpOp::Lt, rec("score"), param("hi")),
            ]),
            &["lo", "hi"],
            order("score", true),
        ),
        query(
            "open_above",
            Expr::And(vec![
                eq(rec("status"), lit("open")),
                cmp(CmpOp::Gt, rec("score"), param("lo")),
            ]),
            &["lo"],
            vec![],
        ),
        query(
            "group_ordered",
            eq(rec("grp"), param("g")),
            &["g"],
            order("grp", false),
        ),
        query(
            "groups_down",
            Expr::In(Box::new(rec("grp")), Box::new(param("gs"))),
            &["gs"],
            order("grp", true),
        ),
        query(
            "recent",
            cmp(CmpOp::Lt, param("t"), rec("at")),
            &["t"],
            order("at", true),
        ),
        query("mine", eq(rec("owner"), Expr::Principal), &[], vec![]),
        query(
            "hidden_residual",
            Expr::And(vec![
                cmp(CmpOp::Gt, rec("score"), int(-5)),
                cmp(CmpOp::Gt, rec("secret"), int(3)),
            ]),
            &[],
            order("score", false),
        ),
        query(
            "group_by_id",
            eq(rec("grp"), param("g")),
            &["g"],
            order("$id", false),
        ),
        query(
            "score_eq_ordered",
            eq(rec("score"), param("s")),
            &["s"],
            order("score", true),
        ),
        // Pair plans (KRN-2920): an equality or a list of values on one field ordered by
        // another attribute.
        query(
            "owner_newest",
            eq(rec("owner"), param("who")),
            &["who"],
            order("$created_at", true),
        ),
        query(
            "owner_by_at",
            eq(param("who"), rec("owner")),
            &["who"],
            order("at", true),
        ),
        query(
            "groups_by_score",
            Expr::In(Box::new(rec("grp")), Box::new(param("gs"))),
            &["gs"],
            order("score", false),
        ),
        query(
            "mine_by_at",
            eq(rec("owner"), Expr::Principal),
            &[],
            order("at", false),
        ),
        query(
            "open_by_updated",
            Expr::And(vec![
                eq(rec("status"), lit("open")),
                cmp(CmpOp::Gt, rec("score"), int(-10)),
            ]),
            &[],
            order("$updated_at", false),
        ),
        query(
            "listed_groups_down",
            Expr::In(
                Box::new(rec("grp")),
                Box::new(Expr::Lit(Value::List(vec![text("g1"), text("g2")]))),
            ),
            &[],
            order("score", true),
        ),
        // A filter that follows a reference (KRN-2922).
        query(
            "owner_open_team",
            Expr::And(vec![
                eq(rec("owner"), param("who")),
                eq(
                    Expr::Through(Scope::Record, "team".into(), "open".into()),
                    Expr::Lit(Value::Bool(true)),
                ),
            ]),
            &["who"],
            order("$created_at", true),
        ),
    ]
}

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

/// Items: anyone reads an open one, a writer reads its own, an editor reads all. Only an editor
/// may read `secret`.
fn item_schema() -> Vec<SchemaChange> {
    let mut changes = vec![
        SchemaChange::DefineObject {
            id: object("team"),
            label: "Team".into(),
        },
        SchemaChange::DefineField {
            object: object("team"),
            field: field_def("open", FieldType::Bool, false),
        },
        SchemaChange::DefinePolicy(PolicyDef {
            id: PolicyId::new("public_reads_teams").unwrap(),
            role: role("public"),
            resource: Resource::Object(object("team")),
            actions: BTreeSet::from([Action::Read]),
            condition: None,
        }),
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
    ];
    for (name, ty) in [
        ("owner", FieldType::Str),
        ("grp", FieldType::Str),
        ("score", FieldType::Int),
        ("at", FieldType::Time),
        ("status", FieldType::Str),
    ] {
        changes.push(SchemaChange::DefineField {
            object: object("item"),
            field: field_def(name, ty, false),
        });
    }
    changes.push(SchemaChange::DefineField {
        object: object("item"),
        field: FieldDef {
            read: Some(Expr::HasRole(role("editor"))),
            ..field_def("secret", FieldType::Int, false)
        },
    });
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

fn item_world() -> World {
    notes_world()
        .step(
            &Intent::ChangeSchema {
                changes: item_schema(),
                expected_version: None,
                existing: ExistingData::Reject,
            },
            &ctx(TENANT, owner(), 3_000),
        )
        .expect("the item schema is accepted")
        .0
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
            .filter(|(_, v)| *v != Value::Null || id % 7 == 0)
            .map(|(name, value)| (field(name), value.clone()))
            .collect(),
    })
}

/// The teams of the tenants the tests build; an even one is open.
const TEAMS: u64 = 6;

fn team(id: u64) -> Arc<Record> {
    Arc::new(Record {
        id: RecordId(id),
        object: object("team"),
        version: 1,
        created_by: None,
        created_at: Timestamp(1),
        updated_at: Timestamp(1),
        fields: BTreeMap::from([(field("open"), Value::Bool(id % 2 == 0))]),
    })
}

/// The tenant of `world` with `items` and the teams added directly.
fn with_items(world: &World, items: impl IntoIterator<Item = Arc<Record>>) -> Arc<TenantState> {
    with_teams(world, items, (1..=TEAMS).map(team))
}

fn with_teams(
    world: &World,
    items: impl IntoIterator<Item = Arc<Record>>,
    teams: impl IntoIterator<Item = Arc<Record>>,
) -> Arc<TenantState> {
    let mut tenant = TenantState::clone(&world.tenants[&tenant_id(TENANT)]);
    let records = tenant.records.entry(object("item")).or_default();
    for record in items {
        records.insert(record.id, record);
    }
    let records = tenant.records.entry(object("team")).or_default();
    for record in teams {
        records.insert(record.id, record);
    }
    Arc::new(tenant)
}

/// A fixed-seed generator.
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

const OWNERS: [&str; 3] = ["alpha:writer1", "alpha:writer2", "x"];
const GROUPS: [&str; 5] = ["g0", "g1", "g2", "g3", "g4"];

/// A random item. Some values are missing or `null`, and some scores are text, as a value kept
/// by a schema change that grandfathered it would be.
fn random_item(rng: &mut Lcg, id: u64) -> Arc<Record> {
    let score = match rng.below(10) {
        0 => Value::Null,
        1 => text(&format!("{}", rng.below(5))),
        _ => Value::Int(rng.below(40) as i64 - 20),
    };
    let at = match rng.below(6) {
        0 => Value::Null,
        _ => Value::Time(rng.below(50) as i64),
    };
    let team = match rng.below(5) {
        0 => Value::Null,
        _ => Value::Ref(RecordId(1 + rng.below(TEAMS))),
    };
    item(
        id,
        &[
            ("owner", text(rng.pick(&OWNERS))),
            ("grp", text(rng.pick(&GROUPS))),
            ("score", score),
            ("at", at),
            (
                "status",
                text(if rng.below(3) == 0 { "closed" } else { "open" }),
            ),
            ("secret", Value::Int(rng.below(10) as i64)),
            ("team", team),
        ],
    )
}

/// What a store hands the kernel for `wanted`.
fn partial(tenant: &TenantState, wanted: &BTreeSet<Need>) -> State {
    let mut thin = TenantState::clone(tenant);
    thin.records.clear();
    for need in wanted {
        let found: Vec<Arc<Record>> = match need {
            Need::Record { object, id } => {
                tenant.record(object, *id).into_iter().cloned().collect()
            }
            Need::AllOf(object) => tenant.records_of(object).cloned().collect(),
            Need::PageOf {
                object,
                after,
                limit,
            } => tenant
                .records_of(object)
                .filter(|r| after.is_none_or(|a| r.id > a))
                .take(*limit as usize)
                .cloned()
                .collect(),
            Need::IndexPage {
                object,
                field,
                lower,
                upper,
                descending,
                after,
                limit,
            } => query_plan::index_page(
                tenant.records_of(object),
                field,
                lower,
                upper,
                *descending,
                after.as_ref(),
                *limit,
            )
            .into_iter()
            .cloned()
            .collect(),
            Need::PairPage {
                object,
                field,
                value,
                order,
                descending,
                after,
                limit,
            } => query_plan::pair_page(
                tenant.records_of(object),
                field,
                value,
                order,
                *descending,
                after.as_ref(),
                *limit,
            )
            .into_iter()
            .cloned()
            .collect(),
            other => panic!("a named query read does not ask for {other:?}"),
        };
        for record in found {
            thin.records
                .entry(record.object.clone())
                .or_default()
                .insert(record.id, record);
        }
    }
    State {
        tenant_id: tenant.id.clone(),
        tenant: Some(Arc::new(thin)),
        coverage: Coverage::Partial(wanted.clone()),
    }
}

/// What a read through the store loop loaded.
struct Loaded {
    /// The most records one of its snapshots held.
    most: usize,
    /// How many snapshots it took.
    rounds: usize,
    /// Every need of its last snapshot.
    wanted: BTreeSet<Need>,
    /// Its last snapshot.
    last: State,
}

/// A read through the store loop: a need found missing is loaded with everything
/// `read_missing` names, as `strata_storage_api::query` does.
fn traced(tenant: &TenantState, request: &ReadRequest, context: &Context) -> (ReadResult, Loaded) {
    let mut wanted: BTreeSet<Need> = read_needs(&tenant.schema, request).into_iter().collect();
    let mut most = 0;
    for rounds in 1..=64 {
        let state = partial(tenant, &wanted);
        most = most.max(state.tenant.as_ref().unwrap().record_count());
        match read(&state, request, context) {
            Err(DomainError {
                kind: ErrorKind::InsufficientState,
                missing: Some(need),
                ..
            }) if !wanted.contains(&need) => {
                wanted.extend(query_plan::read_missing(&state, request, context));
                wanted.insert(need);
            }
            other => {
                let result = other.expect("the read is answered");
                let loaded = Loaded {
                    most,
                    rounds,
                    wanted,
                    last: state,
                };
                return (result, loaded);
            }
        }
    }
    panic!("the read kept asking for more");
}

/// A read through the store loop, and the most records one of its snapshots held.
fn run(tenant: &TenantState, request: &ReadRequest, context: &Context) -> (ReadResult, usize) {
    let (result, loaded) = traced(tenant, request, context);
    (result, loaded.most)
}

fn named(
    name: &str,
    params: &BTreeMap<String, Value>,
    page_size: u32,
    cursor: Option<PageCursor>,
) -> ReadRequest {
    ReadRequest::Named {
        query: QueryId::new(name).unwrap(),
        params: params.clone(),
        page_size: Some(page_size),
        cursor,
    }
}

/// Every row of a query, page by page, and the cursors it issued.
fn page_all(
    tenant: &TenantState,
    name: &str,
    params: &BTreeMap<String, Value>,
    size: u32,
    context: &Context,
) -> (Vec<Arc<Record>>, Vec<PageCursor>) {
    let (mut rows, mut cursors) = (Vec::new(), Vec::new());
    let mut cursor = None;
    for _ in 0..10_000 {
        let (page, _) = run(tenant, &named(name, params, size, cursor.clone()), context);
        assert!(page.rows.len() <= size as usize);
        rows.extend(page.rows);
        match page.next {
            None => return (rows, cursors),
            Some(next) => {
                assert_ne!(
                    Some(&next),
                    cursor.as_ref(),
                    "{name}: the cursor does not move"
                );
                cursors.push(next.clone());
                cursor = Some(next);
            }
        }
    }
    panic!("{name} never ends");
}

fn readers() -> Vec<Principal> {
    vec![
        Principal::Anonymous,
        user("alpha:writer1"),
        user("alpha:writer2"),
        user("alpha:editor"),
        owner(),
        user("alpha:stranger"),
        Principal::System,
    ]
}

fn params_of(rng: &mut Lcg, name: &str) -> BTreeMap<String, Value> {
    let number = |rng: &mut Lcg| match rng.below(12) {
        0 => Value::Null,
        1 => text("3"),
        _ => Value::Int(rng.below(44) as i64 - 22),
    };
    let mut params = BTreeMap::new();
    match name {
        "by_owner" => {
            let who = match rng.below(5) {
                0 => Value::Null,
                1 => text("nobody"),
                _ => text(rng.pick(&OWNERS)),
            };
            params.insert("who".into(), who);
        }
        "score_range" | "score_range_down" => {
            params.insert("lo".into(), number(&mut *rng));
            params.insert("hi".into(), number(&mut *rng));
        }
        "open_above" => {
            params.insert("lo".into(), number(&mut *rng));
        }
        "group_ordered" | "group_by_id" => {
            params.insert("g".into(), text(rng.pick(&GROUPS)));
        }
        "groups_down" => {
            let gs = match rng.below(6) {
                0 => Value::Null,
                1 => text("g1"),
                _ => Value::List(
                    GROUPS
                        .iter()
                        .filter(|_| rng.below(2) == 0)
                        .map(|g| text(g))
                        .chain([Value::Null])
                        .collect(),
                ),
            };
            params.insert("gs".into(), gs);
        }
        "recent" => {
            let t = match rng.below(8) {
                0 => Value::Int(10),
                _ => Value::Time(rng.below(55) as i64 - 3),
            };
            params.insert("t".into(), t);
        }
        "score_eq_ordered" => {
            params.insert("s".into(), number(&mut *rng));
        }
        "owner_newest" | "owner_by_at" | "owner_open_team" => {
            let who = match rng.below(6) {
                0 => Value::Null,
                1 => text("nobody"),
                // Longer than the index key limit: answered by the scan.
                2 => text(&"w".repeat(300)),
                _ => text(rng.pick(&OWNERS)),
            };
            params.insert("who".into(), who);
        }
        "groups_by_score" => {
            let gs = match rng.below(5) {
                0 => Value::Null,
                _ => Value::List(
                    GROUPS
                        .iter()
                        .filter(|_| rng.below(2) == 0)
                        .map(|g| text(g))
                        .collect(),
                ),
            };
            params.insert("gs".into(), gs);
        }
        _ => {}
    }
    params
}

// Verifies: KRN-2524
#[test]
fn a_schema_change_says_which_queries_are_scans_and_why() {
    let world = item_world();
    let define = |def: QueryDef| {
        world
            .step(
                &Intent::ChangeSchema {
                    changes: vec![SchemaChange::DefineQuery(def)],
                    expected_version: None,
                    existing: ExistingData::Reject,
                },
                &ctx(TENANT, owner(), 4_000),
            )
            .expect("the query is accepted")
            .1
    };
    let warnings = |t: &Transition| -> Vec<Check> {
        t.evidence
            .iter()
            .filter(|c| c.rule.starts_with("query-plan:"))
            .cloned()
            .collect()
    };
    let scan = define(query(
        "by_owner_and_status",
        eq(rec("owner"), lit("x")),
        &[],
        order("status", true),
    ));
    let found = warnings(&scan);
    assert_eq!(found.len(), 1, "{:?}", scan.evidence);
    assert_eq!(found[0].rule, "query-plan:by_owner_and_status");
    assert!(found[0].passed, "a warning never refuses the change");
    assert!(
        found[0].detail.starts_with("warning:"),
        "{}",
        found[0].detail
    );
    assert!(found[0].detail.contains("10000"), "{}", found[0].detail);
    assert!(found[0].detail.contains("status"), "{}", found[0].detail);

    let planned = define(query("by_group", eq(rec("grp"), lit("g1")), &[], vec![]));
    assert!(warnings(&planned).is_empty(), "{:?}", planned.evidence);
    // "Posts by this author, newest first" is planned, so its definition warns of nothing
    // (WG-5181).
    for (name, attribute) in [("by_owner_newest", "$created_at"), ("by_owner_at", "at")] {
        let paired = define(query(
            name,
            eq(rec("owner"), param("who")),
            &["who"],
            order(attribute, true),
        ));
        assert!(warnings(&paired).is_empty(), "{:?}", paired.evidence);
    }

    // A read condition put on the field turns its planned query into a scan, and says so.
    let hide = world
        .step(
            &Intent::ChangeSchema {
                changes: vec![SchemaChange::DefineField {
                    object: object("item"),
                    field: FieldDef {
                        read: Some(Expr::HasRole(role("editor"))),
                        ..field_def("owner", FieldType::Str, false)
                    },
                }],
                expected_version: None,
                existing: ExistingData::Reject,
            },
            &ctx(TENANT, owner(), 4_001),
        )
        .expect("the field change is accepted")
        .1;
    let rules: BTreeSet<String> = warnings(&hide).into_iter().map(|c| c.rule).collect();
    assert!(rules.contains("query-plan:by_owner"), "{rules:?}");
    assert!(rules.contains("query-plan:mine"), "{rules:?}");
    assert!(
        !rules.contains("query-plan:by_owner_scan"),
        "a query that was already a scan is not warned of again"
    );
}

// Verifies: KRN-2523
#[test]
fn every_plan_returns_the_rows_of_a_scan_in_the_same_order_across_pages() {
    let world = item_world();
    let mut rng = Lcg(0x5104);
    let mut compared = 0;
    let mut nonempty = 0;
    for dataset in 0..30 {
        // Enough items for several pages of every size drawn (up to 25) and for every plan to
        // match some of them; the cost of a page grows with the items, so no more.
        let count = 60 + rng.below(60);
        let tenant = with_items(&world, (1..=count).map(|id| random_item(&mut rng, id)));
        for def in planned_queries() {
            let name = def.id.as_str().to_string();
            let reader = rng.pick(&readers()).clone();
            let size = 1 + rng.below(25) as u32;
            let params = params_of(&mut rng, &name);
            let context = ctx(TENANT, reader.clone(), 9_000);
            let (planned, _) = page_all(&tenant, &name, &params, size, &context);
            let (scanned, _) = page_all(&tenant, &format!("{name}_scan"), &params, size, &context);
            assert_eq!(
                planned.iter().map(|r| r.id.0).collect::<Vec<_>>(),
                scanned.iter().map(|r| r.id.0).collect::<Vec<_>>(),
                "dataset {dataset}: {name} with {params:?} as {reader:?}, pages of {size}"
            );
            assert_eq!(planned, scanned, "the rows are redacted alike");
            compared += 1;
            nonempty += usize::from(!planned.is_empty());
        }
    }
    assert!(
        compared >= 300 && nonempty > compared / 4,
        "{nonempty} of {compared}"
    );
}

// Verifies: KRN-2521
#[test]
fn an_index_read_examines_at_most_1600_candidates_and_its_cursor_is_a_position() {
    let world = item_world();
    // Two thousand items scored id % 37, so the score index holds them in the order (score, id).
    let tenant = with_items(
        &world,
        (1..=2_000).map(|id| {
            item(
                id,
                &[
                    ("owner", text("x")),
                    ("grp", text("g1")),
                    ("score", Value::Int((id % 37) as i64)),
                    ("status", text("open")),
                    ("secret", Value::Int((id % 10) as i64)),
                ],
            )
        }),
    );
    let mut candidates: Vec<(i64, u64)> = (1..=2_000).map(|id| ((id % 37) as i64, id)).collect();
    candidates.sort();
    // The anonymous reader may not read `secret`, so `hidden_residual` rejects every candidate.
    let context = ctx(TENANT, Principal::Anonymous, 9_000);
    let mut cursor = None;
    let mut examined_before = 0;
    let mut pages = 0;
    loop {
        let (page, _) = run(
            &tenant,
            &named("hidden_residual", &BTreeMap::new(), 10, cursor.clone()),
            &context,
        );
        pages += 1;
        assert!(page.rows.is_empty(), "no candidate passes the filter");
        let Some(next) = page.next else { break };
        let PageCursor::At(position) = &next else {
            panic!("the cursor of an index read is a position, got {next:?}");
        };
        let Value::Int(score) = position.value else {
            panic!("the position holds the score: {position:?}");
        };
        let at = candidates
            .iter()
            .position(|c| *c == (score, position.id.0))
            .expect("the position is a candidate")
            + 1;
        // Fewer rows than the page size (here none) only because 1600 were examined.
        assert_eq!(at - examined_before, if pages == 1 { 1_600 } else { 400 });
        examined_before = at;
        cursor = Some(next);
    }
    assert_eq!(pages, 2, "1600 candidates, then the 400 that remain");
}

// Verifies: KRN-2521
#[test]
fn an_index_cursor_continues_after_its_position_whatever_changed_in_between() {
    let world = item_world();
    let group_item = |id: u64| {
        item(
            id,
            &[
                ("owner", text("x")),
                ("grp", text("g1")),
                ("score", Value::Int(1)),
                ("status", text("open")),
            ],
        )
    };
    // Even ids only.
    let before = with_items(&world, (1..=60).map(|n| group_item(2 * n)));
    let context = ctx(TENANT, owner(), 9_000);
    let params = BTreeMap::from([("g".to_string(), text("g1"))]);
    let (first, _) = run(&before, &named("group_ordered", &params, 5, None), &context);
    let ids = |rows: &[Arc<Record>]| rows.iter().map(|r| r.id.0).collect::<Vec<_>>();
    assert_eq!(ids(&first.rows), [2, 4, 6, 8, 10]);
    let cursor = first.next.expect("more rows may follow");
    assert!(
        matches!(&cursor, PageCursor::At(p) if p.id == RecordId(10) && p.value == text("g1")),
        "{cursor:?}"
    );
    // Before the next read: odd ids inserted on both sides of the position, and the records 4
    // (before it) and 12 (after it) deleted.
    let mut after = TenantState::clone(&before);
    let records = after.records.get_mut(&object("item")).unwrap();
    for id in [1, 3, 9, 11, 13, 121] {
        records.insert(RecordId(id), group_item(id));
    }
    records.remove(&RecordId(4));
    records.remove(&RecordId(12));
    let (second, _) = run(
        &Arc::new(after),
        &named("group_ordered", &params, 5, Some(cursor)),
        &context,
    );
    assert_eq!(ids(&second.rows), [11, 13, 14, 16, 18]);
}

// Verifies: KRN-2522
#[test]
fn a_field_the_reader_may_not_read_does_not_show_through_rows_order_or_pages() {
    let world = item_world();
    // `count` items: two thousand for the index read, which has to examine more than 1600
    // candidates for its cursor to be a position that could carry the hidden field, and a
    // hundred for the scans, whose every page reads the whole tenant.
    let make = |count: u64, secret: &dyn Fn(u64) -> i64| {
        with_items(
            &world,
            (1..=count).map(|id| {
                item(
                    id,
                    &[
                        ("owner", text("x")),
                        ("grp", text(GROUPS[(id % 5) as usize])),
                        ("score", Value::Int((id % 37) as i64)),
                        ("status", text("open")),
                        ("secret", Value::Int(secret(id))),
                    ],
                )
            }),
        )
    };
    let by_id = |id: u64| (id % 10) as i64;
    let by_mix = |id: u64| ((id * 7 + 3) % 10) as i64;
    let one = make(2_000, &by_id);
    let other = make(2_000, &by_mix);
    let (small_one, small_other) = (make(100, &by_id), make(100, &by_mix));
    let filtered_by_secret = query(
        "by_secret",
        cmp(CmpOp::Gt, rec("secret"), int(3)),
        &[],
        vec![],
    );
    let ordered_by_secret = QueryDef {
        filter: Some(eq(rec("status"), lit("open"))),
        ..query("q", lit("x"), &[], order("secret", true))
    };
    let schema = &one.schema;
    assert!(plan_query(schema, &filtered_by_secret).is_scan());
    assert!(plan_query(schema, &ordered_by_secret).is_scan());

    for reader in [Principal::Anonymous, user("alpha:writer1"), owner()] {
        let context = ctx(TENANT, reader.clone(), 9_000);
        for size in [7, 50] {
            // Planned on score, with a condition on the hidden field.
            let a = page_all(&one, "hidden_residual", &BTreeMap::new(), size, &context);
            let b = page_all(&other, "hidden_residual", &BTreeMap::new(), size, &context);
            assert_eq!(
                a, b,
                "as {reader:?}: rows and cursors do not depend on secret"
            );
            assert!(a.0.is_empty(), "a hidden field is null to the filter");
            for row in &a.0 {
                assert!(!row.fields.contains_key(&field("secret")));
            }
        }
        // The same through the scan of a query that filters or orders by the hidden field.
        for def in [&filtered_by_secret, &ordered_by_secret] {
            let with = |tenant: &Arc<TenantState>| {
                let mut tenant = TenantState::clone(tenant);
                let mut schema = TenantSchema::clone(&tenant.schema);
                schema.queries.insert(def.id.clone(), def.clone());
                tenant.schema = Arc::new(schema);
                tenant
            };
            let (a, b) = (with(&small_one), with(&small_other));
            let x = page_all(&a, def.id.as_str(), &BTreeMap::new(), 9, &context);
            let y = page_all(&b, def.id.as_str(), &BTreeMap::new(), 9, &context);
            assert_eq!(x, y, "{} as {reader:?}", def.id);
        }
    }

    // An editor may read the field, and the filter sees it.
    let context = ctx(TENANT, user("alpha:editor"), 9_000);
    let (rows, _) = page_all(&one, "hidden_residual", &BTreeMap::new(), 50, &context);
    assert!(!rows.is_empty());
    assert!(rows
        .iter()
        .all(|r| matches!(r.get(&field("secret")), Value::Int(n) if *n > 3)));
}

// Verifies: KRN-2923
#[test]
fn a_scan_warning_names_the_one_change_that_would_plan_it() {
    let world = item_world();
    let tenant = &world.tenants[&tenant_id(TENANT)];
    let schema = &tenant.schema;
    let hint = |def: QueryDef| {
        assert!(plan_query(schema, &def).is_scan(), "{} is a scan", def.id);
        query_plan::plan_hint(schema, &def)
    };
    let by_status = hint(query(
        "q",
        eq(rec("owner"), lit("x")),
        &[],
        order("status", true),
    ));
    assert!(
        by_status.contains("ordering by $created_at descending instead of status")
            && by_status.contains("pair index (owner, $created_at)"),
        "{by_status}"
    );
    let two_keys = hint(QueryDef {
        order: vec![
            OrderKey {
                field: "at".into(),
                descending: false,
            },
            OrderKey {
                field: "status".into(),
                descending: false,
            },
        ],
        ..query("q", eq(rec("owner"), lit("x")), &[], vec![])
    });
    assert!(
        two_keys.contains("ordering by at alone") && two_keys.contains("pair index (owner, at)"),
        "{two_keys}"
    );
    let hidden = hint(query("q", eq(rec("secret"), int(3)), &[], vec![]));
    assert!(
        hidden.contains("removing the read condition of secret")
            && hidden.contains("the index of secret"),
        "{hidden}"
    );
    let down = hint(query(
        "q",
        eq(rec("grp"), lit("g1")),
        &[],
        order("$id", true),
    ));
    assert!(
        down.contains("ordering by the record id ascending"),
        "{down}"
    );
    // When no single change plans it, the hint says so.
    let hidden_pair = hint(QueryDef {
        order: vec![
            OrderKey {
                field: "secret".into(),
                descending: false,
            },
            OrderKey {
                field: "status".into(),
                descending: false,
            },
        ],
        ..query("q", eq(rec("owner"), lit("x")), &[], vec![])
    });
    assert!(hidden_pair.contains("no single change"), "{hidden_pair}");
    let unequal = hint(query(
        "q",
        cmp(CmpOp::Ne, rec("owner"), lit("x")),
        &[],
        order("at", false),
    ));
    assert!(unequal.contains("no single change"), "{unequal}");

    // The warning of the schema change carries the hint.
    let (_, transition) = world
        .step(
            &Intent::ChangeSchema {
                changes: vec![SchemaChange::DefineQuery(query(
                    "owner_by_status",
                    eq(rec("owner"), param("who")),
                    &["who"],
                    order("status", true),
                ))],
                expected_version: None,
                existing: ExistingData::Reject,
            },
            &ctx(TENANT, owner(), 5_000),
        )
        .expect("the query is accepted");
    let warning = transition
        .evidence
        .iter()
        .find(|c| c.rule == "query-plan:owner_by_status")
        .expect("a scan warning");
    assert!(
        warning
            .detail
            .contains("To serve it from an index: ordering by $created_at descending"),
        "{}",
        warning.detail
    );
}

// Verifies: KRN-2922
#[test]
fn a_read_whose_rows_follow_references_loads_exactly_the_records_they_name() {
    let world = item_world();
    // Ten thousand items, each on its own team of twenty thousand; the team of every fourth item
    // is closed.
    let tenant = with_teams(
        &world,
        (1..=10_000).map(|id| {
            let team_id = if id % 4 == 0 { 2 * id - 1 } else { 2 * id };
            item(
                id,
                &[
                    ("owner", text(OWNERS[(id % 3) as usize])),
                    ("status", text("open")),
                    ("team", Value::Ref(RecordId(team_id))),
                ],
            )
        }),
        (1..=20_000).map(team),
    );
    let complete = State::complete(tenant.clone());
    let who = BTreeMap::from([("who".to_string(), text("x"))]);
    let context = ctx(TENANT, user("alpha:editor"), 9_000);
    // 1600 candidates examined, and the page of candidates the read stopped at: 200.
    let examined = 1_600 + 200;
    let mut cursor = None;
    for size in [1, 20, 200, 200] {
        let request = named("owner_open_team", &who, size, cursor.clone());
        let (page, loaded) = traced(&tenant, &request, &context);
        assert_eq!(
            page,
            read(&complete, &request, &context).unwrap(),
            "pages of {size}: the read of the complete state"
        );
        assert_eq!(page.rows.len(), size as usize);
        let wide: Vec<&Need> = loaded
            .wanted
            .iter()
            .filter(|need| matches!(need, Need::AllOf(_) | Need::PageOf { .. }))
            .collect();
        assert!(
            wide.is_empty(),
            "pages of {size}: no object type and no scan is loaded: {wide:?}"
        );
        let teams = loaded
            .last
            .tenant
            .as_ref()
            .unwrap()
            .records_of(&object("team"))
            .count();
        let named_teams = loaded
            .wanted
            .iter()
            .filter(|need| matches!(need, Need::Record { object: o, .. } if *o == object("team")))
            .count();
        assert_eq!(teams, named_teams, "each team loaded was named alone");
        assert!(
            teams <= examined,
            "pages of {size}: {teams} of 20000 teams loaded"
        );
        assert!(
            loaded.rounds <= 2 + 8,
            "pages of {size}: {} rounds",
            loaded.rounds
        );
        cursor = page.next;
    }
}
