//! A complete multi-tenant state, for tests, simulation and replay. The backend never holds
//! one of these; it works from per-tenant snapshots loaded through the storage contract.

use crate::canon;
use crate::ids::TenantId;
use crate::intent::{Context, DomainError, Intent, ReadRequest, ReadResult, Transition};
use crate::state::{State, TenantState};
use crate::transition::{apply_delta, transition};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct World {
    pub tenants: BTreeMap<TenantId, Arc<TenantState>>,
}

impl World {
    /// The complete snapshot of one tenant, as `transition` wants it.
    pub fn state(&self, tenant: &TenantId) -> State {
        match self.tenants.get(tenant) {
            Some(found) => State::complete(found.clone()),
            None => State::absent(tenant.clone()),
        }
    }

    /// Run one intent and return the world after it. `self` is untouched.
    pub fn step(&self, intent: &Intent, ctx: &Context) -> Result<(World, Transition), DomainError> {
        let outcome = transition(&self.state(&ctx.tenant), intent, ctx)?;
        let next = apply_delta(self.tenants.get(&ctx.tenant).map(Arc::as_ref), &outcome.delta)?;
        let mut tenants = self.tenants.clone();
        tenants.insert(ctx.tenant.clone(), Arc::new(next));
        Ok((World { tenants }, outcome))
    }

    pub fn read(&self, request: &ReadRequest, ctx: &Context) -> Result<ReadResult, DomainError> {
        crate::read::read(&self.state(&ctx.tenant), request, ctx)
    }

    /// Stable hash of the whole world.
    pub fn hash(&self) -> String {
        canon::hash_of(self)
    }

    /// Stable hash of one tenant, or of its absence.
    pub fn tenant_hash(&self, tenant: &TenantId) -> String {
        canon::hash_of(&self.tenants.get(tenant))
    }
}
