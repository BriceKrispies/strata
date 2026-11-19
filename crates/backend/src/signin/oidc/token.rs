//! Checking an OpenID Connect identity token.
//!
//! The signature and the standard claims are checked by the `jsonwebtoken` crate; this module
//! decides what it is asked to check and what is checked beyond it.
//!
//! - **Signature method.** Only the methods the provider's configuration names are accepted, and
//!   the key that verifies the token must be of the family of that method, so that a token that
//!   names a weaker method, a shared-secret method or none is refused before any key is tried
//!   (FED-255). `none` is not a method the library can be asked to accept.
//! - **Key.** The key is the one the token's `kid` names, from this provider's keys only
//!   (FED-256). A token that names a key that is not held is `UnknownKey`, which the caller
//!   answers by fetching the keys once more (FED-004).
//! - **Claims.** Issuer, audience, expiry and not-before by the library with a small skew;
//!   `sub`, `iat` (not in the future), `azp` and the nonce here.
//!
//! Implements: FED-003, FED-004, FED-255, FED-256, H-015

use super::config::ProviderConfig;
use crate::auth::same_secret;
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{decode, decode_header, DecodingKey, Validation};
use serde::Deserialize;
use serde_json::Value as Json;

/// How far a token's clock may differ from this process's.
const SKEW_SECS: u64 = 60;
/// The longest identity token that is read.
const MAX_TOKEN_BYTES: usize = 16 * 1024;
/// The longest subject identifier (OpenID Connect Core, 2).
const MAX_SUBJECT_BYTES: usize = 255;

/// Why a token was refused. It is for the log, never for the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Malformed,
    Algorithm,
    UnknownKey,
    Signature,
    Issuer,
    Audience,
    AuthorizedParty,
    Expired,
    NotYetValid,
    IssuedAt,
    Nonce,
    Subject,
}

impl Refusal {
    pub fn code(self) -> &'static str {
        match self {
            Refusal::Malformed => "token_malformed",
            Refusal::Algorithm => "token_algorithm",
            Refusal::UnknownKey => "token_unknown_key",
            Refusal::Signature => "token_signature",
            Refusal::Issuer => "token_issuer",
            Refusal::Audience => "token_audience",
            Refusal::AuthorizedParty => "token_azp",
            Refusal::Expired => "token_expired",
            Refusal::NotYetValid => "token_not_yet_valid",
            Refusal::IssuedAt => "token_issued_at",
            Refusal::Nonce => "token_nonce",
            Refusal::Subject => "token_subject",
        }
    }
}

/// What a verified token says about the person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asserted {
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub name: Option<String>,
}

#[derive(Deserialize)]
struct Claims {
    sub: Option<String>,
    iat: Option<i64>,
    nonce: Option<String>,
    azp: Option<String>,
    aud: Option<Json>,
    email: Option<String>,
    email_verified: Option<Json>,
    name: Option<String>,
}

fn refusal(kind: &ErrorKind) -> Refusal {
    match kind {
        ErrorKind::InvalidSignature => Refusal::Signature,
        ErrorKind::InvalidAlgorithm | ErrorKind::InvalidAlgorithmName => Refusal::Algorithm,
        ErrorKind::InvalidIssuer => Refusal::Issuer,
        ErrorKind::InvalidAudience => Refusal::Audience,
        ErrorKind::ExpiredSignature => Refusal::Expired,
        ErrorKind::ImmatureSignature => Refusal::NotYetValid,
        ErrorKind::InvalidToken
        | ErrorKind::Base64(_)
        | ErrorKind::Json(_)
        | ErrorKind::Utf8(_) => Refusal::Malformed,
        _ => Refusal::Signature,
    }
}

/// Check an identity token against the provider's configuration and keys, and the nonce of the
/// sign-in it answers. `now` is seconds since the epoch.
///
/// Implements: FED-003, FED-255, FED-256
pub fn verify(
    token: &str,
    provider: &ProviderConfig,
    keys: &JwkSet,
    nonce: &str,
    now: i64,
) -> Result<Asserted, Refusal> {
    if token.len() > MAX_TOKEN_BYTES {
        return Err(Refusal::Malformed);
    }
    let header = decode_header(token).map_err(|e| refusal(e.kind()))?;
    if !provider.algorithms.contains(&header.alg) {
        return Err(Refusal::Algorithm);
    }
    let jwk = match header.kid.as_deref() {
        Some(kid) => keys
            .keys
            .iter()
            .find(|key| key.common.key_id.as_deref() == Some(kid)),
        None if keys.keys.len() == 1 => keys.keys.first(),
        None => None,
    }
    .ok_or(Refusal::UnknownKey)?;
    let key = DecodingKey::from_jwk(jwk).map_err(|_| Refusal::Algorithm)?;

    // `Validation::new` takes exactly the one method the token names and that the configuration
    // allows, and the library refuses a key of another family for it.
    let mut validation = Validation::new(header.alg);
    validation.leeway = SKEW_SECS;
    validation.validate_exp = true;
    validation.validate_nbf = true;
    validation.set_issuer(&[provider.issuer.as_str()]);
    validation.set_audience(&[provider.client_id.as_str()]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    let claims = decode::<Claims>(token, &key, &validation)
        .map_err(|e| refusal(e.kind()))?
        .claims;

    let subject = claims
        .sub
        .filter(|sub| !sub.is_empty() && sub.len() <= MAX_SUBJECT_BYTES)
        .ok_or(Refusal::Subject)?;
    match claims.iat {
        Some(iat) if iat <= now + SKEW_SECS as i64 => {}
        _ => return Err(Refusal::IssuedAt),
    }
    // Several audiences need an authorised party that is this client; a named one must be.
    let audiences = match &claims.aud {
        Some(Json::Array(list)) => list.len(),
        _ => 1,
    };
    match claims.azp.as_deref() {
        Some(azp) if azp != provider.client_id => return Err(Refusal::AuthorizedParty),
        None if audiences > 1 => return Err(Refusal::AuthorizedParty),
        _ => {}
    }
    match claims.nonce.as_deref() {
        Some(given) if same_secret(given, nonce) => {}
        _ => return Err(Refusal::Nonce),
    }
    let email_verified = match &claims.email_verified {
        Some(Json::Bool(value)) => *value,
        Some(Json::String(text)) => text == "true",
        _ => false,
    };
    Ok(Asserted {
        subject,
        email: claims.email.filter(|email| !email.is_empty()),
        email_verified,
        name: claims.name.filter(|name| !name.trim().is_empty()),
    })
}
