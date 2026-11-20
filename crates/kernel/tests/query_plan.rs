//! Index plans of named queries (KRN-2520 to KRN-2524): which queries are planned, that a planned
//! read returns exactly the rows a scan returns in the same order for every reader across pages,
//! that its loads are bounded whatever the size of the table, that its cursor resumes exactly
//! after records are inserted and deleted, and that no field the reader may not read shows
//! through the rows, their order or the page boundaries.
//!
//! The snapshots are cut from a complete tenant by the needs the kernel names, as a store does,
//! and a read that finds a need missing is asked again with it loaded (the loop of
//! `strata_storage_api::query`). The random data is drawn from a fixed seed.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use strata_kernel::query_plan::{
    self, plan_query, IndexPlan, PlanOrder, Probe, QueryPlan, INDEX_PAGE, MAX_PLAN_CANDIDATES,
};
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
    let mut changes = vec![SchemaChange::DefineObject {
        id: object("item"),
        label: "Item".into(),
    }];
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

fn item(id: u64, fields: &[(&str, Value)]) -> Arc<Record> {
    Arc::new(Record {
        id: RecordId(id),
        object: object("item"),
        version: 1,
        created_by: None,
        created_at: Timestamp(1),
        updated_at: Timestamp(1),
        fields: fields
            .iter()
            .filter(|(_, v)| *v != Value::Null || id % 7 == 0)
            .map(|(name, value)| (field(name), value.clone()))
            .collect(),
    })
}

/// The tenant of `world` with `items` added directly.
fn with_items(world: &World, items: impl IntoIterator<Item = Arc<Record>>) -> Arc<TenantState> {
    let mut tenant = TenantState::clone(&world.tenants[&tenant_id(TENANT)]);
    let records = tenant.records.entry(object("item")).or_default();
    for record in items {
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

/// A read through the store loop, and the most records one of its snapshots held.
fn run(tenant: &TenantState, request: &ReadRequest, context: &Context) -> (ReadResult, usize) {
    let mut wanted: BTreeSet<Need> = read_needs(&tenant.schema, request).into_iter().collect();
    let mut most = 0;
    for _ in 0..64 {
        let state = partial(tenant, &wanted);
        most = most.max(state.tenant.as_ref().unwrap().record_count());
        match read(&state, request, context) {
            Err(DomainError {
                kind: ErrorKind::InsufficientState,
                missing: Some(need),
                ..
            }) if !wanted.contains(&need) => {
                wanted.insert(need);
            }
            other => return (other.expect("the read is answered"), most),
        }
    }
    panic!("the read kept asking for more");
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
        _ => {}
    }
    params
}

// Verifies: KRN-2520
#[test]
fn each_shape_is_planned_and_everything_else_is_a_scan() {
    let world = item_world();
    let tenant = &world.tenants[&tenant_id(TENANT)];
    let schema = &tenant.schema;
    let plan = |name: &str| plan_query(schema, &schema.queries[&QueryId::new(name).unwrap()]);
    let index = |name: &str| match plan(name) {
        QueryPlan::Index(plan) => plan,
        other => panic!("{name} is not planned: {other:?}"),
    };
    assert!(matches!(
        index("by_owner"),
        IndexPlan {
            order: PlanOrder::Id,
            probe: Probe::Eq(_),
            ..
        }
    ));
    assert!(matches!(index("in_groups").probe, Probe::InList(_)));
    assert_eq!(index("score_range").field, field("score"));
    assert!(matches!(index("score_range").probe, Probe::Range(ref b) if b.len() == 2));
    assert_eq!(
        index("score_range_down").order,
        PlanOrder::Field { descending: true }
    );
    // The equality is chosen over the range when the order is the record id.
    assert_eq!(index("open_above").field, field("status"));
    assert!(matches!(index("groups_down").probe, Probe::InParam(_)));
    // `t < at` is `at > t`.
    assert!(
        matches!(index("recent").probe, Probe::Range(ref b) if b == &vec![(CmpOp::Gt, query_plan::Operand::Param("t".into()))])
    );
    assert!(matches!(
        index("mine").probe,
        Probe::Eq(query_plan::Operand::Principal)
    ));
    assert_eq!(index("group_by_id").order, PlanOrder::Id);
    for def in planned_queries() {
        assert!(
            plan(&format!("{}_scan", def.id)).is_scan(),
            "the twin of {} is a scan",
            def.id
        );
    }

    // What stays a scan, with its reason.
    let reason = |def: QueryDef| match plan_query(schema, &def) {
        QueryPlan::Scan(reason) => reason,
        other => panic!("{} is planned: {other:?}", def.id),
    };
    let hidden = reason(query("q", eq(rec("secret"), int(3)), &[], vec![]));
    assert!(hidden.contains("read condition"), "{hidden}");
    let text_range = reason(query(
        "q",
        cmp(CmpOp::Gt, rec("grp"), lit("g1")),
        &[],
        order("grp", false),
    ));
    assert!(text_range.contains("integer and time"), "{text_range}");
    let other_order = reason(query(
        "q",
        eq(rec("owner"), lit("x")),
        &[],
        order("score", false),
    ));
    assert!(
        other_order.contains("no condition on score"),
        "{other_order}"
    );
    let range_by_id = reason(query(
        "q",
        cmp(CmpOp::Gt, rec("score"), int(1)),
        &[],
        vec![],
    ));
    assert!(range_by_id.contains("record id order"), "{range_by_id}");
    let either = reason(query(
        "q",
        Expr::Or(vec![eq(rec("owner"), lit("x")), eq(rec("grp"), lit("g1"))]),
        &[],
        vec![],
    ));
    assert!(either.contains("top level"), "{either}");
    let system = reason(query(
        "q",
        eq(rec("owner"), lit("x")),
        &[],
        order("$created_at", false),
    ));
    assert!(system.contains("system attribute"), "{system}");
    let unfiltered = QueryDef {
        filter: None,
        ..query("q", lit("x"), &[], order("score", false))
    };
    assert!(reason(unfiltered).contains("no filter"));
    let two_keys = QueryDef {
        order: vec![
            OrderKey {
                field: "grp".into(),
                descending: false,
            },
            OrderKey {
                field: "score".into(),
                descending: false,
            },
        ],
        ..query("q", eq(rec("grp"), lit("g1")), &[], vec![])
    };
    assert!(reason(two_keys).contains("more than one key"));
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
        "by_owner_and_score",
        eq(rec("owner"), lit("x")),
        &[],
        order("score", true),
    ));
    let found = warnings(&scan);
    assert_eq!(found.len(), 1, "{:?}", scan.evidence);
    assert_eq!(found[0].rule, "query-plan:by_owner_and_score");
    assert!(found[0].passed, "a warning never refuses the change");
    assert!(
        found[0].detail.starts_with("warning:"),
        "{}",
        found[0].detail
    );
    assert!(found[0].detail.contains("10000"), "{}", found[0].detail);
    assert!(found[0].detail.contains("score"), "{}", found[0].detail);

    let planned = define(query("by_group", eq(rec("grp"), lit("g1")), &[], vec![]));
    assert!(warnings(&planned).is_empty(), "{:?}", planned.evidence);

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
// Verifies: KRN-2521
#[test]
fn every_plan_returns_the_rows_of_a_scan_in_the_same_order_across_pages() {
    let world = item_world();
    let mut rng = Lcg(0x5104);
    let mut compared = 0;
    let mut nonempty = 0;
    for dataset in 0..30 {
        let count = 150 + rng.below(250);
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

// Verifies: KRN-2520
#[test]
fn a_planned_query_over_a_hundred_thousand_records_loads_a_bounded_number() {
    let world = item_world();
    let tenant = with_items(
        &world,
        (1..=100_000).map(|id| {
            item(
                id,
                &[
                    ("owner", text(OWNERS[(id % 3) as usize])),
                    ("grp", text(GROUPS[(id % 5) as usize])),
                    ("score", Value::Int((id % 1_000) as i64)),
                    ("at", Value::Time(id as i64)),
                    ("status", text("open")),
                ],
            )
        }),
    );
    let context = ctx(TENANT, Principal::Anonymous, 9_000);
    let bound = 2 * INDEX_PAGE as usize;
    for (name, params) in [
        ("by_owner", BTreeMap::from([("who".to_string(), text("x"))])),
        (
            "score_range",
            BTreeMap::from([
                ("lo".to_string(), Value::Int(100)),
                ("hi".to_string(), Value::Int(900)),
            ]),
        ),
        (
            "recent",
            BTreeMap::from([("t".to_string(), Value::Time(500))]),
        ),
        (
            "groups_down",
            BTreeMap::from([("gs".to_string(), Value::List(vec![text("g1"), text("g2")]))]),
        ),
    ] {
        let (page, most) = run(&tenant, &named(name, &params, 20, None), &context);
        assert_eq!(page.rows.len(), 20, "{name}");
        assert!(most <= bound, "{name} loaded {most} records");
        // The next page is bounded the same way.
        let (_, most) = run(&tenant, &named(name, &params, 20, page.next), &context);
        assert!(
            most <= bound,
            "{name} loaded {most} records for its second page"
        );
    }
    // A scan of the same table is still refused (KRN-1288).
    let (scan, _) = {
        let request = named("by_owner_scan", &BTreeMap::new(), 20, None);
        let state = partial(
            &tenant,
            &read_needs(&tenant.schema, &request).into_iter().collect(),
        );
        (read(&state, &request, &context), ())
    };
    assert_eq!(scan.unwrap_err().kind, ErrorKind::LimitExceeded);
}

// Verifies: KRN-2521
#[test]
fn a_reader_who_sees_few_candidates_gets_short_pages_and_a_cursor_that_ends() {
    let world = item_world();
    // 5000 candidates of owner x; only every 2000th is open, so anonymous sees two, more than the
    // bound of candidates apart.
    let tenant = with_items(
        &world,
        (1..=5_000).map(|id| {
            item(
                id,
                &[
                    ("owner", text("x")),
                    ("grp", text("g1")),
                    ("score", Value::Int(id as i64)),
                    (
                        "status",
                        text(if id % 2_000 == 0 { "open" } else { "closed" }),
                    ),
                ],
            )
        }),
    );
    let context = ctx(TENANT, Principal::Anonymous, 9_000);
    let who = BTreeMap::from([("who".to_string(), text("x"))]);
    let (first, most) = run(&tenant, &named("by_owner", &who, 10, None), &context);
    assert!(first.rows.len() < 10, "a short page");
    assert!(
        most <= (MAX_PLAN_CANDIDATES + INDEX_PAGE) as usize,
        "examined no more than its bound: {most}"
    );
    assert_eq!(
        first.next,
        Some(PageCursor::After(RecordId(MAX_PLAN_CANDIDATES as u64)))
    );
    let (rows, _) = page_all(&tenant, "by_owner", &who, 10, &context);
    let (scanned, _) = page_all(&tenant, "by_owner_scan", &who, 10, &context);
    assert_eq!(rows, scanned);
    assert_eq!(rows.len(), 2);

    // Ordered by a field, a request that reached no record the reader may read answers from the
    // scan rather than put the value of a hidden record in its cursor (KRN-2522).
    let range = BTreeMap::from([
        ("lo".to_string(), Value::Int(1)),
        ("hi".to_string(), Value::Int(4_500)),
    ]);
    let (rows, cursors) = page_all(&tenant, "score_range", &range, 1, &context);
    let (scanned, _) = page_all(&tenant, "score_range_scan", &range, 1, &context);
    assert_eq!(rows, scanned);
    assert_eq!(rows.len(), 2);
    assert!(
        cursors.iter().all(|c| matches!(c, PageCursor::Offset(_))),
        "the first request reached no visible candidate, so the read went on as the scan: {cursors:?}"
    );
    for cursor in &cursors {
        if let PageCursor::At(at) = cursor {
            let record = tenant.record(&object("item"), at.id).unwrap();
            assert!(visible(
                &tenant,
                &Principal::Anonymous,
                record,
                Timestamp(9_000)
            ));
        }
    }
}

// Verifies: KRN-2521
#[test]
fn the_cursor_resumes_exactly_after_records_are_inserted_and_deleted() {
    let world = item_world();
    let mut rng = Lcg(77);
    let start: Vec<Arc<Record>> = (1..=400).map(|id| random_item(&mut rng, id)).collect();
    let before = with_items(&world, start.clone());
    let context = ctx(TENANT, user("alpha:editor"), 9_000);
    for (name, params) in [
        ("by_owner", BTreeMap::from([("who".to_string(), text("x"))])),
        (
            "score_range",
            BTreeMap::from([
                ("lo".to_string(), Value::Int(-15)),
                ("hi".to_string(), Value::Int(15)),
            ]),
        ),
        (
            "score_range_down",
            BTreeMap::from([
                ("lo".to_string(), Value::Int(-15)),
                ("hi".to_string(), Value::Int(15)),
            ]),
        ),
        (
            "groups_down",
            BTreeMap::from([("gs".to_string(), Value::List(vec![text("g0"), text("g4")]))]),
        ),
    ] {
        let (first, _) = run(&before, &named(name, &params, 15, None), &context);
        let cursor = first.next.clone().expect("more than one page");
        let last = first.rows.last().unwrap().clone();

        // Delete the last row returned and every third record, and insert new records, some of
        // which sort before the cursor and some after it.
        let mut changed: Vec<Arc<Record>> = start
            .iter()
            .filter(|r| r.id != last.id && r.id.0 % 3 != 0)
            .cloned()
            .collect();
        changed.extend((1_001..=1_200).map(|id| random_item(&mut rng, id)));
        let after = with_items(&world, []);
        let after = {
            let mut tenant = TenantState::clone(&after);
            tenant.records.insert(
                object("item"),
                changed.into_iter().map(|r| (r.id, r)).collect(),
            );
            Arc::new(tenant)
        };

        // What a scan of the changed table holds after the cursor's position.
        let def = &after.schema.queries[&QueryId::new(name).unwrap()];
        let QueryPlan::Index(plan) = plan_query(&after.schema, def) else {
            panic!("{name} is planned");
        };
        let beyond = |r: &Record| match (&cursor, plan.order) {
            (PageCursor::After(id), PlanOrder::Id) => r.id > *id,
            (PageCursor::At(at), PlanOrder::Field { descending }) => {
                query_plan::is_after(r.get(&plan.field), r.id, at, descending)
            }
            other => panic!("{other:?}"),
        };
        let (all, _) = page_all(&after, &format!("{name}_scan"), &params, 200, &context);
        let expected: Vec<u64> = all
            .iter()
            .filter(|r| beyond(&***r))
            .map(|r| r.id.0)
            .collect();

        let mut rest = Vec::new();
        let mut next = Some(cursor.clone());
        while let Some(c) = next {
            let (page, _) = run(&after, &named(name, &params, 15, Some(c)), &context);
            rest.extend(page.rows.iter().map(|r| r.id.0));
            next = page.next;
        }
        assert_eq!(
            rest, expected,
            "{name}: no gap and no repeat after the cursor"
        );
        let seen: BTreeSet<u64> = first.rows.iter().map(|r| r.id.0).collect();
        assert!(
            rest.iter().all(|id| !seen.contains(id)),
            "{name}: no repeat"
        );
    }
}

// Verifies: KRN-2522
#[test]
fn a_field_the_reader_may_not_read_does_not_show_through_rows_order_or_pages() {
    let world = item_world();
    let make = |secret: &dyn Fn(u64) -> i64| {
        with_items(
            &world,
            (1..=2_000).map(|id| {
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
    let one = make(&|id| (id % 10) as i64);
    let other = make(&|id| ((id * 7 + 3) % 10) as i64);
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
            let (a, b) = (with(&one), with(&other));
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

// Verifies: KRN-2520
#[test]
fn the_index_page_is_the_values_between_the_bounds_in_order_after_the_position() {
    use query_plan::{index_page, IndexBound, IndexPosition};
    let records: Vec<Arc<Record>> = [
        (1, Value::Int(5)),
        (2, Value::Int(3)),
        (3, Value::Int(5)),
        (4, text("5")),
        (5, Value::Null),
        (6, Value::List(vec![Value::Int(5)])),
        (7, Value::Int(9)),
        (8, Value::Time(5)),
    ]
    .into_iter()
    .map(|(id, v)| item(id, &[("score", v)]))
    .collect();
    let bound = |v: i64, inclusive| IndexBound {
        value: Value::Int(v),
        inclusive,
    };
    let ids = |page: Vec<&Arc<Record>>| page.iter().map(|r| r.id.0).collect::<Vec<_>>();
    let score = field("score");
    assert_eq!(
        ids(index_page(records.iter(), &score, &bound(3, true), &bound(9, true), false, None, 10)),
        vec![2, 1, 3, 7],
        "single values of the kind between the bounds, by value then id; not text, null, a list or a time"
    );
    assert_eq!(
        ids(index_page(
            records.iter(),
            &score,
            &bound(3, false),
            &bound(9, false),
            true,
            None,
            10
        )),
        vec![1, 3],
        "descending by value, ties by id ascending, bounds exclusive"
    );
    let at = IndexPosition {
        value: Value::Int(5),
        id: RecordId(1),
    };
    assert_eq!(
        ids(index_page(
            records.iter(),
            &score,
            &bound(3, true),
            &bound(9, true),
            false,
            Some(&at),
            10
        )),
        vec![3, 7]
    );
    assert_eq!(
        ids(index_page(
            records.iter(),
            &score,
            &bound(3, true),
            &bound(9, true),
            true,
            Some(&at),
            1
        )),
        vec![3],
        "descending after (5, 1): the rest of 5, then below it, at most the limit"
    );
}
