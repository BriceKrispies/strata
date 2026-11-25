//! The ownership record of an installed product: which product owns which entity (D-6582,
//! `installed-product`, held as `tenant-schema.products`).
//!
//! The record is the member `products` of the tenant schema, left out of the encoding while no
//! product is installed. It changes through two schema changes,
//! [`SchemaChange::RecordProduct`] (install and upgrade: the record of the product is put, whole,
//! replacing the one there) and [`SchemaChange::RemoveProduct`]. Because they are schema changes
//! they are applied in the same `ChangeSchema` intent as the entities they record, so the record
//! and the entities are written in one commit under one schema version: there is no moment in
//! which an entity exists that no product owns, or a record names an entity that is not there
//! (D-6581).
//!
//! The kernel holds the record and checks it ([`problems`]): each record is stored under its own
//! product id, every entity it owns that the kernel holds exists, and no entity has two owners. It
//! does not decide what a product is: the installer builds the changes and the record from the
//! bundle.

use crate::schema::TenantSchema;
use crate::types::{
    EntityKind, EntityRef, InstalledProduct, LedgerDefinitionRef, LedgerOwner, LedgerPoolName,
    LedgerScope, LedgerUnit, PrincipalId, ProductId, SchemaChange,
};
use std::collections::{BTreeMap, BTreeSet};

/// Apply [`SchemaChange::RecordProduct`] or [`SchemaChange::RemoveProduct`] to a working copy and
/// describe it. Recording a product that is installed replaces its record; removing the record of
/// a product that is not installed is refused.
pub fn apply(schema: &mut TenantSchema, change: &SchemaChange) -> Result<String, String> {
    Ok(match change {
        SchemaChange::RecordProduct(product) => {
            schema
                .products
                .get_or_insert_with(BTreeMap::new)
                .insert(product.product.clone(), product.clone());
            format!("record product {} {}", product.product, product.version)
        }
        SchemaChange::RemoveProduct(id) => {
            let products = schema.products.as_mut();
            products
                .and_then(|products| products.remove(id))
                .ok_or_else(|| format!("no product {id}"))?;
            // A table left empty is none, so the schema hashes as one that never held a product.
            if schema.products.as_ref().is_some_and(BTreeMap::is_empty) {
                schema.products = None;
            }
            format!("remove product {id}")
        }
        other => return Err(format!("{other:?} is not a change of a product record")),
    })
}

/// The installed products of a schema, in product-id order.
pub fn list(schema: &TenantSchema) -> Vec<&InstalledProduct> {
    schema
        .products
        .iter()
        .flatten()
        .map(|(_, product)| product)
        .collect()
}

/// One installed product, by id.
pub fn get<'a>(schema: &'a TenantSchema, id: &ProductId) -> Option<&'a InstalledProduct> {
    schema
        .products
        .as_ref()
        .and_then(|products| products.get(id))
}

/// The product that owns an entity, if one does.
pub fn owner_of<'a>(schema: &'a TenantSchema, kind: EntityKind, id: &str) -> Option<&'a ProductId> {
    list(schema)
        .into_iter()
        .find(|product| product.owns.iter().any(|e| e.kind == kind && e.id == id))
        .map(|product| &product.product)
}

/// Whether the entity exists in the schema; `None` for a segment, which the schema does not hold
/// and which is not checked.
pub fn exists(schema: &TenantSchema, entity: &EntityRef) -> Option<bool> {
    let id = entity.id.as_str();
    Some(match entity.kind {
        EntityKind::Object => schema.objects.keys().any(|k| k.to_string() == id),
        EntityKind::Role => schema.roles.keys().any(|k| k.to_string() == id),
        EntityKind::Policy => schema.policies.keys().any(|k| k.to_string() == id),
        EntityKind::Workflow => schema.workflows.keys().any(|k| k.to_string() == id),
        EntityKind::Query => schema.queries.keys().any(|k| k.to_string() == id),
        EntityKind::View => schema.views.keys().any(|k| k.to_string() == id),
        EntityKind::Trigger => schema
            .triggers
            .iter()
            .flatten()
            .any(|(k, _)| k.to_string() == id),
        EntityKind::Automation => schema
            .automations
            .iter()
            .flatten()
            .any(|(k, _)| k.to_string() == id),
        EntityKind::Operation => schema
            .operations
            .iter()
            .flatten()
            .any(|(k, _)| k.to_string() == id),
        EntityKind::MailTemplate => schema
            .mail_templates
            .iter()
            .flatten()
            .any(|(k, _)| k.to_string() == id),
        EntityKind::NoticeTemplate => schema
            .notice_templates
            .iter()
            .flatten()
            .any(|(k, _)| k.to_string() == id),
        EntityKind::Token => schema.tokens.as_ref().is_some_and(|t| t.contains_key(id)),
        EntityKind::HoldRule => schema
            .hold_rules
            .iter()
            .flatten()
            .any(|(k, _)| k.to_string() == id),
        EntityKind::UiElement => crate::ui::element(schema, id).is_some(),
        EntityKind::Capability => schema
            .capabilities
            .iter()
            .flatten()
            .any(|(k, _)| k.to_string() == id),
        EntityKind::Setting => schema
            .settings
            .iter()
            .flatten()
            .any(|(k, _)| k.to_string() == id),
        EntityKind::LedgerUnit => schema
            .ledger
            .as_ref()
            .is_some_and(|ledger| ledger.units.keys().any(|k| k.to_string() == id)),
        EntityKind::LedgerPool => schema
            .ledger
            .as_ref()
            .is_some_and(|ledger| ledger.pools.keys().any(|k| k.to_string() == id)),
        EntityKind::LedgerLimit => {
            let Some(LedgerDefinitionRef::Limit { unit, owner, scope }) = ledger_limit_ref(id)
            else {
                return Some(false);
            };
            // The limit of one account lives in the ledger's storage, which the schema cannot see.
            if owner.is_some() {
                return None;
            }
            let scope = scope.as_ref().map_or("", |s| s.as_str());
            schema.ledger.as_ref().is_some_and(|ledger| {
                ledger.limits.iter().any(|l| {
                    l.unit == unit
                        && l.owner == owner
                        && l.scope.as_ref().map_or("", |s| s.as_str()) == scope
                })
            })
        }
        // Segments live in the segment store beside the tenant state, not in the schema.
        EntityKind::Segment => return None,
    })
}

/// The id an ownership record gives a ledger limit, which has none of its own: its unit, then `#`
/// and its owner (`principal:<id>`, `pool:<name>`, or nothing for the limit of every account),
/// then `#` and its scope. It begins with the unit, so it is in the namespace of the product that
/// declares the unit (D-6582).
pub fn ledger_limit_id(
    unit: &LedgerUnit,
    owner: &Option<LedgerOwner>,
    scope: &Option<LedgerScope>,
) -> String {
    let owner = match owner {
        None => String::new(),
        Some(LedgerOwner::Principal { principal }) => format!("principal:{principal}"),
        Some(LedgerOwner::Pool { pool }) => format!("pool:{pool}"),
    };
    let scope = scope.as_ref().map_or("", |s| s.as_str());
    format!("{unit}#{owner}#{scope}")
}

/// The limit a [`ledger_limit_id`] names, or `None` for a text it did not write.
pub fn ledger_limit_ref(id: &str) -> Option<LedgerDefinitionRef> {
    let mut parts = id.splitn(3, '#');
    let unit = LedgerUnit::new(parts.next()?).ok()?;
    let owner = match parts.next()? {
        "" => None,
        text => match text.split_once(':')? {
            ("principal", principal) => Some(LedgerOwner::Principal {
                principal: PrincipalId::new(principal).ok()?,
            }),
            ("pool", pool) => Some(LedgerOwner::Pool {
                pool: LedgerPoolName::new(pool).ok()?,
            }),
            _ => return None,
        },
    };
    let scope = match parts.next()? {
        "" => None,
        text => Some(LedgerScope::new(text).ok()?),
    };
    Some(LedgerDefinitionRef::Limit { unit, owner, scope })
}

/// The problems of the product records of a schema (D-6582): each is stored under its own product
/// id, every entity it owns that the kernel holds exists, and no entity has two owners.
pub fn problems(schema: &TenantSchema, problems: &mut Vec<String>) {
    let Some(products) = schema.products.as_ref() else {
        return;
    };
    let mut owners: BTreeMap<(String, &str), &ProductId> = BTreeMap::new();
    let mut reported: BTreeSet<(String, &str)> = BTreeSet::new();
    for (key, product) in products {
        let at = format!("product {key}");
        if *key != product.product {
            problems.push(format!(
                "{at}: is stored under another id, {}",
                product.product
            ));
        }
        for entity in &product.owns {
            if exists(schema, entity) == Some(false) {
                problems.push(format!(
                    "{at}: owns the {:?} {}, which does not exist",
                    entity.kind, entity.id
                ));
            }
            let slot = (format!("{:?}", entity.kind), entity.id.as_str());
            if let Some(first) = owners.insert(slot.clone(), key) {
                if reported.insert(slot) {
                    problems.push(format!(
                        "{} {} is owned by both {first} and {key}",
                        format!("{:?}", entity.kind).to_lowercase(),
                        entity.id
                    ));
                }
            }
        }
    }
}

/// The capability a change needs when it carries a product record (CT-236, CT-836). The owner holds
/// it; a principal that only manages the schema does not, so such a principal cannot make a change
/// look like the install, upgrade or removal of a product.
pub const BUNDLE_INSTALL: &str = "bundle-install";

/// Whether `change` writes or removes the record of a product.
pub fn is_product_change(change: &SchemaChange) -> bool {
    matches!(
        change,
        SchemaChange::RecordProduct(_) | SchemaChange::RemoveProduct(_)
    )
}

fn entity(kind: EntityKind, id: impl ToString) -> Vec<EntityRef> {
    vec![EntityRef {
        kind,
        id: id.to_string(),
    }]
}

/// The entity a schema change alters or removes, as an ownership record names it; none for a
/// change of a product record. A change of a field, a constraint, a unique constraint or a
/// declaration of an object type alters that object type (INT-1582, D-6582): a product owns its
/// record type whole, so a tenant does not add a field to it either.
pub fn touched(change: &SchemaChange) -> Vec<EntityRef> {
    use SchemaChange as C;
    match change {
        C::DefineObject { id, .. } | C::RemoveObject { id, .. } => entity(EntityKind::Object, id),
        C::DefineField { object, .. }
        | C::DefineRelationship { object, .. }
        | C::RemoveField { object, .. }
        | C::RenameField { object, .. }
        | C::MoveField { object, .. }
        | C::RetypeField { object, .. }
        | C::DefineConstraint { object, .. }
        | C::RemoveConstraint { object, .. }
        | C::DefineUnique { object, .. }
        | C::RemoveUnique { object, .. }
        | C::SetRetention { object, .. }
        | C::SetSearchable { object, .. }
        | C::ClearSearchable { object }
        | C::ClearRetention { object }
        | C::SetPublication { object, .. }
        | C::ClearPublication { object }
        | C::SetLanguageField { object, .. }
        | C::ClearLanguageField { object }
        | C::SetTranslationGroup { object, .. }
        | C::SetAppendOnly { object }
        | C::ClearTranslationGroup { object } => entity(EntityKind::Object, object),
        C::DefineRole(def) => entity(EntityKind::Role, &def.id),
        C::DefineRolePermissions { role, .. } | C::RemoveRole(role) => {
            entity(EntityKind::Role, role)
        }
        C::DefinePolicy(def) => entity(EntityKind::Policy, &def.id),
        C::RemovePolicy(id) => entity(EntityKind::Policy, id),
        C::DefineWorkflow(def) => entity(EntityKind::Workflow, &def.id),
        C::RemoveWorkflow(id) => entity(EntityKind::Workflow, id),
        C::DefineQuery(def) => entity(EntityKind::Query, &def.id),
        C::RemoveQuery(id) => entity(EntityKind::Query, id),
        C::DefineView(def) => entity(EntityKind::View, &def.id),
        C::RemoveView(id) => entity(EntityKind::View, id),
        C::DefineTrigger(def) => entity(EntityKind::Trigger, &def.id),
        C::RemoveTrigger(id) => entity(EntityKind::Trigger, id),
        C::DefineAutomation(def) => entity(EntityKind::Automation, &def.id),
        C::RemoveAutomation(id) => entity(EntityKind::Automation, id),
        C::DefineOperation(def) => entity(EntityKind::Operation, &def.id),
        C::RemoveOperation(id) => entity(EntityKind::Operation, id),
        C::DefineSetting(def) => entity(EntityKind::Setting, &def.id),
        C::RemoveSetting(id) => entity(EntityKind::Setting, id),
        C::DefineCapability(def) => entity(EntityKind::Capability, &def.id),
        C::RemoveCapability(name) => entity(EntityKind::Capability, name),
        C::DeclareLedgerUnit(decl) => entity(EntityKind::LedgerUnit, &decl.unit),
        C::DeclareLedgerPool(decl) => entity(EntityKind::LedgerPool, &decl.pool),
        C::SetLedgerLimit(limit) => entity(
            EntityKind::LedgerLimit,
            ledger_limit_id(&limit.unit, &limit.owner, &limit.scope),
        ),
        C::RemoveLedgerDefinition(LedgerDefinitionRef::Unit { unit }) => {
            entity(EntityKind::LedgerUnit, unit)
        }
        C::RemoveLedgerDefinition(LedgerDefinitionRef::Pool { pool }) => {
            entity(EntityKind::LedgerPool, pool)
        }
        C::RemoveLedgerDefinition(LedgerDefinitionRef::Limit { unit, owner, scope }) => {
            entity(EntityKind::LedgerLimit, ledger_limit_id(unit, owner, scope))
        }
        C::DefineMailTemplate(decl) => entity(EntityKind::MailTemplate, &decl.template),
        C::RemoveMailTemplate(id) => entity(EntityKind::MailTemplate, id),
        C::DefineNoticeTemplate(decl) => entity(EntityKind::NoticeTemplate, &decl.template),
        C::RemoveNoticeTemplate(id) => entity(EntityKind::NoticeTemplate, id),
        C::DefineToken(decl) => entity(EntityKind::Token, &decl.name),
        C::RemoveToken(name) => entity(EntityKind::Token, name),
        C::DefineUiElement { id, .. } | C::RemoveUiElement(id) => entity(EntityKind::UiElement, id),
        C::DefineHoldRule(rule) => entity(EntityKind::HoldRule, &rule.id),
        C::RemoveHoldRule(id) => entity(EntityKind::HoldRule, id),
        C::RecordProduct(_) | C::RemoveProduct(_) => Vec::new(),
    }
}

/// The suffix of the reserved role a product's staged upgrade defines, `<product>__staging`
/// (D-DS-16, WG-5472). It holds no permission and nobody holds it; `assign-role` refuses it.
pub const STAGING_ROLE_SUFFIX: &str = "__staging";

/// Whether a role name is the reserved staging role of some product.
pub fn is_staging_role(role: &str) -> bool {
    role.ends_with(STAGING_ROLE_SUFFIX)
}

/// The first reason `changes` alter an entity of an installed product from outside it, or `None`
/// (D-6582, INT-1582). A change comes through a product when it carries, in the same change, the
/// record of that product at a version other than the installed one (the recorded upgrade path:
/// the record names the product's id and its new version) or the removal of that product. A
/// record that repeats the installed version is no upgrade, so it replaces nothing. The change is
/// judged against the schema before it, so an entity that the change itself creates is not owned
/// yet.
pub fn foreign_change(schema: &TenantSchema, changes: &[SchemaChange]) -> Option<String> {
    let mut passing: BTreeSet<&ProductId> = BTreeSet::new();
    for change in changes {
        match change {
            SchemaChange::RecordProduct(record) => match get(schema, &record.product) {
                Some(held) if held.version == record.version => {
                    return Some(format!(
                        "the record of the product {} at version {} is already installed; only an upgrade to another version replaces it",
                        record.product, record.version
                    ));
                }
                Some(_) => {
                    passing.insert(&record.product);
                }
                None => {}
            },
            SchemaChange::RemoveProduct(id) => {
                passing.insert(id);
            }
            _ => {}
        }
    }
    for change in changes {
        for entity in touched(change) {
            if let Some(owner) = owner_of(schema, entity.kind, &entity.id) {
                if !passing.contains(owner) {
                    return Some(format!(
                        "the {} {} is owned by the product {owner}; it changes only through that product's upgrade or removal",
                        entity.kind.as_str(),
                        entity.id
                    ));
                }
            }
        }
    }
    None
}
