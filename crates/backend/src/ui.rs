//! The interface definition of the site's tenant, for the BFF (D-8250, INT-3253, INT-3254).
//!
//! The definition is computed from the tenant schema as it is now (`strata_substrate::ui`): the
//! manifest at `POST /internal/ui`, and one part by its hash at `POST /internal/ui/part`
//! (`{ "hash": "<hex>" }`). The tenant is the site the BFF resolved from the host name; the session,
//! if any, plays no part, since every visitor of a site routes with the same definition. A hash that
//! the current definition does not hold is not found, whichever tenant's definition once held it.

use crate::app::{ApiError, Shared};
use crate::auth;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde_json::{json, Value as Json_};
use strata_protocol::api::ErrorCode;
use strata_substrate::ui::{definition, Definition};

/// The definition of the tenant of the site the request names.
async fn of_site(app: &Shared, headers: &HeaderMap) -> Result<Definition, ApiError> {
    let caller = auth::caller(headers)?;
    let site = caller.site.clone();
    let correlation = caller.correlation.clone();
    let admitted = site.as_str().to_string();
    app.blocking_for(&admitted, move |app| {
        let tenant = app
            .store
            .load(&site, &[])?
            .tenant
            .ok_or_else(|| ApiError::new(ErrorCode::NotFound, format!("no site {site}")))?;
        Ok(definition(&tenant.schema))
    })
    .await?
    .map_err(|error: ApiError| error.with_correlation(&correlation))
}

/// read-tenant-ui-manifest: the manifest of the definition of the site's tenant.
///
/// Implements: CT-1900
pub async fn manifest(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Json_>, ApiError> {
    let definition = of_site(&app, &headers).await?;
    Ok(Json(json!(definition.manifest)))
}

/// read-tenant-ui-part: one part of the definition of the site's tenant, by its hash; not found
/// when the current definition holds no part with that hash (INT-3254).
///
/// Implements: CT-1901
pub async fn part(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    let hash = body
        .get("hash")
        .and_then(Json_::as_str)
        .filter(|hash| hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
        .map(str::to_string)
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::Invalid,
                "A part is named by a hash of 64 hex digits.",
            )
        })?;
    let definition = of_site(&app, &headers).await?;
    definition.part(&hash).cloned().map(Json).ok_or_else(|| {
        ApiError::new(
            ErrorCode::NotFound,
            "The interface definition of this site holds no part with that hash.",
        )
    })
}
