//! Approving a waiting workflow step, activating an automation, and the step that invokes a
//! tenant operation, the kernel's half (`strata_kernel::approvals`, `strata_kernel::activation`,
//! `strata_kernel::definitions`): who may decide an approval, when the person who started the run
//! may not, when an automation starts runs, and what a definition that invokes an operation must
//! satisfy. One rule per test. The worker's half is `crates/backend/tests/approvals.rs`.

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::activation;
use strata_kernel::approvals::{decide, policy_roles, Refusal, WORKFLOW_APPROVE};
use strata_kernel::testkit::*;
use strata_kernel::triggers::{AutomationDecline, AutomationExceptionReason};
use strata_kernel::*;

// ---------------------------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------------------------

fn step_in(
    world: &World,
    who: &str,
    now: i64,
    intent: Intent,
) -> Result<(World, Transition), DomainError> {
    world.step(&intent, &ctx("alpha", user(&format!("alpha:{who}")), now))
}

fn must(world: &World, who: &str, now: i64, intent: Intent) -> (World, Transition) {
    step_in(world, who, now, intent).expect("accepted")
}

fn change(changes: Vec<SchemaChange>) -> Intent {
    Intent::ChangeSchema {
        changes,
        expected_version: None,
        existing: ExistingData::Reject,
    }
}

fn name(s: &str) -> StepName {
    StepName::new(s).expect("step name")
}

fn capability() -> Permission {
    Permission::Capability(WORKFLOW_APPROVE.to_string())
}

fn review_policy(id: &str, role_name: &str) -> SchemaChange {
    SchemaChange::DefinePolicy(PolicyDef {
        id: PolicyId::new(id).expect("policy id"),
        role: role(role_name),
        resource: Resource::Object(object("note")),
        actions: BTreeSet::from([Action::Read]),
        condition: None,
    })
}

/// The notes world with the approval model: editors and writers hold workflow-approve, and the
/// policy `review` names the editors; `member` holds nothing but is the role of the policy
/// `review_members`, held by `alpha:stranger`.
fn approvals_world() -> World {
    let (world, _) = must(
        &notes_world(),
        "owner",
        6_000,
        change(vec![
            SchemaChange::DefineRole(RoleDef {
                id: role("member"),
                label: "Member".into(),
            }),
            SchemaChange::DefineRolePermissions {
                role: role("editor"),
                permissions: vec![capability()],
            },
            SchemaChange::DefineRolePermissions {
                role: role("writer"),
                permissions: vec![capability()],
            },
            review_policy("review", "editor"),
            review_policy("review_members", "member"),
        ]),
    );
    let (world, _) = must(
        &world,
        "owner",
        6_001,
        Intent::AssignRole {
            principal: principal_id("alpha:stranger"),
            role: role("member"),
        },
    );
    world
}

fn editors() -> BTreeSet<RoleId> {
    BTreeSet::from([role("editor")])
}

/// Decide an approval of the policy `review` as `who`, started by `started_by`, due at 1000, at
/// time 500.
fn decided_by(who: &str, started_by: Option<&str>) -> Result<PrincipalId, Refusal> {
    let world = approvals_world();
    let started = started_by.map(principal_id);
    decide(
        &world.tenants[&tenant_id("alpha")],
        &user(&format!("alpha:{who}")),
        &editors(),
        started.as_ref(),
        Timestamp(1_000),
        Timestamp(500),
    )
}

// ---------------------------------------------------------------------------------------------
// Approving.
// ---------------------------------------------------------------------------------------------

// Verifies: WFL-2680
#[test]
fn a_holder_of_the_capability_and_a_role_the_policy_names_may_approve() {
    assert_eq!(
        decided_by("editor", Some("alpha:writer1")),
        Ok(principal_id("alpha:editor"))
    );
}

// Verifies: WFL-2680
#[test]
fn a_principal_without_the_role_the_policy_names_is_refused() {
    // writer1 holds workflow-approve through the writer role, but the policy names the editors.
    let refusal = decided_by("writer1", Some("alpha:writer2")).expect_err("refused");
    assert_eq!(refusal, Refusal::NotNamedByPolicy { roles: editors() });
    assert!(refusal.forbidden());
}

// Verifies: WFL-2680
#[test]
fn a_principal_without_the_capability_is_refused_though_it_holds_the_role() {
    let world = approvals_world();
    let members = BTreeSet::from([role("member")]);
    let refusal = decide(
        &world.tenants[&tenant_id("alpha")],
        &user("alpha:stranger"),
        &members,
        None,
        Timestamp(1_000),
        Timestamp(500),
    )
    .expect_err("refused");
    assert_eq!(refusal, Refusal::MissingCapability);
    assert!(refusal.forbidden());
}

// Verifies: WFL-2680
#[test]
fn the_owner_may_approve_whatever_role_the_policy_names() {
    assert_eq!(
        decided_by("owner", Some("alpha:writer1")),
        Ok(principal_id("alpha:owner"))
    );
}

// Verifies: WFL-2680
#[test]
fn only_a_signed_in_person_approves() {
    let world = approvals_world();
    let tenant = &world.tenants[&tenant_id("alpha")];
    for who in [
        Principal::Anonymous,
        Principal::System,
        user("automation:award"),
    ] {
        assert_eq!(
            decide(
                tenant,
                &who,
                &editors(),
                None,
                Timestamp(1_000),
                Timestamp(500)
            ),
            Err(Refusal::NotAPerson),
            "{who:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Separation.
// ---------------------------------------------------------------------------------------------

// Verifies: WFL-2681
#[test]
fn the_person_whose_action_started_the_run_may_not_approve_it() {
    let refusal = decided_by("editor", Some("alpha:editor")).expect_err("refused");
    assert_eq!(refusal, Refusal::SeparationOfDuties);
    assert!(refusal.forbidden());
    // The owner is no exception to separation.
    assert_eq!(
        decided_by("owner", Some("alpha:owner")),
        Err(Refusal::SeparationOfDuties)
    );
}

// Verifies: WFL-2681
#[test]
fn a_run_that_no_person_started_has_no_one_to_separate() {
    assert_eq!(decided_by("editor", None), Ok(principal_id("alpha:editor")));
}

// ---------------------------------------------------------------------------------------------
// The deadline.
// ---------------------------------------------------------------------------------------------

// Verifies: WFL-2682
#[test]
fn a_decision_after_the_deadline_is_refused_as_a_conflict_and_not_as_forbidden() {
    let world = approvals_world();
    let refusal = decide(
        &world.tenants[&tenant_id("alpha")],
        &user("alpha:editor"),
        &editors(),
        None,
        Timestamp(1_000),
        Timestamp(1_001),
    )
    .expect_err("refused");
    assert_eq!(refusal, Refusal::PastDeadline);
    assert!(!refusal.forbidden());
    // At the deadline itself the decision still stands.
    assert!(decide(
        &world.tenants[&tenant_id("alpha")],
        &user("alpha:editor"),
        &editors(),
        None,
        Timestamp(1_000),
        Timestamp(1_000),
    )
    .is_ok());
}

// ---------------------------------------------------------------------------------------------
// Activation.
// ---------------------------------------------------------------------------------------------

fn award_id() -> WorkflowId {
    WorkflowId::new("award").unwrap()
}

/// An automation of one step that ends, acting as an editor, in `state`.
fn flow(state: Option<AutomationState>) -> AutomationDef {
    let done = name("done");
    AutomationDef {
        id: award_id(),
        label: "Award".into(),
        run_as: RunAs {
            roles: BTreeSet::from([role("editor")]),
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
        state,
    }
}

fn on_note_created() -> TriggerDef {
    TriggerDef {
        id: TriggerId::new("on_note").unwrap(),
        on: TriggerEvent::RecordCreated {
            object: object("note"),
        },
        condition: None,
        automation: award_id(),
        paused: None,
        defined_by: None,
        defined_at: None,
    }
}

fn starts(outcome: &Transition) -> usize {
    outcome
        .effects
        .iter()
        .filter(|effect| matches!(effect, Effect::StartAutomation(_)))
        .count()
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

// Verifies: WFL-2684
#[test]
fn an_automation_without_a_state_is_active_a_draft_is_not_and_a_suspended_one_is_not() {
    assert!(activation::is_active(&flow(None)));
    assert!(activation::is_active(&flow(Some(AutomationState::Active))));
    assert!(!activation::is_active(&flow(Some(AutomationState::Draft))));
    let mut suspended = flow(Some(AutomationState::Active));
    suspended.suspended_for = Some(role("editor"));
    assert_eq!(activation::state_of(&suspended), AutomationState::Suspended);
    assert!(!activation::is_active(&suspended));
    assert_eq!(
        activation::activated(&flow(Some(AutomationState::Draft))).state,
        Some(AutomationState::Active)
    );
}

// Verifies: WFL-1521, WFL-2684
#[test]
fn a_trigger_on_a_draft_declines_and_starts_no_run_until_the_automation_is_activated() {
    let (world, _) = must(
        &notes_world(),
        "owner",
        6_000,
        change(vec![
            SchemaChange::DefineAutomation(flow(Some(AutomationState::Draft))),
            SchemaChange::DefineTrigger(on_note_created()),
        ]),
    );
    let (world, outcome) = must(&world, "writer1", 7_000, new_note("Draft", "draft-1"));
    assert_eq!(starts(&outcome), 0, "a draft starts no run");
    let declined = declines(&outcome);
    assert_eq!(declined.len(), 1);
    assert_eq!(
        declined[0].reason,
        AutomationExceptionReason::AutomationInactive
    );
    assert_eq!(declined[0].automation, award_id());
    assert_eq!(declined[0].detail, "automation award is draft, not active");
    // Activating is the definition made again with the state active.
    let (world, _) = must(
        &world,
        "owner",
        7_001,
        change(vec![SchemaChange::DefineAutomation(activation::activated(
            &flow(Some(AutomationState::Draft)),
        ))]),
    );
    let (_, outcome) = must(&world, "writer1", 7_002, new_note("Live", "live-1"));
    assert_eq!(starts(&outcome), 1, "an active automation starts its run");
    assert!(declines(&outcome).is_empty());
}

// ---------------------------------------------------------------------------------------------
// The step that invokes a tenant operation.
// ---------------------------------------------------------------------------------------------

fn stamp() -> TenantOperationDef {
    TenantOperationDef {
        id: TenantOperationId::new("stamp").unwrap(),
        label: "Stamp".into(),
        inputs: vec![TenantOperationInput {
            name: "slug".into(),
            ty: FieldType::Str,
            required: true,
            label: None,
        }],
        invokers: TenantOperationInvokers {
            signed_in: false,
            roles: BTreeSet::from([role("editor")]),
        },
        subject: None,
        conditions: vec![],
        steps: vec![TenantOperationStep::CreateRecord {
            id: "made".into(),
            object: object("note"),
            fields: BTreeMap::from([
                (field("title"), lit("Stamped")),
                (field("slug"), Expr::Param("slug".into())),
            ]),
        }],
    }
}

/// An automation that invokes `operation` with `inputs` as one of `run_as`, and ends.
fn invoking(operation: &str, inputs: &[(&str, Expr)], run_as: &[&str]) -> AutomationDef {
    let mut def = flow(None);
    def.run_as = RunAs {
        roles: run_as.iter().map(|r| role(r)).collect(),
    };
    def.start = name("call");
    def.steps.insert(
        name("call"),
        AutomationStepDef {
            action: AutomationStep::InvokeOperation {
                operation: TenantOperationId::new(operation).unwrap(),
                inputs: inputs
                    .iter()
                    .map(|(input, expr)| (input.to_string(), expr.clone()))
                    .collect(),
            },
            next: Some(name("done")),
            retry: None,
            timeout: None,
            compensate: None,
        },
    );
    def
}

fn operations_world() -> World {
    must(
        &notes_world(),
        "owner",
        6_000,
        change(vec![SchemaChange::DefineOperation(stamp())]),
    )
    .0
}

fn problems(world: &World, def: AutomationDef) -> Vec<String> {
    let error = step_in(
        world,
        "owner",
        6_100,
        change(vec![SchemaChange::DefineAutomation(def)]),
    )
    .expect_err("refused");
    error
        .evidence
        .iter()
        .filter(|c| c.rule == "schema:consistency")
        .map(|c| c.detail.clone())
        .collect()
}

// Verifies: WFL-2685
#[test]
fn an_automation_that_invokes_a_declared_operation_with_its_inputs_is_accepted() {
    let world = operations_world();
    let def = invoking("stamp", &[("slug", lit("a-slug"))], &["editor"]);
    let (world, _) = must(
        &world,
        "owner",
        6_100,
        change(vec![SchemaChange::DefineAutomation(def)]),
    );
    let held = &world.tenants[&tenant_id("alpha")].schema.automations;
    assert!(held.as_ref().unwrap().contains_key(&award_id()));
}

// Verifies: WFL-2685
#[test]
fn a_step_that_names_an_operation_the_tenant_does_not_have_is_refused() {
    let found = problems(
        &operations_world(),
        invoking("nope", &[("slug", lit("a"))], &["editor"]),
    );
    assert_eq!(found, ["automation award.call: unknown operation nope"]);
}

// Verifies: WFL-2685
#[test]
fn a_step_whose_run_as_roles_may_not_invoke_the_operation_is_refused() {
    // The operation names the editors; the automation acts as a writer.
    let found = problems(
        &operations_world(),
        invoking("stamp", &[("slug", lit("a"))], &["writer"]),
    );
    assert_eq!(
        found,
        ["automation award.call: no run-as role of automation award may invoke operation stamp"]
    );
}

// Verifies: WFL-2685
#[test]
fn a_step_may_give_only_the_inputs_the_operation_declares_and_all_it_requires() {
    let world = operations_world();
    assert_eq!(
        problems(
            &world,
            invoking(
                "stamp",
                &[("slug", lit("a")), ("colour", lit("red"))],
                &["editor"]
            )
        ),
        ["automation award.call: operation stamp has no input colour"]
    );
    assert_eq!(
        problems(&world, invoking("stamp", &[], &["editor"])),
        ["automation award.call: operation stamp needs the input slug"]
    );
}

// Verifies: WFL-2685
#[test]
fn an_input_of_another_shape_than_its_type_or_naming_an_undeclared_parameter_is_refused() {
    let world = operations_world();
    assert_eq!(
        problems(
            &world,
            invoking("stamp", &[("slug", Expr::Lit(Value::Int(3)))], &["editor"])
        ),
        ["automation award.call: input slug is Int, but operation stamp expects string"]
    );
    assert_eq!(
        problems(
            &world,
            invoking(
                "stamp",
                &[("slug", Expr::Param("nope".into()))],
                &["editor"]
            )
        ),
        ["automation award.call: input slug: undeclared parameter nope"]
    );
}

// Verifies: WFL-2685
#[test]
fn an_operation_that_a_step_names_is_not_removed_while_it_does() {
    let world = operations_world();
    let def = invoking("stamp", &[("slug", lit("a"))], &["editor"]);
    let (world, _) = must(
        &world,
        "owner",
        6_100,
        change(vec![SchemaChange::DefineAutomation(def)]),
    );
    let error = step_in(
        &world,
        "owner",
        6_200,
        change(vec![SchemaChange::RemoveOperation(
            TenantOperationId::new("stamp").unwrap(),
        )]),
    )
    .expect_err("refused");
    assert_eq!(
        error.message,
        "automation award.call: unknown operation stamp"
    );
}
