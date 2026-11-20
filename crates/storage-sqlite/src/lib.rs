//! The SQLite adapter. Every SQL statement and table of the tenant store lives in this file, and
//! in `index_page.rs` for the record index's keys and its pages.
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
//! the removal. An install marks the row `provisioning` after its first commit and `active` after
//! its last ([`Store::begin_provisioning`], [`Store::finish_provisioning`]); a tenant in that state
//! loads as absent, and [`Store::rollback_provisioning`] removes it.
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

mod ledger;

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use strata_kernel::ext::*;
use strata_kernel::segments as seg;
use strata_kernel::WorkflowId;
use strata_kernel::{
    canon, Coverage, DeltaOp, DomainEvent, Need, ObjectTypeId, PrincipalId, Record, RoleId,
    SessionId, SettingId, SettingOverride, SettingValues, State, StateRevision, TenantId,
    TenantSchema, TenantState, Timestamp, Value,
};
use strata_runtime::sqlite::{Database, DbError, Migration};
use strata_storage_api::jobs::{
    self, ExceptionKind, ExceptionRecord, Job, JobChange, JobReport, Lease, NewJob, Reported,
    RunRecord, RunState, Settled, StepRecord, WorkerWrites,
};
use strata_storage_api::segments::{write_conflict, SegmentCommit};
use strata_storage_api::{
    check_event_sequence, field_matches, throttle_refund, throttle_step, Account, AuditEntry,
    Capabilities, Commit, FederatedIdentity, Health, IdempotentResult, JournalEntry, OidcFlow,
    OutboxEntry, OutboxFault, PasskeyCeremony, PasskeyChallenge, PasskeyRecord, PendingSignIn,
    PrincipalRemoval, Receipt, RecoveryRequest, SamlRequest, SessionRecord, SignInLink,
    SnapshotInfo, StorageMigration, Store, StoreError, StoreResult, TenantSummary, ThrottleRow,
    ThrottleRule, ThrottleSettle, ThrottleVerdict, TotpRecord, IDEMPOTENCY_RETENTION_MS,
};

mod index_page;
mod pair_index;
mod search_index;

/// The ledger table of the control database's migrations.
const CONTROL_LEDGER: &str = "control_migrations";
/// The ledger table of a tenant file's migrations.
const TENANT_LEDGER: &str = "tenant_migrations";

/// The control database's layout, one step per entry. Steps are only ever appended.
const CONTROL_MIGRATIONS: &[Migration] = &[
    Migration {
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
    },
    Migration {
        version: 2,
        name: "directory state provisioning",
        // The state check cannot be altered, so both tables are rebuilt. The session directory goes
        // first, and its rows are copied back, so that dropping the old tenant table cascades to
        // nothing.
        sql: "
    DROP INDEX session_directory_tenant;
    ALTER TABLE session_directory RENAME TO session_directory_old;
    ALTER TABLE tenant_directory RENAME TO tenant_directory_old;
    CREATE TABLE tenant_directory (
        id         TEXT PRIMARY KEY,
        state      TEXT NOT NULL CHECK (state IN ('active', 'provisioning', 'deleting')),
        file       TEXT NOT NULL UNIQUE,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE session_directory (
        id         TEXT PRIMARY KEY,
        token_hash TEXT NOT NULL UNIQUE,
        tenant     TEXT NOT NULL REFERENCES tenant_directory(id) ON DELETE CASCADE
    );
    INSERT INTO tenant_directory (id, state, file, created_at)
        SELECT id, state, file, created_at FROM tenant_directory_old;
    INSERT INTO session_directory (id, token_hash, tenant)
        SELECT id, token_hash, tenant FROM session_directory_old;
    DROP TABLE session_directory_old;
    DROP TABLE tenant_directory_old;
    CREATE INDEX session_directory_tenant ON session_directory(tenant);
    ",
    },
    Migration {
        version: 3,
        name: "job wake index",
        // The wake index of the automation worker (SCH-007, SCH-008): per tenant, the earliest
        // time one of its jobs is due, and a counter that each writer raises so that a refresh
        // never overwrites a time lowered after it read. Nothing of a job is here.
        sql: "
    CREATE TABLE job_wake (
        tenant  TEXT PRIMARY KEY REFERENCES tenant_directory(id) ON DELETE CASCADE,
        due_at  INTEGER NOT NULL,
        touched INTEGER NOT NULL
    );
    CREATE INDEX job_wake_due ON job_wake(due_at);
    ",
    },
];

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
    Migration {
        version: 8,
        name: "identity service documents",
        sql: "
    CREATE TABLE identity_docs (
        kind  TEXT NOT NULL,
        key   TEXT NOT NULL,
        value TEXT NOT NULL,
        PRIMARY KEY (kind, key)
    );
    ",
    },
    Migration {
        version: 9,
        name: "verified e-mail address of an account",
        sql: "ALTER TABLE accounts ADD COLUMN verified_email TEXT;",
    },
    Migration {
        version: 10,
        name: "idempotency keys",
        sql: "
    CREATE TABLE idempotency (
        principal     TEXT NOT NULL,
        key           TEXT NOT NULL,
        request_hash  TEXT NOT NULL,
        recorded_at   INTEGER NOT NULL,
        revision      INTEGER NOT NULL,
        last_position INTEGER,
        result        TEXT NOT NULL,
        PRIMARY KEY (principal, key)
    );
    CREATE INDEX idempotency_recorded ON idempotency(recorded_at);
    ",
    },
    Migration {
        version: 11,
        name: "security audit trail",
        sql: "
    CREATE TABLE audit_log (
        seq         INTEGER PRIMARY KEY AUTOINCREMENT,
        at          INTEGER NOT NULL,
        kind        TEXT NOT NULL,
        event       TEXT NOT NULL,
        correlation TEXT NOT NULL,
        data        TEXT NOT NULL
    );
    CREATE INDEX audit_log_kind ON audit_log(kind, seq);
    ",
    },
    Migration {
        version: 12,
        name: "setting overrides and revisions",
        sql: "
    CREATE TABLE setting_overrides (
        setting  TEXT NOT NULL,
        level    TEXT NOT NULL,
        subject  TEXT NOT NULL,
        revision INTEGER NOT NULL,
        data     TEXT NOT NULL,
        PRIMARY KEY (setting, level, subject)
    );
    CREATE TABLE setting_revisions (
        setting  TEXT PRIMARY KEY,
        revision INTEGER NOT NULL
    );
    ",
    },
    Migration {
        version: 13,
        name: "automation worker jobs, runs, steps and exceptions",
        // A row's columns are what the statements look it up by; `data` is the whole value as
        // canonical JSON. A leased job's `due_at` is the expiry of its lease, so one index finds
        // both the due jobs and the expired leases (SCH-001, SCH-010).
        sql: "
    CREATE TABLE jobs (
        seq    INTEGER PRIMARY KEY AUTOINCREMENT,
        id     TEXT NOT NULL UNIQUE,
        key    TEXT NOT NULL UNIQUE,
        state  TEXT NOT NULL,
        due_at INTEGER NOT NULL,
        data   TEXT NOT NULL
    );
    CREATE INDEX jobs_due ON jobs(due_at) WHERE state IN ('waiting', 'leased');
    CREATE TABLE automation_runs (
        seq        INTEGER PRIMARY KEY AUTOINCREMENT,
        run        TEXT NOT NULL UNIQUE,
        automation TEXT NOT NULL,
        state      TEXT NOT NULL,
        data       TEXT NOT NULL
    );
    CREATE TABLE automation_steps (
        run      TEXT NOT NULL,
        sequence INTEGER NOT NULL,
        data     TEXT NOT NULL,
        PRIMARY KEY (run, sequence)
    );
    CREATE TABLE automation_exceptions (
        seq     INTEGER PRIMARY KEY AUTOINCREMENT,
        id      TEXT NOT NULL UNIQUE,
        kind    TEXT NOT NULL,
        settled INTEGER NOT NULL,
        data    TEXT NOT NULL
    );
    ",
    },
    ledger::MIGRATION,
    Migration {
        version: 15,
        name: "segments: definitions, memberships, wake rows, batches, erased persons and events",
        // The segment block (SEG-001 to SEG-038), apart from the tenant state: a segment row holds
        // the definition as canonical JSON with its version and kept member count; a membership
        // holds the segment, the person, the version and the time of entry and no field of the
        // person (SEG-034); the rows of a segment go with it (SEG-026).
        sql: "
    CREATE TABLE segments (
        id           TEXT PRIMARY KEY,
        name         TEXT NOT NULL UNIQUE,
        version      INTEGER NOT NULL,
        member_count INTEGER NOT NULL DEFAULT 0,
        data         TEXT NOT NULL
    );
    CREATE TABLE segment_memberships (
        segment   TEXT NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
        principal TEXT NOT NULL,
        version   INTEGER NOT NULL,
        since     INTEGER NOT NULL,
        PRIMARY KEY (segment, principal)
    );
    CREATE INDEX segment_memberships_principal ON segment_memberships(principal);
    CREATE TABLE segment_wakes (
        segment   TEXT NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
        principal TEXT NOT NULL,
        wake_at   INTEGER NOT NULL,
        PRIMARY KEY (segment, principal)
    );
    CREATE INDEX segment_wake_due ON segment_wakes(segment, wake_at);
    CREATE INDEX segment_wakes_principal ON segment_wakes(principal);
    CREATE TABLE segment_batches (
        segment TEXT NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
        scope   TEXT NOT NULL,
        data    TEXT NOT NULL,
        PRIMARY KEY (segment, scope)
    );
    CREATE TABLE segment_erased (
        principal TEXT PRIMARY KEY
    );
    CREATE TABLE segment_events (
        position  INTEGER PRIMARY KEY AUTOINCREMENT,
        data      TEXT NOT NULL,
        published INTEGER NOT NULL DEFAULT 0
    );
    ",
    },
    Migration {
        version: 16,
        name: "order keys of the record index",
        sql: index_page::ORDER_KEYS_SQL,
    },
    Migration {
        version: 17,
        name: "pair index of named queries",
        sql: pair_index::PAIR_INDEX_SQL,
    },
    // The number of this step is the coordinator's to assign.
    Migration {
        version: 17,
        name: "search entries",
        sql: search_index::SEARCH_ENTRIES_SQL,
    },
];

/// The steps of a tenant file's layout, for tests that build a file at an older level.
#[doc(hidden)]
pub fn tenant_migration_steps() -> &'static [Migration] {
    TENANT_MIGRATIONS
}

/// Index values longer than this are not indexed; lookups for them scan the object type. The
/// kernel's query planner knows the same limit.
const MAX_INDEXED: usize = strata_kernel::query_plan::INDEX_KEY_BYTES;

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

/// The default bound on open tenant files.
pub const DEFAULT_MAX_OPEN: usize = 256;
/// How many outbox problems are remembered.
const MAX_FAULTS: usize = 200;
/// `events.published` of an event that could not be decoded: neither waiting nor published.
const PARKED: i64 = 2;

/// Implements: H-008
pub struct SqliteStore {
    control: Database,
    place: Place,
    /// Open tenant files, by tenant. In memory this is where the tenant databases live.
    open: Mutex<BTreeMap<TenantId, Database>>,
    /// When each open tenant file was last used, by a counter; the least recently used file is
    /// closed when more than `max_open` are open.
    recency: Mutex<BTreeMap<TenantId, u64>>,
    clock: AtomicU64,
    /// The most tenant files kept open on disk. In memory the databases are the data and are
    /// never closed.
    max_open: usize,
    /// One gate per tenant being opened, so a cold open blocks only callers of that tenant.
    opening: Mutex<BTreeMap<TenantId, Arc<Mutex<()>>>>,
    /// Where the next outbox sweep starts among the tenants.
    sweep: AtomicUsize,
    /// Outbox problems skipped past, most recent last.
    faults: Mutex<Vec<OutboxFault>>,
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
        setting_values: setting_values(tx)?,
        ledger: None,
    }))
}

/// The setting overrides and the setting revisions of the tenant in an open tenant file, none
/// when there is neither (SET-051). They are loaded with the header: the kernel decides a write
/// and resolves a read from the whole of them.
fn setting_values(tx: &Connection) -> StoreResult<Option<Arc<SettingValues>>> {
    let rows = select_strings(
        tx,
        "SELECT data FROM setting_overrides ORDER BY setting, level, subject",
        &[],
    )?;
    let mut overrides = BTreeMap::new();
    for data in rows {
        let held: SettingOverride = decode(&data, "setting override")?;
        overrides.insert(
            strata_kernel::settings::override_key(&held.setting, held.level, &held.subject),
            held,
        );
    }
    let mut statement = tx
        .prepare_cached("SELECT setting, revision FROM setting_revisions")
        .map_err(db)?;
    let stored = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(db)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db)?;
    let mut revisions = BTreeMap::new();
    for (setting, revision) in stored {
        revisions.insert(
            SettingId::new(setting).map_err(corrupt_id)?,
            revision as u64,
        );
    }
    Ok((!overrides.is_empty() || !revisions.is_empty()).then(|| {
        Arc::new(SettingValues {
            overrides,
            revisions,
        })
    }))
}

/// Write one setting override, replacing the stored one of its setting, level and subject.
fn write_setting_override(tx: &Connection, held: &SettingOverride) -> StoreResult<()> {
    tx.execute(
        "INSERT INTO setting_overrides (setting, level, subject, revision, data)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (setting, level, subject)
         DO UPDATE SET revision = excluded.revision, data = excluded.data",
        params![
            held.setting.as_str(),
            held.level.as_str(),
            held.subject,
            held.revision as i64,
            encode(held)
        ],
    )
    .map_err(db)?;
    Ok(())
}

/// Write the revision of one setting, replacing the stored one.
fn write_setting_revision(tx: &Connection, setting: &SettingId, revision: u64) -> StoreResult<()> {
    tx.execute(
        "INSERT INTO setting_revisions (setting, revision) VALUES (?1, ?2)
         ON CONFLICT (setting) DO UPDATE SET revision = excluded.revision",
        params![setting.as_str(), revision as i64],
    )
    .map_err(db)?;
    Ok(())
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

/// The records of `object` whose every field in `fields` holds its value: the lookup of a
/// composite unique constraint (D-6558, CMS-1557). Answered from the record index, driven by the
/// first field and narrowed by the others, when every value fits the index; otherwise by reading
/// the records of the type and keeping the matching ones.
fn by_fields(
    tx: &Connection,
    object: &ObjectTypeId,
    fields: &BTreeMap<strata_kernel::FieldId, Value>,
) -> StoreResult<Vec<String>> {
    let keys: Vec<(&str, String)> = fields
        .iter()
        .map(|(field, value)| (field.as_str(), encode(value)))
        .collect();
    if keys.is_empty() || keys.iter().any(|(_, key)| key.len() > MAX_INDEXED) {
        let mut matching = Vec::new();
        for data in all_of(tx, object)? {
            let record: Record = decode(&data, "record")?;
            if fields
                .iter()
                .all(|(field, value)| field_matches(&record, field, value))
            {
                matching.push(data);
            }
        }
        return Ok(matching);
    }
    let mut sql = String::from(
        "SELECT r.data FROM record_index i0
         JOIN records r ON r.object = i0.object AND r.id = i0.id
         WHERE i0.object = ?1 AND i0.field = ?2 AND i0.value = ?3",
    );
    for n in 1..keys.len() {
        let (f, v) = (2 * n + 2, 2 * n + 3);
        sql.push_str(&format!(
            " AND EXISTS (SELECT 1 FROM record_index i WHERE i.object = i0.object AND i.id = i0.id AND i.field = ?{f} AND i.value = ?{v})"
        ));
    }
    sql.push_str(" ORDER BY r.id");
    let mut owned: Vec<String> = vec![object.as_str().to_string()];
    for (field, key) in keys {
        owned.push(field.to_string());
        owned.push(key);
    }
    let args: Vec<&dyn rusqlite::ToSql> = owned
        .iter()
        .map(|text| text as &dyn rusqlite::ToSql)
        .collect();
    select_strings(tx, &sql, &args)
}

fn complete(conn: &Connection, tenant: &TenantId) -> StoreResult<Option<TenantState>> {
    let Some(mut state) = header(conn, tenant)? else {
        return Ok(None);
    };
    let rows = select_strings(conn, "SELECT data FROM records ORDER BY object, id", &[])?;
    put_records(&mut state, rows)?;
    ledger::load_all(conn, &mut state)?;
    ledger::tidy(&mut state);
    Ok(Some(state))
}

fn write_record(tx: &Transaction<'_>, record: &Record) -> StoreResult<()> {
    let (object, id) = (record.object.as_str(), record.id.0 as i64);
    // A record is created at version 1 and every later write raises its version, so a version 1
    // write over a stored row would overwrite a record that already exists.
    if record.version == 1 {
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM records WHERE object = ?1 AND id = ?2)",
                params![object, id],
                |r| r.get(0),
            )
            .map_err(db)?;
        if exists {
            return Err(StoreError::Invalid(format!(
                "create would overwrite an existing record {id} of {object}"
            )));
        }
    }
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
    index_page::insert_rows(tx, record)?;
    pair_index::write_rows(tx, record)
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

// ---- Jobs of the automation worker (strata_storage_api::jobs) ----

/// The states of a job that has not ended, as the statements test them.
const OPEN_JOB: &str = "state IN ('waiting', 'leased')";

/// The first problem of a list of jobs to create, as a refusal.
fn check_jobs(new: &[NewJob]) -> StoreResult<()> {
    match new.iter().find_map(jobs::job_problem) {
        Some(problem) => Err(StoreError::Invalid(problem)),
        None => Ok(()),
    }
}

/// Create the jobs whose keys the file does not hold yet; the ids of all, in order.
fn insert_jobs(
    tx: &Connection,
    tenant: &TenantId,
    new: &[NewJob],
    now: Timestamp,
) -> StoreResult<Vec<String>> {
    check_jobs(new)?;
    let mut ids = Vec::with_capacity(new.len());
    for wanted in new {
        let job = jobs::new_job(tenant, wanted, now);
        tx.execute(
            "INSERT OR IGNORE INTO jobs (id, key, state, due_at, data) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                job.id,
                job.key,
                job.state.as_str(),
                job.due_at.0,
                encode(&job)
            ],
        )
        .map_err(db)?;
        ids.push(job.id);
    }
    Ok(ids)
}

/// Write a changed job back.
fn put_job(tx: &Connection, job: &Job) -> StoreResult<()> {
    tx.execute(
        "UPDATE jobs SET state = ?2, due_at = ?3, data = ?4 WHERE id = ?1",
        params![job.id, job.state.as_str(), job.due_at.0, encode(job)],
    )
    .map_err(db)?;
    Ok(())
}

/// The rows of `(seq, data)` a statement selects.
fn seq_rows(
    tx: &Connection,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> StoreResult<Vec<(u64, String)>> {
    let mut statement = tx.prepare_cached(sql).map_err(db)?;
    let rows = statement
        .query_map(args, |row| {
            Ok((row.get::<_, i64>(0)? as u64, row.get::<_, String>(1)?))
        })
        .map_err(db)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db)
}

/// The jobs a statement over `(seq, data)` selects.
fn jobs_of(tx: &Connection, sql: &str, args: &[&dyn rusqlite::ToSql]) -> StoreResult<Vec<Job>> {
    seq_rows(tx, sql, args)?
        .into_iter()
        .map(|(seq, data)| {
            let mut job: Job = decode(&data, "job")?;
            job.seq = seq;
            Ok(job)
        })
        .collect()
}

fn job_by_id(tx: &Connection, id: &str) -> StoreResult<Option<Job>> {
    Ok(jobs_of(tx, "SELECT seq, data FROM jobs WHERE id = ?1", &[&id])?.pop())
}

fn runs_of(
    tx: &Connection,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> StoreResult<Vec<RunRecord>> {
    seq_rows(tx, sql, args)?
        .into_iter()
        .map(|(seq, data)| {
            let mut run: RunRecord = decode(&data, "automation run")?;
            run.seq = seq;
            Ok(run)
        })
        .collect()
}

fn exceptions_of(
    tx: &Connection,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> StoreResult<Vec<ExceptionRecord>> {
    seq_rows(tx, sql, args)?
        .into_iter()
        .map(|(seq, data)| {
            let mut record: ExceptionRecord = decode(&data, "automation exception")?;
            record.seq = seq;
            Ok(record)
        })
        .collect()
}

/// Exception records whose ids the file does not hold yet.
fn insert_exceptions(tx: &Connection, records: &[ExceptionRecord]) -> StoreResult<()> {
    for record in records {
        tx.execute(
            "INSERT OR IGNORE INTO automation_exceptions (id, kind, settled, data) VALUES (?1, ?2, ?3, ?4)",
            params![
                record.id,
                record.kind.as_str(),
                i64::from(record.settled),
                encode(record)
            ],
        )
        .map_err(db)?;
    }
    Ok(())
}

/// Everything of `writes`, as `Store::automation_write` states, in the caller's transaction.
fn write_automation(
    tx: &Connection,
    tenant: &TenantId,
    writes: &WorkerWrites,
    now: Timestamp,
) -> StoreResult<()> {
    for run in &writes.runs {
        tx.execute(
            "INSERT INTO automation_runs (run, automation, state, data) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(run) DO UPDATE SET automation = excluded.automation, state = excluded.state, data = excluded.data",
            params![
                run.run,
                run.automation.as_str(),
                run.state.as_str(),
                encode(run)
            ],
        )
        .map_err(db)?;
    }
    for step in &writes.steps {
        tx.execute(
            "INSERT OR REPLACE INTO automation_steps (run, sequence, data) VALUES (?1, ?2, ?3)",
            params![step.run, step.sequence as i64, encode(step)],
        )
        .map_err(db)?;
    }
    insert_exceptions(tx, &writes.exceptions)?;
    insert_jobs(tx, tenant, &writes.jobs, now)?;
    Ok(())
}

/// The jobs and exception records of a commit, in its transaction (SCH-004, SCH-1532).
fn insert_commit_work(tx: &Connection, commit: &Commit) -> StoreResult<()> {
    insert_jobs(
        tx,
        &commit.delta.tenant,
        &commit.jobs,
        commit.journal.context.now,
    )?;
    insert_exceptions(tx, &commit.exceptions)
}

impl SqliteStore {
    /// Lower the wake time of a tenant to `due` (SCH-008: the index holds a tenant id and a due
    /// time, and a counter, never anything of a job).
    fn wake_at(&self, tenant: &TenantId, due: i64) -> StoreResult<()> {
        self.control.write(dbe, |tx| {
            tx.execute(
                "INSERT INTO job_wake (tenant, due_at, touched) VALUES (?1, ?2, 1)
                 ON CONFLICT(tenant) DO UPDATE SET due_at = MIN(due_at, excluded.due_at), touched = touched + 1",
                params![tenant.as_str(), due],
            )
            .map_err(db)?;
            Ok(())
        })
    }

    /// Set the wake time of a tenant to the earliest time a job of its file is due, or remove it
    /// when none is open. The counter is read first and the write made only if no other writer
    /// raised it since, so a time lowered by a job created meanwhile is never overwritten.
    fn wake_refresh(&self, tenant: &TenantId, file: &Database) -> StoreResult<()> {
        let seen: Option<i64> = self
            .control
            .lock()
            .map_err(dbe)?
            .query_row(
                "SELECT touched FROM job_wake WHERE tenant = ?1",
                params![tenant.as_str()],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)?;
        let next: Option<i64> = file
            .lock()
            .map_err(dbe)?
            .query_row(
                &format!("SELECT MIN(due_at) FROM jobs WHERE {OPEN_JOB}"),
                [],
                |r| r.get::<_, Option<i64>>(0),
            )
            .map_err(db)?;
        match (seen, next) {
            (None, None) => Ok(()),
            (None, Some(due)) => self.wake_at(tenant, due),
            (Some(touched), None) => self.control.write(dbe, |tx| {
                tx.execute(
                    "DELETE FROM job_wake WHERE tenant = ?1 AND touched = ?2",
                    params![tenant.as_str(), touched],
                )
                .map_err(db)?;
                Ok(())
            }),
            (Some(touched), Some(due)) => self.control.write(dbe, |tx| {
                tx.execute(
                    "UPDATE job_wake SET due_at = ?2, touched = touched + 1 WHERE tenant = ?1 AND touched = ?3",
                    params![tenant.as_str(), due, touched],
                )
                .map_err(db)?;
                Ok(())
            }),
        }
    }

    /// Run `work` in a write transaction of the file of a tenant, then refresh the tenant's wake
    /// time; `None` when the tenant has no file. When the work may create jobs, `creates` is the
    /// earliest time one is due: the wake time is lowered to it before the work, so that a crash
    /// between the work and the refresh cannot leave a job that nothing wakes for. A wake time
    /// that is too early costs one look at the file and is put right by that look, so a failed
    /// refresh is not an error of the work.
    fn on_jobs<T>(
        &self,
        tenant: &TenantId,
        creates: Option<i64>,
        work: impl FnOnce(&Transaction<'_>) -> StoreResult<T>,
    ) -> StoreResult<Option<T>> {
        let Some(file) = self.tenant_db(tenant)? else {
            return Ok(None);
        };
        if let Some(due) = creates {
            self.wake_at(tenant, due)?;
        }
        let done = file.write(dbe, work)?;
        let _ = self.wake_refresh(tenant, &file);
        Ok(Some(done))
    }
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
            recency: Mutex::new(BTreeMap::new()),
            clock: AtomicU64::new(0),
            max_open: DEFAULT_MAX_OPEN,
            opening: Mutex::new(BTreeMap::new()),
            sweep: AtomicUsize::new(0),
            faults: Mutex::new(Vec::new()),
            opened_at: now,
        })
    }

    /// Keep at most `max` tenant files open (at least one); the least recently used is closed.
    pub fn with_max_open(mut self, max: usize) -> Self {
        self.max_open = max.max(1);
        self
    }

    /// How many tenant files are open now.
    pub fn open_count(&self) -> usize {
        self.open.lock().map(|open| open.len()).unwrap_or(0)
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
        // A panic while the map was held leaves it whole (it is only inserted into and cleared),
        // so the next caller takes it instead of every call failing until restart.
        Ok(self
            .open
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()))
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
        // The ledger records when the step ran, which is now and not when the process started.
        let ran_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(self.opened_at.0, |d| d.as_millis() as i64);
        db.migrate(TENANT_LEDGER, TENANT_MIGRATIONS, ran_at)
            .map_err(dbe)?;
        db.write(dbe, index_page::rebuild_if_marked)?;
        db.write(dbe, pair_index::sync_stored)?;
        db.write(dbe, search_index::rebuild_if_marked)?;
        Ok(db)
    }

    /// The open database of an active tenant, or `None` when the directory has no such tenant.
    /// The map lock is held only to look up and to insert, never across the control query, the
    /// file open or the migration; a cold open waits on its own tenant's gate only.
    fn tenant_db(&self, tenant: &TenantId) -> StoreResult<Option<Database>> {
        if let Some(found) = self.cached(tenant)? {
            return Ok(Some(found));
        }
        // Another process may have created the tenant since; in memory nothing else can.
        if matches!(self.place, Place::Memory) {
            return Ok(None);
        }
        let gate = self
            .opening
            .lock()
            .map_err(|_| StoreError::Unavailable("a writer panicked".into()))?
            .entry(tenant.clone())
            .or_default()
            .clone();
        let result = (|| -> StoreResult<Option<Database>> {
            let _turn = gate
                .lock()
                .map_err(|_| StoreError::Unavailable("a writer panicked".into()))?;
            if let Some(found) = self.cached(tenant)? {
                return Ok(Some(found));
            }
            let Some(file) = self.directory_file(tenant, "active")? else {
                return Ok(None);
            };
            let found = self.open_file(&file)?;
            let mut open = self.lock_open()?;
            let kept = open.entry(tenant.clone()).or_insert(found).clone();
            self.touch(tenant);
            self.evict(&mut open, tenant);
            Ok(Some(kept))
        })();
        if let Ok(mut opening) = self.opening.lock() {
            opening.remove(tenant);
        }
        result
    }

    fn cached(&self, tenant: &TenantId) -> StoreResult<Option<Database>> {
        let open = self.lock_open()?;
        let found = open.get(tenant).cloned();
        if found.is_some() {
            self.touch(tenant);
        }
        Ok(found)
    }

    fn touch(&self, tenant: &TenantId) {
        let tick = self.clock.fetch_add(1, Ordering::Relaxed) + 1;
        if let Ok(mut recency) = self.recency.lock() {
            recency.insert(tenant.clone(), tick);
        }
    }

    /// Close the least recently used tenant files beyond the bound, never `keep`. Callers hold
    /// the `open` lock, so the order of locks is always `open` then `recency`.
    fn evict(&self, open: &mut BTreeMap<TenantId, Database>, keep: &TenantId) {
        if matches!(self.place, Place::Memory) {
            return;
        }
        let Ok(mut recency) = self.recency.lock() else {
            return;
        };
        recency.retain(|tenant, _| open.contains_key(tenant));
        while open.len() > self.max_open {
            let oldest = open
                .keys()
                .filter(|tenant| *tenant != keep)
                .min_by_key(|tenant| recency.get(*tenant).copied().unwrap_or(0))
                .cloned();
            let Some(oldest) = oldest else {
                break;
            };
            open.remove(&oldest);
            recency.remove(&oldest);
        }
    }

    /// Remember an outbox problem, once.
    fn fault(&self, tenant: &TenantId, position: Option<u64>, reason: String) {
        let Ok(mut faults) = self.faults.lock() else {
            return;
        };
        if faults
            .iter()
            .any(|f| f.tenant == *tenant && f.position == position && f.reason == reason)
        {
            return;
        }
        if faults.len() >= MAX_FAULTS {
            faults.remove(0);
        }
        faults.push(OutboxFault {
            tenant: tenant.clone(),
            position,
            reason,
        });
    }

    /// Up to `room` (at least one) waiting events of one tenant after outbox position `after`.
    /// An event that cannot be decoded is parked and reported; a tenant that cannot be read is
    /// skipped and reported. Returns the events and whether the tenant may have more.
    fn outbox_of(&self, tenant: &TenantId, after: u64, room: usize) -> (Vec<OutboxEntry>, bool) {
        let read = || -> StoreResult<(Vec<OutboxEntry>, bool)> {
            let Some(file) = self.tenant_db(tenant)? else {
                return Ok((Vec::new(), false));
            };
            let conn = file.lock().map_err(dbe)?;
            let mut out: Vec<OutboxEntry> = Vec::new();
            let mut cursor = after as i64;
            loop {
                let want = room - out.len();
                let mut statement = conn
                    .prepare_cached(
                        "SELECT position, data FROM events WHERE published = 0 AND position > ?1 ORDER BY position LIMIT ?2",
                    )
                    .map_err(db)?;
                let rows = statement
                    .query_map(params![cursor, want as i64], |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(db)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(db)?;
                drop(statement);
                let fetched = rows.len();
                for (position, data) in rows {
                    cursor = position;
                    match decode::<DomainEvent>(&data, "event") {
                        Ok(event) => out.push(OutboxEntry {
                            position: position as u64,
                            event,
                        }),
                        Err(error) => {
                            conn.execute(
                                "UPDATE events SET published = ?2 WHERE position = ?1 AND published = 0",
                                params![position, PARKED],
                            )
                            .map_err(db)?;
                            self.fault(tenant, Some(position as u64), error.to_string());
                        }
                    }
                }
                if out.len() >= room || fetched < want {
                    let capped = out.len() >= room;
                    return Ok((out, capped));
                }
            }
        };
        match read() {
            Ok(found) => found,
            Err(error) => {
                self.fault(tenant, None, error.to_string());
                (Vec::new(), false)
            }
        }
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
        // A `deleting` row is a removal a previous process did not finish, and a `provisioning`
        // row is an install it did not finish: either way what is there is cleared first.
        if self.directory_file(tenant, "deleting")?.is_some()
            || self.directory_file(tenant, "provisioning")?.is_some()
        {
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

    /// Move a tenant's directory row from one state to another. The tenant must be in `from`.
    fn set_directory_state(&self, tenant: &TenantId, from: &str, to: &str) -> StoreResult<()> {
        let changed = self.control.write(dbe, |tx| {
            tx.execute(
                "UPDATE tenant_directory SET state = ?3 WHERE id = ?1 AND state = ?2",
                params![tenant.as_str(), from, to],
            )
            .map_err(db)
        })?;
        if changed == 0 {
            return Err(StoreError::NotFound(format!(
                "tenant {tenant} in state {from}"
            )));
        }
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

    /// Write every search entry of a tenant again from its records and schema, in one transaction.
    /// The entries are kept current by every commit, so this changes nothing unless a file was
    /// altered outside the store; it states that the index is a function of the records alone.
    pub fn rebuild_search_index(&self, tenant: &TenantId) -> StoreResult<()> {
        self.existing(tenant)?.write(dbe, search_index::rebuild_all)
    }

    /// The search entries stored for one record, as `field:token`, in order: what the commit that
    /// wrote the record left in the index.
    pub fn search_entries_of(
        &self,
        tenant: &TenantId,
        object: &strata_kernel::ObjectTypeId,
        id: strata_kernel::RecordId,
    ) -> StoreResult<Vec<String>> {
        self.on_existing(tenant, |conn| search_index::stored_entries(conn, object, id))
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
        // Defence in depth: the kernel produces these today, but a bad delta must not reach the
        // tables. A tenant's revision advances by exactly one, and its record counter never falls.
        let base = delta.base_revision.map_or(0, |r| r.0);
        if delta.new_revision.0 != base + 1 {
            return Err(StoreError::Invalid(
                "delta does not advance the revision by one".into(),
            ));
        }
        if let Some(stored) = current.as_ref() {
            if delta.next_record < stored.next_record {
                return Err(StoreError::Invalid(
                    "delta moves the record counter backwards".into(),
                ));
            }
        }

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
                DeltaOp::PutRecord(_)
                | DeltaOp::DeleteRecord { .. }
                | DeltaOp::PutSettingOverride(_)
                | DeltaOp::DeleteSettingOverride { .. }
                | DeltaOp::PutSettingRevision { .. }
                | DeltaOp::Ledger(_) => {}
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
        // The search entries follow the schema that is in force after this commit: the entries of
        // a type whose searchable fields change are rebuilt before the commit's own records are
        // written under the new schema.
        let search_schema: Arc<TenantSchema> = match (&schema, &current) {
            (Some(next), Some(held)) => {
                search_index::reconcile(tx, &held.schema, next)?;
                next.clone()
            }
            (Some(next), None) => next.clone(),
            (None, Some(held)) => held.schema.clone(),
            (None, None) => Arc::new(TenantSchema::genesis()),
        };
        if let Some(schema) = &schema {
            // A schema can change without a new version: teams and their roles live in it and do
            // not change the shape of records. The row of a version keeps the revision the
            // version began at and holds the latest schema at that version.
            tx.execute(
                "INSERT INTO schema_history (version, revision, schema) VALUES (?1, ?2, ?3)                  ON CONFLICT(version) DO UPDATE SET schema = excluded.schema",
                params![
                    schema.version.0 as i64,
                    delta.new_revision.0 as i64,
                    encode(&**schema)
                ],
            )
            .map_err(db)?;
            pair_index::sync(tx, schema)?;
        }
        for op in &delta.ops {
            match op {
                DeltaOp::PutRecord(record) => {
                    write_record(tx, record)?;
                    search_index::write_entries(tx, &search_schema, record)?;
                }
                DeltaOp::DeleteRecord { object, id } => {
                    tx.execute(
                        "DELETE FROM records WHERE object = ?1 AND id = ?2",
                        params![object.as_str(), id.0 as i64],
                    )
                    .map_err(db)?;
                    pair_index::delete_rows(tx, object, id.0)?;
                }
                DeltaOp::PutSettingOverride(held) => write_setting_override(tx, held)?,
                DeltaOp::DeleteSettingOverride {
                    setting,
                    level,
                    subject,
                } => {
                    tx.execute(
                        "DELETE FROM setting_overrides WHERE setting = ?1 AND level = ?2 AND subject = ?3",
                        params![setting.as_str(), level.as_str(), subject],
                    )
                    .map_err(db)?;
                }
                DeltaOp::PutSettingRevision { setting, revision } => {
                    write_setting_revision(tx, setting, *revision)?
                }
                DeltaOp::Ledger(change) => ledger::apply(tx, change, delta.new_revision)?,
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
        // A key is kept for its retention, counted from the commit that recorded it; the rows
        // past it go in the same transaction as the next commit.
        tx.execute(
            "DELETE FROM idempotency WHERE recorded_at < ?1",
            params![commit.journal.context.now.0 - IDEMPOTENCY_RETENTION_MS],
        )
        .map_err(db)?;
        if let Some(entry) = &commit.idempotency {
            tx.execute(
                "INSERT INTO idempotency (principal, key, request_hash, recorded_at, revision, last_position, result)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    entry.principal,
                    entry.key,
                    entry.request_hash,
                    entry.recorded_at.0,
                    delta.new_revision.0 as i64,
                    last_position.map(|p| p as i64),
                    encode(&entry.result)
                ],
            )
            .map_err(db)?;
        }
        insert_commit_work(tx, commit)?;
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

// ---------------------------------------------------------------------------------------------
// The segment block (tenant migration 15)
// ---------------------------------------------------------------------------------------------

fn segment_id(text: &str) -> StoreResult<seg::SegmentId> {
    serde_json::from_value(serde_json::Value::String(text.to_string()))
        .map_err(|e| StoreError::Corrupt(format!("segment id {text}: {e}")))
}

fn principal(text: &str) -> StoreResult<PrincipalId> {
    PrincipalId::new(text).map_err(corrupt_id)
}

fn catalog_rows(conn: &Connection) -> StoreResult<Vec<seg::Segment>> {
    let mut statement = conn
        .prepare_cached("SELECT data, member_count FROM segments ORDER BY id")
        .map_err(db)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(db)?;
    let mut out = Vec::new();
    for row in rows {
        let (data, count) = row.map_err(db)?;
        let mut segment: seg::Segment = decode(&data, "segment")?;
        segment.member_count = count.max(0) as u64;
        out.push(segment);
    }
    Ok(out)
}

fn membership_of(
    conn: &Connection,
    segment: seg::SegmentId,
    person: &PrincipalId,
) -> StoreResult<Option<seg::Membership>> {
    conn.query_row(
        "SELECT version, since FROM segment_memberships WHERE segment = ?1 AND principal = ?2",
        params![segment.to_string(), person.as_str()],
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
    )
    .optional()
    .map_err(db)
    .map(|found| {
        found.map(|(version, since)| seg::Membership {
            segment,
            person: person.clone(),
            version: version as u64,
            since: Timestamp(since),
        })
    })
}

fn wake_of(
    conn: &Connection,
    segment: seg::SegmentId,
    person: &PrincipalId,
) -> StoreResult<Option<Timestamp>> {
    conn.query_row(
        "SELECT wake_at FROM segment_wakes WHERE segment = ?1 AND principal = ?2",
        params![segment.to_string(), person.as_str()],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .map_err(db)
    .map(|found| found.map(Timestamp))
}

fn memberships_where(
    conn: &Connection,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> StoreResult<Vec<seg::Membership>> {
    let mut statement = conn.prepare_cached(sql).map_err(db)?;
    let rows = statement
        .query_map(args, |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })
        .map_err(db)?;
    let mut out = Vec::new();
    for row in rows {
        let (segment, person, version, since) = row.map_err(db)?;
        out.push(seg::Membership {
            segment: segment_id(&segment)?,
            person: principal(&person)?,
            version: version as u64,
            since: Timestamp(since),
        });
    }
    Ok(out)
}

fn wakes_where(
    conn: &Connection,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> StoreResult<Vec<seg::Wake>> {
    let mut statement = conn.prepare_cached(sql).map_err(db)?;
    let rows = statement
        .query_map(args, |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })
        .map_err(db)?;
    let mut out = Vec::new();
    for row in rows {
        let (segment, person, at) = row.map_err(db)?;
        out.push(seg::Wake {
            segment: segment_id(&segment)?,
            person: principal(&person)?,
            at: Timestamp(at),
        });
    }
    Ok(out)
}

/// Apply a segment write in the transaction `tx`: refused whole, as a conflict, when a segment or
/// a row it rests on is not what the decision saw (the transaction then rolls back).
fn apply_segments(tx: &Connection, write: &seg::SegmentWrite) -> StoreResult<()> {
    for (id, expected) in &write.expect {
        let found: Option<i64> = tx
            .query_row(
                "SELECT version FROM segments WHERE id = ?1",
                params![id.to_string()],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)?;
        if found.map(|v| v as u64) != *expected {
            return Err(write_conflict());
        }
    }
    for change in &write.memberships {
        if membership_of(tx, change.segment, &change.person)? != change.before {
            return Err(write_conflict());
        }
    }
    for seen in &write.seen {
        if membership_of(tx, seen.segment, &seen.person)? != seen.membership
            || wake_of(tx, seen.segment, &seen.person)? != seen.wake
        {
            return Err(write_conflict());
        }
    }
    for segment in &write.put_segments {
        tx.execute(
            "INSERT INTO segments (id, name, version, member_count, data) VALUES (?1, ?2, ?3, 0, ?4)
             ON CONFLICT (id) DO UPDATE SET name = excluded.name, version = excluded.version, data = excluded.data",
            params![
                segment.id.to_string(),
                segment.name,
                segment.version as i64,
                encode(segment)
            ],
        )
        .map_err(db)?;
    }
    for id in &write.remove_segments {
        tx.execute(
            "DELETE FROM segments WHERE id = ?1",
            params![id.to_string()],
        )
        .map_err(db)?;
    }
    for change in &write.memberships {
        let (segment, person) = (change.segment.to_string(), change.person.as_str());
        let delta: i64 = match (&change.before, &change.after) {
            (None, Some(_)) => 1,
            (Some(_), None) => -1,
            _ => 0,
        };
        match &change.after {
            Some(m) => {
                tx.execute(
                    "INSERT INTO segment_memberships (segment, principal, version, since) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (segment, principal) DO UPDATE SET version = excluded.version, since = excluded.since",
                    params![segment, person, m.version as i64, m.since.0],
                )
                .map_err(db)?;
            }
            None => {
                tx.execute(
                    "DELETE FROM segment_memberships WHERE segment = ?1 AND principal = ?2",
                    params![segment, person],
                )
                .map_err(db)?;
            }
        }
        if delta != 0 {
            tx.execute(
                "UPDATE segments SET member_count = MAX(member_count + ?1, 0) WHERE id = ?2",
                params![delta, segment],
            )
            .map_err(db)?;
        }
    }
    for change in &write.wakes {
        match change {
            seg::WakeChange::Put(w) => tx.execute(
                "INSERT INTO segment_wakes (segment, principal, wake_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (segment, principal) DO UPDATE SET wake_at = excluded.wake_at",
                params![w.segment.to_string(), w.person.as_str(), w.at.0],
            ),
            seg::WakeChange::Remove { segment, person } => tx.execute(
                "DELETE FROM segment_wakes WHERE segment = ?1 AND principal = ?2",
                params![segment.to_string(), person.as_str()],
            ),
        }
        .map_err(db)?;
    }
    for id in &write.clear_batches {
        tx.execute(
            "DELETE FROM segment_batches WHERE segment = ?1",
            params![id.to_string()],
        )
        .map_err(db)?;
    }
    for change in &write.batches {
        match change {
            seg::BatchChange::Put(b) => tx.execute(
                "INSERT INTO segment_batches (segment, scope, data)
                 SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM segments WHERE id = ?1)
                 ON CONFLICT (segment, scope) DO UPDATE SET data = excluded.data",
                params![b.segment.to_string(), b.scope.key(), encode(b)],
            ),
            seg::BatchChange::Remove { segment, key } => tx.execute(
                "DELETE FROM segment_batches WHERE segment = ?1 AND scope = ?2",
                params![segment.to_string(), key],
            ),
        }
        .map_err(db)?;
    }
    for person in &write.erased {
        tx.execute(
            "INSERT OR IGNORE INTO segment_erased (principal) VALUES (?1)",
            params![person.as_str()],
        )
        .map_err(db)?;
    }
    for (id, at) in &write.refresh_skipped {
        let data: Option<String> = tx
            .query_row(
                "SELECT data FROM segments WHERE id = ?1",
                params![id.to_string()],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)?;
        if let Some(data) = data {
            let mut segment: seg::Segment = decode(&data, "segment")?;
            segment.refresh_skipped_at = Some(*at);
            tx.execute(
                "UPDATE segments SET data = ?1 WHERE id = ?2",
                params![encode(&segment), id.to_string()],
            )
            .map_err(db)?;
        }
    }
    for event in &write.events {
        tx.execute(
            "INSERT INTO segment_events (data) VALUES (?1)",
            params![encode(event)],
        )
        .map_err(db)?;
    }
    Ok(())
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
            let checked = (|| -> StoreResult<Option<(String, bool, u64)>> {
                let Some(file) = self.tenant_db(&tenant)? else {
                    return Ok(None);
                };
                let conn = file.lock().map_err(dbe)?;
                let check = quick(&conn)?;
                let present = header(&conn, &tenant)?.is_some();
                let waiting: i64 = conn
                    .query_row("SELECT COUNT(*) FROM events WHERE published = 0", [], |r| {
                        r.get(0)
                    })
                    .map_err(db)?;
                Ok(Some((check, present, waiting as u64)))
            })();
            match checked {
                Ok(None) => {}
                Ok(Some((check, present, waiting))) => {
                    if check != "ok" {
                        ok = false;
                        detail.push_str(&format!("; {tenant} quick_check {check}"));
                    }
                    if present {
                        tenants += 1;
                    }
                    pending += waiting;
                }
                Err(error) => {
                    ok = false;
                    detail.push_str(&format!("; {tenant} unavailable: {error}"));
                }
            }
        }
        Ok(Health {
            ok,
            detail,
            tenants,
            outbox_pending: pending,
        })
    }

    fn liveness(&self) -> StoreResult<()> {
        self.control
            .lock()
            .map_err(dbe)?
            .query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
            .map_err(db)
            .map(|_| ())
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
            // A tenant that cannot be read is left out, not a failure for the others.
            if let Ok(Some(summary)) = self.tenant_summary(&tenant) {
                out.push(summary);
            }
        }
        Ok(out)
    }

    fn tenant_summary(&self, tenant: &TenantId) -> StoreResult<Option<TenantSummary>> {
        Ok(self
            .on_tenant(tenant, |conn| {
                let Some(state) = header(conn, tenant)? else {
                    return Ok(None);
                };
                let records: i64 = conn
                    .query_row("SELECT COUNT(*) FROM records", [], |r| r.get(0))
                    .map_err(db)?;
                Ok(Some(TenantSummary {
                    id: tenant.clone(),
                    name: state.name,
                    revision: state.revision,
                    schema_version: state.schema.version.0,
                    event_seq: state.event_seq,
                    records: records as u64,
                }))
            })?
            .flatten())
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
                    Need::PageOf {
                        object,
                        after,
                        limit,
                    } => select_strings(
                        tx,
                        "SELECT data FROM records WHERE object = ?1 AND id > ?2 ORDER BY id LIMIT ?3",
                        &[
                            &object.as_str(),
                            &after.map_or(-1_i64, |a| a.0 as i64),
                            &(*limit as i64),
                        ],
                    )?,
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
                    Need::ByFields { object, fields } => by_fields(tx, object, fields)?,
                    Need::Ledger(want) => {
                        ledger::load(tx, &mut state, want)?;
                        Vec::new()
                    }
                    Need::IndexPage {
                        object,
                        field,
                        lower,
                        upper,
                        descending,
                        after,
                        limit,
                    } => index_page::index_page(
                        tx,
                        object,
                        field,
                        lower,
                        upper,
                        *descending,
                        after.as_ref(),
                        *limit,
                    )?,
                    Need::PairPage {
                        object,
                        field,
                        value,
                        order,
                        descending,
                        after,
                        limit,
                    } => pair_index::pair_page(
                        tx,
                        object,
                        field,
                        value,
                        order,
                        *descending,
                        after.as_ref(),
                        *limit,
                    )?,
                };
                put_records(&mut state, rows)?;
            }
            ledger::tidy(&mut state);
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
            // Jobs the commit creates are woken for before and after it (see `on_jobs`).
            let wake = commit.jobs.iter().map(|job| job.due_at.0).min();
            if let Some(due) = wake {
                self.wake_at(&delta.tenant, due)?;
            }
            let receipt = file.write(dbe, |tx| Self::apply(tx, commit))?;
            if let Some(due) = wake {
                let _ = self.wake_at(&delta.tenant, due);
            }
            return Ok(receipt);
        }
        let (file, created) = self.create_tenant_db(&delta.tenant, commit.journal.context.now)?;
        let result = file.write(dbe, |tx| Self::apply(tx, commit));
        if let (Ok(_), Some(due)) = (&result, commit.jobs.iter().map(|job| job.due_at.0).min()) {
            let _ = self.wake_at(&delta.tenant, due);
        }
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

    fn commit_with_segments(
        &self,
        commit: &Commit,
        write: &seg::SegmentWrite,
    ) -> StoreResult<Receipt> {
        if write.is_empty() {
            return self.commit(commit);
        }
        // A commit with a segment write is never the one that creates its tenant: a tenant
        // without a file has no segment.
        let Some(file) = self.tenant_db(&commit.delta.tenant)? else {
            return Err(StoreError::Conflict {
                expected: commit.delta.base_revision,
                found: None,
            });
        };
        // Jobs the commit creates are woken for before and after it, as in `commit`.
        let wake = commit.jobs.iter().map(|job| job.due_at.0).min();
        if let Some(due) = wake {
            self.wake_at(&commit.delta.tenant, due)?;
        }
        let receipt = file.write(dbe, |tx| {
            let receipt = Self::apply(tx, commit)?;
            apply_segments(tx, write)?;
            Ok(receipt)
        })?;
        if let Some(due) = wake {
            let _ = self.wake_at(&commit.delta.tenant, due);
        }
        Ok(receipt)
    }

    fn commit_segments(&self, commit: &SegmentCommit) -> StoreResult<Receipt> {
        let Some(file) = self.tenant_db(&commit.tenant)? else {
            return Err(StoreError::Conflict {
                expected: Some(commit.revision),
                found: None,
            });
        };
        file.write(dbe, |tx| {
            let found = header(tx, &commit.tenant)?.map(|h| h.revision);
            if found != Some(commit.revision) {
                return Err(StoreError::Conflict {
                    expected: Some(commit.revision),
                    found,
                });
            }
            apply_segments(tx, &commit.write)?;
            Ok(Receipt {
                revision: commit.revision,
                last_position: None,
            })
        })
    }

    fn search_candidates(
        &self,
        tenant: &TenantId,
        probe: &strata_kernel::search::Probe,
    ) -> StoreResult<Vec<strata_kernel::search::Candidate>> {
        Ok(self
            .on_tenant(tenant, |conn| search_index::candidates(conn, probe))?
            .unwrap_or_default())
    }

    fn segment_catalog(&self, tenant: &TenantId) -> StoreResult<Vec<seg::Segment>> {
        Ok(self.on_tenant(tenant, catalog_rows)?.unwrap_or_default())
    }

    fn segment_rows(
        &self,
        tenant: &TenantId,
        pairs: &[(seg::SegmentId, PrincipalId)],
    ) -> StoreResult<seg::Rows> {
        let mut rows = seg::Rows {
            covered: pairs.iter().cloned().collect(),
            ..seg::Rows::default()
        };
        let Some(file) = self.tenant_db(tenant)? else {
            return Ok(rows);
        };
        file.read(dbe, |tx| {
            for (segment, person) in pairs {
                let key = (*segment, person.clone());
                if let Some(m) = membership_of(tx, *segment, person)? {
                    rows.memberships.insert(key.clone(), m);
                }
                if let Some(at) = wake_of(tx, *segment, person)? {
                    rows.wakes.insert(key, at);
                }
                let erased: bool = tx
                    .query_row(
                        "SELECT EXISTS (SELECT 1 FROM segment_erased WHERE principal = ?1)",
                        params![person.as_str()],
                        |r| r.get(0),
                    )
                    .map_err(db)?;
                if erased {
                    rows.erased.insert(person.clone());
                }
            }
            Ok(())
        })?;
        Ok(rows)
    }

    fn segment_members(
        &self,
        tenant: &TenantId,
        segment: seg::SegmentId,
        after: Option<&PrincipalId>,
        limit: usize,
    ) -> StoreResult<Vec<seg::Membership>> {
        let after = after.map(|p| p.as_str().to_string()).unwrap_or_default();
        Ok(self
            .on_tenant(tenant, |conn| {
                memberships_where(
                    conn,
                    "SELECT segment, principal, version, since FROM segment_memberships
                     WHERE segment = ?1 AND principal > ?2 ORDER BY principal LIMIT ?3",
                    &[&segment.to_string(), &after, &(limit as i64)],
                )
            })?
            .unwrap_or_default())
    }

    fn segment_memberships_of(
        &self,
        tenant: &TenantId,
        person: &PrincipalId,
    ) -> StoreResult<Vec<seg::Membership>> {
        Ok(self
            .on_tenant(tenant, |conn| {
                memberships_where(
                    conn,
                    "SELECT segment, principal, version, since FROM segment_memberships
                     WHERE principal = ?1 ORDER BY segment",
                    &[&person.as_str()],
                )
            })?
            .unwrap_or_default())
    }

    fn segment_wakes_of(
        &self,
        tenant: &TenantId,
        person: &PrincipalId,
    ) -> StoreResult<Vec<seg::Wake>> {
        Ok(self
            .on_tenant(tenant, |conn| {
                wakes_where(
                    conn,
                    "SELECT segment, principal, wake_at FROM segment_wakes WHERE principal = ?1 ORDER BY segment",
                    &[&person.as_str()],
                )
            })?
            .unwrap_or_default())
    }

    fn segment_due(
        &self,
        tenant: &TenantId,
        segment: seg::SegmentId,
        at: Timestamp,
        limit: usize,
    ) -> StoreResult<Vec<seg::Wake>> {
        Ok(self
            .on_tenant(tenant, |conn| {
                wakes_where(
                    conn,
                    "SELECT segment, principal, wake_at FROM segment_wakes
                     WHERE segment = ?1 AND wake_at <= ?2 ORDER BY wake_at, principal LIMIT ?3",
                    &[&segment.to_string(), &at.0, &(limit as i64)],
                )
            })?
            .unwrap_or_default())
    }

    fn segment_batches(&self, tenant: &TenantId) -> StoreResult<Vec<seg::Batch>> {
        Ok(self
            .on_tenant(tenant, |conn| {
                select_strings(
                    conn,
                    "SELECT data FROM segment_batches ORDER BY segment, scope",
                    &[],
                )?
                .iter()
                .map(|data| decode(data, "segment batch"))
                .collect::<StoreResult<Vec<seg::Batch>>>()
            })?
            .unwrap_or_default())
    }

    fn segment_referencing(
        &self,
        tenant: &TenantId,
        object: &ObjectTypeId,
        field: &strata_kernel::FieldId,
        record: strata_kernel::RecordId,
        after: Option<strata_kernel::RecordId>,
        limit: usize,
    ) -> StoreResult<Vec<Arc<Record>>> {
        let key = encode(&Value::Ref(record));
        let after = after.map_or(-1_i64, |a| a.0 as i64);
        let rows = self
            .on_tenant(tenant, |conn| {
                select_strings(
                    conn,
                    "SELECT r.data FROM record_index i
                     JOIN records r ON r.object = i.object AND r.id = i.id
                     WHERE i.object = ?1 AND i.field = ?2 AND i.value = ?3 AND i.id > ?4
                     ORDER BY i.id LIMIT ?5",
                    &[
                        &object.as_str(),
                        &field.as_str(),
                        &key,
                        &after,
                        &(limit as i64),
                    ],
                )
            })?
            .unwrap_or_default();
        rows.iter()
            .map(|data| decode::<Record>(data, "record").map(Arc::new))
            .collect()
    }

    fn segment_subject_count(&self, tenant: &TenantId, object: &ObjectTypeId) -> StoreResult<u64> {
        Ok(self
            .on_tenant(tenant, |conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM records WHERE object = ?1",
                    params![object.as_str()],
                    |r| r.get::<_, i64>(0),
                )
                .map_err(db)
            })?
            .map_or(0, |n| n as u64))
    }

    fn segment_events_after(
        &self,
        tenant: &TenantId,
        after: u64,
        limit: usize,
    ) -> StoreResult<Vec<(u64, seg::SegmentEvent)>> {
        let rows = self
            .on_tenant(tenant, |conn| {
                let mut statement = conn
                    .prepare_cached(
                        "SELECT position, data FROM segment_events WHERE position > ?1 ORDER BY position LIMIT ?2",
                    )
                    .map_err(db)?;
                let found = statement
                    .query_map(params![after as i64, limit as i64], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                    })
                    .map_err(db)?;
                found.collect::<Result<Vec<_>, _>>().map_err(db)
            })?
            .unwrap_or_default();
        rows.iter()
            .map(|(position, data)| {
                decode::<seg::SegmentEvent>(data, "segment event").map(|e| (*position as u64, e))
            })
            .collect()
    }

    fn idempotent_result(
        &self,
        tenant: &TenantId,
        principal: &str,
        key: &str,
        now: Timestamp,
    ) -> StoreResult<Option<IdempotentResult>> {
        let Some(file) = self.tenant_db(tenant)? else {
            return Ok(None);
        };
        let conn = file.lock().map_err(dbe)?;
        let row = conn
            .query_row(
                "SELECT request_hash, revision, last_position, result FROM idempotency
                 WHERE principal = ?1 AND key = ?2 AND recorded_at >= ?3",
                params![principal, key, now.0 - IDEMPOTENCY_RETENTION_MS],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(db)?;
        row.map(|(request_hash, revision, position, result)| {
            Ok::<_, StoreError>(IdempotentResult {
                request_hash,
                result: decode(&result, "idempotency result")?,
                receipt: Receipt {
                    revision: StateRevision(revision as u64),
                    last_position: position.map(|p| p as u64),
                },
            })
        })
        .transpose()
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
        let tenants = self.directory()?;
        if limit == 0 || tenants.is_empty() {
            return Ok(out);
        }
        // Each tenant gets an equal share of the batch, starting from a tenant that moves on at
        // every sweep; what a short tenant leaves unused goes to the ones with more waiting.
        let n = tenants.len();
        let share = limit.div_ceil(n);
        let start = self.sweep.fetch_add(1, Ordering::Relaxed) % n;
        let mut more: Vec<(&TenantId, u64)> = Vec::new();
        for i in 0..n {
            if out.len() >= limit {
                break;
            }
            let tenant = &tenants[(start + i) % n];
            let (entries, capped) = self.outbox_of(tenant, 0, share.min(limit - out.len()));
            if capped {
                if let Some(last) = entries.last() {
                    more.push((tenant, last.position));
                }
            }
            out.extend(entries);
        }
        for (tenant, after) in more {
            if out.len() >= limit {
                break;
            }
            let (entries, _) = self.outbox_of(tenant, after, limit - out.len());
            out.extend(entries);
        }
        Ok(out)
    }

    fn outbox_faults(&self) -> Vec<OutboxFault> {
        self.faults.lock().map(|f| f.clone()).unwrap_or_default()
    }

    fn outbox_pending_of(&self, tenant: &TenantId) -> StoreResult<u64> {
        Ok(self
            .on_tenant(tenant, |conn| {
                conn.query_row("SELECT COUNT(*) FROM events WHERE published = 0", [], |r| {
                    r.get::<_, i64>(0)
                })
                .map_err(db)
            })?
            .unwrap_or(0) as u64)
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

    fn journal_count(&self, tenant: &TenantId) -> StoreResult<u64> {
        Ok(self
            .on_tenant(tenant, |conn| {
                conn.query_row("SELECT COUNT(*) FROM journal", [], |r| r.get::<_, i64>(0))
                    .map_err(db)
            })?
            .unwrap_or(0) as u64)
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

    fn session_seen(&self, id: &SessionId, at: Timestamp) -> StoreResult<()> {
        let Some(tenant) = self.session_tenant("id", id.as_str())? else {
            return Ok(());
        };
        let Some(file) = self.tenant_db(&tenant)? else {
            return Ok(());
        };
        file.lock()
            .map_err(dbe)?
            .execute(
                "UPDATE sessions SET last_seen_at = ?2 WHERE id = ?1",
                params![id.as_str(), at.0],
            )
            .map_err(db)?;
        Ok(())
    }

    fn session_authenticated(
        &self,
        id: &SessionId,
        at: Timestamp,
        network: Option<&str>,
    ) -> StoreResult<()> {
        let Some(tenant) = self.session_tenant("id", id.as_str())? else {
            return Ok(());
        };
        let Some(file) = self.tenant_db(&tenant)? else {
            return Ok(());
        };
        file.lock()
            .map_err(dbe)?
            .execute(
                "UPDATE sessions SET authenticated_at = ?2, network = ?3 WHERE id = ?1",
                params![id.as_str(), at.0, network],
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
            // One tenant that cannot be read does not hide the sessions of the others.
            if let Ok(found) = self.live_sessions_of(&tenant, None, now) {
                out.extend(found);
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    fn live_sessions_of(
        &self,
        tenant: &TenantId,
        principal: Option<&PrincipalId>,
        now: Timestamp,
    ) -> StoreResult<Vec<SessionRecord>> {
        Ok(self
            .on_tenant(tenant, |conn| match principal {
                Some(principal) => sessions_where(
                    conn,
                    tenant,
                    "revoked_at IS NULL AND expires_at > ?1 AND principal = ?2",
                    &[&now.0, &principal.as_str()],
                ),
                None => sessions_where(
                    conn,
                    tenant,
                    "revoked_at IS NULL AND expires_at > ?1",
                    &[&now.0],
                ),
            })?
            .unwrap_or_default())
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
                "SELECT login, display_name, password_hash, verified_email FROM accounts WHERE principal = ?1 ORDER BY login LIMIT 1",
                params![principal.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(db)
        })?;
        Ok(row.flatten().map(
            |(login, display_name, password_hash, verified_email)| Account {
                tenant: tenant.clone(),
                login,
                principal: principal.clone(),
                display_name,
                password_hash,
                verified_email,
            },
        ))
    }

    fn verified_address(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<Option<String>> {
        // The same account as `account_of`: the principal's first login.
        let address = self.on_tenant(tenant, |conn| {
            conn.query_row(
                "SELECT verified_email FROM accounts WHERE principal = ?1 ORDER BY login LIMIT 1",
                params![principal.as_str()],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(db)
        })?;
        Ok(address.flatten().flatten())
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
                "INSERT INTO accounts (login, principal, display_name, password_hash, verified_email) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (login) DO UPDATE SET principal = excluded.principal,
                    display_name = excluded.display_name, password_hash = excluded.password_hash,
                    verified_email = excluded.verified_email",
                params![
                    account.login,
                    account.principal.as_str(),
                    account.display_name,
                    account.password_hash,
                    account.verified_email
                ],
            )
            .map_err(db)?;
        Ok(())
    }

    fn create_account(&self, account: &Account) -> StoreResult<bool> {
        let file = self.existing(&account.tenant)?;
        let inserted = file
            .lock()
            .map_err(dbe)?
            .execute(
                "INSERT INTO accounts (login, principal, display_name, password_hash, verified_email) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (login) DO NOTHING",
                params![
                    account.login,
                    account.principal.as_str(),
                    account.display_name,
                    account.password_hash,
                    account.verified_email
                ],
            )
            .map_err(db)?;
        Ok(inserted == 1)
    }

    fn update_account(&self, expected: &Account, replacement: &Account) -> StoreResult<bool> {
        if expected.tenant != replacement.tenant || expected.login != replacement.login {
            return Err(StoreError::Invalid(
                "an account update keeps its tenant and login".into(),
            ));
        }
        let file = self.existing(&expected.tenant)?;
        let changed = file
            .lock()
            .map_err(dbe)?
            .execute(
                "UPDATE accounts SET principal = ?2, display_name = ?3, password_hash = ?4, verified_email = ?5
                 WHERE login = ?1 AND principal = ?6 AND display_name = ?7 AND password_hash = ?8
                   AND verified_email IS ?9",
                params![
                    expected.login,
                    replacement.principal.as_str(),
                    replacement.display_name,
                    replacement.password_hash,
                    replacement.verified_email,
                    expected.principal.as_str(),
                    expected.display_name,
                    expected.password_hash,
                    expected.verified_email
                ],
            )
            .map_err(db)?;
        Ok(changed == 1)
    }

    fn account(&self, tenant: &TenantId, login: &str) -> StoreResult<Option<Account>> {
        let Some(file) = self.tenant_db(tenant)? else {
            return Ok(None);
        };
        let conn = file.lock().map_err(dbe)?;
        let row = conn
            .query_row(
                "SELECT principal, display_name, password_hash, verified_email FROM accounts WHERE login = ?1",
                params![login],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(db)?;
        row.map(|(principal, display_name, password_hash, verified_email)| {
            Ok(Account {
                tenant: tenant.clone(),
                login: login.to_string(),
                principal: PrincipalId::new(principal).map_err(corrupt_id)?,
                display_name,
                password_hash,
                verified_email,
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

    fn anonymise_ledger_principal(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<strata_kernel::ledger::Anonymised> {
        let Some(file) = self.tenant_db(tenant)? else {
            return Ok(Default::default());
        };
        file.write(dbe, |tx| ledger::anonymise(tx, principal))
    }

    fn remove_principal(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<PrincipalRemoval> {
        let Some(file) = self.tenant_db(tenant)? else {
            return Ok(PrincipalRemoval::default());
        };
        // One transaction: a sign-in sees all of the principal's rows or none of them.
        file.write(dbe, |tx| {
            let gone = |sql: &str| -> StoreResult<u64> {
                Ok(tx.execute(sql, params![principal.as_str()]).map_err(db)? as u64)
            };
            Ok(PrincipalRemoval {
                accounts: gone("DELETE FROM accounts WHERE principal = ?1")?,
                passkeys: gone("DELETE FROM passkeys WHERE principal = ?1")?,
                passkey_challenges: gone("DELETE FROM passkey_challenges WHERE principal = ?1")?,
                provider_flows: gone("DELETE FROM oidc_flows WHERE principal = ?1")?
                    + gone("DELETE FROM saml_requests WHERE principal = ?1")?,
                identities: gone("DELETE FROM federated_identities WHERE principal = ?1")?,
                totp_factors: gone("DELETE FROM totp_factors WHERE principal = ?1")?,
                recovery_codes: gone("DELETE FROM recovery_codes WHERE principal = ?1")?,
                pending_sign_ins: gone("DELETE FROM pending_sign_ins WHERE principal = ?1")?,
                sign_in_links: gone("DELETE FROM sign_in_links WHERE principal = ?1")?,
                recovery_requests: gone("DELETE FROM recovery_requests WHERE principal = ?1")?,
            })
        })
    }

    fn doc_put(
        &self,
        tenant: &TenantId,
        kind: &str,
        key: &str,
        value: &str,
        create_only: bool,
    ) -> StoreResult<bool> {
        let sql = if create_only {
            "INSERT OR IGNORE INTO identity_docs (kind, key, value) VALUES (?1, ?2, ?3)"
        } else {
            "INSERT INTO identity_docs (kind, key, value) VALUES (?1, ?2, ?3)
             ON CONFLICT (kind, key) DO UPDATE SET value = excluded.value"
        };
        // A tenant that does not exist has no file, and is refused as not found.
        let changed = self.on_existing(tenant, |conn| {
            conn.execute(sql, params![kind, key, value]).map_err(db)
        })?;
        Ok(changed == 1)
    }

    fn doc_get(&self, tenant: &TenantId, kind: &str, key: &str) -> StoreResult<Option<String>> {
        let value = self.on_tenant(tenant, |conn| {
            conn.query_row(
                "SELECT value FROM identity_docs WHERE kind = ?1 AND key = ?2",
                params![kind, key],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(db)
        })?;
        Ok(value.flatten())
    }

    fn doc_list(&self, tenant: &TenantId, kind: &str) -> StoreResult<Vec<(String, String)>> {
        let rows = self.on_tenant(tenant, |conn| {
            let mut statement = conn
                .prepare("SELECT key, value FROM identity_docs WHERE kind = ?1 ORDER BY key")
                .map_err(db)?;
            let rows = statement
                .query_map(params![kind], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)?;
            Ok(rows)
        })?;
        Ok(rows.unwrap_or_default())
    }

    fn doc_remove(&self, tenant: &TenantId, kind: &str, key: &str) -> StoreResult<bool> {
        let removed = self.on_tenant(tenant, |conn| {
            conn.execute(
                "DELETE FROM identity_docs WHERE kind = ?1 AND key = ?2",
                params![kind, key],
            )
            .map_err(db)
        })?;
        Ok(removed == Some(1))
    }

    fn audit_append(&self, tenant: &TenantId, entry: &AuditEntry) -> StoreResult<()> {
        self.on_existing(tenant, |conn| {
            conn.execute(
                "INSERT INTO audit_log (at, kind, event, correlation, data) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![entry.at, entry.kind, entry.event, entry.correlation, entry.data],
            )
            .map_err(db)
            .map(|_| ())
        })
    }

    fn audit_list(
        &self,
        tenant: &TenantId,
        kind: Option<&str>,
        limit: usize,
    ) -> StoreResult<Vec<AuditEntry>> {
        let rows = self.on_tenant(tenant, |conn| {
            let mut statement = conn
                .prepare(
                    "SELECT at, kind, event, correlation, data FROM audit_log
                     WHERE ?1 IS NULL OR kind = ?1 ORDER BY seq DESC LIMIT ?2",
                )
                .map_err(db)?;
            let rows = statement
                .query_map(params![kind, limit as i64], |r| {
                    Ok(AuditEntry {
                        at: r.get(0)?,
                        kind: r.get(1)?,
                        event: r.get(2)?,
                        correlation: r.get(3)?,
                        data: r.get(4)?,
                    })
                })
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)?;
            Ok(rows)
        })?;
        let mut rows = rows.unwrap_or_default();
        rows.reverse();
        Ok(rows)
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
            ledger::check_restorable(tx, state.revision)?;
            tx.execute("DELETE FROM records", []).map_err(db)?;
            tx.execute(
                "UPDATE tenant SET name = ?1, revision = ?2, next_record = ?3, event_seq = MAX(event_seq, ?4), schema = ?5, members = ?6
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
            pair_index::reset(tx, &state.schema)?;
            for record in state.records.values().flat_map(|records| records.values()) {
                write_record(tx, record)?;
            }
            tx.execute("DELETE FROM setting_overrides", []).map_err(db)?;
            tx.execute("DELETE FROM setting_revisions", []).map_err(db)?;
            if let Some(values) = &state.setting_values {
                for held in values.overrides.values() {
                    write_setting_override(tx, held)?;
                }
                for (setting, revision) in &values.revisions {
                    write_setting_revision(tx, setting, *revision)?;
                }
            }
            // Events stay and the counter does not go back (`MAX` above): a subscriber that has
            // passed a sequence number must still see every later event.
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

    fn jobs_create(
        &self,
        tenant: &TenantId,
        new: &[NewJob],
        now: Timestamp,
    ) -> StoreResult<Vec<String>> {
        check_jobs(new)?;
        self.existing(tenant)?;
        let earliest = new.iter().map(|job| job.due_at.0).min();
        self.on_jobs(tenant, earliest, |tx| insert_jobs(tx, tenant, new, now))?
            .ok_or_else(|| StoreError::NotFound(format!("tenant {tenant}")))
    }

    fn job(&self, tenant: &TenantId, id: &str) -> StoreResult<Option<Job>> {
        Ok(self
            .on_tenant(tenant, |conn| job_by_id(conn, id))?
            .flatten())
    }

    fn jobs_list(
        &self,
        tenant: &TenantId,
        before: Option<u64>,
        limit: usize,
    ) -> StoreResult<Vec<Job>> {
        let before = before.map_or(i64::MAX, |b| b as i64);
        Ok(self
            .on_tenant(tenant, |conn| {
                jobs_of(
                    conn,
                    "SELECT seq, data FROM jobs WHERE seq < ?1 ORDER BY seq DESC LIMIT ?2",
                    &[&before, &(limit as i64)],
                )
            })?
            .unwrap_or_default())
    }

    fn job_lease(
        &self,
        tenant: &TenantId,
        worker: &str,
        now: Timestamp,
    ) -> StoreResult<Option<Lease>> {
        Ok(self
            .on_jobs(tenant, None, |tx| {
                let sql = format!(
                    "SELECT seq, data FROM jobs WHERE {OPEN_JOB} AND due_at <= ?1 ORDER BY due_at, seq LIMIT 1"
                );
                let Some(job) = jobs_of(tx, &sql, &[&now.0])?.pop() else {
                    return Ok(None);
                };
                let lease = jobs::apply_lease(&job, worker, now);
                put_job(tx, &lease.job)?;
                Ok(Some(lease))
            })?
            .flatten())
    }

    fn job_report(
        &self,
        tenant: &TenantId,
        id: &str,
        token: u64,
        report: &JobReport,
        writes: &WorkerWrites,
        now: Timestamp,
    ) -> StoreResult<Reported> {
        check_jobs(&writes.jobs)?;
        let earliest = writes.jobs.iter().map(|job| job.due_at.0).min();
        Ok(self
            .on_jobs(tenant, earliest, |tx| {
                let Some(job) = job_by_id(tx, id)? else {
                    return Ok(Reported::Refused(format!("no job {id}")));
                };
                let next = match jobs::apply_report(&job, token, report, now) {
                    Ok(next) => next,
                    Err(why) => return Ok(Reported::Refused(why)),
                };
                put_job(tx, &next)?;
                write_automation(tx, tenant, writes, now)?;
                Ok(Reported::Recorded(next))
            })?
            .unwrap_or_else(|| Reported::Refused(format!("no tenant {tenant}"))))
    }

    fn job_cancel(
        &self,
        tenant: &TenantId,
        id: &str,
        reason: &str,
        now: Timestamp,
    ) -> StoreResult<JobChange> {
        Ok(self
            .on_jobs(tenant, None, |tx| {
                let Some(job) = job_by_id(tx, id)? else {
                    return Ok(JobChange::NotFound);
                };
                let change = jobs::apply_cancel(&job, reason, now);
                if let JobChange::Changed(next) = &change {
                    put_job(tx, next)?;
                }
                Ok(change)
            })?
            .unwrap_or(JobChange::NotFound))
    }

    fn job_retry(&self, tenant: &TenantId, id: &str, now: Timestamp) -> StoreResult<JobChange> {
        Ok(self
            .on_jobs(tenant, Some(now.0), |tx| {
                let Some(job) = job_by_id(tx, id)? else {
                    return Ok(JobChange::NotFound);
                };
                let change = jobs::apply_retry(&job, now);
                if let JobChange::Changed(next) = &change {
                    put_job(tx, next)?;
                }
                Ok(change)
            })?
            .unwrap_or(JobChange::NotFound))
    }

    fn jobs_due(&self, now: Timestamp, limit: usize) -> StoreResult<Vec<TenantId>> {
        // The wake index only: no tenant file is opened to find due work (SCH-007).
        let conn = self.control.lock().map_err(dbe)?;
        select_strings(
            &conn,
            "SELECT w.tenant FROM job_wake w JOIN tenant_directory d ON d.id = w.tenant
             WHERE d.state = 'active' AND w.due_at <= ?1 ORDER BY w.due_at, w.tenant LIMIT ?2",
            &[&now.0, &(limit as i64)],
        )?
        .into_iter()
        .map(|id| TenantId::new(id).map_err(corrupt_id))
        .collect()
    }

    fn automation_write(
        &self,
        tenant: &TenantId,
        writes: &WorkerWrites,
        now: Timestamp,
    ) -> StoreResult<()> {
        check_jobs(&writes.jobs)?;
        self.existing(tenant)?;
        let earliest = writes.jobs.iter().map(|job| job.due_at.0).min();
        self.on_jobs(tenant, earliest, |tx| {
            write_automation(tx, tenant, writes, now)
        })?
        .ok_or_else(|| StoreError::NotFound(format!("tenant {tenant}")))
    }

    fn automation_run(&self, tenant: &TenantId, run: &str) -> StoreResult<Option<RunRecord>> {
        Ok(self
            .on_tenant(tenant, |conn| {
                Ok(runs_of(
                    conn,
                    "SELECT seq, data FROM automation_runs WHERE run = ?1",
                    &[&run],
                )?
                .pop())
            })?
            .flatten())
    }

    fn automation_runs(
        &self,
        tenant: &TenantId,
        automation: Option<&WorkflowId>,
        state: Option<RunState>,
        before: Option<u64>,
        limit: usize,
    ) -> StoreResult<Vec<RunRecord>> {
        let before = before.map_or(i64::MAX, |b| b as i64);
        let automation = automation.map(|a| a.as_str().to_string());
        let state = state.map(|s| s.as_str());
        Ok(self
            .on_tenant(tenant, |conn| {
                runs_of(
                    conn,
                    "SELECT seq, data FROM automation_runs
                     WHERE seq < ?1 AND (?2 IS NULL OR automation = ?2) AND (?3 IS NULL OR state = ?3)
                     ORDER BY seq DESC LIMIT ?4",
                    &[&before, &automation, &state, &(limit as i64)],
                )
            })?
            .unwrap_or_default())
    }

    fn automation_steps(
        &self,
        tenant: &TenantId,
        run: &str,
        after: Option<u64>,
        limit: usize,
    ) -> StoreResult<Vec<StepRecord>> {
        let after = after.map_or(0, |a| a as i64);
        Ok(self
            .on_tenant(tenant, |conn| {
                select_strings(
                    conn,
                    "SELECT data FROM automation_steps WHERE run = ?1 AND sequence > ?2
                     ORDER BY sequence LIMIT ?3",
                    &[&run, &after, &(limit as i64)],
                )?
                .iter()
                .map(|data| decode(data, "automation step"))
                .collect()
            })?
            .unwrap_or_default())
    }

    fn automation_exceptions(
        &self,
        tenant: &TenantId,
        kind: Option<ExceptionKind>,
        unsettled: bool,
        before: Option<u64>,
        limit: usize,
    ) -> StoreResult<Vec<ExceptionRecord>> {
        let before = before.map_or(i64::MAX, |b| b as i64);
        let kind = kind.map(|k| k.as_str());
        let unsettled = i64::from(unsettled);
        Ok(self
            .on_tenant(tenant, |conn| {
                exceptions_of(
                    conn,
                    "SELECT seq, data FROM automation_exceptions
                     WHERE seq < ?1 AND (?2 IS NULL OR kind = ?2) AND (?3 = 0 OR settled = 0)
                     ORDER BY seq DESC LIMIT ?4",
                    &[&before, &kind, &unsettled, &(limit as i64)],
                )
            })?
            .unwrap_or_default())
    }

    fn automation_exception_settle(
        &self,
        tenant: &TenantId,
        id: &str,
        release: bool,
        now: Timestamp,
    ) -> StoreResult<Settled> {
        let creates = release.then_some(now.0);
        Ok(self
            .on_jobs(tenant, creates, |tx| {
                let Some(mut record) = exceptions_of(
                    tx,
                    "SELECT seq, data FROM automation_exceptions WHERE id = ?1",
                    &[&id],
                )?
                .pop() else {
                    return Ok(Settled::NotFound);
                };
                if let Some(why) = jobs::settle_problem(&record, release) {
                    return Ok(Settled::Conflict(why));
                }
                let mut released = None;
                if release {
                    let job_id = record.job.clone().unwrap_or_default();
                    let Some(job) = job_by_id(tx, &job_id)? else {
                        return Ok(Settled::Conflict(format!("no job {job_id} to release")));
                    };
                    let JobChange::Changed(next) = jobs::apply_retry(&job, now) else {
                        return Ok(Settled::Conflict(format!(
                            "job {job_id} has not failed and cannot be given another attempt"
                        )));
                    };
                    put_job(tx, &next)?;
                    released = Some(job_id);
                }
                record.settled = true;
                record.seq = 0;
                tx.execute(
                    "UPDATE automation_exceptions SET settled = 1, data = ?2 WHERE id = ?1",
                    params![id, encode(&record)],
                )
                .map_err(db)?;
                Ok(Settled::Done { job: released })
            })?
            .unwrap_or(Settled::NotFound))
    }

    fn begin_provisioning(&self, tenant: &TenantId) -> StoreResult<()> {
        self.set_directory_state(tenant, "active", "provisioning")
    }

    fn finish_provisioning(&self, tenant: &TenantId) -> StoreResult<()> {
        self.set_directory_state(tenant, "provisioning", "active")
    }

    fn rollback_provisioning(&self) -> StoreResult<Vec<TenantId>> {
        let pending = select_strings(
            &*self.control.lock().map_err(dbe)?,
            "SELECT id FROM tenant_directory WHERE state = 'provisioning' ORDER BY id",
            &[],
        )?;
        let mut removed = Vec::new();
        for id in pending {
            let tenant = TenantId::new(id).map_err(corrupt_id)?;
            self.remove_tenant(&tenant)?;
            removed.push(tenant);
        }
        Ok(removed)
    }
}
