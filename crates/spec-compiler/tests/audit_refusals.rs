//! Refusals of the audit of specc (WG-5021): a repeated object key, a repeated id in the access
//! model, a type pattern matched against a substring, and generated files that share a path.
//! Negative fixtures are in `tests/fixtures/refusals/`.

use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};
use strata_spec_compiler::layers::{self, ProductFile};
use strata_spec_compiler::{
    generate, jsonc, output, Diagnostic, GeneratedFile, Options, RuntimeDependency,
};

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

#[test]
fn a_type_pattern_is_matched_against_the_whole_value() {
    let text = spec(json!({ "types": [
        { "id": "short-code", "name": "Short code", "kind": "id", "repr": "string",
          "constraints": { "pattern": "[a-z]{3}" } }
    ] }));
    let files = generate("spec.jsonc", &text, &options()).unwrap();
    let types = files.iter().find(|f| f.path == "src/types.rs").unwrap();
    // `abc!!!` contains a match of `[a-z]{3}`; only the anchored pattern refuses it.
    assert!(
        types
            .contents
            .contains(r#"Regex::new("\\A(?:[a-z]{3})\\z")"#),
        "the type pattern is not anchored:\n{}",
        types.contents
    );
    assert!(!types.contents.contains(r#"Regex::new("[a-z]{3}")"#));
    let anchored = regex::Regex::new(&strata_spec_compiler::rules::anchored("[a-z]{3}")).unwrap();
    assert!(anchored.is_match("abc") && !anchored.is_match("abc!!!"));
}

#[test]
fn a_repeated_key_in_the_specification_is_refused_with_its_line() {
    let diagnostics = refuse(&fixture("duplicate-key.jsonc"));
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].code, jsonc::DUPLICATE_KEY);
    assert!(
        diagnostics[0].message.contains("line 5"),
        "{}",
        diagnostics[0]
    );
    assert!(
        diagnostics[0].message.contains("\"types\""),
        "{}",
        diagnostics[0]
    );
}

#[test]
fn a_repeated_key_in_a_product_file_is_refused_with_its_line() {
    let product = ProductFile {
        path: "dup.jsonc".to_string(),
        text: fixture("duplicate-key-product.jsonc"),
        file: None,
    };
    let diagnostics =
        layers::load_layered("spec.jsonc", &spec(json!({})), None, &[product]).unwrap_err();
    let found = diagnostics
        .iter()
        .find(|d| d.code == jsonc::DUPLICATE_KEY)
        .unwrap_or_else(|| panic!("no duplicate-key diagnostic: {diagnostics:?}"));
    assert!(found.message.contains("line 7"), "{found}");
}

#[test]
fn a_repeated_id_in_the_access_model_is_refused() {
    let cases = [
        (
            "capabilities",
            json!({ "id": "dup-id", "name": "C", "description": "d" }),
        ),
        ("plans", json!({ "id": "dup-id", "capabilities": [] })),
        (
            "roles",
            json!({ "id": "dup-id", "level": "tenant", "holders": ["person"], "capabilities": [] }),
        ),
        (
            "grants",
            json!({ "id": "dup-id", "issuer": "a", "grantee": "b", "capabilities": [] }),
        ),
        (
            "tokenScopes",
            json!({ "id": "dup-id", "name": "T", "capabilities": [] }),
        ),
        (
            "delegations",
            json!({ "id": "dup-id", "from": "r", "capabilities": [] }),
        ),
        (
            "featureFlags",
            json!({ "id": "dup-id", "gates": [], "default": "off" }),
        ),
    ];
    for (section, entity) in cases {
        let text = spec(json!({ section: [entity.clone(), entity] }));
        let diagnostics = refuse(&text);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == "specc.duplicate-id" && d.entity == "dup-id"),
            "{section}: {diagnostics:?}"
        );
    }
}

fn collision_count(paths: &[&str]) -> usize {
    let files: Vec<GeneratedFile> = paths
        .iter()
        .map(|p| GeneratedFile {
            path: (*p).to_string(),
            contents: String::new(),
        })
        .collect();
    let found = output::path_collisions(&files);
    assert!(found.iter().all(|d| d.code == output::PATH_COLLISION));
    found.len()
}

#[test]
fn generated_files_that_share_a_path_are_refused() {
    assert_eq!(collision_count(&["src/a.rs", "src/b.rs", "forms.js"]), 0);
    // One path twice.
    assert_eq!(collision_count(&["forms.js", "forms.js"]), 1);
    // Paths that differ only in letter case.
    assert_eq!(collision_count(&["A.spec.js", "a.spec.js"]), 1);
    // Windows device names, with and without an extension and in a directory.
    assert_eq!(collision_count(&["con.spec.js"]), 1);
    assert_eq!(collision_count(&["src/AUX.rs"]), 1);
    assert_eq!(collision_count(&["lpt1"]), 1);
    assert_eq!(collision_count(&["console.js", "com10.js"]), 0);
    // A name Windows would shorten.
    assert_eq!(collision_count(&["name."]), 1);
}

#[test]
fn a_client_machine_may_not_take_a_kernel_file_name() {
    for name in [
        "routes.js",
        "contracts.js",
        "forms.js",
        "actions.js",
        "live.js",
        "site-path.js",
        "errors.js",
    ] {
        assert!(
            strata_spec_compiler::web_kernel::KERNEL_FILES.contains(&name),
            "{name}"
        );
    }
}

#[test]
fn a_language_may_not_take_the_name_of_a_backend_module() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/language/spec.jsonc");
    let text = fs::read_to_string(path).unwrap();
    for module in [
        "routes",
        "contracts",
        "records",
        "derived",
        "target_machines",
        "functions",
        "lib",
    ] {
        let renamed = text.replacen(
            "\"id\": \"rule-language\"",
            &format!("\"id\": \"{module}\""),
            1,
        );
        let diagnostics =
            strata_spec_compiler::generate_core("spec.jsonc", &renamed, "core").unwrap_err();
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == "specc.language-unsupported" && d.entity == module),
            "{module}: {diagnostics:?}"
        );
    }
}
