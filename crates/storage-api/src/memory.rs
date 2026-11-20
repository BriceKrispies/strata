//! The reference adapter: everything in memory, behind one lock.
//!
//! It exists to state the contract in the simplest possible code, to back tests, and to show
//! that the rest of the system does not depend on SQLite. It is not durable and says so.

use crate::jobs::{
    self, ExceptionKind, ExceptionRecord, Job, JobChange, JobReport, Lease, NewJob, Reported,
    RunRecord, RunState, Settled, StepRecord, WorkerWrites,
};
use crate::{
    check_event_sequence, partial_state, throttle_refund, throttle_step, Account, AuditEntry,
    Capabilities, Commit, FederatedIdentity, Health, IdempotencyEntry, IdempotentResult,
    JournalEntry, OidcFlow, OutboxEntry, PasskeyChallenge, PasskeyRecord, PendingSignIn,
    PrincipalRemoval, Receipt, RecoveryRequest, SamlRequest, SessionRecord, SignInLink,
    SnapshotInfo, StorageMigration, Store, StoreError, StoreResult, TenantSummary, ThrottleRow,
    ThrottleRule, ThrottleSettle, ThrottleVerdict, TotpRecord, IDEMPOTENCY_RETENTION_MS,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use strata_kernel::ext::*;
use strata_kernel::segments as seg;
use strata_kernel::{
    apply_delta, canon, DomainEvent, FieldId, Need, ObjectTypeId, PrincipalId, Record, RecordId,
    SessionId, State, TenantId, TenantState, Timestamp, Value, WorkflowId,
};

#[derive(Default)]
struct Inner {
    tenants: BTreeMap<TenantId, Arc<TenantState>>,
    /// The change counter of each object type of each tenant ([`Store::object_revisions`]).
    object_revisions: BTreeMap<(TenantId, ObjectTypeId), u64>,
    /// Every event in commit order, with its position in its tenant's outbox and whether it was
    /// published.
    events: Vec<(u64, DomainEvent, bool)>,
    /// The last outbox position given out, per tenant.
    positions: BTreeMap<TenantId, u64>,
    journal: Vec<JournalEntry>,
    sessions: BTreeMap<SessionId, SessionRecord>,
    accounts: BTreeMap<(TenantId, String), Account>,
    throttle: BTreeMap<(TenantId, String), ThrottleRow>,
    passkeys: BTreeMap<(TenantId, String), PasskeyRecord>,
    challenges: BTreeMap<(TenantId, String), PasskeyChallenge>,
    links: BTreeMap<(TenantId, String), SignInLink>,
    flows: BTreeMap<(TenantId, String), OidcFlow>,
    saml_requests: BTreeMap<(TenantId, String), SamlRequest>,
    saml_assertions: BTreeMap<(TenantId, String, String), Timestamp>,
    identities: BTreeMap<(TenantId, String, String), FederatedIdentity>,
    totp: BTreeMap<(TenantId, PrincipalId), TotpRecord>,
    recovery_codes: BTreeMap<(TenantId, PrincipalId), BTreeSet<String>>,
    pending: BTreeMap<(TenantId, String), PendingSignIn>,
    recoveries: BTreeMap<(TenantId, String), RecoveryRequest>,
    docs: BTreeMap<(TenantId, String, String), String>,
    audit: BTreeMap<TenantId, Vec<AuditEntry>>,
    snapshots: BTreeMap<(TenantId, u64), (SnapshotInfo, Arc<TenantState>)>,
    /// Results by tenant, principal scope and key, with the receipt of the commit.
    idempotency: BTreeMap<(TenantId, String, String), (IdempotencyEntry, Receipt)>,
    /// The segment block of each tenant (SEG-001 to SEG-038).
    segments: BTreeMap<TenantId, seg::SegmentTables>,
    /// Jobs of the automation worker per tenant, in creation order: a job's `seq` is its index
    /// plus one.
    jobs: BTreeMap<TenantId, Vec<Job>>,
    /// Runs per tenant, in the order they were first written: a run's `seq` is its index plus one.
    runs: BTreeMap<TenantId, Vec<RunRecord>>,
    /// Steps by tenant, run and position.
    steps: BTreeMap<(TenantId, String, u64), StepRecord>,
    /// Exception records per tenant, in the order they were written: `seq` is the index plus one.
    exceptions: BTreeMap<TenantId, Vec<ExceptionRecord>>,
}

impl Inner {
    /// Create the jobs whose keys the tenant does not hold yet; the ids of all, in order.
    fn add_jobs(&mut self, tenant: &TenantId, new: &[NewJob], now: Timestamp) -> Vec<String> {
        let list = self.jobs.entry(tenant.clone()).or_default();
        new.iter()
            .map(|wanted| {
                let id = jobs::job_id(tenant, &wanted.key);
                if !list.iter().any(|job| job.id == id) {
                    let mut job = jobs::new_job(tenant, wanted, now);
                    job.seq = list.len() as u64 + 1;
                    list.push(job);
                }
                id
            })
            .collect()
    }

    /// Exception records whose ids the tenant does not hold yet.
    fn add_exceptions(&mut self, tenant: &TenantId, records: &[ExceptionRecord]) {
        let list = self.exceptions.entry(tenant.clone()).or_default();
        for record in records {
            if !list.iter().any(|held| held.id == record.id) {
                let mut record = record.clone();
                record.seq = list.len() as u64 + 1;
                list.push(record);
            }
        }
    }

    /// Everything of `writes`, as [`crate::Store::automation_write`] states.
    fn write(&mut self, tenant: &TenantId, writes: &WorkerWrites, now: Timestamp) {
        let runs = self.runs.entry(tenant.clone()).or_default();
        for run in &writes.runs {
            match runs.iter_mut().find(|held| held.run == run.run) {
                Some(held) => {
                    let seq = held.seq;
                    *held = run.clone();
                    held.seq = seq;
                }
                None => {
                    let mut run = run.clone();
                    run.seq = runs.len() as u64 + 1;
                    runs.push(run);
                }
            }
        }
        for step in &writes.steps {
            self.steps.insert(
                (tenant.clone(), step.run.clone(), step.sequence),
                step.clone(),
            );
        }
        self.add_exceptions(tenant, &writes.exceptions);
        self.add_jobs(tenant, &writes.jobs, now);
    }

    fn job_mut(&mut self, tenant: &TenantId, id: &str) -> Option<&mut Job> {
        self.jobs
            .get_mut(tenant)
            .and_then(|list| list.iter_mut().find(|job| job.id == id))
    }
}

/// The first problem of a list of jobs to create, as a refusal.
fn check_jobs(new: &[NewJob]) -> StoreResult<()> {
    match new.iter().find_map(jobs::job_problem) {
        Some(problem) => Err(StoreError::Invalid(problem)),
        None => Ok(()),
    }
}

#[derive(Default)]
pub struct MemoryStore {
    inner: Mutex<Inner>,
    loads: AtomicUsize,
    /// Added to every object type's counter, and raised when a snapshot is restored or a tenant
    /// is reset, so that no counter repeats a value for different records.
    epoch: AtomicU64,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many times `load` has been called, for tests that bound the loads of one decision.
    pub fn loads(&self) -> usize {
        self.loads.load(Ordering::Relaxed)
    }

    fn lock(&self) -> StoreResult<MutexGuard<'_, Inner>> {
        self.inner
            .lock()
            .map_err(|_| StoreError::Unavailable("a writer panicked".into()))
    }
}

impl Store for MemoryStore {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            adapter: "memory".into(),
            durable: false,
            isolation: "serialisable: one lock around every operation".into(),
            read_consistency: "a load sees only whole commits".into(),
            notes: vec!["all data is lost when the process exits".into()],
        }
    }

    fn health(&self) -> StoreResult<Health> {
        let inner = self.lock()?;
        Ok(Health {
            ok: true,
            detail: "in memory".into(),
            tenants: inner.tenants.len() as u64,
            outbox_pending: inner
                .events
                .iter()
                .filter(|(_, _, published)| !published)
                .count() as u64,
        })
    }

    fn liveness(&self) -> StoreResult<()> {
        self.lock().map(|_| ())
    }

    fn migrations(&self) -> StoreResult<Vec<StorageMigration>> {
        Ok(Vec::new())
    }

    fn tenants(&self) -> StoreResult<Vec<TenantSummary>> {
        Ok(self
            .lock()?
            .tenants
            .values()
            .map(|t| TenantSummary {
                id: t.id.clone(),
                name: t.name.clone(),
                revision: t.revision,
                schema_version: t.schema.version.0,
                event_seq: t.event_seq,
                records: t.record_count() as u64,
            })
            .collect())
    }

    fn tenant_summary(&self, tenant: &TenantId) -> StoreResult<Option<TenantSummary>> {
        Ok(self.lock()?.tenants.get(tenant).map(|t| TenantSummary {
            id: t.id.clone(),
            name: t.name.clone(),
            revision: t.revision,
            schema_version: t.schema.version.0,
            event_seq: t.event_seq,
            records: t.record_count() as u64,
        }))
    }

    fn object_revisions(
        &self,
        tenant: &TenantId,
        objects: &[ObjectTypeId],
    ) -> StoreResult<BTreeMap<ObjectTypeId, u64>> {
        let inner = self.lock()?;
        let epoch = self.epoch.load(Ordering::Relaxed);
        Ok(objects
            .iter()
            .map(|object| {
                let counter = inner
                    .object_revisions
                    .get(&(tenant.clone(), object.clone()))
                    .copied()
                    .unwrap_or(0);
                (object.clone(), epoch + counter)
            })
            .collect())
    }

    fn load(&self, tenant: &TenantId, needs: &[Need]) -> StoreResult<State> {
        self.loads.fetch_add(1, Ordering::Relaxed);
        Ok(match self.lock()?.tenants.get(tenant) {
            Some(found) => partial_state(found, needs),
            None => State::absent(tenant.clone()),
        })
    }

    fn load_complete(&self, tenant: &TenantId) -> StoreResult<State> {
        Ok(match self.lock()?.tenants.get(tenant) {
            Some(found) => State::complete(found.clone()),
            None => State::absent(tenant.clone()),
        })
    }

    fn commit(&self, commit: &Commit) -> StoreResult<Receipt> {
        let mut inner = self.lock()?;
        commit_in(&mut inner, commit, None)
    }

    fn commit_with_segments(
        &self,
        commit: &Commit,
        write: &seg::SegmentWrite,
    ) -> StoreResult<Receipt> {
        let mut inner = self.lock()?;
        commit_in(&mut inner, commit, Some(write))
    }

    fn commit_segments(&self, commit: &crate::segments::SegmentCommit) -> StoreResult<Receipt> {
        let mut inner = self.lock()?;
        let found = inner.tenants.get(&commit.tenant).map(|t| t.revision);
        if found != Some(commit.revision) {
            return Err(StoreError::Conflict {
                expected: Some(commit.revision),
                found,
            });
        }
        let tables = inner.segments.entry(commit.tenant.clone()).or_default();
        tables
            .apply(&commit.write)
            .map_err(|_| crate::segments::write_conflict())?;
        Ok(Receipt {
            revision: commit.revision,
            last_position: None,
        })
    }

    fn search_candidates(
        &self,
        tenant: &TenantId,
        probe: &strata_kernel::search::Probe,
    ) -> StoreResult<Vec<strata_kernel::search::Candidate>> {
        let inner = self.lock()?;
        let Some(state) = inner.tenants.get(tenant) else {
            return Ok(Vec::new());
        };
        let Some(def) = state.schema.objects.get(&probe.object) else {
            return Ok(Vec::new());
        };
        Ok(strata_kernel::search::candidates_in(
            state.records_of(&probe.object),
            def,
            probe,
        ))
    }

    fn segment_catalog(&self, tenant: &TenantId) -> StoreResult<Vec<seg::Segment>> {
        Ok(self
            .lock()?
            .segments
            .get(tenant)
            .map(seg::SegmentTables::catalog)
            .unwrap_or_default())
    }

    fn segment_rows(
        &self,
        tenant: &TenantId,
        pairs: &[(seg::SegmentId, PrincipalId)],
    ) -> StoreResult<seg::Rows> {
        let inner = self.lock()?;
        Ok(match inner.segments.get(tenant) {
            Some(tables) => tables.rows(pairs),
            None => seg::SegmentTables::default().rows(pairs),
        })
    }

    fn segment_members(
        &self,
        tenant: &TenantId,
        segment: seg::SegmentId,
        after: Option<&PrincipalId>,
        limit: usize,
    ) -> StoreResult<Vec<seg::Membership>> {
        Ok(self
            .lock()?
            .segments
            .get(tenant)
            .map(|tables| tables.members_after(segment, after, limit))
            .unwrap_or_default())
    }

    fn segment_memberships_of(
        &self,
        tenant: &TenantId,
        person: &PrincipalId,
    ) -> StoreResult<Vec<seg::Membership>> {
        Ok(self
            .lock()?
            .segments
            .get(tenant)
            .map(|tables| tables.memberships_of(person))
            .unwrap_or_default())
    }

    fn segment_wakes_of(
        &self,
        tenant: &TenantId,
        person: &PrincipalId,
    ) -> StoreResult<Vec<seg::Wake>> {
        Ok(self
            .lock()?
            .segments
            .get(tenant)
            .map(|tables| {
                tables
                    .wakes
                    .iter()
                    .filter(|((_, p), _)| p == person)
                    .map(|((s, p), at)| seg::Wake {
                        segment: *s,
                        person: p.clone(),
                        at: *at,
                    })
                    .collect()
            })
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
            .lock()?
            .segments
            .get(tenant)
            .map(|tables| tables.due(segment, at, limit))
            .unwrap_or_default())
    }

    fn segment_batches(&self, tenant: &TenantId) -> StoreResult<Vec<seg::Batch>> {
        Ok(self
            .lock()?
            .segments
            .get(tenant)
            .map(|tables| tables.batches.values().cloned().collect())
            .unwrap_or_default())
    }

    fn segment_referencing(
        &self,
        tenant: &TenantId,
        object: &ObjectTypeId,
        field: &FieldId,
        record: RecordId,
        after: Option<RecordId>,
        limit: usize,
    ) -> StoreResult<Vec<Arc<Record>>> {
        let inner = self.lock()?;
        let wanted = Value::Ref(record);
        Ok(inner
            .tenants
            .get(tenant)
            .map(|t| {
                t.records_of(object)
                    .filter(|r| crate::field_matches(r, field, &wanted))
                    .filter(|r| after.is_none_or(|a| r.id > a))
                    .take(limit)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    fn segment_subject_count(&self, tenant: &TenantId, object: &ObjectTypeId) -> StoreResult<u64> {
        Ok(self
            .lock()?
            .tenants
            .get(tenant)
            .map_or(0, |t| t.records_of(object).count() as u64))
    }

    fn segment_events_after(
        &self,
        tenant: &TenantId,
        after: u64,
        limit: usize,
    ) -> StoreResult<Vec<(u64, seg::SegmentEvent)>> {
        Ok(self
            .lock()?
            .segments
            .get(tenant)
            .map(|tables| tables.events_after(after, limit))
            .unwrap_or_default())
    }

    fn idempotent_result(
        &self,
        tenant: &TenantId,
        principal: &str,
        key: &str,
        now: Timestamp,
    ) -> StoreResult<Option<IdempotentResult>> {
        Ok(self
            .lock()?
            .idempotency
            .get(&(tenant.clone(), principal.to_string(), key.to_string()))
            .filter(|(held, _)| held.recorded_at.0 >= now.0 - IDEMPOTENCY_RETENTION_MS)
            .map(|(held, receipt)| IdempotentResult {
                request_hash: held.request_hash.clone(),
                result: held.result.clone(),
                receipt: receipt.clone(),
            }))
    }

    fn events_after(
        &self,
        tenant: &TenantId,
        after: u64,
        limit: usize,
    ) -> StoreResult<Vec<DomainEvent>> {
        Ok(self
            .lock()?
            .events
            .iter()
            .filter(|(_, event, _)| event.tenant == *tenant && event.seq > after)
            .take(limit)
            .map(|(_, event, _)| event.clone())
            .collect())
    }

    fn outbox(&self, limit: usize) -> StoreResult<Vec<OutboxEntry>> {
        // Round-robin over the tenants, so a long backlog does not hold back another tenant.
        let inner = self.lock()?;
        let mut queues: BTreeMap<&TenantId, std::collections::VecDeque<OutboxEntry>> =
            BTreeMap::new();
        for (position, event, published) in inner.events.iter() {
            if !published {
                queues
                    .entry(&event.tenant)
                    .or_default()
                    .push_back(OutboxEntry {
                        position: *position,
                        event: event.clone(),
                    });
            }
        }
        let mut out = Vec::new();
        while out.len() < limit && queues.values().any(|q| !q.is_empty()) {
            for queue in queues.values_mut() {
                if out.len() >= limit {
                    break;
                }
                if let Some(entry) = queue.pop_front() {
                    out.push(entry);
                }
            }
        }
        Ok(out)
    }

    fn outbox_pending_of(&self, tenant: &TenantId) -> StoreResult<u64> {
        Ok(self
            .lock()?
            .events
            .iter()
            .filter(|(_, event, published)| !published && event.tenant == *tenant)
            .count() as u64)
    }

    fn mark_published(&self, tenant: &TenantId, position: u64) -> StoreResult<()> {
        for entry in self
            .lock()?
            .events
            .iter_mut()
            .filter(|(p, event, _)| event.tenant == *tenant && *p <= position)
        {
            entry.2 = true;
        }
        Ok(())
    }

    fn journal_after(
        &self,
        tenant: &TenantId,
        after: u64,
        limit: usize,
    ) -> StoreResult<Vec<JournalEntry>> {
        Ok(self
            .lock()?
            .journal
            .iter()
            .filter(|entry| entry.tenant == *tenant && entry.revision.0 > after)
            .take(limit)
            .cloned()
            .collect())
    }

    fn journal_count(&self, tenant: &TenantId) -> StoreResult<u64> {
        Ok(self
            .lock()?
            .journal
            .iter()
            .filter(|entry| entry.tenant == *tenant)
            .count() as u64)
    }

    fn put_session(&self, session: &SessionRecord) -> StoreResult<()> {
        let mut inner = self.lock()?;
        if !inner.tenants.contains_key(&session.tenant) {
            return Err(StoreError::NotFound(format!("tenant {}", session.tenant)));
        }
        let clash = inner
            .sessions
            .values()
            .any(|s| s.token_hash == session.token_hash && s.id != session.id);
        if clash {
            return Err(StoreError::Invalid(
                "session token is already in use".into(),
            ));
        }
        inner.sessions.insert(session.id.clone(), session.clone());
        Ok(())
    }

    fn session_seen(&self, id: &SessionId, at: Timestamp) -> StoreResult<()> {
        if let Some(s) = self.lock()?.sessions.get_mut(id) {
            s.last_seen_at = at;
        }
        Ok(())
    }

    fn session_authenticated(
        &self,
        id: &SessionId,
        at: Timestamp,
        network: Option<&str>,
    ) -> StoreResult<()> {
        if let Some(s) = self.lock()?.sessions.get_mut(id) {
            s.authenticated_at = at;
            s.network = network.map(str::to_string);
        }
        Ok(())
    }

    fn session_by_token(&self, token_hash: &str) -> StoreResult<Option<SessionRecord>> {
        Ok(self
            .lock()?
            .sessions
            .values()
            .find(|s| s.token_hash == token_hash)
            .cloned())
    }

    fn session(&self, id: &SessionId) -> StoreResult<Option<SessionRecord>> {
        Ok(self.lock()?.sessions.get(id).cloned())
    }

    fn live_sessions(&self, now: Timestamp) -> StoreResult<Vec<SessionRecord>> {
        Ok(self
            .lock()?
            .sessions
            .values()
            .filter(|s| s.revoked_at.is_none() && s.expires_at > now)
            .cloned()
            .collect())
    }

    fn live_sessions_of(
        &self,
        tenant: &TenantId,
        principal: Option<&strata_kernel::PrincipalId>,
        now: Timestamp,
    ) -> StoreResult<Vec<SessionRecord>> {
        Ok(self
            .lock()?
            .sessions
            .values()
            .filter(|s| {
                s.tenant == *tenant
                    && principal.is_none_or(|p| s.principal == *p)
                    && s.revoked_at.is_none()
                    && s.expires_at > now
            })
            .cloned()
            .collect())
    }

    fn purge_sessions(&self, before: Timestamp) -> StoreResult<u64> {
        let mut inner = self.lock()?;
        let count = inner.sessions.len();
        inner
            .sessions
            .retain(|_, s| s.expires_at >= before && s.revoked_at.is_none_or(|at| at >= before));
        Ok((count - inner.sessions.len()) as u64)
    }

    fn throttle_reserve(
        &self,
        tenant: &TenantId,
        rules: &[ThrottleRule],
        now: Timestamp,
    ) -> StoreResult<ThrottleVerdict> {
        let mut inner = self.lock()?;
        let mut next = Vec::with_capacity(rules.len());
        let mut until: Option<i64> = None;
        for rule in rules {
            let row = inner
                .throttle
                .get(&(tenant.clone(), rule.key.clone()))
                .copied();
            match throttle_step(row, rule, now.0) {
                Ok(row) => next.push((rule.key.clone(), row)),
                Err(locked) => until = Some(until.map_or(locked, |u| u.max(locked))),
            }
        }
        if let Some(until) = until {
            return Ok(ThrottleVerdict::Refused {
                until: Timestamp(until),
            });
        }
        for (key, row) in next {
            inner.throttle.insert((tenant.clone(), key), row);
        }
        Ok(ThrottleVerdict::Allowed)
    }

    fn throttle_settle(
        &self,
        tenant: &TenantId,
        key: &str,
        max: u32,
        how: ThrottleSettle,
    ) -> StoreResult<()> {
        let mut inner = self.lock()?;
        let slot = (tenant.clone(), key.to_string());
        match how {
            ThrottleSettle::Clear => {
                inner.throttle.remove(&slot);
            }
            ThrottleSettle::Refund => {
                if let Some(row) = inner.throttle.get_mut(&slot) {
                    *row = throttle_refund(*row, max);
                }
            }
        }
        Ok(())
    }

    fn purge_throttle(&self, now: Timestamp) -> StoreResult<u64> {
        let mut inner = self.lock()?;
        let count = inner.throttle.len();
        inner.throttle.retain(|_, row| row.expires_at > now.0);
        Ok((count - inner.throttle.len()) as u64)
    }

    fn account_of(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<Option<Account>> {
        Ok(self
            .lock()?
            .accounts
            .values()
            .find(|account| account.tenant == *tenant && account.principal == *principal)
            .cloned())
    }

    fn put_passkey(&self, passkey: &PasskeyRecord) -> StoreResult<bool> {
        let mut inner = self.lock()?;
        let slot = (passkey.tenant.clone(), passkey.id.clone());
        if inner.passkeys.contains_key(&slot) {
            return Ok(false);
        }
        inner.passkeys.insert(slot, passkey.clone());
        Ok(true)
    }

    fn passkey(&self, tenant: &TenantId, id: &str) -> StoreResult<Option<PasskeyRecord>> {
        Ok(self
            .lock()?
            .passkeys
            .get(&(tenant.clone(), id.to_string()))
            .cloned())
    }

    fn passkeys(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<Vec<PasskeyRecord>> {
        let mut found: Vec<PasskeyRecord> = self
            .lock()?
            .passkeys
            .values()
            .filter(|p| p.tenant == *tenant && p.principal == *principal)
            .cloned()
            .collect();
        found.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
        Ok(found)
    }

    fn update_passkey(
        &self,
        tenant: &TenantId,
        id: &str,
        credential: &str,
        used_at: Timestamp,
    ) -> StoreResult<bool> {
        let mut inner = self.lock()?;
        match inner.passkeys.get_mut(&(tenant.clone(), id.to_string())) {
            Some(record) => {
                record.credential = credential.to_string();
                record.last_used_at = Some(used_at);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn remove_passkey(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        id: &str,
    ) -> StoreResult<bool> {
        let mut inner = self.lock()?;
        let slot = (tenant.clone(), id.to_string());
        if inner
            .passkeys
            .get(&slot)
            .is_some_and(|p| p.principal == *principal)
        {
            inner.passkeys.remove(&slot);
            return Ok(true);
        }
        Ok(false)
    }

    fn put_passkey_challenge(&self, challenge: &PasskeyChallenge) -> StoreResult<()> {
        self.lock()?.challenges.insert(
            (challenge.tenant.clone(), challenge.id_hash.clone()),
            challenge.clone(),
        );
        Ok(())
    }

    fn take_passkey_challenge(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        now: Timestamp,
    ) -> StoreResult<Option<PasskeyChallenge>> {
        let taken = self
            .lock()?
            .challenges
            .remove(&(tenant.clone(), id_hash.to_string()));
        Ok(taken.filter(|challenge| challenge.expires_at > now))
    }

    fn purge_passkey_challenges(&self, now: Timestamp) -> StoreResult<u64> {
        let mut inner = self.lock()?;
        let count = inner.challenges.len();
        inner.challenges.retain(|_, c| c.expires_at > now);
        Ok((count - inner.challenges.len()) as u64)
    }

    fn put_sign_in_link(&self, link: &SignInLink) -> StoreResult<()> {
        self.lock()?
            .links
            .insert((link.tenant.clone(), link.id_hash.clone()), link.clone());
        Ok(())
    }

    fn take_sign_in_link(
        &self,
        tenant: &TenantId,
        id_hash: &str,
    ) -> StoreResult<Option<SignInLink>> {
        Ok(self
            .lock()?
            .links
            .remove(&(tenant.clone(), id_hash.to_string())))
    }

    fn purge_sign_in_links(&self, now: Timestamp) -> StoreResult<u64> {
        let mut inner = self.lock()?;
        let count = inner.links.len();
        inner.links.retain(|_, link| link.expires_at >= now);
        Ok((count - inner.links.len()) as u64)
    }

    fn put_oidc_flow(&self, flow: &OidcFlow) -> StoreResult<()> {
        self.lock()?
            .flows
            .insert((flow.tenant.clone(), flow.id_hash.clone()), flow.clone());
        Ok(())
    }

    fn take_oidc_flow(&self, tenant: &TenantId, id_hash: &str) -> StoreResult<Option<OidcFlow>> {
        Ok(self
            .lock()?
            .flows
            .remove(&(tenant.clone(), id_hash.to_string())))
    }

    fn purge_oidc_flows(&self, now: Timestamp) -> StoreResult<u64> {
        let mut inner = self.lock()?;
        let count = inner.flows.len();
        inner.flows.retain(|_, flow| flow.expires_at >= now);
        Ok((count - inner.flows.len()) as u64)
    }

    fn put_saml_request(&self, request: &SamlRequest) -> StoreResult<()> {
        self.lock()?.saml_requests.insert(
            (request.tenant.clone(), request.id_hash.clone()),
            request.clone(),
        );
        Ok(())
    }

    fn take_saml_request(
        &self,
        tenant: &TenantId,
        id_hash: &str,
    ) -> StoreResult<Option<SamlRequest>> {
        Ok(self
            .lock()?
            .saml_requests
            .remove(&(tenant.clone(), id_hash.to_string())))
    }

    fn purge_saml_requests(&self, now: Timestamp) -> StoreResult<u64> {
        let mut inner = self.lock()?;
        let count = inner.saml_requests.len();
        inner
            .saml_requests
            .retain(|_, request| request.expires_at >= now);
        Ok((count - inner.saml_requests.len()) as u64)
    }

    fn put_saml_assertion(
        &self,
        tenant: &TenantId,
        provider: &str,
        id_hash: &str,
        expires_at: Timestamp,
    ) -> StoreResult<bool> {
        let mut inner = self.lock()?;
        if !inner.tenants.contains_key(tenant) {
            return Err(StoreError::NotFound(format!("tenant {tenant}")));
        }
        let slot = (tenant.clone(), provider.to_string(), id_hash.to_string());
        if inner.saml_assertions.contains_key(&slot) {
            return Ok(false);
        }
        inner.saml_assertions.insert(slot, expires_at);
        Ok(true)
    }

    fn purge_saml_assertions(&self, now: Timestamp) -> StoreResult<u64> {
        let mut inner = self.lock()?;
        let count = inner.saml_assertions.len();
        inner
            .saml_assertions
            .retain(|_, expires_at| *expires_at >= now);
        Ok((count - inner.saml_assertions.len()) as u64)
    }

    fn put_identity(&self, identity: &FederatedIdentity) -> StoreResult<bool> {
        let mut inner = self.lock()?;
        if !inner.tenants.contains_key(&identity.tenant) {
            return Err(StoreError::NotFound(format!("tenant {}", identity.tenant)));
        }
        let slot = (
            identity.tenant.clone(),
            identity.provider.clone(),
            identity.subject.clone(),
        );
        if inner.identities.contains_key(&slot) {
            return Ok(false);
        }
        inner.identities.insert(slot, identity.clone());
        Ok(true)
    }

    fn identity(
        &self,
        tenant: &TenantId,
        provider: &str,
        subject: &str,
    ) -> StoreResult<Option<FederatedIdentity>> {
        Ok(self
            .lock()?
            .identities
            .get(&(tenant.clone(), provider.to_string(), subject.to_string()))
            .cloned())
    }

    fn put_totp(&self, totp: &TotpRecord) -> StoreResult<bool> {
        let mut inner = self.lock()?;
        let slot = (totp.tenant.clone(), totp.principal.clone());
        if inner.totp.get(&slot).is_some_and(|held| held.confirmed) {
            return Ok(false);
        }
        inner.totp.insert(slot, totp.clone());
        Ok(true)
    }

    fn totp(&self, tenant: &TenantId, principal: &PrincipalId) -> StoreResult<Option<TotpRecord>> {
        Ok(self
            .lock()?
            .totp
            .get(&(tenant.clone(), principal.clone()))
            .cloned())
    }

    fn confirm_totp(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        step: i64,
    ) -> StoreResult<bool> {
        let mut inner = self.lock()?;
        match inner.totp.get_mut(&(tenant.clone(), principal.clone())) {
            Some(record) if !record.confirmed => {
                record.confirmed = true;
                record.last_step = step;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn use_totp_step(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        step: i64,
    ) -> StoreResult<bool> {
        let mut inner = self.lock()?;
        match inner.totp.get_mut(&(tenant.clone(), principal.clone())) {
            Some(record) if record.confirmed && record.last_step < step => {
                record.last_step = step;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn remove_totp(&self, tenant: &TenantId, principal: &PrincipalId) -> StoreResult<bool> {
        Ok(self
            .lock()?
            .totp
            .remove(&(tenant.clone(), principal.clone()))
            .is_some())
    }

    fn replace_recovery_codes(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        hashes: &[String],
        _at: Timestamp,
    ) -> StoreResult<()> {
        self.lock()?.recovery_codes.insert(
            (tenant.clone(), principal.clone()),
            hashes.iter().cloned().collect(),
        );
        Ok(())
    }

    fn spend_recovery_code(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        hash: &str,
    ) -> StoreResult<bool> {
        Ok(self
            .lock()?
            .recovery_codes
            .get_mut(&(tenant.clone(), principal.clone()))
            .is_some_and(|codes| codes.remove(hash)))
    }

    fn recovery_codes_left(&self, tenant: &TenantId, principal: &PrincipalId) -> StoreResult<u32> {
        Ok(self
            .lock()?
            .recovery_codes
            .get(&(tenant.clone(), principal.clone()))
            .map_or(0, |codes| codes.len() as u32))
    }

    fn put_pending(&self, pending: &PendingSignIn) -> StoreResult<()> {
        self.lock()?.pending.insert(
            (pending.tenant.clone(), pending.id_hash.clone()),
            pending.clone(),
        );
        Ok(())
    }

    fn attempt_pending(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        now: Timestamp,
        max: u32,
    ) -> StoreResult<Option<PendingSignIn>> {
        let mut inner = self.lock()?;
        let slot = (tenant.clone(), id_hash.to_string());
        let Some(held) = inner.pending.get_mut(&slot) else {
            return Ok(None);
        };
        if held.expires_at <= now || held.attempts >= max {
            inner.pending.remove(&slot);
            return Ok(None);
        }
        held.attempts += 1;
        Ok(Some(held.clone()))
    }

    fn take_pending(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        now: Timestamp,
    ) -> StoreResult<Option<PendingSignIn>> {
        let taken = self
            .lock()?
            .pending
            .remove(&(tenant.clone(), id_hash.to_string()));
        Ok(taken.filter(|pending| pending.expires_at > now))
    }

    fn put_recovery(&self, request: &RecoveryRequest) -> StoreResult<()> {
        let mut inner = self.lock()?;
        inner
            .recoveries
            .retain(|(t, _), r| !(t == &request.tenant && r.principal == request.principal));
        inner.recoveries.insert(
            (request.tenant.clone(), request.id_hash.clone()),
            request.clone(),
        );
        Ok(())
    }

    fn recovery(&self, tenant: &TenantId, id_hash: &str) -> StoreResult<Option<RecoveryRequest>> {
        Ok(self
            .lock()?
            .recoveries
            .get(&(tenant.clone(), id_hash.to_string()))
            .cloned())
    }

    fn take_recovery(
        &self,
        tenant: &TenantId,
        id_hash: &str,
    ) -> StoreResult<Option<RecoveryRequest>> {
        Ok(self
            .lock()?
            .recoveries
            .remove(&(tenant.clone(), id_hash.to_string())))
    }

    fn cancel_recovery(
        &self,
        tenant: &TenantId,
        id_hash: &str,
        cancel_hash: &str,
    ) -> StoreResult<bool> {
        let mut inner = self.lock()?;
        let slot = (tenant.clone(), id_hash.to_string());
        if inner
            .recoveries
            .get(&slot)
            .is_some_and(|r| r.cancel_hash == cancel_hash)
        {
            inner.recoveries.remove(&slot);
            return Ok(true);
        }
        Ok(false)
    }

    fn purge_factor_challenges(&self, now: Timestamp) -> StoreResult<u64> {
        let mut inner = self.lock()?;
        let count = inner.pending.len() + inner.recoveries.len();
        inner.pending.retain(|_, p| p.expires_at > now);
        inner.recoveries.retain(|_, r| r.expires_at > now);
        Ok((count - inner.pending.len() - inner.recoveries.len()) as u64)
    }

    fn put_account(&self, account: &Account) -> StoreResult<()> {
        let mut inner = self.lock()?;
        if !inner.tenants.contains_key(&account.tenant) {
            return Err(StoreError::NotFound(format!("tenant {}", account.tenant)));
        }
        inner.accounts.insert(
            (account.tenant.clone(), account.login.clone()),
            account.clone(),
        );
        Ok(())
    }

    fn create_account(&self, account: &Account) -> StoreResult<bool> {
        let mut inner = self.lock()?;
        if !inner.tenants.contains_key(&account.tenant) {
            return Err(StoreError::NotFound(format!("tenant {}", account.tenant)));
        }
        let key = (account.tenant.clone(), account.login.clone());
        if inner.accounts.contains_key(&key) {
            return Ok(false);
        }
        inner.accounts.insert(key, account.clone());
        Ok(true)
    }

    fn update_account(&self, expected: &Account, replacement: &Account) -> StoreResult<bool> {
        if expected.tenant != replacement.tenant || expected.login != replacement.login {
            return Err(StoreError::Invalid(
                "an account update keeps its tenant and login".into(),
            ));
        }
        let mut inner = self.lock()?;
        let key = (expected.tenant.clone(), expected.login.clone());
        match inner.accounts.get_mut(&key) {
            Some(current) if current == expected => {
                *current = replacement.clone();
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn account(&self, tenant: &TenantId, login: &str) -> StoreResult<Option<Account>> {
        Ok(self
            .lock()?
            .accounts
            .get(&(tenant.clone(), login.to_string()))
            .cloned())
    }

    fn remove_account(&self, tenant: &TenantId, login: &str) -> StoreResult<bool> {
        Ok(self
            .lock()?
            .accounts
            .remove(&(tenant.clone(), login.to_string()))
            .is_some())
    }

    fn anonymise_ledger_principal(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<strata_kernel::ledger::Anonymised> {
        let mut inner = self.lock()?;
        let Some(state) = inner.tenants.get_mut(tenant) else {
            return Ok(Default::default());
        };
        let state = Arc::make_mut(state);
        Ok(match state.ledger.as_mut() {
            Some(ledger) => strata_kernel::ledger::anonymise(ledger, principal),
            None => Default::default(),
        })
    }

    fn remove_principal(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<PrincipalRemoval> {
        // Everything is removed under the one lock, so no sign-in sees half of it.
        let mut inner = self.lock()?;
        let inner = &mut *inner;
        let mut removed = PrincipalRemoval::default();
        fn drop_where<K: Ord + Clone, V>(
            rows: &mut BTreeMap<K, V>,
            doomed: impl Fn(&K, &V) -> bool,
        ) -> u64 {
            let before = rows.len();
            rows.retain(|key, value| !doomed(key, value));
            (before - rows.len()) as u64
        }
        removed.accounts = drop_where(&mut inner.accounts, |(t, _), a| {
            t == tenant && a.principal == *principal
        });
        removed.passkeys = drop_where(&mut inner.passkeys, |(t, _), p| {
            t == tenant && p.principal == *principal
        });
        removed.passkey_challenges = drop_where(&mut inner.challenges, |(t, _), c| {
            t == tenant && c.principal.as_ref() == Some(principal)
        });
        removed.provider_flows = drop_where(&mut inner.flows, |(t, _), f| {
            t == tenant && f.principal.as_ref() == Some(principal)
        }) + drop_where(&mut inner.saml_requests, |(t, _), r| {
            t == tenant && r.principal.as_ref() == Some(principal)
        });
        removed.identities = drop_where(&mut inner.identities, |(t, _, _), i| {
            t == tenant && i.principal == *principal
        });
        removed.totp_factors =
            drop_where(&mut inner.totp, |(t, p), _| t == tenant && p == principal);
        removed.recovery_codes = inner
            .recovery_codes
            .remove(&(tenant.clone(), principal.clone()))
            .map_or(0, |codes| codes.len() as u64);
        removed.pending_sign_ins = drop_where(&mut inner.pending, |(t, _), p| {
            t == tenant && p.principal == *principal
        });
        removed.sign_in_links = drop_where(&mut inner.links, |(t, _), l| {
            t == tenant && l.principal == *principal
        });
        removed.recovery_requests = drop_where(&mut inner.recoveries, |(t, _), r| {
            t == tenant && r.principal == *principal
        });
        Ok(removed)
    }

    fn doc_put(
        &self,
        tenant: &TenantId,
        kind: &str,
        key: &str,
        value: &str,
        create_only: bool,
    ) -> StoreResult<bool> {
        let mut inner = self.lock()?;
        if !inner.tenants.contains_key(tenant) {
            return Err(StoreError::NotFound(format!("tenant {tenant}")));
        }
        let slot = (tenant.clone(), kind.to_string(), key.to_string());
        if create_only && inner.docs.contains_key(&slot) {
            return Ok(false);
        }
        inner.docs.insert(slot, value.to_string());
        Ok(true)
    }

    fn doc_get(&self, tenant: &TenantId, kind: &str, key: &str) -> StoreResult<Option<String>> {
        Ok(self
            .lock()?
            .docs
            .get(&(tenant.clone(), kind.to_string(), key.to_string()))
            .cloned())
    }

    fn doc_list(&self, tenant: &TenantId, kind: &str) -> StoreResult<Vec<(String, String)>> {
        Ok(self
            .lock()?
            .docs
            .iter()
            .filter(|((t, k, _), _)| t == tenant && k == kind)
            .map(|((_, _, key), value)| (key.clone(), value.clone()))
            .collect())
    }

    fn doc_remove(&self, tenant: &TenantId, kind: &str, key: &str) -> StoreResult<bool> {
        Ok(self
            .lock()?
            .docs
            .remove(&(tenant.clone(), kind.to_string(), key.to_string()))
            .is_some())
    }

    fn audit_append(&self, tenant: &TenantId, entry: &AuditEntry) -> StoreResult<()> {
        let mut inner = self.lock()?;
        if !inner.tenants.contains_key(tenant) {
            return Err(StoreError::NotFound(format!("tenant {tenant}")));
        }
        inner
            .audit
            .entry(tenant.clone())
            .or_default()
            .push(entry.clone());
        Ok(())
    }

    fn audit_list(
        &self,
        tenant: &TenantId,
        kind: Option<&str>,
        limit: usize,
    ) -> StoreResult<Vec<AuditEntry>> {
        let inner = self.lock()?;
        let mut lines: Vec<AuditEntry> = inner
            .audit
            .get(tenant)
            .map(|lines| {
                lines
                    .iter()
                    .filter(|line| kind.is_none_or(|k| line.kind == k))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let excess = lines.len().saturating_sub(limit);
        lines.drain(..excess);
        Ok(lines)
    }

    fn verified_address(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
    ) -> StoreResult<Option<String>> {
        Ok(self
            .lock()?
            .accounts
            .values()
            .find(|account| account.tenant == *tenant && account.principal == *principal)
            .and_then(|account| account.verified_email.clone()))
    }

    fn create_snapshot(
        &self,
        tenant: &TenantId,
        label: &str,
        at: Timestamp,
    ) -> StoreResult<SnapshotInfo> {
        let mut inner = self.lock()?;
        let state = inner
            .tenants
            .get(tenant)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(format!("tenant {tenant}")))?;
        let id = inner
            .snapshots
            .keys()
            .filter(|(t, _)| t == tenant)
            .map(|(_, id)| *id)
            .max()
            .unwrap_or(0)
            + 1;
        let info = SnapshotInfo {
            tenant: tenant.clone(),
            id,
            label: label.to_string(),
            revision: state.revision,
            hash: canon::hash_of(&*state),
            created_at: at,
        };
        inner
            .snapshots
            .insert((tenant.clone(), id), (info.clone(), state));
        Ok(info)
    }

    fn snapshots(&self, tenant: &TenantId) -> StoreResult<Vec<SnapshotInfo>> {
        Ok(self
            .lock()?
            .snapshots
            .iter()
            .filter(|((t, _), _)| t == tenant)
            .map(|(_, (info, _))| info.clone())
            .collect())
    }

    fn load_snapshot(&self, tenant: &TenantId, id: u64) -> StoreResult<TenantState> {
        self.lock()?
            .snapshots
            .get(&(tenant.clone(), id))
            .map(|(_, state)| TenantState::clone(state))
            .ok_or_else(|| StoreError::NotFound(format!("snapshot {id} of {tenant}")))
    }

    fn restore_snapshot(&self, tenant: &TenantId, id: u64) -> StoreResult<()> {
        let mut inner = self.lock()?;
        let mut state = inner
            .snapshots
            .get(&(tenant.clone(), id))
            .map(|(_, state)| state.clone())
            .ok_or_else(|| StoreError::NotFound(format!("snapshot {id} of {tenant}")))?;
        // Ledger entries are never removed (LDG-022): a snapshot older than an entry is not
        // restored.
        let entries = |t: &TenantState| {
            t.ledger
                .as_ref()
                .map_or(0, |l| l.entries.values().map(Vec::len).sum::<usize>())
        };
        if inner.tenants.get(tenant).map_or(0, |t| entries(t)) > entries(&state) {
            return Err(StoreError::Invalid(
                "the ledger holds entries posted after the snapshot, and entries are never removed"
                    .into(),
            ));
        }
        // Event sequence numbers are never reused: the events stay and the counter does not go
        // back, so a subscriber that has passed a number still sees every later event.
        let seen = inner.tenants.get(tenant).map_or(0, |t| t.event_seq);
        if seen > state.event_seq {
            Arc::make_mut(&mut state).event_seq = seen;
        }
        inner
            .journal
            .retain(|entry| entry.tenant != *tenant || entry.revision <= state.revision);
        inner.tenants.insert(tenant.clone(), state);
        self.epoch.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn reset(&self) -> StoreResult<()> {
        let mut inner = self.lock()?;
        // The counters are kept: a counter that fell back could repeat a value for other records.
        let counters = std::mem::take(&mut inner.object_revisions);
        *inner = Inner::default();
        inner.object_revisions = counters;
        self.epoch.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn reset_tenant(&self, tenant: &TenantId) -> StoreResult<()> {
        let mut inner = self.lock()?;
        self.epoch.fetch_add(1, Ordering::Relaxed);
        inner.tenants.remove(tenant);
        inner.positions.remove(tenant);
        inner.events.retain(|(_, event, _)| event.tenant != *tenant);
        inner.journal.retain(|entry| entry.tenant != *tenant);
        inner.idempotency.retain(|(t, _, _), _| t != tenant);
        inner.sessions.retain(|_, s| s.tenant != *tenant);
        inner.accounts.retain(|(t, _), _| t != tenant);
        inner.throttle.retain(|(t, _), _| t != tenant);
        inner.passkeys.retain(|(t, _), _| t != tenant);
        inner.challenges.retain(|(t, _), _| t != tenant);
        inner.links.retain(|(t, _), _| t != tenant);
        inner.flows.retain(|(t, _), _| t != tenant);
        inner.saml_requests.retain(|(t, _), _| t != tenant);
        inner.saml_assertions.retain(|(t, _, _), _| t != tenant);
        inner.identities.retain(|(t, _, _), _| t != tenant);
        inner.totp.retain(|(t, _), _| t != tenant);
        inner.recovery_codes.retain(|(t, _), _| t != tenant);
        inner.pending.retain(|(t, _), _| t != tenant);
        inner.recoveries.retain(|(t, _), _| t != tenant);
        inner.docs.retain(|(t, _, _), _| t != tenant);
        inner.audit.remove(tenant);
        inner.snapshots.retain(|(t, _), _| t != tenant);
        inner.jobs.remove(tenant);
        inner.runs.remove(tenant);
        inner.steps.retain(|(t, _, _), _| t != tenant);
        inner.exceptions.remove(tenant);
        inner.segments.remove(tenant);
        Ok(())
    }

    fn jobs_create(
        &self,
        tenant: &TenantId,
        new: &[NewJob],
        now: Timestamp,
    ) -> StoreResult<Vec<String>> {
        check_jobs(new)?;
        let mut inner = self.lock()?;
        if !inner.tenants.contains_key(tenant) {
            return Err(StoreError::NotFound(format!("tenant {tenant}")));
        }
        Ok(inner.add_jobs(tenant, new, now))
    }

    fn job(&self, tenant: &TenantId, id: &str) -> StoreResult<Option<Job>> {
        Ok(self
            .lock()?
            .jobs
            .get(tenant)
            .and_then(|list| list.iter().find(|job| job.id == id).cloned()))
    }

    fn jobs_list(
        &self,
        tenant: &TenantId,
        before: Option<u64>,
        limit: usize,
    ) -> StoreResult<Vec<Job>> {
        Ok(self
            .lock()?
            .jobs
            .get(tenant)
            .map(|list| {
                list.iter()
                    .rev()
                    .filter(|job| before.is_none_or(|b| job.seq < b))
                    .take(limit)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    fn job_lease(
        &self,
        tenant: &TenantId,
        worker: &str,
        now: Timestamp,
    ) -> StoreResult<Option<Lease>> {
        let mut inner = self.lock()?;
        let Some(list) = inner.jobs.get_mut(tenant) else {
            return Ok(None);
        };
        let Some(job) = list
            .iter_mut()
            .filter(|job| jobs::leasable(job, now))
            .min_by_key(|job| (job.due_at, job.seq))
        else {
            return Ok(None);
        };
        let lease = jobs::apply_lease(job, worker, now);
        *job = lease.job.clone();
        Ok(Some(lease))
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
        let mut inner = self.lock()?;
        let Some(job) = inner.job_mut(tenant, id) else {
            return Ok(Reported::Refused(format!("no job {id}")));
        };
        let next = match jobs::apply_report(job, token, report, now) {
            Ok(next) => next,
            Err(why) => return Ok(Reported::Refused(why)),
        };
        *job = next.clone();
        inner.write(tenant, writes, now);
        Ok(Reported::Recorded(next))
    }

    fn job_cancel(
        &self,
        tenant: &TenantId,
        id: &str,
        reason: &str,
        now: Timestamp,
    ) -> StoreResult<JobChange> {
        let mut inner = self.lock()?;
        let Some(job) = inner.job_mut(tenant, id) else {
            return Ok(JobChange::NotFound);
        };
        let change = jobs::apply_cancel(job, reason, now);
        if let JobChange::Changed(next) = &change {
            *job = next.clone();
        }
        Ok(change)
    }

    fn job_retry(&self, tenant: &TenantId, id: &str, now: Timestamp) -> StoreResult<JobChange> {
        let mut inner = self.lock()?;
        let Some(job) = inner.job_mut(tenant, id) else {
            return Ok(JobChange::NotFound);
        };
        let change = jobs::apply_retry(job, now);
        if let JobChange::Changed(next) = &change {
            *job = next.clone();
        }
        Ok(change)
    }

    fn jobs_due(&self, now: Timestamp, limit: usize) -> StoreResult<Vec<TenantId>> {
        // In memory every tenant is at hand: the earliest due job of each tenant is its wake time.
        let inner = self.lock()?;
        let mut due: Vec<(Timestamp, &TenantId)> = inner
            .jobs
            .iter()
            .filter(|(tenant, _)| inner.tenants.contains_key(*tenant))
            .filter_map(|(tenant, list)| {
                list.iter()
                    .filter(|job| jobs::leasable(job, now))
                    .map(|job| job.due_at)
                    .min()
                    .map(|at| (at, tenant))
            })
            .collect();
        due.sort();
        Ok(due
            .into_iter()
            .take(limit)
            .map(|(_, tenant)| tenant.clone())
            .collect())
    }

    fn automation_write(
        &self,
        tenant: &TenantId,
        writes: &WorkerWrites,
        now: Timestamp,
    ) -> StoreResult<()> {
        check_jobs(&writes.jobs)?;
        let mut inner = self.lock()?;
        if !inner.tenants.contains_key(tenant) {
            return Err(StoreError::NotFound(format!("tenant {tenant}")));
        }
        inner.write(tenant, writes, now);
        Ok(())
    }

    fn automation_run(&self, tenant: &TenantId, run: &str) -> StoreResult<Option<RunRecord>> {
        Ok(self
            .lock()?
            .runs
            .get(tenant)
            .and_then(|list| list.iter().find(|held| held.run == run).cloned()))
    }

    fn automation_runs(
        &self,
        tenant: &TenantId,
        automation: Option<&WorkflowId>,
        state: Option<RunState>,
        before: Option<u64>,
        limit: usize,
    ) -> StoreResult<Vec<RunRecord>> {
        Ok(self
            .lock()?
            .runs
            .get(tenant)
            .map(|list| {
                list.iter()
                    .rev()
                    .filter(|run| before.is_none_or(|b| run.seq < b))
                    .filter(|run| automation.is_none_or(|a| run.automation == *a))
                    .filter(|run| state.is_none_or(|s| run.state == s))
                    .take(limit)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    fn automation_steps(
        &self,
        tenant: &TenantId,
        run: &str,
        after: Option<u64>,
        limit: usize,
    ) -> StoreResult<Vec<StepRecord>> {
        Ok(self
            .lock()?
            .steps
            .range(
                (
                    tenant.clone(),
                    run.to_string(),
                    after.map_or(0, |a| a.saturating_add(1)),
                )..=(tenant.clone(), run.to_string(), u64::MAX),
            )
            .take(limit)
            .map(|(_, step)| step.clone())
            .collect())
    }

    fn automation_exceptions(
        &self,
        tenant: &TenantId,
        kind: Option<ExceptionKind>,
        unsettled: bool,
        before: Option<u64>,
        limit: usize,
    ) -> StoreResult<Vec<ExceptionRecord>> {
        Ok(self
            .lock()?
            .exceptions
            .get(tenant)
            .map(|list| {
                list.iter()
                    .rev()
                    .filter(|record| before.is_none_or(|b| record.seq < b))
                    .filter(|record| kind.is_none_or(|k| record.kind == k))
                    .filter(|record| !unsettled || !record.settled)
                    .take(limit)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    fn automation_exception_settle(
        &self,
        tenant: &TenantId,
        id: &str,
        release: bool,
        now: Timestamp,
    ) -> StoreResult<Settled> {
        let mut inner = self.lock()?;
        let Some(record) = inner
            .exceptions
            .get(tenant)
            .and_then(|list| list.iter().find(|record| record.id == id))
            .cloned()
        else {
            return Ok(Settled::NotFound);
        };
        if let Some(why) = jobs::settle_problem(&record, release) {
            return Ok(Settled::Conflict(why));
        }
        let mut released = None;
        if release {
            let job_id = record.job.clone().unwrap_or_default();
            let Some(job) = inner.job_mut(tenant, &job_id) else {
                return Ok(Settled::Conflict(format!("no job {job_id} to release")));
            };
            match jobs::apply_retry(job, now) {
                JobChange::Changed(next) => *job = next,
                _ => {
                    return Ok(Settled::Conflict(format!(
                        "job {job_id} has not failed and cannot be given another attempt"
                    )))
                }
            }
            released = Some(job_id);
        }
        if let Some(held) = inner
            .exceptions
            .get_mut(tenant)
            .and_then(|list| list.iter_mut().find(|record| record.id == id))
        {
            held.settled = true;
        }
        Ok(Settled::Done { job: released })
    }
}

/// Apply one commit, with the segment write its change decided when it has one, under the lock:
/// everything or nothing.
fn commit_in(
    inner: &mut Inner,
    commit: &Commit,
    write: Option<&seg::SegmentWrite>,
) -> StoreResult<Receipt> {
    let tenant = &commit.delta.tenant;
    let current = inner.tenants.get(tenant).cloned();
    let found = current.as_ref().map(|t| t.revision);
    if found != commit.delta.base_revision {
        return Err(StoreError::Conflict {
            expected: commit.delta.base_revision,
            found,
        });
    }
    check_event_sequence(commit, current.as_ref().map_or(0, |t| t.event_seq))?;
    let recorded_before = commit.journal.context.now.0 - IDEMPOTENCY_RETENTION_MS;
    if let Some(entry) = &commit.idempotency {
        let scope = (tenant.clone(), entry.principal.clone(), entry.key.clone());
        if inner
            .idempotency
            .get(&scope)
            .is_some_and(|(held, _)| held.recorded_at.0 >= recorded_before)
        {
            return Err(StoreError::Invalid(
                "the idempotency key is already recorded".into(),
            ));
        }
    }
    check_jobs(&commit.jobs)?;
    let next = apply_delta(current.as_deref(), &commit.delta)
        .map_err(|e| StoreError::Invalid(strata_kernel::ext::DomainErrorExt::describe(&e)))?;
    if let Some(write) = write {
        if let Some(tables) = inner.segments.get(tenant) {
            tables
                .check(write)
                .map_err(|_| crate::segments::write_conflict())?;
        } else if !write.is_empty() {
            seg::SegmentTables::default()
                .check(write)
                .map_err(|_| crate::segments::write_conflict())?;
        }
    }

    // Nothing above changed the store; everything below cannot fail.
    inner.tenants.insert(tenant.clone(), Arc::new(next));
    for object in crate::changed_objects(&commit.delta) {
        *inner
            .object_revisions
            .entry((tenant.clone(), object))
            .or_insert(0) += 1;
    }
    if let Some(write) = write.filter(|w| !w.is_empty()) {
        let tables = inner.segments.entry(tenant.clone()).or_default();
        // Checked above, under the same lock.
        let _ = tables.apply(write);
    }
    let now = commit.journal.context.now;
    inner.add_jobs(tenant, &commit.jobs, now);
    inner.add_exceptions(tenant, &commit.exceptions);
    let mut last_position = None;
    for event in &commit.events {
        let next = inner.positions.entry(tenant.clone()).or_insert(0);
        *next += 1;
        let position = *next;
        inner.events.push((position, event.clone(), false));
        last_position = Some(position);
    }
    inner.journal.push(commit.journal.clone());
    let receipt = Receipt {
        revision: commit.delta.new_revision,
        last_position,
    };
    inner
        .idempotency
        .retain(|_, (held, _)| held.recorded_at.0 >= recorded_before);
    if let Some(entry) = &commit.idempotency {
        inner.idempotency.insert(
            (tenant.clone(), entry.principal.clone(), entry.key.clone()),
            (entry.clone(), receipt.clone()),
        );
    }
    for session in inner.sessions.values_mut() {
        if session.tenant == *tenant
            && commit.revoke_sessions.contains(&session.principal)
            && session.revoked_at.is_none()
        {
            session.revoked_at = Some(now);
        }
    }
    Ok(receipt)
}
