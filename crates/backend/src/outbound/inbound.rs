//! Inbound webhook endpoints (rec-webhook-endpoint, inbound-signature-flow): an owner creates an
//! endpoint for a workflow and rotates its secret, and an external system delivers to it to start
//! a run (INT-150, INT-151, INT-1577, INT-1578, D-6569).
//!
//! A delivery, in order:
//!
//! 1. A body over one mebibyte is refused with `too-large`.
//! 2. The delivery is counted against 600 a minute for its endpoint and 600 a minute for its source
//!    address (INT-1577, q-receive-inbound-webhook); past either it is refused with `throttled`.
//! 3. The signature is checked over the event identifier, the timestamp and the raw body, in
//!    constant time, against every secret of the endpoint that verifies now: the current one and,
//!    for twenty-four hours after a rotation, the previous one (INT-2640, INT-1578). An unknown
//!    endpoint is checked against a decoy secret, so that it costs the same, and a missing or
//!    malformed header, an unknown endpoint and a wrong signature get one answer, `unauthenticated`
//!    (INT-1577).
//! 4. A timestamp more than five minutes from the receipt time is refused with `stale-delivery`.
//! 5. The event identifier is remembered for seven days: a repeat within them starts nothing and
//!    answers the run the first delivery started (INT-151). The remembering is one create-only
//!    write, so two deliveries of one event that race are one run.
//! 6. The run is handed to the automation worker as a start job (the receiver of
//!    start-workflow-run), keyed by the endpoint, the event and the time the event was first seen,
//!    with the event, the endpoint and the body (when it is text) as its parameters
//!    `webhook_event`, `webhook_endpoint` and `webhook_body`. The run's id is the job's id.
//!
//! Implements: CT-172, CT-832, CT-833, INT-150, INT-151, INT-1577, INT-1578, H-022

use super::deliveries::{require_capability, same_tenant};
use super::secrets::{self, SealedSecrets};
use super::signature;
use crate::app::{ApiError, App};
use crate::auth::Identity;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use strata_kernel::{canon, TenantId, Timestamp, Value, WorkflowId};
use strata_protocol::api::ErrorCode;
use strata_storage_api::jobs::{self, JobWork, NewJob, RetryPolicy};
use strata_storage_api::{ThrottleRule, ThrottleVerdict};

/// The document kind of an endpoint.
pub const ENDPOINT_KIND: &str = "webhook-endpoint";
/// The document kind of a remembered event identifier.
pub const SEEN_KIND: &str = "webhook-seen";

/// The most a delivery's body may hold: one mebibyte (INT-1577).
pub const MAX_BODY: usize = 1024 * 1024;
/// How far a delivery's timestamp may be from the receipt time: five minutes (INT-1577, D-6569).
pub const TOLERANCE_SECS: i64 = 300;
/// How long an event identifier is remembered: seven days (INT-1577, D-6569).
pub const DEDUPLICATION_MS: i64 = 7 * 24 * 3_600_000;
/// Deliveries one endpoint, and one source, may make in a minute (INT-1577).
pub const PER_MINUTE: u32 = 600;
/// The longest event identifier accepted.
pub const MAX_EVENT_ID: usize = 128;

/// The one answer for an unknown endpoint, a missing or malformed header and a wrong signature.
const REFUSAL: &str = "The delivery was not accepted.";

/// One endpoint: the workflow it starts and its sealed secrets.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Endpoint {
    pub endpoint: String,
    pub workflow: String,
    pub secrets: SealedSecrets,
    pub version: u64,
    pub created_at: i64,
}

/// A remembered event: when it was first seen and the run it started.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Seen {
    at: i64,
    run: String,
}

/// One delivery as it arrived: the endpoint it names, its raw body and its three headers.
#[derive(Clone, Debug)]
pub struct InboundDelivery<'a> {
    pub endpoint: &'a str,
    pub body: &'a [u8],
    pub event: &'a str,
    pub timestamp: &'a str,
    pub signature: &'a str,
}

/// The associated data an endpoint's secrets are sealed with.
fn endpoint_aad(tenant: &TenantId, endpoint: &str) -> Vec<u8> {
    format!("strata/webhook-endpoint/v1\0{tenant}\0{endpoint}").into_bytes()
}

/// An endpoint's name: 1 to 64 lower-case letters, digits and hyphens, starting with a letter or
/// a digit.
pub fn endpoint_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn refusal(code: ErrorCode, name: &str, message: &str) -> ApiError {
    let mut error = ApiError::new(code, message);
    error.0.kind = Some(name.to_string());
    error
}

fn unauthenticated() -> ApiError {
    refusal(ErrorCode::Unauthenticated, "unauthenticated", REFUSAL)
}

fn stored(error: impl std::fmt::Display) -> ApiError {
    ApiError::hidden(ErrorCode::Unavailable, error.to_string())
}

fn read_endpoint(app: &App, tenant: &TenantId, name: &str) -> Result<Option<Endpoint>, ApiError> {
    crate::service::get(app, tenant, ENDPOINT_KIND, name)
}

/// Create an endpoint for a workflow of the tenant (create-webhook-endpoint): its secret is 32
/// random bytes, returned once and kept sealed (INT-1578).
/// Implements: CT-832
pub fn create_endpoint(app: &App, identity: &Identity, body: &Json) -> Result<Json, ApiError> {
    same_tenant(app, identity, body)?;
    require_capability(app, identity, super::WEBHOOKS_MANAGE)?;
    let name = crate::service::text(body, "endpoint", 64)?;
    if !endpoint_name_ok(name) {
        return Err(ApiError::new(
            ErrorCode::Invalid,
            "An endpoint name is 1 to 64 lower-case letters, digits and hyphens.",
        ));
    }
    let workflow = WorkflowId::new(crate::service::text(body, "workflow", 64)?)
        .map_err(|_| ApiError::new(ErrorCode::Invalid, "workflow is not valid."))?;
    let tenant = crate::ops::header(app, identity)?;
    let defined = tenant
        .schema
        .automations
        .as_ref()
        .is_some_and(|all| all.contains_key(&workflow));
    if !defined {
        return Err(ApiError::new(
            ErrorCode::NotFound,
            "The workflow does not exist.",
        ));
    }
    let secret = secrets::fresh()?;
    let endpoint = Endpoint {
        endpoint: name.to_string(),
        workflow: workflow.as_str().to_string(),
        secrets: secrets::seal_new(app, &endpoint_aad(&identity.tenant, name), &secret)?,
        version: 1,
        created_at: app.now().0,
    };
    if !crate::service::put(app, &identity.tenant, ENDPOINT_KIND, name, &endpoint, true)? {
        return Err(ApiError::new(
            ErrorCode::Conflict,
            "The endpoint name is in use.",
        ));
    }
    app.audit(
        "webhook.endpoint_created",
        &identity.correlation,
        json!({
            "tenant": identity.tenant.as_str(),
            "endpoint": name,
            "workflow": workflow.as_str(),
        }),
    );
    Ok(json!({ "secret": secret, "version": endpoint.version }))
}

/// Rotate an endpoint's secret (rotate-webhook-secret): the new secret is returned once, the
/// previous one verifies for twenty-four hours more, and the one before it is retired now
/// (INT-1578).
/// Implements: CT-833
pub fn rotate_endpoint(app: &App, identity: &Identity, body: &Json) -> Result<Json, ApiError> {
    same_tenant(app, identity, body)?;
    require_capability(app, identity, super::WEBHOOKS_MANAGE)?;
    let name = crate::service::text(body, "endpoint", 64)?;
    let Some(mut endpoint) = read_endpoint(app, &identity.tenant, name)? else {
        return Err(ApiError::new(
            ErrorCode::NotFound,
            "There is no such endpoint.",
        ));
    };
    let (sealed, secret, until) = secrets::rotate(
        app,
        &endpoint_aad(&identity.tenant, name),
        &endpoint.secrets,
        app.now().0,
    )?;
    endpoint.secrets = sealed;
    endpoint.version = endpoint.version.saturating_add(1);
    crate::service::put(app, &identity.tenant, ENDPOINT_KIND, name, &endpoint, false)?;
    app.audit(
        "webhook.secret_rotated",
        &identity.correlation,
        json!({ "tenant": identity.tenant.as_str(), "endpoint": name, "version": endpoint.version }),
    );
    Ok(json!({ "secret": secret, "previous_valid_until": until }))
}

/// The counter keys of an endpoint and of a source, bounded in length whatever was sent.
fn counter(kind: &str, value: &str) -> String {
    format!(
        "webhook-in:{kind}:{}",
        &canon::hash_bytes(value.as_bytes())[..32]
    )
}

/// Receive one delivery for `tenant` from `source` at `now` (receive-inbound-webhook). Answers the
/// run the delivery started, or the run a delivery of the same event started before it.
/// Implements: CT-172
pub fn receive(
    app: &App,
    tenant: &TenantId,
    source: Option<&str>,
    delivery: &InboundDelivery<'_>,
    now: Timestamp,
) -> Result<Json, ApiError> {
    if delivery.body.len() > MAX_BODY {
        return Err(refusal(
            ErrorCode::Invalid,
            "too-large",
            "The delivery's body is larger than one mebibyte.",
        ));
    }
    let per_minute = |key: String| ThrottleRule {
        key,
        max: PER_MINUTE,
        window_ms: 60_000,
        lock_ms: 60_000,
        lock_resets: false,
    };
    // Per source, the quota of the specification: the generated model is the one source of it.
    let quota = &strata_model::quotas::Q_RECEIVE_INBOUND_WEBHOOK;
    let rules = [
        per_minute(counter("endpoint", delivery.endpoint)),
        ThrottleRule {
            key: counter("source", source.unwrap_or("unknown")),
            max: u32::try_from(quota.limit).unwrap_or(PER_MINUTE),
            window_ms: i64::try_from(quota.window.as_millis()).unwrap_or(60_000),
            lock_ms: i64::try_from(quota.window.as_millis()).unwrap_or(60_000),
            lock_resets: false,
        },
    ];
    if let ThrottleVerdict::Refused { .. } = app.store.throttle_reserve(tenant, &rules, now)? {
        return Err(refusal(
            ErrorCode::Throttled,
            "throttled",
            "Too many deliveries. Try again shortly.",
        ));
    }
    // The headers: a timestamp of whole seconds and a visible event identifier.
    let timestamp = delivery
        .timestamp
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|_| delivery.timestamp.trim().len() <= 12);
    let event_ok = !delivery.event.is_empty()
        && delivery.event.len() <= MAX_EVENT_ID
        && delivery.event.bytes().all(|b| (0x21..=0x7e).contains(&b));
    let held = if endpoint_name_ok(delivery.endpoint) {
        read_endpoint(app, tenant, delivery.endpoint)?
    } else {
        None
    };
    let keys = match &held {
        Some(endpoint) => secrets::live(
            app,
            &endpoint_aad(tenant, delivery.endpoint),
            &endpoint.secrets,
            now.0,
        ),
        // An unknown endpoint is checked against a decoy, so it costs what a known one does.
        None => vec![b"strata/webhook-endpoint/decoy".to_vec()],
    };
    let verified = signature::verify(
        &keys,
        delivery.event,
        timestamp.unwrap_or(0),
        delivery.body,
        delivery.signature,
    );
    let (Some(endpoint), Some(timestamp), true, true) = (held, timestamp, event_ok, verified)
    else {
        log_refused(tenant, delivery.endpoint);
        return Err(unauthenticated());
    };
    if (now.0.div_euclid(1000) - timestamp).abs() > TOLERANCE_SECS {
        return Err(refusal(
            ErrorCode::Invalid,
            "stale-delivery",
            "The delivery's timestamp is more than five minutes from now.",
        ));
    }
    let workflow = WorkflowId::new(endpoint.workflow.as_str())
        .map_err(|_| ApiError::hidden(ErrorCode::Internal, "an endpoint's workflow is invalid"))?;
    // Remember the event, once: the first delivery of it wins the create-only write.
    let seen_key = canon::hash_bytes(
        format!(
            "strata/webhook-seen/v1\0{}\0{}",
            delivery.endpoint, delivery.event
        )
        .as_bytes(),
    );
    let first_seen = |at: i64| -> (String, String) {
        let job_key = format!(
            "webhook:{}",
            &canon::hash_bytes(
                format!(
                    "strata/webhook-run/v1\0{}\0{}\0{at}",
                    delivery.endpoint, delivery.event
                )
                .as_bytes()
            )[..48]
        );
        let run = jobs::job_id(tenant, &job_key);
        (job_key, run)
    };
    let (job_key, run, repeat) = {
        let (job_key, run) = first_seen(now.0);
        let fresh = Seen {
            at: now.0,
            run: run.clone(),
        };
        let text = serde_json::to_string(&fresh).map_err(stored)?;
        if app
            .store
            .doc_put(tenant, SEEN_KIND, &seen_key, &text, true)?
        {
            (job_key, run, false)
        } else {
            let earlier: Option<Seen> = crate::service::get(app, tenant, SEEN_KIND, &seen_key)?;
            match earlier {
                Some(earlier) if now.0 - earlier.at < DEDUPLICATION_MS => {
                    let (job_key, _) = first_seen(earlier.at);
                    (job_key, earlier.run, true)
                }
                // Seen more than seven days ago: a new delivery of the event.
                _ => {
                    app.store
                        .doc_put(tenant, SEEN_KIND, &seen_key, &text, false)?;
                    (job_key, run, false)
                }
            }
        }
    };
    let mut params = BTreeMap::from([
        (
            "webhook_event".to_string(),
            Value::Str(delivery.event.to_string()),
        ),
        (
            "webhook_endpoint".to_string(),
            Value::Str(delivery.endpoint.to_string()),
        ),
    ]);
    if let Ok(text) = std::str::from_utf8(delivery.body) {
        params.insert("webhook_body".to_string(), Value::Str(text.to_string()));
    }
    // The person the automation acts for: its definer (D-7204).
    let on_behalf_of = app
        .store
        .load(tenant, &[])
        .ok()
        .and_then(|state| state.tenant)
        .and_then(|found| {
            found
                .schema
                .automations
                .as_ref()
                .and_then(|all| all.get(&workflow))
                .and_then(|def| def.defined_by.clone())
        });
    let job = NewJob {
        key: job_key,
        work: JobWork::StartRun {
            trigger: None,
            automation: workflow,
            event_seq: None,
            hop: 0,
            params,
            caused_by: None,
            on_behalf_of,
            definition_version: None,
            run_key: None,
        },
        due_at: now,
        policy: RetryPolicy::default(),
    };
    // Created once for its key: a repeat within the seven days makes sure the job exists (a
    // crash between remembering the event and creating the job is healed by the sender's retry)
    // and starts nothing more.
    app.store.jobs_create(tenant, &[job], now)?;
    crate::app::LOG.info(
        "webhook.received",
        "",
        json!({
            "tenant": tenant.as_str(),
            "endpoint": delivery.endpoint,
            "run": run,
            "repeat": repeat,
            "bytes": delivery.body.len(),
        }),
    );
    Ok(json!({ "run": run, "repeat": repeat }))
}

/// A refused delivery is logged with its endpoint name and nothing it carried.
fn log_refused(tenant: &TenantId, endpoint: &str) {
    let shown: String = endpoint.chars().take(64).collect();
    crate::app::LOG.info(
        "webhook.refused",
        "",
        json!({ "tenant": tenant.as_str(), "endpoint": shown }),
    );
}
