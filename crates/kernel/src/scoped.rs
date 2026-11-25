//! Role assignments narrowed by a scope predicate over the target record (D-8403, ADM-1611).
//!
//! A scoped assignment gives a principal a role on the records of one object type where a
//! predicate holds, such as "the region of the record is the region of its member". It is held in
//! [`TenantSchema::scoped_roles`], changes through the intents `AssignScopedRole` and
//! `RevokeRole`, and does not raise the schema version.
//!
//! **A narrower scope never confers more.** A scoped assignment is not a direct assignment: it
//! adds nothing to [`crate::rules::roles_of`], so every place that reads the roles of a principal
//! (the owner check, the capabilities a role names, the powers a principal holds when it confers a
//! role or widens one, the audience of a read) sees none of it. The one place that reads it is
//! [`crate::rules::authorize_needing`], which grants the permissions and policies of the role on
//! a record of the scope's object type only where the predicate holds, with the predicate ANDed
//! into the condition of each policy. The role-widening check of a schema change
//! ([`crate::transition`], `require_powers_held`, ADM-1088) therefore asks as much of a principal
//! who holds a role only through a scope as of one who holds none of it.
//!
//! The predicate is checked when the assignment is made, against the fields of the object type
//! (a field the type does not declare is `ScopeUnknownField`), and again with the rest of the
//! schema, so a change that removes a field a scope reads is refused.

use crate::expr::{self, Expr, TypeEnv};
use crate::intent::ErrorKind;
use crate::schema::TenantSchema;
use crate::types::{ObjectTypeId, PrincipalId, RoleId, ScopedRole};
use std::collections::{BTreeMap, BTreeSet};

/// The most scoped assignments a tenant holds.
pub const MAX_SCOPED_ROLES: usize = 10_000;

/// The scoped assignments of one principal.
pub fn of<'a>(
    schema: &'a TenantSchema,
    principal: &'a PrincipalId,
) -> impl Iterator<Item = &'a ScopedRole> {
    schema
        .scoped_roles
        .iter()
        .flatten()
        .filter(move |held| held.principal == *principal)
}

/// Why a predicate is not one over the records of `object`: the error kind and the message. A
/// field the object type does not declare is [`ErrorKind::ScopeUnknownField`]; any other fault
/// of the predicate, or an object type that does not exist, is [`ErrorKind::InvalidIntent`] and
/// [`ErrorKind::UnknownObject`].
pub fn predicate_problem(
    schema: &TenantSchema,
    object: &ObjectTypeId,
    scope: &Expr,
) -> Option<(ErrorKind, String)> {
    let Some(def) = schema.objects.get(object) else {
        return Some((ErrorKind::UnknownObject, format!("no object type {object}")));
    };
    let roles: BTreeSet<RoleId> = schema.roles.keys().cloned().collect();
    let no_params = BTreeSet::new();
    let env = TypeEnv {
        fields: &def.fields,
        params: &no_params,
        roles: &roles,
        objects: &schema.objects,
    };
    match expr::check(scope, &env) {
        Ok(expr::Shape::Bool) => None,
        Ok(other) => Some((
            ErrorKind::InvalidIntent,
            format!("a scope predicate is a truth value, not {other:?}"),
        )),
        Err(problem) if problem.contains("unknown field") => Some((
            ErrorKind::ScopeUnknownField,
            format!("the scope names a field {object} does not declare: {problem}"),
        )),
        Err(problem) => Some((ErrorKind::InvalidIntent, format!("the scope: {problem}"))),
    }
}

/// The problems of the scoped assignments of a schema, pushed to `problems`: a role and an object
/// type that exist, a role that is neither `public` nor `owner`, a predicate that checks, and no
/// two assignments of one role to one person on one object type.
pub fn problems(schema: &TenantSchema, problems: &mut Vec<String>) {
    let Some(held) = &schema.scoped_roles else {
        return;
    };
    if held.len() > MAX_SCOPED_ROLES {
        problems.push(format!(
            "limit: {} scoped role assignments is more than the maximum of {MAX_SCOPED_ROLES}",
            held.len()
        ));
    }
    let mut seen: BTreeMap<(&PrincipalId, &RoleId, &ObjectTypeId), usize> = BTreeMap::new();
    for assignment in held {
        let at = format!(
            "scoped role {} of {} on {}",
            assignment.role, assignment.principal, assignment.object
        );
        if !schema.roles.contains_key(&assignment.role) {
            problems.push(format!("{at}: unknown role"));
        }
        if assignment.role == crate::types::owner_role()
            || assignment.role == crate::types::public_role()
        {
            problems.push(format!("{at}: the role is not narrowed by a scope"));
        }
        if let Some((_, message)) = predicate_problem(schema, &assignment.object, &assignment.scope)
        {
            problems.push(format!("{at}: {message}"));
        }
        *seen
            .entry((&assignment.principal, &assignment.role, &assignment.object))
            .or_default() += 1;
    }
    for ((principal, role, object), count) in seen {
        if count > 1 {
            problems.push(format!(
                "scoped role {role} of {principal} on {object} is assigned {count} times"
            ));
        }
    }
}

/// The assignments left after `principal` loses `role` (every object type), and whether any went.
pub fn without(
    schema: &TenantSchema,
    principal: &PrincipalId,
    role: &RoleId,
) -> (Option<Vec<ScopedRole>>, bool) {
    let Some(held) = &schema.scoped_roles else {
        return (None, false);
    };
    let kept: Vec<ScopedRole> = held
        .iter()
        .filter(|a| !(a.principal == *principal && a.role == *role))
        .cloned()
        .collect();
    let changed = kept.len() != held.len();
    ((!kept.is_empty()).then_some(kept), changed)
}
