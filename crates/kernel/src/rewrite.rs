//! The rewrite that an adaptable schema change is applied together with (D-8212, MIG-560,
//! MIG-608): a rename or a move of a field, and the dependents that read the field it changes.
//!
//! The names follow `lean/Strata/Evolution/Model.lean` and `Rewrite.lean`: a [`Read`] is what a
//! read of one cell becomes ([`cell_sub`], Lean `cellSub`), [`Read`] substitution over an
//! expression is Lean `Read.subst`, and [`rewrite_schema`] is Lean `rewrite` applied to every
//! dependent of the schema. The theorem it carries (`rewrite_correct`) is that each dependent
//! evaluates the same on the migrated data as it did on the data before; here that holds because
//! every site that names the cell is substituted, and a site that cannot be substituted makes the
//! change refused, never applied half-way.
//!
//! The rewrite is one substitution over the sites of each dependent (an expression of the tenant
//! rule language, a field name in a list or a map key), not a rule per kind of dependent: the
//! dependent kinds only say where their sites are. The Lean model identifies fields by a number
//! that survives a rename, so its rename rewrites by the identity; the kernel identifies a field
//! by its name, so the rename here substitutes the name, which is the same rewrite with the
//! identity made explicit.
//!
//! What is not rewritten, because the dependency graph does not read it either: the steps of an
//! automation other than the record steps and the branch, the tenant operations, the templates and
//! the segments. A reference they hold to the old name is found by the consistency check of the
//! schema, which refuses the change; it is never left dangling.
//!
//! Pure: a function of the schema and the change.

use crate::dependencies::{Cell, DependencyGraph, DependentId};
use crate::expr::Expr;
use crate::intent::{DomainError, DomainErrorExt, ErrorKind};
use crate::rules::Lookup;
use crate::state::RecordExt;
use crate::types::{
    ArgSource, AutomationStep, FieldDef, FieldId, FieldTypeExt, ObjectTypeId, RecordId,
    RetentionStart, SchemaChange, Scope, TenantSchema, Timestamp, TriggerEvent, Value, ValueExt,
};
use std::collections::{BTreeMap, BTreeSet};

/// What a read of one cell becomes under a change (Lean `Read`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    /// The cell itself.
    Cell(Cell),
    /// A cell reached through a reference field of the same object type: the cell of a moved
    /// field is read from the record the reference points at.
    Through {
        /// The reference field, on the object type the read starts from.
        reference: Cell,
        /// The cell it reaches.
        target: Cell,
    },
}

/// Whether the change is applied together with a rewrite of its dependents (Lean `isRewriting`).
pub fn is_rewriting(change: &SchemaChange) -> bool {
    matches!(
        change,
        SchemaChange::RenameField { .. } | SchemaChange::MoveField { .. }
    )
}

/// The cells a rewriting change creates, which no dependent read before (Lean `newCells`). The
/// target object type of a move is looked up in `schema`.
pub fn new_cells(change: &SchemaChange, schema: &TenantSchema) -> Vec<Cell> {
    match change {
        SchemaChange::RenameField { object, to, .. } => vec![(object.clone(), to.clone())],
        SchemaChange::MoveField {
            object,
            via,
            to_field,
            ..
        } => reached(schema, object, via)
            .map(|to| vec![(to, to_field.clone())])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Where a read of a cell goes under the change (Lean `cellSub`): a renamed field is read under
/// its new name, a moved field through the reference it moved along, every other cell as before.
pub fn cell_sub(
    change: &SchemaChange,
    schema: &TenantSchema,
    object: &ObjectTypeId,
    field: &FieldId,
) -> Read {
    match change {
        SchemaChange::RenameField {
            object: o,
            from,
            to,
        } if o == object && from == field => Read::Cell((o.clone(), to.clone())),
        SchemaChange::MoveField {
            object: o,
            field: f,
            via,
            to_field,
        } if o == object && f == field => match reached(schema, o, via) {
            Some(to) => Read::Through {
                reference: (o.clone(), via.clone()),
                target: (to, to_field.clone()),
            },
            None => Read::Cell((object.clone(), field.clone())),
        },
        _ => Read::Cell((object.clone(), field.clone())),
    }
}

/// The object type a single reference field of `object` points at.
fn reached(
    schema: &TenantSchema,
    object: &ObjectTypeId,
    reference: &FieldId,
) -> Option<ObjectTypeId> {
    let def = schema.objects.get(object)?.fields.get(reference)?;
    match def.ty.ref_target() {
        Some((to, _, false)) => Some(to.clone()),
        _ => None,
    }
}

/// The checks that make a rewriting change meaningful on a schema (Lean `pre`), with the reason
/// it is refused when they fail.
pub fn check(change: &SchemaChange, schema: &TenantSchema) -> Result<(), String> {
    match change {
        SchemaChange::RenameField { object, from, to } => {
            let def = schema
                .objects
                .get(object)
                .ok_or_else(|| format!("no object type {object}"))?;
            if !def.fields.contains_key(from) {
                return Err(format!("no field {object}.{from}"));
            }
            if from != to && def.fields.contains_key(to) {
                return Err(format!("field {object}.{to} exists"));
            }
            Ok(())
        }
        SchemaChange::MoveField {
            object,
            field,
            via,
            to_field,
        } => {
            let def = schema
                .objects
                .get(object)
                .ok_or_else(|| format!("no object type {object}"))?;
            let moved = def
                .fields
                .get(field)
                .ok_or_else(|| format!("no field {object}.{field}"))?;
            if field == via {
                return Err(format!("{object}.{field} cannot move along itself"));
            }
            let target = reached(schema, object, via).ok_or_else(|| {
                format!("{object}.{via} is not a reference to one record of another object type")
            })?;
            if &target == object {
                return Err(format!(
                    "{object}.{via} points at {object}, the type it moves from"
                ));
            }
            let target_def = schema
                .objects
                .get(&target)
                .ok_or_else(|| format!("no object type {target}"))?;
            if target_def.fields.contains_key(to_field) {
                return Err(format!("field {target}.{to_field} exists"));
            }
            if moved.ty.ref_target().is_some() {
                return Err(format!(
                    "{object}.{field} is a reference; a read through it cannot be followed to {target}.{to_field} in one step"
                ));
            }
            if moved.unique {
                return Err(format!(
                    "{object}.{field} is unique among the records of {object}, which it is not among the records of {target}"
                ));
            }
            if moved.read.is_some() || moved.personal.is_some() {
                return Err(format!(
                    "{object}.{field} carries a read rule or personal data declared over {object}"
                ));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// The field definition after the change, in its new place: renamed, or moved to the object type
/// the reference points at.
pub fn moved_def(def: &FieldDef, name: &FieldId) -> FieldDef {
    FieldDef {
        id: name.clone(),
        ..def.clone()
    }
}

/// A substitution of the cell of one change over the sites of a schema.
struct Sub<'a> {
    change: &'a SchemaChange,
    before: &'a TenantSchema,
}

impl Sub<'_> {
    /// The object type and field the change moves or renames.
    fn cell(&self) -> (&ObjectTypeId, &FieldId) {
        match self.change {
            SchemaChange::RenameField { object, from, .. } => (object, from),
            SchemaChange::MoveField { object, field, .. } => (object, field),
            _ => unreachable!("only a rewriting change is substituted"),
        }
    }

    /// A site that names a field of `object` as a name and nothing else: a list of fields, an
    /// order key, a map key. A rename substitutes it; a move cannot follow the field out of the
    /// object type the site names it on, so it refuses.
    fn name(&self, object: &ObjectTypeId, field: &FieldId) -> Result<FieldId, String> {
        let (o, f) = self.cell();
        if object != o || field != f {
            return Ok(field.clone());
        }
        match self.change {
            SchemaChange::RenameField { to, .. } => Ok(to.clone()),
            _ => Err(format!(
                "reads {object}.{field} as a field of its own record, which it is no longer after the move"
            )),
        }
    }

    /// The same for a field name held as text.
    fn text(&self, object: &ObjectTypeId, field: &str) -> Result<String, String> {
        match FieldId::new(field) {
            Ok(id) => Ok(self.name(object, &id)?.as_str().to_string()),
            Err(_) => Ok(field.to_string()),
        }
    }

    /// The substitution over an expression evaluated on a record of `base` (none where the site
    /// has no record, in which case a read is left as it is).
    fn expr(&self, base: Option<&ObjectTypeId>, expr: &Expr) -> Result<Expr, String> {
        let (o, f) = self.cell();
        Ok(match expr {
            Expr::Var(scope, name) => match base {
                Some(base) if base == o && name == f.as_str() => self.var(*scope, name)?,
                _ => Expr::Var(*scope, name.clone()),
            },
            Expr::Through(scope, reference, target) => {
                let mut reference_out = reference.clone();
                let mut target_out = target.clone();
                if let Some(base) = base {
                    let target_object = FieldId::new(reference)
                        .ok()
                        .and_then(|id| reached(self.before, base, &id));
                    match self.change {
                        SchemaChange::RenameField { to, .. } => {
                            if base == o && reference == f.as_str() {
                                reference_out = to.as_str().to_string();
                            }
                            if target_object.as_ref() == Some(o) && target == f.as_str() {
                                target_out = to.as_str().to_string();
                            }
                        }
                        _ => {
                            if (base == o && reference == f.as_str())
                                || (target_object.as_ref() == Some(o) && target == f.as_str())
                            {
                                return Err(format!(
                                    "reads {o}.{f} through a reference, which would take two steps after the move"
                                ));
                            }
                        }
                    }
                }
                Expr::Through(*scope, reference_out, target_out)
            }
            Expr::Lit(_) | Expr::Param(_) | Expr::Now | Expr::Principal | Expr::HasRole(_) => {
                expr.clone()
            }
            Expr::Not(a) => Expr::Not(Box::new(self.expr(base, a)?)),
            Expr::IsNull(a) => Expr::IsNull(Box::new(self.expr(base, a)?)),
            Expr::Len(a) => Expr::Len(Box::new(self.expr(base, a)?)),
            Expr::Lower(a) => Expr::Lower(Box::new(self.expr(base, a)?)),
            Expr::IsSlug(a) => Expr::IsSlug(Box::new(self.expr(base, a)?)),
            Expr::Text(a) => Expr::Text(Box::new(self.expr(base, a)?)),
            Expr::Duration(unit, a) => Expr::Duration(*unit, Box::new(self.expr(base, a)?)),
            Expr::And(items) => Expr::And(self.exprs(base, items)?),
            Expr::Or(items) => Expr::Or(self.exprs(base, items)?),
            Expr::Cmp(op, a, b) => Expr::Cmp(
                *op,
                Box::new(self.expr(base, a)?),
                Box::new(self.expr(base, b)?),
            ),
            Expr::In(a, b) => {
                Expr::In(Box::new(self.expr(base, a)?), Box::new(self.expr(base, b)?))
            }
            Expr::Contains(a, b) => {
                Expr::Contains(Box::new(self.expr(base, a)?), Box::new(self.expr(base, b)?))
            }
            Expr::StartsWith(a, b) => {
                Expr::StartsWith(Box::new(self.expr(base, a)?), Box::new(self.expr(base, b)?))
            }
            Expr::Arith(op, a, b) => Expr::Arith(
                *op,
                Box::new(self.expr(base, a)?),
                Box::new(self.expr(base, b)?),
            ),
            Expr::MsBetween(a, b) => {
                Expr::MsBetween(Box::new(self.expr(base, a)?), Box::new(self.expr(base, b)?))
            }
            Expr::Concat(a, b) => {
                Expr::Concat(Box::new(self.expr(base, a)?), Box::new(self.expr(base, b)?))
            }
            Expr::If(a, b, c) => Expr::If(
                Box::new(self.expr(base, a)?),
                Box::new(self.expr(base, b)?),
                Box::new(self.expr(base, c)?),
            ),
        })
    }

    fn exprs(&self, base: Option<&ObjectTypeId>, items: &[Expr]) -> Result<Vec<Expr>, String> {
        items.iter().map(|item| self.expr(base, item)).collect()
    }

    /// The read of the cell the change moves or renames, as an expression.
    fn var(&self, scope: Scope, name: &str) -> Result<Expr, String> {
        Ok(match self.change {
            SchemaChange::RenameField { to, .. } => Expr::Var(scope, to.as_str().to_string()),
            SchemaChange::MoveField {
                object,
                via,
                to_field,
                ..
            } => match cell_sub(
                self.change,
                self.before,
                object,
                &FieldId::new(name).map_err(|e| e.to_string())?,
            ) {
                Read::Through { .. } => Expr::Through(
                    scope,
                    via.as_str().to_string(),
                    to_field.as_str().to_string(),
                ),
                Read::Cell(_) => Expr::Var(scope, name.to_string()),
            },
            _ => Expr::Var(scope, name.to_string()),
        })
    }

    fn optional(
        &self,
        base: Option<&ObjectTypeId>,
        expr: &Option<Expr>,
    ) -> Result<Option<Expr>, String> {
        expr.as_ref().map(|e| self.expr(base, e)).transpose()
    }

    fn names(&self, object: &ObjectTypeId, fields: &[FieldId]) -> Result<Vec<FieldId>, String> {
        fields.iter().map(|f| self.name(object, f)).collect()
    }

    fn keys(
        &self,
        object: &ObjectTypeId,
        base: Option<&ObjectTypeId>,
        map: &BTreeMap<FieldId, Expr>,
    ) -> Result<BTreeMap<FieldId, Expr>, String> {
        map.iter()
            .map(|(k, v)| Ok((self.name(object, k)?, self.expr(base, v)?)))
            .collect()
    }
}

/// The base an unbased expression of an automation is read on: the object type of the triggers
/// that start it, when they agree.
fn automation_base(
    schema: &TenantSchema,
    automation: &crate::types::WorkflowId,
) -> Option<ObjectTypeId> {
    let bases: BTreeSet<&ObjectTypeId> = schema
        .triggers
        .iter()
        .flatten()
        .filter(|(_, t)| &t.automation == automation)
        .filter_map(|(_, t)| match &t.on {
            TriggerEvent::RecordCreated { object }
            | TriggerEvent::RecordUpdated { object, .. }
            | TriggerEvent::RecordDeleted { object } => Some(object),
            _ => None,
        })
        .collect();
    if bases.len() == 1 {
        bases.into_iter().next().cloned()
    } else {
        None
    }
}

/// Rewrite every dependent of `schema` that reads the cell the change renames or moves, so that
/// it reads the new cell (Lean `rewrite` over `deps`). `schema` is the schema before the change;
/// the caller then applies the change to the object types. Returns the dependents it rewrote,
/// each once, in the order of the dependency graph: a dependent after the dependents it uses
/// (MIG-608). Fails, leaving `schema` as it was, naming the dependent that has a site the change
/// cannot be followed through.
pub fn rewrite_schema(
    change: &SchemaChange,
    schema: &mut TenantSchema,
) -> Result<Vec<DependentId>, String> {
    if !is_rewriting(change) {
        return Ok(Vec::new());
    }
    check(change, schema)?;
    let before = schema.clone();
    let sub = Sub {
        change,
        before: &before,
    };
    let mut out = before.clone();
    let mut changed: BTreeSet<DependentId> = BTreeSet::new();
    let fail = |id: &DependentId, e: String| format!("{id}: {e}");

    for object in before.objects.values() {
        let oid = &object.id;
        let target = out.objects.get_mut(oid).expect("the object is in the copy");
        for field in object.fields.values() {
            if let Some(rule) = &field.read {
                let id = DependentId::FieldRule(oid.clone(), field.id.clone());
                let new = sub.expr(Some(oid), rule).map_err(|e| fail(&id, e))?;
                if &new != rule {
                    target.fields.get_mut(&field.id).expect("field").read = Some(new);
                    changed.insert(id);
                }
            }
        }
        for constraint in object.constraints.values() {
            let id = DependentId::Constraint(oid.clone(), constraint.id.clone());
            let new = sub
                .expr(Some(oid), &constraint.expr)
                .map_err(|e| fail(&id, e))?;
            if new != constraint.expr {
                target
                    .constraints
                    .get_mut(&constraint.id)
                    .expect("constraint")
                    .expr = new;
                changed.insert(id);
            }
        }
        if let Some(uniques) = &object.unique {
            for unique in uniques.values() {
                let id = DependentId::Unique(oid.clone(), unique.id.clone());
                let fields = sub.names(oid, &unique.fields).map_err(|e| fail(&id, e))?;
                if fields != unique.fields {
                    target
                        .unique
                        .as_mut()
                        .and_then(|u| u.get_mut(&unique.id))
                        .expect("unique")
                        .fields = fields;
                    changed.insert(id);
                }
            }
        }
        // The settings of the object type that name fields.
        let id = DependentId::ObjectSetting(oid.clone());
        let one = |field: &Option<FieldId>| -> Result<Option<FieldId>, String> {
            field.as_ref().map(|f| sub.name(oid, f)).transpose()
        };
        let language = one(&object.language_field).map_err(|e| fail(&id, e))?;
        let group = one(&object.translation_group).map_err(|e| fail(&id, e))?;
        let searchable = object
            .searchable
            .as_ref()
            .map(|fields| sub.names(oid, fields))
            .transpose()
            .map_err(|e| fail(&id, e))?;
        let publication = object
            .publication
            .as_ref()
            .map(|p| -> Result<_, String> {
                let mut p = p.clone();
                p.state_field = sub.name(oid, &p.state_field)?;
                p.time_field = one(&p.time_field)?;
                p.review_run_field = one(&p.review_run_field)?;
                Ok(p)
            })
            .transpose()
            .map_err(|e| fail(&id, e))?;
        let retention = object
            .retention
            .as_ref()
            .map(|r| -> Result<_, String> {
                let mut r = r.clone();
                if let RetentionStart::Field { field } = &r.from {
                    r.from = RetentionStart::Field {
                        field: sub.name(oid, field)?,
                    };
                }
                Ok(r)
            })
            .transpose()
            .map_err(|e| fail(&id, e))?;
        if language != object.language_field
            || group != object.translation_group
            || searchable != object.searchable
            || publication != object.publication
            || retention != object.retention
        {
            target.language_field = language;
            target.translation_group = group;
            target.searchable = searchable;
            target.publication = publication;
            target.retention = retention;
            changed.insert(id);
        }
    }

    for policy in before.policies.values() {
        let id = DependentId::Policy(policy.id.clone());
        if let crate::types::Resource::Object(object) = &policy.resource {
            let new = sub
                .optional(Some(object), &policy.condition)
                .map_err(|e| fail(&id, e))?;
            if new != policy.condition {
                out.policies.get_mut(&policy.id).expect("policy").condition = new;
                changed.insert(id);
            }
        }
    }

    for workflow in before.workflows.values() {
        let id = DependentId::Workflow(workflow.id.clone());
        let object = &workflow.object;
        let mut new = workflow.clone();
        let rewrite = (|| -> Result<(), String> {
            new.field = sub.name(object, &workflow.field)?;
            for (slot, transition) in new.transitions.iter_mut().zip(&workflow.transitions) {
                slot.guard = sub.optional(Some(object), &transition.guard)?;
                slot.set = sub.keys(object, Some(object), &transition.set)?;
            }
            Ok(())
        })();
        rewrite.map_err(|e| fail(&id, e))?;
        if &new != workflow {
            out.workflows.insert(workflow.id.clone(), new);
            changed.insert(id);
        }
    }

    for query in before.queries.values() {
        let id = DependentId::Query(query.id.clone());
        let object = &query.object;
        let mut new = query.clone();
        let rewrite = (|| -> Result<(), String> {
            new.filter = sub.optional(Some(object), &query.filter)?;
            for (slot, key) in new.order.iter_mut().zip(&query.order) {
                slot.field = sub.text(object, &key.field)?;
            }
            if let Some(aggregates) = &query.aggregates {
                let mut rewritten = Vec::new();
                for aggregate in aggregates {
                    let mut a = aggregate.clone();
                    a.field = a.field.as_ref().map(|f| sub.name(object, f)).transpose()?;
                    a.group_by = a
                        .group_by
                        .as_ref()
                        .map(|f| sub.name(object, f))
                        .transpose()?;
                    rewritten.push(a);
                }
                new.aggregates = Some(rewritten);
            }
            if let Some(fields) = &query.declared_fields {
                new.declared_fields = Some(sub.names(object, fields)?);
            }
            Ok(())
        })();
        rewrite.map_err(|e| fail(&id, e))?;
        if &new != query {
            out.queries.insert(query.id.clone(), new);
            changed.insert(id);
        }
    }

    for view in before.views.values() {
        let id = DependentId::View(view.id.clone());
        let mut new = view.clone();
        let rewrite = (|| -> Result<(), String> {
            for (name, section) in &view.sections {
                let slot = new.sections.get_mut(name).expect("section");
                let query_object = before.queries.get(&section.query).map(|q| &q.object);
                if let Some(object) = query_object {
                    slot.include = sub.names(object, &section.include)?;
                    slot.markdown = sub.names(object, &section.markdown)?;
                }
                for (arg, source) in &section.args {
                    if let ArgSource::Section {
                        section: from,
                        attribute,
                    } = source
                    {
                        let source_object = view
                            .sections
                            .get(from)
                            .and_then(|s| before.queries.get(&s.query))
                            .map(|q| &q.object);
                        if let Some(object) = source_object {
                            slot.args.insert(
                                arg.clone(),
                                ArgSource::Section {
                                    section: from.clone(),
                                    attribute: sub.text(object, attribute)?,
                                },
                            );
                        }
                    }
                }
            }
            Ok(())
        })();
        rewrite.map_err(|e| fail(&id, e))?;
        if &new != view {
            out.views.insert(view.id.clone(), new);
            changed.insert(id);
        }
    }

    for trigger in before.triggers.iter().flatten().map(|(_, t)| t) {
        let id = DependentId::Trigger(trigger.id.clone());
        let object = match &trigger.on {
            TriggerEvent::RecordCreated { object }
            | TriggerEvent::RecordUpdated { object, .. }
            | TriggerEvent::RecordDeleted { object } => Some(object),
            _ => None,
        };
        let new = sub
            .optional(object, &trigger.condition)
            .map_err(|e| fail(&id, e))?;
        if new != trigger.condition {
            if let Some(slot) = out.triggers.as_mut().and_then(|t| t.get_mut(&trigger.id)) {
                slot.condition = new;
            }
            changed.insert(id);
        }
    }

    for automation in before.automations.iter().flatten().map(|(_, a)| a) {
        let id = DependentId::Automation(automation.id.clone());
        let unbased = automation_base(&before, &automation.id);
        let mut new = automation.clone();
        let rewrite = (|| -> Result<(), String> {
            new.run_key = sub.optional(unbased.as_ref(), &automation.run_key)?;
            for (name, step) in &automation.steps {
                let slot = new.steps.get_mut(name).expect("step");
                match &step.action {
                    AutomationStep::CreateRecord { object, fields } => {
                        slot.action = AutomationStep::CreateRecord {
                            object: object.clone(),
                            fields: sub.keys(object, unbased.as_ref(), fields)?,
                        };
                    }
                    AutomationStep::UpdateRecord {
                        object,
                        record,
                        fields,
                    } => {
                        slot.action = AutomationStep::UpdateRecord {
                            object: object.clone(),
                            record: sub.expr(unbased.as_ref(), record)?,
                            fields: sub.keys(object, unbased.as_ref(), fields)?,
                        };
                    }
                    AutomationStep::DeleteRecord { object, record } => {
                        slot.action = AutomationStep::DeleteRecord {
                            object: object.clone(),
                            record: sub.expr(unbased.as_ref(), record)?,
                        };
                    }
                    AutomationStep::Branch {
                        condition,
                        object: Some(object),
                        record,
                        then,
                        r#else,
                    } => {
                        slot.action = AutomationStep::Branch {
                            condition: sub.expr(Some(object), condition)?,
                            object: Some(object.clone()),
                            record: sub.optional(unbased.as_ref(), record)?,
                            then: then.clone(),
                            r#else: r#else.clone(),
                        };
                    }
                    _ => {}
                }
            }
            Ok(())
        })();
        rewrite.map_err(|e| fail(&id, e))?;
        if &new != automation {
            if let Some(slot) = out
                .automations
                .as_mut()
                .and_then(|a| a.get_mut(&automation.id))
            {
                *slot = new;
            }
            changed.insert(id);
        }
    }

    *schema = out;
    Ok(in_dependency_order(&before, changed))
}

/// The rewritten dependents, each once, a dependent after those it uses (MIG-608): the order of
/// the dependency graph, ties broken by the identity of the dependent.
fn in_dependency_order(before: &TenantSchema, set: BTreeSet<DependentId>) -> Vec<DependentId> {
    let graph = DependencyGraph::of(before);
    let mut remaining = set;
    let mut ordered = Vec::new();
    while !remaining.is_empty() {
        let ready: Vec<DependentId> = remaining
            .iter()
            .filter(|id| {
                graph
                    .reads
                    .get(*id)
                    .map(|r| r.uses.iter().all(|u| !remaining.contains(u) || u == *id))
                    .unwrap_or(true)
            })
            .cloned()
            .collect();
        // A cycle cannot occur in a valid schema; if it did, take the rest in identity order.
        let ready = if ready.is_empty() {
            remaining.iter().cloned().collect()
        } else {
            ready
        };
        for id in ready {
            remaining.remove(&id);
            ordered.push(id);
        }
    }
    ordered
}

/// The records after a rename or a move, for the object type `object`, as far as one record can
/// say: a renamed field is carried under its new name. The records of the object type a move
/// takes the field to are filled by the migration, which reads the records that point at them.
pub fn carry_fields(
    changes: &[SchemaChange],
    object: &ObjectTypeId,
    fields: &BTreeMap<FieldId, Value>,
    now: Timestamp,
) -> Result<BTreeMap<FieldId, Value>, String> {
    let mut out = fields.clone();
    for change in changes {
        match change {
            SchemaChange::RenameField {
                object: o,
                from,
                to,
            } if o == object && from != to => {
                if let Some(value) = out.remove(from) {
                    out.insert(to.clone(), value);
                }
            }
            SchemaChange::RetypeField {
                object: o,
                field,
                encode,
                ..
            } if o == object => {
                if let Some(value) = out.get(field).filter(|v| !v.is_null()) {
                    let encoded = crate::evolution::recode(encode, value, now)
                        .map_err(|e| format!("re-encoding {object}.{field}: {e}"))?;
                    out.insert(field.clone(), encoded);
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// The values a move brings to the records of the object type it moves a field to, by record:
/// the value each record holds in the field, put on the record its reference points at. Refused
/// where that loses a value (MIG-560 keeps values): a record that holds a value but points at no
/// record, or two records that point at the same record with different values. A move is
/// decided on the records as they stand before the change list, so a field that an earlier change
/// of the same list renames or moves belongs in a change of its own.
pub fn plan_moves(
    lookup: Lookup<'_>,
    changes: &[SchemaChange],
    base: &TenantSchema,
) -> Result<BTreeMap<(ObjectTypeId, RecordId), BTreeMap<FieldId, Value>>, DomainError> {
    let mut out: BTreeMap<(ObjectTypeId, RecordId), BTreeMap<FieldId, Value>> = BTreeMap::new();
    for (index, change) in changes.iter().enumerate() {
        let SchemaChange::MoveField {
            object,
            field,
            via,
            to_field,
        } = change
        else {
            continue;
        };
        let refuse = |message: String| DomainError::new(ErrorKind::MigrationRejected, message);
        let earlier_touch = changes[..index].iter().any(|c| match c {
            SchemaChange::RenameField {
                object: o,
                from,
                to,
            } => o == object && (from == field || from == via || to == field || to == via),
            SchemaChange::MoveField {
                object: o,
                field: f,
                via: v,
                ..
            } => o == object && (f == field || f == via || v == via),
            SchemaChange::RetypeField {
                object: o,
                field: f,
                ..
            } => o == object && (f == field || f == via),
            _ => false,
        });
        if earlier_touch {
            return Err(refuse(format!(
                "move of {object}.{field}: an earlier change of the list renames, moves or retypes it or {via}; make the move a change of its own"
            )));
        }
        let Some(target) = reached(base, object, via) else {
            continue;
        };
        let mut seen: BTreeMap<RecordId, (RecordId, Value)> = BTreeMap::new();
        for record in lookup.all_of(object)? {
            let value = record.get(field).clone();
            match record.get(via) {
                Value::Ref(parent) => {
                    if let Some((other, held)) = seen.get(parent) {
                        if held != &value {
                            return Err(refuse(format!(
                                "move of {object}.{field}: records {other} and {} of {object} point at record {parent} of {target} with different values, and {target}.{to_field} holds one",
                                record.id
                            )));
                        }
                    } else {
                        seen.insert(*parent, (record.id, value.clone()));
                    }
                }
                _ => {
                    if !value.is_null() {
                        return Err(refuse(format!(
                            "move of {object}.{field}: record {} of {object} holds a value but {via} points at no {target}; the value would be lost",
                            record.id
                        )));
                    }
                }
            }
        }
        for (parent, (_, value)) in seen {
            if !value.is_null() {
                out.entry((target.clone(), parent))
                    .or_default()
                    .insert(to_field.clone(), value);
            }
        }
    }
    Ok(out)
}
