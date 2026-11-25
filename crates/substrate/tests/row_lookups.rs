#![allow(clippy::err_expect)] // `unwrap_err` would print a whole world when it fails.
//! A view section's row marks and row joins (WG-5599): a mark is the reader's own boolean for each
//! row, a join is a grouped aggregate of another object type keyed by the rows. Both count only the
//! records the reader may read, and both are asked in batches of the rows, never one per row.

use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::*;
use strata_substrate::bundle::{install_into, Bundle};
use strata_substrate::project::{
    argument_values, groups_by_row, lookup_by_rows, match_route, section_request, LOOKUP_BATCH,
};

fn board() -> TenantId {
    TenantId::new("board").unwrap()
}

/// A board of messages and their reactions. Ann and Bob are members; Ann's reaction on the second
/// message is hidden, and so is Bob's hidden reaction on the first, which neither member may read.
fn bundle() -> Bundle {
    let doc = json!({
        "id": "board",
        "name": "Board",
        "owner": "user:admin",
        "created_at": "2026-01-01T00:00:00Z",
        "roles": [ { "id": "member", "label": "Member", "permissions": [] } ],
        "objects": [
            {
                "id": "message", "label": "Message",
                "fields": [
                    { "id": "title", "label": "Title", "type": "string", "required": true }
                ]
            },
            {
                "id": "reaction", "label": "Reaction",
                "fields": [
                    { "id": "message", "label": "Message", "type": "ref:message", "required": true },
                    { "id": "emoji", "label": "Emoji", "type": "string", "required": true },
                    { "id": "reactor", "label": "Reactor", "type": "string", "required": true },
                    { "id": "hidden", "label": "Hidden", "type": "bool", "required": false }
                ]
            }
        ],
        "policies": [
            { "id": "members_read_messages", "role": "member", "on": "message", "actions": ["read"] },
            { "id": "members_read_reactions", "role": "member", "on": "reaction", "actions": ["read"],
              "when": "record.hidden == false" }
        ],
        "queries": [
            { "id": "messages", "object": "message", "limit": 10 },
            {
                "id": "my_reactions", "object": "reaction",
                "where": "record.message in param.ids and record.reactor == principal",
                "params": ["ids"], "declared_fields": ["message"],
                "aggregates": [ { "id": "mine", "kind": "count", "group_by": "message", "scan_bound": 10000 } ]
            },
            {
                "id": "reactions_by_message", "object": "reaction",
                "where": "record.message in param.ids",
                "params": ["ids"], "declared_fields": ["message", "emoji"],
                "aggregates": [ {
                    "id": "per_emoji", "kind": "count", "group_by": "message",
                    "group_by_second": "emoji", "scan_bound": 10000
                } ]
            }
        ],
        "views": [
            {
                "id": "board", "route": "/board", "title": "Board",
                "sections": {
                    "messages": {
                        "query": "messages",
                        "marks": { "mine": { "query": "my_reactions", "aggregate": "mine", "key": "ids" } },
                        "joins": { "reactions": { "query": "reactions_by_message", "aggregate": "per_emoji", "key": "ids" } }
                    }
                }
            }
        ],
        "members": [
            { "principal": "user:ann", "roles": ["member"] },
            { "principal": "user:bob", "roles": ["member"] }
        ],
        "records": [
            { "object": "message", "key": "m1", "as": "user:admin", "at": "2026-01-02", "fields": { "title": "First" } },
            { "object": "message", "key": "m2", "as": "user:admin", "at": "2026-01-02", "fields": { "title": "Second" } },
            { "object": "reaction", "as": "user:admin", "at": "2026-01-03", "fields": { "message": "@m1", "emoji": "👍", "reactor": "user:ann", "hidden": false } },
            { "object": "reaction", "as": "user:admin", "at": "2026-01-03", "fields": { "message": "@m1", "emoji": "👍", "reactor": "user:bob", "hidden": false } },
            { "object": "reaction", "as": "user:admin", "at": "2026-01-03", "fields": { "message": "@m1", "emoji": "🎉", "reactor": "user:ann", "hidden": false } },
            { "object": "reaction", "as": "user:admin", "at": "2026-01-03", "fields": { "message": "@m1", "emoji": "🙈", "reactor": "user:bob", "hidden": true } },
            { "object": "reaction", "as": "user:admin", "at": "2026-01-03", "fields": { "message": "@m2", "emoji": "🙈", "reactor": "user:ann", "hidden": true } }
        ]
    });
    Bundle::from_json(&doc.to_string()).unwrap()
}

fn world() -> World {
    install_into(&World::default(), &bundle()).unwrap_or_else(|e| panic!("{e}"))
}

fn user(name: &str) -> Principal {
    Principal::User(PrincipalId::new(format!("user:{name}")).unwrap())
}

fn ctx(principal: Principal) -> Context {
    Context {
        tenant: board(),
        principal,
        session: None,
        now: Timestamp(1_800_000_000_000),
        correlation: "t".into(),
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
    }
}

/// The rows of the `messages` section of the board, as `reader` reads them.
fn board_rows(world: &World, reader: &Principal) -> Vec<RecordId> {
    let schema = &world.tenants[&board()].schema;
    let (view, route) = match_route(schema, "/board").expect("route matches");
    let section = &view.sections["messages"];
    let request = section_request(section, &route, &BTreeMap::new(), &BTreeMap::new());
    world
        .read(&request, &ctx(reader.clone()))
        .unwrap_or_else(|e| panic!("{}", e.describe()))
        .rows
        .iter()
        .map(|row| row.id)
        .collect()
}

/// The rows a mark holds, as `reader` asks for them: the lookup as the view declares it.
fn marked(world: &World, reader: &Principal, ids: &[RecordId]) -> BTreeSet<RecordId> {
    let schema = &world.tenants[&board()].schema;
    let (view, _) = match_route(schema, "/board").expect("route matches");
    let mark = &view.sections["messages"].marks.as_ref().unwrap()["mine"];
    let state = world.state(&board());
    let context = ctx(reader.clone());
    let params = argument_values(&BTreeMap::new(), &BTreeMap::new(), &BTreeMap::new(), &BTreeMap::new(), &BTreeMap::new(), &Value::Null);
    lookup_by_rows(&mark.query, &mark.aggregate, &mark.key, &params, ids, |request| {
        aggregates::run(&state, request, &context)
    })
    .unwrap_or_else(|e| panic!("{}", e.describe()))
    .into_iter()
    .map(|(id, _)| id)
    .collect()
}

/// The groups of a join per row, as `reader` asks for them, as the emoji and the count of each.
fn joined(world: &World, reader: &Principal, ids: &[RecordId]) -> BTreeMap<RecordId, BTreeMap<String, i64>> {
    let schema = &world.tenants[&board()].schema;
    let (view, _) = match_route(schema, "/board").expect("route matches");
    let join = &view.sections["messages"].joins.as_ref().unwrap()["reactions"];
    let state = world.state(&board());
    let context = ctx(reader.clone());
    let params = BTreeMap::new();
    let found = lookup_by_rows(&join.query, &join.aggregate, &join.key, &params, ids, |request| {
        aggregates::run(&state, request, &context)
    })
    .unwrap_or_else(|e| panic!("{}", e.describe()));
    groups_by_row(found)
        .into_iter()
        .map(|(id, groups)| {
            let by_emoji = groups
                .into_iter()
                .map(|group| {
                    let emoji = group.key.as_ref().and_then(Value::as_str).unwrap_or("").to_string();
                    let count = group.value.as_ref().and_then(|v| match v {
                        Value::Int(n) => Some(*n),
                        _ => None,
                    });
                    (emoji, count.unwrap_or(0))
                })
                .collect();
            (id, by_emoji)
        })
        .collect()
}

/// Verifies: AGG-043
///
/// A mark is the reader's own boolean for each row: true for the rows the reader has a record on
/// that the reader may read. Ann's reaction on the second message is hidden from her, so the second
/// message is not marked for her, and Bob's is marked by his visible reaction on the first.
#[test]
fn a_mark_is_the_readers_own_and_counts_only_what_the_reader_may_read() {
    let world = world();
    let ann = user("ann");
    let rows = board_rows(&world, &ann);
    assert_eq!(rows.len(), 2);
    let (first, second) = (rows[0], rows[1]);
    let ann_marks = marked(&world, &ann, &rows);
    assert_eq!(ann_marks, BTreeSet::from([first]), "ann reacted to the first message only");
    let bob_marks = marked(&world, &user("bob"), &rows);
    assert_eq!(bob_marks, BTreeSet::from([first]), "bob's visible reaction is on the first message");
    assert!(!ann_marks.contains(&second));
}

/// Verifies: AGG-044
///
/// A join is a grouped aggregate keyed by the rows, delivered per row. The reactions hidden from
/// the reader are not counted: the hidden thumbs-see-no-evil of Bob is absent for Ann and for Bob,
/// and Ann's hidden reaction on the second message leaves that message with no groups at all.
#[test]
fn a_join_counts_per_row_only_the_records_the_reader_may_read() {
    let world = world();
    let ann = user("ann");
    let rows = board_rows(&world, &ann);
    let (first, second) = (rows[0], rows[1]);
    let counts = joined(&world, &ann, &rows);
    let on_first = counts.get(&first).cloned().unwrap_or_default();
    assert_eq!(on_first.get("👍"), Some(&2), "two thumbs-up on the first message");
    assert_eq!(on_first.get("🎉"), Some(&1));
    assert!(!on_first.contains_key("🙈"), "a hidden reaction is not counted: {on_first:?}");
    assert!(counts.get(&second).is_none_or(|groups| groups.is_empty()), "a hidden reaction leaves the second message with no groups");
}

/// Verifies: AGG-044
///
/// The rows are looked up in batches of at most sixteen ids, one aggregate per batch, so a page
/// of rows costs one aggregate for every sixteen of them and never one per row.
#[test]
fn rows_are_looked_up_in_batches_of_sixteen() {
    let ids: Vec<RecordId> = (1..=40).map(RecordId).collect();
    let mut sizes = Vec::new();
    let params = BTreeMap::new();
    let query = QueryId::new("reactions_by_message").unwrap();
    let aggregate = AggregateId::new("per_emoji").unwrap();
    let found: Vec<(RecordId, _)> = lookup_by_rows::<()>(&query, &aggregate, "ids", &params, &ids, |request| {
        match request.params.get("ids") {
            Some(Value::List(items)) => sizes.push(items.len()),
            other => panic!("the batch is a list of ids, got {other:?}"),
        }
        Ok(AggregateResult {
            query: query.clone(),
            aggregate: aggregate.clone(),
            groups: Vec::new(),
            scanned: 0,
            depends_on: BTreeSet::new(),
            revision: StateRevision(1),
            schema_version: SchemaVersion(1),
        })
    })
    .unwrap()
    .into_iter()
    .collect();
    assert!(found.is_empty());
    assert_eq!(LOOKUP_BATCH, 16);
    assert_eq!(sizes, vec![16, 16, 8], "forty rows are three batches, each within the in-filter bound");
}

/// Verifies: AGG-043
///
/// A `principal` argument is the reader's own principal string, and null for a reader who has none.
#[test]
fn the_principal_argument_is_the_reader() {
    let args = BTreeMap::from([("me".to_string(), ArgSource::Principal)]);
    let reader = strata_substrate::project::principal_value(&user("ann"));
    let values = argument_values(&args, &BTreeMap::new(), &BTreeMap::new(), &BTreeMap::new(), &BTreeMap::new(), &reader);
    assert_eq!(values.get("me"), Some(&Value::Str("user:ann".into())));
    let anonymous = strata_substrate::project::principal_value(&Principal::Anonymous);
    assert_eq!(anonymous, Value::Null);
}

/// Verifies: AGG-043
///
/// A board's reply carries the row marks and joins, by name, on every row (the substrate reply).
#[test]
fn the_board_declares_its_marks_and_joins_by_name() {
    let world = world();
    let schema = &world.tenants[&board()].schema;
    let (view, _) = match_route(schema, "/board").expect("route matches");
    let section = &view.sections["messages"];
    assert_eq!(section.marks.as_ref().map(|m| m.len()), Some(1));
    assert_eq!(section.joins.as_ref().map(|j| j.len()), Some(1));
    assert_eq!(section.marks.as_ref().unwrap()["mine"].key, "ids");
    assert_eq!(section.joins.as_ref().unwrap()["reactions"].key, "ids");
}
