//! Serving contracts over HTTP.
//!
//! A specification `contract` becomes, in generated code, an async trait plus one [`Contract`]
//! type per operation that states its wire shape and its [`ContractSpec`] (path, auth
//! requirement, timeout, idempotency). [`Routes`] turns a handler for that contract into an axum
//! route, so a trait implementation can be served by any process that already uses axum, and
//! [`Client`](crate::Client) calls it from another component.
//!
//! The wire format, for every contract:
//!
//! - `POST <path>` with the request as a JSON body.
//! - Success: status 200, the response as JSON.
//! - Failure: a [`Failure`] as JSON. A contract error has the status its
//!   [`ContractError::status`] gives and `{"kind": "contract", "error": ...}`; a failure of the
//!   runtime itself (authorization, a malformed request, the timeout) has the status of its
//!   [`FailureCode`] and `{"kind": "runtime", "code": ..., "message": ...}`.
//!
//! Who may call is decided by an [`Authorizer`], written by hand per deployment, which reads the
//! contract's `auth` requirement and the request headers.

use crate::telemetry::{NoTelemetry, Span, Telemetry};
use axum::body::Bytes;
use axum::http::{header::CONTENT_TYPE, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The header carrying a request's idempotency key.
pub const IDEMPOTENCY_KEY: &str = "idempotency-key";

/// The header carrying a request's correlation id.
pub const CORRELATION: &str = "x-correlation-id";

/// What the specification states about one contract operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContractSpec {
    /// The contract's reference, such as `CT-012`.
    pub id: &'static str,
    /// The route it is served at, such as `/internal/leases/grant`.
    pub path: &'static str,
    /// The authorization requirement (`auth`), interpreted by the [`Authorizer`].
    pub auth: &'static str,
    /// How long the handler may run (`timeoutMs`). The server answers [`FailureCode::Timeout`]
    /// after this.
    pub timeout: Duration,
    /// Whether calling twice has the effect of calling once (`idempotent`). Only idempotent
    /// contracts are retried by the client and replayed by the server.
    pub idempotent: bool,
}

/// A contract's error type: says which HTTP status it travels with.
pub trait ContractError {
    /// The HTTP status, 400 to 599.
    fn status(&self) -> u16;
}

/// One contract operation's wire shape. Generated code implements this once per operation.
pub trait Contract: Send + Sync + 'static {
    /// What the specification states about it.
    const SPEC: ContractSpec;
    /// The request (`input`).
    type Request: Serialize + DeserializeOwned + Send + Sync + 'static;
    /// The response (`output`).
    type Response: Serialize + DeserializeOwned + Send + 'static;
    /// The contract's errors (`errors`).
    type Error: Serialize + DeserializeOwned + ContractError + Send + 'static;
}

/// A failure the runtime itself reports, as opposed to a contract error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    /// The caller did not say who it is, or said it wrongly (401).
    Unauthenticated,
    /// The caller is known and not allowed (403).
    Forbidden,
    /// The body is not a valid request (400).
    InvalidRequest,
    /// The handler did not finish within the contract's timeout (504).
    Timeout,
    /// Something the handler needs is not available (503).
    Unavailable,
    /// The runtime failed (500).
    Internal,
}

impl FailureCode {
    /// The HTTP status it travels with.
    pub fn status(self) -> u16 {
        match self {
            FailureCode::Unauthenticated => 401,
            FailureCode::Forbidden => 403,
            FailureCode::InvalidRequest => 400,
            FailureCode::Timeout => 504,
            FailureCode::Unavailable => 503,
            FailureCode::Internal => 500,
        }
    }
}

/// The body of every failed call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Failure<E> {
    /// The handler returned one of the contract's errors.
    Contract {
        /// The error.
        error: E,
    },
    /// The runtime refused or failed the call.
    Runtime {
        /// What kind of failure.
        code: FailureCode,
        /// A message safe to show the caller.
        message: String,
    },
}

/// An authorizer's refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    /// [`FailureCode::Unauthenticated`] or [`FailureCode::Forbidden`], as a rule.
    pub code: FailureCode,
    /// A message safe to show the caller.
    pub message: String,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

/// Who is calling, as the authorizer established it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Caller {
    /// Who the caller is, or `None` for a call that needed no identity. Idempotency keys are
    /// scoped to it.
    pub subject: Option<String>,
    /// The request's correlation id, or empty.
    pub correlation: String,
}

/// Decides who may call a contract. Written by hand per deployment.
pub trait Authorizer: Send + Sync + 'static {
    /// Check `headers` against the contract's `requirement` and say who is calling.
    fn authorize(
        &self,
        requirement: &str,
        headers: &HeaderMap,
    ) -> impl Future<Output = Result<Caller, Refusal>> + Send;
}

/// Compare two secrets without stopping at the first difference.
pub fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
}

/// A shared secret in a header: the requirement `none` lets anyone call, any other requirement
/// needs the secret. Also the matching [`Credentials`](crate::Credentials) on the client side.
#[derive(Clone)]
pub struct ServiceToken {
    /// The header that carries the secret.
    pub header: &'static str,
    /// The secret.
    pub secret: String,
}

impl fmt::Debug for ServiceToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceToken")
            .field("header", &self.header)
            .finish_non_exhaustive()
    }
}

impl ServiceToken {
    /// Whether `headers` carry the secret.
    pub fn presented(&self, headers: &HeaderMap) -> bool {
        header(headers, self.header).is_some_and(|given| same_secret(given, &self.secret))
    }
}

impl Authorizer for ServiceToken {
    async fn authorize(&self, requirement: &str, headers: &HeaderMap) -> Result<Caller, Refusal> {
        let correlation = header(headers, CORRELATION).unwrap_or("").to_string();
        if requirement == "none" {
            return Ok(Caller {
                subject: None,
                correlation,
            });
        }
        if self.presented(headers) {
            Ok(Caller {
                subject: Some("service".into()),
                correlation,
            })
        } else {
            Err(Refusal {
                code: FailureCode::Unauthenticated,
                message: "service token required".into(),
            })
        }
    }
}

type ReplayKey = (&'static str, String, String);

/// Answers to idempotent calls, by contract, subject and idempotency key. Bounded: the oldest
/// answer is forgotten first.
struct Replay {
    capacity: usize,
    entries: Mutex<(BTreeMap<ReplayKey, (u16, Bytes)>, VecDeque<ReplayKey>)>,
}

impl Replay {
    fn get(&self, key: &ReplayKey) -> Option<(u16, Bytes)> {
        self.entries.lock().ok()?.0.get(key).cloned()
    }

    fn put(&self, key: ReplayKey, status: u16, body: Bytes) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        let (map, order) = &mut *entries;
        if map.insert(key.clone(), (status, body)).is_none() {
            order.push_back(key);
        }
        while order.len() > self.capacity {
            if let Some(oldest) = order.pop_front() {
                map.remove(&oldest);
            }
        }
    }
}

fn respond(status: u16, body: Bytes) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = (status, body).into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

fn runtime_failure(code: FailureCode, message: impl Into<String>) -> (u16, Bytes) {
    let failure: Failure<()> = Failure::Runtime {
        code,
        message: message.into(),
    };
    let body = serde_json::to_vec(&failure).unwrap_or_default();
    (code.status(), Bytes::from(body))
}

/// Contract handlers collected into an axum router.
pub struct Routes<A: Authorizer> {
    router: Router,
    authorizer: Arc<A>,
    replay: Arc<Replay>,
    telemetry: Arc<dyn Telemetry>,
}

impl<A: Authorizer> Routes<A> {
    /// No routes yet; every call will be checked by `authorizer`.
    pub fn new(authorizer: A) -> Self {
        Routes {
            router: Router::new(),
            authorizer: Arc::new(authorizer),
            replay: Arc::new(Replay {
                capacity: 1024,
                entries: Mutex::new((BTreeMap::new(), VecDeque::new())),
            }),
            telemetry: Arc::new(NoTelemetry),
        }
    }

    /// Report each call to `telemetry`.
    pub fn with_telemetry(mut self, telemetry: Arc<dyn Telemetry>) -> Self {
        self.telemetry = telemetry;
        self
    }

    /// Serve contract `C` with `handler`.
    ///
    /// Per call: the authorizer checks `C::SPEC.auth` (a refusal answers at once); for an
    /// idempotent contract with an `idempotency-key` header, an answer already given to the same
    /// subject under the same key is replayed without running the handler; the body is decoded
    /// (`invalid_request` if it does not decode); the handler runs, and is dropped with a
    /// `timeout` answer if it does not finish within `C::SPEC.timeout`; its answer is recorded
    /// for replay when the contract is idempotent and a key was given.
    pub fn serve<C, F, Fut>(mut self, handler: F) -> Self
    where
        C: Contract,
        F: Fn(Caller, C::Request) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Result<C::Response, C::Error>> + Send + 'static,
    {
        let authorizer = self.authorizer.clone();
        let replay = self.replay.clone();
        let telemetry = self.telemetry.clone();
        let route = move |headers: HeaderMap, body: Bytes| {
            let (authorizer, replay, telemetry, handler) = (
                authorizer.clone(),
                replay.clone(),
                telemetry.clone(),
                handler.clone(),
            );
            async move {
                let span = Span::start(&telemetry, "contract.call");
                let (status, body) =
                    answer::<C, A, F, Fut>(&authorizer, &replay, handler, headers, body).await;
                span.finish(
                    status < 500,
                    &[
                        ("contract", C::SPEC.id.into()),
                        ("status", u64::from(status).into()),
                    ],
                );
                respond(status, body)
            }
        };
        self.router = self.router.route(C::SPEC.path, post(route));
        self
    }

    /// The router, to serve on its own or merge into a larger one.
    pub fn into_router(self) -> Router {
        self.router
    }
}

async fn answer<C, A, F, Fut>(
    authorizer: &A,
    replay: &Replay,
    handler: F,
    headers: HeaderMap,
    body: Bytes,
) -> (u16, Bytes)
where
    C: Contract,
    A: Authorizer,
    F: Fn(Caller, C::Request) -> Fut,
    Fut: Future<Output = Result<C::Response, C::Error>>,
{
    let caller = match authorizer.authorize(C::SPEC.auth, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => return runtime_failure(refusal.code, refusal.message),
    };
    let replay_key = match header(&headers, IDEMPOTENCY_KEY) {
        Some(key) if C::SPEC.idempotent => Some((
            C::SPEC.id,
            caller.subject.clone().unwrap_or_default(),
            key.to_string(),
        )),
        _ => None,
    };
    if let Some(answer) = replay_key.as_ref().and_then(|key| replay.get(key)) {
        return answer;
    }
    let request: C::Request = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => return runtime_failure(FailureCode::InvalidRequest, error.to_string()),
    };
    let encoded = match tokio::time::timeout(C::SPEC.timeout, handler(caller, request)).await {
        Err(_) => {
            return runtime_failure(
                FailureCode::Timeout,
                format!("{} did not finish in time", C::SPEC.id),
            )
        }
        Ok(Ok(response)) => serde_json::to_vec(&response).map(|body| (200, body)),
        Ok(Err(error)) => {
            let status = error.status();
            serde_json::to_vec(&Failure::<&C::Error>::Contract { error: &error })
                .map(|body| (status, body))
        }
    };
    let (status, body) = match encoded {
        Ok((status, body)) => (status, Bytes::from(body)),
        Err(error) => return runtime_failure(FailureCode::Internal, error.to_string()),
    };
    if let Some(key) = replay_key {
        replay.put(key, status, body.clone());
    }
    (status, body)
}
