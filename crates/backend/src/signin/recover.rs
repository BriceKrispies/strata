//! Account recovery that cannot be used to take an account over.
//!
//! Recovery is the weakest way in, so it is slow, loud and keeps what it does not need to touch.
//!
//! - **Asking is uniform.** The answer to a request is the same whether or not the address has an
//!   account, and every request counts against the limits of the address, the source and the realm.
//! - **The contacts are told first (IDN-265).** The request sends a notice to every registered
//!   contact of the account at once: what was asked for, when it can complete, and a link that
//!   cancels it. Today an account's one contact is the address it signs in with.
//! - **It waits (IDN-265, D-700).** Completing is refused until the delay has passed (twenty-four
//!   hours), however right the token is, and the refusal consumes nothing.
//! - **It takes control back everywhere (IDN-020).** Completing sets the new password and ends every
//!   session of the account.
//! - **It keeps the second factor (IDN-266).** Nothing here touches the authenticator, the passkeys
//!   or the recovery codes, so the next sign-in asks for the second factor as before. Recovery of a
//!   password is not a way past it.
//!
//! Implements: IDN-020, IDN-265, IDN-266

use super::link::site_origin;
use super::{normalise_login, rules, throttled, totp};
use crate::app::{ApiError, App, LOG};
use crate::auth::{self, hash_password, random_hex, same_secret, token_hash, Caller};
use crate::mail::Mail;
use serde_json::{json, Value as Json};
use strata_kernel::{PrincipalId, Timestamp};
use strata_protocol::api::ErrorCode;
use strata_storage_api::{Account, RecoveryRequest, ThrottleSettle, ThrottleVerdict};

/// The longest address a request may carry (RFC 5321).
const ADDRESS_BYTES: usize = 254;
/// The shortest and longest password recovery accepts.
const PASSWORD_MIN: usize = 10;
const PASSWORD_MAX: usize = 256;
/// An identifier or a token: 32 random bytes in hex.
const HEX_LEN: usize = 64;

fn well_formed(value: &str) -> bool {
    value.len() == HEX_LEN && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn not_accepted() -> ApiError {
    ApiError::new(
        ErrorCode::Unauthenticated,
        "That recovery link was not accepted.",
    )
}

/// The contacts of an account: where it is told that recovery was asked for or done.
pub fn contacts(account: &Account) -> Vec<String> {
    if account.login.contains('@') {
        vec![account.login.clone()]
    } else {
        Vec::new()
    }
}

fn notify(app: &App, caller: &Caller, account: &Account, subject: &str, text: &str) {
    for contact in contacts(account) {
        let mail = Mail {
            to: contact,
            subject: subject.to_string(),
            text: text.to_string(),
        };
        if let Err(error) = app.cfg.mail.send(&mail) {
            LOG.error(
                "recovery.not_sent",
                &caller.correlation,
                json!({ "tenant": caller.site.as_str(), "error": error.0 }),
            );
        }
    }
}

/// Ask for a recovery. Nothing changes yet: the contacts are told, and the request waits.
///
/// Implements: IDN-265
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
    let rules = rules(
        app,
        "recovery-request",
        address.as_deref().unwrap_or(""),
        &caller.source,
    );
    if let ThrottleVerdict::Refused { .. } =
        app.store
            .throttle_reserve(&caller.site, &rules, app.now())?
    {
        return Err(throttled());
    }

    // An address in a domain that a provider decides for has no password to recover.
    let account = match &address {
        Some(address) if !app.cfg.federation.domain_bound(&caller.site, address) => {
            app.store.account(&caller.site, address)?
        }
        _ => None,
    };
    // A person their provider deactivated has an account that is kept but does not work: nothing
    // is recovered for it, and the answer is the one for an address with no account (FED-015).
    let account = match account {
        Some(account) if crate::service::is_disabled(app, &caller.site, &account.principal)? => {
            None
        }
        other => other,
    };
    // An account with no contact to tell is not recoverable by this route: a recovery that no one
    // is told of is the one a thief would choose.
    if let Some(account) = account.filter(|account| !contacts(account).is_empty()) {
        let id = totp::token()?;
        let token = random_hex(HEX_LEN / 2)?;
        let cancel = random_hex(HEX_LEN / 2)?;
        let now = app.now();
        let policy = &app.cfg.factors;
        let completes_at = Timestamp(now.0 + policy.recovery_delay_ms);
        app.store.put_recovery(&RecoveryRequest {
            tenant: caller.site.clone(),
            id_hash: token_hash(&id),
            principal: account.principal.clone(),
            login: account.login.clone(),
            token_hash: token_hash(&token),
            cancel_hash: token_hash(&cancel),
            requested_at: now,
            completes_at,
            expires_at: Timestamp(completes_at.0 + policy.recovery_window_ms),
        })?;
        let hours = (policy.recovery_delay_ms / 3_600_000).max(0);
        let text = format!(
            "Someone asked to recover your account.\n\n\
             Nothing has changed. The recovery can complete after {hours} hours, and only with this \
             message's link:\n\n{origin}/login#r={id}&t={token}\n\n\
             If this was not you, cancel it now:\n\n{origin}/login#r={id}&x={cancel}\n\n\
             Whatever happens, your second factor stays in force."
        );
        notify(
            app,
            caller,
            &account,
            "Account recovery was requested",
            &text,
        );
        app.audit(
            "recovery.requested",
            &caller.correlation,
            json!({
                "tenant": caller.site.as_str(),
                "principal": account.principal.as_str(),
                "contacts": contacts(&account).len(),
            }),
        );
    } else {
        // The same durable write, for a placeholder principal that no token opens, so that what
        // the request costs does not say whether the address has a recoverable account.
        let id = totp::token()?;
        let now = app.now();
        let policy = &app.cfg.factors;
        let completes_at = Timestamp(now.0 + policy.recovery_delay_ms);
        app.store.put_recovery(&RecoveryRequest {
            tenant: caller.site.clone(),
            id_hash: token_hash(&id),
            principal: PrincipalId::new("user:recovery-decoy".to_string())
                .map_err(|e| ApiError::hidden(ErrorCode::Internal, format!("decoy: {e}")))?,
            login: String::new(),
            token_hash: token_hash(&random_hex(HEX_LEN / 2)?),
            cancel_hash: token_hash(&random_hex(HEX_LEN / 2)?),
            requested_at: now,
            completes_at,
            expires_at: Timestamp(completes_at.0 + policy.recovery_window_ms),
        })?;
    }
    Ok(json!({ "requested": true }))
}

/// Complete a recovery that has waited out its delay: set the new password and end every session.
///
/// The request holds the `challenge` and `token` of the message's link and the new `password`.
///
/// Implements: IDN-020, IDN-265, IDN-266
pub fn complete(app: &App, caller: &Caller, body: &Json) -> Result<Json, ApiError> {
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
    let rules = rules(app, "recovery-complete", id, &caller.source);
    if let ThrottleVerdict::Refused { .. } =
        app.store
            .throttle_reserve(&caller.site, &rules, app.now())?
    {
        return Err(throttled());
    }
    let id_hash = token_hash(id);
    let Some(held) = app.store.recovery(&caller.site, &id_hash)? else {
        return Err(not_accepted());
    };
    if !same_secret(&token_hash(token), &held.token_hash) {
        return Err(not_accepted());
    }
    let now = app.now();
    if now < held.completes_at {
        // Right, and too early: nothing is consumed, and the contacts still have their time.
        // The token was checked right, so this is no guess: the attempt is given back, or the
        // holder of the link who retries while waiting would lock the recovery they asked for.
        for rule in &rules {
            app.store
                .throttle_settle(&caller.site, &rule.key, rule.max, ThrottleSettle::Refund)?;
        }
        app.decision(
            &caller.site,
            "recovery",
            "too-early",
            &["well-formed", "throttle", "credential"],
            &caller.correlation,
        );
        return Err(ApiError::new(
            ErrorCode::Conflict,
            "This recovery is still waiting out its delay. Try again later.",
        ));
    }
    // One completion for one request, whatever the timing.
    let Some(taken) = app.store.take_recovery(&caller.site, &id_hash)? else {
        return Err(not_accepted());
    };
    if now > taken.expires_at {
        return Err(not_accepted());
    }
    let Some(account) = app.store.account(&caller.site, &taken.login)? else {
        return Err(not_accepted());
    };
    // A person deactivated since the request was made gets no new password (FED-015).
    if crate::service::is_disabled(app, &caller.site, &account.principal)? {
        return Err(not_accepted());
    }
    app.store.put_account(&Account {
        password_hash: hash_password(password)?,
        ..account.clone()
    })?;
    let ended = auth::revoke_all_of(app, &caller.site, &taken.principal, &caller.correlation)?;
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
    notify(
        app,
        caller,
        &account,
        "Your password was changed by account recovery",
        "The recovery that was asked for has completed: your password was changed and every \
         session of your account ended. Your second factor is still in force.",
    );
    app.audit(
        "recovery.completed",
        &caller.correlation,
        json!({
            "tenant": caller.site.as_str(),
            "principal": taken.principal.as_str(),
            "sessions_ended": ended,
        }),
    );
    // The factors are untouched, so the next sign-in asks for them.
    Ok(json!({
        "recovered": true,
        "second_factor": totp::confirmed(app, &caller.site, &taken.principal)?,
    }))
}

/// Cancel a recovery with the link of the notice. Anyone who holds the notice may.
///
/// Implements: IDN-265
pub fn cancel(app: &App, caller: &Caller, body: &Json) -> Result<Json, ApiError> {
    let text = |name: &str| body.get(name).and_then(Json::as_str).unwrap_or_default();
    let (id, cancel) = (text("challenge"), text("cancel"));
    if !well_formed(id) || !well_formed(cancel) {
        return Err(not_accepted());
    }
    // Every ask counts, as at the request; none is given back.
    let rules = rules(app, "recovery-cancel", id, &caller.source);
    if let ThrottleVerdict::Refused { .. } =
        app.store
            .throttle_reserve(&caller.site, &rules, app.now())?
    {
        return Err(throttled());
    }
    let cancelled =
        app.store
            .cancel_recovery(&caller.site, &token_hash(id), &token_hash(cancel))?;
    if cancelled {
        app.audit(
            "recovery.cancelled",
            &caller.correlation,
            json!({ "tenant": caller.site.as_str() }),
        );
    }
    Ok(json!({ "cancelled": cancelled }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_kernel::{PrincipalId, TenantId};

    fn account(login: &str) -> Account {
        Account {
            tenant: TenantId::new("alpha").unwrap(),
            login: login.into(),
            principal: PrincipalId::new("ines").unwrap(),
            display_name: "Ines".into(),
            password_hash: String::new(),
            verified_email: None,
        }
    }

    #[test]
    fn an_account_is_told_at_the_address_it_signs_in_with_and_nowhere_else() {
        assert_eq!(
            contacts(&account("ines@example.test")),
            vec!["ines@example.test"]
        );
        assert!(contacts(&account("ines")).is_empty());
    }
}
