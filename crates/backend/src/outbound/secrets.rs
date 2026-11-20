//! The secrets of webhook subscriptions (outbound) and endpoints (inbound): made from 32 random
//! bytes, shown once, kept only sealed (`crate::sealing`, bound to the tenant and the subscription
//! or endpoint by associated data), written to no log, and rotated with an overlap of twenty-four
//! hours in which the previous secret still verifies (INT-1578, D-6569). A second rotation inside
//! the overlap retires the oldest secret at once, so at most two secrets are ever live.
//!
//! Implements: INT-1578, WFL-009

use crate::app::{ApiError, App};
use serde::{Deserialize, Serialize};
use strata_protocol::api::ErrorCode;

/// How long a rotated secret still verifies: twenty-four hours (D-6569).
pub const OVERLAP_MS: i64 = 24 * 3_600_000;

/// The prefix of a secret's text, so that one found in a file is recognised for what it is.
pub const PREFIX: &str = "whsec_";

/// The sealed secrets of one subscription or endpoint.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SealedSecrets {
    /// The current secret, sealed.
    pub current: String,
    /// The secret before the last rotation, sealed, while it still verifies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
    /// Until when (milliseconds) the previous secret verifies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_valid_until: Option<i64>,
}

impl std::fmt::Debug for SealedSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealedSecrets")
            .field("previous_valid_until", &self.previous_valid_until)
            .finish_non_exhaustive()
    }
}

/// A new secret's text: the prefix and 32 random bytes in hexadecimal. The HMAC key is this
/// text's bytes (INT-2640).
pub fn fresh() -> Result<String, ApiError> {
    Ok(format!("{PREFIX}{}", crate::auth::random_hex(32)?))
}

/// Seal a new secret.
pub fn seal_new(app: &App, aad: &[u8], secret: &str) -> Result<SealedSecrets, ApiError> {
    Ok(SealedSecrets {
        current: crate::sealing::seal(app, aad, secret.as_bytes())?,
        previous: None,
        previous_valid_until: None,
    })
}

/// Rotate: a new current secret, the old current one verifying for [`OVERLAP_MS`] more, and the
/// one before it, if any, retired now. Returns the new sealed secrets, the new secret's text and
/// the time the previous one stops verifying.
pub fn rotate(
    app: &App,
    aad: &[u8],
    held: &SealedSecrets,
    now_ms: i64,
) -> Result<(SealedSecrets, String, i64), ApiError> {
    let secret = fresh()?;
    let until = now_ms.saturating_add(OVERLAP_MS);
    let sealed = SealedSecrets {
        current: crate::sealing::seal(app, aad, secret.as_bytes())?,
        previous: Some(held.current.clone()),
        previous_valid_until: Some(until),
    };
    Ok((sealed, secret, until))
}

/// The secrets that verify at `now_ms`, opened, the current one first. A secret that cannot be
/// opened (sealed under a key this process no longer holds) is left out.
pub fn live(app: &App, aad: &[u8], held: &SealedSecrets, now_ms: i64) -> Vec<Vec<u8>> {
    let mut secrets = Vec::new();
    if let Some(current) = crate::sealing::open(app, aad, &held.current) {
        secrets.push(current);
    }
    if let (Some(previous), Some(until)) = (&held.previous, held.previous_valid_until) {
        if now_ms < until {
            if let Some(previous) = crate::sealing::open(app, aad, previous) {
                secrets.push(previous);
            }
        }
    }
    secrets
}

/// The refusal when no secret of a subscription can be opened.
pub fn unopenable() -> ApiError {
    ApiError::hidden(
        ErrorCode::Internal,
        "no secret of the webhook can be opened with the keys held",
    )
}
