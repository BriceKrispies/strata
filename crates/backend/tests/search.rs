//! Search over the backend's internal route (contract search, CT-175): a draft is found by its
//! author and by an editor and not by the public, a word that only a field the reader may not read
//! holds finds nothing for that reader and the record for one who may, a hit carries the fields
//! the reader may read and no others, pages resume exactly, and one tenant's words are not found in
//! another.
//!
//! The tenant is the seeded `fieldnotes` blog. Its posts declare title, excerpt and body
//! searchable (the publication template); the public reads published posts only, an author reads
//! her own drafts, an editor reads every post, and an author's login identity (`principal`) is read
//! by that author and by editors only. The tests make `name` and `principal` of an author
//! searchable, to have a field the reader's rights decide.

use serde_json::{json, Value as Json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strata_backend::app::Config;
use strata_kernel::{
    Context, ExistingData, FieldId, Intent, ObjectTypeId, Principal, PrincipalId, SchemaChange,
    TenantId, Timestamp,
};
use strata_protocol::api::{HEADER_CORRELATION, HEADER_SERVICE, HEADER_SESSION, HEADER_SITE};
use strata_storage_api::execute;
use strata_storage_api::memory::MemoryStore;

const TOKEN: &str = "test-service-token-0123456789abcdef";
const SITE: &str = "fieldnotes";

struct Backend {
    base: String,
    http: reqwest::Client,
}

async fn start_on(store: Arc<MemoryStore>) -> Backend {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cfg = Config {
        addr,
        service_token: TOKEN.into(),
        dev: true,
        tenants_dir: root.join("tenants"),
        lean_dir: root.join("lean"),
        session_ttl_ms: 3_600_000,
        session_idle_ms: 3_600_000,
        throttle: Default::default(),
        passkey_challenge_ttl_ms: 300_000,
        link_ttl_ms: 900_000,
        mail: Arc::new(strata_backend::mail::DevSink::memory()),
        federation: Arc::new(strata_backend::signin::oidc::Federation::none()),
        factors: Default::default(),
        limits: Default::default(),
    };
    tokio::spawn(strata_backend::run(
        cfg,
        store,
        listener,
        std::future::pending(),
    ));
    let backend = Backend {
        base: format!("http://{addr}"),
        http: reqwest::Client::new(),
    };
    for _ in 0..200 {
        if backend
            .http
            .get(format!("{}/internal/health", backend.base))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            return backend;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("backend did not start");
}

impl Backend {
    async fn call(&self, site: &str, session: Option<&str>, path: &str, body: Json) -> (u16, Json) {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header(HEADER_SERVICE, TOKEN)
            .header(HEADER_SITE, site)
            .header(HEADER_CORRELATION, "t-search-1")
            .json(&body);
        if let Some(token) = session {
            request = request.header(HEADER_SESSION, token);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Json::Null))
    }

    async fn login(&self, login: &str) -> String {
        let (status, body) = self
            .call(
                SITE,
                None,
                "/internal/session/login",
                json!({ "login": login, "password": format!("strata-dev-{login}") }),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        body["token"].as_str().unwrap().to_string()
    }

    /// A search of `site` as the holder of `session`, or as the public.
    async fn search_in(&self, site: &str, session: Option<&str>, request: Json) -> (u16, Json) {
        self.call(site, session, "/internal/search", request).await
    }

    /// The hits of a search of the blog that is accepted.
    async fn hits(&self, session: Option<&str>, words: &str) -> Vec<Json> {
        let (status, body) = self
            .search_in(SITE, session, json!({ "query": words }))
            .await;
        assert_eq!(status, 200, "{body}");
        body["hits"].as_array().unwrap().clone()
    }
}

/// Make `name` and `principal` of an author searchable, as the owner of the tenant does.
fn declare_author_search(store: &MemoryStore) {
    let intent = Intent::ChangeSchema {
        changes: vec![SchemaChange::SetSearchable {
            object: ObjectTypeId::new("author").unwrap(),
            fields: vec![
                FieldId::new("name").unwrap(),
                FieldId::new("principal").unwrap(),
            ],
        }],
        expected_version: None,
        existing: ExistingData::Reject,
    };
    let ctx = Context {
        tenant: TenantId::new(SITE).unwrap(),
        principal: Principal::User(PrincipalId::new("user:admin").unwrap()),
        session: None,
        now: Timestamp(2_000_000_000_000),
        correlation: "t-search".into(),
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
    };
    execute(store, &intent, &ctx).unwrap_or_else(|e| panic!("refused: {e}"));
}

async fn started() -> (Backend, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::new());
    let backend = start_on(store.clone()).await;
    (backend, store)
}

fn objects_of(hits: &[Json]) -> Vec<&str> {
    hits.iter().map(|h| h["object"].as_str().unwrap()).collect()
}

// Verifies: SRH-150
// Verifies: SRH-151
#[tokio::test]
async fn a_draft_is_found_by_its_author_and_an_editor_and_not_by_the_public() {
    let (backend, _store) = started().await;
    let author = backend.login("tomasz").await;
    let editor = backend.login("ines").await;
    let other = backend.login("mirela").await;

    // "deadman" is in the body of Tomasz's draft and nowhere else.
    assert!(backend.hits(None, "deadman").await.is_empty());
    assert!(backend.hits(Some(&other), "deadman").await.is_empty());
    for session in [&author, &editor] {
        let found = backend.hits(Some(session), "deadman").await;
        assert_eq!(objects_of(&found), ["post"], "{found:?}");
        assert_eq!(found[0]["matched"], json!(["body"]));
        assert_eq!(found[0]["fields"]["title"], "Notes Toward a Workbench");
    }

    // A published post is found by everyone, by a word in its title typed in part.
    for session in [None, Some(author.as_str()), Some(editor.as_str())] {
        let found = backend.hits(session, "kil").await;
        assert!(
            found
                .iter()
                .any(|h| h["fields"]["title"] == "What the Kiln Decides"),
            "{found:?}"
        );
    }
}

// Verifies: SRH-1590
#[tokio::test]
async fn a_word_only_a_hidden_field_holds_is_not_a_hit_for_a_reader_who_may_not_read_it() {
    let (backend, store) = started().await;
    declare_author_search(&store);
    let editor = backend.login("ines").await;
    let tomasz = backend.login("tomasz").await;
    let mirela = backend.login("mirela").await;

    // "ines" is the login of one author and nothing in any name: only `principal` holds it, a
    // field that author and editors read. The author record itself is readable by everyone.
    assert!(backend.hits(None, "ines").await.is_empty());
    assert!(backend.hits(Some(&tomasz), "ines").await.is_empty());
    let found = backend.hits(Some(&editor), "ines").await;
    assert_eq!(objects_of(&found), ["author"], "{found:?}");
    assert_eq!(found[0]["matched"], json!(["principal"]));
    assert_eq!(found[0]["fields"]["principal"], "user:ines");

    // "mirela" is in her name, which everyone reads, and in her login, which she and editors read.
    // The public is told of the name only, and the record it receives has no login.
    let public = backend.hits(None, "mirela").await;
    let authors: Vec<&Json> = public.iter().filter(|h| h["object"] == "author").collect();
    assert_eq!(authors.len(), 1, "{public:?}");
    assert_eq!(authors[0]["matched"], json!(["name"]));
    assert!(authors[0]["fields"].get("principal").is_none());
    for session in [&mirela, &editor] {
        let own = backend.hits(Some(session), "mirela").await;
        let author = own.iter().find(|h| h["object"] == "author").unwrap();
        assert_eq!(author["matched"], json!(["name", "principal"]));
    }
}

/// No search hit tells a visitor a login name (WG-4953), even when an author's login identity is
/// a searchable field: the public searches for the logins themselves, for the authors' names and
/// for words of the posts, and no hit it receives holds a login anywhere, while an editor's search
/// for the same words does.
#[tokio::test]
async fn no_search_hit_tells_a_visitor_a_login_name() {
    let (backend, store) = started().await;
    declare_author_search(&store);
    let editor = backend.login("ines").await;
    let logins = ["user:ines", "user:tomasz", "user:mirela", "user:admin"];
    let words = [
        "ines", "tomasz", "mirela", "admin", "user", "the", "kil", "jig", "wren", "okafor",
    ];
    let mut public_hits = 0;
    let mut editor_text = String::new();
    for word in words {
        let hits = backend.hits(None, word).await;
        public_hits += hits.len();
        let text = Json::Array(hits).to_string();
        for login in logins {
            assert!(
                !text.contains(login),
                "a search for {word:?} told a visitor {login}: {text}"
            );
        }
        editor_text.push_str(&Json::Array(backend.hits(Some(&editor), word).await).to_string());
    }
    assert!(public_hits > 0, "the public's searches found something");
    assert!(
        editor_text.contains("user:ines") && editor_text.contains("user:tomasz"),
        "an editor's hits carry the logins she may read: {editor_text}"
    );
}

#[tokio::test]
async fn pages_resume_exactly_and_a_cursor_that_is_not_ours_is_refused() {
    let (backend, _store) = started().await;
    let (status, whole) = backend
        .search_in(SITE, None, json!({ "query": "the", "limit": 200 }))
        .await;
    assert_eq!(status, 200, "{whole}");
    let all: Vec<Json> = whole["hits"].as_array().unwrap().clone();
    assert!(all.len() >= 3, "the word should be in several posts");
    assert!(whole["next"].is_null());

    let mut paged = Vec::new();
    let mut cursor = Json::Null;
    for _ in 0..all.len() + 2 {
        let mut request = json!({ "query": "the", "limit": 1 });
        if !cursor.is_null() {
            request["cursor"] = cursor.clone();
        }
        let (status, page) = backend.search_in(SITE, None, request).await;
        assert_eq!(status, 200, "{page}");
        paged.extend(page["hits"].as_array().unwrap().iter().cloned());
        cursor = page["next"].clone();
        if cursor.is_null() {
            break;
        }
    }
    assert_eq!(
        paged, all,
        "one hit a page gives the hits of one page, in order"
    );

    let (status, _) = backend
        .search_in(
            SITE,
            None,
            json!({ "query": "the", "cursor": "not a cursor" }),
        )
        .await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn a_query_without_words_or_over_the_bounds_is_refused_by_name() {
    let (backend, _store) = started().await;
    for request in [
        json!({ "query": "" }),
        json!({ "query": "   ... --- " }),
        json!({ "query": "one two three four five six seven eight nine" }),
        json!({ "query": "x".repeat(201) }),
        json!({ "query": "kiln", "limit": 0 }),
        json!({ "query": "kiln", "limit": 201 }),
    ] {
        let (status, body) = backend.search_in(SITE, None, request.clone()).await;
        assert_eq!(status, 400, "{request}: {body}");
    }
    let (status, _) = backend
        .search_in(
            SITE,
            None,
            json!({ "query": "kiln", "objects": ["nonesuch"] }),
        )
        .await;
    assert_eq!(status, 404);
}

// Verifies: NG-10
#[tokio::test]
async fn a_search_of_one_tenant_finds_nothing_of_another() {
    let (backend, _store) = started().await;
    assert!(!backend.hits(None, "kiln").await.is_empty());
    let (status, body) = backend
        .search_in("tidewater", None, json!({ "query": "kiln" }))
        .await;
    assert_eq!(status, 200, "{body}");
    assert!(body["hits"].as_array().unwrap().is_empty(), "{body}");
}
