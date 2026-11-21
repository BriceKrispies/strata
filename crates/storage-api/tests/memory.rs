//! The in-memory reference adapter passes the conformance suite.

use strata_kernel::testkit::*;
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{conformance, Store};

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
