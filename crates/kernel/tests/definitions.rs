//! Triggers, automations, tenant operations and composite unique constraints as tenant metadata:
//! what the kernel holds, what it refuses when they are defined, and who may define them.
//! One rule per test.

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_model_core::types::LedgerUnit;

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
        defined_by: None,
        defined_at: None,
        suspended_for: None,
        state: None,
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
        defined_by: None,
        defined_at: None,
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
            self_action: None,
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
        agent_policy: None,
    }
}

/// The declaration of the ledger unit `points` that `posting` names.
fn points() -> SchemaChange {
    SchemaChange::DeclareLedgerUnit(LedgerUnitDecl {
        unit: LedgerUnit::new("points").unwrap(),
        description: "Points".into(),
        expiry: None,
        transfer: None,
    })
}

fn posting(id: &str) -> TenantOperationStep {
    TenantOperationStep::PostLedger {
        id: id.into(),
        owner: TenantOperationPostingOwner::Invoker,
        unit: LedgerUnit::new("points").unwrap(),
        scope: None,
        amount: int(1),
        reason: "redemption".into(),
        reference: None,
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
        points(),
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
    // The kernel writes who defined each and when (D-7204).
    assert_eq!(
        after.automations.as_ref().unwrap()[&WorkflowId::new("award").unwrap()],
        AutomationDef {
            defined_by: Some(principal_id("alpha:owner")),
            defined_at: Some(Timestamp(6_000)),
            ..award(&["writer"])
        }
    );
    assert_eq!(
        after.triggers.as_ref().unwrap()[&TriggerId::new("on_publish").unwrap()],
        TriggerDef {
            defined_by: Some(principal_id("alpha:owner")),
            defined_at: Some(Timestamp(6_000)),
            ..on_publish()
        }
    );
}

// ---------------------------------------------------------------------------------------------
// Triggers.
// ---------------------------------------------------------------------------------------------

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
    // Only an owner confers the owner role.
    let error = refused(
        &world,
        "editor",
        vec![SchemaChange::DefineAutomation(award(&["owner"]))],
    );
    assert_eq!(
        error.message,
        "may not confer role owner: only an owner confers the owner role"
    );
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

// ---------------------------------------------------------------------------------------------
// Tenant operations.
// ---------------------------------------------------------------------------------------------

// Verifies: OPR-1540
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
            vec![points(), SchemaChange::DefineOperation(op)],
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
        self_action: None,
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

// Verifies: SCH-3202
#[test]
fn a_trigger_selects_a_declared_event_kind_and_binds_its_name() {
    let on = TriggerEvent::DeclaredEvent {
        kind: strata_model_core::types::EventKind("app.order-placed".into()),
    };
    assert!(definitions::event_params(&on).contains("kind"));
    // A trigger on a declared kind is held with the automation it starts.
    let trigger = TriggerDef {
        id: TriggerId::new("on_order").unwrap(),
        on,
        condition: None,
        automation: WorkflowId::new("award").unwrap(),
        paused: None,
        defined_by: None,
        defined_at: None,
    };
    let world = must(
        &notes_world(),
        "owner",
        6_000,
        change(vec![
            SchemaChange::DefineAutomation(award(&["writer"])),
            SchemaChange::DefineTrigger(trigger.clone()),
        ]),
    );
    assert_eq!(
        schema(&world).triggers.as_ref().unwrap()[&trigger.id].on,
        trigger.on
    );
}

// ---------------------------------------------------------------------------------------------
// Workflow transitions may not assign a workflow field (the WfDefined rule of the model).
// ---------------------------------------------------------------------------------------------

/// A workflow over `body` of the notes world, whose one transition assigns `set`.
fn body_flow(set: Vec<(FieldId, Expr)>) -> WorkflowDef {
    WorkflowDef {
        id: WorkflowId::new("body_flow").expect("id"),
        object: object("note"),
        field: field("body"),
        initial: BTreeSet::from(["draft".to_string()]),
        transitions: vec![WorkflowTransition {
            name: "tidy".into(),
            from: "draft".into(),
            to: "tidied".into(),
            guard: None,
            set: set.into_iter().collect(),
        }],
    }
}

#[test]
fn a_transition_that_assigns_another_workflows_field_is_refused() {
    // The notes world's status workflow is over `status`; this transition assigns it.
    let set = vec![(field("status"), Expr::Lit(Value::Str("draft".into())))];
    let error = refused(
        &notes_world(),
        "owner",
        vec![SchemaChange::DefineWorkflow(body_flow(set))],
    );
    assert_eq!(error.kind, ErrorKind::InvalidSchema);
    assert!(
        error
            .message
            .contains("sets status, a workflow field of note"),
        "{}",
        error.message
    );
}

#[test]
fn a_transition_that_assigns_null_to_a_workflow_field_is_refused() {
    // `body` is the field of this workflow; a null assignment to it is refused as well.
    let set = vec![(field("body"), Expr::Lit(Value::Null))];
    let error = refused(
        &notes_world(),
        "owner",
        vec![SchemaChange::DefineWorkflow(body_flow(set))],
    );
    assert_eq!(error.kind, ErrorKind::InvalidSchema);
    assert!(
        error
            .message
            .contains("sets body, a workflow field of note"),
        "{}",
        error.message
    );
}

#[test]
fn a_transition_that_assigns_a_plain_field_is_defined() {
    let set = vec![(field("title"), Expr::Lit(Value::Str("tidy".into())))];
    must(
        &notes_world(),
        "owner",
        9_000,
        change(vec![SchemaChange::DefineWorkflow(body_flow(set))]),
    );
}
