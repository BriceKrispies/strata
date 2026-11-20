//! What a view's sections read, as needs (WG-5104, KRN-2526).
//!
//! A view's sections are read in [`section_order`] from one snapshot, each with the request
//! built from the single rows of the sections before it, and each section's included records with
//! a `Records` read. [`view_reads`] runs exactly those reads against a snapshot and names what the
//! snapshot lacks for the first read it cannot answer, so a store loop
//! (`strata_storage_api::covering::load_covering`) loads, for each section, only the page its
//! query reads and the records it includes. A read that follows references into records the
//! snapshot lacks names all of them for the rows it examined at once
//! ([`strata_kernel::query_plan::read_missing`]), so they load in one round, alone.

use crate::project::{included_refs, section_order};
use std::collections::BTreeMap;
use std::sync::Arc;
use strata_kernel::ext::*;
use strata_kernel::query_plan::read_missing;
use strata_kernel::{
    read, read_needs, Context, DomainError, ErrorKind, Need, ReadRequest, ReadResult, Record,
    State, TenantSchema, ViewDef, ViewSection,
};

/// The rows of the single sections read so far, by section name.
pub type Done = BTreeMap<String, Option<Arc<Record>>>;

/// What the sections ask for before anything is read: each section's request with no earlier
/// rows.
pub fn initial_needs(
    schema: &TenantSchema,
    def: &ViewDef,
    request_of: &impl Fn(&ViewSection, &Done) -> ReadRequest,
) -> Vec<Need> {
    let none = Done::new();
    def.sections
        .values()
        .flat_map(|section| read_needs(schema, &request_of(section, &none)))
        .collect()
}

/// Run every read of the view against `state`, as the view handler does. When a read needs state
/// the snapshot does not cover, its needs go to `asked` and the run stops there; a read the kernel
/// refuses is the error.
pub fn view_reads(
    state: &State,
    ctx: &Context,
    def: &ViewDef,
    request_of: &impl Fn(&ViewSection, &Done) -> ReadRequest,
    asked: &mut Vec<Need>,
) -> Result<(), DomainError> {
    let Some(tenant) = state.tenant.as_deref() else {
        return Ok(());
    };
    let schema = &tenant.schema;
    let wanted = |request: &ReadRequest, asked: &mut Vec<Need>| {
        let missing: Vec<Need> = read_needs(schema, request)
            .into_iter()
            .filter(|need| !state.coverage.covers(need))
            .collect();
        let stop = !missing.is_empty();
        asked.extend(missing);
        stop
    };
    let mut done = Done::new();
    for name in section_order(def) {
        let section = &def.sections[name];
        let request = request_of(section, &done);
        if wanted(&request, asked) {
            return Ok(());
        }
        let result = read_asking(state, &request, ctx, asked)?;
        if let Some(query) = schema.queries.get(&section.query) {
            for (_, (target, ids)) in included_refs(schema, &query.object, section, &result.rows) {
                let included = ReadRequest::Records {
                    object: target,
                    ids,
                };
                if wanted(&included, asked) {
                    return Ok(());
                }
                read_asking(state, &included, ctx, asked)?;
            }
        }
        if section.single {
            done.insert(name.clone(), result.rows.first().cloned());
        }
    }
    Ok(())
}

/// A read of the view; when the snapshot lacks state, every need the kernel can name at once
/// (each record the examined rows refer to, and the page the read stopped at) goes to `asked`,
/// so the store loop loads exactly those records and not their object type (KRN-2922).
fn read_asking(
    state: &State,
    request: &ReadRequest,
    ctx: &Context,
    asked: &mut Vec<Need>,
) -> Result<ReadResult, DomainError> {
    let outcome = read(state, request, ctx);
    if let Err(error) = &outcome {
        if matches!(error.kind, ErrorKind::InsufficientState) {
            asked.extend(read_missing(state, request, ctx));
            asked.extend(error.missing.iter().cloned());
        }
    }
    outcome
}
