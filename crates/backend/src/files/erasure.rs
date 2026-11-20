//! What the erasure of a data subject removes of files (WG-5118, PRV-1586, D-6585, PRV-353).
//!
//! Called by `crate::erasure::erase_subject` before the kernel clears the subject's fields, while
//! the fields still say which files they refer to:
//!
//! - a file referred to only by personal `file` fields of the subject's records (fields whose
//!   personal declaration names the subject, as the kernel's erasure decides it) loses its bytes
//!   and its metadata: once the fields are cleared nothing refers to it;
//! - the subject is removed from the upload notes of every file (`rec-file-bytes.uploaded-by`),
//!   and a file that then has no uploader and no reference is removed too.
//!
//! A file that another record, a field that is not personal, or another person's field refers to
//! is kept. What was removed is kept under the erasure's correlation id (kind
//! [`super::ERASURE_KIND`]) and audited by count.

use super::access::{files_in, holds_files, references, Reference};
use super::{json_error, storage, FileMeta, ERASURE_KIND, FILE_KIND};
use crate::app::{ApiError, App};
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::Arc;
use strata_kernel::ext::{RecordExt, TenantStateExt};
use strata_kernel::{
    Context, DataSubject, FieldDef, Need, ObjectTypeId, PrincipalId, Record, TenantSchema, Value,
};

/// How many records one page of a scan of an object type reads.
const PAGE: u32 = 500;

/// Whether a value names the subject, as the kernel's erasure reads a subject link.
fn names(value: &Value, subject: &PrincipalId) -> bool {
    match value {
        Value::Str(text) => text == subject.as_str(),
        Value::List(items) => items.iter().any(|item| names(item, subject)),
        _ => false,
    }
}

/// Whether `field` of `record` is personal data of the subject.
fn is_subject_of(field: &FieldDef, record: &Record, subject: &PrincipalId) -> bool {
    match field.personal.as_ref().map(|p| &p.subject) {
        Some(DataSubject::Creator) => record.created_by.as_ref() == Some(subject),
        Some(DataSubject::Field { field: link }) => names(record.get(link), subject),
        None => false,
    }
}

/// The records of an object type that may hold personal data of the subject: those whose subject
/// link names them (through the index), and, when a field's subject is the creator, every
/// record, read a page at a time.
fn candidate_records(
    app: &App,
    ctx: &Context,
    object: &ObjectTypeId,
    fields: &[&FieldDef],
    subject: &PrincipalId,
) -> Result<Vec<Arc<Record>>, ApiError> {
    let mut found: Vec<Arc<Record>> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut keep = |record: &Arc<Record>, found: &mut Vec<Arc<Record>>| {
        if seen.insert(record.id) {
            found.push(record.clone());
        }
    };
    let links: BTreeSet<_> = fields
        .iter()
        .filter_map(|field| match field.personal.as_ref().map(|p| &p.subject) {
            Some(DataSubject::Field { field: link }) => Some(link.clone()),
            _ => None,
        })
        .collect();
    for link in links {
        let need = Need::ByField {
            object: object.clone(),
            field: link,
            value: Value::Str(subject.as_str().to_string()),
        };
        if let Some(state) = app.store.load(&ctx.tenant, &[need])?.tenant {
            for record in state.records_of(object) {
                keep(record, &mut found);
            }
        }
    }
    let by_creator = fields.iter().any(|field| {
        matches!(
            field.personal.as_ref().map(|p| &p.subject),
            Some(DataSubject::Creator)
        )
    });
    if by_creator {
        let mut after = None;
        loop {
            let need = Need::PageOf {
                object: object.clone(),
                after,
                limit: PAGE,
            };
            let Some(state) = app.store.load(&ctx.tenant, &[need])?.tenant else {
                break;
            };
            let mut last = None;
            let mut count = 0;
            for record in state.records_of(object) {
                count += 1;
                last = Some(record.id);
                if record.created_by.as_ref() == Some(subject) {
                    keep(record, &mut found);
                }
            }
            if count < PAGE as usize || last.is_none() {
                break;
            }
            after = last;
        }
    }
    Ok(found)
}

/// The personal file fields of a schema, by object type.
fn personal_file_fields(schema: &TenantSchema) -> Vec<(ObjectTypeId, Vec<&FieldDef>)> {
    schema
        .objects
        .iter()
        .filter_map(|(object, def)| {
            let fields: Vec<&FieldDef> = def
                .fields
                .values()
                .filter(|field| holds_files(&field.ty) && field.personal.is_some())
                .collect();
            (!fields.is_empty()).then(|| (object.clone(), fields))
        })
        .collect()
}

/// Remove the files of a data subject (see the module). Returns the ids of the files removed.
///
/// Implements: PRV-1586
pub fn erase_subject_files(
    app: &App,
    ctx: &Context,
    subject: &PrincipalId,
) -> Result<Vec<String>, ApiError> {
    let Some(state) = app.store.load(&ctx.tenant, &[])?.tenant else {
        return Ok(Vec::new());
    };
    let schema: &TenantSchema = &state.schema;
    // The subject's personal file fields and the files they hold.
    let mut slots: BTreeSet<Reference> = BTreeSet::new();
    let mut candidates: BTreeSet<String> = BTreeSet::new();
    for (object, fields) in personal_file_fields(schema) {
        for record in candidate_records(app, ctx, &object, &fields, subject)? {
            for field in &fields {
                if !is_subject_of(field, &record, subject) {
                    continue;
                }
                let held = files_in(record.get(&field.id));
                if held.is_empty() {
                    continue;
                }
                slots.insert(Reference {
                    object: object.clone(),
                    record: record.id,
                    field: field.id.clone(),
                });
                candidates.extend(held);
            }
        }
    }
    let mut removed: BTreeSet<String> = BTreeSet::new();
    for file in &candidates {
        let all = references(app, &ctx.tenant, schema, file)?;
        if all.iter().all(|reference| slots.contains(reference)) {
            storage::delete_file(app, &ctx.tenant, file)?;
            removed.insert(file.clone());
        }
    }
    // The subject as an uploader.
    for (key, text) in app.store.doc_list(&ctx.tenant, FILE_KIND)? {
        let Ok(mut meta) = serde_json::from_str::<FileMeta>(&text) else {
            continue;
        };
        if meta.file != key || removed.contains(&key) || !meta.uploaded_by(subject.as_str()) {
            continue;
        }
        meta.uploads
            .retain(|note| note.principal != subject.as_str());
        let unreferenced = references(app, &ctx.tenant, schema, &key)?
            .iter()
            .all(|reference| slots.contains(reference));
        if meta.uploads.is_empty() && unreferenced {
            storage::delete_file(app, &ctx.tenant, &key)?;
            removed.insert(key);
        } else {
            storage::put_meta(app, &ctx.tenant, &meta)?;
        }
    }
    let removed: Vec<String> = removed.into_iter().collect();
    let record = json!({
        "principal": subject.as_str(),
        "at": ctx.now.0,
        "files": removed,
    });
    app.store.doc_put(
        &ctx.tenant,
        ERASURE_KIND,
        &ctx.correlation,
        &serde_json::to_string(&record).map_err(json_error)?,
        false,
    )?;
    app.audit(
        "files.erased",
        &ctx.correlation,
        json!({ "tenant": ctx.tenant.as_str(), "files": removed.len() }),
    );
    Ok(removed)
}
