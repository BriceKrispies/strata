//! The lease machine on the in-memory store and on a SQLite file. The cases themselves are in
//! `common/lease_cases.rs`, shared with the storage-api adapter's tests.

#[path = "common/lease_cases.rs"]
mod cases;

use cases::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use strata_runtime::{Database, MemoryStore, SqliteMachineStore, Time};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A fresh database file under the build directory.
fn fresh_path(name: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "runtime-{name}-{}-{}.db",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}

/// The in-memory store: "reopening" hands back the same store, as a restarted process would
/// find its database.
fn memory() -> impl Fn() -> Arc<MemoryStore> {
    let store = Arc::new(MemoryStore::new());
    move || store.clone()
}

/// A SQLite file: every reopen is a new connection to the file.
fn sqlite(name: &str) -> impl Fn() -> Arc<SqliteMachineStore> {
    let path = fresh_path(name);
    move || Arc::new(SqliteMachineStore::open(Database::open(&path).unwrap(), Time(0)).unwrap())
}

#[tokio::test]
async fn memory_refuses_a_missing_record_when_not_creating() {
    refuses_a_missing_record_when_not_creating(memory()).await;
}

#[tokio::test]
async fn sqlite_refuses_a_missing_record_when_not_creating() {
    refuses_a_missing_record_when_not_creating(sqlite("not-creating")).await;
}

#[tokio::test]
async fn memory_persists() {
    persists(memory()).await;
}

#[tokio::test]
async fn sqlite_persists() {
    persists(sqlite("persists")).await;
}

#[tokio::test]
async fn memory_redelivers_after_a_crash() {
    redelivers_after_a_crash(memory()).await;
}

#[tokio::test]
async fn sqlite_redelivers_after_a_crash() {
    redelivers_after_a_crash(sqlite("redelivers")).await;
}

#[tokio::test]
async fn memory_fires_timers() {
    fires_timers(memory()).await;
}

#[tokio::test]
async fn sqlite_fires_timers() {
    fires_timers(sqlite("timers")).await;
}

#[tokio::test]
async fn memory_never_retries_at_most_once() {
    never_retries_at_most_once(memory()).await;
}

#[tokio::test]
async fn sqlite_never_retries_at_most_once() {
    never_retries_at_most_once(sqlite("at-most-once")).await;
}

#[tokio::test]
async fn memory_quarantines_an_unreadable_timer() {
    quarantines_an_unreadable_timer(memory()).await;
}

#[tokio::test]
async fn sqlite_quarantines_an_unreadable_timer() {
    quarantines_an_unreadable_timer(sqlite("quarantine")).await;
}

#[tokio::test]
async fn memory_refuses_an_elapsed_event_from_outside() {
    refuses_an_elapsed_event_from_outside(memory()).await;
}

#[tokio::test]
async fn sqlite_refuses_an_elapsed_event_from_outside() {
    refuses_an_elapsed_event_from_outside(sqlite("elapsed")).await;
}

#[tokio::test]
async fn memory_refuses_a_record_that_breaks_a_constraint() {
    refuses_a_record_that_breaks_a_constraint(memory()).await;
}

#[tokio::test]
async fn sqlite_refuses_a_record_that_breaks_a_constraint() {
    refuses_a_record_that_breaks_a_constraint(sqlite("constraint")).await;
}
