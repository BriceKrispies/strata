//! Tenant scope of the machine stores (WG-4907): a store belongs to one tenant, given at
//! construction, so two tenants whose machines have equal record ids hold two instances, never
//! one. The same case runs on every store the runtime ships.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use strata_runtime::store::{RecordStore, StorageError, StoredRecord};
use strata_runtime::{Database, Isolation, MemoryStore, SqliteMachineStore, TenantId, Time};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn tenant(name: &str) -> TenantId {
    TenantId::new(name).unwrap()
}

fn fresh_path(name: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "runtime-tenants-{name}-{}-{}.db",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}

fn save<S: RecordStore>(store: &S, collection: &str, key: &str, data: &str) {
    store
        .transact(Isolation::Serializable, |tx| -> Result<_, StorageError> {
            tx.save(collection, key, data, None)
        })
        .unwrap();
}

fn load<S: RecordStore>(store: &S, collection: &str, key: &str) -> Option<StoredRecord> {
    store
        .transact(Isolation::Serializable, |tx| -> Result<_, StorageError> {
            tx.load(collection, key)
        })
        .unwrap()
}

fn keys<S: RecordStore>(store: &S, collection: &str) -> Vec<String> {
    store
        .transact(Isolation::Serializable, |tx| -> Result<_, StorageError> {
            tx.keys(collection)
        })
        .unwrap()
}

/// Two tenants, one record id: each writes its own state under it, and neither sees the other's.
fn equal_ids_are_two_instances<S: RecordStore>(acme: &S, globex: &S) {
    assert_eq!(acme.tenant(), &tenant("acme"));
    assert_eq!(globex.tenant(), &tenant("globex"));
    let key = "[\"post\",\"p1\"]";
    save(acme, "rec-content", key, "{\"version\":7}");
    assert!(
        load(globex, "rec-content", key).is_none(),
        "the other tenant has no instance under the same key"
    );
    assert!(keys(globex, "rec-content").is_empty());
    save(globex, "rec-content", key, "{\"version\":1}");
    assert_eq!(
        load(acme, "rec-content", key).unwrap().data,
        "{\"version\":7}"
    );
    assert_eq!(
        load(globex, "rec-content", key).unwrap().data,
        "{\"version\":1}"
    );
    assert_eq!(load(acme, "rec-content", key).unwrap().version, 1);
    assert_eq!(keys(acme, "rec-content"), vec![key.to_string()]);
}

#[test]
fn stores_of_two_tenants_share_no_instance_state() {
    equal_ids_are_two_instances(
        &MemoryStore::new(tenant("acme")),
        &MemoryStore::new(tenant("globex")),
    );
    let open = |name: &str, who: &str| {
        SqliteMachineStore::open(
            tenant(who),
            Database::open(&fresh_path(name)).unwrap(),
            Time(0),
        )
        .unwrap()
    };
    equal_ids_are_two_instances(&open("acme", "acme"), &open("globex", "globex"));
}

#[test]
fn one_sqlite_file_cannot_hold_the_machines_of_two_tenants() {
    let path = fresh_path("one-file");
    SqliteMachineStore::open(tenant("acme"), Database::open(&path).unwrap(), Time(0)).unwrap();
    // The same tenant reopens it.
    SqliteMachineStore::open(tenant("acme"), Database::open(&path).unwrap(), Time(0)).unwrap();
    let refused =
        SqliteMachineStore::open(tenant("globex"), Database::open(&path).unwrap(), Time(0))
            .expect_err("a second tenant on the file of the first");
    assert!(
        matches!(&refused, StorageError::Invalid(why) if why.contains("acme") && why.contains("globex")),
        "{refused:?}"
    );
}
