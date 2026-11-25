//! A schema change that widens a role is refused unless the changer holds each power it adds
//! (ADM-1088): by the role's permissions, by a policy naming the role, by an operation's invoker
//! list, through a team that holds the role, and by a product's install or upgrade.

use std::collections::BTreeMap;
use strata_kernel::testkit::*;
use strata_kernel::*;

const NOW: i64 = 9_000;

fn object_permission(name: &str, verbs: &[ObjectAction]) -> Permission {
    Permission::Object {
        object: object(name),
        actions: verbs.iter().copied().collect(),
    }
}

fn capability(name: &str) -> Permission {
    Permission::Capability(name.to_string())
}

fn define_role(id: &str) -> SchemaChange {
    SchemaChange::DefineRole(RoleDef {
        id: role(id),
        label: id.into(),
    })
}

fn permissions(id: &str, permissions: Vec<Permission>) -> SchemaChange {
    SchemaChange::DefineRolePermissions {
        role: role(id),
        permissions,
    }
}

fn policy(id: &str, of: &str, resource: Resource, actions: &[Action]) -> PolicyDef {
    PolicyDef {
        id: PolicyId::new(id).unwrap(),
        role: role(of),
        resource,
        actions: actions.iter().copied().collect(),
        condition: None,
    }
}

/// The notes tenant with the roles these tests widen. `stranger` changes the schema when it holds
/// `schema_admin`, which carries the policy that manages the schema; the owner holds everything.
///
/// - `reviewer` reads notes and nothing else.
/// - `publisher` holds the capability `content.publish`.
/// - `installer` holds the capability `bundle-install`, which a product install needs.
/// - `creator` creates notes.
/// - `updater` updates any object through a policy.
/// - `publishers_read_all` is a policy of `publisher` that reads any object.
fn base() -> World {
    let changes = vec![
        define_role("reviewer"),
        define_role("publisher"),
        define_role("installer"),
        define_role("creator"),
        define_role("updater"),
        define_role("schema_admin"),
        permissions(
            "reviewer",
            vec![object_permission("note", &[ObjectAction::Read])],
        ),
        permissions("publisher", vec![capability("content.publish")]),
        permissions("installer", vec![capability("bundle-install")]),
        permissions(
            "creator",
            vec![object_permission("note", &[ObjectAction::Create])],
        ),
        SchemaChange::DefinePolicy(policy(
            "updaters_update_any",
            "updater",
            Resource::AnyObject,
            &[Action::Update],
        )),
        SchemaChange::DefinePolicy(policy(
            "publishers_read_all",
            "publisher",
            Resource::AnyObject,
            &[Action::Read],
        )),
        SchemaChange::DefinePolicy(policy(
            "schema_admins_manage_schema",
            "schema_admin",
            Resource::Schema,
            &[Action::Manage],
        )),
    ];
    change(&notes_world(), "owner", NOW, changes).expect("the base change is accepted")
}

fn change(
    world: &World,
    who: &str,
    now: i64,
    changes: Vec<SchemaChange>,
) -> Result<World, DomainError> {
    world
        .step(
            &Intent::ChangeSchema {
                changes,
                expected_version: None,
                existing: ExistingData::Reject,
            },
            &ctx("alpha", user(&format!("alpha:{who}")), now),
        )
        .map(|(world, _)| world)
}

fn refused(result: Result<World, DomainError>) -> DomainError {
    match result {
        Ok(_) => panic!("the change was accepted"),
        Err(error) => error,
    }
}

/// `who` is given each of `roles`, by the owner.
fn with_roles(world: &World, who: &str, roles: &[&str]) -> World {
    roles.iter().fold(world.clone(), |world, name| {
        world
            .step(
                &Intent::AssignRole {
                    principal: principal_id(&format!("alpha:{who}")),
                    role: role(name),
                },
                &ctx("alpha", user("alpha:owner"), NOW + 1),
            )
            .expect("the owner assigns the role")
            .0
    })
}

/// The change is refused as an unauthorised widening of `role`, its evidence fails the rule
/// `permission:widen:<role>`, and the refusal names each of `lacking`.
fn widening_refused(error: &DomainError, role_name: &str, lacking: &[&str]) {
    assert_eq!(error.kind, ErrorKind::Unauthorized, "{}", error.message);
    let rule = format!("permission:widen:{role_name}");
    let check = error
        .evidence
        .iter()
        .find(|c| c.rule == rule && !c.passed)
        .unwrap_or_else(|| panic!("no failed {rule} in {:?}", error.evidence));
    for power in lacking {
        assert!(
            check.detail.contains(*power),
            "{} does not name {power}",
            check.detail
        );
    }
    assert!(
        error
            .message
            .contains(&format!("may not widen role {role_name}")),
        "{}",
        error.message
    );
}

/// Verifies: ADM-1088
///
/// A change of a role's permissions that adds a permission or capability the changer does not
/// hold is refused and names it. A change that adds only what the changer holds is accepted, and
/// so is one that only takes permissions away. The owner holds everything.
#[test]
fn a_role_definition_that_widens_a_role_is_refused_unless_the_changer_holds_each_power() {
    let world = with_roles(&base(), "stranger", &["schema_admin", "reviewer"]);
    let widen = |permissions_of: Vec<Permission>| permissions("reviewer", permissions_of);

    let error = refused(change(
        &world,
        "stranger",
        NOW + 10,
        vec![widen(vec![
            object_permission("note", &[ObjectAction::Read, ObjectAction::Update]),
            capability("people.manage"),
        ])],
    ));
    widening_refused(
        &error,
        "reviewer",
        &["note:update", "capability people.manage"],
    );

    // The owner keeps full authority over the role.
    change(
        &world,
        "owner",
        NOW + 11,
        vec![widen(vec![
            object_permission("note", &[ObjectAction::Read]),
            capability("people.manage"),
        ])],
    )
    .expect("the owner widens the role");

    // A power the changer holds through its own role is added.
    let holder = with_roles(
        &base(),
        "stranger",
        &["schema_admin", "reviewer", "publisher"],
    );
    change(
        &holder,
        "stranger",
        NOW + 12,
        vec![widen(vec![
            object_permission("note", &[ObjectAction::Read]),
            capability("content.publish"),
        ])],
    )
    .expect("the changer holds what it adds");

    // Taking a permission away adds nothing, so it is accepted.
    change(&world, "stranger", NOW + 13, vec![widen(vec![])])
        .expect("a narrowing change is accepted");
}

/// Verifies: ADM-1088
///
/// A policy that names a role gives the role a power, so a changer may add one only when it holds
/// that power. A policy the changer holds through its own policy is accepted. Defining the same
/// policy again adds nothing and is accepted, and a change that adds one action of a policy is
/// refused for that action alone.
#[test]
fn a_policy_naming_a_role_is_refused_unless_the_changer_holds_its_power() {
    let world = with_roles(&base(), "stranger", &["schema_admin", "reviewer"]);
    let naming_reviewer = |actions: &[Action]| {
        SchemaChange::DefinePolicy(policy(
            "reviewers_update_notes",
            "reviewer",
            Resource::Object(object("note")),
            actions,
        ))
    };
    let error = refused(change(
        &world,
        "stranger",
        NOW + 20,
        vec![naming_reviewer(&[Action::Update])],
    ));
    widening_refused(&error, "reviewer", &["reviewers_update_notes"]);

    // Holding `updater`, the changer holds Update on every object through a policy of its own.
    let holder = with_roles(
        &base(),
        "stranger",
        &["schema_admin", "reviewer", "updater"],
    );
    change(
        &holder,
        "stranger",
        NOW + 21,
        vec![naming_reviewer(&[Action::Update])],
    )
    .expect("the changer holds the power the policy gives");

    // Re-defining an existing policy as it is adds nothing. A publisher's policy names Read and
    // Update: adding Update alone is refused, and Read, which the changer lacks, is not named.
    let publisher_policy = |actions: &[Action]| {
        SchemaChange::DefinePolicy(policy(
            "publishers_read_all",
            "publisher",
            Resource::AnyObject,
            actions,
        ))
    };
    let stranger = with_roles(&base(), "stranger", &["schema_admin"]);
    change(
        &stranger,
        "stranger",
        NOW + 22,
        vec![publisher_policy(&[Action::Read])],
    )
    .expect("the same policy again adds nothing");
    let error = refused(change(
        &stranger,
        "stranger",
        NOW + 23,
        vec![publisher_policy(&[Action::Read, Action::Update])],
    ));
    widening_refused(&error, "publisher", &["AnyObject"]);
    assert!(
        !error.evidence.iter().any(|c| c.detail.contains("Read on")),
        "{:?}",
        error.evidence
    );
}

/// Verifies: ADM-1088
///
/// An operation's invoker list gives the invoker roles the operation's powers. A definition is
/// refused while the definer lacks one of them, and the definer that holds them all may name the
/// role as an invoker. The owner holds every power and may name any role.
#[test]
fn an_operation_that_a_widening_role_invokes_is_defined_only_with_the_definers_powers() {
    let log_note = |invokers: &[&str]| TenantOperationDef {
        id: TenantOperationId::new("log_note").unwrap(),
        label: "Log a note".into(),
        inputs: vec![TenantOperationInput {
            name: "slug".into(),
            ty: FieldType::Str,
            required: true,
            label: None,
        }],
        invokers: TenantOperationInvokers {
            signed_in: false,
            roles: invokers.iter().map(|name| role(name)).collect(),
        },
        subject: None,
        conditions: vec![],
        steps: vec![TenantOperationStep::CreateRecord {
            id: "made".into(),
            object: object("note"),
            fields: BTreeMap::from([
                (field("title"), lit("Logged")),
                (field("slug"), Expr::Param("slug".into())),
            ]),
        }],
        agent_policy: None,
    };
    let definer = with_roles(&base(), "stranger", &["schema_admin", "reviewer"]);
    let error = refused(change(
        &definer,
        "stranger",
        NOW + 30,
        vec![SchemaChange::DefineOperation(log_note(&[
            "reviewer", "updater",
        ]))],
    ));
    assert_eq!(error.kind, ErrorKind::Unauthorized, "{}", error.message);
    assert!(
        error
            .evidence
            .iter()
            .any(|c| c.rule == "operation:log_note:definer_powers" && !c.passed),
        "{:?}",
        error.evidence
    );

    let creator = with_roles(&base(), "stranger", &["schema_admin", "creator"]);
    change(
        &creator,
        "stranger",
        NOW + 31,
        vec![SchemaChange::DefineOperation(log_note(&["reviewer"]))],
    )
    .expect("the definer holds the power the operation uses");
    change(
        &base(),
        "owner",
        NOW + 32,
        vec![SchemaChange::DefineOperation(log_note(&[
            "reviewer", "updater",
        ]))],
    )
    .expect("the owner defines it for any invoker");
}

/// Verifies: ADM-1088
///
/// A team's roles count for each member. A member who holds a role only through its team may not
/// widen it by a schema change, since the team's role is the role's permissions.
#[test]
fn a_role_a_team_holds_is_not_widened_by_a_member_who_lacks_the_power() {
    let team = TeamId::new("desk").unwrap();
    let world = base();
    let world = world
        .step(
            &Intent::DefineTeam {
                team: team.clone(),
                label: "Desk".into(),
            },
            &ctx("alpha", user("alpha:owner"), NOW + 41),
        )
        .expect("the owner defines the team")
        .0;
    let world = world
        .step(
            &Intent::AssignTeamRole {
                team: team.clone(),
                role: role("reviewer"),
            },
            &ctx("alpha", user("alpha:owner"), NOW + 42),
        )
        .expect("the owner gives the team the role")
        .0;
    let world = world
        .step(
            &Intent::AddTeamMember {
                team,
                principal: principal_id("alpha:stranger"),
            },
            &ctx("alpha", user("alpha:owner"), NOW + 43),
        )
        .expect("the owner adds the member")
        .0;
    let world = with_roles(&world, "stranger", &["schema_admin"]);

    let error = refused(change(
        &world,
        "stranger",
        NOW + 44,
        vec![permissions(
            "reviewer",
            vec![
                object_permission("note", &[ObjectAction::Read]),
                capability("people.manage"),
            ],
        )],
    ));
    widening_refused(&error, "reviewer", &["capability people.manage"]);
}

/// Verifies: ADM-1088
///
/// A product's install is a `ChangeSchema` with its record. The installer needs the bundle-install
/// capability as before, and in addition holds every power the bundle adds to a role; an install
/// that adds none is accepted. The owner installs any bundle.
#[test]
fn a_product_install_that_widens_a_role_is_refused_unless_the_installer_holds_each_power() {
    let record = |version: &str| {
        SchemaChange::RecordProduct(InstalledProduct {
            product: ProductId::new("badges").unwrap(),
            version: version.into(),
            platform: None,
            owns: vec![],
            schema_version: None,
        })
    };
    let installer = with_roles(&base(), "stranger", &["schema_admin", "installer"]);

    let error = refused(change(
        &installer,
        "stranger",
        NOW + 50,
        vec![
            record("1.0.0"),
            permissions("reviewer", vec![capability("people.manage")]),
        ],
    ));
    widening_refused(&error, "reviewer", &["capability people.manage"]);

    change(&installer, "stranger", NOW + 51, vec![record("1.0.0")])
        .expect("an install that adds no power is accepted");

    change(
        &base(),
        "owner",
        NOW + 52,
        vec![
            record("1.0.0"),
            permissions("reviewer", vec![capability("people.manage")]),
        ],
    )
    .expect("the owner installs the bundle");
}

/// Verifies: ADM-1088
///
/// An upgrade is a `ChangeSchema` with the record of a later version, so it asks the same of the
/// upgrader: a version that widens a role is refused for an upgrader that lacks the power, and
/// the same upgrade without the widening is accepted.
#[test]
fn a_product_upgrade_that_widens_a_role_is_refused_unless_the_upgrader_holds_each_power() {
    let record = |version: &str| {
        SchemaChange::RecordProduct(InstalledProduct {
            product: ProductId::new("badges").unwrap(),
            version: version.into(),
            platform: None,
            owns: vec![],
            schema_version: None,
        })
    };
    let installed = change(&base(), "owner", NOW + 60, vec![record("1.0.0")])
        .expect("the owner installs version 1.0.0");
    let upgrader = with_roles(&installed, "stranger", &["schema_admin", "installer"]);

    let error = refused(change(
        &upgrader,
        "stranger",
        NOW + 61,
        vec![
            record("2.0.0"),
            permissions("reviewer", vec![capability("people.manage")]),
        ],
    ));
    widening_refused(&error, "reviewer", &["capability people.manage"]);

    change(&upgrader, "stranger", NOW + 62, vec![record("2.0.0")])
        .expect("an upgrade that adds no power is accepted");
}

/// Verifies: ADM-1088
///
/// Every power a change adds is named in the refusal, not only the first.
#[test]
fn a_refusal_names_every_power_the_changer_lacks() {
    let world = with_roles(&base(), "stranger", &["schema_admin", "reviewer"]);
    let error = refused(change(
        &world,
        "stranger",
        NOW + 70,
        vec![permissions(
            "reviewer",
            vec![
                capability("people.manage"),
                capability("content.publish"),
                object_permission("note", &[ObjectAction::Delete]),
            ],
        )],
    ));
    widening_refused(
        &error,
        "reviewer",
        &[
            "capability people.manage",
            "capability content.publish",
            "note:delete",
        ],
    );
}
