//! The tenant owner's view of automation (WFL-1531, SCH-1532): the runs, the steps of a run and
//! the declined, held, parked and dead-lettered work, and settling a parked or dead-lettered
//! record. Each answers for the caller's own tenant only.

use crate::app::{ApiError, App, Shared};
use crate::auth::Identity;
use crate::http::as_caller;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde_json::{json, Value as Json_};
use std::collections::BTreeSet;
use strata_kernel::rules::{holds_any, roles_of};
use strata_kernel::{owner_role, Grant, WorkflowId};
use strata_protocol::api::ErrorCode;
use strata_storage_api::jobs::{
    self, ExceptionKind, ExceptionRecord, Job, JobChange, RunRecord, RunState, Settled, StepRecord,
};

/// The capability that reading runs, steps and exception records needs (WFL-1531).
pub const AUTOMATIONS_READ: &str = "automations-read";
/// The capability that listing jobs and schedules needs (CT-086).
pub const JOBS_READ: &str = "jobs-read";

/// The most entries one page holds, and how many when the request names no limit.
const MAX_PAGE: u64 = 100;
const DEFAULT_PAGE: u64 = 50;

/// The caller is signed in and holds the capability `name`; the owner holds every capability.
pub(super) fn require_capability(
    app: &App,
    identity: &Identity,
    name: &str,
) -> Result<(), ApiError> {
    if identity.anonymous() {
        return Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in to see this.",
        ));
    }
    let tenant = crate::ops::header(app, identity)?;
    let wanted = BTreeSet::from([Grant::Capability(name.to_string())]);
    if holds_any(&tenant, &identity.principal, &wanted) {
        Ok(())
    } else {
        Err(ApiError::new(
            ErrorCode::Forbidden,
            format!("This needs the permission {name}."),
        ))
    }
}

/// A tenant named in the request must be the caller's: another is refused, recorded as an audit
/// entry and raised as an alert (TEN-1140, THR-259).
pub(super) fn same_tenant(app: &App, identity: &Identity, body: &Json_) -> Result<(), ApiError> {
    match body.get("tenant").and_then(Json_::as_str) {
        Some(named) if named != identity.tenant.as_str() => {
            app.audit(
                "tenant.mismatch",
                &identity.correlation,
                json!({ "tenant": identity.tenant.as_str(), "named": named, "route": "automation" }),
            );
            Err(ApiError::new(
                ErrorCode::Forbidden,
                "The request names another site.",
            ))
        }
        _ => Ok(()),
    }
}

/// The page size a request asks for: at most one hundred, fifty when it names none.
fn limit_of(body: &Json_) -> usize {
    body.get("limit")
        .and_then(Json_::as_u64)
        .unwrap_or(DEFAULT_PAGE)
        .clamp(1, MAX_PAGE) as usize
}

/// The cursor a request carries: the sequence number it continues from, as text.
fn cursor_of(body: &Json_) -> Result<Option<u64>, ApiError> {
    match body.get("cursor") {
        None | Some(Json_::Null) => Ok(None),
        Some(cursor) => cursor
            .as_str()
            .and_then(|text| text.parse::<u64>().ok())
            .map(Some)
            .ok_or_else(|| {
                ApiError::new(ErrorCode::Invalid, "The cursor is not one this list gave.")
            }),
    }
}

/// An optional text input that must parse.
fn optional<T>(
    body: &Json_,
    name: &str,
    parse: impl Fn(&str) -> Option<T>,
) -> Result<Option<T>, ApiError> {
    match body.get(name) {
        None | Some(Json_::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .and_then(parse)
            .map(Some)
            .ok_or_else(|| ApiError::new(ErrorCode::Invalid, format!("{name} is not valid."))),
    }
}

fn run_summary(run: &RunRecord) -> Json_ {
    json!({
        "run": run.run,
        "automation": run.automation.as_str(),
        "trigger": run.trigger.as_ref().map(|t| t.as_str()),
        "state": run.state.as_str(),
        "step": run.step.as_ref().map(|s| s.as_str()),
        "hop": run.hop,
        "caused_by": run.caused_by.as_str(),
        "on_behalf_of": run.on_behalf_of.as_ref().map(|p| p.as_str()),
        "started_at": run.started_at.0,
        "ended_at": run.ended_at.map(|t| t.0),
        "error": run.error,
    })
}

fn step_summary(step: &StepRecord) -> Json_ {
    json!({
        "sequence": step.sequence,
        "step": step.step.as_str(),
        "member": Json_::Null,
        "state": step.state,
        "attempts": step.attempts,
        "result": step.result,
        "errors": step.errors,
        "started_at": step.started_at.0,
        "ended_at": step.ended_at.map(|t| t.0),
    })
}

fn exception_summary(record: &ExceptionRecord) -> Json_ {
    json!({
        "exception": record.id,
        "kind": record.kind.as_str(),
        "reason": record.reason,
        "trigger": record.trigger.as_ref().map(|t| t.as_str()),
        "automation": record.automation.as_ref().map(|a| a.as_str()),
        "run": record.run,
        "step": record.step.as_ref().map(|s| s.as_str()),
        "event_seq": record.event_seq,
        "hop": record.hop,
        "at": record.at.0,
        "detail": record.detail,
        "settled": record.settled,
    })
}

/// The runs of the caller's tenant, newest first (list-automation-runs).
/// Implements: CT-794
pub fn list_runs(app: &App, identity: &Identity, body: &Json_) -> Result<Json_, ApiError> {
    same_tenant(app, identity, body)?;
    require_capability(app, identity, AUTOMATIONS_READ)?;
    let automation = optional(body, "automation", |text| WorkflowId::new(text).ok())?;
    let state = optional(body, "state", RunState::parse)?;
    let limit = limit_of(body);
    let runs = app.store.automation_runs(
        &identity.tenant,
        automation.as_ref(),
        state,
        cursor_of(body)?,
        limit,
    )?;
    let next = (runs.len() == limit)
        .then(|| runs.last().map(|run| run.seq.to_string()))
        .flatten();
    Ok(json!({ "runs": runs.iter().map(run_summary).collect::<Vec<_>>(), "next": next }))
}

/// The steps of one run of the caller's tenant, in order (list-automation-steps).
/// Implements: CT-795
pub fn list_steps(app: &App, identity: &Identity, body: &Json_) -> Result<Json_, ApiError> {
    same_tenant(app, identity, body)?;
    require_capability(app, identity, AUTOMATIONS_READ)?;
    let run = crate::service::text(body, "run", 64)?;
    if app.store.automation_run(&identity.tenant, run)?.is_none() {
        return Err(ApiError::new(ErrorCode::NotFound, "There is no such run."));
    }
    let limit = limit_of(body);
    let steps = app
        .store
        .automation_steps(&identity.tenant, run, cursor_of(body)?, limit)?;
    let next = (steps.len() == limit)
        .then(|| steps.last().map(|step| step.sequence.to_string()))
        .flatten();
    Ok(json!({ "steps": steps.iter().map(step_summary).collect::<Vec<_>>(), "next": next }))
}

/// The exception records of the caller's tenant, newest first (list-automation-exceptions).
/// Implements: CT-796
pub fn list_exceptions(app: &App, identity: &Identity, body: &Json_) -> Result<Json_, ApiError> {
    same_tenant(app, identity, body)?;
    require_capability(app, identity, AUTOMATIONS_READ)?;
    let kind = optional(body, "kind", ExceptionKind::parse)?;
    let unsettled = body
        .get("unsettled")
        .and_then(Json_::as_bool)
        .unwrap_or(false);
    let limit = limit_of(body);
    let records = app.store.automation_exceptions(
        &identity.tenant,
        kind,
        unsettled,
        cursor_of(body)?,
        limit,
    )?;
    let next = (records.len() == limit)
        .then(|| records.last().map(|record| record.seq.to_string()))
        .flatten();
    Ok(json!({
        "exceptions": records.iter().map(exception_summary).collect::<Vec<_>>(),
        "next": next,
    }))
}

/// Release or drop a parked or dead-lettered record of the caller's tenant: the owner's, holding
/// automations-define (settle-automation-exception).
/// Implements: CT-797
pub fn settle_exception(app: &App, identity: &Identity, body: &Json_) -> Result<Json_, ApiError> {
    same_tenant(app, identity, body)?;
    require_capability(
        app,
        identity,
        strata_kernel::definitions::AUTOMATIONS_DEFINE,
    )?;
    let tenant = crate::ops::header(app, identity)?;
    if !roles_of(&tenant, &identity.principal).contains(&owner_role()) {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "Only the site's owner may do this.",
        ));
    }
    let exception = crate::service::text(body, "exception", 64)?;
    let release = match crate::service::text(body, "action", 16)? {
        "release" => true,
        "drop" => false,
        _ => {
            return Err(ApiError::new(
                ErrorCode::Invalid,
                "The action is release or drop.",
            ))
        }
    };
    match app
        .store
        .automation_exception_settle(&identity.tenant, exception, release, app.now())?
    {
        Settled::NotFound => Err(ApiError::new(
            ErrorCode::NotFound,
            "There is no such exception.",
        )),
        Settled::Conflict(why) => Err(ApiError::new(ErrorCode::Conflict, why)),
        Settled::Done { job } => {
            app.audit(
                "automation.exception_settled",
                &identity.correlation,
                json!({
                    "tenant": identity.tenant.as_str(),
                    "exception": exception,
                    "action": if release { "release" } else { "drop" },
                }),
            );
            Ok(json!({ "job": job }))
        }
    }
}

/// A job as list-jobs shows it (job-summary): its state, its attempts and the error of each
/// (AGT-416), with what the owner needs to find it again.
fn job_summary(job: &Job) -> Json_ {
    json!({
        "job": job.id,
        "key": job.key,
        "state": job.state.as_str(),
        "due_at": job.due_at.0,
        "attempts": job.attempt,
        "errors": job.errors,
        "reason": job.reason,
        "created_at": job.created_at.0,
        "ended_at": job.ended_at.map(|t| t.0),
    })
}

/// The jobs of the caller's tenant, newest first, and on the first page its schedules
/// (list-jobs).
/// Implements: CT-086
pub fn list_jobs(app: &App, identity: &Identity, body: &Json_) -> Result<Json_, ApiError> {
    same_tenant(app, identity, body)?;
    require_capability(app, identity, JOBS_READ)?;
    let limit = limit_of(body);
    let cursor = cursor_of(body)?;
    let found = app.store.jobs_list(&identity.tenant, cursor, limit)?;
    let next = (found.len() == limit)
        .then(|| found.last().map(|job| job.seq.to_string()))
        .flatten();
    let schedules = match cursor {
        None => app.store.schedules(&identity.tenant)?,
        Some(_) => Vec::new(),
    };
    Ok(json!({
        "jobs": found.iter().map(job_summary).collect::<Vec<_>>(),
        "next": next,
        "schedules": schedules.iter().map(super::schedules::schedule_json).collect::<Vec<_>>(),
    }))
}

/// The scalar a plain JSON value stands for in a job's parameters.
fn param_of(value: &Json_) -> Result<strata_kernel::Value, ApiError> {
    match value {
        Json_::Null => Ok(strata_kernel::Value::Null),
        Json_::Bool(flag) => Ok(strata_kernel::Value::Bool(*flag)),
        Json_::String(text) => Ok(strata_kernel::Value::Str(text.clone())),
        Json_::Number(number) => number
            .as_i64()
            .map(strata_kernel::Value::Int)
            .ok_or_else(|| ApiError::new(ErrorCode::Invalid, "A parameter is a whole number.")),
        _ => Err(ApiError::new(
            ErrorCode::Invalid,
            "A parameter is a text, a whole number, true, false or null.",
        )),
    }
}

/// Schedule a job of the caller's tenant (schedule-job): the one kind of job a tenant may
/// schedule is `start-run`, a run of one of its automations at a time, with parameters (the
/// kinds of a job are the worker's, not free text: D-6520). The idempotency key makes the job:
/// the same key with the same work names the same job, and with other work is a conflict
/// (SCH-004). A time in the past makes the job due now.
/// Implements: CT-081
pub fn schedule_job(app: &App, identity: &Identity, body: &Json_) -> Result<Json_, ApiError> {
    same_tenant(app, identity, body)?;
    require_capability(app, identity, super::schedules::JOBS_MANAGE)?;
    let invalid = |why: &str| ApiError::new(ErrorCode::Invalid, why.to_string());
    if crate::service::text(body, "kind", 64)? != "start-run" {
        return Err(invalid(
            "A job of this kind is not known: schedule a start-run.",
        ));
    }
    let key = crate::service::text(body, "idempotency_key", 255)?;
    if !strata_storage_api::valid_idempotency_key(key) {
        return Err(invalid(
            "The idempotency key is 1 to 255 visible ASCII characters.",
        ));
    }
    let due = body
        .get("due_at")
        .and_then(Json_::as_i64)
        .ok_or_else(|| invalid("due_at is a time in milliseconds."))?;
    // The payload is an object, or JSON text of one (the contract's value-map).
    let payload = match body.get("payload") {
        Some(Json_::String(text)) => {
            serde_json::from_str::<Json_>(text).map_err(|_| invalid("The payload is not JSON."))?
        }
        Some(other) => other.clone(),
        None => {
            return Err(invalid(
                "A start-run needs a payload naming its automation.",
            ))
        }
    };
    let automation = payload
        .get("automation")
        .and_then(Json_::as_str)
        .and_then(|text| strata_kernel::WorkflowId::new(text).ok())
        .ok_or_else(|| invalid("The payload names an automation."))?;
    let mut params = std::collections::BTreeMap::new();
    if let Some(given) = payload.get("params") {
        let given = given
            .as_object()
            .ok_or_else(|| invalid("params is an object."))?;
        for (name, value) in given {
            params.insert(name.clone(), param_of(value)?);
        }
    }
    let tenant = crate::ops::header(app, identity)?;
    let def = tenant
        .schema
        .automations
        .as_ref()
        .and_then(|all| all.get(&automation))
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "There is no such automation."))?;
    if def.suspended_for.is_some() {
        return Err(ApiError::new(
            ErrorCode::Conflict,
            "The automation is suspended.",
        ));
    }
    let caused_by = match &identity.principal {
        strata_kernel::Principal::User(id) => Some(id.clone()),
        _ => None,
    };
    let job = super::schedules::start_job(
        def,
        None,
        0,
        params,
        caused_by,
        tenant.schema.version,
        format!("api:{key}"),
        strata_kernel::Timestamp(due),
    )
    .map_err(|why| ApiError::new(ErrorCode::Invalid, why))?;
    let id = jobs::job_id(&identity.tenant, &job.key);
    if let Some(existing) = app.store.job(&identity.tenant, &id)? {
        if existing.work != job.work {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                "This idempotency key made a job with other work.",
            ));
        }
        return Ok(json!({ "job": id }));
    }
    let ids = app.store.jobs_create(&identity.tenant, &[job], app.now())?;
    app.audit(
        "automation.job_scheduled",
        &identity.correlation,
        json!({ "tenant": identity.tenant.as_str(), "job": ids[0], "automation": automation.as_str() }),
    );
    Ok(json!({ "job": ids[0] }))
}

/// Who asked, as the text the record of a cancellation keeps.
fn requester(identity: &Identity) -> String {
    match &identity.principal {
        strata_kernel::Principal::User(id) => id.as_str().to_string(),
        other => format!("{other:?}"),
    }
}

/// Cancel a job of the caller's tenant (cancel-job): no later attempt happens (SCH-006), and the
/// requester and the reason are recorded on the job and in the audit trail (AGT-417). A job of
/// another tenant is not found. A job that has ended is left as it is.
/// Implements: CT-084
pub fn cancel_job(app: &App, identity: &Identity, body: &Json_) -> Result<Json_, ApiError> {
    same_tenant(app, identity, body)?;
    require_capability(app, identity, super::schedules::JOBS_MANAGE)?;
    let job = crate::service::text(body, "job", 64)?;
    let reason = crate::service::text(body, "reason", 256)?;
    let note = format!("cancelled by {}: {reason}", requester(identity));
    match app
        .store
        .job_cancel(&identity.tenant, job, &note, app.now())?
    {
        JobChange::NotFound => Err(ApiError::new(ErrorCode::NotFound, "There is no such job.")),
        JobChange::Unchanged(held) => Ok(json!({ "state": held.state.as_str() })),
        JobChange::Changed(held) => {
            app.audit(
                "automation.job_cancelled",
                &identity.correlation,
                json!({
                    "tenant": identity.tenant.as_str(),
                    "job": held.id,
                    "by": requester(identity),
                    "reason": reason,
                }),
            );
            Ok(json!({ "state": held.state.as_str() }))
        }
    }
}

/// Give a failed job of the caller's tenant one more attempt (retry-job): the outcomes of the
/// earlier attempts and the step key are unchanged (AGT-418, SCH-1524). A job that has not failed
/// is a conflict; a job of another tenant is not found. The requester is recorded in the audit
/// trail.
/// Implements: CT-085
pub fn retry_job(app: &App, identity: &Identity, body: &Json_) -> Result<Json_, ApiError> {
    same_tenant(app, identity, body)?;
    require_capability(app, identity, super::schedules::JOBS_MANAGE)?;
    let job = crate::service::text(body, "job", 64)?;
    match app.store.job_retry(&identity.tenant, job, app.now())? {
        JobChange::NotFound => Err(ApiError::new(ErrorCode::NotFound, "There is no such job.")),
        JobChange::Unchanged(_) => Err(ApiError::new(
            ErrorCode::Conflict,
            "The job has not failed.",
        )),
        JobChange::Changed(held) => {
            app.audit(
                "automation.job_retried",
                &identity.correlation,
                json!({
                    "tenant": identity.tenant.as_str(),
                    "job": held.id,
                    "by": requester(identity),
                }),
            );
            Ok(json!({ "attempt": held.attempt.saturating_add(1) }))
        }
    }
}

/// `POST /internal/jobs/list`.
pub async fn jobs_list_route(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        list_jobs(app, identity, &body)
    })
    .await
}

/// `POST /internal/jobs/schedule`.
pub async fn jobs_schedule_route(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        schedule_job(app, identity, &body)
    })
    .await
}

/// `POST /internal/jobs/cancel`.
pub async fn jobs_cancel_route(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        cancel_job(app, identity, &body)
    })
    .await
}

/// `POST /internal/jobs/retry`.
pub async fn jobs_retry_route(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        retry_job(app, identity, &body)
    })
    .await
}

/// `POST /internal/automation/runs`.
pub async fn runs_route(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        list_runs(app, identity, &body)
    })
    .await
}

/// `POST /internal/automation/steps`.
pub async fn steps_route(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        list_steps(app, identity, &body)
    })
    .await
}

/// `POST /internal/automation/exceptions`.
pub async fn exceptions_route(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        list_exceptions(app, identity, &body)
    })
    .await
}

/// `POST /internal/automation/exceptions/settle`.
pub async fn settle_route(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        settle_exception(app, identity, &body)
    })
    .await
}
