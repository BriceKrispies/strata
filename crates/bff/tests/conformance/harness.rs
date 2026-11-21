//! The hand-written half of the contract conformance suite (WG-5182). `specc conformance` writes,
//! from the specification, one test per shipped contract that names a `bffRoute`
//! (`conformance/generated/contracts.rs`); each test hands its contract to [`conform`], which
//! starts a target (`super::start_target`: the real BFF and backend for
//! `contract_conformance.rs`, a stand-in with broken handlers for `contract_conformance_fixtures.rs`),
//! makes the calls the declaration implies and compares every answer with it.
//!
//! The checks, each with the rule a failure is reported under (`[rule] what happened`):
//!
//! - `no-session`: a `session` contract called with no session (but our origin and a request
//!   token the site issued) answers `unauthenticated`; `no-credential` the same for a scheme the
//!   suite holds no credential of. An `anonymous` contract is only held to the shape of its answer.
//! - `origin`: without our origin, and with the origin of the other site, the call is refused
//!   `forbidden`, with a body that is not JSON: the refusal comes before the body is read.
//! - `csrf`: without the request token, and with a token that is not the session's, the same.
//! - `capability`: for each required capability, a person of the site who lacks it (by the
//!   specification's roles, which every signed-in person extends with `visitor` and
//!   `account-holder`) is refused `forbidden`.
//! - `role`: a person who holds none of `auth.roles` is refused `forbidden`.
//! - `other-tenant-session`: a session of the other site, presented at this site's host, is
//!   refused (`unauthenticated` or `forbidden`), never answered.
//! - `other-tenant-input`: an input of type `tenant-id` naming the other site is refused
//!   `forbidden`.
//! - `step-up`: after `/api/dev/age-authentication`, a `stepUp` contract is refused, and the
//!   refusal's `kind` is the wire name of the declared error `step-up-required`.
//! - `read-only`: a `readOnly` contract's route refuses a state-changing method.
//! - `success`: with the example inputs (or none, when none is required) a person who holds what
//!   the contract asks for gets a success.
//!   `setup`: the steps the contract declares (`conformanceSetup`) are taken first as that
//!   person: a read whose reply member is copied into an input, or a named step; one that fails
//!   is reported under this rule.
//! - `quota`: on a fresh target, `limit` calls are answered and the next one is `throttled`.
//! - Every answer: `reply-shape` (a success is exactly the declared outputs with their types),
//!   `refusal-shape` (a refusal is the error body type, with the status of its code) and
//!   `declared-error` (the code is the kind of an error the contract declares).
//!
//! A check that applies and cannot be made is printed as `conformance skip: <contract> <check>:
//! <reason>`, with the ones the generator found (`Spec::skipped`); none is silent.

use reqwest::Method;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use totp_rs::{Algorithm, Secret, TOTP};

/// The session cookie of the BFF (and of the fixtures' stand-in).
pub const SESSION_COOKIE: &str = "__Host-strata_sid";
/// The roles every signed-in person holds besides its own: the specification's roles for anyone
/// (`visitor`) and for every signed-in person (`account-holder`).
pub const SIGNED_IN_ROLES: &[&str] = &["visitor", "account-holder"];
/// The roles of a visitor without a session.
pub const ANONYMOUS_ROLES: &[&str] = &["visitor"];
/// A body that is not JSON, sent where a refusal must come before the body is read.
pub const MALFORMED_BODY: &str = "{\"conformance\": ";
/// The development action that ages the caller's authentication past the step-up age.
pub const AGE_AUTHENTICATION: &str = "/api/dev/age-authentication";
/// The largest quota the suite exhausts (the generator's `QUOTA_CALLS`).
pub const QUOTA_CALLS: u64 = 100;

// ---- what the generated suite states --------------------------------------------------------

/// A type on the wire.
#[derive(Debug, Clone, Copy)]
pub enum Ty {
    /// Any JSON value.
    Any,
    Bool,
    /// An integer in the range, inclusive.
    Int(i128, i128),
    Float,
    Text,
    /// A string (base64) or an array of bytes.
    Bytes,
    List(&'static Ty),
    /// A JSON object with values of the type.
    Map(&'static Ty),
    /// `null` or a value of the type.
    Option(&'static Ty),
    /// A declared type of [`Spec::types`].
    Named(&'static str),
}

/// A member of a struct, or an output of a contract.
#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub wire: &'static str,
    pub ty: Ty,
    pub optional: bool,
}

/// How a union is tagged.
#[derive(Debug, Clone, Copy)]
pub enum Tag {
    External,
    Internal(&'static str),
    Adjacent(&'static str, &'static str),
}

/// What a variant holds.
#[derive(Debug, Clone, Copy)]
pub enum Content {
    Unit,
    Newtype(Ty),
    Tuple(&'static [Ty]),
    Fields(&'static [Field]),
}

/// A variant of a union, with its wire name.
#[derive(Debug, Clone, Copy)]
pub struct Variant {
    pub wire: &'static str,
    pub content: Content,
}

/// A declared type.
#[derive(Debug, Clone, Copy)]
pub enum Decl {
    Alias(Ty),
    Enum(&'static [&'static str]),
    Struct(&'static [Field]),
    Union(Tag, &'static [Variant]),
}

/// A declared type with its id.
#[derive(Debug, Clone, Copy)]
pub struct TypeDecl {
    pub id: &'static str,
    pub decl: Decl,
}

/// What the suite reads of the specification besides the contracts.
#[derive(Debug, Clone, Copy)]
pub struct Spec {
    /// The capabilities each role confers.
    pub roles: &'static [(&'static str, &'static [&'static str])],
    pub types: &'static [TypeDecl],
    /// The status of each error kind.
    pub error_status: &'static [(&'static str, u16)],
    /// The type every refusal's body is.
    pub error_body: Option<&'static str>,
    /// The checks the generator found it cannot make: (contract, check, reason).
    pub skipped: &'static [(&'static str, &'static str, &'static str)],
}

/// How the BFF serves the contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Json,
    View,
    Records,
    Socket,
}

/// How a caller proves who it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Anonymous,
    Session,
    Other(&'static str),
}

/// A request: the path, the query pairs (not yet encoded) and the JSON body, if any.
#[derive(Debug, Clone, Copy)]
pub struct Request {
    pub path: &'static str,
    pub query: &'static [(&'static str, &'static str)],
    pub body: Option<&'static str>,
}

/// A step taken as the caller before a success call (the contract's `conformanceSetup`).
#[derive(Debug, Clone, Copy)]
pub enum Setup {
    /// A `GET` of the path; each `(wire name, member path)` copies the reply's member (a dotted
    /// path) into the input of that name in the success call.
    Read {
        path: &'static str,
        set: &'static [(&'static str, &'static str)],
    },
    /// A step implemented here by name: `enrol-authenticator`.
    Step(&'static str),
}

/// Where an output of a records reply is.
#[derive(Debug, Clone, Copy)]
pub enum RecordFrom {
    Id,
    Version,
    Field(&'static str),
}

/// An output of a records reply.
#[derive(Debug, Clone, Copy)]
pub struct RecordOutput {
    pub from: RecordFrom,
    pub ty: Ty,
    pub optional: bool,
}

/// What a success answers.
#[derive(Debug, Clone, Copy)]
pub enum Reply {
    Members(&'static [Field]),
    Records(&'static [RecordOutput]),
    Socket,
}

/// One contract, as the generated suite states it.
#[derive(Debug, Clone, Copy)]
pub struct Contract {
    pub id: &'static str,
    pub method: &'static str,
    pub shape: Shape,
    pub scheme: Scheme,
    /// Every one is required.
    pub requires: &'static [&'static str],
    /// Any one admits.
    pub roles: &'static [&'static str],
    pub origin: bool,
    pub csrf: bool,
    pub step_up: bool,
    pub read_only: bool,
    /// The wire name of the input of type `tenant-id`.
    pub tenant_input: Option<&'static str>,
    /// The request of the refusal checks: the contract's probe inputs, else the example's, else
    /// placeholders.
    pub probe: Request,
    /// The request that must succeed, when there is one.
    pub success: Option<Request>,
    /// The steps taken as the caller before the success call.
    pub setup: &'static [Setup],
    pub reply: Reply,
    /// The kinds of the declared errors.
    pub errors: &'static [&'static str],
    /// The wire name of the declared error `step-up-required`.
    pub step_up_error: Option<&'static str>,
    /// The limit of a quota small enough to exhaust.
    pub quota: Option<u64>,
}

// ---- the system under test ------------------------------------------------------------------

/// A site of the target: its tenant and the host name it is served at.
#[derive(Debug, Clone)]
pub struct Site {
    pub tenant: String,
    pub host: String,
}

/// A person who can sign in with a password: the site (an index of [`Target::sites`]), the
/// credentials and the roles the tenant gives it.
#[derive(Debug, Clone)]
pub struct Persona {
    pub site: usize,
    pub login: String,
    pub password: String,
    pub roles: Vec<String>,
}

/// A running BFF on 127.0.0.1: the first site is the one every contract is checked at, the second
/// the other tenant.
pub struct Target {
    pub port: u16,
    pub sites: Vec<Site>,
    pub personas: Vec<Persona>,
    pub http: reqwest::Client,
}

impl Target {
    pub fn new(port: u16, sites: Vec<Site>, personas: Vec<Persona>) -> Self {
        assert!(sites.len() >= 2, "a target serves this site and another");
        Target {
            port,
            sites,
            personas,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("an HTTP client"),
        }
    }

    fn origin(&self, site: usize) -> String {
        format!("http://{}:{}", self.sites[site].host, self.port)
    }
}

/// A signed-in session: the cookie's value and its request token.
#[derive(Debug, Clone)]
struct Session {
    cookie: String,
    csrf: String,
}

/// What the target answered.
#[derive(Debug, Clone)]
struct Answer {
    status: u16,
    text: String,
    json: Option<Value>,
}

impl Answer {
    fn code(&self) -> Option<&str> {
        self.json.as_ref()?.get("code")?.as_str()
    }

    fn summary(&self) -> String {
        let text: String = self.text.chars().take(240).collect();
        format!("status {} {}", self.status, text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OriginHeader {
    Own,
    Absent,
    OtherSite,
}

/// One call: everything but the contract's own path and body is chosen by the check.
struct Call {
    method: Method,
    site: usize,
    path: String,
    origin: OriginHeader,
    csrf: Option<String>,
    cookie: Option<String>,
    body: Option<String>,
}

fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

async fn send(target: &Target, call: &Call) -> Answer {
    let host = format!("{}:{}", target.sites[call.site].host, target.port);
    let mut request = target
        .http
        .request(
            call.method.clone(),
            format!("http://127.0.0.1:{}{}", target.port, call.path),
        )
        .header("host", host);
    match call.origin {
        OriginHeader::Own => request = request.header("origin", target.origin(call.site)),
        OriginHeader::OtherSite => {
            request = request.header("origin", target.origin(1 - call.site.min(1)))
        }
        OriginHeader::Absent => {}
    }
    if let Some(csrf) = &call.csrf {
        request = request.header("x-csrf-token", csrf);
    }
    if let Some(cookie) = &call.cookie {
        request = request.header("cookie", format!("{SESSION_COOKIE}={cookie}"));
    }
    if let Some(body) = &call.body {
        request = request
            .header("content-type", "application/json")
            .body(body.clone());
    }
    match request.send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            let json = serde_json::from_str(&text).ok();
            Answer { status, text, json }
        }
        Err(e) => Answer {
            status: 0,
            text: format!("the request failed: {e}"),
            json: None,
        },
    }
}

/// A socket upgrade; the request token travels in the address, as a browser's does.
async fn upgrade(target: &Target, call: &Call) -> Answer {
    let mut path = call.path.clone();
    if let Some(csrf) = &call.csrf {
        path.push(if path.contains('?') { '&' } else { '?' });
        path.push_str(&format!("csrf={}", encode(csrf)));
    }
    let address = format!("ws://127.0.0.1:{}{}", target.port, path);
    let mut request = match address.into_client_request() {
        Ok(request) => request,
        Err(e) => {
            return Answer {
                status: 0,
                text: format!("no upgrade request: {e}"),
                json: None,
            }
        }
    };
    let header = |text: String| text.parse::<HeaderValue>().ok();
    let origin = match call.origin {
        OriginHeader::Own => Some(target.origin(call.site)),
        OriginHeader::OtherSite => Some(target.origin(1 - call.site.min(1))),
        OriginHeader::Absent => None,
    };
    let headers = request.headers_mut();
    if let Some(host) = header(format!("{}:{}", target.sites[call.site].host, target.port)) {
        headers.insert("host", host);
    }
    if let Some(origin) = origin.and_then(header) {
        headers.insert("origin", origin);
    }
    if let Some(cookie) = call
        .cookie
        .as_ref()
        .and_then(|c| header(format!("{SESSION_COOKIE}={c}")))
    {
        headers.insert("cookie", cookie);
    }
    match tokio_tungstenite::connect_async(request).await {
        Ok((_, response)) => Answer {
            status: response.status().as_u16(),
            text: String::new(),
            json: None,
        },
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            let text = response
                .body()
                .as_ref()
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_default();
            Answer {
                status: response.status().as_u16(),
                json: serde_json::from_str(&text).ok(),
                text,
            }
        }
        Err(e) => Answer {
            status: 0,
            text: format!("the upgrade failed: {e}"),
            json: None,
        },
    }
}

/// The request token the site hands a browser without a session.
async fn visitor_token(target: &Target, site: usize) -> Result<String, String> {
    let call = Call {
        method: Method::GET,
        site,
        path: "/api/session".into(),
        origin: OriginHeader::Absent,
        csrf: None,
        cookie: None,
        body: None,
    };
    let answer = send(target, &call).await;
    answer
        .json
        .as_ref()
        .and_then(|j| j["session"]["csrf_token"].as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("no request token from /api/session: {}", answer.summary()))
}

async fn sign_in(target: &Target, persona: &Persona) -> Result<Session, String> {
    let token = visitor_token(target, persona.site).await?;
    let host = format!("{}:{}", target.sites[persona.site].host, target.port);
    let response = target
        .http
        .post(format!("http://127.0.0.1:{}/api/login", target.port))
        .header("host", host)
        .header("origin", target.origin(persona.site))
        .header("x-csrf-token", token)
        .header("content-type", "application/json")
        .body(json!({ "login": persona.login, "password": persona.password }).to_string())
        .send()
        .await
        .map_err(|e| format!("signing in {} failed: {e}", persona.login))?;
    let status = response.status().as_u16();
    let cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|v| v.split(';').next())
        .find_map(|pair| {
            pair.strip_prefix(&format!("{SESSION_COOKIE}="))
                .map(str::to_string)
        })
        .filter(|value| !value.is_empty());
    let text = response.text().await.unwrap_or_default();
    let json: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let csrf = json["session"]["csrf_token"].as_str().map(str::to_string);
    match (status, cookie, csrf) {
        (200, Some(cookie), Some(csrf)) => Ok(Session { cookie, csrf }),
        _ => Err(format!(
            "{} could not sign in: status {status} {}",
            persona.login,
            text.chars().take(200).collect::<String>()
        )),
    }
}

// ---- the setup of a success call -------------------------------------------------------------

/// The code an authenticator with the base32 `secret` shows `steps` time steps from now.
fn totp_code(secret: &str, steps: i64) -> Result<String, String> {
    let bytes = Secret::Encoded(secret.to_string())
        .to_bytes()
        .map_err(|e| format!("the authenticator secret is not base32: {e:?}"))?;
    let totp = TOTP::new(Algorithm::SHA1, 6, 1, 30, bytes)
        .map_err(|e| format!("no authenticator from the secret: {e:?}"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    Ok(totp.generate((now + steps * 30) as u64))
}

/// A call as the caller of the success call makes it: our origin, the session and its token.
fn caller_call(method: Method, path: &str, session: Option<&Session>, body: Option<String>) -> Call {
    Call {
        method,
        site: 0,
        path: path.to_string(),
        origin: OriginHeader::Own,
        csrf: session.map(|s| s.csrf.clone()),
        cookie: session.map(|s| s.cookie.clone()),
        body,
    }
}

/// Puts `value` in the input `name` of a call: a POST's JSON body, a GET's query.
fn set_input(call: &mut Call, name: &str, value: &Value) -> Result<(), String> {
    if call.method == Method::POST {
        let mut body: Value = call
            .body
            .as_deref()
            .and_then(|b| serde_json::from_str(b).ok())
            .unwrap_or_else(|| json!({}));
        let map = body
            .as_object_mut()
            .ok_or_else(|| format!("the body of the call is not an object, for {name}"))?;
        map.insert(name.to_string(), value.clone());
        call.body = Some(body.to_string());
    } else {
        let text = match value {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        call.path.push(if call.path.contains('?') { '&' } else { '?' });
        call.path
            .push_str(&format!("{}={}", encode(name), encode(&text)));
    }
    Ok(())
}

/// Takes the steps of the contract's setup as `session`, and puts what they read in `call`.
async fn run_setup(
    target: &Target,
    contract: &Contract,
    session: Option<&Session>,
    call: &mut Call,
) -> Result<(), String> {
    for step in contract.setup {
        match step {
            Setup::Read { path, set } => {
                let answer = send(target, &caller_call(Method::GET, path, session, None)).await;
                if !(200..300).contains(&answer.status) {
                    return Err(format!("the setup read of {path}: {}", answer.summary()));
                }
                for (input, member) in *set {
                    let mut value = answer.json.as_ref();
                    for part in member.split('.') {
                        value = value.and_then(|v| v.get(part));
                    }
                    let Some(value) = value else {
                        return Err(format!(
                            "the reply of {path} has no {member}: {}",
                            answer.summary()
                        ));
                    };
                    set_input(call, input, value)?;
                }
            }
            // Contract regenerate-recovery-codes (CT-1022): recovery codes stand in for an
            // authenticator, so the caller has one set up first, through the routes the desk uses
            // (`POST /api/factors/totp/start` then `/confirm` with a code of the secret).
            Setup::Step("enrol-authenticator") => {
                let Some(session) = session else {
                    return Err("enrol-authenticator needs a signed-in caller".to_string());
                };
                let started = send(
                    target,
                    &caller_call(
                        Method::POST,
                        "/api/factors/totp/start",
                        Some(session),
                        Some("{}".into()),
                    ),
                )
                .await;
                let secret = started
                    .json
                    .as_ref()
                    .and_then(|j| j["secret"].as_str())
                    .map(str::to_string)
                    .ok_or_else(|| format!("no secret to enrol: {}", started.summary()))?;
                // The code one step back, as a person's app would show it before the page loads.
                let body = json!({ "code": totp_code(&secret, -1)? }).to_string();
                let confirmed = send(
                    target,
                    &caller_call(
                        Method::POST,
                        "/api/factors/totp/confirm",
                        Some(session),
                        Some(body),
                    ),
                )
                .await;
                if !(200..300).contains(&confirmed.status) {
                    return Err(format!(
                        "the authenticator was not confirmed: {}",
                        confirmed.summary()
                    ));
                }
            }
            Setup::Step(name) => return Err(format!("the setup step {name} is not implemented")),
        }
    }
    Ok(())
}

// ---- checking values against the declared types ---------------------------------------------

fn decl_of(spec: &Spec, id: &str) -> Option<Decl> {
    spec.types.iter().find(|t| t.id == id).map(|t| t.decl)
}

/// Every way `v` is not a value of `ty`, each as `<path>: <problem>`.
fn conforms(spec: &Spec, ty: &Ty, v: &Value, at: &str, out: &mut Vec<String>) {
    let wrong = |what: &str| format!("{at}: {what}, not {}", short(v));
    match ty {
        Ty::Any => {}
        Ty::Bool => {
            if !v.is_boolean() {
                out.push(wrong("expected a boolean"));
            }
        }
        Ty::Int(min, max) => {
            let n = v
                .as_i64()
                .map(i128::from)
                .or_else(|| v.as_u64().map(i128::from));
            if !n.is_some_and(|n| n >= *min && n <= *max) {
                out.push(wrong(&format!("expected an integer from {min} to {max}")));
            }
        }
        Ty::Float => {
            if !v.is_number() {
                out.push(wrong("expected a number"));
            }
        }
        Ty::Text => {
            if !v.is_string() {
                out.push(wrong("expected a string"));
            }
        }
        Ty::Bytes => {
            let bytes = v.as_array().is_some_and(|a| {
                a.iter()
                    .all(|b| b.as_u64().is_some_and(|b| b <= u64::from(u8::MAX)))
            });
            if !v.is_string() && !bytes {
                out.push(wrong("expected bytes (a string or an array of bytes)"));
            }
        }
        Ty::List(inner) => match v.as_array() {
            Some(items) => {
                for (i, item) in items.iter().enumerate() {
                    conforms(spec, inner, item, &format!("{at}[{i}]"), out);
                }
            }
            None => out.push(wrong("expected a list")),
        },
        Ty::Map(inner) => match v.as_object() {
            Some(map) => {
                for (k, item) in map {
                    conforms(spec, inner, item, &format!("{at}.{k}"), out);
                }
            }
            None => out.push(wrong("expected an object")),
        },
        Ty::Option(inner) => {
            if !v.is_null() {
                conforms(spec, inner, v, at, out);
            }
        }
        Ty::Named(id) => match decl_of(spec, id) {
            None => out.push(format!("{at}: the type {id} is not in the suite")),
            Some(Decl::Alias(t)) => conforms(spec, &t, v, at, out),
            Some(Decl::Enum(values)) => {
                if !v.as_str().is_some_and(|s| values.contains(&s)) {
                    out.push(format!(
                        "{at}: expected one of {} ({id}), not {}",
                        values.join(", "),
                        short(v)
                    ));
                }
            }
            Some(Decl::Struct(fields)) => match v.as_object() {
                Some(map) => members(spec, fields, map, &[], at, out),
                None => out.push(format!("{at}: expected an object ({id}), not {}", short(v))),
            },
            Some(Decl::Union(tag, variants)) => union(spec, id, tag, variants, v, at, out),
        },
    }
}

/// The members of an object against the fields of a struct: each required one present, each
/// present one of its type, and none that is not declared (`ignored` names members a union's tag
/// holds).
fn members(
    spec: &Spec,
    fields: &[Field],
    map: &Map<String, Value>,
    ignored: &[&str],
    at: &str,
    out: &mut Vec<String>,
) {
    for f in fields {
        match map.get(f.wire) {
            None | Some(Value::Null) if f.optional => {}
            None => out.push(format!("{at}.{}: missing", f.wire)),
            Some(v) => conforms(spec, &f.ty, v, &format!("{at}.{}", f.wire), out),
        }
    }
    for k in map.keys() {
        if !fields.iter().any(|f| f.wire == k) && !ignored.contains(&k.as_str()) {
            out.push(format!("{at}.{k}: not a declared member"));
        }
    }
}

fn content(spec: &Spec, c: &Content, v: &Value, at: &str, out: &mut Vec<String>) {
    match c {
        Content::Unit => out.push(format!(
            "{at}: a unit variant holds nothing, not {}",
            short(v)
        )),
        Content::Newtype(t) => conforms(spec, t, v, at, out),
        Content::Tuple(ts) => match v.as_array() {
            Some(items) if items.len() == ts.len() => {
                for (i, (t, item)) in ts.iter().zip(items).enumerate() {
                    conforms(spec, t, item, &format!("{at}[{i}]"), out);
                }
            }
            _ => out.push(format!(
                "{at}: expected {} values, not {}",
                ts.len(),
                short(v)
            )),
        },
        Content::Fields(fields) => match v.as_object() {
            Some(map) => members(spec, fields, map, &[], at, out),
            None => out.push(format!("{at}: expected an object, not {}", short(v))),
        },
    }
}

fn union(
    spec: &Spec,
    id: &str,
    tag: Tag,
    variants: &[Variant],
    v: &Value,
    at: &str,
    out: &mut Vec<String>,
) {
    let find = |name: &str| variants.iter().find(|x| x.wire == name);
    match tag {
        Tag::External => match v {
            Value::String(name) => match find(name) {
                Some(Variant {
                    content: Content::Unit,
                    ..
                }) => {}
                _ => out.push(format!("{at}: \"{name}\" is not a unit variant of {id}")),
            },
            Value::Object(map) if map.len() == 1 => {
                let (name, inner) = map.iter().next().expect("one member");
                match find(name) {
                    Some(variant) => {
                        content(spec, &variant.content, inner, &format!("{at}.{name}"), out)
                    }
                    None => out.push(format!("{at}: \"{name}\" is not a variant of {id}")),
                }
            }
            _ => out.push(format!(
                "{at}: expected a variant of {id}, not {}",
                short(v)
            )),
        },
        Tag::Internal(field) => {
            let Some(map) = v.as_object() else {
                out.push(format!("{at}: expected an object ({id}), not {}", short(v)));
                return;
            };
            match map.get(field).and_then(Value::as_str).and_then(find) {
                Some(variant) => match variant.content {
                    Content::Unit => members(spec, &[], map, &[field], at, out),
                    Content::Fields(fields) => members(spec, fields, map, &[field], at, out),
                    Content::Newtype(Ty::Named(inner)) => {
                        if let Some(Decl::Struct(fields)) = decl_of(spec, inner) {
                            members(spec, fields, map, &[field], at, out);
                        }
                    }
                    _ => {}
                },
                None => out.push(format!("{at}.{field}: not a variant of {id}")),
            }
        }
        Tag::Adjacent(field, content_field) => {
            let Some(map) = v.as_object() else {
                out.push(format!("{at}: expected an object ({id}), not {}", short(v)));
                return;
            };
            match map.get(field).and_then(Value::as_str).and_then(find) {
                Some(variant) => {
                    if let Some(inner) = map.get(content_field) {
                        content(
                            spec,
                            &variant.content,
                            inner,
                            &format!("{at}.{content_field}"),
                            out,
                        );
                    }
                    for k in map.keys() {
                        if k != field && k != content_field {
                            out.push(format!("{at}.{k}: not a declared member"));
                        }
                    }
                }
                None => out.push(format!("{at}.{field}: not a variant of {id}")),
            }
        }
    }
}

fn short(v: &Value) -> String {
    let text = v.to_string();
    if text.chars().count() > 80 {
        format!("{}...", text.chars().take(80).collect::<String>())
    } else {
        text
    }
}

// ---- one contract ---------------------------------------------------------------------------

/// The capabilities a person holds by the specification's roles.
fn holdings(spec: &Spec, roles: &[String], signed_in: bool) -> BTreeSet<&'static str> {
    let implicit = if signed_in {
        SIGNED_IN_ROLES
    } else {
        ANONYMOUS_ROLES
    };
    spec.roles
        .iter()
        .filter(|(id, _)| implicit.contains(id) || roles.iter().any(|r| r == id))
        .flat_map(|(_, caps)| caps.iter().copied())
        .collect()
}

struct Run<'a> {
    spec: &'a Spec,
    contract: &'a Contract,
    target: &'a Target,
    failures: Vec<(String, String)>,
    skips: Vec<(String, String)>,
    sessions: BTreeMap<String, Session>,
}

/// Checks one contract against a fresh target and panics with every failure, each as
/// `[rule] what happened`.
pub async fn conform(spec: &Spec, contract: &Contract) {
    let target = super::start_target().await;
    let mut run = Run {
        spec,
        contract,
        target: &target,
        failures: Vec::new(),
        skips: Vec::new(),
        sessions: BTreeMap::new(),
    };
    run.no_session().await;
    if contract.origin {
        run.origin().await;
    }
    if contract.csrf {
        run.csrf().await;
    }
    if contract.scheme == Scheme::Session {
        run.capabilities().await;
        run.roles().await;
        run.other_tenant_session().await;
    }
    if contract.tenant_input.is_some() {
        run.other_tenant_input().await;
    }
    if contract.step_up {
        run.step_up().await;
    }
    if contract.read_only {
        run.read_only().await;
    }
    run.success().await;
    if let Some(limit) = contract.quota {
        run.quota(limit).await;
    }
    run.finish();
}

impl Run<'_> {
    fn fail(&mut self, rule: &str, detail: String) {
        self.failures.push((rule.to_string(), detail));
    }

    fn skip(&mut self, check: &str, reason: String) {
        self.skips.push((check.to_string(), reason));
    }

    fn finish(self) {
        for (contract, check, reason) in self.spec.skipped {
            if *contract == self.contract.id {
                eprintln!("conformance skip: {contract} {check}: {reason}");
            }
        }
        for (check, reason) in &self.skips {
            eprintln!("conformance skip: {} {check}: {reason}", self.contract.id);
        }
        if !self.failures.is_empty() {
            let lines: Vec<String> = self
                .failures
                .iter()
                .map(|(rule, detail)| format!("[{rule}] {detail}"))
                .collect();
            panic!(
                "contract {} does not conform ({} failure(s)):\n{}",
                self.contract.id,
                lines.len(),
                lines.join("\n")
            );
        }
    }

    fn home_personas(&self) -> Vec<&Persona> {
        self.target
            .personas
            .iter()
            .filter(|p| p.site == 0)
            .collect()
    }

    fn holds(&self, persona: &Persona, capability: &str) -> bool {
        holdings(self.spec, &persona.roles, true).contains(capability)
    }

    /// Whether the person holds what the contract requires: every capability when it names one,
    /// and any one of them when it names several (D-6591).
    fn holds_required(&self, persona: &Persona) -> bool {
        match self.contract.requires {
            [] => true,
            [only] => self.holds(persona, only),
            several => several.iter().any(|c| self.holds(persona, c)),
        }
    }

    fn holds_role(&self, persona: &Persona) -> bool {
        self.contract.roles.is_empty()
            || persona
                .roles
                .iter()
                .any(|r| self.contract.roles.contains(&r.as_str()))
    }

    fn admitted(&self, persona: &Persona) -> bool {
        self.holds_required(persona) && self.holds_role(persona)
    }

    /// A person of the site who holds everything the contract asks for; the owner first.
    fn able(&self) -> Option<Persona> {
        let mut able: Vec<&Persona> = self
            .home_personas()
            .into_iter()
            .filter(|p| self.admitted(p))
            .collect();
        able.sort_by_key(|p| !p.roles.iter().any(|r| r == "owner"));
        able.first().map(|p| (*p).clone())
    }

    async fn session(&mut self, persona: &Persona) -> Option<Session> {
        if let Some(session) = self.sessions.get(&persona.login) {
            return Some(session.clone());
        }
        match sign_in(self.target, persona).await {
            Ok(session) => {
                self.sessions.insert(persona.login.clone(), session.clone());
                Some(session)
            }
            Err(why) => {
                self.fail("sign-in", why);
                None
            }
        }
    }

    /// The path of a request with its query, the tenant input (on a GET) set to `tenant`.
    fn path(&self, request: &Request, tenant: Option<&str>) -> String {
        let mut pairs: Vec<(String, String)> = request
            .query
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        if let (Some(name), Some(tenant), "GET") =
            (self.contract.tenant_input, tenant, self.contract.method)
        {
            pairs.retain(|(k, _)| k != name);
            pairs.push((name.to_string(), tenant.to_string()));
        }
        if pairs.is_empty() {
            request.path.to_string()
        } else {
            format!(
                "{}?{}",
                request.path,
                pairs
                    .iter()
                    .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
                    .collect::<Vec<_>>()
                    .join("&")
            )
        }
    }

    /// The body of a request (a POST's), the tenant input set to `tenant`.
    fn body(&self, request: &Request, tenant: Option<&str>) -> Option<String> {
        if self.contract.method != "POST" {
            return None;
        }
        let mut body: Value = request
            .body
            .and_then(|b| serde_json::from_str(b).ok())
            .unwrap_or_else(|| json!({}));
        if let (Some(name), Some(tenant), Some(map)) =
            (self.contract.tenant_input, tenant, body.as_object_mut())
        {
            map.insert(name.to_string(), json!(tenant));
        }
        Some(body.to_string())
    }

    fn method(&self) -> Method {
        if self.contract.method == "POST" {
            Method::POST
        } else {
            Method::GET
        }
    }

    /// A call of the contract at this site with `request`, its tenant input naming `tenant`.
    fn call(
        &self,
        request: &Request,
        tenant: &str,
        session: Option<&Session>,
        csrf: Option<String>,
    ) -> Call {
        Call {
            method: self.method(),
            site: 0,
            path: self.path(request, Some(tenant)),
            origin: OriginHeader::Own,
            csrf: csrf.or_else(|| session.map(|s| s.csrf.clone())),
            cookie: session.map(|s| s.cookie.clone()),
            body: self.body(request, Some(tenant)),
        }
    }

    fn home(&self) -> String {
        self.target.sites[0].tenant.clone()
    }

    async fn answer(&mut self, call: &Call) -> Answer {
        if self.contract.shape == Shape::Socket {
            upgrade(self.target, call).await
        } else {
            send(self.target, call).await
        }
    }

    /// Holds every answer to the declaration: a success to the outputs, a refusal to the error
    /// body and the declared errors. `what` names the call in a failure.
    fn judge(&mut self, what: &str, answer: &Answer) {
        let socket = self.contract.shape == Shape::Socket;
        match answer.status {
            101 if socket => {}
            200..=299 => self.reply(what, answer),
            0 => self.fail("refusal-shape", format!("{what}: {}", answer.text)),
            _ => self.refusal(what, answer),
        }
    }

    fn reply(&mut self, what: &str, answer: &Answer) {
        let mut problems = Vec::new();
        match self.contract.reply {
            Reply::Socket => problems.push(format!(
                "a socket was answered {} instead of being upgraded",
                answer.status
            )),
            Reply::Members(fields) => match answer.json.as_ref().and_then(Value::as_object) {
                Some(map) => members(self.spec, fields, map, &[], "reply", &mut problems),
                None => problems.push(format!(
                    "the reply is not a JSON object: {}",
                    answer.summary()
                )),
            },
            Reply::Records(outputs) => match answer.json.as_ref().and_then(Value::as_object) {
                Some(map) => {
                    for k in map.keys() {
                        if !matches!(k.as_str(), "id" | "version" | "fields") {
                            problems.push(format!("reply.{k}: not a member of a records reply"));
                        }
                    }
                    for output in outputs {
                        let (value, at) = match output.from {
                            RecordFrom::Id => (map.get("id"), "reply.id".to_string()),
                            RecordFrom::Version => {
                                (map.get("version"), "reply.version".to_string())
                            }
                            RecordFrom::Field(wire) => (
                                map.get("fields").and_then(|f| f.get(wire)),
                                format!("reply.fields.{wire}"),
                            ),
                        };
                        match value {
                            None | Some(Value::Null) if output.optional => {}
                            None => problems.push(format!("{at}: missing")),
                            Some(v) => conforms(self.spec, &output.ty, v, &at, &mut problems),
                        }
                    }
                }
                None => problems.push(format!(
                    "the reply is not a JSON object: {}",
                    answer.summary()
                )),
            },
        }
        for problem in problems {
            self.fail("reply-shape", format!("{what}: {problem}"));
        }
    }

    /// The code of a refusal: its body's, or for a refused socket upgrade whose body the client did
    /// not read, the kind its status belongs to.
    fn code_of(&self, answer: &Answer) -> Option<String> {
        if let Some(code) = answer.code() {
            return Some(code.to_string());
        }
        if self.contract.shape == Shape::Socket && answer.text.is_empty() {
            return self
                .spec
                .error_status
                .iter()
                .find(|(_, status)| *status == answer.status)
                .map(|(kind, _)| (*kind).to_string());
        }
        None
    }

    fn refusal(&mut self, what: &str, answer: &Answer) {
        if self.contract.shape == Shape::Socket && answer.text.is_empty() {
            self.skip(
                "refusal-shape",
                format!(
                    "{what}: the client read no body of the refused upgrade (status {})",
                    answer.status
                ),
            );
            return;
        }
        let Some(json) = answer.json.clone() else {
            self.fail(
                "refusal-shape",
                format!("{what}: the refusal is not JSON: {}", answer.summary()),
            );
            return;
        };
        let mut problems = Vec::new();
        if let Some(body) = self.spec.error_body {
            conforms(self.spec, &Ty::Named(body), &json, "refusal", &mut problems);
        }
        for problem in problems {
            self.fail("refusal-shape", format!("{what}: {problem}"));
        }
        let Some(code) = answer.code().map(str::to_string) else {
            self.fail(
                "refusal-shape",
                format!("{what}: the refusal has no code: {}", answer.summary()),
            );
            return;
        };
        match self.spec.error_status.iter().find(|(k, _)| *k == code) {
            Some((_, status)) if *status == answer.status => {}
            Some((_, status)) => self.fail(
                "refusal-shape",
                format!(
                    "{what}: the code {code} has the status {status}, and was answered with {}",
                    answer.status
                ),
            ),
            None => self.fail(
                "refusal-shape",
                format!("{what}: {code} is not an error kind"),
            ),
        }
        if !self.contract.errors.contains(&code.as_str()) {
            self.fail(
                "declared-error",
                format!(
                    "{what}: refused with {code}, which the contract does not declare (it declares {}): {}",
                    self.contract.errors.join(", "),
                    answer.summary()
                ),
            );
        }
    }

    /// The answer must be a refusal with one of `codes`; every answer is judged as well.
    fn expect_refusal(&mut self, rule: &str, what: &str, answer: &Answer, codes: &[&str]) {
        self.judge(what, answer);
        if (200..300).contains(&answer.status) || answer.status == 101 {
            self.fail(
                rule,
                format!(
                    "{what}: answered {} where {} was expected",
                    answer.status,
                    codes.join(" or ")
                ),
            );
            return;
        }
        match self.code_of(answer) {
            Some(code) if codes.contains(&code.as_str()) => {}
            _ => self.fail(
                rule,
                format!(
                    "{what}: {} where {} was expected",
                    answer.summary(),
                    codes.join(" or ")
                ),
            ),
        }
    }

    // ---- the checks -------------------------------------------------------------------------

    async fn no_session(&mut self) {
        let token = match visitor_token(self.target, 0).await {
            Ok(token) => token,
            Err(why) => return self.fail("no-session", why),
        };
        let call = self.call(&self.contract.probe, &self.home(), None, Some(token));
        let answer = self.answer(&call).await;
        match self.contract.scheme {
            Scheme::Anonymous => self.judge("a call without a session", &answer),
            Scheme::Session => self.expect_refusal(
                "no-session",
                "a call without a session",
                &answer,
                &["unauthenticated"],
            ),
            Scheme::Other(scheme) => {
                // An address that does not verify is not found, not refused for lack of a
                // credential, when the contract declares that: the answer does not say whether
                // the address was never ours or its signature is wrong.
                let signed_address_not_found = scheme == "signed-address"
                    && self.contract.errors.contains(&"not_found");
                let codes: &[&str] = if signed_address_not_found {
                    &["unauthenticated", "not_found"]
                } else {
                    &["unauthenticated"]
                };
                self.expect_refusal(
                    "no-credential",
                    &format!("a call without the {scheme} credential"),
                    &answer,
                    codes,
                )
            }
        }
    }

    /// The caller of a check that must be refused for something else than who it is: a person who
    /// holds everything (any person of the site when none does), or a visitor.
    async fn caller(&mut self) -> Option<(Option<Session>, String)> {
        match self.contract.scheme {
            Scheme::Session => {
                let persona = self
                    .able()
                    .or_else(|| self.home_personas().first().map(|p| (*p).clone()));
                let Some(persona) = persona else {
                    self.skip("caller", "the site has no person to sign in".into());
                    return None;
                };
                let session = self.session(&persona).await?;
                let csrf = session.csrf.clone();
                Some((Some(session), csrf))
            }
            _ => match visitor_token(self.target, 0).await {
                Ok(token) => Some((None, token)),
                Err(why) => {
                    self.fail("sign-in", why);
                    None
                }
            },
        }
    }

    async fn origin(&mut self) {
        let Some((session, csrf)) = self.caller().await else {
            return;
        };
        for (origin, what) in [
            (
                OriginHeader::Absent,
                "a call without an origin and with a body that is not JSON",
            ),
            (
                OriginHeader::OtherSite,
                "a call from the other site's origin with a body that is not JSON",
            ),
        ] {
            let mut call = self.call(
                &self.contract.probe,
                &self.home(),
                session.as_ref(),
                Some(csrf.clone()),
            );
            call.origin = origin;
            if call.body.is_some() {
                call.body = Some(MALFORMED_BODY.to_string());
            }
            let answer = self.answer(&call).await;
            self.expect_refusal("origin", what, &answer, &["forbidden"]);
        }
    }

    async fn csrf(&mut self) {
        let Some((session, _)) = self.caller().await else {
            return;
        };
        // A token the site issued, but not the session's; for a visitor, a made-up one.
        let wrong = match &session {
            Some(_) => match visitor_token(self.target, 0).await {
                Ok(token) => token,
                Err(why) => return self.fail("csrf", why),
            },
            None => "0000".to_string(),
        };
        for (csrf, what) in [
            (
                None,
                "a call without the request token and with a body that is not JSON",
            ),
            (
                Some(wrong),
                "a call with a request token that is not the session's and a body that is not JSON",
            ),
        ] {
            let mut call = self.call(&self.contract.probe, &self.home(), session.as_ref(), None);
            call.csrf = csrf;
            if call.body.is_some() {
                call.body = Some(MALFORMED_BODY.to_string());
            }
            let answer = self.answer(&call).await;
            self.expect_refusal("csrf", what, &answer, &["forbidden"]);
        }
    }

    async fn capabilities(&mut self) {
        // A contract that names several capabilities is met by any one of them (D-6591), so the
        // refused person lacks all of them; one that names a single capability is checked alone.
        let groups: Vec<&[&str]> = match self.contract.requires {
            [] => Vec::new(),
            requires => vec![requires],
        };
        for group in groups {
            let capability = group.join(" or ");
            let capability = capability.as_str();
            let mut lacking: Vec<Persona> = self
                .home_personas()
                .into_iter()
                .filter(|p| group.iter().all(|c| !self.holds(p, c)))
                .cloned()
                .collect();
            // One who holds the role the contract names is refused for the capability alone.
            lacking.sort_by_key(|p| !self.holds_role(p));
            let Some(persona) = lacking.first().cloned() else {
                self.skip(
                    "capability",
                    format!("no person of {} lacks {capability}", self.home()),
                );
                continue;
            };
            let Some(session) = self.session(&persona).await else {
                continue;
            };
            let call = self.call(&self.contract.probe, &self.home(), Some(&session), None);
            let answer = self.answer(&call).await;
            self.expect_refusal(
                "capability",
                &format!("a call by {}, who lacks {capability}", persona.login),
                &answer,
                &["forbidden"],
            );
        }
    }

    async fn roles(&mut self) {
        if self.contract.roles.is_empty() {
            return;
        }
        let mut lacking: Vec<Persona> = self
            .home_personas()
            .into_iter()
            .filter(|p| !self.holds_role(p))
            .cloned()
            .collect();
        lacking.sort_by_key(|p| !self.holds_required(p));
        let Some(persona) = lacking.first().cloned() else {
            self.skip(
                "role",
                format!(
                    "every person of {} holds one of {}",
                    self.home(),
                    self.contract.roles.join(", ")
                ),
            );
            return;
        };
        let Some(session) = self.session(&persona).await else {
            return;
        };
        let call = self.call(&self.contract.probe, &self.home(), Some(&session), None);
        let answer = self.answer(&call).await;
        self.expect_refusal(
            "role",
            &format!(
                "a call by {}, who holds none of {}",
                persona.login,
                self.contract.roles.join(", ")
            ),
            &answer,
            &["forbidden"],
        );
    }

    async fn other_tenant_session(&mut self) {
        let mut others: Vec<Persona> = self
            .target
            .personas
            .iter()
            .filter(|p| p.site == 1)
            .cloned()
            .collect();
        others.sort_by_key(|p| !self.admitted(p));
        let Some(persona) = others.first().cloned() else {
            self.skip(
                "other-tenant-session",
                format!("{} has no person to sign in", self.target.sites[1].tenant),
            );
            return;
        };
        let session = match sign_in(self.target, &persona).await {
            Ok(session) => session,
            Err(why) => return self.fail("sign-in", why),
        };
        let call = self.call(&self.contract.probe, &self.home(), Some(&session), None);
        let answer = self.answer(&call).await;
        self.expect_refusal(
            "other-tenant-session",
            &format!(
                "a call with a session of {} ({}) at {}",
                self.target.sites[1].tenant, persona.login, self.target.sites[0].host
            ),
            &answer,
            &["unauthenticated", "forbidden"],
        );
    }

    async fn other_tenant_input(&mut self) {
        let Some((session, csrf)) = self.caller().await else {
            return;
        };
        let other = self.target.sites[1].tenant.clone();
        let call = self.call(&self.contract.probe, &other, session.as_ref(), Some(csrf));
        let answer = self.answer(&call).await;
        self.expect_refusal(
            "other-tenant-input",
            &format!(
                "a call whose {} names {other}",
                self.contract.tenant_input.unwrap_or("tenant")
            ),
            &answer,
            &["forbidden"],
        );
    }

    async fn step_up(&mut self) {
        if self.contract.scheme != Scheme::Session {
            return self.skip("step-up", "the contract has no session to age".into());
        }
        let Some(persona) = self.able() else {
            return self.skip(
                "step-up",
                format!(
                    "no person of {} holds what the contract asks for",
                    self.home()
                ),
            );
        };
        // A session of its own: ageing it must not touch the ones the other checks use.
        let session = match sign_in(self.target, &persona).await {
            Ok(session) => session,
            Err(why) => return self.fail("sign-in", why),
        };
        let age = Call {
            method: Method::POST,
            site: 0,
            path: AGE_AUTHENTICATION.into(),
            origin: OriginHeader::Own,
            csrf: Some(session.csrf.clone()),
            cookie: Some(session.cookie.clone()),
            body: Some("{}".into()),
        };
        let aged = send(self.target, &age).await;
        if aged.status != 200 {
            return self.fail(
                "step-up",
                format!("the authentication could not be aged: {}", aged.summary()),
            );
        }
        let call = self.call(&self.contract.probe, &self.home(), Some(&session), None);
        let answer = self.answer(&call).await;
        let what = format!("a call by {} with an aged authentication", persona.login);
        self.expect_refusal("step-up", &what, &answer, &["forbidden", "unauthenticated"]);
        if let Some(kind) = self.contract.step_up_error {
            let given = answer
                .json
                .as_ref()
                .and_then(|j| j.get("kind"))
                .and_then(Value::as_str)
                .map(str::to_string);
            if !(200..300).contains(&answer.status) && given.as_deref() != Some(kind) {
                self.fail(
                    "step-up",
                    format!(
                        "{what}: the refusal's kind is {}, not {kind}",
                        given.as_deref().unwrap_or("absent")
                    ),
                );
            }
        }
    }

    async fn read_only(&mut self) {
        let Some((session, csrf)) = self.caller().await else {
            return;
        };
        let mut call = self.call(
            &self.contract.probe,
            &self.home(),
            session.as_ref(),
            Some(csrf),
        );
        let (method, name) = if self.contract.method == "GET" {
            (Method::POST, "POST")
        } else {
            (Method::DELETE, "DELETE")
        };
        call.method = method;
        if call.body.is_none() {
            call.body = Some("{}".into());
        }
        let what = format!("a {name} of a read-only route");
        let answer = send(self.target, &call).await;
        match answer.status {
            405 => {}
            400..=499 => self.judge(&what, &answer),
            _ => self.fail(
                "read-only",
                format!("{what}: {}, where a refusal was expected", answer.summary()),
            ),
        }
    }

    async fn success(&mut self) {
        let Some(request) = self.contract.success else {
            return;
        };
        let (session, csrf) = match self.contract.scheme {
            Scheme::Session => {
                let Some(persona) = self.able() else {
                    return self.skip(
                        "success",
                        format!(
                            "no person of {} holds what the contract asks for",
                            self.home()
                        ),
                    );
                };
                let Some(session) = self.session(&persona).await else {
                    return;
                };
                let csrf = session.csrf.clone();
                (Some(session), csrf)
            }
            Scheme::Anonymous => match visitor_token(self.target, 0).await {
                Ok(token) => (None, token),
                Err(why) => return self.fail("success", why),
            },
            Scheme::Other(_) => return,
        };
        let mut call = self.call(&request, &self.home(), session.as_ref(), Some(csrf));
        if let Err(why) = run_setup(self.target, self.contract, session.as_ref(), &mut call).await {
            return self.fail("setup", why);
        }
        let answer = self.answer(&call).await;
        let what = "a call with the example inputs by one who may make it";
        self.judge(what, &answer);
        let ok = if self.contract.shape == Shape::Socket {
            answer.status == 101
        } else {
            (200..300).contains(&answer.status)
        };
        if !ok {
            self.fail("success", format!("{what}: {}", answer.summary()));
        }
    }

    async fn quota(&mut self, limit: u64) {
        if limit > QUOTA_CALLS {
            return self.skip("quota", format!("its limit {limit} is above {QUOTA_CALLS}"));
        }
        let Some(request) = self.contract.success else {
            return self.skip("quota", "it has no call that succeeds to repeat".into());
        };
        // A target of its own, so the calls of the other checks do not count.
        let fresh = super::start_target().await;
        let (session, csrf) = match self.contract.scheme {
            Scheme::Session => {
                let Some(persona) = self.able() else {
                    return self.skip("quota", "no person holds what the contract asks for".into());
                };
                match sign_in(&fresh, &persona).await {
                    Ok(session) => {
                        let csrf = session.csrf.clone();
                        (Some(session), csrf)
                    }
                    Err(why) => return self.fail("sign-in", why),
                }
            }
            _ => match visitor_token(&fresh, 0).await {
                Ok(token) => (None, token),
                Err(why) => return self.fail("quota", why),
            },
        };
        let mut call = self.call(&request, &self.home(), session.as_ref(), Some(csrf));
        if let Err(why) = run_setup(&fresh, self.contract, session.as_ref(), &mut call).await {
            return self.fail("setup", why);
        }
        for n in 1..=limit {
            let answer = send(&fresh, &call).await;
            if answer.code() == Some("throttled") || answer.status == 429 {
                return self.fail(
                    "quota",
                    format!(
                        "call {n} of the {limit} the quota allows was throttled: {}",
                        answer.summary()
                    ),
                );
            }
        }
        let answer = send(&fresh, &call).await;
        self.expect_refusal(
            "quota",
            &format!("call {} in the window of a quota of {limit}", limit + 1),
            &answer,
            &["throttled"],
        );
    }
}
