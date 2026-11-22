//! The second factor: a stage of the shared sign-in, and the factors a person manages.
//!
//! **The stage.** After a method has accepted its credential, [`stage`] decides once, for every
//! method, whether the first factor is enough. It is not when the realm asks for a second factor
//! (IDN-018) or when the account has one enrolled, unless the method proved two on its own
//! (IDN-267; the machines `second-factor-flow` and `facg-flow`). The sign-in then yields no session:
//! it yields a waiting sign-in that only a verified second factor turns into one. A method does
//! not decide this, so a new method cannot forget to.
//!
//! **The waiting sign-in** is held server-side under the hash of an identifier the browser holds,
//! lapses, and answers once. It is spent after five failed presentations (IDN-253), and a realm
//! sends at most five prompts to one account in an hour (IDN-268). Every presentation also counts
//! against the account, source and realm limits before it is looked at.
//!
//! **The ways to give a second factor** are an authenticator's code, a recovery code and a
//! passkey. A realm that asks for a second factor of a person who has none gets the person to add
//! an authenticator in the same step: the waiting sign-in carries the secret, and the first right
//! code both confirms it and finishes the sign-in.
//!
//! Implements: IDN-018, IDN-253, IDN-267, IDN-268

use super::{passkey, recovery_codes, rules, throttled, totp, SignInOutcome};
use crate::app::{ApiError, App};
use crate::auth::{self, random_hex, Caller, Identity};
use serde_json::{json, Value as Json};
use strata_kernel::{canon, PrincipalId, TenantId};
use strata_protocol::api::{ErrorCode, ErrorReply};
use strata_storage_api::{Account, PendingSignIn, ThrottleRule, ThrottleSettle, ThrottleVerdict};

/// A waiting sign-in's identifier: 32 random bytes in hex.
const ID_LEN: usize = 64;

fn well_formed_id(id: &str) -> bool {
    id.len() == ID_LEN && id.bytes().all(|b| b.is_ascii_hexdigit())
}

fn not_accepted() -> ApiError {
    ApiError::new(ErrorCode::Unauthenticated, "That was not accepted.")
}

fn sign_in_needed() -> ApiError {
    ApiError::new(ErrorCode::Unauthenticated, "Sign in to do this.")
}

/// The refusal of a first enrolment that has only the password behind it.
fn enrolment_needs_proof() -> ApiError {
    ApiError(
        ErrorReply {
            code: ErrorCode::Forbidden,
            message: "This site needs a second factor. Sign in with an emailed link to set one up."
                .into(),
            kind: Some("second_factor_enrolment_needs_proof".into()),
            failed: Vec::new(),
            correlation: String::new(),
        },
        None,
    )
}

/// The ways a person can give a second factor now.
fn methods(
    app: &App,
    tenant: &TenantId,
    principal: &PrincipalId,
) -> Result<Vec<&'static str>, ApiError> {
    let mut found = Vec::new();
    if totp::confirmed(app, tenant, principal)? {
        found.push("totp");
    }
    if passkey::held(app, tenant, principal)? {
        found.push("passkey");
    }
    if app.store.recovery_codes_left(tenant, principal)? > 0 {
        found.push("recovery");
    }
    Ok(found)
}

/// Whether the first factor of this sign-in is not enough.
///
/// Implements: IDN-018, IDN-267
pub fn required(
    app: &App,
    tenant: &TenantId,
    principal: &PrincipalId,
    proved: u8,
) -> Result<bool, ApiError> {
    if proved >= 2 {
        return Ok(false);
    }
    Ok(app.cfg.factors.realm_required.contains(tenant) || totp::confirmed(app, tenant, principal)?)
}

/// The shared stage after a method accepted its credential. `None` when the first factor is
/// enough and the session may start; otherwise the waiting sign-in's reply, and no session.
///
/// Implements: IDN-018, IDN-267, IDN-268
pub fn stage(
    app: &App,
    caller: &Caller,
    method: &str,
    proved: u8,
    account: &Account,
) -> Result<Option<Json>, ApiError> {
    let tenant = &caller.site;
    let principal = &account.principal;
    if !required(app, tenant, principal, proved)? {
        return Ok(None);
    }
    // Prompting a person is limited, whoever asks and from wherever: a flood of prompts wears the
    // person down until they approve one. Not given back.
    let limits = &app.cfg.factors;
    let rule = ThrottleRule {
        key: format!(
            "account:second-factor-prompt:{}",
            canon::hash_bytes(principal.as_str().as_bytes())
        ),
        max: limits.prompts_max,
        window_ms: limits.prompts_window_ms,
        lock_ms: limits.prompts_window_ms,
        lock_resets: false,
    };
    if let ThrottleVerdict::Refused { .. } =
        app.store
            .throttle_reserve(tenant, std::slice::from_ref(&rule), app.now())?
    {
        app.decision(
            &caller.site,
            method,
            "prompt-limit",
            &["well-formed", "throttle", "credential", "prompt-limit"],
            &caller.correlation,
        );
        return Err(throttled());
    }
    let available = methods(app, tenant, principal)?;
    let enrol = if available.is_empty() {
        // The first factor of a person is enrolled only after a proof beyond the password: an
        // emailed link or a provider has shown control of the address or the identity, a password
        // alone has not. Otherwise whoever holds a stolen password owns the factor.
        if !matches!(method, "link" | "oidc" | "saml" | "passkey") {
            app.decision(
                &caller.site,
                method,
                "second-factor-enrolment-refused",
                &["well-formed", "throttle", "credential"],
                &caller.correlation,
            );
            return Err(enrolment_needs_proof());
        }
        // A realm that asks for a second factor of a person who has none: the secret goes with
        // the waiting sign-in, and the first right code confirms it.
        let issuer = app
            .store
            .load(tenant, &[])?
            .tenant
            .map_or_else(|| tenant.as_str().to_string(), |t| t.name.clone());
        Some(totp::begin(
            app,
            tenant,
            principal,
            &issuer,
            &account.login,
        )?)
    } else {
        None
    };
    let id = random_hex(ID_LEN / 2)?;
    app.store.put_pending(&PendingSignIn {
        tenant: tenant.clone(),
        id_hash: canon::hash_bytes(id.as_bytes()),
        principal: principal.clone(),
        method: method.to_string(),
        factors: proved,
        attempts: 0,
        expires_at: strata_kernel::Timestamp(app.now().0 + limits.pending_ttl_ms),
    })?;
    app.decision(
        &caller.site,
        method,
        "second-factor-asked",
        &["well-formed", "throttle", "credential"],
        &caller.correlation,
    );
    let offered = if enrol.is_some() {
        vec!["totp"]
    } else {
        available
    };
    Ok(Some(json!({
        "second_factor": {
            "challenge": id,
            "methods": offered,
            "enrol": enrol,
            "expires_in": limits.pending_ttl_ms / 1000,
        }
    })))
}

/// Give the second factor of a waiting sign-in. A right one yields the session, once.
///
/// The request holds the `challenge` the first factor was answered with, a `factor` (`totp`,
/// `recovery` or `passkey`) and what that factor needs: a `code`, or a passkey's `passkey_challenge`
/// and `assertion`.
///
/// Implements: IDN-018, IDN-253, IDN-268
pub fn verify(app: &App, caller: &Caller, request: &Json) -> Result<SignInOutcome, ApiError> {
    let text = |name: &str| request.get(name).and_then(Json::as_str).unwrap_or_default();
    let challenge = text("challenge");
    if !well_formed_id(challenge) {
        return Err(not_accepted());
    }
    let id_hash = canon::hash_bytes(challenge.as_bytes());
    let limits = &app.cfg.factors;
    // The presentation is counted against the waiting sign-in before it is looked at, so that
    // presentations arriving together are counted together.
    let Some(pending) =
        app.store
            .attempt_pending(&caller.site, &id_hash, app.now(), limits.code_attempts_max)?
    else {
        return Err(not_accepted());
    };
    let principal = &pending.principal;
    let rules = rules(app, "second-factor", principal.as_str(), &caller.source);
    if let ThrottleVerdict::Refused { .. } =
        app.store
            .throttle_reserve(&caller.site, &rules, app.now())?
    {
        return Err(throttled());
    }

    let accepted = match text("factor") {
        "totp" => {
            // A first-time code confirms the authenticator the sign-in itself began.
            let confirming = app
                .store
                .totp(&caller.site, principal)?
                .is_some_and(|record| !record.confirmed);
            totp::check(app, &caller.site, principal, text("code"), confirming)?
        }
        "recovery" => app.store.spend_recovery_code(
            &caller.site,
            principal,
            &recovery_codes::hash(&caller.site, principal, text("code")),
        )?,
        "passkey" => {
            request.get("assertion").is_some()
                && passkey::assert(
                    app,
                    caller,
                    text("passkey_challenge"),
                    &request["assertion"],
                )?
                .is_some_and(|who| who == *principal)
        }
        _ => false,
    };
    if !accepted {
        // The fifth failed presentation spends the sign-in: the person starts again from the
        // first factor.
        if pending.attempts >= limits.code_attempts_max {
            app.store.take_pending(&caller.site, &id_hash, app.now())?;
        }
        app.decision(
            &caller.site,
            &pending.method,
            "second-factor-refused",
            &["well-formed", "throttle", "credential", "second-factor"],
            &caller.correlation,
        );
        return Err(not_accepted());
    }

    // One session for one waiting sign-in, whatever the timing.
    if app
        .store
        .take_pending(&caller.site, &id_hash, app.now())?
        .is_none()
    {
        return Err(not_accepted());
    }
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
    let Some(account) = app.store.account_of(&caller.site, principal)? else {
        return Err(not_accepted());
    };
    let Some(tenant) = app.store.load(&caller.site, &[])?.tenant else {
        return Err(ApiError::new(ErrorCode::NotFound, "no such site"));
    };
    // A person their provider deactivated while the sign-in waited gets no session (FED-015).
    if crate::service::is_disabled(app, &caller.site, principal)? {
        app.decision(
            &caller.site,
            &pending.method,
            "refused",
            &[
                "well-formed",
                "throttle",
                "credential",
                "second-factor",
                "active",
            ],
            &caller.correlation,
        );
        return Err(not_accepted());
    }
    app.decision(
        &caller.site,
        &pending.method,
        "accepted",
        &["well-formed", "throttle", "credential", "second-factor"],
        &caller.correlation,
    );
    auth::start_session(app, caller, &tenant, account, &pending.method)
        .map(|reply| SignInOutcome::Session(Box::new(reply)))
}

/// What the signed-in person's factors are: for the desk's section.
pub fn summary(app: &App, caller: &Caller, identity: &Identity) -> Result<Json, ApiError> {
    let Some(session) = &identity.session else {
        return Err(sign_in_needed());
    };
    let principal = &session.principal;
    Ok(json!({
        "totp": totp::confirmed(app, &caller.site, principal)?,
        "passkeys": app.store.passkeys(&caller.site, principal)?.len(),
        "recovery_codes": app.store.recovery_codes_left(&caller.site, principal)?,
        "realm_requires": app.cfg.factors.realm_required.contains(&caller.site),
        "step_up_age": app.cfg.factors.step_up_age_ms / 1000,
        "authenticated_at": session.authenticated_at.0,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_waiting_sign_in_is_found_by_a_well_formed_identifier_only() {
        assert!(well_formed_id(&"ab".repeat(32)));
        for bad in ["", "ab", &"zz".repeat(32), &"ab".repeat(33)] {
            assert!(!well_formed_id(bad), "{bad:?}");
        }
    }
}
