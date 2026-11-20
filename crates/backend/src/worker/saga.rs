//! Compensation (WG-5156, WFL-005, SCH-1525, WFL-2686, D-7683): when a step fails for good, the
//! steps that completed and declare a compensating step (`compensate`) are undone in reverse
//! order.
//!
//! - **Starting** ([`begin`]): the failing step has spent its attempts. If a completed step
//!   declares a compensating step, the run goes to `compensating` and one job is created for the
//!   last such step. The dead-lettered exception record of the failed step is written as before.
//! - **Chaining** ([`compensate`]): each job runs one compensating step as the automation
//!   principal, with a key derived from the run, the compensating step and the position of the
//!   step it undoes, the same on every attempt, so a retry compensates once. When it succeeds the
//!   undone step is marked `compensated` and the job of the next earlier step is created; when
//!   none is left the run ends `failed`, its error naming the step that failed.
//! - **A compensation that fails for good** leaves the run `compensating`, with a dead-lettered
//!   exception record that names the job. The owner sees it in the run and in the exception list
//!   and releases it (settle-automation-exception): the job gets one more attempt with the same
//!   key, and the chain goes on from there.

use super::steps::{act, automation_of, exception, job_failure, step_policy, Outcome, StepError};
use crate::app::App;
use strata_kernel::{canon, AutomationDef, StepName, TenantId, Timestamp, WorkflowId};
use strata_storage_api::jobs::{
    self, ExceptionKind, JobReport, JobWork, Lease, NewJob, RunRecord, RunState, StepRecord,
    StepState, WorkerWrites,
};

/// The most steps of one run the chain looks through.
const MAX_STEPS: usize = 10_000;

/// The key every attempt of the compensation of step `of_sequence` carries.
pub(super) fn compensation_key(
    tenant: &TenantId,
    automation: &WorkflowId,
    run: &str,
    step: &StepName,
    of_sequence: u64,
) -> String {
    canon::hash_bytes(
        format!("strata/compensation-key/v1\0{tenant}\0{automation}\0{run}\0{step}\0{of_sequence}")
            .as_bytes(),
    )
}

/// The job of the compensation of the step at `of_sequence`: its key is the run and the position,
/// so the job exists once however often a failure is handled.
fn compensation_job(
    run: &str,
    def: &AutomationDef,
    compensating: &StepName,
    of_sequence: u64,
    now: Timestamp,
) -> NewJob {
    NewJob {
        key: format!("compensate:{run}:{of_sequence}"),
        work: JobWork::Compensate {
            run: run.to_string(),
            automation: def.id.clone(),
            step: compensating.clone(),
            of_sequence,
        },
        due_at: now,
        policy: def
            .steps
            .get(compensating)
            .map(step_policy)
            .unwrap_or_default(),
    }
}

/// The last step of a run before position `below` that completed and declares a compensating
/// step: its position and the compensating step.
fn last_to_undo(
    app: &App,
    tenant: &TenantId,
    def: &AutomationDef,
    run: &str,
    below: u64,
) -> Result<Option<(u64, StepName)>, String> {
    let steps = app
        .store
        .automation_steps(tenant, run, None, MAX_STEPS)
        .map_err(|error| error.to_string())?;
    Ok(steps
        .iter()
        .rev()
        .filter(|step| step.sequence < below && step.state == StepState::Completed)
        .find_map(|step| {
            def.steps
                .get(&step.step)
                .and_then(|declared| declared.compensate.clone())
                .map(|compensating| (step.sequence, compensating))
        }))
}

/// A step failed for good at `failed`: when a completed step declares a compensation, put the run
/// in `compensating` and return the job of the last one; otherwise leave the run as the failure
/// made it.
pub(super) fn begin(
    app: &App,
    tenant: &TenantId,
    def: &AutomationDef,
    run: &mut RunRecord,
    failed: u64,
    now: Timestamp,
) -> Vec<NewJob> {
    match last_to_undo(app, tenant, def, &run.run, failed) {
        Ok(Some((sequence, compensating))) => {
            run.state = RunState::Compensating;
            run.ended_at = None;
            vec![compensation_job(
                &run.run,
                def,
                &compensating,
                sequence,
                now,
            )]
        }
        Ok(None) => Vec::new(),
        Err(why) => {
            let note = format!("compensation could not be planned: {why}");
            run.error = Some(jobs::clip_error(&format!(
                "{}; {note}",
                run.error.clone().unwrap_or_default()
            )));
            Vec::new()
        }
    }
}

/// Run the compensating step `step` of the step at `of_sequence` of a run, and chain to the next
/// earlier one.
#[allow(clippy::too_many_arguments)]
pub(super) fn compensate(
    app: &App,
    tenant: &TenantId,
    lease: &Lease,
    run_id: &str,
    automation: &WorkflowId,
    step: &StepName,
    of_sequence: u64,
    now: Timestamp,
) -> Outcome {
    let job = &lease.job;
    let nothing = || Outcome {
        report: JobReport::Succeeded,
        writes: WorkerWrites::default(),
    };
    let mut run = match app.store.automation_run(tenant, run_id) {
        Ok(Some(run)) => run,
        Ok(None) => return nothing(),
        Err(error) => return job_failure(tenant, lease, automation, error.to_string(), now),
    };
    if run.state != RunState::Compensating {
        return nothing();
    }
    let mut undone: StepRecord = match super::approvals::step_at(app, tenant, run_id, of_sequence) {
        Ok(Some(record)) => record,
        Ok(None) => return nothing(),
        Err(error) => return job_failure(tenant, lease, automation, error.to_string(), now),
    };
    let schema = match app.store.load(tenant, &[]) {
        Ok(state) => state.tenant.map(|found| found.schema.clone()),
        Err(error) => return job_failure(tenant, lease, automation, error.to_string(), now),
    };
    let Some(def) = schema
        .as_deref()
        .and_then(|schema| automation_of(schema, automation))
    else {
        // Removing an automation cancels its open runs (D-6539).
        run.state = RunState::Cancelled;
        run.ended_at = Some(now);
        run.error = Some(format!("automation {automation} was removed"));
        return Outcome {
            report: JobReport::Succeeded,
            writes: WorkerWrites {
                runs: vec![run],
                ..WorkerWrites::default()
            },
        };
    };
    let pinned_def = super::steps::pinned(&run, def);
    let def = &pinned_def;
    let attempt: Result<(), StepError> = if undone.state == StepState::Compensated {
        // Done already: an attempt made again after the report was lost.
        Ok(())
    } else if lease.exhausted {
        Err(StepError::fatal(
            job.errors
                .last()
                .cloned()
                .unwrap_or_else(|| "the lease expired".to_string()),
        ))
    } else {
        match def.steps.get(step) {
            None => Err(StepError::fatal(format!(
                "automation {automation} has no step {step}: its definition changed"
            ))),
            Some(step_def) => {
                let key = compensation_key(tenant, automation, run_id, step, of_sequence);
                // A compensation is one step: where it would go next does not matter.
                act(app, tenant, def, step_def, &run, &key, job, now).map(|_| ())
            }
        }
    };
    match attempt {
        Ok(()) => {
            undone.state = StepState::Compensated;
            let mut writes = WorkerWrites {
                steps: vec![undone],
                ..WorkerWrites::default()
            };
            match last_to_undo(app, tenant, def, run_id, of_sequence) {
                Ok(Some((sequence, compensating))) => {
                    writes
                        .jobs
                        .push(compensation_job(run_id, def, &compensating, sequence, now));
                }
                Ok(None) => {
                    run.state = RunState::Failed;
                    run.ended_at = Some(now);
                    run.error = Some(jobs::clip_error(&format!(
                        "{}; compensated",
                        run.error.clone().unwrap_or_default()
                    )));
                    writes.runs.push(run);
                }
                Err(why) => return job_failure(tenant, lease, automation, why, now),
            }
            Outcome {
                report: JobReport::Succeeded,
                writes,
            }
        }
        Err(error) => {
            let report = JobReport::Failed {
                error: error.message.clone(),
                retry: error.retry,
            };
            let fails = lease.exhausted || jobs::report_fails_job(job, &report);
            let mut writes = WorkerWrites::default();
            if fails {
                // The run stays compensating: the owner sees the record, and releasing it gives
                // this job one more attempt under the same key.
                run.error = Some(jobs::clip_error(&format!(
                    "compensation {step} of step {} failed: {}",
                    undone.step, error.message
                )));
                writes.exceptions.push(exception(
                    tenant,
                    ExceptionKind::DeadLettered,
                    "attempts-exhausted",
                    automation,
                    Some(&run),
                    Some(step),
                    job,
                    format!("compensation {step} of step {} failed", undone.step),
                    now,
                ));
                writes.runs.push(run);
            }
            Outcome { report, writes }
        }
    }
}
