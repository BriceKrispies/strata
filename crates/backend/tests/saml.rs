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
    encode, instant, now, read_request, signature_of, Reply, Request, IDP_ENTITY, IDP_KEY,
    OTHER_CERT, OTHER_KEY, SAMLP_NS, SAML_NS, SSO_URL,
};
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strata_backend::app::{Config, ThrottleLimits};
use strata_backend::mail::{DevSink, Mailer};
use strata_backend::signin::oidc::Federation;
use strata_backend::signin::saml::SamlProvider;
use strata_kernel::TenantId;
use strata_protocol::api::{
    HEADER_CORRELATION, HEADER_ORIGIN, HEADER_SERVICE, HEADER_SESSION, HEADER_SITE, HEADER_SOURCE,
};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::Store;

const TOKEN: &str = "test-service-token-0123456789abcdef";
/// The address the tenant is reached at, in these tests.
const SITE: &str = "https://fieldnotes.example";
const ELSEWHERE: &str = "https://evil.fieldnotes.example";
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
        platform["audit"].clone()
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

/// Verifies: FED-005, FED-252
///
/// Case saml-unsigned and its kin: a response whose assertion has no signature is refused, and so
/// is one that is signed around an assertion that is not (the signature covers the response, and
/// the response holds an assertion nobody signed); an assertion signed by a certificate other than
/// the one configured is refused; and the provider's own signature still signs people in
/// afterwards.
#[tokio::test]
async fn saml_unsigned_and_wrongly_signed_assertions_are_refused() {
    let backend = start(federation(vec![provider(|_| {})])).await;

    backend
        .refused("no signature", |round| good(round).response_unsigned())
        .await;
    backend
        .refused("a signature on the response only", |round| {
            good(round).response_signed_around_unsigned_assertion()
        })
        .await;
    backend
        .refused("signed by another key", |round| {
            good(round).response_signed_by(OTHER_KEY)
        })
        .await;

    // A provider that says whose key it is does not make it so: the certificate in the message
    // is not looked at, and the other certificate is not configured.
    assert!(!OTHER_CERT.is_empty());
    backend
        .refused("signed by another key that names itself", |round| {
            let signed = good(round).signed_assertion(OTHER_KEY);
            let named = signed.replacen(
                "</ds:SignatureValue>",
                &format!(
                    "</ds:SignatureValue><ds:KeyInfo><ds:X509Data><ds:X509Certificate>{}\
                     </ds:X509Certificate></ds:X509Data></ds:KeyInfo>",
                    OTHER_CERT
                        .lines()
                        .filter(|line| !line.starts_with("-----"))
                        .collect::<String>()
                ),
                1,
            );
            good(round).response_around(&named, false)
        })
        .await;

    // None of that shut the provider out.
    let (status, reply) = backend.sign_in().await;
    assert_eq!(status, 200, "{reply}");

    // A certificate that is configured as well verifies: a rotation holds both, and a signature
    // by either of them is the provider's.
    let rotating = start(federation(vec![provider(|p| {
        p.certificates = vec![OTHER_CERT.into(), mock_saml::IDP_CERT.into()]
    })]))
    .await;
    assert_eq!(rotating.sign_in().await.0, 200);
    let round = rotating.begin(None, json!({})).await;
    let by_the_other = good(&round).response_signed_by(OTHER_KEY);
    assert_eq!(rotating.finish(&round, &by_the_other).await.0, 200);
}

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

/// Verifies: FED-007
///
/// Case saml-unsolicited: a response that answers no pending request is refused, however well it is
/// signed; one that answers another request than the pending one is refused, on the response and
/// in the assertion; and a request is answered once.
#[tokio::test]
async fn saml_unsolicited_responses_are_refused() {
    let backend = start(federation(vec![provider(|_| {})])).await;

    // A perfectly good response to a request nobody made, under a relay state nobody issued.
    let invented = backend.begin(None, json!({})).await;
    let xml = good(&invented).response();
    let forged = deliver_under(&backend, &"0123456789abcdef".repeat(4), &xml, &invented).await;
    assert_refused(&forged, "a relay state no one issued");

    // Another request's identifier, on the assertion or only on the response.
    let cases: Vec<(&str, Box<dyn Fn(&mut Reply)>)> = vec![
        (
            "the assertion answers another request",
            Box::new(|r| r.confirmation_in_response_to = "_another-request".into()),
        ),
        (
            "the response answers another request",
            Box::new(|r| r.response_in_response_to = Some("_another-request".into())),
        ),
        (
            "the assertion answers no request",
            Box::new(|r| r.confirmation_in_response_to = String::new()),
        ),
    ];
    for (why, change) in &cases {
        assert_refused(&backend.sign_in_with(|r| change(r)).await, why);
    }

    // A response made for one request, presented under another's relay state.
    let one = backend.begin(None, json!({})).await;
    let other = backend.begin(None, json!({})).await;
    let for_one = good(&one).response();
    let crossed = deliver_under(&backend, &other.request.relay_state, &for_one, &other).await;
    assert_refused(&crossed, "an answer to another request");

    // The request that was answered is gone.
    let round = backend.begin(None, json!({})).await;
    let xml = good(&round).response();
    assert_eq!(backend.finish(&round, &xml).await.0, 200);
    let mut again = good(&round);
    again.assertion_id = "_a-new-identifier".into();
    assert_refused(
        &backend.finish(&round, &again.response()).await,
        "a request is answered once",
    );
}

/// The response given to the page under a relay state, from the browser of `round`.
async fn deliver_under(
    backend: &Backend,
    relay_state: &str,
    xml: &str,
    round: &Round,
) -> (u16, Json) {
    backend
        .deliver(relay_state, xml, &round.binding, Some(SITE), None)
        .await
}

/// Verifies: FED-007
///
/// A response is accepted only from the browser that started the sign-in, at the address it started
/// at, and through the request's lifetime.
#[tokio::test]
async fn a_response_is_accepted_only_from_the_browser_and_site_that_started_it() {
    let backend = start(federation(vec![provider(|_| {})])).await;

    let round = backend.begin(None, json!({})).await;
    let xml = good(&round).response();
    let other_browser = deliver_under(
        &backend,
        &round.request.relay_state,
        &xml,
        &Round {
            request: round.request.clone(),
            binding: "f".repeat(64),
        },
    )
    .await;
    assert_refused(&other_browser, "another browser");

    let round = backend.begin(None, json!({})).await;
    let xml = good(&round).response();
    let elsewhere = backend
        .deliver(
            &round.request.relay_state,
            &xml,
            &round.binding,
            Some(ELSEWHERE),
            None,
        )
        .await;
    assert_refused(&elsewhere, "another address");

    let round = backend.begin(None, json!({})).await;
    let xml = good(&round).response();
    let nowhere = backend
        .deliver(&round.request.relay_state, &xml, &round.binding, None, None)
        .await;
    assert_refused(&nowhere, "no origin");

    // The browser that holds nothing of the sort does not get the binding from the body of what
    // it sent: the gateway takes it from the cookie.
    let round = backend.begin(None, json!({})).await;
    let xml = good(&round).response();
    let no_binding = backend
        .call_at(
            Some(SITE),
            "fieldnotes",
            None,
            "/internal/session/sign-in/saml",
            json!({ "relay_state": round.request.relay_state, "response": encode(&xml) }),
        )
        .await;
    assert_refused(&no_binding, "no binding");

    let round = backend.begin(None, json!({})).await;
    let xml = good(&round).response();
    assert_eq!(backend.finish(&round, &xml).await.0, 200);
}

/// Verifies: FED-007
///
/// A pending request lapses: one that waits past its lifetime is not answered.
#[tokio::test]
async fn a_pending_saml_request_lapses() {
    let backend = start_with(federation(vec![provider(|_| {})]), |cfg| {
        cfg.passkey_challenge_ttl_ms = 150
    })
    .await;
    let round = backend.begin(None, json!({})).await;
    let xml = good(&round).response();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_refused(&backend.finish(&round, &xml).await, "lapsed");
}

// ---------------------------------------------------------------------------------------------
// Time (FED-005 and the validity of an assertion)

/// Verifies: FED-005
///
/// An assertion is accepted only inside its validity, with a skew of a minute: one that has
/// expired, one whose confirmation has expired, one that is not valid yet, one with no end and one
/// issued in the future are refused; one that ended a few seconds ago, within the skew, is not.
#[tokio::test]
async fn an_assertion_outside_its_validity_is_refused() {
    let backend = start(federation(vec![provider(|_| {})])).await;
    let cases: Vec<(&str, Box<dyn Fn(&mut Reply)>)> = vec![
        (
            "expired",
            Box::new(|r| {
                r.not_on_or_after = Some(now() - 3600);
                r.confirmation_not_on_or_after = Some(now() - 3600);
            }),
        ),
        (
            "the conditions expired",
            Box::new(|r| r.not_on_or_after = Some(now() - 3600)),
        ),
        (
            "the confirmation expired",
            Box::new(|r| r.confirmation_not_on_or_after = Some(now() - 3600)),
        ),
        (
            "not valid yet",
            Box::new(|r| {
                r.not_before = Some(now() + 3600);
                r.not_on_or_after = Some(now() + 7200);
                r.confirmation_not_on_or_after = Some(now() + 7200);
            }),
        ),
        (
            "no end to the conditions",
            Box::new(|r| r.not_on_or_after = None),
        ),
        (
            "no end to the confirmation",
            Box::new(|r| r.confirmation_not_on_or_after = None),
        ),
        (
            "issued in the future",
            Box::new(|r| r.issued = now() + 3600),
        ),
    ];
    for (why, change) in &cases {
        assert_refused(&backend.sign_in_with(|r| change(r)).await, why);
    }
    // Within the skew of a clock that runs a little apart, either way.
    let (status, reply) = backend
        .sign_in_with(|r| {
            r.not_on_or_after = Some(now() - 20);
            r.confirmation_not_on_or_after = Some(now() - 20);
        })
        .await;
    assert_eq!(status, 200, "{reply}");
    let (status, reply) = backend
        .sign_in_with(|r| {
            r.not_before = Some(now() + 20);
            r.issued = now() + 20;
        })
        .await;
    assert_eq!(status, 200, "{reply}");

    // A realm that allows no skew allows none.
    let strict = start(federation(vec![provider(|p| p.skew_secs = 0)])).await;
    assert_refused(
        &strict
            .sign_in_with(|r| {
                r.not_on_or_after = Some(now() - 20);
                r.confirmation_not_on_or_after = Some(now() - 20);
            })
            .await,
        "no skew",
    );
}

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

/// Verifies: FED-250
///
/// A sign-in starts from the site's own pages and keeps only a path on the site; a provider that is
/// not enabled or does not exist cannot be started; asking to link needs a signed-in person.
#[tokio::test]
async fn a_saml_sign_in_starts_from_the_sites_own_pages_and_keeps_only_a_path_on_the_site() {
    let backend = start(federation(vec![
        provider(|_| {}),
        provider(|p| {
            p.id = "off".into();
            p.enabled = false;
        }),
    ]))
    .await;

    let (status, _) = backend
        .start_at(None, None, json!({ "provider": "acme" }))
        .await;
    assert_eq!(status, 403, "no origin");

    for (next, kept) in [
        ("/desk", Some("/desk")),
        ("/desk/edit?x=1", Some("/desk/edit?x=1")),
        ("//evil.example", None),
        ("https://evil.example", None),
        ("/\\evil.example", None),
        ("javascript:alert(1)", None),
        ("/ok\nLocation: x", None),
    ] {
        let (status, started) = backend
            .start_at(
                Some(SITE),
                None,
                json!({ "provider": "acme", "next": next }),
            )
            .await;
        assert_eq!(status, 200, "{started}");
        assert_eq!(started["return_to"].as_str(), kept, "{next:?}");
        assert!(started["redirect"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{SSO_URL}?SAMLRequest=")));
    }

    for provider in ["off", "nobody"] {
        let (status, _) = backend
            .start_at(Some(SITE), None, json!({ "provider": provider }))
            .await;
        assert_eq!(status, 422, "{provider}");
    }
    let (_, offered) = backend
        .call_at(
            None,
            "fieldnotes",
            None,
            "/internal/sign-in/saml/providers",
            json!({}),
        )
        .await;
    assert_eq!(offered["providers"].as_array().unwrap().len(), 1);
    // Asking to link, with no one signed in.
    let (status, _) = backend
        .start_at(
            Some(SITE),
            None,
            json!({ "provider": "acme", "link": true }),
        )
        .await;
    assert_eq!(status, 401);
}

/// Verifies: FED-008
///
/// Case CT-069, the metadata: it names this service at the address of the site it is asked at, asks
/// for assertions to be signed, and gives the one assertion consumer address, which is a POST
/// address of the site; a provider that does not exist or is not enabled has none, and nor does
/// another site's.
#[tokio::test]
async fn the_metadata_names_this_service_and_its_assertion_consumer_address() {
    let backend = start(federation(vec![
        provider(|_| {}),
        provider(|p| {
            p.id = "off".into();
            p.enabled = false;
        }),
        provider(|p| {
            p.id = "named".into();
            p.sp_entity_id = Some("urn:strata:fieldnotes".into());
        }),
    ]))
    .await;

    let (status, reply) = backend
        .call_at(
            None,
            "fieldnotes",
            None,
            "/internal/sign-in/saml/metadata",
            json!({ "provider": "acme", "origin": SITE }),
        )
        .await;
    assert_eq!(status, 200, "{reply}");
    let xml = reply["metadata"].as_str().unwrap();
    let doc = uppsala::parse(xml).expect("the metadata is XML");
    let root = doc.document_element().unwrap();
    assert_eq!(doc.get_attribute(root, "entityID"), Some(ENTITY));
    let service = doc.get_elements_by_tag_name("AssertionConsumerService")[0];
    assert_eq!(doc.get_attribute(service, "Location"), Some(ACS));
    assert_eq!(
        doc.get_attribute(service, "Binding"),
        Some("urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST")
    );
    let descriptor = doc.get_elements_by_tag_name("SPSSODescriptor")[0];
    assert_eq!(
        doc.get_attribute(descriptor, "WantAssertionsSigned"),
        Some("true")
    );
    assert!(!xml.contains("PRIVATE"), "nothing secret");

    let (_, named) = backend
        .call_at(
            None,
            "fieldnotes",
            None,
            "/internal/sign-in/saml/metadata",
            json!({ "provider": "named", "origin": SITE }),
        )
        .await;
    assert!(named["metadata"]
        .as_str()
        .unwrap()
        .contains("entityID=\"urn:strata:fieldnotes\""));

    for (site, body) in [
        ("fieldnotes", json!({ "provider": "off", "origin": SITE })),
        (
            "fieldnotes",
            json!({ "provider": "nobody", "origin": SITE }),
        ),
        ("tidewater", json!({ "provider": "acme", "origin": SITE })),
        (
            "fieldnotes",
            json!({ "provider": "acme", "origin": "javascript:x" }),
        ),
    ] {
        let (status, _) = backend
            .call_at(None, site, None, "/internal/sign-in/saml/metadata", body)
            .await;
        assert_eq!(status, 404);
    }

    // The entity identifier the request carries and the audience the response must name are the
    // one the metadata gives.
    let round = backend.begin(None, json!({})).await;
    assert_eq!(round.request.issuer, ENTITY);
}

/// Verifies: FED-001
///
/// Responses are limited like every other method's attempts: from one source, past the limit, they
/// are refused as throttled.
#[tokio::test]
async fn saml_responses_are_limited_like_every_other_method() {
    let backend = start_with(federation(vec![provider(|_| {})]), |cfg| {
        cfg.throttle = ThrottleLimits {
            source_max: 3,
            ..Default::default()
        }
    })
    .await;
    let mut statuses = Vec::new();
    for index in 0..5 {
        let (status, _) = backend
            .call_at(
                Some(SITE),
                "fieldnotes",
                None,
                "/internal/session/sign-in/saml",
                json!({
                    "relay_state": format!("{index:064x}"),
                    "response": encode("<x/>"),
                    "binding": "e".repeat(64),
                }),
            )
            .await;
        statuses.push(status);
    }
    assert_eq!(statuses.first(), Some(&401));
    assert_eq!(statuses.last(), Some(&429), "{statuses:?}");
}

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

/// Verifies: FED-011
///
/// A first sign-in creates a principal only when the realm enabled it; a realm that restricted its
/// provider to domains lets in only a verified address in one of them; a principal made with no
/// default role holds only `public`.
#[tokio::test]
async fn a_first_saml_sign_in_creates_a_principal_only_when_the_realm_enabled_it() {
    let closed = start(federation(vec![provider(|p| {
        p.create_on_first_sign_in = false
    })]))
    .await;
    assert_refused(&closed.sign_in().await, "creation is off");
    let tenant = TenantId::new("fieldnotes").unwrap();
    assert!(closed
        .store
        .identity(&tenant, "acme", "subject-1")
        .unwrap()
        .is_none());

    let bare = start(federation(vec![provider(|p| p.default_role = None)])).await;
    let (status, reply) = bare.sign_in().await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(roles(&reply), ["public"], "no default role, no role");

    let restricted = start(federation(vec![provider(|p| {
        p.domains = vec!["bound.example".into()]
    })]))
    .await;
    assert_refused(
        &restricted
            .sign_in_with(|r| r.email = Some("x@other.example".into()))
            .await,
        "an address outside the domains",
    );
    assert_refused(
        &restricted.sign_in_with(|r| r.email = None).await,
        "no address",
    );
    let unvouched = start(federation(vec![provider(|p| {
        p.domains = vec!["bound.example".into()];
        p.email_verified = false;
    })]))
    .await;
    assert_refused(
        &unvouched
            .sign_in_with(|r| r.email = Some("x@bound.example".into()))
            .await,
        "an address the provider does not vouch for",
    );
    assert_eq!(
        restricted
            .sign_in_with(|r| r.email = Some("x@bound.example".into()))
            .await
            .0,
        200
    );
}

/// Verifies: FED-257
///
/// An address that a provider asserts links to an existing principal only when the realm trusts
/// the provider that far and the provider vouches for the address. By default it links to no one.
#[tokio::test]
async fn an_asserted_address_links_to_an_existing_principal_only_when_vouched_and_trusted() {
    let claiming = |r: &mut Reply| {
        r.name_id = "someone-claiming-mirela".into();
        r.email = Some("mirela@fieldnotes.example".into());
    };
    let closed = start(federation(vec![provider(|p| {
        p.create_on_first_sign_in = false
    })]))
    .await;
    assert_refused(
        &closed.sign_in_with(claiming).await,
        "a vouched address, and no trust",
    );
    let open = start(federation(vec![provider(|_| {})])).await;
    let (status, reply) = open.sign_in_with(claiming).await;
    assert_eq!(status, 200, "{reply}");
    assert_ne!(principal(&reply), "user:mirela");

    let unvouched = start(federation(vec![provider(|p| {
        p.link_verified_email = true;
        p.create_on_first_sign_in = false;
        p.email_verified = false;
    })]))
    .await;
    assert_refused(
        &unvouched.sign_in_with(claiming).await,
        "an address that is not vouched for",
    );

    let linked = start(federation(vec![provider(|p| {
        p.link_verified_email = true;
        p.create_on_first_sign_in = false;
    })]))
    .await;
    let (status, reply) = linked.sign_in_with(claiming).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(principal(&reply), "user:mirela");
    assert_eq!(reply["session"]["display_name"], "Mirela Ashby");
    assert!(
        linked
            .audit()
            .await
            .to_string()
            .contains("saml.identity_linked"),
        "a link is audited"
    );
}

/// Verifies: FED-012
///
/// An identity is linked to an existing principal after that principal has signed in and asked to:
/// the response answers only to the person who started it, the identity then signs that principal
/// in, and an identity that belongs to one principal cannot be linked to another.
#[tokio::test]
async fn a_saml_identity_is_linked_only_by_the_principal_that_signed_in_first() {
    let backend = start(federation(vec![provider(|p| {
        p.create_on_first_sign_in = false
    })]))
    .await;
    assert_refused(&backend.sign_in().await, "an identity no one linked");

    let ines = backend.session_of("ines").await;
    let tomasz = backend.session_of("tomasz").await;

    let round = backend.begin(Some(&ines), json!({ "link": true })).await;
    let xml = good(&round).response();
    assert_refused(
        &backend
            .deliver(
                &round.request.relay_state,
                &xml,
                &round.binding,
                Some(SITE),
                Some(&tomasz),
            )
            .await,
        "another person's session",
    );
    let round = backend.begin(Some(&ines), json!({ "link": true })).await;
    let xml = good(&round).response();
    assert_refused(&backend.finish(&round, &xml).await, "no session");
    let round = backend.begin(Some(&ines), json!({ "link": true })).await;
    let xml = good(&round).response();
    let (status, reply) = backend
        .deliver(
            &round.request.relay_state,
            &xml,
            &round.binding,
            Some(SITE),
            Some(&ines),
        )
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(principal(&reply), "user:ines");

    // From then on the identity alone signs Inès in, in a browser that holds nothing.
    let (status, reply) = backend.sign_in().await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(principal(&reply), "user:ines");

    // Tomasz cannot link what is Inès's.
    let round = backend.begin(Some(&tomasz), json!({ "link": true })).await;
    let xml = good(&round).response();
    assert_refused(
        &backend
            .deliver(
                &round.request.relay_state,
                &xml,
                &round.binding,
                Some(SITE),
                Some(&tomasz),
            )
            .await,
        "an identity of another principal",
    );
}

/// Verifies: FED-258
///
/// A principal made at a first sign-in holds the one role the realm names and no other, whatever
/// the provider says about the person.
#[tokio::test]
async fn a_first_saml_sign_in_grants_only_the_default_role_the_realm_names() {
    let backend = start(federation(vec![provider(|p| {
        p.default_role = Some("author".into())
    })]))
    .await;
    let (status, reply) = backend
        .sign_in_with(|r| {
            r.name_id = "wants-everything".into();
            r.name = Some("Admin Owner".into());
            r.email = Some("admin@fieldnotes.example".into());
        })
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(roles(&reply), ["author", "public"]);
    assert_eq!(reply["session"]["may_manage_schema"], false);
    assert_ne!(principal(&reply), "user:admin");
}

/// Verifies: FED-011
///
/// An identity at one provider is not the same identity at another that spells its subject the
/// same, and a principal made for a SAML identity is not one made for an OpenID Connect one.
#[tokio::test]
async fn one_providers_subject_is_not_anothers() {
    let backend = start(federation(vec![
        provider(|_| {}),
        provider(|p| {
            p.id = "second".into();
            p.label = "Second".into();
        }),
    ]))
    .await;
    let (_, first) = backend.sign_in().await;
    let (status, started) = backend
        .start_at(Some(SITE), None, json!({ "provider": "second" }))
        .await;
    assert_eq!(status, 200, "{started}");
    let request = read_request(started["redirect"].as_str().unwrap());
    let reply = Reply::to(
        &request,
        "https://fieldnotes.example/federation/saml/metadata/second",
    );
    let (status, second) = backend
        .deliver(
            &request.relay_state,
            &reply.response(),
            started["binding"].as_str().unwrap(),
            Some(SITE),
            None,
        )
        .await;
    assert_eq!(status, 200, "{second}");
    assert_ne!(principal(&first), principal(&second));
}

// The time helper is part of the support file's own surface; this keeps it from being the one
// thing nobody calls, and checks the form the service reads.
#[test]
fn the_support_writes_times_as_saml_reads_them() {
    assert_eq!(instant(0), "1970-01-01T00:00:00Z");
    assert_eq!(instant(1_795_084_215), "2026-11-19T10:30:15Z");
    assert!(SAMLP_NS.starts_with("urn:oasis:names:tc:SAML:2.0:"));
}
