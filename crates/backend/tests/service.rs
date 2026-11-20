//! The backend as a running service: real sockets, real bundles, the in-memory store.

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value as Json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strata_backend::app::Config;
use strata_kernel::ext::*;
use strata_kernel::{
    canon, CmpOp, Context, ExistingData, Expr, FieldId, Intent, ObjectTypeId, OrderKey, Principal,
    PrincipalId, QueryDef, QueryId, SchemaChange, Scope, TenantId, Timestamp, Value, ViewDef,
    ViewId, ViewSection,
};
use strata_protocol::api::{
    HEADER_CORRELATION, HEADER_ORIGIN, HEADER_SERVICE, HEADER_SESSION, HEADER_SITE, HEADER_SOURCE,
};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, Store};
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
    start_on(Arc::new(MemoryStore::new()), adjust).await
}

/// Start a backend on a store the test keeps a handle to.
async fn start_on(store: Arc<MemoryStore>, adjust: impl FnOnce(&mut Config)) -> Backend {
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
        throttle: Default::default(),
        passkey_challenge_ttl_ms: 300_000,
        link_ttl_ms: 900_000,
        mail: std::sync::Arc::new(strata_backend::mail::DevSink::memory()),
        federation: std::sync::Arc::new(strata_backend::signin::oidc::Federation::none()),
        factors: Default::default(),
        limits: Default::default(),
    };
    adjust(&mut cfg);
    tokio::spawn(strata_backend::run(
        cfg,
        store,
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

    /// A call that says which address it came from, as the BFF does for a browser.
    async fn call_from(
        &self,
        source: &str,
        site: &str,
        session: Option<&str>,
        path: &str,
        body: Json,
    ) -> (u16, Json) {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header(HEADER_SERVICE, TOKEN)
            .header(HEADER_SITE, site)
            .header(HEADER_SOURCE, source)
            .json(&body);
        if let Some(token) = session {
            request = request.header(HEADER_SESSION, token);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Json::Null))
    }

    /// A call that says which origin the browser made it from, as the BFF does once it has checked
    /// the origin is one of the site's own, and which address it came from.
    async fn call_at(
        &self,
        origin: Option<&str>,
        source: &str,
        site: &str,
        session: Option<&str>,
        path: &str,
        body: Json,
    ) -> (u16, Json) {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header(HEADER_SERVICE, TOKEN)
            .header(HEADER_SITE, site)
            .header(HEADER_SOURCE, source)
            .json(&body);
        if let Some(origin) = origin {
            request = request.header(HEADER_ORIGIN, origin);
        }
        if let Some(token) = session {
            request = request.header(HEADER_SESSION, token);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Json::Null))
    }

    /// A socket opened as a visitor from `source`; the refusal when the upgrade is refused.
    async fn socket_from(
        &self,
        source: &str,
        site: &str,
        session: Option<&str>,
    ) -> Result<Socket, tokio_tungstenite::tungstenite::Error> {
        let mut request = format!("{}/internal/ws", self.base.replace("http", "ws"))
            .into_client_request()
            .unwrap();
        let headers = request.headers_mut();
        headers.insert(HEADER_SERVICE, TOKEN.parse().unwrap());
        headers.insert(HEADER_SITE, site.parse().unwrap());
        headers.insert(HEADER_SOURCE, source.parse().unwrap());
        if let Some(token) = session {
            headers.insert(HEADER_SESSION, token.parse().unwrap());
        }
        tokio_tungstenite::connect_async(request)
            .await
            .map(|(socket, _)| socket)
    }

    /// One password attempt, answered with its status.
    async fn attempt(&self, site: &str, login: &str, password: &str) -> u16 {
        self.call(
            site,
            None,
            "/internal/session/login",
            json!({ "login": login, "password": password }),
        )
        .await
        .0
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

/// The routes of the backend's router and the method each answers, read from its definition so
/// that a route added later is covered without anyone remembering to list it.
fn routes() -> Vec<(String, bool)> {
    [
        include_str!("../src/http.rs"),
        include_str!("../src/dev.rs"),
    ]
    .into_iter()
    .flat_map(str::lines)
    .filter(|line| line.contains(".route(\""))
    .map(|line| {
        let path = line.split('"').nth(1).expect("a route path").to_string();
        (path, line.contains("get("))
    })
    .collect()
}

/// Verifies: RUN-003
#[tokio::test]
async fn requests_without_the_service_token_are_refused() {
    let backend = start().await;
    let wrong = backend
        .http
        .post(format!("{}/internal/view", backend.base))
        .header(HEADER_SERVICE, "not-the-token-not-the-token-not")
        .header(HEADER_SITE, "fieldnotes")
        .json(&json!({ "path": "/" }))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status().as_u16(), 401, "a wrong token");
    let shorter = backend
        .http
        .post(format!("{}/internal/view", backend.base))
        .header(HEADER_SERVICE, &TOKEN[..TOKEN.len() - 1])
        .header(HEADER_SITE, "fieldnotes")
        .json(&json!({ "path": "/" }))
        .send()
        .await
        .unwrap();
    assert_eq!(shorter.status().as_u16(), 401, "a prefix of the token");

    // Every route is behind the gate. The health check is the one deliberate exemption, and it
    // answers without the credential; everything else, whichever method it takes and whatever
    // else the request carries, is refused. Paths that are no route at all are refused as well.
    let routes = routes();
    assert!(
        routes.len() >= 14,
        "the routes were not read from the router: {routes:?}"
    );
    for (path, get) in &routes {
        if path == "/internal/health" {
            continue;
        }
        let url = format!("{}{path}", backend.base);
        let request = if *get {
            backend.http.get(url)
        } else {
            backend.http.post(url).json(&json!({}))
        };
        let response = request
            .header(HEADER_SITE, "fieldnotes")
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status().as_u16(),
            401,
            "{path} answered a request without the service credential"
        );
    }
    for path in ["/", "/internal", "/internal/nothing", "/internal/lab"] {
        let response = backend
            .http
            .post(format!("{}{path}", backend.base))
            .header(HEADER_SITE, "fieldnotes")
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 401, "{path} is no route");
    }
    let health = backend
        .http
        .get(format!("{}/internal/health", backend.base))
        .send()
        .await
        .unwrap();
    assert!(health.status().is_success(), "the health check is open");
}

/// Change the tenant's metadata through the kernel, as its owner does.
fn owner_changes(store: &MemoryStore, now: i64, changes: Vec<SchemaChange>) {
    let context = Context {
        tenant: TenantId::new("fieldnotes").unwrap(),
        principal: Principal::User(PrincipalId::new("user:admin").unwrap()),
        session: None,
        now: Timestamp(now),
        correlation: "t-metadata".into(),
        verified_address: None,
        idempotency_key: None,
        language: None,
    };
    let intent = Intent::ChangeSchema {
        changes,
        expected_version: None,
        existing: ExistingData::Reject,
    };
    execute(store, &intent, &context).unwrap_or_else(|e| panic!("the change was refused: {e}"));
}

/// Verifies: CMS-002
///
/// What a delivery response holds follows from the named queries and view definitions stored in
/// the tenant: they are changed through the kernel while the backend runs, and the responses
/// change with them. A page written into platform code would not.
#[tokio::test]
async fn the_seeded_blog_is_served_from_tenant_metadata() {
    let store = Arc::new(MemoryStore::new());
    let backend = start_on(store.clone(), |_| {}).await;
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
    let status_of = |path: &'static str| {
        backend.call(
            "fieldnotes",
            None,
            "/internal/view",
            json!({ "path": path }),
        )
    };
    assert_eq!(status_of("/no/such/page").await.0, 404);
    assert_eq!(status_of("/oldest").await.0, 404);

    // The published essays, oldest first, worked out from the stored records and not from the
    // views: the two a new query is about to select.
    let post_type = ObjectTypeId::new("post").unwrap();
    let tenant = TenantId::new("fieldnotes").unwrap();
    let state = store.load_complete(&tenant).unwrap().tenant.unwrap();
    let mut published: Vec<(i64, String)> = state
        .records_of(&post_type)
        .filter(|r| r.get(&FieldId::new("status").unwrap()) == &Value::Str("published".into()))
        .map(|r| {
            let at = match r.get(&FieldId::new("published_at").unwrap()) {
                Value::Time(at) => *at,
                other => panic!("a published post without a time: {other:?}"),
            };
            let title = match r.get(&FieldId::new("title").unwrap()) {
                Value::Str(title) => title.clone(),
                other => panic!("a post without a title: {other:?}"),
            };
            (at, title)
        })
        .collect();
    published.sort();
    let oldest: Vec<String> = published.iter().take(2).map(|(_, t)| t.clone()).collect();
    assert_eq!(oldest.len(), 2);

    // A new named query and a new view over it: a page appears at an address nothing knew.
    let is_published = Expr::Cmp(
        CmpOp::Eq,
        Box::new(Expr::Var(Scope::Record, "status".into())),
        Box::new(Expr::Lit(Value::Str("published".into()))),
    );
    let query = QueryDef {
        id: QueryId::new("oldest_two").unwrap(),
        object: post_type.clone(),
        filter: Some(is_published),
        params: Default::default(),
        order: vec![OrderKey {
            field: "published_at".into(),
            descending: false,
        }],
        limit: Some(2),
        aggregates: None,
        declared_fields: None,
    };
    let section = |query: &str| ViewSection {
        query: QueryId::new(query).unwrap(),
        args: Default::default(),
        include: vec![],
        markdown: vec![],
        single: false,
        aggregate: None,
    };
    let oldest_view = ViewDef {
        id: ViewId::new("oldest").unwrap(),
        route: "/oldest".into(),
        title: "Oldest".into(),
        sections: [("posts".to_string(), section("oldest_two"))].into(),
    };
    owner_changes(
        &store,
        2_000_000_000_000,
        vec![
            SchemaChange::DefineQuery(query),
            SchemaChange::DefineView(oldest_view),
        ],
    );
    let page = backend.view("fieldnotes", None, "/oldest").await;
    assert_eq!(page["view"], "oldest");
    assert_eq!(titles(&page, "posts"), oldest);

    // The home page is a view too: pointing its one section at the new query changes what the
    // home page delivers, and the featured section it no longer has is gone.
    let home_view = ViewDef {
        id: ViewId::new("home").unwrap(),
        route: "/".into(),
        title: "Home".into(),
        sections: [("recent".to_string(), section("oldest_two"))].into(),
    };
    owner_changes(
        &store,
        2_000_000_000_001,
        vec![SchemaChange::DefineView(home_view)],
    );
    let home = backend.view("fieldnotes", None, "/").await;
    assert_eq!(titles(&home, "recent"), oldest);
    assert_eq!(
        home["sections"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["recent"]
    );

    // Removing the view removes the page.
    owner_changes(
        &store,
        2_000_000_000_002,
        vec![SchemaChange::RemoveView(ViewId::new("oldest").unwrap())],
    );
    assert_eq!(status_of("/oldest").await.0, 404);
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

    // Another author may not edit it, and cannot tell it is there: a draft she may not read
    // answers exactly as an article that does not exist. The editor may edit it.
    let other = backend
        .login("fieldnotes", "mirela", "strata-dev-mirela")
        .await;
    let edit =
        json!({ "op": "update", "object": "post", "id": id, "fields": { "title": "Edited" } });
    let (status, hidden) = backend
        .call("fieldnotes", Some(&other), "/internal/mutate", edit.clone())
        .await;
    let nowhere =
        json!({ "op": "update", "object": "post", "id": 9_999, "fields": { "title": "Edited" } });
    let (missing_status, missing) = backend
        .call("fieldnotes", Some(&other), "/internal/mutate", nowhere)
        .await;
    assert_eq!((status, missing_status), (404, 404), "{hidden} {missing}");
    let same = |reply: &Json, id: u64| {
        reply
            .to_string()
            .replace(&format!("no post {id}"), "no post N")
    };
    assert_eq!(same(&hidden, id), same(&missing, 9_999));
    let delete = json!({ "op": "delete", "object": "post", "id": id });
    let (status, hidden) = backend
        .call("fieldnotes", Some(&other), "/internal/mutate", delete)
        .await;
    let (_, missing) = backend
        .call(
            "fieldnotes",
            Some(&other),
            "/internal/mutate",
            json!({ "op": "delete", "object": "post", "id": 9_999 }),
        )
        .await;
    assert_eq!(status, 404);
    assert_eq!(same(&hidden, id), same(&missing, 9_999));
    // An article she can read but may not change is refused as such: post 10 is Tomasz's,
    // published.
    let (status, refused) = backend
        .call(
            "fieldnotes",
            Some(&other),
            "/internal/mutate",
            json!({ "op": "update", "object": "post", "id": 10, "fields": { "title": "Mine now" } }),
        )
        .await;
    assert_eq!(status, 403, "{refused}");
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

/// Verifies: IDN-011
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

/// Verifies: RUN-002
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

/// Verifies: RUN-001
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
    // Two more principals of the same tenant: another author, who may read published posts and
    // her own, and an editor, who may read every post.
    let mirela = backend
        .login("fieldnotes", "mirela", "strata-dev-mirela")
        .await;
    let editor_session = backend.login("fieldnotes", "ines", "strata-dev-ines").await;
    let mut peer = backend.socket("fieldnotes", Some(&mirela)).await;
    let mut editor = backend.socket("fieldnotes", Some(&editor_session)).await;
    for socket in [&mut peer, &mut editor] {
        next(socket).await;
        say(
            socket,
            json!({ "type": "subscribe", "req": "r1", "sub": "all" }),
        )
        .await;
        next(socket).await;
    }

    // A draft: the author and the editor hear of it; the public, another author and the other
    // tenant hear nothing.
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
    assert!(
        quiet(&mut peer).await,
        "nor to an author whose policies do not reach it"
    );
    let editor_heard = next(&mut editor).await;
    assert_eq!(editor_heard["notice"]["kind"], "upsert");
    assert_eq!(editor_heard["notice"]["id"], id);
    assert_eq!(
        editor_heard["correlation"], "",
        "the correlation id the author's client chose is not passed to another principal"
    );

    // Publishing it: now the public hears, as its first event.
    let publish =
        json!({ "op": "update", "object": "post", "id": id, "fields": { "status": "published" } });
    backend
        .call("fieldnotes", Some(&author), "/internal/mutate", publish)
        .await;
    let announced = next(&mut public).await;
    assert_eq!(
        next(&mut peer).await["notice"]["kind"],
        "upsert",
        "once it is published, the other author hears of it as well"
    );
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
    assert_eq!(diagnostics["tenants"].as_array().unwrap().len(), 1);
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

    // The session-expiry fixture is the owner's, and ends the owner's own session.
    let (status, _) = backend
        .call(
            "fieldnotes",
            Some(&author),
            "/internal/dev/expire-session",
            json!({}),
        )
        .await;
    assert_eq!(status, 403, "an author may not use the fixtures");
    let (status, expired) = backend
        .call(
            "fieldnotes",
            Some(&admin),
            "/internal/dev/expire-session",
            json!({}),
        )
        .await;
    assert_eq!((status, &expired["expired"]), (200, &json!(true)));
    let (_, still) = backend
        .call("fieldnotes", Some(&author), "/internal/session", json!({}))
        .await;
    assert_eq!(still["session"]["authenticated"], true, "only its own");
    let author = admin;
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

/// Verifies: IDN-008
#[tokio::test]
async fn repeated_wrong_passwords_are_throttled() {
    let backend = start().await;
    let attempt = json!({ "login": "ines", "password": "nope" });
    // D-700: ten failures lock the account; the tenth is still answered as a wrong password.
    for _ in 0..10 {
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

/// Everything the store holds about each tenant, as canonical hashes: the complete state, the
/// events and the journal of intents. Accounts and sessions are left out; they hold salts and
/// random tokens by design.
fn stored_state(store: &MemoryStore) -> Vec<(String, u64, String, String, String)> {
    let mut tenants = store.tenants().unwrap();
    tenants.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
    tenants
        .iter()
        .map(|summary| {
            let state = store.load_complete(&summary.id).unwrap();
            let events = store.events_after(&summary.id, 0, 1_000_000).unwrap();
            let journal = store.journal_after(&summary.id, 0, 1_000_000).unwrap();
            (
                summary.id.as_str().to_string(),
                summary.records,
                canon::hash_of(&state.tenant),
                canon::hash_of(&events),
                canon::hash_of(&journal),
            )
        })
        .collect()
}

/// Verifies: RUN-004
#[tokio::test]
async fn installing_the_same_bundles_into_an_empty_store_gives_the_same_state() {
    let (first, second) = (Arc::new(MemoryStore::new()), Arc::new(MemoryStore::new()));
    let backend = start_on(first.clone(), |_| {}).await;
    let _other = start_on(second.clone(), |_| {}).await;

    // Two independent empty stores, the same bundles: the same state, the same events and the
    // same journal for every tenant. The state is read from the stores, not from rendered pages.
    let installed = stored_state(&first);
    assert!(
        installed.len() >= 2 && installed.iter().all(|(_, records, ..)| *records > 0),
        "the bundles were not installed: {installed:?}"
    );
    assert_eq!(installed, stored_state(&second));

    // Work done afterwards makes the states differ, and a reset installs the bundles again into
    // an empty store: the state is the installed one once more.
    let admin = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    let extra = json!({ "op": "create", "object": "category", "fields": { "name": "Extra", "slug": "extra" } });
    assert_eq!(
        backend
            .call("fieldnotes", Some(&admin), "/internal/mutate", extra)
            .await
            .0,
        200
    );
    assert_ne!(stored_state(&first), installed);
    let (status, reply) = backend
        .call("fieldnotes", Some(&admin), "/internal/dev/reset", json!({}))
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(stored_state(&first), installed, "seeding is deterministic");
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

/// The anonymous request token is no key to the fixtures: a visitor, a member who is not the
/// owner and the owner of another tenant are all refused, and nothing is reset. The owner's reset
/// reinstalls the owner's own tenant and leaves every other tenant exactly as it was.
#[tokio::test]
async fn only_a_signed_in_owner_resets_and_only_their_own_tenant() {
    let store = Arc::new(MemoryStore::new());
    let backend = start_on(store.clone(), |_| {}).await;
    let tidewater = TenantId::new("tidewater").unwrap();
    let fieldnotes = TenantId::new("fieldnotes").unwrap();

    // Work in both tenants, so that a reset of either would show.
    let tide = backend
        .login("tidewater", "admin", "strata-dev-tidewater")
        .await;
    let extra = json!({ "op": "create", "object": "category", "fields": { "name": "Extra", "slug": "extra" } });
    let (status, body) = backend
        .call("tidewater", Some(&tide), "/internal/mutate", extra.clone())
        .await;
    assert_eq!(status, 200, "{body}");
    let admin = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    let (status, body) = backend
        .call("fieldnotes", Some(&admin), "/internal/mutate", extra)
        .await;
    assert_eq!(status, 200, "{body}");
    let hash = |tenant: &TenantId| canon::hash_of(&store.load_complete(tenant).unwrap().tenant);
    let (fieldnotes_before, tidewater_before) = (hash(&fieldnotes), hash(&tidewater));

    let editor = backend.login("fieldnotes", "ines", "strata-dev-ines").await;
    for (who, session, expected) in [
        ("a visitor", None, 401),
        ("an editor", Some(editor.as_str()), 403),
        ("tidewater's owner, on fieldnotes", Some(tide.as_str()), 401),
    ] {
        let (status, body) = backend
            .call("fieldnotes", session, "/internal/dev/reset", json!({}))
            .await;
        assert_eq!(status, expected, "{who}: {body}");
    }
    assert_eq!(hash(&fieldnotes), fieldnotes_before, "nothing was reset");
    assert_eq!(hash(&tidewater), tidewater_before);

    let (status, body) = backend
        .call("fieldnotes", Some(&admin), "/internal/dev/reset", json!({}))
        .await;
    assert_eq!(status, 200, "{body}");
    assert_ne!(
        hash(&fieldnotes),
        fieldnotes_before,
        "fieldnotes is reinstalled"
    );
    assert_eq!(
        hash(&tidewater),
        tidewater_before,
        "tidewater, its extra category included, is untouched"
    );
    let (_, session) = backend
        .call("tidewater", Some(&tide), "/internal/session", json!({}))
        .await;
    assert_eq!(
        session["session"]["authenticated"], true,
        "tidewater's sessions survive"
    );

    // Outside development mode the fixtures are not there at all, not even for the owner.
    let production = start_on(store.clone(), |cfg| cfg.dev = false).await;
    let (status, _) = production
        .call("tidewater", Some(&tide), "/internal/dev/reset", json!({}))
        .await;
    assert_eq!(status, 404);
}

/// Development logins have their passwords in the repository. A start outside development mode
/// on a store a development run used takes them away and ends their sessions.
#[tokio::test]
async fn development_logins_are_withdrawn_when_development_mode_is_off() {
    let store = Arc::new(MemoryStore::new());
    let development = start_on(store.clone(), |_| {}).await;
    let owner = development
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;

    let production = start_on(store.clone(), |cfg| cfg.dev = false).await;
    for (site, login, password) in [
        ("fieldnotes", "admin", "strata-dev-admin"),
        ("fieldnotes", "ines", "strata-dev-ines"),
        ("tidewater", "admin", "strata-dev-tidewater"),
    ] {
        let (status, _) = production
            .call(
                site,
                None,
                "/internal/session/login",
                json!({ "login": login, "password": password }),
            )
            .await;
        assert_eq!(status, 401, "{site}/{login} still opens");
        assert!(store
            .account(&TenantId::new(site).unwrap(), login)
            .unwrap()
            .is_none());
    }
    let (_, session) = production
        .call("fieldnotes", Some(&owner), "/internal/session", json!({}))
        .await;
    assert_eq!(
        session["session"]["authenticated"], false,
        "a session opened with a development login has ended"
    );
}

/// A tenant's diagnostics hold that tenant and nothing of any other: no sessions, principals,
/// events, connections, rejections or runs. The platform report, every tenant and the audit
/// entries, is on its own route and needs nothing but the service credential.
#[tokio::test]
async fn diagnostics_hold_only_the_callers_tenant() {
    let backend = start().await;
    // Fieldnotes activity of every kind the report covers.
    let admin = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    let author = backend
        .login("fieldnotes", "mirela", "strata-dev-mirela")
        .await;
    let mut socket = backend.socket("fieldnotes", Some(&author)).await;
    next(&mut socket).await;
    let draft = json!({ "op": "create", "object": "post", "fields": { "title": "Diagnosed", "slug": "diagnosed", "body": "Text.", "author": 4 } });
    backend
        .call("fieldnotes", Some(&author), "/internal/mutate", draft)
        .await;
    backend
        .call(
            "fieldnotes",
            Some(&author),
            "/internal/mutate",
            json!({ "op": "update", "object": "post", "id": 10, "fields": { "title": "Refused" } }),
        )
        .await;
    let (status, _) = backend
        .call(
            "fieldnotes",
            Some(&admin),
            "/internal/lab/run",
            json!({ "change": "require post.excerpt", "existing": "grandfather", "source": "journal" }),
        )
        .await;
    assert_eq!(status, 200);

    let tide = backend
        .login("tidewater", "admin", "strata-dev-tidewater")
        .await;
    // A fieldnotes token presented on tidewater is nobody there, and is audited.
    backend
        .call("tidewater", Some(&admin), "/internal/session", json!({}))
        .await;
    let (status, report) = backend
        .call("tidewater", Some(&tide), "/internal/diagnostics", json!({}))
        .await;
    assert_eq!(status, 200, "{report}");
    let text = report.to_string();
    for other in [
        "fieldnotes",
        "user:mirela",
        "Diagnosed",
        "require post.excerpt",
    ] {
        assert!(
            !text.contains(other),
            "{other} is in tidewater's report: {text}"
        );
    }
    assert_eq!(report["tenants"].as_array().unwrap().len(), 1);
    assert!(report["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["tenant"] == "tidewater"));

    // The platform report sees everything, the replayed token included.
    let (status, platform) = backend
        .call(
            "tidewater",
            None,
            "/internal/platform/diagnostics",
            json!({}),
        )
        .await;
    assert_eq!(status, 200, "{platform}");
    assert!(platform["tenants"].as_array().unwrap().len() >= 2);
    assert!(platform["audit"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["event"] == "session.wrong_site"
            && entry["session_tenant"] == "fieldnotes"
            && entry["site"] == "tidewater"));
}

/// Who created a record is part of a projected row only for its creator. Principal ids are
/// login identities.
#[tokio::test]
async fn authorship_is_shown_to_the_author_only() {
    let backend = start().await;
    let created_by = |view: &Json| -> Vec<Json> {
        view["sections"]
            .as_object()
            .unwrap()
            .values()
            .flat_map(|section| {
                let rows = section["rows"].as_array().unwrap().iter();
                let included = section["included"]
                    .as_object()
                    .unwrap()
                    .values()
                    .flat_map(|records| records.as_object().unwrap().values());
                rows.chain(included)
                    .map(|record| record["created_by"].clone())
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    for path in ["/", "/authors", "/posts/the-jig-is-the-work"] {
        let view = backend.view("fieldnotes", None, path).await;
        let seen = created_by(&view);
        assert!(!seen.is_empty(), "{path} has rows");
        assert!(
            seen.iter().all(Json::is_null),
            "{path} told a visitor who wrote what: {seen:?}"
        );
    }
    let author = backend
        .login("fieldnotes", "tomasz", "strata-dev-tomasz")
        .await;
    let own = backend
        .view("fieldnotes", Some(&author), "/posts/the-jig-is-the-work")
        .await;
    assert_eq!(
        own["sections"]["post"]["rows"][0]["created_by"],
        "user:tomasz"
    );
    let other = backend
        .view("fieldnotes", Some(&author), "/posts/what-the-kiln-decides")
        .await;
    assert_eq!(
        other["sections"]["post"]["rows"][0]["created_by"],
        Json::Null
    );
}

/// An author record's login identity is read by the author it names, by editors and by owners,
/// and by nobody else, in every view a visitor can open.
#[tokio::test]
async fn an_authors_login_name_is_not_in_any_public_view() {
    let backend = start().await;
    let paths = [
        "/",
        "/about",
        "/topics",
        "/topics/craft",
        "/authors",
        "/authors/ines-okafor",
        "/authors/tomasz-wren",
        "/posts/the-jig-is-the-work",
        "/posts/what-the-kiln-decides",
        "/search",
    ];
    for path in paths {
        let view = backend.view("fieldnotes", None, path).await;
        let text = view.to_string();
        for login in ["user:ines", "user:tomasz", "user:mirela", "user:admin"] {
            assert!(!text.contains(login), "{path} told a visitor {login}");
        }
    }
    // The authors are still there, only without the field.
    let authors = backend.view("fieldnotes", None, "/authors").await;
    let rows = authors["sections"]["authors"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows
        .iter()
        .all(|row| row["fields"].get("principal").is_none()));

    // An author reads her own login identity, and only hers.
    let tomasz = backend
        .login("fieldnotes", "tomasz", "strata-dev-tomasz")
        .await;
    let own = backend
        .view("fieldnotes", Some(&tomasz), "/authors")
        .await
        .to_string();
    assert!(own.contains("user:tomasz"));
    assert!(!own.contains("user:ines") && !own.contains("user:mirela"));
    let desk = backend
        .view("fieldnotes", Some(&tomasz), "/desk/new")
        .await
        .to_string();
    assert!(
        desk.contains("user:tomasz"),
        "the new-article form can preselect her"
    );
    assert!(!desk.contains("user:ines") && !desk.contains("user:mirela"));

    // An editor reads them all.
    let ines = backend.login("fieldnotes", "ines", "strata-dev-ines").await;
    let all = backend
        .view("fieldnotes", Some(&ines), "/authors")
        .await
        .to_string();
    assert!(all.contains("user:tomasz") && all.contains("user:mirela"));
}

// ---------------------------------------------------------------------------------------------
// The outbox publisher, which is the generated machine `arc-outbox-delivery-flow-impl` on the
// runtime's System: a commit offers it `rt-committed` and then the kick (`arc-ob-publish-now`);
// its own timer is the 500 ms sweep; what it emits makes the backend publish the outbox.
// ---------------------------------------------------------------------------------------------

/// A socket subscribed to every event the session may read, past its welcome.
async fn listening(backend: &Backend, site: &str, session: &str) -> (Socket, u64) {
    let mut socket = backend.socket(site, Some(session)).await;
    let welcome = next(&mut socket).await;
    let head = welcome["head"].as_u64().unwrap();
    say(
        &mut socket,
        json!({ "type": "subscribe", "req": "r1", "sub": "all" }),
    )
    .await;
    assert_eq!(next(&mut socket).await["type"], "ok");
    (socket, head)
}

/// Create a category as the session's owner; the status of the reply.
async fn add_category(backend: &Backend, site: &str, session: &str, slug: &str) -> u16 {
    backend
        .call(
            site,
            Some(session),
            "/internal/mutate",
            json!({ "op": "create", "object": "category", "fields": { "name": slug, "slug": slug } }),
        )
        .await
        .0
}

/// The sequence number of the next event frame, or `None` when none comes within `wait`.
async fn event_within(socket: &mut Socket, wait: Duration) -> Option<u64> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = tokio::time::timeout(left, socket.next()).await.ok()?;
        if let Message::Text(text) = frame?.ok()? {
            let message: Json = serde_json::from_str(&text).unwrap();
            if message["type"] == "event" {
                return message["seq"].as_u64();
            }
        }
    }
}

async fn dev_kick(backend: &Backend, kick: bool) {
    let (status, body) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/dev/outbox",
            json!({ "kick": kick }),
        )
        .await;
    assert_eq!(status, 200, "{body}");
}

#[tokio::test]
async fn an_event_is_published_only_after_its_commit() {
    let store = Arc::new(MemoryStore::new());
    let backend = start_on(store.clone(), |_| {}).await;
    let tenant = TenantId::new("fieldnotes").unwrap();
    let admin = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    let (mut socket, head) = listening(&backend, "fieldnotes", &admin).await;
    assert_eq!(
        event_within(&mut socket, Duration::from_millis(700)).await,
        None,
        "nothing is published while nothing is committed"
    );

    // A refused write commits nothing, so nothing is published for it.
    let (status, _) = backend
        .call(
            "fieldnotes",
            Some(&admin),
            "/internal/mutate",
            json!({ "op": "create", "object": "no-such-object", "fields": {} }),
        )
        .await;
    assert_ne!(status, 200);
    assert_eq!(
        event_within(&mut socket, Duration::from_millis(700)).await,
        None,
        "a rejected write publishes nothing"
    );

    // An accepted one is published, and what is published is already in the store's log.
    assert_eq!(
        add_category(&backend, "fieldnotes", &admin, "after-commit").await,
        200
    );
    let seq = event_within(&mut socket, Duration::from_secs(3))
        .await
        .expect("the committed event is published");
    assert_eq!(seq, head + 1);
    let stored = store.events_after(&tenant, head, 100).unwrap();
    assert!(
        stored.iter().any(|event| event.seq == seq),
        "the event was committed before it was published"
    );
}

#[tokio::test]
async fn every_commit_kicks_the_publisher_at_once() {
    let backend = start().await;
    let admin = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    let (mut socket, head) = listening(&backend, "fieldnotes", &admin).await;
    for i in 1..=4u64 {
        let status = add_category(&backend, "fieldnotes", &admin, &format!("kick-{i}")).await;
        assert_eq!(status, 200);
        // The sweep is 500 ms after the commit, so an event inside 350 ms came from the kick.
        let seq = event_within(&mut socket, Duration::from_millis(350)).await;
        assert_eq!(
            seq,
            Some(head + i),
            "commit {i} was published without waiting"
        );
    }
}

#[tokio::test]
async fn the_sweep_publishes_what_a_missed_kick_left() {
    let backend = start().await;
    let admin = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    let (mut socket, head) = listening(&backend, "fieldnotes", &admin).await;
    dev_kick(&backend, false).await;
    let committed = tokio::time::Instant::now();
    assert_eq!(
        add_category(&backend, "fieldnotes", &admin, "no-kick").await,
        200
    );
    assert_eq!(
        event_within(&mut socket, Duration::from_millis(250)).await,
        None,
        "with no kick, the event waits for the sweep"
    );
    let seq = event_within(&mut socket, Duration::from_secs(3))
        .await
        .expect("the sweep publishes it");
    assert_eq!(seq, head + 1);
    assert!(
        committed.elapsed() >= Duration::from_millis(400),
        "the sweep is 500 ms after the commit, not earlier: {:?}",
        committed.elapsed()
    );

    // Kicks are back: the next commit is published at once.
    dev_kick(&backend, true).await;
    assert_eq!(
        add_category(&backend, "fieldnotes", &admin, "kick-again").await,
        200
    );
    assert_eq!(
        event_within(&mut socket, Duration::from_millis(350)).await,
        Some(head + 2)
    );
}

#[tokio::test]
async fn no_event_is_published_twice() {
    let backend = start().await;
    let admin = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    let (mut socket, head) = listening(&backend, "fieldnotes", &admin).await;
    // Commits at the same moment, each kicking, with the sweep timers of the earlier ones still
    // set: the publisher is offered the same events many times over.
    let (backend_ref, admin_ref) = (&backend, &admin);
    let statuses = futures_util::future::join_all((0..8).map(|i| async move {
        add_category(backend_ref, "fieldnotes", admin_ref, &format!("twice-{i}")).await
    }))
    .await;
    assert!(statuses.iter().all(|status| *status == 200), "{statuses:?}");
    let mut heard = Vec::new();
    while heard.len() < 8 {
        heard.push(
            event_within(&mut socket, Duration::from_secs(3))
                .await
                .expect("every commit is published"),
        );
    }
    // Long enough for the sweeps of all of them to have come due.
    while let Some(seq) = event_within(&mut socket, Duration::from_millis(900)).await {
        heard.push(seq);
    }
    let mut sorted = heard.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(heard.len(), 8, "published more than once: {heard:?}");
    assert_eq!(sorted, (head + 1..=head + 8).collect::<Vec<_>>());
}

#[tokio::test]
async fn each_tenants_events_are_published_in_its_commit_order() {
    let backend = start().await;
    let fieldnotes = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    let tidewater = backend
        .login("tidewater", "admin", "strata-dev-tidewater")
        .await;
    let (mut fn_socket, fn_head) = listening(&backend, "fieldnotes", &fieldnotes).await;
    let (mut tw_socket, tw_head) = listening(&backend, "tidewater", &tidewater).await;
    // The tenants commit at the same time, one commit after another within each tenant.
    tokio::join!(
        async {
            for i in 0..6 {
                let status =
                    add_category(&backend, "fieldnotes", &fieldnotes, &format!("order-{i}")).await;
                assert_eq!(status, 200);
            }
        },
        async {
            for i in 0..6 {
                let status =
                    add_category(&backend, "tidewater", &tidewater, &format!("order-{i}")).await;
                assert_eq!(status, 200);
            }
        }
    );
    for (socket, head) in [(&mut fn_socket, fn_head), (&mut tw_socket, tw_head)] {
        let mut heard = Vec::new();
        while let Some(seq) = event_within(socket, Duration::from_millis(900)).await {
            heard.push(seq);
        }
        assert_eq!(
            heard,
            (head + 1..=head + 6).collect::<Vec<_>>(),
            "each tenant hears only its own events, once, in commit order"
        );
    }
}

/// A password and a login that open nothing, for failing on purpose.
const WRONG: &str = "not-the-password";

/// Verifies: IDN-008
///
/// Attempts that arrive together are counted before they are checked. Without the reservation
/// every one of them would pass the check of a count that is still low and be verified.
#[tokio::test]
async fn attempts_that_arrive_together_cannot_all_pass_the_count() {
    // The tenant's share of hashing places is as large as the crowd, so that only the throttle's
    // count decides how many are checked (the share refuses at once and would hide it).
    let backend = Arc::new(start_with(|cfg| cfg.limits.argon2_per_tenant = 40).await);
    let mut calls = Vec::new();
    for _ in 0..40 {
        let backend = backend.clone();
        calls.push(tokio::spawn(async move {
            backend.attempt("fieldnotes", "ines", WRONG).await
        }));
    }
    let mut checked = 0;
    let mut refused = 0;
    for call in calls {
        match call.await.unwrap() {
            401 => checked += 1,
            429 => refused += 1,
            other => panic!("unexpected status {other}"),
        }
    }
    assert_eq!(checked, 10, "no more than the limit is ever checked");
    assert_eq!(refused, 30);
}

/// Verifies: IDN-008
///
/// The counters are in the store, not in the process: a second backend on the same store (a
/// replica, or this one after a restart) refuses what the first had counted.
#[tokio::test]
async fn the_throttle_is_in_the_store_and_shared_by_every_backend_on_it() {
    let store = Arc::new(MemoryStore::new());
    let first = start_on(store.clone(), |_| {}).await;
    for _ in 0..10 {
        assert_eq!(first.attempt("fieldnotes", "ines", WRONG).await, 401);
    }
    assert_eq!(first.attempt("fieldnotes", "ines", WRONG).await, 429);
    let second = start_on(store.clone(), |_| {}).await;
    assert_eq!(
        second
            .attempt("fieldnotes", "ines", "strata-dev-ines")
            .await,
        429,
        "the other backend sees the lock, and the right password does not open it"
    );
    assert_eq!(
        second.attempt("fieldnotes", "mirela", WRONG).await,
        401,
        "another account is not locked"
    );
    assert_eq!(
        second.attempt("tidewater", "ines", WRONG).await,
        401,
        "nor is the same login of another site"
    );
}

/// Verifies: IDN-008
#[tokio::test]
async fn a_right_password_clears_the_count_of_failures() {
    let backend = start().await;
    for _ in 0..9 {
        assert_eq!(backend.attempt("fieldnotes", "mirela", WRONG).await, 401);
    }
    // The tenth attempt is right: it signs in, and what was counted is forgotten.
    assert_eq!(
        backend
            .attempt("fieldnotes", "mirela", "strata-dev-mirela")
            .await,
        200
    );
    for _ in 0..10 {
        assert_eq!(backend.attempt("fieldnotes", "mirela", WRONG).await, 401);
    }
    assert_eq!(backend.attempt("fieldnotes", "mirela", WRONG).await, 429);
}

/// Verifies: IDN-008
///
/// Case, width, ligatures and spaces do not make another account to count against.
#[tokio::test]
async fn spellings_of_one_login_share_one_throttle() {
    let backend = start().await;
    let spellings = [
        "ines",
        " INES ",
        "Ines",
        "\u{FF49}\u{FF4E}\u{FF45}\u{FF53}",
        "\u{00A0}ines",
        "ines\u{3000}",
        "\u{FF29}nes",
        "iNeS",
        "  ines",
        "INES\t",
    ];
    for spelling in spellings {
        assert_eq!(backend.attempt("fieldnotes", spelling, WRONG).await, 401);
    }
    for spelling in ["ines", "\u{FF49}\u{FF4E}\u{FF45}\u{FF53}", " Ines "] {
        assert_eq!(
            backend
                .attempt("fieldnotes", spelling, "strata-dev-ines")
                .await,
            429,
            "{spelling:?} is the locked account"
        );
    }
}

/// Verifies: IDN-009
#[tokio::test]
async fn a_login_of_one_site_is_unknown_to_every_other() {
    let backend = start().await;
    assert_eq!(
        backend
            .attempt("tidewater", "ines", "strata-dev-ines")
            .await,
        401
    );
    assert_eq!(
        backend
            .attempt("fieldnotes", "ines", "strata-dev-ines")
            .await,
        200
    );
}

/// Verifies: IDN-250
#[tokio::test]
async fn a_source_that_fails_too_often_is_refused_across_all_accounts() {
    let backend = start_with(|cfg| cfg.throttle.source_max = 6).await;
    let spray = ["ines", "tomasz", "mirela", "admin", "nobody", "nobody-else"];
    for login in spray {
        let (status, _) = backend
            .call_from(
                "203.0.113.9",
                "fieldnotes",
                None,
                "/internal/session/login",
                json!({ "login": login, "password": WRONG }),
            )
            .await;
        assert_eq!(status, 401, "{login}");
    }
    // An account that was never tried from there, with the right password.
    let right = json!({ "login": "tomasz", "password": "strata-dev-tomasz" });
    let (status, refused) = backend
        .call_from(
            "203.0.113.9",
            "fieldnotes",
            None,
            "/internal/session/login",
            right.clone(),
        )
        .await;
    assert_eq!(status, 429, "{refused}");
    let (status, _) = backend
        .call_from(
            "203.0.113.10",
            "fieldnotes",
            None,
            "/internal/session/login",
            right,
        )
        .await;
    assert_eq!(status, 200, "another source is not affected");

    // The refusal of a source says what the refusal of an account says, and no more.
    for _ in 0..10 {
        backend.attempt("fieldnotes", "mirela", WRONG).await;
    }
    let (_, locked_account) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/session/login",
            json!({ "login": "mirela", "password": WRONG }),
        )
        .await;
    assert_eq!(locked_account["message"], refused["message"]);
    assert_eq!(locked_account["code"], refused["code"]);
}

/// Verifies: IDN-250
///
/// A right password gives its slot back: someone who signs in all day from one address does not
/// use up the allowance of everyone who shares it.
#[tokio::test]
async fn right_passwords_do_not_use_up_the_allowance_of_a_source() {
    let backend = start_with(|cfg| cfg.throttle.source_max = 3).await;
    for _ in 0..8 {
        let (status, _) = backend
            .call_from(
                "203.0.113.20",
                "fieldnotes",
                None,
                "/internal/session/login",
                json!({ "login": "ines", "password": "strata-dev-ines" }),
            )
            .await;
        assert_eq!(status, 200);
    }
}

/// Verifies: IDN-251
#[tokio::test]
async fn a_realm_under_a_spray_is_slowed_whatever_the_source() {
    let backend = start_with(|cfg| {
        cfg.throttle.realm_max = 4;
        cfg.throttle.realm_interval_ms = 1500;
    })
    .await;
    for (i, login) in ["ines", "tomasz", "mirela", "admin"].iter().enumerate() {
        let (status, _) = backend
            .call_from(
                &format!("198.51.100.{i}"),
                "fieldnotes",
                None,
                "/internal/session/login",
                json!({ "login": login, "password": WRONG }),
            )
            .await;
        assert_eq!(status, 401);
    }
    let right = json!({ "login": "ines", "password": "strata-dev-ines" });
    let (status, _) = backend
        .call_from(
            "198.51.100.77",
            "fieldnotes",
            None,
            "/internal/session/login",
            right.clone(),
        )
        .await;
    assert_eq!(
        status, 429,
        "the next attempt, from a source never seen, is slowed"
    );
    // Another realm is not slowed by this one.
    assert_eq!(
        backend.attempt("tidewater", "nobody", WRONG).await,
        401,
        "the other site"
    );
    // The slowing ends when the realm's interval has passed. A refused attempt changes nothing,
    // so the same login is retried until it is no longer refused, for at most five seconds.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let status = loop {
        let (status, _) = backend
            .call_from(
                "198.51.100.77",
                "fieldnotes",
                None,
                "/internal/session/login",
                right.clone(),
            )
            .await;
        if status != 429 || std::time::Instant::now() >= deadline {
            break status;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(status, 200, "slowed, not stopped");
}

/// Verifies: IDN-255
#[tokio::test]
async fn an_account_that_does_not_exist_is_answered_and_limited_like_one_that_does() {
    let backend = start().await;
    let time = |login: &'static str| {
        let backend = &backend;
        async move {
            let started = std::time::Instant::now();
            let (status, body) = backend
                .call(
                    "fieldnotes",
                    None,
                    "/internal/session/login",
                    json!({ "login": login, "password": WRONG }),
                )
                .await;
            (status, body, started.elapsed())
        }
    };
    let (known_status, known_body, first_known) = time("tomasz").await;
    let (unknown_status, unknown_body, first_unknown) = time("nobody-at-all").await;
    assert_eq!(known_status, unknown_status);
    assert_eq!(known_body["code"], unknown_body["code"]);
    assert_eq!(known_body["message"], unknown_body["message"]);
    // What this shows, and what it does not. The structural guarantee is that a miss verifies the
    // password against a decoy hash (`signin/password.rs`); this is a coarse wall-clock guard
    // against that work being dropped. It compares the fastest of four attempts on each side, so
    // a scheduling hiccup on one attempt cannot fail it, and a miss that skipped the hash (far
    // faster than a real verification) cannot pass it. It cannot show constant time: it allows
    // a miss to take half as long as a wrong password.
    let (mut known_fastest, mut unknown_fastest) = (first_known, first_unknown);
    // Four timed attempts, then six more, are the ten that lock an account.
    for _ in 0..3 {
        known_fastest = known_fastest.min(time("tomasz").await.2);
        unknown_fastest = unknown_fastest.min(time("nobody-at-all").await.2);
    }
    assert!(
        unknown_fastest * 2 >= known_fastest,
        "a miss takes as long as a wrong password, fastest of four: {unknown_fastest:?} against {known_fastest:?}"
    );
    // The same limit applies to a login that does not exist.
    for _ in 0..6 {
        backend.attempt("fieldnotes", "nobody-at-all", WRONG).await;
    }
    for _ in 0..6 {
        backend.attempt("fieldnotes", "tomasz", WRONG).await;
    }
    let (known_status, known_body, _) = time("tomasz").await;
    let (unknown_status, unknown_body, _) = time("nobody-at-all").await;
    assert_eq!((known_status, unknown_status), (429, 429));
    assert_eq!(known_body["message"], unknown_body["message"]);
}

/// Verifies: IDN-001
#[tokio::test]
async fn sign_in_goes_through_the_method_named_and_no_other() {
    let backend = start().await;
    let (status, reply) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/session/sign-in/password",
            json!({ "login": "ines", "password": "strata-dev-ines" }),
        )
        .await;
    assert_eq!(status, 200, "{reply}");
    assert!(reply["token"].as_str().is_some());
    let (status, _) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/session/sign-in/telepathy",
            json!({ "login": "ines", "password": "strata-dev-ines" }),
        )
        .await;
    assert_eq!(
        status, 404,
        "a method that is not offered issues no session"
    );
    for malformed in [
        json!({}),
        json!({ "login": "ines" }),
        json!({ "login": 7, "password": "x" }),
        json!({ "login": "ines", "password": "x".repeat(300) }),
        json!({ "login": "i".repeat(100), "password": "x" }),
    ] {
        let (status, _) = backend
            .call(
                "fieldnotes",
                None,
                "/internal/session/sign-in/password",
                malformed,
            )
            .await;
        assert_eq!(status, 401);
    }
}

/// Verifies: IDN-021
/// Verifies: IDN-022
#[tokio::test]
async fn every_sign_in_decision_is_audited_and_no_secret_is_in_the_trail() {
    let store = Arc::new(MemoryStore::new());
    let backend = start_on(store.clone(), |_| {}).await;
    let token = backend.login("fieldnotes", "ines", "strata-dev-ines").await;
    backend
        .attempt("fieldnotes", "ines", "hunter2-the-wrong-one")
        .await;
    for _ in 0..11 {
        backend
            .attempt("fieldnotes", "mirela", "hunter2-the-wrong-one")
            .await;
    }
    // The trail is the tenant's, in the store, not in the backend's memory, and no other tenant
    // has it.
    drop(backend);
    let fieldnotes = TenantId::new("fieldnotes").unwrap();
    let lines = store
        .audit_list(&fieldnotes, Some("decision"), 1000)
        .unwrap();
    let decisions: Vec<Json> = lines
        .iter()
        .map(|line| serde_json::from_str(&line.data).unwrap())
        .collect();
    let outcomes = |outcome: &str| decisions.iter().filter(|d| d["outcome"] == outcome).count();
    assert_eq!(outcomes("accepted"), 1);
    assert_eq!(outcomes("refused"), 11);
    assert_eq!(outcomes("throttled"), 1);
    for (line, decision) in lines.iter().zip(&decisions) {
        assert_eq!(line.event, "auth.decision");
        assert_eq!(decision["method"], "password");
        assert!(
            decision["checks"].as_array().is_some_and(|c| !c.is_empty()),
            "{decision}"
        );
    }
    assert!(store
        .audit_list(&TenantId::new("tidewater").unwrap(), None, 1000)
        .unwrap()
        .is_empty());
    let trail = serde_json::to_string(&lines).unwrap();
    for secret in [
        token.as_str(),
        "hunter2-the-wrong-one",
        "strata-dev-ines",
        "argon2",
    ] {
        assert!(!trail.contains(secret), "the audit trail holds {secret}");
    }
}

/// Verifies: IDN-010
#[tokio::test]
async fn a_session_secret_is_stored_only_as_a_hash() {
    let store = Arc::new(MemoryStore::new());
    let backend = start_on(store.clone(), |_| {}).await;
    let token = backend.login("fieldnotes", "ines", "strata-dev-ines").await;
    let (_, view) = backend
        .call("fieldnotes", Some(&token), "/internal/session", json!({}))
        .await;
    let csrf = view["session"]["csrf_token"].as_str().unwrap().to_string();
    assert!(store.session_by_token(&token).unwrap().is_none());
    let held = store
        .session_by_token(&canon::hash_bytes(token.as_bytes()))
        .unwrap()
        .expect("the session is found by the hash of its token");
    assert_ne!(held.token_hash, token);
    let stored = format!("{held:?}");
    assert!(!stored.contains(&token), "the token is not in the record");
    assert_eq!(held.csrf_token, csrf);
}

/// Verifies: IDN-259
#[tokio::test]
async fn a_session_ends_at_the_shorter_of_its_idle_and_absolute_lifetimes() {
    // Used within its idle time, but past its absolute end.
    let absolute = start_with(|cfg| {
        cfg.session_ttl_ms = 500;
        cfg.session_idle_ms = 3_600_000;
    })
    .await;
    let token = absolute
        .login("fieldnotes", "ines", "strata-dev-ines")
        .await;
    let (_, now) = absolute
        .call("fieldnotes", Some(&token), "/internal/session", json!({}))
        .await;
    assert_eq!(now["session"]["authenticated"], true);
    tokio::time::sleep(Duration::from_millis(700)).await;
    let (_, later) = absolute
        .call("fieldnotes", Some(&token), "/internal/session", json!({}))
        .await;
    assert_eq!(
        (
            later["session"]["authenticated"].as_bool(),
            later["state"].as_str()
        ),
        (Some(false), Some("expired"))
    );

    // Within its absolute time, but unused for longer than its idle time.
    let idle = start_with(|cfg| {
        cfg.session_ttl_ms = 3_600_000;
        cfg.session_idle_ms = 500;
    })
    .await;
    let token = idle.login("fieldnotes", "ines", "strata-dev-ines").await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    let (_, later) = idle
        .call("fieldnotes", Some(&token), "/internal/session", json!({}))
        .await;
    assert_eq!(later["state"], "expired");
}

/// Verifies: IDN-012
#[tokio::test]
async fn a_person_lists_and_revokes_their_own_sessions_and_no_one_elses() {
    let backend = start().await;
    let here = backend.login("fieldnotes", "ines", "strata-dev-ines").await;
    let there = backend.login("fieldnotes", "ines", "strata-dev-ines").await;
    let third = backend.login("fieldnotes", "ines", "strata-dev-ines").await;
    let admin = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;

    let (status, listed) = backend
        .call("fieldnotes", Some(&here), "/internal/sessions", json!({}))
        .await;
    assert_eq!(status, 200, "{listed}");
    let list = listed["sessions"].as_array().unwrap();
    assert_eq!(
        list.len(),
        3,
        "three of her own, and not the administrator's"
    );
    assert_eq!(list.iter().filter(|s| s["current"] == true).count(), 1);
    let text = listed.to_string();
    for secret in [&here, &there, &third] {
        assert!(!text.contains(secret.as_str()), "a token is in the list");
    }
    assert!(!text.contains("token") && !text.contains("csrf"));
    let current = list.iter().find(|s| s["current"] == true).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, own) = backend
        .call("fieldnotes", Some(&here), "/internal/session", json!({}))
        .await;
    assert_eq!(own["session"]["session_id"], current.as_str());

    // Anonymous callers have no sessions to list or revoke.
    assert_eq!(
        backend
            .call("fieldnotes", None, "/internal/sessions", json!({}))
            .await
            .0,
        401
    );
    assert_eq!(
        backend
            .call(
                "fieldnotes",
                None,
                "/internal/sessions/revoke",
                json!({ "id": current })
            )
            .await
            .0,
        401
    );

    // Someone else's session, or one that does not exist, is not found, and is not touched.
    let (_, admin_view) = backend
        .call("fieldnotes", Some(&admin), "/internal/session", json!({}))
        .await;
    let admin_id = admin_view["session"]["session_id"].as_str().unwrap();
    for id in [admin_id, "s_0000000000000000"] {
        let (status, _) = backend
            .call(
                "fieldnotes",
                Some(&here),
                "/internal/sessions/revoke",
                json!({ "id": id }),
            )
            .await;
        assert_eq!(status, 404, "{id}");
    }
    let (_, still) = backend
        .call("fieldnotes", Some(&admin), "/internal/session", json!({}))
        .await;
    assert_eq!(still["session"]["authenticated"], true);

    // Revoke one of her own: it is refused from then on, the others are not.
    let (_, there_view) = backend
        .call("fieldnotes", Some(&there), "/internal/session", json!({}))
        .await;
    let there_id = there_view["session"]["session_id"].as_str().unwrap();
    let (status, _) = backend
        .call(
            "fieldnotes",
            Some(&here),
            "/internal/sessions/revoke",
            json!({ "id": there_id }),
        )
        .await;
    assert_eq!(status, 200);
    let (_, gone) = backend
        .call("fieldnotes", Some(&there), "/internal/session", json!({}))
        .await;
    assert_eq!(gone["session"]["authenticated"], false);
    assert_eq!(gone["state"], "revoked");
    let (_, kept) = backend
        .call("fieldnotes", Some(&third), "/internal/session", json!({}))
        .await;
    assert_eq!(kept["session"]["authenticated"], true);

    // Revoke all the others.
    let (status, reply) = backend
        .call(
            "fieldnotes",
            Some(&here),
            "/internal/sessions/revoke-others",
            json!({}),
        )
        .await;
    assert_eq!((status, &reply["revoked"]), (200, &json!(1)));
    let (_, third_view) = backend
        .call("fieldnotes", Some(&third), "/internal/session", json!({}))
        .await;
    assert_eq!(third_view["state"], "revoked");
    let (_, here_view) = backend
        .call("fieldnotes", Some(&here), "/internal/session", json!({}))
        .await;
    assert_eq!(
        here_view["session"]["authenticated"], true,
        "this one stays"
    );
    let (_, listed) = backend
        .call("fieldnotes", Some(&here), "/internal/sessions", json!({}))
        .await;
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 1);
}

/// Verifies: IDN-012
///
/// A session that is revoked takes its open sockets with it, and a person's other sessions keep
/// theirs.
#[tokio::test]
async fn revoking_a_session_closes_the_sockets_it_has_open() {
    let backend = start().await;
    let here = backend.login("fieldnotes", "ines", "strata-dev-ines").await;
    let there = backend.login("fieldnotes", "ines", "strata-dev-ines").await;
    let mut kept = backend.socket("fieldnotes", Some(&here)).await;
    let mut doomed = backend.socket("fieldnotes", Some(&there)).await;
    assert_eq!(next(&mut kept).await["type"], "welcome");
    assert_eq!(next(&mut doomed).await["type"], "welcome");

    let (_, view) = backend
        .call("fieldnotes", Some(&there), "/internal/session", json!({}))
        .await;
    let id = view["session"]["session_id"].as_str().unwrap();
    let (status, _) = backend
        .call(
            "fieldnotes",
            Some(&here),
            "/internal/sessions/revoke",
            json!({ "id": id }),
        )
        .await;
    assert_eq!(status, 200);
    let frame = next(&mut doomed).await;
    assert_eq!(
        (frame["type"].as_str(), frame["state"].as_str()),
        (Some("session"), Some("revoked"))
    );
    assert_eq!(next(&mut doomed).await["type"], "closed");
    assert!(
        quiet(&mut kept).await,
        "the other session's socket is left alone"
    );
}

/// Verifies: IDN-263
#[tokio::test]
async fn signing_out_revokes_the_session_at_the_backend() {
    let store = Arc::new(MemoryStore::new());
    let backend = start_on(store.clone(), |_| {}).await;
    let token = backend.login("fieldnotes", "ines", "strata-dev-ines").await;
    let id = backend
        .call("fieldnotes", Some(&token), "/internal/session", json!({}))
        .await
        .1["session"]["session_id"]
        .as_str()
        .unwrap()
        .to_string();
    backend
        .call(
            "fieldnotes",
            Some(&token),
            "/internal/session/logout",
            json!({}),
        )
        .await;
    let held = store
        .session(&strata_kernel::SessionId::new(id).unwrap())
        .unwrap()
        .expect("the record is kept, revoked");
    assert!(held.revoked_at.is_some());
}

/// Verifies: IDN-012
///
/// A change of someone's roles ends the sessions they hold, in the commit that changes them, and
/// closes their sockets. The sessions of other people, and of the same person on another site,
/// are not touched.
#[tokio::test]
async fn a_change_of_roles_ends_the_sessions_of_the_person_it_changes() {
    let store = Arc::new(MemoryStore::new());
    let backend = start_on(store.clone(), |_| {}).await;
    let mirela = backend
        .login("fieldnotes", "mirela", "strata-dev-mirela")
        .await;
    let tomasz = backend
        .login("fieldnotes", "tomasz", "strata-dev-tomasz")
        .await;
    let mut socket = backend.socket("fieldnotes", Some(&mirela)).await;
    assert_eq!(next(&mut socket).await["type"], "welcome");

    let context = Context {
        tenant: TenantId::new("fieldnotes").unwrap(),
        principal: Principal::User(PrincipalId::new("user:admin").unwrap()),
        session: None,
        now: Timestamp(strata_protocol::log::now_ms()),
        correlation: "t-promote".into(),
        verified_address: None,
        idempotency_key: None,
        language: None,
    };
    let promote = Intent::AssignRole {
        principal: PrincipalId::new("user:mirela").unwrap(),
        role: strata_kernel::RoleId::new("editor").unwrap(),
    };
    execute(store.as_ref(), &promote, &context)
        .unwrap_or_else(|e| panic!("the owner's change was refused: {e}"));

    let (_, after) = backend
        .call("fieldnotes", Some(&mirela), "/internal/session", json!({}))
        .await;
    assert_eq!(after["session"]["authenticated"], false);
    assert_eq!(after["state"], "revoked");
    let (_, other) = backend
        .call("fieldnotes", Some(&tomasz), "/internal/session", json!({}))
        .await;
    assert_eq!(other["session"]["authenticated"], true);
    // She signs in again and has the new role.
    let again = backend
        .login("fieldnotes", "mirela", "strata-dev-mirela")
        .await;
    let (_, view) = backend
        .call("fieldnotes", Some(&again), "/internal/session", json!({}))
        .await;
    assert!(view["session"]["roles"]
        .as_array()
        .unwrap()
        .iter()
        .any(|role| role == "editor"));
    // Her open socket closes within the sweep of the connection, whichever way it was told.
    loop {
        let frame = next(&mut socket).await;
        if frame["type"] == "closed" {
            break;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Passkeys (WebAuthn). The authenticator is the `webauthn-authenticator-rs` software passkey,
// which makes real attestations and assertions with a real key; the server side is the
// `webauthn-rs` library behind the sign-in method. The software authenticator only signs for
// `https` origins and is handed the credential to use, so the tests give it the credential id
// and add the user handle to the assertion as a discoverable credential would.
// ---------------------------------------------------------------------------------------------

use webauthn_authenticator_rs::softpasskey::SoftPasskey;
use webauthn_authenticator_rs::WebauthnAuthenticator as _;
use webauthn_rs::prelude::{CreationChallengeResponse, RequestChallengeResponse, Url};

/// The address the tenant is reached at, in these tests.
const SITE: &str = "https://fieldnotes.example";
const SIBLING: &str = "https://evil.fieldnotes.example";
const OTHER_HOST: &str = "https://elsewhere.example";
const SOURCE: &str = "203.0.113.7";

/// A passkey the software authenticator holds, as the server knows it.
struct Registered {
    /// The credential id, as the server writes it.
    id: String,
    /// The user handle the server gave the authenticator, as the server writes it.
    handle: Json,
}

impl Backend {
    /// Ask for a registration challenge as `session` at `origin`.
    async fn passkey_register_start(&self, origin: &str, session: Option<&str>) -> (u16, Json) {
        self.call_at(
            Some(origin),
            SOURCE,
            "fieldnotes",
            session,
            "/internal/passkeys/register/start",
            json!({}),
        )
        .await
    }

    /// Add a passkey for the person `session` belongs to: the authenticator makes it at
    /// `made_at`, the server issues and takes it at `origin`.
    async fn passkey_register(
        &self,
        authenticator: &mut SoftPasskey,
        origin: &str,
        made_at: &str,
        session: &str,
    ) -> Result<Registered, (u16, Json)> {
        let (status, started) = self.passkey_register_start(origin, Some(session)).await;
        assert_eq!(status, 200, "{started}");
        let handle = started["options"]["publicKey"]["user"]["id"].clone();
        let options: CreationChallengeResponse =
            serde_json::from_value(started["options"].clone()).unwrap();
        let credential = authenticator
            .do_registration(Url::parse(made_at).unwrap(), options)
            .expect("the authenticator makes a passkey");
        let (status, finished) = self
            .call_at(
                Some(origin),
                SOURCE,
                "fieldnotes",
                Some(session),
                "/internal/passkeys/register/finish",
                json!({
                    "challenge": started["challenge"],
                    "credential": serde_json::to_value(&credential).unwrap(),
                    "name": "Test key",
                }),
            )
            .await;
        if status != 200 {
            return Err((status, finished));
        }
        Ok(Registered {
            id: finished["passkey"]["id"].as_str().unwrap().to_string(),
            handle,
        })
    }

    /// Answer a sign-in challenge. The server issues it at `issued_at`, the authenticator signs
    /// it as a page at `made_at`, and the answer reaches the backend as from `presented_at`.
    async fn passkey_sign_in(
        &self,
        authenticator: &mut SoftPasskey,
        passkey: &Registered,
        issued_at: &str,
        made_at: &str,
        presented_at: &str,
    ) -> (u16, Json) {
        let (assertion, challenge) = self
            .passkey_assertion(authenticator, passkey, issued_at, made_at)
            .await;
        self.passkey_present(presented_at, &challenge, assertion)
            .await
    }

    async fn passkey_assertion(
        &self,
        authenticator: &mut SoftPasskey,
        passkey: &Registered,
        issued_at: &str,
        made_at: &str,
    ) -> (Json, String) {
        let (status, issued) = self
            .call_at(
                Some(issued_at),
                SOURCE,
                "fieldnotes",
                None,
                "/internal/passkeys/challenge",
                json!({}),
            )
            .await;
        assert_eq!(status, 200, "{issued}");
        // A browser lists no credential for a discoverable sign-in and lets the person pick; the
        // software authenticator has to be told which one to use.
        let mut options = issued["options"].clone();
        options["publicKey"]["allowCredentials"] =
            json!([{ "type": "public-key", "id": passkey.id }]);
        let options: RequestChallengeResponse = serde_json::from_value(options).unwrap();
        let assertion = authenticator
            .do_authentication(Url::parse(made_at).unwrap(), options)
            .expect("the authenticator signs");
        let mut assertion = serde_json::to_value(&assertion).unwrap();
        assertion["response"]["userHandle"] = passkey.handle.clone();
        (assertion, issued["challenge"].as_str().unwrap().to_string())
    }

    async fn passkey_present(
        &self,
        presented_at: &str,
        challenge: &str,
        assertion: Json,
    ) -> (u16, Json) {
        self.call_at(
            Some(presented_at),
            SOURCE,
            "fieldnotes",
            None,
            "/internal/session/sign-in/passkey",
            json!({ "challenge": challenge, "assertion": assertion }),
        )
        .await
    }

    async fn passkeys_of(&self, session: &str) -> Vec<Json> {
        let (status, listed) = self
            .call_at(
                Some(SITE),
                SOURCE,
                "fieldnotes",
                Some(session),
                "/internal/passkeys",
                json!({}),
            )
            .await;
        assert_eq!(status, 200, "{listed}");
        listed["passkeys"].as_array().unwrap().clone()
    }

    async fn passkey_remove(&self, session: &str, id: &str) -> u16 {
        self.call_at(
            Some(SITE),
            SOURCE,
            "fieldnotes",
            Some(session),
            "/internal/passkeys/remove",
            json!({ "id": id }),
        )
        .await
        .0
    }
}

/// Set the signature counter the server holds for a credential, as if the authenticator had
/// been used that many times elsewhere.
fn set_stored_counter(store: &MemoryStore, passkey: &Registered, counter: u64) {
    let tenant = TenantId::new("fieldnotes").unwrap();
    let record = store.passkey(&tenant, &passkey.id).unwrap().unwrap();
    let mut credential: Json = serde_json::from_str(&record.credential).unwrap();
    credential["cred"]["counter"] = json!(counter);
    store
        .update_passkey(
            &tenant,
            &passkey.id,
            &credential.to_string(),
            Timestamp(strata_protocol::log::now_ms()),
        )
        .unwrap();
}

/// Verifies: H-017
/// Verifies: IDN-007
/// Verifies: IDN-264
///
/// Case `passkey-valid`: an assertion for the tenant address, signed by a registered credential,
/// is accepted and starts a session of the person who registered it, through the shared flow.
#[tokio::test]
async fn a_registered_passkey_signs_its_owner_in() {
    let backend = start().await;
    let mut authenticator = SoftPasskey::new(true);
    let mirela = backend
        .login("fieldnotes", "mirela", "strata-dev-mirela")
        .await;
    let passkey = backend
        .passkey_register(&mut authenticator, SITE, SITE, &mirela)
        .await
        .expect("a signed-in person adds a passkey");
    let listed = backend.passkeys_of(&mirela).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], passkey.id);
    assert_eq!(listed[0]["name"], "Test key");
    assert!(listed[0]["last_used_at"].is_null());

    let (status, reply) = backend
        .passkey_sign_in(&mut authenticator, &passkey, SITE, SITE, SITE)
        .await;
    assert_eq!(status, 200, "{reply}");
    let token = reply["token"].as_str().unwrap();
    assert_eq!(reply["session"]["authenticated"], true);
    assert_eq!(reply["session"]["display_name"], "Mirela Ashby");
    let (_, session) = backend
        .call("fieldnotes", Some(token), "/internal/session", json!({}))
        .await;
    assert_eq!(session["session"]["authenticated"], true);
    assert_ne!(token, mirela, "a session of its own");
    // The use is noted, and the decision is audited under the method's name.
    assert!(!backend.passkeys_of(&mirela).await[0]["last_used_at"].is_null());
    let (_, platform) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/platform/diagnostics",
            json!({}),
        )
        .await;
    assert!(platform["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| {
            entry["event"] == "auth.decision"
                && entry["method"] == "passkey"
                && entry["outcome"] == "accepted"
        }));
}

/// Verifies: IDN-007
///
/// A registration needs a signed-in person, the challenge of that session and of that address;
/// and the credential belongs to the address of the tenant, so it is of no use at another.
#[tokio::test]
async fn a_passkey_is_added_only_by_a_signed_in_person_at_the_address_it_is_for() {
    let backend = start().await;
    let (status, _) = backend.passkey_register_start(SITE, None).await;
    assert_eq!(status, 401, "no session, no challenge");
    let (status, _) = backend
        .call_at(
            Some(SITE),
            SOURCE,
            "fieldnotes",
            None,
            "/internal/passkeys/register/finish",
            json!({ "challenge": "0".repeat(64), "credential": {} }),
        )
        .await;
    assert_eq!(status, 401, "nor can one be finished without a session");

    let mirela = backend
        .login("fieldnotes", "mirela", "strata-dev-mirela")
        .await;
    let tomasz = backend
        .login("fieldnotes", "tomasz", "strata-dev-tomasz")
        .await;
    // A site reached by an address that is not a name cannot have passkeys.
    for origin in ["http://127.0.0.1:4200", "not an origin"] {
        let (status, _) = backend.passkey_register_start(origin, Some(&mirela)).await;
        assert_eq!(status, 422, "{origin}");
    }
    let (status, _) = backend
        .call_at(
            None,
            SOURCE,
            "fieldnotes",
            Some(&mirela),
            "/internal/passkeys/register/start",
            json!({}),
        )
        .await;
    assert_eq!(status, 422, "a request that did not say where it came from");

    // The authenticator makes the passkey for a page at another address than the challenge was
    // issued for.
    let mut authenticator = SoftPasskey::new(true);
    let refused = backend
        .passkey_register(&mut authenticator, SITE, SIBLING, &mirela)
        .await;
    assert_eq!(refused.err().map(|e| e.0), Some(422));
    assert!(backend.passkeys_of(&mirela).await.is_empty());

    // A challenge is the session's own: another person's session cannot finish it, and the
    // attempt spends it.
    let (_, started) = backend.passkey_register_start(SITE, Some(&mirela)).await;
    let options: CreationChallengeResponse =
        serde_json::from_value(started["options"].clone()).unwrap();
    let credential = authenticator
        .do_registration(Url::parse(SITE).unwrap(), options)
        .unwrap();
    let finish = json!({
        "challenge": started["challenge"],
        "credential": serde_json::to_value(&credential).unwrap(),
    });
    let (status, _) = backend
        .call_at(
            Some(SITE),
            SOURCE,
            "fieldnotes",
            Some(&tomasz),
            "/internal/passkeys/register/finish",
            finish.clone(),
        )
        .await;
    assert_eq!(status, 422);
    assert!(backend.passkeys_of(&tomasz).await.is_empty());
    let (status, _) = backend
        .call_at(
            Some(SITE),
            SOURCE,
            "fieldnotes",
            Some(&mirela),
            "/internal/passkeys/register/finish",
            finish,
        )
        .await;
    assert_eq!(status, 422, "the challenge was spent by the first attempt");
    assert!(backend.passkeys_of(&mirela).await.is_empty());

    // A passkey made for this address does not sign in at another address of the same tenant,
    // even where the other address is answered correctly for itself.
    let passkey = backend
        .passkey_register(&mut authenticator, SITE, SITE, &mirela)
        .await
        .unwrap();
    let (status, _) = backend
        .passkey_sign_in(
            &mut authenticator,
            &passkey,
            OTHER_HOST,
            OTHER_HOST,
            OTHER_HOST,
        )
        .await;
    assert_eq!(status, 401);
}

/// Verifies: IDN-007
/// Verifies: IDN-264
///
/// Case `passkey-other-origin`: an assertion made for another address is refused, whether the
/// page that made it was at a sibling address, or the challenge was issued for one address and
/// answered at another.
#[tokio::test]
async fn an_assertion_for_another_address_is_refused() {
    let backend = start().await;
    let mut authenticator = SoftPasskey::new(true);
    let mirela = backend
        .login("fieldnotes", "mirela", "strata-dev-mirela")
        .await;
    let passkey = backend
        .passkey_register(&mut authenticator, SITE, SITE, &mirela)
        .await
        .unwrap();

    // Signed by a page at a sibling address: the origin in what the browser signed is not the
    // origin the challenge was issued for.
    let (status, reply) = backend
        .passkey_sign_in(&mut authenticator, &passkey, SITE, SIBLING, SITE)
        .await;
    assert_eq!(status, 401, "{reply}");
    assert!(reply["token"].is_null());

    // Issued for one address and presented from another.
    let (status, reply) = backend
        .passkey_sign_in(&mut authenticator, &passkey, SITE, SITE, OTHER_HOST)
        .await;
    assert_eq!(status, 401, "{reply}");
    // Presented with no address at all.
    let (assertion, challenge) = backend
        .passkey_assertion(&mut authenticator, &passkey, SITE, SITE)
        .await;
    let (status, _) = backend
        .call_at(
            None,
            SOURCE,
            "fieldnotes",
            None,
            "/internal/session/sign-in/passkey",
            json!({ "challenge": challenge, "assertion": assertion }),
        )
        .await;
    assert_eq!(status, 401);

    // And the right one still works afterwards: nothing above locked the person out.
    let (status, reply) = backend
        .passkey_sign_in(&mut authenticator, &passkey, SITE, SITE, SITE)
        .await;
    assert_eq!(status, 200, "{reply}");
}

/// Verifies: IDN-264
///
/// A challenge answers once, lapses, and is the tenant's own.
#[tokio::test]
async fn a_challenge_answers_once_and_lapses() {
    let backend = start_with(|cfg| cfg.passkey_challenge_ttl_ms = 400).await;
    let mut authenticator = SoftPasskey::new(true);
    let mirela = backend
        .login("fieldnotes", "mirela", "strata-dev-mirela")
        .await;
    let passkey = backend
        .passkey_register(&mut authenticator, SITE, SITE, &mirela)
        .await
        .unwrap();

    // A right answer, replayed, is a refusal: the challenge was taken by the first.
    let (assertion, challenge) = backend
        .passkey_assertion(&mut authenticator, &passkey, SITE, SITE)
        .await;
    let (status, _) = backend
        .passkey_present(SITE, &challenge, assertion.clone())
        .await;
    assert_eq!(status, 200);
    let (status, _) = backend.passkey_present(SITE, &challenge, assertion).await;
    assert_eq!(status, 401, "a replayed assertion");

    // An answer that comes too late.
    let (assertion, challenge) = backend
        .passkey_assertion(&mut authenticator, &passkey, SITE, SITE)
        .await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let (status, _) = backend.passkey_present(SITE, &challenge, assertion).await;
    assert_eq!(status, 401, "a lapsed challenge");

    // An answer to a challenge no one issued, or one issued to another site.
    let (assertion, _) = backend
        .passkey_assertion(&mut authenticator, &passkey, SITE, SITE)
        .await;
    for challenge in ["0".repeat(64), "not a challenge".to_string()] {
        let (status, _) = backend
            .passkey_present(SITE, &challenge, assertion.clone())
            .await;
        assert_eq!(status, 401);
    }
    let (_, issued) = backend
        .call_at(
            Some(SITE),
            SOURCE,
            "tidewater",
            None,
            "/internal/passkeys/challenge",
            json!({}),
        )
        .await;
    let (status, _) = backend
        .passkey_present(SITE, issued["challenge"].as_str().unwrap(), assertion)
        .await;
    assert_eq!(status, 401, "a challenge is of one tenant");
}

/// Verifies: IDN-264
///
/// Case `passkey-counter-regress`: an assertion whose signature counter did not go forward
/// shows a credential that may have been copied, and is refused and audited.
#[tokio::test]
async fn an_assertion_whose_counter_went_backwards_is_refused() {
    let store = Arc::new(MemoryStore::new());
    let backend = start_on(store.clone(), |_| {}).await;
    let mut authenticator = SoftPasskey::new(true);
    let mirela = backend
        .login("fieldnotes", "mirela", "strata-dev-mirela")
        .await;
    let passkey = backend
        .passkey_register(&mut authenticator, SITE, SITE, &mirela)
        .await
        .unwrap();
    // Counters going forward are accepted.
    for _ in 0..2 {
        let (status, reply) = backend
            .passkey_sign_in(&mut authenticator, &passkey, SITE, SITE, SITE)
            .await;
        assert_eq!(status, 200, "{reply}");
    }
    // The server has seen a counter ahead of what this authenticator now signs: a copy of the
    // key has been used elsewhere.
    set_stored_counter(&store, &passkey, 100);
    let (status, reply) = backend
        .passkey_sign_in(&mut authenticator, &passkey, SITE, SITE, SITE)
        .await;
    assert_eq!(status, 401, "{reply}");
    assert!(reply["token"].is_null());
    // The same counter again is not forward either: the next one the authenticator signs is 4.
    set_stored_counter(&store, &passkey, 4);
    let (status, _) = backend
        .passkey_sign_in(&mut authenticator, &passkey, SITE, SITE, SITE)
        .await;
    assert_eq!(status, 401, "the counter did not advance");
    let (_, platform) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/platform/diagnostics",
            json!({}),
        )
        .await;
    assert!(platform["audit"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["event"] == "passkey.counter_regressed"));
}

/// Verifies: H-031
/// Verifies: H-032
/// Verifies: H-033
///
/// A person lists, and removes, their own passkeys and no one else's. A removed passkey signs no
/// one in.
#[tokio::test]
async fn a_person_manages_only_their_own_passkeys() {
    let backend = start().await;
    let mut ines_key = SoftPasskey::new(true);
    let mut tomasz_key = SoftPasskey::new(true);
    let ines = backend.login("fieldnotes", "ines", "strata-dev-ines").await;
    let tomasz = backend
        .login("fieldnotes", "tomasz", "strata-dev-tomasz")
        .await;
    let ines_passkey = backend
        .passkey_register(&mut ines_key, SITE, SITE, &ines)
        .await
        .unwrap();
    let tomasz_passkey = backend
        .passkey_register(&mut tomasz_key, SITE, SITE, &tomasz)
        .await
        .unwrap();
    assert_eq!(backend.passkeys_of(&ines).await.len(), 1);
    assert_eq!(backend.passkeys_of(&tomasz).await.len(), 1);
    // Listing needs a session.
    let (status, _) = backend
        .call_at(
            Some(SITE),
            SOURCE,
            "fieldnotes",
            None,
            "/internal/passkeys",
            json!({}),
        )
        .await;
    assert_eq!(status, 401);

    // Tomasz cannot remove Ines's passkey, and is told no more than that there is none.
    assert_eq!(backend.passkey_remove(&tomasz, &ines_passkey.id).await, 404);
    assert_eq!(backend.passkeys_of(&ines).await.len(), 1);
    assert_eq!(backend.passkey_remove(&ines, "no-such-passkey").await, 404);

    // Removing one's own works, and then it signs no one in.
    assert_eq!(backend.passkey_remove(&ines, &ines_passkey.id).await, 200);
    assert!(backend.passkeys_of(&ines).await.is_empty());
    let (status, _) = backend
        .passkey_sign_in(&mut ines_key, &ines_passkey, SITE, SITE, SITE)
        .await;
    assert_eq!(status, 401);
    // The other person's is untouched.
    let (status, reply) = backend
        .passkey_sign_in(&mut tomasz_key, &tomasz_passkey, SITE, SITE, SITE)
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["session"]["display_name"], "Tomasz Wren");
}

/// Verifies: IDN-250
///
/// A passkey sign-in goes through the same limits as every method, and asking for a challenge is
/// counted too, so that the store cannot be filled from one source.
#[tokio::test]
async fn passkey_attempts_are_limited_like_every_other_method() {
    let backend = start_with(|cfg| cfg.throttle.source_max = 3).await;
    let mut statuses = Vec::new();
    for _ in 0..5 {
        let (status, _) = backend
            .call_at(
                Some(SITE),
                SOURCE,
                "fieldnotes",
                None,
                "/internal/passkeys/challenge",
                json!({}),
            )
            .await;
        statuses.push(status);
    }
    assert_eq!(statuses, [200, 200, 200, 429, 429]);
    // Another source is not held back by it.
    let (status, _) = backend
        .call_at(
            Some(SITE),
            "203.0.113.8",
            "fieldnotes",
            None,
            "/internal/passkeys/challenge",
            json!({}),
        )
        .await;
    assert_eq!(status, 200);

    // Wrong answers count against the source and end in the same refusal as any method.
    let backend = start_with(|cfg| cfg.throttle.source_max = 3).await;
    let mut statuses = Vec::new();
    for _ in 0..5 {
        let (status, _) = backend
            .call_at(
                Some(SITE),
                SOURCE,
                "fieldnotes",
                None,
                "/internal/session/sign-in/passkey",
                json!({ "challenge": "0".repeat(64), "assertion": {} }),
            )
            .await;
        statuses.push(status);
    }
    assert_eq!(statuses, [401, 401, 401, 429, 429]);
}

// ---------------------------------------------------------------------------------------------
// Emailed sign-in links (IDN-003, IDN-004, IDN-005, IDN-006, IDN-269, FED-250)

/// The login of a development account that is an email address.
const LINK_ADDRESS: &str = "mirela@fieldnotes.example";

/// A backend whose mail goes to a sink the test can read.
async fn linking(
    adjust: impl FnOnce(&mut Config),
) -> (Backend, Arc<strata_backend::mail::DevSink>) {
    let sink = Arc::new(strata_backend::mail::DevSink::memory());
    let kept = sink.clone();
    let backend = start_with(move |cfg| {
        cfg.mail = kept;
        adjust(cfg);
    })
    .await;
    (backend, sink)
}

/// What a sign-in link carries: the challenge and the token, from the fragment of its address, and
/// the whole address.
struct Link {
    challenge: String,
    token: String,
    address: String,
}

fn link_in(sink: &strata_backend::mail::DevSink, index: usize) -> Link {
    use strata_backend::mail::Mailer;
    let mail = sink.outbox().remove(index);
    let address = mail
        .text
        .split_whitespace()
        .find(|word| word.starts_with("https://"))
        .expect("the message holds a link")
        .to_string();
    let (_, fragment) = address
        .split_once('#')
        .expect("the token is in the fragment");
    let part = |name: &str| {
        fragment
            .split('&')
            .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
            .expect("fragment field")
            .to_string()
    };
    Link {
        challenge: part("c"),
        token: part("t"),
        address,
    }
}

/// A well-formed nonce that no browser was given.
fn nonce() -> String {
    canon::hash_bytes(format!("{:?}", std::time::Instant::now()).as_bytes())
}

impl Backend {
    /// Ask for a link from a browser at `SITE`.
    async fn ask_link(
        &self,
        source: &str,
        email: &str,
        binding: Option<&str>,
        next: Option<&str>,
    ) -> (u16, Json) {
        let mut body = json!({ "email": email });
        if let Some(binding) = binding {
            body["binding"] = json!(binding);
        }
        if let Some(next) = next {
            body["next"] = json!(next);
        }
        self.call_at(
            Some(SITE),
            source,
            "fieldnotes",
            None,
            "/internal/sign-in/link/request",
            body,
        )
        .await
    }

    /// Open a link from the browser that holds `binding`, at `origin`.
    async fn open_link(&self, link: &Link, binding: &str, origin: &str) -> (u16, Json) {
        self.call_at(
            Some(origin),
            SOURCE,
            "fieldnotes",
            None,
            "/internal/session/sign-in/link",
            json!({ "challenge": link.challenge, "token": link.token, "binding": binding }),
        )
        .await
    }
}

/// Verifies: IDN-003
///
/// Case link-first-use: a fresh link, opened in the browser that asked for it, signs the person
/// in through the shared flow: a session of their own, and a decision audited under `link`. The
/// token is in the fragment of the address and in no query, and the binding nonce is not in the
/// message at all.
#[tokio::test]
async fn link_first_use_in_the_requesting_browser_signs_in() {
    use strata_backend::mail::Mailer;
    let (backend, sink) = linking(|_| {}).await;
    let (status, reply) = backend.ask_link(SOURCE, LINK_ADDRESS, None, None).await;
    assert_eq!(status, 200, "{reply}");
    let binding = reply["binding"].as_str().unwrap().to_string();
    assert_eq!(binding.len(), 64);

    let sent = sink.outbox();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].to, LINK_ADDRESS);
    assert!(
        !sent[0].text.contains(&binding),
        "the binding is not in the message"
    );
    let link = link_in(&sink, 0);
    assert!(
        link.address.starts_with(&format!("{SITE}/login#c=")),
        "{}",
        link.address
    );
    assert!(!link
        .address
        .split('#')
        .next()
        .unwrap()
        .contains(&link.token));

    let (status, reply) = backend.open_link(&link, &binding, SITE).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["session"]["authenticated"], true);
    assert_eq!(reply["session"]["display_name"], "Mirela Ashby");
    let token = reply["token"].as_str().unwrap();
    let (_, session) = backend
        .call("fieldnotes", Some(token), "/internal/session", json!({}))
        .await;
    assert_eq!(session["session"]["authenticated"], true);

    let (_, platform) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/platform/diagnostics",
            json!({}),
        )
        .await;
    assert!(platform["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| {
            entry["event"] == "auth.decision"
                && entry["method"] == "link"
                && entry["outcome"] == "accepted"
        }));
    let trail = format!("{} {}", platform["audit"], platform["decisions"]);
    for secret in [&link.token, &link.challenge, &binding] {
        assert!(
            !trail.contains(secret.as_str()),
            "the audit trail holds {secret}"
        );
    }
    assert!(
        !trail.contains(LINK_ADDRESS),
        "the audit trail holds the address"
    );
}

/// Verifies: IDN-003
///
/// Case link-second-use: a link that was used signs no one in again.
#[tokio::test]
async fn link_second_use_is_refused() {
    let (backend, sink) = linking(|_| {}).await;
    let (_, reply) = backend.ask_link(SOURCE, LINK_ADDRESS, None, None).await;
    let binding = reply["binding"].as_str().unwrap().to_string();
    let link = link_in(&sink, 0);
    assert_eq!(backend.open_link(&link, &binding, SITE).await.0, 200);
    let (status, reply) = backend.open_link(&link, &binding, SITE).await;
    assert_eq!(status, 401, "{reply}");
    assert!(reply["message"].as_str().unwrap().contains("link"));
}

/// Verifies: IDN-004
///
/// Case link-expired: a link is accepted through its lifetime and not after it, however fresh its
/// token and however right its browser. The lifetime the platform states is fifteen minutes.
#[tokio::test]
async fn link_expired_is_refused() {
    use strata_backend::mail::Mailer;
    let (backend, sink) = linking(|cfg| cfg.link_ttl_ms = 150).await;
    let (_, reply) = backend.ask_link(SOURCE, LINK_ADDRESS, None, None).await;
    let binding = reply["binding"].as_str().unwrap().to_string();
    let link = link_in(&sink, 0);
    tokio::time::sleep(Duration::from_millis(400)).await;
    let (status, _) = backend.open_link(&link, &binding, SITE).await;
    assert_eq!(status, 401);

    let (backend, sink) = linking(|_| {}).await;
    let (_, reply) = backend.ask_link(SOURCE, LINK_ADDRESS, None, None).await;
    let binding = reply["binding"].as_str().unwrap().to_string();
    assert!(sink.outbox()[0].text.contains("15 minutes"));
    assert_eq!(
        backend
            .open_link(&link_in(&sink, 0), &binding, SITE)
            .await
            .0,
        200
    );
}

/// Verifies: IDN-005
///
/// Case link-other-browser: a link opened in a browser that did not ask for it signs no one in,
/// whether that browser has no nonce or another one; and a link that was refused has been taken,
/// so the browser that did ask must ask again, and keeps its binding when it does.
#[tokio::test]
async fn link_other_browser_is_refused() {
    let (backend, sink) = linking(|_| {}).await;
    let (_, reply) = backend.ask_link(SOURCE, LINK_ADDRESS, None, None).await;
    let binding = reply["binding"].as_str().unwrap().to_string();
    let link = link_in(&sink, 0);

    let (status, _) = backend.open_link(&link, &nonce(), SITE).await;
    assert_eq!(status, 401, "another browser's nonce");
    let (status, _) = backend.open_link(&link, &binding, SITE).await;
    assert_eq!(status, 401, "taken by the refusal");

    let (_, again) = backend
        .ask_link(SOURCE, LINK_ADDRESS, Some(&binding), None)
        .await;
    assert_eq!(again["binding"], reply["binding"]);
    assert_eq!(
        backend
            .open_link(&link_in(&sink, 1), &binding, SITE)
            .await
            .0,
        200
    );

    // A browser that sends no nonce at all is not the one that asked.
    let (_, reply) = backend
        .ask_link(SOURCE, LINK_ADDRESS, Some(&binding), None)
        .await;
    assert_eq!(reply["binding"], json!(binding));
    let link = link_in(&sink, 2);
    let (status, _) = backend
        .call_at(
            Some(SITE),
            SOURCE,
            "fieldnotes",
            None,
            "/internal/session/sign-in/link",
            json!({ "challenge": link.challenge, "token": link.token }),
        )
        .await;
    assert_eq!(status, 401);
}

/// Verifies: IDN-005, IDN-003
///
/// A link names the address it was asked for at, and a token that is not its own is refused
/// whatever else is right; either refusal takes the link.
#[tokio::test]
async fn a_link_is_accepted_only_at_its_own_address_with_its_own_token() {
    let (backend, sink) = linking(|_| {}).await;
    let (_, reply) = backend.ask_link(SOURCE, LINK_ADDRESS, None, None).await;
    let binding = reply["binding"].as_str().unwrap().to_string();
    let link = link_in(&sink, 0);
    let (status, _) = backend
        .open_link(&link, &binding, "https://elsewhere.example")
        .await;
    assert_eq!(status, 401);

    backend
        .ask_link(SOURCE, LINK_ADDRESS, Some(&binding), None)
        .await;
    let link = link_in(&sink, 1);
    let wrong = Link {
        token: nonce(),
        challenge: link.challenge.clone(),
        address: String::new(),
    };
    assert_eq!(backend.open_link(&wrong, &binding, SITE).await.0, 401);
    assert_eq!(
        backend.open_link(&link, &binding, SITE).await.0,
        401,
        "the wrong token took it"
    );
}

/// Verifies: IDN-006
///
/// The answer to a request for a link is the same whether or not the address has an account, and
/// so is what is said for something that is not an address; mail goes only to an account.
#[tokio::test]
async fn the_answer_to_a_link_request_does_not_say_whether_there_is_an_account() {
    use strata_backend::mail::Mailer;
    let (backend, sink) = linking(|_| {}).await;
    let binding = nonce();
    let mut answers = Vec::new();
    for address in [
        LINK_ADDRESS,
        "nobody@fieldnotes.example",
        "not an address",
        "",
    ] {
        answers.push(
            backend
                .ask_link(SOURCE, address, Some(&binding), None)
                .await,
        );
    }
    for answer in &answers {
        assert_eq!(answer, &answers[0], "one answer for every address");
    }
    assert_eq!(answers[0].0, 200);
    let sent = sink.outbox();
    assert_eq!(
        sent.len(),
        1,
        "mail went to the account and to nothing else"
    );
    assert_eq!(sent[0].to, LINK_ADDRESS);
}

/// Verifies: IDN-269
///
/// Requests are counted per address and per source, whether or not the address has an account,
/// and they are not given back.
#[tokio::test]
async fn link_requests_are_limited_per_address_and_per_source() {
    use strata_backend::mail::Mailer;
    let (backend, _) = linking(|cfg| cfg.throttle.account_max = 2).await;
    for address in [LINK_ADDRESS, "nobody@fieldnotes.example"] {
        let mut statuses = Vec::new();
        for n in 0..4 {
            let source = format!("203.0.113.{}", 20 + n);
            statuses.push(backend.ask_link(&source, address, None, None).await.0);
        }
        assert_eq!(
            statuses,
            [200, 200, 429, 429],
            "{address}: the same for both"
        );
    }

    let (backend, sink) = linking(|cfg| cfg.throttle.source_max = 3).await;
    let mut statuses = Vec::new();
    for n in 0..5 {
        statuses.push(
            backend
                .ask_link(SOURCE, &format!("person{n}@fieldnotes.example"), None, None)
                .await
                .0,
        );
    }
    assert_eq!(statuses, [200, 200, 200, 429, 429]);
    assert_eq!(
        backend
            .ask_link("203.0.113.99", LINK_ADDRESS, None, None)
            .await
            .0,
        200
    );
    assert_eq!(sink.outbox().len(), 1);
}

/// Verifies: FED-250
///
/// A return address goes into the link only when it is a path on the site.
#[tokio::test]
async fn a_return_address_is_followed_only_when_it_is_a_path_on_the_site() {
    let (backend, sink) = linking(|cfg| cfg.throttle.account_max = 100).await;
    let binding = nonce();
    let nexts = [
        Some("/desk/settings"),
        Some("//evil.example/x"),
        Some("https://evil.example"),
        Some("/\\evil.example"),
        Some("javascript:alert(1)"),
        None,
    ];
    for next in nexts {
        let (status, _) = backend
            .ask_link("203.0.113.40", LINK_ADDRESS, Some(&binding), next)
            .await;
        assert_eq!(status, 200);
    }
    let addresses: Vec<String> = (0..6).map(|i| link_in(&sink, i).address).collect();
    assert!(
        addresses[0].contains("?next=%2Fdesk%2Fsettings#c="),
        "{}",
        addresses[0]
    );
    for dropped in &addresses[1..] {
        assert!(!dropped.contains("next="), "{dropped}");
        assert!(!dropped.contains("evil"), "{dropped}");
    }
}

/// Verifies: IDN-003
///
/// The development outbox is the owner's, in development only: a visitor, or a person who is not
/// the site's owner, is refused, and the owner reads the messages the sink holds.
#[tokio::test]
async fn the_development_outbox_is_for_the_owner() {
    let (backend, _) = linking(|_| {}).await;
    backend.ask_link(SOURCE, LINK_ADDRESS, None, None).await;
    let (status, _) = backend
        .call("fieldnotes", None, "/internal/dev/mail-outbox", json!({}))
        .await;
    assert_eq!(status, 401);
    let mirela = backend
        .login("fieldnotes", "mirela", "strata-dev-mirela")
        .await;
    let (status, _) = backend
        .call(
            "fieldnotes",
            Some(&mirela),
            "/internal/dev/mail-outbox",
            json!({}),
        )
        .await;
    assert_eq!(status, 403);
    let admin = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    let (status, outbox) = backend
        .call(
            "fieldnotes",
            Some(&admin),
            "/internal/dev/mail-outbox",
            json!({}),
        )
        .await;
    assert_eq!(status, 200, "{outbox}");
    assert_eq!(outbox["messages"].as_array().unwrap().len(), 1);
    assert_eq!(outbox["messages"][0]["to"], LINK_ADDRESS);
}

fn refused_as_throttled(result: &Result<Socket, tokio_tungstenite::tungstenite::Error>) -> bool {
    matches!(
        result,
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) if response.status().as_u16() == 429
    )
}

/// One session may hold only so many sockets, and one source address only so many, signed in or
/// not. A closed socket frees its place.
#[tokio::test]
async fn sockets_are_capped_per_session_and_per_source() {
    let backend = start_with(|cfg| {
        cfg.limits.sockets_per_session = 2;
        cfg.limits.sockets_per_source = 3;
    })
    .await;
    let author = backend
        .login("fieldnotes", "tomasz", "strata-dev-tomasz")
        .await;

    // Per session: the third socket of one session is refused, wherever it comes from.
    let first = backend
        .socket_from("10.0.0.1", "fieldnotes", Some(&author))
        .await
        .expect("first");
    let _second = backend
        .socket_from("10.0.0.2", "fieldnotes", Some(&author))
        .await
        .expect("second");
    let third = backend
        .socket_from("10.0.0.3", "fieldnotes", Some(&author))
        .await;
    assert!(refused_as_throttled(&third), "third socket of one session");
    // A closed socket frees its place (the backend notices the close a moment later).
    drop(first);
    let mut reopened = false;
    for _ in 0..40 {
        if backend
            .socket_from("10.0.0.3", "fieldnotes", Some(&author))
            .await
            .is_ok()
        {
            reopened = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(reopened, "the place of a closed socket was never freed");

    // Per source: three anonymous sockets from one address, and the fourth is refused.
    let mut held = Vec::new();
    for _ in 0..3 {
        held.push(
            backend
                .socket_from("10.0.1.1", "fieldnotes", None)
                .await
                .expect("within the cap"),
        );
    }
    let fourth = backend.socket_from("10.0.1.1", "fieldnotes", None).await;
    assert!(refused_as_throttled(&fourth), "fourth socket of one source");
    assert!(
        backend
            .socket_from("10.0.1.2", "fieldnotes", None)
            .await
            .is_ok(),
        "another address is not affected"
    );
}

/// Sign-in attempts are counted per source address, whatever the account, and a source over its
/// limit is refused before any password is hashed.
#[tokio::test]
async fn sign_in_is_rate_limited_per_source() {
    let backend = start_with(|cfg| cfg.limits.logins_per_source_per_minute = 3).await;
    for login in ["ines", "mirela", "tomasz"] {
        let (status, _) = backend
            .call_from(
                "10.0.2.1",
                "fieldnotes",
                None,
                "/internal/session/login",
                json!({ "login": login, "password": "nope" }),
            )
            .await;
        assert_eq!(status, 401, "{login}");
    }
    let right = json!({ "login": "ines", "password": "strata-dev-ines" });
    let (status, _) = backend
        .call_from(
            "10.0.2.1",
            "fieldnotes",
            None,
            "/internal/session/login",
            right.clone(),
        )
        .await;
    assert_eq!(status, 429, "over the limit, even with the right password");
    let (status, _) = backend
        .call_from(
            "10.0.2.2",
            "fieldnotes",
            None,
            "/internal/session/login",
            right,
        )
        .await;
    assert_eq!(status, 200, "another source is not affected");
}

/// Password verifications run a few at a time. With memory for one, a burst of sign-ins is
/// served one after another and those that cannot get a place in time are refused.
#[tokio::test]
async fn concurrent_password_hashing_is_bounded() {
    let backend = start_with(|cfg| {
        cfg.limits.argon2_memory_mib = 19;
        cfg.limits.argon2_wait_ms = 20;
    })
    .await;
    let right = json!({ "login": "ines", "password": "strata-dev-ines" });
    let attempts =
        (0..10).map(|_| backend.call("fieldnotes", None, "/internal/session/login", right.clone()));
    let statuses: Vec<u16> = futures_util::future::join_all(attempts)
        .await
        .into_iter()
        .map(|(status, _)| status)
        .collect();
    assert!(
        statuses.iter().all(|s| matches!(s, 200 | 429)),
        "{statuses:?}"
    );
    assert!(statuses.contains(&200), "someone was served: {statuses:?}");
    assert!(
        statuses.contains(&429),
        "the burst was bounded: {statuses:?}"
    );
}

/// Verification builds the Lean project, so a tenant may ask for it once per interval and never
/// while its last one is running.
#[tokio::test]
async fn verification_is_limited_per_tenant() {
    let backend = start_with(|cfg| {
        // No Lean project here: the run ends at once with a report that says so.
        cfg.lean_dir = PathBuf::from("no-such-lean-project");
    })
    .await;
    let admin = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    let (first, body) = backend
        .call(
            "fieldnotes",
            Some(&admin),
            "/internal/lab/verify",
            json!({}),
        )
        .await;
    assert_eq!(first, 200, "{body}");
    let (second, body) = backend
        .call(
            "fieldnotes",
            Some(&admin),
            "/internal/lab/verify",
            json!({}),
        )
        .await;
    assert_eq!(second, 429, "{body}");
}

/// A tenant may have only so many projections running or waiting. A burst of requests is
/// admitted up to that and the rest are refused.
#[tokio::test]
async fn projections_are_limited_per_tenant() {
    let backend = start_with(|cfg| {
        cfg.limits.lab_runs_per_tenant = 1;
        cfg.limits.lab_workers = 1;
    })
    .await;
    let admin = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    let request = json!({ "change": "require post.excerpt", "existing": "grandfather", "source": "generated", "steps": 300, "seed": 5, "faults": true });
    let attempts = (0..6).map(|_| {
        backend.call(
            "fieldnotes",
            Some(&admin),
            "/internal/lab/run",
            request.clone(),
        )
    });
    let statuses: Vec<u16> = futures_util::future::join_all(attempts)
        .await
        .into_iter()
        .map(|(status, _)| status)
        .collect();
    assert!(
        statuses.iter().all(|s| matches!(s, 200 | 429)),
        "{statuses:?}"
    );
    assert!(statuses.contains(&200), "one was admitted: {statuses:?}");
    assert!(
        statuses.contains(&429),
        "the burst was bounded: {statuses:?}"
    );
}

/// A tenants directory of its own under the build directory, holding the given `tenant.json`
/// files by directory name.
fn tenants_dir_with(name: &str, bundles: &[(&str, String)]) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("tenants-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (directory, text) in bundles {
        std::fs::create_dir_all(dir.join(directory)).unwrap();
        std::fs::write(dir.join(directory).join("tenant.json"), text).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A small tenant bundle: an owner and one object type.
fn small_bundle(id: &str, owner: &str) -> Json {
    json!({
        "id": id,
        "name": format!("Site {id}"),
        "owner": owner,
        "created_at": "2026-09-01T09:00:00Z",
        "objects": [{
            "id": "note",
            "label": "Note",
            "fields": [{ "id": "title", "label": "Title", "type": "string", "required": true }]
        }]
    })
}

/// A bundle that cannot be installed, or cannot even be read, does not stop the backend from
/// starting: it is logged and skipped, and the other tenants are installed and served.
#[tokio::test]
async fn one_bundle_that_will_not_install_does_not_stop_the_others() {
    let mut unreadable = small_bundle("unreadable", "user:o");
    unreadable["surprise"] = json!(true);
    let mut bad_time = small_bundle("badtime", "user:o");
    bad_time["created_at"] = json!("not a time");
    let dir = tenants_dir_with(
        "one-bad",
        &[
            ("a-unreadable", unreadable.to_string()),
            ("b-badtime", bad_time.to_string()),
            ("c-not-json", "{".to_string()),
            ("z-good", small_bundle("goodsite", "user:g").to_string()),
        ],
    );
    let store = Arc::new(MemoryStore::new());
    let backend = start_on(store.clone(), |cfg| cfg.tenants_dir = dir).await;
    let (status, _) = backend
        .call("goodsite", None, "/internal/session", json!({}))
        .await;
    assert_eq!(status, 200, "the backend serves");
    let exists = |id: &str| {
        store
            .load(&TenantId::new(id).unwrap(), &[])
            .unwrap()
            .tenant
            .is_some()
    };
    assert!(exists("goodsite"), "the good tenant was installed");
    assert!(!exists("badtime") && !exists("unreadable"));
}

/// A tenant is created while the process runs, behind the service token, and its owner is told
/// where to sign in. The message carries an invitation link; the owner follows it, chooses a
/// password and signs in with it. The site's host is listed for the BFF.
#[tokio::test]
async fn a_tenant_is_created_while_running_and_its_owner_signs_in_by_link() {
    let dir = tenants_dir_with("runtime", &[]);
    let (backend, sink) = linking(|cfg| cfg.tenants_dir = dir).await;
    let origin = "https://newsite.example";
    let request = json!({
        "bundle": small_bundle("newsite", "user:newowner"),
        "owner_email": "New.Owner@example.org",
        "origin": origin,
    });
    let unauthenticated = backend
        .http
        .post(format!("{}/internal/platform/tenants", backend.base))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(unauthenticated.status().as_u16(), 401);

    let (status, reply) = backend
        .call("-", None, "/internal/platform/tenants", request.clone())
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["notified"], true);
    assert_eq!(reply["owner_login"], "new.owner@example.org");
    let welcome = strata_backend::mail::Mailer::outbox(&*sink);
    assert_eq!(welcome.len(), 1);
    assert_eq!(welcome[0].to, "new.owner@example.org");
    assert!(welcome[0].text.contains("https://newsite.example/login#i="));

    let (status, _) = backend
        .call("-", None, "/internal/platform/tenants", request)
        .await;
    assert_eq!(status, 409, "a tenant that exists is not made again");

    let address = welcome[0]
        .text
        .split_whitespace()
        .find(|word| word.starts_with("https://newsite.example/login#"))
        .expect("the message holds the invitation link")
        .to_string();
    let (_, fragment) = address
        .split_once('#')
        .expect("the secrets are in the fragment");
    let part = |name: &str| {
        fragment
            .split('&')
            .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
            .expect("fragment field")
            .to_string()
    };
    let password = "a-first-password-of-mine";
    let (status, accepted) = backend
        .call_at(
            Some(origin),
            SOURCE,
            "newsite",
            None,
            "/internal/sign-in/invite/accept",
            json!({ "challenge": part("i"), "token": part("t"), "password": password }),
        )
        .await;
    assert_eq!(status, 200, "{accepted}");
    assert_eq!(accepted["login"], "new.owner@example.org");
    let (status, signed_in) = backend
        .call(
            "newsite",
            None,
            "/internal/session/sign-in/password",
            json!({ "login": "new.owner@example.org", "password": password }),
        )
        .await;
    assert_eq!(status, 200, "{signed_in}");
    assert!(signed_in["token"].as_str().is_some());

    let (status, listed) = backend
        .call("-", None, "/internal/platform/hosts", json!({}))
        .await;
    assert_eq!(status, 200, "{listed}");
    assert!(
        listed["hosts"].as_array().unwrap().iter().any(|binding| {
            binding["host"] == "newsite.example" && binding["tenant"] == "newsite"
        }),
        "the new site's host is listed: {listed}"
    );
}
