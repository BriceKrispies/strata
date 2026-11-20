//! `specc bundle`: a product file turned into the tenant bundle a tenant installs
//! (`strata_spec_compiler::bundle`, D-6580, WG-5063).
//!
//! - The bundles of `badge-milestones.jsonc`, `promotions.jsonc`, `bundle/member-tiers.jsonc` and
//!   `bundle/ticket-board.jsonc` (a query, a view and a workflow) are golden files
//!   (`tests/fixtures/products/expected`, rewritten by `SPECC_BLESS=1`). Every fixture states its
//!   version and platform range in its header, which the layer checks take and validate.
//! - Every item of a typed section of the file is in the bundle, the bytes do not depend on the order
//!   of the file, and the bundle is a value of `tenant-bundle` (the generation checks it).
//! - A product with no version, a section the bundle has no place for and an id collision are refused
//!   with their rules, a version or a platform range that is malformed is refused by the layer checks
//!   (`layer.header-invalid`), and the layer checks are refused first.
//! - The `specc bundle` command writes the file, and writes nothing for a product it refuses.

mod support;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::{json, Map, Value};
use strata_spec_compiler::layers::{self, ProductFile};
use strata_spec_compiler::{bundle, jsonc, layer_values, output, Diagnostic};

const PLATFORM: &str = "specifications/spec.jsonc";
const PRODUCTS: &str = "crates/spec-compiler/tests/fixtures/products";
const EXPECTED: &str = "crates/spec-compiler/tests/fixtures/products/expected";

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

/// `file` with the version and the platform range of its header replaced by the given ones.
fn with_release(file: ProductFile, version: Option<&str>, platform: Option<Value>) -> ProductFile {
    let mut top = jsonc::parse(&file.text).unwrap();
    let header = top
        .get_mut("product")
        .and_then(Value::as_object_mut)
        .unwrap();
    header.remove("version");
    header.remove("platform");
    if let Some(version) = version {
        header.insert("version".to_string(), json!(version));
    }
    if let Some(platform) = platform {
        header.insert("platform".to_string(), platform);
    }
    ProductFile {
        text: top.to_string(),
        ..file
    }
}

fn generate(file: &ProductFile) -> Result<bundle::Output, Vec<Diagnostic>> {
    bundle::generate(
        PLATFORM,
        &platform_text(),
        Some(&root().join(PLATFORM)),
        file,
    )
}

fn bundle_of(file: &ProductFile) -> bundle::Output {
    match generate(file) {
        Ok(output) => output,
        Err(diagnostics) => panic!(
            "{} was refused:\n{}",
            file.path,
            diagnostics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        ),
    }
}

fn codes(diagnostics: &[Diagnostic]) -> Vec<&str> {
    diagnostics.iter().map(|d| d.code.as_str()).collect()
}

fn check_golden(output: &bundle::Output) {
    let dir = root().join(EXPECTED);
    if std::env::var("SPECC_BLESS").as_deref() == Ok("1") {
        output::write(&dir, &output.files).unwrap();
        return;
    }
    let differences = output::compare(&dir, &output.files).unwrap();
    assert!(
        differences.is_empty(),
        "{EXPECTED} is not what specc bundle writes for {}: {differences:?}. \
         Run the test with SPECC_BLESS=1 to rewrite it, and review the change.",
        output.product
    );
}

// Verifies: INT-1580
#[test]
fn golden_bundle_of_badge_milestones() {
    let output = bundle_of(&product("badge-milestones.jsonc"));
    assert_eq!(output.product, "badge-milestones");
    assert_eq!(output.files.len(), 1);
    assert_eq!(output.files[0].path, "badge-milestones.bundle.json");
    check_golden(&output);
}

// Verifies: INT-1580
#[test]
fn golden_bundle_of_promotions() {
    let output = bundle_of(&product("promotions.jsonc"));
    assert_eq!(output.files[0].path, "promotions.bundle.json");
    check_golden(&output);
}

// Verifies: INT-2347
#[test]
fn golden_bundle_of_a_product_with_a_query_a_view_and_a_workflow() {
    let output = bundle_of(&product("bundle/ticket-board.jsonc"));
    assert_eq!(output.files[0].path, "ticket-board.bundle.json");
    let bundle = the_bundle(&output);
    // Each is an entry of its section, a value of its platform type, keyed by its id.
    for (section, id) in [
        ("queries", "ticket_board__open"),
        ("views", "ticket_board__board"),
        ("workflows", "ticket_board__lifecycle"),
    ] {
        assert_eq!(bundle["content"][section][id]["id"], id, "{section}");
    }
    assert_eq!(
        bundle["content"]["views"]["ticket_board__board"]["route"],
        "/tickets"
    );
    check_golden(&output);
}

// Verifies: INT-1580
#[test]
fn golden_bundle_of_a_product_that_states_its_release_in_its_header() {
    let output = bundle_of(&product("bundle/member-tiers.jsonc"));
    assert_eq!(output.files[0].path, "member-tiers.bundle.json");
    // The claim states a fact about the product and is not tenant metadata.
    assert_eq!(output.omitted, vec![("claims".to_string(), 1)]);
    check_golden(&output);
}

fn the_bundle(output: &bundle::Output) -> Value {
    serde_json::from_str(&output.files[0].contents).unwrap()
}

// Verifies: INT-1580
#[test]
fn the_bundle_names_the_product_its_version_and_the_platform_range() {
    let output = bundle_of(&product("badge-milestones.jsonc"));
    let bundle = the_bundle(&output);
    assert_eq!(bundle["product"], "badge-milestones");
    assert_eq!(bundle["name"], "Badge milestones");
    assert_eq!(bundle["version"], "1.0.0");
    assert_eq!(bundle["platform"], json!({ "min": "0.2" }));
    // Every id is already in the namespace of the product.
    for section in ["objects", "roles", "policies"] {
        for id in bundle["content"][section].as_object().unwrap().keys() {
            assert!(id.starts_with("badge_milestones__"), "{section}: {id}");
        }
    }
    let both = bundle_of(&with_release(
        product("promotions.jsonc"),
        Some("2.3.4"),
        Some(json!({ "min": "0.2", "below": "0.5" })),
    ));
    assert_eq!(
        the_bundle(&both)["platform"],
        json!({ "min": "0.2", "below": "0.5" })
    );
    // A product that states no platform range is written without one.
    let none = bundle_of(&with_release(
        product("promotions.jsonc"),
        Some("2.3.4"),
        None,
    ));
    assert!(the_bundle(&none).get("platform").is_none());
}

// Verifies: INT-1580
#[test]
fn every_item_of_a_typed_section_is_in_the_bundle() {
    for relative in [
        "badge-milestones.jsonc",
        "promotions.jsonc",
        "bundle/member-tiers.jsonc",
        "bundle/ticket-board.jsonc",
    ] {
        let file = product(relative);
        let top = jsonc::parse(&file.text).unwrap();
        let bundle = the_bundle(&bundle_of(&file));
        let mut typed_seen = 0;
        for typed in layer_values::TYPED_SECTIONS {
            let items = top
                .get(typed.section)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let place = bundle::bundle_section(typed.section)
                .unwrap_or_else(|| panic!("{} has no place", typed.section));
            let entries = &bundle["content"][place];
            let count = entries
                .as_object()
                .map(Map::len)
                .or_else(|| entries.as_array().map(Vec::len))
                .unwrap_or(0);
            assert_eq!(count, items.len(), "{relative}: {}", typed.section);
            typed_seen += items.len();
            let mut wanted: Vec<String> = items
                .iter()
                .filter_map(|i| i["id"].as_str().map(str::to_string))
                .collect();
            wanted.sort();
            let mut found: Vec<String> = match (entries.as_object(), typed.id_field) {
                (Some(map), _) => map.keys().cloned().collect(),
                (None, Some(field)) => entries
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e[field].as_str().map(str::to_string))
                    .collect(),
                // A ledger limit has no name of its own.
                (None, None) => wanted.clone(),
            };
            found.sort();
            assert_eq!(found, wanted, "{relative}: {}", typed.section);
        }
        assert!(typed_seen > 0, "{relative} has typed sections");
        // Roles: one entry each, and the permissions of the roles that list capabilities.
        let roles = top
            .get("roles")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            bundle["content"]["roles"].as_object().map_or(0, Map::len),
            roles.len(),
            "{relative}: roles"
        );
        let with_capabilities = roles
            .iter()
            .filter(|r| r["capabilities"].as_array().is_some_and(|c| !c.is_empty()))
            .count();
        assert_eq!(
            bundle["content"]["role-permissions"]
                .as_object()
                .map_or(0, Map::len),
            with_capabilities,
            "{relative}: role-permissions"
        );
    }
}

#[test]
fn a_role_becomes_a_role_and_its_permissions_and_a_capability_keeps_its_description() {
    let output = bundle_of(&product("bundle/member-tiers.jsonc"));
    let bundle = the_bundle(&output);
    assert_eq!(
        bundle["content"]["roles"]["member_tiers__editor"],
        json!({ "id": "member_tiers__editor", "label": "Tier editor" })
    );
    assert_eq!(
        bundle["content"]["role-permissions"]["member_tiers__editor"],
        json!([{ "Capability": "member_tiers__promote" }, { "Capability": "content-read" }])
    );
    assert_eq!(
        bundle["content"]["capabilities"][0],
        json!({
            "id": "member_tiers__promote",
            "name": "Promote members",
            "description": "Promote a member to a higher tier."
        })
    );
    // The member that names a ledger unit is the unit, and the item's own id is gone.
    assert_eq!(
        bundle["content"]["ledger-units"][0],
        json!({ "unit": "member_tiers__points", "description": "Points a member earns." })
    );
}

fn reversed_sections(file: &ProductFile) -> ProductFile {
    let mut top = jsonc::parse(&file.text).unwrap();
    for (key, value) in top.as_object_mut().unwrap() {
        if key != "product" {
            if let Some(items) = value.as_array_mut() {
                items.reverse();
            }
        }
    }
    ProductFile {
        text: top.to_string(),
        path: file.path.clone(),
        file: file.file.clone(),
    }
}

#[test]
fn the_same_product_gives_the_same_bytes_whatever_the_order_of_its_file() {
    for relative in ["badge-milestones.jsonc", "promotions.jsonc"] {
        let file = product(relative);
        let first = bundle_of(&file);
        assert_eq!(first, bundle_of(&file), "{relative}: twice");
        assert_eq!(
            first.files,
            bundle_of(&reversed_sections(&file)).files,
            "{relative}: sections reversed"
        );
    }
}

#[test]
fn sections_and_keys_are_written_in_sorted_order() {
    let output = bundle_of(&product("badge-milestones.jsonc"));
    let text = &output.files[0].contents;
    assert!(text.ends_with("}\n"));
    let mut top = Vec::new();
    let mut content = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line == "  \"content\": {" {
            inside = true;
        } else if line == "  }," && inside {
            inside = false;
        }
        if inside && line.starts_with("    \"") && !line.starts_with("     ") {
            content.push(line.trim().split('"').nth(1).unwrap().to_string());
        }
        if line.starts_with("  \"") && !line.starts_with("   ") {
            top.push(line.trim().split('"').nth(1).unwrap().to_string());
        }
    }
    assert_eq!(top, ["content", "name", "platform", "product", "version"]);
    assert!(content.len() > 3, "{content:?}");
    let mut sorted = content.clone();
    sorted.sort();
    assert_eq!(content, sorted);
}

// ---- refusals -------------------------------------------------------------------------------

fn refused(relative: &str) -> Vec<Diagnostic> {
    match generate(&product(relative)) {
        Ok(_) => panic!("{relative} was accepted"),
        Err(diagnostics) => diagnostics,
    }
}

// Verifies: INT-1583
#[test]
fn a_product_with_no_version_is_refused() {
    let diagnostics = refused("bundle-refused/no-version.jsonc");
    assert_eq!(
        codes(&diagnostics),
        [bundle::VERSION_MISSING],
        "{diagnostics:?}"
    );
    let message = &diagnostics[0].message;
    assert!(
        message.contains("has no \"version\"") && message.contains("add \"version\": \"1.0.0\""),
        "{message}"
    );
    // A product file that states none is refused the same way, whatever else it holds.
    let plain = generate(&with_release(product("badge-milestones.jsonc"), None, None)).unwrap_err();
    assert_eq!(codes(&plain), [bundle::VERSION_MISSING]);
}

// Verifies: INT-2347
#[test]
fn a_version_that_is_not_three_numbers_is_refused_by_the_layer_checks() {
    for version in ["1.0", "v1.0.0", "1.0.0-rc1", ""] {
        let file = with_release(
            product("promotions.jsonc"),
            Some(version),
            Some(json!({ "min": "0.2" })),
        );
        let diagnostics = generate(&file).unwrap_err();
        assert_eq!(codes(&diagnostics), [layers::HEADER_INVALID], "{version:?}");
        assert!(
            diagnostics[0].message.contains("three whole numbers"),
            "{}",
            diagnostics[0].message
        );
    }
    let mut file = with_release(product("promotions.jsonc"), None, None);
    let mut top = jsonc::parse(&file.text).unwrap();
    top["product"]["version"] = json!(3);
    file.text = top.to_string();
    assert_eq!(
        codes(&generate(&file).unwrap_err()),
        [layers::HEADER_INVALID]
    );
}

// Verifies: INT-2347
#[test]
fn a_platform_range_without_a_first_release_is_refused_by_the_layer_checks() {
    let diagnostics = refused("bundle-refused/platform-invalid.jsonc");
    assert_eq!(
        codes(&diagnostics),
        [layers::HEADER_INVALID],
        "{diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("no \"min\""),
        "{}",
        diagnostics[0].message
    );
    for platform in [
        json!("0.2"),
        json!({ "min": "" }),
        json!({ "min": "0.2", "above": "1" }),
        json!({ "min": "banana" }),
        // A range that supports no release.
        json!({ "min": "0.4", "below": "0.3" }),
        json!({ "min": "0.2", "below": "0.2.0" }),
    ] {
        let file = with_release(
            product("promotions.jsonc"),
            Some("1.0.0"),
            Some(platform.clone()),
        );
        let diagnostics = generate(&file).unwrap_err();
        assert_eq!(codes(&diagnostics), [layers::HEADER_INVALID], "{platform}");
    }
}

// Verifies: INT-1580
#[test]
fn a_section_the_bundle_has_no_place_for_is_refused() {
    let diagnostics = refused("bundle-refused/section-no-place.jsonc");
    assert_eq!(
        codes(&diagnostics),
        [bundle::SECTION_NO_PLACE],
        "{diagnostics:?}"
    );
    let message = &diagnostics[0].message;
    assert_eq!(diagnostics[0].entity, "types");
    for word in ["\"types\"", "tenant bundle", "objects", "ledgerUnits"] {
        assert!(message.contains(word), "{word}: {message}");
    }
}

#[test]
fn each_section_of_a_product_has_a_place_or_is_a_statement_about_the_product_or_is_refused() {
    for (section, _) in layers::product_sections() {
        let placed = bundle::bundle_section(section).is_some();
        let documentation = bundle::DOCUMENTATION_SECTIONS.contains(&section);
        let mut top = Map::new();
        top.insert("product".to_string(), json!({}));
        top.insert(section.to_string(), json!([]));
        let result = bundle::build("p", "P", "1.0.0", None, &top);
        match (placed, documentation) {
            (true, false) | (false, true) => assert!(result.is_ok(), "{section}"),
            (false, false) => {
                let diagnostics = result.unwrap_err();
                assert_eq!(codes(&diagnostics), [bundle::SECTION_NO_PLACE], "{section}");
            }
            (true, true) => panic!("{section} is both placed and documentation"),
        }
    }
}

fn top_of(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}

// Verifies: INT-1582
#[test]
fn two_entities_that_would_share_an_id_are_refused() {
    // A mail template and a notice template are one template-id.
    let top = top_of(json!({
        "mailTemplates": [{ "id": "p__welcome", "subject": "s", "body": "b" }],
        "noticeTemplates": [{ "id": "p__welcome", "title": "t", "body": "b" }]
    }));
    let diagnostics = bundle::build("p", "P", "1.0.0", None, &top).unwrap_err();
    assert_eq!(codes(&diagnostics), [bundle::ID_COLLISION]);
    assert_eq!(diagnostics[0].entity, "p__welcome");
    assert!(
        diagnostics[0].message.contains("mailTemplates")
            && diagnostics[0].message.contains("template")
            && diagnostics[0]
                .message
                .contains("Give one of them an id of its own"),
        "{}",
        diagnostics[0].message
    );
    // An automation takes the id space of the workflows, and two of one id are one.
    let top = top_of(json!({
        "objects": [{ "id": "p__a", "label": "A", "fields": {}, "constraints": {} },
                    { "id": "p__a", "label": "A again", "fields": {}, "constraints": {} }]
    }));
    let diagnostics = bundle::build("p", "P", "1.0.0", None, &top).unwrap_err();
    assert_eq!(codes(&diagnostics), [bundle::ID_COLLISION]);
    // Entities of different kinds do not collide.
    let top = top_of(json!({
        "roles": [{ "id": "p__x" }],
        "capabilities": [{ "id": "p__x" }],
        "settings": [{ "id": "p__x" }]
    }));
    assert!(bundle::build("p", "P", "1.0.0", None, &top).is_ok());
}

// Verifies: INT-1582
#[test]
fn the_layer_checks_refuse_a_duplicate_id_first() {
    let diagnostics = refused("bundle-refused/id-collision.jsonc");
    assert!(
        codes(&diagnostics).contains(&layers::REDEFINES_ENTITY),
        "{diagnostics:?}"
    );
    assert!(
        codes(&diagnostics)
            .iter()
            .all(|c| !c.starts_with("bundle.")),
        "{diagnostics:?}"
    );
}

#[test]
fn what_the_layer_checks_refuse_is_refused_as_it_is_with_the_bundle_rules_beside_it() {
    // No version and an id outside the namespace.
    let diagnostics = refused("refused/id-not-namespaced.jsonc");
    let found = codes(&diagnostics);
    assert!(found.contains(&bundle::VERSION_MISSING), "{found:?}");
    assert!(found.contains(&layers::NAMESPACE_MISSING), "{found:?}");
}

// ---- the command ------------------------------------------------------------------------------

fn specc(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_specc"))
        .args(args)
        .current_dir(root())
        .output()
        .expect("specc runs")
}

fn product_arg(relative: &str) -> String {
    format!("{PRODUCTS}/{relative}")
}

// Verifies: INT-1580
#[test]
fn specc_bundle_writes_the_bundle_file() {
    let out = root().join("var/specc-bundle/ok");
    let _ = fs::remove_dir_all(&out);
    let product = product_arg("bundle/member-tiers.jsonc");
    let output = specc(&[
        "bundle",
        "--spec",
        PLATFORM,
        "--product",
        product.as_str(),
        "--out",
        "var/specc-bundle/ok",
    ]);
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let written = fs::read_to_string(out.join("member-tiers.bundle.json")).unwrap();
    let expected =
        fs::read_to_string(root().join(EXPECTED).join("member-tiers.bundle.json")).unwrap();
    assert_eq!(written.replace('\r', ""), expected.replace('\r', ""));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("\"claims\""), "{stderr}");
}

// Verifies: INT-1583
#[test]
fn specc_bundle_refuses_a_product_and_writes_nothing() {
    for (relative, code) in [
        ("bundle-refused/no-version.jsonc", "bundle.version-missing"),
        (
            "bundle-refused/section-no-place.jsonc",
            "bundle.section-no-place",
        ),
        (
            "bundle-refused/platform-invalid.jsonc",
            "layer.header-invalid",
        ),
    ] {
        let out = root().join("var/specc-bundle/refused");
        let _ = fs::remove_dir_all(&out);
        let product = product_arg(relative);
        let output = specc(&[
            "bundle",
            "--spec",
            PLATFORM,
            "--product",
            product.as_str(),
            "--out",
            "var/specc-bundle/refused",
        ]);
        assert_eq!(output.status.code(), Some(1), "{relative}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!("error[{code}]")),
            "{relative}: {stderr}"
        );
        assert!(
            stderr.contains("nothing was written"),
            "{relative}: {stderr}"
        );
        assert!(!out.exists(), "{relative}");
    }
}

#[test]
fn specc_bundle_takes_exactly_one_product() {
    let none = specc(&[
        "bundle",
        "--spec",
        PLATFORM,
        "--out",
        "var/specc-bundle/none",
    ]);
    assert_eq!(none.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&none.stderr).contains("exactly one --product"));
    let a = product_arg("bundle/member-tiers.jsonc");
    let b = product_arg("promotions.jsonc");
    let two = specc(&[
        "bundle",
        "--spec",
        PLATFORM,
        "--product",
        a.as_str(),
        "--product",
        b.as_str(),
        "--out",
        "var/specc-bundle/two",
    ]);
    assert_eq!(two.status.code(), Some(2));
    assert!(!root().join("var/specc-bundle/two").exists());
}
