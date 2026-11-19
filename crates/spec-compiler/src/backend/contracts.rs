//! Contracts (`contracts` section) to the `contracts` module.
//!
//! Each contract becomes `<Name>Request` (its inputs), `<Name>Response` (its outputs) and
//! `<Name>Error` (one variant per error condition, carrying its payload type when it declares
//! one). Each component named in `implementedBy` becomes a trait with one async method per
//! contract it implements. The `meta` module describes every contract as a
//! `prelude::ContractMeta` (reference, realisation, auth scheme, idempotency, timeout).

use proc_macro2::{Literal, TokenStream};
use quote::quote;

use super::Backend;
use crate::diagnostic::Diagnostic;
use crate::ir::{Contract, ContractField, Exposure, Realisation, Spec};
use crate::naming;
use crate::render::{self, Doc, Field, Namespace, Scope, Uses};
use crate::resolve::{self, TypeRef};
use crate::wire::{self, WireCase};

/// Code of an `implementedBy` that names no component.
pub const COMPONENT_UNKNOWN: &str = "specc.component-unknown";
/// Code of a `timeoutMs` that is not a positive integer.
pub const TIMEOUT_INVALID: &str = "specc.timeout-invalid";

/// The contracts backend.
pub struct ContractsBackend;

/// True when `reference` is a hole reference: `H-` and one to four digits.
pub fn is_hole_ref(reference: &str) -> bool {
    reference.strip_prefix("H-").is_some_and(|digits| {
        (1..=4).contains(&digits.len()) && digits.chars().all(|c| c.is_ascii_digit())
    })
}

impl Backend for ContractsBackend {
    fn module(&self) -> &'static str {
        "contracts"
    }

    fn check(&self, spec: &Spec) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut module = Namespace::new("module contracts");
        let mut meta = Namespace::new("module contracts::meta");
        meta.claim(&mut out, "", "the list of every contract", "ALL");
        for c in &spec.contracts {
            check_contract(spec, c, &mut module, &mut meta, &mut out);
        }
        for component in implementing_components(spec) {
            let what = format!("component \"{}\"", component.id);
            if let Some(name) = render::checked_name(
                &mut out,
                &component.id,
                &what,
                naming::pascal(&component.id),
            ) {
                module.claim_type(&mut out, &component.id, &what, &name);
            }
            let mut methods = Namespace::new(format!("trait of {what}"));
            for c in contracts_of(spec, &component.id) {
                if let Ok(name) = naming::snake(&c.id) {
                    methods.claim(&mut out, &c.id, &format!("contract \"{}\"", c.id), &name);
                }
            }
        }
        out
    }

    fn emit(&self, spec: &Spec) -> TokenStream {
        let mut uses = Uses::default();
        let items: Vec<TokenStream> = spec
            .contracts
            .iter()
            .map(|c| contract_items(spec, c, &mut uses))
            .collect();
        let traits: Vec<TokenStream> = implementing_components(spec)
            .into_iter()
            .map(|component| component_trait(spec, component))
            .collect();
        let meta = meta_module(spec);
        let serde = if items.is_empty() {
            TokenStream::new()
        } else {
            quote! {
                use std::fmt;

                use serde::{Deserialize, Serialize};
            }
        };
        let crate_uses = uses.import();
        quote! {
            #![doc = " The contracts of the specification: request, response and error types, one trait per implementing component, and what the specification states about each contract."]
            #serde
            #crate_uses
            #(#items)*
            #(#traits)*
            #meta
        }
    }
}

fn check_contract(
    spec: &Spec,
    c: &Contract,
    module: &mut Namespace,
    meta: &mut Namespace,
    out: &mut Vec<Diagnostic>,
) {
    let what = format!("contract \"{}\"", c.id);
    if let Some(name) = render::checked_name(out, &c.id, &what, naming::pascal(&c.id)) {
        for suffix in ["Request", "Response", "Error"] {
            module.claim_type(
                out,
                &c.id,
                &format!("the {} type of {what}", suffix.to_ascii_lowercase()),
                &format!("{name}{suffix}"),
            );
        }
    }
    if let Ok(name) = naming::upper_snake(&c.id) {
        meta.claim(out, &c.id, &what, &name);
    }

    for (fields, kind, code) in [
        (&c.inputs, "input", "ready.contract-input-type-undeclared"),
        (
            &c.outputs,
            "output",
            "ready.contract-output-type-undeclared",
        ),
    ] {
        let mut names = Namespace::new(format!("the {kind}s of {what}"));
        for f in fields {
            let fwhat = format!("{kind} \"{}\" of {what}", f.name);
            if let Some(name) = render::checked_name(out, &c.id, &fwhat, naming::snake(&f.name)) {
                names.claim(out, &c.id, &fwhat, &name);
            }
            if !f
                .type_name
                .as_deref()
                .is_some_and(|t| resolve::resolves(spec, t))
            {
                out.push(Diagnostic::new(
                    code,
                    &c.id,
                    format!(
                        "The {kind} \"{}\" of contract \"{}\" has {}.",
                        f.name,
                        c.id,
                        resolve::unresolved_text(f.type_name.as_deref())
                    ),
                ));
            }
        }
    }

    let mut variants = Namespace::new(format!("the errors of {what}"));
    for e in &c.errors {
        let ewhat = format!("error \"{}\" of {what}", e.name);
        if let Some(name) = render::checked_name(out, &c.id, &ewhat, naming::pascal(&e.name)) {
            variants.claim(out, &c.id, &ewhat, &name);
        }
        if let Some(t) = &e.type_name {
            if !resolve::resolves(spec, t) {
                out.push(Diagnostic::new(
                    "ready.contract-error-type-undeclared",
                    &c.id,
                    format!(
                        "The error \"{}\" of contract \"{}\" has {}.",
                        e.name,
                        c.id,
                        resolve::unresolved_text(Some(t))
                    ),
                ));
            }
        }
    }

    match c.realisation {
        None => out.push(Diagnostic::new(
            "ready.contract-realisation-missing",
            &c.id,
            format!(
                "Contract \"{}\" does not say whether it is generated or hand-written (\"realisation\").",
                c.id
            ),
        )),
        Some(Realisation::Hand) => {
            if !c.reference.as_deref().is_some_and(is_hole_ref) {
                let found = c
                    .reference
                    .as_ref()
                    .map_or_else(|| "none".to_string(), |r| format!("\"{r}\""));
                out.push(Diagnostic::new(
                    "ready.hole-ref-invalid",
                    &c.id,
                    format!(
                        "Hand-written contract \"{}\" needs a \"ref\" such as H-012; found {found}.",
                        c.id
                    ),
                ));
            }
            if c.implemented_by.is_none() {
                out.push(Diagnostic::new(
                    "ready.hole-implemented-by-missing",
                    &c.id,
                    format!(
                        "Hand-written contract \"{}\" names no component in \"implementedBy\".",
                        c.id
                    ),
                ));
            }
            if c.postconditions.is_empty() {
                out.push(Diagnostic::new(
                    "ready.hole-postcondition-missing",
                    &c.id,
                    format!(
                        "Hand-written contract \"{}\" states no postcondition, so nothing says what its implementation must do.",
                        c.id
                    ),
                ));
            }
        }
        Some(Realisation::Generated) => {}
    }

    if let Some(component) = &c.implemented_by {
        if spec.component(component).is_none() {
            out.push(Diagnostic::new(
                COMPONENT_UNKNOWN,
                &c.id,
                format!(
                    "Contract \"{}\" is implemented by \"{component}\", which is not a component.",
                    c.id
                ),
            ));
        }
    }

    if let Some(n) = &c.timeout_ms {
        if n.as_u64().is_none_or(|v| v == 0) {
            out.push(Diagnostic::new(
                TIMEOUT_INVALID,
                &c.id,
                format!(
                    "The timeout of contract \"{}\" must be a positive integer number of milliseconds; found {n}.",
                    c.id
                ),
            ));
        }
    }
}

/// The components that implement at least one contract, in declaration order.
fn implementing_components(spec: &Spec) -> Vec<&crate::ir::Component> {
    spec.components
        .iter()
        .filter(|component| {
            spec.contracts
                .iter()
                .any(|c| !c.target && c.implemented_by.as_deref() == Some(component.id.as_str()))
        })
        .collect()
}

/// The contracts a component implements, in declaration order.
fn contracts_of<'a>(spec: &'a Spec, component: &str) -> impl Iterator<Item = &'a Contract> + 'a {
    let component = component.to_string();
    spec.contracts
        .iter()
        .filter(move |c| !c.target && c.implemented_by.as_deref() == Some(component.as_str()))
}

fn type_names(c: &Contract) -> (String, String, String) {
    let base = naming::pascal(&c.id).unwrap_or_default();
    (
        format!("{base}Request"),
        format!("{base}Response"),
        format!("{base}Error"),
    )
}

fn data_struct(
    spec: &Spec,
    name: &str,
    doc: Doc,
    fields: &[ContractField],
    case: WireCase,
    uses: &mut Uses,
) -> TokenStream {
    let name = naming::ident(name);
    let total = fields.iter().all(|f| {
        f.type_name
            .as_deref()
            .is_some_and(|t| resolve::is_total(spec, &TypeRef::parse(t)))
    });
    let derives = render::derives(total);
    let doc = doc.tokens();
    let fields: Vec<TokenStream> = fields
        .iter()
        .map(|f| {
            let ty = render::rust_type(
                spec,
                &TypeRef::parse(f.type_name.as_deref().unwrap_or_default()),
                Scope::Other,
                uses,
            );
            Field {
                wire_name: case.name(&f.name),
                rust_name: naming::snake(&f.name).unwrap_or_default(),
                ty,
                optional: f.optional,
                doc: Doc::new(
                    f.description
                        .as_deref()
                        .unwrap_or(&format!("Field `{}`.", f.name)),
                ),
                deserialize_with: None,
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

fn contract_items(spec: &Spec, c: &Contract, uses: &mut Uses) -> TokenStream {
    let (request, response, error) = type_names(c);
    let mut request_doc = Doc::new(&format!("The request of contract `{}`: {}.", c.id, c.name));
    if let Some(d) = &c.description {
        request_doc.paragraph(d);
    }
    let case = wire::contract_case(spec, c);
    let request = data_struct(spec, &request, request_doc, &c.inputs, case, uses);
    let response = data_struct(
        spec,
        &response,
        Doc::new(&format!("The response of contract `{}`: {}.", c.id, c.name)),
        &c.outputs,
        case,
        uses,
    );

    let error_name = naming::ident(&error);
    let total = c.errors.iter().all(|e| {
        e.type_name
            .as_deref()
            .is_none_or(|t| resolve::is_total(spec, &TypeRef::parse(t)))
    });
    let derives = render::derives(total);
    let error_doc = Doc::new(&format!("The errors of contract `{}`: {}.", c.id, c.name)).tokens();
    let mut variants = Vec::new();
    let mut arms = Vec::new();
    for e in &c.errors {
        let variant = naming::ident(&naming::pascal(&e.name).unwrap_or_default());
        let doc = Doc::new(
            e.condition
                .as_deref()
                .unwrap_or(&format!("Error `{}`.", e.name)),
        )
        .tokens();
        let wire = &case.name(&e.name);
        match &e.type_name {
            Some(t) => {
                let ty = render::rust_type(spec, &TypeRef::parse(t), Scope::Other, uses);
                variants.push(quote! {
                    #doc
                    #[serde(rename = #wire)]
                    #variant(#ty),
                });
                arms.push(quote! { Self::#variant(_) => f.write_str(#wire), });
            }
            None => {
                variants.push(quote! {
                    #doc
                    #[serde(rename = #wire)]
                    #variant,
                });
                arms.push(quote! { Self::#variant => f.write_str(#wire), });
            }
        }
    }
    // An error enum with no variants has no value to format, so its formatter goes unused.
    let (formatter, display_body) = if arms.is_empty() {
        (quote! { _ }, quote! { match *self {} })
    } else {
        (quote! { f }, quote! { match self { #(#arms)* } })
    };

    quote! {
        #request
        #response

        #error_doc
        #derives
        pub enum #error_name {
            #(#variants)*
        }

        impl fmt::Display for #error_name {
            fn fmt(&self, #formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                #display_body
            }
        }

        impl std::error::Error for #error_name {}
    }
}

/// The full description of a contract: on its trait method.
fn contract_doc(c: &Contract) -> Doc {
    let mut doc = Doc::new(&c.name);
    if let Some(d) = &c.description {
        doc.paragraph(d);
    }
    doc.list("Preconditions:", &c.preconditions);
    doc.list("Postconditions:", &c.postconditions);
    if let Some(text) = access_text(c) {
        doc.paragraph(&format!("Authorisation: {text}"));
    }
    match c.idempotent {
        Some(true) => {
            doc.paragraph("Idempotent: repeating a call has the effect of making it once.");
        }
        Some(false) => {
            doc.paragraph("Not idempotent: a repeated call may take effect again.");
        }
        None => {}
    }
    if let Some(ms) = c.timeout_ms.as_ref().and_then(serde_json::Number::as_u64) {
        doc.paragraph(&format!("Timeout: {ms} ms."));
    }
    let reference = c
        .reference
        .as_deref()
        .map(|r| format!(", reference {r}"))
        .unwrap_or_default();
    doc.paragraph(&format!("Contract `{}`{reference}.", c.id));
    if c.realisation == Some(Realisation::Hand) {
        let r = c.reference.as_deref().unwrap_or_default();
        doc.paragraph(&format!(
            "Hole {r}: hand-written. The implementing item carries `/// Implements: {r}`."
        ));
    }
    doc
}

/// Who may call contract `c`, in words built from its exposure and structured auth (the prose
/// auth when that is all it states).
pub fn access_text(c: &Contract) -> Option<String> {
    let Some(auth) = &c.auth else {
        return c.auth_prose.clone();
    };
    let exposure = match c.exposure {
        Some(Exposure::Public) => "public",
        Some(Exposure::Internal) => "internal",
        Some(Exposure::Local) => "local",
        None => "of unstated exposure",
    };
    let mut text = format!("{exposure}, scheme {}", auth.scheme.name());
    if !auth.roles.is_empty() {
        text.push_str(&format!(", roles {}", auth.roles.join(" or ")));
    }
    if auth.tenant_policies {
        text.push_str(", the tenant's policies decide");
    }
    if auth.csrf {
        text.push_str(", request token checked");
    }
    if auth.origin {
        text.push_str(", origin checked");
    }
    if let Some(age) = &auth.step_up {
        text.push_str(&format!(", authenticated within {age}"));
    }
    if let Some(secret) = &auth.secret {
        text.push_str(&format!(", verified with the secret {secret}"));
    }
    text.push('.');
    if let Some(words) = &auth.description {
        text.push_str(&format!(" {words}"));
    }
    Some(text)
}

fn component_trait(spec: &Spec, component: &crate::ir::Component) -> TokenStream {
    let name = naming::ident(&naming::pascal(&component.id).unwrap_or_default());
    let doc = Doc::new(&format!(
        "The contracts component `{}` ({}) implements.",
        component.id, component.name
    ))
    .tokens();
    let methods: Vec<TokenStream> = contracts_of(spec, &component.id)
        .map(|c| {
            let method = naming::ident(&naming::snake(&c.id).unwrap_or_default());
            let (request, response, error) = type_names(c);
            let (request, response, error) = (
                naming::ident(&request),
                naming::ident(&response),
                naming::ident(&error),
            );
            let doc = contract_doc(c).tokens();
            quote! {
                #doc
                fn #method(
                    &self,
                    request: #request,
                ) -> impl std::future::Future<Output = Result<#response, #error>> + Send;
            }
        })
        .collect();
    quote! {
        #doc
        pub trait #name {
            #(#methods)*
        }
    }
}

fn option_str(value: Option<&str>) -> TokenStream {
    match value {
        Some(s) => quote! { Some(#s) },
        None => quote! { None },
    }
}

fn meta_module(spec: &Spec) -> TokenStream {
    let mut consts = Vec::new();
    let mut names = Vec::new();
    for c in &spec.contracts {
        let name = naming::ident(&naming::upper_snake(&c.id).unwrap_or_default());
        let id = &c.id;
        let contract_name = &c.name;
        let reference = option_str(c.reference.as_deref());
        let realisation = match c.realisation {
            Some(Realisation::Hand) => quote! { Realisation::Hand },
            _ => quote! { Realisation::Generated },
        };
        let implemented_by = option_str(c.implemented_by.as_deref());
        let auth = option_str(c.auth.as_ref().map(|a| a.scheme.name()));
        let idempotent = match c.idempotent {
            Some(true) => quote! { Some(true) },
            Some(false) => quote! { Some(false) },
            None => quote! { None },
        };
        let timeout_ms = match c.timeout_ms.as_ref().and_then(serde_json::Number::as_u64) {
            Some(ms) => {
                let lit = Literal::u64_unsuffixed(ms);
                quote! { Some(#lit) }
            }
            None => quote! { None },
        };
        let doc = format!(" Contract `{id}`: {contract_name}.");
        consts.push(quote! {
            #[doc = #doc]
            pub const #name: ContractMeta = ContractMeta {
                id: #id,
                name: #contract_name,
                reference: #reference,
                realisation: #realisation,
                implemented_by: #implemented_by,
                auth: #auth,
                idempotent: #idempotent,
                timeout_ms: #timeout_ms,
            };
        });
        names.push(name);
    }
    let import = if spec.contracts.is_empty() {
        quote! { use crate::prelude::ContractMeta; }
    } else {
        quote! { use crate::prelude::{ContractMeta, Realisation}; }
    };
    quote! {
        /// What the specification states about each contract beyond its data.
        pub mod meta {
            #import

            #(#consts)*

            /// Every contract, in declaration order.
            pub const ALL: &[ContractMeta] = &[#(#names),*];
        }
    }
}
