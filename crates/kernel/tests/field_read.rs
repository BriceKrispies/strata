//! The read rule of a field (KRN-1589): a field the reader may not read is absent from the answer of a
//! record read, a named query read and a search.

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::testkit::*;
use strata_kernel::*;

/// The notes world of alpha with a `score` that only an editor or the owner may read, a named
/// query over notes, and one note that holds a score of 7.
fn world() -> World {
    let readers = Expr::Or(vec![
        Expr::HasRole(role("editor")),
        Expr::HasRole(owner_role()),
    ]);
    let (world, _) = notes_world()
        .step(
            &Intent::ChangeSchema {
                changes: vec![
                    SchemaChange::DefineField {
                        object: object("note"),
                        field: FieldDef {
                            read: Some(readers),
                            ..field_def("score", FieldType::Int, false)
                        },
                    },
                    SchemaChange::DefineQuery(QueryDef {
                        id: QueryId::new("all_notes").unwrap(),
                        object: object("note"),
                        filter: None,
                        params: BTreeSet::new(),
                        order: vec![],
                        limit: None,
                        aggregates: None,
                        declared_fields: None,
                    }),
                ],
                expected_version: None,
                existing: ExistingData::Reject,
            },
            &ctx("alpha", user("alpha:owner"), 3_000),
        )
        .expect("the schema is accepted");
    let (world, _) = world
        .step(
            &Intent::CreateRecord {
                object: object("note"),
                fields: BTreeMap::from([
                    (field("title"), text("T")),
                    (field("slug"), text("s")),
                    (field("score"), Value::Int(7)),
                ]),
            },
            &ctx("alpha", user("alpha:owner"), 5_000),
        )
        .expect("the note is created");
    world
}

/// The one row of a read of the note as `who`, by the record read and by the named query read.
fn rows_read_by(who: &str) -> [std::sync::Arc<Record>; 2] {
    let world = world();
    let state = world.state(&tenant_id("alpha"));
    let id = state
        .tenant
        .as_ref()
        .unwrap()
        .records_of(&object("note"))
        .next()
        .unwrap()
        .id;
    let context = ctx("alpha", user(who), 7_000);
    let by_id = read(
        &state,
        &ReadRequest::Records {
            object: object("note"),
            ids: vec![id],
        },
        &context,
    )
    .unwrap();
    let named = read(
        &state,
        &ReadRequest::Named {
            query: QueryId::new("all_notes").unwrap(),
            params: BTreeMap::new(),
            page_size: None,
            cursor: None,
        },
        &context,
    )
    .unwrap();
    [by_id, named].map(|result| {
        assert_eq!(result.rows.len(), 1, "{who} reads the note");
        result.rows[0].clone()
    })
}

// Verifies: KRN-1589
#[test]
fn a_field_the_reader_may_not_read_is_absent_from_a_record_read() {
    // The writer may read the note, and not its score; the editor and the owner may read both.
    let writer = &rows_read_by("alpha:writer1")[0];
    assert!(!writer.fields.contains_key(&field("score")));
    assert_eq!(writer.get(&field("title")), &text("T"));
    for who in ["alpha:editor", "alpha:owner"] {
        assert_eq!(rows_read_by(who)[0].get(&field("score")), &Value::Int(7));
    }
}

// Verifies: KRN-1589
#[test]
fn a_field_the_reader_may_not_read_is_absent_from_a_query_read() {
    let writer = &rows_read_by("alpha:writer1")[1];
    assert!(!writer.fields.contains_key(&field("score")));
    assert_eq!(writer.get(&field("title")), &text("T"));
    for who in ["alpha:editor", "alpha:owner"] {
        assert_eq!(rows_read_by(who)[1].get(&field("score")), &Value::Int(7));
    }
}

/// The notes world with a `memo` that only an editor or the owner may read, searchable beside the
/// title, on one note whose title is "plain" and whose memo is "hush".
fn search_world() -> World {
    let readers = Expr::Or(vec![
        Expr::HasRole(role("editor")),
        Expr::HasRole(owner_role()),
    ]);
    let (world, _) = notes_world()
        .step(
            &Intent::ChangeSchema {
                changes: vec![
                    SchemaChange::DefineField {
                        object: object("note"),
                        field: FieldDef {
                            read: Some(readers),
                            ..field_def("memo", FieldType::Str, false)
                        },
                    },
                    SchemaChange::SetSearchable {
                        object: object("note"),
                        fields: vec![field("title"), field("memo")],
                    },
                ],
                expected_version: None,
                existing: ExistingData::Reject,
            },
            &ctx("alpha", user("alpha:owner"), 3_000),
        )
        .expect("the schema is accepted");
    let (world, _) = world
        .step(
            &Intent::CreateRecord {
                object: object("note"),
                fields: BTreeMap::from([
                    (field("title"), text("plain")),
                    (field("slug"), text("s")),
                    (field("memo"), text("hush")),
                ]),
            },
            &ctx("alpha", user("alpha:owner"), 5_000),
        )
        .expect("the note is created");
    world
}

/// The records `who` finds when searching the tenant for `query`.
fn found_by(who: &str, query: &str) -> Vec<std::sync::Arc<Record>> {
    let world = search_world();
    let state = world.state(&tenant_id("alpha"));
    let tenant = state.tenant.as_ref().unwrap();
    let context = ctx("alpha", user(who), 7_000);
    let request = search::SearchRequest {
        query: query.to_string(),
        objects: vec![],
        page_size: Some(10),
        cursor: None,
    };
    let plan = search::plan(tenant, &request, &context).unwrap();
    let mut hits = Vec::new();
    for (object, fields) in &plan.objects {
        let probe = search::Probe {
            object: object.clone(),
            fields: fields.clone(),
            tokens: plan.tokens.clone(),
            after: None,
            limit: 100,
        };
        let candidates = search::candidates_in(
            tenant.records_of(object),
            &tenant.schema.objects[object],
            &probe,
        );
        let decided = search::decide(&state, &plan, object, &candidates, 10, &context).unwrap();
        hits.extend(decided.hits.into_iter().map(|hit| hit.record));
    }
    hits
}

// Verifies: KRN-1589
#[test]
fn a_field_the_reader_may_not_read_is_absent_from_a_search_result_and_matches_nothing() {
    // A word only the memo holds finds the note for those who may read the memo, with the memo.
    for who in ["alpha:editor", "alpha:owner"] {
        let hits = found_by(who, "hush");
        assert_eq!(hits.len(), 1, "{who}");
        assert_eq!(hits[0].get(&field("memo")), &text("hush"));
    }
    // For a writer it finds nothing, as if the memo did not exist.
    assert!(found_by("alpha:writer1", "hush").is_empty());
    // A word of the title finds the note for the writer, without the memo.
    let hits = found_by("alpha:writer1", "plain");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].get(&field("title")), &text("plain"));
    assert!(!hits[0].fields.contains_key(&field("memo")));
}
