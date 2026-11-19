//! Shared state of the backend process, its configuration, and the error type of its API.

use crate::hub::Hub;
use crate::lab::Lab;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::AtomicI64;
use std::sync::{Arc, Mutex};
use strata_kernel::{DomainError, ErrorKind, Timestamp};
use strata_protocol::api::{ErrorCode, ErrorReply};
use strata_protocol::log::{now_ms, Logger};
use strata_storage_api::{ExecuteError, Store, StoreError};

pub const LOG: Logger = Logger { service: "backend" };

#[derive(Clone, Debug)]
pub struct Config {
    pub addr: SocketAddr,
    /// The shared secret every internal request must carry.
    pub service_token: String,
    /// Enables development logins, fixture reset and session expiry on request.
    pub dev: bool,
    pub tenants_dir: PathBuf,
    /// Where the Lean project lives, for verification runs.
    pub lean_dir: PathBuf,
    /// Absolute lifetime of a session.
    pub session_ttl_ms: i64,
    /// A session unused for this long ends.
    pub session_idle_ms: i64,
}

/// A bounded list of recent things, newest last.
pub struct Recent<T> {
    items: Mutex<VecDeque<T>>,
    capacity: usize,
}

impl<T: Clone> Recent<T> {
    pub fn new(capacity: usize) -> Self {
        Recent {
            items: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
        }
    }

    pub fn push(&self, item: T) {
        if let Ok(mut items) = self.items.lock() {
            if items.len() == self.capacity {
                items.pop_front();
            }
            items.push_back(item);
        }
    }

    pub fn list(&self) -> Vec<T> {
        self.items
            .lock()
            .map(|items| items.iter().cloned().collect())
            .unwrap_or_default()
    }
}

/// Counters per tenant: `intent name -> (accepted, rejected)`, `error kind -> count` and commit
/// races retried.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct TenantCounters {
    pub transitions: BTreeMap<String, (u64, u64)>,
    pub rejections: BTreeMap<String, u64>,
    pub conflicts_retried: u64,
}

/// Counters and recent history for the diagnostics endpoints. Everything is kept per tenant, or
/// carries its tenant, so that a tenant's diagnostics can be given its own and nothing else.
pub struct Stats {
    pub in_flight: AtomicI64,
    pub counters: Mutex<BTreeMap<String, TenantCounters>>,
    pub recent_events: Recent<serde_json::Value>,
    pub recent_rejections: Recent<serde_json::Value>,
}

impl Stats {
    fn new() -> Self {
        Stats {
            in_flight: AtomicI64::new(0),
            counters: Mutex::new(BTreeMap::new()),
            recent_events: Recent::new(50),
            recent_rejections: Recent::new(50),
        }
    }

    pub fn record(
        &self,
        tenant: &str,
        intent: &str,
        rejected: Option<ErrorKind>,
        conflicts: usize,
    ) {
        let Ok(mut counters) = self.counters.lock() else {
            return;
        };
        let counters = counters.entry(tenant.to_string()).or_default();
        let entry = counters.transitions.entry(intent.to_string()).or_default();
        if rejected.is_some() {
            entry.1 += 1;
        } else {
            entry.0 += 1;
        }
        if let Some(kind) = rejected {
            *counters.rejections.entry(format!("{kind:?}")).or_default() += 1;
        }
        counters.conflicts_retried += conflicts as u64;
    }

    /// One tenant's counters.
    pub fn of(&self, tenant: &str) -> TenantCounters {
        self.counters
            .lock()
            .ok()
            .and_then(|counters| counters.get(tenant).cloned())
            .unwrap_or_default()
    }
}

pub struct App {
    pub cfg: Config,
    pub store: Arc<dyn Store>,
    pub hub: Hub,
    pub stats: Stats,
    pub lab: Lab,
    pub started_at: i64,
    /// Failed logins per `tenant/login`: `(count, locked until)`.
    pub throttle: Mutex<BTreeMap<String, (u32, i64)>>,
    /// Recent security-relevant events, newest last. Reported by the platform diagnostics only.
    pub audit: Recent<serde_json::Value>,
}

pub type Shared = Arc<App>;

impl App {
    pub fn new(cfg: Config, store: Arc<dyn Store>) -> Shared {
        Arc::new(App {
            cfg,
            store,
            hub: Hub::new(),
            stats: Stats::new(),
            lab: Lab::new(),
            started_at: now_ms(),
            throttle: Mutex::new(BTreeMap::new()),
            audit: Recent::new(200),
        })
    }

    /// Record a security-relevant event: an audit entry the platform diagnostics report, and a
    /// warning in the log that an alert can be raised on (`event` starts with `audit.`).
    pub fn audit(&self, event: &str, correlation: &str, fields: serde_json::Value) {
        let mut entry = json!({ "at": now_ms(), "event": event, "correlation": correlation });
        if let (Some(entry), serde_json::Value::Object(fields)) = (entry.as_object_mut(), &fields) {
            entry.extend(fields.clone());
        }
        LOG.warn(&format!("audit.{event}"), correlation, fields);
        self.audit.push(entry);
    }

    /// The one place the backend reads the clock for domain purposes.
    pub fn now(&self) -> Timestamp {
        Timestamp(now_ms())
    }

    /// Run store work off the async threads.
    pub async fn blocking<T, F>(self: &Arc<Self>, work: F) -> Result<T, ApiError>
    where
        T: Send + 'static,
        F: FnOnce(&App) -> T + Send + 'static,
    {
        let app = self.clone();
        strata_runtime::server::blocking(move || work(&app))
            .await
            .map_err(|e| ApiError::hidden(ErrorCode::Internal, format!("worker failed: {e}")))
    }
}

/// A failure reply. The correlation id is filled in where the reply is sent.
///
/// The second member is detail that is logged but never sent: what a store or a worker said
/// about a failure is for the operator, and the browser gets a generic message and the
/// correlation id that finds the log line.
#[derive(Clone, Debug)]
pub struct ApiError(pub ErrorReply, pub Option<String>);

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        ApiError(
            ErrorReply {
                code,
                message: message.into(),
                kind: None,
                failed: Vec::new(),
                correlation: String::new(),
            },
            None,
        )
    }

    /// A failure whose detail is logged and not sent: the reply carries a generic message for
    /// its code.
    pub fn hidden(code: ErrorCode, detail: impl Into<String>) -> Self {
        let message = match code {
            ErrorCode::Conflict => "Someone else changed this at the same moment. Try again.",
            ErrorCode::Unavailable => "The service is not available right now.",
            ErrorCode::NotFound => "That could not be found.",
            ErrorCode::Invalid => "The request could not be stored.",
            _ => "Something went wrong on our side.",
        };
        let mut error = ApiError::new(code, message);
        error.1 = Some(detail.into());
        error
    }

    /// Everything known about the failure, for the process's own messages and logs.
    pub fn describe(&self) -> String {
        match &self.1 {
            Some(detail) => format!("{}: {detail}", self.0.message),
            None => self.0.message.clone(),
        }
    }

    pub fn with_correlation(mut self, correlation: &str) -> Self {
        self.0.correlation = correlation.to_string();
        self
    }

    /// Translate a kernel rejection. `anonymous` decides between "sign in" and "not allowed".
    pub fn domain(error: DomainError, anonymous: bool) -> Self {
        let code = match error.kind {
            ErrorKind::Unauthorized if anonymous => ErrorCode::Unauthenticated,
            ErrorKind::Unauthorized => ErrorCode::Forbidden,
            ErrorKind::TenantNotFound
            | ErrorKind::UnknownObject
            | ErrorKind::UnknownRecord
            | ErrorKind::UnknownQuery => ErrorCode::NotFound,
            ErrorKind::VersionConflict | ErrorKind::SchemaConflict => ErrorCode::Conflict,
            ErrorKind::TenantMismatch | ErrorKind::InsufficientState => ErrorCode::Internal,
            _ => ErrorCode::Invalid,
        };
        ApiError(
            ErrorReply {
                code,
                message: error.message,
                kind: Some(format!("{:?}", error.kind)),
                failed: error
                    .evidence
                    .into_iter()
                    .filter(|check| !check.passed)
                    .collect(),
                correlation: String::new(),
            },
            None,
        )
    }

    pub fn execute(error: ExecuteError, anonymous: bool) -> Self {
        match error {
            ExecuteError::Domain(error) => Self::domain(error, anonymous),
            ExecuteError::Store(error) => error.into(),
        }
    }
}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> Self {
        let code = match error {
            StoreError::Conflict { .. } => ErrorCode::Conflict,
            StoreError::NotFound(_) => ErrorCode::NotFound,
            StoreError::Invalid(_) => ErrorCode::Invalid,
            StoreError::Unavailable(_) => ErrorCode::Unavailable,
            StoreError::Corrupt(_) => ErrorCode::Internal,
        };
        ApiError::hidden(code, error.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if let Some(detail) = &self.1 {
            LOG.error(
                "api.failed",
                &self.0.correlation,
                json!({ "code": self.0.code, "detail": detail }),
            );
        }
        let status =
            StatusCode::from_u16(self.0.code.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(json!(self.0))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a store says about a failure is logged, not sent: the reply carries the code, a
    /// generic message and, once it is set, the correlation id.
    #[test]
    fn a_store_failure_reaches_the_client_without_its_detail() {
        for (error, code) in [
            (
                StoreError::Corrupt("table records row 7: bad json at /srv/strata.db".into()),
                ErrorCode::Internal,
            ),
            (
                StoreError::Unavailable("database is locked: /srv/strata.db".into()),
                ErrorCode::Unavailable,
            ),
            (
                StoreError::Invalid("UNIQUE constraint failed: sessions.token_hash".into()),
                ErrorCode::Invalid,
            ),
            (
                StoreError::NotFound("tenant secret-tenant".into()),
                ErrorCode::NotFound,
            ),
        ] {
            let detail = error.to_string();
            let reply = ApiError::from(error).with_correlation("c-1");
            assert_eq!(reply.0.code, code);
            assert_eq!(reply.0.correlation, "c-1");
            let sent = serde_json::to_string(&reply.0).unwrap();
            for secret in [
                "/srv/strata.db",
                "sessions.token_hash",
                "secret-tenant",
                "row 7",
            ] {
                assert!(!sent.contains(secret), "{secret} was sent: {sent}");
            }
            assert!(
                reply.describe().contains(&detail),
                "the detail is kept for the log"
            );
        }
    }
}
