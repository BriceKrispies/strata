//! Calling another component's contract.
//!
//! [`Client::call`] sends a [`Contract`]'s request in the wire format [`Routes`](crate::Routes)
//! serves and decodes the answer into the contract's response or error. Credentials are added by
//! a [`Credentials`] value, written by hand per deployment.
//!
//! Retries: a call to an idempotent contract is sent up to [`Client::with_attempts`] times
//! (default 3), with the same `idempotency-key` header each time, when it fails in transport or
//! with a runtime `timeout` or `unavailable`. A call to a contract that is not idempotent is sent
//! once.

use crate::routes::{Contract, Failure, FailureCode, ServiceToken, CORRELATION, IDEMPOTENCY_KEY};
use std::fmt;
use std::time::Duration;

/// Adds whatever proves who the caller is to an outgoing request.
pub trait Credentials: Send + Sync + 'static {
    /// Return `request` with the credentials added.
    fn apply(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder;
}

/// No credentials, for contracts whose requirement is `none`.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoCredentials;

impl Credentials for NoCredentials {
    fn apply(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
    }
}

impl Credentials for ServiceToken {
    fn apply(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request.header(self.header, &self.secret)
    }
}

/// Why a call did not return the contract's response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallError<E> {
    /// The handler returned one of the contract's errors.
    Contract(E),
    /// The serving runtime refused or failed the call.
    Runtime {
        /// What kind of failure.
        code: FailureCode,
        /// The server's message.
        message: String,
    },
    /// The request did not reach the server or the answer did not come back.
    Transport(String),
    /// The answer came back but could not be decoded.
    Codec(String),
}

impl<E: fmt::Debug> fmt::Display for CallError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::Contract(error) => write!(f, "contract error: {error:?}"),
            CallError::Runtime { code, message } => write!(f, "{code:?}: {message}"),
            CallError::Transport(why) => write!(f, "transport: {why}"),
            CallError::Codec(why) => write!(f, "undecodable answer: {why}"),
        }
    }
}

impl<E: fmt::Debug> std::error::Error for CallError<E> {}

impl<E> CallError<E> {
    fn retryable(&self) -> bool {
        matches!(
            self,
            CallError::Transport(_)
                | CallError::Runtime {
                    code: FailureCode::Timeout | FailureCode::Unavailable,
                    ..
                }
        )
    }
}

/// A typed client for the contracts one component serves.
#[derive(Clone, Debug)]
pub struct Client<K: Credentials> {
    http: reqwest::Client,
    base: String,
    credentials: K,
    attempts: u32,
}

fn idempotency_key() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::getrandom(&mut bytes).is_err() {
        // Without randomness, fall back to the time; keys only need to differ between calls.
        let now = crate::clock::Clock::now(&crate::SystemClock).0;
        bytes[..8].copy_from_slice(&now.to_le_bytes());
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl<K: Credentials> Client<K> {
    /// A client for the component at `base` (such as `http://127.0.0.1:4101`).
    pub fn new(base: &str, credentials: K) -> Result<Self, String> {
        Ok(Client {
            http: reqwest::Client::builder()
                .build()
                .map_err(|e| e.to_string())?,
            base: base.trim_end_matches('/').to_string(),
            credentials,
            attempts: 3,
        })
    }

    /// Send an idempotent call at most `attempts` times (at least once).
    pub fn with_attempts(mut self, attempts: u32) -> Self {
        self.attempts = attempts.max(1);
        self
    }

    /// Call contract `C`.
    pub async fn call<C: Contract>(
        &self,
        request: &C::Request,
    ) -> Result<C::Response, CallError<C::Error>> {
        self.call_with::<C>(request, "").await
    }

    /// Call contract `C`, passing `correlation` on as the correlation id when it is not empty.
    pub async fn call_with<C: Contract>(
        &self,
        request: &C::Request,
        correlation: &str,
    ) -> Result<C::Response, CallError<C::Error>> {
        let attempts = if C::SPEC.idempotent { self.attempts } else { 1 };
        let key = idempotency_key();
        let mut attempt = 1;
        loop {
            let result = self.once::<C>(request, correlation, &key).await;
            match result {
                Err(error) if attempt < attempts && error.retryable() => {
                    tokio::time::sleep(Duration::from_millis(100 * u64::from(attempt))).await;
                    attempt += 1;
                }
                other => return other,
            }
        }
    }

    async fn once<C: Contract>(
        &self,
        request: &C::Request,
        correlation: &str,
        key: &str,
    ) -> Result<C::Response, CallError<C::Error>> {
        let mut builder = self
            .http
            .post(format!("{}{}", self.base, C::SPEC.path))
            // The server answers `timeout` itself at the contract's timeout; this only covers a
            // server that does not answer at all.
            .timeout(C::SPEC.timeout + Duration::from_secs(1))
            .json(request);
        if C::SPEC.idempotent {
            builder = builder.header(IDEMPOTENCY_KEY, key);
        }
        if !correlation.is_empty() {
            builder = builder.header(CORRELATION, correlation);
        }
        let response = self
            .credentials
            .apply(builder)
            .send()
            .await
            .map_err(|e| CallError::Transport(e.to_string()))?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|e| CallError::Transport(e.to_string()))?;
        if status.is_success() {
            return serde_json::from_slice(&body).map_err(|e| CallError::Codec(e.to_string()));
        }
        match serde_json::from_slice::<Failure<C::Error>>(&body) {
            Ok(Failure::Contract { error }) => Err(CallError::Contract(error)),
            Ok(Failure::Runtime { code, message }) => Err(CallError::Runtime { code, message }),
            Err(_) => Err(CallError::Runtime {
                code: if status.as_u16() == 503 {
                    FailureCode::Unavailable
                } else {
                    FailureCode::Internal
                },
                message: format!("status {status} without a failure body"),
            }),
        }
    }
}
