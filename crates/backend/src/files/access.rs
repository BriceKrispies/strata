//! Who may read a file (D-6587, CMS-1591): whoever the kernel lets read a field of a record that
//! refers to it, and, while no record refers to it, its uploaders only.
//!
//! The references of a file are found through the record index: one `Need::ByField` for each
//! field of the `file` type (or a list of them) of each object type, with the file id as the
//! value. Nothing is counted beside the records, so no count can drift from them; the index the
//! store keeps for `ByField` is what makes the lookup cheap.
//!
//! The decision for a referring record is the kernel's read (`strata_kernel::read`): the record's
//! read policies decide whether the reader sees the record, and the field's `read` rule whether
//! the reader sees the field (D-6586). The file is readable when some referring record comes back
//! with the field still holding the file id.

use super::FileMeta;
use crate::app::{ApiError, App};
use std::collections::BTreeSet;
use std::sync::Arc;
use strata_kernel::ext::TenantStateExt;
use strata_kernel::{
    read, read_needs, Context, ErrorKind, FieldId, FieldType, Need, ObjectTypeId, Principal,
    ReadRequest, Record, RecordId, TenantId, TenantSchema, Value,
};
use strata_protocol::api::ErrorCode;
use strata_storage_api::field_matches;

/// One record field that refers to a file.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Reference {
    pub object: ObjectTypeId,
    pub record: RecordId,
    pub field: FieldId,
}

/// Whether a field type holds files: `file`, or a list of them.
pub fn holds_files(ty: &FieldType) -> bool {
    match ty {
        FieldType::File { .. } => true,
        FieldType::List(inner) => matches!(inner.as_ref(), FieldType::File { .. }),
        _ => false,
    }
}

/// Every field of the `file` type in a schema, by object type.
pub fn file_fields(schema: &TenantSchema) -> Vec<(ObjectTypeId, FieldId)> {
    schema
        .objects
        .iter()
        .flat_map(|(object, def)| {
            def.fields
                .values()
                .filter(|field| holds_files(&field.ty))
                .map(move |field| (object.clone(), field.id.clone()))
        })
        .collect()
}

/// The file ids a field value holds.
pub fn files_in(value: &Value) -> Vec<String> {
    match value {
        Value::Str(text) if super::storage::valid_file_id(text) => vec![text.clone()],
        Value::List(items) => items.iter().flat_map(files_in).collect(),
        _ => Vec::new(),
    }
}

/// Every record field of a tenant that refers to `file`.
pub fn references(
    app: &App,
    tenant: &TenantId,
    schema: &TenantSchema,
    file: &str,
) -> Result<Vec<Reference>, ApiError> {
    let fields = file_fields(schema);
    if fields.is_empty() {
        return Ok(Vec::new());
    }
    let value = Value::Str(file.to_string());
    let needs: Vec<Need> = fields
        .iter()
        .map(|(object, field)| Need::ByField {
            object: object.clone(),
            field: field.clone(),
            value: value.clone(),
        })
        .collect();
    let state = app.store.load(tenant, &needs)?;
    let Some(loaded) = state.tenant else {
        return Ok(Vec::new());
    };
    let mut found = BTreeSet::new();
    for (object, field) in &fields {
        for record in loaded.records_of(object) {
            if field_matches(record, field, &value) {
                found.insert(Reference {
                    object: object.clone(),
                    record: record.id,
                    field: field.clone(),
                });
            }
        }
    }
    Ok(found.into_iter().collect())
}

/// One record as the kernel lets the context's principal read it, loading what a policy that
/// follows a reference asks for, a bounded number of times. `None` when the record does not exist
/// or the principal may not read it.
pub fn read_record(
    app: &App,
    ctx: &Context,
    schema: &TenantSchema,
    object: &ObjectTypeId,
    id: RecordId,
) -> Result<Option<Arc<Record>>, ApiError> {
    let request = ReadRequest::Records {
        object: object.clone(),
        ids: vec![id],
    };
    let mut needs: BTreeSet<Need> = read_needs(schema, &request).into_iter().collect();
    for _ in 0..32 {
        let wanted: Vec<Need> = needs.iter().cloned().collect();
        let state = app.store.load(&ctx.tenant, &wanted)?;
        match read(&state, &request, ctx) {
            Ok(result) => return Ok(result.rows.into_iter().next()),
            Err(error) if error.kind == ErrorKind::InsufficientState => match error.missing.clone()
            {
                Some(need) if !needs.contains(&need) => {
                    needs.insert(need);
                }
                _ => return Err(ApiError::domain(error, false)),
            },
            Err(error) if error.kind == ErrorKind::Unauthorized => return Ok(None),
            Err(error) => return Err(ApiError::domain(error, false)),
        }
    }
    Err(ApiError::hidden(
        ErrorCode::Internal,
        "a read of a file's record kept asking for more state",
    ))
}

/// Whether the principal of `ctx` may read the field `reference` names, and the field still
/// holds `file` as the principal sees it.
fn reads_through(
    app: &App,
    ctx: &Context,
    schema: &TenantSchema,
    reference: &Reference,
    file: &str,
) -> Result<bool, ApiError> {
    let shown = read_record(app, ctx, schema, &reference.object, reference.record)?;
    Ok(shown.is_some_and(|record| {
        record.fields.contains_key(&reference.field)
            && field_matches(&record, &reference.field, &Value::Str(file.to_string()))
    }))
}

/// Whether the principal of `ctx` may read a file (D-6587). With `through`, only that record is
/// consulted: it must refer to the file, and the principal must read the referring field of it.
/// Without, every record that refers to the file is; a file no record refers to is readable by
/// its uploaders only.
pub fn may_read(
    app: &App,
    ctx: &Context,
    meta: &FileMeta,
    through: Option<(&ObjectTypeId, RecordId)>,
) -> Result<bool, ApiError> {
    let Some(state) = app.store.load(&ctx.tenant, &[])?.tenant else {
        return Ok(false);
    };
    let schema: &TenantSchema = &state.schema;
    let all = references(app, &ctx.tenant, schema, &meta.file)?;
    if all.is_empty() {
        return Ok(match &ctx.principal {
            Principal::User(principal) => meta.uploaded_by(principal.as_str()),
            Principal::System => true,
            Principal::Anonymous => false,
        });
    }
    for reference in &all {
        if through.is_some_and(|(object, id)| *object != reference.object || id != reference.record)
        {
            continue;
        }
        if reads_through(app, ctx, schema, reference, &meta.file)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// A context of the same request for another principal: what the kernel's read is asked as when
/// the decision is about someone other than the caller.
pub fn as_principal(ctx: &Context, principal: Principal) -> Context {
    Context {
        principal,
        session: None,
        idempotency_key: None,
        verified_address: None,
        ..ctx.clone()
    }
}
