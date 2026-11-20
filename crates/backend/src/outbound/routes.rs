//! The routes of webhooks: the owner's delivery log, endpoints and subscriptions, and the
//! receipt of an inbound delivery, which the BFF relays with the raw body in base64.
//!
//! | Route | Contract |
//! |---|---|
//! | `POST /internal/webhooks/deliveries` | list-webhook-deliveries (CT-173) |
//! | `POST /internal/webhooks/endpoints/create` | create-webhook-endpoint (CT-832) |
//! | `POST /internal/webhooks/endpoints/rotate` | rotate-webhook-secret (CT-833) |
//! | `POST /internal/webhooks/subscriptions/create` | create-webhook-subscription (CT-1630) |
//! | `POST /internal/webhooks/subscriptions/rotate` | rotate-webhook-subscription-secret (CT-1631) |
//! | `POST /internal/webhooks/inbound` | receive-inbound-webhook (CT-172) |

use super::{deliveries, inbound};
use crate::app::{ApiError, Shared};
use crate::http::{as_caller, as_caller_at};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::Value as Json_;
use strata_protocol::api::ErrorCode;

/// The routes, to be merged into the backend's router.
pub fn router() -> Router<Shared> {
    Router::new()
        .route("/internal/webhooks/deliveries", post(deliveries_route))
        .route("/internal/webhooks/endpoints/create", post(endpoint_create))
        .route("/internal/webhooks/endpoints/rotate", post(endpoint_rotate))
        .route(
            "/internal/webhooks/subscriptions/create",
            post(subscription_create),
        )
        .route(
            "/internal/webhooks/subscriptions/rotate",
            post(subscription_rotate),
        )
        .route("/internal/webhooks/inbound", post(inbound_route))
}

/// Implements: CT-173
async fn deliveries_route(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        deliveries::list_deliveries(app, identity, &body)
    })
    .await
}

/// Implements: CT-832
async fn endpoint_create(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        inbound::create_endpoint(app, identity, &body)
    })
    .await
}

/// Implements: CT-833
async fn endpoint_rotate(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        inbound::rotate_endpoint(app, identity, &body)
    })
    .await
}

/// Implements: CT-1630
async fn subscription_create(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        deliveries::create_subscription(app, identity, &body)
    })
    .await
}

/// Implements: CT-1631
async fn subscription_rotate(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        deliveries::rotate_subscription(app, identity, &body)
    })
    .await
}

/// A text field of the relayed delivery; a missing one is the empty text, which is refused as
/// `unauthenticated` with everything else that is wrong with a delivery's headers.
fn field<'a>(body: &'a Json_, name: &str) -> &'a str {
    body.get(name).and_then(Json_::as_str).unwrap_or("")
}

/// `{ endpoint, body (base64), event_id, timestamp, signature }`, as the BFF relays a delivery.
/// Implements: CT-172
async fn inbound_route(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller_at(&app, &headers, move |app, caller, _identity| {
        let raw = STANDARD
            .decode(field(&body, "body"))
            .map_err(|_| ApiError::new(ErrorCode::Invalid, "The body is not base64."))?;
        let delivery = inbound::InboundDelivery {
            endpoint: field(&body, "endpoint"),
            body: &raw,
            event: field(&body, "event_id"),
            timestamp: field(&body, "timestamp"),
            signature: field(&body, "signature"),
        };
        inbound::receive(
            app,
            &caller.site,
            caller.visitor_source(),
            &delivery,
            app.now(),
        )
    })
    .await
}
