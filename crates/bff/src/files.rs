//! The browser's routes of the file store: uploads, signing an address and delivery.
//!
//! Every upload route is a state-changing request: it must come from one of the site's pages with
//! the session's request token (`routes::guard_write`). The content is the raw request body and
//! goes to the backend as it came; a body longer than the route allows is refused before it is
//! read (`DefaultBodyLimit`, set where the routes are mounted). Delivery is a plain `GET` of the
//! signed address, so an `<img>` can use it; the backend decides everything and sets every
//! header, which this process passes on.
//!
//! The browser's uploader is not built (WG-5226); these routes are what it will call.

use crate::routes::{guard_write, visit, Admission, Failure, Visit};
use crate::{Shared, LOG};
use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::header::RANGE;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use axum::Json;
use serde_json::{json, Value as Json_};
use std::collections::BTreeMap;
use strata_protocol::api::{
    ErrorCode, ErrorReply, HEADER_CORRELATION, HEADER_IDEMPOTENCY, HEADER_ORIGIN, HEADER_SERVICE,
    HEADER_SESSION, HEADER_SITE, HEADER_SOURCE,
};

/// The largest body of a single upload: the backend's limit (1048576 bytes, D-6589) and one byte,
/// so that the backend says why a longer one is refused.
pub const UPLOAD_LIMIT: usize = 1_048_576 + 1;

/// The largest body of one chunk (8388608 bytes, D-6589) and one byte.
pub const CHUNK_LIMIT: usize = 8_388_608 + 1;

/// The headers of a delivery that are passed on to the browser.
const DELIVERY_HEADERS: [&str; 11] = [
    "content-type",
    "content-length",
    "content-range",
    "content-disposition",
    "content-security-policy",
    "x-content-type-options",
    "cache-control",
    "accept-ranges",
    "etag",
    "cross-origin-resource-policy",
    "referrer-policy",
];

/// A request to the backend as this visit, with the visit's headers.
fn backend_request(bff: &crate::Bff, visit: &Visit, path: &str) -> reqwest::RequestBuilder {
    let mut request = bff
        .http
        .post(format!("{}{path}", bff.cfg.backend))
        .header(HEADER_SERVICE, &bff.cfg.service_token)
        .header(HEADER_SITE, visit.site.as_str())
        .header(HEADER_CORRELATION, &visit.correlation);
    if let Some(token) = &visit.token {
        request = request.header(HEADER_SESSION, token);
    }
    if let Some(source) = &visit.source {
        request = request.header(HEADER_SOURCE, source);
    }
    if let Some(origin) = &visit.origin {
        request = request.header(HEADER_ORIGIN, origin);
    }
    if let Some(key) = &visit.idempotency_key {
        request = request.header(HEADER_IDEMPOTENCY, key);
    }
    request
}

fn unavailable(visit: &Visit, path: &str, why: String) -> Failure {
    LOG.error(
        "backend.unreachable",
        &visit.correlation,
        json!({ "path": path, "error": why }),
    );
    Failure::new(
        ErrorCode::Unavailable,
        "The service is not available right now.",
        &visit.correlation,
    )
}

/// Send a request to the backend and read its JSON answer or its refusal.
async fn answer(
    visit: &Visit,
    path: &str,
    request: reqwest::RequestBuilder,
) -> Result<Json<Json_>, Failure> {
    let response = request
        .send()
        .await
        .map_err(|e| unavailable(visit, path, e.to_string()))?;
    if response.status().is_success() {
        response
            .json::<Json_>()
            .await
            .map(Json)
            .map_err(|e| unavailable(visit, path, e.to_string()))
    } else {
        let mut reply = response
            .json::<ErrorReply>()
            .await
            .map_err(|e| unavailable(visit, path, e.to_string()))?;
        reply.correlation = visit.correlation.clone();
        Err(Failure(reply))
    }
}

/// The query of an upload as the backend reads it: only the names it knows, each as given.
fn upload_query(query: &BTreeMap<String, String>) -> Vec<(String, String)> {
    ["type", "name", "object", "field", "tenant"]
        .iter()
        .filter_map(|name| {
            query
                .get(*name)
                .filter(|value| value.len() <= 512)
                .map(|value| (name.to_string(), value.clone()))
        })
        .collect()
}

/// Upload a file of at most 1048576 bytes in one call: `POST /api/files/upload?type=&name=`
/// with the content as the body, and `object` and `field` when the file is meant for a field
/// (contract upload-file).
///
/// Implements: CT-203
pub async fn upload(
    State(bff): State<Shared>,
    headers: HeaderMap,
    Query(query): Query<BTreeMap<String, String>>,
    body: Bytes,
) -> Result<Json<Json_>, Failure> {
    let visit = visit(&bff, &headers)?;
    guard_write(&bff, &visit, &headers).await?;
    // Either capability suffices (D-6591); the caller is refused before anything about the
    // upload is looked at.
    Admission {
        capabilities: &["content-write", "self-file-upload"],
        any: true,
        roles: &[],
        step_up: false,
    }
    .admit(&bff, &visit)
    .await?;
    let path = "/internal/files/upload";
    let request = backend_request(&bff, &visit, path)
        .query(&upload_query(&query))
        .header("content-type", "application/octet-stream")
        .body(body);
    answer(&visit, path, request).await
}

/// Start a chunked upload: `POST /api/files/uploads/start` with `{declared_type, name, size,
/// object?, field?}` (contract start-upload).
///
/// Implements: CT-838
pub async fn start(
    State(bff): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, Failure> {
    let visit = visit(&bff, &headers)?;
    guard_write(&bff, &visit, &headers).await?;
    // Either capability suffices (D-6591); the caller is refused before anything about the
    // upload is looked at.
    Admission {
        capabilities: &["content-write", "self-file-upload"],
        any: true,
        roles: &[],
        step_up: false,
    }
    .admit(&bff, &visit)
    .await?;
    let path = "/internal/files/uploads/start";
    let request = backend_request(&bff, &visit, path).json(&body);
    answer(&visit, path, request).await
}

/// Send one chunk: `POST /api/files/uploads/:upload/chunks/:index` with the chunk as the body
/// (contract put-upload-chunk).
///
/// Implements: CT-839
pub async fn put_chunk(
    State(bff): State<Shared>,
    headers: HeaderMap,
    Path((upload, index)): Path<(String, String)>,
    body: Bytes,
) -> Result<Json<Json_>, Failure> {
    let visit = visit(&bff, &headers)?;
    guard_write(&bff, &visit, &headers).await?;
    // Either capability suffices (D-6591); the caller is refused before anything about the
    // upload is looked at.
    Admission {
        capabilities: &["content-write", "self-file-upload"],
        any: true,
        roles: &[],
        step_up: false,
    }
    .admit(&bff, &visit)
    .await?;
    let path = "/internal/files/uploads/chunk";
    let request = backend_request(&bff, &visit, path)
        .query(&[("upload", upload.as_str()), ("index", index.as_str())])
        .header("content-type", "application/octet-stream")
        .body(body);
    answer(&visit, path, request).await
}

/// Finish a chunked upload: `POST /api/files/uploads/:upload/finish` (contract finish-upload).
///
/// Implements: CT-840
pub async fn finish(
    State(bff): State<Shared>,
    headers: HeaderMap,
    Path(upload): Path<String>,
) -> Result<Json<Json_>, Failure> {
    let visit = visit(&bff, &headers)?;
    guard_write(&bff, &visit, &headers).await?;
    // Either capability suffices (D-6591); the caller is refused before anything about the
    // upload is looked at.
    Admission {
        capabilities: &["content-write", "self-file-upload"],
        any: true,
        roles: &[],
        step_up: false,
    }
    .admit(&bff, &visit)
    .await?;
    let path = "/internal/files/uploads/finish";
    let request = backend_request(&bff, &visit, path).json(&json!({ "upload": upload }));
    answer(&visit, path, request).await
}

/// Sign an address for a file: `POST /api/files/sign` with `{file, principal?, expires_at?,
/// object?, record?}` (contract sign-media-address).
///
/// Implements: CT-106
pub async fn sign(
    State(bff): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, Failure> {
    let visit = visit(&bff, &headers)?;
    guard_write(&bff, &visit, &headers).await?;
    let path = "/internal/files/sign";
    let request = backend_request(&bff, &visit, path).json(&body);
    answer(&visit, path, request).await
}

/// Deliver a file at its signed address: `GET /files/:file/:expires/:scope/:signature`
/// (contract deliver-file). The backend checks the address, the session and the right to read,
/// and sets the headers; a refusal is the backend's JSON answer.
///
/// Implements: CT-205
pub async fn deliver(
    State(bff): State<Shared>,
    headers: HeaderMap,
    Path((file, expires, scope, signature)): Path<(String, String, String, String)>,
) -> Result<Response, Failure> {
    let visit = visit(&bff, &headers)?;
    let path = "/internal/files/deliver";
    let range = headers
        .get(RANGE)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 100);
    let request = backend_request(&bff, &visit, path).json(&json!({
        "file": file,
        "expires": expires,
        "scope": scope,
        "signature": signature,
        "range": range,
    }));
    let response = request
        .send()
        .await
        .map_err(|e| unavailable(&visit, path, e.to_string()))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) && status != 416 {
        let mut reply = response
            .json::<ErrorReply>()
            .await
            .map_err(|e| unavailable(&visit, path, e.to_string()))?;
        reply.correlation = visit.correlation.clone();
        return Err(Failure(reply));
    }
    let mut kept: Vec<(HeaderName, HeaderValue)> = Vec::new();
    for name in DELIVERY_HEADERS {
        if let Some(value) = response.headers().get(name) {
            if let Ok(value) = HeaderValue::from_bytes(value.as_bytes()) {
                kept.push((HeaderName::from_static(name), value));
            }
        }
    }
    let body = response
        .bytes()
        .await
        .map_err(|e| unavailable(&visit, path, e.to_string()))?;
    let mut out = Response::new(Body::from(body));
    *out.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
    for (name, value) in kept {
        out.headers_mut().insert(name, value);
    }
    Ok(out)
}
