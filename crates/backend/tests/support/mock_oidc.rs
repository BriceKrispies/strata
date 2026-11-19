//! A local OpenID Connect provider for tests: discovery, signing keys, the authorisation endpoint
//! (which approves at once, as whoever the test says) and the token endpoint, with identity
//! tokens signed with fixed test keys.
//!
//! It checks of the client what a real provider checks: the client id and secret, the redirect
//! address that was asked for, and the proof key (the verifier must hash to the challenge). A test
//! steers what it answers through [`Mock::set`], and reads what it was asked through
//! [`Mock::authorizations`] and [`Mock::token_requests`].
//!
//! The keys are test keys, made for this file and used nowhere else.

#![allow(dead_code)]

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const CLIENT_ID: &str = "strata-test-client";
pub const CLIENT_SECRET: &str = "test-client-secret-0123456789";

/// An Ed25519 key, published as `ed-1`.
const ED_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIIvN9E9So+rf0H/D82HuKwYpr3n9kSu5GuuHa6nTGlpw\n-----END PRIVATE KEY-----\n";
const ED_X: &str = "2GgAviCgL5lFS0tMOtYCYW39tTqOWR7AwQGh2NwGw9k";
/// A P-256 key, `es-1`, unpublished until a test publishes it.
const ES_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgvp70m3FSI9Ks1Q8E\nodXfXU7fyRvt96L+MviW110WbFqhRANCAAQ4jcP2+C6VkHZmMs9nAFEOEQ9TaKWk\n9D+iB2YUgLeinXqYvfFcBd6eptFY9I+y/QDEpkdBKgAAWS2LC5bbpmpI\n-----END PRIVATE KEY-----\n";
const ES_X: &str = "OI3D9vgulZB2ZjLPZwBRDhEPU2ilpPQ_ogdmFIC3op0";
const ES_Y: &str = "epi98VwF3p6m0Vj0j7L9AMSmR0EqAABZLYsLltumakg";
/// A second P-256 key, never published: a token signed with it names a key nobody holds.
const GHOST_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgZG11N0eldRiJRS3Y\ncQl311KllAhB3wWlItRSYp4AuaGhRANCAASp2/364W6yfRUMJQkymmXa80XSmuIP\nZYCG8rax2JdTmHi4tsEkzh3oo9Hoo4g/APLRvXs8EetqzYExTp/8UHCn\n-----END PRIVATE KEY-----\n";

/// Which key signs the identity token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signer {
    /// The Ed25519 key, published as `ed-1`.
    Ed,
    /// The P-256 key `es-1`, which is published only after `publish_es`.
    Es,
    /// The P-256 key nobody publishes, under the name `ghost`.
    Ghost,
}

/// What the next identity tokens say and how they are made.
#[derive(Clone, Debug)]
pub struct Tweaks {
    pub sub: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub name: Option<String>,
    pub signer: Signer,
    /// The audience, when it is not the client.
    pub aud: Option<Value>,
    /// The nonce, when it is not the one that was asked for. `Some("")` leaves it out.
    pub nonce: Option<String>,
    pub exp_offset: i64,
    pub iat_offset: i64,
    pub issuer: Option<String>,
    pub azp: Option<String>,
    /// Leave `sub` out.
    pub no_sub: bool,
    /// A token with `alg: none` and no signature, naming the key `ed-1`.
    pub alg_none: bool,
    /// A token with `alg: HS256`, signed with a shared secret.
    pub hs256: bool,
    /// The provider sends the browser back with an error instead of a code.
    pub deny: bool,
}

impl Default for Tweaks {
    fn default() -> Self {
        Tweaks {
            sub: "subject-1".into(),
            email: Some("person@bound.example".into()),
            email_verified: true,
            name: Some("Pat Person".into()),
            signer: Signer::Ed,
            aud: None,
            nonce: None,
            exp_offset: 300,
            iat_offset: 0,
            issuer: None,
            azp: None,
            no_sub: false,
            alg_none: false,
            hs256: false,
            deny: false,
        }
    }
}

struct Pending {
    nonce: String,
    challenge: String,
    redirect_uri: String,
    tweaks: Tweaks,
}

struct Inner {
    base: String,
    tweaks: Tweaks,
    es_published: bool,
    /// What discovery says the issuer is, when it is not the real one.
    metadata_issuer: Option<String>,
    down: bool,
    codes: HashMap<String, Pending>,
    authorizations: Vec<HashMap<String, String>>,
    token_requests: Vec<HashMap<String, String>>,
    jwks_fetches: usize,
    next_code: u64,
}

type Shared = Arc<Mutex<Inner>>;

#[derive(Clone)]
pub struct Mock {
    /// The address of the provider, which is also its issuer.
    pub base: String,
    inner: Shared,
}

impl Mock {
    pub async fn start() -> Mock {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let inner = Arc::new(Mutex::new(Inner {
            base: base.clone(),
            tweaks: Tweaks::default(),
            es_published: false,
            metadata_issuer: None,
            down: false,
            codes: HashMap::new(),
            authorizations: Vec::new(),
            token_requests: Vec::new(),
            jwks_fetches: 0,
            next_code: 0,
        }));
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/jwks", get(jwks))
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .with_state(inner.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Mock { base, inner }
    }

    /// Change what the next tokens say.
    pub fn set(&self, change: impl FnOnce(&mut Tweaks)) {
        change(&mut self.inner.lock().unwrap().tweaks);
    }

    /// Back to a person who signs in with a valid token.
    pub fn reset(&self) {
        self.inner.lock().unwrap().tweaks = Tweaks::default();
    }

    /// Publish the P-256 key `es-1` in the provider's key set (a rotation).
    pub fn publish_es(&self) {
        self.inner.lock().unwrap().es_published = true;
    }

    /// Make discovery name another issuer than the real one.
    pub fn name_another_issuer(&self, issuer: &str) {
        self.inner.lock().unwrap().metadata_issuer = Some(issuer.to_string());
    }

    /// The provider does not answer: every request is a 503.
    pub fn go_down(&self) {
        self.inner.lock().unwrap().down = true;
    }

    pub fn come_back(&self) {
        self.inner.lock().unwrap().down = false;
    }

    pub fn authorizations(&self) -> Vec<HashMap<String, String>> {
        self.inner.lock().unwrap().authorizations.clone()
    }

    pub fn token_requests(&self) -> Vec<HashMap<String, String>> {
        self.inner.lock().unwrap().token_requests.clone()
    }

    pub fn jwks_fetches(&self) -> usize {
        self.inner.lock().unwrap().jwks_fetches
    }
}

fn down(inner: &Inner) -> Option<Response> {
    inner
        .down
        .then(|| StatusCode::SERVICE_UNAVAILABLE.into_response())
}

async fn discovery(State(shared): State<Shared>) -> Response {
    let inner = shared.lock().unwrap();
    if let Some(refusal) = down(&inner) {
        return refusal;
    }
    Json(json!({
        "issuer": inner.metadata_issuer.clone().unwrap_or_else(|| inner.base.clone()),
        "authorization_endpoint": format!("{}/authorize", inner.base),
        "token_endpoint": format!("{}/token", inner.base),
        "jwks_uri": format!("{}/jwks", inner.base),
        "response_types_supported": ["code"],
        "id_token_signing_alg_values_supported": ["EdDSA", "ES256"],
        "code_challenge_methods_supported": ["S256"],
    }))
    .into_response()
}

async fn jwks(State(shared): State<Shared>) -> Response {
    let mut inner = shared.lock().unwrap();
    if let Some(refusal) = down(&inner) {
        return refusal;
    }
    inner.jwks_fetches += 1;
    let mut keys = vec![json!({
        "kty": "OKP", "crv": "Ed25519", "x": ED_X, "kid": "ed-1", "alg": "EdDSA", "use": "sig",
    })];
    if inner.es_published {
        keys.push(json!({
            "kty": "EC", "crv": "P-256", "x": ES_X, "y": ES_Y, "kid": "es-1", "alg": "ES256", "use": "sig",
        }));
    }
    Json(json!({ "keys": keys })).into_response()
}

async fn authorize(
    State(shared): State<Shared>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let mut inner = shared.lock().unwrap();
    if let Some(refusal) = down(&inner) {
        return refusal;
    }
    inner.authorizations.push(query.clone());
    let get = |name: &str| query.get(name).cloned().unwrap_or_default();
    let valid = get("client_id") == CLIENT_ID
        && get("response_type") == "code"
        && get("code_challenge_method") == "S256"
        && get("code_challenge").len() >= 43
        && !get("state").is_empty()
        && !get("nonce").is_empty()
        && get("scope").split(' ').any(|scope| scope == "openid")
        && get("redirect_uri").starts_with("http");
    if !valid {
        return (StatusCode::BAD_REQUEST, "invalid_request").into_response();
    }
    let separator = if get("redirect_uri").contains('?') {
        '&'
    } else {
        '?'
    };
    if inner.tweaks.deny {
        return Redirect::to(&format!(
            "{}{separator}error=access_denied&state={}",
            get("redirect_uri"),
            get("state")
        ))
        .into_response();
    }
    inner.next_code += 1;
    let code = format!("code-{}-{}", inner.next_code, get("state").len());
    let pending = Pending {
        nonce: get("nonce"),
        challenge: get("code_challenge"),
        redirect_uri: get("redirect_uri"),
        tweaks: inner.tweaks.clone(),
    };
    inner.codes.insert(code.clone(), pending);
    Redirect::to(&format!(
        "{}{separator}code={code}&state={}",
        get("redirect_uri"),
        get("state")
    ))
    .into_response()
}

fn b64(bytes: impl AsRef<[u8]>) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The identity token a pending code is answered with.
fn mint(base: &str, pending: &Pending) -> String {
    let who = &pending.tweaks;
    let now = jsonwebtoken::get_current_timestamp() as i64;
    let mut claims = json!({
        "iss": who.issuer.clone().unwrap_or_else(|| base.to_string()),
        "aud": who.aud.clone().unwrap_or_else(|| json!(CLIENT_ID)),
        "exp": now + who.exp_offset,
        "iat": now + who.iat_offset,
    });
    if !who.no_sub {
        claims["sub"] = json!(who.sub);
    }
    match who.nonce.as_deref() {
        Some("") => {}
        Some(other) => claims["nonce"] = json!(other),
        None => claims["nonce"] = json!(pending.nonce),
    }
    if let Some(email) = &who.email {
        claims["email"] = json!(email);
        claims["email_verified"] = json!(who.email_verified);
    }
    if let Some(name) = &who.name {
        claims["name"] = json!(name);
    }
    if let Some(azp) = &who.azp {
        claims["azp"] = json!(azp);
    }

    if who.alg_none {
        let header = b64(r#"{"alg":"none","typ":"JWT","kid":"ed-1"}"#);
        return format!("{header}.{}.", b64(claims.to_string()));
    }
    if who.hs256 {
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some("ed-1".into());
        return encode(&header, &claims, &EncodingKey::from_secret(ED_X.as_bytes())).unwrap();
    }
    let (algorithm, kid, key) = match who.signer {
        Signer::Ed => (
            Algorithm::EdDSA,
            "ed-1",
            EncodingKey::from_ed_pem(ED_PEM.as_bytes()).unwrap(),
        ),
        Signer::Es => (
            Algorithm::ES256,
            "es-1",
            EncodingKey::from_ec_pem(ES_PEM.as_bytes()).unwrap(),
        ),
        Signer::Ghost => (
            Algorithm::ES256,
            "ghost",
            EncodingKey::from_ec_pem(GHOST_PEM.as_bytes()).unwrap(),
        ),
    };
    let mut header = Header::new(algorithm);
    header.kid = Some(kid.into());
    encode(&header, &claims, &key).unwrap()
}

async fn token(
    State(shared): State<Shared>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let mut inner = shared.lock().unwrap();
    if let Some(refusal) = down(&inner) {
        return refusal;
    }
    inner.token_requests.push(form.clone());
    let get = |name: &str| form.get(name).cloned().unwrap_or_default();
    // A code is good once.
    let pending = inner.codes.remove(&get("code"));
    let challenge = b64(Sha256::digest(get("code_verifier").as_bytes()));
    let right = pending.as_ref().is_some_and(|pending| {
        get("grant_type") == "authorization_code"
            && get("client_id") == CLIENT_ID
            && get("client_secret") == CLIENT_SECRET
            && get("redirect_uri") == pending.redirect_uri
            && challenge == pending.challenge
    });
    if !right {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_grant" })),
        )
            .into_response();
    }
    let id_token = mint(&inner.base, pending.as_ref().unwrap());
    Json(json!({
        "token_type": "Bearer",
        "access_token": "not-used-by-the-relying-party",
        "id_token": id_token,
    }))
    .into_response()
}
