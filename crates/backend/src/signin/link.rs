//! Sign in with a link sent by email.
//!
//! A person gives an address; if it is the login of an account, a message with a link goes to it.
//! The link carries a single-use token and the identifier of a challenge the server kept, and it
//! is good only in the browser that asked.
//!
//! - **Uniform answer.** The answer to a request is the same whether or not the address has an
//!   account, and so is what it costs: the message is handed to the transport, which delivers it
//!   off this thread (IDN-006).
//! - **Stored as hashes.** The challenge row holds the hash of the token, the hash of a binding
//!   nonce and the principal, and no email address (PRV-365).
//! - **Bound to the browser.** The binding nonce goes to the requesting browser as a `__Host-`
//!   cookie and is not in the link. The link is accepted only when the browser presents both the
//!   token and that nonce (IDN-005).
//! - **Once, and for a while.** The row is taken in the step that reads it, so a second use finds
//!   nothing (IDN-003), and it is accepted through its lifetime and not after (IDN-004). A link
//!   that is refused for any reason has been taken: the person asks for another.
//! - **Return address.** A `next` that is not a path on the site is dropped, not followed.
//!
//! Implements: FED-009, H-038, IDN-003, IDN-004, IDN-005, IDN-006, IDN-269, PRV-365

use super::{normalise_login, rules, throttled, SignInMethod};
use crate::app::{ApiError, App, LOG};
use crate::auth::{random_hex, same_secret, token_hash, Caller};
use crate::mail::Mail;
use serde_json::{json, Value as Json};
use strata_kernel::Timestamp;
use strata_protocol::api::ErrorCode;
use strata_storage_api::{Account, SignInLink, ThrottleVerdict};

pub struct LinkMethod;

/// The longest address a request may carry (RFC 5321).
const ADDRESS_BYTES: usize = 254;
/// The longest return address that is followed.
const NEXT_BYTES: usize = 512;
/// A challenge identifier, a token and a binding nonce: 32 random bytes in hex each.
const HEX_LEN: usize = 64;

pub(super) fn well_formed(value: &str) -> bool {
    value.len() == HEX_LEN && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A return address that is a path on this site: it starts with one slash and names no other
/// host, takes no backslash (which browsers read as a slash) and holds no control character.
/// Anything else is `None`, and the person lands on the default page.
///
/// Implements: FED-250, H-035
pub fn same_site_path(next: &str) -> Option<&str> {
    let ok = next.len() <= NEXT_BYTES
        && next.starts_with('/')
        && !next.starts_with("//")
        && !next.contains('\\')
        && !next.chars().any(char::is_control);
    ok.then_some(next)
}

/// An origin in the one spelling that is compared and put in links: `http(s)://host[:port]`.
pub fn site_origin(origin: &str) -> Option<String> {
    let origin = origin.trim_end_matches('/').to_ascii_lowercase();
    let rest = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))?;
    let plain = !rest.is_empty()
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'));
    plain.then_some(origin)
}

/// Percent-encode a value for a query string.
pub(super) fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// The address of the link. The token is in the fragment, which a browser does not send
/// anywhere (D-703, K-250).
fn link_address(origin: &str, next: Option<&str>, challenge: &str, token: &str) -> String {
    let query = next.map_or(String::new(), |next| format!("?next={}", encode(next)));
    format!("{origin}/login{query}#c={challenge}&t={token}")
}

/// Begin a sign-in by link: keep a challenge and send the message when the address has an
/// account, and answer the same either way. The reply holds the binding nonce for the browser's
/// cookie, which the gateway keeps to itself.
///
/// Implements: IDN-006, IDN-269, PRV-365
pub fn request(app: &App, caller: &Caller, body: &Json) -> Result<Json, ApiError> {
    let Some(origin) = caller.origin.as_deref().and_then(site_origin) else {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "This request did not come from the site.",
        ));
    };
    let address = body
        .get("email")
        .and_then(Json::as_str)
        .filter(|email| email.len() <= 4 * ADDRESS_BYTES)
        .map(normalise_login)
        .filter(|email| email.len() <= ADDRESS_BYTES && email.contains('@'));
    // One nonce per browser: a browser that asks twice has one binding for both links.
    let binding = match body.get("binding").and_then(Json::as_str) {
        Some(binding) if well_formed(binding) => binding.to_string(),
        _ => random_hex(HEX_LEN / 2)?,
    };

    // Counted before anything else, per address, per source and per realm. An address that is
    // not well-formed is counted as the empty one; every count is kept, none is given back.
    let rules = rules(
        app,
        "link-request",
        address.as_deref().unwrap_or(""),
        &caller.source,
    );
    if let ThrottleVerdict::Refused { .. } =
        app.store
            .throttle_reserve(&caller.site, &rules, app.now())?
    {
        return Err(throttled());
    }

    // An address in a domain that a provider decides for is answered like one with no account:
    // the provider is the way in for it (FED-009).
    let account = match &address {
        Some(address) if !app.cfg.federation.domain_bound(&caller.site, address) => {
            app.store.account(&caller.site, address)?
        }
        _ => None,
    };
    if let (Some(account), Some(address)) = (account, address) {
        let challenge = random_hex(HEX_LEN / 2)?;
        let token = random_hex(HEX_LEN / 2)?;
        let now = app.now();
        app.store.put_sign_in_link(&SignInLink {
            tenant: caller.site.clone(),
            id_hash: token_hash(&challenge),
            principal: account.principal,
            token_hash: token_hash(&token),
            binding_hash: token_hash(&binding),
            origin: origin.clone(),
            expires_at: Timestamp(now.0 + app.cfg.link_ttl_ms),
        })?;
        let next = body
            .get("next")
            .and_then(Json::as_str)
            .and_then(same_site_path);
        let minutes = (app.cfg.link_ttl_ms / 60_000).max(1);
        let mail = Mail {
            to: address,
            subject: "Your sign-in link".into(),
            text: format!(
                "Open this link in the browser where you asked for it, within {minutes} minutes. \
                 It works once.\n\n{}\n\nIf you did not ask for it, ignore this message.",
                link_address(&origin, next, &challenge, &token)
            ),
        };
        if let Err(error) = app.cfg.mail.send(&mail) {
            // Not told to the person: that the address has an account is not for them to learn.
            LOG.error(
                "link.not_sent",
                &caller.correlation,
                json!({ "tenant": caller.site.as_str(), "error": error.0 }),
            );
        }
    }
    LOG.info(
        "link.requested",
        &caller.correlation,
        json!({ "tenant": caller.site.as_str() }),
    );
    Ok(json!({ "binding": binding, "max_age": (app.cfg.link_ttl_ms / 1000).max(1) }))
}

impl SignInMethod for LinkMethod {
    fn name(&self) -> &'static str {
        "link"
    }

    /// The challenge the link names. The account is not known until the link is checked, so the
    /// limit on an account counts the challenge; the source's and the realm's count the attempt.
    fn subject(&self, request: &Json) -> Option<String> {
        let field = |name: &str| request.get(name)?.as_str().filter(|v| well_formed(v));
        field("token")?;
        field("binding")?;
        field("challenge").map(str::to_string)
    }

    /// Implements: IDN-003, IDN-004, IDN-005
    fn verify(
        &self,
        app: &App,
        caller: &Caller,
        subject: &str,
        request: &Json,
    ) -> Result<Option<Account>, ApiError> {
        let text = |name: &str| request.get(name).and_then(Json::as_str).unwrap_or_default();
        let Some(link) = app
            .store
            .take_sign_in_link(&caller.site, &token_hash(subject))?
        else {
            return Ok(None);
        };
        // Every check is made, and compared in constant time, whichever one fails.
        let in_time = app.now() <= link.expires_at;
        let token = same_secret(&token_hash(text("token")), &link.token_hash);
        let browser = same_secret(&token_hash(text("binding")), &link.binding_hash);
        let site =
            caller.origin.as_deref().and_then(site_origin).as_deref() == Some(link.origin.as_str());
        if !(in_time && token && browser && site) {
            return Ok(None);
        }
        app.store
            .account_of(&caller.site, &link.principal)
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_path_on_this_site_is_followed() {
        for good in ["/desk", "/desk/edit/a-b?x=1#y", "/"] {
            assert_eq!(same_site_path(good), Some(good), "{good}");
        }
        for bad in [
            "",
            "desk",
            "//evil.example",
            "/\\evil.example",
            "\\\\evil.example",
            "https://evil.example",
            "javascript:alert(1)",
            "/ok\nLocation: x",
            "/ok\t",
        ] {
            assert_eq!(same_site_path(bad), None, "{bad:?}");
        }
        assert_eq!(same_site_path(&format!("/{}", "a".repeat(600))), None);
    }

    #[test]
    fn the_address_of_a_link_keeps_its_token_out_of_the_query() {
        let url = link_address("https://a.example", Some("/desk?x=1"), "c", "t");
        assert_eq!(url, "https://a.example/login?next=%2Fdesk%3Fx%3D1#c=c&t=t");
        assert_eq!(
            site_origin("https://A.example/"),
            Some("https://a.example".into())
        );
        assert_eq!(site_origin("https://a.example/x"), None);
    }
}
