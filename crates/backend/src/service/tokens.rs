//! Access tokens for service accounts and for agents acting for a person.
//!
//! A token is a signed JWT (ES256, the tenant's key, see `keys`) that lives five minutes at most.
//! It is made in two ways:
//!
//! - **Client credentials.** A service account presents its secret and gets a token for scopes
//!   its own grant holds, or a part of them (TOK-001, TOK-002).
//! - **Exchange.** An agent presents its credentials and the delegation (a *grant*) a person made
//!   to it, and gets a token that names the person as `sub` and the agent as `act.sub` and `azp`
//!   (TOK-003). It carries no scope the person does not hold when the grant is made, when the
//!   token is made, or when it is checked (TOK-004), and none beyond the grant's expiry.
//!
//! A scope is `action:object` (`read:note`), or `action:*` for every object of the site.
//!
//! **Verifying** a token checks the signature against the tenant's keys, then the claims, then
//! the records the token depends on, every time and in the store, so that a revocation is seen
//! by the very next check and no notice has to arrive for it to count (TOK-007). The bound that
//! applies to sessions is the five minutes of the token lifetime at the outside (D-537): a token
//! is refused after its expiry whatever else is true.
//!
//! Implements: TOK-001, TOK-002, TOK-003, TOK-004, TOK-005, TOK-006, TOK-007, TOK-009, TOK-250

use super::{clients, get, is_disabled, keys, person, put, require_owner, text};
use crate::app::{ApiError, App, LOG};
use crate::auth::{random_hex, Caller, Identity};
use jsonwebtoken::{
    decode, decode_header, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use strata_kernel::rules::may_attempt;
use strata_kernel::{Action, Principal, PrincipalId, Resource, TenantId, TenantState};
use strata_protocol::api::ErrorCode;

/// An access token is valid for at most five minutes (TOK-005, D-537).
pub const TOKEN_TTL_MS: i64 = 5 * 60_000;
/// The longest a delegation may be made for.
const GRANT_MAX_MS: i64 = 30 * 24 * 3_600_000;
pub const GRANT_KIND: &str = "grant";
const EXCHANGE: &str = "urn:ietf:params:oauth:grant-type:token-exchange";

/// The audience of a tenant's tokens: the tenant itself (TOK-006).
pub fn audience_of(tenant: &TenantId) -> String {
    format!("strata:{}", tenant.as_str())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Act {
    pub sub: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Claims {
    pub iss: String,
    pub aud: String,
    /// The principal the token speaks for: the service account, or the person for a delegation.
    pub sub: String,
    pub tid: String,
    /// The client the token was issued to, which is the only one that may present it (TOK-250).
    pub azp: String,
    pub scope: String,
    pub iat: i64,
    pub exp: i64,
    pub jti: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub act: Option<Act>,
}

/// A person's delegation to an agent.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Grant {
    pub id: String,
    pub principal: String,
    pub agent: String,
    pub scopes: Vec<String>,
    pub purpose: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub revoked_at: Option<i64>,
}

fn not_accepted() -> ApiError {
    ApiError::new(
        ErrorCode::Unauthenticated,
        "The credentials or the token were not accepted.",
    )
}

fn split(scope: &str) -> Option<(Action, &str)> {
    let (action, object) = scope.split_once(':')?;
    let action = match action {
        "read" => Action::Read,
        "create" => Action::Create,
        "update" => Action::Update,
        "delete" => Action::Delete,
        _ => return None,
    };
    Some((action, object))
}

/// Whether a scope is one this site has: a known action on `*` or on an object type it defines.
pub fn valid_scope(tenant: &TenantState, scope: &str) -> bool {
    match split(scope) {
        Some((_, "*")) => true,
        Some((_, object)) => tenant.schema.objects.keys().any(|k| k.as_str() == object),
        None => false,
    }
}

/// Whether a held scope covers a wanted one: the same, or the held one is the action on `*`.
fn covers(held: &str, wanted: &str) -> bool {
    if held == wanted {
        return true;
    }
    match (held.split_once(':'), wanted.split_once(':')) {
        (Some((a, "*")), Some((b, _))) => a == b && split(wanted).is_some(),
        _ => false,
    }
}

/// Whether the person may do what a scope says, now (TOK-004). `action:*` needs every object.
fn person_may(tenant: &TenantState, person: &PrincipalId, scope: &str) -> bool {
    let Some((action, object)) = split(scope) else {
        return false;
    };
    let who = Principal::User(person.clone());
    if object == "*" {
        return !tenant.schema.objects.is_empty()
            && tenant
                .schema
                .objects
                .keys()
                .all(|k| may_attempt(tenant, &who, action, &Resource::Object(k.clone())));
    }
    tenant
        .schema
        .objects
        .keys()
        .find(|k| k.as_str() == object)
        .is_some_and(|k| may_attempt(tenant, &who, action, &Resource::Object(k.clone())))
}

/// The scopes a request names: a space-separated text or a list. `None` when it names none.
fn scopes_of(body: &Json) -> Result<Option<Vec<String>>, ApiError> {
    let bad = || ApiError::new(ErrorCode::Invalid, "scope is not a list of scopes.");
    let list: Vec<String> = match body.get("scope") {
        None | Some(Json::Null) => return Ok(None),
        Some(Json::String(text)) => text.split_whitespace().map(str::to_string).collect(),
        Some(Json::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_string).ok_or_else(bad))
            .collect::<Result<_, _>>()?,
        Some(_) => return Err(bad()),
    };
    if list.len() > 32 || list.iter().any(|scope| scope.len() > 96) {
        return Err(bad());
    }
    Ok(Some(list))
}

fn site(app: &App, tenant: &TenantId) -> Result<std::sync::Arc<TenantState>, ApiError> {
    app.store
        .load(tenant, &[])?
        .tenant
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such site"))
}

/// Sign the claims with the tenant's active key (ES256).
///
/// Implements: H-013
fn sign(app: &App, tenant: &TenantId, claims: &Claims) -> Result<String, ApiError> {
    let key = keys::active(app, tenant)?;
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(key.kid.clone());
    let signer = EncodingKey::from_ec_pem(key.private_pem.as_bytes())
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, format!("signing key: {e}")))?;
    encode(&header, claims, &signer)
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, format!("signing: {e}")))
}

/// Make a token for `scopes`, valid for the lifetime or until `cap_ms`, whichever is first.
#[allow(clippy::too_many_arguments)]
fn mint(
    app: &App,
    tenant: &TenantId,
    sub: &str,
    azp: &str,
    gid: Option<String>,
    act: Option<Act>,
    scopes: &[String],
    cap_ms: Option<i64>,
) -> Result<Json, ApiError> {
    let now = app.now().0;
    let expires_ms = cap_ms.map_or(now + TOKEN_TTL_MS, |cap| cap.min(now + TOKEN_TTL_MS));
    // Seconds, rounded down: a token never lives longer than it was meant to.
    let exp = expires_ms / 1000;
    let iat = now / 1000;
    if exp <= iat {
        return Err(ApiError::new(ErrorCode::Invalid, "That expiry has passed."));
    }
    let claims = Claims {
        iss: audience_of(tenant),
        aud: audience_of(tenant),
        sub: sub.to_string(),
        tid: tenant.as_str().to_string(),
        azp: azp.to_string(),
        scope: scopes.join(" "),
        iat,
        exp,
        jti: random_hex(12)?,
        gid,
        act,
    };
    let token = sign(app, tenant, &claims)?;
    Ok(json!({
        "access_token": token,
        "token_type": "Bearer",
        "expires_in": exp - iat,
        "expires_at": exp * 1000,
        "scope": claims.scope,
    }))
}

/// The token endpoint: client credentials, or the exchange of a delegation. The client always
/// presents its credentials; they are counted and limited like a sign-in (TOK-253).
///
/// Implements: TOK-001, TOK-002, TOK-003, TOK-004, TOK-005, TOK-253
pub fn issue(app: &App, caller: &Caller, body: &Json) -> Result<Json, ApiError> {
    let grant_type = body
        .get("grant_type")
        .and_then(Json::as_str)
        .unwrap_or("client_credentials");
    let client_id = text(body, "client_id", 96)?;
    let secret = text(body, "client_secret", 256)?;
    let Some(client) = clients::authenticate(app, caller, client_id, secret)? else {
        LOG.warn(
            "token.refused",
            &caller.correlation,
            json!({ "tenant": caller.site.as_str(), "reason": "client" }),
        );
        return Err(not_accepted());
    };
    if let Some(audience) = body.get("audience").and_then(Json::as_str) {
        if audience != audience_of(&caller.site) {
            return Err(ApiError::new(
                ErrorCode::Invalid,
                "That audience is not this site.",
            ));
        }
    }
    let tenant = site(app, &caller.site)?;
    match grant_type {
        "client_credentials" => {
            let granted = match scopes_of(body)? {
                None => client.scopes.clone(),
                Some(wanted) => {
                    if wanted
                        .iter()
                        .any(|w| !client.scopes.iter().any(|held| covers(held, w)))
                    {
                        return Err(ApiError::new(
                            ErrorCode::Invalid,
                            "A scope asked for is beyond the client's own.",
                        ));
                    }
                    wanted
                }
            };
            if granted.is_empty() {
                return Err(ApiError::new(ErrorCode::Invalid, "Ask for a scope."));
            }
            mint(
                app,
                &caller.site,
                &client.id,
                &client.id,
                None,
                None,
                &granted,
                None,
            )
        }
        EXCHANGE | "token-exchange" => {
            let id = text(body, "grant", 96)?;
            let grant = get::<Grant>(app, &caller.site, GRANT_KIND, id)?
                .filter(|grant| grant.agent == client.id)
                .ok_or_else(not_accepted)?;
            if !grant_live(app, &caller.site, &grant)? {
                return Err(not_accepted());
            }
            let person = PrincipalId::new(grant.principal.clone())
                .map_err(|e| ApiError::hidden(ErrorCode::Internal, e.to_string()))?;
            // What the person may do now, within what they delegated.
            let held: Vec<String> = grant
                .scopes
                .iter()
                .filter(|scope| person_may(&tenant, &person, scope))
                .cloned()
                .collect();
            let granted = match scopes_of(body)? {
                None => held,
                Some(wanted) => {
                    if wanted.iter().any(|w| !held.iter().any(|h| covers(h, w))) {
                        return Err(ApiError::new(
                            ErrorCode::Invalid,
                            "A scope asked for is beyond what the person delegated and holds.",
                        ));
                    }
                    wanted
                }
            };
            if granted.is_empty() {
                return Err(ApiError::new(ErrorCode::Invalid, "Ask for a scope."));
            }
            let mut cap = grant.expires_at;
            if let Some(asked) = body.get("expires_at") {
                let asked = asked
                    .as_i64()
                    .ok_or_else(|| ApiError::new(ErrorCode::Invalid, "expires_at is a time."))?;
                if asked > grant.expires_at {
                    return Err(ApiError::new(
                        ErrorCode::Invalid,
                        "The expiry is later than the delegation's.",
                    ));
                }
                cap = cap.min(asked);
            }
            mint(
                app,
                &caller.site,
                &grant.principal,
                &client.id,
                Some(grant.id.clone()),
                Some(Act {
                    sub: client.id.clone(),
                }),
                &granted,
                Some(cap),
            )
        }
        _ => Err(ApiError::new(
            ErrorCode::Invalid,
            "That grant type is not offered.",
        )),
    }
}

/// Whether a delegation can still be used: not revoked, not past its end, and its person not
/// deactivated.
fn grant_live(app: &App, tenant: &TenantId, grant: &Grant) -> Result<bool, ApiError> {
    if grant.revoked_at.is_some() || app.now().0 >= grant.expires_at {
        return Ok(false);
    }
    match PrincipalId::new(grant.principal.clone()) {
        Ok(person) => Ok(!is_disabled(app, tenant, &person)?),
        Err(_) => Ok(false),
    }
}

/// A person delegates scopes to an agent, none of which they lack themselves (TOK-004).
pub fn create_grant(app: &App, identity: &Identity, body: &Json) -> Result<Json, ApiError> {
    let who = person(identity)?;
    let agent = text(body, "agent", 96)?;
    if clients::live(app, &identity.tenant, agent)?.is_none() {
        return Err(ApiError::new(ErrorCode::Invalid, "No such agent."));
    }
    let tenant = site(app, &identity.tenant)?;
    let scopes = scopes_of(body)?.unwrap_or_default();
    if scopes.is_empty() {
        return Err(ApiError::new(
            ErrorCode::Invalid,
            "Name a scope to delegate.",
        ));
    }
    if let Some(scope) = scopes
        .iter()
        .find(|scope| !valid_scope(&tenant, scope) || !person_may(&tenant, &who, scope))
    {
        return Err(ApiError::new(
            ErrorCode::Invalid,
            format!("You may not delegate {scope:?}."),
        ));
    }
    let now = app.now().0;
    let expires_at = match body.get("expires_at") {
        None | Some(Json::Null) => now + 3_600_000,
        Some(value) => value
            .as_i64()
            .ok_or_else(|| ApiError::new(ErrorCode::Invalid, "expires_at is a time."))?,
    };
    if expires_at <= now || expires_at > now + GRANT_MAX_MS {
        return Err(ApiError::new(
            ErrorCode::Invalid,
            "The delegation must end within 30 days from now.",
        ));
    }
    let grant = Grant {
        id: format!("grt_{}", random_hex(8)?),
        principal: who.as_str().to_string(),
        agent: agent.to_string(),
        scopes,
        purpose: body
            .get("purpose")
            .and_then(Json::as_str)
            .unwrap_or("")
            .chars()
            .take(200)
            .collect(),
        created_at: now,
        expires_at,
        revoked_at: None,
    };
    put(app, &identity.tenant, GRANT_KIND, &grant.id, &grant, true)?;
    Ok(json!({ "grant": grant.id, "expires_at": grant.expires_at, "scopes": grant.scopes }))
}

/// Revoke a delegation. The tokens made from it are refused from the next check (TOK-007).
/// Its person may, and so may an owner.
///
/// Implements: TOK-007
pub fn revoke_grant(app: &App, identity: &Identity, body: &Json) -> Result<Json, ApiError> {
    let who = person(identity)?;
    let id = text(body, "grant", 96)?;
    let Some(mut grant) = get::<Grant>(app, &identity.tenant, GRANT_KIND, id)? else {
        return Err(ApiError::new(ErrorCode::NotFound, "No such delegation."));
    };
    if grant.principal != who.as_str() && require_owner(app, identity).is_err() {
        // The answer does not say whether it exists.
        return Err(ApiError::new(ErrorCode::NotFound, "No such delegation."));
    }
    if grant.revoked_at.is_none() {
        grant.revoked_at = Some(app.now().0);
        put(app, &identity.tenant, GRANT_KIND, id, &grant, false)?;
    }
    app.audit(
        "grant.revoked",
        &identity.correlation,
        json!({ "tenant": identity.tenant.as_str(), "grant": id }),
    );
    Ok(json!({ "grant": id, "revoked": true }))
}

/// Why a token was refused. It is for the log, never for the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Malformed,
    Key,
    Signature,
    Audience,
    Expired,
    Lifetime,
    Client,
    Presenter,
    Grant,
    Subject,
}

/// A token that holds.
#[derive(Clone, Debug)]
pub struct Verified {
    pub claims: Claims,
    /// The scopes it grants now: for a delegation, those the person still holds.
    pub scopes: Vec<String>,
}

/// Check a token against this tenant. `presenter` is the client that presents it, when the
/// resource knows one: a token whose client is another is refused (TOK-250). The outer error is
/// the store failing; the inner is a refusal.
///
/// Implements: H-014, TOK-005, TOK-006, TOK-007, TOK-008, TOK-250, TOK-251
pub fn verify(
    app: &App,
    tenant: &TenantId,
    token: &str,
    presenter: Option<&str>,
) -> Result<Result<Verified, Refusal>, ApiError> {
    if token.len() > 4096 {
        return Ok(Err(Refusal::Malformed));
    }
    let Ok(header) = decode_header(token) else {
        return Ok(Err(Refusal::Malformed));
    };
    let Some(kid) = header.kid.filter(|_| header.alg == Algorithm::ES256) else {
        return Ok(Err(Refusal::Malformed));
    };
    let Some(key) = keys::verifying(app, tenant, &kid)? else {
        return Ok(Err(Refusal::Key));
    };
    let decoder = DecodingKey::from_ec_pem(key.public_pem.as_bytes())
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, format!("signing key: {e}")))?;
    // The library checks the signature and the method. The claims are checked here, against this
    // process's clock, which a test can move.
    let mut validation = Validation::new(Algorithm::ES256);
    validation.validate_exp = false;
    validation.validate_nbf = false;
    validation.validate_aud = false;
    validation.required_spec_claims.clear();
    let Ok(data) = decode::<Claims>(token, &decoder, &validation) else {
        return Ok(Err(Refusal::Signature));
    };
    let claims = data.claims;
    if claims.tid != tenant.as_str() || claims.aud != audience_of(tenant) {
        return Ok(Err(Refusal::Audience));
    }
    if claims.exp - claims.iat > TOKEN_TTL_MS / 1000 {
        return Ok(Err(Refusal::Lifetime));
    }
    if app.now().0 >= claims.exp * 1000 {
        return Ok(Err(Refusal::Expired));
    }
    if clients::live(app, tenant, &claims.azp)?.is_none() {
        return Ok(Err(Refusal::Client));
    }
    if presenter.is_some_and(|presenter| presenter != claims.azp) {
        return Ok(Err(Refusal::Presenter));
    }
    let Ok(subject) = PrincipalId::new(claims.sub.clone()) else {
        return Ok(Err(Refusal::Malformed));
    };
    if is_disabled(app, tenant, &subject)? {
        return Ok(Err(Refusal::Subject));
    }
    let mut scopes: Vec<String> = claims
        .scope
        .split_whitespace()
        .map(str::to_string)
        .collect();
    if let Some(gid) = &claims.gid {
        let Some(grant) = get::<Grant>(app, tenant, GRANT_KIND, gid)? else {
            return Ok(Err(Refusal::Grant));
        };
        if !grant_live(app, tenant, &grant)? {
            return Ok(Err(Refusal::Grant));
        }
        let state = site(app, tenant)?;
        scopes.retain(|scope| person_may(&state, &subject, scope));
    }
    Ok(Ok(Verified { claims, scopes }))
}

fn describe(verified: &Verified) -> Json {
    let claims = &verified.claims;
    json!({
        "active": true,
        "scope": verified.scopes.join(" "),
        "sub": claims.sub,
        "client_id": claims.azp,
        "act": claims.act,
        "tenant": claims.tid,
        "aud": claims.aud,
        "iat": claims.iat,
        "exp": claims.exp,
        "expires_at": claims.exp * 1000,
        "grant": claims.gid,
    })
}

/// A resource's check of a token it was handed: the answer is the token's meaning, or the one
/// refusal.
pub fn verify_for_resource(app: &App, caller: &Caller, body: &Json) -> Result<Json, ApiError> {
    let token = text(body, "token", 4096)?;
    let presenter = body.get("presenter").and_then(Json::as_str);
    match verify(app, &caller.site, token, presenter)? {
        Ok(verified) => Ok(describe(&verified)),
        Err(reason) => {
            LOG.warn(
                "token.refused",
                &caller.correlation,
                json!({ "tenant": caller.site.as_str(), "reason": format!("{reason:?}") }),
            );
            Err(not_accepted())
        }
    }
}

/// Introspection: the scopes, the subject and the expiry of a token, to a caller permitted to
/// ask: a service account asking about its own tokens, or an owner of the site. A token that
/// does not hold is `{"active": false}`, and says nothing about why (TOK-009).
///
/// Implements: TOK-009
pub fn introspect(
    app: &App,
    caller: &Caller,
    identity: &Identity,
    body: &Json,
) -> Result<Json, ApiError> {
    // Who asks is decided before the token is looked at: a caller with no credential is told it
    // is unauthenticated, not that its token is missing (contract introspect-token-over-http).
    let asker = if body.get("client_id").is_some() {
        let id = text(body, "client_id", 96)?;
        let secret = text(body, "client_secret", 256)?;
        match clients::authenticate(app, caller, id, secret)? {
            Some(client) => Some(client.id),
            None => return Err(not_accepted()),
        }
    } else {
        require_owner(app, identity)?;
        None
    };
    let token = text(body, "token", 4096)?;
    Ok(match verify(app, &caller.site, token, None)? {
        Ok(verified)
            if asker
                .as_ref()
                .is_none_or(|asker| *asker == verified.claims.azp) =>
        {
            describe(&verified)
        }
        _ => json!({ "active": false }),
    })
}
