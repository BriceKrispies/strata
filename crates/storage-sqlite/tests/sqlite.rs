//! The SQLite adapter passes the conformance suite, on disk, and keeps its data across a reopen.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_storage_api::{conformance, execute, idempotency_scope, Account, SessionRecord, Store};
use strata_storage_sqlite::{default_tenants_dir, tenant_migration_steps, SqliteStore};

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
    let spec = strata_spec_parts::read_spec_text(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../specifications/spec.jsonc"
    )))
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

/// Verifies: OPS-318
///
/// A retried create after the process restarted: the key was recorded in the tenant's file by the
/// commit, so the retry on a reopened store receives the first result and creates nothing.
#[test]
fn an_idempotency_key_survives_a_restart() {
    let path = fresh_path("idempotent");
    let alpha = tenant_id("alpha");
    let owner = principal_id("alpha:owner");
    let keyed = |now: i64| {
        let mut context = ctx("alpha", Principal::User(owner.clone()), now);
        context.idempotency_key = Some("create-1".into());
        context
    };
    let first = {
        let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
        execute(
            &store,
            &Intent::CreateTenant {
                name: "alpha".into(),
                owner: owner.clone(),
            },
            &ctx("alpha", Principal::System, 10),
        )
        .unwrap();
        execute(
            &store,
            &Intent::ChangeSchema {
                changes: notes_schema(),
                expected_version: None,
                existing: ExistingData::Reject,
            },
            &ctx("alpha", Principal::User(owner.clone()), 11),
        )
        .unwrap();
        execute(&store, &new_note("Once", "once"), &keyed(12)).unwrap()
    };
    assert!(!first.replayed);
    let store = SqliteStore::open(&path, Timestamp(2)).unwrap();
    let again = execute(&store, &new_note("Once", "once"), &keyed(13)).unwrap();
    assert!(again.replayed);
    assert_eq!(again.receipt, first.receipt);
    assert_eq!(again.transition.output, first.transition.output);
    let state = store.load_complete(&alpha).unwrap();
    let records: usize = state
        .tenant
        .unwrap()
        .records
        .values()
        .map(|by_id| by_id.len())
        .sum();
    assert_eq!(records, 1, "the retried create made a second record");
    assert!(store
        .idempotent_result(
            &alpha,
            &idempotency_scope(&Principal::User(owner)),
            "create-1",
            Timestamp(14)
        )
        .unwrap()
        .is_some());
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
        authenticated_at: Timestamp(10),
        network: None,
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
    assert_eq!(
        store
            .verified_address(&alpha, &principal_id("alpha:owner"))
            .unwrap(),
        Some("owner@example.org".to_string())
    );
    assert_eq!(store.snapshots(&alpha).unwrap(), vec![snapshot]);
    assert_eq!(store.journal_after(&alpha, 0, 10).unwrap().len(), 3);
    let migrations = store.migrations().unwrap();
    assert_eq!(migrations.len(), 5, "the control database's steps");
    assert_eq!(
        migrations[0].applied_at,
        Timestamp(1),
        "the layout step is recorded once, not on every open"
    );
    let file = store.tenant_file(&alpha).unwrap().unwrap();
    assert_eq!(
        count(&file, "SELECT COUNT(*) FROM tenant_migrations"),
        tenant_migration_steps().len() as i64,
        "the tenant file's steps"
    );
    assert!(store.health().unwrap().ok);
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

fn live_session(tenant: &str) -> SessionRecord {
    SessionRecord {
        id: SessionId::new(format!("s-{tenant}")).unwrap(),
        token_hash: format!("hash-of-{tenant}"),
        csrf_token: format!("csrf-{tenant}"),
        tenant: tenant_id(tenant),
        principal: principal_id(&format!("{tenant}:owner")),
        display_name: "Owner".into(),
        created_at: Timestamp(10),
        last_seen_at: Timestamp(10),
        expires_at: Timestamp(1_000_000),
        revoked_at: None,
        authenticated_at: Timestamp(10),
        network: None,
    }
}

fn tenants_of(entries: &[strata_storage_api::OutboxEntry], tenant: &str) -> usize {
    entries
        .iter()
        .filter(|e| e.event.tenant.as_str() == tenant)
        .count()
}

/// Verifies: TEN-007
///
/// A tenant file that cannot be opened does not stop the other tenants: their events are still in
/// the outbox and their sessions are still listed, the broken tenant is reported, liveness
/// answers and the integrity check says the store is not well instead of failing.
#[test]
fn a_corrupt_tenant_file_does_not_stop_the_others() {
    let path = fresh_path("corrupt");
    let (beta_file, alpha_events) = {
        let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
        tenant_with_a_note(&store, "alpha", 10);
        tenant_with_a_note(&store, "beta", 20);
        store.put_session(&live_session("alpha")).unwrap();
        store.put_session(&live_session("beta")).unwrap();
        let alpha_events = tenants_of(&store.outbox(100).unwrap(), "alpha");
        (
            store.tenant_file(&tenant_id("beta")).unwrap().unwrap(),
            alpha_events,
        )
    };
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", beta_file.display()));
    }
    std::fs::write(&beta_file, vec![b'x'; 4096]).unwrap();

    let store = SqliteStore::open(&path, Timestamp(2)).unwrap();
    let out = store.outbox(100).unwrap();
    assert!(alpha_events >= 3);
    assert_eq!(
        tenants_of(&out, "alpha"),
        alpha_events,
        "alpha's events are offered"
    );
    assert_eq!(tenants_of(&out, "beta"), 0);
    let faults = store.outbox_faults();
    assert!(
        faults
            .iter()
            .any(|f| f.tenant == tenant_id("beta") && f.position.is_none()),
        "the broken tenant is reported: {faults:?}"
    );
    let live = store.live_sessions(Timestamp(100)).unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].tenant, tenant_id("alpha"));
    assert_eq!(
        store
            .live_sessions_of(
                &tenant_id("alpha"),
                Some(&principal_id("alpha:owner")),
                Timestamp(100)
            )
            .unwrap()
            .len(),
        1
    );
    assert!(store
        .live_sessions_of(&tenant_id("beta"), None, Timestamp(100))
        .is_err());
    assert_eq!(store.tenants().unwrap().len(), 1);
    store.liveness().unwrap();
    assert!(!store.health().unwrap().ok);
}

/// Verifies: OPS-348
///
/// An event row that cannot be decoded is parked and reported, and the events after it in the same
/// tenant and the events of other tenants are still offered.
#[test]
fn an_undecodable_event_is_parked_and_the_rest_is_published() {
    let path = fresh_path("poison");
    let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
    tenant_with_a_note(&store, "alpha", 10);
    tenant_with_a_note(&store, "beta", 20);
    let before = store.outbox(100).unwrap();
    let (alpha_events, beta_events) = (tenants_of(&before, "alpha"), tenants_of(&before, "beta"));
    let alpha_file = store.tenant_file(&tenant_id("alpha")).unwrap().unwrap();
    let poisoned = {
        let conn = rusqlite::Connection::open(&alpha_file).unwrap();
        let position: i64 = conn
            .query_row(
                "SELECT position FROM events WHERE published = 0 ORDER BY position LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "UPDATE events SET data = '{not json' WHERE position = ?1",
            [position],
        )
        .unwrap();
        position as u64
    };
    let out = store.outbox(100).unwrap();
    assert_eq!(
        tenants_of(&out, "alpha"),
        alpha_events - 1,
        "alpha's later events come through"
    );
    assert_eq!(tenants_of(&out, "beta"), beta_events);
    assert!(store
        .outbox_faults()
        .iter()
        .any(|f| f.tenant == tenant_id("alpha") && f.position == Some(poisoned)));
    for entry in &out {
        store
            .mark_published(&entry.event.tenant, entry.position)
            .unwrap();
    }
    assert!(
        store.outbox(100).unwrap().is_empty(),
        "the parked event is not offered again"
    );
    assert_eq!(store.outbox_pending_of(&tenant_id("alpha")).unwrap(), 0);
}

/// Verifies: STO-006
///
/// Open tenant files are bounded: a store that touches more tenants than the bound closes the
/// least recently used, and every tenant still answers.
#[test]
fn open_tenant_files_are_bounded() {
    let path = fresh_path("bounded");
    let per_tenant = {
        let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
        for (i, name) in ["alpha", "beta", "gamma", "delta"].iter().enumerate() {
            tenant_with_a_note(&store, name, 10 + 10 * i as i64);
        }
        tenants_of(&store.outbox(1_000).unwrap(), "alpha")
    };
    let store = SqliteStore::open(&path, Timestamp(2))
        .unwrap()
        .with_max_open(2);
    let out = store.outbox(100).unwrap();
    for name in ["alpha", "beta", "gamma", "delta"] {
        assert_eq!(tenants_of(&out, name), per_tenant, "{name} answers");
    }
    assert!(store.open_count() <= 2, "{} files open", store.open_count());
    for name in ["alpha", "delta", "beta"] {
        assert!(store.load(&tenant_id(name), &[]).unwrap().tenant.is_some());
        assert!(store.open_count() <= 2);
    }
}

/// Verifies: fm-provisioning-step-fails
///
/// An install announced before its creating commit is `provisioning` from that commit on: a
/// process that stops before finishing leaves a tenant the next start rolls back to absent, and
/// an install that finishes is `active` after a reopen.
#[test]
fn an_unfinished_install_is_rolled_back_after_a_restart_and_a_finished_one_stays() {
    let path = fresh_path("provisioning");
    let half = tenant_id("half");
    let whole = tenant_id("whole");
    let create = |store: &SqliteStore, tenant: &TenantId| {
        store.begin_provisioning(tenant).unwrap();
        execute(
            store,
            &Intent::CreateTenant {
                name: tenant.as_str().into(),
                owner: principal_id("owner"),
            },
            &ctx(tenant.as_str(), Principal::System, 10),
        )
        .unwrap();
    };
    {
        let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
        create(&store, &half);
        create(&store, &whole);
        store.finish_provisioning(&whole).unwrap();
        // The process stops here: `half` is never finished.
    }
    let store = SqliteStore::open(&path, Timestamp(2)).unwrap();
    assert_eq!(store.rollback_provisioning().unwrap(), vec![half.clone()]);
    assert!(store.load_complete(&half).unwrap().tenant.is_none());
    assert!(store.load_complete(&whole).unwrap().tenant.is_some());
    assert!(store.rollback_provisioning().unwrap().is_empty());
    drop(store);
    let store = SqliteStore::open(&path, Timestamp(3)).unwrap();
    assert!(store.load_complete(&half).unwrap().tenant.is_none());
    assert!(store.load_complete(&whole).unwrap().tenant.is_some());
}
