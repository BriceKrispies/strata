#![allow(clippy::err_expect)] // `unwrap_err` would print a whole world when it fails.
//! The interface definition of a tenant (D-8250): a product's screen, form, action, live binding
//! and error message, as `specc bundle` writes them, are installed into the tenant schema by the
//! schema change that installs the product, owned by its record; the definition is read back in
//! parts named by their hashes, with a manifest named by the hash of its list of parts; an upgrade
//! changes the parts it changes, and a removal takes the elements out with the rest.

use serde_json::{json, Value as Json};
use strata_kernel::*;
use strata_substrate::bundle::{
    install_into, install_product, remove_product, upgrade_product, Bundle, InstalledRecord,
    ProductBundle, ProductInstall, ProductInstallError, ProductTarget,
};
use strata_substrate::ui::{definition, title_text, ERRORS_PART, SCREEN_PART};

/// The bundle `specc bundle` writes for `tests/fixtures/products/bundle/ticket-desk.jsonc`.
const TICKET_DESK: &str =
    include_str!("../../spec-compiler/tests/fixtures/products/expected/ticket-desk.bundle.json");

fn tenant() -> TenantId {
    TenantId::new("acme").unwrap()
}

fn admin() -> Principal {
    Principal::User(PrincipalId::new("user:admin").unwrap())
}

/// A tenant that exists and has no product.
fn world() -> World {
    let doc = json!({
        "id": "acme",
        "name": "Acme",
        "owner": "user:admin",
        "created_at": "2026-01-01T00:00:00Z"
    });
    let bundle = Bundle::from_json(&doc.to_string()).unwrap();
    install_into(&World::default(), &bundle).unwrap_or_else(|e| panic!("{e}"))
}

fn ticket_desk() -> Json {
    serde_json::from_str(TICKET_DESK).unwrap()
}

fn read(doc: &Json) -> ProductBundle {
    ProductBundle::from_json(&doc.to_string()).unwrap_or_else(|e| panic!("{e}"))
}

fn schema_of(world: &World) -> TenantSchema {
    (*world.tenants[&tenant()].schema).clone()
}

type Changed = Result<ProductInstall, ProductInstallError<DomainError>>;

/// Runs one product change against `world`, which takes the schema change when there is one.
fn change(
    world: &mut World,
    run: impl FnOnce(
        &ProductTarget<'_>,
        &mut dyn FnMut(Context, Intent) -> Result<Output, DomainError>,
    ) -> Changed,
) -> Changed {
    let tenant = tenant();
    let schema = schema_of(world);
    let target = ProductTarget {
        tenant: &tenant,
        principal: admin(),
        now: 1_800_000_000_000,
        release: "0.2",
        schema: &schema,
        installed: None,
        expected_version: None,
        idempotency_key: None,
    };
    let mut execute = |ctx: Context, intent: Intent| -> Result<Output, DomainError> {
        let (next, outcome) = world.step(&intent, &ctx)?;
        *world = next;
        Ok(outcome.output)
    };
    run(&target, &mut execute)
}

fn installed(world: &mut World, doc: &Json) -> u64 {
    let bundle = read(doc);
    match change(world, |t, e| install_product(&bundle, t, e)).unwrap_or_else(|e| panic!("{e}")) {
        ProductInstall::Applied { schema_version, .. } => schema_version,
        other => panic!("the install was not applied: {other:?}"),
    }
}

const SCREEN: &str = "ticket_desk__board_screen";
const FORM: &str = "ticket_desk__open_form";

/// Verifies: INT-3252
///
/// The elements are defined by the one schema change that installs the product, under its schema
/// version, and its ownership record lists each of them as a `ui-element`.
#[test]
fn a_products_interface_is_installed_with_it_and_owned_by_its_record() {
    let mut world = world();
    let before = schema_of(&world).version.0;
    let version = installed(&mut world, &ticket_desk());
    assert_eq!(
        version,
        before + 1,
        "one schema change installs data and interface"
    );
    let schema = schema_of(&world);
    let ids: Vec<&String> = strata_kernel::ui::elements(&schema)
        .map(|(id, _)| id)
        .collect();
    assert_eq!(
        ids,
        [
            "ticket_desk__board_live",
            "ticket_desk__board_screen",
            "ticket_desk__open_again",
            "ticket_desk__open_form",
            "ticket_desk__title_needed",
        ]
    );
    let record = InstalledRecord::of(&schema, "ticket-desk").unwrap();
    assert_eq!(record.schema_version, Some(version));
    for id in &ids {
        assert!(
            record
                .owns
                .iter()
                .any(|e| e.kind == "ui-element" && &e.id == *id),
            "{id} is owned"
        );
        assert_eq!(
            strata_kernel::products::owner_of(&schema, EntityKind::UiElement, id)
                .map(ProductId::as_str),
            Some("ticket-desk")
        );
    }
    let Some(UiElement::Form(form)) = strata_kernel::ui::element(&schema, FORM) else {
        panic!("the form is installed as a form");
    };
    assert_eq!(form.operation.as_str(), "ticket_desk__open_ticket");
    assert_eq!(form.fields[0].control, UiControl::Text);
}

/// Verifies: INT-3253
///
/// The definition read back from the schema: one part for the screen with its form, action and
/// live binding, one for the error messages, each found by the hash the manifest lists, which is
/// the hash of its canonical encoding; the manifest is named by the hash of its list of parts.
#[test]
fn the_installed_definition_is_read_back_in_parts_by_hash() {
    let mut world = world();
    installed(&mut world, &ticket_desk());
    let schema = schema_of(&world);
    let defined = definition(&schema);
    let manifest = &defined.manifest;
    assert_eq!(manifest.schema_version, schema.version.0);
    assert_eq!(manifest.hash, canon::hash_of(&manifest.parts));
    let kinds: Vec<&str> = manifest.parts.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, [SCREEN_PART, ERRORS_PART]);

    let screen = &manifest.parts[0];
    assert_eq!(screen.id, SCREEN);
    assert_eq!(screen.route.as_deref(), Some("/tickets"));
    assert_eq!(screen.view.as_deref(), Some("ticket_desk__board"));
    assert_eq!(screen.title.as_deref(), Some("Open tickets"));
    assert_eq!(screen.layout.as_deref(), Some("list"));
    assert_eq!(screen.auth.as_deref(), Some("session"));
    assert_eq!(manifest.screen_of_view("ticket_desk__board"), Some(screen));

    let part = defined
        .part(&screen.hash)
        .expect("the screen's part is found by its hash");
    assert_eq!(screen.hash, canon::hash_of(part));
    assert_eq!(part["screen"]["route"], "/tickets");
    assert_eq!(part["forms"][FORM]["operation"], "ticket_desk__open_ticket");
    assert_eq!(part["forms"][FORM]["submit_label"], "Open the ticket");
    assert_eq!(
        part["actions"]["ticket_desk__open_again"]["arguments"],
        json!({ "title": "form.title" })
    );
    assert_eq!(part["live"]["ticket_desk__board_live"]["objects"], true);

    let errors = defined
        .part(&manifest.parts[1].hash)
        .expect("the error messages are found by their hash");
    assert_eq!(
        errors["messages"]["ticket_desk__title_needed"]["text"],
        "A ticket needs a title."
    );
    assert!(
        defined.part(&"0".repeat(64)).is_none(),
        "an unknown hash is no part"
    );

    // The same schema gives the same definition, and a change elsewhere in the schema changes no
    // hash of it.
    assert_eq!(definition(&schema), defined);
    let mut elsewhere = schema.clone();
    elsewhere.version = SchemaVersion(schema.version.0 + 1);
    elsewhere.queries.clear();
    assert_eq!(definition(&elsewhere).manifest.hash, manifest.hash);
}

/// Verifies: INT-3252, INT-3253
///
/// An upgrade that changes the screen's title changes that screen's part and the manifest, not the
/// part of the error messages; the removal of the product takes every element out.
#[test]
fn an_upgrade_changes_the_parts_it_changes_and_a_removal_takes_the_interface_out() {
    let mut world = world();
    installed(&mut world, &ticket_desk());
    let first = definition(&schema_of(&world));

    let mut newer = ticket_desk();
    newer["version"] = json!("1.1.0");
    newer["content"]["ui"][SCREEN]["Screen"]["title"] = json!("Tickets to answer");
    let bundle = read(&newer);
    let upgraded = change(&mut world, |t, e| upgrade_product(&bundle, t, e));
    assert!(
        matches!(upgraded, Ok(ProductInstall::Applied { .. })),
        "{upgraded:?}"
    );
    let second = definition(&schema_of(&world));
    assert_ne!(second.manifest.hash, first.manifest.hash);
    assert_ne!(second.manifest.parts[0].hash, first.manifest.parts[0].hash);
    assert_eq!(second.manifest.parts[1].hash, first.manifest.parts[1].hash);
    assert_eq!(
        second.manifest.parts[0].title.as_deref(),
        Some("Tickets to answer")
    );

    let removed = change(&mut world, |t, e| {
        remove_product("ticket-desk", false, t, e)
    });
    assert!(
        matches!(removed, Ok(ProductInstall::Applied { .. })),
        "{removed:?}"
    );
    let schema = schema_of(&world);
    assert!(
        schema.ui.is_none(),
        "no element is left, and no empty table"
    );
    assert!(definition(&schema).manifest.parts.is_empty());
}

/// Verifies: INT-3252
///
/// A schema change from outside the product does not change one of its elements.
#[test]
fn an_element_a_product_owns_changes_only_through_the_product() {
    let mut world = world();
    installed(&mut world, &ticket_desk());
    let schema = schema_of(&world);
    let Some(UiElement::Screen(mut screen)) = strata_kernel::ui::element(&schema, SCREEN).cloned()
    else {
        panic!("the screen is installed");
    };
    screen.title = "Renamed behind the product".to_string();
    let ctx = Context {
        tenant: tenant(),
        principal: admin(),
        session: None,
        now: Timestamp(1_800_000_000_001),
        correlation: "outside".to_string(),
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
    };
    let intent = Intent::ChangeSchema {
        changes: vec![SchemaChange::DefineUiElement {
            id: SCREEN.to_string(),
            element: UiElement::Screen(screen),
        }],
        expected_version: None,
        existing: ExistingData::Reject,
    };
    let error = world.step(&intent, &ctx).err().expect("refused");
    assert!(error.message.contains("ticket-desk"), "{}", error.message);
    assert!(error.message.contains(SCREEN), "{}", error.message);
}

/// An element that names what the tenant does not hold is refused when it is defined.
#[test]
fn an_element_that_names_nothing_of_the_tenant_is_refused() {
    let mut doc = ticket_desk();
    doc["content"]["ui"][FORM]["Form"]["operation"] = json!("ticket_desk__no_such_operation");
    let bundle = read(&doc);
    let mut world = world();
    let refused = change(&mut world, |t, e| install_product(&bundle, t, e));
    let Err(ProductInstallError::Step { error, .. }) = refused else {
        panic!("the install was not refused by the kernel: {refused:?}");
    };
    assert!(
        error.message.contains("ticket_desk__no_such_operation"),
        "{}",
        error.message
    );
    assert!(schema_of(&world).ui.is_none(), "nothing was installed");
}

#[test]
fn a_title_reads_the_view_model() {
    let model = json!({ "data": { "open": [] }, "title": "Tickets" });
    assert_eq!(title_text("Open tickets", &model), "Open tickets");
    assert_eq!(title_text("${title} (all)", &model), "Tickets (all)");
}
