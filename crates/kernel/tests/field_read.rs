//! The read rule of a field (KRN-1589): a field the reader may not read is absent from the answer,
//! on the record read and on the named query read.

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

/// Whether the one row of each read path, as `who`, holds a score: the record read, then the
/// named query read.
fn sees_score(who: &str) -> [bool; 2] {
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
        result.rows[0].fields.contains_key(&field("score"))
    })
}

// Verifies: KRN-1589
#[test]
fn a_field_the_reader_may_not_read_is_absent_from_a_record_read_and_a_query_read() {
    // The writer may read the note, and not its score; the editor may read both.
    assert_eq!(sees_score("alpha:writer1"), [false, false]);
    assert_eq!(sees_score("alpha:editor"), [true, true]);
}
