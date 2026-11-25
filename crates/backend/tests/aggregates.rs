//! Aggregates of named queries over the backend's internal route (contract aggregate-run, CT-310)
//! and in a view section (AGG-041): what a reader is told counts only what the reader may read,
//! two readers with different visibility are told different things, and a refusal is by name.
//!
//! The tenant is the seeded `fieldnotes` blog: its public reads published posts only, an author
//! reads her own drafts, an editor reads every post, and an author's login identity is read by
//! that author and by editors only.

use serde_json::{json, Value as Json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strata_backend::app::Config;
use strata_kernel::{
    read, AggregateDef, AggregateId, AggregateKind, Context, ExistingData, FieldId, Intent, Need,
    ObjectTypeId, Principal, PrincipalId, QueryDef, QueryId, ReadRequest, SchemaChange, TenantId,
    Timestamp, ViewDef, ViewId, ViewSection,
};
use strata_protocol::api::{
    HEADER_CORRELATION, HEADER_SERVICE, HEADER_SESSION, HEADER_SITE, HEADER_SOURCE,
};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, Store};

const TOKEN: &str = "test-service-token-0123456789abcdef";
const SITE: &str = "fieldnotes";

struct Backend {
    base: String,
    http: reqwest::Client,
}

async fn start_on(store: Arc<MemoryStore>) -> Backend {
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
    async fn call(&self, session: Option<&str>, path: &str, body: Json) -> (u16, Json) {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header(HEADER_SERVICE, TOKEN)
            .header(HEADER_SITE, SITE)
            .header(HEADER_CORRELATION, "t-aggregates-1")
            .json(&body);
        if let Some(token) = session {
            request = request.header(HEADER_SESSION, token);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Json::Null))
    }

    async fn login(&self, login: &str) -> String {
        let (status, body) = self
            .call(
                None,
                "/internal/session/login",
                json!({ "login": login, "password": format!("strata-dev-{login}") }),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        body["token"].as_str().unwrap().to_string()
    }

    /// One aggregate of a named query, as the holder of `session` or as the public.
    async fn aggregate(
        &self,
        session: Option<&str>,
        query: &str,
        aggregate: &str,
        filter: Json,
    ) -> (u16, Json) {
        self.call(
            session,
            "/internal/aggregate",
            json!({ "query": query, "aggregate": aggregate, "filter": filter }),
        )
        .await
    }
}

fn tenant() -> TenantId {
    TenantId::new(SITE).unwrap()
}

fn declared(
    name: &str,
    kind: AggregateKind,
    field: Option<&str>,
    group: Option<&str>,
) -> AggregateDef {
    AggregateDef {
        id: AggregateId::new(name).unwrap(),
        kind,
        field: field.map(|f| FieldId::new(f).unwrap()),
        group_by: group.map(|f| FieldId::new(f).unwrap()),
        group_by_second: None,
        n: (kind == AggregateKind::Top).then_some(2),
        descending: true,
        scan_bound: if name == "tight" { 2 } else { 1_000 },
    }
}

fn query(
    id: &str,
    object: &str,
    aggregates: Option<Vec<AggregateDef>>,
    fields: &[&str],
) -> SchemaChange {
    SchemaChange::DefineQuery(QueryDef {
        id: QueryId::new(id).unwrap(),
        object: ObjectTypeId::new(object).unwrap(),
        filter: None,
        params: Default::default(),
        order: vec![],
        limit: None,
        aggregates,
        declared_fields: Some(fields.iter().map(|f| FieldId::new(*f).unwrap()).collect()),
        tree: None,
    })
}

fn section(aggregate: &str) -> ViewSection {
    ViewSection {
        marks: None,
        joins: None,
        query: QueryId::new("post_stats").unwrap(),
        args: Default::default(),
        include: vec![],
        markdown: vec![],
        single: false,
        aggregate: Some(AggregateId::new(aggregate).unwrap()),
    }
}

fn context(principal: Principal) -> Context {
    Context {
        tenant: tenant(),
        principal,
        session: None,
        now: Timestamp(2_000_000_000_000),
        correlation: "t-aggregates".into(),
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
    }
}

/// Define the queries, their aggregates and a view over two of them, as the tenant's owner does.
fn define(store: &MemoryStore) {
    use AggregateKind::*;
    let changes = vec![
        // The rows a reader sees, for comparison.
        query("post_rows", "post", None, &[]),
        query(
            "post_stats",
            "post",
            Some(vec![
                declared("count_all", Count, None, None),
                declared("by_status", Count, None, Some("status")),
                declared("newest", Top, Some("published_at"), None),
                declared("first_published", Minimum, Some("published_at"), None),
                declared("tight", Count, None, None),
            ]),
            &["status", "published_at", "featured"],
        ),
        // An author's login identity is the one field here that the reader's rights decide.
        query(
            "author_stats",
            "author",
            Some(vec![declared("by_login", Count, None, Some("principal"))]),
            &["principal"],
        ),
        SchemaChange::DefineView(ViewDef {
            id: ViewId::new("stats").unwrap(),
            route: "/stats".into(),
            title: "Stats".into(),
            sections: BTreeMap::from([
                ("counts".to_string(), section("count_all")),
                ("by_status".to_string(), section("by_status")),
            ]),
        }),
    ];
    let intent = Intent::ChangeSchema {
        changes,
        expected_version: None,
        existing: ExistingData::Reject,
    };
    let owner = Principal::User(PrincipalId::new("user:admin").unwrap());
    execute(store, &intent, &context(owner)).unwrap_or_else(|e| panic!("refused: {e}"));
}

/// How many posts the plain query returns to a principal: what the principal could count by hand.
fn rows_seen_by(store: &MemoryStore, principal: Principal) -> usize {
    let state = store
        .load(
            &tenant(),
            &[Need::AllOf(ObjectTypeId::new("post").unwrap())],
        )
        .unwrap();
    let request = ReadRequest::Named {
        query: QueryId::new("post_rows").unwrap(),
        params: Default::default(),
        page_size: Some(200),
        cursor: None,
    };
    read(&state, &request, &context(principal))
        .unwrap()
        .rows
        .len()
}

fn user(name: &str) -> Principal {
    Principal::User(PrincipalId::new(name).unwrap())
}

/// The value of the one group of an ungrouped aggregate in the route's plain model.
fn count_of(body: &Json) -> i64 {
    let groups = body["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1, "{body}");
    groups[0]["value"].as_i64().unwrap()
}

/// The route's answer to an aggregate that is accepted.
async fn answer(backend: &Backend, session: Option<&str>, query: &str, aggregate: &str) -> Json {
    let (status, body) = backend
        .aggregate(session, query, aggregate, json!({}))
        .await;
    assert_eq!(status, 200, "{body}");
    body
}

async fn groups_of(
    backend: &Backend,
    session: Option<&str>,
    query: &str,
    aggregate: &str,
) -> Vec<Json> {
    answer(backend, session, query, aggregate).await["groups"]
        .as_array()
        .unwrap()
        .clone()
}

async fn started() -> (Backend, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::new());
    let backend = start_on(store.clone()).await;
    define(&store);
    (backend, store)
}

// Verifies: AGG-015
// Verifies: AGG-016
#[tokio::test]
async fn readers_with_different_visibility_are_told_different_counts() {
    let (backend, store) = started().await;
    let editor = backend.login("ines").await;
    let author = backend.login("tomasz").await;

    let public = answer(&backend, None, "post_stats", "count_all").await;
    let by_author = answer(&backend, Some(&author), "post_stats", "count_all").await;
    let by_editor = answer(&backend, Some(&editor), "post_stats", "count_all").await;

    // Each count is what the reader's own plain query returns, row by row.
    assert_eq!(
        count_of(&public),
        rows_seen_by(&store, Principal::Anonymous) as i64
    );
    assert_eq!(
        count_of(&by_author),
        rows_seen_by(&store, user("user:tomasz")) as i64
    );
    assert_eq!(
        count_of(&by_editor),
        rows_seen_by(&store, user("user:ines")) as i64
    );
    assert_eq!(public["scanned"], count_of(&public));
    // The public reads the six published posts; the editor reads those and the draft; the
    // answers differ, so a count never tells a reader about a post it may not read.
    assert_eq!(count_of(&public), 6);
    assert_eq!(count_of(&by_editor), 7);
    assert!(count_of(&public) <= count_of(&by_author));
    assert!(count_of(&by_author) <= count_of(&by_editor));
    assert_eq!(public["depends_on"], json!(["post"]));
    assert_eq!(public["query"], "post_stats");
}

// Verifies: AGG-014
// Verifies: AGG-017
#[tokio::test]
async fn a_group_names_no_value_the_reader_may_not_read() {
    let (backend, _store) = started().await;
    // By status: the public is told of published posts and no other group.
    let public = groups_of(&backend, None, "post_stats", "by_status").await;
    assert_eq!(public.len(), 1, "{public:?}");
    assert_eq!(public[0]["key"], "published");
    let editor = backend.login("ines").await;
    let all = groups_of(&backend, Some(&editor), "post_stats", "by_status").await;
    let keys: BTreeSet<String> = all
        .iter()
        .map(|g| g["key"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from(["draft".to_string(), "published".to_string()])
    );

    // By login identity of the authors: the public may read the authors but not the field, so
    // it is told of no group; an author is told of her own and no other; an editor of all three.
    assert!(groups_of(&backend, None, "author_stats", "by_login")
        .await
        .is_empty());
    let author = backend.login("tomasz").await;
    let own = groups_of(&backend, Some(&author), "author_stats", "by_login").await;
    assert_eq!(own.len(), 1, "{own:?}");
    assert_eq!(own[0]["key"], "user:tomasz");
    let every = groups_of(&backend, Some(&editor), "author_stats", "by_login").await;
    assert_eq!(every.len(), 3, "{every:?}");
}

// Verifies: AGG-003
// Verifies: AGG-004
// Verifies: AGG-020
// Verifies: AGG-021
#[tokio::test]
async fn what_the_query_does_not_declare_and_what_costs_too_much_are_refused_by_name() {
    let (backend, _store) = started().await;
    // An aggregate or a query that does not exist.
    let (status, _) = backend
        .aggregate(None, "post_stats", "no_such_aggregate", json!({}))
        .await;
    assert_eq!(status, 404);
    let (status, _) = backend
        .aggregate(None, "no_such_query", "count_all", json!({}))
        .await;
    assert_eq!(status, 404);
    // A filter on a field the query does not declare: `title` is a field of a post.
    let (status, body) = backend
        .aggregate(None, "post_stats", "count_all", json!({ "title": "x" }))
        .await;
    assert_eq!(status, 422, "{body}");
    // An aggregate with a bound of 2 over the six posts the public reads is refused, by name,
    // with no result.
    let (status, body) = backend
        .aggregate(None, "post_stats", "tight", json!({}))
        .await;
    assert_eq!(status, 422, "{body}");
    assert_eq!(body["kind"], "LimitExceeded");
    assert_eq!(body["failed"][0]["rule"], "scan-bound-exceeded");
    assert!(body.get("groups").is_none(), "no result with a refusal");
    // The bound counts what the reader may read and the filter selects: the public may read no
    // draft, so a filter for drafts selects nothing and the same aggregate answers, with zero.
    let (status, body) = backend
        .aggregate(None, "post_stats", "tight", json!({ "status": "draft" }))
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(count_of(&body), 0);
}

impl Backend {
    /// Installs a product bundle as the owner, through the route.
    async fn install(&self, owner: &str, bundle: &Json) -> (u16, Json) {
        let response = self
            .http
            .post(format!("{}/internal/products/install", self.base))
            .header(HEADER_SERVICE, TOKEN)
            .header(HEADER_SITE, SITE)
            .header(HEADER_SOURCE, "203.0.113.7")
            .header(HEADER_CORRELATION, "t-aggregates-install")
            .header(HEADER_SESSION, owner)
            .json(&json!({ "bundle": bundle }))
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Json::Null))
    }
}
