//! Shared state of the backend process, its configuration, and the error type of its API.

use crate::hub::Hub;
use crate::lab::Lab;
use crate::limits::{Admission, Limits};
use crate::mail::Mailer;
use crate::signin::oidc::Federation;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use strata_kernel::{DomainError, ErrorKind, TenantId, Timestamp};
use strata_protocol::api::{ErrorCode, ErrorReply};
use strata_protocol::log::{now_ms, Logger};
use strata_storage_api::{AuditEntry, ExecuteError, Store, StoreError};

pub const LOG: Logger = Logger { service: "backend" };

/// The default absolute lifetime of a session: how long after sign-in it ends (`main.rs` reads
/// `STRATA_SESSION_TTL_MS` over it). The specification states two different values (D-700 and
/// the machine `session-authority-flow-impl`); `tests/spec_agreement.rs` holds the one the
/// default is checked against.
pub const DEFAULT_SESSION_TTL_MS: i64 = 12 * 3_600_000;

/// The default idle lifetime of a session: how long after its last use it ends (`main.rs` reads
/// `STRATA_SESSION_IDLE_MS` over it).
pub const DEFAULT_SESSION_IDLE_MS: i64 = 2 * 3_600_000;

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
    /// How many failed attempts are let through, and for how long a limit holds.
    pub throttle: ThrottleLimits,
    /// How long an issued passkey challenge can be answered.
    pub passkey_challenge_ttl_ms: i64,
    /// How long an emailed sign-in link is accepted (D-9: fifteen minutes).
    pub link_ttl_ms: i64,
    /// Where mail goes. A development sink unless an SMTP server was configured.
    pub mail: Arc<dyn Mailer>,
    /// The identity providers of each tenant, and what has been fetched from them.
    pub federation: Arc<Federation>,
    /// Second factors, step-up and recovery: which realms ask for a second factor and the
    /// platform defaults of D-700 for them.
    pub factors: FactorPolicy,
    /// What one visitor, session or tenant may cost. See [`Limits`].
    pub limits: Limits,
}

/// The key that seals TOTP secrets at rest. It is derived from a platform secret and kept out of
/// the store, so that a copy of the database alone opens no second factor.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SealKey(pub [u8; 32]);

impl SealKey {
    /// A key derived from a platform secret, in a domain of its own.
    pub fn derive(secret: &str) -> Self {
        let hex =
            strata_kernel::canon::hash_bytes(format!("strata/seal-key/v1\0{secret}").as_bytes());
        let mut key = [0u8; 32];
        for (slot, pair) in key.iter_mut().zip(hex.as_bytes().chunks(2)) {
            let pair = std::str::from_utf8(pair).unwrap_or("00");
            *slot = u8::from_str_radix(pair, 16).unwrap_or(0);
        }
        SealKey(key)
    }

    /// A short identifier for the key, stored with every secret it seals so that a rotation
    /// knows which key opens which secret. It reveals nothing of the key.
    pub fn id(&self) -> String {
        let mut bytes = b"strata/seal-key-id/v1\0".to_vec();
        bytes.extend_from_slice(&self.0);
        strata_kernel::canon::hash_bytes(&bytes)[..8].to_string()
    }
}

impl std::fmt::Debug for SealKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SealKey(..)")
    }
}

/// What a realm asks of second factors, and the platform defaults of D-700 that go with it.
#[derive(Clone, Debug)]
pub struct FactorPolicy {
    /// The realms whose policy asks for a second factor (IDN-018). An account that has enrolled
    /// one is asked for it in every realm.
    pub realm_required: BTreeSet<TenantId>,
    /// A fresh authentication is one made within this long (D-700: fifteen minutes).
    pub step_up_age_ms: i64,
    /// Account recovery completes this long after it was asked for (D-700: twenty-four hours).
    pub recovery_delay_ms: i64,
    /// How long a recovery request can be completed once its delay has passed.
    pub recovery_window_ms: i64,
    /// Second-factor prompts sent to one account within the window (D-700: five in an hour,
    /// IDN-268).
    pub prompts_max: u32,
    pub prompts_window_ms: i64,
    /// A waiting sign-in is spent after this many presentations of a second factor (D-700: five,
    /// IDN-253).
    pub code_attempts_max: u32,
    /// How long a sign-in waits for its second factor.
    pub pending_ttl_ms: i64,
    /// The key secrets are sealed under from now on.
    pub seal_key: SealKey,
    /// Keys that sealed secrets earlier and still open them, so that a rotation does not lock
    /// anyone out. Secrets are re-sealed under `seal_key` when they are next written.
    pub previous_seal_keys: Vec<SealKey>,
}

impl Default for FactorPolicy {
    fn default() -> Self {
        FactorPolicy {
            realm_required: BTreeSet::new(),
            step_up_age_ms: 15 * 60_000,
            recovery_delay_ms: 24 * 3_600_000,
            recovery_window_ms: 24 * 3_600_000,
            prompts_max: 5,
            prompts_window_ms: 3_600_000,
            code_attempts_max: 5,
            pending_ttl_ms: 5 * 60_000,
            seal_key: SealKey::derive("strata development seal key"),
            previous_seal_keys: Vec::new(),
        }
    }
}

/// The limits on failed sign-in attempts. The defaults are the platform defaults of D-700; a
/// realm may tighten them (D-535).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThrottleLimits {
    /// Failed attempts for one account before it is locked (IDN-008).
    pub account_max: u32,
    /// How long a locked account refuses attempts.
    pub account_lock_ms: i64,
    /// Failures older than this are forgotten, for an account, a source and a realm alike.
    pub window_ms: i64,
    /// Failed attempts from one source, across all accounts, before it is refused (IDN-250).
    pub source_max: u32,
    /// Failed attempts across one realm within the window after which attempts are slowed
    /// (IDN-251).
    pub realm_max: u32,
    /// The gap kept between attempts of a realm that is slowed.
    pub realm_interval_ms: i64,
}

impl Default for ThrottleLimits {
    fn default() -> Self {
        ThrottleLimits {
            account_max: 10,
            account_lock_ms: 30_000,
            window_ms: 15 * 60_000,
            // The quota of the specification on password sign-in from one source (IDN-250):
            // the generated model is the one source of the number.
            source_max: u32::try_from(strata_model::quotas::Q_SIGN_IN_WITH_PASSWORD.limit)
                .unwrap_or(u32::MAX),
            realm_max: 1000,
            realm_interval_ms: 1000,
        }
    }
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
    /// Socket, sign-in and hashing limits in force.
    pub admission: Admission,
    pub started_at: i64,
    /// Recent security-relevant events, newest last: a cache for the development diagnostics.
    /// The record is the tenant's audit trail in the store; this ring is lost on restart.
    pub audit: Recent<serde_json::Value>,
    /// Recent authentication decisions, newest last. Kept apart from `audit`, so that a flood of
    /// sign-in attempts cannot push a security event out of its ring. Also only a cache.
    pub decisions: Recent<serde_json::Value>,
    /// Milliseconds added to the clock. Zero except in development, where a test moves time
    /// forward to see a lifetime or a revocation bound pass.
    pub clock_skew_ms: AtomicI64,
    /// Development only: the identity service is made to answer as if it could not be reached.
    pub identity_outage: AtomicBool,
    /// The aggregate results kept for an audience class (D-5064, D-5065).
    pub aggregate_copies: crate::aggregates::Copies,
}

pub type Shared = Arc<App>;

impl App {
    pub fn new(cfg: Config, store: Arc<dyn Store>) -> Shared {
        Arc::new(App {
            lab: Lab::with_limits(&cfg.limits),
            admission: Admission::new(&cfg.limits),
            cfg,
            store,
            hub: Hub::new(),
            stats: Stats::new(),
            started_at: now_ms(),
            audit: Recent::new(1000),
            decisions: Recent::new(1000),
            clock_skew_ms: AtomicI64::new(0),
            identity_outage: AtomicBool::new(false),
            aggregate_copies: Default::default(),
        })
    }

    /// Record a security-relevant event: a line in the audit trail of the tenant it names (the
    /// `tenant` field, or `session_tenant`), a warning in the log that an alert can be raised on
    /// (`event` starts with `audit.`), and an entry in the diagnostics ring. An event that names
    /// no tenant is logged and kept in the ring only.
    pub fn audit(&self, event: &str, correlation: &str, fields: serde_json::Value) {
        let mut entry = json!({ "at": now_ms(), "event": event, "correlation": correlation });
        if let (Some(entry), serde_json::Value::Object(fields)) = (entry.as_object_mut(), &fields) {
            entry.extend(fields.clone());
        }
        LOG.warn(&format!("audit.{event}"), correlation, fields.clone());
        let tenant = ["tenant", "session_tenant"]
            .iter()
            .find_map(|key| fields.get(key).and_then(|v| v.as_str()))
            .map(str::to_string);
        if let Some(tenant) = tenant {
            self.keep("audit", &tenant, event, correlation, &fields);
        }
        self.audit.push(entry);
    }

    /// Record an authentication decision: the method, the outcome and the checks evaluated
    /// (IDN-021). It names no secret, and no account that was not signed in (IDN-022). Written to
    /// the audit trail of `tenant` and to the log.
    pub fn decision(
        &self,
        tenant: &TenantId,
        method: &str,
        outcome: &str,
        checks: &[&str],
        correlation: &str,
    ) {
        let fields = json!({ "method": method, "outcome": outcome, "checks": checks });
        LOG.info("auth.decision", correlation, fields.clone());
        self.keep(
            "decision",
            tenant.as_str(),
            "auth.decision",
            correlation,
            &fields,
        );
        let mut entry = json!({
            "at": now_ms(),
            "event": "auth.decision",
            "correlation": correlation,
            "tenant": tenant.as_str(),
        });
        if let (Some(entry), serde_json::Value::Object(fields)) = (entry.as_object_mut(), &fields) {
            entry.extend(fields.clone());
        }
        self.decisions.push(entry);
    }

    /// Write one line of a tenant's audit trail. A failure is logged as an error and does not
    /// stop the request: the line is in the log either way.
    fn keep(
        &self,
        kind: &str,
        tenant: &str,
        event: &str,
        correlation: &str,
        fields: &serde_json::Value,
    ) {
        let written = TenantId::new(tenant)
            .map_err(|e| e.to_string())
            .and_then(|tenant| {
                self.store
                    .audit_append(
                        &tenant,
                        &AuditEntry {
                            at: now_ms(),
                            kind: kind.to_string(),
                            event: event.to_string(),
                            correlation: correlation.to_string(),
                            data: fields.to_string(),
                        },
                    )
                    .map_err(|e| e.to_string())
            });
        if let Err(error) = written {
            LOG.error(
                "audit.write_failed",
                correlation,
                json!({ "tenant": tenant, "event": event, "error": error }),
            );
        }
    }

    /// The one place the backend reads the clock for domain purposes.
    pub fn now(&self) -> Timestamp {
        Timestamp(now_ms() + self.clock_skew_ms.load(Ordering::Relaxed))
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

    /// [`blocking`](Self::blocking) for a request of `tenant`: the tenant may have only so many
    /// calls running or waiting, and all tenants together only so many, so one tenant cannot
    /// park the whole pool. Over either limit the request is refused with 429. The places are
    /// held until the work has run, even if the request is abandoned before then.
    pub async fn blocking_for<T, F>(self: &Arc<Self>, tenant: &str, work: F) -> Result<T, ApiError>
    where
        T: Send + 'static,
        F: FnOnce(&App) -> T + Send + 'static,
    {
        let place = self.admission.work(tenant)?;
        self.blocking(move |app| {
            let _place = place;
            work(app)
        })
        .await
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
