//! The segment jobs of the worker (WG-5165; SEG-1543 to SEG-1546, SEG-014, D-6555, D-7280): batches
//! of a segment, its scheduled refresh, and the event log that starts the triggers on entry and
//! exit. What a job is for is [`strata_storage_api::segment_jobs`]; this module runs it.
//!
//! - **A batch job** finds the continuation row it was made for and runs it with
//!   [`segments::run_batch`] in a transaction of its own (CT-297). The transaction that runs a
//!   batch creates the job of the next batch, so a continuation is never left without a job, and
//!   a job whose row has moved on (an attempt made again after a crash) or is gone runs nothing.
//!   The job then hands the events its batch wrote to the triggers ([`segment_pump::pump`]), as
//!   the same job that processed the batch.
//! - **A refresh job** is one tick of the schedule of a time-dependent segment: it asks for the
//!   next tick, then either starts the refresh ([`segments::start_refresh`]: the persons whose
//!   wake time has passed for an indexable predicate, every person otherwise) or, when the daily
//!   job quota of the tenant has no room, records the skip on the segment
//!   ([`segments::skip_refresh`]). A refresh refuses no commit and is never held back by the
//!   change of another; a missed tick is caught up by the next one (SEG-1544).
//! - **An events job** hands the new entries of the log on.
//!
//! **The quota.** A job counts against the job admission rate (SCH-302) by being a job: the worker
//! releases the jobs of a tenant at its budget a pass, whatever made them. It counts against the
//! daily job quota (D-508) the same way: [`jobs_today`] counts every job the tenant created in
//! the quota day, a segment's among them, and a refresh that finds the count at the quota is
//! skipped. Until plans carry quotas (BIL-150) the quota is [`STANDARD_JOBS_PER_DAY`], or what
//! [`set_daily_job_quota`] gave the tenant.

use super::steps::Outcome;
use crate::app::App;
use crate::hub::PublishedSegment;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use strata_kernel::segments::SegmentId;
use strata_kernel::{Context, Principal, TenantId, Timestamp};
use strata_storage_api::jobs::{JobReport, Lease, WorkerWrites};
use strata_storage_api::segment_jobs::{self, SegmentWork};
use strata_storage_api::segment_pump;
use strata_storage_api::segments;
use strata_storage_api::{Store, StoreResult};

/// The jobs a tenant of the Standard plan may create in one quota day (D-508).
pub const STANDARD_JOBS_PER_DAY: u64 = 10_000;

/// The quota of the tenants that were given one.
static QUOTAS: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());

/// Give `tenant` a daily job quota of `jobs` in place of [`STANDARD_JOBS_PER_DAY`], for the
/// process: the quota of the plan of the tenant, until plans are kept (BIL-150).
pub fn set_daily_job_quota(tenant: &TenantId, jobs: u64) {
    if let Ok(mut quotas) = QUOTAS.lock() {
        quotas.insert(tenant.as_str().to_string(), jobs);
    }
}

/// The daily job quota of `tenant`.
pub fn daily_job_quota(tenant: &TenantId) -> u64 {
    QUOTAS
        .lock()
        .ok()
        .and_then(|quotas| quotas.get(tenant.as_str()).copied())
        .unwrap_or(STANDARD_JOBS_PER_DAY)
}

/// How many jobs the tenant created in the quota day that holds `now`, counting no further than
/// `stop_at` plus one page: the jobs of every kind, newest first, until one made before the day.
pub fn jobs_today(
    store: &dyn Store,
    tenant: &TenantId,
    now: Timestamp,
    stop_at: u64,
) -> StoreResult<u64> {
    const PAGE: usize = 200;
    let start = segment_jobs::day_start(now);
    let mut count = 0u64;
    let mut before = None;
    loop {
        let page = store.jobs_list(tenant, before, PAGE)?;
        let Some(oldest) = page.last() else {
            return Ok(count);
        };
        before = Some(oldest.seq);
        for job in &page {
            if job.created_at < start {
                return Ok(count);
            }
            count += 1;
        }
        if count >= stop_at || page.len() < PAGE {
            return Ok(count);
        }
    }
}

/// Whether the daily job quota of the tenant has room for one more job at `now`.
pub fn has_room(store: &dyn Store, tenant: &TenantId, now: Timestamp) -> StoreResult<bool> {
    let quota = daily_job_quota(tenant);
    Ok(jobs_today(store, tenant, now, quota)? < quota)
}

/// The context the worker decides segment work in: the system, no session, the job as the
/// correlation, so that the events a batch writes name the job that wrote them.
fn platform(tenant: &TenantId, job_key: &str, now: Timestamp) -> Context {
    Context {
        tenant: tenant.clone(),
        principal: Principal::System,
        session: None,
        now,
        correlation: format!("segj:{job_key}"),
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
        agent: None,
    }
}

fn done() -> Outcome {
    Outcome {
        report: JobReport::Succeeded,
        writes: WorkerWrites::default(),
    }
}

/// A failed attempt: retried while the policy allows. A lease that expired on its last attempt
/// fails the job for good.
fn failed(lease: &Lease, error: impl ToString) -> Outcome {
    Outcome {
        report: JobReport::Failed {
            error: error.to_string(),
            retry: !lease.exhausted,
        },
        writes: WorkerWrites::default(),
    }
}

/// Run the segment work a lease holds.
pub fn run(
    app: &App,
    tenant: &TenantId,
    lease: &Lease,
    work: &SegmentWork,
    now: Timestamp,
) -> Outcome {
    if lease.exhausted {
        let error = lease.job.errors.last().cloned();
        return failed(
            lease,
            error.unwrap_or_else(|| "the lease expired".to_string()),
        );
    }
    match work {
        SegmentWork::Batch {
            segment,
            scope,
            version,
            after,
        } => batch(app, tenant, lease, *segment, scope, *version, *after, now),
        SegmentWork::Refresh { segment, due } => refresh(app, tenant, lease, *segment, *due, now),
        SegmentWork::Events => match hand_on(app, tenant, now) {
            Ok(()) => done(),
            Err(error) => failed(lease, error),
        },
        SegmentWork::FanOut { .. } => super::fan_out::run_batch(app, tenant, lease, work, now),
    }
}

/// Hand the new events of the segment log on to the triggers and, when subscribers listen, to
/// them (D-7280).
pub fn hand_on(app: &App, tenant: &TenantId, now: Timestamp) -> Result<(), String> {
    let listening = app.hub.segments.listening(tenant);
    let pumped = segment_pump::pump(app.store.as_ref(), tenant, now, listening)
        .map_err(|error| error.to_string())?;
    if let Some(state) = pumped.state {
        let state = Arc::new(state);
        let catalog = Arc::new(pumped.catalog);
        for (position, event) in pumped.events {
            app.hub.segments.send(
                tenant,
                Arc::new(PublishedSegment {
                    position,
                    event,
                    state: state.clone(),
                    catalog: catalog.clone(),
                }),
            );
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn batch(
    app: &App,
    tenant: &TenantId,
    lease: &Lease,
    segment: SegmentId,
    scope: &str,
    version: u64,
    after: Option<strata_kernel::RecordId>,
    now: Timestamp,
) -> Outcome {
    let store = app.store.as_ref();
    let ctx = platform(tenant, &lease.job.key, now);
    let pending = match store.segment_batches(tenant) {
        Ok(pending) => pending,
        Err(error) => return failed(lease, error),
    };
    let stored = pending
        .iter()
        .find(|b| b.segment == segment && b.scope.key() == scope && b.version == version);
    // The row decides what runs. A row that has reached another position was run by an attempt
    // that was not reported: its transaction created the job of the next batch already. A row
    // that is gone was run to its end, or superseded by a newer version (SEG-025).
    if let Some(row) = stored.filter(|row| row.after == after) {
        if let Err(error) = segments::run_batch(store, &ctx, row) {
            return failed(lease, error);
        }
    }
    match hand_on(app, tenant, now) {
        Ok(()) => done(),
        Err(error) => failed(lease, error),
    }
}

fn refresh(
    app: &App,
    tenant: &TenantId,
    lease: &Lease,
    segment: SegmentId,
    due: Timestamp,
    now: Timestamp,
) -> Outcome {
    let store = app.store.as_ref();
    let ctx = platform(tenant, &lease.job.key, now);
    let catalog = match store.segment_catalog(tenant) {
        Ok(catalog) => catalog,
        Err(error) => return failed(lease, error),
    };
    // A segment that is gone, or no longer reads the time, ends its schedule (SEG-026, SEG-020);
    // a tick that is not a multiple of the interval belongs to the schedule of an earlier
    // interval, which the definition of the new one replaced.
    let Some(found) = catalog.iter().find(|s| s.id == segment) else {
        return done();
    };
    let Some(interval) = segment_jobs::interval_ms(found) else {
        return done();
    };
    if due.0.rem_euclid(interval) != 0 {
        return done();
    }
    let step = || -> Result<(), String> {
        if has_room(store, tenant, now).map_err(|e| e.to_string())? {
            segments::start_refresh(store, &ctx, segment)
                .map(|_| ())
                .map_err(|e| e.to_string())
        } else {
            segments::skip_refresh(store, &ctx, segment).map_err(|e| e.to_string())
        }
    };
    if let Err(error) = step() {
        return failed(lease, error);
    }
    // The next tick is asked for in the report of this one, so the schedule has one job at a time.
    let mut writes = WorkerWrites::default();
    if let Some(next) = segment_jobs::next_refresh(found, now) {
        writes.jobs.push(segment_jobs::refresh_job(segment, next));
    }
    Outcome {
        report: JobReport::Succeeded,
        writes,
    }
}
