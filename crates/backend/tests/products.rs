//! An owner installs, upgrades and removes a product through the backend routes (the contracts
//! install-bundle, upgrade-bundle, remove-bundle and list-installed-products): the bundle that
//! specc writes from `badge-milestones.jsonc` goes in as one schema change that also records which
//! product owns what, a dry run shows the projection and changes nothing, a retry changes nothing,
//! an upgrade applies the difference of the two versions, and a removal deletes exactly what the
//! product owns. Real sockets, the in-memory store.

use serde_json::{json, Value as Json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strata_backend::app::{Config, ThrottleLimits};
use strata_backend::mail::DevSink;
use strata_backend::signin::oidc::Federation;
use strata_kernel::{
    canon, Context, ExistingData, Intent, ObjectTypeId, Principal, PrincipalId, RoleDef, RoleId,
    SchemaChange, TenantId, Timestamp,
};
use strata_protocol::api::{
    HEADER_CORRELATION, HEADER_IDEMPOTENCY, HEADER_SERVICE, HEADER_SESSION, HEADER_SITE,
    HEADER_SOURCE,
};
use strata_protocol::log::now_ms;
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, Store};

const TOKEN: &str = "test-service-token-0123456789abcdef";
const SOURCE: &str = "203.0.113.7";
const SITE: &str = "fieldnotes";

/// The bundle specc writes for `badge-milestones.jsonc` (the golden file of its tests).
const BADGE_MILESTONES: &str = include_str!(
    "../../spec-compiler/tests/fixtures/products/expected/badge-milestones.bundle.json"
);

struct Backend {
    base: String,
    http: reqwest::Client,
    store: Arc<MemoryStore>,
}

async fn start() -> Backend {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store = Arc::new(MemoryStore::new());
    let cfg = Config {
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
    };
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
            .header(HEADER_CORRELATION, "t-corr-products")
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

    async fn login(&self, login: &str, password: &str) -> String {
        let (status, reply) = self
            .call(
                None,
                "/internal/session/login",
                json!({ "login": login, "password": password }),
                None,
            )
            .await;
        assert_eq!(status, 200, "{reply}");
        reply["token"].as_str().unwrap().to_string()
    }

    async fn owner(&self) -> String {
        self.login("admin", "strata-dev-admin").await
    }

    /// The tenant as the store holds it, without records.
    fn schema(&self) -> strata_kernel::TenantSchema {
        let state = self.store.load(&tenant(), &[]).unwrap();
        strata_kernel::TenantSchema::clone(&state.tenant.expect("the tenant exists").schema)
    }

    /// Make a session look as if it authenticated `ago_ms` ago.
    fn age(&self, token: &str, ago_ms: i64) {
        let mut session = self
            .store
            .session_by_token(&canon::hash_bytes(token.as_bytes()))
            .unwrap()
            .expect("the session exists");
        session.authenticated_at = Timestamp(session.authenticated_at.0 - ago_ms);
        self.store.put_session(&session).unwrap();
    }

    async fn install(
        &self,
        session: &str,
        bundle: &Json,
        extra: Json,
        key: Option<&str>,
    ) -> (u16, Json) {
        let mut body = json!({ "bundle": bundle });
        for (name, value) in extra.as_object().unwrap() {
            body[name] = value.clone();
        }
        self.call(Some(session), "/internal/products/install", body, key)
            .await
    }

    async fn upgrade(&self, session: &str, bundle: &Json, extra: Json) -> (u16, Json) {
        let mut body = json!({ "bundle": bundle });
        for (name, value) in extra.as_object().unwrap() {
            body[name] = value.clone();
        }
        self.call(Some(session), "/internal/products/upgrade", body, None)
            .await
    }

    async fn remove(
        &self,
        session: &str,
        product: &str,
        delete_records: bool,
        extra: Json,
    ) -> (u16, Json) {
        let mut body = json!({ "product": product, "delete_records": delete_records });
        for (name, value) in extra.as_object().unwrap() {
            body[name] = value.clone();
        }
        self.call(Some(session), "/internal/products/remove", body, None)
            .await
    }

    async fn installed(&self, session: &str) -> Json {
        let (status, reply) = self
            .call(Some(session), "/internal/products", json!({}), None)
            .await;
        assert_eq!(status, 200, "{reply}");
        reply["products"].clone()
    }
}

fn tenant() -> TenantId {
    TenantId::new(SITE).unwrap()
}

fn admin() -> Principal {
    Principal::User(PrincipalId::new("user:admin").unwrap())
}

/// Give the tenant the role `member`, which the policies of the product name.
fn give_the_tenant_a_member_role(backend: &Backend) {
    let context = Context {
        tenant: tenant(),
        principal: admin(),
        session: None,
        now: Timestamp(now_ms()),
        correlation: "t-member-role".into(),
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
    };
    execute(
        backend.store.as_ref(),
        &Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineRole(RoleDef {
                id: RoleId::new("member").unwrap(),
                label: "Member".into(),
            })],
            expected_version: None,
            existing: ExistingData::Reject,
        },
        &context,
    )
    .unwrap_or_else(|e| panic!("the owner's change was refused: {e}"));
}

/// The bundle of `badge-milestones` as specc writes it: its ledger unit is declared by the product,
/// in the same schema change as the trigger that names it.
fn badge_milestones() -> Json {
    serde_json::from_str(BADGE_MILESTONES).unwrap()
}

/// Version 1.1.0: the retention is a year, and an inbox notice more.
fn badge_milestones_1_1() -> Json {
    let mut bundle = badge_milestones();
    bundle["version"] = json!("1.1.0");
    bundle["content"]["objects"]["badge_milestones__award"]["retention"] =
        json!({ "days": 365, "from": "Created" });
    bundle["content"]["notice-templates"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "template": "badge_milestones__extra_notice",
            "title": "Another notice",
            "body": "[{\"type\":\"paragraph\",\"inlines\":[{\"type\":\"text\",\"text\":\"Another.\"}]}]"
        }));
    bundle
}

fn kinds(entities: &Json) -> Vec<String> {
    entities
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            format!(
                "{}:{}",
                e["kind"].as_str().unwrap(),
                e["id"].as_str().unwrap()
            )
        })
        .collect()
}

// Verifies: INT-1581
// Verifies: INT-1582
// Verifies: INT-1583
// Verifies: INT-2342
// Verifies: INT-2345
// Verifies: INT-2346
#[tokio::test]
async fn an_owner_installs_upgrades_and_removes_a_product() {
    let backend = start().await;
    give_the_tenant_a_member_role(&backend);
    let owner = backend.owner().await;
    let before = backend.schema();
    assert_eq!(backend.installed(&owner).await, json!([]));

    // A dry run answers the projection and changes nothing.
    let (status, dry) = backend
        .install(
            &owner,
            &badge_milestones(),
            json!({ "dry_run": true }),
            None,
        )
        .await;
    assert_eq!(status, 200, "{dry}");
    assert_eq!(dry["dry_run"], json!(true));
    assert_eq!(dry["schema_version"], json!(before.version.0));
    let created = kinds(&dry["projection"]["created"]);
    for wanted in [
        "object:badge_milestones__award",
        "mail-template:badge_milestones__silver_mail",
        "notice-template:badge_milestones__silver_notice",
        "trigger:badge_milestones__silver_on_points",
        "automation:badge_milestones__silver",
    ] {
        assert!(
            created.contains(&wanted.to_string()),
            "{wanted}: {created:?}"
        );
    }
    assert!(!dry["projection"]["summary"].as_array().unwrap().is_empty());
    assert_eq!(backend.schema(), before);
    assert_eq!(backend.installed(&owner).await, json!([]));

    // The install is one schema change, and the record of what the product owns is in it.
    let (status, first) = backend
        .install(&owner, &badge_milestones(), json!({}), Some("install-1"))
        .await;
    assert_eq!(status, 200, "{first}");
    assert_eq!(first["unchanged"], json!(false));
    let version = first["schema_version"].as_u64().unwrap();
    assert_eq!(version, before.version.0 + 1, "one schema change");
    let schema = backend.schema();
    assert_eq!(schema.version.0, version);
    let award = &schema.objects[&ObjectTypeId::new("badge_milestones__award").unwrap()];
    assert_eq!(award.retention.as_ref().map(|r| r.days), Some(730));
    // The personal declaration of the field is stored with it and read back.
    let person = award
        .fields
        .iter()
        .find(|(id, _)| id.as_str() == "person")
        .map(|(_, field)| field)
        .unwrap();
    assert!(person.personal.is_some());
    let products = backend.installed(&owner).await;
    assert_eq!(products[0]["product"], "badge-milestones");
    assert_eq!(products[0]["version"], "1.0.0");
    assert_eq!(products[0]["schema-version"], json!(version));
    let owns = kinds(&products[0]["owns"]);
    assert!(owns.contains(&"mail-template:badge_milestones__silver_mail".to_string()));

    // Installing the same product and version again, with or without the key, changes nothing and
    // answers the schema version of the first install.
    for key in [Some("install-1"), None] {
        let (status, again) = backend
            .install(&owner, &badge_milestones(), json!({}), key)
            .await;
        assert_eq!(status, 200, "{again}");
        assert_eq!(again["unchanged"], json!(true));
        assert_eq!(again["schema_version"], json!(version));
    }
    assert_eq!(backend.schema().version.0, version);

    // A record of the product's type.
    let (status, record) = backend
        .call(
            Some(&owner),
            "/internal/mutate",
            json!({ "op": "create", "object": "badge_milestones__award",
                    "fields": { "person": "user:ines", "badge": "silver", "awarded_at": "2026-01-01T00:00:00Z" } }),
            None,
        )
        .await;
    assert_eq!(status, 200, "{record}");

    // An upgrade applies the difference of the two versions, as one schema change.
    let (status, dry) = backend
        .upgrade(&owner, &badge_milestones_1_1(), json!({ "dry_run": true }))
        .await;
    assert_eq!(status, 200, "{dry}");
    assert_eq!(
        kinds(&dry["projection"]["created"]),
        vec!["notice-template:badge_milestones__extra_notice"]
    );
    assert_eq!(
        kinds(&dry["projection"]["changed"]),
        vec!["object:badge_milestones__award"]
    );
    assert_eq!(dry["projection"]["removed"], json!([]));
    let (status, upgraded) = backend
        .upgrade(
            &owner,
            &badge_milestones_1_1(),
            json!({ "expected_version": version }),
        )
        .await;
    assert_eq!(status, 200, "{upgraded}");
    assert_eq!(upgraded["schema_version"], json!(version + 1));
    let schema = backend.schema();
    let award = &schema.objects[&ObjectTypeId::new("badge_milestones__award").unwrap()];
    assert_eq!(award.retention.as_ref().map(|r| r.days), Some(365));
    assert_eq!(
        strata_kernel::templates::list(&schema)
            .iter()
            .filter(|t| t.kind == strata_kernel::EntityKind::NoticeTemplate)
            .count(),
        2
    );
    assert_eq!(backend.installed(&owner).await[0]["version"], "1.1.0");

    // A version that is not greater is refused, and so is an upgrade from a stale schema version.
    let (status, refused) = backend
        .upgrade(&owner, &badge_milestones_1_1(), json!({}))
        .await;
    assert_eq!((status, refused["kind"].as_str()), (422, Some("invalid")));
    let mut later = badge_milestones_1_1();
    later["version"] = json!("1.2.0");
    let (status, stale) = backend
        .upgrade(&owner, &later, json!({ "expected_version": 1 }))
        .await;
    assert_eq!(status, 409, "{stale}");
    assert_eq!(backend.installed(&owner).await[0]["version"], "1.1.0");

    // The product has a record, so a removal that does not say they are deleted is refused and
    // changes nothing.
    let schema_before = backend.schema();
    let (status, refused) = backend
        .remove(&owner, "badge-milestones", false, json!({}))
        .await;
    assert_eq!(status, 422, "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .contains("badge_milestones__award has 1 record(s)"),
        "{refused}"
    );
    assert_eq!(backend.schema(), schema_before);

    // A dry run of the removal that deletes them counts them; the removal deletes exactly what the
    // product owns.
    let (status, dry) = backend
        .remove(&owner, "badge-milestones", true, json!({ "dry_run": true }))
        .await;
    assert_eq!(status, 200, "{dry}");
    assert_eq!(dry["projection"]["records_deleted"], json!(1));
    let (status, removed) = backend
        .remove(&owner, "badge-milestones", true, json!({}))
        .await;
    assert_eq!(status, 200, "{removed}");
    assert_eq!(removed["projection"]["records_deleted"], json!(1));
    let schema = backend.schema();
    assert!(!schema
        .objects
        .contains_key(&ObjectTypeId::new("badge_milestones__award").unwrap()));
    assert!(schema.products.is_none() && schema.mail_templates.is_none());
    assert!(schema.triggers.is_none() && schema.automations.is_none());
    assert_eq!(backend.installed(&owner).await, json!([]));
    // What the tenant had before the product is as it was.
    let mut restored = schema.clone();
    restored.version = before.version;
    assert_eq!(restored, before);

    // Removing a product that is not installed changes nothing.
    let (status, none) = backend
        .remove(&owner, "badge-milestones", false, json!({}))
        .await;
    assert_eq!(status, 200, "{none}");
    assert_eq!(none["unchanged"], json!(true));
}

// Verifies: INT-2346
#[tokio::test]
async fn only_the_owner_installs_and_a_session_that_authenticated_long_ago_needs_a_fresh_factor() {
    let backend = start().await;
    give_the_tenant_a_member_role(&backend);
    let bundle = badge_milestones();

    // Nobody signed in, and a person who is not the owner.
    let (status, _) = backend
        .call(
            None,
            "/internal/products/install",
            json!({ "bundle": bundle }),
            None,
        )
        .await;
    assert_eq!(status, 401);
    let author = backend.login("tomasz", "strata-dev-tomasz").await;
    let (status, _) = backend.install(&author, &bundle, json!({}), None).await;
    assert_eq!(status, 403);
    let (status, _) = backend
        .call(Some(&author), "/internal/products", json!({}), None)
        .await;
    assert_eq!(status, 403);

    // A session that authenticated too long ago may look, not change.
    let owner = backend.owner().await;
    backend.age(&owner, 16 * 60_000);
    let (status, refused) = backend.install(&owner, &bundle, json!({}), None).await;
    assert_eq!(
        (status, refused["kind"].as_str()),
        (403, Some("step_up_required")),
        "{refused}"
    );
    let (status, dry) = backend
        .install(&owner, &bundle, json!({ "dry_run": true }), None)
        .await;
    assert_eq!(status, 200, "{dry}");
    assert!(backend.schema().products.is_none());

    // A bundle that is not a bundle, one with a section the kernel cannot take, and a release
    // outside the range of the bundle are refused before anything runs.
    let owner = backend.owner().await;
    let (status, bad) = backend
        .call(Some(&owner), "/internal/products/install", json!({}), None)
        .await;
    assert_eq!(status, 422, "{bad}");
    let (status, bad) = backend
        .install(&owner, &badge_milestones_with_a_segment(), json!({}), None)
        .await;
    assert_eq!(
        (status, bad["kind"].as_str()),
        (422, Some("invalid-bundle")),
        "{bad}"
    );
    assert!(
        bad["message"].as_str().unwrap().contains("\"segments\""),
        "{bad}"
    );
    let mut future = badge_milestones();
    future["platform"] = json!({ "min": "9.0" });
    let (status, bad) = backend.install(&owner, &future, json!({}), None).await;
    assert_eq!(
        (status, bad["kind"].as_str()),
        (422, Some("invalid")),
        "{bad}"
    );
    assert!(backend.schema().products.is_none());
}

/// The bundle with a segment, the one section that is no schema change and is refused.
fn badge_milestones_with_a_segment() -> Json {
    let mut bundle = badge_milestones();
    bundle["content"]["segments"] = json!({
        "badge_milestones__earners": {
            "name": "Earners",
            "subject-type": "badge_milestones__award",
            "predicate": "record.badge == 'silver'"
        }
    });
    bundle
}
