//! The tenant ledger through the backend's HTTP API (`/internal/ledger/<operation>`): the owner
//! declares a unit and its limits and posts; a retry answers once; a key reused with another
//! request is a conflict; limits refuse at their boundary; two concurrent postings cannot both
//! pass the cap; a member reads only their own account; nothing crosses to another tenant.

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
        mail: Arc::new(strata_backend::mail::DevSink::memory()),
        federation: Arc::new(strata_backend::signin::oidc::Federation::none()),
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
    async fn call(&self, site: &str, session: Option<&str>, path: &str, body: Json) -> (u16, Json) {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header(HEADER_SERVICE, TOKEN)
            .header(HEADER_SITE, site)
            .header(HEADER_CORRELATION, "t-ledger-1")
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

    async fn ledger(&self, site: &str, session: Option<&str>, op: &str, body: Json) -> (u16, Json) {
        self.call(site, session, &format!("/internal/ledger/{op}"), body)
            .await
    }
}

fn person(principal: &str) -> Json {
    json!({ "principal": { "principal": principal } })
}

fn posting(owner: Json, amount: i64, key: &str) -> Json {
    json!({ "owner": owner, "unit": "points", "amount": amount, "idempotency_key": key, "reason": "test" })
}

/// The owner of fieldnotes declares `points` with a floor of 0 and a cap of 100.
async fn declare_points(backend: &Backend, admin: &str) {
    let (status, body) = backend
        .ledger(
            "fieldnotes",
            Some(admin),
            "declare-unit",
            json!({ "unit": "points", "description": "Points" }),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = backend
        .ledger(
            "fieldnotes",
            Some(admin),
            "set-limit",
            json!({ "limit": { "unit": "points", "floor": 0, "cap": 100 } }),
        )
        .await;
    assert_eq!(status, 200, "{body}");
}

/// Verifies: LDG-006, LDG-007, LDG-013, LDG-1541
#[tokio::test]
async fn the_owner_posts_a_retry_counts_once_and_a_member_reads_only_their_own() {
    let backend = start().await;
    let admin = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    let tomasz = backend
        .login("fieldnotes", "tomasz", "strata-dev-tomasz")
        .await;
    declare_points(&backend, &admin).await;

    let (status, first) = backend
        .ledger(
            "fieldnotes",
            Some(&admin),
            "post",
            posting(person("user:tomasz"), 30, "welcome"),
        )
        .await;
    assert_eq!(status, 200, "{first}");
    assert_eq!(first["result"]["balance"]["balance"], 30);
    assert_eq!(first["result"]["entry"]["sequence"], 1);
    assert_eq!(first["result"]["replayed"], false);

    // The same key and request: the first entry again, and the balance counts it once.
    let (status, again) = backend
        .ledger(
            "fieldnotes",
            Some(&admin),
            "post",
            posting(person("user:tomasz"), 30, "welcome"),
        )
        .await;
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["result"]["replayed"], true);
    assert_eq!(again["result"]["entry"], first["result"]["entry"]);
    // The same key with another amount is a conflict.
    let (status, body) = backend
        .ledger(
            "fieldnotes",
            Some(&admin),
            "post",
            posting(person("user:tomasz"), 31, "welcome"),
        )
        .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (409, Some("conflict")),
        "{body}"
    );
    // The cap of 100: 70 more fits, one more does not.
    let (status, body) = backend
        .ledger(
            "fieldnotes",
            Some(&admin),
            "post",
            posting(person("user:tomasz"), 71, "over"),
        )
        .await;
    assert_eq!(status, 422, "{body}");
    assert!(body["failed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check["rule"] == "ledger:above-cap"));
    let (status, body) = backend
        .ledger(
            "fieldnotes",
            Some(&admin),
            "post",
            posting(person("user:tomasz"), 70, "fill"),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["result"]["balance"]["balance"], 100);

    // The member reads their own balance and entries, and nothing of anyone else's.
    let (status, mine) = backend
        .ledger(
            "fieldnotes",
            Some(&tomasz),
            "my-balance",
            json!({ "unit": "points" }),
        )
        .await;
    assert_eq!(status, 200, "{mine}");
    assert_eq!(mine["balance"]["balance"], 100);
    assert_eq!(mine["balance"]["last_sequence"], 2);
    let (status, listed) = backend
        .ledger(
            "fieldnotes",
            Some(&tomasz),
            "my-entries",
            json!({ "unit": "points", "limit": 1 }),
        )
        .await;
    assert_eq!(status, 200, "{listed}");
    assert_eq!(listed["entries"].as_array().unwrap().len(), 1);
    let cursor = listed["next"].as_str().unwrap().to_string();
    let (_, rest) = backend
        .ledger(
            "fieldnotes",
            Some(&tomasz),
            "my-entries",
            json!({ "unit": "points", "cursor": cursor }),
        )
        .await;
    assert_eq!(rest["entries"][0]["sequence"], 2);
    assert!(rest["next"].is_null());
    let (status, body) = backend
        .ledger(
            "fieldnotes",
            Some(&tomasz),
            "balance",
            json!({ "owner": person("user:ines"), "unit": "points" }),
        )
        .await;
    assert_eq!(status, 403, "{body}");
    let (status, _) = backend
        .ledger(
            "fieldnotes",
            Some(&tomasz),
            "my-balance",
            json!({ "owner": person("user:ines"), "unit": "points" }),
        )
        .await;
    assert_eq!(status, 422, "a read of one's own account names no owner");
    for op in ["accounts", "definitions"] {
        let (status, body) = backend
            .ledger(
                "fieldnotes",
                Some(&tomasz),
                op,
                json!({ "unit": "points", "order": "greatest-first" }),
            )
            .await;
        assert_eq!(status, 403, "{op}: {body}");
    }
    // A member cannot post, and a visitor has no account of their own.
    let (status, _) = backend
        .ledger(
            "fieldnotes",
            Some(&tomasz),
            "post",
            posting(person("user:tomasz"), 1, "self"),
        )
        .await;
    assert_eq!(status, 403);
    let (status, _) = backend
        .ledger(
            "fieldnotes",
            None,
            "my-balance",
            json!({ "unit": "points" }),
        )
        .await;
    assert_eq!(status, 401);

    // The owner reads any account, ranks the unit and lists the definitions.
    let (status, body) = backend
        .ledger(
            "fieldnotes",
            Some(&admin),
            "balance",
            json!({ "owner": person("user:tomasz"), "unit": "points" }),
        )
        .await;
    assert_eq!(
        (status, body["balance"]["balance"].as_i64()),
        (200, Some(100))
    );
    let (status, ranked) = backend
        .ledger(
            "fieldnotes",
            Some(&admin),
            "accounts",
            json!({ "unit": "points", "order": "greatest-first" }),
        )
        .await;
    assert_eq!(status, 200, "{ranked}");
    assert_eq!(ranked["accounts"][0]["owner"], person("user:tomasz"));
    let (status, body) = backend
        .ledger(
            "fieldnotes",
            Some(&admin),
            "accounts",
            json!({ "unit": "points", "order": "greatest-first", "cursor": "9990", "limit": 20 }),
        )
        .await;
    assert_eq!(status, 422, "past the scan bound: {body}");
    let (status, defs) = backend
        .ledger("fieldnotes", Some(&admin), "definitions", json!({}))
        .await;
    assert_eq!(status, 200, "{defs}");
    assert_eq!(defs["definitions"]["units"], json!(["points"]));
    // A unit an account names is not removed.
    let (status, body) = backend
        .ledger(
            "fieldnotes",
            Some(&admin),
            "remove-definition",
            json!({ "definition": { "unit": { "unit": "points" } } }),
        )
        .await;
    assert_eq!(status, 422, "{body}");
}

/// Verifies: LDG-017
///
/// Two postings sent at once, each of which fits under the cap alone: one is applied and the other
/// is refused, decided again after it lost the commit race (D-5009), and the balance never passes
/// the cap.
#[tokio::test]
async fn two_concurrent_postings_cannot_both_pass_the_cap() {
    let backend = start().await;
    let admin = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    declare_points(&backend, &admin).await;
    for round in 0..5 {
        let pool = json!({ "principal": { "principal": "user:ines" } });
        let scope = format!("round-{round}");
        let one = |key: &str| {
            let mut body = posting(pool.clone(), 60, key);
            body["scope"] = json!(scope);
            backend.ledger("fieldnotes", Some(&admin), "post", body)
        };
        let ((a, _), (b, _)) = tokio::join!(one("a"), one("b"));
        let mut statuses = [a, b];
        statuses.sort();
        assert_eq!(statuses, [200, 422], "round {round}");
        let (_, body) = backend
            .ledger(
                "fieldnotes",
                Some(&admin),
                "balance",
                json!({ "owner": pool, "unit": "points", "scope": scope }),
            )
            .await;
        assert_eq!(body["balance"]["balance"], 60);
    }
}

/// Verifies: LDG-001
#[tokio::test]
async fn nothing_of_one_tenants_ledger_reaches_another() {
    let backend = start().await;
    let admin = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    declare_points(&backend, &admin).await;
    let (status, _) = backend
        .ledger(
            "fieldnotes",
            Some(&admin),
            "post",
            posting(person("user:tomasz"), 5, "a"),
        )
        .await;
    assert_eq!(status, 200);

    // The owner of tidewater finds no unit points there.
    let harbour = backend
        .login("tidewater", "admin", "strata-dev-tidewater")
        .await;
    let (status, body) = backend
        .ledger(
            "tidewater",
            Some(&harbour),
            "balance",
            json!({ "owner": person("user:tomasz"), "unit": "points" }),
        )
        .await;
    assert_eq!(status, 404, "{body}");
    let (_, defs) = backend
        .ledger("tidewater", Some(&harbour), "definitions", json!({}))
        .await;
    assert_eq!(defs["definitions"]["units"], json!([]));
    // A request that names another tenant is refused.
    let (status, _) = backend
        .ledger(
            "tidewater",
            Some(&harbour),
            "definitions",
            json!({ "tenant": "fieldnotes" }),
        )
        .await;
    assert_eq!(status, 403);
    // A session of fieldnotes is nobody on tidewater.
    let (status, _) = backend
        .ledger(
            "tidewater",
            Some(&admin),
            "balance",
            json!({ "owner": person("user:tomasz"), "unit": "points" }),
        )
        .await;
    assert_eq!(status, 401);
}
