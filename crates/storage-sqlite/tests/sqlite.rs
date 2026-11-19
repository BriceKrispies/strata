//! The SQLite adapter passes the conformance suite, on disk, and keeps its data across a reopen.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_storage_api::{
    conformance, execute, Account, FederatedIdentity, OidcFlow, PasskeyCeremony, PasskeyChallenge,
    PasskeyRecord, SamlRequest, SessionRecord, Store, ThrottleRule, ThrottleSettle,
    ThrottleVerdict,
};
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

/// The identity service's documents are kept per tenant and kind, on a database file.
#[test]
fn sqlite_store_keeps_documents() {
    let store = SqliteStore::open(&fresh_path("documents"), Timestamp(1)).unwrap();
    conformance::documents_are_scoped_to_their_tenant_and_kind(&store);
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
    assert_eq!(migrations.len(), 1, "the control database's steps");
    assert_eq!(
        migrations[0].applied_at,
        Timestamp(1),
        "the layout step is recorded once, not on every open"
    );
    let file = store.tenant_file(&alpha).unwrap().unwrap();
    assert_eq!(
        count(&file, "SELECT COUNT(*) FROM tenant_migrations"),
        8,
        "the tenant file's steps"
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
                authenticated_at: Timestamp(10),
                network: None,
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
             AND name IN ('records', 'events', 'journal', 'sessions', 'accounts', 'tenant', \
             'login_throttle', 'passkeys', 'passkey_challenges', 'sign_in_links', 'oidc_flows', \
             'federated_identities', 'totp_factors', 'recovery_codes', 'pending_sign_ins', \
             'recovery_requests', 'saml_requests', 'saml_assertions', 'identity_docs')"
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

fn rule(key: &str, max: u32, lock_resets: bool) -> ThrottleRule {
    ThrottleRule {
        key: key.into(),
        max,
        window_ms: 1000,
        lock_ms: 100,
        lock_resets,
    }
}

/// Verifies: IDN-008
///
/// The counters are rows: a reopened file holds them, a tenant reset takes them, a right
/// password clears one, and an expired one is purged.
#[test]
fn login_throttle_counters_are_durable_atomic_and_purged() {
    let path = fresh_path("throttle");
    let alpha = tenant_id("alpha");
    let account = rule("account:password:a", 3, true);
    let source = rule("source:password:1.2.3.4", 2, false);
    {
        let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
        execute(
            &store,
            &Intent::CreateTenant {
                name: "alpha".into(),
                owner: principal_id("alpha:owner"),
            },
            &ctx("alpha", Principal::System, 10),
        )
        .unwrap();
        let both = [account.clone(), source.clone()];
        for _ in 0..2 {
            assert_eq!(
                store
                    .throttle_reserve(&alpha, &both, Timestamp(100))
                    .unwrap(),
                ThrottleVerdict::Allowed
            );
        }
        // The source is full: a refusal counts nothing on the account either.
        assert!(matches!(
            store
                .throttle_reserve(&alpha, &both, Timestamp(100))
                .unwrap(),
            ThrottleVerdict::Refused { .. }
        ));
    }
    let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
    let verdict = store
        .throttle_reserve(&alpha, std::slice::from_ref(&account), Timestamp(100))
        .unwrap();
    assert_eq!(
        verdict,
        ThrottleVerdict::Allowed,
        "the account had two, not three"
    );
    assert!(
        matches!(
            store
                .throttle_reserve(&alpha, std::slice::from_ref(&source), Timestamp(100))
                .unwrap(),
            ThrottleVerdict::Refused {
                until: Timestamp(200)
            }
        ),
        "the source count survived the reopen, with its lock, which ends at 200"
    );
    store
        .throttle_settle(&alpha, &account.key, 3, ThrottleSettle::Clear)
        .unwrap();
    assert_eq!(
        store
            .throttle_reserve(&alpha, std::slice::from_ref(&account), Timestamp(100))
            .unwrap(),
        ThrottleVerdict::Allowed
    );
    // After its window a counter starts again, and the expired rows can be purged.
    assert_eq!(
        store
            .throttle_reserve(&alpha, std::slice::from_ref(&source), Timestamp(1200))
            .unwrap(),
        ThrottleVerdict::Allowed
    );
    assert_eq!(store.purge_throttle(Timestamp(1_000_000)).unwrap(), 2);
    assert_eq!(store.purge_throttle(Timestamp(1_000_000)).unwrap(), 0);
}

/// Verifies: IDN-264
///
/// Passkeys and their challenges are rows: a reopened file holds them, a credential id is
/// unique within a tenant, a challenge is taken once and not after it lapses, and a tenant reset
/// takes them all.
#[test]
fn passkeys_and_challenges_are_durable_and_a_challenge_is_taken_once() {
    let path = fresh_path("passkeys");
    let alpha = tenant_id("alpha");
    let ines = principal_id("user:ines");
    let passkey = |id: &str, principal: &PrincipalId, at: i64| PasskeyRecord {
        tenant: alpha.clone(),
        principal: principal.clone(),
        id: id.into(),
        rp_id: "alpha.example".into(),
        label: "Key".into(),
        credential: "{}".into(),
        created_at: Timestamp(at),
        last_used_at: None,
    };
    let challenge = |hash: &str, expires: i64| PasskeyChallenge {
        tenant: alpha.clone(),
        id_hash: hash.into(),
        ceremony: PasskeyCeremony::SignIn,
        session: None,
        principal: None,
        origin: "https://alpha.example".into(),
        state: "{}".into(),
        expires_at: Timestamp(expires),
    };
    {
        let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
        execute(
            &store,
            &Intent::CreateTenant {
                name: "alpha".into(),
                owner: principal_id("alpha:owner"),
            },
            &ctx("alpha", Principal::System, 10),
        )
        .unwrap();
        assert!(store.put_passkey(&passkey("k2", &ines, 20)).unwrap());
        assert!(store.put_passkey(&passkey("k1", &ines, 10)).unwrap());
        assert!(
            !store
                .put_passkey(&passkey("k1", &principal_id("user:omar"), 30))
                .unwrap(),
            "a credential id is held once in a tenant, whoever brings it"
        );
        store
            .put_passkey_challenge(&challenge("live", 1000))
            .unwrap();
        store
            .put_passkey_challenge(&challenge("lapsed", 50))
            .unwrap();
    }
    let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
    let held = store.passkeys(&alpha, &ines).unwrap();
    assert_eq!(
        held.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        ["k1", "k2"],
        "oldest first, and only this person's"
    );
    assert_eq!(held[0].rp_id, "alpha.example");
    assert!(store
        .update_passkey(&alpha, "k1", "{\"cred\":1}", Timestamp(99))
        .unwrap());
    let used = store.passkey(&alpha, "k1").unwrap().unwrap();
    assert_eq!(used.credential, "{\"cred\":1}");
    assert_eq!(used.last_used_at, Some(Timestamp(99)));

    assert!(store
        .take_passkey_challenge(&alpha, "live", Timestamp(100))
        .unwrap()
        .is_some());
    assert!(
        store
            .take_passkey_challenge(&alpha, "live", Timestamp(100))
            .unwrap()
            .is_none(),
        "taken once"
    );
    assert!(
        store
            .take_passkey_challenge(&alpha, "lapsed", Timestamp(100))
            .unwrap()
            .is_none(),
        "not after it lapsed"
    );
    store
        .put_passkey_challenge(&challenge("later", 60))
        .unwrap();
    assert_eq!(store.purge_passkey_challenges(Timestamp(100)).unwrap(), 1);

    assert!(!store
        .remove_passkey(&alpha, &principal_id("user:omar"), "k1")
        .unwrap());
    assert!(store.remove_passkey(&alpha, &ines, "k1").unwrap());
    store.reset_tenant(&alpha).unwrap();
    assert!(store.passkey(&alpha, "k2").unwrap().is_none());
}

/// Verifies: STO-002
///
/// A started identity-provider sign-in is a row: it survives a reopen, is taken once, and is
/// returned even when lapsed (the caller compares the time). A provider's subject is tied to one
/// principal, once, and only within its own tenant and provider; a tenant reset takes them all.
#[test]
fn identity_provider_sign_ins_are_taken_once_and_identities_are_tied_once() {
    let path = fresh_path("oidc");
    let alpha = tenant_id("alpha");
    let ines = principal_id("user:ines");
    let flow = |hash: &str, expires: i64| OidcFlow {
        tenant: alpha.clone(),
        id_hash: hash.into(),
        provider: "acme".into(),
        nonce: "n".into(),
        verifier: "v".into(),
        binding_hash: "b".into(),
        origin: "https://alpha.example".into(),
        principal: Some(ines.clone()),
        expires_at: Timestamp(expires),
    };
    let identity = |provider: &str, subject: &str, principal: &PrincipalId| FederatedIdentity {
        tenant: alpha.clone(),
        provider: provider.into(),
        subject: subject.into(),
        principal: principal.clone(),
        created_at: Timestamp(5),
    };
    {
        let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
        execute(
            &store,
            &Intent::CreateTenant {
                name: "alpha".into(),
                owner: principal_id("alpha:owner"),
            },
            &ctx("alpha", Principal::System, 10),
        )
        .unwrap();
        store.put_oidc_flow(&flow("live", 1000)).unwrap();
        store.put_oidc_flow(&flow("lapsed", 50)).unwrap();
        assert!(store
            .put_identity(&identity("acme", "sub-1", &ines))
            .unwrap());
    }
    let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
    assert_eq!(
        store.take_oidc_flow(&alpha, "live").unwrap(),
        Some(flow("live", 1000))
    );
    assert!(
        store.take_oidc_flow(&alpha, "live").unwrap().is_none(),
        "taken once"
    );
    assert_eq!(
        store
            .take_oidc_flow(&alpha, "lapsed")
            .unwrap()
            .map(|f| f.expires_at),
        Some(Timestamp(50)),
        "a lapsed request is returned, for the caller to refuse"
    );
    store.put_oidc_flow(&flow("later", 60)).unwrap();
    assert_eq!(store.purge_oidc_flows(Timestamp(100)).unwrap(), 1);

    let known = store.identity(&alpha, "acme", "sub-1").unwrap().unwrap();
    assert_eq!(known.principal, ines);
    assert!(
        !store
            .put_identity(&identity("acme", "sub-1", &principal_id("user:omar")))
            .unwrap(),
        "a subject is tied to one principal"
    );
    assert_eq!(
        store
            .identity(&alpha, "acme", "sub-1")
            .unwrap()
            .unwrap()
            .principal,
        ines
    );
    assert!(
        store.identity(&alpha, "other", "sub-1").unwrap().is_none(),
        "one provider's subject is not another's"
    );
    store.reset_tenant(&alpha).unwrap();
    assert!(store.identity(&alpha, "acme", "sub-1").unwrap().is_none());
}

/// Second-factor state: a confirmed authenticator is not replaced, a time step is accepted once,
/// a recovery code is spent once, a waiting sign-in counts its presentations and is taken once,
/// and a recovery is replaced by a newer one of the same person. All of it is durable.
#[test]
fn second_factor_state_is_durable_and_answers_once() {
    use strata_storage_api::{PendingSignIn, RecoveryRequest, TotpRecord};
    let path = fresh_path("factors");
    let alpha = tenant_id("alpha");
    let ines = principal_id("user:ines");
    let totp = |confirmed: bool| TotpRecord {
        tenant: alpha.clone(),
        principal: ines.clone(),
        secret: "sealed".into(),
        confirmed,
        last_step: -1,
        created_at: Timestamp(1),
    };
    {
        let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
        execute(
            &store,
            &Intent::CreateTenant {
                name: "alpha".into(),
                owner: principal_id("alpha:owner"),
            },
            &ctx("alpha", Principal::System, 10),
        )
        .unwrap();

        assert!(store.put_totp(&totp(false)).unwrap());
        assert!(
            !store.use_totp_step(&alpha, &ines, 5).unwrap(),
            "unconfirmed is no factor"
        );
        assert!(store.confirm_totp(&alpha, &ines, 10).unwrap());
        assert!(
            !store.put_totp(&totp(false)).unwrap(),
            "a confirmed factor is not replaced"
        );
        assert!(
            !store.use_totp_step(&alpha, &ines, 10).unwrap(),
            "the step of the confirming code"
        );
        assert!(store.use_totp_step(&alpha, &ines, 11).unwrap());
        assert!(
            !store.use_totp_step(&alpha, &ines, 11).unwrap(),
            "a step is accepted once"
        );

        let hashes = vec!["h1".to_string(), "h2".to_string()];
        store
            .replace_recovery_codes(&alpha, &ines, &hashes, Timestamp(5))
            .unwrap();
        assert!(store.spend_recovery_code(&alpha, &ines, "h1").unwrap());
        assert!(
            !store.spend_recovery_code(&alpha, &ines, "h1").unwrap(),
            "a code is spent once"
        );
        assert_eq!(store.recovery_codes_left(&alpha, &ines).unwrap(), 1);

        let waiting = PendingSignIn {
            tenant: alpha.clone(),
            id_hash: "w".into(),
            principal: ines.clone(),
            method: "password".into(),
            factors: 1,
            attempts: 0,
            expires_at: Timestamp(100),
        };
        store.put_pending(&waiting).unwrap();
        assert_eq!(
            store
                .attempt_pending(&alpha, "w", Timestamp(50), 2)
                .unwrap()
                .unwrap()
                .attempts,
            1
        );
        assert_eq!(
            store
                .attempt_pending(&alpha, "w", Timestamp(50), 2)
                .unwrap()
                .unwrap()
                .attempts,
            2
        );
        assert!(
            store
                .attempt_pending(&alpha, "w", Timestamp(50), 2)
                .unwrap()
                .is_none(),
            "spent"
        );
        assert!(store
            .take_pending(&alpha, "w", Timestamp(50))
            .unwrap()
            .is_none());
        store.put_pending(&waiting).unwrap();
        assert!(store
            .take_pending(&alpha, "w", Timestamp(50))
            .unwrap()
            .is_some());
        assert!(
            store
                .take_pending(&alpha, "w", Timestamp(50))
                .unwrap()
                .is_none(),
            "taken once"
        );
        store.put_pending(&waiting).unwrap();
        assert!(
            store
                .take_pending(&alpha, "w", Timestamp(100))
                .unwrap()
                .is_none(),
            "lapsed"
        );

        let request = |id: &str| RecoveryRequest {
            tenant: alpha.clone(),
            id_hash: id.into(),
            principal: ines.clone(),
            login: "ines".into(),
            token_hash: "t".into(),
            cancel_hash: "c".into(),
            requested_at: Timestamp(1),
            completes_at: Timestamp(2),
            expires_at: Timestamp(3),
        };
        store.put_recovery(&request("r1")).unwrap();
        store.put_recovery(&request("r2")).unwrap();
        assert!(
            store.recovery(&alpha, "r1").unwrap().is_none(),
            "replaced by the newer request"
        );
        assert!(!store.cancel_recovery(&alpha, "r2", "wrong").unwrap());
        assert!(store.cancel_recovery(&alpha, "r2", "c").unwrap());
        store.put_recovery(&request("r3")).unwrap();
        assert_eq!(
            store.take_recovery(&alpha, "r3").unwrap().unwrap().login,
            "ines"
        );
        assert!(store.take_recovery(&alpha, "r3").unwrap().is_none());
    }
    // Reopened: what was kept is still there.
    let store = SqliteStore::open(&path, Timestamp(2)).unwrap();
    assert!(store.totp(&alpha, &ines).unwrap().unwrap().confirmed);
    assert_eq!(store.totp(&alpha, &ines).unwrap().unwrap().last_step, 11);
    assert_eq!(store.recovery_codes_left(&alpha, &ines).unwrap(), 1);
    store.reset_tenant(&alpha).unwrap();
    assert!(store.totp(&alpha, &ines).unwrap().is_none());
}

/// Verifies: STO-002
///
/// A started SAML sign-in is a row: it survives a reopen, is taken once, and is returned even when
/// lapsed (the caller compares the time). An accepted assertion is remembered once per tenant and
/// provider, by one statement, so that a second presentation finds it; the memory outlives a
/// reopen, is purged only when the assertion could no longer be accepted, and goes with the
/// tenant.
#[test]
fn saml_sign_ins_are_taken_once_and_accepted_assertions_are_remembered_once() {
    let path = fresh_path("saml");
    let alpha = tenant_id("alpha");
    let ines = principal_id("user:ines");
    let request = |hash: &str, expires: i64| SamlRequest {
        tenant: alpha.clone(),
        id_hash: hash.into(),
        provider: "acme".into(),
        request_id: "_request-1".into(),
        binding_hash: "b".into(),
        origin: "https://alpha.example".into(),
        principal: Some(ines.clone()),
        expires_at: Timestamp(expires),
    };
    {
        let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
        execute(
            &store,
            &Intent::CreateTenant {
                name: "alpha".into(),
                owner: principal_id("alpha:owner"),
            },
            &ctx("alpha", Principal::System, 10),
        )
        .unwrap();
        store.put_saml_request(&request("live", 1000)).unwrap();
        store.put_saml_request(&request("lapsed", 50)).unwrap();
        assert!(store
            .put_saml_assertion(&alpha, "acme", "assertion-1", Timestamp(500))
            .unwrap());
    }
    let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
    assert_eq!(
        store.take_saml_request(&alpha, "live").unwrap(),
        Some(request("live", 1000))
    );
    assert!(
        store.take_saml_request(&alpha, "live").unwrap().is_none(),
        "taken once"
    );
    assert_eq!(
        store
            .take_saml_request(&alpha, "lapsed")
            .unwrap()
            .map(|r| r.expires_at),
        Some(Timestamp(50)),
        "a lapsed request is returned, for the caller to refuse"
    );
    store.put_saml_request(&request("later", 60)).unwrap();
    assert_eq!(store.purge_saml_requests(Timestamp(100)).unwrap(), 1);

    assert!(
        !store
            .put_saml_assertion(&alpha, "acme", "assertion-1", Timestamp(900))
            .unwrap(),
        "an assertion that was accepted is remembered across a reopen"
    );
    assert!(
        store
            .put_saml_assertion(&alpha, "other", "assertion-1", Timestamp(500))
            .unwrap(),
        "one provider's identifier is not another's"
    );
    assert_eq!(store.purge_saml_assertions(Timestamp(499)).unwrap(), 0);
    assert_eq!(store.purge_saml_assertions(Timestamp(501)).unwrap(), 2);
    assert!(
        store
            .put_saml_assertion(&alpha, "acme", "assertion-1", Timestamp(900))
            .unwrap(),
        "a purged identifier is free again, which is safe once the assertion has expired"
    );
    store.reset_tenant(&alpha).unwrap();
    assert!(store
        .put_saml_assertion(&alpha, "acme", "assertion-1", Timestamp(900))
        .is_err());
}
