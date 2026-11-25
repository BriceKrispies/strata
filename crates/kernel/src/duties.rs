//! Duties kept apart (D-8405, D-8426, D-8482, ADM-1630, ADM-1642).
//!
//! A separation names two capabilities that no principal holds together, whichever roles carry
//! them: the one who may post an adjustment to the ledger is not the one who reviews it. The
//! platform's own separations are the table [`SEPARATIONS`], a default every tenant has; a tenant
//! or a product declares more in the schema (`separations`, the schema changes
//! `define-separation` and `remove-separation`), and a declaration adds to the default and never
//! replaces or removes it. The kernel holds one rule for every path that can give a principal a
//! role (a direct assignment, a scoped assignment, a team's role, a person added to a team, an
//! invitation, a change of what a role grants, a change of the separations themselves), which
//! compares the pairs held before the change with the pairs held after it and refuses a pair the
//! change makes (`ConflictingRoles`). A pair a tenant already holds is not made worse by an
//! unrelated change, so it is not held to this, as with the last owner (ADM-1081); defining a
//! separation that someone already breaks makes a pair and is refused.
//!
//! The built-in `owner` role holds every capability by definition and is not counted: a tenant
//! always has one, and the owner is the one role the platform cannot ask to give a duty up.

use crate::ledger::{LEDGER_POST, LEDGER_READ};
use crate::rules::{assigned_roles, role_capabilities};
use crate::schema::{TenantSchema, TenantSchemaExt};
use crate::types::{
    owner_role, CapabilityName, PrincipalId, RoleId, SchemaChange, SeparationDef, SeparationId,
};
use std::collections::{BTreeMap, BTreeSet};

/// The capability that defines and removes the separations of a tenant (ADM-1642).
pub const SEPARATIONS_DEFINE: &str = "separations-define";

/// One separation of duties of the platform: two capabilities no principal holds together.
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
/// with the capability that reads every account of it. This is the default of every tenant.
pub const SEPARATIONS: &[Separation] = &[Separation {
    id: "ledger-adjust-and-review",
    left_duty: "posts ledger adjustments",
    left: LEDGER_POST,
    right_duty: "reviews the ledger",
    right: LEDGER_READ,
}];

/// The most separations a tenant declares.
pub const MAX_SEPARATIONS: usize = 200;

/// The separations in force in a schema: the platform's, then those the schema declares in id
/// order.
pub fn effective(schema: &TenantSchema) -> Vec<SeparationDef> {
    let mut all: Vec<SeparationDef> = SEPARATIONS
        .iter()
        .filter_map(|s| {
            Some(SeparationDef {
                id: SeparationId::new(s.id).ok()?,
                left: CapabilityName(s.left.to_string()),
                left_duty: s.left_duty.to_string(),
                right: CapabilityName(s.right.to_string()),
                right_duty: s.right_duty.to_string(),
            })
        })
        .collect();
    all.extend(
        schema
            .separations
            .iter()
            .flatten()
            .map(|(_, def)| def.clone()),
    );
    all
}

/// Whether a schema change defines or removes a separation of duties.
pub fn is_separation_change(change: &SchemaChange) -> bool {
    matches!(
        change,
        SchemaChange::DefineSeparation(_) | SchemaChange::RemoveSeparation(_)
    )
}

/// Define (or replace) or remove a declared separation on a working copy of the schema, and
/// describe it. A platform separation is not declared again and is never removed.
pub(crate) fn apply(schema: &mut TenantSchema, change: &SchemaChange) -> Result<String, String> {
    match change {
        SchemaChange::DefineSeparation(def) => {
            if SEPARATIONS.iter().any(|s| s.id == def.id.as_str()) {
                return Err(format!(
                    "{} is a separation of the platform, which a tenant does not declare again",
                    def.id
                ));
            }
            schema
                .separations
                .get_or_insert_with(BTreeMap::new)
                .insert(def.id.clone(), def.clone());
            Ok(format!("define separation {}", def.id))
        }
        SchemaChange::RemoveSeparation(id) => {
            if SEPARATIONS.iter().any(|s| s.id == id.as_str()) {
                return Err(format!(
                    "{id} is a separation of the platform, which is not removed"
                ));
            }
            let held = schema
                .separations
                .as_mut()
                .ok_or_else(|| format!("no separation {id}"))?;
            held.remove(id)
                .ok_or_else(|| format!("no separation {id}"))?;
            if held.is_empty() {
                schema.separations = None;
            }
            Ok(format!("remove separation {id}"))
        }
        _ => Err("not a change of a separation".to_string()),
    }
}

/// The problems of the separations a schema declares: the key is the id and not one of the
/// platform's, the two capabilities differ and are named, each duty is said, and a capability of
/// the product form is declared in the tenant.
pub fn problems(schema: &TenantSchema, problems: &mut Vec<String>) {
    let Some(declared) = &schema.separations else {
        return;
    };
    if declared.len() > MAX_SEPARATIONS {
        problems.push(format!(
            "limit: {} separations is more than the maximum of {MAX_SEPARATIONS}",
            declared.len()
        ));
    }
    for (key, def) in declared {
        let at = format!("separation {key}");
        if key != &def.id {
            problems.push(format!("{at}: the key is not the id {}", def.id));
        }
        if SEPARATIONS.iter().any(|s| s.id == key.as_str()) {
            problems.push(format!("{at}: the id is a separation of the platform"));
        }
        if def.left == def.right {
            problems.push(format!("{at}: it names the capability {} twice", def.left));
        }
        for (side, capability, duty) in [
            ("left", &def.left, &def.left_duty),
            ("right", &def.right, &def.right_duty),
        ] {
            if capability.as_str().trim().is_empty() {
                problems.push(format!("{at}: the {side} capability needs a name"));
            } else if crate::capabilities::is_product_form(capability.as_str())
                && !crate::capabilities::is_declared(schema, capability.as_str())
            {
                problems.push(format!(
                    "{at}: capability {capability} is neither a platform capability nor declared in the tenant"
                ));
            }
            if duty.trim().is_empty() {
                problems.push(format!("{at}: the {side} duty needs a sentence"));
            }
        }
    }
}

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

/// A pair of duties a principal holds together: the principal and the separation, by its id and
/// the two capabilities it names, so that a separation redefined to name others is another pair.
type Pair = (PrincipalId, String, String, String);

/// Every pair of duties a principal holds together.
pub fn conflicts(
    members: &BTreeMap<PrincipalId, BTreeSet<RoleId>>,
    schema: &TenantSchema,
) -> BTreeSet<Pair> {
    let separations = effective(schema);
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
        for separation in &separations {
            if capabilities.contains(separation.left.as_str())
                && capabilities.contains(separation.right.as_str())
            {
                found.insert((
                    principal.clone(),
                    separation.id.to_string(),
                    separation.left.to_string(),
                    separation.right.to_string(),
                ));
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
    let (principal, id, left, right) = conflicts(after.0, after.1)
        .into_iter()
        .find(|pair| !held.contains(pair))?;
    let separation = effective(after.1)
        .into_iter()
        .find(|s| s.id.as_str() == id && s.left.as_str() == left && s.right.as_str() == right)?;
    Some(format!(
        "{principal} would hold the role that {} and the role that {}",
        separation.left_duty, separation.right_duty
    ))
}
