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
//! This test only reads the files; it never writes them, whatever the environment says. They are
//! written by the separate example `cargo run -p strata-storage-sqlite --example record_data`
//! (`examples/record_data.rs`). They were recorded with the hand-written types (the commit before
//! the kernel adopted the generated ones) and must not be re-recorded from the generated types:
//! that would compare the generated types with themselves.

use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::*;
use strata_storage_api::JournalEntry;

mod recorded_support;
use recorded_support::{dir, files, notes, publication};

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
            "{} is missing ({e}); see the header of this file for how it was recorded",
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
