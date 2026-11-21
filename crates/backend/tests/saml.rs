//! Sign-in at a SAML provider, against a mock provider that makes and signs responses the way a
//! provider does (`support/mock_saml.rs`): the flow with its pending request, every refusal of the
//! sign-in table's `saml-*` rows and of the requirements behind them, the attacks on the signature
//! (a response with no signature on its assertion, a wrong signer, a wrapped or a tampered
//! assertion), replay, time, audience and address, and the realm's policy for who gets in, which
//! is the one OpenID Connect uses. Real sockets, real bundles, the in-memory store, and the real
//! XML-DSig verification.

#[path = "support/mock_saml.rs"]
mod mock_saml;

use mock_saml::{
    encode, now, read_request, signature_of, Reply, Request, IDP_ENTITY, IDP_KEY, OTHER_CERT,
    OTHER_KEY, SAML_NS, SSO_URL,
};
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strata_backend::app::Config;
use strata_backend::mail::{DevSink, Mailer};
use strata_backend::signin::oidc::Federation;
use strata_backend::signin::saml::SamlProvider;
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
/// This service's assertion consumer address and entity identifier at that address.
const ACS: &str = "https://fieldnotes.example/federation/saml/acs";
const ENTITY: &str = "https://fieldnotes.example/federation/saml/metadata/acme";
/// What every refusal of the method says, whatever was wrong.
const REFUSAL: &str = "The sign-in with the identity provider was not accepted.";

struct Backend {
    base: String,
    http: reqwest::Client,
    store: Arc<MemoryStore>,
    sink: Arc<DevSink>,
}

/// A provider as a realm would configure it: signing with the test certificate, vouching for the
/// addresses it sends, creating people on a first sign-in as authors.
fn provider(adjust: impl FnOnce(&mut SamlProvider)) -> SamlProvider {
    let mut provider = SamlProvider {
        id: "acme".into(),
        label: "Acme".into(),
        enabled: true,
        idp_entity_id: IDP_ENTITY.into(),
        sso_url: SSO_URL.into(),
        certificates: vec![mock_saml::IDP_CERT.into()],
        sp_entity_id: None,
        email_attribute: "email".into(),
        name_attribute: "name".into(),
        email_verified: true,
        skew_secs: 60,
        domains: Vec::new(),
        create_on_first_sign_in: true,
        default_role: Some("author".into()),
        link_verified_email: false,
    };
    adjust(&mut provider);
    provider
}

fn federation(providers: Vec<SamlProvider>) -> Federation {
    Federation::none().with_saml(BTreeMap::from([(
        TenantId::new("fieldnotes").unwrap(),
        providers,
    )]))
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

/// One sign-in on its way: what the browser holds and what the provider read.
struct Round {
    /// The authentication request the provider read.
    request: Request,
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
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header(HEADER_SERVICE, TOKEN)
            .header(HEADER_SITE, site)
            .header(HEADER_SOURCE, SOURCE)
            .header(HEADER_CORRELATION, "t-corr-saml")
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
            "/internal/sign-in/saml/start",
            body,
        )
        .await
    }

    /// The browser's part up to the provider: start, and read what the provider reads.
    async fn begin(&self, session: Option<&str>, extra: Json) -> Round {
        let mut body = json!({ "provider": "acme" });
        for (name, value) in extra.as_object().cloned().unwrap_or_default() {
            body[name] = value;
        }
        let (status, started) = self.start_at(Some(SITE), session, body).await;
        assert_eq!(status, 200, "{started}");
        let binding = started["binding"].as_str().unwrap().to_string();
        assert_eq!(binding.len(), 64);
        let request = read_request(started["redirect"].as_str().unwrap());
        Round { request, binding }
    }

    /// The provider's response, posted to the page and sent on by it: from the browser that holds
    /// `binding`, at `origin`, under `relay_state`.
    async fn deliver(
        &self,
        relay_state: &str,
        xml: &str,
        binding: &str,
        origin: Option<&str>,
        session: Option<&str>,
    ) -> (u16, Json) {
        self.call_at(
            origin,
            "fieldnotes",
            session,
            "/internal/session/sign-in/saml",
            json!({ "relay_state": relay_state, "response": encode(xml), "binding": binding }),
        )
        .await
    }

    /// The round's own response, as the browser sends it.
    async fn finish(&self, round: &Round, xml: &str) -> (u16, Json) {
        self.deliver(
            &round.request.relay_state,
            xml,
            &round.binding,
            Some(SITE),
            None,
        )
        .await
    }

    /// A complete sign-in in a browser that holds nothing, with the reply changed as given.
    async fn sign_in_with(&self, change: impl FnOnce(&mut Reply)) -> (u16, Json) {
        let round = self.begin(None, json!({})).await;
        let mut reply = Reply::to(&round.request, ENTITY);
        change(&mut reply);
        self.finish(&round, &reply.response()).await
    }

    async fn sign_in(&self) -> (u16, Json) {
        self.sign_in_with(|_| {}).await
    }

    /// A round that is answered with `make`'s message, which is expected to be refused.
    async fn refused(&self, why: &str, make: impl FnOnce(&Round) -> String) {
        let round = self.begin(None, json!({})).await;
        let xml = make(&round);
        assert_refused(&self.finish(&round, &xml).await, why);
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

/// The reply of a correct response, for a round, made by the provider.
fn good(round: &Round) -> Reply {
    Reply::to(&round.request, ENTITY)
}

// ---------------------------------------------------------------------------------------------
// The flow (saml-valid; FED-005, FED-007, FED-010, FED-011, FED-258)

/// Verifies: FED-005, FED-007, FED-010, FED-011, FED-258
///
/// Case saml-valid: a response to a pending request, whose assertion is signed by the configured
/// certificate, signs the person in. The request that went to the provider named this service and
/// the assertion consumer address of this site, and asked for the response to be posted; a first
/// sign-in made a principal that holds the one default role, and a second one finds the same
/// principal.
#[tokio::test]
async fn saml_valid_signs_the_person_in_and_makes_them_once() {
    let backend = start(federation(vec![provider(|_| {})])).await;

    // The page offers the provider the realm configured, and no other realm's.
    let (status, offered) = backend
        .call_at(
            None,
            "fieldnotes",
            None,
            "/internal/sign-in/saml/providers",
            json!({}),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        offered,
        json!({ "providers": [{ "id": "acme", "label": "Acme" }] })
    );
    let (_, none) = backend
        .call_at(
            None,
            "tidewater",
            None,
            "/internal/sign-in/saml/providers",
            json!({}),
        )
        .await;
    assert_eq!(none, json!({ "providers": [] }));

    let round = backend.begin(None, json!({})).await;
    let asked = &round.request;
    assert!(
        asked.id.starts_with('_') && asked.id.len() > 20,
        "{}",
        asked.id
    );
    assert_eq!(asked.issuer, ENTITY, "the request names this service");
    assert_eq!(asked.destination, SSO_URL);
    assert_eq!(asked.acs_url, ACS);
    assert_eq!(
        asked.protocol_binding,
        "urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST"
    );
    assert_eq!(asked.relay_state.len(), 64);
    assert_ne!(asked.relay_state, round.binding);

    let (status, reply) = backend.finish(&round, &good(&round).response()).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["session"]["authenticated"], true);
    assert_eq!(reply["session"]["display_name"], "Pat Person");
    assert_eq!(
        roles(&reply),
        ["author", "public"],
        "only the default role the realm names"
    );
    let first = principal(&reply);
    assert!(first.starts_with("user:saml-"), "{first}");
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

    // The person's name at the provider changes; the site keeps its own, and the principal is the
    // one it made.
    let (status, again) = backend
        .sign_in_with(|reply| reply.name = Some("Renamed Elsewhere".into()))
        .await;
    assert_eq!(status, 200, "{again}");
    assert_eq!(principal(&again), first);
    assert_eq!(again["session"]["display_name"], "Pat Person");

    let audit = backend.audit().await;
    assert!(audit.as_array().unwrap().iter().any(|entry| {
        entry["event"] == "auth.decision"
            && entry["method"] == "saml"
            && entry["outcome"] == "accepted"
    }));
    let trail = audit.to_string();
    for secret in [
        asked.relay_state.as_str(),
        round.binding.as_str(),
        asked.id.as_str(),
    ] {
        assert!(!trail.contains(secret), "the audit trail holds {secret}");
    }
    assert!(
        !trail.contains("pat@bound.example"),
        "no address in the trail"
    );
}

// ---------------------------------------------------------------------------------------------
// The signature (saml-unsigned; FED-005, FED-252)

/// Verifies: FED-252
///
/// Signature wrapping: the subject and the attributes come only from the assertion whose signature
/// covers them. A second assertion beside the signed one, one put where the signed one is
/// expected while the signed one is moved elsewhere, a signature moved onto another assertion and
/// an assertion changed after it was signed are all refused, whichever way round the two are.
#[tokio::test]
async fn a_wrapped_or_tampered_assertion_is_refused() {
    let backend = start(federation(vec![provider(|_| {})])).await;

    let evil = |round: &Round| {
        let mut evil = good(round);
        evil.assertion_id = "_evil-assertion".into();
        evil.name_id = "victim".into();
        evil
    };

    backend
        .refused("an unsigned assertion after the signed one", |round| {
            let genuine = good(round);
            let content = format!(
                "{}{}",
                genuine.signed_assertion(IDP_KEY),
                evil(round).assertion(false)
            );
            genuine.response_around(&content, false)
        })
        .await;
    backend
        .refused("an unsigned assertion before the signed one", |round| {
            let genuine = good(round);
            let content = format!(
                "{}{}",
                evil(round).assertion(false),
                genuine.signed_assertion(IDP_KEY)
            );
            genuine.response_around(&content, false)
        })
        .await;
    backend
        .refused("the signed assertion moved into extensions", |round| {
            let genuine = good(round);
            let content = format!(
                "<samlp:Extensions>{}</samlp:Extensions>{}",
                genuine.signed_assertion(IDP_KEY),
                evil(round).assertion(false)
            );
            genuine.response_around(&content, false)
        })
        .await;
    backend
        .refused("the signed assertion moved into the evil one", |round| {
            let genuine = good(round);
            let signed = genuine.signed_assertion(IDP_KEY);
            let wrapped = evil(round).assertion(false).replacen(
                "</saml:Issuer>",
                &format!("</saml:Issuer><saml:Advice>{signed}</saml:Advice>"),
                1,
            );
            genuine.response_around(&wrapped, false)
        })
        .await;
    backend
        .refused("a signature moved onto another assertion", |round| {
            let genuine = good(round);
            let signature = signature_of(&genuine.signed_assertion(IDP_KEY));
            let carried = evil(round).assertion(false).replacen(
                "</saml:Issuer>",
                &format!("</saml:Issuer>{signature}"),
                1,
            );
            genuine.response_around(&carried, false)
        })
        .await;
    backend
        .refused(
            "the signature of another assertion under this one's name",
            |round| {
                // The evil assertion takes the signed one's identifier, so that the reference of the
                // moved signature resolves to it; the digest it holds is of the other text.
                let genuine = good(round);
                let signature = signature_of(&genuine.signed_assertion(IDP_KEY));
                let mut forged = evil(round);
                forged.assertion_id = genuine.assertion_id.clone();
                let carried = forged.assertion(false).replacen(
                    "</saml:Issuer>",
                    &format!("</saml:Issuer>{signature}"),
                    1,
                );
                genuine.response_around(&carried, false)
            },
        )
        .await;
    backend
        .refused("a name changed after signing", |round| {
            let genuine = good(round);
            let signed = genuine
                .signed_assertion(IDP_KEY)
                .replace(">subject-1<", ">victim<");
            genuine.response_around(&signed, false)
        })
        .await;
    backend
        .refused("an audience changed after signing", |round| {
            let mut genuine = good(round);
            genuine.audience = "https://evil.example/".into();
            let signed = genuine
                .signed_assertion(IDP_KEY)
                .replace("https://evil.example/", ENTITY);
            genuine.response_around(&signed, false)
        })
        .await;

    // Nothing of the above was left behind: the genuine assertion alone still signs in.
    let (status, reply) = backend.sign_in().await;
    assert_eq!(status, 200, "{reply}");
    assert!(
        backend
            .store
            .identity(&TenantId::new("fieldnotes").unwrap(), "acme", "victim")
            .unwrap()
            .is_none(),
        "no identity was made from a wrapped assertion"
    );
}

/// Verifies: FED-251
///
/// A message with a document type declaration is not read, and so an entity it names is not
/// resolved; a message that is not XML, not base64 or too large is refused like the rest, and an
/// encrypted assertion, which this service cannot read, is not taken for a signed one.
#[tokio::test]
async fn a_message_with_a_document_type_or_the_wrong_shape_is_refused() {
    let backend = start(federation(vec![provider(|_| {})])).await;

    backend
        .refused("an external entity", |round| {
            format!(
                "<!DOCTYPE r [<!ENTITY xxe SYSTEM \"file:///etc/passwd\">]>{}",
                good(round).response()
            )
        })
        .await;
    backend
        .refused("an internal entity, which is no better", |round| {
            format!("<!DOCTYPE r [<!ENTITY e \"x\">]>{}", good(round).response())
        })
        .await;
    backend
        .refused("not XML", |_| "this is not a response".into())
        .await;
    backend
        .refused("another document", |_| {
            "<html><body>hi</body></html>".into()
        })
        .await;
    backend
        .refused("an encrypted assertion", |round| {
            good(round).response_around(
                &format!(
                    "<saml:EncryptedAssertion xmlns:saml=\"{SAML_NS}\">\
                     <EncryptedData xmlns=\"http://www.w3.org/2001/04/xmlenc#\"/>\
                     </saml:EncryptedAssertion>"
                ),
                false,
            )
        })
        .await;
    backend
        .refused("a response with no assertion", |round| {
            good(round).response_around("", false)
        })
        .await;
    backend
        .refused("a message past the size limit", |round| {
            let mut reply = good(round);
            reply.name = Some("x".repeat(300 * 1024));
            reply.response()
        })
        .await;

    // Not base64 at all.
    let round = backend.begin(None, json!({})).await;
    let (status, reply) = backend
        .call_at(
            Some(SITE),
            "fieldnotes",
            None,
            "/internal/session/sign-in/saml",
            json!({
                "relay_state": round.request.relay_state,
                "response": "***not base64***",
                "binding": round.binding,
            }),
        )
        .await;
    assert_refused(&(status, reply), "not base64");
}

// ---------------------------------------------------------------------------------------------
// Replay (saml-replayed; FED-006)

/// Verifies: FED-006
///
/// Case saml-replayed: an assertion presented a second time is refused, whether the whole message
/// is presented again or the provider's own assertion identifier comes back in an answer to
/// another request; an assertion with an identifier of its own is not refused.
#[tokio::test]
async fn saml_replayed_assertions_are_refused() {
    let backend = start(federation(vec![provider(|_| {})])).await;

    // The same message again: the request it answered was taken the first time.
    let round = backend.begin(None, json!({})).await;
    let xml = good(&round).response();
    assert_eq!(backend.finish(&round, &xml).await.0, 200);
    assert_refused(
        &backend.finish(&round, &xml).await,
        "the same message again",
    );

    // The same message answering a new request is not an answer to it either.
    let next = backend.begin(None, json!({})).await;
    assert_refused(
        &backend.finish(&next, &xml).await,
        "an answer to the earlier request",
    );

    // The same assertion identifier in answers to two requests, each of them correct: the second
    // is the replay the identifier is remembered for.
    let identifier = format!("_the-same-assertion-{}", now());
    let first = backend.begin(None, json!({})).await;
    let mut reply = good(&first);
    reply.assertion_id = identifier.clone();
    assert_eq!(backend.finish(&first, &reply.response()).await.0, 200);
    let second = backend.begin(None, json!({})).await;
    let mut reply = good(&second);
    reply.assertion_id = identifier.clone();
    assert_refused(
        &backend.finish(&second, &reply.response()).await,
        "an identifier that was accepted",
    );
    // And the one with an identifier of its own is accepted: the refusal was for the identifier.
    let third = backend.begin(None, json!({})).await;
    assert_eq!(
        backend.finish(&third, &good(&third).response()).await.0,
        200
    );

    // A message that was refused did not use its identifier up.
    let fourth = backend.begin(None, json!({})).await;
    let mut wrong = good(&fourth);
    wrong.assertion_id = "_not-used-up".into();
    wrong.audience = "https://another.example/".into();
    assert_refused(
        &backend.finish(&fourth, &wrong.response()).await,
        "the wrong audience",
    );
    let fifth = backend.begin(None, json!({})).await;
    let mut right = good(&fifth);
    right.assertion_id = "_not-used-up".into();
    assert_eq!(backend.finish(&fifth, &right.response()).await.0, 200);

    // Another provider's identifiers are its own: one identifier at two providers is two.
    let two = start(federation(vec![
        provider(|_| {}),
        provider(|p| {
            p.id = "second".into();
            p.label = "Second".into();
        }),
    ]))
    .await;
    let round = two.begin(None, json!({})).await;
    let mut reply = good(&round);
    reply.assertion_id = "_shared-identifier".into();
    assert_eq!(two.finish(&round, &reply.response()).await.0, 200);
    let (status, started) = two
        .start_at(Some(SITE), None, json!({ "provider": "second" }))
        .await;
    assert_eq!(status, 200, "{started}");
    let request = read_request(started["redirect"].as_str().unwrap());
    let mut reply = Reply::to(
        &request,
        "https://fieldnotes.example/federation/saml/metadata/second",
    );
    reply.assertion_id = "_shared-identifier".into();
    let (status, accepted) = two
        .deliver(
            &request.relay_state,
            &reply.response(),
            started["binding"].as_str().unwrap(),
            Some(SITE),
            None,
        )
        .await;
    assert_eq!(status, 200, "{accepted}");
}

// ---------------------------------------------------------------------------------------------
// What the assertion says (FED-007, FED-008, FED-254)

/// Verifies: FED-008
///
/// An audience that is not this service, a recipient or a destination that is not its assertion
/// consumer address (another host of the platform's among them), and no audience at all are
/// refused.
#[tokio::test]
async fn an_assertion_for_another_service_or_address_is_refused() {
    let backend = start(federation(vec![provider(|_| {})])).await;
    let cases: Vec<(&str, Box<dyn Fn(&mut Reply)>)> = vec![
        (
            "another audience",
            Box::new(|r| r.audience = "https://other.example/".into()),
        ),
        (
            "another site's entity identifier",
            Box::new(|r| {
                r.audience = "https://tidewater.example/federation/saml/metadata/acme".into()
            }),
        ),
        (
            "the audience with something added",
            Box::new(|r| r.audience = format!("{ENTITY}/")),
        ),
        (
            "another recipient",
            Box::new(|r| r.recipient = "https://evil.example/federation/saml/acs".into()),
        ),
        (
            "another site's recipient",
            Box::new(|r| r.recipient = "https://tidewater.example/federation/saml/acs".into()),
        ),
        (
            "another destination",
            Box::new(|r| r.destination = Some("https://evil.example/federation/saml/acs".into())),
        ),
    ];
    for (why, change) in &cases {
        let reply = backend.sign_in_with(|r| change(r)).await;
        assert_refused(&reply, why);
    }
    // A response with no destination is the provider's choice; the recipient is still checked.
    assert_eq!(backend.sign_in_with(|r| r.destination = None).await.0, 200);
}

/// Verifies: FED-254
///
/// An assertion from an issuer other than the configured one is refused, on the assertion and on
/// the response, and a response that reports a failure is refused.
#[tokio::test]
async fn an_assertion_from_another_issuer_is_refused() {
    let backend = start(federation(vec![provider(|_| {})])).await;
    let cases: Vec<(&str, Box<dyn Fn(&mut Reply)>)> = vec![
        (
            "another issuer on the assertion",
            Box::new(|r| r.issuer = "https://idp.other.example/metadata".into()),
        ),
        (
            "the issuer with something added",
            Box::new(|r| r.issuer = format!("{IDP_ENTITY}/")),
        ),
        (
            "another issuer on the response",
            Box::new(|r| r.response_issuer = Some("https://idp.other.example/metadata".into())),
        ),
        (
            "a failure status",
            Box::new(|r| r.status = "urn:oasis:names:tc:SAML:2.0:status:Responder".into()),
        ),
    ];
    for (why, change) in &cases {
        let reply = backend.sign_in_with(|r| change(r)).await;
        assert_refused(&reply, why);
    }
    // A provider that names no issuer on the response, only on the signed assertion, is fine.
    assert_eq!(
        backend.sign_in_with(|r| r.response_issuer = None).await.0,
        200
    );
}

// ---------------------------------------------------------------------------------------------
// Time (FED-005 and the validity of an assertion)

// ---------------------------------------------------------------------------------------------
// The subject (FED-253)

/// Verifies: FED-253
///
/// The subject is the complete text of the name identifier with comments left out: an identifier
/// with a comment in it is the person it spells with the comment taken out, and not the one its
/// first part spells. One with an element in it, or two identifiers, or an empty one, is refused.
#[tokio::test]
async fn the_subject_is_the_complete_text_of_the_name_identifier() {
    let backend = start(federation(vec![provider(|_| {})])).await;
    let (status, plain) = backend
        .sign_in_with(|r| r.name_id = "victim@acme.example".into())
        .await;
    assert_eq!(status, 200, "{plain}");
    let victim = principal(&plain);

    // The text before the comment is the victim; the text of the identifier is not.
    let (status, spliced) = backend
        .sign_in_with(|r| r.name_id = "victim@acme.example<!-- -->.evil.example".into())
        .await;
    assert_eq!(status, 200, "{spliced}");
    assert_ne!(
        principal(&spliced),
        victim,
        "a comment does not cut the identifier short"
    );
    let (_, spelled) = backend
        .sign_in_with(|r| r.name_id = "victim@acme.example.evil.example".into())
        .await;
    assert_eq!(
        principal(&spliced),
        principal(&spelled),
        "the comment is left out"
    );

    // Text on both sides of an element, a second identifier, an empty one.
    backend
        .refused("an element inside the identifier", |round| {
            let mut reply = good(round);
            reply.name_id = "victim<b>x</b>".into();
            reply.response()
        })
        .await;
    backend
        .refused("two identifiers", |round| {
            let mut reply = good(round);
            reply.name_id = "a</saml:NameID><saml:NameID>b".into();
            reply.response()
        })
        .await;
    let (status, empty) = backend.sign_in_with(|r| r.name_id = String::new()).await;
    assert_eq!(status, 401, "{empty}");
    let (status, blank) = backend.sign_in_with(|r| r.name_id = "   ".into()).await;
    assert_eq!(status, 401, "{blank}");
}

// ---------------------------------------------------------------------------------------------
// Starting a sign-in

// ---------------------------------------------------------------------------------------------
// Policy and linking (FED-009, FED-011, FED-012, FED-257, FED-258), shared with OpenID Connect

/// Verifies: FED-009
///
/// Case password-domain-bound, for a SAML provider: a correct password for an address in a domain
/// that an enabled SAML provider decides for is refused, and so is an emailed link for it; a login
/// in no such domain is not touched; a provider that is not enabled binds nothing.
#[tokio::test]
async fn a_password_for_a_saml_bound_domain_is_refused() {
    let backend = start(federation(vec![provider(|p| {
        p.domains = vec!["bound.example".into()]
    })]))
    .await;
    let (status, reply) = backend
        .password("ines@bound.example", "strata-dev-ines")
        .await;
    assert_eq!(status, 401, "{reply}");
    assert_eq!(reply["message"], "The name or password is not right.");
    assert_eq!(
        backend
            .password("Ines@BOUND.example", "strata-dev-ines")
            .await
            .0,
        401
    );
    assert_eq!(backend.password("ines", "strata-dev-ines").await.0, 200);

    let (status, _) = backend
        .call_at(
            Some(SITE),
            "fieldnotes",
            None,
            "/internal/sign-in/link/request",
            json!({ "email": "ines@bound.example" }),
        )
        .await;
    assert_eq!(status, 200);
    assert!(
        backend.sink.outbox().is_empty(),
        "no message for a bound address"
    );

    let off = start(federation(vec![provider(|p| {
        p.domains = vec!["bound.example".into()];
        p.enabled = false;
    })]))
    .await;
    assert_eq!(
        off.password("ines@bound.example", "strata-dev-ines")
            .await
            .0,
        200,
        "a provider that is not enabled binds nothing"
    );
}
