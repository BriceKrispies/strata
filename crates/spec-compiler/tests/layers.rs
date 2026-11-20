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
//! - The typed sections (objects, policies, segments, settings, ledger units, pools and limits,
//!   mail and notice templates, tokens, triggers, automations, operations) hold values of platform
//!   types; `badge-milestones.jsonc` and `promotions.jsonc` are complete products (they declare
//!   everything they name, in their own namespace) that specc and Exegra both accept.
//!
//! The Exegra checks need the Exegra CLI (see `support`).

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use strata_spec_compiler::layer_expr;
use strata_spec_compiler::layer_values::{self, Types};
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
            "badges__tier",
            "badges__curator",
            "badges__installed-as-metadata",
            "badges__grant"
        ]
    );
}

/// The redeem-a-promotion product (WG-5032): two object types, the ledger units, pool and floor of
/// its stock and two tenant operations. specc's loader accepts it beside the platform, and so does
/// Exegra (`exegra_accepts_the_complete_products`).
#[test]
fn the_promotions_product_is_accepted_beside_the_platform() {
    let layered = match load_with(&[product("promotions.jsonc")]) {
        Ok(layered) => layered,
        Err(diagnostics) => panic!(
            "the promotions product was refused:\n{}",
            diagnostics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        ),
    };
    let promotions = &layered.products[0];
    assert_eq!(promotions.id, "promotions");
    let ids: Vec<(&str, &str)> = promotions
        .entities
        .iter()
        .map(|e| (e.section.as_str(), e.id.as_str()))
        .collect();
    assert_eq!(
        ids,
        vec![
            ("roles", "promotions__manager"),
            ("claims", "promotions__redeem-is-one-operation"),
            ("objects", "promotions__promotion"),
            ("objects", "promotions__redemption"),
            ("ledgerUnits", "promotions__stock"),
            ("ledgerUnits", "promotions__redemptions"),
            ("ledgerPools", "promotions__pool"),
            ("ledgerLimits", "promotions__stock_floor"),
            ("operations", "promotions__create"),
            ("operations", "promotions__redeem"),
        ]
    );
}

/// Every expression of the promotions product is tenant rule language the kernel parses: the
/// window through the time arithmetic of D-6541 and the stock per promotion through text (D-6542,
/// WG-5064). Constraints are also checked against the promotion's fields.
#[test]
fn the_promotions_expressions_parse_and_the_window_checks() {
    use strata_kernel::expr as kexpr;
    use strata_kernel::testkit::field_def;
    use strata_kernel::{FieldType, RoleId};

    let doc = strata_spec_compiler::jsonc::parse(&product("promotions.jsonc").text).unwrap();
    let mut sources: Vec<String> = Vec::new();
    for o in doc["objects"].as_array().unwrap() {
        for c in o["constraints"].as_object().unwrap().values() {
            sources.push(c["expr"].as_str().unwrap().to_string());
        }
    }
    for op in doc["operations"].as_array().unwrap() {
        for c in op["conditions"].as_array().unwrap() {
            sources.push(c["expr"].as_str().unwrap().to_string());
        }
        for step in op["steps"].as_array().unwrap() {
            for body in step.as_object().unwrap().values() {
                for key in ["amount", "scope", "record"] {
                    if let Some(s) = body[key].as_str() {
                        sources.push(s.to_string());
                    }
                }
                if let Some(fields) = body["fields"].as_object() {
                    sources.extend(fields.values().filter_map(|v| v.as_str()).map(String::from));
                }
            }
        }
    }
    assert!(sources.iter().any(|s| s.contains("days(90)")));
    assert!(sources.iter().any(|s| s == "text(record.$id)"));
    for source in &sources {
        let parsed = kexpr::parse(source).unwrap_or_else(|e| panic!("{source}: {e}"));
        assert_eq!(
            kexpr::parse(&parsed.to_string()).unwrap(),
            parsed,
            "{source}"
        );
    }

    // The window constraint at the promotion: a boolean over its time fields.
    let fields: std::collections::BTreeMap<_, _> = [
        field_def("title", FieldType::Str, true),
        field_def("starts_at", FieldType::Time, true),
        field_def("ends_at", FieldType::Time, true),
    ]
    .into_iter()
    .map(|d| (d.id.clone(), d))
    .collect();
    let (params, roles, objects) = (
        std::collections::BTreeSet::new(),
        std::collections::BTreeSet::<RoleId>::new(),
        std::collections::BTreeMap::new(),
    );
    let env = kexpr::TypeEnv {
        fields: &fields,
        params: &params,
        roles: &roles,
        objects: &objects,
    };
    let window = doc["objects"][0]["constraints"]["window"]["expr"]
        .as_str()
        .unwrap();
    assert_eq!(
        kexpr::check(&kexpr::parse(window).unwrap(), &env),
        Ok(kexpr::Shape::Bool)
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

// Verifies: INT-1522
#[test]
fn an_automation_product_is_accepted_beside_the_platform() {
    let layered = match load_with(&[product("badge-milestones.jsonc")]) {
        Ok(layered) => layered,
        Err(diagnostics) => panic!(
            "the automation product was refused:\n{}",
            diagnostics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        ),
    };
    let milestones = &layered.products[0];
    assert_eq!(milestones.id, "badge-milestones");
    let declared: Vec<(&str, &str)> = milestones
        .entities
        .iter()
        .map(|e| (e.section.as_str(), e.id.as_str()))
        .collect();
    for (section, id) in [
        ("objects", "badge_milestones__award"),
        ("ledgerUnits", "badge_milestones__points"),
        ("mailTemplates", "badge_milestones__silver_mail"),
        ("mailTemplates", "badge_milestones__silver_follow_up"),
        ("noticeTemplates", "badge_milestones__silver_notice"),
        ("triggers", "badge_milestones__silver_on_points"),
        ("automations", "badge_milestones__silver"),
    ] {
        assert!(declared.contains(&(section, id)), "{declared:?}");
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

#[test]
fn a_value_that_does_not_fit_its_platform_type_says_what_is_missing() {
    let diagnostics = load_with(&[product("refused/value-invalid.jsonc")]).unwrap_err();
    let messages: Vec<&str> = diagnostics
        .iter()
        .filter(|d| d.code == layers::VALUE_INVALID)
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(messages.len(), 2, "{diagnostics:?}");
    assert!(messages
        .iter()
        .any(|m| m.contains("\"colour\" is not a field of its type")));
    assert!(messages
        .iter()
        .any(|m| m.contains("\"fields.name\" lacks the required field \"ty\"")));
}

#[test]
fn an_expression_that_does_not_parse_or_check_in_its_scope_is_refused_with_one_rule_per_kind() {
    let found = |name: &str, code: &str| -> Vec<String> {
        load_with(&[product(&format!("refused/{name}.jsonc"))])
            .unwrap_err()
            .into_iter()
            .filter(|d| d.code == code)
            .map(|d| d.message)
            .collect()
    };
    let syntax = found("expression-syntax", layer_expr::EXPR_SYNTAX);
    assert_eq!(syntax.len(), 2, "{syntax:?}");
    assert!(
        syntax
            .iter()
            .any(|m| m.contains("\"member_tiers__on_post\"")
                && m.contains("at \"condition\", `balance(param.owner) > 1`")
                && m.contains("position 0: unknown name \"balance\"")),
        "a trigger condition that calls what the language lacks: {syntax:?}"
    );
    assert!(
        syntax.iter().any(|m| m.contains("\"member_tiers__grant\"")
            && m.contains("at \"conditions.0.expr\"")
            && m.contains("position 14")),
        "an operation condition with a double-quoted string: {syntax:?}"
    );
    let field = found("expression-unknown-field", layer_expr::EXPR_UNKNOWN_FIELD);
    assert_eq!(field.len(), 2, "{field:?}");
    assert!(field
        .iter()
        .any(|m| m
            .contains("unknown field colour; the object type has the fields name, owner, points")));
    assert!(
        field
            .iter()
            .any(|m| m.contains("unknown field phone; the object type has the fields email")),
        "a hop to a field of the target object type: {field:?}"
    );
    let types: Vec<String> = found("expression-type", layer_expr::EXPR_TYPE)
        .iter()
        .filter_map(|m| {
            let rest = m.split("position ").nth(1)?;
            let problem = rest.split_once(": ")?.1;
            Some(problem.split(". An expression").next()?.to_string())
        })
        .collect();
    assert_eq!(
        types,
        [
            "cannot compare Str with Int",
            "len does not accept a Int operand",
            "name is not a single reference"
        ]
    );
}

#[test]
fn a_personal_field_without_retention_is_refused_and_the_complete_products_carry_it() {
    let diagnostics = load_with(&[product("refused/retention-missing.jsonc")]).unwrap_err();
    assert_eq!(
        diagnostics.len(),
        1,
        "the type without a personal field needs no retention: {diagnostics:?}"
    );
    assert_eq!(diagnostics[0].code, layers::RETENTION_MISSING);
    assert!(load_with(&[
        product("badge-milestones.jsonc"),
        product("promotions.jsonc")
    ])
    .is_ok());
}

#[test]
fn a_name_in_a_typed_section_must_be_declared_by_the_product() {
    let diagnostics = load_with(&[product("refused/reference-unknown-object.jsonc")]).unwrap_err();
    let unknown: Vec<&str> = diagnostics
        .iter()
        .filter(|d| d.code == layers::REFERENCE_UNKNOWN)
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(unknown.len(), 2, "{diagnostics:?}");
    assert!(unknown
        .iter()
        .any(|m| m.contains("\"member_tiers__ghost\" at on.RecordCreated.object")));
    assert!(unknown
        .iter()
        .any(|m| m.contains("\"member_tiers__nothing\" at automation")));
}

#[test]
fn a_capability_is_confined_to_the_product_that_declares_it() {
    let diagnostics = load_with(&[
        product("badges.jsonc"),
        product("refused/capability-other-product.jsonc"),
    ])
    .unwrap_err();
    let found = diagnostics
        .iter()
        .find(|d| d.code == layers::CROSS_PRODUCT_REFERENCE)
        .unwrap_or_else(|| panic!("no cross-product reference in {diagnostics:?}"));
    assert!(
        found.message.contains("\"badges__grant\""),
        "{}",
        found.message
    );
    assert!(
        found.message.contains("badges.jsonc declares"),
        "{}",
        found.message
    );
    // Alone, the name is one that nothing declares.
    let alone = load_with(&[product("refused/capability-other-product.jsonc")]).unwrap_err();
    assert!(alone.iter().any(|d| d.code == layers::REFERENCE_UNKNOWN));
}

/// The table of typed sections names platform types that exist, are structs, and are listed with
/// the other sections; the reference types too (so a renamed platform type is noticed here).
#[test]
fn the_typed_sections_name_platform_types() {
    let text = platform_text();
    let platform: Value = layers_json(&text);
    let types = Types::of(&platform);
    let sections: Vec<&str> = layers::product_sections().iter().map(|(s, _)| *s).collect();
    for typed in layer_values::TYPED_SECTIONS {
        assert!(
            sections.contains(&typed.section),
            "{} is not listed",
            typed.section
        );
        let ty = types.get(typed.ty).unwrap_or_else(|| {
            panic!(
                "{} names the platform type {}, which is not declared",
                typed.section, typed.ty
            )
        });
        assert_eq!(ty["kind"], "struct", "{} is not a struct", typed.ty);
        if let Some(field) = typed.id_field {
            assert!(
                ty["fields"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|f| f["name"] == field),
                "{} has no member {field}",
                typed.ty
            );
        }
    }
    for (ty, sections_of) in layer_values::REFERENCE_TYPES {
        assert!(
            types.get(ty).is_some(),
            "the reference type {ty} is not declared"
        );
        for s in *sections_of {
            assert!(
                layer_values::typed_section(s).is_some() || ["roles"].contains(s),
                "{s}"
            );
        }
    }
}

/// The platform specification as JSON, comments removed by specc's own parser.
fn layers_json(text: &str) -> Value {
    strata_spec_compiler::jsonc::parse(text).expect("the platform parses")
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
        "roles": [ { "id": "badges__hook", "name": "Hook", "level": "tenant", "holders": ["external"], "capabilities": [] } ] }"#;
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
        "\"id\": \"content-read\", \"contract\": \"badges__tier\",",
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
    assert!(found.message.contains("\"badges__tier\""));
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

/// Exegra's validation of the platform alone, run once for every test that compares a layered
/// project with it (the platform's analysis is the largest cost of a validation).
fn platform_validation(cli: &Path) -> &'static Value {
    static ALONE: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    ALONE.get_or_init(|| {
        let specs = exegra_specs("exegra-platform", &[]);
        exegra_validate(cli, &specs, "platform")
    })
}

#[test]
fn exegra_accepts_the_product_and_reports_the_platform_hash_unchanged() {
    let Some(cli) = support::exegra_cli("the layered validation") else {
        return;
    };
    let specs = exegra_specs("exegra-accepted", &["badges.jsonc"]);
    let alone = platform_validation(&cli);
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
fn exegra_accepts_the_complete_products_and_reports_the_platform_hash_unchanged() {
    let Some(cli) = support::exegra_cli("the complete products") else {
        return;
    };
    let specs = exegra_specs(
        "exegra-complete",
        &["badge-milestones.jsonc", "promotions.jsonc"],
    );
    let alone = platform_validation(&cli);
    let layered = exegra_validate(&cli, &specs, "layered");
    assert_eq!(layered["ok"], Value::Bool(true), "{layered}");
    assert_eq!(layered["result"]["errors"], Value::from(0), "{layered}");
    assert_eq!(layered["semanticHash"], alone["semanticHash"]);
    let products: Vec<&str> = layered["result"]["products"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["product"].as_str())
        .collect();
    assert_eq!(products, vec!["badge-milestones", "promotions"]);
}

#[test]
fn exegra_refuses_a_capability_of_another_product() {
    let Some(cli) = support::exegra_cli("the capability confinement") else {
        return;
    };
    let specs = exegra_specs(
        "exegra-capability",
        &["badges.jsonc", "refused/capability-other-product.jsonc"],
    );
    let out = exegra_validate(&cli, &specs, "layered");
    assert_eq!(out["ok"], Value::Bool(false), "{out}");
    let codes: Vec<&str> = out["result"]["diagnostics"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| d["code"].as_str())
        .collect();
    assert!(
        codes.contains(&layers::CROSS_PRODUCT_REFERENCE),
        "{codes:?}"
    );
}

/// Exegra reads the same table of typed sections (`src/core/layer-values.js`): the sections, the
/// platform type of each and the member that names an item.
#[test]
fn exegra_has_the_same_typed_sections() {
    let Some(cli) = support::exegra_cli("the typed section comparison") else {
        return;
    };
    let module = cli
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("src/core/layer-values.js");
    let url = format!("file:///{}", module.to_string_lossy().replace('\\', "/"));
    let output = Command::new("node")
        .args([
            "--input-type=module",
            "-e",
            &format!(
                "import('{url}').then((m) => console.log(JSON.stringify(m.TYPED_SECTIONS.map((s) => [s.section, s.type, s.idField ?? null]))))"
            ),
        ])
        .output()
        .expect("node runs");
    let theirs: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stderr)));
    let ours: Vec<Value> = layer_values::TYPED_SECTIONS
        .iter()
        .map(|s| serde_json::json!([s.section, s.ty, s.id_field]))
        .collect();
    assert_eq!(theirs, Value::Array(ours));
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
