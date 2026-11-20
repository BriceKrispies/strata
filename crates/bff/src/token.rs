//! Request tokens that the gateway issues to a browser.
//!
//! A browser that has no session yet still has to prove, when it signs in or opens a socket, that
//! the request comes from one of our pages and not from another site. The gateway hands every
//! browser a token with its session reply and checks it again when the browser presents it. The
//! token is bound to the site (the tenant served at the host name) and expires. It is a keyed
//! hash of those two things under a secret that no browser sees, so no other site can make one,
//! and a token issued for one site is worthless at another. Outside development the secret is
//! configured (or derived from the shared service token), so a token survives a restart and is
//! good at every instance; only a development process draws a random one.
//!
//! A signed-in browser carries the request token of its session instead (see `routes`).

use sha2::{Digest, Sha256};

/// How long an issued token is good for.
const LIFETIME_MS: i64 = 12 * 60 * 60 * 1000;

pub struct Issuer {
    secret: [u8; 32],
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// HMAC-SHA-256 (RFC 2104) for a key of at most one block (64 bytes).
fn hmac(key: &[u8], message: &[u8]) -> Vec<u8> {
    let mut block = [0u8; 64];
    block[..key.len().min(64)].copy_from_slice(&key[..key.len().min(64)]);
    let mut inner = Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner);
    outer.finalize().to_vec()
}

/// Compare two secrets without stopping at the first difference.
pub use strata_runtime::routes::same_secret as same;

impl Issuer {
    pub fn new() -> Result<Self, String> {
        let mut secret = [0u8; 32];
        getrandom::getrandom(&mut secret).map_err(|e| format!("no randomness for tokens: {e}"))?;
        Ok(Issuer { secret })
    }

    /// An issuer whose key is derived from `secret`: two issuers built from the same secret issue
    /// tokens each other accepts, and a token outlives a restart.
    pub fn from_secret(secret: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"strata bff request-token key|");
        hasher.update(secret.as_bytes());
        Issuer {
            secret: hasher.finalize().into(),
        }
    }

    fn mac(&self, site: &str, payload: &str) -> String {
        hex(&hmac(
            &self.secret,
            format!("request-token|{site}|{payload}").as_bytes(),
        ))
    }

    /// A fresh token for the browser of `site`, good until `now_ms` plus its lifetime.
    /// Implements: H-004
    pub fn issue(&self, site: &str, now_ms: i64) -> String {
        let mut nonce = [0u8; 8];
        let _ = getrandom::getrandom(&mut nonce);
        let payload = format!("{}.{}", now_ms + LIFETIME_MS, hex(&nonce));
        format!("{payload}.{}", self.mac(site, &payload))
    }

    /// Whether this process issued `token` for `site` and it has not expired.
    /// Implements: H-005
    pub fn check(&self, site: &str, token: &str, now_ms: i64) -> bool {
        let mut parts = token.split('.');
        let (Some(expires), Some(nonce), Some(mac), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return false;
        };
        let Ok(expires_at) = expires.parse::<i64>() else {
            return false;
        };
        let payload = format!("{expires}.{nonce}");
        expires_at > now_ms && same(mac, &self.mac(site, &payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_the_rfc_4231_vector() {
        // Test case 2 of RFC 4231: key "Jefe", data "what do ya want for nothing?".
        assert_eq!(
            hex(&hmac(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    /// Verifies: H-004
    /// Verifies: H-005
    #[test]
    fn a_token_is_good_for_its_site_until_it_expires_and_for_nothing_else() {
        let issuer = Issuer::new().unwrap();
        let token = issuer.issue("fieldnotes", 1_000);
        assert!(issuer.check("fieldnotes", &token, 2_000));
        assert!(
            !issuer.check("tidewater", &token, 2_000),
            "another site's token"
        );
        assert!(
            !issuer.check("fieldnotes", &token, 1_000 + LIFETIME_MS),
            "expired"
        );
        let (head, mac) = token.rsplit_once('.').unwrap();
        let mut forged = mac.to_string();
        forged.replace_range(0..1, if mac.starts_with('0') { "1" } else { "0" });
        assert!(!issuer.check("fieldnotes", &format!("{head}.{forged}"), 2_000));
        assert!(!issuer.check("fieldnotes", &format!("9{token}"), 2_000));
        for junk in ["", ".", "..", "a.b", "1.2.3.4", "x.y.z"] {
            assert!(!issuer.check("fieldnotes", junk, 2_000), "{junk:?}");
        }
        assert!(
            !Issuer::new().unwrap().check("fieldnotes", &token, 2_000),
            "a token of another process"
        );
    }

    /// Verifies: GWY-1402
    #[test]
    fn issuers_built_from_one_secret_accept_each_others_tokens() {
        let first = Issuer::from_secret("a shared secret of some length");
        let second = Issuer::from_secret("a shared secret of some length");
        let token = first.issue("fieldnotes", 1_000);
        assert!(
            second.check("fieldnotes", &token, 2_000),
            "a restart or a second instance"
        );
        assert!(!second.check("tidewater", &token, 2_000));
        assert!(!Issuer::from_secret("another secret").check("fieldnotes", &token, 2_000));
    }
}
