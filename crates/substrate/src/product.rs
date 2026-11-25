//! Product bundles: the `tenant-bundle` of the specification (D-6580, D-6581, D-6582, D-6583).
//!
//! A product module is installed into a tenant that exists, as a bundle of declared metadata that
//! specc writes from the product's own file (`specc bundle`). Where the whole-tenant
//! [`crate::bundle::Bundle`] is one tenant written down once, a [`ProductBundle`] is read,
//! projected and installed into a tenant that already has data, upgraded to a later version of the
//! product, and removed, and every entity it installs is recorded as owned by its product.
//!
//! The kernel takes every change through one `ChangeSchema` intent, so the edits of a product
//! change are applied together or not at all (D-6581). The ownership record of the product
//! (`installed-product`) is one of those edits ([`SchemaChange::RecordProduct`], held in the
//! tenant schema), so it is committed with the entities it names, under one
//! schema version: there is no moment at which an entity exists that no product owns.
//!
//! Three operations, each in two steps: a plan (what changes, with nothing run) and a run:
//!
//! - install: [`plan_install`], [`install_product`];
//! - upgrade: [`plan_upgrade`], [`upgrade_product`]. The difference between the installed and the
//!   new version is computed from the ownership record: what the new version declares and the
//!   tenant lacks is created, what differs is changed, and what the record owns and the new
//!   version no longer declares is removed;
//! - remove: [`plan_remove`], [`remove_product`]. Exactly what the record owns is removed, and a
//!   record type that has records is refused unless the removal says the records are deleted.
//!
//! A section the kernel cannot take is not dropped: reading the bundle records it
//! ([`ProductBundle::unbuilt`]) and installing a bundle that fills one is refused as an invalid
//! bundle that names it (`install-bundle`, error `invalid-bundle`). No section is in that state
//! now. The capabilities, settings and ledger declarations of a bundle are schema changes
//! (`define-capability`, `define-setting`, `declare-ledger-unit`, `declare-ledger-pool`,
//! `set-ledger-limit`), owned by the product like every other entity.
//!
//! **Segments** are the section that is no schema change (WG-5166): they live in the segment store
//! beside the tenant state, so a plan names them in [`ProductChange::segments`] and the caller
//! that holds the store defines and removes them ([`SegmentChanges`]). The ownership record of the
//! product still lists them (`segment`, by the namespaced key of the bundle), in the same schema
//! change as every other entity, and the id of an installed segment is derived from the tenant
//! and that key (D-6582), so an install gives the same id every time.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json};
use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::expr::parse;
use strata_kernel::segments::{derive_segment_id, SegmentDecl, SegmentId, SegmentKey};
use strata_kernel::*;

// ---- the sections of a bundle ---------------------------------------------------------------

/// The members of a `tenant-bundle` document.
const BUNDLE_MEMBERS: &[&str] = &["name", "version", "product", "platform", "content"];

/// The sections of `bundle-sections` that the kernel takes through schema changes.
const BUILT_SECTIONS: &[&str] = &[
    "objects",
    "roles",
    "role-permissions",
    "policies",
    "queries",
    "views",
    "workflows",
    "mail-templates",
    "notice-templates",
    "tokens",
    "triggers",
    "automations",
    "operations",
    "holds",
    "capabilities",
    "settings",
    "ledger-units",
    "ledger-pools",
    "ledger-limits",
    "segments",
    "event-kinds",
    "ui",
];

/// The kind of document in the store's small-document table (`Store::doc_put`) that held the
/// [`InstalledRecord`] of a product before the record moved into the tenant schema. Nothing writes
/// it any more: the record of a product is [`SchemaChange::RecordProduct`], committed with the
/// schema change that installs the product (D-6581).
pub const CATALOGUE_KIND: &str = "product";

/// Which contract error a bundle problem is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BundleErrorKind {
    /// `invalid-bundle`: the document is malformed, or it fills a section this release does not
    /// install yet.
    InvalidBundle,
    /// `invalid`: an id outside the namespace of the product (INT-1582), a platform release
    /// outside the range of the bundle (INT-1583), or a version that is not greater than the
    /// installed one.
    Invalid,
    /// `conflict`: an entity with an id of the bundle exists and the product does not own it, or
    /// another version of the product is installed (INT-1582, INT-1583).
    Conflict,
    /// `not-found`: the product is not installed.
    NotFound,
    /// `upgrade.final-stage-too-large`: the last stage of a staged upgrade alone is over the cap,
    /// so the product must be split across releases (D-DS-16, KRN-1283).
    FinalStageTooLarge,
    /// `upgrade.in-progress`: a staged upgrade of the product is open, and only its owner may
    /// abandon it (D-DS-16).
    InProgress,
    /// `product.base-moved`: the remaining stages of a staged upgrade no longer match the approved
    /// diff, or a stage conflicted twice (D-DS-16).
    BaseMoved,
}

/// A bundle that cannot be read or installed, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleError {
    pub kind: BundleErrorKind,
    pub message: String,
}

impl BundleError {
    fn invalid_bundle(message: impl Into<String>) -> Self {
        BundleError {
            kind: BundleErrorKind::InvalidBundle,
            message: message.into(),
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        BundleError {
            kind: BundleErrorKind::Invalid,
            message: message.into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        BundleError {
            kind: BundleErrorKind::Conflict,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        BundleError {
            kind: BundleErrorKind::NotFound,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for BundleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for BundleError {}

/// The platform releases a bundle is written for: from `min` up to, and excluding, `below`
/// (`platform-compat`, D-6583).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformRange {
    pub min: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub below: Option<String>,
}

/// A section, or a declaration inside a section, that the bundle fills and the kernel cannot take
/// yet. `section` is the section (`segments`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unbuilt {
    pub section: String,
    /// How many entries of the section there are.
    pub entries: usize,
}

/// A tenant bundle of a product (`tenant-bundle`): the sections the kernel can install are typed
/// kernel values; the others are only counted in [`ProductBundle::unbuilt`].
#[derive(Clone, Debug)]
pub struct ProductBundle {
    pub name: String,
    pub version: String,
    /// The product that owns what the bundle installs. A bundle without one (a platform template)
    /// installs entities that belong to the tenant, and records no owner.
    pub product: Option<String>,
    pub platform: Option<PlatformRange>,
    pub objects: BTreeMap<ObjectTypeId, ObjectTypeDef>,
    pub roles: BTreeMap<RoleId, RoleDef>,
    pub role_permissions: BTreeMap<RoleId, Vec<Permission>>,
    pub policies: BTreeMap<PolicyId, PolicyDef>,
    pub queries: BTreeMap<QueryId, QueryDef>,
    pub views: BTreeMap<ViewId, ViewDef>,
    pub workflows: BTreeMap<WorkflowId, WorkflowDef>,
    pub mail_templates: BTreeMap<TemplateId, MailTemplateDecl>,
    pub notice_templates: BTreeMap<TemplateId, NoticeTemplateDecl>,
    pub tokens: BTreeMap<String, TokenDecl>,
    pub triggers: BTreeMap<TriggerId, TriggerDef>,
    pub automations: BTreeMap<WorkflowId, AutomationDef>,
    pub operations: BTreeMap<TenantOperationId, TenantOperationDef>,
    /// The rules that hold a change for approval (`holds`, D-8400), installed after the
    /// operations, roles and settings they name.
    pub holds: BTreeMap<HoldRuleId, HoldRule>,
    pub capabilities: BTreeMap<CapabilityName, CapabilityDef>,
    pub settings: BTreeMap<SettingId, SettingDef>,
    pub ledger_units: BTreeMap<LedgerUnit, LedgerUnitDecl>,
    pub ledger_pools: BTreeMap<LedgerPoolName, LedgerPoolDecl>,
    /// The ledger limits by the id [`strata_kernel::products::ledger_limit_id`] gives each.
    pub ledger_limits: BTreeMap<String, LedgerLimit>,
    /// The segments of the bundle by their keys, which are in the namespace of the product like
    /// every other id (D-6582). They are installed through the segment store, not the schema.
    pub segments: BTreeMap<SegmentKey, SegmentDecl>,
    /// The event kinds the bundle declares (`event-kinds`, SCH-1576, INT-3203), by kind. Each is
    /// `app.` and a name; the declaration is read and checked here, and installing it is
    /// [`ProductBundle::unbuilt`] until the kernel holds a tenant's declared kinds.
    pub event_kinds: BTreeSet<String>,
    /// The interface elements of the product (`ui`, D-8250): its screens, forms, actions, live
    /// bindings and error messages, by id. Installed into `tenant-schema.ui` with the rest of the
    /// product, last, since they name its views and operations.
    pub ui: BTreeMap<String, UiElement>,
    /// What the bundle fills that the kernel cannot install yet, by section name.
    pub unbuilt: Vec<Unbuilt>,
}

/// One entity a product owns, as the tenant catalogue records it (`entity-ref`). `kind` is a value
/// of `entity-kind`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OwnedEntity {
    pub kind: String,
    pub id: String,
}

impl OwnedEntity {
    fn of(kind: &str, id: impl ToString) -> Self {
        OwnedEntity {
            kind: kind.to_string(),
            id: id.to_string(),
        }
    }

    /// The kernel's reference to the entity.
    pub fn entity_ref(&self) -> Result<EntityRef, String> {
        let kind: EntityKind = serde_json::from_value(Json::String(self.kind.clone()))
            .map_err(|_| format!("{:?} is not a kind of entity", self.kind))?;
        Ok(EntityRef {
            kind,
            id: self.id.clone(),
        })
    }

    /// Whether the tenant schema holds the entity. A kind the kernel does not hold is not.
    fn exists_in(&self, schema: &TenantSchema) -> bool {
        self.entity_ref()
            .ok()
            .and_then(|entity| strata_kernel::products::exists(schema, &entity))
            .unwrap_or(false)
    }

    fn key(&self) -> (String, String) {
        (self.kind.clone(), self.id.clone())
    }
}

/// An installed product, as the tenant catalogue holds it (`installed-product`): its version, the
/// platform range it declared and every entity it owns, which is what a removal finds (D-6582).
/// `schema_version` is the schema version the install produced, which an install of the same
/// version again returns (D-6583).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledRecord {
    pub product: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<PlatformRange>,
    pub owns: Vec<OwnedEntity>,
    #[serde(
        default,
        rename = "schema-version",
        skip_serializing_if = "Option::is_none"
    )]
    pub schema_version: Option<u64>,
}

impl InstalledRecord {
    /// The JSON text of the record.
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| format!("installed product {}: {e}", self.product))
    }

    /// Reads the text written by [`InstalledRecord::to_json`].
    pub fn from_json(text: &str) -> Result<InstalledRecord, String> {
        serde_json::from_str(text).map_err(|e| format!("installed product: {e}"))
    }

    /// The record the tenant schema holds for `product`, if it is installed.
    pub fn of(schema: &TenantSchema, product: &str) -> Option<InstalledRecord> {
        let id = ProductId::new(product).ok()?;
        let held = strata_kernel::products::get(schema, &id)?;
        Some(InstalledRecord {
            product: held.product.to_string(),
            version: held.version.clone(),
            platform: held.platform.as_ref().map(|p| PlatformRange {
                min: p.min.clone(),
                below: p.below.clone(),
            }),
            owns: held
                .owns
                .iter()
                .map(|e| OwnedEntity {
                    kind: serde_json::to_value(e.kind)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_default(),
                    id: e.id.clone(),
                })
                .collect(),
            schema_version: held.schema_version.map(|v| v.0),
        })
    }

    /// Every product the tenant schema holds a record of, in product-id order.
    pub fn all(schema: &TenantSchema) -> Vec<InstalledRecord> {
        strata_kernel::products::list(schema)
            .into_iter()
            .filter_map(|held| InstalledRecord::of(schema, held.product.as_str()))
            .collect()
    }

    /// The record as the kernel holds it.
    pub fn to_kernel(&self) -> Result<InstalledProduct, String> {
        let owns = self
            .owns
            .iter()
            .map(OwnedEntity::entity_ref)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(InstalledProduct {
            product: ProductId::new(self.product.as_str())
                .map_err(|e| format!("product {:?}: {e}", self.product))?,
            version: self.version.clone(),
            platform: self.platform.as_ref().map(|p| PlatformCompat {
                min: p.min.clone(),
                below: p.below.clone(),
            }),
            owns,
            schema_version: self.schema_version.map(SchemaVersion),
        })
    }
}

/// Whether `id` is a product id: lower-case words joined by hyphens, at most 64 bytes.
fn is_product_id(id: &str) -> bool {
    id.len() <= 64
        && id.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

// ---- reading -----------------------------------------------------------------------------------

/// The keys of every object in `value` as the generated types write them: a hyphen becomes an
/// underscore (`run-as` is `run_as`). A product file writes the names of the specification; the
/// kernel's types read them in snake case. The keys of a webhook body are the product's own
/// strings and are left as they are.
fn snake_keys(value: &mut Json) {
    match value {
        Json::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            let mut renamed = Map::new();
            for key in keys {
                let mut inner = map.remove(&key).unwrap_or(Json::Null);
                // The keys of a webhook body are the product's own strings.
                let own_keys = key == "body" && inner.is_object();
                if own_keys {
                    if let Json::Object(own) = &mut inner {
                        own.values_mut().for_each(snake_keys);
                    }
                } else {
                    snake_keys(&mut inner);
                }
                renamed.insert(key.replace('-', "_"), inner);
            }
            *map = renamed;
        }
        Json::Array(items) => items.iter_mut().for_each(snake_keys),
        _ => {}
    }
}

/// The text of an expression (the authoring syntax of the tenant rule language) as the AST a
/// kernel value holds, in place; a value that is not text is the AST already and is left alone.
fn text_to_ast(slot: &mut Json, at: &str) -> Result<(), String> {
    let ast = match &*slot {
        Json::String(text) => {
            let expr = parse(text).map_err(|e| format!("{at}: {e}"))?;
            Some(serde_json::to_value(&expr).map_err(|e| format!("{at}: {e}"))?)
        }
        _ => None,
    };
    if let Some(ast) = ast {
        *slot = ast;
    }
    Ok(())
}

/// The expression at `key` of an object, if it has one.
fn expr_at(map: &mut Map<String, Json>, key: &str, at: &str) -> Result<(), String> {
    match map.get_mut(key) {
        Some(slot) => text_to_ast(slot, &format!("{at}.{key}")),
        None => Ok(()),
    }
}

/// Each expression of the map at `key` of an object, if it has one.
fn exprs_at(map: &mut Map<String, Json>, key: &str, at: &str) -> Result<(), String> {
    if let Some(Json::Object(entries)) = map.get_mut(key) {
        for (name, slot) in entries.iter_mut() {
            text_to_ast(slot, &format!("{at}.{key}.{name}"))?;
        }
    }
    Ok(())
}

/// The expressions of one object type: the default and read rule of each field, the check of each
/// constraint.
fn expressions_of_object(def: &mut Json, at: &str) -> Result<(), String> {
    if let Some(fields) = def.get_mut("fields").and_then(Json::as_object_mut) {
        for (id, field) in fields.iter_mut() {
            for key in ["default", "read"] {
                if let Some(slot) = field.get_mut(key) {
                    text_to_ast(slot, &format!("{at}.{id}.{key}"))?;
                }
            }
        }
    }
    if let Some(constraints) = def.get_mut("constraints").and_then(Json::as_object_mut) {
        for (id, constraint) in constraints.iter_mut() {
            if let Some(slot) = constraint.get_mut("expr") {
                text_to_ast(slot, &format!("{at}.{id}.expr"))?;
            }
        }
    }
    Ok(())
}

/// The condition of a policy.
fn expressions_of_policy(def: &mut Json, at: &str) -> Result<(), String> {
    match def.get_mut("condition") {
        Some(slot) => text_to_ast(slot, &format!("{at}.condition")),
        None => Ok(()),
    }
}

/// The filter of a query.
fn expressions_of_query(def: &mut Json, at: &str) -> Result<(), String> {
    match def.get_mut("filter") {
        Some(slot) => text_to_ast(slot, &format!("{at}.filter")),
        None => Ok(()),
    }
}

/// The guard and the assignments of each transition of a workflow.
fn expressions_of_workflow(def: &mut Json, at: &str) -> Result<(), String> {
    let Some(transitions) = def.get_mut("transitions").and_then(Json::as_array_mut) else {
        return Ok(());
    };
    for transition in transitions {
        let name = transition
            .get("name")
            .and_then(Json::as_str)
            .unwrap_or("?")
            .to_string();
        if let Some(slot) = transition.get_mut("guard") {
            text_to_ast(slot, &format!("{at}.{name}.guard"))?;
        }
        if let Some(set) = transition.get_mut("set").and_then(Json::as_object_mut) {
            for (field, slot) in set.iter_mut() {
                text_to_ast(slot, &format!("{at}.{name}.set.{field}"))?;
            }
        }
    }
    Ok(())
}

/// The condition of a trigger.
fn expressions_of_trigger(def: &mut Json, at: &str) -> Result<(), String> {
    match def.get_mut("condition") {
        Some(slot) => text_to_ast(slot, &format!("{at}.condition")),
        None => Ok(()),
    }
}

/// The expressions of a step action of an automation or of a tenant operation: the members of the
/// variant that are expressions (`scalars`) and those that are maps of expressions (`maps`). A
/// variant without fields is its tag alone. The owner of the posting of an operation is a union
/// (`Invoker`, `Pool`, `Principal`), that of an automation an expression.
fn expressions_of_action(action: &mut Json, at: &str, operation: bool) -> Result<(), String> {
    let Json::Object(tagged) = action else {
        return Ok(());
    };
    for (tag, body) in tagged.iter_mut() {
        let Json::Object(fields) = body else {
            continue;
        };
        // The steps of a tenant operation that find a record and branch on it (WG-5596): the
        // predicate of a find, the condition of a branch test, and the steps of each side, which
        // are steps of the same kind and are converted in turn.
        if operation {
            match tag.as_str() {
                "FindRecord" => {
                    expr_at(fields, "predicate", at)?;
                    continue;
                }
                "Branch" => {
                    if let Some(Json::Object(test)) = fields.get_mut("test") {
                        if let Some(Json::Object(condition)) = test.get_mut("Condition") {
                            expr_at(condition, "expr", &format!("{at}.test"))?;
                        }
                    }
                    for side in ["matched", "unmatched"] {
                        if let Some(Json::Array(steps)) = fields.get_mut(side) {
                            for (index, step) in steps.iter_mut().enumerate() {
                                expressions_of_action(step, &format!("{at}.{side} {index}"), true)?;
                            }
                        }
                    }
                    continue;
                }
                _ => {}
            }
        }
        let (scalars, maps): (&[&str], &[&str]) = match tag.as_str() {
            "CreateRecord" => (&[], &["fields"]),
            "UpdateRecord" => (&["record"], &["fields"]),
            "DeleteRecord" => (&["record"], &[]),
            "PostLedger" => (&["owner", "scope", "amount", "reference"], &[]),
            "ReverseLedger" => (&["reference"], &[]),
            // The value a setting takes; its setting and level are names, not expressions (WG-5606).
            "WriteSetting" => (&["value"], &[]),
            "SendMail" => (&["to", "address"], &[]),
            "CreateNotice" => (&["to", "record"], &[]),
            "CallWebhook" => (&[], &["body"]),
            "WaitUntil" => (&["at"], &[]),
            "Branch" => (&["condition", "record"], &[]),
            _ => (&[], &[]),
        };
        for key in scalars {
            if operation && *key == "owner" {
                // `{"Principal": {"principal": <expression>}}`; the other variants hold none.
                if let Some(Json::Object(owner)) = fields.get_mut("owner") {
                    if let Some(Json::Object(principal)) = owner.get_mut("Principal") {
                        expr_at(principal, "principal", &format!("{at}.owner"))?;
                    }
                }
                continue;
            }
            expr_at(fields, key, at)?;
        }
        for key in maps {
            exprs_at(fields, key, at)?;
        }
    }
    Ok(())
}

/// The run key of an automation and the expressions of each of its steps.
fn expressions_of_automation(def: &mut Json, at: &str) -> Result<(), String> {
    if let Some(slot) = def.get_mut("run_key") {
        text_to_ast(slot, &format!("{at}.run_key"))?;
    }
    if let Some(Json::Object(steps)) = def.get_mut("steps") {
        for (name, step) in steps.iter_mut() {
            if let Some(action) = step.get_mut("action") {
                expressions_of_action(action, &format!("{at}.{name}"), false)?;
            }
        }
    }
    Ok(())
}

/// The expressions of the conditions and of the steps of a tenant operation.
fn expressions_of_operation(def: &mut Json, at: &str) -> Result<(), String> {
    if let Some(Json::Array(conditions)) = def.get_mut("conditions") {
        for (index, condition) in conditions.iter_mut().enumerate() {
            let id = condition
                .get("id")
                .and_then(Json::as_str)
                .map_or_else(|| index.to_string(), str::to_string);
            if let Some(slot) = condition.get_mut("expr") {
                text_to_ast(slot, &format!("{at}.{id}.expr"))?;
            }
        }
    }
    if let Some(Json::Array(steps)) = def.get_mut("steps") {
        for (index, step) in steps.iter_mut().enumerate() {
            expressions_of_action(step, &format!("{at}.step {index}"), true)?;
        }
    }
    Ok(())
}

/// The condition of a hold rule, in the authoring syntax.
fn expressions_of_hold(def: &mut Json, at: &str) -> Result<(), String> {
    if let Some(slot) = def.get_mut("when") {
        text_to_ast(slot, &format!("{at}.when"))?;
    }
    Ok(())
}

/// A section that is a map from id to definition: each definition has its expression texts read
/// as ASTs by `convert`, must name itself with its key when it names itself at all, and is then
/// read as a `V`. A section that is left out or null is empty.
fn parse_map<K, V>(
    content: &mut Map<String, Json>,
    name: &str,
    convert: &dyn Fn(&mut Json, &str) -> Result<(), String>,
) -> Result<BTreeMap<K, V>, BundleError>
where
    K: serde::de::DeserializeOwned + Ord,
    V: serde::de::DeserializeOwned,
{
    let bad =
        |message: String| BundleError::invalid_bundle(format!("tenant bundle: {name}: {message}"));
    let mut value = match content.remove(name) {
        None | Some(Json::Null) => return Ok(BTreeMap::new()),
        Some(value) => value,
    };
    let Json::Object(entries) = &mut value else {
        return Err(bad("must be a map from an id to its definition".to_string()));
    };
    for (id, def) in entries.iter_mut() {
        snake_keys(def);
        convert(def, id).map_err(|e| bad(format!("{id}: {e}")))?;
        if let Some(Json::String(own)) = def.get("id") {
            if own != id {
                return Err(bad(format!(
                    "{id} is keyed {id:?} but names itself {own:?}"
                )));
            }
        }
    }
    serde_json::from_value(value).map_err(|e| bad(e.to_string()))
}

/// A section that is a list of definitions, each named by its member `id_member` (`id`,
/// `template`, `name`): read as a `V` after `convert` reads its expression texts, and keyed by
/// `key_of`. Two definitions with one key are refused. A section that is left out or null is
/// empty.
fn parse_list<K, V>(
    content: &mut Map<String, Json>,
    name: &str,
    id_member: &str,
    convert: &dyn Fn(&mut Json, &str) -> Result<(), String>,
    key_of: &dyn Fn(&V) -> K,
) -> Result<BTreeMap<K, V>, BundleError>
where
    K: Ord + std::fmt::Display,
    V: serde::de::DeserializeOwned,
{
    let bad =
        |message: String| BundleError::invalid_bundle(format!("tenant bundle: {name}: {message}"));
    let mut value = match content.remove(name) {
        None | Some(Json::Null) => return Ok(BTreeMap::new()),
        Some(value) => value,
    };
    let Json::Array(items) = &mut value else {
        return Err(bad("must be a list of definitions".to_string()));
    };
    let mut out = BTreeMap::new();
    for (index, item) in items.iter_mut().enumerate() {
        let id = item
            .get(id_member)
            .and_then(Json::as_str)
            .map_or_else(|| format!("#{index}"), str::to_string);
        snake_keys(item);
        convert(item, &id).map_err(|e| bad(format!("{id}: {e}")))?;
        let parsed: V =
            serde_json::from_value(item.clone()).map_err(|e| bad(format!("{id}: {e}")))?;
        let key = key_of(&parsed);
        if out.contains_key(&key) {
            return Err(bad(format!("{key} is declared twice")));
        }
        out.insert(key, parsed);
    }
    Ok(out)
}

/// The triggers, automations and tenant operations of a bundle.
pub(crate) type Definitions = (
    BTreeMap<TriggerId, TriggerDef>,
    BTreeMap<WorkflowId, AutomationDef>,
    BTreeMap<TenantOperationId, TenantOperationDef>,
);

/// Reads the sections `triggers`, `automations` and `operations` of `content` (lists of
/// `trigger-def`, `automation-def` and `tenant-operation-def`, D-6580), with their expressions in
/// the authoring syntax. The sections are taken out of `content`.
pub(crate) fn read_definitions(
    content: &mut Map<String, Json>,
) -> Result<Definitions, BundleError> {
    let triggers = parse_list(
        content,
        "triggers",
        "id",
        &expressions_of_trigger,
        &|t: &TriggerDef| t.id.clone(),
    )?;
    let automations = parse_list(
        content,
        "automations",
        "id",
        &expressions_of_automation,
        &|a: &AutomationDef| a.id.clone(),
    )?;
    let operations = parse_list(
        content,
        "operations",
        "id",
        &expressions_of_operation,
        &|o: &TenantOperationDef| o.id.clone(),
    )?;
    Ok((triggers, automations, operations))
}

/// One permission of a role as a bundle writes it: a capability name or `{object, actions}`
/// (untagged), or a `Permission` as the kernel writes it (`{"Capability": ...}`).
fn permission_from_json(item: &Json) -> Result<Permission, String> {
    match serde_json::from_value::<crate::bundle::PermissionSpec>(item.clone()) {
        Ok(spec) => Ok(spec.permission()),
        Err(_) => serde_json::from_value::<Permission>(item.clone()).map_err(|e| e.to_string()),
    }
}

fn parse_role_permissions(
    content: &mut Map<String, Json>,
) -> Result<BTreeMap<RoleId, Vec<Permission>>, BundleError> {
    let bad = |message: String| {
        BundleError::invalid_bundle(format!("tenant bundle: role-permissions: {message}"))
    };
    let value = match content.remove("role-permissions") {
        None | Some(Json::Null) => return Ok(BTreeMap::new()),
        Some(value) => value,
    };
    let Json::Object(entries) = value else {
        return Err(bad(
            "must be a map from a role id to its permissions".to_string()
        ));
    };
    let mut out = BTreeMap::new();
    for (role, list) in entries {
        let Json::Array(items) = list else {
            return Err(bad(format!("{role}: must be a list of permissions")));
        };
        let id = RoleId::new(role.as_str()).map_err(|e| bad(format!("{role}: {e}")))?;
        let permissions = items
            .iter()
            .map(permission_from_json)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| bad(format!("{role}: {e}")))?;
        out.insert(id, permissions);
    }
    Ok(out)
}

/// The setting definitions of a bundle, the section `settings` (SET-001, WG-5601). A product and a
/// tenant declare them in the same shape and go through the same checks; the section is taken out
/// of `content`.
pub(crate) fn read_settings(
    content: &mut Map<String, Json>,
) -> Result<BTreeMap<SettingId, SettingDef>, BundleError> {
    let none = |_: &mut Json, _: &str| -> Result<(), String> { Ok(()) };
    parse_list(content, "settings", "id", &none, &|d: &SettingDef| {
        d.id.clone()
    })
}

/// The declarations of a bundle the kernel takes as one schema change apiece: the sections
/// `capabilities`, `settings`, `ledger-units`, `ledger-pools` and `ledger-limits` (D-6590,
/// D-5046, D-5000), each a list. The sections are taken out of `content`.
type Declarations = (
    BTreeMap<CapabilityName, CapabilityDef>,
    BTreeMap<SettingId, SettingDef>,
    BTreeMap<LedgerUnit, LedgerUnitDecl>,
    BTreeMap<LedgerPoolName, LedgerPoolDecl>,
    BTreeMap<String, LedgerLimit>,
);

fn read_declarations(content: &mut Map<String, Json>) -> Result<Declarations, BundleError> {
    let none = |_: &mut Json, _: &str| -> Result<(), String> { Ok(()) };
    let capabilities = parse_list(
        content,
        "capabilities",
        "id",
        &none,
        &|c: &CapabilityDef| c.id.clone(),
    )?;
    let settings = read_settings(content)?;
    let ledger_units = parse_list(
        content,
        "ledger-units",
        "unit",
        &none,
        &|u: &LedgerUnitDecl| u.unit.clone(),
    )?;
    let ledger_pools = parse_list(
        content,
        "ledger-pools",
        "pool",
        &none,
        &|p: &LedgerPoolDecl| p.pool.clone(),
    )?;
    let ledger_limits = parse_list(
        content,
        "ledger-limits",
        "unit",
        &none,
        &|l: &LedgerLimit| strata_kernel::products::ledger_limit_id(&l.unit, &l.owner, &l.scope),
    )?;
    Ok((
        capabilities,
        settings,
        ledger_units,
        ledger_pools,
        ledger_limits,
    ))
}

/// The prefix of the kind of an event a tenant declares (SCH-1576).
const DECLARED_KIND_PREFIX: &str = "app.";

/// Reads the section `event-kinds` of `content`: a list of `event-kind-decl` (`kind`, `fields`,
/// `readers`, SCH-1576, INT-3203). Each kind begins with `app.`, has at most 16 fields and is
/// declared once. The section is taken out of `content`.
fn read_event_kinds(content: &mut Map<String, Json>) -> Result<BTreeSet<String>, BundleError> {
    let bad = |message: String| {
        BundleError::invalid_bundle(format!("tenant bundle: event-kinds: {message}"))
    };
    let items = match content.remove("event-kinds") {
        None | Some(Json::Null) => return Ok(BTreeSet::new()),
        Some(Json::Array(items)) => items,
        Some(_) => return Err(bad("must be a list of event kind declarations".to_string())),
    };
    let mut kinds = BTreeSet::new();
    for (index, item) in items.iter().enumerate() {
        let kind = item
            .get("kind")
            .and_then(Json::as_str)
            .ok_or_else(|| bad(format!("#{index} has no kind")))?;
        if !kind.starts_with(DECLARED_KIND_PREFIX) || kind.len() == DECLARED_KIND_PREFIX.len() {
            return Err(bad(format!(
                "{kind} must begin with {DECLARED_KIND_PREFIX} and a name"
            )));
        }
        let fields = item
            .get("fields")
            .and_then(Json::as_array)
            .map_or(0, Vec::len);
        if fields > 16 {
            return Err(bad(format!("{kind} has {fields} fields, at most 16")));
        }
        if !kinds.insert(kind.to_string()) {
            return Err(bad(format!("{kind} is declared twice")));
        }
    }
    Ok(kinds)
}

impl ProductBundle {
    /// Reads a `tenant-bundle` document: its members, and each section the kernel installs as
    /// kernel values (expressions written as text are read with the tenant rule language, and the
    /// names of the specification are read as the generated types write them). A section the
    /// kernel cannot install yet is recorded in [`ProductBundle::unbuilt`], not read. Refuses a
    /// malformed document, an unknown member or section, a definition whose key is not its id, an
    /// expression that does not parse, and permissions for a role the bundle does not declare.
    pub fn from_json(text: &str) -> Result<ProductBundle, BundleError> {
        let bad =
            |message: String| BundleError::invalid_bundle(format!("tenant bundle: {message}"));
        let doc: Json = serde_json::from_str(text).map_err(|e| bad(e.to_string()))?;
        let Json::Object(mut root) = doc else {
            return Err(bad("the document must be an object".to_string()));
        };
        for key in root.keys() {
            if !BUNDLE_MEMBERS.contains(&key.as_str()) {
                return Err(bad(format!(
                    "unknown member {key:?}; a bundle has {}",
                    BUNDLE_MEMBERS.join(", ")
                )));
            }
        }
        let required = |root: &Map<String, Json>, key: &str| match root.get(key) {
            Some(Json::String(text)) if !text.is_empty() => Ok(text.clone()),
            _ => Err(bad(format!("{key} must be a non-empty string"))),
        };
        let name = required(&root, "name")?;
        let version = required(&root, "version")?;
        let product = match root.get("product") {
            None | Some(Json::Null) => None,
            Some(Json::String(id)) if is_product_id(id) => Some(id.clone()),
            Some(other) => {
                return Err(bad(format!(
                    "product must be lower-case words joined by hyphens, not {other}"
                )))
            }
        };
        let platform = match root.remove("platform") {
            None | Some(Json::Null) => None,
            Some(value) => Some(
                serde_json::from_value::<PlatformRange>(value)
                    .map_err(|e| bad(format!("platform: {e}")))?,
            ),
        };
        let Some(Json::Object(mut content)) = root.remove("content") else {
            return Err(bad("content must be an object of sections".to_string()));
        };
        for key in content.keys() {
            let known = BUILT_SECTIONS.contains(&key.as_str());
            if !known {
                return Err(bad(format!(
                    "content has the section {key:?}, which is not a section of a bundle"
                )));
            }
        }
        let objects: BTreeMap<ObjectTypeId, ObjectTypeDef> =
            parse_map(&mut content, "objects", &expressions_of_object)?;
        let roles: BTreeMap<RoleId, RoleDef> = parse_map(&mut content, "roles", &|_, _| Ok(()))?;
        let role_permissions = parse_role_permissions(&mut content)?;
        let policies: BTreeMap<PolicyId, PolicyDef> =
            parse_map(&mut content, "policies", &expressions_of_policy)?;
        let queries: BTreeMap<QueryId, QueryDef> =
            parse_map(&mut content, "queries", &expressions_of_query)?;
        let views: BTreeMap<ViewId, ViewDef> = parse_map(&mut content, "views", &|_, _| Ok(()))?;
        let workflows: BTreeMap<WorkflowId, WorkflowDef> =
            parse_map(&mut content, "workflows", &expressions_of_workflow)?;
        let mail_templates: BTreeMap<TemplateId, MailTemplateDecl> = parse_list(
            &mut content,
            "mail-templates",
            "template",
            &|_, _| Ok(()),
            &|t: &MailTemplateDecl| t.template.clone(),
        )?;
        let notice_templates: BTreeMap<TemplateId, NoticeTemplateDecl> = parse_list(
            &mut content,
            "notice-templates",
            "template",
            &|_, _| Ok(()),
            &|t: &NoticeTemplateDecl| t.template.clone(),
        )?;
        let tokens: BTreeMap<String, TokenDecl> = parse_list(
            &mut content,
            "tokens",
            "name",
            &|_, _| Ok(()),
            &|t: &TokenDecl| t.name.clone(),
        )?;
        let (triggers, automations, operations) = read_definitions(&mut content)?;
        let holds: BTreeMap<HoldRuleId, HoldRule> = parse_list(
            &mut content,
            "holds",
            "id",
            &expressions_of_hold,
            &|rule: &HoldRule| rule.id.clone(),
        )?;
        for role in role_permissions.keys() {
            if !roles.contains_key(role) {
                return Err(bad(format!(
                    "role-permissions gives permissions to {role}, which the bundle does not declare in roles"
                )));
            }
        }
        let (capabilities, settings, ledger_units, ledger_pools, ledger_limits) =
            read_declarations(&mut content)?;
        let segments: BTreeMap<SegmentKey, SegmentDecl> =
            parse_map(&mut content, "segments", &|def, _| {
                // The predicate is text of the tenant rule language: a bundle whose predicate does
                // not parse is refused when it is read (SEG-003).
                match def.get("predicate").and_then(Json::as_str) {
                    Some(text) => parse(text)
                        .map(|_| ())
                        .map_err(|e| format!("predicate: {e}")),
                    None => Ok(()),
                }
            })?;
        let event_kinds = read_event_kinds(&mut content)?;
        let ui: BTreeMap<String, UiElement> = parse_map(&mut content, "ui", &|_, _| Ok(()))?;
        let unbuilt = if event_kinds.is_empty() {
            Vec::new()
        } else {
            vec![Unbuilt {
                section: "event-kinds".to_string(),
                entries: event_kinds.len(),
            }]
        };
        Ok(ProductBundle {
            name,
            version,
            product,
            platform,
            objects,
            roles,
            role_permissions,
            policies,
            queries,
            views,
            workflows,
            mail_templates,
            notice_templates,
            tokens,
            triggers,
            automations,
            operations,
            holds,
            capabilities,
            settings,
            ledger_units,
            ledger_pools,
            ledger_limits,
            segments,
            event_kinds,
            ui,
            unbuilt,
        })
    }

    /// The namespace of the entities of the product: its id with each hyphen replaced by an
    /// underscore, followed by two underscores (D-6582).
    pub fn namespace(&self) -> Option<String> {
        self.product
            .as_ref()
            .map(|product| format!("{}__", product.replace('-', "_")))
    }

    /// Every entity the bundle installs, in install order: capabilities, settings, ledger units,
    /// pools and limits (which roles, triggers and steps name), roles, object types, policies,
    /// workflows, queries, views, mail templates, notice templates, tokens, triggers, automations,
    /// operations, hold rules (which name operations, roles and settings), segments and interface
    /// elements (which name views, operations and each other), each by id.
    pub fn entities(&self) -> Vec<OwnedEntity> {
        let mut out = Vec::new();
        out.extend(
            self.capabilities
                .keys()
                .map(|id| OwnedEntity::of("capability", id)),
        );
        out.extend(
            self.settings
                .keys()
                .map(|id| OwnedEntity::of("setting", id)),
        );
        out.extend(
            self.ledger_units
                .keys()
                .map(|id| OwnedEntity::of("ledger-unit", id)),
        );
        out.extend(
            self.ledger_pools
                .keys()
                .map(|id| OwnedEntity::of("ledger-pool", id)),
        );
        out.extend(
            self.ledger_limits
                .keys()
                .map(|id| OwnedEntity::of("ledger-limit", id)),
        );
        out.extend(self.roles.keys().map(|id| OwnedEntity::of("role", id)));
        out.extend(self.objects.keys().map(|id| OwnedEntity::of("object", id)));
        out.extend(self.policies.keys().map(|id| OwnedEntity::of("policy", id)));
        out.extend(
            self.workflows
                .keys()
                .map(|id| OwnedEntity::of("workflow", id)),
        );
        out.extend(self.queries.keys().map(|id| OwnedEntity::of("query", id)));
        out.extend(self.views.keys().map(|id| OwnedEntity::of("view", id)));
        out.extend(
            self.mail_templates
                .keys()
                .map(|id| OwnedEntity::of("mail-template", id)),
        );
        out.extend(
            self.notice_templates
                .keys()
                .map(|id| OwnedEntity::of("notice-template", id)),
        );
        out.extend(self.tokens.keys().map(|id| OwnedEntity::of("token", id)));
        out.extend(
            self.triggers
                .keys()
                .map(|id| OwnedEntity::of("trigger", id)),
        );
        out.extend(
            self.automations
                .keys()
                .map(|id| OwnedEntity::of("automation", id)),
        );
        out.extend(
            self.operations
                .keys()
                .map(|id| OwnedEntity::of("operation", id)),
        );
        out.extend(self.holds.keys().map(|id| OwnedEntity::of("hold-rule", id)));
        out.extend(
            self.segments
                .keys()
                .map(|key| OwnedEntity::of("segment", &key.0)),
        );
        out.extend(self.ui.keys().map(|id| OwnedEntity::of("ui-element", id)));
        out
    }

    /// The permissions the bundle gives a role; none when it gives none.
    fn permissions_of(&self, role: &RoleId) -> Vec<Permission> {
        self.role_permissions.get(role).cloned().unwrap_or_default()
    }

    /// The schema changes that define one entity of the bundle (define-or-replace).
    fn definition_changes(&self, entity: &OwnedEntity) -> Vec<SchemaChange> {
        let id = entity.id.as_str();
        let mut changes = Vec::new();
        match entity.kind.as_str() {
            "role" => {
                for (key, role) in self.roles.iter().filter(|(k, _)| k.to_string() == id) {
                    changes.push(SchemaChange::DefineRole(role.clone()));
                    let permissions = self.permissions_of(key);
                    if !permissions.is_empty() {
                        changes.push(SchemaChange::DefineRolePermissions {
                            role: key.clone(),
                            permissions,
                        });
                    }
                }
            }
            "object" => {
                for (key, object) in self.objects.iter().filter(|(k, _)| k.to_string() == id) {
                    changes.push(SchemaChange::DefineObject {
                        id: key.clone(),
                        label: object.label.clone(),
                    });
                    for field in object.fields.values() {
                        changes.push(SchemaChange::DefineField {
                            object: key.clone(),
                            field: field.clone(),
                        });
                    }
                    for constraint in object.constraints.values() {
                        changes.push(SchemaChange::DefineConstraint {
                            object: key.clone(),
                            constraint: constraint.clone(),
                        });
                    }
                    for unique in object.unique.iter().flat_map(BTreeMap::values) {
                        changes.push(SchemaChange::DefineUnique {
                            object: key.clone(),
                            unique: unique.clone(),
                        });
                    }
                    if let Some(retention) = &object.retention {
                        changes.push(SchemaChange::SetRetention {
                            object: key.clone(),
                            retention: retention.clone(),
                        });
                    }
                    if let Some(publication) = &object.publication {
                        changes.push(SchemaChange::SetPublication {
                            object: key.clone(),
                            publication: publication.clone(),
                        });
                    }
                    if let Some(field) = &object.language_field {
                        changes.push(SchemaChange::SetLanguageField {
                            object: key.clone(),
                            field: field.clone(),
                        });
                    }
                    if let Some(field) = &object.translation_group {
                        changes.push(SchemaChange::SetTranslationGroup {
                            object: key.clone(),
                            field: field.clone(),
                        });
                    }
                    if let Some(fields) = &object.searchable {
                        changes.push(SchemaChange::SetSearchable {
                            object: key.clone(),
                            fields: fields.clone(),
                        });
                    }
                }
            }
            "policy" => changes.extend(
                self.policies
                    .iter()
                    .filter(|(k, _)| k.to_string() == id)
                    .map(|(_, v)| SchemaChange::DefinePolicy(v.clone())),
            ),
            "workflow" => changes.extend(
                self.workflows
                    .iter()
                    .filter(|(k, _)| k.to_string() == id)
                    .map(|(_, v)| SchemaChange::DefineWorkflow(v.clone())),
            ),
            "query" => changes.extend(
                self.queries
                    .iter()
                    .filter(|(k, _)| k.to_string() == id)
                    .map(|(_, v)| SchemaChange::DefineQuery(v.clone())),
            ),
            "view" => changes.extend(
                self.views
                    .iter()
                    .filter(|(k, _)| k.to_string() == id)
                    .map(|(_, v)| SchemaChange::DefineView(v.clone())),
            ),
            "mail-template" => changes.extend(
                self.mail_templates
                    .iter()
                    .filter(|(k, _)| k.to_string() == id)
                    .map(|(_, v)| SchemaChange::DefineMailTemplate(v.clone())),
            ),
            "notice-template" => changes.extend(
                self.notice_templates
                    .iter()
                    .filter(|(k, _)| k.to_string() == id)
                    .map(|(_, v)| SchemaChange::DefineNoticeTemplate(v.clone())),
            ),
            "token" => changes.extend(
                self.tokens
                    .iter()
                    .filter(|(k, _)| k.as_str() == id)
                    .map(|(_, v)| SchemaChange::DefineToken(v.clone())),
            ),
            "trigger" => changes.extend(
                self.triggers
                    .iter()
                    .filter(|(k, _)| k.to_string() == id)
                    .map(|(_, v)| SchemaChange::DefineTrigger(v.clone())),
            ),
            "automation" => changes.extend(
                self.automations
                    .iter()
                    .filter(|(k, _)| k.to_string() == id)
                    .map(|(_, v)| SchemaChange::DefineAutomation(v.clone())),
            ),
            "operation" => changes.extend(
                self.operations
                    .iter()
                    .filter(|(k, _)| k.to_string() == id)
                    .map(|(_, v)| SchemaChange::DefineOperation(v.clone())),
            ),
            "hold-rule" => changes.extend(
                self.holds
                    .iter()
                    .filter(|(k, _)| k.to_string() == id)
                    .map(|(_, v)| SchemaChange::DefineHoldRule(v.clone())),
            ),
            "capability" => changes.extend(
                self.capabilities
                    .iter()
                    .filter(|(k, _)| k.to_string() == id)
                    .map(|(_, v)| SchemaChange::DefineCapability(v.clone())),
            ),
            "setting" => changes.extend(
                self.settings
                    .iter()
                    .filter(|(k, _)| k.to_string() == id)
                    .map(|(_, v)| SchemaChange::DefineSetting(v.clone())),
            ),
            "ledger-unit" => changes.extend(
                self.ledger_units
                    .iter()
                    .filter(|(k, _)| k.to_string() == id)
                    .map(|(_, v)| SchemaChange::DeclareLedgerUnit(v.clone())),
            ),
            "ledger-pool" => changes.extend(
                self.ledger_pools
                    .iter()
                    .filter(|(k, _)| k.to_string() == id)
                    .map(|(_, v)| SchemaChange::DeclareLedgerPool(v.clone())),
            ),
            "ledger-limit" => changes.extend(
                self.ledger_limits
                    .iter()
                    .filter(|(k, _)| k.as_str() == id)
                    .map(|(_, v)| SchemaChange::SetLedgerLimit(v.clone())),
            ),
            "ui-element" => {
                changes.extend(
                    self.ui
                        .get(id)
                        .map(|element| SchemaChange::DefineUiElement {
                            id: id.to_string(),
                            element: element.clone(),
                        }),
                )
            }
            _ => {}
        }
        changes
    }

    /// How the tenant schema holds one entity of the bundle: `None` when it has none by that id,
    /// `Some(true)` when it holds exactly what the bundle declares, `Some(false)` when it holds
    /// something else.
    fn same_in(&self, entity: &OwnedEntity, schema: &TenantSchema) -> Option<bool> {
        let id = entity.id.as_str();
        fn same<T: PartialEq>(held: Option<&T>, declared: &T) -> Option<bool> {
            held.map(|held| held == declared)
        }
        match entity.kind.as_str() {
            "role" => self
                .roles
                .iter()
                .find(|(k, _)| k.to_string() == id)
                .and_then(|(key, role)| {
                    let held: Vec<Permission> = schema
                        .role_permissions
                        .as_ref()
                        .and_then(|table| table.get(key))
                        .cloned()
                        .unwrap_or_default();
                    same(schema.roles.get(key), role)
                        .map(|same| same && held == self.permissions_of(key))
                }),
            "object" => self
                .objects
                .iter()
                .find(|(k, _)| k.to_string() == id)
                .and_then(|(key, def)| same(schema.objects.get(key), def)),
            "policy" => self
                .policies
                .iter()
                .find(|(k, _)| k.to_string() == id)
                .and_then(|(key, def)| same(schema.policies.get(key), def)),
            "workflow" => self
                .workflows
                .iter()
                .find(|(k, _)| k.to_string() == id)
                .and_then(|(key, def)| same(schema.workflows.get(key), def)),
            "query" => self
                .queries
                .iter()
                .find(|(k, _)| k.to_string() == id)
                .and_then(|(key, def)| same(schema.queries.get(key), def)),
            "view" => self
                .views
                .iter()
                .find(|(k, _)| k.to_string() == id)
                .and_then(|(key, def)| same(schema.views.get(key), def)),
            "mail-template" => self
                .mail_templates
                .iter()
                .find(|(k, _)| k.to_string() == id)
                .and_then(|(key, def)| {
                    same(schema.mail_templates.as_ref().and_then(|t| t.get(key)), def)
                }),
            "notice-template" => self
                .notice_templates
                .iter()
                .find(|(k, _)| k.to_string() == id)
                .and_then(|(key, def)| {
                    same(
                        schema.notice_templates.as_ref().and_then(|t| t.get(key)),
                        def,
                    )
                }),
            "token" => self
                .tokens
                .iter()
                .find(|(k, _)| k.as_str() == id)
                .and_then(|(key, def)| same(schema.tokens.as_ref().and_then(|t| t.get(key)), def)),
            // The kernel stamps defined-by and defined-at from the context of every define of a
            // trigger or an automation, and the input's values are replaced (D-7204, WFL-2201), so
            // the held definition is compared with those two cleared.
            "trigger" => self
                .triggers
                .iter()
                .find(|(k, _)| k.to_string() == id)
                .and_then(|(key, def)| {
                    schema
                        .triggers
                        .as_ref()
                        .and_then(|t| t.get(key))
                        .map(|held| {
                            let mut held = held.clone();
                            held.defined_by = None;
                            held.defined_at = None;
                            held == *def
                        })
                }),
            "automation" => self
                .automations
                .iter()
                .find(|(k, _)| k.to_string() == id)
                .and_then(|(key, def)| {
                    schema
                        .automations
                        .as_ref()
                        .and_then(|t| t.get(key))
                        .map(|held| {
                            let mut held = held.clone();
                            held.defined_by = None;
                            held.defined_at = None;
                            held == *def
                        })
                }),
            "operation" => self
                .operations
                .iter()
                .find(|(k, _)| k.to_string() == id)
                .and_then(|(key, def)| {
                    same(schema.operations.as_ref().and_then(|t| t.get(key)), def)
                }),
            "hold-rule" => self
                .holds
                .iter()
                .find(|(k, _)| k.to_string() == id)
                .and_then(|(key, def)| {
                    same(schema.hold_rules.as_ref().and_then(|t| t.get(key)), def)
                }),
            "capability" => self
                .capabilities
                .iter()
                .find(|(k, _)| k.to_string() == id)
                .and_then(|(key, def)| {
                    same(schema.capabilities.as_ref().and_then(|t| t.get(key)), def)
                }),
            "setting" => self
                .settings
                .iter()
                .find(|(k, _)| k.to_string() == id)
                .and_then(|(key, def)| {
                    same(schema.settings.as_ref().and_then(|t| t.get(key)), def)
                }),
            "ledger-unit" => self
                .ledger_units
                .iter()
                .find(|(k, _)| k.to_string() == id)
                .and_then(|(key, def)| {
                    same(schema.ledger.as_ref().and_then(|l| l.units.get(key)), def)
                }),
            "ledger-pool" => self
                .ledger_pools
                .iter()
                .find(|(k, _)| k.to_string() == id)
                .and_then(|(key, def)| {
                    same(schema.ledger.as_ref().and_then(|l| l.pools.get(key)), def)
                }),
            "ledger-limit" => self.ledger_limits.get(id).and_then(|def| {
                let held = schema.ledger.as_ref().and_then(|l| {
                    l.limits.iter().find(|held| {
                        strata_kernel::products::ledger_limit_id(
                            &held.unit,
                            &held.owner,
                            &held.scope,
                        ) == id
                    })
                });
                same(held, def)
            }),
            "ui-element" => self
                .ui
                .get(id)
                .and_then(|def| same(strata_kernel::ui::element(schema, id), def)),
            _ => None,
        }
    }

    /// The schema changes that install the bundle, in dependency order: every entity of the
    /// bundle, defined (see [`ProductBundle::entities`] for the order).
    pub fn schema_changes(&self) -> Vec<SchemaChange> {
        self.entities()
            .iter()
            .flat_map(|entity| self.definition_changes(entity))
            .collect()
    }

    /// Refuses a bundle that fills a section the kernel cannot install yet, naming each, and a
    /// bundle with an id outside the namespace of its product.
    pub fn check_installable(&self) -> Result<(), BundleError> {
        if !self.unbuilt.is_empty() {
            let listed = self
                .unbuilt
                .iter()
                .map(|u| format!("\"{}\" ({} entries)", u.section, u.entries))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(BundleError::invalid_bundle(format!(
                "the bundle fills {listed}, which this release does not install yet; remove {} from the bundle, or install it on a release that takes it",
                if self.unbuilt.len() == 1 { "it" } else { "them" }
            )));
        }
        if let (Some(namespace), Some(product)) = (self.namespace(), &self.product) {
            for entity in self.entities() {
                if entity.id.strip_prefix(&namespace).is_none_or(str::is_empty) {
                    return Err(BundleError::invalid(format!(
                        "the {} \"{}\" is not in the namespace \"{namespace}\" of the product \"{product}\"; every entity a product installs has an id that begins with its namespace (INT-1582)",
                        entity.kind, entity.id
                    )));
                }
            }
        }
        Ok(())
    }

    /// Refuses a template whose body is not blocks of the schema or places a token that neither the
    /// bundle nor the tenant defines, and an automation step that sends a template that neither the
    /// bundle nor the tenant declares: a mail template for `SendMail`, a notice template for
    /// `CreateNotice`.
    fn check_templates(&self, schema: &TenantSchema) -> Result<(), BundleError> {
        // The body of a template is blocks of the schema of `strata_kernel::blocks` (D-7900), and
        // every token it places is one the bundle or the tenant defines (CTK-005).
        let bodies = self
            .mail_templates
            .iter()
            .map(|(id, t)| ("mail", id, t.body.as_str()))
            .chain(
                self.notice_templates
                    .iter()
                    .map(|(id, t)| ("notice", id, t.body.as_str())),
            );
        for (kind, id, body) in bodies {
            let analysis = strata_kernel::blocks::analyse(body);
            if let Some(problem) = analysis.problems.first() {
                return Err(BundleError::invalid_bundle(format!(
                    "the {kind} template \"{id}\": the body {problem}"
                )));
            }
            for name in &analysis.tokens {
                let defined = self.tokens.contains_key(name)
                    || schema
                        .tokens
                        .as_ref()
                        .is_some_and(|tokens| tokens.contains_key(name));
                if !defined {
                    return Err(BundleError::invalid_bundle(format!(
                        "the {kind} template \"{id}\" places the token \"{name}\", which neither the bundle nor the tenant defines"
                    )));
                }
            }
        }
        for automation in self.automations.values() {
            for (name, step) in &automation.steps {
                let (template, mail) = match &step.action {
                    AutomationStep::SendMail { template, .. }
                    | AutomationStep::SendDigest { template, .. } => (template, true),
                    AutomationStep::CreateNotice { template, .. } => (template, false),
                    _ => continue,
                };
                let declared = if mail {
                    self.mail_templates.contains_key(template)
                        || schema
                            .mail_templates
                            .as_ref()
                            .is_some_and(|t| t.contains_key(template))
                } else {
                    self.notice_templates.contains_key(template)
                        || schema
                            .notice_templates
                            .as_ref()
                            .is_some_and(|t| t.contains_key(template))
                };
                if !declared {
                    return Err(BundleError::invalid_bundle(format!(
                        "the automation \"{}\", step \"{name}\", uses the {} template \"{template}\", which neither the bundle nor the tenant declares",
                        automation.id,
                        if mail { "mail" } else { "notice" }
                    )));
                }
            }
        }
        Ok(())
    }
}

// ---- releases ----------------------------------------------------------------------------------

/// The leading number of each dot-separated part of a platform release (`0.2-draft` is `[0, 2]`).
fn release_parts(release: &str) -> Vec<u64> {
    release
        .split('.')
        .map(|part| {
            part.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse::<u64>()
                .unwrap_or(0)
        })
        .collect()
}

/// Orders two platform releases, or two versions of a product, by their numbers, a missing part
/// counting as zero.
fn release_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (a, b) = (release_parts(a), release_parts(b));
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            std::cmp::Ordering::Equal => {}
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

/// Orders two versions of a product by semantic versioning (semver.org, section 11): the numeric
/// core `MAJOR.MINOR.PATCH` first (a missing part counting as zero, as `release_cmp` does), then a
/// version with a pre-release part before the same core without one, then the pre-release
/// identifiers. Build metadata after `+` does not order. Platform releases keep `release_cmp`
/// (`PlatformRange::admits`); this orders products only (D-DS-16, WG-5472).
pub fn product_version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (core_a, pre_a) = split_product_version(a);
    let (core_b, pre_b) = split_product_version(b);
    match release_cmp(core_a, core_b) {
        std::cmp::Ordering::Equal => {}
        other => return other,
    }
    match (pre_a, pre_b) {
        (None, None) => std::cmp::Ordering::Equal,
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(x), Some(y)) => cmp_pre_release(x, y),
    }
}

/// The numeric core and the pre-release part (without the leading `-`) of a product version;
/// build metadata is dropped.
fn split_product_version(version: &str) -> (&str, Option<&str>) {
    let version = version.split('+').next().unwrap_or(version);
    match version.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (version, None),
    }
}

/// Compares two pre-release parts identifier by identifier (semver section 11.4): numeric
/// identifiers by value, alphanumeric ones in ASCII order, a numeric one before an alphanumeric
/// one, and a shorter list before a longer one that it prefixes.
fn cmp_pre_release(a: &str, b: &str) -> std::cmp::Ordering {
    let mut left = a.split('.');
    let mut right = b.split('.');
    loop {
        match (left.next(), right.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) => match cmp_pre_identifier(x, y) {
                std::cmp::Ordering::Equal => {}
                other => return other,
            },
        }
    }
}

fn cmp_pre_identifier(x: &str, y: &str) -> std::cmp::Ordering {
    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match (numeric(x), numeric(y)) {
        // Numeric identifiers have no leading zeros in a valid version, so length then text is
        // the numeric order without overflow.
        (true, true) => x.len().cmp(&y.len()).then_with(|| x.cmp(y)),
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        (false, false) => x.cmp(y),
    }
}

impl PlatformRange {
    /// Whether `release` is from `min` up to, and excluding, `below`.
    pub fn admits(&self, release: &str) -> bool {
        release_cmp(release, &self.min) != std::cmp::Ordering::Less
            && self
                .below
                .as_deref()
                .is_none_or(|below| release_cmp(release, below) == std::cmp::Ordering::Less)
    }
}

// ---- planning and running a product change ----------------------------------------------------------

/// What a product change runs against.
pub struct ProductTarget<'a> {
    pub tenant: &'a TenantId,
    /// Who changes: an owner of the tenant.
    pub principal: Principal,
    /// The time of the change, in milliseconds.
    pub now: i64,
    /// The platform release the change runs on.
    pub release: &'a str,
    /// The tenant's schema now, which holds the ownership record of every installed product.
    pub schema: &'a TenantSchema,
    /// The record of the product, when the caller has it from elsewhere; when it is `None` the
    /// record in the schema is used.
    pub installed: Option<&'a InstalledRecord>,
    /// The schema version the caller read, which a different current version refuses (D-6595).
    pub expected_version: Option<SchemaVersion>,
    /// The idempotency key of the request, if it has one: a repeat with the same key and the same
    /// change returns the stored result (D-6595).
    pub idempotency_key: Option<String>,
}

/// The segments a product change defines and removes (WG-5166). A segment is no schema change: the
/// caller that holds the segment store carries these out, around the schema change.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SegmentChanges {
    /// The segments the bundle declares, in key order: defined, or defined again, which changes
    /// nothing when the predicate and the options are as they were.
    pub define: Vec<(SegmentKey, SegmentDecl)>,
    /// The segments the product owns and the change takes out: those a later version no longer
    /// declares, or every one of them when the product is removed.
    pub remove: Vec<SegmentKey>,
}

impl SegmentChanges {
    /// Whether the change touches no segment.
    pub fn is_empty(&self) -> bool {
        self.define.is_empty() && self.remove.is_empty()
    }

    /// The definitions of the segments to define, with the ids derived from the tenant and the
    /// key (D-6582), or the first declaration that cannot be one.
    pub fn definitions(
        &self,
        tenant: &TenantId,
    ) -> Result<Vec<strata_kernel::segments::Definition>, BundleError> {
        self.define
            .iter()
            .map(|(key, decl)| {
                strata_kernel::segments::from_decl(
                    tenant,
                    key,
                    decl,
                    decl.principal_field.clone(),
                    decl.visible_to_members.unwrap_or(false),
                )
                .map_err(|e| BundleError::invalid_bundle(format!("the segment \"{}\": {e}", key.0)))
            })
            .collect()
    }

    /// The ids of the segments to remove.
    pub fn removals(&self, tenant: &TenantId) -> Vec<SegmentId> {
        self.remove
            .iter()
            .map(|key| derive_segment_id(tenant, key))
            .collect()
    }
}

/// What a product change does, before it runs: the edits it makes and the entities it creates,
/// changes and removes (the projection a dry run returns, INT-154).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductChange {
    /// What the change is called in a step name: `install badges`.
    pub step: String,
    /// The version of the product the change leaves installed, or removes.
    pub version: String,
    /// The schema changes, run as one `ChangeSchema` intent.
    pub changes: Vec<SchemaChange>,
    pub created: Vec<OwnedEntity>,
    pub changed: Vec<OwnedEntity>,
    pub removed: Vec<OwnedEntity>,
    /// The segments it defines and removes, through the segment store (WG-5166).
    pub segments: SegmentChanges,
    /// The ownership record the change leaves in the catalogue; `None` for a removal and for a
    /// bundle that names no product.
    pub record: Option<InstalledRecord>,
}

/// A product change that has been planned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Planned {
    /// Nothing to do: the same product and version is installed (this is the schema version of
    /// its install, INT-1583), or the product to remove is not installed (the current schema
    /// version).
    Unchanged {
        schema_version: u64,
    },
    Change(Box<ProductChange>),
}

/// What a product change did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProductInstall {
    /// Nothing was run; this is the schema version (see [`Planned::Unchanged`]).
    Unchanged { schema_version: u64 },
    /// The schema change was applied (one intent, so all of it or none), at this schema version.
    /// `record` is what the catalogue holds for the product from now on: `None` for a bundle that
    /// names no product and for a removal.
    Applied {
        schema_version: u64,
        record: Option<InstalledRecord>,
    },
}

/// Why a product change stopped.
#[derive(Debug)]
pub enum ProductInstallError<E> {
    /// The change was refused before anything ran.
    Refused(BundleError),
    /// The executor refused the schema change.
    Step { step: String, error: E },
}

impl<E> From<BundleError> for ProductInstallError<E> {
    fn from(error: BundleError) -> Self {
        ProductInstallError::Refused(error)
    }
}

impl<E: std::fmt::Debug> std::fmt::Display for ProductInstallError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProductInstallError::Refused(error) => write!(f, "{error}"),
            ProductInstallError::Step { step, error } => write!(f, "{step}: {error:?}"),
        }
    }
}

/// The record of `product` the change starts from: the one the caller gave, else the schema's.
fn installed_of(product: &str, target: &ProductTarget<'_>) -> Option<InstalledRecord> {
    target
        .installed
        .filter(|record| record.product == product)
        .cloned()
        .or_else(|| InstalledRecord::of(target.schema, product))
}

fn check_platform(bundle: &ProductBundle, release: &str) -> Result<(), BundleError> {
    if let Some(range) = &bundle.platform {
        if !range.admits(release) {
            return Err(BundleError::invalid(format!(
                "the platform release {release} is outside the range the bundle supports (from {}{})",
                range.min,
                range
                    .below
                    .as_ref()
                    .map(|below| format!(" up to, and excluding, {below}"))
                    .unwrap_or_default()
            )));
        }
    }
    Ok(())
}

/// The record the change leaves for `bundle`, as the schema version after it will be.
fn record_of(bundle: &ProductBundle, schema: &TenantSchema) -> Option<InstalledRecord> {
    bundle.product.as_ref().map(|product| InstalledRecord {
        product: product.clone(),
        version: bundle.version.clone(),
        platform: bundle.platform.clone(),
        owns: bundle.entities(),
        schema_version: Some(schema.version.0 + 1),
    })
}

/// Plans the install of the bundle of a product. Refuses, in this order: a section the kernel
/// cannot install yet, an id outside the namespace of the product, a platform release outside the
/// bundle's range, another version of the product already installed, a template an automation
/// names that nobody declares, and an entity with one of its ids that exists in the tenant and is
/// not owned by the product. The same product and version again is [`Planned::Unchanged`].
pub fn plan_install(
    bundle: &ProductBundle,
    target: &ProductTarget<'_>,
) -> Result<Planned, BundleError> {
    bundle.check_installable()?;
    check_platform(bundle, target.release)?;
    if let Some(product) = &bundle.product {
        if let Some(installed) = installed_of(product, target) {
            if installed.version == bundle.version {
                return Ok(Planned::Unchanged {
                    schema_version: installed.schema_version.unwrap_or(target.schema.version.0),
                });
            }
            return Err(BundleError::conflict(format!(
                "version {} of the product \"{product}\" is installed; version {} is an upgrade, which upgrade-bundle applies",
                installed.version, bundle.version
            )));
        }
    }
    bundle.check_templates(target.schema)?;
    let owner = bundle
        .product
        .as_ref()
        .map(|product| format!("the product \"{product}\""));
    let entities = bundle.entities();
    for entity in &entities {
        if entity.exists_in(target.schema) {
            return Err(BundleError::conflict(format!(
                "the {} \"{}\" exists in the tenant and is not owned by {}; an install does not take over an entity",
                entity.kind,
                entity.id,
                owner.as_deref().unwrap_or("the bundle")
            )));
        }
    }
    let record = record_of(bundle, target.schema);
    let mut changes = bundle.schema_changes();
    if let Some(record) = &record {
        changes.push(SchemaChange::RecordProduct(
            record.to_kernel().map_err(BundleError::invalid_bundle)?,
        ));
    }
    Ok(Planned::Change(Box::new(ProductChange {
        step: format!(
            "install {}",
            bundle.product.as_deref().unwrap_or(&bundle.name)
        ),
        version: bundle.version.clone(),
        changes,
        created: entities,
        changed: Vec::new(),
        removed: Vec::new(),
        segments: SegmentChanges {
            define: bundle
                .segments
                .iter()
                .map(|(key, decl)| (key.clone(), decl.clone()))
                .collect(),
            remove: Vec::new(),
        },
        record,
    })))
}

/// The order a removal takes entities in: what refers to others first (triggers name automations,
/// automations name templates and object types, views name queries, queries and policies name
/// object types and roles), then the types and roles themselves.
fn removal_rank(kind: &str) -> u8 {
    match kind {
        // An interface element names views, operations and other elements, and nothing names it
        // but another element (D-8250).
        "ui-element" => 0,
        // A hold rule names operations, roles and settings, and nothing names it.
        "hold-rule" => 0,
        "trigger" => 0,
        "automation" => 1,
        "operation" => 2,
        "view" => 3,
        "query" => 4,
        "workflow" => 5,
        "policy" => 6,
        "mail-template" | "notice-template" | "token" | "ledger-limit" => 7,
        "object" | "setting" | "ledger-unit" | "ledger-pool" => 8,
        "role" => 9,
        // Roles grant capabilities, so a capability goes after them.
        "capability" => 10,
        _ => 11,
    }
}

/// The schema change that removes one entity a product owns.
fn removal_change(entity: &OwnedEntity, delete_records: bool) -> Result<SchemaChange, String> {
    let id = entity.id.as_str();
    let at = |e: strata_kernel::IdError| format!("the {} \"{id}\": {e}", entity.kind);
    let bad = |e: String| format!("the {} \"{id}\": {e}", entity.kind);
    Ok(match entity.kind.as_str() {
        "object" => SchemaChange::RemoveObject {
            id: ObjectTypeId::new(id).map_err(at)?,
            delete_records,
        },
        "role" => SchemaChange::RemoveRole(RoleId::new(id).map_err(at)?),
        "policy" => SchemaChange::RemovePolicy(PolicyId::new(id).map_err(at)?),
        "workflow" => SchemaChange::RemoveWorkflow(WorkflowId::new(id).map_err(at)?),
        "query" => SchemaChange::RemoveQuery(QueryId::new(id).map_err(at)?),
        "view" => SchemaChange::RemoveView(ViewId::new(id).map_err(at)?),
        "trigger" => SchemaChange::RemoveTrigger(TriggerId::new(id).map_err(at)?),
        "automation" => SchemaChange::RemoveAutomation(WorkflowId::new(id).map_err(at)?),
        "operation" => SchemaChange::RemoveOperation(TenantOperationId::new(id).map_err(at)?),
        "hold-rule" => SchemaChange::RemoveHoldRule(HoldRuleId::new(id).map_err(at)?),
        "mail-template" => SchemaChange::RemoveMailTemplate(TemplateId(id.to_string())),
        "notice-template" => SchemaChange::RemoveNoticeTemplate(TemplateId(id.to_string())),
        "token" => SchemaChange::RemoveToken(id.to_string()),
        "ui-element" => SchemaChange::RemoveUiElement(id.to_string()),
        "capability" => SchemaChange::RemoveCapability(CapabilityName(id.to_string())),
        "setting" => {
            SchemaChange::RemoveSetting(SettingId::new(id).map_err(|e| bad(e.to_string()))?)
        }
        "ledger-unit" => SchemaChange::RemoveLedgerDefinition(LedgerDefinitionRef::Unit {
            unit: LedgerUnit::new(id).map_err(|e| bad(e.to_string()))?,
        }),
        "ledger-pool" => SchemaChange::RemoveLedgerDefinition(LedgerDefinitionRef::Pool {
            pool: LedgerPoolName::new(id).map_err(|e| bad(e.to_string()))?,
        }),
        "ledger-limit" => SchemaChange::RemoveLedgerDefinition(
            strata_kernel::products::ledger_limit_ref(id)
                .ok_or_else(|| format!("the ledger-limit \"{id}\" is not the id of a limit"))?,
        ),
        other => return Err(format!("this release cannot remove a {other} (\"{id}\")")),
    })
}

/// The edits that take the entities `owned` out of the tenant, in the order of [`removal_rank`],
/// for the entities the tenant still holds.
fn removals(
    owned: &[OwnedEntity],
    schema: &TenantSchema,
    delete_records: bool,
) -> Result<Vec<SchemaChange>, BundleError> {
    let mut held: Vec<&OwnedEntity> = owned.iter().filter(|e| e.exists_in(schema)).collect();
    held.sort_by_key(|e| removal_rank(&e.kind));
    held.into_iter()
        .map(|entity| removal_change(entity, delete_records).map_err(BundleError::invalid_bundle))
        .collect()
}

/// The edits that take what an object type of the tenant holds and its new declaration lacks out
/// of it: a field, a constraint, a composite unique constraint, a declaration.
fn residue_changes(
    id: &ObjectTypeId,
    held: &ObjectTypeDef,
    now: &ObjectTypeDef,
) -> Vec<SchemaChange> {
    let mut changes = Vec::new();
    for field in held.fields.keys().filter(|f| !now.fields.contains_key(*f)) {
        changes.push(SchemaChange::RemoveField {
            object: id.clone(),
            field: field.clone(),
        });
    }
    for constraint in held
        .constraints
        .keys()
        .filter(|c| !now.constraints.contains_key(*c))
    {
        changes.push(SchemaChange::RemoveConstraint {
            object: id.clone(),
            constraint: constraint.clone(),
        });
    }
    let kept_unique = |unique: &ConstraintId| {
        now.unique
            .as_ref()
            .is_some_and(|table| table.contains_key(unique))
    };
    for unique in held.unique.iter().flat_map(BTreeMap::keys) {
        if !kept_unique(unique) {
            changes.push(SchemaChange::RemoveUnique {
                object: id.clone(),
                unique: unique.clone(),
            });
        }
    }
    if held.retention.is_some() && now.retention.is_none() {
        changes.push(SchemaChange::ClearRetention { object: id.clone() });
    }
    if held.publication.is_some() && now.publication.is_none() {
        changes.push(SchemaChange::ClearPublication { object: id.clone() });
    }
    if held.language_field.is_some() && now.language_field.is_none() {
        changes.push(SchemaChange::ClearLanguageField { object: id.clone() });
    }
    if held.translation_group.is_some() && now.translation_group.is_none() {
        changes.push(SchemaChange::ClearTranslationGroup { object: id.clone() });
    }
    if held.searchable.is_some() && now.searchable.is_none() {
        changes.push(SchemaChange::ClearSearchable { object: id.clone() });
    }
    changes
}

/// Plans the upgrade of an installed product to the bundle of a later version. The difference is
/// computed from the ownership record: an entity the bundle declares that the record does not own
/// is created (and refused when the tenant has one by that id), one the record owns and the bundle
/// declares is changed when the tenant holds something else, and one the record owns and the
/// bundle no longer declares is removed (a record type with records is refused by the kernel). A
/// bundle with a version that is not greater is refused; the same version is refused as well, as
/// the contract says (INT-1583).
pub fn plan_upgrade(
    bundle: &ProductBundle,
    target: &ProductTarget<'_>,
) -> Result<Planned, BundleError> {
    bundle.check_installable()?;
    check_platform(bundle, target.release)?;
    let Some(product) = &bundle.product else {
        return Err(BundleError::invalid_bundle(
            "a bundle that names no product cannot be an upgrade",
        ));
    };
    let Some(installed) = installed_of(product, target) else {
        return Err(BundleError::not_found(format!(
            "the product \"{product}\" is not installed; install-bundle installs it"
        )));
    };
    if product_version_cmp(&bundle.version, &installed.version) != std::cmp::Ordering::Greater {
        return Err(BundleError::invalid(format!(
            "version {} is not greater than version {} of the product \"{product}\", which is installed (INT-1583)",
            bundle.version, installed.version
        )));
    }
    bundle.check_templates(target.schema)?;
    let schema = target.schema;
    let owned: BTreeSet<(String, String)> = installed.owns.iter().map(OwnedEntity::key).collect();
    let declared = bundle.entities();
    let declared_keys: BTreeSet<(String, String)> = declared.iter().map(OwnedEntity::key).collect();

    let mut created = Vec::new();
    let mut changed = Vec::new();
    let mut changes = Vec::new();
    let mut residue = Vec::new();
    for entity in &declared {
        let known = owned.contains(&entity.key());
        if entity.kind == "segment" {
            // A segment is defined through the segment store, which a define that changes nothing
            // leaves as it is: only one the product did not own is new.
            if !known {
                created.push(entity.clone());
            }
            continue;
        }
        if !known && entity.exists_in(schema) {
            return Err(BundleError::conflict(format!(
                "the {} \"{}\" exists in the tenant and is not owned by the product \"{product}\"; an upgrade does not take over an entity",
                entity.kind, entity.id
            )));
        }
        match (known, bundle.same_in(entity, schema)) {
            (_, None) => created.push(entity.clone()),
            (true, Some(true)) => continue,
            (_, Some(_)) => changed.push(entity.clone()),
        }
        changes.extend(bundle.definition_changes(entity));
        if entity.kind == "object" {
            if let Some((id, now)) = bundle
                .objects
                .iter()
                .find(|(k, _)| k.to_string() == entity.id)
            {
                if let Some(held) = schema.objects.get(id) {
                    residue.extend(residue_changes(id, held, now));
                }
            }
        }
        if entity.kind == "role" {
            // A role that no longer grants anything is cleared, since a define of no permissions
            // is left out of its definition.
            if let Some((id, _)) = bundle
                .roles
                .iter()
                .find(|(k, _)| k.to_string() == entity.id)
            {
                let held = schema
                    .role_permissions
                    .as_ref()
                    .is_some_and(|table| table.contains_key(id));
                if held && bundle.permissions_of(id).is_empty() {
                    residue.push(SchemaChange::DefineRolePermissions {
                        role: id.clone(),
                        permissions: Vec::new(),
                    });
                }
            }
        }
    }
    changes.extend(residue);
    let dropped: Vec<OwnedEntity> = installed
        .owns
        .iter()
        .filter(|e| !declared_keys.contains(&e.key()))
        .cloned()
        .collect();
    changes.extend(removals(&dropped, schema, false)?);
    let dropped_segments: Vec<SegmentKey> = dropped
        .iter()
        .filter(|e| e.kind == "segment")
        .map(|e| SegmentKey(e.id.clone()))
        .collect();
    let removed: Vec<OwnedEntity> = dropped
        .into_iter()
        .filter(|e| e.exists_in(schema) || e.kind == "segment")
        .collect();
    let record = record_of(bundle, schema);
    if let Some(record) = &record {
        changes.push(SchemaChange::RecordProduct(
            record.to_kernel().map_err(BundleError::invalid_bundle)?,
        ));
    }
    Ok(Planned::Change(Box::new(ProductChange {
        step: format!("upgrade {product}"),
        version: bundle.version.clone(),
        changes,
        created,
        changed,
        removed,
        segments: SegmentChanges {
            define: bundle
                .segments
                .iter()
                .map(|(key, decl)| (key.clone(), decl.clone()))
                .collect(),
            remove: dropped_segments,
        },
        record,
    })))
}

/// Plans the removal of an installed product: exactly the entities its record owns are removed,
/// and the record with them. A record type of the product that has records is refused by the
/// kernel unless `delete_records` says they are deleted with it. A product that is not installed
/// is [`Planned::Unchanged`] at the current schema version.
pub fn plan_remove(
    product: &str,
    delete_records: bool,
    target: &ProductTarget<'_>,
) -> Result<Planned, BundleError> {
    let Some(installed) = installed_of(product, target) else {
        return Ok(Planned::Unchanged {
            schema_version: target.schema.version.0,
        });
    };
    let mut changes = removals(&installed.owns, target.schema, delete_records)?;
    changes.push(SchemaChange::RemoveProduct(
        ProductId::new(product)
            .map_err(|e| BundleError::invalid_bundle(format!("product {product:?}: {e}")))?,
    ));
    let removed = installed
        .owns
        .iter()
        .filter(|e| e.exists_in(target.schema) || e.kind == "segment")
        .cloned()
        .collect();
    let segments = SegmentChanges {
        define: Vec::new(),
        remove: installed
            .owns
            .iter()
            .filter(|e| e.kind == "segment")
            .map(|e| SegmentKey(e.id.clone()))
            .collect(),
    };
    Ok(Planned::Change(Box::new(ProductChange {
        step: format!("remove {product}"),
        version: installed.version.clone(),
        changes,
        created: Vec::new(),
        changed: Vec::new(),
        removed,
        segments,
        record: None,
    })))
}

impl ProductChange {
    /// The intent that runs the change: one `ChangeSchema`, so all of it or none (D-6581).
    pub fn intent(&self, expected_version: Option<SchemaVersion>) -> Intent {
        Intent::ChangeSchema {
            changes: self.changes.clone(),
            expected_version,
            existing: ExistingData::Reject,
        }
    }
}

/// Runs a plan: [`Planned::Unchanged`] runs nothing; a change is handed to `execute` as one
/// `ChangeSchema` intent.
pub fn run_planned<E>(
    planned: Planned,
    target: &ProductTarget<'_>,
    execute: &mut dyn FnMut(Context, Intent) -> Result<Output, E>,
) -> Result<ProductInstall, ProductInstallError<E>> {
    let change = match planned {
        Planned::Unchanged { schema_version } => {
            return Ok(ProductInstall::Unchanged { schema_version })
        }
        Planned::Change(change) => *change,
    };
    let schema_version = if change.changes.is_empty() {
        target.schema.version.0
    } else {
        let ctx = Context {
            tenant: target.tenant.clone(),
            principal: target.principal.clone(),
            session: None,
            now: Timestamp(target.now),
            correlation: format!("{}:{}", change.step, change.version),
            verified_address: None,
            idempotency_key: target.idempotency_key.clone(),
            language: None,
            hop: None,
        };
        match execute(ctx, change.intent(target.expected_version)) {
            Ok(Output::Schema { version }) => version.0,
            Ok(other) => {
                return Err(BundleError::invalid_bundle(format!(
                    "{}: the executor returned {other:?}, not a schema version",
                    change.step
                ))
                .into())
            }
            Err(error) => {
                return Err(ProductInstallError::Step {
                    step: change.step,
                    error,
                })
            }
        }
    };
    Ok(ProductInstall::Applied {
        schema_version,
        record: change.record,
    })
}

/// Installs the bundle of a product into a tenant that exists, by issuing one `ChangeSchema`
/// intent to `execute` (the edits of a bundle, and the record of what the product owns, are
/// applied together or not at all, D-6581): [`plan_install`], then [`run_planned`]. The same
/// product and version again is answered with the first schema version and runs nothing.
pub fn install_product<E>(
    bundle: &ProductBundle,
    target: &ProductTarget<'_>,
    execute: &mut dyn FnMut(Context, Intent) -> Result<Output, E>,
) -> Result<ProductInstall, ProductInstallError<E>> {
    let planned = plan_install(bundle, target)?;
    run_planned(planned, target, execute)
}

/// Upgrades an installed product to the bundle of a later version: [`plan_upgrade`], then
/// [`run_planned`].
pub fn upgrade_product<E>(
    bundle: &ProductBundle,
    target: &ProductTarget<'_>,
    execute: &mut dyn FnMut(Context, Intent) -> Result<Output, E>,
) -> Result<ProductInstall, ProductInstallError<E>> {
    let planned = plan_upgrade(bundle, target)?;
    run_planned(planned, target, execute)
}

/// Removes an installed product: [`plan_remove`], then [`run_planned`].
pub fn remove_product<E>(
    product: &str,
    delete_records: bool,
    target: &ProductTarget<'_>,
    execute: &mut dyn FnMut(Context, Intent) -> Result<Output, E>,
) -> Result<ProductInstall, ProductInstallError<E>> {
    let planned = plan_remove(product, delete_records, target)?;
    run_planned(planned, target, execute)
}

#[cfg(test)]
mod version_order_tests {
    use super::{product_version_cmp, release_cmp, PlatformRange};
    use std::cmp::Ordering::{Equal, Greater, Less};

    fn ascending(versions: &[&str]) {
        for pair in versions.windows(2) {
            assert_eq!(
                product_version_cmp(pair[0], pair[1]),
                Less,
                "{} should sort before {}",
                pair[0],
                pair[1]
            );
            assert_eq!(product_version_cmp(pair[1], pair[0]), Greater);
        }
    }

    #[test]
    fn pre_releases_sort_before_their_release_by_semver_rules() {
        // The example order of semver.org section 11.
        ascending(&[
            "2.0.0-alpha",
            "2.0.0-alpha.1",
            "2.0.0-alpha.beta",
            "2.0.0-beta",
            "2.0.0-beta.2",
            "2.0.0-beta.11",
            "2.0.0-rc.1",
            "2.0.0",
        ]);
    }

    #[test]
    fn stage_markers_sort_between_the_installed_version_and_the_target() {
        // The internal pre-release versions of a staged upgrade (D-DS-16): numeric stage numbers
        // compare by value, so stage 10 follows stage 2.
        ascending(&[
            "1.0.0",
            "1.1.0-stage.1",
            "1.1.0-stage.2",
            "1.1.0-stage.10",
            "1.1.0",
            "1.1.1",
        ]);
    }

    #[test]
    fn a_missing_core_part_counts_as_zero_and_build_metadata_does_not_order() {
        assert_eq!(product_version_cmp("1.0", "1.0.0"), Equal);
        assert_eq!(product_version_cmp("1.0.0+build.7", "1.0.0+build.9"), Equal);
        assert_eq!(
            product_version_cmp("1.0.0-rc.1+build.7", "1.0.0-rc.1"),
            Equal
        );
        assert_eq!(product_version_cmp("1.0.10", "1.0.9"), Greater);
    }

    #[test]
    fn new_and_old_comparators_agree_on_every_repository_version_without_a_pre_release() {
        // Every purely numeric version string the repository uses (bundles, manifests, lock
        // files and platform constants), so the product comparator changes no order that
        // existed before.
        let versions = [
            "0.1.0", "0.1.17", "0.1.3", "0.12", "0.2", "0.2.17", "0.2.46", "0.28.2", "0.3",
            "0.32.1", "0.4", "0.5.3", "0.6.8", "0.7.9", "1.0", "1.0.0", "1.0.10", "1.0.228",
            "1.1.0", "1.1.1", "1.2.2", "1.24.0", "1.3.0", "1.52", "1.65.1", "1.8.0", "1.85",
            "2.0.0", "2.0.119", "2.3.3", "3.0.0", "3.3.20", "4.0.0", "4.0.7", "4.14.0", "4.14.1",
            "4.68.6", "5.0.0", "5.3.0", "5.9.3", "6.0.0", "6.5.0", "7.3.8", "8.5.33",
        ];
        for a in versions {
            for b in versions {
                assert_eq!(
                    product_version_cmp(a, b),
                    release_cmp(a, b),
                    "{a} against {b}"
                );
            }
        }
    }

    #[test]
    fn showcase_products_install_on_the_draft_platform_release() {
        // The showcase products declare `platform.min` 0.2 (admin, community, identity, pages).
        // The platform check keeps `release_cmp`, so a platform release is admitted as before.
        let range = PlatformRange {
            min: "0.2".to_string(),
            below: None,
        };
        assert!(range.admits("0.2-draft"));
        assert!(range.admits("0.2"));
        assert!(!range.admits("0.1.17"));
    }
}
