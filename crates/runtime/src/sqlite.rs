//! SQLite: the shared connection handling, and the machine store on top of it.
//!
//! [`Database`] is how every Strata component opens a SQLite file: one connection behind a
//! mutex, a busy timeout for other processes that have the file open, the WAL journal,
//! `synchronous = FULL` and foreign keys on. Write transactions are opened `IMMEDIATE`, so there
//! is one writer at a time and every write transaction is serializable. Layout changes are
//! numbered migrations recorded in a ledger table, applied in one transaction.
//!
//! The tenant store (`strata-storage-sqlite`) and [`SqliteMachineStore`] both run on a
//! [`Database`]; given the same one, they share the file and the connection, and each keeps its
//! own tables and its own migration ledger.

use crate::clock::Time;
use crate::machine::Isolation;
use crate::store::{
    NewMessage, NewTimer, OutboxEntry, OutboxStore, RecordStore, StorageError, StoredRecord,
    TimerEntry, TimerStore, Tx,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// Why a [`Database`] operation failed.
#[derive(Debug)]
pub enum DbError {
    /// SQLite reported an error.
    Sql(rusqlite::Error),
    /// A thread panicked while holding the connection.
    Poisoned,
    /// A migration ledger name that is not a plain identifier.
    BadLedger(String),
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbError::Sql(error) => error.fmt(f),
            DbError::Poisoned => f.write_str("a writer panicked"),
            DbError::BadLedger(name) => write!(f, "bad migration ledger name {name:?}"),
        }
    }
}

impl std::error::Error for DbError {}

impl From<rusqlite::Error> for DbError {
    fn from(error: rusqlite::Error) -> Self {
        DbError::Sql(error)
    }
}

impl From<DbError> for StorageError {
    fn from(error: DbError) -> Self {
        match &error {
            DbError::Sql(rusqlite::Error::SqliteFailure(code, _))
                if code.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                StorageError::Invalid(error.to_string())
            }
            DbError::BadLedger(_) => StorageError::Invalid(error.to_string()),
            _ => StorageError::Unavailable(error.to_string()),
        }
    }
}

/// One step of a layout: applied once, in version order, never edited after release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Migration {
    /// Its number. Versions start at 1 and have no gaps.
    pub version: u32,
    /// What it does, for the ledger.
    pub name: &'static str,
    /// The statements it runs.
    pub sql: &'static str,
}

/// An open SQLite database. Clones share the connection.
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
    location: Arc<str>,
}

impl fmt::Debug for Database {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Database")
            .field("location", &self.location)
            .finish()
    }
}

impl Database {
    /// Open (creating if needed) the database file at `path`.
    pub fn open(path: &Path) -> Result<Self, DbError> {
        let conn = Connection::open(path)?;
        Self::prepare(conn, path.display().to_string())
    }

    /// A private in-memory database, for tests.
    pub fn open_in_memory() -> Result<Self, DbError> {
        let conn = Connection::open_in_memory()?;
        Self::prepare(conn, ":memory:".into())
    }

    fn prepare(conn: Connection, location: String) -> Result<Self, DbError> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Ok(Database {
            conn: Arc::new(Mutex::new(conn)),
            location: location.into(),
        })
    }

    /// Where the database lives: a file path, or `:memory:`.
    pub fn location(&self) -> &str {
        &self.location
    }

    /// Whether the data outlives the process.
    pub fn durable(&self) -> bool {
        &*self.location != ":memory:"
    }

    /// The connection, exclusively.
    pub fn lock(&self) -> Result<MutexGuard<'_, Connection>, DbError> {
        self.conn.lock().map_err(|_| DbError::Poisoned)
    }

    /// Apply the steps of `migrations` whose version is above the highest one recorded in the
    /// ledger table `ledger` (created if missing), in one `IMMEDIATE` transaction, recording each
    /// with `now` (milliseconds since the epoch). `ledger` must be a plain identifier.
    pub fn migrate(&self, ledger: &str, migrations: &[Migration], now: i64) -> Result<(), DbError> {
        if ledger.is_empty()
            || !ledger
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(DbError::BadLedger(ledger.to_string()));
        }
        let mut conn = self.lock()?;
        conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS {ledger} (
                version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at INTEGER NOT NULL
            );"
        ))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let applied: u32 = tx.query_row(
            &format!("SELECT COALESCE(MAX(version), 0) FROM {ledger}"),
            [],
            |r| r.get(0),
        )?;
        for step in migrations.iter().filter(|m| m.version > applied) {
            tx.execute_batch(step.sql)?;
            tx.execute(
                &format!("INSERT INTO {ledger} (version, name, applied_at) VALUES (?1, ?2, ?3)"),
                params![step.version, step.name, now],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Run `work` in one `IMMEDIATE` transaction and commit it when `work` returns `Ok`. On
    /// `Err`, or when the commit fails, the transaction is rolled back. `error` turns the
    /// database's own failures into the caller's error type.
    pub fn write<T, E>(
        &self,
        error: impl Fn(DbError) -> E,
        work: impl FnOnce(&Transaction<'_>) -> Result<T, E>,
    ) -> Result<T, E> {
        let mut conn = self.lock().map_err(&error)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| error(e.into()))?;
        let result = work(&tx)?;
        tx.commit().map_err(|e| error(e.into()))?;
        Ok(result)
    }

    /// Run `work` in one read transaction, which sees only whole commits. Nothing is committed.
    pub fn read<T, E>(
        &self,
        error: impl Fn(DbError) -> E,
        work: impl FnOnce(&Transaction<'_>) -> Result<T, E>,
    ) -> Result<T, E> {
        let mut conn = self.lock().map_err(&error)?;
        let tx = conn.transaction().map_err(|e| error(e.into()))?;
        work(&tx)
    }
}

/// The machine store's layout, recorded in the `runtime_migrations` ledger.
const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "machine records, outbox and timers",
    sql: "
    CREATE TABLE runtime_records (
        collection TEXT NOT NULL,
        key        TEXT NOT NULL,
        version    INTEGER NOT NULL,
        data       TEXT NOT NULL,
        PRIMARY KEY (collection, key)
    );
    CREATE TABLE runtime_outbox (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        source          TEXT NOT NULL,
        key             TEXT NOT NULL,
        receiver        TEXT NOT NULL,
        event           TEXT NOT NULL,
        created_at      INTEGER NOT NULL,
        attempts        INTEGER NOT NULL DEFAULT 0,
        next_attempt_at INTEGER NOT NULL,
        last_error      TEXT
    );
    CREATE INDEX runtime_outbox_due ON runtime_outbox(source, next_attempt_at);
    CREATE TABLE runtime_timers (
        id      INTEGER PRIMARY KEY AUTOINCREMENT,
        machine TEXT NOT NULL,
        key     TEXT NOT NULL,
        name    TEXT NOT NULL,
        fire_at INTEGER NOT NULL,
        event   TEXT NOT NULL,
        UNIQUE (machine, key, name)
    );
    CREATE INDEX runtime_timers_due ON runtime_timers(machine, fire_at);
    ",
}];

/// The ledger table of the machine store's migrations.
pub const LEDGER: &str = "runtime_migrations";

/// The machine store on a SQLite [`Database`].
///
/// Every transaction is `IMMEDIATE` on the single connection, so it is serializable, which
/// satisfies every declared isolation.
#[derive(Clone, Debug)]
pub struct SqliteMachineStore {
    db: Database,
}

fn sql(error: rusqlite::Error) -> StorageError {
    DbError::Sql(error).into()
}

impl SqliteMachineStore {
    /// The machine store on `db`, bringing its tables up to date. `now` is recorded in the
    /// ledger for any migration applied.
    pub fn open(db: Database, now: Time) -> Result<Self, StorageError> {
        db.migrate(LEDGER, MIGRATIONS, now.0)?;
        Ok(SqliteMachineStore { db })
    }

    /// The database underneath.
    pub fn database(&self) -> &Database {
        &self.db
    }
}

struct SqliteTx<'a, 'c> {
    tx: &'a Transaction<'c>,
}

impl Tx for SqliteTx<'_, '_> {
    fn load(&mut self, collection: &str, key: &str) -> Result<Option<StoredRecord>, StorageError> {
        self.tx
            .query_row(
                "SELECT data, version FROM runtime_records WHERE collection = ?1 AND key = ?2",
                params![collection, key],
                |row| {
                    Ok(StoredRecord {
                        data: row.get(0)?,
                        version: row.get::<_, i64>(1)? as u64,
                    })
                },
            )
            .optional()
            .map_err(sql)
    }

    fn save(
        &mut self,
        collection: &str,
        key: &str,
        data: &str,
        expected: Option<u64>,
    ) -> Result<u64, StorageError> {
        let found = self.load(collection, key)?.map(|r| r.version);
        if found != expected {
            return Err(StorageError::Conflict(format!(
                "{collection} {key}: expected version {expected:?}, found {found:?}"
            )));
        }
        let version = found.unwrap_or(0) + 1;
        self.tx
            .execute(
                "INSERT INTO runtime_records (collection, key, version, data) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (collection, key) DO UPDATE SET version = excluded.version, data = excluded.data",
                params![collection, key, version as i64, data],
            )
            .map_err(sql)?;
        Ok(version)
    }

    fn enqueue(&mut self, message: &NewMessage) -> Result<u64, StorageError> {
        self.tx
            .execute(
                "INSERT INTO runtime_outbox (source, key, receiver, event, created_at, next_attempt_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    message.source,
                    message.key,
                    message.receiver,
                    message.event,
                    message.created_at.0,
                    message.next_attempt_at.0
                ],
            )
            .map_err(sql)?;
        Ok(self.tx.last_insert_rowid() as u64)
    }

    fn set_timer(&mut self, timer: &NewTimer) -> Result<u64, StorageError> {
        self.cancel_timer(&timer.machine, &timer.key, &timer.name)?;
        self.tx
            .execute(
                "INSERT INTO runtime_timers (machine, key, name, fire_at, event) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![timer.machine, timer.key, timer.name, timer.fire_at.0, timer.event],
            )
            .map_err(sql)?;
        Ok(self.tx.last_insert_rowid() as u64)
    }

    fn cancel_timer(&mut self, machine: &str, key: &str, name: &str) -> Result<bool, StorageError> {
        let removed = self
            .tx
            .execute(
                "DELETE FROM runtime_timers WHERE machine = ?1 AND key = ?2 AND name = ?3",
                params![machine, key, name],
            )
            .map_err(sql)?;
        Ok(removed > 0)
    }

    fn take_timer(&mut self, id: u64) -> Result<bool, StorageError> {
        let removed = self
            .tx
            .execute(
                "DELETE FROM runtime_timers WHERE id = ?1",
                params![id as i64],
            )
            .map_err(sql)?;
        Ok(removed > 0)
    }
}

impl RecordStore for SqliteMachineStore {
    fn supports(&self, _: Isolation) -> bool {
        true
    }

    fn transact<T, E>(
        &self,
        _: Isolation,
        work: impl FnOnce(&mut dyn Tx) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<StorageError>,
    {
        self.db.write(
            |error| E::from(error.into()),
            |tx| work(&mut SqliteTx { tx }),
        )
    }
}

fn outbox_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<OutboxEntry> {
    Ok(OutboxEntry {
        id: row.get::<_, i64>(0)? as u64,
        source: row.get(1)?,
        key: row.get(2)?,
        receiver: row.get(3)?,
        event: row.get(4)?,
        created_at: Time(row.get(5)?),
        attempts: row.get(6)?,
        next_attempt_at: Time(row.get(7)?),
        last_error: row.get(8)?,
    })
}

impl OutboxStore for SqliteMachineStore {
    fn due_messages(
        &self,
        source: &str,
        now: Time,
        limit: usize,
    ) -> Result<Vec<OutboxEntry>, StorageError> {
        let conn = self.db.lock()?;
        let mut statement = conn
            .prepare_cached(
                "SELECT id, source, key, receiver, event, created_at, attempts, next_attempt_at, last_error
                 FROM runtime_outbox WHERE source = ?1 AND next_attempt_at <= ?2 ORDER BY id LIMIT ?3",
            )
            .map_err(sql)?;
        let rows = statement
            .query_map(params![source, now.0, limit as i64], outbox_entry)
            .map_err(sql)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql)
    }

    fn mark_delivered(&self, id: u64) -> Result<(), StorageError> {
        self.db
            .lock()?
            .execute(
                "DELETE FROM runtime_outbox WHERE id = ?1",
                params![id as i64],
            )
            .map_err(sql)?;
        Ok(())
    }

    fn defer(
        &self,
        id: u64,
        attempts: u32,
        next_attempt_at: Time,
        error: &str,
    ) -> Result<(), StorageError> {
        self.db
            .lock()?
            .execute(
                "UPDATE runtime_outbox SET attempts = ?1, next_attempt_at = ?2, last_error = ?3 WHERE id = ?4",
                params![attempts, next_attempt_at.0, error, id as i64],
            )
            .map_err(sql)?;
        Ok(())
    }

    fn pending(&self) -> Result<u64, StorageError> {
        let count: i64 = self
            .db
            .lock()?
            .query_row("SELECT COUNT(*) FROM runtime_outbox", [], |r| r.get(0))
            .map_err(sql)?;
        Ok(count as u64)
    }
}

impl TimerStore for SqliteMachineStore {
    fn due_timers(
        &self,
        machine: &str,
        now: Time,
        limit: usize,
    ) -> Result<Vec<TimerEntry>, StorageError> {
        let conn = self.db.lock()?;
        let mut statement = conn
            .prepare_cached(
                "SELECT id, machine, key, name, fire_at, event FROM runtime_timers
                 WHERE machine = ?1 AND fire_at <= ?2 ORDER BY fire_at, id LIMIT ?3",
            )
            .map_err(sql)?;
        let rows = statement
            .query_map(params![machine, now.0, limit as i64], |row| {
                Ok(TimerEntry {
                    id: row.get::<_, i64>(0)? as u64,
                    machine: row.get(1)?,
                    key: row.get(2)?,
                    name: row.get(3)?,
                    fire_at: Time(row.get(4)?),
                    event: row.get(5)?,
                })
            })
            .map_err(sql)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sql)
    }
}
