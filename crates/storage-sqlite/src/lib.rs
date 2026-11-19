//! The SQLite adapter. Every SQL statement and table of the tenant store lives in this file.
//!
//! Layout: tenant state is one header row per tenant (`tenants`) plus one row per record
//! (`records`), each record stored as its canonical JSON document. `record_index` holds one row
//! per scalar field value (and per list element) so that uniqueness and reference lookups do not
//! scan. Events double as the outbox through their `published` flag, written in the same
//! transaction as the state they describe.
//!
//! Concurrency: one connection behind a mutex, write transactions opened `IMMEDIATE`, WAL
//! journal, `synchronous = FULL`, foreign keys on, a busy timeout for other processes that have
//! the file open. The connection handling and the migration runner are the runtime's
//! ([`strata_runtime::sqlite::Database`]); the tables and statements of the tenant store are
//! here. [`SqliteStore::database`] hands the same database to the runtime's machine store, so
//! machine records, their outbox and their timers can live in the same file.

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, MutexGuard};
use strata_kernel::{
    canon, Coverage, DeltaOp, DomainEvent, Need, ObjectTypeId, PrincipalId, Record, RoleId,
    SessionId, State, StateRevision, TenantId, TenantSchema, TenantState, Timestamp, Value,
};
use strata_runtime::sqlite::{Database, DbError, Migration};
use strata_storage_api::{
    check_event_sequence, field_matches, throttle_refund, throttle_step, Account, Capabilities,
    Commit, Health, JournalEntry, OutboxEntry, PasskeyCeremony, PasskeyChallenge, PasskeyRecord,
    Receipt, SessionRecord, SnapshotInfo, StorageMigration, Store, StoreError, StoreResult,
    TenantSummary, ThrottleRow, ThrottleRule, ThrottleSettle, ThrottleVerdict,
};

/// The ledger table of the tenant store's migrations.
const LEDGER: &str = "storage_migrations";

/// The storage layout, one step per entry. Steps are only ever appended.
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial layout",
        sql: "
    CREATE TABLE tenants (
        id          TEXT PRIMARY KEY,
        name        TEXT NOT NULL,
        revision    INTEGER NOT NULL,
        next_record INTEGER NOT NULL,
        event_seq   INTEGER NOT NULL,
        schema      TEXT NOT NULL,
        members     TEXT NOT NULL
    );
    CREATE TABLE schema_history (
        tenant   TEXT NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
        version  INTEGER NOT NULL,
        revision INTEGER NOT NULL,
        schema   TEXT NOT NULL,
        PRIMARY KEY (tenant, version)
    );
    CREATE TABLE records (
        tenant  TEXT NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
        object  TEXT NOT NULL,
        id      INTEGER NOT NULL,
        version INTEGER NOT NULL,
        data    TEXT NOT NULL,
        PRIMARY KEY (tenant, object, id)
    );
    CREATE TABLE record_index (
        tenant TEXT NOT NULL,
        object TEXT NOT NULL,
        field  TEXT NOT NULL,
        value  TEXT NOT NULL,
        id     INTEGER NOT NULL,
        PRIMARY KEY (tenant, object, field, value, id),
        FOREIGN KEY (tenant, object, id) REFERENCES records(tenant, object, id) ON DELETE CASCADE
    );
    CREATE TABLE events (
        position  INTEGER PRIMARY KEY AUTOINCREMENT,
        tenant    TEXT NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
        seq       INTEGER NOT NULL,
        revision  INTEGER NOT NULL,
        data      TEXT NOT NULL,
        published INTEGER NOT NULL DEFAULT 0,
        UNIQUE (tenant, seq)
    );
    CREATE INDEX events_outbox ON events(position) WHERE published = 0;
    CREATE TABLE journal (
        tenant   TEXT NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
        revision INTEGER NOT NULL,
        data     TEXT NOT NULL,
        PRIMARY KEY (tenant, revision)
    );
    CREATE TABLE sessions (
        id           TEXT PRIMARY KEY,
        token_hash   TEXT NOT NULL UNIQUE,
        csrf_token   TEXT NOT NULL,
        tenant       TEXT NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
        principal    TEXT NOT NULL,
        display_name TEXT NOT NULL,
        created_at   INTEGER NOT NULL,
        last_seen_at INTEGER NOT NULL,
        expires_at   INTEGER NOT NULL,
        revoked_at   INTEGER
    );
    CREATE INDEX sessions_principal ON sessions(tenant, principal);
    CREATE TABLE accounts (
        tenant        TEXT NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
        login         TEXT NOT NULL,
        principal     TEXT NOT NULL,
        display_name  TEXT NOT NULL,
        password_hash TEXT NOT NULL,
        PRIMARY KEY (tenant, login)
    );
    CREATE TABLE snapshots (
        tenant     TEXT NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
        id         INTEGER NOT NULL,
        label      TEXT NOT NULL,
        revision   INTEGER NOT NULL,
        hash       TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        data       TEXT NOT NULL,
        PRIMARY KEY (tenant, id)
    );
    ",
    },
    Migration {
        version: 2,
        name: "login throttle",
        sql: "
    CREATE TABLE login_throttle (
        tenant       TEXT NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
        key          TEXT NOT NULL,
        failures     INTEGER NOT NULL,
        window_start INTEGER NOT NULL,
        locked_until INTEGER NOT NULL,
        expires_at   INTEGER NOT NULL,
        PRIMARY KEY (tenant, key)
    );
    CREATE INDEX login_throttle_expiry ON login_throttle(expires_at);
    ",
    },
    Migration {
        version: 3,
        name: "passkeys",
        sql: "
    CREATE TABLE passkeys (
        tenant       TEXT NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
        id           TEXT NOT NULL,
        principal    TEXT NOT NULL,
        rp_id        TEXT NOT NULL,
        label        TEXT NOT NULL,
        credential   TEXT NOT NULL,
        created_at   INTEGER NOT NULL,
        last_used_at INTEGER,
        PRIMARY KEY (tenant, id)
    );
    CREATE INDEX passkeys_principal ON passkeys(tenant, principal, created_at);
    CREATE TABLE passkey_challenges (
        tenant     TEXT NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
        id_hash    TEXT NOT NULL,
        ceremony   TEXT NOT NULL,
        session    TEXT,
        principal  TEXT,
        origin     TEXT NOT NULL,
        state      TEXT NOT NULL,
        expires_at INTEGER NOT NULL,
        PRIMARY KEY (tenant, id_hash)
    );
    CREATE INDEX passkey_challenges_expiry ON passkey_challenges(expires_at);
    CREATE INDEX accounts_principal ON accounts(tenant, principal);
    ",
    },
];

/// Index values longer than this are not indexed; lookups for them scan the object type.
const MAX_INDEXED: usize = 240;

/// Implements: H-008
pub struct SqliteStore {
    db: Database,
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

struct Header {
    state: TenantState,
}

fn header(tx: &Connection, tenant: &TenantId) -> StoreResult<Option<Header>> {
    let row = tx
        .query_row(
            "SELECT name, revision, next_record, event_seq, schema, members FROM tenants WHERE id = ?1",
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
    Ok(Some(Header {
        state: TenantState {
            id: tenant.clone(),
            name,
            revision: StateRevision(revision as u64),
            next_record: next_record as u64,
            event_seq: event_seq as u64,
            schema: Arc::new(schema),
            members,
            records: BTreeMap::new(),
        },
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

fn all_of(tx: &Connection, tenant: &TenantId, object: &ObjectTypeId) -> StoreResult<Vec<String>> {
    select_strings(
        tx,
        "SELECT data FROM records WHERE tenant = ?1 AND object = ?2 ORDER BY id",
        &[&tenant.as_str(), &object.as_str()],
    )
}

fn write_record(tx: &Transaction<'_>, tenant: &TenantId, record: &Record) -> StoreResult<()> {
    let (tenant, object, id) = (tenant.as_str(), record.object.as_str(), record.id.0 as i64);
    tx.execute(
        "INSERT INTO records (tenant, object, id, version, data) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (tenant, object, id) DO UPDATE SET version = excluded.version, data = excluded.data",
        params![tenant, object, id, record.version as i64, encode(record)],
    )
    .map_err(db)?;
    tx.execute(
        "DELETE FROM record_index WHERE tenant = ?1 AND object = ?2 AND id = ?3",
        params![tenant, object, id],
    )
    .map_err(db)?;
    let mut insert = tx
        .prepare_cached("INSERT INTO record_index (tenant, object, field, value, id) VALUES (?1, ?2, ?3, ?4, ?5)")
        .map_err(db)?;
    for (field, value) in index_rows(record) {
        insert
            .execute(params![tenant, object, field, value, id])
            .map_err(db)?;
    }
    Ok(())
}

fn throttle_row(
    conn: &Connection,
    tenant: &TenantId,
    key: &str,
) -> StoreResult<Option<ThrottleRow>> {
    conn.query_row(
        "SELECT failures, window_start, locked_until, expires_at FROM login_throttle WHERE tenant = ?1 AND key = ?2",
        params![tenant.as_str(), key],
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

type PasskeyRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    Option<i64>,
);

fn passkey_from(row: &rusqlite::Row<'_>) -> rusqlite::Result<PasskeyRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}

fn passkey_record(
    (tenant, id, principal, rp_id, label, credential, created_at, last_used_at): PasskeyRow,
) -> StoreResult<PasskeyRecord> {
    let bad = |e: strata_kernel::IdError| StoreError::Corrupt(e.to_string());
    Ok(PasskeyRecord {
        tenant: TenantId::new(tenant).map_err(bad)?,
        principal: PrincipalId::new(principal).map_err(bad)?,
        id,
        rp_id,
        label,
        credential,
        created_at: Timestamp(created_at),
        last_used_at: last_used_at.map(Timestamp),
    })
}

fn session_from(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
    i64,
    Option<i64>,
)> {
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
    ))
}

const SESSION_COLUMNS: &str =
    "id, token_hash, csrf_token, tenant, principal, display_name, created_at, last_seen_at, expires_at, revoked_at";

fn sessions_where(
    conn: &Connection,
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
                tenant,
                principal,
                display_name,
                created,
                seen,
                expires,
                revoked,
            )| {
                let bad = |e: strata_kernel::IdError| StoreError::Corrupt(e.to_string());
                Ok(SessionRecord {
                    id: SessionId::new(id).map_err(bad)?,
                    token_hash,
                    csrf_token,
                    tenant: TenantId::new(tenant).map_err(bad)?,
                    principal: PrincipalId::new(principal).map_err(bad)?,
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

impl SqliteStore {
    /// Open (creating if needed) a database file and bring its layout up to date.
    pub fn open(path: &Path, now: Timestamp) -> StoreResult<Self> {
        Self::on(Database::open(path).map_err(dbe)?, now)
    }

    /// A private in-memory database, for tests.
    pub fn open_in_memory(now: Timestamp) -> StoreResult<Self> {
        Self::on(Database::open_in_memory().map_err(dbe)?, now)
    }

    /// The tenant store on an open database, bringing its tables up to date.
    pub fn on(db: Database, now: Timestamp) -> StoreResult<Self> {
        db.migrate(LEDGER, MIGRATIONS, now.0).map_err(dbe)?;
        Ok(SqliteStore { db })
    }

    /// The database underneath, to share with the runtime's machine store.
    pub fn database(&self) -> &Database {
        &self.db
    }

    fn lock(&self) -> StoreResult<MutexGuard<'_, Connection>> {
        self.db.lock().map_err(dbe)
    }

    fn complete(conn: &Connection, tenant: &TenantId) -> StoreResult<Option<TenantState>> {
        let Some(Header { mut state }) = header(conn, tenant)? else {
            return Ok(None);
        };
        let rows = select_strings(
            conn,
            "SELECT data FROM records WHERE tenant = ?1 ORDER BY object, id",
            &[&tenant.as_str()],
        )?;
        put_records(&mut state, rows)?;
        Ok(Some(state))
    }
}

impl Store for SqliteStore {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            adapter: "sqlite".into(),
            durable: self.db.durable(),
            isolation: "serialisable: one writer at a time (BEGIN IMMEDIATE on a single connection)".into(),
            read_consistency: "a load runs in one read transaction and sees only whole commits".into(),
            notes: vec![
                "WAL journal with synchronous=FULL".into(),
                "field values longer than 240 bytes are not indexed; lookups for them scan the object type".into(),
                format!("database at {}", self.db.location()),
            ],
        }
    }

    fn health(&self) -> StoreResult<Health> {
        let conn = self.lock()?;
        let check: String = conn
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .map_err(db)?;
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .map_err(db)?;
        let tenants: i64 = conn
            .query_row("SELECT COUNT(*) FROM tenants", [], |r| r.get(0))
            .map_err(db)?;
        let pending: i64 = conn
            .query_row("SELECT COUNT(*) FROM events WHERE published = 0", [], |r| {
                r.get(0)
            })
            .map_err(db)?;
        Ok(Health {
            ok: check == "ok",
            detail: format!("quick_check {check}, journal {mode}"),
            tenants: tenants as u64,
            outbox_pending: pending as u64,
        })
    }

    fn migrations(&self) -> StoreResult<Vec<StorageMigration>> {
        let conn = self.lock()?;
        let mut statement = conn
            .prepare("SELECT version, name, applied_at FROM storage_migrations ORDER BY version")
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
        let conn = self.lock()?;
        let mut statement = conn
            .prepare(
                "SELECT t.id, t.name, t.revision, t.event_seq, t.schema,
                        (SELECT COUNT(*) FROM records r WHERE r.tenant = t.id)
                 FROM tenants t ORDER BY t.id",
            )
            .map_err(db)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(db)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db)?;
        rows.into_iter()
            .map(|(id, name, revision, event_seq, schema, records)| {
                let schema: TenantSchema = decode(&schema, "tenant schema")?;
                Ok(TenantSummary {
                    id: TenantId::new(id).map_err(|e| StoreError::Corrupt(e.to_string()))?,
                    name,
                    revision: StateRevision(revision as u64),
                    schema_version: schema.version.0,
                    event_seq: event_seq as u64,
                    records: records as u64,
                })
            })
            .collect()
    }

    fn load(&self, tenant: &TenantId, needs: &[Need]) -> StoreResult<State> {
        self.db.read(dbe, |tx| {
        let Some(Header { mut state }) = header(tx, tenant)? else {
            return Ok(State::absent(tenant.clone()));
        };
        for need in needs {
            let rows = match need {
                Need::Record { object, id } => select_strings(
                    tx,
                    "SELECT data FROM records WHERE tenant = ?1 AND object = ?2 AND id = ?3",
                    &[&tenant.as_str(), &object.as_str(), &(id.0 as i64)],
                )?,
                Need::AllOf(object) => all_of(tx, tenant, object)?,
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
                             JOIN records r ON r.tenant = i.tenant AND r.object = i.object AND r.id = i.id
                             WHERE i.tenant = ?1 AND i.object = ?2 AND i.field = ?3 AND i.value = ?4",
                            &[&tenant.as_str(), &object.as_str(), &field.as_str(), &key],
                        )?
                    } else {
                        let mut matching = Vec::new();
                        for data in all_of(tx, tenant, object)? {
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
        self.db.read(dbe, |tx| {
            Ok(match Self::complete(tx, tenant)? {
                Some(state) => State::complete(Arc::new(state)),
                None => State::absent(tenant.clone()),
            })
        })
    }

    fn commit(&self, commit: &Commit) -> StoreResult<Receipt> {
        self.db.write(dbe, |tx| {
        let delta = &commit.delta;
        let tenant = delta.tenant.as_str();
        let current = header(tx, &delta.tenant)?;
        let found = current.as_ref().map(|h| h.state.revision);
        if found != delta.base_revision {
            return Err(StoreError::Conflict {
                expected: delta.base_revision,
                found,
            });
        }
        check_event_sequence(commit, current.as_ref().map_or(0, |h| h.state.event_seq))?;

        let mut members = current
            .as_ref()
            .map(|h| h.state.members.clone())
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
        let counters = params![
            delta.new_revision.0 as i64,
            delta.next_record as i64,
            delta.event_seq as i64,
            tenant
        ];
        match (current.is_some(), created) {
            (false, Some(name)) => {
                let schema = schema
                    .clone()
                    .unwrap_or_else(|| Arc::new(TenantSchema::genesis()));
                tx.execute(
                    "INSERT INTO tenants (id, name, revision, next_record, event_seq, schema, members)
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
                tx.execute("UPDATE tenants SET revision = ?1, next_record = ?2, event_seq = ?3 WHERE id = ?4", counters)
                    .map_err(db)?;
                if let Some(schema) = &schema {
                    tx.execute(
                        "UPDATE tenants SET schema = ?1 WHERE id = ?2",
                        params![encode(&**schema), tenant],
                    )
                    .map_err(db)?;
                }
                if members_changed {
                    tx.execute(
                        "UPDATE tenants SET members = ?1 WHERE id = ?2",
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
                "INSERT INTO schema_history (tenant, version, revision, schema) VALUES (?1, ?2, ?3, ?4)",
                params![tenant, schema.version.0 as i64, delta.new_revision.0 as i64, encode(&**schema)],
            )
            .map_err(db)?;
        }
        for op in &delta.ops {
            match op {
                DeltaOp::PutRecord(record) => write_record(tx, &delta.tenant, record)?,
                DeltaOp::DeleteRecord { object, id } => {
                    tx.execute(
                        "DELETE FROM records WHERE tenant = ?1 AND object = ?2 AND id = ?3",
                        params![tenant, object.as_str(), id.0 as i64],
                    )
                    .map_err(db)?;
                }
                _ => {}
            }
        }
        let mut last_position = None;
        for event in &commit.events {
            tx.execute(
                "INSERT INTO events (tenant, seq, revision, data) VALUES (?1, ?2, ?3, ?4)",
                params![
                    tenant,
                    event.seq as i64,
                    event.revision.0 as i64,
                    encode(event)
                ],
            )
            .map_err(db)?;
            last_position = Some(tx.last_insert_rowid() as u64);
        }
        tx.execute(
            "INSERT INTO journal (tenant, revision, data) VALUES (?1, ?2, ?3)",
            params![
                tenant,
                commit.journal.revision.0 as i64,
                encode(&commit.journal)
            ],
        )
        .map_err(db)?;
        for principal in &commit.revoke_sessions {
            tx.execute(
                "UPDATE sessions SET revoked_at = ?1 WHERE tenant = ?2 AND principal = ?3 AND revoked_at IS NULL",
                params![commit.journal.context.now.0, tenant, principal.as_str()],
            )
            .map_err(db)?;
        }
        Ok(Receipt {
            revision: delta.new_revision,
            last_position,
        })
        })
    }

    fn events_after(
        &self,
        tenant: &TenantId,
        after: u64,
        limit: usize,
    ) -> StoreResult<Vec<DomainEvent>> {
        let conn = self.lock()?;
        select_strings(
            &conn,
            "SELECT data FROM events WHERE tenant = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
            &[&tenant.as_str(), &(after as i64), &(limit as i64)],
        )?
        .iter()
        .map(|data| decode(data, "event"))
        .collect()
    }

    fn outbox(&self, limit: usize) -> StoreResult<Vec<OutboxEntry>> {
        let conn = self.lock()?;
        let mut statement = conn
            .prepare_cached(
                "SELECT position, data FROM events WHERE published = 0 ORDER BY position LIMIT ?1",
            )
            .map_err(db)?;
        let rows = statement
            .query_map(params![limit as i64], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(db)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db)?;
        rows.into_iter()
            .map(|(position, data)| {
                Ok(OutboxEntry {
                    position: position as u64,
                    event: decode(&data, "event")?,
                })
            })
            .collect()
    }

    fn mark_published(&self, position: u64) -> StoreResult<()> {
        self.lock()?
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
        let conn = self.lock()?;
        select_strings(
            &conn,
            "SELECT data FROM journal WHERE tenant = ?1 AND revision > ?2 ORDER BY revision LIMIT ?3",
            &[&tenant.as_str(), &(after as i64), &(limit as i64)],
        )?
        .iter()
        .map(|data| decode(data, "journal entry"))
        .collect()
    }

    fn put_session(&self, session: &SessionRecord) -> StoreResult<()> {
        let conn = self.lock()?;
        let known: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM tenants WHERE id = ?1",
                params![session.tenant.as_str()],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)?;
        if known.is_none() {
            return Err(StoreError::NotFound(format!("tenant {}", session.tenant)));
        }
        conn.execute(
            &format!(
                "INSERT INTO sessions ({SESSION_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT (id) DO UPDATE SET token_hash = excluded.token_hash, csrf_token = excluded.csrf_token,
                    display_name = excluded.display_name, last_seen_at = excluded.last_seen_at,
                    expires_at = excluded.expires_at, revoked_at = excluded.revoked_at"
            ),
            params![
                session.id.as_str(),
                session.token_hash,
                session.csrf_token,
                session.tenant.as_str(),
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
        Ok(sessions_where(&*self.lock()?, "token_hash = ?1", &[&token_hash])?.pop())
    }

    fn session(&self, id: &SessionId) -> StoreResult<Option<SessionRecord>> {
        Ok(sessions_where(&*self.lock()?, "id = ?1", &[&id.as_str()])?.pop())
    }

    fn live_sessions(&self, now: Timestamp) -> StoreResult<Vec<SessionRecord>> {
        sessions_where(
            &*self.lock()?,
            "revoked_at IS NULL AND expires_at > ?1",
            &[&now.0],
        )
    }

    fn purge_sessions(&self, before: Timestamp) -> StoreResult<u64> {
        let removed = self
            .lock()?
            .execute(
                "DELETE FROM sessions WHERE expires_at < ?1 OR (revoked_at IS NOT NULL AND revoked_at < ?1)",
                params![before.0],
            )
            .map_err(db)?;
        Ok(removed as u64)
    }

    fn throttle_reserve(
        &self,
        tenant: &TenantId,
        rules: &[ThrottleRule],
        now: Timestamp,
    ) -> StoreResult<ThrottleVerdict> {
        self.db.write(dbe, |tx| {
            let mut next = Vec::with_capacity(rules.len());
            let mut until: Option<i64> = None;
            for rule in rules {
                let row = throttle_row(tx, tenant, &rule.key)?;
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
                    "INSERT INTO login_throttle (tenant, key, failures, window_start, locked_until, expires_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT (tenant, key) DO UPDATE SET failures = excluded.failures,
                        window_start = excluded.window_start, locked_until = excluded.locked_until,
                        expires_at = excluded.expires_at",
                    params![
                        tenant.as_str(),
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
        self.db.write(dbe, |tx| {
            match how {
                ThrottleSettle::Clear => {
                    tx.execute(
                        "DELETE FROM login_throttle WHERE tenant = ?1 AND key = ?2",
                        params![tenant.as_str(), key],
                    )
                    .map_err(db)?;
                }
                ThrottleSettle::Refund => {
                    if let Some(row) = throttle_row(tx, tenant, key)? {
                        let row = throttle_refund(row, max);
                        tx.execute(
                            "UPDATE login_throttle SET failures = ?3, locked_until = ?4 WHERE tenant = ?1 AND key = ?2",
                            params![
                                tenant.as_str(),
                                key,
                                i64::from(row.failures),
                                row.locked_until
                            ],
                        )
                        .map_err(db)?;
                    }
                }
            }
            Ok(())
        })
    }

    fn purge_throttle(&self, now: Timestamp) -> StoreResult<u64> {
        let removed = self
            .lock()?
            .execute(
                "DELETE FROM login_throttle WHERE expires_at <= ?1",
                params![now.0],
            )
            .map_err(db)?;
        Ok(removed as u64)
    }

    fn account_of(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<Option<Account>> {
        let conn = self.lock()?;
        let row = conn
            .query_row(
                "SELECT login, display_name, password_hash FROM accounts WHERE tenant = ?1 AND principal = ?2 ORDER BY login LIMIT 1",
                params![tenant.as_str(), principal.as_str()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
            )
            .optional()
            .map_err(db)?;
        Ok(row.map(|(login, display_name, password_hash)| Account {
            tenant: tenant.clone(),
            login,
            principal: principal.clone(),
            display_name,
            password_hash,
        }))
    }

    fn put_passkey(&self, passkey: &PasskeyRecord) -> StoreResult<bool> {
        let added = self
            .lock()?
            .execute(
                "INSERT OR IGNORE INTO passkeys (tenant, id, principal, rp_id, label, credential, created_at, last_used_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    passkey.tenant.as_str(),
                    passkey.id,
                    passkey.principal.as_str(),
                    passkey.rp_id,
                    passkey.label,
                    passkey.credential,
                    passkey.created_at.0,
                    passkey.last_used_at.map(|at| at.0),
                ],
            )
            .map_err(db)?;
        Ok(added == 1)
    }

    fn passkey(&self, tenant: &TenantId, id: &str) -> StoreResult<Option<PasskeyRecord>> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT tenant, id, principal, rp_id, label, credential, created_at, last_used_at FROM passkeys WHERE tenant = ?1 AND id = ?2",
            params![tenant.as_str(), id],
            passkey_from,
        )
        .optional()
        .map_err(db)?
        .map(passkey_record)
        .transpose()
    }

    fn passkeys(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<Vec<PasskeyRecord>> {
        let conn = self.lock()?;
        let mut statement = conn
            .prepare(
                "SELECT tenant, id, principal, rp_id, label, credential, created_at, last_used_at FROM passkeys
                 WHERE tenant = ?1 AND principal = ?2 ORDER BY created_at, id",
            )
            .map_err(db)?;
        let rows = statement
            .query_map(params![tenant.as_str(), principal.as_str()], passkey_from)
            .map_err(db)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db)?;
        rows.into_iter().map(passkey_record).collect()
    }

    fn update_passkey(
        &self,
        tenant: &TenantId,
        id: &str,
        credential: &str,
        used_at: Timestamp,
    ) -> StoreResult<bool> {
        let changed = self
            .lock()?
            .execute(
                "UPDATE passkeys SET credential = ?3, last_used_at = ?4 WHERE tenant = ?1 AND id = ?2",
                params![tenant.as_str(), id, credential, used_at.0],
            )
            .map_err(db)?;
        Ok(changed == 1)
    }

    fn remove_passkey(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        id: &str,
    ) -> StoreResult<bool> {
        let removed = self
            .lock()?
            .execute(
                "DELETE FROM passkeys WHERE tenant = ?1 AND principal = ?2 AND id = ?3",
                params![tenant.as_str(), principal.as_str(), id],
            )
            .map_err(db)?;
        Ok(removed == 1)
    }

    fn put_passkey_challenge(&self, challenge: &PasskeyChallenge) -> StoreResult<()> {
        self.lock()?
            .execute(
                "INSERT OR REPLACE INTO passkey_challenges (tenant, id_hash, ceremony, session, principal, origin, state, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    challenge.tenant.as_str(),
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
            .map_err(db)?;
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
        let row = self
            .lock()?
            .query_row(
                "DELETE FROM passkey_challenges WHERE tenant = ?1 AND id_hash = ?2
                 RETURNING ceremony, session, principal, origin, state, expires_at",
                params![tenant.as_str(), id_hash],
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
            .map_err(db)?;
        let Some((ceremony, session, principal, origin, state, expires_at)) = row else {
            return Ok(None);
        };
        if expires_at <= now.0 {
            return Ok(None);
        }
        let bad = |e: strata_kernel::IdError| StoreError::Corrupt(e.to_string());
        Ok(Some(PasskeyChallenge {
            tenant: tenant.clone(),
            id_hash: id_hash.to_string(),
            ceremony: if ceremony == "register" {
                PasskeyCeremony::Register
            } else {
                PasskeyCeremony::SignIn
            },
            session: session.map(SessionId::new).transpose().map_err(bad)?,
            principal: principal.map(PrincipalId::new).transpose().map_err(bad)?,
            origin,
            state,
            expires_at: Timestamp(expires_at),
        }))
    }

    fn purge_passkey_challenges(&self, now: Timestamp) -> StoreResult<u64> {
        let removed = self
            .lock()?
            .execute(
                "DELETE FROM passkey_challenges WHERE expires_at <= ?1",
                params![now.0],
            )
            .map_err(db)?;
        Ok(removed as u64)
    }

    fn put_account(&self, account: &Account) -> StoreResult<()> {
        let conn = self.lock()?;
        let known: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM tenants WHERE id = ?1",
                params![account.tenant.as_str()],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)?;
        if known.is_none() {
            return Err(StoreError::NotFound(format!("tenant {}", account.tenant)));
        }
        conn.execute(
            "INSERT INTO accounts (tenant, login, principal, display_name, password_hash) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (tenant, login) DO UPDATE SET principal = excluded.principal,
                display_name = excluded.display_name, password_hash = excluded.password_hash",
            params![
                account.tenant.as_str(),
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
        let conn = self.lock()?;
        let row = conn
            .query_row(
                "SELECT principal, display_name, password_hash FROM accounts WHERE tenant = ?1 AND login = ?2",
                params![tenant.as_str(), login],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
            )
            .optional()
            .map_err(db)?;
        row.map(|(principal, display_name, password_hash)| {
            Ok(Account {
                tenant: tenant.clone(),
                login: login.to_string(),
                principal: PrincipalId::new(principal)
                    .map_err(|e| StoreError::Corrupt(e.to_string()))?,
                display_name,
                password_hash,
            })
        })
        .transpose()
    }

    fn remove_account(&self, tenant: &TenantId, login: &str) -> StoreResult<bool> {
        let conn = self.lock()?;
        let removed = conn
            .execute(
                "DELETE FROM accounts WHERE tenant = ?1 AND login = ?2",
                params![tenant.as_str(), login],
            )
            .map_err(db)?;
        Ok(removed > 0)
    }

    fn create_snapshot(
        &self,
        tenant: &TenantId,
        label: &str,
        at: Timestamp,
    ) -> StoreResult<SnapshotInfo> {
        self.db.write(dbe, |tx| {
        let state = Self::complete(tx, tenant)?
            .ok_or_else(|| StoreError::NotFound(format!("tenant {tenant}")))?;
        let id: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(id), 0) + 1 FROM snapshots WHERE tenant = ?1",
                params![tenant.as_str()],
                |r| r.get(0),
            )
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
            "INSERT INTO snapshots (tenant, id, label, revision, hash, created_at, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![tenant.as_str(), id, label, state.revision.0 as i64, info.hash, at.0, encode(&state)],
        )
        .map_err(db)?;
        Ok(info)
        })
    }

    fn snapshots(&self, tenant: &TenantId) -> StoreResult<Vec<SnapshotInfo>> {
        let conn = self.lock()?;
        let mut statement = conn
            .prepare("SELECT id, label, revision, hash, created_at FROM snapshots WHERE tenant = ?1 ORDER BY id")
            .map_err(db)?;
        let rows = statement
            .query_map(params![tenant.as_str()], |row| {
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
        let conn = self.lock()?;
        let data: Option<String> = conn
            .query_row(
                "SELECT data FROM snapshots WHERE tenant = ?1 AND id = ?2",
                params![tenant.as_str(), id as i64],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)?;
        decode(
            &data.ok_or_else(|| StoreError::NotFound(format!("snapshot {id} of {tenant}")))?,
            "snapshot",
        )
    }

    fn restore_snapshot(&self, tenant: &TenantId, id: u64) -> StoreResult<()> {
        let state = self.load_snapshot(tenant, id)?;
        self.db.write(dbe, |tx| {
        let name = tenant.as_str();
        tx.execute("DELETE FROM records WHERE tenant = ?1", params![name])
            .map_err(db)?;
        tx.execute(
            "UPDATE tenants SET name = ?1, revision = ?2, next_record = ?3, event_seq = ?4, schema = ?5, members = ?6
             WHERE id = ?7",
            params![
                state.name,
                state.revision.0 as i64,
                state.next_record as i64,
                state.event_seq as i64,
                encode(&*state.schema),
                encode(&state.members),
                name
            ],
        )
        .map_err(db)?;
        for record in state.records.values().flat_map(|records| records.values()) {
            write_record(tx, tenant, record)?;
        }
        tx.execute(
            "DELETE FROM events WHERE tenant = ?1 AND seq > ?2",
            params![name, state.event_seq as i64],
        )
        .map_err(db)?;
        tx.execute(
            "DELETE FROM journal WHERE tenant = ?1 AND revision > ?2",
            params![name, state.revision.0 as i64],
        )
        .map_err(db)?;
        tx.execute(
            "DELETE FROM schema_history WHERE tenant = ?1 AND version > ?2",
            params![name, state.schema.version.0 as i64],
        )
        .map_err(db)?;
        Ok(())
        })
    }

    fn reset(&self) -> StoreResult<()> {
        let conn = self.lock()?;
        // Every other table hangs off `tenants` with ON DELETE CASCADE.
        conn.execute_batch(
            "DELETE FROM tenants; DELETE FROM sqlite_sequence WHERE name = 'events';",
        )
        .map_err(db)
    }

    fn reset_tenant(&self, tenant: &TenantId) -> StoreResult<()> {
        let conn = self.lock()?;
        // Every other table hangs off `tenants` with ON DELETE CASCADE.
        conn.execute(
            "DELETE FROM tenants WHERE id = ?1",
            params![tenant.as_str()],
        )
        .map_err(db)?;
        Ok(())
    }
}
