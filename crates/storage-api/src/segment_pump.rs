//! The segment event log as the triggers' outbox (WG-5165, SEG-014, SEG-031, SEG-032, D-7280): a
//! reader that turns each new entry or exit event into the jobs of the triggers that match it.
//!
//! **Ordering.** The log is ordered by position, and [`pump`] reads it from the tenant's cursor
//! ([`crate::Store::segment_cursor`]) in position order, at most [`PUMP_BATCH`] events at a time,
//! and matches each event against the triggers of the tenant in trigger id order, as the kernel
//! matches the events of a transition ([`strata_kernel::segment_flow`]).
//!
//! **Exactly once.** The jobs and exception records a batch of events starts are written in the
//! transaction that raises the cursor past them ([`crate::segments::SegmentCommit`]), and the
//! write is refused when the cursor is not where the decision read it
//! ([`strata_kernel::segments::SegmentWrite::published`]). So an event is handed on in one
//! transaction or not at all: a pump that stops before the commit leaves the cursor where it was
//! and the next one reads the same events; two pumps at once cannot both commit the same
//! position. As a second guard, the key of each job is the trigger, the automation and the
//! position of the event (`start:<trigger>:<automation>:<position>`), and the id of each declined
//! record is derived from the same, so even a repeat of a decision creates nothing twice.
//!
//! **Authority and hops** are the kernel's: the automation principal must be able to read the
//! subject record of the person of the event, and an event past the hop limit is declined with
//! `hop-limit` and a declined record, never run.

use crate::jobs::{self, WorkerWrites};
use crate::segment_jobs;
use crate::segments::SegmentCommit;
use crate::{ExecuteError, Store, StoreError};
use std::collections::BTreeSet;
use strata_kernel::ext::*;
use strata_kernel::rules::Lookup;
use strata_kernel::segment_flow;
use strata_kernel::segments::{Segment, SegmentEvent, SegmentWrite};
use strata_kernel::{
    Context, DomainError, Effect, ErrorKind, Need, Principal, State, TenantId, Timestamp,
    TriggerDef,
};

/// The most events one pump hands on.
pub const PUMP_BATCH: usize = 100;

/// How often a pump decides again after losing a race.
const MAX_RETRIES: usize = 8;
/// How many rounds a pump may spend loading the records it names.
const MAX_ROUNDS: usize = 6;

/// What one pump handed on.
#[derive(Clone, Debug, Default)]
pub struct Pumped {
    /// The events it handed on, with their positions, in order.
    pub events: Vec<(u64, SegmentEvent)>,
    /// The runs it started.
    pub started: usize,
    /// The triggers it declined.
    pub declined: usize,
    /// The segments of the tenant when it decided.
    pub catalog: Vec<Segment>,
    /// A snapshot holding the subject record of the person of each event, when the pump was asked
    /// to load them (for a subscriber's read decision).
    pub state: Option<State>,
}

/// Hand the new events of the segment log of `tenant` to its triggers, at `now`: one batch, which
/// moves the cursor past the events it covers and creates, in the same transaction, a job for each
/// run to start, a record for each trigger declined, and, when events remain, the job that hands
/// on the next batch. With `load_subjects` the subject record of the person of every event is
/// loaded as well and returned, for the notices of subscribers; without it only the records the
/// triggers' decisions read are.
pub fn pump(
    store: &dyn Store,
    tenant: &TenantId,
    now: Timestamp,
    load_subjects: bool,
) -> Result<Pumped, ExecuteError> {
    let mut retries = 0;
    loop {
        let cursor = store.segment_cursor(tenant)?;
        let events = store.segment_events_after(tenant, cursor, PUMP_BATCH)?;
        let Some(last) = events.last().map(|(position, _)| *position) else {
            return Ok(Pumped::default());
        };
        let header = store.load(tenant, &[])?;
        let Some(stored) = header.tenant.as_deref() else {
            return Ok(Pumped::default());
        };
        let catalog = store.segment_catalog(tenant)?;
        let triggers: Vec<TriggerDef> = stored
            .schema
            .triggers
            .iter()
            .flat_map(|all| all.values())
            .filter(|trigger| trigger.paused != Some(true))
            .cloned()
            .collect();
        let ctx = Context {
            tenant: tenant.clone(),
            principal: Principal::System,
            session: None,
            now,
            correlation: format!("segment-events:{cursor}"),
            verified_address: None,
            idempotency_key: None,
            language: None,
            hop: None,
        };
        // What every decision reads: the subject record of the person of each event a trigger
        // matches, or of every event when a subscriber's decision follows.
        let mut needs: BTreeSet<Need> = BTreeSet::new();
        for (_, logged) in &events {
            let Some(segment) = catalog.iter().find(|s| s.id == logged.event.segment) else {
                continue;
            };
            let matched = triggers
                .iter()
                .any(|trigger| segment_flow::matches(&trigger.on, &logged.event));
            if matched || load_subjects {
                needs.insert(segment_flow::subject_need(segment, &logged.event.person));
            }
        }
        let mut rounds = 0;
        let (state, work) = loop {
            let list: Vec<Need> = needs.iter().cloned().collect();
            let state = store.load(tenant, &list)?;
            match decide_all(&state, &ctx, &catalog, &triggers, &events) {
                Ok(work) => break (state, work),
                Err(DomainError {
                    kind: ErrorKind::InsufficientState,
                    missing: Some(need),
                    ..
                }) if rounds < MAX_ROUNDS && !needs.contains(&need) => {
                    needs.insert(need);
                    rounds += 1;
                }
                Err(error) => return Err(ExecuteError::Domain(error)),
            }
        };
        let started = work.jobs.len();
        let declined = work.exceptions.len();
        let mut work = work;
        if events.len() == PUMP_BATCH {
            // More may follow: the job that hands on the next batch is made with this one.
            work.jobs
                .push(segment_jobs::events_job(&format!("more:{last}"), now));
        }
        let commit = SegmentCommit {
            tenant: tenant.clone(),
            revision: stored.revision,
            write: SegmentWrite {
                published: Some((cursor, last)),
                ..SegmentWrite::default()
            },
            work,
            now,
        };
        match store.commit_segments(&commit) {
            Ok(_) => {
                return Ok(Pumped {
                    events,
                    started,
                    declined,
                    catalog,
                    state: load_subjects.then_some(state),
                })
            }
            Err(StoreError::Conflict { .. }) if retries < MAX_RETRIES => retries += 1,
            Err(error) => return Err(error.into()),
        }
    }
}

/// The jobs and records the triggers decide for `events`, in event order and, for one event, in
/// trigger id order.
fn decide_all(
    state: &State,
    ctx: &Context,
    catalog: &[Segment],
    triggers: &[TriggerDef],
    events: &[(u64, SegmentEvent)],
) -> Result<WorkerWrites, DomainError> {
    let tenant = state
        .tenant
        .as_deref()
        .ok_or_else(|| DomainError::new(ErrorKind::TenantNotFound, "no such tenant"))?;
    let lookup = Lookup {
        tenant,
        coverage: &state.coverage,
    };
    let mut work = WorkerWrites::default();
    for (position, logged) in events {
        let Some(segment) = catalog.iter().find(|s| s.id == logged.event.segment) else {
            // The segment was removed after the event was written: nothing to start.
            continue;
        };
        for trigger in triggers {
            if !segment_flow::matches(&trigger.on, &logged.event) {
                continue;
            }
            let subject = segment_flow::subject_of(&lookup, segment, &logged.event.person)?;
            let effect = segment_flow::decide(
                lookup,
                ctx,
                *position,
                logged,
                subject.map(|record| &**record),
                trigger,
            )?;
            match effect {
                Some(Effect::StartAutomation(start)) => {
                    work.jobs.push(jobs::start_job(&start, ctx.now))
                }
                Some(Effect::DeclineAutomation(decline)) => work
                    .exceptions
                    .push(jobs::declined_record(&ctx.tenant, &decline, ctx.now)),
                _ => {}
            }
        }
    }
    Ok(work)
}
