//! Delayed emits on the in-memory store and on a SQLite file. The cases themselves are in
//! `common/delayed_cases.rs`, shared with the storage-api adapter's tests.

#[path = "common/delayed_cases.rs"]
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
        "runtime-delayed-{name}-{}-{}.db",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    move || Arc::new(SqliteMachineStore::open(Database::open(&path).unwrap(), Time(0)).unwrap())
}

#[tokio::test]
async fn memory_waits_for_its_delay() {
    waits_for_its_delay(memory()).await;
}

#[tokio::test]
async fn sqlite_waits_for_its_delay() {
    waits_for_its_delay(sqlite("waits")).await;
}

#[tokio::test]
async fn memory_retries_only_at_least_once() {
    retries_only_at_least_once(memory()).await;
}

#[tokio::test]
async fn sqlite_retries_only_at_least_once() {
    retries_only_at_least_once(sqlite("retries")).await;
}

#[tokio::test]
async fn memory_two_hosts_dispatch_once() {
    two_hosts_dispatch_once(memory()).await;
}

#[tokio::test]
async fn sqlite_two_hosts_dispatch_once() {
    two_hosts_dispatch_once(sqlite("two-hosts")).await;
}
