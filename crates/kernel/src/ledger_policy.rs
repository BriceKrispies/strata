//! Policies over the ledger (D-6550, LDG-1540, LDG-1544, WG-5168).
//!
//! A tenant policy may name the ledger, or the ledger of one unit, as its resource
//! (`Resource::Ledger { unit }`) and permit `read`. Its condition reads the account as its
//! record: `owner` (the owning principal as text, null for a pool or an erased owner), `pool`,
//! `unit` and `scope`; and, for an entry, also `amount`, `reason`, `caused_by` and `at`. "The
//! owner reads their own" is the policy with the condition `record.owner == principal`.
//!
//! How a policy and a capability combine, decided here because D-6550 leaves it open:
//!
//! - A principal that holds `ledger-read` (the owner of the tenant does) and has no ledger policy
//!   among its roles reads every account, as before.
//! - A principal that has a ledger policy among its roles, whether or not it holds `ledger-read`,
//!   reads exactly what the policies of its roles permit: the policies are a grant (the first that
//!   holds permits, default deny), and they also bound a holder of `ledger-read`.
//! - Whoever owns an account reads it and every entry of it under `ledger-read-own`, with or
//!   without a policy (D-6550).
//! - A policy of a unit applies to the accounts of that unit and one without a unit to every
//!   account; a policy never widens what a reader of another role may read.
//!
//! A balance and a ranked listing answer for a whole account, so they are permitted only when the
//! policies permit the account with the fields of an entry absent: a condition that reads an
//! entry's `amount`, `reason`, `caused_by` or `at` is then null, and does not permit. A reader of
//! such a policy reads entries (each judged on its own) and no balance of the account, which
//! keeps a balance from disclosing an entry the reader may not read (D-5011, LDG-025). Deriving a
//! balance from the readable entries (CT-288) is not built.
//!
//! The decision is `rules::authorize` for the action `read` on the resource `Ledger`, once for
//! the unit and once for every unit, so the checks, the owner's bypass and the evaluation of a
//! condition are the ones every other resource has.

use crate::rules::{authorize, roles_of, Subject};
use crate::schema::{Action, PolicyDef, Resource, TenantSchema};
use crate::state::{Principal, Record, TenantState};
use crate::types::{
    FieldDef, FieldId, FieldType, LedgerAccount, LedgerEntry, LedgerOwner, LedgerUnit,
    ObjectTypeId, RecordId, Timestamp, Value,
};
use std::collections::BTreeMap;

/// The name of the record type a ledger account and entry stand for in a condition.
const RECORD_TYPE: &str = "ledger";

/// The fields a condition of a ledger policy may read, for the check of the schema.
pub fn fields() -> BTreeMap<FieldId, FieldDef> {
    {
        let field = |name: &str, ty: FieldType| {
            let id = FieldId::new(name).expect("a field name of the ledger is a valid id");
            (
                id.clone(),
                FieldDef {
                    id,
                    label: name.to_string(),
                    ty,
                    required: false,
                    unique: false,
                    default: None,
                    read: None,
                    personal: None,
                    normalise: None,
                    computed: None,
                },
            )
        };
        BTreeMap::from([
            field("owner", FieldType::Str),
            field("pool", FieldType::Str),
            field("unit", FieldType::Str),
            field("scope", FieldType::Str),
            field("amount", FieldType::Int),
            field("reason", FieldType::Str),
            field("caused_by", FieldType::Str),
            field("at", FieldType::Time),
        ])
    }
}

/// The problems of a policy over the ledger: it permits `read` and nothing else, and the unit it
/// names is declared. Called by the check of a schema for every policy.
pub fn policy_problems(
    schema: &TenantSchema,
    policy: &PolicyDef,
    at: &str,
    problems: &mut Vec<String>,
) {
    let Resource::Ledger { unit } = &policy.resource else {
        return;
    };
    for action in &policy.actions {
        if *action != Action::Read {
            problems.push(format!(
                "{at}: action {action:?} does not fit the ledger, which policies only let be read"
            ));
        }
    }
    if let Some(unit) = unit {
        let declared = schema
            .ledger
            .as_ref()
            .is_some_and(|ledger| ledger.units.contains_key(unit));
        if !declared {
            problems.push(format!("{at}: unknown ledger unit {unit}"));
        }
    }
}

/// Whether the policies of the roles of `principal` bound what it reads of the ledger: some
/// policy over the ledger that permits `read` names a role it holds.
pub fn bounded(tenant: &TenantState, principal: &Principal) -> bool {
    let roles = roles_of(tenant, principal);
    tenant.schema.policies.values().any(|policy| {
        matches!(policy.resource, Resource::Ledger { .. })
            && policy.actions.contains(&Action::Read)
            && roles.contains(&policy.role)
    })
}

fn text_or_null(text: Option<&str>) -> Value {
    text.map_or(Value::Null, |t| Value::Str(t.to_string()))
}

/// The record a condition reads: the account, and the entry when there is one.
fn record_of(
    owner: Option<&LedgerOwner>,
    unit: &LedgerUnit,
    scope: &str,
    entry: Option<&LedgerEntry>,
) -> Record {
    let field = |name: &str| FieldId::new(name).expect("a field name of the ledger is a valid id");
    let (principal, pool) = match owner {
        Some(LedgerOwner::Principal { principal }) => (Some(principal.as_str()), None),
        Some(LedgerOwner::Pool { pool }) => (None, Some(pool.as_str())),
        None => (None, None),
    };
    let mut fields = BTreeMap::from([
        (field("owner"), text_or_null(principal)),
        (field("pool"), text_or_null(pool)),
        (field("unit"), Value::Str(unit.as_str().to_string())),
        (field("scope"), Value::Str(scope.to_string())),
    ]);
    if let Some(entry) = entry {
        fields.insert(field("amount"), Value::Int(entry.amount.0));
        fields.insert(field("reason"), Value::Str(entry.reason.clone()));
        fields.insert(
            field("caused_by"),
            text_or_null(entry.caused_by.as_ref().map(|p| p.as_str())),
        );
        fields.insert(field("at"), Value::Time(entry.at.0));
    }
    Record {
        id: RecordId(0),
        object: ObjectTypeId::new(RECORD_TYPE).expect("the ledger record type is a valid id"),
        version: 0,
        created_by: None,
        created_at: Timestamp(0),
        updated_at: Timestamp(0),
        fields,
    }
}

/// Whether `principal` owns the account of `owner`.
pub fn owns(principal: &Principal, owner: Option<&LedgerOwner>) -> bool {
    matches!(
        (principal, owner),
        (Principal::User(id), Some(LedgerOwner::Principal { principal: owned })) if id == owned
    )
}

/// Whether the policies permit `read` of the record, for the unit and for every unit.
fn permitted(
    tenant: &TenantState,
    principal: &Principal,
    now: Timestamp,
    unit: &LedgerUnit,
    record: &Record,
) -> bool {
    [Some(unit.clone()), None].into_iter().any(|named| {
        authorize(
            tenant,
            &tenant.schema,
            principal,
            now,
            Action::Read,
            &Resource::Ledger { unit: named },
            Subject::stored(record),
            None,
        )
        .0
    })
}

/// Whether `principal` may read the account (its balance and its place in a ranking) under the
/// policies of its roles. An account with no owner (its owner was erased) is not read through a
/// policy, since there is no owner to show.
pub fn account_permitted(
    tenant: &TenantState,
    principal: &Principal,
    now: Timestamp,
    owner: Option<&LedgerOwner>,
    unit: &LedgerUnit,
    scope: &str,
) -> bool {
    if owner.is_none() {
        return false;
    }
    owns(principal, owner)
        || permitted(
            tenant,
            principal,
            now,
            unit,
            &record_of(owner, unit, scope, None),
        )
}

/// Whether `principal` may read one entry of the account under the policies of its roles.
pub fn entry_permitted(
    tenant: &TenantState,
    principal: &Principal,
    now: Timestamp,
    account: &LedgerAccount,
    entry: &LedgerEntry,
) -> bool {
    let owner = account.owner.as_ref();
    owns(principal, owner)
        || permitted(
            tenant,
            principal,
            now,
            &account.unit,
            &record_of(owner, &account.unit, account.scope.as_str(), Some(entry)),
        )
}

/// Whether `principal` may read any entry of an account of `owner`, `unit` and `scope` that has
/// `entry`: the same as [`entry_permitted`] for an account that is named by its parts and not by
/// its header, as a read of entries is.
pub fn entry_permitted_for(
    tenant: &TenantState,
    principal: &Principal,
    now: Timestamp,
    owner: &LedgerOwner,
    unit: &LedgerUnit,
    scope: &str,
    entry: &LedgerEntry,
) -> bool {
    owns(principal, Some(owner))
        || permitted(
            tenant,
            principal,
            now,
            unit,
            &record_of(Some(owner), unit, scope, Some(entry)),
        )
}
