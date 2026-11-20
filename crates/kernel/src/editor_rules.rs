//! What the kernel will check on a record, told to the editor that writes it, so the page can
//! refuse a draft before the round trip (WG-5143).
//!
//! The answer is for one reader. It carries, for the fields the reader may read: the field's
//! shape, required flag and size limits; the type's constraints as printed rule text; and, for a
//! workflow field, the transitions out of the record's current state with their guards as text.
//! It carries nothing about a field the reader may not read. The decision which fields those are
//! is [`crate::read::redact`], the one a read makes: the stored record is trimmed by it with every
//! field that has a `read` rule present, and a field that did not survive is not described, not
//! named by a constraint or a guard, and has no `read` rule sent. A constraint or guard that
//! reads such a field, or that follows a reference, is not sent: the kernel still enforces it and
//! the page leaves that verdict to the server.
//!
//! A new record has no stored state to decide a `read` rule on, so a field with one is not sent
//! for it. The server's answer to a write always stands; this is advice to the page, never a
//! decision.

use crate::expr::Expr;
use crate::limits;
use crate::read::redact;
use crate::rules::{roles_of, Lookup};
use crate::schema::{ObjectTypeDef, TenantSchemaExt};
use crate::state::{Principal, Record, RecordExt};
use crate::types::{FieldId, FieldType, FieldTypeExt, Timestamp, Value, ValueExt};
use serde_json::{json, Value as Json};
use std::collections::BTreeSet;
use std::sync::Arc;

/// The shape of a field as the rule language's checker names it.
fn shape(ty: &FieldType) -> &'static str {
    match ty {
        FieldType::Str | FieldType::File { .. } | FieldType::Language => "str",
        FieldType::Int => "int",
        FieldType::Bool => "bool",
        FieldType::Time => "time",
        FieldType::Ref { .. } => "ref",
        FieldType::List(_) => "list",
    }
}

/// The fields an expression reads on its record scopes, in `out`. False when it follows a
/// reference (`through`), which the page cannot evaluate.
fn reads(expr: &Expr, out: &mut BTreeSet<String>) -> bool {
    match expr {
        Expr::Lit(_) | Expr::Param(_) | Expr::Now | Expr::Principal | Expr::HasRole(_) => true,
        Expr::Var(_, name) => {
            out.insert(name.clone());
            true
        }
        Expr::Through(..) => false,
        Expr::Not(a)
        | Expr::IsNull(a)
        | Expr::Len(a)
        | Expr::Lower(a)
        | Expr::IsSlug(a)
        | Expr::Duration(_, a)
        | Expr::Text(a) => reads(a, out),
        Expr::And(items) | Expr::Or(items) => items.iter().all(|item| reads(item, out)),
        Expr::Cmp(_, a, b)
        | Expr::In(a, b)
        | Expr::Contains(a, b)
        | Expr::StartsWith(a, b)
        | Expr::Arith(_, a, b)
        | Expr::MsBetween(a, b)
        | Expr::Concat(a, b) => reads(a, out) && reads(b, out),
        Expr::If(a, b, c) => reads(a, out) && reads(b, out) && reads(c, out),
    }
}

/// Whether an expression may be sent: it follows no reference and reads only fields the reader
/// may read (a system attribute, written `$name`, is not a field).
fn sendable(expr: &Expr, readable: &BTreeSet<FieldId>) -> bool {
    let mut names = BTreeSet::new();
    reads(expr, &mut names)
        && names.iter().all(|name| {
            name.starts_with('$')
                || FieldId::new(name.clone()).is_ok_and(|id| readable.contains(&id))
        })
}

/// The fields of `object` the reader may read, decided by the kernel's redaction. `None` when
/// the decision needs a record the snapshot does not hold.
pub fn readable_fields(
    lookup: Lookup<'_>,
    principal: &Principal,
    now: Timestamp,
    object: &ObjectTypeDef,
    stored: Option<&Arc<Record>>,
) -> Option<BTreeSet<FieldId>> {
    let ruled = |id: &FieldId| object.fields.get(id).is_some_and(|f| f.read.is_some());
    let Some(stored) = stored else {
        return Some(
            object
                .fields
                .keys()
                .filter(|id| !ruled(id))
                .cloned()
                .collect(),
        );
    };
    let mut probe = Record::clone(stored);
    for field in object.fields.values().filter(|f| f.read.is_some()) {
        probe.fields.entry(field.id.clone()).or_insert(Value::Null);
    }
    let roles = roles_of(lookup.tenant, principal);
    let shown = redact(lookup, principal, &roles, now, &Arc::new(probe)).ok()?;
    Some(
        object
            .fields
            .keys()
            .filter(|id| !ruled(id) || shown.fields.contains_key(*id))
            .cloned()
            .collect(),
    )
}

/// The editor's metadata of one object type for one reader. `stored` is the record being edited,
/// as the snapshot holds it (not as it was redacted), or `None` for a new record. `None` when
/// the kernel cannot decide what the reader may read from this snapshot.
pub fn record_rules(
    lookup: Lookup<'_>,
    principal: &Principal,
    now: Timestamp,
    object: &ObjectTypeDef,
    stored: Option<&Arc<Record>>,
) -> Option<Json> {
    let readable = readable_fields(lookup, principal, now, object, stored)?;
    let fields: Vec<Json> = object
        .fields
        .values()
        .filter(|field| readable.contains(&field.id))
        .map(|field| {
            json!({
                "id": field.id.as_str(),
                "label": field.label,
                "type": shape(&field.ty),
                "type_name": field.ty.name(),
                "required": field.required,
                "unique": field.unique,
            })
        })
        .collect();
    let constraints: Vec<Json> = object
        .constraints
        .values()
        .filter(|constraint| sendable(&constraint.expr, &readable))
        .map(|constraint| {
            json!({
                "id": constraint.id.as_str(),
                "rule": constraint.expr.to_string(),
                "message": constraint.message,
            })
        })
        .collect();
    let workflows: Vec<Json> = lookup
        .tenant
        .schema
        .workflows_of(&object.id)
        .filter(|workflow| readable.contains(&workflow.field))
        .map(|workflow| {
            let state = stored
                .map(|record| record.get(&workflow.field))
                .and_then(|value| value.as_str());
            let transitions: Vec<Json> = workflow
                .transitions
                .iter()
                .filter(|step| Some(step.from.as_str()) == state)
                .map(|step| {
                    let open = step
                        .guard
                        .as_ref()
                        .is_none_or(|guard| sendable(guard, &readable));
                    json!({
                        "name": step.name,
                        "from": step.from,
                        "to": step.to,
                        "guard": step.guard.as_ref().filter(|_| open).map(|g| g.to_string()),
                        "server_decides": !open,
                        "sets": step.set.keys().filter(|f| readable.contains(*f)).map(|f| f.as_str()).collect::<Vec<_>>(),
                    })
                })
                .collect();
            json!({
                "id": workflow.id.as_str(),
                "field": workflow.field.as_str(),
                "initial": workflow.initial,
                "state": state,
                "transitions": transitions,
            })
        })
        .collect();
    Some(json!({
        "object": object.id.as_str(),
        "fields": fields,
        "limits": {
            "string_bytes": limits::MAX_STRING_BYTES,
            "list_items": limits::MAX_LIST_LEN,
            "unique_bytes": limits::MAX_UNIQUE_BYTES,
            "record_bytes": limits::MAX_RECORD_BYTES,
        },
        "constraints": constraints,
        "workflows": workflows,
    }))
}
