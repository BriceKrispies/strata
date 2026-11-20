//! The storage of the mail dispatcher: one block, kept apart from everything else it does.
//!
//! Each table is a kind of document in the tenant's own database, through the store's
//! small-document operations ([`strata_storage_api::Store::doc_put`] and the others), so a
//! tenant's mail is never another tenant's and a tenant's reset or removal takes its mail with
//! it:
//!
//! | kind | key | holds |
//! |---|---|---|
//! | [`MESSAGES`] | message id | the mail log (rec-mail-message): state, recipient, its address and the hash of it, template, category, class, send, reference, times; never a body (EML-011) |
//! | [`SUPPRESSIONS`] | address hash `/` category | the suppression list (rec-mail-suppression): a category name, `*` or `*promotional`, with the reason (D-6562) |
//! | [`ALLOWANCE`] | period `/` class, and `limit` | the allowance counters of each month by class (rec-mail-quota, D-6563), and the monthly allowance when an operator set one |
//! | [`CATEGORIES`] | category id | the categories of the tenant with their purpose (rec-message-category) |
//! | [`REPORTS_SEEN`] | hash of the provider event id | the delivery reports seen, for seven days (EML-1567) |
//! | [`BREAKER`] | `state` | the complaint breaker of the tenant (D-6564) |
//!
//! A message is kept for thirty days after its final state (ret-mail-message) and a seen report
//! for seven days: [`housekeep`] removes what has lapsed, at most once an hour for a tenant, from
//! the worker that sends its mail.
//!
//! Two limits of documents, moving to tables in WG-5216: the log is read whole to be listed,
//! counted and measured, and a counter is read and written back, so two passes of the worker for
//! the same tenant at the same moment can each count one message the other does not see. The
//! categories move into tenant metadata in WG-5217, and the erasure of a subject's log entries is
//! WG-5220.

use super::{AllowanceClass, MessageState, Purpose};
use crate::app::{App, LOG};
use serde::{Deserialize, Serialize};
use serde_json::json;
use strata_kernel::{canon, PrincipalId, TemplateId, TenantId, Timestamp};
use strata_storage_api::StoreResult;

/// The mail log.
pub const MESSAGES: &str = "mail-message";
/// The suppression list.
pub const SUPPRESSIONS: &str = "mail-suppression";
/// The allowance counters and the monthly allowance.
pub const ALLOWANCE: &str = "mail-allowance";
/// The message categories.
pub const CATEGORIES: &str = "mail-category";
/// The kind under which the inbox once read categories. Nothing writes it now; a tenant that holds
/// documents of it still has them read, as categories, when no document of [`CATEGORIES`] has the
/// same id (the one definition of a category is [`Category`]).
pub const LEGACY_CATEGORIES: &str = "message-category";
/// The delivery reports seen.
pub const REPORTS_SEEN: &str = "mail-report-seen";
/// The complaint breaker.
pub const BREAKER: &str = "mail-breaker";
/// When the tenant's lapsed entries were last removed, and the decoy of a sign-in request.
pub const HOUSEKEEPING: &str = "mail-housekeeping";

/// The monthly allowance of a tenant when none was set: the standard plan's 5,000 messages
/// (D-508).
pub const DEFAULT_MONTHLY_ALLOWANCE: u32 = 5_000;
/// How long a message is kept after its final state (ret-mail-message, D-6571).
pub const MESSAGE_RETENTION_MS: i64 = 30 * 86_400_000;
/// How long a delivery report's event id is remembered (D-6569).
pub const REPORT_MEMORY_MS: i64 = 7 * 86_400_000;
/// The most categories a tenant holds (define-message-category).
pub const MAX_CATEGORIES: usize = 50;

/// One message of the mail log (rec-mail-message).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageRecord {
    pub message: String,
    /// The SHA-256 of the idempotency key of the send.
    pub key: String,
    /// The principal it was sent to, when the send named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient: Option<PrincipalId>,
    /// The address, once known: the log keeps the recipient (EML-011) for its thirty days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// The hash of the address, which the suppression list is keyed by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address_hash: Option<String>,
    /// None for sign-in mail, which is not templated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<TemplateId>,
    pub state: MessageState,
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fail_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub class: AllowanceClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// The allowance period the message was counted in, once it was: a message is counted once
    /// however often it is attempted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counted: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// When it reached its final state; thirty days later it is removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_at: Option<i64>,
}

/// Why an address is suppressed (spec type `suppression-reason`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SuppressionReason {
    Bounce,
    Complaint,
    Unsubscribe,
}

impl SuppressionReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            SuppressionReason::Bounce => "bounce",
            SuppressionReason::Complaint => "complaint",
            SuppressionReason::Unsubscribe => "unsubscribe",
        }
    }
}

/// The category of a suppression of the whole address.
pub const ALL: &str = "*";
/// The category of a suppression of every promotional category.
pub const ALL_PROMOTIONAL: &str = "*promotional";

/// One entry of the suppression list (rec-mail-suppression).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suppression {
    pub address_hash: String,
    /// A category name, [`ALL`] or [`ALL_PROMOTIONAL`].
    pub category: String,
    pub reason: SuppressionReason,
    pub recorded_at: i64,
}

/// One category of the tenant (rec-message-category).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    /// The id; a legacy document that does not hold it is given the key it is kept under.
    #[serde(default)]
    pub category: String,
    pub purpose: Purpose,
    #[serde(default)]
    pub name: String,
    /// Raised by each change.
    #[serde(default)]
    pub version: u64,
}

/// What one month of the allowance has used, by class (sign-in is not counted).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub transactional: u32,
    pub bulk: u32,
}

/// The state of the complaint breaker of a tenant (D-6564).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BreakerState {
    pub bulk_held: bool,
    pub transactional_held: bool,
    /// When the rates were last measured.
    pub measured_at: i64,
}

fn read<T: for<'de> Deserialize<'de>>(
    app: &App,
    tenant: &TenantId,
    kind: &str,
    key: &str,
) -> StoreResult<Option<T>> {
    Ok(app
        .store
        .doc_get(tenant, kind, key)?
        .and_then(|text| serde_json::from_str(&text).ok()))
}

fn write<T: Serialize>(
    app: &App,
    tenant: &TenantId,
    kind: &str,
    key: &str,
    value: &T,
) -> StoreResult<()> {
    let text = serde_json::to_string(value).unwrap_or_else(|_| "null".into());
    app.store.doc_put(tenant, kind, key, &text, false)?;
    Ok(())
}

/// The hash an address is known by in the suppression list: the address trimmed and in lower
/// case, so the list does not disclose the addresses on it (rec-mail-suppression).
pub fn address_hash(address: &str) -> String {
    canon::hash_bytes(
        format!(
            "strata/mail-address/v1\0{}",
            address.trim().to_ascii_lowercase()
        )
        .as_bytes(),
    )
}

/// The provider reference recorded when a message is submitted, which a delivery report names
/// to reach its tenant and its message (EML-1567): `<tenant>:<message id>`.
pub fn provider_reference(tenant: &TenantId, message: &str) -> String {
    format!("{tenant}:{message}")
}

// ---- The mail log ----

pub fn message(app: &App, tenant: &TenantId, id: &str) -> StoreResult<Option<MessageRecord>> {
    read(app, tenant, MESSAGES, id)
}

pub fn put_message(app: &App, tenant: &TenantId, record: &MessageRecord) -> StoreResult<()> {
    write(app, tenant, MESSAGES, &record.message, record)
}

/// Every message the log holds, oldest first.
pub fn messages(app: &App, tenant: &TenantId) -> StoreResult<Vec<MessageRecord>> {
    let mut all: Vec<MessageRecord> = app
        .store
        .doc_list(tenant, MESSAGES)?
        .into_iter()
        .filter_map(|(_, text)| serde_json::from_str(&text).ok())
        .collect();
    all.sort_by(|a, b| (a.created_at, &a.message).cmp(&(b.created_at, &b.message)));
    Ok(all)
}

/// The decoy of a sign-in request for an address with no account: one write of the same kind of
/// table as the log entry of a real message, to one fixed key.
pub fn decoy_write(app: &App, tenant: &TenantId) {
    let _ = app
        .store
        .doc_put(tenant, HOUSEKEEPING, "decoy", "{}", false);
}

// ---- The suppression list ----

fn suppression_key(hash: &str, category: &str) -> String {
    format!("{hash}/{category}")
}

/// The entries that could stop a message of `category` to the address of `hash`: the whole
/// address, every promotional category and the category itself.
pub fn suppressions_of(
    app: &App,
    tenant: &TenantId,
    hash: &str,
    category: Option<&str>,
) -> StoreResult<Vec<Suppression>> {
    let mut keys = vec![ALL, ALL_PROMOTIONAL];
    if let Some(category) = category {
        keys.push(category);
    }
    let mut found = Vec::new();
    for key in keys {
        if let Some(entry) = read(app, tenant, SUPPRESSIONS, &suppression_key(hash, key))? {
            found.push(entry);
        }
    }
    Ok(found)
}

/// Add an entry. An unsubscribe does not replace a bounce or a complaint of the same address and
/// category, so a resubscribe cannot remove one (EML-1562).
pub fn add_suppression(app: &App, tenant: &TenantId, entry: &Suppression) -> StoreResult<()> {
    let key = suppression_key(&entry.address_hash, &entry.category);
    if entry.reason == SuppressionReason::Unsubscribe {
        if let Some(held) = read::<Suppression>(app, tenant, SUPPRESSIONS, &key)? {
            if held.reason != SuppressionReason::Unsubscribe {
                return Ok(());
            }
        }
    }
    write(app, tenant, SUPPRESSIONS, &key, entry)
}

/// Remove the entry of an address and category; with `unsubscribe_only`, only one whose reason
/// is unsubscribe (a resubscribe, EML-1562). Returns whether one was removed.
pub fn remove_suppression(
    app: &App,
    tenant: &TenantId,
    hash: &str,
    category: &str,
    unsubscribe_only: bool,
) -> StoreResult<bool> {
    let key = suppression_key(hash, category);
    let Some(held) = read::<Suppression>(app, tenant, SUPPRESSIONS, &key)? else {
        return Ok(false);
    };
    if unsubscribe_only && held.reason != SuppressionReason::Unsubscribe {
        return Ok(false);
    }
    app.store.doc_remove(tenant, SUPPRESSIONS, &key)
}

/// Whether any entry of the list names `category`.
pub fn category_suppressed(app: &App, tenant: &TenantId, category: &str) -> StoreResult<bool> {
    let suffix = format!("/{category}");
    Ok(app
        .store
        .doc_list(tenant, SUPPRESSIONS)?
        .iter()
        .any(|(key, _)| key.ends_with(&suffix)))
}

// ---- The allowance ----

fn usage_key(period: &str, class: AllowanceClass) -> String {
    format!("{period}/{}", class.as_str())
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Counter {
    count: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct Limit {
    monthly: u32,
}

/// What the month `period` has used.
pub fn usage(app: &App, tenant: &TenantId, period: &str) -> StoreResult<Usage> {
    let count = |class| -> StoreResult<u32> {
        Ok(
            read::<Counter>(app, tenant, ALLOWANCE, &usage_key(period, class))?
                .map_or(0, |c| c.count),
        )
    };
    Ok(Usage {
        transactional: count(AllowanceClass::Transactional)?,
        bulk: count(AllowanceClass::Bulk)?,
    })
}

/// Count one message of `class` in the month `period`. Sign-in mail is not counted.
pub fn count_use(
    app: &App,
    tenant: &TenantId,
    period: &str,
    class: AllowanceClass,
) -> StoreResult<()> {
    if class == AllowanceClass::SignIn {
        return Ok(());
    }
    let key = usage_key(period, class);
    let mut counter = read::<Counter>(app, tenant, ALLOWANCE, &key)?.unwrap_or_default();
    counter.count = counter.count.saturating_add(1);
    write(app, tenant, ALLOWANCE, &key, &counter)
}

/// The monthly allowance of the tenant: the one an operator set, or the standard plan's.
pub fn allowance_limit(app: &App, tenant: &TenantId) -> StoreResult<u32> {
    Ok(read::<Limit>(app, tenant, ALLOWANCE, "limit")?
        .map_or(DEFAULT_MONTHLY_ALLOWANCE, |l| l.monthly))
}

/// Set the monthly allowance of the tenant (an operator's act; the plans of D-508 are not yet
/// read from a billing record).
pub fn set_allowance_limit(app: &App, tenant: &TenantId, monthly: u32) -> StoreResult<()> {
    write(app, tenant, ALLOWANCE, "limit", &Limit { monthly })
}

// ---- Categories ----

/// The category `id`, from the one store the dispatcher and the inbox share; a document of the
/// legacy kind is read when there is none of [`CATEGORIES`] for the id.
pub fn category(app: &App, tenant: &TenantId, id: &str) -> StoreResult<Option<Category>> {
    if let Some(found) = read::<Category>(app, tenant, CATEGORIES, id)? {
        return Ok(Some(found));
    }
    Ok(
        read::<Category>(app, tenant, LEGACY_CATEGORIES, id)?.map(|mut found| {
            found.category = id.to_string();
            found
        }),
    )
}

/// Every category of the tenant, each once: a legacy document is listed only when no document of
/// [`CATEGORIES`] has its id.
pub fn categories(app: &App, tenant: &TenantId) -> StoreResult<Vec<Category>> {
    let mut all: Vec<Category> = app
        .store
        .doc_list(tenant, CATEGORIES)?
        .into_iter()
        .filter_map(|(_, text)| serde_json::from_str(&text).ok())
        .collect();
    for (key, text) in app.store.doc_list(tenant, LEGACY_CATEGORIES)? {
        if all.iter().any(|c| c.category == key) {
            continue;
        }
        if let Ok(mut found) = serde_json::from_str::<Category>(&text) {
            found.category = key;
            all.push(found);
        }
    }
    Ok(all)
}

pub fn put_category(app: &App, tenant: &TenantId, category: &Category) -> StoreResult<()> {
    write(app, tenant, CATEGORIES, &category.category, category)
}

// ---- Delivery reports ----

#[derive(Debug, Serialize, Deserialize)]
struct Seen {
    at: i64,
}

/// Remember the provider event id `event` as seen at `now`; `false` when it was seen within the
/// last seven days, and then nothing changes (EML-1567).
pub fn first_sighting(
    app: &App,
    tenant: &TenantId,
    event: &str,
    now: Timestamp,
) -> StoreResult<bool> {
    let key = canon::hash_bytes(format!("strata/mail-report/v1\0{event}").as_bytes());
    if let Some(seen) = read::<Seen>(app, tenant, REPORTS_SEEN, &key)? {
        if now.0 - seen.at < REPORT_MEMORY_MS {
            return Ok(false);
        }
    }
    write(app, tenant, REPORTS_SEEN, &key, &Seen { at: now.0 })?;
    Ok(true)
}

// ---- The breaker ----

pub fn breaker(app: &App, tenant: &TenantId) -> StoreResult<BreakerState> {
    Ok(read(app, tenant, BREAKER, "state")?.unwrap_or_default())
}

pub fn put_breaker(app: &App, tenant: &TenantId, state: &BreakerState) -> StoreResult<()> {
    write(app, tenant, BREAKER, "state", state)
}

// ---- Retention ----

#[derive(Debug, Serialize, Deserialize)]
struct Swept {
    at: i64,
}

/// Remove the messages whose final state is thirty days old and the reports seen more than seven
/// days ago, at most once an hour for a tenant. Returns how many entries were removed.
pub fn housekeep(app: &App, tenant: &TenantId, now: Timestamp) -> StoreResult<u64> {
    if let Some(swept) = read::<Swept>(app, tenant, HOUSEKEEPING, "swept")? {
        if now.0 - swept.at < 3_600_000 {
            return Ok(0);
        }
    }
    write(app, tenant, HOUSEKEEPING, "swept", &Swept { at: now.0 })?;
    let mut removed = 0u64;
    for (key, text) in app.store.doc_list(tenant, MESSAGES)? {
        let lapsed = serde_json::from_str::<MessageRecord>(&text)
            .ok()
            .and_then(|record| record.final_at)
            .is_some_and(|at| now.0 - at >= MESSAGE_RETENTION_MS);
        if lapsed && app.store.doc_remove(tenant, MESSAGES, &key)? {
            removed += 1;
        }
    }
    for (key, text) in app.store.doc_list(tenant, REPORTS_SEEN)? {
        let lapsed = serde_json::from_str::<Seen>(&text)
            .ok()
            .is_some_and(|seen| now.0 - seen.at >= REPORT_MEMORY_MS);
        if lapsed && app.store.doc_remove(tenant, REPORTS_SEEN, &key)? {
            removed += 1;
        }
    }
    if removed > 0 {
        LOG.info(
            "mail.purged",
            "",
            json!({ "tenant": tenant.as_str(), "removed": removed }),
        );
    }
    Ok(removed)
}
