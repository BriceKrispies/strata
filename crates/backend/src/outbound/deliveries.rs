//! Outbound webhook deliveries (component `connector-runner`, webhook-delivery-flow): the
//! subscriptions they go to, the job that makes each attempt through the outbound gate, and the
//! log an owner reads (WFL-010, WFL-255, INT-152).
//!
//! - **A subscription** is a target address and a sealed secret (rec-webhook-subscription). The
//!   address's form is judged when it is saved (HTTPS, port 443, no credentials); the address it
//!   resolves to is judged at every attempt, by the gate.
//! - **A delivery** is created by a `call-webhook` step (WFL-010, D-6528): its id is derived from
//!   the step key, so an attempt of the step made again after a crash names the same delivery and
//!   the same job, and nothing is sent twice for it. The step completes once the delivery is
//!   queued; its result names the delivery, with status 0 for "queued, see the log".
//! - **An attempt** is one lease of the delivery's job. It is written to the log before the post
//!   (attempt count) and when the answer comes (status), signed with every live secret of the
//!   subscription and the time of the attempt, under the delivery's id as the event identifier
//!   (INT-2640). A 2xx answer delivers it, and a delivered delivery is never posted again, even
//!   by an attempt made again after a crash. A 5xx, 408 or 429 answer, a time bound passed and an
//!   address that did not answer are retried, at most three attempts five and ten seconds apart
//!   (WFL-2643); the attempt that spends them dead-letters the job, which the owner may release.
//!   Any other answer, and a refused address, ends the delivery at once.
//! - **The log** shows, for each delivery, the target as a log line may show it (no query, no
//!   userinfo, WFL-254), the last response status, the attempt count and the state, and never a
//!   body (INT-152, PRV-366).
//!
//! Implements: CT-173, CT-1630, CT-1631, WFL-010, WFL-255, INT-152, WFL-2643

use super::gate::{Method, Request};
use super::rule::redacted_text;
use super::secrets::{self, SealedSecrets};
use super::signature;
use crate::app::{ApiError, App};
use crate::auth::Identity;
use crate::worker::steps::Outcome;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use strata_kernel::{StepName, TenantId, Timestamp, Value, WorkflowId};
use strata_model::types::Identifier;
use strata_protocol::api::ErrorCode;
use strata_storage_api::jobs::{
    self, ExceptionKind, ExceptionRecord, JobReport, JobWork, Lease, NewJob, RetryPolicy,
    WorkerWrites,
};

/// The document kind of a subscription.
pub const SUBSCRIPTION_KIND: &str = "webhook-subscription";
/// The document kind of a delivery's log entry.
pub const DELIVERY_KIND: &str = "webhook-delivery";

/// The attempts of a delivery (webhook-delivery-flow, WFL-2643).
pub const ATTEMPTS: u32 = 3;
/// The delay before the second attempt; the third waits twice as long (WFL-2643).
pub const FIRST_DELAY_MS: i64 = 5_000;
/// The most response bytes an attempt reads: only the status is kept.
pub const RESPONSE_BYTES: u64 = 64 * 1024;

/// One subscription: where deliveries go and what they are signed with.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Subscription {
    pub id: String,
    /// The target address as saved, query included. Never logged and never listed whole.
    pub target: String,
    pub secrets: SealedSecrets,
    pub created_at: i64,
}

/// Where a delivery is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeliveryState {
    Queued,
    Retrying,
    Delivered,
    Failed,
}

impl DeliveryState {
    pub fn as_str(&self) -> &'static str {
        match self {
            DeliveryState::Queued => "queued",
            DeliveryState::Retrying => "retrying",
            DeliveryState::Delivered => "delivered",
            DeliveryState::Failed => "failed",
        }
    }
}

/// One delivery's log entry (rec-webhook-delivery, type webhook-delivery).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    pub id: String,
    pub subscription: String,
    /// The target as a log line may show it (WFL-254).
    pub target: String,
    pub state: DeliveryState,
    /// The status of the last answer; 0 before one, or when the last attempt had none.
    pub status: u16,
    pub attempts: u32,
    pub created_at: i64,
    /// The time of the last attempt, or of the creation before one.
    pub at: i64,
    /// Why the last attempt did not deliver: `status <n>` or a gate failure's code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
}

/// The associated data a subscription's secrets are sealed with.
fn subscription_aad(tenant: &TenantId, id: &str) -> Vec<u8> {
    format!("strata/webhook-subscription/v1\0{tenant}\0{id}").into_bytes()
}

/// The retry policy of a delivery's job (WFL-2643). One attempt may hold its lease for a minute,
/// twice the gate's bound on a call.
pub fn delivery_policy() -> RetryPolicy {
    RetryPolicy {
        attempts: ATTEMPTS,
        first_delay_ms: FIRST_DELAY_MS,
        max_delay_ms: FIRST_DELAY_MS * 2,
        lease_ms: 60_000,
    }
}

/// A value of the rule language as JSON: what a delivery's body is made of.
pub fn value_json(value: &Value) -> Json {
    match value {
        Value::Null => Json::Null,
        Value::Bool(b) => json!(b),
        Value::Int(n) => json!(n),
        Value::Str(text) => json!(text),
        Value::Time(ms) => json!(ms),
        Value::Ref(id) => json!(id.0),
        Value::List(items) => Json::Array(items.iter().map(value_json).collect()),
    }
}

fn read<T: for<'de> Deserialize<'de>>(
    app: &App,
    tenant: &TenantId,
    kind: &str,
    key: &str,
) -> Result<Option<T>, String> {
    match app
        .store
        .doc_get(tenant, kind, key)
        .map_err(|e| e.to_string())?
    {
        None => Ok(None),
        Some(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| format!("{kind} {key}: {e}")),
    }
}

fn write<T: Serialize>(
    app: &App,
    tenant: &TenantId,
    kind: &str,
    key: &str,
    value: &T,
    create_only: bool,
) -> Result<bool, String> {
    let text = serde_json::to_string(value).map_err(|e| format!("{kind} {key}: {e}"))?;
    app.store
        .doc_put(tenant, kind, key, &text, create_only)
        .map_err(|e| e.to_string())
}

/// Why a delivery could not be queued.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnqueueError {
    /// The subscription does not exist: the step cannot succeed.
    Missing(String),
    /// The store failed: an attempt made again may succeed.
    Store(String),
}

/// The id of the delivery of a step: derived from the step key, the same on every attempt.
pub fn delivery_id(tenant: &TenantId, step_key: &str) -> String {
    jobs::derived_uuid(&format!("strata/webhook-delivery/v1\0{tenant}\0{step_key}"))
}

/// Queue the delivery of a `call-webhook` step (WFL-010): its log entry and its job, each created
/// once for the step key, so an attempt of the step made again changes nothing. Returns the
/// delivery's id.
#[allow(clippy::too_many_arguments)]
pub fn enqueue_from_step(
    app: &App,
    tenant: &TenantId,
    automation: &WorkflowId,
    run: &str,
    step: Option<&StepName>,
    step_key: &str,
    subscription: &Identifier,
    body: Json,
    now: Timestamp,
) -> Result<Identifier, EnqueueError> {
    let subscription_id = subscription.to_string();
    let held: Subscription = read(app, tenant, SUBSCRIPTION_KIND, &subscription_id)
        .map_err(EnqueueError::Store)?
        .ok_or_else(|| {
            EnqueueError::Missing(format!(
                "webhook subscription {subscription_id} does not exist"
            ))
        })?;
    let id = delivery_id(tenant, step_key);
    let entry = Delivery {
        id: id.clone(),
        subscription: subscription_id.clone(),
        target: redacted_text(&held.target),
        state: DeliveryState::Queued,
        status: 0,
        attempts: 0,
        created_at: now.0,
        at: now.0,
        outcome: None,
        run: Some(run.to_string()),
    };
    write(app, tenant, DELIVERY_KIND, &id, &entry, true).map_err(EnqueueError::Store)?;
    let job = NewJob {
        key: format!("deliver:{id}"),
        work: JobWork::Deliver {
            subscription: subscription_id,
            delivery: id.clone(),
            body: body.to_string(),
            automation: Some(automation.clone()),
            run: Some(run.to_string()),
            step: step.cloned(),
        },
        due_at: now,
        policy: delivery_policy(),
    };
    // Created once for its key: an attempt of the step made again creates nothing more.
    app.store
        .jobs_create(tenant, &[job], now)
        .map_err(|e| EnqueueError::Store(e.to_string()))?;
    serde_json::from_value::<Identifier>(json!(id))
        .map_err(|e| EnqueueError::Store(format!("delivery id {e}")))
}

/// The outcome of an attempt that did not deliver: retried while `retry` and the policy allow,
/// dead-lettered when the attempt that spends the policy was retryable.
fn not_delivered(
    app: &App,
    tenant: &TenantId,
    lease: &Lease,
    entry: &mut Delivery,
    why: String,
    retry: bool,
    now: Timestamp,
) -> Outcome {
    let job = &lease.job;
    let report = JobReport::Failed {
        error: format!("webhook delivery {}: {why}", entry.id),
        retry: retry && !lease.exhausted,
    };
    let fails = lease.exhausted || jobs::report_fails_job(job, &report);
    entry.state = if fails {
        DeliveryState::Failed
    } else {
        DeliveryState::Retrying
    };
    entry.outcome = Some(why);
    entry.at = now.0;
    let key = entry.id.clone();
    if let Err(error) = write(app, tenant, DELIVERY_KIND, &key, &*entry, false) {
        crate::app::LOG.error(
            "webhook.log_failed",
            "",
            json!({ "tenant": tenant.as_str(), "delivery": entry.id, "error": error }),
        );
    }
    let mut writes = WorkerWrites::default();
    if fails && (retry || lease.exhausted) {
        let (automation, run, step) = match &job.work {
            JobWork::Deliver {
                automation,
                run,
                step,
                ..
            } => (automation.clone(), run.clone(), step.clone()),
            _ => (None, None, None),
        };
        writes.exceptions.push(ExceptionRecord {
            id: jobs::derived_uuid(&format!(
                "strata/automation-exception/v1\0{tenant}\0{}\0attempts-exhausted\0{}",
                job.id, job.attempt
            )),
            kind: ExceptionKind::DeadLettered,
            reason: "attempts-exhausted".to_string(),
            trigger: None,
            automation,
            run,
            step,
            event_seq: None,
            job: Some(job.id.clone()),
            hop: None,
            at: now,
            detail: format!(
                "webhook delivery {} spent {} attempts",
                entry.id, job.attempt
            ),
            settled: false,
            seq: 0,
        });
    }
    Outcome { report, writes }
}

/// Make one attempt of a delivery job (webhook-delivery-flow).
pub fn run_delivery(app: &App, tenant: &TenantId, lease: &Lease, now: Timestamp) -> Outcome {
    let job = &lease.job;
    let JobWork::Deliver {
        subscription,
        delivery,
        body,
        run,
        ..
    } = &job.work
    else {
        return Outcome {
            report: JobReport::Failed {
                error: "not a webhook delivery".into(),
                retry: false,
            },
            writes: WorkerWrites::default(),
        };
    };
    let retry_later = |error: String| Outcome {
        report: JobReport::Failed {
            error,
            retry: !lease.exhausted,
        },
        writes: WorkerWrites::default(),
    };
    let mut entry = match read::<Delivery>(app, tenant, DELIVERY_KIND, delivery) {
        Ok(Some(entry)) => entry,
        Ok(None) => Delivery {
            id: delivery.clone(),
            subscription: subscription.clone(),
            target: String::new(),
            state: DeliveryState::Queued,
            status: 0,
            attempts: 0,
            created_at: now.0,
            at: now.0,
            outcome: None,
            run: run.clone(),
        },
        Err(error) => return retry_later(error),
    };
    // Delivered once: an attempt made again after a crash between the post and the report posts
    // nothing more.
    if entry.state == DeliveryState::Delivered {
        return Outcome {
            report: JobReport::Succeeded,
            writes: WorkerWrites::default(),
        };
    }
    if lease.exhausted {
        let why = job
            .errors
            .last()
            .cloned()
            .unwrap_or_else(|| "the lease expired".into());
        return not_delivered(
            app,
            tenant,
            lease,
            &mut entry,
            jobs::clip_error(&why),
            false,
            now,
        );
    }
    let held: Subscription = match read(app, tenant, SUBSCRIPTION_KIND, subscription) {
        Ok(Some(held)) => held,
        Ok(None) => {
            return not_delivered(
                app,
                tenant,
                lease,
                &mut entry,
                "subscription-removed".into(),
                false,
                now,
            )
        }
        Err(error) => return retry_later(error),
    };
    let keys = secrets::live(
        app,
        &subscription_aad(tenant, &held.id),
        &held.secrets,
        now.0,
    );
    if keys.is_empty() {
        return not_delivered(
            app,
            tenant,
            lease,
            &mut entry,
            "secret-unavailable".into(),
            false,
            now,
        );
    }
    // The attempt is in the log before it is posted (wh-send-t).
    entry.attempts = job.attempt;
    entry.at = now.0;
    entry.target = redacted_text(&held.target);
    if let Err(error) = write(app, tenant, DELIVERY_KIND, delivery, &entry, false) {
        return retry_later(error);
    }
    let timestamp = now.0.div_euclid(1000);
    let bytes = body.as_bytes();
    let mut request = Request::new(
        Method::Post,
        held.target.clone(),
        format!("webhook {tenant}"),
    );
    request.headers = vec![
        ("content-type".into(), "application/json".into()),
        ("user-agent".into(), "Strata-Webhook/1".into()),
        (signature::HEADER_EVENT.into(), delivery.clone()),
        (signature::HEADER_TIMESTAMP.into(), timestamp.to_string()),
        (
            signature::HEADER_SIGNATURE.into(),
            signature::header_value(&keys, delivery, timestamp, bytes),
        ),
    ];
    request.body = bytes.to_vec();
    request.max_bytes = Some(RESPONSE_BYTES);
    request.idempotency_key = Some(delivery.clone());
    match super::gate(app).call(&request) {
        Ok(answer) if (200..300).contains(&answer.status) => {
            entry.status = answer.status;
            entry.state = DeliveryState::Delivered;
            entry.outcome = None;
            entry.at = now.0;
            if let Err(error) = write(app, tenant, DELIVERY_KIND, delivery, &entry, false) {
                // Posted and answered: the job succeeds whatever the log; a repeat would post again.
                crate::app::LOG.error(
                    "webhook.log_failed",
                    "",
                    json!({ "tenant": tenant.as_str(), "delivery": delivery, "error": error }),
                );
            }
            Outcome {
                report: JobReport::Succeeded,
                writes: WorkerWrites::default(),
            }
        }
        Ok(answer) => {
            entry.status = answer.status;
            let retry = answer.status >= 500 || answer.status == 408 || answer.status == 429;
            not_delivered(
                app,
                tenant,
                lease,
                &mut entry,
                format!("status {}", answer.status),
                retry,
                now,
            )
        }
        Err(failure) => {
            entry.status = 0;
            let retry = failure.transient();
            not_delivered(
                app,
                tenant,
                lease,
                &mut entry,
                failure.code().to_string(),
                retry,
                now,
            )
        }
    }
}

/// The caller is signed in and holds the capability `name`; the owner holds every capability.
pub(crate) fn require_capability(
    app: &App,
    identity: &Identity,
    name: &str,
) -> Result<(), ApiError> {
    if identity.anonymous() {
        return Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in to do this.",
        ));
    }
    let tenant = crate::ops::header(app, identity)?;
    let wanted =
        std::collections::BTreeSet::from([strata_kernel::Grant::Capability(name.to_string())]);
    if strata_kernel::rules::holds_any(&tenant, &identity.principal, &wanted) {
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
pub(crate) fn same_tenant(app: &App, identity: &Identity, body: &Json) -> Result<(), ApiError> {
    match body.get("tenant").and_then(Json::as_str) {
        Some(named) if named != identity.tenant.as_str() => {
            app.audit(
                "tenant.mismatch",
                &identity.correlation,
                json!({ "tenant": identity.tenant.as_str(), "named": named, "route": "webhooks" }),
            );
            Err(ApiError::new(
                ErrorCode::Forbidden,
                "The request names another site.",
            ))
        }
        _ => Ok(()),
    }
}

/// Create a subscription (create-webhook-subscription): its target's form is judged now, its
/// secret is returned once and kept sealed.
/// Implements: CT-1630
pub fn create_subscription(app: &App, identity: &Identity, body: &Json) -> Result<Json, ApiError> {
    same_tenant(app, identity, body)?;
    require_capability(app, identity, super::WEBHOOKS_MANAGE)?;
    let target = crate::service::text(body, "target", super::rule::MAX_URL_LEN)?;
    if let Err(failure) = super::gate(app).check_form(target) {
        let why = match failure {
            super::Failure::AddressRefused(why) => why,
            other => other.to_string(),
        };
        return Err(ApiError::new(
            ErrorCode::Invalid,
            format!("The address is not allowed: {why}."),
        ));
    }
    let id = super::random_uuid()?;
    let secret = secrets::fresh()?;
    let sealed = secrets::seal_new(app, &subscription_aad(&identity.tenant, &id), &secret)?;
    let subscription = Subscription {
        id: id.clone(),
        target: target.to_string(),
        secrets: sealed,
        created_at: app.now().0,
    };
    let created = write(
        app,
        &identity.tenant,
        SUBSCRIPTION_KIND,
        &id,
        &subscription,
        true,
    )
    .map_err(|e| ApiError::hidden(ErrorCode::Unavailable, e))?;
    if !created {
        return Err(ApiError::hidden(
            ErrorCode::Conflict,
            "a random subscription id was taken",
        ));
    }
    app.audit(
        "webhook.subscription_created",
        &identity.correlation,
        json!({
            "tenant": identity.tenant.as_str(),
            "subscription": id,
            "target": redacted_text(target),
        }),
    );
    Ok(json!({ "subscription": id, "secret": secret }))
}

/// Rotate a subscription's secret (rotate-webhook-subscription-secret): the new secret is returned
/// once; for twenty-four hours deliveries carry a signature under each.
/// Implements: CT-1631
pub fn rotate_subscription(app: &App, identity: &Identity, body: &Json) -> Result<Json, ApiError> {
    same_tenant(app, identity, body)?;
    require_capability(app, identity, super::WEBHOOKS_MANAGE)?;
    let id = crate::service::text(body, "subscription", 64)?;
    let Some(mut held) = read::<Subscription>(app, &identity.tenant, SUBSCRIPTION_KIND, id)
        .map_err(|e| ApiError::hidden(ErrorCode::Unavailable, e))?
    else {
        return Err(ApiError::new(
            ErrorCode::NotFound,
            "There is no such subscription.",
        ));
    };
    let now = app.now().0;
    let (sealed, secret, until) = secrets::rotate(
        app,
        &subscription_aad(&identity.tenant, id),
        &held.secrets,
        now,
    )?;
    held.secrets = sealed;
    write(app, &identity.tenant, SUBSCRIPTION_KIND, id, &held, false)
        .map_err(|e| ApiError::hidden(ErrorCode::Unavailable, e))?;
    app.audit(
        "webhook.subscription_rotated",
        &identity.correlation,
        json!({ "tenant": identity.tenant.as_str(), "subscription": id }),
    );
    Ok(json!({ "secret": secret, "previous_valid_until": until }))
}

/// The most entries one page holds, and how many when the request names no limit.
const MAX_PAGE: u64 = 100;
const DEFAULT_PAGE: u64 = 50;

/// One entry of the listing (type webhook-delivery).
fn summary(entry: &Delivery) -> Json {
    json!({
        "id": entry.id,
        "endpoint": entry.target,
        "subscription": entry.subscription,
        "status": entry.status,
        "attempts": entry.attempts,
        "at": entry.at,
        "state": entry.state.as_str(),
        "outcome": entry.outcome,
    })
}

/// The outbound deliveries of the caller's tenant, newest first, each with its response status
/// and attempt count (list-webhook-deliveries, INT-152). An owner's, holding webhooks-read.
/// Implements: CT-173
pub fn list_deliveries(app: &App, identity: &Identity, body: &Json) -> Result<Json, ApiError> {
    same_tenant(app, identity, body)?;
    crate::service::require_owner(app, identity)?;
    require_capability(app, identity, super::WEBHOOKS_READ)?;
    let limit = body
        .get("limit")
        .and_then(Json::as_u64)
        .unwrap_or(DEFAULT_PAGE)
        .clamp(1, MAX_PAGE) as usize;
    // The cursor is the creation time and the id of the last entry of the previous page.
    let after: Option<(i64, String)> = match body.get("cursor") {
        None | Some(Json::Null) => None,
        Some(cursor) => Some(
            cursor
                .as_str()
                .and_then(|text| text.split_once(':'))
                .and_then(|(at, id)| at.parse::<i64>().ok().map(|at| (at, id.to_string())))
                .ok_or_else(|| {
                    ApiError::new(ErrorCode::Invalid, "The cursor is not one this list gave.")
                })?,
        ),
    };
    let mut entries: Vec<Delivery> = crate::service::list(app, &identity.tenant, DELIVERY_KIND)?;
    entries.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    let page: Vec<&Delivery> = entries
        .iter()
        .filter(|entry| match &after {
            None => true,
            Some((at, id)) => (entry.created_at, &entry.id) < (*at, id),
        })
        .take(limit)
        .collect();
    let next = (page.len() == limit)
        .then(|| page.last().map(|e| format!("{}:{}", e.created_at, e.id)))
        .flatten();
    Ok(json!({
        "deliveries": page.iter().map(|e| summary(e)).collect::<Vec<_>>(),
        "next": next,
    }))
}
