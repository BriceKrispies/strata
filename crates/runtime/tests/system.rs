//! The system host on the in-memory store and on a SQLite file. The cases themselves are in
//! `common/system_cases.rs`, shared with the storage-api adapter's tests.

#[path = "common/system_cases.rs"]
mod cases;

use cases::*;
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
        "runtime-system-{name}-{}-{}.db",
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

#[tokio::test]
async fn memory_quarantines_an_unreadable_entry() {
    quarantines_an_unreadable_entry(memory()).await;
}

#[tokio::test]
async fn sqlite_quarantines_an_unreadable_entry() {
    quarantines_an_unreadable_entry(sqlite("quarantine")).await;
}

#[tokio::test]
async fn memory_two_hosts_dispatch_once() {
    two_hosts_dispatch_once(memory()).await;
}

#[tokio::test]
async fn sqlite_two_hosts_dispatch_once() {
    two_hosts_dispatch_once(sqlite("two-hosts")).await;
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

#[tokio::test]
async fn memory_redelivers_at_least_once_after_a_stop_between_dispatch_and_removal() {
    redelivers_at_least_once_after_a_stop_between_dispatch_and_removal(memory()).await;
}

#[tokio::test]
async fn sqlite_redelivers_at_least_once_after_a_stop_between_dispatch_and_removal() {
    redelivers_at_least_once_after_a_stop_between_dispatch_and_removal(sqlite("redeliver")).await;
}

#[tokio::test]
async fn memory_loses_at_most_once_on_a_stop_between_commit_and_dispatch() {
    loses_at_most_once_on_a_stop_between_commit_and_dispatch(memory()).await;
}

#[tokio::test]
async fn sqlite_loses_at_most_once_on_a_stop_between_commit_and_dispatch() {
    loses_at_most_once_on_a_stop_between_commit_and_dispatch(sqlite("lose")).await;
}

#[tokio::test]
async fn memory_retries_at_least_once_with_backoff() {
    retries_at_least_once_with_backoff(memory()).await;
}

#[tokio::test]
async fn sqlite_retries_at_least_once_with_backoff() {
    retries_at_least_once_with_backoff(sqlite("retry")).await;
}

#[tokio::test]
async fn memory_refuses_an_instance_beyond_max() {
    refuses_an_instance_beyond_max(memory()).await;
}

#[tokio::test]
async fn sqlite_refuses_an_instance_beyond_max() {
    refuses_an_instance_beyond_max(sqlite("max")).await;
}

/// Verifies: OPS-2440
#[tokio::test]
async fn memory_a_singleton_takes_its_first_event_without_seeding() {
    a_singleton_takes_its_first_event_without_seeding(memory()).await;
}

/// Verifies: OPS-2440
#[tokio::test]
async fn sqlite_a_singleton_takes_its_first_event_without_seeding() {
    a_singleton_takes_its_first_event_without_seeding(sqlite("singleton")).await;
}

/// Verifies: OPS-2440
#[tokio::test]
async fn memory_two_systems_over_one_store_make_one_singleton() {
    two_systems_over_one_store_make_one_singleton(memory()).await;
}

/// Verifies: OPS-2440
#[tokio::test]
async fn sqlite_two_systems_over_one_store_make_one_singleton() {
    two_systems_over_one_store_make_one_singleton(sqlite("two-singletons")).await;
}

/// Verifies: OPS-2441
#[tokio::test]
async fn memory_a_message_that_always_fails_is_dead_lettered_after_the_cap() {
    a_message_that_always_fails_is_dead_lettered_after_the_cap(memory()).await;
}

/// Verifies: OPS-2441
#[tokio::test]
async fn sqlite_a_message_that_always_fails_is_dead_lettered_after_the_cap() {
    a_message_that_always_fails_is_dead_lettered_after_the_cap(sqlite("cap")).await;
}

/// Verifies: OPS-2442
#[tokio::test]
async fn memory_purges_dead_letters_past_retention() {
    purges_dead_letters_past_retention(memory()).await;
}

/// Verifies: OPS-2442
#[tokio::test]
async fn sqlite_purges_dead_letters_past_retention() {
    purges_dead_letters_past_retention(sqlite("purge-dead")).await;
}

/// Verifies: OPS-2442
#[tokio::test]
async fn memory_a_write_deletes_the_dead_letters_past_the_bound() {
    a_write_deletes_the_dead_letters_past_the_bound(memory()).await;
}

/// Verifies: OPS-2442
#[tokio::test]
async fn sqlite_a_write_deletes_the_dead_letters_past_the_bound() {
    a_write_deletes_the_dead_letters_past_the_bound(sqlite("write-purge")).await;
}

/// Verifies: OPS-2442
#[tokio::test]
async fn memory_purges_finished_records_past_retention() {
    purges_finished_records_past_retention(memory()).await;
}

/// Verifies: OPS-2442
#[tokio::test]
async fn sqlite_purges_finished_records_past_retention() {
    purges_finished_records_past_retention(sqlite("purge-records")).await;
}
