//! The webhook signature, the same for deliveries the platform sends and deliveries it receives
//! (WFL-010, WFL-255, INT-150, INT-1577, INT-2640).
//!
//! A delivery carries three headers:
//!
//! - `Strata-Event-Id`: the event's identifier, the same on every attempt of one delivery, by
//!   which a receiver drops a repeat;
//! - `Strata-Timestamp`: the time of the attempt in whole seconds since 1970 (UTC), in decimal;
//! - `Strata-Signature`: `v1=` and the lower-case hexadecimal HMAC-SHA256, keyed with the bytes of
//!   the secret's text, of the event identifier, a full stop, the timestamp, a full stop and the
//!   raw body. While a rotated secret still verifies, an outbound delivery carries one `v1=` entry
//!   for each secret, separated by spaces.
//!
//! The signature covers the event identifier as well as the timestamp and the body, so a captured
//! delivery cannot be sent again under a new identifier to get past the receiver's deduplication.
//! A comparison does not stop at the first difference (H-022).
//!
//! Implements: H-021, H-022, WFL-010, WFL-255, INT-2640

use sha2::{Digest, Sha256};

/// The event identifier header.
pub const HEADER_EVENT: &str = "strata-event-id";
/// The timestamp header.
pub const HEADER_TIMESTAMP: &str = "strata-timestamp";
/// The signature header.
pub const HEADER_SIGNATURE: &str = "strata-signature";
/// The scheme prefix of a signature entry.
const SCHEME: &str = "v1=";
/// The most entries a signature header may carry that are looked at.
const MAX_ENTRIES: usize = 4;

/// HMAC-SHA256 (RFC 2104) of `message` under `key`.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        let digest = Sha256::digest(key);
        block[..digest.len()].copy_from_slice(&digest);
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner);
    let mut out = [0u8; 32];
    out.copy_from_slice(&outer.finalize());
    out
}

/// Lower-case hexadecimal.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// What is signed: the event identifier, the timestamp and the raw body.
pub fn signed_message(event: &str, timestamp: i64, body: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(event.len() + 24 + body.len());
    message.extend_from_slice(event.as_bytes());
    message.push(b'.');
    message.extend_from_slice(timestamp.to_string().as_bytes());
    message.push(b'.');
    message.extend_from_slice(body);
    message
}

/// The hexadecimal signature of a delivery under one secret (sign-webhook-body, H-021).
pub fn sign(secret: &[u8], event: &str, timestamp: i64, body: &[u8]) -> String {
    hex(&hmac_sha256(
        secret,
        &signed_message(event, timestamp, body),
    ))
}

/// The value of the signature header for these secrets, the current one first.
pub fn header_value(secrets: &[Vec<u8>], event: &str, timestamp: i64, body: &[u8]) -> String {
    secrets
        .iter()
        .map(|secret| format!("{SCHEME}{}", sign(secret, event, timestamp, body)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether two byte strings are equal, looking at every byte of the longer one whatever the
/// first difference.
pub fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    let mut difference = (a.len() ^ b.len()) as u64;
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        difference |= u64::from(x ^ y);
    }
    difference == 0
}

/// Whether a signature header matches a delivery under any of `secrets` (verify-webhook-signature,
/// H-022). Every entry is compared with every secret, in full, so the time taken says nothing
/// about which part was wrong.
pub fn verify(secrets: &[Vec<u8>], event: &str, timestamp: i64, body: &[u8], header: &str) -> bool {
    let message = signed_message(event, timestamp, body);
    let expected: Vec<String> = secrets
        .iter()
        .map(|secret| hex(&hmac_sha256(secret, &message)))
        .collect();
    let mut matched = false;
    for entry in header.split_ascii_whitespace().take(MAX_ENTRIES) {
        let given = entry.strip_prefix(SCHEME).unwrap_or("");
        for want in &expected {
            matched |= same_bytes(given.as_bytes(), want.as_bytes()) && !given.is_empty();
        }
    }
    matched
}
