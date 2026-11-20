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
    ExceptionKind, ExceptionRecord, RunRecord, RunState, Settled, StepRecord,
};

/// The capability that reading runs, steps and exception records needs (WFL-1531).
pub const AUTOMATIONS_READ: &str = "automations-read";

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
        "started_by": run.started_by.as_ref().map(|p| p.as_str()),
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
        "approval": step.approval,
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
