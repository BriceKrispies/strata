//! Sign in with an OpenID Connect identity provider.
//!
//! The flow is the authorization code flow with a proof key (D-7, FED-002):
//!
//! 1. **Start.** The person chooses a provider on the sign-in page. The backend keeps a pending
//!    request, found by the hash of a `state` value, that holds the nonce, the proof-key verifier
//!    and the hash of a binding nonce, and answers with the provider's authorisation address. The
//!    binding nonce goes to the starting browser as a `__Host-` cookie and is in no address.
//! 2. **Callback.** The provider sends the browser back with a `code` and the `state`. The pending
//!    request is taken out of the store in the step that reads it, so a callback answers once
//!    (FED-001), and is accepted only through its lifetime, from the browser that started it and at
//!    the address it started at. The code is redeemed at the provider's token endpoint with the
//!    verifier, and the identity token that comes back is verified (see `token`).
//! 3. **Policy.** What the verified token means for the realm is decided here, after the token and
//!    before any session: a disabled provider accepts nothing (FED-011); a known identity signs
//!    its principal in; an identity is linked to an existing principal only by that principal
//!    having signed in first, or, where the realm trusts the provider that far, by a verified email
//!    address (FED-012, FED-257); a new principal is created only where the realm enabled it and
//!    gets only the one default role (FED-011, FED-258).
//!
//! Everything common to sign-in methods is `signin::sign_in`'s: the limits on attempts, the one
//! refusal, the audit entry and the session. A method here answers who, or no one.
//!
//! Implements: FED-001, FED-002, FED-003, FED-004, FED-009, FED-010, FED-011, FED-012, FED-013,
//! FED-250, FED-255, FED-256, FED-257, FED-258, H-036, H-037, H-038, H-039, D-7

mod config;
mod token;

pub use config::{algorithm, Federation, Fetch, Http, Metadata, ProviderConfig, Secret};
pub use token::{Asserted, Refusal};

use super::link::{encode, same_site_path, site_origin, well_formed};
use super::{normalise_login, rules, throttled, SignInMethod};
use crate::app::{ApiError, App, LOG};
use crate::auth::{identify, random_hex, same_secret, token_hash, Caller, Identity};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};
use strata_kernel::{canon, owner_role, Context, Intent, Principal, PrincipalId, RoleId};
use strata_protocol::api::ErrorCode;
use strata_storage_api::{Account, FederatedIdentity, OidcFlow, ThrottleVerdict};

pub struct OidcMethod;

/// The longest authorisation code that is read.
const CODE_BYTES: usize = 2048;
/// The longest name that is kept for a person.
const NAME_CHARS: usize = 80;
/// The path at the site that a provider sends the browser back to.
pub const CALLBACK_PATH: &str = "/federation/oidc/callback";

fn code_ok(code: &str) -> bool {
    !code.is_empty() && code.len() <= CODE_BYTES && code.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

fn now_secs(app: &App) -> i64 {
    app.now().0 / 1000
}

/// The refusal that says nothing about why, and the reason for the log.
fn refuse(caller: &Caller, provider: &str, reason: &str) -> Result<Option<Account>, ApiError> {
    LOG.warn(
        "oidc.refused",
        &caller.correlation,
        json!({ "tenant": caller.site.as_str(), "provider": provider, "reason": reason }),
    );
    Ok(None)
}

/// The providers the sign-in page offers for the address it is served at (FED-010).
///
/// Implements: FED-010
pub fn providers(app: &App, caller: &Caller) -> Result<Json, ApiError> {
    let list: Vec<Json> = app
        .cfg
        .federation
        .providers(&caller.site)
        .iter()
        .filter(|provider| provider.enabled)
        .map(|provider| json!({ "id": provider.id, "label": provider.label }))
        .collect();
    Ok(json!({ "providers": list }))
}

/// The proof-key challenge of a verifier: the unpadded URL-safe base64 of its SHA-256 (RFC 7636,
/// method `S256`).
fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Begin a sign-in at a provider: keep the pending request and answer with the address to send the
/// browser to. The reply holds the binding nonce for the browser's cookie, which the gateway keeps
/// to itself, and the return address when it is a path on this site (FED-250).
///
/// `identity` is whoever is signed in; a request to link (`"link": true`) needs a person who is
/// (FED-012).
///
/// Implements: FED-002, FED-011, FED-012, FED-013, FED-250
pub fn start(
    app: &App,
    caller: &Caller,
    identity: &Identity,
    body: &Json,
) -> Result<Json, ApiError> {
    let Some(origin) = caller.origin.as_deref().and_then(site_origin) else {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "This request did not come from the site.",
        ));
    };
    let not_available = || ApiError::new(ErrorCode::Invalid, "That provider is not available.");
    let id = body
        .get("provider")
        .and_then(Json::as_str)
        .filter(|id| id.len() <= 32)
        .ok_or_else(not_available)?;
    let provider = app
        .cfg
        .federation
        .provider(&caller.site, id)
        .filter(|provider| provider.enabled)
        .ok_or_else(not_available)?;

    let link = body.get("link").and_then(Json::as_bool).unwrap_or(false);
    let principal = match (&identity.session, link) {
        (Some(session), true) => Some(session.principal.clone()),
        (None, true) => {
            return Err(ApiError::new(
                ErrorCode::Unauthenticated,
                "Sign in to link another way of signing in.",
            ))
        }
        _ => None,
    };

    let rules = rules(app, "oidc-start", &provider.id, &caller.source);
    if let ThrottleVerdict::Refused { .. } =
        app.store
            .throttle_reserve(&caller.site, &rules, app.now())?
    {
        return Err(throttled());
    }

    // A provider that does not answer takes away its own button and nothing else (FED-013).
    let metadata = app
        .cfg
        .federation
        .metadata(&caller.site, provider, app.now().0, app.cfg.dev)
        .map_err(|why| {
            LOG.warn(
                "oidc.provider_unreachable",
                &caller.correlation,
                json!({ "tenant": caller.site.as_str(), "provider": provider.id, "error": why }),
            );
            ApiError::new(
                ErrorCode::Unavailable,
                "That provider did not answer. Use another way to sign in.",
            )
        })?;

    let state = random_hex(32)?;
    let nonce = random_hex(16)?;
    let verifier = random_hex(32)?;
    let binding = match body.get("binding").and_then(Json::as_str) {
        Some(binding) if well_formed(binding) => binding.to_string(),
        _ => random_hex(32)?,
    };
    app.store.put_oidc_flow(&OidcFlow {
        tenant: caller.site.clone(),
        id_hash: token_hash(&state),
        provider: provider.id.clone(),
        nonce: nonce.clone(),
        verifier: verifier.clone(),
        binding_hash: token_hash(&binding),
        origin: origin.clone(),
        principal,
        expires_at: strata_kernel::Timestamp(app.now().0 + app.cfg.passkey_challenge_ttl_ms),
    })?;

    let redirect_uri = format!("{origin}{CALLBACK_PATH}");
    let scope = provider.scopes.join(" ");
    let code_challenge = challenge(&verifier);
    let parameters: [(&str, &str); 8] = [
        ("response_type", "code"),
        ("client_id", provider.client_id.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("scope", scope.as_str()),
        ("state", state.as_str()),
        ("nonce", nonce.as_str()),
        ("code_challenge", code_challenge.as_str()),
        ("code_challenge_method", "S256"),
    ];
    let query = parameters
        .iter()
        .map(|(name, value)| format!("{name}={}", encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    let separator = if metadata.authorization_endpoint.contains('?') {
        '&'
    } else {
        '?'
    };
    let ttl = (app.cfg.passkey_challenge_ttl_ms / 1000).max(1);
    LOG.info(
        "oidc.started",
        &caller.correlation,
        json!({ "tenant": caller.site.as_str(), "provider": provider.id, "link": link }),
    );
    let mut reply = json!({
        "redirect": format!("{}{separator}{query}", metadata.authorization_endpoint),
        "binding": binding,
        "max_age": ttl,
    });
    if let Some(next) = body
        .get("next")
        .and_then(Json::as_str)
        .and_then(same_site_path)
    {
        reply["return_to"] = json!(next);
    }
    Ok(reply)
}

/// Redeem the code and verify the identity token that comes back. `Err` is the reason for the log.
///
/// Implements: FED-003, FED-004, H-037
fn redeem(
    app: &App,
    caller: &Caller,
    provider: &ProviderConfig,
    flow: &OidcFlow,
    code: &str,
) -> Result<Asserted, &'static str> {
    let federation = &app.cfg.federation;
    let metadata = federation
        .metadata(&caller.site, provider, app.now().0, app.cfg.dev)
        .map_err(|_| "provider_unreachable")?;
    let redirect_uri = format!("{}{CALLBACK_PATH}", flow.origin);
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri.as_str()),
        ("client_id", provider.client_id.as_str()),
        ("code_verifier", flow.verifier.as_str()),
    ];
    if let Some(secret) = &provider.client_secret {
        form.push(("client_secret", secret.expose()));
    }
    let reply = federation
        .fetch
        .post_form(&metadata.token_endpoint, &form)
        .map_err(|_| "token_endpoint")?;
    let id_token = reply
        .get("id_token")
        .and_then(Json::as_str)
        .ok_or("no_id_token")?;

    let keys = federation
        .keys(&caller.site, provider, &metadata, app.now().0, false)
        .map_err(|_| "keys_unreachable")?;
    match token::verify(id_token, provider, &keys, &flow.nonce, now_secs(app)) {
        // A key that is not held: the provider's keys are fetched once more and the token is
        // tried once more. A second unknown key is a refusal (FED-004).
        Err(Refusal::UnknownKey) => {
            let keys = federation
                .keys(&caller.site, provider, &metadata, app.now().0, true)
                .map_err(|_| "keys_unreachable")?;
            token::verify(id_token, provider, &keys, &flow.nonce, now_secs(app))
                .map_err(Refusal::code)
        }
        other => other.map_err(Refusal::code),
    }
}

/// Tie the provider's subject to the principal, or find that it already is.
fn tie(
    app: &App,
    caller: &Caller,
    provider: &ProviderConfig,
    subject: &str,
    principal: &PrincipalId,
) -> Result<bool, ApiError> {
    let added = app.store.put_identity(&FederatedIdentity {
        tenant: caller.site.clone(),
        provider: provider.id.clone(),
        subject: subject.to_string(),
        principal: principal.clone(),
        created_at: app.now(),
    })?;
    if added {
        return Ok(true);
    }
    Ok(app
        .store
        .identity(&caller.site, &provider.id, subject)?
        .is_some_and(|known| known.principal == *principal))
}

fn account_of(
    app: &App,
    caller: &Caller,
    provider: &ProviderConfig,
    principal: &PrincipalId,
) -> Result<Option<Account>, ApiError> {
    match app.store.account_of(&caller.site, principal)? {
        Some(account) => Ok(Some(account)),
        None => refuse(caller, &provider.id, "no_account"),
    }
}

/// Give a principal the one role the realm names for a first sign-in. The platform acts as the
/// tenant's owner for it, as installing a bundle does; it is the only role this path grants
/// (FED-258).
///
/// Implements: FED-258
fn grant_default_role(
    app: &App,
    caller: &Caller,
    provider: &ProviderConfig,
    principal: &PrincipalId,
) -> Result<(), ApiError> {
    let Some(role) = &provider.default_role else {
        return Ok(());
    };
    let role = RoleId::new(role.clone())
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, format!("default role: {e}")))?;
    let tenant = app
        .store
        .load(&caller.site, &[])?
        .tenant
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such site"))?;
    let owner = tenant
        .members
        .iter()
        .find(|(_, roles)| roles.contains(&owner_role()))
        .map(|(principal, _)| principal.clone())
        .ok_or_else(|| ApiError::hidden(ErrorCode::Internal, "the site has no owner"))?;
    let ctx = Context {
        tenant: caller.site.clone(),
        principal: Principal::User(owner),
        session: None,
        now: app.now(),
        correlation: caller.correlation.clone(),
    };
    crate::ops::run_intent(
        app,
        &ctx,
        &Intent::AssignRole {
            principal: principal.clone(),
            role,
        },
        false,
    )?;
    Ok(())
}

/// Create the principal of a first sign-in: a login of its own that no password opens, the
/// identity that ties it to the provider's subject, and the default role. The principal's name is
/// made from the tenant, the provider and the subject, so that two first sign-ins that arrive
/// together make one.
///
/// Implements: FED-011, FED-258
fn create(
    app: &App,
    caller: &Caller,
    provider: &ProviderConfig,
    who: &Asserted,
) -> Result<Option<Account>, ApiError> {
    if !provider.domains.is_empty() {
        let domain = who
            .email
            .as_deref()
            .filter(|_| who.email_verified)
            .and_then(|email| email.rsplit_once('@'))
            .map(|(_, domain)| domain.to_lowercase());
        if !domain.is_some_and(|domain| provider.domains.contains(&domain)) {
            return refuse(caller, &provider.id, "domain");
        }
    }
    let digest = canon::hash_bytes(
        format!(
            "oidc\0{}\0{}\0{}",
            caller.site.as_str(),
            provider.id,
            who.subject
        )
        .as_bytes(),
    );
    let principal = PrincipalId::new(format!("user:oidc-{}", &digest[..24]))
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, format!("principal: {e}")))?;
    let display_name = who
        .name
        .as_deref()
        .map(|name| name.trim().chars().take(NAME_CHARS).collect::<String>())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Signed-in person".to_string());
    let account = Account {
        tenant: caller.site.clone(),
        login: format!("oidc:{}:{}", provider.id, &digest[..32]),
        principal: principal.clone(),
        display_name,
        // Not a password hash: no password opens this login.
        password_hash: String::new(),
    };
    app.store.put_account(&account)?;
    if !tie(app, caller, provider, &who.subject, &principal)? {
        return refuse(caller, &provider.id, "identity_tied_to_another");
    }
    grant_default_role(app, caller, provider, &principal)?;
    app.audit(
        "oidc.principal_created",
        &caller.correlation,
        json!({
            "tenant": caller.site.as_str(),
            "provider": provider.id,
            "principal": principal.as_str(),
            "role": provider.default_role,
        }),
    );
    Ok(Some(account))
}

/// What a verified token means: whose account, if anyone's.
///
/// Implements: FED-011, FED-012, FED-257, H-039
fn resolve(
    app: &App,
    caller: &Caller,
    provider: &ProviderConfig,
    flow: &OidcFlow,
    who: &Asserted,
) -> Result<Option<Account>, ApiError> {
    if let Some(known) = app
        .store
        .identity(&caller.site, &provider.id, &who.subject)?
    {
        // A person who asked to link an identity that already belongs to another is refused.
        if flow
            .principal
            .as_ref()
            .is_some_and(|p| *p != known.principal)
        {
            return refuse(caller, &provider.id, "identity_tied_to_another");
        }
        return account_of(app, caller, provider, &known.principal);
    }
    if let Some(principal) = &flow.principal {
        // The principal signed in before asking to link (FED-012).
        if !tie(app, caller, provider, &who.subject, principal)? {
            return refuse(caller, &provider.id, "identity_tied_to_another");
        }
        app.audit(
            "oidc.identity_linked",
            &caller.correlation,
            json!({ "tenant": caller.site.as_str(), "provider": provider.id, "principal": principal.as_str(), "by": "sign_in" }),
        );
        return account_of(app, caller, provider, principal);
    }
    if provider.link_verified_email && who.email_verified {
        if let Some(email) = &who.email {
            if let Some(account) = app.store.account(&caller.site, &normalise_login(email))? {
                if tie(app, caller, provider, &who.subject, &account.principal)? {
                    app.audit(
                        "oidc.identity_linked",
                        &caller.correlation,
                        json!({ "tenant": caller.site.as_str(), "provider": provider.id, "principal": account.principal.as_str(), "by": "verified_email" }),
                    );
                    return Ok(Some(account));
                }
            }
        }
    }
    if provider.create_on_first_sign_in {
        return create(app, caller, provider, who);
    }
    refuse(caller, &provider.id, "not_admitted")
}

impl SignInMethod for OidcMethod {
    fn name(&self) -> &'static str {
        "oidc"
    }

    /// The `state` the callback carries. The account is not known until the token is checked, so
    /// the limit on an account counts the pending request; the source's and the realm's count the
    /// attempt.
    fn subject(&self, request: &Json) -> Option<String> {
        let text = |name: &str| request.get(name)?.as_str();
        text("code").filter(|code| code_ok(code))?;
        text("binding").filter(|binding| well_formed(binding))?;
        text("state")
            .filter(|state| well_formed(state))
            .map(str::to_string)
    }

    /// Implements: FED-001, FED-003, FED-011
    fn verify(
        &self,
        app: &App,
        caller: &Caller,
        subject: &str,
        request: &Json,
    ) -> Result<Option<Account>, ApiError> {
        let text = |name: &str| request.get(name).and_then(Json::as_str).unwrap_or_default();
        // The request is taken in the step that reads it: whatever follows, this callback has
        // used it.
        let Some(flow) = app
            .store
            .take_oidc_flow(&caller.site, &token_hash(subject))?
        else {
            return refuse(caller, "", "no_pending_request");
        };
        // Every check is made, and compared in constant time, whichever one fails.
        let in_time = app.now() <= flow.expires_at;
        let browser = same_secret(&token_hash(text("binding")), &flow.binding_hash);
        let site =
            caller.origin.as_deref().and_then(site_origin).as_deref() == Some(flow.origin.as_str());
        if !(in_time && browser && site) {
            return refuse(caller, &flow.provider, "state");
        }
        let federation = &app.cfg.federation;
        let Some(provider) = federation
            .provider(&caller.site, &flow.provider)
            .filter(|provider| provider.enabled)
        else {
            return refuse(caller, &flow.provider, "provider_not_enabled");
        };
        // A request to link is answered only to the person who made it.
        if let Some(principal) = &flow.principal {
            let identity = identify(app, caller)?;
            if !identity
                .session
                .as_ref()
                .is_some_and(|session| session.principal == *principal)
            {
                return refuse(caller, &provider.id, "link_session");
            }
        }
        let who = match redeem(app, caller, provider, &flow, text("code")) {
            Ok(who) => who,
            Err(reason) => return refuse(caller, &provider.id, reason),
        };
        resolve(app, caller, provider, &flow, &who)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example of RFC 7636, appendix B.
    #[test]
    fn the_proof_key_challenge_is_the_s256_of_the_verifier() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn a_code_is_printable_ascii_and_bounded() {
        assert!(code_ok("SplxlOBeZQQYbYS6WxSbIA"));
        assert!(!code_ok(""));
        assert!(!code_ok("has space"));
        assert!(!code_ok("line\nbreak"));
        assert!(!code_ok(&"a".repeat(CODE_BYTES + 1)));
    }
}
