//! Pieces every backend uses: Rust types for type names, derives, doc comments, struct fields,
//! and the name checks that turn naming problems into refusals.

use std::collections::BTreeMap;

use proc_macro2::TokenStream;
use quote::quote;

use crate::diagnostic::Diagnostic;
use crate::ir::Spec;
use crate::naming;
use crate::resolve::{Builtin, TypeRef};

/// Code of a specification name that cannot become a Rust name.
pub const NAME_INVALID: &str = "specc.name-invalid";
/// Code of two specification names that become the same Rust name.
pub const NAME_COLLISION: &str = "specc.name-collision";
/// Code of a type name that would shadow a name the generated code uses.
pub const NAME_RESERVED: &str = "specc.name-reserved";

/// Which generated modules a file refers to, so it imports exactly those.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Uses {
    /// The file names `prelude::...`.
    pub prelude: bool,
    /// The file names `types::...`.
    pub types: bool,
}

impl Uses {
    /// The `use crate::...;` item for the modules used, or nothing.
    pub fn import(self) -> TokenStream {
        match (self.prelude, self.types) {
            (true, true) => quote! { use crate::{prelude, types}; },
            (true, false) => quote! { use crate::prelude; },
            (false, true) => quote! { use crate::types; },
            (false, false) => TokenStream::new(),
        }
    }
}

/// Where a type is written: inside `types.rs` declared types are named directly, elsewhere as
/// `types::Name`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Inside the `types` module.
    Types,
    /// In any other generated module.
    Other,
}

/// The Rust type of a type name. A name that does not resolve becomes `()`; the readiness
/// checks refuse every such name before emission.
pub fn rust_type(spec: &Spec, t: &TypeRef, scope: Scope, uses: &mut Uses) -> TokenStream {
    match t {
        TypeRef::Builtin(b) => builtin_type(*b, uses),
        TypeRef::Declared(id) => {
            let Some(decl) = spec.type_decl(id) else {
                return quote! { () };
            };
            let name = naming::ident(&naming::pascal(&decl.id).unwrap_or_default());
            match scope {
                Scope::Types => quote! { #name },
                Scope::Other => {
                    uses.types = true;
                    quote! { types::#name }
                }
            }
        }
        TypeRef::List(inner) => {
            let inner = rust_type(spec, inner, scope, uses);
            quote! { Vec<#inner> }
        }
        TypeRef::Set(inner) => {
            let inner = rust_type(spec, inner, scope, uses);
            quote! { std::collections::BTreeSet<#inner> }
        }
        TypeRef::Option(inner) => {
            let inner = rust_type(spec, inner, scope, uses);
            quote! { Option<#inner> }
        }
        TypeRef::Map(key, value) => {
            let key = rust_type(spec, key, scope, uses);
            let value = rust_type(spec, value, scope, uses);
            quote! { std::collections::BTreeMap<#key, #value> }
        }
        TypeRef::Shared(inner) => {
            let inner = rust_type(spec, inner, scope, uses);
            quote! { std::sync::Arc<#inner> }
        }
    }
}

/// As [`rust_type`], with every declared type in `boxed` that the value holds inline (directly or
/// through `option<T>`, not through a list or map, which already hold their items on the heap)
/// written `Box<T>`: what a recursive type needs to have a finite size.
pub fn rust_type_boxed(
    spec: &Spec,
    t: &TypeRef,
    scope: Scope,
    uses: &mut Uses,
    boxed: &dyn Fn(&str) -> bool,
) -> TokenStream {
    match t {
        TypeRef::Declared(id) if boxed(id) => {
            let inner = rust_type(spec, t, scope, uses);
            quote! { Box<#inner> }
        }
        TypeRef::Option(inner) => {
            let inner = rust_type_boxed(spec, inner, scope, uses, boxed);
            quote! { Option<#inner> }
        }
        _ => rust_type(spec, t, scope, uses),
    }
}

/// The Rust type of a built-in representation.
pub fn builtin_type(b: Builtin, uses: &mut Uses) -> TokenStream {
    match b {
        Builtin::Bool => quote! { bool },
        Builtin::U8 => quote! { u8 },
        Builtin::U16 => quote! { u16 },
        Builtin::U32 => quote! { u32 },
        Builtin::U64 => quote! { u64 },
        Builtin::I32 => quote! { i32 },
        Builtin::I64 => quote! { i64 },
        Builtin::F64 => quote! { f64 },
        Builtin::String => quote! { String },
        Builtin::Bytes => quote! { Vec<u8> },
        Builtin::Timestamp => {
            uses.prelude = true;
            quote! { prelude::Timestamp }
        }
        Builtin::Duration => {
            uses.prelude = true;
            quote! { prelude::DurationMs }
        }
        Builtin::Uuid => quote! { uuid::Uuid },
        Builtin::Json => quote! { serde_json::Value },
    }
}

/// The derives of every generated data type: `Eq`, `Ord` and `Hash` only when nothing in the
/// type is an `f64`.
pub fn derives(total: bool) -> TokenStream {
    derives_copy(total, false)
}

/// As [`derives`], with `Copy` as well when `copy` ([`crate::resolve::declared_is_copy`]).
pub fn derives_copy(total: bool, copy: bool) -> TokenStream {
    if copy && total {
        quote! { #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)] }
    } else if copy {
        quote! { #[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)] }
    } else if total {
        quote! { #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)] }
    } else {
        quote! { #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] }
    }
}

/// A doc comment built from paragraphs and bulleted lists.
///
/// Specification text is reduced to one line per paragraph or list item: whitespace runs
/// become one space, and code fences are broken, so that nothing in the text becomes a code
/// block (which rustdoc would compile as a test) or a lazily continued list item.
#[derive(Debug, Clone, Default)]
pub struct Doc {
    blocks: Vec<Vec<String>>,
}

impl Doc {
    /// A doc comment whose first paragraph is `text`.
    pub fn new(text: &str) -> Self {
        let mut doc = Self::default();
        doc.paragraph(text);
        doc
    }

    /// Adds a paragraph; empty text adds nothing.
    pub fn paragraph(&mut self, text: &str) -> &mut Self {
        let line = clean(text);
        if !line.is_empty() {
            self.blocks.push(vec![line]);
        }
        self
    }

    /// Adds `title` followed by a bulleted list; an empty list adds nothing.
    pub fn list(&mut self, title: &str, items: &[String]) -> &mut Self {
        let items: Vec<String> = items
            .iter()
            .map(|i| clean(i))
            .filter(|i| !i.is_empty())
            .collect();
        if !items.is_empty() {
            self.blocks.push(vec![clean(title)]);
            self.blocks
                .push(items.into_iter().map(|i| format!("- {i}")).collect());
        }
        self
    }

    /// The `#[doc = "..."]` attributes, one per line, with blank lines between blocks.
    pub fn tokens(&self) -> TokenStream {
        let mut lines: Vec<String> = Vec::new();
        for (i, block) in self.blocks.iter().enumerate() {
            if i > 0 {
                lines.push(String::new());
            }
            lines.extend(block.iter().map(|l| format!(" {l}")));
        }
        if lines.is_empty() {
            lines.push(" Generated from the specification.".to_string());
        }
        quote! { #( #[doc = #lines] )* }
    }
}

/// One line of doc text: whitespace collapsed, fences broken.
fn clean(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("```", "` ` `")
        .replace("~~~", "~ ~ ~")
}

/// A struct field to emit.
#[derive(Default)]
pub struct Field {
    /// The name on the wire, kept through `serde(rename)`: the specification's name, converted
    /// to the wire case where the field crosses a contract ([`crate::wire`]).
    pub wire_name: String,
    /// The Rust name (from [`naming::snake`]).
    pub rust_name: String,
    /// The field's Rust type, before the optional wrapper.
    pub ty: TokenStream,
    /// True when the field may be left out: `Option<T>`, defaulted and skipped when `None`.
    pub optional: bool,
    /// Its documentation.
    pub doc: Doc,
    /// The function that decodes it, when serde's own decoding is not enough (a `set<T>` that
    /// must refuse a repeated item): `#[serde(deserialize_with = "...")]`.
    pub deserialize_with: Option<String>,
}

impl Field {
    /// The field as it appears in a braced struct.
    pub fn tokens(&self) -> TokenStream {
        self.with_visibility(quote! { pub })
    }

    /// The field as it appears in a struct-like enum variant, which takes no visibility.
    pub fn variant_tokens(&self) -> TokenStream {
        self.with_visibility(TokenStream::new())
    }

    fn with_visibility(&self, vis: TokenStream) -> TokenStream {
        let doc = self.doc.tokens();
        let name = naming::ident(&self.rust_name);
        let ty = &self.ty;
        let spec_name = &self.wire_name;
        let renamed = self.rust_name != *spec_name;
        let serde = match (renamed, self.optional) {
            (false, false) => TokenStream::new(),
            (true, false) => quote! { #[serde(rename = #spec_name)] },
            (false, true) => {
                quote! { #[serde(default, skip_serializing_if = "Option::is_none")] }
            }
            (true, true) => {
                quote! { #[serde(rename = #spec_name, default, skip_serializing_if = "Option::is_none")] }
            }
        };
        let ty = if self.optional {
            quote! { Option<#ty> }
        } else {
            quote! { #ty }
        };
        let decode = match &self.deserialize_with {
            Some(path) => quote! { #[serde(deserialize_with = #path)] },
            None => TokenStream::new(),
        };
        quote! {
            #doc
            #serde
            #decode
            #vis #name: #ty,
        }
    }
}

/// Converts a name, recording `specc.name-invalid` when it cannot become a Rust name.
pub fn checked_name(
    diagnostics: &mut Vec<Diagnostic>,
    entity: &str,
    what: &str,
    converted: Result<String, String>,
) -> Option<String> {
    match converted {
        Ok(name) => Some(name),
        Err(reason) => {
            diagnostics.push(Diagnostic::new(
                NAME_INVALID,
                entity,
                format!("{what} cannot become a Rust name: {reason}."),
            ));
            None
        }
    }
}

/// The Rust names taken in one namespace (a module, a struct's fields, an enum's variants),
/// refusing a second claim on a name.
pub struct Namespace {
    scope: String,
    taken: BTreeMap<String, String>,
}

impl Namespace {
    /// An empty namespace, described as `scope` in diagnostics (for example `module types`).
    pub fn new(scope: impl Into<String>) -> Self {
        Self {
            scope: scope.into(),
            taken: BTreeMap::new(),
        }
    }

    /// Claims `rust` for `what`; records `specc.name-collision` when it is already taken.
    pub fn claim(
        &mut self,
        diagnostics: &mut Vec<Diagnostic>,
        entity: &str,
        what: &str,
        rust: &str,
    ) {
        if let Some(previous) = self.taken.get(rust) {
            diagnostics.push(Diagnostic::new(
                NAME_COLLISION,
                entity,
                format!(
                    "{what} and {previous} both become the Rust name `{rust}` in {}.",
                    self.scope
                ),
            ));
        } else {
            self.taken.insert(rust.to_string(), what.to_string());
        }
    }

    /// Claims a type name, also refusing names the generated code uses unqualified.
    pub fn claim_type(
        &mut self,
        diagnostics: &mut Vec<Diagnostic>,
        entity: &str,
        what: &str,
        rust: &str,
    ) {
        if naming::RESERVED_TYPE_NAMES.contains(&rust) {
            diagnostics.push(Diagnostic::new(
                NAME_RESERVED,
                entity,
                format!(
                    "{what} becomes the Rust name `{rust}`, which the generated code already uses."
                ),
            ));
        }
        self.claim(diagnostics, entity, what, rust);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_text_is_one_line_per_block_without_fences() {
        let mut doc = Doc::new("Title");
        doc.paragraph("  first\n    indented   line ```rust");
        doc.list("Items:", &["a\n b".to_string(), "  ".to_string()]);
        let text = doc.tokens().to_string();
        assert!(text.contains("\" first indented line ` ` `rust\""));
        assert!(text.contains("\" - a b\""));
        assert!(!text.contains("\" - \""));
    }

    #[test]
    fn an_empty_doc_still_documents() {
        assert!(Doc::default().tokens().to_string().contains("Generated"));
    }

    #[test]
    fn a_second_claim_is_a_collision() {
        let mut diagnostics = Vec::new();
        let mut ns = Namespace::new("module types");
        ns.claim(&mut diagnostics, "a-b", "type \"a-b\"", "AB");
        ns.claim(&mut diagnostics, "a_b", "type \"a_b\"", "AB");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, NAME_COLLISION);
        assert_eq!(diagnostics[0].entity, "a_b");
    }

    #[test]
    fn reserved_type_names_are_refused() {
        let mut diagnostics = Vec::new();
        let mut ns = Namespace::new("module types");
        ns.claim_type(&mut diagnostics, "option", "type \"option\"", "Option");
        assert_eq!(diagnostics[0].code, NAME_RESERVED);
    }

    #[test]
    fn invalid_names_are_reported() {
        let mut diagnostics = Vec::new();
        let name = checked_name(&mut diagnostics, "x", "value \"1\"", naming::pascal("1"));
        assert_eq!(name, None);
        assert_eq!(diagnostics[0].code, NAME_INVALID);
    }
}
