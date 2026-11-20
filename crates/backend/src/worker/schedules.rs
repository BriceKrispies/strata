//! Schedules: define-schedule, and the job that makes a schedule fall due (WG-5155, SCH-003,
//! SCH-005, SCH-1533, SCH-1573, SCH-1574, D-6531, D-6567).
//!
//! - **Defining** ([`define_schedule`], CT-087): the owner (capability `jobs-manage`) gives a
//!   schedule an identifier, a recurrence of the subset of SCH-1574 ([`super::recurrence`]) and
//!   whether it is paused. The store holds the schedule and creates its first job in one
//!   transaction ([`strata_storage_api::Store::schedule_set`]); a rule outside the subset is
//!   refused as invalid and nothing is written.
//! - **Falling due** ([`run_due`]): the job of an occurrence emits the schedule's event, hop zero
//!   (D-6531, SCH-1533): each trigger of the tenant on `schedule-due` of this schedule is decided
//!   as a trigger on any event is (D-6524) and, when it starts a run, creates the start job. The
//!   report of the job carries the next occurrence, and the store creates the next job in the
//!   transaction that completes this one (SCH-003), unless the schedule is paused (SCH-005): a
//!   schedule paused while its job is open runs no trigger when the job falls due.
//! - **Missed occurrences**: a worker that was down when occurrences fell due runs the open job
//!   once and the next occurrence is the first after the time it ran, so a long stop does not
//!   release a burst of catch-up runs.

use super::recurrence::Recurrence;
use super::routes::{require_capability, same_tenant};
use super::steps::{self, Outcome};
use crate::app::{ApiError, App, Shared};
use crate::auth::Identity;
use crate::http::as_caller;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde_json::{json, Value as Json_};
use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::automation::automation_principal;
use strata_kernel::expr::{self, Env};
use strata_kernel::{
    canon, AutomationDef, Principal, PrincipalId, RoleId, SchemaVersion, SettingId, TenantId,
    TenantState, Timestamp, TriggerDef, TriggerEvent, TriggerId, Value,
};
use strata_protocol::api::ErrorCode;
use strata_storage_api::jobs::{
    self, ExceptionKind, ExceptionRecord, JobReport, Lease, NewJob, RetryPolicy, WorkerWrites,
};
use strata_storage_api::schedules::{Schedule, ScheduleAdvance, ScheduleSet};

/// The capability that schedules, cancels and retries jobs and defines schedules.
pub const JOBS_MANAGE: &str = "jobs-manage";

/// The zone a tenant reads local times in: the tenant level of the setting `time-zone`, else UTC
/// (SCH-1573).
pub fn tenant_zone(tenant: &TenantState) -> String {
    SettingId::new("time-zone")
        .ok()
        .and_then(|id| strata_kernel::settings::resolve(tenant, &Principal::System, &id).ok())
        .and_then(|resolution| match resolution.value {
            Value::Str(name) => Some(name),
            _ => None,
        })
        .unwrap_or_else(|| "UTC".to_string())
}

/// A schedule identifier as the contract writes it (schedule-id, a UUID), in its canonical lower
/// case form.
fn schedule_id(body: &Json_) -> Result<String, ApiError> {
    let text = crate::service::text(body, "schedule", 64)?.to_ascii_lowercase();
    let shaped = text.len() == 36
        && text.bytes().enumerate().all(|(at, b)| match at {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        });
    if shaped {
        Ok(text)
    } else {
        Err(ApiError::new(
            ErrorCode::Invalid,
            "A schedule is named by a UUID.",
        ))
    }
}

/// Create or change a schedule of the caller's tenant (define-schedule).
///
/// The rule is read in the zone it names, else the tenant's (SCH-1574); a rule outside the subset
/// is refused as invalid. A schedule that is new, or whose rule changed, counts from now; one
/// whose rule is the same keeps its anchor, so that defining it again changes nothing
/// (the contract is idempotent). `paused` left out keeps the schedule as it is paused or not.
/// Implements: CT-087
pub fn define_schedule(app: &App, identity: &Identity, body: &Json_) -> Result<Json_, ApiError> {
    same_tenant(app, identity, body)?;
    require_capability(app, identity, JOBS_MANAGE)?;
    let id = schedule_id(body)?;
    let rule = crate::service::text(body, "rule", 512)?;
    let paused = match body.get("paused") {
        None | Some(Json_::Null) => None,
        Some(Json_::Bool(paused)) => Some(*paused),
        Some(_) => {
            return Err(ApiError::new(
                ErrorCode::Invalid,
                "paused is true or false.",
            ))
        }
    };
    let tenant = crate::ops::header(app, identity)?;
    let recurrence = Recurrence::parse(rule, &tenant_zone(&tenant)).map_err(|why| {
        ApiError::new(
            ErrorCode::Invalid,
            format!("The recurrence is not in the supported subset of RFC 5545: {why}."),
        )
    })?;
    let now = app.now();
    let held = app.store.schedule(&identity.tenant, &id)?;
    let anchor = match &held {
        Some(held) if held.rule == rule => held.anchor,
        _ => now,
    };
    let due = recurrence.next_after(anchor.0, now.0).map(Timestamp);
    let schedule = app.store.schedule_set(
        &identity.tenant,
        &ScheduleSet {
            id: id.clone(),
            rule: rule.to_string(),
            paused: paused.unwrap_or_else(|| held.as_ref().is_some_and(|held| held.paused)),
            anchor,
            due,
        },
        now,
    )?;
    app.audit(
        "automation.schedule_defined",
        &identity.correlation,
        json!({
            "tenant": identity.tenant.as_str(),
            "schedule": id,
            "paused": schedule.paused,
        }),
    );
    Ok(schedule_json(&schedule))
}

/// A schedule as the contract answers it.
pub fn schedule_json(schedule: &Schedule) -> Json_ {
    json!({
        "schedule": schedule.id,
        "rule": schedule.rule,
        "paused": schedule.paused,
        "next_due": schedule.next_due.0,
    })
}

/// `POST /internal/schedules/define`.
pub async fn define_route(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        define_schedule(app, identity, &body)
    })
    .await
}

/// The next occurrence of `schedule` after the occurrence that is due now and after now: none when
/// the rule has none left or no longer reads (the zone setting changed to a name that is not one).
fn next_occurrence(
    schedule: &Schedule,
    zone: &str,
    due: Timestamp,
    now: Timestamp,
) -> Option<Timestamp> {
    Recurrence::parse(&schedule.rule, zone)
        .ok()
        .and_then(|rule| rule.next_after(schedule.anchor.0, due.0.max(now.0)))
        .map(Timestamp)
}

/// A job's parameters as the kernel's values.
fn event_params(n: u64, schedule: &str, due: Timestamp) -> BTreeMap<String, Value> {
    BTreeMap::from([
        (
            "seq".to_string(),
            Value::Int(i64::try_from(n).unwrap_or(i64::MAX)),
        ),
        ("at".to_string(), Value::Time(due.0)),
        ("hop".to_string(), Value::Int(0)),
        ("schedule".to_string(), Value::Str(schedule.to_string())),
        ("due_at".to_string(), Value::Time(due.0)),
    ])
}

/// The job that starts a run of `def` for an event or a request: the run key, when the automation
/// has one, evaluated over `params` as the automation principal (D-6534), and the hop count zero
/// (D-6531). An expression that cannot be evaluated, or gives null, is an `Err` with the reason.
#[allow(clippy::too_many_arguments)]
pub(super) fn start_job(
    def: &AutomationDef,
    trigger: Option<&TriggerId>,
    event_seq: u64,
    params: BTreeMap<String, Value>,
    caused_by: Option<PrincipalId>,
    version: SchemaVersion,
    key: String,
    due: Timestamp,
) -> Result<NewJob, String> {
    let principal = Principal::User(automation_principal(&def.id));
    let roles: BTreeSet<RoleId> = def.run_as.roles.clone();
    let run_key = match &def.run_key {
        None => None,
        Some(expression) => {
            let env = Env {
                record: None,
                old: None,
                new: None,
                params: &params,
                now: due,
                principal: &principal,
                roles: &roles,
                lookup: None,
            };
            match expr::eval(expression, &env) {
                Ok(Value::Null) => {
                    return Err(format!("the run key of automation {} is null", def.id))
                }
                Ok(value) => Some(canon::hash_of(&value)),
                Err(_) => {
                    return Err(format!(
                        "the run key of automation {} cannot be evaluated",
                        def.id
                    ))
                }
            }
        }
    };
    Ok(NewJob {
        key,
        work: jobs::JobWork::StartRun {
            trigger: trigger.cloned(),
            automation: def.id.clone(),
            event_seq: Some(event_seq),
            hop: 0,
            params,
            caused_by,
            on_behalf_of: def.defined_by.clone(),
            definition_version: Some(version),
            run_key,
        },
        due_at: due,
        policy: RetryPolicy::default(),
    })
}

/// The effect of one trigger on a schedule's event: a job to start a run, a declined record, or
/// nothing (a paused trigger, a false condition).
enum Decision {
    Start(NewJob),
    Declined(ExceptionRecord),
    Nothing,
}

/// Decide one trigger on the event of schedule occurrence `n` (D-6524): a missing automation and a
/// suspended one decline, a false condition starts nothing, a condition that cannot be evaluated
/// declines with `condition-failed`, otherwise the run starts at hop zero.
#[allow(clippy::too_many_arguments)]
fn decide(
    tenant: &TenantId,
    lease: &Lease,
    schema: &strata_kernel::TenantSchema,
    trigger: &TriggerDef,
    n: u64,
    schedule: &str,
    due: Timestamp,
    now: Timestamp,
) -> Decision {
    let declined = |reason: &str, detail: String| {
        let mut record = steps::exception(
            tenant,
            ExceptionKind::Declined,
            reason,
            &trigger.automation,
            None,
            None,
            &lease.job,
            detail,
            now,
        );
        record.id = jobs::derived_uuid(&format!(
            "strata/automation-exception/v1\0{tenant}\0declined\0{}\0{n}\0{reason}",
            trigger.id
        ));
        record.trigger = Some(trigger.id.clone());
        record.event_seq = Some(n);
        record.hop = Some(0);
        Decision::Declined(record)
    };
    if trigger.paused == Some(true) {
        return Decision::Nothing;
    }
    let Some(def) = schema
        .automations
        .as_ref()
        .and_then(|all| all.get(&trigger.automation))
    else {
        return declined(
            "automation-removed",
            format!("automation {} does not exist", trigger.automation),
        );
    };
    if let Some(role) = &def.suspended_for {
        return declined(
            "run-as-suspended",
            format!(
                "automation {} is suspended: its definer may not confer role {role}",
                def.id
            ),
        );
    }
    let params = event_params(n, schedule, due);
    if let Some(condition) = &trigger.condition {
        let principal = Principal::User(automation_principal(&def.id));
        let roles: BTreeSet<RoleId> = def.run_as.roles.clone();
        let env = Env {
            record: None,
            old: None,
            new: None,
            params: &params,
            now: due,
            principal: &principal,
            roles: &roles,
            lookup: None,
        };
        match expr::holds(condition, &env) {
            Ok(true) => {}
            Ok(false) => return Decision::Nothing,
            Err(_) => {
                return declined(
                    "condition-failed",
                    format!(
                        "the condition of trigger {} cannot be evaluated",
                        trigger.id
                    ),
                )
            }
        }
    }
    let key = format!("start:{}:{}:{n}", trigger.id, def.id);
    match start_job(
        def,
        Some(&trigger.id),
        n,
        params,
        Some(automation_principal(&def.id)),
        schema.version,
        key,
        now,
    ) {
        Ok(job) => Decision::Start(job),
        Err(why) => declined("condition-failed", why),
    }
}

/// Run the job of a schedule occurrence that fell due at `due`, at `now`: emit the event of the
/// schedule to its triggers and report the next occurrence (see the module notes). A failure that
/// can be retried leaves the schedule alone; the failure that ends the job still advances the
/// schedule, so that one bad occurrence does not stop the recurrence (SCH-003).
pub fn run_due(
    app: &App,
    tenant: &TenantId,
    lease: &Lease,
    schedule_id: &str,
    due: Timestamp,
    now: Timestamp,
) -> Outcome {
    let advance_only = |next_due: Option<Timestamp>| WorkerWrites {
        schedule_advance: Some(ScheduleAdvance {
            schedule: schedule_id.to_string(),
            next_due,
        }),
        ..WorkerWrites::default()
    };
    let schedule = match app.store.schedule(tenant, schedule_id) {
        Ok(Some(schedule)) => schedule,
        // A schedule that is not there has nothing to run or to follow.
        Ok(None) => return Outcome::done(WorkerWrites::default()),
        Err(error) => {
            return Outcome {
                report: JobReport::Failed {
                    error: error.to_string(),
                    retry: !lease.exhausted,
                },
                writes: WorkerWrites::default(),
            }
        }
    };
    let state = app.store.load(tenant, &[]).map(|state| state.tenant);
    let (zone, schema) = match &state {
        Ok(Some(found)) => (tenant_zone(found), Some(found.schema.clone())),
        _ => ("UTC".to_string(), None),
    };
    let next_due = next_occurrence(&schedule, &zone, due, now);
    let (Some(schema), false) = (schema, lease.exhausted) else {
        // The tenant could not be read, or the lease of the last attempt expired: the job fails,
        // and when that is final the schedule moves on.
        let error = match &state {
            Err(error) => error.to_string(),
            _ => lease
                .job
                .errors
                .last()
                .cloned()
                .unwrap_or_else(|| "the schedule could not be run".to_string()),
        };
        let report = JobReport::Failed {
            error,
            retry: !lease.exhausted,
        };
        let last = lease.exhausted || jobs::report_fails_job(&lease.job, &report);
        return Outcome {
            writes: if last {
                advance_only(next_due)
            } else {
                WorkerWrites::default()
            },
            report,
        };
    };
    // A paused schedule runs no trigger (SCH-005); its job completes and no job follows.
    if schedule.paused {
        return Outcome::done(advance_only(next_due));
    }
    let n = schedule.completions + 1;
    let mut writes = advance_only(next_due);
    let triggers = schema.triggers.iter().flat_map(BTreeMap::values);
    for trigger in triggers {
        let on_this = matches!(
            &trigger.on,
            TriggerEvent::ScheduleDue { schedule: named } if named.to_string() == schedule_id
        );
        if !on_this {
            continue;
        }
        match decide(tenant, lease, &schema, trigger, n, schedule_id, due, now) {
            Decision::Start(job) => writes.jobs.push(job),
            Decision::Declined(record) => writes.exceptions.push(record),
            Decision::Nothing => {}
        }
    }
    Outcome::done(writes)
}
