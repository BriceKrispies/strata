//! Mechanical delegation of every [`Store`] method to the store underneath, with hooks on the
//! paths a read cache has to see: `load`, `load_complete`, `commit` and `commit_with_segments`,
//! plus `wrote`, called after every method that changes what `load` returns (the commits,
//! `restore_snapshot`, `reset_tenant`).
//!
//! Every other method forwards unchanged, as the simulator's fault layer does
//! (`crates/simulation/src/faults.rs`), from which this impl was derived. A method added to the
//! trait later is inherited from its default and must be forwarded here when the default is not
//! the right behaviour (the generation methods of WG-5389 are the case in point).

use crate::layer::{Hook, Layer};
use std::sync::Arc;
use strata_kernel::segments as seg;
use strata_kernel::{
    DomainEvent, Need, PrincipalId, Record, SessionId, SettingId, TenantId, TenantState, Timestamp,
    Value, WorkflowId,
};
use strata_storage_api::jobs::{
    ExceptionKind, ExceptionRecord, Job, JobChange, JobReport, Lease, NewJob, Reported, RunRecord,
    RunState, Settled, StepRecord, WorkerWrites,
};
use strata_storage_api::*;

impl<H: Hook> Store for Layer<H> {
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

    // `load` and `load_complete` are the trait's defaults over the two methods below, so every
    // read reaches the hook. Each of the four loads is forwarded explicitly: a layer never falls
    // back to a default that would decode a header it was handed already.
    fn load_at(&self, tenant: &TenantId, needs: &[Need]) -> StoreResult<Loaded> {
        self.hook.load_at(self.inner.as_ref(), tenant, needs)
    }

    fn load_complete_at(&self, tenant: &TenantId) -> StoreResult<Loaded> {
        self.hook.load_complete_at(self.inner.as_ref(), tenant)
    }

    fn load_at_held(
        &self,
        tenant: &TenantId,
        needs: &[Need],
        held: Option<&resident::HeldHeader>,
    ) -> StoreResult<resident::HeldLoad> {
        self.hook
            .load_at_held(self.inner.as_ref(), tenant, needs, held)
    }

    fn load_complete_at_held(
        &self,
        tenant: &TenantId,
        held: Option<&resident::HeldHeader>,
    ) -> StoreResult<resident::HeldLoad> {
        self.hook
            .load_complete_at_held(self.inner.as_ref(), tenant, held)
    }

    fn generation(&self, tenant: &TenantId) -> StoreResult<Option<Generation>> {
        self.inner.generation(tenant)
    }

    fn rotate_nonce(&self, tenant: &TenantId) -> StoreResult<Generation> {
        let done = self.inner.rotate_nonce(tenant);
        self.hook.wrote(tenant, done.as_ref().err());
        done
    }

    fn commit(&self, commit: &Commit) -> StoreResult<Receipt> {
        let done = self.hook.commit(self.inner.as_ref(), commit);
        self.hook.wrote(&commit.delta.tenant, done.as_ref().err());
        done
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
        self.inner.outbox(limit)
    }

    fn outbox_faults(&self) -> Vec<OutboxFault> {
        self.inner.outbox_faults()
    }

    fn outbox_pending_of(&self, tenant: &TenantId) -> StoreResult<u64> {
        self.inner.outbox_pending_of(tenant)
    }

    fn mark_published(&self, tenant: &TenantId, position: u64) -> StoreResult<()> {
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
        expected: Option<&Generation>,
    ) -> StoreResult<strata_kernel::ledger::Anonymised> {
        let done = self
            .inner
            .anonymise_ledger_principal(tenant, principal, expected);
        self.hook.wrote(tenant, done.as_ref().err());
        done
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
        expected: Option<&Generation>,
    ) -> StoreResult<()> {
        let done = self.inner.restore_snapshot(tenant, id, expected);
        self.hook.wrote(tenant, done.as_ref().err());
        done
    }

    fn reset(&self) -> StoreResult<()> {
        self.inner.reset()
    }

    fn reset_tenant(&self, tenant: &TenantId) -> StoreResult<()> {
        let done = self.inner.reset_tenant(tenant);
        self.hook.wrote(tenant, done.as_ref().err());
        done
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
        self.inner.begin_provisioning(tenant)
    }

    fn finish_provisioning(&self, tenant: &TenantId) -> StoreResult<()> {
        self.inner.finish_provisioning(tenant)
    }

    fn rollback_provisioning(&self) -> StoreResult<Vec<TenantId>> {
        self.inner.rollback_provisioning()
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
        let done = self
            .hook
            .commit_with_segments(self.inner.as_ref(), commit, write);
        self.hook.wrote(&commit.delta.tenant, done.as_ref().err());
        done
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
