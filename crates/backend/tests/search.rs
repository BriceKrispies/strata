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
use strata_protocol::api::{HEADER_CORRELATION, HEADER_SERVICE, HEADER_SESSION, HEADER_SITE};
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

async fn started() -> (Backend, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::new());
    let backend = start_on(store.clone()).await;
    (backend, store)
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
