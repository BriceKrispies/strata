//! The in-memory reference adapter passes the conformance suite.

use strata_kernel::testkit::*;
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{conformance, Generation, NonceSource, SeededNonces, Store};

/// The cases of the conformance table, as the specification states them.
fn table() -> Vec<String> {
    let spec = strata_spec_parts::read_spec_text(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../specifications/spec.jsonc"
    )))
    .expect("the specification file");
    conformance::spec_cases(&spec)
}

/// Verifies: STO-001
///
/// Every row of the table in the specification has a check and every check has a row: the cases
/// the suite ran are compared with the rows read from the specification at the time of the run.
#[test]
fn memory_store_conforms() {
    let ran = conformance::run(&|| Box::new(MemoryStore::new()) as Box<dyn Store>);
    let (mut ran, mut rows) = (ran, table());
    assert!(rows.len() >= 13, "the table was not read: {rows:?}");
    ran.sort();
    rows.sort();
    assert_eq!(
        ran, rows,
        "the checks that ran are not the rows of the conformance table"
    );
}

/// The pair index of named queries follows writes and query definitions.
// Verifies: KRN-2921
#[test]
fn memory_store_answers_pair_pages() {
    conformance::pair_pages_follow_writes_and_definitions(&MemoryStore::new());
}

/// The generation of a tenant follows what a load returns, for every method of the trait; a
/// commit is accepted only at the generation its decision was made on; a load's generation is the
/// generation of its rows under concurrent commits; and a seeded store is reproducible.
// Verifies: STO-001
#[test]
fn memory_store_keeps_the_tenant_generation() {
    conformance::generation_follows_every_store_method(&|| {
        Box::new(MemoryStore::new()) as Box<dyn Store>
    });
    conformance::a_commit_names_the_generation_it_was_decided_on(&MemoryStore::new());
    conformance::interleaved_commits_and_loads_keep_each_revision_with_one_generation(
        &MemoryStore::new(),
    );
    conformance::generations_are_reproducible_from_the_seed(&|seed| {
        Box::new(MemoryStore::with_nonces(SeededNonces::shared(seed))) as Box<dyn Store>
    });
}

/// A seeded nonce is a function of the seed, the tenant and the number of draws for that tenant so
/// far (D19), and the nonce of a generation is written as text.
#[test]
fn seeded_nonces_are_a_function_of_the_seed_the_tenant_and_the_draw() {
    let (alpha, beta) = (tenant_id("alpha"), tenant_id("beta"));
    let (one, same, other) = (
        SeededNonces::new(1),
        SeededNonces::new(1),
        SeededNonces::new(2),
    );
    let first = one.draw(&alpha);
    assert_eq!(first, same.draw(&alpha), "one seed, one nonce");
    let second = one.draw(&alpha);
    assert_ne!(first, second, "the next draw for a tenant is another nonce");
    assert_eq!(second, same.draw(&alpha));
    assert_ne!(first, one.draw(&beta), "the tenant is part of the draw");
    assert_ne!(first, other.draw(&alpha), "the seed is part of the draw");

    let generation = Generation {
        nonce: u128::MAX - 3,
        counter: 9,
    };
    let text = serde_json::to_string(&generation).unwrap();
    assert!(
        text.contains(&format!("{:032x}", generation.nonce)),
        "{text}"
    );
    assert_eq!(
        serde_json::from_str::<Generation>(&text).unwrap(),
        generation
    );
    assert!(serde_json::from_str::<Generation>("{\"nonce\":\"12\",\"counter\":1}").is_err());
}

/// Header residency over the memory adapter: the conformance suite passes through a
/// `ResidentStore`, and the header is decoded once per generation, follows commits and refreshes
/// after a commit that bypasses the wrapper (WG-5390).
// Verifies: STO-001
#[test]
fn resident_store_over_memory_conforms() {
    use std::sync::Arc;
    use strata_storage_api::ResidentStore;
    let resident =
        || Box::new(ResidentStore::header_only(Arc::new(MemoryStore::new()))) as Box<dyn Store>;
    let ran = conformance::run(&resident);
    let (mut ran, mut rows) = (ran, table());
    ran.sort();
    rows.sort();
    assert_eq!(
        ran, rows,
        "the checks that ran are not the rows of the table"
    );
    conformance::generation_follows_every_store_method(&resident);
    let fresh = || Arc::new(MemoryStore::new()) as Arc<dyn Store>;
    conformance::resident_header_is_decoded_once_per_generation(&fresh);
    conformance::a_commit_leaves_the_resident_header_current(&fresh);
    let inner = fresh();
    conformance::a_foreign_commit_refreshes_the_resident_header(inner.clone(), inner.as_ref());
}

/// Record residency over the memory adapter (WG-5391): the conformance suite passes through a
/// `ResidentStore` that holds records under a budget, present and absent records are served at the
/// current generation, a commit drops what it writes, a delete that clears references drops the
/// referrers, a foreign commit is seen, a generated schedule of interleaved commits returns one
/// generation every time, and the budget holds.
// Verifies: STO-001
#[test]
fn resident_store_over_memory_serves_records_at_one_generation() {
    use std::sync::Arc;
    use strata_storage_api::ResidentStore;
    let resident =
        || Box::new(ResidentStore::new(Arc::new(MemoryStore::new()), 1 << 22)) as Box<dyn Store>;
    let ran = conformance::run(&resident);
    let (mut ran, mut rows) = (ran, table());
    ran.sort();
    rows.sort();
    assert_eq!(
        ran, rows,
        "the checks that ran are not the rows of the table"
    );
    conformance::generation_follows_every_store_method(&resident);
    let fresh = || Arc::new(MemoryStore::new()) as Arc<dyn Store>;
    conformance::resident_records_are_served_at_the_current_generation(&fresh);
    conformance::a_commit_carries_the_resident_records_and_invalidates_what_it_writes(&fresh);
    conformance::a_delete_that_clears_references_invalidates_the_referrers(&fresh);
    let inner = fresh();
    conformance::a_foreign_commit_refreshes_the_resident_records(inner.clone(), inner.as_ref());
    conformance::interleaved_commits_between_loads_return_one_generation(&fresh, 25);
    conformance::resident_bytes_never_exceed_the_budget(&fresh, 12);
}

/// Simultaneous misses on one record are coalesced (CPY-012): the tenant's lock is held across the
/// load, so one thread reads the record and the others find its entry.
// Verifies: STO-001
#[test]
fn simultaneous_misses_on_one_record_are_one_load() {
    use std::sync::{Arc, Barrier};
    use strata_kernel::{Intent, Need, Principal, RecordId};
    use strata_storage_api::{execute, ResidentStore};
    let inner: Arc<dyn Store> = Arc::new(MemoryStore::new());
    let resident = Arc::new(ResidentStore::new(inner, 1 << 22));
    let alpha = tenant_id("alpha");
    execute(
        resident.as_ref(),
        &Intent::CreateTenant {
            name: "alpha".into(),
            owner: principal_id("alpha:owner"),
        },
        &ctx("alpha", Principal::System, 1),
    )
    .unwrap();
    let before = resident.stats();
    let threads = 8;
    let barrier = Arc::new(Barrier::new(threads));
    let workers: Vec<_> = (0..threads)
        .map(|_| {
            let (resident, barrier, alpha) = (resident.clone(), barrier.clone(), alpha.clone());
            std::thread::spawn(move || {
                barrier.wait();
                let needs = [Need::Record {
                    object: object("note"),
                    id: RecordId(900),
                }];
                resident.load_at(&alpha, &needs).unwrap()
            })
        })
        .collect();
    let loaded: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert!(loaded.windows(2).all(|pair| pair[0] == pair[1]));
    let after = resident.stats();
    assert_eq!(after.record_misses - before.record_misses, 1);
    assert_eq!(after.record_hits - before.record_hits, threads as u64 - 1);
}

/// A ledger transaction is audited in its commit and a restore cannot drop it.
// Verifies: LDG-2925, LDG-2923, LDG-2911
#[test]
fn memory_store_audits_a_transaction_and_refuses_a_restore_that_drops_it() {
    conformance::a_ledger_transaction_is_audited_in_its_commit_and_a_restore_cannot_drop_it(
        &MemoryStore::new(),
    );
}
