//! The Impact Lab projects a proposed change at a revision the caller names (AGT-422): the live
//! state when that is the current revision, the recorded history replayed to it when it is an
//! earlier one, a conflict when the caller wants the current one and the site has moved on.

use serde_json::{json, Value as Json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strata_backend::app::Config;
use strata_protocol::api::{HEADER_CORRELATION, HEADER_SERVICE, HEADER_SESSION, HEADER_SITE};
use strata_storage_api::memory::MemoryStore;

const TOKEN: &str = "test-service-token-0123456789abcdef";

struct Backend {
    base: String,
    http: reqwest::Client,
}

async fn start() -> Backend {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cfg = Config {
        addr,
        service_token: TOKEN.into(),
        dev: true,
        tenants_dir: root.join("tenants"),
        lean_dir: root.join("lean"),
        session_ttl_ms: 3_600_000,
        session_idle_ms: 3_600_000,
        throttle: Default::default(),
        passkey_challenge_ttl_ms: 300_000,
        link_ttl_ms: 900_000,
        mail: std::sync::Arc::new(strata_backend::mail::DevSink::memory()),
        federation: std::sync::Arc::new(strata_backend::signin::oidc::Federation::none()),
        factors: Default::default(),
        limits: Default::default(),
    };
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
    async fn call(&self, session: Option<&str>, path: &str, body: Json) -> (u16, Json) {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header(HEADER_SERVICE, TOKEN)
            .header(HEADER_SITE, "fieldnotes")
            .header(HEADER_CORRELATION, "t-corr-1")
            .json(&body);
        if let Some(token) = session {
            request = request.header(HEADER_SESSION, token);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Json::Null))
    }

    async fn login(&self, login: &str, password: &str) -> String {
        let (status, body) = self
            .call(
                None,
                "/internal/session/login",
                json!({ "login": login, "password": password }),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        body["token"].as_str().unwrap().to_string()
    }

    async fn overview(&self, admin: &str) -> Json {
        let (status, overview) = self
            .call(Some(admin), "/internal/lab/overview", json!({}))
            .await;
        assert_eq!(status, 200, "{overview}");
        overview
    }

    /// Start a projection and wait for its result.
    async fn projection(&self, admin: &str, extra: Json) -> Json {
        let mut request = json!({ "change": "require post.excerpt", "existing": "grandfather", "source": "generated", "steps": 20, "seed": 3 });
        for (key, value) in extra.as_object().unwrap() {
            request[key] = value.clone();
        }
        let (status, started) = self.call(Some(admin), "/internal/lab/run", request).await;
        assert_eq!(status, 200, "{started}");
        let id = started["run"].as_str().unwrap().to_string();
        for _ in 0..400 {
            let (_, run) = self
                .call(Some(admin), "/internal/lab/result", json!({ "id": id }))
                .await;
            if run["status"] == "finished" {
                assert!(run["retention"].as_str().unwrap().contains("process"));
                return run["result"].clone();
            }
            assert_eq!(run["status"], "running", "{run}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the run did not finish");
    }
}

/// A projection at the current revision, a stale one refused, one at an earlier revision taken
/// from the history, the result naming its revision, and the live tenant never changed by any of
/// them.
// Verifies: AGT-422
#[tokio::test]
async fn a_projection_is_taken_at_the_revision_the_caller_names() {
    let backend = start().await;
    let admin = backend.login("admin", "strata-dev-admin").await;
    let author = backend.login("tomasz", "strata-dev-tomasz").await;

    let first = backend.overview(&admin).await;
    let r0 = first["tenant"]["revision"].as_u64().unwrap();

    // At the current revision: the live state, and the result says which revision it was.
    let now = backend
        .projection(&admin, json!({ "revision": r0, "require_current": true }))
        .await;
    assert_eq!(now["revision"], r0);
    assert_eq!(now["current_revision"], r0);
    assert_eq!(now["snapshot"]["revision"], r0);
    assert_eq!(now["request"]["revision"], r0);
    assert_eq!(now["snapshot"]["state_hash"], first["tenant"]["state_hash"]);
    assert_eq!(now["snapshot"]["records"], first["tenant"]["records"]);

    // The site moves on by one committed operation.
    let draft = json!({ "op": "create", "object": "post", "fields": { "title": "From a lab test", "slug": "from-a-lab-test", "body": "Text.", "author": 3 } });
    let (status, created) = backend.call(Some(&author), "/internal/mutate", draft).await;
    assert_eq!(status, 200, "{created}");
    let second = backend.overview(&admin).await;
    let r1 = second["tenant"]["revision"].as_u64().unwrap();
    assert!(r1 > r0);
    assert_eq!(
        second["tenant"]["journal_entries"].as_u64().unwrap(),
        first["tenant"]["journal_entries"].as_u64().unwrap() + 1,
        "the journal count follows the commit"
    );

    // A caller that wants the current revision, and names an old one, is told what it is now.
    let (status, refused) = backend
        .call(
            Some(&admin),
            "/internal/lab/run",
            json!({ "change": "require post.excerpt", "existing": "grandfather", "source": "generated", "revision": r0, "require_current": true }),
        )
        .await;
    assert_eq!(status, 409, "{refused}");
    assert_eq!(refused["code"], "conflict");
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .contains(&format!("revision {r1}")),
        "{refused}"
    );

    // Without require_current the same revision is a projection against history: the baseline
    // is the site as it was, not as it is.
    let past = backend.projection(&admin, json!({ "revision": r0 })).await;
    assert_eq!(past["revision"], r0);
    assert_eq!(past["current_revision"], r1);
    assert_eq!(
        past["snapshot"]["state_hash"],
        first["tenant"]["state_hash"]
    );
    assert_eq!(past["snapshot"]["records"], first["tenant"]["records"]);
    let present = backend.projection(&admin, json!({ "revision": r1 })).await;
    assert_eq!(present["revision"], r1);
    assert_eq!(
        present["snapshot"]["state_hash"],
        second["tenant"]["state_hash"]
    );
    assert_ne!(
        past["snapshot"]["state_hash"], present["snapshot"]["state_hash"],
        "two revisions give two different baselines"
    );
    assert_ne!(
        past["snapshot"]["records"], present["snapshot"]["records"],
        "the later revision holds the record the commit added"
    );

    // A recorded-history projection at the earlier revision replays only what had happened.
    let recorded = backend
        .projection(&admin, json!({ "revision": r0, "source": "journal" }))
        .await;
    assert_eq!(recorded["revision"], r0);
    assert_eq!(recorded["request"]["steps"], r0);

    // No revision means the current one.
    let unnamed = backend.projection(&admin, json!({})).await;
    assert_eq!(unnamed["revision"], r1);

    // A revision the site has not reached does not exist.
    let (status, future) = backend
        .call(
            Some(&admin),
            "/internal/lab/run",
            json!({ "change": "require post.excerpt", "existing": "grandfather", "source": "generated", "revision": r1 + 5 }),
        )
        .await;
    assert_eq!(status, 422, "{future}");

    // None of it reached the live tenant.
    let after = backend.overview(&admin).await;
    assert_eq!(after["tenant"], second["tenant"]);
}

/// The overview says that runs are held in memory only, and counts the ones it has dropped.
#[tokio::test]
async fn the_overview_is_honest_about_the_runs_it_keeps() {
    let backend = start().await;
    let admin = backend.login("admin", "strata-dev-admin").await;
    let overview = backend.overview(&admin).await;
    assert_eq!(overview["runs_kept"]["most_per_site"], 12);
    assert_eq!(overview["runs_kept"]["dropped_since_start"], 0);
    assert!(overview["runs_kept"]["note"]
        .as_str()
        .unwrap()
        .contains("process"));
    let (status, gone) = backend
        .call(
            Some(&admin),
            "/internal/lab/result",
            json!({ "id": "run_000000000000" }),
        )
        .await;
    assert_eq!(status, 404);
    assert!(gone["message"].as_str().unwrap().contains("process"));
}
