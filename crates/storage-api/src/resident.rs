//! Header residency (CPY-014, `docs/working-set.md`, follow-up item 2): the first piece of the
//! per-tenant working set.
//!
//! [`ResidentStore`] wraps a [`Store`] and keeps, per tenant, the decoded header: the schema, the
//! members, the name and the setting values, stamped with the [`Generation`] they were read at
//! and keyed by the schema version and the hash of the schema. The compiled form of the schema
//! hangs off the same entry: the schema is held as one shared `Arc`, so every state served from a
//! hit shares it.
//!
//! # What is served
//!
//! - **Never the counters.** The revision, the record counter and the event sequence are read
//!   from the store on every load (three integers of the tenant row), or taken from the commit's
//!   receipt. A hit rebuilds the header of the state from the held documents and those counters.
//! - **Only at the current generation.** The wrapper does not ask for the generation in a call of
//!   its own. It passes the held header to the adapter ([`Store::load_at_held`]), and the adapter
//!   compares the held generation with the one it reads in the same read transaction as the rows,
//!   and skips decoding the header's documents only when they are equal. A generation changes with
//!   every change to what a load returns, so equal generations mean an equal header, and a
//!   foreign write, a restore, a nonce rotation or a removal makes the next load decode again.
//!
//! # A commit
//!
//! After an accepted commit the header is carried forward to the receipt's generation when the
//! entry was current at the commit's expected generation, the receipt's generation is that one
//! raised by one, and the delta wrote no schema, member, setting or tenant-creation operation.
//! It is dropped otherwise, and so on any error (an unknown outcome assumes no generation). The
//! next load then decodes the header once and holds it again.
//!
//! # Lock scope
//!
//! One lock per tenant runtime, held across the store I/O of a load and of a commit, as the
//! design's lock-scope rule says. Different tenants never share it.
//!
//! # Accounting
//!
//! Resident bytes are an estimate: the length of the canonical JSON of the header's documents
//! plus a fixed overhead per entry. They are not an allocator measurement.

use crate::*;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use strata_kernel::segments as seg;
use strata_kernel::{DeltaOp, RoleId, SettingValues, StateRevision, TenantSchema};

/// A header an adapter may serve from instead of decoding the tenant's documents: what a caller
/// holds for a tenant, with the generation it was read at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldHeader {
    /// The generation the header was read at.
    pub generation: Generation,
    pub name: String,
    pub schema: Arc<TenantSchema>,
    pub members: Arc<BTreeMap<PrincipalId, BTreeSet<RoleId>>>,
    pub setting_values: Option<Arc<SettingValues>>,
}

impl HeldHeader {
    /// The header of a state: the held documents and the counters the store just read. No
    /// records, no ledger.
    pub fn state(
        &self,
        tenant: &TenantId,
        revision: StateRevision,
        next_record: u64,
        event_seq: u64,
    ) -> TenantState {
        TenantState {
            id: tenant.clone(),
            name: self.name.clone(),
            revision,
            next_record,
            event_seq,
            schema: self.schema.clone(),
            members: (*self.members).clone(),
            records: BTreeMap::new(),
            setting_values: self.setting_values.clone(),
            ledger: None,
        }
    }

    fn of(generation: Generation, tenant: &TenantState) -> HeldHeader {
        HeldHeader {
            generation,
            name: tenant.name.clone(),
            schema: tenant.schema.clone(),
            members: Arc::new(tenant.members.clone()),
            setting_values: tenant.setting_values.clone(),
        }
    }
}

/// What [`Store::load_at_held`] and [`Store::load_complete_at_held`] return.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldLoad {
    pub loaded: Loaded,
    /// Whether the adapter decoded the header's stored documents (schema, members, settings) to
    /// answer. `false` when it served them from the held header, or when it keeps them decoded.
    pub header_decoded: bool,
}

/// What a [`ResidentStore`] counts. All read without a lock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResidentStats {
    /// Loads served with the held header: the held generation was the store's.
    pub hits: u64,
    /// Loads that found no entry, or an entry at another generation, or whose adapter decoded the
    /// header anyway.
    pub misses: u64,
    /// Times an adapter decoded a header's documents for a load through this wrapper.
    pub decodes: u64,
    /// Estimated bytes of the headers held now.
    pub resident_bytes: u64,
    /// Tenants with a header held now.
    pub resident_tenants: u64,
}

/// The fixed charge per entry, over the canonical length of its documents.
const ENTRY_OVERHEAD: u64 = 128;

struct Resident {
    held: HeldHeader,
    /// Half of the key besides the generation: the schema's version and the hash of its
    /// canonical form.
    schema_version: u64,
    schema_hash: String,
    bytes: u64,
}

impl Resident {
    fn admit(held: HeldHeader) -> Resident {
        let schema = canon::canonical_json(&*held.schema);
        let rest = canon::canonical_json(&*held.members).len()
            + held
                .setting_values
                .as_ref()
                .map_or(0, |values| canon::canonical_json(&**values).len())
            + held.name.len();
        Resident {
            schema_version: held.schema.version.0,
            schema_hash: canon::hash_bytes(schema.as_bytes()),
            bytes: (schema.len() + rest) as u64 + ENTRY_OVERHEAD,
            held,
        }
    }

    /// The same documents at the generation a commit produced.
    fn carried(&self, generation: Generation) -> Resident {
        Resident {
            held: HeldHeader {
                generation,
                ..self.held.clone()
            },
            schema_version: self.schema_version,
            schema_hash: self.schema_hash.clone(),
            bytes: self.bytes,
        }
    }
}

/// One tenant runtime's slot: its lock, and the header it guards.
type Slot = Arc<Mutex<Option<Resident>>>;

/// A [`Store`] that keeps each tenant's header resident. See the module documentation.
pub struct ResidentStore {
    inner: Arc<dyn Store>,
    slots: Mutex<BTreeMap<TenantId, Slot>>,
    hits: AtomicU64,
    misses: AtomicU64,
    decodes: AtomicU64,
    bytes: AtomicU64,
    tenants: AtomicU64,
}

fn unpoisoned<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Whether a delta writes a part of the header: the tenant row, the schema, a member or a setting.
fn writes_header(delta: &StateDelta) -> bool {
    delta.ops.iter().any(|op| {
        matches!(
            op,
            DeltaOp::CreateTenant { .. }
                | DeltaOp::PutSchema(_)
                | DeltaOp::PutMember { .. }
                | DeltaOp::PutSettingOverride(_)
                | DeltaOp::DeleteSettingOverride { .. }
                | DeltaOp::PutSettingRevision { .. }
        )
    })
}

impl ResidentStore {
    /// Header residency over `inner`: the first stage of the working set.
    pub fn header_only(inner: Arc<dyn Store>) -> ResidentStore {
        ResidentStore {
            inner,
            slots: Mutex::new(BTreeMap::new()),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            decodes: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            tenants: AtomicU64::new(0),
        }
    }

    /// The store underneath.
    pub fn inner(&self) -> &Arc<dyn Store> {
        &self.inner
    }

    /// The counters and the resident bytes now.
    pub fn stats(&self) -> ResidentStats {
        ResidentStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            decodes: self.decodes.load(Ordering::Relaxed),
            resident_bytes: self.bytes.load(Ordering::Relaxed),
            resident_tenants: self.tenants.load(Ordering::Relaxed),
        }
    }

    /// The generation and the key (schema version, schema hash) of the header held for a tenant,
    /// none when none is held.
    pub fn held_key(&self, tenant: &TenantId) -> Option<(Generation, u64, String)> {
        let slot = unpoisoned(&self.slots).get(tenant).cloned()?;
        let entry = unpoisoned(&slot);
        entry
            .as_ref()
            .map(|r| (r.held.generation, r.schema_version, r.schema_hash.clone()))
    }

    fn slot(&self, tenant: &TenantId) -> Slot {
        unpoisoned(&self.slots)
            .entry(tenant.clone())
            .or_default()
            .clone()
    }

    /// Replace what a slot holds, keeping the totals.
    fn set(&self, entry: &mut Option<Resident>, next: Option<Resident>) {
        if let Some(old) = entry.take() {
            self.bytes.fetch_sub(old.bytes, Ordering::Relaxed);
            self.tenants.fetch_sub(1, Ordering::Relaxed);
        }
        if let Some(new) = next {
            self.bytes.fetch_add(new.bytes, Ordering::Relaxed);
            self.tenants.fetch_add(1, Ordering::Relaxed);
            *entry = Some(new);
        }
    }

    /// A load under the tenant's lock, held across the store I/O.
    fn load_held(
        &self,
        tenant: &TenantId,
        run: impl FnOnce(Option<&HeldHeader>) -> StoreResult<HeldLoad>,
    ) -> StoreResult<Loaded> {
        let slot = self.slot(tenant);
        let mut entry = unpoisoned(&slot);
        let held = entry.as_ref().map(|r| r.held.clone());
        let answered = match run(held.as_ref()) {
            Ok(answered) => answered,
            Err(error) => {
                self.set(&mut entry, None);
                return Err(error);
            }
        };
        if answered.header_decoded {
            self.decodes.fetch_add(1, Ordering::Relaxed);
        }
        let loaded = answered.loaded;
        let matched = held
            .as_ref()
            .is_some_and(|h| Some(h.generation) == loaded.generation);
        if matched && !answered.header_decoded {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Ok(loaded);
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        let next = match (&loaded.state.tenant, loaded.generation) {
            (Some(tenant), Some(generation)) => {
                Some(Resident::admit(HeldHeader::of(generation, tenant)))
            }
            _ => None,
        };
        self.set(&mut entry, next);
        Ok(loaded)
    }

    /// A commit under the tenant's lock, held across the store write and the carry-forward.
    fn committed(
        &self,
        commit: &Commit,
        run: impl FnOnce() -> StoreResult<Receipt>,
    ) -> StoreResult<Receipt> {
        let slot = self.slot(&commit.delta.tenant);
        let mut entry = unpoisoned(&slot);
        match run() {
            Ok(receipt) => {
                let next = match (entry.as_ref(), commit.expected_generation) {
                    (Some(held), Some(expected)) if held.held.generation == expected => {
                        if receipt.generation == expected {
                            // A replayed result: nothing was written.
                            return Ok(receipt);
                        }
                        (receipt.generation == expected.raised() && !writes_header(&commit.delta))
                            .then(|| held.carried(receipt.generation))
                    }
                    _ => None,
                };
                self.set(&mut entry, next);
                Ok(receipt)
            }
            Err(error) => {
                self.set(&mut entry, None);
                Err(error)
            }
        }
    }
}

impl Store for ResidentStore {
    fn load_at(&self, tenant: &TenantId, needs: &[Need]) -> StoreResult<Loaded> {
        self.load_held(tenant, |held| self.inner.load_at_held(tenant, needs, held))
    }

    fn load_complete_at(&self, tenant: &TenantId) -> StoreResult<Loaded> {
        self.load_held(tenant, |held| {
            self.inner.load_complete_at_held(tenant, held)
        })
    }

    fn commit(&self, commit: &Commit) -> StoreResult<Receipt> {
        self.committed(commit, || self.inner.commit(commit))
    }

    fn commit_with_segments(
        &self,
        commit: &Commit,
        write: &seg::SegmentWrite,
    ) -> StoreResult<Receipt> {
        self.committed(commit, || self.inner.commit_with_segments(commit, write))
    }

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

    fn generation(&self, tenant: &TenantId) -> StoreResult<Option<Generation>> {
        self.inner.generation(tenant)
    }

    fn rotate_nonce(&self, tenant: &TenantId) -> StoreResult<Generation> {
        self.inner.rotate_nonce(tenant)
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
        expected: Option<&Generation>,
    ) -> StoreResult<()> {
        self.inner.restore_snapshot(tenant, id, expected)
    }

    fn reset(&self) -> StoreResult<()> {
        self.inner.reset()
    }

    fn reset_tenant(&self, tenant: &TenantId) -> StoreResult<()> {
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
