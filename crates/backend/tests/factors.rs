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
use strata_kernel::{canon, PrincipalId, TenantId, Timestamp};
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
    let store = Arc::new(MemoryStore::new());
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

fn principal(name: &str) -> PrincipalId {
    PrincipalId::new(format!("user:{name}")).unwrap()
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

fn wrong(right: &str) -> String {
    format!("{:06}", (right.parse::<u32>().unwrap() + 1) % 1_000_000)
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

/// IDN-018: a first factor alone yields no session when the account has a second factor; the
/// second factor yields it.
#[tokio::test]
async fn a_first_factor_alone_yields_no_session_and_the_second_yields_it() {
    let backend = start().await;
    let secret = backend.enrol("ines").await;

    let waiting = backend.waiting("ines").await;
    assert_eq!(waiting["methods"], json!(["totp"]));
    assert_eq!(waiting["enrol"], Json::Null);
    let challenge = waiting["challenge"].as_str().unwrap();

    let (status, refused) = backend
        .second(challenge, "totp", &wrong(&code(&secret, 0)))
        .await;
    assert_eq!(status, 401, "{refused}");
    let (status, accepted) = backend.second(challenge, "totp", &code(&secret, 0)).await;
    assert_eq!(status, 200, "{accepted}");
    let token = accepted["token"].as_str().unwrap();
    assert!(backend.is_signed_in(token).await);

    // The waiting sign-in answers once: the same challenge gives no second session.
    let (status, _) = backend.second(challenge, "totp", &code(&secret, 1)).await;
    assert_eq!(status, 401);
}

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

/// IDN-018: a realm that asks for a second factor of a person who has none has the person add one
/// in the same step, and the first right code both confirms it and signs in.
#[tokio::test]
async fn a_realm_that_requires_a_second_factor_enrols_one_at_sign_in() {
    let backend = start_with(|cfg| {
        cfg.factors.realm_required.insert(tenant());
    })
    .await;
    let waiting = backend.waiting("tomasz").await;
    assert_eq!(waiting["methods"], json!(["totp"]));
    let secret = waiting["enrol"]["secret"]
        .as_str()
        .expect("the secret goes with the prompt");
    assert!(waiting["enrol"]["uri"]
        .as_str()
        .unwrap()
        .starts_with("otpauth://totp/"));

    let challenge = waiting["challenge"].as_str().unwrap();
    let (status, accepted) = backend.second(challenge, "totp", &code(secret, 0)).await;
    assert_eq!(status, 200, "{accepted}");
    assert!(
        backend
            .store
            .totp(&tenant(), &principal("tomasz"))
            .unwrap()
            .unwrap()
            .confirmed
    );
}

/// IDN-018: a code that was used is refused within its time step.
#[tokio::test]
async fn a_code_that_was_used_is_refused_again() {
    let backend = start().await;
    let secret = backend.enrol("ines").await;
    let used = code(&secret, 0);

    let first = backend.waiting("ines").await;
    let (status, _) = backend
        .second(first["challenge"].as_str().unwrap(), "totp", &used)
        .await;
    assert_eq!(status, 200);

    let again = backend.waiting("ines").await;
    let (status, _) = backend
        .second(again["challenge"].as_str().unwrap(), "totp", &used)
        .await;
    assert_eq!(status, 401, "the same code opened a second sign-in");
    // The code of the step before is no way round it either.
    let (status, _) = backend
        .second(
            again["challenge"].as_str().unwrap(),
            "totp",
            &code(&secret, -1),
        )
        .await;
    assert_eq!(status, 401);
    // A later step is accepted.
    let (status, _) = backend
        .second(
            again["challenge"].as_str().unwrap(),
            "totp",
            &code(&secret, 1),
        )
        .await;
    assert_eq!(status, 200);
}

/// IDN-253: a waiting sign-in is spent when its failed presentations reach the limit, even for a
/// right code afterwards.
#[tokio::test]
async fn five_wrong_codes_spend_the_waiting_sign_in() {
    let backend = start().await;
    let secret = backend.enrol("ines").await;
    let waiting = backend.waiting("ines").await;
    let challenge = waiting["challenge"].as_str().unwrap();
    for _ in 0..5 {
        let (status, _) = backend
            .second(challenge, "totp", &wrong(&code(&secret, 0)))
            .await;
        assert_eq!(status, 401);
    }
    let (status, _) = backend.second(challenge, "totp", &code(&secret, 0)).await;
    assert_eq!(status, 401, "a spent sign-in accepted a right code");
}

/// IDN-008: a code is counted against the account's limit before it is looked at, so that
/// presentations that arrive after the limit are refused whatever they hold.
#[tokio::test]
async fn codes_count_against_the_throttle() {
    let backend = start_with(|cfg| cfg.throttle.account_max = 3).await;
    let secret = backend.enrol("ines").await;
    let waiting = backend.waiting("ines").await;
    let challenge = waiting["challenge"].as_str().unwrap();
    for _ in 0..3 {
        let (status, _) = backend
            .second(challenge, "totp", &wrong(&code(&secret, 0)))
            .await;
        assert_eq!(status, 401);
    }
    let (status, body) = backend.second(challenge, "totp", &code(&secret, 0)).await;
    assert_eq!(status, 429, "{body}");
}

/// IDN-268: a realm sends at most five prompts to one account in an hour.
#[tokio::test]
async fn at_most_five_prompts_are_sent_to_one_account_in_an_hour() {
    let backend = start().await;
    backend.enrol("ines").await;
    for _ in 0..5 {
        backend.waiting("ines").await;
    }
    let (status, body) = backend.sign_in("ines", "strata-dev-ines").await;
    assert_eq!(status, 429, "{body}");
}

/// IDN-018: the secret is not at rest in the clear, and it is bound to its person.
#[tokio::test]
async fn the_secret_is_sealed_at_rest() {
    let backend = start().await;
    let secret = backend.enrol("ines").await;
    let stored = backend
        .store
        .totp(&tenant(), &principal("ines"))
        .unwrap()
        .unwrap();
    assert!(
        !stored.secret.contains(&secret),
        "the base32 secret is in the store"
    );
    let raw = Secret::Encoded(secret.clone()).to_bytes().unwrap();
    assert!(!stored
        .secret
        .as_bytes()
        .windows(8)
        .any(|w| raw.windows(8).any(|r| r == w)));
    // Moved to another person's row, it opens no code for them.
    let mut moved = stored.clone();
    moved.principal = principal("tomasz");
    backend.store.put_totp(&moved).unwrap();
    let (status, body) = backend.sign_in("tomasz", "strata-dev-tomasz").await;
    assert_eq!(status, 200, "{body}");
    let challenge = body["second_factor"]["challenge"]
        .as_str()
        .expect("tomasz now has a factor on file");
    let (status, _) = backend.second(challenge, "totp", &code(&secret, 0)).await;
    assert_eq!(status, 401, "a sealed secret opened for another person");
}

/// IDN-018: recovery codes are made once, each works once, and they are kept as hashes.
#[tokio::test]
async fn recovery_codes_work_once_and_are_stored_hashed() {
    let backend = start().await;
    let secret = backend.enrol("ines").await;
    let session = backend.session_of_second("ines", &secret, 0).await;
    let (status, made) = backend
        .call(
            Some(&session),
            "/internal/factors/recovery-codes",
            json!({}),
        )
        .await;
    assert_eq!(status, 200, "{made}");
    let codes: Vec<String> = made["codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    assert_eq!(codes.len(), 10);
    assert_eq!(
        backend
            .store
            .recovery_codes_left(&tenant(), &principal("ines"))
            .unwrap(),
        10
    );
    // Nothing the store holds is a code that can be presented.
    assert!(!backend
        .store
        .spend_recovery_code(&tenant(), &principal("ines"), &codes[0])
        .unwrap());

    let waiting = backend.waiting("ines").await;
    assert!(waiting["methods"]
        .as_array()
        .unwrap()
        .contains(&json!("recovery")));
    let (status, _) = backend
        .second(
            waiting["challenge"].as_str().unwrap(),
            "recovery",
            &codes[0],
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        backend
            .store
            .recovery_codes_left(&tenant(), &principal("ines"))
            .unwrap(),
        9
    );

    let again = backend.waiting("ines").await;
    let (status, _) = backend
        .second(again["challenge"].as_str().unwrap(), "recovery", &codes[0])
        .await;
    assert_eq!(status, 401, "a recovery code worked twice");
    // Another code of the set still does, however it is written.
    let spaced = codes[1].to_lowercase().replace('-', " ");
    let (status, _) = backend
        .second(again["challenge"].as_str().unwrap(), "recovery", &spaced)
        .await;
    assert_eq!(status, 200);
}

/// Regenerating ends the old set.
#[tokio::test]
async fn new_recovery_codes_end_the_old_set() {
    let backend = start().await;
    let secret = backend.enrol("ines").await;
    let session = backend.session_of_second("ines", &secret, 0).await;
    let (_, first) = backend
        .call(
            Some(&session),
            "/internal/factors/recovery-codes",
            json!({}),
        )
        .await;
    let old = first["codes"][0].as_str().unwrap().to_string();
    let (_, second) = backend
        .call(
            Some(&session),
            "/internal/factors/recovery-codes",
            json!({}),
        )
        .await;
    assert_ne!(second["codes"][0], first["codes"][0]);
    let waiting = backend.waiting("ines").await;
    let (status, _) = backend
        .second(waiting["challenge"].as_str().unwrap(), "recovery", &old)
        .await;
    assert_eq!(status, 401, "a code of the old set worked");
}

/// IDN-018: a passkey is another way to give the second factor; one that is not the person's own
/// is refused. The ceremony itself is the library's, covered with the passkeys; here the refusal
/// of an assertion that answers no challenge.
#[tokio::test]
async fn a_passkey_assertion_that_answers_no_challenge_is_no_second_factor() {
    let backend = start().await;
    backend.enrol("ines").await;
    let waiting = backend.waiting("ines").await;
    let (status, _) = backend
        .call(
            None,
            "/internal/session/second-factor",
            json!({
                "challenge": waiting["challenge"],
                "factor": "passkey",
                "passkey_challenge": "ab".repeat(32),
                "assertion": { "id": "x", "rawId": "x", "type": "public-key", "response": {} },
            }),
        )
        .await;
    assert_eq!(status, 401);
}

/// IDN-019: a sensitive operation is refused when the session authenticated too long ago, and a
/// fresh factor lets it through. Account without a second factor: the password.
#[tokio::test]
async fn a_sensitive_operation_needs_a_recent_authentication() {
    let backend = start().await;
    let session = backend.session_of("tomasz").await;
    backend.age(&session, 16 * 60_000);

    let (status, refused) = backend
        .call(Some(&session), "/internal/factors/totp/start", json!({}))
        .await;
    assert_eq!(status, 403, "{refused}");
    assert_eq!(refused["kind"], "step_up_required");
    for path in [
        "/internal/sessions/revoke-others",
        "/internal/passkeys/register/start",
    ] {
        let (status, refused) = backend.call(Some(&session), path, json!({})).await;
        assert_eq!(
            (status, refused["kind"].as_str()),
            (403, Some("step_up_required")),
            "{path}"
        );
    }

    let (status, _) = backend
        .call(
            Some(&session),
            "/internal/session/step-up",
            json!({ "password": "not it" }),
        )
        .await;
    assert_eq!(status, 401);
    let (status, confirmed) = backend
        .call(
            Some(&session),
            "/internal/session/step-up",
            json!({ "password": "strata-dev-tomasz" }),
        )
        .await;
    assert_eq!(status, 200, "{confirmed}");
    let (status, started) = backend
        .call(Some(&session), "/internal/factors/totp/start", json!({}))
        .await;
    assert_eq!(status, 200, "{started}");
}

/// IDN-019: within the age, no step-up is asked for.
#[tokio::test]
async fn a_recent_authentication_needs_no_step_up() {
    let backend = start().await;
    let session = backend.session_of("tomasz").await;
    backend.age(&session, 14 * 60_000);
    let (status, _) = backend
        .call(Some(&session), "/internal/factors/totp/start", json!({}))
        .await;
    assert_eq!(status, 200);
}

/// IDN-019: a person who has a second factor gives it again; the password alone is not a fresh
/// authentication for them.
#[tokio::test]
async fn a_person_with_a_second_factor_steps_up_with_it() {
    let backend = start().await;
    let secret = backend.enrol("ines").await;
    let session = backend.session_of_second("ines", &secret, 0).await;
    backend.age(&session, 20 * 60_000);

    let (status, _) = backend
        .call(
            Some(&session),
            "/internal/session/step-up",
            json!({ "password": "strata-dev-ines" }),
        )
        .await;
    assert_eq!(status, 401, "the password alone was a fresh authentication");
    let (status, _) = backend
        .call(
            Some(&session),
            "/internal/session/step-up",
            json!({ "code": code(&secret, 1) }),
        )
        .await;
    assert_eq!(status, 200);
    let (status, _) = backend
        .call(
            Some(&session),
            "/internal/factors/recovery-codes",
            json!({}),
        )
        .await;
    assert_eq!(status, 200);
}

/// IDN-260: a session that appears from another network context authenticates again before a
/// sensitive operation, and the record holds a hash and not the address.
#[tokio::test]
async fn a_session_from_another_network_context_is_a_new_arrival() {
    let backend = start().await;
    let (_, body) = backend
        .call_from(
            "203.0.113.9",
            None,
            "/internal/session/login",
            json!({ "login": "tomasz", "password": "strata-dev-tomasz" }),
        )
        .await;
    let session = body["token"].as_str().unwrap().to_string();
    let stored = backend
        .store
        .session_by_token(&canon::hash_bytes(session.as_bytes()))
        .unwrap()
        .unwrap();
    let held = stored.network.expect("the context is kept");
    assert!(
        !held.contains("203.0.113.9"),
        "the address is in the record"
    );

    let (status, _) = backend
        .call_from(
            "203.0.113.9",
            Some(&session),
            "/internal/factors/totp/start",
            json!({}),
        )
        .await;
    assert_eq!(status, 200, "the same context was asked to step up");
    let (status, refused) = backend
        .call_from(
            "198.51.100.4",
            Some(&session),
            "/internal/passkeys/register/start",
            json!({}),
        )
        .await;
    assert_eq!(
        (status, refused["kind"].as_str()),
        (403, Some("step_up_required"))
    );
    let (status, _) = backend
        .call_from(
            "198.51.100.4",
            Some(&session),
            "/internal/session/step-up",
            json!({ "password": "strata-dev-tomasz" }),
        )
        .await;
    assert_eq!(status, 200);
    // From then on the session is at the new context.
    let (status, _) = backend
        .call_from(
            "198.51.100.4",
            Some(&session),
            "/internal/factors/totp/remove",
            json!({}),
        )
        .await;
    assert_eq!(status, 200);
}

/// IDN-020, IDN-265, IDN-266: recovery tells the contacts first, waits out its delay, ends every
/// session and leaves the second factor in force.
#[tokio::test]
async fn recovery_tells_waits_ends_sessions_and_keeps_the_second_factor() {
    let backend = start_with(|cfg| cfg.factors.recovery_delay_ms = 1_200).await;
    let address = "mirela@fieldnotes.example";
    let secret = backend.enrol("mirela").await;
    let before = backend.session_of_second("mirela", &secret, 0).await;

    let (status, asked) = backend
        .call(
            None,
            "/internal/recovery/request",
            json!({ "email": address }),
        )
        .await;
    assert_eq!((status, &asked), (200, &json!({ "requested": true })));
    let notice = backend.mail.outbox().pop().expect("the contact was told");
    assert_eq!(notice.to, address);
    let fragment = notice.text.split("#r=").nth(1).unwrap();
    let id = fragment.split('&').next().unwrap().to_string();
    let token = fragment
        .split("&t=")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    // Asking changed nothing yet.
    assert!(
        backend.is_signed_in(&before).await,
        "asking ended a session"
    );

    let complete =
        json!({ "challenge": id, "token": token, "password": "a new password, long enough" });
    let (status, early) = backend
        .call(None, "/internal/recovery/complete", complete.clone())
        .await;
    assert_eq!(status, 409, "{early}");
    assert!(backend.is_signed_in(&before).await);

    // Completing is refused (409, changing nothing) until the delay has passed: retry until it
    // is not, for at most five seconds.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let (status, done) = loop {
        let (status, done) = backend
            .call(None, "/internal/recovery/complete", complete.clone())
            .await;
        if status != 409 || std::time::Instant::now() >= deadline {
            break (status, done);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(status, 200, "{done}");
    assert!(
        !backend.is_signed_in(&before).await,
        "a session survived the recovery"
    );
    // The link works once.
    let (status, _) = backend
        .call(None, "/internal/recovery/complete", complete)
        .await;
    assert_eq!(status, 401);

    // The old password is no longer right, the new one is, and it still asks for the second factor.
    let (status, _) = backend.sign_in(address, "strata-dev-mirela").await;
    assert_eq!(status, 401);
    let (status, body) = backend
        .sign_in(address, "a new password, long enough")
        .await;
    assert_eq!(status, 200, "{body}");
    assert!(
        body.get("token").is_none(),
        "recovery switched the second factor off: {body}"
    );
    assert!(body["second_factor"]["challenge"].is_string());
    // Both the request and the completion were told to the contacts.
    assert_eq!(backend.mail.outbox().len(), 2);
}

/// IDN-265: a contact can cancel a recovery before it completes, and nothing is revealed about an
/// address that has no account.
#[tokio::test]
async fn a_recovery_can_be_cancelled_and_an_unknown_address_is_answered_alike() {
    let backend = start_with(|cfg| cfg.factors.recovery_delay_ms = 0).await;
    let (status, asked) = backend
        .call(
            None,
            "/internal/recovery/request",
            json!({ "email": "mirela@fieldnotes.example" }),
        )
        .await;
    assert_eq!(status, 200, "{asked}");
    let notice = backend.mail.outbox().pop().unwrap();
    let fragment = notice.text.split("#r=").nth(1).unwrap();
    let id = fragment.split('&').next().unwrap().to_string();
    let token = fragment
        .split("&t=")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    let cancel = notice
        .text
        .split("&x=")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();

    let (status, cancelled) = backend
        .call(
            None,
            "/internal/recovery/cancel",
            json!({ "challenge": id, "cancel": cancel }),
        )
        .await;
    assert_eq!((status, &cancelled["cancelled"]), (200, &json!(true)));
    let (status, _) = backend
        .call(
            None,
            "/internal/recovery/complete",
            json!({ "challenge": id, "token": token, "password": "a new password, long enough" }),
        )
        .await;
    assert_eq!(status, 401, "a cancelled recovery completed");

    let sent = backend.mail.outbox().len();
    let (status, unknown) = backend
        .call(
            None,
            "/internal/recovery/request",
            json!({ "email": "nobody@nowhere.example" }),
        )
        .await;
    assert_eq!((status, &unknown), (200, &json!({ "requested": true })));
    assert_eq!(
        backend.mail.outbox().len(),
        sent,
        "mail went to an address with no account"
    );
}

/// IDN-265: an account with no contact to tell cannot be recovered by this route.
#[tokio::test]
async fn an_account_with_no_contact_to_tell_is_not_recoverable() {
    let backend = start_with(|cfg| cfg.factors.recovery_delay_ms = 0).await;
    let (status, asked) = backend
        .call(
            None,
            "/internal/recovery/request",
            json!({ "email": "ines" }),
        )
        .await;
    assert_eq!((status, &asked), (200, &json!({ "requested": true })));
    assert!(backend.mail.outbox().is_empty());
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
