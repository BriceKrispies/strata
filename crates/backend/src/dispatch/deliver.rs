//! The worker's side of the dispatcher: what a job of kind `mail` does (D-451, D-6561).
//!
//! A job for one recipient renders the message, checks it and hands it to the
//! [`crate::mail::Mailer`]; a job for a batch of a segment reads the next hundred members by
//! cursor and creates one job for each of them and one for the batch after, in the report of the
//! batch. Every outcome is recorded in the mail log ([`super::store`]); the message id is the same
//! on every attempt, so a message that reached a final state is never sent again (EML-006).

use super::render::{self, Rendered};
use super::store::{self, MessageRecord};
use super::{links, policy, MailRequest, MessageState, Purpose, Recipient, BATCH};
use crate::app::{App, LOG};
use crate::mail::Mail;
use crate::worker::steps::Outcome;
use serde_json::json;
use strata_kernel::segments::SegmentId;
use strata_kernel::{PrincipalId, TenantId, Timestamp};
use strata_storage_api::jobs::{
    self, ExceptionKind, ExceptionRecord, JobReport, Lease, NewJob, WorkerWrites,
};

/// How long a message held by the complaint breaker waits before it is attempted again.
const BREAKER_RETRY_MS: i64 = 3_600_000;
/// How often the breaker's rates are measured again for a tenant.
const BREAKER_MEASURE_MS: i64 = 60_000;
/// The days the breaker measures over (D-6564).
const BREAKER_WINDOW_MS: i64 = 7 * 86_400_000;

fn succeeded(writes: WorkerWrites) -> Outcome {
    Outcome {
        report: JobReport::Succeeded,
        writes,
    }
}

/// The dead-lettered record of a mail job that spent its attempts.
fn dead_letter(tenant: &TenantId, lease: &Lease, error: &str, now: Timestamp) -> ExceptionRecord {
    let job = &lease.job;
    ExceptionRecord {
        id: jobs::derived_uuid(&format!(
            "strata/mail-exception/v1\0{tenant}\0{}\0{}",
            job.id, job.attempt
        )),
        kind: ExceptionKind::DeadLettered,
        reason: "attempts-exhausted".to_string(),
        trigger: None,
        automation: None,
        run: None,
        step: None,
        event_seq: None,
        job: Some(job.id.clone()),
        hop: None,
        at: now,
        // Ids and the error only, never an address or a body.
        detail: clip(&format!("mail job {}: {error}", job.key), 256),
        settled: false,
        seq: 0,
    }
}

fn clip(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// Save a record, logging a failure: what was sent stays sent.
fn save(app: &App, tenant: &TenantId, record: &MessageRecord) {
    if let Err(error) = store::put_message(app, tenant, record) {
        LOG.error(
            "mail.log_failed",
            "",
            json!({ "tenant": tenant.as_str(), "message": record.message, "error": error.to_string() }),
        );
    }
}

/// A message that cannot succeed: recorded as failed with the reason, attempted no further
/// (EML-008).
fn fail_for_good(
    app: &App,
    tenant: &TenantId,
    record: &mut MessageRecord,
    reason: String,
    now: Timestamp,
) -> Outcome {
    record.state = MessageState::Failed;
    record.fail_reason = Some(jobs::clip_error(&reason));
    record.updated_at = now.0;
    record.final_at = Some(now.0);
    save(app, tenant, record);
    Outcome {
        report: JobReport::Failed {
            error: reason,
            retry: false,
        },
        writes: WorkerWrites::default(),
    }
}

/// An attempt that may succeed later (EML-007): retried by the job's policy; the attempt that
/// spends it fails the message with the reason and dead-letters the job.
fn try_again(
    app: &App,
    tenant: &TenantId,
    lease: &Lease,
    record: Option<&mut MessageRecord>,
    error: String,
    now: Timestamp,
) -> Outcome {
    let report = JobReport::Failed {
        error: error.clone(),
        retry: !lease.exhausted,
    };
    let mut writes = WorkerWrites::default();
    if lease.exhausted || jobs::report_fails_job(&lease.job, &report) {
        writes
            .exceptions
            .push(dead_letter(tenant, lease, &error, now));
        if let Some(record) = record {
            record.state = MessageState::Failed;
            record.fail_reason = Some(jobs::clip_error(&error));
            record.updated_at = now.0;
            record.final_at = Some(now.0);
            save(app, tenant, record);
        }
    } else if let Some(record) = record {
        record.attempts = lease.job.attempt;
        record.updated_at = now.0;
        save(app, tenant, record);
    }
    Outcome { report, writes }
}

/// Run a job of kind `mail` (the request of `JobWork::Mail`).
///
/// Implements: CT-156
pub fn run_job(
    app: &App,
    tenant: &TenantId,
    lease: &Lease,
    request: &serde_json::Value,
    now: Timestamp,
) -> Outcome {
    let request: MailRequest = match serde_json::from_value(request.clone()) {
        Ok(request) => request,
        Err(error) => {
            return Outcome {
                report: JobReport::Failed {
                    error: format!("the mail request cannot be read: {error}"),
                    retry: false,
                },
                writes: WorkerWrites::default(),
            }
        }
    };
    if let Err(error) = store::housekeep(app, tenant, now) {
        LOG.warn(
            "mail.housekeeping_failed",
            "",
            json!({ "tenant": tenant.as_str(), "error": error.to_string() }),
        );
    }
    match &request.recipient {
        Recipient::Segment { segment, after } => {
            fan_out(app, tenant, lease, &request, *segment, after.as_ref(), now)
        }
        _ => deliver(app, tenant, lease, &request, now),
    }
}

/// One batch of a send to a segment (D-6561, EML-1561): the next hundred members after the
/// cursor, read now, in principal order; a job for each, keyed by the send and the person, so
/// no person gets two messages of one send; and the job of the next batch when this one was
/// full. All are created in the report of this batch, together.
fn fan_out(
    app: &App,
    tenant: &TenantId,
    lease: &Lease,
    request: &MailRequest,
    segment: SegmentId,
    after: Option<&PrincipalId>,
    now: Timestamp,
) -> Outcome {
    if lease.exhausted {
        let error = lease.job.errors.last().cloned().unwrap_or_default();
        return try_again(app, tenant, lease, None, error, now);
    }
    let members = match app.store.segment_members(tenant, segment, after, BATCH) {
        Ok(members) => members,
        Err(error) => return try_again(app, tenant, lease, None, error.to_string(), now),
    };
    let mut follow: Vec<NewJob> = members
        .iter()
        .map(|member| {
            let one = MailRequest {
                message: super::message_id(tenant, &request.key, Some(&member.person)),
                recipient: Recipient::Principal {
                    principal: member.person.clone(),
                },
                ..request.clone()
            };
            super::job_of(
                format!("mail:{}:m:{}", request.key, member.person),
                &one,
                now,
            )
        })
        .collect();
    if members.len() == BATCH {
        if let Some(last) = members.last() {
            let next = MailRequest {
                recipient: Recipient::Segment {
                    segment,
                    after: Some(last.person.clone()),
                },
                ..request.clone()
            };
            follow.push(super::job_of(
                format!("mail:{}:b:{}", request.key, last.person),
                &next,
                now,
            ));
        }
    }
    succeeded(WorkerWrites {
        jobs: follow,
        ..WorkerWrites::default()
    })
}

/// The first origin a host of the tenant is bound to, for the links of a message.
fn site_origin(app: &App, tenant: &TenantId) -> Option<String> {
    let bindings = app.store.doc_list(tenant, crate::hosts::KIND).ok()?;
    bindings.iter().find_map(|(_, text)| {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        value.get("origins")?.get(0)?.as_str().map(str::to_string)
    })
}

/// The breaker of the tenant, measured again when its last measurement is a minute old.
fn breaker_now(
    app: &App,
    tenant: &TenantId,
    now: Timestamp,
) -> strata_storage_api::StoreResult<store::BreakerState> {
    let previous = store::breaker(app, tenant)?;
    if now.0 - previous.measured_at < BREAKER_MEASURE_MS {
        return Ok(previous);
    }
    let mut rates = policy::Rates::default();
    for record in store::messages(app, tenant)? {
        if now.0 - record.updated_at > BREAKER_WINDOW_MS || !record.state.accepted() {
            continue;
        }
        rates.accepted += 1;
        match record.state {
            MessageState::Complained => rates.complaints += 1,
            MessageState::Bounced => rates.bounces += 1,
            _ => {}
        }
    }
    let next = policy::breaker(rates, previous, now.0);
    if next.bulk_held != previous.bulk_held
        || next.transactional_held != previous.transactional_held
    {
        LOG.warn(
            "mail.breaker_changed",
            "",
            json!({
                "tenant": tenant.as_str(),
                "bulk_held": next.bulk_held,
                "transactional_held": next.transactional_held,
            }),
        );
    }
    store::put_breaker(app, tenant, &next)?;
    Ok(next)
}

/// Hold a message until `due`: recorded as held, and a job that attempts it again then, keyed by
/// the message and `bucket`, so a hold makes one later attempt (EML-1563, EML-1564).
fn hold(
    app: &App,
    tenant: &TenantId,
    request: &MailRequest,
    record: &mut MessageRecord,
    bucket: &str,
    due: i64,
    now: Timestamp,
) -> Outcome {
    record.state = MessageState::Held;
    record.updated_at = now.0;
    save(app, tenant, record);
    succeeded(WorkerWrites {
        jobs: vec![super::job_of(
            format!("mail-held:{}:{bucket}", request.message),
            request,
            Timestamp(due),
        )],
        ..WorkerWrites::default()
    })
}

/// Send one message.
fn deliver(
    app: &App,
    tenant: &TenantId,
    lease: &Lease,
    request: &MailRequest,
    now: Timestamp,
) -> Outcome {
    let mut record = match store::message(app, tenant, &request.message) {
        Ok(Some(record)) => record,
        Ok(None) => MessageRecord {
            message: request.message.clone(),
            key: request.key.clone(),
            recipient: match &request.recipient {
                Recipient::Principal { principal } => Some(principal.clone()),
                _ => None,
            },
            address: None,
            address_hash: None,
            template: Some(request.template.clone()),
            state: MessageState::Queued,
            attempts: 0,
            provider_reference: None,
            fail_reason: None,
            category: request.category.clone(),
            class: request.class,
            send: Some(request.send.clone()),
            reference: request.reference.clone(),
            counted: None,
            created_at: now.0,
            updated_at: now.0,
            final_at: None,
        },
        Err(error) => return try_again(app, tenant, lease, None, error.to_string(), now),
    };
    // A message with its outcome is not sent again, whatever repeats its job (EML-006).
    if record.state.is_final() {
        return succeeded(WorkerWrites::default());
    }
    if lease.exhausted {
        let error = lease
            .job
            .errors
            .last()
            .cloned()
            .unwrap_or_else(|| "the lease expired".to_string());
        return try_again(app, tenant, lease, Some(&mut record), error, now);
    }
    record.attempts = lease.job.attempt;

    let address = match &request.recipient {
        Recipient::Address { address } => address.clone(),
        Recipient::Principal { principal } => match app.store.verified_address(tenant, principal) {
            Ok(Some(address)) => address,
            Ok(None) => {
                return fail_for_good(
                    app,
                    tenant,
                    &mut record,
                    format!("principal {principal} has no verified address"),
                    now,
                )
            }
            Err(error) => {
                return try_again(
                    app,
                    tenant,
                    lease,
                    Some(&mut record),
                    error.to_string(),
                    now,
                )
            }
        },
        Recipient::Segment { .. } => {
            return fail_for_good(
                app,
                tenant,
                &mut record,
                "a segment is not one recipient".into(),
                now,
            )
        }
    };
    if let Some(problem) = render::address_problem(&address) {
        return fail_for_good(
            app,
            tenant,
            &mut record,
            format!("invalid: the recipient address {problem}"),
            now,
        );
    }
    let hash = store::address_hash(&address);
    record.address = Some(address.clone());
    record.address_hash = Some(hash.clone());

    let schema = match app.store.load(tenant, &[]) {
        Ok(state) => match state.tenant {
            Some(found) => found.schema.clone(),
            None => {
                return fail_for_good(app, tenant, &mut record, format!("no tenant {tenant}"), now)
            }
        },
        Err(error) => {
            return try_again(
                app,
                tenant,
                lease,
                Some(&mut record),
                error.to_string(),
                now,
            )
        }
    };
    let Some(decl) = schema
        .mail_templates
        .as_ref()
        .and_then(|all| all.get(&request.template))
    else {
        return fail_for_good(
            app,
            tenant,
            &mut record,
            format!("the mail template {} does not exist", request.template),
            now,
        );
    };
    let mut rendered: Rendered = render::render(decl, &request.values);
    if let Some(problem) = render::header_problem(&rendered.subject) {
        return fail_for_good(
            app,
            tenant,
            &mut record,
            format!("invalid: the rendered subject {problem}"),
            now,
        );
    }
    let purpose = match &request.category {
        Some(name) => match store::category(app, tenant, name) {
            Ok(Some(category)) => Some(category.purpose),
            Ok(None) => {
                return fail_for_good(
                    app,
                    tenant,
                    &mut record,
                    format!("the message category {name} does not exist"),
                    now,
                )
            }
            Err(error) => {
                return try_again(
                    app,
                    tenant,
                    lease,
                    Some(&mut record),
                    error.to_string(),
                    now,
                )
            }
        },
        None => None,
    };

    // The suppression of the category (EML-018, EML-1562): recorded and left unsent.
    match store::suppressions_of(app, tenant, &hash, request.category.as_deref()) {
        Ok(entries) => {
            if policy::suppressed(&entries, request.category.as_deref(), purpose) {
                record.state = MessageState::Suppressed;
                record.updated_at = now.0;
                record.final_at = Some(now.0);
                save(app, tenant, &record);
                return succeeded(WorkerWrites::default());
            }
        }
        Err(error) => {
            return try_again(
                app,
                tenant,
                lease,
                Some(&mut record),
                error.to_string(),
                now,
            )
        }
    }

    // The complaint breaker (EML-1564), then the allowance of the class (EML-1563). A message is
    // counted once, in the month it is first released.
    match breaker_now(app, tenant, now) {
        Ok(state) if policy::breaker_holds(&state, request.class) => {
            let bucket = format!("breaker:{}", now.0 / BREAKER_RETRY_MS);
            let due = now.0 + BREAKER_RETRY_MS;
            return hold(app, tenant, request, &mut record, &bucket, due, now);
        }
        Ok(_) => {}
        Err(error) => {
            return try_again(
                app,
                tenant,
                lease,
                Some(&mut record),
                error.to_string(),
                now,
            )
        }
    }
    if record.counted.is_none() {
        let period = policy::period(now.0);
        let counted = store::usage(app, tenant, &period).and_then(|usage| {
            let limit = store::allowance_limit(app, tenant)?;
            Ok((usage, limit))
        });
        match counted {
            Ok((usage, limit)) if policy::allowance_holds(request.class, usage, limit) => {
                let due = policy::next_period_start(now.0);
                return hold(app, tenant, request, &mut record, &period, due, now);
            }
            Ok(_) => {
                if let Err(error) = store::count_use(app, tenant, &period, request.class) {
                    return try_again(
                        app,
                        tenant,
                        lease,
                        Some(&mut record),
                        error.to_string(),
                        now,
                    );
                }
                record.counted = Some(period);
            }
            Err(error) => {
                return try_again(
                    app,
                    tenant,
                    lease,
                    Some(&mut record),
                    error.to_string(),
                    now,
                )
            }
        }
    }

    // A promotional message carries the unsubscribe header and link (EML-020).
    let mut headers = vec![(
        "X-Strata-Message".to_string(),
        store::provider_reference(tenant, &record.message),
    )];
    if purpose == Some(Purpose::Promotional) {
        if let Some(category) = &request.category {
            let origin = site_origin(app, tenant);
            let page =
                links::unsubscribe_url(app, tenant, origin.as_deref(), &hash, category, false);
            let one_click =
                links::unsubscribe_url(app, tenant, origin.as_deref(), &hash, category, true);
            render::with_link(&mut rendered, "To stop receiving these messages:", &page);
            headers.push(("List-Unsubscribe".to_string(), format!("<{one_click}>")));
            headers.push((
                "List-Unsubscribe-Post".to_string(),
                "List-Unsubscribe=One-Click".to_string(),
            ));
        }
    }
    record.state = MessageState::Submitted;
    record.updated_at = now.0;
    save(app, tenant, &record);
    let mail = Mail {
        to: address,
        subject: rendered.subject,
        text: rendered.text,
        html: Some(rendered.html),
        headers,
    };
    match app.cfg.mail.send(&mail) {
        Ok(()) => {
            // The log keeps the state and the recipient; no body was ever written to it (EML-011).
            record.state = MessageState::Sent;
            record.provider_reference = Some(store::provider_reference(tenant, &record.message));
            record.fail_reason = None;
            record.updated_at = now.0;
            record.final_at = Some(now.0);
            save(app, tenant, &record);
            succeeded(WorkerWrites::default())
        }
        Err(error) if error.permanent() => fail_for_good(app, tenant, &mut record, error.0, now),
        Err(error) => try_again(app, tenant, lease, Some(&mut record), error.0, now),
    }
}
