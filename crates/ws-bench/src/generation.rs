//! The generation of a tenant, as the coherence check needs it.
//!
//! The check is written against [`GenerationView`]; [`StoreGeneration`] answers it from
//! `Store::generation`, the row that holds the counter and the nonce. Equal generations are the
//! same state, restores and out-of-band changes included.

use strata_kernel::TenantId;
use strata_storage_api::{Store, StoreResult};

pub use strata_storage_api::Generation;

pub trait GenerationView: Send + Sync {
    /// The tenant's generation, `None` when the tenant does not exist.
    fn generation(&self, store: &dyn Store, tenant: &TenantId) -> StoreResult<Option<Generation>>;
}

/// The generation the store reports.
pub struct StoreGeneration;

impl GenerationView for StoreGeneration {
    fn generation(&self, store: &dyn Store, tenant: &TenantId) -> StoreResult<Option<Generation>> {
        store.generation(tenant)
    }
}
