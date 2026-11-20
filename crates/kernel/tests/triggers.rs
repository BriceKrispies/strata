//! Running triggers, the kernel's half (`strata_kernel::triggers`): which events a trigger
//! matches, the effect it hands the automation worker, the authority its condition is decided
//! with, the hop limit, the run key, the definer of a definition and the suspension of an
//! automation whose definer lost authority, and the transitions a record-updated event names.
//! One rule per test.

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::rules::roles_of;
use strata_kernel::testkit::*;
use strata_kernel::triggers::{
    AutomationDecline, AutomationExceptionReason, AutomationStart, HOP_LIMIT,
};
use strata_kernel::*;

// ---------------------------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------------------------

fn step_in(
    world: &World,
    tenant: &str,
    who: &str,
    now: i64,
    intent: Intent,
) -> Result<(World, Transition), DomainError> {
    world.step(&intent, &ctx(tenant, user(&format!("{tenant}:{who}")), now))
}

fn must(world: &World, who: &str, now: i64, intent: Intent) -> (World, Transition) {
    step_in(world, "alpha", who, now, intent).expect("accepted")
}

fn change(changes: Vec<SchemaChange>) -> Intent {
    Intent::ChangeSchema {
        changes,
        expected_version: None,
        existing: ExistingData::Reject,
    }
}

fn schema(world: &World) -> TenantSchema {
    TenantSchema::clone(&world.tenants[&tenant_id("alpha")].schema)
}

fn award_id() -> WorkflowId {
    WorkflowId::new("award").unwrap()
}

/// An automation of one step that ends, acting with `run_as`.
fn flow(run_as: &[&str]) -> AutomationDef {
    let done = StepName::new("done").unwrap();
    AutomationDef {
        id: award_id(),
        label: "Award".into(),
        run_as: RunAs {
            roles: run_as.iter().map(|r| role(r)).collect(),
        },
        run_key: None,
        start: done.clone(),
        steps: BTreeMap::from([(
            done,
            AutomationStepDef {
                action: AutomationStep::End,
                next: None,
                retry: None,
                timeout: None,
                compensate: None,
            },
        )]),
        defined_by: None,
        defined_at: None,
        suspended_for: None,
        state: None,
    }
}

fn trigger(id: &str, on: TriggerEvent, condition: Option<Expr>) -> TriggerDef {
    TriggerDef {
        id: TriggerId::new(id).unwrap(),
        on,
        condition,
        automation: award_id(),
        paused: None,
        defined_by: None,
        defined_at: None,
    }
}

fn on_transition(name: &str) -> TriggerEvent {
    TriggerEvent::RecordUpdated {
        object: object("note"),
        transition: Some(name.into()),
    }
}

fn on_note_created() -> TriggerEvent {
    TriggerEvent::RecordCreated {
        object: object("note"),
    }
}

fn published() -> Expr {
    eq(var(Scope::New, "status"), lit("published"))
}

/// The notes world with `definitions` defined by the owner.
fn defined(definitions: Vec<SchemaChange>) -> World {
    must(&notes_world(), "owner", 6_000, change(definitions)).0
}

fn set_status(id: u64, status: &str) -> Intent {
    Intent::UpdateRecord {
        object: object("note"),
        id: RecordId(id),
        fields: BTreeMap::from([(field("status"), text(status))]),
        expected_version: None,
    }
}

fn starts(outcome: &Transition) -> Vec<&AutomationStart> {
    outcome
        .effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::StartAutomation(start) => Some(start),
            _ => None,
        })
        .collect()
}

fn declines(outcome: &Transition) -> Vec<&AutomationDecline> {
    outcome
        .effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::DeclineAutomation(decline) => Some(decline),
            _ => None,
        })
        .collect()
}

/// A world with a draft note 1 written by writer1, and the publication of it by the editor.
fn publish_a_note(world: &World) -> (World, Transition) {
    let (world, _) = must(world, "writer1", 7_000, new_note("Draft", "draft"));
    must(&world, "editor", 7_001, set_status(1, "published"))
}

/// The seq of the record-updated event of an outcome.
fn update_seq(outcome: &Transition) -> u64 {
    outcome
        .events
        .iter()
        .find(|event| matches!(event.body, EventBody::RecordUpdated { .. }))
        .expect("an update event")
        .seq
}

// ---------------------------------------------------------------------------------------------
// Matching and starting.
// ---------------------------------------------------------------------------------------------

// Verifies: WFL-1521
#[test]
fn a_true_condition_on_the_fired_transition_hands_over_one_run_start() {
    let world = defined(vec![
        SchemaChange::DefineAutomation(flow(&["editor"])),
        SchemaChange::DefineTrigger(trigger(
            "on_publish",
            on_transition("publish"),
            Some(published()),
        )),
    ]);
    let version = schema(&world).version;
    let (_, outcome) = publish_a_note(&world);
    let seq = update_seq(&outcome);
    assert_eq!(
        starts(&outcome),
        vec![&AutomationStart {
            trigger: TriggerId::new("on_publish").unwrap(),
            automation: award_id(),
            event_seq: seq,
            hop: 0,
            caused_by: principal_id("automation:award"),
            on_behalf_of: Some(principal_id("alpha:owner")),
            definition_version: version,
            run_key: None,
            params: BTreeMap::from([
                ("at".to_string(), Value::Time(7_001)),
                ("hop".to_string(), Value::Int(0)),
                ("seq".to_string(), Value::Int(seq as i64)),
                ("transition".to_string(), text("publish")),
            ]),
        }]
    );
    assert!(declines(&outcome).is_empty());
}

// Verifies: WFL-1521
#[test]
fn a_false_condition_starts_no_run_and_records_nothing() {
    let rank_99 = eq(var(Scope::New, "rank"), Expr::Lit(Value::Int(99)));
    let world = defined(vec![
        SchemaChange::DefineAutomation(flow(&["editor"])),
        SchemaChange::DefineTrigger(trigger(
            "on_publish",
            on_transition("publish"),
            Some(rank_99),
        )),
    ]);
    let (_, outcome) = publish_a_note(&world);
    assert!(starts(&outcome).is_empty());
    assert!(
        declines(&outcome).is_empty(),
        "a false condition writes nothing (D-6530)"
    );
}

#[test]
fn a_paused_trigger_starts_no_run() {
    let mut paused = trigger("on_publish", on_transition("publish"), None);
    paused.paused = Some(true);
    let world = defined(vec![
        SchemaChange::DefineAutomation(flow(&["editor"])),
        SchemaChange::DefineTrigger(paused),
    ]);
    let (_, outcome) = publish_a_note(&world);
    assert!(starts(&outcome).is_empty() && declines(&outcome).is_empty());
}

// Verifies: WFL-1539
#[test]
fn a_trigger_on_one_transition_does_not_match_another() {
    let world = defined(vec![
        SchemaChange::DefineAutomation(flow(&["editor"])),
        SchemaChange::DefineTrigger(trigger("on_unpublish", on_transition("unpublish"), None)),
    ]);
    let (world, outcome) = publish_a_note(&world);
    assert!(starts(&outcome).is_empty(), "publish is not unpublish");
    let (_, outcome) = must(&world, "editor", 7_002, set_status(1, "draft"));
    assert_eq!(starts(&outcome).len(), 1);
}

#[test]
fn no_trigger_fires_on_an_event_of_another_tenant() {
    let world = defined(vec![
        SchemaChange::DefineAutomation(flow(&["editor"])),
        SchemaChange::DefineTrigger(trigger("on_publish", on_transition("publish"), None)),
    ]);
    // Beta has the same notes model and no trigger; its publication starts nothing.
    let (world, _) = step_in(&world, "beta", "writer1", 7_000, new_note("Draft", "draft")).unwrap();
    let beta_note = world.tenants[&tenant_id("beta")].next_record - 1;
    let (world, outcome) = step_in(
        &world,
        "beta",
        "editor",
        7_001,
        set_status(beta_note, "published"),
    )
    .unwrap();
    assert!(starts(&outcome).is_empty() && declines(&outcome).is_empty());
    // Alpha's publication starts a run for alpha's event only.
    let (_, outcome) = publish_a_note(&world);
    assert_eq!(starts(&outcome).len(), 1);
    assert!(outcome
        .events
        .iter()
        .all(|e| e.tenant == tenant_id("alpha")));
}

/// Triggers are decided in id order, one effect each.
#[test]
fn every_matching_trigger_hands_over_its_own_effect_in_id_order() {
    let world = defined(vec![
        SchemaChange::DefineAutomation(flow(&["editor"])),
        SchemaChange::DefineTrigger(trigger("b_second", on_transition("publish"), None)),
        SchemaChange::DefineTrigger(trigger("a_first", on_transition("publish"), None)),
    ]);
    let (_, outcome) = publish_a_note(&world);
    let names: Vec<&str> = starts(&outcome)
        .iter()
        .map(|s| s.trigger.as_str())
        .collect();
    assert_eq!(names, ["a_first", "b_second"]);
}

// ---------------------------------------------------------------------------------------------
// The record-updated event names the transition.
// ---------------------------------------------------------------------------------------------

// Verifies: SCH-2202
#[test]
fn a_record_updated_event_names_the_transition_each_workflow_fired() {
    let (world, outcome) = publish_a_note(&notes_world());
    let names = |outcome: &Transition| match &outcome.events[0].body {
        EventBody::RecordUpdated { transitions, .. } => transitions.clone(),
        other => panic!("not an update: {other:?}"),
    };
    assert_eq!(
        names(&outcome),
        Some(BTreeMap::from([(
            WorkflowId::new("note_status").unwrap(),
            "publish".to_string()
        )]))
    );
    let rank = Intent::UpdateRecord {
        object: object("note"),
        id: RecordId(1),
        fields: BTreeMap::from([(field("rank"), Value::Int(2))]),
        expected_version: None,
    };
    let (_, outcome) = must(&world, "editor", 7_002, rank);
    assert_eq!(
        names(&outcome),
        None,
        "an update that moves no workflow names none"
    );
}

// ---------------------------------------------------------------------------------------------
// Authority of the condition.
// ---------------------------------------------------------------------------------------------

// Verifies: WFL-2200
#[test]
fn a_trigger_does_not_observe_a_record_its_automation_principal_may_not_read() {
    // With no run-as role the automation principal holds public only, which reads published
    // notes and not drafts.
    let world = defined(vec![
        SchemaChange::DefineAutomation(flow(&[])),
        SchemaChange::DefineTrigger(trigger("on_note", on_note_created(), None)),
    ]);
    let (_, outcome) = must(&world, "writer1", 7_000, new_note("Draft", "draft"));
    assert!(starts(&outcome).is_empty() && declines(&outcome).is_empty());

    // A writer reads every note.
    let world = defined(vec![
        SchemaChange::DefineAutomation(flow(&["writer"])),
        SchemaChange::DefineTrigger(trigger("on_note", on_note_created(), None)),
    ]);
    let (_, outcome) = must(&world, "writer1", 7_000, new_note("Draft", "draft"));
    assert_eq!(starts(&outcome).len(), 1);
}

// Verifies: WFL-2200
#[test]
fn a_condition_that_reads_a_field_the_automation_may_not_read_declines_with_condition_failed() {
    // A person's name is readable by editors only; the automation holds public.
    let hidden_name = SchemaChange::DefineField {
        object: object("person"),
        field: FieldDef {
            read: Some(Expr::HasRole(role("editor"))),
            ..field_def("name", FieldType::Str, true)
        },
    };
    let author_is_ada = eq(
        Expr::Through(Scope::New, "author".into(), "name".into()),
        lit("Ada"),
    );
    let world = defined(vec![
        hidden_name,
        SchemaChange::DefineAutomation(flow(&[])),
        SchemaChange::DefineTrigger(trigger(
            "on_publish",
            on_transition("publish"),
            Some(author_is_ada),
        )),
    ]);
    let person = Intent::CreateRecord {
        object: object("person"),
        fields: BTreeMap::from([(field("name"), text("Ada"))]),
    };
    let (world, _) = must(&world, "editor", 6_900, person);
    let note = Intent::CreateRecord {
        object: object("note"),
        fields: BTreeMap::from([
            (field("title"), text("Draft")),
            (field("slug"), text("draft")),
            (field("author"), Value::Ref(RecordId(1))),
        ]),
    };
    let (world, _) = must(&world, "writer1", 7_000, note);
    let (_, outcome) = must(&world, "editor", 7_001, set_status(2, "published"));
    assert!(starts(&outcome).is_empty());
    let declined = declines(&outcome);
    assert_eq!(declined.len(), 1);
    assert_eq!(
        declined[0].reason,
        AutomationExceptionReason::ConditionFailed
    );
    assert!(
        !declined[0].detail.contains("Ada"),
        "a detail names ids, never a value"
    );
}

/// A condition that fails to evaluate (here an integer overflow, D-6540) is a declined exception
/// with reason condition-failed, never a silent false (D-6524).
#[test]
fn a_condition_that_cannot_be_evaluated_declines_with_condition_failed() {
    let overflow = Expr::Cmp(
        CmpOp::Gt,
        Box::new(Expr::Arith(
            ArithOp::Mul,
            Box::new(var(Scope::New, "rank")),
            Box::new(Expr::Lit(Value::Int(9_007_199_254_740_991))),
        )),
        Box::new(Expr::Lit(Value::Int(0))),
    );
    let world = defined(vec![
        SchemaChange::DefineAutomation(flow(&["writer"])),
        SchemaChange::DefineTrigger(trigger("on_note", on_note_created(), Some(overflow))),
    ]);
    let note = Intent::CreateRecord {
        object: object("note"),
        fields: BTreeMap::from([
            (field("title"), text("Draft")),
            (field("slug"), text("draft")),
            (field("rank"), Value::Int(2)),
        ]),
    };
    let (_, outcome) = must(&world, "writer1", 7_000, note);
    assert!(starts(&outcome).is_empty());
    let declined = declines(&outcome);
    assert_eq!(declined.len(), 1);
    assert_eq!(
        declined[0].reason,
        AutomationExceptionReason::ConditionFailed
    );
}

// Verifies: SCH-2203
#[test]
fn a_condition_that_needs_a_record_the_snapshot_lacks_asks_for_it() {
    let person = |name: &str| Intent::CreateRecord {
        object: object("person"),
        fields: BTreeMap::from([(field("name"), text(name))]),
    };
    let world = notes_world();
    let (world, _) = must(&world, "editor", 6_100, person("Ada"));
    let (world, _) = must(&world, "editor", 6_101, person("Bob"));
    let note = Intent::CreateRecord {
        object: object("note"),
        fields: BTreeMap::from([
            (field("title"), text("Note")),
            (field("slug"), text("note")),
            (field("author"), Value::Ref(RecordId(1))),
        ]),
    };
    let (world, _) = must(&world, "writer1", 6_102, note);
    let (world, _) = must(&world, "editor", 6_103, set_status(3, "published"));
    // A trigger on any update of a note, whose condition reads the author the note had before.
    let was_ada = eq(
        Expr::Through(Scope::Old, "author".into(), "name".into()),
        lit("Ada"),
    );
    let on_update = TriggerEvent::RecordUpdated {
        object: object("note"),
        transition: None,
    };
    let (world, _) = must(
        &world,
        "owner",
        6_200,
        change(vec![
            SchemaChange::DefineAutomation(flow(&[])),
            SchemaChange::DefineTrigger(trigger("on_update", on_update, Some(was_ada))),
        ]),
    );
    let reassign = Intent::UpdateRecord {
        object: object("note"),
        id: RecordId(3),
        fields: BTreeMap::from([(field("author"), Value::Ref(RecordId(2)))]),
        expected_version: None,
    };
    let context = ctx("alpha", user("alpha:editor"), 7_000);
    // A snapshot that holds what the update itself needs and not the author it had before.
    let complete = world.state(&tenant_id("alpha"));
    let tenant = complete.tenant.clone().unwrap();
    let mut loaded: BTreeSet<Need> = needs(Some(&*tenant.schema), &reassign)
        .into_iter()
        .collect();
    loaded.extend(refine_needs(&complete, &reassign));
    let partial = State {
        tenant_id: tenant_id("alpha"),
        tenant: Some(tenant),
        coverage: Coverage::Partial(loaded),
    };
    let error = transition(&partial, &reassign, &context).expect_err("the author is not loaded");
    assert_eq!(error.kind, ErrorKind::InsufficientState);
    assert_eq!(
        error.missing,
        Some(Need::Record {
            object: object("person"),
            id: RecordId(1),
        })
    );
    // With it loaded, the condition is decided and holds.
    let outcome = transition(&complete, &reassign, &context).expect("accepted");
    assert_eq!(starts(&outcome).len(), 1);
}

// ---------------------------------------------------------------------------------------------
// Hop limit and run key.
// ---------------------------------------------------------------------------------------------

// Verifies: SCH-300
#[test]
fn past_the_hop_limit_a_trigger_declines_with_hop_limit() {
    let world = defined(vec![
        SchemaChange::DefineAutomation(flow(&["editor"])),
        SchemaChange::DefineTrigger(trigger("on_publish", on_transition("publish"), None)),
    ]);
    let (world, _) = must(&world, "writer1", 7_000, new_note("Draft", "draft"));
    let at_hop = |hop: u32| {
        let mut context = ctx("alpha", user("alpha:editor"), 7_001);
        context.hop = Some(hop);
        world
            .step(&set_status(1, "published"), &context)
            .expect("the change itself always commits")
            .1
    };
    let within = at_hop(HOP_LIMIT);
    assert_eq!(starts(&within).len(), 1);
    assert_eq!(starts(&within)[0].hop, HOP_LIMIT);
    let past = at_hop(HOP_LIMIT + 1);
    assert!(starts(&past).is_empty());
    let declined = declines(&past);
    assert_eq!(declined.len(), 1);
    assert_eq!(declined[0].reason, AutomationExceptionReason::HopLimit);
    assert_eq!(declined[0].hop, HOP_LIMIT + 1);
}

// Verifies: SCH-1533
#[test]
fn a_run_carries_the_hop_count_of_the_event_that_started_it() {
    let world = defined(vec![
        SchemaChange::DefineAutomation(flow(&["editor"])),
        SchemaChange::DefineTrigger(trigger("on_publish", on_transition("publish"), None)),
    ]);
    let (world, _) = must(&world, "writer1", 7_000, new_note("Draft", "draft"));
    let mut context = ctx("alpha", user("alpha:editor"), 7_001);
    context.hop = Some(3);
    let (_, outcome) = world.step(&set_status(1, "published"), &context).unwrap();
    let started = starts(&outcome);
    assert_eq!(started[0].hop, 3);
    assert_eq!(started[0].params["hop"], Value::Int(3));
}

// Verifies: SCH-1536
#[test]
fn a_run_key_is_handed_over_as_the_hash_of_its_value_so_a_repeat_is_recognised() {
    let mut keyed = flow(&["editor"]);
    keyed.run_key = Some(Expr::Param("transition".into()));
    let world = defined(vec![
        SchemaChange::DefineAutomation(keyed),
        SchemaChange::DefineTrigger(trigger("on_publish", on_transition("publish"), None)),
    ]);
    let (world, first) = publish_a_note(&world);
    let (world, _) = must(&world, "editor", 7_002, set_status(1, "draft"));
    let (_, second) = must(&world, "editor", 7_003, set_status(1, "published"));
    let expected = Some(canon::hash_of(&text("publish")));
    assert_eq!(starts(&first)[0].run_key, expected);
    assert_eq!(
        starts(&second)[0].run_key,
        expected,
        "the same key value hands over the same hash, which the worker starts once (D-6534)"
    );
}

// ---------------------------------------------------------------------------------------------
// The definer and the suspension.
// ---------------------------------------------------------------------------------------------

/// The notes world in which writers and editors also manage the schema and hold
/// automations-define, and the editor has defined `award` acting as `run_as`, started by every
/// new note.
fn defined_by_editor(run_as: &[&str]) -> World {
    let manage = |id: &str, role_name: &str| {
        SchemaChange::DefinePolicy(PolicyDef {
            id: PolicyId::new(id).unwrap(),
            role: role(role_name),
            resource: Resource::Schema,
            actions: BTreeSet::from([Action::Manage]),
            condition: None,
        })
    };
    let mut changes = vec![
        manage("writers_manage_schema", "writer"),
        manage("editors_manage_schema", "editor"),
    ];
    for role_name in ["writer", "editor"] {
        changes.push(SchemaChange::DefineRolePermissions {
            role: role(role_name),
            permissions: vec![Permission::Capability("automations-define".into())],
        });
    }
    let world = must(&notes_world(), "owner", 5_000, change(changes)).0;
    must(
        &world,
        "editor",
        6_000,
        change(vec![
            SchemaChange::DefineAutomation(flow(run_as)),
            SchemaChange::DefineTrigger(trigger("on_note", on_note_created(), None)),
        ]),
    )
    .0
}

fn award(world: &World) -> AutomationDef {
    schema(world).automations.unwrap()[&award_id()].clone()
}

fn suspensions(outcome: &Transition) -> Vec<&EventBody> {
    outcome
        .events
        .iter()
        .map(|event| &event.body)
        .filter(|body| matches!(body, EventBody::AutomationSuspended { .. }))
        .collect()
}

// Verifies: WFL-2201
#[test]
fn the_definer_and_the_time_come_from_the_context_and_a_supplied_value_is_ignored() {
    let mut supplied = flow(&["editor"]);
    supplied.defined_by = Some(principal_id("alpha:stranger"));
    supplied.defined_at = Some(Timestamp(1));
    supplied.suspended_for = Some(role("editor"));
    let mut named = trigger("on_note", on_note_created(), None);
    named.defined_by = Some(principal_id("alpha:stranger"));
    let world = defined(vec![
        SchemaChange::DefineAutomation(supplied),
        SchemaChange::DefineTrigger(named),
    ]);
    let held = award(&world);
    assert_eq!(held.defined_by, Some(principal_id("alpha:owner")));
    assert_eq!(held.defined_at, Some(Timestamp(6_000)));
    assert_eq!(held.suspended_for, None);
    let triggers = schema(&world).triggers.unwrap();
    let held_trigger = &triggers[&TriggerId::new("on_note").unwrap()];
    assert_eq!(held_trigger.defined_by, Some(principal_id("alpha:owner")));
    assert_eq!(held_trigger.defined_at, Some(Timestamp(6_000)));
}

// Verifies: WFL-1529
#[test]
fn revoking_the_definers_role_suspends_the_automation_in_the_same_transition() {
    let world = defined_by_editor(&["editor"]);
    assert_eq!(award(&world).defined_by, Some(principal_id("alpha:editor")));
    let revoke = Intent::RevokeRole {
        principal: principal_id("alpha:editor"),
        role: role("editor"),
    };
    let (world, outcome) = must(&world, "owner", 7_000, revoke);
    assert_eq!(award(&world).suspended_for, Some(role("editor")));
    assert_eq!(
        suspensions(&outcome),
        vec![&EventBody::AutomationSuspended {
            automation: award_id(),
            definer: principal_id("alpha:editor"),
            role: role("editor"),
        }]
    );
    // The event is numbered after the transition's own and is published like them.
    let last = outcome.events.last().unwrap();
    assert!(matches!(last.body, EventBody::AutomationSuspended { .. }));
    assert_eq!(outcome.delta.event_seq, last.seq);
    assert!(outcome.effects.contains(&Effect::Publish { seq: last.seq }));
}

// Verifies: WFL-1529
#[test]
fn removing_a_policy_the_definer_relied_on_suspends_the_automation() {
    // The editor may confer writer while editors manage people; writers add people.
    let world = defined_by_editor(&["writer"]);
    assert_eq!(award(&world).suspended_for, None);
    let (world, outcome) = must(
        &world,
        "owner",
        7_000,
        change(vec![SchemaChange::RemovePolicy(
            PolicyId::new("editors_manage_people").unwrap(),
        )]),
    );
    assert_eq!(award(&world).suspended_for, Some(role("writer")));
    assert_eq!(suspensions(&outcome).len(), 1);
}

// Verifies: WFL-1529
#[test]
fn widening_a_run_as_role_beyond_the_definer_suspends_the_automation() {
    let world = defined_by_editor(&["writer"]);
    let widen = SchemaChange::DefineRolePermissions {
        role: role("writer"),
        permissions: vec![
            Permission::Capability("automations-define".into()),
            Permission::Capability("ledger-post".into()),
        ],
    };
    let (world, _) = must(&world, "owner", 7_000, change(vec![widen]));
    assert_eq!(award(&world).suspended_for, Some(role("writer")));
}

#[test]
fn a_change_that_keeps_the_definers_authority_suspends_nothing() {
    let world = defined_by_editor(&["editor"]);
    let assign = Intent::AssignRole {
        principal: principal_id("alpha:stranger"),
        role: role("writer"),
    };
    let (world, outcome) = must(&world, "owner", 7_000, assign);
    assert!(suspensions(&outcome).is_empty());
    assert_eq!(award(&world).suspended_for, None);
}

// Verifies: WFL-1529
#[test]
fn a_suspended_automation_declines_its_triggers_with_run_as_suspended() {
    let world = defined_by_editor(&["editor"]);
    let revoke = Intent::RevokeRole {
        principal: principal_id("alpha:editor"),
        role: role("editor"),
    };
    let (world, _) = must(&world, "owner", 7_000, revoke);
    let (_, outcome) = must(&world, "writer1", 7_001, new_note("Draft", "draft"));
    assert!(starts(&outcome).is_empty());
    let declined = declines(&outcome);
    assert_eq!(declined.len(), 1);
    assert_eq!(
        declined[0].reason,
        AutomationExceptionReason::RunAsSuspended
    );
}

// Verifies: WFL-1529
#[test]
fn defining_it_again_with_authority_lifts_the_suspension() {
    let world = defined_by_editor(&["editor"]);
    let revoke = Intent::RevokeRole {
        principal: principal_id("alpha:editor"),
        role: role("editor"),
    };
    let (world, _) = must(&world, "owner", 7_000, revoke);
    let (world, _) = must(
        &world,
        "owner",
        7_001,
        change(vec![SchemaChange::DefineAutomation(flow(&["editor"]))]),
    );
    let held = award(&world);
    assert_eq!(held.suspended_for, None);
    assert_eq!(held.defined_by, Some(principal_id("alpha:owner")));
    let (_, outcome) = must(&world, "writer1", 7_002, new_note("Draft", "draft"));
    let started = starts(&outcome);
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].on_behalf_of, Some(principal_id("alpha:owner")));
}

// Verifies: WFL-1527
#[test]
fn an_automation_principal_holds_its_run_as_roles_and_no_other() {
    let world = defined_by_editor(&["editor"]);
    let automation = Principal::User(principal_id("automation:award"));
    let held = roles_of(&world.tenants[&tenant_id("alpha")], &automation);
    assert_eq!(held, BTreeSet::from([role("editor"), public_role()]));
    let revoke = Intent::RevokeRole {
        principal: principal_id("alpha:editor"),
        role: role("editor"),
    };
    let (world, _) = must(&world, "owner", 7_000, revoke);
    let held = roles_of(&world.tenants[&tenant_id("alpha")], &automation);
    assert_eq!(
        held,
        BTreeSet::from([public_role()]),
        "a suspended automation holds none"
    );
    // Nobody is made a member under an automation principal's id.
    let assign = Intent::AssignRole {
        principal: principal_id("automation:award"),
        role: role("writer"),
    };
    let error = step_in(&world, "alpha", "owner", 7_001, assign).expect_err("reserved");
    assert_eq!(error.kind, ErrorKind::InvalidIntent);
}

// ---------------------------------------------------------------------------------------------
// A trigger on the ledger (D-6523, D-7903).
// ---------------------------------------------------------------------------------------------

fn param_cmp(op: CmpOp, name: &str, limit: i64) -> Expr {
    Expr::Cmp(
        op,
        Box::new(Expr::Param(name.into())),
        Box::new(Expr::Lit(Value::Int(limit))),
    )
}

fn points_unit() -> LedgerUnit {
    LedgerUnit::new("points").unwrap()
}

fn post_points(world: &World, now: i64, to: &str, amount: i64, key: &str) -> (World, Transition) {
    let posting = LedgerPostingRequest {
        owner: LedgerOwner::Principal {
            principal: principal_id(&format!("alpha:{to}")),
        },
        unit: points_unit(),
        amount: LedgerAmount(amount),
        idempotency_key: key.into(),
        reason: "bonus".into(),
        corrects: None,
        scope: None,
    };
    must(
        world,
        "owner",
        now,
        Intent::PostLedger {
            postings: vec![posting],
        },
    )
}

// Verifies: WFL-2901
#[test]
fn a_ledger_posting_starts_the_run_of_a_trigger_on_its_unit_with_the_balance_on_both_sides() {
    // Crossing 500: the balance before the entry is below it and the balance after is not.
    let crossing = Expr::And(vec![
        param_cmp(CmpOp::Lt, "balance_before", 500),
        param_cmp(CmpOp::Ge, "balance", 500),
    ]);
    let mut once = flow(&["editor"]);
    once.run_key = Some(Expr::Param("owner".into()));
    let world = defined(vec![
        SchemaChange::DeclareLedgerUnit(LedgerUnitDecl {
            expiry: None,
            unit: points_unit(),
            description: "Points".into(),
        }),
        SchemaChange::DefineAutomation(once),
        SchemaChange::DefineTrigger(trigger(
            "on_points",
            TriggerEvent::LedgerPosted {
                unit: points_unit(),
            },
            Some(crossing),
        )),
    ]);
    // 300 points: below the threshold, no run.
    let (world, first) = post_points(&world, 7_000, "writer1", 300, "a");
    assert!(starts(&first).is_empty() && declines(&first).is_empty());
    // 250 more: 300 before and 550 after, so the run starts, with the entry and both balances.
    let (world, second) = post_points(&world, 7_001, "writer1", 250, "b");
    let seq = second
        .events
        .iter()
        .find(|event| matches!(event.body, EventBody::LedgerEntryPosted { .. }))
        .expect("the posting is an event")
        .seq;
    let started = starts(&second);
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].trigger, TriggerId::new("on_points").unwrap());
    assert_eq!(
        started[0].params,
        BTreeMap::from([
            ("at".to_string(), Value::Time(7_001)),
            ("hop".to_string(), Value::Int(0)),
            ("seq".to_string(), Value::Int(seq as i64)),
            ("owner".to_string(), text("alpha:writer1")),
            ("unit".to_string(), text("points")),
            ("amount".to_string(), Value::Int(250)),
            ("balance".to_string(), Value::Int(550)),
            ("balance_before".to_string(), Value::Int(300)),
            ("entry_sequence".to_string(), Value::Int(2)),
            ("reason".to_string(), text("bonus")),
        ])
    );
    let key = Some(canon::hash_of(&text("alpha:writer1")));
    assert_eq!(started[0].run_key, key);
    // Down to 450 and up to 550 again crosses it again: the trigger fires, with the same run key,
    // which the worker starts once (D-6534).
    let (world, down) = post_points(&world, 7_002, "writer1", -100, "c");
    assert!(starts(&down).is_empty());
    let (world, up) = post_points(&world, 7_003, "writer1", 100, "d");
    assert_eq!(starts(&up).len(), 1);
    assert_eq!(starts(&up)[0].run_key, key);
    // Another person's crossing has its own key.
    let (_, other) = post_points(&world, 7_004, "writer2", 600, "e");
    assert_eq!(
        starts(&other)[0].run_key,
        Some(canon::hash_of(&text("alpha:writer2")))
    );
}

// Verifies: WFL-2901
#[test]
fn a_trigger_on_one_unit_does_not_match_a_posting_to_another() {
    let other = LedgerUnit::new("stars").unwrap();
    let world = defined(vec![
        SchemaChange::DeclareLedgerUnit(LedgerUnitDecl {
            expiry: None,
            unit: points_unit(),
            description: "Points".into(),
        }),
        SchemaChange::DeclareLedgerUnit(LedgerUnitDecl {
            expiry: None,
            unit: other.clone(),
            description: "Stars".into(),
        }),
        SchemaChange::DefineAutomation(flow(&["editor"])),
        SchemaChange::DefineTrigger(trigger(
            "on_stars",
            TriggerEvent::LedgerPosted { unit: other },
            None,
        )),
    ]);
    let (_, outcome) = post_points(&world, 7_000, "writer1", 10, "a");
    assert!(starts(&outcome).is_empty() && declines(&outcome).is_empty());
}
