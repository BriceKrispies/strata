//! Agent governance (D-8401, D-8424, ADM-1605 to ADM-1608): an agent acts within a ceiling and a
//! per-operation policy, holds no administration power on any path, and has its settings changes
//! held for a human. The backend's half (the audit entry of each refusal) is
//! `crates/backend/tests/agents.rs`.

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::agents::{self, AGENT_APPROVE};
use strata_kernel::testkit::*;
use strata_kernel::*;

const NOW: i64 = 9_000;
const AGENT: &str = "agent:bot";

/// The refusal of a result, without printing the world an accepted one holds.
trait Refused {
    fn refused(self) -> DomainError;
}

impl<T> Refused for Result<T, DomainError> {
    fn refused(self) -> DomainError {
        match self {
            Ok(_) => panic!("the intent was accepted"),
            Err(error) => error,
        }
    }
}

fn op_id(s: &str) -> TenantOperationId {
    TenantOperationId::new(s).expect("operation id")
}

fn setting_id() -> SettingId {
    SettingId::new("page_size").expect("setting")
}

fn scopes(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// The context of `agent` acting for `person` with a token of `list` scopes.
fn agent_ctx(person: &str, list: &[&str], now: i64) -> Context {
    let mut context = ctx("alpha", user(&format!("alpha:{person}")), now);
    context.agent = Some(AgentCall {
        agent: principal_id(AGENT),
        scopes: scopes(list),
    });
    context
}

fn as_person(person: &str, now: i64) -> Context {
    ctx("alpha", user(&format!("alpha:{person}")), now)
}

fn change(world: &World, changes: Vec<SchemaChange>) -> Result<World, DomainError> {
    world
        .step(
            &Intent::ChangeSchema {
                changes,
                expected_version: None,
                existing: ExistingData::Reject,
            },
            &as_person("owner", NOW),
        )
        .map(|(world, _)| world)
}

fn owner_step(world: &World, now: i64, intent: Intent) -> Result<World, DomainError> {
    world
        .step(&intent, &as_person("owner", now))
        .map(|(world, _)| world)
}

fn assign(who: &str, role_name: &str) -> Intent {
    Intent::AssignRole {
        principal: principal_id(who),
        role: role(role_name),
    }
}

fn define_role(id: &str, permissions: Vec<Permission>) -> Vec<SchemaChange> {
    vec![
        SchemaChange::DefineRole(RoleDef {
            id: role(id),
            label: id.into(),
        }),
        SchemaChange::DefineRolePermissions {
            role: role(id),
            permissions,
        },
    ]
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

fn invoker_anyone() -> TenantOperationInvokers {
    TenantOperationInvokers {
        signed_in: true,
        roles: BTreeSet::new(),
    }
}

/// An operation that creates a note with the slug it is given, with the agent policy `policy`.
fn file_note(id: &str, policy: Option<AgentPolicy>) -> TenantOperationDef {
    TenantOperationDef {
        id: op_id(id),
        label: "File a note".into(),
        inputs: vec![TenantOperationInput {
            name: "slug".into(),
            ty: FieldType::Str,
            required: true,
            label: None,
        }],
        invokers: invoker_anyone(),
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
        agent_policy: policy,
    }
}

fn invoke(op: &str, slug: &str) -> Intent {
    Intent::InvokeOperation {
        operation: op_id(op),
        inputs: BTreeMap::from([("slug".to_string(), text(slug))]),
        idempotency_key: format!("k-{slug}"),
    }
}

fn write_page_size(value: i64) -> Intent {
    Intent::WriteSetting {
        setting: setting_id(),
        level: SettingLevel::Tenant,
        team: None,
        value: Some(Value::Int(value)),
        expected_revision: 0,
    }
}

/// The notes world, with: the agent `agent:bot` holding the role `bot` (it reads, creates and
/// updates notes and writes tenant settings, and it may invoke every signed-in operation); `boss`
/// holding the role `approver` (the capability agent-approve); the operations `direct_note`
/// (direct), `held_note` (approval), `denied_note` (deny) and `silent_note` (none declared); the
/// setting `page_size`; and the roles `admin_caps` (a capability only an administrator holds) and
/// `member_manager` (manage members).
fn base() -> World {
    let mut changes = define_role(
        "bot",
        vec![
            on_note(&[
                ObjectAction::Read,
                ObjectAction::Create,
                ObjectAction::Update,
            ]),
            capability(strata_kernel::settings::SETTING_WRITE_TENANT),
        ],
    );
    changes.extend(define_role("approver", vec![capability(AGENT_APPROVE)]));
    changes.extend(define_role("admin_caps", vec![capability("holds-define")]));
    changes.extend(define_role("member_manager", vec![]));
    changes.push(SchemaChange::DefinePolicy(PolicyDef {
        id: PolicyId::new("managers_manage_members").expect("policy"),
        role: role("member_manager"),
        resource: Resource::Members,
        actions: BTreeSet::from([Action::Manage]),
        condition: None,
    }));
    for (id, policy) in [
        ("direct_note", Some(AgentPolicy::Direct)),
        ("held_note", Some(AgentPolicy::Approval)),
        ("denied_note", Some(AgentPolicy::Deny)),
        ("silent_note", None),
    ] {
        changes.push(SchemaChange::DefineOperation(file_note(id, policy)));
    }
    changes.push(SchemaChange::DefineSetting(SettingDef {
        id: setting_id(),
        label: "Page size".into(),
        r#type: SettingType::new("i64").expect("type"),
        default: Value::Int(20),
        levels: BTreeSet::from([SettingLevel::Tenant]),
        locked_by: None,
        area: None,
        description: None,
        allowed: None,
    }));
    let world = change(&notes_world(), changes).expect("the fixture schema");
    let world = owner_step(&world, NOW + 1, assign(AGENT, "bot")).expect("the agent's role");
    owner_step(&world, NOW + 2, assign("alpha:boss", "approver")).expect("the approver")
}

fn tenant(world: &World) -> &TenantState {
    world
        .tenants
        .get(&tenant_id("alpha"))
        .expect("tenant alpha")
}

fn failed_rules(error: &DomainError) -> Vec<&str> {
    error
        .evidence
        .iter()
        .filter(|check| !check.passed)
        .map(|check| check.rule.as_str())
        .collect()
}

fn note_count(world: &World) -> usize {
    tenant(world).records_of(&object("note")).count()
}

fn create_note(slug: &str) -> Intent {
    new_note("Hello", slug)
}

// Verifies: ADM-1605
#[test]
fn an_agent_acts_only_in_the_intersection_of_its_ceiling_the_person_and_its_token() {
    let world = base();
    // Inside all three: the editor holds create, the agent's ceiling has it, the token names it.
    let (world, _) = world
        .step(
            &create_note("a"),
            &agent_ctx("editor", &["create:note"], NOW + 10),
        )
        .expect("inside the intersection");
    assert_eq!(note_count(&world), 1);

    // The ceiling: the agent's role cannot delete, though the editor (the person) can and the
    // token names it.
    let error = world
        .step(
            &Intent::DeleteRecord {
                object: object("note"),
                id: RecordId(1),
                expected_version: None,
            },
            &agent_ctx("editor", &["delete:*"], NOW + 11),
        )
        .refused();
    assert_eq!(
        error.kind,
        ErrorKind::AgentCeilingDenies,
        "{}",
        error.message
    );
    assert_eq!(failed_rules(&error), ["agent:ceiling"]);

    // The token: the scope names read only, though the agent and the person may create.
    let error = world
        .step(
            &create_note("b"),
            &agent_ctx("editor", &["read:note"], NOW + 12),
        )
        .refused();
    assert_eq!(error.kind, ErrorKind::AgentCeilingDenies);
    assert_eq!(failed_rules(&error), ["agent:token_scope"]);
    // A scope of the action on every object is inside it.
    world
        .step(
            &create_note("b"),
            &agent_ctx("editor", &["create:*"], NOW + 12),
        )
        .expect("the action on every object");

    // The person: a stranger holds no grant, so the agent acting for the stranger has none.
    let error = world
        .step(
            &create_note("c"),
            &agent_ctx("stranger", &["create:note"], NOW + 13),
        )
        .refused();
    assert_eq!(error.kind, ErrorKind::Unauthorized);

    // The same call by the person alone is not an agent's call, and is not held to its ceiling.
    world
        .step(
            &Intent::DeleteRecord {
                object: object("note"),
                id: RecordId(1),
                expected_version: None,
            },
            &as_person("editor", NOW + 14),
        )
        .expect("a person deletes");
}

// Verifies: ADM-1606
#[test]
fn each_operation_has_an_agent_policy_and_deny_is_refused() {
    let world = base();
    // direct: decided as a person's call is.
    let (direct, _) = world
        .step(
            &invoke("direct_note", "d"),
            &agent_ctx("editor", &["create:note"], NOW + 10),
        )
        .expect("direct");
    assert_eq!(note_count(&direct), 1);

    // deny, declared or not declared: refused with the policy named.
    for op in ["denied_note", "silent_note"] {
        let error = world
            .step(
                &invoke(op, "x"),
                &agent_ctx("editor", &["create:note"], NOW + 11),
            )
            .refused();
        assert_eq!(
            error.kind,
            ErrorKind::AgentDenied,
            "{op}: {}",
            error.message
        );
        assert_eq!(failed_rules(&error), ["agent:policy"]);
    }
    // The platform's own table: what touches roles, the schema or holds is deny, however the
    // person's own authority reads.
    for intent in [
        assign("alpha:stranger", "bot"),
        Intent::ChangeSchema {
            changes: vec![],
            expected_version: None,
            existing: ExistingData::Reject,
        },
        Intent::DecideHold {
            hold: HeldOperationId::new("h-1").expect("hold id"),
            decision: HoldDecision::Approve,
            read_revision: 1,
            reason: None,
        },
    ] {
        let error = world
            .step(&intent, &agent_ctx("owner", &["update:*"], NOW + 12))
            .refused();
        assert_eq!(
            error.kind,
            ErrorKind::AgentDenied,
            "{intent:?}: {}",
            error.message
        );
    }

    // approval: filed on behalf of the person the agent acts for, applied only when decided.
    let (held, filed) = world
        .step(
            &invoke("held_note", "h"),
            &agent_ctx("editor", &["create:note"], NOW + 13),
        )
        .expect("filed");
    let Output::Held { approver, .. } = &filed.output else {
        panic!("not held: {:?}", filed.output);
    };
    assert_eq!(approver.as_str(), AGENT_APPROVE);
    assert_eq!(note_count(&held), 0);
    let hold = tenant(&held)
        .schema
        .holds
        .as_ref()
        .and_then(|holds| holds.values().next())
        .expect("the hold");
    assert_eq!(hold.requester, principal_id("alpha:editor"));
    assert_eq!(hold.on_behalf_of, Some(principal_id("alpha:editor")));
    assert_eq!(hold.agent, Some(principal_id(AGENT)));

    // The person the agent acts for does not approve their agent's call; nor does the agent.
    let decide = Intent::DecideHold {
        hold: hold.id.clone(),
        decision: HoldDecision::Approve,
        read_revision: 1,
        reason: None,
    };
    let held_owner = owner_step(&held, NOW + 14, decide.clone());
    assert!(
        held_owner.is_ok(),
        "an owner other than the person approves"
    );
    let error = held.step(&decide, &as_person("editor", NOW + 14)).refused();
    assert_eq!(error.kind, ErrorKind::ApproverPermission);
    let error = held
        .step(&decide, &agent_ctx("boss", &[], NOW + 14))
        .refused();
    assert_eq!(error.kind, ErrorKind::AgentDenied);
    let error = held
        .step(&decide, &ctx("alpha", user(AGENT), NOW + 14))
        .refused();
    assert_eq!(error.kind, ErrorKind::ApproverPermission);
    // The approver applies it, as the person.
    let (applied, _) = held
        .step(&decide, &as_person("boss", NOW + 15))
        .expect("approved");
    assert_eq!(note_count(&applied), 1);

    // The product syntax: an operation of a product file declares `agent_policy`.
    let declared = serde_json::to_value(file_note("p", Some(AgentPolicy::Approval)))
        .expect("an operation is JSON");
    assert_eq!(declared["agent_policy"], "approval");
    let parsed: TenantOperationDef = serde_json::from_value(declared).expect("and reads back");
    assert_eq!(parsed.agent_policy, Some(AgentPolicy::Approval));

    // The list an agent reads shows only what its ceiling allows and its policy does not deny.
    let listed: Vec<_> = agents::visible_operations(tenant(&world), &principal_id(AGENT))
        .into_iter()
        .map(|(id, policy)| (id.as_str().to_string(), policy))
        .collect();
    assert_eq!(
        listed,
        [
            ("direct_note".to_string(), AgentPolicy::Direct),
            ("held_note".to_string(), AgentPolicy::Approval),
        ]
    );
    // An agent with no role that invokes anything sees what every signed-in person may invoke;
    // an operation that names a role is not listed for it.
    let narrowed = change(
        &world,
        vec![SchemaChange::DefineOperation(TenantOperationDef {
            invokers: TenantOperationInvokers {
                signed_in: false,
                roles: BTreeSet::from([role("approver")]),
            },
            ..file_note("approver_only", Some(AgentPolicy::Direct))
        })],
    )
    .expect("defined");
    let listed = agents::visible_operations(tenant(&narrowed), &principal_id(AGENT));
    assert!(listed.iter().all(|(id, _)| id.as_str() != "approver_only"));
}

fn admin_power(error: &DomainError) {
    assert_eq!(error.kind, ErrorKind::AgentAdminPower, "{}", error.message);
}

// Verifies: ADM-1607
#[test]
fn no_path_gives_an_agent_an_administration_power() {
    let world = base();
    let bot = principal_id(AGENT);

    // A role: directly, and the owner role.
    admin_power(&owner_step(&world, NOW + 10, assign(AGENT, "admin_caps")).refused());
    admin_power(&owner_step(&world, NOW + 11, assign(AGENT, "member_manager")).refused());
    admin_power(&owner_step(&world, NOW + 12, assign(AGENT, "owner")).refused());
    // A person with the same role is fine.
    owner_step(&world, NOW + 13, assign("alpha:stranger", "admin_caps")).expect("a person");

    // A scoped role.
    admin_power(
        &owner_step(
            &world,
            NOW + 14,
            Intent::AssignScopedRole {
                principal: bot.clone(),
                role: role("admin_caps"),
                object: object("note"),
                scope: eq(var(Scope::Record, "slug"), lit("north")),
            },
        )
        .refused(),
    );

    // A team: a team that holds an administration role takes no agent, and a team that has an
    // agent takes no administration role.
    let desk = TeamId::new("desk").expect("team");
    let with_team = owner_step(
        &world,
        NOW + 15,
        Intent::DefineTeam {
            team: desk.clone(),
            label: "Desk".into(),
        },
    )
    .expect("team");
    let with_team = owner_step(
        &with_team,
        NOW + 16,
        Intent::AssignTeamRole {
            team: desk.clone(),
            role: role("admin_caps"),
        },
    )
    .expect("a team with no agent in it");
    admin_power(
        &owner_step(
            &with_team,
            NOW + 17,
            Intent::AddTeamMember {
                team: desk.clone(),
                principal: bot.clone(),
            },
        )
        .refused(),
    );
    let agent_team = owner_step(
        &world,
        NOW + 18,
        Intent::DefineTeam {
            team: desk.clone(),
            label: "Desk".into(),
        },
    )
    .expect("team");
    let agent_team = owner_step(
        &agent_team,
        NOW + 19,
        Intent::AddTeamMember {
            team: desk.clone(),
            principal: bot.clone(),
        },
    )
    .expect("an agent joins a team of no power");
    admin_power(
        &owner_step(
            &agent_team,
            NOW + 20,
            Intent::AssignTeamRole {
                team: desk,
                role: role("member_manager"),
            },
        )
        .refused(),
    );

    // An invitation: redeemed by the agent's principal, it would give the roles it offers.
    let invited = owner_step(
        &world,
        NOW + 21,
        Intent::CreateInvitation {
            invitation: InvitationId::new("inv1").expect("invitation"),
            address: "bot@example.test".into(),
            roles: BTreeSet::from([role("admin_caps")]),
        },
    )
    .expect("invitation");
    let mut redeemer = ctx("alpha", Principal::User(bot.clone()), NOW + 22);
    redeemer.verified_address = Some("bot@example.test".into());
    admin_power(
        &invited
            .step(
                &Intent::RedeemInvitation {
                    invitation: InvitationId::new("inv1").expect("invitation"),
                },
                &redeemer,
            )
            .refused(),
    );

    // A schema change: widening the role the agent holds, with a capability or a policy, and a
    // product upgrade that carries either.
    admin_power(
        &change(
            &world,
            vec![SchemaChange::DefineRolePermissions {
                role: role("bot"),
                permissions: vec![capability("holds-define")],
            }],
        )
        .refused(),
    );
    admin_power(
        &change(
            &world,
            vec![SchemaChange::DefinePolicy(PolicyDef {
                id: PolicyId::new("bots_manage_schema").expect("policy"),
                role: role("bot"),
                resource: Resource::Schema,
                actions: BTreeSet::from([Action::Manage]),
                condition: None,
            })],
        )
        .refused(),
    );
    admin_power(
        &change(
            &world,
            vec![
                SchemaChange::RecordProduct(InstalledProduct {
                    product: ProductId::new("demo").expect("product"),
                    version: "2.0.0".into(),
                    platform: None,
                    owns: vec![],
                    schema_version: None,
                }),
                SchemaChange::DefineRolePermissions {
                    role: role("bot"),
                    permissions: vec![capability("bundle-install")],
                },
            ],
        )
        .refused(),
    );
    // A widening that is no administration power is no refusal of this rule.
    change(
        &world,
        vec![SchemaChange::DefineRolePermissions {
            role: role("bot"),
            permissions: vec![on_note(&[ObjectAction::Read])],
        }],
    )
    .expect("an ordinary change");

    // A tenant founded for an agent, and a token that names an administration scope.
    let error = World::default()
        .step(
            &Intent::CreateTenant {
                name: "gamma".into(),
                owner: bot.clone(),
            },
            &ctx("gamma", Principal::System, NOW),
        )
        .refused();
    admin_power(&error);
    let error = world
        .step(
            &create_note("z"),
            &agent_ctx("editor", &["create:note", "manage:members"], NOW + 23),
        )
        .refused();
    admin_power(&error);
    assert_eq!(failed_rules(&error), ["agent:no_admin_scope"]);
}

// Verifies: ADM-1608
#[test]
fn an_agents_settings_change_is_held_for_a_human() {
    let world = base();
    let write = write_page_size(50);
    // Filed, not applied, with no hold rule at all.
    let (held, filed) = world
        .step(&write, &agent_ctx("owner", &["update:*"], NOW + 10))
        .expect("filed");
    let Output::Held { approver, .. } = &filed.output else {
        panic!("not held: {:?}", filed.output);
    };
    assert_eq!(approver.as_str(), AGENT_APPROVE);
    let resolved = |world: &World| {
        strata_kernel::settings::resolve(
            tenant(world),
            &as_person("owner", NOW).principal,
            &setting_id(),
        )
        .expect("resolved")
        .value
    };
    assert_eq!(resolved(&held), Value::Int(20));

    // The person the agent acts for cannot decide it, a human approver does, and it applies once.
    let decide = Intent::DecideHold {
        hold: HeldOperationId::new("h-1").expect("hold id"),
        decision: HoldDecision::Approve,
        read_revision: 1,
        reason: None,
    };
    let error = held.step(&decide, &as_person("owner", NOW + 11)).refused();
    assert_eq!(error.kind, ErrorKind::SelfAction);
    let (applied, _) = held
        .step(&decide, &as_person("boss", NOW + 12))
        .expect("approved");
    assert_eq!(resolved(&applied), Value::Int(50));

    // The ceiling is asked again at the approval: an agent that lost the role cannot have its
    // held call applied.
    let lost = owner_step(
        &held,
        NOW + 11,
        Intent::RevokeRole {
            principal: principal_id(AGENT),
            role: role("bot"),
        },
    )
    .expect("revoked");
    let error = lost.step(&decide, &as_person("boss", NOW + 12)).refused();
    assert_eq!(error.kind, ErrorKind::StaleHold);
    assert!(failed_rules(&error).contains(&"hold:agent_ceiling"));

    // A rejection leaves it as it was.
    let reject = Intent::DecideHold {
        hold: HeldOperationId::new("h-1").expect("hold id"),
        decision: HoldDecision::Reject,
        read_revision: 1,
        reason: None,
    };
    let (rejected, _) = held
        .step(&reject, &as_person("boss", NOW + 12))
        .expect("rejected");
    assert_eq!(resolved(&rejected), Value::Int(20));

    // A person's own write is direct, and an agent whose ceiling lacks the capability is refused
    // before anything is filed.
    let (direct, _) = world
        .step(&write, &as_person("owner", NOW + 13))
        .expect("a person writes");
    assert_eq!(resolved(&direct), Value::Int(50));
    let no_write = change(
        &world,
        vec![SchemaChange::DefineRolePermissions {
            role: role("bot"),
            permissions: vec![on_note(&[ObjectAction::Read])],
        }],
    )
    .expect("narrowed");
    let error = no_write
        .step(&write, &agent_ctx("owner", &["update:*"], NOW + 14))
        .refused();
    assert_eq!(error.kind, ErrorKind::AgentCeilingDenies);
    assert!(tenant(&no_write).schema.holds.is_none());
}
