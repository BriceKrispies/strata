//! Second factors, recovery codes, step-up and recovery, as a running service: real sockets, the
//! real bundles, the in-memory store the test keeps a handle to.
//!
//! Every test names the requirement it holds up, and is written so that it fails when the
//! requirement is not met. Codes are computed here with the same `totp-rs` crate the backend uses.

use serde_json::{json, Value as Json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use strata_backend::app::Config;
use strata_backend::mail::{DevSink, Mailer};
use strata_kernel::{canon, TenantId, Timestamp};
use strata_protocol::api::{
    HEADER_CORRELATION, HEADER_ORIGIN, HEADER_SERVICE, HEADER_SESSION, HEADER_SITE, HEADER_SOURCE,
};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::Store;
use totp_rs::{Algorithm, Secret, TOTP};

const TOKEN: &str = "test-service-token-0123456789abcdef";
const SITE: &str = "fieldnotes";
const ORIGIN: &str = "http://localhost:4200";

struct Backend {
    base: String,
    http: reqwest::Client,
    store: Arc<MemoryStore>,
    mail: Arc<DevSink>,
}

async fn start() -> Backend {
    start_with(|_| {}).await
}

async fn start_with(adjust: impl FnOnce(&mut Config)) -> Backend {
    start_on(Arc::new(MemoryStore::new()), adjust).await
}

/// A backend over a store that already has a history: a restart with other settings.
async fn start_on(store: Arc<MemoryStore>, adjust: impl FnOnce(&mut Config)) -> Backend {
    let mail = Arc::new(DevSink::memory());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut cfg = Config {
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
        mail: mail.clone(),
        federation: Arc::new(strata_backend::signin::oidc::Federation::none()),
        factors: Default::default(),
        limits: Default::default(),
    };
    adjust(&mut cfg);
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
        mail,
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

fn tenant() -> TenantId {
    TenantId::new(SITE).unwrap()
}

/// The code an authenticator shows `steps` time steps from now.
fn code(secret: &str, steps: i64) -> String {
    let bytes = Secret::Encoded(secret.to_string()).to_bytes().unwrap();
    let totp = TOTP::new(Algorithm::SHA1, 6, 1, 30, bytes).unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    totp.generate((now + steps * 30) as u64)
}

impl Backend {
    /// A call from a source, with an origin, as the BFF makes it.
    async fn call_from(
        &self,
        source: &str,
        session: Option<&str>,
        path: &str,
        body: Json,
    ) -> (u16, Json) {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header(HEADER_SERVICE, TOKEN)
            .header(HEADER_SITE, SITE)
            .header(HEADER_SOURCE, source)
            .header(HEADER_ORIGIN, ORIGIN)
            .header(HEADER_CORRELATION, "t-factors-1")
            .json(&body);
        if let Some(token) = session {
            request = request.header(HEADER_SESSION, token);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Json::Null))
    }

    async fn call(&self, session: Option<&str>, path: &str, body: Json) -> (u16, Json) {
        self.call_from("10.0.0.1", session, path, body).await
    }

    /// A password sign-in, answered whole.
    async fn sign_in(&self, login: &str, password: &str) -> (u16, Json) {
        self.call(
            None,
            "/internal/session/login",
            json!({ "login": login, "password": password }),
        )
        .await
    }

    /// A password sign-in that must have yielded a session.
    async fn session_of(&self, name: &str) -> String {
        let (status, body) = self.sign_in(name, &format!("strata-dev-{name}")).await;
        assert_eq!(status, 200, "{body}");
        body["token"]
            .as_str()
            .unwrap_or_else(|| panic!("no session: {body}"))
            .to_string()
    }

    /// A password sign-in that must have yielded a waiting sign-in; its identifier.
    async fn waiting(&self, name: &str) -> Json {
        let (status, body) = self.sign_in(name, &format!("strata-dev-{name}")).await;
        assert_eq!(status, 200, "{body}");
        assert!(
            body.get("token").is_none(),
            "a first factor alone gave a session: {body}"
        );
        body["second_factor"].clone()
    }

    async fn second(&self, challenge: &str, factor: &str, code: &str) -> (u16, Json) {
        self.call(
            None,
            "/internal/session/second-factor",
            json!({ "challenge": challenge, "factor": factor, "code": code }),
        )
        .await
    }

    /// Add an authenticator to a person with a session of its own. Returns the base32 secret.
    async fn enrol(&self, name: &str) -> String {
        let session = self.session_of(name).await;
        let (status, started) = self
            .call(Some(&session), "/internal/factors/totp/start", json!({}))
            .await;
        assert_eq!(status, 200, "{started}");
        let secret = started["secret"].as_str().unwrap().to_string();
        let (status, done) = self
            .call(
                Some(&session),
                "/internal/factors/totp/confirm",
                json!({ "code": code(&secret, -1) }),
            )
            .await;
        assert_eq!(status, 200, "{done}");
        secret
    }

    /// An emailed-link sign-in for an address with an account, answered whole.
    async fn link_sign_in(&self, email: &str) -> (u16, Json) {
        let (status, requested) = self
            .call(
                None,
                "/internal/sign-in/link/request",
                json!({ "email": email }),
            )
            .await;
        assert_eq!(status, 200, "{requested}");
        let binding = requested["binding"].as_str().unwrap().to_string();
        let message = self.mail.outbox().pop().expect("a message was sent");
        let fragment = message
            .text
            .split("#c=")
            .nth(1)
            .expect("the link has a fragment");
        let challenge = fragment.split('&').next().unwrap();
        let token = fragment
            .split("&t=")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap();
        self.call(
            None,
            "/internal/session/sign-in/link",
            json!({ "challenge": challenge, "token": token, "binding": binding }),
        )
        .await
    }

    /// Make a session look as if it authenticated `ago_ms` ago.
    fn age(&self, token: &str, ago_ms: i64) {
        let mut session = self
            .store
            .session_by_token(&canon::hash_bytes(token.as_bytes()))
            .unwrap()
            .expect("the session exists");
        session.authenticated_at = Timestamp(session.authenticated_at.0 - ago_ms);
        self.store.put_session(&session).unwrap();
    }

    async fn is_signed_in(&self, token: &str) -> bool {
        let (_, body) = self.call(Some(token), "/internal/session", json!({})).await;
        body["session"]["authenticated"] == json!(true)
    }
}

/// Verifies: IDN-018, IDN-267
/// IDN-018: the stage is the same for every method, so a method that proves one factor does not
/// get past it. An emailed link is such a method.
#[tokio::test]
async fn a_method_that_proves_one_factor_gets_no_session_in_a_realm_that_asks_for_two() {
    let backend = start_with(|cfg| {
        cfg.factors.realm_required.insert(tenant());
    })
    .await;
    // A link is requested for an address with an account, and its message is read.
    let (status, requested) = backend
        .call(
            None,
            "/internal/sign-in/link/request",
            json!({ "email": "mirela@fieldnotes.example" }),
        )
        .await;
    assert_eq!(status, 200, "{requested}");
    let binding = requested["binding"].as_str().unwrap().to_string();
    let message = backend.mail.outbox().pop().expect("a message was sent");
    let fragment = message
        .text
        .split("#c=")
        .nth(1)
        .expect("the link has a fragment");
    let challenge = fragment.split('&').next().unwrap();
    let token = fragment
        .split("&t=")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap();
    let (status, body) = backend
        .call(
            None,
            "/internal/session/sign-in/link",
            json!({ "challenge": challenge, "token": token, "binding": binding }),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert!(
        body.get("token").is_none(),
        "the link alone gave a session: {body}"
    );
    assert!(body["second_factor"]["challenge"].is_string());
}

impl Backend {
    /// Sign in with a password and an authenticator's code `steps` from now; the session.
    async fn session_of_second(&self, name: &str, secret: &str, steps: i64) -> String {
        let waiting = self.waiting(name).await;
        let (status, body) = self
            .second(
                waiting["challenge"].as_str().unwrap(),
                "totp",
                &code(secret, steps),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        body["token"].as_str().unwrap().to_string()
    }
}
