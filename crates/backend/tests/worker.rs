//! The automation worker: jobs run once, leases expire and are fenced, failing steps retry and
//! dead-letter, waits resume, a run acts with its run-as roles only, and one tenant's queue does
//! not hold back another's. Driven synchronously through `worker::tick` with an injected clock,
//! on the in-memory store, with the kernel's notes fixture.

use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use strata_backend::app::{App, Config, Shared};
use strata_backend::auth::Identity;
use strata_backend::mail::DevSink;
use strata_backend::worker::{self, routes, steps, WorkerConfig};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_model::prelude::DurationMs;
use strata_model::types::TemplateId;
use strata_protocol::ws::SessionState;
use strata_storage_api::jobs::{ExceptionKind, JobState, JobWork, NewJob, RetryPolicy, RunState};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, SessionRecord, Store};

/// The time the tests start at.
const T0: i64 = 1_000_000;

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

/// A note titled `title` whose slug is the text of the starting event's sequence number.
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

fn automation(
    id: &str,
    run_as: &str,
    start: &str,
    steps: Vec<(&str, AutomationStepDef)>,
) -> AutomationDef {
    AutomationDef {
        id: WorkflowId::new(id).unwrap(),
        label: id.to_string(),
        run_as: RunAs {
            roles: BTreeSet::from([role(run_as)]),
        },
        run_key: None,
        start: name(start),
        steps: steps.into_iter().map(|(n, s)| (name(n), s)).collect(),
        defined_by: None,
        defined_at: None,
        suspended_for: None,
        state: None,
    }
}

/// `award`: makes a note and ends.
fn award() -> AutomationDef {
    automation(
        "award",
        "writer",
        "make",
        vec![
            ("make", step(make_note("Silver"), Some("done"))),
            ("done", step(AutomationStep::End, None)),
        ],
    )
}

/// `drip`: waits a minute, then makes a note.
fn drip() -> AutomationDef {
    drip_titled("Later")
}

/// `drip` whose note is titled `title`.
fn drip_titled(title: &str) -> AutomationDef {
    automation(
        "drip",
        "writer",
        "pause",
        vec![
            (
                "pause",
                step(
                    AutomationStep::WaitFor {
                        duration: DurationMs(60_000),
                    },
                    Some("mark"),
                ),
            ),
            ("mark", step(make_note(title), Some("done"))),
            ("done", step(AutomationStep::End, None)),
        ],
    )
}

/// An automation that makes a note and then publishes it, run as `run_as`; publishing needs the
/// editor role. One attempt per step.
fn promote(id: &str, run_as: &str) -> AutomationDef {
    let mut publish = step(
        AutomationStep::UpdateRecord {
            object: object("note"),
            record: Expr::Param("make_record".into()),
            fields: BTreeMap::from([(field("status"), lit("published"))]),
        },
        Some("done"),
    );
    publish.retry = Some(StepRetry {
        attempts: 1,
        first_delay: None,
        max_delay: None,
    });
    automation(
        id,
        run_as,
        "make",
        vec![
            ("make", step(make_note("Promoted"), Some("publish"))),
            ("publish", publish),
            ("done", step(AutomationStep::End, None)),
        ],
    )
}

/// `broken`: updates a note that does not exist, two attempts a tenth of a second apart.
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
        attempts: 2,
        first_delay: Some(DurationMs(100)),
        max_delay: Some(DurationMs(1_000)),
    });
    automation("broken", "writer", "bad", vec![("bad", bad)])
}

/// `mailer`: sends mail, run as a writer, who does not hold messages-send.
fn mailer() -> AutomationDef {
    automation(
        "mailer",
        "writer",
        "send",
        vec![(
            "send",
            step(
                AutomationStep::SendMail {
                    template: TemplateId("welcome".into()),
                    to: Some(lit("person@example.org")),
                    address: None,
                    segment: None,
                    category: None,
                },
                None,
            ),
        )],
    )
}

/// A tenant with the notes schema, its people, and the automations of these tests.
fn install(app: &App, tenant: &str) {
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
    // The template the mailer names: a definition naming a template the tenant does not define
    // is refused.
    changes.push(SchemaChange::DefineMailTemplate(MailTemplateDecl {
        template: TemplateId("welcome".into()),
        subject: "Welcome".into(),
        body: StructuredBlocks("[]".into()),
    }));
    for def in [
        award(),
        drip(),
        promote("promote", "writer"),
        promote("promote_ed", "editor"),
        broken(),
        mailer(),
    ] {
        changes.push(SchemaChange::DefineAutomation(def));
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

/// Start a run of `automation` for the event of sequence number `seq`, due at `due`.
fn start(app: &App, tenant: &str, automation: &str, seq: i64, due: i64) -> String {
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
            run_key: None,
        },
        due_at: Timestamp(due),
        policy: RetryPolicy::default(),
    };
    app.store
        .jobs_create(&tenant_id(tenant), &[job], Timestamp(due))
        .unwrap()
        .remove(0)
}

/// Every note of a tenant.
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

/// The run a start job started: its id is the job's.
fn run_of(app: &App, tenant: &str, job: &str) -> strata_storage_api::jobs::RunRecord {
    app.store
        .automation_run(&tenant_id(tenant), job)
        .unwrap()
        .expect("the run was started")
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
        correlation: "t-worker".into(),
        verified_address: None,
        idempotency_key: None,
        agent: None,
    }
}

// Verifies: SCH-012
#[test]
fn a_job_runs_once() {
    let app = app();
    install(&app, "alpha");
    let job = start(&app, "alpha", "award", 7, T0);
    let pass = worker::tick(&app, "w1", &config(), &at(T0));
    assert_eq!(pass.tenants, 1);
    assert_eq!(pass.jobs, 3, "the start, the note and the end");
    let run = run_of(&app, "alpha", &job);
    assert_eq!(run.state, RunState::Completed);
    assert_eq!(notes(&app, "alpha").len(), 1);
    // Nothing is due any more: another pass runs nothing and makes nothing.
    let again = worker::tick(&app, "w1", &config(), &at(T0 + 3_600_000));
    assert_eq!(again.jobs, 0);
    assert_eq!(notes(&app, "alpha").len(), 1);
    let jobs = app.store.jobs_list(&tenant_id("alpha"), None, 10).unwrap();
    assert_eq!(jobs.len(), 3);
    assert!(jobs
        .iter()
        .all(|j| j.state == JobState::Completed && j.attempt == 1));
    let steps = app
        .store
        .automation_steps(&tenant_id("alpha"), &job, None, 10)
        .unwrap();
    assert_eq!(steps.len(), 2);
}

// Verifies: SCH-001
#[test]
fn a_job_is_not_offered_before_its_time() {
    let app = app();
    install(&app, "alpha");
    let job = start(&app, "alpha", "award", 7, T0);
    let early = worker::tick(&app, "w1", &config(), &at(T0 - 1));
    assert_eq!(early, worker::Pass::default());
    assert!(app
        .store
        .automation_run(&tenant_id("alpha"), &job)
        .unwrap()
        .is_none());
    assert_eq!(worker::tick(&app, "w1", &config(), &at(T0)).jobs, 3);
}

// Verifies: SCH-013, SCH-014, SCH-1525, SCH-1532
#[test]
fn a_failing_step_is_retried_then_dead_lettered() {
    let app = app();
    install(&app, "alpha");
    let alpha = tenant_id("alpha");
    let job = start(&app, "alpha", "broken", 7, T0);
    assert_eq!(worker::tick(&app, "w1", &config(), &at(T0)).jobs, 2);
    assert_eq!(run_of(&app, "alpha", &job).state, RunState::Running);
    // The retry is due after the first delay, not before.
    assert_eq!(worker::tick(&app, "w1", &config(), &at(T0 + 99)).jobs, 0);
    assert_eq!(worker::tick(&app, "w1", &config(), &at(T0 + 100)).jobs, 1);
    let run = run_of(&app, "alpha", &job);
    assert_eq!(run.state, RunState::Failed);
    assert!(run.error.is_some());
    let step = &app.store.automation_steps(&alpha, &job, None, 10).unwrap()[0];
    assert_eq!(
        step.errors.len(),
        2,
        "the error of each attempt: {:?}",
        step.errors
    );
    let failed = app
        .store
        .jobs_list(&alpha, None, 10)
        .unwrap()
        .into_iter()
        .find(|j| matches!(j.work, JobWork::Step { .. }))
        .unwrap();
    assert_eq!(failed.state, JobState::Failed);
    assert_eq!(failed.reason.as_deref(), Some("attempts-exhausted"));
    // The owner lists it as dead-lettered; a writer may not list it.
    let listed = routes::list_exceptions(
        &app,
        &signed_in("alpha", "owner"),
        &json!({ "unsettled": true }),
    )
    .unwrap();
    assert_eq!(listed["exceptions"].as_array().unwrap().len(), 1);
    assert_eq!(listed["exceptions"][0]["kind"], "dead-lettered");
    assert_eq!(listed["exceptions"][0]["reason"], "attempts-exhausted");
    assert!(routes::list_exceptions(&app, &signed_in("alpha", "writer1"), &json!({})).is_err());
    // Releasing it gives the step another attempt with the same key.
    let id = listed["exceptions"][0]["exception"]
        .as_str()
        .unwrap()
        .to_string();
    let settled = routes::settle_exception(
        &app,
        &signed_in("alpha", "owner"),
        &json!({ "exception": id, "action": "release" }),
    )
    .unwrap();
    assert_eq!(settled["job"], failed.id);
    assert_eq!(
        app.store.job(&alpha, &failed.id).unwrap().unwrap().state,
        JobState::Waiting
    );
    let unsettled = app
        .store
        .automation_exceptions(&alpha, Some(ExceptionKind::DeadLettered), true, None, 10)
        .unwrap();
    assert!(unsettled.is_empty());
}

// Verifies: WFL-1537
#[test]
fn a_wait_ends_its_job_and_resumes_when_due() {
    let app = app();
    install(&app, "alpha");
    let job = start(&app, "alpha", "drip", 7, T0);
    assert_eq!(worker::tick(&app, "w1", &config(), &at(T0)).jobs, 2);
    let run = run_of(&app, "alpha", &job);
    assert_eq!(run.state, RunState::Waiting);
    assert_eq!(run.step, Some(name("mark")));
    assert!(notes(&app, "alpha").is_empty());
    // Nothing holds a lease while it waits, and nothing is due before its time.
    assert!(app
        .store
        .jobs_list(&tenant_id("alpha"), None, 10)
        .unwrap()
        .iter()
        .all(|j| j.state != JobState::Leased));
    assert_eq!(
        worker::tick(&app, "w1", &config(), &at(T0 + 59_999)).tenants,
        0
    );
    // A worker that started after the wait resumes it.
    assert_eq!(
        worker::tick(&app, "w9", &config(), &at(T0 + 60_000)).jobs,
        2
    );
    assert_eq!(run_of(&app, "alpha", &job).state, RunState::Completed);
    assert_eq!(notes(&app, "alpha").len(), 1);
}

// Verifies: WFL-1527, WFL-1530
#[test]
fn a_run_acts_with_its_run_as_roles_only() {
    let app = app();
    install(&app, "alpha");
    // Run as a writer: the note is made, as the automation principal, but not published.
    let writer = start(&app, "alpha", "promote", 7, T0);
    worker::tick(&app, "w1", &config(), &at(T0));
    let run = run_of(&app, "alpha", &writer);
    assert_eq!(run.state, RunState::Failed, "{run:?}");
    assert_eq!(run.caused_by.as_str(), "automation:promote");
    let made = notes(&app, "alpha");
    assert_eq!(made.len(), 1);
    assert_eq!(
        made[0].created_by.as_ref().map(|p| p.as_str()),
        Some("automation:promote")
    );
    assert_eq!(made[0].get(&field("status")), &text("draft"));
    // Run as an editor: the same steps publish.
    let editor = start(&app, "alpha", "promote_ed", 8, T0);
    worker::tick(&app, "w1", &config(), &at(T0));
    assert_eq!(run_of(&app, "alpha", &editor).state, RunState::Completed);
    let published = notes(&app, "alpha")
        .into_iter()
        .filter(|n| n.get(&field("status")) == &text("published"))
        .count();
    assert_eq!(published, 1);
}

// Verifies: SCH-007, SCH-009
//
// The bound: a due job of one tenant runs in the first pass after it is due, whatever another
// tenant has queued, as long as no more than `WorkerConfig::tenants` tenants are due; each pass
// runs at most `WorkerConfig::budget` jobs of one tenant.
#[test]
fn one_tenants_flood_does_not_hold_back_another_tenants_job() {
    let app = app();
    install(&app, "alpha");
    install(&app, "beta");
    for seq in 1..=200 {
        start(&app, "alpha", "award", seq, T0);
    }
    let quiet = start(&app, "beta", "award", 1, T0 + 1);
    let config = config();
    let pass = worker::tick(&app, "w1", &config, &at(T0 + 1));
    assert_eq!(pass.tenants, 2);
    assert!(pass.jobs <= 2 * config.budget, "{pass:?}");
    assert_eq!(
        app.store
            .job(&tenant_id("beta"), &quiet)
            .unwrap()
            .unwrap()
            .state,
        JobState::Completed,
        "beta's job ran in the first pass"
    );
    assert!(app
        .store
        .jobs_list(&tenant_id("alpha"), None, 300)
        .unwrap()
        .iter()
        .any(|j| j.state == JobState::Waiting));
    // A tenant with nothing due is not named by the wake index at all.
    install(&app, "gamma");
    assert!(!app
        .store
        .jobs_due(Timestamp(T0 + 1), 10)
        .unwrap()
        .contains(&tenant_id("gamma")));
}

// Verifies: WFL-1531
#[test]
fn the_owner_lists_runs_and_their_steps() {
    let app = app();
    install(&app, "alpha");
    let job = start(&app, "alpha", "award", 7, T0);
    worker::tick(&app, "w1", &config(), &at(T0));
    let owner = signed_in("alpha", "owner");
    let runs = routes::list_runs(&app, &owner, &json!({ "automation": "award" })).unwrap();
    assert_eq!(runs["runs"][0]["run"], job);
    assert_eq!(runs["runs"][0]["state"], "completed");
    assert_eq!(runs["runs"][0]["caused_by"], "automation:award");
    let steps = routes::list_steps(&app, &owner, &json!({ "run": job })).unwrap();
    assert_eq!(steps["steps"].as_array().unwrap().len(), 2);
    assert_eq!(steps["steps"][0]["state"], "completed");
    assert!(routes::list_steps(&app, &owner, &json!({ "run": "nope" })).is_err());
    assert!(routes::list_runs(&app, &owner, &json!({ "tenant": "beta" })).is_err());
    assert!(routes::list_runs(&app, &signed_in("alpha", "writer1"), &json!({})).is_err());
}

// Verifies: MIG-562
#[test]
fn a_run_finishes_under_the_definition_version_that_started_it() {
    let app = app();
    install(&app, "alpha");
    let alpha = tenant_id("alpha");
    let before = app
        .store
        .load_complete(&alpha)
        .unwrap()
        .tenant
        .unwrap()
        .schema
        .version;
    // The first run starts and waits a minute before its note.
    let old = start(&app, "alpha", "drip", 7, T0);
    worker::tick(&app, "w1", &config(), &at(T0));
    let waiting = run_of(&app, "alpha", &old);
    assert_eq!(waiting.state, RunState::Waiting);
    assert_eq!(waiting.definition_version, Some(before));
    // The owner redefines the automation while the run waits: a newer version is in force.
    let owner = Principal::User(principal_id("alpha:owner"));
    execute(
        app.store.as_ref(),
        &Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineAutomation(drip_titled("Newer"))],
            expected_version: None,
            existing: ExistingData::Reject,
        },
        &ctx("alpha", owner, 1_000),
    )
    .expect("the redefinition");
    let after = app
        .store
        .load_complete(&alpha)
        .unwrap()
        .tenant
        .unwrap()
        .schema
        .version;
    assert_ne!(after, before);
    // A run started now reads the new definition; the one in flight finishes under its own.
    let new = start(&app, "alpha", "drip", 8, T0 + 1);
    worker::tick(&app, "w1", &config(), &at(T0 + 1));
    assert_eq!(run_of(&app, "alpha", &new).definition_version, Some(after));
    worker::tick(&app, "w1", &config(), &at(T0 + 60_001));
    assert_eq!(run_of(&app, "alpha", &old).state, RunState::Completed);
    assert_eq!(run_of(&app, "alpha", &new).state, RunState::Completed);
    let titled = |slug: &str| {
        notes(&app, "alpha")
            .into_iter()
            .find(|note| note.get(&field("slug")) == &text(slug))
            .map(|note| note.get(&field("title")).clone())
    };
    assert_eq!(
        titled("7"),
        Some(text("Later")),
        "the run in flight kept its definition"
    );
    assert_eq!(
        titled("8"),
        Some(text("Newer")),
        "a run started later reads the new one"
    );
}

// A definition that names a step kind or a trigger event outside the published catalogue (INT-153)
// is not a definition: the catalogue is the typed unions, so such a definition does not decode
// and so can reach neither define-automation nor activate-workflow, which re-defines a draft
// that is already in the schema.
// Verifies: INT-153
#[test]
fn a_step_outside_the_catalogue_does_not_decode_into_a_definition() {
    let known = step(make_note("Silver"), Some("done"));
    let wire = serde_json::to_value(&known).expect("a step encodes");
    assert_eq!(
        serde_json::from_value::<AutomationStepDef>(wire.clone()).expect("a catalogue step"),
        known
    );
    let mut outside = wire;
    outside["action"] = json!({ "run-script": { "source": "1 + 1" } });
    assert!(serde_json::from_value::<AutomationStepDef>(outside).is_err());
}
