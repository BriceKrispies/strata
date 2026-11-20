//! Step-up: a sensitive operation needs a recent authentication.
//!
//! A session proves who someone was when it started. An operation that changes how the person
//! gets in, or ends the sessions of others, is refused unless the session has authenticated within
//! the step-up age (D-700: fifteen minutes), and unless it is still at the network context that
//! authenticated it (IDN-260). The refusal carries the kind `step_up_required`; the person gives
//! a fresh factor, the session's authentication moves to now, and the operation is asked again.
//!
//! - **Which factor.** A person who has a second factor gives the second factor again; a password
//!   alone is not a fresh authentication for them. A person with none gives the password. A passkey
//!   answers for either.
//! - **Counted.** A presentation counts against the account, source and realm limits before it is
//!   looked at, like a sign-in.
//! - **The network context** is the keyed hash of the source that the session record holds, never
//!   the address (D-704). A session that appears from another context is a new arrival: it
//!   authenticates again, and from then on it is at the new context.
//!
//! The sensitive operations are the ones that call [`require_fresh`]: adding and removing an
//! authenticator, making new recovery codes, adding and removing passkeys, ending every other
//! session, and the owner's reset of a fixture.
//!
//! Implements: IDN-019, IDN-260

use super::{passkey, rules, throttled, totp};
use crate::app::{ApiError, App};
use crate::auth::{self, is_password, Caller, Identity};
use serde_json::{json, Value as Json};
use strata_protocol::api::{ErrorCode, ErrorReply};
use strata_storage_api::{ThrottleSettle, ThrottleVerdict};

/// The kind the refusal carries, which the browser answers by asking for a factor.
pub const REQUIRED: &str = "step_up_required";

/// The refusal of a sensitive operation that has no fresh authentication behind it.
pub fn required() -> ApiError {
    ApiError(
        ErrorReply {
            code: ErrorCode::Forbidden,
            message: "Confirm that it is you to do this.".into(),
            kind: Some(REQUIRED.into()),
            failed: Vec::new(),
            correlation: String::new(),
        },
        None,
    )
}

fn sign_in_needed() -> ApiError {
    ApiError::new(ErrorCode::Unauthenticated, "Sign in to do this.")
}

/// Refuse the operation unless the session has authenticated recently and is still where it
/// authenticated. Call it first in every sensitive operation.
///
/// Implements: IDN-019, IDN-260
pub fn require_fresh(app: &App, caller: &Caller, identity: &Identity) -> Result<(), ApiError> {
    let Some(session) = &identity.session else {
        return Err(sign_in_needed());
    };
    let age = app.now().0 - session.authenticated_at.0;
    let moved = match (&session.network, auth::network_hash(app, caller)) {
        (Some(held), Some(here)) => *held != here,
        _ => false,
    };
    if age > app.cfg.factors.step_up_age_ms || moved {
        app.decision(
            "step-up",
            "required",
            &[if moved { "network" } else { "age" }],
            &caller.correlation,
        );
        return Err(required());
    }
    Ok(())
}

/// Give a fresh factor: the session's authentication moves to now, at the network context of this
/// request.
///
/// The request holds one of `password`, `code` (an authenticator's), or `challenge` and
/// `assertion` (a passkey's).
///
/// Implements: IDN-019, IDN-260
pub fn confirm(
    app: &App,
    caller: &Caller,
    identity: &Identity,
    request: &Json,
) -> Result<Json, ApiError> {
    let Some(session) = &identity.session else {
        return Err(sign_in_needed());
    };
    let refused = || ApiError::new(ErrorCode::Unauthenticated, "That was not accepted.");
    let text = |name: &str| request.get(name).and_then(Json::as_str).unwrap_or_default();
    let principal = &session.principal;
    let has_second = totp::confirmed(app, &caller.site, principal)?;

    let rules = rules(app, "step-up", principal.as_str(), &caller.source);
    if let ThrottleVerdict::Refused { .. } =
        app.store
            .throttle_reserve(&caller.site, &rules, app.now())?
    {
        return Err(throttled());
    }
    let accepted = if request.get("assertion").is_some() {
        passkey::assert(app, caller, text("challenge"), &request["assertion"])?
            .is_some_and(|who| who == *principal)
    } else if request.get("code").is_some() {
        has_second && totp::check(app, &caller.site, principal, text("code"), false)?
    } else if request.get("password").is_some() {
        // A password is the fresh factor of a person who has no second one.
        match app.store.account_of(&caller.site, principal)? {
            Some(account) if !has_second => is_password(text("password"), &account.password_hash),
            Some(account) => {
                // The same work as for a right one, so that the answer does not say which.
                is_password(text("password"), &account.password_hash);
                false
            }
            None => false,
        }
    } else {
        false
    };
    if !accepted {
        app.decision(
            "step-up",
            "refused",
            &["well-formed", "throttle", "credential"],
            &caller.correlation,
        );
        return Err(refused());
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
    let mut renewed = session.clone();
    renewed.authenticated_at = app.now();
    renewed.network = auth::network_hash(app, caller);
    app.store.session_authenticated(
        &renewed.id,
        renewed.authenticated_at,
        renewed.network.as_deref(),
    )?;
    app.decision(
        "step-up",
        "accepted",
        &["well-formed", "throttle", "credential"],
        &caller.correlation,
    );
    Ok(json!({ "authenticated_at": renewed.authenticated_at.0 }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_refusal_names_its_kind_so_that_a_page_can_ask_for_a_factor() {
        let error = required();
        assert_eq!(error.0.code, ErrorCode::Forbidden);
        assert_eq!(error.0.kind.as_deref(), Some("step_up_required"));
    }
}
