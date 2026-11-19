//! The persistence contract.
//!
//! [`Store`] says what Strata needs from a database, in Strata's terms: tenant-scoped snapshots,
//! atomic commits guarded by a revision, a durable ordered event log with an outbox, a journal
//! of accepted transitions, sessions, accounts and snapshots. It says nothing about SQL.
//!
//! An adapter implements the trait and passes [`conformance::run`]. Two adapters exist:
//! [`memory::MemoryStore`] here, and the SQLite adapter in `strata-storage-sqlite`.
//!
//! # Guarantees every adapter must give
//!
//! - **Atomicity.** [`Store::commit`] applies the whole commit (state delta, events, journal
//!   entry, counters, session revocations) or none of it.
//! - **Conflict detection.** A commit is applied only if the tenant's stored revision equals the
//!   delta's base revision (or the tenant is absent when the base is `None`). Otherwise it
//!   fails with [`StoreError::Conflict`] and changes nothing.
//! - **Isolation.** Commits to one tenant are serialisable: each sees the state left by the
//!   previous one. A load returns a state that some commit actually produced, never a mixture.
//!   Adapters may serialise all writers; they may not expose a partially applied commit.
//! - **Tenant scope.** Every read and write names a tenant. Data of one tenant is never
//!   returned for, or changed through, another.
//! - **Ordering.** Events have a per-tenant sequence that continues without gaps, and a global
//!   outbox position that follows commit order.
//! - **Durability.** What [`Capabilities::durable`] says. A durable adapter keeps every
//!   acknowledged commit across a process restart.
//!
//! What adapters may differ in is stated by [`Capabilities`], not hidden.

pub mod memory;

#[cfg(feature = "conformance")]
pub mod conformance;

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;
use strata_kernel::{
    canon, Context, Coverage, DomainError, DomainEvent, Effect, ErrorKind, EventBody, Intent, Need,
    PrincipalId, ReadRequest, ReadResult, Record, SessionId, State, StateDelta, StateRevision,
    TenantId, TenantState, Timestamp, Transition, Value,
};

/// What an adapter actually provides. Reported in diagnostics and checked by the conformance
/// suite where it can be.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub adapter: String,
    /// Acknowledged commits survive a process restart.
    pub durable: bool,
    /// How concurrent commits are ordered, in the adapter's own words.
    pub isolation: String,
    /// What a reader can observe while a commit is in progress.
    pub read_consistency: String,
    /// Notes on behaviour a caller should know about.
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoreError {
    /// The tenant was not at the revision the commit was based on. Nothing was changed.
    Conflict {
        expected: Option<StateRevision>,
        found: Option<StateRevision>,
    },
    NotFound(String),
    /// The commit is malformed. Nothing was changed.
    Invalid(String),
    /// The store could not do the work right now (busy, I/O). Safe to retry.
    Unavailable(String),
    /// Stored data could not be read back. Not safe to ignore.
    Corrupt(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Conflict { expected, found } => {
                write!(
                    f,
                    "revision conflict: commit based on {expected:?}, store is at {found:?}"
                )
            }
            StoreError::NotFound(what) => write!(f, "not found: {what}"),
            StoreError::Invalid(why) => write!(f, "invalid commit: {why}"),
            StoreError::Unavailable(why) => write!(f, "store unavailable: {why}"),
            StoreError::Corrupt(why) => write!(f, "stored data is corrupt: {why}"),
        }
    }
}

impl std::error::Error for StoreError {}

pub type StoreResult<T> = Result<T, StoreError>;

/// One accepted transition as it was decided: enough to run it again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub tenant: TenantId,
    pub base_revision: Option<StateRevision>,
    pub revision: StateRevision,
    pub kernel_version: String,
    pub intent: Intent,
    pub context: Context,
    /// Canonical hash of the state delta the kernel produced.
    pub delta_hash: String,
    /// Canonical hash of the events the kernel produced.
    pub events_hash: String,
}

/// Everything one accepted transition writes, applied atomically.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commit {
    pub delta: StateDelta,
    pub events: Vec<DomainEvent>,
    pub journal: JournalEntry,
    /// Principals whose sessions end with this commit.
    pub revoke_sessions: Vec<PrincipalId>,
}

impl Commit {
    /// The commit for an accepted transition.
    pub fn of(intent: &Intent, context: &Context, transition: &Transition) -> Commit {
        Commit {
            delta: transition.delta.clone(),
            events: transition.events.clone(),
            journal: JournalEntry {
                tenant: transition.delta.tenant.clone(),
                base_revision: transition.delta.base_revision,
                revision: transition.delta.new_revision,
                kernel_version: strata_kernel::KERNEL_VERSION.to_string(),
                intent: intent.clone(),
                context: context.clone(),
                delta_hash: canon::hash_of(&transition.delta),
                events_hash: canon::hash_of(&transition.events),
            },
            // A principal whose roles changed in any way starts again from a new sign-in (IDN-012):
            // a session issued under the old roles is not carried over to the new ones. The
            // kernel's own effect, for a principal left with no role, is included.
            revoke_sessions: transition
                .effects
                .iter()
                .filter_map(|effect| match effect {
                    Effect::RevokeSessions { principal } => Some(principal.clone()),
                    Effect::Publish { .. } => None,
                })
                .chain(
                    transition
                        .events
                        .iter()
                        .filter_map(|event| match &event.body {
                            EventBody::MemberChanged { principal, .. } => Some(principal.clone()),
                            _ => None,
                        }),
                )
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub revision: StateRevision,
    /// Outbox position of the last event written, if the commit had events.
    pub last_position: Option<u64>,
}

/// An event waiting to be handed to subscribers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxEntry {
    /// Global, increasing in commit order.
    pub position: u64,
    pub event: DomainEvent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantSummary {
    pub id: TenantId,
    pub name: String,
    pub revision: StateRevision,
    pub schema_version: u64,
    pub event_seq: u64,
    pub records: u64,
}

/// A server-side session. The cookie holds a random token; only its hash is stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: SessionId,
    pub token_hash: String,
    /// Compared with the request header on every state-changing request.
    pub csrf_token: String,
    pub tenant: TenantId,
    pub principal: PrincipalId,
    pub display_name: String,
    pub created_at: Timestamp,
    pub last_seen_at: Timestamp,
    /// Absolute end of the session.
    pub expires_at: Timestamp,
    pub revoked_at: Option<Timestamp>,
}

/// A locally managed login. Authentication lives in the shell; the kernel only knows principals.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub tenant: TenantId,
    pub login: String,
    pub principal: PrincipalId,
    pub display_name: String,
    /// An Argon2id PHC string.
    pub password_hash: String,
}

/// How one counter of failed attempts limits what follows it.
///
/// Attempts are counted when they are reserved, before the credential is checked, and given
/// back (or cleared) when it turns out to be right. So `max` attempts may be in flight at once,
/// and no more, however they interleave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThrottleRule {
    /// What is counted, such as one account, one source or one realm. Scoped by tenant.
    pub key: String,
    /// The attempt that reaches this count is the last one let through before the lock.
    pub max: u32,
    /// Failures older than this are forgotten.
    pub window_ms: i64,
    /// How long attempts are refused once `max` is reached.
    pub lock_ms: i64,
    /// Whether the count starts again when a lock ends (an account) or goes on until the window
    /// ends (a source, a realm).
    pub lock_resets: bool,
}

/// What a counter holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThrottleRow {
    pub failures: u32,
    pub window_start: i64,
    /// Attempts are refused until this time; 0 when there is no lock.
    pub locked_until: i64,
    /// When the row means nothing any more and may be purged.
    pub expires_at: i64,
}

/// The answer to a reservation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThrottleVerdict {
    /// Every counter had room, and each now holds this attempt.
    Allowed,
    /// A counter is locked; nothing was counted.
    Refused { until: Timestamp },
}

/// What becomes of a reserved attempt once its credential has been checked and found right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThrottleSettle {
    /// Forget the counter.
    Clear,
    /// Give the slot back and keep the other failures.
    Refund,
}

/// Count one more attempt on a counter, or refuse it. Shared by the adapters, so that they cannot
/// disagree on what a limit means.
pub fn throttle_step(
    row: Option<ThrottleRow>,
    rule: &ThrottleRule,
    now: i64,
) -> Result<ThrottleRow, i64> {
    let fresh = ThrottleRow {
        failures: 0,
        window_start: now,
        locked_until: 0,
        expires_at: 0,
    };
    let mut row = row.unwrap_or(fresh);
    if now - row.window_start >= rule.window_ms
        || (rule.lock_resets && row.locked_until != 0 && now >= row.locked_until)
    {
        row = fresh;
    }
    if row.locked_until > now {
        return Err(row.locked_until);
    }
    row.failures = row.failures.saturating_add(1);
    if row.failures >= rule.max {
        row.locked_until = now + rule.lock_ms;
    }
    row.expires_at = (row.window_start + rule.window_ms).max(row.locked_until);
    Ok(row)
}

/// The row after a reserved attempt is given back.
pub fn throttle_refund(mut row: ThrottleRow, max: u32) -> ThrottleRow {
    row.failures = row.failures.saturating_sub(1);
    if row.failures < max {
        row.locked_until = 0;
    }
    row
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub tenant: TenantId,
    pub id: u64,
    pub label: String,
    pub revision: StateRevision,
    /// Canonical hash of the tenant state held in the snapshot.
    pub hash: String,
    pub created_at: Timestamp,
}

/// A step in the adapter's own storage layout history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageMigration {
    pub version: u32,
    pub name: String,
    pub applied_at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub ok: bool,
    pub detail: String,
    pub tenants: u64,
    pub outbox_pending: u64,
}

/// The persistence contract. See the crate documentation for the guarantees.
pub trait Store: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    fn health(&self) -> StoreResult<Health>;
    /// The adapter's storage layout history.
    fn migrations(&self) -> StoreResult<Vec<StorageMigration>>;

    fn tenants(&self) -> StoreResult<Vec<TenantSummary>>;

    /// A snapshot of one tenant holding at least the records that answer `needs`, with partial
    /// coverage stating exactly `needs`. The tenant's schema, members and counters are always
    /// included. `tenant` is `None` in the result when the tenant does not exist.
    fn load(&self, tenant: &TenantId, needs: &[Need]) -> StoreResult<State>;

    /// A complete snapshot of one tenant.
    fn load_complete(&self, tenant: &TenantId) -> StoreResult<State>;

    /// Apply a commit atomically, guarded by the base revision. The commit's events must
    /// continue the tenant's event sequence without a gap.
    fn commit(&self, commit: &Commit) -> StoreResult<Receipt>;

    /// Events of a tenant with a sequence number above `after`, oldest first.
    fn events_after(
        &self,
        tenant: &TenantId,
        after: u64,
        limit: usize,
    ) -> StoreResult<Vec<DomainEvent>>;

    /// Events not yet marked published, in commit order across all tenants.
    fn outbox(&self, limit: usize) -> StoreResult<Vec<OutboxEntry>>;

    /// Mark every outbox entry up to and including `position` as published.
    fn mark_published(&self, position: u64) -> StoreResult<()>;

    /// Journal entries of a tenant with a revision above `after`, oldest first.
    fn journal_after(
        &self,
        tenant: &TenantId,
        after: u64,
        limit: usize,
    ) -> StoreResult<Vec<JournalEntry>>;

    fn put_session(&self, session: &SessionRecord) -> StoreResult<()>;
    fn session_by_token(&self, token_hash: &str) -> StoreResult<Option<SessionRecord>>;
    fn session(&self, id: &SessionId) -> StoreResult<Option<SessionRecord>>;
    /// Every session that is neither revoked nor past its end at `now`.
    fn live_sessions(&self, now: Timestamp) -> StoreResult<Vec<SessionRecord>>;
    /// Remove sessions that ended before `before`. Returns how many were removed.
    fn purge_sessions(&self, before: Timestamp) -> StoreResult<u64>;

    /// Reserve one attempt on every counter named, all or none, as one atomic step. When a counter
    /// is locked the attempt is refused and nothing is counted. The counters outlive the process
    /// and are shared by everything that uses the store.
    fn throttle_reserve(
        &self,
        tenant: &TenantId,
        rules: &[ThrottleRule],
        now: Timestamp,
    ) -> StoreResult<ThrottleVerdict>;
    /// Settle a reserved attempt on one counter whose credential turned out to be right. `max` is
    /// the counter's limit, to know whether giving the slot back lifts a lock.
    fn throttle_settle(
        &self,
        tenant: &TenantId,
        key: &str,
        max: u32,
        how: ThrottleSettle,
    ) -> StoreResult<()>;
    /// Remove counters that mean nothing any more at `now`. Returns how many were removed.
    fn purge_throttle(&self, now: Timestamp) -> StoreResult<u64>;

    fn put_account(&self, account: &Account) -> StoreResult<()>;
    fn account(&self, tenant: &TenantId, login: &str) -> StoreResult<Option<Account>>;
    /// Remove a login. Returns whether it was there.
    fn remove_account(&self, tenant: &TenantId, login: &str) -> StoreResult<bool>;

    /// Keep a copy of the tenant's complete state as it is now.
    fn create_snapshot(
        &self,
        tenant: &TenantId,
        label: &str,
        at: Timestamp,
    ) -> StoreResult<SnapshotInfo>;
    fn snapshots(&self, tenant: &TenantId) -> StoreResult<Vec<SnapshotInfo>>;
    fn load_snapshot(&self, tenant: &TenantId, id: u64) -> StoreResult<TenantState>;
    /// Put the tenant back exactly as the snapshot holds it, discarding events and journal
    /// entries newer than the snapshot. A development and recovery operation.
    fn restore_snapshot(&self, tenant: &TenantId, id: u64) -> StoreResult<()>;

    /// Remove everything. For development fixtures.
    fn reset(&self) -> StoreResult<()>;

    /// Remove one tenant and everything that belongs to it: its state, events, journal,
    /// sessions, logins and snapshots. Other tenants are untouched. For development fixtures.
    fn reset_tenant(&self, tenant: &TenantId) -> StoreResult<()>;
}

/// Check that a commit's events continue a tenant's sequence. Shared by adapters.
pub fn check_event_sequence(commit: &Commit, stored_event_seq: u64) -> StoreResult<()> {
    for (i, event) in commit.events.iter().enumerate() {
        let expected = stored_event_seq + 1 + i as u64;
        if event.seq != expected || event.tenant != commit.delta.tenant {
            return Err(StoreError::Invalid(format!(
                "event {i} has sequence {} for {}, expected {expected} for {}",
                event.seq, event.tenant, commit.delta.tenant
            )));
        }
    }
    if commit.delta.event_seq != stored_event_seq + commit.events.len() as u64 {
        return Err(StoreError::Invalid(
            "delta event counter does not match its events".into(),
        ));
    }
    if commit.journal.tenant != commit.delta.tenant
        || commit.journal.revision != commit.delta.new_revision
    {
        return Err(StoreError::Invalid(
            "journal entry does not describe this delta".into(),
        ));
    }
    Ok(())
}

/// Whether a record answers a `ByField` need.
pub fn field_matches(record: &Record, field: &strata_kernel::FieldId, value: &Value) -> bool {
    match record.get(field) {
        Value::List(items) => items.contains(value),
        other => other == value,
    }
}

/// Cut a complete tenant state down to the records that answer `needs`.
pub fn partial_state(tenant: &TenantState, needs: &[Need]) -> State {
    let mut thin = TenantState {
        records: Default::default(),
        ..tenant.clone()
    };
    for need in needs {
        let found: Vec<&Arc<Record>> = match need {
            Need::Record { object, id } => tenant.record(object, *id).into_iter().collect(),
            Need::AllOf(object) => tenant.records_of(object).collect(),
            Need::ByField {
                object,
                field,
                value,
            } => tenant
                .records_of(object)
                .filter(|record| field_matches(record, field, value))
                .collect(),
        };
        for record in found {
            thin.records
                .entry(record.object.clone())
                .or_default()
                .insert(record.id, record.clone());
        }
    }
    State {
        tenant_id: tenant.id.clone(),
        tenant: Some(Arc::new(thin)),
        coverage: Coverage::Partial(needs.iter().cloned().collect::<BTreeSet<_>>()),
    }
}

/// Why an intent or read did not go through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecuteError {
    /// The kernel rejected it. The store was not changed.
    Domain(DomainError),
    Store(StoreError),
}

impl fmt::Display for ExecuteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecuteError::Domain(e) => write!(f, "{e}"),
            ExecuteError::Store(e) => write!(f, "{e}"),
        }
    }
}

impl From<StoreError> for ExecuteError {
    fn from(e: StoreError) -> Self {
        ExecuteError::Store(e)
    }
}

/// How often a commit is retried after losing a race, and how many extra needs a single
/// decision may ask for.
const MAX_CONFLICT_RETRIES: usize = 8;
const MAX_NEED_ROUNDS: usize = 32;

/// What `execute` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Executed {
    pub transition: Transition,
    pub receipt: Receipt,
    /// Commits that lost a race and were decided again.
    pub conflicts: usize,
}

/// Decide an intent with the kernel against a snapshot loaded from the store, and commit it.
///
/// This is the whole of production command handling: load what the kernel says it needs, run
/// `transition`, commit atomically, and start over if another commit got there first. It holds
/// no domain logic of its own.
pub fn execute(
    store: &dyn Store,
    intent: &Intent,
    ctx: &Context,
) -> Result<Executed, ExecuteError> {
    let mut conflicts = 0;
    loop {
        let header = store.load(&ctx.tenant, &[])?;
        let mut needs =
            strata_kernel::needs(header.tenant.as_deref().map(|t| t.schema.as_ref()), intent);
        let mut rounds = 0;
        let transition = loop {
            let state = store.load(&ctx.tenant, &needs)?;
            match strata_kernel::transition(&state, intent, ctx) {
                Err(DomainError {
                    kind: ErrorKind::InsufficientState,
                    missing: Some(need),
                    ..
                }) if rounds < MAX_NEED_ROUNDS && !needs.contains(&need) => {
                    needs.push(need);
                    rounds += 1;
                }
                Err(error) => return Err(ExecuteError::Domain(error)),
                Ok(transition) => break transition,
            }
        };
        match store.commit(&Commit::of(intent, ctx, &transition)) {
            Ok(receipt) => {
                return Ok(Executed {
                    transition,
                    receipt,
                    conflicts,
                })
            }
            Err(StoreError::Conflict { .. }) if conflicts < MAX_CONFLICT_RETRIES => conflicts += 1,
            Err(error) => return Err(error.into()),
        }
    }
}

/// Answer a read with the kernel against a snapshot loaded from the store.
pub fn query(
    store: &dyn Store,
    request: &ReadRequest,
    ctx: &Context,
) -> Result<ReadResult, ExecuteError> {
    let header = store.load(&ctx.tenant, &[])?;
    let Some(tenant) = header.tenant.as_deref() else {
        return strata_kernel::read(&header, request, ctx).map_err(ExecuteError::Domain);
    };
    let needs = strata_kernel::read_needs(&tenant.schema, request);
    let state = store.load(&ctx.tenant, &needs)?;
    strata_kernel::read(&state, request, ctx).map_err(ExecuteError::Domain)
}
