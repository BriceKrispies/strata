//! The dependency graph of a tenant schema, and the class and impact of a schema change
//! (D-8210, D-8212, D-8217; INT-2910, MIG-589, MIG-592, MIG-604).
//!
//! The graph is computed from the definitions and never declared: [`DependencyGraph::of`] reads
//! each dependent (a query, a policy, a constraint, a composite unique constraint, a workflow, a
//! trigger, an automation, a view, a reference field, the read rule of a field, the settings of an
//! object type that name fields) and records what it reads at the level of a field: the
//! [`Cell`]s `(object type, field)` it names, the object types it is evaluated over or points at,
//! and the other dependents it uses. A read through a reference counts both cells, the reference
//! field and the field it reaches.
//!
//! The names follow `lean/Strata/Evolution/Model.lean` so that a later correspondence test can
//! compare the two: a [`Cell`] is an (object id, field id) pair, [`Reads`] is a dependent's read
//! set, [`footprint`] is the cells a change touches, [`impact`] is the dependents that read them
//! (through other dependents too), [`classify`] is the class, and [`ChangeClass`] has the same
//! three classes. The Lean model identifies entities by a number; here the identity of an entity
//! is its name, and a name that is removed and defined again in one change is treated as one
//! entity that the change touched, so a dependent never binds to a different entity that takes the
//! name of the first (see [`impact_between`]).
//!
//! Where the Lean model and the specification differ (the Lean model calls an added field
//! adaptable and a rename compatible), this module follows the specification: add-field and
//! widening are compatible, and a rename or a move of a field is adaptable
//! ([`crate::rewrite`] rewrites the dependents it reaches).
//!
//! Everything here is a pure function of schemas and changes.

use crate::expr::Expr;
use crate::types::{
    AutomationStep, ConstraintId, FieldDef, FieldId, FieldType, FieldTypeExt, ObjectTypeId,
    PolicyId, QueryId, Resource, SchemaChange, TenantSchema, TriggerEvent, TriggerId, ViewId,
    WorkflowId,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// A field of an object type: the unit a dependent's read set is stated in.
pub type Cell = (ObjectTypeId, FieldId);

/// An entity of the schema that reads other entities: a node of the dependency graph.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DependentId {
    /// A named query: its filter, order, aggregates and declared fields.
    Query(QueryId),
    /// A policy: its condition.
    Policy(PolicyId),
    /// A workflow of an object type: its field, guards and the fields its transitions set.
    Workflow(WorkflowId),
    /// A trigger: its event, its condition and the automation it starts.
    Trigger(TriggerId),
    /// An automation: the fields its steps write and read.
    Automation(WorkflowId),
    /// A view: the queries its sections use and the fields it includes.
    View(ViewId),
    /// A constraint of an object type.
    Constraint(ObjectTypeId, ConstraintId),
    /// A composite unique constraint of an object type.
    Unique(ObjectTypeId, ConstraintId),
    /// The read rule of a field, an expression over its own object type.
    FieldRule(ObjectTypeId, FieldId),
    /// A reference field: it depends on the object type it points at.
    Reference(ObjectTypeId, FieldId),
    /// The settings of an object type that name fields: language, translation group, search,
    /// publication state.
    ObjectSetting(ObjectTypeId),
}

impl fmt::Display for DependentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DependentId::Query(id) => write!(f, "query {id}"),
            DependentId::Policy(id) => write!(f, "policy {id}"),
            DependentId::Workflow(id) => write!(f, "workflow {id}"),
            DependentId::Trigger(id) => write!(f, "trigger {id}"),
            DependentId::Automation(id) => write!(f, "automation {id}"),
            DependentId::View(id) => write!(f, "view {id}"),
            DependentId::Constraint(object, id) => write!(f, "constraint {object}.{id}"),
            DependentId::Unique(object, id) => write!(f, "unique constraint {object}.{id}"),
            DependentId::FieldRule(object, field) => write!(f, "read rule of {object}.{field}"),
            DependentId::Reference(object, field) => write!(f, "reference {object}.{field}"),
            DependentId::ObjectSetting(object) => write!(f, "settings of {object}"),
        }
    }
}

/// What a dependent reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reads {
    /// The object types the dependent is evaluated over or points at.
    pub objects: BTreeSet<ObjectTypeId>,
    /// The fields it names, read or written.
    pub cells: BTreeSet<Cell>,
    /// The other dependents it uses: a view uses queries, a trigger uses an automation.
    pub uses: BTreeSet<DependentId>,
}

impl Reads {
    fn object(&mut self, object: &ObjectTypeId) {
        self.objects.insert(object.clone());
    }

    fn cell(&mut self, object: &ObjectTypeId, field: &str) {
        if let Ok(field) = FieldId::new(field) {
            self.cells.insert((object.clone(), field));
        }
    }

    fn field(&mut self, object: &ObjectTypeId, field: &FieldId) {
        self.cells.insert((object.clone(), field.clone()));
    }

    /// The fields an expression reads, with `base` the object type of the record it is evaluated
    /// on (none where the site has no record: a default, an automation step outside a branch).
    fn expr(&mut self, schema: &TenantSchema, base: Option<&ObjectTypeId>, expr: &Expr) {
        match expr {
            Expr::Var(_, name) => {
                if let (Some(base), false) = (base, name.starts_with('$')) {
                    self.cell(base, name);
                }
            }
            Expr::Through(_, reference, target) => {
                if let Some(base) = base {
                    self.cell(base, reference);
                    let reached = FieldId::new(reference)
                        .ok()
                        .and_then(|id| schema.objects.get(base)?.fields.get(&id))
                        .and_then(|def| def.ty.ref_target().map(|(to, _, _)| to.clone()));
                    if let Some(to) = reached {
                        self.cell(&to, target);
                        self.objects.insert(to);
                    }
                }
            }
            Expr::Lit(_) | Expr::Param(_) | Expr::Now | Expr::Principal | Expr::HasRole(_) => {}
            Expr::Not(a)
            | Expr::IsNull(a)
            | Expr::Len(a)
            | Expr::Lower(a)
            | Expr::IsSlug(a)
            | Expr::Duration(_, a)
            | Expr::Text(a) => self.expr(schema, base, a),
            Expr::And(items) | Expr::Or(items) => {
                for item in items {
                    self.expr(schema, base, item);
                }
            }
            Expr::Cmp(_, a, b)
            | Expr::In(a, b)
            | Expr::Contains(a, b)
            | Expr::StartsWith(a, b)
            | Expr::Arith(_, a, b)
            | Expr::MsBetween(a, b)
            | Expr::Concat(a, b) => {
                self.expr(schema, base, a);
                self.expr(schema, base, b);
            }
            Expr::If(a, b, c) => {
                self.expr(schema, base, a);
                self.expr(schema, base, b);
                self.expr(schema, base, c);
            }
        }
    }
}

/// The read set of every dependent of a schema, computed from the definitions (D-8210).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencyGraph {
    /// The reads of each dependent.
    pub reads: BTreeMap<DependentId, Reads>,
}

impl DependencyGraph {
    /// The graph of a schema.
    pub fn of(schema: &TenantSchema) -> Self {
        let mut reads = BTreeMap::new();
        for object in schema.objects.values() {
            for field in object.fields.values() {
                if let Some((target, _, _)) = field.ty.ref_target() {
                    let mut r = Reads::default();
                    r.object(target);
                    reads.insert(
                        DependentId::Reference(object.id.clone(), field.id.clone()),
                        r,
                    );
                }
                if let Some(rule) = &field.read {
                    let mut r = Reads::default();
                    r.object(&object.id);
                    r.expr(schema, Some(&object.id), rule);
                    reads.insert(
                        DependentId::FieldRule(object.id.clone(), field.id.clone()),
                        r,
                    );
                }
            }
            for constraint in object.constraints.values() {
                let mut r = Reads::default();
                r.object(&object.id);
                r.expr(schema, Some(&object.id), &constraint.expr);
                reads.insert(
                    DependentId::Constraint(object.id.clone(), constraint.id.clone()),
                    r,
                );
            }
            for unique in crate::definitions::uniques(object) {
                let mut r = Reads::default();
                r.object(&object.id);
                for field in &unique.fields {
                    r.field(&object.id, field);
                }
                reads.insert(DependentId::Unique(object.id.clone(), unique.id.clone()), r);
            }
            let mut settings = Reads::default();
            for field in object
                .language_field
                .iter()
                .chain(object.translation_group.iter())
                .chain(object.searchable.iter().flatten())
                .chain(object.publication.iter().map(|p| &p.state_field))
            {
                settings.field(&object.id, field);
            }
            if !settings.cells.is_empty() {
                settings.object(&object.id);
                reads.insert(DependentId::ObjectSetting(object.id.clone()), settings);
            }
        }
        for policy in schema.policies.values() {
            let mut r = Reads::default();
            if let Resource::Object(object) = &policy.resource {
                r.object(object);
                if let Some(condition) = &policy.condition {
                    r.expr(schema, Some(object), condition);
                }
            }
            reads.insert(DependentId::Policy(policy.id.clone()), r);
        }
        for workflow in schema.workflows.values() {
            let object = &workflow.object;
            let mut r = Reads::default();
            r.object(object);
            r.field(object, &workflow.field);
            for transition in &workflow.transitions {
                if let Some(guard) = &transition.guard {
                    r.expr(schema, Some(object), guard);
                }
                for (field, value) in &transition.set {
                    r.field(object, field);
                    r.expr(schema, Some(object), value);
                }
            }
            reads.insert(DependentId::Workflow(workflow.id.clone()), r);
        }
        for query in schema.queries.values() {
            let mut r = Reads::default();
            r.object(&query.object);
            if let Some(filter) = &query.filter {
                r.expr(schema, Some(&query.object), filter);
            }
            for key in &query.order {
                r.cell(&query.object, &key.field);
            }
            for aggregate in query.aggregates.iter().flatten() {
                for field in aggregate.field.iter().chain(aggregate.group_by.iter()) {
                    r.field(&query.object, field);
                }
            }
            for field in query.declared_fields.iter().flatten() {
                r.field(&query.object, field);
            }
            reads.insert(DependentId::Query(query.id.clone()), r);
        }
        for view in schema.views.values() {
            let mut r = Reads::default();
            for section in view.sections.values() {
                r.uses.insert(DependentId::Query(section.query.clone()));
                if let Some(query) = schema.queries.get(&section.query) {
                    for field in section.include.iter().chain(section.markdown.iter()) {
                        r.field(&query.object, field);
                    }
                }
            }
            reads.insert(DependentId::View(view.id.clone()), r);
        }
        for trigger in schema.triggers.iter().flatten().map(|(_, t)| t) {
            let mut r = Reads::default();
            r.uses
                .insert(DependentId::Automation(trigger.automation.clone()));
            let object = match &trigger.on {
                TriggerEvent::RecordCreated { object }
                | TriggerEvent::RecordUpdated { object, .. }
                | TriggerEvent::RecordDeleted { object } => Some(object),
                _ => None,
            };
            if let Some(object) = object {
                r.object(object);
                if let TriggerEvent::RecordUpdated {
                    transition: Some(name),
                    ..
                } = &trigger.on
                {
                    for workflow in schema.workflows.values().filter(|w| &w.object == object) {
                        if workflow.transitions.iter().any(|t| &t.name == name) {
                            r.uses.insert(DependentId::Workflow(workflow.id.clone()));
                        }
                    }
                }
                if let Some(condition) = &trigger.condition {
                    r.expr(schema, Some(object), condition);
                }
            }
            reads.insert(DependentId::Trigger(trigger.id.clone()), r);
        }
        for automation in schema.automations.iter().flatten().map(|(_, a)| a) {
            let mut r = Reads::default();
            for step in automation.steps.values() {
                match &step.action {
                    AutomationStep::CreateRecord { object, fields } => {
                        r.object(object);
                        for (field, value) in fields {
                            r.field(object, field);
                            r.expr(schema, None, value);
                        }
                    }
                    AutomationStep::UpdateRecord {
                        object,
                        record,
                        fields,
                    } => {
                        r.object(object);
                        r.expr(schema, None, record);
                        for (field, value) in fields {
                            r.field(object, field);
                            r.expr(schema, None, value);
                        }
                    }
                    AutomationStep::DeleteRecord { object, record } => {
                        r.object(object);
                        r.expr(schema, None, record);
                    }
                    AutomationStep::Branch {
                        condition,
                        object: Some(object),
                        ..
                    } => {
                        r.object(object);
                        r.expr(schema, Some(object), condition);
                    }
                    AutomationStep::RequestApproval { policy, .. } => {
                        r.uses.insert(DependentId::Policy(policy.clone()));
                    }
                    _ => {}
                }
            }
            reads.insert(DependentId::Automation(automation.id.clone()), r);
        }
        DependencyGraph { reads }
    }

    /// The dependents that read the cell directly.
    pub fn readers_of(&self, cell: &Cell) -> BTreeSet<DependentId> {
        self.reads
            .iter()
            .filter(|(_, r)| r.cells.contains(cell))
            .map(|(id, _)| id.clone())
            .collect()
    }
}

/// The class of a schema change, computed from what it does to the entities it changes and never
/// stated by its author (D-8212, MIG-589).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChangeClass {
    /// Leaves each valid dependent valid, evaluating the same on the data that exists: add a
    /// field, widen a type, define or replace a definition.
    Compatible,
    /// Applied together with a rewrite of the dependents (rename, move; a total split is not a
    /// change of the kernel yet): [`crate::rewrite::rewrite_schema`].
    Adaptable,
    /// Cannot be followed by a rewrite: remove, narrow, retarget, turn a single value into a list,
    /// re-encode. Refused where it reaches a dependent, unless every dependent it reaches is
    /// pinned and the change has a left inverse ([`crate::evolution`]).
    Breaking,
}

impl ChangeClass {
    /// The word the evidence and the preview use.
    pub fn name(self) -> &'static str {
        match self {
            ChangeClass::Compatible => "compatible",
            ChangeClass::Adaptable => "adaptable",
            ChangeClass::Breaking => "breaking",
        }
    }
}

/// Whether a field type may become another with every stored value still meaning the same and
/// every dependent still reading it as before: the same type (a reference keeps its target; its
/// deletion rule is not read by a dependent), or a language tag read as text.
fn same_or_wider(old: &FieldType, new: &FieldType) -> bool {
    match (old, new) {
        (FieldType::Ref { target: a, .. }, FieldType::Ref { target: b, .. }) => a == b,
        (FieldType::List(a), FieldType::List(b)) => same_or_wider(a, b),
        (FieldType::Language, FieldType::Str) => true,
        (a, b) => a == b,
    }
}

/// The class of defining a field with type `ty` where `existing` is the field it replaces.
fn field_class(existing: Option<&FieldDef>, ty: &FieldType) -> ChangeClass {
    match existing {
        Some(old) if !same_or_wider(&old.ty, ty) => ChangeClass::Breaking,
        _ => ChangeClass::Compatible,
    }
}

fn existing_field<'a>(
    schema: &'a TenantSchema,
    object: &ObjectTypeId,
    field: &FieldId,
) -> Option<&'a FieldDef> {
    schema.objects.get(object)?.fields.get(field)
}

/// The class of a change against the schema it is applied to.
///
/// A removal is breaking. Defining a field that exists is breaking when the type is not the same
/// or wider (narrowing, another reference target, a single value into a list, any other type),
/// and compatible otherwise; whether the field is required or unique, its label, default and read
/// rule are not read by a dependent, and the records that no longer conform are the existing-data
/// checks of the change, not a break of a dependent. Defining anything else, or a setting of an
/// object type, is compatible: what the new definition reads is checked with the whole schema.
pub fn classify(change: &SchemaChange, schema: &TenantSchema) -> ChangeClass {
    use SchemaChange::*;
    match change {
        DefineField { object, field } => {
            field_class(existing_field(schema, object, &field.id), &field.ty)
        }
        DefineRelationship {
            object,
            field,
            target,
            on_delete,
            ..
        } => field_class(
            existing_field(schema, object, field),
            &FieldType::Ref {
                target: target.clone(),
                on_delete: *on_delete,
            },
        ),
        RenameField { .. } | MoveField { .. } => ChangeClass::Adaptable,
        RetypeField { .. }
        | RemoveField { .. }
        | RemoveObject { .. }
        | RemoveConstraint { .. }
        | RemoveUnique { .. }
        | RemovePolicy(_)
        | RemoveWorkflow(_)
        | RemoveQuery(_)
        | RemoveView(_)
        | RemoveTrigger(_)
        | RemoveAutomation(_)
        | RemoveOperation(_)
        | RemoveSetting(_)
        | RemoveCapability(_)
        | RemoveLedgerDefinition(_)
        | RemoveRole(_)
        | RemoveMailTemplate(_)
        | RemoveNoticeTemplate(_)
        | RemoveToken(_)
        | RemoveUiElement(_)
        | RemoveProduct(_) => ChangeClass::Breaking,
        DefineObject { .. }
        | DefineConstraint { .. }
        | DefineRole(_)
        | DefineRolePermissions { .. }
        | DefinePolicy(_)
        | DefineWorkflow(_)
        | DefineQuery(_)
        | DefineView(_)
        | DefineTrigger(_)
        | DefineAutomation(_)
        | DefineOperation(_)
        | DefineUnique { .. }
        | DefineSetting(_)
        | DefineCapability(_)
        | DeclareLedgerUnit(_)
        | DeclareLedgerPool(_)
        | SetLedgerLimit(_)
        | SetRetention { .. }
        | ClearRetention { .. }
        | SetPublication { .. }
        | ClearPublication { .. }
        | SetLanguageField { .. }
        | ClearLanguageField { .. }
        | SetTranslationGroup { .. }
        | ClearTranslationGroup { .. }
        | DefineMailTemplate(_)
        | DefineNoticeTemplate(_)
        | DefineToken(_)
        | DefineUiElement { .. }
        | RecordProduct(_)
        | SetSearchable { .. }
        | ClearSearchable { .. } => ChangeClass::Compatible,
    }
}

/// What a change touches (D-8216): the cells, the object types and the dependents it changes or
/// removes. The impact set is what reads these.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Footprint {
    /// Existing fields the change redefines or removes. Adding a field touches none.
    pub cells: BTreeSet<Cell>,
    /// Object types the change removes.
    pub objects: BTreeSet<ObjectTypeId>,
    /// Dependents the change defines again or removes. They are not in their own impact.
    pub entities: BTreeSet<DependentId>,
    /// Cells the change creates under a new name or in a new place (rename, move). No dependent
    /// reads them before the change, so they reach nothing; two changes that create or touch the
    /// same cell overlap all the same (see [`crate::evolution::rebase`]).
    pub created: BTreeSet<Cell>,
}

/// The dependent a change defines, replaces or removes, if it is one of the graph.
fn entity_of(change: &SchemaChange) -> Option<DependentId> {
    use SchemaChange::*;
    Some(match change {
        DefineConstraint { object, constraint } => {
            DependentId::Constraint(object.clone(), constraint.id.clone())
        }
        RemoveConstraint { object, constraint } => {
            DependentId::Constraint(object.clone(), constraint.clone())
        }
        DefineUnique { object, unique } => DependentId::Unique(object.clone(), unique.id.clone()),
        RemoveUnique { object, unique } => DependentId::Unique(object.clone(), unique.clone()),
        DefinePolicy(def) => DependentId::Policy(def.id.clone()),
        RemovePolicy(id) => DependentId::Policy(id.clone()),
        DefineWorkflow(def) => DependentId::Workflow(def.id.clone()),
        RemoveWorkflow(id) => DependentId::Workflow(id.clone()),
        DefineQuery(def) => DependentId::Query(def.id.clone()),
        RemoveQuery(id) => DependentId::Query(id.clone()),
        DefineView(def) => DependentId::View(def.id.clone()),
        RemoveView(id) => DependentId::View(id.clone()),
        DefineTrigger(def) => DependentId::Trigger(def.id.clone()),
        RemoveTrigger(id) => DependentId::Trigger(id.clone()),
        DefineAutomation(def) => DependentId::Automation(def.id.clone()),
        RemoveAutomation(id) => DependentId::Automation(id.clone()),
        _ => return None,
    })
}

/// The footprint of a change against the schema it is applied to.
///
/// Redefining or removing a field touches its cell, adding one touches none (a dependent cannot
/// read a field that does not exist). Removing an object type touches the type and every cell of
/// it. Defining again or removing a dependent touches that dependent.
pub fn footprint(change: &SchemaChange, schema: &TenantSchema) -> Footprint {
    let mut out = Footprint::default();
    match change {
        SchemaChange::DefineField { object, field } => {
            if existing_field(schema, object, &field.id).is_some_and(|old| old != field) {
                out.cells.insert((object.clone(), field.id.clone()));
            }
        }
        SchemaChange::DefineRelationship { object, field, .. } => {
            if existing_field(schema, object, field).is_some() {
                out.cells.insert((object.clone(), field.clone()));
            }
        }
        SchemaChange::RemoveField { object, field } => {
            out.cells.insert((object.clone(), field.clone()));
        }
        SchemaChange::RetypeField { object, field, .. } => {
            if existing_field(schema, object, field).is_some() {
                out.cells.insert((object.clone(), field.clone()));
            }
        }
        SchemaChange::RenameField { object, from, to } => {
            out.cells.insert((object.clone(), from.clone()));
            out.created.insert((object.clone(), to.clone()));
        }
        SchemaChange::MoveField {
            object,
            field,
            via,
            to_field,
        } => {
            out.cells.insert((object.clone(), field.clone()));
            if let Some(to) = existing_field(schema, object, via)
                .and_then(|def| def.ty.ref_target().map(|(to, _, _)| to.clone()))
            {
                out.created.insert((to, to_field.clone()));
            }
        }
        SchemaChange::RemoveObject { id, .. } => {
            out.objects.insert(id.clone());
            if let Some(object) = schema.objects.get(id) {
                for field in object.fields.keys() {
                    out.cells.insert((id.clone(), field.clone()));
                }
            }
        }
        other => {
            out.entities.extend(entity_of(other));
        }
    }
    out
}

/// The dependents a footprint reaches, directly or through other dependents, as the change leaves
/// them (D-8212, D-8217).
///
/// A dependent is reached only if it is in both graphs and its binding survives the change: it
/// reads a touched cell, or is evaluated over a removed object type, in the schema before and in
/// the schema after. A dependent that reads a name in both is bound to it in both, so a field
/// that is removed and defined again by one change reaches every dependent that reads its name:
/// the new field is another entity, and the dependent was not validated against it. A dependent
/// the change itself removes or rewrites away, or defines for the first time, is not reached; it
/// is not bound to anything the change takes from it. Dependents that use a reached dependent, or
/// a dependent the change touches, are reached in turn.
pub fn impact_between(
    footprint: &Footprint,
    before: &DependencyGraph,
    after: &DependencyGraph,
) -> BTreeSet<DependentId> {
    let bound: Vec<(&DependentId, &Reads, &Reads)> = before
        .reads
        .iter()
        .filter(|(id, _)| !footprint.entities.contains(*id))
        .filter_map(|(id, was)| after.reads.get(id).map(|now| (id, was, now)))
        .collect();
    let mut reached: BTreeSet<DependentId> = BTreeSet::new();
    for (id, was, now) in &bound {
        let cell = was
            .cells
            .intersection(&now.cells)
            .any(|c| footprint.cells.contains(c));
        let object = was
            .objects
            .intersection(&now.objects)
            .any(|o| footprint.objects.contains(o));
        if cell || object {
            reached.insert((*id).clone());
        }
    }
    loop {
        let mut grew = false;
        for (id, was, now) in &bound {
            if reached.contains(*id) {
                continue;
            }
            let through = was
                .uses
                .intersection(&now.uses)
                .any(|u| reached.contains(u) || footprint.entities.contains(u));
            if through {
                reached.insert((*id).clone());
                grew = true;
            }
        }
        if !grew {
            return reached;
        }
    }
}

/// The impact set of a change on a schema: the dependents of the schema that the change reaches.
pub fn impact(change: &SchemaChange, schema: &TenantSchema) -> BTreeSet<DependentId> {
    let graph = DependencyGraph::of(schema);
    impact_between(&footprint(change, schema), &graph, &graph)
}

/// The class and impact of one change of a list, as it is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assessment {
    /// What the change touches, against the schema it was applied to.
    pub footprint: Footprint,
    /// For an adaptable change, the dependents it rewrote, each once, a dependent after those it
    /// uses (MIG-608). Empty for any other change.
    pub rewritten: Vec<DependentId>,
    /// What the change does, as the schema change summary words it.
    pub description: String,
    /// Its class, decided against the schema as it stands when the change is applied.
    pub class: ChangeClass,
    /// The dependents it reaches that are still bound to what it touched in the schema after the
    /// whole list.
    pub impact: BTreeSet<DependentId>,
}
