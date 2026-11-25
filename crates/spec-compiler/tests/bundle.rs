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

use crate::support;

use std::fs;
use std::path::PathBuf;

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

/// A presentation of a product file is an interface element of the section `ui` of its bundle,
/// with the display type and props of each field it declares.
#[test]
fn a_presentation_is_an_interface_element_of_the_bundle() {
    let output = bundle_of(&product("bundle/presentation.jsonc"));
    let bundle = the_bundle(&output);
    let presentation = &bundle["content"]["ui"]["ticket_desk__ticket_presentation"]["Presentation"];
    assert_eq!(presentation["object"], "ticket_desk__ticket");
    assert_eq!(presentation["title"], "title");
    assert_eq!(
        presentation["fields"]["assignee"],
        json!({ "display": "person" })
    );
    assert_eq!(
        presentation["fields"]["created_at"],
        json!({ "display": "datetime", "props": { "style": "relative" } })
    );
    assert!(bundle["content"].get("presentations").is_none());
}

/// A presentation that names an object type, a field or a display type that does not exist is
/// refused, each with its ui.binding code.
#[test]
fn a_presentation_that_names_what_does_not_exist_is_refused() {
    let diagnostics = refused("bundle-refused/ui-binding.jsonc");
    assert_eq!(
        codes(&diagnostics),
        [
            bundle_ui::BINDING_UNKNOWN_OBJECT,
            bundle_ui::BINDING_UNKNOWN_FIELD,
            bundle_ui::BINDING_UNKNOWN_DISPLAY
        ],
        "{diagnostics:?}"
    );
}

/// Verifies: INT-5510
///
/// A query that declares the tree form (`tree`: via, root and max-depth) keeps it in the bundle, with
/// the keys of the product file; the view that reads it takes its root from another section's $id.
#[test]
fn golden_bundle_of_a_product_with_a_descendants_query() {
    let output = bundle_of(&product("bundle/thread.jsonc"));
    assert_eq!(output.files[0].path, "thread.bundle.json");
    let bundle = the_bundle(&output);
    let tree = &bundle["content"]["queries"]["thread__replies"]["tree"];
    assert_eq!(tree["via"], "parent");
    assert_eq!(tree["root"], "root");
    assert_eq!(tree["max-depth"], 8);
    assert_eq!(
        bundle["content"]["views"]["thread__view"]["sections"]["replies"]["args"]["root"]
            ["Section"]["attribute"],
        "$id"
    );
    check_golden(&output);
}

/// WG-5596: the toggle compiles into a tenant bundle, the bundle installs into a tenant, and the
/// operation it declares runs in the kernel. The first invocation adds the caller's reaction and the
/// second removes it.
#[test]
fn the_toggle_product_installs_and_its_operation_adds_then_removes_a_reaction() {
    use strata_kernel::testkit::{ctx, field, object, text};
    use strata_kernel::{
        DomainError, Intent, Output, Principal, PrincipalId, RecordId, TenantId, TenantOperationId,
        TenantStateExt, Value, World,
    };
    use strata_substrate::bundle::{
        install_into, install_product, Bundle, ProductBundle, ProductInstall, ProductTarget,
    };

    let output = bundle_of(&product("operation-toggle.jsonc"));
    let bundle = ProductBundle::from_json(&output.files[0].contents)
        .unwrap_or_else(|e| panic!("the bundle is refused: {e}"));

    let tenant = TenantId::new("acme").expect("tenant id");
    let admin = Principal::User(PrincipalId::new("user:admin").expect("principal id"));
    let doc = json!({
        "id": "acme",
        "name": "Acme",
        "owner": "user:admin",
        "created_at": "2026-01-01T00:00:00Z"
    });
    let mut world = install_into(
        &World::default(),
        &Bundle::from_json(&doc.to_string()).expect("tenant bundle"),
    )
    .unwrap_or_else(|e| panic!("{e:?}"));

    let schema = world.tenants[&tenant].schema.clone();
    let target = ProductTarget {
        tenant: &tenant,
        principal: admin.clone(),
        now: 1_800_000_000_000,
        release: "0.2",
        schema: &schema,
        installed: None,
        expected_version: None,
        idempotency_key: None,
    };
    let answer = install_product(&bundle, &target, &mut |ctx: strata_kernel::Context,
                                                         intent: Intent|
     -> Result<Output, DomainError> {
        let (next, outcome) = world.step(&intent, &ctx)?;
        world = next;
        Ok(outcome.output)
    });
    match answer {
        Ok(ProductInstall::Applied { .. }) => {}
        other => panic!("the toggle product was not installed: {other:?}"),
    }

    let run = |world: &World, intent: Intent| -> World {
        world
            .step(&intent, &ctx("acme", admin.clone(), 1_800_000_000_100))
            .unwrap_or_else(|e| panic!("{e:?}"))
            .0
    };
    let reactions = |world: &World| {
        world.tenants[&tenant]
            .records_of(&object("toggle__reaction"))
            .count()
    };

    // The add posts a point received by the message's author (WG-5598), and the author must be a
    // member of the tenant. The owner is the only member this bundle installs, so the owner writes the
    // message: the given and the received point then sit on one person's account, in two units.
    let author = "user:admin";
    let message = Intent::CreateRecord {
        object: object("toggle__message"),
        fields: std::collections::BTreeMap::from([
            (field("body"), text("Hello")),
            (field("author"), text(author)),
        ]),
    };
    let world = run(&world, message);
    let toggle = |key: &str| Intent::InvokeOperation {
        operation: TenantOperationId::new("toggle__react").expect("operation id"),
        inputs: std::collections::BTreeMap::from([
            ("message".to_string(), Value::Ref(RecordId(1))),
            ("kind".to_string(), text("love")),
        ]),
        idempotency_key: key.to_string(),
    };
    // The balance of a person's account in a unit of the product (no scope).
    let balance = |world: &World, owner: &str, unit: &str| -> i64 {
        use strata_kernel::ledger::{account_id, account_key};
        use strata_kernel::{LedgerOwner, LedgerUnit};
        let state = &world.tenants[&TenantId::new("acme").expect("tenant id")];
        let owner = LedgerOwner::Principal {
            principal: PrincipalId::new(owner).expect("principal id"),
        };
        let unit = LedgerUnit::new(unit).expect("ledger unit");
        let id = account_id(&state.id, &owner, &unit, "");
        state
            .ledger
            .as_ref()
            .and_then(|ledger| ledger.accounts.get(&account_key(&id)))
            .map_or(0, |account| account.balance.0)
    };
    let given = |world: &World| balance(world, "user:admin", "toggle__reactions_given");
    let received = |world: &World| balance(world, author, "toggle__reactions_received");

    let added = run(&world, toggle("k-add"));
    assert_eq!(reactions(&added), 1, "the first toggle adds the reaction");
    assert_eq!(given(&added), 1, "the reactor gives one point");
    assert_eq!(received(&added), 1, "the author receives one point");
    let removed = run(&added, toggle("k-remove"));
    assert_eq!(reactions(&removed), 0, "the second toggle removes it");
    assert_eq!(
        given(&removed),
        0,
        "taking the reaction back reverses the given point"
    );
    assert_eq!(
        received(&removed),
        0,
        "taking the reaction back reverses the received point"
    );
}

/// WG-5606: a product whose operations write its own settings compiles into a bundle that installs
/// into a tenant. Its person-level operation writes the invoker's own value of its setting, and its
/// tenant-level operation, which the owner may invoke, writes the value the tenant holds for everyone.
#[test]
fn the_write_setting_product_installs_and_its_operations_write_its_settings() {
    use strata_kernel::testkit::ctx;
    use strata_kernel::{
        settings, DomainError, Intent, Output, Principal, PrincipalId, SettingId, TenantId,
        TenantOperationId, Value, World,
    };
    use strata_substrate::bundle::{
        install_into, install_product, Bundle, ProductBundle, ProductInstall, ProductTarget,
    };

    let output = bundle_of(&product("operation-write-setting.jsonc"));
    let bundle = ProductBundle::from_json(&output.files[0].contents)
        .unwrap_or_else(|e| panic!("the bundle is refused: {e}"));

    let tenant = TenantId::new("acme").expect("tenant id");
    let admin = Principal::User(PrincipalId::new("user:admin").expect("principal id"));
    let doc = json!({
        "id": "acme",
        "name": "Acme",
        "owner": "user:admin",
        "created_at": "2026-01-01T00:00:00Z"
    });
    let mut world = install_into(
        &World::default(),
        &Bundle::from_json(&doc.to_string()).expect("tenant bundle"),
    )
    .unwrap_or_else(|e| panic!("{e:?}"));

    let schema = world.tenants[&tenant].schema.clone();
    let target = ProductTarget {
        tenant: &tenant,
        principal: admin.clone(),
        now: 1_800_000_000_000,
        release: "0.2",
        schema: &schema,
        installed: None,
        expected_version: None,
        idempotency_key: None,
    };
    let answer = install_product(&bundle, &target, &mut |ctx: strata_kernel::Context,
                                                         intent: Intent|
     -> Result<Output, DomainError> {
        let (next, outcome) = world.step(&intent, &ctx)?;
        world = next;
        Ok(outcome.output)
    });
    match answer {
        Ok(ProductInstall::Applied { .. }) => {}
        other => panic!("the write-setting product was not installed: {other:?}"),
    }

    let run = |world: &World, intent: Intent| -> World {
        world
            .step(&intent, &ctx("acme", admin.clone(), 1_800_000_000_100))
            .unwrap_or_else(|e| panic!("{e:?}"))
            .0
    };
    let value_of = |world: &World, setting: &str| -> Value {
        settings::resolve(
            &world.tenants[&tenant],
            &admin,
            &SettingId::new(setting).expect("setting id"),
        )
        .expect("the setting is installed")
        .value
    };

    let choose = Intent::InvokeOperation {
        operation: TenantOperationId::new("channel_prefs__choose_home_channel")
            .expect("operation id"),
        inputs: std::collections::BTreeMap::from([(
            "channel".to_string(),
            Value::Str("random".into()),
        )]),
        idempotency_key: "k-choose".into(),
    };
    let chosen = run(&world, choose);
    assert_eq!(
        value_of(&chosen, "channel_prefs__home_channel"),
        Value::Str("random".into()),
        "the operation writes the invoker's own home channel"
    );

    let set_hour = Intent::InvokeOperation {
        operation: TenantOperationId::new("channel_prefs__set_digest_hour").expect("operation id"),
        inputs: std::collections::BTreeMap::from([("hour".to_string(), Value::Int(7))]),
        idempotency_key: "k-hour".into(),
    };
    let hour = run(&chosen, set_hour);
    assert_eq!(
        value_of(&hour, "channel_prefs__digest_hour"),
        Value::Int(7),
        "the tenant operation writes the tenant's digest hour"
    );
}
