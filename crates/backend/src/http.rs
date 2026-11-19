//! The internal HTTP API: routes, the service-token gate and request logging.

use crate::app::{ApiError, App, Shared, LOG};
use crate::auth::{self, has_service_token, Identity};
use crate::{lab, ops, seed, ws};
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Request, State};
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
    Empty, ErrorCode, IdRequest, LoginRequest, Mutation, SessionReply, ViewRequest,
    HEADER_CORRELATION,
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

async fn login(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let caller = auth::caller(&headers)?;
    let correlation = caller.correlation.clone();
    app.blocking(move |app| auth::login(app, &caller, &request))
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
    let reply = as_caller(&app, &headers, |app, identity| {
        require_dev_owner(app, identity)?;
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
    app.hub.committed();
    Ok(reply)
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

/// Development only: stop or resume the kick that follows each commit, so that the outbox
/// machine's own sweep can be seen publishing what a missed kick left behind.
async fn dev_outbox(
    State(app): State<Shared>,
    Json(request): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    if !app.cfg.dev {
        return Err(ApiError::new(ErrorCode::NotFound, "There is nothing here."));
    }
    let kick = request.get("kick").and_then(Json_::as_bool).unwrap_or(true);
    if let Some(outbox) = app.hub.outbox.get() {
        outbox.set_kick(kick);
    }
    Ok(Json(json!({ "kick": kick })))
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
        .route("/internal/dev/outbox", post(dev_outbox))
        .route("/internal/ws", get(socket))
        .layer(middleware::from_fn_with_state(app.clone(), gate))
        .with_state(app)
}
