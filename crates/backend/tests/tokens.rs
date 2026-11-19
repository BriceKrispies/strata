//! Access tokens for service accounts and for agents acting for a person, their signing keys, and
//! what takes them away: revocation within the stated bound (shown with the backend's test
//! clock), key rotation and compromise, client secrets, and the identity outage. Real sockets,
//! real bundles, the in-memory store.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::{json, Value as Json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strata_backend::app::{Config, ThrottleLimits};
use strata_backend::mail::DevSink;
use strata_backend::signin::oidc::Federation;
use strata_protocol::api::{
    HEADER_CORRELATION, HEADER_SERVICE, HEADER_SESSION, HEADER_SITE, HEADER_SOURCE,
};
use strata_storage_api::memory::MemoryStore;

const TOKEN: &str = "test-service-token-0123456789abcdef";
const SOURCE: &str = "203.0.113.7";
const MINUTE: i64 = 60_000;

struct Backend {
    base: String,
    http: reqwest::Client,
    store: Arc<MemoryStore>,
}

async fn start() -> Backend {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store = Arc::new(MemoryStore::new());
    let cfg = Config {
        addr,
        service_token: TOKEN.into(),
        dev: true,
        tenants_dir: root.join("tenants"),
        lean_dir: root.join("lean"),
        session_ttl_ms: 3_600_000,
        session_idle_ms: 3_600_000,
        throttle: ThrottleLimits::default(),
        passkey_challenge_ttl_ms: 300_000,
        link_ttl_ms: 900_000,
        mail: Arc::new(DevSink::memory()),
        federation: Arc::new(Federation::none()),
    };
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

/// A client with its id and secret.
struct Client {
    id: String,
    secret: String,
}

impl Backend {
    async fn call_as(
        &self,
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
            .header(HEADER_CORRELATION, "t-corr-tokens")
            .json(&body);
        if let Some(token) = session {
            request = request.header(HEADER_SESSION, token);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Json::Null))
    }

    async fn call(&self, site: &str, session: Option<&str>, path: &str, body: Json) -> (u16, Json) {
        self.call_as(SOURCE, site, session, path, body).await
    }

    async fn session(&self, site: &str, login: &str, password: &str) -> String {
        let (status, reply) = self
            .call(
                site,
                None,
                "/internal/session/login",
                json!({ "login": login, "password": password }),
            )
            .await;
        assert_eq!(status, 200, "{reply}");
        reply["token"].as_str().unwrap().to_string()
    }

    async fn owner(&self) -> String {
        self.session("fieldnotes", "admin", "strata-dev-admin")
            .await
    }

    async fn person(&self, login: &str) -> String {
        self.session("fieldnotes", login, &format!("strata-dev-{login}"))
            .await
    }

    /// Make a service account as the owner.
    async fn client(&self, owner: &str, scopes: &[&str]) -> Client {
        let (status, reply) = self
            .call(
                "fieldnotes",
                Some(owner),
                "/internal/clients/create",
                json!({ "name": "Feed reader", "scopes": scopes }),
            )
            .await;
        assert_eq!(status, 200, "{reply}");
        Client {
            id: reply["id"].as_str().unwrap().to_string(),
            secret: reply["secret"].as_str().unwrap().to_string(),
        }
    }

    async fn token(&self, client: &Client, scope: Option<&str>) -> (u16, Json) {
        let mut body = json!({ "client_id": client.id, "client_secret": client.secret });
        if let Some(scope) = scope {
            body["scope"] = json!(scope);
        }
        self.call("fieldnotes", None, "/internal/token", body).await
    }

    async fn access(&self, client: &Client, scope: Option<&str>) -> String {
        let (status, reply) = self.token(client, scope).await;
        assert_eq!(status, 200, "{reply}");
        reply["access_token"].as_str().unwrap().to_string()
    }

    async fn verify(&self, site: &str, token: &str) -> (u16, Json) {
        self.call(
            site,
            None,
            "/internal/token/verify",
            json!({ "token": token }),
        )
        .await
    }

    async fn advance(&self, ms: i64) {
        let (status, _) = self
            .call(
                "fieldnotes",
                None,
                "/internal/dev/clock",
                json!({ "advance_ms": ms }),
            )
            .await;
        assert_eq!(status, 200);
    }

    async fn keys(&self) -> Vec<String> {
        let (status, reply) = self
            .call("fieldnotes", None, "/internal/keys", json!({}))
            .await;
        assert_eq!(status, 200, "{reply}");
        reply["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|key| key["kid"].as_str().unwrap().to_string())
            .collect()
    }
}

/// A part of a JWT, decoded.
fn part(token: &str, index: usize) -> Json {
    let bytes = URL_SAFE_NO_PAD
        .decode(token.split('.').nth(index).unwrap())
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn kid(token: &str) -> String {
    part(token, 0)["kid"].as_str().unwrap().to_string()
}

/// Verifies: TOK-001, TOK-002, TOK-005
///
/// A service account gets a token for its own scopes or a part of them, never more, and the
/// token lives five minutes at most.
#[tokio::test]
async fn a_service_account_gets_a_token_within_its_scopes_for_five_minutes() {
    let backend = start().await;
    let owner = backend.owner().await;
    let client = backend
        .client(&owner, &["read:post", "read:category"])
        .await;

    let (status, reply) = backend.token(&client, None).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["token_type"], "Bearer");
    assert!(reply["expires_in"].as_i64().unwrap() <= 300, "{reply}");
    assert_eq!(reply["scope"], "read:post read:category");
    let claims = part(reply["access_token"].as_str().unwrap(), 1);
    assert!(claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap() <= 300);
    assert_eq!(claims["sub"], client.id);

    let (status, reply) = backend.token(&client, Some("read:post")).await;
    assert_eq!((status, &reply["scope"]), (200, &json!("read:post")));

    for beyond in ["update:post", "read:*", "read:post read:author", "bogus"] {
        let (status, reply) = backend.token(&client, Some(beyond)).await;
        assert_eq!(status, 422, "{beyond}: {reply}");
    }
    // The audience is the site and nothing else.
    let (status, _) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/token",
            json!({ "client_id": client.id, "client_secret": client.secret, "audience": "strata:tidewater" }),
        )
        .await;
    assert_eq!(status, 422);
}

/// Verifies: TOK-005
///
/// With the test clock: a token holds until its five minutes are over and is refused after them,
/// whatever else is true of it.
#[tokio::test]
async fn a_token_is_refused_once_its_five_minutes_have_passed() {
    let backend = start().await;
    let owner = backend.owner().await;
    let client = backend.client(&owner, &["read:post"]).await;
    let token = backend.access(&client, None).await;

    assert_eq!(backend.verify("fieldnotes", &token).await.0, 200);
    backend.advance(4 * MINUTE).await;
    assert_eq!(
        backend.verify("fieldnotes", &token).await.0,
        200,
        "four minutes in"
    );
    backend.advance(MINUTE + 2_000).await;
    assert_eq!(
        backend.verify("fieldnotes", &token).await.0,
        401,
        "five minutes and two seconds in"
    );
}

/// Verifies: TOK-006
///
/// A token made for one site is refused by another: the other has other keys and another audience.
#[tokio::test]
async fn a_token_of_one_tenant_is_refused_by_every_other() {
    let backend = start().await;
    let owner = backend.owner().await;
    let client = backend.client(&owner, &["read:post"]).await;
    let token = backend.access(&client, None).await;
    assert_eq!(backend.verify("fieldnotes", &token).await.0, 200);
    assert_eq!(backend.verify("tidewater", &token).await.0, 401);

    // Nor does the account of one site hold at another.
    let (status, _) = backend
        .call(
            "tidewater",
            None,
            "/internal/token",
            json!({ "client_id": client.id, "client_secret": client.secret }),
        )
        .await;
    assert_eq!(status, 401);
    assert_eq!(backend.token(&client, None).await.0, 200);
}

/// Verifies: TOK-007
///
/// Revoking the service account, with the test clock a minute into the token's life: the very next
/// check refuses the token, which is inside the bound that applies to sessions, and nothing waits
/// for a notice to arrive.
#[tokio::test]
async fn revoking_a_client_refuses_its_tokens_at_once() {
    let backend = start().await;
    let owner = backend.owner().await;
    let client = backend.client(&owner, &["read:post"]).await;
    let token = backend.access(&client, None).await;
    backend.advance(MINUTE).await;
    assert_eq!(backend.verify("fieldnotes", &token).await.0, 200);

    let (status, reply) = backend
        .call(
            "fieldnotes",
            Some(&owner),
            "/internal/clients/revoke",
            json!({ "id": client.id }),
        )
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(backend.verify("fieldnotes", &token).await.0, 401);
    // And it makes no new one.
    assert_eq!(backend.token(&client, None).await.0, 401);
}

/// Verifies: TOK-007, TOK-003, TOK-004
///
/// Revoking a grant refuses the tokens made from it at once, and until then they name the agent
/// and the person.
#[tokio::test]
async fn revoking_a_grant_refuses_the_tokens_made_from_it() {
    let backend = start().await;
    let owner = backend.owner().await;
    let ines = backend.person("ines").await;
    let agent = backend.client(&owner, &["read:post"]).await;
    let (status, grant) = backend
        .call(
            "fieldnotes",
            Some(&ines),
            "/internal/grants/create",
            json!({ "agent": agent.id, "scope": "read:post update:post", "purpose": "tidy drafts" }),
        )
        .await;
    assert_eq!(status, 200, "{grant}");
    let grant_id = grant["grant"].as_str().unwrap().to_string();

    let exchange = |scope: Option<&str>| {
        let mut body = json!({
            "grant_type": "urn:ietf:params:oauth:grant-type:token-exchange",
            "client_id": agent.id,
            "client_secret": agent.secret,
            "grant": grant_id,
        });
        if let Some(scope) = scope {
            body["scope"] = json!(scope);
        }
        backend.call("fieldnotes", None, "/internal/token", body)
    };
    let (status, reply) = exchange(Some("update:post")).await;
    assert_eq!(status, 200, "{reply}");
    let token = reply["access_token"].as_str().unwrap().to_string();
    let claims = part(&token, 1);
    assert_eq!(claims["sub"], "user:ines", "the person");
    assert_eq!(claims["act"]["sub"], agent.id, "and the agent");
    assert_eq!(claims["scope"], "update:post");
    let (status, seen) = backend.verify("fieldnotes", &token).await;
    assert_eq!(
        (status, &seen["act"]["sub"]),
        (200, &json!(agent.id)),
        "{seen}"
    );

    // Beyond what was delegated, even where the agent's own scopes would not stop it.
    assert_eq!(exchange(Some("delete:post")).await.0, 422);
    backend.advance(MINUTE).await;
    let (status, _) = backend
        .call(
            "fieldnotes",
            Some(&ines),
            "/internal/grants/revoke",
            json!({ "grant": grant_id }),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(backend.verify("fieldnotes", &token).await.0, 401);
    assert_eq!(exchange(None).await.0, 401, "and no new one is made");
}

/// Verifies: TOK-004
///
/// A person delegates nothing they lack, and another client cannot use the delegation.
#[tokio::test]
async fn a_delegation_grants_nothing_the_person_lacks_and_belongs_to_its_agent() {
    let backend = start().await;
    let owner = backend.owner().await;
    let tomasz = backend.person("tomasz").await;
    let agent = backend.client(&owner, &["read:post"]).await;
    let other = backend.client(&owner, &["read:post"]).await;

    // An author does not edit categories.
    let (status, reply) = backend
        .call(
            "fieldnotes",
            Some(&tomasz),
            "/internal/grants/create",
            json!({ "agent": agent.id, "scope": "update:category" }),
        )
        .await;
    assert_eq!(status, 422, "{reply}");
    // Nor is "every object" within an author's reach for writing.
    let (status, _) = backend
        .call(
            "fieldnotes",
            Some(&tomasz),
            "/internal/grants/create",
            json!({ "agent": agent.id, "scope": "update:*" }),
        )
        .await;
    assert_eq!(status, 422);

    let (status, grant) = backend
        .call(
            "fieldnotes",
            Some(&tomasz),
            "/internal/grants/create",
            json!({ "agent": agent.id, "scope": "update:post" }),
        )
        .await;
    assert_eq!(status, 200, "{grant}");
    let exchange_as = |client: &Client| {
        backend.call(
            "fieldnotes",
            None,
            "/internal/token",
            json!({
                "grant_type": "token-exchange",
                "client_id": client.id,
                "client_secret": client.secret,
                "grant": grant["grant"],
            }),
        )
    };
    assert_eq!(exchange_as(&agent).await.0, 200);
    assert_eq!(
        exchange_as(&other).await.0,
        401,
        "a delegation is its agent's alone"
    );
    // Anyone who is not signed in cannot delegate.
    let (status, _) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/grants/create",
            json!({ "agent": agent.id, "scope": "read:post" }),
        )
        .await;
    assert_eq!(status, 401);
}

/// Verifies: TOK-250
///
/// A resource that knows which client presents a token refuses a token issued to another.
#[tokio::test]
async fn a_token_is_worthless_to_a_client_it_was_not_issued_to() {
    let backend = start().await;
    let owner = backend.owner().await;
    let client = backend.client(&owner, &["read:post"]).await;
    let other = backend.client(&owner, &["read:post"]).await;
    let token = backend.access(&client, None).await;
    let present = |presenter: &str| {
        backend.call(
            "fieldnotes",
            None,
            "/internal/token/verify",
            json!({ "token": token, "presenter": presenter }),
        )
    };
    assert_eq!(present(&client.id).await.0, 200);
    assert_eq!(present(&other.id).await.0, 401);
}

/// Verifies: TOK-008
///
/// After a rotation the previous key still verifies the tokens it signed, new tokens are signed
/// by the new key, and the previous key leaves the published set once its tokens have expired.
#[tokio::test]
async fn rotating_a_key_leaves_the_tokens_of_the_previous_one_verifiable_until_they_expire() {
    let backend = start().await;
    let owner = backend.owner().await;
    let client = backend.client(&owner, &["read:post"]).await;
    let before = backend.access(&client, None).await;
    let first = kid(&before);
    assert_eq!(backend.keys().await, [first.clone()]);

    let (status, rotated) = backend
        .call(
            "fieldnotes",
            Some(&owner),
            "/internal/keys/rotate",
            json!({}),
        )
        .await;
    assert_eq!(status, 200, "{rotated}");
    let second = rotated["kid"].as_str().unwrap().to_string();
    assert_ne!(first, second);

    let after = backend.access(&client, None).await;
    assert_eq!(kid(&after), second, "new tokens are signed by the new key");
    assert_eq!(backend.verify("fieldnotes", &before).await.0, 200);
    assert_eq!(backend.verify("fieldnotes", &after).await.0, 200);
    let mut published = backend.keys().await;
    published.sort();
    let mut both = vec![first.clone(), second.clone()];
    both.sort();
    assert_eq!(published, both);

    backend.advance(6 * MINUTE).await;
    assert_eq!(
        backend.keys().await,
        [second],
        "the old key's tokens have all expired"
    );
    assert_eq!(backend.verify("fieldnotes", &before).await.0, 401);
}

/// Verifies: TOK-251
///
/// A compromised key is refused from the moment it is marked, even for a token that is far from
/// expiring, and is no longer published; a new key signs at once.
#[tokio::test]
async fn a_compromised_key_is_retired_at_once() {
    let backend = start().await;
    let owner = backend.owner().await;
    let client = backend.client(&owner, &["read:post"]).await;
    let token = backend.access(&client, None).await;
    let signed_with = kid(&token);
    assert_eq!(backend.verify("fieldnotes", &token).await.0, 200);

    let (status, reply) = backend
        .call(
            "fieldnotes",
            Some(&owner),
            "/internal/keys/compromise",
            json!({ "kid": signed_with }),
        )
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(
        backend.verify("fieldnotes", &token).await.0,
        401,
        "no waiting"
    );
    let published = backend.keys().await;
    assert!(!published.contains(&signed_with), "{published:?}");
    assert_eq!(published.len(), 1, "a replacement is already there");
    let fresh = backend.access(&client, None).await;
    assert_ne!(kid(&fresh), signed_with);
    assert_eq!(backend.verify("fieldnotes", &fresh).await.0, 200);

    // Only an owner does this.
    let ines = backend.person("ines").await;
    let (status, _) = backend
        .call(
            "fieldnotes",
            Some(&ines),
            "/internal/keys/compromise",
            json!({ "kid": kid(&fresh) }),
        )
        .await;
    assert_eq!(status, 403);
}

/// Verifies: TOK-252
///
/// A secret is in the reply that made it and in no other, the store holds a hash of it, and
/// rotating it ends the old one.
#[tokio::test]
async fn a_client_secret_is_shown_once_stored_as_a_hash_and_rotated() {
    let backend = start().await;
    let owner = backend.owner().await;
    let client = backend.client(&owner, &["read:post"]).await;
    assert!(client.secret.starts_with("scs_"));

    let (status, listed) = backend
        .call("fieldnotes", Some(&owner), "/internal/clients", json!({}))
        .await;
    assert_eq!(status, 200);
    assert!(!listed.to_string().contains(&client.secret), "{listed}");

    let tenant = strata_kernel::TenantId::new("fieldnotes").unwrap();
    use strata_storage_api::Store;
    let held = backend.store.doc_list(&tenant, "client").unwrap();
    assert_eq!(held.len(), 1);
    assert!(
        !held[0].1.contains(&client.secret),
        "the secret is not stored"
    );
    assert!(held[0].1.contains("$argon2id$"), "a hash of it is");

    let (status, rotated) = backend
        .call(
            "fieldnotes",
            Some(&owner),
            "/internal/clients/rotate-secret",
            json!({ "id": client.id }),
        )
        .await;
    assert_eq!(status, 200, "{rotated}");
    let new_secret = rotated["secret"].as_str().unwrap().to_string();
    assert_ne!(new_secret, client.secret);
    assert_eq!(backend.token(&client, None).await.0, 401, "the old secret");
    let renewed = Client {
        id: client.id.clone(),
        secret: new_secret,
    };
    assert_eq!(backend.token(&renewed, None).await.0, 200);

    // Only an owner makes or rotates one.
    let ines = backend.person("ines").await;
    let (status, _) = backend
        .call(
            "fieldnotes",
            Some(&ines),
            "/internal/clients/rotate-secret",
            json!({ "id": client.id }),
        )
        .await;
    assert_eq!(status, 403);
}

/// Verifies: TOK-253
///
/// Wrong secrets for one client are limited like wrong passwords: past the limit even the right
/// secret is refused for a while, and another client is not affected.
#[tokio::test]
async fn credential_requests_are_limited_per_client() {
    let backend = start().await;
    let owner = backend.owner().await;
    let client = backend.client(&owner, &["read:post"]).await;
    let other = backend.client(&owner, &["read:post"]).await;
    let wrong = Client {
        id: client.id.clone(),
        secret: "scs_wrong".into(),
    };
    for _ in 0..10 {
        assert_eq!(backend.token(&wrong, None).await.0, 401);
    }
    assert_eq!(backend.token(&wrong, None).await.0, 429);
    assert_eq!(
        backend.token(&client, None).await.0,
        429,
        "locked for the client, whoever asks"
    );
    assert_eq!(backend.token(&other, None).await.0, 200);
    // The lock lasts the account lock of D-700, thirty seconds.
    backend.advance(31_000).await;
    assert_eq!(backend.token(&client, None).await.0, 200);
}

/// Verifies: TOK-009
///
/// Introspection tells a service account the scopes, subject and expiry of its own token, and
/// nothing of another's; a caller with no credentials is refused.
#[tokio::test]
async fn introspection_reports_a_token_to_a_caller_permitted_to_ask() {
    let backend = start().await;
    let owner = backend.owner().await;
    let client = backend
        .client(&owner, &["read:post", "read:category"])
        .await;
    let other = backend.client(&owner, &["read:post"]).await;
    let token = backend.access(&client, Some("read:post")).await;
    let ask = |who: &Client, token: &str| {
        backend.call(
            "fieldnotes",
            None,
            "/internal/token/introspect",
            json!({ "token": token, "client_id": who.id, "client_secret": who.secret }),
        )
    };
    let (status, seen) = ask(&client, &token).await;
    assert_eq!(status, 200, "{seen}");
    assert_eq!(seen["active"], true);
    assert_eq!(seen["scope"], "read:post");
    assert_eq!(seen["sub"], client.id);
    assert!(seen["expires_at"].as_i64().unwrap() > 0);

    let (_, theirs) = ask(&other, &token).await;
    assert_eq!(theirs, json!({ "active": false }));
    let (status, _) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/token/introspect",
            json!({ "token": token }),
        )
        .await;
    assert_eq!(status, 401, "no credentials");
    let (status, seen) = backend
        .call(
            "fieldnotes",
            Some(&owner),
            "/internal/token/introspect",
            json!({ "token": token }),
        )
        .await;
    assert_eq!(
        (status, &seen["active"]),
        (200, &json!(true)),
        "an owner may ask"
    );

    backend.advance(6 * MINUTE).await;
    let (_, gone) = ask(&client, &token).await;
    assert_eq!(gone, json!({ "active": false }), "expired");
}

/// Verifies: IDN-300, IDN-301
///
/// While the identity service cannot be reached, a request that needs a session is refused with
/// the outage named, a reader who needs none is served published content, a reader whose session
/// cannot be checked is still served what is published, and every sign-in fails closed. When it
/// is back, everything works again.
#[tokio::test]
async fn an_identity_outage_refuses_sessions_and_keeps_published_content() {
    let backend = start().await;
    let ines = backend.person("ines").await;
    let (status, _) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/dev/identity-outage",
            json!({ "on": true }),
        )
        .await;
    assert_eq!(status, 200);

    let (status, refused) = backend
        .call("fieldnotes", Some(&ines), "/internal/sessions", json!({}))
        .await;
    assert_eq!(status, 503, "{refused}");
    assert_eq!(refused["kind"], "identity_outage", "the outage is named");

    // IDN-301: published content, with no session and with one that cannot be checked.
    let (status, page) = backend
        .call("fieldnotes", None, "/internal/view", json!({ "path": "/" }))
        .await;
    assert_eq!(status, 200, "{page}");
    let (status, page) = backend
        .call(
            "fieldnotes",
            Some(&ines),
            "/internal/view",
            json!({ "path": "/" }),
        )
        .await;
    assert_eq!(status, 200, "{page}");

    // Every method fails closed: passwords here, and the federated ones through the same seam.
    let (status, refused) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/session/login",
            json!({ "login": "ines", "password": "strata-dev-ines" }),
        )
        .await;
    assert_eq!((status, &refused["kind"]), (503, &json!("identity_outage")));
    let (status, _) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/session/sign-in/oidc",
            json!({ "state": "s", "code": "c", "binding": "b" }),
        )
        .await;
    assert_eq!(status, 503);

    backend
        .call(
            "fieldnotes",
            None,
            "/internal/dev/identity-outage",
            json!({ "on": false }),
        )
        .await;
    assert_eq!(
        backend
            .call("fieldnotes", Some(&ines), "/internal/sessions", json!({}))
            .await
            .0,
        200
    );
}
