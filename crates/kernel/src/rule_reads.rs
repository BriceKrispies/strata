//! What the reads of the tenant rule language that look past the record give for the acting
//! principal: `balance(unit, scope)` and `setting(name)` (D-6544, CMS-1547, CMS-1549, CMS-1551).
//!
//! The rule language does not hold these operators yet: D-6543 lands them in Exegra's catalogue
//! first, then in specc's, the kernel and the language entity. This module is the kernel's half
//! that does not depend on the generated `Expr`: the value of each read over the snapshot of a
//! decision, and the need a read asks for when the snapshot does not cover it. The environment
//! of [`crate::expr`] calls it once the generated `Env` has the methods.
//!
//! - `balance(unit, scope)` reads the account of the acting principal for that unit and scope
//!   (D-6551). Its id is derived from the tenant, the owner, the unit and the scope (D-5001), so
//!   the read names its need, [`balance_need`], before anything is loaded. An account never posted
//!   to holds 0; an anonymous or system principal has no account and reads null. A snapshot that
//!   does not cover the account is `InsufficientState` naming it, as a reference followed into a
//!   record that was not loaded is ([`crate::expr::missing`]), never a balance of 0.
//! - `setting(name)` is the value [`settings::resolve`] gives the acting principal (SET-010). The
//!   overrides of the settings are loaded with the tenant, so the read asks for no need of its own.
//!
//! `in_segment(name)` is not here: a decision's snapshot holds no segment membership and `Need`
//! has no variant that asks for one, so a membership read waits for that need (D-6544).

use crate::intent::{DomainError, DomainErrorExt, ErrorKind};
use crate::ledger;
use crate::rules::{insufficient, Lookup};
use crate::settings;
use crate::state::{CoverageExt, Need, Principal};
use crate::types::{
    LedgerAccountId, LedgerNeed, LedgerOwner, LedgerUnit, SettingId, TenantId, Value,
};

/// The account `balance(unit, scope)` reads for `principal`: the principal's own account of the
/// unit in the scope. `None` for an anonymous or system principal, which owns no account, and for
/// a unit name that cannot be a unit.
pub fn balance_account(
    tenant: &TenantId,
    principal: &Principal,
    unit: &str,
    scope: &str,
) -> Option<LedgerAccountId> {
    let Principal::User(id) = principal else {
        return None;
    };
    let unit = LedgerUnit::new(unit).ok()?;
    let owner = LedgerOwner::Principal {
        principal: id.clone(),
    };
    Some(ledger::account_id(tenant, &owner, &unit, scope))
}

/// The need of `balance(unit, scope)` read by `principal`: the account's derived balance record.
/// `None` when the read loads nothing (an anonymous or system principal).
pub fn balance_need(
    tenant: &TenantId,
    principal: &Principal,
    unit: &str,
    scope: &str,
) -> Option<Need> {
    balance_account(tenant, principal, unit, scope)
        .map(|account| Need::Ledger(LedgerNeed::Account { account }))
}

/// The value of `balance(unit, scope)` for `principal` over the snapshot `lookup` reads (CMS-1547):
/// the balance of the principal's account, 0 when the account does not exist, null for an
/// anonymous or system principal. A unit the tenant does not declare is an error; an account the
/// snapshot does not cover is `InsufficientState` naming its need.
pub fn balance(
    lookup: &Lookup<'_>,
    principal: &Principal,
    unit: &str,
    scope: &str,
) -> Result<Value, DomainError> {
    let declared = LedgerUnit::new(unit).ok().is_some_and(|id| {
        lookup
            .tenant
            .schema
            .ledger
            .as_ref()
            .is_some_and(|ledger| ledger.units.contains_key(&id))
    });
    if !declared {
        return Err(DomainError::new(
            ErrorKind::UnknownRecord,
            format!("the tenant declares no ledger unit {unit}"),
        ));
    }
    let Some(account) = balance_account(&lookup.tenant.id, principal, unit, scope) else {
        return Ok(Value::Null);
    };
    let need = Need::Ledger(LedgerNeed::Account { account });
    if !lookup.coverage.covers(&need) {
        return Err(insufficient(need));
    }
    let held = lookup
        .tenant
        .ledger
        .as_ref()
        .and_then(|state| state.accounts.get(&ledger::account_key(&account)));
    Ok(Value::Int(held.map_or(0, |found| found.balance.0)))
}

/// The value of `setting(name)` for `principal` (CMS-1549): the value [`settings::resolve`] gives,
/// the person's own override first and the default last. A setting the tenant does not define is
/// the error `resolve` gives.
pub fn setting(
    lookup: &Lookup<'_>,
    principal: &Principal,
    name: &str,
) -> Result<Value, DomainError> {
    let Some(id) = SettingId::new(name).ok() else {
        return Err(DomainError::new(
            ErrorKind::UnknownRecord,
            format!("no setting {name}"),
        ));
    };
    settings::resolve(lookup.tenant, principal, &id).map(|resolved| resolved.value)
}
