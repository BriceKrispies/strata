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

// ------------------------------------------------------------------------------------------------
// Account limits outside the schema (D-6554, LDG-1550)
