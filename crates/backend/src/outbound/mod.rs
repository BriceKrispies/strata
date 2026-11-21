//! Calls to addresses that tenants name, and webhooks in both directions (WG-5006).
//!
//! - [`gate`]: the outbound gate, the one function every such call goes through (K-252, D-702):
//!   it parses the address, resolves the host once, judges every resolved address under the
//!   outbound rule ([`rule`]), connects to the address it judged, follows a bounded number of
//!   redirects judging each, and bounds time and response size. [`transport`] is the real
//!   resolver and HTTP client; a test puts its own in their place.
//! - [`signature`]: the signature of a webhook delivery, sent and received alike.
//! - [`secrets`]: the sealed, rotating secrets of subscriptions and endpoints.
//! - [`deliveries`]: outbound webhook deliveries as jobs of the automation worker, their log, and
//!   the subscriptions they go to.
//! - [`inbound`]: inbound webhook endpoints and the deliveries they receive, which start runs.
//! - [`routes`]: the routes of all of it.
//!
//! The identity providers' metadata, keys and token endpoints are fetched through the same gate
//! (`crate::signin::oidc::Http`, FED-261).

pub mod deliveries;
pub mod gate;
pub mod inbound;
pub mod routes;
pub mod rule;
pub mod secrets;
pub mod signature;
pub mod transport;

pub use gate::{Failure, Gate, Method, Policy, Request, Response};

use crate::app::App;
use std::sync::Arc;

/// The capability that managing endpoints and subscriptions needs (INT-1578).
pub const WEBHOOKS_MANAGE: &str = "webhooks-manage";
/// The capability that reading the delivery log needs (INT-152).
pub const WEBHOOKS_READ: &str = "webhooks-read";

/// The gate this process's calls go through: the one installed, else the system gate (with the
/// development exception only in development).
pub fn gate(app: &App) -> Arc<Gate> {
    app.outbound
        .get_or_init(|| Arc::new(Gate::system(app.cfg.dev)))
        .clone()
}

/// Put a gate in place before the first call, as a test does to control resolution and the
/// network. Refused once a gate is in use.
pub fn install(app: &App, gate: Gate) -> Result<(), String> {
    app.outbound
        .set(Arc::new(gate))
        .map_err(|_| "the outbound gate is already in use".to_string())
}

/// A random UUID (version 4) in its text form.
pub(crate) fn random_uuid() -> Result<String, crate::app::ApiError> {
    let mut bytes = [0u8; 16];
    strata_runtime::entropy::fill(&mut bytes).map_err(|e| {
        crate::app::ApiError::new(
            strata_protocol::api::ErrorCode::Unavailable,
            format!("no randomness: {e}"),
        )
    })?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = signature::hex(&bytes);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}
