//! A complete multi-tenant state, for tests, simulation and replay. The backend never holds
//! one of these; it works from per-tenant snapshots loaded through the storage contract.

use crate::aggregates;
use crate::canon;
use crate::intent::{Context, DomainError, Intent, ReadRequest, ReadResult, Transition};
use crate::search::{self, Candidate, Probe, SearchRequest, SearchResult};
use crate::state::{State, StateExt, TenantState, TenantStateExt};
use crate::transition::{apply_delta, transition};
use crate::types::{AggregateRequest, AggregateResult, Need, TenantId};
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
        let next = apply_delta(
            self.tenants.get(&ctx.tenant).map(Arc::as_ref),
            &outcome.delta,
        )?;
        let mut tenants = self.tenants.clone();
        tenants.insert(ctx.tenant.clone(), Arc::new(next));
        Ok((World { tenants }, outcome))
    }

    pub fn read(&self, request: &ReadRequest, ctx: &Context) -> Result<ReadResult, DomainError> {
        crate::read::read(&self.state(&ctx.tenant), request, ctx)
    }

    /// A search of the tenant of `ctx`, over this world's records: the index probes are answered by
    /// scanning the records of each object type (`search::candidates_in`).
    pub fn search(
        &self,
        request: &SearchRequest,
        ctx: &Context,
    ) -> Result<SearchResult, DomainError> {
        let mut source = SnapshotSource {
            state: self.state(&ctx.tenant),
        };
        search::run(&mut source, request, ctx)
    }

    /// One aggregate of a named query of the tenant of `ctx`, over this world's records.
    pub fn aggregate(
        &self,
        request: &AggregateRequest,
        ctx: &Context,
    ) -> Result<AggregateResult, DomainError> {
        aggregates::run(&self.state(&ctx.tenant), request, ctx)
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

/// A snapshot of one tenant as the source of a search: the records of each object type are scanned for the
/// index probes, and the snapshot holds every record, so it never asks for more state.
struct SnapshotSource {
    state: State,
}

impl search::Source for SnapshotSource {
    type Error = DomainError;

    fn domain(&self, error: DomainError) -> DomainError {
        error
    }

    fn candidates(&mut self, probe: &Probe) -> Result<Vec<Candidate>, DomainError> {
        let Some(tenant) = self.state.tenant.as_deref() else {
            return Ok(Vec::new());
        };
        let Some(def) = tenant.schema.objects.get(&probe.object) else {
            return Ok(Vec::new());
        };
        Ok(search::candidates_in(
            tenant.records_of(&probe.object),
            def,
            probe,
        ))
    }

    fn load(&mut self, _needs: &[Need]) -> Result<State, DomainError> {
        Ok(self.state.clone())
    }
}
