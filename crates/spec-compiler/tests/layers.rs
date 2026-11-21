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
//! - The typed sections (objects, policies, queries, views, workflows, segments, settings, ledger
//!   units, pools and limits, mail and notice templates, tokens, triggers, automations, operations)
//!   hold values of platform types; `ledger-trigger.jsonc` and `promotions.jsonc` are complete products (they declare
//!   everything they name, in their own namespace) that specc and Exegra both accept.
//!
//! The Exegra checks need the Exegra CLI (see `support`).

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;
use strata_spec_compiler::layer_expr;
use strata_spec_compiler::layer_values::{self};
use strata_spec_compiler::layers::{self, ProductFile};

const PLATFORM: &str = "specifications/spec.jsonc";
const PRODUCTS: &str = "crates/spec-compiler/tests/fixtures/products";

fn root() -> PathBuf {
    support::repo_root()
}

fn platform_text() -> String {
    strata_spec_parts::read_spec_text(&root().join(PLATFORM)).unwrap()
}

fn product(relative: &str) -> ProductFile {
    let file = root().join(PRODUCTS).join(relative);
    ProductFile {
        path: format!("{PRODUCTS}/{relative}"),
        text: fs::read_to_string(&file).unwrap(),
        file: Some(file),
    }
}

/// The platform, loaded and parsed once for every test in this binary.
fn prepared_platform() -> &'static layers::PreparedPlatform {
    static PREPARED: std::sync::OnceLock<layers::PreparedPlatform> = std::sync::OnceLock::new();
    PREPARED.get_or_init(|| layers::prepare_platform(PLATFORM, &platform_text()))
}

fn load_with(
    products: &[ProductFile],
) -> Result<layers::Layered, Vec<strata_spec_compiler::Diagnostic>> {
    layers::layer_products(prepared_platform(), Some(&root().join(PLATFORM)), products)
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
        &["\"kernel\"", "without importing it"],
    ),
    (
        "id-not-namespaced",
        layers::NAMESPACE_MISSING,
        &[
            "\"badges-tier\"",
            "not in the namespace of its product",
            "\"badges__\"",
        ],
    ),
    (
        "value-invalid",
        layers::VALUE_INVALID,
        &[
            "\"colour\"",
            "is not a field of its type",
            "object-type-def",
        ],
    ),
    (
        "expression-syntax",
        layer_expr::EXPR_SYNTAX,
        &["is refused at position", "tenant rule language"],
    ),
    (
        "expression-unknown-field",
        layer_expr::EXPR_UNKNOWN_FIELD,
        &["is refused at position", "unknown field"],
    ),
    (
        "expression-unknown-role",
        layer_expr::EXPR_UNKNOWN_ROLE,
        &["unknown role member_tiers__ghost", "position 9"],
    ),
    (
        "expression-unknown-param",
        layer_expr::EXPR_UNKNOWN_PARAM,
        &["undeclared parameter limit", "the scope binds name"],
    ),
    (
        "expression-type",
        layer_expr::EXPR_TYPE,
        &["is refused at position", "tenant rule language"],
    ),
    (
        "query-value-invalid",
        layers::VALUE_INVALID,
        &["\"colour\"", "is not a field of its type", "query-def"],
    ),
    (
        "view-value-invalid",
        layers::VALUE_INVALID,
        &["\"colour\"", "is not a field of its type", "view-def"],
    ),
    (
        "workflow-value-invalid",
        layers::VALUE_INVALID,
        &["\"colour\"", "is not a field of its type", "workflow-def"],
    ),
    (
        "view-query-unknown",
        layers::REFERENCE_UNKNOWN,
        &["\"ticket_board__ghost\"", "no product file declares"],
    ),
    (
        "header-version-invalid",
        layers::HEADER_INVALID,
        &["\"product.version\"", "three whole numbers"],
    ),
    (
        "header-platform-invalid",
        layers::HEADER_INVALID,
        &["has no \"min\"", "first platform release"],
    ),
    (
        "retention-missing",
        layers::RETENTION_MISSING,
        &[
            "\"member_tiers__member\"",
            "personal field \"email\" and no retention (D-6584)",
        ],
    ),
    (
        "step-outside-catalogue",
        layers::OUTSIDE_CATALOGUE,
        &[
            "outside the published catalogue",
            "belongs in the platform specification",
        ],
    ),
    (
        "operation-step-outside",
        layers::OUTSIDE_CATALOGUE,
        &["\"SendMail\"", "\"tenant-operation-step\"", "PostLedger"],
    ),
    (
        "reference-unknown-object",
        layers::REFERENCE_UNKNOWN,
        &["\"member_tiers__ghost\"", "no product file declares"],
    ),
    (
        "capability-core",
        layers::PLATFORM_POWER,
        &["is a core capability", "Remove \"core\""],
    ),
    (
        "setting-platform-level",
        layers::PLATFORM_POWER,
        &["platform level", "tenant, team and person"],
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

/// Both refusals of the catalogue, with the variants the platform's unions have (the fixture is
/// also in [`REFUSED`], which Exegra refuses too).
// Verifies: INT-1522
#[test]
fn a_step_or_trigger_outside_the_catalogue_is_refused() {
    let diagnostics = match load_with(&[product("refused/step-outside-catalogue.jsonc")]) {
        Ok(_) => panic!("refused/step-outside-catalogue.jsonc was accepted"),
        Err(diagnostics) => diagnostics,
    };
    let outside: Vec<&strata_spec_compiler::Diagnostic> = diagnostics
        .iter()
        .filter(|d| d.code == layers::OUTSIDE_CATALOGUE)
        .collect();
    let step = outside
        .iter()
        .find(|d| d.message.contains("\"RunScript\""))
        .unwrap_or_else(|| panic!("the unknown step was not refused: {diagnostics:?}"));
    for word in [
        "steps.run.action",
        "outside the published catalogue",
        "\"automation-step\"",
        "CreateRecord",
    ] {
        assert!(
            step.message.contains(word),
            "the step refusal does not say {word:?}: {}",
            step.message
        );
    }
    let event = outside
        .iter()
        .find(|d| d.message.contains("\"EmailOpened\""))
        .unwrap_or_else(|| panic!("the unknown trigger event was not refused: {diagnostics:?}"));
    assert!(
        event.message.contains("\"trigger-event\"") && event.message.contains("LedgerPosted"),
        "{}",
        event.message
    );
    assert_eq!(outside.len(), 2, "{diagnostics:?}");
    assert!(
        diagnostics.iter().all(|d| d.code.starts_with("layer.")),
        "other refusals too: {diagnostics:?}"
    );
}

// Verifies: LDG-001
#[test]
fn a_product_that_keeps_a_mutable_balance_outside_the_ledger_is_refused() {
    let diagnostics = load_with(&[product("refused/balance-outside-ledger.jsonc")]).unwrap_err();
    assert_eq!(
        diagnostics.len(),
        1,
        "the write from a parameter is no balance: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].code, layers::BALANCE_OUTSIDE_LEDGER);
    for word in [
        "\"member_tiers__grant\"",
        "the field \"points\"",
        "record.points + 1",
        "PostLedger",
    ] {
        assert!(
            diagnostics[0].message.contains(word),
            "{word:?}: {}",
            diagnostics[0].message
        );
    }
    // The products that count in the ledger, and the one that updates a record from its inputs, pass.
    assert!(load_with(&[product("promotions.jsonc"), product("ledger-trigger.jsonc")]).is_ok());
}

// Verifies: SET-001
#[test]
fn a_product_that_keeps_configuration_outside_a_declared_setting_is_refused() {
    let diagnostics =
        load_with(&[product("refused/configuration-outside-setting.jsonc")]).unwrap_err();
    assert!(
        diagnostics
            .iter()
            .all(|d| d.code == layers::CONFIGURATION_OUTSIDE_SETTING),
        "{diagnostics:?}"
    );
    assert_eq!(
        diagnostics.len(),
        1,
        "only the step on the fixed record is refused, not the step on the record passed in: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].entity, "member_tiers__rename");
    assert!(diagnostics[0].message.contains("`ref(1)`"));
    assert!(diagnostics[0].message.contains("declared setting"));
}

// Verifies: INT-3203
#[test]
fn an_event_kind_without_the_app_prefix_is_refused() {
    let diagnostics = load_with(&[product("refused/event-kind-prefix.jsonc")]).unwrap_err();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].code, layers::EVENT_KIND_INVALID);
    assert!(
        diagnostics[0].message.contains("\"order-placed\"")
            && diagnostics[0].message.contains("\"app.\""),
        "{}",
        diagnostics[0].message
    );
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

/// Exegra's `validate` of every `(key, specs, project)` in `jobs`, by key. One Exegra process per
/// shard (`scripts/exegra-validate.mjs`, two by default, `STRATA_EXEGRA_SHARDS`) loads Exegra once
/// and runs the command the CLI runs for each of its share of the jobs; `exegra validate` once per
/// job started Node and loaded Exegra once per job. Each value is the envelope the CLI prints.
fn exegra_validate_all(
    cli: &Path,
    dir: &str,
    jobs: &[(&str, &Path, &str)],
) -> BTreeMap<String, Value> {
    let home = cli.parent().and_then(Path::parent).unwrap();
    let out = root().join("var").join("specc-layers").join(dir);
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(&out).unwrap();
    let list = out.join("jobs.json");
    let listed: Vec<Value> = jobs
        .iter()
        .map(|(key, specs, project)| {
            serde_json::json!({ "key": key, "specs": specs, "project": project })
        })
        .collect();
    fs::write(&list, Value::Array(listed).to_string()).unwrap();
    let shards: usize = std::env::var("STRATA_EXEGRA_SHARDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(2)
        .min(jobs.len());
    let script = root().join("scripts").join("exegra-validate.mjs");
    let children: Vec<_> = (0..shards)
        .map(|i| {
            Command::new("node")
                .arg(&script)
                .arg("--exegra")
                .arg(home)
                .arg("--jobs")
                .arg(&list)
                .arg("--out")
                .arg(&out)
                .arg("--shard")
                .arg(format!("{i}/{shards}"))
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap_or_else(|e| panic!("node could not be run ({e})"))
        })
        .collect();
    for child in children {
        let output = child.wait_with_output().expect("node is waited for");
        let envelope: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
            panic!(
                "scripts/exegra-validate.mjs did not print JSON ({e}): {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        assert_eq!(
            envelope["ok"],
            Value::Bool(true),
            "scripts/exegra-validate.mjs failed: {envelope}"
        );
    }
    jobs.iter()
        .map(|(key, _, _)| {
            let text = fs::read_to_string(out.join(format!("{key}.json")))
                .unwrap_or_else(|e| panic!("no validation of {key} was written ({e})"));
            let envelope = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("the validation of {key} is not JSON ({e})"));
            (key.to_string(), envelope)
        })
        .collect()
}

/// Exegra accepts the fixture product and the complete products beside the platform, and reports
/// the platform semantic hash unchanged with each set. The three validations (the platform alone
/// and the two layered projects) run together, in one batch.
#[test]
fn exegra_accepts_the_products_and_reports_the_platform_hash_unchanged() {
    let Some(cli) = support::exegra_cli("the layered validation") else {
        return;
    };
    let platform = exegra_specs("exegra-platform", &[]);
    let accepted = exegra_specs("exegra-accepted", &["badges.jsonc"]);
    let complete = exegra_specs(
        "exegra-complete",
        &["ledger-trigger.jsonc", "promotions.jsonc"],
    );
    let validations = exegra_validate_all(
        &cli,
        "validations-accepted",
        &[
            ("alone", &platform, "platform"),
            ("accepted", &accepted, "layered"),
            ("complete", &complete, "layered"),
        ],
    );
    let alone = &validations["alone"];
    assert!(alone["semanticHash"].is_string(), "{alone}");

    let layered = &validations["accepted"];
    assert_eq!(layered["ok"], Value::Bool(true), "{layered}");
    assert_eq!(layered["result"]["errors"], Value::from(0), "{layered}");
    assert_eq!(
        layered["semanticHash"], alone["semanticHash"],
        "adding a product changed the platform semantic hash, which the generated headers carry"
    );
    assert_eq!(
        layered["result"]["products"][0]["product"],
        Value::from("badges")
    );

    let layered = &validations["complete"];
    assert_eq!(layered["ok"], Value::Bool(true), "{layered}");
    assert_eq!(layered["result"]["errors"], Value::from(0), "{layered}");
    assert_eq!(layered["semanticHash"], alone["semanticHash"]);
    let products: Vec<&str> = layered["result"]["products"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["product"].as_str())
        .collect();
    assert_eq!(products, vec!["ledger-trigger", "promotions"]);
}

/// Exegra refuses a few of the refused products with the same rule as specc: one from each kind of
/// layering rule (an import, an expression, the header, a record rule). Every Exegra validation
/// analyses the whole platform, so the parity of every fixture is checked in Exegra's own tests
/// (WG-5350), not here.
const EXEGRA_SAMPLE: [&str; 4] = [
    "import-missing",
    "expression-type",
    "header-missing",
    "retention-missing",
];

#[test]
fn exegra_refuses_each_refused_product_with_the_same_rule() {
    let Some(cli) = support::exegra_cli("the layered refusals") else {
        return;
    };
    let cases: Vec<(&str, &str, PathBuf)> = REFUSED
        .iter()
        .filter(|(name, _, _)| EXEGRA_SAMPLE.contains(name))
        .map(|(name, code, _)| {
            let file = format!("refused/{name}.jsonc");
            (
                *name,
                *code,
                exegra_specs(&format!("exegra-{name}"), &[file.as_str()]),
            )
        })
        .collect();
    assert_eq!(
        cases.len(),
        EXEGRA_SAMPLE.len(),
        "a sampled fixture is missing from REFUSED"
    );
    let jobs: Vec<(&str, &Path, &str)> = cases
        .iter()
        .map(|(name, _, specs)| (*name, specs.as_path(), "layered"))
        .collect();
    let validations = exegra_validate_all(&cli, "validations-refused", &jobs);
    for (name, code, _) in &cases {
        let file = format!("refused/{name}.jsonc");
        let out = &validations[*name];
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
