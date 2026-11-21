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
#[tokio::test]
async fn the_session_cookie_is_http_only_strict_and_never_in_a_body() {
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
}

/// Verifies: IDN-262
/// Verifies: H-006
#[tokio::test]
async fn the_session_cookie_carries_the_host_prefix_and_secure() {
    let edge = start().await;
    let request = edge
        .post(
            "127.0.0.1",
            "/api/login",
            json!({ "login": "ines", "password": "strata-dev-ines" }),
        )
        .header("x-csrf-token", edge.request_token("127.0.0.1").await);
    let (status, _, reply) = edge.send(request).await;
    assert_eq!(status, 200);
    let cookie = reply.cookie.unwrap();
    // `__Host-` is only honoured by a browser for a cookie that is `Secure`, has `Path=/` and has
    // no `Domain`; the prefix, `Secure` and the other two are all there.
    let attributes: Vec<&str> = cookie.split(';').map(str::trim).collect();
    assert!(attributes[0].starts_with("__Host-strata_sid="), "{cookie}");
    assert!(attributes.contains(&"Secure"), "{cookie}");
    assert!(attributes.contains(&"Path=/"), "{cookie}");
    assert!(attributes.contains(&"HttpOnly"), "{cookie}");
    assert!(attributes.contains(&"SameSite=Strict"), "{cookie}");
    assert!(
        !attributes
            .iter()
            .any(|a| a.to_lowercase().starts_with("domain")),
        "{cookie}"
    );
    let token = attributes[0].trim_start_matches("__Host-strata_sid=");

    // The attribute is not an option: the cookie the gateway clears is marked the same way.
    let (_, session, _) = edge.get("127.0.0.1", "/api/session", Some(token)).await;
    let logout = edge
        .post("127.0.0.1", "/api/logout", json!({}))
        .header("cookie", format!("{SESSION_COOKIE}={token}"))
        .header(
            "x-csrf-token",
            session["session"]["csrf_token"].as_str().unwrap(),
        );
    let (status, _, reply) = edge.send(logout).await;
    assert_eq!(status, 200);
    let cleared = reply.cookie.unwrap();
    assert!(
        cleared.starts_with("__Host-strata_sid=;") && cleared.contains("Secure"),
        "an empty token ends the cookie: {cleared}"
    );
    assert!(cleared.contains("Max-Age=0"), "{cleared}");

    // A cookie under the old, unprefixed name is not a session.
    let (token, _) = edge.login("127.0.0.1", "ines", "strata-dev-ines").await;
    let old_name = edge
        .http
        .get(format!("{}/api/session", edge.origin("127.0.0.1")))
        .header("cookie", format!("strata_sid={token}"));
    let (_, body, _) = edge.send(old_name).await;
    assert_eq!(body["session"]["authenticated"], false);
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
            if path.contains(":action") {
                ["expire-session", "reset"]
                    .map(|action| path.replace(":action", action))
                    .to_vec()
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
    let routes = post_routes();
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

    // Internal headers and parameters from a browser are not a way in.
    let spoof = edge
        .http
        .get(format!(
            "{}/api/view?path=/&site=tidewater&tenant=tidewater",
            edge.origin("127.0.0.1")
        ))
        .header("x-strata-site", "tidewater")
        .header("x-strata-service", TOKEN);
    let (_, spoofed, _) = edge.send(spoof).await;
    assert_eq!(spoofed["data"]["site"]["fields"]["name"], "Fieldnotes");

    // A fieldnotes session cookie replayed against the tidewater host is nobody there.
    let (token, _) = edge.login("127.0.0.1", "admin", "strata-dev-admin").await;
    let (_, there, _) = edge.get("localhost", "/api/session", Some(&token)).await;
    assert_eq!(there["session"]["authenticated"], false);
    let (_, desk, _) = edge
        .get("localhost", "/api/view?path=/desk", Some(&token))
        .await;
    assert_eq!(desk["data"]["posts"], json!([]));
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
                "{}/api/view?path=/&host=localhost&tenant=tidewater",
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

#[tokio::test]
async fn views_are_projected_to_plain_json_with_markdown_as_blocks() {
    let edge = start().await;
    let (status, view, _) = edge
        .get(
            "127.0.0.1",
            "/api/view?path=/posts/the-jig-is-the-work",
            None,
        )
        .await;
    assert_eq!(status, 200);
    let post = &view["data"]["post"];
    assert_eq!(
        post["fields"]["title"], "The Jig Is the Work",
        "plain values, not kernel-tagged ones"
    );
    assert_eq!(post["refs"]["author"]["fields"]["name"], "Tomasz Wren");
    assert_eq!(post["refs"]["category"]["fields"]["slug"], "craft");
    assert_eq!(post["blocks"]["body"][0]["type"], "paragraph");
    assert!(post["reading_minutes"].as_u64().unwrap() >= 2);
    assert!(view["depends_on"]
        .as_array()
        .unwrap()
        .contains(&json!("post")));

    let (_, search, _) = edge
        .get("127.0.0.1", "/api/view?path=/search&q=kiln", None)
        .await;
    let found: Vec<&str> = search["data"]["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["fields"]["slug"].as_str().unwrap())
        .collect();
    assert_eq!(found, ["what-the-kiln-decides"]);
    let (status, missing, _) = edge
        .get("127.0.0.1", "/api/view?path=/nope/nope", None)
        .await;
    assert_eq!((status, missing["code"].as_str()), (404, Some("not_found")));
    let (_, draft, _) = edge
        .get(
            "127.0.0.1",
            "/api/view?path=/posts/notes-toward-a-workbench",
            None,
        )
        .await;
    assert_eq!(
        draft["data"]["post"],
        Json::Null,
        "a draft is not there for the public"
    );
}

/// The desk views carry what the specification declares beyond the projected view: the topic
/// and author choices of the editor, the article and publication a save starts from, and the
/// names a theme swatch can take (read-new-article-view, read-edit-article-view,
/// read-settings-view).
#[tokio::test]
async fn desk_views_carry_their_choices_and_the_versions_a_save_starts_from() {
    let edge = start().await;
    let (token, _) = edge.login("127.0.0.1", "admin", "strata-dev-admin").await;

    let (status, new, _) = edge
        .get("127.0.0.1", "/api/view?path=/desk/new", Some(&token))
        .await;
    assert_eq!(status, 200);
    let topics = new["topics"].as_array().unwrap();
    assert_eq!(topics[0], json!({ "value": "", "label": "No topic" }));
    assert_eq!(
        topics.len(),
        new["data"]["categories"].as_array().unwrap().len() + 1
    );
    assert!(topics
        .iter()
        .skip(1)
        .all(|t| t["value"].is_string() && t["label"].is_string()));
    let authors = new["authors"].as_array().unwrap();
    assert_eq!(
        authors.len(),
        new["data"]["authors"].as_array().unwrap().len()
    );
    assert!(authors.iter().any(|a| a["label"] == "Tomasz Wren"));
    assert!(new.get("article_id").is_none());

    let (status, edit, _) = edge
        .get(
            "127.0.0.1",
            "/api/view?path=/desk/edit/the-jig-is-the-work",
            Some(&token),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(edit["article_id"], edit["data"]["post"]["id"]);
    assert_eq!(edit["article_version"], edit["data"]["post"]["version"]);
    assert!(edit["article_version"].is_u64());
    assert_eq!(edit["topics"][0]["label"], "No topic");

    let (status, settings, _) = edge
        .get("127.0.0.1", "/api/view?path=/desk/settings", Some(&token))
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        settings["accents"],
        json!(["moss", "clay", "ink", "ochre", "plum"])
    );
    assert_eq!(settings["papers"], json!(["linen", "stone", "chalk"]));
    assert_eq!(settings["publication_id"], settings["data"]["site"]["id"]);
    assert_eq!(
        settings["publication_version"],
        settings["data"]["site"]["version"]
    );
}

/// Verifies: CMS-004
///
/// A tenant author writes markup into an essay. What reaches a reader's browser, through the
/// gateway as data and inside the page, is structured blocks of a fixed set of kinds in which
/// the markup is text. The raw source is delivered only as a plain string field, which the
/// editor needs and nothing draws as HTML (`web/test/kernel.test.js` checks the drawing).
#[tokio::test]
async fn tenant_rich_text_reaches_the_browser_as_blocks_with_markup_as_text() {
    let edge = start().await;
    let (token, csrf) = edge.login("127.0.0.1", "tomasz", "strata-dev-tomasz").await;
    let cookie = format!("{SESSION_COOKIE}={token}");
    let body = "<script>alert(1)</script> and <img src=x onerror=alert(1)>\n\n[click](javascript:alert(1)) **bold** <b>not bold</b> [ok](https://example.com)\n\n<div>block</div>";
    let write = |change: Json| {
        edge.post("127.0.0.1", "/api/records", change)
            .header("cookie", &cookie)
            .header("x-csrf-token", &csrf)
    };
    let (status, created, _) = edge
        .send(write(json!({ "op": "create", "object": "post", "fields": { "title": "Hostile", "slug": "hostile", "body": body, "author": 3 } })))
        .await;
    assert_eq!(status, 200, "{created}");
    let (status, published, _) = edge
        .send(write(json!({ "op": "update", "object": "post", "id": created["id"], "fields": { "status": "published" } })))
        .await;
    assert_eq!(status, 200, "{published}");

    // As data: an anonymous reader's view of the essay.
    let (_, view, _) = edge
        .get("127.0.0.1", "/api/view?path=/posts/hostile", None)
        .await;
    let blocks = view["data"]["post"]["blocks"]["body"]
        .as_array()
        .expect("the body arrives as blocks");
    fn walk_inlines(
        inlines: &Json,
        kinds: &mut Vec<String>,
        texts: &mut Vec<String>,
        hrefs: &mut Vec<String>,
    ) {
        for inline in inlines.as_array().into_iter().flatten() {
            kinds.push(inline["type"].as_str().unwrap().to_string());
            if let Some(text) = inline["text"].as_str() {
                texts.push(text.to_string());
            }
            if let Some(href) = inline["href"].as_str() {
                hrefs.push(href.to_string());
            }
            walk_inlines(&inline["children"], kinds, texts, hrefs);
        }
    }
    let (mut block_kinds, mut inline_kinds, mut texts, mut hrefs) =
        (vec![], vec![], vec![], vec![]);
    for block in blocks {
        block_kinds.push(block["type"].as_str().unwrap());
        walk_inlines(&block["inlines"], &mut inline_kinds, &mut texts, &mut hrefs);
    }
    let allowed_blocks = [
        "heading",
        "paragraph",
        "quote",
        "list",
        "code",
        "figure",
        "rule",
    ];
    assert!(
        block_kinds.iter().all(|kind| allowed_blocks.contains(kind)),
        "{block_kinds:?}"
    );
    let allowed_inlines = ["text", "em", "strong", "code", "link"];
    assert!(
        inline_kinds
            .iter()
            .all(|kind| allowed_inlines.contains(&kind.as_str())),
        "{inline_kinds:?}"
    );
    let text = texts.join("");
    for markup in [
        "<script>alert(1)</script>",
        "<img src=x onerror=alert(1)>",
        "<b>not bold</b>",
        "<div>block</div>",
    ] {
        assert!(
            text.contains(markup),
            "{markup} is not kept as text: {text}"
        );
    }
    assert_eq!(
        hrefs,
        ["https://example.com"],
        "the unsafe link lost its target"
    );
    assert!(
        !view["data"]["post"]["blocks"]
            .to_string()
            .contains("javascript:"),
        "no unsafe address reaches the browser as a block"
    );
    assert_eq!(
        view["data"]["post"]["fields"]["body"], body,
        "the source is only ever a string"
    );

    // Inside the page: the markup is neither in the document nor able to leave the data block.
    let (status, _, page) = edge.get("127.0.0.1", "/posts/hostile", None).await;
    assert_eq!(status, 200);
    let (before, rest) = page
        .text
        .split_once("<script type=\"application/json\" id=\"strata-bootstrap\">")
        .expect("the data block");
    let (data, after) = rest.split_once("</script>").expect("the data block ends");
    assert!(
        !data.contains('<'),
        "markup can start inside the data block"
    );
    for outside in [before, after] {
        assert!(
            !outside.contains("alert(1)") && !outside.contains("<img src=x"),
            "tenant markup is in the document: {outside}"
        );
    }
    let bootstrap: Json = serde_json::from_str(data).unwrap();
    assert_eq!(
        bootstrap["view"]["data"]["post"]["blocks"], view["data"]["post"]["blocks"],
        "the page carries the same blocks"
    );
}

/// Verifies: GWY-005
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
    assert_eq!(edge.get("127.0.0.1", "/no/such/page", None).await.0, 404);
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

/// The development fixtures need a signed-in owner of the site. The request token the gateway
/// hands to every visitor, sent from our own origin, opens nothing: the reset is refused and the
/// site is as it was.
#[tokio::test]
async fn the_fixtures_refuse_a_visitor_and_anyone_but_the_owner() {
    let edge = start().await;
    let admin = edge.login("127.0.0.1", "admin", "strata-dev-admin").await;
    // A change, so that a reset would show.
    let change = json!({ "op": "create", "object": "category", "fields": { "name": "Before reset", "slug": "before-reset" } });
    let write = edge
        .post("127.0.0.1", "/api/records", change)
        .header("cookie", format!("{SESSION_COOKIE}={}", admin.0))
        .header("x-csrf-token", &admin.1);
    assert_eq!(edge.send(write).await.0, 200);
    let topics = || edge.get("127.0.0.1", "/api/view?path=/topics", None);
    let before = topics().await.1;
    assert!(before.to_string().contains("Before reset"));

    for action in ["reset", "expire-session"] {
        let path = format!("/api/dev/{action}");
        // A visitor, with the token the gateway issued to it, from our own origin.
        let anonymous = edge
            .post("127.0.0.1", &path, json!({}))
            .header("x-csrf-token", edge.request_token("127.0.0.1").await);
        let (status, body, _) = edge.send(anonymous).await;
        assert_eq!(status, 401, "a visitor's {action}: {body}");
        // An editor, with the token of her own session.
        let (token, csrf) = edge.login("127.0.0.1", "ines", "strata-dev-ines").await;
        let editor = edge
            .post("127.0.0.1", &path, json!({}))
            .header("cookie", format!("{SESSION_COOKIE}={token}"))
            .header("x-csrf-token", csrf);
        let (status, body, _) = edge.send(editor).await;
        assert_eq!(status, 403, "an editor's {action}: {body}");
    }
    assert_eq!(topics().await.1, before, "nothing was reset");

    // The owner, signed in, with the session's token: the site is installed again.
    let owner = edge
        .post("127.0.0.1", "/api/dev/reset", json!({}))
        .header("cookie", format!("{SESSION_COOKIE}={}", admin.0))
        .header("x-csrf-token", &admin.1);
    let (status, body, _) = edge.send(owner).await;
    assert_eq!(status, 200, "{body}");
    assert!(!topics().await.1.to_string().contains("Before reset"));
}

/// An origin is accepted only on the site its host serves: a page of one tenant cannot act on
/// another tenant served by the same gateway, even though both origins are configured.
#[tokio::test]
async fn an_origin_counts_only_for_its_own_site() {
    let edge = start().await;
    let fieldnotes_token = edge.request_token("127.0.0.1").await;
    // Fieldnotes is served at 127.0.0.1; `localhost` is tidewater's origin.
    let sign_in = |origin: String| {
        edge.http
            .post(format!("{}/api/login", edge.origin("127.0.0.1")))
            .header("origin", origin)
            .header("x-csrf-token", &fieldnotes_token)
            .json(&json!({ "login": "ines", "password": "strata-dev-ines" }))
    };
    let (status, _, reply) = edge.send(sign_in(edge.origin("localhost"))).await;
    assert_eq!(status, 403, "tidewater's origin signed in on fieldnotes");
    assert!(reply.cookie.is_none());
    assert!(
        connect(
            &edge,
            Some(edge.origin("localhost")),
            None,
            Some(fieldnotes_token.as_str())
        )
        .await
        .is_err(),
        "tidewater's origin opened a fieldnotes socket"
    );
    let preview = edge
        .http
        .post(format!("{}/api/preview", edge.origin("127.0.0.1")))
        .header("origin", edge.origin("localhost"))
        .json(&json!({ "markdown": "a" }));
    assert_eq!(edge.send(preview).await.0, 403);
    // The control: the site's own origin.
    let (status, _, _) = edge.send(sign_in(edge.origin("127.0.0.1"))).await;
    assert_eq!(status, 200);
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

/// Verifies: GWY-253
/// Verifies: GWY-252
/// A page response requires Trusted Types for script sinks and names exactly one policy, the
/// kernel worker's address; it creates no default policy and keeps the base, object and form
/// directives on the page's own origin or on none.
#[tokio::test]
async fn a_page_requires_trusted_types_and_names_one_policy() {
    let edge = start().await;
    for path in ["/", "/login", "/no/such/page"] {
        let (_, _, reply) = edge.get("127.0.0.1", path, None).await;
        let csp = reply.headers["content-security-policy"].to_str().unwrap();
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

/// Verifies: IDN-012
#[tokio::test]
async fn a_person_lists_and_revokes_their_sessions_behind_the_origin_and_token_checks() {
    let edge = start().await;
    let (here, here_csrf) = edge.login("127.0.0.1", "ines", "strata-dev-ines").await;
    let (there, _) = edge.login("127.0.0.1", "ines", "strata-dev-ines").await;
    let ours = edge.origin("127.0.0.1");

    let (status, listed, _) = edge.get("127.0.0.1", "/api/sessions", Some(&here)).await;
    assert_eq!(status, 200, "{listed}");
    let list = listed["sessions"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list.iter().filter(|s| s["current"] == true).count(), 1);
    let text = listed.to_string();
    assert!(!text.contains(&here) && !text.contains(&there) && !text.contains("csrf"));
    assert_eq!(
        edge.get("127.0.0.1", "/api/sessions", None).await.0,
        401,
        "no session, nothing to list"
    );

    let (_, there_view, _) = edge.get("127.0.0.1", "/api/session", Some(&there)).await;
    let there_id = there_view["session"]["session_id"].as_str().unwrap();
    let revoke = |origin: Option<&str>, csrf: Option<&str>| {
        let mut request = edge
            .http
            .post(format!("{ours}/api/sessions/revoke"))
            .header("cookie", format!("{SESSION_COOKIE}={here}"))
            .json(&json!({ "id": there_id }));
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        if let Some(csrf) = csrf {
            request = request.header("x-csrf-token", csrf);
        }
        request
    };
    assert_eq!(
        edge.send(revoke(Some(&ours), None)).await.0,
        403,
        "no token"
    );
    assert_eq!(
        edge.send(revoke(Some(&ours), Some("0000"))).await.0,
        403,
        "wrong token"
    );
    assert_eq!(
        edge.send(revoke(Some("http://evil.example"), Some(&here_csrf)))
            .await
            .0,
        403,
        "foreign origin"
    );
    assert_eq!(
        edge.send(revoke(None, Some(&here_csrf))).await.0,
        403,
        "no origin"
    );
    let (_, still, _) = edge.get("127.0.0.1", "/api/session", Some(&there)).await;
    assert_eq!(
        still["session"]["authenticated"], true,
        "the refusals ended nothing"
    );

    assert_eq!(
        edge.send(revoke(Some(&ours), Some(&here_csrf))).await.0,
        200
    );
    let (_, gone, reply) = edge.get("127.0.0.1", "/api/session", Some(&there)).await;
    assert_eq!(gone["state"], "revoked");
    assert!(
        reply.cookie.unwrap().contains("Max-Age=0"),
        "its cookie is cleared"
    );

    // Ending the session that makes the request clears its own cookie in the same answer.
    let (_, here_view, _) = edge.get("127.0.0.1", "/api/session", Some(&here)).await;
    let own_id = here_view["session"]["session_id"].as_str().unwrap();
    let (status, _, reply) = edge
        .send(
            edge.post("127.0.0.1", "/api/sessions/revoke", json!({ "id": own_id }))
                .header("cookie", format!("{SESSION_COOKIE}={here}"))
                .header("x-csrf-token", &here_csrf),
        )
        .await;
    assert_eq!(status, 200);
    assert!(reply.cookie.expect("a cookie header").contains("Max-Age=0"));

    // The others, all at once.
    let (first, first_csrf) = edge.login("127.0.0.1", "ines", "strata-dev-ines").await;
    let (second, _) = edge.login("127.0.0.1", "ines", "strata-dev-ines").await;
    let (third, _) = edge.login("127.0.0.1", "ines", "strata-dev-ines").await;
    let (status, reply, _) = edge
        .send(
            edge.post("127.0.0.1", "/api/sessions/revoke-others", json!({}))
                .header("cookie", format!("{SESSION_COOKIE}={first}"))
                .header("x-csrf-token", &first_csrf),
        )
        .await;
    assert_eq!((status, &reply["revoked"]), (200, &json!(2)));
    for gone in [&second, &third] {
        let (_, view, _) = edge.get("127.0.0.1", "/api/session", Some(gone)).await;
        assert_eq!(view["session"]["authenticated"], false);
    }
    let (_, kept, _) = edge.get("127.0.0.1", "/api/session", Some(&first)).await;
    assert_eq!(kept["session"]["authenticated"], true);
}

/// Verifies: IDN-250
///
/// The address a sign-in is limited by is the one the connection came from. A browser that
/// writes another into a header does not get a count of its own for each.
#[tokio::test]
async fn the_source_of_a_sign_in_is_the_connection_and_not_what_the_browser_says() {
    let edge = start_with(
        |_| {},
        |backend| {
            backend.throttle = strata_backend::app::ThrottleLimits {
                source_max: 3,
                ..Default::default()
            }
        },
    )
    .await;
    for (i, claimed) in ["203.0.113.1", "203.0.113.2", "203.0.113.3"]
        .iter()
        .enumerate()
    {
        let (status, _, _) = attempt_from(&edge, Some(claimed), &format!("nobody{i}"), "x").await;
        assert_eq!(status, 401);
    }
    let (status, body, reply) =
        attempt_from(&edge, Some("203.0.113.4"), "tomasz", "strata-dev-tomasz").await;
    assert_eq!(status, 429, "{body}");
    assert!(reply.cookie.is_none(), "a refused sign-in sets no cookie");
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

/// Verifies: IDN-001
#[tokio::test]
async fn sign_in_goes_through_the_method_named_and_passes_on_only_its_own_fields() {
    let edge = start().await;
    let token = edge.request_token("127.0.0.1").await;
    let (status, body, reply) = edge
        .send(
            edge.post(
                "127.0.0.1",
                "/api/sign-in/password",
                json!({ "login": "ines", "password": "strata-dev-ines", "tenant": "tidewater", "admin": true }),
            )
            .header("x-csrf-token", &token),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body["session"]["tenant"], "fieldnotes",
        "the site is the host's"
    );
    assert!(reply.cookie.unwrap().starts_with(SESSION_COOKIE));
    for (path, payload, expected) in [
        (
            "/api/sign-in/telepathy",
            json!({ "login": "ines", "password": "x" }),
            404,
        ),
        ("/api/sign-in/password", json!({ "login": "ines" }), 401),
        ("/api/sign-in/password", json!({}), 401),
    ] {
        let (status, _, reply) = edge
            .send(
                edge.post("127.0.0.1", path, payload)
                    .header("x-csrf-token", &token),
            )
            .await;
        assert_eq!(status, expected, "{path}");
        assert!(reply.cookie.is_none());
    }
}

/// Verifies: FED-005, FED-250
///
/// The assertion consumer address takes the provider's post, from another site and without any
/// cookie of the person's but the return address, and signs no one in: it moves the response and
/// the relay state into the fragment of the sign-in page, with the return address as a query
/// that is a path on the site, and clears that cookie. A post that is not a response, or whose
/// relay state is not one this service makes, sends the page to say so. A site that is not served
/// is not answered, and the metadata of a provider nobody configured is not found.
#[tokio::test]
async fn the_assertion_consumer_address_moves_the_response_to_the_page_and_signs_no_one_in() {
    let edge = start().await;
    let relay = "ab".repeat(32);
    let post = |host: &str, body: String, cookie: Option<&str>| {
        let mut request = edge
            .http
            .post(format!("{}/federation/saml/acs", edge.origin(host)))
            .header("origin", "https://idp.acme.example")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body);
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        request
    };
    let sent = edge
        .send(post(
            "127.0.0.1",
            format!("SAMLResponse=PHNhbWw%2BKw%3D%3D&RelayState={relay}&extra=ignored"),
            Some("__Host-strata-return=%2Fdesk"),
        ))
        .await;
    assert_eq!(sent.0, 303);
    assert_eq!(
        sent.2.headers["location"].to_str().unwrap(),
        format!("/login?next=%2Fdesk#m=saml&r=PHNhbWw%2BKw%3D%3D&t={relay}")
    );
    let cookies: Vec<_> = sent
        .2
        .headers
        .get_all("set-cookie")
        .iter()
        .map(|value| value.to_str().unwrap().to_string())
        .collect();
    assert!(
        cookies
            .iter()
            .all(|cookie| !cookie.starts_with(SESSION_COOKIE)),
        "the provider's post signs no one in: {cookies:?}"
    );
    assert!(
        cookies
            .iter()
            .any(|cookie| cookie.starts_with("__Host-strata-return=;")
                && cookie.contains("Max-Age=0")),
        "{cookies:?}"
    );

    for (why, body, cookie) in [
        (
            "a relay state that is not one this service makes",
            "SAMLResponse=PHNhbWw%2BKw%3D%3D&RelayState=short".to_string(),
            None,
        ),
        ("no response", format!("RelayState={relay}"), None),
        (
            "a response that is not base64",
            format!("SAMLResponse=%3Cscript%3E&RelayState={relay}"),
            None,
        ),
        (
            "a return address that is not a path on the site",
            format!("SAMLResponse=PHNhbWw%2BKw%3D%3D&RelayState={relay}"),
            Some("__Host-strata-return=%2F%2Fevil.example"),
        ),
    ] {
        let sent = edge.send(post("127.0.0.1", body, cookie)).await;
        assert_eq!(sent.0, 303, "{why}");
        let location = sent.2.headers["location"].to_str().unwrap().to_string();
        if why.starts_with("a return address") {
            assert!(location.starts_with("/login#m=saml&"), "{why}: {location}");
        } else {
            assert_eq!(location, "/login#e=1", "{why}");
        }
    }

    let (status, _, _) = edge
        .send(
            edge.post("127.0.0.1", "/federation/saml/acs", json!({}))
                .header("host", "not-a-site.example"),
        )
        .await;
    assert_eq!(status, 404, "a host that is not served");

    let (status, _, _) = edge
        .get("127.0.0.1", "/federation/saml/metadata/nobody", None)
        .await;
    assert_eq!(status, 404, "no such provider");
}

/// A socket opened as an anonymous visitor, with extra request headers.
async fn anonymous_socket(edge: &Edge, extra: &[(&'static str, &str)]) -> Upgrade {
    let token = edge.request_token("127.0.0.1").await;
    connect_with(
        edge,
        Some(edge.origin("127.0.0.1")),
        None,
        Some(token.as_str()),
        extra,
    )
    .await
}

fn throttled(upgrade: &Upgrade) -> bool {
    matches!(
        upgrade,
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) if response.status().as_u16() == 429
    )
}

/// Sockets are capped per source address at the BFF. A browser cannot change its source by
/// sending `X-Forwarded-For` or `x-strata-source` itself: no proxy is trusted, so the peer's
/// address is the source whatever the headers say.
#[tokio::test]
async fn forwarded_headers_from_an_untrusted_peer_do_not_change_the_source() {
    let edge = start_with(
        |bff| {
            bff.limits.sockets_per_source = 2;
            bff.limits.sockets_per_session = 8;
        },
        |_| {},
    )
    .await;
    let mut held = Vec::new();
    for spoofed in ["203.0.113.1", "203.0.113.2"] {
        held.push(
            anonymous_socket(
                &edge,
                &[("x-forwarded-for", spoofed), ("x-strata-source", spoofed)],
            )
            .await
            .expect("within the cap"),
        );
    }
    let third = anonymous_socket(
        &edge,
        &[
            ("x-forwarded-for", "203.0.113.3"),
            ("x-strata-source", "203.0.113.3"),
        ],
    )
    .await;
    assert!(
        throttled(&third),
        "a third socket from one peer, however it signs itself: {third:?}"
    );
}

/// Behind a trusted proxy the visitor is the rightmost hop of `X-Forwarded-For` that is not a
/// trusted proxy: two visitors behind it have their own caps, and a client cannot pass as
/// another by writing the left of the chain.
#[tokio::test]
async fn a_trusted_proxy_passes_the_visitor_through_x_forwarded_for() {
    let edge = start_with(
        |bff| {
            bff.trusted_proxies = vec!["127.0.0.1".parse().unwrap()];
            bff.limits.sockets_per_source = 1;
        },
        |_| {},
    )
    .await;
    let first = anonymous_socket(&edge, &[("x-forwarded-for", "203.0.113.1")]).await;
    assert!(first.is_ok(), "{first:?}");
    let other = anonymous_socket(&edge, &[("x-forwarded-for", "203.0.113.2")]).await;
    assert!(
        other.is_ok(),
        "another visitor behind the same proxy: {other:?}"
    );
    let again = anonymous_socket(&edge, &[("x-forwarded-for", "203.0.113.1")]).await;
    assert!(
        throttled(&again),
        "the first visitor's second socket: {again:?}"
    );
    // The left of the chain is whatever the client wrote; the proxy appended the right.
    let forged = anonymous_socket(
        &edge,
        &[("x-forwarded-for", "198.51.100.77, 203.0.113.1, 127.0.0.1")],
    )
    .await;
    assert!(
        throttled(&forged),
        "a chain through the proxy resolves to the rightmost untrusted hop: {forged:?}"
    );
    let fresh = anonymous_socket(
        &edge,
        &[("x-forwarded-for", "198.51.100.77, 203.0.113.9, 127.0.0.1")],
    )
    .await;
    assert!(fresh.is_ok(), "{fresh:?}");
    // Without the header the proxy itself is the source, and it has no socket of its own yet.
    let bare = anonymous_socket(&edge, &[]).await;
    assert!(bare.is_ok(), "{bare:?}");
    drop((first, other, fresh, bare));
}

/// One session may hold only so many sockets through the BFF, and the cap is the BFF's own: the
/// backend's limits are out of the way here.
#[tokio::test]
async fn sockets_are_capped_per_session_at_the_bff() {
    let edge = start_with(
        |bff| {
            bff.limits.sockets_per_session = 2;
            bff.limits.sockets_per_source = 50;
        },
        |backend| {
            backend.limits.sockets_per_session = 50;
            backend.limits.sockets_per_source = 50;
        },
    )
    .await;
    let (token, csrf) = edge.login("127.0.0.1", "ines", "strata-dev-ines").await;
    let mut held = Vec::new();
    for _ in 0..2 {
        held.push(
            connect(
                &edge,
                Some(edge.origin("127.0.0.1")),
                Some(token.as_str()),
                Some(csrf.as_str()),
            )
            .await
            .expect("within the cap"),
        );
    }
    let third = connect(
        &edge,
        Some(edge.origin("127.0.0.1")),
        Some(token.as_str()),
        Some(csrf.as_str()),
    )
    .await;
    assert!(throttled(&third), "{third:?}");
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

/// Verifies: SCH-1575
///
/// A subscription that names event kinds is told only about events of those kinds, a selector
/// that names segments or units is refused rather than matching nothing, and the kinds of the
/// selector never widen what the person may read.
#[tokio::test]
async fn a_socket_selects_events_by_kind_and_refuses_what_nothing_emits() {
    let edge = start().await;
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
    assert_eq!(next(&mut socket).await["type"], "welcome");

    // Refused: the events of segments and units are not emitted, and a kind that is not one.
    for (req, member) in [
        (
            "seg",
            json!({ "segments": ["0b1a6a7e-0000-4000-8000-000000000001"] }),
        ),
        ("unit", json!({ "units": ["eur"] })),
        ("kind", json!({ "kinds": ["record-create"] })),
    ] {
        let mut frame = json!({ "type": "subscribe", "req": req, "sub": req });
        frame
            .as_object_mut()
            .unwrap()
            .extend(member.as_object().unwrap().clone());
        socket.send(Message::Text(frame.to_string())).await.unwrap();
        let reply = next(&mut socket).await;
        assert_eq!(
            (
                reply["type"].as_str(),
                reply["code"].as_str(),
                reply["req"].as_str()
            ),
            (Some("error"), Some("invalid"), Some(req)),
            "{reply}"
        );
    }

    // Two subscriptions: one for deletions, one for creations.
    for (name, kind) in [("deleted", "record-deleted"), ("created", "record-created")] {
        socket
            .send(Message::Text(
                json!({ "type": "subscribe", "req": name, "sub": name, "kinds": [kind] })
                    .to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(
            next(&mut socket).await,
            json!({ "type": "ok", "req": name })
        );
    }
    let as_ines = |body: Json| {
        edge.post("127.0.0.1", "/api/records", body)
            .header("cookie", format!("{SESSION_COOKIE}={token}"))
            .header("x-csrf-token", &csrf)
    };
    let (status, created, _) = edge
        .send(as_ines(json!({ "op": "create", "object": "category", "fields": { "name": "Kinds", "slug": "kinds" } })))
        .await;
    assert_eq!(status, 200, "{created}");
    let told = next(&mut socket).await;
    assert_eq!(
        (told["sub"].as_str(), told["notice"]["kind"].as_str()),
        (Some("created"), Some("upsert")),
        "the creation reaches the creation subscription only: {told}"
    );
    let (status, deleted, _) = edge
        .send(as_ines(
            json!({ "op": "delete", "object": "category", "id": created["id"] }),
        ))
        .await;
    assert_eq!(status, 200, "{deleted}");
    let told = next(&mut socket).await;
    assert_eq!(
        (told["sub"].as_str(), told["notice"]["kind"].as_str()),
        (Some("deleted"), Some("remove")),
        "the deletion reaches the deletion subscription only: {told}"
    );
}

/// Verifies: ADM-1083
///
/// The owner withdraws an invitation through `POST /api/members/revoke-invitation`, behind the
/// origin and request-token checks, and the person it named cannot redeem it afterwards.
#[tokio::test]
async fn an_invitation_is_revoked_through_the_gateway_and_cannot_be_redeemed() {
    let edge = start().await;
    let (owner, owner_csrf) = edge.login("127.0.0.1", "admin", "strata-dev-admin").await;
    let ours = edge.origin("127.0.0.1");
    let as_owner = |op: &str, body: Json| {
        edge.post("127.0.0.1", &format!("/api/members/{op}"), body)
            .header("cookie", format!("{SESSION_COOKIE}={owner}"))
            .header("x-csrf-token", &owner_csrf)
    };
    let (status, invited, _) = edge
        .send(as_owner(
            "invite",
            json!({ "invitation": "inv-gateway", "address": "tomasz@fieldnotes.example", "roles": ["editor"] }),
        ))
        .await;
    assert_eq!(status, 200, "{invited}");

    // The write guard comes first: no token, a wrong token and a foreign origin change nothing.
    let unguarded = |csrf: Option<&str>, origin: &str| {
        let mut request = edge
            .http
            .post(format!("{ours}/api/members/revoke-invitation"))
            .header("cookie", format!("{SESSION_COOKIE}={owner}"))
            .header("origin", origin)
            .json(&json!({ "invitation": "inv-gateway" }));
        if let Some(csrf) = csrf {
            request = request.header("x-csrf-token", csrf);
        }
        request
    };
    assert_eq!(edge.send(unguarded(None, &ours)).await.0, 403, "no token");
    assert_eq!(
        edge.send(unguarded(Some("0000"), &ours)).await.0,
        403,
        "wrong token"
    );
    assert_eq!(
        edge.send(unguarded(Some(&owner_csrf), "http://evil.example"))
            .await
            .0,
        403,
        "foreign origin"
    );

    // A body that does not name the invitation is refused, and so is an unknown operation.
    let (status, _, _) = edge.send(as_owner("revoke-invitation", json!({}))).await;
    assert_eq!(status, 422);
    let (status, _, _) = edge
        .send(as_owner(
            "revoke-everything",
            json!({ "invitation": "inv-gateway" }),
        ))
        .await;
    assert_eq!(status, 404);

    let (status, revoked, _) = edge
        .send(as_owner(
            "revoke-invitation",
            json!({ "invitation": "inv-gateway" }),
        ))
        .await;
    assert_eq!(status, 200, "{revoked}");
    assert!(revoked["revision"].as_u64().is_some(), "{revoked}");
    let (status, again, _) = edge
        .send(as_owner(
            "revoke-invitation",
            json!({ "invitation": "inv-gateway" }),
        ))
        .await;
    assert_eq!(status, 422, "a second revocation: {again}");

    // The person the invitation named signs in and cannot redeem it.
    let (guest, guest_csrf) = edge.login("127.0.0.1", "tomasz", "strata-dev-tomasz").await;
    let (status, refused, _) = edge
        .send(
            edge.post(
                "127.0.0.1",
                "/api/members/redeem",
                json!({ "invitation": "inv-gateway" }),
            )
            .header("cookie", format!("{SESSION_COOKIE}={guest}"))
            .header("x-csrf-token", &guest_csrf),
        )
        .await;
    assert_eq!(status, 422, "{refused}");
}

/// The ids of the rows of one section of a projected view.
fn row_ids(view: &Json, section: &str) -> Vec<u64> {
    view["data"][section]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_u64().unwrap())
        .collect()
}

/// Verifies: CMS-164
///
/// A list is read a page at a time through the browser's address: the cursor an answer holds
/// fetches the next page, and the pages together are the list.
#[tokio::test]
async fn a_second_page_of_a_view_continues_the_first_with_no_gap_or_repeat() {
    let edge = start().await;
    let (status, whole, _) = edge
        .get("127.0.0.1", "/api/view?path=/&limit=100", None)
        .await;
    assert_eq!(status, 200, "{whole}");
    let every = row_ids(&whole, "recent");
    assert!(every.len() > 4);
    assert_eq!(whole["next"], json!({}), "a list that fits has no cursor");

    let mut seen = Vec::new();
    let mut path = "/api/view?path=/&limit=2".to_string();
    let mut pages = 0;
    loop {
        let (status, page, _) = edge.get("127.0.0.1", &path, None).await;
        assert_eq!(status, 200, "{page}");
        let rows = row_ids(&page, "recent");
        assert!(rows.len() <= 2);
        seen.extend(rows);
        pages += 1;
        match page["next"]["recent"].as_str() {
            Some(cursor) => {
                path = format!(
                    "/api/view?path=/&limit=2&cursor={}",
                    cursor.replace(':', "%3A")
                );
            }
            None => break,
        }
        assert!(pages < 20, "the pages end");
    }
    assert!(pages > 1);
    assert_eq!(seen, every, "no row is missing and none repeats");

    for query in [
        "limit=0",
        "limit=101",
        "limit=many",
        "cursor=recent%3Azz",
        "cursor=site%3Ao1",
    ] {
        let (status, body, _) = edge
            .get("127.0.0.1", &format!("/api/view?path=/&{query}"), None)
            .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (422, Some("invalid")),
            "{query}: {body}"
        );
    }
}

/// Verifies: CMS-163
///
/// A selection of fields narrows what a reader would have received and never adds to it: a field
/// the kernel withheld from the reader is not there for being named, and a field that was not
/// named is not there either.
#[tokio::test]
async fn a_view_with_a_field_selection_holds_no_unselected_and_no_redacted_field() {
    let edge = start().await;
    let keys = |view: &Json, section: &str| -> Vec<Vec<String>> {
        view["data"][section]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["fields"].as_object().unwrap().keys().cloned().collect())
            .collect()
    };
    let (status, public, _) = edge
        .get(
            "127.0.0.1",
            "/api/view?path=/authors&fields=name,principal",
            None,
        )
        .await;
    assert_eq!(status, 200, "{public}");
    let rows = keys(&public, "authors");
    assert!(rows.len() > 1);
    assert!(
        rows.iter().all(|fields| fields == &["name"]),
        "the login is the author's own, and the bio and slug were not asked for: {rows:?}"
    );
    assert!(public["data"]["authors"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row.get("blocks").is_none()));

    let (token, _) = edge.login("127.0.0.1", "ines", "strata-dev-ines").await;
    let (_, editor, _) = edge
        .get(
            "127.0.0.1",
            "/api/view?path=/authors&fields=name,principal",
            Some(&token),
        )
        .await;
    let rows = keys(&editor, "authors");
    assert!(
        rows.iter()
            .any(|fields| fields.contains(&"principal".to_string())),
        "an editor may read it, so the same selection holds it"
    );
    assert!(rows
        .iter()
        .all(|fields| fields.iter().all(|f| f == "name" || f == "principal")));

    for query in ["fields=nope", "fields=", "fields=name,,slug"] {
        let (status, body, _) = edge
            .get(
                "127.0.0.1",
                &format!("/api/view?path=/authors&{query}"),
                None,
            )
            .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (422, Some("invalid")),
            "{query}: {body}"
        );
    }
}

/// Verifies: OPS-318
///
/// A create the browser sends twice with the same key makes one record, through the gateway and
/// the backend; the same key with another body is refused and a key that cannot be used is
/// refused before anything is done.
#[tokio::test]
async fn a_retried_create_with_the_same_key_makes_one_record_through_the_gateway() {
    let edge = start().await;
    let (token, csrf) = edge.login("127.0.0.1", "ines", "strata-dev-ines").await;
    let cookie = format!("{SESSION_COOKIE}={token}");
    let create = |slug: &str, key: &str| {
        edge.post(
            "127.0.0.1",
            "/api/records",
            json!({ "op": "create", "object": "category", "fields": { "name": "Keyed", "slug": slug } }),
        )
        .header("cookie", &cookie)
        .header("x-csrf-token", &csrf)
        .header("idempotency-key", key)
    };
    let (first_status, first, _) = edge.send(create("keyed", "k-0001-aaaa")).await;
    assert_eq!(first_status, 200, "{first}");
    let (second_status, second, _) = edge.send(create("keyed", "k-0001-aaaa")).await;
    assert_eq!(second_status, 200, "{second}");
    assert_eq!(
        second["id"], first["id"],
        "the retry answers the first result"
    );

    let (_, topics, _) = edge.get("127.0.0.1", "/api/view?path=/topics", None).await;
    let made = topics["data"]["categories"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["fields"]["slug"] == "keyed")
        .count();
    assert_eq!(made, 1, "one record, not two");

    let (status, other, _) = edge.send(create("keyed-again", "k-0001-aaaa")).await;
    assert_eq!(
        (status, other["code"].as_str()),
        (422, Some("invalid")),
        "{other}"
    );
    let (status, bad, _) = edge.send(create("keyed-long", &"k".repeat(129))).await;
    assert_eq!(
        (status, bad["code"].as_str()),
        (422, Some("invalid")),
        "{bad}"
    );
    let (_, after, _) = edge.get("127.0.0.1", "/api/view?path=/topics", None).await;
    assert_eq!(
        after["data"]["categories"].as_array().unwrap().len(),
        topics["data"]["categories"].as_array().unwrap().len(),
        "the refused requests made nothing"
    );
}

/// Verifies: INT-2346
///
/// An owner installs, lists and removes a product through `POST /api/products/<action>` and
/// `GET /api/products`, behind the origin and request-token checks, and the idempotency key of the
/// browser reaches the backend: a repeat of the install changes nothing.
#[tokio::test]
async fn products_are_installed_listed_and_removed_through_the_gateway() {
    let edge = start().await;
    let (owner, csrf) = edge.login("127.0.0.1", "admin", "strata-dev-admin").await;
    let as_owner = |action: &str, body: Json| {
        edge.post("127.0.0.1", &format!("/api/products/{action}"), body)
            .header("cookie", format!("{SESSION_COOKIE}={owner}"))
            .header("x-csrf-token", &csrf)
    };
    let bundle = json!({
        "name": "Gateway check",
        "version": "1.0.0",
        "product": "gateway-check",
        "content": { "objects": { "gateway_check__thing": {
            "id": "gateway_check__thing", "label": "Thing",
            "fields": { "name": { "id": "name", "label": "Name", "ty": "Str", "required": true, "unique": false } },
            "constraints": {}
        } } }
    });

    // Nobody signed in lists nothing; the write guard comes before the action is looked at.
    assert_eq!(edge.get("127.0.0.1", "/api/products", None).await.0, 401);
    let (status, _, _) = edge
        .send(
            edge.post(
                "127.0.0.1",
                "/api/products/install",
                json!({ "bundle": bundle }),
            )
            .header("cookie", format!("{SESSION_COOKIE}={owner}")),
        )
        .await;
    assert_eq!(status, 403, "no request token");
    let (status, _, _) = edge.send(as_owner("sell", json!({}))).await;
    assert_eq!(status, 404, "an unknown action");
    let (status, _, _) = edge.send(as_owner("install", json!({}))).await;
    assert_eq!(status, 422, "a request that carries no bundle");

    let (_, listed, _) = edge.get("127.0.0.1", "/api/products", Some(&owner)).await;
    assert_eq!(listed["products"], json!([]));
    let (status, dry, _) = edge
        .send(as_owner(
            "install",
            json!({ "bundle": bundle, "dry_run": true }),
        ))
        .await;
    assert_eq!(status, 200, "{dry}");
    assert_eq!(
        dry["projection"]["created"][0]["id"],
        "gateway_check__thing"
    );

    let keyed = |body: Json| as_owner("install", body).header("idempotency-key", "gateway-1");
    let (status, installed, _) = edge.send(keyed(json!({ "bundle": bundle }))).await;
    assert_eq!(status, 200, "{installed}");
    assert_eq!(installed["unchanged"], false);
    let (status, again, _) = edge.send(keyed(json!({ "bundle": bundle }))).await;
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["unchanged"], true);
    assert_eq!(again["schema_version"], installed["schema_version"]);

    let (_, listed, _) = edge.get("127.0.0.1", "/api/products", Some(&owner)).await;
    assert_eq!(listed["products"][0]["product"], "gateway-check");
    let (status, removed, _) = edge
        .send(as_owner(
            "remove",
            json!({ "product": "gateway-check", "delete_records": false }),
        ))
        .await;
    assert_eq!(status, 200, "{removed}");
    let (_, listed, _) = edge.get("127.0.0.1", "/api/products", Some(&owner)).await;
    assert_eq!(listed["products"], json!([]));
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
