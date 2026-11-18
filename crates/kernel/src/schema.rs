//! Tenant structure as data: object types, fields, relationships, constraints, roles, policies,
//! workflows, named queries and view definitions, under one schema version.

use crate::expr::{self, Expr, TypeEnv};
use crate::ids::{
    owner_role, public_role, ConstraintId, FieldId, ObjectTypeId, PolicyId, QueryId, RoleId,
    SchemaVersion, ViewId, WorkflowId,
};
use crate::value::{FieldType, Value};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDef {
    pub id: FieldId,
    pub label: String,
    pub ty: FieldType,
    /// A required field may not be `null`.
    pub required: bool,
    /// No two records of the object type may share a non-null value.
    pub unique: bool,
    /// Evaluated on create when the intent leaves the field out.
    pub default: Option<Expr>,
}

/// A rule every record of the object type must satisfy. Violated only when it evaluates to
/// `false`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConstraintDef {
    pub id: ConstraintId,
    pub expr: Expr,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectTypeDef {
    pub id: ObjectTypeId,
    pub label: String,
    pub fields: BTreeMap<FieldId, FieldDef>,
    pub constraints: BTreeMap<ConstraintId, ConstraintDef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleDef {
    pub id: RoleId,
    pub label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Action {
    Read,
    Create,
    Update,
    Delete,
    /// Change the schema or the membership.
    Manage,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Resource {
    Object(ObjectTypeId),
    AnyObject,
    Schema,
    Members,
}

/// An allow rule. Nothing is permitted without one, except to the `owner` role.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDef {
    pub id: PolicyId,
    pub role: RoleId,
    pub resource: Resource,
    pub actions: BTreeSet<Action>,
    /// When present, the rule applies only where this evaluates to `true`.
    pub condition: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowTransition {
    pub name: String,
    pub from: String,
    pub to: String,
    pub guard: Option<Expr>,
    /// Field assignments applied when this transition fires.
    pub set: BTreeMap<FieldId, Expr>,
}

/// A state machine over one string field of an object type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowDef {
    pub id: WorkflowId,
    pub object: ObjectTypeId,
    pub field: FieldId,
    /// The states a new record may start in.
    pub initial: BTreeSet<String>,
    pub transitions: Vec<WorkflowTransition>,
}

impl WorkflowDef {
    pub fn states(&self) -> BTreeSet<&str> {
        self.initial
            .iter()
            .map(String::as_str)
            .chain(self.transitions.iter().flat_map(|t| [t.from.as_str(), t.to.as_str()]))
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderKey {
    /// A field id or a system attribute such as `$created_at`.
    pub field: String,
    pub descending: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryDef {
    pub id: QueryId,
    pub object: ObjectTypeId,
    pub filter: Option<Expr>,
    pub params: BTreeSet<String>,
    pub order: Vec<OrderKey>,
    pub limit: Option<u32>,
}

/// Where a view section's query argument comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArgSource {
    /// A named segment of the view's route, such as `slug` in `/posts/:slug`.
    Route(String),
    /// A named request parameter, such as a search term.
    Request(String),
    Const(Value),
    /// An attribute of the single row of another section of the same view.
    Section { section: String, attribute: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewSection {
    pub query: QueryId,
    pub args: BTreeMap<String, ArgSource>,
    /// Reference fields whose targets are delivered alongside the rows.
    pub include: Vec<FieldId>,
    /// The section yields at most one row.
    pub single: bool,
}

/// A named bundle of queries that together make one page of data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewDef {
    pub id: ViewId,
    /// An address pattern such as `/posts/:slug`. Opaque to the kernel apart from uniqueness.
    pub route: String,
    pub title: String,
    pub sections: BTreeMap<String, ViewSection>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantSchema {
    pub version: SchemaVersion,
    pub objects: BTreeMap<ObjectTypeId, ObjectTypeDef>,
    pub roles: BTreeMap<RoleId, RoleDef>,
    pub policies: BTreeMap<PolicyId, PolicyDef>,
    pub workflows: BTreeMap<WorkflowId, WorkflowDef>,
    pub queries: BTreeMap<QueryId, QueryDef>,
    pub views: BTreeMap<ViewId, ViewDef>,
}

impl TenantSchema {
    /// The schema of a freshly created tenant: the two built-in roles and nothing else.
    pub fn genesis() -> Self {
        let roles = [(public_role(), "Everyone"), (owner_role(), "Owner")]
            .into_iter()
            .map(|(id, label)| (id.clone(), RoleDef { id, label: label.to_string() }))
            .collect();
        TenantSchema {
            version: SchemaVersion(1),
            objects: BTreeMap::new(),
            roles,
            policies: BTreeMap::new(),
            workflows: BTreeMap::new(),
            queries: BTreeMap::new(),
            views: BTreeMap::new(),
        }
    }

    pub fn workflows_of<'a>(
        &'a self,
        object: &'a ObjectTypeId,
    ) -> impl Iterator<Item = &'a WorkflowDef> {
        self.workflows.values().filter(move |w| &w.object == object)
    }

    /// Fields anywhere in the schema that refer to `target`: `(object, field, is_list)`.
    pub fn referrers(&self, target: &ObjectTypeId) -> Vec<(&ObjectTypeDef, &FieldDef, bool)> {
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

    /// Check that the schema is internally consistent. Returns every problem found, in a
    /// deterministic order.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let roles: BTreeSet<RoleId> = self.roles.keys().cloned().collect();
        let no_params = BTreeSet::new();
        let no_fields = BTreeMap::new();

        for object in self.objects.values() {
            let env = TypeEnv { fields: &object.fields, params: &no_params, roles: &roles };
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
                    let env = TypeEnv { fields: &no_fields, params: &no_params, roles: &roles };
                    if let Err(e) = expr::check(default, &env) {
                        problems.push(format!("{at}: default: {e}"));
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
            let env = TypeEnv { fields: &object.fields, params: &no_params, roles: &roles };
            for transition in &workflow.transitions {
                if let Some(guard) = &transition.guard {
                    if let Err(e) = expr::check(guard, &env) {
                        problems.push(format!("{at}.{}: guard: {e}", transition.name));
                    }
                }
                for (field, value) in &transition.set {
                    if !object.fields.contains_key(field) {
                        problems.push(format!("{at}.{}: sets unknown field {field}", transition.name));
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
            let env = TypeEnv { fields: &object.fields, params: &query.params, roles: &roles };
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
                            Some(dep) if !dep.single => problems
                                .push(format!("{at}.{name}: section {other} is not single")),
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
                }
            }
        }
        problems
    }
}

fn mentions_field(expr: &Expr) -> bool {
    match expr {
        Expr::Var(_, name) => !name.starts_with('$'),
        Expr::Lit(_) | Expr::Param(_) | Expr::Now | Expr::Principal | Expr::HasRole(_) => false,
        Expr::Not(a) | Expr::IsNull(a) | Expr::Len(a) | Expr::Lower(a) | Expr::IsSlug(a) => {
            mentions_field(a)
        }
        Expr::And(items) | Expr::Or(items) => items.iter().any(mentions_field),
        Expr::Cmp(_, a, b) | Expr::In(a, b) | Expr::Contains(a, b) | Expr::StartsWith(a, b) => {
            mentions_field(a) || mentions_field(b)
        }
        Expr::If(a, b, c) => mentions_field(a) || mentions_field(b) || mentions_field(c),
    }
}
