//! The SQLite adapter passes the conformance suite, on disk, and keeps its data across a reopen.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_storage_api::{conformance, execute, Account, SessionRecord, Store};
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
///
/// Every row of the table in the specification has a check and every check has a row, and each
/// check runs on a database file.
#[test]
fn sqlite_store_conforms_on_disk() {
    let ran = conformance::run(&|| {
        Box::new(SqliteStore::open(&fresh_path("conformance"), Timestamp(1)).unwrap())
            as Box<dyn Store>
    });
    let spec = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../specifications/spec.jsonc"
    ))
    .expect("the specification file");
    let (mut ran, mut rows) = (ran, conformance::spec_cases(&spec));
    assert!(rows.len() >= 13, "the table was not read: {rows:?}");
    ran.sort();
    rows.sort();
    assert_eq!(
        ran, rows,
        "the checks that ran are not the rows of the conformance table"
    );
}

/// Verifies: STO-003
///
/// Everything the adapter acknowledges is there after the file is closed and opened again: the
/// tenant state, its events and journal, which events were marked published, a session, a login
/// and a snapshot.
#[test]
fn committed_data_survives_a_reopen() {
    let path = fresh_path("durable");
    let alpha = tenant_id("alpha");
    let session = SessionRecord {
        id: SessionId::new("s-durable").unwrap(),
        token_hash: "hash-of-s-durable".into(),
        csrf_token: "csrf-s-durable".into(),
        tenant: alpha.clone(),
        principal: principal_id("alpha:owner"),
        display_name: "Owner".into(),
        created_at: Timestamp(10),
        last_seen_at: Timestamp(11),
        expires_at: Timestamp(1_000_000),
        revoked_at: None,
    };
    let account = Account {
        tenant: alpha.clone(),
        login: "owner".into(),
        principal: principal_id("alpha:owner"),
        display_name: "Owner".into(),
        password_hash: "$argon2id$placeholder".into(),
        verified_email: Some("owner@example.org".into()),
    };
    let (hash_before, snapshot) = {
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
        store.put_session(&session).unwrap();
        store.put_account(&account).unwrap();
        let snapshot = store
            .create_snapshot(&alpha, "kept", Timestamp(13))
            .unwrap();
        let first = store.outbox(1).unwrap().remove(0);
        store.mark_published(first.position).unwrap();
        (
            canon::hash_of(&store.load_complete(&alpha).unwrap().tenant),
            snapshot,
        )
    };
    let store = SqliteStore::open(&path, Timestamp(2)).unwrap();
    assert_eq!(
        canon::hash_of(&store.load_complete(&alpha).unwrap().tenant),
        hash_before
    );
    assert_eq!(
        store.outbox(10).unwrap().len(),
        2,
        "unpublished events are still waiting after a restart, and the published one is not"
    );
    assert_eq!(
        store.session_by_token(&session.token_hash).unwrap(),
        Some(session.clone())
    );
    assert_eq!(store.session(&session.id).unwrap(), Some(session));
    assert_eq!(store.account(&alpha, "owner").unwrap(), Some(account));
    assert_eq!(
        store
            .verified_address(&alpha, &principal_id("alpha:owner"))
            .unwrap(),
        Some("owner@example.org".to_string())
    );
    assert_eq!(store.snapshots(&alpha).unwrap(), vec![snapshot]);
    assert_eq!(store.journal_after(&alpha, 0, 10).unwrap().len(), 3);
    let migrations = store.migrations().unwrap();
    assert_eq!(migrations.len(), 2);
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

/// Verifies: H-008
///
/// A value of a query is bound as data: a login that is SQL text is looked up as that text, finds
/// only the account stored under it and damages nothing. A file that cannot be opened is
/// unavailable.
#[test]
fn query_values_are_bound_as_data_and_an_unopenable_file_is_unavailable() {
    let store = SqliteStore::open(&fresh_path("bound"), Timestamp(1)).unwrap();
    let alpha = tenant_id("alpha");
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
    let account = |login: &str| Account {
        tenant: alpha.clone(),
        login: login.into(),
        principal: owner.clone(),
        display_name: "Owner".into(),
        password_hash: "$argon2id$placeholder".into(),
        verified_email: None,
    };
    store.put_account(&account("owner")).unwrap();
    let hostile = "x' OR '1'='1'; DROP TABLE accounts; --";
    assert_eq!(
        store.account(&alpha, hostile).unwrap(),
        None,
        "SQL text matches nothing"
    );
    store.put_account(&account(hostile)).unwrap();
    assert_eq!(
        store.account(&alpha, hostile).unwrap(),
        Some(account(hostile)),
        "it is stored and found as the text it is"
    );
    assert_eq!(
        store.account(&alpha, "owner").unwrap(),
        Some(account("owner")),
        "the table is intact"
    );

    let missing = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("no-such-directory")
        .join("store.db");
    assert!(matches!(
        SqliteStore::open(&missing, Timestamp(1)),
        Err(strata_storage_api::StoreError::Unavailable(_))
    ));
}
