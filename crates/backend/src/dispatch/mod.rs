//! The mail dispatcher (component `mail-dispatcher`, D-451): the one place mail leaves the
//! platform, for the sign-in mail of the identity service and for the templated mail a product
//! or an automation sends (EML-001 to EML-021, EML-1560 to EML-1567, D-6560 to D-6573).
//!
//! - **A send is a job** ([`mail_job`], job kind `mail`, [`JobWork::Mail`]): created in the
//!   transaction of the change it belongs to when that change goes through a commit
//!   ([`strata_storage_api::Commit::jobs`]), or alone when the send is the change (the route
//!   send-message and the automation step send-mail). Its key is derived from the sender's
//!   idempotency key, so a repeat names the same job and sends once (EML-005, EML-006, EML-1560).
//! - **The worker delivers it** ([`deliver::run_job`]): it renders the template with the values
//!   ([`render`]), checks the suppression of the category, the complaint breaker and the
//!   allowance of the class ([`policy`]), hands the message to the [`crate::mail::Mailer`] and
//!   records the outcome in the mail log; a temporary refusal is retried by the job's policy and
//!   the last one dead-letters the job (EML-007, EML-008). A send to a segment is cut into
//!   batches of one hundred persons by cursor, each batch its own job (D-6561, EML-1561).
//! - **Sign-in mail is sent at once** ([`send_sign_in`]): the identity service hands its link
//!   and recovery messages over synchronously, as before, and the dispatcher records them in the
//!   log as class sign-in; they are never held (EML-021, EML-1563).
//! - **Suppression is per category** (D-6562, EML-1562): unsubscribe and resubscribe by a signed
//!   link ([`links`]), owner management, and the effects of a delivery report ([`reports`]).
//! - **Storage** is a separate block, [`store`]: the mail log, the suppression list, the
//!   allowance counters, the categories and the seen delivery reports, each a kind of document
//!   of the tenant's own database.
//!
//! The specification states no schema for the blocks of a template and no syntax that places a
//! value in one (WG-5177). Until it does, the blocks are rendered as paragraphs of text and a
//! value is placed where the template writes `{{name}}` ([`render`]).

pub mod adapter;
pub mod deliver;
pub mod links;
pub mod policy;
pub mod render;
pub mod reports;
pub mod routes;
pub mod store;

use crate::app::{ApiError, App, LOG};
use crate::mail::{Mail, MailError};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::automation::automation_principal;
use strata_kernel::rules::holds_any;
use strata_kernel::segments::SegmentId;
use strata_kernel::{canon, Grant, Principal, PrincipalId, TemplateId, TenantId, Timestamp, Value};
use strata_protocol::api::ErrorCode;
use strata_storage_api::jobs::{self, JobWork, NewJob, RetryPolicy};

/// The capability a send needs (send-message, EML-1560).
pub const MESSAGES_SEND: &str = "messages-send";
/// The capability that reads the mail log (list-mail-messages, count-mail-messages, EML-010).
pub const MAIL_READ: &str = "mail-read";
/// The capability that declares categories and manages the suppression list.
pub const MAIL_CONFIGURE: &str = "mail-configure";

/// The persons of one batch of a send to a segment (D-6561, EML-1561, SEG-028).
pub const BATCH: usize = 100;

/// The class of a message for the allowance of its tenant (spec type `allowance-class`,
/// D-6563).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AllowanceClass {
    /// Set by the platform only; outside the allowance and never held.
    SignIn,
    Transactional,
    Bulk,
}

impl AllowanceClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            AllowanceClass::SignIn => "sign-in",
            AllowanceClass::Transactional => "transactional",
            AllowanceClass::Bulk => "bulk",
        }
    }

    pub fn parse(text: &str) -> Option<AllowanceClass> {
        Some(match text {
            "sign-in" => AllowanceClass::SignIn,
            "transactional" => AllowanceClass::Transactional,
            "bulk" => AllowanceClass::Bulk,
            _ => return None,
        })
    }
}

/// What a category is for (spec type `message-category-purpose`, D-6562).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Purpose {
    Required,
    Transactional,
    Promotional,
}

impl Purpose {
    pub fn as_str(&self) -> &'static str {
        match self {
            Purpose::Required => "required",
            Purpose::Transactional => "transactional",
            Purpose::Promotional => "promotional",
        }
    }

    pub fn parse(text: &str) -> Option<Purpose> {
        Some(match text {
            "required" => Purpose::Required,
            "transactional" => Purpose::Transactional,
            "promotional" => Purpose::Promotional,
            _ => return None,
        })
    }
}

/// The state of a message (spec type `message-state`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MessageState {
    Queued,
    Submitted,
    Sent,
    Failed,
    Discarded,
    Held,
    Suppressed,
    Delivered,
    Bounced,
    Complained,
}

impl MessageState {
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageState::Queued => "queued",
            MessageState::Submitted => "submitted",
            MessageState::Sent => "sent",
            MessageState::Failed => "failed",
            MessageState::Discarded => "discarded",
            MessageState::Held => "held",
            MessageState::Suppressed => "suppressed",
            MessageState::Delivered => "delivered",
            MessageState::Bounced => "bounced",
            MessageState::Complained => "complained",
        }
    }

    pub fn parse(text: &str) -> Option<MessageState> {
        Some(match text {
            "queued" => MessageState::Queued,
            "submitted" => MessageState::Submitted,
            "sent" => MessageState::Sent,
            "failed" => MessageState::Failed,
            "discarded" => MessageState::Discarded,
            "held" => MessageState::Held,
            "suppressed" => MessageState::Suppressed,
            "delivered" => MessageState::Delivered,
            "bounced" => MessageState::Bounced,
            "complained" => MessageState::Complained,
            _ => return None,
        })
    }

    /// Whether the message has its outcome: it is never attempted again. A held message is not
    /// final: it is attempted again when its hold ends.
    pub fn is_final(&self) -> bool {
        !matches!(
            self,
            MessageState::Queued | MessageState::Submitted | MessageState::Held
        )
    }

    /// Whether the provider accepted the message (what the complaint breaker counts, D-6564).
    pub fn accepted(&self) -> bool {
        matches!(
            self,
            MessageState::Sent
                | MessageState::Delivered
                | MessageState::Bounced
                | MessageState::Complained
        )
    }
}

/// Who a send is for (spec type `send-recipient`, D-5020).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Recipient {
    /// A principal of the tenant: the address is the verified address of its account, read when
    /// the message is sent.
    Principal { principal: PrincipalId },
    /// One address.
    Address { address: String },
    /// Every member of a segment, from the person after `after` on, in principal order.
    Segment {
        segment: SegmentId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<PrincipalId>,
    },
}

/// What the job of a send carries (the request of [`JobWork::Mail`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailRequest {
    /// The message id (a UUID): of the one message, or for a batch of a segment the send id.
    pub message: String,
    /// The SHA-256 of the sender's idempotency key, from which every job key and message id of
    /// the send is derived.
    pub key: String,
    pub template: TemplateId,
    pub recipient: Recipient,
    /// Token values, as text (EML-016).
    #[serde(default)]
    pub values: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub class: AllowanceClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// The send id (a UUID) every message of the send carries (EML-1565).
    pub send: String,
    /// The SHA-256 of the inputs: a repeat of the key with other inputs is a conflict (EML-1560).
    pub inputs: String,
}

/// A send as a product or an automation asks for it (the inputs of send-message, channel mail).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendInput {
    pub template: TemplateId,
    pub recipient: Recipient,
    pub category: Option<String>,
    /// Absent: transactional for a required or transactional category, or none; bulk for a
    /// promotional one. Sign-in is refused.
    pub class: Option<AllowanceClass>,
    pub idempotency_key: String,
    pub values: BTreeMap<String, String>,
    pub reference: Option<String>,
}

/// What a send answers (send-progress and recipients of send-message).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Submitted {
    /// The send id.
    pub send: String,
    /// The message id of a send to a principal or an address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The batches the send was cut into when it was accepted (EML-1561).
    pub batches: u32,
    /// The recipients, known at once for a principal or an address; for a segment, its members
    /// when the send was accepted.
    pub recipients: u32,
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Invalid, message)
}

fn not_found(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::NotFound, message)
}

/// The SHA-256 of an idempotency key, in a domain of its own.
pub fn key_hash(key: &str) -> String {
    canon::hash_bytes(format!("strata/mail-key/v1\0{key}").as_bytes())
}

/// The job key of the first job of a send.
pub fn job_key(key_hash: &str) -> String {
    format!("mail:{key_hash}")
}

/// The id of the message of a send to one recipient (`member` none) or of one member of a send
/// to a segment.
pub fn message_id(tenant: &TenantId, key_hash: &str, member: Option<&PrincipalId>) -> String {
    let member = member.map(PrincipalId::as_str).unwrap_or("");
    jobs::derived_uuid(&format!(
        "strata/mail-message/v1\0{tenant}\0{key_hash}\0{member}"
    ))
}

/// The id of a send.
pub fn send_id(tenant: &TenantId, key_hash: &str) -> String {
    jobs::derived_uuid(&format!("strata/mail-send/v1\0{tenant}\0{key_hash}"))
}

/// The retry policy of a mail job: the defaults of D-6529.
pub fn mail_policy() -> RetryPolicy {
    RetryPolicy::default()
}

/// The job that carries `request`, keyed `key`, due at `due`.
pub fn job_of(key: String, request: &MailRequest, due: Timestamp) -> NewJob {
    NewJob {
        key,
        work: JobWork::Mail {
            request: serde_json::to_value(request).unwrap_or(serde_json::Value::Null),
        },
        due_at: due,
        policy: mail_policy(),
    }
}

/// Whether `reference` is a send reference (spec type `send-reference`): 1 to 64 bytes of
/// letters, digits and `. _ : -`, beginning with a letter or a digit.
pub fn valid_reference(reference: &str) -> bool {
    reference.len() <= 64
        && reference
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && reference
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

/// Whether `name` is a category id (spec type `message-category-id`): lower-case letters, digits
/// and hyphens, beginning with a letter, at most 48 bytes, and not one of the reserved names.
pub fn valid_category(name: &str) -> bool {
    name.len() <= 48
        && !matches!(name, "all" | "promotional")
        && name.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Check a send and make its job, without creating it: the template and the category exist, the
/// class is not sign-in, the recipient and the rendered subject hold no line break and one
/// address only (EML-1566, refused before any job exists). Returns the job and the request.
///
/// The job goes into the commit of the change the send belongs to ([`strata_storage_api::Commit`]
/// `jobs`), or is created alone by [`submit`].
pub fn mail_job(
    app: &App,
    tenant: &TenantId,
    input: &SendInput,
    now: Timestamp,
) -> Result<(NewJob, MailRequest), ApiError> {
    if input.class == Some(AllowanceClass::SignIn) {
        return Err(invalid(
            "The class sign-in is the platform's; a send is transactional or bulk.",
        ));
    }
    if !strata_storage_api::valid_idempotency_key(&input.idempotency_key) {
        return Err(invalid(
            "An idempotency key is 1 to 128 visible ASCII characters.",
        ));
    }
    if let Some(reference) = &input.reference {
        if !valid_reference(reference) {
            return Err(invalid(
                "A reference is at most 64 letters, digits and . _ : -.",
            ));
        }
    }
    let state = app.store.load(tenant, &[])?;
    let Some(found) = state.tenant else {
        return Err(not_found(format!("no site {tenant}")));
    };
    let decl = found
        .schema
        .mail_templates
        .as_ref()
        .and_then(|all| all.get(&input.template))
        .cloned()
        .ok_or_else(|| not_found(format!("There is no mail template {}.", input.template)))?;
    let purpose = match &input.category {
        Some(name) => store::category(app, tenant, name)?
            .map(|c| c.purpose)
            .ok_or_else(|| not_found(format!("There is no message category {name}.")))?,
        None => Purpose::Transactional,
    };
    let class = input.class.unwrap_or(match purpose {
        Purpose::Promotional => AllowanceClass::Bulk,
        Purpose::Required | Purpose::Transactional => AllowanceClass::Transactional,
    });
    match &input.recipient {
        Recipient::Address { address } => {
            if let Some(problem) = render::address_problem(address) {
                return Err(invalid(format!("The recipient address {problem}.")));
            }
        }
        Recipient::Principal { .. } => {}
        Recipient::Segment { segment, after } => {
            if after.is_some() {
                return Err(invalid("A send to a segment starts at its first member."));
            }
            let known = app
                .store
                .segment_catalog(tenant)?
                .iter()
                .any(|s| s.id == *segment);
            if !known {
                return Err(not_found(format!("There is no segment {segment}.")));
            }
        }
    }
    // A body that does not render (a value it places is missing, a token has no text) is refused
    // before any job exists, as a header break is.
    let tokens = |name: &str| {
        found
            .schema
            .tokens
            .as_ref()
            .and_then(|all| all.get(name))
            .map(|token| token.fallback.clone())
    };
    let rendered = render::render(&decl, &input.values, &tokens).map_err(|problem| {
        invalid(format!(
            "The template {} does not render: {problem}.",
            input.template
        ))
    })?;
    if let Some(problem) = render::header_problem(&rendered.subject) {
        return Err(invalid(format!("The rendered subject {problem}.")));
    }
    let key = key_hash(&input.idempotency_key);
    let inputs = canon::hash_bytes(
        serde_json::to_string(&json!({
            "template": input.template.as_str(),
            "recipient": input.recipient,
            "category": input.category,
            "class": class,
            "values": input.values,
            "reference": input.reference,
        }))
        .unwrap_or_default()
        .as_bytes(),
    );
    let single = !matches!(input.recipient, Recipient::Segment { .. });
    let send = send_id(tenant, &key);
    let request = MailRequest {
        message: if single {
            message_id(tenant, &key, None)
        } else {
            send.clone()
        },
        key: key.clone(),
        template: input.template.clone(),
        recipient: input.recipient.clone(),
        values: input.values.clone(),
        category: input.category.clone(),
        class,
        reference: input.reference.clone(),
        send,
        inputs,
    };
    Ok((job_of(job_key(&key), &request, now), request))
}

/// The members of a segment when a send is accepted, counted by cursor up to a bound: what the
/// sender is told (EML-1561), never who they are.
fn members_now(app: &App, tenant: &TenantId, segment: SegmentId) -> Result<u32, ApiError> {
    const BOUND: usize = 100_000;
    let mut count = 0usize;
    let mut after: Option<PrincipalId> = None;
    loop {
        let page = app
            .store
            .segment_members(tenant, segment, after.as_ref(), 1_000)?;
        count += page.len();
        if page.len() < 1_000 || count >= BOUND {
            break;
        }
        after = page.last().map(|m| m.person.clone());
    }
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

/// Accept a send and create its job alone, as the send-message route and the step send-mail do
/// (EML-1560): the job of the key once; a repeat of the key with the same inputs answers the first
/// result, and with other inputs is refused as a conflict.
pub fn submit(
    app: &App,
    tenant: &TenantId,
    input: &SendInput,
    now: Timestamp,
) -> Result<Submitted, ApiError> {
    let (job, request) = mail_job(app, tenant, input, now)?;
    if let Some(existing) = app.store.job(tenant, &jobs::job_id(tenant, &job.key))? {
        let first: Option<MailRequest> = match &existing.work {
            JobWork::Mail { request } => serde_json::from_value(request.clone()).ok(),
            _ => None,
        };
        match first {
            Some(first) if first.inputs == request.inputs => {}
            _ => {
                return Err(ApiError::new(
                    ErrorCode::Conflict,
                    "The idempotency key was used with other inputs.",
                ))
            }
        }
    } else {
        app.store.jobs_create(tenant, &[job], now)?;
    }
    let (message, recipients) = match &request.recipient {
        Recipient::Segment { segment, .. } => (None, members_now(app, tenant, *segment)?),
        _ => (Some(request.message.clone()), 1),
    };
    Ok(Submitted {
        send: request.send,
        message,
        batches: recipients.div_ceil(BATCH as u32),
        recipients,
    })
}

/// The step send-mail of an automation (D-6560, WG-5149): a send as the automation principal,
/// which must hold messages-send through its run-as roles, keyed by the step key, so an attempt
/// made again after a crash sends once. `to` is an address (it holds an `@`) or a principal id.
/// Returns the message id, or why not and whether an attempt made again may succeed.
#[allow(clippy::too_many_arguments)]
pub fn step_send(
    app: &App,
    tenant: &TenantId,
    automation: &strata_kernel::WorkflowId,
    template: &TemplateId,
    to: &Value,
    category: Option<&str>,
    step_key: &str,
    now: Timestamp,
) -> Result<String, (String, bool)> {
    let state = app
        .store
        .load(tenant, &[])
        .map_err(|e| (e.to_string(), true))?;
    let Some(found) = state.tenant else {
        return Err((format!("no tenant {tenant}"), false));
    };
    let principal = Principal::User(automation_principal(automation));
    let wanted = BTreeSet::from([Grant::Capability(MESSAGES_SEND.to_string())]);
    if !holds_any(&found, &principal, &wanted) {
        return Err((
            format!("automation {automation} may not send mail: its run-as roles do not hold {MESSAGES_SEND}"),
            false,
        ));
    }
    let recipient = match to {
        Value::Str(text) if text.contains('@') => Recipient::Address {
            address: text.clone(),
        },
        Value::Str(text) => Recipient::Principal {
            principal: PrincipalId::new(text.as_str()).map_err(|_| {
                (
                    format!("to: {text} is not an address or a principal id"),
                    false,
                )
            })?,
        },
        other => {
            return Err((
                format!("to: {other:?} is not an address or a principal id"),
                false,
            ))
        }
    };
    let input = SendInput {
        template: template.clone(),
        recipient,
        category: category.map(str::to_string),
        class: None,
        idempotency_key: step_key.to_string(),
        values: BTreeMap::new(),
        reference: None,
    };
    match submit(app, tenant, &input, now) {
        Ok(done) => Ok(done.message.unwrap_or(done.send)),
        Err(error) => {
            let retry = matches!(
                error.0.code,
                ErrorCode::Unavailable | ErrorCode::Internal | ErrorCode::Throttled
            );
            Err((error.0.message.clone(), retry))
        }
    }
}

/// Send a sign-in message (an emailed link, a recovery notice) at once, as class sign-in: never
/// held by the allowance or the breaker, stopped only by a suppression of the whole address
/// (EML-1562, EML-1563), and recorded in the mail log of the tenant without its body (EML-011).
/// A message whose recipient or subject holds a line break is refused (EML-1566).
///
/// Implements: H-034
pub fn send_sign_in(app: &App, tenant: &TenantId, mail: &Mail) -> Result<(), MailError> {
    if let Some(problem) = render::address_problem(&mail.to) {
        return Err(MailError(format!(
            "{}the recipient address {problem}",
            crate::mail::PERMANENT
        )));
    }
    if let Some(problem) = render::header_problem(&mail.subject) {
        return Err(MailError(format!(
            "{}the subject {problem}",
            crate::mail::PERMANENT
        )));
    }
    let now = app.now();
    let nonce = crate::auth::random_hex(16).map_err(|e| MailError(e.0.message))?;
    let key = key_hash(&format!("sign-in:{nonce}"));
    let hash = store::address_hash(&mail.to);
    let mut record = store::MessageRecord {
        message: message_id(tenant, &key, None),
        key,
        recipient: None,
        address: Some(mail.to.clone()),
        address_hash: Some(hash.clone()),
        template: None,
        state: MessageState::Queued,
        attempts: 1,
        provider_reference: None,
        fail_reason: None,
        category: None,
        class: AllowanceClass::SignIn,
        send: None,
        reference: None,
        counted: None,
        created_at: now.0,
        updated_at: now.0,
        final_at: None,
    };
    let suppressed = match store::suppressions_of(app, tenant, &hash, None) {
        Ok(entries) => policy::suppressed(&entries, None, None),
        Err(error) => {
            LOG.error(
                "mail.suppression_unreadable",
                "",
                json!({ "tenant": tenant.as_str(), "error": error.to_string() }),
            );
            false
        }
    };
    let sent = if suppressed {
        record.state = MessageState::Suppressed;
        Ok(())
    } else {
        let sent = app.cfg.mail.send(mail);
        match &sent {
            Ok(()) => {
                record.state = MessageState::Sent;
                record.provider_reference =
                    Some(store::provider_reference(tenant, &record.message));
            }
            Err(error) => {
                record.state = MessageState::Failed;
                record.fail_reason = Some(jobs::clip_error(&error.0));
            }
        }
        sent
    };
    record.final_at = Some(now.0);
    if let Err(error) = store::put_message(app, tenant, &record) {
        LOG.error(
            "mail.log_failed",
            "",
            json!({ "tenant": tenant.as_str(), "error": error.to_string() }),
        );
    }
    sent
}
