//! Who is calling: service token, site, session, and the login and logout flows.
//!
//! Randomness and password hashing happen here, outside the kernel. The kernel only ever sees
//! the resulting `Principal` and the session's public id.

use crate::app::{ApiError, App, Shared};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::http::HeaderMap;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use strata_kernel::rules::{may_attempt, roles_of};
use strata_kernel::{
    canon, Action, Context, Principal, PrincipalId, Resource, SessionId, TenantId, TenantState,
};
use strata_model::telemetry;
use strata_protocol::api::{
    valid_correlation, valid_idempotency_key, ErrorCode, LoginReply, SessionView,
    HEADER_CORRELATION, HEADER_IDEMPOTENCY, HEADER_ORIGIN, HEADER_SERVICE, HEADER_SESSION,
    HEADER_SITE, HEADER_SOURCE,
};
use strata_protocol::ws::SessionState;
use strata_storage_api::{Account, SessionRecord, StoreError};

/// What the BFF asserted about a request.
#[derive(Clone, Debug)]
pub struct Caller {
    pub site: TenantId,
    pub token: Option<String>,
    pub correlation: String,
    /// The address of the browser as the BFF saw it, or `unknown` when it did not say.
    pub source: String,
    /// The origin the browser made the request from, as the BFF checked it, or `None` when the
    /// request did not come from one of the site's own pages.
    pub origin: Option<String>,
    /// The caller's key for the change this request makes, as the BFF passed it on, already checked
    /// by [`caller`]: a repeat with the same key changes the tenant once (OPS-318).
    pub idempotency_key: Option<String>,
}

impl Caller {
    /// The visitor's address as the BFF established it, for the limits on what one source may
    /// open or attempt; `None` when the BFF stated none, and such calls are limited together as
    /// one shared source.
    pub fn visitor_source(&self) -> Option<&str> {
        (self.source != "unknown").then_some(self.source.as_str())
    }
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
    /// The address the signed-in account has verified, from the store: never from a request.
    pub verified_address: Option<String>,
    /// The caller's idempotency key. [`Identity::context`] hands it to the store for a signed-in
    /// caller only.
    pub idempotency_key: Option<String>,
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
            verified_address: self.verified_address.clone(),
            // Only a signed-in caller's key reaches the store: every anonymous caller is the same
            // principal, so a key of theirs would be shared.
            idempotency_key: self.idempotency_key.clone().filter(|_| !self.anonymous()),
            language: None,
            hop: None,
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

/// Implements: H-003
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
    // The one place the key's format is checked: every route that changes state takes the key from
    // the caller, and a key that cannot be used is refused rather than ignored.
    let idempotency_key = match headers.get(HEADER_IDEMPOTENCY) {
        None => None,
        Some(value) => Some(
            value
                .to_str()
                .ok()
                .filter(|key| valid_idempotency_key(key))
                .map(str::to_string)
                .ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::Invalid,
                        "An idempotency key is 1 to 128 visible ASCII characters.",
                    )
                    .with_correlation(&correlation)
                })?,
        ),
    };
    Ok(Caller {
        idempotency_key,
        site,
        token: header(headers, HEADER_SESSION).map(str::to_string),
        source: header(headers, HEADER_SOURCE)
            .filter(|source| valid_source(source))
            .unwrap_or("unknown")
            .to_string(),
        origin: header(headers, HEADER_ORIGIN)
            .filter(|origin| origin.len() <= 255)
            .map(str::to_string),
        correlation,
    })
}

/// A source address as the BFF writes it: an IPv4 or IPv6 address, nothing else.
fn valid_source(source: &str) -> bool {
    source.len() <= 45
        && source
            .chars()
            .all(|c| c.is_ascii_hexdigit() || matches!(c, '.' | ':'))
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

/// The refusal of a request that needs a session while sessions cannot be looked up. It names the
/// identity outage, so that the person is told why and not that something went wrong (IDN-300).
///
/// Implements: IDN-300
pub fn identity_outage() -> ApiError {
    let mut error = ApiError::new(
        ErrorCode::Unavailable,
        "Signing in and signed-in requests are not available right now: the identity service cannot be reached. Published pages are still served.",
    );
    error.0.kind = Some("identity_outage".into());
    error
}

/// Whether an error is the identity outage.
pub fn is_identity_outage(error: &ApiError) -> bool {
    error.0.kind.as_deref() == Some("identity_outage")
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
        verified_address: None,
        idempotency_key: caller.idempotency_key.clone(),
    };
    let Some(token) = &caller.token else {
        return Ok(anonymous(SessionState::Anonymous));
    };
    // A session that cannot be looked up cannot be checked, so it is not guessed at (IDN-300).
    if app.identity_outage.load(Ordering::Relaxed) {
        return Err(identity_outage());
    }
    let found = match app.store.session_by_token(&token_hash(token)) {
        Ok(found) => found,
        Err(StoreError::Unavailable(_)) => return Err(identity_outage()),
        Err(other) => return Err(other.into()),
    };
    let Some(mut session) = found else {
        return Ok(anonymous(SessionState::Expired));
    };
    let now = app.now().0;
    if session.tenant != caller.site {
        // A token is only ever sent to the site that issued it, so this is a token that was
        // taken somewhere and replayed: an audit entry, not just a log line.
        app.audit(
            "session.wrong_site",
            &caller.correlation,
            json!({
                "session": session.id.as_str(),
                "session_tenant": session.tenant.as_str(),
                "site": caller.site.as_str(),
            }),
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
        app.store.session_seen(&session.id, session.last_seen_at)?;
    }
    let verified_address = app
        .store
        .verified_address(&session.tenant, &session.principal)?;
    Ok(Identity {
        tenant: session.tenant.clone(),
        principal: Principal::User(session.principal.clone()),
        session: Some(session),
        state: SessionState::Active,
        correlation: caller.correlation.clone(),
        verified_address,
        idempotency_key: caller.idempotency_key.clone(),
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

/// Implements: H-001
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

/// Implements: H-002
fn verify_password(password: &str, stored: &str) -> bool {
    PasswordHash::new(stored)
        .map(|hash| {
            Argon2::default()
                .verify_password(password.as_bytes(), &hash)
                .is_ok()
        })
        .unwrap_or(false)
}

/// Whether `password` opens the stored hash.
pub(crate) fn is_password(password: &str, stored: &str) -> bool {
    if PasswordHash::new(stored).is_err() {
        // An account no password opens (empty or unreadable hash) costs the same work as a wrong
        // password, so that the time does not say which accounts have no password.
        verify_password(password, decoy_hash());
        return false;
    }
    verify_password(password, stored)
}

/// A hash to verify against when the login does not exist, so that a miss costs the same as a
/// wrong password (IDN-255).
pub(crate) fn decoy_hash() -> &'static str {
    static DECOY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DECOY.get_or_init(|| hash_password("decoy-password").unwrap_or_default())
}

/// Check a login for a method and start a session. Any session the caller already holds is
/// ended, so the session identifier and its token change on every authentication (IDN-011).
pub fn login(
    app: &App,
    caller: &Caller,
    method: &str,
    request: &serde_json::Value,
) -> Result<crate::signin::SignInOutcome, ApiError> {
    crate::signin::sign_in(app, caller, method, request)
}

/// The network context a request comes from, as a keyed hash: the key is the service token, so a
/// copy of a session record holds no address and cannot be matched to one without it (D-704,
/// PRV-364). `None` when the gateway did not say where the request came from.
pub fn network_hash(app: &App, caller: &Caller) -> Option<String> {
    (caller.source != "unknown").then(|| {
        canon::hash_bytes(
            format!("network\0{}\0{}", app.cfg.service_token, caller.source).as_bytes(),
        )
    })
}

/// Start the session of an account that a sign-in method has accepted.
pub(crate) fn start_session(
    app: &App,
    caller: &Caller,
    tenant: &TenantState,
    account: Account,
    method: &str,
) -> Result<LoginReply, ApiError> {
    let now = app.now();
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
        authenticated_at: now,
        network: network_hash(app, caller),
    };
    app.store.put_session(&session)?;
    telemetry::session_started(
        &telemetry::BACKEND,
        &telemetry::SessionStarted {
            correlation: &caller.correlation,
            tenant: caller.site.as_str(),
            session: session.id.as_str(),
            principal: session.principal.as_str(),
            method,
        },
    );
    let identity = Identity {
        tenant: caller.site.clone(),
        principal: Principal::User(account.principal),
        session: Some(session),
        state: SessionState::Active,
        correlation: caller.correlation.clone(),
        verified_address: account.verified_email,
        idempotency_key: None,
    };
    Ok(LoginReply {
        token,
        session: session_view(tenant, &identity),
    })
}

/// End one session: mark it revoked, log it, and tell the connections it has open, which close.
/// When it had ended already, only the notice is sent again.
pub fn revoke(app: &App, mut session: SessionRecord, correlation: &str) -> Result<(), ApiError> {
    if session.revoked_at.is_none() {
        session.revoked_at = Some(app.now());
        app.store.put_session(&session)?;
        log_end(&session, correlation);
    }
    let _ = app.hub.sessions.send((session.id, SessionState::Revoked));
    Ok(())
}

/// The log line of a session that ended.
fn log_end(session: &SessionRecord, correlation: &str) {
    telemetry::session_ended(
        &telemetry::BACKEND,
        &telemetry::SessionEnded {
            correlation,
            tenant: session.tenant.as_str(),
            session: session.id.as_str(),
        },
    );
}

/// End the session behind a token, if there is one, and tell its connections (IDN-263).
pub fn end_session(app: &App, token: &str, correlation: &str) -> Result<(), ApiError> {
    if let Some(session) = app.store.session_by_token(&token_hash(token))? {
        revoke(app, session, correlation)?;
    }
    Ok(())
}

fn sign_in_needed() -> ApiError {
    ApiError::new(ErrorCode::Unauthenticated, "Sign in to do this.")
}

/// Every live session of the person a session belongs to, in the same site.
fn own_live_sessions(app: &App, current: &SessionRecord) -> Result<Vec<SessionRecord>, ApiError> {
    let now = app.now().0;
    Ok(app
        .store
        .live_sessions_of(&current.tenant, Some(&current.principal), app.now())?
        .into_iter()
        .filter(|s| session_state(app, s, now) == SessionState::Active)
        .collect())
}

/// The sessions of the caller that are still live, newest first, the caller's own marked
/// (IDN-012). Nothing of a session's token is in the answer.
pub fn list_sessions(
    app: &App,
    identity: &Identity,
) -> Result<Vec<strata_model::types::SessionSummary>, ApiError> {
    let Some(current) = &identity.session else {
        return Err(sign_in_needed());
    };
    let mut own = own_live_sessions(app, current)?;
    own.sort_by_key(|s| std::cmp::Reverse(s.created_at.0));
    Ok(own
        .into_iter()
        .map(|s| strata_model::types::SessionSummary {
            current: s.id == current.id,
            id: strata_model::types::SessionId(s.id.as_str().to_string()),
            created_at: strata_model::prelude::Timestamp(s.created_at.0),
            last_seen_at: strata_model::prelude::Timestamp(s.last_seen_at.0),
            expires_at: strata_model::prelude::Timestamp(s.expires_at.0),
        })
        .collect())
}

/// Revoke one of the caller's own sessions by its public id. A session that is not the caller's,
/// or does not exist, is answered as not found: the answer does not say which.
pub fn revoke_own_session(app: &App, identity: &Identity, id: &str) -> Result<(), ApiError> {
    let Some(current) = &identity.session else {
        return Err(sign_in_needed());
    };
    let not_found = || ApiError::new(ErrorCode::NotFound, "No such session.");
    let id = SessionId::new(id).map_err(|_| not_found())?;
    let target = app.store.session(&id)?.ok_or_else(not_found)?;
    if target.tenant != current.tenant || target.principal != current.principal {
        return Err(not_found());
    }
    revoke(app, target, &identity.correlation)
}

/// End every live session of a person in a tenant. Returns how many (IDN-020).
pub fn revoke_all_of(
    app: &App,
    tenant: &TenantId,
    principal: &PrincipalId,
    correlation: &str,
) -> Result<usize, ApiError> {
    let now = app.now().0;
    let live: Vec<SessionRecord> = app
        .store
        .live_sessions_of(tenant, Some(principal), app.now())?
        .into_iter()
        .filter(|s| session_state(app, s, now) == SessionState::Active)
        .collect();
    let count = live.len();
    for session in live {
        revoke(app, session, correlation)?;
    }
    Ok(count)
}

/// Revoke every live session of the caller but the one making the request. Returns how many.
pub fn revoke_other_sessions(app: &App, identity: &Identity) -> Result<usize, ApiError> {
    let Some(current) = &identity.session else {
        return Err(sign_in_needed());
    };
    let others: Vec<SessionRecord> = own_live_sessions(app, current)?
        .into_iter()
        .filter(|s| s.id != current.id)
        .collect();
    let count = others.len();
    for session in others {
        revoke(app, session, &identity.correlation)?;
    }
    Ok(count)
}

/// Tell the world about sessions that a commit ended in the store, such as those of a person
/// whose roles changed: log each, and close its connections. `sessions` were live before the
/// commit.
pub fn announce_ended(app: &App, sessions: Vec<SessionRecord>, correlation: &str) {
    for session in sessions {
        match app.store.session(&session.id) {
            Ok(Some(now)) if now.revoked_at.is_none() => {
                let _ = revoke(app, now, correlation);
            }
            _ => {
                log_end(&session, correlation);
                let _ = app.hub.sessions.send((session.id, SessionState::Revoked));
            }
        }
    }
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
    verified_email: Option<&str>,
) -> Result<(), ApiError> {
    app.store.put_account(&Account {
        tenant: tenant.clone(),
        login: crate::signin::normalise_login(login),
        principal,
        display_name: display_name.to_string(),
        password_hash: hash_password(password)?,
        verified_email: verified_email.map(str::to_string),
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: H-001
    #[test]
    fn a_password_hash_is_argon2id_with_a_fresh_salt_each_time() {
        let first = hash_password("correct horse").unwrap();
        let second = hash_password("correct horse").unwrap();
        assert!(first.starts_with("$argon2id$"), "{first}");
        assert_ne!(first, second, "two hashes of one password differ");
        let parsed = PasswordHash::new(&first).expect("PHC string form");
        assert!(parsed.salt.is_some());
    }

    /// Verifies: H-002
    #[test]
    fn a_password_matches_exactly_its_own_hash_and_a_bad_hash_is_no_match() {
        let stored = hash_password("correct horse").unwrap();
        assert!(verify_password("correct horse", &stored));
        assert!(!verify_password("correct horse ", &stored));
        assert!(!verify_password("Correct horse", &stored));
        assert!(!verify_password(
            "correct horse",
            &hash_password("other").unwrap()
        ));
        for junk in [
            "",
            "plain text",
            "$argon2id$broken",
            "$argon2id$v=19$m=1,t=1,p=1$$",
        ] {
            assert!(!verify_password("correct horse", junk), "{junk:?}");
        }
    }

    /// IDN-255: an account that no password opens costs the work of a wrong one, and is never
    /// opened, not even by the password the decoy hash was made from.
    #[test]
    fn an_account_with_no_usable_hash_is_checked_against_the_decoy_and_never_opens() {
        assert!(
            PasswordHash::new(decoy_hash()).is_ok(),
            "the decoy is a real hash"
        );
        for stored in ["", "plain text", "$argon2id$broken"] {
            assert!(!is_password("decoy-password", stored), "{stored:?}");
            assert!(!is_password("", stored), "{stored:?}");
        }
    }

    /// Verifies: H-003
    #[test]
    fn a_random_secret_is_lower_case_hex_of_the_requested_length_and_never_repeats() {
        for bytes in [0usize, 1, 6, 32] {
            let secret = random_hex(bytes).unwrap();
            assert_eq!(secret.len(), bytes * 2);
            assert!(secret
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
        }
        let drawn: std::collections::BTreeSet<String> =
            (0..50).map(|_| random_hex(16).unwrap()).collect();
        assert_eq!(
            drawn.len(),
            50,
            "a secret drawn for one challenge opens no other"
        );
    }
}
