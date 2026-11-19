//! Records (`records` section) to the `records` module.
//!
//! Each record becomes a struct with its key struct (`<Record>Key`, from `key()`), a
//! `validate()` that checks the field constraints, and associated constants that describe what
//! storage needs: the schema version, key, unique sets, indexes and references.

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use super::Backend;
use crate::diagnostic::Diagnostic;
use crate::ir::{Record, RecordField, Spec, TypeKind};
use crate::naming;
use crate::render::{self, Doc, Field, Namespace, Scope, Uses};
use crate::resolve::{self, Builtin, TypeRef};

/// Code of a record that states no key.
pub const KEY_MISSING: &str = "specc.record-key-missing";
/// Code of a reference to a record that is not declared.
pub const REFERS_TO_UNKNOWN: &str = "specc.refers-to-unknown";
/// Code of a constraint that does not apply to its field's type.
pub const CONSTRAINT_TYPE: &str = "specc.constraint-type";
/// Code of a constraint whose value cannot hold for the field's type.
pub const CONSTRAINT_VALUE: &str = "specc.constraint-value";
/// Code of a pattern that is not a valid regular expression.
pub const PATTERN_INVALID: &str = "specc.pattern-invalid";

/// The records backend.
pub struct RecordsBackend;

impl Backend for RecordsBackend {
    fn module(&self) -> &'static str {
        "records"
    }

    fn check(&self, spec: &Spec) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut module = Namespace::new("module records");
        let mut statics = Namespace::new("the statics of module records");
        for r in &spec.records {
            check_record(spec, r, &mut module, &mut statics, &mut out);
        }
        out
    }

    fn emit(&self, spec: &Spec) -> TokenStream {
        let mut uses = Uses::default();
        let mut statics = Vec::new();
        let items: Vec<TokenStream> = spec
            .records
            .iter()
            .map(|r| record_items(spec, r, &mut uses, &mut statics))
            .collect();
        let pattern_uses = if statics.is_empty() {
            TokenStream::new()
        } else {
            quote! {
                use std::sync::LazyLock;

                use regex::Regex;
            }
        };
        let serde = if items.is_empty() {
            TokenStream::new()
        } else {
            quote! { use serde::{Deserialize, Serialize}; }
        };
        let crate_uses = uses.import();
        quote! {
            #![doc = " The stored records of the specification: their shapes, keys and constraints, and what storage needs to know about them."]
            #pattern_uses
            #serde
            #crate_uses
            #(#statics)*
            #(#items)*
        }
    }
}

fn check_record(
    spec: &Spec,
    r: &Record,
    module: &mut Namespace,
    statics: &mut Namespace,
    out: &mut Vec<Diagnostic>,
) {
    let what = format!("record \"{}\"", r.id);
    if let Some(name) = render::checked_name(out, &r.id, &what, naming::pascal(&r.id)) {
        module.claim_type(out, &r.id, &what, &name);
        module.claim_type(
            out,
            &r.id,
            &format!("the key of {what}"),
            &format!("{name}Key"),
        );
    }

    match &r.schema_version {
        None => out.push(Diagnostic::new(
            "ready.schema-version-missing",
            &r.id,
            format!("Record \"{}\" states no schemaVersion.", r.id),
        )),
        Some(n) => {
            if !n
                .as_u64()
                .is_some_and(|v| v >= 1 && v <= u64::from(u32::MAX))
            {
                out.push(Diagnostic::new(
                    "record.schema-version",
                    &r.id,
                    format!(
                        "The schema version of record \"{}\" must be an integer from 1 to {}.",
                        r.id,
                        u32::MAX
                    ),
                ));
            }
        }
    }

    match &r.key {
        None => out.push(Diagnostic::new(
            KEY_MISSING,
            &r.id,
            format!(
                "Record \"{}\" states no \"key\", so it has no key struct.",
                r.id
            ),
        )),
        Some(key) if key.is_empty() => out.push(Diagnostic::new(
            KEY_MISSING,
            &r.id,
            format!("Record \"{}\" has an empty \"key\".", r.id),
        )),
        Some(key) => {
            for k in key {
                if r.field(k).is_none() {
                    out.push(Diagnostic::new(
                        "record.key-field",
                        &r.id,
                        format!(
                            "Key field \"{k}\" of record \"{}\" is not one of its fields.",
                            r.id
                        ),
                    ));
                }
            }
        }
    }

    let mut fields = Namespace::new(format!("struct of {what}"));
    let mut seen = std::collections::BTreeSet::new();
    for f in &r.fields {
        if !seen.insert(f.name.as_str()) {
            out.push(Diagnostic::new(
                "record.field-duplicate",
                &r.id,
                format!("Record \"{}\" declares field \"{}\" twice.", r.id, f.name),
            ));
            continue;
        }
        let fwhat = format!("field \"{}\" of {what}", f.name);
        if let Some(name) = render::checked_name(out, &r.id, &fwhat, naming::snake(&f.name)) {
            fields.claim(out, &r.id, &fwhat, &name);
        }
        check_field(spec, r, f, out);
        if f.constraints.pattern.is_some() {
            if let Some(name) = pattern_static_name(r, f) {
                statics.claim(out, &r.id, &format!("the pattern of {fwhat}"), &name);
            }
        }
    }

    for (i, set) in r.unique.iter().enumerate() {
        for n in set {
            if r.field(n).is_none() {
                out.push(Diagnostic::new(
                    "record.unique-field",
                    &r.id,
                    format!(
                        "Unique set {} of record \"{}\" names \"{n}\", which is not one of its fields.",
                        i + 1,
                        r.id
                    ),
                ));
            }
        }
    }
    for ix in &r.indexes {
        for n in &ix.fields {
            if r.field(n).is_none() {
                out.push(Diagnostic::new(
                    "record.index-field",
                    &r.id,
                    format!(
                        "Index \"{}\" of record \"{}\" names \"{n}\", which is not one of its fields.",
                        ix.name, r.id
                    ),
                ));
            }
        }
    }
}

fn check_field(spec: &Spec, r: &Record, f: &RecordField, out: &mut Vec<Diagnostic>) {
    let resolves = resolve::resolves(spec, &f.type_name);
    if !resolves {
        out.push(Diagnostic::new(
            "ready.field-type-undeclared",
            &r.id,
            format!(
                "Field \"{}\" of record \"{}\" has {}.",
                f.name,
                r.id,
                resolve::unresolved_text(Some(&f.type_name))
            ),
        ));
    }
    match (&f.refers_to, f.on_delete) {
        (Some(target), None) => out.push(Diagnostic::new(
            "ready.on-delete-missing",
            &r.id,
            format!(
                "Field \"{}\" of record \"{}\" refers to \"{target}\" but says nothing about what deleting it does (\"onDelete\").",
                f.name, r.id
            ),
        )),
        (None, Some(_)) => out.push(Diagnostic::new(
            "record.on-delete-without-refers-to",
            &r.id,
            format!(
                "Field \"{}\" of record \"{}\" has \"onDelete\" but no \"refersTo\".",
                f.name, r.id
            ),
        )),
        _ => {}
    }
    if f.on_delete == Some(crate::ir::OnDelete::SetNull) && !f.optional {
        out.push(Diagnostic::new(
            "record.set-null-required",
            &r.id,
            format!(
                "Field \"{}\" of record \"{}\" is \"set-null\" on delete, so it must be \"optional\": true.",
                f.name, r.id
            ),
        ));
    }
    if let Some(target) = &f.refers_to {
        if target != "principal" && spec.record(target).is_none() {
            out.push(Diagnostic::new(
                REFERS_TO_UNKNOWN,
                &r.id,
                format!(
                    "Field \"{}\" of record \"{}\" refers to \"{target}\", which is not a record.",
                    f.name, r.id
                ),
            ));
        }
    }
    if resolves && !f.constraints.is_empty() {
        check_constraints(spec, r, f, out);
    }
}

/// The steps from a field to the value its constraints apply to, and that value's
/// representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Unwrap an `Option`.
    Option,
    /// Take the `.0` of a declared newtype.
    Newtype,
}

/// The path to the constrained value of a field, or `None` when the type has no single
/// built-in value to constrain (enums, structs, lists).
fn constraint_base(spec: &Spec, f: &RecordField) -> Option<(Vec<Step>, Builtin)> {
    let mut steps = Vec::new();
    if f.optional {
        steps.push(Step::Option);
    }
    let mut t = TypeRef::parse(&f.type_name);
    loop {
        match t {
            TypeRef::Builtin(b) => return Some((steps, b)),
            TypeRef::Option(inner) => {
                steps.push(Step::Option);
                t = *inner;
            }
            TypeRef::List(_) | TypeRef::Set(_) | TypeRef::Map(..) | TypeRef::Shared(_) => {
                return None
            }
            TypeRef::Declared(id) => {
                let decl = spec.type_decl(&id)?;
                let (TypeKind::Id { repr } | TypeKind::Scalar { repr }) = &decl.kind else {
                    return None;
                };
                steps.push(Step::Newtype);
                return Some((steps, Builtin::parse(repr.as_deref()?)?));
            }
        }
    }
}

/// A number as an exact integer, when it is one.
fn as_integer(n: &serde_json::Number) -> Option<i128> {
    if let Some(i) = n.as_i64() {
        return Some(i128::from(i));
    }
    if let Some(u) = n.as_u64() {
        return Some(i128::from(u));
    }
    let f = n.as_f64()?;
    // Integral and exactly representable: below 2^53 every integer is exact in an f64.
    if f.fract() == 0.0 && f.abs() < 9_007_199_254_740_992.0 {
        return Some(f as i128);
    }
    None
}

/// A min or max bound, resolved for the field's representation.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Bound {
    /// A bound on an integer representation.
    Int(i128),
    /// A bound on an `f64`.
    Float(f64),
}

/// The bound to check, `None` when it can never be broken (a minimum at or below the type's
/// least value, a maximum at or above its greatest), or `Err` when it cannot hold.
fn bound(n: &serde_json::Number, base: Builtin, is_min: bool) -> Result<Option<Bound>, String> {
    if base == Builtin::F64 {
        return match n.as_f64() {
            Some(f) if f.is_finite() => Ok(Some(Bound::Float(f))),
            _ => Err(format!("{n} is not a finite number")),
        };
    }
    let Some((lo, hi)) = base.integer_range() else {
        return Err(format!("a {} value has no order", base.as_str()));
    };
    let Some(v) = as_integer(n) else {
        return Err(format!(
            "{n} is not an integer, and the field holds {}",
            base.as_str()
        ));
    };
    if is_min {
        if v <= lo {
            return Ok(None);
        }
        if v > hi {
            return Err(format!("no {} value is at least {v}", base.as_str()));
        }
    } else {
        if v >= hi {
            return Ok(None);
        }
        if v < lo {
            return Err(format!("no {} value is at most {v}", base.as_str()));
        }
    }
    Ok(Some(Bound::Int(v)))
}

fn check_constraints(spec: &Spec, r: &Record, f: &RecordField, out: &mut Vec<Diagnostic>) {
    let c = &f.constraints;
    let at = format!("field \"{}\" of record \"{}\"", f.name, r.id);
    let Some((_, base)) = constraint_base(spec, f) else {
        out.push(Diagnostic::new(
            CONSTRAINT_TYPE,
            &r.id,
            format!(
                "The constraints of {at} apply to a number or a string, but its type \"{}\" is neither.",
                f.type_name
            ),
        ));
        return;
    };
    let numeric = base == Builtin::F64 || base.integer_range().is_some();
    let mut bounds = [None, None];
    for (i, (name, value)) in [("min", &c.min), ("max", &c.max)].into_iter().enumerate() {
        let Some(value) = value else { continue };
        if !numeric {
            out.push(Diagnostic::new(
                CONSTRAINT_TYPE,
                &r.id,
                format!(
                    "\"{name}\" on {at} needs a number, but the field holds {}.",
                    base.as_str()
                ),
            ));
            continue;
        }
        match bound(value, base, i == 0) {
            Ok(b) => bounds[i] = Some((value, b)),
            Err(reason) => out.push(Diagnostic::new(
                CONSTRAINT_VALUE,
                &r.id,
                format!("\"{name}\" on {at} cannot hold: {reason}."),
            )),
        }
    }
    if let (Some((min, _)), Some((max, _))) = (bounds[0], bounds[1]) {
        if min.as_f64().unwrap_or(f64::NAN) > max.as_f64().unwrap_or(f64::NAN) {
            out.push(Diagnostic::new(
                CONSTRAINT_VALUE,
                &r.id,
                format!("\"min\" {min} on {at} is greater than \"max\" {max}."),
            ));
        }
    }
    for (name, present) in [
        ("maxLength", c.max_length.is_some()),
        ("pattern", c.pattern.is_some()),
    ] {
        if present && base != Builtin::String {
            out.push(Diagnostic::new(
                CONSTRAINT_TYPE,
                &r.id,
                format!(
                    "\"{name}\" on {at} needs a string, but the field holds {}.",
                    base.as_str()
                ),
            ));
        }
    }
    if let Some(n) = &c.max_length {
        if n.as_u64().is_none_or(|v| v > u64::from(u32::MAX)) {
            out.push(Diagnostic::new(
                CONSTRAINT_VALUE,
                &r.id,
                format!(
                    "\"maxLength\" on {at} must be an integer from 0 to {}; found {n}.",
                    u32::MAX
                ),
            ));
        }
    }
    if let Some(p) = &c.pattern {
        if let Err(e) = regex::Regex::new(p) {
            let reason = e
                .to_string()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            out.push(Diagnostic::new(
                PATTERN_INVALID,
                &r.id,
                format!("The pattern of {at} is not a valid regular expression: {reason}"),
            ));
        }
    }
}

fn pattern_static_name(r: &Record, f: &RecordField) -> Option<String> {
    let record = naming::upper_snake(&r.id).ok()?;
    let field = naming::upper_snake(&f.name).ok()?;
    Some(format!("{record}_{field}_PATTERN"))
}

fn str_list(items: &[String]) -> TokenStream {
    quote! { &[#(#items),*] }
}

fn field_doc(f: &RecordField) -> Doc {
    let mut doc = Doc::new(
        f.description
            .as_deref()
            .unwrap_or(&format!("Field `{}`.", f.name)),
    );
    if let Some(class) = &f.personal {
        doc.paragraph(&format!("Personal data: {class}."));
    }
    if let Some(target) = &f.refers_to {
        let on_delete = f.on_delete.map_or("unstated", |d| d.as_str());
        if target == "principal" {
            doc.paragraph(&format!("Holds a principal id. On delete: {on_delete}."));
        } else {
            doc.paragraph(&format!(
                "Holds the key of record `{target}`. On delete: {on_delete}."
            ));
        }
    }
    let c = &f.constraints;
    let mut parts = Vec::new();
    if let Some(n) = &c.min {
        parts.push(format!("min {n}"));
    }
    if let Some(n) = &c.max {
        parts.push(format!("max {n}"));
    }
    if let Some(n) = &c.max_length {
        parts.push(format!("at most {n} characters"));
    }
    if let Some(p) = &c.pattern {
        parts.push(format!("matches `{p}`"));
    }
    if !parts.is_empty() {
        doc.paragraph(&format!("Constraints: {}.", parts.join(", ")));
    }
    doc
}

fn record_items(
    spec: &Spec,
    r: &Record,
    uses: &mut Uses,
    statics: &mut Vec<TokenStream>,
) -> TokenStream {
    // Every record refers to the prelude through `validate` and its constants.
    uses.prelude = true;
    let pascal = naming::pascal(&r.id).unwrap_or_default();
    let name = naming::ident(&pascal);
    let key_name = naming::ident(&format!("{pascal}Key"));
    let id = &r.id;

    let mut doc = Doc::new(&r.name);
    if let Some(d) = &r.description {
        doc.paragraph(d);
    }
    if let Some(retention) = &r.retention {
        doc.paragraph(&format!("Retention: {retention}"));
    }
    let version = r
        .schema_version
        .as_ref()
        .and_then(serde_json::Number::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(1);
    let version_lit = Literal::u32_unsuffixed(version);
    let owner = r
        .owner
        .as_deref()
        .map(|o| format!(", owned by component `{o}`"))
        .unwrap_or_default();
    doc.paragraph(&format!("Record `{id}`{owner}, schema version {version}."));

    let field_type = |f: &RecordField, uses: &mut Uses| {
        render::rust_type(spec, &TypeRef::parse(&f.type_name), Scope::Other, uses)
    };
    let field_tokens = |f: &RecordField, uses: &mut Uses| {
        Field {
            wire_name: f.name.clone(),
            rust_name: naming::snake(&f.name).unwrap_or_default(),
            ty: field_type(f, uses),
            optional: f.optional,
            doc: field_doc(f),
            deserialize_with: None,
        }
        .tokens()
    };
    let total_field = |f: &RecordField| resolve::is_total(spec, &TypeRef::parse(&f.type_name));

    let fields: Vec<TokenStream> = r.fields.iter().map(|f| field_tokens(f, uses)).collect();
    let total = r.fields.iter().all(total_field);
    let derives = render::derives(total);
    let doc = doc.tokens();

    let key_names: Vec<String> = r.key.clone().unwrap_or_default();
    let key_fields: Vec<&RecordField> = key_names.iter().filter_map(|k| r.field(k)).collect();
    let key_struct_fields: Vec<TokenStream> =
        key_fields.iter().map(|f| field_tokens(f, uses)).collect();
    let key_total = key_fields.iter().all(|f| total_field(f));
    let key_derives = render::derives(key_total);
    let key_doc = Doc::new(&format!(
        "The key of a [`{pascal}`]: {}.",
        key_names
            .iter()
            .map(|k| format!("`{k}`"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
    .tokens();
    let key_inits: Vec<TokenStream> = key_fields
        .iter()
        .map(|f| {
            let field = naming::ident(&naming::snake(&f.name).unwrap_or_default());
            if resolve::is_copy(spec, &TypeRef::parse(&f.type_name)) {
                quote! { #field: self.#field }
            } else {
                quote! { #field: self.#field.clone() }
            }
        })
        .collect();

    let unique: Vec<TokenStream> = r.unique.iter().map(|set| str_list(set)).collect();
    let indexes: Vec<TokenStream> = r
        .indexes
        .iter()
        .map(|ix| {
            let ix_name = &ix.name;
            let ix_fields = str_list(&ix.fields);
            quote! { prelude::Index { name: #ix_name, fields: #ix_fields } }
        })
        .collect();
    let references: Vec<TokenStream> = r
        .fields
        .iter()
        .filter_map(|f| {
            let target = f.refers_to.as_ref()?;
            let field = &f.name;
            let on_delete = match f.on_delete? {
                crate::ir::OnDelete::Restrict => quote! { prelude::OnDelete::Restrict },
                crate::ir::OnDelete::Cascade => quote! { prelude::OnDelete::Cascade },
                crate::ir::OnDelete::SetNull => quote! { prelude::OnDelete::SetNull },
            };
            Some(quote! {
                prelude::Reference { field: #field, record: #target, on_delete: #on_delete }
            })
        })
        .collect();

    let checks: Vec<TokenStream> = r
        .fields
        .iter()
        .filter(|f| !f.constraints.is_empty())
        .filter_map(|f| field_checks(spec, r, f, statics))
        .collect();

    quote! {
        #doc
        #derives
        pub struct #name {
            #(#fields)*
        }

        #key_doc
        #key_derives
        pub struct #key_name {
            #(#key_struct_fields)*
        }

        impl #name {
            /// The record's id in the specification.
            pub const ID: &str = #id;
            /// The version of the record's stored shape.
            pub const SCHEMA_VERSION: u32 = #version_lit;
            /// The key fields, by their specification names.
            pub const KEY: &[&str] = &[#(#key_names),*];
            /// Sets of fields whose combined values are unique, by their specification names.
            pub const UNIQUE: &[&[&str]] = &[#(#unique),*];
            /// The record's indexes.
            pub const INDEXES: &[prelude::Index] = &[#(#indexes),*];
            /// The fields that hold the key of another record, and what deleting it does.
            pub const REFERENCES: &[prelude::Reference] = &[#(#references),*];

            /// The record's key.
            pub fn key(&self) -> #key_name {
                #key_name {
                    #(#key_inits,)*
                }
            }

            /// Checks the field constraints, returning the first one the record breaks.
            pub fn validate(&self) -> Result<(), prelude::ConstraintViolation> {
                #(#checks)*
                Ok(())
            }
        }
    }
}

/// How the constrained value is reached in the generated code.
#[derive(Clone)]
enum Access {
    /// A place expression of the value's type, such as `self.age`.
    Place(TokenStream),
    /// The identifier `value`, a reference to the value.
    Ref,
}

/// The checks of one field, or `None` when it has no constraint that can be broken.
fn field_checks(
    spec: &Spec,
    r: &Record,
    f: &RecordField,
    statics: &mut Vec<TokenStream>,
) -> Option<TokenStream> {
    let (steps, base) = constraint_base(spec, f)?;
    let c = &f.constraints;
    let field_name = &f.name;
    let violation = |kind: TokenStream| {
        quote! {
            return Err(prelude::ConstraintViolation {
                record: Self::ID,
                field: #field_name,
                constraint: prelude::Constraint::#kind,
            });
        }
    };

    let pattern_static = match &c.pattern {
        Some(p) => {
            let name = format_ident!("{}", pattern_static_name(r, f)?);
            let doc = format!(" The pattern of field `{}` of record `{}`.", f.name, r.id);
            statics.push(quote! {
                #[doc = #doc]
                static #name: LazyLock<Regex> =
                    LazyLock::new(|| Regex::new(#p).expect("specc checked that the pattern compiles"));
            });
            Some(name)
        }
        None => None,
    };

    let min = c
        .min
        .as_ref()
        .and_then(|n| bound(n, base, true).ok().flatten());
    let max = c
        .max
        .as_ref()
        .and_then(|n| bound(n, base, false).ok().flatten());
    let max_length = c.max_length.as_ref().and_then(serde_json::Number::as_u64);
    if min.is_none() && max.is_none() && max_length.is_none() && pattern_static.is_none() {
        return None;
    }
    let min_violation = violation(quote! { Min });
    let max_violation = violation(quote! { Max });
    let length_violation = violation(quote! { MaxLength });
    let pattern_violation = violation(quote! { Pattern });
    let newtype_builtin = matches!(base, Builtin::Timestamp | Builtin::Duration);

    let leaf = move |access: Access| -> TokenStream {
        let number = match (&access, newtype_builtin) {
            (Access::Place(p), false) => quote! { #p },
            (Access::Place(p), true) => quote! { #p.0 },
            (Access::Ref, false) => quote! { *value },
            (Access::Ref, true) => quote! { value.0 },
        };
        let text = match &access {
            Access::Place(p) => quote! { #p },
            Access::Ref => quote! { value },
        };
        let text_ref = match &access {
            Access::Place(p) => quote! { &#p },
            Access::Ref => quote! { value },
        };
        let mut out = TokenStream::new();
        if let Some(b) = min {
            let lit = bound_tokens(b);
            out.extend(quote! { if #number < #lit { #min_violation } });
        }
        if let Some(b) = max {
            let lit = bound_tokens(b);
            out.extend(quote! { if #number > #lit { #max_violation } });
        }
        if let Some(n) = max_length {
            let lit = Literal::u64_unsuffixed(n);
            out.extend(quote! { if #text.chars().count() > #lit { #length_violation } });
        }
        if let Some(pattern) = &pattern_static {
            out.extend(quote! { if !#pattern.is_match(#text_ref) { #pattern_violation } });
        }
        out
    };

    let field = naming::ident(&naming::snake(&f.name).unwrap_or_default());
    Some(wrap(&steps, Access::Place(quote! { self.#field }), &leaf))
}

/// Applies the steps to reach the value, then the checks.
fn wrap(steps: &[Step], access: Access, leaf: &dyn Fn(Access) -> TokenStream) -> TokenStream {
    let Some((first, rest)) = steps.split_first() else {
        return leaf(access);
    };
    match first {
        Step::Option => {
            let source = match &access {
                Access::Place(p) => quote! { &#p },
                Access::Ref => quote! { value },
            };
            let inner = wrap(rest, Access::Ref, leaf);
            quote! {
                if let Some(value) = #source {
                    #inner
                }
            }
        }
        Step::Newtype => {
            let place = match &access {
                Access::Place(p) => quote! { #p.0 },
                Access::Ref => quote! { value.0 },
            };
            wrap(rest, Access::Place(place), leaf)
        }
    }
}

fn bound_tokens(b: Bound) -> TokenStream {
    match b {
        Bound::Int(v) => {
            let lit = Literal::u128_unsuffixed(v.unsigned_abs());
            if v < 0 {
                quote! { -#lit }
            } else {
                quote! { #lit }
            }
        }
        Bound::Float(f) => {
            let lit = Literal::f64_unsuffixed(f.abs());
            if f < 0.0 {
                quote! { -#lit }
            } else {
                quote! { #lit }
            }
        }
    }
}
