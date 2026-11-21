//! The outbound gate (component `outbound-gate`, K-252, D-702): the one function every connection
//! to an address that a tenant names goes through.
//!
//! One call, in order:
//!
//! 1. The address is parsed and its form judged ([`super::rule::parse_target`]): HTTPS, port 443,
//!    no user name, password or fragment (WFL-1579).
//! 2. The host is resolved **here**, once, by the gate's resolver, and **every** address the
//!    answer holds is judged under the outbound rule; one refused address refuses the call, so a
//!    name that answers one public and one private address is refused (WFL-008, WFL-252).
//! 3. The transport connects to the address that was judged, sending the original host name for
//!    TLS and in the `Host` header, so no second resolution happens that could answer differently
//!    (WFL-250, D-254).
//! 4. A redirect is not followed by the transport. The gate follows at most
//!    [`Policy::max_redirects`] of them itself, running steps 1 to 3 again on each new address, and
//!    refuses the call at one the rule does not allow or at one too many (WFL-251, WFL-2641).
//! 5. The time to connect, the time of the whole call and the bytes of the response are bounded by
//!    the connector policy; a call that passes a bound is ended and reported as such (WFL-253).
//!
//! What a caller gets back on failure is a [`Failure`]: a kind and a reason, never a response body.
//! Each call writes one log line with the address as [`super::rule::redacted`] shows it (no query,
//! no userinfo, WFL-254), the address connected to, the outcome, the bytes read and the idempotency
//! key, and never a body or a header value (PRV-366).
//!
//! Implements: CT-206, H-026, H-027, WFL-008, WFL-250, WFL-251, WFL-252, WFL-253, WFL-254,
//! WFL-1579, WFL-2641, WFL-2642, K-252

use super::rule::{self, Host, Target};
use crate::app::LOG;
use serde_json::json;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The connector policy (WFL-253, WFL-2642).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    /// How long resolving and connecting may take.
    pub connect_timeout: Duration,
    /// How long the whole call may take, redirects and the response body included.
    pub total_timeout: Duration,
    /// The most response bytes read when the request names no smaller number.
    pub max_bytes: u64,
    /// The most redirects one call follows.
    pub max_redirects: u32,
    /// The development exception (FED-2644): plain HTTP, any port and any address are allowed.
    /// The address is still resolved once and connected to as resolved.
    pub development: bool,
}

/// Ten seconds to connect (WFL-2642).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Thirty seconds in all (outbound-call-flow, WFL-2642).
pub const TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
/// One mebibyte of response (outbound-call-flow, WFL-2642).
pub const MAX_BYTES: u64 = 1024 * 1024;
/// Three redirects (WFL-2641).
pub const MAX_REDIRECTS: u32 = 3;

impl Default for Policy {
    fn default() -> Self {
        Policy {
            connect_timeout: CONNECT_TIMEOUT,
            total_timeout: TOTAL_TIMEOUT,
            max_bytes: MAX_BYTES,
            max_redirects: MAX_REDIRECTS,
            development: false,
        }
    }
}

/// The method of an outbound call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    pub fn as_str(&self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
        }
    }
}

/// One outbound call as a caller asks for it.
#[derive(Clone, Debug)]
pub struct Request {
    pub method: Method,
    pub url: String,
    /// Header names and values. `host`, `content-length`, `transfer-encoding`, `connection` and
    /// the other hop-by-hop headers are dropped: the transport writes them.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// The most response bytes to read; never more than the policy's.
    pub max_bytes: Option<u64>,
    /// What the call is for, for the log line: `webhook`, `oidc` and the like, with the tenant.
    pub purpose: String,
    /// The idempotency key the call carries, for the log line (PRV-366).
    pub idempotency_key: Option<String>,
}

impl Request {
    /// A request with no headers or body, the policy's size bound and no idempotency key.
    pub fn new(method: Method, url: impl Into<String>, purpose: impl Into<String>) -> Self {
        Request {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: Vec::new(),
            max_bytes: None,
            purpose: purpose.into(),
            idempotency_key: None,
        }
    }
}

/// The answer to a call that was allowed and completed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    /// Lower-case header names and their values.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// The address the last request connected to.
    pub address: IpAddr,
    /// How many redirects were followed.
    pub redirects: u32,
}

impl Response {
    /// The first value of a header, by its lower-case name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Why a call did not complete. The reason is for the operator's log and for errors the platform
/// itself reads; a tenant is shown only [`Failure::code`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The first address is outside the outbound rule (WFL-008, WFL-252, WFL-1579).
    AddressRefused(String),
    /// A redirect named an address outside the rule, or there were too many (WFL-251, WFL-2641).
    RedirectRefused(String),
    /// The response passed the size bound (WFL-253).
    TooLarge { received: u64, limit: u64 },
    /// The call passed a time bound (WFL-253).
    TimedOut,
    /// The address did not resolve or did not answer.
    Unreachable(String),
}

impl Failure {
    /// The name a tenant sees: one of `address-refused`, `redirect-refused`, `too-large`,
    /// `timed-out` and `unreachable` (CT-206).
    pub fn code(&self) -> &'static str {
        match self {
            Failure::AddressRefused(_) => "address-refused",
            Failure::RedirectRefused(_) => "redirect-refused",
            Failure::TooLarge { .. } => "too-large",
            Failure::TimedOut => "timed-out",
            Failure::Unreachable(_) => "unreachable",
        }
    }

    /// Whether the same call made again later may succeed: a time bound passed or the address
    /// did not answer. A refusal or a response that is too large is the same next time.
    pub fn transient(&self) -> bool {
        matches!(self, Failure::TimedOut | Failure::Unreachable(_))
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::AddressRefused(why) => write!(f, "address-refused: {why}"),
            Failure::RedirectRefused(why) => write!(f, "redirect-refused: {why}"),
            Failure::TooLarge { received, limit } => {
                write!(f, "too-large: {received} bytes passed the limit of {limit}")
            }
            Failure::TimedOut => f.write_str("timed-out"),
            Failure::Unreachable(why) => write!(f, "unreachable: {why}"),
        }
    }
}

/// Resolves a host name to its addresses. The gate's own resolver is the system's; a test puts
/// another in its place to control the answer.
pub trait Resolve: Send + Sync {
    fn resolve(&self, host: &str, timeout: Duration) -> Result<Vec<IpAddr>, String>;
}

/// What a transport is asked to do: one HTTP exchange with one judged address.
pub struct Exchange<'a> {
    pub method: Method,
    pub url: &'a reqwest::Url,
    /// The host name to send for TLS and in `Host`, when the URL's host is a name; `None` for a
    /// numeric host.
    pub name: Option<&'a str>,
    /// The judged address, with the URL's port. The transport connects here and nowhere else.
    pub address: SocketAddr,
    pub headers: &'a [(String, String)],
    pub body: &'a [u8],
    pub connect_timeout: Duration,
    /// When the whole call must be over.
    pub deadline: Instant,
    /// Whether plain HTTP is allowed (the development exception only).
    pub allow_http: bool,
}

/// A failure of the transport.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportError {
    TimedOut,
    Failed(String),
}

/// The head of an answer and a way to read its body a piece at a time, so that the gate counts
/// every byte against its bound and stops reading the moment one is passed.
pub trait Answer: Send {
    fn status(&self) -> u16;
    /// Lower-case header names and their values.
    fn headers(&self) -> &[(String, String)];
    /// The length the answer announces, when it announces one.
    fn content_length(&self) -> Option<u64>;
    /// The next piece of the body, `None` at its end. Gives up at `deadline`.
    fn next_chunk(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, TransportError>;
}

/// Makes one HTTP exchange with an address the gate judged. Follows no redirect.
pub trait Transport: Send + Sync {
    fn open(&self, exchange: &Exchange<'_>) -> Result<Box<dyn Answer>, TransportError>;
}

/// Headers a caller may not set: the transport writes them, or they steer the connection.
const DROPPED_HEADERS: [&str; 9] = [
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "upgrade",
    "proxy-authorization",
    "proxy-connection",
    "te",
];

/// Headers dropped when a redirect leaves the host the call was made to.
const CREDENTIAL_HEADERS: [&str; 3] = ["authorization", "cookie", "proxy-authorization"];

/// The outbound gate: a resolver, a transport and the connector policy.
pub struct Gate {
    resolver: Arc<dyn Resolve>,
    transport: Arc<dyn Transport>,
    policy: Policy,
}

impl std::fmt::Debug for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gate")
            .field("policy", &self.policy)
            .finish()
    }
}

impl Gate {
    pub fn new(resolver: Arc<dyn Resolve>, transport: Arc<dyn Transport>, policy: Policy) -> Self {
        Gate {
            resolver,
            transport,
            policy,
        }
    }

    /// The gate of this process: the system resolver, HTTP over TLS, the default connector policy
    /// and, in development only, the development exception.
    pub fn system(development: bool) -> Self {
        Gate::new(
            Arc::new(super::transport::SystemResolver),
            Arc::new(super::transport::HttpTransport),
            Policy {
                development,
                ..Policy::default()
            },
        )
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// Judge the form of an address without resolving it (for an address a tenant saves, such
    /// as a subscription's target).
    pub fn check_form(&self, url: &str) -> Result<Target, Failure> {
        rule::parse_target(url, self.policy.development)
            .map_err(|refusal| Failure::AddressRefused(refusal.0))
    }

    /// Resolve the host of a target once and judge every address of the answer (WFL-008,
    /// WFL-252). The address returned is the one to connect to.
    pub fn judge(&self, target: &Target) -> Result<IpAddr, Failure> {
        match &target.host {
            Host::Ip(address) => {
                if !self.policy.development {
                    if let Some(why) = rule::refusal(*address) {
                        return Err(Failure::AddressRefused(format!(
                            "a {why} address is not allowed"
                        )));
                    }
                }
                Ok(*address)
            }
            Host::Name(name) => {
                let answer = self
                    .resolver
                    .resolve(name, self.policy.connect_timeout)
                    .map_err(Failure::Unreachable)?;
                let Some(first) = answer.first().copied() else {
                    return Err(Failure::Unreachable(format!("{name} has no address")));
                };
                if !self.policy.development {
                    // Every address of the answer: a client could be steered to any of them.
                    for address in &answer {
                        if let Some(why) = rule::refusal(*address) {
                            return Err(Failure::AddressRefused(format!(
                                "{name} resolves to a {why} address"
                            )));
                        }
                    }
                }
                Ok(first)
            }
        }
    }

    /// Make one outbound call through the gate. See the module for the order of the checks.
    pub fn call(&self, request: &Request) -> Result<Response, Failure> {
        let started = Instant::now();
        let deadline = started + self.policy.total_timeout;
        let limit = request.max_bytes.map_or(self.policy.max_bytes, |asked| {
            asked.min(self.policy.max_bytes)
        });
        let mut url = request.url.clone();
        let mut method = request.method;
        let mut body = request.body.clone();
        let mut headers: Vec<(String, String)> = request
            .headers
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
            .filter(|(name, _)| !DROPPED_HEADERS.contains(&name.as_str()))
            .collect();
        let mut redirects = 0u32;
        let mut shown = rule::redacted_text(&url);
        let outcome = loop {
            let refused = |why: String| {
                if redirects == 0 {
                    Failure::AddressRefused(why)
                } else {
                    Failure::RedirectRefused(why)
                }
            };
            let target = match rule::parse_target(&url, self.policy.development) {
                Ok(target) => target,
                Err(refusal) => break Err(refused(refusal.0)),
            };
            shown = rule::redacted(&target.url);
            let address = match self.judge(&target) {
                Ok(address) => address,
                Err(Failure::AddressRefused(why)) => break Err(refused(why)),
                Err(other) => break Err(other),
            };
            if Instant::now() >= deadline {
                break Err(Failure::TimedOut);
            }
            let port = target.url.port_or_known_default().unwrap_or(443);
            let name = match &target.host {
                Host::Name(name) => Some(name.as_str()),
                Host::Ip(_) => None,
            };
            let exchange = Exchange {
                method,
                url: &target.url,
                name,
                address: SocketAddr::new(address, port),
                headers: &headers,
                body: &body,
                connect_timeout: self.policy.connect_timeout,
                deadline,
                allow_http: self.policy.development,
            };
            let answer = match self.transport.open(&exchange) {
                Ok(answer) => answer,
                Err(TransportError::TimedOut) => break Err(Failure::TimedOut),
                Err(TransportError::Failed(why)) => break Err(Failure::Unreachable(why)),
            };
            let status = answer.status();
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                let location = answer
                    .headers()
                    .iter()
                    .find(|(name, _)| name == "location")
                    .map(|(_, value)| value.clone());
                let Some(location) = location else {
                    // A redirect with nowhere to go is an answer like any other.
                    break read_body(answer, status, address, redirects, deadline, limit);
                };
                redirects += 1;
                if redirects > self.policy.max_redirects {
                    break Err(Failure::RedirectRefused(format!(
                        "more than {} redirects",
                        self.policy.max_redirects
                    )));
                }
                let Ok(next) = target.url.join(&location) else {
                    break Err(Failure::RedirectRefused(
                        "the redirect's address cannot be read".into(),
                    ));
                };
                // 303, and 301 or 302 after a POST, are followed with a GET and no body, as
                // browsers do; 307 and 308 keep the method and the body.
                if status == 303 || (matches!(status, 301 | 302) && method == Method::Post) {
                    method = Method::Get;
                    body.clear();
                    headers.retain(|(name, _)| name != "content-type");
                }
                if next.host_str() != target.url.host_str() {
                    headers.retain(|(name, _)| !CREDENTIAL_HEADERS.contains(&name.as_str()));
                }
                url = next.to_string();
                continue;
            }
            break read_body(answer, status, address, redirects, deadline, limit);
        };
        let elapsed_ms = started.elapsed().as_millis() as u64;
        match &outcome {
            Ok(response) => LOG.info(
                "outbound.call",
                "",
                json!({
                    "purpose": request.purpose,
                    "method": request.method.as_str(),
                    "target": shown,
                    "address": response.address.to_string(),
                    "outcome": "answered",
                    "status": response.status,
                    "received": response.body.len(),
                    "limit": limit,
                    "redirects": response.redirects,
                    "idempotency_key": request.idempotency_key,
                    "ms": elapsed_ms,
                }),
            ),
            Err(failure) => LOG.warn(
                "outbound.call",
                "",
                json!({
                    "purpose": request.purpose,
                    "method": request.method.as_str(),
                    "target": shown,
                    "outcome": failure.code(),
                    "reason": failure.to_string(),
                    "limit": limit,
                    "redirects": redirects,
                    "idempotency_key": request.idempotency_key,
                    "ms": elapsed_ms,
                }),
            ),
        }
        outcome
    }
}

/// Read the body of an answer, ending the call the moment it passes `limit` bytes or `deadline`
/// (WFL-253).
fn read_body(
    mut answer: Box<dyn Answer>,
    status: u16,
    address: IpAddr,
    redirects: u32,
    deadline: Instant,
    limit: u64,
) -> Result<Response, Failure> {
    if let Some(announced) = answer.content_length() {
        if announced > limit {
            return Err(Failure::TooLarge {
                received: announced,
                limit,
            });
        }
    }
    let mut body: Vec<u8> = Vec::new();
    loop {
        if Instant::now() >= deadline {
            return Err(Failure::TimedOut);
        }
        match answer.next_chunk(deadline) {
            Ok(Some(chunk)) => {
                let received = body.len() as u64 + chunk.len() as u64;
                if received > limit {
                    return Err(Failure::TooLarge { received, limit });
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(TransportError::TimedOut) => return Err(Failure::TimedOut),
            Err(TransportError::Failed(why)) => return Err(Failure::Unreachable(why)),
        }
    }
    Ok(Response {
        status,
        headers: answer.headers().to_vec(),
        body,
        address,
        redirects,
    })
}
