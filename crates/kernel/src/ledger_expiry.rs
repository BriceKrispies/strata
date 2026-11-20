//! Expiry of ledger credits by period (D-6553, LDG-1545 to LDG-1547).
//!
//! A unit may declare an expiry (`LedgerUnitDecl::expiry`): a calendar period and a number of
//! periods from one to thirty-six. A credit belongs to the bucket of the period that contains its
//! time; the header of an account keeps the remaining amount of each bucket
//! (`LedgerAccount::buckets`, oldest first, at most [`MAX_BUCKETS`]). A debit consumes the
//! remaining credit of the buckets oldest first. When the last period of the life of a bucket has
//! ended, the intent `ExpireLedger` ([`decide`]) posts, for each account with credit remaining in
//! such a bucket, one entry of minus the remaining amount with the key `expiry:<bucket start>`
//! and the reason `expiry`, for at most [`EXPIRY_BATCH`] accounts in one transaction.
//!
//! Expiry is an ordinary entry, so a balance is still the sum of its entries (LDG-020) and never
//! depends on the clock: nothing here reads a clock, the time is the context's.
//!
//! Decisions of this module, stated because the specification leaves them open:
//!
//! - Buckets count only the credits posted while the unit declares an expiry. An account that
//!   held a balance before the unit declared one keeps it, and it does not expire.
//! - A credit that arrives while the balance is negative first pays off the debt; only the rest
//!   enters a bucket, so that the buckets always add up to the balance when it is positive.
//! - An account holds at most [`MAX_BUCKETS`] buckets. A credit that would open another merges
//!   the two oldest into the newer one: the oldest credit then lives longer, never shorter.
//! - An expiry entry is not held to the floor of the account: it removes credit that the holder
//!   could no longer count on, and refusing it would keep the account in the index of due
//!   accounts for ever.
//! - The key prefix `expiry:` belongs to this module: an ordinary posting that uses it is refused,
//!   so that an expiry entry never collides with a key a poster chose.

use crate::intent::{Check, CheckExt, Context, DeltaOp, DomainError, DomainErrorExt, ErrorKind};
use crate::intent::{EventBody, Output};
use crate::ledger::{self, Decided, LEDGER_CONFIGURE};
use crate::rules::holds_any;
use crate::schema::{Grant, TenantSchema};
use crate::state::{CoverageExt, Need, Principal, State, TenantState};
use crate::types::{
    LedgerAccount, LedgerAmount, LedgerBucket, LedgerDelta, LedgerEntry, LedgerExpiry, LedgerNeed,
    LedgerPeriod, LedgerPostingResult, LedgerUnit, Timestamp,
};
use std::collections::BTreeSet;

/// The most accounts one expiry transaction handles (LDG-1547).
pub const EXPIRY_BATCH: u32 = 100;
/// The most buckets an account header keeps (D-6553).
pub const MAX_BUCKETS: usize = 36;
/// The reason of an expiry entry.
pub const EXPIRY_REASON: &str = "expiry";
/// The prefix of the key of an expiry entry; the rest is the start of the bucket in milliseconds.
pub const EXPIRY_KEY_PREFIX: &str = "expiry:";

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 86_400_000;

/// The key of the expiry entry of the bucket that starts at `start`.
pub fn expiry_key(start: Timestamp) -> String {
    format!("{EXPIRY_KEY_PREFIX}{}", start.0)
}

/// The expiry a schema declares for a unit, if it declares one.
pub fn expiry_of<'a>(schema: &'a TenantSchema, unit: &LedgerUnit) -> Option<&'a LedgerExpiry> {
    schema
        .ledger
        .as_ref()
        .and_then(|l| l.units.get(unit))
        .and_then(|decl| decl.expiry.as_ref())
}

/// The start of the period that lies `n` periods from the period that starts at `start`; `n` may
/// be negative. Weeks start on Monday and months are calendar months, in UTC.
pub fn shift_periods(period: LedgerPeriod, start: Timestamp, n: i64) -> Timestamp {
    match period {
        LedgerPeriod::Hour => Timestamp(start.0 + n * HOUR_MS),
        LedgerPeriod::Day => Timestamp(start.0 + n * DAY_MS),
        LedgerPeriod::Week => Timestamp(start.0 + n * 7 * DAY_MS),
        LedgerPeriod::Month => {
            let (year, month) = ledger::civil_month(start.0.div_euclid(DAY_MS));
            let total = year * 12 + (month - 1) + n;
            let year = total.div_euclid(12);
            let month = total.rem_euclid(12) + 1;
            Timestamp(ledger::days_from_civil(year, month, 1) * DAY_MS)
        }
    }
}

/// The instant the life of the bucket that starts at `start` ends: the start of the period that
/// lies `periods` periods after it.
pub fn life_end(expiry: &LedgerExpiry, start: Timestamp) -> Timestamp {
    shift_periods(expiry.period, start, i64::from(expiry.periods))
}

/// The start of the latest bucket whose life has ended at `at`: every bucket that starts at or
/// before it is due. The life of the bucket that starts at the cutoff ends at the start of the
/// period that contains `at`, and the life of the next bucket ends after `at`.
pub fn cutoff(expiry: &LedgerExpiry, at: Timestamp) -> Timestamp {
    shift_periods(
        expiry.period,
        ledger::period_start(expiry.period, at),
        -i64::from(expiry.periods),
    )
}

/// The buckets of an account after an entry of `amount` at `now` (`before` is the balance before
/// it): a credit adds to the bucket of its period, a debit consumes the buckets oldest first.
/// Buckets that reach zero are dropped; the buckets stay in order of their start.
pub fn apply_entry(
    buckets: &[LedgerBucket],
    before: i64,
    amount: i64,
    now: Timestamp,
    expiry: &LedgerExpiry,
) -> Vec<LedgerBucket> {
    let mut out: Vec<LedgerBucket> = buckets.to_vec();
    if amount > 0 {
        let debt = if before < 0 { before.unsigned_abs() } else { 0 };
        let credit = (amount as u64).saturating_sub(debt);
        if credit > 0 {
            let start = ledger::period_start(expiry.period, now);
            match out.iter_mut().find(|b| b.period_start == start) {
                Some(bucket) => bucket.remaining = bucket.remaining.saturating_add(credit),
                None => {
                    out.push(LedgerBucket {
                        period_start: start,
                        remaining: credit,
                    });
                    out.sort_by_key(|b| b.period_start);
                }
            }
            if out.len() > MAX_BUCKETS {
                // The two oldest become one, dated by the newer: its credit lives longer.
                let oldest = out.remove(0);
                out[0].remaining = out[0].remaining.saturating_add(oldest.remaining);
            }
        }
    } else {
        let mut left = amount.unsigned_abs();
        for bucket in out.iter_mut() {
            if left == 0 {
                break;
            }
            let taken = left.min(bucket.remaining);
            bucket.remaining -= taken;
            left -= taken;
        }
        out.retain(|b| b.remaining > 0);
    }
    out
}

/// `buckets` as the header keeps them: none at all when there is no credit to expire.
pub fn header_buckets(buckets: Vec<LedgerBucket>) -> Option<Vec<LedgerBucket>> {
    (!buckets.is_empty()).then_some(buckets)
}

/// The start of the oldest bucket of a header: what the index of due accounts orders by.
pub fn oldest_bucket(account: &LedgerAccount) -> Option<Timestamp> {
    account
        .buckets
        .as_ref()
        .and_then(|buckets| buckets.first())
        .map(|bucket| bucket.period_start)
}

/// The accounts of `unit` whose oldest bucket starts at or before `cutoff`, in order of that start
/// and then of the account id, at most `limit` of them: what the need `Expiring` names.
pub fn due_accounts<'a>(
    accounts: impl Iterator<Item = &'a LedgerAccount>,
    unit: &LedgerUnit,
    cutoff: Timestamp,
    limit: usize,
) -> Vec<&'a LedgerAccount> {
    let mut due: Vec<(&LedgerAccount, Timestamp)> = accounts
        .filter(|a| &a.unit == unit)
        .filter_map(|a| {
            oldest_bucket(a)
                .filter(|start| *start <= cutoff)
                .map(|s| (a, s))
        })
        .collect();
    due.sort_by(|(a, x), (b, y)| x.cmp(y).then_with(|| a.account.cmp(&b.account)));
    due.into_iter().take(limit).map(|(a, _)| a).collect()
}

/// The needs of an `ExpireLedger` intent: the due accounts, at most one batch.
pub fn needs(unit: &LedgerUnit, cutoff: Timestamp) -> Vec<Need> {
    vec![Need::Ledger(LedgerNeed::Expiring {
        unit: unit.clone(),
        cutoff,
        limit: EXPIRY_BATCH,
    })]
}

/// Who may decide the expiry of credits: the system, or a holder of `ledger-configure`.
pub fn may_expire(tenant: &TenantState, principal: &Principal) -> bool {
    matches!(principal, Principal::System)
        || holds_any(
            tenant,
            principal,
            &BTreeSet::from([Grant::Capability(LEDGER_CONFIGURE.to_string())]),
        )
}

fn refuse_with(kind: ErrorKind, rule: &str, message: impl Into<String>) -> DomainError {
    let message = message.into();
    DomainError::new(kind, format!("{rule}: {message}")).with(vec![Check::fail(rule, message)])
}

/// Decide the intent `ExpireLedger` (contract expire-ledger-credits, CT-812): one transaction
/// that expires every bucket due at `cutoff` in at most [`EXPIRY_BATCH`] accounts of `unit`.
///
/// Who may decide it: the system (the job of the tenant) or a holder of `ledger-configure`; the
/// effect is the same for both, since the decision depends on the cutoff and the state only.
///
/// Refusals, each changing nothing: a unit that declares no expiry or a cutoff of a bucket whose
/// life has not ended at `ctx.now` is `InvalidIntent`; a principal that may not is `Unauthorized`.
/// A snapshot that lacks the due accounts is `InsufficientState` naming them. The answer lists
/// one posting result per entry written, in order; an empty list means nothing was due.
pub fn decide(
    state: &State,
    unit: &LedgerUnit,
    cutoff_at: Timestamp,
    ctx: &Context,
) -> Result<Decided, DomainError> {
    let Some(tenant) = state.tenant.as_deref() else {
        return Err(refuse_with(
            ErrorKind::TenantNotFound,
            "tenant:exists",
            "no such tenant",
        ));
    };
    if !may_expire(tenant, &ctx.principal) {
        return Err(DomainError::new(
            ErrorKind::Unauthorized,
            format!("the expiry of credits needs the system or {LEDGER_CONFIGURE}"),
        )
        .with(vec![Check::fail(
            format!("permission:capability:{LEDGER_CONFIGURE}"),
            format!("does not hold {LEDGER_CONFIGURE}"),
        )]));
    }
    let Some(expiry) = expiry_of(&tenant.schema, unit) else {
        return Err(refuse_with(
            ErrorKind::InvalidIntent,
            "ledger:no-expiry",
            format!("the unit {unit} declares no expiry"),
        ));
    };
    let latest = cutoff(expiry, ctx.now);
    if cutoff_at > latest {
        return Err(refuse_with(
            ErrorKind::InvalidIntent,
            "ledger:bucket-alive",
            format!(
                "the bucket that starts at {} has not ended its life; the latest that has starts at {}",
                cutoff_at.0, latest.0
            ),
        ));
    }
    let need = LedgerNeed::Expiring {
        unit: unit.clone(),
        cutoff: cutoff_at,
        limit: EXPIRY_BATCH,
    };
    if !state.coverage.covers(&Need::Ledger(need.clone())) {
        return Err(crate::rules::insufficient(Need::Ledger(need)));
    }
    let due = due_accounts(
        tenant.ledger.iter().flat_map(|l| l.accounts.values()),
        unit,
        cutoff_at,
        EXPIRY_BATCH as usize,
    );
    let mut ops = Vec::new();
    let mut events = Vec::new();
    let mut results = Vec::new();
    let mut evidence = vec![Check::pass(
        "ledger:expiry",
        format!(
            "{} account(s) hold credit in buckets that started at or before {}",
            due.len(),
            cutoff_at.0
        ),
    )];
    for account in due {
        let mut header = account.clone();
        let due_buckets: Vec<LedgerBucket> = header
            .buckets
            .iter()
            .flatten()
            .filter(|b| b.period_start <= cutoff_at && b.remaining > 0)
            .cloned()
            .collect();
        for bucket in due_buckets {
            let amount = -(bucket.remaining as i64);
            let before = header.balance.0;
            let after = before + amount;
            let next_buckets = apply_entry(
                header.buckets.as_deref().unwrap_or(&[]),
                before,
                amount,
                ctx.now,
                expiry,
            );
            header = LedgerAccount {
                balance: LedgerAmount(after),
                last_sequence: header.last_sequence + 1,
                buckets: header_buckets(next_buckets),
                ..header
            };
            let entry = LedgerEntry {
                account: header.account,
                sequence: header.last_sequence,
                amount: LedgerAmount(amount),
                idempotency_key: expiry_key(bucket.period_start),
                reason: EXPIRY_REASON.to_string(),
                caused_by: None,
                at: ctx.now,
                corrects: None,
            };
            if let Some(owner) = &header.owner {
                events.push(EventBody::LedgerEntryPosted {
                    account: header.account,
                    owner: owner.clone(),
                    unit: unit.clone(),
                    scope: header.scope.clone(),
                    amount: entry.amount,
                    balance: header.balance,
                    balance_before: LedgerAmount(before),
                    entry_sequence: entry.sequence,
                    reason: entry.reason.clone(),
                });
            }
            results.push(LedgerPostingResult {
                balance: ledger::balance_of(&header),
                entry: entry.clone(),
                replayed: false,
            });
            // The header of this entry is written once more by the next entry of the account, and
            // the commit keeps the last: each entry continues the number the header before it holds.
            ops.push(DeltaOp::Ledger(LedgerDelta::PutAccount(header.clone())));
            ops.push(DeltaOp::Ledger(LedgerDelta::AppendEntry(entry)));
        }
    }
    evidence.push(Check::pass(
        "ledger:expired",
        format!("{} entr(ies) of expiry", results.len()),
    ));
    Ok(Decided {
        ops,
        events,
        evidence,
        output: Output::LedgerPosted { results },
    })
}
