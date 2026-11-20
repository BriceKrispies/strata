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
    start_until(store, dev, std::future::pending()).await
}

/// Start a backend on any store that ends when `stop` resolves.
async fn start_until(
    store: Arc<dyn Store>,
    dev: bool,
    stop: impl std::future::Future<Output = ()> + Send + 'static,
) -> Backend {
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
    tokio::spawn(strata_backend::run(cfg, store, listener, stop));
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

const OPERATOR: &str = "test-operator-token-0123456789abcdef";

impl Backend {
    /// The platform diagnostics, with the service credential and the operator credential as given.
    async fn platform(&self, service: Option<&str>, operator: Option<&str>) -> (u16, Json) {
        let mut request = self
            .http
            .post(format!("{}/internal/platform/diagnostics", self.base))
            .header(HEADER_SITE, "fieldnotes")
            .header(HEADER_CORRELATION, "t-corr-1")
            .json(&json!({}));
        if let Some(token) = service {
            request = request.header(HEADER_SERVICE, token);
        }
        if let Some(token) = operator {
            request = request.header(strata_protocol::api::HEADER_OPERATOR, token);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Json::Null))
    }
}

/// The integrity check of every tenant file is in the platform diagnostics, and those need the
/// operator credential, which is not the service credential the gateway holds. A caller with no
/// credential, with the service credential only, or with a wrong operator credential is refused
/// and sees no integrity result; liveness answers anyone and carries none; a tenant's own
/// diagnostics carry none; the operator is answered, and each refusal and each read is an audit
/// entry. With no operator credential configured outside development the route does not exist.
#[tokio::test]
async fn the_platform_diagnostics_need_the_operator_credential_and_each_call_is_audited() {
    platform_diagnostics_in_development_are_audited().await;
    let production = start(Arc::new(MemoryStore::new()), false).await;
    let (status, _) = production.platform(Some(TOKEN), Some(OPERATOR)).await;
    assert_eq!(status, 404, "no operator credential is configured");

    std::env::set_var("STRATA_OPERATOR_TOKEN", OPERATOR);
    let store = Arc::new(MemoryStore::new());
    let operated = start(store.clone(), false).await;
    std::env::remove_var("STRATA_OPERATOR_TOKEN");

    for (service, operator) in [
        (None, None),
        (None, Some(OPERATOR)),
        (Some(TOKEN), None),
        (Some(TOKEN), Some(TOKEN)),
        (Some(TOKEN), Some("not-the-operator-token-at-all-1")),
    ] {
        let (status, body) = operated.platform(service, operator).await;
        assert_eq!(status, 401, "{service:?} {operator:?}: {body}");
        assert!(!body.to_string().contains("quick_check"), "{body}");
    }
    let (status, health) = operated
        .http
        .get(format!("{}/internal/health", operated.base))
        .send()
        .await
        .map(|r| (r.status().as_u16(), r))
        .unwrap();
    assert_eq!(status, 200);
    let health = health.text().await.unwrap();
    assert!(
        !health.contains("quick_check") && !health.contains("tenants"),
        "liveness carries no integrity result: {health}"
    );
    let (status, report) = operated.platform(Some(TOKEN), Some(OPERATOR)).await;
    assert_eq!(status, 200, "{report}");
    assert!(
        report["store"]["health"].get("detail").is_some(),
        "{report}"
    );
    let (_, report) = operated.platform(Some(TOKEN), Some(OPERATOR)).await;
    let count = |event: &str| {
        report["audit"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["event"] == event)
            .count()
    };
    assert_eq!(count("platform.diagnostics_refused"), 3, "{report}");
    assert_eq!(count("platform.diagnostics_read"), 2, "{report}");
}

/// In development the service credential stands for the operator and each call is an audit
/// entry. Run by the test above before it sets the operator credential in the environment, which
/// every backend started in the process would read.
async fn platform_diagnostics_in_development_are_audited() {
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

/// Every authentication decision is written to the audit trail of its tenant in the store, and
/// stays there across a restart: a sign-in and a refused sign-in are made on a backend over a
/// SQLite file, the backend is stopped, and a store opened on the same file holds both.
#[tokio::test]
async fn authentication_decisions_survive_a_restart_on_sqlite() {
    use strata_storage_sqlite::{default_tenants_dir, SqliteStore};
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("decisions-{}.db", std::process::id()));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    let _ = std::fs::remove_dir_all(default_tenants_dir(&path));
    let fieldnotes = TenantId::new("fieldnotes").unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let store = Arc::new(SqliteStore::open(&path, strata_kernel::Timestamp(1)).unwrap());
    let backend = start_until(store, true, async move {
        let _ = stopped.await;
    })
    .await;
    backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    let (status, _) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/session/login",
            json!({ "login": "admin", "password": "not the password" }),
        )
        .await;
    assert_eq!(status, 401);
    stop.send(()).unwrap();
    // Let the backend finish and let go of its files.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let reopened = SqliteStore::open(&path, strata_kernel::Timestamp(2)).unwrap();
    let decisions = reopened
        .audit_list(&fieldnotes, Some("decision"), 100)
        .unwrap();
    let outcomes: Vec<String> = decisions
        .iter()
        .filter_map(|line| serde_json::from_str::<Json>(&line.data).ok())
        .filter_map(|data| data["outcome"].as_str().map(str::to_string))
        .collect();
    assert!(
        outcomes.len() >= 2,
        "both decisions are in the trail after a restart: {outcomes:?}"
    );
    assert!(outcomes.iter().any(|o| o == "accepted"), "{outcomes:?}");
    assert!(outcomes.iter().any(|o| o == "refused"), "{outcomes:?}");
}
