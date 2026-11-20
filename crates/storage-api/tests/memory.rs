//! The in-memory reference adapter passes the conformance suite.

use strata_kernel::testkit::*;
use strata_kernel::{Intent, Principal};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{conformance, execute, Store};

/// The cases of the conformance table, as the specification states them.
fn table() -> Vec<String> {
    let spec = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../specifications/spec.jsonc"
    ))
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

/// The identity service's documents are kept per tenant and kind.
#[test]
fn memory_store_keeps_documents() {
    conformance::documents_are_scoped_to_their_tenant_and_kind(&MemoryStore::new());
}

/// The jobs of the automation worker and its run, step and exception records.
#[test]
fn memory_store_keeps_jobs() {
    conformance::jobs_conform(&MemoryStore::new());
}

/// The security audit trail is kept per tenant.
#[test]
fn memory_store_keeps_the_audit_trail() {
    conformance::audit_is_kept_per_tenant(&MemoryStore::new());
}

#[test]
fn the_table_is_read_from_its_markdown() {
    let text = "{\"id\":\"cases-storage-conformance\",\"body\":\"# T\\n\\n## Cases\\n\\n| Case | Establishes |\\n|---|---|\\n| one | a |\\n| two | b |\\n\"},";
    assert_eq!(conformance::spec_cases(text), ["one", "two"]);
}

#[test]
fn memory_store_says_it_is_not_durable() {
    assert!(!MemoryStore::new().capabilities().durable);
}

/// The journal is kept for good: the address the shell verified for the acting person is used for
/// the decision and is not written to it.
#[test]
fn the_journal_does_not_keep_the_verified_address() {
    let store = MemoryStore::new();
    let mut context = ctx("alpha", Principal::System, 10);
    context.verified_address = Some("person@example.org".into());
    let intent = Intent::CreateTenant {
        name: "alpha".into(),
        owner: principal_id("alpha:owner"),
    };
    execute(&store, &intent, &context).unwrap();
    let journal = store.journal_after(&tenant_id("alpha"), 0, 10).unwrap();
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].context.verified_address, None);
    assert_eq!(journal[0].context.correlation, context.correlation);
}
