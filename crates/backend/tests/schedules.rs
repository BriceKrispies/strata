//! Schedules, run keys, the step key of a looped step, the retention of a step's idempotency
//! entry, and the owner's jobs routes (WG-5155, WG-5157, WG-5158, WG-5159, WG-5163, WG-5164).
//!
//! Driven synchronously through `worker::tick` with an injected clock on the in-memory store, with
//! the kernel's notes fixture, like `worker.rs`. The recurrence is checked against dates worked out
//! by hand, across the changes of daylight saving time in New York; the store-level rules (the
//! next job in the transaction that completes the last, the retention of an entry, the purge) are
//! also run against both adapters by `strata_storage_api::conformance`.

use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use strata_backend::app::{App, Config, Shared};
use strata_backend::auth::Identity;
use strata_backend::mail::DevSink;
use strata_backend::worker::recurrence::{utc_ms, Recurrence};
use strata_backend::worker::{self, leases, routes, schedules, steps, WorkerConfig};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_model::prelude::DurationMs;
use strata_protocol::api::ErrorCode;
use strata_protocol::ws::SessionState;
use strata_storage_api::jobs::{
    ExceptionKind, JobReport, JobState, JobWork, NewJob, Reported, RetryPolicy, RunState,
};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, SessionRecord, Store};

/// The schedule the tests define.
const SCHEDULE: &str = "0b1f6c3e-52a4-4c55-9a77-0d9a4c7e6b10";

fn app() -> Shared {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cfg = Config {
        addr: "127.0.0.1:0".parse().unwrap(),
        service_token: "test-service-token-0123456789abcdef".into(),
        dev: true,
        tenants_dir: root.join("tenants"),
        lean_dir: root.join("lean"),
        session_ttl_ms: 3_600_000,
        session_idle_ms: 3_600_000,
        throttle: Default::default(),
        passkey_challenge_ttl_ms: 300_000,
        link_ttl_ms: 900_000,
        mail: Arc::new(DevSink::memory()),
        federation: Arc::new(strata_backend::signin::oidc::Federation::none()),
        factors: Default::default(),
        limits: Default::default(),
    };
    App::new(cfg, Arc::new(MemoryStore::new()))
}

/// Make the clock of the backend read `t` (milliseconds since the epoch) from now on.
fn set_clock(app: &App, t: i64) {
    let real = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    app.clock_skew_ms.store(t - real, Ordering::Relaxed);
}

fn name(s: &str) -> StepName {
    StepName::new(s).unwrap()
}

fn step(action: AutomationStep, next: Option<&str>) -> AutomationStepDef {
    AutomationStepDef {
        action,
        next: next.map(name),
        retry: None,
        timeout: None,
        compensate: None,
    }
}

/// A note titled and slugged with the text of the sequence number of the starting event.
fn make_note(title: &str) -> AutomationStep {
    AutomationStep::CreateRecord {
        object: object("note"),
        fields: BTreeMap::from([
            (field("title"), lit(title)),
            (
                field("slug"),
                Expr::Text(Box::new(Expr::Param("seq".into()))),
            ),
        ]),
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
        start: name(start),
        steps: steps.into_iter().map(|(n, s)| (name(n), s)).collect(),
        defined_by: None,
        defined_at: None,
        suspended_for: None,
    }
}

/// `award`: makes a note and ends.
fn award() -> AutomationDef {
    automation(
        "award",
        "make",
        vec![
            ("make", step(make_note("Silver"), Some("done"))),
            ("done", step(AutomationStep::End, None)),
        ],
    )
}

/// `broken`: updates a note that does not exist, once.
fn broken() -> AutomationDef {
    let mut bad = step(
        AutomationStep::UpdateRecord {
            object: object("note"),
            record: Expr::Lit(Value::Int(999_999)),
            fields: BTreeMap::from([(field("title"), lit("Never"))]),
        },
        None,
    );
    bad.retry = Some(StepRetry {
        attempts: 1,
        first_delay: Some(DurationMs(100)),
        max_delay: Some(DurationMs(1_000)),
    });
    automation("broken", "bad", vec![("bad", bad)])
}

/// `twice`: makes a note named by the time its job was made, waits a minute, and goes back to
/// make another while its job was made before T0 + 30 seconds: the step `bump` is visited twice.
fn twice() -> AutomationDef {
    let by_time = || Expr::Text(Box::new(Expr::Now));
    let bump = AutomationStep::CreateRecord {
        object: object("note"),
        fields: BTreeMap::from([(field("title"), by_time()), (field("slug"), by_time())]),
    };
    let check = AutomationStep::Branch {
        condition: Expr::Cmp(
            CmpOp::Lt,
            Box::new(Expr::Now),
            Box::new(Expr::Lit(Value::Time(T0 + 30_000))),
        ),
        object: None,
        record: None,
        then: name("bump"),
        r#else: name("done"),
    };
    automation(
        "twice",
        "bump",
        vec![
            ("bump", step(bump, Some("pause"))),
            (
                "pause",
                step(
                    AutomationStep::WaitFor {
                        duration: DurationMs(60_000),
                    },
                    Some("check"),
                ),
            ),
            ("check", step(check, None)),
            ("done", step(AutomationStep::End, None)),
        ],
    )
}

/// A trigger of `automation` on the schedule falling due.
fn on_schedule(id: &str, automation: &str, schedule: &str) -> TriggerDef {
    TriggerDef {
        id: TriggerId::new(id).unwrap(),
        on: TriggerEvent::ScheduleDue {
            schedule: serde_json::from_value(json!(schedule)).unwrap(),
        },
        condition: None,
        automation: WorkflowId::new(automation).unwrap(),
        paused: None,
        defined_by: None,
        defined_at: None,
    }
}

/// A tenant with the notes schema, its people, the automations of these tests and `triggers`.
fn install(app: &App, tenant: &str, triggers: Vec<TriggerDef>) {
    let store = app.store.as_ref();
    let owner = principal_id(&format!("{tenant}:owner"));
    let mut now = 100;
    let mut run = |principal: Principal, intent: Intent| {
        now += 1;
        execute(store, &intent, &ctx(tenant, principal, now)).expect("fixture intent");
    };
    run(
        Principal::System,
        Intent::CreateTenant {
            name: tenant.to_string(),
            owner: owner.clone(),
        },
    );
    let mut changes = notes_schema();
    for def in [award(), broken(), twice()] {
        changes.push(SchemaChange::DefineAutomation(def));
    }
    for trigger in triggers {
        changes.push(SchemaChange::DefineTrigger(trigger));
    }
    run(
        Principal::User(owner.clone()),
        Intent::ChangeSchema {
            changes,
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    run(
        Principal::User(owner),
        Intent::AssignRole {
            principal: principal_id(&format!("{tenant}:writer1")),
            role: role("writer"),
        },
    );
}

/// Start a run of `automation` for the event of sequence number `seq`, due at `due`, with the run
/// key `run_key` when it has one.
fn start(
    app: &App,
    tenant: &str,
    automation: &str,
    seq: i64,
    due: i64,
    run_key: Option<&str>,
) -> String {
    let job = NewJob {
        key: format!("start:test:{automation}:{seq}"),
        work: JobWork::StartRun {
            trigger: None,
            automation: WorkflowId::new(automation).unwrap(),
            event_seq: Some(seq as u64),
            hop: 0,
            params: BTreeMap::from([("seq".to_string(), Value::Int(seq))]),
            caused_by: None,
            on_behalf_of: None,
            definition_version: None,
            run_key: run_key.map(str::to_string),
        },
        due_at: Timestamp(due),
        policy: RetryPolicy::default(),
    };
    app.store
        .jobs_create(&tenant_id(tenant), &[job], Timestamp(due))
        .unwrap()
        .remove(0)
}

fn notes(app: &App, tenant: &str) -> Vec<Arc<Record>> {
    let state = app.store.load_complete(&tenant_id(tenant)).unwrap();
    state
        .tenant
        .map(|t| t.records_of(&object("note")).cloned().collect())
        .unwrap_or_default()
}

fn at(t: i64) -> impl Fn() -> Timestamp {
    move || Timestamp(t)
}

fn config() -> WorkerConfig {
    WorkerConfig::from_limits(&Default::default())
}

fn signed_in(tenant: &str, who: &str) -> Identity {
    let principal = principal_id(&format!("{tenant}:{who}"));
    Identity {
        tenant: tenant_id(tenant),
        principal: Principal::User(principal.clone()),
        session: Some(SessionRecord {
            id: SessionId::new(format!("s-{who}")).unwrap(),
            token_hash: format!("hash-{who}"),
            csrf_token: "csrf".into(),
            tenant: tenant_id(tenant),
            principal,
            display_name: who.to_string(),
            created_at: Timestamp(0),
            last_seen_at: Timestamp(0),
            expires_at: Timestamp(i64::MAX),
            revoked_at: None,
            authenticated_at: Timestamp(0),
            network: None,
        }),
        state: SessionState::Active,
        correlation: "t-schedules".into(),
        verified_address: None,
        idempotency_key: None,
        agent: None,
    }
}

/// The time zone database and the subset of RFC 5545: the occurrences of rules, as UTC.
fn occurrences(rule: &str, tenant_zone: &str, anchor: i64, count: usize) -> Vec<i64> {
    let recurrence = Recurrence::parse(rule, tenant_zone).expect("a rule of the subset");
    let mut found = Vec::new();
    let mut after = anchor - 1;
    while found.len() < count {
        match recurrence.next_after(anchor, after) {
            Some(next) => {
                found.push(next);
                after = next;
            }
            None => break,
        }
    }
    found
}

// Verifies: SCH-1573, SCH-1574
#[test]
fn a_daily_rule_reads_the_time_that_occurs_twice_at_its_first_occurrence() {
    // New York put its clocks back at 02:00 EDT on Sunday 1 November 2026: 01:30 happens twice,
    // and is read as the first, 01:30 EDT (UTC-4), 05:30 UTC. The day before is also EDT (05:30
    // UTC) and the day after is EST (UTC-5), 06:30 UTC.
    let got = occurrences(
        "FREQ=DAILY;BYHOUR=1;BYMINUTE=30;TZID=America/New_York",
        "UTC",
        utc_ms(2026, 10, 30, 12, 0),
        3,
    );
    assert_eq!(
        got,
        vec![
            utc_ms(2026, 10, 31, 5, 30),
            utc_ms(2026, 11, 1, 5, 30),
            utc_ms(2026, 11, 2, 6, 30),
        ]
    );
}

// Verifies: SCH-005, SCH-2704
#[test]
fn a_paused_schedule_creates_no_job_and_runs_no_trigger_and_resuming_creates_one() {
    let app = app();
    install(
        &app,
        "alpha",
        vec![on_schedule("nightly", "award", SCHEDULE)],
    );
    let alpha = tenant_id("alpha");
    let owner = signed_in("alpha", "owner");
    set_clock(&app, utc_ms(2026, 6, 1, 10, 30));
    schedules::define_schedule(
        &app,
        &owner,
        &json!({ "schedule": SCHEDULE, "rule": "FREQ=HOURLY;TZID=UTC" }),
    )
    .unwrap();
    // Paused while its job is open.
    schedules::define_schedule(
        &app,
        &owner,
        &json!({ "schedule": SCHEDULE, "rule": "FREQ=HOURLY;TZID=UTC", "paused": true }),
    )
    .unwrap();
    let due = utc_ms(2026, 6, 1, 11, 30);
    set_clock(&app, due);
    assert_eq!(worker::tick(&app, "w1", &config(), &at(due)).jobs, 1);
    assert!(
        notes(&app, "alpha").is_empty(),
        "a paused schedule runs no trigger"
    );
    let jobs = app.store.jobs_list(&alpha, None, 50).unwrap();
    assert_eq!(jobs.len(), 1, "a paused schedule creates no job");
    assert_eq!(jobs[0].state, JobState::Completed);
    // A day later it is still paused and still has no job.
    assert_eq!(
        worker::tick(&app, "w1", &config(), &at(due + 86_400_000)).jobs,
        0
    );
    // Resumed with no job open: the next job is created, at the next occurrence after now.
    set_clock(&app, utc_ms(2026, 6, 2, 8, 10));
    let resumed = schedules::define_schedule(
        &app,
        &owner,
        &json!({ "schedule": SCHEDULE, "rule": "FREQ=HOURLY;TZID=UTC", "paused": false }),
    )
    .unwrap();
    assert_eq!(resumed["next_due"], utc_ms(2026, 6, 2, 8, 30));
    assert_eq!(app.store.jobs_list(&alpha, None, 50).unwrap().len(), 2);
    // And it runs the trigger when it falls due.
    let again = utc_ms(2026, 6, 2, 8, 30);
    set_clock(&app, again);
    worker::tick(&app, "w1", &config(), &at(again));
    assert_eq!(notes(&app, "alpha").len(), 1);
}

// Verifies: SCH-003, SCH-010, SCH-011
#[test]
fn exactly_one_next_job_when_the_worker_stops_after_running_and_before_reporting() {
    let app = app();
    install(&app, "alpha", vec![]);
    let alpha = tenant_id("alpha");
    let owner = signed_in("alpha", "owner");
    set_clock(&app, utc_ms(2026, 6, 1, 10, 30));
    schedules::define_schedule(
        &app,
        &owner,
        &json!({ "schedule": SCHEDULE, "rule": "FREQ=HOURLY;TZID=UTC" }),
    )
    .unwrap();
    let due = utc_ms(2026, 6, 1, 11, 30);
    // w1 takes the job, runs it and stops before it reports.
    let held = app
        .store
        .job_lease(&alpha, "w1", Timestamp(due))
        .unwrap()
        .unwrap();
    let outcome = steps::run_job(&app, &alpha, &held, Timestamp(due));
    assert_eq!(outcome.report, JobReport::Succeeded);
    assert_eq!(app.store.jobs_list(&alpha, None, 50).unwrap().len(), 1);
    // w2 takes it over once the lease has expired, and its report creates the next job.
    let later = due + 60_001;
    assert_eq!(worker::tick(&app, "w2", &config(), &at(later)).jobs, 1);
    assert_eq!(app.store.jobs_list(&alpha, None, 50).unwrap().len(), 2);
    // w1's report arrives late and is refused: no second next job.
    let late = app
        .store
        .job_report(
            &alpha,
            &held.job.id,
            held.token,
            &outcome.report,
            &outcome.writes,
            Timestamp(later + 1),
        )
        .unwrap();
    assert!(matches!(late, Reported::Refused(_)), "{late:?}");
    let jobs = app.store.jobs_list(&alpha, None, 50).unwrap();
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs.iter().filter(|job| !job.state.ended()).count(), 1);
}

// Verifies: SCH-1536
#[test]
fn a_repeated_run_key_starts_no_run_and_writes_a_declined_record() {
    let app = app();
    install(&app, "alpha", vec![]);
    let alpha = tenant_id("alpha");
    let first = start(&app, "alpha", "award", 1, T0, Some("key-of-the-person"));
    start(&app, "alpha", "award", 2, T0, Some("key-of-the-person"));
    start(&app, "alpha", "award", 3, T0, Some("key-of-another"));
    worker::tick(&app, "w1", &config(), &at(T0));
    let runs = app
        .store
        .automation_runs(&alpha, None, None, None, 10)
        .unwrap();
    assert_eq!(runs.len(), 2, "one run for each value of the key");
    assert!(runs.iter().any(|run| run.run == first));
    assert_eq!(notes(&app, "alpha").len(), 2);
    let declined = app
        .store
        .automation_exceptions(&alpha, Some(ExceptionKind::Declined), false, None, 10)
        .unwrap();
    assert_eq!(declined.len(), 1);
    assert_eq!(declined[0].reason, "run-key-repeated");
    assert_eq!(declined[0].event_seq, Some(2));
    // A later event with the key is declined too, however long after.
    start(
        &app,
        "alpha",
        "award",
        4,
        T0 + 86_400_000,
        Some("key-of-the-person"),
    );
    worker::tick(&app, "w1", &config(), &at(T0 + 86_400_000));
    assert_eq!(
        app.store
            .automation_runs(&alpha, None, None, None, 10)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        app.store
            .automation_exceptions(&alpha, Some(ExceptionKind::Declined), false, None, 10)
            .unwrap()
            .len(),
        2
    );
}

// Verifies: SCH-1534, SCH-2705
#[test]
fn the_runs_of_one_run_key_start_in_the_order_of_their_events() {
    let app = app();
    install(&app, "alpha", vec![]);
    let alpha = tenant_id("alpha");
    // The later event (5) is due first; the earlier event (3) is due ten milliseconds after.
    let later = start(&app, "alpha", "award", 5, T0, Some("k"));
    let earlier = start(&app, "alpha", "award", 3, T0 + 10, Some("k"));
    // At T0 the later event's start waits for the earlier one: no run starts.
    worker::tick(&app, "w1", &config(), &at(T0));
    assert!(app.store.automation_run(&alpha, &later).unwrap().is_none());
    assert!(app
        .store
        .automation_run(&alpha, &earlier)
        .unwrap()
        .is_none());
    // The earlier event starts its run; the later one then finds the key held and is declined.
    worker::tick(&app, "w1", &config(), &at(T0 + 10));
    assert!(app
        .store
        .automation_run(&alpha, &earlier)
        .unwrap()
        .is_some());
    worker::tick(&app, "w1", &config(), &at(T0 + 10_000));
    assert!(app.store.automation_run(&alpha, &later).unwrap().is_none());
    let declined = app
        .store
        .automation_exceptions(&alpha, Some(ExceptionKind::Declined), false, None, 10)
        .unwrap();
    assert_eq!(declined.len(), 1);
    assert_eq!(declined[0].event_seq, Some(5));
}

/// The time the tests start at.
const T0: i64 = 1_000_000;

// Verifies: SCH-1524, SCH-2700
#[test]
fn a_looped_step_has_a_key_for_each_visit_and_so_runs_each_visit_once() {
    let app = app();
    install(&app, "alpha", vec![]);
    let alpha = tenant_id("alpha");
    let run = start(&app, "alpha", "twice", 1, T0, None);
    worker::tick(&app, "w1", &config(), &at(T0));
    assert_eq!(notes(&app, "alpha").len(), 1, "the first visit of bump");
    worker::tick(&app, "w1", &config(), &at(T0 + 60_000));
    assert_eq!(notes(&app, "alpha").len(), 2, "the second visit of bump");
    worker::tick(&app, "w1", &config(), &at(T0 + 120_000));
    assert_eq!(
        app.store
            .automation_run(&alpha, &run)
            .unwrap()
            .unwrap()
            .state,
        RunState::Completed
    );
    assert_eq!(
        notes(&app, "alpha").len(),
        2,
        "each visit made its note once"
    );
    let recorded = app.store.automation_steps(&alpha, &run, None, 20).unwrap();
    let bumps: Vec<_> = recorded
        .iter()
        .filter(|record| record.step.as_str() == "bump")
        .collect();
    assert_eq!(bumps.len(), 2);
    assert_ne!(bumps[0].idempotency_key, bumps[1].idempotency_key);
    // A retry of one visit has its key: the same sequence gives the same key, another does not.
    let automation = WorkflowId::new("twice").unwrap();
    let key = |sequence| steps::step_key(&alpha, &automation, &run, &name("bump"), sequence);
    assert_eq!(key(1), key(1));
    assert_ne!(key(1), key(4));
    assert_eq!(bumps[0].idempotency_key, key(bumps[0].sequence));
    assert_eq!(bumps[1].idempotency_key, key(bumps[1].sequence));
}

// Verifies: SCH-015, SCH-1524, SCH-2701
#[test]
fn a_step_retried_a_day_after_a_lost_report_makes_no_second_record() {
    let app = app();
    install(&app, "alpha", vec![]);
    let alpha = tenant_id("alpha");
    set_clock(&app, T0);
    let run = start(&app, "alpha", "award", 7, T0, None);
    let first = app
        .store
        .job_lease(&alpha, "w1", Timestamp(T0))
        .unwrap()
        .unwrap();
    leases::run_leased(&app, &alpha, &first, &at(T0)).unwrap();
    // w1 takes the step that makes the note, makes it, and its report is lost.
    let held = app
        .store
        .job_lease(&alpha, "w1", Timestamp(T0))
        .unwrap()
        .unwrap();
    let outcome = steps::run_job(&app, &alpha, &held, Timestamp(T0));
    assert_eq!(outcome.report, JobReport::Succeeded);
    assert_eq!(notes(&app, "alpha").len(), 1);
    // Twenty-five hours later (a step may wait that long between attempts: its delays reach a
    // day) the step is attempted again, with the same key: the note is not made again.
    let day_and_an_hour = 25 * 3_600_000;
    set_clock(&app, T0 + day_and_an_hour);
    let pass = worker::tick(&app, "w2", &config(), &at(T0 + day_and_an_hour));
    assert_eq!(pass.jobs, 2, "the step again, then the end");
    assert_eq!(notes(&app, "alpha").len(), 1, "no second note");
    assert_eq!(
        app.store
            .automation_run(&alpha, &run)
            .unwrap()
            .unwrap()
            .state,
        RunState::Completed
    );
}

// Verifies: SCH-006, AGT-416, AGT-417, AGT-418
#[test]
fn an_owner_lists_cancels_and_retries_jobs_and_another_tenant_or_a_writer_may_not() {
    let app = app();
    install(&app, "alpha", vec![]);
    install(&app, "beta", vec![]);
    let alpha = tenant_id("alpha");
    let owner = signed_in("alpha", "owner");
    let writer = signed_in("alpha", "writer1");
    let beta_owner = signed_in("beta", "owner");
    // A job to cancel, and a job that fails.
    let waiting = start(&app, "alpha", "award", 1, T0 + 86_400_000, None);
    let failing = start(&app, "alpha", "broken", 2, T0, None);
    worker::tick(&app, "w1", &config(), &at(T0));
    let failed = app
        .store
        .jobs_list(&alpha, None, 10)
        .unwrap()
        .into_iter()
        .find(|job| matches!(job.work, JobWork::Step { .. }) && job.state == JobState::Failed)
        .expect("the broken step failed");

    // Listing needs jobs-read; the owner sees the state, the attempts and the errors.
    let listed = routes::list_jobs(&app, &owner, &json!({})).unwrap();
    let jobs = listed["jobs"].as_array().unwrap();
    assert!(jobs
        .iter()
        .any(|job| job["job"] == waiting && job["state"] == "waiting"));
    let seen = jobs.iter().find(|job| job["job"] == failed.id).unwrap();
    assert_eq!(seen["state"], "failed");
    assert_eq!(seen["attempts"], 1);
    assert_eq!(seen["errors"].as_array().unwrap().len(), 1);
    assert_eq!(
        routes::list_jobs(&app, &writer, &json!({}))
            .unwrap_err()
            .0
            .code,
        ErrorCode::Forbidden
    );
    assert_eq!(
        routes::list_jobs(&app, &owner, &json!({ "tenant": "beta" }))
            .unwrap_err()
            .0
            .code,
        ErrorCode::Forbidden
    );

    // Cancelling needs jobs-manage and a job of the caller's own tenant.
    let cancel = |who: &Identity, job: &str| {
        routes::cancel_job(&app, who, &json!({ "job": job, "reason": "not wanted" }))
    };
    assert_eq!(
        cancel(&writer, &waiting).unwrap_err().0.code,
        ErrorCode::Forbidden
    );
    assert_eq!(
        cancel(&beta_owner, &waiting).unwrap_err().0.code,
        ErrorCode::NotFound
    );
    assert_eq!(
        app.store.job(&alpha, &waiting).unwrap().unwrap().state,
        JobState::Waiting
    );
    assert_eq!(cancel(&owner, &waiting).unwrap()["state"], "cancelled");
    let cancelled = app.store.job(&alpha, &waiting).unwrap().unwrap();
    assert_eq!(cancelled.state, JobState::Cancelled);
    let why = cancelled.reason.unwrap();
    assert!(
        why.contains("alpha:owner") && why.contains("not wanted"),
        "{why}"
    );
    // Cancelled for good: nothing runs at its time.
    assert_eq!(
        worker::tick(&app, "w1", &config(), &at(T0 + 86_400_000)).jobs,
        0
    );

    // Retrying needs the same; a job that has not failed is a conflict.
    let retry = |who: &Identity, job: &str| routes::retry_job(&app, who, &json!({ "job": job }));
    assert_eq!(
        retry(&writer, &failed.id).unwrap_err().0.code,
        ErrorCode::Forbidden
    );
    assert_eq!(
        retry(&beta_owner, &failed.id).unwrap_err().0.code,
        ErrorCode::NotFound
    );
    assert_eq!(
        retry(&owner, &waiting).unwrap_err().0.code,
        ErrorCode::Conflict
    );
    let retried = retry(&owner, &failed.id).unwrap();
    assert_eq!(retried["attempt"], 2);
    assert_eq!(
        app.store.job(&alpha, &failed.id).unwrap().unwrap().state,
        JobState::Waiting
    );
    let _ = failing;
}

// Verifies: SCH-004, SCH-015, SCH-2706
#[test]
fn schedule_job_starts_a_run_once_for_its_key_and_refuses_other_kinds() {
    let app = app();
    install(&app, "alpha", vec![]);
    let alpha = tenant_id("alpha");
    let owner = signed_in("alpha", "owner");
    let request = |key: &str, automation: &str| {
        json!({
            "kind": "start-run",
            "due_at": T0,
            "idempotency_key": key,
            "payload": { "automation": automation, "params": { "seq": 41 } },
        })
    };
    let first = routes::schedule_job(&app, &owner, &request("k-1", "award")).unwrap();
    let again = routes::schedule_job(&app, &owner, &request("k-1", "award")).unwrap();
    assert_eq!(first["job"], again["job"], "one key, one job");
    assert_eq!(app.store.jobs_list(&alpha, None, 10).unwrap().len(), 1);
    assert_eq!(
        routes::schedule_job(&app, &owner, &request("k-1", "broken"))
            .unwrap_err()
            .0
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        routes::schedule_job(&app, &owner, &request("k-2", "nosuch"))
            .unwrap_err()
            .0
            .code,
        ErrorCode::NotFound
    );
    let mut other_kind = request("k-3", "award");
    other_kind["kind"] = json!("send-mail");
    assert_eq!(
        routes::schedule_job(&app, &owner, &other_kind)
            .unwrap_err()
            .0
            .code,
        ErrorCode::Invalid
    );
    assert_eq!(
        routes::schedule_job(
            &app,
            &signed_in("alpha", "writer1"),
            &request("k-4", "award")
        )
        .unwrap_err()
        .0
        .code,
        ErrorCode::Forbidden
    );
    // It runs as any job does: a run, hop zero, the note.
    worker::tick(&app, "w1", &config(), &at(T0));
    let id = first["job"].as_str().unwrap();
    let run = app.store.automation_run(&alpha, id).unwrap().unwrap();
    assert_eq!(run.hop, 0);
    assert_eq!(run.state, RunState::Completed);
    assert_eq!(notes(&app, "alpha").len(), 1);
}
