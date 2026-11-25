//! The transaction of the double-entry journal (D-8430, LDG-2903 to LDG-2917).
//!
//! A transaction is one committed fact of at least two entries whose amounts net to zero for each
//! unit, with a header that says who made it and why. [`decide`] is the decision of the intent
//! `PostLedgerTransaction` after the capability `ledger-post` is checked; [`post`] is the
//! decision without the capability check, for a tenant operation or an automation step that acts
//! with the authority of its definition (D-6547). Both are pure: the actor, the correlation and
//! the time come from the context.
//!
//! Each leg is a posting of [`crate::ledger::post_in`], so every rule of a posting holds for a
//! leg: a declared unit and owner, an amount in range, a floor, a cap and a cap per period, the
//! chain of the account. The legs of one transaction see each other (D-6548), so a pool that
//! funds several recipients is checked against its floor after each of them, and the transaction
//! is refused as a whole when one leg is refused. Its entries are one commit with its audit line
//! (LDG-2904, LDG-2925).
//!
//! The entry of leg `n` is keyed `<key>:<n>`, and names the transaction, which is a hash of the
//! tenant, the key and the request, so that a retry with the same request finds its first entries
//! and posts nothing (D-8437), and a retry with another request under the key is refused as
//! `ledger:idempotency-mismatch`.

use crate::canon;
use crate::intent::{
    Check, CheckExt, Context, DeltaOp, DomainError, DomainErrorExt, ErrorKind, Output,
};
use crate::ledger::{self, Decided, LEDGER_POST};
use crate::limits;
use crate::settings;
use crate::state::{Need, Principal, State, TenantState};
use crate::types::{
    LedgerOwner, LedgerPostingRequest, LedgerPostingResult, LedgerTransaction, LedgerUnit,
    SettingId, TenantId, Value,
};
use std::collections::BTreeMap;

/// The setting in which a tenant declares the most entries of one transaction (LDG-2914), a whole
/// number up to [`limits::MAX_LEDGER_TRANSACTION_LEGS`].
pub const MAX_LEGS_SETTING: &str = "ledger_max_transaction_legs";
/// The longest operation name a header records.
const MAX_OPERATION_BYTES: usize = 128;

/// The id of a transaction: a hash of the tenant, the key and the request, so that the same
/// request under the same key is the same transaction.
pub fn transaction_id(tenant: &TenantId, tx: &LedgerTransaction) -> String {
    canon::hash_of(&(
        "strata-ledger-transaction",
        tenant,
        &tx.idempotency_key,
        &tx.reason,
        &tx.legs,
    ))
}

/// The posting that leg `n` of a transaction asks for.
pub fn leg_request(tx: &LedgerTransaction, n: usize) -> LedgerPostingRequest {
    let leg = &tx.legs[n];
    LedgerPostingRequest {
        owner: leg.owner.clone(),
        unit: leg.unit.clone(),
        amount: leg.amount,
        idempotency_key: format!("{}:{n}", tx.idempotency_key),
        reason: tx.reason.clone(),
        corrects: None,
        scope: leg.scope.clone(),
    }
}

/// The needs of a transaction: those of each of its legs.
pub fn needs(tenant: &TenantId, tx: &LedgerTransaction) -> Vec<Need> {
    let mut out = std::collections::BTreeSet::new();
    for n in 0..tx.legs.len() {
        out.extend(ledger::posting_needs(tenant, &leg_request(tx, n)));
    }
    out.into_iter().collect()
}

/// The most entries a transaction of the tenant may name: its declared maximum, never above the
/// platform's (LDG-2914).
pub fn max_legs(tenant: &TenantState) -> usize {
    let ceiling = limits::MAX_LEDGER_TRANSACTION_LEGS;
    let Ok(id) = SettingId::new(MAX_LEGS_SETTING) else {
        return ceiling;
    };
    match settings::resolve(tenant, &Principal::System, &id) {
        Ok(found) => match found.value {
            Value::Int(n) if n >= 2 => (n as usize).min(ceiling),
            _ => ceiling,
        },
        Err(_) => ceiling,
    }
}

fn refuse_with(
    evidence: Vec<Check>,
    kind: ErrorKind,
    rule: &str,
    message: impl Into<String>,
) -> DomainError {
    ledger::refused(evidence, kind, rule, message)
}

/// Decide the intent `PostLedgerTransaction`: the principal holds `ledger-post`, then [`post`].
pub fn decide(
    state: &State,
    tx: &LedgerTransaction,
    ctx: &Context,
) -> Result<Decided, DomainError> {
    let Some(tenant) = state.tenant.as_deref() else {
        return Err(ledger::refuse(
            ErrorKind::TenantNotFound,
            "tenant:exists",
            "no such tenant",
        ));
    };
    let rule = format!("permission:capability:{LEDGER_POST}");
    if !ledger::holds(tenant, &ctx.principal, LEDGER_POST) {
        return Err(DomainError::new(
            ErrorKind::Unauthorized,
            format!("a posting needs the capability {LEDGER_POST}"),
        )
        .with(vec![Check::fail(
            rule,
            format!("does not hold {LEDGER_POST}"),
        )]));
    }
    let mut decided = post(state, &[], tx, ctx)?;
    decided
        .evidence
        .insert(0, Check::pass(rule, format!("holds {LEDGER_POST}")));
    Ok(decided)
}

/// Decide a transaction after the operations `earlier` of the same change, checking no capability.
///
/// Refusals, each changing nothing: a header that lacks the actor, the operation, the key or the
/// correlation is `ledger:header-incomplete` (LDG-2906); more entries than the tenant allows is
/// `ledger:too-many-legs` (LDG-2914); fewer than two entries, or amounts that do not sum to zero
/// for a unit, is `ledger:unbalanced-transaction` (LDG-2903); a leg that a
/// posting would refuse is refused with the rule of the posting.
pub fn post(
    state: &State,
    earlier: &[DeltaOp],
    tx: &LedgerTransaction,
    ctx: &Context,
) -> Result<Decided, DomainError> {
    let Some(tenant) = state.tenant.as_deref() else {
        return Err(ledger::refuse(
            ErrorKind::TenantNotFound,
            "tenant:exists",
            "no such tenant",
        ));
    };
    let mut evidence = Vec::new();

    // LDG-2906: the header records who, what, under which key and correlation.
    let actor = match &ctx.principal {
        Principal::User(id) => Some(id.clone()),
        _ => None,
    };
    let missing: Vec<&str> = [
        ("the actor", actor.is_some()),
        (
            "the operation",
            !tx.operation.is_empty() && tx.operation.len() <= MAX_OPERATION_BYTES,
        ),
        (
            "the idempotency key",
            !tx.idempotency_key.is_empty()
                && tx.idempotency_key.len() + 8 <= limits::MAX_LEDGER_KEY_BYTES,
        ),
        ("the correlation identifier", !ctx.correlation.is_empty()),
    ]
    .into_iter()
    .filter_map(|(name, present)| (!present).then_some(name))
    .collect();
    if !missing.is_empty() {
        return Err(refuse_with(
            evidence,
            ErrorKind::InvalidIntent,
            "ledger:header-incomplete",
            format!("a transaction header records {}", missing.join(", ")),
        ));
    }
    evidence.push(Check::pass(
        "ledger:header",
        format!("operation {}, key {}", tx.operation, tx.idempotency_key),
    ));

    // LDG-2914: no more entries than the tenant declares.
    let most = max_legs(tenant);
    if tx.legs.len() > most {
        return Err(refuse_with(
            evidence,
            ErrorKind::ConstraintViolated,
            "ledger:too-many-legs",
            format!(
                "a transaction names at most {most} entries, not {}",
                tx.legs.len()
            ),
        ));
    }

    // LDG-2903: at least two entries that net to zero for each unit.
    if tx.legs.len() < 2 {
        return Err(refuse_with(
            evidence,
            ErrorKind::ConstraintViolated,
            "ledger:unbalanced-transaction",
            format!(
                "a transaction has at least two entries, not {}",
                tx.legs.len()
            ),
        ));
    }
    let mut sums: BTreeMap<&LedgerUnit, i128> = BTreeMap::new();
    for leg in &tx.legs {
        *sums.entry(&leg.unit).or_default() += i128::from(leg.amount.0);
    }
    if let Some((unit, sum)) = sums.iter().find(|(_, sum)| **sum != 0) {
        return Err(refuse_with(
            evidence,
            ErrorKind::ConstraintViolated,
            "ledger:unbalanced-transaction",
            format!("the amounts of unit {unit} sum to {sum}, not to zero"),
        ));
    }
    evidence.push(Check::pass(
        "ledger:balanced",
        format!(
            "{} entries net to zero in {} unit(s)",
            tx.legs.len(),
            sums.len()
        ),
    ));

    // The legs, each a posting that sees the ones before it.
    let id = transaction_id(&tenant.id, tx);
    let mut ops: Vec<DeltaOp> = Vec::new();
    let mut events = Vec::new();
    let mut results: Vec<LedgerPostingResult> = Vec::new();
    for n in 0..tx.legs.len() {
        let request = leg_request(tx, n);
        let mut before: Vec<DeltaOp> = earlier.to_vec();
        before.extend(ops.iter().cloned());
        let posted =
            ledger::post_in(state, &before, &request, Some(&id), ctx).map_err(|mut error| {
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
    // A retry replays every leg or none: a transaction found in part is another request.
    let replayed = results.iter().filter(|r| r.replayed).count();
    if replayed != 0 && replayed != results.len() {
        return Err(refuse_with(
            evidence,
            ErrorKind::VersionConflict,
            "ledger:idempotency-mismatch",
            format!(
                "the key {} was recorded for some entries of another transaction",
                tx.idempotency_key
            ),
        ));
    }
    Ok(Decided {
        ops,
        events,
        evidence,
        output: Output::LedgerPosted { results },
    })
}

/// The refusal of an approval of a held transaction by the person who made it, or by the person
/// it was made for (LDG-2907): `ledger:self-approval`. The header of the held transaction names
/// the actor and the principal acted for, so that no operation can skip the rule. `None` when the
/// approver is neither.
pub fn approval_refusal(
    approver: &Principal,
    actor: &crate::types::PrincipalId,
    acted_for: Option<&crate::types::PrincipalId>,
) -> Option<DomainError> {
    let Principal::User(who) = approver else {
        return None;
    };
    (who == actor || acted_for == Some(who)).then(|| {
        ledger::refuse(
            ErrorKind::Unauthorized,
            "ledger:self-approval",
            "a held transaction is not approved by its actor or by the person it was made for",
        )
    })
}
