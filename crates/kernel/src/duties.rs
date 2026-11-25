//! Duties the platform keeps apart (D-8405, D-8426, ADM-1630).
//!
//! A separation names two platform capabilities that no principal holds together, whichever roles
//! carry them: the one who may post an adjustment to the ledger is not the one who reviews it.
//! The table below is the platform's; the kernel holds one rule for every path that can give a
//! principal a role (a direct assignment, a scoped assignment, a team's role, a person added to a
//! team, an invitation, a change of what a role grants), which compares the pairs held before the
//! change with the pairs held after it and refuses a pair the change makes (`ConflictingRoles`).
//! A pair a tenant already holds is not made worse by an unrelated change, so it is not held to
//! this, as with the last owner (ADM-1081).
//!
//! The built-in `owner` role holds every capability by definition and is not counted: a tenant
//! always has one, and the owner is the one role the platform cannot ask to give a duty up.

use crate::ledger::{LEDGER_POST, LEDGER_READ};
use crate::rules::{assigned_roles, role_capabilities};
use crate::schema::{TenantSchema, TenantSchemaExt};
use crate::types::{owner_role, PrincipalId, RoleId};
use std::collections::{BTreeMap, BTreeSet};

/// One separation of duties: two capabilities no principal holds together.
pub struct Separation {
    /// The name the refusal and the audit give the separation.
    pub id: &'static str,
    /// What the first capability lets a principal do.
    pub left_duty: &'static str,
    /// The first capability.
    pub left: &'static str,
    /// What the second capability lets a principal do.
    pub right_duty: &'static str,
    /// The second capability.
    pub right: &'static str,
}

/// The platform's separations (D-8426): the capability that posts to the ledger is never held
/// with the capability that reads every account of it.
pub const SEPARATIONS: &[Separation] = &[Separation {
    id: "ledger-adjust-and-review",
    left_duty: "posts ledger adjustments",
    left: LEDGER_POST,
    right_duty: "reviews the ledger",
    right: LEDGER_READ,
}];

/// The roles a principal holds for this rule: those it is assigned directly or through a team,
/// and those it holds narrowed by a scope (a narrower scope still lets the person do the duty on
/// the records it reaches).
fn roles_held(
    members: &BTreeMap<PrincipalId, BTreeSet<RoleId>>,
    schema: &TenantSchema,
    principal: &PrincipalId,
) -> BTreeSet<RoleId> {
    let mut roles = assigned_roles(members, schema, principal);
    roles.extend(crate::scoped::of(schema, principal).map(|scoped| scoped.role.clone()));
    roles
}

/// Every pair of duties a principal holds together, as `(principal, separation id)`.
pub fn conflicts(
    members: &BTreeMap<PrincipalId, BTreeSet<RoleId>>,
    schema: &TenantSchema,
) -> BTreeSet<(PrincipalId, &'static str)> {
    let mut people: BTreeSet<&PrincipalId> = members.keys().collect();
    for team in schema.all_teams() {
        people.extend(team.members.iter());
    }
    people.extend(schema.scoped_roles.iter().flatten().map(|s| &s.principal));
    let mut found = BTreeSet::new();
    for principal in people {
        let roles = roles_held(members, schema, principal);
        if roles.contains(&owner_role()) {
            continue;
        }
        let capabilities: BTreeSet<String> = roles
            .iter()
            .flat_map(|role| role_capabilities(schema, role))
            .collect();
        for separation in SEPARATIONS {
            if capabilities.contains(separation.left) && capabilities.contains(separation.right) {
                found.insert((principal.clone(), separation.id));
            }
        }
    }
    found
}

/// The first pair of duties the state after a change holds that the state before it did not, as a
/// sentence, or `None`.
pub fn made_by(
    before: (&BTreeMap<PrincipalId, BTreeSet<RoleId>>, &TenantSchema),
    after: (&BTreeMap<PrincipalId, BTreeSet<RoleId>>, &TenantSchema),
) -> Option<String> {
    let held = conflicts(before.0, before.1);
    let (principal, id) = conflicts(after.0, after.1)
        .into_iter()
        .find(|pair| !held.contains(pair))?;
    let separation = SEPARATIONS.iter().find(|s| s.id == id)?;
    Some(format!(
        "{principal} would hold the role that {} and the role that {}",
        separation.left_duty, separation.right_duty
    ))
}
