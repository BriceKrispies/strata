//! What one visitor, session or tenant may cost the backend: open sockets, sign-in attempts,
//! concurrent password hashing, log lines and background work.
//!
//! The limits are configuration ([`Limits`], from the environment in `main.rs`) and the state
//! that enforces them ([`Admission`]) lives in the [`App`](crate::app::App). A "source" is the
//! address the BFF states in `x-strata-source`, with an IPv6 address counted as its /64; calls
//! without one share a single source, so leaving the header out is not a way round a limit.

use crate::app::ApiError;
use std::sync::Arc;
use std::time::Duration;
use strata_protocol::api::ErrorCode;
use strata_runtime::limits::{source_key, Cap, Permit, Rate};
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
    /// Argon2 verifications one tenant may hold at once, out of the pool
    /// (`STRATA_ARGON2_PER_TENANT`). A tenant over its share is refused at once, not queued.
    pub argon2_per_tenant: usize,
    /// Blocking store calls one tenant may have running or waiting
    /// (`STRATA_BLOCKING_PER_TENANT`); over it, a request is refused with 429.
    pub blocking_per_tenant: usize,
    /// Blocking store calls, all tenants together, that may be running or waiting
    /// (`STRATA_BLOCKING_QUEUE`); over it, a request is refused with 429.
    pub blocking_queue: usize,
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
            // The burst quota of the specification on password sign-in from one source
            // (`q-sign-in-with-password-burst`): the generated model is the one source of it.
            logins_per_source_per_minute: strata_model::quotas::Q_SIGN_IN_WITH_PASSWORD_BURST.limit
                as usize,
            argon2_memory_mib: 64,
            argon2_wait_ms: 3_000,
            argon2_per_tenant: 2,
            blocking_per_tenant: 16,
            blocking_queue: 256,
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
    argon2_by_tenant: Cap,
    work_by_tenant: Cap,
    work: Arc<Semaphore>,
}

/// A place to run Argon2 verifications: one of the global pool and one of the tenant's share.
/// Dropping it frees both.
pub struct HashingPlace {
    _pool: OwnedSemaphorePermit,
    _tenant: Permit,
}

/// A place in the blocking queue: one of the tenant's and one of the global queue.
pub struct WorkPlace {
    _global: OwnedSemaphorePermit,
    _tenant: Permit,
}

/// The one source every call without a source address counts under.
const NO_SOURCE: &str = "unknown";

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
            argon2_by_tenant: Cap::new(limits.argon2_per_tenant.max(1)),
            work_by_tenant: Cap::new(limits.blocking_per_tenant.max(1)),
            work: Arc::new(Semaphore::new(limits.blocking_queue.max(1))),
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
        let by_source = match source {
            Some(source) => Some(
                self.sockets_by_source
                    .acquire(&source_key(source))
                    .ok_or_else(|| throttled("Too many connections are open from this address."))?,
            ),
            None => None,
        };
        Ok(SocketPlaces {
            source: source.map(str::to_string),
            _session: by_session,
            _source: by_source,
        })
    }

    /// A place in the blocking queue for `tenant`, or a 429. Hold it until the work is done,
    /// including when the request that asked for it has gone away.
    pub fn work(&self, tenant: &str) -> Result<WorkPlace, ApiError> {
        let by_tenant = self.work_by_tenant.acquire(tenant).ok_or_else(|| {
            throttled("This site has too many requests in progress. Try again shortly.")
        })?;
        let global = self
            .work
            .clone()
            .try_acquire_owned()
            .map_err(|_| throttled("The service is busy. Try again shortly."))?;
        Ok(WorkPlace {
            _global: global,
            _tenant: by_tenant,
        })
    }

    /// Whether `source` may write another browser log line now.
    pub fn diag_allowed(&self, source: Option<&str>, now_ms: i64) -> bool {
        self.diag
            .allow(&source_key(source.unwrap_or(NO_SOURCE)), now_ms)
    }

    /// Count a sign-in attempt from `source`; refused when it has made too many this minute.
    /// Calls with no source count together as one.
    pub fn login_attempt(&self, source: Option<&str>, now_ms: i64) -> Result<(), ApiError> {
        if self
            .logins
            .allow(&source_key(source.unwrap_or(NO_SOURCE)), now_ms)
        {
            Ok(())
        } else {
            Err(throttled(
                "Too many sign-in attempts from this address. Wait a minute and try again.",
            ))
        }
    }

    /// A place to run Argon2 verifications for `tenant`. A tenant already holding its share is
    /// refused at once; otherwise this waits briefly for the pool, and is refused when no place
    /// frees. Hold it only around the hashing, never across a network call.
    pub async fn hashing(&self, tenant: &str) -> Result<HashingPlace, ApiError> {
        let busy = || throttled("Sign-in is busy right now. Wait a moment and try again.");
        let by_tenant = self.argon2_by_tenant.acquire(tenant).ok_or_else(busy)?;
        match tokio::time::timeout(self.argon2_wait, self.argon2.clone().acquire_owned()).await {
            Ok(Ok(pool)) => Ok(HashingPlace {
                _pool: pool,
                _tenant: by_tenant,
            }),
            _ => Err(busy()),
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
        let held = admission.hashing("t").await.expect("the one place");
        assert_eq!(admission.hashing_available(), 0);
        let refused = admission.hashing("u").await.err().expect("no second place");
        assert_eq!(refused.0.code, ErrorCode::Throttled);
        drop(held);
        assert!(admission.hashing("t").await.is_ok(), "the place was freed");
    }

    #[tokio::test]
    async fn one_tenant_cannot_take_the_whole_hashing_pool() {
        let admission = Admission::new(&Limits {
            argon2_memory_mib: 38,
            argon2_per_tenant: 1,
            argon2_wait_ms: 30,
            ..Limits::default()
        });
        let _first = admission.hashing("a").await.ok().expect("a's share");
        assert!(admission.hashing("a").await.is_err(), "a's second place");
        assert!(admission.hashing("b").await.is_ok(), "b has the other");
    }

    #[test]
    fn blocking_work_is_capped_per_tenant_and_in_total() {
        let admission = Admission::new(&Limits {
            blocking_per_tenant: 2,
            blocking_queue: 3,
            ..Limits::default()
        });
        let a1 = admission.work("a").ok().expect("a1");
        let _a2 = admission.work("a").ok().expect("a2");
        assert!(admission.work("a").is_err(), "a's third");
        let _b1 = admission.work("b").ok().expect("b1");
        assert!(admission.work("c").is_err(), "the global queue is full");
        drop(a1);
        assert!(admission.work("c").is_ok(), "a place freed");
    }

    #[test]
    fn calls_without_a_source_share_one_login_rate() {
        let admission = Admission::new(&Limits {
            logins_per_source_per_minute: 2,
            ..Limits::default()
        });
        assert!(admission.login_attempt(None, 0).is_ok());
        assert!(admission.login_attempt(None, 1).is_ok());
        assert!(admission.login_attempt(None, 2).is_err());
        assert!(admission.login_attempt(Some("1.1.1.1"), 2).is_ok());
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
