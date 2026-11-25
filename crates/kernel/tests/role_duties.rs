//! Scoped role assignments, self-action and separation of duties, and the last owner, on every
//! path that can touch them (D-8403, D-8405, D-8415, D-8426, D-8427).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use strata_kernel::ledger;
use strata_kernel::rules::roles_of;
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_model_core::types::{LedgerUnit, LedgerUnitDecl};

const NOW: i64 = 9_000;

// ---------------------------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------------------------

fn step(world: &World, who: &str, now: i64, intent: Intent) -> Result<World, DomainError> {
    world
        .step(&intent, &ctx("alpha", user(&format!("alpha:{who}")), now))
        .map(|(world, _)| world)
}

fn must(world: &World, who: &str, now: i64, intent: Intent) -> World {
    step(world, who, now, intent).unwrap_or_else(|e| panic!("refused: {}", e.describe()))
}

fn refused(world: &World, who: &str, now: i64, intent: Intent) -> DomainError {
    match step(world, who, now, intent) {
        Ok(_) => panic!("the intent was accepted"),
        Err(error) => error,
    }
}

fn change(
    world: &World,
    who: &str,
    now: i64,
    changes: Vec<SchemaChange>,
) -> Result<World, DomainError> {
    step(
        world,
        who,
        now,
        Intent::ChangeSchema {
            changes,
            expected_version: None,
            existing: ExistingData::Reject,
        },
    )
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

fn on_note(verbs: &[ObjectAction]) -> Permission {
    Permission::Object {
        object: object("note"),
        actions: verbs.iter().copied().collect(),
    }
}

fn capability(name: &str) -> Permission {
    Permission::Capability(name.to_string())
}

fn policy(
    id: &str,
    of: &str,
    resource: Resource,
    actions: &[Action],
    condition: Option<Expr>,
) -> SchemaChange {
    SchemaChange::DefinePolicy(PolicyDef {
        id: PolicyId::new(id).unwrap(),
        role: role(of),
        resource,
        actions: actions.iter().copied().collect(),
        condition,
    })
}

fn assign(who: &str, role_name: &str) -> Intent {
    Intent::AssignRole {
        principal: principal_id(&format!("alpha:{who}")),
        role: role(role_name),
    }
}

fn revoke(who: &str, role_name: &str) -> Intent {
    Intent::RevokeRole {
        principal: principal_id(&format!("alpha:{who}")),
        role: role(role_name),
    }
}

fn scoped(who: &str, role_name: &str, scope: Expr) -> Intent {
    Intent::AssignScopedRole {
        principal: principal_id(&format!("alpha:{who}")),
        role: role(role_name),
        object: object("note"),
        scope,
    }
}

fn slug_is(slug: &str) -> Expr {
    eq(var(Scope::Record, "slug"), lit(slug))
}

fn tenant(world: &World) -> &TenantState {
    &world.tenants[&tenant_id("alpha")]
}

fn update_title(id: u64, title: &str) -> Intent {
    Intent::UpdateRecord {
        object: object("note"),
        id: RecordId(id),
        fields: BTreeMap::from([(field("title"), text(title))]),
        expected_version: None,
    }
}

fn team(id: &str) -> TeamId {
    TeamId::new(id).unwrap()
}

fn failed_rules(error: &DomainError) -> Vec<&str> {
    error
        .evidence
        .iter()
        .filter(|check| !check.passed)
        .map(|check| check.rule.as_str())
        .collect()
}

/// The notes world with the roles below and two notes made by the owner: note 1 has the slug
/// `north`, note 2 the slug `south`; both are drafts.
///
/// - `regional_edit` may update notes (a permission).
/// - `regional_draft` may update notes that are drafts (a policy with a condition).
/// - `member_admin` manages members; `schema_admin` manages the schema; `reviewer` reads notes.
fn base() -> World {
    let world = change(
        &notes_world(),
        "owner",
        NOW,
        vec![
            define_role("regional_edit"),
            define_role("regional_draft"),
            define_role("member_admin"),
            define_role("schema_admin"),
            define_role("reviewer"),
            define_role("creator"),
            permissions("regional_edit", vec![on_note(&[ObjectAction::Update])]),
            permissions("reviewer", vec![on_note(&[ObjectAction::Read])]),
            permissions("creator", vec![on_note(&[ObjectAction::Create])]),
            policy(
                "regional_draft_updates",
                "regional_draft",
                Resource::Object(object("note")),
                &[Action::Update],
                Some(eq(var(Scope::Record, "status"), lit("draft"))),
            ),
            policy(
                "member_admins_manage_members",
                "member_admin",
                Resource::Members,
                &[Action::Manage],
                None,
            ),
            policy(
                "schema_admins_manage_schema",
                "schema_admin",
                Resource::Schema,
                &[Action::Manage],
                None,
            ),
        ],
    )
    .expect("the roles are defined");
    let world = must(&world, "owner", NOW + 1, new_note("North", "north"));
    must(&world, "owner", NOW + 2, new_note("South", "south"))
}

// ---------------------------------------------------------------------------------------------
// F-scoped-role-assignment.
// ---------------------------------------------------------------------------------------------

/// Verifies: ADM-1611
///
/// A role assigned with a scope applies to a record only where the predicate holds: a permission
/// of the role reaches the notes the scope names and no other, a policy of the role is ANDed with
/// the scope, and the role means nothing to someone who holds it with no scope.
#[test]
fn a_scoped_role_applies_only_where_its_predicate_holds() {
    let world = must(
        &base(),
        "owner",
        NOW + 3,
        scoped("stranger", "regional_edit", slug_is("north")),
    );
    let world = must(
        &world,
        "owner",
        NOW + 4,
        scoped("editor", "regional_draft", slug_is("north")),
    );

    // The permission reaches the north note, not the south one, and not a create.
    let world_after = must(&world, "stranger", NOW + 5, update_title(1, "North 2"));
    assert_eq!(
        refused(&world, "stranger", NOW + 6, update_title(2, "South 2")).kind,
        ErrorKind::Unauthorized
    );
    assert_eq!(
        refused(&world, "stranger", NOW + 7, new_note("Mine", "mine")).kind,
        ErrorKind::Unauthorized
    );
    assert_eq!(
        tenant(&world_after)
            .record(&object("note"), RecordId(1))
            .unwrap()
            .get(&field("title")),
        &text("North 2")
    );

    // The policy and the scope are both needed. The editor already updates every note through
    // its own role, so the check is made with a person who only has the scoped policy role.
    let world = must(
        &world,
        "owner",
        NOW + 8,
        scoped("writer2", "regional_draft", slug_is("north")),
    );
    must(&world, "writer2", NOW + 9, update_title(1, "By policy"));
    assert_eq!(
        refused(
            &world,
            "writer2",
            NOW + 10,
            update_title(2, "South by policy")
        )
        .kind,
        ErrorKind::Unauthorized
    );
    // Once the north note is published the policy's own condition no longer holds.
    let world = must(
        &world,
        "editor",
        NOW + 11,
        Intent::UpdateRecord {
            object: object("note"),
            id: RecordId(1),
            fields: BTreeMap::from([(field("status"), text("published"))]),
            expected_version: None,
        },
    );
    assert_eq!(
        refused(&world, "writer2", NOW + 12, update_title(1, "Too late")).kind,
        ErrorKind::Unauthorized
    );

    // The role held with no scope at all is not a role: it is never given.
    let plain = refused(&base(), "stranger", NOW + 13, update_title(1, "No role"));
    assert_eq!(plain.kind, ErrorKind::Unauthorized);
}

/// Verifies: ADM-1611
///
/// A predicate that names a field the object type does not declare is refused when the assignment
/// is made, with `ScopeUnknownField`, and so is a change of the schema that removes a field a
/// scope reads. The owner role and the public role are not narrowed.
#[test]
fn a_scope_naming_an_unknown_field_is_refused_at_definition() {
    let error = refused(
        &base(),
        "owner",
        NOW + 3,
        scoped(
            "stranger",
            "regional_edit",
            eq(var(Scope::Record, "region"), lit("north")),
        ),
    );
    assert_eq!(error.kind, ErrorKind::ScopeUnknownField);
    assert!(
        failed_rules(&error).contains(&"scope:predicate"),
        "{:?}",
        error.evidence
    );
    // Nothing was assigned.
    assert!(tenant(&base()).schema.scoped_roles.is_none());

    // An unknown object type is not an unknown field.
    let elsewhere = Intent::AssignScopedRole {
        principal: principal_id("alpha:stranger"),
        role: role("regional_edit"),
        object: object("nothing"),
        scope: slug_is("north"),
    };
    assert_eq!(
        refused(&base(), "owner", NOW + 4, elsewhere).kind,
        ErrorKind::UnknownObject
    );

    // A schema change may not remove a field a held scope reads.
    let world = must(
        &base(),
        "owner",
        NOW + 5,
        scoped("stranger", "regional_edit", slug_is("north")),
    );
    let removal = change(
        &world,
        "owner",
        NOW + 6,
        vec![SchemaChange::RemoveField {
            object: object("note"),
            field: field("slug"),
        }],
    );
    assert!(removal.is_err(), "the scope still reads the slug");

    for built_in in ["owner", "public"] {
        let error = refused(
            &base(),
            "owner",
            NOW + 7,
            scoped("stranger", built_in, slug_is("north")),
        );
        assert_eq!(error.kind, ErrorKind::InvalidIntent, "{built_in}");
    }
}

/// Verifies: ADM-1611
///
/// A narrower scope never confers more (and is consistent with the role-widening check of
/// ADM-1088): a person who holds a role only through a scope does not hold it, so it counts for
/// nothing when that person widens a role, adds nothing to the roles they hold, and a person who
/// lacks a power of the role cannot hand it out narrowed either. Taking the role away takes the
/// scoped assignments, and erasing the person takes them too.
#[test]
fn a_scoped_role_never_confers_more_than_the_role() {
    // The stranger changes schemas and holds `reviewer` only through a scope on notes.
    let world = must(
        &base(),
        "owner",
        NOW + 3,
        assign("stranger", "schema_admin"),
    );
    let world = must(
        &world,
        "owner",
        NOW + 4,
        scoped("stranger", "reviewer", slug_is("north")),
    );
    let roles = roles_of(tenant(&world), &user("alpha:stranger"));
    assert!(!roles.contains(&role("reviewer")), "{roles:?}");

    // Widening `creator` to read notes asks for a power the stranger does not hold.
    let widen = vec![permissions(
        "creator",
        vec![on_note(&[ObjectAction::Create, ObjectAction::Read])],
    )];
    let error = change(&world, "stranger", NOW + 5, widen.clone()).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Unauthorized, "{}", error.message);
    assert!(
        failed_rules(&error).contains(&"permission:widen:creator"),
        "{:?}",
        error.evidence
    );
    // Holding the role whole, the same change is accepted.
    let whole = must(&world, "owner", NOW + 6, assign("stranger", "reviewer"));
    change(&whole, "stranger", NOW + 7, widen).expect("a holder of the role widens with it");

    // A member administrator who does not hold `reviewer` cannot hand it out, narrowed or not.
    let world = must(
        &base(),
        "owner",
        NOW + 3,
        assign("stranger", "member_admin"),
    );
    let error = refused(
        &world,
        "stranger",
        NOW + 4,
        scoped("writer2", "reviewer", slug_is("north")),
    );
    assert_eq!(error.kind, ErrorKind::Unauthorized);
    assert!(
        failed_rules(&error).contains(&"permission:confer:reviewer"),
        "{:?}",
        error.evidence
    );

    // Taking the role away takes its scoped assignments with it.
    let world = must(
        &base(),
        "owner",
        NOW + 3,
        scoped("stranger", "regional_edit", slug_is("north")),
    );
    must(&world, "stranger", NOW + 4, update_title(1, "Allowed"));
    let world = must(
        &world,
        "owner",
        NOW + 5,
        revoke("stranger", "regional_edit"),
    );
    assert!(tenant(&world).schema.scoped_roles.is_none());
    assert_eq!(
        refused(&world, "stranger", NOW + 6, update_title(1, "Gone")).kind,
        ErrorKind::Unauthorized
    );

    // Erasing the person erases their scoped assignments.
    let world = must(
        &base(),
        "owner",
        NOW + 3,
        scoped("stranger", "regional_edit", slug_is("north")),
    );
    let world = must(
        &world,
        "owner",
        NOW + 4,
        Intent::EraseSubject {
            subject: principal_id("alpha:stranger"),
            resume_object: None,
            resume_after: None,
            held_objects: Vec::new(),
            addressed_invitations: Vec::new(),
        },
    );
    assert!(tenant(&world).schema.scoped_roles.is_none());
}

// ---------------------------------------------------------------------------------------------
// F-separation-of-duties: the self-action of an operation.
// ---------------------------------------------------------------------------------------------

fn review_ref() -> FieldType {
    FieldType::Ref {
        target: object("review"),
        on_delete: OnDelete::Restrict,
    }
}

/// Approve a review: it is about a person, who may not approve their own.
fn approve_review() -> TenantOperationDef {
    TenantOperationDef {
        id: TenantOperationId::new("approve_review").unwrap(),
        label: "Approve a review".into(),
        inputs: vec![TenantOperationInput {
            name: "review".into(),
            ty: review_ref(),
            required: true,
            label: None,
        }],
        invokers: TenantOperationInvokers {
            signed_in: true,
            roles: BTreeSet::new(),
        },
        subject: Some(TenantOperationSubject {
            object: object("review"),
            input: "review".into(),
            self_action: Some(field("about")),
        }),
        conditions: vec![],
        steps: vec![TenantOperationStep::UpdateRecord {
            id: "approve".into(),
            object: object("review"),
            record: Expr::Param("review".into()),
            fields: BTreeMap::from([(field("state"), lit("approved"))]),
        }],
    }
}

fn invoke(review: u64) -> Intent {
    Intent::InvokeOperation {
        operation: TenantOperationId::new("approve_review").unwrap(),
        inputs: BTreeMap::from([("review".to_string(), Value::Ref(RecordId(review)))]),
        idempotency_key: format!("k-{review}"),
    }
}

/// Verifies: ADM-1613
///
/// An operation that declares a self-action refusal on its subject refuses a person on a record
/// that names that person, with `SelfAction`, whatever roles they hold, and the refusal names its
/// rule in the evidence. The same person approves another person's record, and another person
/// approves theirs. A declaration that names a field the record does not have is refused when
/// the operation is defined.
#[test]
fn an_operation_refuses_a_person_acting_on_a_record_about_themselves() {
    let world = change(
        &notes_world(),
        "owner",
        NOW,
        vec![
            SchemaChange::DefineObject {
                id: object("review"),
                label: "Review".into(),
            },
            SchemaChange::DefineField {
                object: object("review"),
                field: field_def("about", FieldType::Str, true),
            },
            SchemaChange::DefineField {
                object: object("review"),
                field: field_def("state", FieldType::Str, false),
            },
            policy(
                "everyone_reads_reviews",
                "public",
                Resource::Object(object("review")),
                &[Action::Read],
                None,
            ),
            SchemaChange::DefineOperation(approve_review()),
        ],
    )
    .expect("the operation is defined");
    let make = |about: &str| Intent::CreateRecord {
        object: object("review"),
        fields: BTreeMap::from([(field("about"), text(about))]),
    };
    let world = must(&world, "owner", NOW + 1, make("alpha:writer1"));
    let world = must(&world, "owner", NOW + 2, make("alpha:editor"));

    // The person the review is about is refused, with a named rule.
    let error = refused(&world, "writer1", NOW + 3, invoke(1));
    assert_eq!(error.kind, ErrorKind::SelfAction, "{}", error.message);
    assert_eq!(
        failed_rules(&error),
        ["operation:approve_review:self_action"]
    );
    // The owner holds every role and is refused on a review about the owner just the same.
    let world = must(&world, "owner", NOW + 4, make("alpha:owner"));
    assert_eq!(
        refused(&world, "owner", NOW + 5, invoke(3)).kind,
        ErrorKind::SelfAction
    );
    // Others act on it, and a person acts on the review about someone else.
    must(&world, "editor", NOW + 6, invoke(1));
    must(&world, "writer1", NOW + 7, invoke(2));

    // A declaration over a field the subject does not have, or one that is not text, is refused.
    let mut wrong = approve_review();
    wrong.subject.as_mut().unwrap().self_action = Some(field("nothing"));
    assert!(change(
        &world,
        "owner",
        NOW + 8,
        vec![SchemaChange::DefineOperation(wrong)]
    )
    .is_err());
}

// ---------------------------------------------------------------------------------------------
// F-last-owner-guard.
// ---------------------------------------------------------------------------------------------

fn last_owner(error: &DomainError) {
    assert_eq!(error.kind, ErrorKind::LastOwner, "{}", error.message);
    assert!(
        failed_rules(error).contains(&"intent:last_owner")
            || failed_rules(error).contains(&"erasure:an_owner_remains"),
        "the refusal is a failed check: {:?}",
        error.evidence
    );
}

fn owners(world: &World) -> BTreeSet<PrincipalId> {
    strata_kernel::rules::owners_of(tenant(world))
}

/// Verifies: ADM-1624
///
/// Taking the owner role away from the last owner is refused with `LastOwner`; with a second
/// owner it is accepted, and then the new last owner cannot give it up.
#[test]
fn unassigning_the_owner_role_keeps_one_owner() {
    let world = base();
    last_owner(&refused(&world, "owner", NOW + 3, revoke("owner", "owner")));
    let world = must(&world, "owner", NOW + 4, assign("editor", "owner"));
    let world = must(&world, "editor", NOW + 5, revoke("owner", "owner"));
    assert_eq!(
        owners(&world),
        BTreeSet::from([principal_id("alpha:editor")])
    );
    last_owner(&refused(
        &world,
        "editor",
        NOW + 6,
        revoke("editor", "owner"),
    ));
}

/// Verifies: ADM-1624
///
/// An owner through a team counts, and a change to the team that would leave none is refused:
/// taking the owner role from the team, and taking the last member out of it. A team that holds
/// owner, with a direct owner beside it, can lose either.
#[test]
fn team_role_changes_keep_one_owner() {
    let world = base();
    let board = team("board");
    let world = must(
        &world,
        "owner",
        NOW + 3,
        Intent::DefineTeam {
            team: board.clone(),
            label: "Board".into(),
        },
    );
    let world = must(
        &world,
        "owner",
        NOW + 4,
        Intent::AssignTeamRole {
            team: board.clone(),
            role: role("owner"),
        },
    );
    let world = must(
        &world,
        "owner",
        NOW + 5,
        Intent::AddTeamMember {
            team: board.clone(),
            principal: principal_id("alpha:editor"),
        },
    );
    // While the direct owner remains, the team can lose the role, and the member can leave.
    must(
        &world,
        "owner",
        NOW + 6,
        Intent::RevokeTeamRole {
            team: board.clone(),
            role: role("owner"),
        },
    );
    must(
        &world,
        "owner",
        NOW + 7,
        Intent::RemoveTeamMember {
            team: board.clone(),
            principal: principal_id("alpha:editor"),
        },
    );
    // The direct owner steps down: the team's one member is the last owner.
    let world = must(&world, "owner", NOW + 8, revoke("owner", "owner"));
    assert_eq!(
        owners(&world),
        BTreeSet::from([principal_id("alpha:editor")])
    );
    last_owner(&refused(
        &world,
        "editor",
        NOW + 9,
        Intent::RevokeTeamRole {
            team: board.clone(),
            role: role("owner"),
        },
    ));
    last_owner(&refused(
        &world,
        "editor",
        NOW + 10,
        Intent::RemoveTeamMember {
            team: board,
            principal: principal_id("alpha:editor"),
        },
    ));
}

/// Verifies: ADM-1624
///
/// Deleting a member (the erasure of a person, which removes their roles) is refused when the
/// person is the last owner, and accepted when another owner remains.
#[test]
fn deleting_a_member_keeps_one_owner() {
    let erase = |who: &str| Intent::EraseSubject {
        subject: principal_id(&format!("alpha:{who}")),
        resume_object: None,
        resume_after: None,
        held_objects: Vec::new(),
        addressed_invitations: Vec::new(),
    };
    let world = base();
    last_owner(&refused(&world, "owner", NOW + 3, erase("owner")));
    let world = must(&world, "owner", NOW + 4, assign("editor", "owner"));
    let world = must(&world, "editor", NOW + 5, erase("owner"));
    assert_eq!(
        owners(&world),
        BTreeSet::from([principal_id("alpha:editor")])
    );
}

/// Verifies: ADM-1624
///
/// A change of role definitions cannot remove or redefine the owner role, which would demote every
/// owner, whoever makes it, and neither can the install or upgrade of a product, which reaches the
/// schema through the same change.
#[test]
fn a_role_definition_or_a_product_upgrade_keeps_one_owner() {
    let world = base();
    for changes in [
        vec![SchemaChange::RemoveRole(role("owner"))],
        vec![SchemaChange::DefineRole(RoleDef {
            id: role("owner"),
            label: "Renamed".into(),
        })],
        // A product's upgrade: its record, with a role definition that takes the owner role out.
        vec![
            SchemaChange::RecordProduct(InstalledProduct {
                product: ProductId::new("demo").unwrap(),
                version: "2.0.0".into(),
                platform: None,
                owns: vec![],
                schema_version: None,
            }),
            SchemaChange::RemoveRole(role("owner")),
        ],
    ] {
        let error = change(&world, "owner", NOW + 3, changes).unwrap_err();
        last_owner(&error);
    }
    // A tenant that has no owner is not held to it; the owner role is still the platform's.
    assert_eq!(
        owners(&world),
        BTreeSet::from([principal_id("alpha:owner")])
    );
}

// ---------------------------------------------------------------------------------------------
// F-separation-of-duties: duties the platform keeps apart; the ledger.
// ---------------------------------------------------------------------------------------------

fn conflicting(error: &DomainError) {
    assert_eq!(error.kind, ErrorKind::ConflictingRoles, "{}", error.message);
    assert!(
        failed_rules(error).contains(&"intent:conflicting_roles"),
        "{:?}",
        error.evidence
    );
}

/// The base world with `poster` (the capability that posts to the ledger) and `auditor` (the
/// capability that reads every account), and the unit `points`.
fn ledger_world() -> World {
    change(
        &base(),
        "owner",
        NOW + 20,
        vec![
            define_role("poster"),
            define_role("auditor"),
            define_role("poster_too"),
            permissions("poster", vec![capability(ledger::LEDGER_POST)]),
            permissions("auditor", vec![capability(ledger::LEDGER_READ)]),
            permissions("poster_too", vec![capability(ledger::LEDGER_POST)]),
            SchemaChange::DeclareLedgerUnit(LedgerUnitDecl {
                unit: LedgerUnit::new("points").unwrap(),
                description: "Points".into(),
                expiry: None,
            }),
        ],
    )
    .expect("the ledger roles are defined")
}

/// Verifies: ADM-1630
///
/// No principal holds both the role that posts to the ledger and the role that reviews it. The
/// assignment that would give one person both is refused with `ConflictingRoles` and a failed
/// check, on every path that gives a role: a direct assignment, a scoped assignment, a role of a
/// team the person is in, a person added to such a team, an invitation, and a change that makes
/// one role grant both. Each duty alone is held, and the owner is not counted.
#[test]
fn no_path_gives_one_person_the_poster_and_the_reviewer() {
    let world = ledger_world();
    // One duty each, and a second role of the same duty, is fine.
    let world = must(&world, "owner", NOW + 30, assign("writer1", "poster"));
    let world = must(&world, "owner", NOW + 31, assign("writer1", "poster_too"));
    let world = must(&world, "owner", NOW + 32, assign("writer2", "auditor"));

    // Direct and scoped assignments.
    conflicting(&refused(
        &world,
        "owner",
        NOW + 33,
        assign("writer1", "auditor"),
    ));
    conflicting(&refused(
        &world,
        "owner",
        NOW + 34,
        scoped("writer1", "auditor", slug_is("north")),
    ));
    conflicting(&refused(
        &world,
        "owner",
        NOW + 35,
        assign("writer2", "poster"),
    ));
    let scoped_world = must(
        &world,
        "owner",
        NOW + 36,
        scoped("stranger", "poster", slug_is("north")),
    );
    conflicting(&refused(
        &scoped_world,
        "owner",
        NOW + 37,
        assign("stranger", "auditor"),
    ));

    // A team that holds one duty: a person who holds the other cannot join it, and the team
    // cannot take a duty its member already holds the other of.
    let desk = team("desk");
    let with_team = must(
        &world,
        "owner",
        NOW + 38,
        Intent::DefineTeam {
            team: desk.clone(),
            label: "Desk".into(),
        },
    );
    let with_team = must(
        &with_team,
        "owner",
        NOW + 39,
        Intent::AssignTeamRole {
            team: desk.clone(),
            role: role("auditor"),
        },
    );
    conflicting(&refused(
        &with_team,
        "owner",
        NOW + 40,
        Intent::AddTeamMember {
            team: desk.clone(),
            principal: principal_id("alpha:writer1"),
        },
    ));
    let joined = must(
        &with_team,
        "owner",
        NOW + 41,
        Intent::AddTeamMember {
            team: desk.clone(),
            principal: principal_id("alpha:stranger"),
        },
    );
    conflicting(&refused(
        &joined,
        "owner",
        NOW + 42,
        assign("stranger", "poster"),
    ));
    let poster_team = must(
        &joined,
        "owner",
        NOW + 43,
        Intent::DefineTeam {
            team: team("tills"),
            label: "Tills".into(),
        },
    );
    let poster_team = must(
        &poster_team,
        "owner",
        NOW + 44,
        Intent::AddTeamMember {
            team: team("tills"),
            principal: principal_id("alpha:stranger"),
        },
    );
    conflicting(&refused(
        &poster_team,
        "owner",
        NOW + 45,
        Intent::AssignTeamRole {
            team: team("tills"),
            role: role("poster"),
        },
    ));

    // An invitation: its redeemer already reviews the ledger, and the invitation offers the post.
    let invited = must(
        &world,
        "owner",
        NOW + 46,
        Intent::CreateInvitation {
            invitation: InvitationId::new("inv1").unwrap(),
            address: "newcomer@example.test".into(),
            roles: BTreeSet::from([role("poster")]),
        },
    );
    let mut as_reviewer = ctx("alpha", user("alpha:writer2"), NOW + 47);
    as_reviewer.verified_address = Some("newcomer@example.test".into());
    let error = invited
        .step(
            &Intent::RedeemInvitation {
                invitation: InvitationId::new("inv1").unwrap(),
            },
            &as_reviewer,
        )
        .unwrap_err();
    conflicting(&error);
    // Another person redeems it.
    let mut as_other = ctx("alpha", user("alpha:stranger"), NOW + 48);
    as_other.verified_address = Some("newcomer@example.test".into());
    invited
        .step(
            &Intent::RedeemInvitation {
                invitation: InvitationId::new("inv1").unwrap(),
            },
            &as_other,
        )
        .expect("a person who holds neither redeems it");

    // A change that makes the role of a person grant the other duty too.
    let error = change(
        &world,
        "owner",
        NOW + 49,
        vec![permissions(
            "poster",
            vec![
                capability(ledger::LEDGER_POST),
                capability(ledger::LEDGER_READ),
            ],
        )],
    )
    .unwrap_err();
    conflicting(&error);
    // A role nobody holds can grant both until someone is given it.
    let unheld = change(
        &world,
        "owner",
        NOW + 50,
        vec![
            define_role("both"),
            permissions(
                "both",
                vec![
                    capability(ledger::LEDGER_POST),
                    capability(ledger::LEDGER_READ),
                ],
            ),
        ],
    )
    .expect("a role no one holds");
    conflicting(&refused(
        &unheld,
        "owner",
        NOW + 51,
        assign("stranger", "both"),
    ));

    // The owner holds every power and is not counted.
    let world = must(&world, "owner", NOW + 52, assign("editor", "owner"));
    must(&world, "owner", NOW + 53, assign("editor", "poster"));
}

fn post(owner: &str, amount: i64, key: &str, reason: &str, corrects: Option<u64>) -> Intent {
    Intent::PostLedger {
        postings: vec![LedgerPostingRequest {
            owner: LedgerOwner::Principal {
                principal: principal_id(&format!("alpha:{owner}")),
            },
            unit: LedgerUnit::new("points").unwrap(),
            amount: LedgerAmount(amount),
            idempotency_key: key.into(),
            reason: reason.into(),
            corrects,
            scope: None,
        }],
    }
}

/// Verifies: ADM-1629
///
/// A person does not post an adjustment or a reversal (the reasons `adjust` and `reverse`) to a
/// ledger account they own, whatever roles they hold, the owner included: the refusal is `SelfAction` with the check
/// `ledger:self_action`, and nothing is posted. Their own ordinary postings, an adjustment to
/// someone else's account and a reversal posted by someone else are accepted.
#[test]
fn nobody_adjusts_or_reverses_their_own_account() {
    let world = must(
        &ledger_world(),
        "owner",
        NOW + 30,
        assign("writer1", "poster"),
    );
    // An earlier posting by the owner to the poster's account, to reverse.
    let world = must(
        &world,
        "owner",
        NOW + 31,
        post("writer1", 10, "first", "award", None),
    );
    let self_action = |error: &DomainError| {
        assert_eq!(error.kind, ErrorKind::SelfAction, "{}", error.message);
        assert!(
            failed_rules(error).contains(&"ledger:self_action"),
            "{:?}",
            error.evidence
        );
    };

    self_action(&refused(
        &world,
        "writer1",
        NOW + 32,
        post("writer1", 5, "adj", "adjust", None),
    ));
    self_action(&refused(
        &world,
        "writer1",
        NOW + 33,
        post("writer1", -10, "rev", "reverse", None),
    ));
    // A correction under another reason is no adjustment: an operation that takes back its own
    // earlier posting (a reaction taken back) is not refused.
    must(
        &world,
        "writer1",
        NOW + 34,
        post("writer1", -10, "fix", "typo", Some(1)),
    );
    // The owner holds every role, and still may not adjust the owner's own balance.
    self_action(&refused(
        &world,
        "owner",
        NOW + 35,
        post("owner", 5, "own", "adjust", None),
    ));

    // What the rule leaves alone.
    must(
        &world,
        "writer1",
        NOW + 36,
        post("writer1", 3, "plain", "award", None),
    );
    must(
        &world,
        "writer1",
        NOW + 37,
        post("writer2", 5, "other", "adjust", None),
    );
    must(
        &world,
        "owner",
        NOW + 38,
        post("writer1", -10, "rev-by-owner", "reverse", Some(1)),
    );
}

// ---------------------------------------------------------------------------------------------
// Scoped roles in lists, queries, aggregates and audiences (ADM-1611, WG-5642).
// ---------------------------------------------------------------------------------------------

fn named_read(query: &str) -> ReadRequest {
    ReadRequest::Named {
        query: QueryId::new(query).unwrap(),
        params: BTreeMap::new(),
        page_size: None,
        cursor: None,
    }
}

fn row_ids(world: &World, who: &str, request: &ReadRequest) -> Vec<u64> {
    world
        .read(
            request,
            &ctx("alpha", user(&format!("alpha:{who}")), NOW + 50),
        )
        .expect("the read is answered")
        .rows
        .iter()
        .map(|record| record.id.0)
        .collect()
}

/// A query over the notes that declares a count, for the aggregate tests.
fn tally_query() -> SchemaChange {
    SchemaChange::DefineQuery(QueryDef {
        id: QueryId::new("tally").unwrap(),
        object: object("note"),
        filter: None,
        params: BTreeSet::new(),
        order: vec![],
        limit: None,
        aggregates: Some(vec![AggregateDef {
            id: AggregateId::new("how_many").unwrap(),
            kind: AggregateKind::Count,
            field: None,
            group_by: None,
            group_by_second: None,
            n: None,
            descending: false,
            scan_bound: 100,
        }]),
        declared_fields: Some(vec![field("status")]),
        tree: None,
    })
}

/// Verifies: ADM-1644
///
/// A role held through a scope decides a list, a query and an aggregate on each record the way it
/// decides a single record: a reviewer scoped to the north note reads that note and no other in
/// a named query, a read by id and the count of an aggregate, and a reader with no scope reads
/// nothing of the drafts.
#[test]
fn a_scoped_role_decides_lists_queries_and_aggregates_like_a_single_record() {
    let world =
        change(&base(), "owner", NOW + 2, vec![tally_query()]).expect("the query is defined");
    let world = must(
        &world,
        "owner",
        NOW + 3,
        scoped("stranger", "reviewer", slug_is("north")),
    );

    let list = named_read("notes_by_rank");
    assert_eq!(row_ids(&world, "stranger", &list), vec![1]);
    assert_eq!(
        row_ids(&world, "owner", &list).len(),
        2,
        "the owner reads both notes"
    );
    assert!(row_ids(&world, "nobody", &list).is_empty());

    let by_id = ReadRequest::Records {
        object: object("note"),
        ids: vec![RecordId(1), RecordId(2)],
    };
    assert_eq!(row_ids(&world, "stranger", &by_id), vec![1]);

    let count = |who: &str| {
        let request = AggregateRequest {
            query: QueryId::new("tally").unwrap(),
            aggregate: AggregateId::new("how_many").unwrap(),
            params: BTreeMap::new(),
            filter: BTreeMap::new(),
        };
        let state = world.state(&tenant_id("alpha"));
        let result = strata_kernel::aggregates::run(
            &state,
            &request,
            &ctx("alpha", user(&format!("alpha:{who}")), NOW + 51),
        )
        .expect("the aggregate is answered");
        result.groups[0].value.clone()
    };
    assert_eq!(count("stranger"), Some(Value::Int(1)));
    assert_eq!(count("owner"), Some(Value::Int(2)));
    assert_eq!(count("nobody"), Some(Value::Int(0)));

    // Taking the role away takes the rows away.
    let world = must(&world, "owner", NOW + 4, revoke("stranger", "reviewer"));
    assert!(row_ids(&world, "stranger", &list).is_empty());
}

/// Verifies: ADM-1644
///
/// A reader who holds a role through a scope on a carried object type is judged alone: their
/// audience class is the individual, never the class of the readers who hold the same direct
/// roles, so a copy made for one is neither served to nor taken from the other.
#[test]
fn a_reader_with_a_scoped_role_is_an_individual_audience() {
    let world =
        change(&base(), "owner", NOW + 2, vec![tally_query()]).expect("the query is defined");
    let world = must(
        &world,
        "owner",
        NOW + 3,
        scoped("stranger", "reviewer", slug_is("north")),
    );
    let tenant = tenant(&world);
    let query = QueryId::new("notes_by_rank").unwrap();
    let aggregate = AggregateId::new("how_many").unwrap();
    let class = |who: &str, reading: audience::Reading<'_>| {
        audience::class_of(tenant, reading, &user(&format!("alpha:{who}")))
    };
    let readings = [
        audience::Reading::Query(&query),
        audience::Reading::Aggregate {
            query: &QueryId::new("tally").unwrap(),
            aggregate: &aggregate,
        },
    ];
    for reading in readings {
        assert!(
            matches!(
                class("stranger", reading),
                audience::AudienceClass::Individual(_)
            ),
            "the scoped reader is individual"
        );
        assert!(
            class("writer1", reading).is_shareable(),
            "a reader with no scope still shares a copy"
        );
    }
}

/// Verifies: ADM-1644
///
/// A scope that follows a reference needs the record it reaches like a Read policy does: a list
/// read for the scoped reader names the person a note's author refers to, and the delivery of an
/// event about a note loads it, so neither answers for lack of a record that was merely not loaded.
#[test]
fn a_scope_that_follows_a_reference_names_the_record_it_needs() {
    let world = base();
    let world = must(
        &world,
        "owner",
        NOW + 2,
        Intent::CreateRecord {
            object: object("person"),
            fields: BTreeMap::from([(field("name"), text("Ann"))]),
        },
    );
    let world = must(
        &world,
        "owner",
        NOW + 3,
        Intent::UpdateRecord {
            object: object("note"),
            id: RecordId(1),
            fields: BTreeMap::from([(field("author"), Value::Ref(RecordId(3)))]),
            expected_version: None,
        },
    );
    let by_author = eq(
        Expr::Through(Scope::Record, "author".into(), "name".into()),
        lit("Ann"),
    );
    let world = must(
        &world,
        "owner",
        NOW + 4,
        scoped("stranger", "reviewer", by_author),
    );

    // The snapshot holds the notes and nothing the notes refer to.
    let full = world.state(&tenant_id("alpha"));
    let mut thin = TenantState::clone(full.tenant.as_deref().unwrap());
    thin.records.remove(&object("person"));
    let request = named_read("notes_by_rank");
    let wanted: BTreeSet<Need> = read_needs(&thin.schema, &request).into_iter().collect();
    let state = State {
        tenant_id: tenant_id("alpha"),
        tenant: Some(Arc::new(thin)),
        coverage: Coverage::Partial(wanted),
    };
    let context = ctx("alpha", user("alpha:stranger"), NOW + 60);
    let missing = strata_kernel::query_plan::read_missing(&state, &request, &context);
    assert!(
        missing.contains(&Need::Record {
            object: object("person"),
            id: RecordId(3)
        }),
        "the scope follows the author, so the author is named: {missing:?}"
    );

    let record = tenant(&world)
        .record(&object("note"), RecordId(1))
        .unwrap()
        .clone();
    assert_eq!(audience_needs(&tenant(&world).schema, &record).len(), 1);
}

// ---------------------------------------------------------------------------------------------
// Declared separations of duties and the taking away of a scoped assignment (WG-5642).
// ---------------------------------------------------------------------------------------------

fn separation(id: &str, left: &str, right: &str) -> SchemaChange {
    SchemaChange::DefineSeparation(SeparationDef {
        id: SeparationId::new(id).unwrap(),
        left: CapabilityName(left.into()),
        left_duty: "pays out".into(),
        right: CapabilityName(right.into()),
        right_duty: "approves payments".into(),
    })
}

/// The notes world with a role that pays (the capability `tenant-configure`), a role that approves
/// (`segments-define`) and the separation `pay_approve` that keeps them apart.
fn separated() -> World {
    change(
        &base(),
        "owner",
        NOW + 10,
        vec![
            define_role("payer"),
            define_role("approver"),
            permissions("payer", vec![capability("tenant-configure")]),
            permissions("approver", vec![capability("segments-define")]),
            separation("pay_approve", "tenant-configure", "segments-define"),
        ],
    )
    .expect("the roles and the separation are defined")
}

/// Verifies: ADM-1642
/// Verifies: ADM-1646
///
/// A separation a tenant declares is held to by the rule of the platform's own, on every path that
/// gives a person a role: a direct assignment, a scoped assignment, a team's role, a person added
/// to a team and a change of what a role grants, each refused with `ConflictingRoles` and naming
/// the two duties. The built-in owner holds both without being counted, and a person who holds
/// only one duty is not refused.
#[test]
fn a_declared_separation_is_held_to_on_every_path() {
    let world = must(&separated(), "owner", NOW + 11, assign("alice", "payer"));

    let direct = refused(&world, "owner", NOW + 12, assign("alice", "approver"));
    assert_eq!(direct.kind, ErrorKind::ConflictingRoles);
    assert!(
        direct.message.contains("pays out") && direct.message.contains("approves payments"),
        "{}",
        direct.message
    );
    assert_eq!(
        refused(
            &world,
            "owner",
            NOW + 13,
            scoped("alice", "approver", slug_is("north"))
        )
        .kind,
        ErrorKind::ConflictingRoles
    );
    // A person who holds only one duty is not refused, and neither is the owner.
    let world = must(&world, "owner", NOW + 14, assign("bob", "approver"));
    let world = must(&world, "owner", NOW + 15, assign("owner", "approver"));

    // Through a team: the team's role reaches its members, and a member joining reaches its roles.
    let world = must(
        &world,
        "owner",
        NOW + 16,
        Intent::DefineTeam {
            team: team("desk"),
            label: "Desk".into(),
        },
    );
    let world = must(
        &world,
        "owner",
        NOW + 17,
        Intent::AddTeamMember {
            team: team("desk"),
            principal: principal_id("alpha:alice"),
        },
    );
    assert_eq!(
        refused(
            &world,
            "owner",
            NOW + 18,
            Intent::AssignTeamRole {
                team: team("desk"),
                role: role("approver"),
            }
        )
        .kind,
        ErrorKind::ConflictingRoles
    );
    // A person joining a team whose role gives the other duty.
    let world = must(&separated(), "owner", NOW + 21, assign("bob", "approver"));
    let world = must(
        &world,
        "owner",
        NOW + 22,
        Intent::DefineTeam {
            team: team("desk"),
            label: "Desk".into(),
        },
    );
    let world = must(
        &world,
        "owner",
        NOW + 23,
        Intent::AssignTeamRole {
            team: team("desk"),
            role: role("payer"),
        },
    );
    assert_eq!(
        refused(
            &world,
            "owner",
            NOW + 24,
            Intent::AddTeamMember {
                team: team("desk"),
                principal: principal_id("alpha:bob"),
            }
        )
        .kind,
        ErrorKind::ConflictingRoles
    );

    // A change of what a role grants.
    let world = must(&separated(), "owner", NOW + 25, assign("alice", "payer"));
    let grants = change(
        &world,
        "owner",
        NOW + 26,
        vec![permissions(
            "payer",
            vec![
                capability("tenant-configure"),
                capability("segments-define"),
            ],
        )],
    )
    .expect_err("the payer would hold both duties");
    assert_eq!(grants.kind, ErrorKind::ConflictingRoles);
}

/// Verifies: ADM-1642
/// Verifies: ADM-1646
///
/// Defining a separation that a person already breaks makes a pair and is refused, and taking one
/// away needs the capability `separations-define`, which a manager of the schema does not hold
/// until a role grants it.
#[test]
fn a_separation_is_defined_and_removed_by_one_who_may_and_not_over_a_pair_already_held() {
    // A schema manager without the capability is refused to define or remove one.
    let world = change(
        &separated(),
        "owner",
        NOW + 30,
        vec![permissions("schema_admin", Vec::new())],
    )
    .expect("the role is defined");
    let world = must(&world, "owner", NOW + 31, assign("sam", "schema_admin"));
    let define = vec![separation("other", "tenant-configure", "ledger-read")];
    assert_eq!(
        change(&world, "sam", NOW + 32, define.clone())
            .expect_err("sam lacks separations-define")
            .kind,
        ErrorKind::Unauthorized
    );
    let remove = vec![SchemaChange::RemoveSeparation(
        SeparationId::new("pay_approve").unwrap(),
    )];
    assert_eq!(
        change(&world, "sam", NOW + 33, remove.clone())
            .expect_err("sam lacks separations-define")
            .kind,
        ErrorKind::Unauthorized
    );
    // The role grants it, and the owner conferred it.
    let world = change(
        &world,
        "owner",
        NOW + 34,
        vec![permissions(
            "schema_admin",
            vec![capability("separations-define")],
        )],
    )
    .expect("the owner confers the capability");
    let world = change(&world, "sam", NOW + 35, define).expect("sam defines one");
    assert!(tenant(&world)
        .schema
        .separations
        .as_ref()
        .is_some_and(|held| held.contains_key(&SeparationId::new("other").unwrap())));
    let world = change(&world, "sam", NOW + 36, remove).expect("sam removes one");

    // With the separation gone a person holds both duties; declaring it again is a pair made.
    let world = must(&world, "owner", NOW + 37, assign("alice", "payer"));
    let world = must(&world, "owner", NOW + 38, assign("alice", "approver"));
    let again = change(
        &world,
        "owner",
        NOW + 39,
        vec![separation(
            "pay_approve",
            "tenant-configure",
            "segments-define",
        )],
    )
    .expect_err("alice already holds both duties");
    assert_eq!(again.kind, ErrorKind::ConflictingRoles);
    // A separation that names a capability of the product form that nobody declared, or the
    // same capability twice, is not a separation.
    for bad in [
        separation("bad_a", "shop__approve", "tenant-configure"),
        separation("bad_b", "tenant-configure", "tenant-configure"),
    ] {
        assert_eq!(
            change(&world, "owner", NOW + 40, vec![bad])
                .expect_err("not a separation")
                .kind,
            ErrorKind::InvalidSchema
        );
    }
}

/// Verifies: ADM-1647
///
/// The platform's own separation stays in force whatever a tenant declares: its id is not declared
/// again or removed, and declaring others leaves the poster and the reviewer of the ledger apart.
#[test]
fn the_platforms_separation_stays_whatever_is_declared() {
    let world = separated();
    for change_of in [
        separation(
            "ledger-adjust-and-review",
            "tenant-configure",
            "segments-define",
        ),
        SchemaChange::RemoveSeparation(SeparationId::new("ledger-adjust-and-review").unwrap()),
    ] {
        assert_eq!(
            change(&world, "owner", NOW + 41, vec![change_of])
                .expect_err("the platform's separation is not changed")
                .kind,
            ErrorKind::InvalidSchema
        );
    }
    let world = change(
        &world,
        "owner",
        NOW + 42,
        vec![
            define_role("poster"),
            define_role("auditor"),
            permissions("poster", vec![capability("ledger-post")]),
            permissions("auditor", vec![capability("ledger-read")]),
        ],
    )
    .expect("the ledger roles are defined");
    let world = must(&world, "owner", NOW + 43, assign("alice", "poster"));
    assert_eq!(
        refused(&world, "owner", NOW + 44, assign("alice", "auditor")).kind,
        ErrorKind::ConflictingRoles
    );
}

/// Verifies: ADM-1643
///
/// One scoped assignment is taken away alone: the same role scoped to another object type, and the
/// role held directly, stay; an assignment that is not there is `InvalidIntent`, a role that does
/// not exist is `UnknownRole`, and the act is a manager of members's.
#[test]
fn one_scoped_assignment_is_taken_away_alone() {
    let on_people = Intent::AssignScopedRole {
        principal: principal_id("alpha:stranger"),
        role: role("reviewer"),
        object: object("person"),
        scope: eq(var(Scope::Record, "name"), lit("ann")),
    };
    let revoke = |on: &str| Intent::RevokeScopedRole {
        principal: principal_id("alpha:stranger"),
        role: role("reviewer"),
        object: object(on),
    };
    let world = must(
        &base(),
        "owner",
        NOW + 50,
        scoped("stranger", "reviewer", slug_is("north")),
    );
    let world = must(&world, "owner", NOW + 51, on_people);
    let list = named_read("notes_by_rank");
    assert_eq!(row_ids(&world, "stranger", &list), vec![1]);

    // Someone who may not manage members cannot take it away.
    assert_eq!(
        refused(&world, "writer1", NOW + 52, revoke("note")).kind,
        ErrorKind::Unauthorized
    );
    let world = must(&world, "owner", NOW + 53, revoke("note"));
    assert!(row_ids(&world, "stranger", &list).is_empty());
    let held = tenant(&world)
        .schema
        .scoped_roles
        .clone()
        .unwrap_or_default();
    assert_eq!(held.len(), 1, "the assignment on the people stays");
    assert_eq!(held[0].object, object("person"));
    assert_eq!(
        refused(&world, "owner", NOW + 54, revoke("note")).kind,
        ErrorKind::InvalidIntent
    );
    let nosuch = Intent::RevokeScopedRole {
        principal: principal_id("alpha:stranger"),
        role: role("nosuch"),
        object: object("note"),
    };
    assert_eq!(
        refused(&world, "owner", NOW + 55, nosuch).kind,
        ErrorKind::UnknownRole
    );
    let world = must(&world, "owner", NOW + 56, revoke("person"));
    assert!(tenant(&world).schema.scoped_roles.is_none());
}

/// Verifies: ADM-1611
///
/// A role that a hold rule names is not given through a scope, which no hold can freeze: the
/// scoped assignment would go around the approval the rule asks for.
#[test]
fn a_scoped_assignment_is_not_a_way_around_a_hold_rule() {
    let world = change(
        &base(),
        "owner",
        NOW + 60,
        vec![SchemaChange::DefineHoldRule(HoldRule {
            id: HoldRuleId::new("hold_reviewer").unwrap(),
            label: "Reviewers are approved".into(),
            target: HoldTarget::Role {
                role: role("reviewer"),
            },
            when: None,
            approver: CapabilityName("change-approve".into()),
        })],
    )
    .expect("the hold rule is defined");
    assert_eq!(
        refused(
            &world,
            "owner",
            NOW + 61,
            scoped("stranger", "reviewer", slug_is("north"))
        )
        .kind,
        ErrorKind::InvalidIntent
    );
    must(
        &world,
        "owner",
        NOW + 62,
        scoped("stranger", "regional_edit", slug_is("north")),
    );
}

/// Verifies: ADM-1644
///
/// A person who holds a role only through a scope is admitted to what the role gives, as the
/// delivery of events and the offers of an interface ask (`may_attempt`), on the object type the
/// scope names and on no other, and not for an action the role does not give.
#[test]
fn a_role_held_through_a_scope_may_be_attempted_on_its_object_type_only() {
    let world = must(
        &base(),
        "owner",
        NOW + 70,
        scoped("stranger", "regional_edit", slug_is("north")),
    );
    let tenant = tenant(&world);
    let attempt = |who: &str, action: Action, on: &str| {
        strata_kernel::rules::may_attempt(
            tenant,
            &user(&format!("alpha:{who}")),
            action,
            &Resource::Object(object(on)),
        )
    };
    assert!(attempt("stranger", Action::Update, "note"));
    assert!(!attempt("stranger", Action::Delete, "note"));
    assert!(!attempt("stranger", Action::Update, "person"));
    assert!(!attempt("nobody", Action::Update, "note"));
}
