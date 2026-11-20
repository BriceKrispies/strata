//! What a verified external identity means for the realm, for every protocol.
//!
//! A protocol module (OpenID Connect, SAML) answers one question: who does the provider say this
//! is. What that means for the realm is decided here, once, after the protocol's checks and before
//! any session: a disabled provider accepts nothing (FED-011); a known identity signs its
//! principal in; an identity is linked to an existing principal only by that principal having
//! signed in first, or, where the realm trusts the provider that far, by a verified email address
//! (FED-012, FED-257); a new principal is created only where the realm enabled it, in a domain the
//! provider is bound to, and gets only the one default role (FED-011, FED-258).
//!
//! The policy is the same for a provider of either protocol, so it is written once and a protocol
//! hands it an [`Admission`], the part of its configuration that decides who gets in.
//!
//! Implements: FED-009, FED-011, FED-012, FED-257, FED-258, H-039

use super::normalise_login;
use crate::app::{ApiError, App, LOG};
use crate::auth::Caller;
use serde_json::json;
use strata_kernel::{canon, owner_role, Context, Intent, Principal, PrincipalId, RoleId};
use strata_protocol::api::ErrorCode;
use strata_storage_api::{Account, FederatedIdentity};

/// The longest name that is kept for a person.
const NAME_CHARS: usize = 80;

/// What a protocol verified about the person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asserted {
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub name: Option<String>,
}

/// The part of a provider's configuration that decides who gets in.
#[derive(Clone, Copy, Debug)]
pub struct Admission<'a> {
    /// The protocol, which names its audit entries and its principals: `oidc` or `saml`.
    pub protocol: &'static str,
    /// The identifier of the provider in the tenant's configuration.
    pub provider: &'a str,
    /// Email domains the provider decides for.
    pub domains: &'a [String],
    /// Whether a first sign-in creates a principal (FED-011).
    pub create_on_first_sign_in: bool,
    /// The one role a principal created at a first sign-in receives (FED-258).
    pub default_role: Option<&'a str>,
    /// Whether a verified email address links to the account whose login it is (FED-257).
    pub link_verified_email: bool,
}

/// The rules a provider's admission settings must keep, checked when the configuration is loaded:
/// linking by verified email and first sign-in both need the provider's domains, and a first
/// sign-in never grants the owner role.
///
/// Implements: FED-011, FED-257, FED-258
pub fn check_admission(
    domains: &[String],
    create_on_first_sign_in: bool,
    link_verified_email: bool,
    default_role: Option<&str>,
) -> Result<(), &'static str> {
    if link_verified_email && domains.is_empty() {
        return Err("link_verified_email needs domains");
    }
    if create_on_first_sign_in && domains.is_empty() {
        return Err("create_on_first_sign_in needs domains");
    }
    if default_role.is_some_and(|role| role.trim() == "owner") {
        return Err("default_role cannot be owner");
    }
    Ok(())
}

/// The refusal that says nothing about why, and the reason for the log.
pub fn refuse(
    caller: &Caller,
    protocol: &str,
    provider: &str,
    reason: &str,
) -> Result<Option<Account>, ApiError> {
    LOG.warn(
        &format!("{protocol}.refused"),
        &caller.correlation,
        json!({ "tenant": caller.site.as_str(), "provider": provider, "reason": reason }),
    );
    Ok(None)
}

/// Tie the provider's subject to the principal, or find that it already is.
fn tie(
    app: &App,
    caller: &Caller,
    admission: &Admission,
    subject: &str,
    principal: &PrincipalId,
) -> Result<bool, ApiError> {
    let added = app.store.put_identity(&FederatedIdentity {
        tenant: caller.site.clone(),
        provider: admission.provider.to_string(),
        subject: subject.to_string(),
        principal: principal.clone(),
        created_at: app.now(),
    })?;
    if added {
        return Ok(true);
    }
    Ok(app
        .store
        .identity(&caller.site, admission.provider, subject)?
        .is_some_and(|known| known.principal == *principal))
}

fn account_of(
    app: &App,
    caller: &Caller,
    admission: &Admission,
    principal: &PrincipalId,
) -> Result<Option<Account>, ApiError> {
    match app.store.account_of(&caller.site, principal)? {
        Some(account) => Ok(Some(account)),
        None => refuse(caller, admission.protocol, admission.provider, "no_account"),
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
    admission: &Admission,
    principal: &PrincipalId,
) -> Result<(), ApiError> {
    let Some(role) = admission.default_role else {
        return Ok(());
    };
    let role = RoleId::new(role.to_string())
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
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
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
/// made from the protocol, the tenant, the provider and the subject, so that two first sign-ins
/// that arrive together make one.
///
/// Implements: FED-011, FED-258
fn create(
    app: &App,
    caller: &Caller,
    admission: &Admission,
    who: &Asserted,
) -> Result<Option<Account>, ApiError> {
    let protocol = admission.protocol;
    let provider = admission.provider;
    if !admission.domains.is_empty() {
        let domain = who
            .email
            .as_deref()
            .filter(|_| who.email_verified)
            .and_then(|email| email.rsplit_once('@'))
            .map(|(_, domain)| domain.to_lowercase());
        if !domain.is_some_and(|domain| admission.domains.contains(&domain)) {
            return refuse(caller, protocol, provider, "domain");
        }
    }
    let digest = canon::hash_bytes(
        format!(
            "{protocol}\0{}\0{provider}\0{}",
            caller.site.as_str(),
            who.subject
        )
        .as_bytes(),
    );
    let principal = PrincipalId::new(format!("user:{protocol}-{}", &digest[..24]))
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, format!("principal: {e}")))?;
    let display_name = who
        .name
        .as_deref()
        .map(|name| name.trim().chars().take(NAME_CHARS).collect::<String>())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Signed-in person".to_string());
    // The address is the person's own when the provider says it checked it and the realm trusts
    // what the provider says: it asked to link by that claim, or restricted the provider to
    // domains (checked above). Only then does it redeem an invitation (ADM-153).
    let proven = who
        .email
        .as_deref()
        .filter(|_| {
            who.email_verified && (admission.link_verified_email || !admission.domains.is_empty())
        })
        .map(normalise_login);
    let account = Account {
        tenant: caller.site.clone(),
        login: format!("{protocol}:{provider}:{}", &digest[..32]),
        principal: principal.clone(),
        display_name,
        // Not a password hash: no password opens this login.
        password_hash: String::new(),
        verified_email: proven,
    };
    // Created, never overwritten: a second first sign-in that arrives together with this one,
    // or a later one, keeps the account as it is.
    let account = if app.store.create_account(&account)? {
        account
    } else {
        app.store
            .account(&caller.site, &account.login)?
            .unwrap_or(account)
    };
    if !tie(app, caller, admission, &who.subject, &principal)? {
        return refuse(caller, protocol, provider, "identity_tied_to_another");
    }
    grant_default_role(app, caller, admission, &principal)?;
    app.audit(
        &format!("{protocol}.principal_created"),
        &caller.correlation,
        json!({
            "tenant": caller.site.as_str(),
            "provider": provider,
            "principal": principal.as_str(),
            "role": admission.default_role,
        }),
    );
    Ok(Some(account))
}

/// What a verified identity means: whose account, if anyone's. `linking` is the principal that was
/// signed in when the sign-in started and asked to link this identity to its account (FED-012).
///
/// Implements: FED-011, FED-012, FED-257, H-039
pub fn resolve(
    app: &App,
    caller: &Caller,
    admission: &Admission,
    linking: Option<&PrincipalId>,
    who: &Asserted,
) -> Result<Option<Account>, ApiError> {
    let protocol = admission.protocol;
    let provider = admission.provider;
    if let Some(known) = app.store.identity(&caller.site, provider, &who.subject)? {
        // A person who asked to link an identity that already belongs to another is refused.
        if linking.is_some_and(|p| *p != known.principal) {
            return refuse(caller, protocol, provider, "identity_tied_to_another");
        }
        return account_of(app, caller, admission, &known.principal);
    }
    if let Some(principal) = linking {
        // The principal signed in before asking to link (FED-012).
        if !tie(app, caller, admission, &who.subject, principal)? {
            return refuse(caller, protocol, provider, "identity_tied_to_another");
        }
        app.audit(
            &format!("{protocol}.identity_linked"),
            &caller.correlation,
            json!({ "tenant": caller.site.as_str(), "provider": provider, "principal": principal.as_str(), "by": "sign_in" }),
        );
        return account_of(app, caller, admission, principal);
    }
    if admission.link_verified_email && who.email_verified {
        if let Some(email) = who.email.as_ref().filter(|email| {
            // Only an address in a domain the provider decides for may link (FED-257).
            email.rsplit_once('@').is_some_and(|(_, domain)| {
                admission.domains.contains(&domain.trim().to_lowercase())
            })
        }) {
            if let Some(account) = app.store.account(&caller.site, &normalise_login(email))? {
                if tie(app, caller, admission, &who.subject, &account.principal)? {
                    app.audit(
                        &format!("{protocol}.identity_linked"),
                        &caller.correlation,
                        json!({ "tenant": caller.site.as_str(), "provider": provider, "principal": account.principal.as_str(), "by": "verified_email" }),
                    );
                    return Ok(Some(account));
                }
            }
        }
    }
    if admission.create_on_first_sign_in {
        return create(app, caller, admission, who);
    }
    refuse(caller, protocol, provider, "not_admitted")
}
