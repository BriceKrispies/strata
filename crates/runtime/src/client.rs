//! Calling another component's contract.
//!
//! [`Client::call`] sends a [`Contract`]'s request in the wire format [`Routes`](crate::Routes)
//! serves and decodes the answer into the contract's response or error, reading a failure with
//! the contract's [`FailureBody`]. Credentials are added by
//! a [`Credentials`] value, written by hand per deployment.
//!
//! Retries: a call to an idempotent contract is sent up to [`CallRetry::attempts`] times
//! (default 3, set with [`Client::with_attempts`] or [`Client::with_retry`]), with the same
//! `idempotency-key` header each time, when it fails in transport or with a runtime `timeout` or
//! `unavailable`. The wait between attempts and the time one attempt may take come from the
//! [`CallRetry`] too; [`Client::call_retrying`] takes one for a single call, such as the policy the
//! specification states for that contract. A call to a contract that is not idempotent is sent
//! once.

use crate::routes::{
    Contract, Failure, FailureBody, FailureCode, ServiceToken, CORRELATION, IDEMPOTENCY_KEY,
};
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

/// How the wait between attempts of a call grows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallBackoff {
    /// The wait after the n-th failure is `step * n`.
    Linear(Duration),
    /// Every wait is the same.
    Fixed(Duration),
    /// The wait after the n-th failure is `base * 2^(n-1)`, at most `cap` when there is one.
    Exponential {
        /// The wait after the first failure.
        base: Duration,
        /// The longest wait.
        cap: Option<Duration>,
    },
}

/// When and how often a call is repeated: the numbers of a retry policy of the specification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallRetry {
    /// The most attempts, the first included (at least one).
    pub attempts: u32,
    /// How the wait between attempts grows.
    pub backoff: CallBackoff,
    /// How long one attempt may take, instead of the contract's timeout.
    pub timeout: Option<Duration>,
}

impl Default for CallRetry {
    /// Three attempts, waiting 100 ms after the first failure and 200 ms after the second, each
    /// bounded by the contract's timeout.
    fn default() -> Self {
        CallRetry {
            attempts: 3,
            backoff: CallBackoff::Linear(Duration::from_millis(100)),
            timeout: None,
        }
    }
}

impl CallRetry {
    /// The wait after `failures` failed attempts (1 for the first failure).
    pub fn wait_after(&self, failures: u32) -> Duration {
        let failures = failures.max(1);
        match self.backoff {
            CallBackoff::Linear(step) => step.saturating_mul(failures),
            CallBackoff::Fixed(wait) => wait,
            CallBackoff::Exponential { base, cap } => {
                let wait = base.saturating_mul(1u32 << (failures - 1).min(31));
                cap.map_or(wait, |cap| wait.min(cap))
            }
        }
    }
}

/// A typed client for the contracts one component serves.
#[derive(Clone, Debug)]
pub struct Client<K: Credentials> {
    http: reqwest::Client,
    base: String,
    credentials: K,
    retry: CallRetry,
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
            retry: CallRetry::default(),
        })
    }

    /// Send an idempotent call at most `attempts` times (at least once).
    pub fn with_attempts(mut self, attempts: u32) -> Self {
        self.retry.attempts = attempts.max(1);
        self
    }

    /// Repeat idempotent calls as `retry` says, unless a call brings its own policy.
    pub fn with_retry(mut self, retry: CallRetry) -> Self {
        self.retry = CallRetry {
            attempts: retry.attempts.max(1),
            ..retry
        };
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
        self.call_retrying::<C>(request, correlation, &self.retry)
            .await
    }

    /// Call contract `C` with `retry` in place of the client's own policy, such as the one the
    /// specification states for that contract.
    pub async fn call_retrying<C: Contract>(
        &self,
        request: &C::Request,
        correlation: &str,
        retry: &CallRetry,
    ) -> Result<C::Response, CallError<C::Error>> {
        let attempts = if C::SPEC.idempotent {
            retry.attempts.max(1)
        } else {
            1
        };
        let key = idempotency_key();
        let mut attempt = 1;
        loop {
            let result = self
                .once::<C>(request, correlation, &key, retry.timeout)
                .await;
            match result {
                Err(error) if attempt < attempts && error.retryable() => {
                    tokio::time::sleep(retry.wait_after(attempt)).await;
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
        timeout: Option<Duration>,
    ) -> Result<C::Response, CallError<C::Error>> {
        let mut builder = self
            .http
            .post(format!("{}{}", self.base, C::SPEC.path))
            // The server answers `timeout` itself at the contract's timeout; this only covers a
            // server that does not answer at all.
            .timeout(timeout.unwrap_or(C::SPEC.timeout) + Duration::from_secs(1))
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
        match <C::Body as FailureBody<C::Error>>::decode(status.as_u16(), &body) {
            Some(Failure::Contract { error }) => Err(CallError::Contract(error)),
            Some(Failure::Runtime { code, message }) => Err(CallError::Runtime { code, message }),
            None => Err(CallError::Runtime {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_waits_as_the_client_always_did() {
        let retry = CallRetry::default();
        assert_eq!(retry.attempts, 3);
        assert_eq!(retry.wait_after(1), Duration::from_millis(100));
        assert_eq!(retry.wait_after(2), Duration::from_millis(200));
    }

    #[test]
    fn exponential_waits_double_up_to_the_cap() {
        let retry = CallRetry {
            attempts: 6,
            backoff: CallBackoff::Exponential {
                base: Duration::from_millis(100),
                cap: Some(Duration::from_millis(350)),
            },
            timeout: None,
        };
        assert_eq!(retry.wait_after(1), Duration::from_millis(100));
        assert_eq!(retry.wait_after(2), Duration::from_millis(200));
        assert_eq!(retry.wait_after(3), Duration::from_millis(350));
        assert_eq!(retry.wait_after(40), Duration::from_millis(350));
    }

    #[test]
    fn fixed_waits_do_not_grow() {
        let retry = CallRetry {
            attempts: 2,
            backoff: CallBackoff::Fixed(Duration::from_secs(1)),
            timeout: None,
        };
        assert_eq!(retry.wait_after(1), retry.wait_after(5));
    }
}
