//! Determinism, branch equivalence, divergence discovery and minimisation, on the real
//! Fieldnotes tenant installed from its bundle.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use strata_kernel::*;
use strata_protocol::tenants::load_all;
use strata_simulation::engine::{
    replay, run, DeliveryResult, Faults, Outcome, Scenario, Scheduled, Trace,
};
use strata_simulation::journal::{replay_journal, Recorded};
use strata_simulation::project::{project, Proposal};
use strata_simulation::workload::generate;
use strata_substrate::bundle::{install, install_into};
use strata_verification::evidence::Class;

fn tenant() -> TenantId {
    TenantId::new("fieldnotes").unwrap()
}

/// Both seeded tenants, installed through the kernel.
fn seeded() -> World {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tenants");
    load_all(&root)
        .unwrap()
        .iter()
        .fold(World::default(), |world, loaded| {
            install_into(&world, &loaded.bundle).unwrap()
        })
}

fn scenario(seed: u64, steps: usize, faults: Faults) -> Scenario {
    let initial = seeded();
    let schedule = generate(&initial, &tenant(), seed, steps, 1_800_000_000_000, 60_000);
    Scenario {
        name: format!("editorial-{seed}"),
        seed,
        initial,
        schedule,
        faults,
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
            },
        }],
        existing,
        before_step,
    }
}

/// Verifies: SIM-001
#[test]
fn the_same_scenario_gives_the_same_trace() {
    // Two runs of each of several scenarios: different seeds, lengths and fault rates, none of
    // them and heavy ones. Each pair has equal trace hashes and equal final worlds.
    let rough = Faults {
        commit_conflict: 300_000,
        delivery_drop: 400_000,
    };
    let mut hashes = BTreeSet::new();
    for (seed, steps, faults) in [
        (1, 30, Faults::default()),
        (2, 80, Faults::default()),
        (21, 50, rough),
        (34, 70, rough),
    ] {
        let (first, world_a) = run(&scenario(seed, steps, faults));
        let (second, world_b) = run(&scenario(seed, steps, faults));
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

    let faults = Faults {
        commit_conflict: 150_000,
        delivery_drop: 250_000,
    };
    let (first, world_a) = run(&scenario(7, 60, faults));
    let (second, world_b) = run(&scenario(7, 60, faults));
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

    let (other, _) = run(&scenario(8, 60, faults));
    assert_ne!(
        first.hash(),
        other.hash(),
        "a different seed is a different execution"
    );
}

#[test]
fn a_trace_replays_against_the_bare_kernel() {
    let faults = Faults {
        commit_conflict: 200_000,
        delivery_drop: 200_000,
    };
    let scenario = scenario(11, 50, faults);
    let (trace, _) = run(&scenario);
    let replayed = replay(&scenario.initial, &trace);
    assert_eq!(replayed.agreed, replayed.steps);
    assert_eq!(replayed.first_mismatch, None);
    assert!(replayed.final_hash_matches && replayed.kernel_version_matches);

    // A tampered trace is caught at the step that was changed.
    let mut forged = trace.clone();
    let victim = forged
        .steps
        .iter()
        .position(|s| s.outcome.accepted())
        .unwrap();
    forged.steps[victim].outcome = Outcome::Rejected {
        kind: ErrorKind::Unauthorized,
        message: "forged".into(),
        failed: vec![],
    };
    assert_eq!(
        replay(&scenario.initial, &forged).first_mismatch,
        Some(victim)
    );
}

#[test]
fn faults_change_the_path_but_not_the_result() {
    let calm = run(&scenario(3, 60, Faults::default()));
    let rough = run(&scenario(
        3,
        60,
        Faults {
            commit_conflict: 300_000,
            delivery_drop: 400_000,
        },
    ));
    assert_eq!(
        calm.1.hash(),
        rough.1.hash(),
        "retried commits and redelivered events leave the same state"
    );
    assert!(rough.0.steps.iter().any(|s| s.conflicts > 0));
    assert!(rough
        .0
        .deliveries
        .iter()
        .any(|d| d.result == DeliveryResult::Dropped));
    for trace in [&calm.0, &rough.0] {
        for invariant in &trace.invariants {
            assert!(
                invariant.holds,
                "{}: {:?}",
                invariant.id, invariant.violations
            );
        }
    }
    // Every committed event was delivered exactly once despite the drops.
    let delivered: Vec<u64> = rough
        .0
        .deliveries
        .iter()
        .filter(|d| d.tenant == tenant() && d.result == DeliveryResult::Delivered)
        .map(|d| d.seq)
        .collect();
    assert!(delivered.windows(2).all(|pair| pair[1] == pair[0] + 1));
    assert_eq!(
        *delivered.last().unwrap(),
        rough.1.tenants[&tenant()].event_seq
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
                            query: QueryId::new("unused_query").unwrap(),
                            args: Default::default(),
                            include: vec![],
                            markdown: vec![],
                            single: false,
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
    let scenario = scenario(
        5,
        40,
        Faults {
            commit_conflict: 100_000,
            delivery_drop: 100_000,
        },
    );
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

#[test]
fn requiring_the_excerpt_strictly_is_refused_because_of_existing_posts() {
    let scenario = scenario(5, 30, Faults::default());
    let projection = project(
        &scenario,
        &require_excerpt(ExistingData::Reject, 0),
        &mut |_, _, _| true,
    )
    .unwrap();
    match &projection.change_outcome {
        Outcome::Rejected { kind, failed, .. } => {
            assert_eq!(*kind, ErrorKind::MigrationRejected);
            assert!(failed
                .iter()
                .all(|c| c.rule.ends_with("required:post.excerpt")));
        }
        other => panic!("expected a rejection, got {}", other.verdict()),
    }
    let titles: Vec<&str> = projection
        .affected_existing
        .iter()
        .map(|a| a.title.as_str())
        .collect();
    assert_eq!(
        titles,
        [
            "Slow Software",
            "The Second Hundred Hours",
            "Notes Toward a Workbench"
        ]
    );
    assert!(
        projection.comparison.outcome_changes.is_empty(),
        "a refused change changes nothing downstream"
    );
    assert_eq!(projection.baseline.final_hash, projection.branch.final_hash);
}

/// Run an intent and, when the kernel accepts it, add it to the journal the way the store does.
fn record(
    world: &mut World,
    journal: &mut Vec<Recorded>,
    context: Context,
    intent: Intent,
) -> bool {
    let Ok((next, transition)) = world.step(&intent, &context) else {
        return false;
    };
    *world = next;
    journal.push(Recorded {
        revision: transition.delta.new_revision,
        kernel_version: KERNEL_VERSION.into(),
        intent,
        context,
        delta_hash: canon::hash_of(&transition.delta),
        events_hash: canon::hash_of(&transition.events),
    });
    true
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
            let scenario = scenario(seed, 80, Faults::default());
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
            diverged += usize::from(by_hand.is_some());
        }
    }
    assert!(diverged >= 2, "the change diverged only {diverged} times");
    // A change that is refused diverges nowhere, and says so.
    let refused = project(
        &scenario(5, 80, Faults::default()),
        &require_excerpt(ExistingData::Reject, 0),
        &mut |_, _, _| true,
    )
    .unwrap();
    assert_eq!(first_difference(&refused.baseline, &refused.branch), None);
    assert_eq!(refused.comparison.first_divergence, None);

    let scenario = scenario(5, 80, Faults::default());
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

#[test]
fn a_change_applied_midway_leaves_earlier_steps_alone() {
    let scenario = scenario(5, 80, Faults::default());
    let projection = project(
        &scenario,
        &require_excerpt(ExistingData::Grandfather, 40),
        &mut |_, _, _| true,
    )
    .unwrap();
    assert!(projection
        .comparison
        .outcome_changes
        .iter()
        .all(|change| change.step >= 40));
}

#[test]
fn a_projection_can_be_cancelled() {
    let scenario = scenario(5, 40, Faults::default());
    let result = project(
        &scenario,
        &require_excerpt(ExistingData::Grandfather, 0),
        &mut |phase, done, _| !(phase == 1 && done >= 10),
    );
    assert!(result.is_err());
}

/// Verifies: SIM-004
#[test]
fn a_journal_of_real_commits_replays_to_the_live_state() {
    // Build a journal the way the store does: record each accepted intent with its hashes.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tenants");
    let loaded = load_all(&root).unwrap();
    let bundle = &loaded
        .iter()
        .find(|l| l.bundle.id == tenant())
        .unwrap()
        .bundle;
    let mut world = World::default();
    let mut journal = Vec::new();
    install(bundle, &mut |context, intent| {
        let (next, transition) = world.step(&intent, &context)?;
        world = next;
        journal.push(Recorded {
            revision: transition.delta.new_revision,
            kernel_version: KERNEL_VERSION.into(),
            intent,
            context,
            delta_hash: canon::hash_of(&transition.delta),
            events_hash: canon::hash_of(&transition.events),
        });
        Ok::<_, DomainError>(transition.output)
    })
    .unwrap();
    // Then the tenant lives: a generated editorial workload and role and schema changes, all
    // recorded the way the store records them, so the journal holds every kind of intent.
    let admin = |now: i64| Context {
        tenant: tenant(),
        principal: Principal::User(PrincipalId::new("user:admin").unwrap()),
        session: None,
        now: Timestamp(now),
        correlation: format!("t-{now}"),
    };
    let role_of = |name: &str| RoleId::new(name).unwrap();
    let mut work: Vec<(Context, Intent)> =
        generate(&world, &tenant(), 17, 80, 1_800_000_000_000, 60_000)
            .into_iter()
            .map(|s| (s.context, s.intent))
            .collect();
    let mirela = PrincipalId::new("user:mirela").unwrap();
    work.push((
        admin(1_900_000_000_000),
        Intent::RevokeRole {
            principal: mirela.clone(),
            role: role_of("author"),
        },
    ));
    work.push((
        admin(1_900_000_000_001),
        Intent::AssignRole {
            principal: mirela,
            role: role_of("author"),
        },
    ));
    work.push((
        admin(1_900_000_000_002),
        Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineRole(RoleDef {
                id: role_of("reviewer"),
                label: "Reviewer".into(),
            })],
            expected_version: None,
            existing: ExistingData::Reject,
        },
    ));
    for (context, intent) in work {
        record(&mut world, &mut journal, context, intent);
    }
    // A record made and deleted again, so that a delete is in the journal whatever the workload
    // happened to do.
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
    let id = RecordId(world.tenants[&tenant()].next_record);
    assert!(record(
        &mut world,
        &mut journal,
        admin(1_900_000_000_003),
        scratch
    ));
    let delete = Intent::DeleteRecord {
        object: category,
        id,
        expected_version: None,
    };
    assert!(record(
        &mut world,
        &mut journal,
        admin(1_900_000_000_004),
        delete
    ));
    let kinds: BTreeSet<&str> = journal.iter().map(|e| e.intent.name()).collect();
    assert_eq!(
        kinds,
        BTreeSet::from([
            "create_tenant",
            "change_schema",
            "assign_role",
            "revoke_role",
            "create_record",
            "update_record",
            "delete_record"
        ]),
        "the journal does not hold every kind of intent"
    );
    let live = world.tenants[&tenant()].clone();
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

#[test]
fn the_generator_reads_the_schema_and_nothing_else() {
    // The same generator on the notes fixture of the kernel, which has no posts or authors.
    let world = strata_kernel::testkit::notes_world();
    let schedule: Vec<Scheduled> = generate(
        &world,
        &TenantId::new("alpha").unwrap(),
        2,
        40,
        50_000,
        1_000,
    );
    let accepted = schedule.iter().fold((world, 0), |(world, n), s| {
        match world.step(&s.intent, &s.context) {
            Ok((next, _)) => (next, n + 1),
            Err(_) => (world, n),
        }
    });
    assert!(
        accepted.1 >= 10,
        "only {} of 40 generated operations were accepted",
        accepted.1
    );
}
