//! Dependency wiring (the `deps` module): what each component's implementation may call.
//!
//! For every component that implements a contract, `<C>Deps` holds exactly one client
//! (`routes::<D>Client`) per component `<D>` it calls: the `to` of each relationship of kind
//! `calls` from it, where that component implements contracts too. `<C>Deps::new` takes exactly
//! those clients and one accessor returns each, so an implementation handed its `Deps` reaches
//! what the specification says it calls and nothing else: a call the specification does not
//! declare has no client to make it through and does not compile.
//!
//! A `calls` relationship to a component that implements no contract, to the component itself,
//! or to something that is not a component (a port) gives no client; the struct's documentation
//! lists it. The relationship's own `contract` property does not narrow the client.
//!
//! The compile-time guarantee, shown on the wiring fixture, where `scheduler` calls `reports` and
//! `gateway` calls `scheduler`:
//!
//! ```
//! use strata_spec_compiler_wiring_fixture::deps::SchedulerDeps;
//! use strata_spec_compiler_wiring_fixture::routes::ReportsClient;
//!
//! fn reach(deps: &SchedulerDeps) -> &ReportsClient {
//!     deps.reports()
//! }
//! ```
//!
//! and a call `scheduler` does not declare (it does not call `gateway`) does not compile:
//!
//! ```compile_fail
//! use strata_spec_compiler_wiring_fixture::deps::SchedulerDeps;
//! use strata_spec_compiler_wiring_fixture::routes::GatewayClient;
//!
//! fn reach(deps: &SchedulerDeps) -> &GatewayClient {
//!     deps.gateway()
//! }
//! ```

use proc_macro2::TokenStream;
use quote::quote;

use super::routes::{contracts_of, implementing_components};
use super::{view, Backend};
use crate::diagnostic::Diagnostic;
use crate::ir::{Component, Spec};
use crate::naming;
use crate::render::{Doc, Namespace};

/// The deps backend.
pub struct DepsBackend;

/// Why a `calls` relationship gives no client.
enum NoClient {
    Itself,
    NoContracts,
    NotAComponent,
}

/// The components `component` calls that it gets a client for, and those it does not, with why.
fn callees<'a>(
    spec: &'a Spec,
    component: &Component,
) -> (Vec<&'a Component>, Vec<(String, NoClient)>) {
    let mut clients = Vec::new();
    let mut without = Vec::new();
    for to in &component.calls {
        match spec.component(to) {
            _ if *to == component.id => without.push((to.clone(), NoClient::Itself)),
            Some(callee) if !contracts_of(spec, &callee.id).is_empty() => clients.push(callee),
            Some(_) => without.push((to.clone(), NoClient::NoContracts)),
            None => without.push((to.clone(), NoClient::NotAComponent)),
        }
    }
    (clients, without)
}

impl Backend for DepsBackend {
    fn module(&self) -> &'static str {
        "deps"
    }

    fn wanted(&self, spec: &Spec) -> bool {
        !implementing_components(spec).is_empty()
    }

    fn check(&self, spec: &Spec) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut module = Namespace::new("module deps");
        for component in implementing_components(spec) {
            if let Ok(name) = naming::pascal(&component.id) {
                module.claim_type(
                    &mut out,
                    &component.id,
                    &format!("the dependencies of component \"{}\"", component.id),
                    &format!("{name}Deps"),
                );
            }
            let mut fields = Namespace::new(format!(
                "the dependencies of component \"{}\"",
                component.id
            ));
            fields.claim(&mut out, &component.id, "the constructor", "new");
            for callee in callees(spec, component).0 {
                if let Ok(name) = naming::snake(&callee.id) {
                    fields.claim(
                        &mut out,
                        &callee.id,
                        &format!("the client of component \"{}\"", callee.id),
                        &name,
                    );
                }
            }
        }
        out
    }

    fn emit(&self, spec: &Spec) -> TokenStream {
        let structs: Vec<TokenStream> = implementing_components(spec)
            .into_iter()
            .map(|component| deps_struct(spec, component))
            .collect();
        quote! {
            #![doc = " What each component's implementation may call: one typed client per component it calls, as the specification's `calls` relationships say, and nothing else."]
            #(#structs)*
        }
    }
}

fn deps_struct(spec: &Spec, component: &Component) -> TokenStream {
    let name = naming::ident(&format!(
        "{}Deps",
        naming::pascal(&component.id).unwrap_or_default()
    ));
    let (clients, without) = callees(spec, component);
    let mut doc = Doc::new(&format!(
        "The clients component `{}` ({}) may call through: one per component it calls.",
        component.id, component.name
    ));
    if clients.is_empty() {
        doc.paragraph("It calls no component that implements a contract.");
    }
    let unclientable: Vec<String> = without
        .iter()
        .map(|(to, why)| match why {
            NoClient::Itself => format!("`{to}`: the component itself."),
            NoClient::NoContracts => format!("`{to}`: implements no contract."),
            NoClient::NotAComponent => format!("`{to}`: not a component."),
        })
        .collect();
    doc.list("Calls that give no client:", &unclientable);
    let doc = doc.tokens();
    if clients.is_empty() {
        return quote! {
            #doc
            #[derive(Debug, Clone, Copy, Default)]
            pub struct #name;
        };
    }
    let fields: Vec<_> = clients.iter().map(|c| view::snake_ident(&c.id)).collect();
    let clients_ty: Vec<TokenStream> = clients
        .iter()
        .map(|c| {
            let client = naming::ident(&format!(
                "{}Client",
                naming::pascal(&c.id).unwrap_or_default()
            ));
            quote! { crate::routes::#client<K> }
        })
        .collect();
    let field_docs: Vec<String> = clients
        .iter()
        .map(|c| format!(" The client of component `{}` ({}).", c.id, c.name))
        .collect();
    let new_doc = format!(
        " The dependencies of component `{}`: a client for each component it calls.",
        component.id
    );
    quote! {
        #doc
        #[derive(Debug, Clone)]
        pub struct #name<K: strata_runtime::Credentials = strata_runtime::NoCredentials> {
            #( #fields: #clients_ty, )*
        }

        impl<K: strata_runtime::Credentials> #name<K> {
            #[doc = #new_doc]
            #[allow(clippy::too_many_arguments)]
            pub fn new(#( #fields: #clients_ty ),*) -> Self {
                Self { #( #fields ),* }
            }

            #(
                #[doc = #field_docs]
                pub fn #fields(&self) -> &#clients_ty {
                    &self.#fields
                }
            )*
        }
    }
}
