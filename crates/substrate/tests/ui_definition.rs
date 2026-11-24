#![allow(clippy::err_expect)] // `unwrap_err` would print a whole world when it fails.
//! The interface definition of a tenant (D-8250): a product's screen, form, action, live binding
//! and error message, as `specc bundle` writes them, are installed into the tenant schema by the
//! schema change that installs the product, owned by its record; the definition is read back in
//! parts named by their hashes, with a manifest named by the hash of its list of parts; an upgrade
//! changes the parts it changes, and a removal takes the elements out with the rest.

use serde_json::{json, Value as Json};
use strata_kernel::*;
use strata_substrate::bundle::{
    install_into, install_product, remove_product, upgrade_product, Bundle, ProductBundle,
    ProductInstall, ProductInstallError, ProductTarget,
};
use strata_substrate::ui::definition;

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

/// A screen part carries the shape of what it binds, with a fingerprint: the object a section
/// reads with each field's type and requirement, and the operation of a form or action with its
/// inputs. A change to a field's type changes the shape, its fingerprint and the part's hash.
#[test]
fn a_field_type_change_moves_the_section_fingerprint() {
    let mut world = world();
    installed(&mut world, &ticket_desk());
    let mut schema = schema_of(&world);
    let before = definition(&schema);
    let part = |definition: &strata_substrate::ui::Definition| -> Json {
        let entry = &definition.manifest.parts[0];
        assert_eq!(entry.id, SCREEN);
        definition.part(&entry.hash).unwrap().clone()
    };
    let first = part(&before);
    let section = &first["sections"]["open"];
    assert_eq!(section["single"], false);
    assert_eq!(section["shape"]["object"], "ticket_desk__ticket");
    let title = section["shape"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["id"] == "title")
        .expect("the title field is in the shape");
    assert_eq!(title["type"], "str");
    assert_eq!(title["required"], true);
    assert!(title.get("displayType").is_none(), "none is declared");
    assert_eq!(
        section["shape"]["fingerprint"].as_str().unwrap().len(),
        64,
        "a SHA-256 in hex"
    );
    assert!(
        first["forms"]["ticket_desk__open_form"]["shape"]["operation"].is_string(),
        "a form carries the shape of its operation"
    );

    let object = ObjectTypeId::new("ticket_desk__ticket").unwrap();
    let field = FieldId::new("title").unwrap();
    schema
        .objects
        .get_mut(&object)
        .unwrap()
        .fields
        .get_mut(&field)
        .unwrap()
        .ty = FieldType::Int;
    let after = definition(&schema);
    let second = part(&after);
    assert_eq!(
        second["sections"]["open"]["shape"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["id"] == "title")
            .unwrap()["type"],
        "int"
    );
    assert_ne!(
        second["sections"]["open"]["shape"]["fingerprint"],
        section["shape"]["fingerprint"]
    );
    assert_ne!(after.manifest.parts[0].hash, before.manifest.parts[0].hash);
    assert_ne!(after.manifest.hash, before.manifest.hash);
}

/// The shape of a section carries the display type its object type's presentation declares for
/// each field, and the title field the presentation names.
#[test]
fn a_section_shape_carries_the_declared_display_types_and_title() {
    let mut world = world();
    installed(&mut world, &ticket_desk());
    let mut schema = schema_of(&world);
    let plain = definition(&schema);
    let before = plain.part(&plain.manifest.parts[0].hash).unwrap().clone();
    let fields = [(
        FieldId::new("status").unwrap(),
        UiFieldDisplay {
            display: UiDisplayType::Status,
            props: None,
        },
    )];
    schema.ui.get_or_insert_with(Default::default).insert(
        "ticket_desk__ticket_presentation".to_string(),
        UiElement::Presentation(UiPresentation {
            object: ObjectTypeId::new("ticket_desk__ticket").unwrap(),
            title: Some(FieldId::new("status").unwrap()),
            fields: fields.into_iter().collect(),
        }),
    );
    let shown = definition(&schema);
    let part = shown.part(&shown.manifest.parts[0].hash).unwrap();
    let shape = &part["sections"]["open"]["shape"];
    assert_eq!(shape["title"], "status");
    let status = shape["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "status")
        .unwrap();
    assert_eq!(status["displayType"], "status");
    assert_ne!(
        shape["fingerprint"], before["sections"]["open"]["shape"]["fingerprint"],
        "a declared display type changes the fingerprint"
    );
}
