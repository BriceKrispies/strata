//! The declarations of an object type beyond its fields and constraints, and the removal of an
//! object type or a role (what the removal of a product needs, D-6581): retention (D-6584), publication (D-6592), the language field and the translation
//! group (D-6593). A field's `personal` declaration travels in the `FieldDef` of `DefineField`;
//! what this module adds is the schema changes that set and clear the declarations that live on
//! the object type itself, and the definition-time checks that tie them to the fields they name.
//!
//! Each declaration is an optional member of [`ObjectTypeDef`] that is left out of the encoding
//! while it is absent, so a schema that never declares one hashes as it did before, and clearing a
//! declaration returns the schema to the hash it had without it.
//!
//! Removing an object type ([`SchemaChange::RemoveObject`]) is the one change here that touches
//! records: the change is refused while records of the type exist, unless it says they are
//! deleted, and then the transition that applies it deletes them in the same commit
//! ([`deleted_records`]).
//!
//! The hooks in the shared files are one arm in `transition::apply_change` ([`apply`]), one call
//! in `TenantSchemaExt::problems` ([`problems`]), one call in `transition::needs`
//! ([`removal_needs`]) and one in `transition::change_schema` ([`deleted_records`]).

use crate::rules::Lookup;
use crate::schema::{TenantSchema, TenantSchemaExt, WorkflowDefExt};
use crate::types::{
    owner_role, public_role, DataSubject, DomainError, FieldType, Need, ObjectTypeDef,
    ObjectTypeId, PublicationDef, Record, RetentionStart, SchemaChange, TenantState,
};
use std::collections::BTreeSet;
use std::sync::Arc;

/// The object type `id` of a schema, to change it.
fn object<'a>(
    schema: &'a mut TenantSchema,
    id: &ObjectTypeId,
) -> Result<&'a mut ObjectTypeDef, String> {
    schema
        .objects
        .get_mut(id)
        .ok_or_else(|| format!("no object type {id}"))
}

/// Whether `change` is one this module applies.
pub fn is_object_meta(change: &SchemaChange) -> bool {
    matches!(
        change,
        SchemaChange::SetRetention { .. }
            | SchemaChange::ClearRetention { .. }
            | SchemaChange::SetPublication { .. }
            | SchemaChange::ClearPublication { .. }
            | SchemaChange::SetLanguageField { .. }
            | SchemaChange::ClearLanguageField { .. }
            | SchemaChange::SetTranslationGroup { .. }
            | SchemaChange::ClearTranslationGroup { .. }
            | SchemaChange::RemoveObject { .. }
            | SchemaChange::RemoveRole(_)
    )
}

/// Apply one change of this module to a working copy and describe it. Setting a declaration
/// replaces the one there; clearing one that is not declared, and changing an object type that
/// does not exist, is refused.
pub fn apply(schema: &mut TenantSchema, change: &SchemaChange) -> Result<String, String> {
    match change {
        SchemaChange::SetRetention {
            object: id,
            retention,
        } => {
            object(schema, id)?.retention = Some(retention.clone());
            Ok(format!("set retention of {id}"))
        }
        SchemaChange::ClearRetention { object: id } => {
            object(schema, id)?
                .retention
                .take()
                .ok_or_else(|| format!("{id} declares no retention"))?;
            Ok(format!("clear retention of {id}"))
        }
        SchemaChange::SetPublication {
            object: id,
            publication,
        } => {
            object(schema, id)?.publication = Some(publication.clone());
            Ok(format!("set publication of {id}"))
        }
        SchemaChange::ClearPublication { object: id } => {
            object(schema, id)?
                .publication
                .take()
                .ok_or_else(|| format!("{id} declares no publication"))?;
            Ok(format!("clear publication of {id}"))
        }
        SchemaChange::SetLanguageField { object: id, field } => {
            object(schema, id)?.language_field = Some(field.clone());
            Ok(format!("set language field of {id} to {field}"))
        }
        SchemaChange::ClearLanguageField { object: id } => {
            object(schema, id)?
                .language_field
                .take()
                .ok_or_else(|| format!("{id} declares no language field"))?;
            Ok(format!("clear language field of {id}"))
        }
        SchemaChange::SetTranslationGroup { object: id, field } => {
            object(schema, id)?.translation_group = Some(field.clone());
            Ok(format!("set translation group of {id} to {field}"))
        }
        SchemaChange::ClearTranslationGroup { object: id } => {
            object(schema, id)?
                .translation_group
                .take()
                .ok_or_else(|| format!("{id} declares no translation group"))?;
            Ok(format!("clear translation group of {id}"))
        }
        SchemaChange::RemoveObject { id, .. } => {
            schema
                .objects
                .remove(id)
                .ok_or_else(|| format!("no object type {id}"))?;
            Ok(format!("remove object {id}"))
        }
        SchemaChange::RemoveRole(id) => {
            if *id == public_role() || *id == owner_role() {
                return Err(format!("{id} is built in"));
            }
            schema
                .roles
                .remove(id)
                .ok_or_else(|| format!("no role {id}"))?;
            // A role that is gone grants nothing: its entry in the table of permissions goes too.
            schema.set_role_permissions(id, Vec::new());
            Ok(format!("remove role {id}"))
        }
        other => Err(format!(
            "{other:?} is not a change of an object type or a role"
        )),
    }
}

/// The problems of the declarations of the object types of a schema, pushed to `problems`.
///
/// - Retention: at least one day; counted from a field, that field exists and is a timestamp.
///   (That a type with a personal field declares retention is checked with the fields, D-6584.)
/// - A personal field whose subject is another field names a field of the type.
/// - Publication (D-6592): the state field is a field of the type and the type has a workflow on
///   it, and every state the declaration names (draft, published, withdrawn, and, for a type
///   marked for review, in-review and approved) is a state of that workflow; in-review and
///   approved are given together; the time field is a timestamp field; the review workflow is a
///   workflow or an automation of the schema; the review-run field is a field of the type.
/// - Language (D-6593): the language field is a field of the type with the language type; a
///   translation group is a field of the type and needs a language field.
pub fn problems(schema: &TenantSchema, problems: &mut Vec<String>) {
    for object in schema.objects.values() {
        let at = |what: &str| format!("{}: {what}", object.id);
        if let Some(retention) = &object.retention {
            if retention.days == 0 {
                problems.push(at("retention is at least one day"));
            }
            if let RetentionStart::Field { field } = &retention.from {
                match object.fields.get(field) {
                    None => {
                        problems.push(at(&format!("retention counts from unknown field {field}")))
                    }
                    Some(def) if def.ty != FieldType::Time => problems.push(at(&format!(
                        "retention counts from {field}, which is not a timestamp"
                    ))),
                    Some(_) => {}
                }
            }
        }
        for field in object.fields.values() {
            if let Some(DataSubject::Field { field: subject }) =
                field.personal.as_ref().map(|p| &p.subject)
            {
                if !object.fields.contains_key(subject) {
                    problems.push(format!(
                        "{}.{}: the subject of its personal data is unknown field {subject}",
                        object.id, field.id
                    ));
                }
            }
        }
        if let Some(publication) = &object.publication {
            publication_problems(schema, object, publication, problems);
        }
        if let Some(language) = &object.language_field {
            match object.fields.get(language) {
                None => problems.push(at(&format!("language field {language} is not a field"))),
                Some(def) if def.ty != FieldType::Language => problems.push(at(&format!(
                    "language field {language} does not have the language type"
                ))),
                Some(_) => {}
            }
        }
        if let Some(group) = &object.translation_group {
            if !object.fields.contains_key(group) {
                problems.push(at(&format!("translation group {group} is not a field")));
            }
            if object.language_field.is_none() {
                problems.push(at("a translation group needs a language field"));
            }
        }
    }
}

fn publication_problems(
    schema: &TenantSchema,
    object: &ObjectTypeDef,
    publication: &PublicationDef,
    problems: &mut Vec<String>,
) {
    let at = format!("{}: publication", object.id);
    if !object.fields.contains_key(&publication.state_field) {
        problems.push(format!(
            "{at}: state field {} is not a field",
            publication.state_field
        ));
    }
    let workflow = schema
        .workflows
        .values()
        .find(|w| w.object == object.id && w.field == publication.state_field);
    match workflow {
        None => problems.push(format!(
            "{at}: no workflow of {} runs on the state field {}",
            object.id, publication.state_field
        )),
        Some(workflow) => {
            let states = workflow.states();
            let named = [
                Some(publication.draft.as_str()),
                Some(publication.published.as_str()),
                Some(publication.withdrawn.as_str()),
                publication.in_review.as_deref(),
                publication.approved.as_deref(),
            ];
            for state in named.into_iter().flatten() {
                if !states.contains(state) {
                    problems.push(format!(
                        "{at}: {state} is not a state of the workflow {}",
                        workflow.id
                    ));
                }
            }
        }
    }
    if publication.in_review.is_some() != publication.approved.is_some() {
        problems.push(format!(
            "{at}: in-review and approved are declared together, or not at all"
        ));
    }
    if let Some(time) = &publication.time_field {
        match object.fields.get(time) {
            None => problems.push(format!("{at}: time field {time} is not a field")),
            Some(def) if def.ty != FieldType::Time => {
                problems.push(format!("{at}: time field {time} is not a timestamp"))
            }
            Some(_) => {}
        }
    }
    if let Some(review) = &publication.review_workflow {
        let known = schema.workflows.contains_key(review)
            || schema
                .automations
                .as_ref()
                .is_some_and(|automations| automations.contains_key(review));
        if !known {
            problems.push(format!("{at}: unknown review workflow {review}"));
        }
    }
    if let Some(run) = &publication.review_run_field {
        if !object.fields.contains_key(run) {
            problems.push(format!("{at}: review run field {run} is not a field"));
        }
    }
}

/// The object types a list of changes removes, each with whether the change says its records are
/// deleted, in the order of the changes.
pub fn removals(changes: &[SchemaChange]) -> Vec<(&ObjectTypeId, bool)> {
    changes
        .iter()
        .filter_map(|change| match change {
            SchemaChange::RemoveObject { id, delete_records } => Some((id, *delete_records)),
            _ => None,
        })
        .collect()
}

/// The state the decision of `changes` needs beyond the records of the types they touch: every
/// record of each object type they remove.
pub fn removal_needs(changes: &[SchemaChange], out: &mut BTreeSet<Need>) {
    for (object, _) in removals(changes) {
        out.insert(Need::AllOf(object.clone()));
    }
}

/// The reason a list of changes removes a role that principals still hold, or `None`. A role is
/// not taken from the people who hold it: the assignments are revoked first (the schema cannot see
/// them, so this is decided with the members of the tenant).
pub fn held_roles(tenant: &TenantState, changes: &[SchemaChange]) -> Option<String> {
    changes.iter().find_map(|change| {
        let SchemaChange::RemoveRole(role) = change else {
            return None;
        };
        let holders = tenant
            .members
            .values()
            .filter(|roles| roles.contains(role))
            .count();
        (holders > 0)
            .then(|| format!("role {role} is held by {holders} principal(s); revoke it first"))
    })
}

/// The records a list of changes deletes with the object types it removes. A type that has
/// records and is removed without `delete_records` is refused with the count, which is the inner
/// error; the outer error is the snapshot not covering the type.
pub fn deleted_records(
    lookup: Lookup<'_>,
    changes: &[SchemaChange],
) -> Result<Result<Vec<Arc<Record>>, String>, DomainError> {
    let mut deleted = Vec::new();
    for (object, delete_records) in removals(changes) {
        let records = lookup.all_of(object)?;
        if records.is_empty() {
            continue;
        }
        if !delete_records {
            return Ok(Err(format!(
                "{object} has {} record(s); removing it deletes them, which the change does not say",
                records.len()
            )));
        }
        deleted.extend(records.into_iter().cloned());
    }
    Ok(Ok(deleted))
}
