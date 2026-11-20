//! The sealed store over the in-memory adapter, with a stand-in cipher that seals nothing in
//! secret but binds and destroys like the real one: it passes the conformance suite, its journal
//! hashes are the ones a replay computes again, a lookup of a searchable personal field is
//! answered through the blind hash, and a value whose key is gone reads as its token.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::sealed::{
    is_sealed, CipherSession, FieldCipher, SealedStore, TOKEN_PREFIX,
};
use strata_storage_api::{conformance, execute, Store, StoreResult};

/// Seals by writing the binding and the value in hex: enough to test where values go.
#[derive(Default)]
struct StandIn {
    destroyed: Mutex<BTreeSet<String>>,
}

impl StandIn {
    fn destroy(&self, subject: &str) {
        self.destroyed.lock().expect("lock").insert(key_of(subject));
    }

    fn is_destroyed(&self, key_id: &str) -> bool {
        self.destroyed.lock().expect("lock").contains(key_id)
    }
}

fn hex(text: &str) -> String {
    text.bytes().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<String> {
    let bytes: Option<Vec<u8>> = (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok())
        .collect();
    String::from_utf8(bytes?).ok()
}

fn key_of(subject: &str) -> String {
    format!("k{}", hex(subject))
}

struct Session<'a> {
    cipher: &'a StandIn,
    tenant: &'a TenantId,
}

impl CipherSession for Session<'_> {
    fn seal_one(
        &mut self,
        subject: &PrincipalId,
        scope: &str,
        name: &str,
        plain: &str,
        blinds: &str,
        _create: bool,
    ) -> StoreResult<Option<(String, String)>> {
        let key_id = key_of(subject.as_str());
        if self.cipher.is_destroyed(&key_id) {
            return Ok(None);
        }
        let payload = hex(&format!(
            "{}|{key_id}|{scope}|{name}|{blinds}|{plain}",
            self.tenant
        ));
        Ok(Some((key_id, payload)))
    }

    fn open_one(
        &mut self,
        scope: &str,
        name: &str,
        key_id: &str,
        blinds: &str,
        payload: &str,
    ) -> StoreResult<Option<String>> {
        if self.cipher.is_destroyed(key_id) {
            return Ok(None);
        }
        let binding = format!("{}|{key_id}|{scope}|{name}|{blinds}|", self.tenant);
        Ok(unhex(payload).and_then(|text| text.strip_prefix(&binding).map(str::to_string)))
    }

    fn blind(
        &mut self,
        scope: &str,
        name: &str,
        plain: &str,
        _create: bool,
    ) -> StoreResult<Option<String>> {
        let hash = canon::hash_bytes(format!("{}|{scope}|{name}|{plain}", self.tenant).as_bytes());
        Ok(Some(hash[..16].to_string()))
    }
}

impl FieldCipher for StandIn {
    fn session<'a>(
        &'a self,
        _store: &'a dyn Store,
        tenant: &'a TenantId,
    ) -> Box<dyn CipherSession + 'a> {
        Box::new(Session {
            cipher: self,
            tenant,
        })
    }
}

fn sealed_store() -> (SealedStore, Arc<StandIn>) {
    let cipher = Arc::new(StandIn::default());
    (
        SealedStore::new(Arc::new(MemoryStore::new()), cipher.clone()),
        cipher,
    )
}

fn table() -> Vec<String> {
    let spec = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../specifications/spec.jsonc"
    ))
    .expect("the specification file");
    conformance::spec_cases(&spec)
}

/// Verifies: STO-001
#[test]
fn the_sealed_store_conforms() {
    let mut ran = conformance::run(&|| Box::new(sealed_store().0) as Box<dyn Store>);
    let mut rows = table();
    ran.sort();
    rows.sort();
    assert_eq!(ran, rows);
    conformance::audit_is_kept_per_tenant(&sealed_store().0);
    conformance::documents_are_scoped_to_their_tenant_and_kind(&sealed_store().0);
}

fn personal(id: &str, searchable: bool) -> FieldDef {
    FieldDef {
        personal: Some(PersonalDeclaration {
            kind: PersonalKind::EmailAddress,
            subject: DataSubject::Field {
                field: field("person"),
            },
            searchable,
        }),
        ..field_def(id, FieldType::Str, false)
    }
}

/// A tenant `alpha` whose `profile` has `person`, a searchable personal `email` and a personal
/// `bio`; Ada's profile is created and its transition returned.
fn tenant_with_a_profile(store: &dyn Store) -> Transition {
    execute(
        store,
        &Intent::CreateTenant {
            name: "alpha".into(),
            owner: principal_id("alpha:owner"),
        },
        &ctx("alpha", Principal::System, 10),
    )
    .unwrap();
    let profile = object("profile");
    let mut changes = vec![SchemaChange::DefineObject {
        id: profile.clone(),
        label: "Profile".into(),
    }];
    for def in [
        field_def("person", FieldType::Str, false),
        personal("email", true),
        personal("bio", false),
    ] {
        changes.push(SchemaChange::DefineField {
            object: profile.clone(),
            field: def,
        });
    }
    changes.push(SchemaChange::SetRetention {
        object: profile,
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
        &ctx("alpha", user("alpha:owner"), 11),
    )
    .unwrap();
    let fields = [
        ("person", text("alpha:ada")),
        ("email", text("ada@example.org")),
        ("bio", text("a secret biography")),
    ]
    .into_iter()
    .map(|(name, value)| (field(name), value))
    .collect();
    execute(
        store,
        &Intent::CreateRecord {
            object: object("profile"),
            fields,
        },
        &ctx("alpha", user("alpha:owner"), 12),
    )
    .unwrap()
    .transition
}

/// The journal records the hashes of the sealed delta and events, which are the ones a replay
/// computes again from the plain transition, and not the plain ones.
// Verifies: PRV-360
// Verifies: PRV-2722
#[test]
fn the_journal_hashes_the_sealed_form_and_a_replay_computes_it_again() {
    let (store, _) = sealed_store();
    let alpha = tenant_id("alpha");
    let transition = tenant_with_a_profile(&store);
    let entries = store.inner().journal_after(&alpha, 2, 10).unwrap();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_ne!(entry.delta_hash, canon::hash_of(&transition.delta));
    let schema = store
        .load(&alpha, &[])
        .unwrap()
        .tenant
        .unwrap()
        .schema
        .clone();
    let (delta, events) = store
        .journal_hashes(
            &alpha,
            Some(&*schema),
            &transition.delta,
            &transition.events,
        )
        .unwrap()
        .expect("every key is live");
    assert_eq!(entry.delta_hash, delta);
    assert_eq!(entry.events_hash, events);
    // The intent is kept sealed, and opens through the sealed store.
    let stored = serde_json::to_string(&entry.intent).unwrap();
    assert!(!stored.contains("ada@example.org") && stored.contains(TOKEN_PREFIX));
    let opened = serde_json::to_string(&store.journal_after(&alpha, 2, 10).unwrap()).unwrap();
    assert!(opened.contains("ada@example.org"));
}

// Verifies: PRV-1588
// Verifies: PRV-2723
#[test]
fn a_searchable_personal_field_is_found_by_its_blind_hash_and_another_by_a_scan() {
    let (store, _) = sealed_store();
    let alpha = tenant_id("alpha");
    tenant_with_a_profile(&store);
    for (name, value, found) in [
        ("email", "ada@example.org", 1),
        ("email", "grace@example.org", 0),
        ("bio", "a secret biography", 1),
    ] {
        let state = store
            .load(
                &alpha,
                &[Need::ByField {
                    object: object("profile"),
                    field: field(name),
                    value: text(value),
                }],
            )
            .unwrap();
        let tenant = state.tenant.unwrap();
        assert_eq!(
            tenant.records_of(&object("profile")).count(),
            found,
            "{name} = {value}"
        );
        assert!(tenant
            .records_of(&object("profile"))
            .all(|record| !record.fields.values().any(is_sealed)));
    }
}

// Verifies: PRV-361
// Verifies: PRV-2721
#[test]
fn a_value_whose_key_is_destroyed_reads_as_its_token_and_the_read_goes_on() {
    let (store, cipher) = sealed_store();
    let alpha = tenant_id("alpha");
    tenant_with_a_profile(&store);
    cipher.destroy("alpha:ada");
    let state = store.load_complete(&alpha).unwrap().tenant.unwrap();
    let record = state.records_of(&object("profile")).next().unwrap();
    assert!(is_sealed(record.get(&field("email"))));
    assert_eq!(record.get(&field("person")), &text("alpha:ada"));
    let events = serde_json::to_string(&store.events_after(&alpha, 0, 100).unwrap()).unwrap();
    assert!(!events.contains("ada@example.org") && events.contains(TOKEN_PREFIX));
    // A replay cannot seal Ada's values again, and says so instead of a wrong hash.
    let blank = Transition {
        delta: StateDelta {
            tenant: alpha.clone(),
            base_revision: None,
            new_revision: StateRevision(1),
            next_record: 2,
            event_seq: 0,
            ops: vec![DeltaOp::PutRecord(Arc::new(Record {
                fields: [
                    (field("person"), text("alpha:ada")),
                    (field("email"), text("ada@example.org")),
                ]
                .into_iter()
                .collect(),
                ..Record::clone(record)
            }))],
        },
        events: Vec::new(),
        effects: Vec::new(),
        evidence: Vec::new(),
        output: Output::Member,
    };
    assert_eq!(
        store
            .journal_hashes(&alpha, Some(&*state.schema), &blank.delta, &blank.events)
            .unwrap(),
        None
    );
}
