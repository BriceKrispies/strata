//! Taking leases on the due jobs of one tenant, running them and reporting each outcome
//! (SCH-010 to SCH-014).
//!
//! The order is always lease, run, report. A worker that stops between running and reporting
//! leaves the job leased; when the lease expires the job is offered again (SCH-010), its late
//! report is refused by the fencing token (SCH-011), and the attempt made again carries the same
//! step key, so its effect is not applied twice (SCH-015).

use super::steps;
use crate::app::{App, LOG};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use strata_kernel::{TenantId, Timestamp};
use strata_storage_api::jobs::{JobReport, Lease, Reported};
use strata_storage_api::StoreError;

/// Run the job of a lease and report its outcome, with `clock` read when the job starts and again
/// when it is reported. This is the report-job-result operation of this deployment, in process: the
/// worker holds the store and calls it directly, so no route serves the contract and no service
/// credential crosses a network (the contract's auth is the platform component's own).
///
/// Implements: CT-083
pub fn run_leased(
    app: &App,
    tenant: &TenantId,
    lease: &Lease,
    clock: &dyn Fn() -> Timestamp,
) -> Result<Reported, StoreError> {
    let outcome = steps::run_job(app, tenant, lease, clock());
    let reported = app.store.job_report(
        tenant,
        &lease.job.id,
        lease.token,
        &outcome.report,
        &outcome.writes,
        clock(),
    )?;
    let outcome_name = match &outcome.report {
        JobReport::Succeeded => "succeeded",
        JobReport::Failed { .. } => "failed",
        JobReport::Deferred { .. } => "deferred",
    };
    match &reported {
        Reported::Recorded(job) => LOG.info(
            "worker.reported",
            "",
            json!({
                "tenant": tenant.as_str(),
                "job": job.id,
                "attempt": job.attempt,
                "outcome": outcome_name,
                "state": job.state.as_str(),
            }),
        ),
        Reported::Refused(why) => LOG.warn(
            "worker.report_refused",
            "",
            json!({ "tenant": tenant.as_str(), "job": lease.job.id, "why": why }),
        ),
    }
    Ok(reported)
}

/// Lease and run the due jobs of one tenant, one at a time, at most `budget` of them, until none
/// is due or `stop` is set. Returns how many ran. This is the lease-job operation of this
/// deployment, in process (see [`run_leased`]): a worker that cannot reach the tenant file takes
/// no lease, because the lease is a write to that file (SCH-301).
///
/// Implements: CT-082
pub fn drain_tenant(
    app: &App,
    tenant: &TenantId,
    worker: &str,
    budget: usize,
    stop: &AtomicBool,
    clock: &dyn Fn() -> Timestamp,
) -> Result<usize, StoreError> {
    let mut ran = 0;
    while ran < budget && !stop.load(Ordering::Relaxed) {
        let Some(lease) = app.store.job_lease(tenant, worker, clock())? else {
            break;
        };
        run_leased(app, tenant, &lease, clock)?;
        ran += 1;
    }
    Ok(ran)
}
