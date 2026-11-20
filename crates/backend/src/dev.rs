//! Development routes: fixture reset, clock skew, session ageing and expiry, the mail sink, a
//! throwaway tenant and the identity-outage switch. Compiled only with the `development` cargo feature, and mounted
//! only when the process was started with `STRATA_DEV=1`. A build without the feature has none
//! of these routes and refuses to start in development mode.

use crate::app::{ApiError, App, Shared};
use crate::auth::Identity;
use crate::http::{as_caller, as_caller_at};
use crate::signin::stepup;
use crate::{ops, seed};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value as Json_};
use strata_kernel::owner_role;
use strata_kernel::rules::roles_of;
use strata_protocol::api::ErrorCode;
use strata_protocol::ws::SessionState;

/// Add the development routes to `router`.
pub fn mount(router: Router<Shared>) -> Router<Shared> {
    router
        .route("/internal/dev/reset", post(dev_reset))
        .route("/internal/dev/expire-session", post(dev_expire))
        .route("/internal/dev/outbox", post(dev_outbox))
        .route(
            "/internal/dev/age-authentication",
            post(dev_age_authentication),
        )
        .route("/internal/dev/mail-outbox", post(dev_mail_outbox))
        .route("/internal/dev/provision-tenant", post(dev_provision_tenant))
        .route("/internal/dev/clock", post(dev_clock))
        .route("/internal/dev/identity-outage", post(dev_identity_outage))
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
    app.hub.committed();
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

/// The prefix of the id of a tenant the development route makes, so that it can only ever remove or
/// replace a tenant of its own.
const THROWAWAY: &str = "throwaway";

/// Development only: provision a throwaway tenant through the real provisioning route
/// ([`crate::provision::create_tenant`]), so that a browser journey has an owner's invitation to
/// follow. The body is `{ "name": "throwaway...", "owner_email": ..., "origin": "http://host:port" }`.
/// The tenant is a one-object site named by its id, which must begin with `throwaway`; one that
/// already exists under that name is removed first, so a journey can run again on the same stack.
/// The origin's host is bound to the tenant, as the real route does, and the BFF learns it from the
/// backend's list of hosts. The welcome message is in the development mail outbox. For the owner of
/// the site the request is made at, as every development fixture is.
async fn dev_provision_tenant(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    let reply = as_caller(&app, &headers, move |app, identity| {
        require_dev_owner(app, identity)?;
        let text = |name: &str| {
            body.get(name).and_then(Json_::as_str).ok_or_else(|| {
                ApiError::new(ErrorCode::Invalid, format!("The request needs {name}."))
            })
        };
        let id = text("name")?;
        if !id.starts_with(THROWAWAY) {
            return Err(ApiError::new(
                ErrorCode::Invalid,
                format!("A throwaway tenant's name begins with {THROWAWAY}."),
            ));
        }
        let tenant = strata_kernel::TenantId::new(id)
            .map_err(|problem| ApiError::new(ErrorCode::Invalid, format!("name: {problem}")))?;
        let request = crate::provision::CreateTenant {
            bundle: json!({
                "id": tenant.as_str(),
                "name": "Throwaway site",
                "owner": "user:owner",
                "created_at": "2026-09-01T09:00:00Z",
                "objects": [{
                    "id": "note",
                    "label": "Note",
                    "fields": [{ "id": "title", "label": "Title", "type": "string", "required": true }]
                }]
            }),
            owner_email: text("owner_email")?.to_string(),
            owner_name: None,
            origin: text("origin")?.to_string(),
        };
        // Only a tenant this route made is replaced.
        if app.store.load(&tenant, &[])?.tenant.is_some() {
            app.store.reset_tenant(&tenant)?;
        }
        app.audit(
            "dev.provision_tenant",
            &identity.correlation,
            json!({ "tenant": tenant.as_str(), "by": identity.tenant.as_str() }),
        );
        crate::provision::create_tenant(app, request)
    })
    .await?;
    app.hub.committed();
    Ok(reply)
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

/// Development only: move this process's clock forward, to see a lifetime or a revocation bound
/// pass without waiting for it.
async fn dev_clock(
    State(app): State<Shared>,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    if !app.cfg.dev {
        return Err(ApiError::new(ErrorCode::NotFound, "not available"));
    }
    let advance = body.get("advance_ms").and_then(Json_::as_i64).unwrap_or(0);
    let total = app
        .clock_skew_ms
        .fetch_add(advance, std::sync::atomic::Ordering::Relaxed)
        + advance;
    Ok(Json(json!({ "skew_ms": total })))
}

/// Development only: make the identity service answer as if it could not be reached, or again as
/// it should (IDN-300, IDN-301).
async fn dev_identity_outage(
    State(app): State<Shared>,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    if !app.cfg.dev {
        return Err(ApiError::new(ErrorCode::NotFound, "not available"));
    }
    let on = body.get("on").and_then(Json_::as_bool).unwrap_or(false);
    app.identity_outage
        .store(on, std::sync::atomic::Ordering::Relaxed);
    Ok(Json(json!({ "identity_outage": on })))
}
