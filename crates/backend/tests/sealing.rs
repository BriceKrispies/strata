//! Sealed personal fields through the backend (D-350, D-7720 to D-7728): a field declared personal
//! is sealed under its subject's key in every copy the store writes (the record and its index, the
//! events, the journal, the snapshots), reads back plain while the key lives, and is unreadable in
//! every one of them once an erasure has destroyed the key; another subject's data, a field that is
//! not personal, the replay of the journal and the audit chain are untouched by it.

use serde_json::json;
use std::sync::Arc;
use strata_backend::app::{App, Config};
use strata_backend::auth::Identity;
use strata_backend::erasure::erase_subject;
use strata_backend::mail::DevSink;
use strata_kernel::{
    Action, Context, DataSubject, ErrorKind, ExistingData, Expr, FieldDef, FieldId, FieldType,
    Intent, Need, ObjectTypeId, PersonalDeclaration, PersonalKind, PolicyDef, PolicyId, Principal,
    PrincipalId, QueryDef, QueryId, Record, RecordId, Resource, RetentionRule, RetentionStart,
    RoleDef, RoleId, SchemaChange, SessionId, TenantId, Timestamp, Value, ViewDef, ViewId,
    ViewSection,
};
use strata_protocol::api::ViewRequest;
use strata_protocol::ws::SessionState;
use strata_simulation::journal::{replay_journal_with, Recorded};
use strata_storage_api::sealed::{
    is_sealed, parse_token, CipherSession, FieldCipher, TOKEN_PREFIX,
};
use strata_storage_api::{execute, ExecuteError, SessionRecord, Store};
use strata_storage_sqlite::SqliteStore;

const ADA_EMAIL: &str = "ada@example.org";
const ADA_BIO: &str = "a secret biography";
const GRACE_EMAIL: &str = "grace@example.org";

fn tenant(name: &str) -> TenantId {
    TenantId::new(name).expect("tenant id")
}

fn who(name: &str) -> PrincipalId {
    PrincipalId::new(name).expect("principal id")
}

fn object() -> ObjectTypeId {
    ObjectTypeId::new("profile").expect("object id")
}

fn field(name: &str) -> FieldId {
    FieldId::new(name).expect("field id")
}

fn app() -> Arc<App> {
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
    let store = SqliteStore::open_in_memory(Timestamp(1_000)).expect("a store");
    App::new(cfg, Arc::new(store))
}

fn ctx(site: &TenantId, principal: Principal, now: i64) -> Context {
    Context {
        tenant: site.clone(),
        principal,
        session: None,
        now: Timestamp(now),
        correlation: format!("t-sealing-{now}"),
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
    }
}

fn owner(site: &TenantId, now: i64) -> Context {
    ctx(
        site,
        Principal::User(who(&format!("{}:owner", site.as_str()))),
        now,
    )
}

fn def(name: &str, unique: bool, personal: Option<bool>) -> FieldDef {
    FieldDef {
        id: field(name),
        label: name.to_string(),
        ty: FieldType::Str,
        required: false,
        unique,
        default: None,
        read: None,
        personal: personal.map(|searchable| PersonalDeclaration {
            kind: PersonalKind::Other,
            subject: DataSubject::Field {
                field: field("person"),
            },
            searchable,
        }),
        normalise: None,
        computed: None,
    }
}

/// A tenant whose `profile` holds `person` (the principal the profile is about), `email`
/// (personal, searchable, unique), `bio` (personal, not searchable) and `plan` (not personal),
/// made with real intents so that the journal replays.
fn found(store: &dyn Store, name: &str) -> TenantId {
    let site = tenant(name);
    execute(
        store,
        &Intent::CreateTenant {
            name: name.to_string(),
            owner: who(&format!("{name}:owner")),
        },
        &ctx(&site, Principal::System, 1_000),
    )
    .expect("the tenant is created");
    let mut changes = vec![SchemaChange::DefineObject {
        id: object(),
        label: "Profile".into(),
    }];
    for field in [
        def("person", false, None),
        def("email", true, Some(true)),
        def("bio", false, Some(false)),
        def("plan", false, None),
    ] {
        changes.push(SchemaChange::DefineField {
            object: object(),
            field,
        });
    }
    changes.push(SchemaChange::SetRetention {
        object: object(),
        retention: RetentionRule {
            days: 30,
            from: RetentionStart::Created,
        },
    });
    execute(
        store,
        &Intent::ChangeSchema {
            changes,
            expected_version: None,
            existing: ExistingData::Reject,
        },
        &owner(&site, 1_001),
    )
    .expect("the schema is changed");
    site
}

fn create_profile(
    store: &dyn Store,
    site: &TenantId,
    now: i64,
    person: &str,
    email: &str,
    bio: &str,
) -> Result<(), ExecuteError> {
    let fields = [
        ("person", person),
        ("email", email),
        ("bio", bio),
        ("plan", "free"),
    ]
    .into_iter()
    .map(|(name, value)| (field(name), Value::Str(value.to_string())))
    .collect();
    execute(
        store,
        &Intent::CreateRecord {
            object: object(),
            fields,
        },
        &owner(site, now),
    )
    .map(|_| ())
}

/// Ada's profile is record 1, Grace's record 2.
fn populated(store: &dyn Store, name: &str) -> TenantId {
    let site = found(store, name);
    create_profile(store, &site, 2_000, "alpha:ada", ADA_EMAIL, ADA_BIO).expect("Ada's profile");
    create_profile(
        store,
        &site,
        2_001,
        "alpha:grace",
        GRACE_EMAIL,
        "Grace's own words",
    )
    .expect("Grace's profile");
    site
}

fn record(store: &dyn Store, site: &TenantId, id: u64) -> Option<Arc<Record>> {
    let state = store
        .load(
            site,
            &[Need::Record {
                object: object(),
                id: RecordId(id),
            }],
        )
        .expect("load");
    state
        .tenant
        .and_then(|tenant| tenant.records.get(&object())?.get(&RecordId(id)).cloned())
}

fn text_of<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("encode")
}

fn value_of(record: &Record, name: &str) -> Option<Value> {
    record.fields.get(&field(name)).cloned()
}

// Verifies: PRV-358
// Verifies: PRV-2720
// Verifies: PRV-2724
#[test]
fn a_personal_field_is_sealed_in_every_copy_and_unreadable_in_all_of_them_after_erasure() {
    let app = app();
    let store = app.store.clone();
    let raw = app.sealed.inner().clone();
    let alpha = populated(store.as_ref(), "alpha");
    let snapshot = store
        .create_snapshot(&alpha, "before the erasure", Timestamp(2_500))
        .expect("snapshot");

    // Before: plain through the store, sealed as stored.
    let ada = record(store.as_ref(), &alpha, 1).expect("Ada's profile");
    assert_eq!(value_of(&ada, "email"), Some(Value::Str(ADA_EMAIL.into())));
    assert_eq!(value_of(&ada, "bio"), Some(Value::Str(ADA_BIO.into())));
    let stored = record(raw.as_ref(), &alpha, 1).expect("Ada's profile as stored");
    assert!(value_of(&stored, "email").is_some_and(|v| is_sealed(&v)));
    assert!(value_of(&stored, "bio").is_some_and(|v| is_sealed(&v)));
    assert_eq!(value_of(&stored, "plan"), Some(Value::Str("free".into())));
    let as_stored = [
        text_of(&raw.events_after(&alpha, 0, 10_000).expect("events")),
        text_of(&raw.journal_after(&alpha, 0, 10_000).expect("journal")),
        text_of(&raw.load_snapshot(&alpha, snapshot.id).expect("snapshot")),
        text_of(&raw.load_complete(&alpha).expect("state")),
    ];
    for copy in &as_stored {
        assert!(
            !copy.contains(ADA_EMAIL) && !copy.contains(ADA_BIO),
            "{copy}"
        );
        assert!(copy.contains(TOKEN_PREFIX));
    }
    let opened = [
        text_of(&store.events_after(&alpha, 0, 10_000).expect("events")),
        text_of(&store.journal_after(&alpha, 0, 10_000).expect("journal")),
        text_of(&store.load_snapshot(&alpha, snapshot.id).expect("snapshot")),
    ];
    for copy in &opened {
        assert!(copy.contains(ADA_EMAIL) && copy.contains(ADA_BIO), "{copy}");
    }

    let receipt = erase_subject(&app, &owner(&alpha, 3_000), &who("alpha:ada")).expect("erasure");
    assert!(receipt
        .reached
        .iter()
        .any(|line| line.contains("subject keys") && line.contains("1 destroyed")));

    // After: no copy reads back a value of Ada, through the store or as stored.
    let after = [
        text_of(&store.events_after(&alpha, 0, 10_000).expect("events")),
        text_of(&store.journal_after(&alpha, 0, 10_000).expect("journal")),
        text_of(&store.load_snapshot(&alpha, snapshot.id).expect("snapshot")),
        text_of(&store.load_complete(&alpha).expect("state")),
        text_of(&raw.events_after(&alpha, 0, 10_000).expect("events")),
        text_of(&raw.journal_after(&alpha, 0, 10_000).expect("journal")),
    ];
    for copy in &after {
        assert!(
            !copy.contains(ADA_EMAIL) && !copy.contains(ADA_BIO),
            "{copy}"
        );
    }
    // The copies written before the erasure still hold Ada's values, sealed: the erased form.
    let old = store.load_snapshot(&alpha, snapshot.id).expect("snapshot");
    let old_ada = old
        .records
        .get(&object())
        .and_then(|records| records.get(&RecordId(1)))
        .expect("Ada's profile in the snapshot");
    assert!(value_of(old_ada, "email").is_some_and(|v| is_sealed(&v)));

    // Another subject and a field that is not personal are untouched.
    let grace = record(store.as_ref(), &alpha, 2).expect("Grace's profile");
    assert_eq!(
        value_of(&grace, "email"),
        Some(Value::Str(GRACE_EMAIL.into()))
    );
    let ada = record(store.as_ref(), &alpha, 1).expect("Ada's profile");
    assert_eq!(value_of(&ada, "plan"), Some(Value::Str("free".into())));
    for copy in &after[..4] {
        assert!(copy.contains(GRACE_EMAIL), "{copy}");
    }
}

// Verifies: TEN-1212
// Verifies: PRV-2721
// Verifies: PRV-2727
#[test]
fn a_snapshot_taken_before_the_erasure_restores_with_the_subject_erased() {
    let app = app();
    let store = app.store.clone();
    let alpha = populated(store.as_ref(), "alpha");
    let snapshot = store
        .create_snapshot(&alpha, "before the erasure", Timestamp(2_500))
        .expect("snapshot");
    erase_subject(&app, &owner(&alpha, 3_000), &who("alpha:ada")).expect("erasure");
    store
        .restore_snapshot(&alpha, snapshot.id, None)
        .expect("restore");
    // The key is not part of the snapshot, so its destruction holds after the restore.
    let ada = record(store.as_ref(), &alpha, 1).expect("Ada's profile is back");
    assert!(value_of(&ada, "email").is_some_and(|v| is_sealed(&v)));
    assert!(value_of(&ada, "bio").is_some_and(|v| is_sealed(&v)));
    let state = text_of(&store.load_complete(&alpha).expect("state"));
    assert!(!state.contains(ADA_EMAIL) && !state.contains(ADA_BIO));
    assert!(state.contains(GRACE_EMAIL));
}

// Verifies: PRV-361
// Verifies: PRV-2722
#[test]
fn the_journal_replays_and_verifies_before_and_after_an_erasure() {
    let app = app();
    let store = app.store.clone();
    let alpha = populated(store.as_ref(), "alpha");
    let replay = || {
        let entries: Vec<Recorded> = store
            .journal_after(&alpha, 0, 10_000)
            .expect("journal")
            .into_iter()
            .map(|e| Recorded {
                revision: e.revision,
                kernel_version: e.kernel_version,
                intent: e.intent,
                context: e.context,
                delta_hash: e.delta_hash,
                events_hash: e.events_hash,
            })
            .collect();
        let live = store
            .load_complete(&alpha)
            .expect("state")
            .tenant
            .expect("tenant");
        replay_journal_with(&alpha, &entries, Some(&live), &|before, transition| {
            app.sealed
                .journal_hashes(&alpha, before, &transition.delta, &transition.events)
                .ok()
                .flatten()
        })
    };
    let before = replay();
    assert_eq!(before.agreed, before.entries, "{before:?}");
    assert!(before.matches_live, "{before:?}");

    erase_subject(&app, &owner(&alpha, 3_000), &who("alpha:ada")).expect("erasure");

    let after = replay();
    assert!(after.first_mismatch.is_none(), "{after:?}");
    assert_eq!(after.agreed, after.entries, "{after:?}");
    assert!(after.matches_live, "{after:?}");
}

// Verifies: PRV-359
// Verifies: PRV-360
// Verifies: PRV-2725
#[test]
fn the_audit_chain_verifies_after_an_erasure_and_the_subjects_lines_are_sealed() {
    let app = app();
    let store = app.store.clone();
    let alpha = populated(store.as_ref(), "alpha");
    for (principal, role) in [("alpha:ada", "editor"), ("alpha:grace", "viewer")] {
        app.audit(
            "role.assigned",
            "t-audit",
            json!({ "tenant": "alpha", "principal": principal, "role": role }),
        );
    }
    let before = app.verify_audit_chain(&alpha).expect("chain");
    assert!(before.ok && before.verified >= 2, "{before:?}");
    let lines = text_of(&store.audit_list(&alpha, None, 1_000).expect("audit"));
    assert!(lines.contains("alpha:ada") && lines.contains("alpha:grace"));
    let stored = text_of(
        &app.sealed
            .inner()
            .audit_list(&alpha, None, 1_000)
            .expect("audit"),
    );
    assert!(!stored.contains("alpha:ada") && !stored.contains("alpha:grace"));

    erase_subject(&app, &owner(&alpha, 3_000), &who("alpha:ada")).expect("erasure");

    let after = app.verify_audit_chain(&alpha).expect("chain");
    assert!(after.ok, "{after:?}");
    assert!(after.verified > before.verified);
    let lines = store.audit_list(&alpha, None, 1_000).expect("audit");
    let assigned: Vec<serde_json::Value> = lines
        .iter()
        .filter(|line| line.event == "role.assigned")
        .map(|line| serde_json::from_str(&line.data).expect("json"))
        .collect();
    assert_eq!(assigned.len(), 2);
    assert!(assigned[0]["principal"]
        .as_str()
        .is_some_and(|text| text.starts_with(TOKEN_PREFIX)));
    assert_eq!(assigned[1]["principal"], "alpha:grace");
    // The destruction is itself a line, naming the tenant and the key, never the key material.
    assert!(lines
        .iter()
        .any(|line| line.event == "erasure.subject_key_destroyed"));
}

// Verifies: SEC-001
// Verifies: SEC-2728
#[test]
fn another_tenants_keys_open_nothing_and_a_value_opens_only_for_its_own_field() {
    let app = app();
    let store = app.store.clone();
    let raw = app.sealed.inner().clone();
    let alpha = populated(store.as_ref(), "alpha");
    let beta = found(store.as_ref(), "beta");
    create_profile(
        store.as_ref(),
        &beta,
        2_000,
        "beta:ada",
        "b@example.org",
        "b",
    )
    .expect("a profile in beta");

    let stored = record(raw.as_ref(), &alpha, 1).expect("Ada's profile as stored");
    let Some(Value::Str(token)) = value_of(&stored, "email") else {
        panic!("the address is sealed: {stored:?}");
    };
    let parts = parse_token(&token).expect("a token");
    let open = |site: &TenantId, name: &str| {
        app.keys.session(raw.as_ref(), site).open_one(
            "profile",
            name,
            parts.key_id,
            parts.blinds,
            parts.payload,
        )
    };
    assert!(open(&alpha, "email").expect("open").is_some());
    // Another field of the same record does not open it.
    assert_eq!(open(&alpha, "bio").expect("open"), None);
    // Another tenant has no such key, and with Ada's key document copied in, the key is wrapped
    // under alpha's tenant key and bound to alpha: it does not open there.
    assert!(!matches!(open(&beta, "email"), Ok(Some(_))));
    let key_doc = raw
        .doc_get(&alpha, "subject-key", parts.key_id)
        .expect("doc")
        .expect("Ada's key");
    raw.doc_put(&beta, "subject-key", parts.key_id, &key_doc, false)
        .expect("copied");
    assert!(!matches!(open(&beta, "email"), Ok(Some(_))));
}
