//! What a person does to automation runs: start one directly (start-workflow-run), decide an
//! approval a run waits for (approve-workflow-step) and activate an automation
//! (activate-workflow). Each answers for the caller's own tenant only, and each decision is the
//! kernel's: who may approve is `strata_kernel::approvals::decide`, whether an automation may
//! start runs is `strata_kernel::activation`, and activation is the kernel's `define-automation`
//! made again with the state active, so that the checks of defining apply to it (WFL-1528).
//!
//! Every function has an `_at` form that takes the time, so that a test drives the clock of the
//! worker and of the command the same way.

use super::approvals::{resolve, step_at, timeout_key};
use super::routes::{require_capability, same_tenant};
use super::steps::{exception, Outcome};
use crate::app::{ApiError, App, Shared};
use crate::auth::Identity;
use crate::http::as_caller;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde_json::{json, Value as Json_};
use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::activation;
use strata_kernel::approvals::decide;
use strata_kernel::definitions::any_event_params;
use strata_kernel::rules::roles_of;
use strata_kernel::{
    canon, owner_role, AutomationDef, ExistingData, Intent, Principal, PrincipalId, RoleId,
    SchemaChange, TenantId, Timestamp, Value, WorkflowId,
};
use strata_protocol::api::ErrorCode;
use strata_storage_api::jobs::{
    self, ApprovalDecision, ExceptionKind, JobReport, JobWork, Lease, NewJob, RetryPolicy,
    RunState, StepState, WorkerWrites,
};

/// The capability a person needs to start a run directly (start-workflow-run).
pub const JOBS_RUN: &str = "jobs-run";

/// The capability activating an automation needs, besides being the owner (activate-workflow).
pub const WORKFLOW_ACTIVATE: &str = "workflow-activate";

/// The parameter a direct start carries its starter in. The name is not one an expression can
/// read; the run record keeps the starter in `started-by` and the parameter is not kept.
const STARTER_PARAM: &str = "$started_by";

/// The parameters a person may not bind: the ones the platform binds for a direct start.
const BOUND_BY_THE_PLATFORM: [&str; 3] = ["seq", "at", "hop"];

/// The most inputs a direct start carries.
const MAX_INPUTS: usize = 32;

/// A start job that finds its automation inactive (a draft, or suspended after the job was
/// created) starts no run: a declined record with the reason automation-inactive (WFL-2684).
pub(super) fn inactive_outcome(
    tenant: &TenantId,
    lease: &Lease,
    def: &AutomationDef,
    now: Timestamp,
) -> Option<Outcome> {
    if activation::is_active(def) {
        return None;
    }
    Some(Outcome {
        report: JobReport::Succeeded,
        writes: WorkerWrites {
            exceptions: vec![exception(
                tenant,
                ExceptionKind::Declined,
                "automation-inactive",
                &def.id,
                None,
                None,
                &lease.job,
                activation::inactive_detail(def),
                now,
            )],
            ..WorkerWrites::default()
        },
    })
}

/// The person whose action started a run: the one a direct start names in its parameters (taken
/// out of them), or else the principal of the event a trigger matched. `None` when the run was
/// started by no person.
pub(super) fn take_starter(
    app: &App,
    tenant: &TenantId,
    event_seq: Option<u64>,
    params: &mut BTreeMap<String, Value>,
) -> Option<PrincipalId> {
    if let Some(Value::Str(text)) = params.remove(STARTER_PARAM) {
        return PrincipalId::new(text.as_str()).ok();
    }
    let seq = event_seq?;
    let events = app
        .store
        .events_after(tenant, seq.saturating_sub(1), 1)
        .ok()?;
    match events.into_iter().find(|event| event.seq == seq)?.principal {
        Principal::User(id) => Some(id),
        _ => None,
    }
}

/// A JSON input as a value: null, a truth value, a whole number, text or a list of those.
fn value_of(json: &Json_) -> Result<Value, String> {
    Ok(match json {
        Json_::Null => Value::Null,
        Json_::Bool(truth) => Value::Bool(*truth),
        Json_::Number(number) => Value::Int(
            number
                .as_i64()
                .ok_or_else(|| "a number is a whole number".to_string())?,
        ),
        Json_::String(text) => Value::Str(text.clone()),
        Json_::Array(items) => Value::List(
            items
                .iter()
                .map(value_of)
                .collect::<Result<Vec<_>, String>>()?,
        ),
        Json_::Object(_) => return Err("an object is not a value".to_string()),
    })
}

/// The same work, whatever time each start of it was made at.
fn untimed(work: &JobWork) -> JobWork {
    let mut work = work.clone();
    if let JobWork::StartRun { params, .. } = &mut work {
        params.remove("at");
    }
    work
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Invalid, message)
}

/// The person calling, or the refusal for a caller who is not one.
fn person(identity: &Identity) -> Result<PrincipalId, ApiError> {
    match &identity.principal {
        Principal::User(id) if !identity.anonymous() => Ok(id.clone()),
        _ => Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in to do this.",
        )),
    }
}

/// Start a run of an automation directly, as of `now` (start-workflow-run): the caller holds
/// jobs-run, the automation is active, the inputs are names an automation reads (D-6523) and
/// values, and the start is idempotent: the same key by the same person with the same inputs names
/// the same run and starts nothing more, while the same key with other inputs is refused. The run
/// is the job's: it exists once the worker has run the start job, as for a trigger.
/// Implements: CT-090
pub fn start_run_at(
    app: &App,
    identity: &Identity,
    body: &Json_,
    now: Timestamp,
) -> Result<Json_, ApiError> {
    same_tenant(app, identity, body)?;
    require_capability(app, identity, JOBS_RUN)?;
    let starter = person(identity)?;
    let workflow = WorkflowId::new(crate::service::text(body, "workflow", 64)?)
        .map_err(|_| invalid("workflow is missing or invalid."))?;
    let state = crate::ops::header(app, identity)?;
    let def = state
        .schema
        .automations
        .as_ref()
        .and_then(|all| all.get(&workflow))
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "There is no such automation."))?;
    if !activation::is_active(def) {
        return Err(ApiError::new(
            ErrorCode::Conflict,
            format!("{}.", activation::inactive_detail(def)),
        ));
    }
    let allowed = any_event_params();
    let mut params: BTreeMap<String, Value> = BTreeMap::new();
    match body.get("input") {
        None | Some(Json_::Null) => {}
        Some(Json_::Object(inputs)) => {
            if inputs.len() > MAX_INPUTS {
                return Err(invalid(format!("an input has at most {MAX_INPUTS} names.")));
            }
            for (name, json) in inputs {
                if BOUND_BY_THE_PLATFORM.contains(&name.as_str()) {
                    return Err(invalid(format!("{name} is bound by the platform.")));
                }
                if !allowed.contains(name) {
                    return Err(invalid(format!(
                        "{name} is not a name an automation reads."
                    )));
                }
                let value =
                    value_of(json).map_err(|why| invalid(format!("input {name}: {why}.")))?;
                if let Some(problem) = strata_kernel::limits::value_problem(&value) {
                    return Err(invalid(format!("input {name}: {problem}.")));
                }
                params.insert(name.clone(), value);
            }
        }
        Some(_) => return Err(invalid("input is an object of names and values.")),
    }
    params.insert("at".to_string(), Value::Time(now.0));
    params.insert("hop".to_string(), Value::Int(0));
    params.insert(
        STARTER_PARAM.to_string(),
        Value::Str(starter.as_str().to_string()),
    );
    let given = match body.get("idempotency_key") {
        None | Some(Json_::Null) => None,
        Some(key) => Some(
            key.as_str()
                .filter(|key| strata_kernel::operations::valid_key(key))
                .ok_or_else(|| invalid("idempotency_key is 1 to 128 visible characters."))?
                .to_string(),
        ),
    };
    // Without a key the start is not repeatable: the request's own correlation and time make one.
    let key = given.unwrap_or_else(|| {
        canon::hash_bytes(format!("{}\0{}\0{workflow}", identity.correlation, now.0).as_bytes())
    });
    let job_key = format!(
        "start:direct:{}",
        canon::hash_bytes(
            format!("strata/direct-start/v1\0{starter}\0{workflow}\0{key}").as_bytes()
        )
    );
    let work = JobWork::StartRun {
        trigger: None,
        automation: workflow.clone(),
        event_seq: None,
        hop: 0,
        params,
        caused_by: None,
        on_behalf_of: def.defined_by.clone(),
        definition_version: Some(state.schema.version),
        run_key: None,
    };
    let run = jobs::job_id(&identity.tenant, &job_key);
    if let Some(held) = app.store.job(&identity.tenant, &run)? {
        if untimed(&held.work) == untimed(&work) {
            return Ok(json!({ "run": run, "replayed": true }));
        }
        return Err(ApiError::new(
            ErrorCode::Conflict,
            "This idempotency key started a run with other inputs.",
        ));
    }
    app.store.jobs_create(
        &identity.tenant,
        &[NewJob {
            key: job_key,
            work,
            due_at: now,
            policy: RetryPolicy::default(),
        }],
        now,
    )?;
    app.audit(
        "automation.run_started",
        &identity.correlation,
        json!({
            "tenant": identity.tenant.as_str(),
            "automation": workflow.as_str(),
            "run": run,
        }),
    );
    Ok(json!({ "run": run, "replayed": false }))
}

/// [`start_run_at`] at the time of the platform's clock.
pub fn start_run(app: &App, identity: &Identity, body: &Json_) -> Result<Json_, ApiError> {
    start_run_at(app, identity, body, app.now())
}

/// Decide the approval a run waits for, as of `now` (approve-workflow-step): approve or refuse,
/// with a comment. The kernel decides whether the caller may; the run then goes on down the
/// approved or the refused branch through the job of that step, and the deadline job is cancelled.
/// An approval is recorded once: a run that is not waiting, or a step that holds a decision, is a
/// conflict (WFL-2683).
/// Implements: CT-092
pub fn approve_step_at(
    app: &App,
    identity: &Identity,
    body: &Json_,
    now: Timestamp,
) -> Result<Json_, ApiError> {
    same_tenant(app, identity, body)?;
    person(identity)?;
    let run_id = crate::service::text(body, "run", 64)?;
    let approved = body
        .get("approved")
        .and_then(Json_::as_bool)
        .ok_or_else(|| invalid("approved is missing or invalid."))?;
    let comment = match body.get("comment") {
        None | Some(Json_::Null) => None,
        Some(text) => Some(
            text.as_str()
                .filter(|text| text.chars().count() <= 500)
                .ok_or_else(|| invalid("comment is at most 500 characters."))?
                .to_string(),
        ),
    };
    let run = app
        .store
        .automation_run(&identity.tenant, run_id)?
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "There is no such run."))?;
    let decided = || {
        ApiError::new(
            ErrorCode::Conflict,
            "This run is not waiting for an approval: an approval is decided once.",
        )
    };
    if run.state != RunState::AwaitingApproval {
        return Err(decided());
    }
    let record = step_at(app, &identity.tenant, run_id, run.steps)?.ok_or_else(decided)?;
    let approval = match &record.approval {
        Some(approval) if approval.decision.is_none() && record.state == StepState::Waiting => {
            approval.clone()
        }
        _ => return Err(decided()),
    };
    let state = crate::ops::header(app, identity)?;
    let roles: BTreeSet<RoleId> = approval
        .roles
        .iter()
        .filter_map(|role| RoleId::new(role.as_str()).ok())
        .collect();
    let by = decide(
        &state,
        &identity.principal,
        &roles,
        run.started_by.as_ref(),
        approval.deadline,
        now,
    )
    .map_err(|refusal| {
        app.audit(
            "automation.approval_refused",
            &identity.correlation,
            json!({
                "tenant": identity.tenant.as_str(),
                "run": run_id,
                "why": refusal.describe(),
            }),
        );
        let code = if refusal.forbidden() {
            ErrorCode::Forbidden
        } else {
            ErrorCode::Conflict
        };
        ApiError::new(
            code,
            format!("This approval is refused: {}.", refusal.describe()),
        )
    })?;
    let def = state
        .schema
        .automations
        .as_ref()
        .and_then(|all| all.get(&run.automation))
        .ok_or_else(|| ApiError::new(ErrorCode::Conflict, "The automation was removed."))?;
    let pinned_def = super::steps::pinned(&run, def);
    let def = &pinned_def;
    let sequence = record.sequence;
    let decision = ApprovalDecision {
        approved,
        by: Some(by),
        comment,
        at: now,
        timed_out: false,
    };
    let writes = resolve(def, run.clone(), record, decision, now)
        .map_err(|why| ApiError::new(ErrorCode::Conflict, why))?;
    app.store.automation_write(&identity.tenant, &writes, now)?;
    // The deadline no longer applies; a job that is due anyway finds the step decided.
    let deadline = jobs::job_id(&identity.tenant, &timeout_key(run_id, sequence));
    let _ = app
        .store
        .job_cancel(&identity.tenant, &deadline, "the approval was decided", now);
    app.audit(
        "automation.approval_decided",
        &identity.correlation,
        json!({
            "tenant": identity.tenant.as_str(),
            "run": run_id,
            "approved": approved,
        }),
    );
    Ok(json!({ "state": RunState::Running.as_str(), "run": run_id, "approved": approved }))
}

/// [`approve_step_at`] at the time of the platform's clock.
pub fn approve_step(app: &App, identity: &Identity, body: &Json_) -> Result<Json_, ApiError> {
    approve_step_at(app, identity, body, app.now())
}

/// Activate an automation, as of `now` (activate-workflow): the owner, holding workflow-activate,
/// names a draft and the projection that was run before it (WFL-002). Activation is the kernel's
/// define-automation made again with the state active, by the activating principal: the run-as
/// roles must be ones that principal may confer (WFL-1528) and the catalogue check passes
/// (D-6538). An automation that is active already is answered as it is.
/// Implements: CT-089
pub fn activate_at(
    app: &App,
    identity: &Identity,
    body: &Json_,
    now: Timestamp,
) -> Result<Json_, ApiError> {
    same_tenant(app, identity, body)?;
    require_capability(app, identity, WORKFLOW_ACTIVATE)?;
    let state = crate::ops::header(app, identity)?;
    if !roles_of(&state, &identity.principal).contains(&owner_role()) {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "Only the site's owner may do this.",
        ));
    }
    let workflow = WorkflowId::new(crate::service::text(body, "workflow", 64)?)
        .map_err(|_| invalid("workflow is missing or invalid."))?;
    let def = state
        .schema
        .automations
        .as_ref()
        .and_then(|all| all.get(&workflow))
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "There is no such automation."))?;
    // A projection tries the automation on history before it runs on the present (WFL-002). The
    // request carries its summary; the platform does not yet record projections of its own.
    let projected = body
        .get("projection")
        .and_then(|projection| projection.get("diverged"))
        .is_some_and(Json_::is_boolean);
    if !projected {
        return Err(invalid(
            "No projection has run for this automation: the request carries none.",
        ));
    }
    if def.suspended_for.is_some() {
        return Err(ApiError::new(
            ErrorCode::Conflict,
            "The automation is suspended: define it again first.",
        ));
    }
    if activation::is_active(def) {
        return Ok(json!({ "version": state.schema.version.0 }));
    }
    let intent = Intent::ChangeSchema {
        changes: vec![SchemaChange::DefineAutomation(activation::activated(def))],
        expected_version: None,
        existing: ExistingData::Reject,
    };
    let mut context = identity.context(app);
    context.now = now;
    crate::ops::run_intent(app, &context, &intent, false)?;
    let after = crate::ops::header(app, identity)?;
    app.audit(
        "automation.activated",
        &identity.correlation,
        json!({
            "tenant": identity.tenant.as_str(),
            "automation": workflow.as_str(),
        }),
    );
    Ok(json!({ "version": after.schema.version.0 }))
}

/// [`activate_at`] at the time of the platform's clock.
pub fn activate(app: &App, identity: &Identity, body: &Json_) -> Result<Json_, ApiError> {
    activate_at(app, identity, body, app.now())
}

/// `POST /internal/automation/start`.
pub async fn start_route(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        start_run(app, identity, &body)
    })
    .await
}

/// `POST /internal/automation/approve`.
pub async fn approve_route(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        approve_step(app, identity, &body)
    })
    .await
}

/// `POST /internal/automation/activate`.
pub async fn activate_route(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        activate(app, identity, &body)
    })
    .await
}
