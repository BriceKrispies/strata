//! Search around the kernel's decision (SRH-150 to SRH-152, SRH-1590): the store answers the
//! candidates of its index and loads the records a decision names; every rule is the kernel's
//! ([`strata_kernel::search`]). This module only connects a [`Store`] to the kernel's loop, as
//! [`crate::query`] does for named reads.

use crate::{ExecuteError, Store, StoreResult};
use strata_kernel::search::{self, Candidate, Probe, SearchRequest, SearchResult, Source};
use strata_kernel::{Context, DomainError, Need, State};

/// A store as the kernel's search loop sees it, for one tenant.
struct StoreSource<'a> {
    store: &'a dyn Store,
    ctx: &'a Context,
}

impl Source for StoreSource<'_> {
    type Error = ExecuteError;

    fn domain(&self, error: DomainError) -> ExecuteError {
        ExecuteError::Domain(error)
    }

    fn candidates(&mut self, probe: &Probe) -> Result<Vec<Candidate>, ExecuteError> {
        let found: StoreResult<Vec<Candidate>> =
            self.store.search_candidates(&self.ctx.tenant, probe);
        Ok(found?)
    }

    fn load(&mut self, needs: &[Need]) -> Result<State, ExecuteError> {
        Ok(self.store.load(&self.ctx.tenant, needs)?)
    }
}

/// Search the tenant of `ctx` for the reader of `ctx`: the hits of one page, in ascending object
/// type and record id, each the record as the reader may read it with the fields that matched,
/// and the cursor of the page after.
pub fn run(
    store: &dyn Store,
    request: &SearchRequest,
    ctx: &Context,
) -> Result<SearchResult, ExecuteError> {
    search::run(&mut StoreSource { store, ctx }, request, ctx)
}
