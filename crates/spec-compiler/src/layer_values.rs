//! The typed sections of a product file: each value is checked against a platform type.
//!
//! A product file declares tenant metadata that has no entity kind of its own in the specification
//! format: object types, policies, queries, views, workflows, segments, settings, ledger units, pools
//! and limits, mail and notice templates, tokens, triggers, automations and operations (D-6580). Their shape is a type of
//! the platform specification, the one the tenant bundle carries (`bundle-sections`). One table,
//! [`TYPED_SECTIONS`], maps each section to that type, and one check, [`check_item`], tests a value
//! against a declared type: the fields of a struct (required and optional), the variants of a union
//! by tag, the values of an enum, lists, maps, ids by pattern and length, and scalars by
//! representation. Exegra reads the same table and applies the same check (`src/core/layer-values.js`
//! there), and both give the same rule codes:
//!
//! - `layer.value-invalid`: a value that does not have the shape of its platform type;
//! - `layer.outside-catalogue`: a union value whose variant the platform's union does not have (the
//!   published catalogue of INT-153: a trigger event, an automation step, an operation step);
//! - `layer.reference-unknown`, `layer.cross-product-reference`: a name in the product's own
//!   namespace that no entity of the product, or one of another product, declares (resolved by
//!   [`crate::layers::load_layered`] once every product file is read).
//!
//! Expressions are written in the authoring syntax of the tenant rule language, as text, wherever
//! the platform type is the AST `expr` or the text type `expression` ([`AUTHORING_TEXT_TYPES`]). The
//! check here collects each such text with its place ([`ExprUse`]); [`crate::layer_expr`] parses and
//! checks it against the scope it is written in (WG-5060).

use std::collections::BTreeMap;

use regex::Regex;
use serde_json::{Map, Value};

use crate::diagnostic::Diagnostic;
use crate::layer_expr::ExprUse;
use crate::layers::{variant_tag, OUTSIDE_CATALOGUE, VALUE_INVALID};

/// One typed section of a product file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypedSection {
    /// The section's name in a product file.
    pub section: &'static str,
    /// The platform type each value of the section conforms to.
    pub ty: &'static str,
    /// The member of the value that names it, when the type has one. The item's `id` is that
    /// member (it is taken from the id when the item leaves it out, and refused when it differs).
    /// `None`: the type has no name, and the item's `id` is only the product's name for it.
    pub id_field: Option<&'static str>,
    /// What the section is for, as the message of a refused section lists it.
    pub purpose: &'static str,
}

/// The typed sections, in the order the messages list them (D-6580).
pub const TYPED_SECTIONS: &[TypedSection] = &[
    TypedSection {
        section: "objects",
        ty: "object-type-def",
        id_field: Some("id"),
        purpose: "object types (values of the platform type object-type-def)",
    },
    TypedSection {
        section: "policies",
        ty: "policy-def",
        id_field: Some("id"),
        purpose: "policies of roles over object types (policy-def)",
    },
    TypedSection {
        section: "queries",
        ty: "query-def",
        id_field: Some("id"),
        purpose: "named queries over object types (values of the platform type query-def)",
    },
    TypedSection {
        section: "views",
        ty: "view-def",
        id_field: Some("id"),
        purpose: "views that present queries at a route (values of the platform type view-def)",
    },
    TypedSection {
        section: "workflows",
        ty: "workflow-def",
        id_field: Some("id"),
        purpose: "workflows of a field of an object type: its states and the transitions between them (values of the platform type workflow-def)",
    },
    TypedSection {
        section: "segments",
        ty: "segment-decl",
        id_field: None,
        purpose: "segments (segment-decl)",
    },
    TypedSection {
        section: "settings",
        ty: "setting-def",
        id_field: Some("id"),
        purpose: "setting definitions (setting-def)",
    },
    TypedSection {
        section: "ledgerUnits",
        ty: "ledger-unit-decl",
        id_field: Some("unit"),
        purpose: "ledger units (ledger-unit-decl)",
    },
    TypedSection {
        section: "ledgerPools",
        ty: "ledger-pool-decl",
        id_field: Some("pool"),
        purpose: "ledger pools (ledger-pool-decl)",
    },
    TypedSection {
        section: "ledgerLimits",
        ty: "ledger-limit",
        id_field: None,
        purpose: "floors, caps and period caps of ledger accounts (ledger-limit)",
    },
    TypedSection {
        section: "mailTemplates",
        ty: "mail-template-decl",
        id_field: Some("template"),
        purpose: "mail templates (mail-template-decl)",
    },
    TypedSection {
        section: "noticeTemplates",
        ty: "notice-template-decl",
        id_field: Some("template"),
        purpose: "notice templates (notice-template-decl)",
    },
    TypedSection {
        section: "tokens",
        ty: "token-decl",
        id_field: Some("name"),
        purpose: "tokens of templates (token-decl)",
    },
    TypedSection {
        section: "triggers",
        ty: "trigger-def",
        id_field: Some("id"),
        purpose: "triggers (values of the platform type trigger-def)",
    },
    TypedSection {
        section: "automations",
        ty: "automation-def",
        id_field: Some("id"),
        purpose: "automations (values of the platform type automation-def)",
    },
    TypedSection {
        section: "operations",
        ty: "tenant-operation-def",
        id_field: Some("id"),
        purpose: "composite operations (values of the platform type tenant-operation-def)",
    },
    TypedSection {
        section: "holds",
        ty: "hold-rule",
        id_field: Some("id"),
        purpose: "rules that hold a change for approval (values of the platform type hold-rule, D-8400)",
    },
    TypedSection {
        section: "eventKinds",
        ty: "event-kind-decl",
        id_field: None,
        purpose: "event kinds the product declares, which a trigger may select (event-kind-decl, SCH-1576)",
    },
];

/// The typed section called `section`.
pub fn typed_section(section: &str) -> Option<&'static TypedSection> {
    TYPED_SECTIONS.iter().find(|s| s.section == section)
}

/// Platform id types whose value, in a product file, names an entity of the product, with the
/// sections it is declared in. A value in the product's own namespace must be declared there; one
/// without a namespace names something outside the product and is not checked. These types are
/// checked by their pattern and length only: `segment-id` is a uuid once installed (D-6582), and a
/// product names its segment by its namespaced key.
pub const REFERENCE_TYPES: &[(&str, &[&str])] = &[
    ("object-type-id", &["objects"]),
    ("ledger-unit", &["ledgerUnits"]),
    ("ledger-pool-name", &["ledgerPools"]),
    ("role-id", &["roles"]),
    ("policy-id", &["policies"]),
    ("workflow-id", &["automations", "workflows"]),
    ("query-id", &["queries"]),
    ("view-id", &["views"]),
    ("template-id", &["mailTemplates", "noticeTemplates"]),
    ("setting-id", &["settings"]),
    ("segment-id", &["segments"]),
];

/// Platform types whose value may be written as text in the authoring syntax of the tenant rule
/// language instead of as the AST (D-6546: "expressions in the authoring syntax").
pub const AUTHORING_TEXT_TYPES: &[&str] = &["expr", "expression"];

/// The separator between the namespace of a product and the local name (D-6582).
pub const NAMESPACE_SEPARATOR: &str = "__";

/// A name of a typed value that must resolve to an entity of the product, collected while the
/// values are checked and resolved once every product file is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefUse {
    /// The product file.
    pub file: String,
    /// The entity that holds the name.
    pub entity: String,
    /// Where in the entity.
    pub at: String,
    /// The name.
    pub name: String,
    /// The sections it must be declared in.
    pub sections: &'static [&'static str],
}

/// The types of the platform specification, by id.
#[derive(Debug, Clone, Default)]
pub struct Types<'a>(BTreeMap<&'a str, &'a Value>);

impl<'a> Types<'a> {
    /// The `types` of the parsed platform specification.
    pub fn of(platform: &'a Value) -> Self {
        let mut by_id = BTreeMap::new();
        for ty in platform
            .get("types")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(id) = ty.get("id").and_then(Value::as_str) {
                by_id.entry(id).or_insert(ty);
            }
        }
        Types(by_id)
    }

    /// The declaration of the platform type `id`.
    pub fn get(&self, id: &str) -> Option<&'a Value> {
        self.0.get(id).copied()
    }
}

/// Checks one item of a typed section against its platform type, pushing a diagnostic for each
/// mismatch and a [`RefUse`] for each name that refers to an entity of the product.
pub fn check_item(
    types: &Types<'_>,
    file: &str,
    section: &TypedSection,
    id: &str,
    item: &Value,
    out: &mut Vec<Diagnostic>,
    refs: &mut Vec<RefUse>,
) {
    check_item_exprs(types, file, section, id, item, out, refs, &mut Vec::new());
}

/// [`check_item`], also collecting each expression written as text, with its place in the item.
pub fn check_item_exprs(
    types: &Types<'_>,
    file: &str,
    section: &TypedSection,
    id: &str,
    item: &Value,
    out: &mut Vec<Diagnostic>,
    refs: &mut Vec<RefUse>,
    exprs: &mut Vec<ExprUse>,
) {
    let Some(map) = item.as_object() else {
        return;
    };
    let mut value = map.clone();
    let mut check = Check {
        types,
        file,
        entity: id,
        root: section.ty,
        out,
        refs,
        exprs,
    };
    if section.id_field != Some("id") {
        value.remove("id");
    }
    if let Some(field) = section.id_field.filter(|f| *f != "id") {
        match value.get(field).cloned() {
            Some(Value::String(own)) if own == id => {}
            Some(other) => {
                check.bad(
                    field,
                    format!("is {other}, but must be the id \"{id}\" of the item, or be left out"),
                );
            }
            None => {
                value.insert(field.to_string(), Value::String(id.to_string()));
            }
        }
    }
    let Some(ty) = types.get(section.ty) else {
        check.bad(
            "",
            format!(
                "cannot be checked: the platform specification declares no type \"{}\"",
                section.ty
            ),
        );
        return;
    };
    check.declared(section.ty, ty, &Value::Object(value), "", section.id_field);
}

struct Check<'a, 'b> {
    types: &'a Types<'a>,
    file: &'a str,
    entity: &'a str,
    root: &'a str,
    out: &'b mut Vec<Diagnostic>,
    refs: &'b mut Vec<RefUse>,
    exprs: &'b mut Vec<ExprUse>,
}

/// A type text split into its wrapper and what it wraps.
enum Ty<'t> {
    List(&'t str),
    Set(&'t str),
    Option(&'t str),
    Shared(&'t str),
    Map(&'t str, &'t str),
    Name(&'t str),
}

fn parse_ty(text: &str) -> Ty<'_> {
    let text = text.trim();
    for prefix in ["list<", "set<", "option<", "shared<"] {
        if let Some(inner) = text.strip_prefix(prefix).and_then(|t| t.strip_suffix('>')) {
            let inner = inner.trim();
            return match prefix {
                "list<" => Ty::List(inner),
                "set<" => Ty::Set(inner),
                "option<" => Ty::Option(inner),
                _ => Ty::Shared(inner),
            };
        }
    }
    if let Some(inner) = text.strip_prefix("map<").and_then(|t| t.strip_suffix('>')) {
        let mut depth = 0i32;
        for (i, c) in inner.char_indices() {
            match c {
                '<' => depth += 1,
                '>' => depth -= 1,
                ',' if depth == 0 => {
                    let (k, v) = (inner[..i].trim(), inner[i + 1..].trim());
                    if !k.is_empty() && !v.is_empty() {
                        return Ty::Map(k, v);
                    }
                }
                _ => {}
            }
        }
    }
    Ty::Name(text)
}

fn join(at: &str, key: &str) -> String {
    if at.is_empty() {
        key.to_string()
    } else {
        format!("{at}.{key}")
    }
}

impl Check<'_, '_> {
    fn bad(&mut self, at: &str, problem: String) {
        let place = if at.is_empty() {
            "its value".to_string()
        } else {
            format!("\"{at}\"")
        };
        self.out.push(Diagnostic::new(
            VALUE_INVALID,
            self.entity,
            format!(
                "\"{}\" in {}: {place} {problem}. A value in this section has the platform type \"{}\"; correct it to match that type in the platform specification.",
                self.entity, self.file, self.root
            ),
        ));
    }

    /// Checks `value` against the type text `ty`.
    fn value(&mut self, ty: &str, value: &Value, at: &str) {
        match parse_ty(ty) {
            Ty::Option(inner) => {
                if !value.is_null() {
                    self.value(inner, value, at);
                }
            }
            Ty::Shared(inner) => self.value(inner, value, at),
            Ty::List(inner) | Ty::Set(inner) => match value.as_array() {
                Some(items) => {
                    for (i, item) in items.iter().enumerate() {
                        self.value(inner, item, &join(at, &i.to_string()));
                    }
                }
                None => self.bad(at, format!("must be a list of {inner}")),
            },
            Ty::Map(key_ty, value_ty) => match value.as_object() {
                Some(map) => {
                    for (key, item) in map {
                        let place = join(at, key);
                        self.value(key_ty, &Value::String(key.clone()), &place);
                        self.value(value_ty, item, &place);
                        if let Some(Value::String(own)) = item.get("id") {
                            if own != key {
                                self.bad(
                                    &place,
                                    format!("is keyed \"{key}\" but names itself \"{own}\""),
                                );
                            }
                        }
                    }
                }
                None => self.bad(at, format!("must be a map from {key_ty} to {value_ty}")),
            },
            Ty::Name(name) => self.named(name, value, at),
        }
    }

    fn named(&mut self, name: &str, value: &Value, at: &str) {
        match name {
            "string" | "uuid" | "bytes" => match value.as_str() {
                Some(text) if name == "uuid" && !is_uuid(text) => {
                    self.bad(at, format!("must be a uuid, not \"{text}\""))
                }
                Some(_) => {}
                None => self.bad(at, format!("must be a string ({name})")),
            },
            "bool" => {
                if !value.is_boolean() {
                    self.bad(at, "must be true or false".to_string());
                }
            }
            "u8" | "u16" | "u32" | "u64" => {
                if value.as_u64().is_none() {
                    self.bad(at, format!("must be a non-negative whole number ({name})"));
                }
            }
            "i32" | "i64" | "timestamp" | "duration" => {
                if value.as_i64().is_none() {
                    self.bad(at, format!("must be a whole number ({name})"));
                }
            }
            "f64" => {
                if !value.is_number() {
                    self.bad(at, "must be a number".to_string());
                }
            }
            "json" => {}
            other => match self.types.get(other) {
                Some(ty) => self.declared(other, ty, value, at, None),
                None => self.bad(
                    at,
                    format!("names the type \"{other}\", which the platform specification does not declare"),
                ),
            },
        }
    }

    /// Checks `value` against the platform type `ty` called `name`. `decl_field` is the member of
    /// a struct that declares the item (not a reference).
    fn declared(
        &mut self,
        name: &str,
        ty: &Value,
        value: &Value,
        at: &str,
        decl_field: Option<&str>,
    ) {
        if AUTHORING_TEXT_TYPES.contains(&name) {
            if let Some(text) = value.as_str() {
                self.exprs.push(ExprUse {
                    at: at.to_string(),
                    text: text.to_string(),
                });
                return;
            }
        }
        match ty.get("kind").and_then(Value::as_str).unwrap_or("") {
            "struct" => {
                let fields = ty.get("fields").and_then(Value::as_array);
                self.strukt(fields.map_or(&[][..], Vec::as_slice), value, at, decl_field);
            }
            "enum" => {
                let values: Vec<&str> = ty
                    .get("values")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                match value.as_str() {
                    Some(text) if values.contains(&text) => {}
                    _ => self.bad(at, format!("must be one of {} ({name})", values.join(", "))),
                }
            }
            "union" => self.union(name, ty, value, at),
            _ => self.scalar(name, ty, value, at),
        }
    }

    /// An `id` or `scalar` type: its representation, pattern and length.
    fn scalar(&mut self, name: &str, ty: &Value, value: &Value, at: &str) {
        let reference: Option<&'static [&'static str]> = REFERENCE_TYPES
            .iter()
            .find(|(t, _)| *t == name)
            .map(|(_, sections)| *sections);
        let repr = ty.get("repr").and_then(Value::as_str).unwrap_or("string");
        let before = self.out.len();
        if reference.is_some() {
            if !value.is_string() {
                self.bad(
                    at,
                    format!("must be the name of an entity ({name}), written as a string"),
                );
            }
        } else {
            self.value(repr, value, at);
        }
        if self.out.len() > before {
            return;
        }
        let Some(text) = value.as_str() else {
            return;
        };
        let constraints = ty.get("constraints");
        if let Some(max) = constraints
            .and_then(|c| c.get("maxLength"))
            .and_then(Value::as_u64)
        {
            if text.chars().count() as u64 > max {
                self.bad(at, format!("is longer than {max} characters ({name})"));
                return;
            }
        }
        if let Some(pattern) = constraints
            .and_then(|c| c.get("pattern"))
            .and_then(Value::as_str)
        {
            if Regex::new(pattern).is_ok_and(|re| !re.is_match(text)) {
                self.bad(
                    at,
                    format!("is \"{text}\", which does not match the pattern {pattern} of {name}"),
                );
                return;
            }
        }
        if let Some(sections) = reference {
            self.refs.push(RefUse {
                file: self.file.to_string(),
                entity: self.entity.to_string(),
                at: at.to_string(),
                name: text.to_string(),
                sections,
            });
        }
    }

    fn strukt(&mut self, fields: &[Value], value: &Value, at: &str, decl_field: Option<&str>) {
        let Some(map) = value.as_object() else {
            self.bad(at, "must be an object".to_string());
            return;
        };
        let name_of = |f: &Value| {
            f.get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let names: Vec<String> = fields.iter().map(name_of).collect();
        for key in map.keys() {
            if !names.contains(key) {
                self.bad(
                    &join(at, key),
                    format!(
                        "is not a field of its type; the fields are {}",
                        names.join(", ")
                    ),
                );
            }
        }
        for field in fields {
            let name = name_of(field);
            let ty = field.get("type").and_then(Value::as_str).unwrap_or("json");
            let optional = field.get("optional").and_then(Value::as_bool) == Some(true)
                || matches!(parse_ty(ty), Ty::Option(_));
            match map.get(&name) {
                Some(item) => {
                    let place = join(at, &name);
                    if decl_field == Some(name.as_str()) {
                        self.declaration(ty, item, &place);
                    } else {
                        self.value(ty, item, &place);
                    }
                }
                None if optional => {}
                None => self.bad(at, format!("lacks the required field \"{name}\" ({ty})")),
            }
        }
    }

    /// The member that names the item: checked as any value of its type, but not collected as a
    /// reference to another entity.
    fn declaration(&mut self, ty: &str, value: &Value, at: &str) {
        let before = self.refs.len();
        self.value(ty, value, at);
        self.refs.truncate(before);
    }

    fn union(&mut self, name: &str, ty: &Value, value: &Value, at: &str) {
        let encoding = ty.get("encoding");
        let case = encoding
            .and_then(|e| e.get("variantCase"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let internal =
            encoding.and_then(|e| e.get("tag")).and_then(Value::as_str) == Some("internal");
        let tag_field = encoding
            .and_then(|e| e.get("tagField"))
            .and_then(Value::as_str)
            .unwrap_or("type");
        let variants: Vec<&Value> = ty
            .get("variants")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .collect();
        let tag_of =
            |v: &Value| variant_tag(v.get("name").and_then(Value::as_str).unwrap_or(""), case);
        let listing = variants
            .iter()
            .map(|v| tag_of(v))
            .collect::<Vec<_>>()
            .join(", ");
        let written = if internal {
            format!("{{\"{tag_field}\": \"<Variant>\", ... }}")
        } else {
            "{\"<Variant>\": { ... }}, or \"<Variant>\" for a variant without fields".to_string()
        };

        let (tag, payload): (String, Option<Value>) = match value {
            Value::String(tag) if !internal => (tag.clone(), None),
            Value::Object(map) if internal => match map.get(tag_field).and_then(Value::as_str) {
                Some(tag) => {
                    let mut rest: Map<String, Value> = map.clone();
                    rest.remove(tag_field);
                    (tag.to_string(), Some(Value::Object(rest)))
                }
                None => {
                    self.bad(at, format!("must be one variant of the union \"{name}\", written {written}; the variants are {listing}"));
                    return;
                }
            },
            Value::Object(map) if map.len() == 1 => {
                let (tag, payload) = map
                    .iter()
                    .next()
                    .map_or_else(Default::default, |(k, v)| (k.clone(), v.clone()));
                (tag, Some(payload))
            }
            _ => {
                self.bad(at, format!("must be one variant of the union \"{name}\", written {written}; the variants are {listing}"));
                return;
            }
        };
        let Some(variant) = variants.iter().find(|v| tag_of(v) == tag) else {
            let place = if at.is_empty() {
                "its value".to_string()
            } else {
                format!("\"{at}\"")
            };
            self.out.push(Diagnostic::new(
                OUTSIDE_CATALOGUE,
                self.entity,
                format!(
                    "\"{}\" in {} names \"{tag}\" at {}, which is outside the published catalogue (INT-153): the platform's union \"{name}\" has the variants {listing}. Use one of them; a kind the catalogue lacks belongs in the platform specification, not in a product.",
                    self.entity,
                    self.file,
                    if at.is_empty() { place } else { at.to_string() },
                ),
            ));
            return;
        };
        let fields: &[Value] = variant
            .get("fields")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let place = join(at, &tag);
        if variant.get("newtype").and_then(Value::as_bool) == Some(true) {
            let inner = fields
                .first()
                .and_then(|f| f.get("type"))
                .and_then(Value::as_str)
                .unwrap_or("json");
            match &payload {
                Some(payload) => self.value(inner, payload, &place),
                None => self.bad(&place, format!("needs a value of type {inner}")),
            }
        } else if variant.get("tuple").is_some() {
            if payload.as_ref().is_none_or(|p| !p.is_array()) {
                self.bad(&place, "must be a list".to_string());
            }
        } else if fields.is_empty() {
            if payload
                .as_ref()
                .is_some_and(|p| !p.is_null() && p.as_object().is_none_or(|o| !o.is_empty()))
            {
                self.bad(&place, format!("takes no fields: write \"{tag}\" alone"));
            }
        } else {
            match payload {
                Some(payload) => self.strukt(fields, &payload, &place, None),
                None => {
                    let required: Vec<&str> = fields
                        .iter()
                        .filter(|f| {
                            f.get("optional").and_then(Value::as_bool) != Some(true)
                                && !f
                                    .get("type")
                                    .and_then(Value::as_str)
                                    .is_some_and(|t| t.starts_with("option<"))
                        })
                        .filter_map(|f| f.get("name").and_then(Value::as_str))
                        .collect();
                    if !required.is_empty() {
                        self.bad(
                            at,
                            format!("names \"{tag}\" without its fields {}", required.join(", ")),
                        );
                    }
                }
            }
        }
    }
}

/// Whether `text` is a uuid in its canonical form.
fn is_uuid(text: &str) -> bool {
    let parts: Vec<&str> = text.split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(p, n)| p.len() == n && p.chars().all(|c| c.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn platform() -> Value {
        json!({ "types": [
            { "id": "step", "kind": "union", "encoding": { "tag": "external", "variantCase": "PascalCase" },
              "variants": [
                { "name": "wait-for", "fields": [ { "name": "duration", "type": "duration" } ] },
                { "name": "end" } ] },
            { "id": "step-def", "kind": "struct", "fields": [
                { "name": "id", "type": "step-id" },
                { "name": "action", "type": "step" },
                { "name": "next", "type": "option<step-id>", "optional": true },
                { "name": "tags", "type": "map<string, expression>", "optional": true } ] },
            { "id": "step-id", "kind": "id", "repr": "string", "constraints": { "pattern": "^[a-z][a-z0-9_]*$", "maxLength": 8 } },
            { "id": "expression", "kind": "scalar", "repr": "string" }
        ] })
    }

    fn run(item: Value) -> Vec<Diagnostic> {
        let platform = platform();
        let types = Types::of(&platform);
        let section = TypedSection {
            section: "steps",
            ty: "step-def",
            id_field: Some("id"),
            purpose: "steps",
        };
        let mut out = Vec::new();
        let mut refs = Vec::new();
        check_item(
            &types, "p.jsonc", &section, "one", &item, &mut out, &mut refs,
        );
        out
    }

    #[test]
    fn a_conforming_value_is_accepted() {
        assert!(run(
            json!({ "id": "one", "action": { "WaitFor": { "duration": 5 } }, "next": "two" })
        )
        .is_empty());
        assert!(run(json!({ "id": "one", "action": "End", "tags": { "a": "x" } })).is_empty());
    }

    #[test]
    fn a_missing_field_a_stray_field_and_a_wrong_scalar_are_refused() {
        let d = run(json!({ "id": "one" }));
        assert!(
            d.iter().any(|d| d.code == VALUE_INVALID
                && d.message.contains("lacks the required field \"action\"")),
            "{d:?}"
        );
        let d = run(json!({ "id": "one", "action": "End", "extra": 1 }));
        assert!(
            d.iter()
                .any(|d| d.code == VALUE_INVALID && d.message.contains("\"extra\"")),
            "{d:?}"
        );
        let d = run(json!({ "id": "one", "action": { "WaitFor": { "duration": "soon" } } }));
        assert!(
            d.iter()
                .any(|d| d.message.contains("must be a whole number")),
            "{d:?}"
        );
        let d = run(json!({ "id": "Bad-Id", "action": "End" }));
        assert!(
            d.iter()
                .any(|d| d.message.contains("does not match the pattern")),
            "{d:?}"
        );
    }

    #[test]
    fn a_variant_the_union_lacks_is_outside_the_catalogue() {
        let d = run(json!({ "id": "one", "action": { "Run": { "source": "x" } } }));
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].code, OUTSIDE_CATALOGUE);
        assert!(
            d[0].message.contains("\"Run\" at action") && d[0].message.contains("WaitFor, End"),
            "{}",
            d[0]
        );
    }

    #[test]
    fn the_member_that_names_the_item_must_be_the_id() {
        let platform = json!({ "types": [
            { "id": "unit-decl", "kind": "struct", "fields": [ { "name": "unit", "type": "unit-name" }, { "name": "description", "type": "string" } ] },
            { "id": "unit-name", "kind": "id", "repr": "string" } ] });
        let types = Types::of(&platform);
        let section = TypedSection {
            section: "units",
            ty: "unit-decl",
            id_field: Some("unit"),
            purpose: "units",
        };
        let mut out = Vec::new();
        let mut refs = Vec::new();
        check_item(
            &types,
            "p.jsonc",
            &section,
            "p__points",
            &json!({ "id": "p__points", "description": "d" }),
            &mut out,
            &mut refs,
        );
        assert!(out.is_empty(), "{out:?}");
        check_item(
            &types,
            "p.jsonc",
            &section,
            "p__points",
            &json!({ "id": "p__points", "unit": "other", "description": "d" }),
            &mut out,
            &mut refs,
        );
        assert!(
            out.iter().any(|d| d.message.contains("must be the id")),
            "{out:?}"
        );
    }

    #[test]
    fn type_texts_are_split_by_wrapper() {
        assert!(matches!(parse_ty("list<a>"), Ty::List("a")));
        assert!(matches!(
            parse_ty("map<a, list<b>>"),
            Ty::Map("a", "list<b>")
        ));
        assert!(matches!(parse_ty("u32"), Ty::Name("u32")));
    }
}
