//! Search through the stores (SRH-150 to SRH-152, SRH-1590, NG-10): the memory store and the SQLite
//! adapter answer a search the same, and as a plain filter over the rows and fields the reader may
//! read: a draft is found by its author and not by the public, a word only a hidden field holds is
//! not a hit for a reader who may not read the field and is for one who may, a deleted record has
//! no entry in the commit that deletes it, a field that is not searchable (and a personal field
//! that is not declared so) is never indexed, a search examines a bounded number of candidates and
//! its cursor resumes exactly, one tenant's index never answers for another, and the index is a
//! function of the records alone: rebuilt, it answers the same. Random data from a fixed seed.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use strata_kernel::search::{
    self, Candidate, Probe, SearchCursor, SearchRequest, SearchResult, MAX_CANDIDATES,
};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, Commit, Store};
use strata_storage_sqlite::{default_tenants_dir, SqliteStore};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn fresh_path(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let path = dir.join(format!(
        "{name}-{}-{}.db",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    let _ = std::fs::remove_dir_all(default_tenants_dir(&path));
    path
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
        .commit(&Commit::of(&intent, &context, &transition))
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

/// Every page of a search, `size` hits at a time, as (id, matched fields).
fn all_pages(
    store: &dyn Store,
    tenant: &str,
    who: &Principal,
    query: &str,
    size: u32,
) -> Vec<(u64, Vec<String>)> {
    let mut out = Vec::new();
    let mut cursor = None;
    for _ in 0..500 {
        let page = search_page(store, tenant, who, &request(query, size, cursor.clone()));
        out.extend(flat(&page));
        cursor = page.next;
        if cursor.is_none() {
            return out;
        }
    }
    panic!("a search did not end");
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

// Verifies: SRH-1590
#[test]
fn a_word_only_a_hidden_field_holds_is_a_hit_for_a_reader_who_may_read_the_field_alone() {
    for (name, store) in stores() {
        found(&*store, "alpha", &SEARCHABLE);
        put(
            &*store,
            "alpha",
            vec![doc(
                1,
                None,
                &[
                    ("title", text("Public title")),
                    ("status", text("published")),
                    ("secret", text("zebrafish")),
                ],
            )],
        );
        for who in [Principal::Anonymous, writer1(), writer2()] {
            assert!(
                search_as(&*store, "alpha", &who, "zebrafish").is_empty(),
                "{name}: {who:?}"
            );
        }
        for who in [editor(), owner("alpha")] {
            assert_eq!(
                search_as(&*store, "alpha", &who, "zebrafish"),
                [(1, vec!["secret".to_string()])],
                "{name}: {who:?}"
            );
        }
        // The index holds the word and says so to whoever asks it: it authorizes nothing. The
        // kernel decides each candidate, which is why the readers above were told nothing.
        let probe = Probe {
            object: object("doc"),
            fields: vec![field("secret")],
            tokens: vec!["zebrafish".into()],
            after: None,
            limit: 10,
        };
        assert_eq!(
            store
                .search_candidates(&tenant_id("alpha"), &probe)
                .unwrap(),
            [Candidate {
                id: RecordId(1),
                fields: vec![field("secret")]
            }],
            "{name}"
        );
    }
}

// Verifies: SRH-152
#[test]
fn deleting_a_record_removes_it_from_results_in_the_commit_that_deletes_it() {
    for (name, store) in stores() {
        found(&*store, "alpha", &SEARCHABLE);
        put(
            &*store,
            "alpha",
            vec![doc(
                1,
                None,
                &[("title", text("Findable")), ("status", text("published"))],
            )],
        );
        assert_eq!(
            ids(&search_as(
                &*store,
                "alpha",
                &Principal::Anonymous,
                "findable"
            )),
            [1],
            "{name}"
        );
        execute(
            &*store,
            &Intent::DeleteRecord {
                object: object("doc"),
                id: RecordId(1),
                expected_version: None,
            },
            &ctx("alpha", editor(), 40),
        )
        .unwrap();
        assert!(
            search_as(&*store, "alpha", &Principal::Anonymous, "findable").is_empty(),
            "{name}"
        );
        let probe = Probe {
            object: object("doc"),
            fields: vec![field("title")],
            tokens: vec!["findable".into()],
            after: None,
            limit: 10,
        };
        assert!(store
            .search_candidates(&tenant_id("alpha"), &probe)
            .unwrap()
            .is_empty());
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

// Verifies: SRH-152
#[test]
fn declaring_a_field_searchable_indexes_the_records_that_exist_and_clearing_it_removes_them() {
    for (name, store) in stores() {
        found(&*store, "alpha", &["title"]);
        put(
            &*store,
            "alpha",
            vec![doc(
                1,
                None,
                &[
                    ("title", text("Plain")),
                    ("body", text("glazing")),
                    ("status", text("published")),
                ],
            )],
        );
        let find = |query: &str| ids(&search_as(&*store, "alpha", &Principal::Anonymous, query));
        assert!(find("glazing").is_empty(), "{name}: body is not declared");
        let change = |change: SchemaChange| {
            execute(
                &*store,
                &Intent::ChangeSchema {
                    changes: vec![change],
                    expected_version: None,
                    existing: ExistingData::Reject,
                },
                &ctx("alpha", owner("alpha"), 50),
            )
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        };
        change(SchemaChange::SetSearchable {
            object: object("doc"),
            fields: vec![field("title"), field("body")],
        });
        assert_eq!(find("glazing"), [1], "{name}");
        change(SchemaChange::ClearSearchable {
            object: object("doc"),
        });
        assert!(find("glazing").is_empty(), "{name}");
        assert!(find("plain").is_empty(), "{name}");
    }
}

// Verifies: NG-10
#[test]
fn a_search_of_one_tenant_returns_no_record_of_another() {
    for (name, store) in stores() {
        found(&*store, "alpha", &SEARCHABLE);
        found(&*store, "beta", &SEARCHABLE);
        put(
            &*store,
            "alpha",
            vec![doc(
                1,
                None,
                &[("title", text("only alpha")), ("status", text("published"))],
            )],
        );
        put(
            &*store,
            "beta",
            vec![doc(
                1,
                None,
                &[("title", text("only beta")), ("status", text("published"))],
            )],
        );
        let anonymous = Principal::Anonymous;
        assert_eq!(
            ids(&search_as(&*store, "alpha", &anonymous, "alpha")),
            [1],
            "{name}"
        );
        assert!(
            search_as(&*store, "alpha", &anonymous, "beta").is_empty(),
            "{name}"
        );
        assert_eq!(
            ids(&search_as(&*store, "beta", &anonymous, "beta")),
            [1],
            "{name}"
        );
        assert!(
            search_as(&*store, "beta", &anonymous, "alpha").is_empty(),
            "{name}"
        );
        // A probe names no tenant but the one it is given.
        let probe = Probe {
            object: object("doc"),
            fields: vec![field("title")],
            tokens: vec!["alpha".into()],
            after: None,
            limit: 10,
        };
        assert!(store
            .search_candidates(&tenant_id("beta"), &probe)
            .unwrap()
            .is_empty());
        assert!(store
            .search_candidates(&tenant_id("nonesuch"), &probe)
            .unwrap()
            .is_empty());
    }
}

#[test]
fn a_search_examines_a_bounded_number_of_candidates_and_its_cursor_resumes_exactly() {
    for (name, store) in stores() {
        found(&*store, "alpha", &SEARCHABLE);
        // 1,100 drafts the public may not read, then three published docs, all matching.
        let mut docs: Vec<Arc<Record>> = (1..=1_100)
            .map(|id| {
                doc(
                    id,
                    None,
                    &[("title", text("bulk draft")), ("status", text("draft"))],
                )
            })
            .collect();
        docs.extend((1_101..=1_103).map(|id| {
            doc(
                id,
                None,
                &[("title", text("bulk shown")), ("status", text("published"))],
            )
        }));
        put(&*store, "alpha", docs);
        let anonymous = Principal::Anonymous;
        let first = search_page(&*store, "alpha", &anonymous, &request("bulk", 10, None));
        assert!(first.hits.is_empty(), "{name}");
        assert_eq!(first.examined, MAX_CANDIDATES, "{name}");
        let cursor = first.next.clone().expect("the bound was reached: a cursor");
        assert_eq!(cursor.id, RecordId(1_000), "{name}");
        let second = search_page(
            &*store,
            "alpha",
            &anonymous,
            &request("bulk", 10, Some(cursor)),
        );
        assert_eq!(ids(&flat(&second)), [1_101, 1_102, 1_103], "{name}");
        assert_eq!(second.examined, 103, "{name}");
        assert!(second.next.is_none(), "{name}");
    }
}

#[test]
fn pages_resume_exactly_after_records_are_added_and_deleted_between_them() {
    for (name, store) in stores() {
        found(&*store, "alpha", &SEARCHABLE);
        put(
            &*store,
            "alpha",
            (1..=12)
                .map(|id| {
                    doc(
                        id,
                        None,
                        &[("title", text("common")), ("status", text("published"))],
                    )
                })
                .collect(),
        );
        let anonymous = Principal::Anonymous;
        let first = search_page(&*store, "alpha", &anonymous, &request("common", 4, None));
        assert_eq!(ids(&flat(&first)), [1, 2, 3, 4], "{name}");
        commit_ops(
            &*store,
            "alpha",
            [4, 5, 6]
                .into_iter()
                .map(|id| DeltaOp::DeleteRecord {
                    object: object("doc"),
                    id: RecordId(id),
                })
                .chain([DeltaOp::PutRecord(doc(
                    13,
                    None,
                    &[("title", text("common")), ("status", text("published"))],
                ))])
                .collect(),
        );
        let second = search_page(
            &*store,
            "alpha",
            &anonymous,
            &request("common", 200, first.next.clone()),
        );
        assert_eq!(ids(&flat(&second)), [7, 8, 9, 10, 11, 12, 13], "{name}");
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
}

const WORDS: [&str; 9] = [
    "alpha", "Beta", "gamma", "DELTA", "kiln", "kilns", "glaze", "Zeta", "über",
];

fn some_words(rng: &mut Lcg) -> Option<String> {
    let count = rng.below(4);
    (count > 0).then(|| {
        (0..count)
            .map(|_| WORDS[rng.below(WORDS.len() as u64) as usize])
            .collect::<Vec<_>>()
            .join(" ")
    })
}

fn random_docs(rng: &mut Lcg, count: u64) -> Vec<Arc<Record>> {
    (1..=count)
        .map(|id| {
            let mut fields: Vec<(&str, Value)> = Vec::new();
            for name in ["title", "body", "secret", "email"] {
                if let Some(words) = some_words(rng) {
                    fields.push((name, text(&words)));
                }
            }
            if let Some(words) = some_words(rng) {
                let items: Vec<&str> = words.split(' ').collect();
                fields.push(("tags", list(&items)));
            }
            fields.push((
                "status",
                text(if rng.below(2) == 0 {
                    "published"
                } else {
                    "draft"
                }),
            ));
            let created_by = match rng.below(3) {
                0 => Some("alpha:writer1"),
                1 => Some("alpha:editor"),
                _ => None,
            };
            doc(id, created_by, &fields)
        })
        .collect()
}

/// What the reader may read of the docs, filtered by the query, as a plain computation over the
/// kernel's own read: the rows the reader may read, each without the fields it may not, and of
/// those the searchable fields in which every word of the query starts a word.
fn expected(store: &dyn Store, who: &Principal, query: &str) -> Vec<(u64, Vec<String>)> {
    expected_in(store, "alpha", who, query)
}

// Verifies: SRH-150
// Verifies: SRH-1590
#[test]
fn every_store_answers_as_a_filter_over_what_the_reader_may_read() {
    let mut rng = Lcg(0x5EA2_C400);
    let docs = random_docs(&mut rng, 70);
    let others = random_docs(&mut rng, 20);
    let all = stores();
    for (_, store) in &all {
        found(&**store, "alpha", &SEARCHABLE);
        found(&**store, "beta", &SEARCHABLE);
        put(&**store, "alpha", docs.clone());
        put(&**store, "beta", others.clone());
    }
    let readers = [
        Principal::Anonymous,
        writer1(),
        writer2(),
        editor(),
        owner("alpha"),
    ];
    let mut compared = 0;
    for _ in 0..40 {
        let count = 1 + rng.below(2);
        let mut words: Vec<&str> = Vec::new();
        for _ in 0..count {
            let word = WORDS[rng.below(WORDS.len() as u64) as usize];
            words.push(if rng.below(3) == 0 {
                &word[..word.len().min(3)]
            } else {
                word
            });
        }
        let query = words.join(" ");
        for who in &readers {
            let want = expected(&*all[0].1, who, &query);
            for (name, store) in &all {
                assert_eq!(
                    search_as(&**store, "alpha", who, &query),
                    want,
                    "{name}: {who:?} searches {query:?}"
                );
                // Pages of seven hits give the same hits in the same order.
                assert_eq!(
                    all_pages(&**store, "alpha", who, &query, 7),
                    want,
                    "{name}: {who:?} pages {query:?}"
                );
            }
            compared += want.len();
        }
    }
    assert!(
        compared > 100,
        "the fixed seed finds something to compare: {compared}"
    );
    // Each answer is of alpha's docs: the ids of beta's twenty are alpha's too, but beta's words
    // were different, and a search of beta is what beta's own docs give.
    for (name, store) in &all {
        let beta = expected_in(&**store, "beta", &editor_of("beta"), "kiln");
        assert_eq!(
            search_as(&**store, "beta", &editor_of("beta"), "kiln"),
            beta,
            "{name}"
        );
    }
}

fn editor_of(tenant: &str) -> Principal {
    user(&format!("{tenant}:editor"))
}

fn expected_in(
    store: &dyn Store,
    tenant: &str,
    who: &Principal,
    query: &str,
) -> Vec<(u64, Vec<String>)> {
    let state = store.load_complete(&tenant_id(tenant)).unwrap();
    let tenant_state = state.tenant.as_deref().unwrap();
    let def = &tenant_state.schema.objects[&object("doc")];
    let indexed = search::indexed_fields(def);
    let wanted = search::tokens_of(query);
    let all: Vec<RecordId> = tenant_state
        .records_of(&object("doc"))
        .map(|r| r.id)
        .collect();
    strata_kernel::read(
        &state,
        &ReadRequest::Records {
            object: object("doc"),
            ids: all,
        },
        &ctx(tenant, who.clone(), 90_000),
    )
    .unwrap()
    .rows
    .iter()
    .filter_map(|row| {
        let fields: Vec<String> = indexed
            .iter()
            .filter(|f| row.fields.contains_key(*f))
            .filter(|f| {
                let tokens = search::field_tokens(&row.fields[*f]);
                wanted
                    .iter()
                    .all(|w| tokens.iter().any(|t| t.starts_with(w.as_str())))
            })
            .map(|f| f.to_string())
            .collect();
        (!fields.is_empty()).then(|| (row.id.0, fields))
    })
    .collect()
}

#[test]
fn the_index_is_rebuilt_from_the_records_alone_and_answers_the_same() {
    let mut rng = Lcg(0x5EA2_B111);
    let path = fresh_path("search-rebuild");
    let queries = ["alpha", "kil", "glaze zeta", "über", "delta beta"];
    let readers = [Principal::Anonymous, writer1(), editor()];
    let answers = |store: &dyn Store| -> Vec<Vec<(u64, Vec<String>)>> {
        readers
            .iter()
            .flat_map(|who| queries.iter().map(move |q| (who, q)))
            .map(|(who, q)| search_as(store, "alpha", who, q))
            .collect()
    };
    let (file, before) = {
        let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
        found(&store, "alpha", &SEARCHABLE);
        put(&store, "alpha", random_docs(&mut rng, 60));
        let before = answers(&store);
        assert!(before.iter().any(|a| !a.is_empty()));
        // Rebuilt on demand, from the records and the schema in the file.
        store.rebuild_search_index(&tenant_id("alpha")).unwrap();
        assert_eq!(answers(&store), before);
        (
            store.tenant_file(&tenant_id("alpha")).unwrap().unwrap(),
            before,
        )
    };
    // A file whose entries are gone, as the migration that adds them leaves a file that has
    // records, is rebuilt when it is opened.
    {
        let conn = rusqlite::Connection::open(&file).unwrap();
        conn.execute("DELETE FROM search_entries", []).unwrap();
        conn.execute(
            "INSERT INTO search_entries_rebuild (pending) VALUES (1)",
            [],
        )
        .unwrap();
    }
    let store = SqliteStore::open(&path, Timestamp(2)).unwrap();
    assert_eq!(answers(&store), before);
    let marked: i64 = rusqlite::Connection::open(&file)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM search_entries_rebuild", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(marked, 0, "the mark is cleared with the rebuild");
    let kept: BTreeSet<u64> = before.iter().flatten().map(|(id, _)| *id).collect();
    assert!(!kept.is_empty());
}
