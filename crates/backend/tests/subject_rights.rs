//! Subject rights through the backend: the export of the data of one person, the restriction of
//! processing, legal holds, the retention sweep, and the invitations the erasure reaches. Each
//! scenario runs on the in-memory store and on SQLite. Jobs run through `worker::tick` with the
//! clock of the app.

use serde_json::{json, Value as Json};
use std::collections::BTreeSet;
use std::sync::Arc;
use strata_backend::app::{App, Config, Shared};
use strata_backend::auth::{Caller, Identity};
use strata_backend::erasure::erase_subject;
use strata_backend::mail::DevSink;
use strata_backend::retention;
use strata_backend::signin::stepup;
use strata_backend::subject_export::{FORMAT, TTL_MS};
use strata_backend::subject_rights::{handle, Rights};
use strata_backend::worker::{self, WorkerConfig};
use strata_kernel::ext::TenantSchemaExt;
use strata_kernel::{
    Context, DataSubject, DeltaOp, ExistingData, FieldDef, FieldId, FieldType, Intent, Need,
    ObjectTypeDef, ObjectTypeId, Output, PersonalDeclaration, PersonalKind, Principal, PrincipalId,
    QueryDef, QueryId, RecordId, RetentionRule, RetentionStart, RoleId, SessionId, StateDelta,
    StateRevision, TenantId, TenantSchema, Timestamp, Transition, Value, ViewDef, ViewId,
    ViewSection,
};
use strata_protocol::api::ErrorCode;
use strata_protocol::ws::SessionState;
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{
    execute, Account, Commit, PasskeyRecord, SessionRecord, Store, TotpRecord,
};
use strata_storage_sqlite::SqliteStore;

const DAY: i64 = 86_400_000;

fn tenant(name: &str) -> TenantId {
    TenantId::new(name).expect("tenant id")
}

fn who(name: &str) -> PrincipalId {
    PrincipalId::new(name).expect("principal id")
}

fn app_on(store: Arc<dyn Store>) -> Shared {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cfg = Config {
        addr: "127.0.0.1:0".parse().expect("address"),
        service_token: "test-service-token-0123456789abcdef".into(),
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
    App::new(cfg, store)
}

fn ctx(site: &TenantId, principal: Principal, now: i64, correlation: &str) -> Context {
    Context {
        tenant: site.clone(),
        principal,
        session: None,
        now: Timestamp(now),
        correlation: correlation.to_string(),
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
    }
}

fn declared(
    id: &str,
    ty: FieldType,
    required: bool,
    personal: Option<DataSubject>,
) -> (FieldId, FieldDef) {
    let id = FieldId::new(id).expect("field id");
    (
        id.clone(),
        FieldDef {
            id: id.clone(),
            label: id.as_str().to_string(),
            ty,
            required,
            unique: false,
            default: None,
            read: None,
            personal: personal.map(|subject| PersonalDeclaration {
                kind: PersonalKind::Other,
                subject,
                searchable: true,
            }),
            normalise: None,
            computed: None,
        },
    )
}

fn record_type(
    id: &str,
    fields: Vec<(FieldId, FieldDef)>,
    retention: RetentionRule,
) -> ObjectTypeDef {
    ObjectTypeDef {
        searchable: None,
        append_only: None,
        id: ObjectTypeId::new(id).expect("object id"),
        label: id.to_string(),
        fields: fields.into_iter().collect(),
        constraints: Default::default(),
        retention: Some(retention),
        publication: None,
        language_field: None,
        translation_group: None,
        unique: None,
    }
}

/// A tenant with three record types: `profile` (its `name`, `bio` and `city` belong to the person
/// named by `person`; kept 30 days from creation), `diary` (its `entry` belongs to its creator;
/// kept 30 days from creation) and `ticket` (no personal field; kept 10 days from its `closed_at`
/// time, the retention that walks an index). The owner is `<name>:owner`.
fn found(store: &dyn Store, name: &str) -> TenantId {
    let site = tenant(name);
    let owner = who(&format!("{name}:owner"));
    execute(
        store,
        &Intent::CreateTenant {
            name: name.to_string(),
            owner: owner.clone(),
        },
        &ctx(&site, Principal::System, 1_000, "found"),
    )
    .expect("the tenant is created");
    let state = store
        .load_complete(&site)
        .expect("load")
        .tenant
        .expect("the tenant exists");
    let mut schema = TenantSchema::clone(&state.schema);
    let person = || {
        Some(DataSubject::Field {
            field: FieldId::new("person").expect("field id"),
        })
    };
    let thirty = || RetentionRule {
        days: 30,
        from: RetentionStart::Created,
    };
    let mut profile = record_type(
        "profile",
        vec![
            declared("person", FieldType::Str, false, None),
            declared("name", FieldType::Str, true, person()),
            declared("bio", FieldType::Str, false, person()),
            declared("city", FieldType::Str, false, person()),
            declared("plan", FieldType::Str, false, None),
        ],
        thirty(),
    );
    // The search index holds the personal `name` (declared searchable) and the plain `plan`.
    profile.searchable = Some(vec![
        FieldId::new("name").expect("field id"),
        FieldId::new("plan").expect("field id"),
    ]);
    schema
        .objects
        .insert(ObjectTypeId::new("profile").expect("object id"), profile);
    schema.objects.insert(
        ObjectTypeId::new("diary").expect("object id"),
        record_type(
            "diary",
            vec![
                declared("entry", FieldType::Str, true, Some(DataSubject::Creator)),
                declared("topic", FieldType::Str, false, None),
            ],
            thirty(),
        ),
    );
    schema.objects.insert(
        ObjectTypeId::new("ticket").expect("object id"),
        record_type(
            "ticket",
            vec![
                declared("title", FieldType::Str, false, None),
                declared("closed_at", FieldType::Time, false, None),
            ],
            RetentionRule {
                days: 10,
                from: RetentionStart::Field {
                    field: FieldId::new("closed_at").expect("field id"),
                },
            },
        ),
    );
    // A view that lists every profile, as a page of the tenant does.
    schema.queries.insert(
        QueryId::new("all_profiles").expect("query id"),
        QueryDef {
            id: QueryId::new("all_profiles").expect("query id"),
            object: ObjectTypeId::new("profile").expect("object id"),
            filter: None,
            params: Default::default(),
            order: vec![],
            limit: None,
            aggregates: None,
            declared_fields: None,
            tree: None,
        },
    );
    schema.views.insert(
        ViewId::new("profiles").expect("view id"),
        ViewDef {
            id: ViewId::new("profiles").expect("view id"),
            route: "/profiles".into(),
            title: "Profiles".into(),
            sections: [(
                "list".to_string(),
                ViewSection {
                    marks: None,
                    joins: None,
                    query: QueryId::new("all_profiles").expect("query id"),
                    args: Default::default(),
                    include: vec![],
                    markdown: vec![],
                    single: false,
                    aggregate: None,
                },
            )]
            .into(),
        },
    );
    assert!(schema.problems().is_empty(), "{:?}", schema.problems());
    let delta = StateDelta {
        tenant: site.clone(),
        base_revision: Some(state.revision),
        new_revision: StateRevision(state.revision.0 + 1),
        next_record: state.next_record,
        event_seq: state.event_seq,
        ops: vec![DeltaOp::PutSchema(Arc::new(schema))],
    };
    let transition = Transition {
        delta,
        events: Vec::new(),
        effects: Vec::new(),
        evidence: Vec::new(),
        output: Output::Member,
    };
    let intent = Intent::ChangeSchema {
        changes: Vec::new(),
        expected_version: None,
        existing: ExistingData::Reject,
    };
    store
        .commit(&Commit::of(
            &intent,
            &ctx(&site, Principal::System, 1_001, "schema"),
            &transition,
            store.generation(&site).unwrap(),
        ))
        .expect("the schema is committed");
    site
}

fn as_owner(app: &App, site: &TenantId) -> Context {
    ctx(
        site,
        Principal::User(who(&format!("{}:owner", site.as_str()))),
        app.now().0,
        "t-owner",
    )
}

fn create(store: &dyn Store, context: &Context, object: &str, fields: &[(&str, Value)]) {
    execute(
        store,
        &Intent::CreateRecord {
            object: ObjectTypeId::new(object).expect("object id"),
            fields: fields
                .iter()
                .map(|(k, v)| (FieldId::new(*k).expect("field id"), v.clone()))
                .collect(),
        },
        context,
    )
    .expect("the record is created");
}

/// A profile of `person`, created at `at` (so old or young).
fn profile_at(store: &dyn Store, site: &TenantId, at: i64, person: &str, name: &str) {
    let context = ctx(
        site,
        Principal::User(who(&format!("{}:owner", site.as_str()))),
        at,
        "t-seed",
    );
    create(
        store,
        &context,
        "profile",
        &[
            ("person", Value::Str(person.into())),
            ("name", Value::Str(name.into())),
            ("bio", Value::Str(format!("bio of {name}"))),
            ("city", Value::Str(format!("city of {name}"))),
            ("plan", Value::Str("free".into())),
        ],
    );
}

fn ticket_at(store: &dyn Store, site: &TenantId, now: i64, closed_at: i64) {
    let context = ctx(
        site,
        Principal::User(who(&format!("{}:owner", site.as_str()))),
        now,
        "t-seed",
    );
    create(
        store,
        &context,
        "ticket",
        &[
            ("title", Value::Str("a ticket".into())),
            ("closed_at", Value::Time(closed_at)),
        ],
    );
}

/// Everything that signs a person in or holds them outside the records.
fn give_credentials(store: &dyn Store, site: &TenantId, person: &PrincipalId, now: i64) {
    let login = person
        .as_str()
        .rsplit(':')
        .next()
        .expect("a login")
        .to_string();
    store
        .put_account(&Account {
            tenant: site.clone(),
            login,
            principal: person.clone(),
            display_name: "Ada Lovelace".into(),
            password_hash: "not-a-real-hash".into(),
            verified_email: Some("ada@example.org".into()),
        })
        .expect("account");
    store
        .put_passkey(&PasskeyRecord {
            tenant: site.clone(),
            principal: person.clone(),
            id: format!("pk-{}", person.as_str()),
            rp_id: "example.org".into(),
            label: "laptop".into(),
            credential: "{\"secret-credential\":1}".into(),
            created_at: Timestamp(now),
            last_used_at: None,
        })
        .expect("passkey");
    store
        .put_totp(&TotpRecord {
            tenant: site.clone(),
            principal: person.clone(),
            secret: "totp-secret-material".into(),
            confirmed: true,
            last_step: -1,
            created_at: Timestamp(now),
        })
        .expect("totp");
}

/// A signed-in identity of `principal`, whose last authentication is `age_ms` old.
fn identity(app: &App, site: &TenantId, principal: &str, age_ms: i64) -> Identity {
    let principal = who(principal);
    let now = app.now().0;
    Identity {
        tenant: site.clone(),
        principal: Principal::User(principal.clone()),
        session: Some(SessionRecord {
            id: SessionId::new(format!("s-{}-{}", site.as_str(), principal.as_str()))
                .expect("session id"),
            token_hash: "hash-of-the-session-token".into(),
            csrf_token: "csrf-token-value".into(),
            tenant: site.clone(),
            principal,
            display_name: "someone".into(),
            created_at: Timestamp(now - age_ms),
            last_seen_at: Timestamp(now),
            expires_at: Timestamp(i64::MAX),
            revoked_at: None,
            authenticated_at: Timestamp(now - age_ms),
            network: None,
        }),
        state: SessionState::Active,
        correlation: "t-rights".into(),
        verified_address: None,
        idempotency_key: None,
    }
}

fn anonymous(site: &TenantId) -> Identity {
    Identity {
        tenant: site.clone(),
        principal: Principal::Anonymous,
        session: None,
        state: SessionState::Anonymous,
        correlation: "t-anon".into(),
        verified_address: None,
        idempotency_key: None,
    }
}

fn caller(site: &TenantId, correlation: &str) -> Caller {
    Caller {
        site: site.clone(),
        token: None,
        correlation: correlation.to_string(),
        source: "127.0.0.1".into(),
        origin: None,
        idempotency_key: None,
        provisioning_token: None,
    }
}

/// Call one operation of subject rights as `identity`; the correlation names the request.
fn call(
    app: &App,
    who_is: &Identity,
    operation: &str,
    correlation: &str,
    body: Json,
) -> Result<Json, strata_backend::app::ApiError> {
    let mut identity = who_is.clone();
    identity.correlation = correlation.to_string();
    handle(
        app,
        &caller(&who_is.tenant, correlation),
        &identity,
        operation,
        &body,
    )
}

/// Run the due jobs until none is due, at the clock of the app plus `ahead_ms`.
fn run_jobs(app: &Shared, ahead_ms: i64) -> usize {
    let config = WorkerConfig::from_limits(&Default::default());
    let mut ran = 0;
    for _ in 0..500 {
        let clock = || Timestamp(app.now().0 + ahead_ms);
        let pass = worker::tick(app, "w1", &config, &clock);
        if pass.jobs == 0 {
            return ran;
        }
        ran += pass.jobs;
    }
    panic!("the jobs do not end");
}

fn error_of(result: Result<Json, strata_backend::app::ApiError>) -> strata_backend::app::ApiError {
    result.expect_err("refused")
}

fn record_exists(store: &dyn Store, site: &TenantId, object: &str, id: u64) -> bool {
    let object = ObjectTypeId::new(object).expect("object id");
    store
        .load(
            site,
            &[Need::Record {
                object: object.clone(),
                id: RecordId(id),
            }],
        )
        .expect("load")
        .tenant
        .expect("tenant")
        .records
        .get(&object)
        .is_some_and(|records| records.contains_key(&RecordId(id)))
}

fn field_names(document: &Json) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for record in document["records"].as_array().expect("records") {
        for (field, _) in record["fields"].as_object().expect("fields") {
            names.insert(format!(
                "{}.{}",
                record["object"].as_str().expect("object"),
                field
            ));
        }
    }
    names
}

/// Fetch a finished export as `who_is`.
fn fetched(
    app: &App,
    who_is: &Identity,
    export: &str,
    correlation: &str,
) -> Result<Json, strata_backend::app::ApiError> {
    call(
        app,
        who_is,
        "export-fetch",
        correlation,
        json!({ "export": export }),
    )
}

// ---------------------------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------------------------

fn the_export_holds_what_the_erasure_clears_and_goes_to_two_people_only(store: Arc<dyn Store>) {
    let app = app_on(store.clone());
    let store = store.as_ref();
    let alpha = found(store, "alpha");
    let beta = found(store, "beta");
    let ada = who("alpha:ada");
    let now = app.now().0;
    profile_at(store, &alpha, now, "alpha:ada", "Ada Lovelace");
    profile_at(store, &alpha, now, "alpha:grace", "Grace Hopper");
    // A diary Ada created (she is an owner too, so she may).
    execute(
        store,
        &Intent::AssignRole {
            principal: ada.clone(),
            role: RoleId::new("owner").expect("role"),
        },
        &as_owner(&app, &alpha),
    )
    .expect("role");
    create(
        store,
        &ctx(&alpha, Principal::User(ada.clone()), now, "t-ada"),
        "diary",
        &[
            ("entry", Value::Str("my secret day".into())),
            ("topic", Value::Str("weather".into())),
        ],
    );
    give_credentials(store, &alpha, &ada, now);
    // The other tenant holds a person of the same name.
    profile_at(store, &beta, now, "alpha:ada", "Ada Lovelace");
    let beta_before = store
        .tenant_summary(&beta)
        .expect("summary")
        .expect("tenant");

    // Ada asks for her own data; a second request gives the same document.
    let as_ada = identity(&app, &alpha, "alpha:ada", 0);
    let first = call(&app, &as_ada, "export", "t-req-1", json!({})).expect("requested");
    let second = call(&app, &as_ada, "export", "t-req-2", json!({})).expect("requested");
    assert_eq!(first["state"], "queued");
    let (first_id, second_id) = (
        first["export"].as_str().expect("id").to_string(),
        second["export"].as_str().expect("id").to_string(),
    );
    assert_ne!(first_id, second_id);
    // Nothing is delivered before the job has run.
    let early = fetched(&app, &as_ada, &first_id, "t-f0").expect("the state");
    assert_eq!(early["state"], "queued");
    assert!(early.get("document").is_none());
    assert!(run_jobs(&app, 0) >= 2);
    let mut document = fetched(&app, &as_ada, &first_id, "t-f1").expect("ready");
    assert_eq!(document["state"], "ready");
    let mut again = fetched(&app, &as_ada, &second_id, "t-f2").expect("ready");
    let (document, again) = (&mut document["document"], &mut again["document"]);
    assert_eq!(document["format"], FORMAT);
    document["generated_at"] = json!(0);
    again["generated_at"] = json!(0);
    assert_eq!(document, again, "the document is deterministic");

    // What is in it, and what is not.
    let text = document.to_string();
    assert_eq!(document["subject"], "alpha:ada");
    assert!(text.contains("Ada Lovelace") && text.contains("my secret day"));
    assert!(text.contains("ada@example.org") && text.contains("laptop"));
    assert_eq!(document["account"]["login"], "ada");
    assert_eq!(
        document["credentials"]["passkeys"]
            .as_array()
            .expect("passkeys")
            .len(),
        1
    );
    assert_eq!(document["credentials"]["authenticator"]["confirmed"], true);
    assert!(document["membership"]["roles"]
        .as_array()
        .expect("roles")
        .contains(&json!("owner")));
    for secret in [
        "not-a-real-hash",
        "secret-credential",
        "totp-secret-material",
        "hash-of-the-session-token",
        "csrf-token-value",
        "Grace Hopper",
        "bio of Grace",
    ] {
        assert!(!text.contains(secret), "the export holds {secret:?}");
    }
    assert!(document["inbox"].is_null());
    let exported = field_names(document);
    assert!(exported.contains("profile.name") && exported.contains("diary.entry"));

    // Who may fetch it: nobody else, and an owner only with a fresh step-up.
    let grace_id = identity(&app, &alpha, "alpha:grace", 0);
    let refused = error_of(fetched(&app, &grace_id, &first_id, "t-f3"));
    assert!(matches!(refused.0.code, ErrorCode::Forbidden));
    let refused = error_of(call(
        &app,
        &grace_id,
        "export",
        "t-req-3",
        json!({ "principal": "alpha:ada" }),
    ));
    assert!(matches!(refused.0.code, ErrorCode::Forbidden));
    let refused = error_of(fetched(&app, &anonymous(&alpha), &first_id, "t-f4"));
    assert!(matches!(refused.0.code, ErrorCode::Unauthenticated));
    let stale_owner = identity(&app, &alpha, "alpha:owner", 3_600_000);
    let refused = error_of(fetched(&app, &stale_owner, &first_id, "t-f5"));
    assert_eq!(refused.0.kind, stepup::required().0.kind);
    let stale_ada = identity(&app, &alpha, "alpha:ada", 3_600_000);
    let refused = error_of(fetched(&app, &stale_ada, &first_id, "t-f6"));
    assert_eq!(refused.0.kind, stepup::required().0.kind);
    let owner = identity(&app, &alpha, "alpha:owner", 0);
    let delivered = fetched(&app, &owner, &first_id, "t-f7").expect("an owner with step-up");
    assert_eq!(delivered["state"], "ready");
    // An owner of another tenant finds nothing, and the other tenant is as it was.
    let beta_owner = identity(&app, &beta, "beta:owner", 0);
    let refused = error_of(fetched(&app, &beta_owner, &first_id, "t-f8"));
    assert!(matches!(refused.0.code, ErrorCode::NotFound));
    assert_eq!(
        store
            .tenant_summary(&beta)
            .expect("summary")
            .expect("tenant"),
        beta_before
    );
    // Both the request and the fetches are audit entries.
    let audit = store.audit_list(&alpha, None, 1_000).expect("audit");
    assert!(audit
        .iter()
        .any(|entry| entry.event == "subject_export.requested"));
    assert!(audit
        .iter()
        .any(|entry| entry.event == "subject_export.fetched"));
    // The request names the tenant and the export, not the person.
    assert!(audit
        .iter()
        .filter(|entry| entry.event == "subject_export.requested")
        .all(|entry| !entry.data.contains("ada")));

    // The result expires.
    app.clock_skew_ms
        .store(TTL_MS + DAY, std::sync::atomic::Ordering::Relaxed);
    let owner = identity(&app, &alpha, "alpha:owner", 0);
    let gone = error_of(fetched(&app, &owner, &first_id, "t-f9"));
    assert!(matches!(gone.0.code, ErrorCode::NotFound));

    // The export and the erasure read the same fields. The erasure removes the exports of the
    // subject with the data, so it comes after the checks that fetch one (PRV-2741).
    let receipt = erase_subject(&app, &as_owner(&app, &alpha), &ada).expect("the erasure");
    let cleared: BTreeSet<String> = receipt
        .changed
        .iter()
        .flat_map(|record| {
            record
                .fields
                .iter()
                .map(move |field| format!("{}.{}", record.object, field))
        })
        .collect();
    assert_eq!(exported, cleared);
}

// Verifies: PRV-350
// Verifies: PRV-1586
// Verifies: PRV-2740
// Verifies: PRV-2741
#[test]
fn the_export_on_sqlite() {
    let store = SqliteStore::open_in_memory(Timestamp(1_000)).expect("a store");
    the_export_holds_what_the_erasure_clears_and_goes_to_two_people_only(Arc::new(store));
}

// ---------------------------------------------------------------------------------------------
// Restrictions
// ---------------------------------------------------------------------------------------------

// Verifies: PRV-352
// Verifies: PRV-2463
// Verifies: PRV-2742
#[test]
fn erasing_a_restricted_person_reports_the_restriction_and_ends_it() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
    let app = app_on(store.clone());
    let store = store.as_ref();
    let alpha = found(store, "alpha");
    profile_at(store, &alpha, app.now().0, "alpha:ada", "Ada Lovelace");
    give_credentials(store, &alpha, &who("alpha:ada"), 2_002);
    let owner = identity(&app, &alpha, "alpha:owner", 0);
    call(
        &app,
        &owner,
        "restrict",
        "t-r1",
        json!({ "principal": "alpha:ada", "reason": "an objection" }),
    )
    .expect("restricted");
    let receipt = erase_subject(&app, &as_owner(&app, &alpha), &who("alpha:ada")).expect("erased");
    assert!(receipt.reached.iter().any(|line| line
        .contains("restriction of processing (PRV-352): the principal was restricted since")));
    let rights = Rights::load_restrictions(&app, &alpha).expect("rights");
    assert!(rights.restriction_of(&who("alpha:ada")).is_none());
}

// ---------------------------------------------------------------------------------------------
// Legal holds
// ---------------------------------------------------------------------------------------------

// Verifies: PRV-355
// Verifies: PRV-2743
#[test]
fn a_held_subject_is_refused_with_the_hold_and_released_it_is_erased() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
    let app = app_on(store.clone());
    let store = store.as_ref();
    let alpha = found(store, "alpha");
    profile_at(store, &alpha, app.now().0, "alpha:ada", "Ada Lovelace");
    give_credentials(store, &alpha, &who("alpha:ada"), 2_002);
    let owner = identity(&app, &alpha, "alpha:owner", 0);

    let stranger = identity(&app, &alpha, "alpha:grace", 0);
    let body = json!({ "subject": "alpha:ada", "reason": "litigation 2026-11" });
    assert!(matches!(
        error_of(call(&app, &stranger, "place-hold", "t-h0", body.clone()))
            .0
            .code,
        ErrorCode::Forbidden
    ));
    let stale = identity(&app, &alpha, "alpha:owner", 3_600_000);
    assert_eq!(
        error_of(call(&app, &stale, "place-hold", "t-h1", body.clone()))
            .0
            .kind,
        stepup::required().0.kind
    );
    assert!(matches!(
        error_of(call(
            &app,
            &owner,
            "place-hold",
            "t-h2",
            json!({ "reason": "none named" })
        ))
        .0
        .code,
        ErrorCode::Invalid
    ));
    let placed = call(&app, &owner, "place-hold", "t-h3", body).expect("placed");
    let hold = placed["hold"]["id"].as_str().expect("hold id").to_string();
    let listed = call(&app, &owner, "holds", "t-h4", json!({})).expect("listed");
    assert_eq!(listed["holds"].as_array().expect("holds").len(), 1);
    assert!(matches!(
        error_of(call(&app, &stranger, "holds", "t-h4b", json!({})))
            .0
            .code,
        ErrorCode::Forbidden
    ));

    // The erasure is refused with `conflict`, names the hold, and changes nothing.
    let before = store
        .tenant_summary(&alpha)
        .expect("summary")
        .expect("tenant");
    let refused =
        erase_subject(&app, &as_owner(&app, &alpha), &who("alpha:ada")).expect_err("held");
    assert!(matches!(refused.0.code, ErrorCode::Conflict));
    assert!(refused.0.message.contains(&hold), "{}", refused.0.message);
    assert_eq!(
        store
            .tenant_summary(&alpha)
            .expect("summary")
            .expect("tenant"),
        before
    );
    assert!(store
        .account_of(&alpha, &who("alpha:ada"))
        .expect("account")
        .is_some());

    // Released, the same request erases; the hold stays listed as released.
    call(
        &app,
        &owner,
        "release-hold",
        "t-h5",
        json!({ "hold": hold }),
    )
    .expect("released");
    let listed = call(&app, &owner, "holds", "t-h6", json!({})).expect("listed");
    assert!(listed["holds"][0]["released_at"].is_number());
    erase_subject(&app, &as_owner(&app, &alpha), &who("alpha:ada")).expect("erased");
    assert!(store
        .account_of(&alpha, &who("alpha:ada"))
        .expect("account")
        .is_none());
}

// ---------------------------------------------------------------------------------------------
// Retention
// ---------------------------------------------------------------------------------------------

fn retention_deletes_what_is_past_its_period_and_nothing_else(store: Arc<dyn Store>) {
    let app = app_on(store.clone());
    let store = store.as_ref();
    let alpha = found(store, "alpha");
    let beta = found(store, "beta");
    let now = app.now().0;
    // Old and young profiles (created 100 days ago, and now), and tickets whose closing time, not
    // their creation, decides (closed 20 days ago and 2 days ago).
    profile_at(store, &alpha, now - 100 * DAY, "alpha:ada", "Ada Lovelace");
    profile_at(
        store,
        &alpha,
        now - 100 * DAY,
        "alpha:grace",
        "Grace Hopper",
    );
    profile_at(store, &alpha, now, "alpha:ada", "Ada Young");
    ticket_at(store, &alpha, now, now - 20 * DAY);
    ticket_at(store, &alpha, now, now - 2 * DAY);
    // The other tenant has old records too, and no sweep.
    profile_at(store, &beta, now - 100 * DAY, "beta:ada", "Beta Person");
    let beta_before = store
        .tenant_summary(&beta)
        .expect("summary")
        .expect("tenant");
    let seq_before = store
        .tenant_summary(&alpha)
        .expect("summary")
        .expect("tenant")
        .event_seq;

    assert!(retention::ensure_scheduled(&app, &alpha, app.now()).expect("scheduled"));
    // A second call finds the chain and starts no other.
    assert!(!retention::ensure_scheduled(&app, &alpha, app.now()).expect("scheduled"));
    run_jobs(&app, 0);

    // Ids: profiles 1, 2 (old) and 3 (young), tickets 4 (closed long ago) and 5 (closed lately).
    assert!(!record_exists(store, &alpha, "profile", 1));
    assert!(!record_exists(store, &alpha, "profile", 2));
    assert!(record_exists(store, &alpha, "profile", 3));
    assert!(!record_exists(store, &alpha, "ticket", 4));
    assert!(record_exists(store, &alpha, "ticket", 5));
    // The events of the deletions carry no personal value.
    let events = store
        .events_after(&alpha, seq_before, 10_000)
        .expect("events");
    let deleted = events
        .iter()
        .filter(|event| format!("{event:?}").contains("RecordDeleted"))
        .count();
    assert_eq!(deleted, 3, "{events:?}");
    let written = serde_json::to_string(&events).expect("encode");
    assert!(!written.contains("Ada Lovelace") && !written.contains("Grace Hopper"));
    // Another tenant is untouched.
    assert_eq!(
        store
            .tenant_summary(&beta)
            .expect("summary")
            .expect("tenant"),
        beta_before
    );
    assert!(record_exists(store, &beta, "profile", 1));
    // The chain continues: a job is waiting for the next sweep, a day on.
    let jobs = store.jobs_list(&alpha, None, 100).expect("jobs");
    assert!(jobs.iter().any(|job| !job.state.ended()
        && matches!(
            job.work,
            strata_storage_api::jobs::JobWork::Retention { .. }
        )
        && job.due_at.0 >= now + DAY / 2));
}

// Verifies: PRV-357
// Verifies: PRV-2744
#[test]
fn retention_on_sqlite() {
    let store = SqliteStore::open_in_memory(Timestamp(1_000)).expect("a store");
    retention_deletes_what_is_past_its_period_and_nothing_else(Arc::new(store));
}

// ---------------------------------------------------------------------------------------------
// Invitations addressed to the subject
// ---------------------------------------------------------------------------------------------
