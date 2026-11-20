//! The internal routes of the file store, which the gateway calls (`crates/bff/src/files.rs`).
//!
//! Content travels as the raw request body, never as JSON: a single upload's body is refused
//! before it is read when it is longer than [`super::SINGLE_MAX_BYTES`], a chunk's when it is
//! longer than [`super::CHUNK_BYTES`]. Delivery answers the bytes with their headers.

use super::address::{sign_media_address, SignRequest};
use super::deliver::{deliver_file, DeliveryRequest};
use super::upload::{
    finish_upload, put_upload_chunk, start_upload, upload_file, Started, UploadRequest, Uploaded,
};
use super::{CHUNK_BYTES, SINGLE_MAX_BYTES};
use crate::app::{ApiError, Shared};
use crate::auth;
use crate::http::as_caller;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value as Json_;
use std::collections::BTreeMap;

/// The routes, to merge into the backend's router.
pub fn router() -> Router<Shared> {
    Router::new()
        .route(
            "/internal/files/upload",
            post(upload).layer(DefaultBodyLimit::max(SINGLE_MAX_BYTES as usize + 1)),
        )
        .route("/internal/files/uploads/start", post(start))
        .route(
            "/internal/files/uploads/chunk",
            post(chunk).layer(DefaultBodyLimit::max(CHUNK_BYTES as usize + 1)),
        )
        .route("/internal/files/uploads/finish", post(finish))
        .route("/internal/files/sign", post(sign))
        .route("/internal/files/deliver", post(deliver))
}

/// What an upload names, from the query string the gateway built.
fn upload_request(query: &BTreeMap<String, String>) -> UploadRequest {
    UploadRequest {
        declared_type: query.get("type").cloned().unwrap_or_default(),
        name: query.get("name").cloned().unwrap_or_default(),
        object: query.get("object").cloned(),
        field: query.get("field").cloned(),
        tenant: query.get("tenant").cloned(),
    }
}

/// `POST /internal/files/upload?type=&name=[&object=&field=]` with the content as the body.
///
/// Implements: CT-203
async fn upload(
    State(app): State<Shared>,
    headers: HeaderMap,
    Query(query): Query<BTreeMap<String, String>>,
    body: Bytes,
) -> Result<Json<Uploaded>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        upload_file(app, &identity.context(app), &upload_request(&query), &body)
    })
    .await
}

#[derive(Deserialize)]
struct StartBody {
    #[serde(flatten)]
    request: UploadRequest,
    size: u64,
}

/// `POST /internal/files/uploads/start` with `{declared_type, name, size, object?, field?}`.
///
/// Implements: CT-838
async fn start(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<StartBody>,
) -> Result<Json<Started>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        start_upload(app, &identity.context(app), &body.request, body.size)
    })
    .await
}

/// `POST /internal/files/uploads/chunk?upload=&index=` with the chunk as the body.
///
/// Implements: CT-839
async fn chunk(
    State(app): State<Shared>,
    headers: HeaderMap,
    Query(query): Query<BTreeMap<String, String>>,
    body: Bytes,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        let upload = query.get("upload").cloned().unwrap_or_default();
        let index = query
            .get("index")
            .and_then(|text| text.parse::<u32>().ok())
            .ok_or_else(|| super::invalid("A chunk index is a whole number from 0."))?;
        let received = put_upload_chunk(app, &identity.context(app), &upload, index, &body)?;
        Ok(serde_json::json!({ "received": received }))
    })
    .await
}

#[derive(Deserialize)]
struct FinishBody {
    upload: String,
}

/// `POST /internal/files/uploads/finish` with `{upload}`.
///
/// Implements: CT-840
async fn finish(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<FinishBody>,
) -> Result<Json<Uploaded>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        finish_upload(app, &identity.context(app), &body.upload)
    })
    .await
}

/// `POST /internal/files/sign` with `{file, principal?, expires_at?, object?, record?}`.
///
/// Implements: CT-106
async fn sign(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<SignRequest>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        sign_media_address(app, &identity.context(app), &body)
    })
    .await
}

/// `POST /internal/files/deliver` with `{file, expires, scope, signature, range?}`: the bytes
/// of the file with their headers, or a JSON refusal.
///
/// Implements: CT-205
async fn deliver(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<DeliveryRequest>,
) -> Response {
    let caller = match auth::caller(&headers) {
        Ok(caller) => caller,
        Err(error) => return error.into_response(),
    };
    let correlation = caller.correlation.clone();
    let tenant = caller.site.as_str().to_string();
    let delivered = app
        .blocking_for(&tenant, move |app| {
            let identity = auth::identify(app, &caller)?;
            deliver_file(app, &identity.context(app), &body)
        })
        .await
        .and_then(|result| result);
    let delivery = match delivered {
        Ok(delivery) => delivery,
        Err(error) => return error.with_correlation(&correlation).into_response(),
    };
    let mut response = Response::new(Body::from(delivery.body));
    *response.status_mut() = StatusCode::from_u16(delivery.status).unwrap_or(StatusCode::OK);
    for (name, value) in delivery.headers {
        if let Ok(value) = HeaderValue::from_str(&value) {
            response
                .headers_mut()
                .insert(HeaderName::from_static(name), value);
        }
    }
    response
}
