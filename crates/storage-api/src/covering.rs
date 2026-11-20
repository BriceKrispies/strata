//! Loading what a pure decision asks for until it stops asking (WG-5104).
//!
//! The kernel names what it needs: `needs` and `read_needs` before it runs, and the one need it
//! found missing (`InsufficientState`) while it ran. A planned query read finds the next page of
//! candidates missing this way, so a read loads exactly the pages it examines (KRN-2521).
//! [`load_covering`] is that loop for any attempt that runs kernel reads against one snapshot: a
//! view runs every section's read in it (KRN-2526), and the tests count what it loads.

use crate::{ExecuteError, Store};
use std::collections::BTreeSet;
use strata_kernel::ext::*;
use strata_kernel::{DomainError, ErrorKind, Need, State, TenantId};

/// The most rounds of loading one attempt may take after the first, as in [`crate::query`].
pub const MAX_ROUNDS: usize = 32;

/// What [`load_covering`] ended with.
#[derive(Debug)]
pub struct Covered<T> {
    /// The last snapshot loaded.
    pub state: State,
    /// The attempt's result on that snapshot.
    pub outcome: Result<T, DomainError>,
    /// True when the attempt asked for nothing more: its outcome is not a missing need.
    pub settled: bool,
    /// How many times the store was asked to load.
    pub loads: usize,
    /// How many records the loads returned, summed over the loads.
    pub records: usize,
    /// Every need of the last load.
    pub wanted: BTreeSet<Need>,
}

/// Load `initial`, run `attempt` on the snapshot, and load again with what it asks for until it
/// asks for nothing more or [`MAX_ROUNDS`] rounds have passed. The attempt asks in two ways: by
/// pushing needs to its second argument (all of them are added at once), and by failing with
/// `InsufficientState` naming one need. A missing single record is widened to its object type,
/// as [`crate::query`] does, so that a rule followed on every row costs one round.
pub fn load_covering<T>(
    store: &dyn Store,
    tenant: &TenantId,
    initial: impl IntoIterator<Item = Need>,
    mut attempt: impl FnMut(&State, &mut Vec<Need>) -> Result<T, DomainError>,
) -> Result<Covered<T>, ExecuteError> {
    let mut wanted: BTreeSet<Need> = initial.into_iter().collect();
    let (mut loads, mut records, mut rounds) = (0, 0, 0);
    loop {
        let list: Vec<Need> = wanted.iter().cloned().collect();
        let state = store.load(tenant, &list)?;
        loads += 1;
        records += state.tenant.as_deref().map_or(0, |t| t.record_count());
        let mut asked = Vec::new();
        let outcome = attempt(&state, &mut asked);
        let fresh: Vec<Need> = asked
            .into_iter()
            .filter(|need| !wanted.contains(need))
            .collect();
        if !fresh.is_empty() {
            if rounds < MAX_ROUNDS {
                wanted.extend(fresh);
                rounds += 1;
                continue;
            }
            return Ok(Covered {
                state,
                outcome,
                settled: false,
                loads,
                records,
                wanted,
            });
        }
        let missing = match &outcome {
            Err(DomainError {
                kind: ErrorKind::InsufficientState,
                missing: Some(need),
                ..
            }) => Some(need.clone()),
            _ => None,
        };
        match missing {
            Some(need) if rounds < MAX_ROUNDS && !wanted.contains(&need) => {
                wanted.insert(match need {
                    Need::Record { object, .. } => Need::AllOf(object),
                    other => other,
                });
                rounds += 1;
            }
            missing => {
                return Ok(Covered {
                    state,
                    outcome,
                    settled: missing.is_none(),
                    loads,
                    records,
                    wanted,
                })
            }
        }
    }
}
