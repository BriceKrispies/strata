//! Data recorded by the hand-written kernel types still decodes, byte for byte.
//!
//! The kernel's data types were hand-written until they were generated from the specification
//! (`strata-model-core`). A store written before that change holds their serde form: SQLite keeps
//! the canonical JSON (`strata_kernel::canon`) of tenant states, schemas, members, records,
//! events and journal entries. This test holds such data, recorded under `tests/recorded/` from
//! the notes fixture of `strata_kernel::testkit` and the publication template, and checks that
//! the kernel's types (now the generated ones) decode every file and encode it again to exactly
//! the same canonical bytes, and that the journal's delta and event hashes are the hashes of the
//! decoded deltas and events. So existing databases, journals and canonical hashes keep working.
//!
//! The files are written by running this test with `STRATA_RECORD=1`. They were recorded with
//! the hand-written types (the commit before the kernel adopted the generated ones) and must not
//! be re-recorded from the generated types: that would compare the generated types with
//! themselves.

use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_storage_api::JournalEntry;
use strata_substrate::bundle::{install, Bundle};

/// Every step of a fixture: the journal entry the store writes, the delta and the events.
#[derive(Default)]
struct Recording {
    world: World,
    journal: Vec<JournalEntry>,
    deltas: Vec<StateDelta>,
    events: Vec<DomainEvent>,
}

impl Recording {
    fn step(&mut self, ctx: Context, intent: Intent) -> Result<Output, DomainError> {
        let (next, outcome) = self.world.step(&intent, &ctx)?;
        self.world = next;
        self.journal.push(JournalEntry {
            tenant: ctx.tenant.clone(),
            base_revision: outcome.delta.base_revision,
            revision: outcome.delta.new_revision,
            kernel_version: KERNEL_VERSION.to_string(),
            intent,
            context: ctx,
            delta_hash: canon::hash_of(&outcome.delta),
            events_hash: canon::hash_of(&outcome.events),
        });
        self.deltas.push(outcome.delta.clone());
        self.events.extend(outcome.events.iter().cloned());
        Ok(outcome.output)
    }

    fn accept(&mut self, ctx: Context, intent: Intent) {
        if let Err(e) = self.step(ctx, intent) {
            panic!("fixture intent rejected: {:?}: {}", e.kind, e.message);
        }
    }
}

/// The notes tenant founded step by step as `testkit::add_notes_tenant` does, then a note
/// created, updated and deleted, so the journal holds every kind of event.
fn notes() -> Recording {
    let mut rec = Recording::default();
    let owner = principal_id("alpha:owner");
    let mut now = 1_000;
    let mut at = |principal: Principal| {
        now += 1;
        ctx("alpha", principal, now)
    };
    rec.accept(
        at(Principal::System),
        Intent::CreateTenant {
            name: "alpha".to_string(),
            owner: owner.clone(),
        },
    );
    let owner = Principal::User(owner);
    rec.accept(
        at(owner.clone()),
        Intent::ChangeSchema {
            changes: notes_schema(),
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    for (name, role_name) in [
        ("editor", "editor"),
        ("writer1", "writer"),
        ("writer2", "writer"),
    ] {
        rec.accept(
            at(owner.clone()),
            Intent::AssignRole {
                principal: principal_id(&format!("alpha:{name}")),
                role: role(role_name),
            },
        );
    }
    rec.accept(at(owner.clone()), new_note("First note", "first-note"));
    let id = rec
        .world
        .tenants
        .values()
        .flat_map(|t| t.records.values().flat_map(|r| r.keys().copied()))
        .max()
        .expect("the note was created");
    rec.accept(
        at(owner.clone()),
        Intent::UpdateRecord {
            object: object("note"),
            id,
            fields: BTreeMap::from([(field("title"), text("First note, revised"))]),
            expected_version: Some(1),
        },
    );
    rec.accept(at(owner.clone()), new_note("Second note", "second-note"));
    rec.accept(
        at(owner),
        Intent::DeleteRecord {
            object: object("note"),
            id,
            expected_version: Some(2),
        },
    );
    rec
}

/// The publication template installed with a small set of records, as the template's own tests
/// install it.
fn publication() -> Recording {
    let mut doc: serde_json::Value =
        serde_json::from_str(include_str!("../../../tenants/_templates/publication.json"))
            .expect("the publication template is JSON");
    let extra = serde_json::json!({
        "id": "gazette",
        "name": "Gazette",
        "owner": "user:admin",
        "created_at": "2026-01-01T00:00:00Z",
        "members": [
            { "principal": "user:ed", "roles": ["editor"] },
            { "principal": "user:ann", "roles": ["author"] }
        ],
        "records": [
            { "object": "publication", "as": "user:admin", "at": "2026-01-02", "fields": { "key": "main", "name": "Gazette" } },
            { "object": "author", "key": "ann", "as": "user:ed", "at": "2026-01-02", "fields": { "name": "Ann", "slug": "ann", "principal": "user:ann" } },
            { "object": "category", "key": "craft", "as": "user:ed", "at": "2026-01-02", "fields": { "name": "Craft", "slug": "craft" } },
            {
                "object": "post", "key": "first", "as": "user:ann", "at": "2026-01-03T10:00:00Z",
                "fields": { "title": "First", "slug": "first", "body": "Hello *there*.", "author": "@ann", "category": "@craft" },
                "then": [
                    { "as": "user:ann", "at": "2026-01-04T08:00:00Z", "set": { "status": "published" } },
                    { "as": "user:ed", "at": "2026-01-04T09:00:00Z", "set": { "featured": true } }
                ]
            }
        ]
    });
    doc.as_object_mut()
        .expect("the template is an object")
        .extend(extra.as_object().expect("an object").clone());
    let bundle = Bundle::from_json(&doc.to_string()).expect("the bundle reads");
    let mut rec = Recording::default();
    install(&bundle, &mut |ctx, intent| rec.step(ctx, intent))
        .unwrap_or_else(|e| panic!("the publication template installs: {e}"));
    rec
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/recorded")
}

/// One file: a value of `T` in the canonical form SQLite stores.
fn file<T: Serialize>(name: &str, value: &T) -> (String, String) {
    (name.to_string(), canon::canonical_json(value) + "\n")
}

/// Every file of a fixture: what each SQLite table holds for it.
fn files(prefix: &str, rec: &Recording) -> Vec<(String, String)> {
    let mut out = vec![
        file(&format!("{prefix}-world.json"), &rec.world),
        file(&format!("{prefix}-journal.json"), &rec.journal),
        file(&format!("{prefix}-deltas.json"), &rec.deltas),
        file(&format!("{prefix}-events.json"), &rec.events),
    ];
    for (id, tenant) in &rec.world.tenants {
        let at = format!("{prefix}-{id}");
        out.push(file(&format!("{at}-state.json"), &**tenant));
        out.push(file(&format!("{at}-schema.json"), &*tenant.schema));
        out.push(file(&format!("{at}-members.json"), &tenant.members));
        let records: Vec<&Record> = tenant
            .records
            .values()
            .flat_map(|r| r.values().map(|record| &**record))
            .collect();
        out.push(file(&format!("{at}-records.json"), &records));
    }
    out
}

/// Decodes `text` as a `T` and encodes it again in canonical form.
fn again<T: Serialize + DeserializeOwned>(name: &str, text: &str) -> String {
    let value: T =
        serde_json::from_str(text).unwrap_or_else(|e| panic!("{name} no longer decodes: {e}"));
    canon::canonical_json(&value) + "\n"
}

fn read(name: &str) -> String {
    let path = dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{} is missing ({e}); record it with STRATA_RECORD=1 from the hand-written kernel types",
            path.display()
        )
    })
}

/// Recorded store data decodes into the generated kernel types and encodes back to the same
/// canonical bytes, and the recorded journal hashes are the hashes of the decoded deltas and
/// events.
#[test]
fn recorded_store_data_decodes_and_encodes_back_byte_for_byte() {
    let fixtures = [("notes", notes()), ("publication", publication())];
    if std::env::var_os("STRATA_RECORD").is_some() {
        std::fs::create_dir_all(dir()).expect("make the recording directory");
        for (prefix, rec) in &fixtures {
            for (name, text) in files(prefix, rec) {
                std::fs::write(dir().join(name), text).expect("write a recorded file");
            }
        }
        return;
    }
    let mut checked = BTreeSet::new();
    for (prefix, rec) in &fixtures {
        for (name, _) in files(prefix, rec) {
            let text = read(&name);
            let kind = name
                .rsplit('-')
                .next()
                .expect("a name has a kind")
                .trim_end_matches(".json");
            let back = match kind {
                "world" => again::<World>(&name, &text),
                "journal" => again::<Vec<JournalEntry>>(&name, &text),
                "deltas" => again::<Vec<StateDelta>>(&name, &text),
                "events" => again::<Vec<DomainEvent>>(&name, &text),
                "state" => again::<TenantState>(&name, &text),
                "schema" => again::<TenantSchema>(&name, &text),
                "members" => again::<BTreeMap<PrincipalId, BTreeSet<RoleId>>>(&name, &text),
                "records" => again::<Vec<Record>>(&name, &text),
                other => panic!("{name}: no type for {other}"),
            };
            assert_eq!(
                back, text,
                "{name} does not encode back to the recorded bytes"
            );
            checked.insert(name);
        }
        let journal: Vec<JournalEntry> =
            serde_json::from_str(&read(&format!("{prefix}-journal.json"))).expect("journal");
        let deltas: Vec<StateDelta> =
            serde_json::from_str(&read(&format!("{prefix}-deltas.json"))).expect("deltas");
        assert_eq!(journal.len(), deltas.len(), "{prefix}: one delta per entry");
        for (entry, delta) in journal.iter().zip(&deltas) {
            assert_eq!(
                canon::hash_of(delta),
                entry.delta_hash,
                "{prefix}: the delta of revision {} hashes as recorded",
                entry.revision.0
            );
        }
        let events: Vec<DomainEvent> =
            serde_json::from_str(&read(&format!("{prefix}-events.json"))).expect("events");
        // The events of an entry are its tenant's events after the previous entry's last
        // sequence number, up to its own delta's.
        let mut last_seq: BTreeMap<&TenantId, u64> = BTreeMap::new();
        let mut matched = 0;
        for (entry, delta) in journal.iter().zip(&deltas) {
            let after = last_seq.get(&entry.tenant).copied().unwrap_or(0);
            let mine: Vec<DomainEvent> = events
                .iter()
                .filter(|e| e.tenant == entry.tenant && e.seq > after && e.seq <= delta.event_seq)
                .cloned()
                .collect();
            matched += mine.len();
            assert_eq!(
                canon::hash_of(&mine),
                entry.events_hash,
                "{prefix}: the events of revision {} hash as recorded",
                entry.revision.0
            );
            last_seq.insert(&entry.tenant, delta.event_seq);
        }
        assert_eq!(
            matched,
            events.len(),
            "{prefix}: every event belongs to an entry"
        );
    }
    assert!(
        checked.len() >= 8,
        "every fixture wrote its files: {checked:?}"
    );
}
