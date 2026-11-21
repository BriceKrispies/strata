//! The decision of an intent without committing it: what `execute` would write, read through a
//! store with partial loads, as `execute` reads it.
//!
//! The coherence check compares the delta, events and output a store under test commits with the
//! decision reached here on the unwrapped reference handle. Reading only what the intent needs
//! (and not the whole tenant) keeps the check affordable at the larger scales.

use std::collections::BTreeSet;
use strata_kernel::{Context, DomainError, ErrorKind, Intent, Need, Transition};
use strata_storage_api::Store;

/// The most rounds of loading what the kernel found missing, as `execute` allows.
const MAX_NEED_ROUNDS: usize = 32;

/// The result of deciding: the transition, or the kernel's refusal; a store failure is `Err(text)`.
pub type Decided = Result<Result<Transition, DomainError>, String>;

pub fn dry_run(store: &dyn Store, intent: &Intent, ctx: &Context) -> Decided {
    let header = store.load(&ctx.tenant, &[]).map_err(|e| e.to_string())?;
    let mut wanted: BTreeSet<Need> =
        strata_kernel::needs(header.tenant.as_deref().map(|t| t.schema.as_ref()), intent)
            .into_iter()
            .collect();
    let mut rounds = 0;
    loop {
        let list: Vec<Need> = wanted.iter().cloned().collect();
        let state = store.load(&ctx.tenant, &list).map_err(|e| e.to_string())?;
        let more: Vec<Need> = strata_kernel::refine_needs(&state, intent)
            .into_iter()
            .filter(|need| !wanted.contains(need))
            .collect();
        if !more.is_empty() && rounds < MAX_NEED_ROUNDS {
            wanted.extend(more);
            rounds += 1;
            continue;
        }
        match strata_kernel::transition(&state, intent, ctx) {
            Err(DomainError {
                kind: ErrorKind::InsufficientState,
                missing: Some(need),
                ..
            }) if rounds < MAX_NEED_ROUNDS && !wanted.contains(&need) => {
                wanted.insert(need);
                rounds += 1;
            }
            decided => return Ok(decided),
        }
    }
}
