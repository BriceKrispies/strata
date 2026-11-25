//! The segment routes of the backend, end to end over HTTP against the in-memory store: a
//! definition, membership kept by the commits of ordinary changes, the bound on one commit and
//! its continuation, the reads, the refusals, and tenancy.

use serde_json::{json, Value as Json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use strata_backend::app::Config;
use strata_kernel::{
    owner_role, Context, ExistingData, Expr, FieldDef, FieldId, FieldType, Intent, ObjectTypeId,
    OnDelete, Principal, PrincipalId, RecordId, SchemaChange, TenantId, Timestamp, Value,
};
use strata_protocol::api::{HEADER_CORRELATION, HEADER_SERVICE, HEADER_SESSION, HEADER_SITE};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, segments as kept, Store};

const TOKEN: &str = "test-service-token-0123456789abcdef";
const PRO: &str = "6a0f3c1e-2280-4000-8000-000000000002";

struct Backend {
    base: String,
    http: reqwest::Client,
}

async fn start(store: Arc<MemoryStore>) -> Backend {
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
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
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
            .header(HEADER_CORRELATION, "t-segments-1")
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
}

fn fieldnotes() -> TenantId {
    TenantId::new("fieldnotes").unwrap()
}

fn object(name: &str) -> ObjectTypeId {
    ObjectTypeId::new(name).unwrap()
}

fn field(name: &str) -> FieldId {
    FieldId::new(name).unwrap()
}

fn as_admin(now: i64) -> Context {
    Context {
        tenant: fieldnotes(),
        principal: Principal::User(PrincipalId::new("user:admin").unwrap()),
        session: None,
        now: Timestamp(now),
        correlation: format!("t-segments-{now}"),
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
    }
}

fn def(id: &str, ty: FieldType, required: bool) -> FieldDef {
    FieldDef {
        id: field(id),
        label: id.to_string(),
        ty,
        required,
        unique: false,
        default: None,
        read: None,
        personal: None,
        normalise: None,
        computed: None,
    }
}

/// Add `plan` { level } and `member` { principal, tier, salary (read by the owner only), plan }
/// to the seeded tenant, through the same `execute` every request uses.
fn add_members(store: &MemoryStore) {
    let changes = vec![
        SchemaChange::DefineObject {
            id: object("plan"),
            label: "Plan".into(),
        },
        SchemaChange::DefineField {
            object: object("plan"),
            field: def("level", FieldType::Str, true),
        },
        SchemaChange::DefineObject {
            id: object("member"),
            label: "Member".into(),
        },
        SchemaChange::DefineField {
            object: object("member"),
            field: FieldDef {
                unique: true,
                ..def("principal", FieldType::Str, true)
            },
        },
        SchemaChange::DefineField {
            object: object("member"),
            field: def("tier", FieldType::Str, false),
        },
        SchemaChange::DefineField {
            object: object("member"),
            field: FieldDef {
                read: Some(Expr::HasRole(owner_role())),
                ..def("salary", FieldType::Int, false)
            },
        },
        SchemaChange::DefineRelationship {
            object: object("member"),
            field: field("plan"),
            label: "Plan".into(),
            target: object("plan"),
            on_delete: OnDelete::SetNull,
            required: false,
        },
    ];
    execute(
        store,
        &Intent::ChangeSchema {
            changes,
            expected_version: None,
            existing: ExistingData::Reject,
        },
        &as_admin(1_000),
    )
    .expect("the member types are added");
}

fn create(store: &MemoryStore, now: i64, object_name: &str, fields: &[(&str, Value)]) -> RecordId {
    let done = execute(
        store,
        &Intent::CreateRecord {
            object: object(object_name),
            fields: fields
                .iter()
                .map(|(name, value)| (field(name), value.clone()))
                .collect::<BTreeMap<_, _>>(),
        },
        &as_admin(now),
    )
    .expect("the record is created");
    match done.transition.output {
        strata_kernel::Output::Record { id, .. } => id,
        other => panic!("unexpected output {other:?}"),
    }
}

fn update(
    store: &MemoryStore,
    now: i64,
    object_name: &str,
    id: RecordId,
    fields: &[(&str, Value)],
) {
    execute(
        store,
        &Intent::UpdateRecord {
            object: object(object_name),
            id,
            fields: fields
                .iter()
                .map(|(name, value)| (field(name), value.clone()))
                .collect(),
            expected_version: None,
        },
        &as_admin(now),
    )
    .expect("the record is updated");
}

fn text(s: &str) -> Value {
    Value::Str(s.to_string())
}

fn persons(reply: &Json) -> BTreeSet<String> {
    reply["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["person"].as_str().unwrap().to_string())
        .collect()
}

// Verifies: SEG-028
#[tokio::test]
async fn a_change_reaching_more_than_a_hundred_persons_leaves_the_rest_to_batches() {
    let store = Arc::new(MemoryStore::new());
    let backend = start(store.clone()).await;
    let admin = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    add_members(&store);
    let plan = create(&store, 2_000, "plan", &[("level", text("basic"))]);
    for n in 0..150 {
        create(
            &store,
            3_000 + n,
            "member",
            &[
                ("principal", text(&format!("user:m{n:03}"))),
                ("plan", Value::Ref(plan)),
            ],
        );
    }
    let (status, defined) = backend
        .call(
            "fieldnotes",
            Some(&admin),
            "/internal/segments/define",
            json!({
                "segment": PRO,
                "name": "Pro",
                "subject_type": "member",
                "predicate": "record.plan.level == 'pro'",
                "principal_field": "principal",
            }),
        )
        .await;
    assert_eq!(status, 200, "{defined}");
    assert_eq!(defined["batches"], 2, "150 persons: two batches (SEG-023)");
    // The definition's batches find nobody yet.
    let ctx = as_admin(4_000);
    assert_eq!(kept::run_pending(store.as_ref(), &ctx, 10).unwrap(), 0);

    update(&store, 5_000, "plan", plan, &[("level", text("pro"))]);
    assert_eq!(
        store
            .segment_events_after(&fieldnotes(), 0, 1_000)
            .unwrap()
            .len(),
        100,
        "the commit of the change enters a hundred"
    );
    let pending = store.segment_batches(&fieldnotes()).unwrap();
    assert_eq!(pending.len(), 1, "and leaves one continuation");
    let rest = kept::run_pending(store.as_ref(), &as_admin(5_001), 10).unwrap();
    assert_eq!(rest, 50);
    assert!(store.segment_batches(&fieldnotes()).unwrap().is_empty());
    let (_, count) = backend
        .call(
            "fieldnotes",
            Some(&admin),
            "/internal/segments/count",
            json!({ "segment": PRO }),
        )
        .await;
    assert_eq!(count["count"]["count"], 150);
    // Paging by cursor visits every member once.
    let mut seen = BTreeSet::new();
    let mut cursor: Option<String> = None;
    for _ in 0..10 {
        let mut body = json!({ "segment": PRO, "limit": 60 });
        if let Some(c) = &cursor {
            body["cursor"] = json!(c);
        }
        let (status, page) = backend
            .call(
                "fieldnotes",
                Some(&admin),
                "/internal/segments/members",
                body,
            )
            .await;
        assert_eq!(status, 200, "{page}");
        seen.extend(persons(&page));
        match page["next"].as_str() {
            Some(next) => cursor = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(seen.len(), 150);
}

// Verifies: SEG-005
#[tokio::test]
async fn a_predicate_over_a_hidden_field_is_refused_and_the_bounds_hold() {
    let store = Arc::new(MemoryStore::new());
    let backend = start(store.clone()).await;
    let admin = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    add_members(&store);
    let define = |n: u32, predicate: &str| {
        json!({
            "segment": format!("6a0f3c1e-2280-4000-8000-{n:012}"),
            "name": format!("segment {n}"),
            "subject_type": "member",
            "predicate": predicate,
            "principal_field": "principal",
        })
    };
    // salary has a read rule: whether a reader sees it depends on the reader, so no segment may
    // name it and let its members tell who earns what.
    let (status, refused) = backend
        .call(
            "fieldnotes",
            Some(&admin),
            "/internal/segments/define",
            define(100, "record.salary > 10"),
        )
        .await;
    assert_eq!(status, 403, "{refused}");
    // A predicate that does not parse is refused with its position (SEG-003).
    let (status, _) = backend
        .call(
            "fieldnotes",
            Some(&admin),
            "/internal/segments/define",
            define(101, "record.tier =="),
        )
        .await;
    assert_eq!(status, 422);
    // Twenty-five segments read members; the twenty-sixth is refused (SEG-029).
    for n in 1..=25 {
        let (status, body) = backend
            .call(
                "fieldnotes",
                Some(&admin),
                "/internal/segments/define",
                define(n, &format!("record.tier == 't{n}'")),
            )
            .await;
        assert_eq!(status, 200, "{body}");
    }
    let (status, _) = backend
        .call(
            "fieldnotes",
            Some(&admin),
            "/internal/segments/define",
            define(26, "record.tier == 'x'"),
        )
        .await;
    assert_eq!(status, 422);
    // A schema change that removes a field a segment names is refused (SEG-006).
    let removed = execute(
        store.as_ref(),
        &Intent::ChangeSchema {
            changes: vec![SchemaChange::RemoveField {
                object: object("member"),
                field: field("tier"),
            }],
            expected_version: None,
            existing: ExistingData::Grandfather,
        },
        &as_admin(9_000),
    );
    assert!(removed.is_err());
}

// Verifies: SEG-035
#[tokio::test]
async fn test_and_preview_change_nothing() {
    let store = Arc::new(MemoryStore::new());
    let backend = start(store.clone()).await;
    let admin = backend
        .login("fieldnotes", "admin", "strata-dev-admin")
        .await;
    add_members(&store);
    create(
        &store,
        2_000,
        "member",
        &[("principal", text("user:ines")), ("tier", text("gold"))],
    );
    create(
        &store,
        2_001,
        "member",
        &[("principal", text("user:tomasz")), ("tier", text("silver"))],
    );
    let revision = store
        .tenant_summary(&fieldnotes())
        .unwrap()
        .unwrap()
        .revision;
    let (status, tested) = backend
        .call(
            "fieldnotes",
            Some(&admin),
            "/internal/segments/test",
            json!({
                "subject_type": "member",
                "predicate": "record.tier == 'gold'",
                "person": "user:ines",
            }),
        )
        .await;
    assert_eq!(status, 200, "{tested}");
    assert_eq!(tested["result"]["member"], true);
    assert_eq!(tested["result"]["reads"], json!(["member.tier"]));
    // Verifies: SEG-1542
    let (status, previewed) = backend
        .call(
            "fieldnotes",
            Some(&admin),
            "/internal/segments/preview",
            json!({ "subject_type": "member", "predicate": "record.tier == 'gold'" }),
        )
        .await;
    assert_eq!(status, 200, "{previewed}");
    assert_eq!(previewed["preview"]["matched"], 1);
    assert_eq!(previewed["preview"]["examined"], 2);
    assert_eq!(
        store
            .tenant_summary(&fieldnotes())
            .unwrap()
            .unwrap()
            .revision,
        revision
    );
    assert!(store.segment_catalog(&fieldnotes()).unwrap().is_empty());
}
