//! The limit store's first slice as a running backend and as library calls on the in-memory store:
//! the quotas of the specification counted at the one admission point from the generated table,
//! the plan and its allowances (stored bytes, jobs, mail, entitlements), feature flags, and what an
//! owner reads of the use of a plan. Each test builds its own backend or its own app.
//!
//! `fieldnotes` is the seeded tenant: `admin` owns it and `tomasz` is an author.

use serde_json::{json, Value as Json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strata_backend::app::{App, Config, Shared};
use strata_backend::quota::{self, MailClass, MailVerdict, Quantity};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_model::quotas::{self, QuotaOutcome};
use strata_protocol::api::{
    HEADER_CORRELATION, HEADER_SERVICE, HEADER_SESSION, HEADER_SITE, HEADER_SOURCE,
};
use strata_storage_api::admission::DAY_MS;
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::Store;

const TOKEN: &str = "test-service-token-0123456789abcdef";
const SITE: &str = "fieldnotes";

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
        mail: Arc::new(strata_backend::mail::DevSink::memory()),
        federation: Arc::new(strata_backend::signin::oidc::Federation::none()),
        factors: Default::default(),
        limits: Default::default(),
    }
}

/// An app over `store`, one process: its in-memory counters are its own.
fn app_on(store: Arc<MemoryStore>) -> Shared {
    App::new(config("127.0.0.1:0".parse().unwrap()), store)
}

fn tenant() -> TenantId {
    TenantId::new(SITE).unwrap()
}

// ---------------------------------------------------------------------------------------------
// A running backend
// ---------------------------------------------------------------------------------------------

struct Backend {
    base: String,
    http: reqwest::Client,
    store: Arc<MemoryStore>,
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

/// What a call answered: the status, the `Retry-After` header and the body.
struct Reply {
    status: u16,
    retry_after: Option<u64>,
    body: Json,
}

impl Backend {
    /// A call as the BFF makes it, from `source` when one is given.
    async fn call(
        &self,
        source: Option<&str>,
        session: Option<&str>,
        path: &str,
        body: Json,
    ) -> Reply {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header(HEADER_SERVICE, TOKEN)
            .header(HEADER_SITE, SITE)
            .header(HEADER_CORRELATION, "t-quota-1")
            .json(&body);
        if let Some(source) = source {
            request = request.header(HEADER_SOURCE, source);
        }
        if let Some(token) = session {
            request = request.header(HEADER_SESSION, token);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok());
        Reply {
            status,
            retry_after,
            body: response.json().await.unwrap_or(Json::Null),
        }
    }

    /// A call of the platform operator: the service token, no site.
    async fn platform(&self, path: &str, body: Json) -> Reply {
        let response = self
            .http
            .post(format!("{}{path}", self.base))
            .header(HEADER_SERVICE, TOKEN)
            .header(HEADER_CORRELATION, "t-quota-2")
            .json(&body)
            .send()
            .await
            .unwrap();
        Reply {
            status: response.status().as_u16(),
            retry_after: None,
            body: response.json().await.unwrap_or(Json::Null),
        }
    }

    async fn session_of(&self, name: &str) -> String {
        let reply = self
            .call(
                None,
                None,
                "/internal/session/login",
                json!({ "login": name, "password": format!("strata-dev-{name}") }),
            )
            .await;
        assert_eq!(reply.status, 200, "{}", reply.body);
        reply.body["token"].as_str().expect("a session").to_string()
    }

    /// Replace a tenant's value of an allowance, as the operator's operation does.
    fn allow(&self, quantity: &str, value: &str) {
        self.store
            .doc_put(&tenant(), quota::ALLOWANCE_KIND, quantity, value, false)
            .unwrap();
    }
}

// ---------------------------------------------------------------------------------------------
// Request quotas
// ---------------------------------------------------------------------------------------------

/// A window longer than a minute survives a restart; a window of a minute does not (it is counted
/// in the process).
// Verifies: BIL-2800
#[test]
fn a_restart_keeps_a_long_window_count() {
    let store = Arc::new(MemoryStore::new());
    let before = app_on(store.clone());
    strata_backend::seed::seed(&before).expect("the seeded tenant");
    let long = &quotas::Q_RECOVER_ACCOUNT;
    let short = &quotas::Q_AGGREGATE_RUN;
    assert!(long.window > quota::SHORT_WINDOW && short.window <= quota::SHORT_WINDOW);
    let now = 9_000_000;
    let site = tenant();
    for n in 0..long.limit {
        let outcome = before
            .quota
            .count(&before, long, "k", Some(&site), now + n as i64);
        assert!(matches!(outcome, QuotaOutcome::Allowed(_)));
    }
    for n in 0..short.limit {
        before
            .quota
            .count(&before, short, "k", Some(&site), now + n as i64);
    }

    let after = app_on(store);
    assert!(
        matches!(
            after
                .quota
                .count(&after, long, "k", Some(&site), now + 10_000),
            QuotaOutcome::Exceeded { .. }
        ),
        "the long window kept its count in the tenant's database"
    );
    assert!(
        matches!(
            after
                .quota
                .count(&after, short, "k", Some(&site), now + 10_000),
            QuotaOutcome::Allowed(_)
        ),
        "the short window is counted in the process and starts again"
    );
}

// ---------------------------------------------------------------------------------------------
// A tenant with a trigger that starts an automation for each note
// ---------------------------------------------------------------------------------------------

fn name(s: &str) -> StepName {
    StepName::new(s).unwrap()
}

/// `award`: one step that ends the run.
fn award() -> AutomationDef {
    AutomationDef {
        state: None,
        id: WorkflowId::new("award").unwrap(),
        label: "award".to_string(),
        run_as: RunAs {
            roles: BTreeSet::from([role("writer")]),
        },
        run_key: None,
        start: name("done"),
        steps: BTreeMap::from([(
            name("done"),
            AutomationStepDef {
                action: AutomationStep::End,
                next: None,
                retry: None,
                timeout: None,
                compensate: None,
            },
        )]),
        defined_by: None,
        defined_at: None,
        suspended_for: None,
    }
}

/// A tenant with the notes schema and a trigger that starts `award` for each note created.
fn install(app: &App, tenant: &str) {
    let store = app.store.as_ref();
    let owner = principal_id(&format!("{tenant}:owner"));
    let mut now = 100;
    let mut run = |principal: Principal, intent: Intent| {
        now += 1;
        strata_storage_api::execute(store, &intent, &ctx(tenant, principal, now))
            .expect("fixture intent");
    };
    run(
        Principal::System,
        Intent::CreateTenant {
            name: tenant.to_string(),
            owner: owner.clone(),
        },
    );
    let mut changes = notes_schema();
    changes.push(SchemaChange::DefineAutomation(award()));
    changes.push(SchemaChange::DefineTrigger(TriggerDef {
        id: TriggerId::new("on_note").unwrap(),
        on: TriggerEvent::RecordCreated {
            object: object("note"),
        },
        condition: None,
        automation: WorkflowId::new("award").unwrap(),
        paused: None,
        defined_by: None,
        defined_at: None,
    }));
    run(
        Principal::User(owner),
        Intent::ChangeSchema {
            changes,
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
}

// ---------------------------------------------------------------------------------------------
// Stored bytes
// ---------------------------------------------------------------------------------------------

fn post_fields(slug: &str) -> Json {
    json!({ "op": "create", "object": "post", "fields": { "title": "Over", "slug": slug, "body": "Text.", "author": 3 } })
}

/// A write over the stored-bytes allowance is refused naming the quota while a read and a delete
/// are served (BIL-151).
// Verifies: BIL-151
// Verifies: BIL-2801
#[tokio::test]
async fn a_write_over_the_storage_allowance_is_refused_while_a_delete_succeeds() {
    let backend = start().await;
    let admin = backend.session_of("admin").await;
    let made = backend
        .call(
            None,
            Some(&admin),
            "/internal/mutate",
            post_fields("kept-for-delete"),
        )
        .await;
    assert_eq!(made.status, 200, "{}", made.body);
    let id = made.body["id"].as_u64().expect("the record's id");

    backend.allow("stored-bytes", "1");
    let refused = backend
        .call(
            None,
            Some(&admin),
            "/internal/mutate",
            post_fields("one-too-many"),
        )
        .await;
    assert_eq!(refused.status, 403, "{}", refused.body);
    assert_eq!(refused.body["kind"], "QuotaExceeded");
    assert_eq!(refused.body["failed"][0]["rule"], "quota:stored-bytes");

    let read = backend
        .call(None, Some(&admin), "/internal/session", json!({}))
        .await;
    assert_eq!(read.status, 200, "a read is served: {}", read.body);
    let deleted = backend
        .call(
            None,
            Some(&admin),
            "/internal/mutate",
            json!({ "op": "delete", "object": "post", "id": id }),
        )
        .await;
    assert_eq!(deleted.status, 200, "a delete is served: {}", deleted.body);

    backend.allow("stored-bytes", "unlimited");
    let again = backend
        .call(
            None,
            Some(&admin),
            "/internal/mutate",
            post_fields("now-fits"),
        )
        .await;
    assert_eq!(again.status, 200, "{}", again.body);
}

// ---------------------------------------------------------------------------------------------
// Plans: usage, change, entitlements, mail
// ---------------------------------------------------------------------------------------------

/// An owner reads the plan and the use of each allowance; someone who is not an owner may not.
// Verifies: BIL-2802
#[tokio::test]
async fn an_owner_reads_the_plan_and_what_it_has_used() {
    let backend = start().await;
    let admin = backend.session_of("admin").await;
    let tomasz = backend.session_of("tomasz").await;
    let usage = backend
        .call(None, Some(&admin), "/internal/usage", json!({}))
        .await;
    assert_eq!(usage.status, 200, "{}", usage.body);
    assert_eq!(usage.body["plan"], "standard");
    let names: Vec<&str> = usage.body["quantities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| q["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec![
            "stored-bytes",
            "jobs-per-day",
            "jobs-per-minute",
            "segments",
            "mail-per-month",
            "stored-records"
        ]
    );
    let stored = &usage.body["quantities"][0];
    assert!(
        stored["used"].as_u64().unwrap() > 0,
        "the seeded tenant holds data"
    );
    assert_eq!(stored["allowance"], 10_000_000_000u64);
    assert_eq!(usage.body["quantities"][1]["allowance"], 10_000);

    let refused = backend
        .call(None, Some(&tomasz), "/internal/usage", json!({}))
        .await;
    assert_eq!(refused.status, 403, "{}", refused.body);
}

/// A bulk message past the month's allowance is held to the next month while sign-in mail and the
/// mail of another tenant leave (EML-021).
// Verifies: EML-021
#[test]
fn bulk_mail_past_the_monthly_allowance_is_held_and_sign_in_mail_is_not() {
    let app = app_on(Arc::new(MemoryStore::new()));
    install(&app, "alpha");
    install(&app, "beta");
    let (alpha, beta) = (tenant_id("alpha"), tenant_id("beta"));
    quota::set_allowance(&app, &alpha, Quantity::MailPerMonth, Some(2), "test", "t-4").unwrap();
    let now = 20_777 * DAY_MS + 3_600_000;
    assert_eq!(
        quota::mail_admit(&app, &alpha, MailClass::Bulk, now),
        MailVerdict::Send
    );
    quota::mail_sent(&app, &alpha, MailClass::Bulk, now, 2);
    assert_eq!(
        quota::mail_admit(&app, &alpha, MailClass::Bulk, now),
        MailVerdict::Hold {
            release_at_ms: quota::next_month_start(now)
        }
    );
    assert_eq!(
        quota::mail_admit(&app, &alpha, MailClass::SignIn, now),
        MailVerdict::Send
    );
    assert_eq!(
        quota::mail_admit(&app, &beta, MailClass::Bulk, now),
        MailVerdict::Send
    );
    assert_eq!(quota::mail_used(&app, &beta, MailClass::Bulk, now), 0);
    let next_month = quota::next_month_start(now) + 1;
    assert_eq!(
        quota::mail_admit(&app, &alpha, MailClass::Bulk, next_month),
        MailVerdict::Send,
        "a new month"
    );
}

// ---------------------------------------------------------------------------------------------
// Feature flags
// ---------------------------------------------------------------------------------------------

/// A flag that is off by default is on only when the release record enables it for the tenant and
/// the owner has turned the tenant's switch on; an owner can always turn it off (OPS-328, D-6202).
// Verifies: OPS-328
// Verifies: AGT-420
#[tokio::test]
async fn a_flag_off_by_default_is_on_only_when_the_release_and_the_switch_allow() {
    let backend = start().await;
    let admin = backend.session_of("admin").await;
    let tomasz = backend.session_of("tomasz").await;
    let flags = |session: String| {
        let backend = &backend;
        async move {
            backend
                .call(None, Some(&session), "/internal/flags", json!({}))
                .await
        }
    };
    let on = |reply: &Reply| reply.body["flags"][0]["on"].as_bool().unwrap();

    let read = flags(admin.clone()).await;
    assert_eq!(read.status, 200, "{}", read.body);
    assert_eq!(read.body["flags"][0]["name"], "assistant");
    assert!(!on(&read), "off by default");

    // The switch cannot be turned on before the release record enables the flag.
    let early = backend
        .call(
            None,
            Some(&admin),
            "/internal/flags/set",
            json!({ "name": "assistant", "value": "on" }),
        )
        .await;
    assert_eq!(early.status, 409, "{}", early.body);

    // The release record alone does not turn it on.
    let released = backend
        .platform(
            "/internal/platform/quota",
            json!({ "tenant": SITE, "name": "assistant-flag", "value": "on", "release": "r-1" }),
        )
        .await;
    assert_eq!(released.status, 200, "{}", released.body);
    assert!(!on(&flags(admin.clone()).await), "released, not switched");

    // Someone who is not an owner cannot turn it on, a name that is no tenant switch is invalid,
    // and so is a value that is not on or off.
    let author = backend
        .call(
            None,
            Some(&tomasz),
            "/internal/flags/set",
            json!({ "name": "assistant", "value": "on" }),
        )
        .await;
    assert_eq!(author.status, 403, "{}", author.body);
    let unknown = backend
        .call(
            None,
            Some(&admin),
            "/internal/flags/set",
            json!({ "name": "storage", "value": "on" }),
        )
        .await;
    assert_eq!(unknown.status, 422, "{}", unknown.body);
    let maybe = backend
        .call(
            None,
            Some(&admin),
            "/internal/flags/set",
            json!({ "name": "assistant", "value": "maybe" }),
        )
        .await;
    assert_eq!(maybe.status, 422, "{}", maybe.body);

    // Both allow: on.
    let switched = backend
        .call(
            None,
            Some(&admin),
            "/internal/flags/set",
            json!({ "name": "assistant", "value": "on" }),
        )
        .await;
    assert_eq!(switched.status, 200, "{}", switched.body);
    assert!(on(&flags(admin.clone()).await), "released and switched");

    // The release record withdrawing it turns it off whatever the switch says.
    let withdrawn = backend
        .platform(
            "/internal/platform/quota",
            json!({ "tenant": SITE, "name": "assistant-flag", "value": "off" }),
        )
        .await;
    assert_eq!(withdrawn.status, 200, "{}", withdrawn.body);
    assert!(
        !on(&flags(admin.clone()).await),
        "withdrawn by the release record"
    );

    // An owner can always turn the switch off.
    let off = backend
        .call(
            None,
            Some(&admin),
            "/internal/flags/set",
            json!({ "name": "assistant", "value": "off" }),
        )
        .await;
    assert_eq!(off.status, 200, "{}", off.body);
}
