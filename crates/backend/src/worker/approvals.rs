//! The step kind request-approval (WG-5153, WFL-006, WFL-2680 to WFL-2683, D-7680): a run that
//! reaches it stops, with no lease, until a principal the policy of the step names decides, or its
//! deadline passes.
//!
//! - **Reaching the step** ([`begin`]) records the approval in the step record (who may approve,
//!   the deadline, who started the run), puts the run in `awaiting-approval` and creates one job,
//!   keyed by run and step and due at the deadline ([`timeout`]).
//! - **A decision** is made through the kernel (`strata_kernel::approvals::decide`) by
//!   [`super::commands::approve_step`]; this module resolves it ([`resolve`]): the step completes
//!   with the typed result `request-approval`, its outputs are bound as `param.<step>_approved`
//!   and `param.<step>_by`, and the run goes on to the approved or the refused step through the
//!   job of that step, due now. The decision is kept in the step record, and a step that holds
//!   one takes no second (WFL-2683).
//! - **The deadline** is a job like any other: when it is due and the step still waits, the run
//!   takes the refused step, with the decision recorded as timed out (WFL-2682). A job that finds
//!   the step decided, or the run ended, does nothing.

use super::steps::{exception, job_failure, ms, step_job, Outcome};
use crate::app::App;
use strata_kernel::approvals::policy_roles;
use strata_kernel::{
    AutomationDef, AutomationStep, StepName, TenantId, TenantSchema, Timestamp, Value, WorkflowId,
};
use strata_model::types::AutomationStepResult;
use strata_storage_api::jobs::{
    ApprovalDecision, ApprovalRecord, ExceptionKind, JobReport, JobWork, Lease, NewJob,
    RetryPolicy, RunRecord, RunState, StepRecord, StepState, WorkerWrites,
};
use strata_storage_api::StoreResult;

/// The key of the job that takes the deadline of the approval at `sequence` of a run.
pub(super) fn timeout_key(run: &str, sequence: u64) -> String {
    format!("approval-timeout:{run}:{sequence}")
}

/// A job that succeeded with `writes`.
fn succeeded(writes: WorkerWrites) -> Outcome {
    Outcome {
        report: JobReport::Succeeded,
        writes,
    }
}

/// The step at `sequence` of a run, when the store holds it.
pub(super) fn step_at(
    app: &App,
    tenant: &TenantId,
    run: &str,
    sequence: u64,
) -> StoreResult<Option<StepRecord>> {
    Ok(app
        .store
        .automation_steps(tenant, run, Some(sequence.saturating_sub(1)), 1)?
        .into_iter()
        .find(|step| step.sequence == sequence))
}

/// Reaching a request-approval step: record the approval and stop the run. `None` when the step
/// is not an approval step (or its lease ran out), which leaves it to the common path.
#[allow(clippy::too_many_arguments)]
pub(super) fn begin(
    app: &App,
    tenant: &TenantId,
    lease: &Lease,
    schema: Option<&TenantSchema>,
    def: &AutomationDef,
    step: &StepName,
    run: &RunRecord,
    record: &StepRecord,
    now: Timestamp,
) -> Option<Outcome> {
    let AutomationStep::RequestApproval {
        policy, timeout, ..
    } = &def.steps.get(step)?.action
    else {
        return None;
    };
    if lease.exhausted {
        return None;
    }
    // An attempt made again after a crash between recording the wait and reporting it finds the
    // step waiting, or decided, already.
    match step_at(app, tenant, &run.run, record.sequence) {
        Ok(Some(held)) if held.approval.is_some() => {
            return Some(succeeded(WorkerWrites::default()))
        }
        Ok(_) => {}
        Err(error) => return Some(job_failure(tenant, lease, &def.id, error.to_string(), now)),
    }
    let roles = schema
        .and_then(|schema| policy_roles(schema, policy))
        .unwrap_or_default();
    let deadline = Timestamp(now.0.saturating_add(ms(timeout.0)));
    let mut waiting = record.clone();
    waiting.state = StepState::Waiting;
    waiting.approval = Some(ApprovalRecord {
        policy: policy.to_string(),
        roles: roles.iter().map(|role| role.as_str().to_string()).collect(),
        requested_at: now,
        deadline,
        started_by: run.started_by.clone(),
        decision: None,
    });
    let mut run = run.clone();
    run.state = RunState::AwaitingApproval;
    run.step = Some(step.clone());
    run.steps = record.sequence;
    let deadline_job = NewJob {
        key: timeout_key(&run.run, record.sequence),
        work: JobWork::ApprovalTimeout {
            run: run.run.clone(),
            automation: def.id.clone(),
            step: step.clone(),
            sequence: record.sequence,
        },
        due_at: deadline,
        policy: RetryPolicy::default(),
    };
    Some(succeeded(WorkerWrites {
        runs: vec![run],
        steps: vec![waiting],
        exceptions: Vec::new(),
        jobs: vec![deadline_job],
    }))
}

/// Resolve the approval of a waiting step with `decision`: the writes that complete the step and
/// send the run down its approved or refused branch. Fails with the reason when the definition no
/// longer has the step or the step it names.
pub(super) fn resolve(
    def: &AutomationDef,
    mut run: RunRecord,
    mut record: StepRecord,
    decision: ApprovalDecision,
    now: Timestamp,
) -> Result<WorkerWrites, String> {
    let AutomationStep::RequestApproval {
        approved, refused, ..
    } = &def
        .steps
        .get(&record.step)
        .ok_or_else(|| format!("automation {} has no step {}", def.id, record.step))?
        .action
    else {
        return Err(format!("step {} is not an approval", record.step));
    };
    let next = if decision.approved { approved } else { refused };
    let next_def = def
        .steps
        .get(next)
        .ok_or_else(|| format!("automation {} has no step {next}", def.id))?;
    let step_name = record.step.clone();
    run.params.insert(
        format!("{step_name}_approved"),
        Value::Bool(decision.approved),
    );
    run.params.insert(
        format!("{step_name}_by"),
        match &decision.by {
            Some(by) => Value::Str(by.as_str().to_string()),
            None => Value::Null,
        },
    );
    record.result = serde_json::to_value(AutomationStepResult::RequestApproval {
        approved: decision.approved,
        by: Some(decision.by.clone()),
    })
    .ok();
    if let Some(approval) = record.approval.as_mut() {
        approval.decision = Some(decision);
    }
    record.state = StepState::Completed;
    record.ended_at = Some(now);
    run.state = RunState::Running;
    run.step = Some(next.clone());
    run.steps = record.sequence;
    let job = step_job(&run.run, &def.id, next, next_def, record.sequence + 1, now);
    Ok(WorkerWrites {
        runs: vec![run],
        steps: vec![record],
        exceptions: Vec::new(),
        jobs: vec![job],
    })
}

/// The deadline of an approval, as a job: the step that still waits takes its refused branch.
#[allow(clippy::too_many_arguments)]
pub(super) fn timeout(
    app: &App,
    tenant: &TenantId,
    lease: &Lease,
    run_id: &str,
    automation: &WorkflowId,
    step: &StepName,
    sequence: u64,
    now: Timestamp,
) -> Outcome {
    let run = match app.store.automation_run(tenant, run_id) {
        Ok(Some(run)) => run,
        Ok(None) => return succeeded(WorkerWrites::default()),
        Err(error) => return job_failure(tenant, lease, automation, error.to_string(), now),
    };
    if run.state != RunState::AwaitingApproval {
        return succeeded(WorkerWrites::default());
    }
    let record = match step_at(app, tenant, run_id, sequence) {
        Ok(Some(record))
            if record.state == StepState::Waiting
                && record
                    .approval
                    .as_ref()
                    .is_some_and(|approval| approval.decision.is_none()) =>
        {
            record
        }
        Ok(_) => return succeeded(WorkerWrites::default()),
        Err(error) => return job_failure(tenant, lease, automation, error.to_string(), now),
    };
    let schema = match app.store.load(tenant, &[]) {
        Ok(state) => state.tenant.map(|found| found.schema.clone()),
        Err(error) => return job_failure(tenant, lease, automation, error.to_string(), now),
    };
    let Some(def) = schema
        .as_deref()
        .and_then(|schema| super::steps::automation_of(schema, automation))
    else {
        // Removing an automation cancels its open runs (D-6539).
        let mut gone = run;
        let mut waiting = record;
        waiting.state = StepState::Cancelled;
        waiting.ended_at = Some(now);
        gone.state = RunState::Cancelled;
        gone.ended_at = Some(now);
        gone.error = Some(format!("automation {automation} was removed"));
        let removed = exception(
            tenant,
            ExceptionKind::Declined,
            "automation-removed",
            automation,
            Some(&gone),
            Some(step),
            &lease.job,
            format!("automation {automation} was removed"),
            now,
        );
        return succeeded(WorkerWrites {
            runs: vec![gone],
            steps: vec![waiting],
            exceptions: vec![removed],
            jobs: Vec::new(),
        });
    };
    let decision = ApprovalDecision {
        approved: false,
        by: None,
        comment: None,
        at: now,
        timed_out: true,
    };
    match resolve(def, run.clone(), record, decision, now) {
        Ok(writes) => succeeded(writes),
        Err(why) => {
            let mut failed = run;
            failed.state = RunState::Failed;
            failed.ended_at = Some(now);
            failed.error = Some(strata_storage_api::jobs::clip_error(&why));
            let record = exception(
                tenant,
                ExceptionKind::Declined,
                "definition-changed",
                automation,
                Some(&failed),
                Some(step),
                &lease.job,
                why,
                now,
            );
            succeeded(WorkerWrites {
                runs: vec![failed],
                exceptions: vec![record],
                ..WorkerWrites::default()
            })
        }
    }
}
