//! The limit store's first working slice (component `limit-store`, C-100): request quotas from the
//! specification, the plan a tenant holds and what it allows, feature flags, and what a tenant has
//! used.
//!
//! **Request quotas.** Every served contract that has a `quotas` entry is limited by it. The key,
//! the limit and the window come from the generated table `strata_model::quotas`; nothing here
//! restates a number. [`admit`] is the one enforcement point, run by the gate in front of every
//! internal route: it finds the contract of the route ([`ROUTES`]), takes every quota of that
//! contract and counts the call against each in the scope the quota states (source address, with
//! an IPv6 address counted as its /64; principal; tenant). A call beyond a quota is refused as
//! `throttled` with a `Retry-After`; a quota that queues or throttles instead holds the call for
//! a short while when the window is about to end, and refuses it otherwise. A new contract with a
//! quota entry is enforced once its route is in [`ROUTES`], with no other code.
//!
//! A window of a minute or less is counted in memory of the process. A longer window is counted
//! in the tenant's database (documents of kind [`COUNTER_KIND`], as the login throttle rows are),
//! so that a restart keeps the count. Which counters are shared and which are per process is
//! stated in the specification (decision on the limit store) and in [`Quotas`].
//!
//! **Plans.** A tenant holds one plan, recorded in the control database's directory row
//! ([`strata_storage_api::Store::tenant_plan`]). [`plan_allowance`] holds the starting values of
//! D-508 and D-700; a tenant's own value, set by an audited operation, is a document of kind
//! [`ALLOWANCE_KIND`] and takes effect at once. The allowances are enforced where the bounded
//! thing is made: stored bytes before a write ([`check_write`], BIL-151), jobs in the commit that
//! creates them ([`JobAdmitter`], D-6533), segments when one is defined ([`check_new_segment`]),
//! mail through [`mail_admit`] for the dispatcher. [`entitlement`] is the plan and the flag
//! settings of a tenant as the generated authorizer's `AuthHoles::entitlement` asks for them.
//!
//! **Flags.** A flag is on for a tenant only when its release record enables it for the tenant and
//! the tenant's own switch is on (OPS-328, D-6202); [`set_switch`] is `set-feature-flag`.
//!
//! Implements: BIL-150, BIL-151, EDG-013, OPS-328, AGT-420, SCH-302, EML-021

use crate::app::{ApiError, App, Shared, LOG};
use crate::auth::{self, Identity};
use axum::extract::State;
use axum::http::header::RETRY_AFTER;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value as Json_};
use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use strata_kernel::ext::*;
use strata_kernel::segments::SegmentId;
use strata_kernel::{canon, Check, Context, Intent, TenantId, Timestamp};
use strata_model::prelude::Timestamp as ModelTime;
use strata_model::quotas::{self, Counter, OnExceed, Quota, QuotaOutcome, Scope};
use strata_model::routes::{Entitlement, CAPABILITIES, FLAGS, PLANS};
use strata_protocol::api::ErrorCode;
use strata_runtime::limits::{source_key, MAX_KEYS};
use strata_storage_api::admission::{
    place_jobs, JobAdmission, JobLimits, Placement, DAY_MS, MINUTE_MS,
};
use strata_storage_api::jobs::{ExceptionRecord, NewJob};
use strata_storage_api::StoreError;

/// The kind of tenant document that holds the count of a quota with a long window, keyed by the
/// quota and the scope.
pub const COUNTER_KIND: &str = "quota-counter";
/// The kind of tenant document that holds what a tenant has used of an allowance in a period.
pub const USAGE_KIND: &str = "quota-usage";
/// The kind of tenant document that holds a value of an allowance that replaces its plan's.
pub const ALLOWANCE_KIND: &str = "quota-allowance";
/// The kind of tenant document that holds the tenant's switch of a feature flag.
pub const SWITCH_KIND: &str = "feature-switch";
/// The kind of tenant document that holds the release record's decision for the tenant (the
/// platform record `rec-feature-flag`, written by the release controller).
pub const RELEASE_KIND: &str = "feature-release";

/// A window of this length or less is counted in memory of the process; a longer one is stored.
pub const SHORT_WINDOW: Duration = Duration::from_secs(60);
/// The longest a call is held by a quota that queues or throttles; a call that would wait longer is
/// refused with the time to wait.
pub const HOLD_LIMIT: Duration = Duration::from_millis(1_000);
/// The plan a tenant holds when none is recorded.
pub const DEFAULT_PLAN: &str = "standard";

/// The contract each internal route serves, for every route that serves one. A contract with a
/// quota entry is limited when its route is here.
pub const ROUTES: &[(&str, &str)] = &[
    ("/internal/session/login", "sign-in-with-password"),
    (
        "/internal/session/sign-in/password",
        "sign-in-with-password",
    ),
    ("/internal/session/sign-in/link", "sign-in-with-link"),
    ("/internal/session/sign-in/passkey", "sign-in-with-passkey"),
    ("/internal/session/second-factor", "verify-second-factor"),
    ("/internal/sign-in/link/request", "request-sign-in-link"),
    ("/internal/passkeys/challenge", "issue-passkey-challenge"),
    ("/internal/sign-in/oidc/providers", "list-sign-in-providers"),
    ("/internal/sign-in/oidc/start", "start-oidc-sign-in"),
    ("/internal/sign-in/saml/providers", "list-sign-in-providers"),
    ("/internal/sign-in/saml/start", "start-saml-sign-in"),
    ("/internal/sign-in/saml/metadata", "serve-saml-metadata"),
    ("/internal/sign-in/invite/accept", "accept-owner-invitation"),
    ("/internal/recovery/request", "recover-account"),
    ("/internal/recovery/complete", "complete-recovery"),
    ("/internal/recovery/cancel", "cancel-recovery"),
    ("/internal/aggregate", "aggregate-run"),
    ("/internal/view", "read-view"),
    ("/internal/lab/run", "run-lab-projection"),
    ("/internal/lab/verify", "verify-lab-proofs"),
    ("/internal/token", "issue-access-token"),
];

/// The contract a route serves, when it serves one.
pub fn contract_of_route(path: &str) -> Option<&'static str> {
    ROUTES
        .iter()
        .find(|(route, _)| *route == path)
        .map(|(_, contract)| *contract)
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------------------------------
// Request quotas
// ---------------------------------------------------------------------------------------

/// The counters of the quotas.
///
/// Per process: the counts of windows of a minute or less (`short`), and the order in which the
/// durable counters of this process are read and written (`durable`). Shared by every process that
/// uses the store: the counts of windows longer than a minute, the usage of the job, mail and
/// storage allowances, and the allowances, plans and switches themselves. A durable count is read,
/// decided and written without a lock in the database, so two processes counting one key at the
/// same moment may let a call or two past a limit; the shared limit store that closes this is its
/// own task.
#[derive(Default)]
pub struct Quotas {
    short: Mutex<BTreeMap<(&'static str, String), Held>>,
    durable: Mutex<()>,
    usage: Mutex<()>,
}

/// One in-memory count: the counter and when its window ends.
#[derive(Clone, Copy)]
struct Held {
    counter: Counter,
    ends: i64,
}

impl Quotas {
    pub fn new() -> Self {
        Self::default()
    }

    /// Count one call against `quota` for `key` at `now_ms` and say whether it is within the
    /// limit. The decision is the generated `Quota::check`. A window longer than a minute is
    /// counted in the database of `tenant` when there is one that holds it, and otherwise in
    /// memory.
    pub fn count(
        &self,
        app: &App,
        quota: &'static Quota,
        key: &str,
        tenant: Option<&TenantId>,
        now_ms: i64,
    ) -> QuotaOutcome {
        if quota.window > SHORT_WINDOW {
            if let Some(tenant) = tenant {
                match self.count_stored(app, quota, key, tenant, now_ms) {
                    Ok(outcome) => return outcome,
                    Err(error) => LOG.warn(
                        "quota.counter_unavailable",
                        "",
                        json!({ "quota": quota.id, "error": error }),
                    ),
                }
            }
        }
        self.count_memory(quota, key, now_ms)
    }

    fn count_memory(&self, quota: &'static Quota, key: &str, now_ms: i64) -> QuotaOutcome {
        let now = ModelTime(now_ms);
        let mut table = locked(&self.short);
        let identity = (quota.id, key.to_string());
        let outcome = quota.check(table.get(&identity).map(|held| held.counter), now);
        if let QuotaOutcome::Allowed(counter) = outcome {
            if !table.contains_key(&identity) && table.len() >= MAX_KEYS {
                // Full: forget what has ended, and if nothing has, the window that ends first.
                table.retain(|_, held| held.ends > now_ms);
                if table.len() >= MAX_KEYS {
                    if let Some(oldest) = table
                        .iter()
                        .min_by_key(|(_, held)| held.ends)
                        .map(|(identity, _)| identity.clone())
                    {
                        table.remove(&oldest);
                    }
                }
            }
            let ends = counter
                .window_start
                .0
                .saturating_add(i64::try_from(quota.window.as_millis()).unwrap_or(i64::MAX));
            table.insert(identity, Held { counter, ends });
        }
        outcome
    }

    fn count_stored(
        &self,
        app: &App,
        quota: &'static Quota,
        key: &str,
        tenant: &TenantId,
        now_ms: i64,
    ) -> Result<QuotaOutcome, String> {
        let _order = locked(&self.durable);
        let name = format!("{}|{key}", quota.id);
        let held = app
            .store
            .doc_get(tenant, COUNTER_KIND, &name)
            .map_err(|error| error.to_string())?;
        let outcome = quota.check(held.as_deref().and_then(parse_counter), ModelTime(now_ms));
        if let QuotaOutcome::Allowed(counter) = outcome {
            let ends = counter
                .window_start
                .0
                .saturating_add(i64::try_from(quota.window.as_millis()).unwrap_or(i64::MAX));
            let text = json!({
                "start": counter.window_start.0,
                "count": counter.count,
                "ends": ends,
            })
            .to_string();
            app.store
                .doc_put(tenant, COUNTER_KIND, &name, &text, false)
                .map_err(|error| error.to_string())?;
        }
        Ok(outcome)
    }

    /// The count `key` holds under `quota` at `now_ms`, without counting.
    pub fn held(
        &self,
        app: &App,
        quota: &'static Quota,
        key: &str,
        tenant: Option<&TenantId>,
        now_ms: i64,
    ) -> u64 {
        let live = |counter: Counter| {
            let window = i64::try_from(quota.window.as_millis()).unwrap_or(i64::MAX);
            (now_ms >= counter.window_start.0 && now_ms - counter.window_start.0 < window)
                .then_some(counter.count)
        };
        if quota.window > SHORT_WINDOW {
            if let Some(tenant) = tenant {
                let name = format!("{}|{key}", quota.id);
                if let Ok(Some(text)) = app.store.doc_get(tenant, COUNTER_KIND, &name) {
                    return parse_counter(&text).and_then(live).unwrap_or(0);
                }
            }
        }
        locked(&self.short)
            .get(&(quota.id, key.to_string()))
            .and_then(|held| live(held.counter))
            .unwrap_or(0)
    }
}

fn parse_counter(text: &str) -> Option<Counter> {
    let value: Json_ = serde_json::from_str(text).ok()?;
    Some(Counter {
        window_start: ModelTime(value.get("start")?.as_i64()?),
        count: value.get("count")?.as_u64()?,
    })
}

/// The key a call counts under in the scope of `quota`: a source address as its /64 for IPv6, the
/// principal (the session, until the request has been identified, else the source), or the tenant.
pub fn scope_key(
    quota: &Quota,
    tenant: Option<&TenantId>,
    source: Option<&str>,
    session: Option<&str>,
) -> String {
    match quota.scope {
        Scope::SourceAddress => source_key(source.unwrap_or("unknown")),
        Scope::Tenant => tenant.map_or_else(|| "platform".to_string(), |t| t.as_str().to_string()),
        Scope::Principal => match session {
            Some(token) => format!("session:{}", auth::token_hash(token)),
            None => source_key(source.unwrap_or("unknown")),
        },
    }
}

/// The reply to a call a quota refuses: `throttled`, naming the quota and its limit, with the
/// seconds after which the next call is permitted in `Retry-After`.
pub fn refusal(quota: &Quota, retry_after: Duration, correlation: &str) -> Response {
    let seconds = retry_after.as_secs() + u64::from(retry_after.subsec_nanos() > 0);
    let seconds = seconds.max(1);
    let mut error = ApiError::new(
        ErrorCode::Throttled,
        format!(
            "Too many requests: {} in {} seconds is the limit ({}). The next request is permitted in {seconds} seconds.",
            quota.limit,
            quota.window.as_secs(),
            quota.id,
        ),
    );
    error.0.kind = Some(quota.id.to_string());
    let mut response = error.with_correlation(correlation).into_response();
    response
        .headers_mut()
        .insert(RETRY_AFTER, HeaderValue::from(seconds));
    response
}

/// The one enforcement point of request quotas: counts the call at `path` against every quota of
/// the contract it serves, and answers the refusal when one is exceeded. A route that serves no
/// contract with a quota is not counted. A counter that cannot be reached lets the call through
/// (and is logged): a limit that cannot be counted must not take the service down.
pub async fn admit(app: &Shared, path: &str, headers: &HeaderMap) -> Result<(), Response> {
    let Some(contract) = contract_of_route(path) else {
        return Ok(());
    };
    let limits: Vec<&'static Quota> = quotas::for_contract(contract).collect();
    if limits.is_empty() {
        return Ok(());
    }
    // A request the route itself will refuse for its form (no site, a bad key) is not counted.
    let Ok(caller) = auth::caller(headers) else {
        return Ok(());
    };
    // A quota of a tenant or of a principal counts the calls of the tenant's signed-in callers
    // only: a call without a session of this tenant is refused by the operation itself, and
    // counting it would let anyone spend a tenant's allowance (a visitor flooding the route would
    // shut its owner out). A quota of a source address counts every call.
    let personal = limits
        .iter()
        .any(|quota| matches!(quota.scope, Scope::Tenant | Scope::Principal));
    let signed_in = personal && {
        let resolving = caller.clone();
        app.blocking_for(caller.site.as_str(), move |app| {
            auth::identify(app, &resolving).is_ok_and(|identity| !identity.anonymous())
        })
        .await
        .unwrap_or(false)
    };
    for quota in limits {
        if matches!(quota.scope, Scope::Tenant | Scope::Principal) && !signed_in {
            continue;
        }
        let key = scope_key(
            quota,
            Some(&caller.site),
            caller.visitor_source(),
            caller.token.as_deref(),
        );
        let mut outcome = count_call(app, quota, &key, &caller.site).await;
        if let QuotaOutcome::Exceeded { retry_after, .. } = outcome {
            let holds = matches!(quota.on_exceed, OnExceed::Queue | OnExceed::Throttle);
            if holds && retry_after <= HOLD_LIMIT {
                tokio::time::sleep(retry_after).await;
                outcome = count_call(app, quota, &key, &caller.site).await;
            }
        }
        if let QuotaOutcome::Exceeded { retry_after, .. } = outcome {
            LOG.info(
                "quota.refused",
                &caller.correlation,
                json!({ "quota": quota.id, "contract": contract }),
            );
            return Err(refusal(quota, retry_after, &caller.correlation));
        }
    }
    Ok(())
}

async fn count_call(
    app: &Shared,
    quota: &'static Quota,
    key: &str,
    tenant: &TenantId,
) -> QuotaOutcome {
    let now_ms = app.now().0;
    if quota.window <= SHORT_WINDOW {
        return app.quota.count(app, quota, key, None, now_ms);
    }
    let (key, tenant) = (key.to_string(), tenant.clone());
    match app
        .blocking(move |app| app.quota.count(app, quota, &key, Some(&tenant), now_ms))
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            LOG.warn(
                "quota.counter_unavailable",
                "",
                json!({ "quota": quota.id, "error": error.describe() }),
            );
            QuotaOutcome::Allowed(Counter {
                window_start: ModelTime(now_ms),
                count: 0,
            })
        }
    }
}

/// Remove counters whose window has ended, and usage of periods that are over. Returns how many
/// documents were removed.
pub fn purge(app: &App, now: Timestamp) -> Result<u64, StoreError> {
    let mut removed = 0;
    let (day, minute) = (now.0.div_euclid(DAY_MS), now.0.div_euclid(MINUTE_MS));
    let month = month_index(now.0);
    for summary in app.store.tenants()? {
        for (key, value) in app.store.doc_list(&summary.id, COUNTER_KIND)? {
            let ended = serde_json::from_str::<Json_>(&value)
                .ok()
                .and_then(|v| v.get("ends").and_then(Json_::as_i64))
                .is_none_or(|ends| ends <= now.0);
            if ended && app.store.doc_remove(&summary.id, COUNTER_KIND, &key)? {
                removed += 1;
            }
        }
        for (key, _) in app.store.doc_list(&summary.id, USAGE_KIND)? {
            let over = match key.split_once(':') {
                Some(("day", n)) => n.parse::<i64>().is_ok_and(|n| n < day - 1),
                Some(("minute", n)) => n.parse::<i64>().is_ok_and(|n| n < minute - 2),
                Some((name, n)) if name.starts_with("mail-") => {
                    n.parse::<i64>().is_ok_and(|n| n < month - 1)
                }
                _ => false,
            };
            if over && app.store.doc_remove(&summary.id, USAGE_KIND, &key)? {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

// ---------------------------------------------------------------------------------------
// Plans and allowances
// ---------------------------------------------------------------------------------------

/// Something a plan bounds and a tenant uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Quantity {
    /// Bytes the tenant's data takes in the store.
    StoredBytes,
    /// Records the tenant holds, of every object type together.
    StoredRecords,
    /// Jobs created in a quota day.
    JobsPerDay,
    /// Jobs released in a minute.
    JobsPerMinute,
    /// Segments the tenant defines.
    Segments,
    /// Bulk messages sent in a calendar month.
    MailPerMonth,
}

impl Quantity {
    pub const ALL: [Quantity; 6] = [
        Quantity::StoredBytes,
        Quantity::JobsPerDay,
        Quantity::JobsPerMinute,
        Quantity::Segments,
        Quantity::MailPerMonth,
        Quantity::StoredRecords,
    ];

    /// The name the quantity has in replies and in the audited operation that changes it.
    pub fn name(self) -> &'static str {
        match self {
            Quantity::StoredBytes => "stored-bytes",
            Quantity::StoredRecords => "stored-records",
            Quantity::JobsPerDay => "jobs-per-day",
            Quantity::JobsPerMinute => "jobs-per-minute",
            Quantity::Segments => "segments",
            Quantity::MailPerMonth => "mail-per-month",
        }
    }

    pub fn parse(name: &str) -> Option<Quantity> {
        Quantity::ALL.into_iter().find(|q| q.name() == name)
    }
}

const GB: u64 = 1_000_000_000;

/// What a plan allows of a quantity, `None` for no limit: the starting values of D-508 (stored
/// bytes, jobs a day, mail a month; Enterprise is set by contract, so it holds no limit until one
/// is recorded), the job admission rate of D-700 and the segments a tenant may hold (SEG-029).
pub fn plan_allowance(plan: &str, quantity: Quantity) -> Option<u64> {
    match (plan, quantity) {
        (_, Quantity::JobsPerMinute) => Some(100),
        (_, Quantity::Segments) => Some(strata_kernel::segments::MAX_SEGMENTS as u64),
        ("enterprise", _) => None,
        ("business", Quantity::StoredRecords) => Some(10_000_000),
        (_, Quantity::StoredRecords) => Some(1_000_000),
        ("business", Quantity::StoredBytes) => Some(100 * GB),
        ("business", Quantity::JobsPerDay) => Some(100_000),
        ("business", Quantity::MailPerMonth) => Some(50_000),
        (_, Quantity::StoredBytes) => Some(10 * GB),
        (_, Quantity::JobsPerDay) => Some(10_000),
        (_, Quantity::MailPerMonth) => Some(5_000),
    }
}

/// The plan a tenant holds.
pub fn plan_of(app: &App, tenant: &TenantId) -> Result<String, StoreError> {
    Ok(app
        .store
        .tenant_plan(tenant)?
        .unwrap_or_else(|| DEFAULT_PLAN.to_string()))
}

/// What a tenant may of a quantity now: its own value when one was set, else its plan's.
pub fn allowance(
    app: &App,
    tenant: &TenantId,
    quantity: Quantity,
) -> Result<Option<u64>, StoreError> {
    if let Some(text) = app.store.doc_get(tenant, ALLOWANCE_KIND, quantity.name())? {
        return Ok(text.parse::<u64>().ok());
    }
    Ok(plan_allowance(&plan_of(app, tenant)?, quantity))
}

/// Set a tenant's own value of an allowance (`None` is no limit), as an audited operation. It
/// takes effect on the next request, with no restart (D-508, AGT-420).
pub fn set_allowance(
    app: &App,
    tenant: &TenantId,
    quantity: Quantity,
    value: Option<u64>,
    by: &str,
    correlation: &str,
) -> Result<(), StoreError> {
    let previous = allowance(app, tenant, quantity)?;
    let text = value.map_or_else(|| "unlimited".to_string(), |v| v.to_string());
    app.store
        .doc_put(tenant, ALLOWANCE_KIND, quantity.name(), &text, false)?;
    app.audit(
        "quota.changed",
        correlation,
        json!({
            "tenant": tenant.as_str(),
            "quota": quantity.name(),
            "previous": previous,
            "value": value,
            "by": by,
        }),
    );
    Ok(())
}

/// Give a tenant its plan's value of an allowance again.
pub fn clear_allowance(app: &App, tenant: &TenantId, quantity: Quantity) -> Result<(), StoreError> {
    app.store
        .doc_remove(tenant, ALLOWANCE_KIND, quantity.name())?;
    Ok(())
}

fn usage_number(app: &App, tenant: &TenantId, key: &str) -> u64 {
    app.store
        .doc_get(tenant, USAGE_KIND, key)
        .ok()
        .flatten()
        .and_then(|text| text.parse().ok())
        .unwrap_or(0)
}

fn bump(app: &App, tenant: &TenantId, key: &str, by: u64) {
    let _order = locked(&app.quota.usage);
    let next = usage_number(app, tenant, key).saturating_add(by);
    if let Err(error) = app
        .store
        .doc_put(tenant, USAGE_KIND, key, &next.to_string(), false)
    {
        LOG.warn(
            "quota.usage_unrecorded",
            "",
            json!({ "tenant": tenant.as_str(), "key": key, "error": error.to_string() }),
        );
    }
}

/// What a tenant has used of a quantity now.
pub fn used(
    app: &App,
    tenant: &TenantId,
    quantity: Quantity,
    now: Timestamp,
) -> Result<u64, StoreError> {
    Ok(match quantity {
        Quantity::StoredBytes => app.store.stored_bytes(tenant)?,
        Quantity::StoredRecords => stored_records(app, tenant)?,
        Quantity::JobsPerDay => {
            usage_number(app, tenant, &format!("day:{}", now.0.div_euclid(DAY_MS)))
        }
        Quantity::JobsPerMinute => usage_number(
            app,
            tenant,
            &format!("minute:{}", now.0.div_euclid(MINUTE_MS)),
        ),
        Quantity::Segments => app.store.segment_catalog(tenant)?.len() as u64,
        Quantity::MailPerMonth => mail_used(app, tenant, MailClass::Bulk, now.0),
    })
}

/// The records a tenant holds, of every object type.
fn stored_records(app: &App, tenant: &TenantId) -> Result<u64, StoreError> {
    Ok(app
        .store
        .tenant_summary(tenant)?
        .map_or(0, |summary| summary.records))
}

/// A refusal that names the quota it is for (BIL-151).
fn over_quota(quantity: Quantity, used: u64, allowed: u64) -> ApiError {
    let message = format!(
        "This site is at its plan's limit of {} ({used} used of {allowed}).",
        quantity.name()
    );
    let mut error = ApiError::new(ErrorCode::Forbidden, message.clone());
    error.0.kind = Some("QuotaExceeded".to_string());
    error
        .0
        .failed
        .push(Check::fail(format!("quota:{}", quantity.name()), message));
    error
}

/// Whether an intent gives something back or only ends access, and so is never refused for want
/// of room: deleting a record, erasing a subject, and revoking a role, a team role, a team member
/// or an invitation.
fn frees_room(intent: &Intent) -> bool {
    matches!(
        intent,
        Intent::DeleteRecord { .. }
            | Intent::EraseSubject { .. }
            | Intent::RevokeRole { .. }
            | Intent::RevokeTeamRole { .. }
            | Intent::RemoveTeamMember { .. }
            | Intent::RevokeInvitation { .. }
    )
}

/// Refuse a write that would take the tenant past its stored-records or stored-bytes allowance, naming the quota
/// (BIL-151). Reads and deletes are not writes of this kind. The size of the write is the size of
/// the intent, an estimate that errs on the side of room; the measure is the database's size.
pub fn check_write(app: &App, ctx: &Context, intent: &Intent) -> Result<(), ApiError> {
    if frees_room(intent) || matches!(intent, Intent::CreateTenant { .. }) {
        return Ok(());
    }
    // A write that makes a record is refused at the plan's record limit; nothing else adds one.
    if matches!(intent, Intent::CreateRecord { .. }) {
        if let Some(allowed) = allowance(app, &ctx.tenant, Quantity::StoredRecords)? {
            let held = stored_records(app, &ctx.tenant)?;
            if held >= allowed {
                return Err(over_quota(Quantity::StoredRecords, held, allowed));
            }
        }
    }
    let Some(allowed) = allowance(app, &ctx.tenant, Quantity::StoredBytes)? else {
        return Ok(());
    };
    let held = app.store.stored_bytes(&ctx.tenant)?;
    let size = canon::canonical_json(intent).len() as u64;
    if held.saturating_add(size) > allowed {
        return Err(over_quota(Quantity::StoredBytes, held, allowed));
    }
    Ok(())
}

/// Refuse a segment that would make the tenant hold more than its allowance. A segment that is
/// redefined does not count twice.
pub fn check_new_segment(app: &App, tenant: &TenantId, segment: SegmentId) -> Result<(), ApiError> {
    let Some(allowed) = allowance(app, tenant, Quantity::Segments)? else {
        return Ok(());
    };
    let catalog = app.store.segment_catalog(tenant)?;
    if catalog.iter().any(|held| held.id == segment) {
        return Ok(());
    }
    let held = catalog.len() as u64;
    if held >= allowed {
        return Err(over_quota(Quantity::Segments, held, allowed));
    }
    Ok(())
}

/// Hold the jobs a commit creates to the tenant's allowance (D-6533): the commit always goes
/// through; a job past the daily allowance is due at the start of the next quota day and writes a
/// held exception record, and a job past the admission rate is due at the next minute with room.
pub struct JobAdmitter<'a> {
    pub app: &'a App,
}

impl JobAdmission for JobAdmitter<'_> {
    fn admit(
        &self,
        tenant: &TenantId,
        jobs: &mut Vec<NewJob>,
        exceptions: &mut Vec<ExceptionRecord>,
        now: Timestamp,
    ) -> Vec<Placement> {
        if jobs.is_empty() {
            return Vec::new();
        }
        let app = self.app;
        let (per_day, per_minute) = match (
            allowance(app, tenant, Quantity::JobsPerDay),
            allowance(app, tenant, Quantity::JobsPerMinute),
        ) {
            (Ok(day), Ok(minute)) => (day, minute),
            (day, minute) => {
                LOG.warn(
                    "quota.jobs_unchecked",
                    "",
                    json!({
                        "tenant": tenant.as_str(),
                        "error": day.err().or(minute.err()).map(|e| e.to_string()),
                    }),
                );
                (None, None)
            }
        };
        let used_in_day = |day: i64| usage_number(app, tenant, &format!("day:{day}"));
        let used_in_minute = |minute: i64| usage_number(app, tenant, &format!("minute:{minute}"));
        let (placements, held) = place_jobs(
            tenant,
            jobs,
            now,
            &JobLimits {
                per_day,
                per_minute,
                used_in_day: &used_in_day,
                used_in_minute: &used_in_minute,
            },
        );
        exceptions.extend(held);
        placements
    }

    fn committed(&self, tenant: &TenantId, placements: &[Placement]) {
        let mut days: BTreeMap<i64, u64> = BTreeMap::new();
        let mut minutes: BTreeMap<i64, u64> = BTreeMap::new();
        for placement in placements {
            *days.entry(placement.day).or_insert(0) += 1;
            *minutes.entry(placement.minute).or_insert(0) += 1;
        }
        for (day, count) in days {
            bump(self.app, tenant, &format!("day:{day}"), count);
        }
        for (minute, count) in minutes {
            bump(self.app, tenant, &format!("minute:{minute}"), count);
        }
    }
}

// ---------------------------------------------------------------------------------------
// Mail
// ---------------------------------------------------------------------------------------

/// The class of a message for the mail quota (EML-021): sign-in mail is never held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailClass {
    Bulk,
    SignIn,
}

impl MailClass {
    fn key(self) -> &'static str {
        match self {
            MailClass::Bulk => "mail-bulk",
            MailClass::SignIn => "mail-sign-in",
        }
    }
}

/// Whether a message may leave now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailVerdict {
    Send,
    /// Held until the start of the next calendar month (UTC), in milliseconds since the epoch.
    Hold {
        release_at_ms: i64,
    },
}

/// The month number (year times twelve plus the month, from zero) of a time.
pub fn month_index(ms: i64) -> i64 {
    let (year, month) = civil_of_day(ms.div_euclid(DAY_MS));
    year * 12 + (month - 1)
}

/// The first millisecond of the month after the one `ms` is in.
pub fn next_month_start(ms: i64) -> i64 {
    let next = month_index(ms) + 1;
    day_of_civil(next.div_euclid(12), next.rem_euclid(12) + 1) * DAY_MS
}

/// The (year, month) of a day number since the epoch (the proleptic Gregorian calendar).
fn civil_of_day(days: i64) -> (i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month)
}

/// The day number since the epoch of the first of a month.
fn day_of_civil(year: i64, month: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// Messages of a class the tenant has sent in the month of `now_ms`: the counter the mail
/// dispatcher reads (`rec-mail-quota`).
pub fn mail_used(app: &App, tenant: &TenantId, class: MailClass, now_ms: i64) -> u64 {
    usage_number(
        app,
        tenant,
        &format!("{}:{}", class.key(), month_index(now_ms)),
    )
}

/// Whether the dispatcher may release a message of `class` for `tenant` now. A bulk message past
/// the plan's allowance of the month is held to the next month while sign-in mail, and the mail of
/// other tenants, leave (EML-021). A counter that cannot be read lets the message go.
pub fn mail_admit(app: &App, tenant: &TenantId, class: MailClass, now_ms: i64) -> MailVerdict {
    if class == MailClass::SignIn {
        return MailVerdict::Send;
    }
    match allowance(app, tenant, Quantity::MailPerMonth) {
        Ok(Some(allowed)) if mail_used(app, tenant, class, now_ms) >= allowed => {
            MailVerdict::Hold {
                release_at_ms: next_month_start(now_ms),
            }
        }
        _ => MailVerdict::Send,
    }
}

/// Count `count` messages of `class` that left for `tenant` at `now_ms`.
pub fn mail_sent(app: &App, tenant: &TenantId, class: MailClass, now_ms: i64, count: u64) {
    bump(
        app,
        tenant,
        &format!("{}:{}", class.key(), month_index(now_ms)),
        count,
    );
}

// ---------------------------------------------------------------------------------------
// Capabilities a plan entitles
// ---------------------------------------------------------------------------------------

/// Whether a plan entitles a capability: it is a core capability, or the plan lists it.
pub fn entitles(plan: &str, capability: &str) -> bool {
    CAPABILITIES
        .iter()
        .find(|spec| spec.id == capability)
        .is_some_and(|spec| spec.core)
        || PLANS
            .iter()
            .find(|(id, _)| *id == plan)
            .is_some_and(|(_, listed)| listed.contains(&capability))
}

/// The plan and the flag settings of a tenant, as the generated authorizer's
/// `AuthHoles::entitlement` takes them: the plan decides which capabilities beyond the core ones
/// are entitled, and each flag is given as it is now for the tenant (OPS-328).
pub fn entitlement(app: &App, tenant: &TenantId) -> Result<Entitlement, StoreError> {
    let mut flags = Vec::new();
    for flag in FLAGS {
        flags.push((flag.id.to_string(), flag_on(app, tenant, flag.id)?));
    }
    Ok(Entitlement {
        plan: plan_of(app, tenant)?,
        flags,
    })
}

/// Refuse a capability the tenant's plan does not entitle.
pub fn require_entitled(app: &App, tenant: &TenantId, capability: &str) -> Result<(), ApiError> {
    let plan = plan_of(app, tenant)?;
    if entitles(&plan, capability) {
        Ok(())
    } else {
        Err(ApiError::new(
            ErrorCode::Forbidden,
            format!("The {plan} plan does not include {capability}."),
        ))
    }
}

/// Change the plan a tenant holds. The change is refused while the tenant holds more of a metered
/// quantity than the new plan allows, naming the quantity (MIG-583). A plan the specification does
/// not declare is invalid. The billing standing the contract also asks for is the caller's.
pub fn change_plan(
    app: &App,
    tenant: &TenantId,
    plan: &str,
    now: Timestamp,
) -> Result<(), ApiError> {
    if !PLANS.iter().any(|(id, _)| *id == plan) {
        return Err(ApiError::new(
            ErrorCode::Invalid,
            format!("There is no plan {plan}."),
        ));
    }
    for quantity in [
        Quantity::StoredBytes,
        Quantity::StoredRecords,
        Quantity::Segments,
    ] {
        if let Some(allowed) = plan_allowance(plan, quantity) {
            let held = used(app, tenant, quantity, now)?;
            if held > allowed {
                let message = format!(
                    "The tenant holds {held} of {}, more than the {plan} plan allows ({allowed}).",
                    quantity.name()
                );
                let mut error = ApiError::new(ErrorCode::Invalid, message.clone());
                error
                    .0
                    .failed
                    .push(Check::fail(format!("quota:{}", quantity.name()), message));
                return Err(error);
            }
        }
    }
    app.store.set_tenant_plan(tenant, plan)?;
    app.audit(
        "plan.changed",
        "",
        json!({ "tenant": tenant.as_str(), "plan": plan }),
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Feature flags
// ---------------------------------------------------------------------------------------

/// A flag a tenant may turn on or off itself: the name of the switch and the flag it belongs to.
/// The release record of the flag defines it (D-6202); today there is one.
pub struct TenantSwitch {
    pub name: &'static str,
    pub flag: &'static str,
}

pub const TENANT_SWITCHES: &[TenantSwitch] = &[TenantSwitch {
    name: "assistant",
    flag: "assistant-flag",
}];

fn flag_default(flag: &str) -> bool {
    FLAGS
        .iter()
        .find(|spec| spec.id == flag)
        .is_some_and(|spec| spec.default_on)
}

fn bool_doc(
    app: &App,
    tenant: &TenantId,
    kind: &str,
    flag: &str,
) -> Result<Option<bool>, StoreError> {
    Ok(app
        .store
        .doc_get(tenant, kind, flag)?
        .and_then(|text| match text.as_str() {
            "on" => Some(true),
            "off" => Some(false),
            _ => serde_json::from_str::<Json_>(&text)
                .ok()
                .and_then(|v| v.get("enabled").and_then(Json_::as_bool)),
        }))
}

/// Whether the release record of `flag` enables it for the tenant; the flag's default when the
/// record says nothing.
pub fn release_allows(app: &App, tenant: &TenantId, flag: &str) -> Result<bool, StoreError> {
    Ok(bool_doc(app, tenant, RELEASE_KIND, flag)?.unwrap_or_else(|| flag_default(flag)))
}

/// Record the release record's decision for a tenant (a platform operation, `set-platform-flag`).
pub fn set_release(
    app: &App,
    tenant: &TenantId,
    flag: &str,
    enabled: bool,
    release: &str,
) -> Result<(), StoreError> {
    let text = json!({ "enabled": enabled, "release": release }).to_string();
    app.store
        .doc_put(tenant, RELEASE_KIND, flag, &text, false)?;
    app.audit(
        "flag.released",
        "",
        json!({ "tenant": tenant.as_str(), "flag": flag, "enabled": enabled, "release": release }),
    );
    Ok(())
}

/// The tenant's own switch of a flag, when it has set one.
pub fn switch_of(app: &App, tenant: &TenantId, flag: &str) -> Result<Option<bool>, StoreError> {
    bool_doc(app, tenant, SWITCH_KIND, flag)
}

/// Whether a flag is on for a tenant: the release record enables it for the tenant and the
/// tenant's switch allows it, each at the flag's default when nothing was set (OPS-328). A flag
/// that is off by default is on only when both were turned on.
pub fn flag_on(app: &App, tenant: &TenantId, flag: &str) -> Result<bool, StoreError> {
    let released = release_allows(app, tenant, flag)?;
    let switched = switch_of(app, tenant, flag)?.unwrap_or_else(|| flag_default(flag));
    Ok(released && switched)
}

/// Set a tenant switch (contract set-feature-flag): `name` must be a tenant switch, `value` `on`
/// or `off`. A switch can be turned on only when the release record enables the flag for the
/// tenant, and can always be turned off. The change is audited with the previous value.
pub fn set_switch(
    app: &App,
    identity: &Identity,
    name: &str,
    value: &str,
) -> Result<Json_, ApiError> {
    let invalid = |message: String| ApiError::new(ErrorCode::Invalid, message);
    let switch = TENANT_SWITCHES
        .iter()
        .find(|switch| switch.name == name)
        .ok_or_else(|| {
            invalid(format!(
                "{name} is not a tenant switch; a platform flag or a quota is changed by the platform operator."
            ))
        })?;
    let on = match value {
        "on" => true,
        "off" => false,
        _ => return Err(invalid("The value of a switch is on or off.".to_string())),
    };
    let tenant = &identity.tenant;
    if on && !release_allows(app, tenant, switch.flag)? {
        let mut error = ApiError::new(
            ErrorCode::Conflict,
            format!(
                "The release record has not enabled {} for this tenant, so it cannot be turned on.",
                switch.flag
            ),
        );
        error.0.kind = Some("ReleaseNotEnabled".to_string());
        return Err(error);
    }
    let previous = switch_of(app, tenant, switch.flag)?;
    app.store
        .doc_put(tenant, SWITCH_KIND, switch.flag, value, false)?;
    app.audit(
        "flag.switch",
        &identity.correlation,
        json!({
            "tenant": tenant.as_str(),
            "flag": switch.flag,
            "previous": previous.map(|p| if p { "on" } else { "off" }),
            "value": value,
            "principal": identity.session.as_ref().map(|s| s.principal.as_str().to_string()),
        }),
    );
    Ok(json!({ "name": switch.name, "flag": switch.flag, "value": value }))
}

// ---------------------------------------------------------------------------------------
// Routes: usage and tenant switches, for an owner
// ---------------------------------------------------------------------------------------

/// What an owner sees of the plan: each quantity, what the tenant has used and what it may.
pub fn usage_of(app: &App, identity: &Identity) -> Result<Json_, ApiError> {
    crate::service::require_owner(app, identity)?;
    let tenant = &identity.tenant;
    let now = app.now();
    let mut quantities = Vec::new();
    for quantity in Quantity::ALL {
        quantities.push(json!({
            "name": quantity.name(),
            "used": used(app, tenant, quantity, now)?,
            "allowance": allowance(app, tenant, quantity)?,
        }));
    }
    Ok(json!({
        "plan": plan_of(app, tenant)?,
        "quantities": quantities,
        "mail_sign_in_this_month": mail_used(app, tenant, MailClass::SignIn, now.0),
    }))
}

/// `/internal/usage`: current usage against each allowance of the tenant's plan, for an owner.
pub async fn usage_route(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Json_>, ApiError> {
    crate::http::as_caller(&app, &headers, usage_of).await
}

/// `/internal/flags`: each tenant switch, what the release record says, what the tenant set, and
/// whether the flag is on, for an owner (AGT-420).
pub async fn flags_route(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Json_>, ApiError> {
    crate::http::as_caller(&app, &headers, |app, identity| {
        crate::service::require_owner(app, identity)?;
        let tenant = &identity.tenant;
        let mut flags = Vec::new();
        for switch in TENANT_SWITCHES {
            flags.push(json!({
                "name": switch.name,
                "flag": switch.flag,
                "released": release_allows(app, tenant, switch.flag)?,
                "switch": switch_of(app, tenant, switch.flag)?,
                "on": flag_on(app, tenant, switch.flag)?,
            }));
        }
        Ok(json!({ "flags": flags }))
    })
    .await
}

#[derive(Clone, Debug, Deserialize)]
pub struct SwitchRequest {
    #[serde(default)]
    pub tenant: Option<String>,
    pub name: String,
    pub value: String,
}

/// `/internal/flags/set`: contract set-feature-flag, for an owner of the tenant.
///
/// Implements: CT-122
pub async fn flags_set_route(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<SwitchRequest>,
) -> Result<Json<Json_>, ApiError> {
    crate::http::as_caller(&app, &headers, move |app, identity| {
        crate::service::require_owner(app, identity)?;
        match request.tenant.as_deref() {
            Some(named) if named != identity.tenant.as_str() => return Err(ApiError::new(
                ErrorCode::Forbidden,
                "tenant-mismatch: the request names another tenant than the site it was sent to",
            )),
            _ => {}
        }
        set_switch(app, identity, &request.name, &request.value)
    })
    .await
}

/// What the platform operator sets for a tenant (contract set-platform-flag): `name` is `plan`, a
/// quantity (`stored-bytes`, `jobs-per-day`, `jobs-per-minute`, `segments`, `mail-per-month`) or a
/// flag id. For a quantity `value` is a number, `unlimited`, or `plan` to give the tenant its
/// plan's value again; for a flag it is `on` or `off`, the decision of its release record.
#[derive(Clone, Debug, Deserialize)]
pub struct PlatformSet {
    pub tenant: String,
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub release: Option<String>,
}

/// Apply [`PlatformSet`]. A quota below what the tenant holds of a metered quantity is refused as
/// a conflict naming the quantity (MIG-583).
pub fn platform_set(
    app: &App,
    request: &PlatformSet,
    correlation: &str,
) -> Result<Json_, ApiError> {
    let tenant = TenantId::new(request.tenant.as_str())
        .map_err(|_| ApiError::new(ErrorCode::NotFound, "There is no such tenant."))?;
    if app.store.tenant_summary(&tenant)?.is_none() {
        return Err(ApiError::new(
            ErrorCode::NotFound,
            "There is no such tenant.",
        ));
    }
    let invalid = |message: String| ApiError::new(ErrorCode::Invalid, message);
    let (name, value) = (request.name.as_str(), request.value.as_str());
    if name == "plan" {
        change_plan(app, &tenant, value, app.now())?;
    } else if let Some(quantity) = Quantity::parse(name) {
        match value {
            "plan" => clear_allowance(app, &tenant, quantity)?,
            "unlimited" => set_allowance(app, &tenant, quantity, None, "platform", correlation)?,
            number => {
                let number = number.parse::<u64>().map_err(|_| {
                    invalid(format!("{number} is not a number, unlimited or plan."))
                })?;
                if matches!(
                    quantity,
                    Quantity::StoredBytes | Quantity::StoredRecords | Quantity::Segments
                ) {
                    let held = used(app, &tenant, quantity, app.now())?;
                    if held > number {
                        let mut error = ApiError::new(
                            ErrorCode::Conflict,
                            format!(
                                "The tenant holds {held} of {}, more than the new quota allows ({number}).",
                                quantity.name()
                            ),
                        );
                        error.0.kind = Some("QuotaBelowHoldings".to_string());
                        return Err(error);
                    }
                }
                set_allowance(
                    app,
                    &tenant,
                    quantity,
                    Some(number),
                    "platform",
                    correlation,
                )?;
            }
        }
    } else if FLAGS.iter().any(|flag| flag.id == name) {
        let enabled = match value {
            "on" => true,
            "off" => false,
            _ => return Err(invalid("The value of a flag is on or off.".to_string())),
        };
        set_release(
            app,
            &tenant,
            name,
            enabled,
            request.release.as_deref().unwrap_or("manual"),
        )?;
    } else {
        return Err(invalid(format!(
            "{name} is neither a platform flag nor a quota."
        )));
    }
    Ok(json!({ "tenant": tenant.as_str(), "name": name, "value": value }))
}

/// `/internal/platform/quota`: the platform operator's change of a plan, a quota or the release
/// decision of a flag. Like the other platform routes it asks for the service credential only: the
/// BFF has no route to it.
pub async fn platform_set_route(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<PlatformSet>,
) -> Result<Json<Json_>, ApiError> {
    let correlation = headers
        .get(strata_protocol::api::HEADER_CORRELATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    app.blocking(move |app| platform_set(app, &request, &correlation))
        .await?
        .map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_route_serves_a_contract_that_has_a_quota() {
        for (route, contract) in ROUTES {
            assert!(
                quotas::for_contract(contract).next().is_some(),
                "{route} serves {contract}, which has no quota"
            );
        }
    }

    #[test]
    fn months_are_counted_from_the_calendar() {
        // 2026-11-20 00:00 UTC.
        let day = 20_777;
        assert_eq!(civil_of_day(day), (2026, 11));
        assert_eq!(month_index(day * DAY_MS), 2026 * 12 + 10);
        assert_eq!(
            day_of_civil(2026, 12) * DAY_MS,
            next_month_start(day * DAY_MS)
        );
        assert_eq!(civil_of_day(day_of_civil(2027, 1)), (2027, 1));
        assert_eq!(civil_of_day(day_of_civil(2027, 1) - 1), (2026, 12));
    }

    #[test]
    fn the_plans_of_the_decision_hold_its_numbers() {
        assert_eq!(
            plan_allowance("standard", Quantity::StoredBytes),
            Some(10 * GB)
        );
        assert_eq!(
            plan_allowance("standard", Quantity::JobsPerDay),
            Some(10_000)
        );
        assert_eq!(
            plan_allowance("standard", Quantity::MailPerMonth),
            Some(5_000)
        );
        assert_eq!(
            plan_allowance("business", Quantity::StoredBytes),
            Some(100 * GB)
        );
        assert_eq!(
            plan_allowance("business", Quantity::JobsPerDay),
            Some(100_000)
        );
        assert_eq!(
            plan_allowance("business", Quantity::MailPerMonth),
            Some(50_000)
        );
        assert_eq!(plan_allowance("enterprise", Quantity::JobsPerDay), None);
        assert_eq!(
            plan_allowance("enterprise", Quantity::JobsPerMinute),
            Some(100)
        );
    }

    #[test]
    fn a_plan_entitles_its_capabilities_and_the_core_ones() {
        assert!(entitles("enterprise", "placement-dedicated"));
        assert!(!entitles("standard", "placement-dedicated"));
        assert!(!entitles("business", "placement-dedicated"));
        assert!(entitles("standard", "layout-apply"));
        assert!(!entitles("enterprise", "no-such-capability"));
    }
}
