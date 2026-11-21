//! Which generated types are read from outside the process, and so derive `Deserialize`
//! (WG-5371).
//!
//! A model split by area ([`crate::emit::split_crate_files`]) derives `Deserialize` only on the
//! types something reads from the wire, from storage or from another machine, and on every type
//! such a type holds. What is read so:
//!
//! - the request, response and error types of every contract a declared component serves: the
//!   routes read the requests, and the typed clients read the responses and errors;
//! - every record (read back from storage), every event's payload (delivered between machines and
//!   read back from outboxes) and every machine's variables (a stored machine is read back);
//! - the error body of `system.errorWire`, which the clients read;
//! - the contract an audit is written for, whose request and response the audit captures.
//!
//! The other types (the contract types of a contract no component serves, and the declared types
//! that only those, functions, languages or other such types hold) derive no `Deserialize`, and
//! their fields lose the `deserialize_with` decoders that only reading needs. `Deserialize` is
//! the larger half of what the compiler expands for a type.
//!
//! Every type still derives `Serialize`. Leaving it out as well was tried: hand-written code
//! writes declared types the specification gives no boundary for (a kernel `Grant` in an audit
//! entry, a session summary in a JSON answer), and the generator cannot see those uses, so the
//! write side stays whole. Hand-written code that read a type left out would not compile, so the
//! workspace build checks that nothing outside the generator reads more.

use std::collections::BTreeSet;

use proc_macro2::{Delimiter, Group, TokenStream, TokenTree};

use crate::ir::{Contract, Spec, TypeKind};
use crate::resolve::TypeRef;

/// The declared types (by id) and the contracts (by id) whose generated types derive
/// `Deserialize`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Boundary {
    /// Declared type ids.
    pub types: BTreeSet<String>,
    /// Contract ids.
    pub contracts: BTreeSet<String>,
}

impl Boundary {
    /// What crosses a boundary in `spec`: the roots listed in the module documentation, and every
    /// declared type they reach through fields, variants, lists, sets, maps, options and shares.
    pub fn of(spec: &Spec) -> Self {
        let mut contracts: BTreeSet<String> = spec
            .contracts
            .iter()
            .filter(|c| c.served_by().is_some_and(|id| spec.component(id).is_some()))
            .map(|c| c.id.clone())
            .collect();
        for a in &spec.constructs.audits {
            if let Some(c) = &a.contract {
                contracts.insert(c.clone());
            }
        }
        let mut texts: Vec<&str> = Vec::new();
        for c in spec.contracts.iter().filter(|c| contracts.contains(&c.id)) {
            texts.extend(contract_texts(c));
        }
        for r in &spec.records {
            texts.extend(r.fields.iter().map(|f| f.type_name.as_str()));
        }
        for e in &spec.events {
            texts.extend(e.payload.iter().filter_map(|f| f.type_name.as_deref()));
        }
        for b in spec
            .machines
            .iter()
            .chain(&spec.memory_machines)
            .chain(&spec.target_machines)
        {
            texts.extend(b.variables.iter().filter_map(|v| v.repr.as_deref()));
        }
        if let Some(body) = &spec.system.error_wire.body {
            texts.push(&body.type_name);
        }
        let mut types = BTreeSet::new();
        let mut stack: Vec<TypeRef> = texts.into_iter().map(TypeRef::parse).collect();
        while let Some(t) = stack.pop() {
            match t {
                TypeRef::Builtin(_) => {}
                TypeRef::List(inner)
                | TypeRef::Set(inner)
                | TypeRef::Option(inner)
                | TypeRef::Shared(inner) => stack.push(*inner),
                TypeRef::Map(k, v) => {
                    stack.push(*k);
                    stack.push(*v);
                }
                TypeRef::Declared(id) => {
                    let Some(decl) = spec.type_decl(&id) else {
                        continue;
                    };
                    if !types.insert(id) {
                        continue;
                    }
                    match &decl.kind {
                        TypeKind::Struct { fields } => {
                            stack.extend(fields.iter().map(|f| TypeRef::parse(&f.type_name)));
                        }
                        TypeKind::Union { variants, .. } => stack.extend(
                            variants
                                .iter()
                                .flat_map(|v| &v.fields)
                                .map(|f| TypeRef::parse(&f.type_name)),
                        ),
                        TypeKind::Id { .. } | TypeKind::Scalar { .. } | TypeKind::Enum { .. } => {}
                    }
                }
            }
        }
        Self { types, contracts }
    }

    /// True when declared type `id` derives `Deserialize`.
    pub fn has_type(&self, id: &str) -> bool {
        self.types.contains(id)
    }

    /// True when the types of contract `c` derive `Deserialize`.
    pub fn has_contract(&self, c: &Contract) -> bool {
        self.contracts.contains(&c.id)
    }
}

/// The type texts of a contract's inputs, outputs and errors.
fn contract_texts(c: &Contract) -> impl Iterator<Item = &str> {
    c.inputs
        .iter()
        .chain(&c.outputs)
        .filter_map(|f| f.type_name.as_deref())
        .chain(c.errors.iter().filter_map(|e| e.type_name.as_deref()))
}

/// `tokens` without reading: `Deserialize` taken out of every `#[derive(...)]`, and every
/// `deserialize_with` taken out of every `#[serde(...)]` (an attribute left empty is dropped), at
/// any depth. The attributes `Serialize` reads (renames, tags, `transparent`, `into`) stay.
pub fn strip_deserialize(tokens: TokenStream) -> TokenStream {
    let trees: Vec<TokenTree> = tokens.into_iter().collect();
    let mut out = Vec::with_capacity(trees.len());
    let mut i = 0;
    while i < trees.len() {
        if let (TokenTree::Punct(hash), Some(TokenTree::Group(attr))) =
            (&trees[i], trees.get(i + 1))
        {
            let inner: Vec<TokenTree> = attr.stream().into_iter().collect();
            let list = match (inner.first(), inner.get(1)) {
                (Some(TokenTree::Ident(name)), Some(TokenTree::Group(list)))
                    if hash.as_char() == '#'
                        && attr.delimiter() == Delimiter::Bracket
                        && (name == "derive" || name == "serde") =>
                {
                    Some((name.to_string(), list))
                }
                _ => None,
            };
            if let Some((name, list)) = list {
                let derive = name == "derive";
                let kept = without_entry(list.stream(), |entry| {
                    if derive {
                        matches!(entry.last(), Some(TokenTree::Ident(i)) if i == "Deserialize")
                    } else {
                        matches!(entry.first(), Some(TokenTree::Ident(i)) if i == "deserialize_with")
                    }
                });
                if derive || !kept.is_empty() {
                    let mut group = Group::new(list.delimiter(), kept);
                    group.set_span(list.span());
                    let mut body: Vec<TokenTree> = vec![inner[0].clone(), TokenTree::Group(group)];
                    body.extend(inner[2..].iter().cloned());
                    let mut attr_group = Group::new(Delimiter::Bracket, body.into_iter().collect());
                    attr_group.set_span(attr.span());
                    out.push(trees[i].clone());
                    out.push(TokenTree::Group(attr_group));
                }
                i += 2;
                continue;
            }
        }
        match &trees[i] {
            TokenTree::Group(g) => {
                let mut group = Group::new(g.delimiter(), strip_deserialize(g.stream()));
                group.set_span(g.span());
                out.push(TokenTree::Group(group));
            }
            other => out.push(other.clone()),
        }
        i += 1;
    }
    out.into_iter().collect()
}

/// A comma-separated list without the entries `drop` matches.
fn without_entry(list: TokenStream, drop: impl Fn(&[TokenTree]) -> bool) -> TokenStream {
    let mut entries: Vec<Vec<TokenTree>> = vec![Vec::new()];
    for tree in list {
        match &tree {
            TokenTree::Punct(p) if p.as_char() == ',' => entries.push(Vec::new()),
            _ => entries.last_mut().expect("an entry").push(tree),
        }
    }
    let mut out: Vec<TokenTree> = Vec::new();
    for entry in entries.into_iter().filter(|e| !e.is_empty() && !drop(e)) {
        if !out.is_empty() {
            out.push(TokenTree::Punct(proc_macro2::Punct::new(
                ',',
                proc_macro2::Spacing::Alone,
            )));
        }
        out.extend(entry);
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn deserialize_and_its_decoders_are_stripped_at_any_depth() {
        let item = quote! {
            #[doc = " A thing."]
            #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
            #[serde(tag = "kind")]
            pub enum Thing {
                #[serde(rename = "one")]
                One {
                    #[serde(deserialize_with = "decode_set")]
                    a: Vec<u8>,
                    #[serde(rename = "bee", deserialize_with = "decode_set")]
                    b: Vec<u8>,
                },
            }
        };
        let out = strip_deserialize(item).to_string();
        assert!(!out.contains("Deserialize"), "{out}");
        assert!(!out.contains("deserialize_with"), "{out}");
        assert!(
            out.contains("derive (Debug , Clone , PartialEq , Serialize)"),
            "{out}"
        );
        assert!(out.contains("serde (tag = \"kind\")"), "{out}");
        assert!(out.contains("serde (rename = \"bee\")"), "{out}");
        assert!(out.contains("A thing."), "{out}");
    }

    #[test]
    fn a_type_reached_from_a_record_crosses_and_one_reached_from_nothing_does_not() {
        let spec = crate::load::load(
            "s.jsonc",
            r#"{ "system": { "id": "s", "name": "S", "generation": { "target": "rust" } },
                 "types": [
                   { "id": "inner", "name": "Inner", "kind": "struct", "fields": [{ "name": "n", "type": "u32" }] },
                   { "id": "outer", "name": "Outer", "kind": "struct", "fields": [{ "name": "i", "type": "list<inner>" }] },
                   { "id": "loose", "name": "Loose", "kind": "struct", "fields": [{ "name": "n", "type": "u32" }] }
                 ],
                 "records": [{ "id": "thing", "name": "Thing", "key": ["id"],
                   "fields": [{ "name": "id", "type": "string" }, { "name": "o", "type": "option<outer>" }] }] }"#,
        )
        .expect("loads");
        let b = Boundary::of(&spec);
        assert!(b.has_type("outer"));
        assert!(b.has_type("inner"));
        assert!(!b.has_type("loose"));
    }
}
