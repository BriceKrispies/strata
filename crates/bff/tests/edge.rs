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

struct Edge {
    port: u16,
    http: reqwest::Client,
}

async fn start() -> Edge {
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
    };
    tokio::spawn(strata_backend::run(backend_cfg, Arc::new(MemoryStore::new()), backend_listener, std::future::pending()));

    let dist = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("dist-{}", std::process::id()));
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
        origins: vec![format!("http://127.0.0.1:{port}"), format!("http://localhost:{port}")],
        dist,
        dev: true,
        secure_cookies: false,
    };
    tokio::spawn(strata_bff::run(cfg, listener, std::future::pending()));
    let edge = Edge { port, http: reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap() };
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
        (status, body, Reply { cookie, headers, text })
    }

    async fn get(&self, host: &str, path: &str, cookie: Option<&str>) -> (u16, Json, Reply) {
        let mut request = self.http.get(format!("{}{path}", self.origin(host)));
        if let Some(cookie) = cookie {
            request = request.header("cookie", format!("strata_sid={cookie}"));
        }
        self.send(request).await
    }

    fn post(&self, host: &str, path: &str, body: Json) -> reqwest::RequestBuilder {
        self.http.post(format!("{}{path}", self.origin(host))).header("origin", self.origin(host)).json(&body)
    }

    /// Sign in and return `(cookie token, csrf token)`.
    async fn login(&self, host: &str, login: &str, password: &str) -> (String, String) {
        let request = self
            .post(host, "/api/login", json!({ "login": login, "password": password }))
            .header("x-strata-request", "1");
        let (status, body, reply) = self.send(request).await;
        assert_eq!(status, 200, "{body}");
        let cookie = reply.cookie.expect("a session cookie");
        let token = cookie.split(';').next().unwrap().trim_start_matches("strata_sid=").to_string();
        (token, body["session"]["csrf_token"].as_str().unwrap().to_string())
    }
}

#[tokio::test]
async fn the_session_cookie_is_http_only_strict_and_never_in_a_body() {
    let edge = start().await;
    let request = edge
        .post("127.0.0.1", "/api/login", json!({ "login": "ines", "password": "strata-dev-ines" }))
        .header("x-strata-request", "1");
    let (status, body, reply) = edge.send(request).await;
    assert_eq!(status, 200);
    let cookie = reply.cookie.unwrap();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict") && cookie.contains("Path=/"), "{cookie}");
    let token = cookie.split(';').next().unwrap().trim_start_matches("strata_sid=");
    assert_eq!(token.len(), 64);
    assert!(!reply.text.contains(token), "the token is only ever in the cookie");
    assert_eq!(body["session"]["display_name"], "Inès Okafor");

    let (_, session, _) = edge.get("127.0.0.1", "/api/session", Some(token)).await;
    assert_eq!(session["session"]["authenticated"], true);
    // An ended session makes the BFF clear the cookie.
    let (csrf_status, _, _) = edge
        .send(edge.post("127.0.0.1", "/api/logout", json!({})).header("cookie", format!("strata_sid={token}")).header(
            "x-csrf-token",
            session["session"]["csrf_token"].as_str().unwrap(),
        ))
        .await;
    assert_eq!(csrf_status, 200);
    let (_, after, reply) = edge.get("127.0.0.1", "/api/session", Some(token)).await;
    assert_eq!(after["session"]["authenticated"], false);
    assert!(reply.cookie.unwrap().contains("Max-Age=0"));
}

#[tokio::test]
async fn writes_need_our_origin_and_the_sessions_csrf_token() {
    let edge = start().await;
    let (token, csrf) = edge.login("127.0.0.1", "ines", "strata-dev-ines").await;
    let cookie = format!("strata_sid={token}");
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
    assert_eq!(edge.send(write(Some(ours.clone()), None)).await.0, 403, "no CSRF token");
    assert_eq!(edge.send(write(Some(ours.clone()), Some("0000"))).await.0, 403, "wrong CSRF token");
    assert_eq!(edge.send(write(Some("http://evil.example".into()), Some(&csrf))).await.0, 403, "foreign origin");
    assert_eq!(edge.send(write(None, Some(&csrf))).await.0, 403, "no origin at all");
    assert_eq!(edge.send(write(Some(ours), Some(&csrf))).await.0, 200);

    // Login is guarded too, before any session exists.
    let cross_site = edge
        .http
        .post(format!("{}/api/login", edge.origin("127.0.0.1")))
        .header("origin", "http://evil.example")
        .header("x-strata-request", "1")
        .json(&json!({ "login": "ines", "password": "strata-dev-ines" }));
    assert_eq!(edge.send(cross_site).await.0, 403);
    let form_post = edge.post("127.0.0.1", "/api/login", json!({ "login": "ines", "password": "strata-dev-ines" }));
    assert_eq!(edge.send(form_post).await.0, 403, "missing the header a form cannot send");
}

#[tokio::test]
async fn the_site_comes_from_the_host_and_nothing_the_client_says() {
    let edge = start().await;
    let (_, fieldnotes, _) = edge.get("127.0.0.1", "/api/view?path=/", None).await;
    assert_eq!(fieldnotes["data"]["site"]["fields"]["name"], "Fieldnotes");
    assert_eq!(fieldnotes["theme"], json!({ "accent": "moss", "paper": "linen" }));
    let (_, tidewater, _) = edge.get("localhost", "/api/view?path=/", None).await;
    assert_eq!(tidewater["data"]["site"]["fields"]["name"], "Tidewater Review");
    assert_eq!(tidewater["theme"]["accent"], "ink");

    // Internal headers and parameters from a browser are not a way in.
    let spoof = edge
        .http
        .get(format!("{}/api/view?path=/&site=tidewater&tenant=tidewater", edge.origin("127.0.0.1")))
        .header("x-strata-site", "tidewater")
        .header("x-strata-service", TOKEN);
    let (_, spoofed, _) = edge.send(spoof).await;
    assert_eq!(spoofed["data"]["site"]["fields"]["name"], "Fieldnotes");

    // A fieldnotes session cookie replayed against the tidewater host is nobody there.
    let (token, _) = edge.login("127.0.0.1", "admin", "strata-dev-admin").await;
    let (_, there, _) = edge.get("localhost", "/api/session", Some(&token)).await;
    assert_eq!(there["session"]["authenticated"], false);
    let (_, desk, _) = edge.get("localhost", "/api/view?path=/desk", Some(&token)).await;
    assert_eq!(desk["data"]["posts"], json!([]));
    assert!(!edge.get("127.0.0.1", "/api/view?path=/desk", Some(&token)).await.2.text.contains("Tidewater"));

    let unknown = edge.http.get(format!("{}/api/session", edge.origin("127.0.0.1"))).header("host", "elsewhere.example");
    assert_eq!(edge.send(unknown).await.0, 404);
}

#[tokio::test]
async fn views_are_projected_to_plain_json_with_markdown_as_blocks() {
    let edge = start().await;
    let (status, view, _) = edge.get("127.0.0.1", "/api/view?path=/posts/the-jig-is-the-work", None).await;
    assert_eq!(status, 200);
    let post = &view["data"]["post"];
    assert_eq!(post["fields"]["title"], "The Jig Is the Work", "plain values, not kernel-tagged ones");
    assert_eq!(post["refs"]["author"]["fields"]["name"], "Tomasz Wren");
    assert_eq!(post["refs"]["category"]["fields"]["slug"], "craft");
    assert_eq!(post["blocks"]["body"][0]["type"], "paragraph");
    assert!(post["reading_minutes"].as_u64().unwrap() >= 2);
    assert!(view["depends_on"].as_array().unwrap().contains(&json!("post")));

    let (_, search, _) = edge.get("127.0.0.1", "/api/view?path=/search&q=kiln", None).await;
    let found: Vec<&str> = search["data"]["results"].as_array().unwrap().iter().map(|r| r["fields"]["slug"].as_str().unwrap()).collect();
    assert_eq!(found, ["what-the-kiln-decides"]);
    let (status, missing, _) = edge.get("127.0.0.1", "/api/view?path=/nope/nope", None).await;
    assert_eq!((status, missing["code"].as_str()), (404, Some("not_found")));
    let (_, draft, _) = edge.get("127.0.0.1", "/api/view?path=/posts/notes-toward-a-workbench", None).await;
    assert_eq!(draft["data"]["post"], Json::Null, "a draft is not there for the public");
}

#[tokio::test]
async fn pages_carry_security_headers_and_a_bootstrap_that_cannot_break_out() {
    let edge = start().await;
    let (status, _, reply) = edge.get("127.0.0.1", "/posts/a-margin-is-a-promise", None).await;
    assert_eq!(status, 200);
    let csp = reply.headers.get("content-security-policy").unwrap().to_str().unwrap();
    assert!(csp.contains("script-src 'self'") && csp.contains("frame-ancestors 'none'") && !csp.contains("unsafe-inline"));
    assert_eq!(reply.headers.get("x-content-type-options").unwrap(), "nosniff");
    assert!(reply.text.contains("<title>Article · Fieldnotes</title>"));
    assert!(reply.text.contains("data-accent=\"moss\""));
    assert!(reply.text.contains("id=\"strata-bootstrap\""));
    let data = reply.text.split("id=\"strata-bootstrap\">").nth(1).unwrap().split("</script>").next().unwrap();
    assert!(!data.contains('<'), "no markup can start inside the data block");
    let bootstrap: Json = serde_json::from_str(data).unwrap();
    assert_eq!(bootstrap["view"]["data"]["post"]["fields"]["slug"], "a-margin-is-a-promise");
    assert_eq!(bootstrap["session"]["session"]["authenticated"], false);
    assert_eq!(edge.get("127.0.0.1", "/no/such/page", None).await.0, 404);
}

#[tokio::test]
async fn the_socket_checks_origin_validates_frames_and_relays_events() {
    let edge = start().await;
    let connect = |origin: Option<String>, cookie: Option<String>| {
        let mut request = format!("ws://127.0.0.1:{}/ws", edge.port).into_client_request().unwrap();
        if let Some(origin) = origin {
            request.headers_mut().insert("origin", origin.parse().unwrap());
        }
        if let Some(cookie) = cookie {
            request.headers_mut().insert("cookie", format!("strata_sid={cookie}").parse().unwrap());
        }
        tokio_tungstenite::connect_async(request)
    };
    assert!(connect(Some("http://evil.example".into()), None).await.is_err(), "foreign origin is refused at upgrade");
    assert!(connect(None, None).await.is_err(), "so is a missing origin");

    let (token, csrf) = edge.login("127.0.0.1", "ines", "strata-dev-ines").await;
    let (mut socket, _) = connect(Some(edge.origin("127.0.0.1")), Some(token.clone())).await.unwrap();
    async fn next(socket: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin)) -> Json {
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(5), socket.next()).await.unwrap().unwrap().unwrap();
            if let Message::Text(text) = frame {
                return serde_json::from_str(&text).unwrap();
            }
        }
    }
    let welcome = next(&mut socket).await;
    assert_eq!((welcome["type"].as_str(), welcome["session"].as_str()), (Some("welcome"), Some("active")));
    socket.send(Message::Text(json!({ "type": "become", "principal": "user:admin" }).to_string())).await.unwrap();
    assert_eq!(next(&mut socket).await["type"], "error", "the relay refuses frames outside the protocol");
    socket.send(Message::Text(json!({ "type": "subscribe", "req": "a", "sub": "s" }).to_string())).await.unwrap();
    assert_eq!(next(&mut socket).await, json!({ "type": "ok", "req": "a" }));

    let change = json!({ "op": "create", "object": "category", "fields": { "name": "Live", "slug": "live" } });
    let write = edge
        .post("127.0.0.1", "/api/records", change)
        .header("cookie", format!("strata_sid={token}"))
        .header("x-csrf-token", &csrf)
        .header("x-correlation-id", "c-browser-42");
    assert_eq!(edge.send(write).await.0, 200);
    let event = next(&mut socket).await;
    assert_eq!(event["notice"]["kind"], "upsert");
    assert_eq!(event["correlation"], "c-browser-42", "the browser's correlation id comes back with the event");
}
