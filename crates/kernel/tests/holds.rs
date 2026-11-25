//! Held operations (D-8400, D-8406, ADM-1601 to ADM-1604, ADM-1614): a hold rule files the change
//! instead of applying it, a named approver decides it, and an approval applies it once, as the
//! requester, on the state it read. One rule per test; the backend's half (the route, the audit
//! line of each step) is `crates/backend/tests/holds.rs`.

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::testkit::*;
use strata_kernel::*;

// ---------------------------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------------------------

const APPROVER: &str = "change-approve";
const FILED_AT: i64 = 10_000;
const WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1000;

fn op_id(s: &str) -> TenantOperationId {
    TenantOperationId::new(s).expect("operation id")
}

fn rule_id(s: &str) -> HoldRuleId {
    HoldRuleId::new(s).expect("rule id")
}

fn hold_id(n: u64) -> HeldOperationId {
    HeldOperationId::new(format!("h-{n}")).expect("hold id")
}

fn as_user(who: &str) -> Principal {
    user(&format!("alpha:{who}"))
}

fn step(
    world: &World,
    who: Principal,
    now: i64,
    intent: &Intent,
) -> Result<(World, Transition), DomainError> {
    world.step(intent, &ctx("alpha", who, now))
}

fn must(world: &World, who: &str, now: i64, intent: Intent) -> World {
    step(world, as_user(who), now, &intent)
        .unwrap_or_else(|error| panic!("{intent:?} as {who}: {error:?}"))
        .0
}

fn change(changes: Vec<SchemaChange>) -> Intent {
    Intent::ChangeSchema {
        changes,
        expected_version: None,
        existing: ExistingData::Reject,
    }
}

/// An operation that creates a note with the slug it is given.
fn file_note(id: &str, invokers: TenantOperationInvokers) -> TenantOperationDef {
    TenantOperationDef {
        id: op_id(id),
        label: "File a note".into(),
        inputs: vec![TenantOperationInput {
            name: "slug".into(),
            ty: FieldType::Str,
            required: true,
            label: None,
        }],
        invokers,
        subject: None,
        conditions: vec![],
        steps: vec![TenantOperationStep::CreateRecord {
            id: "made".into(),
            object: object("note"),
            fields: BTreeMap::from([
                (field("title"), lit("Filed")),
                (field("slug"), Expr::Param("slug".into())),
            ]),
        }],
    }
}

fn anyone() -> TenantOperationInvokers {
    TenantOperationInvokers {
        signed_in: true,
        roles: BTreeSet::new(),
    }
}

fn rule(id: &str, target: HoldTarget, when: Option<Expr>) -> HoldRule {
    HoldRule {
        id: rule_id(id),
        label: id.into(),
        target,
        when,
        approver: CapabilityName(APPROVER.into()),
    }
}

fn invoke(op: &str, slug: &str, key: &str) -> Intent {
    Intent::InvokeOperation {
        operation: op_id(op),
        inputs: BTreeMap::from([("slug".to_string(), text(slug))]),
        idempotency_key: key.to_string(),
    }
}

fn decide(n: u64, decision: HoldDecision, read_revision: u64) -> Intent {
    Intent::DecideHold {
        hold: hold_id(n),
        decision,
        read_revision,
        reason: None,
    }
}

/// The notes world with: a role `member` that grants nothing, held by `alpha:stranger`; a role
/// `approver` that grants the capability change-approve, held by `alpha:boss` and `alpha:boss2`;
/// the operation `file_note` for any signed-in person, held by the rule `hold_file_note`.
fn world() -> World {
    let world = must(
        &notes_world(),
        "owner",
        5_000,
        change(vec![
            SchemaChange::DefineRole(RoleDef {
                id: role("member"),
                label: "Member".into(),
            }),
            SchemaChange::DefineRole(RoleDef {
                id: role("approver"),
                label: "Approver".into(),
            }),
            SchemaChange::DefineRolePermissions {
                role: role("approver"),
                permissions: vec![Permission::Capability(APPROVER.into())],
            },
            SchemaChange::DefineOperation(file_note("file_note", anyone())),
            SchemaChange::DefineHoldRule(rule(
                "hold_file_note",
                HoldTarget::Operation {
                    operation: op_id("file_note"),
                },
                None,
            )),
        ]),
    );
    let mut world = world;
    for (who, role_name) in [
        ("stranger", "member"),
        ("boss", "approver"),
        ("boss2", "approver"),
    ] {
        world = must(
            &world,
            "owner",
            5_001,
            Intent::AssignRole {
                principal: principal_id(&format!("alpha:{who}")),
                role: role(role_name),
            },
        );
    }
    world
}

fn tenant(world: &World) -> &TenantState {
    &world.tenants[&tenant_id("alpha")]
}

fn hold(world: &World, n: u64) -> HeldOperation {
    tenant(world)
        .schema
        .holds
        .as_ref()
        .and_then(|holds| holds.get(&hold_id(n)))
        .cloned()
        .unwrap_or_else(|| panic!("no hold h-{n}"))
}

fn note_count(world: &World) -> usize {
    tenant(world)
        .records
        .get(&object("note"))
        .map_or(0, BTreeMap::len)
}

/// The world after `stranger` invoked `file_note`: hold h-1 pending, no note made.
fn with_hold() -> World {
    step(
        &world(),
        as_user("stranger"),
        FILED_AT,
        &invoke("file_note", "slug-a", "k-1"),
    )
    .expect("filed")
    .0
}

fn refused(world: &World, who: Principal, now: i64, intent: &Intent) -> DomainError {
    step(world, who, now, intent).expect_err("refused")
}

// ---------------------------------------------------------------------------------------------
// Filing (ADM-1601).
// ---------------------------------------------------------------------------------------------

// Verifies: ADM-1601
#[test]
fn a_change_a_rule_names_is_filed_and_changes_no_record() {
    let before = world();
    let (after, outcome) = step(
        &before,
        as_user("stranger"),
        FILED_AT,
        &invoke("file_note", "slug-a", "k-1"),
    )
    .expect("a held change is accepted as a filing");
    assert_eq!(
        outcome.output,
        Output::Held {
            hold: hold_id(1),
            approver: CapabilityName(APPROVER.into()),
            expires_at: Timestamp(FILED_AT + WEEK_MS),
        }
    );
    // Nothing the operation does happened: no note, and the delta holds only the schema.
    assert_eq!(note_count(&after), 0);
    assert!(outcome
        .delta
        .ops
        .iter()
        .all(|op| matches!(op, DeltaOp::PutSchema(_))));
    let filed = hold(&after, 1);
    assert_eq!(filed.requester, principal_id("alpha:stranger"));
    assert_eq!(filed.on_behalf_of, None);
    assert_eq!(filed.approver.0, APPROVER);
    assert_eq!(filed.rule, Some(rule_id("hold_file_note")));
    assert_eq!(filed.state, HoldState::Pending);
    assert_eq!(filed.revision, 1);
    assert_eq!(filed.filed_at, Timestamp(FILED_AT));
    assert_eq!(filed.expires_at, Timestamp(FILED_AT + WEEK_MS));
    assert_eq!(filed.input_hash.len(), 64);
    assert!(
        matches!(&filed.change, HeldChange::InvokeOperation { operation, .. } if *operation == op_id("file_note"))
    );
    assert!(outcome
        .evidence
        .iter()
        .any(|check| check.passed && check.rule == "hold:filed"));
}

// Verifies: ADM-1601
#[test]
fn a_condition_of_the_rule_holds_only_the_changes_it_does_not_rule_out() {
    // Hold file_note only for the slug `secret`.
    let world = must(
        &world(),
        "owner",
        5_100,
        change(vec![SchemaChange::DefineHoldRule(rule(
            "hold_file_note",
            HoldTarget::Operation {
                operation: op_id("file_note"),
            },
            Some(eq(Expr::Param("slug".into()), lit("secret"))),
        ))]),
    );
    let (open, direct) = step(
        &world,
        as_user("stranger"),
        FILED_AT,
        &invoke("file_note", "open", "k-1"),
    )
    .expect("accepted");
    assert!(matches!(direct.output, Output::Operation { .. }));
    assert_eq!(note_count(&open), 1);

    let (held, filed) = step(
        &world,
        as_user("stranger"),
        FILED_AT,
        &invoke("file_note", "secret", "k-2"),
    )
    .expect("accepted");
    assert!(matches!(filed.output, Output::Held { .. }));
    assert_eq!(note_count(&held), 0);
}

// Verifies: ADM-1601
#[test]
fn a_rule_that_names_a_role_holds_the_ways_to_confer_it() {
    let world = must(
        &world(),
        "owner",
        5_100,
        change(vec![SchemaChange::DefineHoldRule(rule(
            "hold_member",
            HoldTarget::Role {
                role: role("member"),
            },
            None,
        ))]),
    );
    let newcomer = principal_id("alpha:newcomer");
    // A direct assignment, an invitation, and a place in a team that holds the role.
    let (after, assign) = step(
        &world,
        as_user("owner"),
        FILED_AT,
        &Intent::AssignRole {
            principal: newcomer.clone(),
            role: role("member"),
        },
    )
    .expect("filed");
    assert!(matches!(assign.output, Output::Held { .. }));
    assert!(!tenant(&after).members.contains_key(&newcomer));

    let (_, invite) = step(
        &world,
        as_user("owner"),
        FILED_AT,
        &Intent::CreateInvitation {
            invitation: InvitationId::new("welcome").expect("id"),
            address: "new@example.org".into(),
            roles: BTreeSet::from([role("member")]),
        },
    )
    .expect("filed");
    assert!(matches!(invite.output, Output::Held { .. }));

    let team = TeamId::new("crew").expect("team");
    let world = must(
        &world,
        "owner",
        FILED_AT,
        Intent::DefineTeam {
            team: team.clone(),
            label: "Crew".into(),
        },
    );
    // Defining the team and giving it the role is itself a grant of the role, and is held.
    let (world, team_role) = step(
        &world,
        as_user("owner"),
        FILED_AT + 1,
        &Intent::AssignTeamRole {
            team: team.clone(),
            role: role("member"),
        },
    )
    .expect("filed");
    assert!(matches!(team_role.output, Output::Held { .. }));
    // Once the approver lets it through, joining the team is held too.
    let world = must(
        &world,
        "boss",
        FILED_AT + 2,
        decide(1, HoldDecision::Approve, 1),
    );
    let (_, join) = step(
        &world,
        as_user("owner"),
        FILED_AT + 3,
        &Intent::AddTeamMember {
            team,
            principal: newcomer,
        },
    )
    .expect("filed");
    assert!(matches!(join.output, Output::Held { .. }));
}

// Verifies: ADM-1640
#[test]
fn a_rule_is_refused_at_definition_unless_it_names_what_exists() {
    let world = world();
    let define = |rule: HoldRule| change(vec![SchemaChange::DefineHoldRule(rule)]);

    let no_operation = rule(
        "bad_operation",
        HoldTarget::Operation {
            operation: op_id("nothing"),
        },
        None,
    );
    let error = refused(&world, as_user("owner"), 6_000, &define(no_operation));
    assert_eq!(error.kind, ErrorKind::InvalidSchema);

    let mut undeclared = rule(
        "bad_approver",
        HoldTarget::Operation {
            operation: op_id("file_note"),
        },
        None,
    );
    undeclared.approver = CapabilityName("shop__approve".into());
    let error = refused(&world, as_user("owner"), 6_000, &define(undeclared));
    assert_eq!(error.kind, ErrorKind::InvalidSchema);

    // Defining a rule is the power of a holder of holds-define: an editor does not hold it.
    let fine = rule(
        "fine",
        HoldTarget::Operation {
            operation: op_id("file_note"),
        },
        None,
    );
    let error = refused(&world, as_user("editor"), 6_000, &define(fine));
    assert_eq!(error.kind, ErrorKind::Unauthorized);
}

// Verifies: ADM-1641
#[test]
fn file_hold_files_only_a_change_that_would_be_accepted_now() {
    let world = world();
    let assign = HeldChange::AssignRole {
        principal: principal_id("alpha:newcomer"),
        role: role("member"),
    };
    let file = |change: HeldChange| Intent::FileHold {
        change,
        approver: CapabilityName(APPROVER.into()),
        on_behalf_of: None,
    };
    // A member may not assign a role, so nothing that could never apply is filed.
    let error = refused(&world, as_user("stranger"), FILED_AT, &file(assign.clone()));
    assert_eq!(error.kind, ErrorKind::Unauthorized);
    assert!(tenant(&world).schema.holds.is_none());

    // The owner may, and the hold waits whether or not a rule names the change.
    let (after, outcome) = step(&world, as_user("owner"), FILED_AT, &file(assign)).expect("filed");
    assert!(matches!(outcome.output, Output::Held { .. }));
    assert_eq!(hold(&after, 1).rule, None);
    assert!(!tenant(&after)
        .members
        .contains_key(&principal_id("alpha:newcomer")));

    // An approver capability nobody declared is no approver.
    let error = refused(
        &world,
        as_user("owner"),
        FILED_AT,
        &Intent::FileHold {
            change: HeldChange::AssignRole {
                principal: principal_id("alpha:newcomer"),
                role: role("member"),
            },
            approver: CapabilityName("shop__approve".into()),
            on_behalf_of: None,
        },
    );
    assert_eq!(error.kind, ErrorKind::InvalidIntent);
}

// ---------------------------------------------------------------------------------------------
// Who decides (ADM-1602).
// ---------------------------------------------------------------------------------------------

// Verifies: ADM-1602
#[test]
fn only_a_person_who_holds_the_approver_permission_decides() {
    let world = with_hold();
    let approve = decide(1, HoldDecision::Approve, 1);
    // A member without the capability.
    let error = refused(&world, as_user("stranger"), FILED_AT + 1, &approve);
    assert_eq!(error.kind, ErrorKind::ApproverPermission);
    // An automation principal, the system and an anonymous visitor are no approver.
    for who in [
        user("automation:nightly"),
        Principal::System,
        Principal::Anonymous,
    ] {
        let error = refused(&world, who, FILED_AT + 1, &approve);
        assert_eq!(error.kind, ErrorKind::ApproverPermission);
    }
    // The hold is as it was.
    assert_eq!(hold(&world, 1).state, HoldState::Pending);
    // The owner holds every capability, and decides.
    assert!(step(&world, as_user("owner"), FILED_AT + 1, &approve).is_ok());
}

// Verifies: ADM-1602
#[test]
fn nobody_decides_their_own_request_or_the_one_made_for_them() {
    // boss holds the approver permission and files a change for approval.
    let (world, _) = step(
        &world(),
        as_user("boss"),
        FILED_AT,
        &invoke("file_note", "slug-b", "k-1"),
    )
    .expect("filed");
    let error = refused(
        &world,
        as_user("boss"),
        FILED_AT + 1,
        &decide(1, HoldDecision::Approve, 1),
    );
    assert_eq!(error.kind, ErrorKind::SelfAction);
    let error = refused(
        &world,
        as_user("boss"),
        FILED_AT + 1,
        &decide(1, HoldDecision::Reject, 1),
    );
    assert_eq!(error.kind, ErrorKind::SelfAction);
    assert!(step(
        &world,
        as_user("boss2"),
        FILED_AT + 1,
        &decide(1, HoldDecision::Approve, 1)
    )
    .is_ok());

    // A hold filed on behalf of boss2 is not boss2's to decide, though boss2 holds the permission.
    let (world, _) = step(
        &self::world(),
        as_user("boss"),
        FILED_AT,
        &Intent::FileHold {
            change: HeldChange::InvokeOperation {
                operation: op_id("file_note"),
                inputs: BTreeMap::from([("slug".to_string(), text("slug-c"))]),
                idempotency_key: "k-2".into(),
            },
            approver: CapabilityName(APPROVER.into()),
            on_behalf_of: Some(principal_id("alpha:boss2")),
        },
    )
    .expect("filed");
    let error = refused(
        &world,
        as_user("boss2"),
        FILED_AT + 1,
        &decide(1, HoldDecision::Approve, 1),
    );
    assert_eq!(error.kind, ErrorKind::SelfAction);
}

// ---------------------------------------------------------------------------------------------
// Expiry and staleness (ADM-1603, ADM-1604).
// ---------------------------------------------------------------------------------------------

// Verifies: ADM-1603
#[test]
fn a_hold_not_decided_within_seven_days_expires() {
    let world = with_hold();
    let approve = decide(1, HoldDecision::Approve, 1);
    let error = refused(&world, as_user("boss"), FILED_AT + WEEK_MS + 1, &approve);
    assert_eq!(error.kind, ErrorKind::HoldExpired);
    // The day it expires it can still be decided; a day later nothing changes it.
    assert!(step(&world, as_user("boss"), FILED_AT + WEEK_MS, &approve).is_ok());
    assert_eq!(hold(&world, 1).state, HoldState::Pending);
    assert_eq!(note_count(&world), 0);
}

// Verifies: ADM-1604
#[test]
fn an_approval_refuses_when_the_state_the_change_read_has_moved() {
    // Hold the assignment of the role member.
    let world = must(
        &world(),
        "owner",
        5_100,
        change(vec![SchemaChange::DefineHoldRule(rule(
            "hold_member",
            HoldTarget::Role {
                role: role("member"),
            },
            None,
        ))]),
    );
    let newcomer = principal_id("alpha:newcomer");
    let (world, _) = step(
        &world,
        as_user("owner"),
        FILED_AT,
        &Intent::AssignRole {
            principal: newcomer.clone(),
            role: role("member"),
        },
    )
    .expect("filed");
    // Meanwhile the person is given another role directly (not held).
    let world = must(
        &world,
        "owner",
        FILED_AT + 1,
        Intent::AssignRole {
            principal: newcomer.clone(),
            role: role("writer"),
        },
    );
    let error = refused(
        &world,
        as_user("boss"),
        FILED_AT + 2,
        &decide(1, HoldDecision::Approve, 1),
    );
    assert_eq!(error.kind, ErrorKind::StaleHold);
    assert!(error
        .evidence
        .iter()
        .any(|c| !c.passed && c.rule == "hold:input_hash"));
    // It stays pending, to be rejected or to lapse; the person's roles are untouched.
    assert_eq!(hold(&world, 1).state, HoldState::Pending);
    assert_eq!(
        tenant(&world).members[&newcomer],
        BTreeSet::from([role("writer")])
    );
}

// Verifies: ADM-1604
#[test]
fn an_approval_applies_with_the_authority_the_requester_holds_at_that_moment() {
    // An operation only staff may invoke, held by the rule.
    let world = must(
        &world(),
        "owner",
        5_100,
        change(vec![
            SchemaChange::DefineRole(RoleDef {
                id: role("staff"),
                label: "Staff".into(),
            }),
            SchemaChange::DefineOperation(file_note(
                "staff_note",
                TenantOperationInvokers {
                    signed_in: false,
                    roles: BTreeSet::from([role("staff")]),
                },
            )),
            SchemaChange::DefineHoldRule(rule(
                "hold_staff_note",
                HoldTarget::Operation {
                    operation: op_id("staff_note"),
                },
                None,
            )),
        ]),
    );
    let world = must(
        &world,
        "owner",
        5_101,
        Intent::AssignRole {
            principal: principal_id("alpha:stranger"),
            role: role("staff"),
        },
    );
    let (world, filed) = step(
        &world,
        as_user("stranger"),
        FILED_AT,
        &invoke("staff_note", "slug-s", "k-1"),
    )
    .expect("filed");
    assert!(matches!(filed.output, Output::Held { .. }));
    // The requester leaves the staff before the approver decides.
    let world = must(
        &world,
        "owner",
        FILED_AT + 1,
        Intent::RevokeRole {
            principal: principal_id("alpha:stranger"),
            role: role("staff"),
        },
    );
    let error = refused(
        &world,
        as_user("boss"),
        FILED_AT + 2,
        &decide(1, HoldDecision::Approve, 1),
    );
    assert_eq!(error.kind, ErrorKind::StaleHold);
    assert!(error
        .evidence
        .iter()
        .any(|c| !c.passed && c.rule == "hold:requester_authority"));
    assert_eq!(note_count(&world), 0);
    assert_eq!(hold(&world, 1).state, HoldState::Pending);
}

// Verifies: ADM-1604
#[test]
fn an_approved_change_applies_once_as_the_requester() {
    let world = with_hold();
    let (after, outcome) = step(
        &world,
        as_user("boss"),
        FILED_AT + 1,
        &decide(1, HoldDecision::Approve, 1),
    )
    .expect("approved");
    assert_eq!(
        outcome.output,
        Output::HoldDecided {
            hold: hold_id(1),
            state: HoldState::Applied,
            revision: 2,
        }
    );
    // The change is applied as the person who asked, in the commit that records the approval.
    let record = tenant(&after)
        .record(&object("note"), RecordId(1))
        .expect("the note exists");
    assert_eq!(record.created_by, Some(principal_id("alpha:stranger")));
    assert_eq!(record.get(&field("slug")), &text("slug-a"));
    let decided = hold(&after, 1);
    assert_eq!(decided.state, HoldState::Applied);
    assert_eq!(decided.revision, 2);
    assert_eq!(decided.decided_by, Some(principal_id("alpha:boss")));
    assert_eq!(decided.decided_at, Some(Timestamp(FILED_AT + 1)));
    // And never again.
    for read in [1, 2] {
        let error = refused(
            &after,
            as_user("boss2"),
            FILED_AT + 2,
            &decide(1, HoldDecision::Approve, read),
        );
        assert_eq!(error.kind, ErrorKind::VersionConflict);
    }
    assert_eq!(note_count(&after), 1);
}

// Verifies: ADM-1604
#[test]
fn a_setting_write_that_a_rule_names_waits_for_its_approver() {
    let page_size = SettingDef {
        id: SettingId::new("page_size").expect("setting"),
        label: "Page size".into(),
        r#type: SettingType::new("i64").expect("type"),
        default: Value::Int(20),
        levels: BTreeSet::from([SettingLevel::Tenant]),
        locked_by: None,
        area: None,
        description: None,
        allowed: None,
    };
    let world = must(
        &world(),
        "owner",
        5_100,
        change(vec![
            SchemaChange::DefineSetting(page_size),
            SchemaChange::DefineHoldRule(rule(
                "hold_page_size",
                HoldTarget::Setting {
                    setting: SettingId::new("page_size").expect("setting"),
                },
                None,
            )),
        ]),
    );
    let write = Intent::WriteSetting {
        setting: SettingId::new("page_size").expect("setting"),
        level: SettingLevel::Tenant,
        team: None,
        value: Some(Value::Int(50)),
        expected_revision: 0,
    };
    let (world, filed) = step(&world, as_user("owner"), FILED_AT, &write).expect("filed");
    assert!(matches!(filed.output, Output::Held { .. }));
    assert_eq!(page_size_of(&world), Value::Int(20));
    let world = must(
        &world,
        "boss",
        FILED_AT + 1,
        decide(1, HoldDecision::Approve, 1),
    );
    assert_eq!(page_size_of(&world), Value::Int(50));
}

fn page_size_of(world: &World) -> Value {
    strata_kernel::settings::resolve(
        tenant(world),
        &as_user("owner"),
        &SettingId::new("page_size").expect("setting"),
    )
    .expect("resolved")
    .value
}

// ---------------------------------------------------------------------------------------------
// One decision (ADM-1614, D-8406).
// ---------------------------------------------------------------------------------------------

// Verifies: ADM-1614
#[test]
fn of_two_approvers_that_race_one_decision_takes_effect() {
    let world = with_hold();
    // Both read revision 1. The first commits.
    let first = must(
        &world,
        "boss",
        FILED_AT + 1,
        decide(1, HoldDecision::Approve, 1),
    );
    // The second, on the state the first left, loses with a conflict and changes nothing.
    let error = refused(
        &first,
        as_user("boss2"),
        FILED_AT + 1,
        &decide(1, HoldDecision::Approve, 1),
    );
    assert_eq!(error.kind, ErrorKind::VersionConflict);
    assert_eq!(note_count(&first), 1);
    assert_eq!(hold(&first, 1).decided_by, Some(principal_id("alpha:boss")));

    // The same when the loser would reject: the winner's decision stands.
    let error = refused(
        &first,
        as_user("boss2"),
        FILED_AT + 1,
        &decide(1, HoldDecision::Reject, 1),
    );
    assert_eq!(error.kind, ErrorKind::VersionConflict);
    assert_eq!(hold(&first, 1).state, HoldState::Applied);

    // And a rejection first leaves an approval nothing to apply.
    let rejected = must(
        &world,
        "boss",
        FILED_AT + 1,
        decide(1, HoldDecision::Reject, 1),
    );
    assert_eq!(hold(&rejected, 1).state, HoldState::Rejected);
    let error = refused(
        &rejected,
        as_user("boss2"),
        FILED_AT + 2,
        &decide(1, HoldDecision::Approve, 1),
    );
    assert_eq!(error.kind, ErrorKind::VersionConflict);
    assert_eq!(note_count(&rejected), 0);
}

// Verifies: ADM-1614
#[test]
fn a_decision_that_read_another_revision_is_refused() {
    let world = with_hold();
    for read in [0, 2, 9] {
        let error = refused(
            &world,
            as_user("boss"),
            FILED_AT + 1,
            &decide(1, HoldDecision::Approve, read),
        );
        assert_eq!(error.kind, ErrorKind::VersionConflict);
    }
    assert_eq!(hold(&world, 1).revision, 1);
}

// Verifies: ADM-1614
#[test]
fn a_hold_that_does_not_exist_is_refused() {
    let error = refused(
        &world(),
        as_user("boss"),
        FILED_AT,
        &decide(7, HoldDecision::Approve, 1),
    );
    assert_eq!(error.kind, ErrorKind::UnknownRecord);
}
