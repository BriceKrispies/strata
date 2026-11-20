//! Deciding the approval of a waiting workflow step (contract approve-workflow-step, WFL-006,
//! WFL-2680 to WFL-2683, D-7680).
//!
//! A request-approval step stops its run until a principal the step's policy names decides. The
//! policy names a role (`PolicyDef::role`): the principals who hold that role may decide, and so
//! may the owner, who holds every power. The decision is made here, purely, from the schema and
//! members of the tenant, the approval as it was requested and the time:
//!
//! 1. The approver is a signed-in person. An automation principal never decides an approval: it
//!    would be approving its own work.
//! 2. The approver holds the capability workflow-approve, which every role that may approve
//!    holds (the owner holds it by being the owner).
//! 3. The approver holds a role the policy of the step names.
//! 4. The approver is not the person whose action started the run (WFL-2681): an approval is a
//!    second person's check, as AGS-409 states for a proposal. A run that no person started has
//!    no such person.
//! 5. The deadline has not passed. After it the step takes its refused branch and nothing else.
//!
//! What happens to the run afterwards, and recording the decision once, is the automation
//! worker's: the step record keeps the decision, so a second one finds the step decided.

use crate::automation::automation_named;
use crate::rules::{holds_any, roles_of};
use crate::schema::{Grant, TenantSchema};
use crate::state::{Principal, TenantState};
use crate::types::{owner_role, PolicyId, PrincipalId, RoleId, Timestamp};
use std::collections::BTreeSet;

/// The capability that approving a waiting step needs, besides the role the policy names.
pub const WORKFLOW_APPROVE: &str = "workflow-approve";

/// Why a decision on an approval is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The caller is not a signed-in person (anonymous, the system or an automation principal).
    NotAPerson,
    /// The caller does not hold workflow-approve.
    MissingCapability,
    /// The caller holds none of the roles the policy of the step names.
    NotNamedByPolicy { roles: BTreeSet<RoleId> },
    /// The caller is the person whose action started the run (WFL-2681).
    SeparationOfDuties,
    /// The deadline has passed: the step takes its refused branch (WFL-2682).
    PastDeadline,
}

impl Refusal {
    /// The refusal as a sentence for the caller.
    pub fn describe(&self) -> String {
        match self {
            Refusal::NotAPerson => "only a signed-in person decides an approval".to_string(),
            Refusal::MissingCapability => {
                format!("deciding an approval needs the permission {WORKFLOW_APPROVE}")
            }
            Refusal::NotNamedByPolicy { roles } => {
                let names: Vec<&str> = roles.iter().map(RoleId::as_str).collect();
                format!(
                    "the policy of this step names the role {}, which the caller does not hold",
                    names.join(", ")
                )
            }
            Refusal::SeparationOfDuties => {
                "the person who started the run may not approve it".to_string()
            }
            Refusal::PastDeadline => {
                "the deadline of this approval has passed; the step takes its refused branch"
                    .to_string()
            }
        }
    }

    /// Whether the caller is refused for what they are (forbidden) and not for the state of the
    /// approval (a conflict).
    pub fn forbidden(&self) -> bool {
        !matches!(self, Refusal::PastDeadline)
    }
}

/// The roles whose holders the policy names, or `None` when the schema holds no such policy.
pub fn policy_roles(schema: &TenantSchema, policy: &PolicyId) -> Option<BTreeSet<RoleId>> {
    schema
        .policies
        .get(policy)
        .map(|def| BTreeSet::from([def.role.clone()]))
}

/// Decide whether `approver` may decide an approval that names `roles`, was started by
/// `started_by` and is due at `deadline`, at time `now`. The approver's id is returned for the
/// record of the decision.
pub fn decide(
    tenant: &TenantState,
    approver: &Principal,
    roles: &BTreeSet<RoleId>,
    started_by: Option<&PrincipalId>,
    deadline: Timestamp,
    now: Timestamp,
) -> Result<PrincipalId, Refusal> {
    let Principal::User(who) = approver else {
        return Err(Refusal::NotAPerson);
    };
    if automation_named(who).is_some() {
        return Err(Refusal::NotAPerson);
    }
    let wanted = BTreeSet::from([Grant::Capability(WORKFLOW_APPROVE.to_string())]);
    if !holds_any(tenant, approver, &wanted) {
        return Err(Refusal::MissingCapability);
    }
    let held = roles_of(tenant, approver);
    let owner = held.contains(&owner_role());
    if !owner && held.is_disjoint(roles) {
        return Err(Refusal::NotNamedByPolicy {
            roles: roles.clone(),
        });
    }
    if started_by == Some(who) {
        return Err(Refusal::SeparationOfDuties);
    }
    if now > deadline {
        return Err(Refusal::PastDeadline);
    }
    Ok(who.clone())
}
