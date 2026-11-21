//! Delayed emits on the in-memory store and on a SQLite file. The cases themselves are in
//! `common/delayed_cases.rs`, shared with the storage-api adapter's tests. Each test runs its case
//! on both stores in turn.

#[path = "common/delayed_cases.rs"]
mod cases;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use strata_runtime::{Database, MemoryStore, SqliteMachineStore, TenantId, Time};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn tenant() -> TenantId {
    TenantId::new("acme").unwrap()
}

fn memory() -> impl Fn() -> Arc<MemoryStore> {
    let store = Arc::new(MemoryStore::new(tenant()));
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
    move || {
        Arc::new(
            SqliteMachineStore::open(tenant(), Database::open(&path).unwrap(), Time(0)).unwrap(),
        )
    }
}

#[tokio::test]
async fn waits_for_its_delay() {
    cases::waits_for_its_delay(memory()).await;
    cases::waits_for_its_delay(sqlite("waits")).await;
}

#[tokio::test]
async fn retries_only_at_least_once() {
    cases::retries_only_at_least_once(memory()).await;
    cases::retries_only_at_least_once(sqlite("retries")).await;
}

#[tokio::test]
async fn two_hosts_dispatch_once() {
    cases::two_hosts_dispatch_once(memory()).await;
    cases::two_hosts_dispatch_once(sqlite("two-hosts")).await;
}
