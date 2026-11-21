//! The runtime's own host cases, run on the machine store over `strata-storage-api`, with the
//! storage-api memory store and with the SQLite store. The cases are the files the runtime's
//! tests use for its own stores (`crates/runtime/tests/common`), so the same behaviour is held
//! to every store. Each test runs its case on both stores in turn.

#[path = "../../runtime/tests/common/system_cases.rs"]
mod system;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use strata_kernel::ext::*;
use strata_kernel::TenantId;
use strata_machine_store::StorageApiMachineStore;
use strata_runtime::SystemClock;
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::Store;
use strata_storage_sqlite::SqliteStore;

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn tenant() -> TenantId {
    TenantId::new("runtime").unwrap()
}

fn on(store: Arc<dyn Store>) -> Arc<StorageApiMachineStore> {
    Arc::new(StorageApiMachineStore::new(
        store,
        tenant(),
        Arc::new(SystemClock),
    ))
}

/// The storage-api memory store: "reopening" is a new adapter on the same store, as a restarted
/// process would find its database.
fn api_memory() -> impl Fn() -> Arc<StorageApiMachineStore> {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
    move || on(store.clone())
}

/// The SQLite store: every reopen is a new connection to the file.
fn api_sqlite(name: &str) -> impl Fn() -> Arc<StorageApiMachineStore> {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "machine-store-{name}-{}-{}.db",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    move || {
        on(Arc::new(
            SqliteStore::open(&path, strata_kernel::Timestamp(0)).unwrap(),
        ))
    }
}

/// Verifies: OPS-2440
#[tokio::test]
async fn two_systems_over_one_store_make_one_singleton() {
    system::two_systems_over_one_store_make_one_singleton(api_memory()).await;
    system::two_systems_over_one_store_make_one_singleton(api_sqlite("two-singletons")).await;
}

/// Verifies: OPS-2441
#[tokio::test]
async fn a_message_that_always_fails_is_dead_lettered_after_the_cap() {
    system::a_message_that_always_fails_is_dead_lettered_after_the_cap(api_memory()).await;
    system::a_message_that_always_fails_is_dead_lettered_after_the_cap(api_sqlite("cap")).await;
}

/// Verifies: OPS-2442
#[tokio::test]
async fn purges_dead_letters_past_retention() {
    system::purges_dead_letters_past_retention(api_memory()).await;
    system::purges_dead_letters_past_retention(api_sqlite("purge-dead")).await;
}
