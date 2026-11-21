//! The reference product badge-milestones, end to end (WG-5008): the bundle that specc writes from
//! `badge-milestones.jsonc` is installed into a tenant, points are posted to a person's ledger
//! account across the threshold, the worker runs, and the award record exists exactly once, read
//! through the API as that person. A second crossing starts no second run. An operation awards a
//! badge by hand, to the holders of the awarder role only, and a person reads their own awards and
//! no one else's. Real sockets, the in-memory store, the worker driven by `worker::tick`.
//!
//! Every step of the award runs: the award record, the inbox notice (create-notice, once for the
//! person), the mail (send-mail) and then the wait of seven days, where the run stays (Waiting). A
//! person is told in their own inbox and in no one else's, and a second crossing starts no second
//! run and so makes no second notice.

use serde_json::{json, Value as Json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strata_backend::app::{App, Config, Shared, ThrottleLimits};
use strata_backend::mail::DevSink;
use strata_backend::signin::oidc::Federation;
use strata_backend::worker::{self, WorkerConfig};
use strata_kernel::{
    Context, ExistingData, Intent, Principal, PrincipalId, RecordExt, RoleDef, RoleId,
    SchemaChange, TenantId, TenantStateExt, Timestamp, WorkflowId,
};
use strata_protocol::api::{
    HEADER_CORRELATION, HEADER_IDEMPOTENCY, HEADER_SERVICE, HEADER_SESSION, HEADER_SITE,
    HEADER_SOURCE,
};
use strata_protocol::log::now_ms;
use strata_storage_api::jobs::{JobWork, RunRecord, RunState, StepRecord, StepState};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, Store};

const TOKEN: &str = "test-service-token-0123456789abcdef";
const SOURCE: &str = "203.0.113.7";
const SITE: &str = "fieldnotes";

/// The bundle of `badge-milestones` as specc writes it (the golden file of its tests).
const BADGE_MILESTONES: &str = include_str!(
    "../../spec-compiler/tests/fixtures/products/expected/badge-milestones.bundle.json"
);

const UNIT: &str = "badge_milestones__points";
const AWARDER: &str = "badge_milestones__awarder";

/// The steps of the automation badge_milestones__silver that run before its wait of seven days.
const BEFORE_THE_WAIT: [&str; 3] = ["award", "notify", "mail"];

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
        throttle: ThrottleLimits::default(),
        passkey_challenge_ttl_ms: 300_000,
        link_ttl_ms: 900_000,
        mail: Arc::new(DevSink::memory()),
        federation: Arc::new(Federation::none()),
        factors: Default::default(),
        limits: Default::default(),
    }
}

struct Backend {
    base: String,
    http: reqwest::Client,
    store: Arc<MemoryStore>,
    /// The application the worker passes of the test run on: the same store as the server's, so
    /// the jobs the server's commits create are the jobs these passes take.
    app: Shared,
}

async fn start() -> Backend {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let store = Arc::new(MemoryStore::new());
    tokio::spawn(strata_backend::run(
        config(addr),
        store.clone(),
        listener,
        std::future::pending(),
    ));
    let backend = Backend {
        base: format!("http://{addr}"),
        http: reqwest::Client::new(),
        app: App::new(config(addr), store.clone()),
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

fn tenant() -> TenantId {
    TenantId::new(SITE).unwrap()
}

fn silver() -> WorkflowId {
    WorkflowId::new("badge_milestones__silver").unwrap()
}

impl Backend {
    async fn call(
        &self,
        session: Option<&str>,
        path: &str,
        body: Json,
        key: Option<&str>,
    ) -> (u16, Json) {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header(HEADER_SERVICE, TOKEN)
            .header(HEADER_SITE, SITE)
            .header(HEADER_SOURCE, SOURCE)
            .header(HEADER_CORRELATION, "t-badge-milestones")
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

    async fn login(&self, login: &str) -> String {
        let (status, reply) = self
            .call(
                None,
                "/internal/session/login",
                json!({ "login": login, "password": format!("strata-dev-{login}") }),
                None,
            )
            .await;
        assert_eq!(status, 200, "{reply}");
        reply["token"].as_str().unwrap().to_string()
    }

    /// Post `amount` points to the account of `person` as the owner.
    async fn post_points(&self, owner: &str, person: &str, amount: i64, key: &str) -> Json {
        let (status, reply) = self
            .call(
                Some(owner),
                "/internal/ledger/post",
                json!({
                    "owner": { "principal": { "principal": person } },
                    "unit": UNIT,
                    "amount": amount,
                    "idempotency_key": key,
                    "reason": "test"
                }),
                None,
            )
            .await;
        assert_eq!(status, 200, "{reply}");
        reply
    }

    /// The badges the reader sees on the view of the product, as `(person, badge)` pairs.
    async fn badges(&self, session: &str) -> Vec<(String, String)> {
        let (status, view) = self
            .call(
                Some(session),
                "/internal/view",
                json!({ "path": "/badges" }),
                None,
            )
            .await;
        assert_eq!(status, 200, "{view}");
        view["sections"]["awards"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row["fields"]["person"]["Str"].as_str().unwrap().to_string(),
                    row["fields"]["badge"]["Str"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    /// Pass the worker until the runs of the automation are in the state `done` accepts.
    async fn work_until(&self, done: impl Fn(&[RunRecord]) -> bool) -> Vec<RunRecord> {
        let config = WorkerConfig::from_limits(&Default::default());
        for _ in 0..200 {
            worker::tick(&self.app, "t-worker", &config, &|| Timestamp(now_ms()));
            let runs = self
                .store
                .automation_runs(&tenant(), Some(&silver()), None, None, 50)
                .unwrap();
            if done(&runs) {
                return runs;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("the worker did not bring the runs to the state the test waits for");
    }

    /// The notices in the inbox of the reader, as `(template, text)` pairs.
    async fn notices(&self, session: &str) -> Vec<(String, String)> {
        let (status, reply) = self
            .call(Some(session), "/internal/inbox/list", json!({}), None)
            .await;
        assert_eq!(status, 200, "{reply}");
        reply["notices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|notice| {
                let body: Json = serde_json::from_str(notice["body"].as_str().unwrap()).unwrap();
                (
                    notice["kind"].as_str().unwrap().to_string(),
                    body["text"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    /// The tenant declares the mail category the mail steps of the product name.
    fn declare_the_mail_category(&self) {
        self.store
            .doc_put(
                &tenant(),
                strata_backend::dispatch::store::CATEGORIES,
                "transactional",
                &json!({
                    "category": "transactional",
                    "purpose": "transactional",
                    "name": "Transactional",
                    "version": 1
                })
                .to_string(),
                false,
            )
            .unwrap();
    }

    fn silver_start_jobs(&self) -> usize {
        self.store
            .jobs_list(&tenant(), None, 500)
            .unwrap()
            .iter()
            .filter(|job| matches!(&job.work, JobWork::StartRun { automation, .. } if *automation == silver()))
            .count()
    }

    fn awards(&self) -> Vec<(String, String)> {
        let state = self.store.load_complete(&tenant()).unwrap();
        let tenant = state.tenant.expect("the tenant exists");
        tenant
            .records_of(&strata_kernel::ObjectTypeId::new("badge_milestones__award").unwrap())
            .map(|record| {
                let text =
                    |field: &str| match record.get(&strata_kernel::FieldId::new(field).unwrap()) {
                        strata_kernel::Value::Str(text) => text.clone(),
                        other => panic!("{field}: {other:?}"),
                    };
                (text("person"), text("badge"))
            })
            .collect()
    }
}

fn admin_context(correlation: &str) -> Context {
    Context {
        tenant: tenant(),
        principal: Principal::User(PrincipalId::new("user:admin").unwrap()),
        session: None,
        now: Timestamp(now_ms()),
        correlation: correlation.into(),
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
    }
}

/// The owner defines the role `member`, which the policies of the product name, and gives it to
/// the people the test reads as.
fn give_the_member_role(backend: &Backend, people: &[&str]) {
    let run = |intent: Intent| {
        execute(backend.store.as_ref(), &intent, &admin_context("t-members"))
            .unwrap_or_else(|e| panic!("the owner's change was refused: {e}"));
    };
    run(Intent::ChangeSchema {
        changes: vec![SchemaChange::DefineRole(RoleDef {
            id: RoleId::new("member").unwrap(),
            label: "Member".into(),
        })],
        expected_version: None,
        existing: ExistingData::Reject,
    });
    for person in people {
        run(Intent::AssignRole {
            principal: PrincipalId::new(format!("user:{person}")).unwrap(),
            role: RoleId::new("member").unwrap(),
        });
    }
}

fn assign(backend: &Backend, person: &str, role: &str) {
    execute(
        backend.store.as_ref(),
        &Intent::AssignRole {
            principal: PrincipalId::new(format!("user:{person}")).unwrap(),
            role: RoleId::new(role).unwrap(),
        },
        &admin_context("t-role"),
    )
    .unwrap_or_else(|e| panic!("the role was refused: {e}"));
}

/// The steps before the wait completed, in order, and the run is waiting for its seven days.
fn assert_waiting_after_the_mail(run: &RunRecord, steps: &[StepRecord]) {
    let done: Vec<&str> = steps
        .iter()
        .filter(|step| step.state == StepState::Completed)
        .map(|step| step.step.as_str())
        .collect();
    assert_eq!(
        done.iter()
            .take(BEFORE_THE_WAIT.len())
            .copied()
            .collect::<Vec<_>>(),
        BEFORE_THE_WAIT,
        "steps: {steps:?}, run error: {:?}",
        run.error
    );
    assert!(steps.iter().all(|step| step.state != StepState::Failed));
    assert_eq!(run.state, RunState::Waiting, "{:?}", run.error);
}

// Verifies: INT-2902
// Verifies: WFL-2901
#[tokio::test]
async fn points_that_cross_the_threshold_award_the_badge_once() {
    let backend = start().await;
    give_the_member_role(&backend, &["mirela", "tomasz"]);
    let owner = backend.login("admin").await;

    // The owner installs the product from its bundle: one schema change, with the trigger, the
    // automation, the record type, the ledger unit and the templates it owns.
    let bundle: Json = serde_json::from_str(BADGE_MILESTONES).unwrap();
    let (status, installed) = backend
        .call(
            Some(&owner),
            "/internal/products/install",
            json!({ "bundle": bundle }),
            Some("install-badges"),
        )
        .await;
    assert_eq!(status, 200, "{installed}");
    backend.declare_the_mail_category();

    // Sessions are made once the roles are held.
    let mirela = backend.login("mirela").await;
    let tomasz = backend.login("tomasz").await;
    assert!(backend.badges(&mirela).await.is_empty());
    assert!(backend.notices(&mirela).await.is_empty());

    // 300 points are below the threshold: no run, no award.
    backend.post_points(&owner, "user:mirela", 300, "p1").await;
    assert_eq!(backend.silver_start_jobs(), 0);
    assert!(backend.awards().is_empty());

    // 250 more take the balance from 300 to 550: it crosses 500, and one run starts.
    backend.post_points(&owner, "user:mirela", 250, "p2").await;
    let runs = backend
        .work_until(|runs| runs.len() == 1 && runs[0].state != RunState::Running)
        .await;
    let run = &runs[0];
    assert_eq!(run.automation, silver());
    assert_eq!(
        run.params["owner"],
        strata_kernel::Value::Str("user:mirela".into())
    );
    assert_eq!(run.params["balance_before"], strata_kernel::Value::Int(300));
    assert_eq!(run.params["balance"], strata_kernel::Value::Int(550));
    let steps = backend
        .store
        .automation_steps(&tenant(), &run.run, None, 100)
        .unwrap();
    assert_eq!(steps[0].step.as_str(), "award");
    assert_eq!(steps[0].state, StepState::Completed);
    assert_waiting_after_the_mail(run, &steps);

    // The award exists, once, and the person reads it through the API; another person reads none.
    assert_eq!(
        backend.awards(),
        vec![("user:mirela".to_string(), "silver".to_string())]
    );
    assert_eq!(
        backend.badges(&mirela).await,
        vec![("user:mirela".to_string(), "silver".to_string())]
    );
    assert!(backend.badges(&tomasz).await.is_empty());
    // The person is told in their own inbox, once, and nobody else is (CT-183).
    assert_eq!(
        backend.notices(&mirela).await,
        vec![(
            "badge_milestones__silver_notice".to_string(),
            "Your points reached 500.".to_string()
        )]
    );
    assert!(backend.notices(&tomasz).await.is_empty());

    // The balance falls to 450 and crosses 500 again: the trigger fires again, and the run key,
    // which is the person, makes it the same run: no second job, no second award (SCH-1536).
    backend.post_points(&owner, "user:mirela", -100, "p3").await;
    backend.post_points(&owner, "user:mirela", 100, "p4").await;
    let runs = backend.work_until(|runs| runs.len() == 1).await;
    assert_eq!(runs.len(), 1);
    assert_eq!(backend.silver_start_jobs(), 1);
    assert_eq!(backend.awards().len(), 1);
    assert_eq!(backend.badges(&mirela).await.len(), 1);
    assert_eq!(backend.notices(&mirela).await.len(), 1, "no second notice");

    // Another person's crossing is another run and another award.
    backend.post_points(&owner, "user:tomasz", 700, "p5").await;
    let runs = backend
        .work_until(|runs| runs.len() == 2 && runs.iter().all(|r| r.state != RunState::Running))
        .await;
    assert_eq!(runs.len(), 2);
    assert_eq!(backend.notices(&tomasz).await.len(), 1);
    assert_eq!(backend.notices(&mirela).await.len(), 1);
    assert_eq!(backend.awards().len(), 2);
    assert_eq!(
        backend.badges(&tomasz).await,
        vec![("user:tomasz".to_string(), "silver".to_string())]
    );
    assert_eq!(backend.badges(&mirela).await.len(), 1);
}

// Verifies: INT-2902
#[tokio::test]
async fn a_holder_of_the_awarder_role_awards_a_badge_by_hand_and_no_one_else_does() {
    let backend = start().await;
    give_the_member_role(&backend, &["mirela", "tomasz"]);
    let owner = backend.login("admin").await;
    let bundle: Json = serde_json::from_str(BADGE_MILESTONES).unwrap();
    let (status, installed) = backend
        .call(
            Some(&owner),
            "/internal/products/install",
            json!({ "bundle": bundle }),
            Some("install-badges"),
        )
        .await;
    assert_eq!(status, 200, "{installed}");
    // Ines is given the awarder role of the product.
    assign(&backend, "ines", AWARDER);
    let ines = backend.login("ines").await;
    let tomasz = backend.login("tomasz").await;
    let mirela = backend.login("mirela").await;

    let invoke = |session: String, key: &'static str| {
        let backend = &backend;
        async move {
            backend
                .call(
                    Some(&session),
                    "/internal/operations/invoke",
                    json!({
                        "operation": "badge_milestones__award_by_hand",
                        "inputs": { "person": "user:mirela", "badge": "gold" }
                    }),
                    Some(key),
                )
                .await
        }
    };
    // A person who does not hold the role is refused, and nothing is written.
    let (status, reply) = invoke(tomasz.clone(), "by-hand-1").await;
    assert_eq!(status, 403, "{reply}");
    assert!(backend.awards().is_empty());
    // The holder awards it; a retry with the same key answers the same and writes nothing more.
    let (status, reply) = invoke(ines.clone(), "by-hand-2").await;
    assert_eq!(status, 200, "{reply}");
    let (status, reply) = invoke(ines, "by-hand-2").await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(
        backend.awards(),
        vec![("user:mirela".to_string(), "gold".to_string())]
    );
    assert_eq!(
        backend.badges(&mirela).await,
        vec![("user:mirela".to_string(), "gold".to_string())]
    );
    assert!(backend.badges(&tomasz).await.is_empty());
}

// Verifies: INT-2902
#[tokio::test]
async fn a_tenant_that_has_not_installed_the_product_has_none_of_it() {
    let backend = start().await;
    let owner = backend.login("admin").await;
    // Nothing is installed in fieldnotes: the view, the operation and the unit do not exist.
    let (status, _) = backend
        .call(
            Some(&owner),
            "/internal/view",
            json!({ "path": "/badges" }),
            None,
        )
        .await;
    assert_eq!(status, 404);
    let (status, _) = backend
        .call(
            Some(&owner),
            "/internal/operations/invoke",
            json!({ "operation": "badge_milestones__award_by_hand", "inputs": {} }),
            Some("none-1"),
        )
        .await;
    assert!(status == 404 || status == 422, "{status}");
    let state = backend.store.load(&tenant(), &[]).unwrap();
    let schema = &state.tenant.unwrap().schema;
    assert!(schema
        .objects
        .keys()
        .all(|id| !id.to_string().starts_with("badge_milestones__")));
    assert!(
        schema.views.is_empty()
            || schema
                .views
                .keys()
                .all(|id| !id.to_string().starts_with("badge_milestones__"))
    );
}

// Verifies: INT-2902
#[tokio::test]
async fn a_holder_of_the_awarder_role_defines_a_badge_and_no_one_else_does() {
    let backend = start().await;
    give_the_member_role(&backend, &["mirela", "tomasz"]);
    let owner = backend.login("admin").await;
    let bundle: Json = serde_json::from_str(BADGE_MILESTONES).unwrap();
    let (status, installed) = backend
        .call(
            Some(&owner),
            "/internal/products/install",
            json!({ "bundle": bundle }),
            Some("install-badges"),
        )
        .await;
    assert_eq!(status, 200, "{installed}");
    assign(&backend, "ines", AWARDER);
    let ines = backend.login("ines").await;
    let tomasz = backend.login("tomasz").await;

    let define = |session: String, key: &'static str, points: i64| {
        let backend = &backend;
        async move {
            backend
                .call(
                    Some(&session),
                    "/internal/operations/invoke",
                    json!({
                        "operation": "badge_milestones__define_badge",
                        "inputs": { "name": "silver", "label": "Silver", "points": points }
                    }),
                    Some(key),
                )
                .await
        }
    };
    let defined = || -> Vec<String> {
        let state = backend.store.load_complete(&tenant()).unwrap();
        let tenant = state.tenant.expect("the tenant exists");
        tenant
            .records_of(&strata_kernel::ObjectTypeId::new("badge_milestones__badge").unwrap())
            .map(
                |record| match record.get(&strata_kernel::FieldId::new("name").unwrap()) {
                    strata_kernel::Value::Str(text) => text.clone(),
                    other => panic!("name: {other:?}"),
                },
            )
            .collect()
    };
    // A person who does not hold the role is refused, and a badge needs positive points.
    let (status, reply) = define(tomasz, "define-1", 500).await;
    assert_eq!(status, 403, "{reply}");
    assert!(defined().is_empty());
    let (status, reply) = define(ines.clone(), "define-2", 0).await;
    assert!(status >= 400 && status != 403, "{status} {reply}");
    assert!(defined().is_empty());
    // The holder defines it, and a retry with the same key writes nothing more.
    let (status, reply) = define(ines.clone(), "define-3", 500).await;
    assert_eq!(status, 200, "{reply}");
    let (status, reply) = define(ines, "define-3", 500).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(defined(), vec!["silver".to_string()]);
}
