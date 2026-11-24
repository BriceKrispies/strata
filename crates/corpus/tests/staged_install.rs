//! A product over the schema-change cap installs through the planner in stages (D-DS-16, WG-5472).
//! The product is a committed fixture shaped like the showcase community product: about 300 schema
//! changes, so one commit cannot hold it. The loader plans it, commits each stage under the approval
//! its install gives, and the tenant holds every entity the product owns at the end. No stage file
//! is read: the stages are the planner's.

use std::path::PathBuf;
use strata_corpus::{load_corpus, verify_loaded, CompiledProduct, LoadOptions};
use strata_storage_api::memory::MemoryStore;

fn over_cap_product() -> CompiledProduct {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("club-over-cap.bundle.json");
    let bundle =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    CompiledProduct {
        id: "club".to_string(),
        file: "club.jsonc".to_string(),
        bundle,
    }
}

#[test]
fn a_product_over_the_cap_installs_in_stages_through_the_planner() {
    let products = [over_cap_product()];
    let store = MemoryStore::new();
    let loaded =
        load_corpus(&store, &LoadOptions::default(), &products).unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(loaded.products.len(), 1);
    assert_eq!(
        loaded.products[0].version, "1.0.0",
        "the report shows the version only"
    );
    assert!(
        loaded.commits >= 3,
        "the product is over the cap, so its install is committed in stages: {} commits",
        loaded.commits
    );

    let verification =
        verify_loaded(&store, &loaded.tenant, &products).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        verification.missing.is_empty(),
        "owned but absent after the staged install: {:?}",
        verification.missing
    );
    assert!(
        verification.owned >= 200,
        "the product owns {} entities",
        verification.owned
    );
}

#[test]
fn the_fixture_is_over_the_cap_and_its_last_stage_is_within_it() {
    use strata_corpus::load_corpus as load;
    use strata_kernel::{limits::MAX_SCHEMA_CHANGES, Principal, PrincipalId};
    use strata_storage_api::Store;
    use strata_substrate::bundle::{plan_install, Planned, ProductBundle, ProductTarget};
    use strata_substrate::stages::plan_stages;

    // A tenant with no product yet: the schema an install plans against.
    let store = MemoryStore::new();
    let empty = load(&store, &LoadOptions::default(), &[]).unwrap_or_else(|e| panic!("{e}"));
    let state = store
        .load(&empty.tenant, &[])
        .unwrap_or_else(|e| panic!("{e}"));
    let schema = state.tenant.expect("the tenant exists").schema.clone();

    let product = over_cap_product();
    let bundle = ProductBundle::from_json(&product.bundle).unwrap_or_else(|e| panic!("{e}"));
    let target = ProductTarget {
        tenant: &empty.tenant,
        principal: Principal::User(PrincipalId::new("user:admin").unwrap()),
        now: 1_800_000_000_000,
        release: "0.2-draft",
        schema: &schema,
        installed: None,
        expected_version: None,
        idempotency_key: None,
    };
    let planned = plan_install(&bundle, &target).unwrap_or_else(|e| panic!("{e}"));
    let Planned::Change(change) = planned else {
        panic!("the fixture has changes to install");
    };
    assert!(
        change.changes.len() > MAX_SCHEMA_CHANGES,
        "{} changes",
        change.changes.len()
    );
    let plan = plan_stages(&change, "club", None, &schema)
        .unwrap_or_else(|e| panic!("{e}"))
        .expect("the install is over the cap");
    assert!(plan.stages.len() >= 3, "{} stages", plan.stages.len());
    for stage in &plan.stages {
        assert!(
            stage.changes.len() <= MAX_SCHEMA_CHANGES,
            "{} holds {}",
            stage.version,
            stage.changes.len()
        );
    }
    let last = plan.stages.last().expect("a last stage");
    assert_eq!(last.version, "1.0.0");
    assert!(last.changes.len() <= MAX_SCHEMA_CHANGES);
}
