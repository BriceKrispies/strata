//! The SQLite adapter passes the conformance suite, on disk, and keeps its data across a reopen.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_storage_api::{
    conformance, execute, idempotency_scope, Account, SeededNonces, SessionRecord, Store,
    StoreError,
};
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

/// The tenant generation of the contract holds on SQLite: it follows what a load returns for
/// every method of the trait, a commit is accepted only at the generation its decision was made
/// on, a load's generation is the generation of its rows under concurrent commits, and a seeded
/// store is reproducible.
// Verifies: STO-001
#[test]
fn sqlite_keeps_the_tenant_generation() {
    let fresh = || Box::new(SqliteStore::open_in_memory(Timestamp(1)).unwrap()) as Box<dyn Store>;
    conformance::generation_follows_every_store_method(&fresh);
    conformance::a_commit_names_the_generation_it_was_decided_on(fresh().as_ref());
    conformance::interleaved_commits_and_loads_keep_each_revision_with_one_generation(
        fresh().as_ref(),
    );
    conformance::generations_are_reproducible_from_the_seed(&|seed| {
        Box::new(
            SqliteStore::open_in_memory(Timestamp(1))
                .unwrap()
                .with_nonces(SeededNonces::shared(seed)),
        ) as Box<dyn Store>
    });
}

/// A handle cycle (the open file is closed to make room and opened again) leaves the generation
/// as it was: opening draws no nonce and raises no counter.
// Verifies: STO-001
#[test]
fn a_handle_cycle_leaves_the_generation_unchanged() {
    let path = fresh_path("handle-cycle");
    let (alpha, beta) = (tenant_id("alpha"), tenant_id("beta"));
    let (held_alpha, held_beta) = {
        let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
        tenant_with_a_note(&store, "alpha", 10);
        tenant_with_a_note(&store, "beta", 20);
        (
            store.generation(&alpha).unwrap().expect("alpha has one"),
            store.generation(&beta).unwrap().expect("beta has one"),
        )
    };
    assert_ne!(
        held_alpha.nonce, held_beta.nonce,
        "each tenant has a nonce of its own"
    );
    // A store that keeps one file open: each tenant read closes the other's file, which the next
    // call opens again.
    let store = SqliteStore::open(&path, Timestamp(2))
        .unwrap()
        .with_max_open(1);
    for _ in 0..3 {
        assert_eq!(store.generation(&alpha).unwrap(), Some(held_alpha));
        assert_eq!(store.open_count(), 1);
        assert_eq!(
            store.load_at(&beta, &[]).unwrap().generation,
            Some(held_beta)
        );
        assert_eq!(store.open_count(), 1, "alpha's file was closed for beta's");
    }
    // A commit after the cycles raises the counter by one, under the nonce the tenant had.
    let executed = execute(
        &store,
        &new_note("After", "after"),
        &ctx("alpha", Principal::User(principal_id("alpha:owner")), 100),
    )
    .unwrap();
    assert_eq!(executed.receipt.generation, held_alpha.raised());
}

/// A second store over the same files commits to the tenant between a load and the commit decided
/// on it: the commit is a conflict that carries the newer generation, and nothing changes.
// Verifies: STO-001
#[test]
fn a_second_handle_that_commits_moves_the_generation() {
    let path = fresh_path("second-handle");
    let first = SqliteStore::open(&path, Timestamp(1)).unwrap();
    tenant_with_a_note(&first, "alpha", 10);
    let alpha = tenant_id("alpha");
    let intent = new_note("Decided", "decided");
    let schema = first
        .load(&alpha, &[])
        .unwrap()
        .tenant
        .unwrap()
        .schema
        .clone();
    let loaded = first
        .load_at(&alpha, &needs(Some(&schema), &intent))
        .unwrap();
    let context = ctx("alpha", Principal::User(principal_id("alpha:owner")), 100);
    let decided = transition(&loaded.state, &intent, &context).unwrap();

    let second = SqliteStore::open(&path, Timestamp(2)).unwrap();
    let foreign = execute(
        &second,
        &new_note("Foreign", "foreign"),
        &ctx("alpha", Principal::User(principal_id("alpha:owner")), 101),
    )
    .unwrap();
    let newer = foreign.receipt.generation;
    assert_eq!(first.generation(&alpha).unwrap(), Some(newer));
    let revision = first.load(&alpha, &[]).unwrap().tenant.unwrap().revision;

    let error = first
        .commit(&strata_storage_api::Commit::of(
            &intent,
            &context,
            &decided,
            loaded.generation,
        ))
        .unwrap_err();
    assert!(
        matches!(&error, StoreError::Conflict { found_generation, .. } if *found_generation == Some(newer)),
        "{error:?}"
    );
    assert_eq!(first.generation(&alpha).unwrap(), Some(newer));
    assert_eq!(
        first.load(&alpha, &[]).unwrap().tenant.unwrap().revision,
        revision
    );
}

/// A file put back from an older copy while the store was closed has a generation the store held
/// before; the control plane's step after the restore (rotating the nonce) gives one it never
/// held.
// Verifies: STO-001
#[test]
fn an_out_of_band_restore_followed_by_the_restore_step_gives_a_new_generation() {
    let path = fresh_path("out-of-band");
    let alpha = tenant_id("alpha");
    let mut observed = Vec::new();
    let (file, backup) = {
        let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
        tenant_with_a_note(&store, "alpha", 10);
        observed.push(store.generation(&alpha).unwrap().unwrap());
        let file = store.tenant_file(&alpha).unwrap().expect("a file on disk");
        let backup = file.with_extension("backup");
        (file, backup)
    };
    std::fs::copy(&file, &backup).unwrap();
    {
        let store = SqliteStore::open(&path, Timestamp(2)).unwrap();
        execute(
            &store,
            &new_note("Later", "later"),
            &ctx("alpha", Principal::User(principal_id("alpha:owner")), 100),
        )
        .unwrap();
        observed.push(store.generation(&alpha).unwrap().unwrap());
    }
    // The older copy goes back in place of the file, as a restore from outside the cell does.
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", file.display()));
    }
    std::fs::copy(&backup, &file).unwrap();

    let store = SqliteStore::open(&path, Timestamp(3)).unwrap();
    let restored = store.generation(&alpha).unwrap().unwrap();
    assert_eq!(
        restored, observed[0],
        "without the control plane's step the old copy shows an old generation"
    );
    let stepped = store.rotate_nonce(&alpha).unwrap();
    assert!(
        !observed.contains(&stepped),
        "the restore step yields a generation never observed before: {stepped}"
    );
    assert_eq!(store.generation(&alpha).unwrap(), Some(stepped));
    assert_eq!(
        store.load_at(&alpha, &[]).unwrap().generation,
        Some(stepped)
    );
}

/// The generation is one small row: it is read from a tenant whose schema document cannot be read.
// Verifies: STO-001
#[test]
fn the_generation_is_read_from_a_tenant_with_an_unreadable_schema() {
    let path = fresh_path("unreadable");
    let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
    tenant_with_a_note(&store, "alpha", 10);
    let alpha = tenant_id("alpha");
    let held = store.generation(&alpha).unwrap().unwrap();
    let file = store.tenant_file(&alpha).unwrap().expect("a file on disk");
    rusqlite::Connection::open(&file)
        .unwrap()
        .execute("UPDATE tenant SET schema = 'not json'", [])
        .unwrap();
    assert!(
        store.load(&alpha, &[]).is_err(),
        "the schema document is unreadable"
    );
    assert_eq!(store.generation(&alpha).unwrap(), Some(held));
}

/// A tenant file made before the generation row existed is given one by the step that adds the
/// table: a nonce of its own and counter 1, in a file that holds a tenant.
// Verifies: STO-001
#[test]
fn a_tenant_file_from_before_the_generation_gets_one() {
    let path = fresh_path("migrated");
    let alpha = tenant_id("alpha");
    let file = {
        let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
        tenant_with_a_note(&store, "alpha", 10);
        store.tenant_file(&alpha).unwrap().expect("a file on disk")
    };
    // Put the file back at the level before the generation step.
    {
        let conn = rusqlite::Connection::open(&file).unwrap();
        conn.execute_batch(
            "DROP TABLE generation;
             DELETE FROM tenant_migrations WHERE version = 24;",
        )
        .unwrap();
    }
    let store = SqliteStore::open(&path, Timestamp(2)).unwrap();
    let held = store
        .generation(&alpha)
        .unwrap()
        .expect("the step wrote one");
    assert_eq!(held.counter, 1);
    // The tenant commits from there.
    let executed = execute(
        &store,
        &new_note("After", "after"),
        &ctx("alpha", Principal::User(principal_id("alpha:owner")), 100),
    )
    .unwrap();
    assert_eq!(executed.receipt.generation, held.raised());
}

/// Header residency over SQLite (WG-5390): the conformance suite passes through a
/// `ResidentStore`, the schema is decoded once per tenant per generation, a commit that does and
/// one that does not touch the header each leave it current, and a commit from a second handle on
/// the same files makes the next load refresh.
// Verifies: STO-001
#[test]
fn resident_store_over_sqlite_keeps_the_header_current() {
    use std::sync::Arc;
    use strata_storage_api::ResidentStore;
    let resident = || {
        let inner = Arc::new(SqliteStore::open_in_memory(Timestamp(1)).unwrap());
        Box::new(ResidentStore::header_only(inner)) as Box<dyn Store>
    };
    let ran = conformance::run(&resident);
    assert!(ran.len() >= 13, "the suite ran: {ran:?}");
    conformance::generation_follows_every_store_method(&resident);
    let fresh = || Arc::new(SqliteStore::open_in_memory(Timestamp(1)).unwrap()) as Arc<dyn Store>;
    conformance::resident_header_is_decoded_once_per_generation(&fresh);
    conformance::a_commit_leaves_the_resident_header_current(&fresh);

    let path = fresh_path("resident-foreign");
    let first = Arc::new(SqliteStore::open(&path, Timestamp(1)).unwrap());
    let second = SqliteStore::open(&path, Timestamp(2)).unwrap();
    conformance::a_foreign_commit_refreshes_the_resident_header(first, &second);
}
