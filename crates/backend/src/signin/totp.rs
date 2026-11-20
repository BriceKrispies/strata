//! A time-based one-time password as a second factor (RFC 6238).
//!
//! The codes are the `totp-rs` crate's: HMAC-SHA1, six digits, thirty-second steps, the settings
//! every authenticator application supports. This module decides what the crate does not: where
//! the secret is kept, when a code counts and how often.
//!
//! - **The secret is sealed, not hashed.** Checking a code needs the secret itself, so a hash would
//!   serve no purpose. It is encrypted with AES-256-GCM under a key that is derived from a platform
//!   secret and kept out of the store, with the tenant and the person as associated data, so a copy
//!   of the database opens nothing and a sealed secret moved to another person's row does not open.
//!   The nonce is fresh for every sealing.
//! - **A code is accepted once.** A code is a time step's value; the step that was accepted is
//!   remembered, and a code of that step or an earlier one is refused (replay), in one atomic step
//!   of the store so that two presentations arriving together cannot both win.
//! - **A code is counted.** Whoever presents one counts against the limits of the account, the
//!   source and the realm before the code is looked at, like a password (IDN-008, IDN-253).
//! - **Enrolment is two steps.** The secret is shown once and is no factor until the person has
//!   given a code that proves they hold it.
//!
//! Implements: IDN-018, IDN-253

use super::stepup;
use super::{rules, throttled};
use crate::app::{ApiError, App};
use crate::auth::{random_hex, same_secret, Caller, Identity};
use serde_json::{json, Value as Json};
use strata_kernel::{PrincipalId, TenantId};
use strata_protocol::api::ErrorCode;
use strata_storage_api::{ThrottleSettle, ThrottleVerdict, TotpRecord};
use totp_rs::{Algorithm, TOTP};

/// The seconds in one time step of a code.
const STEP_SECONDS: u64 = 30;
const DIGITS: usize = 6;
/// A code of the step before or after the current one is accepted, for clocks that drift.
const SKEW_STEPS: i64 = 1;
/// The secret's length: 160 bits, as RFC 4226 recommends.
const SECRET_BYTES: usize = 20;

fn sign_in_needed() -> ApiError {
    ApiError::new(ErrorCode::Unauthenticated, "Sign in to do this.")
}

fn not_accepted() -> ApiError {
    ApiError::new(ErrorCode::Unauthenticated, "The code was not accepted.")
}

/// What the cipher is bound to: the tenant and the person.
fn bound_to(tenant: &TenantId, principal: &PrincipalId) -> Vec<u8> {
    format!("totp-secret\0{}\0{}", tenant.as_str(), principal.as_str()).into_bytes()
}

/// Seal a secret under the current factor key, bound to the person.
pub(super) fn seal(
    app: &App,
    tenant: &TenantId,
    principal: &PrincipalId,
    secret: &[u8],
) -> Result<String, ApiError> {
    crate::sealing::seal(app, &bound_to(tenant, principal), secret)
}

/// Open a sealed secret, or `None` when it was not sealed for this person under a key we hold.
pub(super) fn open(
    app: &App,
    tenant: &TenantId,
    principal: &PrincipalId,
    sealed: &str,
) -> Option<Vec<u8>> {
    crate::sealing::open(app, &bound_to(tenant, principal), sealed)
}

fn engine(secret: Vec<u8>) -> Option<TOTP> {
    TOTP::new(
        Algorithm::SHA1,
        DIGITS,
        SKEW_STEPS as u8,
        STEP_SECONDS,
        secret,
    )
    .ok()
}

/// The time step a code belongs to within the skew of `now_ms`, or `None`. Every step in the
/// window is compared, in constant time, whichever one matches.
pub fn matching_step(totp: &TOTP, code: &str, now_ms: i64) -> Option<i64> {
    let code = code.trim();
    if code.len() != DIGITS || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let current = now_ms.div_euclid(1000) / STEP_SECONDS as i64;
    let mut found = None;
    for offset in -SKEW_STEPS..=SKEW_STEPS {
        let step = current + offset;
        if step < 0 {
            continue;
        }
        let expected = totp.generate(step as u64 * STEP_SECONDS);
        if same_secret(&expected, code) {
            found = Some(step);
        }
    }
    found
}

/// The address an authenticator application reads, for the QR code and for the people who type
/// the secret instead. The issuer is the site's name, so the application labels the entry with it.
fn otpauth_uri(issuer: &str, account: &str, secret_base32: &str) -> String {
    use super::link::encode;
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECONDS}",
        encode(issuer),
        encode(account),
        secret_base32,
        encode(issuer),
    )
}

/// Whether the person has a confirmed TOTP factor.
pub fn confirmed(app: &App, tenant: &TenantId, principal: &PrincipalId) -> Result<bool, ApiError> {
    Ok(app
        .store
        .totp(tenant, principal)?
        .is_some_and(|record| record.confirmed))
}

/// Make a secret for a person and keep it, unconfirmed. Returns what the person needs to add it
/// to an authenticator: the secret in base32 and the address of the QR code.
pub(super) fn begin(
    app: &App,
    tenant: &TenantId,
    principal: &PrincipalId,
    issuer: &str,
    account: &str,
) -> Result<Json, ApiError> {
    let mut secret = vec![0u8; SECRET_BYTES];
    getrandom::getrandom(&mut secret)
        .map_err(|e| ApiError::new(ErrorCode::Unavailable, format!("no randomness: {e}")))?;
    let Some(totp) = engine(secret.clone()) else {
        return Err(ApiError::hidden(
            ErrorCode::Internal,
            "a generated secret was refused by the one-time password library",
        ));
    };
    let record = TotpRecord {
        tenant: tenant.clone(),
        principal: principal.clone(),
        secret: seal(app, tenant, principal, &secret)?,
        confirmed: false,
        last_step: -1,
        created_at: app.now(),
    };
    if !app.store.put_totp(&record)? {
        return Err(ApiError::new(
            ErrorCode::Conflict,
            "An authenticator is already set up. Remove it before adding another.",
        ));
    }
    let base32 = totp.get_secret_base32();
    Ok(json!({
        "secret": base32,
        "uri": otpauth_uri(issuer, account, &base32),
        "digits": DIGITS,
        "period": STEP_SECONDS,
    }))
}

/// The tenant's name and the person's login, for the label an authenticator shows.
fn label(
    app: &App,
    tenant: &TenantId,
    principal: &PrincipalId,
) -> Result<(String, String), ApiError> {
    let name = app
        .store
        .load(tenant, &[])?
        .tenant
        .map_or_else(|| tenant.as_str().to_string(), |t| t.name.clone());
    let login = app
        .store
        .account_of(tenant, principal)?
        .map_or_else(|| principal.as_str().to_string(), |a| a.login);
    Ok((name, login))
}

/// Check a code against the person's factor and accept it once.
///
/// With `confirming`, the factor is one that was begun and not yet confirmed, and a right code
/// confirms it; otherwise the factor must be confirmed, and a right code is spent.
///
/// Implements: IDN-018
pub(super) fn check(
    app: &App,
    tenant: &TenantId,
    principal: &PrincipalId,
    code: &str,
    confirming: bool,
) -> Result<bool, ApiError> {
    let Some(record) = app.store.totp(tenant, principal)? else {
        return Ok(false);
    };
    if record.confirmed == confirming {
        return Ok(false);
    }
    let Some(secret) = open(app, tenant, principal, &record.secret) else {
        return Ok(false);
    };
    let Some(totp) = engine(secret) else {
        return Ok(false);
    };
    let Some(step) = matching_step(&totp, code, app.now().0) else {
        return Ok(false);
    };
    Ok(if confirming {
        app.store.confirm_totp(tenant, principal, step)?
    } else {
        app.store.use_totp_step(tenant, principal, step)?
    })
}

/// Check a code for a person with the limits that apply to presentations of a code, counting it
/// before it is looked at and giving the slot back when it is right. Returns `Ok(true)` for a code
/// that was accepted, and the same refusal for every way it can fail.
///
/// Implements: IDN-008, IDN-253
pub(super) fn present(
    app: &App,
    caller: &Caller,
    kind: &str,
    principal: &PrincipalId,
    code: &str,
    confirming: bool,
) -> Result<bool, ApiError> {
    let rules = rules(app, kind, principal.as_str(), &caller.source);
    if let ThrottleVerdict::Refused { .. } =
        app.store
            .throttle_reserve(&caller.site, &rules, app.now())?
    {
        return Err(throttled());
    }
    if !check(app, &caller.site, principal, code, confirming)? {
        return Ok(false);
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
    Ok(true)
}

/// Begin adding an authenticator to the signed-in person's account. A sensitive operation: it
/// changes the factors, so it needs a fresh authentication (IDN-019).
///
/// Implements: IDN-018, IDN-019
pub fn enrol_start(app: &App, caller: &Caller, identity: &Identity) -> Result<Json, ApiError> {
    let Some(session) = &identity.session else {
        return Err(sign_in_needed());
    };
    stepup::require_fresh(app, caller, identity)?;
    let (issuer, account) = label(app, &caller.site, &session.principal)?;
    let started = begin(app, &caller.site, &session.principal, &issuer, &account)?;
    app.audit(
        "totp.enrol_started",
        &caller.correlation,
        json!({ "tenant": caller.site.as_str(), "principal": session.principal.as_str() }),
    );
    Ok(started)
}

/// Finish adding an authenticator: the code proves the person holds the secret.
///
/// Implements: IDN-018, IDN-019
pub fn enrol_finish(
    app: &App,
    caller: &Caller,
    identity: &Identity,
    request: &Json,
) -> Result<Json, ApiError> {
    let Some(session) = &identity.session else {
        return Err(sign_in_needed());
    };
    stepup::require_fresh(app, caller, identity)?;
    let code = request
        .get("code")
        .and_then(Json::as_str)
        .unwrap_or_default();
    if !present(app, caller, "totp-enrol", &session.principal, code, true)? {
        return Err(not_accepted());
    }
    app.audit(
        "totp.enrolled",
        &caller.correlation,
        json!({ "tenant": caller.site.as_str(), "principal": session.principal.as_str() }),
    );
    Ok(json!({ "enabled": true }))
}

/// Remove the signed-in person's authenticator, and the recovery codes that went with it.
///
/// Implements: IDN-019
pub fn remove(app: &App, caller: &Caller, identity: &Identity) -> Result<Json, ApiError> {
    let Some(session) = &identity.session else {
        return Err(sign_in_needed());
    };
    stepup::require_fresh(app, caller, identity)?;
    let removed = app.store.remove_totp(&caller.site, &session.principal)?;
    app.store
        .replace_recovery_codes(&caller.site, &session.principal, &[], app.now())?;
    if removed {
        app.audit(
            "totp.removed",
            &caller.correlation,
            json!({ "tenant": caller.site.as_str(), "principal": session.principal.as_str() }),
        );
    }
    Ok(json!({ "enabled": false }))
}

/// A random identifier for a pending sign-in or a recovery, as the browser holds it.
pub(super) fn token() -> Result<String, ApiError> {
    random_hex(32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238, appendix B: the SHA-1 test vectors, at the eight digits the RFC prints.
    #[test]
    fn the_codes_are_those_of_the_rfc() {
        let secret = b"12345678901234567890".to_vec();
        let totp = TOTP::new(Algorithm::SHA1, 8, 1, 30, secret).unwrap();
        for (time, code) in [
            (59u64, "94287082"),
            (1_111_111_109, "07081804"),
            (1_234_567_890, "89005924"),
            (20_000_000_000, "65353130"),
        ] {
            assert_eq!(totp.generate(time), code, "at {time}");
        }
    }

    #[test]
    fn a_code_is_found_in_its_step_and_the_one_either_side_and_nowhere_else() {
        let totp = engine(b"12345678901234567890".to_vec()).unwrap();
        let now_ms = 1_700_000_015_000;
        let step = now_ms / 1000 / 30;
        for (offset, expected) in [
            (-2, None),
            (-1, Some(step - 1)),
            (0, Some(step)),
            (1, Some(step + 1)),
            (2, None),
        ] {
            let code = totp.generate(((step + offset) * 30) as u64);
            assert_eq!(
                matching_step(&totp, &code, now_ms),
                expected,
                "offset {offset}"
            );
        }
        for junk in ["", "12345", "1234567", "abcdef", "12 456"] {
            assert_eq!(matching_step(&totp, junk, now_ms), None, "{junk:?}");
        }
    }

    #[test]
    fn the_address_names_the_site_the_person_and_the_secret() {
        let uri = otpauth_uri("Field notes", "ines@example.test", "ABC234");
        assert_eq!(
            uri,
            "otpauth://totp/Field%20notes:ines%40example.test?secret=ABC234&issuer=Field%20notes&algorithm=SHA1&digits=6&period=30"
        );
    }
}
