//! Uploads (contracts upload-file, start-upload, put-upload-chunk and finish-upload, D-6589,
//! CMS-1593).
//!
//! A file of at most [`SINGLE_MAX_BYTES`] goes in one call; a larger one is started with its
//! size, sent in chunks of [`CHUNK_BYTES`] (the last one shorter) and finished. The size is
//! checked against the smallest of the platform maximum, the tenant's limit and the field's cap
//! before any byte is kept: a single upload's route refuses a longer body before reading it, and
//! a chunked upload is refused at its start. A chunk has exactly the length its index implies, so
//! the bytes received never exceed the size given at the start.
//!
//! Finishing (or the single call) hashes the content as it is written to a temporary file,
//! decides its media type from the content ([`sniff`]) and refuses it when that is not the
//! declared type, not a type the field accepts, or an image that decodes past the limit; then
//! renames it to its hash, keeps its metadata and creates its scan job. Until a record refers to
//! the file only its uploader may read it (CMS-1591).

use super::sniff;
use super::{
    invalid, io_error, json_error, not_found, scan, storage, tenant_limits, FieldRef, FileMeta,
    FileState, UploadNote, CHUNK_BYTES, SINGLE_MAX_BYTES, UNATTACHED_MS, UPLOAD_KIND,
};
use crate::app::{ApiError, App, LOG};
use crate::auth::random_hex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use strata_kernel::rules::{authorize, held_capabilities, roles_of, Subject};
use strata_kernel::{
    owner_role, Action, Context, FieldId, FieldType, ObjectTypeId, Principal, PrincipalId,
    Resource, TenantId, TenantState, Timestamp,
};
use strata_protocol::api::ErrorCode;

/// The longest file name kept, in bytes.
pub const MAX_NAME_BYTES: usize = 255;

/// What an upload names besides its content.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct UploadRequest {
    /// The media type the client declares; it must be what the content is.
    pub declared_type: String,
    /// The file name the client gives: data, never a path.
    #[serde(default)]
    pub name: String,
    /// The object type and field the file is meant for, when the client says.
    #[serde(default)]
    pub object: Option<String>,
    #[serde(default)]
    pub field: Option<String>,
    /// A tenant the request names: it must be the one the request is made at (TEN-1140).
    #[serde(default)]
    pub tenant: Option<String>,
}

/// The field a file is uploaded for, as its schema declares it.
#[derive(Clone, Debug)]
struct Target {
    object: ObjectTypeId,
    field: FieldId,
    media_types: Vec<String>,
    max_bytes: u64,
}

impl Target {
    fn reference(&self) -> FieldRef {
        FieldRef {
            object: self.object.as_str().to_string(),
            field: self.field.as_str().to_string(),
        }
    }
}

/// What an upload came to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Uploaded {
    pub file: String,
    pub state: &'static str,
    pub media_type: String,
    pub size: u64,
}

/// An unfinished chunked upload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadSession {
    pub upload: String,
    pub principal: String,
    pub declared_type: String,
    pub name: String,
    pub size: u64,
    pub chunk_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<FieldRef>,
    pub created_at: i64,
}

impl UploadSession {
    /// How many chunks the upload has.
    pub fn chunks(&self) -> u64 {
        self.size.div_ceil(self.chunk_bytes).max(1)
    }

    /// The length chunk `index` must have.
    pub fn chunk_len(&self, index: u64) -> u64 {
        let start = index * self.chunk_bytes;
        self.size.saturating_sub(start).min(self.chunk_bytes)
    }
}

fn tenant_mismatch(app: &App, ctx: &Context, named: Option<&str>) -> Result<(), ApiError> {
    match named {
        Some(named) if named != ctx.tenant.as_str() => {
            app.audit(
                "files.tenant_mismatch",
                &ctx.correlation,
                json!({ "tenant": ctx.tenant.as_str() }),
            );
            let mut error = ApiError::new(
                ErrorCode::Forbidden,
                "The request names another tenant than the one it is made at.",
            );
            error.0.kind = Some("tenant_mismatch".to_string());
            Err(error)
        }
        _ => Ok(()),
    }
}

/// The signed-in principal of a request, or the refusal.
fn signed_in(ctx: &Context) -> Result<PrincipalId, ApiError> {
    match &ctx.principal {
        Principal::User(principal) => Ok(principal.clone()),
        _ => Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in to do this.",
        )),
    }
}

/// The field a request names, as the schema declares it. Both or neither must be named, and the
/// field must be of the `file` type.
fn target_of(tenant: &TenantState, request: &UploadRequest) -> Result<Option<Target>, ApiError> {
    let (object, field) = match (&request.object, &request.field) {
        (None, None) => return Ok(None),
        (Some(object), Some(field)) => (object, field),
        _ => {
            return Err(invalid(
                "Name both the object type and the field, or neither.",
            ))
        }
    };
    let unknown = || invalid("No such file field.");
    let object = ObjectTypeId::new(object.as_str()).map_err(|_| unknown())?;
    let field = FieldId::new(field.as_str()).map_err(|_| unknown())?;
    let def = tenant
        .schema
        .objects
        .get(&object)
        .and_then(|def| def.fields.get(&field))
        .ok_or_else(unknown)?;
    let ty = match &def.ty {
        FieldType::List(inner) => inner.as_ref(),
        other => other,
    };
    let FieldType::File {
        media_types,
        max_bytes,
        ..
    } = ty
    else {
        return Err(unknown());
    };
    Ok(Some(Target {
        object,
        field,
        media_types: media_types.clone(),
        max_bytes: *max_bytes,
    }))
}

/// Whether the principal may upload (D-6591): an owner, or a holder of `content-write`, or, for a
/// named field, a holder of `self-file-upload`. With a field named, the principal's policies
/// must also let it create or change records of that type.
fn may_upload(tenant: &TenantState, ctx: &Context, target: Option<&Target>) -> bool {
    let owner = roles_of(tenant, &ctx.principal).contains(&owner_role());
    let held = held_capabilities(tenant, &ctx.principal);
    let authoring = owner || held.contains("content-write");
    match target {
        None => authoring,
        Some(target) => {
            let writes = [Action::Create, Action::Update].into_iter().any(|action| {
                authorize(
                    tenant,
                    &tenant.schema,
                    &ctx.principal,
                    ctx.now,
                    action,
                    &Resource::Object(target.object.clone()),
                    Subject::default(),
                    None,
                )
                .0
            });
            (authoring || held.contains("self-file-upload")) && writes
        }
    }
}

/// Everything an upload is checked against before a byte is kept: the session, the tenant, the
/// permission, the field, the name and the size against every cap.
fn admit(
    app: &App,
    ctx: &Context,
    request: &UploadRequest,
    size: u64,
) -> Result<(PrincipalId, Option<Target>), ApiError> {
    let principal = signed_in(ctx)?;
    tenant_mismatch(app, ctx, request.tenant.as_deref())?;
    let tenant = app
        .store
        .load(&ctx.tenant, &[])?
        .tenant
        .ok_or_else(not_found)?;
    let target = target_of(&tenant, request)?;
    if !may_upload(&tenant, ctx, target.as_ref()) {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "You may not upload files here.",
        ));
    }
    if request.name.len() > MAX_NAME_BYTES {
        return Err(invalid(format!(
            "A file name is at most {MAX_NAME_BYTES} bytes."
        )));
    }
    if sniff::normalise(&request.declared_type).is_empty() {
        return Err(invalid("Say what type of file this is."));
    }
    let cap = limit(app, &ctx.tenant, target.as_ref());
    if size > cap {
        return Err(too_large(cap));
    }
    if size == 0 {
        return Err(invalid("The file is empty."));
    }
    if let Some(target) = &target {
        if !sniff::field_accepts(
            &target.media_types,
            &sniff::normalise(&request.declared_type),
        ) {
            return Err(invalid("The field does not accept files of this type."));
        }
    }
    Ok((principal, target))
}

/// The largest file an upload for `target` may be: the smallest of the platform maximum, the
/// tenant's limit and the field's cap (CMS-1593).
pub fn limit_for(app: &App, tenant: &TenantId, field_cap: Option<u64>) -> u64 {
    let tenant_cap = tenant_limits(app, tenant).max_bytes;
    match field_cap {
        Some(cap) if cap > 0 => cap.min(tenant_cap),
        _ => tenant_cap,
    }
}

fn limit(app: &App, tenant: &TenantId, target: Option<&Target>) -> u64 {
    limit_for(app, tenant, target.map(|t| t.max_bytes))
}

pub(crate) fn too_large(cap: u64) -> ApiError {
    let mut error = invalid(format!("The file is larger than the limit of {cap} bytes."));
    error.0.kind = Some("too_large".to_string());
    error
}

/// Check the written content, then keep it: rename it to its hash, keep or extend its metadata
/// and create its scan job. A refused file leaves nothing behind.
fn intake(
    app: &App,
    ctx: &Context,
    principal: &PrincipalId,
    request: &UploadRequest,
    target: Option<&Target>,
    mut pending: storage::Pending,
) -> Result<Uploaded, ApiError> {
    let file = pending.seal()?;
    let size = pending.written();
    let head = storage::read_head(pending.path(), sniff::HEAD_BYTES)?;
    let limits = tenant_limits(app, &ctx.tenant);
    let kind = sniff::check(
        &head,
        &request.declared_type,
        target.map(|t| t.media_types.as_slice()),
        limits.max_decompressed_bytes,
    )
    .map_err(|why| {
        LOG.info(
            "files.refused",
            &ctx.correlation,
            json!({ "tenant": ctx.tenant.as_str(), "why": why }),
        );
        invalid(why)
    })?;
    let note = UploadNote {
        principal: principal.as_str().to_string(),
        name: request.name.clone(),
        declared_type: sniff::normalise(&request.declared_type),
        at: ctx.now.0,
    };
    // Content seen before keeps its state: a rejected file is refused again, a clean or scanning
    // one gains an upload note.
    if let Some(mut meta) = storage::get_meta(app, &ctx.tenant, &file)? {
        if meta.state == FileState::Rejected {
            return Err(invalid("The file was rejected by the upload scan."));
        }
        pending.rename_to(&storage::bytes_path(app, &ctx.tenant, &file)?)?;
        if !meta.uploaded_by(principal.as_str()) {
            meta.uploads.push(note);
            storage::put_meta(app, &ctx.tenant, &meta)?;
        }
        return Ok(Uploaded {
            file,
            state: meta.state.as_str(),
            media_type: meta.media_type,
            size: meta.size,
        });
    }
    pending.rename_to(&storage::bytes_path(app, &ctx.tenant, &file)?)?;
    let meta = FileMeta {
        file: file.clone(),
        media_type: kind.media_type.to_string(),
        size,
        state: FileState::Scanning,
        uploads: vec![note],
        created_at: ctx.now.0,
        referenced_at: None,
        scanned_at: None,
        verdict: None,
        field: target.map(Target::reference),
    };
    if !storage::create_meta(app, &ctx.tenant, &meta)? {
        // The same content was finished at the same moment by another request: add this upload
        // to what that one kept.
        if let Some(mut kept) = storage::get_meta(app, &ctx.tenant, &file)? {
            if !kept.uploaded_by(principal.as_str()) {
                kept.uploads.extend(meta.uploads.clone());
                storage::put_meta(app, &ctx.tenant, &kept)?;
            }
        }
    }
    app.store
        .jobs_create(&ctx.tenant, &[scan::scan_job(&file, ctx.now)], ctx.now)?;
    LOG.info(
        "files.stored",
        &ctx.correlation,
        json!({
            "tenant": ctx.tenant.as_str(),
            "file": file,
            "media_type": meta.media_type,
            "size": size,
        }),
    );
    Ok(Uploaded {
        file,
        state: FileState::Scanning.as_str(),
        media_type: meta.media_type,
        size,
    })
}

/// Upload a file of at most [`SINGLE_MAX_BYTES`] in one call (contract upload-file).
///
/// Implements: CT-203, CMS-257, CMS-261, CMS-263
pub fn upload_file(
    app: &App,
    ctx: &Context,
    request: &UploadRequest,
    content: &[u8],
) -> Result<Uploaded, ApiError> {
    let size = content.len() as u64;
    if size > SINGLE_MAX_BYTES {
        return Err(too_large(SINGLE_MAX_BYTES));
    }
    let (principal, target) = admit(app, ctx, request, size)?;
    let mut pending = storage::Pending::of_tenant(app, &ctx.tenant)?;
    pending.write(content)?;
    intake(app, ctx, &principal, request, target.as_ref(), pending)
}

/// What starting a chunked upload answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Started {
    pub upload: String,
    pub chunk_bytes: u64,
    pub chunks: u64,
    pub state: &'static str,
}

/// Start a chunked upload of `size` bytes (contract start-upload). The size is checked against
/// every cap here, before any byte is accepted.
///
/// Implements: CT-838
pub fn start_upload(
    app: &App,
    ctx: &Context,
    request: &UploadRequest,
    size: u64,
) -> Result<Started, ApiError> {
    let (principal, target) = admit(app, ctx, request, size)?;
    let session = UploadSession {
        upload: random_hex(16)?,
        principal: principal.as_str().to_string(),
        declared_type: sniff::normalise(&request.declared_type),
        name: request.name.clone(),
        size,
        chunk_bytes: CHUNK_BYTES,
        field: target.as_ref().map(Target::reference),
        created_at: ctx.now.0,
    };
    let text = serde_json::to_string(&session).map_err(json_error)?;
    app.store
        .doc_put(&ctx.tenant, UPLOAD_KIND, &session.upload, &text, true)?;
    Ok(Started {
        chunks: session.chunks(),
        upload: session.upload,
        chunk_bytes: CHUNK_BYTES,
        state: "uploading",
    })
}

/// The unfinished upload `upload` of the caller, when it exists and has not expired. Another
/// principal's upload is not found.
fn session_of(app: &App, ctx: &Context, upload: &str) -> Result<UploadSession, ApiError> {
    let principal = signed_in(ctx)?;
    if !storage::valid_upload_id(upload) {
        return Err(not_found());
    }
    let text = app
        .store
        .doc_get(&ctx.tenant, UPLOAD_KIND, upload)?
        .ok_or_else(not_found)?;
    let session: UploadSession = serde_json::from_str(&text).map_err(json_error)?;
    if session.principal != principal.as_str()
        || session.upload != upload
        || ctx.now.0 - session.created_at >= UNATTACHED_MS
    {
        return Err(not_found());
    }
    Ok(session)
}

/// Send chunk `index` of an upload (contract put-upload-chunk). A chunk has the length its index
/// implies; the same index sent again with the same content changes nothing, and with other
/// content is refused. Returns the bytes received so far.
///
/// Implements: CT-839
pub fn put_upload_chunk(
    app: &App,
    ctx: &Context,
    upload: &str,
    index: u32,
    content: &[u8],
) -> Result<u64, ApiError> {
    let session = session_of(app, ctx, upload)?;
    let index64 = u64::from(index);
    if index64 >= session.chunks() {
        return Err(invalid(format!(
            "The upload has {} chunks, numbered from 0.",
            session.chunks()
        )));
    }
    let expected = session.chunk_len(index64);
    let given = content.len() as u64;
    if given > session.chunk_bytes || given > expected {
        return Err(too_large(expected));
    }
    if given != expected {
        return Err(invalid(format!(
            "Chunk {index} is {expected} bytes, not {given}."
        )));
    }
    let path = storage::chunk_path(app, &ctx.tenant, upload, index)?;
    if path.exists() {
        let kept = fs::read(&path).map_err(|e| io_error("read a chunk", e))?;
        if kept != content {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                format!("Chunk {index} was received with other content."),
            ));
        }
    } else {
        let dir = storage::upload_dir(app, &ctx.tenant, upload)?;
        let mut pending = storage::Pending::in_dir(&dir)?;
        pending.write(content)?;
        pending.seal()?;
        pending.rename_to(&path)?;
    }
    received(app, &ctx.tenant, &session)
}

/// The bytes of an upload received so far: the lengths of the chunks kept.
fn received(app: &App, tenant: &TenantId, session: &UploadSession) -> Result<u64, ApiError> {
    let mut total = 0;
    for index in 0..session.chunks() {
        let index = u32::try_from(index).map_err(|_| invalid("Too many chunks."))?;
        let path = storage::chunk_path(app, tenant, &session.upload, index)?;
        if let Ok(meta) = fs::metadata(&path) {
            total += meta.len();
        }
    }
    Ok(total)
}

/// Finish a chunked upload (contract finish-upload): every chunk must be there with its length.
/// The chunks are joined into one file, hashed and checked as a single upload is, then removed.
///
/// Implements: CT-840
pub fn finish_upload(app: &App, ctx: &Context, upload: &str) -> Result<Uploaded, ApiError> {
    let session = session_of(app, ctx, upload)?;
    let principal = signed_in(ctx)?;
    let request = UploadRequest {
        declared_type: session.declared_type.clone(),
        name: session.name.clone(),
        object: session.field.as_ref().map(|f| f.object.clone()),
        field: session.field.as_ref().map(|f| f.field.clone()),
        tenant: None,
    };
    // The field may have changed since the start: it is read again.
    let tenant = app
        .store
        .load(&ctx.tenant, &[])?
        .tenant
        .ok_or_else(not_found)?;
    let target = target_of(&tenant, &request)?;
    let cap = limit(app, &ctx.tenant, target.as_ref());
    if session.size > cap {
        return Err(too_large(cap));
    }
    let mut pending = storage::Pending::of_tenant(app, &ctx.tenant)?;
    for index in 0..session.chunks() {
        let index32 = u32::try_from(index).map_err(|_| invalid("Too many chunks."))?;
        let path = storage::chunk_path(app, &ctx.tenant, upload, index32)?;
        let chunk = fs::read(&path)
            .map_err(|_| invalid(format!("Chunk {index} has not been received.")))?;
        if chunk.len() as u64 != session.chunk_len(index) {
            return Err(invalid(format!("Chunk {index} has the wrong length.")));
        }
        pending.write(&chunk)?;
    }
    if pending.written() != session.size {
        return Err(invalid("The chunks do not add up to the size given."));
    }
    let uploaded = intake(app, ctx, &principal, &request, target.as_ref(), pending);
    // Finished or refused, the upload is over: its chunks go.
    remove_upload(app, &ctx.tenant, upload)?;
    uploaded
}

/// Remove an unfinished upload with its chunks.
fn remove_upload(app: &App, tenant: &TenantId, upload: &str) -> Result<(), ApiError> {
    let dir = storage::upload_dir(app, tenant, upload)?;
    match fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error("remove an upload", error)),
    }
    app.store.doc_remove(tenant, UPLOAD_KIND, upload)?;
    Ok(())
}

/// Remove the unfinished uploads of a tenant that are 24 hours old. Returns how many.
pub fn purge_uploads(app: &App, tenant: &TenantId, now: Timestamp) -> Result<u64, ApiError> {
    let mut removed = 0;
    for (key, text) in app.store.doc_list(tenant, UPLOAD_KIND)? {
        let expired = match serde_json::from_str::<UploadSession>(&text) {
            Ok(session) => now.0 - session.created_at >= UNATTACHED_MS,
            // A document that cannot be read is not an upload anyone can finish.
            Err(_) => true,
        };
        if expired && storage::valid_upload_id(&key) {
            remove_upload(app, tenant, &key)?;
            removed += 1;
        }
    }
    Ok(removed)
}
