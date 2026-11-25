//! Scoped role assignments, self-action and separation of duties, and the last owner, on every
//! path that can touch them (D-8403, D-8405, D-8415, D-8426, D-8427).

use std::collections::{BTreeMap, BTreeSet};
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
