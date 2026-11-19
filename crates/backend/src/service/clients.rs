//! Service accounts: a caller that is itself, with a client secret.
//!
//! A client's secret is shown once, when it is made or rotated, and only a hash of it is kept
//! (TOK-252). Credential requests are counted per client, per source and per realm, with the
//! same limits and the same giving back of a right attempt as sign-in (TOK-253, D-700). A client
//! that is revoked is refused at once, and so is every token issued to it.
//!
//! Implements: TOK-001, TOK-252, TOK-253

use super::tokens::valid_scope;
use super::{get, list, put, require_owner, text};
use crate::app::{ApiError, App};
use crate::auth::{decoy_hash, hash_password, is_password, random_hex, Caller, Identity};
use crate::signin::{rules, throttled};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use strata_kernel::TenantId;
use strata_protocol::api::ErrorCode;
use strata_storage_api::{ThrottleSettle, ThrottleVerdict};

pub const KIND: &str = "client";
/// The longest client identifier that is looked up.
const ID_CHARS: usize = 96;
/// The most scopes one client holds.
const MAX_SCOPES: usize = 32;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Client {
    pub id: String,
    pub name: String,
    /// What the client may be issued tokens for, at most.
    pub scopes: Vec<String>,
    /// An Argon2id hash. The secret itself is kept nowhere.
    pub secret_hash: String,
    pub secret_set_at: i64,
    pub created_at: i64,
    /// Set when the client was revoked: nothing issued to it is accepted from then on.
    pub disabled_at: Option<i64>,
}

fn fresh_secret() -> Result<String, ApiError> {
    Ok(format!("scs_{}", random_hex(24)?))
}

fn summary(client: &Client) -> Json {
    json!({
        "id": client.id,
        "name": client.name,
        "scopes": client.scopes,
        "created_at": client.created_at,
        "secret_set_at": client.secret_set_at,
        "revoked": client.disabled_at.is_some(),
    })
}

/// Make a service account. Its secret is in the reply and nowhere else, ever (TOK-252).
pub fn create(app: &App, identity: &Identity, body: &Json) -> Result<Json, ApiError> {
    require_owner(app, identity)?;
    let name = text(body, "name", 80)?.to_string();
    let scopes: Vec<String> = body
        .get("scopes")
        .and_then(Json::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|scope| scope.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if scopes.is_empty() || scopes.len() > MAX_SCOPES {
        return Err(ApiError::new(
            ErrorCode::Invalid,
            "Name between one and 32 scopes.",
        ));
    }
    let tenant = crate::ops::header(app, identity)?;
    if let Some(bad) = scopes.iter().find(|scope| !valid_scope(&tenant, scope)) {
        return Err(ApiError::new(
            ErrorCode::Invalid,
            format!("{bad:?} is not a scope of this site."),
        ));
    }
    let secret = fresh_secret()?;
    let now = app.now().0;
    let client = Client {
        id: format!("svc:{}", random_hex(8)?),
        name,
        scopes,
        secret_hash: hash_password(&secret)?,
        secret_set_at: now,
        created_at: now,
        disabled_at: None,
    };
    put(app, &identity.tenant, KIND, &client.id, &client, true)?;
    app.audit(
        "client.created",
        &identity.correlation,
        json!({ "tenant": identity.tenant.as_str(), "client": client.id }),
    );
    let mut reply = summary(&client);
    reply["secret"] = json!(secret);
    Ok(reply)
}

/// The tenant's service accounts, without any secret.
pub fn all(app: &App, identity: &Identity) -> Result<Json, ApiError> {
    require_owner(app, identity)?;
    let clients: Vec<Client> = list(app, &identity.tenant, KIND)?;
    Ok(json!({ "clients": clients.iter().map(summary).collect::<Vec<_>>() }))
}

/// A new secret: the old one stops working at once. Tokens issued with the old one live out
/// their five minutes; a client whose old secret leaked is revoked instead.
pub fn rotate_secret(app: &App, identity: &Identity, body: &Json) -> Result<Json, ApiError> {
    require_owner(app, identity)?;
    let id = text(body, "id", ID_CHARS)?;
    let Some(mut client) = get::<Client>(app, &identity.tenant, KIND, id)? else {
        return Err(ApiError::new(
            ErrorCode::NotFound,
            "No such service account.",
        ));
    };
    if client.disabled_at.is_some() {
        return Err(ApiError::new(
            ErrorCode::Invalid,
            "That service account is revoked.",
        ));
    }
    let secret = fresh_secret()?;
    client.secret_hash = hash_password(&secret)?;
    client.secret_set_at = app.now().0;
    put(app, &identity.tenant, KIND, id, &client, false)?;
    app.audit(
        "client.secret_rotated",
        &identity.correlation,
        json!({ "tenant": identity.tenant.as_str(), "client": id }),
    );
    let mut reply = summary(&client);
    reply["secret"] = json!(secret);
    Ok(reply)
}

/// Revoke a service account because it, or its secret, is compromised: it signs in no more, and
/// every token issued to it is refused from this moment, whatever its expiry.
///
/// Implements: TOK-007
pub fn revoke(app: &App, identity: &Identity, body: &Json) -> Result<Json, ApiError> {
    require_owner(app, identity)?;
    let id = text(body, "id", ID_CHARS)?;
    let Some(mut client) = get::<Client>(app, &identity.tenant, KIND, id)? else {
        return Err(ApiError::new(
            ErrorCode::NotFound,
            "No such service account.",
        ));
    };
    if client.disabled_at.is_none() {
        client.disabled_at = Some(app.now().0);
        put(app, &identity.tenant, KIND, id, &client, false)?;
    }
    app.audit(
        "client.revoked",
        &identity.correlation,
        json!({ "tenant": identity.tenant.as_str(), "client": id }),
    );
    Ok(summary(&client))
}

/// A client by identifier, if it is one of the tenant's and has not been revoked.
pub fn live(app: &App, tenant: &TenantId, id: &str) -> Result<Option<Client>, ApiError> {
    if id.is_empty() || id.len() > ID_CHARS {
        return Ok(None);
    }
    Ok(get::<Client>(app, tenant, KIND, id)?.filter(|client| client.disabled_at.is_none()))
}

/// Check a client's credentials. The attempt is counted first and a right one gives its slot
/// back, as at sign-in; a client that does not exist costs the same as a wrong secret
/// (TOK-253, IDN-255). `Ok(None)` is the one refusal.
pub fn authenticate(
    app: &App,
    caller: &Caller,
    client_id: &str,
    secret: &str,
) -> Result<Option<Client>, ApiError> {
    let limits = rules(app, "token", client_id, &caller.source);
    if let ThrottleVerdict::Refused { .. } =
        app.store
            .throttle_reserve(&caller.site, &limits, app.now())?
    {
        return Err(throttled());
    }
    let client = live(app, &caller.site, client_id)?;
    let hash = client
        .as_ref()
        .map_or_else(|| decoy_hash().to_string(), |c| c.secret_hash.clone());
    let right = is_password(secret, &hash) && client.is_some();
    if !right {
        return Ok(None);
    }
    let [account_rule, source_rule, realm_rule] = &limits;
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
    Ok(client)
}
