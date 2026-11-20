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
//! - `layer.product-id-duplicate`: two product files with one `product.id`;
//! - `layer.entity-id-invalid`, `layer.entity-id-missing`: a product entity whose id fails the id
//!   pattern, or an item with no id;
//! - `layer.platform-power`: a product entity that claims what only the platform may have (a
//!   platform-level role, an anonymous, service or external holder, a public anonymous contract, a
//!   contract implemented by a platform component or realised in code, an implementation-level
//!   behavior, a domain, timer or delivery-guaranteed event);
//! - `layer.platform-references-product`: a platform entity that names a product entity;
//! - `layer.namespace-missing`: a product entity whose id is not in the namespace of its product,
//!   the product id with each hyphen replaced by an underscore, followed by two underscores
//!   (D-6582, D-6597);
//! - `layer.value-invalid`, `layer.outside-catalogue`: a value in a typed section (see
//!   [`crate::layer_values`]) that does not conform to its platform type, or a union value whose
//!   variant the platform's union lacks (INT-153, INT-1522, D-6536);
//! - `layer.expr-syntax`, `layer.expr-unknown-field`, `layer.expr-unknown-role`,
//!   `layer.expr-unknown-param`, `layer.expr-type`: an expression text of a typed section that does
//!   not parse or does not check in its scope ([`crate::layer_expr`], WG-5060);
//! - `layer.retention-missing`: an object type with a personal field and no retention (D-6584);
//! - `layer.balance-outside-ledger`: a step or transition that writes a record field from its own
//!   current value, a mutable balance outside the ledger (WG-4998);
//! - `layer.configuration-outside-setting`: an object type that holds configuration as records
//!   (named for configuration, or a key and value store) instead of a declared setting (WG-5000);
//! - `layer.reference-unknown`, `layer.cross-product-reference`: a name in a typed section, or a
//!   product capability in a `capabilities` or `requires` list, that no entity of the product
//!   declares, or that another product file declares.
//!
//! References between entities are typed by Exegra's meta-model. specc does not have it, so it
//! reads the keys that hold references ([`REFERENCE_KEYS`], checked against the meta-model by
//! `tests/layers.rs`) and requires an import for a platform entity named under one; Exegra checks
//! the typed references of the entity sections itself. Nothing is generated from a product: its
//! entities are tenant metadata, installed into a tenant as a bundle, not code of the kernel.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use regex::Regex;
use serde_json::{Map, Value};

use crate::diagnostic::Diagnostic;
use crate::ir::Spec;
use crate::layer_expr::{self, ProductContext};
use crate::layer_values::{self, RefUse, Types, NAMESPACE_SEPARATOR};
use crate::{jsonc, load};

/// The header key of a product file.
pub const PRODUCT_HEADER: &str = "product";

/// The entity sections a product file may declare (entities of the specification format), in the
/// order the messages list them, and what each is for. Permissions are roles over platform
/// capabilities; the one platform section a product adds to is its own capabilities (D-6590).
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
        "capabilities",
        "capabilities (names that tenant roles grant; each id in the namespace of the product, D-6590)",
    ),
];

/// Every section a product file may declare, in the order the messages list them: the entity
/// sections of [`PRODUCT_SECTIONS`], then the typed sections of
/// [`layer_values::TYPED_SECTIONS`] (each conforms to a platform type).
pub fn product_sections() -> Vec<(&'static str, &'static str)> {
    PRODUCT_SECTIONS
        .iter()
        .copied()
        .chain(
            layer_values::TYPED_SECTIONS
                .iter()
                .map(|s| (s.section, s.purpose)),
        )
        .collect()
}

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
/// A union value (a trigger event, a step action) outside the platform's published catalogue.
pub const OUTSIDE_CATALOGUE: &str = "layer.outside-catalogue";
/// A value in a typed section that does not conform to its platform type.
pub const VALUE_INVALID: &str = "layer.value-invalid";
/// A product entity whose id is not in the namespace of its product.
pub const NAMESPACE_MISSING: &str = "layer.namespace-missing";
/// A name in the namespace of a product that nothing declares.
pub const REFERENCE_UNKNOWN: &str = "layer.reference-unknown";
/// A name that another product file declares.
pub const CROSS_PRODUCT_REFERENCE: &str = "layer.cross-product-reference";
/// The code of a product file that is not JSONC.
pub const SYNTAX: &str = "layer.syntax";
/// Two product files with one `product.id`.
pub const PRODUCT_ID_DUPLICATE: &str = "layer.product-id-duplicate";
/// A product entity whose id fails the id pattern.
pub const ENTITY_ID_INVALID: &str = "layer.entity-id-invalid";
/// A product section item with no id (or not an object).
pub const ENTITY_ID_MISSING: &str = "layer.entity-id-missing";
/// A product entity that claims what only the platform may have.
pub const PLATFORM_POWER: &str = "layer.platform-power";
/// An object type with a personal field and no retention rule (D-6584).
pub const RETENTION_MISSING: &str = "layer.retention-missing";
/// An event kind a product declares that does not begin with `app.` and a name, or has more than
/// sixteen fields (SCH-1576, INT-3203).
pub const EVENT_KIND_INVALID: &str = "layer.event-kind-invalid";
/// A platform entity that names a product entity.
pub const PLATFORM_REFERENCES_PRODUCT: &str = "layer.platform-references-product";
/// An entity that keeps a mutable balance outside the ledger: a step or transition that writes a
/// field of a record from that field's own value (WG-4998, LDG-001).
pub const BALANCE_OUTSIDE_LEDGER: &str = "layer.balance-outside-ledger";
/// An object type that keeps configuration as records instead of declared settings (WG-5000).
pub const CONFIGURATION_OUTSIDE_SETTING: &str = "layer.configuration-outside-setting";

/// Stands for "any section": a key listing it accepts any platform entity.
const ANY: &[&str] = &[];

/// Keys that hold a reference to another entity (a string, or a list of strings), with the
/// platform sections the referenced entity may be declared in (empty: any). They are the names of
/// the reference properties of Exegra's meta-model (`src/core/metamodel.json`); `tests/layers.rs`
/// compares them with it. A platform entity named under one of them must be imported.
pub const REFERENCE_KEYS: &[(&str, &[&str])] = &[
    ("component", &["components"]),
    ("parent", &["components"]),
    ("trusts", &["assumptions"]),
    ("contract", &["contracts"]),
    (
        "from",
        &[
            "components",
            "ports",
            "roles",
            "tokenScopes",
            "delegations",
            "grants",
        ],
    ),
    ("to", &["components", "ports"]),
    ("load", &["contracts"]),
    ("commit", &["contracts"]),
    ("implementedBy", &["components"]),
    ("subject", &["components"]),
    ("record", &["records"]),
    ("createdBy", &["contracts"]),
    ("refines", &["behaviors"]),
    ("event", &["events"]),
    ("resource", &["resources", "records"]),
    ("behavior", &["behaviors"]),
    ("target", &["components", "relationships", "environments"]),
    ("environment", &["environments"]),
    ("modes", &["failureModes"]),
    ("reply", &["events"]),
    ("behaviors", &["behaviors"]),
    ("faults", &["faults"]),
    ("faultModel", &["faultModels"]),
    ("workloads", &["workloads"]),
    ("realises", &["failureModes"]),
    ("owner", &["components"]),
    ("refersTo", &["records"]),
    ("describedBy", &["types"]),
    ("of", &["records"]),
    ("heldBy", &["components"]),
    ("until", &["events"]),
    ("archiveTo", &["components"]),
    ("issuer", &["roles"]),
    ("grantee", &["roles"]),
    ("viewModel", &["contracts"]),
    ("screen", &["screens"]),
    ("form", &["forms"]),
    ("appliesTo", &["forms", "actions"]),
    ("verifies", &["claims"]),
    ("subjects", ANY),
    ("assumptions", &["assumptions"]),
    (
        "realisedBy",
        &[
            "policies",
            "validations",
            "retentions",
            "quotas",
            "retries",
            "audits",
        ],
    ),
    ("claim", &["claims"]),
    ("within", &["components"]),
];

/// Reference properties of the meta-model that [`REFERENCE_KEYS`] leaves out, because the same key
/// also holds plain text or an id local to one entity: `type` is a type text (read as one),
/// `values`, `errors` and `ast` hold literals, `source` and `basis` accept anything, and `initial`,
/// `on`, `ignores` and `transition` name states and transitions inside one behavior.
pub const UNCHECKED_REFERENCE_KEYS: &[&str] = &[
    "type",
    "values",
    "errors",
    "ast",
    "source",
    "basis",
    "initial",
    "on",
    "ignores",
    "transition",
];

/// Holders only a platform role may have.
const PLATFORM_HOLDERS: &[&str] = &["anonymous", "service", "external"];

/// Realisations that put server code behind a contract; only the platform declares those (a
/// `facade` has none).
const PLATFORM_REALISATIONS: &[&str] = &["generated", "hand", "derived"];

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
    let platform_value = jsonc::parse(platform_text).ok();
    let platform_ids = match &platform_value {
        Some(value) => declared_ids(value),
        // load::load has reported the syntax error.
        None => BTreeMap::new(),
    };
    let none = Value::Null;
    let types = Types::of(platform_value.as_ref().unwrap_or(&none));
    let mut layers = Vec::new();
    // Ids the products declare, with the file and section that declare each, to find a second
    // declaration and to resolve names.
    let mut product_ids: BTreeMap<String, (String, String)> = BTreeMap::new();
    // The namespace of each product file that has a sound header.
    let mut namespaces: BTreeMap<String, String> = BTreeMap::new();
    // The names in typed values and capability lists that must resolve to an entity of the product.
    let mut refs: Vec<RefUse> = Vec::new();
    // `product.id` of each product file, with the file, to find a second file with the same id.
    let mut header_ids: BTreeMap<String, String> = BTreeMap::new();
    for product in products {
        if let Some(layer) = check_product(
            product,
            &platform_ids,
            &types,
            platform_file,
            &mut product_ids,
            &mut namespaces,
            &mut refs,
            &mut header_ids,
            &mut diagnostics,
        ) {
            layers.push(layer);
        }
    }
    resolve_refs(
        &refs,
        &platform_ids,
        &product_ids,
        &namespaces,
        &mut diagnostics,
    );
    if let Some(value) = &platform_value {
        check_platform_independent(value, &product_ids, &mut diagnostics);
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
    types: &Types<'_>,
    platform_file: Option<&Path>,
    product_ids: &mut BTreeMap<String, (String, String)>,
    namespaces: &mut BTreeMap<String, String>,
    refs: &mut Vec<RefUse>,
    header_ids: &mut BTreeMap<String, String>,
    out: &mut Vec<Diagnostic>,
) -> Option<ProductLayer> {
    let file = product.path.replace('\\', "/");
    let before = out.len();
    let value = match jsonc::parse(&product.text) {
        Ok(value) => value,
        Err(e) => {
            let message = if e.duplicate_key {
                format!("{file}, {e}; the file is valid JSONC otherwise.")
            } else {
                format!("{file} is not valid JSONC: {e}; correct the syntax.")
            };
            out.push(Diagnostic::new(e.code(SYNTAX), &file, message));
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
    let namespace = parsed.as_ref().map(|(id, _, _)| namespace_of(id));
    if let Some(namespace) = &namespace {
        namespaces.insert(file.clone(), namespace.clone());
    }
    if let Some((product_id, _, _)) = &parsed {
        match header_ids.get(product_id) {
            Some(other) => out.push(Diagnostic::new(
                PRODUCT_ID_DUPLICATE,
                product_id,
                format!("{file} has product id \"{product_id}\", which product file {other} already has. A product is one file; give this one a product id of its own, or merge the two files."),
            )),
            None => {
                header_ids.insert(product_id.clone(), file.clone());
            }
        }
    }

    // Every key that is neither the header nor a product section is refused.
    let mut entities = Vec::new();
    let sections = product_sections();
    for key in top.keys() {
        if key == PRODUCT_HEADER || sections.iter().any(|(s, _)| s == key) {
            continue;
        }
        let allowed = sections
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
    // What the expressions of the typed sections may name: the object types and roles of the file and
    // the platform's roles.
    let context = ProductContext::of(platform_ids, top);
    let own: BTreeSet<String> = sections
        .iter()
        .filter_map(|(s, _)| top.get(*s).and_then(Value::as_array))
        .flatten()
        .filter_map(|item| item.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    for (section, _) in &sections {
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
                out.push(Diagnostic::new(
                    ENTITY_ID_MISSING,
                    &file,
                    format!("An item of \"{section}\" in {file} has no \"id\" (or is not an object): {item}. Every entity is an object with an \"id\" of lower-case words joined by hyphens; add it, or remove the item."),
                ));
                continue;
            };
            if !is_id(id) {
                out.push(Diagnostic::new(
                    ENTITY_ID_INVALID,
                    id,
                    format!("The id \"{id}\" of an entity in \"{section}\" of {file} does not match the id pattern ^[a-z][a-z0-9_]*(-[a-z0-9_]+)*$; write it as lower-case words joined by hyphens, prefixed with the product id."),
                ));
            }
            if is_id(id) {
                if let Some(namespace) = &namespace {
                    if id
                        .strip_prefix(namespace.as_str())
                        .is_none_or(str::is_empty)
                    {
                        out.push(Diagnostic::new(
                            NAMESPACE_MISSING,
                            id,
                            format!("The id \"{id}\" of an entity in \"{section}\" of {file} is not in the namespace of its product (D-6582). Every id of product \"{}\" begins with \"{namespace}\" followed by a local name; write it as \"{namespace}<local name>\".", header_product_id(&parsed)),
                        ));
                    }
                }
            }
            check_power(&file, section, id, item, platform_ids, out);
            let mut ids = BTreeMap::new();
            collect_ids(section, item, &mut ids);
            for (declared, in_section) in &ids {
                if let Some(platform_section) = platform_ids.get(declared) {
                    out.push(Diagnostic::new(
                        REDEFINES_ENTITY,
                        declared,
                        format!("{file} declares \"{declared}\" in \"{in_section}\", but the platform specification already declares \"{declared}\" in \"{platform_section}\". A product file cannot redefine or change a platform entity. Give the product's entity an id of its own (prefix it with the product id), or, to use the platform's, delete this declaration, list \"{declared}\" in product.imports.entities and refer to it by id; to change the platform's, change the platform specification."),
                    ));
                } else if let Some((other, _)) = product_ids.get(declared) {
                    out.push(Diagnostic::new(
                        REDEFINES_ENTITY,
                        declared,
                        format!("{file} declares \"{declared}\", but product file {other} already declares it. Ids must be unique across the platform and every product file of a project; rename one of them (prefix it with its product id)."),
                    ));
                } else {
                    product_ids.insert(declared.clone(), (file.clone(), in_section.clone()));
                }
            }
            entities.push(ProductEntity {
                section: section.to_string(),
                id: id.to_string(),
            });
            if parsed.is_some() {
                match layer_values::typed_section(section) {
                    Some(typed) => {
                        let mut exprs = Vec::new();
                        layer_values::check_item_exprs(
                            types, &file, typed, id, item, out, refs, &mut exprs,
                        );
                        let scope = context.scope_of(section, item);
                        for text in &exprs {
                            out.extend(layer_expr::check_text(
                                &file, id, &text.at, &text.text, &scope, &context,
                            ));
                        }
                        if *section == "objects" {
                            check_retention(&file, id, item, out);
                            check_configuration(&file, namespace.as_deref(), id, item, out);
                        }
                        if matches!(*section, "operations" | "automations" | "workflows") {
                            check_balance(&file, id, item, out);
                        }
                        if *section == "eventKinds" {
                            check_event_kind(&file, id, item, out);
                        }
                    }
                    None => named_uses(&file, id, item, platform_ids, &imports, &own, out, refs),
                }
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

/// The namespace of the entities of a product: its id with each hyphen replaced by an underscore,
/// followed by two underscores (D-6582).
pub fn namespace_of(product_id: &str) -> String {
    format!("{}{NAMESPACE_SEPARATOR}", product_id.replace('-', "_"))
}

fn header_product_id(parsed: &Option<(String, String, Vec<String>)>) -> &str {
    parsed.as_ref().map_or("", |(id, _, _)| id.as_str())
}

/// Resolves the names collected from the typed sections and the capability lists. A name with the
/// namespace separator must be declared by the product file that holds it, in one of the sections
/// its type allows; another product's entity is a cross-product reference. A name without it names
/// something outside the product and is not checked.
fn resolve_refs(
    refs: &[RefUse],
    platform_ids: &BTreeMap<String, String>,
    declared: &BTreeMap<String, (String, String)>,
    namespaces: &BTreeMap<String, String>,
    out: &mut Vec<Diagnostic>,
) {
    let mut seen = BTreeSet::new();
    for r in refs {
        if !namespaces.contains_key(&r.file)
            || !r.name.contains(NAMESPACE_SEPARATOR)
            || platform_ids.contains_key(&r.name)
            || !seen.insert((
                r.file.clone(),
                r.entity.clone(),
                r.at.clone(),
                r.name.clone(),
            ))
        {
            continue;
        }
        let (entity, file, at, name) = (&r.entity, &r.file, &r.at, &r.name);
        let wanted = r.sections.join(" or ");
        match declared.get(name) {
            Some((owner, section)) if owner == file => {
                if !r.sections.contains(&section.as_str()) {
                    out.push(Diagnostic::new(
                        REFERENCE_UNKNOWN,
                        entity,
                        format!("\"{entity}\" in {file} names \"{name}\" at {at}, which the product declares in \"{section}\", but a name there is one declared in \"{wanted}\". Name an entity of the right kind, or declare \"{name}\" in \"{wanted}\"."),
                    ));
                }
            }
            Some((owner, _)) => out.push(Diagnostic::new(
                CROSS_PRODUCT_REFERENCE,
                entity,
                format!("\"{entity}\" in {file} names \"{name}\" at {at}, which product file {owner} declares. A product may name only platform entities and its own; move what is shared into the platform specification, or declare one under this product's own namespace."),
            )),
            None => out.push(Diagnostic::new(
                REFERENCE_UNKNOWN,
                entity,
                format!("\"{entity}\" in {file} names \"{name}\" at {at}, which no product file declares (a name in \"{wanted}\" is expected). Declare it in this file under that name, or correct the name."),
            )),
        }
    }
}

/// Whether `version` is a product version: three whole numbers joined by dots (D-6583, D-6920).
pub fn is_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 9 && p.chars().all(|c| c.is_ascii_digit()))
}

/// Whether `release` names a platform release: one to three whole numbers joined by dots, and
/// optionally a hyphen and a label (`0.2`, `0.2-draft`, `1.4.0`).
pub fn is_release(release: &str) -> bool {
    let (numbers, label) = match release.split_once('-') {
        Some((numbers, label)) => (numbers, Some(label)),
        None => (release, None),
    };
    let parts: Vec<&str> = numbers.split('.').collect();
    (1..=3).contains(&parts.len())
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 9 && p.chars().all(|c| c.is_ascii_digit()))
        && label.is_none_or(|l| {
            !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
        })
}

/// The numbers of a platform release (`0.2-draft` is `[0, 2]`), for ordering two of them.
fn release_numbers(release: &str) -> Vec<u64> {
    let mut numbers: Vec<u64> = release
        .split('-')
        .next()
        .unwrap_or("")
        .split('.')
        .map(|p| p.parse::<u64>().unwrap_or(0))
        .collect();
    // `0.2` and `0.2.0` are one release.
    while numbers.last() == Some(&0) {
        numbers.pop();
    }
    numbers
}

/// The problems of a platform range of a product header (`platform-compat`, D-6583): an object with
/// a `min` that is a platform release and, when it excludes later releases, a `below` that is one
/// too and is later than `min`.
pub fn platform_problems(file: &str, value: &Value) -> Vec<String> {
    let shape =
        "{\"min\": \"<first release>\", \"below\": \"<first release it does not support>\"}";
    let Some(map) = value.as_object() else {
        return vec![format!("\"product.platform\" in {file} must be an object {shape}; correct it, or remove it for a product that states no platform range.")];
    };
    let mut problems = Vec::new();
    for key in map.keys() {
        if key != "min" && key != "below" {
            problems.push(format!("\"product.platform\" in {file} has \"{key}\", which it does not take; it holds only min and below. Remove \"{key}\"."));
        }
    }
    let mut releases = Vec::new();
    for (key, required) in [("min", true), ("below", false)] {
        match map.get(key) {
            Some(Value::String(text)) if is_release(text) => releases.push(text.as_str()),
            Some(_) => problems.push(format!("\"product.platform.{key}\" in {file} must be a platform release such as \"0.2\" or \"1.4.0\" (whole numbers joined by dots); correct it.")),
            None if required => problems.push(format!("\"product.platform\" in {file} has no \"min\", the first platform release the product supports; add it, as in {shape}.")),
            None => {}
        }
    }
    if let [min, below] = releases[..] {
        if release_numbers(below) <= release_numbers(min) {
            problems.push(format!("\"product.platform\" in {file} excludes from \"{below}\", which is not later than its first release \"{min}\", so no release is supported; make \"below\" later than \"min\"."));
        }
    }
    problems
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
        if ![
            "id",
            "name",
            "description",
            "imports",
            "version",
            "platform",
        ]
        .contains(&key.as_str())
        {
            invalid(out, format!("The product header of {file} has \"{key}\", which it does not take; it holds only id, name, description, version, platform and imports. Remove \"{key}\"."));
        }
    }
    if let Some(version) = header.get("version") {
        if !version.as_str().is_some_and(is_version) {
            invalid(out, format!("\"product.version\" in {file} is {version}, which is not a version. A version is three whole numbers joined by dots, such as \"1.0.0\" (an upgrade is offered only to a greater one, D-6583); correct it, or remove it."));
        }
    }
    if let Some(platform) = header.get("platform") {
        for message in platform_problems(file, platform) {
            invalid(out, message);
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

/// How a variant name is written as the tag of a value, under the union's `variantCase`.
pub(crate) fn variant_tag(name: &str, case: &str) -> String {
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

/// The sections of the types a type text names.
const TYPES: &[&str] = &["types"];
/// The sections of the capabilities a `capabilities` or `requires` list names.
const CAPABILITIES: &[&str] = &["capabilities"];

/// One name an entity uses: the name, the platform sections it would be declared in (empty: any),
/// and where.
type Use = (String, &'static [&'static str], String);

/// Refuses the platform entities an entity names without importing them: types (in `type` and
/// `repr` texts), capabilities (in `capabilities` and `requires` lists) and the entities under a
/// reference key ([`REFERENCE_KEYS`]), once each.
fn named_uses(
    file: &str,
    entity: &str,
    item: &Value,
    platform_ids: &BTreeMap<String, String>,
    imports: &[String],
    own: &BTreeSet<String>,
    out: &mut Vec<Diagnostic>,
    refs: &mut Vec<RefUse>,
) {
    let mut seen = BTreeSet::new();
    let mut uses = Vec::new();
    walk_uses(item, "", &mut uses);
    for (name, sections, at) in uses {
        if sections == CAPABILITIES && name.contains(NAMESPACE_SEPARATOR) {
            refs.push(RefUse {
                file: file.to_string(),
                entity: entity.to_string(),
                at,
                name,
                sections,
            });
            continue;
        }
        let declared_in_section = platform_ids
            .get(&name)
            .is_some_and(|s| sections.is_empty() || sections.contains(&s.as_str()));
        if !declared_in_section
            || imports.iter().any(|i| *i == name)
            || own.contains(&name)
            || !seen.insert(name.clone())
        {
            continue;
        }
        let kind = if sections == TYPES {
            "type"
        } else if sections == CAPABILITIES {
            "capability"
        } else {
            "entity"
        };
        out.push(Diagnostic::new(
            IMPORT_MISSING,
            entity,
            format!("\"{entity}\" in {file} uses the platform {kind} \"{name}\" (at {at}) without importing it. Add \"{name}\" to product.imports.entities, so that every platform entity the product depends on is listed in its header."),
        ));
    }
}

/// The sections a reference key's target may be declared in, when `key` is a reference key.
fn reference_sections(key: &str) -> Option<&'static [&'static str]> {
    REFERENCE_KEYS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, sections)| *sections)
}

/// The names an entity uses, and where.
fn walk_uses(value: &Value, at: &str, out: &mut Vec<Use>) {
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
                            out.push((name, TYPES, join(k)));
                        }
                    }
                    ("capabilities" | "requires", Value::Array(items)) => {
                        for (i, item) in items.iter().enumerate() {
                            if let Some(c) = item.as_str() {
                                out.push((c.to_string(), CAPABILITIES, format!("{}.{i}", join(k))));
                            }
                        }
                    }
                    (key, Value::String(name)) if reference_sections(key).is_some() => {
                        out.push((
                            name.clone(),
                            reference_sections(key).unwrap_or(ANY),
                            join(k),
                        ));
                    }
                    (key, Value::Array(items))
                        if reference_sections(key).is_some()
                            && items.iter().all(Value::is_string) =>
                    {
                        let sections = reference_sections(key).unwrap_or(ANY);
                        for (i, item) in items.iter().enumerate() {
                            if let Some(name) = item.as_str() {
                                out.push((name.to_string(), sections, format!("{}.{i}", join(k))));
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

/// Refuses a platform entity that names an entity a product file declares: the platform, and
/// everything generated from it, must not depend on a product. A name counts when it stands under
/// a reference key, in a type text, or in a capability list.
fn check_platform_independent(
    platform: &Value,
    product_ids: &BTreeMap<String, (String, String)>,
    out: &mut Vec<Diagnostic>,
) {
    if product_ids.is_empty() {
        return;
    }
    let Some(top) = platform.as_object() else {
        return;
    };
    for (section, items) in top {
        for item in items.as_array().into_iter().flatten() {
            let entity = item.get("id").and_then(Value::as_str).unwrap_or(section);
            let mut uses = Vec::new();
            walk_uses(item, "", &mut uses);
            let mut seen = BTreeSet::new();
            for (name, _, at) in uses {
                let Some((file, _)) = product_ids.get(&name) else {
                    continue;
                };
                if !seen.insert(name.clone()) {
                    continue;
                }
                out.push(Diagnostic::new(
                    PLATFORM_REFERENCES_PRODUCT,
                    entity,
                    format!("The platform entity \"{entity}\" (in \"{section}\") refers to \"{name}\" (at {at}), which product file {file} declares. The platform must not depend on a product: its text and everything generated from it are the same with or without product files. Remove the reference, or move \"{name}\" into the platform specification."),
                ));
            }
        }
    }
}

/// Refuses what a product entity claims that only the platform may have: the product adds tenant
/// metadata (ARC-113) and may not widen who can reach it or add code behind it.
/// An object type with a field that declares personal data and no retention rule (D-6584): a type
/// with a personal field must say how long its records are kept; any other type takes the default of
/// the tenant policy.
fn check_retention(file: &str, id: &str, item: &Value, out: &mut Vec<Diagnostic>) {
    if item.get("retention").is_some_and(|r| !r.is_null()) {
        return;
    }
    let personal: Vec<&str> = item
        .get("fields")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(_, field)| field.get("personal").is_some_and(|p| !p.is_null()))
        .map(|(name, _)| name.as_str())
        .collect();
    if personal.is_empty() {
        return;
    }
    let names = personal
        .iter()
        .map(|n| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(", ");
    out.push(Diagnostic::new(
        RETENTION_MISSING,
        id,
        format!(
            "The object type \"{id}\" in {file} has the personal field{} {names} and no retention (D-6584). A type with a personal field must say how long its records are kept; add \"retention\": {{ \"days\": <days>, \"from\": \"Created\" }} (or \"LastChanged\", or {{ \"Field\": {{ \"field\": <a time field> }} }}) to the object type.",
            if personal.len() == 1 { "" } else { "s" }
        ),
    ));
}

/// The step of an operation or automation, or the transition of a workflow, that writes a field of a
/// record from the same field's current value (`points = record.points + 1`) keeps a balance in the
/// record: it can be overwritten, it loses a concurrent write, and it has no journal. A balance is a
/// ledger unit posted to by `PostLedger`, whose entries are the journal and whose limits hold under
/// concurrent writes (WG-4998).
fn check_balance(file: &str, id: &str, item: &Value, out: &mut Vec<Diagnostic>) {
    let mut writes: Vec<(String, String)> = Vec::new();
    collect_self_writes(item, &mut writes);
    for (field, text) in writes {
        out.push(Diagnostic::new(
            BALANCE_OUTSIDE_LEDGER,
            id,
            format!(
                "\"{id}\" in {file} writes the field \"{field}\" from its own current value ({text:?}), which keeps a mutable balance in a record instead of the ledger. A count, a budget or a stock that goes up and down is a ledger unit: declare it under \"ledgerUnits\" and change it with a \"PostLedger\" step, whose entries are the journal, whose idempotency key makes a retry change nothing and whose limits hold under concurrent writes (LDG-001). Remove the write and post to the ledger."
            ),
        ));
    }
}

fn collect_self_writes(value: &Value, out: &mut Vec<(String, String)>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                let fields = match key.as_str() {
                    "UpdateRecord" => inner.get("fields"),
                    "set" => Some(inner),
                    _ => None,
                };
                if let Some(Value::Object(fields)) = fields {
                    for (field, expr) in fields {
                        let Some(text) = expr.as_str() else { continue };
                        let own = format!(r"\b(?:record|old|before)\.{}\b", regex::escape(field));
                        if Regex::new(&own).is_ok_and(|re| re.is_match(text)) {
                            out.push((field.clone(), text.to_string()));
                        }
                    }
                }
                collect_self_writes(inner, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|i| collect_self_writes(i, out)),
        _ => {}
    }
}

/// Words in the local name of an object type that say its records are configuration.
const CONFIGURATION_WORDS: &[&str] = &[
    "config",
    "configs",
    "configuration",
    "configurations",
    "setting",
    "settings",
    "preference",
    "preferences",
    "flag",
    "flags",
    "option",
    "options",
    "parameter",
    "parameters",
];

/// An object type whose records are configuration: its local name is made of a configuration word,
/// or it has a `key` field and a `value` field, a store of named values (WG-5000). Configuration of a
/// product is a declared setting (a `settings` entry with a type, a default and the levels that may
/// override it), which the platform layers over platform, tenant, team and person, validates, and
/// announces when it changes.
fn check_configuration(
    file: &str,
    namespace: Option<&str>,
    id: &str,
    item: &Value,
    out: &mut Vec<Diagnostic>,
) {
    let local = namespace
        .and_then(|n| id.strip_prefix(n))
        .unwrap_or(id)
        .to_string();
    let named = local
        .split(['_', '-'])
        .find(|word| CONFIGURATION_WORDS.contains(word))
        .map(|word| format!("its name has the word \"{word}\""));
    let has = |name: &str| {
        item.get("fields")
            .and_then(Value::as_object)
            .is_some_and(|f| f.contains_key(name))
    };
    let reason = named.or_else(|| {
        (has("key") && has("value"))
            .then(|| "it has a \"key\" field and a \"value\" field".to_string())
    });
    let Some(reason) = reason else { return };
    out.push(Diagnostic::new(
        CONFIGURATION_OUTSIDE_SETTING,
        id,
        format!(
            "The object type \"{id}\" in {file} holds configuration as records ({reason}). Configuration is a declared setting: add an entry to \"settings\" with a type, a default and the levels that may override it (\"tenant\", \"team\", \"person\"), and read it as the setting, so it is typed, layered, refused when a level may not override it, and announced when it changes (SET-001). Remove the object type, or rename it if its records are not configuration."
        ),
    ));
}

/// An event kind a product declares: the kind begins with `app.` and a name, which keeps it from
/// colliding with a kind of the platform, and its payload has at most sixteen fields (SCH-1576).
fn check_event_kind(file: &str, id: &str, item: &Value, out: &mut Vec<Diagnostic>) {
    const PREFIX: &str = "app.";
    const MOST_FIELDS: usize = 16;
    let kind = item.get("kind").and_then(Value::as_str).unwrap_or_default();
    if !kind.starts_with(PREFIX) || kind.len() == PREFIX.len() {
        out.push(Diagnostic::new(
            EVENT_KIND_INVALID,
            id,
            format!("The event kind \"{kind}\" of \"{id}\" in {file} does not begin with \"{PREFIX}\" and a name (SCH-1576). A kind a tenant declares is prefixed so that it cannot be a kind of the platform; write it as \"{PREFIX}<name>\"."),
        ));
    }
    let fields = item
        .get("fields")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if fields > MOST_FIELDS {
        out.push(Diagnostic::new(
            EVENT_KIND_INVALID,
            id,
            format!("The event kind \"{kind}\" of \"{id}\" in {file} has {fields} fields, and a declared kind has at most {MOST_FIELDS} (SCH-1576). Remove some, or carry them in a record the event names."),
        ));
    }
}

fn check_power(
    file: &str,
    section: &str,
    id: &str,
    item: &Value,
    platform_ids: &BTreeMap<String, String>,
    out: &mut Vec<Diagnostic>,
) {
    let text = |key: &str| item.get(key).and_then(Value::as_str);
    let mut refuse = |what: String, fix: &str| {
        out.push(Diagnostic::new(
            PLATFORM_POWER,
            id,
            format!("\"{id}\" in \"{section}\" of {file} {what}. A product adds tenant metadata only and cannot claim a power of the platform; {fix}"),
        ));
    };
    match section {
        "roles" => {
            if text("level") == Some("platform") {
                refuse(
                    "has level \"platform\"".to_string(),
                    "set level to \"tenant\": a product role is tenant data the tenant may change.",
                );
            }
            for holder in item
                .get("holders")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|h| PLATFORM_HOLDERS.contains(h))
            {
                refuse(
                    format!("lets \"{holder}\" hold it"),
                    "a product role is held by people, teams or agents; remove the holder, or declare the role in the platform specification.",
                );
            }
        }
        "contracts" => {
            let scheme = item
                .get("auth")
                .and_then(|a| a.get("scheme"))
                .and_then(Value::as_str);
            if text("exposure") == Some("public") && scheme == Some("anonymous") {
                refuse(
                    "is public with the anonymous scheme".to_string(),
                    "require a session (scheme \"session\") or make the contract internal; an anonymous public entry point is the platform's to declare.",
                );
            }
            if let Some(component) = text("implementedBy") {
                if platform_ids.get(component).map(String::as_str) == Some("components") {
                    refuse(
                        format!("is implemented by the platform component \"{component}\""),
                        "a product contract is installed as tenant metadata and has no code of the platform behind it; remove implementedBy.",
                    );
                }
            }
            if let Some(realisation) =
                text("realisation").filter(|r| PLATFORM_REALISATIONS.contains(r))
            {
                refuse(
                    format!("declares realisation \"{realisation}\""),
                    "only the platform puts code behind a contract; remove realisation (or use \"facade\").",
                );
            }
        }
        "capabilities" => {
            if item.get("core").and_then(Value::as_bool) == Some(true) {
                refuse(
                    "is a core capability".to_string(),
                    "core capabilities are held by every tenant whatever its plan; a product capability is entitled by a plan like any other. Remove \"core\".",
                );
            }
        }
        "settings" => {
            let platform_level = |key: &str| {
                item.get(key)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .any(|l| l == "platform")
            };
            if platform_level("levels") || text("locked-by") == Some("platform") {
                refuse(
                    "is set at, or locked by, the platform level".to_string(),
                    "the platform level of a setting is the platform's; a product setting takes the levels tenant, team and person (D-6594).",
                );
            }
        }
        "behaviors" => {
            if text("level") == Some("implementation") {
                refuse(
                    "has level \"implementation\"".to_string(),
                    "a product behavior is a workflow model (level \"model\"); an implementation-level machine is platform code.",
                );
            }
        }
        "events" => {
            if item.get("domain").and_then(Value::as_bool) == Some(true) {
                refuse(
                    "is a domain event".to_string(),
                    "domain events are facts the running platform records; remove \"domain\".",
                );
            }
            if item.get("elapsed").and_then(Value::as_bool) == Some(true) {
                refuse(
                    "is an elapsed-time event".to_string(),
                    "timer events are implementation level; remove \"elapsed\".",
                );
            }
            if item.get("delivery").is_some() {
                refuse(
                    "states a delivery guarantee".to_string(),
                    "delivery is implementation level; remove \"delivery\".",
                );
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
    fn a_namespace_is_the_product_id_with_underscores_and_two_underscores() {
        assert_eq!(namespace_of("badge-milestones"), "badge_milestones__");
        assert_eq!(namespace_of("badges"), "badges__");
    }

    #[test]
    fn the_typed_sections_are_listed_once_beside_the_entity_sections() {
        let sections = product_sections();
        for typed in layer_values::TYPED_SECTIONS {
            assert_eq!(
                sections.iter().filter(|(s, _)| *s == typed.section).count(),
                1,
                "{}",
                typed.section
            );
            assert!(PRODUCT_SECTIONS.iter().all(|(s, _)| *s != typed.section));
        }
        assert_eq!(
            sections.len(),
            PRODUCT_SECTIONS.len() + layer_values::TYPED_SECTIONS.len()
        );
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
