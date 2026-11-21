//! `specc bundle`: a product file turned into a tenant bundle (D-6580, D-6700).
//!
//! A tenant installs a product as a bundle (the platform type `tenant-bundle`, installed by the
//! contract `install-bundle`). Nothing but this module turns the product's own file into one. The
//! product file is checked first, with the checks of [`crate::layers`] (every typed value against
//! its platform type, every id in the namespace of the product, every name resolved), and then
//! written as one deterministic JSON file, `<product id>.bundle.json`:
//!
//! ```text
//! { "content": { "automations": [...], "objects": { "<id>": {...} }, ... },
//!   "name": "<product.name>", "platform": { "min": "...", "below": "..." },
//!   "product": "<product.id>", "version": "<product.version>" }
//! ```
//!
//! - The header of the product file carries the two facts a bundle needs: `product.version`
//!   (`MAJOR.MINOR.PATCH`, required by the bundle) and `product.platform` (`min`, and optionally
//!   `below`: the platform releases the product is written for, D-6583). The layer checks take both
//!   and validate them (`layer.header-invalid`, [`crate::layers::platform_problems`]); a header
//!   with neither is a product file that has no version, which a bundle refuses.
//! - `content` is a value of `bundle-sections`, filled from the typed sections of the file
//!   ([`crate::layer_values::TYPED_SECTIONS`]) and from its `roles` and `capabilities`. Ids are
//!   already in the namespace of the product (`layer.namespace-missing` refuses any that is not),
//!   so they are copied as they are; the bundle states no namespace of its own, which is the
//!   product id with each hyphen replaced by an underscore and two underscores after it (D-6582).
//! - Sections appear in the alphabetical order of their names and every object has its keys sorted.
//!   A list section holds its entries in the alphabetical order of their ids, so the same product
//!   gives the same bytes whatever the order of its file.
//! - A role of the product file becomes an entry of `roles` (its id and its name as the label) and
//!   an entry of `role-permissions` (each capability it lists as `{"Capability": "<name>"}`); the
//!   attributes that only describe a role of the platform specification (description, level,
//!   holders) are not tenant data. A typed section that has an id the type does not name (a ledger
//!   unit's `unit`, a template's `template`, a token's `name`) takes it from the item's id; one
//!   whose type has no name (a segment, a ledger limit) loses the id, which is only the product's
//!   name for it (the key of `segments`).
//! - The screens, forms, actions, live bindings and error messages of the file are its interface:
//!   each becomes an interface element (`ui-element`) of the section `ui`, bound to the product's
//!   views and operations by [`crate::bundle_ui`], which refuses what it cannot bind (D-8250,
//!   INT-3250, INT-3251). The five sections share one id space.
//! - The claims, scenarios and journeys of the file (`claims`, `scenarios`, `uiScenarios`,
//!   `evidence`) are statements about the product, not tenant metadata, and are not in the bundle.
//! - The finished bundle is checked against `tenant-bundle` as a typed section of its own, so a
//!   bundle that does not have the platform type is never written.
//!
//! Refused, each with a rule code and a message that says what to do:
//!
//! - `bundle.version-missing`: the header has no `version`. (A `version` that is not three numbers
//!   joined by dots and a `platform` that is not a range `{ "min": "<release>", "below": "<release>" }`
//!   are refused by the layer checks, `layer.header-invalid`, which run first.)
//! - `bundle.section-no-place`: a section of the file that `bundle-sections` has no place for;
//! - `bundle.id-collision`: two entities of the product that would be one entity of the bundle;
//! - `bundle.ui-unresolved`, `bundle.ui-unsupported`: an interface element that names what the
//!   product does not declare, or uses what the interpreter does not draw ([`crate::bundle_ui`]).
//!
//! Anything the layer checks refuse (`layer.*`) is refused first and unchanged.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Map, Value};

use crate::bundle_ui;
use crate::diagnostic::Diagnostic;
use crate::emit::GeneratedFile;
use crate::jsonc;
use crate::layer_values::{self, TypedSection, Types};
use crate::layers::{self, ProductFile, PRODUCT_HEADER};

/// The header has no `version`.
pub const VERSION_MISSING: &str = "bundle.version-missing";
/// A section of the product file that a tenant bundle has no place for.
pub const SECTION_NO_PLACE: &str = "bundle.section-no-place";
/// Two entities of the product that would be one entity of the bundle.
pub const ID_COLLISION: &str = "bundle.id-collision";

/// The header keys that carry the release facts of a bundle.
const HEADER_VERSION: &str = "version";
const HEADER_PLATFORM: &str = "platform";

/// The sections of a product file that state facts about the product and are not tenant metadata.
pub const DOCUMENTATION_SECTIONS: &[&str] = &["claims", "scenarios", "uiScenarios", "evidence"];

/// The sections of `bundle-sections` that are maps from an id (the others are lists).
const MAP_SECTIONS: &[&str] = &[
    "objects",
    "roles",
    "role-permissions",
    "policies",
    "queries",
    "views",
    "workflows",
    "segments",
    "ui",
];

/// Where a section of a product file lands in a tenant bundle.
struct Place {
    /// The section of `bundle-sections`.
    section: &'static str,
    /// The id space in which two entities must not share an id: every kind of entity has its own,
    /// except mail and notice templates (one `template-id`) and automations (the ids of workflows).
    space: &'static str,
}

/// The place of a section of a product file, when `bundle-sections` has one.
fn place(section: &str) -> Option<Place> {
    let (bundle, space) = match section {
        "objects" => ("objects", "object"),
        "roles" => ("roles", "role"),
        "capabilities" => ("capabilities", "capability"),
        "policies" => ("policies", "policy"),
        "queries" => ("queries", "query"),
        "views" => ("views", "view"),
        // A field workflow and an automation share the ids of workflows (D-6521).
        "workflows" => ("workflows", "workflow"),
        "segments" => ("segments", "segment"),
        "settings" => ("settings", "setting"),
        "ledgerUnits" => ("ledger-units", "ledger-unit"),
        "ledgerPools" => ("ledger-pools", "ledger-pool"),
        "ledgerLimits" => ("ledger-limits", "ledger-limit"),
        "mailTemplates" => ("mail-templates", "template"),
        "noticeTemplates" => ("notice-templates", "template"),
        "tokens" => ("tokens", "token"),
        "triggers" => ("triggers", "trigger"),
        "automations" => ("automations", "workflow"),
        "operations" => ("operations", "operation"),
        "eventKinds" => ("event-kinds", "event-kind"),
        // The interface entities are one kind of tenant entity, the interface element (D-8250).
        "screens" | "forms" | "actions" | "liveBindings" | "errorMessages" => ("ui", "ui-element"),
        _ => return None,
    };
    Some(Place {
        section: bundle,
        space,
    })
}

/// The section of `bundle-sections` that a section of a product file lands in, when it has one.
pub fn bundle_section(section: &str) -> Option<&'static str> {
    place(section).map(|p| p.section)
}

/// The sections of a product file that have a place in a bundle, in the order of the messages.
pub fn placed_sections() -> Vec<&'static str> {
    let mut sections = vec!["roles", "capabilities"];
    sections.extend(layer_values::TYPED_SECTIONS.iter().map(|s| s.section));
    sections
}

/// What to do about a section that has no place.
fn no_place_hint(section: &str) -> &'static str {
    match section {
        "records" => "Declare the record type as an object type in \"objects\" (a value of object-type-def).",
        "types" => "A tenant has no value types of its own: the types of a field are the kernel's field types, written in \"objects\".",
        "behaviors" => "A bundle holds the states of a field as values of workflow-def: declare them in \"workflows\"; declare a sequence of steps in \"automations\".",
        "events" => "The events of the platform are the platform's: a trigger names one of them in \"triggers\"; remove the event.",
        "contracts" => "A product operation is a composite operation: declare it in \"operations\" (a value of tenant-operation-def).",
        _ => "Remove it, or declare what it stands for in one of the sections that have a place.",
    }
}

/// The version and the platform range of a product header. The layer checks have accepted both
/// (`layer.header-invalid` refuses a malformed one), so what is left to refuse here is a header with
/// no version, which a bundle cannot be written without (D-6583).
fn read_release(
    file: &str,
    header: &Map<String, Value>,
    out: &mut Vec<Diagnostic>,
) -> Option<(String, Option<Value>)> {
    match header.get(HEADER_VERSION) {
        Some(Value::String(version)) if layers::is_version(version) => {
            Some((version.clone(), header.get(HEADER_PLATFORM).cloned()))
        }
        // A malformed version is the layer checks' to refuse, with their own rule.
        Some(_) => None,
        None => {
            out.push(Diagnostic::new(
                VERSION_MISSING,
                file,
                format!("The product header of {file} has no \"version\". A bundle carries the version of its product (D-6583), so that installing it again changes nothing and an upgrade is offered only to a greater version; add \"version\": \"1.0.0\" (or the version of this release) to \"product\"."),
            ));
            None
        }
    }
}

/// A bundle built from a product file.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    /// The value of `tenant-bundle`.
    pub bundle: Value,
    /// The sections of the file that are not tenant metadata and are not in the bundle, with the
    /// number of items each held.
    pub omitted: Vec<(String, usize)>,
}

/// What `specc bundle` writes.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    /// The id of the product.
    pub product: String,
    /// The bundle file.
    pub files: Vec<GeneratedFile>,
    /// See [`Built::omitted`].
    pub omitted: Vec<(String, usize)>,
}

/// `value` with the keys of every object in sorted order.
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut sorted = Map::new();
            for key in keys {
                sorted.insert(key.clone(), canonical(&map[key]));
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

/// The entry of a typed section: the item with the member that names it as its type declares it.
fn typed_entry(typed: &TypedSection, id: &str, item: &Map<String, Value>) -> Value {
    let mut value = item.clone();
    match typed.id_field {
        Some("id") => {}
        Some(field) => {
            value.remove("id");
            value.insert(field.to_string(), Value::String(id.to_string()));
        }
        None => {
            value.remove("id");
        }
    }
    Value::Object(value)
}

/// Fills `bundle-sections` from the sections of a product file and wraps it as a `tenant-bundle`.
/// `top` is the product file as a JSON object (its `product` header is skipped); every section is
/// expected to have passed the layer checks, but one that has not is still refused, never dropped.
/// Refuses a section that has no place in a bundle and two entities that would share an id.
pub fn build(
    id: &str,
    name: &str,
    version: &str,
    platform: Option<&Value>,
    top: &Map<String, Value>,
) -> Result<Built, Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let mut omitted = Vec::new();
    let mut entries: BTreeMap<&'static str, Vec<(String, Value)>> = BTreeMap::new();
    // (id space, id) -> the section that first declared it.
    let mut seen: BTreeMap<(&'static str, String), String> = BTreeMap::new();
    let mut has_ui = false;
    for (section, value) in top {
        if section == PRODUCT_HEADER {
            continue;
        }
        let items = value.as_array().map_or(&[][..], Vec::as_slice);
        if DOCUMENTATION_SECTIONS.contains(&section.as_str()) {
            omitted.push((section.clone(), items.len()));
            continue;
        }
        let Some(place) = place(section) else {
            let mut allowed = placed_sections();
            allowed.extend_from_slice(bundle_ui::UI_SECTIONS);
            let allowed = allowed.join(", ");
            diagnostics.push(Diagnostic::new(
                SECTION_NO_PLACE,
                section,
                format!("\"{section}\" in the product file of \"{id}\" has no place in a tenant bundle, which holds the sections {allowed} (bundle-sections, D-6580). {}", no_place_hint(section)),
            ));
            continue;
        };
        let interface = bundle_ui::UI_SECTIONS.contains(&section.as_str());
        has_ui |= interface;
        let typed = layer_values::typed_section(section);
        for item in items {
            let (Some(map), Some(item_id)) =
                (item.as_object(), item.get("id").and_then(Value::as_str))
            else {
                continue;
            };
            let key = (place.space, item_id.to_string());
            if let Some(first) = seen.get(&key) {
                diagnostics.push(Diagnostic::new(
                    ID_COLLISION,
                    item_id,
                    format!("\"{item_id}\" in \"{section}\" of the product \"{id}\" and \"{item_id}\" in \"{first}\" would be one {} of the bundle. Give one of them an id of its own, in the namespace of the product.", place.space),
                ));
                continue;
            }
            seen.insert(key, section.clone());
            if interface {
                // Written once every section is read, since an element names others.
                continue;
            }
            match (typed, section.as_str()) {
                (Some(typed), _) => entries
                    .entry(place.section)
                    .or_default()
                    .push((item_id.to_string(), typed_entry(typed, item_id, map))),
                (None, "roles") => {
                    let label = map.get("name").and_then(Value::as_str).unwrap_or(item_id);
                    entries.entry("roles").or_default().push((
                        item_id.to_string(),
                        json!({ "id": item_id, "label": label }),
                    ));
                    let permissions: Vec<Value> = map
                        .get("capabilities")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(|capability| json!({ "Capability": capability }))
                        .collect();
                    if !permissions.is_empty() {
                        entries
                            .entry("role-permissions")
                            .or_default()
                            .push((item_id.to_string(), Value::Array(permissions)));
                    }
                }
                (None, _) => {
                    let text = |key: &str| {
                        map.get(key)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    let label = match text("name") {
                        name if name.is_empty() => item_id.to_string(),
                        name => name,
                    };
                    entries.entry("capabilities").or_default().push((
                        item_id.to_string(),
                        json!({ "id": item_id, "name": label, "description": text("description") }),
                    ));
                }
            }
        }
    }
    if has_ui {
        let elements = bundle_ui::ui_section(id, top, &mut diagnostics);
        entries.entry("ui").or_default().extend(elements);
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let mut content = Map::new();
    for (section, mut items) in entries {
        items.sort_by(|a, b| a.0.cmp(&b.0));
        let value = if MAP_SECTIONS.contains(&section) {
            Value::Object(items.into_iter().collect())
        } else {
            Value::Array(items.into_iter().map(|(_, value)| value).collect())
        };
        content.insert(section.to_string(), value);
    }
    let mut bundle = Map::new();
    bundle.insert("name".to_string(), Value::String(name.to_string()));
    bundle.insert("version".to_string(), Value::String(version.to_string()));
    bundle.insert("product".to_string(), Value::String(id.to_string()));
    if let Some(platform) = platform {
        bundle.insert("platform".to_string(), platform.clone());
    }
    bundle.insert("content".to_string(), Value::Object(content));
    Ok(Built {
        bundle: canonical(&Value::Object(bundle)),
        omitted,
    })
}

/// The bundle of `product`, a product file layered on the platform specification (`platform_path`
/// is the path to print, `platform_file` where the file is, to check the product's `imports.spec`).
/// Refuses with every diagnostic of the layer checks and of the bundle rules.
pub fn generate(
    platform_path: &str,
    platform_text: &str,
    platform_file: Option<&Path>,
    product: &ProductFile,
) -> Result<Output, Vec<Diagnostic>> {
    generate_prepared(
        platform_path,
        &layers::prepare_platform(platform_path, platform_text),
        platform_file,
        product,
    )
}

/// The bundle of `product` on a platform specification that was loaded and parsed once
/// ([`layers::prepare_platform`]). The same output, and the same refusals, as [`generate`] gives
/// for the text the platform was prepared from: callers that make many bundles on one platform
/// use this, so the 6 MB platform is not parsed for each.
pub fn generate_prepared(
    platform_path: &str,
    prepared: &layers::PreparedPlatform,
    platform_file: Option<&Path>,
    product: &ProductFile,
) -> Result<Output, Vec<Diagnostic>> {
    let file = product.path.replace('\\', "/");
    let mut diagnostics = Vec::new();
    let parsed = jsonc::parse(&product.text).ok();
    let mut release = None;
    if let Some(Value::Object(top)) = &parsed {
        if let Some(Value::Object(header)) = top.get(PRODUCT_HEADER) {
            release = read_release(&file, header, &mut diagnostics);
        }
    }
    let layered = layers::layer_products(prepared, platform_file, std::slice::from_ref(product));
    let layered = match layered {
        Ok(layered) => layered,
        Err(layer_diagnostics) => {
            diagnostics.extend(layer_diagnostics);
            return Err(diagnostics);
        }
    };
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let (Some(layer), Some((version, platform)), Some(Value::Object(top))) =
        (layered.products.first(), release, &parsed)
    else {
        return Err(vec![Diagnostic::new(
            layers::HEADER_MISSING,
            &file,
            format!(
                "{file} is not a product file with a header; start it with the \"product\" header."
            ),
        )]);
    };
    let built = build(&layer.id, &layer.name, &version, platform.as_ref(), top)?;
    // The bundle is a value of tenant-bundle: check it as the typed sections are checked.
    // The layer checks above refuse a platform that does not parse, so the value is there.
    let platform_value = prepared.value().ok_or_else(|| {
        vec![Diagnostic::new(
            layers::SYNTAX,
            platform_path,
            format!("{platform_path} is not valid JSONC"),
        )]
    })?;
    let types = Types::of(platform_value);
    let whole = TypedSection {
        section: "bundle",
        ty: "tenant-bundle",
        id_field: None,
        purpose: "the bundle of a product (tenant-bundle)",
    };
    let mut checks = Vec::new();
    let mut names = Vec::new();
    layer_values::check_item(
        &types,
        &file,
        &whole,
        &layer.id,
        &built.bundle,
        &mut checks,
        &mut names,
    );
    if !checks.is_empty() {
        return Err(checks);
    }
    let mut contents = serde_json::to_string_pretty(&built.bundle).map_err(|e| {
        vec![Diagnostic::new(
            layers::SYNTAX,
            &file,
            format!("the bundle cannot be written as JSON: {e}"),
        )]
    })?;
    contents.push('\n');
    Ok(Output {
        product: layer.id.clone(),
        files: vec![GeneratedFile {
            path: format!("{}.bundle.json", layer.id),
            contents,
        }],
        omitted: built.omitted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_is_three_numbers_joined_by_dots() {
        assert!(layers::is_version("1.0.0"));
        assert!(layers::is_version("12.345.6"));
        assert!(!layers::is_version("1.0"));
        assert!(!layers::is_version("1.0.0-rc1"));
        assert!(!layers::is_version("1..0"));
        assert!(!layers::is_version(""));
    }

    #[test]
    fn every_typed_section_has_a_place_and_a_documentation_section_has_none() {
        for typed in layer_values::TYPED_SECTIONS {
            assert!(place(typed.section).is_some(), "{}", typed.section);
        }
        for section in DOCUMENTATION_SECTIONS {
            assert!(place(section).is_none(), "{section}");
        }
    }

    #[test]
    fn the_places_of_the_bundle_are_each_named_once_per_section() {
        let mut targets: Vec<&str> = placed_sections()
            .into_iter()
            .filter_map(|s| place(s).map(|p| p.section))
            .collect();
        targets.sort();
        let count = targets.len();
        targets.dedup();
        assert_eq!(count, targets.len());
    }
}
