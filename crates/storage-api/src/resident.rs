//! The per-tenant working set (CPY-014, `docs/working-set.md`, follow-up items 2 to 4): the header
//! and the single records of each tenant, held below sealing.
//!
//! [`ResidentStore`] wraps a [`Store`] and keeps, per tenant, the decoded header (the schema, the
//! members, the name and the setting values) and a set of record entries, each stamped with the
//! [`Generation`] it was read at. The header is keyed by the schema version and the hash of the
//! schema, and the compiled form of the schema hangs off it: the schema is held as one shared
//! `Arc`, so every state served from a hit shares it.
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
//! - **Records, present or absent.** A [`Need::Record`] is answered from an entry when the entry's
//!   stamp is the generation the load reads in its own transaction. The seam is the same
//!   `load_at_held`: the wrapper asks the adapter only for the needs it holds no entry for (with no
//!   needs at all, a load reads the header and the counters), and compares the generation that
//!   load returns with the entries' stamp. If they are equal, the entries are served beside what
//!   was read. If they differ, no entry is served: all are dropped and the whole request is read
//!   again at the generation that read finds, which is a snapshot by itself (the request round
//!   that compares its load's generation and rereads on a change). The state returned is the one
//!   an unwrapped store returns at that generation, with the coverage the caller asked for.
//! - **Answers are not resident.** `ByField`, `ByFields`, `AllOf`, `PageOf`, `IndexPage`,
//!   `PairPage` and `Ledger` always go to the store (follow-up item 5 adds them), and so does a
//!   complete load. No key is made of a value of a field, so the plaintext exclusion for the
//!   lookups of a personal field holds trivially: a resident key is a tenant, an object type and
//!   a record id.
//!
//! # A commit
//!
//! The store guards the commit with the expected generation and the base revision in its write
//! transaction. After an accepted commit the wrapper checks the receipt: entries are confirmed
//! only when they were current at the commit's expected generation and the receipt's generation
//! is that one raised by one (a replayed result, whose receipt carries the expected generation
//! itself, wrote nothing and leaves everything). They are then carried forward to the receipt's
//! generation, minus the invalidation map of the delta: the key of every record the delta writes
//! (the records of a delete's clearing are named in the delta as writes, like any other), and
//! everything when the delta creates the tenant or writes a schema, which is not proved untouched
//! by a record key. The header is carried when the delta wrote no schema, member, setting or
//! tenant-creation operation. On any error, a lost or ambiguous outcome, the tenant is dropped and
//! no generation is assumed.
//!
//! # Lock scope and coalescing
//!
//! One lock per tenant runtime, held across the store I/O of a load (the miss load and a reread
//! included) and of a commit with its carry-forward, as the design's lock-scope rule says. Different
//! tenants never share it. Simultaneous misses on one key (CPY-012) wait for the lock and then
//! find the entry the first one admitted: one load per key per generation.
//!
//! # Placement
//!
//! Below [`crate::sealed::SealedStore`]: the entries are the stored form, with sealed tokens. The
//! sealed store opens every token on every load, so a destroyed subject key (a document write that
//! raises no generation) makes the next read return the erased token. Key documents are reached
//! through `doc_*` calls, which are never served from here, and the cipher session caches nothing.
//!
//! # Accounting
//!
//! Resident bytes are an estimate: the length of the canonical JSON of what is held plus a fixed
//! overhead per entry. They are not an allocator measurement. A byte budget bounds the header and
//! record entries of all tenants together and is never exceeded: an entry is admitted only after
//! its bytes are reserved, and room is made by CLOCK eviction of record entries (the header is
//! pinned), first from the tenant being served and then from the other tenants whose lock is free.
//! An entry that does not fit is not admitted.

use crate::*;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use strata_kernel::segments as seg;
use strata_kernel::{
    DeltaOp, ObjectTypeId, RecordId, RoleId, SettingValues, StateRevision, TenantSchema,
};

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
    /// Estimated bytes held now: headers and records.
    pub resident_bytes: u64,
    /// Tenants with a header held now.
    pub resident_tenants: u64,
    /// Record needs answered from an entry (a present record or a held absence).
    pub record_hits: u64,
    /// Record needs that went to the store, or whose entry could not be served because the load
    /// found another generation.
    pub record_misses: u64,
    /// An estimate of what the record hits saved: the estimated bytes of the entries served.
    pub hit_saved_bytes: u64,
    /// What the record hits saved, in records (an absence counts one row too).
    pub hit_saved_rows: u64,
    /// Record entries held now.
    pub resident_records: u64,
    /// Record entries removed to make room.
    pub evictions: u64,
    /// Record entries dropped because a commit wrote them, or because the tenant was dropped or
    /// found at another generation.
    pub invalidations: u64,
    /// Requests that were read again because an entry's stamp was not the generation the load found.
    pub rereads: u64,
}

/// The fixed charge per header entry, over the canonical length of its documents.
const ENTRY_OVERHEAD: u64 = 128;

/// The fixed charge per record entry, over the canonical length of the record.
const RECORD_OVERHEAD: u64 = 64;

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
}

/// The key of a record entry within a tenant.
pub type RecordKey = (ObjectTypeId, RecordId);

/// A record entry: the record, or `None` for a held absence. Valid at the tenant's stamp.
struct RecordEntry {
    record: Option<Arc<Record>>,
    bytes: u64,
    /// The CLOCK bit: set on admission and on every hit, cleared as the hand passes.
    referenced: bool,
}

fn entry_bytes(key: &RecordKey, record: Option<&Record>) -> u64 {
    let size = match record {
        Some(record) => canon::canonical_json(record).len(),
        None => key.0.as_str().len() + 8,
    };
    size as u64 + RECORD_OVERHEAD
}

/// Everything held for one tenant, behind the tenant runtime's lock.
#[derive(Default)]
struct Tenant {
    header: Option<Resident>,
    /// The generation every record entry is valid at. `None` only while `records` is empty.
    stamp: Option<Generation>,
    records: BTreeMap<RecordKey, RecordEntry>,
    /// The CLOCK hand: the key last examined.
    hand: Option<RecordKey>,
}

/// One tenant runtime's slot: its lock, and what it guards.
type Slot = Arc<Mutex<Tenant>>;

/// A [`Store`] that keeps each tenant's header and single records resident. See the module
/// documentation.
pub struct ResidentStore {
    inner: Arc<dyn Store>,
    slots: Mutex<BTreeMap<TenantId, Slot>>,
    /// Whether records are held at all (not in [`ResidentStore::header_only`]).
    records: bool,
    /// The most accounted bytes the store holds, headers and records together.
    budget: u64,
    hits: AtomicU64,
    misses: AtomicU64,
    decodes: AtomicU64,
    bytes: AtomicU64,
    tenants: AtomicU64,
    record_hits: AtomicU64,
    record_misses: AtomicU64,
    saved_bytes: AtomicU64,
    saved_rows: AtomicU64,
    resident_records: AtomicU64,
    evictions: AtomicU64,
    invalidations: AtomicU64,
    rereads: AtomicU64,
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

/// The record entries a delta invalidates: the invalidation map of the working set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Invalidated {
    /// Every record entry of the tenant: the delta creates the tenant or writes the schema.
    All,
    /// These keys, and no other: every record the delta writes or deletes. A delete's clearing of
    /// references writes the referrers, and the delta names each of them as a write.
    Keys(BTreeSet<RecordKey>),
}

/// The invalidation map. The match is exhaustive, so a new kind of operation must be placed here.
pub fn invalidated_by(delta: &StateDelta) -> Invalidated {
    let mut keys = BTreeSet::new();
    for op in &delta.ops {
        match op {
            DeltaOp::CreateTenant { .. } | DeltaOp::PutSchema(_) => return Invalidated::All,
            DeltaOp::PutRecord(record) => {
                keys.insert((record.object.clone(), record.id));
            }
            DeltaOp::DeleteRecord { object, id } => {
                keys.insert((object.clone(), *id));
            }
            // Members, settings and the ledger are not records and are not read through a record
            // need.
            DeltaOp::PutMember { .. }
            | DeltaOp::PutSettingOverride(_)
            | DeltaOp::DeleteSettingOverride { .. }
            | DeltaOp::PutSettingRevision { .. }
            | DeltaOp::Ledger(_) => {}
        }
    }
    Invalidated::Keys(keys)
}

/// What a read asks for.
#[derive(Clone, Copy)]
enum Read<'a> {
    Needs(&'a [Need]),
    Complete,
}

impl ResidentStore {
    /// The working set over `inner` with a byte budget: the header and the single records of every
    /// tenant, together at most `budget_bytes` (estimated bytes).
    pub fn new(inner: Arc<dyn Store>, budget_bytes: u64) -> ResidentStore {
        ResidentStore::build(inner, budget_bytes, true)
    }

    /// Header residency over `inner`, unbounded: the first stage of the working set.
    pub fn header_only(inner: Arc<dyn Store>) -> ResidentStore {
        ResidentStore::build(inner, u64::MAX, false)
    }

    fn build(inner: Arc<dyn Store>, budget: u64, records: bool) -> ResidentStore {
        ResidentStore {
            inner,
            slots: Mutex::new(BTreeMap::new()),
            records,
            budget,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            decodes: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            tenants: AtomicU64::new(0),
            record_hits: AtomicU64::new(0),
            record_misses: AtomicU64::new(0),
            saved_bytes: AtomicU64::new(0),
            saved_rows: AtomicU64::new(0),
            resident_records: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            invalidations: AtomicU64::new(0),
            rereads: AtomicU64::new(0),
        }
    }

    /// The store underneath.
    pub fn inner(&self) -> &Arc<dyn Store> {
        &self.inner
    }

    /// The byte budget.
    pub fn budget(&self) -> u64 {
        self.budget
    }

    /// The counters and the resident bytes now.
    pub fn stats(&self) -> ResidentStats {
        let get = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        ResidentStats {
            hits: get(&self.hits),
            misses: get(&self.misses),
            decodes: get(&self.decodes),
            resident_bytes: get(&self.bytes),
            resident_tenants: get(&self.tenants),
            record_hits: get(&self.record_hits),
            record_misses: get(&self.record_misses),
            hit_saved_bytes: get(&self.saved_bytes),
            hit_saved_rows: get(&self.saved_rows),
            resident_records: get(&self.resident_records),
            evictions: get(&self.evictions),
            invalidations: get(&self.invalidations),
            rereads: get(&self.rereads),
        }
    }

    /// The generation and the key (schema version, schema hash) of the header held for a tenant,
    /// none when none is held.
    pub fn held_key(&self, tenant: &TenantId) -> Option<(Generation, u64, String)> {
        let slot = unpoisoned(&self.slots).get(tenant).cloned()?;
        let entry = unpoisoned(&slot);
        entry
            .header
            .as_ref()
            .map(|r| (r.held.generation, r.schema_version, r.schema_hash.clone()))
    }

    /// The generation the record entries of a tenant are stamped with, none when it holds none.
    pub fn record_stamp(&self, tenant: &TenantId) -> Option<Generation> {
        let slot = unpoisoned(&self.slots).get(tenant).cloned()?;
        let held = unpoisoned(&slot);
        held.stamp
    }

    /// What the entry of a record holds: `None` when there is no entry, `Some(None)` for a held
    /// absence, `Some(Some(record))` for a record.
    pub fn held_record(
        &self,
        tenant: &TenantId,
        object: &ObjectTypeId,
        id: RecordId,
    ) -> Option<Option<Arc<Record>>> {
        let slot = unpoisoned(&self.slots).get(tenant).cloned()?;
        let held = unpoisoned(&slot);
        held.records
            .get(&(object.clone(), id))
            .map(|entry| entry.record.clone())
    }

    /// The keys of everything resident, as text: `tenant/object/id` for a record entry. A key is
    /// made of a tenant, an object type and a record id, never of a value of a field.
    pub fn resident_keys(&self) -> Vec<String> {
        let slots: Vec<(TenantId, Slot)> = unpoisoned(&self.slots)
            .iter()
            .map(|(tenant, slot)| (tenant.clone(), slot.clone()))
            .collect();
        let mut keys = Vec::new();
        for (tenant, slot) in slots {
            let held = unpoisoned(&slot);
            if held.header.is_some() {
                keys.push(format!("{tenant}/header"));
            }
            keys.extend(
                held.records
                    .keys()
                    .map(|(object, id)| format!("{tenant}/{object}/{}", id.0)),
            );
        }
        keys
    }

    fn slot(&self, tenant: &TenantId) -> Slot {
        unpoisoned(&self.slots)
            .entry(tenant.clone())
            .or_default()
            .clone()
    }

    fn bump(counter: &AtomicU64, by: u64) {
        counter.fetch_add(by, Ordering::Relaxed);
    }

    /// Reserve `n` accounted bytes if they fit in the budget.
    fn try_reserve(&self, n: u64) -> bool {
        let mut current = self.bytes.load(Ordering::Relaxed);
        loop {
            if current.saturating_add(n) > self.budget {
                return false;
            }
            match self.bytes.compare_exchange_weak(
                current,
                current + n,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(now) => current = now,
            }
        }
    }

    fn release(&self, n: u64) {
        self.bytes.fetch_sub(n, Ordering::Relaxed);
    }

    /// Remove one record entry by CLOCK from a tenant. `false` when it holds none.
    fn evict_one(&self, held: &mut Tenant) -> bool {
        let steps = held.records.len().saturating_mul(2).saturating_add(1);
        for _ in 0..steps {
            let after = held.hand.clone().and_then(|hand| {
                held.records
                    .range((Bound::Excluded(hand), Bound::Unbounded))
                    .next()
                    .map(|(key, _)| key.clone())
            });
            let Some(key) = after.or_else(|| held.records.keys().next().cloned()) else {
                return false;
            };
            let Some(entry) = held.records.get_mut(&key) else {
                return false;
            };
            held.hand = Some(key.clone());
            if entry.referenced {
                entry.referenced = false;
                continue;
            }
            let bytes = entry.bytes;
            held.records.remove(&key);
            self.release(bytes);
            Self::bump(&self.evictions, 1);
            self.resident_records.fetch_sub(1, Ordering::Relaxed);
            return true;
        }
        false
    }

    /// Reserve `n` bytes, evicting record entries (the header is pinned) to make room: first the
    /// served tenant's own, then those of other tenants whose lock is free. `false` when they do
    /// not fit.
    fn make_room(&self, own: &mut Tenant, own_slot: &Slot, n: u64) -> bool {
        if n > self.budget {
            return false;
        }
        loop {
            if self.try_reserve(n) {
                return true;
            }
            if !self.evict_one(own) {
                break;
            }
        }
        let others: Vec<Slot> = unpoisoned(&self.slots)
            .values()
            .filter(|slot| !Arc::ptr_eq(slot, own_slot))
            .cloned()
            .collect();
        for slot in others {
            // Never wait for another tenant's lock while holding one.
            let Ok(mut other) = slot.try_lock() else {
                continue;
            };
            while self.evict_one(&mut other) {
                if self.try_reserve(n) {
                    return true;
                }
            }
        }
        self.try_reserve(n)
    }

    /// Replace the header a tenant holds, keeping the totals. Not admitted when it does not fit.
    fn set_header(&self, held: &mut Tenant, slot: &Slot, next: Option<Resident>) {
        if let Some(old) = held.header.take() {
            self.release(old.bytes);
            self.tenants.fetch_sub(1, Ordering::Relaxed);
        }
        if let Some(new) = next {
            if self.make_room(held, slot, new.bytes) {
                self.tenants.fetch_add(1, Ordering::Relaxed);
                held.header = Some(new);
            }
        }
    }

    /// Drop every record entry of a tenant and its stamp.
    fn drop_records(&self, held: &mut Tenant) {
        let count = held.records.len() as u64;
        let bytes: u64 = held.records.values().map(|entry| entry.bytes).sum();
        held.records.clear();
        held.hand = None;
        held.stamp = None;
        self.release(bytes);
        self.resident_records.fetch_sub(count, Ordering::Relaxed);
        Self::bump(&self.invalidations, count);
    }

    /// Drop everything a tenant holds.
    fn drop_tenant(&self, held: &mut Tenant, slot: &Slot) {
        self.set_header(held, slot, None);
        self.drop_records(held);
    }

    /// Admit the record entry of `key`, valid at the tenant's stamp, if it fits.
    fn admit_record(
        &self,
        held: &mut Tenant,
        slot: &Slot,
        key: RecordKey,
        record: Option<Arc<Record>>,
    ) {
        if held.records.contains_key(&key) {
            return;
        }
        let bytes = entry_bytes(&key, record.as_deref());
        if !self.make_room(held, slot, bytes) {
            return;
        }
        self.resident_records.fetch_add(1, Ordering::Relaxed);
        held.records.insert(
            key,
            RecordEntry {
                record,
                bytes,
                referenced: true,
            },
        );
    }

    /// A load under the tenant's lock, held across the store I/O.
    fn read(&self, tenant: &TenantId, what: Read<'_>) -> StoreResult<Loaded> {
        let slot = self.slot(tenant);
        let mut guard = unpoisoned(&slot);
        let held = &mut *guard;
        let header = held.header.as_ref().map(|r| r.held.clone());
        let run = |needs: &[Need]| match what {
            Read::Needs(_) => self.inner.load_at_held(tenant, needs, header.as_ref()),
            Read::Complete => self.inner.load_complete_at_held(tenant, header.as_ref()),
        };
        let wanted: &[Need] = match what {
            Read::Needs(needs) => needs,
            Read::Complete => &[],
        };

        // What the entries answer, and what is left for the store.
        let mut hits: Vec<(RecordKey, Option<Arc<Record>>, u64)> = Vec::new();
        let mut rest: Vec<Need> = Vec::new();
        let mut probes: Vec<RecordKey> = Vec::new();
        for need in wanted {
            match need {
                Need::Record { object, id } if self.records => {
                    let key = (object.clone(), *id);
                    if let Some(entry) = held.records.get_mut(&key) {
                        entry.referenced = true;
                        hits.push((key, entry.record.clone(), entry.bytes));
                    } else {
                        probes.push(key);
                        rest.push(need.clone());
                    }
                }
                other => rest.push(other.clone()),
            }
        }

        let first: &[Need] = if hits.is_empty() { wanted } else { &rest };
        let mut answered = match run(first) {
            Ok(answered) => answered,
            Err(error) => {
                self.drop_tenant(held, &slot);
                return Err(error);
            }
        };
        if answered.header_decoded {
            Self::bump(&self.decodes, 1);
        }
        if !hits.is_empty()
            && (answered.loaded.generation.is_none() || answered.loaded.generation != held.stamp)
        {
            // An entry is not at the generation this load found: serve none of them. Drop them
            // and read the whole request again, which is one snapshot by itself.
            self.drop_records(held);
            Self::bump(&self.rereads, 1);
            // The reread reads every record of the request, the former hits included: they are
            // misses now, and are admitted again at the new generation below.
            probes.extend(hits.drain(..).map(|(key, _, _)| key));
            answered = match run(wanted) {
                Ok(answered) => answered,
                Err(error) => {
                    self.drop_tenant(held, &slot);
                    return Err(error);
                }
            };
            if answered.header_decoded {
                Self::bump(&self.decodes, 1);
            }
        }

        // Stamp the entries with the generation of the load, and admit what it read.
        match answered.loaded.generation {
            None => self.drop_records(held),
            Some(generation) => {
                if held.stamp != Some(generation) {
                    self.drop_records(held);
                    held.stamp = Some(generation);
                }
                if let Some(state) = answered.loaded.state.tenant.as_deref() {
                    for key in probes.iter() {
                        let record = state
                            .records
                            .get(&key.0)
                            .and_then(|records| records.get(&key.1))
                            .cloned();
                        self.admit_record(held, &slot, key.clone(), record);
                    }
                }
            }
        }
        Self::bump(&self.record_misses, probes.len() as u64);

        // The header.
        let matched = header
            .as_ref()
            .is_some_and(|h| Some(h.generation) == answered.loaded.generation);
        if matched && !answered.header_decoded {
            Self::bump(&self.hits, 1);
        } else {
            Self::bump(&self.misses, 1);
            let next = match (&answered.loaded.state.tenant, answered.loaded.generation) {
                (Some(state), Some(generation)) => {
                    Some(Resident::admit(HeldHeader::of(generation, state)))
                }
                _ => None,
            };
            self.set_header(held, &slot, next);
        }

        // The state: what the store read, and the entries beside it.
        let mut loaded = answered.loaded;
        if !hits.is_empty() {
            if let Some(shared) = loaded.state.tenant.as_mut() {
                let state = Arc::make_mut(shared);
                for (key, record, _) in &hits {
                    if let Some(record) = record {
                        state
                            .records
                            .entry(key.0.clone())
                            .or_default()
                            .insert(key.1, record.clone());
                    }
                }
            }
            if matches!(loaded.state.coverage, Coverage::Partial(_)) {
                loaded.state.coverage = Coverage::Partial(wanted.iter().cloned().collect());
            }
            Self::bump(&self.record_hits, hits.len() as u64);
            Self::bump(&self.saved_rows, hits.len() as u64);
            Self::bump(
                &self.saved_bytes,
                hits.iter().map(|(_, _, bytes)| *bytes).sum(),
            );
        }
        Ok(loaded)
    }

    /// Run a write that is not a commit, under the tenant's lock, and drop what the tenant holds
    /// whatever the outcome: its next load finds what the write left.
    fn exclusive<T>(
        &self,
        tenant: &TenantId,
        run: impl FnOnce() -> StoreResult<T>,
    ) -> StoreResult<T> {
        let slot = self.slot(tenant);
        let mut guard = unpoisoned(&slot);
        let done = run();
        self.drop_tenant(&mut guard, &slot);
        done
    }

    /// A commit under the tenant's lock, held across the store write and the carry-forward.
    fn committed(
        &self,
        commit: &Commit,
        run: impl FnOnce() -> StoreResult<Receipt>,
    ) -> StoreResult<Receipt> {
        let slot = self.slot(&commit.delta.tenant);
        let mut guard = unpoisoned(&slot);
        let held = &mut *guard;
        let receipt = match run() {
            Ok(receipt) => receipt,
            Err(error) => {
                // A refusal, a lost or an ambiguous outcome: no generation is assumed.
                self.drop_tenant(held, &slot);
                return Err(error);
            }
        };
        let expected = commit.expected_generation;
        let confirmed = |at: Option<Generation>| match (at, expected) {
            (Some(at), Some(expected)) if at == expected => Some(expected),
            _ => None,
        };

        // The header.
        match confirmed(held.header.as_ref().map(|r| r.held.generation)) {
            Some(expected) if receipt.generation == expected => {}
            Some(expected)
                if receipt.generation == expected.raised() && !writes_header(&commit.delta) =>
            {
                if let Some(header) = held.header.as_mut() {
                    header.held.generation = receipt.generation;
                }
            }
            _ => self.set_header(held, &slot, None),
        }

        // The records.
        match confirmed(held.stamp) {
            // A replayed result: nothing was written.
            Some(expected) if receipt.generation == expected => {}
            Some(expected) if receipt.generation == expected.raised() => {
                match invalidated_by(&commit.delta) {
                    Invalidated::All => self.drop_records(held),
                    Invalidated::Keys(keys) => {
                        for key in keys {
                            if let Some(entry) = held.records.remove(&key) {
                                self.release(entry.bytes);
                                self.resident_records.fetch_sub(1, Ordering::Relaxed);
                                Self::bump(&self.invalidations, 1);
                            }
                        }
                        held.stamp = Some(receipt.generation);
                    }
                }
            }
            _ => self.drop_records(held),
        }
        Ok(receipt)
    }
}

impl Store for ResidentStore {
    fn load_at(&self, tenant: &TenantId, needs: &[Need]) -> StoreResult<Loaded> {
        self.read(tenant, Read::Needs(needs))
    }

    fn load_complete_at(&self, tenant: &TenantId) -> StoreResult<Loaded> {
        self.read(tenant, Read::Complete)
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
        self.exclusive(tenant, || self.inner.remove_principal(tenant, principal))
    }

    fn anonymise_ledger_principal(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        expected: Option<&Generation>,
    ) -> StoreResult<strata_kernel::ledger::Anonymised> {
        self.exclusive(tenant, || {
            self.inner
                .anonymise_ledger_principal(tenant, principal, expected)
        })
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

    fn audit_anchor(&self, tenant: &TenantId) -> StoreResult<Option<audit_chain::AuditAnchor>> {
        self.inner.audit_anchor(tenant)
    }

    fn audit_anchor_advance(
        &self,
        tenant: &TenantId,
        at: i64,
    ) -> StoreResult<Option<audit_chain::AuditAnchor>> {
        self.inner.audit_anchor_advance(tenant, at)
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
        self.exclusive(tenant, || self.inner.restore_snapshot(tenant, id, expected))
    }

    fn reset(&self) -> StoreResult<()> {
        let done = self.inner.reset();
        let slots: Vec<Slot> = unpoisoned(&self.slots).values().cloned().collect();
        for slot in slots {
            let mut guard = unpoisoned(&slot);
            self.drop_tenant(&mut guard, &slot);
        }
        done
    }

    fn reset_tenant(&self, tenant: &TenantId) -> StoreResult<()> {
        self.exclusive(tenant, || self.inner.reset_tenant(tenant))
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
