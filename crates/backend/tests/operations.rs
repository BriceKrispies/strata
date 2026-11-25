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

/// Count a tally the reference input names: a reference input of an operation, with no subject.
fn recount() -> TenantOperationDef {
    TenantOperationDef {
        id: TenantOperationId::new("recount").unwrap(),
        label: "Recount".into(),
        inputs: vec![TenantOperationInput {
            name: "of".into(),
            ty: FieldType::Ref {
                target: ObjectTypeId::new("tally").unwrap(),
                on_delete: OnDelete::Restrict,
            },
            required: true,
            label: None,
        }],
        invokers: TenantOperationInvokers {
            signed_in: true,
            roles: BTreeSet::new(),
        },
        subject: None,
        conditions: vec![],
        steps: vec![TenantOperationStep::UpdateRecord {
            id: "bump".into(),
            object: ObjectTypeId::new("tally").unwrap(),
            record: Expr::Param("of".into()),
            fields: BTreeMap::from([(FieldId::new("count").unwrap(), Expr::Lit(Value::Int(3)))]),
        }],
    }
}

impl Backend {
    /// The owner of fieldnotes defines the object type `tally`, on which no member holds any
    /// power, and the operations.
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
                SchemaChange::DefineOperation(recount()),
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

// A reference input is a record id on the wire, and a form's text control sends the same id as a
// string (showcase gap 6): both are accepted, and the second is not refused as a type mismatch.
#[tokio::test]
async fn a_reference_input_sent_as_a_record_id_is_accepted() {
    let backend = start().await;
    backend.define();
    let tomasz = backend.sign_in("tomasz", "strata-dev-tomasz").await;
    let (status, reply) = backend
        .invoke(
            Some(&tomasz),
            Some("k-20"),
            json!({ "operation": "tally_up", "inputs": { "label": "linked" } }),
        )
        .await;
    assert_eq!(status, 200, "{reply}");
    let id = reply["result"]["created"][0]
        .as_u64()
        .expect("the created record id");
    let (status, reply) = backend
        .invoke(
            Some(&tomasz),
            Some("k-21"),
            json!({ "operation": "recount", "inputs": { "of": id.to_string() } }),
        )
        .await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["result"]["changed"], json!([id]));
    let (status, reply) = backend
        .invoke(
            Some(&tomasz),
            Some("k-22"),
            json!({ "operation": "recount", "inputs": { "of": id } }),
        )
        .await;
    assert_eq!(status, 200, "{reply}");
}
