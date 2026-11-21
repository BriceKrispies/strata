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
