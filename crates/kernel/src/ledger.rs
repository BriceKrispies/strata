//! The tenant ledger (D-5000 to D-5013, D-6550 to D-6554): append-only entries numbered per
//! account, a balance that is the sum of the entries, units and pools a tenant declares, limits
//! (a floor, a cap and a cap per calendar period), an idempotency key per account, one domain
//! event per entry, and erasure that anonymises.
//!
//! Where the parts live:
//!
//! - The units, pools and limits are tenant metadata, `TenantSchema::ledger`
//!   ([`LedgerDeclarations`]), changed by the schema changes `DeclareLedgerUnit`,
//!   `DeclareLedgerPool`, `SetLedgerLimit` and `RemoveLedgerDefinition` ([`apply_change`],
//!   checked by [`declaration_problems`] and [`check_removals`]).
//! - The accounts and entries are state, `TenantState::ledger` ([`LedgerState`]), loaded through
//!   [`Need::Ledger`] and changed only by [`DeltaOp::Ledger`]. An account's header (balance, last
//!   entry number, the credits of the current periods) loads without its entries (K-100).
//! - A posting is the pure decision [`post`] (contract `decide-ledger-posting`, CT-280). It
//!   checks no capability: the direct intent `PostLedger` checks `ledger-post` before calling it
//!   ([`decide_intent`]), and a tenant operation or an automation step calls it with the
//!   authority of its definition (D-6547). Several postings of one change call [`post_after`] so
//!   that each sees the ones before it; a transfer is two postings committed together (D-5013).
//! - Reads ([`read_balance`], [`read_entries`], [`read_ranked`], [`read_definitions`]) are
//!   answered from a snapshot that holds what their `*_needs` function names.
//!
//! The kernel reads no clock: the time of an entry is `Context::now` (D-5010). An account's id is
//! derived from the tenant, the owner, the unit and the scope (D-5001, D-6551), so a decision can
//! name the account it needs before loading anything.

use crate::canon;
use crate::intent::{
    Check, CheckExt, Context, DeltaOp, DomainError, DomainErrorExt, ErrorKind, EventBody, Output,
    SchemaChange,
};
use crate::limits;
use crate::rules::{holds_any, insufficient, Lookup};
use crate::schema::{Grant, TenantSchema};
use crate::state::{CoverageExt, Need, Principal, State, TenantState};
use crate::types::{
    LedgerAccount, LedgerAccountId, LedgerAccountRank, LedgerAmount, LedgerBalance,
    LedgerDeclarations, LedgerDefinitionRef, LedgerDefinitions, LedgerDelta, LedgerEntry,
    LedgerLimit, LedgerNeed, LedgerOrder, LedgerOwner, LedgerPeriod, LedgerPeriodTotal,
    LedgerPostingRequest, LedgerPostingResult, LedgerScope, LedgerState, LedgerUnit, PrincipalId,
    TenantId, Timestamp,
};
use std::collections::BTreeSet;

/// The capability of a direct posting (CT-281).
pub const LEDGER_POST: &str = "ledger-post";
/// The capability of reading any account and the definitions (CT-282, CT-283, CT-809, CT-810).
pub const LEDGER_READ: &str = "ledger-read";
/// The capability of reading one's own accounts (CT-807, CT-808); every signed-in person holds it
/// (D-6550).
pub const LEDGER_READ_OWN: &str = "ledger-read-own";
/// The capability of declaring units, pools and limits (CT-284 to CT-286, CT-811).
pub const LEDGER_CONFIGURE: &str = "ledger-configure";

/// The four calendar periods, in the order an account header keeps their credits.
const PERIODS: [LedgerPeriod; 4] = [
    LedgerPeriod::Hour,
    LedgerPeriod::Day,
    LedgerPeriod::Week,
    LedgerPeriod::Month,
];

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 86_400_000;

// ------------------------------------------------------------------------------------------------
// Accounts

/// The id of the account of `owner`, `unit` and `scope` in `tenant` (D-5001, D-6551): the first
/// sixteen bytes of the SHA-256 of their canonical encoding, written as a UUID of version 8. The
/// same four always give the same id, and nothing random is drawn (K-11).
pub fn account_id(
    tenant: &TenantId,
    owner: &LedgerOwner,
    unit: &LedgerUnit,
    scope: &str,
) -> LedgerAccountId {
    let hex = canon::hash_of(&("strata-ledger-account", tenant, owner, unit, scope));
    let mut digits: Vec<char> = hex.chars().take(32).collect();
    // RFC 9562: version 8 (custom) in the thirteenth digit, variant 10xx in the seventeenth.
    digits[12] = '8';
    let variant = digits[16].to_digit(16).expect("hex digit") & 0x3 | 0x8;
    digits[16] = char::from_digit(variant, 16).expect("hex digit");
    let text: String = digits.into_iter().collect();
    let hyphenated = format!(
        "{}-{}-{}-{}-{}",
        &text[0..8],
        &text[8..12],
        &text[12..16],
        &text[16..20],
        &text[20..32]
    );
    serde_json::from_value(serde_json::Value::String(hyphenated))
        .expect("a 32-digit hex string with hyphens is a UUID")
}

/// The key of an account in [`LedgerState`]: the text of its id.
pub fn account_key(account: &LedgerAccountId) -> String {
    account.to_string()
}

/// The scope of a request: empty when it names none (D-6551).
fn scope_text(scope: &Option<LedgerScope>) -> &str {
    scope.as_ref().map_or("", |s| s.as_str())
}

/// A scope as its type, for the answers that carry one.
fn scope_value(scope: &str) -> LedgerScope {
    LedgerScope(scope.to_string())
}

/// The header of an account, if the snapshot holds it.
fn header<'a>(tenant: &'a TenantState, account: &LedgerAccountId) -> Option<&'a LedgerAccount> {
    tenant
        .ledger
        .as_ref()
        .and_then(|ledger| ledger.accounts.get(&account_key(account)))
}

/// The entries of an account the snapshot holds, in order.
fn loaded_entries<'a>(tenant: &'a TenantState, account: &LedgerAccountId) -> &'a [LedgerEntry] {
    tenant
        .ledger
        .as_ref()
        .and_then(|ledger| ledger.entries.get(&account_key(account)))
        .map_or(&[], Vec::as_slice)
}

/// Whether the snapshot answers a ledger need; when it does not, the refusal that asks for it.
fn require(state: &State, need: LedgerNeed) -> Result<(), DomainError> {
    let need = Need::Ledger(need);
    if state.coverage.covers(&need) {
        Ok(())
    } else {
        Err(insufficient(need))
    }
}

/// The declarations of a schema; an empty set when it declares nothing.
pub fn declarations(schema: &TenantSchema) -> LedgerDeclarations {
    schema.ledger.clone().unwrap_or_else(|| LedgerDeclarations {
        units: Default::default(),
        pools: Default::default(),
        limits: Vec::new(),
    })
}

fn unit_declared(schema: &TenantSchema, unit: &LedgerUnit) -> bool {
    schema
        .ledger
        .as_ref()
        .is_some_and(|ledger| ledger.units.contains_key(unit))
}

fn pool_declared(schema: &TenantSchema, pool: &crate::types::LedgerPoolName) -> bool {
    schema
        .ledger
        .as_ref()
        .is_some_and(|ledger| ledger.pools.contains_key(pool))
}

// ------------------------------------------------------------------------------------------------
// Periods (LDG-015)

/// Days since 1970-01-01 of a civil date (proleptic Gregorian), after H. Hinnant.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil year and month of a count of days since 1970-01-01.
fn civil_month(days: i64) -> (i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month)
}

/// The start of the calendar `period` in UTC that contains `at`: the hour, the day, the week
/// starting on Monday, or the month (D-5005, LDG-015).
pub fn period_start(period: LedgerPeriod, at: Timestamp) -> Timestamp {
    let t = at.0;
    let days = t.div_euclid(DAY_MS);
    Timestamp(match period {
        LedgerPeriod::Hour => t.div_euclid(HOUR_MS) * HOUR_MS,
        LedgerPeriod::Day => days * DAY_MS,
        LedgerPeriod::Week => {
            // 1970-01-01 was a Thursday, three days after a Monday.
            let since_monday = (days + 3).rem_euclid(7);
            (days - since_monday) * DAY_MS
        }
        LedgerPeriod::Month => {
            let (year, month) = civil_month(days);
            days_from_civil(year, month, 1) * DAY_MS
        }
    })
}

/// The credits an account has posted in the calendar `period` that contains `at`.
fn credits_in(account: &LedgerAccount, period: LedgerPeriod, at: Timestamp) -> u64 {
    let start = period_start(period, at);
    account
        .periods
        .iter()
        .find(|total| total.period == period && total.start == start)
        .map_or(0, |total| total.credits)
}

// ------------------------------------------------------------------------------------------------
// Limits (LDG-011 to LDG-016, LDG-1542)

/// The limits that apply to one account: each of the account's own limits, and the limit of its
/// unit for each one the account does not set (LDG-012). A cap per period and its period are one
/// limit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    pub floor: Option<i64>,
    pub cap: Option<i64>,
    pub period_cap: Option<(u64, LedgerPeriod)>,
}

fn is_account_limit(
    limit: &LedgerLimit,
    owner: &LedgerOwner,
    unit: &LedgerUnit,
    scope: &str,
) -> bool {
    &limit.unit == unit && limit.owner.as_ref() == Some(owner) && scope_text(&limit.scope) == scope
}

/// The limits that apply to the account of `owner`, `unit` and `scope` under `schema`.
pub fn limits_of(
    schema: &TenantSchema,
    owner: &LedgerOwner,
    unit: &LedgerUnit,
    scope: &str,
) -> Limits {
    let Some(ledger) = schema.ledger.as_ref() else {
        return Limits::default();
    };
    let of_unit = ledger
        .limits
        .iter()
        .find(|limit| &limit.unit == unit && limit.owner.is_none());
    let of_account = ledger
        .limits
        .iter()
        .find(|limit| is_account_limit(limit, owner, unit, scope));
    let floor = |limit: &LedgerLimit| limit.floor.map(|a| a.0);
    let cap = |limit: &LedgerLimit| limit.cap.map(|a| a.0);
    let period = |limit: &LedgerLimit| limit.period_cap.zip(limit.period);
    Limits {
        floor: of_account
            .and_then(floor)
            .or_else(|| of_unit.and_then(floor)),
        cap: of_account.and_then(cap).or_else(|| of_unit.and_then(cap)),
        period_cap: of_account
            .and_then(period)
            .or_else(|| of_unit.and_then(period)),
    }
}

// ------------------------------------------------------------------------------------------------
// Posting (CT-280)

/// What an accepted posting adds to the change that causes it: the delta operations (the new
/// header, then the entry), the event, the result and the checks that passed. A posting that
/// repeats a key with the same request adds no operation and no event and answers with the
/// first entry (`result.replayed`, LDG-006).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Posted {
    pub ops: Vec<DeltaOp>,
    pub events: Vec<EventBody>,
    pub result: LedgerPostingResult,
    pub evidence: Vec<Check>,
}

/// The needs of one posting: the account's header and the entry of its idempotency key.
pub fn posting_needs(tenant: &TenantId, request: &LedgerPostingRequest) -> Vec<Need> {
    let account = account_id(
        tenant,
        &request.owner,
        &request.unit,
        scope_text(&request.scope),
    );
    vec![
        Need::Ledger(LedgerNeed::Account { account }),
        Need::Ledger(LedgerNeed::EntryByKey {
            account,
            key: request.idempotency_key.clone(),
        }),
    ]
}

/// The needs of the postings of a `PostLedger` intent, each once.
pub fn intent_needs(tenant: &TenantId, postings: &[LedgerPostingRequest]) -> Vec<Need> {
    let mut out = BTreeSet::new();
    for request in postings {
        out.extend(posting_needs(tenant, request));
    }
    out.into_iter().collect()
}

/// Decide one posting against a snapshot (contract `decide-ledger-posting`, CT-280).
///
/// Implements: CT-280
///
/// Pure: the result depends on the snapshot, the request and the context only; the time of the
/// entry is `ctx.now` (LDG-008, D-5010) and the principal of the context is recorded as
/// `caused_by`. No capability is checked here (see the module documentation).
///
/// Refusals, each changing nothing: an undeclared unit (LDG-002) or owner (LDG-004) is
/// `UnknownRecord`; a zero amount, an amount or a balance out of the range ±(2^53 − 1), an empty
/// or too long key, a too long reason or scope (LDG-003, D-5004) and a correction of an entry the
/// account does not hold (LDG-023) are `InvalidIntent`; a key recorded with another amount, reason
/// or correction is `VersionConflict` (LDG-007); a posting past the floor, the cap or the cap per
/// period is `ConstraintViolated` (LDG-013, LDG-014). A snapshot that lacks the header or the
/// entry of the key is `InsufficientState` naming it.
pub fn post(
    state: &State,
    request: &LedgerPostingRequest,
    ctx: &Context,
) -> Result<Posted, DomainError> {
    post_after(state, &[], request, ctx)
}

/// [`post`] after the operations `earlier` of the same change, so that a posting sees the
/// headers and entries that earlier postings of its change wrote (D-6548: each step on the state
/// the steps before it leave).
pub fn post_after(
    state: &State,
    earlier: &[DeltaOp],
    request: &LedgerPostingRequest,
    ctx: &Context,
) -> Result<Posted, DomainError> {
    let Some(tenant) = state.tenant.as_deref() else {
        return Err(refuse(
            ErrorKind::TenantNotFound,
            "tenant:exists",
            "no such tenant",
        ));
    };
    let mut evidence = Vec::new();
    let unit = &request.unit;
    let scope = scope_text(&request.scope);

    if !unit_declared(&tenant.schema, unit) {
        return Err(refused(
            evidence,
            ErrorKind::UnknownRecord,
            "ledger:unknown-unit",
            format!("the tenant declares no ledger unit {unit}"),
        ));
    }
    evidence.push(Check::pass(
        "ledger:unit",
        format!("unit {unit} is declared"),
    ));
    match &request.owner {
        LedgerOwner::Principal { principal } if !tenant.members.contains_key(principal) => {
            return Err(refused(
                evidence,
                ErrorKind::UnknownRecord,
                "ledger:unknown-owner",
                format!("the tenant has no principal {principal}"),
            ))
        }
        LedgerOwner::Pool { pool } if !pool_declared(&tenant.schema, pool) => {
            return Err(refused(
                evidence,
                ErrorKind::UnknownRecord,
                "ledger:unknown-owner",
                format!("the tenant declares no ledger pool {pool}"),
            ))
        }
        _ => evidence.push(Check::pass("ledger:owner", "the owner exists")),
    }
    let amount = request.amount.0;
    if amount == 0 || amount.unsigned_abs() > limits::MAX_LEDGER_AMOUNT as u64 {
        return Err(refused(
            evidence,
            ErrorKind::InvalidIntent,
            "ledger:invalid-amount",
            format!(
                "an amount is a whole number from -{0} to {0} other than 0, not {amount}",
                limits::MAX_LEDGER_AMOUNT
            ),
        ));
    }
    let key = &request.idempotency_key;
    if key.is_empty() || key.len() > limits::MAX_LEDGER_KEY_BYTES {
        return Err(refused(
            evidence,
            ErrorKind::InvalidIntent,
            "ledger:invalid-key",
            format!(
                "an idempotency key is 1 to {} bytes",
                limits::MAX_LEDGER_KEY_BYTES
            ),
        ));
    }
    if request.reason.len() > limits::MAX_LEDGER_REASON_BYTES {
        return Err(refused(
            evidence,
            ErrorKind::InvalidIntent,
            "ledger:invalid-reason",
            format!(
                "a reason is at most {} bytes",
                limits::MAX_LEDGER_REASON_BYTES
            ),
        ));
    }
    if scope.len() > limits::MAX_LEDGER_SCOPE_BYTES {
        return Err(refused(
            evidence,
            ErrorKind::InvalidIntent,
            "ledger:invalid-scope",
            format!(
                "a scope is at most {} bytes",
                limits::MAX_LEDGER_SCOPE_BYTES
            ),
        ));
    }

    let account = account_id(&tenant.id, &request.owner, unit, scope);
    // The header: the one an earlier posting of this change wrote, else the stored one.
    let written = earlier.iter().rev().find_map(|op| match op {
        DeltaOp::Ledger(LedgerDelta::PutAccount(a)) if a.account == account => Some(a),
        _ => None,
    });
    let current: Option<LedgerAccount> = match written {
        Some(found) => Some(found.clone()),
        None => {
            require(state, LedgerNeed::Account { account })?;
            header(tenant, &account).cloned()
        }
    };

    // LDG-006, LDG-007: a key of the account already recorded.
    if let Some(found) = &current {
        let mut recorded: Option<LedgerEntry> = earlier.iter().find_map(|op| match op {
            DeltaOp::Ledger(LedgerDelta::AppendEntry(e))
                if e.account == account && &e.idempotency_key == key =>
            {
                Some(e.clone())
            }
            _ => None,
        });
        if recorded.is_none() && found.last_sequence > 0 {
            require(
                state,
                LedgerNeed::EntryByKey {
                    account,
                    key: key.clone(),
                },
            )?;
            recorded = loaded_entries(tenant, &account)
                .iter()
                .find(|e| &e.idempotency_key == key)
                .cloned();
        }
        if let Some(entry) = recorded {
            if entry.amount == request.amount
                && entry.reason == request.reason
                && entry.corrects == request.corrects
            {
                evidence.push(Check::pass(
                    "ledger:idempotency",
                    format!("key {key} was recorded as entry {}", entry.sequence),
                ));
                let result = LedgerPostingResult {
                    balance: balance_of(found),
                    entry,
                    replayed: true,
                };
                return Ok(Posted {
                    ops: Vec::new(),
                    events: Vec::new(),
                    result,
                    evidence,
                });
            }
            return Err(refused(
                evidence,
                ErrorKind::VersionConflict,
                "ledger:idempotency-mismatch",
                format!(
                    "key {key} was recorded as entry {} with another amount, reason or correction",
                    entry.sequence
                ),
            ));
        }
    }
    evidence.push(Check::pass(
        "ledger:idempotency",
        format!("key {key} is new"),
    ));

    let before = current.as_ref().map_or(0, |a| a.balance.0);
    let last = current.as_ref().map_or(0, |a| a.last_sequence);
    if let Some(corrects) = request.corrects {
        if corrects == 0 || corrects > last {
            return Err(refused(
                evidence,
                ErrorKind::InvalidIntent,
                "ledger:bad-correction",
                format!("the account holds no entry {corrects}"),
            ));
        }
    }
    let after = match before.checked_add(amount) {
        Some(after) if after.abs() <= limits::MAX_LEDGER_AMOUNT => after,
        _ => {
            return Err(refused(
                evidence,
                ErrorKind::InvalidIntent,
                "ledger:invalid-amount",
                format!(
                    "the balance after the posting would be outside -{0} to {0}",
                    limits::MAX_LEDGER_AMOUNT
                ),
            ))
        }
    };

    // LDG-013: a posting may leave a balance outside a limit only when it moves a balance that is
    // already outside it back toward it.
    let applying = limits_of(&tenant.schema, &request.owner, unit, scope);
    if let Some(floor) = applying.floor {
        let toward = before < floor && amount > 0;
        if after < floor && !toward {
            return Err(refused(
                evidence,
                ErrorKind::ConstraintViolated,
                "ledger:below-floor",
                format!("the balance {after} would be below the floor {floor}"),
            ));
        }
        evidence.push(Check::pass(
            "ledger:floor",
            format!("{after} against floor {floor}"),
        ));
    }
    if let Some(cap) = applying.cap {
        let toward = before > cap && amount < 0;
        if after > cap && !toward {
            return Err(refused(
                evidence,
                ErrorKind::ConstraintViolated,
                "ledger:above-cap",
                format!("the balance {after} would be above the cap {cap}"),
            ));
        }
        evidence.push(Check::pass(
            "ledger:cap",
            format!("{after} against cap {cap}"),
        ));
    }
    // LDG-014: the credits of the period of the entry.
    if let Some((period_cap, period)) = applying.period_cap {
        if amount > 0 {
            let so_far = current
                .as_ref()
                .map_or(0, |a| credits_in(a, period, ctx.now));
            let total = so_far.saturating_add(amount as u64);
            if total > period_cap {
                return Err(refused(
                    evidence,
                    ErrorKind::ConstraintViolated,
                    "ledger:above-period-cap",
                    format!(
                        "the credits of this {} would be {total}, above the cap per period {period_cap}",
                        period.as_str()
                    ),
                ));
            }
            evidence.push(Check::pass(
                "ledger:period-cap",
                format!(
                    "{total} credits this {} against {period_cap}",
                    period.as_str()
                ),
            ));
        }
    }

    // The new header: the period totals of the time of the entry, the balance and the number.
    let periods = PERIODS
        .iter()
        .map(|&period| {
            let kept = current
                .as_ref()
                .map_or(0, |a| credits_in(a, period, ctx.now));
            LedgerPeriodTotal {
                period,
                start: period_start(period, ctx.now),
                credits: if amount > 0 {
                    kept.saturating_add(amount as u64)
                } else {
                    kept
                },
            }
        })
        .collect();
    let next = LedgerAccount {
        account,
        owner: match &current {
            Some(a) => a.owner.clone(),
            None => Some(request.owner.clone()),
        },
        unit: unit.clone(),
        scope: scope_value(scope),
        balance: LedgerAmount(after),
        last_sequence: last + 1,
        opened_at: current.as_ref().map_or(ctx.now, |a| a.opened_at),
        periods,
    };
    let entry = LedgerEntry {
        account,
        sequence: last + 1,
        amount: request.amount,
        idempotency_key: key.clone(),
        reason: request.reason.clone(),
        caused_by: match &ctx.principal {
            Principal::User(id) => Some(id.clone()),
            _ => None,
        },
        at: ctx.now,
        corrects: request.corrects,
    };
    let event = EventBody::LedgerEntryPosted {
        account,
        owner: request.owner.clone(),
        unit: unit.clone(),
        scope: scope_value(scope),
        amount: request.amount,
        balance: LedgerAmount(after),
        balance_before: LedgerAmount(before),
        entry_sequence: entry.sequence,
        reason: request.reason.clone(),
    };
    evidence.push(Check::pass(
        "ledger:posted",
        format!(
            "entry {} of the account, balance {before} to {after}",
            entry.sequence
        ),
    ));
    Ok(Posted {
        result: LedgerPostingResult {
            entry: entry.clone(),
            balance: balance_of(&next),
            replayed: false,
        },
        ops: vec![
            DeltaOp::Ledger(LedgerDelta::PutAccount(next)),
            DeltaOp::Ledger(LedgerDelta::AppendEntry(entry)),
        ],
        events: vec![event],
        evidence,
    })
}

/// The balance answer of a header.
fn balance_of(account: &LedgerAccount) -> LedgerBalance {
    LedgerBalance {
        account: account.account,
        unit: account.unit.clone(),
        balance: account.balance,
        last_sequence: account.last_sequence,
        scope: (!account.scope.as_str().is_empty()).then(|| account.scope.clone()),
    }
}

/// A refusal made before anything was checked.
fn refuse(kind: ErrorKind, rule: &str, message: impl Into<String>) -> DomainError {
    let message = message.into();
    DomainError::new(kind, message.clone()).with(vec![Check::fail(rule, message)])
}

/// A refusal after the checks in `evidence`, naming the rule that failed.
fn refused(
    mut evidence: Vec<Check>,
    kind: ErrorKind,
    rule: &str,
    message: impl Into<String>,
) -> DomainError {
    let message = message.into();
    evidence.push(Check::fail(rule, message.clone()));
    DomainError::new(kind, format!("{rule}: {message}")).with(evidence)
}

/// What a `PostLedger` intent adds to its transition.
pub struct Decided {
    pub ops: Vec<DeltaOp>,
    pub events: Vec<EventBody>,
    pub evidence: Vec<Check>,
    pub output: Output,
}

/// Decide the intent `PostLedger` (CT-281): the principal holds `ledger-post`, the intent names
/// one to [`limits::MAX_LEDGER_POSTINGS`] postings, and each is decided by [`post_after`] on the
/// state the postings before it leave; one refused posting refuses them all (D-5013, LDG-009).
pub fn decide_intent(
    state: &State,
    postings: &[LedgerPostingRequest],
    ctx: &Context,
) -> Result<Decided, DomainError> {
    let Some(tenant) = state.tenant.as_deref() else {
        return Err(refuse(
            ErrorKind::TenantNotFound,
            "tenant:exists",
            "no such tenant",
        ));
    };
    let mut evidence = Vec::new();
    let rule = format!("permission:capability:{LEDGER_POST}");
    if holds(tenant, &ctx.principal, LEDGER_POST) {
        evidence.push(Check::pass(rule, format!("holds {LEDGER_POST}")));
    } else {
        evidence.push(Check::fail(rule, format!("does not hold {LEDGER_POST}")));
        return Err(DomainError::new(
            ErrorKind::Unauthorized,
            format!("a posting needs the capability {LEDGER_POST}"),
        )
        .with(evidence));
    }
    if postings.is_empty() || postings.len() > limits::MAX_LEDGER_POSTINGS {
        return Err(refused(
            evidence,
            ErrorKind::InvalidIntent,
            "ledger:postings",
            format!(
                "a posting change names 1 to {} postings, not {}",
                limits::MAX_LEDGER_POSTINGS,
                postings.len()
            ),
        ));
    }
    let mut ops = Vec::new();
    let mut events = Vec::new();
    let mut results = Vec::new();
    for request in postings {
        let posted = post_after(state, &ops, request, ctx).map_err(|mut error| {
            let mut all = evidence.clone();
            all.append(&mut error.evidence);
            error.evidence = all;
            error
        })?;
        evidence.extend(posted.evidence);
        ops.extend(posted.ops);
        events.extend(posted.events);
        results.push(posted.result);
    }
    Ok(Decided {
        ops,
        events,
        evidence,
        output: Output::LedgerPosted { results },
    })
}

/// Whether the principal holds a capability; the owner holds every one.
fn holds(tenant: &TenantState, principal: &Principal, name: &str) -> bool {
    holds_any(
        tenant,
        principal,
        &BTreeSet::from([Grant::Capability(name.to_string())]),
    )
}

/// Whether `principal` may read the account of `owner`: the decision of every ledger read
/// ([`may_read`]) as a yes or no, for a subscription that tells a person of a posting only when
/// they could read the account it was made in.
pub fn may_read_account(tenant: &TenantState, principal: &Principal, owner: &LedgerOwner) -> bool {
    may_read(tenant, principal, Some(owner)).is_ok()
}

// ------------------------------------------------------------------------------------------------
// Applying a delta

/// Apply one ledger change to a tenant state (`apply_delta`). A header replaces the stored one;
/// an entry is added after the entries of its account. Nothing rewrites or removes an entry.
pub fn apply(tenant: &mut TenantState, op: &LedgerDelta) {
    let ledger = tenant.ledger.get_or_insert_with(empty_state);
    match op {
        LedgerDelta::PutAccount(account) => {
            ledger
                .accounts
                .insert(account_key(&account.account), account.clone());
        }
        LedgerDelta::AppendEntry(entry) => {
            let entries = ledger
                .entries
                .entry(account_key(&entry.account))
                .or_default();
            if !entries.iter().any(|e| e.sequence == entry.sequence) {
                entries.push(entry.clone());
                entries.sort_by_key(|e| e.sequence);
            }
        }
    }
}

/// A ledger state that holds nothing.
pub fn empty_state() -> LedgerState {
    LedgerState {
        accounts: Default::default(),
        entries: Default::default(),
    }
}

/// A ledger state that holds nothing is none at all, so a snapshot without accounts reads and
/// hashes as one of a tenant that never posted.
pub fn tidy_state(ledger: &mut Option<LedgerState>) {
    if ledger
        .as_ref()
        .is_some_and(|l| l.accounts.is_empty() && l.entries.is_empty())
    {
        *ledger = None;
    }
}

// ------------------------------------------------------------------------------------------------
// Partial snapshots

/// The order of a ranked listing: balance as asked, then the owner ascending (an erased owner
/// first), then the account id (LDG-1543).
fn rank_order(
    order: LedgerOrder,
) -> impl Fn(&&LedgerAccount, &&LedgerAccount) -> std::cmp::Ordering {
    move |a, b| {
        let by_balance = match order {
            LedgerOrder::GreatestFirst => b.balance.cmp(&a.balance),
            LedgerOrder::LeastFirst => a.balance.cmp(&b.balance),
        };
        by_balance
            .then_with(|| a.owner.cmp(&b.owner))
            .then_with(|| a.account.cmp(&b.account))
    }
}

/// Copy from a complete ledger `from` into `into` what `need` names. The memory store and tests
/// build partial snapshots with it; the SQLite adapter answers the same needs with SQL.
pub fn partial(from: &LedgerState, need: &LedgerNeed, into: &mut LedgerState) {
    fn put_account(into: &mut LedgerState, account: &LedgerAccount) {
        into.accounts
            .insert(account_key(&account.account), account.clone());
    }
    fn put_entry(into: &mut LedgerState, entry: &LedgerEntry) {
        let entries = into.entries.entry(account_key(&entry.account)).or_default();
        if !entries.iter().any(|e| e.sequence == entry.sequence) {
            entries.push(entry.clone());
            entries.sort_by_key(|e| e.sequence);
        }
    }
    match need {
        LedgerNeed::Account { account } => {
            if let Some(found) = from.accounts.get(&account_key(account)) {
                put_account(into, found);
            }
        }
        LedgerNeed::EntryByKey { account, key } => {
            if let Some(found) = from
                .entries
                .get(&account_key(account))
                .and_then(|entries| entries.iter().find(|e| &e.idempotency_key == key))
            {
                put_entry(into, found);
            }
        }
        LedgerNeed::Entries {
            account,
            after,
            limit,
        } => {
            for entry in from
                .entries
                .get(&account_key(account))
                .into_iter()
                .flatten()
                .filter(|e| after.is_none_or(|a| e.sequence > a))
                .take(*limit as usize)
            {
                put_entry(into, entry);
            }
        }
        LedgerNeed::Ranked {
            unit,
            scope,
            order,
            offset,
            limit,
        } => {
            let mut matching: Vec<&LedgerAccount> = from
                .accounts
                .values()
                .filter(|a| &a.unit == unit && a.scope == *scope)
                .collect();
            matching.sort_by(rank_order(*order));
            for account in matching
                .into_iter()
                .take(*offset as usize + *limit as usize)
            {
                put_account(into, account);
            }
        }
        LedgerNeed::UnitUsed { unit } => {
            if let Some(found) = from.accounts.values().find(|a| &a.unit == unit) {
                put_account(into, found);
            }
        }
        LedgerNeed::PoolUsed { pool } => {
            if let Some(found) = from
                .accounts
                .values()
                .find(|a| matches!(&a.owner, Some(LedgerOwner::Pool { pool: p }) if p == pool))
            {
                put_account(into, found);
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Erasure (LDG-027, LDG-028)

/// What an erasure cleared: the accounts it took the owner from and the entries it took
/// `caused_by` from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Anonymised {
    pub accounts: u64,
    pub entries: u64,
}

/// Clear `principal` from the accounts it owns and the entries it caused, keeping every amount,
/// key, reason, number, time and balance (LDG-027, LDG-028, D-5012). Account ids stay as they
/// were derived. The memory store calls it; the SQLite adapter does the same in SQL.
pub fn anonymise(ledger: &mut LedgerState, principal: &PrincipalId) -> Anonymised {
    let mut done = Anonymised::default();
    for account in ledger.accounts.values_mut() {
        if matches!(&account.owner, Some(LedgerOwner::Principal { principal: p }) if p == principal)
        {
            account.owner = None;
            done.accounts += 1;
        }
    }
    for entry in ledger.entries.values_mut().flatten() {
        if entry.caused_by.as_ref() == Some(principal) {
            entry.caused_by = None;
            done.entries += 1;
        }
    }
    done
}

// ------------------------------------------------------------------------------------------------
// Declarations (LDG-011, LDG-1548 to LDG-1550)

/// Whether a schema change is one of the ledger's.
pub fn is_ledger_change(change: &SchemaChange) -> bool {
    matches!(
        change,
        SchemaChange::DeclareLedgerUnit(_)
            | SchemaChange::DeclareLedgerPool(_)
            | SchemaChange::SetLedgerLimit(_)
            | SchemaChange::RemoveLedgerDefinition(_)
    )
}

/// The table of declarations, to change it. A table left empty is no table at all, so a schema
/// that declares nothing hashes as one that never did.
fn declarations_mut(schema: &mut TenantSchema) -> &mut LedgerDeclarations {
    schema.ledger.get_or_insert_with(|| LedgerDeclarations {
        units: Default::default(),
        pools: Default::default(),
        limits: Vec::new(),
    })
}

fn tidy(schema: &mut TenantSchema) {
    if schema
        .ledger
        .as_ref()
        .is_some_and(|l| l.units.is_empty() && l.pools.is_empty() && l.limits.is_empty())
    {
        schema.ledger = None;
    }
}

/// Whether two limits are of the same unit, or of the same account.
fn same_limit(
    a: &LedgerLimit,
    unit: &LedgerUnit,
    owner: &Option<LedgerOwner>,
    scope: &str,
) -> bool {
    &a.unit == unit && &a.owner == owner && (owner.is_none() || scope_text(&a.scope) == scope)
}

/// Apply one ledger schema change to a working copy and describe it (`apply_change`). Declaring
/// a unit, a pool or a limit again replaces it (CT-284 to CT-286); removing one that is not
/// declared is refused. Whether an account still names a unit or a pool is checked by
/// [`check_removals`], which can see the accounts.
pub fn apply_change(schema: &mut TenantSchema, change: &SchemaChange) -> Result<String, String> {
    let line = match change {
        SchemaChange::DeclareLedgerUnit(decl) => {
            let previous = declarations_mut(schema)
                .units
                .insert(decl.unit.clone(), decl.clone());
            match previous {
                Some(_) => format!("redeclare ledger unit {}", decl.unit),
                None => format!("declare ledger unit {}", decl.unit),
            }
        }
        SchemaChange::DeclareLedgerPool(decl) => {
            let previous = declarations_mut(schema)
                .pools
                .insert(decl.pool.clone(), decl.clone());
            match previous {
                Some(_) => format!("redeclare ledger pool {}", decl.pool),
                None => format!("declare ledger pool {}", decl.pool),
            }
        }
        SchemaChange::SetLedgerLimit(limit) => {
            let scope = scope_text(&limit.scope).to_string();
            let limits = &mut declarations_mut(schema).limits;
            limits.retain(|l| !same_limit(l, &limit.unit, &limit.owner, &scope));
            limits.push(limit.clone());
            limits.sort();
            match &limit.owner {
                None => format!("set the limits of ledger unit {}", limit.unit),
                Some(_) => format!(
                    "set the limits of one account of ledger unit {}",
                    limit.unit
                ),
            }
        }
        SchemaChange::RemoveLedgerDefinition(definition) => {
            let line = match definition {
                LedgerDefinitionRef::Unit { unit } => {
                    declarations_mut(schema)
                        .units
                        .remove(unit)
                        .ok_or_else(|| format!("no ledger unit {unit}"))?;
                    format!("remove ledger unit {unit}")
                }
                LedgerDefinitionRef::Pool { pool } => {
                    declarations_mut(schema)
                        .pools
                        .remove(pool)
                        .ok_or_else(|| format!("no ledger pool {pool}"))?;
                    format!("remove ledger pool {pool}")
                }
                LedgerDefinitionRef::Limit { unit, owner, scope } => {
                    let scope = scope_text(scope).to_string();
                    let limits = &mut declarations_mut(schema).limits;
                    let before = limits.len();
                    limits.retain(|l| !same_limit(l, unit, owner, &scope));
                    if limits.len() == before {
                        tidy(schema);
                        return Err(format!("no such limit of ledger unit {unit}"));
                    }
                    format!("remove a limit of ledger unit {unit}")
                }
            };
            tidy(schema);
            line
        }
        other => return Err(format!("not a ledger change: {other:?}")),
    };
    Ok(line)
}

/// The problems of a schema's ledger declarations, in a fixed order: the ceilings of LDG-1550, a
/// limit whose unit or pool is not declared or whose principal is not a valid owner, a limit of a
/// unit that names a scope, a floor above the cap, a cap per period without its period (or the
/// reverse), and a limit that sets nothing (CT-286).
pub fn declaration_problems(schema: &TenantSchema, problems: &mut Vec<String>) {
    let Some(ledger) = schema.ledger.as_ref() else {
        return;
    };
    if ledger.units.len() > limits::MAX_LEDGER_UNITS {
        problems.push(format!(
            "limit: {} ledger units is more than the maximum of {}",
            ledger.units.len(),
            limits::MAX_LEDGER_UNITS
        ));
    }
    if ledger.pools.len() > limits::MAX_LEDGER_POOLS {
        problems.push(format!(
            "limit: {} ledger pools is more than the maximum of {}",
            ledger.pools.len(),
            limits::MAX_LEDGER_POOLS
        ));
    }
    let account_limits = ledger.limits.iter().filter(|l| l.owner.is_some()).count();
    if account_limits > limits::MAX_LEDGER_ACCOUNT_LIMITS {
        problems.push(format!(
            "limit: {account_limits} ledger account limits is more than the maximum of {}",
            limits::MAX_LEDGER_ACCOUNT_LIMITS
        ));
    }
    for (unit, decl) in &ledger.units {
        if &decl.unit != unit {
            problems.push(format!("ledger unit {unit}: declared under another name"));
        }
    }
    for (pool, decl) in &ledger.pools {
        if &decl.pool != pool {
            problems.push(format!("ledger pool {pool}: declared under another name"));
        }
    }
    for limit in &ledger.limits {
        let at = format!("ledger limit of {}", limit.unit);
        if !ledger.units.contains_key(&limit.unit) {
            problems.push(format!("{at}: unknown ledger unit {}", limit.unit));
        }
        match &limit.owner {
            Some(LedgerOwner::Pool { pool }) if !ledger.pools.contains_key(pool) => {
                problems.push(format!("{at}: unknown ledger pool {pool}"));
            }
            None if limit.scope.as_ref().is_some_and(|s| !s.as_str().is_empty()) => {
                problems.push(format!("{at}: a limit of a unit names no scope"));
            }
            _ => {}
        }
        if scope_text(&limit.scope).len() > limits::MAX_LEDGER_SCOPE_BYTES {
            problems.push(format!(
                "{at}: a scope is at most {} bytes",
                limits::MAX_LEDGER_SCOPE_BYTES
            ));
        }
        if let (Some(floor), Some(cap)) = (limit.floor, limit.cap) {
            if floor.0 > cap.0 {
                problems.push(format!(
                    "{at}: the floor {} is above the cap {}",
                    floor.0, cap.0
                ));
            }
        }
        for amount in [limit.floor, limit.cap].into_iter().flatten() {
            if amount.0.abs() > limits::MAX_LEDGER_AMOUNT {
                problems.push(format!(
                    "{at}: {} is outside the range of an amount",
                    amount.0
                ));
            }
        }
        if limit.period_cap.is_some() != limit.period.is_some() {
            problems.push(format!(
                "{at}: a cap per period needs its period, and a period its cap"
            ));
        }
        if limit.floor.is_none() && limit.cap.is_none() && limit.period_cap.is_none() {
            problems.push(format!("{at}: sets no limit"));
        }
    }
}

/// The needs of the ledger changes of a schema change: whether an account names a unit or a
/// pool being removed (LDG-1549).
pub fn schema_change_needs(changes: &[SchemaChange]) -> Vec<Need> {
    changes
        .iter()
        .filter_map(|change| match change {
            SchemaChange::RemoveLedgerDefinition(LedgerDefinitionRef::Unit { unit }) => {
                Some(Need::Ledger(LedgerNeed::UnitUsed { unit: unit.clone() }))
            }
            SchemaChange::RemoveLedgerDefinition(LedgerDefinitionRef::Pool { pool }) => {
                Some(Need::Ledger(LedgerNeed::PoolUsed { pool: pool.clone() }))
            }
            _ => None,
        })
        .collect()
}

/// LDG-1549: a unit or a pool that an account names is not removed. Refused with
/// `StillReferenced`, after the checks in `evidence`.
pub fn check_removals(
    lookup: Lookup<'_>,
    changes: &[SchemaChange],
    evidence: &mut Vec<Check>,
) -> Result<(), DomainError> {
    for change in changes {
        let (need, what, used) = match change {
            SchemaChange::RemoveLedgerDefinition(LedgerDefinitionRef::Unit { unit }) => (
                LedgerNeed::UnitUsed { unit: unit.clone() },
                format!("ledger unit {unit}"),
                accounts_of(lookup.tenant).any(|a| &a.unit == unit),
            ),
            SchemaChange::RemoveLedgerDefinition(LedgerDefinitionRef::Pool { pool }) => (
                LedgerNeed::PoolUsed { pool: pool.clone() },
                format!("ledger pool {pool}"),
                accounts_of(lookup.tenant)
                    .any(|a| matches!(&a.owner, Some(LedgerOwner::Pool { pool: p }) if p == pool)),
            ),
            _ => continue,
        };
        let need = Need::Ledger(need);
        if !lookup.coverage.covers(&need) {
            return Err(insufficient(need));
        }
        if used {
            let message = format!("{what} is named by an account and is not removed");
            evidence.push(Check::fail("ledger:in-use", message.clone()));
            return Err(
                DomainError::new(ErrorKind::StillReferenced, message).with(evidence.clone())
            );
        }
        evidence.push(Check::pass(
            "ledger:in-use",
            format!("no account names {what}"),
        ));
    }
    Ok(())
}

fn accounts_of(tenant: &TenantState) -> impl Iterator<Item = &LedgerAccount> {
    tenant
        .ledger
        .as_ref()
        .into_iter()
        .flat_map(|ledger| ledger.accounts.values())
}

// ------------------------------------------------------------------------------------------------
// Reads (D-5011, D-6550 to D-6554)

/// Who may read the account of `owner`: a holder of `ledger-read` (the owner of the tenant holds
/// it) any account; a signed-in person, under `ledger-read-own`, an account the person owns
/// (LDG-026, LDG-1541). Policies over the ledger as a resource (D-6550) are not yet held by the
/// kernel: a holder of `ledger-read` reads every entry, so balances are answered from the header.
fn may_read(
    tenant: &TenantState,
    principal: &Principal,
    owner: Option<&LedgerOwner>,
) -> Result<Check, DomainError> {
    if holds(tenant, principal, LEDGER_READ) {
        return Ok(Check::pass(
            format!("permission:capability:{LEDGER_READ}"),
            format!("holds {LEDGER_READ}"),
        ));
    }
    if let (Principal::User(id), Some(LedgerOwner::Principal { principal: owned })) =
        (principal, owner)
    {
        if id == owned {
            return Ok(Check::pass(
                format!("permission:capability:{LEDGER_READ_OWN}"),
                "reads an account it owns",
            ));
        }
    }
    let rule = format!("permission:capability:{LEDGER_READ}");
    Err(DomainError::new(
        ErrorKind::Unauthorized,
        format!("a read of the ledger needs {LEDGER_READ}"),
    )
    .with(vec![Check::fail(
        rule,
        format!("does not hold {LEDGER_READ}"),
    )]))
}

/// The owner a read of one's own account names: the signed-in caller (CT-807, CT-808).
pub fn own_owner(ctx: &Context) -> Result<LedgerOwner, DomainError> {
    match &ctx.principal {
        Principal::User(id) => Ok(LedgerOwner::Principal {
            principal: id.clone(),
        }),
        _ => Err(DomainError::new(
            ErrorKind::Unauthorized,
            "a read of one's own ledger account needs a signed-in person",
        )
        .with(vec![Check::fail(
            format!("permission:capability:{LEDGER_READ_OWN}"),
            "no signed-in person",
        )])),
    }
}

fn unknown_unit(unit: &LedgerUnit) -> DomainError {
    refuse(
        ErrorKind::UnknownRecord,
        "ledger:unknown-unit",
        format!("the tenant declares no ledger unit {unit}"),
    )
}

fn no_account() -> DomainError {
    // CT-282: no account and an account the reader may read none of are not told apart.
    refuse(
        ErrorKind::UnknownRecord,
        "ledger:unknown-account",
        "no such ledger account",
    )
}

/// The needs of a read of one account's balance.
pub fn balance_needs(
    tenant: &TenantId,
    owner: &LedgerOwner,
    unit: &LedgerUnit,
    scope: &str,
) -> Vec<Need> {
    vec![Need::Ledger(LedgerNeed::Account {
        account: account_id(tenant, owner, unit, scope),
    })]
}

/// The balance of the account of `owner`, `unit` and `scope` (CT-282). With `own`, the read is
/// one of one's own account (CT-807): the owner must be the caller, and an account that does not
/// exist reads as 0 at sequence 0; otherwise it is not found.
pub fn read_balance(
    state: &State,
    owner: &LedgerOwner,
    unit: &LedgerUnit,
    scope: &str,
    own: bool,
    ctx: &Context,
) -> Result<LedgerBalance, DomainError> {
    let tenant = tenant_of(state, ctx)?;
    may_read(tenant, &ctx.principal, Some(owner))?;
    if !unit_declared(&tenant.schema, unit) {
        return Err(unknown_unit(unit));
    }
    let account = account_id(&tenant.id, owner, unit, scope);
    require(state, LedgerNeed::Account { account })?;
    match header(tenant, &account) {
        Some(found) => Ok(balance_of(found)),
        None if own => Ok(LedgerBalance {
            account,
            unit: unit.clone(),
            balance: LedgerAmount(0),
            last_sequence: 0,
            scope: (!scope.is_empty()).then(|| scope_value(scope)),
        }),
        None => Err(no_account()),
    }
}

/// The page size of an entry listing that names none, and the largest it may name (CT-283).
pub const DEFAULT_ENTRY_PAGE: u32 = 50;
pub const MAX_ENTRY_PAGE: u32 = 100;
/// The page size of a ranked listing that names none, and the largest it may name (CT-809).
pub const DEFAULT_RANK_PAGE: u32 = 20;
pub const MAX_RANK_PAGE: u32 = 100;

fn page(limit: Option<u32>, default: u32, max: u32) -> Result<u32, DomainError> {
    match limit.unwrap_or(default) {
        0 => Err(refuse(
            ErrorKind::InvalidIntent,
            "ledger:page",
            "a page holds at least one item",
        )),
        n if n > max => Err(refuse(
            ErrorKind::InvalidIntent,
            "ledger:page",
            format!("a page holds at most {max} items"),
        )),
        n => Ok(n),
    }
}

/// The needs of a page of the entries of one account: its header, and one entry more than the
/// page, to know whether another page follows.
pub fn entries_needs(
    tenant: &TenantId,
    owner: &LedgerOwner,
    unit: &LedgerUnit,
    scope: &str,
    after: Option<u64>,
    limit: Option<u32>,
) -> Result<Vec<Need>, DomainError> {
    let limit = page(limit, DEFAULT_ENTRY_PAGE, MAX_ENTRY_PAGE)?;
    let account = account_id(tenant, owner, unit, scope);
    Ok(vec![
        Need::Ledger(LedgerNeed::Account { account }),
        Need::Ledger(LedgerNeed::Entries {
            account,
            after,
            limit: limit + 1,
        }),
    ])
}

/// A page of the entries of one account, in the order of their numbers, and the number to pass
/// as `after` for the next page, if there is one (CT-283; with `own`, CT-808: an account that
/// does not exist has no entries).
pub fn read_entries(
    state: &State,
    owner: &LedgerOwner,
    unit: &LedgerUnit,
    scope: &str,
    after: Option<u64>,
    limit: Option<u32>,
    own: bool,
    ctx: &Context,
) -> Result<(Vec<LedgerEntry>, Option<u64>), DomainError> {
    let tenant = tenant_of(state, ctx)?;
    may_read(tenant, &ctx.principal, Some(owner))?;
    if !unit_declared(&tenant.schema, unit) {
        return Err(unknown_unit(unit));
    }
    let limit = page(limit, DEFAULT_ENTRY_PAGE, MAX_ENTRY_PAGE)?;
    let account = account_id(&tenant.id, owner, unit, scope);
    require(state, LedgerNeed::Account { account })?;
    if header(tenant, &account).is_none() {
        return if own {
            Ok((Vec::new(), None))
        } else {
            Err(no_account())
        };
    }
    require(
        state,
        LedgerNeed::Entries {
            account,
            after,
            limit: limit + 1,
        },
    )?;
    let mut entries: Vec<LedgerEntry> = loaded_entries(tenant, &account)
        .iter()
        .filter(|e| after.is_none_or(|a| e.sequence > a))
        .take(limit as usize + 1)
        .cloned()
        .collect();
    let next = if entries.len() > limit as usize {
        entries.truncate(limit as usize);
        entries.last().map(|e| e.sequence)
    } else {
        None
    };
    Ok((entries, next))
}

/// The needs of a ranked listing (CT-809): the first `offset + limit + 1` accounts of the unit
/// and scope in the order asked. Refused before anything is read when the walk would examine
/// more than [`limits::MAX_LEDGER_RANK_SCAN`] accounts (LDG-1543).
pub fn ranked_needs(
    unit: &LedgerUnit,
    scope: &str,
    order: LedgerOrder,
    offset: u32,
    limit: Option<u32>,
) -> Result<Vec<Need>, DomainError> {
    let limit = page(limit, DEFAULT_RANK_PAGE, MAX_RANK_PAGE)?;
    let examined = u64::from(offset) + u64::from(limit);
    if examined > u64::from(limits::MAX_LEDGER_RANK_SCAN) {
        return Err(refuse(
            ErrorKind::LimitExceeded,
            "ledger:scan-bound-exceeded",
            format!(
                "scan-bound-exceeded: the listing would examine {examined} accounts, more than {}",
                limits::MAX_LEDGER_RANK_SCAN
            ),
        ));
    }
    Ok(vec![Need::Ledger(LedgerNeed::Ranked {
        unit: unit.clone(),
        scope: scope_value(scope),
        order,
        offset,
        limit: limit + 1,
    })])
}

/// A page of the accounts of a unit and scope in order of balance (CT-809, LDG-1543), and the
/// offset of the next page, if there is one. The reader holds `ledger-read`, so every entry of
/// every account is one the reader may read (LDG-1544); an account whose owner was erased has no
/// owner to show and is left out.
pub fn read_ranked(
    state: &State,
    unit: &LedgerUnit,
    scope: &str,
    order: LedgerOrder,
    offset: u32,
    limit: Option<u32>,
    ctx: &Context,
) -> Result<(Vec<LedgerAccountRank>, Option<u32>), DomainError> {
    let tenant = tenant_of(state, ctx)?;
    may_read(tenant, &ctx.principal, None)?;
    if !unit_declared(&tenant.schema, unit) {
        return Err(unknown_unit(unit));
    }
    let need = ranked_needs(unit, scope, order, offset, limit)?;
    let Some(Need::Ledger(need)) = need.into_iter().next() else {
        unreachable!("ranked_needs names one ledger need");
    };
    let limit = page(limit, DEFAULT_RANK_PAGE, MAX_RANK_PAGE)?;
    require(state, need)?;
    let mut matching: Vec<&LedgerAccount> = accounts_of(tenant)
        .filter(|a| &a.unit == unit && a.scope.as_str() == scope)
        .collect();
    matching.sort_by(rank_order(order));
    let window: Vec<&LedgerAccount> = matching
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize + 1)
        .collect();
    let next = (window.len() > limit as usize).then(|| offset + limit);
    let rows = window
        .into_iter()
        .take(limit as usize)
        .filter_map(|a| {
            a.owner.clone().map(|owner| LedgerAccountRank {
                owner,
                scope: a.scope.clone(),
                balance: a.balance,
                last_sequence: a.last_sequence,
            })
        })
        .collect();
    Ok((rows, next))
}

/// Every unit, pool and limit the tenant has declared (CT-810, LDG-1548), to a holder of
/// `ledger-read`. Needs only the header.
pub fn read_definitions(state: &State, ctx: &Context) -> Result<LedgerDefinitions, DomainError> {
    let tenant = tenant_of(state, ctx)?;
    may_read(tenant, &ctx.principal, None)?;
    let declared = declarations(&tenant.schema);
    Ok(LedgerDefinitions {
        units: declared.units.keys().cloned().collect(),
        pools: declared.pools.keys().cloned().collect(),
        limits: declared.limits,
    })
}

fn tenant_of<'a>(state: &'a State, ctx: &Context) -> Result<&'a TenantState, DomainError> {
    let mismatch = || {
        refuse(
            ErrorKind::TenantMismatch,
            "tenant:snapshot_matches_context",
            "snapshot holds another tenant",
        )
    };
    if state.tenant_id != ctx.tenant {
        return Err(mismatch());
    }
    let tenant = state
        .tenant
        .as_deref()
        .ok_or_else(|| refuse(ErrorKind::TenantNotFound, "tenant:exists", "no such tenant"))?;
    if tenant.id != ctx.tenant {
        return Err(mismatch());
    }
    Ok(tenant)
}

/// The sum of the amounts of entries numbered 1, 2, 3 without a gap (CT-287, LDG-019,
/// LDG-020): `Ok(sum)`, or `Err(n)` naming the first number that is missing or repeated.
pub fn replayed_balance(entries: &[LedgerEntry]) -> Result<i64, u64> {
    let mut sum: i64 = 0;
    for (index, entry) in entries.iter().enumerate() {
        let expected = index as u64 + 1;
        if entry.sequence != expected {
            return Err(expected);
        }
        sum = sum.checked_add(entry.amount.0).ok_or(expected)?;
    }
    Ok(sum)
}
