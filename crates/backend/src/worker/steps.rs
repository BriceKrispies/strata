//! What one job does: start a run of an automation, or run one step of a run (D-6520 to
//! D-6539, WG-5098).
//!
//! Every step that changes the tenant is decided **by the kernel**, as the automation principal
//! `automation:<workflow-id>` holding only the automation's run-as roles (D-6525, WFL-1527), with
//! no session, the run as its correlation (WFL-1530) and the step key as its idempotency key
//! (D-6528, SCH-1524). The step key is the same on every attempt, so an attempt that is made
//! again after a crash between doing and reporting finds the first result recorded under it and
//! changes nothing (SCH-015).
//!
//! The expressions of a step are evaluated against the run's parameters at the time its job was
//! created, which is the same on every attempt: a retried step asks the kernel for exactly the
//! same intent, so the idempotency record answers it.
//!
//! Built: create, update and delete a record, post to the ledger (decided by the kernel's ledger
//! module through the intent post-ledger, keyed by the step key), branch on a condition (on a record read through the
//! kernel as the automation principal), wait for a duration or until a time (the next step's job
//! is created due then: nothing sleeps, D-6535), invoke a tenant operation ([`super::invoke`]), stop
//! at an approval ([`super::approvals`]), and end. A step that fails for good starts the compensation
//! of the completed steps ([`super::saga`]). Every other kind of the catalogue fails
//! its step with the error "step kind ... is not built", recorded on the step and the run; it is
//! never skipped.

use crate::app::App;
use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::automation::automation_principal;
use strata_kernel::expr::{self, Env};
use strata_kernel::{
    canon, AutomationDef, AutomationStep, AutomationStepDef, Context, Expr, FieldId, Intent,
    ObjectTypeId, Output, Principal, ReadRequest, RecordId, RoleId, StepName, TenantId,
    TenantSchema, Timestamp, Value, WorkflowId,
};
use strata_model::types::AutomationStepResult;
use strata_storage_api::jobs::{
    self, ExceptionKind, ExceptionRecord, Job, JobReport, JobWork, Lease, NewJob, RetryPolicy,
    RunRecord, RunState, StepRecord, StepState, WorkerWrites,
};

/// What a job came to: the report for its attempt and what goes with it in the same
/// transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub report: JobReport,
    pub writes: WorkerWrites,
}

impl Outcome {
    fn done(writes: WorkerWrites) -> Self {
        Outcome {
            report: JobReport::Succeeded,
            writes,
        }
    }
}

/// The step key of D-6528 (SCH-1524): the SHA-256 of the tenant, the automation, the run, the
/// step and the member of a fan-out (none here), the same on every attempt.
pub fn step_key(tenant: &TenantId, automation: &WorkflowId, run: &str, step: &StepName) -> String {
    canon::hash_bytes(
        format!("strata/step-key/v1\0{tenant}\0{automation}\0{run}\0{step}\0").as_bytes(),
    )
}

/// The key of the job of the step at `sequence` of a run.
pub(super) fn step_job_key(run: &str, sequence: u64) -> String {
    format!("step:{run}:{sequence}")
}

/// Milliseconds as the store counts them.
pub(super) fn ms(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The retry policy and attempt timeout of a step's job (D-6529), with the defaults the
/// definition leaves out.
pub fn step_policy(step: &AutomationStepDef) -> RetryPolicy {
    let retry = step.retry.as_ref();
    RetryPolicy {
        attempts: retry.map_or(jobs::DEFAULT_ATTEMPTS, |r| r.attempts.max(1)),
        first_delay_ms: retry
            .and_then(|r| r.first_delay)
            .map_or(jobs::DEFAULT_FIRST_DELAY_MS, |d| ms(d.0)),
        max_delay_ms: retry
            .and_then(|r| r.max_delay)
            .map_or(jobs::DEFAULT_MAX_DELAY_MS, |d| ms(d.0)),
        lease_ms: step.timeout.map_or(jobs::DEFAULT_LEASE_MS, |d| ms(d.0)),
    }
}

/// The job of the step `step` at position `sequence` of a run, due at `due`.
pub(super) fn step_job(
    run: &str,
    automation: &WorkflowId,
    step: &StepName,
    def: &AutomationStepDef,
    sequence: u64,
    due: Timestamp,
) -> NewJob {
    NewJob {
        key: step_job_key(run, sequence),
        work: JobWork::Step {
            run: run.to_string(),
            automation: automation.clone(),
            step: step.clone(),
            sequence,
        },
        due_at: due,
        policy: step_policy(def),
    }
}

/// The automation of a schema.
pub(super) fn automation_of<'a>(
    schema: &'a TenantSchema,
    id: &WorkflowId,
) -> Option<&'a AutomationDef> {
    schema.automations.as_ref().and_then(|all| all.get(id))
}

/// An exception record of the run of a job, with an id derived from the job and the reason.
#[allow(clippy::too_many_arguments)]
pub(super) fn exception(
    tenant: &TenantId,
    kind: ExceptionKind,
    reason: &str,
    automation: &WorkflowId,
    run: Option<&RunRecord>,
    step: Option<&StepName>,
    job: &Job,
    detail: String,
    now: Timestamp,
) -> ExceptionRecord {
    let mut detail = detail;
    let cut = (0..=detail.len().min(256))
        .rev()
        .find(|i| detail.is_char_boundary(*i))
        .unwrap_or(0);
    detail.truncate(cut);
    ExceptionRecord {
        id: jobs::derived_uuid(&format!(
            "strata/automation-exception/v1\0{tenant}\0{}\0{reason}\0{}",
            job.id, job.attempt
        )),
        kind,
        reason: reason.to_string(),
        trigger: run.and_then(|r| r.trigger.clone()),
        automation: Some(automation.clone()),
        run: run.map(|r| r.run.clone()),
        step: step.cloned(),
        event_seq: run.and_then(|r| r.event_seq),
        job: Some(job.id.clone()),
        hop: run.map(|r| r.hop),
        at: now,
        detail,
        settled: !kind.settled_by_owner(),
        seq: 0,
    }
}

/// Run the job a lease holds, as of `now`, and say how to report it.
pub fn run_job(app: &App, tenant: &TenantId, lease: &Lease, now: Timestamp) -> Outcome {
    match &lease.job.work {
        JobWork::StartRun {
            trigger,
            automation,
            event_seq,
            hop,
            params,
            caused_by,
            on_behalf_of,
            ..
        } => start_run(
            app,
            tenant,
            lease,
            StartOf {
                trigger: trigger.clone(),
                automation,
                event_seq: *event_seq,
                hop: *hop,
                params,
                caused_by: caused_by.clone(),
                on_behalf_of: on_behalf_of.clone(),
            },
            now,
        ),
        JobWork::Step {
            run,
            automation,
            step,
            sequence,
        } => run_step(app, tenant, lease, run, automation, step, *sequence, now),
        JobWork::ApprovalTimeout {
            run,
            automation,
            step,
            sequence,
        } => super::approvals::timeout(app, tenant, lease, run, automation, step, *sequence, now),
        JobWork::Compensate {
            run,
            automation,
            step,
            of_sequence,
        } => super::saga::compensate(app, tenant, lease, run, automation, step, *of_sequence, now),
    }
}

/// What starts a run.
struct StartOf<'a> {
    trigger: Option<strata_kernel::TriggerId>,
    automation: &'a WorkflowId,
    event_seq: Option<u64>,
    hop: u64,
    params: &'a BTreeMap<String, Value>,
    /// Who caused the event (the trigger's `caused-by`); the automation principal when the job
    /// does not say.
    caused_by: Option<strata_kernel::PrincipalId>,
    /// The person the automation acts for: its definer (D-7204).
    on_behalf_of: Option<strata_kernel::PrincipalId>,
}

/// A failure of a job that is not a step of a run: retried while its policy allows, then failed
/// with a dead-lettered record.
pub(super) fn job_failure(
    tenant: &TenantId,
    lease: &Lease,
    automation: &WorkflowId,
    error: String,
    now: Timestamp,
) -> Outcome {
    let report = JobReport::Failed {
        error: error.clone(),
        retry: !lease.exhausted,
    };
    let mut writes = WorkerWrites::default();
    if lease.exhausted || jobs::report_fails_job(&lease.job, &report) {
        writes.exceptions.push(exception(
            tenant,
            ExceptionKind::DeadLettered,
            "attempts-exhausted",
            automation,
            None,
            None,
            &lease.job,
            error,
            now,
        ));
    }
    Outcome { report, writes }
}

/// Start a run (start-workflow-run, WFL-1521): its record and the job of its start step. The
/// run's id is the job's, so a repeated start finds the run there and starts nothing.
fn start_run(
    app: &App,
    tenant: &TenantId,
    lease: &Lease,
    start: StartOf<'_>,
    now: Timestamp,
) -> Outcome {
    let run_id = lease.job.id.clone();
    if lease.exhausted {
        let error = lease.job.errors.last().cloned().unwrap_or_default();
        return job_failure(tenant, lease, start.automation, error, now);
    }
    match app.store.automation_run(tenant, &run_id) {
        Ok(Some(_)) => return Outcome::done(WorkerWrites::default()),
        Ok(None) => {}
        Err(error) => return job_failure(tenant, lease, start.automation, error.to_string(), now),
    }
    let schema = match app.store.load(tenant, &[]) {
        Ok(state) => match state.tenant {
            Some(found) => found.schema.clone(),
            None => {
                return job_failure(
                    tenant,
                    lease,
                    start.automation,
                    format!("no tenant {tenant}"),
                    now,
                )
            }
        },
        Err(error) => return job_failure(tenant, lease, start.automation, error.to_string(), now),
    };
    let Some(def) = automation_of(&schema, start.automation) else {
        // The automation was removed after its trigger fired: no run starts (D-6539).
        return Outcome::done(WorkerWrites {
            exceptions: vec![exception(
                tenant,
                ExceptionKind::Declined,
                "automation-removed",
                start.automation,
                None,
                None,
                &lease.job,
                format!("automation {} does not exist", start.automation),
                now,
            )],
            ..WorkerWrites::default()
        });
    };
    if let Some(role) = &def.suspended_for {
        // Suspended after its trigger fired: the definer lost a run-as role (WFL-1529), and no
        // run starts until the automation is defined again.
        return Outcome::done(WorkerWrites {
            exceptions: vec![exception(
                tenant,
                ExceptionKind::Declined,
                "run-as-suspended",
                start.automation,
                None,
                None,
                &lease.job,
                format!(
                    "automation {} is suspended: its definer may not confer role {role}",
                    start.automation
                ),
                now,
            )],
            ..WorkerWrites::default()
        });
    }
    if let Some(outcome) = super::commands::inactive_outcome(tenant, lease, def, now) {
        return outcome;
    }
    let Some(first) = def.steps.get(&def.start) else {
        return job_failure(
            tenant,
            lease,
            start.automation,
            format!(
                "automation {} has no start step {}",
                start.automation, def.start
            ),
            now,
        );
    };
    let mut params = start.params.clone();
    let started_by = super::commands::take_starter(app, tenant, start.event_seq, &mut params);
    let run = RunRecord {
        run: run_id.clone(),
        automation: start.automation.clone(),
        trigger: start.trigger,
        event_seq: start.event_seq,
        state: RunState::Running,
        step: Some(def.start.clone()),
        steps: 0,
        hop: start.hop,
        caused_by: start
            .caused_by
            .unwrap_or_else(|| automation_principal(start.automation)),
        on_behalf_of: start.on_behalf_of,
        started_by,
        params,
        started_at: now,
        ended_at: None,
        error: None,
        seq: 0,
    };
    Outcome::done(WorkerWrites {
        runs: vec![run],
        jobs: vec![step_job(
            &run_id,
            start.automation,
            &def.start,
            first,
            1,
            now,
        )],
        ..WorkerWrites::default()
    })
}

/// Why a step did not complete, and whether an attempt made again may.
pub(super) struct StepError {
    pub(super) message: String,
    pub(super) retry: bool,
}

impl StepError {
    pub(super) fn retry(message: impl Into<String>) -> Self {
        StepError {
            message: message.into(),
            retry: true,
        }
    }

    pub(super) fn fatal(message: impl Into<String>) -> Self {
        StepError {
            message: message.into(),
            retry: false,
        }
    }
}

/// Where a run goes after a step.
pub(super) enum Next {
    Step(StepName, Timestamp),
    End,
}

/// A completed step: its typed result (WFL-1523), the outputs it binds, and where the run goes.
pub(super) struct Done {
    pub(super) result: AutomationStepResult,
    pub(super) outputs: Vec<(&'static str, Value)>,
    pub(super) next: Next,
}

/// Run one step of a run.
#[allow(clippy::too_many_arguments)]
fn run_step(
    app: &App,
    tenant: &TenantId,
    lease: &Lease,
    run_id: &str,
    automation: &WorkflowId,
    step: &StepName,
    sequence: u64,
    now: Timestamp,
) -> Outcome {
    let job = &lease.job;
    let mut run = match app.store.automation_run(tenant, run_id) {
        Ok(Some(run)) => run,
        Ok(None) => return job_failure(tenant, lease, automation, format!("no run {run_id}"), now),
        Err(error) => return job_failure(tenant, lease, automation, error.to_string(), now),
    };
    if run.state.ended() {
        // The run ended (cancelled, failed) after this job was created: nothing to do.
        return Outcome::done(WorkerWrites::default());
    }
    let key = step_key(tenant, automation, run_id, step);
    let mut record = StepRecord {
        run: run_id.to_string(),
        sequence,
        step: step.clone(),
        state: StepState::Leased,
        attempts: job.attempt,
        job: Some(job.id.clone()),
        idempotency_key: key.clone(),
        result: None,
        errors: job.errors.clone(),
        approval: None,
        started_at: job.created_at,
        ended_at: None,
    };
    let schema = match app.store.load(tenant, &[]) {
        Ok(state) => state.tenant.map(|found| found.schema.clone()),
        Err(error) => return job_failure(tenant, lease, automation, error.to_string(), now),
    };
    let def = schema
        .as_deref()
        .and_then(|schema| automation_of(schema, automation));
    let Some(def) = def else {
        // Removing an automation cancels its open runs (D-6539).
        record.state = StepState::Cancelled;
        record.ended_at = Some(now);
        run.state = RunState::Cancelled;
        run.ended_at = Some(now);
        run.error = Some(format!("automation {automation} was removed"));
        let removed = exception(
            tenant,
            ExceptionKind::Declined,
            "automation-removed",
            automation,
            Some(&run),
            Some(step),
            job,
            format!("automation {automation} was removed"),
            now,
        );
        return Outcome::done(WorkerWrites {
            runs: vec![run],
            steps: vec![record],
            exceptions: vec![removed],
            jobs: Vec::new(),
        });
    };
    if let Some(role) = &def.suspended_for {
        // Suspended while the run was open (WFL-1529): the step is not run, and the run ends
        // with the reason recorded; defining the automation again starts new runs, not this one.
        let why =
            format!("automation {automation} is suspended: its definer may not confer role {role}");
        record.state = StepState::Cancelled;
        record.ended_at = Some(now);
        run.state = RunState::Cancelled;
        run.ended_at = Some(now);
        run.error = Some(why.clone());
        let suspended = exception(
            tenant,
            ExceptionKind::Declined,
            "run-as-suspended",
            automation,
            Some(&run),
            Some(step),
            job,
            why,
            now,
        );
        return Outcome::done(WorkerWrites {
            runs: vec![run],
            steps: vec![record],
            exceptions: vec![suspended],
            jobs: Vec::new(),
        });
    }
    if let Some(outcome) = super::approvals::begin(
        app,
        tenant,
        lease,
        schema.as_deref(),
        def,
        step,
        &run,
        &record,
        now,
    ) {
        return outcome;
    }
    let attempt = match def.steps.get(step) {
        _ if lease.exhausted => Err(StepError::fatal(
            job.errors
                .last()
                .cloned()
                .unwrap_or_else(|| "the lease expired".to_string()),
        )),
        None => Err(StepError::fatal(format!(
            "automation {automation} has no step {step}: its definition changed"
        ))),
        Some(step_def) => act(app, tenant, def, step_def, &run, &key, job, now),
    };
    match attempt {
        Ok(done) => {
            record.state = StepState::Completed;
            record.ended_at = Some(now);
            record.result = serde_json::to_value(&done.result).ok();
            for (output, value) in done.outputs {
                run.params.insert(format!("{step}_{output}"), value);
            }
            run.steps = sequence;
            let mut follow = Vec::new();
            match done.next {
                Next::Step(name, due) => {
                    match def.steps.get(&name) {
                        Some(next_def) => {
                            follow.push(step_job(
                                run_id,
                                automation,
                                &name,
                                next_def,
                                sequence + 1,
                                due,
                            ));
                            run.state = if due > now {
                                RunState::Waiting
                            } else {
                                RunState::Running
                            };
                        }
                        None => {
                            run.state = RunState::Failed;
                            run.ended_at = Some(now);
                            run.error = Some(format!("no step {name}"));
                        }
                    }
                    run.step = Some(name);
                }
                Next::End => {
                    run.state = RunState::Completed;
                    run.ended_at = Some(now);
                }
            }
            Outcome::done(WorkerWrites {
                runs: vec![run],
                steps: vec![record],
                exceptions: Vec::new(),
                jobs: follow,
            })
        }
        Err(error) => {
            let report = JobReport::Failed {
                error: error.message.clone(),
                retry: error.retry,
            };
            let fails = lease.exhausted || jobs::report_fails_job(job, &report);
            record
                .errors
                .push(format!("attempt {}: {}", job.attempt, error.message));
            let mut exceptions = Vec::new();
            let mut compensation = Vec::new();
            if fails {
                record.state = StepState::Failed;
                record.ended_at = Some(now);
                run.state = RunState::Failed;
                run.ended_at = Some(now);
                run.error = Some(jobs::clip_error(&error.message));
                // A step that spent its attempts is dead-lettered (SCH-1525); a step that cannot
                // succeed (a kind that is not built) fails its run, recorded there.
                if error.retry || lease.exhausted {
                    exceptions.push(exception(
                        tenant,
                        ExceptionKind::DeadLettered,
                        "attempts-exhausted",
                        automation,
                        Some(&run),
                        Some(step),
                        job,
                        format!("step {step} spent {} attempts", job.attempt),
                        now,
                    ));
                }
                compensation = super::saga::begin(app, tenant, def, &mut run, sequence, now);
            } else {
                record.state = StepState::Due;
            }
            Outcome {
                report,
                writes: WorkerWrites {
                    runs: vec![run],
                    steps: vec![record],
                    exceptions,
                    jobs: compensation,
                },
            }
        }
    }
}

/// The context a step is decided in: the automation principal, no session, the run as the
/// correlation, the step key when the step changes the tenant, and one hop more than the event
/// that started the run, so that the triggers its events match count the chain (D-6531, D-7202).
pub(super) fn step_context(
    tenant: &TenantId,
    automation: &WorkflowId,
    run: &RunRecord,
    key: Option<&str>,
    now: Timestamp,
) -> Context {
    Context {
        tenant: tenant.clone(),
        principal: Principal::User(automation_principal(automation)),
        session: None,
        now,
        correlation: format!("run:{}", run.run),
        verified_address: None,
        idempotency_key: key.map(str::to_string),
        language: None,
        hop: Some(u32::try_from(run.hop.saturating_add(1)).unwrap_or(u32::MAX)),
    }
}

/// The record an expression names: a reference, or a positive whole number.
fn record_id(value: Value) -> Result<RecordId, StepError> {
    match value {
        Value::Ref(id) => Ok(id),
        Value::Int(n) if n > 0 => Ok(RecordId(n as u64)),
        other => Err(StepError::retry(format!(
            "{other:?} does not name a record"
        ))),
    }
}

/// The name of a step kind, as the catalogue writes it.
fn kind_name(action: &AutomationStep) -> &'static str {
    match action {
        AutomationStep::CreateRecord { .. } => "create-record",
        AutomationStep::UpdateRecord { .. } => "update-record",
        AutomationStep::DeleteRecord { .. } => "delete-record",
        AutomationStep::PostLedger { .. } => "post-ledger",
        AutomationStep::SendMail { .. } => "send-mail",
        AutomationStep::CreateNotice { .. } => "create-notice",
        AutomationStep::CallWebhook { .. } => "call-webhook",
        AutomationStep::WaitFor { .. } => "wait-for",
        AutomationStep::WaitUntil { .. } => "wait-until",
        AutomationStep::Branch { .. } => "branch",
        AutomationStep::FanOut { .. } => "fan-out",
        AutomationStep::RequestApproval { .. } => "request-approval",
        AutomationStep::InvokeOperation { .. } => "invoke-operation",
        AutomationStep::End => "end",
    }
}

/// Carry out one step's action.
#[allow(clippy::too_many_arguments)]
pub(super) fn act(
    app: &App,
    tenant: &TenantId,
    def: &AutomationDef,
    step: &AutomationStepDef,
    run: &RunRecord,
    key: &str,
    job: &Job,
    now: Timestamp,
) -> Result<Done, StepError> {
    let principal = Principal::User(automation_principal(&def.id));
    let roles: BTreeSet<RoleId> = def.run_as.roles.clone();
    let env = Env {
        record: None,
        old: None,
        new: None,
        params: &run.params,
        // The time the job was created: the same on every attempt, so a retry decides the same
        // intent and its step key returns the first result.
        now: job.created_at,
        principal: &principal,
        roles: &roles,
        lookup: None,
    };
    let eval = |what: &str, e: &Expr| {
        expr::eval(e, &env).map_err(|error| StepError::retry(format!("{what}: {error}")))
    };
    let fields_of =
        |fields: &BTreeMap<FieldId, Expr>| -> Result<BTreeMap<FieldId, Value>, StepError> {
            fields
                .iter()
                .map(|(field, e)| -> Result<(FieldId, Value), StepError> {
                    Ok((field.clone(), eval(field.as_str(), e)?))
                })
                .collect()
        };
    let then = || match &step.next {
        Some(next) => Next::Step(next.clone(), now),
        None => Next::End,
    };
    let decide = |intent: Intent| -> Result<Output, StepError> {
        let context = step_context(tenant, &def.id, run, Some(key), app.now());
        crate::ops::run_intent(app, &context, &intent, false)
            .map(|done| done.transition.output)
            .map_err(|error| StepError::retry(error.describe()))
    };
    match &step.action {
        AutomationStep::CreateRecord { object, fields } => {
            let intent = Intent::CreateRecord {
                object: object.clone(),
                fields: fields_of(fields)?,
            };
            match decide(intent)? {
                Output::Record { id, version, .. } => Ok(Done {
                    result: AutomationStepResult::CreateRecord {
                        record: id,
                        version,
                    },
                    outputs: vec![
                        ("record", Value::Ref(id)),
                        ("version", Value::Int(version as i64)),
                    ],
                    next: then(),
                }),
                other => Err(StepError::fatal(format!("unexpected output {other:?}"))),
            }
        }
        AutomationStep::UpdateRecord {
            object,
            record,
            fields,
        } => {
            let intent = Intent::UpdateRecord {
                object: object.clone(),
                id: record_id(eval("record", record)?)?,
                fields: fields_of(fields)?,
                expected_version: None,
            };
            match decide(intent)? {
                Output::Record { version, .. } => Ok(Done {
                    result: AutomationStepResult::UpdateRecord { version },
                    outputs: vec![("version", Value::Int(version as i64))],
                    next: then(),
                }),
                other => Err(StepError::fatal(format!("unexpected output {other:?}"))),
            }
        }
        AutomationStep::DeleteRecord { object, record } => {
            let intent = Intent::DeleteRecord {
                object: object.clone(),
                id: record_id(eval("record", record)?)?,
                expected_version: None,
            };
            decide(intent)?;
            Ok(Done {
                result: AutomationStepResult::DeleteRecord,
                outputs: Vec::new(),
                next: then(),
            })
        }
        AutomationStep::Branch {
            condition,
            object,
            record,
            then: yes,
            r#else: no,
        } => {
            let holds = match (object, record) {
                (Some(object), Some(record)) => {
                    let id = record_id(eval("record", record)?)?;
                    let row = read_record(app, tenant, def, run, object, id)?;
                    let with_record = Env {
                        record: Some(&row),
                        ..env_copy(&env)
                    };
                    expr::holds(condition, &with_record)
                }
                _ => expr::holds(condition, &env),
            }
            .map_err(|error| StepError::retry(format!("condition: {error}")))?;
            let taken = if holds { yes.clone() } else { no.clone() };
            Ok(Done {
                result: AutomationStepResult::Branch {
                    taken: taken.clone(),
                },
                outputs: vec![("taken", Value::Str(taken.as_str().to_string()))],
                next: Next::Step(taken, now),
            })
        }
        AutomationStep::WaitFor { duration } => {
            let due = Timestamp(now.0.saturating_add(ms(duration.0)));
            Ok(Done {
                result: AutomationStepResult::WaitFor,
                outputs: Vec::new(),
                next: match &step.next {
                    Some(next) => Next::Step(next.clone(), due),
                    None => Next::End,
                },
            })
        }
        AutomationStep::WaitUntil { at } => {
            let due = match eval("at", at)? {
                Value::Time(when) | Value::Int(when) => Timestamp(when.max(now.0)),
                other => {
                    return Err(StepError::retry(format!("at: {other:?} is not a time")));
                }
            };
            Ok(Done {
                result: AutomationStepResult::WaitUntil,
                outputs: Vec::new(),
                next: match &step.next {
                    Some(next) => Next::Step(next.clone(), due),
                    None => Next::End,
                },
            })
        }
        AutomationStep::PostLedger {
            owner,
            unit,
            amount,
            reason,
        } => {
            // A posting is decided by the kernel's ledger module as the automation principal,
            // which needs the capability ledger-post through a run-as role; the step key is the
            // posting's idempotency key, so a retried attempt answers the first entry (LDG-006).
            let owner = match eval("owner", owner)? {
                Value::Str(text) => strata_kernel::PrincipalId::new(text.as_str())
                    .map(|principal| strata_kernel::LedgerOwner::Principal { principal })
                    .map_err(|_| {
                        StepError::fatal(format!("owner: {text} is not a principal id"))
                    })?,
                other => {
                    return Err(StepError::fatal(format!(
                        "owner: {other:?} is not a principal id"
                    )))
                }
            };
            let amount = match eval("amount", amount)? {
                Value::Int(n) => strata_kernel::LedgerAmount(n),
                other => {
                    return Err(StepError::fatal(format!(
                        "amount: {other:?} is not a whole number"
                    )))
                }
            };
            let intent = Intent::PostLedger {
                postings: vec![strata_kernel::LedgerPostingRequest {
                    owner,
                    unit: unit.clone(),
                    amount,
                    idempotency_key: key.to_string(),
                    reason: reason.clone(),
                    corrects: None,
                    scope: None,
                }],
            };
            match decide(intent)? {
                Output::LedgerPosted { results } => match results.first() {
                    Some(posted) => Ok(Done {
                        result: AutomationStepResult::PostLedger {
                            entry_sequence: posted.entry.sequence,
                            balance: posted.balance.balance,
                        },
                        outputs: vec![
                            ("entry_sequence", Value::Int(posted.entry.sequence as i64)),
                            ("balance", Value::Int(posted.balance.balance.0)),
                        ],
                        next: then(),
                    }),
                    None => Err(StepError::fatal("the posting answered no entry")),
                },
                other => Err(StepError::fatal(format!("unexpected output {other:?}"))),
            }
        }
        AutomationStep::InvokeOperation { operation, inputs } => {
            super::invoke::run(operation, inputs, key, &eval, &decide, &then)
        }
        AutomationStep::End => Ok(Done {
            result: AutomationStepResult::End,
            outputs: Vec::new(),
            next: Next::End,
        }),
        other => Err(StepError::fatal(format!(
            "step kind {} is not built",
            kind_name(other)
        ))),
    }
}

/// An [`Env`] with the same references, so that one field can be changed.
fn env_copy<'a>(env: &Env<'a>) -> Env<'a> {
    Env {
        record: env.record,
        old: env.old,
        new: env.new,
        params: env.params,
        now: env.now,
        principal: env.principal,
        roles: env.roles,
        lookup: env.lookup,
    }
}

/// A record a branch looks at, read through the kernel as the automation principal: a record it
/// may not read is not there for it.
fn read_record(
    app: &App,
    tenant: &TenantId,
    def: &AutomationDef,
    run: &RunRecord,
    object: &ObjectTypeId,
    id: RecordId,
) -> Result<strata_kernel::Record, StepError> {
    let context = step_context(tenant, &def.id, run, None, app.now());
    let found = strata_storage_api::query(
        app.store.as_ref(),
        &ReadRequest::Records {
            object: object.clone(),
            ids: vec![id],
        },
        &context,
    )
    .map_err(|error| StepError::retry(error.to_string()))?;
    found
        .rows
        .first()
        .map(|row| strata_kernel::Record::clone(row))
        .ok_or_else(|| {
            StepError::retry(format!(
                "{object} {id} does not exist or the automation may not read it"
            ))
        })
}
