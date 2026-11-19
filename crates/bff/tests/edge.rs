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
}

async fn start() -> Edge {
    start_with(Default::default()).await
}

async fn start_with(throttle: strata_backend::app::ThrottleLimits) -> Edge {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let backend_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_addr = backend_listener.local_addr().unwrap();
    let backend_cfg = strata_backend::app::Config {
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
        throttle,
    };
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
    let cfg = Config {
        addr: listener.local_addr().unwrap(),
        backend: format!("http://{backend_addr}"),
        service_token: TOKEN.into(),
        sites: Config::parse_sites("127.0.0.1=fieldnotes,localhost=tidewater").unwrap(),
        origins: vec![
            format!("http://127.0.0.1:{port}"),
            format!("http://localhost:{port}"),
        ],
        dist,
        dev: true,
    };
    tokio::spawn(strata_bff::run(cfg, listener, std::future::pending()));
    let edge = Edge {
        port,
        http: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
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
    include_str!("../src/routes.rs")
        .lines()
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
    // token and with a wrong one, and must be refused before anything happens. The one POST that
    // is exempt changes nothing: the Markdown preview is a pure function of its text.
    let routes = post_routes();
    assert!(
        routes.len() >= 8,
        "the routes were not read from the router: {routes:?}"
    );
    let before = edge.get("127.0.0.1", "/api/view?path=/", None).await.1;
    for path in routes.iter().filter(|path| path.as_str() != "/api/preview") {
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
    let preview = edge
        .post("127.0.0.1", "/api/preview", json!({ "markdown": "a *b*" }))
        .header("cookie", &cookie);
    assert_eq!(
        edge.send(preview).await.0,
        200,
        "the preview changes nothing"
    );
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
    assert!(report["bff"]["open_sockets"].is_number());
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
    let edge = start_with(strata_backend::app::ThrottleLimits {
        source_max: 3,
        ..Default::default()
    })
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
