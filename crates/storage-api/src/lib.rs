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
//! - **Ordering.** Events have a per-tenant sequence that continues without gaps, and an outbox
//!   position of their tenant that follows that tenant's commit order. The outboxes of different
//!   tenants are not ordered against each other: each tenant may live in a database of its own.
//! - **Publish after commit.** An event is in the outbox only once the commit that wrote it is
//!   applied, and stays there until it is marked published for its tenant.
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
use strata_kernel::ext::*;
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
    /// Outbox position, in the tenant's own outbox, of the last event written, if the commit had
    /// events.
    pub last_position: Option<u64>,
}

/// An event waiting to be handed to subscribers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxEntry {
    /// Position in the outbox of the event's tenant, increasing in that tenant's commit order.
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
    /// When the person last proved who they are for this session: at sign-in, and again at each
    /// step-up. A sensitive operation needs this to be recent (IDN-019).
    pub authenticated_at: Timestamp,
    /// A keyed hash of the network context that authenticated the session, and never the address
    /// itself (D-704, PRV-364). A session that appears from another context is a new arrival
    /// (IDN-260).
    #[serde(default)]
    pub network: Option<String>,
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
    /// The e-mail address of the person, once the identity service has verified that the person
    /// controls it. `None` until then: an account with none redeems no invitation (ADM-153).
    pub verified_email: Option<String>,
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

/// A passkey a person registered: the relying party's record of one WebAuthn credential. The
/// credential itself (public key, signature counter, flags) is the WebAuthn library's own
/// serialisation, kept as it is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PasskeyRecord {
    pub tenant: TenantId,
    pub principal: PrincipalId,
    /// The credential id, base64url without padding. Unique within the tenant.
    pub id: String,
    /// The relying party the credential was made for: the host of the tenant's address. It is
    /// used at that address only.
    pub rp_id: String,
    /// What the person calls it.
    pub label: String,
    /// The library's serialised credential.
    pub credential: String,
    pub created_at: Timestamp,
    pub last_used_at: Option<Timestamp>,
}

/// What kind of ceremony a challenge was issued for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PasskeyCeremony {
    Register,
    SignIn,
}

/// A challenge the server issued and has not yet seen answered. It is held server-side, bound to
/// the address it was requested at (and for a registration to the session that asked), is
/// answered once, and lapses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PasskeyChallenge {
    pub tenant: TenantId,
    /// A hash of the identifier the browser holds, so that the store holds no usable one.
    pub id_hash: String,
    pub ceremony: PasskeyCeremony,
    /// The session that asked for a registration.
    pub session: Option<SessionId>,
    pub principal: Option<PrincipalId>,
    /// The origin the ceremony was requested at, such as `https://tidewater.example`.
    pub origin: String,
    /// The library's serialised ceremony state.
    pub state: String,
    pub expires_at: Timestamp,
}

/// An emailed sign-in link that has been sent and not yet used. The row holds the principal and
/// no email address (PRV-365), the hash of the token and the hash of the binding nonce of the
/// browser that asked, so the store holds nothing that opens the link.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInLink {
    pub tenant: TenantId,
    /// A hash of the challenge identifier the link carries.
    pub id_hash: String,
    pub principal: PrincipalId,
    /// A hash of the single-use token the link carries.
    pub token_hash: String,
    /// A hash of the nonce kept in the requesting browser's cookie.
    pub binding_hash: String,
    /// The origin the link was requested at and points to.
    pub origin: String,
    /// The link is accepted through this time.
    pub expires_at: Timestamp,
}

/// A sign-in at an external identity provider that has been started and not yet answered. The
/// row is held server-side, found by the hash of the `state` value the browser returns with, is
/// answered once, and lapses. It holds what the callback must check: the nonce the identity token
/// must carry, the proof-key verifier the code is redeemed with, the hash of the nonce kept in
/// the starting browser's cookie, and the address the sign-in started at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OidcFlow {
    pub tenant: TenantId,
    /// A hash of the `state` value.
    pub id_hash: String,
    /// The identifier of the provider in the tenant's configuration.
    pub provider: String,
    /// The nonce the identity token must carry.
    pub nonce: String,
    /// The proof-key verifier the authorisation code is redeemed with.
    pub verifier: String,
    /// A hash of the nonce kept in the cookie of the browser that started the sign-in.
    pub binding_hash: String,
    /// The origin the sign-in started at, and the one its callback must come back to.
    pub origin: String,
    /// The principal that was signed in when it started, when the sign-in is to link the
    /// identity to that principal's account (FED-012).
    pub principal: Option<PrincipalId>,
    pub expires_at: Timestamp,
}

/// A sign-in at a SAML identity provider that has been started and not yet answered. The row is
/// held server-side, found by the hash of the `RelayState` value the response returns with, is
/// answered once, and lapses. It holds what the response must be checked against: the identifier
/// of the authentication request it must answer, the hash of the nonce kept in the starting
/// browser's cookie, and the address the sign-in started at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SamlRequest {
    pub tenant: TenantId,
    /// A hash of the `RelayState` value.
    pub id_hash: String,
    /// The identifier of the provider in the tenant's configuration.
    pub provider: String,
    /// The `ID` of the authentication request, which the response's `InResponseTo` must name.
    pub request_id: String,
    /// A hash of the nonce kept in the cookie of the browser that started the sign-in.
    pub binding_hash: String,
    /// The origin the sign-in started at, and the one its assertion consumer address is at.
    pub origin: String,
    /// The principal that was signed in when it started, when the sign-in is to link the
    /// identity to that principal's account (FED-012).
    pub principal: Option<PrincipalId>,
    pub expires_at: Timestamp,
}

/// A person at an external identity provider, tied to the principal they sign in as. Keyed by
/// the tenant, the provider of its configuration and the provider's subject identifier, so that
/// one provider's subject is never another provider's (FED-256).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederatedIdentity {
    pub tenant: TenantId,
    pub provider: String,
    pub subject: String,
    pub principal: PrincipalId,
    pub created_at: Timestamp,
}

/// A time-based one-time password factor of one person (RFC 6238). The secret has to be read back
/// to check a code, so it cannot be hashed; it is sealed with an authenticated cipher under a key
/// the store never sees, and bound to the tenant and the person.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TotpRecord {
    pub tenant: TenantId,
    pub principal: PrincipalId,
    /// The sealed secret, as the backend wrote it. Opaque to the store.
    pub secret: String,
    /// Whether the person has proved they hold the secret with a code. An unconfirmed
    /// enrolment is no factor yet.
    pub confirmed: bool,
    /// The time step of the last code accepted, so that a code is accepted once; `-1` when none.
    pub last_step: i64,
    pub created_at: Timestamp,
}

/// A sign-in whose first factor was right and which waits for the second. It is held server-side,
/// found by the hash of the identifier the browser holds, and it lapses. It is the only thing a
/// first factor alone yields (IDN-018).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingSignIn {
    pub tenant: TenantId,
    /// A hash of the identifier the browser holds.
    pub id_hash: String,
    pub principal: PrincipalId,
    /// The method that proved the first factor.
    pub method: String,
    /// How many factors that method proved.
    pub factors: u8,
    /// How many presentations of a second factor have been counted against this sign-in.
    pub attempts: u32,
    pub expires_at: Timestamp,
}

/// An account recovery that has been asked for and not yet completed. It holds the hashes of two
/// tokens: the one that completes it once the delay has passed, and the one that cancels it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryRequest {
    pub tenant: TenantId,
    /// A hash of the identifier the links carry.
    pub id_hash: String,
    pub principal: PrincipalId,
    /// The login whose password the recovery sets: the one the recovery was asked for.
    pub login: String,
    pub token_hash: String,
    pub cancel_hash: String,
    pub requested_at: Timestamp,
    /// The recovery may complete from this time (D-700: twenty-four hours after the request).
    pub completes_at: Timestamp,
    pub expires_at: Timestamp,
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

    /// At most `limit` events not yet marked published, from every tenant's outbox: each tenant's
    /// in its commit order, the tenants in no promised order against each other.
    fn outbox(&self, limit: usize) -> StoreResult<Vec<OutboxEntry>>;

    /// Mark every outbox entry of `tenant` up to and including `position` as published. Other
    /// tenants' outboxes are untouched.
    fn mark_published(&self, tenant: &TenantId, position: u64) -> StoreResult<()>;

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
    /// The login of a principal, for a sign-in method that identifies the person by something
    /// other than a login.
    fn account_of(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<Option<Account>>;

    /// Keep a registered passkey. Returns `false`, and keeps nothing, when the tenant already
    /// holds a credential with that id.
    fn put_passkey(&self, passkey: &PasskeyRecord) -> StoreResult<bool>;
    /// One credential of a tenant, by its id.
    fn passkey(&self, tenant: &TenantId, id: &str) -> StoreResult<Option<PasskeyRecord>>;
    /// The passkeys of one principal, oldest first.
    fn passkeys(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<Vec<PasskeyRecord>>;
    /// Replace the stored credential after a sign-in moved its counter, and note when it was
    /// used. Returns whether the credential was there.
    fn update_passkey(
        &self,
        tenant: &TenantId,
        id: &str,
        credential: &str,
        used_at: Timestamp,
    ) -> StoreResult<bool>;
    /// Remove one of a principal's passkeys. Returns whether it was there.
    fn remove_passkey(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        id: &str,
    ) -> StoreResult<bool>;
    /// Keep an issued challenge.
    fn put_passkey_challenge(&self, challenge: &PasskeyChallenge) -> StoreResult<()>;
    /// Take a challenge, removing it in the same step, so that it answers once whatever the
    /// timing. A challenge that has lapsed at `now` is removed and not returned.
    fn take_passkey_challenge(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        now: Timestamp,
    ) -> StoreResult<Option<PasskeyChallenge>>;
    /// Remove challenges that have lapsed at `now`. Returns how many were removed.
    fn purge_passkey_challenges(&self, now: Timestamp) -> StoreResult<u64>;

    /// Keep a sent sign-in link.
    fn put_sign_in_link(&self, link: &SignInLink) -> StoreResult<()>;
    /// Take a link, removing it in the same step, so that it is answered once whatever the
    /// timing. The row is returned whether or not it has lapsed: the caller compares the time.
    fn take_sign_in_link(
        &self,
        tenant: &TenantId,
        id_hash: &str,
    ) -> StoreResult<Option<SignInLink>>;
    /// Remove links that have lapsed before `now`. Returns how many were removed.
    fn purge_sign_in_links(&self, now: Timestamp) -> StoreResult<u64>;

    /// Keep a started identity-provider sign-in.
    fn put_oidc_flow(&self, flow: &OidcFlow) -> StoreResult<()>;
    /// Take a started sign-in, removing it in the same step, so that a callback answers it once
    /// whatever the timing. The row is returned whether or not it has lapsed: the caller compares
    /// the time.
    fn take_oidc_flow(&self, tenant: &TenantId, id_hash: &str) -> StoreResult<Option<OidcFlow>>;
    /// Remove started sign-ins that have lapsed before `now`. Returns how many were removed.
    fn purge_oidc_flows(&self, now: Timestamp) -> StoreResult<u64>;

    /// Keep a started SAML sign-in.
    fn put_saml_request(&self, request: &SamlRequest) -> StoreResult<()>;
    /// Take a started SAML sign-in, removing it in the same step, so that a response answers it
    /// once whatever the timing. The row is returned whether or not it has lapsed: the caller
    /// compares the time.
    fn take_saml_request(
        &self,
        tenant: &TenantId,
        id_hash: &str,
    ) -> StoreResult<Option<SamlRequest>>;
    /// Remove started SAML sign-ins that have lapsed before `now`. Returns how many were removed.
    fn purge_saml_requests(&self, now: Timestamp) -> StoreResult<u64>;

    /// Remember that an assertion was accepted, by the hash of its identifier, until `expires_at`.
    /// Returns `false`, and changes nothing, when the tenant already remembers an assertion of
    /// that provider with that identifier: in one step, so that two presentations at once are one.
    fn put_saml_assertion(
        &self,
        tenant: &TenantId,
        provider: &str,
        id_hash: &str,
        expires_at: Timestamp,
    ) -> StoreResult<bool>;
    /// Forget assertions that would no longer be accepted at `now` whatever is remembered.
    /// Returns how many were forgotten.
    fn purge_saml_assertions(&self, now: Timestamp) -> StoreResult<u64>;

    /// Tie a provider's subject to a principal. Returns `false`, and keeps nothing, when the
    /// tenant already ties that provider's subject to a principal.
    fn put_identity(&self, identity: &FederatedIdentity) -> StoreResult<bool>;
    /// The principal a provider's subject is tied to.
    fn identity(
        &self,
        tenant: &TenantId,
        provider: &str,
        subject: &str,
    ) -> StoreResult<Option<FederatedIdentity>>;

    /// Keep a TOTP enrolment that is not confirmed. Returns `false`, and keeps nothing, when the
    /// person already holds a confirmed one: a confirmed factor is removed before it is replaced.
    fn put_totp(&self, totp: &TotpRecord) -> StoreResult<bool>;
    /// The person's TOTP factor, confirmed or not.
    fn totp(&self, tenant: &TenantId, principal: &PrincipalId) -> StoreResult<Option<TotpRecord>>;
    /// Confirm an enrolment with the time step of the code that proved it. Returns whether an
    /// unconfirmed enrolment was there.
    fn confirm_totp(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        step: i64,
    ) -> StoreResult<bool>;
    /// Accept a code of this time step, once: returns `true` and remembers the step only when it
    /// is later than every step accepted before, in one atomic step, so that a code that was
    /// used is refused whatever the timing (replay).
    fn use_totp_step(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        step: i64,
    ) -> StoreResult<bool>;
    /// Remove the person's TOTP factor. Returns whether it was there.
    fn remove_totp(&self, tenant: &TenantId, principal: &PrincipalId) -> StoreResult<bool>;

    /// Replace the person's recovery codes with these hashes, in one step.
    fn replace_recovery_codes(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        hashes: &[String],
        at: Timestamp,
    ) -> StoreResult<()>;
    /// Spend one recovery code: removes it and returns `true` only for the call that removed it,
    /// so a code answers once whatever the timing.
    fn spend_recovery_code(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        hash: &str,
    ) -> StoreResult<bool>;
    /// How many recovery codes the person has left.
    fn recovery_codes_left(&self, tenant: &TenantId, principal: &PrincipalId) -> StoreResult<u32>;

    /// Keep a sign-in that waits for its second factor.
    fn put_pending(&self, pending: &PendingSignIn) -> StoreResult<()>;
    /// Count one presentation against a waiting sign-in and return it, in one atomic step. A
    /// sign-in that has lapsed, or that has already had `max` presentations counted, is removed
    /// and not returned.
    fn attempt_pending(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        now: Timestamp,
        max: u32,
    ) -> StoreResult<Option<PendingSignIn>>;
    /// Take a waiting sign-in, removing it in the same step, so that it yields one session
    /// whatever the timing. A lapsed one is removed and not returned.
    fn take_pending(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        now: Timestamp,
    ) -> StoreResult<Option<PendingSignIn>>;

    /// Keep an account recovery that was asked for. It replaces an earlier one of the same person.
    fn put_recovery(&self, request: &RecoveryRequest) -> StoreResult<()>;
    /// An account recovery, left in place.
    fn recovery(&self, tenant: &TenantId, id_hash: &str) -> StoreResult<Option<RecoveryRequest>>;
    /// Take an account recovery, removing it in the same step.
    fn take_recovery(
        &self,
        tenant: &TenantId,
        id_hash: &str,
    ) -> StoreResult<Option<RecoveryRequest>>;
    /// Cancel a recovery whose cancel token hashes to `cancel_hash`. Returns whether it was there.
    fn cancel_recovery(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        cancel_hash: &str,
    ) -> StoreResult<bool>;
    /// Remove waiting sign-ins and recoveries that have lapsed at `now`. Returns how many.
    fn purge_factor_challenges(&self, now: Timestamp) -> StoreResult<u64>;

    /// Keep a small document of the identity service under a kind and a key of one tenant: a
    /// client, a signing key, a grant, a provisioning token. `value` is JSON text the store does
    /// not read. With `create_only` the document is kept only when the key is free, in one step,
    /// and `false` says it was not; otherwise an existing document is replaced and the answer is
    /// `true`.
    fn doc_put(
        &self,
        tenant: &TenantId,
        kind: &str,
        key: &str,
        value: &str,
        create_only: bool,
    ) -> StoreResult<bool>;
    /// One document of a tenant.
    fn doc_get(&self, tenant: &TenantId, kind: &str, key: &str) -> StoreResult<Option<String>>;
    /// Every document of one kind of a tenant, by key.
    fn doc_list(&self, tenant: &TenantId, kind: &str) -> StoreResult<Vec<(String, String)>>;
    /// Remove one document. Returns whether it was there.
    fn doc_remove(&self, tenant: &TenantId, kind: &str, key: &str) -> StoreResult<bool>;

    /// The verified e-mail address of the account of `principal` in `tenant`, if it has an
    /// account and the account has one.
    fn verified_address(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<Option<String>>;

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
            ExecuteError::Domain(e) => f.write_str(&e.describe()),
            ExecuteError::Store(e) => write!(f, "{e}"),
        }
    }
}

impl From<StoreError> for ExecuteError {
    fn from(e: StoreError) -> Self {
        ExecuteError::Store(e)
    }
}

/// How often a commit is retried after losing a race, and how many rounds a single decision
/// may spend loading state the kernel found missing. Each round adds every need the kernel can
/// name at once, so the number of loads does not grow with the number of references a record
/// holds.
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
        let mut wanted: BTreeSet<Need> =
            strata_kernel::needs(header.tenant.as_deref().map(|t| t.schema.as_ref()), intent)
                .into_iter()
                .collect();
        let mut rounds = 0;
        let transition = loop {
            let list: Vec<Need> = wanted.iter().cloned().collect();
            let state = store.load(&ctx.tenant, &list)?;
            // What the loaded state itself says is still needed (the references and unique
            // values of a stored record), asked for in one reload.
            let more: Vec<Need> = strata_kernel::refine_needs(&state, intent)
                .into_iter()
                .filter(|need| !wanted.contains(need))
                .collect();
            if !more.is_empty() && rounds < MAX_NEED_ROUNDS {
                wanted.extend(more);
                rounds += 1;
                continue;
            }
            match strata_kernel::transition(&state, intent, ctx) {
                Err(DomainError {
                    kind: ErrorKind::InsufficientState,
                    missing: Some(need),
                    ..
                }) if rounds < MAX_NEED_ROUNDS && !wanted.contains(&need) => {
                    wanted.insert(need);
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
    let mut wanted: BTreeSet<Need> = strata_kernel::read_needs(&tenant.schema, request)
        .into_iter()
        .collect();
    let mut rounds = 0;
    loop {
        let list: Vec<Need> = wanted.iter().cloned().collect();
        let state = store.load(&ctx.tenant, &list)?;
        match strata_kernel::read(&state, request, ctx) {
            Err(DomainError {
                kind: ErrorKind::InsufficientState,
                missing: Some(need),
                ..
            }) if rounds < MAX_NEED_ROUNDS && !wanted.contains(&need) => {
                // A rule that follows a reference reaches a record per row: load the whole
                // object type it points into, so one round answers every row.
                wanted.insert(match need {
                    Need::Record { object, .. } => Need::AllOf(object),
                    other => other,
                });
                rounds += 1;
            }
            other => return other.map_err(ExecuteError::Domain),
        }
    }
}
