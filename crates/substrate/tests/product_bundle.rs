#![allow(clippy::err_expect)] // `unwrap_err` would print a whole world when it fails.
//! The bundle of a product (`tenant-bundle`, D-6580): it is read, the sections the kernel can take
//! are installed into a tenant that exists, the sections it cannot take are refused by name, and
//! the install is idempotent and records which product owns each entity (D-6582, D-6583).

use serde_json::{json, Value as Json};
use strata_kernel::expr::parse;
use strata_kernel::*;
use strata_substrate::bundle::{
    install_into, install_product, Bundle, BundleError, BundleErrorKind, InstalledRecord,
    PlatformRange, ProductBundle, ProductInstall, ProductInstallError, ProductTarget,
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
fn a_view_is_read_with_its_sections() {
    let mut doc = tiers();
    doc["content"]["views"] = json!({
        "tiers__index": {
            "id": "tiers__index",
            "route": "/tiers",
            "title": "Tiers",
            "sections": {
                "top": { "query": "tiers__top", "args": {}, "include": [], "markdown": [], "single": false }
            }
        }
    });
    let bundle = read(&doc).unwrap();
    assert_eq!(bundle.views.len(), 1);
    assert_eq!(bundle.entities().last().unwrap().kind, "view");
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
fn a_platform_range_runs_from_the_first_release_up_to_the_first_it_excludes() {
    let range = PlatformRange {
        min: "0.2".to_string(),
        below: Some("0.4".to_string()),
    };
    assert!(range.admits("0.2"));
    assert!(range.admits("0.2-draft"));
    assert!(range.admits("0.3.7"));
    assert!(!range.admits("0.4"));
    assert!(!range.admits("0.1.9"));
    assert!(!range.admits("1.0"));
    let open = PlatformRange {
        min: "0.2".to_string(),
        below: None,
    };
    assert!(open.admits("12.0"));
    assert!(!open.admits("0.1"));
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
    install(&mut world, &bundle, None, "0.2").0.unwrap();
    // Without the catalogue record the entities that exist belong to nobody that installs here.
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

// Verifies: INT-155
#[test]
fn a_section_the_kernel_cannot_take_yet_is_refused_by_name_and_never_dropped() {
    let mut doc = tiers();
    doc["content"]["segments"] = json!({ "tiers__gold": { "name": "Gold", "subject-type": "tiers__member", "predicate": "record.points >= 100" } });
    doc["content"]["triggers"] = json!([{ "id": "tiers__on_points" }, { "id": "tiers__on_pause" }]);
    doc["content"]["settings"] = json!([]);
    doc["content"]["objects"]["tiers__member"]["retention"] =
        json!({ "days": 30, "from": "Created" });
    let bundle = read(&doc).unwrap();
    // An empty section is not a filled one.
    let names: Vec<(&str, usize)> = bundle
        .unbuilt
        .iter()
        .map(|u| (u.section.as_str(), u.entries))
        .collect();
    assert_eq!(
        names,
        vec![("objects.retention", 1), ("segments", 1), ("triggers", 2)]
    );
    let mut world = world();
    let before = world.hash();
    let (result, calls) = install(&mut world, &bundle, None, "0.2");
    let error = refusal(result);
    assert_eq!(error.kind, BundleErrorKind::InvalidBundle);
    for section in ["\"segments\"", "\"triggers\"", "\"objects.retention\""] {
        assert!(error.message.contains(section), "{}", error.message);
    }
    assert!(!error.message.contains("\"settings\""), "{}", error.message);
    assert_eq!(calls, 0, "nothing of a refused bundle is installed");
    assert_eq!(world.hash(), before);
}

/// Every section of `bundle-sections` that the kernel has no intent for is refused when filled,
/// and each by its own name.
#[test]
fn each_section_without_a_kernel_intent_is_refused_naming_it() {
    let filled = [
        (
            "capabilities",
            json!([{ "id": "tiers__promote", "name": "Promote", "description": "d" }]),
        ),
        ("segments", json!({ "tiers__gold": {} })),
        ("settings", json!([{ "id": "tiers__limit" }])),
        (
            "ledger-units",
            json!([{ "unit": "tiers__points", "description": "d" }]),
        ),
        (
            "ledger-pools",
            json!([{ "pool": "tiers__pool", "description": "d" }]),
        ),
        ("ledger-limits", json!([{ "unit": "tiers__points" }])),
        ("mail-templates", json!([{ "template": "tiers__mail" }])),
        ("notice-templates", json!([{ "template": "tiers__notice" }])),
        ("tokens", json!([{ "name": "tiers__token" }])),
        ("triggers", json!([{ "id": "tiers__trigger" }])),
        ("automations", json!([{ "id": "tiers__automation" }])),
        ("operations", json!([{ "id": "tiers__operation" }])),
    ];
    for (section, value) in filled {
        let mut doc = tiers();
        doc["content"][section] = value;
        let bundle = read(&doc).unwrap();
        let mut world = world();
        let (result, calls) = install(&mut world, &bundle, None, "0.2");
        let error = refusal(result);
        assert_eq!(error.kind, BundleErrorKind::InvalidBundle, "{section}");
        assert!(
            error.message.contains(&format!("\"{section}\"")),
            "{section}: {}",
            error.message
        );
        assert_eq!(calls, 0, "{section}");
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
    let sections: Vec<&str> = tiers.unbuilt.iter().map(|u| u.section.as_str()).collect();
    assert_eq!(
        sections,
        vec!["capabilities", "ledger-units", "mail-templates"]
    );

    let badges = ProductBundle::from_json(BADGE_MILESTONES).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(badges.objects.len(), 1);
    assert_eq!(badges.policies.len(), 2);
    let sections: Vec<&str> = badges.unbuilt.iter().map(|u| u.section.as_str()).collect();
    assert_eq!(
        sections,
        vec![
            "automations",
            "ledger-units",
            "mail-templates",
            "notice-templates",
            "objects.retention",
            "triggers"
        ]
    );
    let error = badges.check_installable().unwrap_err();
    assert_eq!(error.kind, BundleErrorKind::InvalidBundle);

    let promotions = ProductBundle::from_json(PROMOTIONS).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(promotions.objects.len(), 2);
    assert!(promotions.unbuilt.iter().any(|u| u.section == "operations"));
}
