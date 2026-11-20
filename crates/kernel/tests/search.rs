//! Search (SRH-150 to SRH-152, SRH-1590, D-6585, D-6586): how text becomes tokens, what a record
//! type may declare searchable, what the index holds of a record, and what a reader finds: a draft
//! is found by whoever may read it and by no one else, a word that only a field the reader may not
//! read holds is not a hit for that reader and is for one who may read it, a hit carries the
//! readable fields that matched and no others, and pages resume exactly. The last test compares
//! the kernel's answer with a plain filter over the rows and fields each reader may read, over
//! random records from a fixed seed.
//!
//! The world is the notes world of the test kit with an `article` type: `title`, `body`, `tags` and
//! `status` anyone who may read the article reads, `secret` that editors and the owner read, `mine`
//! that its creator, editors and the owner read, and two personal fields, `email` (not searchable)
//! and `note` (searchable). The public reads published articles, a writer her own, an editor all.

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::search::{
    self, candidates_in, entries_of, indexed_fields, tokens_of, Candidate, Probe, SearchCursor,
    SearchRequest, SearchResult, Source,
};
use strata_kernel::testkit::*;
use strata_kernel::*;

fn owner() -> Principal {
    user("alpha:owner")
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

fn rec(name: &str) -> Expr {
    var(Scope::Record, name)
}

fn policy(id: &str, role_name: &str, actions: &[Action], condition: Option<Expr>) -> SchemaChange {
    SchemaChange::DefinePolicy(PolicyDef {
        id: PolicyId::new(id).unwrap(),
        role: role(role_name),
        resource: Resource::Object(object("article")),
        actions: actions.iter().copied().collect(),
        condition,
    })
}

fn personal(searchable: bool) -> Option<PersonalDeclaration> {
    Some(PersonalDeclaration {
        kind: PersonalKind::Other,
        subject: DataSubject::Creator,
        searchable,
    })
}

const SEARCHABLE: [&str; 6] = ["title", "body", "tags", "secret", "mine", "note"];

fn article_schema(searchable: &[&str]) -> Vec<SchemaChange> {
    let staff = || vec![Expr::HasRole(role("editor")), Expr::HasRole(owner_role())];
    let mut mine = staff();
    mine.push(eq(rec("$created_by"), Expr::Principal));
    let a = object("article");
    let define = |def: FieldDef| SchemaChange::DefineField {
        object: object("article"),
        field: def,
    };
    let mut changes = vec![
        SchemaChange::DefineObject {
            id: a.clone(),
            label: "Article".into(),
        },
        define(field_def("title", FieldType::Str, false)),
        define(field_def("body", FieldType::Str, false)),
        define(field_def(
            "tags",
            FieldType::List(Box::new(FieldType::Str)),
            false,
        )),
        define(field_def("status", FieldType::Str, false)),
        define(field_def("count", FieldType::Int, false)),
        define(FieldDef {
            read: Some(Expr::Or(staff())),
            ..field_def("secret", FieldType::Str, false)
        }),
        define(FieldDef {
            read: Some(Expr::Or(mine)),
            ..field_def("mine", FieldType::Str, false)
        }),
        define(FieldDef {
            personal: personal(false),
            ..field_def("email", FieldType::Str, false)
        }),
        define(FieldDef {
            personal: personal(true),
            ..field_def("note", FieldType::Str, false)
        }),
        SchemaChange::SetRetention {
            object: a.clone(),
            retention: RetentionRule {
                days: 30,
                from: RetentionStart::Created,
            },
        },
        policy(
            "public_reads_published",
            "public",
            &[Action::Read],
            Some(eq(rec("status"), lit("published"))),
        ),
        policy(
            "writers_read_own",
            "writer",
            &[Action::Read],
            Some(eq(rec("$created_by"), Expr::Principal)),
        ),
        policy("writers_create", "writer", &[Action::Create], None),
        policy(
            "editors_manage",
            "editor",
            &[Action::Read, Action::Create, Action::Update, Action::Delete],
            None,
        ),
    ];
    if !searchable.is_empty() {
        changes.push(SchemaChange::SetSearchable {
            object: a,
            fields: searchable.iter().map(|f| field(f)).collect(),
        });
    }
    changes
}

fn base() -> World {
    notes_world()
        .step(
            &Intent::ChangeSchema {
                changes: article_schema(&SEARCHABLE),
                expected_version: None,
                existing: ExistingData::Reject,
            },
            &ctx("alpha", owner(), 3_000),
        )
        .unwrap_or_else(|e| panic!("the schema is accepted: {}", e.describe()))
        .0
}

fn list(items: &[&str]) -> Value {
    Value::List(items.iter().map(|i| text(i)).collect())
}

fn create(world: &World, who: &Principal, now: i64, fields: &[(&str, Value)]) -> (World, RecordId) {
    let (next, done) = world
        .step(
            &Intent::CreateRecord {
                object: object("article"),
                fields: fields.iter().map(|(f, v)| (field(f), v.clone())).collect(),
            },
            &ctx("alpha", who.clone(), now),
        )
        .unwrap_or_else(|e| panic!("the article is created: {}", e.describe()));
    let Output::Record { id, .. } = done.output else {
        panic!("a created record is the output");
    };
    (next, id)
}

fn delete(world: &World, now: i64, id: RecordId) -> World {
    world
        .step(
            &Intent::DeleteRecord {
                object: object("article"),
                id,
                expected_version: None,
            },
            &ctx("alpha", editor(), now),
        )
        .expect("the article is deleted")
        .0
}

/// The candidates of a probe and the state of a world, as a store gives them: nothing is checked
/// but what the probe says.
struct WorldSource<'a> {
    world: &'a World,
}

impl Source for WorldSource<'_> {
    type Error = DomainError;

    fn domain(&self, error: DomainError) -> DomainError {
        error
    }

    fn candidates(&mut self, probe: &Probe) -> Result<Vec<Candidate>, DomainError> {
        let state = self.world.state(&tenant_id("alpha"));
        let tenant = state.tenant.as_deref().unwrap();
        let def = tenant.schema.objects.get(&probe.object).unwrap();
        Ok(candidates_in(tenant.records_of(&probe.object), def, probe))
    }

    fn load(&mut self, _needs: &[Need]) -> Result<State, DomainError> {
        Ok(self.world.state(&tenant_id("alpha")))
    }
}

fn request(query: &str) -> SearchRequest {
    SearchRequest {
        query: query.to_string(),
        objects: vec![],
        page_size: Some(200),
        cursor: None,
    }
}

fn search_as(world: &World, who: &Principal, request: &SearchRequest) -> SearchResult {
    search::run(
        &mut WorldSource { world },
        request,
        &ctx("alpha", who.clone(), 90_000),
    )
    .unwrap_or_else(|e| panic!("{who:?} searches {:?}: {}", request.query, e.describe()))
}

fn ids(result: &SearchResult) -> Vec<u64> {
    result.hits.iter().map(|h| h.record.id.0).collect()
}

fn matched(result: &SearchResult) -> Vec<Vec<String>> {
    result
        .hits
        .iter()
        .map(|h| h.matched.iter().map(|f| f.to_string()).collect())
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------------------------

#[test]
fn text_is_split_at_what_is_not_alphanumeric_and_lowercased() {
    assert_eq!(
        tokens_of("Hello, WORLD — Ünï-code 42x hello"),
        ["hello", "world", "ünï", "code", "42x"]
    );
    assert!(tokens_of("  ... --- ").is_empty());
    // A lowercase mapping, not case folding, and no normalisation: the stated rule.
    assert_ne!(tokens_of("Straße"), tokens_of("STRASSE"));
}

#[test]
fn a_token_is_cut_and_a_field_holds_a_bounded_number_of_them() {
    let long = "x".repeat(200);
    assert_eq!(tokens_of(&long), ["x".repeat(search::MAX_TOKEN_CHARS)]);
    let many: String = (0..1_500).map(|n| format!("w{n} ")).collect();
    let tokens = tokens_of(&many);
    assert_eq!(tokens.len(), search::MAX_FIELD_TOKENS);
    assert_eq!(tokens[0], "w0", "the first in reading order are kept");
}

#[test]
fn the_items_of_a_list_are_separate_pieces() {
    assert_eq!(
        search::field_tokens(&list(&["ab", "cd ef"])),
        ["ab", "cd", "ef"]
    );
    assert!(search::field_tokens(&Value::Int(5)).is_empty());
}

// ---------------------------------------------------------------------------------------------
// The declaration
// ---------------------------------------------------------------------------------------------

fn schema_with(searchable: &[&str]) -> TenantSchema {
    let mut schema = TenantSchema::clone(&base().tenants[&tenant_id("alpha")].schema);
    schema
        .objects
        .get_mut(&object("article"))
        .unwrap()
        .searchable = Some(searchable.iter().map(|f| field(f)).collect());
    schema
}

fn problems_of(searchable: &[&str]) -> Vec<String> {
    schema_with(searchable).problems()
}

#[test]
fn a_declaration_of_text_fields_is_accepted() {
    assert_eq!(problems_of(&SEARCHABLE), Vec::<String>::new());
    let def = &schema_with(&SEARCHABLE).objects[&object("article")];
    assert_eq!(
        indexed_fields(def),
        ["body", "mine", "note", "secret", "tags", "title"]
            .iter()
            .map(|f| field(f))
            .collect::<BTreeSet<_>>()
    );
}

#[test]
fn a_declaration_that_names_a_field_that_is_not_searchable_text_is_refused() {
    let found = |names: &[&str], what: &str| {
        let problems = problems_of(names);
        assert!(
            problems.iter().any(|p| p.contains(what)),
            "{names:?} should be refused for {what}: {problems:?}"
        );
    };
    found(&["nonesuch"], "is not a field");
    found(&["count"], "is not text");
    found(&["title", "title"], "named twice");
    found(&["email"], "personal and its declaration is not searchable");
    let many: Vec<String> = (0..=search::MAX_SEARCHABLE_FIELDS)
        .map(|n| format!("f{n}"))
        .collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    found(&many, "more than the maximum");
}

#[test]
fn a_change_that_declares_an_unknown_field_is_refused_when_it_is_made() {
    let refused = base().step(
        &Intent::ChangeSchema {
            changes: vec![SchemaChange::SetSearchable {
                object: object("article"),
                fields: vec![field("nonesuch")],
            }],
            expected_version: None,
            existing: ExistingData::Reject,
        },
        &ctx("alpha", owner(), 4_000),
    );
    assert_eq!(refused.err().unwrap().kind, ErrorKind::InvalidSchema);
}

#[test]
fn clearing_a_declaration_that_is_not_there_is_refused_and_setting_none_is_too() {
    let world = notes_world();
    let clear = |world: &World, change: SchemaChange| {
        world.step(
            &Intent::ChangeSchema {
                changes: vec![change],
                expected_version: None,
                existing: ExistingData::Reject,
            },
            &ctx("alpha", owner(), 4_000),
        )
    };
    let with_type = world
        .step(
            &Intent::ChangeSchema {
                changes: article_schema(&[]),
                expected_version: None,
                existing: ExistingData::Reject,
            },
            &ctx("alpha", owner(), 3_000),
        )
        .unwrap()
        .0;
    assert!(clear(
        &with_type,
        SchemaChange::ClearSearchable {
            object: object("article")
        }
    )
    .is_err());
    assert!(clear(
        &with_type,
        SchemaChange::SetSearchable {
            object: object("article"),
            fields: vec![]
        }
    )
    .is_err());
    let declared = clear(
        &with_type,
        SchemaChange::SetSearchable {
            object: object("article"),
            fields: vec![field("title")],
        },
    )
    .unwrap()
    .0;
    let cleared = clear(
        &declared,
        SchemaChange::ClearSearchable {
            object: object("article"),
        },
    )
    .unwrap()
    .0;
    // A cleared declaration leaves the schema as it was without one: it hashes the same.
    assert_eq!(
        canon::canonical_json(
            &cleared.tenants[&tenant_id("alpha")].schema.objects[&object("article")]
        ),
        canon::canonical_json(
            &with_type.tenants[&tenant_id("alpha")].schema.objects[&object("article")]
        ),
    );
}

// ---------------------------------------------------------------------------------------------
// What the index holds
// ---------------------------------------------------------------------------------------------

// Verifies: SRH-152
#[test]
fn a_record_has_entries_for_its_searchable_fields_and_for_no_other() {
    let world = base();
    let (world, id) = create(
        &world,
        &editor(),
        4_000,
        &[
            ("title", text("Kiln Notes")),
            ("body", text("Firing the kiln")),
            ("tags", list(&["pottery", "heat"])),
            ("status", text("published")),
            ("email", text("quokka@example.test")),
            ("note", text("remember the glaze")),
            ("count", Value::Int(7)),
        ],
    );
    let state = world.state(&tenant_id("alpha"));
    let tenant = state.tenant.as_deref().unwrap();
    let record = tenant.record(&object("article"), id).unwrap();
    let def = &tenant.schema.objects[&object("article")];
    let entries: BTreeMap<String, Vec<String>> = entries_of(def, record)
        .into_iter()
        .map(|(f, t)| (f.to_string(), t))
        .collect();
    assert_eq!(
        entries.keys().cloned().collect::<Vec<_>>(),
        ["body", "note", "tags", "title"],
        "the status and the count are not searchable, the email is personal and not searchable"
    );
    assert_eq!(entries["title"], ["kiln", "notes"]);
    assert_eq!(entries["tags"], ["pottery", "heat"]);
}

#[test]
fn a_personal_field_that_is_listed_but_not_declared_searchable_is_never_indexed() {
    // The schema is made without the checks of a change, to hold a listing that is refused when
    // made: the index follows the personal declaration whatever the listing says.
    let world = base();
    let mut schema = TenantSchema::clone(&world.tenants[&tenant_id("alpha")].schema);
    schema
        .objects
        .get_mut(&object("article"))
        .unwrap()
        .searchable = Some(vec![field("title"), field("email")]);
    let def = &schema.objects[&object("article")];
    assert_eq!(indexed_fields(def), BTreeSet::from([field("title")]));
}

// ---------------------------------------------------------------------------------------------
// What a reader finds
// ---------------------------------------------------------------------------------------------

// Verifies: SRH-150
// Verifies: SRH-151
#[test]
fn a_draft_is_found_by_whoever_may_read_it_and_by_no_one_else() {
    let (world, draft) = create(
        &base(),
        &writer1(),
        4_000,
        &[
            ("title", text("Quokka field notes")),
            ("status", text("draft")),
        ],
    );
    let (world, published) = create(
        &world,
        &editor(),
        4_001,
        &[
            ("title", text("Quokka habits")),
            ("status", text("published")),
        ],
    );
    let anonymous = search_as(&world, &Principal::Anonymous, &request("quokka"));
    assert_eq!(ids(&anonymous), [published.0]);
    assert_eq!(
        ids(&search_as(&world, &writer2(), &request("quokka"))),
        [published.0]
    );
    for who in [writer1(), editor(), owner()] {
        assert_eq!(
            ids(&search_as(&world, &who, &request("quokka"))),
            [draft.0, published.0],
            "{who:?}"
        );
    }
}

// Verifies: SRH-1590
#[test]
fn a_word_only_a_field_the_reader_may_not_read_holds_is_not_a_hit_for_that_reader() {
    let (world, id) = create(
        &base(),
        &writer1(),
        4_000,
        &[
            ("title", text("Public title")),
            ("status", text("published")),
            ("secret", text("zebrafish")),
            ("mine", text("platypus")),
        ],
    );
    // `secret` is read by editors and the owner: a rule that looks at the reader alone, so the
    // field is not even probed for anyone else. `mine` is read by its creator, editors and the
    // owner: a rule that looks at the record, decided when the record is.
    for word in ["zebrafish", "platypus"] {
        for who in [Principal::Anonymous, writer2()] {
            assert!(
                search_as(&world, &who, &request(word)).hits.is_empty(),
                "{who:?} finds {word}"
            );
        }
    }
    assert!(search_as(&world, &writer1(), &request("zebrafish"))
        .hits
        .is_empty());
    for who in [editor(), owner()] {
        let found = search_as(&world, &who, &request("zebrafish"));
        assert_eq!(ids(&found), [id.0]);
        assert_eq!(matched(&found), [["secret"]]);
    }
    for who in [writer1(), editor(), owner()] {
        let found = search_as(&world, &who, &request("platypus"));
        assert_eq!(ids(&found), [id.0], "{who:?}");
        assert_eq!(matched(&found), [["mine"]]);
    }
}

// Verifies: SRH-1590
#[test]
fn a_hit_carries_the_fields_the_reader_may_read_and_matches_in_them_alone() {
    let (world, id) = create(
        &base(),
        &editor(),
        4_000,
        &[
            ("title", text("Shared word")),
            ("status", text("published")),
            ("secret", text("shared secret")),
        ],
    );
    let public = search_as(&world, &Principal::Anonymous, &request("shared"));
    assert_eq!(ids(&public), [id.0]);
    assert_eq!(matched(&public), [["title"]]);
    assert!(public.hits[0].record.fields.contains_key(&field("title")));
    assert!(!public.hits[0].record.fields.contains_key(&field("secret")));
    let staff = search_as(&world, &editor(), &request("shared"));
    assert_eq!(matched(&staff), [["secret", "title"]]);
    assert!(staff.hits[0].record.fields.contains_key(&field("secret")));
}

// Verifies: SRH-1590
#[test]
fn the_words_of_a_query_never_combine_across_fields() {
    // Each word is readable alone, but a record whose second word is in a hidden field would be
    // found because of it, so a query matches within one field only.
    let (world, _) = create(
        &base(),
        &editor(),
        4_000,
        &[
            ("title", text("alpha")),
            ("status", text("published")),
            ("secret", text("beta")),
        ],
    );
    for who in [Principal::Anonymous, editor()] {
        assert!(search_as(&world, &who, &request("alpha beta"))
            .hits
            .is_empty());
    }
}

#[test]
fn a_query_word_matches_the_start_of_a_word_and_in_any_case() {
    let (world, id) = create(
        &base(),
        &editor(),
        4_000,
        &[
            ("title", text("Kilns and Firing")),
            ("status", text("published")),
        ],
    );
    for query in ["kil", "KILN", "kilns fir", "  firing  "] {
        assert_eq!(
            ids(&search_as(&world, &Principal::Anonymous, &request(query))),
            [id.0],
            "{query}"
        );
    }
    for query in ["ilns", "kilnss", "kiln glaze"] {
        assert!(
            search_as(&world, &Principal::Anonymous, &request(query))
                .hits
                .is_empty(),
            "{query}"
        );
    }
}

#[test]
fn a_deleted_record_is_not_found_and_an_edited_one_is_found_by_its_new_words() {
    let (world, id) = create(
        &base(),
        &editor(),
        4_000,
        &[("title", text("Old words")), ("status", text("published"))],
    );
    let world = world
        .step(
            &Intent::UpdateRecord {
                object: object("article"),
                id,
                fields: BTreeMap::from([(field("title"), text("New words"))]),
                expected_version: None,
            },
            &ctx("alpha", editor(), 4_001),
        )
        .unwrap()
        .0;
    assert!(search_as(&world, &Principal::Anonymous, &request("old"))
        .hits
        .is_empty());
    assert_eq!(
        ids(&search_as(&world, &Principal::Anonymous, &request("new"))),
        [id.0]
    );
    let world = delete(&world, 4_002, id);
    assert!(search_as(&world, &Principal::Anonymous, &request("new"))
        .hits
        .is_empty());
}

#[test]
fn a_search_of_one_tenant_finds_nothing_of_another() {
    let (world, _) = create(
        &base(),
        &editor(),
        4_000,
        &[
            ("title", text("Only in alpha")),
            ("status", text("published")),
        ],
    );
    // Beta has no article type: nothing there is searchable.
    let found = search::run(
        &mut BetaSource { world: &world },
        &request("alpha"),
        &ctx("beta", Principal::Anonymous, 90_000),
    )
    .unwrap();
    assert!(found.hits.is_empty());
    // A snapshot of another tenant is refused.
    let error = search::run(
        &mut WorldSource { world: &world },
        &request("alpha"),
        &ctx("beta", Principal::Anonymous, 90_000),
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::TenantMismatch);
}

struct BetaSource<'a> {
    world: &'a World,
}

impl Source for BetaSource<'_> {
    type Error = DomainError;
    fn domain(&self, error: DomainError) -> DomainError {
        error
    }
    fn candidates(&mut self, probe: &Probe) -> Result<Vec<Candidate>, DomainError> {
        let state = self.world.state(&tenant_id("beta"));
        let tenant = state.tenant.as_deref().unwrap();
        Ok(match tenant.schema.objects.get(&probe.object) {
            Some(def) => candidates_in(tenant.records_of(&probe.object), def, probe),
            None => Vec::new(),
        })
    }
    fn load(&mut self, _needs: &[Need]) -> Result<State, DomainError> {
        Ok(self.world.state(&tenant_id("beta")))
    }
}

#[test]
fn a_request_that_is_not_a_query_is_refused() {
    let world = base();
    let refused = |request: SearchRequest, kind: ErrorKind| {
        let error = search::run(
            &mut WorldSource { world: &world },
            &request,
            &ctx("alpha", Principal::Anonymous, 90_000),
        )
        .unwrap_err();
        assert_eq!(error.kind, kind, "{request:?}");
    };
    refused(request(""), ErrorKind::InvalidIntent);
    refused(request(" ... "), ErrorKind::InvalidIntent);
    refused(request("a b c d e f g h i"), ErrorKind::InvalidIntent);
    refused(request(&"x".repeat(201)), ErrorKind::InvalidIntent);
    refused(
        SearchRequest {
            page_size: Some(0),
            ..request("kiln")
        },
        ErrorKind::InvalidIntent,
    );
    refused(
        SearchRequest {
            page_size: Some(201),
            ..request("kiln")
        },
        ErrorKind::InvalidIntent,
    );
    refused(
        SearchRequest {
            objects: vec![object("nonesuch")],
            ..request("kiln")
        },
        ErrorKind::UnknownObject,
    );
}

#[test]
fn a_cursor_is_text_that_names_a_place_and_nothing_else() {
    let cursor = SearchCursor {
        object: object("article"),
        id: RecordId(42),
    };
    assert_eq!(SearchCursor::decode(&cursor.encode()), Some(cursor));
    for text in [
        "",
        "42",
        "~article",
        "x~article",
        "42~",
        "-1~article",
        "99999999999999999999999~a",
    ] {
        assert_eq!(SearchCursor::decode(text), None, "{text:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------------------------

/// Every page of a search, a page of `size` hits at a time; the world a page is decided on comes
/// from `world_at`, which is given the number of pages already read.
fn pages(
    size: u32,
    query: &str,
    who: &Principal,
    world_at: &dyn Fn(usize) -> World,
) -> Vec<Vec<u64>> {
    let mut out = Vec::new();
    let mut cursor = None;
    loop {
        let world = world_at(out.len());
        let result = search_as(
            &world,
            who,
            &SearchRequest {
                query: query.to_string(),
                objects: vec![],
                page_size: Some(size),
                cursor: cursor.clone(),
            },
        );
        out.push(ids(&result));
        assert!(result.hits.len() <= size as usize);
        cursor = result.next;
        if cursor.is_none() || out.len() > 100 {
            return out;
        }
    }
}

#[test]
fn pages_follow_each_other_with_no_gap_and_no_repeat() {
    let mut world = base();
    let mut all = Vec::new();
    for n in 0..23 {
        let (next, id) = create(
            &world,
            &editor(),
            4_000 + n,
            &[
                ("title", text(&format!("common word {n}"))),
                (
                    "status",
                    text(if n % 3 == 0 { "draft" } else { "published" }),
                ),
            ],
        );
        world = next;
        all.push((id.0, n % 3 != 0));
    }
    let published: Vec<u64> = all.iter().filter(|(_, p)| *p).map(|(id, _)| *id).collect();
    let fixed = |_: usize| world.clone();
    for size in [1, 4, 7, 200] {
        let read: Vec<Vec<u64>> = pages(size, "common", &Principal::Anonymous, &fixed);
        let flat: Vec<u64> = read.iter().flatten().copied().collect();
        assert_eq!(flat, published, "page size {size}");
    }
    // The editor reads the drafts too, and a page of the editor's is what it was said to be.
    let every: Vec<u64> = all.iter().map(|(id, _)| *id).collect();
    let flat: Vec<u64> = pages(5, "common", &editor(), &fixed)
        .into_iter()
        .flatten()
        .collect();
    assert_eq!(flat, every);
}

// Verifies: SRH-152
#[test]
fn a_cursor_resumes_after_records_are_added_and_deleted_between_pages() {
    let mut world = base();
    let mut ids_made = Vec::new();
    for n in 0..12 {
        let (next, id) = create(
            &world,
            &editor(),
            4_000 + n,
            &[("title", text("common")), ("status", text("published"))],
        );
        world = next;
        ids_made.push(id);
    }
    let first = search_as(
        &world,
        &Principal::Anonymous,
        &SearchRequest {
            page_size: Some(4),
            ..request("common")
        },
    );
    assert_eq!(
        ids(&first),
        ids_made[..4].iter().map(|i| i.0).collect::<Vec<_>>()
    );
    // Between the pages: the last record of the first page and the next two are deleted, and a
    // record is added.
    let mut world = delete(&world, 5_000, ids_made[3]);
    world = delete(&world, 5_001, ids_made[4]);
    world = delete(&world, 5_002, ids_made[5]);
    let (world, added) = create(
        &world,
        &editor(),
        5_003,
        &[("title", text("common")), ("status", text("published"))],
    );
    let second = search_as(
        &world,
        &Principal::Anonymous,
        &SearchRequest {
            page_size: Some(200),
            cursor: first.next.clone(),
            ..request("common")
        },
    );
    let expected: Vec<u64> = ids_made[6..].iter().map(|i| i.0).chain([added.0]).collect();
    assert_eq!(ids(&second), expected);
    assert!(second.next.is_none());
}

// ---------------------------------------------------------------------------------------------
// Search equals a filter over what the reader may read
// ---------------------------------------------------------------------------------------------

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

/// A text of up to three words from the pool, or none.
fn some_words(rng: &mut Lcg) -> Option<String> {
    let count = rng.below(4);
    (count > 0).then(|| {
        (0..count)
            .map(|_| WORDS[rng.below(WORDS.len() as u64) as usize])
            .collect::<Vec<_>>()
            .join(" ")
    })
}

// Verifies: SRH-150
// Verifies: SRH-1590
#[test]
fn search_is_a_filter_over_the_rows_and_fields_the_reader_may_read() {
    let mut rng = Lcg(0x5EA7_C4ED);
    let mut world = base();
    let creators = [editor(), writer1(), writer2()];
    let mut made = Vec::new();
    for n in 0..45 {
        let who = &creators[rng.below(3) as usize];
        let mut fields: Vec<(&str, Value)> = Vec::new();
        for name in ["title", "body", "secret", "mine", "note", "email"] {
            if let Some(words) = some_words(&mut rng) {
                fields.push((name, text(&words)));
            }
        }
        if let Some(words) = some_words(&mut rng) {
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
        let (next, id) = create(&world, who, 4_000 + n, &fields);
        world = next;
        made.push(id);
    }

    let readers = [
        Principal::Anonymous,
        writer1(),
        writer2(),
        editor(),
        owner(),
    ];
    let state = world.state(&tenant_id("alpha"));
    let def = state.tenant.as_deref().unwrap().schema.objects[&object("article")].clone();
    let indexed = indexed_fields(&def);
    let mut some_hits = 0;
    for round in 0..60 {
        let count = 1 + rng.below(2);
        let mut query_words: Vec<&str> = Vec::new();
        for _ in 0..count {
            let word = WORDS[rng.below(WORDS.len() as u64) as usize];
            // Some words are typed in part.
            query_words.push(if rng.below(3) == 0 {
                &word[..word.len().min(3)]
            } else {
                word
            });
        }
        let query = query_words.join(" ");
        let wanted = tokens_of(&query);
        for who in &readers {
            let c = ctx("alpha", who.clone(), 90_000 + round);
            // What the reader may read, by the kernel's own read: the records it may read, each
            // without the fields it may not.
            let read = strata_kernel::read(
                &state,
                &ReadRequest::Records {
                    object: object("article"),
                    ids: made.clone(),
                },
                &c,
            )
            .unwrap();
            let expected: Vec<(u64, Vec<String>)> = read
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
                .collect();
            let found = search_as(&world, who, &request(&query));
            let got: Vec<(u64, Vec<String>)> = found
                .hits
                .iter()
                .map(|h| {
                    (
                        h.record.id.0,
                        h.matched.iter().map(|f| f.to_string()).collect(),
                    )
                })
                .collect();
            assert_eq!(got, expected, "{who:?} searches {query:?}");
            // A hit's record is the record as the reader reads it.
            for hit in &found.hits {
                let row = read.rows.iter().find(|r| r.id == hit.record.id).unwrap();
                assert_eq!(hit.record.fields, row.fields, "{who:?} {query:?}");
            }
            some_hits += got.len();
        }
    }
    assert!(
        some_hits > 100,
        "the fixed seed finds something to compare: {some_hits}"
    );
}
