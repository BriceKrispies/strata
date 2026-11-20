//! The `fan-out` automation step (WG-5152; WFL-1538, D-6535): run a step once for each member of a
//! segment, in batches, each batch one job.
//!
//! - **Starting.** The job of the fan-out step ([`begin`]) does not complete the step: it leaves the
//!   step `waiting` and creates the job of the first batch. The run stays running.
//! - **A batch** ([`run_batch`]) reads the members of the segment when it runs, at most the
//!   step's batch size and never more than one hundred ([`MAX_BATCH`]), after the cursor the job
//!   carries (the last member the batch before ended at, in principal order), and runs the step
//!   named by `each` for each, with the member bound as `param.member` and decided as the
//!   automation principal with its run-as roles, as every step is (D-6525). The `each` step is a
//!   leaf: its effect is carried out and the step it names next is not followed, and a wait in it
//!   does not wait.
//! - **Once for each member.** The idempotency key of the step for a member is the step key of
//!   the tenant, automation, run and step with the member added (SCH-1524, D-6528), the same on
//!   every attempt. A batch that is attempted again after a crash between doing and reporting
//!   asks the kernel for the same intents, which answers each with the result it recorded: no
//!   member is done twice. The job of the next batch is created in the transaction that reports
//!   this one, keyed by the run, the step and the batch number, so it is created once.
//! - **Completing.** The batch that finds no member after its own completes the step: its result
//!   is `fan-out` with the members done and the batches run (WFL-1523), bound as
//!   `param.<step>_members` and `param.<step>_batches`, and the run goes to the next step of the
//!   fan-out, or ends (WFL-1538).
//! - **A failure** of a member is the failure of the batch: the batch is attempted again while its
//!   policy (the step's retry and timeout) allows, and then the step and the run fail.

use super::steps::{self, Outcome, StepError};
use crate::app::App;
use strata_kernel::{
    canon, AutomationDef, AutomationStep, AutomationStepDef, PrincipalId, StepName, TenantId,
    Timestamp, Value, WorkflowId,
};
use strata_model::types::AutomationStepResult;
use strata_storage_api::jobs::{
    self, ExceptionKind, JobReport, JobWork, Lease, NewJob, RunRecord, RunState, StepRecord,
    StepState, WorkerWrites,
};
use strata_storage_api::segment_jobs::SegmentWork;

/// The most members one batch of a fan-out works on (D-6535).
pub const MAX_BATCH: usize = 100;

/// The idempotency key of a fan-out step for one member: the step key with the member added.
pub fn member_key(
    tenant: &TenantId,
    automation: &WorkflowId,
    run: &str,
    step: &StepName,
    member: &PrincipalId,
) -> String {
    canon::hash_bytes(
        format!("strata/step-key/v1\0{tenant}\0{automation}\0{run}\0{step}\0{member}").as_bytes(),
    )
}

/// The job of batch number `batch` (from zero) of a fan-out step, after the member `after`, with
/// `members` done so far.
#[allow(clippy::too_many_arguments)]
fn batch_job(
    run: &str,
    automation: &WorkflowId,
    step: &StepName,
    def: &AutomationStepDef,
    sequence: u64,
    after: Option<PrincipalId>,
    batch: u32,
    members: u64,
    due: Timestamp,
) -> NewJob {
    NewJob {
        key: format!("fanout:{run}:{sequence}:{batch}"),
        work: JobWork::Segment(SegmentWork::FanOut {
            run: run.to_string(),
            automation: automation.clone(),
            step: step.clone(),
            sequence,
            after,
            batch,
            members,
        }),
        due_at: due,
        policy: steps::step_policy(def),
    }
}

/// Start a fan-out step: the step waits for its batches and the job of the first one is created,
/// in the report of the job of the step.
pub(super) fn begin(
    automation: &WorkflowId,
    step: &StepName,
    step_def: &AutomationStepDef,
    run: RunRecord,
    mut record: StepRecord,
    sequence: u64,
    now: Timestamp,
) -> Outcome {
    record.state = StepState::Waiting;
    let job = batch_job(
        &run.run, automation, step, step_def, sequence, None, 0, 0, now,
    );
    Outcome {
        report: JobReport::Succeeded,
        writes: WorkerWrites {
            runs: vec![run],
            steps: vec![record],
            exceptions: Vec::new(),
            jobs: vec![job],
            ..WorkerWrites::default()
        },
    }
}

/// A failed attempt that changes nothing else: retried while the policy allows.
fn failed(lease: &Lease, error: impl ToString, retry: bool) -> Outcome {
    Outcome {
        report: JobReport::Failed {
            error: error.to_string(),
            retry: retry && !lease.exhausted,
        },
        writes: WorkerWrites::default(),
    }
}

/// The record of the fan-out step as it was written, or one made again from what is known.
fn step_record(
    app: &App,
    tenant: &TenantId,
    lease: &Lease,
    run: &RunRecord,
    step: &StepName,
    sequence: u64,
) -> StepRecord {
    let found = app
        .store
        .automation_steps(tenant, &run.run, Some(sequence.saturating_sub(1)), 1)
        .ok()
        .and_then(|steps| steps.into_iter().find(|held| held.sequence == sequence));
    found.unwrap_or_else(|| StepRecord {
        run: run.run.clone(),
        approval: None,
        sequence,
        step: step.clone(),
        state: StepState::Waiting,
        attempts: lease.job.attempt,
        job: Some(lease.job.id.clone()),
        idempotency_key: String::new(),
        result: None,
        errors: Vec::new(),
        started_at: lease.job.created_at,
        ended_at: None,
    })
}

/// Why a batch cannot run at all and no attempt will change that: the run, step and segment it
/// names are as the definition of the automation says them to be.
fn definition<'a>(
    def: &'a AutomationDef,
    step: &StepName,
) -> Result<(&'a AutomationStep, &'a AutomationStepDef), StepError> {
    let fan = def
        .steps
        .get(step)
        .ok_or_else(|| StepError::fatal(format!("automation {} has no step {step}", def.id)))?;
    let AutomationStep::FanOut { each, .. } = &fan.action else {
        return Err(StepError::fatal(format!("step {step} is not a fan-out")));
    };
    let each_def = def.steps.get(each).ok_or_else(|| {
        StepError::fatal(format!(
            "the fan-out names the step {each}, which does not exist"
        ))
    })?;
    if matches!(each_def.action, AutomationStep::FanOut { .. }) {
        return Err(StepError::fatal(
            "a fan-out cannot run a fan-out for each member",
        ));
    }
    Ok((&fan.action, each_def))
}

/// Run one batch of a fan-out (see the module).
pub(super) fn run_batch(
    app: &App,
    tenant: &TenantId,
    lease: &Lease,
    work: &SegmentWork,
    now: Timestamp,
) -> Outcome {
    let SegmentWork::FanOut {
        run: run_id,
        automation,
        step,
        sequence,
        after,
        batch,
        members,
    } = work
    else {
        return failed(lease, "not the batch of a fan-out", false);
    };
    let job = &lease.job;
    let mut run = match app.store.automation_run(tenant, run_id) {
        Ok(Some(run)) => run,
        Ok(None) => return failed(lease, format!("no run {run_id}"), true),
        Err(error) => return failed(lease, error, true),
    };
    if run.state.ended() {
        // The run ended (cancelled, failed) after this job was created: nothing to do.
        return Outcome {
            report: JobReport::Succeeded,
            writes: WorkerWrites::default(),
        };
    }
    let schema = match app.store.load(tenant, &[]) {
        Ok(state) => state.tenant.map(|found| found.schema.clone()),
        Err(error) => return failed(lease, error, true),
    };
    let Some(def) = schema
        .as_deref()
        .and_then(|schema| steps::automation_of(schema, automation))
    else {
        // The automation was removed while the run was open: the run is cancelled (D-6539).
        let why = format!("automation {automation} was removed");
        let mut record = step_record(app, tenant, lease, &run, step, *sequence);
        record.state = StepState::Cancelled;
        record.ended_at = Some(now);
        run.state = RunState::Cancelled;
        run.ended_at = Some(now);
        run.error = Some(why.clone());
        let removed = steps::exception(
            tenant,
            ExceptionKind::Declined,
            "automation-removed",
            automation,
            Some(&run),
            Some(step),
            job,
            why,
            now,
        );
        return Outcome {
            report: JobReport::Succeeded,
            writes: WorkerWrites {
                runs: vec![run],
                steps: vec![record],
                exceptions: vec![removed],
                jobs: Vec::new(),
                ..WorkerWrites::default()
            },
        };
    };
    let pinned_def = steps::pinned(&run, def);
    let def = &pinned_def;
    let attempt = (|| -> Result<(Vec<PrincipalId>, bool), StepError> {
        let (action, each_def) = definition(def, step)?;
        let AutomationStep::FanOut {
            segment,
            batch: size,
            each,
        } = action
        else {
            return Err(StepError::fatal("not a fan-out"));
        };
        let catalog = app
            .store
            .segment_catalog(tenant)
            .map_err(|e| StepError::retry(e.to_string()))?;
        if !catalog.iter().any(|s| s.id == *segment) {
            return Err(StepError::fatal(format!(
                "segment {segment} does not exist"
            )));
        }
        let size = (*size as usize).clamp(1, MAX_BATCH);
        // The members as they are when the batch runs, one more than the batch to know whether
        // another follows.
        let rows = app
            .store
            .segment_members(tenant, *segment, after.as_ref(), size + 1)
            .map_err(|e| StepError::retry(e.to_string()))?;
        let more = rows.len() > size;
        let mut reached = Vec::new();
        for membership in rows.iter().take(size) {
            let mut for_member: RunRecord = run.clone();
            for_member.params.insert(
                "member".to_string(),
                Value::Str(membership.person.as_str().to_string()),
            );
            let key = member_key(tenant, automation, run_id, each, &membership.person);
            steps::act(app, tenant, def, each_def, &for_member, &key, job, now)?;
            reached.push(membership.person.clone());
        }
        Ok((reached, more))
    })();
    let (reached, more) = match attempt {
        Ok(found) => found,
        Err(error) => return member_failure(app, tenant, lease, run, step, *sequence, error, now),
    };
    let members = members + reached.len() as u64;
    let Some(fan) = def.steps.get(step) else {
        return failed(
            lease,
            format!("automation {automation} has no step {step}"),
            false,
        );
    };
    if more {
        // The next batch starts after the last member of this one; its job is created in the
        // report of this one, once for its key.
        let last = reached.last().cloned().or_else(|| after.clone());
        return Outcome {
            report: JobReport::Succeeded,
            writes: WorkerWrites {
                jobs: vec![batch_job(
                    run_id,
                    automation,
                    step,
                    fan,
                    *sequence,
                    last,
                    batch + 1,
                    members,
                    now,
                )],
                ..WorkerWrites::default()
            },
        };
    }
    // The last batch: the step completes and the run goes on.
    let batches = batch + 1;
    let mut record = step_record(app, tenant, lease, &run, step, *sequence);
    record.state = StepState::Completed;
    record.ended_at = Some(now);
    record.result = serde_json::to_value(AutomationStepResult::FanOut { members, batches }).ok();
    run.params.insert(
        format!("{step}_members"),
        Value::Int(i64::try_from(members).unwrap_or(i64::MAX)),
    );
    run.params
        .insert(format!("{step}_batches"), Value::Int(i64::from(batches)));
    run.steps = *sequence;
    let mut follow = Vec::new();
    match &fan.next {
        Some(name) => match def.steps.get(name) {
            Some(next_def) => {
                follow.push(steps::step_job(
                    run_id,
                    automation,
                    name,
                    next_def,
                    sequence + 1,
                    now,
                ));
                run.state = RunState::Running;
                run.step = Some(name.clone());
            }
            None => {
                run.state = RunState::Failed;
                run.ended_at = Some(now);
                run.error = Some(format!("no step {name}"));
            }
        },
        None => {
            run.state = RunState::Completed;
            run.ended_at = Some(now);
        }
    }
    Outcome {
        report: JobReport::Succeeded,
        writes: WorkerWrites {
            runs: vec![run],
            steps: vec![record],
            exceptions: Vec::new(),
            jobs: follow,
            ..WorkerWrites::default()
        },
    }
}

/// A member's step failed: the batch is attempted again while its policy allows; when it does not,
/// the step and the run fail with the error recorded on both.
#[allow(clippy::too_many_arguments)]
fn member_failure(
    app: &App,
    tenant: &TenantId,
    lease: &Lease,
    mut run: RunRecord,
    step: &StepName,
    sequence: u64,
    error: StepError,
    now: Timestamp,
) -> Outcome {
    let job = &lease.job;
    let report = JobReport::Failed {
        error: error.message.clone(),
        retry: error.retry,
    };
    let fails = lease.exhausted || jobs::report_fails_job(job, &report);
    let mut writes = WorkerWrites::default();
    if fails {
        let mut record = step_record(app, tenant, lease, &run, step, sequence);
        record
            .errors
            .push(format!("attempt {}: {}", job.attempt, error.message));
        record.state = StepState::Failed;
        record.ended_at = Some(now);
        run.state = RunState::Failed;
        run.ended_at = Some(now);
        run.error = Some(jobs::clip_error(&error.message));
        if error.retry || lease.exhausted {
            writes.exceptions.push(steps::exception(
                tenant,
                ExceptionKind::DeadLettered,
                "attempts-exhausted",
                &run.automation.clone(),
                Some(&run),
                Some(step),
                job,
                format!("step {step} spent {} attempts", job.attempt),
                now,
            ));
        }
        writes.runs.push(run);
        writes.steps.push(record);
    }
    Outcome { report, writes }
}
