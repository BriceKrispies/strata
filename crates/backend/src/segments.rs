//! The segment routes: define, remove, test and preview a segment, list and count its members,
//! and list one's own visible segments (SEG-001 to SEG-038, SEG-1540 to SEG-1542). Each loads
//! through the storage contract, lets the kernel decide and reports; no rule is written here.
//!
//! The membership itself is kept by every commit ([`strata_storage_api::execute_by`] decides it
//! with the change). The batches a definition starts are rows the job worker runs
//! ([`strata_storage_api::segments::run_batch`], CT-297).

use crate::app::{ApiError, App, Shared};
use crate::auth::Identity;
use crate::http::as_caller;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};
use strata_kernel::segments::{
    Definition, SegmentCount, SegmentId, SegmentMember, SegmentPreview, SegmentSummary,
    SegmentTestResult,
};
use strata_kernel::{expr, Expr, FieldId, ObjectTypeId, PrincipalId, RecordId};
use strata_protocol::api::ErrorCode;
use strata_storage_api::segments as kept;
use strata_storage_api::ExecuteError;

/// The field that holds the principal when a test or a preview names none.
const DEFAULT_PRINCIPAL_FIELD: &str = "principal";

#[derive(Clone, Debug, Deserialize)]
pub struct DefineRequest {
    #[serde(default)]
    pub tenant: Option<String>,
    pub segment: SegmentId,
    pub name: String,
    pub subject_type: String,
    pub predicate: String,
    #[serde(default)]
    pub refresh_seconds: Option<u32>,
    pub principal_field: String,
    #[serde(default)]
    pub visible_to_members: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DefineReply {
    pub version: u64,
    pub batches: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SegmentRequest {
    #[serde(default)]
    pub tenant: Option<String>,
    pub segment: SegmentId,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MembersReply {
    pub members: Vec<SegmentMember>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CountReply {
    pub count: SegmentCount,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TenantRequest {
    #[serde(default)]
    pub tenant: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MineReply {
    pub segments: Vec<SegmentSummary>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DraftRequest {
    #[serde(default)]
    pub tenant: Option<String>,
    pub subject_type: String,
    pub predicate: String,
    #[serde(default)]
    pub principal_field: Option<String>,
    /// For a test: the person.
    #[serde(default)]
    pub person: Option<String>,
    /// For a preview: the cursor and the number of persons.
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TestReply {
    pub result: SegmentTestResult,
}

#[derive(Clone, Debug, Serialize)]
pub struct PreviewReply {
    pub preview: SegmentPreview,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

fn invalid(what: &str, problem: impl std::fmt::Display) -> ApiError {
    ApiError::new(ErrorCode::Invalid, format!("{what}: {problem}"))
}

/// A tenant input that names another tenant than the one of the host and session is refused
/// (TEN-1140).
fn same_tenant(identity: &Identity, named: Option<&str>) -> Result<(), ApiError> {
    match named {
        Some(name) if name != identity.tenant.as_str() => Err(ApiError::new(
            ErrorCode::Forbidden,
            "tenant-mismatch: the request names another tenant than the site it was sent to",
        )),
        _ => Ok(()),
    }
}

fn object(name: &str) -> Result<ObjectTypeId, ApiError> {
    ObjectTypeId::new(name).map_err(|e| invalid("subject_type", e))
}

fn field(name: &str) -> Result<FieldId, ApiError> {
    FieldId::new(name).map_err(|e| invalid("principal_field", e))
}

/// The predicate parsed from its text; a parse error carries its position (SEG-003).
fn predicate(text: &str) -> Result<Expr, ApiError> {
    expr::parse(text).map_err(|e| invalid("predicate", e))
}

fn failed(identity: &Identity) -> impl Fn(ExecuteError) -> ApiError + '_ {
    move |error| ApiError::execute(error, identity.anonymous())
}

fn draft_parts(request: &DraftRequest) -> Result<(ObjectTypeId, FieldId, Expr), ApiError> {
    Ok((
        object(&request.subject_type)?,
        field(
            request
                .principal_field
                .as_deref()
                .unwrap_or(DEFAULT_PRINCIPAL_FIELD),
        )?,
        predicate(&request.predicate)?,
    ))
}

fn define_now(
    app: &App,
    identity: &Identity,
    request: &DefineRequest,
) -> Result<DefineReply, ApiError> {
    same_tenant(identity, request.tenant.as_deref())?;
    crate::quota::check_new_segment(app, &identity.tenant, request.segment)?;
    let def = Definition {
        id: request.segment,
        name: request.name.clone(),
        subject: object(&request.subject_type)?,
        predicate: predicate(&request.predicate)?,
        refresh_seconds: request.refresh_seconds,
        principal_field: field(&request.principal_field)?,
        visible_to_members: request.visible_to_members.unwrap_or(false),
    };
    let ctx = identity.context(app);
    let done = kept::define(app.store.as_ref(), &ctx, &def).map_err(failed(identity))?;
    Ok(DefineReply {
        version: done.segment.version,
        batches: done.batches,
    })
}

/// Create or change a segment.
///
/// Implements: CT-290
pub async fn define(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<DefineRequest>,
) -> Result<Json<DefineReply>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        define_now(app, identity, &request)
    })
    .await
}

/// Remove a segment, its memberships and its batches, with no exit event.
///
/// Implements: CT-291
pub async fn remove(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<SegmentRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        same_tenant(identity, request.tenant.as_deref())?;
        let ctx = identity.context(app);
        kept::remove(app.store.as_ref(), &ctx, request.segment).map_err(failed(identity))?;
        Ok(serde_json::json!({}))
    })
    .await
}

/// List the members of a segment the caller may see, by cursor.
///
/// Implements: CT-292
pub async fn members(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<SegmentRequest>,
) -> Result<Json<MembersReply>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        same_tenant(identity, request.tenant.as_deref())?;
        let after = match request.cursor.as_deref() {
            Some(text) => Some(PrincipalId::new(text).map_err(|e| invalid("cursor", e))?),
            None => None,
        };
        let ctx = identity.context(app);
        let (members, next) = kept::members(
            app.store.as_ref(),
            &ctx,
            request.segment,
            after.as_ref(),
            request.limit,
        )
        .map_err(failed(identity))?;
        Ok(MembersReply {
            members,
            next: next.map(|p| p.as_str().to_string()),
        })
    })
    .await
}

/// Test a draft predicate against one person, changing nothing.
///
/// Implements: CT-293
pub async fn test(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<DraftRequest>,
) -> Result<Json<TestReply>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        same_tenant(identity, request.tenant.as_deref())?;
        let (subject, principal_field, predicate) = draft_parts(&request)?;
        let person = request
            .person
            .as_deref()
            .ok_or_else(|| invalid("person", "a test names the person"))?;
        let person = PrincipalId::new(person).map_err(|e| invalid("person", e))?;
        let ctx = identity.context(app);
        let result = kept::test(
            app.store.as_ref(),
            &ctx,
            &subject,
            &principal_field,
            &predicate,
            &person,
        )
        .map_err(failed(identity))?;
        Ok(TestReply { result })
    })
    .await
}

/// List the segments marked visible to members that the caller belongs to.
///
/// Implements: CT-813
pub async fn mine(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<TenantRequest>,
) -> Result<Json<MineReply>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        same_tenant(identity, request.tenant.as_deref())?;
        let ctx = identity.context(app);
        let segments = kept::mine(app.store.as_ref(), &ctx).map_err(failed(identity))?;
        Ok(MineReply { segments })
    })
    .await
}

/// Count the members of a segment the caller may see.
///
/// Implements: CT-814
pub async fn count(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<SegmentRequest>,
) -> Result<Json<CountReply>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        same_tenant(identity, request.tenant.as_deref())?;
        let ctx = identity.context(app);
        let count =
            kept::count(app.store.as_ref(), &ctx, request.segment).map_err(failed(identity))?;
        Ok(CountReply { count })
    })
    .await
}

/// Preview a draft predicate over at most one thousand persons, changing nothing.
///
/// Implements: CT-815
pub async fn preview(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<DraftRequest>,
) -> Result<Json<PreviewReply>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        same_tenant(identity, request.tenant.as_deref())?;
        let (subject, principal_field, predicate) = draft_parts(&request)?;
        let after = match request.cursor.as_deref() {
            Some(text) => Some(RecordId(
                text.parse::<u64>().map_err(|e| invalid("cursor", e))?,
            )),
            None => None,
        };
        let ctx = identity.context(app);
        let (preview, next) = kept::preview(
            app.store.as_ref(),
            &ctx,
            &subject,
            &principal_field,
            &predicate,
            after,
            request.limit,
        )
        .map_err(failed(identity))?;
        Ok(PreviewReply {
            preview,
            next: next.map(|id| id.0.to_string()),
        })
    })
    .await
}
