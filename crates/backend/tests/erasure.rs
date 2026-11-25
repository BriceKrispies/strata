//! The erasure of one subject through the backend: what it removes from the tenant's records,
//! credentials, sessions, memberships and identity documents, what it leaves for the receipt to
//! name, and what it never touches. Each scenario runs on the in-memory store and on SQLite.

use serde_json::json;
use std::sync::Arc;
use strata_backend::app::{App, Config};
use strata_backend::erasure::{erase_subject, Receipt, RECEIPT_KIND};
use strata_backend::mail::DevSink;
use strata_kernel::ext::TenantSchemaExt;
use strata_kernel::{
    Context, DataSubject, DeltaOp, ExistingData, FieldDef, FieldId, FieldType, Intent,
    ObjectTypeDef, ObjectTypeId, Output, PersonalDeclaration, PersonalKind, Principal, PrincipalId,
    RecordId, RetentionRule, RetentionStart, RoleId, SessionId, StateDelta, StateRevision,
    TenantId, TenantSchema, Timestamp, Transition, Value,
};

use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{
    execute, Account, Commit, PasskeyRecord, SessionRecord, Store, TotpRecord,
};
use strata_storage_sqlite::SqliteStore;

fn tenant(name: &str) -> TenantId {
    TenantId::new(name).expect("tenant id")
}

fn who(name: &str) -> PrincipalId {
    PrincipalId::new(name).expect("principal id")
}

fn app_on(store: Arc<dyn Store>) -> Arc<App> {
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
                searchable: false,
            }),
            normalise: None,
        },
    )
}

fn record_type(id: &str, fields: Vec<(FieldId, FieldDef)>) -> ObjectTypeDef {
    ObjectTypeDef {
        id: ObjectTypeId::new(id).expect("object id"),
        label: id.to_string(),
        fields: fields.into_iter().collect(),
        constraints: Default::default(),
        retention: Some(RetentionRule {
            days: 30,
            from: RetentionStart::Created,
        }),
        publication: None,
        language_field: None,
        translation_group: None,
        unique: None,
        searchable: None,
    }
}

/// A tenant with two record types that declare personal data: `profile`, whose `name`, `bio` and
/// `city` belong to the person named by `person`, and `diary`, whose `entry` belongs to its
/// creator. The owner is `<name>:owner`.
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
    // A schema change cannot declare retention, so the types are committed as they are.
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
    schema.objects.insert(
        ObjectTypeId::new("profile").expect("object id"),
        record_type(
            "profile",
            vec![
                declared("person", FieldType::Str, false, None),
                declared("name", FieldType::Str, true, person()),
                declared("bio", FieldType::Str, false, person()),
                declared("city", FieldType::Str, false, person()),
                declared("plan", FieldType::Str, false, None),
            ],
        ),
    );
    schema.objects.insert(
        ObjectTypeId::new("diary").expect("object id"),
        record_type(
            "diary",
            vec![
                declared("entry", FieldType::Str, true, Some(DataSubject::Creator)),
                declared("topic", FieldType::Str, false, None),
            ],
        ),
    );
    assert!(schema.problems().is_empty());
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

fn as_owner(site: &TenantId, now: i64) -> Context {
    ctx(
        site,
        Principal::User(who(&format!("{}:owner", site.as_str()))),
        now,
        "t-erasure-1",
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

fn profile(store: &dyn Store, context: &Context, person: &str, name: &str) {
    create(
        store,
        context,
        "profile",
        &[
            ("person", Value::Str(person.into())),
            ("name", Value::Str(name.into())),
            ("bio", Value::Str("a long personal biography".into())),
            ("city", Value::Str("Rue du Secret 7".into())),
            ("plan", Value::Str("free".into())),
        ],
    );
}

fn diary(store: &dyn Store, context: &Context, entry: &str) {
    create(
        store,
        context,
        "diary",
        &[
            ("entry", Value::Str(entry.into())),
            ("topic", Value::Str("weather".into())),
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
            credential: "{}".into(),
            created_at: Timestamp(now),
            last_used_at: None,
        })
        .expect("passkey");
    store
        .put_totp(&TotpRecord {
            tenant: site.clone(),
            principal: person.clone(),
            secret: "sealed".into(),
            confirmed: true,
            last_step: -1,
            created_at: Timestamp(now),
        })
        .expect("totp");
    store
        .put_session(&SessionRecord {
            id: SessionId::new(format!("s-{}-{}", site.as_str(), person.as_str()))
                .expect("session id"),
            token_hash: format!("hash-{}-{}", site.as_str(), person.as_str()),
            csrf_token: "csrf".into(),
            tenant: site.clone(),
            principal: person.clone(),
            display_name: "Ada Lovelace".into(),
            created_at: Timestamp(now),
            last_seen_at: Timestamp(now),
            expires_at: Timestamp(now + 3_600_000),
            revoked_at: None,
            authenticated_at: Timestamp(now),
            network: None,
        })
        .expect("session");
    store
        .doc_put(
            site,
            "scim-user",
            person.as_str(),
            &json!({ "userName": "ada", "principal": person.as_str() }).to_string(),
            false,
        )
        .expect("user document");
    store
        .doc_put(
            site,
            "grant",
            &format!("g-{}", person.as_str()),
            &json!({
                "id": format!("g-{}", person.as_str()),
                "principal": person.as_str(),
                "agent": "helper",
                "scopes": [],
                "purpose": "a private purpose",
                "created_at": now,
                "expires_at": now + 3_600_000,
                "revoked_at": null,
            })
            .to_string(),
            false,
        )
        .expect("grant");
}

fn plan_of(store: &dyn Store, site: &TenantId, id: u64) -> Option<String> {
    let state = store
        .load(
            site,
            &[strata_kernel::Need::Record {
                object: ObjectTypeId::new("profile").expect("object id"),
                id: RecordId(id),
            }],
        )
        .expect("load")
        .tenant
        .expect("tenant");
    state
        .records
        .get(&ObjectTypeId::new("profile").expect("object id"))
        .and_then(|records| records.get(&RecordId(id)))
        .and_then(|record| {
            record
                .fields
                .get(&FieldId::new("plan").expect("field id"))
                .and_then(|v| match v {
                    Value::Str(text) => Some(text.clone()),
                    _ => None,
                })
        })
}

fn record_json(store: &dyn Store, site: &TenantId, object: &str, id: u64) -> String {
    let object = ObjectTypeId::new(object).expect("object id");
    let state = store
        .load(
            site,
            &[strata_kernel::Need::Record {
                object: object.clone(),
                id: RecordId(id),
            }],
        )
        .expect("load")
        .tenant
        .expect("tenant");
    serde_json::to_string(
        state
            .records
            .get(&object)
            .and_then(|records| records.get(&RecordId(id)))
            .expect("the record exists")
            .as_ref(),
    )
    .expect("encode")
}

const PERSONAL: [&str; 5] = [
    "Ada Lovelace",
    "a long personal biography",
    "Rue du Secret 7",
    "my secret day",
    "a private purpose",
];

/// The whole subject-rights scenario on one store.
fn the_subject_is_erased_and_the_rest_is_kept(store: Arc<dyn Store>) {
    let app = app_on(store.clone());
    let store = store.as_ref();
    let alpha = found(store, "alpha");
    let beta = found(store, "beta");
    let ada = who("alpha:ada");
    let owner = as_owner(&alpha, 2_000);

    // Ada is an owner too, so she can create a record of her own; her roles go with her.
    execute(
        store,
        &Intent::AssignRole {
            principal: ada.clone(),
            role: RoleId::new("owner").expect("role"),
        },
        &owner,
    )
    .expect("role");
    let as_ada = ctx(&alpha, Principal::User(ada.clone()), 2_001, "t-ada");
    profile(store, &owner, "alpha:ada", "Ada Lovelace");
    profile(store, &owner, "alpha:grace", "Grace Hopper");
    diary(store, &as_ada, "my secret day");
    diary(store, &owner, "the owner's day");
    give_credentials(store, &alpha, &ada, 2_002);

    // The other tenant holds a person of the same name, and a profile that names her.
    let beta_owner = as_owner(&beta, 2_000);
    profile(store, &beta_owner, "alpha:ada", "Ada Lovelace");
    give_credentials(store, &beta, &ada, 2_002);
    let beta_before = store
        .tenant_summary(&beta)
        .expect("summary")
        .expect("tenant");
    let beta_record = record_json(store, &beta, "profile", 1);

    let audit_before = store.audit_list(&alpha, None, 1_000).expect("audit");
    let seq_before = store
        .tenant_summary(&alpha)
        .expect("summary")
        .expect("tenant")
        .event_seq;
    let grace_before = record_json(store, &alpha, "profile", 2);
    // Record ids count across the types: profiles 1 and 2, then the diaries 3 (Ada) and 4.
    let owner_diary_before = record_json(store, &alpha, "diary", 4);

    let receipt = erase_subject(&app, &owner, &ada).expect("the erasure");

    // Records: personal fields are cleared, the others kept, other people's records untouched.
    let mine = record_json(store, &alpha, "profile", 1);
    for value in PERSONAL {
        assert!(!mine.contains(value), "{value:?} is still in {mine}");
    }
    assert_eq!(plan_of(store, &alpha, 1).as_deref(), Some("free"));
    assert_eq!(record_json(store, &alpha, "profile", 2), grace_before);
    let diary_mine = record_json(store, &alpha, "diary", 3);
    assert!(!diary_mine.contains("my secret day"));
    assert!(diary_mine.contains("weather"));
    assert_eq!(record_json(store, &alpha, "diary", 4), owner_diary_before);
    assert_eq!(receipt.records.len(), 2);
    assert_eq!(receipt.changed.len(), 2);

    // Credentials, sessions and memberships are gone.
    assert!(store.account_of(&alpha, &ada).expect("account").is_none());
    assert!(store.passkeys(&alpha, &ada).expect("passkeys").is_empty());
    assert!(store.totp(&alpha, &ada).expect("totp").is_none());
    assert!(store
        .live_sessions_of(&alpha, Some(&ada), Timestamp(3_000))
        .expect("sessions")
        .is_empty());
    assert!(store
        .session(&SessionId::new("s-alpha-alpha:ada").expect("session id"))
        .expect("session")
        .is_some_and(|s| s.revoked_at.is_some()));
    let header = store
        .load(&alpha, &[])
        .expect("load")
        .tenant
        .expect("tenant");
    assert!(!header.members.contains_key(&ada));
    assert!(store
        .doc_get(&alpha, "scim-user", ada.as_str())
        .expect("doc")
        .is_none());
    let grant = store
        .doc_get(&alpha, "grant", "g-alpha:ada")
        .expect("doc")
        .expect("the grant stays as a record");
    let grant: serde_json::Value = serde_json::from_str(&grant).expect("a grant");
    assert_eq!(grant["purpose"], "");
    assert!(!grant["revoked_at"].is_null());

    // Events written by the erasure carry no personal value.
    let events = store
        .events_after(&alpha, seq_before, 10_000)
        .expect("events");
    assert!(!events.is_empty());
    let written = serde_json::to_string(&events).expect("encode");
    for value in PERSONAL {
        assert!(!written.contains(value), "an event carries {value:?}");
    }

    // Another tenant is untouched: its revision, its record, its credentials.
    assert_eq!(
        store
            .tenant_summary(&beta)
            .expect("summary")
            .expect("tenant"),
        beta_before
    );
    assert_eq!(record_json(store, &beta, "profile", 1), beta_record);
    assert!(store.account_of(&beta, &ada).expect("account").is_some());
    assert!(store.totp(&beta, &ada).expect("totp").is_some());

    // The audit log only grew.
    let audit_after = store.audit_list(&alpha, None, 1_000).expect("audit");
    assert!(audit_after.len() > audit_before.len());
    assert_eq!(audit_after[..audit_before.len()], audit_before[..]);

    // The receipt names what was reached and what was not, and is kept.
    assert!(!receipt.complete);
    for copy in [
        "values written before sealing",
        "events and outbox",
        "journal",
        "snapshots",
        "backup images",
        "exports",
        "audit log",
    ] {
        assert!(
            receipt.not_reached.iter().any(|line| line.contains(copy)),
            "the receipt does not name {copy}: {:?}",
            receipt.not_reached
        );
    }
    for copy in ["subject keys", "schema history"] {
        assert!(
            receipt.reached.iter().any(|line| line.contains(copy)),
            "the receipt does not name {copy} as reached: {:?}",
            receipt.reached
        );
    }
    assert!(receipt
        .reached
        .iter()
        .any(|line| line.contains("1 logins, 1 passkeys")));
    // Restrictions and legal holds are built: the receipt says what was in force, none here.
    for line in [
        "restriction of processing (PRV-352): none",
        "legal holds (PRV-355): none",
    ] {
        assert!(
            receipt.reached.iter().any(|reached| reached.contains(line)),
            "the receipt does not say {line:?}: {:?}",
            receipt.reached
        );
    }
    let kept = store.doc_list(&alpha, RECEIPT_KIND).expect("receipts");
    assert_eq!(kept.len(), 1);
    let again: Receipt = serde_json::from_str(&kept[0].1).expect("a receipt");
    assert_eq!(again, receipt);
    assert!(store
        .doc_list(&beta, RECEIPT_KIND)
        .expect("receipts")
        .is_empty());
    assert!(!kept[0].1.contains("Ada Lovelace"));
}

// Verifies: PRV-2460
// Verifies: PRV-2462
// Verifies: PRV-2463
#[test]
fn the_subject_is_erased_on_sqlite() {
    let store = SqliteStore::open_in_memory(Timestamp(1_000)).expect("a store");
    the_subject_is_erased_and_the_rest_is_kept(Arc::new(store));
}

// Verifies: PRV-353
// Verifies: PRV-3122
#[test]
fn the_receipt_names_each_backup_image_and_export_with_its_own_expiry() {
    const DAY: i64 = 86_400_000;
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
    let app = app_on(store.clone());
    let store = store.as_ref();
    let alpha = found(store, "alpha");
    let owner = as_owner(&alpha, 2_000);
    profile(store, &owner, "alpha:ada", "Ada Lovelace");
    give_credentials(store, &alpha, &who("alpha:ada"), 2_002);
    // Two backup images taken at different times, and two exports of her data and one of Grace's.
    let early = store
        .create_snapshot(&alpha, "nightly", Timestamp(5_000))
        .expect("an image");
    let late = store
        .create_snapshot(&alpha, "weekly", Timestamp(5_000 + 3 * DAY))
        .expect("an image");
    for (id, subject, requested, expires) in [
        ("x-1", "alpha:ada", 6_000, 6_000 + 7 * DAY),
        ("x-2", "alpha:ada", 7_000, 7_000 + 7 * DAY),
        ("x-3", "alpha:grace", 8_000, 8_000 + 7 * DAY),
    ] {
        store
            .doc_put(
                &alpha,
                strata_backend::subject_export::REQUEST_KIND,
                id,
                &json!({
                    "id": id, "subject": subject, "requested_by": "alpha:owner",
                    "requested_at": requested, "expires_at": expires,
                    "state": "ready", "batches": 1, "records": 1,
                })
                .to_string(),
                false,
            )
            .expect("an export request");
    }

    let receipt = erase_subject(&app, &owner, &who("alpha:ada")).expect("the erasure");

    // One line per image, each with the expiry of that image: 35 days after it was taken.
    for image in [&early, &late] {
        let expires = image.created_at.0 + 35 * DAY;
        let lines: Vec<&String> = receipt
            .reached
            .iter()
            .filter(|line| line.contains(&format!("snapshot {} \"{}\"", image.id, image.label)))
            .collect();
        assert_eq!(lines.len(), 1, "{:?}", receipt.reached);
        assert!(
            lines[0].contains(&format!("expires at {expires} ")),
            "{}",
            lines[0]
        );
    }
    // One line per export of hers, with its own expiry, and none for Grace's; hers are removed.
    for (id, expires) in [("x-1", 6_000 + 7 * DAY), ("x-2", 7_000 + 7 * DAY)] {
        let lines: Vec<&String> = receipt
            .reached
            .iter()
            .filter(|line| line.contains(&format!("(export-subject-data) {id},")))
            .collect();
        assert_eq!(lines.len(), 1, "{:?}", receipt.reached);
        assert!(
            lines[0].contains(&format!("expiry {expires} ")),
            "{}",
            lines[0]
        );
    }
    assert!(!receipt.reached.iter().any(|line| line.contains("x-3")));
    let left: Vec<String> = store
        .doc_list(&alpha, strata_backend::subject_export::REQUEST_KIND)
        .expect("requests")
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    assert_eq!(left, vec!["x-3".to_string()]);
    // The receipt that is kept says the same.
    let kept = store.doc_list(&alpha, RECEIPT_KIND).expect("receipts");
    assert!(
        kept[0].1.contains(&format!("snapshot {}", early.id)),
        "{}",
        kept[0].1
    );
}
