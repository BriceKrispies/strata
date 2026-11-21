//! Running triggers, the kernel's half (`strata_kernel::triggers`): which events a trigger
//! matches, the effect it hands the automation worker, the authority its condition is decided
//! with, the hop limit, the run key, the definer of a definition and the suspension of an
//! automation whose definer lost authority, and the transitions a record-updated event names.
//! One rule per test.

use std::collections::{BTreeMap, BTreeSet};
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

// ---------------------------------------------------------------------------------------------
// Matching and starting.
// ---------------------------------------------------------------------------------------------

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
// Verifies: SCH-1533
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
    // The run carries the hop count of the event that started it.
    assert_eq!(
        starts(&within)[0].params["hop"],
        Value::Int(HOP_LIMIT as i64)
    );
    let past = at_hop(HOP_LIMIT + 1);
    assert!(starts(&past).is_empty());
    let declined = declines(&past);
    assert_eq!(declined.len(), 1);
    assert_eq!(declined[0].reason, AutomationExceptionReason::HopLimit);
    assert_eq!(declined[0].hop, HOP_LIMIT + 1);
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

// ---------------------------------------------------------------------------------------------
// A trigger on the ledger (D-6523, D-7903).
// ---------------------------------------------------------------------------------------------
