//! The working set below sealing (WG-5391, `docs/working-set.md`, placement): `ResidentStore` sits
//! under `SealedStore`, so what is resident is the stored form, sealed tokens included, and the
//! sealed store opens every token on every load. A key destroyed through any handle therefore
//! shows on the next read although the generation did not move; key documents are never resident;
//! a commit that names a new data subject creates its key without a conflict; and no resident key
//! holds a personal value.

use std::sync::Arc;
use strata_backend::app::SealKey;
use strata_backend::subject_keys::{InProcessKeyService, SubjectKeys};
use strata_kernel::{
    DataSubject, ExistingData, FieldDef, FieldId, FieldType, Intent, Need, ObjectTypeId,
    PersonalDeclaration, PersonalKind, Principal, PrincipalId, Record, RecordId, RetentionRule,
    RetentionStart, SchemaChange, TenantId, Timestamp, Value,
};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::resident::ResidentStore;
use strata_storage_api::sealed::{is_sealed, SealedStore};
use strata_storage_api::{conformance, execute, Store};
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

fn ctx(site: &TenantId, principal: Principal, now: i64) -> strata_kernel::Context {
    strata_kernel::Context {
        tenant: site.clone(),
        principal,
        session: None,
        now: Timestamp(now),
        correlation: format!("t-resident-sealed-{now}"),
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
    }
}

fn owner(site: &TenantId, now: i64) -> strata_kernel::Context {
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
    }
}

/// A tenant whose `profile` holds `person`, `email` (personal, searchable, unique), `bio`
/// (personal, not searchable) and `plan`, made with real intents through `store`.
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
    // A type with a personal field must declare its retention (D-6584).
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
) -> strata_storage_api::Executed {
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
    .expect("a profile")
}

/// Ada's profile is record 1, Grace's record 2.
fn populated(store: &dyn Store, name: &str) -> TenantId {
    let site = found(store, name);
    create_profile(store, &site, 2_000, "alpha:ada", ADA_EMAIL, ADA_BIO);
    create_profile(
        store,
        &site,
        2_001,
        "alpha:grace",
        GRACE_EMAIL,
        "Grace's own words",
    );
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

fn email_of(record: &Record) -> Value {
    record.fields.get(&field("email")).cloned().expect("email")
}

/// Two handles on one storage: the first reads through the working set under sealing, the second
/// is another sealed handle straight on the storage. Both use the same key service.
struct Handles {
    resident: Arc<ResidentStore>,
    first: Arc<SealedStore>,
    second: Arc<SealedStore>,
    keys: Arc<SubjectKeys>,
    raw: Arc<dyn Store>,
}

fn handles() -> Handles {
    let keys = Arc::new(SubjectKeys::new(Arc::new(InProcessKeyService::new(
        SealKey::derive("resident sealed test key"),
        Vec::new(),
    ))));
    let raw: Arc<dyn Store> = Arc::new(MemoryStore::new());
    let resident = Arc::new(ResidentStore::new(raw.clone(), 1 << 22));
    Handles {
        first: Arc::new(SealedStore::new(resident.clone(), keys.clone())),
        second: Arc::new(SealedStore::new(raw.clone(), keys.clone())),
        resident,
        keys,
        raw,
    }
}

// Verifies: PRV-361
#[test]
fn a_key_destroyed_through_a_second_handle_returns_the_erased_token_on_the_next_read() {
    let h = handles();
    let alpha = populated(h.first.as_ref(), "alpha");

    // Plain through the working set while the key lives; what is resident is the token.
    let ada = record(h.first.as_ref(), &alpha, 1).expect("Ada's profile");
    assert_eq!(email_of(&ada), Value::Str(ADA_EMAIL.into()));
    let held = h
        .resident
        .held_record(&alpha, &object(), RecordId(1))
        .expect("the record is resident")
        .expect("and present");
    assert!(is_sealed(&email_of(&held)), "the entry is the stored form");
    let hits = h.resident.stats().record_hits;
    let ada = record(h.first.as_ref(), &alpha, 1).expect("Ada's profile");
    assert_eq!(email_of(&ada), Value::Str(ADA_EMAIL.into()));
    assert!(
        h.resident.stats().record_hits > hits,
        "served from the entry"
    );

    // The key is destroyed through the second handle: a document write, no generation moves.
    let generation = h.raw.generation(&alpha).unwrap();
    let destroyed = h
        .keys
        .destroy(
            h.second.inner().as_ref(),
            &alpha,
            &who("alpha:ada"),
            Timestamp(3_000),
        )
        .expect("the key is destroyed");
    assert_eq!(destroyed.len(), 1);
    assert_eq!(h.raw.generation(&alpha).unwrap(), generation);

    // The next read through the first handle is still a hit, and returns the erased token.
    let hits = h.resident.stats().record_hits;
    let erased = record(h.first.as_ref(), &alpha, 1).expect("Ada's profile");
    assert!(
        h.resident.stats().record_hits > hits,
        "still served from the entry"
    );
    assert!(is_sealed(&email_of(&erased)), "{:?}", email_of(&erased));
    let grace = record(h.first.as_ref(), &alpha, 2).expect("Grace's profile");
    assert_eq!(email_of(&grace), Value::Str(GRACE_EMAIL.into()));
}

// Verifies: PRV-358
#[test]
fn a_commit_naming_a_new_data_subject_does_not_conflict() {
    let h = handles();
    let alpha = populated(h.first.as_ref(), "alpha");
    // Warm entries, so that the commit has something to confirm.
    record(h.first.as_ref(), &alpha, 1).expect("Ada's profile");
    record(h.first.as_ref(), &alpha, 2).expect("Grace's profile");
    let stamp = h.resident.record_stamp(&alpha);
    assert!(stamp.is_some());

    // A subject with no key yet: the commit creates the key in its own transaction.
    let executed = create_profile(
        h.first.as_ref(),
        &alpha,
        4_000,
        "alpha:new",
        "new@example.org",
        "a new person",
    );
    assert_eq!(
        executed.conflicts, 0,
        "no conflict with its own key creation"
    );
    assert_eq!(
        h.resident.record_stamp(&alpha),
        h.raw.generation(&alpha).unwrap()
    );
    assert_ne!(
        h.resident.record_stamp(&alpha),
        stamp,
        "the stamp followed the commit"
    );
    // The entries the commit did not write are carried, and still read plain.
    assert!(h
        .resident
        .held_record(&alpha, &object(), RecordId(2))
        .is_some());
    let grace = record(h.first.as_ref(), &alpha, 2).expect("Grace's profile");
    assert_eq!(email_of(&grace), Value::Str(GRACE_EMAIL.into()));
    let new = record(h.first.as_ref(), &alpha, 3).expect("the new profile");
    assert_eq!(email_of(&new), Value::Str("new@example.org".into()));
}

// Verifies: PRV-358
#[test]
fn no_plaintext_personal_value_appears_in_a_resident_key_with_legacy_plain_rows() {
    let h = handles();
    // Rows written before sealing: through the storage, not the sealed store. They hold the plain
    // values, and the schema declares the fields personal.
    let alpha = populated(h.raw.as_ref(), "alpha");
    let looked_up = |need: Need| {
        h.first
            .load(&alpha, &[need])
            .expect("a load through the working set under sealing")
    };
    // The lookups of a personal field, in every form the sealing plan translates.
    let by_field = looked_up(Need::ByField {
        object: object(),
        field: field("email"),
        value: Value::Str(ADA_EMAIL.into()),
    });
    let found = by_field.tenant.expect("the tenant").records[&object()]
        .get(&RecordId(1))
        .cloned()
        .expect("the plain row is found by its plain value");
    assert_eq!(email_of(&found), Value::Str(ADA_EMAIL.into()));
    looked_up(Need::ByFields {
        object: object(),
        fields: [(field("email"), Value::Str(ADA_EMAIL.into()))].into(),
    });
    looked_up(Need::AllOf(object()));
    record(h.first.as_ref(), &alpha, 1).expect("Ada's profile");
    record(h.first.as_ref(), &alpha, 2).expect("Grace's profile");

    let keys = h.resident.resident_keys();
    assert!(keys.iter().any(|key| key == "alpha/profile/1"), "{keys:?}");
    for key in &keys {
        for plain in [
            ADA_EMAIL,
            ADA_BIO,
            GRACE_EMAIL,
            "example.org",
            "secret",
            "ada",
            "grace",
        ] {
            assert!(
                !key.to_lowercase().contains(plain),
                "{key:?} holds the personal value {plain:?}"
            );
        }
        let ok = key == "alpha/header"
            || key
                .strip_prefix("alpha/profile/")
                .is_some_and(|id| id.chars().all(|c| c.is_ascii_digit()));
        assert!(ok, "{key:?} is a tenant, an object type and a record id");
    }
    // The answers themselves were not made resident.
    assert_eq!(
        keys.iter().filter(|key| key.contains("/profile/")).count() as u64,
        h.resident.stats().resident_records
    );
}

// Verifies: STO-001
#[test]
fn the_storage_conformance_suite_passes_through_the_working_set_under_sealing() {
    let keys = || {
        Arc::new(SubjectKeys::new(Arc::new(InProcessKeyService::new(
            SealKey::derive("resident sealed test key"),
            Vec::new(),
        ))))
    };
    let over_memory = || {
        let resident = Arc::new(ResidentStore::new(Arc::new(MemoryStore::new()), 1 << 22));
        Box::new(SealedStore::new(resident, keys())) as Box<dyn Store>
    };
    let ran = conformance::run(&over_memory);
    assert!(ran.len() >= 13, "the suite ran: {ran:?}");
    let over_sqlite = || {
        let inner = Arc::new(SqliteStore::open_in_memory(Timestamp(1)).expect("a store"));
        let resident = Arc::new(ResidentStore::new(inner, 1 << 22));
        Box::new(SealedStore::new(resident, keys())) as Box<dyn Store>
    };
    let ran = conformance::run(&over_sqlite);
    assert!(ran.len() >= 13, "the suite ran: {ran:?}");
}
