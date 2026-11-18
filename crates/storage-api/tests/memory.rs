//! The in-memory reference adapter passes the conformance suite.

use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{conformance, Store};

/// Verifies: STO-001
#[test]
fn memory_store_conforms() {
    conformance::run(&|| Box::new(MemoryStore::new()) as Box<dyn Store>);
}

#[test]
fn memory_store_says_it_is_not_durable() {
    assert!(!MemoryStore::new().capabilities().durable);
}
