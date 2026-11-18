//! Browser-facing routes.

use crate::{relay, Bff, Shared, COOKIE, LOG};
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, Query, Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, COOKIE as COOKIE_HEADER, HOST, ORIGIN, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::de::DeserializeOwned;
use serde_json::{json, Value as Json_};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::time::Instant;
use strata_kernel::TenantId;
use strata_protocol::api::{
    valid_correlation, ErrorCode, ErrorReply, LoginReply, LoginRequest, Mutation, SessionReply, SessionView,
    ViewRequest, HEADER_CORRELATION, HEADER_SERVICE, HEADER_SESSION, HEADER_SITE,
};
use strata_protocol::ws::SessionState;
use strata_substrate::project::{project, ViewData};
use strata_substrate::theme;
use tower_http::services::ServeDir;

/// A failure reply with its status.
pub struct Failure(pub ErrorReply);

impl Failure {
    pub fn new(code: ErrorCode, message: &str, correlation: &str) -> Self {
        Failure(ErrorReply {
            code,
            message: message.to_string(),
            kind: None,
            failed: Vec::new(),
            correlation: correlation.to_string(),
        })
    }
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.0.code.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(json!(self.0))).into_response()
    }
}

/// What this process established about a browser request before doing anything with it.
#[derive(Clone)]
pub struct Visit {
    pub site: TenantId,
    pub token: Option<String>,
    pub correlation: String,
}

fn text<'a>(headers: &'a HeaderMap, name: impl axum::http::header::AsHeaderName) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

pub fn cookie(headers: &HeaderMap) -> Option<String> {
    text(headers, COOKIE_HEADER)?
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE)
        .map(|(_, value)| value.to_string())
        .filter(|value| value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit()))
}

fn random_id(prefix: &str) -> String {
    let mut bytes = [0u8; 6];
    let _ = getrandom::getrandom(&mut bytes);
    format!("{prefix}-{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// Establish site, session token and correlation id. The site comes from the host name and this
/// process's configuration only.
pub fn visit(bff: &Bff, headers: &HeaderMap) -> Result<Visit, Failure> {
    let correlation = text(headers, HEADER_CORRELATION)
        .filter(|id| valid_correlation(id))
        .map(str::to_string)
        .unwrap_or_else(|| random_id("f"));
    let host = text(headers, HOST).unwrap_or("");
    let host = match host.rsplit_once(':') {
        Some((name, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => name,
        _ => host,
    };
    let site = bff
        .cfg
        .sites
        .get(&host.to_lowercase())
        .cloned()
        .ok_or_else(|| Failure::new(ErrorCode::NotFound, "No site is served at this address.", &correlation))?;
    Ok(Visit { site, token: cookie(headers), correlation })
}

/// Whether the request's `Origin` is one of ours. Browsers send it on every cross-origin
/// request and on every same-origin request that is not a plain GET.
pub fn origin_allowed(bff: &Bff, headers: &HeaderMap) -> bool {
    text(headers, ORIGIN).is_some_and(|origin| bff.cfg.origins.iter().any(|allowed| allowed == origin))
}

/// Call the backend's internal API as this visit.
pub async fn backend<T: DeserializeOwned>(bff: &Bff, visit: &Visit, path: &str, body: &Json_) -> Result<T, Failure> {
    let unavailable = |why: String| {
        LOG.error("backend.unreachable", &visit.correlation, json!({ "path": path, "error": why }));
        Failure::new(ErrorCode::Unavailable, "The service is not available right now.", &visit.correlation)
    };
    let mut request = bff
        .http
        .post(format!("{}{path}", bff.cfg.backend))
        .header(HEADER_SERVICE, &bff.cfg.service_token)
        .header(HEADER_SITE, visit.site.as_str())
        .header(HEADER_CORRELATION, &visit.correlation)
        .json(body);
    if let Some(token) = &visit.token {
        request = request.header(HEADER_SESSION, token);
    }
    let response = request.send().await.map_err(|e| unavailable(e.to_string()))?;
    if response.status().is_success() {
        response.json::<T>().await.map_err(|e| unavailable(e.to_string()))
    } else {
        let mut reply = response.json::<ErrorReply>().await.map_err(|e| unavailable(e.to_string()))?;
        reply.correlation = visit.correlation.clone();
        Err(Failure(reply))
    }
}

fn session_cookie(bff: &Bff, token: &str, max_age: i64) -> HeaderValue {
    let secure = if bff.cfg.secure_cookies { "; Secure" } else { "" };
    HeaderValue::from_str(&format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}"))
        .expect("cookie values are ASCII")
}

/// Require a state-changing request to come from our own pages with the session's CSRF token.
async fn guard_write(bff: &Bff, visit: &Visit, headers: &HeaderMap) -> Result<SessionView, Failure> {
    let refuse = |message| Failure::new(ErrorCode::Forbidden, message, &visit.correlation);
    if !origin_allowed(bff, headers) {
        LOG.warn("csrf.origin_refused", &visit.correlation, json!({ "origin": text(headers, ORIGIN) }));
        return Err(refuse("This request did not come from the site."));
    }
    let reply: SessionReply = backend(bff, visit, "/internal/session", &json!({})).await?;
    let Some(expected) = &reply.session.csrf_token else {
        return Err(Failure::new(ErrorCode::Unauthenticated, "Sign in to do this.", &visit.correlation));
    };
    let given = text(headers, "x-csrf-token").unwrap_or("");
    let same = given.len() == expected.len() && given.bytes().zip(expected.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0;
    if !same {
        LOG.warn("csrf.token_refused", &visit.correlation, json!({}));
        return Err(refuse("The request token is missing or stale. Reload the page and try again."));
    }
    Ok(reply.session)
}

async fn session(State(bff): State<Shared>, headers: HeaderMap) -> Result<Response, Failure> {
    let visit = visit(&bff, &headers)?;
    let reply: SessionReply = backend(&bff, &visit, "/internal/session", &json!({})).await?;
    let mut response = Json(json!({ "session": reply.session, "state": reply.state })).into_response();
    if visit.token.is_some() && matches!(reply.state, SessionState::Expired | SessionState::Revoked) {
        response.headers_mut().insert(SET_COOKIE, session_cookie(&bff, "", 0));
    }
    Ok(response)
}

async fn login(State(bff): State<Shared>, headers: HeaderMap, Json(request): Json<LoginRequest>) -> Result<Response, Failure> {
    let visit = visit(&bff, &headers)?;
    if !origin_allowed(&bff, &headers) || text(&headers, "x-strata-request") != Some("1") {
        return Err(Failure::new(ErrorCode::Forbidden, "This request did not come from the site.", &visit.correlation));
    }
    let reply: LoginReply = backend(&bff, &visit, "/internal/session/login", &json!(request)).await?;
    let lifetime = reply.session.expires_at.map_or(0, |at| (at - strata_protocol::log::now_ms()) / 1000).max(0);
    let mut response = Json(json!({ "session": reply.session, "state": "active" })).into_response();
    response.headers_mut().insert(SET_COOKIE, session_cookie(&bff, &reply.token, lifetime));
    Ok(response)
}

async fn logout(State(bff): State<Shared>, headers: HeaderMap) -> Result<Response, Failure> {
    let visit = visit(&bff, &headers)?;
    if visit.token.is_some() {
        // Without a live session there is nothing to protect; with one, the usual guard applies.
        match guard_write(&bff, &visit, &headers).await {
            Ok(_) => {
                let _: Json_ = backend(&bff, &visit, "/internal/session/logout", &json!({})).await?;
            }
            Err(Failure(reply)) if reply.code == ErrorCode::Unauthenticated => {}
            Err(failure) => return Err(failure),
        }
    }
    let anonymous = Visit { token: None, ..visit };
    let reply: SessionReply = backend(&bff, &anonymous, "/internal/session", &json!({})).await?;
    let mut response = Json(json!({ "session": reply.session, "state": "anonymous" })).into_response();
    response.headers_mut().insert(SET_COOKIE, session_cookie(&bff, "", 0));
    Ok(response)
}

/// Fetch a view from the backend and project it into the browser's view model.
async fn view_model(bff: &Bff, visit: &Visit, path: &str, request: BTreeMap<String, String>) -> Result<Json_, Failure> {
    let data: ViewData =
        backend(bff, visit, "/internal/view", &json!(ViewRequest { path: path.to_string(), request })).await?;
    let mut model = project(&data);
    let settings = data.sections.get("site").and_then(|section| section.rows.first());
    let (resolved, rejected) = settings.map(|record| theme::resolve(&record.fields)).unwrap_or_default();
    if !rejected.is_empty() {
        LOG.warn("theme.rejected", &visit.correlation, json!({ "settings": rejected }));
    }
    model["theme"] = json!(resolved);
    model["path"] = json!(path);
    Ok(model)
}

async fn view(
    State(bff): State<Shared>,
    headers: HeaderMap,
    Query(mut query): Query<BTreeMap<String, String>>,
) -> Result<Json<Json_>, Failure> {
    let visit = visit(&bff, &headers)?;
    let path = query.remove("path").filter(|p| p.starts_with('/') && p.len() <= 300).unwrap_or_else(|| "/".into());
    query.retain(|name, value| name.len() <= 32 && value.len() <= 200);
    view_model(&bff, &visit, &path, query).await.map(Json)
}

async fn records(State(bff): State<Shared>, headers: HeaderMap, Json(mutation): Json<Mutation>) -> Result<Json<Json_>, Failure> {
    let visit = visit(&bff, &headers)?;
    guard_write(&bff, &visit, &headers).await?;
    backend(&bff, &visit, "/internal/mutate", &json!(mutation)).await.map(Json)
}

/// Relay a guarded POST to an internal endpoint unchanged.
async fn guarded(bff: &Bff, headers: &HeaderMap, path: &str, body: Json_) -> Result<Json<Json_>, Failure> {
    let visit = visit(bff, headers)?;
    guard_write(bff, &visit, headers).await?;
    backend(bff, &visit, path, &body).await.map(Json)
}

async fn lab_overview(State(bff): State<Shared>, headers: HeaderMap) -> Result<Json<Json_>, Failure> {
    let visit = visit(&bff, &headers)?;
    backend(&bff, &visit, "/internal/lab/overview", &json!({})).await.map(Json)
}

async fn lab_run(State(bff): State<Shared>, headers: HeaderMap, Json(body): Json<Json_>) -> Result<Json<Json_>, Failure> {
    guarded(&bff, &headers, "/internal/lab/run", body).await
}

async fn lab_result(State(bff): State<Shared>, headers: HeaderMap, Path(id): Path<String>) -> Result<Json<Json_>, Failure> {
    let visit = visit(&bff, &headers)?;
    backend(&bff, &visit, "/internal/lab/result", &json!({ "id": id })).await.map(Json)
}

async fn lab_replay(State(bff): State<Shared>, headers: HeaderMap) -> Result<Json<Json_>, Failure> {
    guarded(&bff, &headers, "/internal/lab/replay", json!({})).await
}

async fn lab_verify(State(bff): State<Shared>, headers: HeaderMap) -> Result<Json<Json_>, Failure> {
    guarded(&bff, &headers, "/internal/lab/verify", json!({})).await
}

async fn diagnostics(State(bff): State<Shared>, headers: HeaderMap) -> Result<Json<Json_>, Failure> {
    let visit = visit(&bff, &headers)?;
    let backend_report: Json_ = backend(&bff, &visit, "/internal/diagnostics", &json!({})).await?;
    Ok(Json(json!({
        "bff": {
            "service": "bff",
            "open_sockets": bff.sockets.load(Ordering::Relaxed),
            "pending_requests": bff.in_flight.load(Ordering::Relaxed),
            "sites": bff.cfg.sites.iter().map(|(host, tenant)| json!({ "host": host, "tenant": tenant.as_str() })).collect::<Vec<_>>(),
            "allowed_origins": bff.cfg.origins,
        },
        "backend": backend_report,
    })))
}

async fn dev(State(bff): State<Shared>, headers: HeaderMap, Path(action): Path<String>) -> Result<Json<Json_>, Failure> {
    let visit = visit(&bff, &headers)?;
    let path = match (bff.cfg.dev, action.as_str()) {
        (true, "expire-session") => "/internal/dev/expire-session",
        (true, "reset") => "/internal/dev/reset",
        _ => return Err(Failure::new(ErrorCode::NotFound, "Not available.", &visit.correlation)),
    };
    if !origin_allowed(&bff, &headers) {
        return Err(Failure::new(ErrorCode::Forbidden, "This request did not come from the site.", &visit.correlation));
    }
    backend(&bff, &visit, path, &json!({})).await.map(Json)
}

async fn socket(State(bff): State<Shared>, headers: HeaderMap, upgrade: WebSocketUpgrade) -> Result<Response, Failure> {
    let visit = visit(&bff, &headers)?;
    if !origin_allowed(&bff, &headers) {
        LOG.warn("ws.origin_refused", &visit.correlation, json!({ "origin": text(&headers, ORIGIN) }));
        return Err(Failure::new(ErrorCode::Forbidden, "This connection did not come from the site.", &visit.correlation));
    }
    Ok(upgrade
        .max_message_size(strata_protocol::ws::MAX_FRAME)
        .on_upgrade(move |browser| relay::relay(bff, visit, browser)))
}

/// Decode a query-string value: `+` is a space, `%XX` is a byte.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => {
                match (bytes.get(i + 1).copied().and_then(hex), bytes.get(i + 2).copied().and_then(hex)) {
                    (Some(high), Some(low)) => {
                        out.push((high * 16 + low) as u8);
                        i += 2;
                    }
                    _ => out.push(b'%'),
                }
            }
            other => out.push(other),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The page shell with the first view model and session embedded, so the first paint needs no
/// request. The data block is JSON, not script: `<` is escaped and the CSP allows no inline code.
async fn page(State(bff): State<Shared>, headers: HeaderMap, request: Request) -> Response {
    let visit = match visit(&bff, &headers) {
        Ok(visit) => visit,
        Err(failure) => return failure.into_response(),
    };
    let Some(index) = &bff.index else {
        let body = "The frontend has not been built. Run `npm run build` in web/, or use the development server.";
        return (StatusCode::SERVICE_UNAVAILABLE, body).into_response();
    };
    let path = request.uri().path().to_string();
    let query: BTreeMap<String, String> = request
        .uri()
        .query()
        .map(|q| {
            q.split('&')
                .filter_map(|pair| pair.split_once('='))
                .filter(|(name, value)| name.len() <= 32 && value.len() <= 200)
                .map(|(name, value)| (name.to_string(), percent_decode(value)))
                .collect()
        })
        .unwrap_or_default();
    let session = backend::<SessionReply>(&bff, &visit, "/internal/session", &json!({})).await;
    let view = view_model(&bff, &visit, &path, query).await;
    let status = match &view {
        Err(Failure(reply)) if reply.code == ErrorCode::NotFound => StatusCode::NOT_FOUND,
        Err(Failure(reply)) if reply.code == ErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::OK,
    };
    let view = view.ok();
    let site_name = session.as_ref().map(|s| s.session.tenant_name.clone()).unwrap_or_else(|_| "Strata".into());
    let title = view.as_ref().and_then(|v| v["title"].as_str()).map_or(site_name.clone(), |t| format!("{t} · {site_name}"));
    let description = view
        .as_ref()
        .and_then(|v| v["data"]["site"]["fields"]["tagline"].as_str())
        .unwrap_or("")
        .to_string();
    let theme = view.as_ref().map(|v| v["theme"].clone()).unwrap_or_else(|| json!(theme::Theme::default()));
    let bootstrap = json!({
        "path": path,
        "session": session.ok().map(|reply| json!({ "session": reply.session, "state": reply.state })),
        "view": view,
    })
    .to_string()
    .replace('<', "\\u003c");
    let html = index
        .replace(
            "<html lang=\"en\">",
            &format!(
                "<html lang=\"en\" data-accent=\"{}\" data-paper=\"{}\">",
                theme["accent"].as_str().unwrap_or("moss"),
                theme["paper"].as_str().unwrap_or("linen")
            ),
        )
        .replace("<title>Strata</title>", &format!("<title>{}</title>", escape_html(&title)))
        .replace(
            "<!--strata:head-->",
            &format!("<meta name=\"description\" content=\"{}\">", escape_html(&description)),
        )
        .replace(
            "<!--strata:bootstrap-->",
            &format!("<script type=\"application/json\" id=\"strata-bootstrap\">{bootstrap}</script>"),
        );
    let mut response = (status, html).into_response();
    response.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    response.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Security headers on everything, and one log line per request.
async fn envelope(State(bff): State<Shared>, request: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let correlation = text(request.headers(), HEADER_CORRELATION).filter(|id| valid_correlation(id)).unwrap_or("").to_string();
    bff.in_flight.fetch_add(1, Ordering::Relaxed);
    let mut response = next.run(request).await;
    bff.in_flight.fetch_sub(1, Ordering::Relaxed);
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self'; \
             connect-src 'self'; worker-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; \
             frame-ancestors 'none'",
        ),
    );
    headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    headers.insert("referrer-policy", HeaderValue::from_static("same-origin"));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    if path.starts_with("/api/") {
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    } else if path.starts_with("/assets/") {
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("public, max-age=31536000, immutable"));
    }
    if path.starts_with("/api/") || method != Method::GET {
        // The query string is left out: a search term is the reader's business.
        LOG.info(
            "bff.request",
            &correlation,
            json!({ "method": method.as_str(), "path": path, "status": response.status().as_u16(), "ms": started.elapsed().as_millis() as u64 }),
        );
    }
    response
}

async fn api_not_found(headers: HeaderMap) -> Failure {
    let correlation = text(&headers, HEADER_CORRELATION).unwrap_or("").to_string();
    Failure::new(ErrorCode::NotFound, "No such endpoint.", &correlation)
}

pub fn router(bff: Shared) -> Router {
    let api = Router::new()
        .route("/session", get(session))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/view", get(view))
        .route("/records", post(records))
        .route("/lab/overview", get(lab_overview))
        .route("/lab/run", post(lab_run))
        .route("/lab/runs/:id", get(lab_result))
        .route("/lab/replay", post(lab_replay))
        .route("/lab/verify", post(lab_verify))
        .route("/diagnostics", get(diagnostics))
        .route("/dev/:action", post(dev))
        .fallback(api_not_found);
    Router::new()
        .nest("/api", api)
        .route("/ws", get(socket))
        .nest_service("/assets", ServeDir::new(bff.cfg.dist.join("assets")))
        .nest_service("/media", ServeDir::new(bff.cfg.dist.join("media")))
        .fallback(get(page))
        .layer(middleware::from_fn_with_state(bff.clone(), envelope))
        .with_state(bff)
}
