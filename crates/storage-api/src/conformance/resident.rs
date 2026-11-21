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

use super::*;
use crate::resident::ResidentStore;
use crate::{execute, Generation};
use std::collections::BTreeSet;
use std::sync::Arc;

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
