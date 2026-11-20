//! The in-app inbox of the notification service (component `notification-service`, NTF-150 to
//! NTF-152, NTF-1568 to NTF-1572, D-6565, D-6566): one inbox for each person, notices made from a
//! notice template, read and unread, removal, the preference of each category, mute and quiet
//! hours, and the live push of a notice to its recipient's open sockets.
//!
//! # Where authorization lives
//!
//! An inbox is a person's own data, and every call that reads or changes it names no person: the
//! recipient is the signed-in principal of the session, never a field of the request, exactly as
//! the person level of a setting is (`crate::settings::set_person`, SET-034). Items are kept under
//! a kind that holds the recipient, so another person's notice is not found by the caller's
//! lookup and the answer is not-found (NTF-1569); nothing is compared and nothing can be forgotten
//! in a check. An owner holds every capability and still reads only their own inbox. The kernel
//! decides nothing here, because the inbox is not tenant state the kernel loads: it holds no
//! record and no rule of the tenant's schema. The capabilities `notifications-read` and
//! `notifications-manage` are every signed-in person's and are checked at the gateway with the
//! session, as `setting-write-person` is; the capability `messages-send` is a tenant capability
//! and is checked here against the tenant's roles, as `ledger-post` is.
//!
//! # Storage
//!
//! The items, the preferences and the categories are small JSON documents in the store's
//! per-tenant document table (`Store::doc_put`), so a tenant's inbox is in the tenant's own
//! database and another tenant sees nothing of it. The block marked "Storage" below is all that
//! touches the store; nothing else in this file reads a document. A removal keeps a tombstone
//! with the text erased, so that a send that is repeated after the recipient removed the notice
//! does not make it again (CT-183 is keyed).
//!
//! The documents of one person are listed whole and paged in memory: an inbox is small by use, and
//! a tenant that sends without end to one person has a store that must index by recipient
//! (a follow-up task is filed).
//!
//! # What is not built
//!
//! Subscriptions (`subscribe-notifications`), the notices the platform itself produces
//! (NTF-152), the mail copy of a notice and the expansion of the fields of a record into a notice
//! (NTF-1570) are not built here; each is named in the report of the slice. Digests (D-6573,
//! NTF-1572) and the end of an inbox item at an erasure (ret-inbox-item) are in [`digest`].
//!
//! Implements: CT-182, CT-183, CT-821, CT-822, CT-823, CT-824, CT-826

use crate::app::{ApiError, App, Shared, LOG};
use crate::auth::Identity;
use crate::http::as_caller;
use crate::ops::{header, run_intent};
use crate::service::{person, text};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json_};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use strata_kernel::rules::holds_any;
use strata_kernel::segments::SegmentId;
use strata_kernel::settings as kernel_settings;
use strata_kernel::{
    canon, ExistingData, Grant, Intent, NoticeTemplateDecl, Output, Principal, PrincipalId,
    SchemaChange, SchemaVersion, SettingId, StructuredBlocks, TemplateId, TenantId, TenantState,
    Timestamp, Value,
};
use strata_protocol::api::ErrorCode;
use strata_storage_api::jobs::{derived_uuid, JobWork, NewJob, RetryPolicy};
use strata_storage_api::segments as kept;

pub mod digest;

/// The capability of sending a notice or a message (EML-1560).
pub const MESSAGES_SEND: &str = "messages-send";
/// The capability of defining a template (CT-824).
pub const MAIL_CONFIGURE: &str = "mail-configure";

/// The topics the platform produces, and no tenant does (NTF-1568, D-6566).
pub const PLATFORM_TOPICS: [&str; 6] = [
    "approval.requested",
    "approval.decided",
    "form.submitted",
    "file.scan-finished",
    "mail.sending-warning",
    "mail.sending-held",
];
/// What every topic of a tenant begins with (NTF-1568).
pub const TENANT_TOPIC_PREFIX: &str = "app.";
/// The topic of a notice that an automation step creates, which names none (CT-183).
pub const AUTOMATION_TOPIC: &str = "app.automation";
/// The longest topic, in bytes.
pub const MAX_TOPIC_BYTES: usize = 96;
/// The most labels of a topic.
pub const MAX_TOPIC_LABELS: usize = 4;
/// The most persons of a segment one batch makes a notice for (EML-1561).
pub const BATCH: u32 = 100;
/// The most notices one call to set read or remove names (CT-821, CT-822).
pub const MAX_NAMED: usize = 100;
/// The most notices a page of the inbox holds, and how many when the call names no limit.
const MAX_PAGE: usize = 100;
const DEFAULT_PAGE: usize = 50;
/// The most values a notice carries, and the longest name and value of one, in bytes.
const MAX_VALUES: usize = 32;
const MAX_VALUE_NAME: usize = 64;
const MAX_VALUE_BYTES: usize = 1024;
/// The longest rendered text of a notice, in bytes.
const MAX_BODY_BYTES: usize = 8192;

/// The setting that holds the time until which a person mutes their notices, a timestamp (D-6565).
pub const MUTE_UNTIL: &str = "mute-until";
/// The setting that holds a person's quiet hours, text of the form `22:00-07:00` (D-6565).
pub const QUIET_HOURS: &str = "quiet-hours";
/// The setting that holds a person's time zone (D-6565).
pub const TIME_ZONE: &str = "time-zone";

/// The answer of the mail copy of a notice while the mail dispatcher is not wired to this module.
pub const MAIL_NOT_WIRED: &str = "mail delivery not wired";

// ---------------------------------------------------------------------------------------------
// Topics (NTF-1568)
// ---------------------------------------------------------------------------------------------

/// Who produces a notice: the platform, whose topics are the fixed six, or a tenant, whose topics
/// begin with `app.` (D-6566).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Platform,
    Tenant,
}

/// Why a text is not a topic of the grammar, or `None` when it is: one to four labels of lower
/// case letters, digits and hyphens joined by dots, each beginning with a letter, at most 96 bytes.
pub fn grammar_problem(topic: &str) -> Option<String> {
    if topic.is_empty() || topic.len() > MAX_TOPIC_BYTES {
        return Some(format!("a topic is one to {MAX_TOPIC_BYTES} bytes"));
    }
    let labels: Vec<&str> = topic.split('.').collect();
    if labels.len() > MAX_TOPIC_LABELS {
        return Some(format!("a topic has at most {MAX_TOPIC_LABELS} labels"));
    }
    for label in labels {
        let mut chars = label.chars();
        let first = matches!(chars.next(), Some(c) if c.is_ascii_lowercase());
        if !first || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Some(
                "each label of a topic is lower case letters, digits and hyphens, beginning with a letter"
                    .to_string(),
            );
        }
    }
    None
}

/// Why `topic` may not be produced by `source`, or `None` when it may (NTF-1568): it follows the
/// grammar, and it is one of the platform's six when the platform produces it, or begins with
/// `app.` and so is not any of them when a tenant does.
pub fn topic_problem(topic: &str, source: Source) -> Option<String> {
    if let Some(problem) = grammar_problem(topic) {
        return Some(problem);
    }
    match source {
        Source::Platform if !PLATFORM_TOPICS.contains(&topic) => Some(format!(
            "{topic} is not a topic of the platform; the platform produces {}",
            PLATFORM_TOPICS.join(", ")
        )),
        Source::Tenant if !topic.starts_with(TENANT_TOPIC_PREFIX) => Some(format!(
            "a topic of a tenant begins with {TENANT_TOPIC_PREFIX}; {topic} does not"
        )),
        _ => None,
    }
}

/// A topic or a prefix of topics that a list or a count filters by (NTF-1568, NTF-1569).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TopicFilter {
    /// One topic.
    Exact(String),
    /// Every topic that begins with this and a dot: the filter `a.b.*` holds `a.b`.
    Prefix(String),
}

impl TopicFilter {
    /// A filter from its text: a topic, or a prefix that ends in `.*`.
    pub fn parse(text: &str) -> Result<TopicFilter, String> {
        match text.strip_suffix(".*") {
            Some(base) => match grammar_problem(base) {
                Some(problem) => Err(problem),
                None if base.split('.').count() >= MAX_TOPIC_LABELS => {
                    Err("a prefix of topics leaves room for a label after it".to_string())
                }
                None => Ok(TopicFilter::Prefix(format!("{base}."))),
            },
            None => match grammar_problem(text) {
                Some(problem) => Err(problem),
                None => Ok(TopicFilter::Exact(text.to_string())),
            },
        }
    }

    /// Whether a topic is one the filter holds.
    pub fn matches(&self, topic: &str) -> bool {
        match self {
            TopicFilter::Exact(wanted) => wanted == topic,
            TopicFilter::Prefix(prefix) => topic.starts_with(prefix.as_str()),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Categories and preferences (NTF-1571)
// ---------------------------------------------------------------------------------------------

/// What a message category is for (D-6562): a `required` category cannot be turned off, muted or
/// deferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Purpose {
    Required,
    Transactional,
    Promotional,
}

/// The mode of a person for one category on one channel (NTF-1571).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    On,
    Off,
    Digest,
}

impl Mode {
    fn parse(text: &str) -> Option<Mode> {
        match text {
            "on" => Some(Mode::On),
            "off" => Some(Mode::Off),
            "digest" => Some(Mode::Digest),
            _ => None,
        }
    }
}

/// The channels a preference is set for (`notification-channel`).
pub const CHANNELS: [&str; 2] = ["mail", "inbox"];

// ---------------------------------------------------------------------------------------------
// The notice and the item
// ---------------------------------------------------------------------------------------------

/// What one send makes for each of its recipients: the inputs of `send-notice` other than the
/// tenant, the recipient and the idempotency key (CT-183).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub topic: String,
    /// The category; an automation step names none (CT-183), and a notice with none has no
    /// preference and is not required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub template: String,
    #[serde(default)]
    pub values: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// The record the notice is about, by number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<u64>,
    /// Whether the notice also goes out by mail (rec-subscription, NTF-151).
    #[serde(default)]
    pub mail: bool,
}

/// What became of the live push of a notice (D-6565).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PushState {
    /// Pushed to the recipient's open sockets.
    Sent,
    /// Not pushed, because the recipient is in quiet hours.
    Deferred,
    /// Not pushed, because the recipient muted notices.
    Dropped,
    /// Not pushed, because the person's mode for the category is digest.
    Held,
}

impl PushState {
    pub fn as_str(self) -> &'static str {
        match self {
            PushState::Sent => "sent",
            PushState::Deferred => "deferred",
            PushState::Dropped => "dropped",
            PushState::Held => "held",
        }
    }
}

/// One item of an inbox (`rec-inbox-item`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub recipient: String,
    pub topic: String,
    #[serde(default)]
    pub category: Option<String>,
    pub template: String,
    /// The title, rendered for this recipient; erased by a removal.
    pub title: String,
    /// The text, rendered for this recipient; erased by a removal.
    pub body: String,
    /// The values the notice was made with; erased by a removal.
    #[serde(default)]
    pub values: BTreeMap<String, String>,
    pub created_at: i64,
    #[serde(default)]
    pub read_at: Option<i64>,
    #[serde(default)]
    pub removed_at: Option<i64>,
    #[serde(default)]
    pub reference: Option<String>,
    #[serde(default)]
    pub record: Option<u64>,
    pub push: PushState,
    /// For each channel, the digest this item was listed in (NTF-1572): an item is listed once
    /// for each channel.
    #[serde(default)]
    pub digested: BTreeMap<String, String>,
}

impl Item {
    fn live(&self) -> bool {
        self.removed_at.is_none()
    }

    fn unread(&self) -> bool {
        self.live() && self.read_at.is_none()
    }

    /// The item as `inbox-notice` states it.
    fn as_notice(&self) -> Json_ {
        let mut notice = json!({
            "id": self.id,
            "kind": self.template,
            "body": json!({ "title": self.title, "text": self.body }).to_string(),
            "at": self.created_at,
            "read": self.read_at.is_some(),
            "topic": self.topic,
        });
        if let Some(object) = notice.as_object_mut() {
            if let Some(category) = &self.category {
                object.insert("category".into(), json!(category));
            }
            if let Some(reference) = &self.reference {
                object.insert("reference".into(), json!(reference));
            }
            if let Some(read_at) = self.read_at {
                object.insert("read_at".into(), json!(read_at));
            }
        }
        notice
    }
}

/// What a delivery did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Delivery {
    /// The id of the item this send of this key makes, which the inbox holds only when `stored`.
    pub item: String,
    /// Whether this call made it.
    pub created: bool,
    /// Whether the inbox holds it: false for a category the recipient turned off for the inbox.
    pub stored: bool,
    pub push: &'static str,
    /// The answer of the mail copy, when the notice asked for one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mail: Option<String>,
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Invalid, message)
}

fn not_found(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::NotFound, message)
}

// ---------------------------------------------------------------------------------------------
// Storage: the only block that reads or writes a document
// ---------------------------------------------------------------------------------------------

/// The kind of the documents that hold one person's items.
fn item_kind(recipient: &PrincipalId) -> String {
    format!("inbox-item:{recipient}")
}

/// The kind of the documents that hold preferences, and the key of one.
const PREFERENCE_KIND: &str = "notice-preference";

fn preference_key(recipient: &PrincipalId, category: &str, channel: &str) -> String {
    format!("{recipient}|{category}|{channel}")
}

/// The kind of the documents that hold a tenant's message categories, each under its name with
/// `{"purpose": "required" | "transactional" | "promotional"}` (D-6562).
///
/// Wiring point: the categories are declared by `define-message-category` (CT-825), which the mail
/// dispatcher owns. This reads the document that declaration is expected to leave; if the
/// dispatcher keeps its categories elsewhere, [`category_purpose`] is the one function to change.
pub const CATEGORY_KIND: &str = "message-category";

/// The writes to items, one at a time within this process: a change of an item is read, changed
/// and written, so two changes at once could otherwise bring back a removed item.
static WRITES: Mutex<()> = Mutex::new(());

fn writing() -> std::sync::MutexGuard<'static, ()> {
    WRITES.lock().unwrap_or_else(|e| e.into_inner())
}

/// The id of the item that a send of `key` makes for `recipient`: the same on every attempt.
pub fn item_id(tenant: &TenantId, recipient: &PrincipalId, key: &str) -> String {
    derived_uuid(&format!(
        "strata/inbox-item/v1\0{tenant}\0{recipient}\0{key}"
    ))
}

fn decode<T: for<'de> Deserialize<'de>>(kind: &str, key: &str, text: &str) -> Result<T, ApiError> {
    serde_json::from_str(text)
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, format!("{kind} {key}: {e}")))
}

fn encode<T: Serialize>(kind: &str, key: &str, value: &T) -> Result<String, ApiError> {
    serde_json::to_string(value)
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, format!("{kind} {key}: {e}")))
}

/// One item of a person, removed or not.
fn load_item(
    app: &App,
    tenant: &TenantId,
    recipient: &PrincipalId,
    id: &str,
) -> Result<Option<Item>, ApiError> {
    let kind = item_kind(recipient);
    match app.store.doc_get(tenant, &kind, id)? {
        None => Ok(None),
        Some(text) => decode(&kind, id, &text).map(Some),
    }
}

/// Every item of a person, removed or not, newest first.
fn load_items(
    app: &App,
    tenant: &TenantId,
    recipient: &PrincipalId,
) -> Result<Vec<Item>, ApiError> {
    let kind = item_kind(recipient);
    let mut items = app
        .store
        .doc_list(tenant, &kind)?
        .into_iter()
        .map(|(key, text)| decode::<Item>(&kind, &key, &text))
        .collect::<Result<Vec<_>, _>>()?;
    items.sort_by(|a, b| (b.created_at, &b.id).cmp(&(a.created_at, &a.id)));
    Ok(items)
}

/// Keep an item, only when its id is free; `false` says the send was made before.
fn create_item(app: &App, tenant: &TenantId, item: &Item) -> Result<bool, ApiError> {
    let kind = item_kind(&principal_of(&item.recipient)?);
    let text = encode(&kind, &item.id, item)?;
    Ok(app.store.doc_put(tenant, &kind, &item.id, &text, true)?)
}

/// Replace an item.
fn save_item(app: &App, tenant: &TenantId, item: &Item) -> Result<(), ApiError> {
    let kind = item_kind(&principal_of(&item.recipient)?);
    let text = encode(&kind, &item.id, item)?;
    app.store.doc_put(tenant, &kind, &item.id, &text, false)?;
    Ok(())
}

fn principal_of(text: &str) -> Result<PrincipalId, ApiError> {
    PrincipalId::new(text).map_err(|e| invalid(format!("principal: {e}")))
}

/// The mode of a person for a category on a channel: on when they set none.
fn preference(
    app: &App,
    tenant: &TenantId,
    recipient: &PrincipalId,
    category: &str,
    channel: &str,
) -> Result<Mode, ApiError> {
    let key = preference_key(recipient, category, channel);
    match app.store.doc_get(tenant, PREFERENCE_KIND, &key)? {
        None => Ok(Mode::On),
        Some(text) => {
            let found: Json_ = decode(PREFERENCE_KIND, &key, &text)?;
            Ok(found
                .get("mode")
                .and_then(Json_::as_str)
                .and_then(Mode::parse)
                .unwrap_or(Mode::On))
        }
    }
}

fn save_preference(
    app: &App,
    tenant: &TenantId,
    recipient: &PrincipalId,
    category: &str,
    channel: &str,
    mode: Mode,
    period: digest::Period,
) -> Result<(), ApiError> {
    let key = preference_key(recipient, category, channel);
    let text = encode(
        PREFERENCE_KIND,
        &key,
        &json!({ "mode": mode, "period": period }),
    )?;
    app.store
        .doc_put(tenant, PREFERENCE_KIND, &key, &text, false)?;
    Ok(())
}

/// What a category of the tenant is for, or `None` when the tenant declares no such category.
pub fn category_purpose(
    app: &App,
    tenant: &TenantId,
    category: &str,
) -> Result<Option<Purpose>, ApiError> {
    match app.store.doc_get(tenant, CATEGORY_KIND, category)? {
        None => Ok(None),
        Some(text) => {
            let found: Json_ = decode(CATEGORY_KIND, category, &text)?;
            Ok(found
                .get("purpose")
                .cloned()
                .and_then(|purpose| serde_json::from_value(purpose).ok()))
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Rendering (NTF-1570)
// ---------------------------------------------------------------------------------------------

/// `{{name}}` of a text replaced by the value of that name, or by nothing when there is none. A
/// value is placed as it is and is not read again, so a value that holds `{{` expands nothing.
fn substitute(text: &str, values: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let name = after[..end].trim();
                out.push_str(values.get(name).map_or("", String::as_str));
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Text only: control characters other than a line break are dropped, and the text is cut at the
/// longest a notice holds.
fn plain(text: &str, limit: usize) -> String {
    let mut clean: String = text
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect();
    if clean.len() > limit {
        let cut = (0..=limit)
            .rev()
            .find(|i| clean.is_char_boundary(*i))
            .unwrap_or(0);
        clean.truncate(cut);
    }
    clean
}

/// A template rendered for one recipient as text: its title on one line and its body (NTF-1570).
/// The body is rendered by `strata_kernel::blocks::render`, as a mail body is: each value it
/// places is text, and each token is the text `token` gives. The values are the sender's; no
/// field of a record is placed here, so nothing of a record the recipient may not read can reach
/// the text. Why not, when the body does not render.
pub fn render(
    title: &str,
    blocks: &str,
    values: &BTreeMap<String, String>,
    token: &dyn Fn(&str) -> Option<String>,
) -> Result<(String, String), String> {
    let typed = crate::dispatch::render::typed_values(blocks, values)?;
    let done = strata_kernel::blocks::render(blocks, &typed, token).map_err(|e| e.to_string())?;
    let title = plain(&substitute(title, values), 998).replace('\n', " ");
    Ok((title, plain(&done.text, MAX_BODY_BYTES)))
}

// ---------------------------------------------------------------------------------------------
// Mute and quiet hours (D-6565)
// ---------------------------------------------------------------------------------------------

/// A person setting of a recipient, read as the platform's services read it (CT-816, CT-829); a
/// setting the tenant does not define, or a recipient that is no person of the tenant, has none.
fn recipient_setting(state: &TenantState, person: &PrincipalId, name: &str) -> Option<Value> {
    let id = SettingId::new(name).ok()?;
    kernel_settings::resolve_for(state, person, &id)
        .ok()
        .map(|found| found.value)
}

/// The offset from UTC in minutes of a zone the platform can read without a time zone database:
/// `UTC`, `Z`, and `UTC+hh`, `UTC-hh`, `UTC+hh:mm`, `UTC-hh:mm`. Another name is read as UTC.
pub fn zone_offset_minutes(zone: &str) -> i64 {
    let Some(rest) = zone.strip_prefix("UTC") else {
        return 0;
    };
    let (sign, digits) = match rest.chars().next() {
        Some('+') => (1, &rest[1..]),
        Some('-') => (-1, &rest[1..]),
        _ => return 0,
    };
    let (hours, minutes) = digits.split_once(':').unwrap_or((digits, "0"));
    match (hours.parse::<i64>(), minutes.parse::<i64>()) {
        (Ok(h), Ok(m)) if (0..=14).contains(&h) && (0..60).contains(&m) => sign * (h * 60 + m),
        _ => 0,
    }
}

/// Whether `now` (milliseconds since the epoch) is inside quiet hours written `HH:MM-HH:MM`, read
/// in a zone at `offset` minutes from UTC. A range that ends before it starts crosses midnight; an
/// empty range or a text that is not a range holds no time.
pub fn in_quiet_hours(spec: &str, offset: i64, now: i64) -> bool {
    let minute = |text: &str| -> Option<i64> {
        let (h, m) = text.trim().split_once(':')?;
        let (h, m) = (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?);
        ((0..24).contains(&h) && (0..60).contains(&m)).then_some(h * 60 + m)
    };
    let Some((start, end)) = spec.split_once('-') else {
        return false;
    };
    let (Some(start), Some(end)) = (minute(start), minute(end)) else {
        return false;
    };
    let local = (now.div_euclid(60_000) + offset).rem_euclid(1440);
    match start.cmp(&end) {
        std::cmp::Ordering::Equal => false,
        std::cmp::Ordering::Less => start <= local && local < end,
        std::cmp::Ordering::Greater => local >= start || local < end,
    }
}

/// What the recipient's settings say about pushing at `now`: muted drops, quiet hours defer.
fn push_hold(state: &TenantState, recipient: &PrincipalId, now: i64) -> Option<PushState> {
    if let Some(Value::Time(until) | Value::Int(until)) =
        recipient_setting(state, recipient, MUTE_UNTIL)
    {
        if until > now {
            return Some(PushState::Dropped);
        }
    }
    if let Some(Value::Str(spec)) = recipient_setting(state, recipient, QUIET_HOURS) {
        let offset = match recipient_setting(state, recipient, TIME_ZONE) {
            Some(Value::Str(zone)) => zone_offset_minutes(&zone),
            _ => 0,
        };
        if in_quiet_hours(&spec, offset, now) {
            return Some(PushState::Deferred);
        }
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Live delivery (NTF-1569)
// ---------------------------------------------------------------------------------------------

/// A change of an inbox, told to the recipient's open sockets (`crate::hub::Hub::notices`). It
/// names the notice and carries none of its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Push {
    pub tenant: TenantId,
    pub recipient: PrincipalId,
    /// `arrived`, `read`, `unread` or `ended`.
    pub kind: &'static str,
    pub id: String,
    pub topic: String,
    pub category: Option<String>,
    pub at: i64,
    pub unread: u32,
}

/// Whether a connection is told of a push: the connection is signed in as the recipient, in the
/// tenant of the notice, and nobody else is (the rule of `strata_substrate::subscribe` that what a
/// subscriber is told is what that subscriber may read, applied to a notice, which only its
/// recipient may).
pub fn visible_to(push: &Push, identity: &Identity) -> bool {
    !identity.anonymous()
        && push.tenant == identity.tenant
        && matches!(&identity.principal, Principal::User(id) if *id == push.recipient)
}

fn announce(app: &App, tenant: &TenantId, kind: &'static str, item: &Item, unread: u32, at: i64) {
    let Ok(recipient) = principal_of(&item.recipient) else {
        return;
    };
    app.hub.notices.send(
        tenant,
        Push {
            tenant: tenant.clone(),
            recipient,
            kind,
            id: item.id.clone(),
            topic: item.topic.clone(),
            category: item.category.clone(),
            at,
            unread,
        },
    );
}

fn unread_count(items: &[Item], filter: Option<&TopicFilter>) -> u32 {
    items
        .iter()
        .filter(|item| item.unread() && filter.is_none_or(|f| f.matches(&item.topic)))
        .count() as u32
}

// ---------------------------------------------------------------------------------------------
// Sending (CT-183)
// ---------------------------------------------------------------------------------------------

/// The mail copy of a notice (NTF-151, rec-subscription): it hands the notice to the mail
/// dispatcher as a message of its category.
///
/// Wiring point: the dispatcher is another slice's (`crate::dispatch`, which this tree does not
/// hold). Until it exposes a function that makes the mail job, this answers that mail delivery is
/// not wired, and [`deliver`] reports that in the delivery without failing the notice.
pub fn request_mail_copy(
    _app: &App,
    _tenant: &TenantId,
    _recipient: &PrincipalId,
    _item: &Item,
) -> Result<(), String> {
    Err(MAIL_NOT_WIRED.to_string())
}

/// Make the notice for one recipient, once for `key` (CT-183): the same key makes no second item
/// and no second push, whether the first send is in flight, finished or removed by the recipient.
///
/// The recipient's category preference on the inbox channel decides whether the inbox holds the
/// notice at all (off, for a category that is not required, stores nothing); mute and quiet hours
/// and the digest mode decide only the live push, and the inbox still stores it (D-6565, NTF-1571).
pub fn deliver(
    app: &App,
    tenant: &TenantId,
    state: &TenantState,
    recipient: &PrincipalId,
    key: &str,
    notice: &Notice,
    now: Timestamp,
) -> Result<Delivery, ApiError> {
    let id = item_id(tenant, recipient, key);
    if load_item(app, tenant, recipient, &id)?.is_some() {
        return Ok(Delivery {
            item: id,
            created: false,
            stored: true,
            push: "none",
            mail: None,
        });
    }
    let template = state
        .schema
        .notice_templates
        .as_ref()
        .and_then(|all| all.get(&TemplateId(notice.template.clone())))
        .ok_or_else(|| not_found(format!("no notice template {}", notice.template)))?;
    let required = match &notice.category {
        None => false,
        Some(category) => match category_purpose(app, tenant, category)? {
            None => return Err(not_found(format!("no message category {category}"))),
            Some(purpose) => purpose == Purpose::Required,
        },
    };
    let mode = match (&notice.category, required) {
        (Some(category), false) => preference(app, tenant, recipient, category, "inbox")?,
        _ => Mode::On,
    };
    if mode == Mode::Off {
        return Ok(Delivery {
            item: id,
            created: false,
            stored: false,
            push: "none",
            mail: None,
        });
    }
    let push = if required {
        PushState::Sent
    } else if let Some(held) = push_hold(state, recipient, now.0) {
        held
    } else if mode == Mode::Digest {
        PushState::Held
    } else {
        PushState::Sent
    };
    // A token has no reader here yet (CTK-015): it renders as its fallback text.
    let tokens = |name: &str| {
        state
            .schema
            .tokens
            .as_ref()
            .and_then(|all| all.get(name))
            .map(|token| token.fallback.clone())
    };
    let (title, body) = render(
        &template.title,
        template.body.as_str(),
        &notice.values,
        &tokens,
    )
    .map_err(|problem| {
        invalid(format!(
            "The template {} does not render: {problem}.",
            notice.template
        ))
    })?;
    let item = Item {
        id: id.clone(),
        recipient: recipient.as_str().to_string(),
        topic: notice.topic.clone(),
        category: notice.category.clone(),
        template: notice.template.clone(),
        title,
        body,
        values: notice.values.clone(),
        created_at: now.0,
        read_at: None,
        removed_at: None,
        reference: notice.reference.clone(),
        record: notice.record,
        push,
        digested: BTreeMap::new(),
    };
    if !create_item(app, tenant, &item)? {
        // A send of the same key won the race.
        return Ok(Delivery {
            item: id,
            created: false,
            stored: true,
            push: "none",
            mail: None,
        });
    }
    // The person has an inbox: an erasure finds the items that name a record there (ret-inbox-item).
    app.store.doc_put(
        tenant,
        digest::RECIPIENT_KIND,
        recipient.as_str(),
        "{}",
        true,
    )?;
    if push == PushState::Sent {
        let unread = unread_count(&load_items(app, tenant, recipient)?, None);
        announce(app, tenant, "arrived", &item, unread, now.0);
    }
    let mail = notice
        .mail
        .then(|| request_mail_copy(app, tenant, recipient, &item).err())
        .flatten();
    Ok(Delivery {
        item: id,
        created: true,
        stored: true,
        push: push.as_str(),
        mail,
    })
}

/// The notice of an automation step `create-notice` (WFL-1523): made as the automation principal,
/// which must hold `messages-send` through its run-as roles, once for the step key, so that a step
/// that is attempted again after a crash between doing and reporting makes no second notice
/// (SCH-015). The topic is [`AUTOMATION_TOPIC`] and the notice has no category, because the step
/// names neither.
#[allow(clippy::too_many_arguments)]
pub fn automation_notice(
    app: &App,
    tenant: &TenantId,
    automation: &PrincipalId,
    step_key: &str,
    template: &str,
    recipient: &str,
    record: Option<u64>,
    now: Timestamp,
) -> Result<Delivery, ApiError> {
    let state = app
        .store
        .load(tenant, &[])?
        .tenant
        .ok_or_else(|| not_found(format!("no tenant {tenant}")))?;
    let wanted = BTreeSet::from([Grant::Capability(MESSAGES_SEND.to_string())]);
    if !holds_any(&state, &Principal::User(automation.clone()), &wanted) {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            format!("a notice needs the permission {MESSAGES_SEND}, which the automation's run-as roles do not hold"),
        ));
    }
    let notice = Notice {
        topic: AUTOMATION_TOPIC.to_string(),
        category: None,
        template: template.to_string(),
        values: BTreeMap::new(),
        reference: None,
        record,
        mail: false,
    };
    deliver(
        app,
        tenant,
        &state,
        &principal_of(recipient)?,
        step_key,
        &notice,
        now,
    )
}

/// The key of the job of batch `number` of a send, within its tenant.
fn batch_job_key(tenant: &TenantId, key: &str, number: u64) -> String {
    format!(
        "notice-batch:{}:{number}",
        canon::hash_bytes(format!("strata/notice-batch/v1\0{tenant}\0{key}").as_bytes())
    )
}

#[allow(clippy::too_many_arguments)]
fn batch_job(
    tenant: &TenantId,
    key: &str,
    sender: &PrincipalId,
    segment: SegmentId,
    after: Option<PrincipalId>,
    notice: &Notice,
    number: u64,
    now: Timestamp,
) -> Result<NewJob, ApiError> {
    Ok(NewJob {
        key: batch_job_key(tenant, key, number),
        work: JobWork::NoticeBatch {
            key: key.to_string(),
            sender: sender.clone(),
            segment,
            after,
            notice: serde_json::to_value(notice)
                .map_err(|e| ApiError::hidden(ErrorCode::Internal, e.to_string()))?,
            number,
        },
        due_at: now,
        policy: RetryPolicy::default(),
    })
}

/// Run one batch of a segment send as the worker does: make the notice for at most [`BATCH`]
/// members after the cursor, each once for the send key, and say which job runs next, if any. The
/// members are read as the sender, so a member the sender may not see gets no notice.
#[allow(clippy::too_many_arguments)]
pub fn run_batch(
    app: &App,
    tenant: &TenantId,
    key: &str,
    sender: &PrincipalId,
    segment: SegmentId,
    after: Option<&PrincipalId>,
    notice: &Json_,
    number: u64,
    now: Timestamp,
) -> Result<Option<NewJob>, String> {
    let notice: Notice = serde_json::from_value(notice.clone()).map_err(|e| e.to_string())?;
    let state = app
        .store
        .load(tenant, &[])
        .map_err(|e| e.to_string())?
        .tenant
        .ok_or_else(|| format!("no tenant {tenant}"))?;
    let ctx = strata_kernel::Context {
        tenant: tenant.clone(),
        principal: Principal::User(sender.clone()),
        session: None,
        now,
        correlation: format!("notice:{key}"),
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
    };
    let (members, next) = kept::members(app.store.as_ref(), &ctx, segment, after, Some(BATCH))
        .map_err(|e| ApiError::execute(e, false).describe())?;
    for member in &members {
        deliver(app, tenant, &state, &member.person, key, &notice, now)
            .map_err(|e| e.describe())?;
    }
    match next {
        None => Ok(None),
        Some(last) => batch_job(
            tenant,
            key,
            sender,
            segment,
            Some(last),
            &notice,
            number + 1,
            now,
        )
        .map(Some)
        .map_err(|e| e.describe()),
    }
}

// ---------------------------------------------------------------------------------------------
// The routes
// ---------------------------------------------------------------------------------------------

/// A tenant named in the request must be the caller's (TEN-1140).
fn same_tenant(app: &App, identity: &Identity, body: &Json_) -> Result<(), ApiError> {
    match body.get("tenant").and_then(Json_::as_str) {
        Some(named) if named != identity.tenant.as_str() => {
            app.audit(
                "tenant.mismatch",
                &identity.correlation,
                json!({ "tenant": identity.tenant.as_str(), "named": named, "route": "inbox" }),
            );
            Err(ApiError::new(
                ErrorCode::Forbidden,
                "tenant-mismatch: the request names another site.",
            ))
        }
        _ => Ok(()),
    }
}

/// The caller holds a tenant capability; the owner holds every one.
fn require_capability(
    state: &TenantState,
    identity: &Identity,
    name: &str,
) -> Result<(), ApiError> {
    let wanted = BTreeSet::from([Grant::Capability(name.to_string())]);
    if holds_any(state, &identity.principal, &wanted) {
        Ok(())
    } else {
        Err(ApiError::new(
            ErrorCode::Forbidden,
            format!("This needs the permission {name}."),
        ))
    }
}

/// The notices a call names: one to a hundred distinct ids (CT-821, CT-822).
fn named_notices(body: &Json_) -> Result<Vec<String>, ApiError> {
    let list = body
        .get("notices")
        .and_then(Json_::as_array)
        .ok_or_else(|| invalid("The request needs notices, a list."))?;
    if list.is_empty() {
        return Err(invalid("Name at least one notice."));
    }
    if list.len() > MAX_NAMED {
        return Err(invalid(format!("Name at most {MAX_NAMED} notices.")));
    }
    let mut seen = BTreeSet::new();
    for entry in list {
        let id = entry
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 64)
            .ok_or_else(|| invalid("A notice is named by its id."))?;
        seen.insert(id.to_string());
    }
    Ok(seen.into_iter().collect())
}

fn topic_filter(body: &Json_) -> Result<Option<TopicFilter>, ApiError> {
    match body.get("topic") {
        None | Some(Json_::Null) => Ok(None),
        Some(Json_::String(text)) => TopicFilter::parse(text)
            .map(Some)
            .map_err(|problem| invalid(format!("topic: {problem}"))),
        Some(_) => Err(invalid("topic is text.")),
    }
}

/// List the inbox of the caller, newest first, by cursor (CT-182, NTF-1569): `{ "state":
/// "unread" | "read", "topic": <topic or prefix.*>, "cursor", "limit" }`. The answer is the
/// notices, the cursor of the next page and the unread count for the same topic.
///
/// Implements: CT-182
pub async fn list(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        let me = person(identity)?;
        same_tenant(app, identity, &body)?;
        let topic = topic_filter(&body)?;
        let state = match body.get("state").and_then(Json_::as_str) {
            None => None,
            Some("unread") => Some(false),
            Some("read") => Some(true),
            Some(_) => return Err(invalid("state is unread or read.")),
        };
        let limit = body
            .get("limit")
            .and_then(Json_::as_u64)
            .map_or(DEFAULT_PAGE, |n| (n as usize).clamp(1, MAX_PAGE));
        let after = match body.get("cursor").and_then(Json_::as_str) {
            None => None,
            Some(text) => {
                let (at, id) = text
                    .split_once(':')
                    .and_then(|(at, id)| Some((at.parse::<i64>().ok()?, id.to_string())))
                    .ok_or_else(|| invalid("cursor is not one this call gave."))?;
                Some((at, id))
            }
        };
        let items: Vec<Item> = load_items(app, &identity.tenant, &me)?
            .into_iter()
            .filter(Item::live)
            .collect();
        let unread = unread_count(&items, topic.as_ref());
        let mut page: Vec<&Item> = items
            .iter()
            .filter(|item| topic.as_ref().is_none_or(|f| f.matches(&item.topic)))
            .filter(|item| state.is_none_or(|read| item.read_at.is_some() == read))
            .filter(|item| {
                after
                    .as_ref()
                    .is_none_or(|(at, id)| (item.created_at, &item.id) < (*at, id))
            })
            .take(limit + 1)
            .collect();
        let next = (page.len() > limit).then(|| {
            page.truncate(limit);
            page.last()
                .map(|last| format!("{}:{}", last.created_at, last.id))
        });
        let notices: Vec<Json_> = page.iter().map(|item| item.as_notice()).collect();
        let mut reply = json!({ "notices": notices, "unread": unread });
        if let (Some(Some(next)), Some(object)) = (next, reply.as_object_mut()) {
            object.insert("next".into(), json!(next));
        }
        Ok(reply)
    })
    .await
}

/// Count the unread notices of the caller, for a topic or a prefix (CT-823, NTF-1569): the number
/// that [`list`] answers as `unread` for the same topic.
///
/// Implements: CT-823
pub async fn count(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        let me = person(identity)?;
        same_tenant(app, identity, &body)?;
        let topic = topic_filter(&body)?;
        let items = load_items(app, &identity.tenant, &me)?;
        Ok(json!({ "unread": unread_count(&items, topic.as_ref()) }))
    })
    .await
}

/// What a change of notices does to each.
#[derive(Clone, Copy)]
enum Change {
    Read,
    Unread,
    Remove,
}

/// Change notices of the caller: every one named must be in the caller's inbox, or nothing is
/// changed and the answer is not-found (NTF-1569).
fn change(app: &App, identity: &Identity, body: &Json_, how: Change) -> Result<Json_, ApiError> {
    let me = person(identity)?;
    same_tenant(app, identity, body)?;
    let ids = named_notices(body)?;
    let tenant = &identity.tenant;
    let _writing = writing();
    let mut items = Vec::new();
    for id in &ids {
        match load_item(app, tenant, &me, id)? {
            Some(item) if item.live() => items.push(item),
            _ => return Err(not_found("A named notice is not in your inbox.")),
        }
    }
    let now = app.now().0;
    let mut changed = Vec::new();
    for mut item in items {
        let kind = match how {
            Change::Read => {
                if item.read_at.is_none() {
                    item.read_at = Some(now);
                }
                "read"
            }
            Change::Unread => {
                item.read_at = None;
                "unread"
            }
            Change::Remove => {
                // The inbox item ends: its text is erased and a tombstone keeps the key (NTF-1569).
                item.removed_at = Some(now);
                item.title.clear();
                item.body.clear();
                item.values.clear();
                "ended"
            }
        };
        save_item(app, tenant, &item)?;
        changed.push((kind, item));
    }
    let unread = unread_count(&load_items(app, tenant, &me)?, None);
    for (kind, item) in &changed {
        if *kind == "read" {
            // The log line of the domain event inbox-notice-read: no principal (SCH-1575).
            LOG.info(
                "inbox.read",
                &identity.correlation,
                json!({ "topic": item.topic }),
            );
        }
        announce(app, tenant, *kind, item, unread, now);
    }
    Ok(json!({ "unread": unread }))
}

/// Mark notices of the caller read or unread (CT-821): `{ "notices": [<id>], "read": <bool> }`.
/// The answer is the unread count after the change.
///
/// Implements: CT-821
pub async fn set_read(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        let read = body
            .get("read")
            .and_then(Json_::as_bool)
            .ok_or_else(|| invalid("The request needs read, true or false."))?;
        change(
            app,
            identity,
            &body,
            if read { Change::Read } else { Change::Unread },
        )
    })
    .await
}

/// Remove notices of the caller (CT-822): `{ "notices": [<id>] }`. Each removal ends its inbox
/// item and tells the caller's open sockets so.
///
/// Implements: CT-822
pub async fn remove(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        change(app, identity, &body, Change::Remove)
    })
    .await
}

/// Set the mode of the caller for a category on a channel (CT-826, NTF-1571): `{ "category",
/// "channel": "mail" | "inbox", "mode": "on" | "off" | "digest", "period": "daily" | "weekly" }`. The preference is the caller's
/// own. A category the tenant does not declare is not found, and off for a required category is
/// refused.
///
/// Implements: CT-826
pub async fn set_preference(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        let me = person(identity)?;
        same_tenant(app, identity, &body)?;
        let category = text(&body, "category", 48)?;
        let channel = text(&body, "channel", 16)?;
        if !CHANNELS.contains(&channel) {
            return Err(invalid("channel is mail or inbox."));
        }
        let mode = Mode::parse(text(&body, "mode", 16)?)
            .ok_or_else(|| invalid("mode is on, off or digest."))?;
        let purpose = category_purpose(app, &identity.tenant, category)?
            .ok_or_else(|| not_found(format!("no message category {category}")))?;
        if purpose == Purpose::Required && mode == Mode::Off {
            return Err(invalid(format!(
                "{category} is a required category and cannot be turned off."
            )));
        }
        let period = match body.get("period") {
            None | Some(Json_::Null) => digest::Period::Daily,
            Some(given) => given
                .as_str()
                .and_then(digest::Period::parse)
                .ok_or_else(|| invalid("period is daily or weekly."))?,
        };
        save_preference(app, &identity.tenant, &me, category, channel, mode, period)?;
        if mode == Mode::Digest {
            // The chain of digest jobs delivers the digest when its period ends (NTF-1572).
            digest::ensure_chain(app, &identity.tenant, app.now())?;
        }
        Ok(json!({}))
    })
    .await
}

/// Define a notice template of the tenant (CT-824, NTF-1570): `{ "template", "title", "body":
/// <structured blocks as JSON text>, "expected_version": <schema version> }`, as a schema change
/// the kernel decides. The answer is the schema version.
///
/// Implements: CT-824
pub async fn define_template(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        person(identity)?;
        same_tenant(app, identity, &body)?;
        let state = header(app, identity)?;
        require_capability(&state, identity, MAIL_CONFIGURE)?;
        let template = text(&body, "template", 64)?;
        let title = text(&body, "title", 998)?;
        let blocks = body
            .get("body")
            .and_then(Json_::as_str)
            .ok_or_else(|| invalid("The request needs body, structured blocks as JSON text."))?;
        let expected = body
            .get("expected_version")
            .and_then(Json_::as_u64)
            .ok_or_else(|| invalid("The request needs expected_version, a whole number."))?;
        let intent = Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineNoticeTemplate(NoticeTemplateDecl {
                template: TemplateId(template.to_string()),
                title: title.to_string(),
                body: StructuredBlocks(blocks.to_string()),
            })],
            expected_version: Some(SchemaVersion(expected)),
            existing: ExistingData::Reject,
        };
        let done = run_intent(app, &identity.context(app), &intent, identity.anonymous())?;
        match done.transition.output {
            Output::Schema { version } => Ok(json!({ "version": version.0 })),
            other => Err(ApiError::new(
                ErrorCode::Internal,
                format!("unexpected output {other:?}"),
            )),
        }
    })
    .await
}

/// The notice a request describes, for a tenant sender.
fn notice_of(body: &Json_) -> Result<Notice, ApiError> {
    let topic = text(body, "topic", MAX_TOPIC_BYTES)?;
    if let Some(problem) = topic_problem(topic, Source::Tenant) {
        return Err(invalid(format!("topic: {problem}")));
    }
    let category = match body.get("category") {
        None | Some(Json_::Null) => None,
        Some(_) => Some(text(body, "category", 48)?.to_string()),
    };
    let template = text(body, "template", 64)?.to_string();
    let mut values = BTreeMap::new();
    match body.get("values") {
        None | Some(Json_::Null) => {}
        Some(Json_::Object(given)) => {
            if given.len() > MAX_VALUES {
                return Err(invalid(format!(
                    "A notice carries at most {MAX_VALUES} values."
                )));
            }
            for (name, value) in given {
                let text = match value {
                    Json_::String(text) => text.clone(),
                    Json_::Number(number) => number.to_string(),
                    Json_::Bool(flag) => flag.to_string(),
                    _ => {
                        return Err(invalid(format!(
                            "The value {name} is text, a number or a flag."
                        )))
                    }
                };
                if name.len() > MAX_VALUE_NAME || text.len() > MAX_VALUE_BYTES {
                    return Err(invalid(format!("The value {name} is too long.")));
                }
                values.insert(name.clone(), text);
            }
        }
        Some(_) => return Err(invalid("values is a map of names to text.")),
    }
    let reference = match body.get("reference") {
        None | Some(Json_::Null) => None,
        Some(_) => {
            let reference = text(body, "reference", 64)?;
            let mut chars = reference.chars();
            let well_formed = matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
                && chars.all(|c| c.is_ascii_alphanumeric() || "._:-".contains(c));
            if !well_formed {
                return Err(invalid("reference is letters, digits and . _ : -."));
            }
            Some(reference.to_string())
        }
    };
    Ok(Notice {
        topic: topic.to_string(),
        category,
        template,
        values,
        reference,
        record: None,
        mail: body.get("mail").and_then(Json_::as_bool).unwrap_or(false),
    })
}

/// Send a notice to a person or to every member of a segment (CT-183, the inbox half of CT-820),
/// for a caller who holds `messages-send`: `{ "recipient": { "principal": <id> } | { "segment":
/// <id> }, "topic", "category", "template", "values", "reference", "idempotency_key", "mail" }`.
/// A topic outside the grammar or outside `app.` is refused (NTF-1568). A send to a principal
/// makes the notice now and answers its delivery; a send to a segment is cut into jobs of at most
/// [`BATCH`] members each and answers counts only (EML-1561). The same key makes the same notices.
///
/// Implements: CT-183
pub async fn send(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        let sender = person(identity)?;
        same_tenant(app, identity, &body)?;
        let state = header(app, identity)?;
        require_capability(&state, identity, MESSAGES_SEND)?;
        let key = text(&body, "idempotency_key", 255)?;
        if !strata_storage_api::valid_idempotency_key(key) {
            return Err(invalid("idempotency_key is visible characters without spaces."));
        }
        let notice = notice_of(&body)?;
        let recipient = body
            .get("recipient")
            .and_then(Json_::as_object)
            .ok_or_else(|| invalid("The request needs recipient."))?;
        let tenant = &identity.tenant;
        let now = app.now();
        if let Some(principal) = recipient.get("principal").and_then(Json_::as_str) {
            let to = principal_of(principal)?;
            let delivery = deliver(app, tenant, &state, &to, key, &notice, now)?;
            return Ok(json!({ "recipients": 1, "delivery": delivery }));
        }
        if recipient.contains_key("address") {
            return Err(invalid("An address has no inbox."));
        }
        let segment: SegmentId = serde_json::from_value(
            recipient.get("segment").cloned().unwrap_or(Json_::Null),
        )
        .map_err(|_| invalid("recipient names a principal or a segment."))?;
        // The sender must be allowed to read the segment: the first page answers or refuses.
        let ctx = identity.context(app);
        kept::members(app.store.as_ref(), &ctx, segment, None, Some(1))
            .map_err(|e| ApiError::execute(e, identity.anonymous()))?;
        let total = kept::count(app.store.as_ref(), &ctx, segment)
            .map_err(|e| ApiError::execute(e, identity.anonymous()))?
            .count;
        let job = batch_job(tenant, key, &sender, segment, None, &notice, 1, now)?;
        app.store.jobs_create(tenant, &[job], now)?;
        Ok(json!({
            "recipients": total,
            "progress": {
                "send": canon::hash_bytes(format!("strata/notice-send/v1\0{tenant}\0{key}").as_bytes()),
                "state": "queued",
                "batches": total.div_ceil(u64::from(BATCH)),
            },
        }))
    })
    .await
}
