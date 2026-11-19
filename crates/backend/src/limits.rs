//! What one visitor, session or tenant may cost the backend: open sockets, sign-in attempts,
//! concurrent password hashing, log lines and background work.
//!
//! The limits are configuration ([`Limits`], from the environment in `main.rs`) and the state
//! that enforces them ([`Admission`]) lives in the [`App`](crate::app::App). A "source" is the
//! address the BFF states in `x-strata-source`; a call without one (the operator's own tools)
//! is not limited per source.

use crate::app::ApiError;
use std::sync::Arc;
use std::time::Duration;
use strata_protocol::api::ErrorCode;
use strata_runtime::limits::{Cap, Permit, Rate};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// The memory one Argon2 verification takes with the default parameters (m = 19456 KiB).
pub const ARGON2_MIB: u32 = 19;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Open sockets one session may hold (`STRATA_SOCKETS_PER_SESSION`).
    pub sockets_per_session: usize,
    /// Open sockets one source address may hold, signed in or not (`STRATA_SOCKETS_PER_SOURCE`).
    pub sockets_per_source: usize,
    /// Browser log lines one development socket may write (`STRATA_DIAG_PER_SOCKET`).
    pub diag_per_socket: u32,
    /// Browser log lines one source may write per minute (`STRATA_DIAG_PER_SOURCE`).
    pub diag_per_source_per_minute: usize,
    /// Sign-in attempts one source may make per minute (`STRATA_LOGINS_PER_SOURCE`).
    pub logins_per_source_per_minute: usize,
    /// Memory, in MiB, that concurrent Argon2 verifications may take together
    /// (`STRATA_ARGON2_MEMORY_MIB`). One verification takes [`ARGON2_MIB`].
    pub argon2_memory_mib: u32,
    /// How long a sign-in waits for a place to hash before it is refused
    /// (`STRATA_ARGON2_WAIT_MS`).
    pub argon2_wait_ms: u64,
    /// Threads that run Impact Lab projections (`STRATA_LAB_WORKERS`).
    pub lab_workers: usize,
    /// Projections that may wait for a thread (`STRATA_LAB_QUEUE`).
    pub lab_queue: usize,
    /// Projections one tenant may have running or waiting (`STRATA_LAB_RUNS_PER_TENANT`).
    pub lab_runs_per_tenant: usize,
    /// The least time between two verification runs of one tenant (`STRATA_VERIFY_INTERVAL_MS`).
    pub verify_interval_ms: i64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            sockets_per_session: 8,
            sockets_per_source: 32,
            diag_per_socket: 50,
            diag_per_source_per_minute: 200,
            logins_per_source_per_minute: 30,
            argon2_memory_mib: 64,
            argon2_wait_ms: 3_000,
            lab_workers: 2,
            lab_queue: 4,
            lab_runs_per_tenant: 2,
            verify_interval_ms: 60_000,
        }
    }
}

impl Limits {
    /// Concurrent Argon2 verifications the memory budget allows: at least one.
    pub fn argon2_permits(&self) -> usize {
        ((self.argon2_memory_mib / ARGON2_MIB) as usize).max(1)
    }
}

/// The places a connection holds while it is open. Dropping it frees them.
pub struct SocketPlaces {
    pub source: Option<String>,
    _session: Option<Permit>,
    _source: Option<Permit>,
}

pub struct Admission {
    sockets_by_session: Cap,
    sockets_by_source: Cap,
    diag: Rate,
    logins: Rate,
    argon2: Arc<Semaphore>,
    argon2_wait: Duration,
}

fn throttled(message: &str) -> ApiError {
    ApiError::new(ErrorCode::Throttled, message)
}

impl Admission {
    pub fn new(limits: &Limits) -> Self {
        Admission {
            sockets_by_session: Cap::new(limits.sockets_per_session),
            sockets_by_source: Cap::new(limits.sockets_per_source),
            diag: Rate::new(limits.diag_per_source_per_minute, 60_000),
            logins: Rate::new(limits.logins_per_source_per_minute, 60_000),
            argon2: Arc::new(Semaphore::new(limits.argon2_permits())),
            argon2_wait: Duration::from_millis(limits.argon2_wait_ms),
        }
    }

    /// Places for a new socket of `session` (when signed in) from `source`, or the refusal.
    pub fn socket(
        &self,
        session: Option<&str>,
        source: Option<&str>,
    ) -> Result<SocketPlaces, ApiError> {
        let by_session = match session {
            Some(session) => Some(self.sockets_by_session.acquire(session).ok_or_else(|| {
                throttled("This session already has as many connections open as it may.")
            })?),
            None => None,
        };
        let by_source =
            match source {
                Some(source) => Some(self.sockets_by_source.acquire(source).ok_or_else(|| {
                    throttled("Too many connections are open from this address.")
                })?),
                None => None,
            };
        Ok(SocketPlaces {
            source: source.map(str::to_string),
            _session: by_session,
            _source: by_source,
        })
    }

    /// Whether `source` may write another browser log line now.
    pub fn diag_allowed(&self, source: Option<&str>, now_ms: i64) -> bool {
        source.is_none_or(|source| self.diag.allow(source, now_ms))
    }

    /// Count a sign-in attempt from `source`; refused when it has made too many this minute.
    pub fn login_attempt(&self, source: Option<&str>, now_ms: i64) -> Result<(), ApiError> {
        if source.is_none_or(|source| self.logins.allow(source, now_ms)) {
            Ok(())
        } else {
            Err(throttled(
                "Too many sign-in attempts from this address. Wait a minute and try again.",
            ))
        }
    }

    /// A place to run one Argon2 verification. Waits briefly for one; refused when none frees.
    pub async fn hashing(&self) -> Result<OwnedSemaphorePermit, ApiError> {
        match tokio::time::timeout(self.argon2_wait, self.argon2.clone().acquire_owned()).await {
            Ok(Ok(permit)) => Ok(permit),
            _ => Err(throttled(
                "Sign-in is busy right now. Wait a moment and try again.",
            )),
        }
    }

    /// Verifications that could start right now.
    pub fn hashing_available(&self) -> usize {
        self.argon2.available_permits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_memory_budget_sets_the_number_of_concurrent_verifications() {
        let sized = |mib| {
            Limits {
                argon2_memory_mib: mib,
                ..Limits::default()
            }
            .argon2_permits()
        };
        assert_eq!(sized(0), 1, "never none");
        assert_eq!(sized(19), 1);
        assert_eq!(sized(38), 2);
        assert_eq!(sized(64), 3);
    }

    #[test]
    fn a_source_may_write_only_so_many_browser_log_lines_per_minute() {
        let admission = Admission::new(&Limits {
            diag_per_source_per_minute: 3,
            ..Limits::default()
        });
        assert!((0..3).all(|n| admission.diag_allowed(Some("1.1.1.1"), n)));
        assert!(!admission.diag_allowed(Some("1.1.1.1"), 10), "the fourth");
        assert!(
            admission.diag_allowed(Some("2.2.2.2"), 10),
            "another source"
        );
        assert!(
            admission.diag_allowed(Some("1.1.1.1"), 60_001),
            "a minute later"
        );
    }

    #[tokio::test]
    async fn a_sign_in_that_finds_no_place_to_hash_is_refused() {
        let admission = Admission::new(&Limits {
            argon2_memory_mib: 19,
            argon2_wait_ms: 30,
            ..Limits::default()
        });
        let held = admission.hashing().await.expect("the one place");
        assert_eq!(admission.hashing_available(), 0);
        let refused = admission.hashing().await.expect_err("no second place");
        assert_eq!(refused.0.code, ErrorCode::Throttled);
        drop(held);
        assert!(admission.hashing().await.is_ok(), "the place was freed");
    }

    #[test]
    fn sockets_are_capped_per_session_and_per_source_and_freed_on_close() {
        let admission = Admission::new(&Limits {
            sockets_per_session: 2,
            sockets_per_source: 3,
            ..Limits::default()
        });
        let a = admission.socket(Some("s1"), Some("1.1.1.1")).unwrap();
        let _b = admission.socket(Some("s1"), Some("1.1.1.1")).unwrap();
        assert!(
            admission.socket(Some("s1"), Some("2.2.2.2")).is_err(),
            "third of one session"
        );
        let _c = admission.socket(Some("s2"), Some("1.1.1.1")).unwrap();
        assert!(
            admission.socket(None, Some("1.1.1.1")).is_err(),
            "fourth from one source"
        );
        assert!(
            admission.socket(None, None).is_ok(),
            "no source, no session: unlimited"
        );
        drop(a);
        assert!(admission.socket(Some("s1"), Some("1.1.1.1")).is_ok());
    }
}
