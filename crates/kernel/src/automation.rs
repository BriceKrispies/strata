//! The automation principal (D-6525, WFL-1527, WG-5098).
//!
//! Every intent of an automated step is decided with the principal `automation:<workflow-id>`,
//! which holds exactly the roles of the automation's run-as and no other: not `public`, not a
//! role of the table of members, not a role of a team. The principal of an automation the schema
//! no longer holds holds no role, so a step of a removed automation is refused like any other
//! intent of a principal without authority.
//!
//! The prefix is reserved: a person may not be the owner of a new tenant, be assigned a role or
//! be added to a team under an id that starts with it, and a caller with a session (a person)
//! may not act under one. The automation worker acts under it without a session.

use crate::types::{Principal, PrincipalId, RoleId, TenantSchema, WorkflowId};
use std::collections::BTreeSet;

/// The prefix of the principal an automation acts as.
pub const AUTOMATION_PREFIX: &str = "automation:";

/// The principal an automation acts as: `automation:<workflow-id>`.
pub fn automation_principal(automation: &WorkflowId) -> PrincipalId {
    PrincipalId::new(format!("{AUTOMATION_PREFIX}{automation}"))
        .expect("a workflow id is a valid principal token")
}

/// The automation a principal id names, when it starts with the reserved prefix.
pub fn automation_named(id: &PrincipalId) -> Option<&str> {
    id.as_str().strip_prefix(AUTOMATION_PREFIX)
}

/// The roles of an automation principal under `schema`: exactly the run-as roles of its
/// automation, and none when the schema holds no such automation. `None` for any other principal.
pub fn automation_roles(schema: &TenantSchema, id: &PrincipalId) -> Option<BTreeSet<RoleId>> {
    let name = automation_named(id)?;
    Some(
        WorkflowId::new(name)
            .ok()
            .and_then(|automation| {
                schema
                    .automations
                    .as_ref()
                    .and_then(|all| all.get(&automation))
            })
            .map(|def| def.run_as.roles.clone())
            .unwrap_or_default(),
    )
}

/// Why an intent names the automation prefix where a person is meant, or `None`: `named` is the
/// owner of a new tenant, or the principal a role is assigned to or that is added to a team; the
/// caller is refused when it has a session, since an automation acts without one.
pub fn reserved_problem(
    named: Option<&PrincipalId>,
    caller: &Principal,
    has_session: bool,
) -> Option<String> {
    if let Some(id) = named.filter(|id| automation_named(id).is_some()) {
        return Some(format!(
            "the principal id {id} starts with the reserved prefix {AUTOMATION_PREFIX}"
        ));
    }
    match caller {
        Principal::User(id) if has_session && automation_named(id).is_some() => Some(format!(
            "the principal id {id} starts with the reserved prefix {AUTOMATION_PREFIX}, which no signed-in person may hold"
        )),
        _ => None,
    }
}
