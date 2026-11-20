//! Triggers, automations, tenant operations and composite unique constraints as tenant metadata:
//! what the kernel holds, what it refuses when they are defined, and who may define them.
//! One rule per test.

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_model_core::types::{LedgerUnit, SegmentId};

// ---------------------------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------------------------

fn step(
    world: &World,
    who: &str,
    now: i64,
    intent: Intent,
) -> Result<(World, Transition), DomainError> {
    world.step(&intent, &ctx("alpha", user(&format!("alpha:{who}")), now))
}

fn must(world: &World, who: &str, now: i64, intent: Intent) -> World {
    step(world, who, now, intent).expect("accepted").0
}

fn change(changes: Vec<SchemaChange>) -> Intent {
    Intent::ChangeSchema {
        changes,
        expected_version: None,
        existing: ExistingData::Reject,
    }
}

fn refused(world: &World, who: &str, changes: Vec<SchemaChange>) -> DomainError {
    step(world, who, 9_000, change(changes)).expect_err("refused")
}

fn schema(world: &World) -> TenantSchema {
    TenantSchema::clone(&world.tenants[&tenant_id("alpha")].schema)
}

fn name(s: &str) -> StepName {
    StepName::new(s).expect("step name")
}

fn param(s: &str) -> Expr {
    Expr::Param(s.to_string())
}

fn int(n: i64) -> Expr {
    Expr::Lit(Value::Int(n))
}

fn ge(a: Expr, b: Expr) -> Expr {
    Expr::Cmp(CmpOp::Ge, Box::new(a), Box::new(b))
}

fn plain(action: AutomationStep, next: Option<&str>) -> AutomationStepDef {
    AutomationStepDef {
        action,
        next: next.map(name),
        retry: None,
        timeout: None,
        compensate: None,
    }
}

/// An automation that creates a note, looks at it and ends, acting with `run_as`.
fn award(run_as: &[&str]) -> AutomationDef {
    AutomationDef {
        id: WorkflowId::new("award").unwrap(),
        label: "Award".into(),
        run_as: RunAs {
            roles: run_as.iter().map(|r| role(r)).collect(),
        },
        run_key: Some(param("owner")),
        start: name("make"),
        steps: BTreeMap::from([
            (
                name("make"),
                plain(
                    AutomationStep::CreateRecord {
                        object: object("note"),
                        fields: BTreeMap::from([
                            (field("title"), lit("Silver")),
                            (field("slug"), param("owner")),
                        ]),
                    },
                    Some("check"),
                ),
            ),
            (
                name("check"),
                plain(
                    AutomationStep::Branch {
                        condition: eq(var(Scope::Record, "status"), lit("draft")),
                        object: Some(object("note")),
                        record: Some(param("make_record")),
                        then: name("done"),
                        r#else: name("done"),
                    },
                    None,
                ),
            ),
            (name("done"), plain(AutomationStep::End, None)),
        ]),
    }
}

fn on_publish() -> TriggerDef {
    TriggerDef {
        id: TriggerId::new("on_publish").unwrap(),
        on: TriggerEvent::RecordUpdated {
            object: object("note"),
            transition: Some("publish".into()),
        },
        condition: Some(eq(var(Scope::New, "status"), lit("published"))),
        automation: WorkflowId::new("award").unwrap(),
        paused: None,
    }
}

fn on_points(id: &str) -> TriggerDef {
    TriggerDef {
        id: TriggerId::new(id).unwrap(),
        on: TriggerEvent::LedgerPosted {
            unit: LedgerUnit::new("points").unwrap(),
        },
        condition: Some(ge(param("balance"), int(500))),
        automation: WorkflowId::new("award").unwrap(),
        paused: None,
    }
}

/// A tenant operation that copies a draft note (its subject) and ranks the copy.
fn redeem() -> TenantOperationDef {
    TenantOperationDef {
        id: TenantOperationId::new("redeem").unwrap(),
        label: "Redeem".into(),
        inputs: vec![TenantOperationInput {
            name: "note".into(),
            ty: FieldType::Ref {
                target: object("note"),
                on_delete: OnDelete::Restrict,
            },
            required: true,
            label: None,
        }],
        invokers: TenantOperationInvokers {
            signed_in: true,
            roles: BTreeSet::new(),
        },
        subject: Some(TenantOperationSubject {
            object: object("note"),
            input: "note".into(),
        }),
        conditions: vec![TenantOperationCondition {
            id: "drafted".into(),
            expr: eq(var(Scope::Record, "status"), lit("draft")),
            error: "not-draft".into(),
            message: "Only a draft is redeemed.".into(),
        }],
        steps: vec![
            TenantOperationStep::CreateRecord {
                id: "copy".into(),
                object: object("note"),
                fields: BTreeMap::from([
                    (field("title"), var(Scope::Record, "title")),
                    (field("slug"), lit("copy")),
                ]),
            },
            TenantOperationStep::UpdateRecord {
                id: "touch".into(),
                object: object("note"),
                record: param("copy"),
                fields: BTreeMap::from([(field("rank"), int(1))]),
            },
        ],
    }
}

fn posting(id: &str) -> TenantOperationStep {
    TenantOperationStep::PostLedger {
        id: id.into(),
        owner: TenantOperationPostingOwner::Invoker,
        unit: LedgerUnit::new("points").unwrap(),
        scope: None,
        amount: int(1),
        reason: "redemption".into(),
    }
}

fn title_rank() -> UniqueDef {
    UniqueDef {
        id: ConstraintId::new("title_rank").unwrap(),
        fields: vec![field("title"), field("rank")],
        message: "One note per title and rank".into(),
    }
}

fn define_unique(unique: UniqueDef) -> SchemaChange {
    SchemaChange::DefineUnique {
        object: object("note"),
        unique,
    }
}

/// The notes world in which writers and editors also manage the schema; `capability` gives each
/// of them automations-define as well.
fn definers(capability: bool) -> World {
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
    if capability {
        for role_name in ["writer", "editor"] {
            changes.push(SchemaChange::DefineRolePermissions {
                role: role(role_name),
                permissions: vec![Permission::Capability("automations-define".into())],
            });
        }
    }
    must(&notes_world(), "owner", 5_000, change(changes))
}

fn note_with(title: &str, slug: &str, rank: Option<i64>) -> Intent {
    let mut fields = BTreeMap::from([(field("title"), text(title)), (field("slug"), text(slug))]);
    if let Some(rank) = rank {
        fields.insert(field("rank"), Value::Int(rank));
    }
    Intent::CreateRecord {
        object: object("note"),
        fields,
    }
}

// ---------------------------------------------------------------------------------------------
// Holding the definitions.
// ---------------------------------------------------------------------------------------------

// Verifies: WFL-1520
#[test]
fn a_trigger_and_an_automation_are_held_under_a_new_schema_version() {
    let world = notes_world();
    let before = schema(&world).version;
    let (world, outcome) = step(
        &world,
        "owner",
        6_000,
        change(vec![
            SchemaChange::DefineAutomation(award(&["writer"])),
            SchemaChange::DefineTrigger(on_publish()),
        ]),
    )
    .unwrap();
    let after = schema(&world);
    assert_eq!(after.version, SchemaVersion(before.0 + 1));
    assert_eq!(
        outcome.output,
        Output::Schema {
            version: after.version
        }
    );
    assert_eq!(
        after.automations.as_ref().unwrap()[&WorkflowId::new("award").unwrap()],
        award(&["writer"])
    );
    assert_eq!(
        after.triggers.as_ref().unwrap()[&TriggerId::new("on_publish").unwrap()],
        on_publish()
    );
}

#[test]
fn a_schema_without_definitions_writes_no_member_for_them_and_decodes_without_them() {
    let world = notes_world();
    let text = serde_json::to_string(&schema(&world)).unwrap();
    for member in ["triggers", "automations", "operations", "\"unique\":{"] {
        assert!(!text.contains(member), "{member} in {text}");
    }
    let decoded: TenantSchema = serde_json::from_str(&text).unwrap();
    assert!(decoded.triggers.is_none() && decoded.automations.is_none());
    assert!(decoded.operations.is_none());
}

#[test]
fn removing_every_definition_restores_the_hash_of_the_schema_before_them() {
    let world = notes_world();
    let before = canon::hash_of(&TenantSchema {
        version: SchemaVersion(0),
        ..schema(&world)
    });
    let world = must(
        &world,
        "owner",
        6_000,
        change(vec![
            SchemaChange::DefineAutomation(award(&["writer"])),
            SchemaChange::DefineTrigger(on_publish()),
            SchemaChange::DefineOperation(redeem()),
            define_unique(title_rank()),
        ]),
    );
    let world = must(
        &world,
        "owner",
        6_001,
        change(vec![
            SchemaChange::RemoveTrigger(TriggerId::new("on_publish").unwrap()),
            SchemaChange::RemoveAutomation(WorkflowId::new("award").unwrap()),
            SchemaChange::RemoveOperation(TenantOperationId::new("redeem").unwrap()),
            SchemaChange::RemoveUnique {
                object: object("note"),
                unique: ConstraintId::new("title_rank").unwrap(),
            },
        ]),
    );
    let after = canon::hash_of(&TenantSchema {
        version: SchemaVersion(0),
        ..schema(&world)
    });
    assert_eq!(before, after);
}

#[test]
fn removing_a_definition_that_does_not_exist_is_refused() {
    let world = notes_world();
    let error = refused(
        &world,
        "owner",
        vec![SchemaChange::RemoveTrigger(
            TriggerId::new("nothing").unwrap(),
        )],
    );
    assert_eq!(error.kind, ErrorKind::InvalidSchema);
    assert_eq!(error.message, "no trigger nothing");
    let error = refused(
        &world,
        "owner",
        vec![SchemaChange::RemoveOperation(
            TenantOperationId::new("nothing").unwrap(),
        )],
    );
    assert_eq!(error.message, "no operation nothing");
}

// ---------------------------------------------------------------------------------------------
// Triggers.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_trigger_that_names_no_automation_of_the_schema_is_refused() {
    let error = refused(
        &notes_world(),
        "owner",
        vec![SchemaChange::DefineTrigger(on_publish())],
    );
    assert_eq!(error.kind, ErrorKind::InvalidSchema);
    assert_eq!(
        error.message,
        "trigger on_publish: unknown automation award"
    );
}

// Verifies: INT-1522
#[test]
fn a_trigger_on_a_transition_no_workflow_declares_is_refused() {
    let mut trigger = on_publish();
    trigger.on = TriggerEvent::RecordUpdated {
        object: object("note"),
        transition: Some("archive".into()),
    };
    let error = refused(
        &notes_world(),
        "owner",
        vec![
            SchemaChange::DefineAutomation(award(&["writer"])),
            SchemaChange::DefineTrigger(trigger),
        ],
    );
    assert_eq!(
        error.message,
        "trigger on_publish: no workflow of note has a transition archive"
    );
}

#[test]
fn a_trigger_condition_reads_only_what_its_event_binds() {
    // A record event binds no ledger parameter.
    let mut trigger = on_publish();
    trigger.condition = Some(ge(param("balance"), int(500)));
    let error = refused(
        &notes_world(),
        "owner",
        vec![
            SchemaChange::DefineAutomation(award(&["writer"])),
            SchemaChange::DefineTrigger(trigger),
        ],
    );
    assert_eq!(
        error.message,
        "trigger on_publish: condition: undeclared parameter balance"
    );
    // A ledger event binds no record.
    let mut trigger = on_points("on_points");
    trigger.condition = Some(eq(var(Scope::New, "title"), lit("x")));
    let error = refused(
        &notes_world(),
        "owner",
        vec![
            SchemaChange::DefineAutomation(award(&["writer"])),
            SchemaChange::DefineTrigger(trigger),
        ],
    );
    assert_eq!(
        error.message,
        "trigger on_points: condition: unknown field title"
    );
}

#[test]
fn a_trigger_condition_is_a_truth_value() {
    let mut trigger = on_points("on_points");
    trigger.condition = Some(param("balance"));
    // A parameter has no known shape, so it passes; a literal string does not.
    must(
        &notes_world(),
        "owner",
        6_000,
        change(vec![
            SchemaChange::DefineAutomation(award(&["writer"])),
            SchemaChange::DefineTrigger(trigger.clone()),
        ]),
    );
    trigger.condition = Some(lit("yes"));
    let error = refused(
        &notes_world(),
        "owner",
        vec![
            SchemaChange::DefineAutomation(award(&["writer"])),
            SchemaChange::DefineTrigger(trigger),
        ],
    );
    assert_eq!(
        error.message,
        "trigger on_points: condition is Str, not a truth value"
    );
}

#[test]
fn a_tenant_holds_at_most_one_hundred_triggers() {
    let mut changes = vec![SchemaChange::DefineAutomation(award(&["writer"]))];
    for n in 0..100 {
        changes.push(SchemaChange::DefineTrigger(on_points(&format!("t{n}"))));
    }
    let world = must(&notes_world(), "owner", 6_000, change(changes));
    let error = refused(
        &world,
        "owner",
        vec![SchemaChange::DefineTrigger(on_points("one_more"))],
    );
    assert_eq!(
        error.message,
        "limit: 101 triggers is more than the maximum of 100"
    );
}

// ---------------------------------------------------------------------------------------------
// Automations.
// ---------------------------------------------------------------------------------------------

// Verifies: WFL-1528
#[test]
fn an_automation_whose_run_as_names_a_role_its_definer_may_not_confer_is_refused() {
    // A writer manages the schema and holds automations-define, but holds only some of what the
    // editor role grants (it updates and deletes only its own notes).
    let world = definers(true);
    let error = refused(
        &world,
        "writer1",
        vec![SchemaChange::DefineAutomation(award(&["editor"]))],
    );
    assert_eq!(error.kind, ErrorKind::Unauthorized);
    assert!(
        error
            .message
            .starts_with("may not confer role editor: lacks"),
        "{}",
        error.message
    );
    assert!(error
        .evidence
        .iter()
        .any(|check| check.rule == "permission:confer:editor" && !check.passed));
    // The editor may make it act as an editor, and as a writer.
    must(
        &world,
        "editor",
        6_000,
        change(vec![SchemaChange::DefineAutomation(award(&[
            "editor", "writer",
        ]))]),
    );
}

#[test]
fn only_an_owner_makes_an_automation_act_as_an_owner() {
    let error = refused(
        &definers(true),
        "editor",
        vec![SchemaChange::DefineAutomation(award(&["owner"]))],
    );
    assert_eq!(
        error.message,
        "may not confer role owner: only an owner confers the owner role"
    );
}

// Verifies: WFL-1520
#[test]
fn a_definer_without_automations_define_may_not_change_a_trigger_or_an_automation() {
    let world = definers(false);
    let error = refused(
        &world,
        "editor",
        vec![SchemaChange::DefineAutomation(award(&["writer"]))],
    );
    assert_eq!(error.kind, ErrorKind::Unauthorized);
    assert_eq!(
        error.message,
        "the change needs the capability automations-define"
    );
    let error = refused(
        &world,
        "editor",
        vec![SchemaChange::RemoveTrigger(
            TriggerId::new("on_publish").unwrap(),
        )],
    );
    assert_eq!(error.kind, ErrorKind::Unauthorized);
    // Other schema changes ask nothing more of the same principal.
    must(
        &world,
        "editor",
        6_000,
        change(vec![define_unique(title_rank())]),
    );
}

#[test]
fn an_automation_id_equal_to_a_field_workflow_id_is_refused() {
    let mut automation = award(&["writer"]);
    automation.id = WorkflowId::new("note_status").unwrap();
    let error = refused(
        &notes_world(),
        "owner",
        vec![SchemaChange::DefineAutomation(automation)],
    );
    assert_eq!(
        error.message,
        "automation note_status: the id is taken by a field workflow"
    );
}

#[test]
fn every_step_an_automation_names_exists_and_can_be_reached() {
    let mut automation = award(&["writer"]);
    automation
        .steps
        .insert(name("lost"), plain(AutomationStep::End, Some("gone")));
    let error = refused(
        &notes_world(),
        "owner",
        vec![SchemaChange::DefineAutomation(automation)],
    );
    let problems: Vec<&str> = error
        .evidence
        .iter()
        .filter(|c| c.rule == "schema:consistency")
        .map(|c| c.detail.as_str())
        .collect();
    assert_eq!(
        problems,
        [
            "automation award.lost: unknown step gone",
            "automation award.lost: cannot be reached from the start step"
        ]
    );
    let mut automation = award(&["writer"]);
    automation.start = name("begin");
    let error = refused(
        &notes_world(),
        "owner",
        vec![SchemaChange::DefineAutomation(automation)],
    );
    assert_eq!(error.message, "automation award: unknown start step begin");
}

#[test]
fn a_step_writes_only_fields_of_its_object_type_and_reads_only_declared_parameters() {
    let mut automation = award(&["writer"]);
    automation.steps.insert(
        name("make"),
        plain(
            AutomationStep::CreateRecord {
                object: object("note"),
                fields: BTreeMap::from([(field("colour"), param("hue"))]),
            },
            Some("check"),
        ),
    );
    let error = refused(
        &notes_world(),
        "owner",
        vec![SchemaChange::DefineAutomation(automation)],
    );
    let problems: Vec<&str> = error
        .evidence
        .iter()
        .filter(|c| c.rule == "schema:consistency")
        .map(|c| c.detail.as_str())
        .collect();
    assert_eq!(
        problems,
        [
            "automation award.make: unknown field colour",
            "automation award.make: colour: undeclared parameter hue"
        ]
    );
}

#[test]
fn the_bounds_of_a_step_are_enforced_when_it_is_defined() {
    let bad = |def: AutomationStepDef| {
        let mut automation = award(&["writer"]);
        automation.steps.insert(name("check"), def);
        let error = refused(
            &notes_world(),
            "owner",
            vec![SchemaChange::DefineAutomation(automation)],
        );
        error.message
    };
    let mut retried = plain(AutomationStep::End, None);
    retried.retry = Some(StepRetry {
        attempts: 11,
        first_delay: None,
        max_delay: None,
    });
    assert_eq!(
        bad(retried),
        "automation award.check: retry attempts outside 1 to 10"
    );
    let mut slow = plain(AutomationStep::End, None);
    slow.retry = Some(StepRetry {
        attempts: 3,
        first_delay: Some(strata_model_core::prelude::DurationMs(90_000_000)),
        max_delay: None,
    });
    assert_eq!(
        bad(slow),
        "automation award.check: a retry delay is longer than one day"
    );
    let mut waiting = plain(
        AutomationStep::WaitFor {
            duration: strata_model_core::prelude::DurationMs(1_000),
        },
        Some("done"),
    );
    waiting.timeout = Some(strata_model_core::prelude::DurationMs(1_000));
    assert_eq!(
        bad(waiting),
        "automation award.check: a wait or an approval has no attempt timeout"
    );
    // WFL-1537: a wait of more than 366 days.
    let long = plain(
        AutomationStep::WaitFor {
            duration: strata_model_core::prelude::DurationMs(367 * 86_400_000),
        },
        Some("done"),
    );
    assert_eq!(
        bad(long),
        "automation award.check: a wait is from 1 ms to 366 days"
    );
    let mut timed = plain(AutomationStep::End, None);
    timed.timeout = Some(strata_model_core::prelude::DurationMs(900_001));
    assert_eq!(
        bad(timed),
        "automation award.check: a timeout is from 1 ms to 15 minutes"
    );
    let segment: SegmentId =
        serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
    let fan = plain(
        AutomationStep::FanOut {
            segment,
            batch: 101,
            each: name("done"),
        },
        None,
    );
    assert_eq!(
        bad(fan),
        "automation award.check: a fan-out batch is from 1 to 100"
    );
}

#[test]
fn an_automation_has_at_most_fifty_steps() {
    let mut automation = award(&["writer"]);
    automation
        .steps
        .insert(name("done"), plain(AutomationStep::End, Some("s0")));
    for n in 0..48 {
        let next = if n == 47 {
            None
        } else {
            Some(format!("s{}", n + 1))
        };
        automation.steps.insert(
            name(&format!("s{n}")),
            plain(AutomationStep::End, next.as_deref()),
        );
    }
    let error = refused(
        &notes_world(),
        "owner",
        vec![SchemaChange::DefineAutomation(automation)],
    );
    assert_eq!(
        error.message,
        "automation award: 51 steps is more than the maximum of 50"
    );
}

// Verifies: D-6539
#[test]
fn an_automation_a_trigger_names_is_not_removed_until_the_trigger_is() {
    let world = must(
        &notes_world(),
        "owner",
        6_000,
        change(vec![
            SchemaChange::DefineAutomation(award(&["writer"])),
            SchemaChange::DefineTrigger(on_publish()),
        ]),
    );
    let error = refused(
        &world,
        "owner",
        vec![SchemaChange::RemoveAutomation(
            WorkflowId::new("award").unwrap(),
        )],
    );
    assert_eq!(error.kind, ErrorKind::InvalidSchema);
    assert_eq!(
        error.message,
        "automation award is named by trigger on_publish"
    );
    let world = must(
        &world,
        "owner",
        6_001,
        change(vec![
            SchemaChange::RemoveTrigger(TriggerId::new("on_publish").unwrap()),
            SchemaChange::RemoveAutomation(WorkflowId::new("award").unwrap()),
        ]),
    );
    assert!(schema(&world).automations.is_none());
    assert!(schema(&world).triggers.is_none());
}

#[test]
fn a_field_an_automation_writes_is_not_removed_while_the_automation_writes_it() {
    let mut automation = award(&["writer"]);
    if let AutomationStep::CreateRecord { fields, .. } =
        &mut automation.steps.get_mut(&name("make")).unwrap().action
    {
        fields.insert(field("body"), lit("Well done"));
    }
    let world = must(
        &notes_world(),
        "owner",
        6_000,
        change(vec![SchemaChange::DefineAutomation(automation)]),
    );
    let error = refused(
        &world,
        "owner",
        vec![SchemaChange::RemoveField {
            object: object("note"),
            field: field("body"),
        }],
    );
    assert_eq!(error.message, "automation award.make: unknown field body");
}

// ---------------------------------------------------------------------------------------------
// Tenant operations.
// ---------------------------------------------------------------------------------------------

// Verifies: OPR-1540
#[test]
fn an_owner_defines_a_tenant_operation() {
    let world = must(
        &notes_world(),
        "owner",
        6_000,
        change(vec![SchemaChange::DefineOperation(redeem())]),
    );
    assert_eq!(
        schema(&world).operations.as_ref().unwrap()[&TenantOperationId::new("redeem").unwrap()],
        redeem()
    );
}

// Verifies: OPR-1547
#[test]
fn a_tenant_operation_is_refused_when_its_definer_lacks_a_power_its_steps_use() {
    let world = definers(true);
    // The editor creates and updates any note: accepted.
    must(
        &world,
        "editor",
        6_000,
        change(vec![SchemaChange::DefineOperation(redeem())]),
    );
    // A writer updates only its own notes, under a condition: refused.
    let error = refused(
        &world,
        "writer1",
        vec![SchemaChange::DefineOperation(redeem())],
    );
    assert_eq!(error.kind, ErrorKind::Unauthorized);
    assert_eq!(
        error.message,
        "may not define operation redeem: lacks Update on Object(ObjectTypeId(\"note\"))"
    );
    // A posting needs ledger-post, which the editor does not hold.
    let mut op = redeem();
    op.steps.push(posting("count"));
    let error = refused(&world, "editor", vec![SchemaChange::DefineOperation(op)]);
    assert_eq!(
        error.message,
        "may not define operation redeem: lacks capability ledger-post"
    );
}

// Verifies: OPR-1549
#[test]
fn the_bounds_and_names_of_a_tenant_operation_are_checked_when_it_is_defined() {
    let problem = |op: TenantOperationDef| {
        refused(
            &notes_world(),
            "owner",
            vec![SchemaChange::DefineOperation(op)],
        )
        .message
    };
    let mut op = redeem();
    op.steps = (0..9).map(|n| posting(&format!("p{n}"))).collect();
    assert_eq!(
        problem(op),
        "operation redeem: 9 posting steps is more than the maximum of 8"
    );
    let mut op = redeem();
    op.steps = (0..17)
        .map(|n| TenantOperationStep::DeleteRecord {
            id: format!("d{n}"),
            object: object("note"),
            record: param("note"),
        })
        .collect();
    assert_eq!(
        problem(op),
        "operation redeem: 17 steps is more than the maximum of 16"
    );
    let mut op = redeem();
    op.steps.push(TenantOperationStep::DeleteRecord {
        id: "note".into(),
        object: object("note"),
        record: param("note"),
    });
    assert_eq!(
        problem(op),
        "operation redeem: step note has the name of an input"
    );
    let mut op = redeem();
    op.conditions[0].expr = lit("draft");
    assert_eq!(
        problem(op),
        "operation redeem: condition drafted is Str, not a truth value"
    );
    let mut op = redeem();
    op.subject = Some(TenantOperationSubject {
        object: object("tag"),
        input: "note".into(),
    });
    assert_eq!(
        problem(op),
        "operation redeem: unknown subject object type tag"
    );
    // A condition of more than two hundred nodes.
    let mut op = redeem();
    op.conditions[0].expr = Expr::And((0..100).map(|_| Expr::Not(Box::new(lit("x")))).collect());
    let error = refused(
        &notes_world(),
        "owner",
        vec![SchemaChange::DefineOperation(op)],
    );
    assert!(error.evidence.iter().any(|c| c.detail
        == "operation redeem: condition drafted has 201 nodes, more than the maximum of 200"));
}

#[test]
fn a_later_step_reads_the_record_an_earlier_create_step_made() {
    // `copy` is read by the step after it; read before it, it is not declared.
    let mut op = redeem();
    op.steps.reverse();
    let error = refused(
        &notes_world(),
        "owner",
        vec![SchemaChange::DefineOperation(op)],
    );
    assert_eq!(
        error.message,
        "operation redeem.touch: record: undeclared parameter copy"
    );
}

// Verifies: OPR-1551
#[test]
fn a_field_a_tenant_operation_names_is_not_removed_while_it_names_it() {
    let mut op = redeem();
    if let TenantOperationStep::UpdateRecord { fields, .. } = &mut op.steps[1] {
        fields.insert(field("body"), lit("redeemed"));
    }
    let world = must(
        &notes_world(),
        "owner",
        6_000,
        change(vec![SchemaChange::DefineOperation(op)]),
    );
    let error = refused(
        &world,
        "owner",
        vec![SchemaChange::RemoveField {
            object: object("note"),
            field: field("body"),
        }],
    );
    assert_eq!(error.message, "operation redeem.touch: unknown field body");
}

// ---------------------------------------------------------------------------------------------
// Composite unique constraints.
// ---------------------------------------------------------------------------------------------

fn uniquely_ranked() -> World {
    must(
        &notes_world(),
        "owner",
        6_000,
        change(vec![define_unique(title_rank())]),
    )
}

// Verifies: CMS-1554
#[test]
fn a_create_that_repeats_the_values_of_a_composite_unique_constraint_is_refused() {
    let world = must(
        &uniquely_ranked(),
        "writer1",
        6_100,
        note_with("A", "a", Some(1)),
    );
    // Another title, or another rank, is another tuple.
    let world = must(&world, "writer1", 6_101, note_with("B", "b", Some(1)));
    let world = must(&world, "writer1", 6_102, note_with("A", "c", Some(2)));
    let error = step(&world, "writer1", 6_103, note_with("A", "d", Some(1))).unwrap_err();
    assert_eq!(error.kind, ErrorKind::UniqueViolated);
    let failed = error.evidence.iter().find(|c| !c.passed).unwrap();
    assert_eq!(failed.rule, "unique:note.title_rank");
    assert_eq!(
        failed.detail,
        "One note per title and rank: record 1 already has these values"
    );
}

#[test]
fn an_update_into_the_values_of_another_record_is_refused() {
    let world = must(
        &uniquely_ranked(),
        "writer1",
        6_100,
        note_with("A", "a", Some(1)),
    );
    let world = must(&world, "writer1", 6_101, note_with("A", "b", Some(2)));
    let error = step(
        &world,
        "writer1",
        6_102,
        Intent::UpdateRecord {
            object: object("note"),
            id: RecordId(2),
            fields: BTreeMap::from([(field("rank"), Value::Int(1))]),
            expected_version: None,
        },
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::UniqueViolated);
    // Updating a record without changing its own tuple is not a clash with itself.
    must(
        &world,
        "writer1",
        6_103,
        Intent::UpdateRecord {
            object: object("note"),
            id: RecordId(1),
            fields: BTreeMap::from([(field("body"), text("again"))]),
            expected_version: None,
        },
    );
}

// Verifies: CMS-1555
#[test]
fn a_record_with_null_in_a_field_of_the_constraint_is_not_constrained() {
    let world = must(
        &uniquely_ranked(),
        "writer1",
        6_100,
        note_with("A", "a", None),
    );
    must(&world, "writer1", 6_101, note_with("A", "b", None));
}

// Verifies: CMS-1556
#[test]
fn a_composite_unique_constraint_over_existing_duplicates_is_refused_naming_a_pair() {
    let world = must(
        &notes_world(),
        "writer1",
        6_000,
        note_with("A", "a", Some(1)),
    );
    let world = must(&world, "writer1", 6_001, note_with("A", "b", Some(1)));
    let error = step(
        &world,
        "owner",
        6_002,
        Intent::ChangeSchema {
            changes: vec![define_unique(title_rank())],
            expected_version: None,
            existing: ExistingData::Grandfather,
        },
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::UniqueViolated);
    assert_eq!(
        error.message,
        "unique:note.title_rank: records 1 and 2 have the same values in title, rank"
    );
}

// Verifies: CMS-1556
#[test]
fn a_composite_unique_constraint_names_two_to_four_declared_fields_that_are_not_lists() {
    let problem = |fields: &[&str]| {
        let unique = UniqueDef {
            fields: fields.iter().map(|f| field(f)).collect(),
            ..title_rank()
        };
        refused(&notes_world(), "owner", vec![define_unique(unique)]).message
    };
    assert_eq!(
        problem(&["title"]),
        "note.title_rank: a unique constraint names 1 fields, not two to four"
    );
    assert_eq!(
        problem(&["title", "body", "rank", "status", "slug"]),
        "note.title_rank: a unique constraint names 5 fields, not two to four"
    );
    assert_eq!(
        problem(&["title", "colour"]),
        "note.title_rank: unknown field colour"
    );
    assert_eq!(
        problem(&["title", "title"]),
        "note.title_rank: names a field twice"
    );
    let world = must(
        &notes_world(),
        "owner",
        6_000,
        change(vec![SchemaChange::DefineField {
            object: object("note"),
            field: field_def("tags", FieldType::List(Box::new(FieldType::Str)), false),
        }]),
    );
    let unique = UniqueDef {
        fields: vec![field("title"), field("tags")],
        ..title_rank()
    };
    assert_eq!(
        refused(&world, "owner", vec![define_unique(unique)]).message,
        "note.title_rank: tags is a list field"
    );
}

// Verifies: CMS-1557
#[test]
fn the_check_plans_one_lookup_of_the_written_values_and_no_more() {
    let world = uniquely_ranked();
    let schema = schema(&world);
    let wanted = Need::ByFields {
        object: object("note"),
        fields: BTreeMap::from([(field("title"), text("A")), (field("rank"), Value::Int(1))]),
    };
    let planned = needs(Some(&schema), &note_with("A", "a", Some(1)));
    assert!(planned.contains(&wanted), "{planned:?}");
    assert!(!planned.iter().any(|n| matches!(n, Need::AllOf(_))));
    // Without a rank the constraint does not apply, so nothing is planned for it.
    let planned = needs(Some(&schema), &note_with("A", "a", None));
    assert!(!planned.iter().any(|n| matches!(n, Need::ByFields { .. })));
    // An update planned from the stored record and the change together.
    let world = must(&world, "writer1", 6_100, note_with("A", "a", None));
    let update = Intent::UpdateRecord {
        object: object("note"),
        id: RecordId(1),
        fields: BTreeMap::from([(field("rank"), Value::Int(1))]),
        expected_version: None,
    };
    let refined = refine_needs(&world.state(&tenant_id("alpha")), &update);
    assert!(refined.contains(&wanted), "{refined:?}");
}

#[test]
fn a_snapshot_without_the_lookup_is_asked_for_it() {
    let world = uniquely_ranked();
    let tenant = world.tenants[&tenant_id("alpha")].clone();
    let thin = State {
        tenant_id: tenant_id("alpha"),
        tenant: Some(tenant),
        coverage: Coverage::Partial(BTreeSet::from([Need::ByField {
            object: object("note"),
            field: field("slug"),
            value: text("a"),
        }])),
    };
    let error = transition(
        &thin,
        &note_with("A", "a", Some(1)),
        &ctx("alpha", user("alpha:writer1"), 6_100),
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::InsufficientState);
    assert_eq!(
        error.missing,
        Some(Need::ByFields {
            object: object("note"),
            fields: BTreeMap::from([(field("title"), text("A")), (field("rank"), Value::Int(1))]),
        })
    );
}
