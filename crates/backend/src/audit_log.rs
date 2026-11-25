//! Verifying the audit chain of a tenant (contract `verify-audit-chain`, CT-128, OBS-007,
//! PRV-359): `POST /internal/audit/verify`.
//!
//! The chain is the one the store keeps beside every audit line ([`strata_storage_api::audit_chain`]):
//! each line carries the hash of the line before it and of itself in the form stored, so a line
//! that was altered or one removed from the middle is found at the next line, and the first break
//! is named by its sequence number. The caller must hold the capability `audit-read` (the owner
//! holds every capability); an anonymous caller is not signed in, and one without the capability
//! is refused. A `tenant` that is not the tenant of the request is refused as `tenant_mismatch`
//! and is an audit entry (TEN-1140).
//!
//! The answer is `{ "intact": bool, "first_break": <sequence number>, ... }`; `first_break` is
//! absent while the chain is intact. `entries`, `verified`, `unchained` and `reason` say how much
//! was read, how many lines chained, how many were written before the chain existed, and what
//! was wrong at the first break.
//!
//! The head of the chain is anchored outside the tenant database at each anchor interval (D-6208,
//! D-8423, OBS-3129): a chain that stops short of its last anchor is a break with the code
//! `AnchorMismatch`, and `first_break` is the first sequence number it has lost. What it does not
//! find: the alteration of a line written before the chain existed, and the removal of lines
//! written after the last anchor.
//!
//! Implements: CT-128

use crate::app::{ApiError, App, Shared};
use crate::auth::Identity;
use crate::http::as_caller;
use crate::ops::header;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde_json::{json, Value as Json_};
use std::collections::BTreeSet;
use strata_kernel::rules::holds_any;
use strata_kernel::Grant;
use strata_protocol::api::ErrorCode;

/// The capability that reads and verifies the audit log.
pub const AUDIT_READ: &str = "audit-read";

fn refuse_other_tenant(app: &App, identity: &Identity, body: &Json_) -> Result<(), ApiError> {
    match body.get("tenant").and_then(Json_::as_str) {
        Some(named) if named != identity.tenant.as_str() => {
            app.audit(
                "audit_log.tenant_mismatch",
                &identity.correlation,
                json!({ "tenant": identity.tenant.as_str() }),
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

/// Check the audit chain of the caller's tenant.
pub fn verify(app: &App, identity: &Identity, body: &Json_) -> Result<Json_, ApiError> {
    if identity.anonymous() {
        return Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in to use this.",
        ));
    }
    refuse_other_tenant(app, identity, body)?;
    let tenant = header(app, identity)?;
    let wanted = BTreeSet::from([Grant::Capability(AUDIT_READ.to_string())]);
    if !holds_any(&tenant, &identity.principal, &wanted) {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            format!("This needs the permission {AUDIT_READ}."),
        ));
    }
    let report = app.verify_audit_chain(&identity.tenant)?;
    let mut answer = json!({
        "intact": report.ok,
        "entries": report.entries,
        "verified": report.verified,
        "unchained": report.unchained,
    });
    if let Some((sequence, reason)) = &report.first_break {
        answer["first_break"] = json!(sequence);
        answer["reason"] = json!(reason);
    }
    if let Some(code) = &report.code {
        answer["code"] = json!(code);
    }
    Ok(answer)
}

/// `POST /internal/audit/verify`.
///
/// Implements: CT-128
pub async fn route(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        verify(app, identity, &body)
    })
    .await
}
