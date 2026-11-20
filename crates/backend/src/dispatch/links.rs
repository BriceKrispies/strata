//! Unsubscribe and resubscribe by a signed link (change-subscription-by-link, EML-020,
//! EML-1562).
//!
//! A link names the hash of the recipient's address, a category (a category name or
//! `*promotional`), the action and an HMAC-SHA-256 over the tenant and those three, under a key
//! derived from the platform's sealing secret in a domain of its own. Following it needs no
//! sign-in; a link whose signature does not match, or whose category no longer exists, is
//! answered as not found without saying which. Links are issued for promotional categories only,
//! so there is no unsubscribe of a required or transactional category to follow.

use super::store::{self, Suppression, SuppressionReason, ALL_PROMOTIONAL};
use super::Purpose;
use crate::app::{ApiError, App};
use crate::auth::same_secret;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use strata_kernel::{TenantId, Timestamp};
use strata_protocol::api::ErrorCode;

/// What a link asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkAction {
    Unsubscribe,
    Resubscribe,
}

impl LinkAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            LinkAction::Unsubscribe => "unsubscribe",
            LinkAction::Resubscribe => "resubscribe",
        }
    }

    fn parse(text: &str) -> Option<LinkAction> {
        match text {
            "unsubscribe" => Some(LinkAction::Unsubscribe),
            "resubscribe" => Some(LinkAction::Resubscribe),
            _ => None,
        }
    }
}

/// What a signed link names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkClaim {
    pub address_hash: String,
    /// A category name or `*promotional`.
    pub category: String,
    pub action: LinkAction,
}

/// HMAC-SHA-256 (RFC 2104).
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut padded = [0u8; BLOCK];
    if key.len() > BLOCK {
        padded[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        padded[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(padded.map(|b| b ^ 0x36));
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(padded.map(|b| b ^ 0x5c));
    outer.update(inner);
    let mut out = [0u8; 32];
    out.copy_from_slice(&outer.finalize());
    out
}

/// The signature of a claim of `tenant`, in hex.
fn signature(app: &App, tenant: &TenantId, claim: &LinkClaim) -> String {
    let mut key = b"strata/subscription-link-key/v1\0".to_vec();
    key.extend_from_slice(&app.cfg.factors.seal_key.0);
    let key = Sha256::digest(&key);
    let message = format!(
        "{tenant}\0{}\0{}\0{}",
        claim.address_hash,
        claim.category,
        claim.action.as_str()
    );
    hmac_sha256(&key, message.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The query string of a signed link: `r` the address hash, `c` the category, `a` the action and
/// `s` the signature. Each value is hex, a category id, `*promotional` or an action word, so none
/// needs escaping.
pub fn query(app: &App, tenant: &TenantId, claim: &LinkClaim) -> String {
    format!(
        "r={}&c={}&a={}&s={}",
        claim.address_hash,
        claim.category,
        claim.action.as_str(),
        signature(app, tenant, claim)
    )
}

/// The parameters of a query string.
pub fn parameters(query: &str) -> BTreeMap<String, String> {
    let query = query.rsplit_once('?').map_or(query, |(_, q)| q);
    let query = query.split_once('#').map_or(query, |(q, _)| q);
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.to_string(), v.replace("%2A", "*").replace("%2a", "*")))
        .collect()
}

/// The claim of a link of `tenant` whose signature holds, or `None`.
pub fn verify(
    app: &App,
    tenant: &TenantId,
    params: &BTreeMap<String, String>,
) -> Option<LinkClaim> {
    let claim = LinkClaim {
        address_hash: params.get("r")?.clone(),
        category: params.get("c")?.clone(),
        action: LinkAction::parse(params.get("a")?)?,
    };
    let given = params.get("s")?;
    same_secret(given, &signature(app, tenant, &claim)).then_some(claim)
}

/// The links of a promotional message of `category` to the address of `hash`: the one that
/// unsubscribes from the category, and the one for the `List-Unsubscribe` header, which is the
/// same request made by the mail client (EML-020). `origin` is the site's, when it has one.
pub fn unsubscribe_url(
    app: &App,
    tenant: &TenantId,
    origin: Option<&str>,
    hash: &str,
    category: &str,
    api: bool,
) -> String {
    let claim = LinkClaim {
        address_hash: hash.to_string(),
        category: category.to_string(),
        action: LinkAction::Unsubscribe,
    };
    let path = if api {
        "/api/mail/subscription"
    } else {
        "/mail/subscription"
    };
    format!(
        "{}{path}?{}",
        origin.unwrap_or(""),
        query(app, tenant, &claim)
    )
}

/// What following a link did: the category and `unsubscribed` or `subscribed`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Followed {
    pub category: String,
    pub state: &'static str,
}

fn not_found() -> ApiError {
    ApiError::new(ErrorCode::NotFound, "That link is not one we know.")
}

/// Follow a link of `tenant` (change-subscription-by-link): an unsubscribe adds a suppression of
/// reason unsubscribe for the category it names, or for every promotional category; a
/// resubscribe removes a suppression of reason unsubscribe only, and leaves a bounce or a
/// complaint in place (EML-1562).
pub fn follow(
    app: &App,
    tenant: &TenantId,
    params: &BTreeMap<String, String>,
    now: Timestamp,
) -> Result<Followed, ApiError> {
    let claim = verify(app, tenant, params).ok_or_else(not_found)?;
    if claim.category != ALL_PROMOTIONAL {
        let promotional = store::category(app, tenant, &claim.category)?
            .is_some_and(|c| c.purpose == Purpose::Promotional);
        if !promotional {
            return Err(not_found());
        }
    }
    let state = match claim.action {
        LinkAction::Unsubscribe => {
            store::add_suppression(
                app,
                tenant,
                &Suppression {
                    address_hash: claim.address_hash.clone(),
                    category: claim.category.clone(),
                    reason: SuppressionReason::Unsubscribe,
                    recorded_at: now.0,
                },
            )?;
            "unsubscribed"
        }
        LinkAction::Resubscribe => {
            store::remove_suppression(app, tenant, &claim.address_hash, &claim.category, true)?;
            "subscribed"
        }
    };
    app.audit(
        "mail.subscription_changed",
        "",
        serde_json::json!({
            "tenant": tenant.as_str(),
            "category": claim.category,
            "action": claim.action.as_str(),
        }),
    );
    Ok(Followed {
        category: claim.category,
        state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 4231, test case 2.
    #[test]
    fn hmac_matches_the_published_vector() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn parameters_read_a_full_address_or_a_query() {
        let p = parameters(
            "https://x.example/mail/subscription?r=ab&c=%2Apromotional&a=unsubscribe&s=00",
        );
        assert_eq!(p.get("c").map(String::as_str), Some("*promotional"));
        assert_eq!(
            parameters("r=ab&a=resubscribe")
                .get("a")
                .map(String::as_str),
            Some("resubscribe")
        );
    }
}
