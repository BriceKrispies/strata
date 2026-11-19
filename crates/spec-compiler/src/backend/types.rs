//! Declared types (`types` section) to the `types` module.
//!
//! - `id` and `scalar`: a `#[serde(transparent)]` newtype over the representation.
//! - `enum`: one unit variant per value, serialised as the specification's exact string, and
//!   `as_str()`.
//! - `struct`: a struct with one field per declared field.

use std::collections::BTreeSet;

use proc_macro2::TokenStream;
use quote::quote;

use super::Backend;
use crate::diagnostic::Diagnostic;
use crate::ir::{Spec, TypeDecl, TypeKind};
use crate::naming;
use crate::render::{self, Doc, Field, Namespace, Scope, Uses};
use crate::resolve::{self, Builtin, TypeRef, BUILTIN_TYPES};

/// Code of a struct that contains itself other than through a list, so it has no finite size.
pub const TYPE_RECURSIVE: &str = "specc.type-recursive";

/// The declared-types backend.
pub struct TypesBackend;

impl Backend for TypesBackend {
    fn module(&self) -> &'static str {
        "types"
    }

    fn check(&self, spec: &Spec) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut module = Namespace::new("module types");
        let builtins = BUILTIN_TYPES.join(", ");
        for t in &spec.types {
            let what = format!("type \"{}\"", t.id);
            if let Some(name) = render::checked_name(&mut out, &t.id, &what, naming::pascal(&t.id))
            {
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
                    let mut names = Namespace::new(format!("struct \"{}\"", t.id));
                    for f in fields {
                        let what = format!("field \"{}\" of struct \"{}\"", f.name, t.id);
                        if let Some(name) =
                            render::checked_name(&mut out, &t.id, &what, naming::snake(&f.name))
                        {
                            names.claim(&mut out, &t.id, &what, &name);
                        }
                        if !resolve::resolves(spec, &f.type_name) {
                            out.push(Diagnostic::new(
                                "type.field-unresolved",
                                &t.id,
                                format!(
                                    "Field \"{}\" of struct \"{}\" has type \"{}\", which is neither built-in nor a declared type.",
                                    f.name, t.id, f.type_name
                                ),
                            ));
                        }
                    }
                    if contains_itself(spec, &t.id) {
                        out.push(Diagnostic::new(
                            TYPE_RECURSIVE,
                            &t.id,
                            format!(
                                "Struct \"{}\" contains itself other than through a list, so it has no finite size; wrap the recursive field in list<...>.",
                                t.id
                            ),
                        ));
                    }
                }
            }
        }
        out
    }

    fn emit(&self, spec: &Spec) -> TokenStream {
        let mut uses = Uses::default();
        let items: Vec<TokenStream> = spec
            .types
            .iter()
            .map(|t| type_item(spec, t, &mut uses))
            .collect();
        let serde = if items.is_empty() {
            TokenStream::new()
        } else {
            quote! { use serde::{Deserialize, Serialize}; }
        };
        let crate_uses = uses.import();
        quote! {
            #![doc = " The declared types of the specification: identifiers, scalars, enumerations and structs."]
            #serde
            #crate_uses
            #(#items)*
        }
    }
}

/// True when struct `start` reaches itself through fields and options, not through a list.
fn contains_itself(spec: &Spec, start: &str) -> bool {
    let mut seen = BTreeSet::new();
    let Some(decl) = spec.type_decl(start) else {
        return false;
    };
    struct_reaches(spec, decl, start, &mut seen)
}

fn struct_reaches(spec: &Spec, decl: &TypeDecl, target: &str, seen: &mut BTreeSet<String>) -> bool {
    let TypeKind::Struct { fields } = &decl.kind else {
        return false;
    };
    fields
        .iter()
        .any(|f| ref_reaches(spec, &TypeRef::parse(&f.type_name), target, seen))
}

fn ref_reaches(spec: &Spec, t: &TypeRef, target: &str, seen: &mut BTreeSet<String>) -> bool {
    match t {
        TypeRef::Builtin(_) | TypeRef::List(_) => false,
        TypeRef::Option(inner) => ref_reaches(spec, inner, target, seen),
        TypeRef::Declared(id) => {
            if id == target {
                return true;
            }
            if !seen.insert(id.clone()) {
                return false;
            }
            spec.type_decl(id)
                .is_some_and(|d| struct_reaches(spec, d, target, seen))
        }
    }
}

fn type_item(spec: &Spec, t: &TypeDecl, uses: &mut Uses) -> TokenStream {
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
            let doc = doc.tokens();
            quote! {
                #doc
                #derives
                #[serde(transparent)]
                pub struct #name(pub #ty);
            }
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
                .map(|f| {
                    let ty = rust_type_of(spec, &f.type_name, uses);
                    Field {
                        spec_name: f.name.clone(),
                        rust_name: naming::snake(&f.name).unwrap_or_default(),
                        ty,
                        optional: f.optional,
                        doc: Doc::new(&format!("Field `{}`.", f.name)),
                    }
                    .tokens()
                })
                .collect();
            quote! {
                #doc
                #derives
                pub struct #name {
                    #(#fields)*
                }
            }
        }
    }
}

fn rust_type_of(spec: &Spec, type_name: &str, uses: &mut Uses) -> TokenStream {
    render::rust_type(spec, &TypeRef::parse(type_name), Scope::Types, uses)
}
