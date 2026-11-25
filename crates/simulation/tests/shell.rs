//! The shell path: requests, jobs, outbox delivery and automation runs through the backend's own
//! code (`strata_backend::simulated`), over the in-memory store with logical time, seeded
//! randomness and the faults of the specification's failure modes, each exercised here.

use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use strata_backend::simulated::Backend;
use strata_kernel::ext::*;
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_protocol::tenants::load_all;
use strata_simulation::engine::Scheduled;
use strata_simulation::engine::{run as run_kernel, Outcome, Scenario};
use strata_simulation::faults::{StoreFault, StoreFaults};
use strata_simulation::journal::replay_journal;
use strata_simulation::shell::{
    replay_recording, run_shell, run_shell_with, Adapter, Answer, Arrival, Faults, Operation,
    ShellRun, ShellScenario, ShellTrace, Verdict,
};
use strata_simulation::workload::generate;
use strata_substrate::bundle::install_into;

/// When the generated workloads start.
const T0: i64 = 1_800_000_000_000;

fn fieldnotes() -> TenantId {
    tenant_id("fieldnotes")
}

fn tenants_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tenants")
}

/// The seeded tenants installed through the kernel alone: what the workload generator reads, and
/// what the kernel-level engine starts from.
fn kernel_seeded() -> World {
    static SEEDED: OnceLock<World> = OnceLock::new();
    SEEDED
        .get_or_init(|| {
            load_all(&tenants_dir())
                .unwrap()
                .iter()
                .fold(World::default(), |world, loaded| {
                    install_into(&world, &loaded.bundle).unwrap()
                })
        })
        .clone()
}

/// The backend, installing the repository's tenant bundles when a process starts.
fn backend() -> Backend {
    Backend::new(Some(tenants_dir()))
}

/// The generated workload of a seed and length. A test that compares a clean run with a faulted
/// one asks for the same workload twice; it is generated (each step decided by the kernel on a
/// scratch world) once.
fn workload(seed: u64, steps: usize) -> Vec<Scheduled> {
    static MADE: Mutex<BTreeMap<(u64, usize), Vec<Scheduled>>> = Mutex::new(BTreeMap::new());
    let mut made = MADE.lock().unwrap();
    made.entry((seed, steps))
        .or_insert_with(|| generate(&kernel_seeded(), &fieldnotes(), seed, steps, T0, 60_000))
        .clone()
}

/// An editorial workload on Fieldnotes, sent with idempotency keys.
fn editorial(seed: u64, steps: usize, faults: Faults) -> ShellScenario {
    ShellScenario {
        name: format!("editorial-{seed}"),
        seed,
        start: T0 - 3_600_000,
        schedule: workload(seed, steps),
        faults,
        listen: vec![fieldnotes()],
        keys: true,
        start_faults: StoreFaults::default(),
        operations: Vec::new(),
    }
}

fn store_faults(faults: StoreFaults) -> Faults {
    Faults {
        store: faults,
        ..Faults::default()
    }
}

fn run(deploy: &Backend, scenario: &ShellScenario) -> ShellRun {
    run_shell(deploy, scenario).unwrap()
}

/// Every invariant the run checks holds, and nothing was left to do at the end.
fn assert_clean(trace: &ShellTrace) {
    assert!(
        trace.violations().is_empty(),
        "{}: {:#?}",
        trace.scenario,
        trace.violations()
    );
    assert!(
        trace.quiesced,
        "{}: work was left at the end",
        trace.scenario
    );
}

/// The last answer of each request, `replayed` read as `committed`: what the client finally
/// learned.
fn outcomes(trace: &ShellTrace) -> BTreeMap<usize, String> {
    trace
        .last_answers()
        .into_iter()
        .map(|(index, answer)| {
            let verdict = match answer.verdict().as_str() {
                "replayed" => "committed".to_string(),
                other => other.to_string(),
            };
            (index, verdict)
        })
        .collect()
}

/// The run without faults that the fault tests compare with, made once for the binary: every
/// fault test below runs the same seed and schedule, so the one clean run serves them all (a run
/// starts a backend and installs the seed tenants, which is most of its cost).
fn baseline() -> &'static ShellRun {
    static CLEAN: OnceLock<ShellRun> = OnceLock::new();
    CLEAN.get_or_init(|| {
        run(
            &backend(),
            &editorial(BASELINE_SEED, BASELINE_STEPS, Faults::default()),
        )
    })
}

/// The run with the store's and the process's faults together: a commit that loses a race, one
/// that stops before its write, one whose reply is lost, an outbox that cannot be read or whose
/// acknowledgement is lost, and a process that stops. Each fault test below reads this one run
/// for its own event, so the binary pays for the schedule once and not once per fault.
fn faulted() -> &'static ShellRun {
    static FAULTED: OnceLock<ShellRun> = OnceLock::new();
    FAULTED.get_or_init(|| {
        run(
            &backend(),
            &editorial(
                BASELINE_SEED,
                BASELINE_STEPS,
                Faults {
                    store: StoreFaults {
                        commit_conflict: 250_000,
                        commit_unavailable: 200_000,
                        commit_reply_lost: 300_000,
                        publish_unavailable: 400_000,
                        publish_ack_lost: 400_000,
                        ..StoreFaults::default()
                    },
                    crash: 250_000,
                    ..Faults::default()
                },
            ),
        )
    })
}

const BASELINE_SEED: u64 = 5;
const BASELINE_STEPS: usize = 6;

fn injected(trace: &ShellTrace, fault: StoreFault) -> usize {
    trace.injected.iter().filter(|i| i.fault == fault).count()
}

#[test]
fn the_shell_path_decides_as_the_kernel_does() {
    // Without faults, the backend's request path (store contract, partial loads, segments,
    // sealing, job intake) answers every request as the kernel alone does, and leaves the same
    // tenant.
    let scenario = ShellScenario {
        keys: false,
        ..editorial(7, 4, Faults::default())
    };
    let shell = run(&backend(), &scenario);
    assert_clean(&shell.trace);
    let (kernel, world) = run_kernel(&Scenario {
        name: scenario.name.clone(),
        seed: scenario.seed,
        initial: kernel_seeded(),
        schedule: scenario.schedule.clone(),
    });
    let answers = shell.trace.last_answers();
    for step in &kernel.steps {
        let index = step.index.unwrap();
        let answer = answers[&index];
        match (&step.outcome, answer) {
            (
                Outcome::Accepted {
                    revision,
                    delta_hash,
                    ..
                },
                Answer::Committed {
                    revision: r,
                    delta_hash: d,
                    replayed: false,
                    ..
                },
            ) => {
                assert_eq!((revision, delta_hash), (r, d), "step {index}");
            }
            (Outcome::Rejected { .. }, Answer::Refused { .. }) => {}
            (outcome, answer) => panic!(
                "step {index}: the kernel says {}, the backend {}",
                outcome.verdict(),
                answer.verdict()
            ),
        }
    }
    assert!(shell
        .trace
        .steps
        .iter()
        .all(|s| s.arrival == Arrival::First));
    assert_eq!(
        shell.trace.final_hashes[&fieldnotes()],
        world.tenant_hash(&fieldnotes())
    );
    // Every committed event reached the subscriber exactly once without faults.
    let seqs: Vec<u64> = shell.trace.heard.iter().map(|h| h.event.seq).collect();
    let distinct: BTreeSet<u64> = seqs.iter().copied().collect();
    assert_eq!(seqs.len(), distinct.len());
}

/// Two runs of one faulty scenario give the same trace, and the nonces of the tenant generations
/// come from the scenario's seed (D19): the two end at the same generations, and another seed
/// (the baseline's) ends at another.
#[test]
fn the_same_seed_gives_the_same_trace_and_generations() {
    let faults = Faults {
        store: StoreFaults {
            commit_conflict: 100_000,
            commit_unavailable: 100_000,
            commit_reply_lost: 100_000,
            publish_unavailable: 200_000,
            publish_ack_lost: 200_000,
            ..StoreFaults::default()
        },
        request_repeated: 100_000,
        request_delayed: 100_000,
        crash: 100_000,
        ..Faults::default()
    };
    let scenario = editorial(11, 4, faults);
    let first = run(&backend(), &scenario);
    let second = run(&backend(), &scenario);
    assert_eq!(first.trace.hash(), second.trace.hash());
    assert_eq!(first.trace, second.trace);
    assert_clean(&first.trace);
    assert!(!first.trace.injected.is_empty());
    assert!(
        first.generations.contains_key(&fieldnotes()),
        "the run ends with the tenant's generation"
    );
    assert_eq!(first.generations, second.generations);
    assert_ne!(
        first.generations[&fieldnotes()].nonce,
        baseline().generations[&fieldnotes()].nonce,
        "another seed draws another nonce"
    );
}

#[test]
fn a_commit_that_loses_a_race_is_decided_again() {
    // fm: a store conflict. The store refuses the commit; the backend decides again on fresh
    // state and the client learns the same outcome.
    let clean = baseline();
    let raced = faulted();
    assert_clean(&raced.trace);
    assert!(injected(&raced.trace, StoreFault::CommitConflict) > 0);
    assert!(raced
        .trace
        .steps
        .iter()
        .any(|s| matches!(s.answer, Answer::Committed { conflicts, .. } if conflicts > 0)));
    assert_eq!(outcomes(&raced.trace), outcomes(&clean.trace));
    assert_eq!(raced.trace.final_hashes, clean.trace.final_hashes);
}

#[test]
fn a_process_stopped_before_its_write_commits_nothing_and_the_retry_commits_once() {
    // fm: a stopped process mid-commit (fm-database-failover, fm-disk-full as the request sees
    // them): nothing is written, the client is told the service is unavailable and sends the
    // request again with its key.
    let clean = baseline();
    let stopped = faulted();
    assert_clean(&stopped.trace);
    assert!(injected(&stopped.trace, StoreFault::CommitUnavailable) > 0);
    assert!(stopped
        .trace
        .steps
        .iter()
        .any(|s| s.arrival == Arrival::Retried));
    assert!(stopped
        .trace
        .last_answers()
        .values()
        .all(|answer| !answer.unavailable()));
    assert_eq!(outcomes(&stopped.trace), outcomes(&clean.trace));
    assert_eq!(stopped.trace.final_hashes, clean.trace.final_hashes);
}

#[test]
fn a_commit_whose_answer_is_lost_is_answered_from_its_key() {
    // fm: the process stops after the write and before it answers. The client's retry finds the
    // first result under its key; nothing is committed twice.
    let clean = baseline();
    let lost = faulted();
    assert_clean(&lost.trace);
    assert!(injected(&lost.trace, StoreFault::CommitReplyLost) > 0);
    assert!(lost
        .trace
        .steps
        .iter()
        .any(|s| s.arrival == Arrival::Retried
            && matches!(s.answer, Answer::Committed { replayed: true, .. })));
    assert_eq!(outcomes(&lost.trace), outcomes(&clean.trace));
    assert_eq!(lost.trace.final_hashes, clean.trace.final_hashes);
}

#[test]
fn a_repeated_request_commits_once() {
    // fm: a repeated message (env-cloud message-duplication).
    let clean = baseline();
    let repeated = run(
        &backend(),
        &editorial(
            BASELINE_SEED,
            4,
            Faults {
                request_repeated: 1_000_000,
                ..Faults::default()
            },
        ),
    );
    assert_clean(&repeated.trace);
    let steps = &repeated.trace.steps;
    assert_eq!(steps.len(), 2 * repeated.trace.last_answers().len());
    for pair in steps.chunks(2) {
        let (first, again) = (&pair[0], &pair[1]);
        assert_eq!(again.arrival, Arrival::Repeated);
        match (&first.answer, &again.answer) {
            (
                Answer::Committed {
                    revision,
                    replayed: false,
                    ..
                },
                Answer::Committed {
                    revision: second,
                    replayed: true,
                    ..
                },
            ) => assert_eq!(revision, second, "step {}", first.index),
            (Answer::Refused { .. }, Answer::Refused { .. }) => {
                assert_eq!(first.answer.verdict(), again.answer.verdict())
            }
            (a, b) => panic!("step {}: {} then {}", first.index, a.verdict(), b.verdict()),
        }
    }
    // The first four requests of the baseline schedule, repeated, end as they did without repeats.
    let clean_outcomes = outcomes(&clean.trace);
    for (index, verdict) in outcomes(&repeated.trace) {
        assert_eq!(clean_outcomes[&index], verdict, "request {index}");
    }
}

#[test]
fn a_delayed_request_arrives_after_later_ones() {
    // fm: a delayed message. It is decided on the state it finds when it arrives.
    let delayed = run(
        &backend(),
        &editorial(
            10,
            6,
            Faults {
                request_delayed: 300_000,
                ..Faults::default()
            },
        ),
    );
    assert_clean(&delayed.trace);
    assert!(delayed
        .trace
        .steps
        .iter()
        .any(|s| s.arrival == Arrival::Delayed));
    assert_eq!(
        delayed.trace.last_answers().len(),
        delayed.trace.steps.len()
    );
}

#[test]
fn a_process_stopped_between_commit_and_publication_publishes_on_restart() {
    // fm-runtime-crash: the process stops after a transaction commits and before its events are
    // published; the next process publishes them from the store's outbox.
    let crashed = faulted();
    assert_clean(&crashed.trace);
    assert!(!crashed.trace.restarts.is_empty());
}

#[test]
fn a_publisher_that_cannot_read_or_mark_the_outbox_publishes_everything_in_order() {
    // fm-poison-event and a lost acknowledgement: publication is refused or its mark is lost; the
    // events reach the subscriber, the first time in order, some of them more than once.
    let flaky = faulted();
    assert_clean(&flaky.trace);
    assert!(injected(&flaky.trace, StoreFault::PublishUnavailable) > 0);
    assert!(injected(&flaky.trace, StoreFault::PublishAckLost) > 0);
    let distinct: BTreeSet<u64> = flaky.trace.heard.iter().map(|h| h.event.seq).collect();
    assert!(flaky.trace.heard.len() > distinct.len());
}

#[test]
fn a_failed_installation_leaves_no_half_tenant_and_the_next_start_installs_it() {
    // fm-provisioning-step-fails: installing a tenant stops part way (the fifth commit of the
    // start fails); the tenant is left absent, not half made, and the next start, after the
    // process stopped, installs it whole.
    let tidewater = tenant_id("tidewater");
    let scenario = ShellScenario {
        name: "provisioning".into(),
        seed: 21,
        start: T0 - 3_600_000,
        schedule: workload(21, 3),
        faults: Faults {
            crash: 1_000_000,
            ..Faults::default()
        },
        listen: vec![fieldnotes(), tidewater.clone()],
        keys: true,
        start_faults: StoreFaults {
            stop_before_commit: Some(5),
            ..StoreFaults::default()
        },
        operations: Vec::new(),
    };
    let installed = run(&backend(), &scenario);
    assert_clean(&installed.trace);
    assert!(installed
        .trace
        .injected
        .iter()
        .any(|i| i.fault == StoreFault::CommitUnavailable && i.at == scenario.start));
    assert!(!installed.trace.restarts.is_empty());
    let seeded = kernel_seeded();
    for tenant in [fieldnotes(), tidewater] {
        let state = &installed.world.tenants[&tenant];
        assert_eq!(state.schema, seeded.tenants[&tenant].schema, "{tenant}");
        // Its journal starts from its creation, once: nothing of the failed attempt is left.
        let journal = &installed.recordings[&tenant].entries;
        assert_eq!(journal[0].intent.name(), "create_tenant", "{tenant}");
        assert_eq!(
            journal
                .iter()
                .filter(|e| e.intent.name() == "create_tenant")
                .count(),
            1,
            "{tenant}"
        );
    }
}

/// Verifies: fm-provisioning-step-fails
#[test]
fn an_installation_whose_creating_commit_loses_its_answer_leaves_no_half_tenant() {
    // fm-provisioning-step-fails: the commit that creates a tenant is made, but its answer is lost
    // (the first commit of each bundle, as every commit's answer is lost during the start). The
    // install fails, and a tenant is whole or absent: after the restart each tenant is whole,
    // with exactly one `create_tenant` in its journal.
    let tidewater = tenant_id("tidewater");
    let scenario = ShellScenario {
        name: "provisioning-reply-lost".into(),
        seed: 22,
        start: T0 - 3_600_000,
        schedule: workload(22, 3),
        faults: Faults {
            crash: 1_000_000,
            ..Faults::default()
        },
        listen: vec![fieldnotes(), tidewater.clone()],
        keys: true,
        start_faults: StoreFaults {
            commit_reply_lost: 1_000_000,
            ..StoreFaults::default()
        },
        operations: Vec::new(),
    };
    let installed = run(&backend(), &scenario);
    assert_clean(&installed.trace);
    assert!(injected(&installed.trace, StoreFault::CommitReplyLost) > 0);
    assert!(!installed.trace.restarts.is_empty());
    let seeded = kernel_seeded();
    for tenant in [fieldnotes(), tidewater] {
        let state = &installed.world.tenants[&tenant];
        assert_eq!(state.schema, seeded.tenants[&tenant].schema, "{tenant}");
        let journal = &installed.recordings[&tenant].entries;
        assert_eq!(journal[0].intent.name(), "create_tenant", "{tenant}");
        assert_eq!(
            journal
                .iter()
                .filter(|e| e.intent.name() == "create_tenant")
                .count(),
            1,
            "{tenant}"
        );
    }
}

/// Verifies: fm-provisioning-step-fails
#[test]
fn a_process_that_stops_right_after_the_creating_commit_leaves_no_half_tenant() {
    // fm-provisioning-step-fails: the process stops after the commit that creates a tenant and
    // before the next step of its install, and does nothing more (it cannot clean up). A tenant
    // is whole or absent: after the restart each tenant is whole, with exactly one
    // `create_tenant` in its journal.
    let tidewater = tenant_id("tidewater");
    let scenario = ShellScenario {
        name: "provisioning-stopped-after-create".into(),
        seed: 23,
        start: T0 - 3_600_000,
        schedule: workload(23, 3),
        faults: Faults {
            crash: 1_000_000,
            ..Faults::default()
        },
        listen: vec![fieldnotes(), tidewater.clone()],
        keys: true,
        start_faults: StoreFaults {
            stop_after_commit: Some(1),
            ..StoreFaults::default()
        },
        operations: Vec::new(),
    };
    let installed = run(&backend(), &scenario);
    assert_clean(&installed.trace);
    assert!(injected(&installed.trace, StoreFault::CommitReplyLost) > 0);
    assert!(!installed.trace.restarts.is_empty());
    let seeded = kernel_seeded();
    for tenant in [fieldnotes(), tidewater] {
        let state = &installed.world.tenants[&tenant];
        assert_eq!(state.schema, seeded.tenants[&tenant].schema, "{tenant}");
        let journal = &installed.recordings[&tenant].entries;
        assert_eq!(journal[0].intent.name(), "create_tenant", "{tenant}");
        assert_eq!(
            journal
                .iter()
                .filter(|e| e.intent.name() == "create_tenant")
                .count(),
            1,
            "{tenant}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Jobs and automation runs, on the kernel's notes fixture
// ---------------------------------------------------------------------------------------------

fn alpha() -> TenantId {
    tenant_id("alpha")
}

fn owner() -> PrincipalId {
    principal_id("alpha:owner")
}

fn scheduled(label: &str, principal: Principal, at: i64, intent: Intent) -> Scheduled {
    Scheduled {
        label: label.to_string(),
        context: ctx("alpha", principal, at),
        intent,
    }
}

fn step_name(name: &str) -> StepName {
    StepName::new(name).unwrap()
}

fn plain(action: AutomationStep, next: Option<&str>) -> AutomationStepDef {
    AutomationStepDef {
        action,
        next: next.map(step_name),
        retry: None,
        timeout: None,
        compensate: None,
    }
}

fn automation(id: &str, start: &str, steps: Vec<(&str, AutomationStepDef)>) -> AutomationDef {
    AutomationDef {
        state: None,
        id: WorkflowId::new(id).unwrap(),
        label: id.to_string(),
        run_as: RunAs {
            roles: BTreeSet::from([role("writer")]),
        },
        run_key: None,
        start: step_name(start),
        steps: steps.into_iter().map(|(n, s)| (step_name(n), s)).collect(),
        defined_by: None,
        defined_at: None,
        suspended_for: None,
    }
}

/// A trigger on every new note, starting `automation`.
fn on_note(automation: &str) -> SchemaChange {
    SchemaChange::DefineTrigger(TriggerDef {
        id: TriggerId::new("on_note").unwrap(),
        on: TriggerEvent::RecordCreated {
            object: object("note"),
        },
        condition: None,
        automation: WorkflowId::new(automation).unwrap(),
        paused: None,
        defined_by: None,
        defined_at: None,
    })
}

/// `award`: makes a note whose slug is the starting event's sequence number, and ends. With the
/// trigger on new notes it starts itself again: a trigger loop.
fn award() -> AutomationDef {
    automation(
        "award",
        "make",
        vec![
            (
                "make",
                plain(
                    AutomationStep::CreateRecord {
                        object: object("note"),
                        fields: BTreeMap::from([
                            (field("title"), lit("Silver")),
                            (
                                field("slug"),
                                Expr::Text(Box::new(Expr::Param("seq".into()))),
                            ),
                        ]),
                    },
                    Some("done"),
                ),
            ),
            ("done", plain(AutomationStep::End, None)),
        ],
    )
}

/// The tenant `alpha` with the notes schema and `extra` schema changes, and a writer.
fn alpha_setup(extra: Vec<SchemaChange>) -> Vec<Scheduled> {
    let mut changes = notes_schema();
    changes.extend(extra);
    vec![
        scheduled(
            "create tenant",
            Principal::System,
            T0,
            Intent::CreateTenant {
                name: "alpha".into(),
                owner: owner(),
            },
        ),
        scheduled(
            "schema",
            Principal::User(owner()),
            T0 + 1,
            Intent::ChangeSchema {
                changes,
                expected_version: None,
                existing: ExistingData::Reject,
            },
        ),
        scheduled(
            "writer",
            Principal::User(owner()),
            T0 + 2,
            Intent::AssignRole {
                principal: principal_id("alpha:writer1"),
                role: role("writer"),
            },
        ),
    ]
}

/// The writer's note `n`, a minute apart from the next.
fn user_note(n: usize) -> Scheduled {
    scheduled(
        &format!("note {n}"),
        user("alpha:writer1"),
        T0 + 60_000 * (n as i64 + 1),
        Intent::CreateRecord {
            object: object("note"),
            fields: BTreeMap::from([
                (field("title"), text(&format!("Note {n}"))),
                (field("slug"), text(&format!("note-{n}"))),
            ]),
        },
    )
}

fn notes_scenario(name: &str, seed: u64, notes: usize, faults: Faults) -> ShellScenario {
    let mut schedule = alpha_setup(vec![
        SchemaChange::DefineAutomation(award()),
        on_note("award"),
    ]);
    schedule.extend((0..notes).map(user_note));
    ShellScenario {
        name: name.into(),
        seed,
        start: T0 - 1_000,
        schedule,
        faults,
        listen: vec![alpha()],
        keys: true,
        start_faults: StoreFaults::default(),
        operations: Vec::new(),
    }
}

fn note_count(shell: &ShellRun) -> usize {
    shell.world.tenants[&alpha()]
        .records_of(&object("note"))
        .count()
}

/// No job is left waiting or leased.
fn assert_jobs_settled(trace: &ShellTrace) {
    let jobs = &trace.jobs[&alpha()];
    assert!(
        !jobs.contains_key("waiting") && !jobs.contains_key("leased"),
        "{}: {jobs:?}",
        trace.scenario
    );
}

#[test]
fn a_trigger_loop_ends_at_the_hop_limit() {
    // fm-trigger-loop: a trigger's job creates an event that starts the trigger again.
    let looped = run(
        &Backend::new(None),
        &notes_scenario("loop", 31, 1, Faults::default()),
    );
    assert_clean(&looped.trace);
    assert_jobs_settled(&looped.trace);
    let notes = note_count(&looped);
    let limit = strata_kernel::triggers::HOP_LIMIT as usize;
    assert!(
        notes > 1 && notes <= 2 + limit,
        "{notes} notes for a hop limit of {limit}"
    );
    assert!(looped.trace.jobs_run > 0);
}

#[test]
fn a_worker_that_stalls_holding_a_lease_applies_no_step_twice() {
    // fm-worker-stalls: a worker stops between running a job and reporting it, and resumes with
    // the lease it held after the job was offered again. Its late report is refused, and the step
    // key keeps the step from being applied twice.
    let clean = run(
        &Backend::new(None),
        &notes_scenario("stall-clean", 32, 2, Faults::default()),
    );
    let stalled = run(
        &Backend::new(None),
        &notes_scenario(
            "stall",
            32,
            2,
            store_faults(StoreFaults {
                report_lost: 150_000,
                ..StoreFaults::default()
            }),
        ),
    );
    assert_clean(&stalled.trace);
    assert_jobs_settled(&stalled.trace);
    assert!(!stalled.trace.late_reports.is_empty());
    assert!(stalled
        .trace
        .late_reports
        .iter()
        .any(|late| late.result.starts_with("refused")));
    assert_eq!(note_count(&stalled), note_count(&clean));
}

#[test]
fn a_worker_cut_off_from_its_tenant_database_takes_no_lease_and_resumes() {
    // fm-automation-worker-partition.
    let clean = run(
        &Backend::new(None),
        &notes_scenario("partition-clean", 33, 2, Faults::default()),
    );
    let cut = run(
        &Backend::new(None),
        &notes_scenario(
            "partition",
            33,
            2,
            store_faults(StoreFaults {
                lease_partition: 500_000,
                ..StoreFaults::default()
            }),
        ),
    );
    assert_clean(&cut.trace);
    assert_jobs_settled(&cut.trace);
    assert!(injected(&cut.trace, StoreFault::LeasePartition) > 0);
    assert_eq!(note_count(&cut), note_count(&clean));
}

/// `notify`: delivers a webhook to `subscription`, then ends.
fn notify(subscription: &str) -> AutomationDef {
    automation(
        "notify",
        "call",
        vec![
            (
                "call",
                plain(
                    AutomationStep::CallWebhook {
                        subscription: serde_json::from_value(json!(subscription)).unwrap(),
                        body: BTreeMap::from([
                            ("event".to_string(), lit("note-made")),
                            ("seq".to_string(), Expr::Param("seq".into())),
                        ]),
                    },
                    Some("done"),
                ),
            ),
            ("done", plain(AutomationStep::End, None)),
        ],
    )
}

#[test]
fn a_webhook_receiver_that_fails_or_times_out_is_retried_and_answered_once() {
    // An adapter failing or timing out: each delivery is tried again until the receiver answers,
    // and a delivery the receiver answered is never sent again.
    let mut schedule = alpha_setup(Vec::new());
    schedule.extend((0..6).map(user_note));
    let scenario = ShellScenario {
        name: "webhooks".into(),
        seed: 34,
        start: T0 - 1_000,
        schedule,
        faults: Faults {
            adapter_fails: 300_000,
            adapter_times_out: 200_000,
            ..Faults::default()
        },
        listen: vec![alpha()],
        keys: true,
        start_faults: StoreFaults::default(),
        operations: vec![Operation {
            at: T0 + 10,
            operation: "create-webhook-subscription".into(),
            tenant: alpha(),
            principal: owner(),
            body: json!({ "target": "https://hooks.example/in" }),
        }],
    };
    let shell = run_shell_with(&Backend::new(None), &scenario, &|_, answer| {
        let subscription = answer.as_ref().unwrap()["subscription"]
            .as_str()
            .unwrap()
            .to_string();
        vec![scheduled(
            "define notify",
            Principal::User(owner()),
            T0 + 20,
            Intent::ChangeSchema {
                changes: vec![
                    SchemaChange::DefineAutomation(notify(&subscription)),
                    on_note("notify"),
                ],
                expected_version: None,
                existing: ExistingData::Reject,
            },
        )]
    })
    .unwrap();
    assert_clean(&shell.trace);
    assert_jobs_settled(&shell.trace);
    assert!(shell.trace.operations[0].answer.is_ok());
    let calls: Vec<_> = shell
        .trace
        .adapter_calls
        .iter()
        .filter(|call| call.adapter == Adapter::Webhook)
        .collect();
    assert!(calls.iter().any(|call| call.verdict == Verdict::Fails));
    assert!(calls.iter().any(|call| call.verdict == Verdict::TimesOut));
    let mut answered: BTreeMap<&str, usize> = BTreeMap::new();
    for call in calls.iter().filter(|call| call.verdict == Verdict::Answers) {
        *answered.entry(call.subject.as_str()).or_default() += 1;
    }
    assert!(!answered.is_empty());
    assert!(answered.values().all(|count| *count == 1), "{answered:?}");
    assert!(calls
        .iter()
        .any(|call| call.verdict == Verdict::Answers && call.attempt > 0));
}

// ---------------------------------------------------------------------------------------------
// Checked against production: the journal and recorded workloads
// ---------------------------------------------------------------------------------------------

#[test]
fn a_recorded_workload_replays_to_its_recorded_outcomes() {
    // A deployment's run is recorded as the store journalled it, and replayed through a fresh
    // deployment's request path: every entry is answered with its recorded revision, state delta
    // and events, and the tenant left is the recorded one.
    let original = baseline();
    assert_clean(&original.trace);
    let recording = &original.recordings[&fieldnotes()];
    assert!(recording.entries.len() > 10);
    let replay = replay_recording(&Backend::new(None), recording, 99).unwrap();
    assert_eq!(
        (replay.agreed, replay.entries),
        (recording.entries.len(), recording.entries.len())
    );
    assert!(replay.first_mismatch.is_none() && replay.matches);

    // A recording that does not match what the deployment does is caught where it differs.
    let late = recording.entries.len() - 3;
    let mut forged = recording.clone();
    forged.entries[late].delta_hash = "0".repeat(64);
    let found = replay_recording(&Backend::new(None), &forged, 99).unwrap();
    assert_eq!(
        found.first_mismatch.map(|m| m.0),
        Some(forged.entries[late].revision.0)
    );
    let mut drifted = recording.clone();
    drifted.final_hash = "0".repeat(64);
    assert!(
        !replay_recording(&Backend::new(None), &drifted, 99)
            .unwrap()
            .matches
    );

    // The kernel-level replay of the same journal agrees with it too.
    let live = original.world.tenants[&fieldnotes()].clone();
    let kernel = replay_journal(&fieldnotes(), &recording.entries, Some(&live));
    assert!(kernel.matches_live && kernel.agreed == kernel.entries);
}
