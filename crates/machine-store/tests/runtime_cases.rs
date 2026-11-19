//! The runtime's own host cases, run on the machine store over `strata-storage-api`, with the
//! storage-api memory store and with the SQLite store. The cases are the files the runtime's
//! tests use for its own stores (`crates/runtime/tests/common`), so the same behaviour is held
//! to every store.

#[path = "../../runtime/tests/common/delayed_cases.rs"]
mod delayed;
#[path = "../../runtime/tests/common/lease_cases.rs"]
mod lease;
#[path = "../../runtime/tests/common/system_cases.rs"]
mod system;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use strata_kernel::ext::*;
use strata_kernel::{EventBody, FieldId, ObjectTypeId, TenantId, Value};
use strata_machine_store::{StorageApiMachineStore, OUTBOX, TIMERS};
use strata_runtime::store::{NewMessage, NewTimer, OutboxStore, RecordStore, TimerStore};
use strata_runtime::{Delivery, Isolation, StorageError, SystemClock, Time};
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

#[tokio::test]
async fn api_memory_refuses_a_missing_record_when_not_creating() {
    lease::refuses_a_missing_record_when_not_creating(api_memory()).await;
}

#[tokio::test]
async fn api_sqlite_refuses_a_missing_record_when_not_creating() {
    lease::refuses_a_missing_record_when_not_creating(api_sqlite("not-creating")).await;
}

#[tokio::test]
async fn api_memory_persists() {
    lease::persists(api_memory()).await;
}

#[tokio::test]
async fn api_sqlite_persists() {
    lease::persists(api_sqlite("persists")).await;
}

#[tokio::test]
async fn api_memory_redelivers_after_a_crash() {
    lease::redelivers_after_a_crash(api_memory()).await;
}

#[tokio::test]
async fn api_sqlite_redelivers_after_a_crash() {
    lease::redelivers_after_a_crash(api_sqlite("redelivers")).await;
}

#[tokio::test]
async fn api_memory_fires_timers() {
    lease::fires_timers(api_memory()).await;
}

#[tokio::test]
async fn api_sqlite_fires_timers() {
    lease::fires_timers(api_sqlite("timers")).await;
}

#[tokio::test]
async fn api_memory_never_retries_at_most_once() {
    lease::never_retries_at_most_once(api_memory()).await;
}

#[tokio::test]
async fn api_sqlite_never_retries_at_most_once() {
    lease::never_retries_at_most_once(api_sqlite("at-most-once")).await;
}

#[tokio::test]
async fn api_memory_waits_for_its_delay() {
    delayed::waits_for_its_delay(api_memory()).await;
}

#[tokio::test]
async fn api_sqlite_waits_for_its_delay() {
    delayed::waits_for_its_delay(api_sqlite("waits")).await;
}

#[tokio::test]
async fn api_memory_retries_only_at_least_once() {
    delayed::retries_only_at_least_once(api_memory()).await;
}

#[tokio::test]
async fn api_sqlite_retries_only_at_least_once() {
    delayed::retries_only_at_least_once(api_sqlite("retries")).await;
}

#[tokio::test]
async fn api_memory_delivers_at_deadlines() {
    system::delivers_at_deadlines(api_memory()).await;
}

#[tokio::test]
async fn api_sqlite_delivers_at_deadlines() {
    system::delivers_at_deadlines(api_sqlite("deadlines")).await;
}

#[tokio::test]
async fn api_memory_ignores_without_writing() {
    system::ignores_without_writing(api_memory()).await;
}

#[tokio::test]
async fn api_sqlite_ignores_without_writing() {
    system::ignores_without_writing(api_sqlite("ignores")).await;
}

#[tokio::test]
async fn api_memory_a_failed_step_commits_nothing() {
    system::a_failed_step_commits_nothing(api_memory()).await;
}

#[tokio::test]
async fn api_sqlite_a_failed_step_commits_nothing() {
    system::a_failed_step_commits_nothing(api_sqlite("failed")).await;
}

fn message(source: &str) -> NewMessage {
    NewMessage {
        source: source.into(),
        key: "\"k\"".into(),
        receiver: "mailer".into(),
        event: "{}".into(),
        created_at: Time(1),
        delivery: Delivery::AtLeastOnce,
        not_before: Time(1),
        next_attempt_at: Time(1),
    }
}

/// What the adapter writes is store records and store events: a machine record appears as a
/// record with its key and JSON data and as a `RecordCreated` event, the outbox and timers are
/// records without events, and a transaction that fails leaves nothing.
#[test]
fn a_transaction_is_one_store_commit() {
    let api: Arc<dyn Store> = Arc::new(MemoryStore::new());
    let store = on(api.clone());

    let failed = store.transact(Isolation::Serializable, |tx| -> Result<(), StorageError> {
        tx.save("rec-thing", "\"a\"", "{\"n\":1}", None)?;
        Err(StorageError::Invalid("refused".into()))
    });
    assert!(failed.is_err());
    assert!(store.pending().unwrap() == 0);
    let revision_after_failure = api
        .load_complete(&tenant())
        .unwrap()
        .tenant
        .unwrap()
        .revision;

    store
        .transact(Isolation::Serializable, |tx| -> Result<(), StorageError> {
            assert_eq!(tx.save("rec-thing", "\"a\"", "{\"n\":1}", None)?, 1);
            // The transaction reads its own writes.
            assert_eq!(tx.load("rec-thing", "\"a\"")?.unwrap().version, 1);
            assert_eq!(tx.save("rec-thing", "\"a\"", "{\"n\":2}", Some(1))?, 2);
            tx.enqueue(&message("thing"))?;
            tx.set_timer(&NewTimer {
                machine: "thing".into(),
                key: "\"a\"".into(),
                name: "expire".into(),
                fire_at: Time(5),
                event: "{}".into(),
            })?;
            Ok(())
        })
        .unwrap();

    let state = api.load_complete(&tenant()).unwrap().tenant.unwrap();
    assert_eq!(state.revision.0, revision_after_failure.0 + 1, "one commit");
    let thing = state
        .records_of(&ObjectTypeId::new("rec_thing").unwrap())
        .next();
    let thing = thing.expect("the machine record is a store record");
    assert_eq!(thing.version, 2);
    assert_eq!(
        thing.get(&FieldId::new("data").unwrap()),
        &Value::Str("{\"n\":2}".into())
    );
    assert_eq!(
        state
            .records_of(&ObjectTypeId::new(OUTBOX).unwrap())
            .count(),
        1
    );
    assert_eq!(
        state
            .records_of(&ObjectTypeId::new(TIMERS).unwrap())
            .count(),
        1
    );

    let events = api.events_after(&tenant(), 0, 100).unwrap();
    let created: Vec<_> = events
        .iter()
        .filter(|e| matches!(e.body, EventBody::RecordCreated { .. }))
        .collect();
    assert_eq!(created.len(), 1, "the outbox and timer rows emit no event");
}

/// Setting a timer again replaces it under a new id, a cancelled or taken timer does not fire,
/// and an outbox entry is due only when both its times have come.
#[test]
fn timers_and_outbox_follow_the_runtime_contract() {
    let store = on(Arc::new(MemoryStore::new()));
    let timer = |fire_at| NewTimer {
        machine: "thing".into(),
        key: "\"a\"".into(),
        name: "expire".into(),
        fire_at: Time(fire_at),
        event: "{}".into(),
    };
    let (first, second) = store
        .transact(Isolation::Serializable, |tx| -> Result<_, StorageError> {
            let first = tx.set_timer(&timer(10))?;
            let second = tx.set_timer(&timer(20))?;
            Ok((first, second))
        })
        .unwrap();
    assert_ne!(first, second);
    assert!(store.due_timers("thing", Time(15), 10).unwrap().is_empty());
    let due = store.due_timers("thing", Time(20), 10).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].id, second);
    let taken = store
        .transact(Isolation::Serializable, |tx| -> Result<_, StorageError> {
            Ok((tx.take_timer(second)?, tx.take_timer(second)?))
        })
        .unwrap();
    assert_eq!(taken, (true, false));

    let id = store
        .transact(Isolation::Serializable, |tx| -> Result<_, StorageError> {
            let mut late = message("thing");
            late.not_before = Time(50);
            tx.enqueue(&late)
        })
        .unwrap();
    assert!(store
        .due_messages("thing", Time(49), 10)
        .unwrap()
        .is_empty());
    assert!(store
        .due_messages("other", Time(60), 10)
        .unwrap()
        .is_empty());
    store.defer(id, 1, Time(70), "down").unwrap();
    assert!(store
        .due_messages("thing", Time(60), 10)
        .unwrap()
        .is_empty());
    let due = store.due_messages("thing", Time(70), 10).unwrap();
    assert_eq!(
        (due[0].attempts, due[0].last_error.as_deref()),
        (1, Some("down"))
    );
    assert!(
        store.mark_delivered(id).unwrap(),
        "the first claim removes it"
    );
    assert!(
        !store.mark_delivered(id).unwrap(),
        "the second finds it gone"
    );
    assert_eq!(store.pending().unwrap(), 0);
}

/// A commit to the tenant by another writer between a transaction's read and its commit is a
/// conflict, not a lost update.
#[test]
fn a_concurrent_writer_is_a_conflict() {
    let api: Arc<dyn Store> = Arc::new(MemoryStore::new());
    let (one, two) = (on(api.clone()), on(api));
    one.transact(Isolation::Serializable, |tx| -> Result<(), StorageError> {
        tx.save("rec-thing", "\"a\"", "{}", None).map(|_| ())
    })
    .unwrap();
    let result = one.transact(Isolation::Serializable, |tx| -> Result<(), StorageError> {
        tx.load("rec-thing", "\"a\"")?;
        two.transact(
            Isolation::Serializable,
            |other| -> Result<(), StorageError> {
                other
                    .save("rec-thing", "\"a\"", "{\"other\":1}", Some(1))
                    .map(|_| ())
            },
        )
        .unwrap();
        tx.save("rec-thing", "\"a\"", "{\"mine\":1}", Some(1))
            .map(|_| ())
    });
    assert!(
        matches!(result, Err(StorageError::Conflict(_))),
        "{result:?}"
    );
}

#[tokio::test]
async fn api_memory_quarantines_an_unreadable_entry() {
    system::quarantines_an_unreadable_entry(api_memory()).await;
}

#[tokio::test]
async fn api_sqlite_quarantines_an_unreadable_entry() {
    system::quarantines_an_unreadable_entry(api_sqlite("quarantine")).await;
}

#[tokio::test]
async fn api_memory_two_hosts_dispatch_once() {
    system::two_hosts_dispatch_once(api_memory()).await;
}

#[tokio::test]
async fn api_sqlite_two_hosts_dispatch_once() {
    system::two_hosts_dispatch_once(api_sqlite("two-hosts")).await;
}

#[tokio::test]
async fn api_memory_refuses_an_elapsed_event_from_outside() {
    system::refuses_an_elapsed_event_from_outside(api_memory()).await;
}

#[tokio::test]
async fn api_sqlite_refuses_an_elapsed_event_from_outside() {
    system::refuses_an_elapsed_event_from_outside(api_sqlite("elapsed")).await;
}

#[tokio::test]
async fn api_memory_refuses_a_record_that_breaks_a_constraint() {
    system::refuses_a_record_that_breaks_a_constraint(api_memory()).await;
}

#[tokio::test]
async fn api_sqlite_refuses_a_record_that_breaks_a_constraint() {
    system::refuses_a_record_that_breaks_a_constraint(api_sqlite("constraint")).await;
}

#[tokio::test]
async fn api_memory_lease_quarantines_an_unreadable_timer() {
    lease::quarantines_an_unreadable_timer(api_memory()).await;
}

#[tokio::test]
async fn api_sqlite_lease_quarantines_an_unreadable_timer() {
    lease::quarantines_an_unreadable_timer(api_sqlite("quarantine")).await;
}

#[tokio::test]
async fn api_memory_lease_refuses_an_elapsed_event_from_outside() {
    lease::refuses_an_elapsed_event_from_outside(api_memory()).await;
}

#[tokio::test]
async fn api_sqlite_lease_refuses_an_elapsed_event_from_outside() {
    lease::refuses_an_elapsed_event_from_outside(api_sqlite("elapsed")).await;
}

#[tokio::test]
async fn api_memory_lease_refuses_a_record_that_breaks_a_constraint() {
    lease::refuses_a_record_that_breaks_a_constraint(api_memory()).await;
}

#[tokio::test]
async fn api_sqlite_lease_refuses_a_record_that_breaks_a_constraint() {
    lease::refuses_a_record_that_breaks_a_constraint(api_sqlite("constraint")).await;
}

#[tokio::test]
async fn api_memory_delayed_two_hosts_dispatch_once() {
    delayed::two_hosts_dispatch_once(api_memory()).await;
}

#[tokio::test]
async fn api_sqlite_delayed_two_hosts_dispatch_once() {
    delayed::two_hosts_dispatch_once(api_sqlite("two-hosts-delayed")).await;
}

#[tokio::test]
async fn api_memory_redelivers_at_least_once_after_a_stop_between_dispatch_and_removal() {
    system::redelivers_at_least_once_after_a_stop_between_dispatch_and_removal(api_memory()).await;
}

#[tokio::test]
async fn api_sqlite_redelivers_at_least_once_after_a_stop_between_dispatch_and_removal() {
    system::redelivers_at_least_once_after_a_stop_between_dispatch_and_removal(api_sqlite(
        "redeliver",
    ))
    .await;
}

#[tokio::test]
async fn api_memory_loses_at_most_once_on_a_stop_between_commit_and_dispatch() {
    system::loses_at_most_once_on_a_stop_between_commit_and_dispatch(api_memory()).await;
}

#[tokio::test]
async fn api_sqlite_loses_at_most_once_on_a_stop_between_commit_and_dispatch() {
    system::loses_at_most_once_on_a_stop_between_commit_and_dispatch(api_sqlite("lose")).await;
}

#[tokio::test]
async fn api_memory_retries_at_least_once_with_backoff() {
    system::retries_at_least_once_with_backoff(api_memory()).await;
}

#[tokio::test]
async fn api_sqlite_retries_at_least_once_with_backoff() {
    system::retries_at_least_once_with_backoff(api_sqlite("retry")).await;
}

#[tokio::test]
async fn api_memory_refuses_an_instance_beyond_max() {
    system::refuses_an_instance_beyond_max(api_memory()).await;
}

#[tokio::test]
async fn api_sqlite_refuses_an_instance_beyond_max() {
    system::refuses_an_instance_beyond_max(api_sqlite("max")).await;
}

/// Two tenants on one storage-api store, with machines whose record ids are equal: each tenant's
/// machine store keeps its rows in that tenant, so they hold two instances (WG-4907). The store is
/// built for a tenant, and a machine store of another tenant over the same store sees none of it.
fn equal_ids_are_two_instances(api: Arc<dyn Store>) {
    let for_tenant = |name: &str| {
        StorageApiMachineStore::new(
            api.clone(),
            TenantId::new(name).unwrap(),
            Arc::new(SystemClock),
        )
    };
    let (acme, globex) = (for_tenant("acme"), for_tenant("globex"));
    assert_eq!(acme.tenant().as_str(), "acme");
    let key = "[\"post\",\"p1\"]";
    let save = |store: &StorageApiMachineStore, data: &str| {
        store
            .transact(Isolation::Serializable, |tx| -> Result<_, StorageError> {
                tx.save("rec-content", key, data, None)
            })
            .unwrap();
    };
    let load = |store: &StorageApiMachineStore| {
        store
            .transact(Isolation::Serializable, |tx| -> Result<_, StorageError> {
                tx.load("rec-content", key)
            })
            .unwrap()
    };
    save(&acme, "{\"version\":7}");
    assert!(load(&globex).is_none(), "the other tenant has no instance");
    save(&globex, "{\"version\":1}");
    assert_eq!(load(&acme).unwrap().data, "{\"version\":7}");
    assert_eq!(load(&globex).unwrap().data, "{\"version\":1}");
}

#[test]
fn api_memory_tenants_with_equal_record_ids_share_no_instance_state() {
    equal_ids_are_two_instances(Arc::new(MemoryStore::new()));
}

#[test]
fn api_sqlite_tenants_with_equal_record_ids_share_no_instance_state() {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "machine-store-tenants-{}-{}.db",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    equal_ids_are_two_instances(Arc::new(
        SqliteStore::open(&path, strata_kernel::Timestamp(0)).unwrap(),
    ));
}
