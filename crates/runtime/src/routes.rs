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
//! - Failure: the status and body the contract's [`FailureBody`] (`Contract::Body`) makes of the
//!   [`Failure`]. With the runtime's own body, [`RuntimeBody`], a contract error has the status its
//!   [`ContractError::status`] gives and `{"kind": "contract", "error": ...}`, and a failure of the
//!   runtime itself (authorization, a malformed request, the timeout) has the status of its
//!   [`FailureCode`] and `{"kind": "runtime", "code": ..., "message": ...}`. A specification that
//!   states an error body has generated code supply a body of its own instead.
//!
//! Who may call is decided by an [`Authorizer`], written by hand per deployment, which reads the
//! contract's `auth` requirement and the request headers. It is the one seam through which a route
//! asks who the caller is. An authorizer refuses a requirement it does not recognise; a contract
//! that states no requirement carries the empty one, which [`ServiceToken`] admits only with the
//! token.

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
    /// How its failures are written on the wire and read back: [`RuntimeBody`] unless the
    /// specification states an error body.
    type Body: FailureBody<Self::Error>;
}

/// How the failures of a contract are written on the wire and read back. The server encodes every
/// failed call with it and the client decodes every answer that is not a success with it.
pub trait FailureBody<E>: Send + Sync + 'static {
    /// The status and JSON body of `failure`, answered to a caller whose correlation id is
    /// `correlation` (empty when it gave none).
    fn encode(failure: Failure<&E>, correlation: &str) -> serde_json::Result<(u16, Vec<u8>)>;
    /// The failure a `body` answered with `status` stands for, or `None` when it is not a body of
    /// this kind.
    fn decode(status: u16, body: &[u8]) -> Option<Failure<E>>;
}

/// The runtime's own failure body: `{"kind": "contract", "error": ...}` with the error's
/// [`ContractError::status`], or `{"kind": "runtime", "code": ..., "message": ...}` with the
/// status of the [`FailureCode`]. The correlation id is not part of it.
#[derive(Clone, Copy, Debug, Default)]
pub struct RuntimeBody;

impl<E> FailureBody<E> for RuntimeBody
where
    E: Serialize + DeserializeOwned + ContractError,
{
    fn encode(failure: Failure<&E>, _correlation: &str) -> serde_json::Result<(u16, Vec<u8>)> {
        let status = match &failure {
            Failure::Contract { error } => error.status(),
            Failure::Runtime { code, .. } => code.status(),
        };
        Ok((status, serde_json::to_vec(&failure)?))
    }

    fn decode(_status: u16, body: &[u8]) -> Option<Failure<E>> {
        serde_json::from_slice(body).ok()
    }
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
    /// Whatever else the authorizer established about the caller, by name, such as the
    /// principal's `kind`, `role` or `tenant`. The runtime never reads it; generated contract
    /// routes read `kind`, `role` and `tenant` for the specification's policies and audits.
    pub attributes: BTreeMap<String, String>,
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
/// needs the secret, and a caller who presents it is subject `service` of kind `service`. Also the matching [`Credentials`](crate::Credentials) on the client side.
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
                attributes: BTreeMap::new(),
            });
        }
        if self.presented(headers) {
            Ok(Caller {
                subject: Some("service".into()),
                correlation,
                attributes: BTreeMap::from([("kind".to_string(), "service".to_string())]),
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

/// The status and body of `failure` in contract `C`'s failure body. A body that cannot be written
/// falls back to an `internal` failure, and that to an empty 500.
fn encoded<C: Contract>(failure: Failure<&C::Error>, correlation: &str) -> (u16, Bytes) {
    match C::Body::encode(failure, correlation) {
        Ok((status, body)) => (status, Bytes::from(body)),
        Err(error) => {
            let internal = Failure::Runtime {
                code: FailureCode::Internal,
                message: error.to_string(),
            };
            match C::Body::encode(internal, correlation) {
                Ok((status, body)) => (status, Bytes::from(body)),
                Err(_) => (500, Bytes::new()),
            }
        }
    }
}

fn runtime_failure<C: Contract>(
    code: FailureCode,
    message: impl Into<String>,
    correlation: &str,
) -> (u16, Bytes) {
    encoded::<C>(
        Failure::Runtime {
            code,
            message: message.into(),
        },
        correlation,
    )
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
    pub fn serve<C, F, Fut>(self, handler: F) -> Self
    where
        C: Contract,
        F: Fn(Caller, C::Request) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Result<C::Response, C::Error>> + Send + 'static,
    {
        self.serve_checked::<C, _, _>(move |caller, request| {
            let answer = handler(caller, request);
            async move { answer.await.map_err(|error| Failure::Contract { error }) }
        })
    }

    /// Serve contract `C` with a `handler` that may also refuse a call itself, with a
    /// [`Failure::Runtime`]: generated routes answer a request that breaks a validation rule with
    /// [`FailureCode::InvalidRequest`] and one the policies refuse with
    /// [`FailureCode::Forbidden`] this way. Everything else is as in [`Routes::serve`]; a runtime
    /// failure is never recorded for replay.
    pub fn serve_checked<C, F, Fut>(mut self, handler: F) -> Self
    where
        C: Contract,
        F: Fn(Caller, C::Request) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Result<C::Response, Failure<C::Error>>> + Send + 'static,
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
    Fut: Future<Output = Result<C::Response, Failure<C::Error>>>,
{
    let caller = match authorizer.authorize(C::SPEC.auth, &headers).await {
        Ok(caller) => caller,
        Err(refusal) => {
            let correlation = header(&headers, CORRELATION).unwrap_or("");
            return runtime_failure::<C>(refusal.code, refusal.message, correlation);
        }
    };
    let correlation = caller.correlation.clone();
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
        Err(error) => {
            return runtime_failure::<C>(
                FailureCode::InvalidRequest,
                error.to_string(),
                &correlation,
            )
        }
    };
    let encoded = match tokio::time::timeout(C::SPEC.timeout, handler(caller, request)).await {
        Err(_) => {
            return runtime_failure::<C>(
                FailureCode::Timeout,
                format!("{} did not finish in time", C::SPEC.id),
                &correlation,
            )
        }
        Ok(Ok(response)) => serde_json::to_vec(&response).map(|body| (200, Bytes::from(body))),
        Ok(Err(Failure::Runtime { code, message })) => {
            return runtime_failure::<C>(code, message, &correlation)
        }
        Ok(Err(Failure::Contract { error })) => Ok(encoded::<C>(
            Failure::Contract { error: &error },
            &correlation,
        )),
    };
    let (status, body) = match encoded {
        Ok(answer) => answer,
        Err(error) => {
            return runtime_failure::<C>(FailureCode::Internal, error.to_string(), &correlation)
        }
    };
    if let Some(key) = replay_key {
        replay.put(key, status, body.clone());
    }
    (status, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    enum Oops {
        #[serde(rename = "busy")]
        Busy,
    }

    impl ContractError for Oops {
        fn status(&self) -> u16 {
            409
        }
    }

    struct Probe;

    impl Contract for Probe {
        const SPEC: ContractSpec = ContractSpec {
            id: "CT-0",
            path: "/probe",
            auth: "",
            timeout: Duration::from_secs(1),
            idempotent: false,
        };
        type Request = ();
        type Response = ();
        type Error = Oops;
        type Body = RuntimeBody;
    }

    #[test]
    fn the_runtime_body_carries_the_error_status_and_reads_back() {
        let (status, body) = encoded::<Probe>(Failure::Contract { error: &Oops::Busy }, "c-1");
        assert_eq!(status, 409);
        assert_eq!(&body[..], br#"{"kind":"contract","error":"busy"}"#);
        assert_eq!(
            <RuntimeBody as FailureBody<Oops>>::decode(status, &body),
            Some(Failure::Contract { error: Oops::Busy })
        );
        let (status, body) = runtime_failure::<Probe>(FailureCode::Forbidden, "no", "c-1");
        assert_eq!(status, 403);
        assert_eq!(
            &body[..],
            br#"{"kind":"runtime","code":"forbidden","message":"no"}"#
        );
    }

    /// A body of its own: the status and the correlation id in a flat object.
    struct Flat;

    impl FailureBody<Oops> for Flat {
        fn encode(
            failure: Failure<&Oops>,
            correlation: &str,
        ) -> serde_json::Result<(u16, Vec<u8>)> {
            let (status, what) = match failure {
                Failure::Contract { error } => (error.status(), "busy".to_string()),
                Failure::Runtime { code, message } => (code.status(), message),
            };
            serde_json::to_vec(&serde_json::json!({ "what": what, "correlation": correlation }))
                .map(|body| (status, body))
        }

        fn decode(status: u16, _body: &[u8]) -> Option<Failure<Oops>> {
            (status == 409).then_some(Failure::Contract { error: Oops::Busy })
        }
    }

    struct FlatProbe;

    impl Contract for FlatProbe {
        const SPEC: ContractSpec = Probe::SPEC;
        type Request = ();
        type Response = ();
        type Error = Oops;
        type Body = Flat;
    }

    #[test]
    fn a_contract_answers_in_its_own_body_with_the_correlation_id() {
        let (status, body) = encoded::<FlatProbe>(Failure::Contract { error: &Oops::Busy }, "c-9");
        assert_eq!(status, 409);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({ "what": "busy", "correlation": "c-9" })
        );
        let (status, body) = runtime_failure::<FlatProbe>(FailureCode::Unavailable, "down", "");
        assert_eq!(status, 503);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({ "what": "down", "correlation": "" })
        );
    }

    #[test]
    fn the_service_token_refuses_an_unstated_requirement_without_the_token() {
        let token = ServiceToken {
            header: "x-token",
            secret: "s".into(),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let empty = HeaderMap::new();
        assert!(runtime.block_on(token.authorize("", &empty)).is_err());
        assert!(runtime.block_on(token.authorize("none", &empty)).is_ok());
    }
}
