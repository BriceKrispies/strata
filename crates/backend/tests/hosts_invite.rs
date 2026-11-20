//! Tenant host bindings (the list the BFF learns its sites from) and the owner's first sign-in
//! invitation, as a running service: real sockets, the real bundles, the in-memory store the test
//! keeps a handle to.

use serde_json::{json, Value as Json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strata_backend::app::{App, Config};
use strata_backend::mail::DevSink;
use strata_backend::signin::invite;
use strata_kernel::TenantId;
use strata_protocol::api::{
    HEADER_CORRELATION, HEADER_ORIGIN, HEADER_SERVICE, HEADER_SITE, HEADER_SOURCE,
};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::Store;

const TOKEN: &str = "test-service-token-0123456789abcdef";
const SITE: &str = "fieldnotes";
const ORIGIN: &str = "http://localhost:4200";

struct Backend {
    base: String,
    http: reqwest::Client,
    store: Arc<MemoryStore>,
    /// A second handle on the same store, as the process that creates tenants at run time holds.
    app: Arc<App>,
}

async fn start() -> Backend {
    let store = Arc::new(MemoryStore::new());
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
        mail: Arc::new(DevSink::memory()),
        federation: Arc::new(strata_backend::signin::oidc::Federation::none()),
        factors: Default::default(),
        limits: Default::default(),
    };
    let app = App::new(cfg.clone(), store.clone());
    tokio::spawn(strata_backend::run(
        cfg,
        store.clone(),
        listener,
        std::future::pending(),
    ));
    let backend = Backend {
        base: format!("http://{addr}"),
        http: reqwest::Client::new(),
        store,
        app,
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

fn tenant(name: &str) -> TenantId {
    TenantId::new(name).unwrap()
}

impl Backend {
    async fn post(
        &self,
        path: &str,
        body: Json,
        service: bool,
        origin: &str,
        source: &str,
    ) -> (u16, Json) {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header(HEADER_SITE, SITE)
            .header(HEADER_SOURCE, source)
            .header(HEADER_ORIGIN, origin)
            .header(HEADER_CORRELATION, "t-hosts-invite-1")
            .json(&body);
        if service {
            request = request.header(HEADER_SERVICE, TOKEN);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Json::Null))
    }

    async fn hosts(&self, body: Json) -> (u16, Json) {
        self.post("/internal/platform/hosts", body, true, ORIGIN, "10.0.0.1")
            .await
    }

    async fn accept(&self, link: &str, password: &str, origin: &str, source: &str) -> (u16, Json) {
        let (id, token) = parts(link);
        self.post(
            "/internal/sign-in/invite/accept",
            json!({ "challenge": id, "token": token, "password": password }),
            true,
            origin,
            source,
        )
        .await
    }

    async fn sign_in(&self, login: &str, password: &str) -> u16 {
        self.post(
            "/internal/session/login",
            json!({ "login": login, "password": password }),
            true,
            ORIGIN,
            "10.0.0.2",
        )
        .await
        .0
    }
}

/// The identifier and the token of an invitation address.
fn parts(link: &str) -> (String, String) {
    let fragment = link.split_once("#i=").expect("an invitation address").1;
    let (id, token) = fragment.split_once("&t=").expect("a token");
    (id.to_string(), token.to_string())
}

/// Verifies: TEN-1680
#[tokio::test]
async fn hosts_are_bound_to_one_tenant_and_listed_to_the_service() {
    let backend = start().await;
    let (status, listed) = backend.hosts(json!({})).await;
    assert_eq!(status, 200, "{listed}");
    assert_eq!(
        listed["hosts"],
        json!([]),
        "no host data until one is bound"
    );

    let (status, bound) = backend
        .hosts(json!({ "bind": { "tenant": "tidewater", "origin": "https://Tide.example/" } }))
        .await;
    assert_eq!(status, 200, "{bound}");
    assert_eq!(
        bound["hosts"],
        json!([{ "host": "tide.example", "tenant": "tidewater", "origins": ["https://tide.example"] }])
    );
    // A second origin of the same host is added to it.
    let (_, again) = backend
        .hosts(json!({ "bind": { "tenant": "tidewater", "origin": "http://tide.example:4100" } }))
        .await;
    assert_eq!(
        again["hosts"][0]["origins"],
        json!(["http://tide.example:4100", "https://tide.example"])
    );

    // A host belongs to one tenant; a tenant that does not exist has no hosts; an origin that is
    // not an origin binds nothing.
    let (status, _) = backend
        .hosts(json!({ "bind": { "tenant": "fieldnotes", "origin": "https://tide.example" } }))
        .await;
    assert_eq!(status, 409, "a host bound to a second tenant");
    let (status, _) = backend
        .hosts(json!({ "bind": { "tenant": "nonesuch", "origin": "https://x.example" } }))
        .await;
    assert_eq!(status, 404);
    for bad in ["x.example", "https://x.example/path", "ftp://x.example", ""] {
        let (status, _) = backend
            .hosts(json!({ "bind": { "tenant": "fieldnotes", "origin": bad } }))
            .await;
        assert_eq!(status, 400, "{bad:?}");
    }
    let (_, listed) = backend.hosts(json!({})).await;
    assert_eq!(listed["hosts"].as_array().unwrap().len(), 1);

    // Only the service may read or change it.
    let (status, _) = backend
        .post(
            "/internal/platform/hosts",
            json!({}),
            false,
            ORIGIN,
            "10.0.0.1",
        )
        .await;
    assert_eq!(status, 401);
}

/// Verifies: IDN-1680
/// Verifies: IDN-1681
/// Verifies: IDN-1682
#[tokio::test]
async fn an_owner_invitation_sets_the_first_password_once_in_any_browser() {
    let backend = start().await;
    let fieldnotes = tenant(SITE);
    let ines = backend.store.account(&fieldnotes, "ines").unwrap().unwrap();
    let link = invite::issue(&backend.app, &fieldnotes, &ines.principal, ORIGIN).unwrap();
    assert!(link.starts_with(&format!("{ORIGIN}/login#i=")), "{link}");

    // Stored as hashes: neither the identifier nor the token is in the store.
    let (id, token) = parts(&link);
    for (key, value) in backend.store.doc_list(&fieldnotes, invite::KIND).unwrap() {
        for secret in [&id, &token] {
            assert!(!key.contains(secret.as_str()) && !value.contains(secret.as_str()));
        }
        assert!(!value.contains('@'), "no address is kept: {value}");
    }

    // What is wrong with an attempt: a short password, a wrong token, another site's origin.
    let (status, _) = backend.accept(&link, "short", ORIGIN, "10.0.0.3").await;
    assert_eq!(status, 400);
    let wrong_token = link.replace(&token, &"0".repeat(64));
    let (status, _) = backend
        .accept(&wrong_token, "a-brand-new-password", ORIGIN, "10.0.0.3")
        .await;
    assert_eq!(status, 401, "a wrong token");
    let (status, _) = backend
        .accept(
            &link,
            "a-brand-new-password",
            "http://evil.example",
            "10.0.0.3",
        )
        .await;
    assert_eq!(status, 401, "another origin");
    assert_eq!(
        backend.sign_in("ines", "strata-dev-ines").await,
        200,
        "the refusals changed nothing, and spent nothing"
    );

    // The right use, from a source that has never been seen: no browser state is needed.
    let (status, accepted) = backend
        .accept(&link, "a-brand-new-password", ORIGIN, "198.51.100.77")
        .await;
    assert_eq!(status, 200, "{accepted}");
    assert_eq!(accepted["login"], "ines");
    assert_eq!(backend.sign_in("ines", "a-brand-new-password").await, 200);
    assert_eq!(
        backend.sign_in("ines", "strata-dev-ines").await,
        401,
        "the unknown password was replaced"
    );

    // Once.
    let (status, _) = backend
        .accept(&link, "another-new-password", ORIGIN, "198.51.100.77")
        .await;
    assert_eq!(status, 401, "a second use");
    assert_eq!(backend.sign_in("ines", "another-new-password").await, 401);

    // For a while, and not after.
    let lapsed =
        invite::issue_for(&backend.app, &fieldnotes, &ines.principal, ORIGIN, -1_000).unwrap();
    let (status, _) = backend
        .accept(&lapsed, "yet-another-password", ORIGIN, "198.51.100.78")
        .await;
    assert_eq!(status, 401, "an expired invitation");
    assert_eq!(backend.sign_in("ines", "yet-another-password").await, 401);
}
