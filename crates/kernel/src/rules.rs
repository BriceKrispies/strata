//! Guarded state access, authorization and record validation. These are the rules every
//! transition and read is built from.

use crate::expr::{self, Env};
use crate::ids::{
    owner_role, public_role, FieldId, ObjectTypeId, PrincipalId, RecordId, RoleId, Timestamp,
};
use crate::intent::{Check, DomainError, ErrorKind};
use crate::schema::{Action, Grant, ObjectTypeDef, Resource, TenantSchema, Verb};
use crate::state::{Coverage, Need, Principal, Record, TenantState};
use crate::value::{refs_in, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// State access that refuses to answer from a snapshot that does not cover the question.
///
/// Every record the kernel looks at goes through here, so a decision can never rest on a
/// record that was simply not loaded.
#[derive(Clone, Copy)]
pub struct Lookup<'a> {
    pub tenant: &'a TenantState,
    pub coverage: &'a Coverage,
}

fn insufficient(need: Need) -> DomainError {
    let mut error = DomainError::new(
        ErrorKind::InsufficientState,
        format!("the snapshot does not cover {need:?}"),
    );
    error
        .evidence
        .push(Check::fail("coverage", format!("{need:?}")));
    error.missing = Some(need);
    error
}

impl<'a> Lookup<'a> {
    pub fn record(
        &self,
        object: &ObjectTypeId,
        id: RecordId,
    ) -> Result<Option<&'a Arc<Record>>, DomainError> {
        let need = Need::Record {
            object: object.clone(),
            id,
        };
        if !self.coverage.covers(&need) {
            return Err(insufficient(need));
        }
        Ok(self.tenant.record(object, id))
    }

    /// Records of `object` whose `field` equals `value`, or contains it when the field is a list.
    pub fn by_field(
        &self,
        object: &ObjectTypeId,
        field: &FieldId,
        value: &Value,
    ) -> Result<Vec<&'a Arc<Record>>, DomainError> {
        let need = Need::ByField {
            object: object.clone(),
            field: field.clone(),
            value: value.clone(),
        };
        if !self.coverage.covers(&need) {
            return Err(insufficient(need));
        }
        Ok(self
            .tenant
            .records_of(object)
            .filter(|record| match record.get(field) {
                Value::List(items) => items.contains(value),
                other => other == value,
            })
            .collect())
    }

    pub fn all_of(&self, object: &ObjectTypeId) -> Result<Vec<&'a Arc<Record>>, DomainError> {
        let need = Need::AllOf(object.clone());
        if !self.coverage.covers(&need) {
            return Err(insufficient(need));
        }
        Ok(self.tenant.records_of(object).collect())
    }
}

/// The roles a person holds through the table of members and the teams of a schema: the roles
/// assigned to it directly and the roles of each team it belongs to, and no other.
pub fn assigned_roles(
    members: &BTreeMap<PrincipalId, BTreeSet<RoleId>>,
    schema: &TenantSchema,
    id: &PrincipalId,
) -> BTreeSet<RoleId> {
    let mut roles = members.get(id).cloned().unwrap_or_default();
    for team in schema.teams.values() {
        if team.members.contains(id) {
            roles.extend(team.roles.iter().cloned());
        }
    }
    roles
}

/// The roles a principal holds in a tenant, directly or through a team. Everyone but the
/// system holds `public`.
pub fn roles_of(tenant: &TenantState, principal: &Principal) -> BTreeSet<RoleId> {
    match principal {
        Principal::System => BTreeSet::new(),
        Principal::Anonymous => BTreeSet::from([public_role()]),
        Principal::User(id) => {
            let mut roles = assigned_roles(&tenant.members, &tenant.schema, id);
            roles.insert(public_role());
            roles
        }
    }
}

/// The permissions a principal holds (ADM-151): exactly the union of the permissions of every
/// role in [`roles_of`], that is of each role assigned to it directly and of each role of each
/// team it belongs to. The `owner` role is built in and also holds every permission; that is
/// decided by [`holds_any`], not listed here.
pub fn effective_permissions(tenant: &TenantState, principal: &Principal) -> BTreeSet<Grant> {
    roles_of(tenant, principal)
        .iter()
        .flat_map(|role| tenant.schema.grants_of(role))
        .collect()
}

/// Whether the principal holds at least one of `wanted`. The owner holds all of them.
pub fn holds_any(tenant: &TenantState, principal: &Principal, wanted: &BTreeSet<Grant>) -> bool {
    roles_of(tenant, principal).contains(&owner_role())
        || !effective_permissions(tenant, principal).is_disjoint(wanted)
}

/// The grants of `wanted` the principal does not hold. The owner holds every one, so for the
/// owner this is empty.
pub fn missing_grants(
    tenant: &TenantState,
    principal: &Principal,
    wanted: &BTreeSet<Grant>,
) -> BTreeSet<Grant> {
    if roles_of(tenant, principal).contains(&owner_role()) {
        return BTreeSet::new();
    }
    let held = effective_permissions(tenant, principal);
    wanted.difference(&held).cloned().collect()
}

/// A grant as a sentence fragment: `capability <name>` or `<object>:<verb>`.
fn describe_grant(grant: &Grant) -> String {
    match grant {
        Grant::Capability(name) => format!("capability {name}"),
        Grant::Object(object, verb) => {
            let verb = format!("{verb:?}").to_lowercase();
            format!("{object}:{verb}")
        }
    }
}

/// ADM-152: whether `principal` may confer `role`, which it may only when it holds **every**
/// permission and capability the role grants (a strict superset, not an overlap). A role that
/// grants nothing has nothing to over-grant, except the built-in `owner`, which only an owner
/// confers. The check names the rule and the role; a refusal names what the principal lacks.
pub fn may_confer(tenant: &TenantState, principal: &Principal, role: &RoleId) -> Check {
    let rule = format!("permission:confer:{role}");
    if *role == owner_role() {
        return if roles_of(tenant, principal).contains(&owner_role()) {
            Check::pass(rule, "an owner confers owner")
        } else {
            Check::fail(rule, "only an owner confers the owner role")
        };
    }
    let grants = tenant.schema.grants_of(role);
    if grants.is_empty() {
        return Check::pass(rule, format!("role {role} grants no permission"));
    }
    let missing = missing_grants(tenant, principal, &grants);
    if missing.is_empty() {
        Check::pass(
            rule,
            format!("holds every permission that role {role} grants"),
        )
    } else {
        let lacking: Vec<String> = missing.iter().map(describe_grant).collect();
        Check::fail(
            rule,
            format!(
                "lacks {} of the {} permissions that role {role} grants: {}",
                missing.len(),
                grants.len(),
                lacking.join(", ")
            ),
        )
    }
}

/// The capabilities a role names among its permissions: its capability strings, and nothing of
/// its object-type permissions.
pub fn role_capabilities(schema: &TenantSchema, role: &RoleId) -> BTreeSet<String> {
    schema
        .grants_of(role)
        .into_iter()
        .filter_map(|grant| match grant {
            Grant::Capability(name) => Some(name),
            Grant::Object(..) => None,
        })
        .collect()
}

/// The tenant capabilities a principal holds (ADM-151): those named by a role assigned to it
/// directly or by a role of a team it belongs to, and no other. This is what the generated
/// authorizer's `AuthHoles::holding` fills a principal's tenant roles and teams from; a
/// capability is never held because of a role's name, only because a role names it. The owner
/// holds what its own role names, like any role.
pub fn held_capabilities(tenant: &TenantState, principal: &Principal) -> BTreeSet<String> {
    roles_of(tenant, principal)
        .iter()
        .flat_map(|role| role_capabilities(&tenant.schema, role))
        .collect()
}

/// The records a policy condition may read.
#[derive(Clone, Copy, Default)]
pub struct Subject<'a> {
    pub old: Option<&'a Record>,
    pub new: Option<&'a Record>,
}

impl<'a> Subject<'a> {
    pub fn stored(record: &'a Record) -> Self {
        Subject {
            old: Some(record),
            new: Some(record),
        }
    }

    fn record(&self) -> Option<&'a Record> {
        self.old.or(self.new)
    }
}

/// Decide whether `principal` may perform `action` on `resource`.
///
/// Default deny. The `owner` role is allowed everything in its tenant. Otherwise at least one
/// policy must match role, resource and action, and its condition must evaluate to `true`. A
/// condition that fails to evaluate denies.
pub fn authorize(
    tenant: &TenantState,
    schema: &TenantSchema,
    principal: &Principal,
    now: Timestamp,
    action: Action,
    resource: &Resource,
    subject: Subject<'_>,
) -> (bool, Vec<Check>) {
    let roles = roles_of(tenant, principal);
    let what = format!("{action:?} on {resource:?}");
    if roles.contains(&owner_role()) {
        return (true, vec![Check::pass("policy:builtin_owner", what)]);
    }
    if let (Resource::Object(object), Some(verb)) = (resource, Verb::of(action)) {
        let grant = Grant::Object(object.clone(), verb);
        if effective_permissions(tenant, principal).contains(&grant) {
            return (true, vec![Check::pass("permission:role", what)]);
        }
    }
    let no_params = BTreeMap::new();
    let env = Env {
        record: subject.record(),
        old: subject.old,
        new: subject.new,
        params: &no_params,
        now,
        principal,
        roles: &roles,
    };
    let mut checks = Vec::new();
    for policy in schema.policies.values() {
        let applies = roles.contains(&policy.role)
            && policy.actions.contains(&action)
            && match (&policy.resource, resource) {
                (Resource::AnyObject, Resource::Object(_)) => true,
                (a, b) => a == b,
            };
        if !applies {
            continue;
        }
        let rule = format!("policy:{}", policy.id);
        match &policy.condition {
            None => return (true, vec![Check::pass(rule, what)]),
            Some(condition) => match expr::eval(condition, &env) {
                Ok(Value::Bool(true)) => {
                    return (
                        true,
                        vec![Check::pass(rule, format!("{what} where {condition}"))],
                    )
                }
                Ok(other) => checks.push(Check::fail(
                    rule,
                    format!(
                        "{condition} is {}",
                        if other.is_null() { "null" } else { "false" }
                    ),
                )),
                Err(e) => checks.push(Check::fail(rule, format!("{condition}: {e}"))),
            },
        }
    }
    if checks.is_empty() {
        checks.push(Check::fail(
            "policy:none",
            format!("no policy grants {what}"),
        ));
    }
    (false, checks)
}

/// Whether some policy could grant the action, ignoring conditions. For user interfaces that
/// need to know what to offer; the real decision is always `authorize`.
pub fn may_attempt(
    tenant: &TenantState,
    principal: &Principal,
    action: Action,
    resource: &Resource,
) -> bool {
    let roles = roles_of(tenant, principal);
    let granted = match (resource, Verb::of(action)) {
        (Resource::Object(object), Some(verb)) => {
            effective_permissions(tenant, principal).contains(&Grant::Object(object.clone(), verb))
        }
        _ => false,
    };
    roles.contains(&owner_role())
        || granted
        || tenant.schema.policies.values().any(|policy| {
            roles.contains(&policy.role)
                && policy.actions.contains(&action)
                && match (&policy.resource, resource) {
                    (Resource::AnyObject, Resource::Object(_)) => true,
                    (a, b) => a == b,
                }
        })
}

/// A failed check together with the error it maps to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub check: Check,
    pub kind: ErrorKind,
}

/// The outcome of validating one record against a schema.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Validation {
    pub passed: Vec<Check>,
    pub failed: Vec<Failure>,
}

impl Validation {
    fn pass(&mut self, rule: String, detail: impl Into<String>) {
        self.passed.push(Check::pass(rule, detail));
    }

    fn fail(&mut self, kind: ErrorKind, rule: String, detail: impl Into<String>) {
        self.failed.push(Failure {
            check: Check::fail(rule, detail),
            kind,
        });
    }

    /// Turn the outcome into an error if anything failed. The first failure names the kind.
    pub fn into_result(self, evidence: &mut Vec<Check>) -> Result<(), DomainError> {
        evidence.extend(self.passed);
        let Some(first) = self.failed.first() else {
            return Ok(());
        };
        let kind = first.kind;
        let message = format!("{}: {}", first.check.rule, first.check.detail);
        evidence.extend(self.failed.into_iter().map(|f| f.check));
        Err(DomainError::new(kind, message).with(evidence.clone()))
    }
}

/// Validate a whole candidate record: known fields, types, required fields, workflow state,
/// constraints, uniqueness and references.
///
/// `old` is the stored record on update, `None` on create and when re-validating existing data
/// against a proposed schema.
pub fn validate_record(
    lookup: Lookup<'_>,
    schema: &TenantSchema,
    object: &ObjectTypeDef,
    candidate: &Record,
    old: Option<&Record>,
    principal: &Principal,
    now: Timestamp,
) -> Result<Validation, DomainError> {
    let mut out = Validation::default();
    let name = &object.id;

    for field in candidate.fields.keys() {
        if !object.fields.contains_key(field) {
            out.fail(
                ErrorKind::UnknownField,
                format!("field:{name}.{field}"),
                "no such field",
            );
        }
    }

    for field in object.fields.values() {
        let value = candidate.get(&field.id);
        let at = format!("{name}.{}", field.id);
        if value.is_null() {
            if field.required {
                out.fail(
                    ErrorKind::MissingRequired,
                    format!("required:{at}"),
                    "a value is required",
                );
            }
            continue;
        }
        if field.required {
            out.pass(format!("required:{at}"), "present");
        }
        if !field.ty.admits(value) {
            out.fail(
                ErrorKind::TypeMismatch,
                format!("type:{at}"),
                format!("expected {}, found {}", field.ty.name(), value.kind()),
            );
            continue;
        }
        out.pass(format!("type:{at}"), field.ty.name());

        if field.unique {
            let clash = lookup
                .by_field(name, &field.id, value)?
                .into_iter()
                .find(|other| other.id != candidate.id);
            match clash {
                Some(other) => out.fail(
                    ErrorKind::UniqueViolated,
                    format!("unique:{at}"),
                    format!("record {} already has this value", other.id),
                ),
                None => out.pass(format!("unique:{at}"), "no other record has this value"),
            }
        }

        if let Some((target, _, _)) = field.ty.ref_target() {
            for id in refs_in(value) {
                if lookup.record(target, id)?.is_some() {
                    out.pass(format!("reference:{at}"), format!("{target} {id} exists"));
                } else {
                    out.fail(
                        ErrorKind::DanglingReference,
                        format!("reference:{at}"),
                        format!("{target} {id} does not exist"),
                    );
                }
            }
        }
    }

    for workflow in schema.workflows_of(name) {
        if let Value::Str(state) = candidate.get(&workflow.field) {
            let rule = format!("workflow_state:{}", workflow.id);
            if workflow.states().contains(state.as_str()) {
                out.pass(rule, format!("{state} is a state"));
            } else {
                out.fail(
                    ErrorKind::WorkflowViolation,
                    rule,
                    format!("{state} is not a state"),
                );
            }
        }
    }

    // Constraints read typed fields, so they are only meaningful on a record whose fields
    // already passed the checks above.
    if out.failed.is_empty() {
        let roles = roles_of(lookup.tenant, principal);
        let no_params = BTreeMap::new();
        let env = Env {
            record: Some(candidate),
            old,
            new: Some(candidate),
            params: &no_params,
            now,
            principal,
            roles: &roles,
        };
        for constraint in object.constraints.values() {
            let rule = format!("constraint:{name}.{}", constraint.id);
            match expr::eval(&constraint.expr, &env) {
                Ok(Value::Bool(false)) => out.fail(
                    ErrorKind::ConstraintViolated,
                    rule,
                    format!("{} [{}]", constraint.message, constraint.expr),
                ),
                Ok(_) => out.pass(rule, constraint.expr.to_string()),
                Err(e) => out.fail(
                    ErrorKind::Evaluation,
                    rule,
                    format!("{}: {e}", constraint.expr),
                ),
            }
        }
    }
    Ok(out)
}
