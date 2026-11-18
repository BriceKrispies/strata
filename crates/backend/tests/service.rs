//! The backend as a running service: real sockets, real bundles, the in-memory store.

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value as Json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strata_backend::app::Config;
use strata_protocol::api::{HEADER_CORRELATION, HEADER_SERVICE, HEADER_SESSION, HEADER_SITE};
use strata_storage_api::memory::MemoryStore;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

const TOKEN: &str = "test-service-token-0123456789abcdef";

struct Backend {
    base: String,
    http: reqwest::Client,
}

async fn start() -> Backend {
    start_with(|_| {}).await
}

async fn start_with(adjust: impl FnOnce(&mut Config)) -> Backend {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut cfg = Config {
        addr,
        service_token: TOKEN.into(),
        dev: true,
        tenants_dir: root.join("tenants"),
        lean_dir: root.join("lean"),
        session_ttl_ms: 3_600_000,
        session_idle_ms: 3_600_000,
    };
    adjust(&mut cfg);
    tokio::spawn(strata_backend::run(
        cfg,
        Arc::new(MemoryStore::new()),
        listener,
        std::future::pending(),
    ));
    let backend = Backend {
        base: format!("http://{addr}"),
        http: reqwest::Client::new(),
    };
    for _ in 0..200 {
        if backend
            .http
            .get(format!("{}/internal/health", backend.base))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            return backend;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("backend did not start");
}

impl Backend {
    async fn call(&self, site: &str, session: Option<&str>, path: &str, body: Json) -> (u16, Json) {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header(HEADER_SERVICE, TOKEN)
            .header(HEADER_SITE, site)
            .header(HEADER_CORRELATION, "t-corr-1")
            .json(&body);
        if let Some(token) = session {
            request = request.header(HEADER_SESSION, token);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Json::Null))
    }

    async fn login(&self, site: &str, login: &str, password: &str) -> String {
        let (status, body) = self
            .call(
                site,
                None,
                "/internal/session/login",
                json!({ "login": login, "password": password }),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        body["token"].as_str().unwrap().to_string()
    }

    async fn view(&self, site: &str, session: Option<&str>, path: &str) -> Json {
        let (status, body) = self
            .call(site, session, "/internal/view", json!({ "path": path }))
            .await;
        assert_eq!(status, 200, "{body}");
        body
    }

    async fn socket(
        &self,
        site: &str,
        session: Option<&str>,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let mut request = format!("{}/internal/ws", self.base.replace("http", "ws"))
            .into_client_request()
            .unwrap();
        let headers = request.headers_mut();
        headers.insert(HEADER_SERVICE, TOKEN.parse().unwrap());
        headers.insert(HEADER_SITE, site.parse().unwrap());
        if let Some(token) = session {
            headers.insert(HEADER_SESSION, token.parse().unwrap());
        }
        tokio_tungstenite::connect_async(request).await.unwrap().0
    }
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next(socket: &mut Socket) -> Json {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("a frame in time");
        match frame.expect("socket open").expect("frame") {
            Message::Text(text) => return serde_json::from_str(&text).unwrap(),
            Message::Close(_) => return json!({ "type": "closed" }),
            _ => continue,
        }
    }
}

async fn quiet(socket: &mut Socket) -> bool {
    tokio::time::timeout(Duration::from_millis(400), socket.next())
        .await
        .is_err()
}

async fn say(socket: &mut Socket, message: Json) {
    socket
        .send(Message::Text(message.to_string()))
        .await
        .unwrap();
}

fn titles(view: &Json, section: &str) -> Vec<String> {
    view["sections"][section]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["fields"]["title"]["Str"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn requests_without_the_service_token_are_refused() {
    let backend = start().await;
    let response = backend
        .http
        .post(format!("{}/internal/view", backend.base))
        .header(HEADER_SITE, "fieldnotes")
        .json(&json!({ "path": "/" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401);
    let wrong = backend
        .http
        .post(format!("{}/internal/view", backend.base))
        .header(HEADER_SERVICE, "not-the-token-not-the-token-not")
        .header(HEADER_SITE, "fieldnotes")
        .json(&json!({ "path": "/" }))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status().as_u16(), 401);
}

#[tokio::test]
async fn the_seeded_blog_is_served_from_tenant_metadata() {
    let backend = start().await;
    let home = backend.view("fieldnotes", None, "/").await;
    assert_eq!(home["view"], "home");
    assert_eq!(titles(&home, "featured"), ["The Jig Is the Work"]);
    assert_eq!(
        titles(&home, "recent").len(),
        6,
        "six published essays; the draft is not among them"
    );
    assert!(!titles(&home, "recent").contains(&"Notes Toward a Workbench".to_string()));
    let post = backend
        .view("fieldnotes", None, "/posts/what-the-kiln-decides")
        .await;
    let author = post["sections"]["post"]["included"]["author"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap();
    assert_eq!(author["fields"]["name"]["Str"], "Mirela Ashby");
    let (status, _) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/view",
            json!({ "path": "/no/such/page" }),
        )
        .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn login_logout_and_authorized_writes() {
    let backend = start().await;
    let (status, body) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/session/login",
            json!({ "login": "tomasz", "password": "wrong" }),
        )
        .await;
    assert_eq!(status, 401, "{body}");
    assert!(
        !body.to_string().contains("wrong"),
        "the password is not echoed"
    );

    let draft = json!({ "op": "create", "object": "post", "fields": { "title": "From a test", "slug": "from-a-test", "body": "Text.", "author": 3 } });
    let (status, body) = backend
        .call("fieldnotes", None, "/internal/mutate", draft.clone())
        .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (401, Some("unauthenticated"))
    );

    let token = backend
        .login("fieldnotes", "tomasz", "strata-dev-tomasz")
        .await;
    let (_, session) = backend
        .call("fieldnotes", Some(&token), "/internal/session", json!({}))
        .await;
    assert_eq!(session["session"]["display_name"], "Tomasz Wren");
    assert!(session["session"]["roles"]
        .as_array()
        .unwrap()
        .contains(&json!("author")));
    assert_eq!(session["session"]["may_manage_schema"], false);
    assert!(session["session"]["csrf_token"].as_str().unwrap().len() >= 32);

    let (status, created) = backend
        .call("fieldnotes", Some(&token), "/internal/mutate", draft)
        .await;
    assert_eq!(status, 200, "{created}");
    let id = created["id"].as_u64().unwrap();
    assert!(titles(
        &backend.view("fieldnotes", Some(&token), "/desk").await,
        "posts"
    )
    .contains(&"From a test".to_string()));
    assert!(
        !titles(&backend.view("fieldnotes", None, "/").await, "recent")
            .contains(&"From a test".to_string())
    );

    // The kernel's evidence comes back with a rejection.
    let bad =
        json!({ "op": "update", "object": "post", "id": id, "fields": { "slug": "Not A Slug" } });
    let (status, body) = backend
        .call("fieldnotes", Some(&token), "/internal/mutate", bad)
        .await;
    assert_eq!(status, 422);
    assert_eq!(body["failed"][0]["rule"], "constraint:post.slug_format");
    assert_eq!(body["correlation"], "t-corr-1");

    // Another author may not edit it; the editor may.
    let other = backend
        .login("fieldnotes", "mirela", "strata-dev-mirela")
        .await;
    let edit =
        json!({ "op": "update", "object": "post", "id": id, "fields": { "title": "Edited" } });
    assert_eq!(
        backend
            .call("fieldnotes", Some(&other), "/internal/mutate", edit.clone())
            .await
            .0,
        403
    );
    let editor = backend.login("fieldnotes", "ines", "strata-dev-ines").await;
    assert_eq!(
        backend
            .call("fieldnotes", Some(&editor), "/internal/mutate", edit)
            .await
            .0,
        200
    );

    let publish = json!({ "op": "update", "object": "post", "id": id, "fields": { "status": "published" }, "expected_version": 2 });
    assert_eq!(
        backend
            .call(
                "fieldnotes",
                Some(&token),
                "/internal/mutate",
                publish.clone()
            )
            .await
            .0,
        200
    );
    assert_eq!(
        backend
            .call("fieldnotes", Some(&token), "/internal/mutate", publish)
            .await
            .0,
        409,
        "stale version"
    );
    assert!(
        titles(&backend.view("fieldnotes", None, "/").await, "recent")
            .contains(&"Edited".to_string())
    );

    backend
        .call(
            "fieldnotes",
            Some(&token),
            "/internal/session/logout",
            json!({}),
        )
        .await;
    let (_, after) = backend
        .call("fieldnotes", Some(&token), "/internal/session", json!({}))
        .await;
    assert_eq!(after["session"]["authenticated"], false);
    assert_eq!(after["state"], "revoked");
}

#[tokio::test]
async fn a_new_login_ends_the_session_it_replaces() {
    let backend = start().await;
    let first = backend.login("fieldnotes", "ines", "strata-dev-ines").await;
    let (status, body) = backend
        .call(
            "fieldnotes",
            Some(&first),
            "/internal/session/login",
            json!({ "login": "ines", "password": "strata-dev-ines" }),
        )
        .await;
    assert_eq!(status, 200);
    let second = body["token"].as_str().unwrap();
    assert_ne!(first, second);
    let (_, old) = backend
        .call("fieldnotes", Some(&first), "/internal/session", json!({}))
        .await;
    assert_eq!(old["session"]["authenticated"], false);
}

#[tokio::test]
async fn a_session_of_one_tenant_is_nobody_on_another() {
    let backend = start().await;
    let tide = backend
        .login("tidewater", "admin", "strata-dev-tidewater")
        .await;
    let (_, here) = backend
        .call("tidewater", Some(&tide), "/internal/session", json!({}))
        .await;
    assert_eq!(
        here["session"]["may_manage_schema"], true,
        "owner of tidewater"
    );

    // The same token presented on the fieldnotes site carries no identity at all.
    let (_, there) = backend
        .call("fieldnotes", Some(&tide), "/internal/session", json!({}))
        .await;
    assert_eq!(there["session"]["authenticated"], false);
    assert_eq!(there["session"]["may_manage_schema"], false);
    let desk = backend.view("fieldnotes", Some(&tide), "/desk").await;
    assert!(
        titles(&desk, "posts").is_empty(),
        "no fieldnotes drafts for a tidewater owner"
    );
    let steal =
        json!({ "op": "update", "object": "post", "id": 9, "fields": { "title": "Taken" } });
    assert_eq!(
        backend
            .call("fieldnotes", Some(&tide), "/internal/mutate", steal)
            .await
            .0,
        401
    );

    // Record ids overlap between tenants; a tidewater id resolves to tidewater's record only.
    let own_desk = backend.view("tidewater", Some(&tide), "/desk").await;
    assert_eq!(titles(&own_desk, "posts").len(), 2);
    let fieldnotes_admin = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    let fn_desk = backend
        .view("fieldnotes", Some(&fieldnotes_admin), "/desk")
        .await;
    assert!(!titles(&fn_desk, "posts")
        .iter()
        .any(|t| t.contains("Tidewater")));
    assert!(!backend
        .view("fieldnotes", None, "/")
        .await
        .to_string()
        .contains("Tidewater"));
    // Fieldnotes logins do not exist on tidewater.
    let (status, _) = backend
        .call(
            "tidewater",
            None,
            "/internal/session/login",
            json!({ "login": "ines", "password": "strata-dev-ines" }),
        )
        .await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn subscribers_hear_only_what_they_may_read_and_can_resume() {
    let backend = start().await;
    let author = backend
        .login("fieldnotes", "tomasz", "strata-dev-tomasz")
        .await;

    let mut public = backend.socket("fieldnotes", None).await;
    let welcome = next(&mut public).await;
    assert_eq!(
        (welcome["type"].as_str(), welcome["v"].as_u64()),
        (Some("welcome"), Some(1))
    );
    assert_eq!(welcome["session"], "anonymous");
    let head = welcome["head"].as_u64().unwrap();
    say(
        &mut public,
        json!({ "type": "subscribe", "req": "r1", "sub": "all" }),
    )
    .await;
    assert_eq!(
        next(&mut public).await,
        json!({ "type": "ok", "req": "r1" })
    );

    let mut writer = backend.socket("fieldnotes", Some(&author)).await;
    assert_eq!(next(&mut writer).await["session"], "active");
    say(
        &mut writer,
        json!({ "type": "subscribe", "req": "r1", "sub": "posts", "objects": ["post"] }),
    )
    .await;
    next(&mut writer).await;
    let mut other_tenant = backend.socket("tidewater", None).await;
    next(&mut other_tenant).await;
    say(
        &mut other_tenant,
        json!({ "type": "subscribe", "req": "r1", "sub": "all" }),
    )
    .await;
    next(&mut other_tenant).await;

    // A draft: the author hears of it, the public and the other tenant hear nothing.
    let draft = json!({ "op": "create", "object": "post", "fields": { "title": "Live", "slug": "live", "body": "Text.", "author": 3 } });
    let (_, created) = backend
        .call("fieldnotes", Some(&author), "/internal/mutate", draft)
        .await;
    let id = created["id"].as_u64().unwrap();
    let heard = next(&mut writer).await;
    assert_eq!(
        (heard["type"].as_str(), heard["n"].as_u64()),
        (Some("event"), Some(1))
    );
    assert_eq!(
        heard["notice"],
        json!({ "kind": "upsert", "object": "post", "id": id, "version": 1 })
    );
    assert_eq!(heard["correlation"], "t-corr-1");
    assert!(
        quiet(&mut public).await,
        "a draft is not announced to the public"
    );

    // Publishing it: now the public hears, as its first event.
    let publish =
        json!({ "op": "update", "object": "post", "id": id, "fields": { "status": "published" } });
    backend
        .call("fieldnotes", Some(&author), "/internal/mutate", publish)
        .await;
    let announced = next(&mut public).await;
    assert_eq!(announced["n"], 1);
    assert_eq!(announced["notice"]["kind"], "upsert");
    let seen = announced["seq"].as_u64().unwrap();
    assert_eq!(
        seen,
        head + 2,
        "the sequence skipped the draft the public could not see"
    );
    assert!(
        quiet(&mut other_tenant).await,
        "nothing crosses the tenant boundary"
    );

    // Disconnect, miss an unpublish, resume from the last sequence seen.
    drop(public);
    let unpublish =
        json!({ "op": "update", "object": "post", "id": id, "fields": { "status": "draft" } });
    backend
        .call("fieldnotes", Some(&author), "/internal/mutate", unpublish)
        .await;
    let mut public = backend.socket("fieldnotes", None).await;
    assert_eq!(next(&mut public).await["head"].as_u64(), Some(seen + 1));
    say(
        &mut public,
        json!({ "type": "subscribe", "req": "r2", "sub": "all", "after": seen }),
    )
    .await;
    next(&mut public).await;
    let missed = next(&mut public).await;
    assert_eq!(
        missed["notice"],
        json!({ "kind": "remove", "object": "post", "id": id })
    );

    // Heartbeat and malformed frames.
    say(&mut public, json!({ "type": "ping", "nonce": 7 })).await;
    assert_eq!(
        next(&mut public).await,
        json!({ "type": "pong", "nonce": 7, "head": seen + 1 })
    );
    say(
        &mut public,
        json!({ "type": "impersonate", "principal": "user:admin" }),
    )
    .await;
    assert_eq!(next(&mut public).await["type"], "error");

    // Logging out ends the author's connection.
    backend
        .call(
            "fieldnotes",
            Some(&author),
            "/internal/session/logout",
            json!({}),
        )
        .await;
    loop {
        let message = next(&mut writer).await;
        if message["type"] == "session" {
            assert_eq!(message["state"], "revoked");
            break;
        }
    }
}

#[tokio::test]
async fn sessions_expire_and_diagnostics_hold_no_secrets() {
    let backend = start_with(|cfg| cfg.session_idle_ms = 3_600_000).await;
    let admin = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    let (status, diagnostics) = backend
        .call(
            "fieldnotes",
            Some(&admin),
            "/internal/diagnostics",
            json!({}),
        )
        .await;
    assert_eq!(status, 200, "{diagnostics}");
    let text = diagnostics.to_string();
    assert!(!text.contains(&admin), "no session token in diagnostics");
    assert!(
        !text.contains("strata-dev-admin") && !text.contains("argon2"),
        "no credentials in diagnostics"
    );
    assert_eq!(diagnostics["sessions"].as_array().unwrap().len(), 1);
    assert!(diagnostics["tenants"].as_array().unwrap().len() >= 2);
    assert!(
        diagnostics["transitions"]["create_record"][0]
            .as_u64()
            .unwrap()
            > 0
    );

    let author = backend
        .login("fieldnotes", "mirela", "strata-dev-mirela")
        .await;
    assert_eq!(
        backend
            .call(
                "fieldnotes",
                Some(&author),
                "/internal/diagnostics",
                json!({})
            )
            .await
            .0,
        403
    );
    assert_eq!(
        backend
            .call("fieldnotes", None, "/internal/diagnostics", json!({}))
            .await
            .0,
        401
    );

    let (_, expired) = backend
        .call(
            "fieldnotes",
            Some(&author),
            "/internal/dev/expire-session",
            json!({}),
        )
        .await;
    assert_eq!(expired["expired"], true);
    let (_, after) = backend
        .call("fieldnotes", Some(&author), "/internal/session", json!({}))
        .await;
    assert_eq!(
        (
            after["session"]["authenticated"].as_bool(),
            after["state"].as_str()
        ),
        (Some(false), Some("expired"))
    );
    let write = json!({ "op": "update", "object": "post", "id": 9, "fields": { "title": "Late" } });
    assert_eq!(
        backend
            .call("fieldnotes", Some(&author), "/internal/mutate", write)
            .await
            .0,
        401
    );
}

#[tokio::test]
async fn repeated_wrong_passwords_are_throttled() {
    let backend = start().await;
    let attempt = json!({ "login": "ines", "password": "nope" });
    for _ in 0..5 {
        assert_eq!(
            backend
                .call(
                    "fieldnotes",
                    None,
                    "/internal/session/login",
                    attempt.clone()
                )
                .await
                .0,
            401
        );
    }
    assert_eq!(
        backend
            .call("fieldnotes", None, "/internal/session/login", attempt)
            .await
            .0,
        429
    );
    let right = json!({ "login": "ines", "password": "strata-dev-ines" });
    assert_eq!(
        backend
            .call("fieldnotes", None, "/internal/session/login", right)
            .await
            .0,
        429,
        "locked for now"
    );
}

#[tokio::test]
async fn fixture_reset_rebuilds_the_same_state() {
    let backend = start().await;
    let admin = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    let before = backend.view("fieldnotes", None, "/").await;
    let extra = json!({ "op": "create", "object": "category", "fields": { "name": "Extra", "slug": "extra" } });
    assert_eq!(
        backend
            .call("fieldnotes", Some(&admin), "/internal/mutate", extra)
            .await
            .0,
        200
    );
    assert_ne!(
        backend.view("fieldnotes", None, "/").await["revision"],
        before["revision"]
    );
    let (status, _) = backend
        .call("fieldnotes", None, "/internal/dev/reset", json!({}))
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        backend.view("fieldnotes", None, "/").await,
        before,
        "seeding is deterministic"
    );
}

async fn finished_run(backend: &Backend, admin: &str, request: Json) -> Json {
    let (status, started) = backend
        .call("fieldnotes", Some(admin), "/internal/lab/run", request)
        .await;
    assert_eq!(status, 200, "{started}");
    let id = started["run"].as_str().unwrap().to_string();
    for _ in 0..400 {
        let (_, run) = backend
            .call(
                "fieldnotes",
                Some(admin),
                "/internal/lab/result",
                json!({ "id": id }),
            )
            .await;
        if run["status"] == "finished" {
            return run["result"].clone();
        }
        assert_eq!(run["status"], "running");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the run did not finish");
}

#[tokio::test]
async fn the_impact_lab_projects_a_change_without_touching_live_state() {
    let backend = start().await;
    let admin = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    let author = backend
        .login("fieldnotes", "tomasz", "strata-dev-tomasz")
        .await;
    assert_eq!(
        backend
            .call(
                "fieldnotes",
                Some(&author),
                "/internal/lab/overview",
                json!({})
            )
            .await
            .0,
        403
    );
    assert_eq!(
        backend
            .call("fieldnotes", None, "/internal/lab/overview", json!({}))
            .await
            .0,
        401
    );

    let (status, overview) = backend
        .call(
            "fieldnotes",
            Some(&admin),
            "/internal/lab/overview",
            json!({}),
        )
        .await;
    assert_eq!(status, 200, "{overview}");
    let post = overview["objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == "post")
        .unwrap();
    assert_eq!(post["records"], 7);
    assert!(post["fields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["id"] == "excerpt" && f["required"] == false));
    assert_eq!(overview["classes"].as_array().unwrap().len(), 5);
    let before = overview["tenant"].clone();

    // A proposal the parser refuses is an ordinary validation error.
    let bad =
        json!({ "change": "require post.nonsense", "existing": "reject", "source": "generated" });
    let (status, body) = backend
        .call("fieldnotes", Some(&admin), "/internal/lab/run", bad)
        .await;
    assert_eq!(status, 422);
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("post has no field nonsense"));

    let mut socket = backend.socket("fieldnotes", Some(&admin)).await;
    next(&mut socket).await;
    let request = json!({ "change": "require post.excerpt", "existing": "grandfather", "source": "generated", "steps": 60, "seed": 5, "faults": true });
    let result = finished_run(&backend, &admin, request.clone()).await;
    assert_eq!(result["change_outcome"]["result"], "accepted");
    assert_eq!(result["affected_existing"].as_array().unwrap().len(), 3);
    assert!(!result["comparison"]["outcome_changes"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(result["comparison"]["other_tenants_untouched"], true);
    assert_eq!(
        result["trace_replay"]["agreed"],
        result["trace_replay"]["steps"]
    );
    assert_eq!(result["findings"].as_array().unwrap().len(), 7);
    assert!(result["counterexample"]["steps"].as_array().unwrap().len() <= 3);

    // Progress and the result arrived on the socket of the person who started it.
    let mut kinds = Vec::new();
    loop {
        let message = next(&mut socket).await;
        if message["type"] == "run" {
            kinds.push(message["kind"].as_str().unwrap().to_string());
            if message["kind"] == "finished" {
                break;
            }
        }
    }
    assert!(kinds.contains(&"progress".to_string()));

    // The same request again: the same two trace hashes.
    let again = finished_run(&backend, &admin, request).await;
    // The generated schedule is stamped with the time of the run, so hashes are compared for a
    // recorded schedule instead, where every input is fixed.
    assert_eq!(
        again["comparison"]["outcome_changes"]
            .as_array()
            .unwrap()
            .len(),
        result["comparison"]["outcome_changes"]
            .as_array()
            .unwrap()
            .len()
    );
    let recorded =
        json!({ "change": "require post.excerpt", "existing": "grandfather", "source": "journal" });
    let first = finished_run(&backend, &admin, recorded.clone()).await;
    let second = finished_run(&backend, &admin, recorded).await;
    assert_eq!(
        first["baseline"]["trace_hash"],
        second["baseline"]["trace_hash"]
    );
    assert_eq!(
        first["branch"]["trace_hash"],
        second["branch"]["trace_hash"]
    );
    let refused: Vec<&str> = first["comparison"]["outcome_changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["label"].as_str().unwrap())
        .collect();
    // The first refusal is the first article written without an excerpt. Later steps refer to
    // record ids, which shift once a create is refused, so they diverge as a knock-on.
    assert!(refused[0].starts_with("create record"), "{refused:?}");
    assert!(refused.len() >= 5);
    assert_eq!(
        first["comparison"]["outcome_changes"][0]["because"][0]["rule"],
        "required:post.excerpt"
    );

    // Nothing the Lab did reached the live tenant.
    let (_, after) = backend
        .call(
            "fieldnotes",
            Some(&admin),
            "/internal/lab/overview",
            json!({}),
        )
        .await;
    assert_eq!(after["tenant"], before);
    assert!(after["objects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|o| o["id"] == "post"
            && o["fields"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["id"] == "excerpt" && f["required"] == false)));

    // The journal of what really happened replays to the live state.
    let (status, replayed) = backend
        .call(
            "fieldnotes",
            Some(&admin),
            "/internal/lab/replay",
            json!({}),
        )
        .await;
    assert_eq!(status, 200, "{replayed}");
    assert_eq!(replayed["report"]["matches_live"], true);
    assert_eq!(replayed["report"]["agreed"], replayed["report"]["entries"]);
    assert_eq!(replayed["class"], "reproduced");
}
