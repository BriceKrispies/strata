//! Product layers: a specification made of the platform file plus product files beside it.
//!
//! A product file is its own JSONC document. Its `product` header names the product, the platform
//! specification it builds on (`imports.spec`, a path relative to the product file) and the
//! platform entities it uses (`imports.entities`, ids). The rest of the file is sections of
//! entities, restricted to what a product may add: tenant metadata (record types, views,
//! workflows, operations, roles) and the claims, scenarios and journeys about them (the platform's
//! d-103 and ARC-113). Exegra reads the same files the same way (`src/core/layers.js` there).
//!
//! ```text
//! { "product": { "id": "badges", "name": "Badges",
//!                "imports": { "spec": "../spec.jsonc", "entities": ["tenant-id", "owner"] } },
//!   "types": [ ... ], "roles": [ ... ] }
//! ```
//!
//! [`load_layered`] loads the platform exactly as [`crate::load::load`] does, so the parsed
//! platform, and everything generated from it, is the same with or without product files. It then
//! checks every product file and refuses with one diagnostic per problem, each saying what is wrong
//! and what to do:
//!
//! - `layer.header-missing`, `layer.header-invalid`: the `product` header is absent or malformed;
//! - `layer.import-mismatch`: `imports.spec` does not resolve to the platform file;
//! - `layer.kind-not-allowed`, `layer.section-invalid`: a section a product may not declare, or a
//!   section that is not an array;
//! - `layer.import-unknown`: an imported id the platform does not declare;
//! - `layer.import-missing`: a platform type named in a type text (`type`, `repr`) or a platform
//!   capability in a `capabilities` or `requires` list that is not imported;
//! - `layer.redefines-entity`: an id the platform, or another product file, already declares;
//! - `layer.outside-catalogue`: a trigger whose event, or an automation whose step action, is not a
//!   variant of the platform's published catalogue, the unions `trigger-event` and
//!   `automation-step` (INT-153, INT-1522, D-6536).
//!
//! References between entities are typed by Exegra's meta-model, which specc does not have; Exegra
//! checks them (`layer.import-missing` for a reference, `layer.reference-unknown`,
//! `layer.cross-product-reference`). Nothing is generated from a product: its entities are tenant
//! metadata, installed into a tenant as a bundle, not code of the kernel.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value};

use crate::diagnostic::Diagnostic;
use crate::ir::Spec;
use crate::{jsonc, load};

/// The header key of a product file.
pub const PRODUCT_HEADER: &str = "product";

/// The sections a product file may declare, in the order the messages list them, and what each
/// is for. Settings are a record type; permissions are roles over platform capabilities.
pub const PRODUCT_SECTIONS: &[(&str, &str)] = &[
    ("records", "record types"),
    ("types", "the value types of its records and operations"),
    ("screens", "views"),
    ("forms", "views"),
    ("actions", "views"),
    ("liveBindings", "views"),
    ("errorMessages", "views"),
    ("behaviors", "workflows"),
    ("events", "workflows"),
    ("contracts", "operations"),
    ("roles", "roles (over platform capabilities)"),
    ("scenarios", "scenarios about its own entities"),
    ("uiScenarios", "journeys about its own entities"),
    ("claims", "claims about its own entities"),
    ("evidence", "evidence of its claims"),
    (
        "triggers",
        "triggers (values of the platform type trigger-def)",
    ),
    (
        "automations",
        "automations (values of the platform type automation-def)",
    ),
];

/// The platform union whose variants are the step kinds an automation may name (D-6522).
pub const STEP_CATALOGUE: &str = "automation-step";
/// The platform union whose variants are the events a trigger may subscribe to (D-6522).
pub const EVENT_CATALOGUE: &str = "trigger-event";

/// The header is absent, or the file is not a JSON object.
pub const HEADER_MISSING: &str = "layer.header-missing";
/// The header is malformed.
pub const HEADER_INVALID: &str = "layer.header-invalid";
/// `imports.spec` does not name the platform file.
pub const IMPORT_MISMATCH: &str = "layer.import-mismatch";
/// A section a product may not declare.
pub const KIND_NOT_ALLOWED: &str = "layer.kind-not-allowed";
/// A section that is not an array of entities.
pub const SECTION_INVALID: &str = "layer.section-invalid";
/// An imported id the platform does not declare.
pub const IMPORT_UNKNOWN: &str = "layer.import-unknown";
/// A platform entity used by name without being imported.
pub const IMPORT_MISSING: &str = "layer.import-missing";
/// An id the platform or another product file already declares.
pub const REDEFINES_ENTITY: &str = "layer.redefines-entity";
/// A trigger event or a step action outside the platform's published catalogue.
pub const OUTSIDE_CATALOGUE: &str = "layer.outside-catalogue";
/// The code of a product file that is not JSONC.
pub const SYNTAX: &str = "layer.syntax";

/// Sections of the specification format that are the platform's alone, with the reason a
/// product may not declare them.
const PLATFORM_OWNED: &[(&str, &str)] = &[
    ("exegra", "The schema version is the platform specification's; remove it."),
    ("system", "The system is the platform's; remove it."),
    (
        "vocabulary",
        "The vocabulary is the platform's, and a product must not change how the platform is read; remove it.",
    ),
];

/// Every section of the specification format (Exegra's meta-model) that holds entities.
const FORMAT_SECTIONS: &[&str] = &[
    "components",
    "relationships",
    "contracts",
    "events",
    "behaviors",
    "faults",
    "environments",
    "failureModes",
    "faultModels",
    "types",
    "languages",
    "functions",
    "exampleFixtures",
    "records",
    "copies",
    "policies",
    "validations",
    "retentions",
    "quotas",
    "retries",
    "audits",
    "capabilities",
    "plans",
    "roles",
    "grants",
    "delegations",
    "tokenScopes",
    "featureFlags",
    "screens",
    "forms",
    "actions",
    "liveBindings",
    "errorMessages",
    "uiScenarios",
    "resources",
    "links",
    "workloads",
    "requestClasses",
    "scenarios",
    "assumptions",
    "claims",
    "integrityRules",
    "evidence",
    "views",
];

/// The arrays inside an entity that hold entities of their own, with ids in the same id space
/// (a behavior's states and transitions, a component's ports).
const NESTED: &[&str] = &["states", "transitions", "ports"];

const HEADER_SHAPE: &str = "{\"product\": {\"id\": \"<product id>\", \"name\": \"<name>\", \"imports\": {\"spec\": \"<path of the platform specification, relative to this file>\", \"entities\": [\"<platform entity id>\", ...]}}}";

/// One product file to load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductFile {
    /// The path to print in diagnostics, as given.
    pub path: String,
    /// The file's text.
    pub text: String,
    /// Where the file is, to resolve its `imports.spec` against; without it the import path is
    /// not checked.
    pub file: Option<PathBuf>,
}

/// One entity a product declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductEntity {
    /// The section it is declared in, such as `records`.
    pub section: String,
    /// Its id.
    pub id: String,
}

/// A product file that passed every check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductLayer {
    /// The path it was loaded from, with forward slashes.
    pub path: String,
    /// `product.id`.
    pub id: String,
    /// `product.name`.
    pub name: String,
    /// `product.imports.entities`, in order.
    pub imports: Vec<String>,
    /// The entities of its sections, in file order (sections in the order of [`PRODUCT_SECTIONS`]).
    pub entities: Vec<ProductEntity>,
}

/// A layered specification: the platform, loaded exactly as [`load::load`] loads it, and its
/// product files.
#[derive(Debug, Clone, PartialEq)]
pub struct Layered {
    /// The platform specification.
    pub platform: Spec,
    /// The product files, in the order given.
    pub products: Vec<ProductLayer>,
}

/// Loads the platform specification and checks the product files layered on it. `platform_path`
/// is the path to print in headers and diagnostics; `platform_file` where the platform file is,
/// to check each product's `imports.spec` against (not checked when `None`). Refuses with every
/// diagnostic of the platform and of every product file at once.
pub fn load_layered(
    platform_path: &str,
    platform_text: &str,
    platform_file: Option<&Path>,
    products: &[ProductFile],
) -> Result<Layered, Vec<Diagnostic>> {
    let loaded = load::load(platform_path, platform_text);
    let mut diagnostics = Vec::new();
    let (platform_ids, catalogue) = match jsonc::parse(platform_text) {
        Ok(value) => (declared_ids(&value), Catalogue::of(&value)),
        // load::load has reported the syntax error.
        Err(_) => (BTreeMap::new(), Catalogue::default()),
    };
    let mut layers = Vec::new();
    // Ids the products declare, with the file that declares each, to find a second declaration.
    let mut product_ids: BTreeMap<String, String> = BTreeMap::new();
    for product in products {
        if let Some(layer) = check_product(
            product,
            &platform_ids,
            &catalogue,
            platform_file,
            &mut product_ids,
            &mut diagnostics,
        ) {
            layers.push(layer);
        }
    }
    match loaded {
        Ok(platform) if diagnostics.is_empty() => Ok(Layered {
            platform,
            products: layers,
        }),
        Ok(_) => Err(diagnostics),
        Err(mut platform_diagnostics) => {
            platform_diagnostics.extend(diagnostics);
            Err(platform_diagnostics)
        }
    }
}

/// Every entity id the specification declares, at the top of a section or nested in an entity,
/// with the section it is declared in.
pub fn declared_ids(value: &Value) -> BTreeMap<String, String> {
    let mut ids = BTreeMap::new();
    let Some(top) = value.as_object() else {
        return ids;
    };
    for (section, items) in top {
        for item in items.as_array().into_iter().flatten() {
            collect_ids(section, item, &mut ids);
        }
    }
    ids
}

fn collect_ids(section: &str, item: &Value, ids: &mut BTreeMap<String, String>) {
    let Some(obj) = item.as_object() else {
        return;
    };
    if let Some(id) = obj.get("id").and_then(Value::as_str) {
        ids.entry(id.to_string())
            .or_insert_with(|| section.to_string());
    }
    for nested in NESTED {
        for child in obj
            .get(*nested)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            collect_ids(nested, child, ids);
        }
    }
}

fn check_product(
    product: &ProductFile,
    platform_ids: &BTreeMap<String, String>,
    catalogue: &Catalogue,
    platform_file: Option<&Path>,
    product_ids: &mut BTreeMap<String, String>,
    out: &mut Vec<Diagnostic>,
) -> Option<ProductLayer> {
    let file = product.path.replace('\\', "/");
    let before = out.len();
    let value = match jsonc::parse(&product.text) {
        Ok(value) => value,
        Err(e) => {
            out.push(Diagnostic::new(
                SYNTAX,
                &file,
                format!("{file} is not valid JSONC: {e}; correct the syntax."),
            ));
            return None;
        }
    };
    let Some(top) = value.as_object() else {
        out.push(Diagnostic::new(
            HEADER_MISSING,
            &file,
            format!("{file} is not a JSON object. A product file is an object that starts with the header {HEADER_SHAPE}, followed by sections of entities."),
        ));
        return None;
    };
    let header = match top.get(PRODUCT_HEADER) {
        Some(Value::Object(header)) => Some(header),
        Some(_) => {
            out.push(Diagnostic::new(
                HEADER_INVALID,
                &file,
                format!("\"product\" in {file} must be an object {HEADER_SHAPE}."),
            ));
            None
        }
        None => {
            out.push(Diagnostic::new(
                HEADER_MISSING,
                &file,
                format!("{file} has no \"product\" header. A product file starts with {HEADER_SHAPE}; add it."),
            ));
            None
        }
    };
    let parsed = header.and_then(|h| check_header(&file, h, product, platform_file, out));
    let imports: Vec<String> = parsed
        .as_ref()
        .map(|(_, _, imports)| imports.clone())
        .unwrap_or_default();

    // Every key that is neither the header nor a product section is refused.
    let mut entities = Vec::new();
    for key in top.keys() {
        if key == PRODUCT_HEADER || PRODUCT_SECTIONS.iter().any(|(s, _)| s == key) {
            continue;
        }
        let allowed = PRODUCT_SECTIONS
            .iter()
            .map(|(s, _)| *s)
            .collect::<Vec<_>>()
            .join(", ");
        let why = PLATFORM_OWNED
            .iter()
            .find(|(s, _)| s == key)
            .map(|(_, why)| why.to_string())
            .unwrap_or_else(|| {
                if FORMAT_SECTIONS.contains(&key.as_str()) {
                    "It is platform code or structure: a product refers to the platform's by id (list them in product.imports.entities) and does not declare its own. If the product truly needs a new one, it belongs in the platform specification.".to_string()
                } else {
                    "It is not a section of the specification format either: remove it, or, if it is product data, model it as a record type.".to_string()
                }
            });
        out.push(Diagnostic::new(
            KIND_NOT_ALLOWED,
            &file,
            format!("{file} declares \"{key}\", which a product file may not declare. A product adds only tenant metadata with no change to the kernel (ARC-113), in these sections: {allowed}. {why}"),
        ));
    }
    let own: BTreeSet<String> = PRODUCT_SECTIONS
        .iter()
        .filter_map(|(s, _)| top.get(*s).and_then(Value::as_array))
        .flatten()
        .filter_map(|item| item.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    for (section, _) in PRODUCT_SECTIONS {
        let Some(value) = top.get(*section) else {
            continue;
        };
        let Some(items) = value.as_array() else {
            out.push(Diagnostic::new(
                SECTION_INVALID,
                &file,
                format!("\"{section}\" in {file} must be an array of entities; write it as \"{section}\": [ {{ \"id\": ... }}, ... ]."),
            ));
            continue;
        };
        for item in items {
            let Some(id) = item.get("id").and_then(Value::as_str) else {
                // An entity without an id is Exegra's to report (id.missing).
                continue;
            };
            let mut ids = BTreeMap::new();
            collect_ids(section, item, &mut ids);
            for (declared, in_section) in &ids {
                if let Some(platform_section) = platform_ids.get(declared) {
                    out.push(Diagnostic::new(
                        REDEFINES_ENTITY,
                        declared,
                        format!("{file} declares \"{declared}\" in \"{in_section}\", but the platform specification already declares \"{declared}\" in \"{platform_section}\". A product file cannot redefine or change a platform entity. Give the product's entity an id of its own (prefix it with the product id), or, to use the platform's, delete this declaration, list \"{declared}\" in product.imports.entities and refer to it by id; to change the platform's, change the platform specification."),
                    ));
                } else if let Some(other) = product_ids.get(declared) {
                    out.push(Diagnostic::new(
                        REDEFINES_ENTITY,
                        declared,
                        format!("{file} declares \"{declared}\", but product file {other} already declares it. Ids must be unique across the platform and every product file of a project; rename one of them (prefix it with its product id)."),
                    ));
                } else {
                    product_ids.insert(declared.clone(), file.clone());
                }
            }
            entities.push(ProductEntity {
                section: section.to_string(),
                id: id.to_string(),
            });
            if parsed.is_some() {
                named_uses(&file, id, item, platform_ids, &imports, &own, out);
                catalogue_uses(&file, id, section, item, catalogue, out);
            }
        }
    }
    for id in &imports {
        if platform_ids.contains_key(id) {
            continue;
        }
        let message = if own.contains(id) {
            format!("{file} imports \"{id}\", but that is this product's own entity, not a platform one; remove \"{id}\" from product.imports.entities.")
        } else {
            format!("{file} imports \"{id}\" (product.imports.entities), but the platform specification declares no entity with that id. Correct the id to one the platform declares, or remove it from product.imports.entities; if the product needs a new entity of its own, declare it in this file under an id of its own.")
        };
        out.push(Diagnostic::new(IMPORT_UNKNOWN, id, message));
    }
    if out.len() > before {
        return None;
    }
    let (id, name, imports) = parsed?;
    Some(ProductLayer {
        path: file,
        id,
        name,
        imports,
        entities,
    })
}

/// Checks the header; returns its id, name and imported ids when it is well-formed.
fn check_header(
    file: &str,
    header: &Map<String, Value>,
    product: &ProductFile,
    platform_file: Option<&Path>,
    out: &mut Vec<Diagnostic>,
) -> Option<(String, String, Vec<String>)> {
    let before = out.len();
    let invalid = |out: &mut Vec<Diagnostic>, message: String| {
        out.push(Diagnostic::new(HEADER_INVALID, file, message));
    };
    for key in header.keys() {
        if !["id", "name", "description", "imports"].contains(&key.as_str()) {
            invalid(out, format!("The product header of {file} has \"{key}\", which it does not take; it holds only id, name, description and imports. Remove \"{key}\"."));
        }
    }
    let id = header
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| is_id(id));
    if id.is_none() {
        invalid(out, format!("\"product.id\" in {file} must be a lower-case id such as \"badges\" (lower-case words joined by hyphens); {}.", if header.contains_key("id") { "correct it" } else { "add it" }));
    }
    let name = header
        .get("name")
        .and_then(Value::as_str)
        .filter(|n| !n.trim().is_empty());
    if name.is_none() {
        invalid(
            out,
            format!(
                "\"product.name\" in {file} must be a non-empty string naming the product; {}.",
                if header.contains_key("name") {
                    "correct it"
                } else {
                    "add it"
                }
            ),
        );
    }
    let Some(imports) = header.get("imports").and_then(Value::as_object) else {
        invalid(out, format!("\"product.imports\" in {file} must be an object {{\"spec\": \"<path of the platform specification, relative to this file>\", \"entities\": [\"<platform entity id>\", ...]}}; {}.", if header.contains_key("imports") { "correct it" } else { "add it" }));
        return None;
    };
    for key in imports.keys() {
        if key != "spec" && key != "entities" {
            invalid(out, format!("\"product.imports\" in {file} has \"{key}\", which it does not take; it holds only spec and entities. Remove \"{key}\"."));
        }
    }
    let expected = match (platform_file, &product.file) {
        (Some(platform), Some(own)) => own
            .parent()
            .and_then(|dir| crate::emit::relative_path(&lexical(dir), &lexical(platform))),
        _ => None,
    };
    match imports.get("spec").and_then(Value::as_str) {
        Some(spec) if spec.ends_with(".jsonc") => {
            if let (Some(platform), Some(own)) = (platform_file, &product.file) {
                let resolved = own
                    .parent()
                    .map(|dir| lexical(&dir.join(spec)))
                    .unwrap_or_else(|| lexical(Path::new(spec)));
                if Path::new(spec).is_absolute() || !same_path(&resolved, &lexical(platform)) {
                    out.push(Diagnostic::new(
                        IMPORT_MISMATCH,
                        file,
                        format!("{file} imports the platform specification \"{spec}\", which is not the platform specification it is loaded with. Set \"product.imports.spec\" to \"{}\", or do not pass {file} with this platform if it belongs to another one.", expected.as_deref().unwrap_or("<the platform file, relative to this file>")),
                    ));
                }
            }
        }
        other => invalid(out, format!("\"product.imports.spec\" in {file} must name the platform specification as a path relative to this file, ending in .jsonc{}; {}.", expected.as_ref().map(|e| format!(" (here \"{e}\")")).unwrap_or_default(), if other.is_some() || imports.contains_key("spec") { "correct it" } else { "add it" })),
    }
    let mut ids = Vec::new();
    match imports.get("entities").and_then(Value::as_array) {
        Some(items) => {
            for item in items {
                match item.as_str() {
                    Some(id) if ids.iter().any(|i| i == id) => {
                        invalid(out, format!("{file} imports \"{id}\" twice; remove the second."))
                    }
                    Some(id) => ids.push(id.to_string()),
                    None => invalid(out, format!("Every item of \"product.imports.entities\" in {file} must be a platform entity id (a string); correct or remove {item}.")),
                }
            }
        }
        None => invalid(out, format!("\"product.imports.entities\" in {file} must be an array of the ids of the platform entities the product uses (possibly empty); {}.", if imports.contains_key("entities") { "correct it" } else { "add it" })),
    }
    if out.len() > before {
        return None;
    }
    Some((id?.to_string(), name?.to_string(), ids))
}

/// The platform's published catalogue (INT-153, INT-1522): the tags of the variants of the unions
/// [`STEP_CATALOGUE`] and [`EVENT_CATALOGUE`], as a value of each is written (its
/// `encoding.variantCase`). `None` when the platform declares no such union.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalogue {
    /// The tags of the step kinds an automation may name.
    pub steps: Option<BTreeSet<String>>,
    /// The tags of the events a trigger may subscribe to.
    pub events: Option<BTreeSet<String>>,
}

impl Catalogue {
    /// Reads the catalogue from the parsed platform specification.
    pub fn of(platform: &Value) -> Self {
        Catalogue {
            steps: union_tags(platform, STEP_CATALOGUE),
            events: union_tags(platform, EVENT_CATALOGUE),
        }
    }
}

/// The tags of the variants of the union type `id`, or `None` when the platform declares none.
fn union_tags(platform: &Value, id: &str) -> Option<BTreeSet<String>> {
    let ty = platform
        .get("types")?
        .as_array()?
        .iter()
        .find(|t| t.get("id").and_then(Value::as_str) == Some(id))?;
    let case = ty
        .get("encoding")
        .and_then(|e| e.get("variantCase"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let variants = ty.get("variants")?.as_array()?;
    Some(
        variants
            .iter()
            .filter_map(|v| v.get("name").and_then(Value::as_str))
            .map(|name| variant_tag(name, case))
            .collect(),
    )
}

/// How a variant name is written as the tag of a value, under the union's `variantCase`.
fn variant_tag(name: &str, case: &str) -> String {
    match case {
        "PascalCase" => name
            .split(|c: char| c == '-' || c == '_')
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect(),
        "snake_case" => name.replace('-', "_"),
        _ => name.to_string(),
    }
}

/// Refuses a trigger whose `on`, or an automation whose step `action`, is not a variant of the
/// platform's catalogue (`layer.outside-catalogue`).
fn catalogue_uses(
    file: &str,
    entity: &str,
    section: &str,
    item: &Value,
    catalogue: &Catalogue,
    out: &mut Vec<Diagnostic>,
) {
    match section {
        "triggers" => check_tag(
            file,
            entity,
            "on",
            item.get("on"),
            catalogue.events.as_ref(),
            EVENT_CATALOGUE,
            out,
        ),
        "automations" => {
            let Some(steps) = item.get("steps").and_then(Value::as_object) else {
                return;
            };
            for (name, step) in steps {
                check_tag(
                    file,
                    entity,
                    &format!("steps.{name}.action"),
                    step.get("action"),
                    catalogue.steps.as_ref(),
                    STEP_CATALOGUE,
                    out,
                );
            }
        }
        _ => {}
    }
}

/// Refuses `value` unless it is one variant of the union `union`: an object with one key that is
/// a published tag, or, for a variant without fields, the tag as a string.
fn check_tag(
    file: &str,
    entity: &str,
    at: &str,
    value: Option<&Value>,
    published: Option<&BTreeSet<String>>,
    union: &str,
    out: &mut Vec<Diagnostic>,
) {
    let Some(published) = published else {
        out.push(Diagnostic::new(
            OUTSIDE_CATALOGUE,
            entity,
            format!("\"{entity}\" in {file} declares {at}, but the platform specification publishes no catalogue for it: it declares no union \"{union}\". Remove the entity, or layer the product on a platform that publishes \"{union}\"."),
        ));
        return;
    };
    let tag = match value {
        Some(Value::String(tag)) => Some(tag.as_str()),
        Some(Value::Object(map)) if map.len() == 1 => map.keys().next().map(String::as_str),
        _ => None,
    };
    let variants = published.iter().cloned().collect::<Vec<_>>().join(", ");
    match tag {
        Some(tag) if published.contains(tag) => {}
        Some(tag) => out.push(Diagnostic::new(
            OUTSIDE_CATALOGUE,
            entity,
            format!("\"{entity}\" in {file} names \"{tag}\" at {at}, which is outside the published catalogue (INT-153): the platform's union \"{union}\" has the variants {variants}. Use one of them; a kind the catalogue lacks belongs in the platform specification, not in a product."),
        )),
        None => out.push(Diagnostic::new(
            OUTSIDE_CATALOGUE,
            entity,
            format!("{at} of \"{entity}\" in {file} must be one variant of the platform's union \"{union}\", written {{\"<Variant>\": {{ ... }}}}, or \"<Variant>\" for a variant without fields; the variants are {variants}."),
        )),
    }
}

/// Refuses the platform types (named in `type` and `repr` texts) and capabilities (in
/// `capabilities` and `requires` lists) an entity uses without importing them, once each.
fn named_uses(
    file: &str,
    entity: &str,
    item: &Value,
    platform_ids: &BTreeMap<String, String>,
    imports: &[String],
    own: &BTreeSet<String>,
    out: &mut Vec<Diagnostic>,
) {
    let mut seen = BTreeSet::new();
    let mut uses = Vec::new();
    walk_uses(item, "", &mut uses);
    for (name, section, at) in uses {
        if platform_ids.get(&name).map(String::as_str) != Some(section)
            || imports.iter().any(|i| *i == name)
            || own.contains(&name)
            || !seen.insert(name.clone())
        {
            continue;
        }
        let kind = if section == "types" {
            "type"
        } else {
            "capability"
        };
        out.push(Diagnostic::new(
            IMPORT_MISSING,
            entity,
            format!("\"{entity}\" in {file} uses the platform {kind} \"{name}\" (at {at}) without importing it. Add \"{name}\" to product.imports.entities, so that every platform entity the product depends on is listed in its header."),
        ));
    }
}

/// The names an entity uses: (name, the platform section it would be declared in, where).
fn walk_uses(value: &Value, at: &str, out: &mut Vec<(String, &'static str, String)>) {
    let join = |k: &str| {
        if at.is_empty() {
            k.to_string()
        } else {
            format!("{at}.{k}")
        }
    };
    match value {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                walk_uses(item, &join(&i.to_string()), out);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                match (k.as_str(), v) {
                    ("type" | "repr", Value::String(text)) => {
                        for name in type_names(text) {
                            out.push((name, "types", join(k)));
                        }
                    }
                    ("capabilities" | "requires", Value::Array(items)) => {
                        for (i, item) in items.iter().enumerate() {
                            if let Some(c) = item.as_str() {
                                out.push((
                                    c.to_string(),
                                    "capabilities",
                                    format!("{}.{i}", join(k)),
                                ));
                            }
                        }
                    }
                    _ => walk_uses(v, &join(k), out),
                }
            }
        }
        _ => {}
    }
}

/// The plain type names inside a type text: list<T>, set<T>, option<T>, shared<T> and map<K, V>
/// are read as Exegra reads them (`parseTypeName`); anything else is one name.
pub fn type_names(text: &str) -> Vec<String> {
    let text = text.trim();
    for wrapper in ["list<", "set<", "option<", "shared<"] {
        if let Some(inner) = text.strip_prefix(wrapper).and_then(|t| t.strip_suffix('>')) {
            return type_names(inner);
        }
    }
    if let Some(inner) = text.strip_prefix("map<").and_then(|t| t.strip_suffix('>')) {
        let args = split_top(inner);
        if args.len() == 2 && !args[0].trim().is_empty() && !args[1].trim().is_empty() {
            let mut names = type_names(args[0]);
            names.extend(type_names(args[1]));
            return names;
        }
    }
    if text.is_empty() {
        Vec::new()
    } else {
        vec![text.to_string()]
    }
}

/// `text` split on the commas outside angle brackets.
fn split_top(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in text.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

/// Whether `id` is lower-case words joined by hyphens (Exegra's id pattern
/// `^[a-z][a-z0-9_]*(-[a-z0-9_]+)*$`).
fn is_id(id: &str) -> bool {
    let mut words = id.split('-');
    let first_ok = words.next().is_some_and(|w| {
        w.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            && w.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    });
    first_ok
        && words.all(|w| {
            !w.is_empty()
                && w.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
}

/// `path` made absolute against the current directory, with `.` and `..` resolved by name.
fn lexical(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|d| d.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut out = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Whether two normalised paths name the same file (case-insensitively on Windows).
fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_names_read_wrappers_as_exegra_does() {
        assert_eq!(type_names("list<tenant-id>"), vec!["tenant-id"]);
        assert_eq!(
            type_names("map<string, option<tier>>"),
            vec!["string", "tier"]
        );
        assert_eq!(type_names("map<a>"), vec!["map<a>"]);
        assert_eq!(type_names("item-id[]"), vec!["item-id[]"]);
    }

    #[test]
    fn variant_tags_follow_the_encoding_of_the_union() {
        assert_eq!(variant_tag("create-record", "PascalCase"), "CreateRecord");
        assert_eq!(variant_tag("end", "PascalCase"), "End");
        assert_eq!(variant_tag("create-record", "snake_case"), "create_record");
        assert_eq!(variant_tag("create-record", ""), "create-record");
    }

    #[test]
    fn the_catalogue_is_read_from_the_platform_unions() {
        let platform: Value = serde_json::json!({ "types": [
            { "id": "automation-step", "kind": "union", "encoding": { "variantCase": "PascalCase" },
              "variants": [ { "name": "wait-for" }, { "name": "end" } ] }
        ] });
        let catalogue = Catalogue::of(&platform);
        assert_eq!(
            catalogue.steps,
            Some(
                ["End".to_string(), "WaitFor".to_string()]
                    .into_iter()
                    .collect()
            )
        );
        assert_eq!(catalogue.events, None);
    }

    #[test]
    fn ids_follow_the_id_pattern() {
        assert!(is_id("badges"));
        assert!(is_id("badges-tier_2"));
        assert!(!is_id("Badges"));
        assert!(!is_id("badges-"));
        assert!(!is_id("-badges"));
    }
}
