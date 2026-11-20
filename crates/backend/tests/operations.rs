//! Invoking a tenant operation through the backend (contract invoke-tenant-operation, CT-805;
//! WG-5100): a running service on the in-memory store, the real bundles, and an operation the
//! owner of fieldnotes defines. A member invokes it without any power over the records it writes;
//! it commits all of its steps or none; a repeat of the key answers with the first result.

use serde_json::{json, Value as Json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strata_backend::app::{App, Config};
use strata_backend::mail::DevSink;
use strata_kernel::*;
use strata_protocol::api::{
    HEADER_CORRELATION, HEADER_IDEMPOTENCY, HEADER_ORIGIN, HEADER_SERVICE, HEADER_SESSION,
    HEADER_SITE, HEADER_SOURCE,
};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, Store};

const TOKEN: &str = "test-service-token-0123456789abcdef";
const ORIGIN: &str = "http://localhost:4200";

struct Backend {
    base: String,
    http: reqwest::Client,
    store: Arc<MemoryStore>,
    app: Arc<App>,
}

async fn start() -> Backend {
    let store = Arc::new(MemoryStore::new());
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
        mail: Arc::new(DevSink::memory()),
        federation: Arc::new(strata_backend::signin::oidc::Federation::none()),
        factors: Default::default(),
        limits: Default::default(),
    };
    let app = App::new(cfg.clone(), store.clone());
    tokio::spawn(strata_backend::run(
        cfg,
        store.clone(),
        listener,
        std::future::pending(),
    ));
    let backend = Backend {
        base: format!("http://{addr}"),
        http: reqwest::Client::new(),
        store,
        app,
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
    TenantId::new("fieldnotes").unwrap()
}

fn field_def(id: &str, ty: FieldType, required: bool, unique: bool) -> FieldDef {
    FieldDef {
        id: FieldId::new(id).unwrap(),
        label: id.to_string(),
        ty,
        required,
        unique,
        default: None,
        read: None,
        personal: None,
    }
}

/// An operation that creates a tally with the label it is given and then counts it twice; the
/// second step reads the record of the first as `param.made`. With `fail`, the second step writes
/// text into the count, which the field type refuses.
fn tally(id: &str, fail: bool) -> TenantOperationDef {
    let object = ObjectTypeId::new("tally").unwrap();
    let count = if fail {
        Expr::Lit(Value::Str("two".into()))
    } else {
        Expr::Lit(Value::Int(2))
    };
    TenantOperationDef {
        id: TenantOperationId::new(id).unwrap(),
        label: "Tally".into(),
        inputs: vec![TenantOperationInput {
            name: "label".into(),
            ty: FieldType::Str,
            required: true,
            label: None,
        }],
        invokers: TenantOperationInvokers {
            signed_in: true,
            roles: BTreeSet::new(),
        },
        subject: None,
        conditions: vec![TenantOperationCondition {
            id: "labelled".into(),
            expr: Expr::Cmp(
                CmpOp::Gt,
                Box::new(Expr::Len(Box::new(Expr::Param("label".into())))),
                Box::new(Expr::Lit(Value::Int(0))),
            ),
            error: "empty-label".into(),
            message: "A tally has a label.".into(),
        }],
        steps: vec![
            TenantOperationStep::CreateRecord {
                id: "made".into(),
                object: object.clone(),
                fields: BTreeMap::from([
                    (FieldId::new("label").unwrap(), Expr::Param("label".into())),
                    (FieldId::new("count").unwrap(), Expr::Lit(Value::Int(1))),
                ]),
            },
            TenantOperationStep::UpdateRecord {
                id: "bump".into(),
                object,
                record: Expr::Param("made".into()),
                fields: BTreeMap::from([(FieldId::new("count").unwrap(), count)]),
            },
        ],
    }
}

impl Backend {
    /// The owner of fieldnotes defines the object type `tally`, on which no member holds any
    /// power, and the two operations.
    fn define(&self) {
        let object = ObjectTypeId::new("tally").unwrap();
        let intent = Intent::ChangeSchema {
            changes: vec![
                SchemaChange::DefineObject {
                    id: object.clone(),
                    label: "Tally".into(),
                },
                SchemaChange::DefineField {
                    object: object.clone(),
                    field: field_def("label", FieldType::Str, true, true),
                },
                SchemaChange::DefineField {
                    object,
                    field: field_def("count", FieldType::Int, true, false),
                },
                SchemaChange::DefineOperation(tally("tally_up", false)),
                SchemaChange::DefineOperation(tally("tally_fail", true)),
            ],
            expected_version: None,
            existing: ExistingData::Reject,
        };
        let ctx = Context {
            tenant: fieldnotes(),
            principal: Principal::User(PrincipalId::new("user:admin").unwrap()),
            session: None,
            now: self.app.now(),
            correlation: "t-operations-define".into(),
            verified_address: None,
            idempotency_key: None,
            language: None,
            hop: None,
        };
        execute(self.store.as_ref(), &intent, &ctx).expect("the owner defines the operations");
    }

    async fn post(
        &self,
        path: &str,
        site: &str,
        session: Option<&str>,
        key: Option<&str>,
        body: Json,
    ) -> (u16, Json) {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header(HEADER_SERVICE, TOKEN)
            .header(HEADER_SITE, site)
            .header(HEADER_SOURCE, "10.0.0.9")
            .header(HEADER_ORIGIN, ORIGIN)
            .header(HEADER_CORRELATION, "t-operations-1")
            .json(&body);
        if let Some(token) = session {
            request = request.header(HEADER_SESSION, token);
        }
        if let Some(key) = key {
            request = request.header(HEADER_IDEMPOTENCY, key);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Json::Null))
    }

    async fn sign_in(&self, login: &str, password: &str) -> String {
        let (status, reply) = self
            .post(
                "/internal/session/login",
                "fieldnotes",
                None,
                None,
                json!({ "login": login, "password": password }),
            )
            .await;
        assert_eq!(status, 200, "{reply}");
        reply["token"]
            .as_str()
            .expect("a session token")
            .to_string()
    }

    async fn invoke(&self, session: Option<&str>, key: Option<&str>, body: Json) -> (u16, Json) {
        self.post(
            "/internal/operations/invoke",
            "fieldnotes",
            session,
            key,
            body,
        )
        .await
    }

    /// The tallies of fieldnotes, by label.
    fn tallies(&self) -> BTreeMap<String, Arc<Record>> {
        let state = self.store.load_complete(&fieldnotes()).unwrap();
        let tenant = state.tenant.unwrap();
        tenant
            .records_of(&ObjectTypeId::new("tally").unwrap())
            .map(|record| {
                let label = match record.get(&FieldId::new("label").unwrap()) {
                    Value::Str(label) => label.clone(),
                    other => panic!("a label: {other:?}"),
                };
                (label, record.clone())
            })
            .collect()
    }
}

fn failed(reply: &Json) -> Vec<String> {
    reply["failed"]
        .as_array()
        .map(|checks| {
            checks
                .iter()
                .filter_map(|check| check["rule"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

// Verifies: OPR-1546
// Verifies: OPR-1548
#[tokio::test]
async fn a_member_invokes_an_operation_without_any_power_over_what_it_writes() {
    let backend = start().await;
    backend.define();
    let tomasz = backend.sign_in("tomasz", "strata-dev-tomasz").await;

    // Directly, the member may not create a tally.
    let (status, reply) = backend
        .post(
            "/internal/mutate",
            "fieldnotes",
            Some(&tomasz),
            None,
            json!({ "op": "create", "object": "tally", "fields": { "label": "direct", "count": 1 } }),
        )
        .await;
    assert_eq!(status, 403, "{reply}");

    let (status, reply) = backend
        .invoke(
            Some(&tomasz),
            Some("k-1"),
            json!({ "operation": "tally_up", "inputs": { "label": "first" } }),
        )
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["dry_run"], false);
    assert_eq!(reply["result"]["replayed"], false);
    assert_eq!(reply["result"]["created"].as_array().unwrap().len(), 1);
    let tallies = backend.tallies();
    let made = &tallies["first"];
    assert_eq!(
        made.created_by,
        Some(PrincipalId::new("user:tomasz").unwrap())
    );
    assert_eq!(made.get(&FieldId::new("count").unwrap()), &Value::Int(2));
    assert_eq!(tallies.len(), 1, "the direct create made nothing");
}

// Verifies: OPR-1541
// Verifies: OPR-1542
#[tokio::test]
async fn a_refused_later_step_commits_nothing() {
    let backend = start().await;
    backend.define();
    let tomasz = backend.sign_in("tomasz", "strata-dev-tomasz").await;
    let (status, reply) = backend
        .invoke(
            Some(&tomasz),
            Some("k-2"),
            json!({ "operation": "tally_fail", "inputs": { "label": "half" } }),
        )
        .await;
    assert_eq!(status, 422, "{reply}");
    assert_eq!(reply["kind"], "TypeMismatch");
    assert!(failed(&reply).contains(&"operation:tally_fail:step:bump".to_string()));
    assert!(
        backend.tallies().is_empty(),
        "the first step is not committed"
    );
}

// Verifies: OPR-1544
#[tokio::test]
async fn a_repeat_of_the_key_answers_the_first_result_and_other_input_is_a_mismatch() {
    let backend = start().await;
    backend.define();
    let tomasz = backend.sign_in("tomasz", "strata-dev-tomasz").await;
    let body = json!({ "operation": "tally_up", "inputs": { "label": "once" } });
    let (status, first) = backend
        .invoke(Some(&tomasz), Some("k-3"), body.clone())
        .await;
    assert_eq!(status, 200, "{first}");
    let (status, again) = backend.invoke(Some(&tomasz), Some("k-3"), body).await;
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["result"]["replayed"], true);
    assert_eq!(again["result"]["created"], first["result"]["created"]);
    assert_eq!(again["result"]["revision"], first["result"]["revision"]);
    assert_eq!(backend.tallies().len(), 1, "the repeat changed nothing");

    let (status, reply) = backend
        .invoke(
            Some(&tomasz),
            Some("k-3"),
            json!({ "operation": "tally_up", "inputs": { "label": "other" } }),
        )
        .await;
    assert_eq!(status, 409, "{reply}");
    assert_eq!(reply["kind"], "IdempotencyMismatch");
    assert_eq!(backend.tallies().len(), 1);

    // Another person's key is another key.
    let mirela = backend.sign_in("mirela", "strata-dev-mirela").await;
    let (status, reply) = backend
        .invoke(
            Some(&mirela),
            Some("k-3"),
            json!({ "operation": "tally_up", "inputs": { "label": "hers" } }),
        )
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["result"]["replayed"], false);
}

#[tokio::test]
async fn an_invocation_reaches_only_the_tenant_of_its_session() {
    let backend = start().await;
    backend.define();
    let tomasz = backend.sign_in("tomasz", "strata-dev-tomasz").await;
    // A session of fieldnotes is no session at tidewater.
    let (status, reply) = backend
        .post(
            "/internal/operations/invoke",
            "tidewater",
            Some(&tomasz),
            Some("k-4"),
            json!({ "operation": "tally_up", "inputs": { "label": "elsewhere" } }),
        )
        .await;
    assert_eq!(status, 401, "{reply}");
    assert!(backend.tallies().is_empty());
    let tidewater = backend
        .store
        .load_complete(&TenantId::new("tidewater").unwrap())
        .unwrap();
    assert!(tidewater
        .tenant
        .unwrap()
        .records_of(&ObjectTypeId::new("tally").unwrap())
        .next()
        .is_none());
}

#[tokio::test]
async fn each_refusal_of_an_invocation() {
    let backend = start().await;
    backend.define();
    let tomasz = backend.sign_in("tomasz", "strata-dev-tomasz").await;
    let up = |label: &str| json!({ "operation": "tally_up", "inputs": { "label": label } });

    // No session: sign in first.
    let (status, _) = backend.invoke(None, Some("k-5"), up("a")).await;
    assert_eq!(status, 401);
    // No key, or one that is not a key.
    let (status, _) = backend.invoke(Some(&tomasz), None, up("a")).await;
    assert_eq!(status, 422);
    let long = "k".repeat(129);
    let (status, _) = backend.invoke(Some(&tomasz), Some(&long), up("a")).await;
    assert_eq!(status, 422);
    // An operation the tenant does not have.
    let (status, reply) = backend
        .invoke(
            Some(&tomasz),
            Some("k-6"),
            json!({ "operation": "nothing", "inputs": {} }),
        )
        .await;
    assert_eq!(status, 404, "{reply}");
    // An input the operation does not declare, and one that does not coerce.
    let (status, reply) = backend
        .invoke(
            Some(&tomasz),
            Some("k-7"),
            json!({ "operation": "tally_up", "inputs": { "label": "a", "count": 9 } }),
        )
        .await;
    assert_eq!(status, 422, "{reply}");
    assert_eq!(failed(&reply), vec!["input:tally_up.count".to_string()]);
    let (status, reply) = backend
        .invoke(
            Some(&tomasz),
            Some("k-8"),
            json!({ "operation": "tally_up", "inputs": { "label": 5 } }),
        )
        .await;
    assert_eq!(status, 422, "{reply}");
    // A false condition, with its code.
    let (status, reply) = backend.invoke(Some(&tomasz), Some("k-9"), up("")).await;
    assert_eq!(status, 422, "{reply}");
    assert_eq!(reply["kind"], "ConstraintViolated");
    assert_eq!(reply["message"], "empty-label: A tally has a label.");
    // A refused step: the label is unique.
    let (status, _) = backend.invoke(Some(&tomasz), Some("k-10"), up("dup")).await;
    assert_eq!(status, 200);
    let (status, reply) = backend.invoke(Some(&tomasz), Some("k-11"), up("dup")).await;
    assert_eq!(status, 422, "{reply}");
    assert_eq!(reply["kind"], "UniqueViolated");
    assert!(failed(&reply).contains(&"operation:tally_up:step:made".to_string()));
    assert_eq!(backend.tallies().len(), 1, "only the first dup was made");
}

// Verifies: OPR-1552
#[tokio::test]
async fn a_dry_run_answers_and_commits_nothing() {
    let backend = start().await;
    backend.define();
    let tomasz = backend.sign_in("tomasz", "strata-dev-tomasz").await;
    let (status, reply) = backend
        .invoke(
            Some(&tomasz),
            Some("k-12"),
            json!({ "operation": "tally_up", "inputs": { "label": "maybe" }, "dry_run": true }),
        )
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["dry_run"], true);
    assert_eq!(reply["result"]["created"].as_array().unwrap().len(), 1);
    assert!(backend.tallies().is_empty());
}
