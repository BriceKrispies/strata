//! The PostgreSQL adapter. It keeps what the SQLite adapter keeps, in the same tables, and
//! answers every method of [`Store`] the way the SQLite adapter answers it; where it differs, the
//! method says why.
//!
//! # Placement
//!
//! The adapter is given a **control connection** and a [`Placement`] rule. The control connection's
//! database holds, in the schema `strata_control`, what SQLite keeps in its control database: the
//! tenant directory (`tenant_directory`: id, state, the schema of the tenant, its plan), the session
//! directory, the wake index of the automation worker, the platform level of settings, and the
//! ledger of the control steps. A tenant's tables live in the database and schema the placement
//! rule names for it, and that schema holds the tables SQLite keeps in a tenant file. The default
//! rule, [`SchemaPerTenant`], puts every tenant in the control connection's database, each in a
//! schema of its own, `t_<tenant id>` ([`tenant_schema`]). A rule that puts tenants in other
//! databases (one per tenant, or a few shared ones) changes nothing else: the database must exist,
//! and the adapter creates the schema in it.
//!
//! The directory records the schema a tenant was created in, and the database is the rule's: no
//! connection string is ever stored.
//!
//! # Transactions
//!
//! Every write of a tenant runs in a READ COMMITTED transaction that first takes the tenant's
//! writer lock, a transaction-level advisory lock on its schema, so that the writers of one tenant
//! go one at a time as they do on a SQLite file. A commit then locks the tenant's row
//! (`SELECT ... FOR UPDATE`), compares the revision under that lock, assigns the outbox positions of
//! its events from the row's counter, and writes. Positions therefore follow commit order and are
//! never taken from a sequence. Every read of more than one statement runs in a REPEATABLE READ,
//! READ ONLY transaction, so it sees whole commits only.
//!
//! # Identifiers
//!
//! The only text formatted into SQL is a schema name, and only one [`pool::valid_schema`] accepts:
//! lower-case letters, digits and `_`, at most 63 bytes. Every value is a bind parameter.

mod ledger;
mod migrations;
mod pool;

mod index_page;
mod pair_index;
mod search_index;

use migrations::{migrate, CONTROL_LEDGER, CONTROL_MIGRATIONS, TENANT_LEDGER, TENANT_MIGRATIONS};
use pool::{quoted, Pool, Scope};
use postgres::types::{FromSql, ToSql};
use postgres::{Row, Transaction};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use strata_kernel::ext::*;
use strata_kernel::segments as seg;
use strata_kernel::WorkflowId;
use strata_kernel::{
    canon, Coverage, DeltaOp, DomainEvent, Need, ObjectTypeId, PrincipalId, Record, RoleId,
    SessionId, SettingId, SettingOverride, SettingValues, State, StateRevision, TenantId,
    TenantSchema, TenantState, Timestamp, Value,
};
use strata_storage_api::jobs::{
    self, ExceptionKind, ExceptionRecord, Job, JobChange, JobReport, Lease, NewJob, Reported,
    RunRecord, RunState, Settled, StepRecord, WorkerWrites,
};
use strata_storage_api::schedules::{
    self, AutomationPurged, AutomationRetention, Schedule, ScheduleSet,
};
use strata_storage_api::segments::{write_conflict, SegmentCommit};
use strata_storage_api::{
    check_event_sequence, field_matches, random_nonces, throttle_refund, throttle_step, Account,
    AuditEntry, Capabilities, Commit, FederatedIdentity, Generation, Health, HeldHeader, HeldLoad,
    IdempotentResult, JournalEntry, Loaded, NonceSource, OidcFlow, OutboxEntry, OutboxFault,
    PasskeyCeremony, PasskeyChallenge, PasskeyRecord, PendingSignIn, PlatformSetting,
    PlatformWrite, PrincipalRemoval, Receipt, RecoveryRequest, SamlRequest, SessionRecord,
    SignInLink, SnapshotInfo, StorageMigration, Store, StoreError, StoreResult, TenantSummary,
    ThrottleRow, ThrottleRule, ThrottleSettle, ThrottleVerdict, TotpRecord,
};

// ---------------------------------------------------------------------------------------------
// Placement
// ---------------------------------------------------------------------------------------------

/// Where one tenant's tables live: a database, by its connection string, and a schema in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    /// A connection string the `postgres` crate accepts, such as
    /// `postgres://strata:secret@db.example:5432/strata`.
    pub url: String,
    /// The schema of the tenant's tables: lower-case letters, digits and `_`, at most 63 bytes,
    /// not starting with a digit. A tenant is refused a schema that does not meet this.
    pub schema: String,
}

/// The rule that says where a tenant's tables live (WG-5286 leaves the choice open). It is asked
/// when a tenant is created, and for the database of a tenant that exists; the schema of a tenant
/// that exists is the one the directory recorded at its creation.
pub trait Placement: Send + Sync {
    /// The database and schema of `tenant`. The same tenant must always get the same database.
    fn place(&self, tenant: &TenantId) -> Location;

    /// The rule in a few words, for the diagnostics. It must not name a password.
    fn describe(&self) -> String {
        "a placement rule of the caller's".into()
    }
}

/// The default rule: every tenant in the control connection's database, in a schema of its own
/// named by [`tenant_schema`].
pub struct SchemaPerTenant {
    url: String,
}

impl SchemaPerTenant {
    pub fn new(url: &str) -> Self {
        SchemaPerTenant {
            url: url.to_string(),
        }
    }
}

impl Placement for SchemaPerTenant {
    fn place(&self, tenant: &TenantId) -> Location {
        Location {
            url: self.url.clone(),
            schema: tenant_schema(tenant),
        }
    }

    fn describe(&self) -> String {
        "every tenant in the control database, one schema t_<tenant id> each".into()
    }
}

/// The schema of a tenant under [`SchemaPerTenant`]: `t_` and the tenant id. Tenant ids are
/// lower-case letters, digits and `_` and at most 64 bytes; one that would not fit PostgreSQL's
/// 63 bytes, or that holds another character, is cut to 40 bytes, every other character turned
/// into `_`, and a hash of the whole id appended, so that two ids never share a schema.
pub fn tenant_schema(tenant: &TenantId) -> String {
    let id = tenant.as_str();
    let clean: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let plain = format!("t_{clean}");
    if clean == id && plain.len() <= 63 {
        plain
    } else {
        format!("t_{}_{:016x}", &clean[..clean.len().min(40)], fnv1a(id))
    }
}

/// FNV-1a, 64 bits: a stable hash for names, not for secrets.
fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

// ---------------------------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------------------------

/// The schema of the control tables in the control connection's database.
const CONTROL_SCHEMA: &str = "strata_control";
/// How many outbox problems are remembered.
const MAX_FAULTS: usize = 200;
/// `events.published` of an event that could not be decoded: neither waiting nor published.
const PARKED: i64 = 2;
/// Index values longer than this are not indexed; lookups for them scan the object type.
const MAX_INDEXED: usize = strata_kernel::query_plan::INDEX_KEY_BYTES;
/// The states of a job that has not ended, as the statements test them.
const OPEN_JOB: &str = "state IN ('waiting', 'leased')";

/// A database error as the store reports it, as the SQLite adapter reports SQLite's: a broken
/// constraint (a unique, foreign-key, check or not-null violation, or the ledger's append-only
/// trigger, all SQLSTATE class 23) is `Invalid`; anything else, a serialisation failure or a
/// deadlock included, is `Unavailable` and may be retried.
pub(crate) fn db(error: postgres::Error) -> StoreError {
    match error.code() {
        Some(code) if code.code().starts_with("23") => StoreError::Invalid(error.to_string()),
        _ => StoreError::Unavailable(error.to_string()),
    }
}

pub(crate) fn encode<T: Serialize>(value: &T) -> String {
    canon::canonical_json(value)
}

pub(crate) fn decode<T: DeserializeOwned>(text: &str, what: &str) -> StoreResult<T> {
    serde_json::from_str(text).map_err(|e| StoreError::Corrupt(format!("{what}: {e}")))
}

pub(crate) fn corrupt_id(e: strata_kernel::IdError) -> StoreError {
    StoreError::Corrupt(e.to_string())
}

/// Column `idx` of a row, as `T`; a column of another type is corrupt data.
pub(crate) fn get<'a, T: FromSql<'a>>(row: &'a Row, idx: usize) -> StoreResult<T> {
    row.try_get(idx)
        .map_err(|e| StoreError::Corrupt(format!("column {idx}: {e}")))
}

/// The first column of every row a statement selects, as text.
pub(crate) fn select_strings(
    t: &mut Transaction<'_>,
    sql: &str,
    args: &[&(dyn ToSql + Sync)],
) -> StoreResult<Vec<String>> {
    let rows = t.query(sql, args).map_err(db)?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let text: String = get(row, 0)?;
        out.push(text);
    }
    Ok(out)
}

/// One count a statement selects.
fn count(t: &mut Transaction<'_>, sql: &str, args: &[&(dyn ToSql + Sync)]) -> StoreResult<u64> {
    let row = t.query_one(sql, args).map_err(db)?;
    let n: i64 = get(&row, 0)?;
    Ok(n.max(0) as u64)
}

/// Every record of one object type, as stored JSON, in id order.
pub(crate) fn all_of(t: &mut Transaction<'_>, object: &ObjectTypeId) -> StoreResult<Vec<String>> {
    select_strings(
        t,
        "SELECT data FROM records WHERE object = $1 ORDER BY id",
        &[&object.as_str()],
    )
}

/// A redacted connection string for the diagnostics: whatever precedes an `@` (the user and the
/// password) is left out.
fn redact(url: &str) -> String {
    match (url.find("://"), url.rfind('@')) {
        (Some(scheme), Some(at)) if at > scheme => {
            format!("{}{}", &url[..scheme + 3], &url[at + 1..])
        }
        (None, _) => "a key=value connection string".to_string(),
        _ => url.to_string(),
    }
}

/// The header of the tenant in its schema, without records.
fn header(t: &mut Transaction<'_>, tenant: &TenantId) -> StoreResult<Option<TenantState>> {
    let Some(row) = t
        .query_opt(
            "SELECT name, revision, next_record, event_seq, schema, members FROM tenant WHERE id = $1",
            &[&tenant.as_str()],
        )
        .map_err(db)?
    else {
        return Ok(None);
    };
    let name: String = get(&row, 0)?;
    let revision: i64 = get(&row, 1)?;
    let next_record: i64 = get(&row, 2)?;
    let event_seq: i64 = get(&row, 3)?;
    let schema: String = get(&row, 4)?;
    let members: String = get(&row, 5)?;
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
        setting_values: setting_values(t)?,
        ledger: None,
    }))
}

/// The header for a load, and whether its documents were decoded. When `held` is the header at
/// the generation this transaction read, the schema, the members and the settings are not read or
/// decoded: only the counters of the tenant row are, and the held documents are used. Otherwise
/// the header is decoded as always.
fn header_for_load(
    t: &mut Transaction<'_>,
    tenant: &TenantId,
    generation: Option<Generation>,
    held: Option<&HeldHeader>,
) -> StoreResult<Option<(TenantState, bool)>> {
    if let (Some(held), Some(generation)) = (held, generation) {
        if held.generation == generation {
            let Some(row) = t
                .query_opt(
                    "SELECT revision, next_record, event_seq FROM tenant WHERE id = $1",
                    &[&tenant.as_str()],
                )
                .map_err(db)?
            else {
                return Ok(None);
            };
            let revision: i64 = get(&row, 0)?;
            let next_record: i64 = get(&row, 1)?;
            let event_seq: i64 = get(&row, 2)?;
            return Ok(Some((
                held.state(
                    tenant,
                    StateRevision(revision as u64),
                    next_record as u64,
                    event_seq as u64,
                ),
                false,
            )));
        }
    }
    Ok(header(t, tenant)?.map(|state| (state, true)))
}

/// Whether the tenant's row is in its schema.
fn tenant_present(t: &mut Transaction<'_>, tenant: &TenantId) -> StoreResult<bool> {
    let row = t
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM tenant WHERE id = $1)",
            &[&tenant.as_str()],
        )
        .map_err(db)?;
    get(&row, 0)
}

/// The generation row of the tenant's schema, none when the schema holds no tenant. Reads that row
/// only.
fn read_generation(t: &mut Transaction<'_>) -> StoreResult<Option<Generation>> {
    let Some(row) = t
        .query_opt("SELECT nonce, counter FROM generation WHERE id = 1", &[])
        .map_err(db)?
    else {
        return Ok(None);
    };
    let nonce: String = get(&row, 0)?;
    let counter: i64 = get(&row, 1)?;
    Ok(Some(Generation {
        nonce: Generation::parse_nonce(&nonce)
            .ok_or_else(|| StoreError::Corrupt(format!("generation nonce {nonce:?}")))?,
        counter: counter as u64,
    }))
}

/// Write the generation row, keeping the fence token it holds.
fn write_generation(t: &mut Transaction<'_>, generation: &Generation) -> StoreResult<()> {
    let nonce = generation.nonce_hex();
    let counter: i64 = generation.counter as i64;
    t.execute(
        "INSERT INTO generation (id, nonce, counter, fence) VALUES (1, $1, $2, 0)
         ON CONFLICT (id) DO UPDATE SET nonce = EXCLUDED.nonce, counter = EXCLUDED.counter",
        &[&nonce, &counter],
    )
    .map_err(db)?;
    Ok(())
}

/// Compare the generation a caller expects with the one the schema holds, in the caller's write
/// transaction (which holds the schema's writer lock). `None` is the unguarded control-plane
/// call. Returns the generation held.
fn guard_generation(
    t: &mut Transaction<'_>,
    tenant: &TenantId,
    expected: Option<&Generation>,
) -> StoreResult<Option<Generation>> {
    let held = read_generation(t)?;
    if let Some(expected) = expected {
        if Some(*expected) != held {
            return Err(StoreError::Conflict {
                expected: None,
                found: header(t, tenant)?.map(|h| h.revision),
                found_generation: held,
            });
        }
    }
    Ok(held)
}

/// The setting overrides and revisions of the tenant, none when there is neither (SET-051).
fn setting_values(t: &mut Transaction<'_>) -> StoreResult<Option<Arc<SettingValues>>> {
    let rows = select_strings(
        t,
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
    let mut revisions = BTreeMap::new();
    for row in &t
        .query("SELECT setting, revision FROM setting_revisions", &[])
        .map_err(db)?
    {
        let setting: String = get(row, 0)?;
        let revision: i64 = get(row, 1)?;
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
fn write_setting_override(t: &mut Transaction<'_>, held: &SettingOverride) -> StoreResult<()> {
    let revision: i64 = held.revision as i64;
    t.execute(
        "INSERT INTO setting_overrides (setting, level, subject, revision, data)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (setting, level, subject)
         DO UPDATE SET revision = EXCLUDED.revision, data = EXCLUDED.data",
        &[
            &held.setting.as_str(),
            &held.level.as_str(),
            &held.subject,
            &revision,
            &encode(held),
        ],
    )
    .map_err(db)?;
    Ok(())
}

/// Write the revision of one setting, replacing the stored one.
fn write_setting_revision(
    t: &mut Transaction<'_>,
    setting: &SettingId,
    revision: u64,
) -> StoreResult<()> {
    let revision: i64 = revision as i64;
    t.execute(
        "INSERT INTO setting_revisions (setting, revision) VALUES ($1, $2)
         ON CONFLICT (setting) DO UPDATE SET revision = EXCLUDED.revision",
        &[&setting.as_str(), &revision],
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

/// The records of `object` whose every field in `fields` holds its value (D-6558, CMS-1557):
/// from the record index when every value fits it, otherwise by reading the records of the type.
fn by_fields(
    t: &mut Transaction<'_>,
    object: &ObjectTypeId,
    fields: &BTreeMap<strata_kernel::FieldId, Value>,
) -> StoreResult<Vec<String>> {
    let keys: Vec<(&str, String)> = fields
        .iter()
        .map(|(field, value)| (field.as_str(), encode(value)))
        .collect();
    if keys.is_empty() || keys.iter().any(|(_, key)| key.len() > MAX_INDEXED) {
        let mut matching = Vec::new();
        for data in all_of(t, object)? {
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
         WHERE i0.object = $1 AND i0.field = $2 AND i0.value = $3",
    );
    for n in 1..keys.len() {
        let (f, v) = (2 * n + 2, 2 * n + 3);
        sql.push_str(&format!(
            " AND EXISTS (SELECT 1 FROM record_index i WHERE i.object = i0.object AND i.id = i0.id AND i.field = ${f} AND i.value = ${v})"
        ));
    }
    sql.push_str(" ORDER BY r.id");
    let mut owned: Vec<String> = vec![object.as_str().to_string()];
    for (field, key) in keys {
        owned.push(field.to_string());
        owned.push(key);
    }
    let args: Vec<&(dyn ToSql + Sync)> = owned
        .iter()
        .map(|text| text as &(dyn ToSql + Sync))
        .collect();
    select_strings(t, &sql, &args)
}

/// The complete state of the tenant in its schema.
fn complete(t: &mut Transaction<'_>, tenant: &TenantId) -> StoreResult<Option<TenantState>> {
    let Some(state) = header(t, tenant)? else {
        return Ok(None);
    };
    complete_from(t, state).map(Some)
}

/// The complete state that grows from a header: every record and the ledger.
fn complete_from(t: &mut Transaction<'_>, mut state: TenantState) -> StoreResult<TenantState> {
    let rows = select_strings(t, "SELECT data FROM records ORDER BY object, id", &[])?;
    put_records(&mut state, rows)?;
    ledger::load_all(t, &mut state)?;
    ledger::tidy(&mut state);
    Ok(state)
}

fn write_record(t: &mut Transaction<'_>, record: &Record) -> StoreResult<()> {
    let (object, id) = (record.object.as_str(), record.id.0 as i64);
    // A record is created at version 1 and every later write raises its version, so a version 1
    // write over a stored row would overwrite a record that already exists.
    if record.version == 1 {
        let row = t
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM records WHERE object = $1 AND id = $2)",
                &[&object, &id],
            )
            .map_err(db)?;
        let exists: bool = get(&row, 0)?;
        if exists {
            return Err(StoreError::Invalid(format!(
                "create would overwrite an existing record {id} of {object}"
            )));
        }
    }
    let version: i64 = record.version as i64;
    t.execute(
        "INSERT INTO records (object, id, version, data) VALUES ($1, $2, $3, $4)
         ON CONFLICT (object, id) DO UPDATE SET version = EXCLUDED.version, data = EXCLUDED.data",
        &[&object, &id, &version, &encode(record)],
    )
    .map_err(db)?;
    t.execute(
        "DELETE FROM record_index WHERE object = $1 AND id = $2",
        &[&object, &id],
    )
    .map_err(db)?;
    index_page::insert_rows(t, record)?;
    pair_index::write_rows(t, record)
}

/// Raise the change counter of one object type ("" for all of them) by one.
fn raise_object_revision(t: &mut Transaction<'_>, object: &str) -> StoreResult<()> {
    t.execute(
        "INSERT INTO object_revisions (object, revision) VALUES ($1, 1)
         ON CONFLICT (object) DO UPDATE SET revision = object_revisions.revision + 1",
        &[&object],
    )
    .map_err(db)?;
    Ok(())
}

/// The form of a schema the history keeps: without its teams and invitations (WG-5111).
fn history_form(schema: &TenantSchema) -> TenantSchema {
    TenantSchema {
        teams: None,
        invitations: None,
        ..schema.clone()
    }
}

/// A throttle counter of the tenant.
fn throttle_row(t: &mut Transaction<'_>, key: &str) -> StoreResult<Option<ThrottleRow>> {
    let Some(row) = t
        .query_opt(
            "SELECT failures, window_start, locked_until, expires_at FROM login_throttle WHERE key = $1",
            &[&key],
        )
        .map_err(db)?
    else {
        return Ok(None);
    };
    let failures: i64 = get(&row, 0)?;
    Ok(Some(ThrottleRow {
        failures: failures as u32,
        window_start: get(&row, 1)?,
        locked_until: get(&row, 2)?,
        expires_at: get(&row, 3)?,
    }))
}

const PASSKEY_COLUMNS: &str = "id, principal, rp_id, label, credential, created_at, last_used_at";

fn passkey_record(tenant: &TenantId, row: &Row) -> StoreResult<PasskeyRecord> {
    let principal: String = get(row, 1)?;
    let created_at: i64 = get(row, 5)?;
    let last_used_at: Option<i64> = get(row, 6)?;
    Ok(PasskeyRecord {
        tenant: tenant.clone(),
        principal: PrincipalId::new(principal).map_err(corrupt_id)?,
        id: get(row, 0)?,
        rp_id: get(row, 2)?,
        label: get(row, 3)?,
        credential: get(row, 4)?,
        created_at: Timestamp(created_at),
        last_used_at: last_used_at.map(Timestamp),
    })
}

const RECOVERY_COLUMNS: &str =
    "principal, login, token_hash, cancel_hash, requested_at, completes_at, expires_at";

fn recovery_request(tenant: &TenantId, id_hash: &str, row: &Row) -> StoreResult<RecoveryRequest> {
    let principal: String = get(row, 0)?;
    let requested_at: i64 = get(row, 4)?;
    let completes_at: i64 = get(row, 5)?;
    let expires_at: i64 = get(row, 6)?;
    Ok(RecoveryRequest {
        tenant: tenant.clone(),
        id_hash: id_hash.to_string(),
        principal: PrincipalId::new(principal).map_err(corrupt_id)?,
        login: get(row, 1)?,
        token_hash: get(row, 2)?,
        cancel_hash: get(row, 3)?,
        requested_at: Timestamp(requested_at),
        completes_at: Timestamp(completes_at),
        expires_at: Timestamp(expires_at),
    })
}

const PENDING_COLUMNS: &str = "principal, method, factors, attempts, expires_at";

fn pending_sign_in(tenant: &TenantId, id_hash: &str, row: &Row) -> StoreResult<PendingSignIn> {
    let principal: String = get(row, 0)?;
    let factors: i64 = get(row, 2)?;
    let attempts: i64 = get(row, 3)?;
    let expires_at: i64 = get(row, 4)?;
    Ok(PendingSignIn {
        tenant: tenant.clone(),
        id_hash: id_hash.to_string(),
        principal: PrincipalId::new(principal).map_err(corrupt_id)?,
        method: get(row, 1)?,
        factors: factors as u8,
        attempts: attempts as u32,
        expires_at: Timestamp(expires_at),
    })
}

const SESSION_COLUMNS: &str =
    "id, token_hash, csrf_token, principal, display_name, created_at, last_seen_at, expires_at, revoked_at, authenticated_at, network";

fn session_record(tenant: &TenantId, row: &Row) -> StoreResult<SessionRecord> {
    let id: String = get(row, 0)?;
    let principal: String = get(row, 3)?;
    let created: i64 = get(row, 5)?;
    let seen: i64 = get(row, 6)?;
    let expires: i64 = get(row, 7)?;
    let revoked: Option<i64> = get(row, 8)?;
    let authenticated: i64 = get(row, 9)?;
    Ok(SessionRecord {
        id: SessionId::new(id).map_err(corrupt_id)?,
        token_hash: get(row, 1)?,
        csrf_token: get(row, 2)?,
        tenant: tenant.clone(),
        principal: PrincipalId::new(principal).map_err(corrupt_id)?,
        display_name: get(row, 4)?,
        created_at: Timestamp(created),
        last_seen_at: Timestamp(seen),
        expires_at: Timestamp(expires),
        revoked_at: revoked.map(Timestamp),
        authenticated_at: Timestamp(authenticated),
        network: get(row, 10)?,
    })
}

/// The sessions of the tenant that meet `clause`, ordered by id.
fn sessions_where(
    t: &mut Transaction<'_>,
    tenant: &TenantId,
    clause: &str,
    args: &[&(dyn ToSql + Sync)],
) -> StoreResult<Vec<SessionRecord>> {
    let sql = format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE {clause} ORDER BY id");
    let rows = t.query(sql.as_str(), args).map_err(db)?;
    rows.iter().map(|row| session_record(tenant, row)).collect()
}

fn account_from(tenant: &TenantId, login: String, row: &Row, from: usize) -> StoreResult<Account> {
    let principal: String = get(row, from)?;
    Ok(Account {
        tenant: tenant.clone(),
        login,
        principal: PrincipalId::new(principal).map_err(corrupt_id)?,
        display_name: get(row, from + 1)?,
        password_hash: get(row, from + 2)?,
        verified_email: get(row, from + 3)?,
    })
}

// ---- Jobs of the automation worker (strata_storage_api::jobs) ----

/// The first problem of a list of jobs to create, as a refusal.
fn check_jobs(new: &[NewJob]) -> StoreResult<()> {
    match new.iter().find_map(jobs::job_problem) {
        Some(problem) => Err(StoreError::Invalid(problem)),
        None => Ok(()),
    }
}

/// Create the jobs whose keys the tenant does not hold yet; the ids of all, in order.
fn insert_jobs(
    t: &mut Transaction<'_>,
    tenant: &TenantId,
    new: &[NewJob],
    now: Timestamp,
) -> StoreResult<Vec<String>> {
    check_jobs(new)?;
    let mut ids = Vec::with_capacity(new.len());
    for wanted in new {
        let job = jobs::new_job(tenant, wanted, now);
        t.execute(
            "INSERT INTO jobs (id, key, state, due_at, data) VALUES ($1, $2, $3, $4, $5::text::jsonb)
             ON CONFLICT DO NOTHING",
            &[
                &job.id,
                &job.key,
                &job.state.as_str(),
                &job.due_at.0,
                &encode(&job),
            ],
        )
        .map_err(db)?;
        ids.push(job.id);
    }
    Ok(ids)
}

/// Write a changed job back.
fn put_job(t: &mut Transaction<'_>, job: &Job) -> StoreResult<()> {
    t.execute(
        "UPDATE jobs SET state = $2, due_at = $3, data = $4::text::jsonb WHERE id = $1",
        &[&job.id, &job.state.as_str(), &job.due_at.0, &encode(job)],
    )
    .map_err(db)?;
    Ok(())
}

/// The rows of `(seq, data)` a statement selects.
fn seq_rows(
    t: &mut Transaction<'_>,
    sql: &str,
    args: &[&(dyn ToSql + Sync)],
) -> StoreResult<Vec<(u64, String)>> {
    let rows = t.query(sql, args).map_err(db)?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let seq: i64 = get(row, 0)?;
        let data: String = get(row, 1)?;
        out.push((seq as u64, data));
    }
    Ok(out)
}

/// The jobs a statement over `(seq, data::text)` selects.
fn jobs_of(
    t: &mut Transaction<'_>,
    sql: &str,
    args: &[&(dyn ToSql + Sync)],
) -> StoreResult<Vec<Job>> {
    seq_rows(t, sql, args)?
        .into_iter()
        .map(|(seq, data)| {
            let mut job: Job = decode(&data, "job")?;
            job.seq = seq;
            Ok(job)
        })
        .collect()
}

fn job_by_id(t: &mut Transaction<'_>, id: &str) -> StoreResult<Option<Job>> {
    Ok(jobs_of(t, "SELECT seq, data::text FROM jobs WHERE id = $1", &[&id])?.pop())
}

fn runs_of(
    t: &mut Transaction<'_>,
    sql: &str,
    args: &[&(dyn ToSql + Sync)],
) -> StoreResult<Vec<RunRecord>> {
    seq_rows(t, sql, args)?
        .into_iter()
        .map(|(seq, data)| {
            let mut run: RunRecord = decode(&data, "automation run")?;
            run.seq = seq;
            Ok(run)
        })
        .collect()
}

fn exceptions_of(
    t: &mut Transaction<'_>,
    sql: &str,
    args: &[&(dyn ToSql + Sync)],
) -> StoreResult<Vec<ExceptionRecord>> {
    seq_rows(t, sql, args)?
        .into_iter()
        .map(|(seq, data)| {
            let mut record: ExceptionRecord = decode(&data, "automation exception")?;
            record.seq = seq;
            Ok(record)
        })
        .collect()
}

/// Exception records whose ids the tenant does not hold yet.
fn insert_exceptions(t: &mut Transaction<'_>, records: &[ExceptionRecord]) -> StoreResult<()> {
    for record in records {
        let settled: i64 = i64::from(record.settled);
        t.execute(
            "INSERT INTO automation_exceptions (id, kind, settled, data) VALUES ($1, $2, $3, $4::text::jsonb)
             ON CONFLICT DO NOTHING",
            &[&record.id, &record.kind.as_str(), &settled, &encode(record)],
        )
        .map_err(db)?;
    }
    Ok(())
}

/// Everything of `writes`, as `Store::automation_write` states, in the caller's transaction.
fn write_automation(
    t: &mut Transaction<'_>,
    tenant: &TenantId,
    writes: &WorkerWrites,
    now: Timestamp,
) -> StoreResult<()> {
    for run in &writes.runs {
        t.execute(
            "INSERT INTO automation_runs (run, automation, state, data) VALUES ($1, $2, $3, $4::text::jsonb)
             ON CONFLICT (run) DO UPDATE SET automation = EXCLUDED.automation, state = EXCLUDED.state, data = EXCLUDED.data",
            &[
                &run.run,
                &run.automation.as_str(),
                &run.state.as_str(),
                &encode(run),
            ],
        )
        .map_err(db)?;
    }
    for step in &writes.steps {
        let sequence: i64 = step.sequence as i64;
        t.execute(
            "INSERT INTO automation_steps (run, sequence, data) VALUES ($1, $2, $3)
             ON CONFLICT (run, sequence) DO UPDATE SET data = EXCLUDED.data",
            &[&step.run, &sequence, &encode(step)],
        )
        .map_err(db)?;
    }
    insert_exceptions(t, &writes.exceptions)?;
    insert_jobs(t, tenant, &writes.jobs, now)?;
    for claim in &writes.run_keys {
        t.execute(
            "INSERT INTO automation_run_keys (automation, key, run) VALUES ($1, $2, $3)
             ON CONFLICT DO NOTHING",
            &[&claim.automation.as_str(), &claim.key, &claim.run],
        )
        .map_err(db)?;
    }
    for automation in &writes.run_keys_released {
        t.execute(
            "DELETE FROM automation_run_keys WHERE automation = $1",
            &[&automation.as_str()],
        )
        .map_err(db)?;
    }
    if let Some(advance) = &writes.schedule_advance {
        if let Some(held) = schedule_by_id(t, &advance.schedule)? {
            let (next, create) = schedules::apply_completion(&held, advance);
            put_schedule(t, &next)?;
            if let Some(job) = create {
                insert_jobs(t, tenant, std::slice::from_ref(&job), now)?;
            }
        }
    }
    Ok(())
}

/// The earliest time a job that `writes` creates is due, the next job of a schedule included.
fn earliest_due(writes: &WorkerWrites) -> Option<i64> {
    writes
        .jobs
        .iter()
        .map(|job| job.due_at.0)
        .chain(
            writes
                .schedule_advance
                .iter()
                .filter_map(|advance| advance.next_due.map(|due| due.0)),
        )
        .min()
}

/// One schedule of the tenant.
fn schedule_by_id(t: &mut Transaction<'_>, id: &str) -> StoreResult<Option<Schedule>> {
    let found = select_strings(t, "SELECT data FROM schedules WHERE id = $1", &[&id])?;
    found
        .into_iter()
        .next()
        .map(|data| decode(&data, "schedule"))
        .transpose()
}

/// Write a schedule, replacing the one of its id.
fn put_schedule(t: &mut Transaction<'_>, schedule: &Schedule) -> StoreResult<()> {
    t.execute(
        "INSERT INTO schedules (id, data) VALUES ($1, $2)
         ON CONFLICT (id) DO UPDATE SET data = EXCLUDED.data",
        &[&schedule.id, &encode(schedule)],
    )
    .map_err(db)?;
    Ok(())
}

/// The jobs and exception records of a commit, in its transaction (SCH-004, SCH-1532).
fn insert_commit_work(t: &mut Transaction<'_>, commit: &Commit) -> StoreResult<()> {
    insert_jobs(
        t,
        &commit.delta.tenant,
        &commit.jobs,
        commit.journal.context.now,
    )?;
    insert_exceptions(t, &commit.exceptions)?;
    // The audit lines of the ledger entries of the commit (LDG-2925): a line that cannot be
    // written refuses the commit, which then changes nothing (LDG-2926).
    for entry in &commit.audit {
        append_audit(t, entry)?;
    }
    Ok(())
}

/// One line of the audit trail, chained to the line before it, in the transaction `t`.
fn append_audit(t: &mut Transaction<'_>, entry: &AuditEntry) -> StoreResult<()> {
    let previous = select_strings(
        t,
        "SELECT chain FROM audit_log WHERE chain IS NOT NULL ORDER BY seq DESC LIMIT 1",
        &[],
    )?;
    let chain = strata_storage_api::audit_chain::audit_link(
        previous.first().map(String::as_str).unwrap_or(""),
        entry,
    );
    t.execute(
        "INSERT INTO audit_log (seq, at, kind, event, correlation, data, chain)
         VALUES ((SELECT COALESCE(MAX(seq), 0) + 1 FROM audit_log), $1, $2, $3, $4, $5, $6)",
        &[
            &entry.at,
            &entry.kind,
            &entry.event,
            &entry.correlation,
            &entry.data,
            &chain,
        ],
    )
    .map_err(db)?;
    Ok(())
}

// ---- The segment block (tenant step 15) ----

fn segment_id(text: &str) -> StoreResult<seg::SegmentId> {
    serde_json::from_value(serde_json::Value::String(text.to_string()))
        .map_err(|e| StoreError::Corrupt(format!("segment id {text}: {e}")))
}

fn principal(text: &str) -> StoreResult<PrincipalId> {
    PrincipalId::new(text).map_err(corrupt_id)
}

fn catalog_rows(t: &mut Transaction<'_>) -> StoreResult<Vec<seg::Segment>> {
    let rows = t
        .query("SELECT data, member_count FROM segments ORDER BY id", &[])
        .map_err(db)?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let data: String = get(row, 0)?;
        let count: i64 = get(row, 1)?;
        let mut segment: seg::Segment = decode(&data, "segment")?;
        segment.member_count = count.max(0) as u64;
        out.push(segment);
    }
    Ok(out)
}

fn membership_of(
    t: &mut Transaction<'_>,
    segment: seg::SegmentId,
    person: &PrincipalId,
) -> StoreResult<Option<seg::Membership>> {
    let Some(row) = t
        .query_opt(
            "SELECT version, since FROM segment_memberships WHERE segment = $1 AND principal = $2",
            &[&segment.to_string(), &person.as_str()],
        )
        .map_err(db)?
    else {
        return Ok(None);
    };
    let version: i64 = get(&row, 0)?;
    let since: i64 = get(&row, 1)?;
    Ok(Some(seg::Membership {
        segment,
        person: person.clone(),
        version: version as u64,
        since: Timestamp(since),
    }))
}

fn wake_of(
    t: &mut Transaction<'_>,
    segment: seg::SegmentId,
    person: &PrincipalId,
) -> StoreResult<Option<Timestamp>> {
    let Some(row) = t
        .query_opt(
            "SELECT wake_at FROM segment_wakes WHERE segment = $1 AND principal = $2",
            &[&segment.to_string(), &person.as_str()],
        )
        .map_err(db)?
    else {
        return Ok(None);
    };
    let at: i64 = get(&row, 0)?;
    Ok(Some(Timestamp(at)))
}

fn memberships_where(
    t: &mut Transaction<'_>,
    sql: &str,
    args: &[&(dyn ToSql + Sync)],
) -> StoreResult<Vec<seg::Membership>> {
    let rows = t.query(sql, args).map_err(db)?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let segment: String = get(row, 0)?;
        let person: String = get(row, 1)?;
        let version: i64 = get(row, 2)?;
        let since: i64 = get(row, 3)?;
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
    t: &mut Transaction<'_>,
    sql: &str,
    args: &[&(dyn ToSql + Sync)],
) -> StoreResult<Vec<seg::Wake>> {
    let rows = t.query(sql, args).map_err(db)?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let segment: String = get(row, 0)?;
        let person: String = get(row, 1)?;
        let at: i64 = get(row, 2)?;
        out.push(seg::Wake {
            segment: segment_id(&segment)?,
            person: principal(&person)?,
            at: Timestamp(at),
        });
    }
    Ok(out)
}

/// Apply a segment write in the transaction `t`: refused whole, as a conflict, when a segment or
/// a row it rests on is not what the decision saw (the transaction then rolls back).
fn apply_segments(t: &mut Transaction<'_>, write: &seg::SegmentWrite) -> StoreResult<()> {
    if let Some((before, _)) = write.published {
        if segment_cursor(t)? != before {
            return Err(write_conflict());
        }
    }
    for (id, expected) in &write.expect {
        let found: Option<i64> = match t
            .query_opt(
                "SELECT version FROM segments WHERE id = $1",
                &[&id.to_string()],
            )
            .map_err(db)?
        {
            Some(row) => Some(get(&row, 0)?),
            None => None,
        };
        if found.map(|v| v as u64) != *expected {
            return Err(write_conflict());
        }
    }
    for change in &write.memberships {
        if membership_of(t, change.segment, &change.person)? != change.before {
            return Err(write_conflict());
        }
    }
    for seen in &write.seen {
        if membership_of(t, seen.segment, &seen.person)? != seen.membership
            || wake_of(t, seen.segment, &seen.person)? != seen.wake
        {
            return Err(write_conflict());
        }
    }
    for segment in &write.put_segments {
        let version: i64 = segment.version as i64;
        t.execute(
            "INSERT INTO segments (id, name, version, member_count, data) VALUES ($1, $2, $3, 0, $4)
             ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name, version = EXCLUDED.version, data = EXCLUDED.data",
            &[&segment.id.to_string(), &segment.name, &version, &encode(segment)],
        )
        .map_err(db)?;
    }
    for id in &write.remove_segments {
        t.execute("DELETE FROM segments WHERE id = $1", &[&id.to_string()])
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
                let version: i64 = m.version as i64;
                t.execute(
                    "INSERT INTO segment_memberships (segment, principal, version, since) VALUES ($1, $2, $3, $4)
                     ON CONFLICT (segment, principal) DO UPDATE SET version = EXCLUDED.version, since = EXCLUDED.since",
                    &[&segment, &person, &version, &m.since.0],
                )
                .map_err(db)?;
            }
            None => {
                t.execute(
                    "DELETE FROM segment_memberships WHERE segment = $1 AND principal = $2",
                    &[&segment, &person],
                )
                .map_err(db)?;
            }
        }
        if delta != 0 {
            t.execute(
                "UPDATE segments SET member_count = GREATEST(member_count + $1, 0) WHERE id = $2",
                &[&delta, &segment],
            )
            .map_err(db)?;
        }
    }
    for change in &write.wakes {
        match change {
            seg::WakeChange::Put(w) => t.execute(
                "INSERT INTO segment_wakes (segment, principal, wake_at) VALUES ($1, $2, $3)
                 ON CONFLICT (segment, principal) DO UPDATE SET wake_at = EXCLUDED.wake_at",
                &[&w.segment.to_string(), &w.person.as_str(), &w.at.0],
            ),
            seg::WakeChange::Remove { segment, person } => t.execute(
                "DELETE FROM segment_wakes WHERE segment = $1 AND principal = $2",
                &[&segment.to_string(), &person.as_str()],
            ),
        }
        .map_err(db)?;
    }
    for id in &write.clear_batches {
        t.execute(
            "DELETE FROM segment_batches WHERE segment = $1",
            &[&id.to_string()],
        )
        .map_err(db)?;
    }
    for change in &write.batches {
        match change {
            seg::BatchChange::Put(b) => t.execute(
                "INSERT INTO segment_batches (segment, scope, data)
                 SELECT $1::text, $2::text, $3::text WHERE EXISTS (SELECT 1 FROM segments WHERE id = $1::text)
                 ON CONFLICT (segment, scope) DO UPDATE SET data = EXCLUDED.data",
                &[&b.segment.to_string(), &b.scope.key(), &encode(b)],
            ),
            seg::BatchChange::Remove { segment, key } => t.execute(
                "DELETE FROM segment_batches WHERE segment = $1 AND scope = $2",
                &[&segment.to_string(), key],
            ),
        }
        .map_err(db)?;
    }
    for person in &write.erased {
        t.execute(
            "INSERT INTO segment_erased (principal) VALUES ($1) ON CONFLICT DO NOTHING",
            &[&person.as_str()],
        )
        .map_err(db)?;
    }
    for (id, at) in &write.refresh_skipped {
        let found = select_strings(
            t,
            "SELECT data FROM segments WHERE id = $1",
            &[&id.to_string()],
        )?;
        if let Some(data) = found.into_iter().next() {
            let mut segment: seg::Segment = decode(&data, "segment")?;
            segment.refresh_skipped_at = Some(*at);
            t.execute(
                "UPDATE segments SET data = $1 WHERE id = $2",
                &[&encode(&segment), &id.to_string()],
            )
            .map_err(db)?;
        }
    }
    for event in &write.events {
        // The next position after the highest, under the tenant's writer lock: the log is never
        // cut, so a position is never handed out twice.
        t.execute(
            "INSERT INTO segment_events (position, data)
             VALUES ((SELECT COALESCE(MAX(position), 0) + 1 FROM segment_events), $1)",
            &[&encode(event)],
        )
        .map_err(db)?;
    }
    if let Some((_, after)) = write.published {
        let after: i64 = after as i64;
        t.execute(
            "UPDATE segment_events SET published = 1 WHERE position <= $1",
            &[&after],
        )
        .map_err(db)?;
    }
    Ok(())
}

/// The position of the segment event log handed on to the job worker: the newest event marked
/// published, 0 before any (D-7280).
fn segment_cursor(t: &mut Transaction<'_>) -> StoreResult<u64> {
    count(
        t,
        "SELECT COALESCE(MAX(position), 0)::bigint FROM segment_events WHERE published = 1",
        &[],
    )
}

// ---------------------------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------------------------

/// The PostgreSQL store. See the crate documentation for the placement and the transactions.
pub struct PostgresStore {
    /// The control schema of the control connection's database.
    control: Scope,
    placement: Box<dyn Placement>,
    /// One pool per database, by connection string.
    pools: Mutex<BTreeMap<String, Arc<Pool>>>,
    /// The tenants whose schema this process has brought up to date, with their scope.
    open: Mutex<BTreeMap<TenantId, Scope>>,
    /// Held while a tenant is created, so that two creations in this process do not interleave.
    creating: Mutex<()>,
    /// Where the next outbox sweep starts among the tenants.
    sweep: AtomicUsize,
    /// Outbox problems skipped past, most recent last.
    faults: Mutex<Vec<OutboxFault>>,
    /// Whether the control server keeps acknowledged commits (`fsync` on, `synchronous_commit`
    /// not off), read when the store was connected.
    durable: bool,
    /// The control connection without its user and password, for the diagnostics.
    control_location: String,
    /// Where the nonce of a new generation comes from: the operating system's random source
    /// unless a test or the simulator gives a seeded one ([`PostgresStore::with_nonces`]).
    nonces: Arc<dyn NonceSource>,
    /// Tenants an install announced ([`Store::begin_provisioning`]) whose creating commit has not
    /// happened yet: that commit writes their directory row as `provisioning`.
    announced: Mutex<BTreeSet<TenantId>>,
}

impl PostgresStore {
    /// Connect to the control database at `control_url`, with every tenant in a schema of its own
    /// in the same database ([`SchemaPerTenant`]), and bring the control layout up to date.
    pub fn connect(control_url: &str, now: Timestamp) -> StoreResult<Self> {
        Self::connect_with(
            control_url,
            Box::new(SchemaPerTenant::new(control_url)),
            now,
        )
    }

    /// Connect to the control database at `control_url`, with the tenants where `placement`
    /// puts them, and bring the control layout up to date. Removals a previous process did not
    /// finish are finished.
    pub fn connect_with(
        control_url: &str,
        placement: Box<dyn Placement>,
        now: Timestamp,
    ) -> StoreResult<Self> {
        let pool = Arc::new(Pool::new(control_url));
        let control = Scope {
            pool: pool.clone(),
            schema: CONTROL_SCHEMA.to_string(),
        };
        pool.with(|client| {
            let mut t = client.transaction().map_err(db)?;
            t.execute(
                "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
                &[&format!("strata:{CONTROL_SCHEMA}")],
            )
            .map_err(db)?;
            t.batch_execute(&format!(
                "CREATE SCHEMA IF NOT EXISTS {CONTROL_SCHEMA}; SET LOCAL search_path TO {CONTROL_SCHEMA}"
            ))
            .map_err(db)?;
            migrate(&mut t, CONTROL_LEDGER, CONTROL_MIGRATIONS, now.0)?;
            t.commit().map_err(db)
        })?;
        let durable = pool.with(|client| {
            let fsync: String = get(&client.query_one("SHOW fsync", &[]).map_err(db)?, 0)?;
            let commit: String = get(
                &client
                    .query_one("SHOW synchronous_commit", &[])
                    .map_err(db)?,
                0,
            )?;
            Ok(fsync == "on" && commit != "off")
        })?;
        let mut pools = BTreeMap::new();
        pools.insert(control_url.to_string(), pool);
        let store = PostgresStore {
            control,
            placement,
            pools: Mutex::new(pools),
            open: Mutex::new(BTreeMap::new()),
            creating: Mutex::new(()),
            announced: Mutex::new(BTreeSet::new()),
            sweep: AtomicUsize::new(0),
            faults: Mutex::new(Vec::new()),
            durable,
            control_location: redact(control_url),
            nonces: random_nonces(),
        };
        store.finish_deletions();
        Ok(store)
    }

    /// Draw the nonces of new generations from `nonces`: a seeded source, so that a run is
    /// reproduced from its seed.
    pub fn with_nonces(mut self, nonces: Arc<dyn NonceSource>) -> Self {
        self.nonces = nonces;
        self
    }

    /// The schema of a tenant that exists in the directory as active, or `None`.
    pub fn tenant_schema_of(&self, tenant: &TenantId) -> StoreResult<Option<String>> {
        self.directory_schema(tenant, "active")
    }

    /// How many tenants this process has open (their schema brought up to date).
    pub fn open_count(&self) -> usize {
        self.lock_open().len()
    }

    /// Write every search entry of a tenant again from its records and schema, in one transaction.
    pub fn rebuild_search_index(&self, tenant: &TenantId) -> StoreResult<()> {
        self.existing(tenant)?.write(search_index::rebuild_all)
    }

    /// The search entries stored for one record, as `field:token`, in order.
    pub fn search_entries_of(
        &self,
        tenant: &TenantId,
        object: &ObjectTypeId,
        id: strata_kernel::RecordId,
    ) -> StoreResult<Vec<String>> {
        self.existing(tenant)?
            .read(|t| search_index::stored_entries(t, object, id))
    }

    fn lock_open(&self) -> MutexGuard<'_, BTreeMap<TenantId, Scope>> {
        // The map is only inserted into and removed from, so a panic leaves it whole.
        self.open
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn cached(&self, tenant: &TenantId) -> Option<Scope> {
        self.lock_open().get(tenant).cloned()
    }

    fn forget(&self, tenant: &TenantId) {
        self.lock_open().remove(tenant);
    }

    /// The pool of one database, made on first use.
    fn pool_for(&self, url: &str) -> Arc<Pool> {
        let mut pools = self
            .pools
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pools
            .entry(url.to_string())
            .or_insert_with(|| Arc::new(Pool::new(url)))
            .clone()
    }

    /// The scope of a tenant whose schema is `schema`: that schema, in the database the placement
    /// rule names.
    fn scope_of(&self, tenant: &TenantId, schema: &str) -> StoreResult<Scope> {
        quoted(schema)?;
        let location = self.placement.place(tenant);
        Ok(Scope {
            pool: self.pool_for(&location.url),
            schema: schema.to_string(),
        })
    }

    /// The schema a directory row in `state` names for a tenant.
    fn directory_schema(&self, tenant: &TenantId, state: &str) -> StoreResult<Option<String>> {
        self.control.read(|t| {
            let found = select_strings(
                t,
                "SELECT schema_name FROM tenant_directory WHERE id = $1 AND state = $2",
                &[&tenant.as_str(), &state],
            )?;
            Ok(found.into_iter().next())
        })
    }

    /// Every tenant of the directory that is active, by id.
    fn directory(&self) -> StoreResult<Vec<TenantId>> {
        self.ids_in_state("active")
    }

    fn ids_in_state(&self, state: &str) -> StoreResult<Vec<TenantId>> {
        self.control
            .read(|t| {
                select_strings(
                    t,
                    "SELECT id FROM tenant_directory WHERE state = $1 ORDER BY id",
                    &[&state],
                )
            })?
            .into_iter()
            .map(|id| TenantId::new(id).map_err(corrupt_id))
            .collect()
    }

    /// Bring a tenant schema up to date: create it when it is missing, apply the steps it lacks,
    /// and do the rebuilds steps 16 to 18 mark, in one transaction under the schema's writer lock.
    fn prepare(&self, scope: &Scope) -> StoreResult<()> {
        // The ledger records when the step ran, which is now and not when the process started.
        let ran_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        let schema = quoted(&scope.schema)?;
        scope.write(|t| {
            t.batch_execute(&format!("CREATE SCHEMA IF NOT EXISTS {schema}"))
                .map_err(db)?;
            migrate(t, TENANT_LEDGER, TENANT_MIGRATIONS, ran_at)?;
            index_page::rebuild_if_marked(t)?;
            pair_index::sync_stored(t)?;
            search_index::rebuild_if_marked(t)
        })
    }

    /// The scope of an active tenant, its schema brought up to date on first use in this
    /// process, or `None` when the directory has no such tenant.
    fn tenant_db(&self, tenant: &TenantId) -> StoreResult<Option<Scope>> {
        if let Some(found) = self.cached(tenant) {
            return Ok(Some(found));
        }
        let Some(schema) = self.directory_schema(tenant, "active")? else {
            return Ok(None);
        };
        let scope = self.scope_of(tenant, &schema)?;
        self.prepare(&scope)?;
        let kept = self
            .lock_open()
            .entry(tenant.clone())
            .or_insert(scope)
            .clone();
        Ok(Some(kept))
    }

    /// Refuse a placement that puts a new tenant in the schema of another tenant of the same
    /// database.
    fn check_collision(&self, tenant: &TenantId, scope: &Scope) -> StoreResult<()> {
        let others = self.control.read(|t| {
            select_strings(
                t,
                "SELECT id FROM tenant_directory WHERE schema_name = $1 AND id <> $2",
                &[&scope.schema, &tenant.as_str()],
            )
        })?;
        for other in others {
            let other = TenantId::new(other).map_err(corrupt_id)?;
            if self.placement.place(&other).url == scope.pool.url() {
                return Err(StoreError::Invalid(format!(
                    "the placement puts {tenant} in the schema {} of {other}",
                    scope.schema
                )));
            }
        }
        Ok(())
    }

    /// The scope of a tenant, creating its directory row and schema when there is none. Returns
    /// whether this call created it.
    fn create_tenant_db(&self, tenant: &TenantId, now: Timestamp) -> StoreResult<(Scope, bool)> {
        let _turn = self
            .creating
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(found) = self.tenant_db(tenant)? {
            return Ok((found, false));
        }
        // A `deleting` row is a removal a previous process did not finish, and a `provisioning`
        // row is an install it did not finish: either way what is there is cleared first.
        if self.directory_schema(tenant, "deleting")?.is_some()
            || self.directory_schema(tenant, "provisioning")?.is_some()
        {
            self.remove_tenant(tenant)?;
        }
        let location = self.placement.place(tenant);
        quoted(&location.schema)?;
        let scope = Scope {
            pool: self.pool_for(&location.url),
            schema: location.schema.clone(),
        };
        self.check_collision(tenant, &scope)?;
        let state = if self
            .announced
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(tenant)
        {
            "provisioning"
        } else {
            "active"
        };
        // The directory row claims the tenant first: a second creator, in another process, fails
        // on its key and finds the tenant made.
        let claimed = self.control.write(|t| {
            t.execute(
                "INSERT INTO tenant_directory (id, state, schema_name, created_at) VALUES ($1, $4, $2, $3)",
                &[&tenant.as_str(), &scope.schema, &now.0, &state],
            )
            .map_err(db)?;
            Ok(())
        });
        if let Err(error) = claimed {
            return match self.tenant_db(tenant)? {
                Some(found) => Ok((found, false)),
                None => Err(error),
            };
        }
        // The directory is the authority: a schema left behind without a row is stale.
        let schema = quoted(&scope.schema)?;
        scope.write(|t| {
            t.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
                .map_err(db)
        })?;
        self.prepare(&scope)?;
        self.lock_open().insert(tenant.clone(), scope.clone());
        Ok((scope, true))
    }

    /// Remove a tenant: mark it deleting, drop its schema, then drop its directory rows (the
    /// session directory and the wake index follow by cascade). A tenant that is not there is
    /// fine.
    fn remove_tenant(&self, tenant: &TenantId) -> StoreResult<()> {
        self.announced
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(tenant);
        let schema = self.control.read(|t| {
            let found = select_strings(
                t,
                "SELECT schema_name FROM tenant_directory WHERE id = $1",
                &[&tenant.as_str()],
            )?;
            Ok(found.into_iter().next())
        })?;
        let Some(schema) = schema else {
            self.forget(tenant);
            return Ok(());
        };
        self.control.write(|t| {
            t.execute(
                "UPDATE tenant_directory SET state = 'deleting' WHERE id = $1",
                &[&tenant.as_str()],
            )
            .map_err(db)?;
            Ok(())
        })?;
        self.forget(tenant);
        let scope = self.scope_of(tenant, &schema)?;
        let quoted_schema = quoted(&schema)?;
        scope.write(|t| {
            t.batch_execute(&format!("DROP SCHEMA IF EXISTS {quoted_schema} CASCADE"))
                .map_err(db)
        })?;
        self.control.write(|t| {
            t.execute(
                "DELETE FROM tenant_directory WHERE id = $1",
                &[&tenant.as_str()],
            )
            .map_err(db)?;
            Ok(())
        })
    }

    /// Move a tenant's directory row from one state to another. The tenant must be in `from`.
    fn set_directory_state(&self, tenant: &TenantId, from: &str, to: &str) -> StoreResult<()> {
        let changed = self.control.write(|t| {
            t.execute(
                "UPDATE tenant_directory SET state = $3 WHERE id = $1 AND state = $2",
                &[&tenant.as_str(), &from, &to],
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
        for tenant in self.ids_in_state("deleting").unwrap_or_default() {
            let _ = self.remove_tenant(&tenant);
        }
    }

    /// The scope of a tenant that exists (has its row), or `NotFound`.
    fn existing(&self, tenant: &TenantId) -> StoreResult<Scope> {
        let found = self
            .tenant_db(tenant)?
            .ok_or_else(|| StoreError::NotFound(format!("tenant {tenant}")))?;
        if found.read(|t| tenant_present(t, tenant))? {
            Ok(found)
        } else {
            Err(StoreError::NotFound(format!("tenant {tenant}")))
        }
    }

    /// Where a session lives, from the session directory. `column` is `id` or `token_hash`.
    fn session_tenant(&self, column: &str, value: &str) -> StoreResult<Option<TenantId>> {
        let sql = match column {
            "id" => "SELECT tenant FROM session_directory WHERE id = $1",
            _ => "SELECT tenant FROM session_directory WHERE token_hash = $1",
        };
        let found = self.control.read(|t| select_strings(t, sql, &[&value]))?;
        found
            .into_iter()
            .next()
            .map(|t| TenantId::new(t).map_err(corrupt_id))
            .transpose()
    }

    /// The session a directory lookup points at, read from its tenant's schema.
    fn session_where(&self, column: &str, value: &str) -> StoreResult<Option<SessionRecord>> {
        let Some(tenant) = self.session_tenant(column, value)? else {
            return Ok(None);
        };
        let Some(scope) = self.tenant_db(&tenant)? else {
            return Ok(None);
        };
        let clause = match column {
            "id" => "id = $1",
            _ => "token_hash = $1",
        };
        Ok(scope
            .read(|t| sessions_where(t, &tenant, clause, &[&value]))?
            .pop())
    }

    /// Read from the schema of a tenant, or answer `None` when the tenant has none.
    fn peek<T>(
        &self,
        tenant: &TenantId,
        work: impl FnOnce(&mut Transaction<'_>) -> StoreResult<T>,
    ) -> StoreResult<Option<T>> {
        let Some(scope) = self.tenant_db(tenant)? else {
            return Ok(None);
        };
        scope.read(work).map(Some)
    }

    /// Write to the schema of a tenant, or answer `None` when the tenant has none.
    fn on_tenant<T>(
        &self,
        tenant: &TenantId,
        work: impl FnOnce(&mut Transaction<'_>) -> StoreResult<T>,
    ) -> StoreResult<Option<T>> {
        let Some(scope) = self.tenant_db(tenant)? else {
            return Ok(None);
        };
        scope.write(work).map(Some)
    }

    /// Write to the schema of a tenant that exists, or answer `NotFound`.
    fn on_existing<T>(
        &self,
        tenant: &TenantId,
        work: impl FnOnce(&mut Transaction<'_>) -> StoreResult<T>,
    ) -> StoreResult<T> {
        self.existing(tenant)?.write(work)
    }

    /// Run one statement in every tenant's schema and count the rows it changed: the purges of
    /// what lapses, which are not about one tenant.
    fn on_every_tenant(&self, sql: &str, args: &[&(dyn ToSql + Sync)]) -> StoreResult<u64> {
        let mut changed = 0u64;
        for tenant in self.directory()? {
            let Some(scope) = self.tenant_db(&tenant)? else {
                continue;
            };
            changed += scope.write(|t| t.execute(sql, args).map_err(db))?;
        }
        Ok(changed)
    }

    /// Lower the wake time of a tenant to `due` (SCH-008).
    fn wake_at(&self, tenant: &TenantId, due: i64) -> StoreResult<()> {
        self.control.write(|t| {
            t.execute(
                "INSERT INTO job_wake (tenant, due_at, touched) VALUES ($1, $2, 1)
                 ON CONFLICT (tenant) DO UPDATE SET due_at = LEAST(job_wake.due_at, EXCLUDED.due_at),
                    touched = job_wake.touched + 1",
                &[&tenant.as_str(), &due],
            )
            .map_err(db)?;
            Ok(())
        })
    }

    /// Set the wake time of a tenant to the earliest time one of its jobs is due, or remove it
    /// when none is open. The counter is read first and the write made only if no other writer
    /// raised it since, so a time lowered by a job created meanwhile is never overwritten.
    fn wake_refresh(&self, tenant: &TenantId, scope: &Scope) -> StoreResult<()> {
        let seen: Option<i64> = self.control.read(|t| {
            match t
                .query_opt(
                    "SELECT touched FROM job_wake WHERE tenant = $1",
                    &[&tenant.as_str()],
                )
                .map_err(db)?
            {
                Some(row) => Ok(Some(get(&row, 0)?)),
                None => Ok(None),
            }
        })?;
        let next: Option<i64> = scope.read(|t| {
            let row = t
                .query_one(
                    &*format!("SELECT MIN(due_at) FROM jobs WHERE {OPEN_JOB}"),
                    &[],
                )
                .map_err(db)?;
            get(&row, 0)
        })?;
        match (seen, next) {
            (None, None) => Ok(()),
            (None, Some(due)) => self.wake_at(tenant, due),
            (Some(touched), None) => self.control.write(|t| {
                t.execute(
                    "DELETE FROM job_wake WHERE tenant = $1 AND touched = $2",
                    &[&tenant.as_str(), &touched],
                )
                .map_err(db)?;
                Ok(())
            }),
            (Some(touched), Some(due)) => self.control.write(|t| {
                t.execute(
                    "UPDATE job_wake SET due_at = $2, touched = touched + 1 WHERE tenant = $1 AND touched = $3",
                    &[&tenant.as_str(), &due, &touched],
                )
                .map_err(db)?;
                Ok(())
            }),
        }
    }

    /// Run `work` in a write transaction of a tenant, then refresh the tenant's wake time; `None`
    /// when the tenant has no schema. When the work may create jobs, `creates` is the earliest
    /// time one is due: the wake time is lowered to it before the work, so that a crash between
    /// the work and the refresh cannot leave a job that nothing wakes for.
    fn on_jobs<T>(
        &self,
        tenant: &TenantId,
        creates: Option<i64>,
        work: impl FnOnce(&mut Transaction<'_>) -> StoreResult<T>,
    ) -> StoreResult<Option<T>> {
        let Some(scope) = self.tenant_db(tenant)? else {
            return Ok(None);
        };
        if let Some(due) = creates {
            self.wake_at(tenant, due)?;
        }
        let done = scope.write(work)?;
        let _ = self.wake_refresh(tenant, &scope);
        Ok(Some(done))
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
            let Some(scope) = self.tenant_db(tenant)? else {
                return Ok((Vec::new(), false));
            };
            let mut parked: Vec<(u64, String)> = Vec::new();
            let found = scope.write(|t| {
                let mut out: Vec<OutboxEntry> = Vec::new();
                let mut cursor = after as i64;
                loop {
                    let want = room - out.len();
                    let want_rows: i64 = want as i64;
                    let rows = t
                        .query(
                            "SELECT position, data FROM events WHERE published = 0 AND position > $1 ORDER BY position LIMIT $2",
                            &[&cursor, &want_rows],
                        )
                        .map_err(db)?;
                    let fetched = rows.len();
                    for row in &rows {
                        let position: i64 = get(row, 0)?;
                        let data: String = get(row, 1)?;
                        cursor = position;
                        match decode::<DomainEvent>(&data, "event") {
                            Ok(event) => out.push(OutboxEntry {
                                position: position as u64,
                                event,
                            }),
                            Err(error) => {
                                t.execute(
                                    "UPDATE events SET published = $2 WHERE position = $1 AND published = 0",
                                    &[&position, &PARKED],
                                )
                                .map_err(db)?;
                                parked.push((position as u64, error.to_string()));
                            }
                        }
                    }
                    if out.len() >= room || fetched < want {
                        let capped = out.len() >= room;
                        return Ok((out, capped));
                    }
                }
            })?;
            for (position, reason) in parked {
                self.fault(tenant, Some(position), reason);
            }
            Ok(found)
        };
        match read() {
            Ok(found) => found,
            Err(error) => {
                self.fault(tenant, None, error.to_string());
                (Vec::new(), false)
            }
        }
    }

    /// Apply a commit in the transaction `t`, which holds the tenant's writer lock.
    fn apply(
        t: &mut Transaction<'_>,
        commit: &Commit,
        nonces: &dyn NonceSource,
    ) -> StoreResult<Receipt> {
        let delta = &commit.delta;
        let tenant = delta.tenant.as_str();
        if commit.expected_generation.is_none() && delta.base_revision.is_some() {
            return Err(StoreError::Invalid(
                "a commit with a base revision names the generation it was decided on".into(),
            ));
        }
        // The row lock of the tenant: the revision is compared under it, and the outbox
        // positions are handed out from its counter under it.
        let mut position: i64 = match t
            .query_opt(
                "SELECT outbox_position FROM tenant WHERE id = $1 FOR UPDATE",
                &[&tenant],
            )
            .map_err(db)?
        {
            Some(row) => get(&row, 0)?,
            None => 0,
        };
        let current = header(t, &delta.tenant)?;
        let found = current.as_ref().map(|h| h.revision);
        // A tenant that does not exist has no generation, whatever a leftover row says. The
        // generation is compared under the row lock taken above, in this transaction.
        let held = match &current {
            Some(_) => Some(read_generation(t)?.ok_or_else(|| {
                StoreError::Corrupt(format!("tenant {} has no generation", delta.tenant))
            })?),
            None => None,
        };
        if found != delta.base_revision || commit.expected_generation != held {
            return Err(StoreError::Conflict {
                expected: delta.base_revision,
                found,
                found_generation: held,
            });
        }
        let produced = match held {
            Some(held) => held.raised(),
            None => Generation::first(nonces.draw(&delta.tenant)),
        };
        check_event_sequence(commit, current.as_ref().map_or(0, |h| h.event_seq))?;
        // Defence in depth: a tenant's revision advances by exactly one, and its record counter
        // never falls.
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
        let new_revision: i64 = delta.new_revision.0 as i64;
        let next_record: i64 = delta.next_record as i64;
        let event_seq: i64 = delta.event_seq as i64;
        match (current.is_some(), created) {
            (false, Some(name)) => {
                let schema = schema
                    .clone()
                    .unwrap_or_else(|| Arc::new(TenantSchema::genesis()));
                t.execute(
                    "INSERT INTO tenant (id, name, revision, next_record, event_seq, schema, members)
                     VALUES ($1, $2, $3, $4, $5, $6, $7)",
                    &[
                        &tenant,
                        &name,
                        &new_revision,
                        &next_record,
                        &event_seq,
                        &encode(&*schema),
                        &encode(&members),
                    ],
                )
                .map_err(db)?;
            }
            (true, None) => {
                t.execute(
                    "UPDATE tenant SET revision = $1, next_record = $2, event_seq = $3 WHERE id = $4",
                    &[&new_revision, &next_record, &event_seq, &tenant],
                )
                .map_err(db)?;
                if let Some(schema) = &schema {
                    t.execute(
                        "UPDATE tenant SET schema = $1 WHERE id = $2",
                        &[&encode(&**schema), &tenant],
                    )
                    .map_err(db)?;
                }
                if members_changed {
                    t.execute(
                        "UPDATE tenant SET members = $1 WHERE id = $2",
                        &[&encode(&members), &tenant],
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
        // The search entries follow the schema in force after this commit: the entries of a
        // type whose searchable fields change are rebuilt before the commit's own records are
        // written under the new schema.
        let search_schema: Arc<TenantSchema> = match (&schema, &current) {
            (Some(next), Some(held)) => {
                search_index::reconcile(t, &held.schema, next)?;
                next.clone()
            }
            (Some(next), None) => next.clone(),
            (None, Some(held)) => held.schema.clone(),
            (None, None) => Arc::new(TenantSchema::genesis()),
        };
        if let Some(schema) = &schema {
            // The row of a version keeps the revision the version began at and holds the latest
            // schema at that version.
            let version: i64 = schema.version.0 as i64;
            t.execute(
                "INSERT INTO schema_history (version, revision, schema) VALUES ($1, $2, $3)
                 ON CONFLICT (version) DO UPDATE SET schema = EXCLUDED.schema",
                &[&version, &new_revision, &encode(&history_form(schema))],
            )
            .map_err(db)?;
            pair_index::sync(t, schema)?;
        }
        for op in &delta.ops {
            match op {
                DeltaOp::PutRecord(record) => {
                    write_record(t, record)?;
                    search_index::write_entries(t, &search_schema, record)?;
                }
                DeltaOp::DeleteRecord { object, id } => {
                    let row_id: i64 = id.0 as i64;
                    t.execute(
                        "DELETE FROM records WHERE object = $1 AND id = $2",
                        &[&object.as_str(), &row_id],
                    )
                    .map_err(db)?;
                    pair_index::delete_rows(t, object, id.0)?;
                }
                DeltaOp::PutSettingOverride(held) => write_setting_override(t, held)?,
                DeltaOp::DeleteSettingOverride {
                    setting,
                    level,
                    subject,
                } => {
                    t.execute(
                        "DELETE FROM setting_overrides WHERE setting = $1 AND level = $2 AND subject = $3",
                        &[&setting.as_str(), &level.as_str(), subject],
                    )
                    .map_err(db)?;
                }
                DeltaOp::PutSettingRevision { setting, revision } => {
                    write_setting_revision(t, setting, *revision)?
                }
                DeltaOp::Ledger(change) => ledger::apply(t, change, delta.new_revision)?,
                _ => {}
            }
        }
        for object in strata_storage_api::changed_objects(delta) {
            raise_object_revision(t, object.as_str())?;
        }
        // The generation is raised in the transaction that changes what a load returns, and is
        // in no hash and in no journalled form.
        write_generation(t, &produced)?;
        let mut last_position = None;
        for event in &commit.events {
            position += 1;
            let seq: i64 = event.seq as i64;
            let revision: i64 = event.revision.0 as i64;
            t.execute(
                "INSERT INTO events (position, seq, revision, data) VALUES ($1, $2, $3, $4)",
                &[&position, &seq, &revision, &encode(event)],
            )
            .map_err(db)?;
            last_position = Some(position as u64);
        }
        if !commit.events.is_empty() {
            t.execute(
                "UPDATE tenant SET outbox_position = $1 WHERE id = $2",
                &[&position, &tenant],
            )
            .map_err(db)?;
        }
        let journal_revision: i64 = commit.journal.revision.0 as i64;
        t.execute(
            "INSERT INTO journal (revision, data) VALUES ($1, $2)",
            &[&journal_revision, &encode(&commit.journal)],
        )
        .map_err(db)?;
        // A key is kept for the retention of its entry, counted from the commit that recorded it
        // (`expires_at`); the rows past it go in the same transaction as the next commit.
        t.execute(
            "DELETE FROM idempotency WHERE expires_at < $1",
            &[&commit.journal.context.now.0],
        )
        .map_err(db)?;
        if let Some(entry) = &commit.idempotency {
            let last: Option<i64> = last_position.map(|p| p as i64);
            let expires: i64 = entry.recorded_at.0.saturating_add(entry.retention_ms);
            t.execute(
                "INSERT INTO idempotency (principal, key, request_hash, recorded_at, revision, last_position, result, expires_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                &[
                    &entry.principal,
                    &entry.key,
                    &entry.request_hash,
                    &entry.recorded_at.0,
                    &new_revision,
                    &last,
                    &encode(&entry.result),
                    &expires,
                ],
            )
            .map_err(db)?;
        }
        insert_commit_work(t, commit)?;
        for principal in &commit.revoke_sessions {
            t.execute(
                "UPDATE sessions SET revoked_at = $1 WHERE principal = $2 AND revoked_at IS NULL",
                &[&commit.journal.context.now.0, &principal.as_str()],
            )
            .map_err(db)?;
        }
        Ok(Receipt {
            revision: delta.new_revision,
            last_position,
            generation: produced,
        })
    }
}

impl Store for PostgresStore {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            adapter: "postgres".into(),
            durable: self.durable,
            isolation: "READ COMMITTED; the writers of one tenant are serialised by a transaction-level advisory lock on the tenant's schema, and a commit locks the tenant's row (SELECT ... FOR UPDATE), compares the revision under that lock and assigns its outbox positions from the row's counter".into(),
            read_consistency: "a load runs in one REPEATABLE READ, READ ONLY transaction and sees only whole commits".into(),
            notes: vec![
                "field values longer than 240 bytes are not indexed; lookups for them scan the object type".into(),
                format!("control schema {CONTROL_SCHEMA} at {}", self.control_location),
                format!("placement: {}", self.placement.describe()),
                "outbox positions are per tenant; the outboxes of different tenants are not ordered against each other".into(),
                "durability is the control server's fsync and synchronous_commit settings, read at connect".into(),
            ],
        }
    }

    fn health(&self) -> StoreResult<Health> {
        let version: String = self.control.read(|t| {
            let row = t.query_one("SHOW server_version", &[]).map_err(db)?;
            get(&row, 0)
        })?;
        let mut ok = true;
        let mut detail = format!("control reachable, PostgreSQL {version}");
        let (mut tenants, mut pending) = (0u64, 0u64);
        for tenant in self.directory()? {
            let checked = (|| -> StoreResult<Option<(bool, u64)>> {
                let Some(scope) = self.tenant_db(&tenant)? else {
                    return Ok(None);
                };
                scope
                    .read(|t| {
                        let present = tenant_present(t, &tenant)?;
                        let waiting =
                            count(t, "SELECT COUNT(*) FROM events WHERE published = 0", &[])?;
                        Ok((present, waiting))
                    })
                    .map(Some)
            })();
            match checked {
                Ok(None) => {}
                Ok(Some((present, waiting))) => {
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
        self.control.pool.with(|client| {
            client.query_one("SELECT 1", &[]).map_err(db)?;
            Ok(())
        })
    }

    fn migrations(&self) -> StoreResult<Vec<StorageMigration>> {
        self.control.read(|t| {
            let rows = t
                .query(
                    "SELECT version, name, applied_at FROM control_migrations ORDER BY version",
                    &[],
                )
                .map_err(db)?;
            let mut out = Vec::with_capacity(rows.len());
            for row in &rows {
                let version: i64 = get(row, 0)?;
                let applied_at: i64 = get(row, 2)?;
                out.push(StorageMigration {
                    version: version as u32,
                    name: get(row, 1)?,
                    applied_at: Timestamp(applied_at),
                });
            }
            Ok(out)
        })
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

    fn tenant_plan(&self, tenant: &TenantId) -> StoreResult<Option<String>> {
        self.control.read(|t| {
            let found = select_strings(
                t,
                "SELECT plan FROM tenant_directory WHERE id = $1 AND state = 'active'",
                &[&tenant.as_str()],
            )?;
            Ok(found.into_iter().next())
        })
    }

    fn set_tenant_plan(&self, tenant: &TenantId, plan: &str) -> StoreResult<()> {
        let changed = self.control.write(|t| {
            t.execute(
                "UPDATE tenant_directory SET plan = $2 WHERE id = $1 AND state = 'active'",
                &[&tenant.as_str(), &plan],
            )
            .map_err(db)
        })?;
        if changed == 0 {
            return Err(StoreError::NotFound(format!("tenant {tenant}")));
        }
        Ok(())
    }

    /// The size of the tenant's schema: every table of it with its indexes and TOAST data.
    fn stored_bytes(&self, tenant: &TenantId) -> StoreResult<u64> {
        let Some(scope) = self.tenant_db(tenant)? else {
            return Ok(0);
        };
        let schema = scope.schema.clone();
        scope.read(|t| {
            count(
                t,
                "SELECT COALESCE(SUM(pg_total_relation_size(c.oid)), 0)::bigint
                   FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
                  WHERE n.nspname::text = $1::text AND c.relkind = 'r'",
                &[&schema],
            )
        })
    }

    fn tenant_summary(&self, tenant: &TenantId) -> StoreResult<Option<TenantSummary>> {
        Ok(self
            .peek(tenant, |t| {
                let Some(state) = header(t, tenant)? else {
                    return Ok(None);
                };
                let records = count(t, "SELECT COUNT(*) FROM records", &[])?;
                Ok(Some(TenantSummary {
                    id: tenant.clone(),
                    name: state.name,
                    revision: state.revision,
                    schema_version: state.schema.version.0,
                    event_seq: state.event_seq,
                    records,
                }))
            })?
            .flatten())
    }

    fn object_revisions(
        &self,
        tenant: &TenantId,
        objects: &[ObjectTypeId],
    ) -> StoreResult<BTreeMap<ObjectTypeId, u64>> {
        let counters = self
            .peek(tenant, |t| {
                let mut counters: BTreeMap<String, u64> = BTreeMap::new();
                for row in &t
                    .query("SELECT object, revision FROM object_revisions", &[])
                    .map_err(db)?
                {
                    let object: String = get(row, 0)?;
                    let revision: i64 = get(row, 1)?;
                    counters.insert(object, revision.max(0) as u64);
                }
                Ok(counters)
            })?
            .unwrap_or_default();
        let everything = counters.get("").copied().unwrap_or(0);
        Ok(objects
            .iter()
            .map(|object| {
                let own = counters.get(object.as_str()).copied().unwrap_or(0);
                (object.clone(), everything + own)
            })
            .collect())
    }

    fn load_at(&self, tenant: &TenantId, needs: &[Need]) -> StoreResult<Loaded> {
        Ok(self.load_at_held(tenant, needs, None)?.loaded)
    }

    fn load_at_held(
        &self,
        tenant: &TenantId,
        needs: &[Need],
        held: Option<&HeldHeader>,
    ) -> StoreResult<HeldLoad> {
        let Some(scope) = self.tenant_db(tenant)? else {
            return Ok(HeldLoad {
                loaded: Loaded {
                    state: State::absent(tenant.clone()),
                    generation: None,
                },
                header_decoded: false,
            });
        };
        scope.read(|t| {
            // The generation and the rows are read in this one REPEATABLE READ transaction.
            let generation = read_generation(t)?;
            let Some((mut state, header_decoded)) =
                header_for_load(t, tenant, generation, held)?
            else {
                return Ok(HeldLoad {
                    loaded: Loaded {
                        state: State::absent(tenant.clone()),
                        generation: None,
                    },
                    header_decoded: false,
                });
            };
            for need in needs {
                let rows = match need {
                    Need::Record { object, id } => {
                        let id: i64 = id.0 as i64;
                        select_strings(
                            t,
                            "SELECT data FROM records WHERE object = $1 AND id = $2",
                            &[&object.as_str(), &id],
                        )?
                    }
                    Need::AllOf(object) => all_of(t, object)?,
                    Need::PageOf {
                        object,
                        after,
                        limit,
                    } => {
                        let after: i64 = after.map_or(-1_i64, |a| a.0 as i64);
                        let limit: i64 = *limit as i64;
                        select_strings(
                            t,
                            "SELECT data FROM records WHERE object = $1 AND id > $2 ORDER BY id LIMIT $3",
                            &[&object.as_str(), &after, &limit],
                        )?
                    }
                    Need::ByField {
                        object,
                        field,
                        value,
                    } => {
                        let key = encode(value);
                        if key.len() <= MAX_INDEXED {
                            select_strings(
                                t,
                                "SELECT r.data FROM record_index i
                                 JOIN records r ON r.object = i.object AND r.id = i.id
                                 WHERE i.object = $1 AND i.field = $2 AND i.value = $3
                                 ORDER BY r.id",
                                &[&object.as_str(), &field.as_str(), &key],
                            )?
                        } else {
                            let mut matching = Vec::new();
                            for data in all_of(t, object)? {
                                let record: Record = decode(&data, "record")?;
                                if field_matches(&record, field, value) {
                                    matching.push(data);
                                }
                            }
                            matching
                        }
                    }
                    Need::ByFields { object, fields } => by_fields(t, object, fields)?,
                    Need::Ledger(want) => {
                        ledger::load(t, &mut state, want)?;
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
                        t,
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
                        t,
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
            Ok(HeldLoad {
                loaded: Loaded {
                    state: State {
                        tenant_id: tenant.clone(),
                        tenant: Some(Arc::new(state)),
                        coverage: Coverage::Partial(needs.iter().cloned().collect()),
                    },
                    generation,
                },
                header_decoded,
            })
        })
    }

    fn load_complete_at(&self, tenant: &TenantId) -> StoreResult<Loaded> {
        Ok(self.load_complete_at_held(tenant, None)?.loaded)
    }

    fn load_complete_at_held(
        &self,
        tenant: &TenantId,
        held: Option<&HeldHeader>,
    ) -> StoreResult<HeldLoad> {
        let Some(scope) = self.tenant_db(tenant)? else {
            return Ok(HeldLoad {
                loaded: Loaded {
                    state: State::absent(tenant.clone()),
                    generation: None,
                },
                header_decoded: false,
            });
        };
        scope.read(|t| {
            let generation = read_generation(t)?;
            Ok(match header_for_load(t, tenant, generation, held)? {
                Some((state, header_decoded)) => HeldLoad {
                    loaded: Loaded {
                        state: State::complete(Arc::new(complete_from(t, state)?)),
                        generation,
                    },
                    header_decoded,
                },
                None => HeldLoad {
                    loaded: Loaded {
                        state: State::absent(tenant.clone()),
                        generation: None,
                    },
                    header_decoded: false,
                },
            })
        })
    }

    fn generation(&self, tenant: &TenantId) -> StoreResult<Option<Generation>> {
        Ok(self.peek(tenant, read_generation)?.flatten())
    }

    fn rotate_nonce(&self, tenant: &TenantId) -> StoreResult<Generation> {
        let scope = self.existing(tenant)?;
        scope.write(|t| {
            let held = read_generation(t)?
                .ok_or_else(|| StoreError::Corrupt(format!("tenant {tenant} has no generation")))?;
            let next = held.rotated(self.nonces.draw(tenant));
            write_generation(t, &next)?;
            Ok(next)
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
            let Some(scope) = self.tenant_db(&delta.tenant)? else {
                if delta.base_revision.is_some() {
                    return Err(StoreError::Conflict {
                        expected: delta.base_revision,
                        found: None,
                        found_generation: None,
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
            let receipt = scope.write(|t| Self::apply(t, commit, &*self.nonces))?;
            if let Some(due) = wake {
                let _ = self.wake_at(&delta.tenant, due);
            }
            return Ok(receipt);
        }
        let (scope, created) = self.create_tenant_db(&delta.tenant, commit.journal.context.now)?;
        let result = scope.write(|t| Self::apply(t, commit, &*self.nonces));
        if let (Ok(_), Some(due)) = (&result, commit.jobs.iter().map(|job| job.due_at.0).min()) {
            let _ = self.wake_at(&delta.tenant, due);
        }
        if result.is_err() && created {
            // The schema this call made holds nothing: take it away again.
            let empty = !scope.read(|t| tenant_present(t, &delta.tenant))?;
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
        // A commit with a segment write is never the one that creates its tenant.
        let Some(scope) = self.tenant_db(&commit.delta.tenant)? else {
            return Err(StoreError::Conflict {
                expected: commit.delta.base_revision,
                found: None,
                found_generation: None,
            });
        };
        let wake = commit.jobs.iter().map(|job| job.due_at.0).min();
        if let Some(due) = wake {
            self.wake_at(&commit.delta.tenant, due)?;
        }
        let receipt = scope.write(|t| {
            let receipt = Self::apply(t, commit, &*self.nonces)?;
            apply_segments(t, write)?;
            Ok(receipt)
        })?;
        if let Some(due) = wake {
            let _ = self.wake_at(&commit.delta.tenant, due);
        }
        Ok(receipt)
    }

    fn commit_segments(&self, commit: &SegmentCommit) -> StoreResult<Receipt> {
        let Some(scope) = self.tenant_db(&commit.tenant)? else {
            return Err(StoreError::Conflict {
                expected: Some(commit.revision),
                found: None,
                found_generation: None,
            });
        };
        check_jobs(&commit.work.jobs)?;
        let wake = commit.work.jobs.iter().map(|job| job.due_at.0).min();
        if let Some(due) = wake {
            self.wake_at(&commit.tenant, due)?;
        }
        let receipt = scope.write(|t| {
            // The tenant's row is locked for the comparison, as in a commit.
            let found: Option<StateRevision> = match t
                .query_opt(
                    "SELECT revision FROM tenant WHERE id = $1 FOR UPDATE",
                    &[&commit.tenant.as_str()],
                )
                .map_err(db)?
            {
                Some(row) => {
                    let revision: i64 = get(&row, 0)?;
                    Some(StateRevision(revision as u64))
                }
                None => None,
            };
            let held = read_generation(t)?;
            if found != Some(commit.revision) {
                return Err(StoreError::Conflict {
                    expected: Some(commit.revision),
                    found,
                    found_generation: held,
                });
            }
            apply_segments(t, &commit.write)?;
            write_automation(t, &commit.tenant, &commit.work, commit.now)?;
            // No record changed: the generation is not raised.
            Ok(Receipt {
                revision: commit.revision,
                last_position: None,
                generation: held.ok_or_else(|| {
                    StoreError::Corrupt(format!("tenant {} has no generation", commit.tenant))
                })?,
            })
        })?;
        if let Some(due) = wake {
            let _ = self.wake_at(&commit.tenant, due);
        }
        Ok(receipt)
    }

    fn segment_cursor(&self, tenant: &TenantId) -> StoreResult<u64> {
        Ok(self.peek(tenant, segment_cursor)?.unwrap_or(0))
    }

    fn search_candidates(
        &self,
        tenant: &TenantId,
        probe: &strata_kernel::search::Probe,
    ) -> StoreResult<Vec<strata_kernel::search::Candidate>> {
        Ok(self
            .peek(tenant, |t| search_index::candidates(t, probe))?
            .unwrap_or_default())
    }

    fn segment_catalog(&self, tenant: &TenantId) -> StoreResult<Vec<seg::Segment>> {
        Ok(self.peek(tenant, catalog_rows)?.unwrap_or_default())
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
        let Some(scope) = self.tenant_db(tenant)? else {
            return Ok(rows);
        };
        scope.read(|t| {
            for (segment, person) in pairs {
                let key = (*segment, person.clone());
                if let Some(m) = membership_of(t, *segment, person)? {
                    rows.memberships.insert(key.clone(), m);
                }
                if let Some(at) = wake_of(t, *segment, person)? {
                    rows.wakes.insert(key, at);
                }
                let row = t
                    .query_one(
                        "SELECT EXISTS (SELECT 1 FROM segment_erased WHERE principal = $1)",
                        &[&person.as_str()],
                    )
                    .map_err(db)?;
                let erased: bool = get(&row, 0)?;
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
        let limit: i64 = limit as i64;
        Ok(self
            .peek(tenant, |t| {
                memberships_where(
                    t,
                    "SELECT segment, principal, version, since FROM segment_memberships
                     WHERE segment = $1 AND principal > $2 ORDER BY principal LIMIT $3",
                    &[&segment.to_string(), &after, &limit],
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
            .peek(tenant, |t| {
                memberships_where(
                    t,
                    "SELECT segment, principal, version, since FROM segment_memberships
                     WHERE principal = $1 ORDER BY segment",
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
            .peek(tenant, |t| {
                wakes_where(
                    t,
                    "SELECT segment, principal, wake_at FROM segment_wakes WHERE principal = $1 ORDER BY segment",
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
        let limit: i64 = limit as i64;
        Ok(self
            .peek(tenant, |t| {
                wakes_where(
                    t,
                    "SELECT segment, principal, wake_at FROM segment_wakes
                     WHERE segment = $1 AND wake_at <= $2 ORDER BY wake_at, principal LIMIT $3",
                    &[&segment.to_string(), &at.0, &limit],
                )
            })?
            .unwrap_or_default())
    }

    fn segment_batches(&self, tenant: &TenantId) -> StoreResult<Vec<seg::Batch>> {
        Ok(self
            .peek(tenant, |t| {
                select_strings(
                    t,
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
        let after: i64 = after.map_or(-1_i64, |a| a.0 as i64);
        let limit: i64 = limit as i64;
        let rows = self
            .peek(tenant, |t| {
                select_strings(
                    t,
                    "SELECT r.data FROM record_index i
                     JOIN records r ON r.object = i.object AND r.id = i.id
                     WHERE i.object = $1 AND i.field = $2 AND i.value = $3 AND i.id > $4
                     ORDER BY i.id LIMIT $5",
                    &[&object.as_str(), &field.as_str(), &key, &after, &limit],
                )
            })?
            .unwrap_or_default();
        rows.iter()
            .map(|data| decode::<Record>(data, "record").map(Arc::new))
            .collect()
    }

    fn segment_subject_count(&self, tenant: &TenantId, object: &ObjectTypeId) -> StoreResult<u64> {
        Ok(self
            .peek(tenant, |t| {
                count(
                    t,
                    "SELECT COUNT(*) FROM records WHERE object = $1",
                    &[&object.as_str()],
                )
            })?
            .unwrap_or(0))
    }

    fn segment_events_after(
        &self,
        tenant: &TenantId,
        after: u64,
        limit: usize,
    ) -> StoreResult<Vec<(u64, seg::SegmentEvent)>> {
        let after: i64 = after as i64;
        let limit: i64 = limit as i64;
        let rows = self
            .peek(tenant, |t| seq_rows(
                t,
                "SELECT position, data FROM segment_events WHERE position > $1 ORDER BY position LIMIT $2",
                &[&after, &limit],
            ))?
            .unwrap_or_default();
        rows.iter()
            .map(|(position, data)| {
                decode::<seg::SegmentEvent>(data, "segment event").map(|e| (*position, e))
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
        let found = self.peek(tenant, |t| {
            let Some(row) = t
                .query_opt(
                    "SELECT request_hash, revision, last_position, result FROM idempotency
                     WHERE principal = $1 AND key = $2 AND expires_at >= $3",
                    &[&principal, &key, &now.0],
                )
                .map_err(db)?
            else {
                return Ok(None);
            };
            let request_hash: String = get(&row, 0)?;
            let revision: i64 = get(&row, 1)?;
            let position: Option<i64> = get(&row, 2)?;
            let result: String = get(&row, 3)?;
            // The generation the tenant holds now: what the first commit produced is not kept.
            let generation = read_generation(t)?
                .ok_or_else(|| StoreError::Corrupt(format!("tenant {tenant} has no generation")))?;
            Ok(Some(IdempotentResult {
                request_hash,
                result: decode(&result, "idempotency result")?,
                receipt: Receipt {
                    revision: StateRevision(revision as u64),
                    last_position: position.map(|p| p as u64),
                    generation,
                },
            }))
        })?;
        Ok(found.flatten())
    }

    fn events_after(
        &self,
        tenant: &TenantId,
        after: u64,
        limit: usize,
    ) -> StoreResult<Vec<DomainEvent>> {
        let after: i64 = after as i64;
        let limit: i64 = limit as i64;
        self.peek(tenant, |t| {
            select_strings(
                t,
                "SELECT data FROM events WHERE seq > $1 ORDER BY seq LIMIT $2",
                &[&after, &limit],
            )
        })?
        .unwrap_or_default()
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
            .peek(tenant, |t| {
                count(t, "SELECT COUNT(*) FROM events WHERE published = 0", &[])
            })?
            .unwrap_or(0))
    }

    fn mark_published(&self, tenant: &TenantId, position: u64) -> StoreResult<()> {
        let position: i64 = position.min(i64::MAX as u64) as i64;
        self.on_tenant(tenant, |t| {
            t.execute(
                "UPDATE events SET published = 1 WHERE published = 0 AND position <= $1",
                &[&position],
            )
            .map_err(db)?;
            Ok(())
        })?;
        Ok(())
    }

    fn journal_after(
        &self,
        tenant: &TenantId,
        after: u64,
        limit: usize,
    ) -> StoreResult<Vec<JournalEntry>> {
        let after: i64 = after as i64;
        let limit: i64 = limit as i64;
        self.peek(tenant, |t| {
            select_strings(
                t,
                "SELECT data FROM journal WHERE revision > $1 ORDER BY revision LIMIT $2",
                &[&after, &limit],
            )
        })?
        .unwrap_or_default()
        .iter()
        .map(|data| decode(data, "journal entry"))
        .collect()
    }

    fn journal_count(&self, tenant: &TenantId) -> StoreResult<u64> {
        Ok(self
            .peek(tenant, |t| count(t, "SELECT COUNT(*) FROM journal", &[]))?
            .unwrap_or(0))
    }

    fn put_session(&self, session: &SessionRecord) -> StoreResult<()> {
        self.existing(&session.tenant)?;
        // The directory keeps the tenant a session was first stored under, and its token hash
        // unique across tenants.
        self.control.write(|t| {
            t.execute(
                "INSERT INTO session_directory (id, token_hash, tenant) VALUES ($1, $2, $3)
                 ON CONFLICT (id) DO UPDATE SET token_hash = EXCLUDED.token_hash",
                &[
                    &session.id.as_str(),
                    &session.token_hash,
                    &session.tenant.as_str(),
                ],
            )
            .map_err(db)?;
            Ok(())
        })?;
        let tenant = self
            .session_tenant("id", session.id.as_str())?
            .ok_or_else(|| StoreError::Unavailable("the session directory lost a row".into()))?;
        let revoked: Option<i64> = session.revoked_at.map(|t| t.0);
        self.on_existing(&tenant, |t| {
            t.execute(
                &*format!(
                    "INSERT INTO sessions ({SESSION_COLUMNS}) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                     ON CONFLICT (id) DO UPDATE SET token_hash = EXCLUDED.token_hash, csrf_token = EXCLUDED.csrf_token,
                        display_name = EXCLUDED.display_name, last_seen_at = EXCLUDED.last_seen_at,
                        expires_at = EXCLUDED.expires_at, revoked_at = EXCLUDED.revoked_at,
                        authenticated_at = EXCLUDED.authenticated_at, network = EXCLUDED.network"
                ),
                &[
                    &session.id.as_str(),
                    &session.token_hash,
                    &session.csrf_token,
                    &session.principal.as_str(),
                    &session.display_name,
                    &session.created_at.0,
                    &session.last_seen_at.0,
                    &session.expires_at.0,
                    &revoked,
                    &session.authenticated_at.0,
                    &session.network,
                ],
            )
            .map_err(db)?;
            Ok(())
        })
    }

    fn session_seen(&self, id: &SessionId, at: Timestamp) -> StoreResult<()> {
        let Some(tenant) = self.session_tenant("id", id.as_str())? else {
            return Ok(());
        };
        self.on_tenant(&tenant, |t| {
            t.execute(
                "UPDATE sessions SET last_seen_at = $2 WHERE id = $1",
                &[&id.as_str(), &at.0],
            )
            .map_err(db)?;
            Ok(())
        })?;
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
        self.on_tenant(&tenant, |t| {
            t.execute(
                "UPDATE sessions SET authenticated_at = $2, network = $3 WHERE id = $1",
                &[&id.as_str(), &at.0, &network],
            )
            .map_err(db)?;
            Ok(())
        })?;
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
            .peek(tenant, |t| match principal {
                Some(principal) => sessions_where(
                    t,
                    tenant,
                    "revoked_at IS NULL AND expires_at > $1 AND principal = $2",
                    &[&now.0, &principal.as_str()],
                ),
                None => sessions_where(
                    t,
                    tenant,
                    "revoked_at IS NULL AND expires_at > $1",
                    &[&now.0],
                ),
            })?
            .unwrap_or_default())
    }

    fn purge_sessions(&self, before: Timestamp) -> StoreResult<u64> {
        let mut removed: Vec<String> = Vec::new();
        for tenant in self.directory()? {
            let Some(scope) = self.tenant_db(&tenant)? else {
                continue;
            };
            let ids = scope.write(|t| {
                select_strings(
                    t,
                    "DELETE FROM sessions WHERE expires_at < $1 OR (revoked_at IS NOT NULL AND revoked_at < $1)
                     RETURNING id",
                    &[&before.0],
                )
            })?;
            removed.extend(ids);
        }
        self.control.write(|t| {
            for id in &removed {
                t.execute("DELETE FROM session_directory WHERE id = $1", &[id])
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
        self.on_existing(tenant, |t| {
            let mut next = Vec::with_capacity(rules.len());
            let mut until: Option<i64> = None;
            for rule in rules {
                let row = throttle_row(t, &rule.key)?;
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
                let failures: i64 = i64::from(row.failures);
                t.execute(
                    "INSERT INTO login_throttle (key, failures, window_start, locked_until, expires_at)
                     VALUES ($1, $2, $3, $4, $5)
                     ON CONFLICT (key) DO UPDATE SET failures = EXCLUDED.failures,
                        window_start = EXCLUDED.window_start, locked_until = EXCLUDED.locked_until,
                        expires_at = EXCLUDED.expires_at",
                    &[
                        &key,
                        &failures,
                        &row.window_start,
                        &row.locked_until,
                        &row.expires_at,
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
        self.on_existing(tenant, |t| {
            match how {
                ThrottleSettle::Clear => {
                    t.execute("DELETE FROM login_throttle WHERE key = $1", &[&key])
                        .map_err(db)?;
                }
                ThrottleSettle::Refund => {
                    if let Some(row) = throttle_row(t, key)? {
                        let row = throttle_refund(row, max);
                        let failures: i64 = i64::from(row.failures);
                        t.execute(
                            "UPDATE login_throttle SET failures = $2, locked_until = $3 WHERE key = $1",
                            &[&key, &failures, &row.locked_until],
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
            "DELETE FROM login_throttle WHERE expires_at <= $1",
            &[&now.0],
        )
    }

    fn account_of(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<Option<Account>> {
        let found = self.peek(tenant, |t| {
            let Some(row) = t
                .query_opt(
                    "SELECT login, principal, display_name, password_hash, verified_email FROM accounts
                     WHERE principal = $1 ORDER BY login LIMIT 1",
                    &[&principal.as_str()],
                )
                .map_err(db)?
            else {
                return Ok(None);
            };
            let login: String = get(&row, 0)?;
            account_from(tenant, login, &row, 1).map(Some)
        })?;
        Ok(found.flatten())
    }

    fn verified_address(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<Option<String>> {
        // The same account as `account_of`: the principal's first login.
        let address = self.peek(tenant, |t| {
            let Some(row) = t
                .query_opt(
                    "SELECT verified_email FROM accounts WHERE principal = $1 ORDER BY login LIMIT 1",
                    &[&principal.as_str()],
                )
                .map_err(db)?
            else {
                return Ok(None);
            };
            let email: Option<String> = get(&row, 0)?;
            Ok(email)
        })?;
        Ok(address.flatten())
    }

    fn put_passkey(&self, passkey: &PasskeyRecord) -> StoreResult<bool> {
        let last_used: Option<i64> = passkey.last_used_at.map(|at| at.0);
        let added = self.on_existing(&passkey.tenant, |t| {
            t.execute(
                &*format!(
                    "INSERT INTO passkeys ({PASSKEY_COLUMNS}) VALUES ($1, $2, $3, $4, $5, $6, $7)
                     ON CONFLICT DO NOTHING"
                ),
                &[
                    &passkey.id,
                    &passkey.principal.as_str(),
                    &passkey.rp_id,
                    &passkey.label,
                    &passkey.credential,
                    &passkey.created_at.0,
                    &last_used,
                ],
            )
            .map_err(db)
        })?;
        Ok(added == 1)
    }

    fn passkey(&self, tenant: &TenantId, id: &str) -> StoreResult<Option<PasskeyRecord>> {
        let found = self.peek(tenant, |t| {
            match t
                .query_opt(
                    &*format!("SELECT {PASSKEY_COLUMNS} FROM passkeys WHERE id = $1"),
                    &[&id],
                )
                .map_err(db)?
            {
                Some(row) => passkey_record(tenant, &row).map(Some),
                None => Ok(None),
            }
        })?;
        Ok(found.flatten())
    }

    fn passkeys(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<Vec<PasskeyRecord>> {
        Ok(self
            .peek(tenant, |t| {
                let rows = t
                    .query(
                        &*format!(
                            "SELECT {PASSKEY_COLUMNS} FROM passkeys WHERE principal = $1 ORDER BY created_at, id"
                        ),
                        &[&principal.as_str()],
                    )
                    .map_err(db)?;
                rows.iter().map(|row| passkey_record(tenant, row)).collect()
            })?
            .unwrap_or_default())
    }

    fn update_passkey(
        &self,
        tenant: &TenantId,
        id: &str,
        credential: &str,
        used_at: Timestamp,
    ) -> StoreResult<bool> {
        let changed = self.on_tenant(tenant, |t| {
            t.execute(
                "UPDATE passkeys SET credential = $2, last_used_at = $3 WHERE id = $1",
                &[&id, &credential, &used_at.0],
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
        let removed = self.on_tenant(tenant, |t| {
            t.execute(
                "DELETE FROM passkeys WHERE principal = $1 AND id = $2",
                &[&principal.as_str(), &id],
            )
            .map_err(db)
        })?;
        Ok(removed == Some(1))
    }

    fn put_passkey_challenge(&self, challenge: &PasskeyChallenge) -> StoreResult<()> {
        let ceremony = match challenge.ceremony {
            PasskeyCeremony::Register => "register",
            PasskeyCeremony::SignIn => "sign-in",
        };
        let session: Option<String> = challenge.session.as_ref().map(|s| s.as_str().to_string());
        let principal: Option<String> =
            challenge.principal.as_ref().map(|p| p.as_str().to_string());
        self.on_existing(&challenge.tenant, |t| {
            t.execute(
                "INSERT INTO passkey_challenges (id_hash, ceremony, session, principal, origin, state, expires_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
                 ON CONFLICT (id_hash) DO UPDATE SET ceremony = EXCLUDED.ceremony, session = EXCLUDED.session,
                    principal = EXCLUDED.principal, origin = EXCLUDED.origin, state = EXCLUDED.state,
                    expires_at = EXCLUDED.expires_at",
                &[
                    &challenge.id_hash,
                    &ceremony,
                    &session,
                    &principal,
                    &challenge.origin,
                    &challenge.state,
                    &challenge.expires_at.0,
                ],
            )
            .map_err(db)?;
            Ok(())
        })
    }

    fn take_passkey_challenge(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        now: Timestamp,
    ) -> StoreResult<Option<PasskeyChallenge>> {
        // One statement reads and removes the row, so two answers to one challenge cannot both
        // take it.
        let found = self.on_tenant(tenant, |t| {
            let Some(row) = t
                .query_opt(
                    "DELETE FROM passkey_challenges WHERE id_hash = $1
                     RETURNING ceremony, session, principal, origin, state, expires_at",
                    &[&id_hash],
                )
                .map_err(db)?
            else {
                return Ok(None);
            };
            let ceremony: String = get(&row, 0)?;
            let session: Option<String> = get(&row, 1)?;
            let principal: Option<String> = get(&row, 2)?;
            let origin: String = get(&row, 3)?;
            let state: String = get(&row, 4)?;
            let expires_at: i64 = get(&row, 5)?;
            Ok(Some((
                ceremony, session, principal, origin, state, expires_at,
            )))
        })?;
        let Some((ceremony, session, principal, origin, state, expires_at)) = found.flatten()
        else {
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
            "DELETE FROM passkey_challenges WHERE expires_at <= $1",
            &[&now.0],
        )
    }

    fn put_sign_in_link(&self, link: &SignInLink) -> StoreResult<()> {
        self.on_existing(&link.tenant, |t| {
            t.execute(
                "INSERT INTO sign_in_links (id_hash, principal, token_hash, binding_hash, origin, expires_at)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (id_hash) DO UPDATE SET principal = EXCLUDED.principal,
                    token_hash = EXCLUDED.token_hash, binding_hash = EXCLUDED.binding_hash,
                    origin = EXCLUDED.origin, expires_at = EXCLUDED.expires_at",
                &[
                    &link.id_hash,
                    &link.principal.as_str(),
                    &link.token_hash,
                    &link.binding_hash,
                    &link.origin,
                    &link.expires_at.0,
                ],
            )
            .map_err(db)?;
            Ok(())
        })
    }

    fn take_sign_in_link(
        &self,
        tenant: &TenantId,
        id_hash: &str,
    ) -> StoreResult<Option<SignInLink>> {
        // One statement reads and removes the row, so two uses of one link cannot both take it.
        let found = self.on_tenant(tenant, |t| {
            let Some(row) = t
                .query_opt(
                    "DELETE FROM sign_in_links WHERE id_hash = $1
                     RETURNING principal, token_hash, binding_hash, origin, expires_at",
                    &[&id_hash],
                )
                .map_err(db)?
            else {
                return Ok(None);
            };
            let principal: String = get(&row, 0)?;
            let expires_at: i64 = get(&row, 4)?;
            Ok(Some(SignInLink {
                tenant: tenant.clone(),
                id_hash: id_hash.to_string(),
                principal: PrincipalId::new(principal).map_err(corrupt_id)?,
                token_hash: get(&row, 1)?,
                binding_hash: get(&row, 2)?,
                origin: get(&row, 3)?,
                expires_at: Timestamp(expires_at),
            }))
        })?;
        Ok(found.flatten())
    }

    fn purge_sign_in_links(&self, now: Timestamp) -> StoreResult<u64> {
        self.on_every_tenant("DELETE FROM sign_in_links WHERE expires_at < $1", &[&now.0])
    }

    fn put_oidc_flow(&self, flow: &OidcFlow) -> StoreResult<()> {
        let principal: Option<&str> = flow.principal.as_ref().map(|p| p.as_str());
        self.on_existing(&flow.tenant, |t| {
            t.execute(
                "INSERT INTO oidc_flows (id_hash, provider, nonce, verifier, binding_hash, origin, principal, expires_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                 ON CONFLICT (id_hash) DO UPDATE SET provider = EXCLUDED.provider, nonce = EXCLUDED.nonce,
                    verifier = EXCLUDED.verifier, binding_hash = EXCLUDED.binding_hash,
                    origin = EXCLUDED.origin, principal = EXCLUDED.principal, expires_at = EXCLUDED.expires_at",
                &[
                    &flow.id_hash,
                    &flow.provider,
                    &flow.nonce,
                    &flow.verifier,
                    &flow.binding_hash,
                    &flow.origin,
                    &principal,
                    &flow.expires_at.0,
                ],
            )
            .map_err(db)?;
            Ok(())
        })
    }

    fn take_oidc_flow(&self, tenant: &TenantId, id_hash: &str) -> StoreResult<Option<OidcFlow>> {
        // One statement reads and removes the row, so two callbacks cannot both take it.
        let found = self.on_tenant(tenant, |t| {
            let Some(row) = t
                .query_opt(
                    "DELETE FROM oidc_flows WHERE id_hash = $1
                     RETURNING provider, nonce, verifier, binding_hash, origin, principal, expires_at",
                    &[&id_hash],
                )
                .map_err(db)?
            else {
                return Ok(None);
            };
            let principal: Option<String> = get(&row, 5)?;
            let expires_at: i64 = get(&row, 6)?;
            Ok(Some(OidcFlow {
                tenant: tenant.clone(),
                id_hash: id_hash.to_string(),
                provider: get(&row, 0)?,
                nonce: get(&row, 1)?,
                verifier: get(&row, 2)?,
                binding_hash: get(&row, 3)?,
                origin: get(&row, 4)?,
                principal: principal
                    .map(PrincipalId::new)
                    .transpose()
                    .map_err(corrupt_id)?,
                expires_at: Timestamp(expires_at),
            }))
        })?;
        Ok(found.flatten())
    }

    fn purge_oidc_flows(&self, now: Timestamp) -> StoreResult<u64> {
        self.on_every_tenant("DELETE FROM oidc_flows WHERE expires_at < $1", &[&now.0])
    }

    fn put_saml_request(&self, request: &SamlRequest) -> StoreResult<()> {
        let principal: Option<&str> = request.principal.as_ref().map(|p| p.as_str());
        self.on_existing(&request.tenant, |t| {
            t.execute(
                "INSERT INTO saml_requests (id_hash, provider, request_id, binding_hash, origin, principal, expires_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
                 ON CONFLICT (id_hash) DO UPDATE SET provider = EXCLUDED.provider,
                    request_id = EXCLUDED.request_id, binding_hash = EXCLUDED.binding_hash,
                    origin = EXCLUDED.origin, principal = EXCLUDED.principal, expires_at = EXCLUDED.expires_at",
                &[
                    &request.id_hash,
                    &request.provider,
                    &request.request_id,
                    &request.binding_hash,
                    &request.origin,
                    &principal,
                    &request.expires_at.0,
                ],
            )
            .map_err(db)?;
            Ok(())
        })
    }

    fn take_saml_request(
        &self,
        tenant: &TenantId,
        id_hash: &str,
    ) -> StoreResult<Option<SamlRequest>> {
        // One statement reads and removes the row, so two responses cannot both take it.
        let found = self.on_tenant(tenant, |t| {
            let Some(row) = t
                .query_opt(
                    "DELETE FROM saml_requests WHERE id_hash = $1
                     RETURNING provider, request_id, binding_hash, origin, principal, expires_at",
                    &[&id_hash],
                )
                .map_err(db)?
            else {
                return Ok(None);
            };
            let principal: Option<String> = get(&row, 4)?;
            let expires_at: i64 = get(&row, 5)?;
            Ok(Some(SamlRequest {
                tenant: tenant.clone(),
                id_hash: id_hash.to_string(),
                provider: get(&row, 0)?,
                request_id: get(&row, 1)?,
                binding_hash: get(&row, 2)?,
                origin: get(&row, 3)?,
                principal: principal
                    .map(PrincipalId::new)
                    .transpose()
                    .map_err(corrupt_id)?,
                expires_at: Timestamp(expires_at),
            }))
        })?;
        Ok(found.flatten())
    }

    fn purge_saml_requests(&self, now: Timestamp) -> StoreResult<u64> {
        self.on_every_tenant("DELETE FROM saml_requests WHERE expires_at < $1", &[&now.0])
    }

    fn put_saml_assertion(
        &self,
        tenant: &TenantId,
        provider: &str,
        id_hash: &str,
        expires_at: Timestamp,
    ) -> StoreResult<bool> {
        // One statement, so that two presentations at once are one: the second changes no row.
        let added = self.on_existing(tenant, |t| {
            t.execute(
                "INSERT INTO saml_assertions (provider, id_hash, expires_at)
                 VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
                &[&provider, &id_hash, &expires_at.0],
            )
            .map_err(db)
        })?;
        Ok(added == 1)
    }

    fn purge_saml_assertions(&self, now: Timestamp) -> StoreResult<u64> {
        self.on_every_tenant(
            "DELETE FROM saml_assertions WHERE expires_at < $1",
            &[&now.0],
        )
    }

    fn put_identity(&self, identity: &FederatedIdentity) -> StoreResult<bool> {
        let added = self.on_existing(&identity.tenant, |t| {
            t.execute(
                "INSERT INTO federated_identities (provider, subject, principal, created_at)
                 VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
                &[
                    &identity.provider,
                    &identity.subject,
                    &identity.principal.as_str(),
                    &identity.created_at.0,
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
        let found = self.peek(tenant, |t| {
            let Some(row) = t
                .query_opt(
                    "SELECT principal, created_at FROM federated_identities
                     WHERE provider = $1 AND subject = $2",
                    &[&provider, &subject],
                )
                .map_err(db)?
            else {
                return Ok(None);
            };
            let principal: String = get(&row, 0)?;
            let created_at: i64 = get(&row, 1)?;
            Ok(Some(FederatedIdentity {
                tenant: tenant.clone(),
                provider: provider.to_string(),
                subject: subject.to_string(),
                principal: PrincipalId::new(principal).map_err(corrupt_id)?,
                created_at: Timestamp(created_at),
            }))
        })?;
        Ok(found.flatten())
    }

    fn put_totp(&self, totp: &TotpRecord) -> StoreResult<bool> {
        // An unconfirmed enrolment is replaced; a confirmed one is not.
        let confirmed: i64 = i64::from(totp.confirmed);
        let changed = self.on_existing(&totp.tenant, |t| {
            t.execute(
                "INSERT INTO totp_factors (principal, secret, confirmed, last_step, created_at)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (principal) DO UPDATE SET secret = EXCLUDED.secret,
                    confirmed = EXCLUDED.confirmed, last_step = EXCLUDED.last_step,
                    created_at = EXCLUDED.created_at
                 WHERE totp_factors.confirmed = 0",
                &[
                    &totp.principal.as_str(),
                    &totp.secret,
                    &confirmed,
                    &totp.last_step,
                    &totp.created_at.0,
                ],
            )
            .map_err(db)
        })?;
        Ok(changed == 1)
    }

    fn totp(&self, tenant: &TenantId, principal: &PrincipalId) -> StoreResult<Option<TotpRecord>> {
        let found = self.peek(tenant, |t| {
            let Some(row) = t
                .query_opt(
                    "SELECT secret, confirmed, last_step, created_at FROM totp_factors WHERE principal = $1",
                    &[&principal.as_str()],
                )
                .map_err(db)?
            else {
                return Ok(None);
            };
            let confirmed: i64 = get(&row, 1)?;
            let created_at: i64 = get(&row, 3)?;
            Ok(Some(TotpRecord {
                tenant: tenant.clone(),
                principal: principal.clone(),
                secret: get(&row, 0)?,
                confirmed: confirmed != 0,
                last_step: get(&row, 2)?,
                created_at: Timestamp(created_at),
            }))
        })?;
        Ok(found.flatten())
    }

    fn confirm_totp(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        step: i64,
    ) -> StoreResult<bool> {
        let changed = self.on_tenant(tenant, |t| {
            t.execute(
                "UPDATE totp_factors SET confirmed = 1, last_step = $2
                 WHERE principal = $1 AND confirmed = 0",
                &[&principal.as_str(), &step],
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
        let changed = self.on_tenant(tenant, |t| {
            t.execute(
                "UPDATE totp_factors SET last_step = $2
                 WHERE principal = $1 AND confirmed = 1 AND last_step < $2",
                &[&principal.as_str(), &step],
            )
            .map_err(db)
        })?;
        Ok(changed == Some(1))
    }

    fn remove_totp(&self, tenant: &TenantId, principal: &PrincipalId) -> StoreResult<bool> {
        let removed = self.on_tenant(tenant, |t| {
            t.execute(
                "DELETE FROM totp_factors WHERE principal = $1",
                &[&principal.as_str()],
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
        self.on_existing(tenant, |t| {
            t.execute(
                "DELETE FROM recovery_codes WHERE principal = $1",
                &[&principal.as_str()],
            )
            .map_err(db)?;
            for hash in hashes {
                t.execute(
                    "INSERT INTO recovery_codes (principal, code_hash, created_at)
                     VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
                    &[&principal.as_str(), hash, &at.0],
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
        let removed = self.on_tenant(tenant, |t| {
            t.execute(
                "DELETE FROM recovery_codes WHERE principal = $1 AND code_hash = $2",
                &[&principal.as_str(), &hash],
            )
            .map_err(db)
        })?;
        Ok(removed == Some(1))
    }

    fn recovery_codes_left(&self, tenant: &TenantId, principal: &PrincipalId) -> StoreResult<u32> {
        let left = self.peek(tenant, |t| {
            count(
                t,
                "SELECT COUNT(*) FROM recovery_codes WHERE principal = $1",
                &[&principal.as_str()],
            )
        })?;
        Ok(left.unwrap_or(0) as u32)
    }

    fn put_pending(&self, pending: &PendingSignIn) -> StoreResult<()> {
        let factors: i64 = i64::from(pending.factors);
        let attempts: i64 = i64::from(pending.attempts);
        self.on_existing(&pending.tenant, |t| {
            t.execute(
                "INSERT INTO pending_sign_ins (id_hash, principal, method, factors, attempts, expires_at)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (id_hash) DO UPDATE SET principal = EXCLUDED.principal,
                    method = EXCLUDED.method, factors = EXCLUDED.factors,
                    attempts = EXCLUDED.attempts, expires_at = EXCLUDED.expires_at",
                &[
                    &pending.id_hash,
                    &pending.principal.as_str(),
                    &pending.method,
                    &factors,
                    &attempts,
                    &pending.expires_at.0,
                ],
            )
            .map_err(db)?;
            Ok(())
        })
    }

    fn attempt_pending(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        now: Timestamp,
        max: u32,
    ) -> StoreResult<Option<PendingSignIn>> {
        let found = self.on_tenant(tenant, |t| {
            let Some(row) = t
                .query_opt(
                    &*format!(
                        "SELECT {PENDING_COLUMNS} FROM pending_sign_ins WHERE id_hash = $1 FOR UPDATE"
                    ),
                    &[&id_hash],
                )
                .map_err(db)?
            else {
                return Ok(None);
            };
            let held = pending_sign_in(tenant, id_hash, &row)?;
            if held.expires_at.0 <= now.0 || held.attempts >= max {
                t.execute(
                    "DELETE FROM pending_sign_ins WHERE id_hash = $1",
                    &[&id_hash],
                )
                .map_err(db)?;
                return Ok(None);
            }
            t.execute(
                "UPDATE pending_sign_ins SET attempts = attempts + 1 WHERE id_hash = $1",
                &[&id_hash],
            )
            .map_err(db)?;
            Ok(Some(PendingSignIn {
                attempts: held.attempts + 1,
                ..held
            }))
        })?;
        Ok(found.flatten())
    }

    fn take_pending(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        now: Timestamp,
    ) -> StoreResult<Option<PendingSignIn>> {
        // One statement reads and removes the row, so two answers cannot both take it.
        let found = self.on_tenant(tenant, |t| {
            match t
                .query_opt(
                    &*format!(
                        "DELETE FROM pending_sign_ins WHERE id_hash = $1 RETURNING {PENDING_COLUMNS}"
                    ),
                    &[&id_hash],
                )
                .map_err(db)?
            {
                Some(row) => pending_sign_in(tenant, id_hash, &row).map(Some),
                None => Ok(None),
            }
        })?;
        Ok(found.flatten().filter(|held| held.expires_at.0 > now.0))
    }

    fn put_recovery(&self, request: &RecoveryRequest) -> StoreResult<()> {
        self.on_existing(&request.tenant, |t| {
            t.execute(
                "DELETE FROM recovery_requests WHERE principal = $1",
                &[&request.principal.as_str()],
            )
            .map_err(db)?;
            t.execute(
                &*format!(
                    "INSERT INTO recovery_requests (id_hash, {RECOVERY_COLUMNS})
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                     ON CONFLICT (id_hash) DO UPDATE SET principal = EXCLUDED.principal,
                        login = EXCLUDED.login, token_hash = EXCLUDED.token_hash,
                        cancel_hash = EXCLUDED.cancel_hash, requested_at = EXCLUDED.requested_at,
                        completes_at = EXCLUDED.completes_at, expires_at = EXCLUDED.expires_at"
                ),
                &[
                    &request.id_hash,
                    &request.principal.as_str(),
                    &request.login,
                    &request.token_hash,
                    &request.cancel_hash,
                    &request.requested_at.0,
                    &request.completes_at.0,
                    &request.expires_at.0,
                ],
            )
            .map_err(db)?;
            Ok(())
        })
    }

    fn recovery(&self, tenant: &TenantId, id_hash: &str) -> StoreResult<Option<RecoveryRequest>> {
        let found = self.peek(tenant, |t| {
            match t
                .query_opt(
                    &*format!(
                        "SELECT {RECOVERY_COLUMNS} FROM recovery_requests WHERE id_hash = $1"
                    ),
                    &[&id_hash],
                )
                .map_err(db)?
            {
                Some(row) => recovery_request(tenant, id_hash, &row).map(Some),
                None => Ok(None),
            }
        })?;
        Ok(found.flatten())
    }

    fn take_recovery(
        &self,
        tenant: &TenantId,
        id_hash: &str,
    ) -> StoreResult<Option<RecoveryRequest>> {
        let found = self.on_tenant(tenant, |t| {
            match t
                .query_opt(
                    &*format!(
                        "DELETE FROM recovery_requests WHERE id_hash = $1 RETURNING {RECOVERY_COLUMNS}"
                    ),
                    &[&id_hash],
                )
                .map_err(db)?
            {
                Some(row) => recovery_request(tenant, id_hash, &row).map(Some),
                None => Ok(None),
            }
        })?;
        Ok(found.flatten())
    }

    fn cancel_recovery(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        cancel_hash: &str,
    ) -> StoreResult<bool> {
        let removed = self.on_tenant(tenant, |t| {
            t.execute(
                "DELETE FROM recovery_requests WHERE id_hash = $1 AND cancel_hash = $2",
                &[&id_hash, &cancel_hash],
            )
            .map_err(db)
        })?;
        Ok(removed == Some(1))
    }

    fn purge_factor_challenges(&self, now: Timestamp) -> StoreResult<u64> {
        let pending = self.on_every_tenant(
            "DELETE FROM pending_sign_ins WHERE expires_at <= $1",
            &[&now.0],
        )?;
        let recoveries = self.on_every_tenant(
            "DELETE FROM recovery_requests WHERE expires_at <= $1",
            &[&now.0],
        )?;
        Ok(pending + recoveries)
    }

    fn put_account(&self, account: &Account) -> StoreResult<()> {
        self.on_existing(&account.tenant, |t| {
            t.execute(
                "INSERT INTO accounts (login, principal, display_name, password_hash, verified_email) VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (login) DO UPDATE SET principal = EXCLUDED.principal,
                    display_name = EXCLUDED.display_name, password_hash = EXCLUDED.password_hash,
                    verified_email = EXCLUDED.verified_email",
                &[
                    &account.login,
                    &account.principal.as_str(),
                    &account.display_name,
                    &account.password_hash,
                    &account.verified_email,
                ],
            )
            .map_err(db)?;
            Ok(())
        })
    }

    fn create_account(&self, account: &Account) -> StoreResult<bool> {
        let inserted = self.on_existing(&account.tenant, |t| {
            t.execute(
                "INSERT INTO accounts (login, principal, display_name, password_hash, verified_email) VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (login) DO NOTHING",
                &[
                    &account.login,
                    &account.principal.as_str(),
                    &account.display_name,
                    &account.password_hash,
                    &account.verified_email,
                ],
            )
            .map_err(db)
        })?;
        Ok(inserted == 1)
    }

    fn update_account(&self, expected: &Account, replacement: &Account) -> StoreResult<bool> {
        if expected.tenant != replacement.tenant || expected.login != replacement.login {
            return Err(StoreError::Invalid(
                "an account update keeps its tenant and login".into(),
            ));
        }
        let changed = self.on_existing(&expected.tenant, |t| {
            t.execute(
                "UPDATE accounts SET principal = $2, display_name = $3, password_hash = $4, verified_email = $5
                 WHERE login = $1 AND principal = $6 AND display_name = $7 AND password_hash = $8
                   AND verified_email IS NOT DISTINCT FROM $9::text",
                &[
                    &expected.login,
                    &replacement.principal.as_str(),
                    &replacement.display_name,
                    &replacement.password_hash,
                    &replacement.verified_email,
                    &expected.principal.as_str(),
                    &expected.display_name,
                    &expected.password_hash,
                    &expected.verified_email,
                ],
            )
            .map_err(db)
        })?;
        Ok(changed == 1)
    }

    fn account(&self, tenant: &TenantId, login: &str) -> StoreResult<Option<Account>> {
        let found = self.peek(tenant, |t| {
            match t
                .query_opt(
                    "SELECT principal, display_name, password_hash, verified_email FROM accounts WHERE login = $1",
                    &[&login],
                )
                .map_err(db)?
            {
                Some(row) => account_from(tenant, login.to_string(), &row, 0).map(Some),
                None => Ok(None),
            }
        })?;
        Ok(found.flatten())
    }

    fn remove_account(&self, tenant: &TenantId, login: &str) -> StoreResult<bool> {
        let removed = self.on_tenant(tenant, |t| {
            t.execute("DELETE FROM accounts WHERE login = $1", &[&login])
                .map_err(db)
        })?;
        Ok(removed.unwrap_or(0) > 0)
    }

    fn anonymise_ledger_principal(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        expected: Option<&Generation>,
    ) -> StoreResult<strata_kernel::ledger::Anonymised> {
        Ok(self
            .on_tenant(tenant, |t| {
                // The tenant row is locked for the comparison, as in a commit.
                t.query_opt(
                    "SELECT 1 FROM tenant WHERE id = $1 FOR UPDATE",
                    &[&tenant.as_str()],
                )
                .map_err(db)?;
                let held = guard_generation(t, tenant, expected)?;
                let done = ledger::anonymise(t, principal)?;
                // The counter is raised when, and only when, the clearing changed what a load
                // returns.
                if done != Default::default() {
                    if let Some(held) = held {
                        write_generation(t, &held.raised())?;
                    }
                }
                Ok(done)
            })?
            .unwrap_or_default())
    }

    fn remove_principal(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<PrincipalRemoval> {
        // One transaction: a sign-in sees all of the principal's rows or none of them.
        Ok(self
            .on_tenant(tenant, |t| {
                let who = principal.as_str();
                let mut gone =
                    |sql: &str| -> StoreResult<u64> { t.execute(sql, &[&who]).map_err(db) };
                Ok(PrincipalRemoval {
                    accounts: gone("DELETE FROM accounts WHERE principal = $1")?,
                    passkeys: gone("DELETE FROM passkeys WHERE principal = $1")?,
                    passkey_challenges: gone(
                        "DELETE FROM passkey_challenges WHERE principal = $1",
                    )?,
                    provider_flows: gone("DELETE FROM oidc_flows WHERE principal = $1")?
                        + gone("DELETE FROM saml_requests WHERE principal = $1")?,
                    identities: gone("DELETE FROM federated_identities WHERE principal = $1")?,
                    totp_factors: gone("DELETE FROM totp_factors WHERE principal = $1")?,
                    recovery_codes: gone("DELETE FROM recovery_codes WHERE principal = $1")?,
                    pending_sign_ins: gone("DELETE FROM pending_sign_ins WHERE principal = $1")?,
                    sign_in_links: gone("DELETE FROM sign_in_links WHERE principal = $1")?,
                    recovery_requests: gone("DELETE FROM recovery_requests WHERE principal = $1")?,
                })
            })?
            .unwrap_or_default())
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
            "INSERT INTO identity_docs (kind, key, value) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING"
        } else {
            "INSERT INTO identity_docs (kind, key, value) VALUES ($1, $2, $3)
             ON CONFLICT (kind, key) DO UPDATE SET value = EXCLUDED.value"
        };
        // A tenant that does not exist has no schema, and is refused as not found.
        let changed = self.on_existing(tenant, |t| {
            t.execute(sql, &[&kind, &key, &value]).map_err(db)
        })?;
        Ok(changed == 1)
    }

    fn doc_get(&self, tenant: &TenantId, kind: &str, key: &str) -> StoreResult<Option<String>> {
        let found = self.peek(tenant, |t| {
            select_strings(
                t,
                "SELECT value FROM identity_docs WHERE kind = $1 AND key = $2",
                &[&kind, &key],
            )
        })?;
        Ok(found.and_then(|values| values.into_iter().next()))
    }

    fn doc_list(&self, tenant: &TenantId, kind: &str) -> StoreResult<Vec<(String, String)>> {
        Ok(self
            .peek(tenant, |t| {
                let rows = t
                    .query(
                        "SELECT key, value FROM identity_docs WHERE kind = $1 ORDER BY key",
                        &[&kind],
                    )
                    .map_err(db)?;
                let mut out = Vec::with_capacity(rows.len());
                for row in &rows {
                    let key: String = get(row, 0)?;
                    let value: String = get(row, 1)?;
                    out.push((key, value));
                }
                Ok(out)
            })?
            .unwrap_or_default())
    }

    fn doc_remove(&self, tenant: &TenantId, kind: &str, key: &str) -> StoreResult<bool> {
        let removed = self.on_tenant(tenant, |t| {
            t.execute(
                "DELETE FROM identity_docs WHERE kind = $1 AND key = $2",
                &[&kind, &key],
            )
            .map_err(db)
        })?;
        Ok(removed == Some(1))
    }

    fn audit_append(&self, tenant: &TenantId, entry: &AuditEntry) -> StoreResult<()> {
        // The chain value of the line before, under the tenant's writer lock, so that two appends
        // never chain to the same line.
        self.on_existing(tenant, |t| append_audit(t, entry))
    }

    fn audit_chain(
        &self,
        tenant: &TenantId,
        after: u64,
        limit: usize,
    ) -> StoreResult<Vec<strata_storage_api::audit_chain::ChainedAuditEntry>> {
        let after: i64 = after as i64;
        let limit: i64 = limit as i64;
        Ok(self
            .peek(tenant, |t| {
                let rows = t
                    .query(
                        "SELECT seq, at, kind, event, correlation, data, chain FROM audit_log
                         WHERE seq > $1 ORDER BY seq LIMIT $2",
                        &[&after, &limit],
                    )
                    .map_err(db)?;
                let mut out = Vec::with_capacity(rows.len());
                for row in &rows {
                    let seq: i64 = get(row, 0)?;
                    out.push(strata_storage_api::audit_chain::ChainedAuditEntry {
                        seq: seq as u64,
                        entry: AuditEntry {
                            at: get(row, 1)?,
                            kind: get(row, 2)?,
                            event: get(row, 3)?,
                            correlation: get(row, 4)?,
                            data: get(row, 5)?,
                        },
                        chain: get(row, 6)?,
                    });
                }
                Ok(out)
            })?
            .unwrap_or_default())
    }

    fn audit_list(
        &self,
        tenant: &TenantId,
        kind: Option<&str>,
        limit: usize,
    ) -> StoreResult<Vec<AuditEntry>> {
        let limit: i64 = limit as i64;
        let mut rows = self
            .peek(tenant, |t| {
                let rows = t
                    .query(
                        "SELECT at, kind, event, correlation, data FROM audit_log
                         WHERE $1::text IS NULL OR kind = $1::text ORDER BY seq DESC LIMIT $2",
                        &[&kind, &limit],
                    )
                    .map_err(db)?;
                let mut out = Vec::with_capacity(rows.len());
                for row in &rows {
                    out.push(AuditEntry {
                        at: get(row, 0)?,
                        kind: get(row, 1)?,
                        event: get(row, 2)?,
                        correlation: get(row, 3)?,
                        data: get(row, 4)?,
                    });
                }
                Ok(out)
            })?
            .unwrap_or_default();
        rows.reverse();
        Ok(rows)
    }

    fn create_snapshot(
        &self,
        tenant: &TenantId,
        label: &str,
        at: Timestamp,
    ) -> StoreResult<SnapshotInfo> {
        self.on_existing(tenant, |t| {
            let state = complete(t, tenant)?
                .ok_or_else(|| StoreError::NotFound(format!("tenant {tenant}")))?;
            let id = count(t, "SELECT COALESCE(MAX(id), 0) + 1 FROM snapshots", &[])?;
            let info = SnapshotInfo {
                tenant: tenant.clone(),
                id,
                label: label.to_string(),
                revision: state.revision,
                hash: canon::hash_of(&state),
                created_at: at,
            };
            let row_id: i64 = id as i64;
            let revision: i64 = state.revision.0 as i64;
            t.execute(
                "INSERT INTO snapshots (id, label, revision, hash, created_at, data) VALUES ($1, $2, $3, $4, $5, $6)",
                &[&row_id, &label, &revision, &info.hash, &at.0, &encode(&state)],
            )
            .map_err(db)?;
            Ok(info)
        })
    }

    fn snapshots(&self, tenant: &TenantId) -> StoreResult<Vec<SnapshotInfo>> {
        Ok(self
            .peek(tenant, |t| {
                let rows = t
                    .query(
                        "SELECT id, label, revision, hash, created_at FROM snapshots ORDER BY id",
                        &[],
                    )
                    .map_err(db)?;
                let mut out = Vec::with_capacity(rows.len());
                for row in &rows {
                    let id: i64 = get(row, 0)?;
                    let revision: i64 = get(row, 2)?;
                    let created_at: i64 = get(row, 4)?;
                    out.push(SnapshotInfo {
                        tenant: tenant.clone(),
                        id: id as u64,
                        label: get(row, 1)?,
                        revision: StateRevision(revision as u64),
                        hash: get(row, 3)?,
                        created_at: Timestamp(created_at),
                    });
                }
                Ok(out)
            })?
            .unwrap_or_default())
    }

    fn load_snapshot(&self, tenant: &TenantId, id: u64) -> StoreResult<TenantState> {
        let missing = || StoreError::NotFound(format!("snapshot {id} of {tenant}"));
        let row_id: i64 = id as i64;
        let data = self
            .peek(tenant, |t| {
                select_strings(t, "SELECT data FROM snapshots WHERE id = $1", &[&row_id])
            })?
            .and_then(|found| found.into_iter().next())
            .ok_or_else(missing)?;
        decode(&data, "snapshot")
    }

    /// As SQLite restores a snapshot, and one thing more: the search entries of the records put
    /// back are written again from the snapshot's schema, so that the index stays the function of
    /// the records and the schema that it is everywhere else. (SQLite's restore leaves them out.)
    fn restore_snapshot(
        &self,
        tenant: &TenantId,
        id: u64,
        expected: Option<&Generation>,
    ) -> StoreResult<()> {
        let state = self.load_snapshot(tenant, id)?;
        let scope = self.existing(tenant)?;
        scope.write(|t| {
            t.query_opt(
                "SELECT 1 FROM tenant WHERE id = $1 FOR UPDATE",
                &[&tenant.as_str()],
            )
            .map_err(db)?;
            let held = guard_generation(t, tenant, expected)?;
            ledger::check_restorable(t, &state)?;
            t.execute("DELETE FROM records", &[]).map_err(db)?;
            raise_object_revision(t, "")?;
            let revision: i64 = state.revision.0 as i64;
            let next_record: i64 = state.next_record as i64;
            let event_seq: i64 = state.event_seq as i64;
            t.execute(
                "UPDATE tenant SET name = $1, revision = $2, next_record = $3, event_seq = GREATEST(event_seq, $4),
                    schema = $5, members = $6
                 WHERE id = $7",
                &[
                    &state.name,
                    &revision,
                    &next_record,
                    &event_seq,
                    &encode(&*state.schema),
                    &encode(&state.members),
                    &tenant.as_str(),
                ],
            )
            .map_err(db)?;
            pair_index::reset(t, &state.schema)?;
            for record in state.records.values().flat_map(|records| records.values()) {
                write_record(t, record)?;
                search_index::write_entries(t, &state.schema, record)?;
            }
            t.execute("DELETE FROM setting_overrides", &[]).map_err(db)?;
            t.execute("DELETE FROM setting_revisions", &[]).map_err(db)?;
            if let Some(values) = &state.setting_values {
                for held in values.overrides.values() {
                    write_setting_override(t, held)?;
                }
                for (setting, revision) in &values.revisions {
                    write_setting_revision(t, setting, *revision)?;
                }
            }
            // Events stay and the counter does not go back (`GREATEST` above): a subscriber that
            // has passed a sequence number must still see every later event.
            t.execute("DELETE FROM journal WHERE revision > $1", &[&revision])
                .map_err(db)?;
            let version: i64 = state.schema.version.0 as i64;
            t.execute(
                "DELETE FROM schema_history WHERE version > $1",
                &[&version],
            )
            .map_err(db)?;
            // The membership and wake rows no longer rest on the records that were put back:
            // every segment is evaluated again as if its predicate had changed (WG-5167).
            let catalog = catalog_rows(t)?;
            if !catalog.is_empty() {
                let write = strata_kernel::segment_flow::restore_write(&catalog);
                t.execute("DELETE FROM segment_memberships", &[])
                    .map_err(db)?;
                t.execute("DELETE FROM segment_wakes", &[]).map_err(db)?;
                apply_segments(t, &write)?;
                t.execute("UPDATE segments SET member_count = 0", &[])
                    .map_err(db)?;
                let jobs =
                    strata_storage_api::segment_jobs::of_write(&write, "restore", Timestamp(0));
                insert_jobs(t, tenant, &jobs, Timestamp(0))?;
            }
            // A restore draws a new nonce and raises the counter, in this transaction.
            write_generation(
                t,
                &Generation {
                    nonce: self.nonces.draw(tenant),
                    counter: held.map_or(0, |g| g.counter) + 1,
                },
            )?;
            Ok(())
        })?;
        let _ = self.wake_refresh(tenant, &scope);
        Ok(())
    }

    fn reset(&self) -> StoreResult<()> {
        let all = self
            .control
            .read(|t| select_strings(t, "SELECT id FROM tenant_directory ORDER BY id", &[]))?;
        for id in all {
            self.remove_tenant(&TenantId::new(id).map_err(corrupt_id)?)?;
        }
        self.lock_open().clear();
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
        self.on_jobs(tenant, earliest, |t| insert_jobs(t, tenant, new, now))?
            .ok_or_else(|| StoreError::NotFound(format!("tenant {tenant}")))
    }

    fn job(&self, tenant: &TenantId, id: &str) -> StoreResult<Option<Job>> {
        Ok(self.peek(tenant, |t| job_by_id(t, id))?.flatten())
    }

    fn jobs_list(
        &self,
        tenant: &TenantId,
        before: Option<u64>,
        limit: usize,
    ) -> StoreResult<Vec<Job>> {
        let before: i64 = before.map_or(i64::MAX, |b| b as i64);
        let limit: i64 = limit as i64;
        Ok(self
            .peek(tenant, |t| {
                jobs_of(
                    t,
                    "SELECT seq, data::text FROM jobs WHERE seq < $1 ORDER BY seq DESC LIMIT $2",
                    &[&before, &limit],
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
            .on_jobs(tenant, None, |t| {
                let sql = format!(
                    "SELECT seq, data::text FROM jobs WHERE {OPEN_JOB} AND due_at <= $1 ORDER BY due_at, seq LIMIT 1"
                );
                let Some(job) = jobs_of(t, &sql, &[&now.0])?.pop() else {
                    return Ok(None);
                };
                let lease = jobs::apply_lease(&job, worker, now);
                put_job(t, &lease.job)?;
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
        let earliest = earliest_due(writes);
        Ok(self
            .on_jobs(tenant, earliest, |t| {
                let Some(job) = job_by_id(t, id)? else {
                    return Ok(Reported::Refused(format!("no job {id}")));
                };
                let next = match jobs::apply_report(&job, token, report, now) {
                    Ok(next) => next,
                    Err(why) => return Ok(Reported::Refused(why)),
                };
                put_job(t, &next)?;
                write_automation(t, tenant, writes, now)?;
                Ok(Reported::Recorded(Box::new(next)))
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
            .on_jobs(tenant, None, |t| {
                let Some(job) = job_by_id(t, id)? else {
                    return Ok(JobChange::NotFound);
                };
                let change = jobs::apply_cancel(&job, reason, now);
                if let JobChange::Changed(next) = &change {
                    put_job(t, next)?;
                }
                Ok(change)
            })?
            .unwrap_or(JobChange::NotFound))
    }

    fn job_retry(&self, tenant: &TenantId, id: &str, now: Timestamp) -> StoreResult<JobChange> {
        Ok(self
            .on_jobs(tenant, Some(now.0), |t| {
                let Some(job) = job_by_id(t, id)? else {
                    return Ok(JobChange::NotFound);
                };
                let change = jobs::apply_retry(&job, now);
                if let JobChange::Changed(next) = &change {
                    put_job(t, next)?;
                }
                Ok(change)
            })?
            .unwrap_or(JobChange::NotFound))
    }

    fn jobs_due(&self, now: Timestamp, limit: usize) -> StoreResult<Vec<TenantId>> {
        // The wake index only: no tenant schema is read to find due work (SCH-007).
        let limit: i64 = limit as i64;
        self.control
            .read(|t| {
                select_strings(
                    t,
                    "SELECT w.tenant FROM job_wake w JOIN tenant_directory d ON d.id = w.tenant
                     WHERE d.state = 'active' AND w.due_at <= $1 ORDER BY w.due_at, w.tenant LIMIT $2",
                    &[&now.0, &limit],
                )
            })?
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
        let earliest = earliest_due(writes);
        self.on_jobs(tenant, earliest, |t| {
            write_automation(t, tenant, writes, now)
        })?
        .ok_or_else(|| StoreError::NotFound(format!("tenant {tenant}")))
    }

    fn automation_run(&self, tenant: &TenantId, run: &str) -> StoreResult<Option<RunRecord>> {
        Ok(self
            .peek(tenant, |t| {
                Ok(runs_of(
                    t,
                    "SELECT seq, data::text FROM automation_runs WHERE run = $1",
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
        let before: i64 = before.map_or(i64::MAX, |b| b as i64);
        let automation: Option<String> = automation.map(|a| a.as_str().to_string());
        let state: Option<&str> = state.map(|s| s.as_str());
        let limit: i64 = limit as i64;
        Ok(self
            .peek(tenant, |t| {
                runs_of(
                    t,
                    "SELECT seq, data::text FROM automation_runs
                     WHERE seq < $1 AND ($2::text IS NULL OR automation = $2::text)
                       AND ($3::text IS NULL OR state = $3::text)
                     ORDER BY seq DESC LIMIT $4",
                    &[&before, &automation, &state, &limit],
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
        let after: i64 = after.map_or(0, |a| a as i64);
        let limit: i64 = limit as i64;
        Ok(self
            .peek(tenant, |t| {
                select_strings(
                    t,
                    "SELECT data FROM automation_steps WHERE run = $1 AND sequence > $2
                     ORDER BY sequence LIMIT $3",
                    &[&run, &after, &limit],
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
        let before: i64 = before.map_or(i64::MAX, |b| b as i64);
        let kind: Option<&str> = kind.map(|k| k.as_str());
        let unsettled: i64 = i64::from(unsettled);
        let limit: i64 = limit as i64;
        Ok(self
            .peek(tenant, |t| {
                exceptions_of(
                    t,
                    "SELECT seq, data::text FROM automation_exceptions
                     WHERE seq < $1 AND ($2::text IS NULL OR kind = $2::text) AND ($3::bigint = 0 OR settled = 0)
                     ORDER BY seq DESC LIMIT $4",
                    &[&before, &kind, &unsettled, &limit],
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
            .on_jobs(tenant, creates, |t| {
                let Some(mut record) = exceptions_of(
                    t,
                    "SELECT seq, data::text FROM automation_exceptions WHERE id = $1",
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
                    let Some(job) = job_by_id(t, &job_id)? else {
                        return Ok(Settled::Conflict(format!("no job {job_id} to release")));
                    };
                    let JobChange::Changed(next) = jobs::apply_retry(&job, now) else {
                        return Ok(Settled::Conflict(format!(
                            "job {job_id} has not failed and cannot be given another attempt"
                        )));
                    };
                    put_job(t, &next)?;
                    released = Some(job_id);
                }
                record.settled = true;
                record.seq = 0;
                t.execute(
                    "UPDATE automation_exceptions SET settled = 1, data = $2::text::jsonb WHERE id = $1",
                    &[&id, &encode(&record)],
                )
                .map_err(db)?;
                Ok(Settled::Done { job: released })
            })?
            .unwrap_or(Settled::NotFound))
    }

    fn schedule_set(
        &self,
        tenant: &TenantId,
        set: &ScheduleSet,
        now: Timestamp,
    ) -> StoreResult<Schedule> {
        self.existing(tenant)?;
        self.on_jobs(tenant, set.due.map(|due| due.0), |t| {
            let held = schedule_by_id(t, &set.id)?;
            let outcome = schedules::apply_set(held.as_ref(), set, now);
            if let Some(problem) = outcome.create.as_ref().and_then(jobs::job_problem) {
                return Err(StoreError::Invalid(problem));
            }
            if outcome.replace_open {
                let sql = format!(
                    "SELECT seq, data::text FROM jobs WHERE {OPEN_JOB}
                     AND data -> 'work' -> 'schedule-due' ->> 'schedule' = $1"
                );
                for job in jobs_of(t, &sql, &[&set.id])? {
                    if let JobChange::Changed(next) =
                        jobs::apply_cancel(&job, "the rule of the schedule was changed", now)
                    {
                        put_job(t, &next)?;
                    }
                }
            }
            put_schedule(t, &outcome.schedule)?;
            if let Some(job) = &outcome.create {
                insert_jobs(t, tenant, std::slice::from_ref(job), now)?;
            }
            Ok(outcome.schedule)
        })?
        .ok_or_else(|| StoreError::NotFound(format!("tenant {tenant}")))
    }

    fn schedule(&self, tenant: &TenantId, id: &str) -> StoreResult<Option<Schedule>> {
        Ok(self.peek(tenant, |t| schedule_by_id(t, id))?.flatten())
    }

    fn schedules(&self, tenant: &TenantId) -> StoreResult<Vec<Schedule>> {
        Ok(self
            .peek(tenant, |t| {
                select_strings(t, "SELECT data FROM schedules ORDER BY id", &[])?
                    .iter()
                    .map(|data| decode(data, "schedule"))
                    .collect()
            })?
            .unwrap_or_default())
    }

    fn run_key_holder(
        &self,
        tenant: &TenantId,
        automation: &WorkflowId,
        key: &str,
    ) -> StoreResult<Option<String>> {
        let automation = automation.as_str();
        Ok(self
            .peek(tenant, |t| {
                Ok(select_strings(
                    t,
                    "SELECT run FROM automation_run_keys WHERE automation = $1 AND key = $2",
                    &[&automation, &key],
                )?
                .into_iter()
                .next())
            })?
            .flatten())
    }

    fn open_run_key_starts(
        &self,
        tenant: &TenantId,
        automation: &WorkflowId,
        key: &str,
    ) -> StoreResult<Vec<Job>> {
        let automation = automation.as_str();
        let sql = format!(
            "SELECT seq, data::text FROM jobs WHERE {OPEN_JOB}
             AND data -> 'work' -> 'start-run' ->> 'run_key' = $1
             AND data -> 'work' -> 'start-run' ->> 'automation' = $2"
        );
        Ok(self
            .peek(tenant, |t| jobs_of(t, &sql, &[&key, &automation]))?
            .unwrap_or_default())
    }

    fn purge_automation(
        &self,
        now: Timestamp,
        retention: &AutomationRetention,
    ) -> StoreResult<AutomationPurged> {
        let jobs_before: i64 = now.0.saturating_sub(retention.jobs_ms);
        let runs_before: i64 = now.0.saturating_sub(retention.runs_ms);
        let exceptions_before: i64 = now.0.saturating_sub(retention.exceptions_ms);
        let mut total = AutomationPurged::default();
        let ended_runs = "state IN ('completed', 'failed', 'cancelled')
             AND (data ->> 'ended_at')::bigint <= $1";
        for tenant in self.directory()? {
            let Some(scope) = self.tenant_db(&tenant)? else {
                continue;
            };
            let done = scope.write(|t| {
                let steps = t
                    .execute(
                        &*format!(
                            "DELETE FROM automation_steps WHERE run IN
                             (SELECT run FROM automation_runs WHERE {ended_runs})"
                        ),
                        &[&runs_before],
                    )
                    .map_err(db)?;
                let runs = t
                    .execute(
                        &*format!("DELETE FROM automation_runs WHERE {ended_runs}"),
                        &[&runs_before],
                    )
                    .map_err(db)?;
                let jobs = t
                    .execute(
                        "DELETE FROM jobs WHERE state IN ('completed', 'failed', 'cancelled')
                         AND (data ->> 'ended_at')::bigint <= $1",
                        &[&jobs_before],
                    )
                    .map_err(db)?;
                let exceptions = t
                    .execute(
                        "DELETE FROM automation_exceptions
                         WHERE (data ->> 'at')::bigint <= $1 AND (kind IN ('declined', 'held') OR settled = 1)",
                        &[&exceptions_before],
                    )
                    .map_err(db)?;
                Ok(AutomationPurged {
                    jobs,
                    runs,
                    steps,
                    exceptions,
                })
            })?;
            total.jobs += done.jobs;
            total.runs += done.runs;
            total.steps += done.steps;
            total.exceptions += done.exceptions;
        }
        Ok(total)
    }

    fn platform_settings(&self) -> StoreResult<Vec<PlatformSetting>> {
        let rows: Vec<(String, Option<String>, i64)> = self.control.read(|t| {
            let rows = t
                .query(
                    "SELECT setting, value, revision FROM platform_settings ORDER BY setting",
                    &[],
                )
                .map_err(db)?;
            let mut out = Vec::with_capacity(rows.len());
            for row in &rows {
                out.push((get(row, 0)?, get(row, 1)?, get(row, 2)?));
            }
            Ok(out)
        })?;
        rows.into_iter()
            .map(|(setting, value, revision)| {
                Ok(PlatformSetting {
                    setting: SettingId::new(setting).map_err(corrupt_id)?,
                    value: value
                        .map(|text| decode::<Value>(&text, "a platform setting value"))
                        .transpose()?,
                    revision: revision as u64,
                })
            })
            .collect()
    }

    fn platform_setting_write(
        &self,
        setting: &SettingId,
        value: Option<&Value>,
        expected_revision: u64,
        at: Timestamp,
    ) -> StoreResult<PlatformWrite> {
        self.control.write(|t| {
            let held: Option<(Option<String>, i64)> = match t
                .query_opt(
                    "SELECT value, revision FROM platform_settings WHERE setting = $1 FOR UPDATE",
                    &[&setting.as_str()],
                )
                .map_err(db)?
            {
                Some(row) => Some((get(&row, 0)?, get(&row, 1)?)),
                None => None,
            };
            let current = held.as_ref().map_or(0, |(_, revision)| *revision as u64);
            if current != expected_revision {
                return Ok(PlatformWrite::Conflict { current });
            }
            let previous = held
                .and_then(|(text, _)| text)
                .map(|text| decode::<Value>(&text, "a platform setting value"))
                .transpose()?;
            let revision = current + 1;
            let stored_revision: i64 = revision as i64;
            let text: Option<String> = value.map(encode);
            t.execute(
                "INSERT INTO platform_settings (setting, value, revision, updated_at)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (setting) DO UPDATE SET
                    value = EXCLUDED.value, revision = EXCLUDED.revision,
                    updated_at = EXCLUDED.updated_at",
                &[&setting.as_str(), &text, &stored_revision, &at.0],
            )
            .map_err(db)?;
            Ok(PlatformWrite::Written {
                now: PlatformSetting {
                    setting: setting.clone(),
                    value: value.cloned(),
                    revision,
                },
                previous,
            })
        })
    }

    fn begin_provisioning(&self, tenant: &TenantId) -> StoreResult<()> {
        self.announced
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(tenant.clone());
        Ok(())
    }

    fn finish_provisioning(&self, tenant: &TenantId) -> StoreResult<()> {
        self.set_directory_state(tenant, "provisioning", "active")
    }

    fn rollback_provisioning(&self) -> StoreResult<Vec<TenantId>> {
        let pending = self.ids_in_state("provisioning")?;
        let mut removed = Vec::new();
        for tenant in pending {
            self.remove_tenant(&tenant)?;
            removed.push(tenant);
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_tenant_id_is_its_own_schema() {
        let tenant = TenantId::new("alpha_1").unwrap();
        assert_eq!(tenant_schema(&tenant), "t_alpha_1");
        assert!(pool::valid_schema(&tenant_schema(&tenant)));
    }

    #[test]
    fn a_long_tenant_id_gets_a_schema_that_fits() {
        let id = "a".repeat(64);
        let tenant = TenantId::new(id.clone()).unwrap();
        let schema = tenant_schema(&tenant);
        assert!(schema.len() <= 63, "{schema}");
        assert!(pool::valid_schema(&schema));
        let other = TenantId::new(format!("{}b", "a".repeat(63))).unwrap();
        assert_ne!(schema, tenant_schema(&other));
    }

    #[test]
    fn a_password_is_left_out_of_the_diagnostics() {
        assert_eq!(
            redact("postgres://strata:secret@127.0.0.1:55432/strata"),
            "postgres://127.0.0.1:55432/strata"
        );
    }

    #[test]
    fn an_unusual_schema_name_is_refused() {
        assert!(quoted("t_alpha").is_ok());
        assert!(quoted("t-alpha").is_err());
        assert!(quoted("T_alpha").is_err());
        assert!(quoted("1alpha").is_err());
        assert!(quoted("x\"; DROP SCHEMA public; --").is_err());
        assert!(quoted("").is_err());
    }
}
