#![allow(clippy::err_expect)] // `unwrap_err` would print a whole world when it fails.
//! The bundle of a product (`tenant-bundle`, D-6580): it is read, the sections the kernel can take
//! are installed into a tenant that exists, the sections it cannot take are refused by name, and
//! the install is idempotent and records which product owns each entity (D-6582, D-6583).

use serde_json::{json, Value as Json};
use strata_kernel::expr::parse;
use strata_kernel::*;
use strata_substrate::bundle::{
    install_into, install_product, plan_install, plan_remove, plan_upgrade, remove_product,
    upgrade_product, Bundle, BundleError, BundleErrorKind, InstalledRecord, Planned, PlatformRange,
    ProductBundle, ProductInstall, ProductInstallError, ProductTarget,
};

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

/// A small product in the shape specc writes: an object type with a constraint, a role with
/// permissions in each of the three ways a bundle writes one, a policy, a query and a workflow.
fn tiers() -> Json {
    json!({
        "name": "Tiers",
        "version": "1.0.0",
        "product": "tiers",
        "platform": { "min": "0.2", "below": "0.4" },
        "content": {
            "objects": {
                "tiers__member": {
                    "id": "tiers__member",
                    "label": "Member",
                    "fields": {
                        "name": { "id": "name", "label": "Name", "ty": "Str", "required": true, "unique": false },
                        "points": { "id": "points", "label": "Points", "ty": "Int", "required": true, "unique": false, "default": "0" },
                        "status": { "id": "status", "label": "Status", "ty": "Str", "required": false, "unique": false }
                    },
                    "constraints": {
                        "points_not_negative": { "id": "points_not_negative", "expr": "new.points >= 0", "message": "Points are not negative." }
                    }
                }
            },
            "roles": { "tiers__editor": { "id": "tiers__editor", "label": "Tier editor" } },
            "role-permissions": {
                "tiers__editor": [
                    "content-read",
                    { "object": "tiers__member", "actions": ["read", "create", "update"] },
                    { "Capability": "content-write" }
                ]
            },
            "policies": {
                "tiers__editor_members": {
                    "id": "tiers__editor_members",
                    "role": "tiers__editor",
                    "resource": { "Object": "tiers__member" },
                    "actions": ["Read", "Create", "Update"],
                    "condition": "record.points >= 0"
                }
            },
            "queries": {
                "tiers__top": {
                    "id": "tiers__top",
                    "object": "tiers__member",
                    "filter": "record.points >= 100",
                    "params": [],
                    "order": [{ "field": "points", "descending": true }],
                    "limit": 10
                }
            },
            "workflows": {
                "tiers__promotion": {
                    "id": "tiers__promotion",
                    "object": "tiers__member",
                    "field": "status",
                    "initial": ["new"],
                    "transitions": [
                        { "name": "promote", "from": "new", "to": "gold", "guard": "record.points >= 100", "set": {} }
                    ]
                }
            }
        }
    })
}

fn read(doc: &Json) -> Result<ProductBundle, BundleError> {
    ProductBundle::from_json(&doc.to_string())
}

fn refused(doc: &Json) -> BundleError {
    read(doc).err().expect("the bundle is refused")
}

type Installed = Result<ProductInstall, ProductInstallError<DomainError>>;

/// Installs into `world`, which takes the schema change when there is one. Returns the answer and
/// how many intents were run.
fn install(
    world: &mut World,
    bundle: &ProductBundle,
    installed: Option<&InstalledRecord>,
    release: &str,
) -> (Installed, usize) {
    let tenant = tenant();
    let schema = world.tenants[&tenant].schema.clone();
    let target = ProductTarget {
        tenant: &tenant,
        principal: admin(),
        now: 1_800_000_000_000,
        release,
        schema: &schema,
        installed,
        expected_version: None,
        idempotency_key: None,
    };
    let mut calls = 0;
    let result = install_product(bundle, &target, &mut |ctx: Context,
                                                        intent: Intent|
     -> Result<Output, DomainError> {
        calls += 1;
        let (next, outcome) = world.step(&intent, &ctx)?;
        *world = next;
        Ok(outcome.output)
    });
    (result, calls)
}

fn refusal(result: Installed) -> BundleError {
    match result {
        Err(ProductInstallError::Refused(error)) => error,
        other => panic!("the install was not refused: {other:?}"),
    }
}

fn object_id(id: &str) -> ObjectTypeId {
    ObjectTypeId::new(id).unwrap()
}

// Verifies: INT-1580
#[test]
fn a_product_bundle_is_read_as_kernel_values() {
    let bundle = read(&tiers()).unwrap();
    assert_eq!(bundle.name, "Tiers");
    assert_eq!(bundle.version, "1.0.0");
    assert_eq!(bundle.product.as_deref(), Some("tiers"));
    assert_eq!(
        bundle.platform,
        Some(PlatformRange {
            min: "0.2".to_string(),
            below: Some("0.4".to_string())
        })
    );
    assert!(bundle.unbuilt.is_empty());
    let member = &bundle.objects[&object_id("tiers__member")];
    assert_eq!(member.fields.len(), 3);
    // Expressions written as text are read with the tenant rule language.
    let constraint = member.constraints.values().next().unwrap();
    assert_eq!(constraint.expr, parse("new.points >= 0").unwrap());
    let permissions = &bundle.role_permissions[&RoleId::new("tiers__editor").unwrap()];
    assert_eq!(permissions.len(), 3);
    assert_eq!(
        permissions[0],
        Permission::Capability("content-read".into())
    );
    assert!(
        matches!(&permissions[1], Permission::Object { object, .. } if object.to_string() == "tiers__member")
    );
    assert_eq!(
        permissions[2],
        Permission::Capability("content-write".into())
    );
    let ids: Vec<(String, String)> = bundle
        .entities()
        .into_iter()
        .map(|e| (e.kind, e.id))
        .collect();
    let expected: Vec<(String, String)> = [
        ("role", "tiers__editor"),
        ("object", "tiers__member"),
        ("policy", "tiers__editor_members"),
        ("workflow", "tiers__promotion"),
        ("query", "tiers__top"),
    ]
    .into_iter()
    .map(|(k, i)| (k.to_string(), i.to_string()))
    .collect();
    assert_eq!(ids, expected);
}

#[test]
fn a_malformed_bundle_is_refused_as_an_invalid_bundle() {
    let cases: Vec<(&str, Json, &str)> = vec![
        (
            "unknown member",
            {
                let mut doc = tiers();
                doc["owner"] = json!("user:x");
                doc
            },
            "\"owner\"",
        ),
        (
            "no version",
            {
                let mut doc = tiers();
                doc.as_object_mut().unwrap().remove("version");
                doc
            },
            "version",
        ),
        (
            "unknown section",
            {
                let mut doc = tiers();
                doc["content"]["pages"] = json!({});
                doc
            },
            "\"pages\"",
        ),
        (
            "key is not the id",
            {
                let mut doc = tiers();
                doc["content"]["roles"]["tiers__editor"]["id"] = json!("tiers__other");
                doc
            },
            "tiers__editor",
        ),
        (
            "expression does not parse",
            {
                let mut doc = tiers();
                doc["content"]["objects"]["tiers__member"]["constraints"]["points_not_negative"]
                    ["expr"] = json!("new.points >=");
                doc
            },
            "points_not_negative",
        ),
        (
            "permissions of a role that is not declared",
            {
                let mut doc = tiers();
                doc["content"]["role-permissions"]["tiers__ghost"] = json!(["content-read"]);
                doc
            },
            "tiers__ghost",
        ),
        (
            "a list where a map belongs",
            {
                let mut doc = tiers();
                doc["content"]["policies"] = json!([]);
                doc
            },
            "policies",
        ),
    ];
    for (what, doc, word) in cases {
        let error = refused(&doc);
        assert_eq!(error.kind, BundleErrorKind::InvalidBundle, "{what}");
        assert!(error.message.contains(word), "{what}: {}", error.message);
    }
    assert_eq!(
        ProductBundle::from_json("{ not json").err().unwrap().kind,
        BundleErrorKind::InvalidBundle
    );
}

// Verifies: INT-1583
#[test]
fn a_release_outside_the_range_of_the_bundle_is_refused() {
    let bundle = read(&tiers()).unwrap();
    let mut world = world();
    let before = world.hash();
    for release in ["0.1", "0.4", "1.0"] {
        let (result, calls) = install(&mut world, &bundle, None, release);
        let error = refusal(result);
        assert_eq!(error.kind, BundleErrorKind::Invalid, "{release}");
        assert!(error.message.contains(release), "{}", error.message);
        assert_eq!(calls, 0);
    }
    assert_eq!(world.hash(), before);
}

// Verifies: INT-1582
#[test]
fn an_install_creates_the_entities_and_records_the_product_that_owns_them() {
    let bundle = read(&tiers()).unwrap();
    let mut world = world();
    let (result, calls) = install(&mut world, &bundle, None, "0.2-draft");
    assert_eq!(calls, 1, "the edits of a bundle are one schema change");
    let ProductInstall::Applied {
        schema_version,
        record: Some(record),
    } = result.unwrap_or_else(|e| panic!("{e}"))
    else {
        panic!("the install was not applied");
    };
    let schema = &world.tenants[&tenant()].schema;
    assert_eq!(schema.version.0, schema_version);
    assert!(schema.objects.contains_key(&object_id("tiers__member")));
    assert!(schema
        .roles
        .contains_key(&RoleId::new("tiers__editor").unwrap()));
    assert!(schema
        .policies
        .contains_key(&PolicyId::new("tiers__editor_members").unwrap()));
    assert!(schema
        .queries
        .contains_key(&QueryId::new("tiers__top").unwrap()));
    assert!(schema
        .workflows
        .contains_key(&WorkflowId::new("tiers__promotion").unwrap()));
    // The permissions of the role are the three of the bundle.
    let granted = schema
        .role_permissions
        .as_ref()
        .and_then(|all| all.get(&RoleId::new("tiers__editor").unwrap()))
        .map(Vec::len);
    assert_eq!(granted, Some(3));

    assert_eq!(record.product, "tiers");
    assert_eq!(record.version, "1.0.0");
    assert_eq!(record.schema_version, Some(schema_version));
    let owns: Vec<(&str, &str)> = record
        .owns
        .iter()
        .map(|e| (e.kind.as_str(), e.id.as_str()))
        .collect();
    assert_eq!(
        owns,
        vec![
            ("role", "tiers__editor"),
            ("object", "tiers__member"),
            ("policy", "tiers__editor_members"),
            ("workflow", "tiers__promotion"),
            ("query", "tiers__top"),
        ]
    );
    // The catalogue document round-trips, so the store can keep it as JSON text.
    let text = record.to_json().unwrap();
    assert_eq!(InstalledRecord::from_json(&text).unwrap(), record);
}

// Verifies: RUN-004
#[test]
fn installing_the_same_bundle_into_two_empty_tenants_gives_the_same_state() {
    let bundle = read(&tiers()).unwrap();
    let mut first = world();
    let mut second = world();
    install(&mut first, &bundle, None, "0.2").0.unwrap();
    install(&mut second, &bundle, None, "0.2").0.unwrap();
    assert_eq!(first.hash(), second.hash());
}

// Verifies: INT-1583
#[test]
fn installing_the_same_version_again_changes_nothing_and_returns_the_first_schema_version() {
    let bundle = read(&tiers()).unwrap();
    let mut world = world();
    let (first, _) = install(&mut world, &bundle, None, "0.2");
    let ProductInstall::Applied {
        schema_version,
        record: Some(record),
    } = first.unwrap_or_else(|e| panic!("{e}"))
    else {
        panic!("the install was not applied");
    };
    let before = world.hash();
    let (again, calls) = install(&mut world, &bundle, Some(&record), "0.2");
    assert_eq!(calls, 0, "a repeat runs no intent");
    assert_eq!(
        again.unwrap_or_else(|e| panic!("{e}")),
        ProductInstall::Unchanged { schema_version }
    );
    assert_eq!(world.hash(), before);
}

// Verifies: INT-1583
#[test]
fn another_version_of_an_installed_product_is_an_upgrade_and_not_an_install() {
    let bundle = read(&tiers()).unwrap();
    let mut world = world();
    let (first, _) = install(&mut world, &bundle, None, "0.2");
    let ProductInstall::Applied {
        record: Some(record),
        ..
    } = first.unwrap_or_else(|e| panic!("{e}"))
    else {
        panic!("the install was not applied");
    };
    let mut doc = tiers();
    doc["version"] = json!("1.1.0");
    let newer = read(&doc).unwrap();
    let before = world.hash();
    let (result, calls) = install(&mut world, &newer, Some(&record), "0.2");
    let error = refusal(result);
    assert_eq!(error.kind, BundleErrorKind::Conflict);
    assert!(
        error.message.contains("upgrade-bundle"),
        "{}",
        error.message
    );
    assert_eq!(calls, 0);
    assert_eq!(world.hash(), before);
}

// Verifies: INT-1582
#[test]
fn an_entity_that_exists_and_is_not_owned_by_the_product_blocks_the_install() {
    let bundle = read(&tiers()).unwrap();
    let mut world = world();
    // The same entities installed by a bundle that names no product: they belong to the tenant.
    let mut template = tiers();
    template.as_object_mut().unwrap().remove("product");
    let template = read(&template).unwrap();
    install(&mut world, &template, None, "0.2").0.unwrap();
    let before = world.hash();
    let (result, calls) = install(&mut world, &bundle, None, "0.2");
    let error = refusal(result);
    assert_eq!(error.kind, BundleErrorKind::Conflict);
    assert!(error.message.contains("tiers__editor"), "{}", error.message);
    assert_eq!(calls, 0);
    assert_eq!(world.hash(), before);
}

// Verifies: INT-1582
#[test]
fn an_id_outside_the_namespace_of_the_product_is_refused() {
    let mut doc = tiers();
    doc["product"] = json!("other-product");
    let bundle = read(&doc).unwrap();
    let mut world = world();
    let before = world.hash();
    let (result, calls) = install(&mut world, &bundle, None, "0.2");
    let error = refusal(result);
    assert_eq!(error.kind, BundleErrorKind::Invalid);
    assert!(
        error.message.contains("other_product__") && error.message.contains("tiers__editor"),
        "{}",
        error.message
    );
    assert_eq!(calls, 0);
    assert_eq!(world.hash(), before);
}

/// The bundle of `tiers` with one segment declared in its namespace (WG-5166).
fn tiers_with_segment() -> Json {
    let mut doc = tiers();
    doc["content"]["segments"] = json!({
        "tiers__gold": {
            "name": "Gold",
            "subject-type": "tiers__member",
            "predicate": "record.points >= 100",
            "principal-field": "name",
            "visible-to-members": true
        }
    });
    doc
}

fn gold_key() -> strata_kernel::segments::SegmentKey {
    strata_kernel::segments::SegmentKey::new("tiers__gold").unwrap()
}

// Verifies: INT-155
#[test]
fn a_segments_section_is_read_planned_and_owned_but_is_no_schema_change() {
    let doc = tiers_with_segment();
    let bundle = read(&doc).unwrap();
    assert!(bundle.unbuilt.is_empty(), "no section is refused any more");
    assert_eq!(bundle.segments.len(), 1);
    // The predicate is read with the rule language: a bundle whose predicate does not parse is
    // refused when it is read.
    let mut broken = tiers_with_segment();
    broken["content"]["segments"]["tiers__gold"]["predicate"] = json!("record.points >=");
    assert_eq!(refused(&broken).kind, BundleErrorKind::InvalidBundle);

    let mut world = world();
    let schema = world.tenants[&tenant()].schema.clone();
    let owner = tenant();
    let target = target_of(&owner, &schema, "0.2");
    let Planned::Change(plan) = plan_install(&bundle, &target).unwrap() else {
        panic!("the install is a change")
    };
    // The segment is defined through the segment store, with the id derived from the tenant and
    // the key (D-6582); the schema change carries none of it.
    assert_eq!(plan.segments.define.len(), 1);
    assert!(plan.segments.remove.is_empty());
    let defs = plan.segments.definitions(&tenant()).unwrap();
    assert_eq!(
        defs[0].id,
        strata_kernel::segments::derive_segment_id(&tenant(), &gold_key())
    );
    assert_eq!(defs[0].name, "Gold");
    assert!(defs[0].visible_to_members);
    assert_eq!(defs[0].principal_field, FieldId::new("name").unwrap());
    assert!(plan
        .created
        .iter()
        .any(|e| e.kind == "segment" && e.id == "tiers__gold"));
    // The ownership record the one schema change writes lists it.
    let record = plan.record.clone().expect("a product leaves a record");
    assert!(record
        .owns
        .iter()
        .any(|e| e.kind == "segment" && e.id == "tiers__gold"));

    let (result, calls) = install(&mut world, &bundle, None, "0.2");
    result.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(calls, 1, "one schema change for the whole bundle");
    let held = InstalledRecord::of(&world.tenants[&tenant()].schema, "tiers").unwrap();
    assert!(held
        .owns
        .iter()
        .any(|e| e.kind == "segment" && e.id == "tiers__gold"));
    // The same bundle again is the same install: nothing runs.
    let (again, calls) = install(&mut world, &bundle, None, "0.2");
    assert!(matches!(again, Ok(ProductInstall::Unchanged { .. })));
    assert_eq!(calls, 0);
}

// Verifies: INT-1582
#[test]
fn a_segment_key_outside_the_namespace_of_the_product_is_refused() {
    let mut doc = tiers_with_segment();
    let decl = doc["content"]["segments"]["tiers__gold"].clone();
    doc["content"]["segments"] = json!({ "gold": decl });
    let bundle = read(&doc).unwrap();
    let error = bundle.check_installable().unwrap_err();
    assert_eq!(error.kind, BundleErrorKind::Invalid);
    assert!(error.message.contains("\"gold\""), "{}", error.message);
}

#[test]
fn an_upgrade_defines_the_segments_it_declares_and_removes_those_it_drops() {
    let mut world = world();
    installed(&mut world, &tiers_with_segment());
    let mut newer = tiers_with_segment();
    newer["version"] = json!("1.1.0");
    let decl = newer["content"]["segments"]["tiers__gold"].clone();
    newer["content"]["segments"] = json!({ "tiers__silver": decl });
    let bundle = read(&newer).unwrap();
    let schema = world.tenants[&tenant()].schema.clone();
    let owner = tenant();
    let target = target_of(&owner, &schema, "0.2");
    let Planned::Change(plan) = plan_upgrade(&bundle, &target).unwrap() else {
        panic!("the upgrade is a change")
    };
    let defined: Vec<&str> = plan
        .segments
        .define
        .iter()
        .map(|(key, _)| key.0.as_str())
        .collect();
    assert_eq!(defined, vec!["tiers__silver"]);
    assert_eq!(plan.segments.remove, vec![gold_key()]);
    assert!(plan
        .removed
        .iter()
        .any(|e| e.kind == "segment" && e.id == "tiers__gold"));
    assert!(plan
        .created
        .iter()
        .any(|e| e.kind == "segment" && e.id == "tiers__silver"));
    let record = plan.record.unwrap();
    assert!(record.owns.iter().any(|e| e.id == "tiers__silver"));
    assert!(!record.owns.iter().any(|e| e.id == "tiers__gold"));
}

#[test]
fn a_removal_names_every_segment_the_product_owns() {
    let mut world = world();
    installed(&mut world, &tiers_with_segment());
    let schema = world.tenants[&tenant()].schema.clone();
    let owner = tenant();
    let target = target_of(&owner, &schema, "0.2");
    let Planned::Change(plan) = plan_remove("tiers", false, &target).unwrap() else {
        panic!("the removal is a change")
    };
    assert_eq!(plan.segments.remove, vec![gold_key()]);
    assert!(plan.segments.define.is_empty());
    assert_eq!(
        plan.segments.removals(&tenant()),
        vec![strata_kernel::segments::derive_segment_id(
            &tenant(),
            &gold_key()
        )]
    );
    // No schema change removes a segment: the schema edits are the other entities and the record.
    assert!(plan
        .removed
        .iter()
        .any(|e| e.kind == "segment" && e.id == "tiers__gold"));
}

/// `tiers` with a capability, a setting and the three ledger sections in its namespace.
fn tiers_with_catalogue() -> Json {
    let mut doc = tiers();
    let content = &mut doc["content"];
    content["capabilities"] =
        json!([{ "id": "tiers__promote", "name": "Promote", "description": "Promote a member." }]);
    content["settings"] = json!([{
        "id": "tiers__gold_threshold",
        "label": "Points for gold",
        "type": "i64",
        "default": { "Int": 100 },
        "levels": ["tenant", "team", "person"]
    }]);
    content["ledger-units"] =
        json!([{ "unit": "tiers__points", "description": "Points a member earns." }]);
    content["ledger-pools"] = json!([{ "pool": "tiers__pool", "description": "The pool." }]);
    content["ledger-limits"] = json!([{
        "unit": "tiers__points",
        "owner": { "pool": { "pool": "tiers__pool" } },
        "floor": 0
    }]);
    doc
}

fn owned(record: &InstalledRecord) -> Vec<(String, String)> {
    record
        .owns
        .iter()
        .map(|e| (e.kind.clone(), e.id.clone()))
        .collect()
}

/// Capabilities, settings and the ledger sections install, and the record of the install owns
/// each of them. The one section still named as not installed is `event-kinds`
/// (`the_event_kinds_of_a_bundle_are_read_and_checked`).
// Verifies: INT-1921
#[test]
fn capabilities_settings_and_the_ledger_sections_install_and_are_owned_by_the_product() {
    let doc = tiers_with_catalogue();
    let bundle = read(&doc).unwrap();
    assert!(bundle.unbuilt.is_empty(), "no section is refused by name");
    let mut world = world();
    let (result, calls) = install(&mut world, &bundle, None, "0.2");
    assert_eq!(calls, 1, "one schema change for the whole bundle");
    let ProductInstall::Applied {
        record: Some(record),
        ..
    } = result.unwrap_or_else(|e| panic!("{e}"))
    else {
        panic!("the install was not applied");
    };
    let owns = owned(&record);
    for (kind, id) in [
        ("capability", "tiers__promote"),
        ("setting", "tiers__gold_threshold"),
        ("ledger-unit", "tiers__points"),
        ("ledger-pool", "tiers__pool"),
        ("ledger-limit", "tiers__points#pool:tiers__pool#"),
    ] {
        assert!(
            owns.contains(&(kind.to_string(), id.to_string())),
            "{kind} {id}: {owns:?}"
        );
    }
    // The schema holds what the record owns.
    let schema = schema_of(&world);
    assert!(schema
        .capabilities
        .as_ref()
        .is_some_and(|c| c.contains_key(&CapabilityName("tiers__promote".into()))));
    assert!(schema
        .settings
        .as_ref()
        .is_some_and(|s| s.contains_key(&SettingId::new("tiers__gold_threshold").unwrap())));
    let ledger = schema.ledger.as_ref().expect("the ledger is declared");
    assert!(ledger
        .units
        .contains_key(&LedgerUnit::new("tiers__points").unwrap()));
    assert!(ledger
        .pools
        .contains_key(&LedgerPoolName::new("tiers__pool").unwrap()));
    assert_eq!(InstalledRecord::of(&schema, "tiers"), Some(record));
}

/// A removal finds the capability, the setting and the ledger declarations through the record.
// Verifies: INT-1582
#[test]
fn a_removal_takes_out_the_capabilities_settings_and_ledger_declarations_a_product_owns() {
    let mut world = world();
    installed(&mut world, &tiers_with_catalogue());
    let schema = schema_of(&world);
    let owner = tenant();
    let target = target_of(&owner, &schema, "0.2");
    let Planned::Change(plan) = plan_remove("tiers", false, &target).unwrap() else {
        panic!("the removal is a change")
    };
    let removed = entity_ids(&plan.removed);
    for want in [
        "capability:tiers__promote",
        "setting:tiers__gold_threshold",
        "ledger-unit:tiers__points",
        "ledger-pool:tiers__pool",
        "ledger-limit:tiers__points#pool:tiers__pool#",
    ] {
        assert!(removed.iter().any(|r| r == want), "{want}: {removed:?}");
    }
    let (result, calls) = with_target(&mut world, "0.2", |t, e| {
        remove_product("tiers", false, t, e)
    });
    result.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(calls, 1);
    let schema = schema_of(&world);
    assert!(schema.capabilities.as_ref().is_none_or(|c| c.is_empty()));
    assert!(schema.settings.as_ref().is_none_or(|s| s.is_empty()));
    assert!(schema
        .ledger
        .as_ref()
        .is_none_or(|l| l.units.is_empty() && l.pools.is_empty()));
    assert!(InstalledRecord::of(&schema, "tiers").is_none());
}

/// The event kinds of a bundle are read (SCH-1576, INT-3203): each begins with `app.` and a name,
/// has at most sixteen fields and is declared once; a bundle that fills the section is named as
/// holding something this release does not install.
// Verifies: INT-3203
#[test]
fn the_event_kinds_of_a_bundle_are_read_and_checked() {
    let mut doc = tiers();
    doc["content"]["event-kinds"] = json!([
        { "kind": "app.order-placed", "fields": [{ "name": "order", "type": "string" }], "readers": [] },
        { "kind": "app.order-shipped", "fields": [], "readers": [] }
    ]);
    let bundle = read(&doc).unwrap();
    assert_eq!(
        bundle
            .event_kinds
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["app.order-placed", "app.order-shipped"]
    );
    assert_eq!(bundle.unbuilt.len(), 1);
    assert_eq!(bundle.unbuilt[0].section, "event-kinds");
    assert_eq!(bundle.unbuilt[0].entries, 2);

    for (kinds, wanted) in [
        (
            json!([{ "kind": "order-placed", "fields": [], "readers": [] }]),
            "must begin with app.",
        ),
        (
            json!([{ "kind": "app.", "fields": [], "readers": [] }]),
            "must begin with app.",
        ),
        (
            json!([
                { "kind": "app.a", "fields": [], "readers": [] },
                { "kind": "app.a", "fields": [], "readers": [] }
            ]),
            "declared twice",
        ),
    ] {
        let mut doc = tiers();
        doc["content"]["event-kinds"] = kinds;
        let error = refused(&doc);
        assert_eq!(error.kind, BundleErrorKind::InvalidBundle);
        assert!(error.message.contains(wanted), "{}", error.message);
    }
}

// The bundles specc writes from the fixture products (tests/bundle.rs of the spec-compiler).
const BADGE_MILESTONES: &str = include_str!(
    "../../spec-compiler/tests/fixtures/products/expected/badge-milestones.bundle.json"
);
const PROMOTIONS: &str =
    include_str!("../../spec-compiler/tests/fixtures/products/expected/promotions.bundle.json");
const MEMBER_TIERS: &str =
    include_str!("../../spec-compiler/tests/fixtures/products/expected/member-tiers.bundle.json");

// Verifies: INT-1580
#[test]
fn the_bundles_specc_writes_are_read_and_their_unbuilt_sections_are_named() {
    let tiers = ProductBundle::from_json(MEMBER_TIERS).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(tiers.product.as_deref(), Some("member-tiers"));
    assert_eq!(tiers.roles.len(), 1);
    let permissions = &tiers.role_permissions[&RoleId::new("member_tiers__editor").unwrap()];
    assert_eq!(
        permissions,
        &vec![
            Permission::Capability("member_tiers__promote".into()),
            Permission::Capability("content-read".into())
        ]
    );
    // The capabilities and the ledger units are schema changes: read, not refused.
    assert!(tiers.unbuilt.is_empty());
    assert_eq!(tiers.capabilities.len(), 1);
    assert_eq!(tiers.ledger_units.len(), 1);
    assert_eq!(tiers.mail_templates.len(), 1);

    let badges = ProductBundle::from_json(BADGE_MILESTONES).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(badges.objects.len(), 1);
    assert_eq!(badges.policies.len(), 2);
    // The ledger unit its trigger names is declared by the product.
    assert!(badges.unbuilt.is_empty());
    assert_eq!(badges.ledger_units.len(), 1);
    assert_eq!(badges.mail_templates.len(), 2);
    assert_eq!(badges.notice_templates.len(), 1);
    assert_eq!(badges.triggers.len(), 1);
    assert_eq!(badges.automations.len(), 1);
    let award = &badges.objects[&object_id("badge_milestones__award")];
    // The retention and the personal declaration of the field survive the reading.
    assert_eq!(award.retention.as_ref().map(|r| r.days), Some(730));
    assert!(award.fields[&FieldId::new("person").unwrap()]
        .personal
        .is_some());
    badges.check_installable().unwrap_or_else(|e| panic!("{e}"));

    let promotions = ProductBundle::from_json(PROMOTIONS).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(promotions.objects.len(), 2);
    assert_eq!(promotions.operations.len(), 2);
    assert!(promotions.unbuilt.is_empty());
    assert_eq!(promotions.ledger_units.len(), 2);
    assert_eq!(promotions.ledger_pools.len(), 1);
    // The floor of the stock and the cap of one redemption per person and promotion.
    assert_eq!(promotions.ledger_limits.len(), 2);
    // The changes that install it declare the ledger before the operations that post to it.
    let changes = promotions.schema_changes();
    let first_ledger = changes
        .iter()
        .position(|c| matches!(c, SchemaChange::DeclareLedgerUnit(_)))
        .unwrap();
    let first_operation = changes
        .iter()
        .position(|c| matches!(c, SchemaChange::DefineOperation(_)))
        .unwrap();
    assert!(first_ledger < first_operation);
    assert!(changes
        .iter()
        .any(|c| matches!(c, SchemaChange::SetLedgerLimit(_))));
}

/// The redeem operation of the promotions product, installed from the bundle specc writes: one
/// invocation checks the window, debits the stock of the promotion, counts the redemption for the
/// person and creates the redemption record. The cap of the redemptions unit is the limit per
/// person: a second redemption of the same promotion by the same person is refused, and the
/// refusal leaves the stock and the records as they were (WG-5032, OPR-1542, LDG-013).
#[test]
fn the_promotions_redeem_operation_admits_one_redemption_per_person() {
    let mut world = world();
    let bundle = ProductBundle::from_json(PROMOTIONS).unwrap_or_else(|e| panic!("{e}"));
    let (result, _) = install(&mut world, &bundle, None, "0.2");
    result.unwrap_or_else(|e| panic!("{e}"));

    const DAY: i64 = 86_400_000;
    let start = 1_800_000_100_000;
    let step = |world: &World, now: i64, intent: Intent| {
        world.step(&intent, &context_at(now, "promotions"))
    };
    let promotion_type = object_id("promotions__promotion");
    let (next, _) = step(
        &world,
        start,
        Intent::CreateRecord {
            object: promotion_type.clone(),
            fields: std::collections::BTreeMap::from([
                (FieldId::new("title").unwrap(), Value::Str("Spring".into())),
                (FieldId::new("starts_at").unwrap(), Value::Time(start)),
                (
                    FieldId::new("ends_at").unwrap(),
                    Value::Time(start + 30 * DAY),
                ),
            ]),
        },
    )
    .unwrap_or_else(|e| panic!("the promotion is created: {}", e.message));
    world = next;
    let promotion = world.tenants[&tenant()]
        .records_of(&promotion_type)
        .next()
        .expect("the promotion exists")
        .id;
    // The stock of this promotion: the pool account in the scope of its id, as text(record.$id) writes it.
    let stock = |amount: i64, key: &str| LedgerPostingRequest {
        owner: LedgerOwner::Pool {
            pool: LedgerPoolName::new("promotions__pool").unwrap(),
        },
        unit: LedgerUnit::new("promotions__stock").unwrap(),
        amount: LedgerAmount(amount),
        idempotency_key: key.into(),
        reason: "stock".into(),
        corrects: None,
        scope: Some(LedgerScope::new(&promotion.0.to_string()).unwrap()),
    };
    let (next, _) = step(
        &world,
        start + 1,
        Intent::PostLedger {
            postings: vec![stock(10, "fund")],
        },
    )
    .unwrap_or_else(|e| panic!("the stock is funded: {}", e.message));
    world = next;

    let redeem = |key: &str| Intent::InvokeOperation {
        operation: TenantOperationId::new("promotions__redeem").unwrap(),
        inputs: [("promotion".to_string(), Value::Ref(promotion))]
            .into_iter()
            .collect(),
        idempotency_key: key.to_string(),
    };
    let redemption_type = object_id("promotions__redemption");
    let redemptions = |world: &World| {
        world.tenants[&tenant()]
            .records_of(&redemption_type)
            .count()
    };
    let (next, _) = step(&world, start + DAY, redeem("first"))
        .unwrap_or_else(|e| panic!("the first redemption is accepted: {}", e.message));
    world = next;
    assert_eq!(redemptions(&world), 1);

    let before = world.hash();
    let error = step(&world, start + DAY + 1, redeem("second"))
        .err()
        .expect("a second redemption by the same person is refused");
    assert!(
        error.message.contains("ledger:above-cap"),
        "{}",
        error.message
    );
    assert_eq!(world.hash(), before);
    assert_eq!(redemptions(&world), 1);
}

// ---- what a product holds beyond its record types (WG-5076, WG-5084, WG-5103) ----------------------

fn schema_of(world: &World) -> std::sync::Arc<TenantSchema> {
    world.tenants[&tenant()].schema.clone()
}

/// The hash of the schema of the tenant, whatever its version.
fn schema_hash(world: &World) -> String {
    strata_kernel::canon::hash_of(&TenantSchema {
        version: SchemaVersion(0),
        ..TenantSchema::clone(&schema_of(world))
    })
}

fn context_at(now: i64, correlation: &str) -> Context {
    Context {
        tenant: tenant(),
        principal: admin(),
        session: None,
        now: Timestamp(now),
        correlation: correlation.into(),
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
    }
}

/// A tenant that exists and has the role `member`, which the policies of a product name.
fn world_with_member() -> World {
    let intent = Intent::ChangeSchema {
        changes: vec![SchemaChange::DefineRole(RoleDef {
            id: RoleId::new("member").unwrap(),
            label: "Member".into(),
        })],
        expected_version: None,
        existing: ExistingData::Reject,
    };
    world()
        .step(&intent, &context_at(1_700_000_000_000, "fixture"))
        .expect("accepted")
        .0
}

type Executor<'a> = &'a mut dyn FnMut(Context, Intent) -> Result<Output, DomainError>;

fn target_of<'a>(
    tenant: &'a TenantId,
    schema: &'a TenantSchema,
    release: &'a str,
) -> ProductTarget<'a> {
    ProductTarget {
        tenant,
        principal: admin(),
        now: 1_800_000_000_000,
        release,
        schema,
        installed: None,
        expected_version: None,
        idempotency_key: None,
    }
}

/// Runs `work` against the tenant of `world`, which takes each intent as it comes. Returns the
/// answer and how many intents were run.
fn with_target<R>(
    world: &mut World,
    release: &str,
    work: impl FnOnce(&ProductTarget<'_>, Executor<'_>) -> R,
) -> (R, usize) {
    let tenant = tenant();
    let schema = world.tenants[&tenant].schema.clone();
    let target = target_of(&tenant, &schema, release);
    let mut calls = 0;
    let result = work(&target, &mut |ctx: Context,
                                     intent: Intent|
     -> Result<Output, DomainError> {
        calls += 1;
        let (next, outcome) = world.step(&intent, &ctx)?;
        *world = next;
        Ok(outcome.output)
    });
    (result, calls)
}

fn installed(world: &mut World, doc: &Json) -> InstalledRecord {
    let bundle = read(doc).unwrap();
    let (result, _) = with_target(world, "0.2", |t, e| install_product(&bundle, t, e));
    match result.unwrap_or_else(|e| panic!("{e}")) {
        ProductInstall::Applied {
            record: Some(record),
            ..
        } => record,
        other => panic!("the install was not applied: {other:?}"),
    }
}

fn step_error(result: Installed) -> DomainError {
    match result {
        Err(ProductInstallError::Step { error, .. }) => error,
        other => panic!("the kernel did not refuse the change: {other:?}"),
    }
}

// Verifies: INT-2340
#[test]
fn an_object_type_with_a_personal_field_and_a_retention_installs_and_holds_both() {
    let mut doc = tiers();
    let member = &mut doc["content"]["objects"]["tiers__member"];
    member["retention"] = json!({ "days": 30, "from": "Created" });
    member["fields"]["name"]["personal"] =
        json!({ "kind": "name", "subject": "Creator", "searchable": false });
    // The names of the specification are read as the generated types write them.
    member["fields"]["lang"] = json!({ "id": "lang", "label": "Language", "ty": "Language", "required": false, "unique": false });
    member["language-field"] = json!("lang");
    let mut world = world();
    installed(&mut world, &doc);
    let schema = schema_of(&world);
    let member = &schema.objects[&object_id("tiers__member")];
    assert_eq!(member.retention.as_ref().map(|r| r.days), Some(30));
    assert_eq!(member.language_field, Some(FieldId::new("lang").unwrap()));
    let name = member.fields[&FieldId::new("name").unwrap()]
        .personal
        .as_ref()
        .expect("the declaration is stored with the field");
    assert_eq!(name.kind, PersonalKind::Name);
    assert!(!name.searchable);
}

// Verifies: INT-2340
#[test]
fn a_personal_field_without_a_retention_is_refused_by_the_kernel() {
    let mut doc = tiers();
    doc["content"]["objects"]["tiers__member"]["fields"]["name"]["personal"] =
        json!({ "kind": "name", "subject": "Creator", "searchable": false });
    let bundle = read(&doc).unwrap();
    let mut world = world();
    let before = world.hash();
    let (result, calls) = install(&mut world, &bundle, None, "0.2");
    let error = step_error(result);
    assert_eq!(calls, 1);
    assert!(
        error
            .message
            .contains("a type with a personal field must declare retention (D-6584)"),
        "{}",
        error.message
    );
    assert_eq!(world.hash(), before);
}

/// The product that reacts to the ledger: its object type, templates, trigger and automation.
fn badge_milestones() -> Json {
    let mut doc: Json = serde_json::from_str(BADGE_MILESTONES).unwrap();
    // The ledger declarations are the one section the kernel cannot take yet.
    doc["content"]
        .as_object_mut()
        .unwrap()
        .remove("ledger-units");
    doc
}

// Verifies: INT-1921
#[test]
fn badge_milestones_installs_with_its_templates_its_trigger_and_its_automation() {
    let mut world = world_with_member();
    let record = installed(&mut world, &badge_milestones());
    let schema = schema_of(&world);
    assert!(schema
        .objects
        .contains_key(&object_id("badge_milestones__award")));
    assert_eq!(schema.triggers.as_ref().map(|t| t.len()), Some(1));
    assert_eq!(schema.automations.as_ref().map(|t| t.len()), Some(1));
    let templates: Vec<String> = strata_kernel::templates::list(&schema)
        .into_iter()
        .map(|t| t.id.to_string())
        .collect();
    assert_eq!(
        templates,
        vec![
            "badge_milestones__silver_follow_up",
            "badge_milestones__silver_mail",
            "badge_milestones__silver_notice"
        ]
    );
    // The record names what the product owns, templates included, and the schema holds it.
    let kinds: Vec<&str> = record.owns.iter().map(|e| e.kind.as_str()).collect();
    for kind in [
        "role",
        "object",
        "policy",
        "mail-template",
        "notice-template",
        "trigger",
        "automation",
    ] {
        assert!(kinds.contains(&kind), "{kind}: {kinds:?}");
    }
    assert_eq!(
        InstalledRecord::of(&schema, "badge-milestones"),
        Some(record)
    );
}

// Verifies: INT-2344
#[test]
fn an_automation_that_sends_a_template_nobody_declares_is_refused_before_it_runs() {
    let mut doc = tiers();
    doc["content"]["automations"] = json!([{
        "id": "tiers__greet",
        "label": "Greet",
        "run-as": { "roles": [] },
        "start": "send",
        "steps": { "send": { "action": { "SendMail": { "template": "tiers__nothing", "to": "param.owner" } } } }
    }]);
    let bundle = read(&doc).unwrap();
    let mut world = world();
    let (result, calls) = install(&mut world, &bundle, None, "0.2");
    let error = refusal(result);
    assert_eq!(error.kind, BundleErrorKind::InvalidBundle);
    assert!(
        error.message.contains("tiers__nothing"),
        "{}",
        error.message
    );
    assert_eq!(calls, 0);
}

// Verifies: INT-2344
#[test]
fn an_automation_that_names_an_unknown_step_is_refused_with_the_kernels_problem_text() {
    let mut doc = tiers();
    doc["content"]["automations"] = json!([{
        "id": "tiers__greet",
        "label": "Greet",
        "run-as": { "roles": [] },
        "start": "send",
        "steps": { "send": { "action": "End", "next": "ghost" } }
    }]);
    let bundle = read(&doc).unwrap();
    let mut world = world();
    let before = world.hash();
    let (result, _) = install(&mut world, &bundle, None, "0.2");
    let error = step_error(result);
    assert_eq!(error.kind, ErrorKind::InvalidSchema);
    assert!(
        error.message.contains("unknown step ghost"),
        "{}",
        error.message
    );
    assert_eq!(world.hash(), before);
}

// Verifies: INT-1580
#[test]
fn a_trigger_an_automation_and_an_operation_in_the_authoring_syntax_are_installed() {
    let mut doc = tiers();
    doc["content"]["automations"] = json!([{
        "id": "tiers__promote",
        "label": "Promote",
        "run-as": { "roles": [] },
        "run-key": "param.record",
        "start": "mark",
        "steps": {
            "mark": { "action": { "UpdateRecord": { "object": "tiers__member", "record": "param.record", "fields": { "status": "'gold'" } } } }
        }
    }]);
    doc["content"]["triggers"] = json!([{
        "id": "tiers__on_update",
        "on": { "RecordUpdated": { "object": "tiers__member" } },
        "condition": "new.points >= 100",
        "automation": "tiers__promote"
    }]);
    doc["content"]["operations"] = json!([{
        "id": "tiers__grant",
        "label": "Grant points",
        "inputs": [{ "name": "member", "ty": { "Ref": { "target": "tiers__member", "on-delete": "Restrict" } }, "required": true }],
        "invokers": { "roles": [], "signed-in": true },
        "subject": { "object": "tiers__member", "input": "member" },
        "conditions": [{ "id": "has_name", "expr": "record.name != null", "error": "no-name", "message": "The member has a name." }],
        "steps": [{ "UpdateRecord": { "id": "bump", "object": "tiers__member", "record": "param.member", "fields": { "points": "record.points + 1" } } }]
    }]);
    let bundle = read(&doc).unwrap();
    assert_eq!(bundle.triggers.len(), 1);
    assert_eq!(bundle.automations.len(), 1);
    assert_eq!(bundle.operations.len(), 1);
    let mut world = world();
    let (result, calls) = install(&mut world, &bundle, None, "0.2");
    result.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(calls, 1);
    let schema = schema_of(&world);
    assert!(schema.triggers.as_ref().is_some_and(|t| t.len() == 1));
    assert!(schema.automations.as_ref().is_some_and(|t| t.len() == 1));
    assert!(schema.operations.as_ref().is_some_and(|t| t.len() == 1));
}

// ---- upgrade (INT-1581, INT-1583) --------------------------------------------------------------------

/// Version 1.1.0 of the `tiers` product: a field more, the constraint and the workflow gone, a
/// query more, and the policy condition changed.
fn tiers_1_1() -> Json {
    let mut doc = tiers();
    doc["version"] = json!("1.1.0");
    let content = &mut doc["content"];
    content["objects"]["tiers__member"]["fields"]["tier"] =
        json!({ "id": "tier", "label": "Tier", "ty": "Str", "required": false, "unique": false });
    content["objects"]["tiers__member"]["constraints"] = json!({});
    content.as_object_mut().unwrap().remove("workflows");
    content["queries"]["tiers__all"] =
        json!({ "id": "tiers__all", "object": "tiers__member", "params": [], "order": [] });
    content["policies"]["tiers__editor_members"]["condition"] = json!("record.points >= 1");
    doc
}

fn entity_ids(list: &[strata_substrate::bundle::OwnedEntity]) -> Vec<String> {
    list.iter()
        .map(|e| format!("{}:{}", e.kind, e.id))
        .collect()
}

// Verifies: INT-1581
#[test]
fn an_upgrade_creates_changes_and_removes_what_differs_in_one_schema_change() {
    let mut world = world();
    let old = installed(&mut world, &tiers());
    let newer = read(&tiers_1_1()).unwrap();
    let before_version = schema_of(&world).version.0;

    // The plan names each entity created, changed and removed, and runs nothing.
    let schema = schema_of(&world);
    let tenant = tenant();
    let target = target_of(&tenant, &schema, "0.2");
    let Planned::Change(plan) = plan_upgrade(&newer, &target).unwrap() else {
        panic!("a later version is a change");
    };
    assert_eq!(entity_ids(&plan.created), vec!["query:tiers__all"]);
    assert_eq!(
        entity_ids(&plan.changed),
        vec!["object:tiers__member", "policy:tiers__editor_members"]
    );
    assert_eq!(entity_ids(&plan.removed), vec!["workflow:tiers__promotion"]);

    let (result, calls) = with_target(&mut world, "0.2", |t, e| upgrade_product(&newer, t, e));
    let ProductInstall::Applied {
        schema_version,
        record: Some(record),
    } = result.unwrap_or_else(|e| panic!("{e}"))
    else {
        panic!("the upgrade was not applied");
    };
    assert_eq!(calls, 1, "an upgrade is one schema change");
    assert_eq!(schema_version, before_version + 1);
    assert_eq!(record.version, "1.1.0");
    let schema = schema_of(&world);
    let member = &schema.objects[&object_id("tiers__member")];
    assert!(member.fields.contains_key(&FieldId::new("tier").unwrap()));
    assert!(member.constraints.is_empty());
    assert!(schema.workflows.is_empty());
    assert!(schema
        .queries
        .contains_key(&QueryId::new("tiers__all").unwrap()));
    // The record in the schema is the one the upgrade returned, and the old one is replaced.
    assert_eq!(InstalledRecord::of(&schema, "tiers"), Some(record.clone()));
    assert_ne!(record.owns, old.owns);
    assert!(record.owns.iter().all(|e| e.id != "tiers__promotion"));
}

// Verifies: INT-1583
#[test]
fn an_upgrade_to_a_version_that_is_not_greater_or_of_a_product_that_is_not_installed_is_refused() {
    let mut world = world();
    let bundle = read(&tiers()).unwrap();
    let (result, calls) = with_target(&mut world, "0.2", |t, e| upgrade_product(&bundle, t, e));
    let error = refusal(result);
    assert_eq!(error.kind, BundleErrorKind::NotFound);
    assert_eq!(calls, 0);

    installed(&mut world, &tiers());
    for version in ["1.0.0", "0.9.0"] {
        let mut doc = tiers();
        doc["version"] = json!(version);
        let bundle = read(&doc).unwrap();
        let before = world.hash();
        let (result, calls) = with_target(&mut world, "0.2", |t, e| upgrade_product(&bundle, t, e));
        let error = refusal(result);
        assert_eq!(error.kind, BundleErrorKind::Invalid, "{version}");
        assert!(error.message.contains("not greater"), "{}", error.message);
        assert_eq!(calls, 0);
        assert_eq!(world.hash(), before);
    }
}

/// A record of the `tiers__member` type.
fn a_member(world: &World) -> World {
    let make = Intent::CreateRecord {
        object: object_id("tiers__member"),
        fields: std::collections::BTreeMap::from([
            (FieldId::new("name").unwrap(), Value::Str("Ada".into())),
            (FieldId::new("points").unwrap(), Value::Int(3)),
        ]),
    };
    world
        .step(&make, &context_at(1_800_000_000_001, "record"))
        .expect("a record")
        .0
}

// Verifies: INT-1581
#[test]
fn an_upgrade_that_drops_a_type_with_records_is_refused_and_changes_nothing() {
    let mut world = world();
    installed(&mut world, &tiers());
    world = a_member(&world);
    // Version 2.0.0 declares no object type at all.
    let mut doc = tiers();
    doc["version"] = json!("2.0.0");
    for section in [
        "objects",
        "policies",
        "queries",
        "workflows",
        "role-permissions",
    ] {
        doc["content"].as_object_mut().unwrap().remove(section);
    }
    let newer = read(&doc).unwrap();
    let before = world.hash();
    let (result, _) = with_target(&mut world, "0.2", |t, e| upgrade_product(&newer, t, e));
    let error = step_error(result);
    assert_eq!(error.kind, ErrorKind::MigrationRejected);
    assert!(
        error.message.contains("tiers__member has 1 record(s)"),
        "{}",
        error.message
    );
    assert_eq!(world.hash(), before);
}

// ---- removal (INT-1581, INT-1582, INT-1583) -------------------------------------------------------------

// Verifies: INT-1582
#[test]
fn a_removal_deletes_exactly_what_the_product_owns_and_its_record() {
    let mut world = world_with_member();
    let before = schema_hash(&world);
    installed(&mut world, &tiers());
    assert_ne!(schema_hash(&world), before);
    let (result, calls) = with_target(&mut world, "0.2", |t, e| {
        remove_product("tiers", false, t, e)
    });
    let ProductInstall::Applied { record, .. } = result.unwrap_or_else(|e| panic!("{e}")) else {
        panic!("the removal was not applied");
    };
    assert_eq!(calls, 1);
    assert_eq!(record, None);
    // The schema is the one the tenant had before the product: entities, permissions and record.
    assert_eq!(schema_hash(&world), before);
    assert!(InstalledRecord::of(&schema_of(&world), "tiers").is_none());
}

// Verifies: INT-1583
#[test]
fn removing_a_product_that_is_not_installed_changes_nothing() {
    let mut world = world();
    let version = schema_of(&world).version.0;
    let before = world.hash();
    let (result, calls) = with_target(&mut world, "0.2", |t, e| {
        remove_product("tiers", false, t, e)
    });
    assert_eq!(
        result.unwrap_or_else(|e| panic!("{e}")),
        ProductInstall::Unchanged {
            schema_version: version
        }
    );
    assert_eq!(calls, 0);
    assert_eq!(world.hash(), before);
}

// Verifies: INT-1581
#[test]
fn a_removal_is_refused_while_records_of_its_types_exist_unless_it_deletes_them() {
    let mut world = world();
    installed(&mut world, &tiers());
    world = a_member(&world);
    let before = world.hash();
    let (result, _) = with_target(&mut world, "0.2", |t, e| {
        remove_product("tiers", false, t, e)
    });
    let error = step_error(result);
    assert_eq!(error.kind, ErrorKind::MigrationRejected);
    assert_eq!(world.hash(), before);

    // The plan of the removal that deletes the records lists what is removed.
    let schema = schema_of(&world);
    let tenant = tenant();
    let target = target_of(&tenant, &schema, "0.2");
    let Planned::Change(plan) = plan_remove("tiers", true, &target).unwrap() else {
        panic!("an installed product is removed");
    };
    assert_eq!(plan.removed.len(), 5);

    let (result, calls) = with_target(&mut world, "0.2", |t, e| {
        remove_product("tiers", true, t, e)
    });
    result.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(calls, 1);
    assert!(schema_of(&world).objects.is_empty());
    assert_eq!(
        world.tenants[&tenant]
            .records_of(&object_id("tiers__member"))
            .count(),
        0
    );
}

// Verifies: INT-1582
#[test]
fn a_removal_does_not_touch_what_another_product_owns() {
    let mut world = world();
    installed(&mut world, &tiers());
    let mut other = tiers();
    other["product"] = json!("other");
    other["name"] = json!("Other");
    // The same entities under the namespace of the other product.
    let text = other.to_string().replace("tiers__", "other__");
    let other: Json = serde_json::from_str(&text).unwrap();
    installed(&mut world, &other);
    let (result, _) = with_target(&mut world, "0.2", |t, e| {
        remove_product("tiers", false, t, e)
    });
    result.unwrap_or_else(|e| panic!("{e}"));
    let schema = schema_of(&world);
    assert!(schema.objects.contains_key(&object_id("other__member")));
    assert!(!schema.objects.contains_key(&object_id("tiers__member")));
    assert!(InstalledRecord::of(&schema, "other").is_some());
    assert!(InstalledRecord::of(&schema, "tiers").is_none());
}

// ---- the whole-tenant bundle carries the same sections -----------------------------------------------------

// Verifies: INT-1580
#[test]
fn a_tenant_bundle_with_a_trigger_an_automation_and_an_operation_installs_through_install_into() {
    let doc = json!({
        "id": "acme",
        "name": "Acme",
        "owner": "user:admin",
        "created_at": "2026-01-01T00:00:00Z",
        "objects": [{
            "id": "member", "label": "Member",
            "fields": [
                { "id": "name", "label": "Name", "type": "string", "required": true,
                  "personal": { "kind": "name", "subject": "Creator", "searchable": false } },
                { "id": "points", "label": "Points", "type": "int" }
            ],
            "retention": { "days": 90, "from": "Created" }
        }],
        "automations": [{
            "id": "welcome", "label": "Welcome",
            "run-as": { "roles": [] }, "start": "done",
            "steps": { "done": { "action": "End" } }
        }],
        "triggers": [{
            "id": "on_member", "on": { "RecordCreated": { "object": "member" } },
            "condition": "new.points >= 0", "automation": "welcome"
        }],
        "operations": [{
            "id": "grant", "label": "Grant",
            "inputs": [{ "name": "amount", "ty": "Int", "required": true }],
            "invokers": { "roles": [], "signed-in": true },
            "conditions": [{ "id": "positive", "expr": "param.amount > 0", "error": "not-positive", "message": "Positive." }],
            "steps": []
        }]
    });
    let bundle = Bundle::from_json(&doc.to_string()).unwrap();
    let world = install_into(&World::default(), &bundle).unwrap_or_else(|e| panic!("{e}"));
    let schema = schema_of(&world);
    assert!(schema.triggers.as_ref().is_some_and(|t| t.len() == 1));
    assert!(schema.automations.as_ref().is_some_and(|t| t.len() == 1));
    assert!(schema.operations.as_ref().is_some_and(|t| t.len() == 1));
    let member = &schema.objects[&object_id("member")];
    assert_eq!(member.retention.as_ref().map(|r| r.days), Some(90));
    assert!(member.fields[&FieldId::new("name").unwrap()]
        .personal
        .is_some());
}

/// `tiers` with an aggregate pair on its query and a view section that names one of them, in the
/// shape specc writes (members in the specification's kebab case).
fn tiers_with_aggregates() -> Json {
    let mut doc = tiers();
    doc["content"]["queries"]["tiers__top"]["declared-fields"] = json!(["points", "status"]);
    doc["content"]["queries"]["tiers__top"]["aggregates"] = json!([
        { "id": "total", "kind": "sum", "field": "points", "descending": false, "scan-bound": 500 },
        { "id": "by_status", "kind": "count", "group-by": "status", "descending": false, "scan-bound": 500 }
    ]);
    doc["content"]["views"] = json!({
        "tiers__board": {
            "id": "tiers__board",
            "route": "/board",
            "title": "Board",
            "sections": {
                "total": {
                    "query": "tiers__top", "args": {}, "include": [], "markdown": [],
                    "single": false, "aggregate": "total"
                }
            }
        }
    });
    doc
}

// Verifies: AGG-001
// Verifies: AGG-041
#[test]
fn a_product_query_declares_aggregates_and_a_view_section_names_one() {
    let bundle = read(&tiers_with_aggregates()).unwrap();
    let query = &bundle.queries[&QueryId::new("tiers__top").unwrap()];
    assert_eq!(query.aggregates.as_ref().map(Vec::len), Some(2));
    assert_eq!(query.declared_fields.as_ref().map(Vec::len), Some(2));
    let mut world = world();
    let (result, _) = install(&mut world, &bundle, None, "0.2-draft");
    result.unwrap_or_else(|e| panic!("{e}"));
    let schema = schema_of(&world);
    let installed = &schema.queries[&QueryId::new("tiers__top").unwrap()];
    let aggregates = installed.aggregates.as_ref().unwrap();
    assert_eq!(aggregates[0].kind, AggregateKind::Sum);
    assert_eq!(aggregates[0].scan_bound, 500);
    assert_eq!(
        aggregates[1].group_by,
        Some(FieldId::new("status").unwrap())
    );
    let section = &schema.views[&ViewId::new("tiers__board").unwrap()].sections["total"];
    assert_eq!(section.aggregate, Some(AggregateId::new("total").unwrap()));
}

// Verifies: AGG-002
#[test]
fn a_product_aggregate_that_names_a_field_its_query_does_not_declare_is_refused_at_install() {
    let mut doc = tiers_with_aggregates();
    doc["content"]["queries"]["tiers__top"]["declared-fields"] = json!(["status"]);
    let bundle = read(&doc).unwrap();
    let mut world = world();
    let before = world.hash();
    let (result, _) = install(&mut world, &bundle, None, "0.2-draft");
    let text = format!("{result:?}");
    assert!(
        text.contains("points") && text.contains("is not a declared field"),
        "{text}"
    );
    assert_eq!(world.hash(), before, "nothing is installed");
}

// Verifies: AGG-001
// Verifies: AGG-041
#[test]
fn a_tenant_bundle_declares_aggregates_and_a_view_section_that_names_one() {
    let doc = |declared: Json| {
        json!({
            "id": "scores", "name": "Scores", "owner": "user:admin",
            "created_at": "2026-01-01T00:00:00Z",
            "objects": [{
                "id": "entry", "label": "Entry",
                "fields": [
                    { "id": "player", "label": "Player", "type": "string", "required": true },
                    { "id": "points", "label": "Points", "type": "int", "required": true }
                ]
            }],
            "queries": [{
                "id": "board", "object": "entry",
                "declared_fields": declared,
                "aggregates": [
                    { "id": "leaders", "kind": "top", "field": "points", "n": 3, "descending": true, "scan_bound": 1000 },
                    { "id": "per_player", "kind": "sum", "field": "points", "group_by": "player", "scan_bound": 1000 }
                ]
            }],
            "views": [{
                "id": "leaderboard", "route": "/leaders", "title": "Leaders",
                "sections": { "leaders": { "query": "board", "aggregate": "leaders" } }
            }]
        })
    };
    let bundle = Bundle::from_json(&doc(json!(["player", "points"])).to_string()).unwrap();
    let world = install_into(&World::default(), &bundle).unwrap_or_else(|e| panic!("{e}"));
    let schema = world.tenants[&TenantId::new("scores").unwrap()]
        .schema
        .clone();
    let query = &schema.queries[&QueryId::new("board").unwrap()];
    let aggregates = query.aggregates.as_ref().unwrap();
    assert_eq!(aggregates.len(), 2);
    assert_eq!(aggregates[0].kind, AggregateKind::Top);
    assert_eq!(aggregates[0].n, Some(3));
    assert_eq!(query.declared_fields.as_ref().map(Vec::len), Some(2));
    let section = &schema.views[&ViewId::new("leaderboard").unwrap()].sections["leaders"];
    assert_eq!(
        section.aggregate,
        Some(AggregateId::new("leaders").unwrap())
    );

    // A field the query does not declare is refused with the kernel's problem text.
    let bundle = Bundle::from_json(&doc(json!(["player"])).to_string()).unwrap();
    let error = install_into(&World::default(), &bundle)
        .err()
        .expect("the install is refused");
    let text = error.to_string();
    assert!(
        text.contains("points") && text.contains("is not a declared field"),
        "{text}"
    );
}

/// The ledger sections of the bundle specc writes for `promotions`: its units, its pool and its
/// limit, without the record types and operations that post to them.
fn promotions_ledger() -> Json {
    let mut doc: Json = serde_json::from_str(PROMOTIONS).unwrap();
    let content = doc["content"].as_object_mut().unwrap();
    for section in ["objects", "operations", "roles", "role-permissions"] {
        content.remove(section);
    }
    doc
}

// Verifies: INT-1921
#[test]
fn the_ledger_sections_of_promotions_install_and_the_record_lists_units_pools_and_limits() {
    let mut world = world();
    let record = installed(&mut world, &promotions_ledger());
    let owns = owned(&record);
    let of_kind = |kind: &str| -> Vec<String> {
        owns.iter()
            .filter(|(k, _)| k == kind)
            .map(|(_, id)| id.clone())
            .collect()
    };
    assert_eq!(
        of_kind("ledger-unit"),
        vec!["promotions__redemptions", "promotions__stock"]
    );
    assert_eq!(of_kind("ledger-pool"), vec!["promotions__pool"]);
    assert_eq!(
        of_kind("ledger-limit"),
        vec!["promotions__stock#pool:promotions__pool#"]
    );
    let schema = schema_of(&world);
    let ledger = schema.ledger.as_ref().expect("the ledger is declared");
    assert_eq!(ledger.units.len(), 2);
    assert_eq!(ledger.pools.len(), 1);
    assert_eq!(InstalledRecord::of(&schema, "promotions"), Some(record));
}

// Verifies: CTK-005
#[test]
fn a_removal_deletes_the_templates_and_tokens_the_record_owns() {
    let token =
        json!([{ "name": "tiers__first_name", "expression": "record.name", "fallback": "friend" }]);
    let mut doc = with_notice(PLACES_A_TOKEN, token);
    doc["content"]["mail-templates"] = json!([{
        "template": "tiers__welcome",
        "subject": "Welcome",
        "body": "[{\"type\":\"paragraph\",\"inlines\":[{\"type\":\"text\",\"text\":\"Hello.\"}]}]"
    }]);
    let mut world = world();
    let record = installed(&mut world, &doc);
    let owns = owned(&record);
    for (kind, id) in [
        ("token", "tiers__first_name"),
        ("notice-template", "tiers__promoted"),
        ("mail-template", "tiers__welcome"),
    ] {
        assert!(
            owns.contains(&(kind.to_string(), id.to_string())),
            "{kind} {id}: {owns:?}"
        );
    }
    let schema = schema_of(&world);
    assert_eq!(strata_kernel::templates::tokens(&schema).len(), 1);
    assert_eq!(strata_kernel::templates::list(&schema).len(), 2);

    let (result, calls) = with_target(&mut world, "0.2", |t, e| {
        remove_product("tiers", false, t, e)
    });
    result.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(calls, 1);
    let schema = schema_of(&world);
    assert!(strata_kernel::templates::tokens(&schema).is_empty());
    assert!(strata_kernel::templates::list(&schema).is_empty());
    assert!(InstalledRecord::of(&schema, "tiers").is_none());
}

// ---- the blocks of a template (D-7900) --------------------------------------------------------

/// A product with one notice template whose body is `body`, and the tokens it defines.
fn with_notice(body: &str, tokens: Json) -> Json {
    let mut doc = tiers();
    doc["content"]["notice-templates"] = json!([{
        "template": "tiers__promoted",
        "title": "Promoted",
        "body": body
    }]);
    if !tokens.is_null() {
        doc["content"]["tokens"] = tokens;
    }
    doc
}

const PLACES_A_TOKEN: &str =
    r#"[{"type":"paragraph","inlines":[{"type":"token","name":"tiers__first_name"}]}]"#;

// Verifies: CTK-005
#[test]
fn a_bundle_whose_template_places_a_token_nobody_defines_is_refused_at_install() {
    let mut world = world();
    let bundle = read(&with_notice(PLACES_A_TOKEN, Json::Null)).unwrap();
    let (result, calls) = install(&mut world, &bundle, None, "0.2");
    let error = refusal(result);
    assert_eq!(error.kind, BundleErrorKind::InvalidBundle);
    assert_eq!(
        error.message,
        "the notice template \"tiers__promoted\" places the token \"tiers__first_name\", which neither the bundle nor the tenant defines"
    );
    assert_eq!(calls, 0, "nothing was asked of the kernel");
    // The bundle that defines the token installs, and the kernel holds both.
    let token =
        json!([{ "name": "tiers__first_name", "expression": "record.name", "fallback": "friend" }]);
    let bundle = read(&with_notice(PLACES_A_TOKEN, token)).unwrap();
    let (result, _) = install(&mut world, &bundle, None, "0.2");
    result.unwrap_or_else(|e| panic!("{e:?}"));
    let schema = schema_of(&world);
    assert_eq!(strata_kernel::templates::tokens(&schema).len(), 1);
    assert_eq!(strata_kernel::templates::list(&schema).len(), 1);
}

// Verifies: EML-012
// Verifies: CMS-250
#[test]
fn a_bundle_whose_template_body_is_not_blocks_is_refused_at_install() {
    for (body, want) in [
        (
            r#"[{"type":"paragraph","text":"Hello"}]"#,
            "the body at [0]: has the member \"text\"",
        ),
        (
            r#"[{"type":"paragraph","inlines":[{"type":"link","href":"javascript:alert(1)","children":[]}]}]"#,
            "a link carries a web (http, https) or mail (mailto) address",
        ),
        ("not json", "the body is not JSON text"),
    ] {
        let mut world = world();
        let bundle = read(&with_notice(body, Json::Null)).unwrap();
        let error = refusal(install(&mut world, &bundle, None, "0.2").0);
        assert_eq!(error.kind, BundleErrorKind::InvalidBundle);
        assert!(error.message.contains(want), "{body}: {}", error.message);
    }
}

// Verifies: CMS-004
#[test]
fn what_the_markdown_parser_produces_is_a_valid_template_body() {
    let source = "# Title\n\nSome *emphasis*, **strong**, `code` and a [link](https://example.test/a) \
and a [bad one](javascript:alert(1)).\n\n> quoted\n> text\n\n- one\n- two\n\n1. first\n2. second\n\n\
```rust\nlet a = 1;\n```\n\n![A badge](/media/badge.png \"Silver\")\n\n---\n";
    let blocks = strata_substrate::markdown::parse(source);
    assert!(blocks.len() >= 7);
    let json = serde_json::to_string(&blocks).unwrap();
    let analysis = strata_kernel::blocks::analyse(&json);
    assert!(analysis.problems.is_empty(), "{:?}", analysis.problems);
    // The bounds are the parser's.
    assert_eq!(
        strata_kernel::blocks::MAX_QUOTE_DEPTH,
        strata_substrate::markdown::MAX_QUOTE_DEPTH
    );
    assert_eq!(
        strata_kernel::blocks::MAX_INLINE_DEPTH,
        strata_substrate::markdown::MAX_INLINE_DEPTH
    );
    // A link the parser keeps is a link the schema allows, and the other way round.
    for href in [
        "https://a.test",
        "http://a.test",
        "mailto:a@b.test",
        "/path",
        "#top",
        "//host",
        "javascript:x",
        "data:text/html,x",
        "ftp://a.test",
    ] {
        let parsed = strata_substrate::markdown::parse(&format!("[t]({href})"));
        let kept = serde_json::to_string(&parsed).unwrap().contains("\"link\"");
        assert_eq!(kept, strata_kernel::blocks::allowed_href(href), "{href}");
    }
}
