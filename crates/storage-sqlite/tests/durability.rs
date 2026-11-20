//! What the SQLite store promises when things go wrong: a commit that fails part-way leaves nothing
//! of itself, a reader on another connection sees only whole commits, a file made by an older
//! version is upgraded with its data, a file made by a newer version is refused, a panic does not
//! disable a database, many tenants work, and a snapshot restores without taking back event
//! sequence numbers.

use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_runtime::sqlite::{Database, DbError};
use strata_storage_api::{execute, Store};
use strata_storage_sqlite::{default_tenants_dir, tenant_migration_steps, SqliteStore};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A fresh control database path under the build directory, with no files of it or its tenants.
fn fresh_path(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let path = dir.join(format!(
        "dur-{name}-{}-{}.db",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    remove_database(&path);
    let _ = std::fs::remove_dir_all(default_tenants_dir(&path));
    path
}

fn remove_database(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

fn owner(tenant: &str) -> Principal {
    Principal::User(principal_id(&format!("{tenant}:owner")))
}

/// Create a tenant with the notes schema and one note in it.
fn tenant_with_a_note(store: &SqliteStore, tenant: &str, at: i64) {
    execute(
        store,
        &Intent::CreateTenant {
            name: tenant.into(),
            owner: principal_id(&format!("{tenant}:owner")),
        },
        &ctx(tenant, Principal::System, at),
    )
    .unwrap();
    let schema = Intent::ChangeSchema {
        changes: notes_schema(),
        expected_version: None,
        existing: ExistingData::Reject,
    };
    execute(store, &schema, &ctx(tenant, owner(tenant), at + 1)).unwrap();
    add_note(store, tenant, "First", "first", at + 2);
}

fn add_note(store: &SqliteStore, tenant: &str, title: &str, slug: &str, at: i64) {
    execute(
        store,
        &new_note(title, slug),
        &ctx(tenant, owner(tenant), at),
    )
    .unwrap();
}

fn count(file: &Path, sql: &str) -> i64 {
    let conn = Connection::open(file).unwrap();
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// Everything a commit writes, as one comparable value: the header, the record rows, the index
/// rows, the events and the journal, read from the file by a connection of its own, and the state
/// the store loads.
fn fingerprint(store: &SqliteStore, file: &Path, tenant: &TenantId) -> (String, Vec<i64>) {
    let state = canon::hash_of(&store.load_complete(tenant).unwrap().tenant);
    let numbers = [
        "SELECT revision FROM tenant",
        "SELECT next_record FROM tenant",
        "SELECT event_seq FROM tenant",
        "SELECT COUNT(*) FROM records",
        "SELECT COUNT(*) FROM record_index",
        "SELECT COUNT(*) FROM events",
        "SELECT COUNT(*) FROM journal",
        "SELECT COUNT(*) FROM schema_history",
    ]
    .iter()
    .map(|sql| count(file, sql))
    .collect();
    (state, numbers)
}

/// A commit that fails after it has already written some of its rows leaves none of them: the
/// header, the records, the index, the events and the journal are as they were, and the store
/// takes the same commit once the fault is gone. The fault is a trigger on the table the commit
/// writes last in each case, put into the file by a second connection.
#[test]
fn a_commit_that_fails_part_way_leaves_nothing_of_itself() {
    let path = fresh_path("fault");
    let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
    tenant_with_a_note(&store, "alpha", 10);
    let alpha = tenant_id("alpha");
    let file = store.tenant_file(&alpha).unwrap().unwrap();
    let mut at = 100;
    for table in ["record_index", "events", "journal"] {
        let before = fingerprint(&store, &file, &alpha);
        let injector = Connection::open(&file).unwrap();
        injector
            .execute_batch(&format!(
                "CREATE TRIGGER inject_failure BEFORE INSERT ON {table}
                 BEGIN SELECT RAISE(ABORT, 'injected failure'); END;"
            ))
            .unwrap();
        at += 1;
        let result = execute(
            &store,
            &new_note("Lost", &format!("lost-{table}")),
            &ctx("alpha", owner("alpha"), at),
        );
        assert!(result.is_err(), "the commit survived a fault in {table}");
        assert_eq!(
            fingerprint(&store, &file, &alpha),
            before,
            "a failed commit left rows behind (fault in {table})"
        );
        injector
            .execute_batch("DROP TRIGGER inject_failure")
            .unwrap();
        drop(injector);
        at += 1;
        add_note(&store, "alpha", "Kept", &format!("kept-{table}"), at);
        let after = fingerprint(&store, &file, &alpha);
        assert_eq!(
            after.1[0],
            before.1[0] + 1,
            "the same commit goes through once the fault is gone"
        );
        assert_eq!(after.1[6], before.1[6] + 1, "one journal entry per commit");
    }
    // And the file reopens whole.
    let kept = fingerprint(&store, &file, &alpha);
    drop(store);
    let store = SqliteStore::open(&path, Timestamp(2)).unwrap();
    assert_eq!(fingerprint(&store, &file, &alpha), kept);
}

/// A transaction that is open on the tenant file and abandoned before its commit (the process
/// died, or the transaction was dropped) leaves nothing: after it, a reopened store holds exactly
/// what was committed before.
#[test]
fn an_abandoned_transaction_leaves_nothing() {
    let path = fresh_path("abandoned");
    let alpha = tenant_id("alpha");
    let (file, before) = {
        let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
        tenant_with_a_note(&store, "alpha", 10);
        let file = store.tenant_file(&alpha).unwrap().unwrap();
        let before = fingerprint(&store, &file, &alpha);
        let mut raw = Connection::open(&file).unwrap();
        let tx = raw
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        tx.execute(
            "UPDATE tenant SET revision = revision + 1, event_seq = event_seq + 1",
            [],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO events (seq, revision, data) VALUES (99, 99, '{}')",
            [],
        )
        .unwrap();
        tx.execute("DELETE FROM records", []).unwrap();
        drop(tx); // not committed
        (file, before)
    };
    let store = SqliteStore::open(&path, Timestamp(2)).unwrap();
    assert_eq!(fingerprint(&store, &file, &alpha), before);
}

/// While a writer has a transaction open and half written, a reader on another connection sees
/// the last whole commit (header, events and journal agree), and after the writer commits it sees
/// all of it at once; a rolled back transaction is never seen.
#[test]
fn a_reader_sees_the_last_whole_commit_while_a_writer_is_mid_transaction() {
    let path = fresh_path("midway");
    let alpha = tenant_id("alpha");
    let writer = SqliteStore::open(&path, Timestamp(1)).unwrap();
    tenant_with_a_note(&writer, "alpha", 10);
    let file = writer.tenant_file(&alpha).unwrap().unwrap();
    let reader = SqliteStore::open(&path, Timestamp(1)).unwrap();
    let before = fingerprint(&reader, &file, &alpha);
    let revision_before = before.1[0];

    let mut raw = Connection::open(&file).unwrap();
    for finish in ["rollback", "commit"] {
        let tx = raw
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        tx.execute(
            "UPDATE tenant SET revision = revision + 1, event_seq = event_seq + 1",
            [],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO events (seq, revision, data) VALUES (?1, ?2, '{}')",
            rusqlite::params![before.1[2] + 1, revision_before + 1],
        )
        .unwrap();
        // The header and the event are written; the journal entry is not yet.
        let during = reader.load_complete(&alpha).unwrap().tenant.unwrap();
        assert_eq!(
            during.revision,
            StateRevision(revision_before as u64),
            "the reader saw a half-written commit"
        );
        assert_eq!(count(&file, "SELECT COUNT(*) FROM events"), before.1[5]);
        assert_eq!(count(&file, "SELECT COUNT(*) FROM journal"), before.1[6]);
        tx.execute(
            "INSERT INTO journal (revision, data) VALUES (?1, '{}')",
            rusqlite::params![revision_before + 1],
        )
        .unwrap();
        assert_eq!(
            count(&file, "SELECT COUNT(*) FROM journal"),
            before.1[6],
            "the journal entry is not visible before the commit"
        );
        if finish == "rollback" {
            tx.rollback().unwrap();
            assert_eq!(
                reader
                    .load_complete(&alpha)
                    .unwrap()
                    .tenant
                    .unwrap()
                    .revision,
                StateRevision(revision_before as u64)
            );
            assert_eq!(count(&file, "SELECT COUNT(*) FROM events"), before.1[5]);
        } else {
            tx.commit().unwrap();
            assert_eq!(
                reader
                    .load_complete(&alpha)
                    .unwrap()
                    .tenant
                    .unwrap()
                    .revision,
                StateRevision(revision_before as u64 + 1)
            );
            assert_eq!(count(&file, "SELECT COUNT(*) FROM events"), before.1[5] + 1);
            assert_eq!(
                count(&file, "SELECT COUNT(*) FROM journal"),
                before.1[6] + 1
            );
        }
    }
}

/// While one store commits note after note, a read transaction on another connection always finds
/// the header, the journal and the events in step: one journal entry per revision, one event row
/// per sequence number.
#[test]
fn concurrent_reads_always_find_a_whole_commit() {
    let path = fresh_path("concurrent");
    let alpha = tenant_id("alpha");
    let writer = SqliteStore::open(&path, Timestamp(1)).unwrap();
    tenant_with_a_note(&writer, "alpha", 10);
    let file = writer.tenant_file(&alpha).unwrap().unwrap();
    let done = AtomicBool::new(false);
    let reads = AtomicU32::new(0);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut raw = Connection::open(&file).unwrap();
            raw.busy_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            loop {
                let tx = raw.transaction().unwrap();
                let (revision, event_seq): (i64, i64) = tx
                    .query_row("SELECT revision, event_seq FROM tenant", [], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })
                    .unwrap();
                let journal: i64 = tx
                    .query_row("SELECT COUNT(*) FROM journal", [], |r| r.get(0))
                    .unwrap();
                let events: i64 = tx
                    .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
                    .unwrap();
                let records: i64 = tx
                    .query_row("SELECT COUNT(*) FROM records", [], |r| r.get(0))
                    .unwrap();
                let indexed: i64 = tx
                    .query_row(
                        "SELECT COUNT(DISTINCT id) FROM record_index WHERE object = 'note'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                drop(tx);
                assert_eq!(revision, journal, "header and journal disagree");
                assert_eq!(event_seq, events, "header and events disagree");
                assert_eq!(records, indexed, "records and their index disagree");
                reads.fetch_add(1, Ordering::SeqCst);
                if done.load(Ordering::SeqCst) {
                    break;
                }
            }
        });
        for i in 0..40 {
            add_note(&writer, "alpha", "Note", &format!("note-{i}"), 100 + i);
        }
        done.store(true, Ordering::SeqCst);
    });
    assert!(reads.load(Ordering::SeqCst) > 0, "the reader never ran");
    assert_eq!(count(&file, "SELECT COUNT(*) FROM records"), 41);
}

/// A tenant file made by every older layout level, with data in it, opens and is brought to the
/// current layout with the data intact, the earlier ledger rows kept, and the store keeps working.
#[test]
fn a_file_made_at_an_older_level_is_upgraded_with_its_data() {
    let steps = tenant_migration_steps();
    assert!(steps.len() >= 2);
    let path = fresh_path("upgrade");
    let alpha = tenant_id("alpha");
    let (file, hash, journal) = {
        let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
        tenant_with_a_note(&store, "alpha", 10);
        let file = store.tenant_file(&alpha).unwrap().unwrap();
        (
            file.clone(),
            canon::hash_of(&store.load_complete(&alpha).unwrap().tenant),
            count(&file, "SELECT COUNT(*) FROM journal"),
        )
    };
    let current = fresh_path("upgrade-current");
    std::fs::copy(&file, &current).unwrap();

    for level in 1..steps.len() {
        remove_database(&file);
        {
            let old = Database::open(&file).unwrap();
            old.migrate("tenant_migrations", &steps[..level], 5)
                .unwrap();
            let conn = old.lock().unwrap();
            conn.execute(
                "ATTACH DATABASE ?1 AS cur",
                rusqlite::params![current.to_string_lossy()],
            )
            .unwrap();
            for table in ["tenant", "schema_history", "records", "events", "journal"] {
                conn.execute(
                    &format!("INSERT INTO main.{table} SELECT * FROM cur.{table}"),
                    [],
                )
                .unwrap();
            }
            // The current layout's record index has the order keys of migration 16, which the
            // older level has not.
            conn.execute(
                "INSERT INTO main.record_index (object, field, value, id)
                 SELECT object, field, value, id FROM cur.record_index",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO accounts (login, principal, display_name, password_hash)
                 VALUES ('owner', 'alpha:owner', 'Owner', '$argon2id$placeholder')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO sessions (id, token_hash, csrf_token, principal, display_name,
                                       created_at, last_seen_at, expires_at)
                 VALUES ('s-old', 'hash-old', 'csrf', 'alpha:owner', 'Owner', 77, 78, 1000000)",
                [],
            )
            .unwrap();
            conn.execute("DETACH DATABASE cur", []).unwrap();
        }
        assert_eq!(
            count(&file, "SELECT COUNT(*) FROM tenant_migrations"),
            level as i64,
            "the fixture is at level {level}"
        );

        let store = SqliteStore::open(&path, Timestamp(2)).unwrap();
        assert_eq!(
            canon::hash_of(&store.load_complete(&alpha).unwrap().tenant),
            hash,
            "level {level}: the tenant's state changed in the upgrade"
        );
        assert_eq!(
            count(&file, "SELECT COUNT(*) FROM tenant_migrations"),
            steps.len() as i64,
            "level {level}: not brought to the current layout"
        );
        assert_eq!(
            count(
                &file,
                "SELECT COUNT(*) FROM tenant_migrations WHERE applied_at = 5"
            ),
            level as i64,
            "level {level}: earlier ledger rows were rewritten"
        );
        assert_eq!(count(&file, "SELECT COUNT(*) FROM journal"), journal);
        let account = store
            .account(&alpha, "owner")
            .unwrap()
            .expect("the account");
        assert_eq!(account.principal, principal_id("alpha:owner"));
        assert_eq!(account.verified_email, None);
        let authenticated = if level < 6 { 77 } else { 0 };
        assert_eq!(
            count(
                &file,
                "SELECT authenticated_at FROM sessions WHERE id = 's-old'"
            ),
            authenticated,
            "level {level}: the session row"
        );
        add_note(
            &store,
            "alpha",
            "After upgrade",
            &format!("after-{level}"),
            500,
        );
        assert_eq!(count(&file, "SELECT COUNT(*) FROM records"), 2);
        drop(store);
    }
}

/// A file whose ledger holds a step this version does not know is refused, for the control
/// database and for a tenant file, and nothing in it is changed.
#[test]
fn a_database_migrated_by_a_newer_version_is_refused() {
    let path = fresh_path("newer");
    let alpha = tenant_id("alpha");
    let file = {
        let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
        tenant_with_a_note(&store, "alpha", 10);
        store.tenant_file(&alpha).unwrap().unwrap()
    };
    let known = tenant_migration_steps().len() as i64;
    Connection::open(&file)
        .unwrap()
        .execute(
            "INSERT INTO tenant_migrations (version, name, applied_at) VALUES (?1, 'from the future', 1)",
            [known + 1],
        )
        .unwrap();
    let store = SqliteStore::open(&path, Timestamp(2)).unwrap();
    let error = store
        .load_complete(&alpha)
        .expect_err("a tenant file of a newer version was opened");
    assert!(error.to_string().contains("newer version"), "{error}");
    drop(store);
    assert_eq!(
        count(&file, "SELECT COUNT(*) FROM tenant_migrations"),
        known + 1,
        "the refused file was not touched"
    );

    let control = fresh_path("newer-control");
    drop(SqliteStore::open(&control, Timestamp(1)).unwrap());
    Connection::open(&control)
        .unwrap()
        .execute(
            "INSERT INTO control_migrations (version, name, applied_at) VALUES (99, 'from the future', 1)",
            [],
        )
        .unwrap();
    let Err(error) = SqliteStore::open(&control, Timestamp(2)) else {
        panic!("a control database of a newer version was opened");
    };
    assert!(error.to_string().contains("newer version"), "{error}");
}

/// A migration is recorded with the time it ran, not the time the store was opened, and every
/// database file is in the write-ahead log.
#[test]
fn migrations_carry_their_own_time_and_files_are_in_wal_mode() {
    let path = fresh_path("settings");
    let alpha = tenant_id("alpha");
    let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
    tenant_with_a_note(&store, "alpha", 10);
    let file = store.tenant_file(&alpha).unwrap().unwrap();
    assert_eq!(
        count(&path, "SELECT MIN(applied_at) FROM control_migrations"),
        1,
        "the control database's step ran when the store was opened"
    );
    assert!(
        count(&file, "SELECT MIN(applied_at) FROM tenant_migrations") > 1_600_000_000_000,
        "a tenant file's steps are recorded with the time they ran"
    );
    for database in [&path, &file] {
        let mode: String = Connection::open(database)
            .unwrap()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal", "{}", database.display());
    }
}

/// A thread that panics while it holds the connection does not disable the database: its
/// transaction is rolled back and the next call works.
#[test]
fn a_panic_while_holding_the_connection_does_not_disable_the_database() {
    let db = Database::open_in_memory().unwrap();
    db.write(
        |e| e,
        |tx| -> Result<(), DbError> {
            tx.execute_batch("CREATE TABLE t (n INTEGER)")?;
            Ok(())
        },
    )
    .unwrap();
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Result<(), DbError> = db.write(
            |e| e,
            |tx| {
                tx.execute("INSERT INTO t VALUES (1)", [])?;
                panic!("a writer panicked");
            },
        );
    }));
    assert!(panicked.is_err());
    db.write(
        |e| e,
        |tx| -> Result<(), DbError> {
            tx.execute("INSERT INTO t VALUES (2)", [])?;
            Ok(())
        },
    )
    .expect("the database works after a panic");
    let rows: Vec<i64> = db
        .read(
            |e| e,
            |conn| -> Result<Vec<i64>, DbError> {
                let mut statement = conn.prepare("SELECT n FROM t ORDER BY n")?;
                let rows = statement.query_map([], |r| r.get(0))?;
                Ok(rows.collect::<Result<_, _>>()?)
            },
        )
        .unwrap();
    assert_eq!(rows, vec![2], "the panicked transaction left nothing");
}

/// More tenants than any bound on open files, each read and written, and read again in the
/// opposite order, with every tenant seeing only its own data. Uses the public store only.
#[test]
fn many_tenants_each_read_and_write_their_own_data() {
    const TENANTS: usize = 160;
    let path = fresh_path("many");
    let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
    let name = |i: usize| format!("tenant{i}");
    for i in 0..TENANTS {
        tenant_with_a_note(&store, &name(i), 10);
    }
    assert_eq!(store.tenants().unwrap().len(), TENANTS);
    for i in 0..TENANTS {
        let tenant = tenant_id(&name(i));
        let state = store.load_complete(&tenant).unwrap().tenant.unwrap();
        assert_eq!(state.record_count(), 1, "{tenant}");
        add_note(&store, &name(i), &format!("Second of {i}"), "second", 20);
    }
    for i in (0..TENANTS).rev() {
        let tenant = tenant_id(&name(i));
        let state = store.load_complete(&tenant).unwrap().tenant.unwrap();
        assert_eq!(state.record_count(), 2, "{tenant}");
        let titles = canon::canonical_json(&state.records);
        assert!(titles.contains(&format!("Second of {i}\"")), "{tenant}");
        assert!(
            !titles.contains(&format!("Second of {}\"", (i + 1) % TENANTS)),
            "{tenant} holds another tenant's record"
        );
        assert_eq!(store.journal_after(&tenant, 0, 100).unwrap().len(), 4);
    }
    drop(store);
    let store = SqliteStore::open(&path, Timestamp(2)).unwrap();
    for i in 0..TENANTS {
        let state = store
            .load_complete(&tenant_id(&name(i)))
            .unwrap()
            .tenant
            .unwrap();
        assert_eq!(state.record_count(), 2);
    }
}

/// A snapshot is a backup: restoring it returns the records, the revision, the schema and the
/// members of the moment it was taken, through a reopen too, and the journal returns to that
/// revision. The event sequence is not taken back: no sequence number a subscriber may have seen
/// is given to a different event.
#[test]
fn a_snapshot_restores_state_without_reusing_event_sequence_numbers() {
    let path = fresh_path("restore");
    let alpha = tenant_id("alpha");
    let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
    tenant_with_a_note(&store, "alpha", 10);
    let info = store
        .create_snapshot(&alpha, "backup", Timestamp(50))
        .unwrap();
    let snapshot = store.load_snapshot(&alpha, info.id).unwrap();
    add_note(&store, "alpha", "Discarded", "discarded", 60);
    add_note(&store, "alpha", "Discarded too", "discarded-too", 61);
    let seen = store.events_after(&alpha, 0, 1_000).unwrap();
    let passed = seen.last().unwrap().seq;
    assert!(passed > snapshot.event_seq);

    store.restore_snapshot(&alpha, info.id).unwrap();
    let check = |store: &SqliteStore| {
        let restored = store.load_complete(&alpha).unwrap().tenant.unwrap();
        let mut as_snapshot = TenantState::clone(&restored);
        assert!(
            as_snapshot.event_seq >= passed,
            "the event counter went back"
        );
        as_snapshot.event_seq = snapshot.event_seq;
        assert_eq!(
            canon::hash_of(&as_snapshot),
            info.hash,
            "the restored state is not the snapshot's"
        );
        assert_eq!(
            store.journal_after(&alpha, 0, 1_000).unwrap().len() as u64,
            snapshot.revision.0,
            "the journal is cut back to the snapshot's revision"
        );
    };
    check(&store);
    // Life goes on, and no sequence number is used twice or goes backwards.
    add_note(&store, "alpha", "After", "after", 70);
    let seqs: Vec<u64> = store
        .events_after(&alpha, 0, 1_000)
        .unwrap()
        .iter()
        .map(|e| e.seq)
        .collect();
    assert!(
        seqs.windows(2).all(|pair| pair[0] < pair[1]),
        "event sequence numbers repeat or go back: {seqs:?}"
    );
    assert!(
        !store
            .events_after(&alpha, passed, 1_000)
            .unwrap()
            .is_empty(),
        "the event after the restore is past every number that was seen"
    );
    drop(store);
    let store = SqliteStore::open(&path, Timestamp(3)).unwrap();
    assert_eq!(store.snapshots(&alpha).unwrap(), vec![info]);
    assert_eq!(
        store
            .load_complete(&alpha)
            .unwrap()
            .tenant
            .unwrap()
            .record_count(),
        2,
        "the note after the restore survives a reopen"
    );
}

/// The variable that makes [`a_killed_writer_child`] write; unset, that test does nothing.
const KILL_CHILD: &str = "STRATA_KILL_CHILD_DB";

/// The child half of [`a_process_killed_while_committing_leaves_only_whole_commits`]: open the
/// store the parent made and commit notes until the parent kills the process, telling the parent
/// each commit it has been answered for.
#[test]
fn a_killed_writer_child() {
    use std::io::Write;
    let Ok(path) = std::env::var(KILL_CHILD) else {
        return;
    };
    let store = SqliteStore::open(Path::new(&path), Timestamp(2)).unwrap();
    for i in 0.. {
        add_note(&store, "alpha", "Child", &format!("child-{i}"), 1_000 + i);
        let mut out = std::io::stdout().lock();
        writeln!(out, "committed {i}").unwrap();
        out.flush().unwrap();
    }
}

/// Verifies: TEN-007
///
/// A process killed while it is committing, with no chance to clean up, leaves only whole
/// commits: this test starts itself as a child that commits notes in a loop, kills the child
/// once a number of commits have been acknowledged, and opens the file again. The file passes its
/// integrity check, holds every acknowledged commit and at most one more, loads, and takes a
/// further commit.
#[test]
fn a_process_killed_while_committing_leaves_only_whole_commits() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    if std::env::var(KILL_CHILD).is_ok() {
        return;
    }
    let path = fresh_path("kill");
    {
        let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
        tenant_with_a_note(&store, "alpha", 10);
    }
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "a_killed_writer_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(KILL_CHILD, &path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut acknowledged = 0u32;
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        let line = line.unwrap();
        if line.starts_with("committed ") {
            acknowledged += 1;
            if acknowledged >= 25 {
                break;
            }
        }
    }
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(
        acknowledged >= 25,
        "the child committed only {acknowledged} times"
    );

    let store = SqliteStore::open(&path, Timestamp(3)).unwrap();
    let alpha = tenant_id("alpha");
    let file = store.tenant_file(&alpha).unwrap().unwrap();
    let check = Connection::open(&file).unwrap();
    let verdict: String = check
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(verdict, "ok");
    // The first note, the acknowledged commits, and possibly the one being written at the kill.
    let notes = count(&file, "SELECT COUNT(*) FROM records");
    assert!(
        notes == i64::from(acknowledged) + 1 || notes == i64::from(acknowledged) + 2,
        "{notes} records after {acknowledged} acknowledged commits"
    );
    // A commit is whole: the header's revision is the last journal entry, and every event stands
    // on a journal entry.
    assert_eq!(
        count(&file, "SELECT revision FROM tenant"),
        count(&file, "SELECT MAX(revision) FROM journal"),
    );
    assert_eq!(
        count(
            &file,
            "SELECT COUNT(*) FROM events WHERE revision NOT IN (SELECT revision FROM journal)"
        ),
        0,
        "an event of a commit that did not stand"
    );
    assert!(store.load_complete(&alpha).unwrap().tenant.is_some());
    add_note(&store, "alpha", "After", "after-the-kill", 5_000);
    assert_eq!(count(&file, "SELECT COUNT(*) FROM records"), notes + 1);
}
