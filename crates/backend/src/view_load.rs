//! What a view loads (WG-5104, KRN-2526): the page each section's query reads and the records
//! each section includes, and not every record of every object type the view depends on.
//!
//! The view handler reads every section from one snapshot. This module finds that snapshot: it
//! runs the same section reads, in the same order and with the same requests
//! ([`strata_substrate::view_needs::view_reads`]), against what is loaded so far, and loads what
//! they ask for ([`load_covering`]) until they ask for nothing more. The handler then reads from
//! the snapshot that answered them, so its reads are answered as they were here. When the reads
//! still ask for more after the rounds a read may take, the snapshot is every record of every
//! object type the view depends on, as before.

use crate::app::{ApiError, App};
use std::collections::BTreeSet;
use strata_kernel::{Context, Need, ObjectTypeId, ReadRequest, State, ViewDef, ViewSection};
use strata_storage_api::covering::load_covering;
use strata_storage_api::{ExecuteError, Store};
use strata_substrate::view_needs::{initial_needs, view_reads, Done};

/// The snapshot a view's sections are read from, from a store. `request_of` builds a section's
/// read request from the rows of the single sections read before it, exactly as the handler
/// does.
pub fn view_state(
    store: &dyn Store,
    ctx: &Context,
    def: &ViewDef,
    depends_on: &BTreeSet<ObjectTypeId>,
    request_of: impl Fn(&ViewSection, &Done) -> ReadRequest,
) -> Result<State, ExecuteError> {
    let header = store.load(&ctx.tenant, &[])?;
    let Some(schema) = header.tenant.as_ref().map(|tenant| tenant.schema.clone()) else {
        return Ok(header);
    };
    let initial = initial_needs(&schema, def, &request_of);
    let covered = load_covering(store, &ctx.tenant, initial, |state, asked| {
        view_reads(state, ctx, def, &request_of, asked)
    })?;
    if covered.settled {
        return Ok(covered.state);
    }
    let whole: Vec<Need> = depends_on.iter().cloned().map(Need::AllOf).collect();
    Ok(store.load(&ctx.tenant, &whole)?)
}

/// [`view_state`] from the application's store, with its errors as API errors.
pub fn load_view_state(
    app: &App,
    ctx: &Context,
    def: &ViewDef,
    depends_on: &BTreeSet<ObjectTypeId>,
    request_of: impl Fn(&ViewSection, &Done) -> ReadRequest,
) -> Result<State, ApiError> {
    view_state(app.store.as_ref(), ctx, def, depends_on, request_of).map_err(|error| match error {
        ExecuteError::Store(error) => ApiError::from(error),
        ExecuteError::Domain(error) => ApiError::domain(error, false),
    })
}
