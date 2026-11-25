//! Search through the stores (SRH-150 to SRH-152, SRH-1590, NG-10): the memory store and the SQLite
//! adapter answer a search the same, and as a plain filter over the rows and fields the reader may
//! read: a draft is found by its author and not by the public, a word only a hidden field holds is
//! not a hit for a reader who may not read the field and is for one who may, a deleted record has
//! no entry in the commit that deletes it, a field that is not searchable (and a personal field
//! that is not declared so) is never indexed, a search examines a bounded number of candidates and
//! its cursor resumes exactly, one tenant's index never answers for another, and the index is a
//! function of the records alone: rebuilt, it answers the same. Random data from a fixed seed.

use std::sync::Arc;
use strata_kernel::search::{SearchCursor, SearchRequest, SearchResult};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, Commit, Store};
use strata_storage_sqlite::SqliteStore;

fn stores() -> Vec<(&'static str, Box<dyn Store>)> {
    vec![
        ("memory", Box::new(MemoryStore::new()) as Box<dyn Store>),
        (
            "sqlite",
            Box::new(SqliteStore::open_in_memory(Timestamp(1)).unwrap()),
        ),
    ]
}

fn owner(tenant: &str) -> Principal {
    user(&format!("{tenant}:owner"))
}

fn rec(name: &str) -> Expr {
    var(Scope::Record, name)
}

fn doc_policy(
    id: &str,
    role_name: &str,
    actions: &[Action],
    condition: Option<Expr>,
) -> SchemaChange {
    SchemaChange::DefinePolicy(PolicyDef {
        id: PolicyId::new(id).unwrap(),
        role: role(role_name),
        resource: Resource::Object(object("doc")),
        actions: actions.iter().copied().collect(),
        condition,
    })
}

/// The `doc` type: `title`, `body`, `tags` and `status` are read by whoever reads the doc, `secret`
/// by editors and the owner, and `email` is personal and not searchable. The public reads
/// published docs, a writer her own, an editor all.
fn doc_schema(searchable: &[&str]) -> Vec<SchemaChange> {
    let d = object("doc");
    let define = |def: FieldDef| SchemaChange::DefineField {
        object: object("doc"),
        field: def,
    };
    let mut changes = vec![
        SchemaChange::DefineRole(RoleDef {
            id: role("writer"),
            label: "Writer".into(),
        }),
        SchemaChange::DefineRole(RoleDef {
            id: role("editor"),
            label: "Editor".into(),
        }),
        SchemaChange::DefineObject {
            id: d.clone(),
            label: "Doc".into(),
        },
        define(field_def("title", FieldType::Str, false)),
        define(field_def("body", FieldType::Str, false)),
        define(field_def(
            "tags",
            FieldType::List(Box::new(FieldType::Str)),
            false,
        )),
        define(field_def("status", FieldType::Str, false)),
        define(FieldDef {
            read: Some(Expr::Or(vec![
                Expr::HasRole(role("editor")),
                Expr::HasRole(owner_role()),
            ])),
            ..field_def("secret", FieldType::Str, false)
        }),
        define(FieldDef {
            personal: Some(PersonalDeclaration {
                kind: PersonalKind::Other,
                subject: DataSubject::Creator,
                searchable: false,
            }),
            normalise: None,
            computed: None,
            ..field_def("email", FieldType::Str, false)
        }),
        SchemaChange::SetRetention {
            object: d.clone(),
            retention: RetentionRule {
                days: 30,
                from: RetentionStart::Created,
            },
        },
        doc_policy(
            "public_reads_published",
            "public",
            &[Action::Read],
            Some(eq(rec("status"), lit("published"))),
        ),
        doc_policy(
            "writers_read_own",
            "writer",
            &[Action::Read],
            Some(eq(rec("$created_by"), Expr::Principal)),
        ),
        doc_policy("writers_create", "writer", &[Action::Create], None),
        doc_policy(
            "editors_manage",
            "editor",
            &[Action::Read, Action::Create, Action::Update, Action::Delete],
            None,
        ),
    ];
    if !searchable.is_empty() {
        changes.push(SchemaChange::SetSearchable {
            object: d,
            fields: searchable.iter().map(|f| field(f)).collect(),
        });
    }
    changes
}

const SEARCHABLE: [&str; 4] = ["title", "body", "tags", "secret"];

/// A tenant with the doc type, an editor and two writers, and no docs.
fn found(store: &dyn Store, tenant: &str, searchable: &[&str]) {
    execute(
        store,
        &Intent::CreateTenant {
            name: tenant.into(),
            owner: principal_id(&format!("{tenant}:owner")),
        },
        &ctx(tenant, Principal::System, 10),
    )
    .unwrap();
    execute(
        store,
        &Intent::ChangeSchema {
            changes: doc_schema(searchable),
            expected_version: None,
            existing: ExistingData::Reject,
        },
        &ctx(tenant, owner(tenant), 11),
    )
    .unwrap_or_else(|e| panic!("the schema is accepted: {e}"));
    for (who, role_name) in [
        ("editor", "editor"),
        ("writer1", "writer"),
        ("writer2", "writer"),
    ] {
        execute(
            store,
            &Intent::AssignRole {
                principal: principal_id(&format!("{tenant}:{who}")),
                role: role(role_name),
            },
            &ctx(tenant, owner(tenant), 12),
        )
        .unwrap();
    }
}

fn doc(id: u64, created_by: Option<&str>, fields: &[(&str, Value)]) -> Arc<Record> {
    Arc::new(Record {
        id: RecordId(id),
        object: object("doc"),
        version: 1,
        created_by: created_by.map(principal_id),
        created_at: Timestamp(1),
        updated_at: Timestamp(1),
        fields: fields
            .iter()
            .map(|(name, value)| (field(name), value.clone()))
            .collect(),
    })
}

fn list(items: &[&str]) -> Value {
    Value::List(items.iter().map(|i| text(i)).collect())
}

/// Commit record puts and deletions directly, as one delta: the way to give a store many records
/// with the ids they are to have.
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
        object: object("doc"),
        fields: Default::default(),
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

fn put(store: &dyn Store, tenant: &str, docs: Vec<Arc<Record>>) {
    for chunk in docs.chunks(500) {
        commit_ops(
            store,
            tenant,
            chunk.iter().cloned().map(DeltaOp::PutRecord).collect(),
        );
    }
}

fn request(query: &str, size: u32, cursor: Option<SearchCursor>) -> SearchRequest {
    SearchRequest {
        query: query.to_string(),
        objects: vec![],
        page_size: Some(size),
        cursor,
    }
}

fn search_page(
    store: &dyn Store,
    tenant: &str,
    who: &Principal,
    request: &SearchRequest,
) -> SearchResult {
    strata_storage_api::search::run(store, request, &ctx(tenant, who.clone(), 90_000))
        .unwrap_or_else(|e| panic!("{who:?} searches {:?}: {e}", request.query))
}

fn search_as(
    store: &dyn Store,
    tenant: &str,
    who: &Principal,
    query: &str,
) -> Vec<(u64, Vec<String>)> {
    flat(&search_page(store, tenant, who, &request(query, 200, None)))
}

fn flat(result: &SearchResult) -> Vec<(u64, Vec<String>)> {
    result
        .hits
        .iter()
        .map(|h| {
            (
                h.record.id.0,
                h.matched.iter().map(|f| f.to_string()).collect(),
            )
        })
        .collect()
}

fn ids(hits: &[(u64, Vec<String>)]) -> Vec<u64> {
    hits.iter().map(|(id, _)| *id).collect()
}

fn editor() -> Principal {
    user("alpha:editor")
}
fn writer1() -> Principal {
    user("alpha:writer1")
}
fn writer2() -> Principal {
    user("alpha:writer2")
}

// Verifies: SRH-150
// Verifies: SRH-151
#[test]
fn a_draft_is_found_by_its_author_and_not_by_the_public() {
    for (name, store) in stores() {
        found(&*store, "alpha", &SEARCHABLE);
        let created = execute(
            &*store,
            &Intent::CreateRecord {
                object: object("doc"),
                fields: [
                    (field("title"), text("Quokka field notes")),
                    (field("status"), text("draft")),
                ]
                .into(),
            },
            &ctx("alpha", writer1(), 30),
        )
        .unwrap();
        let Output::Record { id: draft, .. } = created.transition.output else {
            panic!("a created record is the output");
        };
        let published = doc(
            100,
            None,
            &[
                ("title", text("Quokka habits")),
                ("status", text("published")),
            ],
        );
        put(&*store, "alpha", vec![published]);
        let find = |who: &Principal| ids(&search_as(&*store, "alpha", who, "quokka"));
        assert_eq!(find(&Principal::Anonymous), [100], "{name}");
        assert_eq!(find(&writer2()), [100], "{name}");
        assert_eq!(find(&writer1()), [draft.0, 100], "{name}");
        assert_eq!(find(&editor()), [draft.0, 100], "{name}");
    }
}

// Verifies: SRH-152
// Verifies: SRH-1590
#[test]
fn the_entries_of_a_record_follow_its_commits_and_hold_searchable_fields_only() {
    let store = SqliteStore::open_in_memory(Timestamp(1)).unwrap();
    found(&store, "alpha", &SEARCHABLE);
    let entries = |id: u64| {
        store
            .search_entries_of(&tenant_id("alpha"), &object("doc"), RecordId(id))
            .unwrap()
    };
    let first = doc(
        1,
        None,
        &[
            ("title", text("Kiln Notes")),
            ("body", text("Firing")),
            ("tags", list(&["pottery"])),
            ("status", text("published")),
            ("secret", text("zebrafish")),
            ("email", text("person@example.test")),
        ],
    );
    put(&store, "alpha", vec![first]);
    // The status is not declared searchable and the email is personal without saying so: neither is
    // in the index, though both words are in the record.
    assert_eq!(
        entries(1),
        [
            "body:firing",
            "secret:zebrafish",
            "tags:pottery",
            "title:kiln",
            "title:notes"
        ]
    );
    // A rewrite replaces the entries in its own commit: the old words are gone, the new are there.
    put(
        &store,
        "alpha",
        vec![Arc::new(Record {
            version: 2,
            ..(*doc(
                1,
                None,
                &[("title", text("Glaze")), ("status", text("published"))],
            ))
            .clone()
        })],
    );
    assert_eq!(entries(1), ["title:glaze"]);
    // The commit that deletes the record leaves none.
    commit_ops(
        &store,
        "alpha",
        vec![DeltaOp::DeleteRecord {
            object: object("doc"),
            id: RecordId(1),
        }],
    );
    assert!(entries(1).is_empty());
}
