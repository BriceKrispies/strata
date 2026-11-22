//! The job worker over segments (WG-5165, WG-5152, WG-5166, WG-5167): the batches of a segment run
//! as jobs, a refresh runs on its schedule and leaves a skip on the segment when the quota has no
//! room, the event log starts the triggers once, a fan-out reaches each member once, a bundle
//! installs and removes its segments, and a restore re-evaluates every segment. Driven
//! synchronously through `worker::tick` with an injected clock, on the in-memory store.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use strata_backend::app::{App, Config, Shared};
use strata_backend::mail::DevSink;
use strata_backend::worker::{self, steps, WorkerConfig};
use strata_kernel::segment_flow;
use strata_kernel::segments::{self as seg, Definition, SegmentDirection, SegmentId};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_model::types::AutomationStepResult;
use strata_storage_api::jobs::{
    JobReport, JobState, JobWork, NewJob, RetryPolicy, RunState, StepState,
};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::segment_jobs::SegmentWork;
use strata_storage_api::{execute, segments as kept, Store};
use strata_substrate::subscribe::{segment_notice, Notice, Selector, SelectorExt};

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
        state: None,
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

// Verifies: SEG-022, SEG-024, CT-297
#[test]
fn a_changed_predicate_over_260_people_leaves_membership_equal_to_a_fresh_evaluation() {
    let fx = Fx::new("rechange", vec![]);
    let tiers = ["gold", "silver", "bronze"];
    for n in 0..260 {
        fx.create_member(T0 + n, &format!("m{n:03}"), tiers[(n % 3) as usize], None);
    }
    fx.define(T0 + 1_000, &gold_definition());
    fx.drain(T0 + 2_000);
    assert_eq!(fx.members(gold_id()).len(), 87);
    // The predicate changes: the same segment now holds the silver tier. The change leaves one
    // all-scope continuation, and nothing is re-evaluated until the worker runs it.
    let mut silver = gold_definition();
    silver.predicate = expr::parse("record.tier == 'silver'").unwrap();
    fx.define(T0 + 3_000, &silver);
    assert_eq!(fx.app.store.segment_batches(&fx.tenant()).unwrap().len(), 1);
    fx.drain(T0 + 4_000);
    // The members now equal a fresh evaluation of the new predicate over every record, and no
    // continuation is left.
    let state = fx.app.store.load_complete(&fx.tenant()).unwrap();
    let fresh: BTreeSet<String> = state
        .tenant
        .unwrap()
        .records_of(&object("member"))
        .filter(|r| r.get(&field("tier")) == &text("silver"))
        .filter_map(|r| match r.get(&field("principal")) {
            Value::Str(person) => Some(person.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(fresh.len(), 87);
    let members = fx.members(gold_id());
    assert_eq!(members.len(), fresh.len(), "no person is a member twice");
    assert_eq!(members.into_iter().collect::<BTreeSet<_>>(), fresh);
    assert!(fx
        .app
        .store
        .segment_batches(&fx.tenant())
        .unwrap()
        .is_empty());
    // The gold members left and the silver ones entered, each once.
    let events = fx.events();
    let exited = events
        .iter()
        .filter(|(_, e)| e.event.direction == SegmentDirection::Exited)
        .count();
    assert_eq!(exited, 87);
    assert_eq!(events.len(), 87 + 87 + 87);
}

// Verifies: SEG-036, SEG-037, SEG-038, PRV-353
#[test]
fn an_erasure_through_the_real_path_ends_the_memberships_sealed_and_a_later_change_does_not_re_enter(
) {
    let fx = Fx::new("erased", extras());
    // The person is known to the tenant by a role, so that the erasure finds them.
    fx.assign(T0, "m1", "greeter");
    let m1 = fx.create_member(T0 + 1, "m1", "gold", None);
    fx.create_member(T0 + 2, "m2", "gold", None);
    fx.define(T0 + 3, &gold_definition());
    fx.drain(T0 + 4);
    assert_eq!(fx.members(gold_id()), [fx.person("m1"), fx.person("m2")]);
    let person = principal_id(&fx.person("m1"));

    let receipt = strata_backend::erasure::erase_subject(&fx.app, &fx.ctx(T0 + 10), &person)
        .expect("the erasure runs");
    assert_eq!(receipt.principal, fx.person("m1"));

    // The membership of the person is gone and the other person's is not.
    assert_eq!(fx.members(gold_id()), [fx.person("m2")]);
    // The exit names the person sealed, with the reason erasure, and never by their own id.
    let exits: Vec<_> = fx
        .events()
        .into_iter()
        .map(|(_, logged)| logged.event)
        .filter(|event| event.direction == SegmentDirection::Exited)
        .collect();
    assert_eq!(exits.len(), 1);
    assert_eq!(exits[0].reason, seg::SegmentReason::Erasure);
    assert!(
        exits[0].person.as_str().starts_with("sealed:"),
        "{:?}",
        exits[0]
    );
    assert!(fx
        .events()
        .iter()
        .skip(2)
        .all(|(_, logged)| logged.event.person.as_str() != fx.person("m1")));

    // A later change that would make the person a member again does not: the person is marked.
    fx.set_tier(T0 + 20, m1, "silver");
    fx.set_tier(T0 + 21, m1, "gold");
    fx.drain(T0 + 30);
    assert_eq!(fx.members(gold_id()), [fx.person("m2")]);
    assert_eq!(
        fx.events()
            .iter()
            .filter(|(_, logged)| logged.event.direction == SegmentDirection::Entered)
            .count(),
        2,
        "no entry after the two first"
    );
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
        Selector::from_request(None, None, Some(vec![gold_id().to_string()]), None, None).unwrap();
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
        .filter(|r| r.get(&field("tier")) == &text("gold"))
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
