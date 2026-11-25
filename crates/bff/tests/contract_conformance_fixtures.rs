//! The self-test of the contract conformance suite (WG-5182): the suite can fail. The contracts of
//! `conformance/fixtures/spec.jsonc` are served by the stand-in below; each `violates-<rule>`
//! contract has a handler that breaks exactly that rule (it returns an undeclared member, reads the
//! body before the guard, forgets the capability check, answers another tenant, ignores the age of
//! the authentication, refuses with an undeclared error, never throttles, accepts a write on a
//! read-only route, answers without a session), and its generated test is expected to fail with
//! `[<rule>]`. The `conforms-*` contracts are served as declared, and their tests pass.
//!
//!   specc conformance --spec crates/bff/tests/conformance/fixtures/spec.jsonc \
//!     --out crates/bff/tests/conformance/fixtures/generated --expect-fail-prefix violates-

#[allow(dead_code)]
#[path = "conformance/harness.rs"]
mod harness;

#[allow(dead_code)]
#[path = "conformance/fixtures/generated/contracts.rs"]
mod generated;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use harness::{Persona, Site, Target, SESSION_COOKIE};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The sites of the stand-in: (tenant, host).
const SITES: [(&str, &str); 2] = [("alpha", "127.0.0.1"), ("beta", "localhost")];

/// The persons of the stand-in: (tenant, login, password, role).
const ACCOUNTS: [(&str, &str, &str, &str); 4] = [
    ("alpha", "olga", "pw-olga", "owner"),
    ("alpha", "wren", "pw-wren", "writer"),
    ("alpha", "rhea", "pw-rhea", "reader"),
    ("beta", "bo", "pw-bo", "owner"),
];

/// What each role may do, as the fixture specification's roles say.
fn capabilities(role: &str) -> &'static [&'static str] {
    match role {
        "owner" => &["notes-read", "notes-write", "notes-delete"],
        "writer" => &["notes-read", "notes-write"],
        "reader" => &["notes-read"],
        _ => &[],
    }
}

#[derive(Clone)]
struct Session {
    tenant: String,
    role: String,
    csrf: String,
    aged: bool,
}

#[derive(Default)]
struct Fixture {
    sessions: Mutex<BTreeMap<String, Session>>,
    visitor_tokens: Mutex<BTreeSet<(String, String)>>,
    counter: AtomicU64,
    pings: AtomicU64,
}

type Shared = Arc<Fixture>;

impl Fixture {
    fn token(&self, what: &str) -> String {
        format!(
            "{what}{:060x}",
            self.counter.fetch_add(1, Ordering::Relaxed) + 1
        )
    }
}

fn refuse(code: &str, status: u16) -> Response {
    refuse_kind(code, status, None)
}

fn refuse_kind(code: &str, status: u16, kind: Option<&str>) -> Response {
    let mut body =
        json!({ "code": code, "message": format!("refused: {code}"), "correlation": "fixture" });
    if let Some(kind) = kind {
        body["kind"] = json!(kind);
    }
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(body),
    )
        .into_response()
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// The tenant the request's host serves.
fn tenant_of(headers: &HeaderMap) -> Option<String> {
    let host = header(headers, "host")?;
    let name = host.split(':').next().unwrap_or(host);
    SITES
        .iter()
        .find(|(_, h)| *h == name)
        .map(|(t, _)| (*t).to_string())
}

/// The live session the cookie names, when it is a session of this tenant.
fn session_of(fixture: &Fixture, headers: &HeaderMap, tenant: &str) -> Option<(String, Session)> {
    let cookie = header(headers, "cookie")?;
    let token = cookie
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .map(|(_, value)| value.to_string())?;
    let session = fixture.sessions.lock().unwrap().get(&token).cloned()?;
    (session.tenant == tenant).then_some((token, session))
}

/// What a route asks of its caller, checked in the order the specification's authorizer checks it.
#[derive(Default, Clone, Copy)]
struct Need {
    session: bool,
    origin: bool,
    csrf: bool,
    role: Option<&'static str>,
    requires: &'static [&'static str],
    step_up: bool,
}

fn guard(fixture: &Fixture, headers: &HeaderMap, need: Need) -> Result<Option<Session>, Response> {
    let Some(tenant) = tenant_of(headers) else {
        return Err(refuse("not_found", 404));
    };
    if need.origin {
        let own = header(headers, "host").map(|host| format!("http://{host}"));
        if own.is_none() || header(headers, "origin") != own.as_deref() {
            return Err(refuse("forbidden", 403));
        }
    }
    let session = session_of(fixture, headers, &tenant).map(|(_, s)| s);
    if need.session && session.is_none() {
        return Err(refuse("unauthenticated", 401));
    }
    if need.csrf {
        let given = header(headers, "x-csrf-token").unwrap_or("");
        let valid = !given.is_empty()
            && match &session {
                Some(session) => session.csrf == given,
                None => fixture
                    .visitor_tokens
                    .lock()
                    .unwrap()
                    .contains(&(tenant.clone(), given.to_string())),
            };
        if !valid {
            return Err(refuse("forbidden", 403));
        }
    }
    if let Some(session) = &session {
        if need.role.is_some_and(|role| role != session.role) {
            return Err(refuse("forbidden", 403));
        }
        if !need
            .requires
            .iter()
            .all(|c| capabilities(&session.role).contains(c))
        {
            return Err(refuse("forbidden", 403));
        }
        if need.step_up && session.aged {
            return Err(refuse_kind("forbidden", 403, Some("step_up_required")));
        }
    }
    Ok(session)
}

const READ: Need = Need {
    session: true,
    origin: false,
    csrf: false,
    role: None,
    requires: &["notes-read"],
    step_up: false,
};

const WRITE: Need = Need {
    session: true,
    origin: true,
    csrf: true,
    role: None,
    requires: &["notes-write"],
    step_up: true,
};

fn note() -> Value {
    json!({ "note": { "title": "First", "word_count": 3 } })
}

async fn session(State(fixture): State<Shared>, headers: HeaderMap) -> Response {
    let Some(tenant) = tenant_of(&headers) else {
        return refuse("not_found", 404);
    };
    if let Some((_, session)) = session_of(&fixture, &headers, &tenant) {
        return Json(json!({ "session": { "authenticated": true, "csrf_token": session.csrf } }))
            .into_response();
    }
    let token = fixture.token("v");
    fixture
        .visitor_tokens
        .lock()
        .unwrap()
        .insert((tenant, token.clone()));
    Json(json!({ "session": { "authenticated": false, "csrf_token": token } })).into_response()
}

async fn login(State(fixture): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    let need = Need {
        origin: true,
        csrf: true,
        ..Need::default()
    };
    if let Err(refusal) = guard(&fixture, &headers, need) {
        return refusal;
    }
    let tenant = tenant_of(&headers).unwrap_or_default();
    let Ok(body) = serde_json::from_slice::<Value>(&body) else {
        return refuse("invalid", 422);
    };
    let account = ACCOUNTS.iter().find(|(t, login, password, _)| {
        *t == tenant && body["login"] == *login && body["password"] == *password
    });
    let Some((_, _, _, role)) = account else {
        return refuse("unauthenticated", 401);
    };
    let token = fixture.token("s");
    let csrf = fixture.token("c");
    fixture.sessions.lock().unwrap().insert(
        token.clone(),
        Session {
            tenant,
            role: (*role).to_string(),
            csrf: csrf.clone(),
            aged: false,
        },
    );
    let mut response =
        Json(json!({ "session": { "authenticated": true, "csrf_token": csrf } })).into_response();
    if let Ok(cookie) =
        HeaderValue::from_str(&format!("{SESSION_COOKIE}={token}; Path=/; HttpOnly"))
    {
        response.headers_mut().insert("set-cookie", cookie);
    }
    response
}

async fn age_authentication(State(fixture): State<Shared>, headers: HeaderMap) -> Response {
    let need = Need {
        session: true,
        origin: true,
        csrf: true,
        ..Need::default()
    };
    if let Err(refusal) = guard(&fixture, &headers, need) {
        return refusal;
    }
    let tenant = tenant_of(&headers).unwrap_or_default();
    if let Some((token, _)) = session_of(&fixture, &headers, &tenant) {
        if let Some(session) = fixture.sessions.lock().unwrap().get_mut(&token) {
            session.aged = true;
        }
    }
    Json(json!({ "aged": true })).into_response()
}

// ---- reading a note -------------------------------------------------------------------------

async fn read_note(
    State(fixture): State<Shared>,
    headers: HeaderMap,
    Path(_): Path<String>,
) -> Response {
    match guard(&fixture, &headers, READ) {
        Ok(_) => Json(note()).into_response(),
        Err(refusal) => refusal,
    }
}

/// violates-reply-shape: the reply has a member the contract does not declare.
async fn read_note_with_secret(
    State(fixture): State<Shared>,
    headers: HeaderMap,
    Path(_): Path<String>,
) -> Response {
    match guard(&fixture, &headers, READ) {
        Ok(_) => {
            let mut reply = note();
            reply["secret"] = json!("the author's address");
            Json(reply).into_response()
        }
        Err(refusal) => refusal,
    }
}

/// violates-no-session: anyone is answered.
async fn read_note_open(Path(_): Path<String>) -> Response {
    Json(note()).into_response()
}

/// violates-declared-error: a call without a session is refused `unavailable`, which the contract
/// does not declare.
async fn read_note_undeclared(
    State(fixture): State<Shared>,
    headers: HeaderMap,
    Path(_): Path<String>,
) -> Response {
    let tenant = tenant_of(&headers).unwrap_or_default();
    if session_of(&fixture, &headers, &tenant).is_none() {
        return refuse("unavailable", 503);
    }
    match guard(&fixture, &headers, READ) {
        Ok(_) => Json(note()).into_response(),
        Err(refusal) => refusal,
    }
}

// ---- writing a note -------------------------------------------------------------------------

/// The body of a write, after the guard: JSON, the site's own tenant, a title and a word count.
fn write_body(headers: &HeaderMap, body: &[u8], check_tenant: bool) -> Response {
    let Ok(body) = serde_json::from_slice::<Value>(body) else {
        return refuse("invalid", 422);
    };
    if check_tenant && body["tenant"].as_str() != tenant_of(headers).as_deref() {
        return refuse("forbidden", 403);
    }
    if !body["title"].is_string() || !body["word_count"].is_u64() {
        return refuse("invalid", 422);
    }
    Json(json!({ "note": 7 })).into_response()
}

async fn write_note(State(fixture): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    match guard(&fixture, &headers, WRITE) {
        Ok(_) => write_body(&headers, &body, true),
        Err(refusal) => refusal,
    }
}

/// violates-csrf: the body is parsed before the guard, so a body that is not JSON is refused as a
/// body error, as a typed extractor does.
async fn write_parses_first(
    State(fixture): State<Shared>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if serde_json::from_slice::<Value>(&body).is_err() {
        return (
            StatusCode::BAD_REQUEST,
            "Failed to parse the request body as JSON",
        )
            .into_response();
    }
    match guard(&fixture, &headers, WRITE) {
        Ok(_) => write_body(&headers, &body, true),
        Err(refusal) => refusal,
    }
}

/// violates-capability: the capability is not checked.
async fn write_unchecked(
    State(fixture): State<Shared>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let need = Need {
        requires: &[],
        ..WRITE
    };
    match guard(&fixture, &headers, need) {
        Ok(_) => write_body(&headers, &body, true),
        Err(refusal) => refusal,
    }
}

/// violates-other-tenant-input: a body naming another tenant is answered.
async fn write_any_tenant(
    State(fixture): State<Shared>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match guard(&fixture, &headers, WRITE) {
        Ok(_) => write_body(&headers, &body, false),
        Err(refusal) => refusal,
    }
}

/// violates-step-up: the age of the authentication is not checked.
async fn write_stale(State(fixture): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    let need = Need {
        step_up: false,
        ..WRITE
    };
    match guard(&fixture, &headers, need) {
        Ok(_) => write_body(&headers, &body, true),
        Err(refusal) => refusal,
    }
}

async fn delete_note(State(fixture): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    let need = Need {
        session: true,
        origin: true,
        csrf: true,
        role: Some("owner"),
        requires: &["notes-delete"],
        step_up: false,
    };
    if let Err(refusal) = guard(&fixture, &headers, need) {
        return refusal;
    }
    match serde_json::from_slice::<Value>(&body) {
        Ok(body) if body["note"].is_u64() => Json(json!({ "deleted": true })).into_response(),
        _ => refuse("invalid", 422),
    }
}

// ---- pings ----------------------------------------------------------------------------------

/// Three pings a window, then `throttled`.
async fn ping(State(fixture): State<Shared>) -> Response {
    if fixture.pings.fetch_add(1, Ordering::Relaxed) >= 3 {
        return refuse("throttled", 429);
    }
    Json(json!({ "ok": true })).into_response()
}

/// violates-quota (never throttled) and violates-read-only (also mounted for POST).
async fn ping_unlimited() -> Response {
    Json(json!({ "ok": true })).into_response()
}

/// The stand-in on 127.0.0.1, with the persons of [`ACCOUNTS`].
pub async fn start_target() -> Target {
    let fixture: Shared = Arc::new(Fixture::default());
    let app = Router::new()
        .route("/api/session", get(session))
        .route("/api/login", post(login))
        .route("/api/dev/age-authentication", post(age_authentication))
        .route("/api/notes/write", post(write_note))
        .route("/api/notes/delete", post(delete_note))
        .route("/api/notes/:note", get(read_note))
        .route("/api/broken/notes/:note", get(read_note_with_secret))
        .route("/api/broken/notes-open/:note", get(read_note_open))
        .route(
            "/api/broken/notes-undeclared/:note",
            get(read_note_undeclared),
        )
        .route("/api/broken/write-parses-first", post(write_parses_first))
        .route("/api/broken/write-unchecked", post(write_unchecked))
        .route("/api/broken/write-any-tenant", post(write_any_tenant))
        .route("/api/broken/write-stale", post(write_stale))
        .route("/api/ping", get(ping))
        .route("/api/broken/ping", get(ping_unlimited))
        .route(
            "/api/broken/ping-writable",
            get(ping_unlimited).post(ping_unlimited),
        )
        .with_state(fixture);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let sites = SITES
        .iter()
        .map(|(tenant, host)| Site {
            tenant: (*tenant).to_string(),
            host: (*host).to_string(),
        })
        .collect();
    let personas = ACCOUNTS
        .iter()
        .map(|(tenant, login, password, role)| Persona {
            site: SITES.iter().position(|(t, _)| t == tenant).unwrap_or(0),
            login: (*login).to_string(),
            password: (*password).to_string(),
            roles: vec![(*role).to_string()],
        })
        .collect();
    Target::new(port, sites, personas)
}

/// The stand-in is cheap and each fixture breaks it its own way, so every test has its own.
pub async fn target() -> Target {
    start_target().await
}

/// The fixtures cover every rule a test can fail with, and none conforms by accident: every
/// `violates-` contract is generated to fail.
#[test]
fn every_violation_is_a_fixture() {
    for rule in [
        "reply-shape",
        "csrf",
        "capability",
        "other-tenant-input",
        "step-up",
        "declared-error",
        "quota",
        "read-only",
        "no-session",
    ] {
        let id = format!("violates-{rule}");
        assert!(
            generated::COVERED.contains(&id.as_str()),
            "no fixture breaks {rule}"
        );
    }
    assert!(!generated::COVERED.contains(&"unshipped-note"));
}
