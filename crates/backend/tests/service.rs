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
    HEADER_CORRELATION, HEADER_IDEMPOTENCY, HEADER_ORIGIN, HEADER_SERVICE, HEADER_SESSION,
    HEADER_SITE, HEADER_SOURCE,
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

    /// A call that carries the idempotency key the browser sent, as the BFF passes it on.
    async fn call_keyed(
        &self,
        site: &str,
        session: Option<&str>,
        path: &str,
        body: Json,
        key: &str,
    ) -> (u16, Json) {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header(HEADER_SERVICE, TOKEN)
            .header(HEADER_SITE, site)
            .header(HEADER_CORRELATION, "t-corr-key")
            .header(HEADER_IDEMPOTENCY, key)
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

    // Every route is behind the gate. The health check is the one route outside the gate, and it
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
        hop: None,
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
        tree: None,
    };
    let section = |query: &str| ViewSection {
        marks: None,
        joins: None,
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

/// Verifies: IDN-011
/// Verifies: IDN-263
#[tokio::test]
async fn login_logout_and_authorized_writes() {
    let store = Arc::new(MemoryStore::new());
    let backend = start_on(store.clone(), |_| {}).await;
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
    let session_id = session["session"]["session_id"]
        .as_str()
        .unwrap()
        .to_string();

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
    // The record of the session is kept, revoked, and holds no token.
    let held = store
        .session(&strata_kernel::SessionId::new(session_id).unwrap())
        .unwrap()
        .expect("the record is kept, revoked");
    assert!(held.revoked_at.is_some());

    // A new login ends the session it replaces.
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
    assert_ne!(first, body["token"].as_str().unwrap());
    let (_, old) = backend
        .call("fieldnotes", Some(&first), "/internal/session", json!({}))
        .await;
    assert_eq!(old["session"]["authenticated"], false);
}

/// Verifies: RUN-002
/// Verifies: IDN-009
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

// ---------------------------------------------------------------------------------------------
// The outbox publisher, which is the generated machine `arc-outbox-delivery-flow-impl` on the
// runtime's System: a commit offers it `rt-committed` and then the kick (`arc-ob-publish-now`);
// its own timer is the 500 ms sweep; what it emits makes the backend publish the outbox.
// ---------------------------------------------------------------------------------------------

/// A password and a login that open nothing, for failing on purpose.
const WRONG: &str = "not-the-password";

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

/// Verifies: IDN-004
///
/// Case link-expired: a link is accepted through its lifetime and not after it, however fresh its
/// token and however right its browser.
#[tokio::test]
async fn link_expired_is_refused() {
    let (backend, sink) = linking(|cfg| cfg.link_ttl_ms = 150).await;
    let (_, reply) = backend.ask_link(SOURCE, LINK_ADDRESS, None, None).await;
    let binding = reply["binding"].as_str().unwrap().to_string();
    let link = link_in(&sink, 0);
    tokio::time::sleep(Duration::from_millis(400)).await;
    let (status, _) = backend.open_link(&link, &binding, SITE).await;
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

/// The ids of the rows of one section of a view answer.
fn row_ids(view: &Json, section: &str) -> Vec<u64> {
    view["sections"][section]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_u64().unwrap())
        .collect()
}

/// Verifies: CMS-164
#[tokio::test]
async fn a_second_page_of_a_view_continues_the_first_with_no_gap_or_repeat() {
    let backend = start().await;
    let (status, whole) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/view",
            json!({ "path": "/", "limit": 100 }),
        )
        .await;
    assert_eq!(status, 200, "{whole}");
    let every = row_ids(&whole, "recent");
    assert!(every.len() > 4, "the fixture has enough essays to page");
    assert!(
        whole["sections"]["recent"].get("next").is_none(),
        "one page that holds the list has no cursor"
    );

    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut request = json!({ "path": "/", "limit": 2 });
        if let Some(cursor) = &cursor {
            request["cursor"] = json!(cursor);
        }
        let (status, page) = backend
            .call("fieldnotes", None, "/internal/view", request)
            .await;
        assert_eq!(status, 200, "{page}");
        let rows = row_ids(&page, "recent");
        assert!(rows.len() <= 2, "a page holds at most the limit");
        seen.extend(rows);
        pages += 1;
        // A single-row section has no cursor, and the other sections answer their first page.
        assert!(page["sections"]["site"].get("next").is_none());
        match page["sections"]["recent"]["next"].as_str() {
            Some(next) => cursor = Some(next.to_string()),
            None => break,
        }
        assert!(pages < 20, "the pages end");
    }
    assert!(pages > 1, "the list took more than one page");
    assert_eq!(seen, every, "the pages hold every row once, in order");

    for (request, why) in [
        (json!({ "path": "/", "limit": 0 }), "a limit of 0"),
        (json!({ "path": "/", "limit": 101 }), "a limit over 100"),
        (
            json!({ "path": "/", "cursor": "recent:zz" }),
            "a cursor that is not one",
        ),
        (
            json!({ "path": "/", "cursor": "site:o1" }),
            "a cursor of a single-row section",
        ),
        (
            json!({ "path": "/", "cursor": "nothing:o1" }),
            "a cursor of no section",
        ),
        (
            json!({ "path": "/", "cursor": "recent:a1" }),
            "a cursor of another kind of window",
        ),
    ] {
        let (status, body) = backend
            .call("fieldnotes", None, "/internal/view", request)
            .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (422, Some("invalid")),
            "{why}: {body}"
        );
    }
}

/// Verifies: CMS-163
#[tokio::test]
async fn a_view_with_a_field_selection_holds_no_unselected_and_no_redacted_field() {
    let backend = start().await;
    let fields_of = |view: &Json, section: &str| -> Vec<Vec<String>> {
        view["sections"][section]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["fields"].as_object().unwrap().keys().cloned().collect())
            .collect()
    };
    // The public reads an author's name and not the login that is theirs: asking for it changes
    // nothing, and the fields that were not asked for are not there.
    let (status, public) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/view",
            json!({ "path": "/authors", "fields": ["name", "principal"] }),
        )
        .await;
    assert_eq!(status, 200, "{public}");
    let rows = fields_of(&public, "authors");
    assert!(rows.len() > 1);
    assert!(rows.iter().all(|keys| keys == &["name"]), "{rows:?}");
    assert_eq!(
        public["sections"]["authors"]["markdown"],
        json!([]),
        "the bio was not selected, so no blocks are made of it"
    );

    // An editor reads it, so for them the same selection holds it, and still nothing else.
    let token = backend.login("fieldnotes", "ines", "strata-dev-ines").await;
    let (status, editor) = backend
        .call(
            "fieldnotes",
            Some(&token),
            "/internal/view",
            json!({ "path": "/authors", "fields": ["name", "principal"] }),
        )
        .await;
    assert_eq!(status, 200, "{editor}");
    let rows = fields_of(&editor, "authors");
    assert!(rows
        .iter()
        .any(|keys| keys.contains(&"principal".to_string())));
    assert!(rows
        .iter()
        .all(|keys| keys.iter().all(|k| k == "name" || k == "principal")));

    // The records a section includes are narrowed the same way.
    let (_, home) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/view",
            json!({ "path": "/", "fields": ["title"] }),
        )
        .await;
    for included in home["sections"]["recent"]["included"]
        .as_object()
        .unwrap()
        .values()
    {
        for record in included.as_object().unwrap().values() {
            assert!(record["fields"]
                .as_object()
                .unwrap()
                .keys()
                .all(|k| k == "title"));
        }
    }
    assert!(fields_of(&home, "recent")
        .iter()
        .all(|keys| keys == &["title"]));

    for (selection, why) in [
        (json!([]), "an empty selection"),
        (json!(["nope"]), "a field no section has"),
        (json!(["Name"]), "a name that is not a field name"),
    ] {
        let (status, body) = backend
            .call(
                "fieldnotes",
                None,
                "/internal/view",
                json!({ "path": "/authors", "fields": selection }),
            )
            .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (422, Some("invalid")),
            "{why}: {body}"
        );
    }
}

/// A view names the people behind its rows by handle: the reply to a signed-in author carries
/// their own display name, marked as theirs, and no principal string anywhere; the rows of other
/// authors name nobody, and a reference field carries its target's title.
#[tokio::test]
async fn a_view_reply_names_people_by_handle_and_titles_its_references() {
    let backend = start().await;
    let token = backend
        .login("fieldnotes", "tomasz", "strata-dev-tomasz")
        .await;
    let draft = json!({
        "op": "create",
        "object": "post",
        "fields": { "title": "People test", "slug": "people-test", "body": "Text.", "author": 3 },
    });
    let (status, body) = backend
        .call("fieldnotes", Some(&token), "/internal/mutate", draft)
        .await;
    assert_eq!(status, 200, "{body}");

    let desk = backend.view("fieldnotes", Some(&token), "/desk").await;
    let people = desk["people"].as_object().expect("a people table");
    let me: Vec<_> = people.iter().filter(|(_, p)| p["reader"] == true).collect();
    assert_eq!(me.len(), 1, "the reader is one of the people: {people:?}");
    assert_eq!(me[0].0, "user:tomasz");
    assert_eq!(me[0].1["name"], "Tomasz Wren");
    assert!(
        people.len() == 1,
        "other authors' rows name nobody: {people:?}"
    );

    // A reference field of a row has its target's title when the reader may read the target.
    let targets = &desk["sections"]["posts"]["ref_targets"];
    let author = &targets["author"];
    assert_eq!(author["object"], "author");
    assert!(
        author["titles"]
            .as_object()
            .is_some_and(|titles| !titles.is_empty() && titles.values().all(|t| t.is_string())),
        "titles of readable authors: {author}"
    );

    // An anonymous reader is given no people.
    let home = backend.view("fieldnotes", None, "/").await;
    assert!(home.get("people").is_none(), "{home}");
}

/// The events of the audit trail of `fieldnotes` that carry `event`, newest data last.
fn audit_events(store: &MemoryStore, event: &str) -> Vec<Json> {
    store
        .audit_list(&TenantId::new("fieldnotes").unwrap(), Some("audit"), 1000)
        .unwrap()
        .iter()
        .filter(|line| line.event == event)
        .map(|line| serde_json::from_str(&line.data).unwrap())
        .collect()
}

/// The titles of the rows of a section, leaving out a row that holds none.
fn any_titles(view: &Json, section: &str) -> Vec<String> {
    view["sections"][section]["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row["fields"]["title"]["Str"].as_str().map(str::to_string))
        .collect()
}

/// Verifies: ADM-1612
#[tokio::test]
async fn a_view_reads_under_the_invoker_and_a_write_to_it_is_refused_and_audited() {
    let store = Arc::new(MemoryStore::new());
    let backend = start_on(store.clone(), |_| {}).await;
    // The same view serves each reader the posts their own policies let them read: the public
    // reads published ones, the editor every one, and nothing the view declares widens that.
    let draft = "Notes Toward a Workbench";
    let editor = backend.login("fieldnotes", "ines", "strata-dev-ines").await;
    let public = backend.view("fieldnotes", None, "/").await;
    assert!(!titles(&public, "recent").contains(&draft.to_string()));
    let (_, desk) = backend
        .call(
            "fieldnotes",
            Some(&editor),
            "/internal/view",
            json!({ "path": "/desk" }),
        )
        .await;
    let seen: Vec<String> = desk["sections"]
        .as_object()
        .unwrap()
        .keys()
        .flat_map(|section| any_titles(&desk, section))
        .collect();
    assert!(
        seen.contains(&draft.to_string()),
        "the editor reads it: {seen:?}"
    );
    let (status, _) = backend
        .call(
            "fieldnotes",
            None,
            "/internal/view",
            json!({ "path": "/desk" }),
        )
        .await;
    if status == 200 {
        let (_, as_public) = backend
            .call(
                "fieldnotes",
                None,
                "/internal/view",
                json!({ "path": "/desk" }),
            )
            .await;
        let seen: Vec<String> = as_public["sections"]
            .as_object()
            .unwrap()
            .keys()
            .flat_map(|section| any_titles(&as_public, section))
            .collect();
        assert!(!seen.contains(&draft.to_string()), "{seen:?}");
    }

    // A write that names a view is refused as such, for the editor and for the owner, and each
    // refusal is in the audit trail.
    let owner = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    for (who, token) in [("editor", &editor), ("owner", &owner)] {
        for write in [
            json!({ "op": "create", "object": "home", "fields": {} }),
            json!({ "op": "update", "object": "home", "id": 1, "fields": {} }),
            json!({ "op": "delete", "object": "home", "id": 1 }),
        ] {
            let (status, body) = backend
                .call("fieldnotes", Some(token), "/internal/mutate", write)
                .await;
            assert_eq!(
                (status, body["kind"].as_str()),
                (403, Some("ViewReadOnly")),
                "{who}: {body}"
            );
        }
    }
    assert_eq!(audit_events(&store, "view.write_refused").len(), 6);
}

/// Verifies: ADM-1632
#[tokio::test]
async fn an_audience_through_a_reference_is_decided_live_and_a_subscription_without_one_is_refused()
{
    use strata_kernel::{Action, PolicyDef, PolicyId, Resource, RoleId};
    let store = Arc::new(MemoryStore::new());
    let backend = start_on(store.clone(), |_| {}).await;
    // A post is read by the author whose byline it carries, even when an editor created it: the
    // audience of the post is stated through the reference it holds to its author.
    owner_changes(
        &store,
        50_000,
        vec![SchemaChange::DefinePolicy(PolicyDef {
            id: PolicyId::new("bylines_read_their_posts").unwrap(),
            role: RoleId::new("author").unwrap(),
            resource: Resource::Object(ObjectTypeId::new("post").unwrap()),
            actions: [Action::Read].into_iter().collect(),
            condition: Some(Expr::Cmp(
                CmpOp::Eq,
                Box::new(Expr::Through(
                    Scope::Record,
                    "author".into(),
                    "principal".into(),
                )),
                Box::new(Expr::Principal),
            )),
        })],
    );
    let authors = backend.view("fieldnotes", None, "/authors").await;
    let mirela_id = authors["sections"]["authors"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["fields"]["name"]["Str"] == "Mirela Ashby")
        .map(|row| row["id"].as_u64().unwrap())
        .expect("Mirela's author record");

    let mirela = backend
        .login("fieldnotes", "mirela", "strata-dev-mirela")
        .await;
    let tomasz = backend
        .login("fieldnotes", "tomasz", "strata-dev-tomasz")
        .await;
    let editor = backend.login("fieldnotes", "ines", "strata-dev-ines").await;
    let mut hers = backend.socket("fieldnotes", Some(&mirela)).await;
    let mut his = backend.socket("fieldnotes", Some(&tomasz)).await;
    for socket in [&mut hers, &mut his] {
        next(socket).await;
        say(
            socket,
            json!({ "type": "subscribe", "req": "r1", "sub": "posts", "objects": ["post"] }),
        )
        .await;
        assert_eq!(next(socket).await["type"], "ok");
    }

    // The editor drafts a post under Mirela's byline: she hears of it, and the other author, who
    // has no byline on it, does not.
    let draft = json!({ "op": "create", "object": "post", "fields": { "title": "Byline", "slug": "byline", "body": "Text.", "author": mirela_id } });
    let (status, created) = backend
        .call("fieldnotes", Some(&editor), "/internal/mutate", draft)
        .await;
    assert_eq!(status, 200, "{created}");
    let heard = next(&mut hers).await;
    assert_eq!(heard["notice"]["kind"], "upsert", "{heard}");
    assert_eq!(heard["notice"]["id"], created["id"]);
    assert!(quiet(&mut his).await, "another author is not told of it");

    // A subscription to what the subscriber has no audience for is refused and audited, and the
    // refusal is the same for a type the tenant does not have.
    let mut public = backend.socket("fieldnotes", None).await;
    next(&mut public).await;
    say(
        &mut public,
        json!({ "type": "subscribe", "req": "r9", "sub": "x", "objects": ["nothing"] }),
    )
    .await;
    let refused = next(&mut public).await;
    assert_eq!(
        (refused["type"].as_str(), refused["code"].as_str()),
        (Some("error"), Some("audience_not_permitted")),
        "{refused}"
    );
    assert_eq!(audit_events(&store, "subscription.refused").len(), 1);
}
