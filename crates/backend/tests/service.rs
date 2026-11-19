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
use strata_protocol::api::{HEADER_CORRELATION, HEADER_SERVICE, HEADER_SESSION, HEADER_SITE};
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
    include_str!("../src/http.rs")
        .lines()
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
    };
    let section = |query: &str| ViewSection {
        query: QueryId::new(query).unwrap(),
        args: Default::default(),
        include: vec![],
        markdown: vec![],
        single: false,
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
