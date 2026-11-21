//! The store under test: what sits between the harness and the adapter, by mode.
//!
//! ```text
//! harness -> [mode layer] -> [counting layer] -> adapter
//! ```
//!
//! The counting layer is under the mode layer, so it counts what reaches the store whatever the
//! mode layer absorbed. The reference handle the coherence check compares with is the adapter
//! itself, never through either layer.
//!
//! - `none`: no mode layer.
//! - `header`: placeholder, a pass-through until header residency (WG-5390) lands.
//! - `full`: the working set of header and single records ([`ResidentStore::new`]) under a byte
//!   budget; answers always go to the store.
//! - `stale`: a deliberately broken read cache that never learns of a write made through another
//!   handle. The coherence check must catch it.
//!
//! To wire the real layers in, add a [`Hook`] (or a `Store` wrapper) and construct it in [`build`].

use crate::layer::{Hook, Layer, WrapperStats};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use strata_kernel::{Need, State, TenantId};
use strata_storage_api::resident::{HeldHeader, HeldLoad, ResidentStore};
use strata_storage_api::{Commit, Loaded, Receipt, Store, StoreError, StoreResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    None,
    Header,
    Full,
    Stale,
}

impl Mode {
    pub fn parse(text: &str) -> Result<Mode, String> {
        match text {
            "none" => Ok(Mode::None),
            "header" => Ok(Mode::Header),
            "full" => Ok(Mode::Full),
            "stale" => Ok(Mode::Stale),
            other => Err(format!(
                "unknown mode {other:?}: none, header, full or stale"
            )),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::None => "none",
            Mode::Header => "header",
            Mode::Full => "full",
            Mode::Stale => "stale",
        }
    }
}

/// The name of a need's kind, for the counters.
pub fn need_kind(need: &Need) -> &'static str {
    match need {
        Need::Record { .. } => "record",
        Need::ByField { .. } => "by-field",
        Need::AllOf(_) => "all-of",
        Need::PageOf { .. } => "page-of",
        Need::ByFields { .. } => "by-fields",
        Need::Ledger(_) => "ledger",
        Need::IndexPage { .. } => "index-page",
        Need::PairPage { .. } => "pair-page",
    }
}

/// The size of the records a state holds, as stored (their JSON), for the bytes counter.
pub fn state_bytes(state: &State) -> u64 {
    let Some(tenant) = state.tenant.as_deref() else {
        return 0;
    };
    tenant
        .records
        .values()
        .flat_map(|records| records.values())
        .map(|record| serde_json::to_vec(record.as_ref()).map_or(0, |bytes| bytes.len() as u64))
        .sum()
}

/// What reached the store.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct StoreCounts {
    pub loads: u64,
    pub load_completes: u64,
    pub commits: u64,
    pub commit_conflicts: u64,
    pub loads_by_need: BTreeMap<String, u64>,
    /// The size of the records returned, when counting was switched on.
    pub bytes_read: Option<u64>,
}

/// Counts every call that reaches the adapter.
#[derive(Default)]
pub struct CountingHook {
    loads: AtomicU64,
    load_completes: AtomicU64,
    commits: AtomicU64,
    commit_conflicts: AtomicU64,
    bytes: AtomicU64,
    count_bytes: AtomicBool,
    by_need: Mutex<BTreeMap<&'static str, u64>>,
}

impl CountingHook {
    pub fn with_bytes(count_bytes: bool) -> Self {
        let hook = CountingHook::default();
        hook.count_bytes.store(count_bytes, Ordering::Relaxed);
        hook
    }

    pub fn counts(&self) -> StoreCounts {
        let by_need = self
            .by_need
            .lock()
            .map(|map| {
                map.iter()
                    .map(|(kind, count)| ((*kind).to_string(), *count))
                    .collect()
            })
            .unwrap_or_default();
        StoreCounts {
            loads: self.loads.load(Ordering::Relaxed),
            load_completes: self.load_completes.load(Ordering::Relaxed),
            commits: self.commits.load(Ordering::Relaxed),
            commit_conflicts: self.commit_conflicts.load(Ordering::Relaxed),
            loads_by_need: by_need,
            bytes_read: self
                .count_bytes
                .load(Ordering::Relaxed)
                .then(|| self.bytes.load(Ordering::Relaxed)),
        }
    }

    fn note_commit(&self, done: &StoreResult<Receipt>) {
        self.commits.fetch_add(1, Ordering::Relaxed);
        if matches!(done, Err(StoreError::Conflict { .. })) {
            self.commit_conflicts.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl CountingHook {
    fn note_load(&self, needs: &[Need]) {
        self.loads.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut map) = self.by_need.lock() {
            if needs.is_empty() {
                *map.entry("header").or_default() += 1;
            }
            for need in needs {
                *map.entry(need_kind(need)).or_default() += 1;
            }
        }
    }

    fn note_bytes(&self, state: &State) {
        if self.count_bytes.load(Ordering::Relaxed) {
            self.bytes.fetch_add(state_bytes(state), Ordering::Relaxed);
        }
    }
}

impl Hook for CountingHook {
    fn load_at(&self, inner: &dyn Store, tenant: &TenantId, needs: &[Need]) -> StoreResult<Loaded> {
        self.note_load(needs);
        let loaded = inner.load_at(tenant, needs)?;
        self.note_bytes(&loaded.state);
        Ok(loaded)
    }

    fn load_complete_at(&self, inner: &dyn Store, tenant: &TenantId) -> StoreResult<Loaded> {
        self.load_completes.fetch_add(1, Ordering::Relaxed);
        let loaded = inner.load_complete_at(tenant)?;
        self.note_bytes(&loaded.state);
        Ok(loaded)
    }

    fn load_at_held(
        &self,
        inner: &dyn Store,
        tenant: &TenantId,
        needs: &[Need],
        held: Option<&HeldHeader>,
    ) -> StoreResult<HeldLoad> {
        self.note_load(needs);
        let done = inner.load_at_held(tenant, needs, held)?;
        self.note_bytes(&done.loaded.state);
        Ok(done)
    }

    fn load_complete_at_held(
        &self,
        inner: &dyn Store,
        tenant: &TenantId,
        held: Option<&HeldHeader>,
    ) -> StoreResult<HeldLoad> {
        self.load_completes.fetch_add(1, Ordering::Relaxed);
        let done = inner.load_complete_at_held(tenant, held)?;
        self.note_bytes(&done.loaded.state);
        Ok(done)
    }

    fn commit(&self, inner: &dyn Store, commit: &Commit) -> StoreResult<Receipt> {
        let done = inner.commit(commit);
        self.note_commit(&done);
        done
    }

    fn commit_with_segments(
        &self,
        inner: &dyn Store,
        commit: &Commit,
        write: &strata_kernel::segments::SegmentWrite,
    ) -> StoreResult<Receipt> {
        let done = inner.commit_with_segments(commit, write);
        self.note_commit(&done);
        done
    }
}

/// A layer that forwards everything and reports itself a placeholder.
pub struct PlaceholderHook {
    pub name: &'static str,
}

impl Hook for PlaceholderHook {
    fn stats(&self) -> WrapperStats {
        WrapperStats {
            name: self.name.to_string(),
            placeholder: true,
            ..WrapperStats::default()
        }
    }
}

struct Entry<T> {
    value: T,
    /// What loading it took, for the hit-by-cost figure.
    cost_ns: u64,
    bytes: u64,
}

#[derive(Default)]
struct StaleCache {
    reads: BTreeMap<(String, Vec<Need>), Entry<Loaded>>,
    completes: BTreeMap<String, Entry<Loaded>>,
}

/// A read cache that is broken on purpose: it keeps every answer it has loaded and drops a
/// tenant's answers only when a write goes through *this* layer, or a commit through it is refused
/// as a conflict. A write made through another handle (a foreign commit, a restore, an erasure, a
/// schema change) leaves its entries in place, so it serves what the tenant no longer holds.
#[derive(Default)]
pub struct StaleHook {
    cache: Mutex<StaleCache>,
    hits: AtomicU64,
    misses: AtomicU64,
    miss_cost_ns: AtomicU64,
    hit_saved_ns: AtomicU64,
}

impl StaleHook {
    fn bytes_of(state: &State) -> u64 {
        256 + state_bytes(state)
    }
}

impl Hook for StaleHook {
    fn load_at(&self, inner: &dyn Store, tenant: &TenantId, needs: &[Need]) -> StoreResult<Loaded> {
        let key = (tenant.to_string(), needs.to_vec());
        if let Ok(cache) = self.cache.lock() {
            if let Some(entry) = cache.reads.get(&key) {
                self.hits.fetch_add(1, Ordering::Relaxed);
                self.hit_saved_ns
                    .fetch_add(entry.cost_ns, Ordering::Relaxed);
                return Ok(entry.value.clone());
            }
        }
        let started = Instant::now();
        let loaded = inner.load_at(tenant, needs)?;
        let cost_ns = started.elapsed().as_nanos() as u64;
        self.misses.fetch_add(1, Ordering::Relaxed);
        self.miss_cost_ns.fetch_add(cost_ns, Ordering::Relaxed);
        if let Ok(mut cache) = self.cache.lock() {
            cache.reads.insert(
                key,
                Entry {
                    bytes: Self::bytes_of(&loaded.state),
                    value: loaded.clone(),
                    cost_ns,
                },
            );
        }
        Ok(loaded)
    }

    fn load_complete_at(&self, inner: &dyn Store, tenant: &TenantId) -> StoreResult<Loaded> {
        let key = tenant.to_string();
        if let Ok(cache) = self.cache.lock() {
            if let Some(entry) = cache.completes.get(&key) {
                self.hits.fetch_add(1, Ordering::Relaxed);
                self.hit_saved_ns
                    .fetch_add(entry.cost_ns, Ordering::Relaxed);
                return Ok(entry.value.clone());
            }
        }
        let started = Instant::now();
        let loaded = inner.load_complete_at(tenant)?;
        let cost_ns = started.elapsed().as_nanos() as u64;
        self.misses.fetch_add(1, Ordering::Relaxed);
        self.miss_cost_ns.fetch_add(cost_ns, Ordering::Relaxed);
        if let Ok(mut cache) = self.cache.lock() {
            cache.completes.insert(
                key,
                Entry {
                    bytes: Self::bytes_of(&loaded.state),
                    value: loaded.clone(),
                    cost_ns,
                },
            );
        }
        Ok(loaded)
    }

    fn wrote(&self, tenant: &TenantId, error: Option<&StoreError>) {
        // A write that went through, or a commit refused because the cache's header was behind:
        // the tenant's entries go. Nothing else drops them.
        let own_write = error.is_none() || matches!(error, Some(StoreError::Conflict { .. }));
        if !own_write {
            return;
        }
        let name = tenant.to_string();
        if let Ok(mut cache) = self.cache.lock() {
            cache.reads.retain(|(t, _), _| *t != name);
            cache.completes.remove(&name);
        }
    }

    fn stats(&self) -> WrapperStats {
        let resident_bytes = self
            .cache
            .lock()
            .map(|cache| {
                cache.reads.values().map(|e| e.bytes).sum::<u64>()
                    + cache.completes.values().map(|e| e.bytes).sum::<u64>()
            })
            .unwrap_or(0);
        WrapperStats {
            name: "stale".to_string(),
            placeholder: false,
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            miss_cost_ns: self.miss_cost_ns.load(Ordering::Relaxed),
            hit_saved_ns: self.hit_saved_ns.load(Ordering::Relaxed),
            resident_bytes,
            ..WrapperStats::default()
        }
    }
}

/// The store under test and what to ask it afterwards.
pub struct Built {
    pub store: Arc<dyn Store>,
    pub counting: Arc<CountingHook>,
    pub wrapper: Option<Arc<dyn Fn() -> WrapperStats + Send + Sync>>,
}

impl Built {
    pub fn wrapper_stats(&self, mode: Mode) -> WrapperStats {
        match &self.wrapper {
            Some(stats) => stats(),
            None => WrapperStats {
                name: mode.name().to_string(),
                ..WrapperStats::default()
            },
        }
    }
}

/// Stack the layers of `mode` over `base`. `counting` is shared so that the counts survive a
/// rebuild of the stack (a reopen).
pub fn build(
    base: Arc<dyn Store>,
    mode: Mode,
    counting: Arc<CountingHook>,
    budget_bytes: u64,
) -> Built {
    let counted: Arc<dyn Store> = Arc::new(Layer::new(base, counting.clone()));
    let layered = |hook: Arc<dyn Hook>| -> (
        Arc<dyn Store>,
        Option<Arc<dyn Fn() -> WrapperStats + Send + Sync>>,
    ) {
        let probe = hook.clone();
        let store: Arc<dyn Store> = Arc::new(Layer::new(counted.clone(), hook));
        let stats: Arc<dyn Fn() -> WrapperStats + Send + Sync> = Arc::new(move || probe.stats());
        (store, Some(stats))
    };
    let (store, wrapper) = match mode {
        Mode::None => (counted.clone(), None),
        Mode::Header => {
            let resident = Arc::new(ResidentStore::header_only(counted.clone()));
            let probe = resident.clone();
            let stats: Arc<dyn Fn() -> WrapperStats + Send + Sync> = Arc::new(move || {
                let s = probe.stats();
                WrapperStats {
                    name: "header".to_string(),
                    placeholder: false,
                    hits: s.hits,
                    misses: s.misses,
                    decodes: Some(s.decodes),
                    resident_bytes: s.resident_bytes,
                    resident_tenants: Some(s.resident_tenants),
                    ..WrapperStats::default()
                }
            });
            (resident as Arc<dyn Store>, Some(stats))
        }
        Mode::Full => {
            let resident = Arc::new(ResidentStore::new(counted.clone(), budget_bytes));
            let probe = resident.clone();
            let stats: Arc<dyn Fn() -> WrapperStats + Send + Sync> = Arc::new(move || {
                let s = probe.stats();
                WrapperStats {
                    name: "full".to_string(),
                    placeholder: false,
                    // The header and the record entries both answer a load.
                    hits: s.hits + s.record_hits,
                    misses: s.misses + s.record_misses,
                    decodes: Some(s.decodes),
                    resident_bytes: s.resident_bytes,
                    resident_tenants: Some(s.resident_tenants),
                    resident_records: Some(s.resident_records),
                    budget_bytes: Some(probe.budget()),
                    hit_saved_bytes: Some(s.hit_saved_bytes),
                    hit_saved_rows: Some(s.hit_saved_rows),
                    evictions: Some(s.evictions),
                    invalidations: Some(s.invalidations),
                    rereads: Some(s.rereads),
                    ..WrapperStats::default()
                }
            });
            (resident as Arc<dyn Store>, Some(stats))
        }
        Mode::Stale => layered(Arc::new(StaleHook::default())),
    };
    Built {
        store,
        counting,
        wrapper,
    }
}
