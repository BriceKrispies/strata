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
