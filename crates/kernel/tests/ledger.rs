//! The tenant ledger as the kernel decides it (`strata_kernel::ledger`): declarations, postings
//! at every limit's boundary, idempotent retries, concurrent postings under the revision guard,
//! transfers, reads of one's own and of others' accounts, and the balance as the sum of the
//! entries over random postings.

use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use std::sync::Arc;
use strata_kernel::ledger;
use strata_kernel::testkit::*;
use strata_kernel::*;

const T0: i64 = 1_763_640_000_000; // 2025-11-20T12:00:00Z, a Thursday

fn points() -> LedgerUnit {
    LedgerUnit::new("points").unwrap()
}

fn budget() -> LedgerPoolName {
    LedgerPoolName::new("budget").unwrap()
}

fn person(name: &str) -> LedgerOwner {
    LedgerOwner::Principal {
        principal: principal_id(&format!("alpha:{name}")),
    }
}

fn pool() -> LedgerOwner {
    LedgerOwner::Pool { pool: budget() }
}

fn unit_limit(
    floor: Option<i64>,
    cap: Option<i64>,
    period: Option<(u64, LedgerPeriod)>,
) -> LedgerLimit {
    LedgerLimit {
        unit: points(),
        owner: None,
        floor: floor.map(LedgerAmount),
        cap: cap.map(LedgerAmount),
        period_cap: period.map(|p| p.0),
        period: period.map(|p| p.1),
        scope: None,
        additions_only: None,
    }
}

fn change_as(
    world: &World,
    who: &str,
    now: i64,
    changes: Vec<SchemaChange>,
) -> Result<World, DomainError> {
    let intent = Intent::ChangeSchema {
        changes,
        expected_version: None,
        existing: ExistingData::Reject,
    };
    world
        .step(&intent, &ctx("alpha", user(&format!("alpha:{who}")), now))
        .map(|(next, _)| next)
}

/// The notes world with the unit `points` and the pool `budget` declared in alpha, and the
/// given limits set.
fn ledger_world(limits: Vec<LedgerLimit>) -> World {
    let mut changes = vec![
        SchemaChange::DeclareLedgerUnit(LedgerUnitDecl {
            unit: points(),
            description: "Points".into(),
            expiry: None,
        }),
        SchemaChange::DeclareLedgerPool(LedgerPoolDecl {
            pool: budget(),
            description: "Promotion budget".into(),
        }),
    ];
    changes.extend(limits.into_iter().map(SchemaChange::SetLedgerLimit));
    change_as(&notes_world(), "owner", T0 - 1_000, changes).expect("the declarations are accepted")
}

fn request(owner: LedgerOwner, amount: i64, key: &str) -> LedgerPostingRequest {
    LedgerPostingRequest {
        owner,
        unit: points(),
        amount: LedgerAmount(amount),
        idempotency_key: key.into(),
        reason: "test".into(),
        corrects: None,
        scope: None,
    }
}

fn post_as(
    world: &World,
    who: &str,
    now: i64,
    postings: Vec<LedgerPostingRequest>,
) -> Result<(World, Transition), DomainError> {
    world.step(
        &Intent::PostLedger { postings },
        &ctx("alpha", user(&format!("alpha:{who}")), now),
    )
}

/// Post as the owner; the posting must be accepted.
fn posted(world: &World, now: i64, owner: LedgerOwner, amount: i64, key: &str) -> World {
    post_as(world, "owner", now, vec![request(owner, amount, key)])
        .unwrap_or_else(|e| panic!("posting {amount} under {key} was refused: {}", e.describe()))
        .0
}

/// Post as the owner; the posting must be refused, and the world must not change.
fn refused(world: &World, now: i64, owner: LedgerOwner, amount: i64, key: &str) -> DomainError {
    post_as(world, "owner", now, vec![request(owner, amount, key)])
        .err()
        .unwrap_or_else(|| panic!("posting {amount} under {key} was accepted"))
}

fn failed_rule(error: &DomainError) -> &str {
    error
        .evidence
        .iter()
        .find(|c| !c.passed)
        .map_or("", |c| c.rule.as_str())
}

fn account_of(world: &World, owner: &LedgerOwner, scope: &str) -> Option<LedgerAccount> {
    let tenant = &world.tenants[&tenant_id("alpha")];
    let id = ledger::account_id(&tenant.id, owner, &points(), scope);
    tenant
        .ledger
        .as_ref()
        .and_then(|l| l.accounts.get(&ledger::account_key(&id)).cloned())
}

fn balance(world: &World, owner: &LedgerOwner) -> i64 {
    account_of(world, owner, "").map_or(0, |a| a.balance.0)
}

fn entries(world: &World, owner: &LedgerOwner, scope: &str) -> Vec<LedgerEntry> {
    let tenant = &world.tenants[&tenant_id("alpha")];
    let id = ledger::account_id(&tenant.id, owner, &points(), scope);
    tenant
        .ledger
        .as_ref()
        .and_then(|l| l.entries.get(&ledger::account_key(&id)).cloned())
        .unwrap_or_default()
}

// ------------------------------------------------------------------------------------------------
// Limits at their boundaries

/// Verifies: LDG-013, LDG-016
#[test]
fn a_balance_outside_a_lowered_limit_may_only_move_back_toward_it() {
    let world = ledger_world(vec![unit_limit(Some(0), Some(100), None)]);
    let world = posted(&world, T0, pool(), 100, "fill");
    // Lowering the cap below the balance rewrites nothing (LDG-016).
    let world = change_as(
        &world,
        "owner",
        T0 + 1,
        vec![SchemaChange::SetLedgerLimit(unit_limit(
            Some(0),
            Some(50),
            None,
        ))],
    )
    .unwrap();
    assert_eq!(balance(&world, &pool()), 100);
    assert_eq!(entries(&world, &pool(), "").len(), 1);
    assert_eq!(
        failed_rule(&refused(&world, T0 + 2, pool(), 1, "more")),
        "ledger:above-cap"
    );
    // A debit that leaves the balance above the cap still moves it toward the cap.
    let world = posted(&world, T0 + 3, pool(), -10, "less");
    assert_eq!(balance(&world, &pool()), 90);

    // The same for a floor raised above a balance.
    let world = change_as(
        &world,
        "owner",
        T0 + 4,
        vec![SchemaChange::SetLedgerLimit(unit_limit(
            Some(95),
            Some(500),
            None,
        ))],
    )
    .unwrap();
    let world = posted(&world, T0 + 5, pool(), 1, "toward");
    assert_eq!(
        failed_rule(&refused(&world, T0 + 6, pool(), -1, "away")),
        "ledger:below-floor"
    );
}

/// Verifies: LDG-014, LDG-015
#[test]
fn a_cap_per_day_counts_the_credits_of_the_utc_day_and_starts_again_the_next() {
    let world = ledger_world(vec![unit_limit(None, None, Some((10, LedgerPeriod::Day)))]);
    let world = posted(&world, T0, person("writer1"), 6, "a");
    let world = posted(&world, T0 + 1, person("writer1"), 4, "b");
    let error = refused(&world, T0 + 2, person("writer1"), 1, "c");
    assert_eq!(failed_rule(&error), "ledger:above-period-cap");
    // A debit is not a credit: it neither counts nor is refused by the cap per period.
    let world = posted(&world, T0 + 3, person("writer1"), -5, "d");
    assert_eq!(
        failed_rule(&refused(&world, T0 + 4, person("writer1"), 1, "e")),
        "ledger:above-period-cap"
    );
    // Midnight UTC starts the next day.
    let midnight = ledger::period_start(LedgerPeriod::Day, Timestamp(T0)).0 + 86_400_000;
    let world = posted(&world, midnight, person("writer1"), 10, "f");
    assert_eq!(balance(&world, &person("writer1")), 15);
    assert_eq!(
        failed_rule(&refused(&world, midnight + 1, person("writer1"), 1, "g")),
        "ledger:above-period-cap"
    );
}

/// Verifies: LDG-012, LDG-1542
#[test]
fn an_account_limit_replaces_the_unit_limit_limit_by_limit_and_each_scope_counts_alone() {
    let account_cap = LedgerLimit {
        owner: Some(person("writer1")),
        additions_only: None,
        ..unit_limit(None, Some(200), None)
    };
    let world = ledger_world(vec![unit_limit(Some(0), Some(100), None), account_cap]);
    // The account's cap is 200; its floor is still the unit's 0.
    let world = posted(&world, T0, person("writer1"), 200, "a");
    assert_eq!(
        failed_rule(&refused(&world, T0 + 1, person("writer1"), 1, "b")),
        "ledger:above-cap"
    );
    assert_eq!(
        failed_rule(&refused(&world, T0 + 2, person("writer1"), -201, "c")),
        "ledger:below-floor"
    );
    // Another person has the unit's cap of 100.
    assert_eq!(
        failed_rule(&refused(&world, T0 + 3, person("writer2"), 101, "d")),
        "ledger:above-cap"
    );
    // The unit's cap applies to each scope separately.
    let scoped = |scope: &str, key: &str| LedgerPostingRequest {
        scope: Some(LedgerScope::new(scope).unwrap()),
        ..request(pool(), 100, key)
    };
    let (world, _) = post_as(&world, "owner", T0 + 4, vec![scoped("42", "s42")]).unwrap();
    let (world, _) = post_as(&world, "owner", T0 + 5, vec![scoped("43", "s43")]).unwrap();
    assert_eq!(
        account_of(&world, &pool(), "42").unwrap().balance,
        LedgerAmount(100)
    );
    assert_eq!(
        account_of(&world, &pool(), "43").unwrap().balance,
        LedgerAmount(100)
    );
    assert!(post_as(&world, "owner", T0 + 6, vec![scoped("42", "again")]).is_err());
}

// ------------------------------------------------------------------------------------------------
// Postings

/// Verifies: LDG-002, LDG-003, LDG-004
#[test]
fn a_posting_is_refused_for_what_it_names_who_sends_it_or_its_size() {
    let world = ledger_world(vec![]);
    let mut stamps = request(person("writer1"), 1, "u");
    stamps.unit = LedgerUnit::new("stamps").unwrap();
    let error = post_as(&world, "owner", T0, vec![stamps]).unwrap_err();
    assert_eq!(
        (error.kind, failed_rule(&error)),
        (ErrorKind::UnknownRecord, "ledger:unknown-unit")
    );
    for owner in [
        person("stranger"),
        LedgerOwner::Pool {
            pool: LedgerPoolName::new("nothing").unwrap(),
        },
    ] {
        let error = refused(&world, T0, owner, 1, "o");
        assert_eq!(
            (error.kind, failed_rule(&error)),
            (ErrorKind::UnknownRecord, "ledger:unknown-owner")
        );
    }
    for amount in [0, 9_007_199_254_740_992, -9_007_199_254_740_992] {
        let error = refused(&world, T0, person("writer1"), amount, "z");
        assert_eq!(failed_rule(&error), "ledger:invalid-amount", "{amount}");
    }
    // The largest amount is accepted; one more would leave the range of a balance.
    let world = posted(&world, T0, person("writer1"), 9_007_199_254_740_991, "max");
    assert_eq!(
        failed_rule(&refused(&world, T0 + 1, person("writer1"), 1, "over")),
        "ledger:invalid-amount"
    );

    // Only a holder of ledger-post may post; a batch names one to eight postings.
    let world = ledger_world(vec![]);
    let error = post_as(
        &world,
        "writer1",
        T0,
        vec![request(person("writer1"), 1, "self")],
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Unauthorized);
    assert_eq!(failed_rule(&error), "permission:capability:ledger-post");
    // A role granted the capability may post.
    let world = change_as(
        &world,
        "owner",
        T0,
        vec![SchemaChange::DefineRolePermissions {
            role: role("editor"),
            permissions: vec![Permission::Capability(ledger::LEDGER_POST.into())],
        }],
    )
    .unwrap();
    assert!(post_as(
        &world,
        "editor",
        T0 + 1,
        vec![request(person("writer1"), 1, "e")]
    )
    .is_ok());
    assert_eq!(
        post_as(&world, "owner", T0 + 2, vec![]).unwrap_err().kind,
        ErrorKind::InvalidIntent
    );
    let nine = (0..9)
        .map(|i| request(person("writer1"), 1, &format!("n{i}")))
        .collect();
    assert_eq!(
        post_as(&world, "owner", T0 + 3, nine).unwrap_err().kind,
        ErrorKind::InvalidIntent
    );
}

/// Verifies: LDG-023
#[test]
fn a_correction_names_an_entry_its_account_holds() {
    let world = ledger_world(vec![]);
    let world = posted(&world, T0, person("writer1"), 50, "wrong");
    let mut fix = request(person("writer1"), -45, "fix");
    fix.corrects = Some(1);
    let (world, _) = post_as(&world, "owner", T0 + 1, vec![fix]).unwrap();
    assert_eq!(entries(&world, &person("writer1"), "")[1].corrects, Some(1));
    for bad in [0, 3] {
        let mut wrong = request(person("writer1"), 1, &format!("bad-{bad}"));
        wrong.corrects = Some(bad);
        let error = post_as(&world, "owner", T0 + 2, vec![wrong]).unwrap_err();
        assert_eq!(failed_rule(&error), "ledger:bad-correction");
    }
}

/// Verifies: LDG-005, LDG-008, LDG-010, LDG-019
#[test]
fn an_entry_records_who_and_when_and_each_posting_has_one_event() {
    let world = ledger_world(vec![]);
    let (world, done) = post_as(
        &world,
        "owner",
        T0,
        vec![
            request(person("writer1"), 5, "a"),
            request(person("writer1"), 3, "b"),
        ],
    )
    .unwrap();
    let recorded = entries(&world, &person("writer1"), "");
    assert_eq!(
        recorded.iter().map(|e| e.sequence).collect::<Vec<_>>(),
        [1, 2]
    );
    assert!(recorded
        .iter()
        .all(|e| e.caused_by == Some(principal_id("alpha:owner")) && e.at == Timestamp(T0)));
    let posted_events: Vec<_> = done
        .events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::LedgerEntryPosted {
                balance_before,
                balance,
                entry_sequence,
                ..
            } => Some((balance_before.0, balance.0, *entry_sequence)),
            _ => None,
        })
        .collect();
    assert_eq!(posted_events, [(0, 5, 1), (5, 8, 2)]);
    // The same snapshot, request and context decide the same way (LDG-008).
    let world2 = ledger_world(vec![]);
    let (_, again) = post_as(
        &world2,
        "owner",
        T0,
        vec![
            request(person("writer1"), 5, "a"),
            request(person("writer1"), 3, "b"),
        ],
    )
    .unwrap();
    assert_eq!(done, again);
}

/// Verifies: LDG-009
#[test]
fn a_transfer_is_two_postings_committed_together_or_not_at_all() {
    let cap = LedgerLimit {
        owner: Some(pool()),
        additions_only: None,
        ..unit_limit(None, Some(40), None)
    };
    let world = ledger_world(vec![unit_limit(Some(0), None, None), cap]);
    let world = posted(&world, T0, person("writer1"), 100, "earn");
    let transfer = |amount: i64, key: &str| {
        vec![
            request(person("writer1"), -amount, key),
            request(pool(), amount, key),
        ]
    };
    let (world, _) = post_as(&world, "owner", T0 + 1, transfer(30, "t1")).unwrap();
    assert_eq!(balance(&world, &person("writer1")), 70);
    assert_eq!(balance(&world, &pool()), 30);
    // The second leg would pass the pool's cap: neither leg is applied.
    let error = post_as(&world, "owner", T0 + 2, transfer(20, "t2")).unwrap_err();
    assert_eq!(failed_rule(&error), "ledger:above-cap");
    assert_eq!(balance(&world, &person("writer1")), 70);
    assert_eq!(entries(&world, &person("writer1"), "").len(), 2);
}

// ------------------------------------------------------------------------------------------------
// Concurrency (D-5009)

/// Verifies: LDG-017, LDG-018
///
/// Two postings decided against one snapshot: the commit of the first moves the revision, so the
/// delta of the second no longer applies (the store's revision guard is `apply_delta`'s base
/// revision check, which every adapter enforces); decided again on the new state, it is refused
/// when it no longer fits and applied when it still does.
#[test]
fn two_concurrent_postings_cannot_both_pass_a_cap() {
    let world = ledger_world(vec![unit_limit(None, Some(100), None)]);
    let world = posted(&world, T0, pool(), 60, "start");
    let alpha = tenant_id("alpha");
    let snapshot = world.state(&alpha);
    let decide = |state: &State, amount: i64, key: &str, now: i64| {
        transition(
            state,
            &Intent::PostLedger {
                postings: vec![request(pool(), amount, key)],
            },
            &ctx("alpha", user("alpha:owner"), now),
        )
    };
    // Each fits alone on the snapshot.
    let first = decide(&snapshot, 30, "a", T0 + 1).unwrap();
    let second = decide(&snapshot, 30, "b", T0 + 2).unwrap();
    let after_first = apply_delta(Some(&world.tenants[&alpha]), &first.delta).unwrap();
    let lost = apply_delta(Some(&after_first), &second.delta).unwrap_err();
    assert_eq!(
        lost.kind,
        ErrorKind::VersionConflict,
        "the loser does not commit"
    );
    let mut tenants = world.tenants.clone();
    tenants.insert(alpha.clone(), Arc::new(after_first));
    let world = World { tenants };
    // Decided again on the winner's state, it no longer fits.
    let again = decide(&world.state(&alpha), 30, "b", T0 + 3).unwrap_err();
    assert_eq!(failed_rule(&again), "ledger:above-cap");
    assert_eq!(balance(&world, &pool()), 90);
    // A loser that still fits after the winner is applied (LDG-018).
    let fits = decide(&world.state(&alpha), 10, "b", T0 + 3).unwrap();
    let applied = apply_delta(Some(&world.tenants[&alpha]), &fits.delta).unwrap();
    assert_eq!(
        applied.ledger.as_ref().unwrap().accounts
            [&ledger::account_key(&ledger::account_id(&alpha, &pool(), &points(), ""))]
            .balance,
        LedgerAmount(100)
    );
}

// ------------------------------------------------------------------------------------------------
// Needs and partial snapshots (K-100)

// ------------------------------------------------------------------------------------------------
// The balance is the sum of the entries (LDG-019, LDG-020)

/// Verifies: LDG-019, LDG-020, LDG-022
///
/// Random postings, with repeated keys, refusals at the floor and the cap, transfers and three
/// accounts, from a fixed seed: after each step every balance equals the sum of its account's
/// entries, numbered 1, 2, 3 without a gap, and no entry already recorded ever changes.
#[test]
fn the_balance_always_equals_the_sum_of_the_entries() {
    let owners = [person("writer1"), person("writer2"), pool()];
    let step = (0usize..3, -60i64..60, 0u8..12, any::<bool>());
    let mut runner = TestRunner::deterministic();
    for _ in 0..16 {
        let steps = proptest::collection::vec(step.clone(), 1..40)
            .new_tree(&mut runner)
            .unwrap()
            .current();
        let mut world = ledger_world(vec![unit_limit(Some(-50), Some(150), None)]);
        let mut seen: Vec<Vec<LedgerEntry>> = vec![Vec::new(); owners.len()];
        for (n, (who, amount, key, transfer)) in steps.into_iter().enumerate() {
            let amount = if amount == 0 { 1 } else { amount };
            let key = format!("k{key}");
            let mut postings = vec![request(owners[who].clone(), amount, &key)];
            if transfer {
                postings.push(request(owners[(who + 1) % 3].clone(), -amount, &key));
            }
            if let Ok((next, _)) = post_as(&world, "owner", T0 + n as i64, postings) {
                world = next;
            }
            for (i, owner) in owners.iter().enumerate() {
                let journal = entries(&world, owner, "");
                assert_eq!(
                    ledger::replayed_balance(&journal),
                    Ok(balance(&world, owner)),
                    "the balance of {owner:?} is the sum of its entries"
                );
                assert!(
                    journal.starts_with(&seen[i]),
                    "an entry was changed or removed"
                );
                if let Some(header) = account_of(&world, owner, "") {
                    assert_eq!(header.last_sequence, journal.len() as u64);
                    assert!((-50..=150).contains(&header.balance.0));
                }
                seen[i] = journal;
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Reads

/// Verifies: LDG-001, LDG-026, LDG-1541
#[test]
fn reads_stay_within_the_member_and_the_tenant() {
    let world = ledger_world(vec![]);
    let world = posted(&world, T0, person("writer1"), 7, "a");
    let world = posted(&world, T0 + 1, person("writer2"), 9, "b");
    let alpha = tenant_id("alpha");
    let state = world.state(&alpha);
    let as_writer = ctx("alpha", user("alpha:writer1"), T0 + 2);
    let own = ledger::own_owner(&as_writer).unwrap();
    assert_eq!(own, person("writer1"));
    let mine = ledger::read_balance(&state, &own, &points(), "", true, &as_writer).unwrap();
    assert_eq!(mine.balance, LedgerAmount(7));
    let (rows, next) =
        ledger::read_entries(&state, &own, &points(), "", None, None, true, &as_writer).unwrap();
    assert_eq!((rows.len(), next), (1, None));
    // Another person's account, or a pool's, is refused.
    for other in [person("writer2"), pool()] {
        let error =
            ledger::read_balance(&state, &other, &points(), "", false, &as_writer).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Unauthorized);
        let error =
            ledger::read_entries(&state, &other, &points(), "", None, None, false, &as_writer)
                .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Unauthorized);
    }
    assert_eq!(
        ledger::read_ranked(
            &state,
            &points(),
            "",
            LedgerOrder::GreatestFirst,
            0,
            None,
            &as_writer
        )
        .unwrap_err()
        .kind,
        ErrorKind::Unauthorized
    );
    assert_eq!(
        ledger::read_definitions(&state, &as_writer)
            .unwrap_err()
            .kind,
        ErrorKind::Unauthorized
    );
    // An account never posted to reads as 0 for its owner.
    let as_other = ctx("alpha", user("alpha:editor"), T0 + 2);
    let none =
        ledger::read_balance(&state, &person("editor"), &points(), "", true, &as_other).unwrap();
    assert_eq!((none.balance, none.last_sequence), (LedgerAmount(0), 0));
    // Anonymous readers have no own account.
    assert!(ledger::own_owner(&ctx("alpha", Principal::Anonymous, T0)).is_err());
    // The owner of the tenant reads every account.
    let as_owner = ctx("alpha", user("alpha:owner"), T0 + 2);
    let theirs =
        ledger::read_balance(&state, &person("writer2"), &points(), "", false, &as_owner).unwrap();
    assert_eq!(theirs.balance, LedgerAmount(9));
    assert_eq!(
        ledger::read_balance(&state, &person("editor"), &points(), "", false, &as_owner)
            .unwrap_err()
            .kind,
        ErrorKind::UnknownRecord,
        "an account that does not exist is not found"
    );

    // Accounts of two tenants never meet.
    let world = ledger_world(vec![]);
    let world = posted(&world, T0, person("writer1"), 5, "a");
    let alpha = tenant_id("alpha");
    let beta = tenant_id("beta");
    assert_ne!(
        ledger::account_id(&alpha, &person("writer1"), &points(), ""),
        ledger::account_id(&beta, &person("writer1"), &points(), "")
    );
    // Beta declares no unit, and holds no ledger.
    assert!(world.tenants[&beta].ledger.is_none());
    let as_beta = ctx("beta", user("beta:owner"), T0 + 1);
    assert_eq!(
        ledger::read_balance(
            &world.state(&beta),
            &person("writer1"),
            &points(),
            "",
            false,
            &as_beta
        )
        .unwrap_err()
        .kind,
        ErrorKind::UnknownRecord
    );
    // A snapshot of alpha read in beta's context is refused.
    assert_eq!(
        ledger::read_balance(
            &world.state(&alpha),
            &person("writer1"),
            &points(),
            "",
            false,
            &as_beta
        )
        .unwrap_err()
        .kind,
        ErrorKind::TenantMismatch
    );
    // A posting in beta's context names beta's principals only.
    let error = world
        .step(
            &Intent::PostLedger {
                postings: vec![request(person("writer1"), 1, "x")],
            },
            &ctx("beta", user("beta:owner"), T0 + 2),
        )
        .unwrap_err();
    assert_eq!(failed_rule(&error), "ledger:unknown-unit");
}

/// Verifies: LDG-024, LDG-1543
#[test]
fn reads_are_ordered_and_bounded() {
    let mut world = ledger_world(vec![]);
    for n in 0..5 {
        world = posted(&world, T0 + n, person("writer1"), n + 1, &format!("k{n}"));
    }
    let state = world.state(&tenant_id("alpha"));
    let as_owner = ctx("alpha", user("alpha:owner"), T0 + 10);
    let read = |after, limit| {
        ledger::read_entries(
            &state,
            &person("writer1"),
            &points(),
            "",
            after,
            limit,
            false,
            &as_owner,
        )
        .unwrap()
    };
    let (first, next) = read(None, Some(2));
    assert_eq!(first.iter().map(|e| e.sequence).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(next, Some(2));
    let (rest, next) = read(next, Some(10));
    assert_eq!(
        rest.iter().map(|e| e.sequence).collect::<Vec<_>>(),
        [3, 4, 5]
    );
    assert_eq!(next, None);
    assert_eq!(
        ledger::read_entries(
            &state,
            &person("writer1"),
            &points(),
            "",
            None,
            Some(101),
            false,
            &as_owner
        )
        .unwrap_err()
        .kind,
        ErrorKind::InvalidIntent
    );

    let world = ledger_world(vec![]);
    let world = posted(&world, T0, person("writer2"), 50, "a");
    let world = posted(&world, T0 + 1, person("writer1"), 50, "b");
    let world = posted(&world, T0 + 2, person("editor"), 10, "c");
    let world = posted(&world, T0 + 3, pool(), 70, "d");
    let state = world.state(&tenant_id("alpha"));
    let as_owner = ctx("alpha", user("alpha:owner"), T0 + 10);
    let (rows, next) = ledger::read_ranked(
        &state,
        &points(),
        "",
        LedgerOrder::GreatestFirst,
        0,
        Some(3),
        &as_owner,
    )
    .unwrap();
    let order: Vec<(LedgerOwner, i64)> = rows
        .iter()
        .map(|r| (r.owner.clone(), r.balance.0))
        .collect();
    assert_eq!(
        order,
        [
            (pool(), 70),
            (person("writer1"), 50),
            (person("writer2"), 50)
        ]
    );
    assert_eq!(next, Some(3));
    let (rows, next) = ledger::read_ranked(
        &state,
        &points(),
        "",
        LedgerOrder::LeastFirst,
        0,
        None,
        &as_owner,
    )
    .unwrap();
    assert_eq!(rows[0].owner, person("editor"));
    assert_eq!(next, None);
    let error = ledger::ranked_needs(&points(), "", LedgerOrder::GreatestFirst, 9_990, Some(20))
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::LimitExceeded);
    assert!(error.message.contains("scan-bound-exceeded"));
    assert!(
        ledger::ranked_needs(&points(), "", LedgerOrder::GreatestFirst, 9_900, Some(100)).is_ok()
    );
}

// ------------------------------------------------------------------------------------------------
// Declarations

/// Verifies: LDG-011, LDG-1548, LDG-1549, LDG-1550
#[test]
fn declarations_are_checked_and_a_unit_or_pool_in_use_is_not_removed() {
    // The definitions are listed as declared.
    let world = ledger_world(vec![unit_limit(Some(0), Some(10), None)]);
    let state = world.state(&tenant_id("alpha"));
    let listed = ledger::read_definitions(&state, &ctx("alpha", user("alpha:owner"), T0)).unwrap();
    assert_eq!(listed.units, [points()]);
    assert_eq!(listed.pools, [budget()]);
    assert_eq!(listed.limits, [unit_limit(Some(0), Some(10), None)]);

    let world = ledger_world(vec![]);
    // A limit of a unit that is not declared, a floor above the cap, a cap per period without its
    // period.
    let stamps = LedgerLimit {
        unit: LedgerUnit::new("stamps").unwrap(),
        additions_only: None,
        ..unit_limit(Some(0), None, None)
    };
    for bad in [
        stamps,
        unit_limit(Some(10), Some(5), None),
        LedgerLimit {
            period_cap: Some(5),
            additions_only: None,
            ..unit_limit(None, None, None)
        },
        unit_limit(None, None, None),
    ] {
        let error =
            change_as(&world, "owner", T0, vec![SchemaChange::SetLedgerLimit(bad)]).unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidSchema, "{}", error.message);
    }
    // A writer holds neither Manage on the schema nor ledger-configure.
    let error = change_as(
        &world,
        "writer1",
        T0,
        vec![SchemaChange::DeclareLedgerUnit(LedgerUnitDecl {
            unit: LedgerUnit::new("stamps").unwrap(),
            description: "Stamps".into(),
            expiry: None,
        })],
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Unauthorized);
    // At most 64 units.
    let many = (0..65)
        .map(|n| {
            SchemaChange::DeclareLedgerUnit(LedgerUnitDecl {
                unit: LedgerUnit::new(format!("u{n}")).unwrap(),
                description: String::new(),
                expiry: None,
            })
        })
        .collect();
    assert_eq!(
        change_as(&world, "owner", T0, many).unwrap_err().kind,
        ErrorKind::InvalidSchema
    );

    // A unit or a pool that an account names is not removed; an unused one is.
    let used = posted(&world, T0, pool(), 5, "a");
    for definition in [
        LedgerDefinitionRef::Unit { unit: points() },
        LedgerDefinitionRef::Pool { pool: budget() },
    ] {
        let error = change_as(
            &used,
            "owner",
            T0 + 1,
            vec![SchemaChange::RemoveLedgerDefinition(definition)],
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::StillReferenced);
    }
    let removed = change_as(
        &world,
        "owner",
        T0 + 1,
        vec![
            SchemaChange::RemoveLedgerDefinition(LedgerDefinitionRef::Pool { pool: budget() }),
            SchemaChange::RemoveLedgerDefinition(LedgerDefinitionRef::Unit { unit: points() }),
        ],
    )
    .unwrap();
    assert!(
        removed.tenants[&tenant_id("alpha")].schema.ledger.is_none(),
        "a schema that declares nothing holds no ledger table"
    );
    // Removing a limit changes no entry and no balance (LDG-016).
    let limited = change_as(
        &used,
        "owner",
        T0 + 2,
        vec![SchemaChange::SetLedgerLimit(unit_limit(
            Some(0),
            None,
            None,
        ))],
    )
    .unwrap();
    let unlimited = change_as(
        &limited,
        "owner",
        T0 + 3,
        vec![SchemaChange::RemoveLedgerDefinition(
            LedgerDefinitionRef::Limit {
                unit: points(),
                owner: None,
                scope: None,
            },
        )],
    )
    .unwrap();
    assert_eq!(balance(&unlimited, &pool()), 5);
    let unlimited = posted(&unlimited, T0 + 4, pool(), -50, "below");
    assert_eq!(balance(&unlimited, &pool()), -45);
    assert!(change_as(
        &unlimited,
        "owner",
        T0 + 5,
        vec![SchemaChange::RemoveLedgerDefinition(
            LedgerDefinitionRef::Limit {
                unit: points(),
                owner: None,
                scope: None,
            }
        )],
    )
    .is_err());
}

// ------------------------------------------------------------------------------------------------
// Erasure

/// Verifies: LDG-027, LDG-028
#[test]
fn erasure_clears_the_principal_and_keeps_every_amount_and_balance() {
    let world = ledger_world(vec![]);
    let world = posted(&world, T0, person("writer1"), 5, "a");
    let world = posted(&world, T0 + 1, pool(), 8, "b");
    let mut ledger_state = world.tenants[&tenant_id("alpha")].ledger.clone().unwrap();
    let before = ledger_state.clone();
    let done = ledger::anonymise(&mut ledger_state, &principal_id("alpha:writer1"));
    assert_eq!(
        done,
        ledger::Anonymised {
            accounts: 1,
            entries: 0
        }
    );
    let done = ledger::anonymise(&mut ledger_state, &principal_id("alpha:owner"));
    assert_eq!(
        done,
        ledger::Anonymised {
            accounts: 0,
            entries: 2
        }
    );
    for (key, account) in &ledger_state.accounts {
        let old = &before.accounts[key];
        assert_eq!(
            (account.balance, account.last_sequence),
            (old.balance, old.last_sequence)
        );
        assert_ne!(account.owner, Some(person("writer1")));
    }
    for (key, list) in &ledger_state.entries {
        let old = &before.entries[key];
        assert_eq!(list.len(), old.len());
        for (new, old) in list.iter().zip(old) {
            assert_eq!(
                (
                    new.amount,
                    &new.idempotency_key,
                    &new.reason,
                    new.sequence,
                    new.at
                ),
                (
                    old.amount,
                    &old.idempotency_key,
                    &old.reason,
                    old.sequence,
                    old.at
                )
            );
            assert_eq!(new.caused_by, None);
        }
    }
}

// ------------------------------------------------------------------------------------------------
// A cap per period that counts additions only (WG-5598)

/// The unit limit of a cap of `cap` points a day, counting additions only or every credit.
fn daily_cap(cap: u64, additions_only: Option<bool>) -> LedgerLimit {
    LedgerLimit {
        additions_only,
        ..unit_limit(None, None, Some((cap, LedgerPeriod::Day)))
    }
}

/// A reversal of `entry` as the owner: the negated amount, naming the entry it corrects.
fn reversal_of(entry: u64, amount: i64, key: &str) -> LedgerPostingRequest {
    LedgerPostingRequest {
        corrects: Some(entry),
        ..request(person("writer1"), amount, key)
    }
}

/// Verifies: LDG-2902
/// A debit of five and its reversal, under a daily cap of ten that counts additions only: the
/// reversal counts nothing, so ten more points still fit the day and one more does not.
#[test]
fn a_reversal_under_a_cap_that_counts_additions_only_neither_counts_nor_frees() {
    let world = ledger_world(vec![daily_cap(10, Some(true))]);
    let world = posted(&world, T0, person("writer1"), -5, "spend");
    let world = post_as(
        &world,
        "owner",
        T0 + 1,
        vec![reversal_of(1, 5, "spend-back")],
    )
    .expect("the reversal is accepted")
    .0;
    assert_eq!(balance(&world, &person("writer1")), 0);
    let world = posted(&world, T0 + 2, person("writer1"), 10, "credit-ten");
    assert_eq!(
        failed_rule(&refused(&world, T0 + 3, person("writer1"), 1, "one-more")),
        "ledger:above-period-cap"
    );
}

/// Verifies: LDG-2902
/// The same postings under a cap that counts every credit: the reversal of the debit is a credit
/// of five, so ten more points do not fit the day.
#[test]
fn a_reversal_under_a_cap_that_counts_every_credit_counts_as_one() {
    let world = ledger_world(vec![daily_cap(10, None)]);
    let world = posted(&world, T0, person("writer1"), -5, "spend");
    let world = post_as(
        &world,
        "owner",
        T0 + 1,
        vec![reversal_of(1, 5, "spend-back")],
    )
    .expect("the reversal is accepted")
    .0;
    assert_eq!(
        failed_rule(&refused(
            &world,
            T0 + 2,
            person("writer1"),
            10,
            "credit-ten"
        )),
        "ledger:above-period-cap"
    );
    let world = posted(&world, T0 + 3, person("writer1"), 5, "credit-five");
    assert_eq!(balance(&world, &person("writer1")), 5);
}
