//! Audits (`audits` section) to the `audits` module.
//!
//! For each audit on a contract the module has
//!
//! - `<Audit>Entry`, a struct of the captured fields, typed as the record's fields;
//! - `audit_<audit>(caller, request, response, now) -> Result<<Audit>Entry, Overflow>`, which
//!   fills them from the translated `from` expressions (`Err` only when an expression's arithmetic
//!   overflows);
//! - `<Audit>Entry::into_record([key])`, which completes the record: the key fields the audit does
//!   not capture come from `key`, the chain field starts empty and every other field is absent.
//!
//! A `chained-hash` audit also gets `chain_<audit>(entry, previous)`, which sets the entry's chain
//! field to the SHA-256 of the previous entry's chain field followed by the captured fields (each
//! as length-prefixed name and value; numbers and booleans as their decimal text), as bytes or
//! as lowercase hex according to the chain field's type; and `verify_<audit>(entries)`, which
//! returns the index of the first entry whose chain field is not what that computation gives.
//!
//! A capture takes a value of the field's own type by copying a bare reference to a value of
//! that type (`request.id`); otherwise the expression is a number, text or boolean, and the field
//! must be built from one: a built-in, or a declared id or scalar over one (not a `uuid` or an
//! enum). Audits of transitions and events are refused until the machine backend can name their
//! payload types.

use std::collections::BTreeSet;

use proc_macro2::TokenStream;
use quote::quote;
use strata_spec_expr::{Binding, Ty};

use super::view::{self, Class, Entry, ExprScope, Leaf};
use super::Backend;
use crate::constructs::{Audit, Immutability};
use crate::diagnostic::Diagnostic;
use crate::ir::{Record, RecordField, Spec};
use crate::naming;
use crate::render::{self, Namespace, Scope, Uses};
use crate::resolve::{self, Builtin, TypeRef};

/// Code of an audit that does not name exactly one target.
pub const TARGET: &str = "specc.audit-target";
/// Code of an audit of a transition or an event.
pub const TARGET_UNSUPPORTED: &str = "specc.audit-target-unsupported";
/// Code of an audit of a contract that is not declared.
pub const CONTRACT_UNKNOWN: &str = "specc.audit-contract-unknown";
/// Code of an audit whose record is not declared.
pub const RECORD_UNKNOWN: &str = "specc.audit-record-unknown";
/// Code of a capture list that is empty or captures a field twice.
pub const CAPTURE: &str = "specc.audit-capture";
/// Code of a captured field the record does not have.
pub const FIELD_UNKNOWN: &str = "specc.audit-field-unknown";
/// Code of a `from` expression that does not translate.
pub const EXPRESSION: &str = "specc.audit-expression";
/// Code of a captured field that cannot be made from an expression.
pub const CAPTURE_TYPE: &str = "specc.audit-capture-type";
/// Code of a missing, unknown or unusable chain field.
pub const CHAIN_FIELD: &str = "specc.audit-chain-field";
/// Code of a captured field of a chained audit that cannot be hashed.
pub const CHAIN_CAPTURE_TYPE: &str = "specc.audit-chain-capture-type";
/// Code of a required record field the audit does not fill.
pub const FIELD_UNFILLED: &str = "ready.audit-field-unfilled";

/// The audits backend.
pub struct AuditsBackend;

type Failure = (&'static str, String);

fn fail(code: &'static str, message: String) -> Failure {
    (code, message)
}

fn eff_of(f: &RecordField) -> TypeRef {
    let t = TypeRef::parse(&f.type_name);
    if f.optional {
        TypeRef::Option(Box::new(t))
    } else {
        t
    }
}

/// True when a leaf's plain value is `Copy`, so a reference to it is matched as `&value`.
fn plain_copy(l: &Leaf) -> bool {
    l.layers_empty() && !matches!(l.base, Builtin::String | Builtin::Bytes | Builtin::Uuid)
}

/// The scope of an audit of a contract.
fn scope_for(spec: &Spec, a: &Audit) -> ExprScope {
    let mut scope = ExprScope::new();
    scope.principal(&quote! { caller });
    if let Some(c) = a
        .contract
        .as_deref()
        .and_then(|id| spec.contracts.iter().find(|c| c.id == id))
    {
        scope.head(
            "request",
            view::contract_entries(spec, &c.inputs, &quote! { request }),
        );
        scope.head(
            "response",
            view::contract_entries(spec, &c.outputs, &quote! { response }),
        );
    }
    scope.single(
        "time",
        Entry {
            binding: Some(Binding {
                ty: Ty::I64,
                tokens: quote! { now.0 },
            }),
            why: None,
            place: TokenStream::new(),
            eff: TypeRef::Builtin(Builtin::I64),
        },
    );
    scope
}

/// The value of one captured field, and the names its expression reads.
fn compile_capture(
    spec: &Spec,
    scope: &ExprScope,
    field: &RecordField,
    from: &str,
    uses: &mut Uses,
) -> Result<(TokenStream, BTreeSet<String>), Failure> {
    let eff = eff_of(field);
    if let Some(path) = view::bare_reference(from) {
        if let Some(entry) = scope.lookup(&path) {
            if entry.eff == eff && !entry.place.is_empty() {
                let place = &entry.place;
                let heads = BTreeSet::from([path[0].clone()]);
                return Ok(if resolve::is_copy(&eff) {
                    (quote! { #place }, heads)
                } else {
                    (quote! { #place.clone() }, heads)
                });
            }
        }
    }
    if view::is_null(from) {
        return if field.optional || matches!(eff, TypeRef::Option(_)) {
            Ok((quote! { None }, BTreeSet::new()))
        } else {
            Err(fail(
                CAPTURE_TYPE,
                format!(
                    "\"{from}\" is null, but field \"{}\" is not optional.",
                    field.name
                ),
            ))
        };
    }
    let base = TypeRef::parse(&field.type_name);
    let leaf = view::leaf(spec, &base).filter(Leaf::buildable).ok_or_else(|| {
        fail(
            CAPTURE_TYPE,
            format!(
                "Field \"{}\" has type {}, which cannot be made from an expression; capture it by naming a value of that same type.",
                field.name,
                view::type_text(&eff)
            ),
        )
    })?;
    let translated = view::typed(scope, from, &leaf.ty).map_err(|why| {
        fail(
            EXPRESSION,
            format!(
                "The capture of field \"{}\" cannot be generated: {why}.",
                field.name
            ),
        )
    })?;
    let value = leaf.build(translated.tokens, uses).ok_or_else(|| {
        fail(
            CAPTURE_TYPE,
            format!("Field \"{}\" cannot be built.", field.name),
        )
    })?;
    let value = if field.optional {
        quote! { Some(#value) }
    } else {
        value
    };
    Ok((value, translated.heads))
}

/// What the module has for one audit.
struct Compiled {
    items: TokenStream,
    chained: bool,
    hex: bool,
}

fn target_record<'a>(spec: &'a Spec, a: &Audit) -> Result<&'a Record, Failure> {
    a.record
        .as_deref()
        .and_then(|r| spec.record(r))
        .ok_or_else(|| {
            fail(
                RECORD_UNKNOWN,
                format!(
                    "Audit \"{}\" writes \"{}\", which is not a record.",
                    a.id,
                    a.record.clone().unwrap_or_default()
                ),
            )
        })
}

fn compile(spec: &Spec, a: &Audit, uses: &mut Uses) -> Result<Compiled, Vec<Failure>> {
    let mut errors: Vec<Failure> = Vec::new();
    match (&a.contract, &a.transition, &a.event) {
        (Some(c), None, None) => {
            if !spec.contracts.iter().any(|x| &x.id == c) {
                errors.push(fail(
                    CONTRACT_UNKNOWN,
                    format!("Audit \"{}\" is about \"{c}\", which is not a contract.", a.id),
                ));
            }
        }
        (None, Some(_), None) | (None, None, Some(_)) => errors.push(fail(
            TARGET_UNSUPPORTED,
            format!("Audit \"{}\" is about a transition or an event; only audits of contracts are generated yet.", a.id),
        )),
        _ => errors.push(fail(
            TARGET,
            format!("Audit \"{}\" must name exactly one of \"contract\", \"transition\" and \"event\".", a.id),
        )),
    }
    let record = match target_record(spec, a) {
        Ok(r) => r,
        Err(e) => {
            errors.push(e);
            return Err(errors);
        }
    };
    if a.capture.is_empty() {
        errors.push(fail(
            CAPTURE,
            format!("Audit \"{}\" captures nothing.", a.id),
        ));
    }

    let scope = scope_for(spec, a);
    let mut captured: Vec<(&RecordField, TokenStream)> = Vec::new();
    let mut heads: BTreeSet<String> = BTreeSet::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for c in &a.capture {
        if !seen.insert(&c.field) {
            errors.push(fail(
                CAPTURE,
                format!("Audit \"{}\" captures field \"{}\" twice.", a.id, c.field),
            ));
            continue;
        }
        let Some(field) = record.field(&c.field) else {
            errors.push(fail(
                FIELD_UNKNOWN,
                format!("Record \"{}\" has no field \"{}\".", record.id, c.field),
            ));
            continue;
        };
        if !resolve::resolves(spec, &field.type_name) {
            continue;
        }
        match compile_capture(spec, &scope, field, &c.from, uses) {
            Ok((value, used)) => {
                heads.extend(used);
                captured.push((field, value));
            }
            Err(e) => errors.push(e),
        }
    }

    // The chain.
    let chained = a.immutability == Immutability::ChainedHash;
    let mut chain: Option<(&RecordField, Leaf)> = None;
    match (&a.chain_field, chained) {
        (None, true) => errors.push(fail(
            CHAIN_FIELD,
            format!("Audit \"{}\" is chained by hash, so it names the field that holds the previous hash in \"chainField\".", a.id),
        )),
        (Some(_), false) => errors.push(fail(
            CHAIN_FIELD,
            format!("Audit \"{}\" is not chained by hash, so it takes no \"chainField\".", a.id),
        )),
        (Some(name), true) => match record.field(name) {
            None => errors.push(fail(CHAIN_FIELD, format!("Record \"{}\" has no field \"{name}\".", record.id))),
            Some(f) => {
                let leaf = view::leaf(spec, &TypeRef::parse(&f.type_name))
                    .filter(|l| l.buildable() && matches!(l.base, Builtin::String | Builtin::Bytes));
                match leaf {
                    Some(l) if !f.optional => {
                        if a.capture.iter().any(|c| &c.field == name) {
                            errors.push(fail(CHAIN_FIELD, format!("Chain field \"{name}\" is filled by the chain, so it is not captured.")));
                        } else {
                            chain = Some((f, l));
                        }
                    }
                    _ => errors.push(fail(
                        CHAIN_FIELD,
                        format!("Chain field \"{name}\" of record \"{}\" must be a required string or bytes field; it is {}.", record.id, f.type_name),
                    )),
                }
            }
        },
        (None, false) => {}
    }

    // Readiness: every required field is filled.
    let key: &[String] = record.key.as_deref().unwrap_or_default();
    for f in &record.fields {
        let filled = a.capture.iter().any(|c| c.field == f.name)
            || key.contains(&f.name)
            || a.chain_field.as_deref() == Some(f.name.as_str());
        if !f.optional && !filled {
            errors.push(fail(
                FIELD_UNFILLED,
                format!("Audit \"{}\" writes record \"{}\" but captures nothing for its required field \"{}\".", a.id, record.id, f.name),
            ));
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    // Emission. Everything below assumes the checks above passed.
    let snake = naming::snake(&a.id).unwrap_or_default();
    let entry_name = naming::ident(&format!(
        "{}Entry",
        naming::pascal(&a.id).unwrap_or_default()
    ));
    let record_ty = view::record_path(&record.id);
    let contract = a
        .contract
        .as_deref()
        .and_then(|id| spec.contracts.iter().find(|c| c.id == id));

    let entry_fields: Vec<TokenStream> = captured
        .iter()
        .map(|(f, _)| {
            let ident = view::snake_ident(&f.name);
            let ty = render::rust_type(spec, &TypeRef::parse(&f.type_name), Scope::Other, uses);
            let ty = if f.optional {
                quote! { Option<#ty> }
            } else {
                ty
            };
            let doc = format!(" The value of field `{}`.", f.name);
            quote! {
                #[doc = #doc]
                pub #ident: #ty,
            }
        })
        .collect();
    let inits: Vec<TokenStream> = captured
        .iter()
        .map(|(f, value)| {
            let ident = view::snake_ident(&f.name);
            quote! { #ident: #value }
        })
        .collect();

    uses.prelude = true;
    let param = |name: &str, used: bool| {
        let n = if used {
            name.to_string()
        } else {
            format!("_{name}")
        };
        naming::ident(&n)
    };
    let (caller, request, response, now) = (
        param("caller", heads.contains("principal")),
        param("request", heads.contains("request")),
        param("response", heads.contains("response")),
        param("now", heads.contains("time")),
    );
    let request_ty = contract
        .map(|c| view::request_path(&c.id))
        .unwrap_or_default();
    let response_ty = contract
        .map(|c| view::response_path(&c.id))
        .unwrap_or_default();
    let audit_fn = naming::ident(&format!("audit_{snake}"));
    let entry_doc = format!(
        " The captured fields of one entry of audit `{}`, to be completed into a record of `{}`.",
        a.id, record.id
    );
    let fn_doc = format!(
        " Captures the fields of audit `{}` from a call of contract `{}`.",
        a.id,
        a.contract.clone().unwrap_or_default()
    );

    // into_record
    let key_struct = naming::ident(&format!(
        "{}Key",
        naming::pascal(&record.id).unwrap_or_default()
    ));
    let uncaptured_key = key.iter().any(|k| !a.capture.iter().any(|c| &c.field == k));
    let mut record_inits = Vec::new();
    for f in &record.fields {
        let ident = view::snake_ident(&f.name);
        let is_captured = a.capture.iter().any(|c| c.field == f.name);
        let value = if is_captured {
            quote! { self.#ident }
        } else if key.contains(&f.name) {
            quote! { key.#ident }
        } else if let Some((cf, l)) = &chain {
            if cf.name == f.name {
                let empty = match l.base {
                    Builtin::Bytes => quote! { Vec::new() },
                    _ => quote! { String::new() },
                };
                l.build(empty, uses).unwrap_or_default()
            } else {
                quote! { None }
            }
        } else {
            quote! { None }
        };
        record_inits.push(quote! { #ident: #value });
    }
    let (key_param, key_doc) = if uncaptured_key {
        let owner = quote! { crate::records::#key_struct };
        (
            quote! { , key: #owner },
            " The key fields the audit does not capture come from `key`; the chain field starts empty.",
        )
    } else {
        (
            TokenStream::new(),
            " The chain field starts empty; set it with the audit's `chain` function.",
        )
    };
    let into_record = quote! {
        impl #entry_name {
            /// Completes the entry into a record.
            #[doc = #key_doc]
            pub fn into_record(self #key_param) -> #record_ty {
                #record_ty {
                    #(#record_inits,)*
                }
            }
        }
    };

    let mut chain_items = TokenStream::new();
    let mut hex = false;
    if let Some((chain_field, leaf)) = &chain {
        let chain_ident = view::snake_ident(&chain_field.name);
        let hash_fn = naming::ident(&format!("hash_{snake}"));
        let chain_fn = naming::ident(&format!("chain_{snake}"));
        let verify_fn = naming::ident(&format!("verify_{snake}"));
        let previous_bytes = {
            let read = leaf.read(quote! { p.#chain_ident });
            match leaf.base {
                Builtin::Bytes => quote! { #read.clone() },
                _ => quote! { #read.as_bytes().to_vec() },
            }
        };
        let mut encodings = Vec::new();
        for (f, _) in &captured {
            let name = &f.name;
            let ident = view::snake_ident(&f.name);
            let Some(l) = view::leaf(spec, view::peel_options(&TypeRef::parse(&f.type_name)).0)
            else {
                continue;
            };
            let encode = |place: TokenStream| -> TokenStream {
                let read = l.read(place.clone());
                match l.class {
                    Class::Text => {
                        let text = l.as_str(place);
                        quote! { feed(&mut hasher, #name, #text.as_bytes()); }
                    }
                    Class::Bytes => quote! { feed(&mut hasher, #name, &#read); },
                    _ => quote! { feed(&mut hasher, #name, #read.to_string().as_bytes()); },
                }
            };
            let place = quote! { entry.#ident };
            encodings.push(if f.optional {
                let inner = encode(quote! { value });
                let pattern = if plain_copy(&l) {
                    quote! { Some(&value) }
                } else {
                    quote! { Some(value) }
                };
                quote! {
                    match &#place {
                        #pattern => {
                            feed(&mut hasher, #name, &[1]);
                            #inner
                        }
                        None => feed(&mut hasher, #name, &[0]),
                    }
                }
            } else {
                encode(place)
            });
        }
        let digest_value = match leaf.base {
            Builtin::Bytes => quote! { digest.to_vec() },
            _ => {
                hex = true;
                quote! { to_hex(&digest) }
            }
        };
        let value = leaf.build(digest_value, uses).unwrap_or_default();
        let hash_doc = format!(" The hash of an entry of audit `{}`: the previous entry's chain field and the captured fields.", a.id);
        let chain_doc = format!(" Sets the chain field of an entry of audit `{}` from the entry before it, if there is one.", a.id);
        let verify_doc = format!(" The index of the first entry of audit `{}` whose chain field is not what the chain gives, if any.", a.id);
        chain_items = quote! {
            #[doc = #hash_doc]
            fn #hash_fn(entry: &#record_ty, previous: Option<&#record_ty>) -> [u8; 32] {
                let mut hasher = Sha256::new();
                let previous_hash: Vec<u8> = match previous {
                    Some(p) => #previous_bytes,
                    None => Vec::new(),
                };
                feed(&mut hasher, "previous", &previous_hash);
                #(#encodings)*
                hasher.finalize().into()
            }

            #[doc = #chain_doc]
            pub fn #chain_fn(entry: &mut #record_ty, previous: Option<&#record_ty>) {
                let digest = #hash_fn(entry, previous);
                entry.#chain_ident = #value;
            }

            #[doc = #verify_doc]
            pub fn #verify_fn(entries: &[#record_ty]) -> Result<(), usize> {
                for (i, entry) in entries.iter().enumerate() {
                    let previous = i.checked_sub(1).and_then(|j| entries.get(j));
                    let digest = #hash_fn(entry, previous);
                    if entry.#chain_ident != #value {
                        return Err(i);
                    }
                }
                Ok(())
            }
        };
    }

    let items = quote! {
        #[doc = #entry_doc]
        #[derive(Debug, Clone, PartialEq)]
        pub struct #entry_name {
            #(#entry_fields)*
        }

        #[doc = #fn_doc]
        pub fn #audit_fn(
            #caller: &Caller,
            #request: &#request_ty,
            #response: &#response_ty,
            #now: prelude::Timestamp,
        ) -> Result<#entry_name, crate::support::Overflow> {
            Ok(#entry_name {
                #(#inits,)*
            })
        }

        #into_record

        #chain_items
    };
    Ok(Compiled {
        items,
        chained: chain.is_some(),
        hex,
    })
}

impl Backend for AuditsBackend {
    fn module(&self) -> &'static str {
        "audits"
    }

    fn check(&self, spec: &Spec) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut names = Namespace::new("module audits");
        for n in ["feed", "to_hex"] {
            names.claim(&mut out, "", "a helper of the module", n);
        }
        for a in &spec.constructs.audits {
            let what = format!("audit \"{}\"", a.id);
            match (naming::pascal(&a.id), naming::snake(&a.id)) {
                (Ok(pascal), Ok(snake)) => {
                    names.claim_type(
                        &mut out,
                        &a.id,
                        &format!("the entry of {what}"),
                        &format!("{pascal}Entry"),
                    );
                    for prefix in ["audit", "chain", "verify", "hash"] {
                        names.claim(
                            &mut out,
                            &a.id,
                            &format!("the {prefix} function of {what}"),
                            &format!("{prefix}_{snake}"),
                        );
                    }
                }
                (Err(reason), _) | (_, Err(reason)) => out.push(Diagnostic::new(
                    render::NAME_INVALID,
                    &a.id,
                    format!("{what} cannot become a Rust name: {reason}."),
                )),
            }
            if let Err(errors) = compile(spec, a, &mut Uses::default()) {
                for (code, message) in errors {
                    out.push(Diagnostic::new(code, &a.id, message));
                }
            }
        }
        out
    }

    fn emit(&self, spec: &Spec) -> TokenStream {
        let mut uses = Uses::default();
        let compiled: Vec<Compiled> = spec
            .constructs
            .audits
            .iter()
            .filter_map(|a| compile(spec, a, &mut uses).ok())
            .collect();
        let any = !compiled.is_empty();
        let chained = compiled.iter().any(|c| c.chained);
        let hex = compiled.iter().any(|c| c.hex);
        let items: Vec<&TokenStream> = compiled.iter().map(|c| &c.items).collect();
        let caller = if any {
            quote! { use crate::support::Caller; }
        } else {
            TokenStream::new()
        };
        let sha = if chained {
            quote! { use sha2::{Digest, Sha256}; }
        } else {
            TokenStream::new()
        };
        let feed = if chained {
            quote! {
                /// Adds a named value to a hash: the name and the value, each with its length.
                fn feed(hasher: &mut Sha256, name: &str, bytes: &[u8]) {
                    hasher.update(u64::try_from(name.len()).unwrap_or(u64::MAX).to_le_bytes());
                    hasher.update(name.as_bytes());
                    hasher.update(u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_le_bytes());
                    hasher.update(bytes);
                }
            }
        } else {
            TokenStream::new()
        };
        let to_hex = if hex {
            quote! {
                /// The bytes as lowercase hexadecimal text.
                fn to_hex(bytes: &[u8]) -> String {
                    const DIGITS: &[u8; 16] = b"0123456789abcdef";
                    let mut text = String::with_capacity(bytes.len() * 2);
                    for byte in bytes {
                        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
                        text.push(char::from(DIGITS[usize::from(byte & 15)]));
                    }
                    text
                }
            }
        } else {
            TokenStream::new()
        };
        let crate_uses = uses.import();
        quote! {
            #![doc = " What the specification's audited acts write to their append-only logs: the entry of each audit and, for hash-chained logs, the chain."]
            #sha
            #caller
            #crate_uses
            #feed
            #to_hex
            #(#items)*
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load::load;

    fn spec(extra: &str) -> Spec {
        load(
            "s",
            &format!(
                r#"{{ "system": {{ "id": "s", "name": "S" }},
                "records": [ {{ "id": "log", "name": "Log", "owner": "svc", "key": ["seq"], "schemaVersion": 1,
                    "fields": [ {{ "name": "seq", "type": "u64" }}, {{ "name": "who", "type": "string" }},
                                {{ "name": "at", "type": "timestamp" }}, {{ "name": "amount", "type": "u32" }},
                                {{ "name": "hash", "type": "bytes" }}, {{ "name": "note", "type": "string", "optional": true }} ] }} ],
                "contracts": [ {{ "id": "pay", "name": "Pay", "realisation": "generated",
                    "inputs": [ {{ "name": "amount", "type": "u32" }}, {{ "name": "memo", "type": "string", "optional": true }} ],
                    "outputs": [ {{ "name": "ok", "type": "bool" }} ] }} ],
                {extra} }}"#
            ),
        )
        .unwrap()
    }

    fn codes(s: &Spec) -> Vec<String> {
        AuditsBackend.check(s).into_iter().map(|d| d.code).collect()
    }

    const CAPTURES: &str = r#"[ { "field": "who", "from": "principal.id" }, { "field": "at", "from": "time" },
        { "field": "amount", "from": "request.amount" }, { "field": "note", "from": "request.memo" } ]"#;

    #[test]
    fn a_required_field_nobody_fills_is_refused() {
        let s = spec(&format!(
            r#""audits": [ {{ "id": "pay-log", "contract": "pay", "record": "log", "capture": {CAPTURES}, "immutability": "append-only" }} ]"#
        ));
        // `hash` is a required field, so an audit that does not chain leaves it unfilled.
        assert_eq!(codes(&s), [FIELD_UNFILLED]);
    }

    #[test]
    fn a_chained_audit_gets_the_chain() {
        let s = spec(&format!(
            r#""audits": [ {{ "id": "pay-log", "contract": "pay", "record": "log", "capture": {CAPTURES}, "immutability": "chained-hash", "chainField": "hash" }} ]"#
        ));
        assert_eq!(codes(&s), Vec::<String>::new());
        let text = AuditsBackend.emit(&s).to_string();
        assert!(text.contains("pub struct PayLogEntry"));
        assert!(text.contains("pub fn audit_pay_log"));
        assert!(text.contains("pub fn chain_pay_log"));
        assert!(text.contains("pub fn verify_pay_log"));
        assert!(text.contains("pub fn into_record (self , key : crate :: records :: LogKey)"));
    }

    #[test]
    fn audits_that_cannot_be_generated_are_refused() {
        let s = spec(
            r#""audits": [
              { "id": "a", "transition": "go", "record": "log", "capture": [ { "field": "who", "from": "principal.id" } ], "immutability": "append-only" },
              { "id": "b", "contract": "pay", "record": "log", "capture": [ { "field": "who", "from": "request.amount" }, { "field": "who", "from": "principal.id" }, { "field": "nope", "from": "time" } ], "immutability": "chained-hash" },
              { "id": "c", "contract": "pay", "record": "ghost", "capture": [], "immutability": "append-only" } ]"#,
        );
        let got = codes(&s);
        for code in [
            TARGET_UNSUPPORTED,
            EXPRESSION,
            CAPTURE,
            FIELD_UNKNOWN,
            CHAIN_FIELD,
            RECORD_UNKNOWN,
        ] {
            assert!(got.iter().any(|c| c == code), "{code} in {got:?}");
        }
    }
}
