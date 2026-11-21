//! Conformance of header residency (WG-5390, `docs/working-set.md`, follow-up item 2).
//!
//! Not rows of the conformance table: the adapters' tests call these by name, over a
//! [`ResidentStore`] on the adapter.
//!
//! - [`resident_header_is_decoded_once_per_generation`]: repeated loads decode the header once per
//!   tenant per generation, and every state served equals the unwrapped store's.
//! - [`a_commit_leaves_the_resident_header_current`]: a commit that does not touch the header
//!   carries it to the receipt's generation, and one that changes the schema drops it; either way
//!   the next load equals the unwrapped store's.
//! - [`a_foreign_commit_refreshes_the_resident_header`]: a commit through another handle makes the
//!   next load refresh.
//!
//! And of record residency (WG-5391, follow-up items 3 and 4):
//!
//! - [`resident_records_are_served_at_the_current_generation`]: present and absent records are
//!   served from entries, and what is served equals the unwrapped store's.
//! - [`a_commit_carries_the_resident_records_and_invalidates_what_it_writes`]: a create, an update
//!   and a delete drop exactly their records, a schema change drops all, the rest are carried.
//! - [`a_delete_that_clears_references_invalidates_the_referrers`]: the records a delete clears are
//!   named in the delta and dropped.
//! - [`a_foreign_commit_refreshes_the_resident_records`]: a commit through another handle makes
//!   the next load read again.
//! - [`interleaved_commits_between_loads_return_one_generation`]: a generated schedule of loads and
//!   commits from two handles; every state returned is the unwrapped store's at one generation.
//! - [`resident_bytes_never_exceed_the_budget`]: the same schedule under a small budget.

use super::*;
use crate::resident::ResidentStore;
use crate::{execute, Generation, Invalidated};
use std::collections::BTreeSet;
use std::sync::Arc;
use strata_kernel::{DeltaOp, RecordId, StateDelta, StateRevision, Value};

fn owner(tenant: &str) -> Principal {
    Principal::User(principal_id(&format!("{tenant}:owner")))
}

/// Create a tenant with the notes schema, through `store`.
fn create(store: &dyn Store, tenant: &str, at: i64) {
    execute(
        store,
        &Intent::CreateTenant {
            name: tenant.into(),
            owner: principal_id(&format!("{tenant}:owner")),
        },
        &ctx(tenant, Principal::System, at),
    )
    .expect("the tenant is created");
    change_schema(store, tenant, notes_schema(), at + 1);
}

fn change_schema(store: &dyn Store, tenant: &str, changes: Vec<SchemaChange>, at: i64) {
    execute(
        store,
        &Intent::ChangeSchema {
            changes,
            expected_version: None,
            existing: ExistingData::Reject,
        },
        &ctx(tenant, owner(tenant), at),
    )
    .expect("the schema changes");
}

/// A schema change that is not the one `create` made: an optional field on the notes.
fn add_field(store: &dyn Store, tenant: &str, name: &str, at: i64) {
    change_schema(
        store,
        tenant,
        vec![SchemaChange::DefineField {
            object: object("note"),
            field: field_def(name, FieldType::Int, false),
        }],
        at,
    );
}

fn add_note(store: &dyn Store, tenant: &str, slug: &str, at: i64) {
    execute(
        store,
        &new_note(slug, slug),
        &ctx(tenant, owner(tenant), at),
    )
    .expect("the note is accepted");
}

fn schema_of(state: &State) -> (u64, String) {
    let tenant = state.tenant.as_ref().expect("the tenant exists");
    (tenant.schema.version.0, canon::hash_of(&*tenant.schema))
}

/// What the resident store serves is what the store underneath returns: the state of a complete
/// load and of a partial one, and their generation.
fn assert_serves_what_the_store_returns(resident: &ResidentStore, tenant: &TenantId, why: &str) {
    let plain = resident.inner().load_complete_at(tenant).expect("plain");
    let served = resident.load_complete_at(tenant).expect("served");
    assert_eq!(served, plain, "{why}: a complete load");
    let needs = [Need::AllOf(object("note"))];
    let plain = resident.inner().load_at(tenant, &needs).expect("plain");
    let served = resident.load_at(tenant, &needs).expect("served");
    assert_eq!(served, plain, "{why}: a partial load");
}

/// Repeated loads decode each tenant's header once per generation, and nothing served differs from
/// what the unwrapped store returns.
// Verifies: STO-001
pub fn resident_header_is_decoded_once_per_generation(make: &dyn Fn() -> Arc<dyn Store>) {
    let inner = make();
    let resident = ResidentStore::header_only(inner.clone());
    let tenants = ["alpha", "beta"];
    for (index, name) in tenants.iter().enumerate() {
        create(&resident, name, 10 + 10 * index as i64);
    }
    let sweep = |rounds: usize| {
        let mut seen: BTreeSet<(&str, Generation)> = BTreeSet::new();
        for _ in 0..rounds {
            for name in tenants {
                let id = tenant_id(name);
                let complete = resident.load_complete_at(&id).expect("load_complete_at");
                let partial = resident
                    .load_at(&id, &[Need::AllOf(object("note"))])
                    .expect("load_at");
                assert_eq!(complete.generation, partial.generation);
                let plain = inner.load_complete_at(&id).expect("plain");
                assert_eq!(complete, plain, "{name} is served as the store returns it");
                seen.insert((name, complete.generation.expect("a generation")));
            }
        }
        seen
    };

    // After the schema commits, the first load of each tenant decodes; the rest are hits.
    let base = resident.stats();
    let seen = sweep(10);
    let now = resident.stats();
    assert_eq!(
        seen.len(),
        2,
        "one generation per tenant while nothing writes"
    );
    assert_eq!(now.misses - base.misses, 2, "one miss per tenant");
    assert!(
        now.decodes - base.decodes <= 2,
        "one decode per tenant per generation, got {}",
        now.decodes - base.decodes
    );
    assert_eq!(now.hits - base.hits, 38, "every other load is a hit");
    assert_eq!(now.resident_tenants, 2);
    assert!(now.resident_bytes > 0, "the headers have a size");

    // A commit that leaves the header as it was: the header is carried to the new generation and
    // no load decodes again.
    add_note(&resident, "alpha", "carried", 100);
    let before = resident.stats();
    let seen = sweep(5);
    let after = resident.stats();
    assert_eq!(seen.len(), 2);
    assert_eq!(
        after.decodes, before.decodes,
        "a carried header is not decoded"
    );
    assert_eq!(after.misses, before.misses, "a carried header is a hit");

    // A commit that changes the schema: the next load decodes it, once.
    add_field(&resident, "alpha", "rank", 110);
    let before = resident.stats();
    let seen = sweep(5);
    let after = resident.stats();
    assert_eq!(seen.len(), 2);
    assert!(
        after.decodes - before.decodes <= 1,
        "one decode for the one changed generation"
    );
    assert_eq!(after.misses - before.misses, 1, "one miss, for alpha");
}

/// A commit that does not touch the header carries it to the receipt's generation; one that
/// changes the schema drops it; the next load equals the unwrapped store's either way.
// Verifies: STO-001
pub fn a_commit_leaves_the_resident_header_current(make: &dyn Fn() -> Arc<dyn Store>) {
    let inner = make();
    let resident = ResidentStore::header_only(inner.clone());
    let alpha = tenant_id("alpha");
    create(&resident, "alpha", 10);
    assert_serves_what_the_store_returns(&resident, &alpha, "after creation");
    let (generation, version, hash) = resident.held_key(&alpha).expect("a header is held");
    assert_eq!(Some(generation), inner.generation(&alpha).unwrap());

    // Not the schema, members, name or settings.
    add_note(&resident, "alpha", "one", 20);
    let (carried, carried_version, carried_hash) = resident
        .held_key(&alpha)
        .expect("the header is carried through a commit that does not touch it");
    assert_eq!(
        Some(carried),
        inner.generation(&alpha).unwrap(),
        "the carried header is at the receipt's generation"
    );
    assert_eq!(carried, generation.raised());
    assert_eq!((carried_version, &carried_hash), (version, &hash));
    let decodes = resident.stats().decodes;
    assert_serves_what_the_store_returns(&resident, &alpha, "after a note");
    assert_eq!(resident.stats().decodes, decodes, "served without decoding");

    // The schema.
    add_field(&resident, "alpha", "rank", 30);
    assert!(
        resident.held_key(&alpha).is_none(),
        "a header the commit changed is dropped, not carried"
    );
    assert_serves_what_the_store_returns(&resident, &alpha, "after a schema change");
    let (generation, new_version, new_hash) = resident.held_key(&alpha).expect("held again");
    assert_eq!(Some(generation), inner.generation(&alpha).unwrap());
    let plain = inner.load_complete_at(&alpha).unwrap();
    assert_eq!(schema_of(&plain.state), (new_version, new_hash.clone()));
    assert!(new_version > version, "the schema version moved");
    assert_ne!(new_hash, hash, "the schema hash moved");
}

/// A commit through another handle on the same storage makes the next load refresh: the header
/// served is the one the foreign commit left. `foreign` is a second handle on what `inner` is
/// over (two stores on the same files), or `inner` itself for an adapter that has no second
/// handle, which still writes around the resident store.
// Verifies: STO-001
pub fn a_foreign_commit_refreshes_the_resident_header(inner: Arc<dyn Store>, foreign: &dyn Store) {
    let resident = ResidentStore::header_only(inner.clone());
    let alpha = tenant_id("alpha");
    create(&resident, "alpha", 10);
    resident.load_complete_at(&alpha).unwrap();
    let held = resident.held_key(&alpha).expect("a header is held");

    add_note(foreign, "alpha", "foreign", 20);
    let before = resident.stats();
    let served = resident.load_complete_at(&alpha).unwrap();
    let after = resident.stats();
    assert_eq!(served, foreign.load_complete_at(&alpha).unwrap());
    assert_eq!(
        after.misses - before.misses,
        1,
        "the held generation is not the store's"
    );
    assert_eq!(after.hits, before.hits);
    let refreshed = resident.held_key(&alpha).expect("held again");
    assert_eq!(Some(refreshed.0), foreign.generation(&alpha).unwrap());
    assert_ne!(refreshed.0, held.0);

    add_field(foreign, "alpha", "rank", 30);
    let served = resident.load_complete_at(&alpha).unwrap();
    let plain = foreign.load_complete_at(&alpha).unwrap();
    assert_eq!(served, plain, "a foreign schema change is served");
    assert!(schema_of(&served.state).0 > held.1);
    assert_eq!(
        resident.held_key(&alpha).map(|k| k.1),
        Some(schema_of(&plain.state).0)
    );
}

// Record residency.

fn rec(id: RecordId) -> Need {
    Need::Record {
        object: object("note"),
        id,
    }
}

fn note_ids(store: &dyn Store, tenant: &TenantId) -> Vec<RecordId> {
    store
        .load_at(tenant, &[Need::AllOf(object("note"))])
        .expect("the notes")
        .state
        .tenant
        .and_then(|state| {
            state
                .records
                .get(&object("note"))
                .map(|notes| notes.keys().copied().collect())
        })
        .unwrap_or_default()
}

fn update_note(store: &dyn Store, tenant: &str, id: RecordId, title: &str, at: i64) {
    execute(
        store,
        &Intent::UpdateRecord {
            object: object("note"),
            id,
            fields: BTreeMap::from([(field("title"), text(title))]),
            expected_version: None,
        },
        &ctx(tenant, owner(tenant), at),
    )
    .expect("the note is updated");
}

fn delete_record(store: &dyn Store, tenant: &str, object_id: &str, id: RecordId, at: i64) {
    execute(
        store,
        &Intent::DeleteRecord {
            object: object(object_id),
            id,
            expected_version: None,
        },
        &ctx(tenant, owner(tenant), at),
    )
    .expect("the record is deleted");
}

fn assert_same_as_store(resident: &ResidentStore, tenant: &TenantId, needs: &[Need], why: &str) {
    let plain = resident.inner().load_at(tenant, needs).expect("plain");
    let served = resident.load_at(tenant, needs).expect("served");
    assert_eq!(served, plain, "{why}");
}

/// Present and absent records are served from entries at the generation the load reads, and what
/// is served equals what the store returns.
// Verifies: STO-001
pub fn resident_records_are_served_at_the_current_generation(make: &dyn Fn() -> Arc<dyn Store>) {
    let inner = make();
    let resident = ResidentStore::new(inner.clone(), 1 << 20);
    let alpha = tenant_id("alpha");
    create(&resident, "alpha", 10);
    for (index, slug) in ["a", "b", "c"].iter().enumerate() {
        add_note(&resident, "alpha", slug, 20 + index as i64);
    }
    let ids = note_ids(inner.as_ref(), &alpha);
    assert_eq!(ids.len(), 3);
    let missing = RecordId(ids.iter().map(|id| id.0).max().unwrap() + 100);
    let needs: Vec<Need> = ids.iter().copied().chain([missing]).map(rec).collect();

    let before = resident.stats();
    assert_same_as_store(&resident, &alpha, &needs, "the first load");
    let first = resident.stats();
    assert_eq!(first.record_misses - before.record_misses, 4);
    assert_eq!(first.record_hits, before.record_hits);
    assert_eq!(
        resident.held_record(&alpha, &object("note"), missing),
        Some(None)
    );
    assert!(resident
        .held_record(&alpha, &object("note"), ids[0])
        .is_some_and(|held| held.is_some()));
    assert_eq!(
        resident.record_stamp(&alpha),
        inner.generation(&alpha).unwrap()
    );

    assert_same_as_store(&resident, &alpha, &needs, "the second load");
    let second = resident.stats();
    assert_eq!(
        second.record_hits - first.record_hits,
        4,
        "present and absent"
    );
    assert_eq!(second.record_misses, first.record_misses);
    assert_eq!(second.hit_saved_rows - first.hit_saved_rows, 4);
    assert!(second.hit_saved_bytes > first.hit_saved_bytes);
    assert_eq!(second.rereads, 0);

    // A mix of held records, a record not held, and an answer that always goes to the store.
    let extra = RecordId(missing.0 + 1);
    let mixed: Vec<Need> = needs
        .iter()
        .cloned()
        .chain([rec(extra), Need::AllOf(object("note"))])
        .collect();
    assert_same_as_store(&resident, &alpha, &mixed, "held, missed and answered");
    let third = resident.stats();
    assert_eq!(third.record_hits - second.record_hits, 4);
    assert_eq!(third.record_misses - second.record_misses, 1);
    assert_eq!(
        resident.held_key(&alpha).map(|key| key.0),
        inner.generation(&alpha).unwrap()
    );
    // A complete load is not resident and does not disturb the entries.
    assert_eq!(
        resident.load_complete_at(&alpha).unwrap(),
        inner.load_complete_at(&alpha).unwrap()
    );
    assert!(resident
        .held_record(&alpha, &object("note"), ids[1])
        .is_some());
}

/// A commit through the wrapper drops exactly the records its delta writes, carries the rest to the
/// receipt's generation, and a schema change drops every entry.
// Verifies: STO-001
pub fn a_commit_carries_the_resident_records_and_invalidates_what_it_writes(
    make: &dyn Fn() -> Arc<dyn Store>,
) {
    let inner = make();
    let resident = ResidentStore::new(inner.clone(), 1 << 20);
    let alpha = tenant_id("alpha");
    let note = object("note");
    create(&resident, "alpha", 10);
    for (index, slug) in ["a", "b", "c"].iter().enumerate() {
        add_note(&resident, "alpha", slug, 20 + index as i64);
    }
    let ids = note_ids(inner.as_ref(), &alpha);
    let top = ids.iter().map(|id| id.0).max().unwrap();
    let future: Vec<RecordId> = (1..=3).map(|n| RecordId(top + n)).collect();
    let needs: Vec<Need> = ids.iter().chain(future.iter()).copied().map(rec).collect();
    assert_same_as_store(&resident, &alpha, &needs, "warm");
    let held = |id: RecordId| resident.held_record(&alpha, &note, id);

    // A create: the id it takes was a held absence, and goes; every other entry is carried.
    add_note(&resident, "alpha", "d", 100);
    let after = note_ids(inner.as_ref(), &alpha);
    let created = *after
        .iter()
        .find(|id| !ids.contains(id))
        .expect("a new note");
    assert_eq!(
        held(created),
        None,
        "the record the commit wrote is dropped"
    );
    for id in ids.iter().chain(future.iter().filter(|id| **id != created)) {
        assert!(held(*id).is_some(), "{id:?} is carried");
    }
    assert_eq!(
        resident.record_stamp(&alpha),
        inner.generation(&alpha).unwrap()
    );
    assert_same_as_store(&resident, &alpha, &needs, "after a create");
    assert!(
        held(created).is_some_and(|record| record.is_some()),
        "read again"
    );

    // An update drops its record only.
    update_note(&resident, "alpha", ids[0], "changed", 110);
    assert_eq!(held(ids[0]), None);
    assert!(held(ids[1]).is_some() && held(ids[2]).is_some());
    assert_eq!(
        resident.record_stamp(&alpha),
        inner.generation(&alpha).unwrap()
    );
    assert_same_as_store(&resident, &alpha, &needs, "after an update");

    // A delete drops its record only, and a held absence takes its place when it is read again.
    delete_record(&resident, "alpha", "note", ids[1], 120);
    assert_eq!(held(ids[1]), None);
    assert!(held(ids[2]).is_some());
    assert_same_as_store(&resident, &alpha, &needs, "after a delete");
    assert_eq!(held(ids[1]), Some(None));

    // A schema change is not proved untouched by a record key: everything goes.
    add_field(&resident, "alpha", "rank", 130);
    assert_eq!(resident.record_stamp(&alpha), None);
    assert_eq!(held(ids[2]), None);
    assert_same_as_store(&resident, &alpha, &needs, "after a schema change");

    // The map itself.
    let state = inner.load_complete_at(&alpha).unwrap().state;
    let record = state.tenant.unwrap().records[&note][&ids[2]].clone();
    let delta = StateDelta {
        tenant: alpha.clone(),
        base_revision: None,
        new_revision: StateRevision(0),
        next_record: 0,
        event_seq: 0,
        ops: vec![
            DeltaOp::PutRecord(record),
            DeltaOp::DeleteRecord {
                object: note.clone(),
                id: ids[0],
            },
        ],
    };
    assert_eq!(
        crate::invalidated_by(&delta),
        Invalidated::Keys(
            [(note.clone(), ids[2]), (note.clone(), ids[0])]
                .into_iter()
                .collect()
        )
    );
}

/// A delete that clears the references to the deleted record writes the referrers, and the
/// invalidation drops them: no resident referrer still holds the reference.
// Verifies: STO-001
pub fn a_delete_that_clears_references_invalidates_the_referrers(
    make: &dyn Fn() -> Arc<dyn Store>,
) {
    let inner = make();
    let resident = ResidentStore::new(inner.clone(), 1 << 20);
    let alpha = tenant_id("alpha");
    let note = object("note");
    create(&resident, "alpha", 10);
    execute(
        &resident,
        &Intent::CreateRecord {
            object: object("person"),
            fields: BTreeMap::from([(field("name"), text("Ada"))]),
        },
        &ctx("alpha", owner("alpha"), 20),
    )
    .expect("a person");
    let person = inner
        .load_at(&alpha, &[Need::AllOf(object("person"))])
        .unwrap()
        .state
        .tenant
        .unwrap()
        .records[&object("person")]
        .keys()
        .copied()
        .next()
        .expect("the person");
    for (index, (slug, author)) in [("one", true), ("two", true), ("three", false)]
        .iter()
        .enumerate()
    {
        let mut fields =
            BTreeMap::from([(field("title"), text(slug)), (field("slug"), text(slug))]);
        if *author {
            fields.insert(field("author"), Value::Ref(person));
        }
        execute(
            &resident,
            &Intent::CreateRecord {
                object: note.clone(),
                fields,
            },
            &ctx("alpha", owner("alpha"), 30 + index as i64),
        )
        .expect("a note");
    }
    let ids = note_ids(inner.as_ref(), &alpha);
    assert_eq!(ids.len(), 3);
    let mut needs: Vec<Need> = ids.iter().copied().map(rec).collect();
    needs.push(Need::Record {
        object: object("person"),
        id: person,
    });
    assert_same_as_store(&resident, &alpha, &needs, "warm");
    let referrers: Vec<RecordId> = ids
        .iter()
        .copied()
        .filter(|id| {
            let state = inner.load_at(&alpha, &[rec(*id)]).unwrap().state;
            state.tenant.unwrap().records[&note][id]
                .fields
                .get(&field("author"))
                .is_some_and(|value| *value == Value::Ref(person))
        })
        .collect();
    assert_eq!(referrers.len(), 2);

    delete_record(&resident, "alpha", "person", person, 100);
    for id in &referrers {
        assert_eq!(
            resident.held_record(&alpha, &note, *id),
            None,
            "a referrer the delete cleared is dropped"
        );
    }
    let untouched: Vec<&RecordId> = ids.iter().filter(|id| !referrers.contains(id)).collect();
    assert_eq!(untouched.len(), 1);
    assert!(resident.held_record(&alpha, &note, *untouched[0]).is_some());
    assert_same_as_store(&resident, &alpha, &needs, "after the delete");
    for id in &referrers {
        let served = resident.load_at(&alpha, &[rec(*id)]).unwrap();
        let record = &served.state.tenant.unwrap().records[&note][id];
        assert_ne!(
            record.fields.get(&field("author")),
            Some(&Value::Ref(person)),
            "the reference is cleared"
        );
    }
}

/// A commit through another handle makes the next load read again: the entries are not served, and
/// the state returned is the other handle's.
// Verifies: STO-001
pub fn a_foreign_commit_refreshes_the_resident_records(inner: Arc<dyn Store>, foreign: &dyn Store) {
    let resident = ResidentStore::new(inner.clone(), 1 << 20);
    let alpha = tenant_id("alpha");
    create(&resident, "alpha", 10);
    add_note(&resident, "alpha", "one", 20);
    add_note(&resident, "alpha", "two", 21);
    let ids = note_ids(inner.as_ref(), &alpha);
    let needs: Vec<Need> = ids.iter().copied().map(rec).collect();
    assert_same_as_store(&resident, &alpha, &needs, "warm");
    assert_same_as_store(&resident, &alpha, &needs, "held");
    let warm = resident.stats();

    update_note(foreign, "alpha", ids[0], "foreign", 30);
    let served = resident.load_at(&alpha, &needs).unwrap();
    assert_eq!(served, foreign.load_at(&alpha, &needs).unwrap());
    let after = resident.stats();
    assert_eq!(after.rereads - warm.rereads, 1);
    assert_eq!(after.record_hits, warm.record_hits, "no stale entry served");
    assert_eq!(
        resident.record_stamp(&alpha),
        foreign.generation(&alpha).unwrap()
    );
    let state = served.state.tenant.unwrap();
    assert_eq!(
        state.records[&object("note")][&ids[0]]
            .fields
            .get(&field("title")),
        Some(&text("foreign"))
    );

    // Held again, and a foreign delete is seen the same way.
    assert_same_as_store(&resident, &alpha, &needs, "refreshed");
    delete_record(foreign, "alpha", "note", ids[1], 40);
    let served = resident.load_at(&alpha, &needs).unwrap();
    assert_eq!(served, foreign.load_at(&alpha, &needs).unwrap());
    assert_eq!(
        resident.held_record(&alpha, &object("note"), ids[1]),
        Some(None)
    );
}

/// A generated schedule: loads of single records (and an answer now and then) through the wrapper,
/// interleaved with creates, updates and deletes from the wrapper and from a second handle. Every
/// state returned equals the unwrapped store's, read at once, and carries its generation. The
/// bytes the wrapper holds never pass `budget`.
fn run_schedule(inner: Arc<dyn Store>, foreign: &dyn Store, seed: u64, steps: usize, budget: u64) {
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n.max(1)
        }
    }
    let resident = ResidentStore::new(inner.clone(), budget);
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let names = ["alpha", "beta"];
    for (index, name) in names.iter().enumerate() {
        create(&resident, name, 10 * (index as i64 + 1));
        add_note(&resident, name, "seed", 15 + index as i64);
    }
    let mut at = 1_000;
    for step in 0..steps {
        at += 10;
        let name = names[rng.below(2) as usize];
        let tenant = tenant_id(name);
        let ids = note_ids(inner.as_ref(), &tenant);
        let top = ids.iter().map(|id| id.0).max().unwrap_or(0);
        let pick = |rng: &mut Rng| {
            if ids.is_empty() || rng.below(4) == 0 {
                RecordId(top + 1 + rng.below(3))
            } else {
                ids[rng.below(ids.len() as u64) as usize]
            }
        };
        match rng.below(10) {
            0..=4 => {
                let mut needs: Vec<Need> =
                    (0..1 + rng.below(4)).map(|_| rec(pick(&mut rng))).collect();
                if rng.below(5) == 0 {
                    needs.push(Need::AllOf(object("note")));
                }
                let served = resident.load_at(&tenant, &needs).expect("served");
                let plain = inner.load_at(&tenant, &needs).expect("plain");
                assert_eq!(
                    served, plain,
                    "seed {seed} step {step}: a load of {needs:?}"
                );
                assert_eq!(served.generation, inner.generation(&tenant).unwrap());
            }
            5 => add_note(&resident, name, &format!("n{at}"), at),
            6 if !ids.is_empty() => {
                // Only a record that exists can be updated.
                let id = ids[rng.below(ids.len() as u64) as usize];
                update_note(&resident, name, id, &format!("t{at}"), at)
            }
            7 if !ids.is_empty() => {
                // Only a record that exists can be deleted.
                let id = ids[rng.below(ids.len() as u64) as usize];
                delete_record(&resident, name, "note", id, at)
            }
            8 => add_note(foreign, name, &format!("f{at}"), at),
            9 if !ids.is_empty() => {
                let id = ids[rng.below(ids.len() as u64) as usize];
                if rng.below(2) == 0 {
                    update_note(foreign, name, id, &format!("f{at}"), at)
                } else {
                    delete_record(foreign, name, "note", id, at)
                }
            }
            _ => {
                let served = resident.load_complete_at(&tenant).expect("served");
                assert_eq!(served, inner.load_complete_at(&tenant).expect("plain"));
            }
        }
        assert!(
            resident.stats().resident_bytes <= budget,
            "seed {seed} step {step}: {} bytes held over a budget of {budget}",
            resident.stats().resident_bytes
        );
    }
}

/// Every state returned while a second handle commits between the loads is the unwrapped store's
/// state at one generation.
// Verifies: STO-001
pub fn interleaved_commits_between_loads_return_one_generation(
    make: &dyn Fn() -> Arc<dyn Store>,
    seeds: u64,
) {
    for seed in 1..=seeds {
        let inner = make();
        run_schedule(inner.clone(), inner.as_ref(), seed, 80, 1 << 24);
    }
}

/// The same schedule under a budget that holds the two headers and a few records: resident bytes
/// never pass the budget, entries are evicted, and what is served stays the store's.
// Verifies: STO-001
pub fn resident_bytes_never_exceed_the_budget(make: &dyn Fn() -> Arc<dyn Store>, seeds: u64) {
    // What the wrapper estimates for the two headers and for a record.
    let probe_store = make();
    let headers = ResidentStore::header_only(probe_store.clone());
    let probe = ResidentStore::new(probe_store.clone(), u64::MAX);
    for name in ["alpha", "beta"] {
        create(&probe, name, 10);
        add_note(&probe, name, "seed", 15);
        headers.load_complete_at(&tenant_id(name)).unwrap();
    }
    let header = headers.stats().resident_bytes / 2;
    let before = probe.stats().resident_bytes;
    for name in ["alpha", "beta"] {
        let ids = note_ids(probe_store.as_ref(), &tenant_id(name));
        let needs: Vec<Need> = ids.into_iter().map(rec).collect();
        probe.load_at(&tenant_id(name), &needs).unwrap();
    }
    let record = (probe.stats().resident_bytes - before).max(1) / 2;
    let budget = 2 * header + 6 * record;
    for seed in 1..=seeds {
        let inner = make();
        run_schedule(inner.clone(), inner.as_ref(), seed, 120, budget);
    }

    // Many more records than fit: the budget holds and the oldest entries make room.
    let inner = make();
    let budget = header + 6 * record;
    let resident = ResidentStore::new(inner.clone(), budget);
    create(&resident, "alpha", 10);
    for n in 0..30 {
        add_note(&resident, "alpha", &format!("n{n}"), 20 + n);
    }
    let alpha = tenant_id("alpha");
    let needs: Vec<Need> = note_ids(inner.as_ref(), &alpha)
        .into_iter()
        .map(rec)
        .collect();
    for _ in 0..3 {
        assert_same_as_store(&resident, &alpha, &needs, "a working set over the budget");
        assert!(resident.stats().resident_bytes <= budget);
    }
    let stats = resident.stats();
    assert!(stats.evictions > 0, "the budget was small enough to evict");
    assert!(stats.resident_records < 30);
    assert!(stats.record_hits > 0, "and what stayed was served");
}
