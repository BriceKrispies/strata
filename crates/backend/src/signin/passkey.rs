//! Sign in with a passkey (WebAuthn), and the registration and management of passkeys.
//!
//! The protocol is the `webauthn-rs` library's: it builds the challenge, parses what the
//! browser returns and checks the attestation, the signature, the relying party, the origin,
//! the user-verification flag and the signature counter. This module decides only what the
//! library cannot: whose credential it is, where its state is kept, and what a refusal says.
//!
//! - **Relying party.** The relying party is the host of the address the person is at, so one
//!   tenant's passkey is of no use at another's address, and an assertion made for another
//!   address does not verify. The address is the origin the BFF checked and passed on.
//! - **Challenges.** Every challenge is kept server-side with the address it was issued at (and,
//!   for a registration, the session that asked), is taken out of the store in the step that
//!   reads it so it answers once, and lapses.
//! - **Sign-in.** The person does not name themselves: the credential names its owner (a
//!   discoverable credential), so there is no login whose existence a challenge could give away.
//!
//! Implements: IDN-007, IDN-264

use super::{throttled, SignInMethod};
use crate::app::{ApiError, App};
use crate::auth::{random_hex, Caller, Identity};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::{json, Value as Json};
use strata_kernel::{canon, PrincipalId, TenantId, Timestamp};
use strata_protocol::api::ErrorCode;
use strata_storage_api::{
    Account, PasskeyCeremony, PasskeyChallenge, PasskeyRecord, ThrottleRule, ThrottleVerdict,
};
use webauthn_rs::prelude::{
    DiscoverableAuthentication, Passkey as Credential, PasskeyRegistration, PublicKeyCredential,
    RegisterPublicKeyCredential, Url, Uuid, Webauthn, WebauthnBuilder, WebauthnError,
};

pub struct PasskeyMethod;

/// The most passkeys one person keeps, so that a session cannot fill the store.
const MAX_PER_PERSON: usize = 10;
/// The longest name a person gives a passkey, in bytes.
const LABEL_BYTES: usize = 64;
/// A challenge identifier as issued: 32 random bytes in hex.
const ID_LEN: usize = 64;

fn not_added() -> ApiError {
    ApiError::new(ErrorCode::Invalid, "The passkey could not be added.")
}

fn sign_in_needed() -> ApiError {
    ApiError::new(ErrorCode::Unauthenticated, "Sign in to do this.")
}

/// A challenge identifier is hex of the length that is issued, so that nothing else is looked up.
fn well_formed_id(id: &str) -> bool {
    id.len() == ID_LEN && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The relying party for an origin: its host is the relying party id, and only that origin
/// verifies. A site served at an address that is not a name (an IP address) cannot have passkeys,
/// because WebAuthn does not allow it.
///
/// Implements: IDN-007
fn relying_party(origin: Option<&str>) -> Result<(Webauthn, String), ApiError> {
    let unavailable = || {
        ApiError::new(
            ErrorCode::Invalid,
            "Passkeys need the site to be reached by its name.",
        )
    };
    let url = Url::parse(origin.ok_or_else(unavailable)?).map_err(|_| unavailable())?;
    let host = url.domain().ok_or_else(unavailable)?.to_string();
    let webauthn = WebauthnBuilder::new(&host, &url)
        .and_then(|builder| builder.rp_name(&host).build())
        .map_err(|_| unavailable())?;
    Ok((webauthn, url.origin().ascii_serialization()))
}

/// An origin in the one spelling that is compared and kept.
fn normal_origin(origin: &str) -> Option<String> {
    let origin = Url::parse(origin).ok()?.origin();
    origin.is_tuple().then(|| origin.ascii_serialization())
}

/// The relying party id an origin has: its host.
fn relying_party_id(origin: &str) -> Option<String> {
    Url::parse(origin).ok()?.domain().map(str::to_string)
}

/// The identifier a person has at the authenticator, the same for all of one person's passkeys
/// at one tenant and for nobody else's. It carries no information about the person.
fn user_handle(tenant: &TenantId, principal: &PrincipalId) -> Uuid {
    let hash = canon::hash_bytes(
        format!("passkey-user\0{}\0{}", tenant.as_str(), principal.as_str()).as_bytes(),
    );
    Uuid::from_u128(u128::from_str_radix(&hash[..32], 16).unwrap_or_default())
}

fn credential_id(id: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(id)
}

fn encode<T: serde::Serialize>(value: &T) -> Result<String, ApiError> {
    serde_json::to_string(value)
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, format!("passkey state: {e}")))
}

/// Issue a challenge: keep its state, hand back the identifier the browser answers with.
fn issue(
    app: &App,
    caller: &Caller,
    ceremony: PasskeyCeremony,
    identity: Option<&Identity>,
    origin: String,
    state: String,
) -> Result<String, ApiError> {
    let id = random_hex(ID_LEN / 2)?;
    let session = identity.and_then(|i| i.session.as_ref());
    app.store.put_passkey_challenge(&PasskeyChallenge {
        tenant: caller.site.clone(),
        id_hash: canon::hash_bytes(id.as_bytes()),
        ceremony,
        session: session.map(|s| s.id.clone()),
        principal: session.map(|s| s.principal.clone()),
        origin,
        state,
        expires_at: Timestamp(app.now().0 + app.cfg.passkey_challenge_ttl_ms),
    })?;
    Ok(id)
}

/// Take a challenge for the ceremony, once, if it is the one issued for this address and has not
/// lapsed. Whatever the reason it is not usable, the answer is the same.
///
/// Implements: IDN-264
fn take(
    app: &App,
    caller: &Caller,
    id: &str,
    ceremony: PasskeyCeremony,
) -> Result<Option<PasskeyChallenge>, ApiError> {
    if !well_formed_id(id) {
        return Ok(None);
    }
    let taken = app.store.take_passkey_challenge(
        &caller.site,
        &canon::hash_bytes(id.as_bytes()),
        app.now(),
    )?;
    let at = caller.origin.as_deref().and_then(normal_origin);
    Ok(taken.filter(|challenge| {
        challenge.ceremony == ceremony && at.as_deref() == Some(challenge.origin.as_str())
    }))
}

/// Begin a sign-in: a challenge for the address the person is at. Anyone may ask, so each ask
/// counts against the source's limit; it is never given back.
///
/// Implements: H-031
pub fn sign_in_challenge(app: &App, caller: &Caller) -> Result<Json, ApiError> {
    let (webauthn, origin) = relying_party(caller.origin.as_deref())?;
    let limits = &app.cfg.throttle;
    let rule = ThrottleRule {
        key: format!("source:passkey-challenge:{}", caller.source),
        max: limits.source_max,
        window_ms: limits.window_ms,
        lock_ms: limits.window_ms,
        lock_resets: false,
    };
    if let ThrottleVerdict::Refused { .. } =
        app.store
            .throttle_reserve(&caller.site, std::slice::from_ref(&rule), app.now())?
    {
        return Err(throttled());
    }
    let (options, state) = webauthn
        .start_discoverable_authentication()
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, format!("passkey challenge: {e}")))?;
    let challenge = issue(
        app,
        caller,
        PasskeyCeremony::SignIn,
        None,
        origin,
        encode(&state)?,
    )?;
    Ok(json!({ "challenge": challenge, "options": options }))
}

impl SignInMethod for PasskeyMethod {
    fn name(&self) -> &'static str {
        "passkey"
    }

    /// The challenge the assertion answers. The person is not known until the assertion is
    /// checked, so the limit on an account has nothing to count here; the source's and the
    /// realm's do.
    fn subject(&self, request: &Json) -> Option<String> {
        let challenge = request.get("challenge")?.as_str()?;
        request.get("assertion")?.as_object()?;
        well_formed_id(challenge).then(|| challenge.to_string())
    }

    /// Implements: H-017, IDN-007, IDN-264
    fn verify(
        &self,
        app: &App,
        caller: &Caller,
        subject: &str,
        request: &Json,
    ) -> Result<Option<Account>, ApiError> {
        let Some(challenge) = take(app, caller, subject, PasskeyCeremony::SignIn)? else {
            return Ok(None);
        };
        let Ok((webauthn, _)) = relying_party(Some(&challenge.origin)) else {
            return Ok(None);
        };
        let Ok(assertion) =
            serde_json::from_value::<PublicKeyCredential>(request["assertion"].clone())
        else {
            return Ok(None);
        };
        let Ok(state) = serde_json::from_str::<DiscoverableAuthentication>(&challenge.state) else {
            return Ok(None);
        };
        // The credential says whose it is, and which it is; both are looked up in this tenant.
        let Ok((handle, id)) = webauthn.identify_discoverable_authentication(&assertion) else {
            return Ok(None);
        };
        let Some(record) = app.store.passkey(&caller.site, &credential_id(id))? else {
            return Ok(None);
        };
        // A credential is used at the address it was made for, and by the person it names.
        if user_handle(&record.tenant, &record.principal) != handle
            || relying_party_id(&challenge.origin).as_deref() != Some(record.rp_id.as_str())
        {
            return Ok(None);
        }
        let Ok(mut credential) = serde_json::from_str::<Credential>(&record.credential) else {
            return Ok(None);
        };
        let result =
            match webauthn.finish_discoverable_authentication_v2(&assertion, state, &credential) {
                Ok(result) => result,
                Err(error) => {
                    if matches!(error, WebauthnError::CredentialPossibleCompromise) {
                        // The signature counter went backwards: the credential may have been copied.
                        app.audit(
                            "passkey.counter_regressed",
                            &caller.correlation,
                            json!({ "tenant": caller.site.as_str(), "credential": record.id }),
                        );
                    }
                    return Ok(None);
                }
            };
        // The counter moves on, and the use is noted.
        credential.update_credential(&result);
        app.store
            .update_passkey(&caller.site, &record.id, &encode(&credential)?, app.now())?;
        app.store
            .account_of(&caller.site, &record.principal)
            .map_err(Into::into)
    }
}

/// Begin adding a passkey to the signed-in person's account: a challenge bound to the session.
///
/// Implements: H-031, IDN-007
pub fn register_start(app: &App, caller: &Caller, identity: &Identity) -> Result<Json, ApiError> {
    let Some(session) = &identity.session else {
        return Err(sign_in_needed());
    };
    let (webauthn, origin) = relying_party(caller.origin.as_deref())?;
    let held = app.store.passkeys(&caller.site, &session.principal)?;
    if held.len() >= MAX_PER_PERSON {
        return Err(ApiError::new(
            ErrorCode::Conflict,
            "Remove a passkey before adding another.",
        ));
    }
    let exclude: Vec<_> = held
        .iter()
        .filter_map(|p| serde_json::from_str::<Credential>(&p.credential).ok())
        .map(|credential| credential.cred_id().clone())
        .collect();
    let login = app
        .store
        .account_of(&caller.site, &session.principal)?
        .map_or_else(|| session.principal.as_str().to_string(), |a| a.login);
    let (options, state) = webauthn
        .start_passkey_registration(
            user_handle(&caller.site, &session.principal),
            &login,
            &session.display_name,
            Some(exclude),
        )
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, format!("passkey challenge: {e}")))?;
    let challenge = issue(
        app,
        caller,
        PasskeyCeremony::Register,
        Some(identity),
        origin,
        encode(&state)?,
    )?;
    Ok(json!({ "challenge": challenge, "options": options }))
}

/// Finish adding a passkey: the attestation answers the challenge of this session, at this
/// address, and the credential is kept for this tenant and person.
///
/// Implements: IDN-007, IDN-264
pub fn register_finish(
    app: &App,
    caller: &Caller,
    identity: &Identity,
    request: &Json,
) -> Result<Json, ApiError> {
    let Some(session) = &identity.session else {
        return Err(sign_in_needed());
    };
    let challenge_id = request
        .get("challenge")
        .and_then(Json::as_str)
        .unwrap_or_default();
    let Some(challenge) = take(app, caller, challenge_id, PasskeyCeremony::Register)?
        .filter(|c| c.session.as_ref() == Some(&session.id))
    else {
        return Err(not_added());
    };
    let (webauthn, _) = relying_party(Some(&challenge.origin))?;
    let attestation = serde_json::from_value::<RegisterPublicKeyCredential>(
        request.get("credential").cloned().unwrap_or(Json::Null),
    )
    .map_err(|_| not_added())?;
    let state =
        serde_json::from_str::<PasskeyRegistration>(&challenge.state).map_err(|_| not_added())?;
    let credential = webauthn
        .finish_passkey_registration(&attestation, &state)
        .map_err(|_| not_added())?;
    let label = request
        .get("name")
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty() && name.len() <= LABEL_BYTES)
        .unwrap_or("Passkey")
        .to_string();
    let record = PasskeyRecord {
        tenant: caller.site.clone(),
        principal: session.principal.clone(),
        id: credential_id(credential.cred_id()),
        rp_id: relying_party_id(&challenge.origin).unwrap_or_default(),
        label,
        credential: encode(&credential)?,
        created_at: app.now(),
        last_used_at: None,
    };
    if !app.store.put_passkey(&record)? {
        // The credential is already registered, to this person or to another: say nothing more.
        return Err(not_added());
    }
    app.audit(
        "passkey.registered",
        &caller.correlation,
        json!({ "tenant": caller.site.as_str(), "principal": session.principal.as_str() }),
    );
    Ok(json!({ "passkey": summary(&record) }))
}

fn summary(record: &PasskeyRecord) -> Json {
    json!({
        "id": record.id,
        "name": record.label,
        "created_at": record.created_at.0,
        "last_used_at": record.last_used_at.map(|at| at.0),
    })
}

/// The signed-in person's own passkeys.
///
/// Implements: H-032
pub fn list(app: &App, caller: &Caller, identity: &Identity) -> Result<Json, ApiError> {
    let Some(session) = &identity.session else {
        return Err(sign_in_needed());
    };
    let passkeys = app.store.passkeys(&caller.site, &session.principal)?;
    Ok(json!({ "passkeys": passkeys.iter().map(summary).collect::<Vec<_>>() }))
}

/// Remove one of the signed-in person's own passkeys. One that is not theirs, or not there, is
/// answered as not found: the answer does not say which.
///
/// Implements: H-033
pub fn remove(app: &App, caller: &Caller, identity: &Identity, id: &str) -> Result<Json, ApiError> {
    let Some(session) = &identity.session else {
        return Err(sign_in_needed());
    };
    if !app
        .store
        .remove_passkey(&caller.site, &session.principal, id)?
    {
        return Err(ApiError::new(ErrorCode::NotFound, "No such passkey."));
    }
    app.audit(
        "passkey.removed",
        &caller.correlation,
        json!({ "tenant": caller.site.as_str(), "principal": session.principal.as_str() }),
    );
    Ok(json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_relying_party_is_the_host_of_the_origin() {
        let (_, origin) = relying_party(Some("https://tidewater.example:8443")).unwrap();
        assert_eq!(origin, "https://tidewater.example:8443");
        assert!(relying_party(Some("http://127.0.0.1:4200")).is_err());
        assert!(relying_party(Some("not a url")).is_err());
        assert!(relying_party(None).is_err());
    }

    #[test]
    fn a_person_has_one_handle_at_a_tenant_and_it_is_not_shared() {
        let a = TenantId::new("alpha").unwrap();
        let b = TenantId::new("beta").unwrap();
        let ines = PrincipalId::new("ines").unwrap();
        let omar = PrincipalId::new("omar").unwrap();
        assert_eq!(user_handle(&a, &ines), user_handle(&a, &ines));
        assert_ne!(user_handle(&a, &ines), user_handle(&b, &ines));
        assert_ne!(user_handle(&a, &ines), user_handle(&a, &omar));
    }
}
