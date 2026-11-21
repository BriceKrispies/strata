//! Sign-in at an OpenID Connect provider, against a local provider that plays the other side
//! (`support/mock_oidc.rs`): the flow with its state, nonce and proof key, every refusal of the
//! sign-in table's `oidc-*` rows and of the requirements behind them, and the realm's policy for
//! who gets in. Real sockets, real bundles, the in-memory store.

#[path = "support/mock_oidc.rs"]
mod mock_oidc;

use jsonwebtoken::Algorithm;
use mock_oidc::{Mock, Signer, CLIENT_ID, CLIENT_SECRET};
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strata_backend::app::Config;
use strata_backend::mail::{DevSink, Mailer};
use strata_backend::signin::oidc::{Federation, Http, ProviderConfig, Secret};
use strata_kernel::{canon, TenantId, Timestamp};
use strata_protocol::api::{
    HEADER_CORRELATION, HEADER_ORIGIN, HEADER_SERVICE, HEADER_SESSION, HEADER_SITE, HEADER_SOURCE,
};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::Store;

const TOKEN: &str = "test-service-token-0123456789abcdef";
/// The address the tenant is reached at, in these tests.
const SITE: &str = "https://fieldnotes.example";
const SOURCE: &str = "203.0.113.7";
const SITE_CALLBACK: &str = "https://fieldnotes.example/federation/oidc/callback";
/// What every refusal of the method says, whatever was wrong.
const REFUSAL: &str = "The sign-in with the identity provider was not accepted.";

struct Backend {
    base: String,
    http: reqwest::Client,
    /// A client that follows no redirect: the browser's move from the provider back to the site
    /// is made by the test.
    plain: reqwest::Client,
    store: Arc<MemoryStore>,
    sink: Arc<DevSink>,
}

/// A provider as a realm would configure it: Ed25519 tokens, creating people on a first sign-in
/// as authors.
fn provider(mock: &Mock, adjust: impl FnOnce(&mut ProviderConfig)) -> ProviderConfig {
    let mut provider = ProviderConfig {
        id: "acme".into(),
        label: "Acme".into(),
        enabled: true,
        issuer: mock.base.clone(),
        client_id: CLIENT_ID.into(),
        client_secret: Some(Secret::new(CLIENT_SECRET)),
        scopes: vec!["openid".into(), "email".into(), "profile".into()],
        algorithms: vec![Algorithm::EdDSA],
        domains: Vec::new(),
        create_on_first_sign_in: true,
        default_role: Some("author".into()),
        link_verified_email: false,
    };
    adjust(&mut provider);
    provider
}

fn federation(providers: Vec<ProviderConfig>, floor_ms: Option<i64>) -> Federation {
    let federation = Federation::new(
        BTreeMap::from([(TenantId::new("fieldnotes").unwrap(), providers)]),
        // The provider is on this machine over plain HTTP: the development exception (FED-2644).
        Arc::new(Http::new(true)),
    );
    match floor_ms {
        Some(floor) => federation.with_refresh_floor(floor),
        None => federation,
    }
}

async fn start(federation: Federation) -> Backend {
    start_with(federation, |_| {}).await
}

async fn start_with(federation: Federation, adjust: impl FnOnce(&mut Config)) -> Backend {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let sink = Arc::new(DevSink::memory());
    let store = Arc::new(MemoryStore::new());
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
        mail: sink.clone(),
        federation: Arc::new(federation),
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
        plain: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        store,
        sink,
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

/// One sign-in on its way: what the browser holds and what the provider sent it back with.
struct Round {
    state: String,
    code: String,
    /// The nonce in the starting browser's cookie.
    binding: String,
}

impl Backend {
    /// A call as the gateway makes it for a browser at `origin`.
    async fn call_at(
        &self,
        origin: Option<&str>,
        site: &str,
        session: Option<&str>,
        path: &str,
        body: Json,
    ) -> (u16, Json) {
        self.call_from(origin, SOURCE, site, session, path, body)
            .await
    }

    async fn call_from(
        &self,
        origin: Option<&str>,
        source: &str,
        site: &str,
        session: Option<&str>,
        path: &str,
        body: Json,
    ) -> (u16, Json) {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header(HEADER_SERVICE, TOKEN)
            .header(HEADER_SITE, site)
            .header(HEADER_SOURCE, source)
            .header(HEADER_CORRELATION, "t-corr-oidc")
            .json(&body);
        if let Some(origin) = origin {
            request = request.header(HEADER_ORIGIN, origin);
        }
        if let Some(token) = session {
            request = request.header(HEADER_SESSION, token);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Json::Null))
    }

    async fn password(&self, login: &str, password: &str) -> (u16, Json) {
        self.call_at(
            Some(SITE),
            "fieldnotes",
            None,
            "/internal/session/login",
            json!({ "login": login, "password": password }),
        )
        .await
    }

    async fn session_of(&self, login: &str) -> String {
        let (status, reply) = self.password(login, &format!("strata-dev-{login}")).await;
        assert_eq!(status, 200, "{reply}");
        reply["token"].as_str().unwrap().to_string()
    }

    /// Start a sign-in as the sign-in page does and return what the gateway would hold.
    async fn start_at(
        &self,
        origin: Option<&str>,
        session: Option<&str>,
        body: Json,
    ) -> (u16, Json) {
        self.call_at(
            origin,
            "fieldnotes",
            session,
            "/internal/sign-in/oidc/start",
            body,
        )
        .await
    }

    /// The whole of the browser's part up to the callback: start, go to the provider, and come
    /// back with what it sent.
    async fn begin(&self, session: Option<&str>, extra: Json) -> Round {
        let mut body = json!({ "provider": "acme" });
        for (name, value) in extra.as_object().cloned().unwrap_or_default() {
            body[name] = value;
        }
        let (status, started) = self.start_at(Some(SITE), session, body).await;
        assert_eq!(status, 200, "{started}");
        let binding = started["binding"].as_str().unwrap().to_string();
        assert_eq!(binding.len(), 64);
        let redirect = started["redirect"].as_str().unwrap();
        let (state, code) = self.at_the_provider(redirect).await;
        Round {
            state,
            code: code.expect("the provider sent a code"),
            binding,
        }
    }

    /// Follow the provider's authorisation address once, without following where it sends the
    /// browser, and read what it sends the browser back with.
    async fn at_the_provider(&self, redirect: &str) -> (String, Option<String>) {
        let response = self.plain.get(redirect).send().await.unwrap();
        assert!(response.status().is_redirection(), "{}", response.status());
        let location = response.headers()["location"].to_str().unwrap().to_string();
        assert!(location.starts_with(SITE_CALLBACK), "{location}");
        let url = reqwest::Url::parse(&location).unwrap();
        let find = |name: &str| {
            url.query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };
        (find("state").unwrap_or_default(), find("code"))
    }

    /// The callback, from the browser that holds `binding`, at `origin`.
    async fn finish(
        &self,
        round: &Round,
        origin: Option<&str>,
        session: Option<&str>,
    ) -> (u16, Json) {
        self.call_at(
            origin,
            "fieldnotes",
            session,
            "/internal/session/sign-in/oidc",
            json!({ "state": round.state, "code": round.code, "binding": round.binding }),
        )
        .await
    }

    /// A complete sign-in in a browser that holds nothing.
    async fn sign_in(&self) -> (u16, Json) {
        let round = self.begin(None, json!({})).await;
        self.finish(&round, Some(SITE), None).await
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

    async fn audit(&self) -> Json {
        let (_, platform) = self
            .call_at(
                None,
                "fieldnotes",
                None,
                "/internal/platform/diagnostics",
                json!({}),
            )
            .await;
        // Security events and authentication decisions are kept in separate rings.
        let mut entries = platform["audit"].as_array().cloned().unwrap_or_default();
        entries.extend(
            platform["decisions"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
        Json::Array(entries)
    }
}

fn roles(reply: &Json) -> Vec<String> {
    let mut roles: Vec<String> = reply["session"]["roles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|role| role.as_str().unwrap().to_string())
        .collect();
    roles.sort();
    roles
}

fn principal(reply: &Json) -> String {
    reply["session"]["principal"].as_str().unwrap().to_string()
}

fn assert_refused(reply: &(u16, Json), why: &str) {
    assert_eq!(reply.0, 401, "{why}: {}", reply.1);
    assert_eq!(reply.1["message"], REFUSAL, "{why}");
    assert!(reply.1.get("token").is_none(), "{why}");
}

/// A change to what the provider says, for a table of them.
type Change = Box<dyn Fn(&mut mock_oidc::Tweaks)>;

fn change(f: impl Fn(&mut mock_oidc::Tweaks) + 'static) -> Change {
    Box::new(f)
}

fn s256(verifier: &str) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

// ---------------------------------------------------------------------------------------------
// The flow (oidc-valid; FED-002, FED-010, FED-011, FED-258)

/// Verifies: FED-002, FED-010, FED-011, FED-258
///
/// Case oidc-valid: a callback with the pending state and a token that verifies signs the person
/// in. The request that went to the provider carried a state, a nonce and a proof-key challenge
/// of method S256 and the redirect address of this site; the code was redeemed with the verifier
/// that hashes to that challenge and with the client secret. A first sign-in made a principal
/// that holds the one default role, and a second one finds the same principal.
#[tokio::test]
async fn oidc_valid_signs_the_person_in_with_a_proof_key_and_makes_them_once() {
    let mock = Mock::start().await;
    let config = federation(vec![provider(&mock, |_| {})], Some(0));
    assert!(
        !format!("{config:?}").contains(CLIENT_SECRET),
        "the configuration does not print its secret"
    );
    let backend = start(config).await;

    // The page offers the provider the realm configured.
    let (status, offered) = backend
        .call_at(
            None,
            "fieldnotes",
            None,
            "/internal/sign-in/oidc/providers",
            json!({}),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        offered,
        json!({ "providers": [{ "id": "acme", "label": "Acme" }] })
    );
    // Another site of the platform has none.
    let (_, none) = backend
        .call_at(
            None,
            "tidewater",
            None,
            "/internal/sign-in/oidc/providers",
            json!({}),
        )
        .await;
    assert_eq!(none, json!({ "providers": [] }));

    let round = backend.begin(None, json!({})).await;
    let asked = mock.authorizations();
    assert_eq!(asked.len(), 1);
    let ask = &asked[0];
    assert_eq!(ask["response_type"], "code");
    assert_eq!(ask["client_id"], CLIENT_ID);
    assert_eq!(ask["redirect_uri"], SITE_CALLBACK);
    assert_eq!(ask["code_challenge_method"], "S256");
    assert_eq!(ask["state"], round.state);
    assert_eq!(round.state.len(), 64);
    assert!(!ask["nonce"].is_empty());
    assert!(ask["scope"].split(' ').any(|scope| scope == "openid"));
    assert_ne!(ask["state"], ask["nonce"]);

    let (status, reply) = backend.finish(&round, Some(SITE), None).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["session"]["authenticated"], true);
    assert_eq!(reply["session"]["display_name"], "Pat Person");
    assert_eq!(
        roles(&reply),
        ["author", "public"],
        "only the default role the realm names"
    );
    let first = principal(&reply);
    assert!(first.starts_with("user:oidc-"), "{first}");
    let token = reply["token"].as_str().unwrap();
    let (_, session) = backend
        .call_at(
            Some(SITE),
            "fieldnotes",
            Some(token),
            "/internal/session",
            json!({}),
        )
        .await;
    assert_eq!(session["session"]["authenticated"], true);

    let redeemed = mock.token_requests();
    assert_eq!(redeemed.len(), 1);
    assert_eq!(redeemed[0]["redirect_uri"], SITE_CALLBACK);
    assert_eq!(
        s256(&redeemed[0]["code_verifier"]),
        ask["code_challenge"],
        "the code was redeemed with the verifier of the challenge"
    );
    assert_eq!(redeemed[0]["client_secret"], CLIENT_SECRET);

    // The person's name at the provider changes; the site keeps its own, and the principal is
    // the one it made.
    mock.set(|who| who.name = Some("Renamed Elsewhere".into()));
    let (status, again) = backend.sign_in().await;
    assert_eq!(status, 200, "{again}");
    assert_eq!(principal(&again), first);
    assert_eq!(again["session"]["display_name"], "Pat Person");

    let audit = backend.audit().await;
    assert!(audit.as_array().unwrap().iter().any(|entry| {
        entry["event"] == "auth.decision"
            && entry["method"] == "oidc"
            && entry["outcome"] == "accepted"
    }));
    let trail = audit.to_string();
    for secret in [
        round.state.as_str(),
        round.code.as_str(),
        round.binding.as_str(),
        CLIENT_SECRET,
        redeemed[0]["code_verifier"].as_str(),
    ] {
        assert!(!trail.contains(secret), "the audit trail holds {secret}");
    }
    assert!(
        !trail.contains("person@bound.example"),
        "no address in the trail"
    );
}

// ---------------------------------------------------------------------------------------------
// State, binding and replay (oidc-wrong-state; FED-001)

/// Verifies: FED-001
///
/// A pending request lapses: one that waits past its lifetime is not answered.
#[tokio::test]
async fn a_pending_request_lapses() {
    let mock = Mock::start().await;
    let backend = start_with(federation(vec![provider(&mock, |_| {})], Some(0)), |cfg| {
        cfg.passkey_challenge_ttl_ms = 150
    })
    .await;
    let round = backend.begin(None, json!({})).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_refused(&backend.finish(&round, Some(SITE), None).await, "lapsed");
}

// ---------------------------------------------------------------------------------------------
// Tokens (oidc-wrong-audience, oidc-wrong-nonce; FED-003, FED-255, FED-256)

/// Verifies: FED-003, FED-255
///
/// Cases oidc-wrong-audience and oidc-wrong-nonce, and the rest of what a token is refused for:
/// an audience that is not this client, an authorised party that is not, a nonce that is not the
/// one asked for or is missing, an expired token, one issued in the future, another issuer, no
/// subject, `alg: none`, a shared-secret method, a method the configuration does not name and a
/// key nobody publishes. Every refusal says the same, and the provider still signs people in
/// afterwards.
#[tokio::test]
async fn a_token_that_is_wrong_in_any_way_signs_no_one_in() {
    let mock = Mock::start().await;
    mock.publish_es();
    // The configuration names Ed25519 alone, though the provider publishes a P-256 key too.
    let backend = start(federation(vec![provider(&mock, |_| {})], Some(0))).await;

    let cases: Vec<(&str, Change)> = vec![
        (
            "wrong audience",
            change(|t| t.aud = Some(json!("another-client"))),
        ),
        (
            "audience list without this client",
            change(|t| t.aud = Some(json!(["a", "b"]))),
        ),
        (
            "several audiences and no authorised party",
            change(|t| t.aud = Some(json!([CLIENT_ID, "b"]))),
        ),
        (
            "authorised party that is not this client",
            change(|t| t.azp = Some("other".into())),
        ),
        (
            "wrong nonce",
            change(|t| t.nonce = Some("a-nonce-nobody-asked-for".into())),
        ),
        ("no nonce", change(|t| t.nonce = Some(String::new()))),
        ("expired", change(|t| t.exp_offset = -3600)),
        (
            "issued in the future",
            change(|t| {
                t.iat_offset = 3600;
                t.exp_offset = 7200;
            }),
        ),
        (
            "another issuer",
            change(|t| t.issuer = Some("https://evil.example".into())),
        ),
        ("no subject", change(|t| t.no_sub = true)),
        ("alg none", change(|t| t.alg_none = true)),
        ("a shared-secret method", change(|t| t.hs256 = true)),
        (
            "a method the configuration does not name",
            change(|t| t.signer = Signer::Es),
        ),
        (
            "a key nobody publishes",
            change(|t| t.signer = Signer::Ghost),
        ),
    ];
    for (why, apply) in &cases {
        mock.reset();
        mock.set(|t| apply(t));
        let round = backend.begin(None, json!({})).await;
        let reply = backend.finish(&round, Some(SITE), None).await;
        assert_refused(&reply, why);
    }

    // A provider that was refused is not shut out: the right token signs in.
    mock.reset();
    let (status, reply) = backend.sign_in().await;
    assert_eq!(status, 200, "{reply}");

    // Each refusal is in the audit trail as a refusal of the method, and nothing of the
    // token is.
    let audit = backend.audit().await;
    let refused = audit
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["event"] == "auth.decision" && e["method"] == "oidc" && e["outcome"] == "refused"
        })
        .count();
    assert_eq!(refused, cases.len());
}

/// Verifies: FED-256
///
/// A key of one provider verifies the tokens of that provider only. Two providers of one realm are
/// both configured for P-256; the second publishes a key, and a token signed with it is accepted
/// from the second; the first does not publish it, so the same token from the first is refused,
/// though the keys of the second are held by the same process.
#[tokio::test]
async fn a_providers_key_does_not_verify_another_providers_tokens() {
    let mock = Mock::start().await;
    let other = Mock::start().await;
    other.publish_es();
    let both = |p: &mut ProviderConfig| p.algorithms = vec![Algorithm::EdDSA, Algorithm::ES256];
    let backend = start(federation(
        vec![
            provider(&mock, both),
            provider(&other, |p| {
                both(p);
                p.id = "other".into();
                p.label = "Other".into();
            }),
        ],
        Some(0),
    ))
    .await;

    other.set(|t| t.signer = Signer::Es);
    let round = backend.begin(None, json!({ "provider": "other" })).await;
    let (status, reply) = backend.finish(&round, Some(SITE), None).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(other.jwks_fetches(), 1, "its keys are held now");

    mock.set(|t| t.signer = Signer::Es);
    let round = backend.begin(None, json!({})).await;
    assert_refused(
        &backend.finish(&round, Some(SITE), None).await,
        "a key the second provider publishes, at the first",
    );
    assert!(
        other.token_requests().len() == 1,
        "the second was not asked again"
    );
}

/// Verifies: FED-004
///
/// A token that names a key that is not held makes the provider's keys be fetched once more, and
/// is accepted when the key is there now (a rotation) and refused when it is not.
#[tokio::test]
async fn a_token_naming_an_unknown_key_fetches_the_keys_once() {
    let mock = Mock::start().await;
    let backend = start(federation(
        vec![provider(&mock, |p| {
            p.algorithms = vec![Algorithm::EdDSA, Algorithm::ES256]
        })],
        Some(0),
    ))
    .await;

    let (status, reply) = backend.sign_in().await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(
        mock.jwks_fetches(),
        1,
        "the keys are fetched when first needed"
    );
    let (status, _) = backend.sign_in().await;
    assert_eq!(status, 200);
    assert_eq!(
        mock.jwks_fetches(),
        1,
        "and are not fetched again for a key that is held"
    );

    // The provider rotates: a token names a key that is published now and was not.
    mock.publish_es();
    mock.set(|t| t.signer = Signer::Es);
    let (status, reply) = backend.sign_in().await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(
        mock.jwks_fetches(),
        2,
        "one refresh, and the token verifies"
    );

    // A key that is not there after a refresh is a refusal, after one refresh and not more.
    mock.set(|t| t.signer = Signer::Ghost);
    let round = backend.begin(None, json!({})).await;
    assert_refused(
        &backend.finish(&round, Some(SITE), None).await,
        "a ghost key",
    );
    assert_eq!(
        mock.jwks_fetches(),
        3,
        "exactly one refresh for the unknown key"
    );
}

// ---------------------------------------------------------------------------------------------
// Starting, return addresses, a provider that is off or does not answer (FED-011, FED-013, FED-250)

/// Verifies: FED-013
///
/// A provider that takes a request and never answers it holds only the callbacks that wait on it:
/// with more of them waiting than there are places to hash a password, password sign-in still
/// answers, each time, because the wait on the provider holds no hashing place.
#[tokio::test]
async fn a_provider_that_hangs_does_not_block_password_sign_in() {
    let mock = Mock::start().await;
    let backend = start(federation(vec![provider(&mock, |_| {})], Some(0))).await;
    let mut rounds = Vec::new();
    // More callbacks than the per-tenant hashing cap (two) is the smallest count that shows it.
    for _ in 0..3 {
        rounds.push(backend.begin(None, json!({})).await);
    }
    mock.hang();
    let waiting = futures_util::future::join_all(
        rounds
            .iter()
            .map(|round| backend.finish(round, Some(SITE), None)),
    );
    let password = async {
        for _ in 0..2 {
            assert_eq!(backend.password("ines", "strata-dev-ines").await.0, 200);
        }
    };
    let outcome = tokio::time::timeout(Duration::from_secs(8), async {
        tokio::select! {
            () = password => true,
            _ = waiting => false,
        }
    })
    .await;
    assert_eq!(
        outcome,
        Ok(true),
        "password sign-in answered while three callbacks waited on a provider that never does"
    );
}

// ---------------------------------------------------------------------------------------------
// Policy and linking (FED-009, FED-011, FED-012, FED-257, FED-258)

/// Verifies: FED-257
///
/// A provider that asserts a verified address outside its own domains links to no one, however
/// the address matches an account's login.
#[tokio::test]
async fn a_verified_address_outside_the_providers_domains_links_to_no_one() {
    let mock = Mock::start().await;
    mock.set(|t| {
        t.sub = "someone-claiming-mirela".into();
        t.email = Some("mirela@fieldnotes.example".into());
        t.email_verified = true;
    });
    let backend = start(federation(
        vec![provider(&mock, |p| {
            p.link_verified_email = true;
            p.create_on_first_sign_in = false;
            p.domains = vec!["bound.example".into()];
        })],
        Some(0),
    ))
    .await;
    assert_refused(
        &backend.sign_in().await,
        "an address outside the provider's domains",
    );
    let tenant = TenantId::new("fieldnotes").unwrap();
    assert!(backend
        .store
        .identity(&tenant, "acme", "someone-claiming-mirela")
        .unwrap()
        .is_none());
}

/// Verifies: IDN-019
///
/// Linking an identity to a signed-in account needs a recent authentication, at the start and at
/// the callback.
#[tokio::test]
async fn linking_an_identity_needs_a_recent_authentication() {
    let mock = Mock::start().await;
    mock.set(|t| {
        t.sub = "ines-at-the-provider".into();
        t.email = Some("someone-else@elsewhere.example".into());
    });
    let backend = start(federation(
        vec![provider(&mock, |p| p.create_on_first_sign_in = false)],
        Some(0),
    ))
    .await;
    let ines = backend.session_of("ines").await;
    let tenant = TenantId::new("fieldnotes").unwrap();

    // At the start: a session that authenticated long ago is asked to confirm first.
    let started_fresh = backend.begin(Some(&ines), json!({ "link": true })).await;
    backend.age(&ines, 16 * 60_000);
    let (status, reply) = backend
        .start_at(
            Some(SITE),
            Some(&ines),
            json!({ "provider": "acme", "link": true }),
        )
        .await;
    assert_eq!(status, 403, "{reply}");
    assert_eq!(reply["kind"], "step_up_required");

    // At the callback: the request was made fresh, and the session has gone stale since.
    assert_refused(
        &backend
            .finish(&started_fresh, Some(SITE), Some(&ines))
            .await,
        "a session that is no longer fresh",
    );
    assert!(backend
        .store
        .identity(&tenant, "acme", "ines-at-the-provider")
        .unwrap()
        .is_none());
}

/// Verifies: FED-012
///
/// An identity is linked to an existing principal after that principal has signed in and asked to:
/// the callback answers only to the person who started it, the identity then signs that principal
/// in, and an identity that belongs to one principal cannot be linked to another.
#[tokio::test]
async fn an_identity_is_linked_only_by_the_principal_that_signed_in_first() {
    let mock = Mock::start().await;
    mock.set(|t| {
        t.sub = "ines-at-the-provider".into();
        t.email = Some("someone-else@elsewhere.example".into());
    });
    let backend = start(federation(
        vec![provider(&mock, |p| p.create_on_first_sign_in = false)],
        Some(0),
    ))
    .await;

    // Not signed in: asking to link is refused at the start.
    let (status, _) = backend
        .start_at(
            Some(SITE),
            None,
            json!({ "provider": "acme", "link": true }),
        )
        .await;
    assert_eq!(status, 401);
    // Not known, and no creation: the provider alone signs no one in.
    assert_refused(&backend.sign_in().await, "an identity no one linked");

    let ines = backend.session_of("ines").await;
    let round = backend.begin(Some(&ines), json!({ "link": true })).await;
    // The callback of a request to link answers to the person who made it, and no other.
    let tomasz = backend.session_of("tomasz").await;
    assert_refused(
        &backend.finish(&round, Some(SITE), Some(&tomasz)).await,
        "another person's session",
    );
    let round = backend.begin(Some(&ines), json!({ "link": true })).await;
    assert_refused(
        &backend.finish(&round, Some(SITE), None).await,
        "no session",
    );
    let round = backend.begin(Some(&ines), json!({ "link": true })).await;
    let (status, reply) = backend.finish(&round, Some(SITE), Some(&ines)).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(principal(&reply), "user:ines");

    // From then on the identity alone signs Inès in, in a browser that holds nothing.
    let (status, reply) = backend.sign_in().await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(principal(&reply), "user:ines");

    // Tomasz cannot link what is Inès's.
    let round = backend.begin(Some(&tomasz), json!({ "link": true })).await;
    assert_refused(
        &backend.finish(&round, Some(SITE), Some(&tomasz)).await,
        "an identity of another principal",
    );
}
