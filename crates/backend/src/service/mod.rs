//! The identity service's records and the parts of it that are not a way of signing in: issued
//! tokens and their signing keys, service accounts (clients) and delegations (grants), and the
//! provisioning of people from an identity provider.
//!
//! Each record is a small JSON document the store keeps under a kind and a key of its tenant
//! (`Store::doc_put`), so a tenant's keys, clients and grants are never another tenant's: the key
//! of a document is looked up inside the tenant the request was made at, and nothing else.
//!
//! Implements: TOK-001, TOK-252, FED-259

pub mod clients;
pub mod keys;
pub mod scim;
pub mod tokens;

use crate::app::{ApiError, App};
use crate::auth::Identity;
use serde::de::DeserializeOwned;
use serde::Serialize;
use strata_kernel::rules::roles_of;
use strata_kernel::{owner_role, Context, Principal, PrincipalId, TenantId};
use strata_protocol::api::ErrorCode;

/// A document of the tenant, read.
pub(crate) fn get<T: DeserializeOwned>(
    app: &App,
    tenant: &TenantId,
    kind: &str,
    key: &str,
) -> Result<Option<T>, ApiError> {
    match app.store.doc_get(tenant, kind, key)? {
        None => Ok(None),
        Some(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| ApiError::hidden(ErrorCode::Internal, format!("{kind} {key}: {e}"))),
    }
}

/// A document of the tenant, kept. With `create_only`, `false` says the key was taken.
pub(crate) fn put<T: Serialize>(
    app: &App,
    tenant: &TenantId,
    kind: &str,
    key: &str,
    value: &T,
    create_only: bool,
) -> Result<bool, ApiError> {
    let text = serde_json::to_string(value)
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, format!("{kind} {key}: {e}")))?;
    Ok(app.store.doc_put(tenant, kind, key, &text, create_only)?)
}

/// Every document of a kind, by key.
pub(crate) fn list<T: DeserializeOwned>(
    app: &App,
    tenant: &TenantId,
    kind: &str,
) -> Result<Vec<T>, ApiError> {
    app.store
        .doc_list(tenant, kind)?
        .into_iter()
        .map(|(key, text)| {
            serde_json::from_str(&text)
                .map_err(|e| ApiError::hidden(ErrorCode::Internal, format!("{kind} {key}: {e}")))
        })
        .collect()
}

/// The caller must be signed in as an owner of the tenant.
pub(crate) fn require_owner(app: &App, identity: &Identity) -> Result<(), ApiError> {
    if identity.anonymous() {
        return Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in as the site's owner to do this.",
        ));
    }
    let tenant = crate::ops::header(app, identity)?;
    if roles_of(&tenant, &identity.principal).contains(&owner_role()) {
        Ok(())
    } else {
        Err(ApiError::new(
            ErrorCode::Forbidden,
            "Only the site's owner may do this.",
        ))
    }
}

/// The signed-in person of a request.
pub(crate) fn person(identity: &Identity) -> Result<PrincipalId, ApiError> {
    match &identity.principal {
        Principal::User(id) if !identity.anonymous() => Ok(id.clone()),
        _ => Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in to do this.",
        )),
    }
}

/// A field of a request body that must be a text of at most `max` characters.
pub(crate) fn text<'a>(
    body: &'a serde_json::Value,
    name: &str,
    max: usize,
) -> Result<&'a str, ApiError> {
    body.get(name)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty() && value.chars().count() <= max)
        .ok_or_else(|| ApiError::new(ErrorCode::Invalid, format!("{name} is missing or invalid.")))
}

/// The context the platform acts in for a change that a provider or a realm rule makes to roles:
/// the tenant's owner, as installing a bundle does.
pub(crate) fn owner_context(
    app: &App,
    tenant: &TenantId,
    correlation: &str,
) -> Result<Context, ApiError> {
    let state = app
        .store
        .load(tenant, &[])?
        .tenant
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such site"))?;
    let owner = state
        .members
        .iter()
        .find(|(_, roles)| roles.contains(&owner_role()))
        .map(|(principal, _)| principal.clone())
        .ok_or_else(|| ApiError::hidden(ErrorCode::Internal, "the site has no owner"))?;
    Ok(Context {
        tenant: tenant.clone(),
        principal: Principal::User(owner),
        session: None,
        now: app.now(),
        correlation: correlation.to_string(),
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
        agent: None,
    })
}

/// Whether a person was deactivated by their provider. A deactivated person signs in by no
/// method and is no subject of a token (FED-015).
pub fn is_disabled(
    app: &App,
    tenant: &TenantId,
    principal: &PrincipalId,
) -> Result<bool, ApiError> {
    Ok(app
        .store
        .doc_get(tenant, scim::DISABLED, principal.as_str())?
        .is_some())
}
