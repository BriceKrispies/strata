//! Concrete behaviours of the transition function, one rule per test.

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::testkit::*;
use strata_kernel::*;

fn step(
    world: &World,
    tenant: &str,
    who: &str,
    now: i64,
    intent: Intent,
) -> Result<(World, Transition), DomainError> {
    world.step(&intent, &ctx(tenant, user(&format!("{tenant}:{who}")), now))
}

fn must(world: &World, tenant: &str, who: &str, now: i64, intent: Intent) -> World {
    step(world, tenant, who, now, intent).expect("accepted").0
}

fn patch(id: u64, fields: &[(&str, Value)]) -> Intent {
    Intent::UpdateRecord {
        object: object("note"),
        id: RecordId(id),
        fields: fields.iter().map(|(k, v)| (field(k), v.clone())).collect(),
        expected_version: None,
    }
}

fn note(world: &World, tenant: &str, id: u64) -> std::sync::Arc<Record> {
    world.tenants[&tenant_id(tenant)]
        .record(&object("note"), RecordId(id))
        .expect("note exists")
        .clone()
}

/// Verifies: KRN-007
///
/// Every kind of rejection, not one example: each refused step must list at least one check
/// that failed. A rejection whose evidence holds only passed checks is a refusal that does not
/// say why.
#[test]
fn every_rejection_lists_at_least_one_failed_check() {
    let world = must(
        &notes_world(),
        "alpha",
        "writer1",
        5_000,
        new_note("First", "first"),
    );
    let rejections: Vec<(&str, &str, Intent)> = vec![
        (
            "constraint",
            "writer1",
            new_note("A title that is far too long to fit", "long"),
        ),
        (
            "missing required",
            "writer1",
            Intent::CreateRecord {
                object: object("note"),
                fields: BTreeMap::from([(field("slug"), text("x"))]),
            },
        ),
        (
            "not authorised",
            "writer2",
            patch(1, &[("title", text("Mine now"))]),
        ),
        (
            "workflow",
            "writer1",
            patch(1, &[("status", text("published"))]),
        ),
        (
            "unknown record",
            "writer1",
            patch(42, &[("title", text("x"))]),
        ),
        (
            "unknown role",
            "owner",
            Intent::RevokeRole {
                principal: principal_id("alpha:editor"),
                role: role("nobody"),
            },
        ),
        (
            "stale version",
            "writer1",
            Intent::UpdateRecord {
                object: object("note"),
                id: RecordId(1),
                fields: BTreeMap::from([(field("body"), text("two"))]),
                expected_version: Some(7),
            },
        ),
    ];
    for (name, who, intent) in rejections {
        let Err(error) = step(&world, "alpha", who, 5_001, intent) else {
            panic!("{name} was accepted");
        };
        assert!(
            error.evidence.iter().any(|c| !c.passed),
            "{name} was refused as {:?} with no failed check listed: {:?}",
            error.kind,
            error.evidence
        );
    }

    // Refusals that come before any rule of the tenant is consulted, and the ones that come
    // from a stale or partial snapshot, are refusals too.
    let create = Intent::CreateTenant {
        name: "Gamma".into(),
        owner: principal_id("gamma:owner"),
    };
    let blank = Intent::CreateTenant {
        name: " ".into(),
        owner: principal_id("gamma:owner"),
    };
    let tenant = world.tenants[&tenant_id("alpha")].clone();
    let mut thin = TenantState::clone(&tenant);
    thin.records.clear();
    let thin = State {
        tenant_id: tenant_id("alpha"),
        tenant: Some(std::sync::Arc::new(thin)),
        coverage: Coverage::Partial(BTreeSet::new()),
    };
    let owner_of_alpha = ctx("alpha", user("alpha:owner"), 5_001);
    let early: Vec<(&str, DomainError)> = vec![
        (
            "tenant exists",
            world
                .step(&create, &ctx("alpha", Principal::System, 5_001))
                .unwrap_err(),
        ),
        (
            "no such tenant",
            world
                .step(
                    &new_note("T", "t"),
                    &ctx("gamma", user("alpha:owner"), 5_001),
                )
                .unwrap_err(),
        ),
        (
            "tenant created by someone but the system",
            world
                .step(&create, &ctx("gamma", user("alpha:owner"), 5_001))
                .unwrap_err(),
        ),
        (
            "tenant without a name",
            world
                .step(&blank, &ctx("gamma", Principal::System, 5_001))
                .unwrap_err(),
        ),
        (
            "snapshot of another tenant",
            transition(
                &world.state(&tenant_id("alpha")),
                &new_note("T", "t"),
                &ctx("beta", user("beta:owner"), 5_001),
            )
            .unwrap_err(),
        ),
        (
            "partial snapshot that lacks state",
            transition(
                &thin,
                &new_note("Second", "first"),
                &ctx("alpha", user("alpha:writer1"), 5_001),
            )
            .unwrap_err(),
        ),
        (
            "unknown object type",
            world
                .step(
                    &Intent::CreateRecord {
                        object: object("nothing"),
                        fields: BTreeMap::new(),
                    },
                    &owner_of_alpha,
                )
                .unwrap_err(),
        ),
        (
            "the last owner",
            world
                .step(
                    &Intent::RevokeRole {
                        principal: principal_id("alpha:owner"),
                        role: role("owner"),
                    },
                    &owner_of_alpha,
                )
                .unwrap_err(),
        ),
        (
            "schema change that is not consistent",
            world
                .step(
                    &Intent::ChangeSchema {
                        changes: vec![SchemaChange::RemoveField {
                            object: object("note"),
                            field: field("nothing"),
                        }],
                        expected_version: None,
                        existing: ExistingData::Reject,
                    },
                    &owner_of_alpha,
                )
                .unwrap_err(),
        ),
        (
            "stale schema version",
            world
                .step(
                    &Intent::ChangeSchema {
                        changes: vec![SchemaChange::RemoveField {
                            object: object("note"),
                            field: field("body"),
                        }],
                        expected_version: Some(SchemaVersion(99)),
                        existing: ExistingData::Reject,
                    },
                    &owner_of_alpha,
                )
                .unwrap_err(),
        ),
    ];
    for (name, error) in early {
        assert!(
            error.evidence.iter().any(|c| !c.passed),
            "{name} was refused as {:?} with no failed check listed: {:?}",
            error.kind,
            error.evidence
        );
    }
}

/// Verifies: KRN-008
///
/// From each state of the declared workflow, every status the workflow does not declare as a
/// transition from that state is refused, whoever asks; the declared move is accepted for an
/// editor. The rows cover both states and the undeclared targets, not one example.
#[test]
fn undeclared_status_changes_are_refused_from_every_state() {
    let world = must(
        &notes_world(),
        "alpha",
        "writer1",
        5_000,
        new_note("First", "first"),
    );
    // draft: the declared move is to published; archived and an unknown word are not declared.
    for target in ["archived", "nonsense"] {
        let refused = step(
            &world,
            "alpha",
            "editor",
            5_001,
            patch(1, &[("status", text(target))]),
        )
        .unwrap_err();
        assert_eq!(
            refused.kind,
            ErrorKind::WorkflowViolation,
            "draft -> {target}"
        );
    }
    let published = must(
        &world,
        "alpha",
        "editor",
        5_002,
        patch(1, &[("status", text("published"))]),
    );
    // published: the declared move is back to draft; archived and an unknown word are not declared.
    for target in ["archived", "nonsense"] {
        let refused = step(
            &published,
            "alpha",
            "editor",
            5_003,
            patch(1, &[("status", text(target))]),
        )
        .unwrap_err();
        assert_eq!(
            refused.kind,
            ErrorKind::WorkflowViolation,
            "published -> {target}"
        );
    }
    let back = must(
        &published,
        "alpha",
        "editor",
        5_004,
        patch(1, &[("status", text("draft"))]),
    );
    assert_eq!(
        note(&back, "alpha", 1).get(&field("status")),
        &text("draft")
    );

    // A declared transition is permitted only where its guard holds: the writer who owns the
    // note is refused the move the editor was allowed.
    let guarded = step(
        &world,
        "alpha",
        "writer1",
        5_005,
        patch(1, &[("status", text("published"))]),
    )
    .unwrap_err();
    assert_eq!(guarded.kind, ErrorKind::WorkflowViolation);
    assert!(guarded
        .evidence
        .iter()
        .any(|c| !c.passed && c.rule == "workflow:note_status"));
    // A record is created only in an initial state of the workflow.
    for start in ["published", "archived", "nonsense"] {
        let refused = step(
            &world,
            "alpha",
            "editor",
            5_006,
            Intent::CreateRecord {
                object: object("note"),
                fields: BTreeMap::from([
                    (field("title"), text("T")),
                    (field("slug"), text("t")),
                    (field("status"), text(start)),
                ]),
            },
        )
        .unwrap_err();
        assert_eq!(
            refused.kind,
            ErrorKind::WorkflowViolation,
            "created as {start}"
        );
    }
    // A change that leaves the workflow field alone is not a workflow matter, in any state.
    must(
        &world,
        "alpha",
        "writer1",
        5_007,
        patch(1, &[("body", text("one"))]),
    );
    must(
        &published,
        "alpha",
        "writer1",
        5_008,
        patch(1, &[("body", text("two"))]),
    );
}

/// The notes world without the query `notes_by_rank`, the one dependent that reads `note.rank`.
/// The migration tests below are about records: with that query present, changing the type of
/// `rank` is a breaking change with a reader and is refused before any record is examined
/// (MIG-592), which `a_type_change_a_query_reads_is_refused_naming_the_query` checks.
fn unread_notes_world() -> World {
    must(
        &notes_world(),
        "alpha",
        "owner",
        4_999,
        Intent::ChangeSchema {
            changes: vec![SchemaChange::RemoveQuery(
                QueryId::new("notes_by_rank").unwrap(),
            )],
            expected_version: None,
            existing: ExistingData::Reject,
        },
    )
}

/// Note 1 has a short title and a rank, note 2 has a long title and no rank.
fn mixed_notes() -> World {
    let world = must(
        &unread_notes_world(),
        "alpha",
        "writer1",
        5_000,
        new_note("Short", "first"),
    );
    let world = must(
        &world,
        "alpha",
        "writer1",
        5_001,
        patch(1, &[("rank", Value::Int(1))]),
    );
    must(
        &world,
        "alpha",
        "writer1",
        5_002,
        new_note("A longer title", "second"),
    )
}

/// One change of each of three kinds that existing records can break, the rule each breaks and
/// the records of `mixed_notes` that break it: a new required field (note 2 has no rank), a
/// tighter constraint (note 2 has a long title) and a changed field type (note 1's rank is a
/// number).
fn breaking_changes() -> Vec<(&'static str, SchemaChange, &'static str, Vec<u64>)> {
    let shorter = SchemaChange::DefineConstraint {
        object: object("note"),
        constraint: ConstraintDef {
            id: ConstraintId::new("title_length").unwrap(),
            expr: Expr::Cmp(
                CmpOp::Le,
                Box::new(Expr::Len(Box::new(var(Scope::New, "title")))),
                Box::new(Expr::Lit(Value::Int(5))),
            ),
            message: "short".into(),
        },
    };
    vec![
        (
            "new required field",
            SchemaChange::DefineField {
                object: object("note"),
                field: field_def("rank", FieldType::Int, true),
            },
            "required:note.rank",
            vec![2],
        ),
        (
            "tighter constraint",
            shorter,
            "constraint:note.title_length",
            vec![2],
        ),
        (
            "changed field type",
            SchemaChange::DefineField {
                object: object("note"),
                field: field_def("rank", FieldType::Str, false),
            },
            "type:note.rank",
            vec![1],
        ),
    ]
}

fn change_in(mode: ExistingData, change: SchemaChange) -> Intent {
    Intent::ChangeSchema {
        changes: vec![change],
        expected_version: None,
        existing: mode,
    }
}

/// The checks `existing:<object>/<id>:<rule>` of an outcome that failed.
fn listed_existing(evidence: &[Check]) -> BTreeSet<String> {
    evidence
        .iter()
        .filter(|c| !c.passed && c.rule.starts_with("existing:"))
        .map(|c| c.rule.clone())
        .collect()
}

/// Verifies: KRN-009
///
/// Strict mode refuses each kind of change while a record would not conform, listing exactly
/// the records that do not conform, and accepts the same change where every record conforms.
#[test]
fn a_strict_migration_is_refused_for_every_kind_of_change_that_a_record_breaks() {
    let world = mixed_notes();
    for (name, change, rule, broken) in breaking_changes() {
        let intent = change_in(ExistingData::Reject, change);
        let error = step(&world, "alpha", "owner", 5_003, intent.clone()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::MigrationRejected, "{name}");
        let expected: BTreeSet<String> = broken
            .iter()
            .map(|id| format!("existing:note/{id}:{rule}"))
            .collect();
        assert_eq!(listed_existing(&error.evidence), expected, "{name}");
        // Where every record conforms, the same change is accepted.
        must(&unread_notes_world(), "alpha", "owner", 5_003, intent);
    }
}

/// Verifies: MIG-592
///
/// Changing the type of a field that a query reads is a breaking change with a reader: it is
/// refused naming the query, under either migration mode, before any record is examined.
#[test]
fn a_type_change_a_query_reads_is_refused_naming_the_query() {
    let retype = SchemaChange::DefineField {
        object: object("note"),
        field: field_def("rank", FieldType::Str, false),
    };
    for mode in [ExistingData::Reject, ExistingData::Grandfather] {
        let error = step(
            &notes_world(),
            "alpha",
            "owner",
            5_003,
            change_in(mode, retype.clone()),
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidSchema);
        assert!(
            error.message.contains("notes_by_rank"),
            "the refusal names the query: {}",
            error.message
        );
    }
}

/// Verifies: KRN-010
///
/// Grandfather mode accepts each kind of change that records break, listing exactly the records
/// that do not conform in the change's own evidence and leaving every record as it was.
#[test]
fn a_grandfathered_migration_is_accepted_for_every_kind_of_change_that_a_record_breaks() {
    let world = mixed_notes();
    for (name, change, rule, broken) in breaking_changes() {
        let intent = change_in(ExistingData::Grandfather, change);
        let (changed, outcome) = step(&world, "alpha", "owner", 5_003, intent)
            .unwrap_or_else(|e| panic!("{name} was refused: {}", e.describe()));
        let expected: BTreeSet<String> = broken
            .iter()
            .map(|id| format!("existing:note/{id}:{rule}"))
            .collect();
        assert_eq!(listed_existing(&outcome.evidence), expected, "{name}");
        let EventBody::SchemaChanged { grandfathered, .. } = &outcome.events[0].body else {
            panic!("{name}: no schema event");
        };
        assert_eq!(*grandfathered, broken.len() as u64, "{name}");
        assert_eq!(
            changed.tenants[&tenant_id("alpha")].records,
            world.tenants[&tenant_id("alpha")].records,
            "{name}"
        );
    }
}

// Roles as sets of permissions, teams and invitations (ADM-150 to ADM-153).

fn object_permission(name: &str, verbs: &[ObjectAction]) -> Permission {
    Permission::Object {
        object: object(name),
        actions: verbs.iter().copied().collect(),
    }
}

fn capability(name: &str) -> Permission {
    Permission::Capability(name.to_string())
}

/// The notes tenant with four more roles. `writer` creates notes, `reviewer` reads them,
/// `publisher` holds one capability, and `moderator` manages members and holds a capability and
/// the right to read notes. The principal `alpha:stranger` is a moderator.
fn access_world() -> World {
    let role_def = |id: &str, label: &str| {
        SchemaChange::DefineRole(RoleDef {
            id: role(id),
            label: label.into(),
        })
    };
    let permissions =
        |id: &str, permissions: Vec<Permission>| SchemaChange::DefineRolePermissions {
            role: role(id),
            permissions,
        };
    let world = must(
        &notes_world(),
        "alpha",
        "owner",
        6_000,
        Intent::ChangeSchema {
            changes: vec![
                role_def("moderator", "Moderator"),
                role_def("publisher", "Publisher"),
                role_def("reviewer", "Reviewer"),
                permissions(
                    "writer",
                    vec![object_permission("note", &[ObjectAction::Create])],
                ),
                permissions(
                    "reviewer",
                    vec![object_permission("note", &[ObjectAction::Read])],
                ),
                permissions("publisher", vec![capability("content.publish")]),
                permissions(
                    "moderator",
                    vec![
                        capability("people.manage"),
                        object_permission("note", &[ObjectAction::Read]),
                    ],
                ),
                SchemaChange::DefinePolicy(PolicyDef {
                    id: PolicyId::new("moderators_manage_members").unwrap(),
                    role: role("moderator"),
                    resource: Resource::Members,
                    actions: BTreeSet::from([Action::Manage]),
                    condition: None,
                }),
            ],
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    must(
        &world,
        "alpha",
        "owner",
        6_001,
        Intent::AssignRole {
            principal: principal_id("alpha:stranger"),
            role: role("moderator"),
        },
    )
}

fn roles_held_by(world: &World, who: &str) -> BTreeSet<RoleId> {
    strata_kernel::rules::roles_of(
        &world.tenants[&tenant_id("alpha")],
        &user(&format!("alpha:{who}")),
    )
}

fn refused(result: Result<(World, Transition), DomainError>) -> DomainError {
    match result {
        Ok(_) => panic!("the intent was accepted"),
        Err(error) => error,
    }
}

fn team(id: &str) -> TeamId {
    TeamId::new(id).unwrap()
}

fn define_team(id: &str) -> Intent {
    Intent::DefineTeam {
        team: team(id),
        label: id.to_string(),
    }
}

fn team_gets(id: &str, role_name: &str) -> Intent {
    Intent::AssignTeamRole {
        team: team(id),
        role: role(role_name),
    }
}

fn joins(id: &str, who: &str) -> Intent {
    Intent::AddTeamMember {
        team: team(id),
        principal: principal_id(&format!("alpha:{who}")),
    }
}

/// A step as a person whose account has the verified address `address` (or none).
fn step_verified(
    world: &World,
    who: &str,
    now: i64,
    address: Option<&str>,
    intent: Intent,
) -> Result<(World, Transition), DomainError> {
    let mut context = ctx("alpha", user(&format!("alpha:{who}")), now);
    context.verified_address = address.map(str::to_string);
    world.step(&intent, &context)
}

/// Verifies: ADM-152
///
/// A role may be conferred only by a principal that holds every permission and capability it
/// grants: one it lacks among several refuses the assignment, each kind of permission on its
/// own, and what the principal holds through its roles and its teams' roles together counts.
#[test]
fn assigning_a_role_is_refused_unless_the_assigner_holds_all_of_its_permissions() {
    let role_def = |id: &str| {
        SchemaChange::DefineRole(RoleDef {
            id: role(id),
            label: id.into(),
        })
    };
    let permissions =
        |id: &str, permissions: Vec<Permission>| SchemaChange::DefineRolePermissions {
            role: role(id),
            permissions,
        };
    // `alpha:stranger` is a moderator: it manages members, holds people.manage and reads notes.
    let world = must(
        &access_world(),
        "alpha",
        "owner",
        8_000,
        Intent::ChangeSchema {
            changes: vec![
                role_def("reader_editor"),
                role_def("reader_publisher"),
                role_def("manager_reader"),
                role_def("note_rw"),
                permissions(
                    "reader_editor",
                    vec![object_permission(
                        "note",
                        &[ObjectAction::Read, ObjectAction::Update],
                    )],
                ),
                permissions(
                    "reader_publisher",
                    vec![
                        object_permission("note", &[ObjectAction::Read]),
                        capability("content.publish"),
                    ],
                ),
                permissions(
                    "manager_reader",
                    vec![
                        object_permission("note", &[ObjectAction::Read]),
                        capability("people.manage"),
                    ],
                ),
                permissions(
                    "note_rw",
                    vec![object_permission(
                        "note",
                        &[ObjectAction::Read, ObjectAction::Create],
                    )],
                ),
            ],
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    let assign = |role_name: &str| Intent::AssignRole {
        principal: principal_id("alpha:writer2"),
        role: role(role_name),
    };
    let confer_failed = |error: &DomainError, role_name: &str, lacking: &str| {
        assert_eq!(error.kind, ErrorKind::Unauthorized);
        let rule = format!("permission:confer:{role_name}");
        let check = error
            .evidence
            .iter()
            .find(|c| c.rule == rule && !c.passed)
            .unwrap_or_else(|| panic!("no failed {rule} in {:?}", error.evidence));
        assert!(
            check.detail.contains(lacking),
            "{} does not name {lacking}",
            check.detail
        );
        assert!(
            check.detail.contains("lacks 1 of the 2"),
            "{}",
            check.detail
        );
    };

    // An object permission among two is missing: the one it holds does not make up for it.
    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        8_001,
        assign("reader_editor"),
    ));
    confer_failed(&error, "reader_editor", "note:update");
    assert!(!roles_held_by(&world, "writer2").contains(&role("reader_editor")));

    // A capability among two is missing, though the object permission with it is held.
    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        8_002,
        assign("reader_publisher"),
    ));
    confer_failed(&error, "reader_publisher", "capability content.publish");

    // The same holds to a team, in a team's membership and in an invitation.
    let world = must(&world, "alpha", "stranger", 8_003, define_team("desk"));
    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        8_004,
        team_gets("desk", "reader_publisher"),
    ));
    confer_failed(&error, "reader_publisher", "capability content.publish");
    let world = must(&world, "alpha", "owner", 8_005, define_team("crew"));
    let world = must(
        &world,
        "alpha",
        "owner",
        8_006,
        team_gets("crew", "reader_editor"),
    );
    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        8_007,
        joins("crew", "writer2"),
    ));
    confer_failed(&error, "reader_editor", "note:update");
    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        8_008,
        Intent::CreateInvitation {
            invitation: InvitationId::new("inv-partial").unwrap(),
            address: "ines@example.org".into(),
            roles: BTreeSet::from([role("reader_editor")]),
        },
    ));
    confer_failed(&error, "reader_editor", "note:update");

    // Holding every permission is enough: the object permission and the capability are held.
    let world = must(&world, "alpha", "stranger", 8_009, assign("manager_reader"));
    assert!(roles_held_by(&world, "writer2").contains(&role("manager_reader")));

    // What the assigner holds through several roles and teams counts together: it reads notes
    // as a moderator and creates them through a team that holds the writer role.
    let error = refused(step(&world, "alpha", "stranger", 8_010, assign("note_rw")));
    confer_failed(&error, "note_rw", "note:create");
    let world = must(&world, "alpha", "owner", 8_011, team_gets("crew", "writer"));
    let world = must(&world, "alpha", "owner", 8_012, joins("crew", "stranger"));
    let world = must(&world, "alpha", "stranger", 8_013, assign("note_rw"));
    assert!(roles_held_by(&world, "writer2").contains(&role("note_rw")));

    // The owner holds every permission and confers any role.
    must(&world, "alpha", "owner", 8_014, assign("reader_publisher"));
}

// Conferral through policies, revocation, owners through teams, invitation lifetime, and
// refusals that do not reveal what exists (ADM-1080 to ADM-1085, KRN-1080).

/// `access_world` with two more roles that hold no permission of their own but are empowered by
/// a policy: `admin2` manages the schema, `reader2` reads every object type.
fn policy_world() -> World {
    must(
        &access_world(),
        "alpha",
        "owner",
        7_500,
        Intent::ChangeSchema {
            changes: vec![
                SchemaChange::DefineRole(RoleDef {
                    id: role("admin2"),
                    label: "Admin two".into(),
                }),
                SchemaChange::DefineRole(RoleDef {
                    id: role("reader2"),
                    label: "Reader two".into(),
                }),
                SchemaChange::DefinePolicy(PolicyDef {
                    id: PolicyId::new("admin2_manage_schema").unwrap(),
                    role: role("admin2"),
                    resource: Resource::Schema,
                    actions: BTreeSet::from([Action::Manage]),
                    condition: None,
                }),
                SchemaChange::DefinePolicy(PolicyDef {
                    id: PolicyId::new("reader2_read_all").unwrap(),
                    role: role("reader2"),
                    resource: Resource::AnyObject,
                    actions: BTreeSet::from([Action::Read]),
                    condition: None,
                }),
            ],
            expected_version: None,
            existing: ExistingData::Reject,
        },
    )
}

fn assert_confer_refused(error: &DomainError, role_name: &str) {
    assert_eq!(error.kind, ErrorKind::Unauthorized);
    let rule = format!("permission:confer:{role_name}");
    assert!(
        error.evidence.iter().any(|c| c.rule == rule && !c.passed),
        "no failed {rule} in {:?}",
        error.evidence
    );
}

/// Verifies: ADM-1085
///
/// A role that holds no permission but is empowered by a policy is conferred only by a principal
/// that holds that power: not by assigning it, not by adding someone to a team that holds it,
/// not by giving it to a team, not in an invitation. The owner confers it.
#[test]
fn a_role_empowered_by_a_policy_is_not_conferred_without_that_power() {
    let world = policy_world();
    // `alpha:stranger` manages members; it does not manage the schema.
    let world = must(&world, "alpha", "owner", 7_501, define_team("ops"));
    let world = must(&world, "alpha", "owner", 7_502, team_gets("ops", "admin2"));
    let world = must(&world, "alpha", "stranger", 7_503, define_team("desk"));

    for role_name in ["admin2", "reader2"] {
        let error = refused(step(
            &world,
            "alpha",
            "stranger",
            7_504,
            Intent::AssignRole {
                principal: principal_id("alpha:writer2"),
                role: role(role_name),
            },
        ));
        assert_confer_refused(&error, role_name);
        assert!(!roles_held_by(&world, "writer2").contains(&role(role_name)));
    }

    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        7_505,
        joins("ops", "writer2"),
    ));
    assert_confer_refused(&error, "admin2");

    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        7_506,
        team_gets("desk", "admin2"),
    ));
    assert_confer_refused(&error, "admin2");

    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        7_507,
        Intent::CreateInvitation {
            invitation: InvitationId::new("inv-policy").unwrap(),
            address: "ines@example.org".into(),
            roles: BTreeSet::from([role("admin2")]),
        },
    ));
    assert_confer_refused(&error, "admin2");

    // The owner holds every power and confers the role.
    let world = must(
        &world,
        "alpha",
        "owner",
        7_508,
        Intent::AssignRole {
            principal: principal_id("alpha:writer2"),
            role: role("admin2"),
        },
    );
    assert!(roles_held_by(&world, "writer2").contains(&role("admin2")));
}

/// Verifies: ADM-1080
///
/// Taking a role away asks what giving it asks: a member manager who could not confer a role
/// cannot revoke it from a person, revoke it from a team or take a person out of a team that
/// holds it. The same manager revokes a role it could confer.
#[test]
fn revoking_a_role_needs_the_authority_to_confer_it() {
    let world = access_world();
    let world = must(
        &world,
        "alpha",
        "owner",
        7_600,
        Intent::AssignRole {
            principal: principal_id("alpha:writer2"),
            role: role("publisher"),
        },
    );
    let world = must(
        &world,
        "alpha",
        "owner",
        7_601,
        Intent::AssignRole {
            principal: principal_id("alpha:writer2"),
            role: role("reviewer"),
        },
    );
    let world = must(&world, "alpha", "owner", 7_602, define_team("press"));
    let world = must(
        &world,
        "alpha",
        "owner",
        7_603,
        team_gets("press", "publisher"),
    );
    let world = must(&world, "alpha", "owner", 7_604, joins("press", "writer1"));

    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        7_605,
        Intent::RevokeRole {
            principal: principal_id("alpha:writer2"),
            role: role("publisher"),
        },
    ));
    assert_confer_refused(&error, "publisher");
    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        7_606,
        Intent::RevokeTeamRole {
            team: team("press"),
            role: role("publisher"),
        },
    ));
    assert_confer_refused(&error, "publisher");
    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        7_607,
        Intent::RemoveTeamMember {
            team: team("press"),
            principal: principal_id("alpha:writer1"),
        },
    ));
    assert_confer_refused(&error, "publisher");
    assert!(roles_held_by(&world, "writer2").contains(&role("publisher")));
    assert!(roles_held_by(&world, "writer1").contains(&role("publisher")));

    // A role the manager could confer, it may revoke.
    let world = must(
        &world,
        "alpha",
        "stranger",
        7_608,
        Intent::RevokeRole {
            principal: principal_id("alpha:writer2"),
            role: role("reviewer"),
        },
    );
    assert!(!roles_held_by(&world, "writer2").contains(&role("reviewer")));
    // The owner revokes the rest.
    must(
        &world,
        "alpha",
        "owner",
        7_609,
        Intent::RevokeTeamRole {
            team: team("press"),
            role: role("publisher"),
        },
    );
}

/// Verifies: ADM-1081
///
/// Owners are counted in effect, through teams as well as directly: no revocation, of the role
/// or of a team's role or of a team member, leaves the tenant with none.
#[test]
fn no_sequence_of_revocations_leaves_a_tenant_without_an_owner() {
    let world = notes_world();
    let world = must(&world, "alpha", "owner", 7_700, define_team("board"));
    let world = must(&world, "alpha", "owner", 7_701, team_gets("board", "owner"));
    let world = must(&world, "alpha", "owner", 7_702, joins("board", "writer1"));
    // The direct owner steps down: the board still holds an owner.
    let world = must(
        &world,
        "alpha",
        "owner",
        7_703,
        Intent::RevokeRole {
            principal: principal_id("alpha:owner"),
            role: role("owner"),
        },
    );
    assert!(roles_held_by(&world, "writer1").contains(&role("owner")));

    // The one owner left, through the team, cannot leave the team or take the role from it.
    let error = refused(step(
        &world,
        "alpha",
        "writer1",
        7_704,
        Intent::RemoveTeamMember {
            team: team("board"),
            principal: principal_id("alpha:writer1"),
        },
    ));
    assert_eq!(error.kind, ErrorKind::InvalidIntent);
    let error = refused(step(
        &world,
        "alpha",
        "writer1",
        7_705,
        Intent::RevokeTeamRole {
            team: team("board"),
            role: role("owner"),
        },
    ));
    assert_eq!(error.kind, ErrorKind::InvalidIntent);

    // With another owner in place, leaving is allowed.
    let world = must(
        &world,
        "alpha",
        "writer1",
        7_706,
        Intent::AssignRole {
            principal: principal_id("alpha:writer2"),
            role: role("owner"),
        },
    );
    let world = must(
        &world,
        "alpha",
        "writer1",
        7_707,
        Intent::RemoveTeamMember {
            team: team("board"),
            principal: principal_id("alpha:writer1"),
        },
    );
    assert!(!roles_held_by(&world, "writer1").contains(&role("owner")));
    assert!(roles_held_by(&world, "writer2").contains(&role("owner")));
}

fn invite(id: &str, address: &str, role_name: &str) -> Intent {
    Intent::CreateInvitation {
        invitation: InvitationId::new(id).unwrap(),
        address: address.into(),
        roles: BTreeSet::from([role(role_name)]),
    }
}

fn redeem_invitation(id: &str) -> Intent {
    Intent::RedeemInvitation {
        invitation: InvitationId::new(id).unwrap(),
    }
}

/// Verifies: ADM-1082
///
/// An invitation is redeemable until its expiry and not after.
#[test]
fn an_invitation_expires() {
    let world = must(
        &access_world(),
        "alpha",
        "owner",
        7_800,
        invite("inv-late", "ines@example.org", "writer"),
    );
    let fourteen_days = 14 * 24 * 60 * 60 * 1000;
    let error = refused(step_verified(
        &world,
        "ines",
        7_800 + fourteen_days,
        Some("ines@example.org"),
        redeem_invitation("inv-late"),
    ));
    assert_eq!(error.kind, ErrorKind::InvalidIntent);
    assert!(error
        .evidence
        .iter()
        .any(|c| c.rule == "invitation:not_expired" && !c.passed));
    assert!(!roles_held_by(&world, "ines").contains(&role("writer")));

    let (world, _) = step_verified(
        &world,
        "ines",
        7_800 + fourteen_days - 1,
        Some("ines@example.org"),
        redeem_invitation("inv-late"),
    )
    .unwrap();
    assert!(roles_held_by(&world, "ines").contains(&role("writer")));
}

/// Verifies: ADM-1083
///
/// An invitation is revoked by a principal who could confer its roles, once, and a revoked
/// invitation is never redeemable.
#[test]
fn an_invitation_can_be_revoked() {
    let world = must(
        &access_world(),
        "alpha",
        "owner",
        7_900,
        invite("inv-gone", "ines@example.org", "writer"),
    );
    let revoke = || Intent::RevokeInvitation {
        invitation: InvitationId::new("inv-gone").unwrap(),
    };
    // A member manager who could not confer `writer` cannot revoke it either.
    let error = refused(step(&world, "alpha", "stranger", 7_901, revoke()));
    assert_confer_refused(&error, "writer");
    // Someone who does not manage members is refused by policy.
    let error = refused(step(&world, "alpha", "writer1", 7_902, revoke()));
    assert_eq!(error.kind, ErrorKind::Unauthorized);

    let world = must(&world, "alpha", "owner", 7_903, revoke());
    let error = refused(step(&world, "alpha", "owner", 7_904, revoke()));
    assert_eq!(error.kind, ErrorKind::InvalidIntent);
    let error = refused(step_verified(
        &world,
        "ines",
        7_905,
        Some("ines@example.org"),
        redeem_invitation("inv-gone"),
    ));
    assert_eq!(error.kind, ErrorKind::InvalidIntent);
    assert!(error
        .evidence
        .iter()
        .any(|c| c.rule == "invitation:not_revoked" && !c.passed));
    assert!(!roles_held_by(&world, "ines").contains(&role("writer")));
}

/// Verifies: ADM-1084
///
/// Redeeming asks again whether the issuer may still issue the invitation: one issued by a
/// member manager who has since lost the role grants nothing.
#[test]
fn redeeming_asks_again_for_the_authority_of_the_issuer() {
    let world = must(
        &access_world(),
        "alpha",
        "stranger",
        8_000,
        invite("inv-moderated", "ines@example.org", "reviewer"),
    );
    // While the issuer still manages members the invitation is good (checked on a copy).
    step_verified(
        &world,
        "ines",
        8_001,
        Some("ines@example.org"),
        redeem_invitation("inv-moderated"),
    )
    .unwrap();
    let world = must(
        &world,
        "alpha",
        "owner",
        8_002,
        Intent::RevokeRole {
            principal: principal_id("alpha:stranger"),
            role: role("moderator"),
        },
    );
    let error = refused(step_verified(
        &world,
        "ines",
        8_003,
        Some("ines@example.org"),
        redeem_invitation("inv-moderated"),
    ));
    assert_eq!(error.kind, ErrorKind::Unauthorized);
    assert!(error
        .evidence
        .iter()
        .any(|c| c.rule == "invitation:issuer_authority" && !c.passed));
    assert!(!roles_held_by(&world, "ines").contains(&role("reviewer")));
}

/// Verifies: KRN-1080
///
/// A caller who may not act on a record or an object type is refused the same way whether the
/// record or the type exists or not, so a refusal tells nothing about what exists.
#[test]
fn an_unauthorised_caller_cannot_tell_a_missing_record_from_an_existing_one() {
    let world = must(
        &access_world(),
        "alpha",
        "writer1",
        8_100,
        new_note("First", "first"),
    );
    // `alpha:stranger` reads notes and may do nothing else with them.
    let delete = |object_name: &str, id: u64| Intent::DeleteRecord {
        object: object(object_name),
        id: RecordId(id),
        expected_version: None,
    };
    let update = |object_name: &str, id: u64| Intent::UpdateRecord {
        object: object(object_name),
        id: RecordId(id),
        fields: BTreeMap::from([(field("title"), text("x"))]),
        expected_version: None,
    };
    let create = |object_name: &str| Intent::CreateRecord {
        object: object(object_name),
        fields: BTreeMap::new(),
    };
    let pairs = [
        (update("note", 1), update("note", 999)),
        (delete("note", 1), delete("note", 999)),
        (update("note", 1), update("ghost", 1)),
        (delete("note", 1), delete("ghost", 1)),
        (create("note"), create("ghost")),
    ];
    // What a refusal says, with the name the caller supplied for the missing type put back to the
    // existing one: its message and the checks that failed.
    let said = |error: &DomainError| {
        (
            error.kind,
            error.message.replace("ghost", "note"),
            error
                .evidence
                .iter()
                .filter(|c| !c.passed)
                .map(|c| (c.rule.clone(), c.detail.replace("ghost", "note")))
                .collect::<Vec<_>>(),
        )
    };
    for (n, (present, absent)) in pairs.into_iter().enumerate() {
        let a = refused(step(&world, "alpha", "stranger", 8_101 + n as i64, present));
        let b = refused(step(&world, "alpha", "stranger", 8_101 + n as i64, absent));
        assert_eq!(a.kind, ErrorKind::Unauthorized, "pair {n}");
        assert_eq!(said(&a), said(&b), "pair {n}");
    }
    // One who may act on the type is told what is missing.
    let error = refused(step(&world, "alpha", "owner", 8_200, update("note", 999)));
    assert_eq!(error.kind, ErrorKind::UnknownRecord);
    let error = refused(step(&world, "alpha", "owner", 8_201, update("ghost", 1)));
    assert_eq!(error.kind, ErrorKind::UnknownObject);
}

// Cited by the Lean correspondence (crates/verification/src/obligations.rs).
#[test]
fn a_strict_migration_is_rejected_when_existing_records_would_not_conform() {
    let world = must(
        &notes_world(),
        "alpha",
        "writer1",
        5_000,
        new_note("First", "first"),
    );
    let world = must(
        &world,
        "alpha",
        "writer1",
        5_001,
        new_note("Second", "second"),
    );
    let error = step(
        &world,
        "alpha",
        "owner",
        5_002,
        require_rank(ExistingData::Reject),
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::MigrationRejected);
    for id in [1, 2] {
        let rule = format!("existing:note/{id}:required:note.rank");
        assert!(
            error.evidence.iter().any(|c| c.rule == rule && !c.passed),
            "{rule} is not listed among the refusal's checks"
        );
    }
}

// Cited by the Lean correspondence (crates/verification/src/obligations.rs).
#[test]
fn writers_edit_only_their_own_notes() {
    let world = must(
        &notes_world(),
        "alpha",
        "writer1",
        5_000,
        new_note("First", "first"),
    );
    let other = step(
        &world,
        "alpha",
        "writer2",
        5_001,
        patch(1, &[("title", text("Mine now"))]),
    )
    .unwrap_err();
    assert_eq!(other.kind, ErrorKind::Unauthorized);
    assert!(other
        .evidence
        .iter()
        .any(|c| c.rule == "policy:writers_edit_own" && !c.passed));
    let world = must(
        &world,
        "alpha",
        "writer1",
        5_002,
        patch(1, &[("title", text("Still mine"))]),
    );
    assert_eq!(note(&world, "alpha", 1).version, 2);
}

fn require_rank(mode: ExistingData) -> Intent {
    Intent::ChangeSchema {
        changes: vec![SchemaChange::DefineField {
            object: object("note"),
            field: field_def("rank", FieldType::Int, true),
        }],
        expected_version: None,
        existing: mode,
    }
}
