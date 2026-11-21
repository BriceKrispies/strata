//! `specc bundle`: a product file turned into the tenant bundle a tenant installs
//! (`strata_spec_compiler::bundle`, D-6580, WG-5063).
//!
//! - The bundles of `ledger-trigger.jsonc`, `promotions.jsonc`, `bundle/member-tiers.jsonc` and
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
use strata_spec_compiler::{bundle, bundle_ui, jsonc, output, Diagnostic};

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

/// The platform, loaded and parsed once for every test in this binary.
fn prepared_platform() -> &'static layers::PreparedPlatform {
    static PREPARED: std::sync::OnceLock<layers::PreparedPlatform> = std::sync::OnceLock::new();
    PREPARED.get_or_init(|| layers::prepare_platform(PLATFORM, &platform_text()))
}

fn generate(file: &ProductFile) -> Result<bundle::Output, Vec<Diagnostic>> {
    bundle::generate_prepared(
        PLATFORM,
        prepared_platform(),
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
fn golden_bundle_of_ledger_trigger() {
    let output = bundle_of(&product("ledger-trigger.jsonc"));
    assert_eq!(output.product, "ledger-trigger");
    assert_eq!(output.files.len(), 1);
    assert_eq!(output.files[0].path, "ledger-trigger.bundle.json");
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

/// Verifies: INT-3250
///
/// The screens, forms, actions, live bindings and error messages of a product file are the section
/// `ui` of its bundle: each a value of `ui-element`, keyed by its id, bound to the product's view by
/// its route and to its operation by its id.
#[test]
fn golden_bundle_of_a_product_that_declares_its_interface() {
    let output = bundle_of(&product("bundle/ticket-desk.jsonc"));
    assert_eq!(output.files[0].path, "ticket-desk.bundle.json");
    let bundle = the_bundle(&output);
    let ui = &bundle["content"]["ui"];
    let screen = &ui["ticket_desk__board_screen"]["Screen"];
    assert_eq!(screen["view"], "ticket_desk__board");
    assert_eq!(screen["route"], "/tickets");
    assert_eq!(screen["kind"], "list");
    assert_eq!(screen["auth"], "session");
    let form = &ui["ticket_desk__open_form"]["Form"];
    assert_eq!(form["operation"], "ticket_desk__open_ticket");
    assert_eq!(form["fields"][0]["trim"], true);
    assert_eq!(
        ui["ticket_desk__open_again"]["Action"]["arguments"],
        json!({ "title": "form.title" })
    );
    assert_eq!(
        ui["ticket_desk__board_live"]["LiveBinding"],
        json!({ "screen": "ticket_desk__board_screen", "event": "notice", "kinds": ["membership"], "objects": true })
    );
    assert_eq!(
        ui["ticket_desk__title_needed"]["ErrorMessage"]["applies-to"],
        json!(["ticket_desk__open_form"])
    );
    // The interface is the section `ui`, and no other section holds it.
    for section in [
        "screens",
        "forms",
        "actions",
        "liveBindings",
        "errorMessages",
    ] {
        assert!(bundle["content"].get(section).is_none(), "{section}");
    }
    check_golden(&output);
}

/// Verifies: INT-3251
///
/// A screen whose route no view of the product answers and a form that submits to no operation of
/// the product are refused, each naming the element.
#[test]
fn an_interface_element_bound_to_nothing_of_the_product_is_refused() {
    let diagnostics = refused("bundle-refused/ui-unresolved.jsonc");
    assert_eq!(
        codes(&diagnostics),
        [bundle_ui::UI_UNRESOLVED, bundle_ui::UI_UNRESOLVED],
        "{diagnostics:?}"
    );
    let entities: Vec<&str> = diagnostics.iter().map(|d| d.entity.as_str()).collect();
    assert_eq!(entities, ["ticket_desk__desk", "ticket_desk__open_form"]);
    assert!(
        diagnostics[0].message.contains("\"/desk\""),
        "{}",
        diagnostics[0].message
    );
    assert!(
        diagnostics[1].message.contains("ticket_desk__open_ticket"),
        "{}",
        diagnostics[1].message
    );
}

fn the_bundle(output: &bundle::Output) -> Value {
    serde_json::from_str(&output.files[0].contents).unwrap()
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
    for relative in ["ledger-trigger.jsonc", "promotions.jsonc"] {
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
    let plain = generate(&with_release(product("ledger-trigger.jsonc"), None, None)).unwrap_err();
    assert_eq!(codes(&plain), [bundle::VERSION_MISSING]);
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

// ---- the command ------------------------------------------------------------------------------

fn specc(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_specc"))
        .args(args)
        .current_dir(root())
        .output()
        .expect("specc runs")
}

// Verifies: AGG-001
// Verifies: AGG-041
#[test]
fn a_query_with_aggregates_and_a_section_that_names_one_reach_the_bundle() {
    let output = bundle_of(&product("bundle/score-board.jsonc"));
    assert_eq!(output.files[0].path, "score-board.bundle.json");
    let bundle = the_bundle(&output);
    let query = &bundle["content"]["queries"]["score_board__entries"];
    assert_eq!(query["declared-fields"], json!(["player", "points"]));
    assert_eq!(query["aggregates"][0]["id"], "leaders");
    assert_eq!(query["aggregates"][0]["kind"], "top");
    assert_eq!(query["aggregates"][0]["n"], 3);
    assert_eq!(query["aggregates"][1]["group-by"], "player");
    assert_eq!(query["aggregates"][1]["scan-bound"], 1000);
    assert_eq!(
        bundle["content"]["views"]["score_board__leaderboard"]["sections"]["leaders"]["aggregate"],
        "leaders"
    );
}
