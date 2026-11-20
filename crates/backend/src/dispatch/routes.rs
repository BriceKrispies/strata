//! The routes of the mail dispatcher: send a templated mail, read and count the mail log, declare
//! a category, manage the suppression list, and follow a subscription link. Each answers for the
//! caller's own tenant only; a request that names another tenant is refused, recorded as an audit
//! entry and raised as an alert (TEN-1140, THR-259).

use super::store::{self, Category, Suppression, SuppressionReason, ALL, ALL_PROMOTIONAL};
use super::{
    links, valid_category, valid_reference, AllowanceClass, MessageState, Purpose, Recipient,
    SendInput, MAIL_CONFIGURE, MAIL_READ, MESSAGES_SEND,
};
use crate::app::{ApiError, App, Shared};
use crate::auth::Identity;
use crate::http::as_caller;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde_json::{json, Value as Json_};
use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::rules::holds_any;
use strata_kernel::segments::SegmentId;
use strata_kernel::{Grant, PrincipalId, TemplateId};
use strata_protocol::api::ErrorCode;

/// The most entries one page of the log holds, and how many when the request names no limit.
const MAX_PAGE: usize = 100;
const DEFAULT_PAGE: usize = 50;

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Invalid, message)
}

/// The caller is signed in and holds the capability `name`; the owner holds every capability.
fn require(app: &App, identity: &Identity, name: &str) -> Result<(), ApiError> {
    if identity.anonymous() {
        return Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in to do this.",
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

/// A tenant named in the request must be the caller's.
fn same_tenant(app: &App, identity: &Identity, body: &Json_) -> Result<(), ApiError> {
    match body.get("tenant").and_then(Json_::as_str) {
        Some(named) if named != identity.tenant.as_str() => {
            app.audit(
                "tenant.mismatch",
                &identity.correlation,
                json!({ "tenant": identity.tenant.as_str(), "named": named, "route": "mail" }),
            );
            Err(ApiError::new(
                ErrorCode::Forbidden,
                "The request names another site.",
            ))
        }
        _ => Ok(()),
    }
}

/// A text input under its spec name (`a-b`) or its JSON name (`a_b`), when present.
fn optional_text<'a>(body: &'a Json_, name: &str) -> Result<Option<&'a str>, ApiError> {
    let value = body
        .get(name)
        .or_else(|| body.get(name.replace('-', "_").as_str()));
    match value {
        None | Some(Json_::Null) => Ok(None),
        Some(Json_::String(text)) if !text.is_empty() && text.len() <= 1_024 => {
            Ok(Some(text.as_str()))
        }
        Some(_) => Err(invalid(format!("{name} is not valid."))),
    }
}

fn required_text<'a>(body: &'a Json_, name: &str) -> Result<&'a str, ApiError> {
    optional_text(body, name)?.ok_or_else(|| invalid(format!("{name} is missing.")))
}

fn optional_time(body: &Json_, name: &str) -> Result<Option<i64>, ApiError> {
    match body.get(name) {
        None | Some(Json_::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .map(Some)
            .ok_or_else(|| invalid(format!("{name} is not a time in milliseconds."))),
    }
}

/// The recipient of send-message (spec type `send-recipient`, externally tagged).
fn recipient_of(value: Option<&Json_>) -> Result<Recipient, ApiError> {
    let bad = || invalid("The recipient is a principal, an address or a segment.");
    let Some(Json_::Object(tagged)) = value else {
        return Err(bad());
    };
    let (tag, inner) = tagged.iter().next().ok_or_else(bad)?;
    let field = |name: &str| inner.get(name).and_then(Json_::as_str).ok_or_else(bad);
    match tag.to_ascii_lowercase().as_str() {
        "principal" => Ok(Recipient::Principal {
            principal: PrincipalId::new(field("principal")?).map_err(|_| bad())?,
        }),
        "address" => Ok(Recipient::Address {
            address: field("address")?.to_string(),
        }),
        "segment" => Ok(Recipient::Segment {
            segment: serde_json::from_value::<SegmentId>(json!(field("segment")?))
                .map_err(|_| bad())?,
            after: None,
        }),
        _ => Err(bad()),
    }
}

/// The values of a send (spec type `value-map`): an object, or JSON text of one, whose members
/// are rendered as text.
fn values_of(value: Option<&Json_>) -> Result<BTreeMap<String, String>, ApiError> {
    let object = match value {
        None | Some(Json_::Null) => return Ok(BTreeMap::new()),
        Some(Json_::String(text)) => serde_json::from_str::<Json_>(text)
            .map_err(|_| invalid("The values are not a JSON object."))?,
        Some(other) => other.clone(),
    };
    let Json_::Object(members) = object else {
        return Err(invalid("The values are not a JSON object."));
    };
    Ok(members
        .into_iter()
        .map(|(name, value)| {
            let text = match value {
                Json_::String(text) => text,
                Json_::Null => String::new(),
                other => other.to_string(),
            };
            (name, text)
        })
        .collect())
}

/// Send a templated mail to a principal, an address or a segment (send-message, channel mail).
///
/// Implements: CT-820
pub fn send(app: &App, identity: &Identity, body: &Json_) -> Result<Json_, ApiError> {
    same_tenant(app, identity, body)?;
    require(app, identity, MESSAGES_SEND)?;
    match optional_text(body, "channel")? {
        None | Some("mail") => {}
        Some(_) => {
            return Err(invalid(
                "Only the channel mail is served; inbox notices are not built yet.",
            ))
        }
    }
    if body.get("deliver").is_some_and(|d| !d.is_null()) {
        return Err(invalid(
            "A delivery time is not built yet; a send delivers now.",
        ));
    }
    let category = match optional_text(body, "category")? {
        Some(name) if valid_category(name) => Some(name.to_string()),
        Some(_) => return Err(invalid("The category is not a category id.")),
        None => None,
    };
    let class = match optional_text(body, "allowance-class")? {
        Some(text) => Some(
            AllowanceClass::parse(text)
                .ok_or_else(|| invalid("The allowance class is transactional or bulk."))?,
        ),
        None => None,
    };
    let idempotency_key = match optional_text(body, "idempotency-key")? {
        Some(key) => key.to_string(),
        None => identity
            .idempotency_key
            .clone()
            .ok_or_else(|| invalid("A send names an idempotency key."))?,
    };
    let input = SendInput {
        template: TemplateId(required_text(body, "template")?.to_string()),
        recipient: recipient_of(body.get("recipient"))?,
        category,
        class,
        idempotency_key,
        values: values_of(body.get("values"))?,
        reference: optional_text(body, "reference")?.map(str::to_string),
    };
    let done = super::submit(app, &identity.tenant, &input, app.now())?;
    Ok(json!({
        "progress": { "send": done.send, "state": "accepted", "batches": done.batches },
        "recipients": done.recipients,
        "message": done.message,
    }))
}

/// The filters the log is listed and counted by (EML-1565).
struct Filter {
    state: Option<MessageState>,
    category: Option<String>,
    template: Option<String>,
    reference: Option<String>,
    from: Option<i64>,
    until: Option<i64>,
}

impl Filter {
    fn of(body: &Json_, with_state: bool) -> Result<Filter, ApiError> {
        let state = if with_state {
            match optional_text(body, "state")? {
                Some(text) => Some(
                    MessageState::parse(text).ok_or_else(|| invalid("The state is not valid."))?,
                ),
                None => None,
            }
        } else {
            None
        };
        let reference = optional_text(body, "reference")?.map(str::to_string);
        if reference.as_deref().is_some_and(|r| !valid_reference(r)) {
            return Err(invalid("The reference is not valid."));
        }
        Ok(Filter {
            state,
            category: optional_text(body, "category")?.map(str::to_string),
            template: optional_text(body, "template")?.map(str::to_string),
            reference,
            from: optional_time(body, "from")?,
            until: optional_time(body, "until")?,
        })
    }

    fn admits(&self, record: &store::MessageRecord) -> bool {
        self.state.is_none_or(|s| record.state == s)
            && self
                .category
                .as_ref()
                .is_none_or(|c| record.category.as_ref() == Some(c))
            && self.template.as_ref().is_none_or(|t| {
                record.template.as_ref().map(TemplateId::as_str) == Some(t.as_str())
            })
            && self
                .reference
                .as_ref()
                .is_none_or(|r| record.reference.as_ref() == Some(r))
            && self.from.is_none_or(|from| record.created_at >= from)
            && self.until.is_none_or(|until| record.created_at < until)
    }
}

/// One entry of the mail log as list-mail-messages answers it (spec type `mail-log-entry`): no
/// body (EML-011).
fn entry(record: &store::MessageRecord) -> Json_ {
    json!({
        "message": record.message,
        "recipient": record.address,
        "state": record.state.as_str(),
        "at": record.updated_at,
        "category": record.category,
        "template": record.template.as_ref().map(TemplateId::as_str),
        "send": record.send,
        "reference": record.reference,
        "allowance-class": record.class.as_str(),
    })
}

/// The mail log of the caller's tenant, newest first, by cursor (list-mail-messages, EML-010).
///
/// Implements: CT-222
pub fn list(app: &App, identity: &Identity, body: &Json_) -> Result<Json_, ApiError> {
    same_tenant(app, identity, body)?;
    require(app, identity, MAIL_READ)?;
    let filter = Filter::of(body, true)?;
    let limit = body
        .get("limit")
        .and_then(Json_::as_u64)
        .map_or(DEFAULT_PAGE, |n| n as usize)
        .clamp(1, MAX_PAGE);
    let cursor = optional_text(body, "cursor")?;
    let mut all = store::messages(app, &identity.tenant)?;
    all.reverse();
    fn position(r: &store::MessageRecord) -> String {
        format!("{:020}:{}", r.created_at, r.message)
    }
    let page: Vec<&store::MessageRecord> = all
        .iter()
        .filter(|r| cursor.is_none_or(|c| position(r).as_str() < c))
        .filter(|r| filter.admits(r))
        .take(limit)
        .collect();
    let next = (page.len() == limit)
        .then(|| page.last().map(|r| position(r)))
        .flatten();
    let messages: Vec<Json_> = page.iter().map(|r| entry(r)).collect();
    Ok(json!({ "messages": messages, "next": next }))
}

/// The number of messages in each state grouped by reference, category or template, over what
/// the log holds; no count of opens or clicks (count-mail-messages, EML-1565, D-6571).
///
/// Implements: CT-834
pub fn count(app: &App, identity: &Identity, body: &Json_) -> Result<Json_, ApiError> {
    same_tenant(app, identity, body)?;
    require(app, identity, MAIL_READ)?;
    let group_by = required_text(body, "group-by")?;
    if !matches!(group_by, "reference" | "category" | "template") {
        return Err(invalid("group-by is reference, category or template."));
    }
    let filter = Filter::of(body, false)?;
    if let (Some(from), Some(until)) = (filter.from, filter.until) {
        if from > until {
            return Err(invalid("from is after until."));
        }
        if until - from > store::MESSAGE_RETENTION_MS {
            return Err(invalid("The period is longer than the log keeps."));
        }
    }
    let mut counts: BTreeMap<(Option<String>, &'static str), u64> = BTreeMap::new();
    for record in store::messages(app, &identity.tenant)? {
        if !filter.admits(&record) {
            continue;
        }
        let group = match group_by {
            "reference" => record.reference.clone(),
            "category" => record.category.clone(),
            _ => record.template.as_ref().map(|t| t.as_str().to_string()),
        };
        *counts.entry((group, record.state.as_str())).or_default() += 1;
    }
    let counts: Vec<Json_> = counts
        .into_iter()
        .map(|((group, state), count)| json!({ "group": group, "state": state, "count": count }))
        .collect();
    Ok(json!({ "counts": counts }))
}

/// Declare or change a message category with its purpose (define-message-category, D-6562).
/// A category that holds suppressions or messages keeps its purpose.
///
/// Implements: CT-825
pub fn define_category(app: &App, identity: &Identity, body: &Json_) -> Result<Json_, ApiError> {
    same_tenant(app, identity, body)?;
    require(app, identity, MAIL_CONFIGURE)?;
    let id = required_text(body, "category")?;
    if !valid_category(id) {
        return Err(invalid(
            "A category is lower-case letters, digits and hyphens, beginning with a letter, at most 48 bytes, and not all or promotional.",
        ));
    }
    let purpose = Purpose::parse(required_text(body, "purpose")?)
        .ok_or_else(|| invalid("The purpose is required, transactional or promotional."))?;
    let name = required_text(body, "name")?;
    if name.chars().count() > 200 {
        return Err(invalid("The name is at most 200 characters."));
    }
    let tenant = &identity.tenant;
    let version = match store::category(app, tenant, id)? {
        Some(held) => {
            if held.purpose != purpose {
                let in_use = store::category_suppressed(app, tenant, id)?
                    || store::messages(app, tenant)?
                        .iter()
                        .any(|m| m.category.as_deref() == Some(id));
                if in_use {
                    return Err(invalid(
                        "The category holds suppressions or messages, so its purpose cannot change.",
                    ));
                }
            }
            held.version + 1
        }
        None => {
            if store::categories(app, tenant)?.len() >= store::MAX_CATEGORIES {
                return Err(invalid(
                    "limit-reached: the site already holds 50 categories.",
                ));
            }
            1
        }
    };
    store::put_category(
        app,
        tenant,
        &Category {
            category: id.to_string(),
            purpose,
            name: name.to_string(),
            version,
        },
    )?;
    Ok(json!({ "version": version }))
}

/// Add or remove an entry of the suppression list (manage-suppression-list): the owner's, or a
/// principal holding mail-configure. An entry is for one category, the whole address (`*`, when
/// none is named) or every promotional category (`*promotional`).
///
/// Implements: CT-223
pub fn manage_suppression(app: &App, identity: &Identity, body: &Json_) -> Result<Json_, ApiError> {
    same_tenant(app, identity, body)?;
    require(app, identity, MAIL_CONFIGURE)?;
    let address = required_text(body, "address")?;
    if let Some(problem) = super::render::address_problem(address) {
        return Err(invalid(format!("The address {problem}.")));
    }
    let category = optional_text(body, "category")?.unwrap_or(ALL);
    if category != ALL && category != ALL_PROMOTIONAL && !valid_category(category) {
        return Err(invalid("The category is a category id, * or *promotional."));
    }
    let hash = store::address_hash(address);
    match required_text(body, "action")? {
        "add" => store::add_suppression(
            app,
            &identity.tenant,
            &Suppression {
                address_hash: hash,
                category: category.to_string(),
                reason: SuppressionReason::Unsubscribe,
                recorded_at: app.now().0,
            },
        )?,
        "remove" => {
            store::remove_suppression(app, &identity.tenant, &hash, category, false)?;
        }
        _ => return Err(invalid("The action is add or remove.")),
    }
    app.audit(
        "mail.suppression_changed",
        &identity.correlation,
        json!({ "tenant": identity.tenant.as_str(), "category": category }),
    );
    Ok(json!({}))
}

/// Follow a signed unsubscribe or resubscribe link; no sign-in is needed
/// (change-subscription-by-link, EML-020, EML-1562). The body carries the link's address, or its
/// query string.
///
/// Implements: CT-827
pub fn subscription(app: &App, identity: &Identity, body: &Json_) -> Result<Json_, ApiError> {
    let params = match body.get("address").and_then(Json_::as_str) {
        Some(address) if address.len() <= 2_048 => links::parameters(address),
        Some(_) => return Err(invalid("The address is too long.")),
        None => body
            .as_object()
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                    .collect()
            })
            .unwrap_or_default(),
    };
    let done = links::follow(app, &identity.tenant, &params, app.now())?;
    Ok(json!({ "category": done.category, "state": done.state }))
}

macro_rules! route {
    ($name:ident, $handler:ident, $path:literal) => {
        #[doc = concat!("`POST ", $path, "`.")]
        pub async fn $name(
            State(app): State<Shared>,
            headers: HeaderMap,
            Json(body): Json<Json_>,
        ) -> Result<Json<Json_>, ApiError> {
            as_caller(&app, &headers, move |app, identity| {
                $handler(app, identity, &body)
            })
            .await
        }
    };
}

route!(send_route, send, "/internal/mail/send");
route!(messages_route, list, "/internal/mail/messages");
route!(counts_route, count, "/internal/mail/counts");
route!(
    categories_route,
    define_category,
    "/internal/mail/categories"
);
route!(
    suppressions_route,
    manage_suppression,
    "/internal/mail/suppressions"
);
route!(
    subscription_route,
    subscription,
    "/internal/mail/subscription"
);
