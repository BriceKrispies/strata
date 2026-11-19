//! Who is calling: service token, site, session, and the login and logout flows.
//!
//! Randomness and password hashing happen here, outside the kernel. The kernel only ever sees
//! the resulting `Principal` and the session's public id.

use crate::app::{ApiError, App, Shared, LOG};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::http::HeaderMap;
use serde_json::json;
use std::collections::BTreeMap;
use strata_kernel::rules::{may_attempt, roles_of};
use strata_kernel::{
    canon, Action, Context, Principal, PrincipalId, Resource, SessionId, TenantId, TenantState,
};
use strata_protocol::api::{
    valid_correlation, ErrorCode, LoginReply, LoginRequest, SessionView, HEADER_CORRELATION,
    HEADER_SERVICE, HEADER_SESSION, HEADER_SITE,
};
use strata_protocol::ws::SessionState;
use strata_storage_api::{Account, SessionRecord};

/// What the BFF asserted about a request.
#[derive(Clone, Debug)]
pub struct Caller {
    pub site: TenantId,
    pub token: Option<String>,
    pub correlation: String,
}

/// Who a request acts as, after the session has been checked.
#[derive(Clone, Debug)]
pub struct Identity {
    pub tenant: TenantId,
    pub principal: Principal,
    pub session: Option<SessionRecord>,
    /// Why there is no session, when a token was presented but did not hold.
    pub state: SessionState,
    pub correlation: String,
}

impl Identity {
    pub fn anonymous(&self) -> bool {
        self.session.is_none()
    }

    pub fn context(&self, app: &App) -> Context {
        Context {
            tenant: self.tenant.clone(),
            principal: self.principal.clone(),
            session: self.session.as_ref().map(|s| s.id.clone()),
            now: app.now(),
            correlation: self.correlation.clone(),
        }
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
}

/// Compare two secrets without stopping at the first difference.
pub use strata_runtime::routes::same_secret;

pub fn has_service_token(app: &App, headers: &HeaderMap) -> bool {
    header(headers, HEADER_SERVICE).is_some_and(|given| same_secret(given, &app.cfg.service_token))
}

pub fn random_hex(bytes: usize) -> Result<String, ApiError> {
    let mut buffer = vec![0u8; bytes];
    getrandom::getrandom(&mut buffer)
        .map_err(|e| ApiError::new(ErrorCode::Unavailable, format!("no randomness: {e}")))?;
    Ok(buffer.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn caller(headers: &HeaderMap) -> Result<Caller, ApiError> {
    let correlation = header(headers, HEADER_CORRELATION)
        .filter(|id| valid_correlation(id))
        .map(str::to_string)
        .unwrap_or_else(|| format!("b-{}", random_hex(6).unwrap_or_default()));
    let site = header(headers, HEADER_SITE)
        .and_then(|site| TenantId::new(site).ok())
        .ok_or_else(|| {
            ApiError::new(ErrorCode::Invalid, "missing site").with_correlation(&correlation)
        })?;
    Ok(Caller {
        site,
        token: header(headers, HEADER_SESSION).map(str::to_string),
        correlation,
    })
}

pub fn token_hash(token: &str) -> String {
    canon::hash_bytes(token.as_bytes())
}

/// Whether a stored session is usable at `now`.
pub fn session_state(app: &App, session: &SessionRecord, now: i64) -> SessionState {
    if session.revoked_at.is_some() {
        SessionState::Revoked
    } else if now >= session.expires_at.0 || now - session.last_seen_at.0 >= app.cfg.session_idle_ms
    {
        SessionState::Expired
    } else {
        SessionState::Active
    }
}

/// Resolve a caller to an identity. Blocking: reads the store.
///
/// A token that does not hold (unknown, ended, or issued for another tenant than the site being
/// served) yields the anonymous identity, never an error and never another tenant's identity.
pub fn identify(app: &App, caller: &Caller) -> Result<Identity, ApiError> {
    let anonymous = |state| Identity {
        tenant: caller.site.clone(),
        principal: Principal::Anonymous,
        session: None,
        state,
        correlation: caller.correlation.clone(),
    };
    let Some(token) = &caller.token else {
        return Ok(anonymous(SessionState::Anonymous));
    };
    let Some(mut session) = app.store.session_by_token(&token_hash(token))? else {
        return Ok(anonymous(SessionState::Expired));
    };
    let now = app.now().0;
    if session.tenant != caller.site {
        LOG.warn(
            "session.wrong_site",
            &caller.correlation,
            json!({ "session": session.id.as_str(), "site": caller.site.as_str() }),
        );
        return Ok(anonymous(SessionState::Anonymous));
    }
    match session_state(app, &session, now) {
        SessionState::Active => {}
        ended => return Ok(anonymous(ended)),
    }
    // Sliding idle timeout, written at most once a minute.
    if now - session.last_seen_at.0 > 60_000 {
        session.last_seen_at = app.now();
        app.store.put_session(&session)?;
    }
    Ok(Identity {
        tenant: session.tenant.clone(),
        principal: Principal::User(session.principal.clone()),
        session: Some(session),
        state: SessionState::Active,
        correlation: caller.correlation.clone(),
    })
}

/// What the browser may know about an identity.
pub fn session_view(tenant: &TenantState, identity: &Identity) -> SessionView {
    let roles = roles_of(tenant, &identity.principal);
    let mut may = BTreeMap::new();
    for object in tenant.schema.objects.keys() {
        let resource = Resource::Object(object.clone());
        let actions: Vec<String> = [Action::Read, Action::Create, Action::Update, Action::Delete]
            .into_iter()
            .filter(|action| may_attempt(tenant, &identity.principal, *action, &resource))
            .map(|action| format!("{action:?}").to_lowercase())
            .collect();
        may.insert(object.as_str().to_string(), actions);
    }
    let session = identity.session.as_ref();
    SessionView {
        authenticated: session.is_some(),
        tenant: tenant.id.as_str().to_string(),
        tenant_name: tenant.name.clone(),
        session_id: session.map(|s| s.id.as_str().to_string()),
        principal: session.map(|s| s.principal.as_str().to_string()),
        display_name: session.map(|s| s.display_name.clone()),
        roles: roles.iter().map(|role| role.as_str().to_string()).collect(),
        csrf_token: session.map(|s| s.csrf_token.clone()),
        expires_at: session.map(|s| s.expires_at.0),
        may,
        may_manage_schema: may_attempt(
            tenant,
            &identity.principal,
            Action::Manage,
            &Resource::Schema,
        ),
    }
}

pub fn hash_password(password: &str) -> Result<String, ApiError> {
    let mut salt = [0u8; 16];
    getrandom::getrandom(&mut salt)
        .map_err(|e| ApiError::new(ErrorCode::Unavailable, e.to_string()))?;
    let salt = SaltString::encode_b64(&salt)
        .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;
    // `Argon2::default()` is Argon2id, version 19.
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))
}

fn verify_password(password: &str, stored: &str) -> bool {
    PasswordHash::new(stored)
        .map(|hash| {
            Argon2::default()
                .verify_password(password.as_bytes(), &hash)
                .is_ok()
        })
        .unwrap_or(false)
}

/// A hash to verify against when the login does not exist, so that a miss costs the same as a
/// wrong password.
fn decoy_hash() -> &'static str {
    static DECOY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DECOY.get_or_init(|| hash_password("decoy-password").unwrap_or_default())
}

const MAX_FAILURES: u32 = 5;
const LOCKOUT_MS: i64 = 30_000;

/// Check a login and start a session. Any session the caller already holds is ended, so the
/// session identifier changes on every authentication.
pub fn login(app: &App, caller: &Caller, request: &LoginRequest) -> Result<LoginReply, ApiError> {
    let now = app.now();
    let refused = || {
        ApiError::new(
            ErrorCode::Unauthenticated,
            "The name or password is not right.",
        )
    };
    let login = request.login.trim().to_lowercase();
    if login.is_empty() || login.len() > 64 || request.password.len() > 256 {
        return Err(refused());
    }
    let key = format!("{}/{login}", caller.site);
    if let Ok(throttle) = app.throttle.lock() {
        if throttle
            .get(&key)
            .is_some_and(|(count, until)| *count >= MAX_FAILURES && now.0 < *until)
        {
            return Err(ApiError::new(
                ErrorCode::Throttled,
                "Too many attempts. Wait half a minute and try again.",
            ));
        }
    }

    let Some(tenant) = app.store.load(&caller.site, &[])?.tenant else {
        return Err(ApiError::new(ErrorCode::NotFound, "no such site"));
    };
    let account: Option<Account> = app.store.account(&caller.site, &login)?;
    let verified = match &account {
        Some(account) => verify_password(&request.password, &account.password_hash),
        None => {
            verify_password(&request.password, decoy_hash());
            false
        }
    };
    let Some(account) = account.filter(|_| verified) else {
        if let Ok(mut throttle) = app.throttle.lock() {
            let entry = throttle.entry(key).or_insert((0, 0));
            entry.0 = if now.0 >= entry.1 && entry.0 >= MAX_FAILURES {
                1
            } else {
                entry.0 + 1
            };
            entry.1 = now.0 + LOCKOUT_MS;
        }
        LOG.warn(
            "login.refused",
            &caller.correlation,
            json!({ "tenant": caller.site.as_str() }),
        );
        return Err(refused());
    };
    if let Ok(mut throttle) = app.throttle.lock() {
        throttle.remove(&key);
    }

    if let Some(previous) = &caller.token {
        end_session(app, previous, &caller.correlation)?;
    }
    let token = random_hex(32)?;
    let session = SessionRecord {
        id: SessionId::new(format!("s_{}", random_hex(8)?))
            .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?,
        token_hash: token_hash(&token),
        csrf_token: random_hex(24)?,
        tenant: caller.site.clone(),
        principal: account.principal.clone(),
        display_name: account.display_name.clone(),
        created_at: now,
        last_seen_at: now,
        expires_at: strata_kernel::Timestamp(now.0 + app.cfg.session_ttl_ms),
        revoked_at: None,
    };
    app.store.put_session(&session)?;
    LOG.info(
        "login.accepted",
        &caller.correlation,
        json!({ "tenant": caller.site.as_str(), "session": session.id.as_str(), "principal": session.principal.as_str() }),
    );
    let identity = Identity {
        tenant: caller.site.clone(),
        principal: Principal::User(account.principal),
        session: Some(session),
        state: SessionState::Active,
        correlation: caller.correlation.clone(),
    };
    Ok(LoginReply {
        token,
        session: session_view(&tenant, &identity),
    })
}

/// End the session behind a token, if there is one, and tell its connections.
pub fn end_session(app: &App, token: &str, correlation: &str) -> Result<(), ApiError> {
    if let Some(mut session) = app.store.session_by_token(&token_hash(token))? {
        if session.revoked_at.is_none() {
            session.revoked_at = Some(app.now());
            app.store.put_session(&session)?;
            LOG.info(
                "session.ended",
                correlation,
                json!({ "session": session.id.as_str() }),
            );
        }
        let _ = app.hub.sessions.send((session.id, SessionState::Revoked));
    }
    Ok(())
}

pub async fn identify_async(app: &Shared, caller: Caller) -> Result<Identity, ApiError> {
    let correlation = caller.correlation.clone();
    app.blocking(move |app| identify(app, &caller))
        .await?
        .map_err(|e| e.with_correlation(&correlation))
}

/// Create or replace a development login. Used by seeding only.
pub fn put_dev_account(
    app: &App,
    tenant: &TenantId,
    login: &str,
    password: &str,
    principal: PrincipalId,
    display_name: &str,
) -> Result<(), ApiError> {
    app.store.put_account(&Account {
        tenant: tenant.clone(),
        login: login.to_lowercase(),
        principal,
        display_name: display_name.to_string(),
        password_hash: hash_password(password)?,
    })?;
    Ok(())
}
