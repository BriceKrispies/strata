//! Recovery codes: the way back in when the authenticator is lost.
//!
//! A set of codes is made once, shown once, and each code works once. Regenerating makes a new set
//! and ends the old one.
//!
//! - **Stored as hashes.** A code has 80 bits of randomness, which no one guesses, so a plain
//!   SHA-256 of it (with the tenant and the person mixed in, so equal codes of two people differ)
//!   is enough at rest: a slow password hash would only cost the person's own sign-in time. A copy of
//!   the database holds no code that can be presented.
//! - **Spent in one step.** Presenting a code removes it in the same store step that finds it, so
//!   two presentations arriving together cannot both win.
//! - **Counted.** A presentation counts against the account, source and realm limits before the
//!   code is looked at, like any other secret someone types.
//!
//! Implements: IDN-018

use super::stepup;
use super::{rules, throttled, totp};
use crate::app::{ApiError, App};
use crate::auth::{Caller, Identity};
use serde_json::{json, Value as Json};
use strata_kernel::{canon, PrincipalId, TenantId};
use strata_protocol::api::ErrorCode;
use strata_storage_api::{ThrottleSettle, ThrottleVerdict};

/// How many codes a set holds.
pub const SET_SIZE: usize = 10;
/// Thirty-two symbols without the ones that are read as another (0 and O, 1 and I), so that a
/// byte's low five bits choose one with no bias.
const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
/// Symbols in a code: sixteen of five bits is eighty.
const SYMBOLS: usize = 16;

fn sign_in_needed() -> ApiError {
    ApiError::new(ErrorCode::Unauthenticated, "Sign in to do this.")
}

/// A code as typed or printed in the one form that is hashed: upper case, with the dashes and
/// spaces that make it readable taken out.
pub fn normalise(code: &str) -> String {
    code.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .flat_map(char::to_uppercase)
        .collect()
}

/// The hash a code is kept as.
pub fn hash(tenant: &TenantId, principal: &PrincipalId, code: &str) -> String {
    canon::hash_bytes(
        format!(
            "recovery-code\0{}\0{}\0{}",
            tenant.as_str(),
            principal.as_str(),
            normalise(code)
        )
        .as_bytes(),
    )
}

/// A new code, written in four groups of four.
fn new_code() -> Result<String, ApiError> {
    let mut bytes = [0u8; SYMBOLS];
    getrandom::getrandom(&mut bytes)
        .map_err(|e| ApiError::new(ErrorCode::Unavailable, format!("no randomness: {e}")))?;
    let symbols: Vec<char> = bytes
        .iter()
        .map(|byte| ALPHABET[(byte & 31) as usize] as char)
        .collect();
    Ok(symbols
        .chunks(4)
        .map(|group| group.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-"))
}

/// Make a set and keep its hashes in place of whatever set there was. Returns the codes, which
/// are never available again.
pub(super) fn generate(
    app: &App,
    tenant: &TenantId,
    principal: &PrincipalId,
) -> Result<Vec<String>, ApiError> {
    let mut codes = Vec::with_capacity(SET_SIZE);
    while codes.len() < SET_SIZE {
        let code = new_code()?;
        if !codes.contains(&code) {
            codes.push(code);
        }
    }
    let hashes: Vec<String> = codes
        .iter()
        .map(|code| hash(tenant, principal, code))
        .collect();
    app.store
        .replace_recovery_codes(tenant, principal, &hashes, app.now())?;
    Ok(codes)
}

/// Spend a code with the limits that apply to presentations of a secret. Returns whether it was
/// one of the person's unspent codes.
///
/// Implements: IDN-008
pub(super) fn present(
    app: &App,
    caller: &Caller,
    principal: &PrincipalId,
    code: &str,
) -> Result<bool, ApiError> {
    let rules = rules(app, "recovery-code", principal.as_str(), &caller.source);
    if let ThrottleVerdict::Refused { .. } =
        app.store
            .throttle_reserve(&caller.site, &rules, app.now())?
    {
        return Err(throttled());
    }
    if !app.store.spend_recovery_code(
        &caller.site,
        principal,
        &hash(&caller.site, principal, code),
    )? {
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

/// Make a new set of recovery codes for the signed-in person. A sensitive operation, because it
/// changes how the person can get in (IDN-019). There has to be an authenticator for the codes
/// to stand in for.
///
/// Implements: IDN-018, IDN-019
pub fn regenerate(app: &App, caller: &Caller, identity: &Identity) -> Result<Json, ApiError> {
    let Some(session) = &identity.session else {
        return Err(sign_in_needed());
    };
    stepup::require_fresh(app, caller, identity)?;
    if !totp::confirmed(app, &caller.site, &session.principal)? {
        return Err(ApiError::new(
            ErrorCode::Conflict,
            "Set up an authenticator first. Recovery codes stand in for it.",
        ));
    }
    let codes = generate(app, &caller.site, &session.principal)?;
    app.audit(
        "recovery_codes.generated",
        &caller.correlation,
        json!({ "tenant": caller.site.as_str(), "principal": session.principal.as_str() }),
    );
    Ok(json!({ "codes": codes, "left": codes.len() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_sixteen_symbols_in_four_groups_from_the_alphabet() {
        let code = new_code().unwrap();
        assert_eq!(code.len(), 19, "{code}");
        for group in code.split('-') {
            assert_eq!(group.len(), 4);
            assert!(group.bytes().all(|b| ALPHABET.contains(&b)), "{code}");
        }
        assert_ne!(new_code().unwrap(), new_code().unwrap());
    }

    #[test]
    fn a_code_hashes_the_same_however_it_is_written_and_differently_for_another_person() {
        let tenant = TenantId::new("alpha").unwrap();
        let ines = PrincipalId::new("ines").unwrap();
        let omar = PrincipalId::new("omar").unwrap();
        let one = hash(&tenant, &ines, "ABCD-EFGH-JKLM-NPQR");
        assert_eq!(one, hash(&tenant, &ines, "abcd efgh jklm npqr"));
        assert_eq!(one, hash(&tenant, &ines, "ABCDEFGHJKLMNPQR"));
        assert_ne!(one, hash(&tenant, &omar, "ABCD-EFGH-JKLM-NPQR"));
        assert_ne!(
            one,
            hash(
                &TenantId::new("beta").unwrap(),
                &ines,
                "ABCD-EFGH-JKLM-NPQR"
            )
        );
    }
}
