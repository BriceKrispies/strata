//! The browser-facing edge: cookies, CSRF, origins, site resolution, projection, the socket.
//! A real backend (in-memory store) and a real BFF, talking over loopback.

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value as Json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strata_bff::Config;
use strata_storage_api::memory::MemoryStore;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

const TOKEN: &str = "edge-service-token-0123456789abcdef";
const SESSION_COOKIE: &str = "__Host-strata_sid";

struct Edge {
    port: u16,
    http: reqwest::Client,
    /// The backend's internal API, such as `http://127.0.0.1:4101`.
    backend: String,
    /// Asks the BFF to stop (as a termination signal does).
    stop: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// The BFF's own task, which ends when its server has drained.
    served: std::sync::Mutex<Option<tokio::task::JoinHandle<Result<(), String>>>>,
}

async fn start() -> Edge {
    start_with(|_| {}, |_| {}).await
}

/// Start the edge, with the BFF's and the backend's configuration adjusted by the caller.
async fn start_with(
    adjust_bff: impl FnOnce(&mut Config),
    adjust_backend: impl FnOnce(&mut strata_backend::app::Config),
) -> Edge {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let backend_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_addr = backend_listener.local_addr().unwrap();
    let mut backend_cfg = strata_backend::app::Config {
        addr: backend_addr,
        service_token: TOKEN.into(),
        dev: true,
        tenants_dir: root.join("tenants"),
        lean_dir: root.join("lean"),
        session_ttl_ms: 3_600_000,
        session_idle_ms: 3_600_000,
        passkey_challenge_ttl_ms: 300_000,
        link_ttl_ms: 900_000,
        mail: std::sync::Arc::new(strata_backend::mail::DevSink::memory()),
        federation: std::sync::Arc::new(strata_backend::signin::oidc::Federation::none()),
        factors: Default::default(),
        limits: Default::default(),
        throttle: Default::default(),
    };
    adjust_backend(&mut backend_cfg);
    tokio::spawn(strata_backend::run(
        backend_cfg,
        Arc::new(MemoryStore::new()),
        backend_listener,
        std::future::pending(),
    ));

    let dist =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("dist-{}", std::process::id()));
    std::fs::create_dir_all(dist.join("assets")).unwrap();
    std::fs::write(
        dist.join("index.html"),
        "<!doctype html><html lang=\"en\"><head><title>Strata</title><!--strata:head--></head><body><div id=\"app\"></div><!--strata:bootstrap--></body></html>",
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut cfg = Config {
        addr: listener.local_addr().unwrap(),
        backend: format!("http://{backend_addr}"),
        service_token: TOKEN.into(),
        sites: Config::parse_sites("127.0.0.1=fieldnotes,localhost=tidewater").unwrap(),
        origins: vec![
            format!("http://127.0.0.1:{port}"),
            format!("http://localhost:{port}"),
        ],
        hosts: Default::default(),
        dist,
        dev: true,
        token_secret: None,
        trusted_proxies: Vec::new(),
        limits: Default::default(),
    };
    adjust_bff(&mut cfg);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = tokio::spawn(strata_bff::run(cfg, listener, async move {
        let _ = stopped.await;
    }));
    let edge = Edge {
        port,
        http: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        backend: format!("http://{backend_addr}"),
        stop: std::sync::Mutex::new(Some(stop)),
        served: std::sync::Mutex::new(Some(served)),
    };
    for _ in 0..400 {
        if edge.get("127.0.0.1", "/api/session", None).await.0 == 200 {
            return edge;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the edge did not start");
}

struct Reply {
    cookie: Option<String>,
    headers: reqwest::header::HeaderMap,
    text: String,
}

impl Edge {
    /// Bind an origin to a tenant at the backend, as the operator's tools do while it runs.
    async fn bind(&self, tenant: &str, origin: &str) {
        let response = self
            .http
            .post(format!("{}/internal/platform/hosts", self.backend))
            .header(strata_protocol::api::HEADER_SERVICE, TOKEN)
            .json(&json!({ "bind": { "tenant": tenant, "origin": origin } }))
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        assert_eq!(status, 200, "{}", response.text().await.unwrap());
    }

    /// A request to the BFF for a host name that is not in its static configuration.
    fn as_host(&self, host: &str, path: &str) -> reqwest::RequestBuilder {
        self.http
            .get(format!("{}{path}", self.origin("127.0.0.1")))
            .header("host", host)
    }

    /// Ask the BFF to stop and wait for its server to finish draining; the time it took.
    async fn stop_and_wait(&self) -> Duration {
        let started = std::time::Instant::now();
        let stop = self.stop.lock().unwrap().take().expect("not stopped yet");
        stop.send(()).unwrap();
        let served = self.served.lock().unwrap().take().unwrap();
        tokio::time::timeout(Duration::from_secs(20), served)
            .await
            .expect("the BFF ended before the drain deadline")
            .unwrap()
            .unwrap();
        started.elapsed()
    }

    fn origin(&self, host: &str) -> String {
        format!("http://{host}:{}", self.port)
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> (u16, Json, Reply) {
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let cookie = headers
            .get("set-cookie")
            .map(|value| value.to_str().unwrap().to_string());
        let text = response.text().await.unwrap();
        let body = serde_json::from_str(&text).unwrap_or(Json::Null);
        (
            status,
            body,
            Reply {
                cookie,
                headers,
                text,
            },
        )
    }

    async fn get(&self, host: &str, path: &str, cookie: Option<&str>) -> (u16, Json, Reply) {
        let mut request = self.http.get(format!("{}{path}", self.origin(host)));
        if let Some(cookie) = cookie {
            request = request.header("cookie", format!("{SESSION_COOKIE}={cookie}"));
        }
        self.send(request).await
    }

    fn post(&self, host: &str, path: &str, body: Json) -> reqwest::RequestBuilder {
        self.http
            .post(format!("{}{path}", self.origin(host)))
            .header("origin", self.origin(host))
            .json(&body)
    }

    /// The request token the gateway issues to a browser that has no session.
    async fn request_token(&self, host: &str) -> String {
        let (_, body, _) = self.get(host, "/api/session", None).await;
        body["session"]["csrf_token"]
            .as_str()
            .expect("the gateway issues a request token with the session reply")
            .to_string()
    }

    /// Sign in and return `(cookie token, csrf token)`.
    async fn login(&self, host: &str, login: &str, password: &str) -> (String, String) {
        let request = self
            .post(
                host,
                "/api/login",
                json!({ "login": login, "password": password }),
            )
            .header("x-csrf-token", self.request_token(host).await);
        let (status, body, reply) = self.send(request).await;
        assert_eq!(status, 200, "{body}");
        let cookie = reply.cookie.expect("a session cookie");
        let token = cookie
            .split(';')
            .next()
            .unwrap()
            .trim_start_matches(&format!("{SESSION_COOKIE}="))
            .to_string();
        (
            token,
            body["session"]["csrf_token"].as_str().unwrap().to_string(),
        )
    }
}

/// Verifies: GWY-003
/// Verifies: IDN-262
/// Verifies: H-006
#[tokio::test]
async fn the_session_cookie_is_host_prefixed_http_only_strict_and_never_in_a_body() {
    let edge = start().await;
    let request = edge
        .post(
            "127.0.0.1",
            "/api/login",
            json!({ "login": "ines", "password": "strata-dev-ines" }),
        )
        .header("x-csrf-token", edge.request_token("127.0.0.1").await);
    let (status, body, reply) = edge.send(request).await;
    assert_eq!(status, 200);
    let cookie = reply.cookie.unwrap();
    assert!(
        cookie.contains("HttpOnly")
            && cookie.contains("SameSite=Strict")
            && cookie.contains("Path=/"),
        "{cookie}"
    );
    let token = cookie
        .split(';')
        .next()
        .unwrap()
        .trim_start_matches(&format!("{SESSION_COOKIE}="));
    assert_eq!(token.len(), 64);
    assert!(
        !reply.text.contains(token),
        "the token is only ever in the cookie"
    );
    assert_eq!(body["session"]["display_name"], "Inès Okafor");

    let (_, session, _) = edge.get("127.0.0.1", "/api/session", Some(token)).await;
    assert_eq!(session["session"]["authenticated"], true);
    // An ended session makes the BFF clear the cookie.
    let (csrf_status, _, logout_reply) = edge
        .send(
            edge.post("127.0.0.1", "/api/logout", json!({}))
                .header("cookie", format!("{SESSION_COOKIE}={token}"))
                .header(
                    "x-csrf-token",
                    session["session"]["csrf_token"].as_str().unwrap(),
                ),
        )
        .await;
    assert_eq!(csrf_status, 200);
    // Every cookie the gateway sets carries the attributes: the one that signs in, the one that
    // signs out and the one that clears a session that ended.
    let signed_out = logout_reply.cookie.expect("sign-out sets a cookie");
    assert!(
        signed_out.contains("HttpOnly") && signed_out.contains("SameSite=Strict"),
        "{signed_out}"
    );
    let (_, after, reply) = edge.get("127.0.0.1", "/api/session", Some(token)).await;
    assert_eq!(after["session"]["authenticated"], false);
    let cleared = reply.cookie.unwrap();
    assert!(cleared.contains("Max-Age=0"), "{cleared}");
    assert!(
        cleared.contains("HttpOnly") && cleared.contains("SameSite=Strict"),
        "the cleared cookie keeps its attributes: {cleared}"
    );

    // The name carries the host prefix, which a browser honours only for a cookie that is
    // `Secure`, has `Path=/` and has no `Domain`.
    let attributes: Vec<&str> = cookie.split(';').map(str::trim).collect();
    assert!(attributes[0].starts_with("__Host-strata_sid="), "{cookie}");
    assert!(attributes.contains(&"Secure"), "{cookie}");
    assert!(attributes.contains(&"Path=/"), "{cookie}");
    assert!(
        !attributes
            .iter()
            .any(|a| a.to_lowercase().starts_with("domain")),
        "{cookie}"
    );
    // The sign-out cookie is marked the same way, and an empty token ends the cookie.
    assert!(
        signed_out.starts_with("__Host-strata_sid=;") && signed_out.contains("Secure"),
        "{signed_out}"
    );

    // A cookie under the old, unprefixed name is not a session.
    let (live, _) = edge.login("127.0.0.1", "ines", "strata-dev-ines").await;
    let old_name = edge
        .http
        .get(format!("{}/api/session", edge.origin("127.0.0.1")))
        .header("cookie", format!("strata_sid={live}"));
    let (_, old, _) = edge.send(old_name).await;
    assert_eq!(old["session"]["authenticated"], false);
}

/// The `POST` routes of the browser-facing API, read from its definition so that a route added
/// later is covered without anyone remembering to list it. `:action` stands for the two
/// development actions. The routes outside `/api` are a provider's own posts, which come from
/// another site and carry no session of the person's: they are tested on their own.
fn post_routes() -> Vec<String> {
    // The routes of the `/api` router only: the routes outside it (the OAuth token and
    // introspection endpoints, federation callbacks) authenticate a client or a provider, not a
    // session, so they take no session token.
    let source = include_str!("../src/routes.rs");
    let api = source
        .split("let api = Router::new()")
        .nth(1)
        .and_then(|rest| rest.split(".fallback(api_not_found)").next())
        .expect("the /api router in routes.rs");
    api.lines()
        .filter(|line| {
            line.contains(".route(\"") && line.contains("post(") && !line.contains("\"/federation/")
        })
        .map(|line| line.split('"').nth(1).expect("a route path").to_string())
        .flat_map(|path| {
            // A route with an action in its path answers an action it does not know with 404
            // before it asks for the token, so each is sent the actions that change state there.
            let actions: &[&str] = if path.starts_with("/dev/") {
                &["expire-session", "reset"]
            } else if path.starts_with("/segments/") {
                &["define", "remove", "test"]
            } else if path.starts_with("/inbox/") {
                &["read", "remove", "preferences", "templates"]
            } else if path.starts_with("/automation/commands/") {
                &["start-run", "approve", "activate"]
            } else if path.starts_with("/jobs/") {
                &["schedule", "cancel", "retry"]
            } else if path.starts_with("/products/") {
                &["install", "upgrade", "remove"]
            } else {
                &[]
            };
            if path.contains(":action") {
                actions
                    .iter()
                    .map(|action| path.replace(":action", action))
                    .collect::<Vec<_>>()
            } else {
                vec![path]
            }
        })
        .map(|path| format!("/api{path}"))
        .collect()
}

/// Verifies: GWY-002
#[tokio::test]
async fn every_request_that_changes_state_needs_our_origin_and_the_sessions_token() {
    let edge = start().await;
    let (token, csrf) = edge.login("127.0.0.1", "ines", "strata-dev-ines").await;
    let cookie = format!("{SESSION_COOKIE}={token}");
    let change = json!({ "op": "create", "object": "category", "fields": { "name": "Tools", "slug": "tools" } });
    let write = |origin: Option<String>, csrf: Option<&str>| {
        let mut request = edge
            .http
            .post(format!("{}/api/records", edge.origin("127.0.0.1")))
            .header("cookie", &cookie)
            .json(&change);
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        if let Some(csrf) = csrf {
            request = request.header("x-csrf-token", csrf);
        }
        request
    };
    let ours = edge.origin("127.0.0.1");
    assert_eq!(
        edge.send(write(Some(ours.clone()), None)).await.0,
        403,
        "no CSRF token"
    );
    assert_eq!(
        edge.send(write(Some(ours.clone()), Some("0000"))).await.0,
        403,
        "wrong CSRF token"
    );
    assert_eq!(
        edge.send(write(Some("http://evil.example".into()), Some(&csrf)))
            .await
            .0,
        403,
        "foreign origin"
    );
    assert_eq!(
        edge.send(write(None, Some(&csrf))).await.0,
        403,
        "no origin at all"
    );
    assert_eq!(
        edge.send(write(Some(ours.clone()), Some(&csrf))).await.0,
        200
    );

    // Every other route that changes state: the lab, the development fixtures, sign-in and
    // sign-out as well. Each is sent from our own origin with the session cookie, without the
    // token and with a wrong one, and must be refused before anything happens.
    // A signed address is its own credential and carries no session token (CT-827, scheme
    // signed-address), so the route that takes one is not among them.
    let routes: Vec<String> = post_routes()
        .into_iter()
        .filter(|path| path != "/api/mail/subscription")
        .collect();
    assert!(
        routes.len() >= 8,
        "the routes were not read from the router: {routes:?}"
    );
    let before = edge.get("127.0.0.1", "/api/view?path=/", None).await.1;
    for path in &routes {
        let body = match path.as_str() {
            "/api/records" => change.clone(),
            "/api/login" => json!({ "login": "ines", "password": "strata-dev-ines" }),
            _ => json!({}),
        };
        for presented in [None, Some("0000")] {
            let mut request = edge
                .http
                .post(format!("{}{path}", edge.origin("127.0.0.1")))
                .header("cookie", &cookie)
                .header("origin", &ours)
                .json(&body);
            if let Some(presented) = presented {
                request = request.header("x-csrf-token", presented);
            }
            let (status, reply, _) = edge.send(request).await;
            assert_eq!(
                status, 403,
                "{path} with token {presented:?} answered {status}: {reply}"
            );
        }
    }
    let (_, after, _) = edge.get("127.0.0.1", "/api/view?path=/", None).await;
    assert_eq!(
        after, before,
        "the refused requests changed nothing, the fixture reset least of all"
    );
    let (_, still, _) = edge.get("127.0.0.1", "/api/session", Some(&token)).await;
    assert_eq!(
        still["session"]["authenticated"], true,
        "the refused sign-out and fixtures ended nothing"
    );
}

/// The Markdown preview parses text for whoever asks, so it needs a live session, our origin and
/// the session's request token, like the routes that change state.
/// Verifies: CMS-1020
#[tokio::test]
async fn the_markdown_preview_is_refused_without_a_session() {
    let edge = start().await;
    let body = json!({ "markdown": "a *b*" });
    // No cookie and no token: refused.
    let (status, _, _) = edge
        .send(edge.post("127.0.0.1", "/api/preview", body.clone()))
        .await;
    assert!(matches!(status, 401 | 403), "answered {status}");
    // The request token every visitor is issued, but no session: refused.
    let anonymous = edge
        .post("127.0.0.1", "/api/preview", body.clone())
        .header("x-csrf-token", edge.request_token("127.0.0.1").await);
    let (status, _, _) = edge.send(anonymous).await;
    assert!(matches!(status, 401 | 403), "answered {status}");
    // A session with its own token: answered.
    let (token, csrf) = edge.login("127.0.0.1", "ines", "strata-dev-ines").await;
    let signed_in = edge
        .post("127.0.0.1", "/api/preview", body)
        .header("cookie", format!("{SESSION_COOKIE}={token}"))
        .header("x-csrf-token", csrf);
    let (status, reply, _) = edge.send(signed_in).await;
    assert_eq!(status, 200, "{reply}");
    assert!(reply["blocks"].is_array());
}

/// Verifies: GWY-257
#[tokio::test]
async fn sign_in_and_sign_out_carry_a_request_token_the_gateway_issued() {
    let edge = start().await;
    let credentials = json!({ "login": "ines", "password": "strata-dev-ines" });
    let issued = edge.request_token("127.0.0.1").await;
    let other_site = edge.request_token("localhost").await;
    let mut tampered = issued.clone();
    tampered.pop();
    tampered.push(if issued.ends_with('0') { '1' } else { '0' });

    // Sign-in: nothing but a token this gateway issued for this site is accepted. The fixed
    // header the prototype used to ask for is not a token.
    let sign_in = |origin: &str, header: Option<(&str, &str)>| {
        let mut request = edge
            .http
            .post(format!("{}/api/login", edge.origin("127.0.0.1")))
            .header("origin", origin)
            .json(&credentials);
        if let Some((name, value)) = header {
            request = request.header(name, value);
        }
        request
    };
    let ours = edge.origin("127.0.0.1");
    for (what, header) in [
        ("no token", None),
        ("the fixed header alone", Some(("x-strata-request", "1"))),
        ("a made-up token", Some(("x-csrf-token", "0000"))),
        ("an empty token", Some(("x-csrf-token", ""))),
        (
            "another site's token",
            Some(("x-csrf-token", other_site.as_str())),
        ),
        (
            "a tampered token",
            Some(("x-csrf-token", tampered.as_str())),
        ),
    ] {
        let (status, _, reply) = edge.send(sign_in(&ours, header)).await;
        assert_eq!(status, 403, "sign-in with {what}");
        assert!(reply.cookie.is_none(), "{what} was given a session");
    }
    let (status, _, _) = edge
        .send(sign_in(
            "http://evil.example",
            Some(("x-csrf-token", issued.as_str())),
        ))
        .await;
    assert_eq!(status, 403, "a token from a foreign origin");
    let (status, _, reply) = edge
        .send(sign_in(&ours, Some(("x-csrf-token", issued.as_str()))))
        .await;
    assert_eq!(status, 200, "the issued token signs in");
    let cookie = reply.cookie.expect("a session cookie");
    let session = cookie
        .split(';')
        .next()
        .unwrap()
        .trim_start_matches(&format!("{SESSION_COOKIE}="))
        .to_string();

    // Sign-out of a signed-in browser: the token of its session, and not the one issued to
    // anonymous browsers.
    let sign_out = |header: Option<&str>| {
        let mut request = edge
            .post("127.0.0.1", "/api/logout", json!({}))
            .header("cookie", format!("{SESSION_COOKIE}={session}"));
        if let Some(value) = header {
            request = request.header("x-csrf-token", value);
        }
        request
    };
    let (_, current, _) = edge.get("127.0.0.1", "/api/session", Some(&session)).await;
    let csrf = current["session"]["csrf_token"]
        .as_str()
        .unwrap()
        .to_string();
    for (what, header) in [
        ("no token", None),
        ("a made-up token", Some("0000")),
        (
            "the token issued to anonymous browsers",
            Some(issued.as_str()),
        ),
    ] {
        assert_eq!(
            edge.send(sign_out(header)).await.0,
            403,
            "sign-out with {what}"
        );
        let (_, still, _) = edge.get("127.0.0.1", "/api/session", Some(&session)).await;
        assert_eq!(still["session"]["authenticated"], true, "{what} ended it");
    }
    assert_eq!(edge.send(sign_out(Some(&csrf))).await.0, 200);
    let (_, after, _) = edge.get("127.0.0.1", "/api/session", Some(&session)).await;
    assert_eq!(after["session"]["authenticated"], false);

    // Sign-out of a browser that has no session still carries the token the gateway issued.
    let anonymous_out = |header: Option<(&str, &str)>| {
        let mut request = edge.post("127.0.0.1", "/api/logout", json!({}));
        if let Some((name, value)) = header {
            request = request.header(name, value);
        }
        request
    };
    for (what, header) in [
        ("no token", None),
        ("the fixed header alone", Some(("x-strata-request", "1"))),
        (
            "another site's token",
            Some(("x-csrf-token", other_site.as_str())),
        ),
    ] {
        assert_eq!(
            edge.send(anonymous_out(header)).await.0,
            403,
            "anonymous sign-out with {what}"
        );
    }
    assert_eq!(
        edge.send(anonymous_out(Some(("x-csrf-token", issued.as_str()))))
            .await
            .0,
        200
    );
}

/// Verifies: GWY-001
#[tokio::test]
async fn the_site_comes_from_the_host_and_nothing_the_client_says() {
    let edge = start().await;
    let (_, fieldnotes, _) = edge.get("127.0.0.1", "/api/view?path=/", None).await;
    assert_eq!(fieldnotes["data"]["site"]["fields"]["name"], "Fieldnotes");
    assert_eq!(
        fieldnotes["theme"],
        json!({ "accent": "moss", "paper": "linen" })
    );
    let (_, tidewater, _) = edge.get("localhost", "/api/view?path=/", None).await;
    assert_eq!(
        tidewater["data"]["site"]["fields"]["name"],
        "Tidewater Review"
    );
    assert_eq!(tidewater["theme"]["accent"], "ink");

    // Internal headers and parameters from a browser are not a way in: the host alone names the
    // tenant (GWY-001).
    let spoof = edge
        .http
        .get(format!(
            "{}/api/view?path=/&site=tidewater",
            edge.origin("127.0.0.1")
        ))
        .header("x-strata-site", "tidewater")
        .header("x-strata-service", TOKEN);
    let (_, spoofed, _) = edge.send(spoof).await;
    assert_eq!(spoofed["data"]["site"]["fields"]["name"], "Fieldnotes");
    // A view request that names another tenant is refused, not served from it (read-view's tenant
    // scope is the request's).
    let (status, named, _) = edge
        .get("127.0.0.1", "/api/view?path=/&tenant=tidewater", None)
        .await;
    assert_eq!(status, 403, "{named}");
    assert_eq!(named["code"], "forbidden");

    // A fieldnotes session cookie replayed against the tidewater host is nobody there.
    let (token, _) = edge.login("127.0.0.1", "admin", "strata-dev-admin").await;
    let (_, there, _) = edge.get("localhost", "/api/session", Some(&token)).await;
    assert_eq!(there["session"]["authenticated"], false);
    // The desk is the signed-in's (contract read-desk-view): the cookie is no session there, so
    // the view is refused and shows nothing.
    let (desk_status, desk, _) = edge
        .get("localhost", "/api/view?path=/desk", Some(&token))
        .await;
    assert_eq!(desk_status, 401, "{desk}");
    assert_eq!(desk["code"], "unauthenticated");
    assert!(!edge
        .get("127.0.0.1", "/api/view?path=/desk", Some(&token))
        .await
        .2
        .text
        .contains("Tidewater"));

    let unknown = edge
        .http
        .get(format!("{}/api/session", edge.origin("127.0.0.1")))
        .header("host", "elsewhere.example");
    assert_eq!(edge.send(unknown).await.0, 404);

    // Nothing else a client sends names a tenant either: the headers proxies use for the host,
    // the origin and the referrer, other parameters, and a field in a sign-in body.
    for (name, value) in [
        ("x-forwarded-host", "localhost"),
        ("x-original-host", "localhost"),
        ("x-forwarded-server", "localhost"),
        ("forwarded", "host=localhost"),
        ("origin", "http://localhost"),
        ("referer", "http://localhost/"),
        ("x-tenant", "tidewater"),
        ("x-site", "tidewater"),
        ("x-strata-tenant", "tidewater"),
    ] {
        let request = edge
            .http
            .get(format!(
                "{}/api/view?path=/&host=localhost",
                edge.origin("127.0.0.1")
            ))
            .header(name, value);
        let (_, view, _) = edge.send(request).await;
        assert_eq!(
            view["data"]["site"]["fields"]["name"], "Fieldnotes",
            "{name}: {value} moved the request to another tenant"
        );
    }
    let sneaky = edge
        .post(
            "127.0.0.1",
            "/api/login",
            json!({ "login": "ines", "password": "strata-dev-ines", "tenant": "tidewater", "site": "tidewater" }),
        )
        .header("x-csrf-token", edge.request_token("127.0.0.1").await);
    let (status, body, _) = edge.send(sneaky).await;
    assert!(
        status != 200 || body["session"]["tenant"] == "fieldnotes",
        "a tenant named in the sign-in body was honoured: {status} {body}"
    );
}

/// Verifies: GWY-005
/// Verifies: GWY-253
/// Verifies: GWY-252
///
/// Every page response carries the same policy: scripts from the page's own origin only, no
/// inline script, and Trusted Types for script sinks naming exactly one policy, the kernel
/// worker's address, with no default policy. The base, object and form directives stay on the
/// page's own origin or on none.
#[tokio::test]
async fn pages_carry_security_headers_and_a_bootstrap_that_cannot_break_out() {
    let edge = start().await;
    // Pages of different kinds, of both tenants and the page that says nothing is there: the
    // policy is the same on each.
    for (host, path, expected) in [
        ("127.0.0.1", "/posts/a-margin-is-a-promise", 200),
        ("127.0.0.1", "/", 200),
        ("127.0.0.1", "/desk", 200),
        ("localhost", "/", 200),
        ("127.0.0.1", "/no/such/page", 404),
    ] {
        let (status, _, reply) = edge.get(host, path, None).await;
        assert_eq!(status, expected, "{path}");
        let csp = reply
            .headers
            .get("content-security-policy")
            .unwrap()
            .to_str()
            .unwrap();
        // The script directive is exactly 'self': no inline script, no hash, no nonce, no
        // other origin, and no separate element or attribute directive that could allow inline.
        let script_src = csp
            .split(';')
            .map(str::trim)
            .find(|d| d.starts_with("script-src"))
            .unwrap_or("");
        assert_eq!(script_src, "script-src 'self'", "{path}: {csp}");
        assert!(
            !csp.contains("script-src-elem") && !csp.contains("script-src-attr"),
            "{path}: {csp}"
        );
        assert!(csp.contains("frame-ancestors 'none'"), "{path}: {csp}");
        assert!(
            !csp.contains("unsafe-inline") && !csp.contains("unsafe-eval"),
            "{path}: {csp}"
        );
        let directives: Vec<&str> = csp.split(';').map(str::trim).collect();
        assert!(
            directives.contains(&"require-trusted-types-for 'script'"),
            "{path}: {csp}"
        );
        assert!(
            directives.contains(&"trusted-types strata-worker"),
            "{path}: {csp}"
        );
        assert!(
            !csp.contains("'allow-duplicates'") && !csp.contains("default'"),
            "{path}: {csp}"
        );
        for required in ["base-uri 'none'", "object-src 'none'", "form-action 'self'"] {
            assert!(
                directives.contains(&required),
                "{path}: {required} in {csp}"
            );
        }
    }
    let (_, _, reply) = edge
        .get("127.0.0.1", "/posts/a-margin-is-a-promise", None)
        .await;
    assert_eq!(
        reply.headers.get("x-content-type-options").unwrap(),
        "nosniff"
    );
    assert!(reply
        .text
        .contains("<title>A Margin Is a Promise — Fieldnotes</title>"));
    assert!(reply.text.contains("data-accent=\"moss\""));
    assert!(reply.text.contains("id=\"strata-bootstrap\""));
    let data = reply
        .text
        .split("id=\"strata-bootstrap\">")
        .nth(1)
        .unwrap()
        .split("</script>")
        .next()
        .unwrap();
    assert!(
        !data.contains('<'),
        "no markup can start inside the data block"
    );
    let bootstrap: Json = serde_json::from_str(data).unwrap();
    assert_eq!(
        bootstrap["view"]["data"]["post"]["fields"]["slug"],
        "a-margin-is-a-promise"
    );
    assert_eq!(bootstrap["session"]["session"]["authenticated"], false);
}

type Upgrade = Result<
    (
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        tokio_tungstenite::tungstenite::handshake::client::Response,
    ),
    tokio_tungstenite::tungstenite::Error,
>;

/// Try to open the browser socket with the given origin, session cookie and request token.
async fn connect(
    edge: &Edge,
    origin: Option<String>,
    cookie: Option<&str>,
    csrf: Option<&str>,
) -> Upgrade {
    connect_with(edge, origin, cookie, csrf, &[]).await
}

/// `connect`, with extra request headers.
async fn connect_with(
    edge: &Edge,
    origin: Option<String>,
    cookie: Option<&str>,
    csrf: Option<&str>,
    extra: &[(&'static str, &str)],
) -> Upgrade {
    let address = match csrf {
        Some(csrf) => format!("ws://127.0.0.1:{}/ws?csrf={csrf}", edge.port),
        None => format!("ws://127.0.0.1:{}/ws", edge.port),
    };
    let mut request = address.into_client_request().unwrap();
    if let Some(origin) = origin {
        request
            .headers_mut()
            .insert("origin", origin.parse().unwrap());
    }
    if let Some(cookie) = cookie {
        request.headers_mut().insert(
            "cookie",
            format!("{SESSION_COOKIE}={cookie}").parse().unwrap(),
        );
    }
    for (name, value) in extra {
        request.headers_mut().insert(*name, value.parse().unwrap());
    }
    tokio_tungstenite::connect_async(request).await
}

/// Verifies: GWY-004
#[tokio::test]
async fn the_socket_checks_origin_validates_frames_and_relays_events() {
    let edge = start().await;
    // Every refusal below carries a valid request token, so it is the origin that is refused.
    let anonymous = edge.request_token("127.0.0.1").await;
    assert!(
        connect(
            &edge,
            Some("http://evil.example".into()),
            None,
            Some(anonymous.as_str())
        )
        .await
        .is_err(),
        "foreign origin is refused at upgrade"
    );
    assert!(
        connect(&edge, None, None, Some(anonymous.as_str()))
            .await
            .is_err(),
        "so is a missing origin"
    );
    for near_miss in [
        format!("https://127.0.0.1:{}", edge.port),
        format!("http://127.0.0.1.evil.example:{}", edge.port),
        format!("http://127.0.0.1:{}", edge.port.wrapping_add(1)),
        "null".to_string(),
    ] {
        assert!(
            connect(
                &edge,
                Some(near_miss.clone()),
                None,
                Some(anonymous.as_str())
            )
            .await
            .is_err(),
            "origin {near_miss} was accepted"
        );
    }
    assert!(
        connect(
            &edge,
            Some(edge.origin("127.0.0.1")),
            None,
            Some(anonymous.as_str())
        )
        .await
        .is_ok(),
        "the control: our own origin with the token is let in"
    );

    let (token, csrf) = edge.login("127.0.0.1", "ines", "strata-dev-ines").await;
    let (mut socket, _) = connect(
        &edge,
        Some(edge.origin("127.0.0.1")),
        Some(token.as_str()),
        Some(csrf.as_str()),
    )
    .await
    .unwrap();
    async fn next(
        socket: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
                  + Unpin),
    ) -> Json {
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            if let Message::Text(text) = frame {
                return serde_json::from_str(&text).unwrap();
            }
        }
    }
    let welcome = next(&mut socket).await;
    assert_eq!(
        (welcome["type"].as_str(), welcome["session"].as_str()),
        (Some("welcome"), Some("active"))
    );
    socket
        .send(Message::Text(
            json!({ "type": "become", "principal": "user:admin" }).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(
        next(&mut socket).await["type"],
        "error",
        "the relay refuses frames outside the protocol"
    );
    socket
        .send(Message::Text(
            json!({ "type": "subscribe", "req": "a", "sub": "s" }).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(next(&mut socket).await, json!({ "type": "ok", "req": "a" }));

    let change = json!({ "op": "create", "object": "category", "fields": { "name": "Live", "slug": "live" } });
    let write = edge
        .post("127.0.0.1", "/api/records", change)
        .header("cookie", format!("{SESSION_COOKIE}={token}"))
        .header("x-csrf-token", &csrf)
        .header("x-correlation-id", "c-browser-42");
    assert_eq!(edge.send(write).await.0, 200);
    let event = next(&mut socket).await;
    assert_eq!(event["notice"]["kind"], "upsert");
    assert_eq!(
        event["correlation"], "c-browser-42",
        "the browser's correlation id comes back with the event"
    );
}

/// Verifies: GWY-256
#[tokio::test]
async fn a_socket_upgrade_without_the_request_token_of_its_session_is_refused() {
    let edge = start().await;
    let ours = || Some(edge.origin("127.0.0.1"));

    // A visitor with no session: the token the gateway issued to the browser, and no other.
    let anonymous = edge.request_token("127.0.0.1").await;
    let other_site = edge.request_token("localhost").await;
    let mut forged = anonymous.clone();
    forged.replace_range(0..1, if anonymous.starts_with('9') { "8" } else { "9" });
    for (what, token) in [
        ("no token", None),
        ("an empty token", Some("")),
        ("a made-up token", Some("0000")),
        ("a token of another site", Some(other_site.as_str())),
        ("a tampered token", Some(forged.as_str())),
    ] {
        assert!(
            connect(&edge, ours(), None, token).await.is_err(),
            "{what} was let in"
        );
    }
    assert!(
        connect(&edge, ours(), None, Some(anonymous.as_str()))
            .await
            .is_ok(),
        "the issued token opens a socket for a visitor"
    );

    // A signed-in browser: the token of its own session. The cookie alone, the token the gateway
    // issued to anonymous browsers, and another session's token are all refused.
    let (token, csrf) = edge.login("127.0.0.1", "ines", "strata-dev-ines").await;
    let (_, admin_csrf) = edge.login("127.0.0.1", "admin", "strata-dev-admin").await;
    for (what, presented) in [
        ("the cookie alone", None),
        ("the anonymous token", Some(anonymous.as_str())),
        ("another session's token", Some(admin_csrf.as_str())),
        ("a made-up token", Some("0000")),
    ] {
        assert!(
            connect(&edge, ours(), Some(token.as_str()), presented)
                .await
                .is_err(),
            "{what} was let in for a signed-in cookie"
        );
    }
    assert!(
        connect(&edge, ours(), Some(token.as_str()), Some(csrf.as_str()))
            .await
            .is_ok(),
        "the session's own token opens the socket"
    );
}

/// Verifies: GWY-1408
/// The gateway's part of the diagnostics names no other site and no configuration: not the host
/// map and not the allowed origins.
#[tokio::test]
async fn diagnostics_name_no_other_site() {
    let edge = start().await;
    let (token, _) = edge.login("127.0.0.1", "admin", "strata-dev-admin").await;
    let (status, report, _) = edge
        .get("127.0.0.1", "/api/diagnostics", Some(&token))
        .await;
    assert_eq!(status, 200, "{report}");
    let text = report.to_string();
    for other in ["tidewater", "localhost", "allowed_origins", "\"sites\""] {
        assert!(!text.contains(other), "{other} is in the report: {text}");
    }
    assert!(
        report.get("bff").is_none(),
        "the gateway's counts span every tenant: {report}"
    );
    assert!(report["backend"].is_object());
}

/// Verifies: GWY-1403
/// Assets are cached for a year only when they were served; source maps are not served at all.
#[tokio::test]
async fn assets_are_cached_only_on_success_and_source_maps_are_not_served() {
    let edge = start_with(
        |cfg| {
            std::fs::write(cfg.dist.join("assets/app-abc.js"), "export {};").unwrap();
            std::fs::write(cfg.dist.join("assets/app-abc.js.map"), "{}").unwrap();
        },
        |_| {},
    )
    .await;
    let (status, _, reply) = edge.get("127.0.0.1", "/assets/app-abc.js", None).await;
    assert_eq!(status, 200);
    assert!(reply.headers["cache-control"]
        .to_str()
        .unwrap()
        .contains("immutable"));
    let (status, _, reply) = edge.get("127.0.0.1", "/assets/missing.js", None).await;
    assert_eq!(status, 404);
    assert!(
        reply
            .headers
            .get("cache-control")
            .map_or(true, |v| !v.to_str().unwrap().contains("immutable")),
        "a failure is not cached for a year"
    );
    let (status, _, _) = edge.get("127.0.0.1", "/assets/app-abc.js.map", None).await;
    assert_eq!(
        status, 404,
        "the map exists on disk and is still not served"
    );
}

/// Verifies: GWY-1404
/// Transport headers: Strict-Transport-Security and `upgrade-insecure-requests` where the origin
/// is https, Permissions-Policy everywhere.
#[tokio::test]
async fn an_https_origin_gets_strict_transport_security_and_a_plain_one_does_not() {
    let plain = start().await;
    let (_, _, reply) = plain.get("127.0.0.1", "/", None).await;
    assert!(reply.headers.get("strict-transport-security").is_none());
    assert!(reply.headers.get("permissions-policy").is_some());
    let secure = start_with(|cfg| cfg.origins.push("https://127.0.0.1".into()), |_| {}).await;
    let (_, _, reply) = secure.get("127.0.0.1", "/", None).await;
    assert!(reply.headers["strict-transport-security"]
        .to_str()
        .unwrap()
        .contains("max-age="));
    assert!(reply.headers["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("upgrade-insecure-requests"));
    assert!(reply.headers.get("permissions-policy").is_some());
}

/// Verifies: GWY-1405
/// Page requests are limited per source at the edge.
#[tokio::test]
async fn page_requests_are_limited_per_source() {
    let edge = start_with(|cfg| cfg.limits.requests_per_minute = 5, |_| {}).await;
    let mut refused = 0;
    for _ in 0..12 {
        if edge.get("127.0.0.1", "/", None).await.0 == 429 {
            refused += 1;
        }
    }
    assert!(refused >= 1, "a source that keeps asking is slowed");
    let (status, _, _) = edge.get("127.0.0.1", "/api/session", None).await;
    assert_eq!(status, 200, "the session read is not a page request");
}

// Verifies: SRH-2790
/// A search is answered through the backend for anyone, and is limited per source at the edge as a
/// page is.
#[tokio::test]
async fn searches_are_limited_per_source() {
    let edge = start_with(|cfg| cfg.limits.requests_per_minute = 5, |_| {}).await;
    let (status, body, _) = edge.get("127.0.0.1", "/api/search?q=kiln", None).await;
    assert_eq!(status, 200, "{body}");
    assert!(body["hits"].as_array().is_some_and(|hits| !hits.is_empty()));
    let mut refused = 0;
    for _ in 0..12 {
        if edge.get("127.0.0.1", "/api/search?q=kiln", None).await.0 == 429 {
            refused += 1;
        }
    }
    assert!(refused >= 1, "a source that keeps searching is slowed");
    let (status, _, _) = edge.get("127.0.0.1", "/api/search?q=", None).await;
    assert!(status == 400 || status == 429, "{status}");
}

/// One sign-in attempt through the edge, from a browser that says it comes from `claimed`.
async fn attempt_from(
    edge: &Edge,
    claimed: Option<&str>,
    login: &str,
    password: &str,
) -> (u16, Json, Reply) {
    let mut request = edge
        .post(
            "127.0.0.1",
            "/api/login",
            json!({ "login": login, "password": password }),
        )
        .header("x-csrf-token", edge.request_token("127.0.0.1").await);
    if let Some(claimed) = claimed {
        request = request.header("x-strata-source", claimed);
    }
    edge.send(request).await
}

/// Verifies: IDN-008
/// Verifies: IDN-255
#[tokio::test]
async fn a_throttled_sign_in_is_answered_the_same_for_an_account_that_does_not_exist() {
    let edge = start().await;
    let mut wrong = Vec::new();
    for login in ["tomasz", "nobody-at-all"] {
        for _ in 0..10 {
            let (status, body, reply) = attempt_from(&edge, None, login, "wrong").await;
            assert_eq!(status, 401);
            assert!(reply.cookie.is_none());
            wrong.push(body["message"].clone());
        }
    }
    assert!(wrong.windows(2).all(|pair| pair[0] == pair[1]), "{wrong:?}");
    let mut refused = Vec::new();
    for login in ["tomasz", "nobody-at-all"] {
        let (status, body, reply) = attempt_from(&edge, None, login, "strata-dev-tomasz").await;
        assert_eq!(status, 429, "{login}: {body}");
        assert!(reply.cookie.is_none(), "a refused sign-in sets no cookie");
        refused.push((body["code"].clone(), body["message"].clone()));
    }
    assert_eq!(
        refused[0], refused[1],
        "the refusal does not say which exists"
    );
}

/// Verifies: GWY-1680
/// A tenant bound at the backend while the BFF runs is served on its next request that names the
/// host, with its origin accepted, and the static configuration is never moved by what the
/// backend says.
#[tokio::test]
async fn a_tenant_bound_at_run_time_is_served_without_a_restart() {
    let edge = start_with(
        |cfg| cfg.hosts = strata_bff::hosts::Refresh::new(60_000, 0),
        |_| {},
    )
    .await;
    let (status, _, _) = edge
        .send(edge.as_host("tide.example", "/api/session"))
        .await;
    assert_eq!(status, 404, "no site is served at an unbound host");

    let tide_origin = format!("http://tide.example:{}", edge.port);
    edge.bind("tidewater", &tide_origin).await;
    let (status, view, _) = edge
        .send(edge.as_host("tide.example", "/api/view?path=/"))
        .await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["data"]["site"]["fields"]["name"], "Tidewater Review");

    // Its origin is one of ours for that site and for no other, and a foreign one is still refused.
    let token = edge
        .send(edge.as_host("tide.example", "/api/session"))
        .await
        .1["session"]["csrf_token"]
        .as_str()
        .expect("a request token")
        .to_string();
    let sign_in = |origin: &str| {
        edge.http
            .post(format!("{}/api/login", edge.origin("127.0.0.1")))
            .header("host", "tide.example")
            .header("origin", origin)
            .header("x-csrf-token", &token)
            .json(&json!({ "login": "nobody", "password": "not-a-password" }))
    };
    let (status, _, _) = edge.send(sign_in(&tide_origin)).await;
    assert_ne!(status, 403, "the learned origin was refused");
    let (status, _, _) = edge
        .send(sign_in(&format!("http://other.example:{}", edge.port)))
        .await;
    assert_eq!(status, 403, "an origin nobody named was accepted");

    // The backend binds a host the static configuration already serves to another tenant: the
    // static entry wins.
    edge.bind("tidewater", &edge.origin("127.0.0.1")).await;
    let _ = edge
        .send(edge.as_host("unknown.example", "/api/session"))
        .await;
    let (_, view, _) = edge.get("127.0.0.1", "/api/view?path=/", None).await;
    assert_eq!(view["data"]["site"]["fields"]["name"], "Fieldnotes");
}

/// Verifies: GWY-1681
/// Hosts nobody knows cannot make the BFF ask the backend more than once per interval.
#[tokio::test]
async fn unknown_hosts_do_not_make_the_backend_be_asked_again_within_the_interval() {
    let edge = start_with(
        |cfg| cfg.hosts = strata_bff::hosts::Refresh::new(60_000, 600_000),
        |_| {},
    )
    .await;
    edge.bind("tidewater", &format!("http://tide.example:{}", edge.port))
        .await;
    for _ in 0..5 {
        let (status, _, _) = edge
            .send(edge.as_host("tide.example", "/api/session"))
            .await;
        assert_eq!(
            status, 404,
            "the backend was asked again inside the interval"
        );
    }
}

/// Verifies: GWY-1682
/// On a stop, an open socket is sent a Close frame and the server ends before its drain deadline.
#[tokio::test]
async fn an_open_socket_is_closed_when_the_server_is_stopped() {
    let edge = start().await;
    let anonymous = edge.request_token("127.0.0.1").await;
    let (mut socket, _) = connect(
        &edge,
        Some(edge.origin("127.0.0.1")),
        None,
        Some(anonymous.as_str()),
    )
    .await
    .unwrap();
    // The socket is open and the backend has welcomed it.
    let welcome = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(welcome, Message::Text(_)), "{welcome:?}");

    let waited = edge.stop_and_wait();
    let closed = async {
        loop {
            match tokio::time::timeout(Duration::from_secs(10), socket.next()).await {
                Ok(Some(Ok(Message::Close(frame)))) => break frame,
                Ok(Some(Ok(_))) => continue,
                other => panic!("the socket ended without a Close frame: {other:?}"),
            }
        }
    };
    let (took, frame) = tokio::join!(waited, closed);
    assert_eq!(
        frame.expect("a close frame with a code").code,
        tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Away
    );
    assert!(
        took < Duration::from_secs(15),
        "the drain took {took:?}, which is the deadline and not the close"
    );
}

/// Verifies: GWY-1683
/// Outside development, a request with `X-Forwarded-For` and no trusted proxy configured is
/// refused with the reason named; in development it is served.
#[tokio::test]
async fn forwarded_headers_without_trusted_proxies_are_refused_outside_development() {
    let forwarded = |edge: &Edge| {
        edge.http
            .get(format!("{}/api/session", edge.origin("127.0.0.1")))
            .header("x-forwarded-for", "203.0.113.9")
    };
    let production = start_with(|cfg| cfg.dev = false, |_| {}).await;
    let (status, _, reply) = production.send(forwarded(&production)).await;
    assert_eq!(status, 400, "{}", reply.text);
    assert!(
        reply.text.contains("forwarded_without_trusted_proxies"),
        "the refusal names its reason: {}",
        reply.text
    );
    assert_eq!(
        production.get("127.0.0.1", "/api/session", None).await.0,
        200,
        "a request without the header is served"
    );

    let behind = start_with(
        |cfg| {
            cfg.dev = false;
            cfg.trusted_proxies = vec!["127.0.0.1".parse().unwrap()];
        },
        |_| {},
    )
    .await;
    assert_eq!(behind.send(forwarded(&behind)).await.0, 200);

    let development = start().await;
    assert_eq!(development.send(forwarded(&development)).await.0, 200);
}

/// Verifies: INT-3254, INT-3255
///
/// A product that declares its interface is installed into the first site; the gateway serves the
/// manifest of that site's interface definition and each part by its hash, kept by the browser as
/// immutable, and the view of the product's screen is titled as the screen states. The other site's
/// definition holds none of it, and a part of the first site is not found from the second.
#[tokio::test]
async fn a_tenants_interface_definition_is_served_by_hash_to_its_own_site_only() {
    let edge = start().await;
    let (owner, csrf) = edge.login("127.0.0.1", "admin", "strata-dev-admin").await;
    let bundle: Json = serde_json::from_str(include_str!(
        "../../spec-compiler/tests/fixtures/products/expected/ticket-desk.bundle.json"
    ))
    .unwrap();
    let (status, installed, _) = edge
        .send(
            edge.post(
                "127.0.0.1",
                "/api/products/install",
                json!({ "bundle": bundle }),
            )
            .header("cookie", format!("{SESSION_COOKIE}={owner}"))
            .header("x-csrf-token", &csrf),
        )
        .await;
    assert_eq!(status, 200, "{installed}");

    // Every visitor of the site routes with the same definition: no session is needed.
    let (status, manifest, reply) = edge.get("127.0.0.1", "/api/ui", None).await;
    assert_eq!(status, 200, "{manifest}");
    assert_eq!(
        reply.headers.get("cache-control").unwrap(),
        "no-store",
        "the manifest changes with the schema"
    );
    let screen = manifest["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == "ticket_desk__board_screen")
        .cloned()
        .expect("the manifest lists the product's screen");
    assert_eq!(screen["route"], "/tickets");
    assert_eq!(screen["view"], "ticket_desk__board");
    let hash = screen["hash"].as_str().unwrap().to_string();

    let (status, part, reply) = edge
        .get("127.0.0.1", &format!("/api/ui/parts/{hash}"), None)
        .await;
    assert_eq!(status, 200, "{part}");
    assert_eq!(
        reply.headers.get("cache-control").unwrap(),
        "private, max-age=31536000, immutable"
    );
    assert_eq!(part["id"], "ticket_desk__board_screen");
    assert_eq!(
        part["forms"]["ticket_desk__open_form"]["operation"],
        "ticket_desk__open_ticket"
    );

    // The second site has no such screen, and does not serve the first site's part.
    let (status, other, _) = edge.get("localhost", "/api/ui", None).await;
    assert_eq!(status, 200, "{other}");
    assert!(
        other["parts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["id"] != "ticket_desk__board_screen"),
        "{other}"
    );
    let (status, _, reply) = edge
        .get("localhost", &format!("/api/ui/parts/{hash}"), None)
        .await;
    assert_eq!(status, 404);
    assert_eq!(reply.headers.get("cache-control").unwrap(), "no-store");
    let (status, _, _) = edge
        .get(
            "127.0.0.1",
            &format!("/api/ui/parts/{}", "0".repeat(64)),
            None,
        )
        .await;
    assert_eq!(
        status, 404,
        "a hash the definition does not hold is no part"
    );

    // The view of the product's screen carries the title the screen states, not the view's own.
    let (status, view, _) = edge
        .get("127.0.0.1", "/api/view?path=/tickets", Some(&owner))
        .await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["view"], "ticket_desk__board");
    assert_eq!(view["title"], "Open tickets");
}

/// Sign in as a person of the first site, presenting `token` as the request token; the status.
async fn login_with(edge: &Edge, token: String) -> u16 {
    edge.send(
        edge.post(
            "127.0.0.1",
            "/api/login",
            json!({ "login": "ines", "password": "strata-dev-ines" }),
        )
        .header("x-csrf-token", token),
    )
    .await
    .0
}

/// Verifies: GWY-1402
/// A request token issued by one gateway is good at a second instance configured with the same
/// secret and at a restart of the first, and worthless at a gateway with another secret.
#[tokio::test]
async fn a_request_token_survives_a_restart_and_is_good_at_a_second_instance() {
    let shared =
        |cfg: &mut Config| cfg.token_secret = Some("one secret held by every instance".into());
    let first = start_with(shared, |_| {}).await;
    let token = first.request_token("127.0.0.1").await;
    assert_eq!(login_with(&first, token.clone()).await, 200);

    let second = start_with(shared, |_| {}).await;
    assert_eq!(
        login_with(&second, token.clone()).await,
        200,
        "a second instance accepts it"
    );

    first.stop_and_wait().await;
    let restarted = start_with(shared, |_| {}).await;
    assert_eq!(
        login_with(&restarted, token.clone()).await,
        200,
        "a restart accepts it"
    );

    let other = start_with(
        |cfg| cfg.token_secret = Some("a different secret entirely".into()),
        |_| {},
    )
    .await;
    assert_ne!(
        login_with(&other, token).await,
        200,
        "another secret does not"
    );
}
