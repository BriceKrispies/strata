//! Refusals of the audit of specc (WG-5021): a repeated object key, a repeated id in the access
//! model, a type pattern matched against a substring, and generated files that share a path.
//! Negative fixtures are in `tests/fixtures/refusals/`.

use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};
use strata_spec_compiler::{generate, Diagnostic, Options, RuntimeDependency};

fn options() -> Options {
    Options {
        crate_name: "generated".to_string(),
        runtime: RuntimeDependency::Workspace,
    }
}

fn spec(sections: Value) -> String {
    let mut base = json!({
        "exegra": "0.1",
        "system": { "id": "sys", "name": "System" },
        "components": [ { "id": "svc", "name": "Service" } ],
        "types": [],
        "records": [],
        "contracts": []
    });
    if let (Some(base), Some(extra)) = (base.as_object_mut(), sections.as_object()) {
        for (k, v) in extra {
            base.insert(k.clone(), v.clone());
        }
    }
    base.to_string()
}

fn refuse(text: &str) -> Vec<Diagnostic> {
    match generate("spec.jsonc", text, &options()) {
        Ok(_) => panic!("the specification was not refused"),
        Err(diagnostics) => diagnostics,
    }
}

fn fixture(name: &str) -> String {
    fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/refusals")
            .join(name),
    )
    .unwrap()
}

fn codes_of(diagnostics: &[Diagnostic]) -> Vec<(&str, &str)> {
    diagnostics
        .iter()
        .map(|d| (d.code.as_str(), d.entity.as_str()))
        .collect()
}

#[test]
fn a_contract_on_the_step_up_list_without_an_age_and_the_error_is_refused() {
    use strata_spec_compiler::contract_rules::{STEP_UP_MISSING, STEP_UP_UNKNOWN};
    let diagnostics = refuse(&fixture("step-up-missing.jsonc"));
    // Verifies: IDN-1202
    assert_eq!(
        codes_of(&diagnostics),
        [
            (STEP_UP_MISSING, "rotate-client-secret"),
            (STEP_UP_MISSING, "export-tenant"),
            // CT-213 is on the list and the specification has no such contract.
            (STEP_UP_UNKNOWN, "CT-213"),
        ]
    );
}

#[test]
fn a_public_tenant_input_without_the_tenant_mismatch_error_is_refused() {
    use strata_spec_compiler::contract_rules::TENANT_MISMATCH_MISSING;
    let diagnostics = refuse(&fixture("tenant-mismatch-missing.jsonc"));
    // Verifies: TEN-1140
    assert_eq!(
        codes_of(&diagnostics),
        [(TENANT_MISMATCH_MISSING, "export-tenant")]
    );
}

#[test]
fn a_public_contract_that_breaks_the_conventions_is_refused() {
    use strata_spec_compiler::contract_rules::{
        CONVENTION_CONCURRENCY, CONVENTION_ERROR_KIND, CONVENTION_IDEMPOTENCY,
        CONVENTION_PAGINATION,
    };
    let diagnostics = refuse(&fixture("convention-breaks.jsonc"));
    // Verifies: ARC-1599
    let own = codes_of(&diagnostics);
    assert_eq!(
        own,
        [
            (CONVENTION_CONCURRENCY, "save-new-draft"),
            (CONVENTION_PAGINATION, "list-new-things"),
            (CONVENTION_IDEMPOTENCY, "post-new-thing"),
            (CONVENTION_ERROR_KIND, "post-new-thing"),
        ]
    );
    assert!(
        !diagnostics.iter().any(|d| d.entity == "conforming"),
        "{diagnostics:?}"
    );
}

#[test]
fn a_markdown_claim_without_its_bounds_is_refused() {
    use strata_spec_compiler::contract_rules::MARKDOWN_BOUND_MISSING;
    let diagnostics = refuse(&fixture("markdown-bound-missing.jsonc"));
    // Verifies: CMS-1020
    assert_eq!(
        codes_of(&diagnostics),
        [(MARKDOWN_BOUND_MISSING, "cms-1020")]
    );
}

#[test]
fn a_contract_that_revokes_without_naming_the_conferral_rule_is_refused() {
    use strata_spec_compiler::contract_rules::REVOCATION_AUTHORITY_MISSING;
    let diagnostics = refuse(&fixture("revocation-authority-missing.jsonc"));
    // Verifies: ADM-1080
    assert_eq!(
        codes_of(&diagnostics),
        [(REVOCATION_AUTHORITY_MISSING, "revoke-role")]
    );
}

#[test]
fn a_catalogue_claim_without_a_negative_scenario_is_refused() {
    use strata_spec_compiler::contract_rules::CATALOGUE_NEGATIVE_MISSING;
    let diagnostics = refuse(&fixture("catalogue-negative-missing.jsonc"));
    // Verifies: AGT-1209
    assert_eq!(
        codes_of(&diagnostics),
        [(CATALOGUE_NEGATIVE_MISSING, "AGT-1209")]
    );
}
