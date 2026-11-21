//! The generation of a tenant, as the coherence check needs it (WG-5389).
//!
//! `Store::generation` does not exist on this branch. The check is written against
//! [`GenerationView`]; [`RevisionView`] is the stand-in, which answers the tenant's revision. A
//! revision does not tell a restored tenant from the one it replaced (a restore can return to a
//! revision seen before), which is the reason the design adds a nonce. The coordinator replaces
//! [`RevisionView`] with a view that calls `Store::generation` once that method lands.

use strata_kernel::TenantId;
use strata_storage_api::{Store, StoreResult};

/// Which state of a tenant an answer was read at. Equal generations are the same state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Generation {
    pub nonce: u128,
    pub counter: u64,
}

pub trait GenerationView: Send + Sync {
    /// The tenant's generation, `None` when the tenant does not exist.
    fn generation(&self, store: &dyn Store, tenant: &TenantId) -> StoreResult<Option<Generation>>;
}

/// The generation of today's store: the revision, with a zero nonce.
pub struct RevisionView;

impl GenerationView for RevisionView {
    fn generation(&self, store: &dyn Store, tenant: &TenantId) -> StoreResult<Option<Generation>> {
        Ok(store.tenant_summary(tenant)?.map(|summary| Generation {
            nonce: 0,
            counter: summary.revision.0,
        }))
    }
}
