//! Approval steps, compensation, the step that invokes a tenant operation, and the direct start
//! and activation of an automation (WG-5003, WG-5153, WG-5154, WG-5156): an approval stops a run
//! until a principal the policy names decides, the person who started the run may not, the
//! deadline takes the refused branch and a decision is taken once; a failed step undoes the
//! completed steps in reverse order, once however often it is retried, and a compensation that
//! fails stays visible and retryable; an operation is invoked once as the automation principal and
//! never beyond its definition; a draft starts no run. Driven synchronously through
//! `worker::tick` and the `_at` forms of the commands with an injected clock, on the in-memory
//! store, with the kernel's notes fixture.

use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use strata_backend::app::{ApiError, App, Config, Shared};
use strata_backend::auth::Identity;
use strata_backend::mail::DevSink;
use strata_backend::worker::{commands, steps, WorkerConfig};
use strata_kernel::approvals::WORKFLOW_APPROVE;
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_model::prelude::DurationMs;
use strata_model::types::AutomationStepResult;
use strata_protocol::api::ErrorCode;
use strata_protocol::ws::SessionState;
use strata_storage_api::jobs::{
    ExceptionKind, JobWork, NewJob, RetryPolicy, RunRecord, RunState, StepState,
};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, SessionRecord, Store};

/// The time the tests start at.
const T0: i64 = 1_000_000;

/// How long an approval waits in these tests: one hour.
const HOUR: u64 = 3_600_000;

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

// ---------------------------------------------------------------------------------------------
// Fixtures: the automations of these tests.
// ---------------------------------------------------------------------------------------------

fn name(s: &str) -> StepName {
    StepName::new(s).unwrap()
}

fn param(s: &str) -> Expr {
    Expr::Param(s.to_string())
}

fn once() -> Option<StepRetry> {
    Some(StepRetry {
        attempts: 1,
        first_delay: None,
        max_delay: None,
    })
}

fn step(action: AutomationStep, next: Option<&str>) -> AutomationStepDef {
    AutomationStepDef {
        action,
        next: next.map(name),
        retry: once(),
        timeout: None,
        compensate: None,
    }
}

/// A step that is attempted up to three times, so that an attempt lost to a crash is attempted again.
fn retried(mut def: AutomationStepDef) -> AutomationStepDef {
    def.retry = Some(StepRetry {
        attempts: 3,
        first_delay: Some(DurationMs(100)),
        max_delay: Some(DurationMs(1_000)),
    });
    def
}

fn compensated_by(mut def: AutomationStepDef, undo: &str) -> AutomationStepDef {
    def.compensate = Some(name(undo));
    def
}

fn automation(
    id: &str,
    run_as: &str,
    start: &str,
    state: Option<AutomationState>,
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
        state,
    }
}

/// A note with the literal title and slug.
fn make(title: &str, slug: &str) -> AutomationStep {
    AutomationStep::CreateRecord {
        object: object("note"),
        fields: BTreeMap::from([(field("title"), lit(title)), (field("slug"), lit(slug))]),
    }
}

fn delete_of(record: Expr) -> AutomationStep {
    AutomationStep::DeleteRecord {
        object: object("note"),
        record,
    }
}

fn update_of(record: Expr, set: &str, to: &str) -> AutomationStep {
    AutomationStep::UpdateRecord {
        object: object("note"),
        record,
        fields: BTreeMap::from([(field(set), lit(to))]),
    }
}

/// A content review (CMS-156, D-6592): the note named by the input `subject` is published when an
/// editor approves, and marked rejected when one refuses or nobody decides within the hour.
fn review() -> AutomationDef {
    automation(
        "review",
        "editor",
        "gate",
        None,
        vec![
            (
                "gate",
                step(
                    AutomationStep::RequestApproval {
                        policy: PolicyId::new("review").unwrap(),
                        timeout: DurationMs(HOUR),
                        approved: name("publish"),
                        refused: name("reject"),
                    },
                    None,
                ),
            ),
            (
                "publish",
                step(
                    update_of(param("subject"), "status", "published"),
                    Some("done"),
                ),
            ),
            (
                "reject",
                step(
                    update_of(param("subject"), "body", "rejected"),
                    Some("done"),
                ),
            ),
            ("done", step(AutomationStep::End, None)),
        ],
    )
}

/// An approval started by a trigger, with nothing after it.
fn gated() -> AutomationDef {
    automation(
        "gated",
        "editor",
        "gate",
        None,
        vec![
            (
                "gate",
                step(
                    AutomationStep::RequestApproval {
                        policy: PolicyId::new("review").unwrap(),
                        timeout: DurationMs(HOUR),
                        approved: name("done"),
                        refused: name("done"),
                    },
                    None,
                ),
            ),
            ("done", step(AutomationStep::End, None)),
        ],
    )
}

/// Makes two notes, then fails; undoes them in reverse order.
fn saga() -> AutomationDef {
    automation(
        "saga",
        "writer",
        "one",
        None,
        vec![
            (
                "one",
                compensated_by(step(make("One", "one"), Some("two")), "undo_one"),
            ),
            (
                "two",
                compensated_by(step(make("Two", "two"), Some("three")), "undo_two"),
            ),
            (
                "three",
                step(
                    AutomationStep::UpdateRecord {
                        object: object("note"),
                        record: Expr::Lit(Value::Int(999_999)),
                        fields: BTreeMap::from([(field("title"), lit("Never"))]),
                    },
                    None,
                ),
            ),
            (
                "undo_one",
                retried(step(delete_of(param("one_record")), None)),
            ),
            (
                "undo_two",
                retried(step(delete_of(param("two_record")), None)),
            ),
        ],
    )
}

/// Makes a note, then fails; its compensation cannot succeed.
fn saga_stuck() -> AutomationDef {
    automation(
        "saga_stuck",
        "writer",
        "one",
        None,
        vec![
            (
                "one",
                compensated_by(step(make("Stuck", "stuck"), Some("two")), "undo_one"),
            ),
            (
                "two",
                step(
                    update_of(Expr::Lit(Value::Int(999_999)), "title", "Never"),
                    None,
                ),
            ),
            (
                "undo_one",
                step(
                    update_of(Expr::Lit(Value::Int(999_998)), "title", "Never"),
                    None,
                ),
            ),
        ],
    )
}

/// A tenant operation that makes a note with the slug it is given.
fn stamp_note() -> TenantOperationDef {
    TenantOperationDef {
        id: TenantOperationId::new("stamp_note").unwrap(),
        label: "Stamp".into(),
        inputs: vec![TenantOperationInput {
            name: "slug".into(),
            ty: FieldType::Str,
            required: true,
            label: None,
        }],
        invokers: TenantOperationInvokers {
            signed_in: true,
            roles: BTreeSet::new(),
        },
        subject: None,
        conditions: vec![],
        steps: vec![TenantOperationStep::CreateRecord {
            id: "made".into(),
            object: object("note"),
            fields: BTreeMap::from([
                (field("title"), lit("Stamped")),
                (field("slug"), param("slug")),
            ]),
        }],
    }
}

/// [`stamp_note`] that only editors invoke.
fn stamp_editors() -> TenantOperationDef {
    let mut op = stamp_note();
    op.id = TenantOperationId::new("stamp_editors").unwrap();
    op.invokers = TenantOperationInvokers {
        signed_in: false,
        roles: BTreeSet::from([role("editor")]),
    };
    op
}

fn invoking(id: &str, run_as: &str, operation: &str, inputs: &[(&str, Expr)]) -> AutomationDef {
    automation(
        id,
        run_as,
        "call",
        None,
        vec![
            (
                "call",
                retried(step(
                    AutomationStep::InvokeOperation {
                        operation: TenantOperationId::new(operation).unwrap(),
                        inputs: inputs
                            .iter()
                            .map(|(name, expr)| (name.to_string(), expr.clone()))
                            .collect(),
                    },
                    Some("done"),
                )),
            ),
            ("done", step(AutomationStep::End, None)),
        ],
    )
}

fn drafty() -> AutomationDef {
    automation(
        "drafty",
        "writer",
        "done",
        Some(AutomationState::Draft),
        vec![("done", step(AutomationStep::End, None))],
    )
}

/// A tenant with the notes schema, the approval model and the automations of these tests. Who
/// holds what: editors and writers hold workflow-approve and jobs-run; the policy `review` names
/// the editors. `editor`, `editor2` are editors; `writer1`, `writer2` are writers; `owner` owns.
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
    let powers = vec![
        Permission::Capability(WORKFLOW_APPROVE.to_string()),
        Permission::Capability("jobs-run".to_string()),
    ];
    for who in ["editor", "writer"] {
        changes.push(SchemaChange::DefineRolePermissions {
            role: role(who),
            permissions: powers.clone(),
        });
    }
    changes.push(SchemaChange::DefinePolicy(PolicyDef {
        id: PolicyId::new("review").unwrap(),
        role: role("editor"),
        resource: Resource::Object(object("note")),
        actions: BTreeSet::from([Action::Read]),
        condition: None,
    }));
    changes.push(SchemaChange::DefineOperation(stamp_note()));
    changes.push(SchemaChange::DefineOperation(stamp_editors()));
    for def in [
        review(),
        gated(),
        saga(),
        saga_stuck(),
        drafty(),
        invoking(
            "stamper",
            "writer",
            "stamp_note",
            &[("slug", lit("stamped"))],
        ),
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
    for (who, held) in [
        ("editor", "editor"),
        ("editor2", "editor"),
        ("writer1", "writer"),
        ("writer2", "writer"),
    ] {
        run(
            Principal::User(owner.clone()),
            Intent::AssignRole {
                principal: principal_id(&format!("{tenant}:{who}")),
                role: role(held),
            },
        );
    }
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
        correlation: "t-approvals".into(),
        verified_address: None,
        idempotency_key: None,
    }
}

fn at(t: i64) -> impl Fn() -> Timestamp {
    move || Timestamp(t)
}

fn config() -> WorkerConfig {
    WorkerConfig::from_limits(&Default::default())
}

/// Every note of a tenant, in id order.
fn notes(app: &App, tenant: &str) -> Vec<Arc<Record>> {
    let state = app.store.load_complete(&tenant_id(tenant)).unwrap();
    state
        .tenant
        .map(|t| t.records_of(&object("note")).cloned().collect())
        .unwrap_or_default()
}

fn status_of(note: &Record) -> Value {
    note.fields
        .get(&field("status"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// A note written by `who` (a draft), returning its id.
fn write_note(app: &App, who: &str, title: &str, slug: &str, now: i64) -> u64 {
    execute(
        app.store.as_ref(),
        &new_note(title, slug),
        &ctx("alpha", user(&format!("alpha:{who}")), now),
    )
    .expect("a note");
    notes(app, "alpha")
        .iter()
        .find(|note| note.fields.get(&field("slug")) == Some(&text(slug)))
        .expect("the note")
        .id
        .0
}

fn run_of(app: &App, run: &str) -> RunRecord {
    app.store
        .automation_run(&tenant_id("alpha"), run)
        .unwrap()
        .expect("the run was started")
}

/// Start the content review of `note` as `who` at `now`, and let the worker take it to its
/// approval step. Returns the run.
fn submit(app: &App, who: &str, note: u64, key: &str, now: i64) -> String {
    let reply = commands::start_run_at(
        app,
        &signed_in("alpha", who),
        &json!({ "workflow": "review", "input": { "subject": note }, "idempotency_key": key }),
        Timestamp(now),
    )
    .expect("a start");
    let run = reply["run"].as_str().expect("a run id").to_string();
    worker::tick(app, "w1", &config(), &at(now));
    run
}

fn decide(
    app: &App,
    who: &str,
    run: &str,
    approved: bool,
    now: i64,
) -> Result<serde_json::Value, ApiError> {
    commands::approve_step_at(
        app,
        &signed_in("alpha", who),
        &json!({ "run": run, "approved": approved, "comment": "reviewed" }),
        Timestamp(now),
    )
}

fn code_of(error: &ApiError) -> ErrorCode {
    error.0.code
}

fn steps_of(app: &App, run: &str) -> Vec<strata_storage_api::jobs::StepRecord> {
    app.store
        .automation_steps(&tenant_id("alpha"), run, None, 50)
        .unwrap()
}

/// A start job for an automation of the tenant, due at `due`, started by no person.
fn start_job(app: &App, automation: &str, key: &str, due: i64) -> String {
    let job = NewJob {
        key: format!("start:test:{automation}:{key}"),
        work: JobWork::StartRun {
            trigger: None,
            automation: WorkflowId::new(automation).unwrap(),
            event_seq: None,
            hop: 0,
            params: BTreeMap::new(),
            caused_by: None,
            on_behalf_of: None,
            definition_version: None,
            run_key: None,
        },
        due_at: Timestamp(due),
        policy: RetryPolicy::default(),
    };
    app.store
        .jobs_create(&tenant_id("alpha"), &[job], Timestamp(due))
        .unwrap()
        .remove(0)
}

// ---------------------------------------------------------------------------------------------
// Approval.
// ---------------------------------------------------------------------------------------------

// Verifies: WFL-006, WFL-2680, CMS-156
#[test]
fn a_content_review_publishes_the_note_only_when_an_editor_approves() {
    let app = app();
    install(&app, "alpha");
    let note = write_note(&app, "writer1", "Review me", "review-me", 200);
    // The author submits the note for review: the run stops at the approval step.
    let run = submit(&app, "writer1", note, "k-1", T0);
    let waiting = run_of(&app, &run);
    assert_eq!(waiting.state, RunState::AwaitingApproval);
    assert_eq!(waiting.started_by, Some(principal_id("alpha:writer1")));
    let records = steps_of(&app, &run);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].state, StepState::Waiting);
    let approval = records[0].approval.clone().expect("the approval");
    assert_eq!(approval.roles, ["editor"]);
    assert_eq!(approval.deadline, Timestamp(T0 + HOUR as i64));
    assert_eq!(approval.started_by, Some(principal_id("alpha:writer1")));
    assert_eq!(approval.decision, None);
    assert_eq!(
        status_of(&notes(&app, "alpha")[0]),
        text("draft"),
        "nothing happens while it waits"
    );
    // Nothing continues the run but a decision: the worker finds no job due before the deadline.
    assert_eq!(worker::tick(&app, "w1", &config(), &at(T0 + 1)).jobs, 0);
    // An editor approves with a comment; the run goes down the approved branch.
    let reply = decide(&app, "editor", &run, true, T0 + 10).expect("approved");
    assert_eq!(reply["state"], "running");
    worker::tick(&app, "w1", &config(), &at(T0 + 10));
    assert_eq!(run_of(&app, &run).state, RunState::Completed);
    assert_eq!(status_of(&notes(&app, "alpha")[0]), text("published"));
    let gate = steps_of(&app, &run).remove(0);
    assert_eq!(gate.state, StepState::Completed);
    let decision = gate
        .approval
        .expect("the approval")
        .decision
        .expect("the decision");
    assert_eq!(decision.by, Some(principal_id("alpha:editor")));
    assert_eq!(decision.comment.as_deref(), Some("reviewed"));
    assert!(decision.approved && !decision.timed_out);
    // The typed result of the step is the variant of the same name, its outputs are parameters.
    match serde_json::from_value(gate.result.expect("a result")).expect("a typed result") {
        AutomationStepResult::RequestApproval { approved, by } => {
            assert!(approved);
            assert_eq!(by, Some(Some(principal_id("alpha:editor"))));
        }
        other => panic!("{other:?}"),
    }
    let finished = run_of(&app, &run);
    assert_eq!(finished.params["gate_approved"], Value::Bool(true));
    assert_eq!(finished.params["gate_by"], text("alpha:editor"));
}

// Verifies: WFL-2682
#[test]
fn a_decision_before_the_deadline_cancels_it_so_nothing_is_taken_twice() {
    let app = app();
    install(&app, "alpha");
    let note = write_note(&app, "writer1", "Review me", "review-me", 200);
    let run = submit(&app, "writer1", note, "k-1", T0);
    decide(&app, "editor", &run, true, T0 + 10).expect("approved");
    worker::tick(&app, "w1", &config(), &at(T0 + 10));
    // The hour passes: the deadline job was cancelled, and nothing changes.
    assert_eq!(
        worker::tick(&app, "w1", &config(), &at(T0 + HOUR as i64 + 1)).jobs,
        0
    );
    assert_eq!(status_of(&notes(&app, "alpha")[0]), text("published"));
    assert_eq!(run_of(&app, &run).state, RunState::Completed);
}

// Verifies: WFL-2683
#[test]
fn a_second_decision_is_refused_and_changes_nothing() {
    let app = app();
    install(&app, "alpha");
    let note = write_note(&app, "writer1", "Review me", "review-me", 200);
    let run = submit(&app, "writer1", note, "k-1", T0);
    decide(&app, "editor", &run, true, T0 + 10).expect("approved");
    let before = steps_of(&app, &run);
    // Another editor refuses a moment later, before the worker has run the next step.
    let error = decide(&app, "editor2", &run, false, T0 + 11).expect_err("decided already");
    assert_eq!(code_of(&error), ErrorCode::Conflict);
    // The same editor again, after the run went on.
    worker::tick(&app, "w1", &config(), &at(T0 + 12));
    let error = decide(&app, "editor", &run, true, T0 + 13).expect_err("decided already");
    assert_eq!(code_of(&error), ErrorCode::Conflict);
    let after = steps_of(&app, &run);
    assert_eq!(after[0].approval, before[0].approval, "the decision stands");
    assert_eq!(status_of(&notes(&app, "alpha")[0]), text("published"));
    assert_eq!(run_of(&app, &run).state, RunState::Completed);
}

// ---------------------------------------------------------------------------------------------
// Starting a run and activating an automation.
// ---------------------------------------------------------------------------------------------

// Verifies: WFL-2687
#[test]
fn a_person_starts_a_run_once_per_key_and_the_same_key_with_other_inputs_is_refused() {
    let app = app();
    install(&app, "alpha");
    let note = write_note(&app, "writer1", "Review me", "review-me", 200);
    let writer = signed_in("alpha", "writer1");
    let body =
        json!({ "workflow": "review", "input": { "subject": note }, "idempotency_key": "k-1" });
    let first = commands::start_run_at(&app, &writer, &body, Timestamp(T0)).expect("started");
    let again = commands::start_run_at(&app, &writer, &body, Timestamp(T0 + 5)).expect("repeated");
    assert_eq!(first["run"], again["run"]);
    assert_eq!(first["replayed"], false);
    assert_eq!(again["replayed"], true);
    assert_eq!(
        app.store
            .jobs_list(&tenant_id("alpha"), None, 10)
            .unwrap()
            .len(),
        1
    );
    let other =
        json!({ "workflow": "review", "input": { "subject": note + 1 }, "idempotency_key": "k-1" });
    let error =
        commands::start_run_at(&app, &writer, &other, Timestamp(T0 + 6)).expect_err("other input");
    assert_eq!(code_of(&error), ErrorCode::Conflict);
    // Another person with the same key starts another run.
    let editor = signed_in("alpha", "editor");
    let theirs = commands::start_run_at(&app, &editor, &body, Timestamp(T0 + 7)).expect("started");
    assert_ne!(theirs["run"], first["run"]);
    // The worker starts the run once.
    worker::tick(&app, "w1", &config(), &at(T0 + 10));
    let runs = app
        .store
        .automation_runs(&tenant_id("alpha"), None, None, None, 10)
        .unwrap();
    assert_eq!(runs.len(), 2);
}

// Verifies: WFL-1521, WFL-2684
#[test]
fn a_draft_starts_no_run_until_the_owner_activates_it() {
    let app = app();
    install(&app, "alpha");
    let alpha = tenant_id("alpha");
    // A start job that was created before: the worker declines it with automation-inactive.
    let job = start_job(&app, "drafty", "early", T0);
    worker::tick(&app, "w1", &config(), &at(T0));
    assert!(app.store.automation_run(&alpha, &job).unwrap().is_none());
    let declined = app
        .store
        .automation_exceptions(&alpha, Some(ExceptionKind::Declined), false, None, 10)
        .unwrap();
    assert_eq!(declined.len(), 1);
    assert_eq!(declined[0].reason, "automation-inactive");
    // Activation: refused to a writer, refused without a projection, accepted for the owner.
    let activation =
        json!({ "workflow": "drafty", "projection": { "diverged": false, "summary": [] } });
    let error = commands::activate_at(
        &app,
        &signed_in("alpha", "writer1"),
        &activation,
        Timestamp(T0 + 1),
    )
    .expect_err("not the owner");
    assert_eq!(code_of(&error), ErrorCode::Forbidden);
    let owner = signed_in("alpha", "owner");
    let error = commands::activate_at(
        &app,
        &owner,
        &json!({ "workflow": "drafty" }),
        Timestamp(T0 + 1),
    )
    .expect_err("no projection");
    assert_eq!(code_of(&error), ErrorCode::Invalid);
    let reply =
        commands::activate_at(&app, &owner, &activation, Timestamp(T0 + 2)).expect("activated");
    assert!(reply["version"].as_u64().unwrap() > 0);
    // Active now: activating again changes nothing, and a person may start it.
    let again =
        commands::activate_at(&app, &owner, &activation, Timestamp(T0 + 3)).expect("already");
    assert_eq!(again["version"], reply["version"]);
    let started = commands::start_run_at(
        &app,
        &signed_in("alpha", "writer1"),
        &json!({ "workflow": "drafty" }),
        Timestamp(T0 + 4),
    )
    .expect("started");
    worker::tick(&app, "w1", &config(), &at(T0 + 4));
    let run = run_of(&app, started["run"].as_str().unwrap());
    assert_eq!(run.state, RunState::Completed);
    assert_eq!(run.started_by, Some(principal_id("alpha:writer1")));
}

// ---------------------------------------------------------------------------------------------
// Compensation.
// ---------------------------------------------------------------------------------------------

/// The compensation jobs of a tenant in the order they were created: the compensating step and the
/// position of the step it undoes.
fn compensations(app: &App) -> Vec<(String, u64)> {
    let mut jobs = app.store.jobs_list(&tenant_id("alpha"), None, 50).unwrap();
    jobs.sort_by_key(|job| job.seq);
    jobs.into_iter()
        .filter_map(|job| match job.work {
            JobWork::Compensate {
                step, of_sequence, ..
            } => Some((step.as_str().to_string(), of_sequence)),
            _ => None,
        })
        .collect()
}

// Verifies: WFL-005, SCH-1525, WFL-2686
#[test]
fn a_failed_step_undoes_the_completed_steps_in_reverse_order() {
    let app = app();
    install(&app, "alpha");
    let run = start_job(&app, "saga", "1", T0);
    worker::tick(&app, "w1", &config(), &at(T0));
    // The third step failed for good; the second was undone first and the first last.
    assert_eq!(
        compensations(&app),
        [("undo_two".to_string(), 2), ("undo_one".to_string(), 1)]
    );
    assert!(notes(&app, "alpha").is_empty(), "both notes were deleted");
    let finished = run_of(&app, &run);
    assert_eq!(finished.state, RunState::Failed);
    assert!(
        finished.error.as_deref().unwrap().ends_with("compensated"),
        "{:?}",
        finished.error
    );
    assert!(finished.ended_at.is_some());
    let records = steps_of(&app, &run);
    let states: Vec<(&str, StepState)> =
        records.iter().map(|r| (r.step.as_str(), r.state)).collect();
    assert_eq!(
        states,
        [
            ("one", StepState::Compensated),
            ("two", StepState::Compensated),
            ("three", StepState::Failed),
        ]
    );
    // The failed step is dead-lettered as before.
    let dead = app
        .store
        .automation_exceptions(
            &tenant_id("alpha"),
            Some(ExceptionKind::DeadLettered),
            true,
            None,
            10,
        )
        .unwrap();
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].step.as_ref().map(|s| s.as_str()), Some("three"));
}

// ---------------------------------------------------------------------------------------------
// The step that invokes a tenant operation.
// ---------------------------------------------------------------------------------------------
