//! The system host on the in-memory store and on a SQLite file. The cases themselves are in
//! `common/system_cases.rs`, shared with the storage-api adapter's tests.

#[path = "common/system_cases.rs"]
mod cases;

use cases::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use strata_runtime::{Database, MemoryStore, SqliteMachineStore, Time};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn memory() -> impl Fn() -> Arc<MemoryStore> {
    let store = Arc::new(MemoryStore::new());
    move || store.clone()
}

fn sqlite(name: &str) -> impl Fn() -> Arc<SqliteMachineStore> {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "runtime-system-{name}-{}-{}.db",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    move || Arc::new(SqliteMachineStore::open(Database::open(&path).unwrap(), Time(0)).unwrap())
}

#[tokio::test]
async fn memory_delivers_at_deadlines() {
    delivers_at_deadlines(memory()).await;
}

#[tokio::test]
async fn sqlite_delivers_at_deadlines() {
    delivers_at_deadlines(sqlite("deadlines")).await;
}

#[tokio::test]
async fn memory_ignores_without_writing() {
    ignores_without_writing(memory()).await;
}

#[tokio::test]
async fn sqlite_ignores_without_writing() {
    ignores_without_writing(sqlite("ignores")).await;
}

#[tokio::test]
async fn memory_a_failed_step_commits_nothing() {
    a_failed_step_commits_nothing(memory()).await;
}

#[tokio::test]
async fn sqlite_a_failed_step_commits_nothing() {
    a_failed_step_commits_nothing(sqlite("failed")).await;
}
