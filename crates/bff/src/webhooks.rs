//! The webhook routes at the edge: the owner's delivery log, endpoints and subscriptions under
//! `/api/webhooks/`, and `POST /webhooks/inbound/<endpoint>`, where an external system delivers.
//! Each is relayed to the backend, which decides; the writes pass the write guard first.
//!
//! An inbound delivery carries no session and no request token: what proves it is its signature,
//! which the backend checks over the raw body. The body is relayed byte for byte, in base64, with
//! the three headers that sign it; a body over one mebibyte is refused here already.
//!
//! Implements: CT-172, CT-173, CT-832, CT-833, CT-1630, CT-1631

use crate::routes::{backend, guard_write, uncached, visit, Failure};
use crate::Shared;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::Json;
use serde_json::{json, Value as Json_};
use std::collections::BTreeMap;
use strata_protocol::api::ErrorCode;

/// The most an inbound delivery's body may hold: one mebibyte (INT-1577).
const MAX_INBOUND_BODY: usize = 1024 * 1024;

/// `GET /api/webhooks/deliveries?cursor=&limit=` (list-webhook-deliveries).
pub async fn deliveries(
    State(bff): State<Shared>,
    headers: HeaderMap,
    Query(query): Query<BTreeMap<String, String>>,
) -> Result<Response, Failure> {
    let visit = visit(&bff, &headers)?;
    let mut body = serde_json::Map::new();
    if let Some(cursor) = query
        .get("cursor")
        .filter(|v| !v.is_empty() && v.len() <= 100)
    {
        body.insert("cursor".into(), json!(cursor));
    }
    if let Some(limit) = query.get("limit").and_then(|v| v.parse::<u64>().ok()) {
        body.insert("limit".into(), json!(limit));
    }
    let reply: Json_ = backend(
        &bff,
        &visit,
        "/internal/webhooks/deliveries",
        &Json_::Object(body),
    )
    .await?;
    Ok(uncached(reply))
}

/// The writes, by the last part of their path, with the backend route and the inputs passed on.
const WRITES: &[(&str, &str, &[&str])] = &[
    (
        "endpoints",
        "/internal/webhooks/endpoints/create",
        &["endpoint", "workflow"],
    ),
    (
        "endpoints/rotate",
        "/internal/webhooks/endpoints/rotate",
        &["endpoint"],
    ),
    (
        "subscriptions",
        "/internal/webhooks/subscriptions/create",
        &["target"],
    ),
    (
        "subscriptions/rotate",
        "/internal/webhooks/subscriptions/rotate",
        &["subscription"],
    ),
];

async fn write(
    bff: Shared,
    headers: HeaderMap,
    body: Json_,
    name: &str,
) -> Result<Response, Failure> {
    let visit = visit(&bff, &headers)?;
    guard_write(&bff, &visit, &headers).await?;
    let Some((_, path, inputs)) = WRITES.iter().find(|(key, _, _)| *key == name) else {
        return Err(Failure::new(
            ErrorCode::NotFound,
            "No such endpoint.",
            &visit.correlation,
        ));
    };
    let mut request = serde_json::Map::new();
    for input in *inputs {
        request.insert(
            (*input).to_string(),
            body.get(*input).cloned().unwrap_or(Json_::Null),
        );
    }
    let reply: Json_ = backend(&bff, &visit, path, &Json_::Object(request)).await?;
    Ok(uncached(reply))
}

/// `POST /api/webhooks/endpoints` (create-webhook-endpoint).
pub async fn endpoint_create(
    State(bff): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Response, Failure> {
    write(bff, headers, body, "endpoints").await
}

/// `POST /api/webhooks/endpoints/rotate` (rotate-webhook-secret).
pub async fn endpoint_rotate(
    State(bff): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Response, Failure> {
    write(bff, headers, body, "endpoints/rotate").await
}

/// `POST /api/webhooks/subscriptions` (create-webhook-subscription).
pub async fn subscription_create(
    State(bff): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Response, Failure> {
    write(bff, headers, body, "subscriptions").await
}

/// `POST /api/webhooks/subscriptions/rotate` (rotate-webhook-subscription-secret).
pub async fn subscription_rotate(
    State(bff): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Response, Failure> {
    write(bff, headers, body, "subscriptions/rotate").await
}

/// A header's text, or the empty text.
fn header<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
}

/// Standard base64 with padding (RFC 4648).
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// `POST /webhooks/inbound/<endpoint>` (receive-inbound-webhook): the raw body and the headers
/// `Strata-Event-Id`, `Strata-Timestamp` and `Strata-Signature`.
pub async fn inbound(
    State(bff): State<Shared>,
    headers: HeaderMap,
    Path(endpoint): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Failure> {
    let mut visit = visit(&bff, &headers)?;
    // A delivery acts for nobody: whatever session cookie came along is not passed on.
    visit.token = None;
    visit.idempotency_key = None;
    if body.len() > MAX_INBOUND_BODY {
        let mut failure = Failure::new(
            ErrorCode::Invalid,
            "The delivery's body is larger than one mebibyte.",
            &visit.correlation,
        );
        failure.0.kind = Some("too-large".into());
        return Err(failure);
    }
    let request = json!({
        "endpoint": endpoint,
        "body": base64(&body),
        "event_id": header(&headers, "strata-event-id"),
        "timestamp": header(&headers, "strata-timestamp"),
        "signature": header(&headers, "strata-signature"),
    });
    let reply: Json_ = backend(&bff, &visit, "/internal/webhooks/inbound", &request).await?;
    Ok(uncached(reply))
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_is_the_standard_alphabet_with_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xfb, 0xff]), "+/8=");
    }
}
