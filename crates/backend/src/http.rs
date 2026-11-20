//! The internal HTTP API: routes, the service-token gate and request logging.

use crate::app::{ApiError, App, Shared, LOG};
use crate::auth::{self, has_service_token, Caller, Identity};
use crate::service::{self, clients, keys, scim, tokens};
use crate::signin::{
    link, oidc, passkey, recover, recovery_codes, saml, second, stepup, totp, SignInOutcome,
};
use crate::{lab, ops, provision, ws};
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Extension, Path, Request, State};
use axum::http::HeaderMap;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use serde_json::{json, Value as Json_};
use std::time::Instant;
use strata_protocol::api::{
    valid_idempotency_key, Empty, ErrorCode, IdRequest, MembersRequest, Mutation, SessionReply,
    ViewRequest, HEADER_CORRELATION, HEADER_IDEMPOTENCY,
};
use strata_protocol::ws::MAX_FRAME;

/// Run blocking work as the identified caller and wrap the result as JSON.
pub(crate) async fn as_caller<T, F>(
    app: &Shared,
    headers: &HeaderMap,
    work: F,
) -> Result<Json<T>, ApiError>
where
    T: Serialize + Send + 'static,
    F: FnOnce(&App, &Identity) -> Result<T, ApiError> + Send + 'static,
{
    let caller = auth::caller(headers)?;
    let correlation = caller.correlation.clone();
    let tenant = caller.site.as_str().to_string();
    app.blocking_for(&tenant, move |app| {
        let identity = auth::identify(app, &caller)?;
        work(app, &identity)
    })
    .await?
    .map(Json)
    .map_err(|error| error.with_correlation(&correlation))
}

/// Run blocking work as the identified caller, with what the caller asserted as well.
pub(crate) async fn as_caller_at<T, F>(
    app: &Shared,
    headers: &HeaderMap,
    work: F,
) -> Result<Json<T>, ApiError>
where
    T: Serialize + Send + 'static,
    F: FnOnce(&App, &Caller, &Identity) -> Result<T, ApiError> + Send + 'static,
{
    as_caller_at_hashing(app, headers, false, work).await
}

/// [`as_caller_at`] for work that may verify a secret with Argon2 (`hashes`): the request is
/// counted against the per-source sign-in rate and holds a place to hash for the work.
async fn as_caller_at_hashing<T, F>(
    app: &Shared,
    headers: &HeaderMap,
    hashes: bool,
    work: F,
) -> Result<Json<T>, ApiError>
where
    T: Serialize + Send + 'static,
    F: FnOnce(&App, &Caller, &Identity) -> Result<T, ApiError> + Send + 'static,
{
    let caller = auth::caller(headers)?;
    let correlation = caller.correlation.clone();
    let place = if hashes {
        Some(hashing_place(app, &caller).await?)
    } else {
        None
    };
    let tenant = caller.site.as_str().to_string();
    app.blocking_for(&tenant, move |app| {
        let _place = place;
        let identity = auth::identify(app, &caller)?;
        work(app, &caller, &identity)
    })
    .await?
    .map(Json)
    .map_err(|error| error.with_correlation(&correlation))
}

async fn health(State(app): State<Shared>) -> Result<Json<Json_>, ApiError> {
    // Liveness only: it opens no tenant file. The integrity check is in the platform
    // diagnostics, behind the service credential.
    app.blocking(|app| app.store.liveness()).await??;
    // The publisher moves committed events to the sockets; if its task has stopped, nothing is
    // published, so the backend is not well. Before it is started there is nothing to report.
    let publisher = app.hub.outbox.get().map(|outbox| outbox.status());
    let publisher_ok = publisher
        .as_ref()
        .is_none_or(|status| status["running"] == true);
    Ok(Json(json!({
        "ok": publisher_ok,
        "service": "backend",
        "kernel_version": strata_kernel::KERNEL_VERSION,
        "publisher": publisher,
    })))
}

async fn session(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<SessionReply>, ApiError> {
    as_caller(&app, &headers, |app, identity| {
        let tenant = ops::header(app, identity)?;
        Ok(SessionReply {
            session: auth::session_view(&tenant, identity),
            state: identity.state,
        })
    })
    .await
}

/// The per-source sign-in rate, then a place to hash for the caller's tenant. The refusal carries
/// the caller's correlation id.
async fn hashing_place(
    app: &Shared,
    caller: &Caller,
) -> Result<crate::limits::HashingPlace, ApiError> {
    let refuse = |error: ApiError| error.with_correlation(&caller.correlation);
    app.admission
        .login_attempt(caller.visitor_source(), strata_protocol::log::now_ms())
        .map_err(refuse)?;
    app.admission
        .hashing(caller.site.as_str())
        .await
        .map_err(refuse)
}

/// Sign in with the named method. The request is the method's own: the backend does not look
/// inside it.
async fn sign_in(
    app: Shared,
    headers: HeaderMap,
    method: String,
    request: Json_,
) -> Result<impl IntoResponse, ApiError> {
    let caller = auth::caller(&headers)?;
    let correlation = caller.correlation.clone();
    // The per-source limit comes first and costs nothing. Only the password method hashes, so
    // only it takes a place to hash; the methods that call an identity provider hold no place
    // while they wait on the network. The limits on failed attempts in `signin::sign_in` sit
    // behind both.
    let place = if method == "password" {
        Some(hashing_place(&app, &caller).await?)
    } else {
        app.admission
            .login_attempt(caller.visitor_source(), strata_protocol::log::now_ms())
            .map_err(|error| error.with_correlation(&correlation))?;
        None
    };
    let tenant = caller.site.as_str().to_string();
    app.blocking_for(&tenant, move |app| {
        let _place = place;
        auth::login(app, &caller, &method, &request)
    })
    .await?
    .map(Json)
    .map_err(|error| error.with_correlation(&correlation))
}

async fn login(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<Json_>,
) -> Result<impl IntoResponse, ApiError> {
    sign_in(app, headers, "password".into(), request).await
}

async fn login_with(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(method): Path<String>,
    Json(request): Json<Json_>,
) -> Result<impl IntoResponse, ApiError> {
    sign_in(app, headers, method, request).await
}

/// A challenge for signing in with a passkey at the address the caller is at.
async fn passkey_challenge(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Json_>, ApiError> {
    let caller = auth::caller(&headers)?;
    let correlation = caller.correlation.clone();
    app.blocking(move |app| passkey::sign_in_challenge(app, &caller))
        .await?
        .map(Json)
        .map_err(|error| error.with_correlation(&correlation))
}

/// Ask for an emailed sign-in link. The answer is the same whether or not the address has an
/// account.
async fn link_request(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    let caller = auth::caller(&headers)?;
    let correlation = caller.correlation.clone();
    app.blocking(move |app| link::request(app, &caller, &body))
        .await?
        .map(Json)
        .map_err(|error| error.with_correlation(&correlation))
}

/// The providers the sign-in page offers at the address it is served at.
async fn oidc_providers(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Json_>, ApiError> {
    let caller = auth::caller(&headers)?;
    let correlation = caller.correlation.clone();
    app.blocking(move |app| oidc::providers(app, &caller))
        .await?
        .map(Json)
        .map_err(|error| error.with_correlation(&correlation))
}

/// Begin a sign-in at an identity provider. Anyone may; to link a provider to the caller's own
/// account the caller is signed in.
async fn oidc_start(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller_at(&app, &headers, move |app, caller, identity| {
        oidc::start(app, caller, identity, &body)
    })
    .await
}

/// The SAML providers the sign-in page offers at the address it is served at.
async fn saml_providers(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Json_>, ApiError> {
    let caller = auth::caller(&headers)?;
    let correlation = caller.correlation.clone();
    app.blocking(move |app| saml::providers(app, &caller))
        .await?
        .map(Json)
        .map_err(|error| error.with_correlation(&correlation))
}

/// Begin a sign-in at a SAML provider. Anyone may; to link a provider to the caller's own account
/// the caller is signed in.
async fn saml_start(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller_at(&app, &headers, move |app, caller, identity| {
        saml::start(app, caller, identity, &body)
    })
    .await
}

/// This service's SAML metadata for a provider. Anyone may ask.
async fn saml_metadata(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    let caller = auth::caller(&headers)?;
    let correlation = caller.correlation.clone();
    app.blocking(move |app| saml::metadata(app, &caller, &body))
        .await?
        .map(Json)
        .map_err(|error| error.with_correlation(&correlation))
}

async fn passkey_register_start(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Json_>, ApiError> {
    as_caller_at(&app, &headers, |app, caller, identity| {
        passkey::register_start(app, caller, identity)
    })
    .await
}

async fn passkey_register_finish(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller_at(&app, &headers, move |app, caller, identity| {
        passkey::register_finish(app, caller, identity, &request)
    })
    .await
}

/// The caller's own passkeys.
async fn passkeys(State(app): State<Shared>, headers: HeaderMap) -> Result<Json<Json_>, ApiError> {
    as_caller_at(&app, &headers, |app, caller, identity| {
        passkey::list(app, caller, identity)
    })
    .await
}

async fn passkey_remove(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<IdRequest>,
) -> Result<Json<Json_>, ApiError> {
    as_caller_at(&app, &headers, move |app, caller, identity| {
        passkey::remove(app, caller, identity, &request.id)
    })
    .await
}

/// The caller's own live sessions.
async fn sessions(State(app): State<Shared>, headers: HeaderMap) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, |app, identity| {
        Ok(json!({ "sessions": auth::list_sessions(app, identity)? }))
    })
    .await
}

async fn session_revoke(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<IdRequest>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        auth::revoke_own_session(app, identity, &request.id)?;
        Ok(json!({}))
    })
    .await
}

/// End every other session of the caller. A sensitive operation (IDN-019).
async fn sessions_revoke_others(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Json_>, ApiError> {
    as_caller_at(&app, &headers, |app, caller, identity| {
        if identity.session.is_some() {
            stepup::require_fresh(app, caller, identity)?;
        }
        Ok(json!({ "revoked": auth::revoke_other_sessions(app, identity)? }))
    })
    .await
}

/// Give the second factor of a waiting sign-in, and have the session.
async fn second_factor(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<Json_>,
) -> Result<Json<SignInOutcome>, ApiError> {
    let caller = auth::caller(&headers)?;
    let correlation = caller.correlation.clone();
    app.blocking(move |app| second::verify(app, &caller, &request))
        .await?
        .map(Json)
        .map_err(|error| error.with_correlation(&correlation))
}

/// The caller's own factors: what is set up, for the desk.
async fn factors(State(app): State<Shared>, headers: HeaderMap) -> Result<Json<Json_>, ApiError> {
    as_caller_at(&app, &headers, |app, caller, identity| {
        second::summary(app, caller, identity)
    })
    .await
}

async fn totp_start(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Json_>, ApiError> {
    as_caller_at(&app, &headers, |app, caller, identity| {
        totp::enrol_start(app, caller, identity)
    })
    .await
}

async fn totp_confirm(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller_at(&app, &headers, move |app, caller, identity| {
        totp::enrol_finish(app, caller, identity, &request)
    })
    .await
}

async fn totp_remove(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Json_>, ApiError> {
    as_caller_at(&app, &headers, |app, caller, identity| {
        totp::remove(app, caller, identity)
    })
    .await
}

async fn recovery_codes_new(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Json_>, ApiError> {
    as_caller_at(&app, &headers, |app, caller, identity| {
        recovery_codes::regenerate(app, caller, identity)
    })
    .await
}

/// Give a fresh factor to the session, before a sensitive operation.
async fn step_up(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    // A password factor is checked with Argon2.
    let hashes = request.get("password").is_some();
    as_caller_at_hashing(&app, &headers, hashes, move |app, caller, identity| {
        stepup::confirm(app, caller, identity, &request)
    })
    .await
}

async fn recovery_request(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    let caller = auth::caller(&headers)?;
    let correlation = caller.correlation.clone();
    app.blocking(move |app| recover::request(app, &caller, &request))
        .await?
        .map(Json)
        .map_err(|error| error.with_correlation(&correlation))
}

async fn recovery_complete(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    let caller = auth::caller(&headers)?;
    let correlation = caller.correlation.clone();
    app.blocking(move |app| recover::complete(app, &caller, &request))
        .await?
        .map(Json)
        .map_err(|error| error.with_correlation(&correlation))
}

async fn recovery_cancel(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    let caller = auth::caller(&headers)?;
    let correlation = caller.correlation.clone();
    app.blocking(move |app| recover::cancel(app, &caller, &request))
        .await?
        .map(Json)
        .map_err(|error| error.with_correlation(&correlation))
}

async fn logout(State(app): State<Shared>, headers: HeaderMap) -> Result<Json<Empty>, ApiError> {
    let caller = auth::caller(&headers)?;
    app.blocking(move |app| match &caller.token {
        Some(token) => auth::end_session(app, token, &caller.correlation),
        None => Ok(()),
    })
    .await??;
    Ok(Json(Empty {}))
}

async fn view(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<ViewRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // Published content needs no session (IDN-301): when sessions cannot be looked up, a reader
    // is served as a reader with none, and what that reader may not see is refused with the
    // outage named (IDN-300).
    let caller = auth::caller(&headers)?;
    let correlation = caller.correlation.clone();
    app.blocking(move |app| {
        let (identity, outage) = match auth::identify(app, &caller) {
            Err(error) if auth::is_identity_outage(&error) => {
                let anonymous = Caller {
                    token: None,
                    ..caller.clone()
                };
                (auth::identify(app, &anonymous)?, true)
            }
            other => (other?, false),
        };
        match ops::view(app, &identity, &request) {
            Err(error) if outage && matches!(error.0.code, ErrorCode::Unauthenticated) => {
                Err(auth::identity_outage())
            }
            other => other,
        }
    })
    .await?
    .map(Json)
    .map_err(|error| error.with_correlation(&correlation))
}

async fn mutate(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(mutation): Json<Mutation>,
) -> Result<impl IntoResponse, ApiError> {
    // The caller's key for this change, so that a retry after a lost reply changes the tenant
    // once (OPS-318).
    let key = match headers.get(HEADER_IDEMPOTENCY) {
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
                        "An idempotency key is 1 to 255 visible ASCII characters.",
                    )
                })?,
        ),
    };
    as_caller(&app, &headers, move |app, identity| {
        ops::mutate(app, identity, &mutation, key.as_deref())
    })
    .await
}

async fn members(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<MembersRequest>,
) -> Result<impl IntoResponse, ApiError> {
    as_caller_at(&app, &headers, move |app, caller, identity| {
        // Giving someone a role, the owner's included, is a sensitive operation (IDN-019).
        if matches!(request, MembersRequest::AssignRole { .. }) && identity.session.is_some() {
            stepup::require_fresh(app, caller, identity)?;
        }
        ops::members(app, identity, &request)
    })
    .await
}

/// Operations for people who may change the schema: the Impact Lab and diagnostics.
fn require_schema_manager(app: &App, identity: &Identity) -> Result<(), ApiError> {
    let tenant = ops::header(app, identity)?;
    if auth::session_view(&tenant, identity).may_manage_schema {
        Ok(())
    } else if identity.anonymous() {
        Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in to use this.",
        ))
    } else {
        Err(ApiError::new(
            ErrorCode::Forbidden,
            "Only someone who can change the schema may use this.",
        ))
    }
}

async fn diagnostics(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    as_caller(&app, &headers, |app, identity| {
        require_schema_manager(app, identity)?;
        ops::diagnostics(app, &identity.tenant)
    })
    .await
}

/// Every tenant's diagnostics and the audit entries. The backend has no operator identity to hold
/// a caller to, and the service credential is shared with the BFF, so the route exists only in
/// development; outside it the answer is "not found". Each call is an audit entry.
async fn platform_diagnostics(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Json_>, ApiError> {
    if !app.cfg.dev {
        return Err(ApiError::new(ErrorCode::NotFound, "not available"));
    }
    let correlation = headers
        .get(HEADER_CORRELATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    app.audit(
        "platform.diagnostics_read",
        correlation,
        json!({ "scope": "every tenant" }),
    );
    app.blocking(ops::platform_diagnostics).await?.map(Json)
}

/// Create a tenant while the process runs: install its bundle and give its owner a first login.
/// The service credential is the only requirement, as for the platform diagnostics, so this is for
/// the operator's tools on the backend's own network; the BFF has no route to it.
async fn platform_create_tenant(
    State(app): State<Shared>,
    Json(request): Json<provision::CreateTenant>,
) -> Result<Json<Json_>, ApiError> {
    let reply = app
        .blocking(move |app| provision::create_tenant(app, request))
        .await??;
    app.hub.committed();
    Ok(Json(reply))
}

async fn lab_overview(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    as_caller(&app, &headers, |app, identity| {
        require_schema_manager(app, identity)?;
        lab::overview(app, identity)
    })
    .await
}

async fn lab_run(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<lab::RunRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let shared = app.clone();
    as_caller(&app, &headers, move |app, identity| {
        require_schema_manager(app, identity)?;
        lab::start(&shared, app, identity, request)
    })
    .await
}

async fn lab_result(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<IdRequest>,
) -> Result<impl IntoResponse, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        require_schema_manager(app, identity)?;
        app.lab
            .result(&request.id, identity)
            .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "no such run"))
    })
    .await
}

async fn lab_replay(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    as_caller(&app, &headers, |app, identity| {
        require_schema_manager(app, identity)?;
        lab::replay(app, identity)
    })
    .await
}

/// Runs `lake` on the machine, so it exists only in development: a production backend never starts
/// a build tool.
async fn lab_verify(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    if !app.cfg.dev {
        return Err(ApiError::new(ErrorCode::NotFound, "not available"));
    }
    let shared = app.clone();
    as_caller(&app, &headers, move |app, identity| {
        require_schema_manager(app, identity)?;
        lab::verify(&shared, app, identity)
    })
    .await
}

/// Run blocking work for a request that is not a person's: nothing is looked up about who sent
/// it, and the work checks the credentials the request itself carries.
async fn as_public<T, F>(app: &Shared, headers: &HeaderMap, work: F) -> Result<Json<T>, ApiError>
where
    T: Serialize + Send + 'static,
    F: FnOnce(&App, &Caller) -> Result<T, ApiError> + Send + 'static,
{
    let caller = auth::caller(headers)?;
    let correlation = caller.correlation.clone();
    let tenant = caller.site.as_str().to_string();
    app.blocking_for(&tenant, move |app| work(app, &caller))
        .await?
        .map(Json)
        .map_err(|error| error.with_correlation(&correlation))
}

/// The token endpoint: client credentials, or the exchange of a delegation. Both check the
/// client's secret with Argon2, so the request takes the per-source rate and a place to hash.
async fn token(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    let caller = auth::caller(&headers)?;
    let correlation = caller.correlation.clone();
    let place = hashing_place(&app, &caller).await?;
    let tenant = caller.site.as_str().to_string();
    app.blocking_for(&tenant, move |app| {
        let _place = place;
        tokens::issue(app, &caller, &body)
    })
    .await?
    .map(Json)
    .map_err(|error| error.with_correlation(&correlation))
}

async fn token_verify(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_public(&app, &headers, move |app, caller| {
        tokens::verify_for_resource(app, caller, &body)
    })
    .await
}

async fn token_introspect(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    // With client credentials the call checks a secret with Argon2.
    let hashes = body.get("client_id").is_some();
    as_caller_at_hashing(&app, &headers, hashes, move |app, caller, identity| {
        tokens::introspect(app, caller, identity, &body)
    })
    .await
}

/// The public keys the tenant's tokens verify with.
async fn keys_published(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Json_>, ApiError> {
    as_public(&app, &headers, |app, caller| {
        keys::published(app, &caller.site)
    })
    .await
}

async fn keys_rotate(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Json_>, ApiError> {
    as_caller_at(&app, &headers, |app, caller, identity| {
        service::require_owner(app, identity)?;
        stepup::require_fresh(app, caller, identity)?;
        keys::rotate(app, &identity.tenant, &identity.correlation)
    })
    .await
}

async fn keys_compromise(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller_at(&app, &headers, move |app, caller, identity| {
        service::require_owner(app, identity)?;
        stepup::require_fresh(app, caller, identity)?;
        let kid = service::text(&body, "kid", 64)?;
        keys::compromise(app, &identity.tenant, kid, &identity.correlation)
    })
    .await
}

async fn clients_list(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, |app, identity| clients::all(app, identity)).await
}

async fn clients_create(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller_at(&app, &headers, move |app, caller, identity| {
        service::require_owner(app, identity)?;
        stepup::require_fresh(app, caller, identity)?;
        clients::create(app, identity, &body)
    })
    .await
}

async fn clients_rotate_secret(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller_at(&app, &headers, move |app, caller, identity| {
        service::require_owner(app, identity)?;
        stepup::require_fresh(app, caller, identity)?;
        clients::rotate_secret(app, identity, &body)
    })
    .await
}

async fn clients_revoke(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        clients::revoke(app, identity, &body)
    })
    .await
}

async fn grants_create(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        tokens::create_grant(app, identity, &body)
    })
    .await
}

async fn grants_revoke(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        tokens::revoke_grant(app, identity, &body)
    })
    .await
}

async fn provisioning_token_create(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller_at(&app, &headers, move |app, caller, identity| {
        service::require_owner(app, identity)?;
        stepup::require_fresh(app, caller, identity)?;
        scim::create_token(app, identity, &body)
    })
    .await
}

async fn provisioning_token_revoke(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        scim::revoke_token(app, identity, &body)
    })
    .await
}

async fn provisioning_groups(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, |app, identity| {
        scim::group_roles(app, identity)
    })
    .await
}

async fn provisioning_group_set(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller_at(&app, &headers, move |app, caller, identity| {
        service::require_owner(app, identity)?;
        stepup::require_fresh(app, caller, identity)?;
        scim::set_group_role(app, identity, &body)
    })
    .await
}

async fn provisioning_group_remove(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        scim::remove_group_role(app, identity, &body)
    })
    .await
}

/// A SCIM request from the gateway, as an envelope: the token it carried is the credential.
async fn scim_request(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_public(&app, &headers, move |app, caller| {
        scim::handle(app, caller, &body)
    })
    .await
}

async fn socket(
    State(app): State<Shared>,
    headers: HeaderMap,
    stop: Option<Extension<strata_runtime::server::Shutdown>>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let caller = auth::caller(&headers)?;
    let correlation = caller.correlation.clone();
    let source = caller.visitor_source().map(str::to_string);
    let identity = auth::identify_async(&app, caller).await?;
    let session = identity.session.as_ref().map(|s| s.id.as_str().to_string());
    let places = app
        .admission
        .socket(session.as_deref(), source.as_deref())
        .map_err(|error| {
            app.audit(
                "socket.refused",
                &correlation,
                json!({ "tenant": identity.tenant.as_str(), "session": session, "source": source }),
            );
            error.with_correlation(&correlation)
        })?;
    // Registered with the server's drain, so that a stop waits for the socket to close.
    let stop = stop.map(|Extension(stop)| stop);
    let held = match &stop {
        Some(stop) => Some(stop.hold().ok_or_else(|| {
            ApiError::new(ErrorCode::Unavailable, "The service is stopping.")
                .with_correlation(&correlation)
        })?),
        None => None,
    };
    Ok(upgrade
        .max_message_size(MAX_FRAME)
        .on_upgrade(move |socket| async move {
            ws::serve(app, identity, places, socket, stop).await;
            drop(held);
        }))
}

/// Refuse anything that does not carry the service token. The backend listens on loopback, and
/// this keeps other local processes and stray browser requests out as well.
async fn gate(State(app): State<Shared>, request: Request, next: Next) -> Response {
    let started = Instant::now();
    let path = request.uri().path().to_string();
    let correlation = request
        .headers()
        .get(HEADER_CORRELATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let open = path == "/internal/health";
    let response = if open || has_service_token(&app, request.headers()) {
        next.run(request).await
    } else {
        ApiError::new(ErrorCode::Unauthenticated, "service token required").into_response()
    };
    if !open {
        LOG.info(
            "api.request",
            &correlation,
            json!({ "path": path, "status": response.status().as_u16(), "ms": started.elapsed().as_millis() as u64 }),
        );
    }
    response
}

pub fn router(app: Shared) -> Router {
    let router = Router::new()
        .route("/internal/health", get(health))
        .route("/internal/platform/hosts", post(crate::hosts::route))
        .route(
            "/internal/sign-in/invite/accept",
            post(crate::signin::invite::route),
        )
        .route("/internal/session", post(session))
        .route("/internal/session/login", post(login))
        .route("/internal/session/sign-in/:method", post(login_with))
        .route("/internal/sign-in/link/request", post(link_request))
        .route("/internal/sign-in/oidc/providers", post(oidc_providers))
        .route("/internal/sign-in/oidc/start", post(oidc_start))
        .route("/internal/sign-in/saml/providers", post(saml_providers))
        .route("/internal/sign-in/saml/start", post(saml_start))
        .route("/internal/sign-in/saml/metadata", post(saml_metadata))
        .route("/internal/passkeys", post(passkeys))
        .route("/internal/passkeys/challenge", post(passkey_challenge))
        .route(
            "/internal/passkeys/register/start",
            post(passkey_register_start),
        )
        .route(
            "/internal/passkeys/register/finish",
            post(passkey_register_finish),
        )
        .route("/internal/passkeys/remove", post(passkey_remove))
        .route("/internal/sessions", post(sessions))
        .route("/internal/sessions/revoke", post(session_revoke))
        .route(
            "/internal/sessions/revoke-others",
            post(sessions_revoke_others),
        )
        .route("/internal/session/second-factor", post(second_factor))
        .route("/internal/session/step-up", post(step_up))
        .route("/internal/factors", post(factors))
        .route("/internal/factors/totp/start", post(totp_start))
        .route("/internal/factors/totp/confirm", post(totp_confirm))
        .route("/internal/factors/totp/remove", post(totp_remove))
        .route("/internal/factors/recovery-codes", post(recovery_codes_new))
        .route("/internal/recovery/request", post(recovery_request))
        .route("/internal/recovery/complete", post(recovery_complete))
        .route("/internal/recovery/cancel", post(recovery_cancel))
        .route("/internal/session/logout", post(logout))
        .route("/internal/view", post(view))
        .route("/internal/mutate", post(mutate))
        .route("/internal/members", post(members))
        .route("/internal/diagnostics", post(diagnostics))
        .route("/internal/platform/diagnostics", post(platform_diagnostics))
        .route("/internal/platform/tenants", post(platform_create_tenant))
        .route("/internal/lab/overview", post(lab_overview))
        .route("/internal/lab/run", post(lab_run))
        .route("/internal/lab/result", post(lab_result))
        .route("/internal/lab/replay", post(lab_replay))
        .route("/internal/lab/verify", post(lab_verify))
        .route("/internal/token", post(token))
        .route("/internal/token/verify", post(token_verify))
        .route("/internal/token/introspect", post(token_introspect))
        .route("/internal/keys", post(keys_published))
        .route("/internal/keys/rotate", post(keys_rotate))
        .route("/internal/keys/compromise", post(keys_compromise))
        .route("/internal/clients", post(clients_list))
        .route("/internal/clients/create", post(clients_create))
        .route(
            "/internal/clients/rotate-secret",
            post(clients_rotate_secret),
        )
        .route("/internal/clients/revoke", post(clients_revoke))
        .route("/internal/grants/create", post(grants_create))
        .route("/internal/grants/revoke", post(grants_revoke))
        .route(
            "/internal/provisioning/tokens/create",
            post(provisioning_token_create),
        )
        .route(
            "/internal/provisioning/tokens/revoke",
            post(provisioning_token_revoke),
        )
        .route("/internal/provisioning/groups", post(provisioning_groups))
        .route(
            "/internal/provisioning/groups/set",
            post(provisioning_group_set),
        )
        .route(
            "/internal/provisioning/groups/remove",
            post(provisioning_group_remove),
        )
        .route("/internal/scim", post(scim_request))
        .route("/internal/ws", get(socket));
    // The development routes are compiled only with the `development` feature, and mounted only
    // when the process was started in development mode.
    #[cfg(feature = "development")]
    let router = if app.cfg.dev {
        crate::dev::mount(router)
    } else {
        router
    };
    router
        .layer(middleware::from_fn_with_state(app.clone(), gate))
        .with_state(app)
}
