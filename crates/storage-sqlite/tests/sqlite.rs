//! The SQLite adapter passes the conformance suite, on disk, and keeps its data across a reopen.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_storage_api::{conformance, execute, Store};
use strata_storage_sqlite::SqliteStore;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A fresh database file under the build directory.
fn fresh_path(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let path = dir.join(format!(
        "{name}-{}-{}.db",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}

/// Verifies: STO-002
#[test]
fn sqlite_store_conforms_on_disk() {
    conformance::run(&|| {
        Box::new(SqliteStore::open(&fresh_path("conformance"), Timestamp(1)).unwrap())
            as Box<dyn Store>
    });
}

/// Verifies: STO-003
#[test]
fn committed_data_survives_a_reopen() {
    let path = fresh_path("durable");
    let alpha = tenant_id("alpha");
    let hash_before = {
        let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
        assert!(store.capabilities().durable);
        let owner = principal_id("alpha:owner");
        execute(
            &store,
            &Intent::CreateTenant {
                name: "alpha".into(),
                owner: owner.clone(),
            },
            &ctx("alpha", Principal::System, 10),
        )
        .unwrap();
        let schema = Intent::ChangeSchema {
            changes: notes_schema(),
            expected_version: None,
            existing: ExistingData::Reject,
        };
        execute(
            &store,
            &schema,
            &ctx("alpha", Principal::User(owner.clone()), 11),
        )
        .unwrap();
        execute(
            &store,
            &new_note("Kept", "kept"),
            &ctx("alpha", Principal::User(owner), 12),
        )
        .unwrap();
        canon::hash_of(&store.load_complete(&alpha).unwrap().tenant)
    };
    let store = SqliteStore::open(&path, Timestamp(2)).unwrap();
    assert_eq!(
        canon::hash_of(&store.load_complete(&alpha).unwrap().tenant),
        hash_before
    );
    assert_eq!(
        store.outbox(10).unwrap().len(),
        3,
        "unpublished events are still waiting after a restart"
    );
    assert_eq!(store.journal_after(&alpha, 0, 10).unwrap().len(), 3);
    let migrations = store.migrations().unwrap();
    assert_eq!(migrations.len(), 1);
    assert_eq!(
        migrations[0].applied_at,
        Timestamp(1),
        "the layout step is recorded once, not on every open"
    );
    assert!(store.health().unwrap().ok);
}

#[test]
fn a_second_connection_sees_only_whole_commits() {
    let path = fresh_path("shared");
    let writer = SqliteStore::open(&path, Timestamp(1)).unwrap();
    let reader = SqliteStore::open(&path, Timestamp(1)).unwrap();
    let owner = principal_id("alpha:owner");
    execute(
        &writer,
        &Intent::CreateTenant {
            name: "alpha".into(),
            owner: owner.clone(),
        },
        &ctx("alpha", Principal::System, 10),
    )
    .unwrap();
    let seen = reader
        .load_complete(&tenant_id("alpha"))
        .unwrap()
        .tenant
        .expect("committed tenant is visible");
    assert_eq!(seen.revision, StateRevision(1));
    assert_eq!(
        reader
            .events_after(&tenant_id("alpha"), 0, 10)
            .unwrap()
            .len(),
        1
    );
}
