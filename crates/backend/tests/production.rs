//! What a backend refuses to be outside development: no operator-wide diagnostics, no build tool,
//! no development routes, and nothing of another tenant in a tenant's own diagnostics.

use serde_json::{json, Value as Json};
use std::path::PathBuf;
use std::sync::Arc;
use strata_backend::app::Config;
use strata_kernel::TenantId;
use strata_protocol::api::{HEADER_CORRELATION, HEADER_SERVICE, HEADER_SESSION, HEADER_SITE};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::Store;

const TOKEN: &str = "test-service-token-0123456789abcdef";

struct Backend {
    base: String,
    http: reqwest::Client,
}

async fn start(store: Arc<MemoryStore>, dev: bool) -> Backend {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cfg = Config {
        addr,
        service_token: TOKEN.into(),
        dev,
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
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
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
            .header(HEADER_CORRELATION, "t-corr-1")
            .json(&body);
        if let Some(token) = session {
            request = request.header(HEADER_SESSION, token);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Json::Null))
    }

    async fn login(&self, site: &str, login: &str, password: &str) -> String {
        let (status, body) = self
            .call(
                site,
                None,
                "/internal/session/login",
                json!({ "login": login, "password": password }),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        body["token"].as_str().unwrap().to_string()
    }
}

/// The platform diagnostics read every tenant and the backend holds no operator identity to hold
/// a caller to, so outside development the route answers "not found"; in development each call
/// is an audit entry.
#[tokio::test]
async fn platform_diagnostics_exist_only_in_development_and_each_call_is_audited() {
    let production = start(Arc::new(MemoryStore::new()), false).await;
    let (status, _) = production
        .call(
            "fieldnotes",
            None,
            "/internal/platform/diagnostics",
            json!({}),
        )
        .await;
    assert_eq!(status, 404);

    let development = start(Arc::new(MemoryStore::new()), true).await;
    for _ in 0..2 {
        let (status, _) = development
            .call(
                "fieldnotes",
                None,
                "/internal/platform/diagnostics",
                json!({}),
            )
            .await;
        assert_eq!(status, 200);
    }
    let (_, report) = development
        .call(
            "fieldnotes",
            None,
            "/internal/platform/diagnostics",
            json!({}),
        )
        .await;
    let reads = report["audit"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["event"] == "platform.diagnostics_read")
        .count();
    assert_eq!(
        reads, 3,
        "the third call sees the two before it and its own"
    );
}

/// `/internal/lab/verify` runs `lake`; a production backend never starts a build tool.
#[tokio::test]
async fn the_lean_verification_is_refused_outside_development() {
    let store = Arc::new(MemoryStore::new());
    let development = start(store.clone(), true).await;
    let admin = development
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    let production = start(store, false).await;
    let (status, _) = production
        .call(
            "fieldnotes",
            Some(&admin),
            "/internal/lab/verify",
            json!({}),
        )
        .await;
    assert_eq!(status, 404);
}

/// A build without the `development` feature has no development routes at all, whatever the
/// process was told: the route table does not hold them.
#[cfg(not(feature = "development"))]
#[tokio::test]
async fn a_build_without_the_development_feature_has_no_development_routes() {
    let backend = start(Arc::new(MemoryStore::new()), true).await;
    for path in [
        "reset",
        "expire-session",
        "outbox",
        "age-authentication",
        "mail-outbox",
        "clock",
        "identity-outage",
    ] {
        let (status, _) = backend
            .call(
                "fieldnotes",
                None,
                &format!("/internal/dev/{path}"),
                json!({}),
            )
            .await;
        assert_eq!(status, 404, "/internal/dev/{path}");
    }
}

/// A tenant's own diagnostics hold nothing of another tenant: not its id, and not a count of the
/// whole platform's work.
#[tokio::test]
async fn a_tenants_diagnostics_hold_nothing_of_another_tenant() {
    let store = Arc::new(MemoryStore::new());
    let backend = start(store.clone(), true).await;
    let tide = backend
        .login("tidewater", "admin", "strata-dev-tidewater")
        .await;
    let admin = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    let (status, report) = backend
        .call(
            "fieldnotes",
            Some(&admin),
            "/internal/diagnostics",
            json!({}),
        )
        .await;
    assert_eq!(status, 200, "{report}");
    let text = report.to_string();
    assert!(!text.contains("tidewater"), "{text}");
    assert!(report.get("pending_operations").is_none(), "{text}");
    assert!(report["store"]["health"].get("detail").is_none(), "{text}");
    drop(tide);
    // The audit trail of fieldnotes is its own.
    assert!(store
        .audit_list(&TenantId::new("tidewater").unwrap(), None, 10)
        .unwrap()
        .iter()
        .all(|line| !line.data.contains("fieldnotes")));
}
