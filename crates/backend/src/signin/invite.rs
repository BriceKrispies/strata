//! The first sign-in of a tenant's owner, by an invitation link.
//!
//! A tenant created at run time (`provision::create_tenant`) has an owner whose account has a
//! password nobody knows. The sign-in link of [`super::link`] cannot be the way in: it is bound to
//! the browser that asked for it by a nonce in that browser's cookie, and the owner has asked for
//! nothing yet. This invitation is the other shape of the same idea, made for one purpose.
//!
//! - **Not bound to a browser.** The link works in whatever browser opens it, because the owner
//!   was given it by the operator's message and has no browser state yet. What stands in for the
//!   binding is that the token is 256 random bits that only the message holds.
//! - **Sets up the first sign-in.** Opening the link asks the owner for a password, and accepting
//!   it replaces the unknown one. The owner then signs in as anyone does, and enrols a second
//!   factor in the usual way. The link signs nobody in by itself: a leaked link yields a password
//!   the thief chose only if the thief gets there before the owner, and the owner learns of that
//!   at once, because their own use then fails.
//! - **Once.** The invitation is taken in one step (`doc_remove` says whether this caller removed
//!   it), so of two concurrent uses one finds nothing.
//! - **For a while.** It is accepted through [`INVITE_TTL_MS`] (seven days: an operator's message
//!   is read later than a sign-in link is) and not after.
//! - **Stored as hashes.** The document is kept under the hash of its identifier and holds the
//!   hash of its token, the principal and the site's origin; it holds no address and nothing that
//!   opens the link (PRV-365).
//! - **At the site it was made for.** The origin the request comes from must be the one the
//!   invitation names.
//! - **Counted.** Every attempt counts against the limits of the invitation, the source and the
//!   realm, as a recovery's do.
//!
//! Implements: IDN-1680, IDN-1681, IDN-1682

use super::link::{site_origin, well_formed};
use super::{rules, throttled};
use crate::app::{ApiError, App, Shared, LOG};
use crate::auth::{self, hash_password, random_hex, same_secret, token_hash, Caller};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json as Reply;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use strata_kernel::{PrincipalId, TenantId};
use strata_protocol::api::ErrorCode;
use strata_storage_api::{Account, ThrottleSettle, ThrottleVerdict};

/// The kind of document that holds an invitation, keyed by the hash of its identifier.
pub const KIND: &str = "invite";
/// How long an invitation is accepted: seven days.
pub const INVITE_TTL_MS: i64 = 7 * 24 * 3_600_000;
/// The shortest and longest password accepted (as at recovery).
const PASSWORD_MIN: usize = 10;
const PASSWORD_MAX: usize = 256;
/// An identifier or a token: 32 random bytes in hex.
const HEX_BYTES: usize = 32;

/// What is kept for one invitation. No address, and no secret.
#[derive(Debug, Serialize, Deserialize)]
struct Invite {
    principal: String,
    token_hash: String,
    origin: String,
    expires_at: i64,
}

fn not_accepted() -> ApiError {
    ApiError::new(
        ErrorCode::Unauthenticated,
        "That invitation was not accepted. Ask for a new one.",
    )
}

/// Make an invitation for the account of `principal` at the site served at `origin` and return
/// the address to send to its owner: `{origin}/login#i={id}&t={token}`. The secrets are in the
/// fragment, which a browser sends nowhere.
pub fn issue(
    app: &App,
    tenant: &TenantId,
    principal: &PrincipalId,
    origin: &str,
) -> Result<String, ApiError> {
    issue_for(app, tenant, principal, origin, INVITE_TTL_MS)
}

/// [`issue`] with a lifetime other than the default.
pub fn issue_for(
    app: &App,
    tenant: &TenantId,
    principal: &PrincipalId,
    origin: &str,
    ttl_ms: i64,
) -> Result<String, ApiError> {
    let origin = site_origin(origin).ok_or_else(|| {
        ApiError::new(ErrorCode::Invalid, "The origin must be like https://host.")
    })?;
    let id = random_hex(HEX_BYTES)?;
    let token = random_hex(HEX_BYTES)?;
    let invite = Invite {
        principal: principal.as_str().to_string(),
        token_hash: token_hash(&token),
        origin: origin.clone(),
        expires_at: app.now().0 + ttl_ms,
    };
    let text = serde_json::to_string(&invite)
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, e.to_string()))?;
    if !app
        .store
        .doc_put(tenant, KIND, &token_hash(&id), &text, true)?
    {
        return Err(ApiError::hidden(
            ErrorCode::Internal,
            "an invitation identifier collided",
        ));
    }
    Ok(format!("{origin}/login#i={id}&t={token}"))
}

/// The text of the message that carries an invitation to the owner of a new site.
pub fn first_sign_in_text(address: &str) -> String {
    let days = INVITE_TTL_MS / (24 * 3_600_000);
    format!(
        "Your site is ready. Open this link to choose your password and sign in for the first \
         time. It works once, in any browser, within {days} days.\n\n{address}\n\nIf you did not \
         expect this message, ignore it."
    )
}

/// Accept an invitation: set the owner's password. The body holds the `challenge` and `token` of
/// the link and the new `password`.
///
/// Implements: IDN-1680, IDN-1681, IDN-1682
pub fn accept(app: &App, caller: &Caller, body: &Json) -> Result<Json, ApiError> {
    let text = |name: &str| body.get(name).and_then(Json::as_str).unwrap_or_default();
    let (id, token, password) = (text("challenge"), text("token"), text("password"));
    if !well_formed(id) || !well_formed(token) {
        return Err(not_accepted());
    }
    if password.len() < PASSWORD_MIN {
        return Err(ApiError::new(
            ErrorCode::Invalid,
            format!("Choose a password of at least {PASSWORD_MIN} characters."),
        ));
    }
    if password.len() > PASSWORD_MAX {
        return Err(ApiError::new(
            ErrorCode::Invalid,
            format!("Choose a password of at most {PASSWORD_MAX} characters."),
        ));
    }
    let rules = rules(app, "invite-accept", id, &caller.source);
    if let ThrottleVerdict::Refused { .. } =
        app.store
            .throttle_reserve(&caller.site, &rules, app.now())?
    {
        return Err(throttled());
    }
    let key = token_hash(id);
    let Some(held) = app.store.doc_get(&caller.site, KIND, &key)? else {
        return Err(not_accepted());
    };
    let invite: Invite = serde_json::from_str(&held)
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, format!("invitation: {e}")))?;
    // Every check is made, and compared in constant time, whichever one fails. A refusal takes
    // nothing: only someone who holds the whole link gets this far, and a wrong guess must not
    // be a way to spend the owner's invitation.
    let in_time = app.now().0 <= invite.expires_at;
    let secret = same_secret(&token_hash(token), &invite.token_hash);
    let site =
        caller.origin.as_deref().and_then(site_origin).as_deref() == Some(invite.origin.as_str());
    if !in_time {
        let _ = app.store.doc_remove(&caller.site, KIND, &key);
    }
    if !(in_time && secret && site) {
        return Err(not_accepted());
    }
    // One acceptance for one invitation, whatever the timing.
    if !app.store.doc_remove(&caller.site, KIND, &key)? {
        return Err(not_accepted());
    }
    let principal = PrincipalId::new(invite.principal.clone())
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, format!("invitation: {e}")))?;
    let Some(account) = app.store.account_of(&caller.site, &principal)? else {
        return Err(not_accepted());
    };
    // A deactivated person's account is kept but does not work: no password is chosen for it.
    if crate::service::is_disabled(app, &caller.site, &principal)? {
        return Err(not_accepted());
    }
    app.store.put_account(&Account {
        password_hash: hash_password(password)?,
        ..account.clone()
    })?;
    let [account_rule, source_rule, realm_rule] = &rules;
    app.store.throttle_settle(
        &caller.site,
        &account_rule.key,
        account_rule.max,
        ThrottleSettle::Clear,
    )?;
    for rule in [source_rule, realm_rule] {
        app.store
            .throttle_settle(&caller.site, &rule.key, rule.max, ThrottleSettle::Refund)?;
    }
    app.audit(
        "invite.accepted",
        &caller.correlation,
        json!({ "tenant": caller.site.as_str(), "principal": principal.as_str() }),
    );
    LOG.info(
        "invite.accepted",
        &caller.correlation,
        json!({ "tenant": caller.site.as_str() }),
    );
    Ok(json!({ "accepted": true, "login": account.login }))
}

/// `/internal/sign-in/invite/accept`, as the BFF calls it for the sign-in page.
pub async fn route(
    State(app): State<Shared>,
    headers: HeaderMap,
    Reply(body): Reply<Json>,
) -> Result<Reply<Json>, ApiError> {
    let caller = auth::caller(&headers)?;
    let correlation = caller.correlation.clone();
    app.blocking(move |app| accept(app, &caller, &body))
        .await?
        .map(Reply)
        .map_err(|error| error.with_correlation(&correlation))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_carries_the_link_and_says_how_long_it_lasts() {
        let text = first_sign_in_text("https://blog.example/login#i=a&t=b");
        assert!(text.contains("https://blog.example/login#i=a&t=b"));
        assert!(text.contains("within 7 days"), "{text}");
        assert!(text.contains("works once"), "{text}");
    }
}
