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
use std::sync::atomic::{AtomicI64, Ordering};
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

/// Counters and recent history for the diagnostics endpoint.
pub struct Stats {
    pub in_flight: AtomicI64,
    /// `intent name -> (accepted, rejected)`.
    pub transitions: Mutex<BTreeMap<String, (u64, u64)>>,
    /// `error kind -> count`.
    pub rejections: Mutex<BTreeMap<String, u64>>,
    pub conflicts_retried: AtomicI64,
    pub recent_events: Recent<serde_json::Value>,
    pub recent_rejections: Recent<serde_json::Value>,
}

impl Stats {
    fn new() -> Self {
        Stats {
            in_flight: AtomicI64::new(0),
            transitions: Mutex::new(BTreeMap::new()),
            rejections: Mutex::new(BTreeMap::new()),
            conflicts_retried: AtomicI64::new(0),
            recent_events: Recent::new(50),
            recent_rejections: Recent::new(50),
        }
    }

    pub fn record(&self, intent: &str, rejected: Option<ErrorKind>, conflicts: usize) {
        if let Ok(mut transitions) = self.transitions.lock() {
            let entry = transitions.entry(intent.to_string()).or_default();
            if rejected.is_some() {
                entry.1 += 1;
            } else {
                entry.0 += 1;
            }
        }
        if let (Some(kind), Ok(mut rejections)) = (rejected, self.rejections.lock()) {
            *rejections.entry(format!("{kind:?}")).or_default() += 1;
        }
        self.conflicts_retried
            .fetch_add(conflicts as i64, Ordering::Relaxed);
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
        })
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
        tokio::task::spawn_blocking(move || work(&app))
            .await
            .map_err(|e| ApiError::new(ErrorCode::Internal, format!("worker failed: {e}")))
    }
}

/// A failure reply. The correlation id is filled in where the reply is sent.
#[derive(Clone, Debug)]
pub struct ApiError(pub ErrorReply);

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        ApiError(ErrorReply {
            code,
            message: message.into(),
            kind: None,
            failed: Vec::new(),
            correlation: String::new(),
        })
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
        ApiError(ErrorReply {
            code,
            message: error.message,
            kind: Some(format!("{:?}", error.kind)),
            failed: error
                .evidence
                .into_iter()
                .filter(|check| !check.passed)
                .collect(),
            correlation: String::new(),
        })
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
        ApiError::new(code, error.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.code.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(json!(self.0))).into_response()
    }
}
