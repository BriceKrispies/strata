//! The system host on the in-memory store and on a SQLite file. The cases themselves are in
//! `common/system_cases.rs`, shared with the storage-api adapter's tests. Each test runs its case
//! on both stores in turn.

#[path = "common/system_cases.rs"]
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
async fn delivers_at_deadlines() {
    cases::delivers_at_deadlines(memory()).await;
    cases::delivers_at_deadlines(sqlite("deadlines")).await;
}

#[tokio::test]
async fn ignores_without_writing() {
    cases::ignores_without_writing(memory()).await;
    cases::ignores_without_writing(sqlite("ignores")).await;
}

#[tokio::test]
async fn a_failed_step_commits_nothing() {
    cases::a_failed_step_commits_nothing(memory()).await;
    cases::a_failed_step_commits_nothing(sqlite("failed")).await;
}

#[tokio::test]
async fn quarantines_an_unreadable_entry() {
    cases::quarantines_an_unreadable_entry(memory()).await;
    cases::quarantines_an_unreadable_entry(sqlite("quarantine")).await;
}

#[tokio::test]
async fn two_hosts_dispatch_once() {
    cases::two_hosts_dispatch_once(memory()).await;
    cases::two_hosts_dispatch_once(sqlite("two-hosts")).await;
}

#[tokio::test]
async fn refuses_an_elapsed_event_from_outside() {
    cases::refuses_an_elapsed_event_from_outside(memory()).await;
    cases::refuses_an_elapsed_event_from_outside(sqlite("elapsed")).await;
}

#[tokio::test]
async fn refuses_a_record_that_breaks_a_constraint() {
    cases::refuses_a_record_that_breaks_a_constraint(memory()).await;
    cases::refuses_a_record_that_breaks_a_constraint(sqlite("constraint")).await;
}

#[tokio::test]
async fn redelivers_at_least_once_after_a_stop_between_dispatch_and_removal() {
    cases::redelivers_at_least_once_after_a_stop_between_dispatch_and_removal(memory()).await;
    cases::redelivers_at_least_once_after_a_stop_between_dispatch_and_removal(sqlite("redeliver"))
        .await;
}

#[tokio::test]
async fn loses_at_most_once_on_a_stop_between_commit_and_dispatch() {
    cases::loses_at_most_once_on_a_stop_between_commit_and_dispatch(memory()).await;
    cases::loses_at_most_once_on_a_stop_between_commit_and_dispatch(sqlite("lose")).await;
}

#[tokio::test]
async fn retries_at_least_once_with_backoff() {
    cases::retries_at_least_once_with_backoff(memory()).await;
    cases::retries_at_least_once_with_backoff(sqlite("retry")).await;
}

#[tokio::test]
async fn refuses_an_instance_beyond_max() {
    cases::refuses_an_instance_beyond_max(memory()).await;
    cases::refuses_an_instance_beyond_max(sqlite("max")).await;
}

/// Verifies: OPS-2440
#[tokio::test]
async fn a_singleton_takes_its_first_event_without_seeding() {
    cases::a_singleton_takes_its_first_event_without_seeding(memory()).await;
    cases::a_singleton_takes_its_first_event_without_seeding(sqlite("singleton")).await;
}

/// Verifies: OPS-2440
#[tokio::test]
async fn two_systems_over_one_store_make_one_singleton() {
    cases::two_systems_over_one_store_make_one_singleton(memory()).await;
    cases::two_systems_over_one_store_make_one_singleton(sqlite("two-singletons")).await;
}

/// Verifies: OPS-2441
#[tokio::test]
async fn a_message_that_always_fails_is_dead_lettered_after_the_cap() {
    cases::a_message_that_always_fails_is_dead_lettered_after_the_cap(memory()).await;
    cases::a_message_that_always_fails_is_dead_lettered_after_the_cap(sqlite("cap")).await;
}

/// Verifies: OPS-2442
#[tokio::test]
async fn purges_dead_letters_past_retention() {
    cases::purges_dead_letters_past_retention(memory()).await;
    cases::purges_dead_letters_past_retention(sqlite("purge-dead")).await;
}

/// Verifies: OPS-2442
#[tokio::test]
async fn a_write_deletes_the_dead_letters_past_the_bound() {
    cases::a_write_deletes_the_dead_letters_past_the_bound(memory()).await;
    cases::a_write_deletes_the_dead_letters_past_the_bound(sqlite("write-purge")).await;
}

/// Verifies: OPS-2442
#[tokio::test]
async fn purges_finished_records_past_retention() {
    cases::purges_finished_records_past_retention(memory()).await;
    cases::purges_finished_records_past_retention(sqlite("purge-records")).await;
}
