//! Tenant structure as data: object types, fields, relationships, constraints, roles, policies,
//! workflows, named queries, view definitions, and the triggers, automations, tenant operations
//! and composite unique constraints of [`crate::definitions`], under one schema version.
//!
//! The types are generated ([`crate::types`]); this module holds what the kernel does with them:
//! the genesis schema, the consistency check, and the reading of a role's permissions and of the
//! teams and invitations a schema holds.

use crate::expr::{self, Expr, TypeEnv};
use crate::limits;
use crate::types::{
    owner_role, public_role, FieldTypeExt, InvitationId, ObjectTypeId, RoleId, SchemaVersion,
    TeamId,
};
pub use crate::types::{
    Action, ArgSource, ConstraintDef, FieldDef, FieldType, Grant, InvitationDef, ObjectAction,
    ObjectTypeDef, OrderKey, Permission, PolicyDef, QueryDef, Resource, RoleDef, TeamDef,
    TenantSchema, ViewDef, ViewSection, WorkflowDef, WorkflowTransition,
};
use std::collections::{BTreeMap, BTreeSet};

/// What the kernel asks of an [`ObjectAction`], one of the four actions on an object type that a
/// role's permission can name.
pub trait ObjectActionExt: Sized {
    /// The policy action it is.
    fn action(self) -> Action;
    /// The object action of a policy action; `None` for `Manage`.
    fn of(action: Action) -> Option<Self>;
}

impl ObjectActionExt for ObjectAction {
    fn action(self) -> Action {
        match self {
            ObjectAction::Read => Action::Read,
            ObjectAction::Create => Action::Create,
            ObjectAction::Update => Action::Update,
            ObjectAction::Delete => Action::Delete,
        }
    }

    fn of(action: Action) -> Option<Self> {
        match action {
            Action::Read => Some(ObjectAction::Read),
            Action::Create => Some(ObjectAction::Create),
            Action::Update => Some(ObjectAction::Update),
            Action::Delete => Some(ObjectAction::Delete),
            Action::Manage => None,
        }
    }
}

/// What the kernel asks of a [`Permission`]: the indivisible grants it stands for.
pub trait PermissionExt {
    /// One [`Grant`] for a capability, or one per action on the object type.
    fn grants(&self) -> Vec<Grant>;
}

impl PermissionExt for Permission {
    fn grants(&self) -> Vec<Grant> {
        match self {
            Permission::Capability(name) => vec![Grant::Capability(name.clone())],
            Permission::Object { object, actions } => actions
                .iter()
                .map(|verb| Grant::Object(object.clone(), *verb))
                .collect(),
        }
    }
}

/// The form in which an address is compared.
pub fn normalise_address(address: &str) -> String {
    address.trim().to_ascii_lowercase()
}

/// What the kernel asks of a [`WorkflowDef`].
pub trait WorkflowDefExt {
    /// Every state the workflow names: its initial states and both ends of its transitions.
    fn states(&self) -> BTreeSet<&str>;
}

impl WorkflowDefExt for WorkflowDef {
    fn states(&self) -> BTreeSet<&str> {
        self.initial
            .iter()
            .map(String::as_str)
            .chain(
                self.transitions
                    .iter()
                    .flat_map(|t| [t.from.as_str(), t.to.as_str()]),
            )
            .collect()
    }
}

/// What the kernel asks of a [`TenantSchema`].
pub trait TenantSchemaExt {
    /// The schema of a freshly created tenant: the two built-in roles and nothing else.
    fn genesis() -> Self;
    /// The workflows over one object type.
    fn workflows_of<'a>(
        &'a self,
        object: &'a ObjectTypeId,
    ) -> impl Iterator<Item = &'a WorkflowDef>;
    /// Fields anywhere in the schema that refer to `target`: `(object, field, is_list)`.
    fn referrers(&self, target: &ObjectTypeId) -> Vec<(&ObjectTypeDef, &FieldDef, bool)>;
    /// Check that the schema is internally consistent. Returns every problem found, in a
    /// deterministic order.
    fn problems(&self) -> Vec<String>;
    /// Every grant of a role. A role without permissions grants none.
    fn grants_of(&self, role: &RoleId) -> BTreeSet<Grant>;
    /// Replace the permissions a role grants; none removes the role's entry. A schema whose
    /// roles grant nothing holds no table of permissions at all, as the genesis schema does.
    fn set_role_permissions(&mut self, role: &RoleId, permissions: Vec<Permission>);
    /// One team, by its id.
    fn team(&self, id: &TeamId) -> Option<&TeamDef>;
    /// Every team, in id order.
    fn all_teams(&self) -> impl Iterator<Item = &TeamDef>;
    /// Define or replace a team. A team's membership changes through its own intents, not
    /// through `ChangeSchema`, and does not raise the schema version.
    fn put_team(&mut self, team: TeamDef);
    /// One invitation, by its id.
    fn invitation(&self, id: &InvitationId) -> Option<&InvitationDef>;
    /// The invitation, to change it.
    fn invitation_mut(&mut self, id: &InvitationId) -> Option<&mut InvitationDef>;
    /// Every invitation, redeemed or not, in id order.
    fn all_invitations(&self) -> impl Iterator<Item = &InvitationDef>;
    /// Record an invitation.
    fn put_invitation(&mut self, invitation: InvitationDef);
}

impl TenantSchemaExt for TenantSchema {
    fn genesis() -> Self {
        let roles = [(public_role(), "Everyone"), (owner_role(), "Owner")]
            .into_iter()
            .map(|(id, label)| {
                (
                    id.clone(),
                    RoleDef {
                        id,
                        label: label.to_string(),
                    },
                )
            })
            .collect();
        TenantSchema {
            version: SchemaVersion(1),
            objects: BTreeMap::new(),
            roles,
            role_permissions: None,
            teams: None,
            invitations: None,
            policies: BTreeMap::new(),
            workflows: BTreeMap::new(),
            queries: BTreeMap::new(),
            views: BTreeMap::new(),
            triggers: None,
            automations: None,
            operations: None,
            settings: None,
            capabilities: None,
        }
    }

    fn grants_of(&self, role: &RoleId) -> BTreeSet<Grant> {
        self.role_permissions
            .as_ref()
            .and_then(|table| table.get(role))
            .into_iter()
            .flatten()
            .flat_map(PermissionExt::grants)
            .collect()
    }

    fn set_role_permissions(&mut self, role: &RoleId, permissions: Vec<Permission>) {
        let mut table = self.role_permissions.take().unwrap_or_default();
        if permissions.is_empty() {
            table.remove(role);
        } else {
            table.insert(role.clone(), permissions);
        }
        self.role_permissions = (!table.is_empty()).then_some(table);
    }

    fn team(&self, id: &TeamId) -> Option<&TeamDef> {
        self.teams.as_ref().and_then(|teams| teams.get(id))
    }

    fn all_teams(&self) -> impl Iterator<Item = &TeamDef> {
        self.teams.iter().flat_map(BTreeMap::values)
    }

    fn put_team(&mut self, team: TeamDef) {
        self.teams
            .get_or_insert_with(BTreeMap::new)
            .insert(team.id.clone(), team);
    }

    fn invitation(&self, id: &InvitationId) -> Option<&InvitationDef> {
        self.invitations
            .as_ref()
            .and_then(|invitations| invitations.get(id))
    }

    fn invitation_mut(&mut self, id: &InvitationId) -> Option<&mut InvitationDef> {
        self.invitations
            .as_mut()
            .and_then(|invitations| invitations.get_mut(id))
    }

    fn all_invitations(&self) -> impl Iterator<Item = &InvitationDef> {
        self.invitations.iter().flat_map(BTreeMap::values)
    }

    fn put_invitation(&mut self, invitation: InvitationDef) {
        self.invitations
            .get_or_insert_with(BTreeMap::new)
            .insert(invitation.id.clone(), invitation);
    }

    fn workflows_of<'a>(
        &'a self,
        object: &'a ObjectTypeId,
    ) -> impl Iterator<Item = &'a WorkflowDef> {
        self.workflows.values().filter(move |w| &w.object == object)
    }

    fn referrers(&self, target: &ObjectTypeId) -> Vec<(&ObjectTypeDef, &FieldDef, bool)> {
        let mut found = Vec::new();
        for object in self.objects.values() {
            for field in object.fields.values() {
                if let Some((to, _, is_list)) = field.ty.ref_target() {
                    if to == target {
                        found.push((object, field, is_list));
                    }
                }
            }
        }
        found
    }

    fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let roles: BTreeSet<RoleId> = self.roles.keys().cloned().collect();
        let no_params = BTreeSet::new();
        let no_fields = BTreeMap::new();

        for (what, count, max) in [
            ("object types", self.objects.len(), limits::MAX_OBJECTS),
            ("policies", self.policies.len(), limits::MAX_POLICIES),
            ("queries", self.queries.len(), limits::MAX_QUERIES),
        ] {
            if count > max {
                problems.push(format!(
                    "limit: {count} {what} is more than the maximum of {max}"
                ));
            }
        }
        for query in self.queries.values() {
            if query
                .limit
                .is_some_and(|n| n == 0 || n > limits::MAX_PAGE_SIZE)
            {
                problems.push(format!(
                    "limit: query {} declares a limit outside 1 to {}",
                    query.id,
                    limits::MAX_PAGE_SIZE
                ));
            }
        }

        for object in self.objects.values() {
            if object.fields.len() > limits::MAX_FIELDS_PER_OBJECT {
                problems.push(format!(
                    "limit: {} has {} fields, more than the maximum of {}",
                    object.id,
                    object.fields.len(),
                    limits::MAX_FIELDS_PER_OBJECT
                ));
            }
            let env = TypeEnv {
                fields: &object.fields,
                params: &no_params,
                roles: &roles,
                objects: &self.objects,
            };
            for field in object.fields.values() {
                let at = format!("{}.{}", object.id, field.id);
                if let Some((target, _, _)) = field.ty.ref_target() {
                    if !self.objects.contains_key(target) {
                        problems.push(format!("{at}: refers to unknown object type {target}"));
                    }
                }
                if let FieldType::List(inner) = &field.ty {
                    if matches!(inner.as_ref(), FieldType::List(_)) {
                        problems.push(format!("{at}: nested lists are not supported"));
                    }
                    if field.unique {
                        problems.push(format!("{at}: a list field cannot be unique"));
                    }
                }
                if let Some(default) = &field.default {
                    // A default may not read the record it is helping to build.
                    let env = TypeEnv {
                        fields: &no_fields,
                        params: &no_params,
                        roles: &roles,
                        objects: &self.objects,
                    };
                    if let Err(e) = expr::check(default, &env) {
                        problems.push(format!("{at}: default: {e}"));
                    }
                }
                if let Some(read) = &field.read {
                    match expr::check(read, &env) {
                        Ok(expr::Shape::Bool | expr::Shape::Any) => {}
                        Ok(shape) => {
                            problems.push(format!("{at}: read is {shape:?}, not a truth value"))
                        }
                        Err(e) => problems.push(format!("{at}: read: {e}")),
                    }
                }
            }
            for constraint in object.constraints.values() {
                match expr::check(&constraint.expr, &env) {
                    Ok(expr::Shape::Bool | expr::Shape::Any) => {}
                    Ok(shape) => problems.push(format!(
                        "{}.{}: constraint is {shape:?}, not a truth value",
                        object.id, constraint.id
                    )),
                    Err(e) => problems.push(format!("{}.{}: {e}", object.id, constraint.id)),
                }
            }
            // A type with a personal field says how long its records are kept (D-6584).
            if object.retention.is_none() && object.fields.values().any(|f| f.personal.is_some()) {
                problems.push(format!(
                    "{}: a type with a personal field must declare retention (D-6584)",
                    object.id
                ));
            }
        }

        for (role, permissions) in self.role_permissions.iter().flatten() {
            let at = format!("role {role}");
            if !roles.contains(role) {
                problems.push(format!("{at}: permissions of an unknown role"));
            }
            if *role == owner_role() {
                problems.push(format!("{at}: the owner role holds every permission"));
            }
            for permission in permissions {
                match permission {
                    Permission::Capability(name) if name.trim().is_empty() => {
                        problems.push(format!("{at}: a capability needs a name"))
                    }
                    Permission::Capability(name) => {
                        if let Some(problem) = crate::capabilities::grant_problem(self, role, name)
                        {
                            problems.push(format!("{at}: {problem}"));
                        }
                    }
                    Permission::Object { object, actions } => {
                        if !self.objects.contains_key(object) {
                            problems
                                .push(format!("{at}: permission on unknown object type {object}"));
                        }
                        if actions.is_empty() {
                            problems.push(format!("{at}: permission on {object} names no action"));
                        }
                    }
                }
            }
        }
        for team in self.all_teams() {
            let at = format!("team {}", team.id);
            for role in &team.roles {
                if !roles.contains(role) {
                    problems.push(format!("{at}: unknown role {role}"));
                }
                if *role == public_role() {
                    problems.push(format!("{at}: everyone already holds public"));
                }
            }
        }
        for invitation in self.all_invitations() {
            let at = format!("invitation {}", invitation.id);
            if invitation.address.is_empty() {
                problems.push(format!("{at}: names no address"));
            }
            for role in &invitation.roles {
                if !roles.contains(role) {
                    problems.push(format!("{at}: unknown role {role}"));
                }
            }
        }

        for policy in self.policies.values() {
            let at = format!("policy {}", policy.id);
            if !roles.contains(&policy.role) {
                problems.push(format!("{at}: unknown role {}", policy.role));
            }
            if policy.role == owner_role() {
                problems.push(format!("{at}: the owner role needs no policy"));
            }
            let fields = match &policy.resource {
                Resource::Object(id) => match self.objects.get(id) {
                    Some(object) => Some(&object.fields),
                    None => {
                        problems.push(format!("{at}: unknown object type {id}"));
                        None
                    }
                },
                _ => None,
            };
            let manage = matches!(policy.resource, Resource::Schema | Resource::Members);
            for action in &policy.actions {
                if manage != (*action == Action::Manage) {
                    problems.push(format!("{at}: action {action:?} does not fit the resource"));
                }
            }
            if let Some(condition) = &policy.condition {
                let env = TypeEnv {
                    fields: fields.unwrap_or(&no_fields),
                    params: &no_params,
                    roles: &roles,
                    objects: &self.objects,
                };
                if matches!(policy.resource, Resource::AnyObject) && mentions_field(condition) {
                    problems.push(format!("{at}: a policy on every object cannot read fields"));
                } else if let Err(e) = expr::check(condition, &env) {
                    problems.push(format!("{at}: {e}"));
                }
            }
        }

        for workflow in self.workflows.values() {
            let at = format!("workflow {}", workflow.id);
            let Some(object) = self.objects.get(&workflow.object) else {
                problems.push(format!("{at}: unknown object type {}", workflow.object));
                continue;
            };
            match object.fields.get(&workflow.field) {
                Some(field) if field.ty == FieldType::Str => {}
                Some(_) => problems.push(format!("{at}: field {} is not a string", workflow.field)),
                None => problems.push(format!("{at}: unknown field {}", workflow.field)),
            }
            if workflow.initial.is_empty() {
                problems.push(format!("{at}: no initial state"));
            }
            let env = TypeEnv {
                fields: &object.fields,
                params: &no_params,
                roles: &roles,
                objects: &self.objects,
            };
            for transition in &workflow.transitions {
                if let Some(guard) = &transition.guard {
                    if let Err(e) = expr::check(guard, &env) {
                        problems.push(format!("{at}.{}: guard: {e}", transition.name));
                    }
                }
                for (field, value) in &transition.set {
                    if !object.fields.contains_key(field) {
                        problems.push(format!(
                            "{at}.{}: sets unknown field {field}",
                            transition.name
                        ));
                    }
                    if let Err(e) = expr::check(value, &env) {
                        problems.push(format!("{at}.{}: set {field}: {e}", transition.name));
                    }
                }
            }
        }

        for query in self.queries.values() {
            let at = format!("query {}", query.id);
            let Some(object) = self.objects.get(&query.object) else {
                problems.push(format!("{at}: unknown object type {}", query.object));
                continue;
            };
            let env = TypeEnv {
                fields: &object.fields,
                params: &query.params,
                roles: &roles,
                objects: &self.objects,
            };
            if let Some(filter) = &query.filter {
                if let Err(e) = expr::check(filter, &env) {
                    problems.push(format!("{at}: filter: {e}"));
                }
            }
            for key in &query.order {
                let var = Expr::Var(expr::Scope::Record, key.field.clone());
                if let Err(e) = expr::check(&var, &env) {
                    problems.push(format!("{at}: order: {e}"));
                }
            }
        }

        let mut routes = BTreeSet::new();
        for view in self.views.values() {
            let at = format!("view {}", view.id);
            if !view.route.starts_with('/') {
                problems.push(format!("{at}: route must start with /"));
            }
            if !routes.insert(view.route.as_str()) {
                problems.push(format!("{at}: route {} is already taken", view.route));
            }
            for (name, section) in &view.sections {
                let Some(query) = self.queries.get(&section.query) else {
                    problems.push(format!("{at}.{name}: unknown query {}", section.query));
                    continue;
                };
                for (arg, source) in &section.args {
                    if !query.params.contains(arg) {
                        problems.push(format!("{at}.{name}: query has no parameter {arg}"));
                    }
                    if let ArgSource::Section { section: other, .. } = source {
                        match view.sections.get(other) {
                            Some(dep) if !dep.single => {
                                problems.push(format!("{at}.{name}: section {other} is not single"))
                            }
                            Some(dep) => {
                                let nested = dep
                                    .args
                                    .values()
                                    .any(|a| matches!(a, ArgSource::Section { .. }));
                                if nested {
                                    problems.push(format!(
                                        "{at}.{name}: section {other} itself depends on a section"
                                    ));
                                }
                            }
                            None => problems.push(format!("{at}.{name}: unknown section {other}")),
                        }
                    }
                }
                if let Some(object) = self.objects.get(&query.object) {
                    for field in &section.include {
                        let is_ref = object
                            .fields
                            .get(field)
                            .is_some_and(|f| f.ty.ref_target().is_some());
                        if !is_ref {
                            problems.push(format!("{at}.{name}: {field} is not a reference field"));
                        }
                    }
                    for field in &section.markdown {
                        let is_text = object
                            .fields
                            .get(field)
                            .is_some_and(|f| f.ty == FieldType::Str);
                        if !is_text {
                            problems.push(format!("{at}.{name}: {field} is not a string field"));
                        }
                    }
                }
            }
        }

        for object in self.objects.values() {
            crate::definitions::unique_problems(object, &mut problems);
        }
        crate::definitions::trigger_problems(self, &roles, &mut problems);
        crate::definitions::automation_problems(self, &roles, &mut problems);
        crate::definitions::operation_problems(self, &roles, &mut problems);
        crate::settings::definition_problems(self, &mut problems);
        crate::capabilities::declaration_problems(self, &mut problems);
        problems
    }
}

fn mentions_field(expr: &Expr) -> bool {
    match expr {
        Expr::Var(_, name) => !name.starts_with('$'),
        Expr::Through(..) => true,
        Expr::Lit(_) | Expr::Param(_) | Expr::Now | Expr::Principal | Expr::HasRole(_) => false,
        Expr::Not(a)
        | Expr::IsNull(a)
        | Expr::Len(a)
        | Expr::Lower(a)
        | Expr::IsSlug(a)
        | Expr::Duration(_, a)
        | Expr::Text(a) => mentions_field(a),
        Expr::And(items) | Expr::Or(items) => items.iter().any(mentions_field),
        Expr::Cmp(_, a, b)
        | Expr::In(a, b)
        | Expr::Contains(a, b)
        | Expr::StartsWith(a, b)
        | Expr::Arith(_, a, b)
        | Expr::MsBetween(a, b)
        | Expr::Concat(a, b) => mentions_field(a) || mentions_field(b),
        Expr::If(a, b, c) => mentions_field(a) || mentions_field(b) || mentions_field(c),
    }
}
