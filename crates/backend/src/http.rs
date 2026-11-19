//! The internal HTTP API: routes, the service-token gate and request logging.

use crate::app::{ApiError, App, Shared, LOG};
use crate::auth::{self, has_service_token, Caller, Identity};
use crate::signin::{
    link, oidc, passkey, recover, recovery_codes, second, stepup, totp, SignInOutcome,
};
use crate::{lab, ops, seed, ws};
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, Request, State};
use axum::http::HeaderMap;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use serde_json::{json, Value as Json_};
use std::time::Instant;
use strata_kernel::owner_role;
use strata_kernel::rules::roles_of;
use strata_protocol::api::{
    Empty, ErrorCode, IdRequest, Mutation, SessionReply, ViewRequest, HEADER_CORRELATION,
};
use strata_protocol::ws::{SessionState, MAX_FRAME};

/// Run blocking work as the identified caller and wrap the result as JSON.
async fn as_caller<T, F>(app: &Shared, headers: &HeaderMap, work: F) -> Result<Json<T>, ApiError>
where
    T: Serialize + Send + 'static,
    F: FnOnce(&App, &Identity) -> Result<T, ApiError> + Send + 'static,
{
    let caller = auth::caller(headers)?;
    let correlation = caller.correlation.clone();
    app.blocking(move |app| {
        let identity = auth::identify(app, &caller)?;
        work(app, &identity)
    })
    .await?
    .map(Json)
    .map_err(|error| error.with_correlation(&correlation))
}

/// Run blocking work as the identified caller, with what the caller asserted as well.
async fn as_caller_at<T, F>(app: &Shared, headers: &HeaderMap, work: F) -> Result<Json<T>, ApiError>
where
    T: Serialize + Send + 'static,
    F: FnOnce(&App, &Caller, &Identity) -> Result<T, ApiError> + Send + 'static,
{
    let caller = auth::caller(headers)?;
    let correlation = caller.correlation.clone();
    app.blocking(move |app| {
        let identity = auth::identify(app, &caller)?;
        work(app, &caller, &identity)
    })
    .await?
    .map(Json)
    .map_err(|error| error.with_correlation(&correlation))
}

async fn health(State(app): State<Shared>) -> Result<Json<Json_>, ApiError> {
    let health = app.blocking(|app| app.store.health()).await??;
    Ok(Json(json!({
        "ok": health.ok,
        "service": "backend",
        "kernel_version": strata_kernel::KERNEL_VERSION,
        "store": health,
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
    app.blocking(move |app| auth::login(app, &caller, &method, &request))
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
    as_caller_at(&app, &headers, move |app, caller, identity| {
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
    as_caller(&app, &headers, move |app, identity| {
        ops::view(app, identity, &request)
    })
    .await
}

async fn mutate(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(mutation): Json<Mutation>,
) -> Result<impl IntoResponse, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        ops::mutate(app, identity, &mutation)
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

/// Every tenant's diagnostics and the audit entries. The service credential is the only
/// requirement, so this is for the operator's tools on the backend's own network; the BFF has no
/// route to it.
async fn platform_diagnostics(State(app): State<Shared>) -> Result<Json<Json_>, ApiError> {
    app.blocking(ops::platform_diagnostics).await?.map(Json)
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

async fn lab_verify(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let shared = app.clone();
    as_caller(&app, &headers, move |app, identity| {
        require_schema_manager(app, identity)?;
        lab::verify(&shared, app, identity)
    })
    .await
}

/// Development fixtures are for the tenant's owner, signed in, on a backend in development mode.
/// Outside development mode they do not exist.
fn require_dev_owner(app: &App, identity: &Identity) -> Result<(), ApiError> {
    if !app.cfg.dev {
        return Err(ApiError::new(ErrorCode::NotFound, "not available"));
    }
    if identity.anonymous() {
        return Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in as the site's owner to use this.",
        ));
    }
    let tenant = ops::header(app, identity)?;
    if roles_of(&tenant, &identity.principal).contains(&owner_role()) {
        Ok(())
    } else {
        Err(ApiError::new(
            ErrorCode::Forbidden,
            "Only the site's owner may use this.",
        ))
    }
}

/// Development only: remove the caller's tenant and install its bundle again. Other tenants are
/// not touched.
async fn dev_reset(State(app): State<Shared>, headers: HeaderMap) -> Result<Json<Json_>, ApiError> {
    let reply = as_caller_at(&app, &headers, |app, caller, identity| {
        require_dev_owner(app, identity)?;
        // Wiping a tenant's data is one of the owner's sensitive operations (IDN-019).
        stepup::require_fresh(app, caller, identity)?;
        let session = identity.session.as_ref().map(|s| s.id.as_str().to_string());
        app.audit(
            "dev.reset",
            &identity.correlation,
            json!({ "tenant": identity.tenant.as_str(), "session": session }),
        );
        seed::reset_tenant(app, &identity.tenant)
            .map_err(|problem| ApiError::hidden(ErrorCode::Internal, problem))?;
        Ok(json!({ "reset": true, "tenant": identity.tenant.as_str() }))
    })
    .await?;
    app.hub.kick.notify_one();
    Ok(reply)
}

/// Development only: the messages the development sink holds, so that a test can read the link in
/// one.
async fn dev_mail_outbox(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, |app, identity| {
        require_dev_owner(app, identity)?;
        let messages: Vec<Json_> = app
            .cfg
            .mail
            .outbox()
            .into_iter()
            .map(|mail| json!({ "to": mail.to, "subject": mail.subject, "text": mail.text }))
            .collect();
        Ok(json!({ "messages": messages }))
    })
    .await
}

/// Development only: make the caller's own session look as if it authenticated long ago, so that a
/// test can see a sensitive operation ask for a fresh factor.
async fn dev_age_authentication(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, |app, identity| {
        // Only the caller's own session is touched, so no role is asked for; development only.
        if !app.cfg.dev {
            return Err(ApiError::new(ErrorCode::NotFound, "not available"));
        }
        let Some(mut session) = identity.session.clone() else {
            return Ok(json!({ "aged": false }));
        };
        session.authenticated_at =
            strata_kernel::Timestamp(app.now().0 - app.cfg.factors.step_up_age_ms - 60_000);
        app.store.put_session(&session)?;
        Ok(json!({ "aged": true }))
    })
    .await
}

/// Development only: end the caller's own session as if its time had run out.
async fn dev_expire(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, |app, identity| {
        require_dev_owner(app, identity)?;
        let Some(mut session) = identity.session.clone() else {
            return Ok(json!({ "expired": false }));
        };
        session.expires_at = strata_kernel::Timestamp(app.now().0 - 1);
        app.store.put_session(&session)?;
        let _ = app.hub.sessions.send((session.id, SessionState::Expired));
        Ok(json!({ "expired": true }))
    })
    .await
}

async fn socket(
    State(app): State<Shared>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let caller = auth::caller(&headers)?;
    let identity = auth::identify_async(&app, caller).await?;
    Ok(upgrade
        .max_message_size(MAX_FRAME)
        .on_upgrade(move |socket| ws::serve(app, identity, socket)))
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
    Router::new()
        .route("/internal/health", get(health))
        .route("/internal/session", post(session))
        .route("/internal/session/login", post(login))
        .route("/internal/session/sign-in/:method", post(login_with))
        .route("/internal/sign-in/link/request", post(link_request))
        .route("/internal/sign-in/oidc/providers", post(oidc_providers))
        .route("/internal/sign-in/oidc/start", post(oidc_start))
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
        .route("/internal/diagnostics", post(diagnostics))
        .route("/internal/platform/diagnostics", post(platform_diagnostics))
        .route("/internal/lab/overview", post(lab_overview))
        .route("/internal/lab/run", post(lab_run))
        .route("/internal/lab/result", post(lab_result))
        .route("/internal/lab/replay", post(lab_replay))
        .route("/internal/lab/verify", post(lab_verify))
        .route("/internal/dev/reset", post(dev_reset))
        .route("/internal/dev/expire-session", post(dev_expire))
        .route(
            "/internal/dev/age-authentication",
            post(dev_age_authentication),
        )
        .route("/internal/dev/mail-outbox", post(dev_mail_outbox))
        .route("/internal/ws", get(socket))
        .layer(middleware::from_fn_with_state(app.clone(), gate))
        .with_state(app)
}
