//! Determinism, branch equivalence, divergence discovery and minimisation, on the real
//! Fieldnotes tenant installed from its bundle.

use std::path::PathBuf;
use strata_kernel::*;
use strata_protocol::tenants::load_all;
use strata_simulation::engine::{
    replay, run, DeliveryResult, Faults, Outcome, Scenario, Scheduled,
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
            },
        }],
        existing,
        before_step,
    }
}

/// Verifies: SIM-001
#[test]
fn the_same_scenario_gives_the_same_trace() {
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

/// Verifies: SIM-002
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
    // Redefining a query exactly as it is: a schema change with no behavioural content.
    let same_query = scenario.initial.tenants[&tenant()]
        .schema
        .queries
        .values()
        .next()
        .unwrap()
        .clone();
    let proposal = Proposal {
        changes: vec![SchemaChange::DefineQuery(same_query)],
        ..require_excerpt(ExistingData::Reject, 0)
    };
    let projection = project(&scenario, &proposal, &mut |_, _, _| true).unwrap();
    assert!(projection.change_outcome.accepted());
    let c = &projection.comparison;
    assert!(
        c.outcome_changes.is_empty() && c.record_changes.is_empty() && c.query_changes.is_empty()
    );
    assert!(c.events_only_in_baseline.is_empty() && c.events_only_in_branch.is_empty());
    assert_eq!(c.first_divergence, None);
    assert!(projection.counterexample.is_none());
    assert!(c.queries_compared > 0);
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

/// Verifies: SIM-003
#[test]
fn grandfathering_the_requirement_diverges_and_the_divergence_is_explained() {
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
    // So is a journal entry whose recorded hash does not match what the kernel does.
    journal[5].delta_hash = "0".repeat(64);
    assert_eq!(
        replay_journal(&tenant(), &journal, Some(&live))
            .first_mismatch
            .map(|m| m.0),
        Some(6)
    );
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
