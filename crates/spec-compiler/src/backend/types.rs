//! Declared types (`types` section) to the `types` module.
//!
//! - `id` and `scalar`: a `#[serde(transparent)]` newtype over the representation. One with
//!   `constraints` gets `new` and `validate`, and is decoded through `TryFrom`, so a value read
//!   from the wire always meets them.
//! - `enum`: one unit variant per value, serialised as the specification's exact string, and
//!   `as_str()`.
//! - `struct`: a struct with one field per declared field.
//! - `union`: an enum with one variant per declared variant (unit, `Variant(T)` for a newtype
//!   variant, `Variant(T1, T2, ...)` for a tuple variant, written as a JSON array of its values,
//!   struct-like otherwise), tagged as its `encoding` says: serde's external tagging
//!   (the default), `#[serde(tag)]` (internal) or `#[serde(tag, content)]` (adjacent). Variant
//!   names are written in the `variantCase` (`#[serde(rename)]` where it differs from the Rust
//!   name); fields keep the wire case of the contract that reaches the union.
//!
//! `set<T>` is a `BTreeSet<T>`: a JSON array of distinct items in ascending order. serde's own
//! `BTreeSet` decoding drops a repeated item, so a field that holds a set (also inside a list,
//! an option or a map) is decoded through a generated function that refuses one. A set is
//! declared only in a declared type's fields, and its items are ordered (no `f64` or `json`).
//!
//! `map<K, V>` is a `BTreeMap<K, V>`, so it serialises as a JSON object in key order; integer
//! keys (and ids over integers) are written as decimal strings, as serde_json does. A field that
//! holds a type of its own recursive group inline (directly or through `option<T>`) is boxed;
//! through a list or a map it is not, since those already hold their items on the heap.
//!
//! The module can be split between two crates: the pure crate (`model-core`, see
//! [`crate::model_core`]) holds every declared type ([`is_core_type`]), and the model crate
//! re-exports them and declares the rest ([`Selection`]).

use std::collections::BTreeSet;

use proc_macro2::{Ident, Literal, TokenStream};
use quote::quote;

use super::Backend;
use crate::diagnostic::Diagnostic;
use crate::ir::{
    variant_wire_name, Spec, StructField, TypeConstraints, TypeDecl, TypeKind, UnionTag,
};
use crate::naming;
use crate::render::{self, Doc, Field, Namespace, Scope, Uses};
use crate::resolve::{self, Builtin, TypeRef, BUILTIN_TYPES};
use crate::wire::{self, WireCase};

/// Code of a struct that contains itself through struct fields alone, so it has no finite value.
pub const TYPE_RECURSIVE: &str = "specc.type-recursive";
/// Code of a union whose variants or encoding cannot be written as serde writes them.
pub const TYPE_UNION: &str = "specc.type-union";
/// Code of a map whose key cannot be a JSON object key.
pub const TYPE_MAP_KEY: &str = "specc.type-map-key";
/// Code of `constraints` that cannot be checked as stated.
pub const TYPE_CONSTRAINTS: &str = "specc.type-constraints";
/// Code of a `set<T>` that cannot be generated: its items are not ordered, or it is written
/// outside a declared type's fields.
pub const TYPE_SET: &str = "specc.type-set";

/// The name of the error a constrained type's check returns, declared in the module that holds
/// the constrained types.
pub const VIOLATION: &str = "TypeConstraintViolation";

/// The declared-types backend.
pub struct TypesBackend;

/// Which declared types a `types` module declares.
#[derive(Debug, Clone, Copy)]
pub enum Selection<'a> {
    /// Every declared type (a crate with no pure crate beside it).
    All,
    /// The types of the pure crate: those for which [`is_core_type`] holds.
    Core,
    /// The rest, after `pub use <crate>::types::*;` re-exporting the pure crate's types.
    Rest {
        /// The pure crate's Rust name (`strata_model_core`).
        core_crate: &'a str,
    },
}

impl Backend for TypesBackend {
    fn module(&self) -> &'static str {
        "types"
    }

    fn check(&self, spec: &Spec) -> Vec<Diagnostic> {
        check(spec)
    }

    fn emit(&self, spec: &Spec) -> TokenStream {
        emit(spec, Selection::All)
    }
}

/// The readiness checks of the declared types.
pub fn check(spec: &Spec) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut module = Namespace::new("module types");
    if spec.types.iter().any(|t| !t.constraints.is_empty()) {
        module.claim_type(&mut out, "types", "the constraint error", VIOLATION);
    }
    let builtins = BUILTIN_TYPES.join(", ");
    for t in &spec.types {
        let what = format!("type \"{}\"", t.id);
        if let Some(name) = render::checked_name(&mut out, &t.id, &what, naming::pascal(&t.id)) {
            module.claim_type(&mut out, &t.id, &what, &name);
        }
        if Builtin::parse(&t.id).is_some() {
            out.push(Diagnostic::new(
                "type.shadows-builtin",
                &t.id,
                format!(
                    "Type \"{}\" has the name of a built-in representation type ({builtins}).",
                    t.id
                ),
            ));
        }
        match &t.kind {
            TypeKind::Id { repr } | TypeKind::Scalar { repr } => {
                let article = if matches!(t.kind, TypeKind::Id { .. }) {
                    "an id"
                } else {
                    "a scalar"
                };
                match repr {
                    None => out.push(Diagnostic::new(
                        "type.repr-missing",
                        &t.id,
                        format!(
                            "Type \"{}\" is {article} and needs a \"repr\", one of {builtins}.",
                            t.id
                        ),
                    )),
                    Some(r) if Builtin::parse(r).is_none() => out.push(Diagnostic::new(
                        "type.repr-not-builtin",
                        &t.id,
                        format!(
                            "Type \"{}\" has representation \"{r}\", which is not a built-in type ({builtins}).",
                            t.id
                        ),
                    )),
                    Some(r) if Builtin::parse(r) == Some(Builtin::Json) => {
                        out.push(Diagnostic::new(
                            "type.repr-json",
                            &t.id,
                            format!(
                                "Type \"{}\" is {article} represented as json; an id or scalar has a representation of a single kind, so use json directly where any JSON value is meant.",
                                t.id
                            ),
                        ))
                    }
                    Some(_) => {}
                }
            }
            TypeKind::Enum { values } => {
                if values.is_empty() {
                    out.push(Diagnostic::new(
                        "type.values-missing",
                        &t.id,
                        format!(
                            "Type \"{}\" is an enum and needs a non-empty \"values\".",
                            t.id
                        ),
                    ));
                }
                let mut variants = Namespace::new(format!("enum \"{}\"", t.id));
                for v in values {
                    let what = format!("value \"{v}\" of enum \"{}\"", t.id);
                    if let Some(name) =
                        render::checked_name(&mut out, &t.id, &what, naming::pascal(v))
                    {
                        variants.claim(&mut out, &t.id, &what, &name);
                    }
                }
            }
            TypeKind::Struct { fields } => {
                if fields.is_empty() {
                    out.push(Diagnostic::new(
                        "type.fields-missing",
                        &t.id,
                        format!("Type \"{}\" is a struct and needs \"fields\".", t.id),
                    ));
                }
                check_fields(spec, &mut out, t, &format!("struct \"{}\"", t.id), fields);
                if contains_itself(spec, &t.id) {
                    out.push(Diagnostic::new(
                        TYPE_RECURSIVE,
                        &t.id,
                        format!(
                            "Struct \"{}\" contains itself through struct fields alone, so a value would be infinitely large; close the cycle through a list, option, map or union variant.",
                            t.id
                        ),
                    ));
                }
            }
            TypeKind::Union { variants, encoding } => {
                check_union(spec, &mut out, t, variants, encoding);
            }
        }
        check_constraints(&mut out, t);
    }
    check_map_keys(spec, &mut out);
    check_sets(spec, &mut out);
    out.extend(wire::check(spec));
    out
}

/// True when the type holds a `set<T>` anywhere in its text (not through a declared type).
pub fn has_set(t: &TypeRef) -> bool {
    match t {
        TypeRef::Set(_) => true,
        TypeRef::List(inner) | TypeRef::Option(inner) => has_set(inner),
        TypeRef::Map(k, v) => has_set(k) || has_set(v),
        TypeRef::Builtin(_) | TypeRef::Declared(_) => false,
    }
}

/// The items of every `set<T>` in the type text.
fn set_items(t: &TypeRef, found: &mut Vec<TypeRef>) {
    match t {
        TypeRef::Set(inner) => {
            found.push((**inner).clone());
            set_items(inner, found);
        }
        TypeRef::List(inner) | TypeRef::Option(inner) => set_items(inner, found),
        TypeRef::Map(k, v) => {
            set_items(k, found);
            set_items(v, found);
        }
        TypeRef::Builtin(_) | TypeRef::Declared(_) => {}
    }
}

/// A set's items must be ordered (a `BTreeSet` needs `Ord`), and a set is written only in a
/// declared type's fields, where its decoding refuses a repeated item.
fn check_sets(spec: &Spec, out: &mut Vec<Diagnostic>) {
    let declared: BTreeSet<&str> = spec.types.iter().map(|t| t.id.as_str()).collect();
    for (entity, text) in type_texts(spec) {
        let t = TypeRef::parse(text);
        if !has_set(&t) {
            continue;
        }
        if !declared.contains(entity) {
            out.push(Diagnostic::new(
                TYPE_SET,
                entity,
                format!(
                    "\"{}\" holds a set outside a declared type; declare a struct or union type whose field is the set and use that type here.",
                    text.trim()
                ),
            ));
            continue;
        }
        let mut items = Vec::new();
        set_items(&t, &mut items);
        for item in items {
            if resolve::ref_resolves(spec, &item) && !resolve::is_total(spec, &item) {
                out.push(Diagnostic::new(
                    TYPE_SET,
                    entity,
                    format!(
                        "The items of a set in \"{}\" are of type \"{}\", which has no order (it holds an f64 or json value); a set needs ordered items.",
                        text.trim(),
                        item.text()
                    ),
                ));
            }
        }
    }
}

/// The names and types of the fields of a struct or a union variant.
fn check_fields(
    spec: &Spec,
    out: &mut Vec<Diagnostic>,
    t: &TypeDecl,
    owner: &str,
    fields: &[StructField],
) {
    let mut names = Namespace::new(owner.to_string());
    for f in fields {
        let what = format!("field \"{}\" of {owner}", f.name);
        if let Some(name) = render::checked_name(out, &t.id, &what, naming::snake(&f.name)) {
            names.claim(out, &t.id, &what, &name);
        }
        if !resolve::resolves(spec, &f.type_name) {
            out.push(Diagnostic::new(
                "type.field-unresolved",
                &t.id,
                format!(
                    "Field \"{}\" of {owner} has type \"{}\", which is neither built-in nor a declared type.",
                    f.name, f.type_name
                ),
            ));
        }
    }
}

fn check_union(
    spec: &Spec,
    out: &mut Vec<Diagnostic>,
    t: &TypeDecl,
    variants: &[crate::ir::Variant],
    encoding: &crate::ir::Encoding,
) {
    let id = &t.id;
    let union_error = |out: &mut Vec<Diagnostic>, message: String| {
        out.push(Diagnostic::new(TYPE_UNION, id, message));
    };
    if variants.is_empty() {
        union_error(
            out,
            format!("Type \"{id}\" is a union and needs a non-empty \"variants\"."),
        );
    }
    let mut rust_names = Namespace::new(format!("union \"{id}\""));
    let mut wire_names: Vec<(String, &str)> = Vec::new();
    for v in variants {
        let what = format!("variant \"{}\" of union \"{id}\"", v.name);
        if let Some(name) = render::checked_name(out, id, &what, naming::pascal(&v.name)) {
            rust_names.claim(out, id, &what, &name);
        }
        let wire = variant_wire_name(&v.name, encoding.variant_case);
        if let Some((_, first)) = wire_names.iter().find(|(w, _)| *w == wire) {
            union_error(
                out,
                format!(
                    "Variants \"{first}\" and \"{}\" of union \"{id}\" are both written \"{wire}\" on the wire.",
                    v.name
                ),
            );
        } else {
            wire_names.push((wire, &v.name));
        }
        check_fields(
            spec,
            out,
            t,
            &format!("variant \"{}\" of union \"{id}\"", v.name),
            &v.fields,
        );
        if v.newtype && v.fields.len() != 1 {
            union_error(
                out,
                format!(
                    "Variant \"{}\" of union \"{id}\" is a newtype and needs exactly one field; it has {}.",
                    v.name,
                    v.fields.len()
                ),
            );
        }
        if v.tuple {
            if v.newtype {
                union_error(
                    out,
                    format!(
                        "Variant \"{}\" of union \"{id}\" is both a newtype and a tuple.",
                        v.name
                    ),
                );
            } else if v.fields.len() < 2 {
                union_error(
                    out,
                    format!(
                        "Variant \"{}\" of union \"{id}\" is a tuple and needs two or more fields; it has {}.",
                        v.name,
                        v.fields.len()
                    ),
                );
            }
            if let Some(f) = v.fields.iter().find(|f| f.optional) {
                union_error(
                    out,
                    format!(
                        "Field \"{}\" of tuple variant \"{}\" of union \"{id}\" is optional; a positional field is always written.",
                        f.name, v.name
                    ),
                );
            }
            if encoding.tag == UnionTag::Internal {
                union_error(
                    out,
                    format!(
                        "Variant \"{}\" of union \"{id}\" is a tuple, which an internally tagged union cannot write.",
                        v.name
                    ),
                );
            }
        }
        if encoding.tag == UnionTag::Internal && v.newtype {
            union_error(
                out,
                format!(
                    "Variant \"{}\" of union \"{id}\" is a newtype, which an internally tagged union cannot write.",
                    v.name
                ),
            );
        }
        if encoding.tag == UnionTag::Internal {
            if let Some(tag) = &encoding.tag_field {
                let case = wire::type_cases(spec).get(id).copied().unwrap_or_default();
                if v.fields.iter().any(|f| case.name(&f.name) == *tag) {
                    union_error(
                        out,
                        format!(
                            "Variant \"{}\" of union \"{id}\" has a field written \"{tag}\", which is the tag field of its internal encoding.",
                            v.name
                        ),
                    );
                }
            }
        }
    }
    let tag = encoding.tag;
    let tag_field = encoding.tag_field.as_deref().filter(|f| !f.is_empty());
    let content_field = encoding.content_field.as_deref().filter(|f| !f.is_empty());
    match tag {
        UnionTag::External => {
            if encoding.tag_field.is_some() || encoding.content_field.is_some() {
                union_error(
                    out,
                    format!("Union \"{id}\" has an external tag, which takes no tagField or contentField."),
                );
            }
        }
        UnionTag::Internal => {
            if tag_field.is_none() {
                union_error(
                    out,
                    format!("Union \"{id}\" has an internal tag and needs a tagField."),
                );
            }
            if encoding.content_field.is_some() {
                union_error(
                    out,
                    format!("Union \"{id}\" has an internal tag, which takes no contentField."),
                );
            }
        }
        UnionTag::Adjacent => match (tag_field, content_field) {
            (Some(a), Some(b)) if a == b => union_error(
                out,
                format!("The tagField and contentField of union \"{id}\" are both \"{a}\"."),
            ),
            (Some(_), Some(_)) => {}
            _ => union_error(
                out,
                format!(
                    "Union \"{id}\" has an adjacent tag and needs a tagField and a contentField."
                ),
            ),
        },
    }
}

fn check_constraints(out: &mut Vec<Diagnostic>, t: &TypeDecl) {
    let c = &t.constraints;
    if c.is_empty() {
        return;
    }
    let id = &t.id;
    let error = |out: &mut Vec<Diagnostic>, message: String| {
        out.push(Diagnostic::new(TYPE_CONSTRAINTS, id, message));
    };
    let repr = match &t.kind {
        TypeKind::Id { repr } | TypeKind::Scalar { repr } => {
            repr.as_deref().and_then(Builtin::parse)
        }
        other => {
            error(
                out,
                format!(
                    "Type \"{id}\" is a {} and has \"constraints\"; only an id or scalar type takes them.",
                    other.as_str()
                ),
            );
            return;
        }
    };
    if let Some(p) = &c.pattern {
        if repr != Some(Builtin::String) {
            error(
                out,
                format!("Type \"{id}\" has a pattern, which needs a string representation."),
            );
        }
        if let Err(e) = regex::Regex::new(p) {
            let reason = e
                .to_string()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            error(
                out,
                format!("The pattern of type \"{id}\" does not compile as a Rust regular expression: {reason}"),
            );
        }
    }
    let length = |n: &Option<serde_json::Number>| {
        n.as_ref()
            .map(|n| n.as_u64().filter(|v| *v <= u64::from(u32::MAX)))
    };
    for (name, value) in [
        ("minLength", length(&c.min_length)),
        ("maxLength", length(&c.max_length)),
    ] {
        match value {
            None => {}
            Some(None) => error(
                out,
                format!("The {name} of type \"{id}\" must be a whole number from 0 to 4294967295."),
            ),
            Some(Some(_)) if !matches!(repr, Some(Builtin::String | Builtin::Bytes)) => error(
                out,
                format!(
                    "Type \"{id}\" has a {name}, which needs a string or bytes representation."
                ),
            ),
            Some(Some(_)) => {}
        }
    }
    if let (Some(Some(min)), Some(Some(max))) = (length(&c.min_length), length(&c.max_length)) {
        if min > max {
            error(
                out,
                format!("Type \"{id}\" has a minLength above its maxLength."),
            );
        }
    }
}

/// Every type text of the specification that holds a map, with the entity that writes it.
fn type_texts(spec: &Spec) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    for t in &spec.types {
        match &t.kind {
            TypeKind::Struct { fields } => {
                out.extend(fields.iter().map(|f| (t.id.as_str(), f.type_name.as_str())));
            }
            TypeKind::Union { variants, .. } => out.extend(
                variants
                    .iter()
                    .flat_map(|v| &v.fields)
                    .map(|f| (t.id.as_str(), f.type_name.as_str())),
            ),
            _ => {}
        }
    }
    for r in &spec.records {
        out.extend(
            r.fields
                .iter()
                .map(|f| (r.id.as_str(), f.type_name.as_str())),
        );
    }
    for c in &spec.contracts {
        out.extend(
            c.inputs
                .iter()
                .chain(&c.outputs)
                .filter_map(|f| f.type_name.as_deref())
                .chain(c.errors.iter().filter_map(|e| e.type_name.as_deref()))
                .map(|n| (c.id.as_str(), n)),
        );
    }
    for e in &spec.events {
        out.extend(
            e.payload
                .iter()
                .filter_map(|f| f.type_name.as_deref())
                .map(|n| (e.id.as_str(), n)),
        );
    }
    for b in &spec.machines {
        out.extend(
            b.variables
                .iter()
                .filter_map(|v| v.repr.as_deref())
                .map(|n| (b.id.as_str(), n)),
        );
    }
    out
}

fn check_map_keys(spec: &Spec, out: &mut Vec<Diagnostic>) {
    fn keys(t: &TypeRef, found: &mut Vec<TypeRef>) {
        match t {
            TypeRef::List(inner) | TypeRef::Set(inner) | TypeRef::Option(inner) => {
                keys(inner, found)
            }
            TypeRef::Map(k, v) => {
                found.push((**k).clone());
                keys(k, found);
                keys(v, found);
            }
            TypeRef::Builtin(_) | TypeRef::Declared(_) => {}
        }
    }
    for (entity, text) in type_texts(spec) {
        let mut found = Vec::new();
        keys(&TypeRef::parse(text), &mut found);
        for k in found {
            if resolve::ref_resolves(spec, &k) && !resolve::is_map_key(spec, &k) {
                out.push(Diagnostic::new(
                    TYPE_MAP_KEY,
                    entity,
                    format!(
                        "\"{}\" cannot be the key of a map in \"{}\"; a key is string, an integer type (u8, u16, u32, u64, i32, i64), or an id or scalar type over one.",
                        k.text(),
                        text.trim()
                    ),
                ));
            }
        }
    }
}

/// True when struct `start` reaches itself through struct fields alone (not through an option,
/// a list, a map or a union, any of which can end the value).
fn contains_itself(spec: &Spec, start: &str) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack = direct_structs(spec, start);
    while let Some(id) = stack.pop() {
        if id == start {
            return true;
        }
        if seen.insert(id.clone()) {
            stack.extend(direct_structs(spec, &id));
        }
    }
    false
}

/// The structs a struct holds through a field whose type is exactly a struct's name.
fn direct_structs(spec: &Spec, id: &str) -> Vec<String> {
    let Some(TypeKind::Struct { fields }) = spec.type_decl(id).map(|d| &d.kind) else {
        return Vec::new();
    };
    fields
        .iter()
        .filter_map(|f| match TypeRef::parse(&f.type_name) {
            TypeRef::Declared(n)
                if matches!(
                    spec.type_decl(&n).map(|d| &d.kind),
                    Some(TypeKind::Struct { .. })
                ) =>
            {
                Some(n)
            }
            _ => None,
        })
        .collect()
}

/// The structs and unions a struct or union holds inline: through a field (of the struct or of a
/// variant) whose type is the type's name or `option<...>` of it.
fn inline_types(spec: &Spec, id: &str) -> Vec<String> {
    let fields: Vec<&StructField> = match spec.type_decl(id).map(|d| &d.kind) {
        Some(TypeKind::Struct { fields }) => fields.iter().collect(),
        Some(TypeKind::Union { variants, .. }) => variants.iter().flat_map(|v| &v.fields).collect(),
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    for f in fields {
        let mut t = TypeRef::parse(&f.type_name);
        while let TypeRef::Option(inner) = t {
            t = *inner;
        }
        if let TypeRef::Declared(n) = t {
            if matches!(
                spec.type_decl(&n).map(|d| &d.kind),
                Some(TypeKind::Struct { .. } | TypeKind::Union { .. })
            ) {
                out.push(n);
            }
        }
    }
    out
}

/// True when `from` reaches `to` through inline fields (or is it).
fn reaches_inline(spec: &Spec, from: &str, to: &str) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack = vec![from.to_string()];
    while let Some(id) = stack.pop() {
        if id == to {
            return true;
        }
        if seen.insert(id.clone()) {
            stack.extend(inline_types(spec, &id));
        }
    }
    false
}

/// True when the declared type is in the pure crate. Every declared type is: a `uuid` is a
/// `uuid::Uuid` from the `uuid` crate with its `serde` feature only (no `v4`, so no randomness),
/// which keeps the pure crate pure (KRN-012). The function stays the one place that decides it.
pub fn is_core_type(_spec: &Spec, _id: &str) -> bool {
    true
}

/// True when some declared type of the pure crate reaches a `uuid`, so the crate depends on the
/// `uuid` crate.
pub fn core_reaches_uuid(spec: &Spec) -> bool {
    spec.types.iter().any(|t| {
        is_core_type(spec, &t.id)
            && reaches_uuid(spec, &TypeRef::Declared(t.id.clone()), &mut BTreeSet::new())
    })
}

fn reaches_uuid(spec: &Spec, t: &TypeRef, seen: &mut BTreeSet<String>) -> bool {
    match t {
        TypeRef::Builtin(b) => *b == Builtin::Uuid,
        TypeRef::List(inner) | TypeRef::Set(inner) | TypeRef::Option(inner) => {
            reaches_uuid(spec, inner, seen)
        }
        TypeRef::Map(k, v) => reaches_uuid(spec, k, seen) || reaches_uuid(spec, v, seen),
        TypeRef::Declared(id) => {
            if !seen.insert(id.clone()) {
                return false;
            }
            let Some(decl) = spec.type_decl(id) else {
                return false;
            };
            let fields: Vec<&StructField> = match &decl.kind {
                TypeKind::Id { repr } | TypeKind::Scalar { repr } => {
                    return repr.as_deref() == Some("uuid");
                }
                TypeKind::Enum { .. } => return false,
                TypeKind::Struct { fields } => fields.iter().collect(),
                TypeKind::Union { variants, .. } => {
                    variants.iter().flat_map(|v| &v.fields).collect()
                }
            };
            fields
                .iter()
                .any(|f| reaches_uuid(spec, &TypeRef::parse(&f.type_name), seen))
        }
    }
}

/// The `types` module for the selection.
pub fn emit(spec: &Spec, selection: Selection<'_>) -> TokenStream {
    let mut uses = Uses::default();
    let cases = wire::type_cases(spec);
    let chosen: Vec<&TypeDecl> = spec
        .types
        .iter()
        .filter(|t| match selection {
            Selection::All => true,
            Selection::Core => is_core_type(spec, &t.id),
            Selection::Rest { .. } => !is_core_type(spec, &t.id),
        })
        .collect();
    let mut decoders = Decoders::default();
    let items: Vec<TokenStream> = chosen
        .iter()
        .map(|t| {
            let case = cases.get(&t.id).copied().unwrap_or_default();
            type_item(spec, t, case, &mut uses, &mut decoders)
        })
        .collect();
    let decoders = decoders.tokens();
    let constrained = chosen.iter().any(|t| !t.constraints.is_empty());
    let patterns = chosen.iter().any(|t| t.constraints.pattern.is_some());
    let serde = if items.is_empty() {
        TokenStream::new()
    } else {
        quote! { use serde::{Deserialize, Serialize}; }
    };
    let regex = if patterns {
        quote! {
            use std::sync::LazyLock;

            use regex::Regex;
        }
    } else {
        TokenStream::new()
    };
    let violation = if constrained {
        violation_item()
    } else {
        TokenStream::new()
    };
    let reexport = match selection {
        Selection::Rest { core_crate } => {
            let krate = proc_macro2::Ident::new(core_crate, proc_macro2::Span::call_site());
            quote! {
                #[doc = " The types of the pure crate, which reach no runtime type."]
                pub use #krate::types::*;
            }
        }
        _ => TokenStream::new(),
    };
    let crate_uses = uses.import();
    let kinds = if spec
        .types
        .iter()
        .any(|t| matches!(t.kind, TypeKind::Union { .. }))
    {
        "identifiers, scalars, enumerations, structs and unions"
    } else {
        "identifiers, scalars, enumerations and structs"
    };
    let summary = match selection {
        Selection::Core => {
            format!(" The declared types of the specification: {kinds}.")
        }
        _ => format!(" The declared types of the specification: {kinds}."),
    };
    quote! {
        #![doc = #summary]
        #regex
        #serde
        #crate_uses
        #reexport
        #violation
        #(#items)*
        #decoders
    }
}

/// The functions that decode the set-holding field types of a module, one per type text, and
/// the `DistinctSet` they read a set through.
#[derive(Default)]
struct Decoders {
    functions: std::collections::BTreeMap<String, TokenStream>,
}

impl Decoders {
    /// The name of the function that decodes a field of type `t` (already wrapped in `option<>`
    /// when the field is optional), or `None` when serde's own decoding is enough (no set).
    fn decoder(&mut self, spec: &Spec, t: &TypeRef) -> Option<String> {
        if !has_set(t) {
            return None;
        }
        let mut name = String::from("decode");
        for part in t.text().split(|c: char| !c.is_ascii_alphanumeric()) {
            if !part.is_empty() {
                name.push('_');
                name.push_str(&part.to_ascii_lowercase());
            }
        }
        if !self.functions.contains_key(&name) {
            let mut uses = Uses::default();
            let ty = render::rust_type(spec, t, Scope::Types, &mut uses);
            let raw = raw_type(spec, t);
            let convert = convert(t, quote! { raw });
            let ident = naming::ident(&name);
            let doc = format!(
                " Decodes a `{}`, refusing a set that repeats an item.",
                t.text()
            );
            self.functions.insert(
                name.clone(),
                quote! {
                    #[doc = #doc]
                    fn #ident<'de, D: serde::Deserializer<'de>>(d: D) -> Result<#ty, D::Error> {
                        let raw: #raw = Deserialize::deserialize(d)?;
                        Ok(#convert)
                    }
                },
            );
        }
        Some(name)
    }

    fn tokens(&self) -> TokenStream {
        if self.functions.is_empty() {
            return TokenStream::new();
        }
        let functions = self.functions.values();
        quote! {
            /// A `set<T>` as it is read from the wire: a JSON array of distinct items. A repeated
            /// item is refused, where serde's `BTreeSet` would drop it.
            #[derive(PartialEq, Eq, PartialOrd, Ord)]
            struct DistinctSet<T>(std::collections::BTreeSet<T>);

            impl<'de, T: Deserialize<'de> + Ord> Deserialize<'de> for DistinctSet<T> {
                fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                    struct Items<T>(std::marker::PhantomData<T>);

                    impl<'de, T: Deserialize<'de> + Ord> serde::de::Visitor<'de> for Items<T> {
                        type Value = DistinctSet<T>;

                        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                            f.write_str("an array of distinct items")
                        }

                        fn visit_seq<A: serde::de::SeqAccess<'de>>(
                            self,
                            mut seq: A,
                        ) -> Result<Self::Value, A::Error> {
                            let mut items = std::collections::BTreeSet::new();
                            let mut index = 0usize;
                            while let Some(item) = seq.next_element::<T>()? {
                                if !items.insert(item) {
                                    return Err(serde::de::Error::custom(format!(
                                        "item {index} of a set repeats an earlier item"
                                    )));
                                }
                                index += 1;
                            }
                            Ok(DistinctSet(items))
                        }
                    }

                    d.deserialize_seq(Items(std::marker::PhantomData))
                }
            }

            #(#functions)*
        }
    }
}

/// The type a set-holding type is first decoded as: every `set<T>` read as a `DistinctSet`.
fn raw_type(spec: &Spec, t: &TypeRef) -> TokenStream {
    let mut uses = Uses::default();
    if !has_set(t) {
        return render::rust_type(spec, t, Scope::Types, &mut uses);
    }
    match t {
        TypeRef::Set(inner) => {
            let inner = raw_type(spec, inner);
            quote! { DistinctSet<#inner> }
        }
        TypeRef::List(inner) => {
            let inner = raw_type(spec, inner);
            quote! { Vec<#inner> }
        }
        TypeRef::Option(inner) => {
            let inner = raw_type(spec, inner);
            quote! { Option<#inner> }
        }
        TypeRef::Map(k, v) => {
            let k = render::rust_type(spec, k, Scope::Types, &mut uses);
            let v = raw_type(spec, v);
            quote! { std::collections::BTreeMap<#k, #v> }
        }
        TypeRef::Builtin(_) | TypeRef::Declared(_) => {
            render::rust_type(spec, t, Scope::Types, &mut uses)
        }
    }
}

/// The expression that turns `value`, decoded as [`raw_type`], into the field's type.
fn convert(t: &TypeRef, value: TokenStream) -> TokenStream {
    if !has_set(t) {
        return value;
    }
    let item = |inner: &TypeRef| convert(inner, quote! { x });
    match t {
        TypeRef::Set(inner) if has_set(inner) => {
            let item = item(inner);
            quote! { #value.0.into_iter().map(|x| #item).collect() }
        }
        TypeRef::Set(_) => quote! { #value.0 },
        TypeRef::List(inner) => {
            let item = item(inner);
            quote! { #value.into_iter().map(|x| #item).collect() }
        }
        TypeRef::Option(inner) => {
            let item = item(inner);
            quote! { #value.map(|x| #item) }
        }
        TypeRef::Map(_, v) => {
            let item = item(v);
            quote! { #value.into_iter().map(|(k, x)| (k, #item)).collect() }
        }
        TypeRef::Builtin(_) | TypeRef::Declared(_) => value,
    }
}

/// The error of a constrained type's check.
fn violation_item() -> TokenStream {
    let name = naming::ident(VIOLATION);
    quote! {
        /// A value of a declared type that breaks one of the type's constraints.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct #name {
            /// The type, by its specification id.
            pub type_id: &'static str,
            /// The constraint broken: `pattern`, `minLength` or `maxLength`.
            pub constraint: &'static str,
        }

        impl std::fmt::Display for #name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "a value of type {} breaks its {} constraint", self.type_id, self.constraint)
            }
        }

        impl std::error::Error for #name {}
    }
}

/// `case` names the fields of a struct or a union's variants on the wire ([`crate::wire`]).
fn type_item(
    spec: &Spec,
    t: &TypeDecl,
    case: WireCase,
    uses: &mut Uses,
    decoders: &mut Decoders,
) -> TokenStream {
    let name = naming::ident(&naming::pascal(&t.id).unwrap_or_default());
    let mut doc = Doc::new(&t.name);
    if let Some(d) = &t.description {
        doc.paragraph(d);
    }
    match &t.kind {
        TypeKind::Id { repr } | TypeKind::Scalar { repr } => {
            let repr_name = repr.as_deref().unwrap_or_default();
            let what = if matches!(t.kind, TypeKind::Id { .. }) {
                "an identifier"
            } else {
                "a scalar"
            };
            doc.paragraph(&format!(
                "Specification type `{}`: {what} represented as `{repr_name}`.",
                t.id
            ));
            let builtin = Builtin::parse(repr_name).unwrap_or(Builtin::String);
            let ty = render::builtin_type(builtin, uses);
            let derives = render::derives(builtin.is_total());
            if t.constraints.is_empty() {
                let doc = doc.tokens();
                return quote! {
                    #doc
                    #derives
                    #[serde(transparent)]
                    pub struct #name(pub #ty);
                };
            }
            constrained_item(t, &name, doc, derives, ty, builtin)
        }
        TypeKind::Enum { values } => {
            doc.paragraph(&format!(
                "Specification type `{}`: an enumeration, serialised as the specification's values.",
                t.id
            ));
            let derives = render::derives(true);
            let doc = doc.tokens();
            let variants: Vec<_> = values
                .iter()
                .map(|v| naming::ident(&naming::pascal(v).unwrap_or_default()))
                .collect();
            let docs: Vec<String> = values
                .iter()
                .map(|v| format!(" The value `{v}`."))
                .collect();
            quote! {
                #doc
                #derives
                pub enum #name {
                    #(
                        #[doc = #docs]
                        #[serde(rename = #values)]
                        #variants,
                    )*
                }

                impl #name {
                    /// The value as the specification writes it.
                    pub fn as_str(&self) -> &'static str {
                        match self {
                            #( Self::#variants => #values, )*
                        }
                    }
                }
            }
        }
        TypeKind::Struct { fields } => {
            doc.paragraph(&format!("Specification type `{}`: a struct.", t.id));
            let total = resolve::is_total(spec, &TypeRef::Declared(t.id.clone()));
            let derives = render::derives(total);
            let doc = doc.tokens();
            let fields: Vec<TokenStream> = fields
                .iter()
                .map(|f| field(spec, t, f, case, uses, decoders).tokens())
                .collect();
            quote! {
                #doc
                #derives
                pub struct #name {
                    #(#fields)*
                }
            }
        }
        TypeKind::Union { variants, encoding } => {
            let tagging = match encoding.tag {
                UnionTag::External => "externally tagged".to_string(),
                UnionTag::Internal => format!(
                    "internally tagged by `{}`",
                    encoding.tag_field.as_deref().unwrap_or_default()
                ),
                UnionTag::Adjacent => format!(
                    "adjacently tagged by `{}` with content `{}`",
                    encoding.tag_field.as_deref().unwrap_or_default(),
                    encoding.content_field.as_deref().unwrap_or_default()
                ),
            };
            doc.paragraph(&format!(
                "Specification type `{}`: a union, {tagging}.",
                t.id
            ));
            let total = resolve::is_total(spec, &TypeRef::Declared(t.id.clone()));
            let derives = render::derives(total);
            let doc = doc.tokens();
            let container = match encoding.tag {
                UnionTag::External => TokenStream::new(),
                UnionTag::Internal => {
                    let tag = encoding.tag_field.as_deref().unwrap_or_default();
                    quote! { #[serde(tag = #tag)] }
                }
                UnionTag::Adjacent => {
                    let tag = encoding.tag_field.as_deref().unwrap_or_default();
                    let content = encoding.content_field.as_deref().unwrap_or_default();
                    quote! { #[serde(tag = #tag, content = #content)] }
                }
            };
            let items: Vec<TokenStream> = variants
                .iter()
                .map(|v| {
                    let rust = naming::pascal(&v.name).unwrap_or_default();
                    let ident = naming::ident(&rust);
                    let wire = variant_wire_name(&v.name, encoding.variant_case);
                    let rename = if wire == rust {
                        TokenStream::new()
                    } else {
                        quote! { #[serde(rename = #wire)] }
                    };
                    let vdoc = format!(" The variant `{}`.", v.name);
                    if v.fields.is_empty() {
                        quote! {
                            #[doc = #vdoc]
                            #rename
                            #ident,
                        }
                    } else if v.newtype || v.tuple {
                        let positional: Vec<TokenStream> = v
                            .fields
                            .iter()
                            .map(|f| {
                                let mut ty = field_type(spec, t, f, uses);
                                let mut text = TypeRef::parse(&f.type_name);
                                if f.optional {
                                    ty = quote! { Option<#ty> };
                                    text = TypeRef::Option(Box::new(text));
                                }
                                let decode = match decoders.decoder(spec, &text) {
                                    Some(path) => quote! { #[serde(deserialize_with = #path)] },
                                    None => TokenStream::new(),
                                };
                                quote! { #decode #ty }
                            })
                            .collect();
                        quote! {
                            #[doc = #vdoc]
                            #rename
                            #ident(#(#positional),*),
                        }
                    } else {
                        let fields: Vec<TokenStream> = v
                            .fields
                            .iter()
                            .map(|f| field(spec, t, f, case, uses, decoders).variant_tokens())
                            .collect();
                        quote! {
                            #[doc = #vdoc]
                            #rename
                            #ident {
                                #(#fields)*
                            },
                        }
                    }
                })
                .collect();
            quote! {
                #doc
                #derives
                #container
                pub enum #name {
                    #(#items)*
                }
            }
        }
    }
}

/// The Rust type of a field of `owner`, boxed where it holds a type of `owner`'s recursive group
/// inline.
fn field_type(spec: &Spec, owner: &TypeDecl, f: &StructField, uses: &mut Uses) -> TokenStream {
    let boxed = |id: &str| reaches_inline(spec, id, &owner.id);
    render::rust_type_boxed(
        spec,
        &TypeRef::parse(&f.type_name),
        Scope::Types,
        uses,
        &boxed,
    )
}

fn field(
    spec: &Spec,
    owner: &TypeDecl,
    f: &StructField,
    case: WireCase,
    uses: &mut Uses,
    decoders: &mut Decoders,
) -> Field {
    let mut text = TypeRef::parse(&f.type_name);
    if f.optional {
        text = TypeRef::Option(Box::new(text));
    }
    Field {
        wire_name: case.name(&f.name),
        rust_name: naming::snake(&f.name).unwrap_or_default(),
        ty: field_type(spec, owner, f, uses),
        optional: f.optional,
        doc: Doc::new(&format!("Field `{}`.", f.name)),
        deserialize_with: decoders.decoder(spec, &text),
    }
}

/// An id or scalar with `constraints`: decoded through `TryFrom` so a wire value always meets
/// them, with `new` and `validate`.
fn constrained_item(
    t: &TypeDecl,
    name: &Ident,
    mut doc: Doc,
    derives: TokenStream,
    ty: TokenStream,
    builtin: Builtin,
) -> TokenStream {
    let c: &TypeConstraints = &t.constraints;
    let mut stated = Vec::new();
    if let Some(p) = &c.pattern {
        stated.push(format!("matches `{p}`"));
    }
    if let Some(n) = &c.min_length {
        stated.push(format!("has a length of at least {n}"));
    }
    if let Some(n) = &c.max_length {
        stated.push(format!("has a length of at most {n}"));
    }
    doc.paragraph(&format!(
        "A value {}; decoding refuses one that does not, and [`{}::new`] checks it.",
        stated.join(" and "),
        naming::pascal(&t.id).unwrap_or_default()
    ));
    let doc = doc.tokens();
    let repr_text = match builtin {
        Builtin::Bytes => "Vec<u8>",
        _ => "String",
    };
    let id = &t.id;
    let violation = naming::ident(VIOLATION);
    let fail = |constraint: &str| {
        quote! {
            return Err(#violation { type_id: #id, constraint: #constraint });
        }
    };
    let length = if builtin == Builtin::Bytes {
        quote! { self.0.len() }
    } else {
        quote! { self.0.chars().count() }
    };
    let mut checks = TokenStream::new();
    let mut statics = TokenStream::new();
    if let Some(n) = c.min_length.as_ref().and_then(serde_json::Number::as_u64) {
        let lit = Literal::u64_unsuffixed(n);
        let fail = fail("minLength");
        checks.extend(quote! { if #length < #lit { #fail } });
    }
    if let Some(n) = c.max_length.as_ref().and_then(serde_json::Number::as_u64) {
        let lit = Literal::u64_unsuffixed(n);
        let fail = fail("maxLength");
        checks.extend(quote! { if #length > #lit { #fail } });
    }
    if let Some(p) = &c.pattern {
        let pattern = naming::ident(&format!(
            "{}_PATTERN",
            naming::upper_snake(&t.id).unwrap_or_default()
        ));
        let pdoc = format!(" The pattern of type `{id}`.");
        statics.extend(quote! {
            #[doc = #pdoc]
            static #pattern: LazyLock<Regex> =
                LazyLock::new(|| Regex::new(#p).expect("specc checked that the pattern compiles"));
        });
        let fail = fail("pattern");
        checks.extend(quote! { if !#pattern.is_match(&self.0) { #fail } });
    }
    let repr_ty: TokenStream = repr_text.parse().unwrap_or_default();
    quote! {
        #statics

        #doc
        #derives
        #[serde(try_from = #repr_text, into = #repr_text)]
        pub struct #name(pub #ty);

        impl #name {
            /// The value, when it meets the type's constraints.
            pub fn new(value: #repr_ty) -> Result<Self, #violation> {
                let value = Self(value);
                value.validate()?;
                Ok(value)
            }

            /// Checks the type's constraints.
            pub fn validate(&self) -> Result<(), #violation> {
                #checks
                Ok(())
            }
        }

        impl TryFrom<#repr_ty> for #name {
            type Error = #violation;

            fn try_from(value: #repr_ty) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<#name> for #repr_ty {
            fn from(value: #name) -> Self {
                value.0
            }
        }
    }
}
