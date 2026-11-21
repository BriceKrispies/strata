//! A store that forwards to another, with hooks on the paths a read cache cares about.

use std::sync::Arc;
use strata_kernel::segments as seg;
use strata_kernel::{Need, TenantId};
use strata_storage_api::resident::{HeldHeader, HeldLoad};
use strata_storage_api::{Commit, Loaded, Receipt, Store, StoreError, StoreResult};

/// What a layer reports of itself. A wrapper that is only a placeholder says so.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct WrapperStats {
    pub name: String,
    /// The layer is a pass-through standing in for work not yet built; its figures mean nothing.
    pub placeholder: bool,
    pub hits: u64,
    pub misses: u64,
    /// Wall time spent loading on misses, nanoseconds.
    pub miss_cost_ns: u64,
    /// Wall time the hits did not spend: for each hit, what the load that filled the entry took.
    pub hit_saved_ns: u64,
    pub resident_bytes: u64,
    /// Times the wrapper's own decode counter says an adapter decoded a header, where it keeps one.
    pub decodes: Option<u64>,
    /// Tenants with something resident, where the wrapper counts them.
    pub resident_tenants: Option<u64>,
}

/// The behaviour of a layer. Every method defaults to forwarding.
pub trait Hook: Send + Sync + 'static {
    fn load_at(&self, inner: &dyn Store, tenant: &TenantId, needs: &[Need]) -> StoreResult<Loaded> {
        inner.load_at(tenant, needs)
    }
    fn load_complete_at(&self, inner: &dyn Store, tenant: &TenantId) -> StoreResult<Loaded> {
        inner.load_complete_at(tenant)
    }
    fn load_at_held(
        &self,
        inner: &dyn Store,
        tenant: &TenantId,
        needs: &[Need],
        held: Option<&HeldHeader>,
    ) -> StoreResult<HeldLoad> {
        inner.load_at_held(tenant, needs, held)
    }
    fn load_complete_at_held(
        &self,
        inner: &dyn Store,
        tenant: &TenantId,
        held: Option<&HeldHeader>,
    ) -> StoreResult<HeldLoad> {
        inner.load_complete_at_held(tenant, held)
    }
    fn commit(&self, inner: &dyn Store, commit: &Commit) -> StoreResult<Receipt> {
        inner.commit(commit)
    }
    fn commit_with_segments(
        &self,
        inner: &dyn Store,
        commit: &Commit,
        write: &seg::SegmentWrite,
    ) -> StoreResult<Receipt> {
        inner.commit_with_segments(commit, write)
    }
    /// Called after every write through this layer, with the error when it failed.
    fn wrote(&self, _tenant: &TenantId, _error: Option<&StoreError>) {}
    fn stats(&self) -> WrapperStats {
        WrapperStats::default()
    }
}

impl<T: Hook + ?Sized> Hook for Arc<T> {
    fn load_at(&self, inner: &dyn Store, tenant: &TenantId, needs: &[Need]) -> StoreResult<Loaded> {
        (**self).load_at(inner, tenant, needs)
    }
    fn load_complete_at(&self, inner: &dyn Store, tenant: &TenantId) -> StoreResult<Loaded> {
        (**self).load_complete_at(inner, tenant)
    }
    fn load_at_held(
        &self,
        inner: &dyn Store,
        tenant: &TenantId,
        needs: &[Need],
        held: Option<&HeldHeader>,
    ) -> StoreResult<HeldLoad> {
        (**self).load_at_held(inner, tenant, needs, held)
    }
    fn load_complete_at_held(
        &self,
        inner: &dyn Store,
        tenant: &TenantId,
        held: Option<&HeldHeader>,
    ) -> StoreResult<HeldLoad> {
        (**self).load_complete_at_held(inner, tenant, held)
    }
    fn commit(&self, inner: &dyn Store, commit: &Commit) -> StoreResult<Receipt> {
        (**self).commit(inner, commit)
    }
    fn commit_with_segments(
        &self,
        inner: &dyn Store,
        commit: &Commit,
        write: &seg::SegmentWrite,
    ) -> StoreResult<Receipt> {
        (**self).commit_with_segments(inner, commit, write)
    }
    fn wrote(&self, tenant: &TenantId, error: Option<&StoreError>) {
        (**self).wrote(tenant, error)
    }
    fn stats(&self) -> WrapperStats {
        (**self).stats()
    }
}

/// The store `inner` behind `hook`. The `Store` impl is in `delegate.rs`.
pub struct Layer<H> {
    pub(crate) inner: Arc<dyn Store>,
    pub(crate) hook: H,
}

impl<H: Hook> Layer<H> {
    pub fn new(inner: Arc<dyn Store>, hook: H) -> Self {
        Layer { inner, hook }
    }

    pub fn hook(&self) -> &H {
        &self.hook
    }
}
