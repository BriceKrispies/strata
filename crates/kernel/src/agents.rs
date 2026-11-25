//! Agent governance (D-8401, D-8424, ADM-1605 to ADM-1608).
//!
//! An agent acts for a person. The gateway says so in [`Context::agent`], from the `act` claim
//! and the scope of the access token the agent presents (`TOK-003`): the principal of the context
//! is the person, and the agent is the principal `agent:<client>`. Every call by an agent goes
//! through [`gate`] before anything else is decided, and the gate decides three things in this
//! order, each refusal naming its check:
//!
//! 1. **No administration power.** The agent holds none in its roles, none in the scopes of its
//!    token, and a principal id that starts with the reserved prefix is never the owner of a
//!    tenant (`AgentAdminPower`, ADM-1607). What counts as an administration power is
//!    [`admin_powers`]: the owner role, a policy that manages members or the schema, and the
//!    capabilities in [`ADMIN_CAPABILITIES`].
//! 2. **The per-operation policy.** Every operation has an agent policy, [`policy_of`]: `direct`,
//!    `preview`, `approval` or `deny`. A tenant operation declares its own; one that declares none
//!    is `deny`. The platform's other intents have the fixed policy of the table in [`policy_of`]:
//!    record changes are `direct`, a setting write is `approval`, and everything that touches
//!    roles, teams, invitations, the schema, holds, the ledger or a subject's data is `deny`
//!    (`AgentDenied`, ADM-1606).
//! 3. **The ceiling.** The call must be inside the intersection of the ceiling of the agent (what
//!    its own roles allow), the scope of its token, and the current grants of the person it acts
//!    for. The third is the ordinary authority check the rest of the decision makes of the
//!    person; the first two are asked here (`AgentCeilingDenies`, ADM-1605).
//!
//! A call whose policy is `approval` is not applied. The gate files it as a held operation for a
//! human approver on behalf of the person ([`crate::holds`]), and the approval applies it as the
//! person after asking the agent's ceiling again (ADM-1606, ADM-1608).
//!
//! **No path gives an agent an administration power.** [`made_by`] compares the powers the agents
//! of a tenant hold before and after a change, and every path that can give a principal a role (a
//! direct assignment, a scoped assignment, a team's role, a person added to a team, an invitation,
//! a change of what a role grants and so a schema change or a product install) refuses a change
//! that gives one to an agent, as [`crate::duties`] does for duties kept apart.

use crate::holds::HOLDS_DEFINE;
use crate::intent::{CheckExt, DomainErrorExt};
use crate::rules::{assigned_roles, holds_any, may_attempt, role_capabilities, roles_of};
use crate::schema::{Grant, TenantSchema, TenantSchemaExt};
use crate::state::{Principal, TenantState};
use crate::types::{
    owner_role, Action, AgentCall, AgentPolicy, CapabilityName, Check, Context, DomainError,
    ErrorKind, HeldChange, Intent, ObjectTypeId, PrincipalId, Resource, RoleId, SettingLevel,
    TenantOperationDef, TenantOperationId,
};
use std::collections::{BTreeMap, BTreeSet};

/// The prefix of the principal that stands for an agent. A person never holds an id that starts
/// with it, and a principal that does is held to the rules of this module wherever it is found.
pub const AGENT_PREFIX: &str = "agent:";

/// The capability whose holder decides the held operation of an agent's call when no hold rule
/// names another approver (ADM-1606).
pub const AGENT_APPROVE: &str = "agent-approve";

/// The capabilities that are administration powers, which no agent holds (D-8424): defining holds,
/// settings, automations, segments, ledger declarations and products, and deciding what is held.
pub const ADMIN_CAPABILITIES: &[&str] = &[
    HOLDS_DEFINE,
    crate::settings::SETTING_DEFINE,
    crate::products::BUNDLE_INSTALL,
    crate::definitions::AUTOMATIONS_DEFINE,
    crate::segments::SEGMENTS_DEFINE,
    crate::ledger::LEDGER_CONFIGURE,
    crate::approvals::WORKFLOW_APPROVE,
    AGENT_APPROVE,
];

/// The agent a principal id names, when it starts with the reserved prefix.
pub fn agent_named(id: &PrincipalId) -> Option<&str> {
    id.as_str().strip_prefix(AGENT_PREFIX)
}

/// What the gate decided about a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// The call goes on to be decided as a person's call is.
    Pass,
    /// The call is not applied; it is filed as a held operation for `approver`.
    Hold {
        /// The capability its approver needs.
        approver: CapabilityName,
    },
}

fn refuse(evidence: &[Check], kind: ErrorKind, rule: &str, message: String) -> DomainError {
    let mut checks = evidence.to_vec();
    checks.push(Check::fail(rule, message.clone()));
    DomainError::new(kind, message).with(checks)
}

/// The scope of a token that is not one of the four record actions, or `None`: such a scope is an
/// administration power the token cannot carry (ADM-1607).
fn admin_scope(scopes: &[String]) -> Option<&str> {
    scopes
        .iter()
        .map(String::as_str)
        .find(|scope| match scope.split_once(':') {
            Some((action, object)) => {
                !matches!(action, "read" | "create" | "update" | "delete") || object.is_empty()
            }
            None => true,
        })
}

/// The roles a principal holds for this module: those assigned to it directly or through a team,
/// and those it holds narrowed by a scope (a narrower scope is still a power on the records it
/// reaches).
fn roles_held(
    members: &BTreeMap<PrincipalId, BTreeSet<RoleId>>,
    schema: &TenantSchema,
    principal: &PrincipalId,
) -> BTreeSet<RoleId> {
    let mut roles = assigned_roles(members, schema, principal);
    roles.extend(crate::scoped::of(schema, principal).map(|scoped| scoped.role.clone()));
    roles
}

/// The administration powers a principal holds, each as a short phrase: the owner role, a policy
/// of one of its roles that manages members or the schema, and a capability of
/// [`ADMIN_CAPABILITIES`] one of its roles names.
pub fn admin_powers(
    members: &BTreeMap<PrincipalId, BTreeSet<RoleId>>,
    schema: &TenantSchema,
    principal: &PrincipalId,
) -> BTreeSet<String> {
    let roles = roles_held(members, schema, principal);
    let mut powers = BTreeSet::new();
    if roles.contains(&owner_role()) {
        powers.insert("the owner role".to_string());
    }
    for role in &roles {
        for capability in role_capabilities(schema, role) {
            if ADMIN_CAPABILITIES.contains(&capability.as_str()) {
                powers.insert(format!("capability {capability}"));
            }
        }
    }
    for policy in schema.policies.values().filter(|p| roles.contains(&p.role)) {
        if policy.actions.contains(&Action::Manage) {
            match policy.resource {
                Resource::Members => powers.insert("manage members".to_string()),
                Resource::Schema => powers.insert("manage the schema".to_string()),
                _ => false,
            };
        }
    }
    powers
}

/// The principals that are agents among those that hold anything: members, team members and
/// holders of a scoped role.
fn agents_of(
    members: &BTreeMap<PrincipalId, BTreeSet<RoleId>>,
    schema: &TenantSchema,
) -> BTreeSet<PrincipalId> {
    let mut all: BTreeSet<&PrincipalId> = members.keys().collect();
    for team in schema.all_teams() {
        all.extend(team.members.iter());
    }
    all.extend(schema.scoped_roles.iter().flatten().map(|s| &s.principal));
    all.into_iter()
        .filter(|id| agent_named(id).is_some())
        .cloned()
        .collect()
}

/// The first administration power the state after a change gives an agent that the state before
/// it did not, as a sentence, or `None` (ADM-1607). A power an agent already held is not made
/// worse by an unrelated change; the gate refuses the agent's own calls while it holds one.
pub fn made_by(
    before: (&BTreeMap<PrincipalId, BTreeSet<RoleId>>, &TenantSchema),
    after: (&BTreeMap<PrincipalId, BTreeSet<RoleId>>, &TenantSchema),
) -> Option<String> {
    for agent in agents_of(after.0, after.1) {
        let held = admin_powers(before.0, before.1, &agent);
        if let Some(power) = admin_powers(after.0, after.1, &agent)
            .into_iter()
            .find(|power| !held.contains(power))
        {
            return Some(format!(
                "the agent {agent} would hold an administration power: {power}"
            ));
        }
    }
    None
}

/// What an operation does for an agent. A tenant operation declares it and is `deny` when it
/// declares none; the other intents have the platform's table (ADM-1606).
pub fn policy_of(tenant: &TenantState, intent: &Intent) -> AgentPolicy {
    match intent {
        Intent::InvokeOperation { operation, .. } => tenant
            .schema
            .operations
            .as_ref()
            .and_then(|ops| ops.get(operation))
            .and_then(|op| op.agent_policy)
            .unwrap_or(AgentPolicy::Deny),
        Intent::CreateRecord { .. } | Intent::UpdateRecord { .. } | Intent::DeleteRecord { .. } => {
            AgentPolicy::Direct
        }
        // Site, programme and appearance settings are held for a human (ADM-1608).
        Intent::WriteSetting { .. } => AgentPolicy::Approval,
        _ => AgentPolicy::Deny,
    }
}

/// Whether the scopes of a token cover `action` on `object`: the same scope, or the action on `*`.
fn scope_covers(scopes: &[String], action: &str, object: Option<&ObjectTypeId>) -> bool {
    scopes.iter().any(|scope| {
        scope.split_once(':').is_some_and(|(a, o)| {
            a == action && (o == "*" || object.is_some_and(|object| o == object.as_str()))
        })
    })
}

/// Whether the scopes hold some scope that writes, on whatever object.
fn scope_writes(scopes: &[String]) -> bool {
    scopes.iter().any(|scope| {
        scope
            .split_once(':')
            .is_some_and(|(action, _)| matches!(action, "create" | "update" | "delete"))
    })
}

/// The capability a write of a setting at a level needs.
fn setting_capability(level: SettingLevel) -> Option<&'static str> {
    match level {
        SettingLevel::Platform => Some(crate::settings::SETTING_WRITE_PLATFORM),
        SettingLevel::Tenant => Some(crate::settings::SETTING_WRITE_TENANT),
        SettingLevel::Team => Some(crate::settings::SETTING_WRITE_TEAM),
        SettingLevel::Person => None,
    }
}

/// Whether the agent's own roles let it invoke an operation: the invokers name every signed-in
/// person or one of the agent's roles.
fn may_invoke(tenant: &TenantState, agent: &Principal, op: &TenantOperationDef) -> bool {
    op.invokers.signed_in || !op.invokers.roles.is_disjoint(&roles_of(tenant, agent))
}

/// Why a call is outside the ceiling of `agent`, ignoring the scope of any token: the sentence, or
/// `None` when the agent's own roles allow it. The first term of the intersection of ADM-1605.
pub fn ceiling_problem(
    tenant: &TenantState,
    agent: &PrincipalId,
    intent: &Intent,
) -> Option<String> {
    let who = Principal::User(agent.clone());
    match intent {
        Intent::CreateRecord { object, .. } => (!may_attempt(
            tenant,
            &who,
            Action::Create,
            &Resource::Object(object.clone()),
        ))
        .then(|| format!("the ceiling of {agent} does not allow creating a record of {object}")),
        Intent::UpdateRecord { object, .. } => (!may_attempt(
            tenant,
            &who,
            Action::Update,
            &Resource::Object(object.clone()),
        ))
        .then(|| format!("the ceiling of {agent} does not allow updating a record of {object}")),
        Intent::DeleteRecord { object, .. } => (!may_attempt(
            tenant,
            &who,
            Action::Delete,
            &Resource::Object(object.clone()),
        ))
        .then(|| format!("the ceiling of {agent} does not allow deleting a record of {object}")),
        Intent::InvokeOperation { operation, .. } => {
            let op = tenant
                .schema
                .operations
                .as_ref()
                .and_then(|ops| ops.get(operation));
            match op {
                Some(op) if may_invoke(tenant, &who, op) => None,
                _ => Some(format!(
                    "the ceiling of {agent} does not allow invoking operation {operation}"
                )),
            }
        }
        Intent::WriteSetting { level, .. } => setting_capability(*level).and_then(|name| {
            let wanted = BTreeSet::from([Grant::Capability(name.to_string())]);
            (!holds_any(tenant, &who, &wanted))
                .then(|| format!("the ceiling of {agent} does not hold the capability {name}"))
        }),
        _ => Some(format!(
            "the ceiling of {agent} allows no {}",
            crate::intent::IntentExt::name(intent)
        )),
    }
}

/// Why a call is outside the scope of the token, or `None`: the second term of the intersection.
fn scope_problem(tenant: &TenantState, scopes: &[String], intent: &Intent) -> Option<String> {
    let (covered, what) = match intent {
        Intent::CreateRecord { object, .. } => (
            scope_covers(scopes, "create", Some(object)),
            format!("create:{object}"),
        ),
        Intent::UpdateRecord { object, .. } => (
            scope_covers(scopes, "update", Some(object)),
            format!("update:{object}"),
        ),
        Intent::DeleteRecord { object, .. } => (
            scope_covers(scopes, "delete", Some(object)),
            format!("delete:{object}"),
        ),
        Intent::InvokeOperation { operation, .. } => {
            let subject = tenant
                .schema
                .operations
                .as_ref()
                .and_then(|ops| ops.get(operation))
                .and_then(|op| op.subject.as_ref())
                .map(|subject| &subject.object);
            let covered = match subject {
                Some(object) => {
                    scope_covers(scopes, "update", Some(object)) || scope_writes(scopes)
                }
                None => scope_writes(scopes),
            };
            (covered, "a scope that writes".to_string())
        }
        Intent::WriteSetting { .. } => (scope_writes(scopes), "a scope that writes".into()),
        _ => (false, "no scope".into()),
    };
    (!covered).then(|| format!("the scope of the token does not hold {what}"))
}

/// The approver of a call whose policy is approval: the one the first hold rule that names the
/// change gives, else [`AGENT_APPROVE`].
fn approver_of(tenant: &TenantState, ctx: &Context, change: &HeldChange) -> CapabilityName {
    crate::holds::rule_holding(tenant, ctx, change)
        .map(|rule| rule.approver.clone())
        .unwrap_or_else(|| CapabilityName(AGENT_APPROVE.to_string()))
}

/// The gate every call of an agent passes before it is decided (ADM-1605 to ADM-1607).
pub fn gate(
    tenant: &TenantState,
    ctx: &Context,
    call: &AgentCall,
    intent: &Intent,
) -> Result<Gate, DomainError> {
    let mut evidence: Vec<Check> = Vec::new();
    let Principal::User(person) = &ctx.principal else {
        return Err(refuse(
            &evidence,
            ErrorKind::AgentCeilingDenies,
            "agent:acts_for_a_person",
            "an agent acts only for a signed-in person".into(),
        ));
    };
    if agent_named(&call.agent).is_none() {
        return Err(refuse(
            &evidence,
            ErrorKind::AgentCeilingDenies,
            "agent:is_an_agent_principal",
            format!("{} is not an agent principal id", call.agent),
        ));
    }
    evidence.push(Check::pass(
        "agent:acts_for",
        format!("{} for {person}", call.agent),
    ));

    // 1. No administration power, in the scope of the token or in the roles of the agent.
    if let Some(scope) = admin_scope(&call.scopes) {
        return Err(refuse(
            &evidence,
            ErrorKind::AgentAdminPower,
            "agent:no_admin_scope",
            format!(
                "the token of {} names the scope {scope}, which is no record scope",
                call.agent
            ),
        ));
    }
    let powers = admin_powers(&tenant.members, &tenant.schema, &call.agent);
    if let Some(power) = powers.iter().next() {
        return Err(refuse(
            &evidence,
            ErrorKind::AgentAdminPower,
            "agent:no_admin_power",
            format!(
                "the agent {} holds an administration power: {power}",
                call.agent
            ),
        ));
    }
    evidence.push(Check::pass("agent:no_admin_power", call.agent.as_str()));

    // 2. The per-operation policy.
    let policy = policy_of(tenant, intent);
    if policy == AgentPolicy::Deny {
        return Err(refuse(
            &evidence,
            ErrorKind::AgentDenied,
            "agent:policy",
            format!(
                "the agent policy of {} is deny",
                crate::intent::IntentExt::name(intent)
            ),
        ));
    }
    evidence.push(Check::pass("agent:policy", format!("{policy:?}")));

    // 3. The ceiling, and the scope of the token. The grants of the person are asked by the
    // decision itself, as they are of any call.
    if let Some(problem) = ceiling_problem(tenant, &call.agent, intent) {
        return Err(refuse(
            &evidence,
            ErrorKind::AgentCeilingDenies,
            "agent:ceiling",
            problem,
        ));
    }
    if let Some(problem) = scope_problem(tenant, &call.scopes, intent) {
        return Err(refuse(
            &evidence,
            ErrorKind::AgentCeilingDenies,
            "agent:token_scope",
            problem,
        ));
    }
    if policy == AgentPolicy::Approval {
        let Some(change) = crate::holds::change_of(intent) else {
            return Err(refuse(
                &evidence,
                ErrorKind::AgentDenied,
                "agent:policy",
                "this call cannot be held for approval".into(),
            ));
        };
        return Ok(Gate::Hold {
            approver: approver_of(tenant, ctx, &change),
        });
    }
    Ok(Gate::Pass)
}

/// Why a held change of an agent no longer applies: the agent now holds an administration power,
/// or its ceiling no longer allows the change. Asked again when the hold is approved.
pub fn recheck(tenant: &TenantState, agent: &PrincipalId, intent: &Intent) -> Option<String> {
    admin_powers(&tenant.members, &tenant.schema, agent)
        .into_iter()
        .next()
        .map(|power| format!("the agent {agent} holds an administration power: {power}"))
        .or_else(|| ceiling_problem(tenant, agent, intent))
}

/// The operations an agent may see, with the policy of each: those whose policy is not `deny`
/// and that the agent's own roles let it invoke (ADM-1606). An agent that holds an administration
/// power sees none.
pub fn visible_operations(
    tenant: &TenantState,
    agent: &PrincipalId,
) -> Vec<(TenantOperationId, AgentPolicy)> {
    if !admin_powers(&tenant.members, &tenant.schema, agent).is_empty() {
        return Vec::new();
    }
    let who = Principal::User(agent.clone());
    tenant
        .schema
        .operations
        .iter()
        .flatten()
        .filter_map(|(id, op)| {
            let policy = op.agent_policy.unwrap_or(AgentPolicy::Deny);
            (policy != AgentPolicy::Deny && may_invoke(tenant, &who, op))
                .then(|| (id.clone(), policy))
        })
        .collect()
}
