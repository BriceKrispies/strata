//! The reference adapter: everything in memory, behind one lock.
//!
//! It exists to state the contract in the simplest possible code, to back tests, and to show
//! that the rest of the system does not depend on SQLite. It is not durable and says so.

use crate::{
    check_event_sequence, partial_state, throttle_refund, throttle_step, Account, Capabilities,
    Commit, FederatedIdentity, Health, JournalEntry, OidcFlow, OutboxEntry, PasskeyChallenge,
    PasskeyRecord, PendingSignIn, Receipt, RecoveryRequest, SamlRequest, SessionRecord, SignInLink,
    SnapshotInfo, StorageMigration, Store, StoreError, StoreResult, TenantSummary, ThrottleRow,
    ThrottleRule, ThrottleSettle, ThrottleVerdict, TotpRecord,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use strata_kernel::ext::*;
use strata_kernel::{
    apply_delta, canon, DomainEvent, Need, PrincipalId, SessionId, State, TenantId, TenantState,
    Timestamp,
};

#[derive(Default)]
struct Inner {
    tenants: BTreeMap<TenantId, Arc<TenantState>>,
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
    snapshots: BTreeMap<(TenantId, u64), (SnapshotInfo, Arc<TenantState>)>,
}

#[derive(Default)]
pub struct MemoryStore {
    inner: Mutex<Inner>,
    loads: AtomicUsize,
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
        let next = apply_delta(current.as_deref(), &commit.delta)
            .map_err(|e| StoreError::Invalid(strata_kernel::ext::DomainErrorExt::describe(&e)))?;

        // Nothing above changed the store; everything below cannot fail.
        inner.tenants.insert(tenant.clone(), Arc::new(next));
        let mut last_position = None;
        for event in &commit.events {
            let next = inner.positions.entry(tenant.clone()).or_insert(0);
            *next += 1;
            let position = *next;
            inner.events.push((position, event.clone(), false));
            last_position = Some(position);
        }
        inner.journal.push(commit.journal.clone());
        let now = commit.journal.context.now;
        for session in inner.sessions.values_mut() {
            if session.tenant == *tenant
                && commit.revoke_sessions.contains(&session.principal)
                && session.revoked_at.is_none()
            {
                session.revoked_at = Some(now);
            }
        }
        Ok(Receipt {
            revision: commit.delta.new_revision,
            last_position,
        })
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
        Ok(self
            .lock()?
            .events
            .iter()
            .filter(|(_, _, published)| !published)
            .take(limit)
            .map(|(position, event, _)| OutboxEntry {
                position: *position,
                event: event.clone(),
            })
            .collect())
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
        let state = inner
            .snapshots
            .get(&(tenant.clone(), id))
            .map(|(_, state)| state.clone())
            .ok_or_else(|| StoreError::NotFound(format!("snapshot {id} of {tenant}")))?;
        inner
            .events
            .retain(|(_, event, _)| event.tenant != *tenant || event.seq <= state.event_seq);
        inner
            .journal
            .retain(|entry| entry.tenant != *tenant || entry.revision <= state.revision);
        inner.tenants.insert(tenant.clone(), state);
        Ok(())
    }

    fn reset(&self) -> StoreResult<()> {
        *self.lock()? = Inner::default();
        Ok(())
    }

    fn reset_tenant(&self, tenant: &TenantId) -> StoreResult<()> {
        let mut inner = self.lock()?;
        inner.tenants.remove(tenant);
        inner.positions.remove(tenant);
        inner.events.retain(|(_, event, _)| event.tenant != *tenant);
        inner.journal.retain(|entry| entry.tenant != *tenant);
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
        inner.snapshots.retain(|(t, _), _| t != tenant);
        Ok(())
    }
}
