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
fn writer() -> Principal {
    user("alpha:writer1")
}
fn stranger() -> Principal {
    user("alpha:stranger")
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

/// Add the notes, one intent each: created as the owner and, when published, published by the
/// editor, as the workflow of the notes requires. Returns the ids in order.
fn populate(world: &World, rows: &[Row]) -> (World, Vec<RecordId>) {
    let mut world = world.clone();
    let mut ids = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let now = 4_000 + 2 * i as i64;
        let mut fields = BTreeMap::from([
            (field("title"), text(&format!("N{i}"))),
            (field("slug"), text(&format!("s{i}"))),
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
    for who in [editor(), owner()] {
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
    // The same request, asked twice, gives the same answer.
    assert_eq!(
        ok_as(&world, &editor, "count_by_team"),
        ok_as(&world, &editor, "count_by_team")
    );
}

// Verifies: AGG-012
#[test]
fn an_aggregate_over_no_record_is_zero_or_null_and_a_grouped_one_has_no_group() {
    let (world, _) = six();
    let nothing = [("team", text("green"))];
    let run = |name: &str| run_as(&world, &editor(), name, &nothing).unwrap();
    assert_eq!(single(&run("count_all")), Some(int(0)));
    assert_eq!(single(&run("sum_rank")), Some(int(0)));
    assert_eq!(single(&run("min_rank")), None);
    assert_eq!(single(&run("max_rank")), None);
    assert_eq!(listed(&run("top_rank")), vec![]);
    assert!(run("count_by_team").groups.is_empty());
    assert_eq!(run("count_all").scanned, 0);
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
    // Nothing is answered in part: the refusal is the whole answer.
    assert!(run_as(&world, &editor(), "tight", &[]).is_err());
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

// Verifies: AGG-015
// Verifies: AGG-016
#[test]
fn two_readers_with_different_visibility_get_different_results() {
    let (world, ids) = six();
    let (n1, n2, n3) = (ids[0].0, ids[1].0, ids[2].0);
    let public = Principal::Anonymous;
    let editor = editor();
    // A count never includes a note the reader may not read.
    let seen = ok_as(&world, &public, "count_all");
    assert_eq!(single(&seen), Some(int(4)));
    assert_eq!(seen.scanned, 4);
    assert_eq!(single(&ok_as(&world, &editor, "count_all")), Some(int(6)));
    // Nor a group: n5, the only note without a team, is a draft.
    assert_eq!(
        pairs(&ok_as(&world, &public, "count_by_team")),
        vec![
            (Some(text("blue")), Some(int(2))),
            (Some(text("red")), Some(int(2))),
        ]
    );
    // Nor a sum, a minimum or a maximum: the drafts hold ranks 8 and 2.
    assert_eq!(single(&ok_as(&world, &public, "sum_rank")), Some(int(16)));
    assert_eq!(single(&ok_as(&world, &public, "min_rank")), Some(int(3)));
    assert_eq!(single(&ok_as(&world, &editor, "min_rank")), Some(int(2)));
    assert_eq!(single(&ok_as(&world, &public, "max_rank")), Some(int(8)));
    // Nor a place in a top list: n4 is not there, so n1 takes its place.
    assert_eq!(
        listed(&ok_as(&world, &public, "top_rank")),
        vec![(n3, int(8)), (n1, int(5)), (n2, int(3))]
    );
    // A writer reads every note but cannot read the score; the stranger and the public read the
    // published ones only: three readers, three answers to the same request.
    assert_eq!(single(&ok_as(&world, &writer(), "count_all")), Some(int(6)));
    assert_eq!(
        single(&ok_as(&world, &stranger(), "count_all")),
        Some(int(4))
    );
    // The answers do not depend on who asked before.
    let again = ok_as(&world, &editor, "count_all");
    let _ = ok_as(&world, &public, "count_all");
    assert_eq!(ok_as(&world, &editor, "count_all"), again);
}

// Verifies: AGG-017
#[test]
fn a_note_is_left_out_when_the_reader_may_not_read_a_field_the_aggregate_names() {
    let (world, _) = six();
    for who in [writer(), stranger(), Principal::Anonymous] {
        // The score is the editors'. Every note is left out for the others, the one without a
        // score too: no figure tells them which notes hold one.
        let sum = ok_as(&world, &who, "sum_score");
        assert_eq!(single(&sum), Some(int(0)), "{who:?}");
        assert_eq!(sum.scanned, 0);
        assert!(ok_as(&world, &who, "count_by_score").groups.is_empty());
        // A filter on the field is a field the aggregate names.
        let filtered = run_as(&world, &who, "count_all", &[("score", int(10))]).unwrap();
        assert_eq!(single(&filtered), Some(int(0)));
    }
    let editor = editor();
    // For the editor the notes are there, those without a score in a group of their own.
    let by_score = ok_as(&world, &editor, "count_by_score");
    assert_eq!(
        pairs(&by_score),
        vec![
            (Some(int(10)), Some(int(1))),
            (Some(int(20)), Some(int(1))),
            (Some(int(30)), Some(int(1))),
            (Some(int(40)), Some(int(1))),
            (Some(int(50)), Some(int(1))),
            (None, Some(int(1))),
        ]
    );
    let filtered = run_as(&world, &editor, "count_all", &[("score", int(10))]).unwrap();
    assert_eq!(single(&filtered), Some(int(1)));
}

// Verifies: AGG-003
// Verifies: AGG-004
#[test]
fn a_request_for_something_the_query_does_not_declare_is_refused() {
    let (world, _) = six();
    let kind_of = |result: Result<AggregateResult, DomainError>| result.unwrap_err().kind;
    assert_eq!(
        kind_of(run_as(&world, &editor(), "no_such_aggregate", &[])),
        ErrorKind::UnknownQuery
    );
    let state = world.state(&tenant_id("alpha"));
    let mut ask = request("count_all", &[]);
    ask.query = QueryId::new("plain").unwrap();
    let context = ctx("alpha", editor(), 90_000);
    // `plain` declares no aggregate at all.
    assert_eq!(
        aggregates::run(&state, &ask, &context).unwrap_err().kind,
        ErrorKind::UnknownQuery
    );
    ask.query = QueryId::new("no_such_query").unwrap();
    assert_eq!(
        aggregates::run(&state, &ask, &context).unwrap_err().kind,
        ErrorKind::UnknownQuery
    );
    // `body` is a field of the notes that `stats` does not declare.
    assert_eq!(
        kind_of(run_as(
            &world,
            &editor(),
            "count_all",
            &[("body", text("x"))]
        )),
        ErrorKind::InvalidIntent
    );
    let mut ask = request("count_all", &[]);
    ask.params.insert("slug".into(), text("x"));
    assert_eq!(
        aggregates::run(&state, &ask, &context).unwrap_err().kind,
        ErrorKind::InvalidIntent
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
    let error = result.err().expect("the definition is refused");
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
    assert!(define(all_aggregates(), decl).is_ok());
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

// Verifies: AGG-002
#[test]
fn removing_a_field_an_aggregate_names_is_refused() {
    let world = base();
    let result = change(
        &world,
        5_000,
        vec![SchemaChange::RemoveField {
            object: object("note"),
            field: field("team"),
        }],
    );
    refused(result, "team");
}

// Verifies: AGG-041
#[test]
fn a_view_section_may_name_an_aggregate_its_query_declares_and_no_other() {
    let section = |query: &str, aggregate_name: &str, single: bool| ViewSection {
        query: QueryId::new(query).unwrap(),
        args: BTreeMap::new(),
        include: vec![],
        markdown: vec![],
        single,
        aggregate: Some(agg_id(aggregate_name)),
    };
    let view = |section: ViewSection| {
        SchemaChange::DefineView(ViewDef {
            id: ViewId::new("scores").unwrap(),
            route: "/scores".into(),
            title: "Scores".into(),
            sections: BTreeMap::from([("counts".to_string(), section)]),
        })
    };
    let world = base();
    assert!(change(
        &world,
        5_000,
        vec![view(section("stats", "count_all", false))]
    )
    .is_ok());
    refused(
        change(
            &world,
            5_000,
            vec![view(section("stats", "no_such", false))],
        ),
        "declares no aggregate no_such",
    );
    refused(
        change(
            &world,
            5_000,
            vec![view(section("plain", "count_all", false))],
        ),
        "declares no aggregate count_all",
    );
    refused(
        change(
            &world,
            5_000,
            vec![view(section("stats", "count_all", true))],
        ),
        "delivers no rows",
    );
}

/// The oracle: the same aggregate computed one record at a time over the rows a reader's plain
/// query returned. The reader's field-level rights are given, not derived: the score is the
/// editors' and the owner's.
fn one_at_a_time(
    rows: &[Arc<Record>],
    agg: &AggregateDef,
    may_read_score: bool,
) -> Vec<(Option<Value>, Option<Value>, Vec<(u64, Value)>)> {
    let names = |name: &str| {
        [agg.field.as_ref(), agg.group_by.as_ref()]
            .into_iter()
            .flatten()
            .any(|f| f.as_str() == name)
    };
    let usable: &[Arc<Record>] = if names("score") && !may_read_score {
        &[]
    } else {
        rows
    };
    #[derive(Default)]
    struct Acc {
        count: i64,
        sum: i128,
        low: Option<Value>,
        high: Option<Value>,
        ranked: Vec<(Value, u64)>,
    }
    let mut groups: BTreeMap<Option<Value>, Acc> = BTreeMap::new();
    if agg.group_by.is_none() {
        groups.insert(None, Acc::default());
    }
    let present = |record: &Record, f: &FieldId| {
        let value = record.get(f);
        (!matches!(value, Value::Null)).then(|| value.clone())
    };
    for record in usable {
        let key = agg.group_by.as_ref().and_then(|f| present(record, f));
        let acc = groups.entry(key).or_default();
        acc.count += 1;
        if let Some(value) = agg.field.as_ref().and_then(|f| present(record, f)) {
            if let Value::Int(n) = &value {
                acc.sum += i128::from(*n);
            }
            if acc.low.as_ref().is_none_or(|low| value < *low) {
                acc.low = Some(value.clone());
            }
            if acc.high.as_ref().is_none_or(|high| value > *high) {
                acc.high = Some(value.clone());
            }
            acc.ranked.push((value, record.id.0));
        }
    }
    let mut ordered: Vec<(Option<Value>, Acc)> = groups.into_iter().collect();
    // The groups of a value ascending, the group of no value last.
    ordered.sort_by(|(a, _), (b, _)| a.is_none().cmp(&b.is_none()).then(a.cmp(b)));
    ordered
        .into_iter()
        .map(|(key, mut acc)| match agg.kind {
            AggregateKind::Count => (key, Some(int(acc.count)), vec![]),
            AggregateKind::Sum => (
                key,
                Some(int(i64::try_from(acc.sum).expect("the sum fits"))),
                vec![],
            ),
            AggregateKind::Minimum => (key, acc.low, vec![]),
            AggregateKind::Maximum => (key, acc.high, vec![]),
            AggregateKind::Top => {
                if agg.descending {
                    acc.ranked
                        .sort_by(|(va, ia), (vb, ib)| vb.cmp(va).then(ia.cmp(ib)));
                } else {
                    acc.ranked
                        .sort_by(|(va, ia), (vb, ib)| va.cmp(vb).then(ia.cmp(ib)));
                }
                let top = acc
                    .ranked
                    .into_iter()
                    .take(agg.n.unwrap() as usize)
                    .map(|(value, id)| (id, value))
                    .collect();
                (key, None, top)
            }
        })
        .collect()
}

fn row_strategy() -> impl Strategy<Value = Row> {
    (
        proptest::option::of(-3i64..12),
        any::<bool>(),
        proptest::option::of(0usize..3),
        proptest::option::of(-5i64..40),
    )
        .prop_map(|(rank, published, team, score)| Row {
            rank,
            published,
            team: team.map(|t| ["t0", "t1", "t2"][t]),
            score,
        })
}

proptest! {
    // A fixed, recorded seed: a run is the same run every time, and a failure reproduces.
    #![proptest_config(ProptestConfig {
        cases: 96,
        rng_seed: proptest::test_runner::RngSeed::Fixed(20261120),
        ..ProptestConfig::default()
    })]

    /// Every aggregate equals the same computation done one record at a time over the rows the
    /// plain query returns to that reader, whoever the reader is.
    /// Verifies: AGG-015
    /// Verifies: AGG-016
    #[test]
    fn every_aggregate_equals_the_same_computation_over_the_rows_the_query_returns(
        rows in proptest::collection::vec(row_strategy(), 0..12),
        who in 0usize..6,
    ) {
        let (world, _) = populate(&base(), &rows);
        let reader = [owner(), editor(), writer(), user("alpha:writer2"), stranger(), Principal::Anonymous][who].clone();
        let may_read_score = who < 2;
        let context = ctx("alpha", reader.clone(), 90_000);
        let plain = ReadRequest::Named {
            query: QueryId::new("plain").unwrap(),
            params: BTreeMap::new(),
            page_size: Some(200),
            cursor: None,
        };
        let seen = world.read(&plain, &context).unwrap().rows;
        for agg in all_aggregates().iter().filter(|a| a.scan_bound >= 100) {
            let got = run_as(&world, &reader, agg.id.as_str(), &[]).unwrap();
            let expected = one_at_a_time(&seen, agg, may_read_score);
            let got_groups: Vec<_> = got
                .groups
                .iter()
                .map(|g| {
                    let listed = g.rows.iter().map(|r| {
                        let json: serde_json::Value = serde_json::from_str(r.as_str()).unwrap();
                        (json["id"].as_u64().unwrap(), int(json["value"].as_i64().unwrap()))
                    }).collect::<Vec<_>>();
                    (g.key.clone(), g.value.clone(), listed)
                })
                .collect();
            prop_assert_eq!(&got_groups, &expected, "{} as {:?}", agg.id, reader);
            // The records examined are the rows the reader could count.
            let counted = if matches!(agg.field.as_ref().map(|f| f.as_str()), Some("score"))
                || matches!(agg.group_by.as_ref().map(|f| f.as_str()), Some("score"))
            {
                if may_read_score { seen.len() } else { 0 }
            } else {
                seen.len()
            };
            prop_assert_eq!(got.scanned as usize, counted);
            // The same request twice is the same answer.
            prop_assert_eq!(&got, &run_as(&world, &reader, agg.id.as_str(), &[]).unwrap());
        }
    }
}
