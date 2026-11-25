//! Aggregates of named queries (AGG-001 to AGG-042): each kind against a hand-computed result,
//! grouping, the edges of an empty set, exact sums, the scan bound at its edge, what a reader may
//! and may not count, and the definition-time refusals.
//!
//! The notes world of the test kit has two things a reader's view depends on: the public reads
//! published notes only, and writers read every note. The tests add a `team` field and a `score`
//! field that only an editor or the owner may read, and a query `stats` that declares the
//! aggregates under test.

use proptest::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use strata_kernel::limits;
use strata_kernel::testkit::*;
use strata_kernel::*;

fn owner() -> Principal {
    user("alpha:owner")
}
fn editor() -> Principal {
    user("alpha:editor")
}
fn int(n: i64) -> Value {
    Value::Int(n)
}

fn agg_id(name: &str) -> AggregateId {
    AggregateId::new(name).expect("aggregate id")
}

fn aggregate(
    id: &str,
    kind: AggregateKind,
    field_name: Option<&str>,
    group: Option<&str>,
    n: Option<u32>,
    descending: bool,
    scan_bound: u32,
) -> AggregateDef {
    AggregateDef {
        id: agg_id(id),
        kind,
        field: field_name.map(field),
        group_by: group.map(field),
        group_by_second: None,
        n,
        descending,
        scan_bound,
    }
}

/// The fields `stats` declares: the ones an aggregate or a request filter may name.
const DECLARED: [&str; 4] = ["rank", "status", "team", "score"];

/// The aggregates of `stats`. Every one has a bound of at least 100 but `tight`, which is 3.
fn all_aggregates() -> Vec<AggregateDef> {
    use AggregateKind::*;
    vec![
        aggregate("count_all", Count, None, None, None, false, 100),
        aggregate("count_by_team", Count, None, Some("team"), None, false, 100),
        aggregate("sum_rank", Sum, Some("rank"), None, None, false, 100),
        aggregate(
            "sum_by_team",
            Sum,
            Some("rank"),
            Some("team"),
            None,
            false,
            100,
        ),
        aggregate("min_rank", Minimum, Some("rank"), None, None, false, 100),
        aggregate("max_rank", Maximum, Some("rank"), None, None, false, 100),
        aggregate("top_rank", Top, Some("rank"), None, Some(3), true, 100),
        aggregate("bottom_rank", Top, Some("rank"), None, Some(2), false, 100),
        aggregate("sum_score", Sum, Some("score"), None, None, false, 100),
        aggregate(
            "count_by_score",
            Count,
            None,
            Some("score"),
            None,
            false,
            100,
        ),
        aggregate(
            "max_by_team",
            Maximum,
            Some("rank"),
            Some("team"),
            None,
            false,
            100,
        ),
        aggregate("wide", Count, None, None, None, false, 10_000),
        aggregate("tight", Count, None, None, None, false, 3),
    ]
}

fn stats_query(aggs: Vec<AggregateDef>, declared: &[&str]) -> QueryDef {
    QueryDef {
        id: QueryId::new("stats").unwrap(),
        object: object("note"),
        filter: None,
        params: BTreeSet::new(),
        order: vec![],
        limit: None,
        aggregates: Some(aggs),
        declared_fields: Some(declared.iter().map(|f| field(f)).collect()),
    }
}

/// The query a reader would use to see the rows themselves: the same object type, no filter.
fn plain_query() -> QueryDef {
    QueryDef {
        id: QueryId::new("plain").unwrap(),
        object: object("note"),
        filter: None,
        params: BTreeSet::new(),
        order: vec![],
        limit: None,
        aggregates: None,
        declared_fields: None,
    }
}

fn field_changes() -> Vec<SchemaChange> {
    let only_editors = Expr::Or(vec![
        Expr::HasRole(role("editor")),
        Expr::HasRole(owner_role()),
    ]);
    vec![
        SchemaChange::DefineField {
            object: object("note"),
            field: field_def("team", FieldType::Str, false),
        },
        SchemaChange::DefineField {
            object: object("note"),
            field: FieldDef {
                read: Some(only_editors),
                ..field_def("score", FieldType::Int, false)
            },
        },
        SchemaChange::DefineField {
            object: object("note"),
            field: field_def("tags", FieldType::List(Box::new(FieldType::Str)), false),
        },
    ]
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

/// The notes world with the new fields, `stats` and `plain`.
fn base() -> World {
    let mut changes = field_changes();
    changes.push(SchemaChange::DefineQuery(stats_query(
        all_aggregates(),
        &DECLARED,
    )));
    changes.push(SchemaChange::DefineQuery(plain_query()));
    change(&notes_world(), 3_000, changes).expect("the schema is accepted")
}

/// What one note holds.
#[derive(Clone, Debug)]
struct Row {
    rank: Option<i64>,
    published: bool,
    team: Option<&'static str>,
    score: Option<i64>,
}

fn row(rank: Option<i64>, published: bool, team: Option<&'static str>, score: Option<i64>) -> Row {
    Row {
        rank,
        published,
        team,
        score,
    }
}

/// A slug no note of any world built in this process has: notes are added to worlds that already
/// hold notes, so the slugs of one call cannot be numbered from zero.
fn next_slug() -> usize {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Add the notes, one intent each: created as the owner and, when published, published by the
/// editor, as the workflow of the notes requires. Returns the ids in order.
fn populate(world: &World, rows: &[Row]) -> (World, Vec<RecordId>) {
    let mut world = world.clone();
    let mut ids = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let now = 4_000 + 2 * i as i64;
        let mut fields = BTreeMap::from([
            (field("title"), text(&format!("N{i}"))),
            (field("slug"), text(&format!("s{}", next_slug()))),
        ]);
        if let Some(rank) = row.rank {
            fields.insert(field("rank"), int(rank));
        }
        if let Some(team) = row.team {
            fields.insert(field("team"), text(team));
        }
        if let Some(score) = row.score {
            fields.insert(field("score"), int(score));
        }
        let (next, done) = world
            .step(
                &Intent::CreateRecord {
                    object: object("note"),
                    fields,
                },
                &ctx("alpha", owner(), now),
            )
            .expect("the note is created");
        world = next;
        let Output::Record { id, .. } = done.output else {
            panic!("a created record is the output");
        };
        if row.published {
            let publish = Intent::UpdateRecord {
                object: object("note"),
                id,
                fields: BTreeMap::from([(field("status"), text("published"))]),
                expected_version: None,
            };
            world = world
                .step(&publish, &ctx("alpha", editor(), now + 1))
                .expect("the note is published")
                .0;
        }
        ids.push(id);
    }
    (world, ids)
}

/// The six notes of the hand computations:
///
/// | note | rank | status    | team | score |
/// |------|------|-----------|------|-------|
/// | n1   | 5    | published | red  | 10    |
/// | n2   | 3    | published | red  | 20    |
/// | n3   | 8    | published | blue | 30    |
/// | n4   | 8    | draft     | blue | 40    |
/// | n5   | 2    | draft     |      | 50    |
/// | n6   |      | published | blue |       |
fn six() -> (World, Vec<RecordId>) {
    populate(
        &base(),
        &[
            row(Some(5), true, Some("red"), Some(10)),
            row(Some(3), true, Some("red"), Some(20)),
            row(Some(8), true, Some("blue"), Some(30)),
            row(Some(8), false, Some("blue"), Some(40)),
            row(Some(2), false, None, Some(50)),
            row(None, true, Some("blue"), None),
        ],
    )
}

fn request(aggregate_name: &str, filter: &[(&str, Value)]) -> AggregateRequest {
    AggregateRequest {
        query: QueryId::new("stats").unwrap(),
        aggregate: agg_id(aggregate_name),
        params: BTreeMap::new(),
        filter: filter.iter().map(|(f, v)| (field(f), v.clone())).collect(),
    }
}

fn run_as(
    world: &World,
    who: &Principal,
    name: &str,
    filter: &[(&str, Value)],
) -> Result<AggregateResult, DomainError> {
    let state = world.state(&tenant_id("alpha"));
    aggregates::run(
        &state,
        &request(name, filter),
        &ctx("alpha", who.clone(), 90_000),
    )
}

fn ok_as(world: &World, who: &Principal, name: &str) -> AggregateResult {
    run_as(world, who, name, &[]).unwrap_or_else(|e| panic!("{name} as {who:?}: {}", e.describe()))
}

/// The groups of a result as (key, value) pairs.
fn pairs(result: &AggregateResult) -> Vec<(Option<Value>, Option<Value>)> {
    result
        .groups
        .iter()
        .map(|g| (g.key.clone(), g.value.clone()))
        .collect()
}

/// The rows of the one group of a top list, as (record id, value).
fn listed(result: &AggregateResult) -> Vec<(u64, Value)> {
    assert_eq!(result.groups.len(), 1, "a top list is one group");
    result.groups[0]
        .rows
        .iter()
        .map(|row| {
            let json: serde_json::Value = serde_json::from_str(row.as_str()).expect("row is JSON");
            (
                json["id"].as_u64().expect("an id"),
                int(json["value"].as_i64().expect("a number")),
            )
        })
        .collect()
}

fn single(result: &AggregateResult) -> Option<Value> {
    assert_eq!(
        result.groups.len(),
        1,
        "an ungrouped aggregate is one group"
    );
    assert_eq!(result.groups[0].key, None);
    result.groups[0].value.clone()
}

fn failed_rule(error: &DomainError) -> String {
    error
        .evidence
        .iter()
        .find(|c| !c.passed)
        .map(|c| c.rule.clone())
        .unwrap_or_default()
}

// Verifies: AGG-010
// Verifies: AGG-011
// Verifies: AGG-012
// Verifies: AGG-013
#[test]
fn each_kind_gives_the_result_worked_out_by_hand() {
    let (world, ids) = six();
    let (n1, n2, n3, n4, n5) = (ids[0].0, ids[1].0, ids[2].0, ids[3].0, ids[4].0);
    // The editor reads all six notes.
    let who = editor();
    let count = ok_as(&world, &who, "count_all");
    assert_eq!(single(&count), Some(int(6)));
    assert_eq!(count.scanned, 6);
    // 5 + 3 + 8 + 8 + 2; the note without a rank adds nothing.
    assert_eq!(single(&ok_as(&world, &who, "sum_rank")), Some(int(26)));
    assert_eq!(single(&ok_as(&world, &who, "min_rank")), Some(int(2)));
    assert_eq!(single(&ok_as(&world, &who, "max_rank")), Some(int(8)));
    // 8 is held by n3 and n4: the lower id first; then 5.
    assert_eq!(
        listed(&ok_as(&world, &who, "top_rank")),
        vec![(n3, int(8)), (n4, int(8)), (n1, int(5))]
    );
    // Least first: 2, then 3.
    assert_eq!(
        listed(&ok_as(&world, &who, "bottom_rank")),
        vec![(n5, int(2)), (n2, int(3))]
    );
    // 10 + 20 + 30 + 40 + 50.
    assert_eq!(single(&ok_as(&world, &who, "sum_score")), Some(int(150)));
}

// Verifies: AGG-005
// Verifies: AGG-014
#[test]
fn a_grouped_aggregate_gives_one_result_per_value_in_ascending_order_and_the_empty_group_last() {
    let (world, _) = six();
    let editor = editor();
    // blue: n3, n4, n6; red: n1, n2; no team: n5.
    assert_eq!(
        pairs(&ok_as(&world, &editor, "count_by_team")),
        vec![
            (Some(text("blue")), Some(int(3))),
            (Some(text("red")), Some(int(2))),
            (None, Some(int(1))),
        ]
    );
    // blue: 8 + 8 (n6 has no rank); red: 5 + 3; no team: 2.
    assert_eq!(
        pairs(&ok_as(&world, &editor, "sum_by_team")),
        vec![
            (Some(text("blue")), Some(int(16))),
            (Some(text("red")), Some(int(8))),
            (None, Some(int(2))),
        ]
    );
    assert_eq!(
        pairs(&ok_as(&world, &editor, "max_by_team")),
        vec![
            (Some(text("blue")), Some(int(8))),
            (Some(text("red")), Some(int(5))),
            (None, Some(int(2))),
        ]
    );
}

// Verifies: AGG-022
#[test]
fn a_sum_is_exact_and_a_sum_that_does_not_fit_is_refused_and_never_wrapped() {
    let rows = |ranks: &[i64]| -> Vec<Row> {
        ranks
            .iter()
            .map(|r| row(Some(*r), true, None, None))
            .collect()
    };
    let sum_of = |ranks: &[i64]| {
        let (world, _) = populate(&base(), &rows(ranks));
        run_as(&world, &owner(), "sum_rank", &[])
    };
    // The total fits although a partial sum in the order the notes were made does not.
    assert_eq!(
        single(&sum_of(&[i64::MAX, 1, -1]).unwrap()),
        Some(int(i64::MAX))
    );
    assert_eq!(
        single(&sum_of(&[i64::MIN, -1, 1]).unwrap()),
        Some(int(i64::MIN))
    );
    let overflowing: [&[i64]; 3] = [&[i64::MAX, 1], &[i64::MAX, i64::MAX], &[i64::MIN, -1]];
    for ranks in overflowing {
        let error = sum_of(ranks).unwrap_err();
        assert_eq!(error.kind, ErrorKind::LimitExceeded, "{ranks:?}");
        assert_eq!(failed_rule(&error), aggregates::SUM_OVERFLOW);
    }
}

// Verifies: AGG-020
// Verifies: AGG-021
// Verifies: AGG-2325
#[test]
fn the_scan_bound_refuses_one_record_past_it_and_counts_only_what_the_reader_may_read() {
    let (world, _) = six();
    // `tight` has a bound of 3. The editor with a filter selects n3, n4 and n6: exactly 3.
    let blue = [("team", text("blue"))];
    let at_the_bound = run_as(&world, &editor(), "tight", &blue).unwrap();
    assert_eq!(single(&at_the_bound), Some(int(3)));
    assert_eq!(at_the_bound.scanned, 3);
    // Published notes are n1, n2, n3 and n6: one past it.
    let published = [("status", text("published"))];
    let error = run_as(&world, &editor(), "tight", &published).unwrap_err();
    assert_eq!(error.kind, ErrorKind::LimitExceeded);
    assert_eq!(failed_rule(&error), aggregates::SCAN_BOUND_EXCEEDED);
    // The public reads four notes, one past the bound too...
    let error = run_as(&world, &Principal::Anonymous, "tight", &[]).unwrap_err();
    assert_eq!(failed_rule(&error), aggregates::SCAN_BOUND_EXCEEDED);
    // ... but of the blue ones only n3 and n6, and the object type holds six notes, more than the
    // bound: what counts against the bound is what the reader may read and the filter selects.
    let reader = run_as(&world, &Principal::Anonymous, "tight", &blue).unwrap();
    assert_eq!(single(&reader), Some(int(2)));
    assert_eq!(reader.scanned, 2);
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
                    (field("rank"), int(i as i64)),
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
        // The records an index answers for one value: the matches and nothing else.
        if let Need::ByField {
            object: kind,
            field: name,
            value,
        } = need
        {
            for record in tenant
                .records_of(kind)
                .filter(|record| match record.get(name) {
                    Value::List(items) => items.contains(value),
                    other => other == value,
                })
            {
                records
                    .entry(kind.clone())
                    .or_default()
                    .insert(record.id, record.clone());
            }
            continue;
        }
        let Need::PageOf {
            object: kind,
            after,
            limit,
        } = need
        else {
            panic!("an aggregate asks for a window, not {need:?}");
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

// Verifies: AGG-2324
#[test]
fn an_object_type_past_the_platform_window_is_refused_and_no_more_than_10001_records_are_loaded() {
    let world = base();
    let schema_of = |state: &State| state.tenant.as_ref().unwrap().schema.clone();
    for (count, outcome_ok) in [(10_000_u64, true), (10_001, false)] {
        let full = crowded(&world, count);
        let ask = request("wide", &[]);
        let needs = aggregates::needs(&schema_of(&full), &ask);
        assert_eq!(
            needs,
            vec![Need::PageOf {
                object: object("note"),
                after: None,
                limit: limits::MAX_SCAN_RECORDS + 1
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
        assert!(loaded <= 10_001, "loaded {loaded}");
        let result = aggregates::run(&state, &ask, &ctx("alpha", Principal::Anonymous, 90_000));
        if outcome_ok {
            let result = result.unwrap();
            assert_eq!(single(&result), Some(int(10_000)));
            assert_eq!(result.scanned, 10_000);
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.kind, ErrorKind::LimitExceeded);
            assert_eq!(failed_rule(&error), aggregates::SCAN_BOUND_EXCEEDED);
        }
    }
    // A request the kernel will refuse needs nothing.
    let schema = schema_of(&world.state(&tenant_id("alpha")));
    assert!(aggregates::needs(&schema, &request("no_such", &[])).is_empty());
}

/// The notes counted by team: `team == $team` is answered by the record index (AGG-2326). Its
/// `count_in_tight` has a bound of 3.
fn by_team_query() -> QueryDef {
    use AggregateKind::*;
    QueryDef {
        id: QueryId::new("by_team").unwrap(),
        object: object("note"),
        filter: Some(eq(var(Scope::Record, "team"), Expr::Param("team".into()))),
        params: BTreeSet::from(["team".to_string()]),
        order: vec![],
        limit: None,
        aggregates: Some(vec![
            aggregate("count_rows", Count, None, None, None, false, 100),
            aggregate(
                "count_by_status",
                Count,
                None,
                Some("status"),
                None,
                false,
                100,
            ),
            aggregate("count_in_tight", Count, None, None, None, false, 3),
        ]),
        declared_fields: Some(vec![field("status"), field("rank")]),
    }
}

/// The notes filtered by score, which has a read rule and so no index (AGG-2326).
fn scored_query() -> QueryDef {
    use AggregateKind::*;
    QueryDef {
        id: QueryId::new("scored").unwrap(),
        object: object("note"),
        filter: Some(eq(var(Scope::Record, "score"), Expr::Param("score".into()))),
        params: BTreeSet::from(["score".to_string()]),
        order: vec![],
        limit: None,
        aggregates: Some(vec![aggregate(
            "count_rows",
            Count,
            None,
            None,
            None,
            false,
            100,
        )]),
        declared_fields: Some(vec![field("status")]),
    }
}

/// Every note, with the averages and the two-field groups of the hand computations.
fn more_query() -> QueryDef {
    use AggregateKind::*;
    QueryDef {
        id: QueryId::new("more").unwrap(),
        object: object("note"),
        filter: None,
        params: BTreeSet::new(),
        order: vec![],
        limit: None,
        aggregates: Some(vec![
            aggregate("avg_rank", Average, Some("rank"), None, None, false, 100),
            aggregate(
                "avg_by_team",
                Average,
                Some("rank"),
                Some("team"),
                None,
                false,
                100,
            ),
            AggregateDef {
                group_by_second: Some(field("status")),
                ..aggregate(
                    "count_by_team_status",
                    Count,
                    None,
                    Some("team"),
                    None,
                    false,
                    100,
                )
            },
        ]),
        declared_fields: Some(vec![field("rank"), field("status"), field("team")]),
    }
}

/// `base()` with the queries of the index tests and of the averages and two-field groups.
fn with_queries() -> World {
    change(
        &base(),
        3_500,
        vec![
            SchemaChange::DefineQuery(by_team_query()),
            SchemaChange::DefineQuery(scored_query()),
            SchemaChange::DefineQuery(more_query()),
        ],
    )
    .expect("the queries are accepted")
}

fn ask(query: &str, aggregate_name: &str, params: &[(&str, Value)]) -> AggregateRequest {
    AggregateRequest {
        query: QueryId::new(query).unwrap(),
        aggregate: agg_id(aggregate_name),
        params: params
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone()))
            .collect(),
        filter: BTreeMap::new(),
    }
}

fn run_on(state: &State, who: &Principal, request: &AggregateRequest) -> Result<AggregateResult, DomainError> {
    aggregates::run(state, request, &ctx("alpha", who.clone(), 90_000))
}

fn run_request(
    world: &World,
    who: &Principal,
    request: &AggregateRequest,
) -> Result<AggregateResult, DomainError> {
    let state = world.state(&tenant_id("alpha"));
    aggregates::run(&state, request, &ctx("alpha", who.clone(), 90_000))
}

/// The groups of a result as (key, second key, value) triples.
fn triples(result: &AggregateResult) -> Vec<(Option<Value>, Option<Value>, Option<Value>)> {
    result
        .groups
        .iter()
        .map(|g| (g.key.clone(), g.key_second.clone(), g.value.clone()))
        .collect()
}

/// As `crowded`, with the first `red` notes in team red and the others in no team.
fn crowded_with_team(world: &World, n: u64, red: u64) -> State {
    let state = crowded(world, n);
    let mut tenant = TenantState::clone(state.tenant.as_ref().unwrap());
    let notes = tenant.records.entry(object("note")).or_default();
    for i in 1..=red {
        let id = RecordId(1_000 + i);
        let mut record = Record::clone(notes.get(&id).expect("a crowded note"));
        record.fields.insert(field("team"), text("red"));
        notes.insert(id, Arc::new(record));
    }
    State {
        tenant_id: state.tenant_id,
        tenant: Some(Arc::new(tenant)),
        coverage: Coverage::Complete,
    }
}

// Verifies: AGG-2326
// Verifies: AGG-2329
// Verifies: AGG-2324
#[test]
fn an_index_answerable_aggregate_reads_only_its_matches_past_the_window_and_a_scanned_one_is_refused(
) {
    let world = with_queries();
    let full = crowded_with_team(&world, 10_001, 30);
    let schema = full.tenant.as_ref().unwrap().schema.clone();

    // The filter `team == red` is answered by the record index: exactly its 30 notes are loaded.
    let red = ask("by_team", "count_rows", &[("team", text("red"))]);
    let needs = aggregates::needs(&schema, &red);
    assert_eq!(
        needs,
        vec![Need::ByField {
            object: object("note"),
            field: field("team"),
            value: text("red"),
        }]
    );
    let state = thin(&full, &needs);
    assert_eq!(
        state
            .tenant
            .as_ref()
            .unwrap()
            .records_of(&object("note"))
            .count(),
        30
    );
    let result = run_on(&state, &Principal::Anonymous, &red).unwrap();
    assert_eq!(single(&result), Some(int(30)));
    assert_eq!(result.scanned, 30);
    let by_status = ask("by_team", "count_by_status", &[("team", text("red"))]);
    assert_eq!(
        pairs(&run_on(&state, &Principal::Anonymous, &by_status).unwrap()),
        vec![(Some(text("published")), Some(int(30)))]
    );
    // A team with no notes is answered from no records at all.
    let blue = ask("by_team", "count_rows", &[("team", text("blue"))]);
    let needs = aggregates::needs(&schema, &blue);
    let state = thin(&full, &needs);
    assert_eq!(
        state
            .tenant
            .as_ref()
            .unwrap()
            .records_of(&object("note"))
            .count(),
        0
    );
    assert_eq!(
        single(&run_on(&state, &Principal::Anonymous, &blue).unwrap()),
        Some(int(0))
    );

    // `scored` filters on a field with a read rule, so no index answers it and the type is scanned:
    // past the window it is refused, as before.
    let scored = ask("scored", "count_rows", &[("score", int(1))]);
    assert_eq!(
        aggregates::needs(&schema, &scored),
        vec![Need::PageOf {
            object: object("note"),
            after: None,
            limit: limits::MAX_SCAN_RECORDS + 1,
        }]
    );
    let error = run_on(&full, &Principal::Anonymous, &scored).unwrap_err();
    assert_eq!(error.kind, ErrorKind::LimitExceeded);
    assert_eq!(failed_rule(&error), aggregates::SCAN_BOUND_EXCEEDED);
}

// Verifies: AGG-2326
// Verifies: AGG-2329
// Verifies: AGG-015
// Verifies: AGG-017
#[test]
fn an_index_answerable_aggregate_counts_only_the_matches_the_reader_may_read() {
    let (world, _) = populate(
        &with_queries(),
        &[
            row(Some(5), true, Some("red"), None),
            row(Some(3), true, Some("red"), None),
            // A draft, which the public may not read, among the red notes.
            row(Some(1), false, Some("red"), None),
            row(Some(8), true, Some("blue"), None),
        ],
    );
    let red = ask("by_team", "count_rows", &[("team", text("red"))]);
    // The public counts the two published red notes; the editor counts all three.
    assert_eq!(
        single(&run_request(&world, &Principal::Anonymous, &red).unwrap()),
        Some(int(2))
    );
    let editor_result = run_request(&world, &editor(), &red).unwrap();
    assert_eq!(single(&editor_result), Some(int(3)));
    assert_eq!(editor_result.scanned, 3);
    // The bound of `count_in_tight` is 3: the editor's three are at it, and one more would not be.
    let tight = ask("by_team", "count_in_tight", &[("team", text("red"))]);
    assert_eq!(
        single(&run_request(&world, &editor(), &tight).unwrap()),
        Some(int(3))
    );
    let (world, _) = populate(&world, &[row(Some(9), true, Some("red"), None)]);
    let error = run_request(&world, &editor(), &tight).unwrap_err();
    assert_eq!(failed_rule(&error), aggregates::SCAN_BOUND_EXCEEDED);
}

// Verifies: AGG-010
// Verifies: AGG-2327
// Verifies: AGG-2328
// Verifies: AGG-016
// Verifies: AGG-001
#[test]
fn an_average_is_the_integer_part_of_the_mean_and_null_without_a_value() {
    let (world, _) = populate(
        &with_queries(),
        &[
            row(Some(5), true, Some("red"), None),
            row(Some(3), true, Some("red"), None),
            row(Some(8), true, Some("blue"), None),
            row(Some(8), false, Some("blue"), None),
            row(Some(2), false, None, None),
            row(None, true, Some("blue"), None),
        ],
    );
    let editor = editor();
    // 5 + 3 + 8 + 8 + 2 = 26 over five values: 5.2, which is 5.
    assert_eq!(
        single(&run_request(&world, &editor, &ask("more", "avg_rank", &[])).unwrap()),
        Some(int(5))
    );
    // blue 16 over two values, red 8 over two, no team 2.
    assert_eq!(
        pairs(&run_request(&world, &editor, &ask("more", "avg_by_team", &[])).unwrap()),
        vec![
            (Some(text("blue")), Some(int(8))),
            (Some(text("red")), Some(int(4))),
            (None, Some(int(2))),
        ]
    );
    // No note of the team has a rank, so the average of the team is null.
    let (world, _) = populate(&world, &[row(None, true, Some("green"), None)]);
    let green = AggregateRequest {
        filter: BTreeMap::from([(field("team"), text("green"))]),
        ..ask("more", "avg_rank", &[])
    };
    assert_eq!(single(&run_request(&world, &editor, &green).unwrap()), None);
}

// Verifies: AGG-016
// Verifies: AGG-2327
#[test]
fn an_average_rounds_toward_zero() {
    let (world, _) = populate(
        &with_queries(),
        &[
            row(Some(-3), true, None, None),
            row(Some(-4), true, None, None),
        ],
    );
    // -7 over two values is -3.5, which is -3.
    assert_eq!(
        single(&run_request(&world, &editor(), &ask("more", "avg_rank", &[])).unwrap()),
        Some(int(-3))
    );
}

// Verifies: AGG-005
// Verifies: AGG-014
#[test]
fn a_group_by_two_fields_gives_one_result_per_pair_in_order_and_the_records_without_a_value_last() {
    let (world, _) = six();
    let world = change(&world, 3_500, vec![SchemaChange::DefineQuery(more_query())])
        .expect("the query is accepted");
    // blue draft: n4; blue published: n3 and n6; red published: n1 and n2; no team, draft: n5.
    let result = run_request(&world, &editor(), &ask("more", "count_by_team_status", &[])).unwrap();
    assert_eq!(
        triples(&result),
        vec![
            (Some(text("blue")), Some(text("draft")), Some(int(1))),
            (Some(text("blue")), Some(text("published")), Some(int(2))),
            (Some(text("red")), Some(text("published")), Some(int(2))),
            (None, Some(text("draft")), Some(int(1))),
        ]
    );
}

// Verifies: AGG-2321
// Verifies: AGG-2322
// Verifies: AGG-005
#[test]
fn a_second_group_field_and_an_average_are_refused_when_they_cannot_mean_anything() {
    use AggregateKind::*;
    let pair = |group: Option<&str>, second: &str| AggregateDef {
        group_by_second: Some(field(second)),
        ..aggregate("pair", Count, None, group, None, false, 100)
    };
    refused(
        define(vec![pair(None, "team")], &["team"]),
        "needs a first one",
    );
    refused(
        define(vec![pair(Some("team"), "team")], &["team"]),
        "both team",
    );
    refused(
        define(
            vec![aggregate("avg_none", Average, None, None, None, false, 100)],
            &["rank"],
        ),
        "names a field",
    );
    refused(
        define(
            vec![aggregate(
                "avg_text",
                Average,
                Some("team"),
                None,
                None,
                false,
                100,
            )],
            &["team"],
        ),
        "not one this aggregate admits",
    );
}

// Verifies: AGG-040
// Verifies: AGG-042
#[test]
fn a_result_names_the_object_types_it_depends_on_and_one_tenant_is_read() {
    let (world, _) = six();
    let result = ok_as(&world, &editor(), "count_all");
    assert_eq!(result.depends_on, BTreeSet::from([object("note")]));
    assert_eq!(result.query, QueryId::new("stats").unwrap());
    assert_eq!(result.aggregate, agg_id("count_all"));
    assert_eq!(result.revision, world.tenants[&tenant_id("alpha")].revision);
    // A snapshot of another tenant answers nothing.
    let state = world.state(&tenant_id("beta"));
    let error = aggregates::run(
        &state,
        &request("count_all", &[]),
        &ctx("alpha", editor(), 90_000),
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::TenantMismatch);
}

fn define(aggs: Vec<AggregateDef>, declared: &[&str]) -> Result<World, DomainError> {
    let mut changes = field_changes();
    changes.push(SchemaChange::DefineQuery(stats_query(aggs, declared)));
    change(&notes_world(), 3_000, changes)
}

fn refused(result: Result<World, DomainError>, mentions: &str) {
    let error = result.expect_err("the definition is refused");
    assert_eq!(error.kind, ErrorKind::InvalidSchema, "{}", error.message);
    assert!(
        error.evidence.iter().any(|c| c.detail.contains(mentions)),
        "no refusal mentions {mentions:?}: {:?}",
        error.evidence
    );
}

// Verifies: AGG-001
// Verifies: AGG-002
// Verifies: AGG-023
// Verifies: AGG-2320
// Verifies: AGG-2321
// Verifies: AGG-2322
#[test]
fn a_query_that_declares_what_an_aggregate_cannot_do_is_refused_when_it_is_defined() {
    use AggregateKind::*;
    let decl = &DECLARED;
    // What is declared well is accepted, at the bounds too.
    assert!(define(
        vec![aggregate("a", Count, None, None, None, false, 10_000)],
        decl
    )
    .is_ok());
    assert!(define(
        vec![aggregate("a", Top, Some("rank"), None, Some(100), true, 1)],
        decl
    )
    .is_ok());
    // An aggregate over a field the query does not declare (AGG-002).
    refused(
        define(
            vec![aggregate("a", Sum, Some("rank"), None, None, false, 10)],
            &["team"],
        ),
        "not a declared field",
    );
    refused(
        define(
            vec![aggregate("a", Count, None, Some("team"), None, false, 10)],
            &["rank"],
        ),
        "not a declared field",
    );
    // A declared field the object type lacks.
    refused(define(vec![], &["no_such_field"]), "declares no_such_field");
    // A bound above the platform maximum, and none (AGG-023).
    refused(
        define(
            vec![aggregate("a", Count, None, None, None, false, 10_001)],
            decl,
        ),
        "scan bound",
    );
    refused(
        define(
            vec![aggregate("a", Count, None, None, None, false, 0)],
            decl,
        ),
        "scan bound",
    );
    // A type the kind does not admit (AGG-2321): a sum of text, an order over a list or a
    // truth value, a group of lists.
    refused(
        define(
            vec![aggregate("a", Sum, Some("team"), None, None, false, 10)],
            decl,
        ),
        "not one this aggregate admits",
    );
    let with_tags: Vec<&str> = DECLARED.iter().copied().chain(["tags"]).collect();
    refused(
        define(
            vec![aggregate("a", Maximum, Some("tags"), None, None, false, 10)],
            &with_tags,
        ),
        "not one this aggregate admits",
    );
    refused(
        define(
            vec![aggregate("a", Count, None, Some("tags"), None, false, 10)],
            &with_tags,
        ),
        "not one this aggregate admits",
    );
    // The arguments each kind has and lacks (AGG-2322).
    refused(
        define(
            vec![aggregate("a", Count, Some("rank"), None, None, false, 10)],
            decl,
        ),
        "names no field",
    );
    refused(
        define(vec![aggregate("a", Sum, None, None, None, false, 10)], decl),
        "names a field",
    );
    refused(
        define(
            vec![aggregate("a", Top, Some("rank"), None, None, true, 10)],
            decl,
        ),
        "a top list is of 1 to 100",
    );
    refused(
        define(
            vec![aggregate("a", Top, Some("rank"), None, Some(0), true, 10)],
            decl,
        ),
        "a top list is of 1 to 100",
    );
    refused(
        define(
            vec![aggregate("a", Top, Some("rank"), None, Some(101), true, 10)],
            decl,
        ),
        "a top list is of 1 to 100",
    );
    refused(
        define(
            vec![aggregate(
                "a",
                Maximum,
                Some("rank"),
                None,
                Some(2),
                true,
                10,
            )],
            decl,
        ),
        "only a top list states a length",
    );
    // Two aggregates of one name.
    refused(
        define(
            vec![
                aggregate("a", Count, None, None, None, false, 10),
                aggregate("a", Count, None, None, None, false, 10),
            ],
            decl,
        ),
        "declared twice",
    );
    // At most 20 aggregates (AGG-2320).
    let many = |n: usize| -> Vec<AggregateDef> {
        (0..n)
            .map(|i| aggregate(&format!("a{i}"), Count, None, None, None, false, 10))
            .collect()
    };
    assert!(define(many(limits::MAX_AGGREGATES_PER_QUERY), decl).is_ok());
    refused(
        define(many(limits::MAX_AGGREGATES_PER_QUERY + 1), decl),
        "more than the maximum of 20",
    );
}

proptest! {
    // A fixed, recorded seed: a run is the same run every time, and a failure reproduces.
    #![proptest_config(ProptestConfig {
        cases: 96,
        rng_seed: proptest::test_runner::RngSeed::Fixed(20261120),
        ..ProptestConfig::default()
    })]

}
