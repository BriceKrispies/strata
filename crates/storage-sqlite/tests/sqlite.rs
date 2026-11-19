//! The SQLite adapter passes the conformance suite, on disk, and keeps its data across a reopen.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_storage_api::{conformance, execute, Account, SessionRecord, Store};
use strata_storage_sqlite::{default_tenants_dir, tenant_file_name, SqliteStore};

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
    let _ = std::fs::remove_dir_all(default_tenants_dir(&path));
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
        store.mark_published(&alpha, first.position).unwrap();
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
    assert_eq!(store.snapshots(&alpha).unwrap(), vec![snapshot]);
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

/// Create a tenant with the notes schema and one note in it.
fn tenant_with_a_note(store: &SqliteStore, tenant: &str, at: i64) {
    let owner = principal_id(&format!("{tenant}:owner"));
    execute(
        store,
        &Intent::CreateTenant {
            name: tenant.into(),
            owner: owner.clone(),
        },
        &ctx(tenant, Principal::System, at),
    )
    .unwrap();
    let schema = Intent::ChangeSchema {
        changes: notes_schema(),
        expected_version: None,
        existing: ExistingData::Reject,
    };
    execute(
        store,
        &schema,
        &ctx(tenant, Principal::User(owner.clone()), at + 1),
    )
    .unwrap();
    execute(
        store,
        &new_note(&format!("Note of {tenant}"), "note"),
        &ctx(tenant, Principal::User(owner), at + 2),
    )
    .unwrap();
}

/// Count the rows of a database file that a query counts, read with a connection of its own.
fn count(file: &std::path::Path, sql: &str) -> i64 {
    let conn = rusqlite::Connection::open(file).unwrap();
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// Verifies: TEN-001
///
/// Each tenant's data lives in a database file of its own beside the control database, which
/// holds the tenant directory and no tenant content: alpha's file holds alpha's header, records,
/// events, journal, sessions and logins and nothing of beta, and the other way round.
#[test]
fn two_tenants_data_live_in_different_files() {
    let path = fresh_path("files");
    let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
    tenant_with_a_note(&store, "alpha", 10);
    tenant_with_a_note(&store, "beta", 20);
    for tenant in ["alpha", "beta"] {
        let id = tenant_id(tenant);
        store
            .put_account(&Account {
                tenant: id.clone(),
                login: "owner".into(),
                principal: principal_id(&format!("{tenant}:owner")),
                display_name: "Owner".into(),
                password_hash: "$argon2id$placeholder".into(),
            })
            .unwrap();
        store
            .put_session(&SessionRecord {
                id: SessionId::new(format!("s-{tenant}")).unwrap(),
                token_hash: format!("hash-of-{tenant}"),
                csrf_token: format!("csrf-{tenant}"),
                tenant: id,
                principal: principal_id(&format!("{tenant}:owner")),
                display_name: "Owner".into(),
                created_at: Timestamp(10),
                last_seen_at: Timestamp(10),
                expires_at: Timestamp(1_000_000),
                revoked_at: None,
            })
            .unwrap();
    }

    let dir = default_tenants_dir(&path);
    assert_eq!(store.tenants_dir(), Some(dir.as_path()));
    let alpha = dir.join(tenant_file_name(&tenant_id("alpha")));
    let beta = dir.join(tenant_file_name(&tenant_id("beta")));
    assert_ne!(alpha, beta);
    assert_eq!(
        store.tenant_file(&tenant_id("alpha")).unwrap(),
        Some(alpha.clone())
    );
    assert_eq!(
        store.tenant_file(&tenant_id("beta")).unwrap(),
        Some(beta.clone())
    );
    for (file, mine, other) in [(&alpha, "alpha", "beta"), (&beta, "beta", "alpha")] {
        assert!(file.exists(), "{} exists", file.display());
        let mine_count = |sql: &str| count(file, &sql.replace("{mine}", mine));
        assert_eq!(
            mine_count("SELECT COUNT(*) FROM tenant WHERE id = '{mine}'"),
            1
        );
        assert_eq!(
            count(file, "SELECT COUNT(*) FROM tenant"),
            1,
            "{mine}'s file holds one tenant"
        );
        assert_eq!(count(file, "SELECT COUNT(*) FROM records"), 1);
        assert_eq!(
            mine_count("SELECT COUNT(*) FROM records WHERE data LIKE '%Note of {mine}%'"),
            1
        );
        assert_eq!(
            count(
                file,
                &format!("SELECT COUNT(*) FROM records WHERE data LIKE '%Note of {other}%'")
            ),
            0,
            "{mine}'s file holds nothing of {other}"
        );
        assert_eq!(count(file, "SELECT COUNT(*) FROM journal"), 3);
        assert!(count(file, "SELECT COUNT(*) FROM events") >= 3);
        assert_eq!(
            mine_count("SELECT COUNT(*) FROM sessions WHERE id = 's-{mine}'"),
            1
        );
        assert_eq!(count(file, "SELECT COUNT(*) FROM sessions"), 1);
        assert_eq!(count(file, "SELECT COUNT(*) FROM accounts"), 1);
    }
    assert_eq!(
        count(
            &path,
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' \
             AND name IN ('records', 'events', 'journal', 'sessions', 'accounts', 'tenant')"
        ),
        0,
        "the control database holds no tenant content"
    );
    assert_eq!(
        count(
            &path,
            "SELECT COUNT(*) FROM tenant_directory WHERE state = 'active'"
        ),
        2
    );
    // The sessions are found through the control database's session directory.
    assert_eq!(
        store
            .session_by_token("hash-of-beta")
            .unwrap()
            .unwrap()
            .tenant,
        tenant_id("beta")
    );
}

/// Verifies: TEN-001
///
/// Deleting a tenant removes its file and its directory row and leaves the other tenant's file
/// as it was; a reset removes every tenant file. The store stays usable throughout.
#[test]
fn deleting_a_tenant_removes_its_file() {
    let path = fresh_path("delete");
    let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
    tenant_with_a_note(&store, "alpha", 10);
    tenant_with_a_note(&store, "beta", 20);
    let alpha = store.tenant_file(&tenant_id("alpha")).unwrap().unwrap();
    let beta = store.tenant_file(&tenant_id("beta")).unwrap().unwrap();
    let beta_before = canon::hash_of(&store.load_complete(&tenant_id("beta")).unwrap().tenant);

    store.reset_tenant(&tenant_id("alpha")).unwrap();
    assert!(!alpha.exists(), "alpha's file is gone");
    for suffix in ["-wal", "-shm"] {
        assert!(!PathBuf::from(format!("{}{suffix}", alpha.display())).exists());
    }
    assert!(beta.exists());
    assert_eq!(store.tenant_file(&tenant_id("alpha")).unwrap(), None);
    assert_eq!(count(&path, "SELECT COUNT(*) FROM tenant_directory"), 1);
    assert_eq!(
        canon::hash_of(&store.load_complete(&tenant_id("beta")).unwrap().tenant),
        beta_before
    );
    assert!(store
        .load(&tenant_id("alpha"), &[])
        .unwrap()
        .tenant
        .is_none());

    // The tenant can be created again, in a new file.
    tenant_with_a_note(&store, "alpha", 30);
    assert!(alpha.exists());

    store.reset().unwrap();
    assert!(
        !alpha.exists() && !beta.exists(),
        "a reset removes every tenant file"
    );
    assert_eq!(count(&path, "SELECT COUNT(*) FROM tenant_directory"), 0);
    assert!(store.tenants().unwrap().is_empty());
}

/// A file left in the tenants directory without a directory row (the control database was
/// deleted, the tenant files were not) is stale: creating that tenant starts from an empty file.
#[test]
fn a_tenant_file_without_a_directory_row_is_not_reused() {
    let path = fresh_path("stale");
    {
        let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
        tenant_with_a_note(&store, "alpha", 10);
    }
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    let store = SqliteStore::open(&path, Timestamp(2)).unwrap();
    assert!(store.tenants().unwrap().is_empty());
    tenant_with_a_note(&store, "alpha", 20);
    assert_eq!(
        store
            .load_complete(&tenant_id("alpha"))
            .unwrap()
            .tenant
            .unwrap()
            .record_count(),
        1
    );
}

/// The old single-file layout is refused with a message that says what to do.
#[test]
fn the_old_single_file_layout_is_refused() {
    let path = fresh_path("old-layout");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE storage_migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at INTEGER NOT NULL);
             INSERT INTO storage_migrations VALUES (1, 'initial layout', 1);
             CREATE TABLE tenants (id TEXT PRIMARY KEY, name TEXT NOT NULL, revision INTEGER NOT NULL,
                 next_record INTEGER NOT NULL, event_seq INTEGER NOT NULL, schema TEXT NOT NULL, members TEXT NOT NULL);",
        )
        .unwrap();
    }
    let Err(error) = SqliteStore::open(&path, Timestamp(1)) else {
        panic!("the old layout was opened");
    };
    let text = error.to_string();
    assert!(text.contains("old single-file layout"), "{text}");
    assert!(text.contains("delete"), "{text}");
}

/// The in-memory store keeps one database per tenant too, and passes the conformance suite.
#[test]
fn the_in_memory_store_conforms() {
    conformance::run(&|| {
        Box::new(SqliteStore::open_in_memory(Timestamp(1)).unwrap()) as Box<dyn Store>
    });
}
