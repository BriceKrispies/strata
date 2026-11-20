//! Product specifications in files of their own, layered on the platform specification
//! (`strata_spec_compiler::layers`, WG-4989).
//!
//! - `tests/fixtures/products/badges.jsonc` imports `specifications/spec.jsonc` and is accepted by
//!   specc's loader and by Exegra, and the platform it is loaded with is exactly the platform
//!   loaded alone.
//! - `tests/fixtures/products/refused/` holds one product per refusal; specc and Exegra refuse each
//!   with the same named rule, whose message names what is wrong and says what to do.
//! - The files specc writes are byte-identical with and without `--product`, and Exegra reports
//!   the same platform semantic hash (the one the generated headers carry) with and without the
//!   product.
//!
//! The Exegra checks need the Exegra CLI (see `support`).

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use strata_spec_compiler::layers::{self, ProductFile};
use strata_spec_compiler::load;

const PLATFORM: &str = "specifications/spec.jsonc";
const PRODUCTS: &str = "crates/spec-compiler/tests/fixtures/products";

fn root() -> PathBuf {
    support::repo_root()
}

fn platform_text() -> String {
    fs::read_to_string(root().join(PLATFORM)).unwrap()
}

fn product(relative: &str) -> ProductFile {
    let file = root().join(PRODUCTS).join(relative);
    ProductFile {
        path: format!("{PRODUCTS}/{relative}"),
        text: fs::read_to_string(&file).unwrap(),
        file: Some(file),
    }
}

fn load_with(
    products: &[ProductFile],
) -> Result<layers::Layered, Vec<strata_spec_compiler::Diagnostic>> {
    layers::load_layered(
        PLATFORM,
        &platform_text(),
        Some(&root().join(PLATFORM)),
        products,
    )
}

#[test]
fn the_fixture_product_is_accepted_beside_the_platform() {
    let layered = match load_with(&[product("badges.jsonc")]) {
        Ok(layered) => layered,
        Err(diagnostics) => panic!(
            "the fixture product was refused:\n{}",
            diagnostics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        ),
    };
    assert_eq!(layered.products.len(), 1);
    let badges = &layered.products[0];
    assert_eq!(badges.id, "badges");
    assert_eq!(
        badges.imports,
        vec!["content-read", "content-write", "product-module"]
    );
    let ids: Vec<&str> = badges.entities.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "badges-tier",
            "badges-curator",
            "badges-installed-as-metadata"
        ]
    );
}

#[test]
fn the_parsed_platform_is_unchanged_by_layering() {
    let alone = load::load(PLATFORM, &platform_text()).expect("the platform loads");
    let layered = load_with(&[product("badges.jsonc")]).expect("the fixture product is accepted");
    assert!(
        layered.platform == alone,
        "the platform loaded with a product differs from the platform loaded alone"
    );
}

/// Each refused fixture, the rule that refuses it, and words its message must hold: the
/// offending id or section, and the fix.
const REFUSED: &[(&str, &str, &[&str])] = &[
    (
        "import-unknown",
        layers::IMPORT_UNKNOWN,
        &[
            "\"tenant-identifier\"",
            "Correct the id",
            "remove it from product.imports.entities",
        ],
    ),
    (
        "redefines-entity",
        layers::REDEFINES_ENTITY,
        &[
            "\"tenant-id\"",
            "already declares",
            "Give the product's entity an id of its own",
        ],
    ),
    (
        "kind-not-allowed",
        layers::KIND_NOT_ALLOWED,
        &[
            "\"functions\"",
            "records, types, screens",
            "belongs in the platform specification",
        ],
    ),
    (
        "import-missing",
        layers::IMPORT_MISSING,
        &[
            "\"tenant-id\"",
            "Add \"tenant-id\" to product.imports.entities",
        ],
    ),
    (
        "header-missing",
        layers::HEADER_MISSING,
        &["no \"product\" header", "add it"],
    ),
    (
        "role-platform-level",
        layers::PLATFORM_POWER,
        &["has level \"platform\"", "set level to \"tenant\""],
    ),
    (
        "role-anonymous-holder",
        layers::PLATFORM_POWER,
        &["lets \"anonymous\" hold it", "remove the holder"],
    ),
    (
        "role-service-holder",
        layers::PLATFORM_POWER,
        &["lets \"service\" hold it", "remove the holder"],
    ),
    (
        "contract-public-anonymous",
        layers::PLATFORM_POWER,
        &["public with the anonymous scheme", "scheme \"session\""],
    ),
    (
        "contract-platform-implemented",
        layers::PLATFORM_POWER,
        &["platform component \"kernel\"", "remove implementedBy"],
    ),
    (
        "contract-realisation",
        layers::PLATFORM_POWER,
        &["realisation \"hand\"", "only the platform puts code"],
    ),
    (
        "behavior-implementation-level",
        layers::PLATFORM_POWER,
        &["level \"implementation\"", "workflow model"],
    ),
    (
        "event-domain",
        layers::PLATFORM_POWER,
        &["is a domain event", "remove \"domain\""],
    ),
    (
        "entity-id-invalid",
        layers::ENTITY_ID_INVALID,
        &["\"BadgesTier\"", "id pattern"],
    ),
    (
        "entity-id-missing",
        layers::ENTITY_ID_MISSING,
        &["has no \"id\"", "add it"],
    ),
    (
        "import-missing-reference",
        layers::IMPORT_MISSING,
        &[
            "\"kernel\"",
            "without importing it",
        ],
    ),
    (
        "import-mismatch",
        layers::IMPORT_MISMATCH,
        &[
            "\"../../full/spec.jsonc\"",
            "Set \"product.imports.spec\" to \"../../../../../../specifications/spec.jsonc\"",
        ],
    ),
];

#[test]
fn each_refused_product_is_refused_by_specc_with_its_rule() {
    for (name, code, words) in REFUSED {
        let diagnostics = match load_with(&[product(&format!("refused/{name}.jsonc"))]) {
            Ok(_) => panic!("refused/{name}.jsonc was accepted"),
            Err(diagnostics) => diagnostics,
        };
        let found = diagnostics
            .iter()
            .find(|d| d.code == *code)
            .unwrap_or_else(|| {
                panic!(
                    "refused/{name}.jsonc was not refused with {code}, but with:\n{}",
                    diagnostics
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            });
        for word in *words {
            assert!(
                found.message.contains(word),
                "the {code} message for refused/{name}.jsonc does not say {word:?}: {}",
                found.message
            );
        }
        assert!(
            diagnostics.iter().all(|d| d.code.starts_with("layer.")),
            "refused/{name}.jsonc has other refusals too: {diagnostics:?}"
        );
    }
}

#[test]
fn import_missing_names_the_capability_too() {
    let diagnostics = load_with(&[product("refused/import-missing.jsonc")]).unwrap_err();
    assert!(diagnostics.iter().any(|d| d.code == layers::IMPORT_MISSING
        && d.message.contains("capability \"content-write\"")
        && d.message
            .contains("Add \"content-write\" to product.imports.entities")));
}

#[test]
fn two_products_declaring_one_id_are_refused() {
    let diagnostics = load_with(&[product("badges.jsonc"), product("badges.jsonc")]).unwrap_err();
    assert!(diagnostics
        .iter()
        .any(|d| d.code == layers::REDEFINES_ENTITY && d.message.contains("already declares it")));
}

#[test]
fn a_reference_key_requires_an_import_for_each_platform_entity() {
    let diagnostics = load_with(&[product("refused/import-missing-reference.jsonc")]).unwrap_err();
    let missing: Vec<&str> = diagnostics
        .iter()
        .filter(|d| d.code == layers::IMPORT_MISSING)
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(missing.len(), 3, "{missing:?}");
    assert!(missing
        .iter()
        .any(|m| m.contains("\"export-tenant\"") && m.contains("at contract")));
    assert!(missing
        .iter()
        .any(|m| m.contains("\"kernel\"") && m.contains("at owner")));
    assert!(missing
        .iter()
        .any(|m| m.contains("\"kernel\"") && m.contains("at subject")));
}

#[test]
fn two_product_files_with_one_product_id_are_refused() {
    let diagnostics = load_with(&[
        product("badges.jsonc"),
        product("refused/duplicate-product-id.jsonc"),
    ])
    .unwrap_err();
    let found = diagnostics
        .iter()
        .find(|d| d.code == layers::PRODUCT_ID_DUPLICATE)
        .unwrap_or_else(|| panic!("no {} in {diagnostics:?}", layers::PRODUCT_ID_DUPLICATE));
    assert!(found.message.contains("\"badges\""));
    assert!(found.message.contains("badges.jsonc already has"));
    assert!(diagnostics.iter().all(|d| d.code.starts_with("layer.")));
}

#[test]
fn an_external_holder_is_a_platform_power() {
    let text = r#"{ "product": { "id": "badges", "name": "Badges", "imports": { "spec": "x.jsonc", "entities": [] } },
        "roles": [ { "id": "badges-hook", "name": "Hook", "level": "tenant", "holders": ["external"], "capabilities": [] } ] }"#;
    let file = ProductFile {
        path: "inline.jsonc".to_string(),
        text: text.to_string(),
        file: None,
    };
    let diagnostics = load_with(&[file]).unwrap_err();
    assert!(diagnostics
        .iter()
        .any(|d| d.code == layers::PLATFORM_POWER && d.message.contains("\"external\"")));
}

#[test]
fn a_platform_entity_that_names_a_product_entity_is_refused() {
    let platform = platform_text();
    let marker = "\"id\": \"content-read\",";
    assert!(platform.contains(marker));
    let edited = platform.replacen(
        marker,
        "\"id\": \"content-read\", \"contract\": \"badges-tier\",",
        1,
    );
    let diagnostics = layers::load_layered(
        PLATFORM,
        &edited,
        Some(&root().join(PLATFORM)),
        &[product("badges.jsonc")],
    )
    .unwrap_err();
    let found = diagnostics
        .iter()
        .find(|d| d.code == layers::PLATFORM_REFERENCES_PRODUCT)
        .unwrap_or_else(|| {
            panic!(
                "no {} in {diagnostics:?}",
                layers::PLATFORM_REFERENCES_PRODUCT
            )
        });
    assert!(found.message.contains("\"content-read\""));
    assert!(found.message.contains("\"badges-tier\""));
    assert!(found.message.contains("badges.jsonc"));
    // The unedited platform with the same product is accepted.
    assert!(load_with(&[product("badges.jsonc")]).is_ok());
}

/// The reference-property names of Exegra's meta-model: every property of type `ref`, in a list
/// of refs, or inside an object's fields or a map's values.
fn metamodel_reference_keys(model: &Value) -> std::collections::BTreeSet<String> {
    fn walk(name: &str, prop: &Value, out: &mut std::collections::BTreeSet<String>) {
        if prop["type"] == "ref" {
            out.insert(name.to_string());
        }
        if prop["items"].is_object() {
            walk(name, &prop["items"], out);
        }
        if prop["type"] == "map" && prop["values"].is_object() {
            walk(name, &prop["values"], out);
        }
        if let Some(fields) = prop["fields"].as_object() {
            for (k, v) in fields {
                walk(k, v, out);
            }
        }
    }
    let mut out = std::collections::BTreeSet::new();
    for entity in model["entities"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(_, v)| v)
    {
        for (k, v) in entity["properties"].as_object().into_iter().flatten() {
            walk(k, v, &mut out);
        }
    }
    out
}

#[test]
fn the_reference_keys_are_the_reference_properties_of_the_meta_model() {
    let Some(cli) = support::exegra_cli("the reference key comparison") else {
        return;
    };
    let path = cli
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("src/core/metamodel.json");
    let model: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let in_model = metamodel_reference_keys(&model);
    let mut ours: std::collections::BTreeSet<String> = layers::REFERENCE_KEYS
        .iter()
        .map(|(k, _)| k.to_string())
        .collect();
    for key in layers::UNCHECKED_REFERENCE_KEYS {
        assert!(
            ours.insert(key.to_string()),
            "{key} is both a reference key and unchecked"
        );
    }
    assert_eq!(
        ours, in_model,
        "REFERENCE_KEYS and UNCHECKED_REFERENCE_KEYS must name exactly the reference properties of {}",
        path.display()
    );
    let sections = model["sections"].as_object().unwrap();
    for (key, listed) in layers::REFERENCE_KEYS {
        for section in *listed {
            assert!(
                sections.contains_key(*section) || *section == "ports",
                "{key} lists {section}, which is not a section of the meta-model"
            );
        }
    }
}

// ---- generated files ----------------------------------------------------------------------

/// Runs specc from the repository root and fails with its output unless it succeeds.
fn specc(args: &[&str]) {
    let output = Command::new(env!("CARGO_BIN_EXE_specc"))
        .args(args)
        .current_dir(root())
        .output()
        .expect("specc runs");
    assert!(
        output.status.success(),
        "specc {} failed:\n{}{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Every file under `dir`, relative to it with forward slashes, with its bytes.
fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                let name = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((name, fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out
}

/// Runs one specc command into `var/specc-layers/<name>/alone` and `.../layered` (the second with
/// the product files) and asserts both directories hold the same files, byte for byte.
fn same_output(name: &str, args: &[&str], products: &[&str]) {
    let base = root().join("var").join("specc-layers").join(name);
    let _ = fs::remove_dir_all(&base);
    let alone = base.join("alone");
    let layered = base.join("layered");
    let rel = |p: &Path| {
        p.strip_prefix(root())
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    };
    let (alone_dir, layered_dir) = (rel(&alone), rel(&layered));
    let mut first: Vec<&str> = args.to_vec();
    first.extend(["--out", alone_dir.as_str()]);
    specc(&first);
    let mut second: Vec<&str> = args.to_vec();
    second.extend(["--out", layered_dir.as_str()]);
    for p in products {
        second.extend(["--product", *p]);
    }
    specc(&second);
    let (a, b) = (tree(&alone), tree(&layered));
    assert!(!a.is_empty(), "specc {name} wrote nothing");
    assert_eq!(
        a.iter().map(|(n, _)| n).collect::<Vec<_>>(),
        b.iter().map(|(n, _)| n).collect::<Vec<_>>(),
        "specc {name} wrote other files with a product"
    );
    for ((n, x), (_, y)) in a.iter().zip(&b) {
        assert!(x == y, "specc {name}: {n} differs with a product");
    }
}

#[test]
fn a_product_leaves_the_generated_crate_byte_identical() {
    same_output(
        "generate",
        &[
            "generate",
            "--spec",
            "crates/spec-compiler/tests/fixtures/full/spec.jsonc",
            "--crate-name",
            "layers-fixture",
            "--spec-hash",
            "0123456789abcd",
        ],
        &["crates/spec-compiler/tests/fixtures/products/full-badges.jsonc"],
    );
}

#[test]
fn a_product_leaves_the_platform_web_modules_and_journeys_byte_identical() {
    let badges = "crates/spec-compiler/tests/fixtures/products/badges.jsonc";
    same_output(
        "web",
        &["web", "--spec", PLATFORM, "--spec-hash", "0123456789abcd"],
        &[badges],
    );
    same_output(
        "e2e",
        &[
            "e2e",
            "--spec",
            PLATFORM,
            "--origin",
            "http://127.0.0.1:4200",
        ],
        &[badges],
    );
}

#[test]
fn specc_refuses_a_command_whose_product_is_refused() {
    let output = Command::new(env!("CARGO_BIN_EXE_specc"))
        .args([
            "web",
            "--spec",
            PLATFORM,
            "--out",
            "var/specc-layers/refused",
            "--product",
            "crates/spec-compiler/tests/fixtures/products/refused/import-unknown.jsonc",
        ])
        .current_dir(root())
        .output()
        .expect("specc runs");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error[layer.import-unknown]"), "{stderr}");
    assert!(stderr.contains("nothing was written"), "{stderr}");
    assert!(!root().join("var/specc-layers/refused").exists());
}

// ---- Exegra -------------------------------------------------------------------------------

/// A specs directory `var/specc-layers/<dir>` with project `platform` (the platform alone) and
/// project `layered` (the platform and `products`, listed in its project.jsonc). Each test uses
/// its own directory, so tests running at once do not rewrite each other's links.
fn exegra_specs(dir: &str, products: &[&str]) -> PathBuf {
    let specs = root().join("var").join("specc-layers").join(dir);
    for (name, list) in [("platform", &[][..]), ("layered", products)] {
        let dir = specs.join(name);
        fs::create_dir_all(&dir).unwrap();
        // From var/specc-layers/specs/<name> to the root: four directories up.
        let products: Vec<String> = list
            .iter()
            .map(|p| format!("\"../../../../{PRODUCTS}/{p}\""))
            .collect();
        let text = format!(
            "// Written by crates/spec-compiler/tests/layers.rs.\n{{ \"spec\": \"../../../../{PLATFORM}\"{} }}\n",
            if products.is_empty() {
                String::new()
            } else {
                format!(", \"products\": [{}]", products.join(", "))
            }
        );
        fs::write(dir.join("project.jsonc"), text).unwrap();
    }
    specs
}

fn exegra_validate(cli: &Path, specs: &Path, project: &str) -> Value {
    let output = Command::new("node")
        .arg(cli)
        .args(["validate", "--project", project, "--specs"])
        .arg(specs)
        .output()
        .expect("node runs the Exegra CLI");
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "the Exegra CLI did not print JSON ({e}): {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn exegra_accepts_the_product_and_reports_the_platform_hash_unchanged() {
    let Some(cli) = support::exegra_cli("the layered validation") else {
        return;
    };
    let specs = exegra_specs("exegra-accepted", &["badges.jsonc"]);
    let alone = exegra_validate(&cli, &specs, "platform");
    let layered = exegra_validate(&cli, &specs, "layered");
    assert_eq!(layered["ok"], Value::Bool(true), "{layered}");
    assert_eq!(layered["result"]["errors"], Value::from(0), "{layered}");
    assert!(alone["semanticHash"].is_string(), "{alone}");
    assert_eq!(
        layered["semanticHash"], alone["semanticHash"],
        "adding a product changed the platform semantic hash, which the generated headers carry"
    );
    assert_eq!(
        layered["result"]["products"][0]["product"],
        Value::from("badges")
    );
}

#[test]
fn exegra_refuses_each_refused_product_with_the_same_rule() {
    let Some(cli) = support::exegra_cli("the layered refusals") else {
        return;
    };
    for (name, code, _) in REFUSED {
        let file = format!("refused/{name}.jsonc");
        let specs = exegra_specs(&format!("exegra-{name}"), &[file.as_str()]);
        let out = exegra_validate(&cli, &specs, "layered");
        assert_eq!(out["ok"], Value::Bool(false), "{file}: {out}");
        let codes: Vec<&str> = out["result"]["diagnostics"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| d["severity"] == "error")
            .filter_map(|d| d["code"].as_str())
            .collect();
        assert!(
            codes.contains(code),
            "{file}: Exegra refused with {codes:?}, not {code}"
        );
    }
}
