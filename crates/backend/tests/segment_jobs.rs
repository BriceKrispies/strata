//! The job worker over segments (WG-5165, WG-5152, WG-5166, WG-5167): the batches of a segment run
//! as jobs, a refresh runs on its schedule and leaves a skip on the segment when the quota has no
//! room, the event log starts the triggers once, a fan-out reaches each member once, a bundle
//! installs and removes its segments, and a restore re-evaluates every segment. Driven
//! synchronously through `worker::tick` with an injected clock, on the in-memory store.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use strata_backend::app::{App, Config, Shared};
use strata_backend::auth::Identity;
use strata_backend::mail::DevSink;
use strata_backend::product_segments;
use strata_backend::worker::{self, leases, segment_work, steps, WorkerConfig};
use strata_kernel::segment_flow;
use strata_kernel::segments::{self as seg, Definition, SegmentDirection, SegmentId, SegmentKey};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_model::types::AutomationStepResult;
use strata_protocol::ws::SessionState;
use strata_storage_api::jobs::{
    JobReport, JobState, JobWork, NewJob, RetryPolicy, RunState, StepState,
};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::segment_jobs::SegmentWork;
use strata_storage_api::{execute, segments as kept, ExecuteError, SessionRecord, Store};
use strata_substrate::bundle::{
    plan_install, plan_remove, run_planned, InstalledRecord, Planned, ProductBundle, ProductTarget,
};
use strata_substrate::subscribe::{segment_notice, Notice, Selector};

/// The time the tests start at: noon of a quota day, so that a few minutes never cross midnight.
const T0: i64 = 1_728_043_200_000;
const DAY: i64 = 86_400_000;

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

fn config() -> WorkerConfig {
    WorkerConfig::from_limits(&Default::default())
}

fn at(t: i64) -> impl Fn() -> Timestamp {
    move || Timestamp(t)
}

/// The id of the segment numbered `n`.
fn sid(n: u8) -> SegmentId {
    let mut bytes = [0u8; 16];
    bytes[15] = n;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    seg::segment_id_of(bytes)
}

fn gold_id() -> SegmentId {
    sid(1)
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

fn automation(id: &str, start: &str, steps: Vec<(&str, AutomationStepDef)>) -> AutomationDef {
    AutomationDef {
        id: WorkflowId::new(id).unwrap(),
        label: id.to_string(),
        run_as: RunAs {
            roles: BTreeSet::from([role("greeter")]),
        },
        run_key: None,
        start: name(start),
        steps: steps.into_iter().map(|(n, s)| (name(n), s)).collect(),
        defined_by: None,
        defined_at: None,
        suspended_for: None,
    }
}

/// What a test tenant has beyond `member`: the role the automations run as, a note they make, and
/// two roles that read segments, one that reads members and one that does not.
fn extras() -> Vec<SchemaChange> {
    let read = |object_name: &str| Permission::Object {
        object: object(object_name),
        actions: BTreeSet::from([ObjectAction::Read]),
    };
    vec![
        SchemaChange::DefineRole(RoleDef {
            id: role("greeter"),
            label: "greeter".into(),
        }),
        SchemaChange::DefineRole(RoleDef {
            id: role("lister"),
            label: "lister".into(),
        }),
        SchemaChange::DefineRole(RoleDef {
            id: role("reader"),
            label: "reader".into(),
        }),
        SchemaChange::DefineObject {
            id: object("note"),
            label: "Note".into(),
        },
        SchemaChange::DefineField {
            object: object("note"),
            field: field_def("title", FieldType::Str, true),
        },
        SchemaChange::DefineRolePermissions {
            role: role("greeter"),
            permissions: vec![
                read("member"),
                Permission::Object {
                    object: object("note"),
                    actions: BTreeSet::from([ObjectAction::Read, ObjectAction::Create]),
                },
            ],
        },
        SchemaChange::DefineRolePermissions {
            role: role("lister"),
            permissions: vec![
                Permission::Capability(seg::SEGMENTS_READ.into()),
                read("member"),
            ],
        },
        SchemaChange::DefineRolePermissions {
            role: role("reader"),
            permissions: vec![Permission::Capability(seg::SEGMENTS_READ.into())],
        },
    ]
}

/// `welcome`: ends at once; the trigger on entering gold starts it.
fn welcome() -> Vec<SchemaChange> {
    vec![
        SchemaChange::DefineAutomation(automation(
            "welcome",
            "end",
            vec![("end", step(AutomationStep::End, None))],
        )),
        SchemaChange::DefineTrigger(TriggerDef {
            id: TriggerId::new("on_gold").unwrap(),
            on: TriggerEvent::SegmentEntered { segment: gold_id() },
            condition: None,
            automation: WorkflowId::new("welcome").unwrap(),
            paused: None,
            defined_by: None,
            defined_at: None,
        }),
    ]
}

/// `campaign`: runs `tell` for each member of gold, two at a time, then ends. `tell` makes a note
/// titled with the member.
fn campaign() -> Vec<SchemaChange> {
    vec![SchemaChange::DefineAutomation(automation(
        "campaign",
        "blast",
        vec![
            (
                "blast",
                step(
                    AutomationStep::FanOut {
                        segment: gold_id(),
                        batch: 2,
                        each: name("tell"),
                    },
                    Some("done"),
                ),
            ),
            (
                "tell",
                step(
                    AutomationStep::CreateRecord {
                        object: object("note"),
                        fields: BTreeMap::from([(field("title"), Expr::Param("member".into()))]),
                    },
                    None,
                ),
            ),
            ("done", step(AutomationStep::End, None)),
        ],
    ))]
}

fn gold_definition() -> Definition {
    Definition {
        id: gold_id(),
        name: "Gold".into(),
        subject: object("member"),
        predicate: expr::parse("record.tier == 'gold'").unwrap(),
        refresh_seconds: None,
        principal_field: field("principal"),
        visible_to_members: false,
    }
}

fn recent_definition() -> Definition {
    Definition {
        id: sid(2),
        name: "Recent".into(),
        subject: object("member"),
        predicate: expr::parse("record.joined > now - days(7)").unwrap(),
        refresh_seconds: Some(60),
        principal_field: field("principal"),
        visible_to_members: false,
    }
}

/// A tenant on its own in-memory store.
struct Fx {
    app: Shared,
    name: &'static str,
}

impl Fx {
    /// A tenant `name` with the `member` object type and `extra` changes.
    fn new(name: &'static str, extra: Vec<SchemaChange>) -> Fx {
        let fx = Fx { app: app(), name };
        let store = fx.app.store.as_ref();
        let mut now = 100;
        let mut run = |principal: Principal, intent: Intent| {
            now += 1;
            execute(store, &intent, &ctx(name, principal, now)).expect("fixture intent");
        };
        run(
            Principal::System,
            Intent::CreateTenant {
                name: name.to_string(),
                owner: principal_id(&format!("{name}:owner")),
            },
        );
        let mut changes = vec![
            SchemaChange::DefineObject {
                id: object("member"),
                label: "Member".into(),
            },
            SchemaChange::DefineField {
                object: object("member"),
                field: FieldDef {
                    unique: true,
                    ..field_def("principal", FieldType::Str, true)
                },
            },
            SchemaChange::DefineField {
                object: object("member"),
                field: field_def("tier", FieldType::Str, false),
            },
            SchemaChange::DefineField {
                object: object("member"),
                field: field_def("joined", FieldType::Time, false),
            },
        ];
        changes.extend(extra);
        run(
            fx.owner(),
            Intent::ChangeSchema {
                changes,
                expected_version: None,
                existing: ExistingData::Reject,
            },
        );
        fx
    }

    fn tenant(&self) -> TenantId {
        tenant_id(self.name)
    }

    fn owner(&self) -> Principal {
        user(&format!("{}:owner", self.name))
    }

    fn ctx(&self, now: i64) -> Context {
        ctx(self.name, self.owner(), now)
    }

    fn person(&self, who: &str) -> String {
        format!("{}:{who}", self.name)
    }

    fn assign(&self, now: i64, who: &str, role_name: &str) {
        execute(
            self.app.store.as_ref(),
            &Intent::AssignRole {
                principal: principal_id(&self.person(who)),
                role: role(role_name),
            },
            &self.ctx(now),
        )
        .expect("the role is assigned");
    }

    fn create_member(&self, now: i64, who: &str, tier: &str, joined: Option<i64>) -> RecordId {
        let mut fields = BTreeMap::from([
            (field("principal"), text(&self.person(who))),
            (field("tier"), text(tier)),
        ]);
        if let Some(at) = joined {
            fields.insert(field("joined"), Value::Time(at));
        }
        let done = execute(
            self.app.store.as_ref(),
            &Intent::CreateRecord {
                object: object("member"),
                fields,
            },
            &self.ctx(now),
        )
        .expect("the member is created");
        match done.transition.output {
            Output::Record { id, .. } => id,
            other => panic!("unexpected output {other:?}"),
        }
    }

    fn set_tier(&self, now: i64, id: RecordId, tier: &str) {
        execute(
            self.app.store.as_ref(),
            &Intent::UpdateRecord {
                object: object("member"),
                id,
                fields: BTreeMap::from([(field("tier"), text(tier))]),
                expected_version: None,
            },
            &self.ctx(now),
        )
        .expect("the member is updated");
    }

    fn define(&self, now: i64, def: &Definition) {
        kept::define(self.app.store.as_ref(), &self.ctx(now), def).expect("the segment is defined");
    }

    /// Run the worker at `t` until nothing is due; how many jobs ran.
    fn drain(&self, t: i64) -> usize {
        let mut total = 0;
        for _ in 0..200 {
            let pass = worker::tick(&self.app, "w1", &config(), &at(t));
            if pass.jobs == 0 {
                return total;
            }
            total += pass.jobs;
        }
        panic!("the worker never went idle");
    }

    /// The members of a segment, in principal order, read page by page.
    fn members(&self, segment: SegmentId) -> Vec<String> {
        let mut out = Vec::new();
        let mut after: Option<PrincipalId> = None;
        loop {
            let page = self
                .app
                .store
                .segment_members(&self.tenant(), segment, after.as_ref(), 100)
                .unwrap();
            let Some(last) = page.last() else {
                return out;
            };
            after = Some(last.person.clone());
            out.extend(page.iter().map(|m| m.person.as_str().to_string()));
        }
    }

    fn events(&self) -> Vec<(u64, seg::SegmentEvent)> {
        self.app
            .store
            .segment_events_after(&self.tenant(), 0, 10_000)
            .unwrap()
    }

    fn jobs(&self) -> Vec<strata_storage_api::jobs::Job> {
        self.app
            .store
            .jobs_list(&self.tenant(), None, 10_000)
            .unwrap()
    }

    /// The persons the runs started by a trigger were started for.
    fn started_for(&self) -> Vec<String> {
        self.jobs()
            .iter()
            .filter_map(|job| match &job.work {
                JobWork::StartRun { params, .. } => match params.get("person") {
                    Some(Value::Str(person)) => Some(person.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    fn notes(&self) -> Vec<String> {
        let state = self.app.store.load_complete(&self.tenant()).unwrap();
        state
            .tenant
            .map(|t| {
                t.records_of(&object("note"))
                    .filter_map(|r| match r.get(&field("title")) {
                        Value::Str(title) => Some(title.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A job that starts a run of `automation`, due at `due`.
    fn start(&self, automation: &str, due: i64) -> String {
        let job = NewJob {
            key: format!("start:test:{automation}"),
            work: JobWork::StartRun {
                trigger: None,
                automation: WorkflowId::new(automation).unwrap(),
                event_seq: Some(1),
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
        self.app
            .store
            .jobs_create(&self.tenant(), &[job], Timestamp(due))
            .unwrap()
            .remove(0)
    }
}

fn batch_jobs(fx: &Fx) -> Vec<strata_storage_api::jobs::Job> {
    fx.jobs()
        .into_iter()
        .filter(|job| matches!(&job.work, JobWork::Segment(SegmentWork::Batch { .. })))
        .collect()
}

// Verifies: SEG-028, CT-297
#[test]
fn a_segment_of_350_people_is_evaluated_across_batches_with_no_one_twice_or_missed() {
    let fx = Fx::new("batches", vec![]);
    for n in 0..350 {
        fx.create_member(T0 + n, &format!("m{n:03}"), "gold", None);
    }
    fx.define(T0 + 1_000, &gold_definition());
    // The definition left one continuation, and its job is in the same commit.
    assert_eq!(fx.app.store.segment_batches(&fx.tenant()).unwrap().len(), 1);
    assert_eq!(batch_jobs(&fx).len(), 1);
    assert!(
        fx.members(gold_id()).is_empty(),
        "nothing is evaluated before the worker runs"
    );

    fx.drain(T0 + 2_000);

    let members = fx.members(gold_id());
    let distinct: BTreeSet<&String> = members.iter().collect();
    assert_eq!(members.len(), 350);
    assert_eq!(distinct.len(), 350, "no person is a member twice");
    // 350 persons are 100, 100, 100 and 50: four batches, each the job the one before made.
    assert_eq!(batch_jobs(&fx).len(), 4);
    assert!(fx
        .app
        .store
        .segment_batches(&fx.tenant())
        .unwrap()
        .is_empty());
    // One entry for each person, none twice, none missed.
    let events = fx.events();
    let entered: BTreeSet<String> = events
        .iter()
        .map(|(_, e)| e.event.person.as_str().to_string())
        .collect();
    assert_eq!(events.len(), 350);
    assert_eq!(entered.len(), 350);
    assert!(events
        .iter()
        .all(|(_, e)| e.event.direction == SegmentDirection::Entered));
    // The log is handed on and every job ended well.
    assert_eq!(fx.app.store.segment_cursor(&fx.tenant()).unwrap(), 350);
    assert!(fx.jobs().iter().all(|job| job.state == JobState::Completed));
}

// Verifies: SEG-018, SEG-1545
#[test]
fn a_time_dependent_segments_member_leaves_at_the_first_refresh_after_the_wake_time() {
    let fx = Fx::new("timed", vec![]);
    fx.define(T0, &recent_definition());
    let joined = T0;
    fx.create_member(T0 + 1_000, "fresh", "silver", Some(joined));
    assert_eq!(fx.members(sid(2)), vec![fx.person("fresh")]);

    // The person next changes seven days after joining: the wake row says so.
    let wake = joined + 7 * DAY;
    let rows = fx
        .app
        .store
        .segment_wakes_of(&fx.tenant(), &principal_id(&fx.person("fresh")))
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(
        rows[0].at >= Timestamp(wake) && rows[0].at <= Timestamp(wake + 1),
        "{:?}",
        rows[0]
    );

    // A refresh before the wake time evaluates nobody and changes nothing.
    fx.drain(wake - 1_000);
    assert_eq!(fx.members(sid(2)), vec![fx.person("fresh")]);

    // The first refresh after it takes the person out, and a refresh follows it.
    fx.drain(wake + 61_000);
    assert!(fx.members(sid(2)).is_empty());
    let events = fx.events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].1.event.direction, SegmentDirection::Exited);
    assert_eq!(events[1].1.event.reason, seg::SegmentReason::Refresh);
    assert!(
        fx.jobs().iter().any(|job| job.state == JobState::Waiting
            && matches!(&job.work, JobWork::Segment(SegmentWork::Refresh { .. }))),
        "the schedule goes on"
    );
}

// Verifies: SEG-1543, SEG-1544
#[test]
fn a_refresh_with_no_quota_is_skipped_and_recorded_and_refuses_no_commit() {
    let fx = Fx::new("capped", vec![]);
    segment_work::set_daily_job_quota(&fx.tenant(), 1);
    fx.define(T0, &recent_definition());
    // The definition made a batch and the schedule: two jobs of the quota day, past a quota of one.
    fx.drain(T0 + 60_000);
    let catalog = fx.app.store.segment_catalog(&fx.tenant()).unwrap();
    assert_eq!(catalog[0].refresh_skipped_at, Some(Timestamp(T0 + 60_000)));
    // The change of a person still commits, and the person enters with it.
    fx.create_member(T0 + 61_000, "fresh", "silver", Some(T0 + 61_000));
    assert_eq!(fx.members(sid(2)), vec![fx.person("fresh")]);
    // The schedule is not dropped: the next refresh catches up.
    assert!(fx.jobs().iter().any(|job| job.state == JobState::Waiting
        && matches!(&job.work, JobWork::Segment(SegmentWork::Refresh { .. }))));
    // With room again the refresh runs and records nothing new.
    segment_work::set_daily_job_quota(&fx.tenant(), 1_000);
    fx.drain(T0 + 130_000);
    let catalog = fx.app.store.segment_catalog(&fx.tenant()).unwrap();
    assert_eq!(catalog[0].refresh_skipped_at, Some(Timestamp(T0 + 60_000)));
}

// Verifies: SEG-014, SEG-1543, SCH-015
#[test]
fn a_trigger_on_entry_fires_once_for_each_person_under_a_retried_batch() {
    let mut schema = extras();
    schema.extend(welcome());
    let fx = Fx::new("entry", schema);
    for n in 0..5 {
        fx.create_member(T0 + n, &format!("m{n}"), "gold", None);
    }
    fx.define(T0 + 100, &gold_definition());
    let tenant = fx.tenant();

    // A worker leases the batch job, runs it, and stops before it reports.
    let lease = fx
        .app
        .store
        .job_lease(&tenant, "w1", Timestamp(T0 + 200))
        .unwrap()
        .unwrap();
    assert!(matches!(
        lease.job.work,
        JobWork::Segment(SegmentWork::Batch { .. })
    ));
    let first = steps::run_job(&fx.app, &tenant, &lease, Timestamp(T0 + 200));
    assert_eq!(first.report, JobReport::Succeeded);
    assert_eq!(fx.started_for().len(), 5);

    // The lease expires; another worker runs the job again, and so does everything else due.
    let later = T0 + 200 + 60_001;
    fx.drain(later);
    let started = fx.started_for();
    let distinct: BTreeSet<&String> = started.iter().collect();
    assert_eq!(started.len(), 5, "one run for each entry: {started:?}");
    assert_eq!(distinct.len(), 5, "and each for another person");
    let retried = fx
        .jobs()
        .into_iter()
        .find(|job| job.id == lease.job.id)
        .unwrap();
    assert_eq!(retried.attempt, 2, "the batch was attempted again");
    assert_eq!(retried.state, JobState::Completed);
    // The runs were made, as the automation, once each.
    let welcome = WorkflowId::new("welcome").unwrap();
    let runs = fx
        .app
        .store
        .automation_runs(&tenant, Some(&welcome), None, None, 100)
        .unwrap();
    assert_eq!(runs.len(), 5);
    assert!(runs.iter().all(|run| run.state == RunState::Completed));
    // The log is handed on to its end, and a second pass hands nothing on.
    assert_eq!(fx.app.store.segment_cursor(&tenant).unwrap(), 5);
    assert_eq!(fx.drain(later + 1_000), 0);
    assert_eq!(fx.started_for().len(), 5);
}

// Verifies: WFL-1538
#[test]
fn a_fan_out_reaches_each_member_once_and_its_typed_result_is_in_the_step_record() {
    let mut schema = extras();
    schema.extend(campaign());
    let fx = Fx::new("fanout", schema);
    for n in 0..5 {
        fx.create_member(T0 + n, &format!("m{n}"), "gold", None);
    }
    fx.define(T0 + 100, &gold_definition());
    fx.drain(T0 + 200);
    assert_eq!(fx.members(gold_id()).len(), 5);

    let run = fx.start("campaign", T0 + 300);
    fx.drain(T0 + 300);

    let mut notes = fx.notes();
    notes.sort();
    let mut wanted: Vec<String> = (0..5).map(|n| fx.person(&format!("m{n}"))).collect();
    wanted.sort();
    assert_eq!(notes, wanted, "each member once");
    let record = fx
        .app
        .store
        .automation_run(&fx.tenant(), &run)
        .unwrap()
        .unwrap();
    assert_eq!(record.state, RunState::Completed);
    let recorded = fx
        .app
        .store
        .automation_steps(&fx.tenant(), &run, None, 20)
        .unwrap();
    let blast = recorded
        .iter()
        .find(|step| step.step.as_str() == "blast")
        .expect("the fan-out step is recorded");
    assert_eq!(blast.state, StepState::Completed);
    let result: AutomationStepResult =
        serde_json::from_value(blast.result.clone().expect("a typed result")).unwrap();
    assert!(
        matches!(
            result,
            AutomationStepResult::FanOut {
                members: 5,
                batches: 3
            }
        ),
        "{result:?}"
    );
    // The outputs are bound for the steps that follow.
    assert_eq!(
        record.params.get("blast_members"),
        Some(&Value::Int(5)),
        "{:?}",
        record.params
    );
}

// Verifies: SCH-015, WFL-1538
#[test]
fn a_batch_of_a_fan_out_attempted_again_after_a_crash_applies_its_effect_once() {
    let mut schema = extras();
    schema.extend(campaign());
    let fx = Fx::new("crash", schema);
    for n in 0..5 {
        fx.create_member(T0 + n, &format!("m{n}"), "gold", None);
    }
    fx.define(T0 + 100, &gold_definition());
    fx.drain(T0 + 200);
    fx.start("campaign", T0 + 300);
    let tenant = fx.tenant();
    let now = Timestamp(T0 + 300);
    // The start of the run and the fan-out step are reported; the first batch is run and not.
    for _ in 0..2 {
        let lease = fx.app.store.job_lease(&tenant, "w1", now).unwrap().unwrap();
        leases::run_leased(&fx.app, &tenant, &lease, &at(now.0)).unwrap();
    }
    let held = fx.app.store.job_lease(&tenant, "w1", now).unwrap().unwrap();
    assert!(matches!(
        held.job.work,
        JobWork::Segment(SegmentWork::FanOut { .. })
    ));
    let outcome = steps::run_job(&fx.app, &tenant, &held, now);
    assert_eq!(outcome.report, JobReport::Succeeded);
    assert_eq!(fx.notes().len(), 2, "the first batch made its two notes");

    // Its lease expires, the batch is attempted again, and the run goes on to its end.
    fx.drain(now.0 + 60_001);
    let mut notes = fx.notes();
    notes.sort();
    let mut wanted: Vec<String> = (0..5).map(|n| fx.person(&format!("m{n}"))).collect();
    wanted.sort();
    assert_eq!(notes, wanted, "no member was told twice");
}

// Verifies: SEG-033
#[test]
fn a_subscription_hears_the_entry_of_a_person_whose_record_the_subscriber_may_read() {
    let fx = Fx::new("notices", extras());
    fx.assign(T0, "lister1", "lister");
    fx.assign(T0 + 1, "reader1", "reader");
    fx.define(T0 + 2, &gold_definition());
    fx.create_member(T0 + 3, "m1", "gold", None);
    let (position, logged) = fx.events().remove(0);
    let catalog = fx.app.store.segment_catalog(&fx.tenant()).unwrap();
    let segment = &catalog[0];
    let need = segment_flow::subject_need(segment, &logged.event.person);
    let state = fx.app.store.load(&fx.tenant(), &[need]).unwrap();
    let selector =
        Selector::new(None, None, Some(vec![gold_id().to_string()]), None, None).unwrap();
    let hears = |who: &str, selector: &Selector| {
        let principal = Principal::User(principal_id(&fx.person(who)));
        segment_notice(&state, &principal, selector, segment, position, &logged)
    };
    // The subscriber that reads members and the person's record is told.
    assert!(matches!(
        hears("lister1", &selector),
        Some(Notice::Segment {
            direction: SegmentDirection::Entered,
            ..
        })
    ));
    assert!(hears("owner", &selector).is_some());
    // One that may list segments but not read the record is told nothing, and neither is one
    // that holds no right to the members at all.
    assert_eq!(hears("reader1", &selector), None);
    assert_eq!(hears("nobody", &selector), None);
    // A selector that does not name the segment never hears its events.
    assert_eq!(hears("lister1", &Selector::all()), None);
}

// Verifies: SEG-022, SEG-025
#[test]
fn a_restored_snapshot_leaves_membership_equal_to_a_fresh_evaluation_once_the_batches_run() {
    let fx = Fx::new("restore", vec![]);
    let m1 = fx.create_member(T0, "m1", "gold", None);
    fx.create_member(T0 + 1, "m2", "gold", None);
    fx.create_member(T0 + 2, "m3", "silver", None);
    fx.define(T0 + 10, &gold_definition());
    fx.drain(T0 + 20);
    assert_eq!(fx.members(gold_id()), [fx.person("m1"), fx.person("m2")]);
    let snapshot = fx
        .app
        .store
        .create_snapshot(&fx.tenant(), "before", Timestamp(T0 + 30))
        .unwrap();

    // Memberships move on: m1 leaves, m3 and a new person enter.
    fx.set_tier(T0 + 40, m1, "silver");
    fx.create_member(T0 + 41, "m4", "gold", None);
    assert_eq!(fx.members(gold_id()), [fx.person("m2"), fx.person("m4")]);

    fx.app
        .store
        .restore_snapshot(&fx.tenant(), snapshot.id)
        .unwrap();
    // The rows no longer rest on the records: they are cleared and every person is evaluated
    // again by the batches the restore left.
    assert!(fx.members(gold_id()).is_empty());
    assert_eq!(fx.app.store.segment_batches(&fx.tenant()).unwrap().len(), 1);
    fx.drain(T0 + 100);
    let state = fx.app.store.load_complete(&fx.tenant()).unwrap();
    let fresh: BTreeSet<String> = state
        .tenant
        .unwrap()
        .records_of(&object("member"))
        .filter(|r| r.get(&field("tier")) == text("gold"))
        .filter_map(|r| match r.get(&field("principal")) {
            Value::Str(person) => Some(person.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        fx.members(gold_id()).into_iter().collect::<BTreeSet<_>>(),
        fresh
    );
    assert_eq!(fresh, BTreeSet::from([fx.person("m1"), fx.person("m2")]));
    assert!(fx
        .app
        .store
        .segment_batches(&fx.tenant())
        .unwrap()
        .is_empty());
    let catalog = fx.app.store.segment_catalog(&fx.tenant()).unwrap();
    assert_eq!(
        catalog[0].version, 2,
        "a restore supersedes what was pending"
    );
}

fn signed_in(fx: &Fx, who: &str) -> Identity {
    let principal = principal_id(&fx.person(who));
    Identity {
        tenant: fx.tenant(),
        principal: Principal::User(principal.clone()),
        session: Some(SessionRecord {
            id: SessionId::new(format!("s-{who}")).unwrap(),
            token_hash: format!("hash-{who}"),
            csrf_token: "csrf".into(),
            tenant: fx.tenant(),
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
        correlation: "t-segment-jobs".into(),
        verified_address: None,
        idempotency_key: None,
    }
}

/// A product that declares a record type and the segment of its gold members.
fn tiers_bundle() -> serde_json::Value {
    serde_json::json!({
        "name": "Tiers",
        "version": "1.0.0",
        "product": "tiers",
        "platform": { "min": "0.2", "below": "0.4" },
        "content": {
            "objects": {
                "tiers__member": {
                    "id": "tiers__member",
                    "label": "Member",
                    "fields": {
                        "principal": { "id": "principal", "label": "Principal", "ty": "Str", "required": true, "unique": true },
                        "tier": { "id": "tier", "label": "Tier", "ty": "Str", "required": false, "unique": false }
                    }
                }
            },
            "segments": {
                "tiers__gold": {
                    "name": "Gold",
                    "subject-type": "tiers__member",
                    "predicate": "record.tier == 'gold'",
                    "principal-field": "principal"
                }
            }
        }
    })
}

// Verifies: INT-1581, SEG-026, D-6582
#[test]
fn a_bundle_with_a_segment_installs_defines_it_once_and_removes_it_without_an_exit_event() {
    let fx = Fx::new("bundle", vec![]);
    let identity = signed_in(&fx, "owner");
    let tenant = fx.tenant();
    let bundle = ProductBundle::from_json(&tiers_bundle().to_string()).unwrap();
    let derived = seg::derive_segment_id(&tenant, &SegmentKey::new("tiers__gold").unwrap());
    let store = fx.app.store.as_ref();
    let mut run = |sub: Context, intent: Intent| -> Result<Output, ExecuteError> {
        execute(store, &intent, &sub).map(|done| done.transition.output)
    };

    // Install: the schema change is one commit; the segment follows through the segment store.
    let schema = store
        .load(&tenant, &[])
        .unwrap()
        .tenant
        .unwrap()
        .schema
        .clone();
    let target = ProductTarget {
        tenant: &tenant,
        principal: fx.owner(),
        now: T0,
        release: "0.2",
        schema: &schema,
        installed: None,
        expected_version: None,
        idempotency_key: None,
    };
    let planned = plan_install(&bundle, &target).unwrap();
    let Planned::Change(change) = planned.clone() else {
        panic!("the install is a change")
    };
    product_segments::precheck(&fx.app, &identity, &change, None).unwrap();
    assert!(
        store.segment_catalog(&tenant).unwrap().is_empty(),
        "the pre-check touches nothing"
    );
    run_planned(planned, &target, &mut run).unwrap();
    product_segments::define_all(&fx.app, &identity, &change).unwrap();
    let catalog = store.segment_catalog(&tenant).unwrap();
    assert_eq!(catalog.len(), 1);
    assert_eq!(
        catalog[0].id, derived,
        "the id is derived from the tenant and the key"
    );
    assert_eq!(catalog[0].version, 1);
    let after = store
        .load(&tenant, &[])
        .unwrap()
        .tenant
        .unwrap()
        .schema
        .clone();
    let record = InstalledRecord::of(&after, "tiers").unwrap();
    assert!(record
        .owns
        .iter()
        .any(|e| e.kind == "segment" && e.id == "tiers__gold"));

    // The same bundle again is the same install: the same id at the same version.
    let target = ProductTarget {
        tenant: &tenant,
        principal: fx.owner(),
        now: T0 + 1,
        release: "0.2",
        schema: &after,
        installed: None,
        expected_version: None,
        idempotency_key: None,
    };
    assert!(matches!(
        plan_install(&bundle, &target).unwrap(),
        Planned::Unchanged { .. }
    ));
    product_segments::ensure(&fx.app, &identity, &bundle).unwrap();
    let again = store.segment_catalog(&tenant).unwrap();
    assert_eq!(
        (again.len(), again[0].id, again[0].version),
        (1, derived, 1)
    );

    // A member enters with the commit that creates it, so that a removal has something to leave
    // behind.
    execute(
        store,
        &Intent::CreateRecord {
            object: object("tiers__member"),
            fields: BTreeMap::from([
                (field("principal"), text("bundle:ines")),
                (field("tier"), text("gold")),
            ]),
        },
        &fx.ctx(T0 + 2),
    )
    .unwrap();
    assert_eq!(fx.members(derived), vec!["bundle:ines".to_string()]);
    let logged = fx.events().len();
    assert_eq!(logged, 1);

    // Removal: the segment goes first, with its members and with no exit event, then the schema.
    let target = ProductTarget {
        tenant: &tenant,
        principal: fx.owner(),
        now: T0 + 20,
        release: "0.2",
        schema: &after,
        installed: None,
        expected_version: None,
        idempotency_key: None,
    };
    let planned = plan_remove("tiers", true, &target).unwrap();
    let Planned::Change(removal) = planned.clone() else {
        panic!("the removal is a change")
    };
    assert_eq!(removal.segments.removals(&tenant), vec![derived]);
    product_segments::precheck(&fx.app, &identity, &removal, None).unwrap();
    let taken = product_segments::take(&fx.app, &identity, &removal).unwrap();
    assert!(!taken.is_empty());
    run_planned(planned, &target, &mut run).unwrap();
    assert!(store.segment_catalog(&tenant).unwrap().is_empty());
    assert!(fx.members(derived).is_empty());
    assert_eq!(fx.events().len(), logged, "a removal writes no exit event");
    let gone = store
        .load(&tenant, &[])
        .unwrap()
        .tenant
        .unwrap()
        .schema
        .clone();
    assert!(InstalledRecord::of(&gone, "tiers").is_none());
}
