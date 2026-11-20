//! Who may name a file in a record (CMS-1591): the check made before a commit that creates or
//! updates a record holding a file id.
//!
//! A field of the `file` type holds a content hash, and the kernel accepts any string there.
//! Readability of a file is decided through the records that refer to it ([`super::access`]), so
//! without this check anyone could attach another principal's unattached upload to a record of
//! their own and read it through that record, and a file id is the hash of the content, so
//! anyone who knows a file's content knows its id.
//!
//! **The rule.** For every file-typed field (a `file` field or a list of files) whose value is
//! new or changed in a write, each file id in the new value that the old value did not hold must
//! name a file of this tenant that is `clean`, and the acting principal must have uploaded it or
//! already be able to read it ([`super::access::may_read`]). An unchanged value is not checked
//! again, so a record keeps working for someone who could not attach its file afresh. Anything
//! else is refused with `not_found`, the same answer for a hash that is not stored, a file of
//! another tenant, a file that is not the caller's, a rejected file and a text that is no file id,
//! so the refusal tells nothing about whether the content exists.
//!
//! **A file still scanning** is refused, for the uploader too: only a clean file is ever
//! delivered (CMS-260), and nothing lets an uploader attach before the verdict.
//!
//! **Automation and system principals.** The principal in the context is the one checked. An
//! automation runs as its own principal, which must itself be able to read the file (it never
//! uploads, so it attaches only files some record it may read already holds). The system
//! principal may attach any clean file of the tenant.
//!
//! **Where it applies.** [`check_intent`] is called by `ops::run_intent`, which every write of
//! records goes through: the record mutation route, tenant operations, automation steps, product
//! installs, seeding and erasure. An operation's writes are known only once it is decided, so it
//! is decided first and [`check_events`] looks at the records its events carry; the dry run of an
//! operation calls [`check_events`] too, so it does not answer differently from the real call.
//!
//! Implements: CMS-1591

use super::{access, not_found, storage, FileState};
use crate::app::{ApiError, App};
use std::collections::BTreeSet;
use strata_kernel::ext::*;
use strata_kernel::{
    Context, EventBody, FieldId, Intent, Need, ObjectTypeId, Principal, Record, TenantSchema,
    Transition, Value,
};
use strata_storage_api::idempotency_scope;

/// Check the file ids a record write names, before it is decided and committed.
pub fn check_intent(app: &App, ctx: &Context, intent: &Intent) -> Result<(), ApiError> {
    match intent {
        Intent::CreateRecord { object, fields } => {
            let Some(schema) = file_schema(app, ctx)? else {
                return Ok(());
            };
            let named = named_by(&schema, object, fields.iter());
            check_files(app, ctx, &named)
        }
        Intent::UpdateRecord {
            object, id, fields, ..
        } => {
            let Some(schema) = file_schema(app, ctx)? else {
                return Ok(());
            };
            let named = named_by(&schema, object, fields.iter());
            if named.is_empty() {
                return Ok(());
            }
            let old = app
                .store
                .load(
                    &ctx.tenant,
                    &[Need::Record {
                        object: object.clone(),
                        id: *id,
                    }],
                )?
                .tenant
                .and_then(|tenant| {
                    tenant
                        .record(object, *id)
                        .map(|record| held_by(&schema, object, record))
                })
                .unwrap_or_default();
            let fresh: BTreeSet<String> = named.difference(&old).cloned().collect();
            check_files(app, ctx, &fresh)
        }
        Intent::InvokeOperation { .. } => {
            if file_schema(app, ctx)?.is_none() {
                return Ok(());
            }
            // A repeat of a committed invocation answers with the first result and writes nothing.
            if let Some(key) = &ctx.idempotency_key {
                let scope = idempotency_scope(&ctx.principal);
                if app
                    .store
                    .idempotent_result(&ctx.tenant, &scope, key, ctx.now)?
                    .is_some()
                {
                    return Ok(());
                }
            }
            let transition = crate::operations::decide(app, ctx, intent)?;
            check_events(app, ctx, &transition)
        }
        _ => Ok(()),
    }
}

/// Check the file ids the records of a decided transition newly hold.
pub fn check_events(app: &App, ctx: &Context, transition: &Transition) -> Result<(), ApiError> {
    let Some(schema) = file_schema(app, ctx)? else {
        return Ok(());
    };
    let mut fresh = BTreeSet::new();
    for event in &transition.events {
        match &event.body {
            EventBody::RecordCreated { record } => {
                fresh.extend(held_by(&schema, &record.object, record));
            }
            EventBody::RecordUpdated { before, after, .. } => {
                let old = held_by(&schema, &before.object, before);
                for file in held_by(&schema, &after.object, after) {
                    if !old.contains(&file) {
                        fresh.insert(file);
                    }
                }
            }
            _ => {}
        }
    }
    check_files(app, ctx, &fresh)
}

/// The schema of the tenant when it has any field that holds files, else `None`: a tenant with
/// none has nothing to check.
fn file_schema(app: &App, ctx: &Context) -> Result<Option<TenantSchema>, ApiError> {
    let Some(state) = app.store.load(&ctx.tenant, &[])?.tenant else {
        return Ok(None);
    };
    if access::file_fields(&state.schema).is_empty() {
        return Ok(None);
    }
    Ok(Some(TenantSchema::clone(&state.schema)))
}

/// Every text a value of a file field holds.
fn texts_in(value: &Value, into: &mut BTreeSet<String>) {
    match value {
        Value::Str(text) => {
            into.insert(text.clone());
        }
        Value::List(items) => items.iter().for_each(|item| texts_in(item, into)),
        _ => {}
    }
}

/// The texts the given field values hold in the file-typed fields of `object`.
fn named_by<'a>(
    schema: &TenantSchema,
    object: &ObjectTypeId,
    fields: impl Iterator<Item = (&'a FieldId, &'a Value)>,
) -> BTreeSet<String> {
    let mut named = BTreeSet::new();
    let Some(def) = schema.objects.get(object) else {
        return named;
    };
    for (id, value) in fields {
        if def
            .fields
            .get(id)
            .is_some_and(|field| access::holds_files(&field.ty))
        {
            texts_in(value, &mut named);
        }
    }
    named
}

/// The texts a stored record holds in the file-typed fields of its object type.
fn held_by(schema: &TenantSchema, object: &ObjectTypeId, record: &Record) -> BTreeSet<String> {
    named_by(schema, object, record.fields.iter())
}

fn check_files(app: &App, ctx: &Context, files: &BTreeSet<String>) -> Result<(), ApiError> {
    files.iter().try_for_each(|file| check_file(app, ctx, file))
}

/// The rule for one file id; every refusal is the same `not_found`.
fn check_file(app: &App, ctx: &Context, file: &str) -> Result<(), ApiError> {
    if !storage::valid_file_id(file) {
        return Err(not_found());
    }
    let Some(meta) = storage::get_meta(app, &ctx.tenant, file)? else {
        return Err(not_found());
    };
    if meta.state != FileState::Clean {
        return Err(not_found());
    }
    let uploaded = matches!(&ctx.principal, Principal::User(who) if meta.uploaded_by(who.as_str()));
    if uploaded || access::may_read(app, ctx, &meta, None)? {
        Ok(())
    } else {
        Err(not_found())
    }
}
