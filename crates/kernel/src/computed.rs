//! Kernel-computed fields: a field whose value the kernel derives from its record and the record
//! that record refers to (D-8307; KRN-2924 to KRN-2926).
//!
//! A field declares `computed`, an expression of the tenant rule language. The kernel stores the
//! value on the record like any other field, so every reader, the client included, reads it the
//! same way. The value is recomputed:
//!
//! - in the commit of every write that changes one of its inputs: the record's own stored fields,
//!   and the stored fields of the one record that a reference of the record follows ([`values`]
//!   for the written record, [`refer`] for the records that refer to a written one);
//! - on the clock, for a field that reads `now`: [`decide_refresh`] examines one batch of records
//!   of an object type from a cursor and writes only the records whose value changed.
//!
//! What a computed expression may read is limited so that one stored value is right for every
//! reader and one recomputation pass per record is enough ([`problems`]): no parameter, no
//! principal and no role, since those make the value depend on who reads it (a reply count that
//! excludes replies the reader may not see is not a computed field; WG-5599 serves it); no other
//! computed field of its record and no computed field of the record it follows, so there is no
//! cascade; no `$version` or `$updated_at`, which the recomputation itself changes; and no system
//! attribute through a reference. A computed field has no default, is not unique and is not
//! required, and no operation, workflow or automation step writes it ([`writer_problems`],
//! [`written`]). A write that would recompute more referring records than
//! [`limits::MAX_COMPUTED_FANOUT`] is refused, never cut short.
//!
//! Everything here is a pure function of the schema, the snapshot and the context.

use crate::erasure::BATCH;
use crate::expr::{self, Env, Shape, TypeEnv};
use crate::intent::{
    Check, CheckExt, Context, DeltaOp, DomainError, DomainErrorExt, ErrorKind, EventBody, Output,
};
use crate::limits;
use crate::operations::children;
use crate::rules::{missing_error, validate_record, Lookup};
use crate::state::{Principal, Record, RecordExt, TenantStateExt};
use crate::types::{
    AutomationStep, Expr, FieldId, FieldType, FieldTypeExt, ObjectTypeDef, ObjectTypeId, RecordId,
    RoleId, Scope, TenantSchema, Timestamp, Value, ValueExt,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The result of a decision that writes records: the operations, the events and the checks.
#[derive(Default)]
pub struct Recomputed {
    pub ops: Vec<DeltaOp>,
    pub events: Vec<EventBody>,
    pub evidence: Vec<Check>,
}

/// The result of one refresh of time-dependent computed fields.
pub struct Refresh {
    pub recomputed: Recomputed,
    pub output: Output,
}

/// Every node of an expression, the expression itself first.
fn nodes(expr: &Expr) -> Vec<&Expr> {
    let mut out = vec![expr];
    for child in children(expr) {
        out.extend(nodes(child));
    }
    out
}

/// Whether an expression reads the clock.
pub fn reads_now(expr: &Expr) -> bool {
    nodes(expr)
        .into_iter()
        .any(|node| matches!(node, Expr::Now))
}

/// The fields of the object a reference field `reference` of the same record reads, through
/// `Through(Record, reference, field)`.
fn through_names(expr: &Expr, reference: &FieldId) -> BTreeSet<String> {
    nodes(expr)
        .into_iter()
        .filter_map(|node| match node {
            Expr::Through(Scope::Record, via, name) if via == reference.as_str() => {
                Some(name.clone())
            }
            _ => None,
        })
        .collect()
}

/// Whether a computed field holds a value of the shape its type takes.
fn fits(shape: Shape, ty: &FieldType) -> bool {
    let want = match ty {
        FieldType::Str | FieldType::File { .. } | FieldType::Language => Shape::Str,
        FieldType::Int => Shape::Int,
        FieldType::Bool => Shape::Bool,
        FieldType::Time => Shape::Time,
        FieldType::Ref { .. } => Shape::Ref,
        FieldType::List(_) => Shape::List,
    };
    shape == want || shape == Shape::Any
}

/// The problems of the computed fields of one object type, for the schema check. `roles` are the
/// roles the schema defines, which the expression may name.
pub fn problems(
    schema: &TenantSchema,
    object: &ObjectTypeDef,
    roles: &BTreeSet<RoleId>,
    out: &mut Vec<String>,
) {
    let no_params = BTreeSet::new();
    for field in object.fields.values() {
        let Some(expr) = &field.computed else {
            continue;
        };
        let at = format!("{}.{}", object.id, field.id);
        if field.default.is_some() {
            out.push(format!(
                "{at}: a computed field declares no default (D-8307)"
            ));
        }
        if field.unique {
            out.push(format!("{at}: a computed field cannot be unique (D-8307)"));
        }
        if field.required {
            out.push(format!(
                "{at}: a computed field cannot be required (D-8307)"
            ));
        }
        let env = TypeEnv {
            fields: &object.fields,
            params: &no_params,
            roles,
            objects: &schema.objects,
        };
        match expr::check(expr, &env) {
            Ok(shape) if fits(shape, &field.ty) => {}
            Ok(shape) => out.push(format!(
                "{at}: computed is {shape:?}, which the type of the field does not hold"
            )),
            Err(e) => out.push(format!("{at}: computed: {e}")),
        }
        for node in nodes(expr) {
            match node {
                Expr::Param(name) => out.push(format!(
                    "{at}: computed reads parameter {name}, which no record supplies (D-8307)"
                )),
                Expr::Principal | Expr::HasRole(_) => out.push(format!(
                    "{at}: computed reads the principal or a role, so one stored value would not be right for every reader (D-8307)"
                )),
                Expr::Var(scope, name) => {
                    if !matches!(scope, Scope::Record) {
                        out.push(format!(
                            "{at}: computed reads the old or new record, which a recomputation does not have"
                        ));
                    }
                    if name == "$version" || name == "$updated_at" {
                        out.push(format!(
                            "{at}: computed reads {name}, which its own recomputation changes (D-8307)"
                        ));
                    }
                    if FieldId::new(name)
                        .ok()
                        .and_then(|id| object.fields.get(&id))
                        .is_some_and(|other| other.computed.is_some())
                    {
                        out.push(format!(
                            "{at}: computed reads computed field {name}; a computed field reads stored fields only (D-8307)"
                        ));
                    }
                }
                Expr::Through(scope, reference, name) => {
                    if !matches!(scope, Scope::Record) {
                        out.push(format!(
                            "{at}: computed follows a reference of the old or new record, which a recomputation does not have"
                        ));
                    }
                    if name.starts_with('$') {
                        out.push(format!(
                            "{at}: computed follows {reference} to the system attribute {name} (D-8307)"
                        ));
                    }
                    let target = FieldId::new(reference)
                        .ok()
                        .and_then(|id| object.fields.get(&id))
                        .and_then(|def| def.ty.ref_target().map(|(to, _, _)| to.clone()))
                        .and_then(|to| schema.objects.get(&to));
                    let computed = target
                        .and_then(|to| FieldId::new(name).ok().and_then(|id| to.fields.get(&id)))
                        .is_some_and(|other| other.computed.is_some());
                    if computed {
                        out.push(format!(
                            "{at}: computed follows {reference} to computed field {name}; it reads stored fields only (D-8307)"
                        ));
                    }
                }
                _ => {}
            }
        }
    }
}

/// The problems of a schema whose operations, workflows or automations write a computed field.
pub fn writer_problems(schema: &TenantSchema, out: &mut Vec<String>) {
    let computed = |object: &ObjectTypeId, field: &FieldId| {
        schema
            .objects
            .get(object)
            .and_then(|def| def.fields.get(field))
            .is_some_and(|def| def.computed.is_some())
    };
    for operation in schema.operations.iter().flat_map(BTreeMap::values) {
        for step in &operation.steps {
            let (object, fields) = match step {
                crate::types::TenantOperationStep::CreateRecord { object, fields, .. }
                | crate::types::TenantOperationStep::UpdateRecord { object, fields, .. } => {
                    (object, fields)
                }
                _ => continue,
            };
            for field in fields.keys().filter(|field| computed(object, field)) {
                out.push(format!(
                    "operation {}: writes computed field {object}.{field} (D-8307)",
                    operation.id
                ));
            }
        }
    }
    for workflow in schema.workflows.values() {
        for transition in &workflow.transitions {
            for field in transition
                .set
                .keys()
                .filter(|field| computed(&workflow.object, field))
            {
                out.push(format!(
                    "workflow {}.{}: sets computed field {}.{field} (D-8307)",
                    workflow.id, transition.name, workflow.object
                ));
            }
        }
    }
    for (id, automation) in schema.automations.iter().flat_map(BTreeMap::iter) {
        for step in automation.steps.values() {
            let (object, fields) = match &step.action {
                AutomationStep::CreateRecord { object, fields }
                | AutomationStep::UpdateRecord { object, fields, .. } => (object, fields),
                _ => continue,
            };
            for field in fields.keys().filter(|field| computed(object, field)) {
                out.push(format!(
                    "automation {id}: writes computed field {object}.{field} (D-8307)"
                ));
            }
        }
    }
}

/// The refusal of a create or update intent that names a computed field: the kernel computes it.
pub fn written(object: &ObjectTypeDef, fields: &BTreeMap<FieldId, Value>) -> Option<String> {
    fields
        .keys()
        .find(|field| {
            object
                .fields
                .get(field)
                .is_some_and(|def| def.computed.is_some())
        })
        .map(|field| {
            format!(
                "{}.{field} is computed by the kernel and cannot be written (D-8307)",
                object.id
            )
        })
}

/// The values of the computed fields of `object`, as `record` stands, at `now`. A reference that
/// the snapshot does not cover is `InsufficientState`, naming the record, as for any expression.
pub fn values(
    lookup: Lookup<'_>,
    object: &ObjectTypeDef,
    record: &Record,
    principal: &Principal,
    roles: &BTreeSet<RoleId>,
    now: Timestamp,
) -> Result<BTreeMap<FieldId, Value>, DomainError> {
    let no_params = BTreeMap::new();
    let env = Env {
        record: Some(record),
        old: None,
        new: None,
        params: &no_params,
        now,
        principal,
        roles,
        lookup: Some(lookup),
    };
    let mut out = BTreeMap::new();
    for field in object.fields.values() {
        let Some(expr) = &field.computed else {
            continue;
        };
        let value = expr::eval(expr, &env).map_err(|error| {
            missing_error(expr, &env).unwrap_or_else(|| {
                DomainError::new(
                    ErrorKind::Evaluation,
                    format!("computed {}.{}: {error}", object.id, field.id),
                )
                .with(vec![Check::fail(
                    format!("computed:{}.{}", object.id, field.id),
                    format!("{expr}: {error}"),
                )])
            })
        })?;
        out.insert(field.id.clone(), value);
    }
    Ok(out)
}

/// Store computed values on a record: a null value is an absent field, as everywhere else.
pub fn store(record: &mut Record, values: BTreeMap<FieldId, Value>) {
    for (field, value) in values {
        if value.is_null() {
            record.fields.remove(&field);
        } else {
            record.fields.insert(field, value);
        }
    }
}

/// A reference field of one object type that computed fields of that type follow, with the
/// fields of the referred record they read.
pub struct Dependent {
    pub object: ObjectTypeId,
    pub reference: FieldId,
    pub names: BTreeSet<String>,
}

/// The reference fields, of any object type, that the computed fields of the type follow to
/// `target`, with the fields of `target` they read. A record of `target` changed by a write
/// changes the referring records computed from it, and those are the ones a write must find.
pub fn dependents(schema: &TenantSchema, target: &ObjectTypeId) -> Vec<Dependent> {
    let mut out = Vec::new();
    for object in schema.objects.values() {
        for field in object.fields.values() {
            let Some((to, _, false)) = field.ty.ref_target() else {
                continue;
            };
            if to != target {
                continue;
            }
            let mut names = BTreeSet::new();
            for computed in object.fields.values() {
                if let Some(expr) = &computed.computed {
                    names.extend(through_names(expr, &field.id));
                }
            }
            if !names.is_empty() {
                out.push(Dependent {
                    object: object.id.clone(),
                    reference: field.id.clone(),
                    names,
                });
            }
        }
    }
    out
}

/// Whether a write that turned `before` into `after` changes a field a dependent reads. A system
/// attribute always changes with a write.
fn triggered(dependent: &Dependent, before: &Record, after: &Record) -> bool {
    dependent.names.iter().any(|name| {
        name.starts_with('$')
            || FieldId::new(name).map_or(true, |id| before.get(&id) != after.get(&id))
    })
}

/// The records that refer to `after` through a computed field, recomputed in the commit of the
/// write that changed `after` from `before`. Each is recomputed once, its version advanced, and
/// validated as a write of its own; a record whose computed values do not change is left alone.
/// A write that would reach more than [`limits::MAX_COMPUTED_FANOUT`] records is refused.
pub fn refer(
    schema: &TenantSchema,
    lookup: Lookup<'_>,
    target: &ObjectTypeId,
    before: &Record,
    after: &Record,
    principal: &Principal,
    roles: &BTreeSet<RoleId>,
    now: Timestamp,
) -> Result<Recomputed, DomainError> {
    let mut out = Recomputed::default();
    let deps: Vec<Dependent> = dependents(schema, target)
        .into_iter()
        .filter(|dependent| triggered(dependent, before, after))
        .collect();
    if deps.is_empty() {
        return Ok(out);
    }
    let mut reached: BTreeSet<(ObjectTypeId, RecordId)> = BTreeSet::new();
    for dependent in &deps {
        for record in lookup.by_field(
            &dependent.object,
            &dependent.reference,
            &Value::Ref(after.id),
        )? {
            if &dependent.object == target && record.id == after.id {
                continue;
            }
            reached.insert((dependent.object.clone(), record.id));
        }
    }
    if reached.len() > limits::MAX_COMPUTED_FANOUT {
        return Err(DomainError::new(
            ErrorKind::LimitExceeded,
            format!(
                "the write would recompute {} referring records through computed fields, more than the maximum of {}",
                reached.len(),
                limits::MAX_COMPUTED_FANOUT
            ),
        )
        .with(vec![Check::fail(
            "computed:fanout",
            format!("{} referring records, at most {}", reached.len(), limits::MAX_COMPUTED_FANOUT),
        )]));
    }
    // The state the recomputation reads: the written record as it will stand.
    let mut next = lookup.tenant.clone();
    next.records
        .entry(target.clone())
        .or_default()
        .insert(after.id, Arc::new(after.clone()));
    let later = Lookup {
        tenant: &next,
        coverage: lookup.coverage,
    };
    for (object, id) in reached {
        let Some(def) = schema.objects.get(&object) else {
            continue;
        };
        let Some(old) = lookup.tenant.record(&object, id).cloned() else {
            continue;
        };
        let values = values(later, def, &old, principal, roles, now)?;
        if values.iter().all(|(field, value)| old.get(field) == value) {
            continue;
        }
        let mut candidate = Record::clone(&old);
        store(&mut candidate, values);
        candidate.version = old.version + 1;
        candidate.updated_at = now;
        validate_record(later, schema, def, &candidate, Some(&*old), principal, now)?
            .into_result(&mut out.evidence)?;
        let new = Arc::new(candidate);
        out.ops.push(DeltaOp::PutRecord(new.clone()));
        out.events.push(EventBody::RecordUpdated {
            before: old,
            after: new,
            transitions: None,
        });
    }
    Ok(out)
}

/// One refresh of the time-dependent computed fields of an object type, for one batch of its
/// records from `after` (KRN-2926). Only the system refreshes them. A record is written only when
/// one of its computed values changed, with its version advanced and its change time set to
/// `now`. `more` says that the batch was full, and `next_after` is where the next batch starts.
pub fn decide_refresh(
    lookup: Lookup<'_>,
    schema: &TenantSchema,
    ctx: &Context,
    object: &ObjectTypeId,
    after: Option<RecordId>,
) -> Result<Refresh, DomainError> {
    if ctx.principal != Principal::System {
        return Err(DomainError::new(
            ErrorKind::Unauthorized,
            "only the system refreshes computed fields",
        )
        .with(vec![Check::fail(
            "computed:refresh_by_system",
            format!("{:?} may not refresh {object}", ctx.principal),
        )]));
    }
    let Some(def) = schema.objects.get(object) else {
        return Err(DomainError::new(
            ErrorKind::UnknownObject,
            format!("no object type {object}"),
        ));
    };
    if !def
        .fields
        .values()
        .any(|field| field.computed.as_ref().is_some_and(reads_now))
    {
        return Err(DomainError::new(
            ErrorKind::InvalidIntent,
            format!("{object} has no time-dependent computed field"),
        )
        .with(vec![Check::fail(
            "computed:time_dependent",
            format!("{object} declares no computed field that reads now"),
        )]));
    }
    let page = lookup.page_of(object, after, BATCH as u32)?;
    let more = page.len() == BATCH;
    let next_after = if more {
        page.last().map(|record| record.id)
    } else {
        None
    };
    let roles = crate::rules::roles_of(lookup.tenant, &ctx.principal);
    let mut out = Recomputed::default();
    let mut changed: u64 = 0;
    for old in page {
        let values = values(lookup, def, old, &ctx.principal, &roles, ctx.now)?;
        if values.iter().all(|(field, value)| old.get(field) == value) {
            continue;
        }
        let mut candidate = Record::clone(old);
        store(&mut candidate, values);
        candidate.version = old.version + 1;
        candidate.updated_at = ctx.now;
        validate_record(
            lookup,
            schema,
            def,
            &candidate,
            Some(&**old),
            &ctx.principal,
            ctx.now,
        )?
        .into_result(&mut out.evidence)?;
        changed += 1;
        let new = Arc::new(candidate);
        out.ops.push(DeltaOp::PutRecord(new.clone()));
        out.events.push(EventBody::RecordUpdated {
            before: old.clone(),
            after: new,
            transitions: None,
        });
    }
    Ok(Refresh {
        recomputed: out,
        output: Output::Refreshed {
            changed,
            more,
            next_after,
        },
    })
}
