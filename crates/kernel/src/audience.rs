//! Audience classes (D-15, CPY-005, CPY-006, AGG-033, D-5065): which readers are indistinguishable
//! for a result, derived from the policies of a tenant.
//!
//! # What a class is
//!
//! The audience class of a reader for a result is a function of the schema, of what is read (a
//! named query, a view or an aggregate) and of the reader's roles. Two readers have the same class
//! only when the kernel can show that everything either of them may read of the result is the same
//! for both. The class says nothing about what the result holds; it says who may be given the same
//! copy of it.
//!
//! What a reader may read of a result is decided by exactly these things, and the derivation looks
//! at nothing else:
//!
//! 1. the Read policies of every object type whose records the result carries: the policies of the
//!    roles the reader holds that name Read on the object type or on every object type, and their
//!    conditions;
//! 2. the role permissions that grant Read on those object types, and the built-in owner, which
//!    reads everything the policies of its tenant allow;
//! 3. the read rule of every field the result can carry: for a named query or a view every field of
//!    every object type it carries, for an aggregate the field it reads, the field it groups by
//!    and every field its query declares (a request filter can name no other);
//! 4. the filter of the query, which is evaluated for the reader.
//!
//! The class of a reader is then:
//!
//! - [`AudienceClass::System`] for the system, which reads everything;
//! - [`AudienceClass::Roles`] with the reader's roles that matter to the result, when no condition
//!   that applies to the reader mentions the reader or the clock: the roles that hold a Read
//!   permission on a carried object type, the roles of a Read policy of one, the roles a condition
//!   tests with `has_role`, and the owner. A reader's other roles are left out, because nothing in
//!   the result depends on them, so two readers who differ only in such a role share one class;
//! - [`AudienceClass::Individual`], the reader alone, in every other case: a condition that applies
//!   to the reader (a policy of a role the reader holds, a field read rule, the query filter)
//!   mentions `principal`, which includes comparing a record's author with the reader, or `now`,
//!   which makes the answer depend on the moment; and anything the derivation cannot resolve (a
//!   query, view or aggregate or an object type or field that the schema does not have).
//!
//! The derivation is conservative by construction: a finer class is always sound, a coarser one
//! never is, so every doubt makes a reader individual. An individual class is never shared and
//! nothing is stored for it ([`AudienceClass::key`] is `None`).
//!
//! It is a pure function: it reads no record, no clock and no state apart from its arguments.
//! Roles come from [`roles_of`], which already includes the roles of the reader's teams and the
//! run-as roles of an automation principal.

use crate::aggregates::{aggregates_of, declared_of, walk};
use crate::expr::Expr;
use crate::rules::roles_of;
use crate::schema::{
    Action, Grant, ObjectAction, PolicyDef, Resource, TenantSchema, TenantSchemaExt,
};
use crate::state::{Principal, TenantState};
use crate::types::{
    owner_role, AggregateId, FieldId, FieldTypeExt, ObjectTypeId, QueryDef, QueryId, RoleId, ViewId,
};
use std::collections::BTreeSet;

/// The reader's class for one result; see the module documentation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AudienceClass {
    /// The system principal, which reads everything.
    System,
    /// Every reader whose roles that matter to the result are these: they may read the same of it.
    Roles(BTreeSet<RoleId>),
    /// The result depends on who or when the reader is, or the kernel cannot show otherwise. The
    /// class holds the reader and is shared with nobody.
    Individual(Principal),
}

impl AudienceClass {
    /// Whether a result may be shared between the readers of the class and stored for them.
    pub fn is_shareable(&self) -> bool {
        !matches!(self, AudienceClass::Individual(_))
    }

    /// The class as the part of a copy key that names it, or `None` for a class that is not
    /// shared: nothing is stored under it. Two classes have the same key exactly when they are
    /// equal.
    pub fn key(&self) -> Option<String> {
        match self {
            AudienceClass::System => Some("system".to_string()),
            AudienceClass::Roles(roles) => Some(format!(
                "roles:{}",
                serde_json::to_string(roles).unwrap_or_default()
            )),
            AudienceClass::Individual(_) => None,
        }
    }
}

/// What a reader reads.
#[derive(Clone, Copy, Debug)]
pub enum Reading<'a> {
    /// The rows of a named query.
    Query(&'a QueryId),
    /// A view: every section's rows, the records it includes and its aggregates.
    View(&'a ViewId),
    /// One aggregate of a named query.
    Aggregate {
        query: &'a QueryId,
        aggregate: &'a AggregateId,
    },
}

/// Which fields of an object type a result can carry.
enum Fields<'a> {
    All,
    Named(Vec<&'a FieldId>),
}

/// Everything a result's content depends on, collected from the schema.
struct Footprint<'a> {
    schema: &'a TenantSchema,
    /// The object types whose records the result carries.
    objects: BTreeSet<ObjectTypeId>,
    /// The Read policies of those object types.
    policies: Vec<&'a PolicyDef>,
    /// The conditions that apply to every reader: field read rules and query filters.
    exprs: Vec<&'a Expr>,
    /// The schema lacks something the result names.
    unresolved: bool,
}

impl<'a> Footprint<'a> {
    fn new(schema: &'a TenantSchema) -> Self {
        Footprint {
            schema,
            objects: BTreeSet::new(),
            policies: Vec::new(),
            exprs: Vec::new(),
            unresolved: false,
        }
    }

    fn add_object(&mut self, object: &ObjectTypeId, fields: Fields<'_>) {
        let schema = self.schema;
        let Some(def) = schema.objects.get(object) else {
            self.unresolved = true;
            return;
        };
        if self.objects.insert(object.clone()) {
            for policy in schema.policies.values() {
                let reaches = match &policy.resource {
                    Resource::AnyObject => true,
                    Resource::Object(named) => named == object,
                    Resource::Schema | Resource::Members | Resource::Ledger { .. } => false,
                };
                if reaches && policy.actions.contains(&Action::Read) {
                    self.policies.push(policy);
                }
            }
        }
        if let Fields::Named(names) = &fields {
            if names.iter().any(|name| !def.fields.contains_key(*name)) {
                self.unresolved = true;
            }
        }
        for (id, field) in &def.fields {
            let carried = match &fields {
                Fields::All => true,
                Fields::Named(names) => names.contains(&id),
            };
            if carried {
                if let Some(rule) = &field.read {
                    self.exprs.push(rule);
                }
            }
        }
    }

    fn add_query(&mut self, def: &'a QueryDef, fields: Fields<'_>) {
        if let Some(filter) = &def.filter {
            self.exprs.push(filter);
        }
        self.add_object(&def.object, fields);
    }

    fn add_aggregate(&mut self, query: &QueryId, aggregate: &AggregateId) {
        let schema = self.schema;
        let Some(def) = schema.queries.get(query) else {
            self.unresolved = true;
            return;
        };
        let Some(agg) = aggregates_of(def).iter().find(|a| a.id == *aggregate) else {
            self.unresolved = true;
            return;
        };
        let mut named: Vec<&FieldId> = declared_of(def).iter().collect();
        named.extend(agg.field.iter());
        named.extend(agg.group_by.iter());
        self.add_query(def, Fields::Named(named));
    }

    fn add_view(&mut self, view: &ViewId) {
        let schema = self.schema;
        let Some(def) = schema.views.get(view) else {
            self.unresolved = true;
            return;
        };
        for section in def.sections.values() {
            if let Some(aggregate) = &section.aggregate {
                self.add_aggregate(&section.query, aggregate);
                continue;
            }
            let Some(query) = schema.queries.get(&section.query) else {
                self.unresolved = true;
                continue;
            };
            self.add_query(query, Fields::All);
            // A section that includes a reference carries the record it points at.
            let Some(object) = schema.objects.get(&query.object) else {
                continue;
            };
            for included in &section.include {
                match object.fields.get(included).and_then(|f| f.ty.ref_target()) {
                    Some((target, _, _)) => self.add_object(target, Fields::All),
                    None => self.unresolved = true,
                }
            }
        }
    }
}

/// What an expression asks of the reader: whether it mentions the principal or the clock, and the
/// roles it tests.
fn scan(expr: &Expr, roles: &mut BTreeSet<RoleId>) -> bool {
    let mut dependent = false;
    walk(expr, &mut |node| match node {
        Expr::Principal | Expr::Now => dependent = true,
        Expr::HasRole(role) => {
            roles.insert(role.clone());
        }
        _ => {}
    });
    dependent
}

/// The audience class of `principal`, who holds `roles`, for `reading` under `schema`.
///
/// `roles` is what [`roles_of`] gives for the principal in the tenant; [`class_of`] takes it from
/// the tenant.
pub fn audience_class(
    schema: &TenantSchema,
    reading: Reading<'_>,
    principal: &Principal,
    roles: &BTreeSet<RoleId>,
) -> AudienceClass {
    if matches!(principal, Principal::System) {
        return AudienceClass::System;
    }
    let individual = || AudienceClass::Individual(principal.clone());
    let mut footprint = Footprint::new(schema);
    match reading {
        Reading::Query(query) => match schema.queries.get(query) {
            Some(def) => footprint.add_query(def, Fields::All),
            None => footprint.unresolved = true,
        },
        Reading::View(view) => footprint.add_view(view),
        Reading::Aggregate { query, aggregate } => footprint.add_aggregate(query, aggregate),
    }
    if footprint.unresolved {
        return individual();
    }

    // Roles that matter: those a Read policy of a carried object type names, and those a condition
    // tests. Conditions that apply to every reader make the reader individual when they mention
    // the reader or the clock; a policy's condition does so only for a reader who holds its role,
    // since no other reader is judged by it.
    let mut relevant: BTreeSet<RoleId> = BTreeSet::new();
    for expr in &footprint.exprs {
        if scan(expr, &mut relevant) {
            return individual();
        }
    }
    for policy in &footprint.policies {
        relevant.insert(policy.role.clone());
        if let Some(condition) = &policy.condition {
            let dependent = scan(condition, &mut relevant);
            if dependent && roles.contains(&policy.role) {
                return individual();
            }
        }
    }
    let reads: Vec<Grant> = footprint
        .objects
        .iter()
        .map(|object| Grant::Object(object.clone(), ObjectAction::Read))
        .collect();
    let owner = owner_role();
    let kept = roles
        .iter()
        .filter(|role| {
            **role == owner || relevant.contains(*role) || {
                let grants = schema.grants_of(role);
                reads.iter().any(|read| grants.contains(read))
            }
        })
        .cloned()
        .collect();
    AudienceClass::Roles(kept)
}

/// The audience class of `principal` in `tenant` for `reading`.
pub fn class_of(
    tenant: &TenantState,
    reading: Reading<'_>,
    principal: &Principal,
) -> AudienceClass {
    audience_class(
        &tenant.schema,
        reading,
        principal,
        &roles_of(tenant, principal),
    )
}
