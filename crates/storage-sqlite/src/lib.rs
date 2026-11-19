//! The SQLite adapter. Every SQL statement and table of the tenant store lives in this file.
//!
//! Layout (TEN-001): a **control database** and **one database file per tenant**.
//!
//! - The control database (the file named when the store is opened, such as `var/strata.db`)
//!   holds the tenant directory (`tenant_directory`: id, state, file) and the session directory
//!   (`session_directory`: which tenant file holds a session, so that a request's session is
//!   found by its token hash without opening every tenant file). It holds no tenant content.
//! - Each tenant file (by default `<control stem>-tenants/<tenant>.db` beside the control
//!   database) holds everything the tenant owns: its header row with the current schema
//!   (`tenant`), the schema history, its records (each stored as its canonical JSON document)
//!   with their field index, its events (which double as its outbox through their `published`
//!   flag, written in the same transaction as the state they describe), its journal, snapshots,
//!   sessions and logins, and what its sign-in methods keep (throttle counters, passkeys, links,
//!   identity-provider flows, federated identities, second factors and recoveries).
//!
//! A tenant file is created by the commit that creates the tenant, and removed by
//! [`Store::reset_tenant`] and [`Store::reset`]. Removal marks the directory row `deleting`
//! first, so a tenant whose file could not be removed loads as absent and the next open finishes
//! the removal.
//!
//! The outbox is per tenant: positions increase in one tenant's commit order and are never
//! reused within its file. [`Store::outbox`] fans the tenants' outboxes in, and
//! [`Store::mark_published`] marks one tenant's outbox. Since every outbox row is written by the
//! commit it describes, an event is only ever returned after its commit.
//!
//! The old single-file layout (every tenant in one file, separated by a tenant column) is
//! refused on open with a message saying to delete the file: development data is reseeded.
//!
//! Concurrency: one connection per file behind a mutex, write transactions opened `IMMEDIATE`,
//! WAL journal, `synchronous = FULL`, foreign keys on, a busy timeout for other processes that
//! have the file open. The connection handling and the migration runner are the runtime's
//! ([`strata_runtime::sqlite::Database`]); the tables and statements of the tenant store are
//! here.

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use strata_kernel::{
    canon, Coverage, DeltaOp, DomainEvent, Need, ObjectTypeId, PrincipalId, Record, RoleId,
    SessionId, State, StateRevision, TenantId, TenantSchema, TenantState, Timestamp, Value,
};
use strata_runtime::sqlite::{Database, DbError, Migration};
use strata_storage_api::{
    check_event_sequence, field_matches, throttle_refund, throttle_step, Account, Capabilities,
    Commit, FederatedIdentity, Health, JournalEntry, OidcFlow, OutboxEntry, PasskeyCeremony,
    PasskeyChallenge, PasskeyRecord, PendingSignIn, Receipt, RecoveryRequest, SamlRequest,
    SessionRecord, SignInLink, SnapshotInfo, StorageMigration, Store, StoreError, StoreResult,
    TenantSummary, ThrottleRow, ThrottleRule, ThrottleSettle, ThrottleVerdict, TotpRecord,
};

/// The ledger table of the control database's migrations.
const CONTROL_LEDGER: &str = "control_migrations";
/// The ledger table of a tenant file's migrations.
const TENANT_LEDGER: &str = "tenant_migrations";

/// The control database's layout, one step per entry. Steps are only ever appended.
const CONTROL_MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "tenant directory and session directory",
    sql: "
    CREATE TABLE tenant_directory (
        id         TEXT PRIMARY KEY,
        state      TEXT NOT NULL CHECK (state IN ('active', 'deleting')),
        file       TEXT NOT NULL UNIQUE,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE session_directory (
        id         TEXT PRIMARY KEY,
        token_hash TEXT NOT NULL UNIQUE,
        tenant     TEXT NOT NULL REFERENCES tenant_directory(id) ON DELETE CASCADE
    );
    CREATE INDEX session_directory_tenant ON session_directory(tenant);
    ",
}];

/// A tenant file's layout, one step per entry. Steps are only ever appended.
///
/// Everything a sign-in method keeps (throttle counters, passkeys and their challenges, emailed
/// links, identity-provider flows and the identities they tie, second factors, recovery codes,
/// waiting sign-ins and recovery requests) belongs to one tenant and is looked up with the tenant
/// known, so it lives in the tenant's file and goes with it when the tenant is removed.
const TENANT_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "tenant file layout",
        sql: "
    CREATE TABLE tenant (
        id          TEXT PRIMARY KEY,
        name        TEXT NOT NULL,
        revision    INTEGER NOT NULL,
        next_record INTEGER NOT NULL,
        event_seq   INTEGER NOT NULL,
        schema      TEXT NOT NULL,
        members     TEXT NOT NULL
    );
    CREATE TABLE schema_history (
        version  INTEGER PRIMARY KEY,
        revision INTEGER NOT NULL,
        schema   TEXT NOT NULL
    );
    CREATE TABLE records (
        object  TEXT NOT NULL,
        id      INTEGER NOT NULL,
        version INTEGER NOT NULL,
        data    TEXT NOT NULL,
        PRIMARY KEY (object, id)
    );
    CREATE TABLE record_index (
        object TEXT NOT NULL,
        field  TEXT NOT NULL,
        value  TEXT NOT NULL,
        id     INTEGER NOT NULL,
        PRIMARY KEY (object, field, value, id),
        FOREIGN KEY (object, id) REFERENCES records(object, id) ON DELETE CASCADE
    );
    CREATE TABLE events (
        position  INTEGER PRIMARY KEY AUTOINCREMENT,
        seq       INTEGER NOT NULL UNIQUE,
        revision  INTEGER NOT NULL,
        data      TEXT NOT NULL,
        published INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX events_outbox ON events(position) WHERE published = 0;
    CREATE TABLE journal (
        revision INTEGER PRIMARY KEY,
        data     TEXT NOT NULL
    );
    CREATE TABLE sessions (
        id           TEXT PRIMARY KEY,
        token_hash   TEXT NOT NULL UNIQUE,
        csrf_token   TEXT NOT NULL,
        principal    TEXT NOT NULL,
        display_name TEXT NOT NULL,
        created_at   INTEGER NOT NULL,
        last_seen_at INTEGER NOT NULL,
        expires_at   INTEGER NOT NULL,
        revoked_at   INTEGER
    );
    CREATE INDEX sessions_principal ON sessions(principal);
    CREATE TABLE accounts (
        login         TEXT PRIMARY KEY,
        principal     TEXT NOT NULL,
        display_name  TEXT NOT NULL,
        password_hash TEXT NOT NULL
    );
    CREATE TABLE snapshots (
        id         INTEGER PRIMARY KEY,
        label      TEXT NOT NULL,
        revision   INTEGER NOT NULL,
        hash       TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        data       TEXT NOT NULL
    );
    ",
    },
    Migration {
        version: 2,
        name: "login throttle",
        sql: "
    CREATE TABLE login_throttle (
        key          TEXT PRIMARY KEY,
        failures     INTEGER NOT NULL,
        window_start INTEGER NOT NULL,
        locked_until INTEGER NOT NULL,
        expires_at   INTEGER NOT NULL
    );
    CREATE INDEX login_throttle_expiry ON login_throttle(expires_at);
    ",
    },
    Migration {
        version: 3,
        name: "passkeys",
        sql: "
    CREATE TABLE passkeys (
        id           TEXT PRIMARY KEY,
        principal    TEXT NOT NULL,
        rp_id        TEXT NOT NULL,
        label        TEXT NOT NULL,
        credential   TEXT NOT NULL,
        created_at   INTEGER NOT NULL,
        last_used_at INTEGER
    );
    CREATE INDEX passkeys_principal ON passkeys(principal, created_at);
    CREATE TABLE passkey_challenges (
        id_hash    TEXT PRIMARY KEY,
        ceremony   TEXT NOT NULL,
        session    TEXT,
        principal  TEXT,
        origin     TEXT NOT NULL,
        state      TEXT NOT NULL,
        expires_at INTEGER NOT NULL
    );
    CREATE INDEX passkey_challenges_expiry ON passkey_challenges(expires_at);
    CREATE INDEX accounts_principal ON accounts(principal);
    ",
    },
    Migration {
        version: 4,
        name: "sign-in links",
        sql: "
    CREATE TABLE sign_in_links (
        id_hash      TEXT PRIMARY KEY,
        principal    TEXT NOT NULL,
        token_hash   TEXT NOT NULL,
        binding_hash TEXT NOT NULL,
        origin       TEXT NOT NULL,
        expires_at   INTEGER NOT NULL
    );
    CREATE INDEX sign_in_links_expiry ON sign_in_links(expires_at);
    ",
    },
    Migration {
        version: 5,
        name: "identity-provider sign-ins and federated identities",
        sql: "
    CREATE TABLE oidc_flows (
        id_hash      TEXT PRIMARY KEY,
        provider     TEXT NOT NULL,
        nonce        TEXT NOT NULL,
        verifier     TEXT NOT NULL,
        binding_hash TEXT NOT NULL,
        origin       TEXT NOT NULL,
        principal    TEXT,
        expires_at   INTEGER NOT NULL
    );
    CREATE INDEX oidc_flows_expiry ON oidc_flows(expires_at);
    CREATE TABLE federated_identities (
        provider   TEXT NOT NULL,
        subject    TEXT NOT NULL,
        principal  TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        PRIMARY KEY (provider, subject)
    );
    CREATE INDEX federated_identities_principal ON federated_identities(principal);
    ",
    },
    Migration {
        version: 6,
        name: "second factors, recovery codes, pending sign-ins, recovery requests and session authentication",
        sql: "
    CREATE TABLE totp_factors (
        principal  TEXT PRIMARY KEY,
        secret     TEXT NOT NULL,
        confirmed  INTEGER NOT NULL,
        last_step  INTEGER NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE recovery_codes (
        principal  TEXT NOT NULL,
        code_hash  TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        PRIMARY KEY (principal, code_hash)
    );
    CREATE TABLE pending_sign_ins (
        id_hash    TEXT PRIMARY KEY,
        principal  TEXT NOT NULL,
        method     TEXT NOT NULL,
        factors    INTEGER NOT NULL,
        attempts   INTEGER NOT NULL,
        expires_at INTEGER NOT NULL
    );
    CREATE INDEX pending_sign_ins_expiry ON pending_sign_ins(expires_at);
    CREATE TABLE recovery_requests (
        id_hash      TEXT PRIMARY KEY,
        principal    TEXT NOT NULL,
        login        TEXT NOT NULL,
        token_hash   TEXT NOT NULL,
        cancel_hash  TEXT NOT NULL,
        requested_at INTEGER NOT NULL,
        completes_at INTEGER NOT NULL,
        expires_at   INTEGER NOT NULL
    );
    CREATE INDEX recovery_requests_principal ON recovery_requests(principal);
    CREATE INDEX recovery_requests_expiry ON recovery_requests(expires_at);
    ALTER TABLE sessions ADD COLUMN authenticated_at INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE sessions ADD COLUMN network TEXT;
    UPDATE sessions SET authenticated_at = created_at;
    ",
    },
    Migration {
        version: 7,
        name: "SAML sign-ins and accepted assertions",
        sql: "
    CREATE TABLE saml_requests (
        id_hash      TEXT PRIMARY KEY,
        provider     TEXT NOT NULL,
        request_id   TEXT NOT NULL,
        binding_hash TEXT NOT NULL,
        origin       TEXT NOT NULL,
        principal    TEXT,
        expires_at   INTEGER NOT NULL
    );
    CREATE INDEX saml_requests_expiry ON saml_requests(expires_at);
    CREATE TABLE saml_assertions (
        provider   TEXT NOT NULL,
        id_hash    TEXT NOT NULL,
        expires_at INTEGER NOT NULL,
        PRIMARY KEY (provider, id_hash)
    );
    CREATE INDEX saml_assertions_expiry ON saml_assertions(expires_at);
    ",
    },
];

/// Index values longer than this are not indexed; lookups for them scan the object type.
const MAX_INDEXED: usize = 240;

/// Device names Windows reserves whatever their extension; a tenant with such an id gets a file
/// name that no tenant id can produce (tenant ids never contain `-`).
const RESERVED_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// The directory that holds the tenant files of a control database when none is given:
/// `<stem>-tenants` beside it, so that two control databases in one directory (`var/strata.db`,
/// `var/e2e.db`) never share tenant files.
pub fn default_tenants_dir(control: &Path) -> PathBuf {
    let stem = control
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "strata".into());
    control
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(format!("{stem}-tenants"))
}

/// The file name of a tenant's database inside the tenants directory.
pub fn tenant_file_name(tenant: &TenantId) -> String {
    let id = tenant.as_str();
    if RESERVED_NAMES.contains(&id) {
        format!("{id}-.db")
    } else {
        format!("{id}.db")
    }
}

/// Where the tenant files live.
enum Place {
    Disk(PathBuf),
    Memory,
}

/// Implements: H-008
pub struct SqliteStore {
    control: Database,
    place: Place,
    /// Open tenant files, by tenant. In memory this is where the tenant databases live.
    open: Mutex<BTreeMap<TenantId, Database>>,
    /// When the store was opened: recorded in the ledger of the tenant files it lays out.
    opened_at: Timestamp,
}

fn db(error: rusqlite::Error) -> StoreError {
    match &error {
        rusqlite::Error::SqliteFailure(code, _)
            if code.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            StoreError::Invalid(error.to_string())
        }
        _ => StoreError::Unavailable(error.to_string()),
    }
}

fn dbe(error: DbError) -> StoreError {
    match error {
        DbError::Sql(error) => db(error),
        other => StoreError::Unavailable(other.to_string()),
    }
}

fn encode<T: Serialize>(value: &T) -> String {
    canon::canonical_json(value)
}

fn decode<T: DeserializeOwned>(text: &str, what: &str) -> StoreResult<T> {
    serde_json::from_str(text).map_err(|e| StoreError::Corrupt(format!("{what}: {e}")))
}

fn corrupt_id(e: strata_kernel::IdError) -> StoreError {
    StoreError::Corrupt(e.to_string())
}

/// The index rows of a record: `(field, value key)`.
fn index_rows(record: &Record) -> Vec<(String, String)> {
    let mut rows = BTreeSet::new();
    for (field, value) in &record.fields {
        let items: Vec<&Value> = match value {
            Value::List(items) => items.iter().collect(),
            other => vec![other],
        };
        for item in items {
            let key = encode(item);
            if key.len() <= MAX_INDEXED {
                rows.insert((field.as_str().to_string(), key));
            }
        }
    }
    rows.into_iter().collect()
}

/// The header of the tenant in an open tenant file, without records.
fn header(tx: &Connection, tenant: &TenantId) -> StoreResult<Option<TenantState>> {
    let row = tx
        .query_row(
            "SELECT name, revision, next_record, event_seq, schema, members FROM tenant WHERE id = ?1",
            params![tenant.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(db)?;
    let Some((name, revision, next_record, event_seq, schema, members)) = row else {
        return Ok(None);
    };
    let schema: TenantSchema = decode(&schema, "tenant schema")?;
    let members: BTreeMap<PrincipalId, BTreeSet<RoleId>> = decode(&members, "tenant members")?;
    Ok(Some(TenantState {
        id: tenant.clone(),
        name,
        revision: StateRevision(revision as u64),
        next_record: next_record as u64,
        event_seq: event_seq as u64,
        schema: Arc::new(schema),
        members,
        records: BTreeMap::new(),
    }))
}

fn put_records(state: &mut TenantState, rows: Vec<String>) -> StoreResult<()> {
    for data in rows {
        let record: Record = decode(&data, "record")?;
        state
            .records
            .entry(record.object.clone())
            .or_default()
            .insert(record.id, Arc::new(record));
    }
    Ok(())
}

fn select_strings(
    tx: &Connection,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> StoreResult<Vec<String>> {
    let mut statement = tx.prepare_cached(sql).map_err(db)?;
    let rows = statement
        .query_map(args, |row| row.get::<_, String>(0))
        .map_err(db)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db)
}

fn all_of(tx: &Connection, object: &ObjectTypeId) -> StoreResult<Vec<String>> {
    select_strings(
        tx,
        "SELECT data FROM records WHERE object = ?1 ORDER BY id",
        &[&object.as_str()],
    )
}

fn complete(conn: &Connection, tenant: &TenantId) -> StoreResult<Option<TenantState>> {
    let Some(mut state) = header(conn, tenant)? else {
        return Ok(None);
    };
    let rows = select_strings(conn, "SELECT data FROM records ORDER BY object, id", &[])?;
    put_records(&mut state, rows)?;
    Ok(Some(state))
}

fn write_record(tx: &Transaction<'_>, record: &Record) -> StoreResult<()> {
    let (object, id) = (record.object.as_str(), record.id.0 as i64);
    tx.execute(
        "INSERT INTO records (object, id, version, data) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (object, id) DO UPDATE SET version = excluded.version, data = excluded.data",
        params![object, id, record.version as i64, encode(record)],
    )
    .map_err(db)?;
    tx.execute(
        "DELETE FROM record_index WHERE object = ?1 AND id = ?2",
        params![object, id],
    )
    .map_err(db)?;
    let mut insert = tx
        .prepare_cached(
            "INSERT INTO record_index (object, field, value, id) VALUES (?1, ?2, ?3, ?4)",
        )
        .map_err(db)?;
    for (field, value) in index_rows(record) {
        insert
            .execute(params![object, field, value, id])
            .map_err(db)?;
    }
    Ok(())
}

/// A throttle counter of an open tenant file.
fn throttle_row(conn: &Connection, key: &str) -> StoreResult<Option<ThrottleRow>> {
    conn.query_row(
        "SELECT failures, window_start, locked_until, expires_at FROM login_throttle WHERE key = ?1",
        params![key],
        |r| {
            Ok(ThrottleRow {
                failures: r.get::<_, i64>(0)? as u32,
                window_start: r.get(1)?,
                locked_until: r.get(2)?,
                expires_at: r.get(3)?,
            })
        },
    )
    .optional()
    .map_err(db)
}

const PASSKEY_COLUMNS: &str = "id, principal, rp_id, label, credential, created_at, last_used_at";

type PasskeyRow = (String, String, String, String, String, i64, Option<i64>);

fn passkey_from(row: &rusqlite::Row<'_>) -> rusqlite::Result<PasskeyRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

fn passkey_record(
    tenant: &TenantId,
    (id, principal, rp_id, label, credential, created_at, last_used_at): PasskeyRow,
) -> StoreResult<PasskeyRecord> {
    Ok(PasskeyRecord {
        tenant: tenant.clone(),
        principal: PrincipalId::new(principal).map_err(corrupt_id)?,
        id,
        rp_id,
        label,
        credential,
        created_at: Timestamp(created_at),
        last_used_at: last_used_at.map(Timestamp),
    })
}

type RecoveryRow = (String, String, String, String, i64, i64, i64);

fn recovery_from(row: &rusqlite::Row<'_>) -> rusqlite::Result<RecoveryRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

fn recovery_request(
    tenant: &TenantId,
    id_hash: &str,
    (principal, login, token_hash, cancel_hash, requested_at, completes_at, expires_at): RecoveryRow,
) -> StoreResult<RecoveryRequest> {
    Ok(RecoveryRequest {
        tenant: tenant.clone(),
        id_hash: id_hash.to_string(),
        principal: PrincipalId::new(principal).map_err(corrupt_id)?,
        login,
        token_hash,
        cancel_hash,
        requested_at: Timestamp(requested_at),
        completes_at: Timestamp(completes_at),
        expires_at: Timestamp(expires_at),
    })
}

type PendingRow = (String, String, i64, i64, i64);

fn pending_from(row: &rusqlite::Row<'_>) -> rusqlite::Result<PendingRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
    ))
}

fn pending_sign_in(
    tenant: &TenantId,
    id_hash: &str,
    (principal, method, factors, attempts, expires_at): PendingRow,
) -> StoreResult<PendingSignIn> {
    Ok(PendingSignIn {
        tenant: tenant.clone(),
        id_hash: id_hash.to_string(),
        principal: PrincipalId::new(principal).map_err(corrupt_id)?,
        method,
        factors: factors as u8,
        attempts: attempts as u32,
        expires_at: Timestamp(expires_at),
    })
}

const SESSION_COLUMNS: &str =
    "id, token_hash, csrf_token, principal, display_name, created_at, last_seen_at, expires_at, revoked_at, authenticated_at, network";

type SessionRow = (
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
    i64,
    Option<i64>,
    i64,
    Option<String>,
);

fn session_from(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
    ))
}

/// The sessions of one tenant file that meet `clause`, ordered by id.
fn sessions_where(
    conn: &Connection,
    tenant: &TenantId,
    clause: &str,
    args: &[&dyn rusqlite::ToSql],
) -> StoreResult<Vec<SessionRecord>> {
    let sql = format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE {clause} ORDER BY id");
    let mut statement = conn.prepare_cached(&sql).map_err(db)?;
    let rows = statement
        .query_map(args, session_from)
        .map_err(db)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db)?;
    rows.into_iter()
        .map(
            |(
                id,
                token_hash,
                csrf_token,
                principal,
                display_name,
                created,
                seen,
                expires,
                revoked,
                authenticated,
                network,
            )| {
                Ok(SessionRecord {
                    id: SessionId::new(id).map_err(corrupt_id)?,
                    token_hash,
                    csrf_token,
                    tenant: tenant.clone(),
                    principal: PrincipalId::new(principal).map_err(corrupt_id)?,
                    display_name,
                    created_at: Timestamp(created),
                    last_seen_at: Timestamp(seen),
                    expires_at: Timestamp(expires),
                    revoked_at: revoked.map(Timestamp),
                    authenticated_at: Timestamp(authenticated),
                    network,
                })
            },
        )
        .collect()
}

/// Whether the file at the end of `conn` holds the old single-file layout.
fn is_old_layout(conn: &Connection) -> StoreResult<bool> {
    let found: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('tenants', 'storage_migrations')",
            [],
            |r| r.get(0),
        )
        .map_err(db)?;
    Ok(found > 0)
}

/// Remove a database file and its WAL companions. A file that is not there is fine.
fn remove_files(path: &Path) -> StoreResult<()> {
    for suffix in ["", "-wal", "-shm"] {
        let file = PathBuf::from(format!("{}{suffix}", path.display()));
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(StoreError::Unavailable(format!(
                    "could not remove {}: {e}",
                    file.display()
                )))
            }
        }
    }
    Ok(())
}

impl SqliteStore {
    /// Open (creating if needed) the control database at `path`, with the tenant files in
    /// [`default_tenants_dir`], and bring the layout up to date.
    pub fn open(path: &Path, now: Timestamp) -> StoreResult<Self> {
        Self::open_with(path, &default_tenants_dir(path), now)
    }

    /// Open (creating if needed) the control database at `path`, with the tenant files in
    /// `tenants_dir`. A file in the old single-file layout is refused.
    pub fn open_with(path: &Path, tenants_dir: &Path, now: Timestamp) -> StoreResult<Self> {
        let control = Database::open(path).map_err(dbe)?;
        if is_old_layout(&*control.lock().map_err(dbe)?)? {
            return Err(StoreError::Corrupt(format!(
                "{} holds the old single-file layout, with every tenant in one file. This version \
                 keeps a control database and one database file per tenant. Development data is \
                 reseeded on start: delete {} (and its -wal and -shm files) and start again.",
                path.display(),
                path.display()
            )));
        }
        std::fs::create_dir_all(tenants_dir)
            .map_err(|e| StoreError::Unavailable(format!("{}: {e}", tenants_dir.display())))?;
        let store = Self::on(control, Place::Disk(tenants_dir.to_path_buf()), now)?;
        store.finish_deletions();
        Ok(store)
    }

    /// A private in-memory store, for tests: the control database and every tenant database in
    /// memory.
    pub fn open_in_memory(now: Timestamp) -> StoreResult<Self> {
        Self::on(Database::open_in_memory().map_err(dbe)?, Place::Memory, now)
    }

    fn on(control: Database, place: Place, now: Timestamp) -> StoreResult<Self> {
        control
            .migrate(CONTROL_LEDGER, CONTROL_MIGRATIONS, now.0)
            .map_err(dbe)?;
        Ok(SqliteStore {
            control,
            place,
            open: Mutex::new(BTreeMap::new()),
            opened_at: now,
        })
    }

    /// The directory of the tenant files, or `None` in memory.
    pub fn tenants_dir(&self) -> Option<&Path> {
        match &self.place {
            Place::Disk(dir) => Some(dir),
            Place::Memory => None,
        }
    }

    /// The database file of a tenant that has one, or `None` (no such tenant, or in memory).
    pub fn tenant_file(&self, tenant: &TenantId) -> StoreResult<Option<PathBuf>> {
        let Place::Disk(dir) = &self.place else {
            return Ok(None);
        };
        Ok(self
            .directory_file(tenant, "active")?
            .map(|file| dir.join(file)))
    }

    fn lock_open(&self) -> StoreResult<MutexGuard<'_, BTreeMap<TenantId, Database>>> {
        self.open
            .lock()
            .map_err(|_| StoreError::Unavailable("a writer panicked".into()))
    }

    /// The file a directory row in `state` names for a tenant.
    fn directory_file(&self, tenant: &TenantId, state: &str) -> StoreResult<Option<String>> {
        self.control
            .lock()
            .map_err(dbe)?
            .query_row(
                "SELECT file FROM tenant_directory WHERE id = ?1 AND state = ?2",
                params![tenant.as_str(), state],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)
    }

    /// Every tenant of the directory that is active, by id.
    fn directory(&self) -> StoreResult<Vec<TenantId>> {
        let conn = self.control.lock().map_err(dbe)?;
        select_strings(
            &conn,
            "SELECT id FROM tenant_directory WHERE state = 'active' ORDER BY id",
            &[],
        )?
        .into_iter()
        .map(|id| TenantId::new(id).map_err(corrupt_id))
        .collect()
    }

    fn open_file(&self, file: &str) -> StoreResult<Database> {
        let db = match &self.place {
            Place::Disk(dir) => Database::open(&dir.join(file)).map_err(dbe)?,
            Place::Memory => Database::open_in_memory().map_err(dbe)?,
        };
        db.migrate(TENANT_LEDGER, TENANT_MIGRATIONS, self.opened_at.0)
            .map_err(dbe)?;
        Ok(db)
    }

    /// The open database of an active tenant, or `None` when the directory has no such tenant.
    fn tenant_db(&self, tenant: &TenantId) -> StoreResult<Option<Database>> {
        let mut open = self.lock_open()?;
        if let Some(found) = open.get(tenant) {
            return Ok(Some(found.clone()));
        }
        // Another process may have created the tenant since; in memory nothing else can.
        if matches!(self.place, Place::Memory) {
            return Ok(None);
        }
        let Some(file) = self.directory_file(tenant, "active")? else {
            return Ok(None);
        };
        let found = self.open_file(&file)?;
        open.insert(tenant.clone(), found.clone());
        Ok(Some(found))
    }

    /// The database of a tenant, creating its directory row and file when there is none.
    /// Returns whether this call created it.
    fn create_tenant_db(&self, tenant: &TenantId, now: Timestamp) -> StoreResult<(Database, bool)> {
        let mut open = self.lock_open()?;
        if let Some(found) = open.get(tenant) {
            return Ok((found.clone(), false));
        }
        if let Some(file) = self.directory_file(tenant, "active")? {
            let found = self.open_file(&file)?;
            open.insert(tenant.clone(), found.clone());
            return Ok((found, false));
        }
        if self.directory_file(tenant, "deleting")?.is_some() {
            drop(open);
            self.remove_tenant(tenant)?;
            open = self.lock_open()?;
        }
        let file = tenant_file_name(tenant);
        // The directory is the authority: a file left behind without a row is stale.
        if let Place::Disk(dir) = &self.place {
            remove_files(&dir.join(&file))?;
        }
        self.control
            .write(dbe, |tx| {
                tx.execute(
                    "INSERT INTO tenant_directory (id, state, file, created_at) VALUES (?1, 'active', ?2, ?3)",
                    params![tenant.as_str(), file, now.0],
                )
                .map_err(db)
            })?;
        let created = self.open_file(&file)?;
        open.insert(tenant.clone(), created.clone());
        Ok((created, true))
    }

    /// Remove a tenant: mark it deleting, close and remove its file, then drop its directory
    /// rows (the session directory follows by cascade). A tenant that is not there is fine.
    fn remove_tenant(&self, tenant: &TenantId) -> StoreResult<()> {
        let mut open = self.lock_open()?;
        let file: Option<String> = self
            .control
            .lock()
            .map_err(dbe)?
            .query_row(
                "SELECT file FROM tenant_directory WHERE id = ?1",
                params![tenant.as_str()],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)?;
        let Some(file) = file else {
            open.remove(tenant);
            return Ok(());
        };
        self.control.write(dbe, |tx| {
            tx.execute(
                "UPDATE tenant_directory SET state = 'deleting' WHERE id = ?1",
                params![tenant.as_str()],
            )
            .map_err(db)
        })?;
        // Closing the connection lets the file go, also on Windows.
        open.remove(tenant);
        if let Place::Disk(dir) = &self.place {
            remove_files(&dir.join(&file))?;
        }
        self.control.write(dbe, |tx| {
            tx.execute(
                "DELETE FROM tenant_directory WHERE id = ?1",
                params![tenant.as_str()],
            )
            .map_err(db)
        })?;
        Ok(())
    }

    /// Finish removals a previous process could not complete. Failures leave them for later.
    fn finish_deletions(&self) {
        let pending = self.control.lock().ok().and_then(|conn| {
            select_strings(
                &conn,
                "SELECT id FROM tenant_directory WHERE state = 'deleting'",
                &[],
            )
            .ok()
        });
        for id in pending.unwrap_or_default() {
            if let Ok(tenant) = TenantId::new(id) {
                let _ = self.remove_tenant(&tenant);
            }
        }
    }

    /// The database of a tenant that exists (has its header), or `NotFound`.
    fn existing(&self, tenant: &TenantId) -> StoreResult<Database> {
        let found = self
            .tenant_db(tenant)?
            .ok_or_else(|| StoreError::NotFound(format!("tenant {tenant}")))?;
        let present = header(&*found.lock().map_err(dbe)?, tenant)?.is_some();
        if present {
            Ok(found)
        } else {
            Err(StoreError::NotFound(format!("tenant {tenant}")))
        }
    }

    /// Where a session lives, from the session directory.
    fn session_tenant(&self, clause: &str, value: &str) -> StoreResult<Option<TenantId>> {
        let sql = format!("SELECT tenant FROM session_directory WHERE {clause} = ?1");
        let tenant: Option<String> = self
            .control
            .lock()
            .map_err(dbe)?
            .query_row(&sql, params![value], |r| r.get(0))
            .optional()
            .map_err(db)?;
        tenant
            .map(|t| TenantId::new(t).map_err(corrupt_id))
            .transpose()
    }

    /// The session a directory lookup points at, read from its tenant file.
    fn session_where(&self, clause: &str, value: &str) -> StoreResult<Option<SessionRecord>> {
        let Some(tenant) = self.session_tenant(clause, value)? else {
            return Ok(None);
        };
        let Some(file) = self.tenant_db(&tenant)? else {
            return Ok(None);
        };
        let conn = file.lock().map_err(dbe)?;
        Ok(sessions_where(&conn, &tenant, &format!("{clause} = ?1"), &[&value])?.pop())
    }

    /// Run `work` on the file of a tenant, or answer `None` when the tenant has no file.
    fn on_tenant<T>(
        &self,
        tenant: &TenantId,
        work: impl FnOnce(&Connection) -> StoreResult<T>,
    ) -> StoreResult<Option<T>> {
        let Some(file) = self.tenant_db(tenant)? else {
            return Ok(None);
        };
        let conn = file.lock().map_err(dbe)?;
        work(&conn).map(Some)
    }

    /// Run `work` on the file of a tenant that exists, or answer `NotFound`.
    fn on_existing<T>(
        &self,
        tenant: &TenantId,
        work: impl FnOnce(&Connection) -> StoreResult<T>,
    ) -> StoreResult<T> {
        let file = self.existing(tenant)?;
        let conn = file.lock().map_err(dbe)?;
        work(&conn)
    }

    /// Run one statement on every tenant's file and count the rows it changed: the purges of
    /// what lapses, which are not about one tenant.
    fn on_every_tenant(&self, sql: &str, args: &[&dyn rusqlite::ToSql]) -> StoreResult<u64> {
        let mut changed = 0u64;
        for tenant in self.directory()? {
            let Some(file) = self.tenant_db(&tenant)? else {
                continue;
            };
            changed += file.lock().map_err(dbe)?.execute(sql, args).map_err(db)? as u64;
        }
        Ok(changed)
    }

    fn apply(tx: &Transaction<'_>, commit: &Commit) -> StoreResult<Receipt> {
        let delta = &commit.delta;
        let tenant = delta.tenant.as_str();
        let current = header(tx, &delta.tenant)?;
        let found = current.as_ref().map(|h| h.revision);
        if found != delta.base_revision {
            return Err(StoreError::Conflict {
                expected: delta.base_revision,
                found,
            });
        }
        check_event_sequence(commit, current.as_ref().map_or(0, |h| h.event_seq))?;

        let mut members = current
            .as_ref()
            .map(|h| h.members.clone())
            .unwrap_or_default();
        let mut members_changed = false;
        let mut schema: Option<Arc<TenantSchema>> = None;
        let mut created: Option<&str> = None;
        for op in &delta.ops {
            match op {
                DeltaOp::CreateTenant { name } => created = Some(name),
                DeltaOp::PutSchema(next) => schema = Some(next.clone()),
                DeltaOp::PutMember { principal, roles } => {
                    members_changed = true;
                    if roles.is_empty() {
                        members.remove(principal);
                    } else {
                        members.insert(principal.clone(), roles.clone());
                    }
                }
                DeltaOp::PutRecord(_) | DeltaOp::DeleteRecord { .. } => {}
            }
        }
        match (current.is_some(), created) {
            (false, Some(name)) => {
                let schema = schema
                    .clone()
                    .unwrap_or_else(|| Arc::new(TenantSchema::genesis()));
                tx.execute(
                    "INSERT INTO tenant (id, name, revision, next_record, event_seq, schema, members)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        tenant,
                        name,
                        delta.new_revision.0 as i64,
                        delta.next_record as i64,
                        delta.event_seq as i64,
                        encode(&*schema),
                        encode(&members)
                    ],
                )
                .map_err(db)?;
            }
            (true, None) => {
                tx.execute(
                    "UPDATE tenant SET revision = ?1, next_record = ?2, event_seq = ?3 WHERE id = ?4",
                    params![
                        delta.new_revision.0 as i64,
                        delta.next_record as i64,
                        delta.event_seq as i64,
                        tenant
                    ],
                )
                .map_err(db)?;
                if let Some(schema) = &schema {
                    tx.execute(
                        "UPDATE tenant SET schema = ?1 WHERE id = ?2",
                        params![encode(&**schema), tenant],
                    )
                    .map_err(db)?;
                }
                if members_changed {
                    tx.execute(
                        "UPDATE tenant SET members = ?1 WHERE id = ?2",
                        params![encode(&members), tenant],
                    )
                    .map_err(db)?;
                }
            }
            _ => {
                return Err(StoreError::Invalid(
                    "delta does not fit the tenant's existence".into(),
                ))
            }
        }
        if let Some(schema) = &schema {
            tx.execute(
                "INSERT INTO schema_history (version, revision, schema) VALUES (?1, ?2, ?3)",
                params![
                    schema.version.0 as i64,
                    delta.new_revision.0 as i64,
                    encode(&**schema)
                ],
            )
            .map_err(db)?;
        }
        for op in &delta.ops {
            match op {
                DeltaOp::PutRecord(record) => write_record(tx, record)?,
                DeltaOp::DeleteRecord { object, id } => {
                    tx.execute(
                        "DELETE FROM records WHERE object = ?1 AND id = ?2",
                        params![object.as_str(), id.0 as i64],
                    )
                    .map_err(db)?;
                }
                _ => {}
            }
        }
        let mut last_position = None;
        for event in &commit.events {
            tx.execute(
                "INSERT INTO events (seq, revision, data) VALUES (?1, ?2, ?3)",
                params![event.seq as i64, event.revision.0 as i64, encode(event)],
            )
            .map_err(db)?;
            last_position = Some(tx.last_insert_rowid() as u64);
        }
        tx.execute(
            "INSERT INTO journal (revision, data) VALUES (?1, ?2)",
            params![commit.journal.revision.0 as i64, encode(&commit.journal)],
        )
        .map_err(db)?;
        for principal in &commit.revoke_sessions {
            tx.execute(
                "UPDATE sessions SET revoked_at = ?1 WHERE principal = ?2 AND revoked_at IS NULL",
                params![commit.journal.context.now.0, principal.as_str()],
            )
            .map_err(db)?;
        }
        Ok(Receipt {
            revision: delta.new_revision,
            last_position,
        })
    }
}

impl Store for SqliteStore {
    fn capabilities(&self) -> Capabilities {
        let files = match &self.place {
            Place::Disk(dir) => format!("one database file per tenant in {}", dir.display()),
            Place::Memory => "one in-memory database per tenant".into(),
        };
        Capabilities {
            adapter: "sqlite".into(),
            durable: self.control.durable(),
            isolation: "serialisable per tenant: one writer at a time on each tenant file (BEGIN IMMEDIATE on a single connection)".into(),
            read_consistency: "a load runs in one read transaction on the tenant file and sees only whole commits".into(),
            notes: vec![
                "WAL journal with synchronous=FULL".into(),
                "field values longer than 240 bytes are not indexed; lookups for them scan the object type".into(),
                format!("control database (tenant and session directory) at {}", self.control.location()),
                files,
                "outbox positions are per tenant; the outboxes of different tenants are not ordered against each other".into(),
            ],
        }
    }

    fn health(&self) -> StoreResult<Health> {
        let quick = |conn: &Connection| -> StoreResult<String> {
            conn.query_row("PRAGMA quick_check", [], |r| r.get(0))
                .map_err(db)
        };
        let (control_check, mode) = {
            let conn = self.control.lock().map_err(dbe)?;
            let mode: String = conn
                .query_row("PRAGMA journal_mode", [], |r| r.get(0))
                .map_err(db)?;
            (quick(&conn)?, mode)
        };
        let mut ok = control_check == "ok";
        let mut detail = format!("control quick_check {control_check}, journal {mode}");
        let (mut tenants, mut pending) = (0u64, 0u64);
        for tenant in self.directory()? {
            let Some(file) = self.tenant_db(&tenant)? else {
                continue;
            };
            let conn = file.lock().map_err(dbe)?;
            let check = quick(&conn)?;
            if check != "ok" {
                ok = false;
                detail.push_str(&format!("; {tenant} quick_check {check}"));
            }
            if header(&conn, &tenant)?.is_some() {
                tenants += 1;
            }
            let waiting: i64 = conn
                .query_row("SELECT COUNT(*) FROM events WHERE published = 0", [], |r| {
                    r.get(0)
                })
                .map_err(db)?;
            pending += waiting as u64;
        }
        Ok(Health {
            ok,
            detail,
            tenants,
            outbox_pending: pending,
        })
    }

    fn migrations(&self) -> StoreResult<Vec<StorageMigration>> {
        let conn = self.control.lock().map_err(dbe)?;
        let mut statement = conn
            .prepare("SELECT version, name, applied_at FROM control_migrations ORDER BY version")
            .map_err(db)?;
        let rows = statement
            .query_map([], |row| {
                Ok(StorageMigration {
                    version: row.get(0)?,
                    name: row.get(1)?,
                    applied_at: Timestamp(row.get(2)?),
                })
            })
            .map_err(db)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db)
    }

    fn tenants(&self) -> StoreResult<Vec<TenantSummary>> {
        let mut out = Vec::new();
        for tenant in self.directory()? {
            let Some(file) = self.tenant_db(&tenant)? else {
                continue;
            };
            let conn = file.lock().map_err(dbe)?;
            let Some(state) = header(&conn, &tenant)? else {
                continue;
            };
            let records: i64 = conn
                .query_row("SELECT COUNT(*) FROM records", [], |r| r.get(0))
                .map_err(db)?;
            out.push(TenantSummary {
                id: tenant,
                name: state.name,
                revision: state.revision,
                schema_version: state.schema.version.0,
                event_seq: state.event_seq,
                records: records as u64,
            });
        }
        Ok(out)
    }

    fn load(&self, tenant: &TenantId, needs: &[Need]) -> StoreResult<State> {
        let Some(file) = self.tenant_db(tenant)? else {
            return Ok(State::absent(tenant.clone()));
        };
        file.read(dbe, |tx| {
            let Some(mut state) = header(tx, tenant)? else {
                return Ok(State::absent(tenant.clone()));
            };
            for need in needs {
                let rows = match need {
                    Need::Record { object, id } => select_strings(
                        tx,
                        "SELECT data FROM records WHERE object = ?1 AND id = ?2",
                        &[&object.as_str(), &(id.0 as i64)],
                    )?,
                    Need::AllOf(object) => all_of(tx, object)?,
                    Need::ByField {
                        object,
                        field,
                        value,
                    } => {
                        let key = encode(value);
                        if key.len() <= MAX_INDEXED {
                            select_strings(
                                tx,
                                "SELECT r.data FROM record_index i
                                 JOIN records r ON r.object = i.object AND r.id = i.id
                                 WHERE i.object = ?1 AND i.field = ?2 AND i.value = ?3",
                                &[&object.as_str(), &field.as_str(), &key],
                            )?
                        } else {
                            let mut matching = Vec::new();
                            for data in all_of(tx, object)? {
                                let record: Record = decode(&data, "record")?;
                                if field_matches(&record, field, value) {
                                    matching.push(data);
                                }
                            }
                            matching
                        }
                    }
                };
                put_records(&mut state, rows)?;
            }
            Ok(State {
                tenant_id: tenant.clone(),
                tenant: Some(Arc::new(state)),
                coverage: Coverage::Partial(needs.iter().cloned().collect()),
            })
        })
    }

    fn load_complete(&self, tenant: &TenantId) -> StoreResult<State> {
        let Some(file) = self.tenant_db(tenant)? else {
            return Ok(State::absent(tenant.clone()));
        };
        file.read(dbe, |tx| {
            Ok(match complete(tx, tenant)? {
                Some(state) => State::complete(Arc::new(state)),
                None => State::absent(tenant.clone()),
            })
        })
    }

    fn commit(&self, commit: &Commit) -> StoreResult<Receipt> {
        let delta = &commit.delta;
        let creating = delta.base_revision.is_none()
            && delta
                .ops
                .iter()
                .any(|op| matches!(op, DeltaOp::CreateTenant { .. }));
        if !creating {
            let Some(file) = self.tenant_db(&delta.tenant)? else {
                if delta.base_revision.is_some() {
                    return Err(StoreError::Conflict {
                        expected: delta.base_revision,
                        found: None,
                    });
                }
                check_event_sequence(commit, 0)?;
                return Err(StoreError::Invalid(
                    "delta does not fit the tenant's existence".into(),
                ));
            };
            return file.write(dbe, |tx| Self::apply(tx, commit));
        }
        let (file, created) = self.create_tenant_db(&delta.tenant, commit.journal.context.now)?;
        let result = file.write(dbe, |tx| Self::apply(tx, commit));
        if result.is_err() && created {
            // The file this call made holds nothing: take it away again.
            let empty = header(&*file.lock().map_err(dbe)?, &delta.tenant)?.is_none();
            drop(file);
            if empty {
                let _ = self.remove_tenant(&delta.tenant);
            }
        }
        result
    }

    fn events_after(
        &self,
        tenant: &TenantId,
        after: u64,
        limit: usize,
    ) -> StoreResult<Vec<DomainEvent>> {
        let Some(file) = self.tenant_db(tenant)? else {
            return Ok(Vec::new());
        };
        let conn = file.lock().map_err(dbe)?;
        select_strings(
            &conn,
            "SELECT data FROM events WHERE seq > ?1 ORDER BY seq LIMIT ?2",
            &[&(after as i64), &(limit as i64)],
        )?
        .iter()
        .map(|data| decode(data, "event"))
        .collect()
    }

    fn outbox(&self, limit: usize) -> StoreResult<Vec<OutboxEntry>> {
        let mut out = Vec::new();
        for tenant in self.directory()? {
            if out.len() >= limit {
                break;
            }
            let Some(file) = self.tenant_db(&tenant)? else {
                continue;
            };
            let conn = file.lock().map_err(dbe)?;
            let mut statement = conn
                .prepare_cached(
                    "SELECT position, data FROM events WHERE published = 0 ORDER BY position LIMIT ?1",
                )
                .map_err(db)?;
            let rows = statement
                .query_map(params![(limit - out.len()) as i64], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)?;
            for (position, data) in rows {
                out.push(OutboxEntry {
                    position: position as u64,
                    event: decode(&data, "event")?,
                });
            }
        }
        Ok(out)
    }

    fn mark_published(&self, tenant: &TenantId, position: u64) -> StoreResult<()> {
        let Some(file) = self.tenant_db(tenant)? else {
            return Ok(());
        };
        file.lock()
            .map_err(dbe)?
            .execute(
                "UPDATE events SET published = 1 WHERE published = 0 AND position <= ?1",
                params![position as i64],
            )
            .map_err(db)?;
        Ok(())
    }

    fn journal_after(
        &self,
        tenant: &TenantId,
        after: u64,
        limit: usize,
    ) -> StoreResult<Vec<JournalEntry>> {
        let Some(file) = self.tenant_db(tenant)? else {
            return Ok(Vec::new());
        };
        let conn = file.lock().map_err(dbe)?;
        select_strings(
            &conn,
            "SELECT data FROM journal WHERE revision > ?1 ORDER BY revision LIMIT ?2",
            &[&(after as i64), &(limit as i64)],
        )?
        .iter()
        .map(|data| decode(data, "journal entry"))
        .collect()
    }

    fn put_session(&self, session: &SessionRecord) -> StoreResult<()> {
        self.existing(&session.tenant)?;
        // The directory keeps the tenant a session was first stored under, and its token hash
        // unique across tenants.
        self.control.write(dbe, |tx| {
            tx.execute(
                "INSERT INTO session_directory (id, token_hash, tenant) VALUES (?1, ?2, ?3)
                 ON CONFLICT (id) DO UPDATE SET token_hash = excluded.token_hash",
                params![
                    session.id.as_str(),
                    session.token_hash,
                    session.tenant.as_str()
                ],
            )
            .map_err(db)
        })?;
        let tenant = self
            .session_tenant("id", session.id.as_str())?
            .ok_or_else(|| StoreError::Unavailable("the session directory lost a row".into()))?;
        let file = self.existing(&tenant)?;
        file.lock()
            .map_err(dbe)?
            .execute(
                &format!(
                    "INSERT INTO sessions ({SESSION_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                     ON CONFLICT (id) DO UPDATE SET token_hash = excluded.token_hash, csrf_token = excluded.csrf_token,
                        display_name = excluded.display_name, last_seen_at = excluded.last_seen_at,
                        expires_at = excluded.expires_at, revoked_at = excluded.revoked_at,
                        authenticated_at = excluded.authenticated_at, network = excluded.network"
                ),
                params![
                    session.id.as_str(),
                    session.token_hash,
                    session.csrf_token,
                    session.principal.as_str(),
                    session.display_name,
                    session.created_at.0,
                    session.last_seen_at.0,
                    session.expires_at.0,
                    session.revoked_at.map(|t| t.0),
                    session.authenticated_at.0,
                    session.network
                ],
            )
            .map_err(db)?;
        Ok(())
    }

    fn session_by_token(&self, token_hash: &str) -> StoreResult<Option<SessionRecord>> {
        self.session_where("token_hash", token_hash)
    }

    fn session(&self, id: &SessionId) -> StoreResult<Option<SessionRecord>> {
        self.session_where("id", id.as_str())
    }

    fn live_sessions(&self, now: Timestamp) -> StoreResult<Vec<SessionRecord>> {
        let mut out = Vec::new();
        for tenant in self.directory()? {
            let Some(file) = self.tenant_db(&tenant)? else {
                continue;
            };
            let conn = file.lock().map_err(dbe)?;
            out.extend(sessions_where(
                &conn,
                &tenant,
                "revoked_at IS NULL AND expires_at > ?1",
                &[&now.0],
            )?);
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    fn purge_sessions(&self, before: Timestamp) -> StoreResult<u64> {
        let mut removed = Vec::new();
        for tenant in self.directory()? {
            let Some(file) = self.tenant_db(&tenant)? else {
                continue;
            };
            let ids = file.write(dbe, |tx| {
                let ids = select_strings(
                    tx,
                    "SELECT id FROM sessions WHERE expires_at < ?1 OR (revoked_at IS NOT NULL AND revoked_at < ?1)",
                    &[&before.0],
                )?;
                tx.execute(
                    "DELETE FROM sessions WHERE expires_at < ?1 OR (revoked_at IS NOT NULL AND revoked_at < ?1)",
                    params![before.0],
                )
                .map_err(db)?;
                Ok(ids)
            })?;
            removed.extend(ids);
        }
        self.control.write(dbe, |tx| {
            for id in &removed {
                tx.execute("DELETE FROM session_directory WHERE id = ?1", params![id])
                    .map_err(db)?;
            }
            Ok(())
        })?;
        Ok(removed.len() as u64)
    }

    fn throttle_reserve(
        &self,
        tenant: &TenantId,
        rules: &[ThrottleRule],
        now: Timestamp,
    ) -> StoreResult<ThrottleVerdict> {
        self.existing(tenant)?.write(dbe, |tx| {
            let mut next = Vec::with_capacity(rules.len());
            let mut until: Option<i64> = None;
            for rule in rules {
                let row = throttle_row(tx, &rule.key)?;
                match throttle_step(row, rule, now.0) {
                    Ok(row) => next.push((rule.key.as_str(), row)),
                    Err(locked) => until = Some(until.map_or(locked, |u| u.max(locked))),
                }
            }
            if let Some(until) = until {
                return Ok(ThrottleVerdict::Refused {
                    until: Timestamp(until),
                });
            }
            for (key, row) in next {
                tx.execute(
                    "INSERT INTO login_throttle (key, failures, window_start, locked_until, expires_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT (key) DO UPDATE SET failures = excluded.failures,
                        window_start = excluded.window_start, locked_until = excluded.locked_until,
                        expires_at = excluded.expires_at",
                    params![
                        key,
                        i64::from(row.failures),
                        row.window_start,
                        row.locked_until,
                        row.expires_at
                    ],
                )
                .map_err(db)?;
            }
            Ok(ThrottleVerdict::Allowed)
        })
    }

    fn throttle_settle(
        &self,
        tenant: &TenantId,
        key: &str,
        max: u32,
        how: ThrottleSettle,
    ) -> StoreResult<()> {
        self.existing(tenant)?.write(dbe, |tx| {
            match how {
                ThrottleSettle::Clear => {
                    tx.execute("DELETE FROM login_throttle WHERE key = ?1", params![key])
                        .map_err(db)?;
                }
                ThrottleSettle::Refund => {
                    if let Some(row) = throttle_row(tx, key)? {
                        let row = throttle_refund(row, max);
                        tx.execute(
                            "UPDATE login_throttle SET failures = ?2, locked_until = ?3 WHERE key = ?1",
                            params![key, i64::from(row.failures), row.locked_until],
                        )
                        .map_err(db)?;
                    }
                }
            }
            Ok(())
        })
    }

    fn purge_throttle(&self, now: Timestamp) -> StoreResult<u64> {
        self.on_every_tenant(
            "DELETE FROM login_throttle WHERE expires_at <= ?1",
            &[&now.0],
        )
    }

    fn account_of(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<Option<Account>> {
        let row = self.on_tenant(tenant, |conn| {
            conn.query_row(
                "SELECT login, display_name, password_hash FROM accounts WHERE principal = ?1 ORDER BY login LIMIT 1",
                params![principal.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(db)
        })?;
        Ok(row
            .flatten()
            .map(|(login, display_name, password_hash)| Account {
                tenant: tenant.clone(),
                login,
                principal: principal.clone(),
                display_name,
                password_hash,
            }))
    }

    fn put_passkey(&self, passkey: &PasskeyRecord) -> StoreResult<bool> {
        let added = self.on_existing(&passkey.tenant, |conn| {
            conn.execute(
                &format!(
                    "INSERT OR IGNORE INTO passkeys ({PASSKEY_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
                ),
                params![
                    passkey.id,
                    passkey.principal.as_str(),
                    passkey.rp_id,
                    passkey.label,
                    passkey.credential,
                    passkey.created_at.0,
                    passkey.last_used_at.map(|at| at.0),
                ],
            )
            .map_err(db)
        })?;
        Ok(added == 1)
    }

    fn passkey(&self, tenant: &TenantId, id: &str) -> StoreResult<Option<PasskeyRecord>> {
        let row = self.on_tenant(tenant, |conn| {
            conn.query_row(
                &format!("SELECT {PASSKEY_COLUMNS} FROM passkeys WHERE id = ?1"),
                params![id],
                passkey_from,
            )
            .optional()
            .map_err(db)
        })?;
        row.flatten()
            .map(|row| passkey_record(tenant, row))
            .transpose()
    }

    fn passkeys(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<Vec<PasskeyRecord>> {
        let rows = self.on_tenant(tenant, |conn| {
            let mut statement = conn
                .prepare(&format!(
                    "SELECT {PASSKEY_COLUMNS} FROM passkeys WHERE principal = ?1 ORDER BY created_at, id"
                ))
                .map_err(db)?;
            let rows = statement
                .query_map(params![principal.as_str()], passkey_from)
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)?;
            Ok(rows)
        })?;
        rows.unwrap_or_default()
            .into_iter()
            .map(|row| passkey_record(tenant, row))
            .collect()
    }

    fn update_passkey(
        &self,
        tenant: &TenantId,
        id: &str,
        credential: &str,
        used_at: Timestamp,
    ) -> StoreResult<bool> {
        let changed = self.on_tenant(tenant, |conn| {
            conn.execute(
                "UPDATE passkeys SET credential = ?2, last_used_at = ?3 WHERE id = ?1",
                params![id, credential, used_at.0],
            )
            .map_err(db)
        })?;
        Ok(changed == Some(1))
    }

    fn remove_passkey(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        id: &str,
    ) -> StoreResult<bool> {
        let removed = self.on_tenant(tenant, |conn| {
            conn.execute(
                "DELETE FROM passkeys WHERE principal = ?1 AND id = ?2",
                params![principal.as_str(), id],
            )
            .map_err(db)
        })?;
        Ok(removed == Some(1))
    }

    fn put_passkey_challenge(&self, challenge: &PasskeyChallenge) -> StoreResult<()> {
        self.on_existing(&challenge.tenant, |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO passkey_challenges (id_hash, ceremony, session, principal, origin, state, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    challenge.id_hash,
                    match challenge.ceremony {
                        PasskeyCeremony::Register => "register",
                        PasskeyCeremony::SignIn => "sign-in",
                    },
                    challenge.session.as_ref().map(|s| s.as_str().to_string()),
                    challenge.principal.as_ref().map(|p| p.as_str().to_string()),
                    challenge.origin,
                    challenge.state,
                    challenge.expires_at.0,
                ],
            )
            .map_err(db)
        })?;
        Ok(())
    }

    fn take_passkey_challenge(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        now: Timestamp,
    ) -> StoreResult<Option<PasskeyChallenge>> {
        // One statement reads and removes the row, so two answers to one challenge cannot both
        // take it.
        let row = self.on_tenant(tenant, |conn| {
            conn.query_row(
                "DELETE FROM passkey_challenges WHERE id_hash = ?1
                 RETURNING ceremony, session, principal, origin, state, expires_at",
                params![id_hash],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, i64>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(db)
        })?;
        let Some((ceremony, session, principal, origin, state, expires_at)) = row.flatten() else {
            return Ok(None);
        };
        if expires_at <= now.0 {
            return Ok(None);
        }
        Ok(Some(PasskeyChallenge {
            tenant: tenant.clone(),
            id_hash: id_hash.to_string(),
            ceremony: if ceremony == "register" {
                PasskeyCeremony::Register
            } else {
                PasskeyCeremony::SignIn
            },
            session: session
                .map(SessionId::new)
                .transpose()
                .map_err(corrupt_id)?,
            principal: principal
                .map(PrincipalId::new)
                .transpose()
                .map_err(corrupt_id)?,
            origin,
            state,
            expires_at: Timestamp(expires_at),
        }))
    }

    fn purge_passkey_challenges(&self, now: Timestamp) -> StoreResult<u64> {
        self.on_every_tenant(
            "DELETE FROM passkey_challenges WHERE expires_at <= ?1",
            &[&now.0],
        )
    }

    fn put_sign_in_link(&self, link: &SignInLink) -> StoreResult<()> {
        self.on_existing(&link.tenant, |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO sign_in_links (id_hash, principal, token_hash, binding_hash, origin, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    link.id_hash,
                    link.principal.as_str(),
                    link.token_hash,
                    link.binding_hash,
                    link.origin,
                    link.expires_at.0,
                ],
            )
            .map_err(db)
        })?;
        Ok(())
    }

    fn take_sign_in_link(
        &self,
        tenant: &TenantId,
        id_hash: &str,
    ) -> StoreResult<Option<SignInLink>> {
        // One statement reads and removes the row, so two uses of one link cannot both take it.
        let row = self.on_tenant(tenant, |conn| {
            conn.query_row(
                "DELETE FROM sign_in_links WHERE id_hash = ?1
                 RETURNING principal, token_hash, binding_hash, origin, expires_at",
                params![id_hash],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(db)
        })?;
        let Some((principal, token_hash, binding_hash, origin, expires_at)) = row.flatten() else {
            return Ok(None);
        };
        Ok(Some(SignInLink {
            tenant: tenant.clone(),
            id_hash: id_hash.to_string(),
            principal: PrincipalId::new(principal).map_err(corrupt_id)?,
            token_hash,
            binding_hash,
            origin,
            expires_at: Timestamp(expires_at),
        }))
    }

    fn purge_sign_in_links(&self, now: Timestamp) -> StoreResult<u64> {
        self.on_every_tenant("DELETE FROM sign_in_links WHERE expires_at < ?1", &[&now.0])
    }

    fn put_oidc_flow(&self, flow: &OidcFlow) -> StoreResult<()> {
        self.on_existing(&flow.tenant, |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO oidc_flows (id_hash, provider, nonce, verifier, binding_hash, origin, principal, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    flow.id_hash,
                    flow.provider,
                    flow.nonce,
                    flow.verifier,
                    flow.binding_hash,
                    flow.origin,
                    flow.principal.as_ref().map(|p| p.as_str()),
                    flow.expires_at.0,
                ],
            )
            .map_err(db)
        })?;
        Ok(())
    }

    fn take_oidc_flow(&self, tenant: &TenantId, id_hash: &str) -> StoreResult<Option<OidcFlow>> {
        // One statement reads and removes the row, so two callbacks cannot both take it.
        let row = self.on_tenant(tenant, |conn| {
            conn.query_row(
                "DELETE FROM oidc_flows WHERE id_hash = ?1
                 RETURNING provider, nonce, verifier, binding_hash, origin, principal, expires_at",
                params![id_hash],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, i64>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(db)
        })?;
        let Some((provider, nonce, verifier, binding_hash, origin, principal, expires_at)) =
            row.flatten()
        else {
            return Ok(None);
        };
        Ok(Some(OidcFlow {
            tenant: tenant.clone(),
            id_hash: id_hash.to_string(),
            provider,
            nonce,
            verifier,
            binding_hash,
            origin,
            principal: principal
                .map(PrincipalId::new)
                .transpose()
                .map_err(corrupt_id)?,
            expires_at: Timestamp(expires_at),
        }))
    }

    fn purge_oidc_flows(&self, now: Timestamp) -> StoreResult<u64> {
        self.on_every_tenant("DELETE FROM oidc_flows WHERE expires_at < ?1", &[&now.0])
    }

    fn put_saml_request(&self, request: &SamlRequest) -> StoreResult<()> {
        self.on_existing(&request.tenant, |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO saml_requests (id_hash, provider, request_id, binding_hash, origin, principal, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    request.id_hash,
                    request.provider,
                    request.request_id,
                    request.binding_hash,
                    request.origin,
                    request.principal.as_ref().map(|p| p.as_str()),
                    request.expires_at.0,
                ],
            )
            .map_err(db)
        })?;
        Ok(())
    }

    fn take_saml_request(
        &self,
        tenant: &TenantId,
        id_hash: &str,
    ) -> StoreResult<Option<SamlRequest>> {
        // One statement reads and removes the row, so two responses cannot both take it.
        let row = self.on_tenant(tenant, |conn| {
            conn.query_row(
                "DELETE FROM saml_requests WHERE id_hash = ?1
                 RETURNING provider, request_id, binding_hash, origin, principal, expires_at",
                params![id_hash],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, i64>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(db)
        })?;
        let Some((provider, request_id, binding_hash, origin, principal, expires_at)) =
            row.flatten()
        else {
            return Ok(None);
        };
        Ok(Some(SamlRequest {
            tenant: tenant.clone(),
            id_hash: id_hash.to_string(),
            provider,
            request_id,
            binding_hash,
            origin,
            principal: principal
                .map(PrincipalId::new)
                .transpose()
                .map_err(corrupt_id)?,
            expires_at: Timestamp(expires_at),
        }))
    }

    fn purge_saml_requests(&self, now: Timestamp) -> StoreResult<u64> {
        self.on_every_tenant("DELETE FROM saml_requests WHERE expires_at < ?1", &[&now.0])
    }

    fn put_saml_assertion(
        &self,
        tenant: &TenantId,
        provider: &str,
        id_hash: &str,
        expires_at: Timestamp,
    ) -> StoreResult<bool> {
        // One statement, so that two presentations at once are one: the second changes no row.
        let added = self.on_existing(tenant, |conn| {
            conn.execute(
                "INSERT OR IGNORE INTO saml_assertions (provider, id_hash, expires_at)
                 VALUES (?1, ?2, ?3)",
                params![provider, id_hash, expires_at.0],
            )
            .map_err(db)
        })?;
        Ok(added == 1)
    }

    fn purge_saml_assertions(&self, now: Timestamp) -> StoreResult<u64> {
        self.on_every_tenant(
            "DELETE FROM saml_assertions WHERE expires_at < ?1",
            &[&now.0],
        )
    }

    fn put_identity(&self, identity: &FederatedIdentity) -> StoreResult<bool> {
        let added = self.on_existing(&identity.tenant, |conn| {
            conn.execute(
                "INSERT OR IGNORE INTO federated_identities (provider, subject, principal, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    identity.provider,
                    identity.subject,
                    identity.principal.as_str(),
                    identity.created_at.0,
                ],
            )
            .map_err(db)
        })?;
        Ok(added == 1)
    }

    fn identity(
        &self,
        tenant: &TenantId,
        provider: &str,
        subject: &str,
    ) -> StoreResult<Option<FederatedIdentity>> {
        let row = self.on_tenant(tenant, |conn| {
            conn.query_row(
                "SELECT principal, created_at FROM federated_identities
                 WHERE provider = ?1 AND subject = ?2",
                params![provider, subject],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(db)
        })?;
        let Some((principal, created_at)) = row.flatten() else {
            return Ok(None);
        };
        Ok(Some(FederatedIdentity {
            tenant: tenant.clone(),
            provider: provider.to_string(),
            subject: subject.to_string(),
            principal: PrincipalId::new(principal).map_err(corrupt_id)?,
            created_at: Timestamp(created_at),
        }))
    }

    fn put_totp(&self, totp: &TotpRecord) -> StoreResult<bool> {
        // An unconfirmed enrolment is replaced; a confirmed one is not.
        let changed = self.on_existing(&totp.tenant, |conn| {
            conn.execute(
                "INSERT INTO totp_factors (principal, secret, confirmed, last_step, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (principal) DO UPDATE SET secret = excluded.secret,
                    confirmed = excluded.confirmed, last_step = excluded.last_step,
                    created_at = excluded.created_at
                 WHERE totp_factors.confirmed = 0",
                params![
                    totp.principal.as_str(),
                    totp.secret,
                    totp.confirmed,
                    totp.last_step,
                    totp.created_at.0,
                ],
            )
            .map_err(db)
        })?;
        Ok(changed == 1)
    }

    fn totp(&self, tenant: &TenantId, principal: &PrincipalId) -> StoreResult<Option<TotpRecord>> {
        let row = self.on_tenant(tenant, |conn| {
            conn.query_row(
                "SELECT secret, confirmed, last_step, created_at FROM totp_factors WHERE principal = ?1",
                params![principal.as_str()],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, bool>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(db)
        })?;
        Ok(row
            .flatten()
            .map(|(secret, confirmed, last_step, created_at)| TotpRecord {
                tenant: tenant.clone(),
                principal: principal.clone(),
                secret,
                confirmed,
                last_step,
                created_at: Timestamp(created_at),
            }))
    }

    fn confirm_totp(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        step: i64,
    ) -> StoreResult<bool> {
        let changed = self.on_tenant(tenant, |conn| {
            conn.execute(
                "UPDATE totp_factors SET confirmed = 1, last_step = ?2
                 WHERE principal = ?1 AND confirmed = 0",
                params![principal.as_str(), step],
            )
            .map_err(db)
        })?;
        Ok(changed == Some(1))
    }

    fn use_totp_step(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        step: i64,
    ) -> StoreResult<bool> {
        // One statement checks the step and remembers it, so two uses of one code cannot both win.
        let changed = self.on_tenant(tenant, |conn| {
            conn.execute(
                "UPDATE totp_factors SET last_step = ?2
                 WHERE principal = ?1 AND confirmed = 1 AND last_step < ?2",
                params![principal.as_str(), step],
            )
            .map_err(db)
        })?;
        Ok(changed == Some(1))
    }

    fn remove_totp(&self, tenant: &TenantId, principal: &PrincipalId) -> StoreResult<bool> {
        let removed = self.on_tenant(tenant, |conn| {
            conn.execute(
                "DELETE FROM totp_factors WHERE principal = ?1",
                params![principal.as_str()],
            )
            .map_err(db)
        })?;
        Ok(removed == Some(1))
    }

    fn replace_recovery_codes(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        hashes: &[String],
        at: Timestamp,
    ) -> StoreResult<()> {
        self.existing(tenant)?.write(dbe, |tx| {
            tx.execute(
                "DELETE FROM recovery_codes WHERE principal = ?1",
                params![principal.as_str()],
            )
            .map_err(db)?;
            for hash in hashes {
                tx.execute(
                    "INSERT OR IGNORE INTO recovery_codes (principal, code_hash, created_at)
                     VALUES (?1, ?2, ?3)",
                    params![principal.as_str(), hash, at.0],
                )
                .map_err(db)?;
            }
            Ok(())
        })
    }

    fn spend_recovery_code(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        hash: &str,
    ) -> StoreResult<bool> {
        // One statement removes the row, so two presentations of one code cannot both win.
        let removed = self.on_tenant(tenant, |conn| {
            conn.execute(
                "DELETE FROM recovery_codes WHERE principal = ?1 AND code_hash = ?2",
                params![principal.as_str(), hash],
            )
            .map_err(db)
        })?;
        Ok(removed == Some(1))
    }

    fn recovery_codes_left(&self, tenant: &TenantId, principal: &PrincipalId) -> StoreResult<u32> {
        let left = self.on_tenant(tenant, |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM recovery_codes WHERE principal = ?1",
                params![principal.as_str()],
                |r| r.get::<_, i64>(0),
            )
            .map_err(db)
        })?;
        Ok(left.unwrap_or(0) as u32)
    }

    fn put_pending(&self, pending: &PendingSignIn) -> StoreResult<()> {
        self.on_existing(&pending.tenant, |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO pending_sign_ins (id_hash, principal, method, factors, attempts, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    pending.id_hash,
                    pending.principal.as_str(),
                    pending.method,
                    i64::from(pending.factors),
                    i64::from(pending.attempts),
                    pending.expires_at.0,
                ],
            )
            .map_err(db)
        })?;
        Ok(())
    }

    fn attempt_pending(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        now: Timestamp,
        max: u32,
    ) -> StoreResult<Option<PendingSignIn>> {
        let Some(file) = self.tenant_db(tenant)? else {
            return Ok(None);
        };
        file.write(dbe, |tx| {
            let row = tx
                .query_row(
                    "SELECT principal, method, factors, attempts, expires_at FROM pending_sign_ins
                     WHERE id_hash = ?1",
                    params![id_hash],
                    pending_from,
                )
                .optional()
                .map_err(db)?;
            let Some((principal, method, factors, attempts, expires_at)) = row else {
                return Ok(None);
            };
            if expires_at <= now.0 || attempts >= i64::from(max) {
                tx.execute(
                    "DELETE FROM pending_sign_ins WHERE id_hash = ?1",
                    params![id_hash],
                )
                .map_err(db)?;
                return Ok(None);
            }
            tx.execute(
                "UPDATE pending_sign_ins SET attempts = attempts + 1 WHERE id_hash = ?1",
                params![id_hash],
            )
            .map_err(db)?;
            pending_sign_in(
                tenant,
                id_hash,
                (principal, method, factors, attempts + 1, expires_at),
            )
            .map(Some)
        })
    }

    fn take_pending(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        now: Timestamp,
    ) -> StoreResult<Option<PendingSignIn>> {
        // One statement reads and removes the row, so two answers cannot both take it.
        let row = self.on_tenant(tenant, |conn| {
            conn.query_row(
                "DELETE FROM pending_sign_ins WHERE id_hash = ?1
                 RETURNING principal, method, factors, attempts, expires_at",
                params![id_hash],
                pending_from,
            )
            .optional()
            .map_err(db)
        })?;
        let Some(row) = row.flatten() else {
            return Ok(None);
        };
        if row.4 <= now.0 {
            return Ok(None);
        }
        pending_sign_in(tenant, id_hash, row).map(Some)
    }

    fn put_recovery(&self, request: &RecoveryRequest) -> StoreResult<()> {
        self.existing(&request.tenant)?.write(dbe, |tx| {
            tx.execute(
                "DELETE FROM recovery_requests WHERE principal = ?1",
                params![request.principal.as_str()],
            )
            .map_err(db)?;
            tx.execute(
                "INSERT OR REPLACE INTO recovery_requests
                    (id_hash, principal, login, token_hash, cancel_hash, requested_at, completes_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    request.id_hash,
                    request.principal.as_str(),
                    request.login,
                    request.token_hash,
                    request.cancel_hash,
                    request.requested_at.0,
                    request.completes_at.0,
                    request.expires_at.0,
                ],
            )
            .map_err(db)?;
            Ok(())
        })
    }

    fn recovery(&self, tenant: &TenantId, id_hash: &str) -> StoreResult<Option<RecoveryRequest>> {
        let row = self.on_tenant(tenant, |conn| {
            conn.query_row(
                "SELECT principal, login, token_hash, cancel_hash, requested_at, completes_at, expires_at
                 FROM recovery_requests WHERE id_hash = ?1",
                params![id_hash],
                recovery_from,
            )
            .optional()
            .map_err(db)
        })?;
        row.flatten()
            .map(|row| recovery_request(tenant, id_hash, row))
            .transpose()
    }

    fn take_recovery(
        &self,
        tenant: &TenantId,
        id_hash: &str,
    ) -> StoreResult<Option<RecoveryRequest>> {
        let row = self.on_tenant(tenant, |conn| {
            conn.query_row(
                "DELETE FROM recovery_requests WHERE id_hash = ?1
                 RETURNING principal, login, token_hash, cancel_hash, requested_at, completes_at, expires_at",
                params![id_hash],
                recovery_from,
            )
            .optional()
            .map_err(db)
        })?;
        row.flatten()
            .map(|row| recovery_request(tenant, id_hash, row))
            .transpose()
    }

    fn cancel_recovery(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        cancel_hash: &str,
    ) -> StoreResult<bool> {
        let removed = self.on_tenant(tenant, |conn| {
            conn.execute(
                "DELETE FROM recovery_requests WHERE id_hash = ?1 AND cancel_hash = ?2",
                params![id_hash, cancel_hash],
            )
            .map_err(db)
        })?;
        Ok(removed == Some(1))
    }

    fn purge_factor_challenges(&self, now: Timestamp) -> StoreResult<u64> {
        let pending = self.on_every_tenant(
            "DELETE FROM pending_sign_ins WHERE expires_at <= ?1",
            &[&now.0],
        )?;
        let recoveries = self.on_every_tenant(
            "DELETE FROM recovery_requests WHERE expires_at <= ?1",
            &[&now.0],
        )?;
        Ok(pending + recoveries)
    }

    fn put_account(&self, account: &Account) -> StoreResult<()> {
        let file = self.existing(&account.tenant)?;
        file.lock()
            .map_err(dbe)?
            .execute(
                "INSERT INTO accounts (login, principal, display_name, password_hash) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (login) DO UPDATE SET principal = excluded.principal,
                    display_name = excluded.display_name, password_hash = excluded.password_hash",
                params![
                    account.login,
                    account.principal.as_str(),
                    account.display_name,
                    account.password_hash
                ],
            )
            .map_err(db)?;
        Ok(())
    }

    fn account(&self, tenant: &TenantId, login: &str) -> StoreResult<Option<Account>> {
        let Some(file) = self.tenant_db(tenant)? else {
            return Ok(None);
        };
        let conn = file.lock().map_err(dbe)?;
        let row = conn
            .query_row(
                "SELECT principal, display_name, password_hash FROM accounts WHERE login = ?1",
                params![login],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(db)?;
        row.map(|(principal, display_name, password_hash)| {
            Ok(Account {
                tenant: tenant.clone(),
                login: login.to_string(),
                principal: PrincipalId::new(principal).map_err(corrupt_id)?,
                display_name,
                password_hash,
            })
        })
        .transpose()
    }

    fn remove_account(&self, tenant: &TenantId, login: &str) -> StoreResult<bool> {
        let Some(file) = self.tenant_db(tenant)? else {
            return Ok(false);
        };
        let removed = file
            .lock()
            .map_err(dbe)?
            .execute("DELETE FROM accounts WHERE login = ?1", params![login])
            .map_err(db)?;
        Ok(removed > 0)
    }

    fn create_snapshot(
        &self,
        tenant: &TenantId,
        label: &str,
        at: Timestamp,
    ) -> StoreResult<SnapshotInfo> {
        let file = self.existing(tenant)?;
        file.write(dbe, |tx| {
            let state = complete(tx, tenant)?
                .ok_or_else(|| StoreError::NotFound(format!("tenant {tenant}")))?;
            let id: i64 = tx
                .query_row("SELECT COALESCE(MAX(id), 0) + 1 FROM snapshots", [], |r| {
                    r.get(0)
                })
                .map_err(db)?;
            let info = SnapshotInfo {
                tenant: tenant.clone(),
                id: id as u64,
                label: label.to_string(),
                revision: state.revision,
                hash: canon::hash_of(&state),
                created_at: at,
            };
            tx.execute(
                "INSERT INTO snapshots (id, label, revision, hash, created_at, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, label, state.revision.0 as i64, info.hash, at.0, encode(&state)],
            )
            .map_err(db)?;
            Ok(info)
        })
    }

    fn snapshots(&self, tenant: &TenantId) -> StoreResult<Vec<SnapshotInfo>> {
        let Some(file) = self.tenant_db(tenant)? else {
            return Ok(Vec::new());
        };
        let conn = file.lock().map_err(dbe)?;
        let mut statement = conn
            .prepare("SELECT id, label, revision, hash, created_at FROM snapshots ORDER BY id")
            .map_err(db)?;
        let rows = statement
            .query_map([], |row| {
                Ok(SnapshotInfo {
                    tenant: tenant.clone(),
                    id: row.get::<_, i64>(0)? as u64,
                    label: row.get(1)?,
                    revision: StateRevision(row.get::<_, i64>(2)? as u64),
                    hash: row.get(3)?,
                    created_at: Timestamp(row.get(4)?),
                })
            })
            .map_err(db)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db)
    }

    fn load_snapshot(&self, tenant: &TenantId, id: u64) -> StoreResult<TenantState> {
        let missing = || StoreError::NotFound(format!("snapshot {id} of {tenant}"));
        let file = self.tenant_db(tenant)?.ok_or_else(missing)?;
        let data: Option<String> = file
            .lock()
            .map_err(dbe)?
            .query_row(
                "SELECT data FROM snapshots WHERE id = ?1",
                params![id as i64],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)?;
        decode(&data.ok_or_else(missing)?, "snapshot")
    }

    fn restore_snapshot(&self, tenant: &TenantId, id: u64) -> StoreResult<()> {
        let state = self.load_snapshot(tenant, id)?;
        let file = self.existing(tenant)?;
        file.write(dbe, |tx| {
            tx.execute("DELETE FROM records", []).map_err(db)?;
            tx.execute(
                "UPDATE tenant SET name = ?1, revision = ?2, next_record = ?3, event_seq = ?4, schema = ?5, members = ?6
                 WHERE id = ?7",
                params![
                    state.name,
                    state.revision.0 as i64,
                    state.next_record as i64,
                    state.event_seq as i64,
                    encode(&*state.schema),
                    encode(&state.members),
                    tenant.as_str()
                ],
            )
            .map_err(db)?;
            for record in state.records.values().flat_map(|records| records.values()) {
                write_record(tx, record)?;
            }
            tx.execute(
                "DELETE FROM events WHERE seq > ?1",
                params![state.event_seq as i64],
            )
            .map_err(db)?;
            tx.execute(
                "DELETE FROM journal WHERE revision > ?1",
                params![state.revision.0 as i64],
            )
            .map_err(db)?;
            tx.execute(
                "DELETE FROM schema_history WHERE version > ?1",
                params![state.schema.version.0 as i64],
            )
            .map_err(db)?;
            Ok(())
        })
    }

    fn reset(&self) -> StoreResult<()> {
        let all: Vec<String> = {
            let conn = self.control.lock().map_err(dbe)?;
            select_strings(&conn, "SELECT id FROM tenant_directory ORDER BY id", &[])?
        };
        for id in all {
            self.remove_tenant(&TenantId::new(id).map_err(corrupt_id)?)?;
        }
        self.lock_open()?.clear();
        Ok(())
    }

    fn reset_tenant(&self, tenant: &TenantId) -> StoreResult<()> {
        self.remove_tenant(tenant)
    }
}
