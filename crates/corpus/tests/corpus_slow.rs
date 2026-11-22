//! The enterprise corpus on real stores: it installs into one tenant, the tenant's schema holds
//! every entity the 14 products own, and a fixed seed gives a byte-identical populated tenant. Specc compiles the 14 product files once
//! per binary; the population runs at the small scale, and the large scale is only planned.

use std::sync::OnceLock;
use strata_corpus::{
    build, compile_corpus, load_corpus, planned_counts, verify_loaded, CompiledProduct,
    CorpusPaths, LoadOptions, PopulateOptions, Scale,
};
use strata_kernel::Timestamp;
use strata_storage_api::memory::MemoryStore;
use strata_storage_sqlite::SqliteStore;

fn products() -> &'static [CompiledProduct] {
    static PRODUCTS: OnceLock<Vec<CompiledProduct>> = OnceLock::new();
    PRODUCTS
        .get_or_init(|| compile_corpus(&CorpusPaths::default()).unwrap_or_else(|e| panic!("{e}")))
}

fn built(store: &dyn strata_storage_api::Store, seed: u64) -> strata_corpus::Built {
    build(
        store,
        products(),
        &LoadOptions::default(),
        &PopulateOptions::new(Scale::Small, seed),
    )
    .unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn the_loaded_tenant_holds_every_entity_of_the_14_products() {
    assert_eq!(products().len(), 14, "one bundle per product file");
    let store = MemoryStore::new();
    let loaded =
        load_corpus(&store, &LoadOptions::default(), products()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(loaded.products.len(), 14);
    let verification =
        verify_loaded(&store, &loaded.tenant, products()).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        verification.missing.is_empty(),
        "owned but absent: {:?}",
        verification.missing
    );
    assert_eq!(verification.owned, loaded.entities());
    assert!(
        verification.owned >= 900,
        "the 14 products own {} entities",
        verification.owned
    );
}

#[test]
fn a_fixed_seed_gives_a_byte_identical_tenant_on_both_stores_and_another_seed_another_tenant() {
    // Two independent populations of one seed on two different stores agree, which shows both
    // that the seed fixes the tenant and that the stores hold the same one.
    let memory = built(&MemoryStore::new(), 7);
    let sqlite =
        SqliteStore::open_in_memory(Timestamp(1_767_225_600_000)).unwrap_or_else(|e| panic!("{e}"));
    let on_sqlite = built(&sqlite, 7);
    assert_eq!(
        memory.hash, on_sqlite.hash,
        "the same seed, the same tenant"
    );
    assert_eq!(
        memory.catalogue.to_json(),
        on_sqlite.catalogue.to_json(),
        "the same seed, the same catalogue"
    );
    let other = built(&MemoryStore::new(), 8);
    assert_ne!(memory.hash, other.hash, "another seed, another tenant");
}

#[test]
fn every_object_type_has_planned_records_and_the_large_scale_plans_over_100000() {
    let store = MemoryStore::new();
    let loaded =
        load_corpus(&store, &LoadOptions::default(), products()).unwrap_or_else(|e| panic!("{e}"));
    let large = planned_counts(&store, &loaded.tenant, Scale::Large).unwrap();
    assert_eq!(large.len(), 73, "every object type of the corpus");
    assert!(large.values().all(|&n| n > 0));
    assert!(large.values().sum::<u64>() >= 100_000);
    let small = planned_counts(&store, &loaded.tenant, Scale::Small).unwrap();
    assert!(small.values().sum::<u64>() < large.values().sum::<u64>());
}
