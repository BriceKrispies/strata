//! Determinism, branch equivalence, divergence discovery and minimisation, on the real
//! Fieldnotes tenant installed from its bundle.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use strata_backend::simulated::Backend;
use strata_kernel::*;
use strata_protocol::tenants::load_all;
use strata_simulation::engine::{run, Outcome, Scenario, Scheduled, Trace};
use strata_simulation::journal::{replay_journal, Recorded};
use strata_simulation::project::{project, Proposal};
use strata_simulation::shell::{
    run_shell, Answer as ShellAnswer, Faults as ShellFaults, ShellScenario,
};
use strata_simulation::workload::generate;
use strata_substrate::bundle::install_into;
use strata_verification::evidence::Class;

fn tenant() -> TenantId {
    TenantId::new("fieldnotes").unwrap()
}

/// Both seeded tenants, installed through the kernel. Loaded and installed once per binary: every
/// scenario starts from a clone of the same world.
fn seeded() -> World {
    static SEEDED: OnceLock<World> = OnceLock::new();
    SEEDED
        .get_or_init(|| {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tenants");
            load_all(&root)
                .unwrap()
                .iter()
                .fold(World::default(), |world, loaded| {
                    install_into(&world, &loaded.bundle).unwrap()
                })
        })
        .clone()
}

/// The generated schedule of a seed and length, made once per binary run: each of its steps is
/// decided by the kernel on a scratch world, and the tests ask for the same schedules again.
fn schedule_of(seed: u64, steps: usize) -> Vec<Scheduled> {
    static MADE: Mutex<BTreeMap<(u64, usize), Vec<Scheduled>>> = Mutex::new(BTreeMap::new());
    let mut made = MADE.lock().unwrap();
    made.entry((seed, steps))
        .or_insert_with(|| generate(&seeded(), &tenant(), seed, steps, 1_800_000_000_000, 60_000))
        .clone()
}

fn scenario(seed: u64, steps: usize) -> Scenario {
    let initial = seeded();
    let schedule = schedule_of(seed, steps);
    Scenario {
        name: format!("editorial-{seed}"),
        seed,
        initial,
        schedule,
    }
}

fn field(name: &str) -> FieldId {
    FieldId::new(name).unwrap()
}

/// The first Impact Lab scenario: make the optional excerpt required.
fn require_excerpt(existing: ExistingData, before_step: usize) -> Proposal {
    Proposal {
        tenant: tenant(),
        principal: Principal::User(PrincipalId::new("user:admin").unwrap()),
        changes: vec![SchemaChange::DefineField {
            object: ObjectTypeId::new("post").unwrap(),
            field: FieldDef {
                id: field("excerpt"),
                label: "Excerpt".into(),
                ty: FieldType::Str,
                required: true,
                unique: false,
                default: None,
                read: None,
                personal: None,
                normalise: None,
                computed: None,
            },
        }],
        existing,
        before_step,
    }
}

/// Verifies: SIM-001
#[test]
fn the_same_scenario_gives_the_same_trace() {
    // Two runs of each of several scenarios, differing in seed and length. Each pair has equal
    // trace hashes and equal final worlds.
    let mut hashes = BTreeSet::new();
    for (seed, steps) in [(1, 30), (2, 80), (21, 50), (34, 70)] {
        let (first, world_a) = run(&scenario(seed, steps));
        let (second, world_b) = run(&scenario(seed, steps));
        assert_eq!(first.hash(), second.hash(), "seed {seed}");
        assert_eq!(world_a.hash(), world_b.hash(), "seed {seed}");
        assert_eq!(first, second, "seed {seed}: the traces themselves differ");
        hashes.insert(first.hash());
    }
    assert_eq!(
        hashes.len(),
        4,
        "four different scenarios, four different traces"
    );

    let (first, world_a) = run(&scenario(7, 60));
    let (second, world_b) = run(&scenario(7, 60));
    assert_eq!(first.hash(), second.hash());
    assert_eq!(world_a.hash(), world_b.hash());
    assert_eq!(first.kernel_version, KERNEL_VERSION);
    assert!(
        first.accepted() > 10,
        "the generated workload is mostly meaningful: {} accepted",
        first.accepted()
    );
    assert!(
        first.rejected() > 0,
        "and some of it is refused, as real traffic is"
    );

    let (other, _) = run(&scenario(8, 60));
    assert_ne!(
        first.hash(),
        other.hash(),
        "a different seed is a different execution"
    );
}

/// Changes that alter no behaviour, one of each kind of schema content: a definition restated
/// exactly, a new definition nothing uses, and a rename.
fn neutral_changes(schema: &TenantSchema) -> Vec<(&'static str, Vec<SchemaChange>)> {
    let post = ObjectTypeId::new("post").unwrap();
    let query = schema.queries.values().next().unwrap().clone();
    let policy = schema.policies.values().next().unwrap().clone();
    let field = schema.objects[&post]
        .fields
        .values()
        .next()
        .unwrap()
        .clone();
    let unused_query = QueryDef {
        id: QueryId::new("unused_query").unwrap(),
        object: post.clone(),
        filter: None,
        params: Default::default(),
        order: vec![],
        limit: Some(5),
        aggregates: None,
        declared_fields: None,
        tree: None,
    };
    vec![
        ("a query restated", vec![SchemaChange::DefineQuery(query)]),
        (
            "every query restated",
            schema
                .queries
                .values()
                .cloned()
                .map(SchemaChange::DefineQuery)
                .collect(),
        ),
        (
            "a policy restated",
            vec![SchemaChange::DefinePolicy(policy)],
        ),
        (
            "a field restated",
            vec![SchemaChange::DefineField {
                object: post.clone(),
                field,
            }],
        ),
        (
            "a query nothing uses",
            vec![SchemaChange::DefineQuery(unused_query.clone())],
        ),
        (
            "a view nothing links to",
            vec![
                SchemaChange::DefineQuery(unused_query),
                SchemaChange::DefineView(ViewDef {
                    id: ViewId::new("unused").unwrap(),
                    route: "/unused-page".into(),
                    title: "Unused".into(),
                    sections: [(
                        "posts".to_string(),
                        ViewSection {
                            marks: None,
                            joins: None,
                            query: QueryId::new("unused_query").unwrap(),
                            args: Default::default(),
                            include: vec![],
                            markdown: vec![],
                            single: false,
                            aggregate: None,
                        },
                    )]
                    .into(),
                }),
            ],
        ),
        (
            "a role no policy names",
            vec![SchemaChange::DefineRole(RoleDef {
                id: RoleId::new("reviewer").unwrap(),
                label: "Reviewer".into(),
            })],
        ),
        (
            "an object renamed",
            vec![SchemaChange::DefineObject {
                id: post,
                label: "Essay".into(),
            }],
        ),
        (
            "a constraint that always holds",
            vec![SchemaChange::DefineConstraint {
                object: ObjectTypeId::new("post").unwrap(),
                constraint: ConstraintDef {
                    id: ConstraintId::new("always").unwrap(),
                    expr: Expr::Lit(Value::Bool(true)),
                    message: "never refused".into(),
                },
            }],
        ),
    ]
}

/// Verifies: SIM-002
///
/// A change that alters no behaviour reports no divergence, for each kind of such change and
/// whether it lands before the first step or in the middle of the schedule.
#[test]
fn a_branch_with_a_change_that_changes_nothing_equals_its_baseline() {
    let scenario = scenario(5, 40);
    let changes = neutral_changes(&scenario.initial.tenants[&tenant()].schema);
    assert!(changes.len() >= 8);
    for (what, changes) in changes {
        for before_step in [0, 20] {
            let proposal = Proposal {
                changes: changes.clone(),
                before_step,
                ..require_excerpt(ExistingData::Reject, 0)
            };
            let projection = project(&scenario, &proposal, &mut |_, _, _| true).unwrap();
            let at = format!("{what}, before step {before_step}");
            assert!(projection.change_outcome.accepted(), "{at}");
            let c = &projection.comparison;
            assert!(
                c.outcome_changes.is_empty()
                    && c.record_changes.is_empty()
                    && c.query_changes.is_empty(),
                "{at}: {:?}",
                c.outcome_changes.first()
            );
            assert!(
                c.events_only_in_baseline.is_empty() && c.events_only_in_branch.is_empty(),
                "{at}"
            );
            assert_eq!(c.first_divergence, None, "{at}");
            assert!(projection.counterexample.is_none(), "{at}");
            assert!(c.queries_compared > 0, "{at}");
            assert!(projection.affected_existing.is_empty(), "{at}");
        }
    }
}

/// The first step whose outcome differs between two traces, worked out here from the traces
/// and not read from the comparison: the same step of the schedule, accepted with another
/// result or refused for another reason.
fn first_difference(baseline: &Trace, branch: &Trace) -> Option<usize> {
    let by_index = |trace: &Trace| -> BTreeMap<usize, Outcome> {
        trace
            .steps
            .iter()
            .filter_map(|step| step.index.map(|i| (i, step.outcome.clone())))
            .collect()
    };
    let (a, b) = (by_index(baseline), by_index(branch));
    a.iter()
        .find(|(index, left)| !match (left, &b[index]) {
            (Outcome::Accepted { output: x, .. }, Outcome::Accepted { output: y, .. }) => x == y,
            (Outcome::Rejected { kind: x, .. }, Outcome::Rejected { kind: y, .. }) => x == y,
            _ => false,
        })
        .map(|(index, _)| *index)
}

/// Verifies: SIM-003
#[test]
fn grandfathering_the_requirement_diverges_and_the_divergence_is_explained() {
    // For several schedules and several places to apply the change, the reported first divergence
    // is the first step at which the two executions' outcomes really differ.
    let mut diverged = 0;
    for seed in [5, 9, 14] {
        for before_step in [0, 25] {
            let scenario = scenario(seed, 30);
            let projection = project(
                &scenario,
                &require_excerpt(ExistingData::Grandfather, before_step),
                &mut |_, _, _| true,
            )
            .unwrap();
            let by_hand = first_difference(&projection.baseline, &projection.branch);
            assert_eq!(
                projection.comparison.first_divergence, by_hand,
                "seed {seed}, change before step {before_step}"
            );
            assert!(
                by_hand.is_none_or(|step| step >= before_step),
                "seed {seed}, change before step {before_step}: {by_hand:?}"
            );
            // A change applied midway leaves the steps before it alone: no outcome differs there.
            assert!(
                projection
                    .comparison
                    .outcome_changes
                    .iter()
                    .all(|change| change.step >= before_step),
                "seed {seed}, change before step {before_step}: an earlier step differs"
            );
            diverged += usize::from(by_hand.is_some());
        }
    }
    assert!(diverged >= 2, "the change diverged only {diverged} times");
    // A change that is refused diverges nowhere, and says so.
    let refused = project(
        &scenario(5, 30),
        &require_excerpt(ExistingData::Reject, 0),
        &mut |_, _, _| true,
    )
    .unwrap();
    assert_eq!(first_difference(&refused.baseline, &refused.branch), None);
    assert_eq!(refused.comparison.first_divergence, None);

    let scenario = scenario(5, 30);
    let proposal = require_excerpt(ExistingData::Grandfather, 0);
    let projection = project(&scenario, &proposal, &mut |_, _, _| true).unwrap();
    assert!(projection.change_outcome.accepted());
    let c = &projection.comparison;
    assert!(!c.identical);
    let first = c.first_divergence.expect("something diverges");
    assert_eq!(c.outcome_changes[0].step, first);
    // "First" is checked against every differing outcome, not only the first one listed: no
    // differing step comes before the reported divergence.
    assert!(
        c.outcome_changes.iter().all(|change| change.step >= first),
        "a differing outcome precedes the reported first divergence"
    );
    assert_eq!(c.outcome_changes[0].baseline, "accepted");
    assert_eq!(c.outcome_changes[0].branch, "rejected: MissingRequired");
    assert!(c
        .outcome_changes
        .iter()
        .all(|change| change.class == Class::Reproduced));
    assert!(c.outcome_changes[0]
        .because
        .iter()
        .any(|check| check.rule == "required:post.excerpt"));
    assert!(c.new_failures["required:post.excerpt"] >= 1);
    assert!(
        !c.record_changes.is_empty(),
        "writes that were refused leave different records behind"
    );
    assert!(
        c.other_tenants_untouched,
        "the second tenant is identical in both executions"
    );

    // The baseline keeps every invariant; the branch breaks exactly record conformance.
    let broken: Vec<&str> = c
        .invariants
        .iter()
        .filter(|i| i.baseline_holds && !i.branch_holds)
        .map(|i| i.id.as_str())
        .collect();
    assert_eq!(broken, ["records_conform"]);
    assert!(c.invariants.iter().all(|i| i.baseline_holds));

    // The counterexample is a shorter schedule that still diverges the same way.
    let counterexample = projection
        .counterexample
        .as_ref()
        .expect("a counterexample");
    assert_eq!(*counterexample.steps.last().unwrap(), first);
    assert!(counterexample.steps.len() <= first + 1);
    assert!(
        counterexample.steps.len() <= 3,
        "kept {:?}",
        counterexample.labels
    );
    assert_eq!(counterexample.branch, "rejected: MissingRequired");

    // Every evidence class appears, and the whole projection is reproducible.
    let classes: Vec<Class> = projection.findings.iter().map(|f| f.class).collect();
    for class in [
        Class::Proven,
        Class::Reproduced,
        Class::SoundApproximation,
        Class::Empirical,
        Class::Unknown,
    ] {
        assert!(classes.contains(&class), "{class:?} missing");
    }
    let again = project(&scenario, &proposal, &mut |_, _, _| true).unwrap();
    assert_eq!(
        (again.baseline_hash, again.branch_hash),
        (
            projection.baseline_hash.clone(),
            projection.branch_hash.clone()
        )
    );
    assert_eq!(again.comparison, projection.comparison);
}

/// Verifies: SIM-004
#[test]
fn a_journal_of_real_commits_replays_to_the_live_state() {
    // The journal is the one the backend's store writes: the tenant installed by the backend as a
    // deployed process installs it, then a generated editorial workload and role and schema
    // changes sent through the backend's request path, so the journal holds every kind of intent.
    let admin = |now: i64| Context {
        tenant: tenant(),
        principal: Principal::User(PrincipalId::new("user:admin").unwrap()),
        session: None,
        now: Timestamp(now),
        correlation: format!("t-{now}"),
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
    };
    let role_of = |name: &str| RoleId::new(name).unwrap();
    let mut work: Vec<Scheduled> = schedule_of(17, 8);
    fn add(work: &mut Vec<Scheduled>, label: &str, context: Context, intent: Intent) {
        work.push(Scheduled {
            label: label.to_string(),
            context,
            intent,
        })
    }
    let mirela = PrincipalId::new("user:mirela").unwrap();
    add(
        &mut work,
        "revoke",
        admin(1_900_000_000_000),
        Intent::RevokeRole {
            principal: mirela.clone(),
            role: role_of("author"),
        },
    );
    add(
        &mut work,
        "assign",
        admin(1_900_000_000_001),
        Intent::AssignRole {
            principal: mirela,
            role: role_of("author"),
        },
    );
    add(
        &mut work,
        "define role",
        admin(1_900_000_000_002),
        Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineRole(RoleDef {
                id: role_of("reviewer"),
                label: "Reviewer".into(),
            })],
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    // A record made and deleted again, so that a delete is in the journal whatever the workload
    // happened to do. Its id is the one the kernel gives it after the work above: the inputs are
    // worked out on a scratch world, the outcomes come from the backend.
    let category = ObjectTypeId::new("category").unwrap();
    let scratch = Intent::CreateRecord {
        object: category.clone(),
        fields: BTreeMap::from([
            (FieldId::new("name").unwrap(), Value::Str("Scratch".into())),
            (
                FieldId::new("slug").unwrap(),
                Value::Str("scratch-journal".into()),
            ),
        ]),
    };
    let mut scratch_world = seeded();
    for step in &work {
        if let Ok((next, _)) = scratch_world.step(&step.intent, &step.context) {
            scratch_world = next;
        }
    }
    let id = RecordId(scratch_world.tenants[&tenant()].next_record);
    add(&mut work, "scratch", admin(1_900_000_000_003), scratch);
    add(
        &mut work,
        "delete scratch",
        admin(1_900_000_000_004),
        Intent::DeleteRecord {
            object: category,
            id,
            expected_version: None,
        },
    );
    let shell = run_shell(
        &Backend::new(Some(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tenants"),
        )),
        &ShellScenario {
            name: "journal".into(),
            seed: 17,
            start: 1_800_000_000_000 - 3_600_000,
            schedule: work,
            faults: ShellFaults::default(),
            listen: vec![tenant()],
            keys: false,
            start_faults: Default::default(),
            operations: Vec::new(),
        },
    )
    .unwrap();
    let last = shell.trace.last_answers();
    let answers: Vec<&ShellAnswer> = last.values().copied().collect();
    assert!(matches!(
        answers[answers.len() - 2..],
        [ShellAnswer::Committed { .. }, ShellAnswer::Committed { .. }]
    ));
    let journal = shell.recordings[&tenant()].entries.clone();
    let kinds: BTreeSet<&str> = journal.iter().map(|e| e.intent.name()).collect();
    assert_eq!(
        kinds,
        BTreeSet::from([
            "create_tenant",
            "change_schema",
            "assign_role",
            "revoke_role",
            "define_team",
            "assign_team_role",
            "add_team_member",
            "create_record",
            "update_record",
            "delete_record"
        ]),
        "the journal does not hold every kind of intent"
    );
    // The backend's own replay of its journal, as the Impact Lab's replay runs it, agrees.
    let own = &shell.trace.journal[0];
    assert!(own.matches_live && own.first_mismatch.is_none());
    assert_eq!((own.agreed, own.entries), (journal.len(), journal.len()));
    let live = shell.world.tenants[&tenant()].clone();
    let report = replay_journal(&tenant(), &journal, Some(&live));
    assert_eq!(
        (report.agreed, report.entries),
        (journal.len(), journal.len())
    );
    assert!(
        report.matches_live
            && report.first_mismatch.is_none()
            && report.other_kernel_versions.is_empty()
    );

    // Live state that the journal does not account for is detected.
    let mut drifted = TenantState::clone(&live);
    drifted.records.values_mut().next().unwrap().clear();
    assert!(!replay_journal(&tenant(), &journal, Some(&drifted)).matches_live);
    // So is a journal entry whose recorded hash does not match what the kernel does, whichever
    // hash it is and wherever in the journal it sits.
    let late = journal.len() - 4;
    for (what, tamper) in [
        (
            "delta hash",
            (|e: &mut Recorded| e.delta_hash = "0".repeat(64)) as fn(&mut Recorded),
        ),
        ("events hash", |e| e.events_hash = "0".repeat(64)),
        ("revision", |e| e.revision = StateRevision(e.revision.0 + 9)),
    ] {
        for position in [5, late] {
            let mut forged = journal.clone();
            tamper(&mut forged[position]);
            // The mismatch is reported under the revision the entry now claims.
            let revision = forged[position].revision.0;
            let found = replay_journal(&tenant(), &forged, Some(&live));
            assert_eq!(
                found.first_mismatch.map(|m| m.0),
                Some(revision),
                "{what} at entry {position}"
            );
            assert!(found.agreed < found.entries, "{what} at entry {position}");
        }
    }
}
