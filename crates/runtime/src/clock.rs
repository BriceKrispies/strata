//! Time as the runtime sees it.
//!
//! Every timer deadline, outbox retry and "is it due" decision reads a [`Clock`], never the
//! system time directly, so a test can hold time still and move it by hand with [`TestClock`].
//! Durations that only measure how long something took (telemetry spans) are not decisions and
//! use the monotonic clock of the process.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A point in time: milliseconds since the Unix epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Time(pub i64);

impl Time {
    /// The time `duration` after this one, saturating at the end of the range.
    pub fn after(self, duration: Duration) -> Time {
        let millis = i64::try_from(duration.as_millis()).unwrap_or(i64::MAX);
        Time(self.0.saturating_add(millis))
    }
}

/// A source of the current time.
pub trait Clock: Send + Sync + 'static {
    /// The current time.
    fn now(&self) -> Time;
}

/// The system's wall clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Time {
        Time(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)),
        )
    }
}

/// A clock that only moves when told to. Clones share the same time.
#[derive(Clone, Debug)]
pub struct TestClock {
    now: Arc<AtomicI64>,
}

impl TestClock {
    /// A clock standing at `start`.
    pub fn at(start: Time) -> Self {
        TestClock {
            now: Arc::new(AtomicI64::new(start.0)),
        }
    }

    /// Move the clock forward by `by`.
    pub fn advance(&self, by: Duration) {
        let millis = i64::try_from(by.as_millis()).unwrap_or(i64::MAX);
        self.now.fetch_add(millis, Ordering::SeqCst);
    }

    /// Set the clock to `to`, which may be earlier than the current time.
    pub fn set(&self, to: Time) {
        self.now.store(to.0, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now(&self) -> Time {
        Time(self.now.load(Ordering::SeqCst))
    }
}
