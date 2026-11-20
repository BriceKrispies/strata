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
use strata_backend::worker::{self, commands, leases, routes, steps, WorkerConfig};
use strata_kernel::approvals::WORKFLOW_APPROVE;
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_model::prelude::DurationMs;
use strata_model::types::AutomationStepResult;
use strata_protocol::api::ErrorCode;
use strata_protocol::ws::SessionState;
use strata_storage_api::jobs::{
    ExceptionKind, JobReport, JobWork, NewJob, Reported, RetryPolicy, RunRecord, RunState,
    StepState,
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

fn body_of(note: &Record) -> Value {
    note.fields
        .get(&field("body"))
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

// Verifies: WFL-2680
#[test]
fn a_refusal_takes_the_refused_branch_and_the_note_stays_a_draft() {
    let app = app();
    install(&app, "alpha");
    let note = write_note(&app, "writer1", "Review me", "review-me", 200);
    let run = submit(&app, "writer1", note, "k-1", T0);
    decide(&app, "editor", &run, false, T0 + 10).expect("refused");
    worker::tick(&app, "w1", &config(), &at(T0 + 10));
    assert_eq!(run_of(&app, &run).state, RunState::Completed);
    let note = notes(&app, "alpha").remove(0);
    assert_eq!(status_of(&note), text("draft"));
    assert_eq!(body_of(&note), text("rejected"));
    let decision = steps_of(&app, &run)
        .remove(0)
        .approval
        .and_then(|approval| approval.decision)
        .expect("the decision");
    assert!(!decision.approved && !decision.timed_out);
}

// Verifies: WFL-2682
#[test]
fn a_deadline_that_passes_takes_the_refused_branch_and_a_late_decision_is_refused() {
    let app = app();
    install(&app, "alpha");
    let note = write_note(&app, "writer1", "Review me", "review-me", 200);
    let run = submit(&app, "writer1", note, "k-1", T0);
    let deadline = T0 + HOUR as i64;
    // After the deadline and before the worker has taken it, a decision is a conflict.
    let late = decide(&app, "editor", &run, true, deadline + 1).expect_err("late");
    assert_eq!(code_of(&late), ErrorCode::Conflict);
    assert_eq!(run_of(&app, &run).state, RunState::AwaitingApproval);
    // The worker takes the deadline: the refused branch, the decision recorded as timed out.
    worker::tick(&app, "w1", &config(), &at(deadline + 1));
    assert_eq!(run_of(&app, &run).state, RunState::Completed);
    assert_eq!(body_of(&notes(&app, "alpha")[0]), text("rejected"));
    let decision = steps_of(&app, &run)
        .remove(0)
        .approval
        .and_then(|approval| approval.decision)
        .expect("the decision");
    assert!(decision.timed_out && !decision.approved);
    assert_eq!(decision.by, None);
    assert_eq!(run_of(&app, &run).params["gate_by"], Value::Null);
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

// Verifies: WFL-006, WFL-2680
#[test]
fn a_principal_without_the_role_the_policy_names_is_refused_and_the_run_keeps_waiting() {
    let app = app();
    install(&app, "alpha");
    let note = write_note(&app, "writer1", "Review me", "review-me", 200);
    let run = submit(&app, "writer1", note, "k-1", T0);
    // writer2 holds workflow-approve and is not the author, but the policy names the editors.
    let error = decide(&app, "writer2", &run, true, T0 + 10).expect_err("refused");
    assert_eq!(code_of(&error), ErrorCode::Forbidden);
    // A stranger holds nothing at all.
    let error = decide(&app, "stranger", &run, true, T0 + 10).expect_err("refused");
    assert_eq!(code_of(&error), ErrorCode::Forbidden);
    assert_eq!(run_of(&app, &run).state, RunState::AwaitingApproval);
    assert_eq!(steps_of(&app, &run)[0].state, StepState::Waiting);
    assert_eq!(worker::tick(&app, "w1", &config(), &at(T0 + 10)).jobs, 0);
}

// Verifies: WFL-2681
#[test]
fn the_person_who_started_the_run_may_not_approve_it_even_holding_the_role() {
    let app = app();
    install(&app, "alpha");
    let note = write_note(&app, "editor", "Review me", "review-me", 200);
    // An editor starts the review of their own note.
    let run = submit(&app, "editor", note, "k-1", T0);
    let error = decide(&app, "editor", &run, true, T0 + 10).expect_err("refused");
    assert_eq!(code_of(&error), ErrorCode::Forbidden);
    assert_eq!(run_of(&app, &run).state, RunState::AwaitingApproval);
    // Another editor may.
    decide(&app, "editor2", &run, true, T0 + 11).expect("approved by another editor");
    worker::tick(&app, "w1", &config(), &at(T0 + 11));
    assert_eq!(status_of(&notes(&app, "alpha")[0]), text("published"));
}

// Verifies: WFL-2681
#[test]
fn a_run_started_by_a_trigger_records_the_person_who_caused_the_event_and_separates_them() {
    let app = app();
    install(&app, "alpha");
    execute(
        app.store.as_ref(),
        &Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineTrigger(TriggerDef {
                id: TriggerId::new("on_note").unwrap(),
                on: TriggerEvent::RecordCreated {
                    object: object("note"),
                },
                condition: None,
                automation: WorkflowId::new("gated").unwrap(),
                paused: None,
                defined_by: None,
                defined_at: None,
            })],
            expected_version: None,
            existing: ExistingData::Reject,
        },
        &ctx("alpha", user("alpha:owner"), 150),
    )
    .expect("a trigger");
    // editor creates a note: the trigger on note-created starts `gated` for that event.
    write_note(&app, "editor", "Gated", "gated", 200);
    worker::tick(&app, "w1", &config(), &at(T0));
    let runs = app
        .store
        .automation_runs(&tenant_id("alpha"), None, None, None, 10)
        .unwrap();
    assert_eq!(runs.len(), 1);
    let run = runs[0].run.clone();
    assert_eq!(runs[0].state, RunState::AwaitingApproval);
    assert_eq!(runs[0].started_by, Some(principal_id("alpha:editor")));
    let error = decide(&app, "editor", &run, true, T0 + 10).expect_err("refused");
    assert_eq!(code_of(&error), ErrorCode::Forbidden);
    decide(&app, "editor2", &run, true, T0 + 11).expect("approved");
    worker::tick(&app, "w1", &config(), &at(T0 + 11));
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

// Verifies: WFL-2680
#[test]
fn an_approval_names_a_run_of_this_tenant_and_a_decision_that_is_one() {
    let app = app();
    install(&app, "alpha");
    let missing = decide(&app, "editor", "no-such-run", true, T0).expect_err("unknown");
    assert_eq!(code_of(&missing), ErrorCode::NotFound);
    let note = write_note(&app, "writer1", "Review me", "review-me", 200);
    let run = submit(&app, "writer1", note, "k-1", T0);
    let undecided = commands::approve_step_at(
        &app,
        &signed_in("alpha", "editor"),
        &json!({ "run": run }),
        Timestamp(T0 + 1),
    )
    .expect_err("no decision");
    assert_eq!(code_of(&undecided), ErrorCode::Invalid);
    let mut anonymous = signed_in("alpha", "editor");
    anonymous.principal = Principal::Anonymous;
    anonymous.session = None;
    let error = commands::approve_step_at(
        &app,
        &anonymous,
        &json!({ "run": run, "approved": true }),
        Timestamp(T0 + 1),
    )
    .expect_err("anonymous");
    assert_eq!(code_of(&error), ErrorCode::Unauthenticated);
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

// Verifies: WFL-2687
#[test]
fn starting_a_run_needs_jobs_run_an_active_automation_and_inputs_an_automation_reads() {
    let app = app();
    install(&app, "alpha");
    let body =
        |workflow: &str, input: serde_json::Value| json!({ "workflow": workflow, "input": input });
    // A stranger holds no permission.
    let error = commands::start_run_at(
        &app,
        &signed_in("alpha", "stranger"),
        &body("review", json!({})),
        Timestamp(T0),
    )
    .expect_err("forbidden");
    assert_eq!(code_of(&error), ErrorCode::Forbidden);
    let writer = signed_in("alpha", "writer1");
    let refused = |workflow: &str, input: serde_json::Value| {
        code_of(
            &commands::start_run_at(&app, &writer, &body(workflow, input), Timestamp(T0))
                .expect_err("refused"),
        )
    };
    assert_eq!(refused("nobody", json!({})), ErrorCode::NotFound);
    assert_eq!(
        refused("drafty", json!({})),
        ErrorCode::Conflict,
        "a draft starts no run"
    );
    assert_eq!(
        refused("review", json!({ "colour": 1 })),
        ErrorCode::Invalid,
        "not a name an automation reads"
    );
    assert_eq!(
        refused("review", json!({ "at": 1 })),
        ErrorCode::Invalid,
        "bound by the platform"
    );
    assert_eq!(
        refused("review", json!({ "subject": { "a": 1 } })),
        ErrorCode::Invalid
    );
    assert!(app
        .store
        .jobs_list(&tenant_id("alpha"), None, 10)
        .unwrap()
        .is_empty());
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

// Verifies: SCH-1524, WFL-2686
#[test]
fn a_compensation_that_is_attempted_again_after_a_crash_compensates_once() {
    let app = app();
    install(&app, "alpha");
    let alpha = tenant_id("alpha");
    let run = start_job(&app, "saga", "1", T0);
    // Run the jobs by hand until the compensation of the second step, run it, and stop before
    // reporting, as a worker that crashed would.
    let (held, outcome) = loop {
        let lease = app
            .store
            .job_lease(&alpha, "w1", Timestamp(T0))
            .unwrap()
            .expect("a job is due");
        if matches!(lease.job.work, JobWork::Compensate { of_sequence: 2, .. }) {
            let outcome = steps::run_job(&app, &alpha, &lease, Timestamp(T0));
            break (lease, outcome);
        }
        leases::run_leased(&app, &alpha, &lease, &at(T0)).unwrap();
    };
    assert_eq!(outcome.report, JobReport::Succeeded);
    assert_eq!(
        notes(&app, "alpha").len(),
        1,
        "the second note is deleted, the first is not yet"
    );
    // The lease expires; another worker attempts it again under the same key, and the delete is
    // not applied a second time (a second delete would fail and leave the run compensating).
    let later = T0 + 60_001;
    worker::tick(&app, "w2", &config(), &at(later));
    assert!(notes(&app, "alpha").is_empty());
    assert_eq!(run_of(&app, &run).state, RunState::Failed);
    assert_eq!(
        compensations(&app),
        [("undo_two".to_string(), 2), ("undo_one".to_string(), 1)]
    );
    // The first worker's late report is refused by its token.
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
    assert_eq!(run_of(&app, &run).state, RunState::Failed);
}

// Verifies: WFL-005, WFL-2686
#[test]
fn a_compensation_that_fails_leaves_the_run_compensating_and_the_owner_can_retry_it() {
    let app = app();
    install(&app, "alpha");
    let alpha = tenant_id("alpha");
    let run = start_job(&app, "saga_stuck", "1", T0);
    worker::tick(&app, "w1", &config(), &at(T0));
    // The run is visible as compensating, with the note it could not undo still there.
    let stuck = run_of(&app, &run);
    assert_eq!(stuck.state, RunState::Compensating);
    assert!(stuck.ended_at.is_none());
    assert!(stuck
        .error
        .as_deref()
        .unwrap()
        .contains("compensation undo_one of step one failed"));
    assert_eq!(notes(&app, "alpha").len(), 1);
    assert_eq!(steps_of(&app, &run)[0].state, StepState::Completed);
    // The owner finds an unsettled dead-lettered record that names the compensating step and job.
    let unsettled = |app: &App| {
        app.store
            .automation_exceptions(&alpha, Some(ExceptionKind::DeadLettered), true, None, 10)
            .unwrap()
    };
    let records: Vec<_> = unsettled(&app)
        .into_iter()
        .filter(|record| record.step.as_ref().map(|s| s.as_str()) == Some("undo_one"))
        .collect();
    assert_eq!(records.len(), 1);
    let job = records[0].job.clone().expect("the compensation job");
    // Releasing it gives that job one more attempt, which fails again for the same reason.
    let released = routes::settle_exception(
        &app,
        &signed_in("alpha", "owner"),
        &json!({ "exception": records[0].id, "action": "release" }),
    )
    .expect("released");
    assert_eq!(released["job"], job);
    // The retry is due at the platform's clock: the worker's clock is moved past it.
    let after_release = app.now().0 + 10_000;
    worker::tick(&app, "w1", &config(), &at(after_release));
    assert_eq!(run_of(&app, &run).state, RunState::Compensating);
    let again: Vec<_> = unsettled(&app)
        .into_iter()
        .filter(|record| record.step.as_ref().map(|s| s.as_str()) == Some("undo_one"))
        .collect();
    assert_eq!(
        again.len(),
        1,
        "the first record is settled and the second is the new failure"
    );
    assert_ne!(again[0].id, records[0].id);
}

// ---------------------------------------------------------------------------------------------
// The step that invokes a tenant operation.
// ---------------------------------------------------------------------------------------------

// Verifies: WFL-2685
#[test]
fn an_invoke_operation_step_invokes_the_operation_as_the_automation_and_records_its_result() {
    let app = app();
    install(&app, "alpha");
    let run = start_job(&app, "stamper", "1", T0);
    worker::tick(&app, "w1", &config(), &at(T0));
    assert_eq!(run_of(&app, &run).state, RunState::Completed);
    let made = notes(&app, "alpha");
    assert_eq!(made.len(), 1);
    assert_eq!(made[0].fields[&field("title")], text("Stamped"));
    assert_eq!(made[0].fields[&field("slug")], text("stamped"));
    assert_eq!(
        made[0].created_by,
        Some(principal_id("automation:stamper")),
        "the automation principal invoked it"
    );
    let call = steps_of(&app, &run).remove(0);
    match serde_json::from_value(call.result.expect("a result")).expect("a typed result") {
        AutomationStepResult::InvokeOperation {
            created,
            changed,
            deleted,
        } => {
            assert_eq!(created, [made[0].id]);
            assert!(changed.is_empty() && deleted.is_empty());
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        run_of(&app, &run).params["call_created"],
        Value::List(vec![Value::Ref(made[0].id)])
    );
}

// Verifies: SCH-1524, WFL-2685
#[test]
fn an_invoke_operation_step_that_is_attempted_again_after_a_crash_invokes_once() {
    let app = app();
    install(&app, "alpha");
    let alpha = tenant_id("alpha");
    let run = start_job(&app, "stamper", "1", T0);
    // The start job runs and reports; then w1 leases the step, invokes, and stops before reporting.
    let first = app
        .store
        .job_lease(&alpha, "w1", Timestamp(T0))
        .unwrap()
        .unwrap();
    leases::run_leased(&app, &alpha, &first, &at(T0)).unwrap();
    let held = app
        .store
        .job_lease(&alpha, "w1", Timestamp(T0))
        .unwrap()
        .unwrap();
    let outcome = steps::run_job(&app, &alpha, &held, Timestamp(T0));
    assert_eq!(outcome.report, JobReport::Succeeded);
    assert_eq!(notes(&app, "alpha").len(), 1);
    // The lease expires and another worker attempts it again with the same step key: no second note.
    worker::tick(&app, "w2", &config(), &at(T0 + 60_001));
    assert_eq!(notes(&app, "alpha").len(), 1);
    assert_eq!(run_of(&app, &run).state, RunState::Completed);
}

// Verifies: WFL-2685
#[test]
fn an_automation_cannot_invoke_an_operation_beyond_what_its_run_as_and_the_operation_allow() {
    let app = app();
    install(&app, "alpha");
    let alpha = tenant_id("alpha");
    let owner = Principal::User(principal_id("alpha:owner"));
    let define = |def: AutomationDef| {
        execute(
            app.store.as_ref(),
            &Intent::ChangeSchema {
                changes: vec![SchemaChange::DefineAutomation(def)],
                expected_version: None,
                existing: ExistingData::Reject,
            },
            &ctx("alpha", owner.clone(), 5_000),
        )
    };
    // The operation names the editors; this automation acts as a writer.
    assert!(define(invoking(
        "sneak",
        "writer",
        "stamp_editors",
        &[("slug", lit("s"))]
    ))
    .is_err());
    // Acting as an editor it may.
    assert!(define(invoking(
        "allowed",
        "editor",
        "stamp_editors",
        &[("slug", lit("s"))]
    ))
    .is_ok());
    // An input the operation does not declare is refused, so a step cannot reach past the operation.
    assert!(define(invoking(
        "extra",
        "writer",
        "stamp_note",
        &[("slug", lit("s")), ("status", lit("published"))]
    ))
    .is_err());
    // A missing required input and an input of another type are refused.
    assert!(define(invoking("missing", "writer", "stamp_note", &[])).is_err());
    assert!(define(invoking(
        "typed",
        "writer",
        "stamp_note",
        &[("slug", Expr::Lit(Value::Int(1)))]
    ))
    .is_err());
    // What was refused left nothing behind.
    let state = app.store.load(&alpha, &[]).unwrap().tenant.unwrap();
    let held = state.schema.automations.clone().unwrap_or_default();
    for refused in ["sneak", "extra", "missing", "typed"] {
        assert!(
            !held.contains_key(&WorkflowId::new(refused).unwrap()),
            "{refused}"
        );
    }
    assert!(held.contains_key(&WorkflowId::new("allowed").unwrap()));
}
