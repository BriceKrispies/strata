//! The keys a tenant's access tokens are signed with.
//!
//! Each tenant has its own ES256 (P-256) keys, so a token signed for one tenant is verified with
//! that tenant's keys and no other's (TOK-006). One key is active and signs; a key that was
//! rotated out is retired and still verifies, until every token it signed has expired
//! (TOK-008); a key that was compromised is refused from the moment it is marked, and is in no
//! published set (TOK-251).
//!
//! The private half is kept in the tenant's documents as PEM. It is not wrapped by a key service
//! here: the key service is its own component of the specification and is not built yet.
//!
//! Implements: TOK-008, TOK-251

use super::tokens::TOKEN_TTL_MS;
use super::{list, put};
use crate::app::{ApiError, App};
use crate::auth::random_hex;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use openssl::ec::{EcGroup, EcKey};
use openssl::nid::Nid;
use openssl::pkey::PKey;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use strata_kernel::TenantId;
use strata_protocol::api::ErrorCode;

pub const KIND: &str = "signing-key";

/// How long a retired key still verifies: every token it signed has expired by then, because no
/// token lives longer than `TOKEN_TTL_MS`.
const RETIRED_GRACE_MS: i64 = TOKEN_TTL_MS + 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyState {
    Active,
    Retired,
    Compromised,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SigningKey {
    pub kid: String,
    pub private_pem: String,
    pub public_pem: String,
    /// The public point, base64url, for the published set.
    pub x: String,
    pub y: String,
    pub state: KeyState,
    pub created_at: i64,
    pub retired_at: Option<i64>,
    pub compromised_at: Option<i64>,
}

fn internal(why: impl std::fmt::Display) -> ApiError {
    ApiError::hidden(ErrorCode::Internal, format!("signing key: {why}"))
}

fn generate(now: i64) -> Result<SigningKey, ApiError> {
    let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).map_err(internal)?;
    let ec = EcKey::generate(&group).map_err(internal)?;
    let pkey = PKey::from_ec_key(ec).map_err(internal)?;
    let private_pem =
        String::from_utf8(pkey.private_key_to_pem_pkcs8().map_err(internal)?).map_err(internal)?;
    let public_pem =
        String::from_utf8(pkey.public_key_to_pem().map_err(internal)?).map_err(internal)?;
    // The last 65 bytes of a P-256 public key's DER are the uncompressed point: 0x04, x, y.
    let der = pkey.public_key_to_der().map_err(internal)?;
    if der.len() < 65 || der[der.len() - 65] != 4 {
        return Err(internal(
            "the public key is not an uncompressed P-256 point",
        ));
    }
    let point = &der[der.len() - 65..];
    Ok(SigningKey {
        kid: format!("k_{}", random_hex(8)?),
        private_pem,
        public_pem,
        x: URL_SAFE_NO_PAD.encode(&point[1..33]),
        y: URL_SAFE_NO_PAD.encode(&point[33..65]),
        state: KeyState::Active,
        created_at: now,
        retired_at: None,
        compromised_at: None,
    })
}

fn all(app: &App, tenant: &TenantId) -> Result<Vec<SigningKey>, ApiError> {
    list(app, tenant, KIND)
}

/// The key that signs now: the newest active one. A tenant that has none yet gets one.
pub fn active(app: &App, tenant: &TenantId) -> Result<SigningKey, ApiError> {
    let now = app.now().0;
    if let Some(key) = all(app, tenant)?
        .into_iter()
        .filter(|key| key.state == KeyState::Active)
        .max_by_key(|key| key.created_at)
    {
        return Ok(key);
    }
    let key = generate(now)?;
    put(app, tenant, KIND, &key.kid, &key, true)?;
    Ok(key)
}

/// The key a token names, if it may verify a token now: an active key, or a retired one while a
/// token it signed could still be alive. A compromised key never does (TOK-251).
pub fn verifying(app: &App, tenant: &TenantId, kid: &str) -> Result<Option<SigningKey>, ApiError> {
    let Some(key) = super::get::<SigningKey>(app, tenant, KIND, kid)? else {
        return Ok(None);
    };
    Ok(usable(&key, app.now().0).then_some(key))
}

fn usable(key: &SigningKey, now: i64) -> bool {
    match key.state {
        KeyState::Active => true,
        KeyState::Retired => key
            .retired_at
            .is_some_and(|at| now < at.saturating_add(RETIRED_GRACE_MS)),
        KeyState::Compromised => false,
    }
}

/// The published set: the public halves of the keys that verify now (TOK-008, TOK-251).
pub fn published(app: &App, tenant: &TenantId) -> Result<Json, ApiError> {
    // A tenant that has never issued has no key; asking for the set makes the first one, so
    // that the set a verifier fetches before the first token is not empty.
    active(app, tenant)?;
    let now = app.now().0;
    let keys: Vec<Json> = all(app, tenant)?
        .iter()
        .filter(|key| usable(key, now))
        .map(|key| {
            json!({
                "kty": "EC", "crv": "P-256", "alg": "ES256", "use": "sig",
                "kid": key.kid, "x": key.x, "y": key.y,
            })
        })
        .collect();
    Ok(json!({ "keys": keys }))
}

/// Rotate: a new key signs from now on, and the keys that signed until now are retired and verify
/// until their tokens have expired (TOK-008).
pub fn rotate(app: &App, tenant: &TenantId, correlation: &str) -> Result<Json, ApiError> {
    let now = app.now().0;
    let fresh = generate(now)?;
    put(app, tenant, KIND, &fresh.kid, &fresh, true)?;
    for mut key in all(app, tenant)? {
        if key.state == KeyState::Active && key.kid != fresh.kid {
            key.state = KeyState::Retired;
            key.retired_at = Some(now);
            put(app, tenant, KIND, &key.kid.clone(), &key, false)?;
        }
    }
    app.audit(
        "signing_key.rotated",
        correlation,
        json!({ "tenant": tenant.as_str(), "kid": &fresh.kid }),
    );
    Ok(json!({ "kid": fresh.kid }))
}

/// A key was compromised: refused at once, and replaced when it was the one signing (TOK-251).
pub fn compromise(
    app: &App,
    tenant: &TenantId,
    kid: &str,
    correlation: &str,
) -> Result<Json, ApiError> {
    let now = app.now().0;
    let Some(mut key) = super::get::<SigningKey>(app, tenant, KIND, kid)? else {
        return Err(ApiError::new(ErrorCode::NotFound, "No such key."));
    };
    key.state = KeyState::Compromised;
    key.compromised_at = Some(now);
    put(app, tenant, KIND, kid, &key, false)?;
    // Whatever happens next, a key signs: the next one is made here and not at the next token.
    active(app, tenant)?;
    app.audit(
        "signing_key.compromised",
        correlation,
        json!({ "tenant": tenant.as_str(), "kid": kid }),
    );
    Ok(json!({ "kid": kid, "state": "compromised" }))
}
