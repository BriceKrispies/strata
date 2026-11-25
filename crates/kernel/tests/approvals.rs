//! Approving a waiting workflow step, activating an automation, and the step that invokes a
//! tenant operation, the kernel's half (`strata_kernel::approvals`, `strata_kernel::activation`,
//! `strata_kernel::definitions`): who may decide an approval, when the person who started the run
//! may not, when an automation starts runs, and what a definition that invokes an operation must
//! satisfy. One rule per test. The worker's half is `crates/backend/tests/approvals.rs`.

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::approvals::{decide, Refusal, WORKFLOW_APPROVE};
use strata_kernel::testkit::*;
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

// ---------------------------------------------------------------------------------------------
// Separation.
// ---------------------------------------------------------------------------------------------

// Verifies: WFL-2681
#[test]
fn a_run_that_no_person_started_has_no_one_to_separate() {
    assert_eq!(decided_by("editor", None), Ok(principal_id("alpha:editor")));
}

// ---------------------------------------------------------------------------------------------
// The deadline.
// ---------------------------------------------------------------------------------------------

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
        agent_policy: None,
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
