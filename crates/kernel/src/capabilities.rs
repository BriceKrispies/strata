//! Capabilities a product declares (D-6590, D-6701).
//!
//! A capability is a name that a tenant role grants. The platform's capabilities are the
//! specification's; the kernel treats them as opaque strings and holds no list of them. A product
//! may declare capabilities of its own, confined to its own operations: the id begins with the
//! namespace of the product (a product id with each hyphen replaced by an underscore, then two
//! underscores, D-6582), the capability confers nothing by itself, and a role of another product
//! may not grant it (D-6701).
//!
//! The declarations are tenant metadata, held in [`TenantSchema::capabilities`] and changed by
//! the schema changes `define-capability` and `remove-capability`. They are checked with the rest
//! of the schema, so a grant that names a capability nobody declared is refused when the role
//! permissions are defined, and a capability a role grants is not removed (the removal leaves the
//! grant naming an undeclared capability, which the check refuses).
//!
//! Platform capabilities never contain two underscores in a row, so the form of a name says
//! which kind it is: a name of the product form must be declared; a name of the platform form is
//! the platform's, whose existence the gateway checks against the generated list.

use crate::limits;
use crate::schema::TenantSchema;
use crate::types::{CapabilityDef, CapabilityName, RoleId};

/// What separates the namespace of a product from the name of its entity (D-6582).
pub const NAMESPACE_SEPARATOR: &str = "__";

/// The namespace of an id of the product form (`member_tiers__promote` is in `member_tiers`), or
/// `None` for an id of the platform form.
pub fn namespace_of(id: &str) -> Option<&str> {
    let (namespace, rest) = id.split_once(NAMESPACE_SEPARATOR)?;
    (!namespace.is_empty() && !rest.is_empty()).then_some(namespace)
}

/// Whether a capability name is of the product form, which the platform never uses.
pub fn is_product_form(name: &str) -> bool {
    name.contains(NAMESPACE_SEPARATOR)
}

/// Whether the schema declares a capability.
pub fn is_declared(schema: &TenantSchema, name: &str) -> bool {
    schema
        .capabilities
        .as_ref()
        .is_some_and(|declared| declared.contains_key(&CapabilityName(name.to_string())))
}

/// Why a role may not grant a capability, or `None` when it may (D-6590, D-6701): a capability of
/// the product form must be declared in the tenant, and a role that belongs to a product (its id
/// has a namespace) grants only the capabilities of that product. A role of the tenant's own, with
/// no namespace, may grant any declared capability. A name of the platform form is the platform's.
pub fn grant_problem(schema: &TenantSchema, role: &RoleId, name: &str) -> Option<String> {
    if !is_product_form(name) {
        return None;
    }
    if !is_declared(schema, name) {
        return Some(format!(
            "capability {name} is neither a platform capability nor declared in the tenant"
        ));
    }
    match (namespace_of(role.as_str()), namespace_of(name)) {
        (Some(of_role), Some(of_capability)) if of_role != of_capability => Some(format!(
            "capability {name} belongs to product {of_capability} and role {role} to product {of_role}"
        )),
        _ => None,
    }
}

/// The problems of the capabilities a schema declares (D-6590): at most the stated number; the id
/// is a name of the product form, a namespace and a name of lower-case letters, digits and
/// underscores, each starting with a letter; the key of the table is the id; a capability has a
/// name.
pub fn declaration_problems(schema: &TenantSchema, problems: &mut Vec<String>) {
    let Some(declared) = &schema.capabilities else {
        return;
    };
    if declared.len() > limits::MAX_CAPABILITIES {
        problems.push(format!(
            "limit: {} capabilities is more than the maximum of {}",
            declared.len(),
            limits::MAX_CAPABILITIES
        ));
    }
    for (key, def) in declared {
        let at = format!("capability {}", def.id);
        if key != &def.id {
            problems.push(format!("{at}: held under the key {key}"));
        }
        if !well_formed(def.id.as_str()) {
            problems.push(format!(
                "{at}: an id is a namespace and a name of lower-case letters, digits and underscores, joined by two underscores"
            ));
        }
        if def.name.trim().is_empty() {
            problems.push(format!("{at}: a capability needs a name"));
        }
    }
}

/// Whether an id is `<namespace>__<name>`, each part of `[a-z][a-z0-9_]*`, the namespace not
/// ending in an underscore so that the first two underscores in a row are the separator.
fn well_formed(id: &str) -> bool {
    let Some((namespace, name)) = id.split_once(NAMESPACE_SEPARATOR) else {
        return false;
    };
    let part = |text: &str| {
        let mut chars = text.chars();
        matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    };
    part(namespace) && !namespace.ends_with('_') && part(name) && id.len() <= 128
}

/// Define or replace a declared capability in a working copy of a schema.
pub fn apply_define(schema: &mut TenantSchema, def: &CapabilityDef) -> String {
    schema
        .capabilities
        .get_or_insert_with(Default::default)
        .insert(def.id.clone(), def.clone());
    format!("define capability {}", def.id)
}

/// Remove a declared capability from a working copy of a schema. Removing one that is not
/// declared is refused. A table left empty is dropped, so a schema that declares nothing holds no
/// table, as the genesis schema does.
pub fn apply_remove(schema: &mut TenantSchema, id: &CapabilityName) -> Result<String, String> {
    let mut declared = schema.capabilities.take().unwrap_or_default();
    let found = declared.remove(id).is_some();
    schema.capabilities = (!declared.is_empty()).then_some(declared);
    if found {
        Ok(format!("remove capability {id}"))
    } else {
        Err(format!("no capability {id}"))
    }
}
