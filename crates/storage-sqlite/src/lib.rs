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
//!   sessions and logins.
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
use strata_kernel::ext::*;
use strata_kernel::{
    canon, Coverage, DeltaOp, DomainEvent, Need, ObjectTypeId, PrincipalId, Record, RoleId,
    SessionId, State, StateRevision, TenantId, TenantSchema, TenantState, Timestamp, Value,
};
use strata_runtime::sqlite::{Database, DbError, Migration};
use strata_storage_api::{
    check_event_sequence, field_matches, Account, Capabilities, Commit, Health, JournalEntry,
    OutboxEntry, Receipt, SessionRecord, SnapshotInfo, StorageMigration, Store, StoreError,
    StoreResult, TenantSummary,
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
const TENANT_MIGRATIONS: &[Migration] = &[Migration {
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
}];

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

const SESSION_COLUMNS: &str =
    "id, token_hash, csrf_token, principal, display_name, created_at, last_seen_at, expires_at, revoked_at";

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
                    "INSERT INTO sessions ({SESSION_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                     ON CONFLICT (id) DO UPDATE SET token_hash = excluded.token_hash, csrf_token = excluded.csrf_token,
                        display_name = excluded.display_name, last_seen_at = excluded.last_seen_at,
                        expires_at = excluded.expires_at, revoked_at = excluded.revoked_at"
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
                    session.revoked_at.map(|t| t.0)
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
