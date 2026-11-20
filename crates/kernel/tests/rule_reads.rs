//! The reads of the tenant rule language that look past the record (`strata_kernel::rule_reads`,
//! D-6544): the balance and the setting of the acting principal over the snapshot of a decision,
//! and the need a balance read asks for when the snapshot does not cover the account.

use std::collections::BTreeSet;
use strata_kernel::rule_reads;
use strata_kernel::rules::Lookup;
use strata_kernel::testkit::*;
use strata_kernel::*;

const T0: i64 = 1_763_640_000_000;

fn points() -> LedgerUnit {
    LedgerUnit::new("points").unwrap()
}

fn step(world: &World, who: &str, now: i64, intent: Intent) -> World {
    world
        .step(&intent, &ctx("alpha", user(&format!("alpha:{who}")), now))
        .unwrap_or_else(|e| panic!("{}", e.describe()))
        .0
}

fn change(world: &World, changes: Vec<SchemaChange>) -> World {
    step(
        world,
        "owner",
        T0 - 1_000,
        Intent::ChangeSchema {
            changes,
            expected_version: None,
            existing: ExistingData::Reject,
        },
    )
}

/// The notes world with the unit `points` declared and, when `amount` is given, that many points
/// posted by the owner to the account of `alpha:writer1` in the scope `promo`.
fn ledger_world(amount: Option<i64>) -> World {
    let world = change(
        &notes_world(),
        vec![SchemaChange::DeclareLedgerUnit(LedgerUnitDecl {
            unit: points(),
            description: "Points".into(),
            expiry: None,
        })],
    );
    match amount {
        None => world,
        Some(amount) => step(
            &world,
            "owner",
            T0,
            Intent::PostLedger {
                postings: vec![LedgerPostingRequest {
                    owner: LedgerOwner::Principal {
                        principal: principal_id("alpha:writer1"),
                    },
                    unit: points(),
                    amount: LedgerAmount(amount),
                    idempotency_key: "grant".into(),
                    reason: "test".into(),
                    corrects: None,
                    scope: Some(LedgerScope::new("promo").unwrap()),
                }],
            },
        ),
    }
}

fn complete(world: &World) -> (std::sync::Arc<TenantState>, Coverage) {
    (
        world.tenants[&tenant_id("alpha")].clone(),
        Coverage::Complete,
    )
}

#[test]
fn balance_reads_the_acting_persons_account_of_the_unit_and_scope() {
    let world = ledger_world(Some(7));
    let (tenant, coverage) = complete(&world);
    let lookup = Lookup {
        tenant: &tenant,
        coverage: &coverage,
    };
    let writer = user("alpha:writer1");
    assert_eq!(
        rule_reads::balance(&lookup, &writer, "points", "promo").unwrap(),
        Value::Int(7)
    );
    // Another scope of the same unit is another account, never posted to: 0, not an error.
    assert_eq!(
        rule_reads::balance(&lookup, &writer, "points", "").unwrap(),
        Value::Int(0)
    );
    // Another person reads their own account, not writer1's.
    assert_eq!(
        rule_reads::balance(&lookup, &user("alpha:writer2"), "points", "promo").unwrap(),
        Value::Int(0)
    );
}

#[test]
fn balance_is_null_for_an_anonymous_or_system_principal_and_an_error_for_an_unknown_unit() {
    let world = ledger_world(Some(7));
    let (tenant, coverage) = complete(&world);
    let lookup = Lookup {
        tenant: &tenant,
        coverage: &coverage,
    };
    for principal in [Principal::Anonymous, Principal::System] {
        assert_eq!(
            rule_reads::balance(&lookup, &principal, "points", "promo").unwrap(),
            Value::Null
        );
        assert_eq!(
            rule_reads::balance_need(&tenant.id, &principal, "points", "promo"),
            None
        );
    }
    let error = rule_reads::balance(&lookup, &user("alpha:writer1"), "stars", "").unwrap_err();
    assert!(error.message.contains("no ledger unit stars"), "{error:?}");
}

/// A read of an account the snapshot does not hold asks for it, as a hop into a record that was
/// not loaded does, and reads it once the need is loaded.
#[test]
fn balance_over_a_snapshot_without_the_account_asks_for_it() {
    let world = ledger_world(Some(7));
    let tenant = world.tenants[&tenant_id("alpha")].clone();
    let writer = user("alpha:writer1");
    let need = rule_reads::balance_need(&tenant.id, &writer, "points", "promo").unwrap();
    let account = ledger::account_id(
        &tenant.id,
        &LedgerOwner::Principal {
            principal: principal_id("alpha:writer1"),
        },
        &points(),
        "promo",
    );
    assert_eq!(need, Need::Ledger(LedgerNeed::Account { account }));

    let without = Coverage::Partial(BTreeSet::new());
    let lookup = Lookup {
        tenant: &tenant,
        coverage: &without,
    };
    let error = rule_reads::balance(&lookup, &writer, "points", "promo").unwrap_err();
    assert_eq!(error.kind, ErrorKind::InsufficientState);
    assert_eq!(error.missing, Some(need.clone()));

    let with = Coverage::Partial(BTreeSet::from([need]));
    let lookup = Lookup {
        tenant: &tenant,
        coverage: &with,
    };
    assert_eq!(
        rule_reads::balance(&lookup, &writer, "points", "promo").unwrap(),
        Value::Int(7)
    );
}

#[test]
fn setting_reads_the_value_resolved_for_the_acting_principal() {
    let world = change(
        &notes_world(),
        vec![SchemaChange::DefineSetting(SettingDef {
            id: SettingId::new("page_size").unwrap(),
            label: "page_size".into(),
            r#type: SettingType::new("i64").unwrap(),
            default: Value::Int(20),
            levels: BTreeSet::from([SettingLevel::Tenant, SettingLevel::Person]),
            locked_by: None,
        })],
    );
    let world = step(
        &world,
        "owner",
        T0,
        Intent::WriteSetting {
            setting: SettingId::new("page_size").unwrap(),
            level: SettingLevel::Person,
            team: None,
            value: Some(Value::Int(50)),
            expected_revision: 0,
        },
    );
    let (tenant, coverage) = complete(&world);
    let lookup = Lookup {
        tenant: &tenant,
        coverage: &coverage,
    };
    // The owner's own override, then the default for anyone without one.
    assert_eq!(
        rule_reads::setting(&lookup, &user("alpha:owner"), "page_size").unwrap(),
        Value::Int(50)
    );
    assert_eq!(
        rule_reads::setting(&lookup, &Principal::Anonymous, "page_size").unwrap(),
        Value::Int(20)
    );
    assert!(rule_reads::setting(&lookup, &user("alpha:owner"), "nothing").is_err());
}
