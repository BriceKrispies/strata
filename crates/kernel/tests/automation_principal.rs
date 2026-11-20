//! The automation principal (D-6525, WFL-1527, WG-5098): a step is decided as
//! `automation:<workflow-id>`, which holds exactly the run-as roles of its automation; the prefix
//! is reserved for it. One rule per test.

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::automation::{automation_principal, AUTOMATION_PREFIX};
use strata_kernel::rules::roles_of;
use strata_kernel::testkit::*;
use strata_kernel::*;

fn award() -> WorkflowId {
    WorkflowId::new("award").unwrap()
}

/// The notes world with the automation `award`, run as `writer`, defined by alpha's owner.
fn world_with_award() -> World {
    let def = AutomationDef {
        id: award(),
        label: "Award".into(),
        run_as: RunAs {
            roles: BTreeSet::from([role("writer")]),
        },
        run_key: None,
        start: StepName::new("done").unwrap(),
        steps: BTreeMap::from([(
            StepName::new("done").unwrap(),
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
    };
    notes_world()
        .step(
            &Intent::ChangeSchema {
                changes: vec![SchemaChange::DefineAutomation(def)],
                expected_version: None,
                existing: ExistingData::Reject,
            },
            &ctx("alpha", user("alpha:owner"), 5_000),
        )
        .expect("the owner defines the automation")
        .0
}

fn as_award(now: i64) -> Context {
    ctx(
        "alpha",
        Principal::User(automation_principal(&award())),
        now,
    )
}

// Verifies: WFL-1527
#[test]
fn a_step_is_authorized_exactly_as_its_run_as_role() {
    let world = world_with_award();
    // A writer may create a note, and edit the notes it created.
    let (world, created) = world
        .step(&new_note("Silver", "silver"), &as_award(5_001))
        .expect("a writer creates notes");
    let Output::Record { id, .. } = created.output else {
        panic!("a record output");
    };
    let rename = Intent::UpdateRecord {
        object: object("note"),
        id,
        fields: BTreeMap::from([(field("title"), text("Gold"))]),
        expected_version: None,
    };
    let (world, _) = world
        .step(&rename, &as_award(5_002))
        .expect("a writer edits its own note");
    // Publishing needs the editor role, which the run-as does not name.
    let publish = Intent::UpdateRecord {
        object: object("note"),
        id,
        fields: BTreeMap::from([(field("status"), text("published"))]),
        expected_version: None,
    };
    let refused = world
        .step(&publish, &as_award(5_003))
        .expect_err("only an editor publishes");
    assert_eq!(refused.kind, ErrorKind::WorkflowViolation);
    // Deleting a note someone else wrote is the editor's, not the writer's.
    let (world, theirs) = world
        .step(
            &new_note("Theirs", "theirs"),
            &ctx("alpha", user("alpha:writer1"), 5_004),
        )
        .expect("writer1 writes a note");
    let Output::Record { id: theirs, .. } = theirs.output else {
        panic!("a record output");
    };
    let refused = world
        .step(
            &Intent::DeleteRecord {
                object: object("note"),
                id: theirs,
                expected_version: None,
            },
            &as_award(5_005),
        )
        .expect_err("a writer deletes only its own notes");
    assert_eq!(refused.kind, ErrorKind::Unauthorized);
}

// Verifies: WFL-1527
#[test]
fn an_automation_principal_holds_its_run_as_roles_and_no_other() {
    let world = world_with_award();
    let tenant = &world.tenants[&tenant_id("alpha")];
    let principal = Principal::User(automation_principal(&award()));
    assert_eq!(
        roles_of(tenant, &principal),
        BTreeSet::from([role("writer")]),
        "neither public nor any role of the table of members"
    );
}

#[test]
fn the_principal_of_a_removed_automation_holds_no_role() {
    let world = world_with_award()
        .step(
            &Intent::ChangeSchema {
                changes: vec![SchemaChange::RemoveAutomation(award())],
                expected_version: None,
                existing: ExistingData::Reject,
            },
            &ctx("alpha", user("alpha:owner"), 5_100),
        )
        .expect("the owner removes the automation")
        .0;
    let tenant = &world.tenants[&tenant_id("alpha")];
    let principal = Principal::User(automation_principal(&award()));
    assert!(roles_of(tenant, &principal).is_empty());
    assert!(world
        .step(&new_note("Late", "late"), &as_award(5_101))
        .is_err());
}

#[test]
fn a_person_may_not_be_given_roles_under_the_automation_prefix() {
    let world = world_with_award();
    let impostor = PrincipalId::new(format!("{AUTOMATION_PREFIX}award")).unwrap();
    let refused = world
        .step(
            &Intent::AssignRole {
                principal: impostor,
                role: role("editor"),
            },
            &ctx("alpha", user("alpha:owner"), 5_200),
        )
        .expect_err("the prefix is reserved");
    assert_eq!(refused.kind, ErrorKind::InvalidIntent);
}

#[test]
fn a_signed_in_caller_may_not_act_under_the_automation_prefix() {
    let world = world_with_award();
    let mut context = as_award(5_300);
    context.session = Some(SessionId::new("s-1").unwrap());
    let refused = world
        .step(&new_note("Mine", "mine"), &context)
        .expect_err("an automation acts without a session");
    assert_eq!(refused.kind, ErrorKind::InvalidIntent);
}

/// The one rule for reserved principal ids that the backend's entry points (sessions, SCIM users,
/// identities from a provider, invitations) apply, beside the kernel's own refusals.
#[test]
fn every_reserved_prefix_is_named_and_a_person_id_is_not() {
    use strata_kernel::automation::reserved_prefix;
    let id = |text: &str| PrincipalId::new(text).unwrap();
    assert_eq!(
        reserved_prefix(&id("automation:award")),
        Some("automation:")
    );
    assert_eq!(reserved_prefix(&id("team:editors")), Some("team:"));
    assert_eq!(reserved_prefix(&id("alpha:ada")), None);
}
