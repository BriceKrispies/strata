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
//! Idempotent contracts are replayed. A call to an idempotent contract that carries an
//! `idempotency-key` header is answered once per *scope*: the contract, the tenant, the caller and
//! the key. The tenant is the `tenant` attribute of the [`Caller`] the authorizer established, and
//! only when it set none, the [`TENANT`] header. The caller is [`Caller::subject`] (empty for a call
//! that needed no identity, which then shares one scope per tenant). The request body is part of
//! the call too: its SHA-256 digest is kept with the answer, and the same key with another body is
//! refused with [`FailureCode::IdempotencyConflict`] (409; generated code answers it as the error
//! kind `conflict` unless `errorBody.runtime` maps `idempotency-conflict` elsewhere), never
//! replayed.
//!
//! A key is reserved while its first call runs. A duplicate that arrives meanwhile waits for the
//! first call to finish and gets its answer, for at most the contract's timeout, after which it
//! answers `timeout` and the client's retry waits again. A timeout cancels nothing: the handler
//! runs on its own task to the end, whether or not the caller is still there, and its answer is
//! recorded under the key, so a retry after a `timeout` never runs a half-applied handler a
//! second time but receives the answer when it is ready. A `timeout` answer itself is never
//! recorded, nor is any other runtime failure; the reservation is released so the next call with
//! the key runs the handler. (A handler that panics is such a failure: `internal`.)
//!
//! This table is the replay of the HTTP layer: it lives in memory, is bounded by bytes and is lost
//! when the process ends. It does not make a retried write to a tenant database safe. That is
//! the store's idempotency key (`strata_kernel::Context::idempotency_key`, journaled in the
//! tenant database with the commit and kept for `IDEMPOTENCY_RETENTION_MS`), which holds across a
//! restart and is what a handler that writes through a `Store` relies on; a deadline for the
//! write is available to it as [`crate::server::work_deadline`].
//!
//! The recorded answers are bounded by bytes, not by number: [`Routes::with_replay_bytes`] (8 MiB
//! by default). The oldest answers are forgotten first; an answer larger than the bound is not
//! recorded, and a repeated call then runs the handler again.
//!
//! Who may call is decided by an [`Authorizer`] from the contract's [`Access`]: its [`Exposure`]
//! and its auth, a [`Scheme`] with the roles, CSRF, origin, step-up and secret the specification
//! states, all data, never words. It is the one seam through which a route asks who the caller is.
//! Generated code supplies an authorizer built from the schemes; [`ServiceToken`] is the small one
//! for a service network, which admits the service credential and anonymous calls that ask for no
//! browser check, and refuses every other scheme.
//!
//! Where a contract is served depends on its exposure. [`Routes::new`] is the service network's
//! listener: public and internal contracts. [`Routes::gateway`] faces browsers and the internet:
//! public contracts only, so an internal contract has no route there at all. A local contract is
//! an in-process call and is never served on either.

use crate::telemetry::{NoTelemetry, Span, Telemetry};
use axum::body::Bytes;
use axum::http::{header::CONTENT_TYPE, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tokio::sync::watch;

/// The header carrying a request's idempotency key.
pub const IDEMPOTENCY_KEY: &str = "idempotency-key";

/// The header a request's tenant is read from when the [`Authorizer`] established none.
pub const TENANT: &str = "x-tenant-id";

/// The header carrying a request's correlation id.
pub const CORRELATION: &str = "x-correlation-id";

/// The header carrying the request token of a browser's session or visit (an auth with `csrf`).
pub const CSRF_TOKEN: &str = "x-csrf-token";

/// Where a contract can be reached from (`contract.exposure`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exposure {
    /// Browsers and the internet, through the gateway.
    Public,
    /// The service network only.
    Internal,
    /// An in-process call; never served on a route.
    Local,
}

/// How a caller proves who it is (`contract.auth.scheme`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    /// Nothing: anyone may call.
    Anonymous,
    /// A session established by signing in.
    Session,
    /// The credential of a platform component.
    Service,
    /// The provisioning token of one realm.
    ProvisioningToken,
    /// A request signed with the named secret.
    Signature,
    /// An address signed with the named secret.
    SignedAddress,
    /// A token an agent holds on behalf of a person.
    Delegation,
    /// A local call inside the process.
    InProcess,
}

/// What tenant data a contract reads and returns (`contract.tenantScope`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TenantScope {
    /// Only the tenant the request is made in; a principal's own tenant must be that one.
    Request,
    /// No tenant's data.
    None,
    /// Data across tenants.
    Platform,
}

/// Who may call a contract and from where, as the specification states it (`contract.exposure`,
/// `contract.auth`, `contract.tenantScope`, `contract.development`). Every member is data the
/// authorizer checks; a member left at its empty value asks for no check of its own, and the
/// scheme alone decides what credential is needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Access {
    /// Where it can be reached from.
    pub exposure: Exposure,
    /// How the caller proves who it is.
    pub scheme: Scheme,
    /// The roles any one of which admits the caller; empty when the scheme alone decides.
    pub roles: &'static [&'static str],
    /// The capabilities a call needs, every one (`auth.requires`): the generated authorizer
    /// checks the tenant's plan and flags, the principal's permissions and the delegation it
    /// acts under against them. Empty when the contract names none.
    pub requires: &'static [&'static str],
    /// Whether the tenant's own policies decide in the handler (the route then only establishes
    /// the principal).
    pub tenant_policies: bool,
    /// Whether a browser call carries the request token of its session or visit
    /// ([`CSRF_TOKEN`]).
    pub csrf: bool,
    /// Whether a browser call comes from an origin of the site it addresses.
    pub origin: bool,
    /// How recently the caller must have proved who it is, when the contract asks for it.
    pub step_up: Option<Duration>,
    /// The name of the secret a signature or a signed address is verified with.
    pub secret: Option<&'static str>,
    /// What tenant data it returns, when the specification states it.
    pub tenant_scope: Option<TenantScope>,
    /// The development condition without which it does not exist, for a development fixture.
    pub development: Option<&'static str>,
}

impl Access {
    /// The access of an in-process call: local, no credential.
    pub const LOCAL: Access = Access {
        exposure: Exposure::Local,
        scheme: Scheme::InProcess,
        roles: &[],
        requires: &[],
        tenant_policies: false,
        csrf: false,
        origin: false,
        step_up: None,
        secret: None,
        tenant_scope: None,
        development: None,
    };

    /// An internal contract taking the service credential.
    pub const SERVICE: Access = Access {
        exposure: Exposure::Internal,
        scheme: Scheme::Service,
        ..Access::LOCAL
    };

    /// An internal contract anyone on the service network may call, such as a health check.
    pub const INTERNAL_ANONYMOUS: Access = Access {
        exposure: Exposure::Internal,
        scheme: Scheme::Anonymous,
        ..Access::LOCAL
    };
}

/// What a request presents to the authorizer: its headers, its target (path and query) and its
/// body, for a scheme that verifies a signature over it.
#[derive(Clone, Copy, Debug)]
pub struct Presented<'a> {
    /// The request headers.
    pub headers: &'a HeaderMap,
    /// The path and query the request was made to.
    pub target: &'a str,
    /// The request body, as received.
    pub body: &'a [u8],
}

impl<'a> Presented<'a> {
    /// The value of header `name`, when it is present, text and not empty.
    pub fn header(&self, name: &str) -> Option<&'a str> {
        header(self.headers, name)
    }

    /// The bearer token of the `authorization` header, when it carries one.
    pub fn bearer(&self) -> Option<&'a str> {
        self.header("authorization")
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|token| !token.is_empty())
    }
}

/// A site the gateway serves, as the deployment knows it: the tenant whose site it is and the
/// origins its pages are served from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Site {
    /// The tenant whose site it is.
    pub tenant: String,
    /// The origins (`scheme://host[:port]`) its pages are served from.
    pub origins: Vec<String>,
}

/// Who a credential established, as the deployment's lookup returns it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Principal {
    /// Its id.
    pub subject: String,
    /// What kind of principal it is, such as `person`, `agent` or `service`.
    pub kind: String,
    /// The tenant it belongs to, when it belongs to one.
    pub tenant: Option<String>,
    /// The roles it holds.
    pub roles: Vec<String>,
    /// When it last proved who it is, in milliseconds since the Unix epoch, when known.
    pub authenticated_at_ms: Option<i64>,
    /// The request token of its session, for a session.
    pub csrf_token: Option<String>,
}

/// One route through the gateway: the path of a public contract, the contract's reference and
/// the component that serves it. Generated code lists exactly the public contracts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GatewayRoute {
    /// The path it is served at.
    pub path: &'static str,
    /// The contract's reference, such as `CT-012`.
    pub contract: &'static str,
    /// The component that serves it.
    pub component: &'static str,
}

/// What the specification states about one contract operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContractSpec {
    /// The contract's reference, such as `CT-012`.
    pub id: &'static str,
    /// The route it is served at, such as `/internal/leases/grant`.
    pub path: &'static str,
    /// Who may call it and from where, checked by the [`Authorizer`].
    pub access: Access,
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
    /// An idempotency key was reused with a different request (409).
    IdempotencyConflict,
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
            FailureCode::IdempotencyConflict => 409,
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

/// Decides who may call a contract. Generated from the specification's schemes, or written by
/// hand for a small service network ([`ServiceToken`]).
pub trait Authorizer: Send + Sync + 'static {
    /// Check what `request` presents against the contract's `access` and say who is calling. An
    /// authorizer refuses any scheme it cannot check in full.
    fn authorize(
        &self,
        access: &Access,
        request: &Presented<'_>,
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

/// A shared secret in a header, for a service network: [`Scheme::Service`] needs the secret, and
/// a caller who presents it is subject `service` of kind `service`; [`Scheme::Anonymous`] admits
/// anyone when the contract asks for no CSRF or origin check, which this authorizer cannot make;
/// every other scheme, anything local, and any contract asking for roles, step-up or a
/// development condition is refused. It does not read [`Access::requires`]: the capabilities a
/// service or an anonymous probe needs are fixed by the specification's platform roles, and only
/// the generated authorizer checks them. Also the matching [`Credentials`](crate::Credentials) on
/// the client side.
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
    async fn authorize(&self, access: &Access, request: &Presented<'_>) -> Result<Caller, Refusal> {
        let correlation = request.header(CORRELATION).unwrap_or("").to_string();
        let refused = |message: &str| Refusal {
            code: FailureCode::Forbidden,
            message: message.into(),
        };
        if access.exposure == Exposure::Local {
            return Err(refused("a local contract is not served on a route"));
        }
        if access.development.is_some() || !access.roles.is_empty() || access.step_up.is_some() {
            return Err(refused(
                "this service network cannot check what the contract asks",
            ));
        }
        match access.scheme {
            Scheme::Anonymous if !access.csrf && !access.origin => Ok(Caller {
                subject: None,
                correlation,
                attributes: BTreeMap::new(),
            }),
            Scheme::Service if self.presented(request.headers) => Ok(Caller {
                subject: Some("service".into()),
                correlation,
                attributes: BTreeMap::from([("kind".to_string(), "service".to_string())]),
            }),
            Scheme::Service => Err(Refusal {
                code: FailureCode::Unauthenticated,
                message: "service token required".into(),
            }),
            Scheme::Anonymous
            | Scheme::Session
            | Scheme::ProvisioningToken
            | Scheme::Signature
            | Scheme::SignedAddress
            | Scheme::Delegation
            | Scheme::InProcess => Err(refused("this service network does not admit the scheme")),
        }
    }
}

/// The scope of an idempotency key: contract, tenant, caller subject and the key itself.
type ReplayKey = (&'static str, String, String, String);

/// A SHA-256 digest of a request body.
type Digest32 = [u8; 32];

/// The default bound of the replay cache, in bytes.
const DEFAULT_REPLAY_BYTES: usize = 8 * 1024 * 1024;

/// What a recorded answer is charged besides its key and body: the entry's own bookkeeping.
const ENTRY_OVERHEAD: usize = 96;

fn digest_of(body: &[u8]) -> Digest32 {
    let mut out = [0u8; 32];
    out.copy_from_slice(Sha256::digest(body).as_slice());
    out
}

enum Slot {
    /// The first call is running. Removing the slot drops `done`, which wakes every waiter.
    Pending {
        digest: Digest32,
        done: watch::Sender<()>,
    },
    /// The recorded answer.
    Done {
        digest: Digest32,
        status: u16,
        body: Bytes,
    },
}

#[derive(Default)]
struct ReplayState {
    entries: BTreeMap<ReplayKey, Slot>,
    /// The recorded answers, oldest first. Reserved keys are not in it and cost nothing.
    order: VecDeque<ReplayKey>,
    /// What the recorded answers cost together.
    bytes: usize,
}

/// What [`Replay::claim`] found.
enum Claim {
    /// Nothing was known of the key: it is now reserved for the caller.
    Reserved(Reservation),
    /// The same request was answered before.
    Replay(u16, Bytes),
    /// The key was used with another request.
    Conflict,
    /// The same request is running; the receiver reports when it is over.
    Wait(watch::Receiver<()>),
}

/// Answers to idempotent calls by scope, and the keys of the calls still running. Bounded by the
/// bytes of the answers: the oldest answer is forgotten first.
struct Replay {
    max_bytes: usize,
    state: Mutex<ReplayState>,
}

fn cost(key: &ReplayKey, body: &Bytes) -> usize {
    ENTRY_OVERHEAD + key.0.len() + key.1.len() + key.2.len() + key.3.len() + body.len()
}

impl Replay {
    fn new(max_bytes: usize) -> Self {
        Replay {
            max_bytes,
            state: Mutex::new(ReplayState::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, ReplayState> {
        // The state is consistent between statements; a panic elsewhere must not stop replays.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Look the key up and, when it is unknown, reserve it for the caller.
    fn claim(this: &Arc<Replay>, key: &ReplayKey, digest: Digest32) -> Claim {
        let mut state = this.lock();
        match state.entries.get(key) {
            Some(Slot::Done {
                digest: known,
                status,
                body,
            }) => {
                if *known == digest {
                    Claim::Replay(*status, body.clone())
                } else {
                    Claim::Conflict
                }
            }
            Some(Slot::Pending {
                digest: known,
                done,
            }) => {
                if *known == digest {
                    Claim::Wait(done.subscribe())
                } else {
                    Claim::Conflict
                }
            }
            None => {
                let (done, _) = watch::channel(());
                state
                    .entries
                    .insert(key.clone(), Slot::Pending { digest, done });
                Claim::Reserved(Reservation {
                    replay: this.clone(),
                    key: key.clone(),
                    settled: false,
                })
            }
        }
    }

    /// End a reservation: record `answer` under the key, or only free the key when there is none
    /// (or it does not fit the bound). Waiters wake either way.
    fn settle(&self, key: &ReplayKey, answer: Option<(u16, &Bytes)>) {
        let mut state = self.lock();
        let Some(Slot::Pending { digest, done }) = state.entries.remove(key) else {
            return;
        };
        if let Some((status, body)) = answer {
            let charge = cost(key, body);
            if charge <= self.max_bytes {
                state.entries.insert(
                    key.clone(),
                    Slot::Done {
                        digest,
                        status,
                        body: body.clone(),
                    },
                );
                state.order.push_back(key.clone());
                state.bytes += charge;
                while state.bytes > self.max_bytes {
                    let Some(oldest) = state.order.pop_front() else {
                        break;
                    };
                    if let Some(Slot::Done { body, .. }) = state.entries.remove(&oldest) {
                        state.bytes = state.bytes.saturating_sub(cost(&oldest, &body));
                    }
                }
            }
        }
        drop(state);
        // Dropping the sender wakes every receiver.
        drop(done);
    }
}

/// A key reserved for the call that is running. Dropped unsettled (the task ended abnormally), it
/// frees the key.
struct Reservation {
    replay: Arc<Replay>,
    key: ReplayKey,
    settled: bool,
}

impl Reservation {
    fn record(mut self, status: u16, body: &Bytes) {
        self.settled = true;
        self.replay.settle(&self.key, Some((status, body)));
    }

    fn release(mut self) {
        self.settled = true;
        self.replay.settle(&self.key, None);
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.settled {
            self.replay.settle(&self.key, None);
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

/// Which listener a [`Routes`] is: the service network's, or the gateway's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Facing {
    /// The service network: public and internal contracts.
    Service,
    /// Browsers and the internet: public contracts only.
    Gateway,
}

impl Facing {
    /// Whether a contract of `exposure` is served on this listener. A local contract never is.
    pub fn serves(self, exposure: Exposure) -> bool {
        match (self, exposure) {
            (_, Exposure::Local) => false,
            (Facing::Service, Exposure::Public | Exposure::Internal) => true,
            (Facing::Gateway, Exposure::Public) => true,
            (Facing::Gateway, Exposure::Internal) => false,
        }
    }
}

/// Contract handlers collected into an axum router.
pub struct Routes<A: Authorizer> {
    router: Router,
    authorizer: Arc<A>,
    replay: Arc<Replay>,
    telemetry: Arc<dyn Telemetry>,
    facing: Facing,
}

impl<A: Authorizer> Routes<A> {
    /// No routes yet, on the service network: every public and internal contract served is
    /// checked by `authorizer`; a local contract is not served.
    pub fn new(authorizer: A) -> Self {
        Routes {
            router: Router::new(),
            authorizer: Arc::new(authorizer),
            replay: Arc::new(Replay::new(DEFAULT_REPLAY_BYTES)),
            telemetry: Arc::new(NoTelemetry),
            facing: Facing::Service,
        }
    }

    /// No routes yet, facing browsers and the internet: only public contracts are served, so an
    /// internal or local contract given to [`Routes::serve`] has no route here and a call to its
    /// path is answered 404 by the router.
    pub fn gateway(authorizer: A) -> Self {
        Routes {
            facing: Facing::Gateway,
            ..Routes::new(authorizer)
        }
    }

    /// The listener these routes are.
    pub fn facing(&self) -> Facing {
        self.facing
    }

    /// Keep at most `max_bytes` of recorded answers for replay (8 MiB by default), counting each
    /// answer's body, its key and a fixed overhead. Call it before serving any contract: it
    /// starts a new, empty cache.
    pub fn with_replay_bytes(mut self, max_bytes: usize) -> Self {
        self.replay = Arc::new(Replay::new(max_bytes));
        self
    }

    /// Report each call to `telemetry`.
    pub fn with_telemetry(mut self, telemetry: Arc<dyn Telemetry>) -> Self {
        self.telemetry = telemetry;
        self
    }

    /// Serve contract `C` with `handler`, when its exposure is served on this listener ([`Facing`]);
    /// otherwise the routes are returned unchanged and the contract has no route here.
    ///
    /// Per call: the authorizer checks `C::SPEC.access` against the request's headers, target and
    /// body (a refusal answers at once); for an
    /// idempotent contract with an `idempotency-key` header, an answer already given to the same
    /// tenant and subject under the same key and request is replayed without running the handler,
    /// the same key with another request is refused (`idempotency_conflict`), and a call of the
    /// same scope still running is waited for; the body is decoded (`invalid_request` if it does
    /// not decode); the handler runs on a task of its own and the call answers `timeout` if it has
    /// not finished within `C::SPEC.timeout`, without cancelling the handler; its answer is
    /// recorded for replay when the contract is idempotent and a key was given. See the module
    /// docs.
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
        if !self.facing.serves(C::SPEC.access.exposure) {
            return self;
        }
        let authorizer = self.authorizer.clone();
        let replay = self.replay.clone();
        let telemetry = self.telemetry.clone();
        let route = move |uri: Uri, headers: HeaderMap, body: Bytes| {
            let (authorizer, replay, telemetry, handler) = (
                authorizer.clone(),
                replay.clone(),
                telemetry.clone(),
                handler.clone(),
            );
            async move {
                let span = Span::start(&telemetry, "contract.call");
                let target = uri
                    .path_and_query()
                    .map_or_else(|| uri.path().to_string(), |p| p.as_str().to_string());
                let (status, body) =
                    answer::<C, A, F, Fut>(&authorizer, &replay, handler, &target, headers, body)
                        .await;
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
    replay: &Arc<Replay>,
    handler: F,
    target: &str,
    headers: HeaderMap,
    body: Bytes,
) -> (u16, Bytes)
where
    C: Contract,
    A: Authorizer,
    F: Fn(Caller, C::Request) -> Fut,
    Fut: Future<Output = Result<C::Response, Failure<C::Error>>> + Send + 'static,
{
    let presented = Presented {
        headers: &headers,
        target,
        body: &body,
    };
    let caller = match authorizer.authorize(&C::SPEC.access, &presented).await {
        Ok(caller) => caller,
        Err(refusal) => {
            let correlation = header(&headers, CORRELATION).unwrap_or("");
            return runtime_failure::<C>(refusal.code, refusal.message, correlation);
        }
    };
    let correlation = caller.correlation.clone();
    let replay_key: Option<ReplayKey> = match header(&headers, IDEMPOTENCY_KEY) {
        Some(key) if C::SPEC.idempotent => {
            let tenant = caller
                .attributes
                .get("tenant")
                .map(String::as_str)
                .or_else(|| header(&headers, TENANT))
                .unwrap_or("");
            Some((
                C::SPEC.id,
                tenant.to_string(),
                caller.subject.clone().unwrap_or_default(),
                key.to_string(),
            ))
        }
        _ => None,
    };
    let mut reservation = None;
    if let Some(key) = &replay_key {
        let digest = digest_of(&body);
        let deadline = tokio::time::Instant::now() + C::SPEC.timeout;
        loop {
            match Replay::claim(replay, key, digest) {
                Claim::Replay(status, body) => return (status, body),
                Claim::Conflict => {
                    return runtime_failure::<C>(
                        FailureCode::IdempotencyConflict,
                        "the idempotency key was used with another request",
                        &correlation,
                    )
                }
                Claim::Reserved(held) => {
                    reservation = Some(held);
                    break;
                }
                Claim::Wait(mut finished) => {
                    // The first call ends by dropping its slot, which makes `changed` fail.
                    if tokio::time::timeout_at(deadline, finished.changed())
                        .await
                        .is_err()
                    {
                        return runtime_failure::<C>(
                            FailureCode::Timeout,
                            format!("{} did not finish in time", C::SPEC.id),
                            &correlation,
                        );
                    }
                }
            }
        }
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
    // The handler runs on a task of its own, which settles the reservation when it is done. The
    // timeout below only stops this call from waiting: it cancels nothing.
    let pending = handler(caller, request);
    let task_correlation = correlation.clone();
    let mut task = tokio::spawn(async move {
        let (answer, record) = match pending.await {
            Ok(response) => match serde_json::to_vec(&response) {
                Ok(body) => ((200, Bytes::from(body)), true),
                Err(error) => (
                    runtime_failure::<C>(
                        FailureCode::Internal,
                        error.to_string(),
                        &task_correlation,
                    ),
                    false,
                ),
            },
            Err(Failure::Runtime { code, message }) => (
                runtime_failure::<C>(code, message, &task_correlation),
                false,
            ),
            Err(Failure::Contract { error }) => (
                encoded::<C>(Failure::Contract { error: &error }, &task_correlation),
                true,
            ),
        };
        if let Some(held) = reservation {
            if record {
                held.record(answer.0, &answer.1);
            } else {
                held.release();
            }
        }
        answer
    });
    match tokio::time::timeout(C::SPEC.timeout, &mut task).await {
        Err(_) => runtime_failure::<C>(
            FailureCode::Timeout,
            format!("{} did not finish in time", C::SPEC.id),
            &correlation,
        ),
        Ok(Ok(answer)) => answer,
        // The handler panicked; the reservation freed the key as the task unwound.
        Ok(Err(_)) => runtime_failure::<C>(
            FailureCode::Internal,
            format!("{} failed", C::SPEC.id),
            &correlation,
        ),
    }
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
            access: Access::SERVICE,
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

    fn presented<'a>(headers: &'a HeaderMap) -> Presented<'a> {
        Presented {
            headers,
            target: "/probe",
            body: b"{}",
        }
    }

    #[test]
    fn the_service_token_admits_the_service_scheme_only_with_the_token() {
        let token = ServiceToken {
            header: "x-token",
            secret: "s".into(),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let empty = HeaderMap::new();
        let mut wrong = HeaderMap::new();
        wrong.insert("x-token", HeaderValue::from_static("t"));
        let mut right = HeaderMap::new();
        right.insert("x-token", HeaderValue::from_static("s"));
        let service = Access::SERVICE;
        let refused = runtime
            .block_on(token.authorize(&service, &presented(&empty)))
            .unwrap_err();
        assert_eq!(refused.code, FailureCode::Unauthenticated);
        assert!(runtime
            .block_on(token.authorize(&service, &presented(&wrong)))
            .is_err());
        let caller = runtime
            .block_on(token.authorize(&service, &presented(&right)))
            .unwrap();
        assert_eq!(caller.subject.as_deref(), Some("service"));
        assert!(runtime
            .block_on(token.authorize(&Access::INTERNAL_ANONYMOUS, &presented(&empty)))
            .is_ok());
    }

    /// Default deny: every scheme the service token cannot check in full is refused, even with
    /// the token; so is anything local, and an anonymous call that asks for browser checks.
    #[test]
    fn the_service_token_refuses_every_scheme_it_cannot_check() {
        let token = ServiceToken {
            header: "x-token",
            secret: "s".into(),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let mut right = HeaderMap::new();
        right.insert("x-token", HeaderValue::from_static("s"));
        for scheme in [
            Scheme::Session,
            Scheme::ProvisioningToken,
            Scheme::Signature,
            Scheme::SignedAddress,
            Scheme::Delegation,
            Scheme::InProcess,
        ] {
            let access = Access {
                scheme,
                ..Access::SERVICE
            };
            assert!(
                runtime
                    .block_on(token.authorize(&access, &presented(&right)))
                    .is_err(),
                "{scheme:?}"
            );
        }
        let local = Access {
            scheme: Scheme::Service,
            ..Access::LOCAL
        };
        assert!(runtime
            .block_on(token.authorize(&local, &presented(&right)))
            .is_err());
        let checked = Access {
            csrf: true,
            ..Access::INTERNAL_ANONYMOUS
        };
        assert!(runtime
            .block_on(token.authorize(&checked, &presented(&right)))
            .is_err());
        let with_roles = Access {
            roles: &["operator"],
            ..Access::SERVICE
        };
        assert!(runtime
            .block_on(token.authorize(&with_roles, &presented(&right)))
            .is_err());
    }

    #[test]
    fn the_gateway_serves_public_contracts_only_and_nothing_serves_a_local_one() {
        assert!(Facing::Gateway.serves(Exposure::Public));
        assert!(!Facing::Gateway.serves(Exposure::Internal));
        assert!(!Facing::Gateway.serves(Exposure::Local));
        assert!(Facing::Service.serves(Exposure::Public));
        assert!(Facing::Service.serves(Exposure::Internal));
        assert!(!Facing::Service.serves(Exposure::Local));
    }
}
