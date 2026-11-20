//! Sealing a secret at rest: AES-256-GCM under the platform's factor key, bound to what the secret
//! belongs to by associated data, with a fresh nonce for every sealing.
//!
//! A sealed secret is `<key id>.<nonce, ciphertext and tag, URL-safe base64>`. The key id names the
//! key that sealed it, so a rotation can keep the old key beside the new one: a secret is opened
//! by the key it names, and sealed again under the current key when it is next written. A secret
//! sealed before key ids existed has no `.` and is tried against every key.

use crate::app::{ApiError, App, SealKey};
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use strata_protocol::api::ErrorCode;

const NONCE_BYTES: usize = 12;
const TAG_BYTES: usize = 16;

/// Seal `secret` under the current key, bound to `aad`.
pub fn seal(app: &App, aad: &[u8], secret: &[u8]) -> Result<String, ApiError> {
    let key = &app.cfg.factors.seal_key;
    let mut nonce = [0u8; NONCE_BYTES];
    getrandom::getrandom(&mut nonce)
        .map_err(|e| ApiError::new(ErrorCode::Unavailable, format!("no randomness: {e}")))?;
    let cipher = Aes256Gcm::new_from_slice(&key.0)
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, format!("sealing a secret: {e}")))?;
    // AES-256-GCM's output is the ciphertext followed by its 16-byte tag.
    let sealed = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: secret, aad })
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, format!("sealing a secret: {e}")))?;
    let mut out = Vec::with_capacity(NONCE_BYTES + sealed.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    Ok(format!("{}.{}", key.id(), URL_SAFE_NO_PAD.encode(out)))
}

fn open_with(key: &SealKey, aad: &[u8], bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < NONCE_BYTES + TAG_BYTES {
        return None;
    }
    let (nonce, sealed_and_tag) = bytes.split_at(NONCE_BYTES);
    let cipher = Aes256Gcm::new_from_slice(&key.0).ok()?;
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: sealed_and_tag,
                aad,
            },
        )
        .ok()
}

/// Open a sealed secret, or `None` when it was not sealed for `aad` under a key we hold.
pub fn open(app: &App, aad: &[u8], sealed: &str) -> Option<Vec<u8>> {
    let policy = &app.cfg.factors;
    let keys = || std::iter::once(&policy.seal_key).chain(policy.previous_seal_keys.iter());
    match sealed.split_once('.') {
        Some((id, body)) => {
            let bytes = URL_SAFE_NO_PAD.decode(body).ok()?;
            let key = keys().find(|key| key.id() == id)?;
            open_with(key, aad, &bytes)
        }
        None => {
            let bytes = URL_SAFE_NO_PAD.decode(sealed).ok()?;
            keys().find_map(|key| open_with(key, aad, &bytes))
        }
    }
}
