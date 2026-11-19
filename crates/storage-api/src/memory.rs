//! The reference adapter: everything in memory, behind one lock.
//!
//! It exists to state the contract in the simplest possible code, to back tests, and to show
//! that the rest of the system does not depend on SQLite. It is not durable and says so.

use crate::{
    check_event_sequence, partial_state, Account, Capabilities, Commit, Health, JournalEntry,
    OutboxEntry, Receipt, SessionRecord, SnapshotInfo, StorageMigration, Store, StoreError,
    StoreResult, TenantSummary,
};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use strata_kernel::ext::*;
use strata_kernel::{
    apply_delta, canon, DomainEvent, Need, SessionId, State, TenantId, TenantState, Timestamp,
};

#[derive(Default)]
struct Inner {
    tenants: BTreeMap<TenantId, Arc<TenantState>>,
    events: Vec<(u64, DomainEvent, bool)>,
    next_position: u64,
    journal: Vec<JournalEntry>,
    sessions: BTreeMap<SessionId, SessionRecord>,
    accounts: BTreeMap<(TenantId, String), Account>,
    snapshots: BTreeMap<(TenantId, u64), (SnapshotInfo, Arc<TenantState>)>,
}

#[derive(Default)]
pub struct MemoryStore {
    inner: Mutex<Inner>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
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
            .map_err(|e| StoreError::Invalid(e.to_string()))?;

        // Nothing above changed the store; everything below cannot fail.
        inner.tenants.insert(tenant.clone(), Arc::new(next));
        let mut last_position = None;
        for event in &commit.events {
            inner.next_position += 1;
            let position = inner.next_position;
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

    fn mark_published(&self, position: u64) -> StoreResult<()> {
        for entry in self
            .lock()?
            .events
            .iter_mut()
            .filter(|(p, _, _)| *p <= position)
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
        inner.events.retain(|(_, event, _)| event.tenant != *tenant);
        inner.journal.retain(|entry| entry.tenant != *tenant);
        inner.sessions.retain(|_, s| s.tenant != *tenant);
        inner.accounts.retain(|(t, _), _| t != tenant);
        inner.snapshots.retain(|(t, _), _| t != tenant);
        Ok(())
    }
}
