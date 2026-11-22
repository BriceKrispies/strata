//! The store a simulated deployment writes through: the in-memory adapter of the persistence
//! contract (`strata_storage_api::memory::MemoryStore`), behind a layer that injects the faults
//! a store, and the network to it, show in a deployment.
//!
//! The layer sits where a deployed process has its adapter: under the backend's sealing layer,
//! so every fault reaches the backend's own code exactly as an adapter's error would. It decides
//! nothing about the domain and changes nothing it passes through. Each call that can fail is
//! decided from the seed, the kind of fault and how many calls of that kind came before, so the
//! same scenario meets the same faults at the same calls.
//!
//! Faults, each as it happens in a deployment:
//!
//! - **commit conflict**: the store refuses a commit because the tenant moved under it (a lost
//!   race). Nothing is written. The backend's `execute` decides again on fresh state.
//! - **commit unavailable**: the process or its database stops before the write. Nothing is
//!   written and the caller is told the store is unavailable.
//! - **commit reply lost**: the write is durable, then the process stops before it answers (or
//!   the answer is lost on the way). The caller is told the store is unavailable although the
//!   change was made.
//! - **report lost**: a worker stalls between running a job and recording its outcome, and
//!   later resumes and sends the report it held ([`FaultyStore::take_held_reports`]).
//! - **lease partition**: a worker cannot reach the tenant's database when it asks for a lease.
//! - **publish unavailable**: the publisher cannot read the outbox.
//! - **publish acknowledgement lost**: the publisher handed events on, then its mark of them as
//!   published was lost, so they are offered again.

use crate::engine::decide;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use strata_kernel::segments as seg;
use strata_kernel::{
    DeltaOp, DomainEvent, Need, PrincipalId, Record, SessionId, SettingId, TenantId, TenantState,
    Timestamp, Value, WorkflowId,
};
use strata_storage_api::jobs::{
    ExceptionKind, ExceptionRecord, Job, JobChange, JobReport, Lease, NewJob, Reported, RunRecord,
    RunState, Settled, StepRecord, WorkerWrites,
};
use strata_storage_api::*;

/// Store faults, each in events per million calls of its kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreFaults {
    pub commit_conflict: u32,
    pub commit_unavailable: u32,
    pub commit_reply_lost: u32,
    pub report_lost: u32,
    pub lease_partition: u32,
    pub publish_unavailable: u32,
    pub publish_ack_lost: u32,
    /// A scripted stop: the process stops before the write of this commit, counted from one
    /// over every commit the store is asked to make while these faults apply.
    #[serde(default)]
    pub stop_before_commit: Option<u64>,
    /// A scripted crash: the process stops right after the write of this commit, counted as
    /// `stop_before_commit` is. The commit is durable and its caller hears nothing; the process
    /// then does nothing more (every later write is refused, and so is a removal of a tenant)
    /// until the faults are set again, which is the restart.
    #[serde(default)]
    pub stop_after_commit: Option<u64>,
}

/// The kinds of store fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreFault {
    CommitConflict,
    CommitUnavailable,
    CommitReplyLost,
    ReportLost,
    LeasePartition,
    PublishUnavailable,
    PublishAckLost,
}

impl StoreFault {
    /// The kind as the decision numbers it (the engine's own kinds are below 16).
    fn kind(self) -> u64 {
        16 + self as u64
    }

    fn rate(self, faults: &StoreFaults) -> u32 {
        match self {
            StoreFault::CommitConflict => faults.commit_conflict,
            StoreFault::CommitUnavailable => faults.commit_unavailable,
            StoreFault::CommitReplyLost => faults.commit_reply_lost,
            StoreFault::ReportLost => faults.report_lost,
            StoreFault::LeasePartition => faults.lease_partition,
            StoreFault::PublishUnavailable => faults.publish_unavailable,
            StoreFault::PublishAckLost => faults.publish_ack_lost,
        }
    }
}

/// A report a stalled worker holds, to be sent when it resumes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldReport {
    pub tenant: TenantId,
    pub job: String,
    pub token: u64,
    pub report: JobReport,
    pub writes: WorkerWrites,
    /// When the worker stalled.
    pub at: Timestamp,
}

/// A fault that was injected: which, at which call of its kind, and when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Injected {
    pub fault: StoreFault,
    pub call: u64,
    pub at: i64,
}

/// A clock the layer stamps injected faults and held reports with.
pub type LayerClock = Arc<dyn Fn() -> Timestamp + Send + Sync>;

/// The in-memory adapter behind a fault-injecting layer.
pub struct FaultyStore {
    inner: Arc<dyn Store>,
    seed: u64,
    faults: Mutex<StoreFaults>,
    armed: AtomicBool,
    /// Calls so far per fault kind: the subject of each decision.
    calls: Mutex<[u64; 7]>,
    /// Commits that were written, whatever was answered.
    commits: AtomicU64,
    /// Commits asked for since the faults were last set.
    attempts: AtomicU64,
    held: Mutex<Vec<HeldReport>>,
    injected: Mutex<Vec<Injected>>,
    clock: LayerClock,
    /// Whether the process was stopped by `stop_after_commit` and has not been started again.
    halted: AtomicBool,
    /// Tenants an install announced whose creating commit was not yet made, and tenants whose
    /// install is not finished. A persistent adapter keeps the second in its directory; the
    /// in-memory adapter keeps nothing, so the layer stands in for it: a restart rolls them back.
    announced: Mutex<BTreeSet<TenantId>>,
    provisioning: Mutex<BTreeSet<TenantId>>,
}

impl FaultyStore {
    /// `inner` behind the layer, disarmed: nothing is injected until [`FaultyStore::arm`].
    pub fn new(inner: Arc<dyn Store>, seed: u64, faults: StoreFaults, clock: LayerClock) -> Self {
        FaultyStore {
            inner,
            seed,
            faults: Mutex::new(faults),
            armed: AtomicBool::new(false),
            calls: Mutex::new([0; 7]),
            commits: AtomicU64::new(0),
            attempts: AtomicU64::new(0),
            held: Mutex::new(Vec::new()),
            injected: Mutex::new(Vec::new()),
            clock,
            halted: AtomicBool::new(false),
            announced: Mutex::new(BTreeSet::new()),
            provisioning: Mutex::new(BTreeSet::new()),
        }
    }

    /// Inject `faults` from now on.
    pub fn set_faults(&self, faults: StoreFaults) {
        if let Ok(mut slot) = self.faults.lock() {
            *slot = faults;
        }
        self.attempts.store(0, Ordering::SeqCst);
        self.halted.store(false, Ordering::SeqCst);
    }

    fn stopped() -> StoreError {
        StoreError::Unavailable("simulated: the process is stopped".into())
    }

    /// Start or stop injecting faults.
    pub fn arm(&self, armed: bool) {
        self.armed.store(armed, Ordering::SeqCst);
    }

    /// The store behind the layer.
    pub fn inner(&self) -> &Arc<dyn Store> {
        &self.inner
    }

    /// How many commits were written so far.
    pub fn commits(&self) -> u64 {
        self.commits.load(Ordering::SeqCst)
    }

    /// The faults injected so far, in order.
    pub fn injected(&self) -> Vec<Injected> {
        self.injected
            .lock()
            .map(|list| list.clone())
            .unwrap_or_default()
    }

    /// The reports stalled workers hold that are due to be sent at `now`, those held for at least
    /// `stall_ms`, oldest first. They are removed from the layer.
    pub fn take_held_reports(&self, now: Timestamp, stall_ms: i64) -> Vec<HeldReport> {
        let Ok(mut held) = self.held.lock() else {
            return Vec::new();
        };
        let (due, kept): (Vec<HeldReport>, Vec<HeldReport>) = held
            .drain(..)
            .partition(|report| report.at.0.saturating_add(stall_ms) <= now.0);
        *held = kept;
        due
    }

    /// Whether any report is still held.
    pub fn holds_reports(&self) -> bool {
        self.held
            .lock()
            .map(|held| !held.is_empty())
            .unwrap_or(false)
    }

    /// Whether to inject `fault` at this call. Every call of a kind counts, armed or not, so that
    /// the decisions do not depend on when the layer was armed.
    fn hit(&self, fault: StoreFault) -> bool {
        let call = {
            let Ok(mut calls) = self.calls.lock() else {
                return false;
            };
            let slot = &mut calls[fault as usize];
            *slot += 1;
            *slot
        };
        let rate = self
            .faults
            .lock()
            .map(|faults| fault.rate(&faults))
            .unwrap_or(0);
        if rate == 0 || !self.armed.load(Ordering::SeqCst) {
            return false;
        }
        let hit = decide(self.seed, fault.kind(), call, 0, rate);
        if hit {
            if let Ok(mut injected) = self.injected.lock() {
                injected.push(Injected {
                    fault,
                    call,
                    at: (self.clock)().0,
                });
            }
        }
        hit
    }

    /// A commit that was written: if it created a tenant whose install was announced, the tenant
    /// is now `provisioning`, as the directory of a persistent adapter would have it.
    fn note_creation(&self, commit: &Commit) {
        if !commit
            .delta
            .ops
            .iter()
            .any(|op| matches!(op, DeltaOp::CreateTenant { .. }))
        {
            return;
        }
        let tenant = &commit.delta.tenant;
        let announced = self
            .announced
            .lock()
            .map(|mut set| set.remove(tenant))
            .unwrap_or(false);
        if announced {
            if let Ok(mut set) = self.provisioning.lock() {
                set.insert(tenant.clone());
            }
        }
    }

    /// An install of `tenant` is over, finished or removed: nothing of it is pending any more.
    fn forget_install(&self, tenant: &TenantId) {
        if let Ok(mut set) = self.announced.lock() {
            set.remove(tenant);
        }
        if let Ok(mut set) = self.provisioning.lock() {
            set.remove(tenant);
        }
    }

    /// A commit made by `write`, with the commit faults around it.
    fn commit_through(&self, write: impl FnOnce() -> StoreResult<Receipt>) -> StoreResult<Receipt> {
        if self.halted.load(Ordering::SeqCst) {
            return Err(Self::stopped());
        }
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
        let scripted = self
            .faults
            .lock()
            .map(|faults| faults.stop_before_commit == Some(attempt))
            .unwrap_or(false);
        if scripted && self.armed.load(Ordering::SeqCst) {
            if let Ok(mut injected) = self.injected.lock() {
                injected.push(Injected {
                    fault: StoreFault::CommitUnavailable,
                    call: attempt,
                    at: (self.clock)().0,
                });
            }
            return Err(StoreError::Unavailable(
                "simulated: the process stopped before the write; nothing was committed".into(),
            ));
        }
        if self.hit(StoreFault::CommitConflict) {
            return Err(StoreError::Conflict {
                expected: None,
                found: None,
                found_generation: None,
            });
        }
        if self.hit(StoreFault::CommitUnavailable) {
            return Err(StoreError::Unavailable(
                "simulated: the process stopped before the write; nothing was committed".into(),
            ));
        }
        let receipt = write()?;
        self.commits.fetch_add(1, Ordering::SeqCst);
        let crash = self
            .faults
            .lock()
            .map(|faults| faults.stop_after_commit == Some(attempt))
            .unwrap_or(false);
        if crash && self.armed.load(Ordering::SeqCst) {
            self.halted.store(true, Ordering::SeqCst);
            if let Ok(mut injected) = self.injected.lock() {
                injected.push(Injected {
                    fault: StoreFault::CommitReplyLost,
                    call: attempt,
                    at: (self.clock)().0,
                });
            }
            return Err(StoreError::Unavailable(
                "simulated: the process stopped after the write; the commit was made".into(),
            ));
        }
        if self.hit(StoreFault::CommitReplyLost) {
            return Err(StoreError::Unavailable(
                "simulated: the commit was made and its answer was lost".into(),
            ));
        }
        Ok(receipt)
    }
}

impl Store for FaultyStore {
    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    fn health(&self) -> StoreResult<Health> {
        self.inner.health()
    }

    fn liveness(&self) -> StoreResult<()> {
        self.inner.liveness()
    }

    fn migrations(&self) -> StoreResult<Vec<StorageMigration>> {
        self.inner.migrations()
    }

    fn tenants(&self) -> StoreResult<Vec<TenantSummary>> {
        self.inner.tenants()
    }

    fn tenant_summary(&self, tenant: &TenantId) -> StoreResult<Option<TenantSummary>> {
        self.inner.tenant_summary(tenant)
    }

    fn object_revisions(
        &self,
        tenant: &TenantId,
        objects: &[strata_kernel::ObjectTypeId],
    ) -> StoreResult<std::collections::BTreeMap<strata_kernel::ObjectTypeId, u64>> {
        self.inner.object_revisions(tenant, objects)
    }

    fn tenant_plan(&self, tenant: &TenantId) -> StoreResult<Option<String>> {
        self.inner.tenant_plan(tenant)
    }

    fn set_tenant_plan(&self, tenant: &TenantId, plan: &str) -> StoreResult<()> {
        self.inner.set_tenant_plan(tenant, plan)
    }

    fn stored_bytes(&self, tenant: &TenantId) -> StoreResult<u64> {
        self.inner.stored_bytes(tenant)
    }

    fn load_at(
        &self,
        tenant: &TenantId,
        needs: &[Need],
    ) -> StoreResult<strata_storage_api::Loaded> {
        self.inner.load_at(tenant, needs)
    }

    fn load_complete_at(&self, tenant: &TenantId) -> StoreResult<strata_storage_api::Loaded> {
        self.inner.load_complete_at(tenant)
    }

    fn generation(&self, tenant: &TenantId) -> StoreResult<Option<strata_storage_api::Generation>> {
        self.inner.generation(tenant)
    }

    fn rotate_nonce(&self, tenant: &TenantId) -> StoreResult<strata_storage_api::Generation> {
        self.inner.rotate_nonce(tenant)
    }

    fn commit(&self, commit: &Commit) -> StoreResult<Receipt> {
        self.commit_through(|| {
            let receipt = self.inner.commit(commit)?;
            self.note_creation(commit);
            Ok(receipt)
        })
    }

    fn idempotent_result(
        &self,
        tenant: &TenantId,
        principal: &str,
        key: &str,
        now: Timestamp,
    ) -> StoreResult<Option<IdempotentResult>> {
        self.inner.idempotent_result(tenant, principal, key, now)
    }

    fn events_after(
        &self,
        tenant: &TenantId,
        after: u64,
        limit: usize,
    ) -> StoreResult<Vec<DomainEvent>> {
        self.inner.events_after(tenant, after, limit)
    }

    fn outbox(&self, limit: usize) -> StoreResult<Vec<OutboxEntry>> {
        if self.hit(StoreFault::PublishUnavailable) {
            return Err(StoreError::Unavailable(
                "simulated: the outbox cannot be read".into(),
            ));
        }
        self.inner.outbox(limit)
    }

    fn outbox_faults(&self) -> Vec<OutboxFault> {
        self.inner.outbox_faults()
    }

    fn outbox_pending_of(&self, tenant: &TenantId) -> StoreResult<u64> {
        self.inner.outbox_pending_of(tenant)
    }

    fn mark_published(&self, tenant: &TenantId, position: u64) -> StoreResult<()> {
        if self.hit(StoreFault::PublishAckLost) {
            return Err(StoreError::Unavailable(
                "simulated: the mark of the published events was lost".into(),
            ));
        }
        self.inner.mark_published(tenant, position)
    }

    fn journal_after(
        &self,
        tenant: &TenantId,
        after: u64,
        limit: usize,
    ) -> StoreResult<Vec<JournalEntry>> {
        self.inner.journal_after(tenant, after, limit)
    }

    fn journal_count(&self, tenant: &TenantId) -> StoreResult<u64> {
        self.inner.journal_count(tenant)
    }

    fn put_session(&self, session: &SessionRecord) -> StoreResult<()> {
        self.inner.put_session(session)
    }

    fn session_seen(&self, id: &SessionId, at: Timestamp) -> StoreResult<()> {
        self.inner.session_seen(id, at)
    }

    fn session_authenticated(
        &self,
        id: &SessionId,
        at: Timestamp,
        network: Option<&str>,
    ) -> StoreResult<()> {
        self.inner.session_authenticated(id, at, network)
    }

    fn session_by_token(&self, token_hash: &str) -> StoreResult<Option<SessionRecord>> {
        self.inner.session_by_token(token_hash)
    }

    fn session(&self, id: &SessionId) -> StoreResult<Option<SessionRecord>> {
        self.inner.session(id)
    }

    fn live_sessions(&self, now: Timestamp) -> StoreResult<Vec<SessionRecord>> {
        self.inner.live_sessions(now)
    }

    fn live_sessions_of(
        &self,
        tenant: &TenantId,
        principal: Option<&PrincipalId>,
        now: Timestamp,
    ) -> StoreResult<Vec<SessionRecord>> {
        self.inner.live_sessions_of(tenant, principal, now)
    }

    fn purge_sessions(&self, before: Timestamp) -> StoreResult<u64> {
        self.inner.purge_sessions(before)
    }

    fn throttle_reserve(
        &self,
        tenant: &TenantId,
        rules: &[ThrottleRule],
        now: Timestamp,
    ) -> StoreResult<ThrottleVerdict> {
        self.inner.throttle_reserve(tenant, rules, now)
    }

    fn throttle_settle(
        &self,
        tenant: &TenantId,
        key: &str,
        max: u32,
        how: ThrottleSettle,
    ) -> StoreResult<()> {
        self.inner.throttle_settle(tenant, key, max, how)
    }

    fn purge_throttle(&self, now: Timestamp) -> StoreResult<u64> {
        self.inner.purge_throttle(now)
    }

    fn put_account(&self, account: &Account) -> StoreResult<()> {
        self.inner.put_account(account)
    }

    fn create_account(&self, account: &Account) -> StoreResult<bool> {
        self.inner.create_account(account)
    }

    fn update_account(&self, expected: &Account, replacement: &Account) -> StoreResult<bool> {
        self.inner.update_account(expected, replacement)
    }

    fn account(&self, tenant: &TenantId, login: &str) -> StoreResult<Option<Account>> {
        self.inner.account(tenant, login)
    }

    fn remove_account(&self, tenant: &TenantId, login: &str) -> StoreResult<bool> {
        self.inner.remove_account(tenant, login)
    }

    fn remove_principal(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<PrincipalRemoval> {
        self.inner.remove_principal(tenant, principal)
    }

    fn anonymise_ledger_principal(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        expected: Option<&strata_storage_api::Generation>,
    ) -> StoreResult<strata_kernel::ledger::Anonymised> {
        self.inner
            .anonymise_ledger_principal(tenant, principal, expected)
    }

    fn account_of(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<Option<Account>> {
        self.inner.account_of(tenant, principal)
    }

    fn put_passkey(&self, passkey: &PasskeyRecord) -> StoreResult<bool> {
        self.inner.put_passkey(passkey)
    }

    fn passkey(&self, tenant: &TenantId, id: &str) -> StoreResult<Option<PasskeyRecord>> {
        self.inner.passkey(tenant, id)
    }

    fn passkeys(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<Vec<PasskeyRecord>> {
        self.inner.passkeys(tenant, principal)
    }

    fn update_passkey(
        &self,
        tenant: &TenantId,
        id: &str,
        credential: &str,
        used_at: Timestamp,
    ) -> StoreResult<bool> {
        self.inner.update_passkey(tenant, id, credential, used_at)
    }

    fn remove_passkey(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        id: &str,
    ) -> StoreResult<bool> {
        self.inner.remove_passkey(tenant, principal, id)
    }

    fn put_passkey_challenge(&self, challenge: &PasskeyChallenge) -> StoreResult<()> {
        self.inner.put_passkey_challenge(challenge)
    }

    fn take_passkey_challenge(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        now: Timestamp,
    ) -> StoreResult<Option<PasskeyChallenge>> {
        self.inner.take_passkey_challenge(tenant, id_hash, now)
    }

    fn purge_passkey_challenges(&self, now: Timestamp) -> StoreResult<u64> {
        self.inner.purge_passkey_challenges(now)
    }

    fn put_sign_in_link(&self, link: &SignInLink) -> StoreResult<()> {
        self.inner.put_sign_in_link(link)
    }

    fn take_sign_in_link(
        &self,
        tenant: &TenantId,
        id_hash: &str,
    ) -> StoreResult<Option<SignInLink>> {
        self.inner.take_sign_in_link(tenant, id_hash)
    }

    fn purge_sign_in_links(&self, now: Timestamp) -> StoreResult<u64> {
        self.inner.purge_sign_in_links(now)
    }

    fn put_oidc_flow(&self, flow: &OidcFlow) -> StoreResult<()> {
        self.inner.put_oidc_flow(flow)
    }

    fn take_oidc_flow(&self, tenant: &TenantId, id_hash: &str) -> StoreResult<Option<OidcFlow>> {
        self.inner.take_oidc_flow(tenant, id_hash)
    }

    fn purge_oidc_flows(&self, now: Timestamp) -> StoreResult<u64> {
        self.inner.purge_oidc_flows(now)
    }

    fn put_saml_request(&self, request: &SamlRequest) -> StoreResult<()> {
        self.inner.put_saml_request(request)
    }

    fn take_saml_request(
        &self,
        tenant: &TenantId,
        id_hash: &str,
    ) -> StoreResult<Option<SamlRequest>> {
        self.inner.take_saml_request(tenant, id_hash)
    }

    fn purge_saml_requests(&self, now: Timestamp) -> StoreResult<u64> {
        self.inner.purge_saml_requests(now)
    }

    fn put_saml_assertion(
        &self,
        tenant: &TenantId,
        provider: &str,
        id_hash: &str,
        expires_at: Timestamp,
    ) -> StoreResult<bool> {
        self.inner
            .put_saml_assertion(tenant, provider, id_hash, expires_at)
    }

    fn purge_saml_assertions(&self, now: Timestamp) -> StoreResult<u64> {
        self.inner.purge_saml_assertions(now)
    }

    fn put_identity(&self, identity: &FederatedIdentity) -> StoreResult<bool> {
        self.inner.put_identity(identity)
    }

    fn identity(
        &self,
        tenant: &TenantId,
        provider: &str,
        subject: &str,
    ) -> StoreResult<Option<FederatedIdentity>> {
        self.inner.identity(tenant, provider, subject)
    }

    fn put_totp(&self, totp: &TotpRecord) -> StoreResult<bool> {
        self.inner.put_totp(totp)
    }

    fn totp(&self, tenant: &TenantId, principal: &PrincipalId) -> StoreResult<Option<TotpRecord>> {
        self.inner.totp(tenant, principal)
    }

    fn confirm_totp(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        step: i64,
    ) -> StoreResult<bool> {
        self.inner.confirm_totp(tenant, principal, step)
    }

    fn use_totp_step(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        step: i64,
    ) -> StoreResult<bool> {
        self.inner.use_totp_step(tenant, principal, step)
    }

    fn remove_totp(&self, tenant: &TenantId, principal: &PrincipalId) -> StoreResult<bool> {
        self.inner.remove_totp(tenant, principal)
    }

    fn replace_recovery_codes(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        hashes: &[String],
        at: Timestamp,
    ) -> StoreResult<()> {
        self.inner
            .replace_recovery_codes(tenant, principal, hashes, at)
    }

    fn spend_recovery_code(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        hash: &str,
    ) -> StoreResult<bool> {
        self.inner.spend_recovery_code(tenant, principal, hash)
    }

    fn recovery_codes_left(&self, tenant: &TenantId, principal: &PrincipalId) -> StoreResult<u32> {
        self.inner.recovery_codes_left(tenant, principal)
    }

    fn put_pending(&self, pending: &PendingSignIn) -> StoreResult<()> {
        self.inner.put_pending(pending)
    }

    fn attempt_pending(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        now: Timestamp,
        max: u32,
    ) -> StoreResult<Option<PendingSignIn>> {
        self.inner.attempt_pending(tenant, id_hash, now, max)
    }

    fn take_pending(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        now: Timestamp,
    ) -> StoreResult<Option<PendingSignIn>> {
        self.inner.take_pending(tenant, id_hash, now)
    }

    fn put_recovery(&self, request: &RecoveryRequest) -> StoreResult<()> {
        self.inner.put_recovery(request)
    }

    fn recovery(&self, tenant: &TenantId, id_hash: &str) -> StoreResult<Option<RecoveryRequest>> {
        self.inner.recovery(tenant, id_hash)
    }

    fn take_recovery(
        &self,
        tenant: &TenantId,
        id_hash: &str,
    ) -> StoreResult<Option<RecoveryRequest>> {
        self.inner.take_recovery(tenant, id_hash)
    }

    fn cancel_recovery(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        cancel_hash: &str,
    ) -> StoreResult<bool> {
        self.inner.cancel_recovery(tenant, id_hash, cancel_hash)
    }

    fn purge_factor_challenges(&self, now: Timestamp) -> StoreResult<u64> {
        self.inner.purge_factor_challenges(now)
    }

    fn doc_put(
        &self,
        tenant: &TenantId,
        kind: &str,
        key: &str,
        value: &str,
        create_only: bool,
    ) -> StoreResult<bool> {
        self.inner.doc_put(tenant, kind, key, value, create_only)
    }

    fn doc_get(&self, tenant: &TenantId, kind: &str, key: &str) -> StoreResult<Option<String>> {
        self.inner.doc_get(tenant, kind, key)
    }

    fn doc_list(&self, tenant: &TenantId, kind: &str) -> StoreResult<Vec<(String, String)>> {
        self.inner.doc_list(tenant, kind)
    }

    fn doc_remove(&self, tenant: &TenantId, kind: &str, key: &str) -> StoreResult<bool> {
        self.inner.doc_remove(tenant, kind, key)
    }

    fn audit_append(&self, tenant: &TenantId, entry: &AuditEntry) -> StoreResult<()> {
        self.inner.audit_append(tenant, entry)
    }

    fn audit_list(
        &self,
        tenant: &TenantId,
        kind: Option<&str>,
        limit: usize,
    ) -> StoreResult<Vec<AuditEntry>> {
        self.inner.audit_list(tenant, kind, limit)
    }

    fn audit_chain(
        &self,
        tenant: &TenantId,
        after: u64,
        limit: usize,
    ) -> StoreResult<Vec<audit_chain::ChainedAuditEntry>> {
        self.inner.audit_chain(tenant, after, limit)
    }

    fn verified_address(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<Option<String>> {
        self.inner.verified_address(tenant, principal)
    }

    fn create_snapshot(
        &self,
        tenant: &TenantId,
        label: &str,
        at: Timestamp,
    ) -> StoreResult<SnapshotInfo> {
        self.inner.create_snapshot(tenant, label, at)
    }

    fn snapshots(&self, tenant: &TenantId) -> StoreResult<Vec<SnapshotInfo>> {
        self.inner.snapshots(tenant)
    }

    fn load_snapshot(&self, tenant: &TenantId, id: u64) -> StoreResult<TenantState> {
        self.inner.load_snapshot(tenant, id)
    }

    fn restore_snapshot(
        &self,
        tenant: &TenantId,
        id: u64,
        expected: Option<&strata_storage_api::Generation>,
    ) -> StoreResult<()> {
        self.inner.restore_snapshot(tenant, id, expected)
    }

    fn reset(&self) -> StoreResult<()> {
        self.inner.reset()
    }

    fn reset_tenant(&self, tenant: &TenantId) -> StoreResult<()> {
        if self.halted.load(Ordering::SeqCst) {
            return Err(Self::stopped());
        }
        self.forget_install(tenant);
        self.inner.reset_tenant(tenant)
    }

    fn jobs_create(
        &self,
        tenant: &TenantId,
        jobs: &[NewJob],
        now: Timestamp,
    ) -> StoreResult<Vec<String>> {
        self.inner.jobs_create(tenant, jobs, now)
    }

    fn job(&self, tenant: &TenantId, id: &str) -> StoreResult<Option<Job>> {
        self.inner.job(tenant, id)
    }

    fn jobs_list(
        &self,
        tenant: &TenantId,
        before: Option<u64>,
        limit: usize,
    ) -> StoreResult<Vec<Job>> {
        self.inner.jobs_list(tenant, before, limit)
    }

    fn job_lease(
        &self,
        tenant: &TenantId,
        worker: &str,
        now: Timestamp,
    ) -> StoreResult<Option<Lease>> {
        if self.hit(StoreFault::LeasePartition) {
            return Err(StoreError::Unavailable(format!(
                "simulated: the worker cannot reach the database of {tenant}"
            )));
        }
        self.inner.job_lease(tenant, worker, now)
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
        if self.hit(StoreFault::ReportLost) {
            if let Ok(mut held) = self.held.lock() {
                held.push(HeldReport {
                    tenant: tenant.clone(),
                    job: id.to_string(),
                    token,
                    report: report.clone(),
                    writes: writes.clone(),
                    at: now,
                });
            }
            return Err(StoreError::Unavailable(
                "simulated: the worker stalled before its report reached the store".into(),
            ));
        }
        self.inner
            .job_report(tenant, id, token, report, writes, now)
    }

    fn job_cancel(
        &self,
        tenant: &TenantId,
        id: &str,
        reason: &str,
        now: Timestamp,
    ) -> StoreResult<JobChange> {
        self.inner.job_cancel(tenant, id, reason, now)
    }

    fn job_retry(&self, tenant: &TenantId, id: &str, now: Timestamp) -> StoreResult<JobChange> {
        self.inner.job_retry(tenant, id, now)
    }

    fn jobs_due(&self, now: Timestamp, limit: usize) -> StoreResult<Vec<TenantId>> {
        self.inner.jobs_due(now, limit)
    }

    fn automation_write(
        &self,
        tenant: &TenantId,
        writes: &WorkerWrites,
        now: Timestamp,
    ) -> StoreResult<()> {
        self.inner.automation_write(tenant, writes, now)
    }

    fn automation_run(&self, tenant: &TenantId, run: &str) -> StoreResult<Option<RunRecord>> {
        self.inner.automation_run(tenant, run)
    }

    fn automation_runs(
        &self,
        tenant: &TenantId,
        automation: Option<&WorkflowId>,
        state: Option<RunState>,
        before: Option<u64>,
        limit: usize,
    ) -> StoreResult<Vec<RunRecord>> {
        self.inner
            .automation_runs(tenant, automation, state, before, limit)
    }

    fn automation_steps(
        &self,
        tenant: &TenantId,
        run: &str,
        after: Option<u64>,
        limit: usize,
    ) -> StoreResult<Vec<StepRecord>> {
        self.inner.automation_steps(tenant, run, after, limit)
    }

    fn automation_exceptions(
        &self,
        tenant: &TenantId,
        kind: Option<ExceptionKind>,
        unsettled: bool,
        before: Option<u64>,
        limit: usize,
    ) -> StoreResult<Vec<ExceptionRecord>> {
        self.inner
            .automation_exceptions(tenant, kind, unsettled, before, limit)
    }

    fn automation_exception_settle(
        &self,
        tenant: &TenantId,
        id: &str,
        release: bool,
        now: Timestamp,
    ) -> StoreResult<Settled> {
        self.inner
            .automation_exception_settle(tenant, id, release, now)
    }

    fn schedule_set(
        &self,
        tenant: &TenantId,
        set: &schedules::ScheduleSet,
        now: Timestamp,
    ) -> StoreResult<schedules::Schedule> {
        self.inner.schedule_set(tenant, set, now)
    }

    fn schedule(&self, tenant: &TenantId, id: &str) -> StoreResult<Option<schedules::Schedule>> {
        self.inner.schedule(tenant, id)
    }

    fn schedules(&self, tenant: &TenantId) -> StoreResult<Vec<schedules::Schedule>> {
        self.inner.schedules(tenant)
    }

    fn run_key_holder(
        &self,
        tenant: &TenantId,
        automation: &WorkflowId,
        key: &str,
    ) -> StoreResult<Option<String>> {
        self.inner.run_key_holder(tenant, automation, key)
    }

    fn open_run_key_starts(
        &self,
        tenant: &TenantId,
        automation: &WorkflowId,
        key: &str,
    ) -> StoreResult<Vec<Job>> {
        self.inner.open_run_key_starts(tenant, automation, key)
    }

    fn purge_automation(
        &self,
        now: Timestamp,
        retention: &schedules::AutomationRetention,
    ) -> StoreResult<schedules::AutomationPurged> {
        self.inner.purge_automation(now, retention)
    }

    fn platform_settings(&self) -> StoreResult<Vec<PlatformSetting>> {
        self.inner.platform_settings()
    }

    fn platform_setting_write(
        &self,
        setting: &SettingId,
        value: Option<&Value>,
        expected_revision: u64,
        at: Timestamp,
    ) -> StoreResult<PlatformWrite> {
        self.inner
            .platform_setting_write(setting, value, expected_revision, at)
    }

    fn begin_provisioning(&self, tenant: &TenantId) -> StoreResult<()> {
        if self.halted.load(Ordering::SeqCst) {
            return Err(Self::stopped());
        }
        if let Ok(mut set) = self.announced.lock() {
            set.insert(tenant.clone());
        }
        self.inner.begin_provisioning(tenant)
    }

    fn finish_provisioning(&self, tenant: &TenantId) -> StoreResult<()> {
        if self.halted.load(Ordering::SeqCst) {
            return Err(Self::stopped());
        }
        self.forget_install(tenant);
        self.inner.finish_provisioning(tenant)
    }

    fn rollback_provisioning(&self) -> StoreResult<Vec<TenantId>> {
        let pending: Vec<TenantId> = self
            .provisioning
            .lock()
            .map(|mut set| std::mem::take(&mut *set).into_iter().collect())
            .unwrap_or_default();
        if let Ok(mut set) = self.announced.lock() {
            set.clear();
        }
        for tenant in &pending {
            self.inner.reset_tenant(tenant)?;
        }
        let mut removed = self.inner.rollback_provisioning()?;
        removed.extend(pending);
        Ok(removed)
    }

    fn segment_catalog(&self, tenant: &TenantId) -> StoreResult<Vec<seg::Segment>> {
        self.inner.segment_catalog(tenant)
    }

    fn segment_rows(
        &self,
        tenant: &TenantId,
        pairs: &[(seg::SegmentId, PrincipalId)],
    ) -> StoreResult<seg::Rows> {
        self.inner.segment_rows(tenant, pairs)
    }

    fn segment_members(
        &self,
        tenant: &TenantId,
        segment: seg::SegmentId,
        after: Option<&PrincipalId>,
        limit: usize,
    ) -> StoreResult<Vec<seg::Membership>> {
        self.inner.segment_members(tenant, segment, after, limit)
    }

    fn segment_memberships_of(
        &self,
        tenant: &TenantId,
        person: &PrincipalId,
    ) -> StoreResult<Vec<seg::Membership>> {
        self.inner.segment_memberships_of(tenant, person)
    }

    fn segment_wakes_of(
        &self,
        tenant: &TenantId,
        person: &PrincipalId,
    ) -> StoreResult<Vec<seg::Wake>> {
        self.inner.segment_wakes_of(tenant, person)
    }

    fn segment_due(
        &self,
        tenant: &TenantId,
        segment: seg::SegmentId,
        at: Timestamp,
        limit: usize,
    ) -> StoreResult<Vec<seg::Wake>> {
        self.inner.segment_due(tenant, segment, at, limit)
    }

    fn segment_batches(&self, tenant: &TenantId) -> StoreResult<Vec<seg::Batch>> {
        self.inner.segment_batches(tenant)
    }

    fn segment_referencing(
        &self,
        tenant: &TenantId,
        object: &strata_kernel::ObjectTypeId,
        field: &strata_kernel::FieldId,
        record: strata_kernel::RecordId,
        after: Option<strata_kernel::RecordId>,
        limit: usize,
    ) -> StoreResult<Vec<Arc<Record>>> {
        self.inner
            .segment_referencing(tenant, object, field, record, after, limit)
    }

    fn segment_subject_count(
        &self,
        tenant: &TenantId,
        object: &strata_kernel::ObjectTypeId,
    ) -> StoreResult<u64> {
        self.inner.segment_subject_count(tenant, object)
    }

    fn segment_events_after(
        &self,
        tenant: &TenantId,
        after: u64,
        limit: usize,
    ) -> StoreResult<Vec<(u64, seg::SegmentEvent)>> {
        self.inner.segment_events_after(tenant, after, limit)
    }

    fn segment_cursor(&self, tenant: &TenantId) -> StoreResult<u64> {
        self.inner.segment_cursor(tenant)
    }

    fn commit_with_segments(
        &self,
        commit: &Commit,
        write: &seg::SegmentWrite,
    ) -> StoreResult<Receipt> {
        self.commit_through(|| {
            let receipt = self.inner.commit_with_segments(commit, write)?;
            self.note_creation(commit);
            Ok(receipt)
        })
    }

    fn commit_segments(&self, commit: &segments::SegmentCommit) -> StoreResult<Receipt> {
        self.inner.commit_segments(commit)
    }

    fn search_candidates(
        &self,
        tenant: &TenantId,
        probe: &strata_kernel::search::Probe,
    ) -> StoreResult<Vec<strata_kernel::search::Candidate>> {
        self.inner.search_candidates(tenant, probe)
    }
}
