//! The in-app inbox as a running service (NTF-1568 to NTF-1571, CT-182, CT-183, CT-821 to CT-826):
//! a notice arrives once under a retried send, read, unread, removal and the unread count, an
//! inbox is its person's own, a topic of a tenant must begin with `app.`, a muted category is
//! stored and not pushed, a send to a segment reaches each member once through jobs, the
//! automation step makes one notice, and another tenant sees nothing. Real sockets, the seeded
//! `fieldnotes` and `tidewater` tenants and the in-memory store the test keeps a handle to.
//!
//! In `fieldnotes` `admin` owns the tenant, `ines` is an editor, `tomasz` and `mirela` are authors.

use futures_util::StreamExt;
use serde_json::{json, Value as Json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strata_backend::app::{App, Config, Shared};
use strata_backend::inbox::{self, Source, TopicFilter};
use strata_backend::mail::DevSink;
use strata_backend::worker::{self, leases, steps, WorkerConfig};
use strata_kernel::testkit::*;
use strata_kernel::{
    Context, ExistingData, FieldDef, FieldType, Intent, MailTemplateDecl, NoticeTemplateDecl,
    Permission, Principal, PrincipalId, SchemaChange, SettingDef, SettingId, SettingLevel,
    SettingType, StructuredBlocks, TemplateId, TenantId, Timestamp, Value,
};
use strata_protocol::api::{HEADER_CORRELATION, HEADER_SERVICE, HEADER_SESSION, HEADER_SITE};
use strata_storage_api::jobs::{JobState, JobWork, NewJob, Reported, RetryPolicy, RunState};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, Store};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

const TOKEN: &str = "test-service-token-0123456789abcdef";
const SITE: &str = "fieldnotes";
const GOLD: &str = "6a0f3c1e-2280-4000-8000-0000000000a1";

struct Backend {
    base: String,
    http: reqwest::Client,
    store: Arc<MemoryStore>,
}

fn config(addr: std::net::SocketAddr) -> Config {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    Config {
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
        mail: Arc::new(DevSink::memory()),
        federation: Arc::new(strata_backend::signin::oidc::Federation::none()),
        factors: Default::default(),
        limits: Default::default(),
    }
}

async fn start() -> Backend {
    let store = Arc::new(MemoryStore::new());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(strata_backend::run(
        config(addr),
        store.clone(),
        listener,
        std::future::pending(),
    ));
    let backend = Backend {
        base: format!("http://{addr}"),
        http: reqwest::Client::new(),
        store,
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

fn fieldnotes() -> TenantId {
    TenantId::new(SITE).unwrap()
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn frame(socket: &mut Socket) -> Json {
    loop {
        let next = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("a frame in time");
        if let Message::Text(text) = next.expect("socket open").expect("frame") {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

/// Whether nothing arrives on the socket for a while.
async fn silent(socket: &mut Socket) -> bool {
    tokio::time::timeout(Duration::from_millis(500), socket.next())
        .await
        .is_err()
}

impl Backend {
    async fn call_at(
        &self,
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
            .header(HEADER_CORRELATION, "t-inbox-1")
            .json(&body);
        if let Some(token) = session {
            request = request.header(HEADER_SESSION, token);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Json::Null))
    }

    async fn call(&self, session: Option<&str>, path: &str, body: Json) -> (u16, Json) {
        self.call_at(SITE, session, path, body).await
    }

    async fn login_at(&self, site: &str, login: &str, password: &str) -> String {
        let (status, body) = self
            .call_at(
                site,
                None,
                "/internal/session/login",
                json!({ "login": login, "password": password }),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        body["token"].as_str().unwrap().to_string()
    }

    async fn session_of(&self, name: &str) -> String {
        self.login_at(SITE, name, &format!("strata-dev-{name}"))
            .await
    }

    async fn socket(&self, session: &str) -> Socket {
        let mut request = format!("{}/internal/ws", self.base.replace("http", "ws"))
            .into_client_request()
            .unwrap();
        let headers = request.headers_mut();
        headers.insert(HEADER_SERVICE, TOKEN.parse().unwrap());
        headers.insert(HEADER_SITE, SITE.parse().unwrap());
        headers.insert(HEADER_SESSION, session.parse().unwrap());
        let mut socket = tokio_tungstenite::connect_async(request).await.unwrap().0;
        assert_eq!(frame(&mut socket).await["type"], "welcome");
        socket
    }

    fn schema_version(&self) -> u64 {
        self.store
            .load(&fieldnotes(), &[])
            .unwrap()
            .tenant
            .expect("the tenant exists")
            .schema
            .version
            .0
    }

    /// The notice template `welcome` and the categories `news` (transactional) and `security`
    /// (required), as the owner declares them through the dispatcher's route.
    async fn prepare(&self) {
        let admin = self.session_of("admin").await;
        let (status, body) = self
            .call(
                Some(&admin),
                "/internal/inbox/templates",
                json!({
                    "template": "welcome",
                    "title": "Hello {{name}}",
                    "body": "[{\"type\":\"paragraph\",\"inlines\":[{\"type\":\"text\",\"text\":\"Welcome, \"},{\"type\":\"value\",\"name\":\"name\",\"value_type\":\"str\"},{\"type\":\"text\",\"text\":\".\"}]}]",
                    "expected_version": self.schema_version(),
                }),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        // Declared through the dispatcher's route, the one place categories are declared.
        for (category, purpose) in [("news", "transactional"), ("security", "required")] {
            let (status, body) = self
                .call(
                    Some(&admin),
                    "/internal/mail/categories",
                    json!({ "category": category, "purpose": purpose, "name": category }),
                )
                .await;
            assert_eq!(status, 200, "{body}");
        }
    }

    /// Send a notice to a person as the owner.
    async fn send_to(&self, to: &str, key: &str, topic: &str, category: &str) -> (u16, Json) {
        let admin = self.session_of("admin").await;
        self.call(
            Some(&admin),
            "/internal/inbox/send",
            json!({
                "recipient": { "principal": to },
                "topic": topic,
                "category": category,
                "template": "welcome",
                "values": { "name": "Ines" },
                "idempotency_key": key,
            }),
        )
        .await
    }

    async fn list(&self, session: &str, body: Json) -> Json {
        let (status, reply) = self.call(Some(session), "/internal/inbox/list", body).await;
        assert_eq!(status, 200, "{reply}");
        reply
    }

    fn items_of(&self, person: &str) -> usize {
        self.store
            .doc_list(&fieldnotes(), &format!("inbox-item:{person}"))
            .unwrap()
            .len()
    }
}

fn ids(reply: &Json) -> Vec<String> {
    reply["notices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap().to_string())
        .collect()
}

// ---------------------------------------------------------------------------------------------

// Verifies: NTF-1568
#[test]
fn a_topic_follows_the_grammar_and_its_source() {
    // The platform produces its six; a tenant produces app. and nothing else.
    for topic in inbox::PLATFORM_TOPICS {
        assert_eq!(
            inbox::topic_problem(topic, Source::Platform),
            None,
            "{topic}"
        );
        assert!(
            inbox::topic_problem(topic, Source::Tenant).is_some(),
            "{topic}"
        );
    }
    assert!(inbox::topic_problem("app.orders", Source::Platform).is_some());
    assert_eq!(
        inbox::topic_problem("app.orders.shipped", Source::Tenant),
        None
    );
    assert!(inbox::topic_problem("orders.shipped", Source::Tenant).is_some());
    assert!(inbox::topic_problem("application.x", Source::Tenant).is_some());
    // The grammar: one to four labels, each beginning with a lower case letter.
    assert!(
        inbox::grammar_problem("app.a.b.c.d").is_some(),
        "five labels"
    );
    assert_eq!(inbox::grammar_problem("app.a.b.c"), None);
    for bad in [
        "", "app.", ".app", "app..x", "app.1x", "app.Bad", "app.a_b", "app. x",
    ] {
        assert!(inbox::grammar_problem(bad).is_some(), "{bad:?}");
    }
    assert!(inbox::grammar_problem(&format!("app.{}", "a".repeat(93))).is_some());
    assert_eq!(
        inbox::grammar_problem(&format!("app.{}", "a".repeat(92))),
        None
    );
    // A filter is a topic or a prefix that ends in .*
    let prefix = TopicFilter::parse("app.orders.*").unwrap();
    assert!(prefix.matches("app.orders.shipped"));
    assert!(!prefix.matches("app.orders"));
    assert!(!prefix.matches("app.ordersx.y"));
    let exact = TopicFilter::parse("app.orders").unwrap();
    assert!(exact.matches("app.orders") && !exact.matches("app.orders.shipped"));
    assert!(TopicFilter::parse("app.*.x").is_err());
    assert!(TopicFilter::parse("*").is_err());
}

// Verifies: NTF-1570
#[test]
fn a_template_is_rendered_as_text_from_the_values_and_expands_nothing_twice() {
    let values = BTreeMap::from([
        ("name".to_string(), "Ines <b>{{secret}}</b>".to_string()),
        ("secret".to_string(), "hidden".to_string()),
    ]);
    let body = |text: &str| {
        format!(
            "[{{\"type\":\"paragraph\",\"inlines\":[{{\"type\":\"text\",\"text\":\"{text}\"}},{{\"type\":\"value\",\"name\":\"name\",\"value_type\":\"str\"}}]}},{{\"type\":\"paragraph\",\"inlines\":[{{\"type\":\"text\",\"text\":\"Second\"}}]}}]"
        )
    };
    let (title, text) =
        inbox::render("Hello {{name}}", &body("Dear "), &values, &|_| None).expect("rendered");
    assert_eq!(title, "Hello Ines <b>{{secret}}</b>");
    assert_eq!(text, "Dear Ines <b>{{secret}}</b>\n\nSecond");
    // A value the body places and the send does not give refuses the notice.
    assert!(inbox::render("t", &body("a"), &BTreeMap::new(), &|_| None).is_err());
}

// Verifies: NTF-1569
#[tokio::test]
async fn a_notice_arrives_once_under_a_retried_send() {
    let backend = start().await;
    backend.prepare().await;
    let ines = backend.session_of("ines").await;
    let tomasz = backend.session_of("tomasz").await;

    let (status, first) = backend
        .send_to("user:ines", "welcome-1", "app.news", "news")
        .await;
    assert_eq!(status, 200, "{first}");
    assert_eq!(first["delivery"]["created"], true);
    let (status, again) = backend
        .send_to("user:ines", "welcome-1", "app.news", "news")
        .await;
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["delivery"]["created"], false);
    assert_eq!(again["delivery"]["item"], first["delivery"]["item"]);

    let listed = backend.list(&ines, json!({})).await;
    assert_eq!(listed["notices"].as_array().unwrap().len(), 1);
    assert_eq!(listed["unread"], 1);
    let notice = &listed["notices"][0];
    assert_eq!(notice["id"], first["delivery"]["item"]);
    assert_eq!(notice["topic"], "app.news");
    assert_eq!(notice["category"], "news");
    assert_eq!(notice["read"], false);
    let body: Json = serde_json::from_str(notice["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["title"], "Hello Ines");
    assert_eq!(body["text"], "Welcome, Ines.");
    // Nobody else's inbox holds it, and another key is another notice.
    assert!(backend.list(&tomasz, json!({})).await["notices"]
        .as_array()
        .unwrap()
        .is_empty());
    backend
        .send_to("user:ines", "welcome-2", "app.news", "news")
        .await;
    assert_eq!(backend.items_of("user:ines"), 2);

    // A sender needs messages-send: an author does not hold it, and a visitor is not signed in.
    let body = json!({
        "recipient": { "principal": "user:ines" }, "topic": "app.news", "category": "news",
        "template": "welcome", "idempotency_key": "k-author",
    });
    let (status, refused) = backend
        .call(Some(&tomasz), "/internal/inbox/send", body.clone())
        .await;
    assert_eq!(status, 403, "{refused}");
    let (status, _) = backend.call(None, "/internal/inbox/send", body).await;
    assert_eq!(status, 401);
    assert_eq!(backend.items_of("user:ines"), 2);
}

// Verifies: NTF-1569
#[tokio::test]
async fn a_notice_is_read_unread_and_removed_and_the_count_agrees_with_the_list() {
    let backend = start().await;
    backend.prepare().await;
    let ines = backend.session_of("ines").await;
    let mut sent = Vec::new();
    for (key, topic) in [
        ("k1", "app.orders.new"),
        ("k2", "app.orders.paid"),
        ("k3", "app.alerts"),
    ] {
        let (status, reply) = backend.send_to("user:ines", key, topic, "news").await;
        assert_eq!(status, 200, "{reply}");
        sent.push(reply["delivery"]["item"].as_str().unwrap().to_string());
    }
    let count = |topic: Option<&str>| {
        let backend = &backend;
        let ines = ines.clone();
        let body = topic.map_or(json!({}), |t| json!({ "topic": t }));
        async move {
            let (status, reply) = backend
                .call(Some(&ines), "/internal/inbox/count", body)
                .await;
            assert_eq!(status, 200, "{reply}");
            reply["unread"].as_u64().unwrap()
        }
    };
    assert_eq!(count(None).await, 3);
    assert_eq!(count(Some("app.orders.*")).await, 2);
    assert_eq!(count(Some("app.alerts")).await, 1);
    // The count is the number of unread that the list answers for the same topic.
    let listed = backend
        .list(&ines, json!({ "topic": "app.orders.*" }))
        .await;
    assert_eq!(listed["unread"], 2);
    assert_eq!(listed["notices"].as_array().unwrap().len(), 2);

    // Read one; the answer is the count after the change.
    let (status, read) = backend
        .call(
            Some(&ines),
            "/internal/inbox/read",
            json!({ "notices": [sent[0]], "read": true }),
        )
        .await;
    assert_eq!(status, 200, "{read}");
    assert_eq!(read["unread"], 2);
    assert_eq!(count(None).await, 2);
    let read_only = backend.list(&ines, json!({ "state": "read" })).await;
    assert_eq!(ids(&read_only), vec![sent[0].clone()]);
    assert!(read_only["notices"][0]["read_at"].is_number());
    let unread_only = backend.list(&ines, json!({ "state": "unread" })).await;
    assert_eq!(unread_only["notices"].as_array().unwrap().len(), 2);
    assert_eq!(unread_only["unread"], 2);

    // Unread again.
    let (_, unread) = backend
        .call(
            Some(&ines),
            "/internal/inbox/read",
            json!({ "notices": [sent[0]], "read": false }),
        )
        .await;
    assert_eq!(unread["unread"], 3);
    assert!(
        backend.list(&ines, json!({ "state": "read" })).await["notices"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // Paging by cursor visits each notice once.
    let first = backend.list(&ines, json!({ "limit": 2 })).await;
    assert_eq!(first["notices"].as_array().unwrap().len(), 2);
    let cursor = first["next"].as_str().expect("a next page").to_string();
    let second = backend
        .list(&ines, json!({ "limit": 2, "cursor": cursor }))
        .await;
    assert!(second["next"].is_null());
    let mut all: Vec<String> = ids(&first);
    all.extend(ids(&second));
    assert_eq!(
        all.iter().cloned().collect::<BTreeSet<_>>(),
        sent.iter().cloned().collect::<BTreeSet<_>>()
    );
    assert_eq!(all.len(), 3);

    // Removal ends the item: it is not listed or counted, and the same send makes it no more.
    let (status, removed) = backend
        .call(
            Some(&ines),
            "/internal/inbox/remove",
            json!({ "notices": [sent[2]] }),
        )
        .await;
    assert_eq!(status, 200, "{removed}");
    assert_eq!(removed["unread"], 2);
    assert!(!ids(&backend.list(&ines, json!({})).await).contains(&sent[2]));
    assert_eq!(count(None).await, 2);
    let (_, resent) = backend
        .send_to("user:ines", "k3", "app.alerts", "news")
        .await;
    assert_eq!(resent["delivery"]["created"], false);
    assert_eq!(
        backend.list(&ines, json!({})).await["notices"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // A named notice that is not there changes nothing; more than a hundred is refused.
    let (status, missing) = backend
        .call(
            Some(&ines),
            "/internal/inbox/read",
            json!({ "notices": [sent[1], "00000000-0000-4000-8000-000000000000"], "read": true }),
        )
        .await;
    assert_eq!(status, 404, "{missing}");
    assert_eq!(count(None).await, 2, "nothing was changed");
    let many: Vec<String> = (0..101).map(|n| format!("n{n}")).collect();
    let (status, _) = backend
        .call(
            Some(&ines),
            "/internal/inbox/remove",
            json!({ "notices": many }),
        )
        .await;
    assert_eq!(status, 422);
}

// Verifies: NTF-1569
#[tokio::test]
async fn one_person_cannot_read_or_change_the_inbox_of_another() {
    let backend = start().await;
    backend.prepare().await;
    let admin = backend.session_of("admin").await;
    let ines = backend.session_of("ines").await;
    let tomasz = backend.session_of("tomasz").await;
    let (_, sent) = backend
        .send_to("user:ines", "private-1", "app.news", "news")
        .await;
    let id = sent["delivery"]["item"].as_str().unwrap().to_string();

    // An author, and the owner who holds every capability, find nothing of ines's.
    for (who, session) in [("tomasz", &tomasz), ("admin", &admin)] {
        assert!(
            backend.list(session, json!({})).await["notices"]
                .as_array()
                .unwrap()
                .is_empty(),
            "{who}"
        );
        for (path, body) in [
            (
                "/internal/inbox/read",
                json!({ "notices": [id], "read": true }),
            ),
            ("/internal/inbox/remove", json!({ "notices": [id] })),
        ] {
            let (status, refused) = backend.call(Some(session), path, body).await;
            assert_eq!(status, 404, "{who} {path}: {refused}");
        }
        let (_, count) = backend
            .call(Some(session), "/internal/inbox/count", json!({}))
            .await;
        assert_eq!(count["unread"], 0, "{who}");
    }
    // Naming ines in the request gives no more: the inbox is the session's own.
    let listed = backend
        .list(
            &tomasz,
            json!({ "recipient": "user:ines", "principal": "user:ines" }),
        )
        .await;
    assert!(listed["notices"].as_array().unwrap().is_empty());
    // A visitor has no inbox.
    let (status, _) = backend.call(None, "/internal/inbox/list", json!({})).await;
    assert_eq!(status, 401);
    // Nothing of ines's changed.
    let mine = backend.list(&ines, json!({})).await;
    assert_eq!(ids(&mine), vec![id]);
    assert_eq!(mine["unread"], 1);
    // A request that names another tenant is refused.
    let (status, _) = backend
        .call(
            Some(&ines),
            "/internal/inbox/list",
            json!({ "tenant": "tidewater" }),
        )
        .await;
    assert_eq!(status, 403);
}

// Verifies: NTF-1568
#[tokio::test]
async fn a_topic_of_a_tenant_outside_app_is_refused() {
    let backend = start().await;
    backend.prepare().await;
    for topic in [
        "approval.requested",
        "mail.sending-held",
        "orders.shipped",
        "application",
        "app.Bad",
        "app.a.b.c.d",
        "app.",
    ] {
        let (status, refused) = backend.send_to("user:ines", "t-1", topic, "news").await;
        assert_eq!(status, 422, "{topic}: {refused}");
    }
    assert_eq!(backend.items_of("user:ines"), 0);
    let (status, ok) = backend
        .send_to("user:ines", "t-1", "app.orders.shipped", "news")
        .await;
    assert_eq!(status, 200, "{ok}");
    assert_eq!(backend.items_of("user:ines"), 1);
    // A category the tenant does not declare, and a template it does not define, are not found.
    let (status, _) = backend
        .send_to("user:ines", "t-2", "app.news", "unknown")
        .await;
    assert_eq!(status, 404);
    // The list filters by a topic that follows the grammar too.
    let ines = backend.session_of("ines").await;
    let (status, _) = backend
        .call(
            Some(&ines),
            "/internal/inbox/list",
            json!({ "topic": "Not A Topic" }),
        )
        .await;
    assert_eq!(status, 422);
}

// Verifies: NTF-1571, D-6565
#[tokio::test]
async fn a_muted_category_is_stored_and_not_pushed_and_a_required_one_is_pushed() {
    let backend = start().await;
    backend.prepare().await;
    let admin = backend.session_of("admin").await;
    let ines = backend.session_of("ines").await;
    let tomasz = backend.session_of("tomasz").await;
    // The settings of D-6565, defined by the tenant for the test, with a person level.
    let level = BTreeSet::from([SettingLevel::Tenant, SettingLevel::Person]);
    for (id, ty, default) in [
        (inbox::MUTE_UNTIL, "timestamp", Value::Time(0)),
        (inbox::QUIET_HOURS, "string", Value::Str(String::new())),
    ] {
        let def = SettingDef {
            id: SettingId::new(id).unwrap(),
            label: id.to_string(),
            r#type: SettingType::new(ty).unwrap(),
            default,
            levels: level.clone(),
            locked_by: None,
        };
        let (status, body) = backend
            .call(
                Some(&admin),
                "/internal/settings/save-definition",
                json!({
                    "definition": serde_json::to_value(def).unwrap(),
                    "expected_version": backend.schema_version(),
                }),
            )
            .await;
        assert_eq!(status, 200, "{body}");
    }
    let mut socket = backend.socket(&ines).await;
    let mut other = backend.socket(&tomasz).await;
    let mut owner = backend.socket(&admin).await;

    // Not muted: the notice is pushed to its recipient's socket and to no other.
    let (_, sent) = backend
        .send_to("user:ines", "m-1", "app.news", "news")
        .await;
    assert_eq!(sent["delivery"]["push"], "sent");
    let pushed = frame(&mut socket).await;
    assert_eq!(pushed["type"], "inbox", "{pushed}");
    assert_eq!(pushed["kind"], "arrived");
    assert_eq!(pushed["notice"], sent["delivery"]["item"]);
    assert_eq!(pushed["topic"], "app.news");
    assert_eq!(pushed["unread"], 1);
    assert!(
        pushed.get("body").is_none() && pushed.get("title").is_none(),
        "no text on the socket"
    );
    assert!(
        silent(&mut other).await,
        "another person's socket hears nothing"
    );
    assert!(silent(&mut owner).await, "the owner's socket hears nothing");

    // Reading is told to the recipient's own sockets.
    let (_, read) = backend
        .call(
            Some(&ines),
            "/internal/inbox/read",
            json!({ "notices": [sent["delivery"]["item"]], "read": true }),
        )
        .await;
    assert_eq!(read["unread"], 0);
    let told = frame(&mut socket).await;
    assert_eq!(
        (told["kind"].as_str(), told["unread"].as_u64()),
        (Some("read"), Some(0))
    );

    // Muted until tomorrow: stored and not pushed.
    let tomorrow = strata_protocol::log::now_ms() + 86_400_000;
    let (status, set) = backend
        .call(
            Some(&ines),
            "/internal/settings/set-person",
            json!({ "setting": inbox::MUTE_UNTIL, "value": tomorrow, "expected_version": 0 }),
        )
        .await;
    assert_eq!(status, 200, "{set}");
    let mute_revision = set["revision"]
        .as_u64()
        .expect("the revision of the override");
    let (_, muted) = backend
        .send_to("user:ines", "m-2", "app.news", "news")
        .await;
    assert_eq!(muted["delivery"]["push"], "dropped", "{muted}");
    assert_eq!(muted["delivery"]["stored"], true);
    assert!(silent(&mut socket).await, "a muted notice is not pushed");
    let listed = backend.list(&ines, json!({})).await;
    assert!(ids(&listed).contains(&muted["delivery"]["item"].as_str().unwrap().to_string()));
    assert_eq!(listed["unread"], 1);

    // A required category is pushed all the same.
    let (_, required) = backend
        .send_to("user:ines", "m-3", "app.security", "security")
        .await;
    assert_eq!(required["delivery"]["push"], "sent");
    assert_eq!(frame(&mut socket).await["kind"], "arrived");

    // Quiet hours around now defer the push; the inbox stores the notice. (Mute is lifted first.)
    let (status, _) = backend
        .call(
            Some(&ines),
            "/internal/settings/set-person",
            json!({ "setting": inbox::MUTE_UNTIL, "expected_version": mute_revision }),
        )
        .await;
    assert_eq!(status, 200);
    let minute = (strata_protocol::log::now_ms() / 60_000) % 1440;
    let clock = |m: i64| {
        format!(
            "{:02}:{:02}",
            (m.rem_euclid(1440)) / 60,
            m.rem_euclid(1440) % 60
        )
    };
    let (status, set) = backend
        .call(
            Some(&ines),
            "/internal/settings/set-person",
            json!({
                "setting": inbox::QUIET_HOURS,
                "value": format!("{}-{}", clock(minute - 60), clock(minute + 60)),
                "expected_version": 0,
            }),
        )
        .await;
    assert_eq!(status, 200, "{set}");
    let (_, quiet) = backend
        .send_to("user:ines", "m-4", "app.news", "news")
        .await;
    assert_eq!(quiet["delivery"]["push"], "deferred", "{quiet}");
    assert!(silent(&mut socket).await);
    assert_eq!(backend.items_of("user:ines"), 4);
}

// Verifies: NTF-1571
#[tokio::test]
async fn a_category_turned_off_is_not_stored_unless_it_is_required() {
    let backend = start().await;
    backend.prepare().await;
    let ines = backend.session_of("ines").await;
    let set = |category: &str, channel: &str, mode: &str| json!({ "category": category, "channel": channel, "mode": mode });
    // Off for a required category is refused; a category that is not declared is not found.
    let (status, refused) = backend
        .call(
            Some(&ines),
            "/internal/inbox/preferences",
            set("security", "inbox", "off"),
        )
        .await;
    assert_eq!(status, 422, "{refused}");
    let (status, _) = backend
        .call(
            Some(&ines),
            "/internal/inbox/preferences",
            set("nothing", "inbox", "off"),
        )
        .await;
    assert_eq!(status, 404);
    let (status, _) = backend
        .call(
            Some(&ines),
            "/internal/inbox/preferences",
            set("news", "post", "off"),
        )
        .await;
    assert_eq!(status, 422);
    let (status, _) = backend
        .call(
            Some(&ines),
            "/internal/inbox/preferences",
            set("news", "inbox", "sometimes"),
        )
        .await;
    assert_eq!(status, 422);
    // Off for news on the inbox: nothing is stored. The preference is ines's own.
    let (status, _) = backend
        .call(
            Some(&ines),
            "/internal/inbox/preferences",
            set("news", "inbox", "off"),
        )
        .await;
    assert_eq!(status, 200);
    let (_, off) = backend
        .send_to("user:ines", "p-1", "app.news", "news")
        .await;
    assert_eq!(off["delivery"]["stored"], false, "{off}");
    let (_, other) = backend
        .send_to("user:tomasz", "p-1", "app.news", "news")
        .await;
    assert_eq!(other["delivery"]["stored"], true);
    let (_, required) = backend
        .send_to("user:ines", "p-2", "app.security", "security")
        .await;
    assert_eq!(required["delivery"]["stored"], true);
    assert_eq!(backend.items_of("user:ines"), 1);
    // A digest keeps the notice and holds the push.
    backend
        .call(
            Some(&ines),
            "/internal/inbox/preferences",
            set("news", "inbox", "digest"),
        )
        .await;
    let (_, digest) = backend
        .send_to("user:ines", "p-3", "app.news", "news")
        .await;
    assert_eq!(
        (
            digest["delivery"]["stored"].as_bool(),
            digest["delivery"]["push"].as_str()
        ),
        (Some(true), Some("held"))
    );
}

// ---------------------------------------------------------------------------------------------
// A send to a segment
// ---------------------------------------------------------------------------------------------

fn as_admin(now: i64) -> Context {
    Context {
        tenant: fieldnotes(),
        principal: Principal::User(PrincipalId::new("user:admin").unwrap()),
        session: None,
        now: Timestamp(now),
        correlation: format!("t-inbox-{now}"),
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
    }
}

fn field_def(id: &str, required: bool, unique: bool) -> FieldDef {
    FieldDef {
        id: field(id),
        label: id.to_string(),
        ty: FieldType::Str,
        required,
        unique,
        default: None,
        read: None,
        personal: None,
    }
}

// Verifies: NTF-1569
#[tokio::test]
async fn a_segment_send_reaches_each_member_once_in_batches_through_jobs() {
    let backend = start().await;
    backend.prepare().await;
    let admin = backend.session_of("admin").await;
    // 205 members: three people of the tenant and 202 more, so that the send is three batches.
    execute(
        backend.store.as_ref(),
        &Intent::ChangeSchema {
            changes: vec![
                SchemaChange::DefineObject {
                    id: object("member"),
                    label: "Member".into(),
                },
                SchemaChange::DefineField {
                    object: object("member"),
                    field: field_def("principal", true, true),
                },
                SchemaChange::DefineField {
                    object: object("member"),
                    field: field_def("tier", false, false),
                },
            ],
            expected_version: None,
            existing: ExistingData::Reject,
        },
        &as_admin(1_000),
    )
    .expect("the member type is defined");
    let (status, defined) = backend
        .call(
            Some(&admin),
            "/internal/segments/define",
            json!({
                "segment": GOLD, "name": "Gold", "subject_type": "member",
                "predicate": "record.tier == 'gold'", "principal_field": "principal",
            }),
        )
        .await;
    assert_eq!(status, 200, "{defined}");
    let mut people: Vec<String> = ["ines", "tomasz", "mirela"]
        .iter()
        .map(|n| format!("user:{n}"))
        .collect();
    people.extend((0..202).map(|n| format!("user:m{n:03}")));
    for (n, person) in people.iter().enumerate() {
        execute(
            backend.store.as_ref(),
            &Intent::CreateRecord {
                object: object("member"),
                fields: BTreeMap::from([
                    (field("principal"), Value::Str(person.clone())),
                    (field("tier"), Value::Str("gold".into())),
                ]),
            },
            &as_admin(2_000 + n as i64),
        )
        .expect("the member is created");
    }
    let send = json!({
        "recipient": { "segment": GOLD }, "topic": "app.news", "category": "news",
        "template": "welcome", "values": { "name": "Gold" }, "idempotency_key": "seg-1",
    });
    let (status, accepted) = backend
        .call(Some(&admin), "/internal/inbox/send", send.clone())
        .await;
    assert_eq!(status, 200, "{accepted}");
    assert_eq!(accepted["recipients"], 205);
    assert_eq!(accepted["progress"]["batches"], 3);
    // A segment the caller may not read is refused, and so is a send without the capability.
    let tomasz = backend.session_of("tomasz").await;
    let (status, _) = backend
        .call(Some(&tomasz), "/internal/inbox/send", send.clone())
        .await;
    assert_eq!(status, 403);

    // The worker of the running backend takes the three jobs.
    let mut settled = false;
    for _ in 0..50 {
        let jobs = backend.store.jobs_list(&fieldnotes(), None, 50).unwrap();
        if jobs.len() == 3 && jobs.iter().all(|j| j.state == JobState::Completed) {
            settled = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(settled, "the three batches completed");
    for person in &people {
        assert_eq!(backend.items_of(person), 1, "{person} holds one notice");
    }
    // The same send again, and a retry of it, make nothing more.
    let (status, again) = backend
        .call(Some(&admin), "/internal/inbox/send", send)
        .await;
    assert_eq!(status, 200, "{again}");
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(
        backend
            .store
            .jobs_list(&fieldnotes(), None, 50)
            .unwrap()
            .len(),
        3
    );
    for person in &people {
        assert_eq!(
            backend.items_of(person),
            1,
            "{person} still holds one notice"
        );
    }
    // Each member reads it in their own inbox.
    let ines = backend.session_of("ines").await;
    assert_eq!(
        backend.list(&ines, json!({})).await["notices"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

// ---------------------------------------------------------------------------------------------
// Another tenant
// ---------------------------------------------------------------------------------------------

// Verifies: TEN-1140
#[tokio::test]
async fn another_tenant_sees_nothing() {
    let backend = start().await;
    backend.prepare().await;
    backend
        .send_to("user:ines", "x-1", "app.news", "news")
        .await;
    backend
        .send_to("user:harbour", "x-1", "app.news", "news")
        .await;
    assert_eq!(backend.items_of("user:ines"), 1);
    let harbour = backend
        .login_at("tidewater", "admin", "strata-dev-tidewater")
        .await;
    let (status, listed) = backend
        .call_at(
            "tidewater",
            Some(&harbour),
            "/internal/inbox/list",
            json!({}),
        )
        .await;
    assert_eq!(status, 200, "{listed}");
    assert!(listed["notices"].as_array().unwrap().is_empty());
    assert_eq!(listed["unread"], 0);
    let tidewater = TenantId::new("tidewater").unwrap();
    for person in ["user:ines", "user:harbour"] {
        assert!(backend
            .store
            .doc_list(&tidewater, &format!("inbox-item:{person}"))
            .unwrap()
            .is_empty());
    }
    // The notice a harbour master gets in fieldnotes is in fieldnotes only, and tidewater's owner
    // cannot send with fieldnotes's template: there is none in tidewater.
    let (status, refused) = backend
        .call_at(
            "tidewater",
            Some(&harbour),
            "/internal/inbox/send",
            json!({
                "recipient": { "principal": "user:harbour" }, "topic": "app.news",
                "template": "welcome", "idempotency_key": "x-2",
            }),
        )
        .await;
    assert_eq!(status, 404, "{refused}");
}

// ---------------------------------------------------------------------------------------------
// The automation step create-notice
// ---------------------------------------------------------------------------------------------

const T0: i64 = 1_000_000;

fn app() -> Shared {
    App::new(
        config("127.0.0.1:0".parse().unwrap()),
        Arc::new(MemoryStore::new()),
    )
}

fn name(s: &str) -> strata_kernel::StepName {
    strata_kernel::StepName::new(s).unwrap()
}

/// `notify`: creates a notice for `alpha:reader` and ends, run as a role that holds messages-send.
/// `quiet`: the same, run as a role that does not.
fn notifier(id: &str, run_as: &str) -> strata_kernel::AutomationDef {
    let step = |action, next: Option<&str>| strata_kernel::AutomationStepDef {
        action,
        next: next.map(name),
        retry: None,
        timeout: None,
        compensate: None,
    };
    strata_kernel::AutomationDef {
        state: None,
        id: strata_kernel::WorkflowId::new(id).unwrap(),
        label: id.to_string(),
        run_as: strata_kernel::RunAs {
            roles: BTreeSet::from([role(run_as)]),
        },
        run_key: None,
        start: name("tell"),
        steps: BTreeMap::from([
            (
                name("tell"),
                step(
                    strata_kernel::AutomationStep::CreateNotice {
                        template: TemplateId("update".into()),
                        to: Some(lit("alpha:reader")),
                        segment: None,
                        record: None,
                    },
                    Some("done"),
                ),
            ),
            (name("done"), step(strata_kernel::AutomationStep::End, None)),
        ]),
        defined_by: None,
        defined_at: None,
        suspended_for: None,
    }
}

fn install(app: &App) {
    let store = app.store.as_ref();
    let owner = principal_id("alpha:owner");
    let mut now = 100;
    let mut run = |principal: Principal, intent: Intent| {
        now += 1;
        execute(store, &intent, &ctx("alpha", principal, now)).expect("fixture intent");
    };
    run(
        Principal::System,
        Intent::CreateTenant {
            name: "alpha".to_string(),
            owner: owner.clone(),
        },
    );
    run(
        Principal::User(owner.clone()),
        Intent::ChangeSchema {
            changes: vec![
                SchemaChange::DefineRole(strata_kernel::RoleDef {
                    id: role("messenger"),
                    label: "Messenger".into(),
                }),
                SchemaChange::DefineRolePermissions {
                    role: role("messenger"),
                    permissions: vec![Permission::Capability(inbox::MESSAGES_SEND.to_string())],
                },
                SchemaChange::DefineRole(strata_kernel::RoleDef {
                    id: role("bystander"),
                    label: "Bystander".into(),
                }),
                SchemaChange::DefineNoticeTemplate(NoticeTemplateDecl {
                    template: TemplateId("update".into()),
                    title: "An update".into(),
                    body: StructuredBlocks(
                        "[{\"type\":\"paragraph\",\"inlines\":[{\"type\":\"text\",\"text\":\"Something changed.\"}]}]".into(),
                    ),
                }),
                SchemaChange::DefineAutomation(notifier("notify", "messenger")),
                SchemaChange::DefineAutomation(notifier("hush", "bystander")),
            ],
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
}

fn start_run(app: &App, automation: &str, seq: i64, due: i64) -> String {
    let job = NewJob {
        key: format!("start:test:{automation}:{seq}"),
        work: JobWork::StartRun {
            trigger: None,
            automation: strata_kernel::WorkflowId::new(automation).unwrap(),
            event_seq: Some(seq as u64),
            hop: 0,
            params: BTreeMap::new(),
            caused_by: None,
            on_behalf_of: None,
            definition_version: None,
            run_key: None,
        },
        due_at: Timestamp(due),
        policy: RetryPolicy::default(),
    };
    app.store
        .jobs_create(&tenant_id("alpha"), &[job], Timestamp(due))
        .unwrap()
        .remove(0)
}

fn at(t: i64) -> impl Fn() -> Timestamp {
    move || Timestamp(t)
}

fn notices_of(app: &App, person: &str) -> Vec<(String, String)> {
    app.store
        .doc_list(&tenant_id("alpha"), &format!("inbox-item:{person}"))
        .unwrap()
}

// Verifies: WFL-1523, SCH-015
#[test]
fn the_automation_step_creates_one_notice_with_a_typed_result() {
    let app = app();
    install(&app);
    let job = start_run(&app, "notify", 7, T0);
    let config = WorkerConfig::from_limits(&Default::default());
    let pass = worker::tick(&app, "w1", &config, &at(T0));
    assert_eq!(pass.jobs, 3, "the start, the notice and the end");
    let run = app
        .store
        .automation_run(&tenant_id("alpha"), &job)
        .unwrap()
        .expect("the run");
    assert_eq!(run.state, RunState::Completed);
    let found = notices_of(&app, "alpha:reader");
    assert_eq!(found.len(), 1);
    let item: Json = serde_json::from_str(&found[0].1).unwrap();
    assert_eq!(item["title"], "An update");
    assert_eq!(item["body"], "Something changed.");
    assert_eq!(item["topic"], inbox::AUTOMATION_TOPIC);
    // The step record holds the typed result: the item, which is the document's key.
    let steps = app
        .store
        .automation_steps(&tenant_id("alpha"), &job, None, 10)
        .unwrap();
    let result = steps[0].result.clone().expect("the step's result");
    assert_eq!(result, json!({ "CreateNotice": { "item": found[0].0 } }));
    // Nothing more is due, and nobody else has a notice.
    assert_eq!(
        worker::tick(&app, "w1", &config, &at(T0 + 3_600_000)).jobs,
        0
    );
    assert_eq!(notices_of(&app, "alpha:reader").len(), 1);
    assert!(notices_of(&app, "alpha:other").is_empty());
}

// Verifies: SCH-015
#[test]
fn a_step_attempted_again_after_a_crash_makes_no_second_notice() {
    let app = app();
    install(&app);
    let alpha = tenant_id("alpha");
    let job = start_run(&app, "notify", 8, T0);
    let first = app
        .store
        .job_lease(&alpha, "w1", Timestamp(T0))
        .unwrap()
        .unwrap();
    assert!(matches!(
        leases::run_leased(&app, &alpha, &first, &at(T0)).unwrap(),
        Reported::Recorded(_)
    ));
    // w1 leases the step, makes the notice, and stops before it reports.
    let held = app
        .store
        .job_lease(&alpha, "w1", Timestamp(T0))
        .unwrap()
        .unwrap();
    let outcome = steps::run_job(&app, &alpha, &held, Timestamp(T0));
    assert_eq!(
        outcome.report,
        strata_storage_api::jobs::JobReport::Succeeded
    );
    assert_eq!(notices_of(&app, "alpha:reader").len(), 1);
    // The lease expires; w2 attempts the step again with the same step key.
    let config = WorkerConfig::from_limits(&Default::default());
    let pass = worker::tick(&app, "w2", &config, &at(T0 + 60_001));
    assert_eq!(pass.jobs, 2, "the step again, then the end");
    assert_eq!(
        notices_of(&app, "alpha:reader").len(),
        1,
        "one notice, not two"
    );
    let run = app.store.automation_run(&alpha, &job).unwrap().unwrap();
    assert_eq!(run.state, RunState::Completed);
    // A second run is a second step key and so a second notice.
    start_run(&app, "notify", 9, T0 + 70_000);
    worker::tick(&app, "w2", &config, &at(T0 + 70_000));
    assert_eq!(notices_of(&app, "alpha:reader").len(), 2);
}

// Verifies: WFL-1527
#[test]
fn an_automation_without_messages_send_fails_its_step_and_makes_no_notice() {
    let app = app();
    install(&app);
    let job = start_run(&app, "hush", 10, T0);
    let config = WorkerConfig::from_limits(&Default::default());
    worker::tick(&app, "w1", &config, &at(T0));
    let run = app
        .store
        .automation_run(&tenant_id("alpha"), &job)
        .unwrap()
        .unwrap();
    assert_eq!(run.state, RunState::Failed);
    assert!(
        run.error
            .as_deref()
            .unwrap_or("")
            .contains(inbox::MESSAGES_SEND),
        "{:?}",
        run.error
    );
    assert!(notices_of(&app, "alpha:reader").is_empty());
}

// ---------------------------------------------------------------------------------------------
// Digests (NTF-1572) and the end of an inbox item (ret-inbox-item)
// ---------------------------------------------------------------------------------------------

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;
/// A UTC midnight; a daily period of a person in UTC ends at 09:00.
const DAY0: i64 = 20_000 * DAY;

fn person(name: &str) -> PrincipalId {
    principal_id(&format!("alpha:{name}"))
}

/// The tenant of `install`, with the category `news` and the mode of `alpha:reader` for it on the
/// inbox channel.
fn digest_fixture(app: &App, mode: &str) {
    install(app);
    let tenant = tenant_id("alpha");
    strata_backend::dispatch::store::put_category(
        app,
        &tenant,
        &strata_backend::dispatch::store::Category {
            category: "news".into(),
            purpose: strata_backend::dispatch::Purpose::Transactional,
            name: "news".into(),
            version: 1,
        },
    )
    .unwrap();
    set_mode(app, "reader", mode);
}

fn set_mode(app: &App, who: &str, mode: &str) {
    app.store
        .doc_put(
            &tenant_id("alpha"),
            "notice-preference",
            &format!("alpha:{who}|news|inbox"),
            &json!({ "mode": mode, "period": "daily" }).to_string(),
            false,
        )
        .unwrap();
}

/// A notice of the category `news` for `who`, made at `at`, about the record `record`.
fn notify_at(app: &App, who: &str, key: &str, at: i64, record: Option<u64>) -> inbox::Delivery {
    let tenant = tenant_id("alpha");
    let state = app.store.load(&tenant, &[]).unwrap().tenant.unwrap();
    let notice = inbox::Notice {
        topic: "app.news".into(),
        category: Some("news".into()),
        template: "update".into(),
        values: BTreeMap::new(),
        reference: None,
        record,
        mail: false,
    };
    inbox::deliver(
        app,
        &tenant,
        &state,
        &person(who),
        key,
        &notice,
        Timestamp(at),
    )
    .unwrap()
}

fn items_of(app: &App, who: &str) -> Vec<Json> {
    notices_of(app, &format!("alpha:{who}"))
        .into_iter()
        .map(|(_, text)| serde_json::from_str(&text).unwrap())
        .collect()
}

fn digests_of(app: &App, who: &str) -> Vec<Json> {
    items_of(app, who)
        .into_iter()
        .filter(|item| item["topic"] == inbox::digest::DIGEST_TOPIC)
        .collect()
}

// Verifies: NTF-3101
#[test]
fn a_digest_lists_the_notices_of_a_period_once_and_an_empty_period_sends_none() {
    let app = app();
    digest_fixture(&app, "digest");
    let tenant = tenant_id("alpha");
    notify_at(&app, "reader", "n-1", DAY0 + 10 * HOUR, None);
    notify_at(&app, "reader", "n-2", DAY0 + 11 * HOUR, None);
    let run = |at: i64| {
        inbox::digest::run_due(&app, &tenant, Timestamp(at))
            .unwrap()
            .digests
    };
    // The period that holds the notices has not ended at noon.
    assert_eq!(run(DAY0 + 12 * HOUR), 0);
    assert!(digests_of(&app, "reader").is_empty());
    // At 09:00 the next day it has: one message lists both.
    assert_eq!(run(DAY0 + 34 * HOUR), 1);
    let made = digests_of(&app, "reader");
    assert_eq!(made.len(), 1);
    assert_eq!(made[0]["title"], "2 new notices");
    assert_eq!(made[0]["body"], "An update\nAn update");
    // Running again, in the same period or an hour later, makes no second message.
    assert_eq!(run(DAY0 + 34 * HOUR), 0);
    assert_eq!(run(DAY0 + 35 * HOUR), 0);
    assert_eq!(digests_of(&app, "reader").len(), 1);
    // A notice made after the boundary waits for the next period and has a digest of its own.
    notify_at(&app, "reader", "n-3", DAY0 + 35 * HOUR, None);
    assert_eq!(run(DAY0 + 40 * HOUR), 0);
    assert_eq!(run(DAY0 + 58 * HOUR), 1);
    let mut titles: Vec<String> = digests_of(&app, "reader")
        .iter()
        .map(|item| item["title"].as_str().unwrap().to_string())
        .collect();
    titles.sort();
    assert_eq!(titles, ["1 new notice", "2 new notices"]);
    // A period with no notice sends none.
    assert_eq!(run(DAY0 + 82 * HOUR), 0);
    assert_eq!(digests_of(&app, "reader").len(), 2);
    // A person whose mode is on has no digest.
    notify_at(&app, "other", "n-4", DAY0 + 10 * HOUR, None);
    set_mode(&app, "other", "on");
    run(DAY0 + 106 * HOUR);
    assert!(digests_of(&app, "other").is_empty());
}

// Verifies: NTF-3101
#[test]
fn the_digest_chain_delivers_through_the_worker_and_ends_with_the_last_digest_preference() {
    let app = app();
    digest_fixture(&app, "digest");
    let tenant = tenant_id("alpha");
    notify_at(&app, "reader", "n-1", DAY0 + 10 * HOUR, None);
    let config = WorkerConfig::from_limits(&Default::default());
    assert!(inbox::digest::ensure_chain(&app, &tenant, Timestamp(DAY0 + 33 * HOUR + 10)).unwrap());
    assert!(
        !inbox::digest::ensure_chain(&app, &tenant, Timestamp(DAY0 + 33 * HOUR + 20)).unwrap(),
        "one open job at a time"
    );
    assert_eq!(
        worker::tick(&app, "w1", &config, &at(DAY0 + 33 * HOUR)).jobs,
        0
    );
    assert_eq!(
        worker::tick(&app, "w1", &config, &at(DAY0 + 34 * HOUR)).jobs,
        1
    );
    assert_eq!(digests_of(&app, "reader").len(), 1);
    let next = |step: u64| {
        app.store
            .jobs_list(&tenant, None, 20)
            .unwrap()
            .iter()
            .any(|job| matches!(job.work, JobWork::Digest { step: s } if s == step))
    };
    assert!(next(1), "a digest preference remains, so the chain goes on");
    // The last digest preference is turned off: the next run ends the chain.
    set_mode(&app, "reader", "on");
    worker::tick(&app, "w1", &config, &at(DAY0 + 35 * HOUR));
    assert!(!next(2));
}

// Verifies: NTF-3103
#[test]
fn an_inbox_item_ends_when_the_record_it_names_is_erased_or_its_person_is() {
    let app = app();
    digest_fixture(&app, "on");
    let tenant = tenant_id("alpha");
    notify_at(&app, "reader", "n-1", T0, Some(42));
    notify_at(&app, "reader", "n-2", T0 + 1, Some(7));
    notify_at(&app, "reader", "n-3", T0 + 2, None);
    notify_at(&app, "other", "n-4", T0 + 3, Some(42));
    set_mode(&app, "other", "off");
    let ended = inbox::digest::end_for_erasure(
        &app,
        &tenant,
        &person("other"),
        &BTreeSet::from([42]),
        Timestamp(T0 + 10),
    )
    .unwrap();
    assert_eq!(
        ended.items, 2,
        "the reader's item that names 42, and the subject's own"
    );
    assert_eq!(ended.preferences, 1);
    let live = |who: &str| {
        items_of(&app, who)
            .into_iter()
            .filter(|item| item["removed_at"].is_null())
            .count()
    };
    assert_eq!(live("reader"), 2, "the other notices of the reader stay");
    assert_eq!(live("other"), 0);
    // An ended item holds no text, only its id.
    for item in items_of(&app, "reader") {
        if !item["removed_at"].is_null() {
            assert_eq!(item["title"], "");
            assert_eq!(item["body"], "");
        }
    }
    // A send that is repeated after the end makes nothing again (CT-183).
    assert!(!notify_at(&app, "reader", "n-1", T0 + 20, Some(42)).created);
}

// Verifies: NTF-3104
#[test]
fn a_mail_digest_is_one_message_of_the_digest_template_under_a_key_of_its_own() {
    let app = app();
    digest_fixture(&app, "on");
    let tenant = tenant_id("alpha");
    execute(
        app.store.as_ref(),
        &Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineMailTemplate(MailTemplateDecl {
                template: TemplateId("digest_mail_news".into()),
                subject: "Your news".into(),
                body: StructuredBlocks(
                    r#"[{"type":"paragraph","inlines":[{"type":"value","name":"items","value_type":"str"}]}]"#
                        .into(),
                ),
            })],
            expected_version: None,
            existing: ExistingData::Reject,
        },
        &ctx("alpha", Principal::User(person("owner")), 500),
    )
    .expect("the owner defines the template");
    app.store
        .doc_put(
            &tenant,
            strata_backend::dispatch::store::CATEGORIES,
            "news",
            &json!({ "category": "news", "purpose": "transactional", "name": "News", "version": 1 })
                .to_string(),
            false,
        )
        .unwrap();
    app.store
        .doc_put(
            &tenant,
            "notice-preference",
            "alpha:reader|news|mail",
            &json!({ "mode": "digest", "period": "daily" }).to_string(),
            false,
        )
        .unwrap();
    notify_at(&app, "reader", "n-1", DAY0 + 10 * HOUR, None);
    notify_at(&app, "reader", "n-2", DAY0 + 11 * HOUR, None);
    let mail_jobs = || -> Vec<Json> {
        app.store
            .jobs_list(&tenant, None, 50)
            .unwrap()
            .into_iter()
            .filter_map(|job| match job.work {
                JobWork::Mail { request } => Some(request),
                _ => None,
            })
            .collect()
    };
    let run = |at: i64| {
        inbox::digest::run_due(&app, &tenant, Timestamp(at))
            .unwrap()
            .digests
    };
    assert_eq!(run(DAY0 + 12 * HOUR), 0, "the period has not ended");
    assert_eq!(run(DAY0 + 34 * HOUR), 1);
    let sent = mail_jobs();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["template"], "digest_mail_news");
    assert_eq!(
        sent[0]["values"]["items"],
        "An update
An update"
    );
    // The same period again, or a retry of the run, makes no second message.
    assert_eq!(run(DAY0 + 34 * HOUR), 0);
    assert_eq!(run(DAY0 + 36 * HOUR), 0);
    assert_eq!(mail_jobs().len(), 1);
}
