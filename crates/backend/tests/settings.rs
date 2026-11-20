//! Layered settings as a running service (D-5040 to D-5049): the routes that define a setting,
//! write its levels and read it, who may do each, the conflict of a stale write, what a person
//! can read of another, the service call of the dispatcher, and what the events carry. Real
//! sockets, the seeded `fieldnotes` tenant, the in-memory store the test keeps a handle to.
//!
//! In `fieldnotes` `admin` owns the tenant, `ines` is an editor, `tomasz` and `mirela` are authors,
//! and `dana` belongs to the team `newsdesk`.

use serde_json::{json, Value as Json};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strata_backend::app::Config;
use strata_kernel::{SettingDef, SettingId, SettingLevel, SettingType, TenantId, Value};
use strata_protocol::api::{HEADER_CORRELATION, HEADER_SERVICE, HEADER_SESSION, HEADER_SITE};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::Store;

const TOKEN: &str = "test-service-token-0123456789abcdef";
const SITE: &str = "fieldnotes";

struct Backend {
    base: String,
    http: reqwest::Client,
    store: Arc<MemoryStore>,
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
        mail: Arc::new(strata_backend::mail::DevSink::memory()),
        federation: Arc::new(strata_backend::signin::oidc::Federation::none()),
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

fn tenant() -> TenantId {
    TenantId::new(SITE).unwrap()
}

impl Backend {
    /// A call as the BFF makes it: the service token, the site, and the session when there is one.
    async fn call(&self, session: Option<&str>, path: &str, body: Json) -> (u16, Json) {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header(HEADER_SERVICE, TOKEN)
            .header(HEADER_SITE, SITE)
            .header(HEADER_CORRELATION, "t-settings-1")
            .json(&body);
        if let Some(token) = session {
            request = request.header(HEADER_SESSION, token);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Json::Null))
    }

    /// A call of a platform service: the service token and no session, and no site header.
    async fn call_as_service(&self, path: &str, body: Json) -> (u16, Json) {
        let response = self
            .http
            .post(format!("{}{path}", self.base))
            .header(HEADER_SERVICE, TOKEN)
            .header(HEADER_CORRELATION, "t-settings-2")
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Json::Null))
    }

    /// A password sign-in of a development login that must have yielded a session.
    async fn session_of(&self, name: &str) -> String {
        let (status, body) = self
            .call(
                None,
                "/internal/session/login",
                json!({ "login": name, "password": format!("strata-dev-{name}") }),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        body["token"]
            .as_str()
            .unwrap_or_else(|| panic!("no session: {body}"))
            .to_string()
    }

    fn schema_version(&self) -> u64 {
        self.store
            .load(&tenant(), &[])
            .unwrap()
            .tenant
            .expect("the tenant exists")
            .schema
            .version
            .0
    }

    /// Define a setting as the owner; the definition must be accepted.
    async fn define(&self, def: &SettingDef) {
        let admin = self.session_of("admin").await;
        let (status, body) = self
            .call(
                Some(&admin),
                "/internal/settings/save-definition",
                json!({
                    "definition": serde_json::to_value(def).unwrap(),
                    "expected_version": self.schema_version(),
                }),
            )
            .await;
        assert_eq!(status, 200, "{body}");
    }

    async fn resolve(&self, session: Option<&str>, setting: &str) -> (u16, Json) {
        self.call(
            session,
            "/internal/settings/resolve",
            json!({ "setting": setting }),
        )
        .await
    }
}

fn layered(id: &str, ty: &str, default: Value) -> SettingDef {
    SettingDef {
        id: SettingId::new(id).unwrap(),
        label: id.to_string(),
        r#type: SettingType::new(ty).unwrap(),
        default,
        levels: BTreeSet::from([
            SettingLevel::Tenant,
            SettingLevel::Team,
            SettingLevel::Person,
        ]),
        locked_by: None,
    }
}

fn page_size() -> SettingDef {
    layered("page_size", "i64", Value::Int(20))
}

// ---------------------------------------------------------------------------------------------

/// CT-300, SET-001, SET-003: defining a setting is a schema change that needs the capability
/// `setting-define`, which the owner holds, and it is applied against the version of the schema the
/// caller read.
#[tokio::test]
async fn a_setting_is_defined_by_the_owner_against_the_schema_version() {
    let backend = start().await;
    let tomasz = backend.session_of("tomasz").await;
    let admin = backend.session_of("admin").await;
    let version = backend.schema_version();
    let body = json!({
        "definition": serde_json::to_value(page_size()).unwrap(),
        "expected_version": version,
    });

    let (status, refused) = backend
        .call(
            Some(&tomasz),
            "/internal/settings/save-definition",
            body.clone(),
        )
        .await;
    assert_eq!(status, 403, "an author may not define settings: {refused}");
    let (status, _) = backend
        .call(None, "/internal/settings/save-definition", body.clone())
        .await;
    assert_eq!(status, 401, "a visitor may not");

    let (status, saved) = backend
        .call(
            Some(&admin),
            "/internal/settings/save-definition",
            body.clone(),
        )
        .await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(saved["version"], json!(version + 1));

    // The version the caller read is now stale: a conflict that changes nothing.
    let (status, stale) = backend
        .call(Some(&admin), "/internal/settings/save-definition", body)
        .await;
    assert_eq!(status, 409, "{stale}");
    assert_eq!(backend.schema_version(), version + 1);

    // A default that is not of the type is refused when the definition is made (SET-002).
    let mut bad = page_size();
    bad.default = Value::Str("many".into());
    let (status, invalid) = backend
        .call(
            Some(&admin),
            "/internal/settings/save-definition",
            json!({
                "definition": serde_json::to_value(bad).unwrap(),
                "expected_version": backend.schema_version(),
            }),
        )
        .await;
    assert_eq!(status, 422, "{invalid}");
}

/// SET-010 to SET-012, SET-031 to SET-034: every level has its own write capability, a person
/// writes only their own override, and a read answers the most specific level and names it.
#[tokio::test]
async fn each_level_is_written_by_whoever_may_and_a_read_names_the_level() {
    let backend = start().await;
    backend.define(&page_size()).await;
    let admin = backend.session_of("admin").await;
    let ines = backend.session_of("ines").await;
    let tomasz = backend.session_of("tomasz").await;
    let dana = backend.session_of("dana").await;
    let mirela = backend.session_of("mirela").await;

    // Nobody has overridden it: the default, and the answer says so.
    let (status, found) = backend.resolve(Some(&tomasz), "page_size").await;
    assert_eq!(status, 200, "{found}");
    assert_eq!(found["value"], json!(20));
    assert_eq!(found["source"], json!("default"));

    // The tenant level needs setting-write-tenant: an author and an editor do not hold it.
    let tenant_write = json!({ "setting": "page_size", "value": 30, "expected_version": 0 });
    for (who, session) in [("tomasz", &tomasz), ("ines", &ines)] {
        let (status, refused) = backend
            .call(
                Some(session),
                "/internal/settings/set-tenant",
                tenant_write.clone(),
            )
            .await;
        assert_eq!(status, 403, "{who}: {refused}");
    }
    let (status, wrote) = backend
        .call(Some(&admin), "/internal/settings/set-tenant", tenant_write)
        .await;
    assert_eq!(status, 200, "{wrote}");
    assert!(wrote["revision"].as_u64().unwrap() >= 2);
    assert!(wrote["event_seq"].as_u64().is_some());

    // The team level needs setting-write-team.
    let team_write = json!({
        "setting": "page_size", "team": "newsdesk", "value": 40, "expected_version": 0
    });
    let (status, refused) = backend
        .call(
            Some(&ines),
            "/internal/settings/set-team",
            team_write.clone(),
        )
        .await;
    assert_eq!(status, 403, "{refused}");
    let (status, wrote) = backend
        .call(Some(&admin), "/internal/settings/set-team", team_write)
        .await;
    assert_eq!(status, 200, "{wrote}");
    // A team that does not exist is refused as invalid.
    let (status, invalid) = backend
        .call(
            Some(&admin),
            "/internal/settings/set-team",
            json!({ "setting": "page_size", "team": "ghosts", "value": 1, "expected_version": 0 }),
        )
        .await;
    assert_eq!(status, 422, "{invalid}");

    // The person level is the caller's own: every signed-in person writes it, and the body names
    // no person.
    let person_write = json!({ "setting": "page_size", "value": 5, "expected_version": 0 });
    let (status, wrote) = backend
        .call(Some(&tomasz), "/internal/settings/set-person", person_write)
        .await;
    assert_eq!(status, 200, "{wrote}");
    let (status, _) = backend
        .call(
            None,
            "/internal/settings/set-person",
            json!({ "setting": "page_size", "value": 5, "expected_version": 0 }),
        )
        .await;
    assert_eq!(status, 401, "a visitor writes nobody's level");

    // Each reader is answered from the most specific level that reaches them.
    let (_, found) = backend.resolve(Some(&tomasz), "page_size").await;
    assert_eq!(
        (found["value"].clone(), found["source"].clone()),
        (json!(5), json!("person"))
    );
    let (_, found) = backend.resolve(Some(&dana), "page_size").await;
    assert_eq!(
        (found["value"].clone(), found["source"].clone()),
        (json!(40), json!("team"))
    );
    assert_eq!(found["team"], json!("newsdesk"));
    let (_, found) = backend.resolve(Some(&mirela), "page_size").await;
    assert_eq!(
        (found["value"].clone(), found["source"].clone()),
        (json!(30), json!("tenant"))
    );

    // A value of another type, and a setting that is not defined, are refused.
    let (status, invalid) = backend
        .call(
            Some(&mirela),
            "/internal/settings/set-person",
            json!({ "setting": "page_size", "value": "many", "expected_version": 0 }),
        )
        .await;
    assert_eq!(status, 422, "{invalid}");
    let (status, missing) = backend
        .call(
            Some(&mirela),
            "/internal/settings/set-person",
            json!({ "setting": "nothing", "value": 1, "expected_version": 0 }),
        )
        .await;
    assert_eq!(status, 404, "{missing}");
    let (status, _) = backend.resolve(Some(&mirela), "nothing").await;
    assert_eq!(status, 404);
    let (status, _) = backend.resolve(None, "page_size").await;
    assert_eq!(status, 401, "a visitor has no session to read for");

    // The list answers every setting for the caller (CT-302).
    let (status, all) = backend
        .call(Some(&tomasz), "/internal/settings/resolve-all", json!({}))
        .await;
    assert_eq!(status, 200, "{all}");
    assert_eq!(all["settings"].as_array().unwrap().len(), 1);
    assert_eq!(all["settings"][0]["value"], json!(5));
}

/// SET-025, D-5045: a write names the revision it read; another revision is a conflict that
/// changes nothing, and removing an override is a write without a value.
#[tokio::test]
async fn a_write_against_a_stale_revision_is_a_conflict_and_removing_falls_back() {
    let backend = start().await;
    backend.define(&page_size()).await;
    let tomasz = backend.session_of("tomasz").await;
    let write = |value: Option<i64>, expected: u64| {
        let mut body = json!({ "setting": "page_size", "expected_version": expected });
        if let Some(value) = value {
            body["value"] = json!(value);
        }
        body
    };

    let (status, first) = backend
        .call(
            Some(&tomasz),
            "/internal/settings/set-person",
            write(Some(5), 0),
        )
        .await;
    assert_eq!(status, 200, "{first}");
    let revision = first["revision"].as_u64().unwrap();
    let (status, stale) = backend
        .call(
            Some(&tomasz),
            "/internal/settings/set-person",
            write(Some(6), 0),
        )
        .await;
    assert_eq!(status, 409, "{stale}");
    let (_, found) = backend.resolve(Some(&tomasz), "page_size").await;
    assert_eq!(found["value"], json!(5), "the stale write changed nothing");

    // Removing it names the revision too, and the next read falls back to the default.
    let (status, removed) = backend
        .call(
            Some(&tomasz),
            "/internal/settings/set-person",
            write(None, revision),
        )
        .await;
    assert_eq!(status, 200, "{removed}");
    assert!(removed["revision"].as_u64().unwrap() > revision);
    let (_, found) = backend.resolve(Some(&tomasz), "page_size").await;
    assert_eq!(
        (found["value"].clone(), found["source"].clone()),
        (json!(20), json!("default"))
    );
    // The revision the first write made is not found again by a later write.
    let (status, replay) = backend
        .call(
            Some(&tomasz),
            "/internal/settings/set-person",
            write(Some(7), revision),
        )
        .await;
    assert_eq!(status, 409, "{replay}");
}

/// SET-1541, SET-1542, CT-816: no tenant principal reads the value of another person, whether it
/// asks for the list or for one setting; the dispatcher resolves a recipient's setting through the
/// service call, which takes no session.
#[tokio::test]
async fn no_person_reads_another_persons_value_and_a_service_resolves_for_a_recipient() {
    let backend = start().await;
    backend.define(&page_size()).await;
    let tomasz = backend.session_of("tomasz").await;
    let ines = backend.session_of("ines").await;
    let (status, wrote) = backend
        .call(
            Some(&tomasz),
            "/internal/settings/set-person",
            json!({ "setting": "page_size", "value": 5, "expected_version": 0 }),
        )
        .await;
    assert_eq!(status, 200, "{wrote}");

    // Ines asks for the setting, and for all of them, and is answered her own value.
    let (_, one) = backend.resolve(Some(&ines), "page_size").await;
    assert_eq!(one["value"], json!(20));
    let (_, all) = backend
        .call(Some(&ines), "/internal/settings/resolve-all", json!({}))
        .await;
    assert!(!all.to_string().contains("\"value\":5"), "{all}");
    // The request carries no person to ask for: a body that names one changes nothing.
    let (_, named) = backend
        .call(
            Some(&ines),
            "/internal/settings/resolve",
            json!({ "setting": "page_size", "principal": "user:tomasz" }),
        )
        .await;
    assert_eq!(named["value"], json!(20));

    // The service call answers what the recipient would be answered.
    let ask = json!({ "tenant": SITE, "principal": "user:tomasz", "setting": "page_size" });
    let (status, resolved) = backend
        .call_as_service("/internal/settings/resolve-for", ask.clone())
        .await;
    assert_eq!(status, 200, "{resolved}");
    assert_eq!(resolved["value"], json!(5));
    assert_eq!(resolved["source"], json!("person"));
    let (_, own) = backend.resolve(Some(&tomasz), "page_size").await;
    assert_eq!(resolved, own);

    // A request that carries a session is a tenant principal, possibly relayed by the BFF, and is
    // refused whoever it names.
    for session in [&ines, &tomasz] {
        let (status, refused) = backend
            .call(Some(session), "/internal/settings/resolve-for", ask.clone())
            .await;
        assert_eq!(status, 403, "{refused}");
        assert!(!refused.to_string().contains("\"value\""), "{refused}");
    }
    // A person who is not of the tenant, a setting that is not defined and a tenant that does not
    // exist are not found.
    for body in [
        json!({ "tenant": SITE, "principal": "user:nobody", "setting": "page_size" }),
        json!({ "tenant": SITE, "principal": "user:tomasz", "setting": "nothing" }),
        json!({ "tenant": "nowhere", "principal": "user:tomasz", "setting": "page_size" }),
    ] {
        let (status, missing) = backend
            .call_as_service("/internal/settings/resolve-for", body)
            .await;
        assert_eq!(status, 404, "{missing}");
    }
}

/// SET-040, SET-1543, D-5048: the event of a change names the setting, the level and the subject
/// and carries no value, so the journal and its subscribers never hold a preference.
#[tokio::test]
async fn the_event_of_a_change_carries_no_value() {
    let backend = start().await;
    backend
        .define(&layered("motto", "string", Value::Str(String::new())))
        .await;
    let tomasz = backend.session_of("tomasz").await;
    let admin = backend.session_of("admin").await;
    let secret = "hunter2-a-personal-motto";

    for (session, path, extra) in [
        (&tomasz, "/internal/settings/set-person", json!({})),
        (&admin, "/internal/settings/set-tenant", json!({})),
        (
            &admin,
            "/internal/settings/set-team",
            json!({ "team": "newsdesk" }),
        ),
    ] {
        let mut body = json!({ "setting": "motto", "value": secret, "expected_version": 0 });
        for (key, value) in extra.as_object().unwrap() {
            body[key] = value.clone();
        }
        let (status, wrote) = backend.call(Some(session), path, body).await;
        assert_eq!(status, 200, "{path}: {wrote}");
    }

    let events = backend.store.events_after(&tenant(), 0, 10_000).unwrap();
    let changed: Vec<_> = events
        .iter()
        .filter(|event| format!("{:?}", event.body).starts_with("SettingChanged"))
        .collect();
    // The definition and the three overrides.
    assert_eq!(changed.len(), 4, "{changed:?}");
    for event in &changed {
        let text = serde_json::to_string(event).unwrap();
        assert!(!text.contains(secret), "an event carries the value: {text}");
        assert!(text.contains("motto"), "{text}");
    }
    let everything = serde_json::to_string(&events).unwrap();
    assert!(
        !everything.contains(secret),
        "the journal of events holds a preference"
    );
    // The value is held where the kernel decides from it, in the state.
    let state = backend.store.load(&tenant(), &[]).unwrap().tenant.unwrap();
    assert!(serde_json::to_string(&state.setting_values)
        .unwrap()
        .contains(secret));
}

/// CT-303, SET-005, SET-025, SET-030: the platform operator writes the platform level through the
/// control plane's route, a tenant that has no override of its own reads it with its source named,
/// a stale write is a conflict naming the current revision, and no tenant session writes it. The
/// write is an audit entry of the platform.
#[tokio::test]
async fn the_platform_operator_writes_the_platform_level_and_a_tenant_without_an_override_reads_it()
{
    let backend = start().await;
    let mut def = page_size();
    def.levels.insert(SettingLevel::Platform);
    backend.define(&def).await;
    let tomasz = backend.session_of("tomasz").await;
    let admin = backend.session_of("admin").await;
    let path = "/internal/platform/settings/set";

    let (_, found) = backend.resolve(Some(&tomasz), "page_size").await;
    assert_eq!(
        (found["source"].clone(), found["platform_revision"].clone()),
        (json!("default"), json!(0))
    );

    // A tenant session, the owner's included, does not write the platform level.
    for session in [&tomasz, &admin] {
        let (status, refused) = backend
            .call(
                Some(session),
                path,
                json!({ "setting": "page_size", "value": 50, "expected_version": 0 }),
            )
            .await;
        assert_eq!(status, 403, "{refused}");
    }
    let (_, found) = backend.resolve(Some(&tomasz), "page_size").await;
    assert_eq!(found["source"], json!("default"), "nothing was written");

    // The operator's tools call it with the service credential and no session.
    let (status, written) = backend
        .call_as_service(
            path,
            json!({ "setting": "page_size", "value": 50, "expected_version": 0 }),
        )
        .await;
    assert_eq!(status, 200, "{written}");
    assert_eq!(written["revision"], json!(1));
    let (_, found) = backend.resolve(Some(&tomasz), "page_size").await;
    assert_eq!(
        (
            found["value"].clone(),
            found["source"].clone(),
            found["platform_revision"].clone()
        ),
        (json!(50), json!("platform"), json!(1))
    );
    let (_, all) = backend
        .call(Some(&tomasz), "/internal/settings/resolve-all", json!({}))
        .await;
    assert_eq!(all["settings"][0]["source"], json!("platform"), "{all}");

    // The tenant's own override is nearer than the platform's.
    let (status, set) = backend
        .call(
            Some(&admin),
            "/internal/settings/set-tenant",
            json!({ "setting": "page_size", "value": 35, "expected_version": 0 }),
        )
        .await;
    assert_eq!(status, 200, "{set}");
    let (_, found) = backend.resolve(Some(&tomasz), "page_size").await;
    assert_eq!(
        (found["value"].clone(), found["source"].clone()),
        (json!(35), json!("tenant"))
    );

    // A write against a stale revision changes nothing and names the current one.
    let (status, stale) = backend
        .call_as_service(
            path,
            json!({ "setting": "page_size", "value": 60, "expected_version": 0 }),
        )
        .await;
    assert_eq!(status, 409, "{stale}");
    assert!(
        stale.to_string().contains("revision 1"),
        "the conflict names the current revision: {stale}"
    );
    // A value that is not a flag, a whole number or text is refused.
    let (status, _) = backend
        .call_as_service(
            path,
            json!({ "setting": "page_size", "value": [1], "expected_version": 1 }),
        )
        .await;
    assert_eq!(status, 422);

    // Removing it falls back to the default for a tenant with no override of its own.
    let (status, removed) = backend
        .call_as_service(
            path,
            json!({ "setting": "page_size", "expected_version": 1 }),
        )
        .await;
    assert_eq!(status, 200, "{removed}");
    assert_eq!(removed["revision"], json!(2));
    let (_, found) = backend.resolve(Some(&tomasz), "page_size").await;
    assert_eq!(found["platform_revision"], json!(2));

    // Both writes are audit entries of the platform, holding the previous and the new value.
    let (status, diagnostics) = backend
        .call_as_service("/internal/platform/diagnostics", json!({}))
        .await;
    assert_eq!(status, 200, "{diagnostics}");
    let entries: Vec<&Json> = diagnostics["audit"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["event"] == "platform.setting_set")
        .collect();
    assert_eq!(entries.len(), 2, "{diagnostics}");
    assert_eq!(entries[0]["value"], json!(50));
    assert_eq!(entries[0]["previous"], Json::Null);
    assert_eq!(entries[1]["previous"], json!(50));
    assert_eq!(entries[1]["value"], Json::Null);
}
