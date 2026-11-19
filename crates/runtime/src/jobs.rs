//! Background jobs: work that runs for as long as the process does.
//!
//! Two shapes cover what the services need. [`every`] runs work on a fixed period, such as a
//! purge of expired rows. [`pump`] runs work, then waits until it is woken or a period passes,
//! and runs it again; an outbox publisher uses it, woken by each commit and polling for commits
//! made by other processes.

use std::future::Future;
use std::time::Duration;
use tokio::sync::Notify;

/// Run `work` every `period`, forever, starting one period from now.
pub async fn every<F, Fut>(period: Duration, mut work: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    loop {
        tokio::time::sleep(period).await;
        work().await;
    }
}

/// Run `work` now, then again each time `wake` is notified or `idle` passes without a
/// notification, forever.
pub async fn pump<F, Fut>(wake: &Notify, idle: Duration, mut work: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    loop {
        work().await;
        let _ = tokio::time::timeout(idle, wake.notified()).await;
    }
}
