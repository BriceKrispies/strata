//! The ledger under the fourth wave's three changes, as the kernel decides them: policies over the
//! ledger resource (D-6550, LDG-1540, LDG-1544), the expiry of credits by period (D-6553,
//! LDG-1545 to LDG-1547) and the limits of single accounts held by the ledger's storage and not by
//! the schema (D-6554, LDG-1550).

use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use strata_kernel::ledger;
use strata_kernel::ledger_expiry;
use strata_kernel::testkit::*;
use strata_kernel::*;

const T0: i64 = 1_763_640_000_000; // 2025-11-20T12:00:00Z, a Thursday
const HOUR: i64 = 3_600_000;
const DAY: i64 = 86_400_000;

fn points() -> LedgerUnit {
    LedgerUnit::new("points").unwrap()
}

fn stamps() -> LedgerUnit {
    LedgerUnit::new("stamps").unwrap()
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

fn scope(text: &str) -> LedgerScope {
    LedgerScope(text.to_string())
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

fn unit_decl(unit: LedgerUnit, expiry: Option<LedgerExpiry>) -> SchemaChange {
    SchemaChange::DeclareLedgerUnit(LedgerUnitDecl {
        description: unit.to_string(),
        unit,
        expiry,
    })
}

/// The notes world with the units `points` and `stamps` and the pool `budget` declared in alpha.
fn ledger_world() -> World {
    change_as(
        &notes_world(),
        "owner",
        T0 - 1_000,
        vec![
            unit_decl(points(), None),
            unit_decl(stamps(), None),
            SchemaChange::DeclareLedgerPool(LedgerPoolDecl {
                pool: budget(),
                description: "Promotion budget".into(),
            }),
        ],
    )
    .expect("the declarations are accepted")
}

fn request(
    owner: LedgerOwner,
    unit: LedgerUnit,
    scope_text: &str,
    amount: i64,
    key: &str,
) -> LedgerPostingRequest {
    LedgerPostingRequest {
        owner,
        unit,
        amount: LedgerAmount(amount),
        idempotency_key: key.into(),
        reason: "test".into(),
        corrects: None,
        scope: (!scope_text.is_empty()).then(|| scope(scope_text)),
    }
}

/// Post as the owner of the tenant; the posting must be accepted.
fn posted(world: &World, now: i64, request: LedgerPostingRequest) -> World {
    world
        .step(
            &Intent::PostLedger {
                postings: vec![request],
            },
            &ctx("alpha", user("alpha:owner"), now),
        )
        .unwrap_or_else(|e| panic!("the posting was refused: {}", e.describe()))
        .0
}

fn tenant_of(world: &World) -> Arc<TenantState> {
    world.tenants[&tenant_id("alpha")].clone()
}

fn header_of(
    world: &World,
    owner: &LedgerOwner,
    unit: &LedgerUnit,
    scope: &str,
) -> Option<LedgerAccount> {
    let tenant = tenant_of(world);
    let id = ledger::account_id(&tenant.id, owner, unit, scope);
    tenant
        .ledger
        .as_ref()
        .and_then(|l| l.accounts.get(&ledger::account_key(&id)).cloned())
}

fn journal_of(world: &World, owner: &LedgerOwner, unit: &LedgerUnit) -> Vec<LedgerEntry> {
    let tenant = tenant_of(world);
    let id = ledger::account_id(&tenant.id, owner, unit, "");
    tenant
        .ledger
        .as_ref()
        .and_then(|l| l.entries.get(&ledger::account_key(&id)).cloned())
        .unwrap_or_default()
}

// ------------------------------------------------------------------------------------------------
// Policies over the ledger (D-6550, LDG-1540, LDG-1544)

fn read_policy(
    id: &str,
    role_name: &str,
    unit: Option<LedgerUnit>,
    condition: Option<Expr>,
) -> SchemaChange {
    SchemaChange::DefinePolicy(PolicyDef {
        id: PolicyId::new(id).expect("policy id"),
        role: role(role_name),
        resource: Resource::Ledger { unit },
        actions: BTreeSet::from([Action::Read]),
        condition,
    })
}

fn can_read_ledger(role_name: &str) -> SchemaChange {
    SchemaChange::DefineRolePermissions {
        role: role(role_name),
        permissions: vec![Permission::Capability(ledger::LEDGER_READ.into())],
    }
}

/// `writer` holds `ledger-read` and the policy "an owner of an account reads it"; four accounts of
/// `points`: writer1 7, writer2 9, editor 3 and the pool 100.
fn owner_policy_world() -> World {
    let world = change_as(
        &ledger_world(),
        "owner",
        T0,
        vec![
            can_read_ledger("writer"),
            read_policy(
                "owners_read_their_accounts",
                "writer",
                None,
                Some(eq(var(Scope::Record, "owner"), Expr::Principal)),
            ),
        ],
    )
    .expect("the policy is accepted");
    let mut world = world;
    for (n, (owner, amount)) in [
        (person("writer1"), 7),
        (person("writer2"), 9),
        (person("editor"), 3),
        (pool(), 100),
    ]
    .into_iter()
    .enumerate()
    {
        world = posted(
            &world,
            T0 + 1 + n as i64,
            request(owner, points(), "", amount, &format!("k{n}")),
        );
    }
    world
}

/// Verifies: LDG-1540
#[test]
fn an_owner_policy_lets_a_reader_read_their_own_account_and_not_another() {
    let world = owner_policy_world();
    let state = world.state(&tenant_id("alpha"));
    let as_writer1 = ctx("alpha", user("alpha:writer1"), T0 + 50);
    let mine = ledger::read_balance(
        &state,
        &person("writer1"),
        &points(),
        "",
        false,
        &as_writer1,
    )
    .unwrap();
    assert_eq!(mine.balance, LedgerAmount(7));
    let (rows, _) = ledger::read_entries(
        &state,
        &person("writer1"),
        &points(),
        "",
        None,
        None,
        false,
        &as_writer1,
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    // Another person's account and a pool's are not told apart from an account that is not there.
    for other in [person("writer2"), pool(), person("editor")] {
        let error =
            ledger::read_balance(&state, &other, &points(), "", false, &as_writer1).unwrap_err();
        assert_eq!(error.kind, ErrorKind::UnknownRecord, "{other:?}");
        let error = ledger::read_entries(
            &state,
            &other,
            &points(),
            "",
            None,
            None,
            false,
            &as_writer1,
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::UnknownRecord, "{other:?}");
    }
    let nobody = ledger::read_balance(
        &state,
        &person("stranger"),
        &points(),
        "",
        false,
        &as_writer1,
    )
    .unwrap_err();
    assert_eq!(nobody.kind, ErrorKind::UnknownRecord);
    // The owner of the tenant, which no policy bounds, still reads every account.
    let as_owner = ctx("alpha", user("alpha:owner"), T0 + 50);
    assert_eq!(
        ledger::read_balance(&state, &pool(), &points(), "", false, &as_owner)
            .unwrap()
            .balance,
        LedgerAmount(100)
    );
    // A person with no capability and no policy reads their own account and no other, as before.
    let as_editor = ctx("alpha", user("alpha:editor"), T0 + 50);
    assert_eq!(
        ledger::read_balance(&state, &person("writer1"), &points(), "", false, &as_editor)
            .unwrap_err()
            .kind,
        ErrorKind::Unauthorized
    );
}

fn ranked(
    state: &State,
    who: &str,
    order: LedgerOrder,
    offset: u32,
    limit: Option<u32>,
) -> Result<(Vec<LedgerAccountRank>, Option<u32>), DomainError> {
    ledger::read_ranked(
        state,
        &points(),
        "",
        order,
        offset,
        limit,
        &ctx("alpha", user(&format!("alpha:{who}")), T0 + 50),
    )
}

/// Verifies: LDG-1544
#[test]
fn a_ranked_listing_shows_a_reader_only_the_accounts_the_policies_permit() {
    let world = owner_policy_world();
    let state = world.state(&tenant_id("alpha"));
    // The owner of the tenant sees all four, the pool first.
    let (all, _) = ranked(&state, "owner", LedgerOrder::GreatestFirst, 0, None).unwrap();
    assert_eq!(all.len(), 4);
    assert_eq!(all[0].owner, pool());
    // writer1 ranks third of four, and is shown alone: the others are neither listed nor counted.
    let (rows, next) = ranked(&state, "writer1", LedgerOrder::GreatestFirst, 0, None).unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| (r.owner.clone(), r.balance.0))
            .collect::<Vec<_>>(),
        [(person("writer1"), 7)]
    );
    assert_eq!(next, None);
    // The offset counts the accounts the reader may read: there is no second, and no position
    // is disclosed by an empty page that begins after the first.
    let (rows, next) = ranked(&state, "writer1", LedgerOrder::LeastFirst, 1, Some(5)).unwrap();
    assert!(rows.is_empty());
    assert_eq!(next, None);
    let (rows, _) = ranked(&state, "writer2", LedgerOrder::GreatestFirst, 0, Some(1)).unwrap();
    assert_eq!(rows[0].owner, person("writer2"));
}

/// The snapshot a store builds from `needs`: no records, and of the ledger only what the needs name.
fn thin(world: &World, needs: &[Need]) -> State {
    let alpha = tenant_id("alpha");
    let full = &world.tenants[&alpha];
    let mut part = ledger::empty_state();
    for need in needs {
        if let (Need::Ledger(want), Some(from)) = (need, full.ledger.as_ref()) {
            ledger::partial(from, want, &mut part);
        }
    }
    let mut part = Some(part);
    ledger::tidy_state(&mut part);
    State {
        tenant_id: alpha,
        tenant: Some(Arc::new(TenantState {
            records: BTreeMap::new(),
            ledger: part,
            ..TenantState::clone(full)
        })),
        coverage: Coverage::Partial(needs.iter().cloned().collect::<BTreeSet<_>>()),
    }
}

fn window(limit: u32) -> Need {
    Need::Ledger(LedgerNeed::Ranked {
        unit: points(),
        scope: scope(""),
        order: LedgerOrder::GreatestFirst,
        offset: 0,
        limit,
    })
}

/// Verifies: LDG-1544, LDG-1543
///
/// The scan bound applies to the accounts examined: a page that is short of readable accounts asks
/// for a wider window of the ranking, and answers once the window holds enough or all of them.
#[test]
fn a_filtered_ranking_widens_its_window_until_the_page_is_found() {
    let world = owner_policy_world();
    let as_writer1 = ctx("alpha", user("alpha:writer1"), T0 + 50);
    let ask = |state: &State| {
        ledger::read_ranked(
            state,
            &points(),
            "",
            LedgerOrder::GreatestFirst,
            0,
            Some(1),
            &as_writer1,
        )
    };
    // The first window holds two accounts, neither the reader's: the walk asks for four.
    let narrow = thin(&world, &[window(2)]);
    let error = ask(&narrow).unwrap_err();
    assert_eq!(error.kind, ErrorKind::InsufficientState);
    assert_eq!(error.missing, Some(window(4)));
    // Four accounts fill the second window, so the walk cannot tell that no more follow, and asks
    // for eight; the store answers with the four it has, which is all of them.
    let wider = thin(&world, &[window(2), window(4)]);
    let error = ask(&wider).unwrap_err();
    assert_eq!(error.missing, Some(window(8)));
    let wide = thin(&world, &[window(2), window(4), window(8)]);
    let (rows, next) = ask(&wide).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].owner, person("writer1"));
    assert_eq!(next, None);
}

/// Verifies: LDG-1540
#[test]
fn a_policy_of_one_unit_bounds_a_reader_to_that_unit() {
    let world = change_as(
        &ledger_world(),
        "owner",
        T0,
        vec![
            can_read_ledger("editor"),
            read_policy("editors_read_stamps", "editor", Some(stamps()), None),
        ],
    )
    .unwrap();
    let world = posted(
        &world,
        T0 + 1,
        request(person("writer1"), points(), "", 5, "a"),
    );
    let world = posted(
        &world,
        T0 + 2,
        request(person("writer1"), stamps(), "", 2, "b"),
    );
    let state = world.state(&tenant_id("alpha"));
    let as_editor = ctx("alpha", user("alpha:editor"), T0 + 50);
    assert_eq!(
        ledger::read_balance(&state, &person("writer1"), &stamps(), "", false, &as_editor)
            .unwrap()
            .balance,
        LedgerAmount(2)
    );
    assert_eq!(
        ledger::read_balance(&state, &person("writer1"), &points(), "", false, &as_editor)
            .unwrap_err()
            .kind,
        ErrorKind::UnknownRecord
    );
    // The same policy without a unit reaches every unit.
    let world = change_as(
        &world,
        "owner",
        T0 + 3,
        vec![read_policy("editors_read_everything", "editor", None, None)],
    )
    .unwrap();
    let state = world.state(&tenant_id("alpha"));
    assert!(
        ledger::read_balance(&state, &person("writer1"), &points(), "", false, &as_editor).is_ok()
    );
}

/// Verifies: LDG-1540
#[test]
fn a_condition_over_the_fields_of_an_entry_filters_entries_and_withholds_the_balance() {
    let positive = Expr::Cmp(
        CmpOp::Gt,
        Box::new(var(Scope::Record, "amount")),
        Box::new(Expr::Lit(Value::Int(0))),
    );
    let world = change_as(
        &ledger_world(),
        "owner",
        T0,
        vec![
            can_read_ledger("editor"),
            read_policy("editors_read_credits", "editor", None, Some(positive)),
        ],
    )
    .unwrap();
    let world = posted(
        &world,
        T0 + 1,
        request(person("writer1"), points(), "", 5, "a"),
    );
    let world = posted(
        &world,
        T0 + 2,
        request(person("writer1"), points(), "", -2, "b"),
    );
    let state = world.state(&tenant_id("alpha"));
    let as_editor = ctx("alpha", user("alpha:editor"), T0 + 50);
    let (rows, _) = ledger::read_entries(
        &state,
        &person("writer1"),
        &points(),
        "",
        None,
        None,
        false,
        &as_editor,
    )
    .unwrap();
    assert_eq!(
        rows.iter().map(|e| e.amount.0).collect::<Vec<_>>(),
        [5],
        "only the credit is readable"
    );
    // A balance counts the debit, which the reader may not read, so none is given (LDG-025), and
    // the account is not in a ranking either (LDG-1544).
    assert_eq!(
        ledger::read_balance(&state, &person("writer1"), &points(), "", false, &as_editor)
            .unwrap_err()
            .kind,
        ErrorKind::UnknownRecord
    );
    let (rows, _) = ranked(&state, "editor", LedgerOrder::GreatestFirst, 0, None).unwrap();
    assert!(rows.is_empty());
}

/// Verifies: LDG-1540
#[test]
fn a_policy_over_the_ledger_is_checked_when_it_is_defined() {
    let world = ledger_world();
    let create = SchemaChange::DefinePolicy(PolicyDef {
        id: PolicyId::new("editors_write_ledger").unwrap(),
        role: role("editor"),
        resource: Resource::Ledger { unit: None },
        actions: BTreeSet::from([Action::Create]),
        condition: None,
    });
    assert!(change_as(&world, "owner", T0, vec![create]).is_err());
    let unknown_unit = read_policy(
        "editors_read_nothing",
        "editor",
        Some(LedgerUnit::new("nope").unwrap()),
        None,
    );
    assert!(change_as(&world, "owner", T0, vec![unknown_unit]).is_err());
    let unknown_field = read_policy(
        "editors_read_nope",
        "editor",
        None,
        Some(eq(var(Scope::Record, "nope"), lit("x"))),
    );
    assert!(change_as(&world, "owner", T0, vec![unknown_field]).is_err());
    let fine = read_policy(
        "editors_read_their_pools",
        "editor",
        None,
        Some(eq(var(Scope::Record, "pool"), lit("budget"))),
    );
    assert!(change_as(&world, "owner", T0, vec![fine]).is_ok());
}

/// Verifies: SCH-1575
#[test]
fn the_kernel_decides_who_hears_of_a_posting() {
    let world = owner_policy_world();
    let tenant = tenant_of(&world);
    let hears = |who: &str, owner: &LedgerOwner| {
        ledger::may_hear(
            &tenant,
            &user(&format!("alpha:{who}")),
            Timestamp(T0 + 50),
            owner,
            &points(),
            "",
        )
    };
    assert!(hears("writer1", &person("writer1")), "the owner hears");
    assert!(
        !hears("writer2", &person("writer1")),
        "another person does not"
    );
    assert!(
        !hears("writer1", &pool()),
        "a pool's postings are not told to a person"
    );
    assert!(
        hears("owner", &person("writer1")),
        "the owner of the tenant hears all"
    );
    assert!(!hears("stranger", &person("writer1")));
    assert!(
        hears("editor", &person("editor")),
        "an owner hears under ledger-read-own"
    );
    assert!(
        !ledger::may_hear(
            &tenant,
            &Principal::Anonymous,
            Timestamp(T0),
            &person("writer1"),
            &points(),
            ""
        ),
        "an anonymous subscriber hears nothing"
    );
}

// ------------------------------------------------------------------------------------------------
// Expiry of credits (D-6553, LDG-1545 to LDG-1547)

fn expiring_world() -> World {
    let expiry = LedgerExpiry {
        period: LedgerPeriod::Day,
        periods: 2,
    };
    change_as(
        &notes_world(),
        "owner",
        T0 - 1_000,
        vec![unit_decl(points(), Some(expiry))],
    )
    .expect("a unit with an expiry is declared")
}

fn day_start(at: i64) -> i64 {
    at.div_euclid(DAY) * DAY
}

fn expire_at(world: &World, now: i64) -> Result<(World, Vec<LedgerPostingResult>), DomainError> {
    let expiry = LedgerExpiry {
        period: LedgerPeriod::Day,
        periods: 2,
    };
    let cutoff = ledger_expiry::cutoff(&expiry, Timestamp(now));
    let (next, outcome) = world.step(
        &Intent::ExpireLedger {
            unit: points(),
            cutoff,
        },
        &ctx("alpha", Principal::System, now),
    )?;
    match outcome.output {
        Output::LedgerPosted { results } => Ok((next, results)),
        other => panic!("unexpected output {other:?}"),
    }
}

/// Verifies: LDG-1545
#[test]
fn a_debit_consumes_the_oldest_credit_first() {
    let world = expiring_world();
    let world = posted(
        &world,
        T0,
        request(person("writer1"), points(), "", 10, "a"),
    );
    let world = posted(
        &world,
        T0 + DAY,
        request(person("writer1"), points(), "", 5, "b"),
    );
    let before = header_of(&world, &person("writer1"), &points(), "").unwrap();
    assert_eq!(
        before.buckets,
        Some(vec![
            LedgerBucket {
                period_start: Timestamp(day_start(T0)),
                remaining: 10
            },
            LedgerBucket {
                period_start: Timestamp(day_start(T0 + DAY)),
                remaining: 5
            },
        ])
    );
    let world = posted(
        &world,
        T0 + DAY + 1,
        request(person("writer1"), points(), "", -12, "c"),
    );
    let after = header_of(&world, &person("writer1"), &points(), "").unwrap();
    assert_eq!(after.balance, LedgerAmount(3));
    assert_eq!(
        after.buckets,
        Some(vec![LedgerBucket {
            period_start: Timestamp(day_start(T0 + DAY)),
            remaining: 3
        }]),
        "the first day's credit is spent first, then 2 of the second's"
    );
}

/// Verifies: LDG-1546, LDG-1547
#[test]
fn an_expired_credit_is_posted_once_as_an_entry_and_a_retry_posts_nothing() {
    let world = expiring_world();
    let world = posted(
        &world,
        T0,
        request(person("writer1"), points(), "", 10, "a"),
    );
    let world = posted(
        &world,
        T0 + DAY,
        request(person("writer1"), points(), "", 5, "b"),
    );
    let world = posted(&world, T0, request(person("writer2"), points(), "", 4, "c"));
    // Two days after the end of the first day, only the first day's credit has ended its life.
    let now = day_start(T0) + 2 * DAY + HOUR;
    let (world, results) = expire_at(&world, now).unwrap();
    assert_eq!(
        results.len(),
        2,
        "one entry for each account with credit in the bucket"
    );
    let key = ledger_expiry::expiry_key(Timestamp(day_start(T0)));
    let journal = journal_of(&world, &person("writer1"), &points());
    let last = journal.last().unwrap();
    assert_eq!(
        (
            last.amount.0,
            last.idempotency_key.as_str(),
            last.reason.as_str()
        ),
        (-10, key.as_str(), "expiry")
    );
    let header = header_of(&world, &person("writer1"), &points(), "").unwrap();
    assert_eq!(
        header.balance,
        LedgerAmount(5),
        "the second day's credit remains"
    );
    assert_eq!(ledger::replayed_balance(&journal), Ok(5));
    assert_eq!(
        header_of(&world, &person("writer2"), &points(), "")
            .unwrap()
            .balance,
        LedgerAmount(0)
    );
    // The same job again, as after a crash between doing and reporting, posts nothing.
    let (again, results) = expire_at(&world, now).unwrap();
    assert!(results.is_empty());
    assert_eq!(tenant_of(&again).ledger, tenant_of(&world).ledger);
    // A day later the second day's credit expires, once.
    let (world, results) = expire_at(&world, now + DAY).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(
        header_of(&world, &person("writer1"), &points(), "")
            .unwrap()
            .balance,
        LedgerAmount(0)
    );
    assert_eq!(journal_of(&world, &person("writer1"), &points()).len(), 4);
}

/// Verifies: LDG-1547
#[test]
fn one_transaction_of_expiry_handles_at_most_one_hundred_accounts() {
    let mut world = expiring_world();
    // 130 accounts of scope s0..s129 for one person, each with credit in the first day's bucket.
    for n in 0..130 {
        world = posted(
            &world,
            T0,
            request(
                person("writer1"),
                points(),
                &format!("s{n}"),
                3,
                &format!("k{n}"),
            ),
        );
    }
    let now = day_start(T0) + 3 * DAY;
    let (world, results) = expire_at(&world, now).unwrap();
    assert_eq!(results.len(), 100);
    let (world, results) = expire_at(&world, now).unwrap();
    assert_eq!(results.len(), 30);
    let (_, results) = expire_at(&world, now).unwrap();
    assert!(results.is_empty());
}

/// Verifies: LDG-1546
#[test]
fn the_expiry_of_a_bucket_whose_life_has_not_ended_is_refused() {
    let world = expiring_world();
    let world = posted(
        &world,
        T0,
        request(person("writer1"), points(), "", 10, "a"),
    );
    let too_early = Intent::ExpireLedger {
        unit: points(),
        cutoff: Timestamp(day_start(T0)),
    };
    let error = world
        .step(&too_early, &ctx("alpha", Principal::System, T0 + HOUR))
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidIntent);
    // A person without the capability may not expire credits; the owner of the tenant may.
    let late = Intent::ExpireLedger {
        unit: points(),
        cutoff: Timestamp(day_start(T0)),
    };
    let now = day_start(T0) + 2 * DAY;
    assert_eq!(
        world
            .step(&late, &ctx("alpha", user("alpha:writer1"), now))
            .unwrap_err()
            .kind,
        ErrorKind::Unauthorized
    );
    assert!(world
        .step(&late, &ctx("alpha", user("alpha:owner"), now))
        .is_ok());
    // A unit that declares no expiry has none to run.
    let plain = change_as(&world, "owner", T0 + 5, vec![unit_decl(stamps(), None)]).unwrap();
    let none = Intent::ExpireLedger {
        unit: stamps(),
        cutoff: Timestamp(0),
    };
    assert_eq!(
        plain
            .step(&none, &ctx("alpha", Principal::System, now))
            .unwrap_err()
            .kind,
        ErrorKind::InvalidIntent
    );
    // The expiry of a unit is one to thirty-six periods.
    let bad = unit_decl(
        stamps(),
        Some(LedgerExpiry {
            period: LedgerPeriod::Hour,
            periods: 37,
        }),
    );
    assert!(change_as(&world, "owner", T0 + 6, vec![bad]).is_err());
}

/// What the ledger should hold for one account, worked out without the kernel: the lots of
/// credit by the start of their day, the balance and the sum of the journal.
#[derive(Clone, Default, Debug)]
struct Model {
    lots: BTreeMap<i64, u64>,
    balance: i64,
}

impl Model {
    fn post(&mut self, amount: i64, now: i64) {
        if amount > 0 {
            let debt = if self.balance < 0 {
                self.balance.unsigned_abs()
            } else {
                0
            };
            let lot = (amount as u64).saturating_sub(debt);
            if lot > 0 {
                *self.lots.entry(day_start(now)).or_insert(0) += lot;
            }
        } else {
            let mut left = amount.unsigned_abs();
            for remaining in self.lots.values_mut() {
                let taken = left.min(*remaining);
                *remaining -= taken;
                left -= taken;
            }
            self.lots.retain(|_, remaining| *remaining > 0);
        }
        self.balance += amount;
    }

    fn expire(&mut self, now: i64) {
        // The life of a day's credit is two days: it has ended when its day is two days gone.
        let cutoff = day_start(now) - 2 * DAY;
        let due: Vec<i64> = self.lots.range(..=cutoff).map(|(s, _)| *s).collect();
        for start in due {
            let remaining = self.lots.remove(&start).unwrap();
            self.balance -= remaining as i64;
        }
    }

    fn buckets(&self) -> Option<Vec<LedgerBucket>> {
        (!self.lots.is_empty()).then(|| {
            self.lots
                .iter()
                .map(|(start, remaining)| LedgerBucket {
                    period_start: Timestamp(*start),
                    remaining: *remaining,
                })
                .collect()
        })
    }
}

/// Verifies: LDG-1545, LDG-1546, LDG-020
///
/// Random credits, debits and expiries over two accounts, from a fixed seed: after every step each
/// balance is the sum of its entries (numbered without a gap), equals what a model that spends the
/// oldest credit first and expires a credit two days after its day holds, and, after an expiry,
/// holds no credit whose life has ended.
#[test]
fn credits_debits_and_expiries_keep_the_balance_the_sum_of_the_entries() {
    let owners = [person("writer1"), person("writer2")];
    // (account, kind: 0 credit, 1 debit, 2 expiry only, amount, hours that pass first)
    let step = (0usize..2, 0u8..3, 1i64..40, 0i64..=24);
    let mut runner = TestRunner::deterministic();
    for _ in 0..16 {
        let steps = proptest::collection::vec(step.clone(), 1..30)
            .new_tree(&mut runner)
            .unwrap()
            .current();
        let mut world = expiring_world();
        let mut models = vec![Model::default(); owners.len()];
        let mut now = T0;
        for (n, (who, kind, amount, hours)) in steps.into_iter().enumerate() {
            now += hours * HOUR;
            let signed = match kind {
                0 => Some(amount),
                1 => Some(-amount),
                _ => None,
            };
            if let Some(signed) = signed {
                world = posted(
                    &world,
                    now,
                    request(owners[who].clone(), points(), "", signed, &format!("k{n}")),
                );
                models[who].post(signed, now);
            }
            if kind != 0 {
                let (next, _) = expire_at(&world, now).unwrap();
                world = next;
                for model in models.iter_mut() {
                    model.expire(now);
                }
            }
            for (i, owner) in owners.iter().enumerate() {
                let journal = journal_of(&world, owner, &points());
                let sum: i64 = journal.iter().map(|e| e.amount.0).sum();
                assert_eq!(ledger::replayed_balance(&journal), Ok(sum));
                let header = header_of(&world, owner, &points(), "");
                let balance = header.as_ref().map_or(0, |h| h.balance.0);
                assert_eq!(balance, sum, "the balance is the sum of the entries");
                assert_eq!(balance, models[i].balance, "step {n}, account {i}");
                assert_eq!(
                    header.and_then(|h| h.buckets),
                    models[i].buckets(),
                    "step {n}, account {i}: the oldest credit is spent first"
                );
            }
        }
        // After an expiry at the last time, no credit that has ended its life is left to count.
        let (world, _) = expire_at(&world, now).unwrap();
        let cutoff = day_start(now) - 2 * DAY;
        for owner in &owners {
            if let Some(header) = header_of(&world, owner, &points(), "") {
                assert!(header
                    .buckets
                    .iter()
                    .flatten()
                    .all(|b| b.period_start.0 > cutoff && b.remaining > 0));
            }
        }
    }
}

/// Verifies: LDG-006
#[test]
fn a_poster_may_not_use_the_key_of_an_expiry() {
    let world = expiring_world();
    let error = world
        .step(
            &Intent::PostLedger {
                postings: vec![request(person("writer1"), points(), "", 1, "expiry:0")],
            },
            &ctx("alpha", user("alpha:owner"), T0),
        )
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidIntent);
}

// ------------------------------------------------------------------------------------------------
// Account limits outside the schema (D-6554, LDG-1550)

fn account_limit(owner: LedgerOwner, scope_text: &str, cap: i64) -> LedgerLimit {
    LedgerLimit {
        unit: points(),
        owner: Some(owner),
        floor: None,
        cap: Some(LedgerAmount(cap)),
        period_cap: None,
        period: None,
        scope: (!scope_text.is_empty()).then(|| scope(scope_text)),
    }
}

/// Verifies: LDG-011, LDG-012, LDG-1550
#[test]
fn the_limit_of_an_account_lives_in_the_ledger_and_not_in_the_schema() {
    let world = change_as(
        &ledger_world(),
        "owner",
        T0,
        vec![SchemaChange::SetLedgerLimit(account_limit(
            person("writer1"),
            "",
            10,
        ))],
    )
    .unwrap();
    let tenant = tenant_of(&world);
    assert!(
        tenant.schema.ledger.as_ref().unwrap().limits.is_empty(),
        "the schema holds no account limit"
    );
    assert_eq!(
        tenant
            .ledger
            .as_ref()
            .unwrap()
            .limits
            .as_ref()
            .unwrap()
            .len(),
        1
    );
    // The limit applies to its account only.
    let refused = world
        .step(
            &Intent::PostLedger {
                postings: vec![request(person("writer1"), points(), "", 11, "a")],
            },
            &ctx("alpha", user("alpha:owner"), T0 + 1),
        )
        .unwrap_err();
    assert_eq!(refused.kind, ErrorKind::ConstraintViolated);
    let world = posted(
        &world,
        T0 + 2,
        request(person("writer2"), points(), "", 11, "b"),
    );
    // It is listed with the definitions, and a limit that is not set cannot be removed.
    let listed = ledger::read_definitions(
        &world.state(&tenant_id("alpha")),
        &ctx("alpha", user("alpha:owner"), T0 + 3),
    )
    .unwrap();
    assert_eq!(listed.limits, [account_limit(person("writer1"), "", 10)]);
    let absent = change_as(
        &world,
        "owner",
        T0 + 4,
        vec![SchemaChange::RemoveLedgerDefinition(
            LedgerDefinitionRef::Limit {
                unit: points(),
                owner: Some(person("writer2")),
                scope: None,
            },
        )],
    );
    assert!(absent.is_err());
    let world = change_as(
        &world,
        "owner",
        T0 + 5,
        vec![SchemaChange::RemoveLedgerDefinition(
            LedgerDefinitionRef::Limit {
                unit: points(),
                owner: Some(person("writer1")),
                scope: None,
            },
        )],
    )
    .unwrap();
    let held = tenant_of(&world)
        .ledger
        .clone()
        .and_then(|l| l.limits)
        .unwrap_or_default();
    assert!(held.is_empty(), "the removed limit is gone");
    assert!(world
        .step(
            &Intent::PostLedger {
                postings: vec![request(person("writer1"), points(), "", 11, "c")],
            },
            &ctx("alpha", user("alpha:owner"), T0 + 6),
        )
        .is_ok());
    // A limit of a unit is still a declaration of the schema.
    let unit_limit = LedgerLimit {
        owner: None,
        ..account_limit(person("writer1"), "", 5)
    };
    let world = change_as(
        &world,
        "owner",
        T0 + 7,
        vec![SchemaChange::SetLedgerLimit(unit_limit.clone())],
    )
    .unwrap();
    assert_eq!(
        tenant_of(&world).schema.ledger.as_ref().unwrap().limits,
        [unit_limit]
    );
}

/// The world with `count` limits of single accounts set, as if they had been set one by one.
fn world_with_limits(count: usize) -> World {
    let mut world = ledger_world();
    let alpha = tenant_id("alpha");
    let mut tenant = TenantState::clone(&world.tenants[&alpha]);
    let mut limits: Vec<LedgerLimit> = (0..count)
        .map(|n| account_limit(person("writer1"), &format!("s{n}"), 50 + n as i64))
        .collect();
    limits.sort();
    tenant.ledger = Some(LedgerState {
        accounts: BTreeMap::new(),
        entries: BTreeMap::new(),
        limits: Some(limits),
    });
    world.tenants.insert(alpha, Arc::new(tenant));
    world
}

/// Verifies: LDG-1550
#[test]
fn a_posting_loads_the_limit_of_its_account_and_no_other() {
    let world = world_with_limits(limits::MAX_LEDGER_ACCOUNT_LIMITS);
    let alpha = tenant_id("alpha");
    let intent = Intent::PostLedger {
        postings: vec![request(person("writer1"), points(), "s77", 100, "a")],
    };
    let context = ctx("alpha", user("alpha:owner"), T0);
    let header_only = world.state(&alpha);
    let planned = refine_needs(&header_only, &intent);
    // The header, the entry of the key and one limit: three needs, however many limits there are.
    assert_eq!(planned.len(), 3, "{planned:?}");
    assert_eq!(
        planned
            .iter()
            .filter(|n| matches!(n, Need::Ledger(LedgerNeed::Limit { .. })))
            .count(),
        1
    );
    let state = thin(&world, &planned);
    let held = state.tenant.as_ref().unwrap().ledger.as_ref().unwrap();
    assert_eq!(
        held.limits.as_ref().unwrap().len(),
        1,
        "no other account's limit was loaded"
    );
    // The cap of that account is 127, and the posting of 100 fits it; 128 does not.
    assert!(transition(&state, &intent, &context).is_ok());
    let over = Intent::PostLedger {
        postings: vec![request(person("writer1"), points(), "s77", 128, "b")],
    };
    let planned = refine_needs(&header_only, &over);
    let error = transition(&thin(&world, &planned), &over, &context).unwrap_err();
    assert_eq!(error.kind, ErrorKind::ConstraintViolated);
    // Without the need of the limit the decision asks for it and does not go on without it.
    let account = ledger::account_id(&alpha, &person("writer1"), &points(), "s77");
    let without = thin(&world, &[Need::Ledger(LedgerNeed::Account { account })]);
    let error = transition(&without, &intent, &context).unwrap_err();
    assert_eq!(
        error.missing,
        Some(Need::Ledger(LedgerNeed::Limit {
            owner: person("writer1"),
            unit: points(),
            scope: scope("s77"),
        }))
    );
}

/// Verifies: LDG-1550
#[test]
fn a_tenant_holds_at_most_ten_thousand_account_limits() {
    let world = world_with_limits(limits::MAX_LEDGER_ACCOUNT_LIMITS);
    let error = change_as(
        &world,
        "owner",
        T0,
        vec![SchemaChange::SetLedgerLimit(account_limit(
            person("writer2"),
            "",
            9,
        ))],
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::LimitExceeded);
    assert!(error.describe().contains("too-many"));
    // Changing one that is set is not another one.
    assert!(change_as(
        &world,
        "owner",
        T0,
        vec![SchemaChange::SetLedgerLimit(account_limit(
            person("writer1"),
            "s5",
            9,
        ))],
    )
    .is_ok());
}

/// Verifies: LDG-020
#[test]
fn a_journal_replays_to_its_balance_and_a_gap_is_reported() {
    let world = ledger_world();
    let world = posted(&world, T0, request(person("writer1"), points(), "", 4, "a"));
    let world = posted(
        &world,
        T0 + 1,
        request(person("writer1"), points(), "", -1, "b"),
    );
    let journal = journal_of(&world, &person("writer1"), &points());
    let header = header_of(&world, &person("writer1"), &points(), "").unwrap();
    let mut replay = ledger::Replay::default();
    replay.feed(&journal[..1]);
    replay.feed(&journal[1..]);
    assert!(replay.agrees(header.balance.0, header.last_sequence));
    // A tampered header disagrees.
    assert!(!replay.agrees(header.balance.0 + 1, header.last_sequence));
    // An entry missing from the journal is reported, and not summed over.
    let mut gap = ledger::Replay::default();
    gap.feed(&journal[1..]);
    assert_eq!(gap.gap, Some(1));
    assert!(!gap.agrees(header.balance.0, header.last_sequence));
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

    /// Verifies: LDG-1545
    ///
    /// Whatever the credits and debits, the buckets add up to the balance when it is positive and
    /// to nothing when it is not.
    #[test]
    fn the_buckets_add_up_to_the_positive_balance(
        posts in proptest::collection::vec((-30i64..30, 0i64..48), 1..25)
    ) {
        let mut world = expiring_world();
        let mut now = T0;
        for (n, (amount, hours)) in posts.into_iter().enumerate() {
            let amount = if amount == 0 { 1 } else { amount };
            now += hours * HOUR;
            world = posted(
                &world,
                now,
                request(person("writer1"), points(), "", amount, &format!("p{n}")),
            );
            let header = header_of(&world, &person("writer1"), &points(), "").unwrap();
            let held: u64 = header.buckets.iter().flatten().map(|b| b.remaining).sum();
            prop_assert_eq!(held as i64, header.balance.0.max(0));
        }
    }
}
