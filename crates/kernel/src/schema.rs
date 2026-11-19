//! Tenant structure as data: object types, fields, relationships, constraints, roles, policies,
//! workflows, named queries and view definitions, under one schema version.
//!
//! The types are generated ([`crate::types`]); this module holds what the kernel does with them:
//! the genesis schema and the consistency check.

use crate::expr::{self, Expr, TypeEnv};
use crate::types::{owner_role, public_role, FieldTypeExt, ObjectTypeId, RoleId, SchemaVersion};
pub use crate::types::{
    Action, ArgSource, ConstraintDef, FieldDef, FieldType, ObjectTypeDef, OrderKey, PolicyDef,
    QueryDef, Resource, RoleDef, TenantSchema, ViewDef, ViewSection, WorkflowDef,
    WorkflowTransition,
};
use std::collections::{BTreeMap, BTreeSet};

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
            policies: BTreeMap::new(),
            workflows: BTreeMap::new(),
            queries: BTreeMap::new(),
            views: BTreeMap::new(),
        }
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

        for object in self.objects.values() {
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
        problems
    }
}

fn mentions_field(expr: &Expr) -> bool {
    match expr {
        Expr::Var(_, name) => !name.starts_with('$'),
        Expr::Through(..) => true,
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
