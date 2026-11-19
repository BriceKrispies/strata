//! Machine handlers (the `handlers` module): the glue that brings an incoming request or event to
//! a machine and runs it through `strata_runtime::Host`.
//!
//! An event a machine receives arrives in one of three ways the specification can state:
//!
//! - **Through a contract.** The event names the contract (`contract`, with `role` `request` or
//!   no role). The machine's handler module gets `<contract>_event(&request) -> (Key, Event)`,
//!   which builds the event from the request's inputs of the same names, and
//!   `<contract>(&host, &request)`, which steps the instance through `Host::handle`. The key is
//!   the request's input named like the machine's key field (`storage.key`, or `instances.key`),
//!   of the key field's type; every payload field is the input of its name, of its type and
//!   optionality. A request that cannot give them is refused with [`KEY`] or [`PAYLOAD`].
//! - **Published by another machine.** Another implementation-level machine emits the event to
//!   this machine's behavior id (`emit` with `to` naming it). The handler module gets
//!   `from_<emitter>(&emitted) -> Option<(Key, Event)>` and `deliver_from_<emitter>(&host,
//!   &emitted)`, for the emitter's dispatcher to call. The key is the payload field named like
//!   the key field, of its type, refused with [`KEY`] otherwise.
//! - **Fired by the machine's own timer.** Nothing is needed: the host fires it.
//!
//! For an event the specification does not say how it arrives, nothing is generated, and the
//! machine's handler module lists it in its documentation.

use std::collections::BTreeSet;

use proc_macro2::TokenStream;
use quote::quote;

use super::{view, Backend};
use crate::diagnostic::Diagnostic;
use crate::ir::{Behavior, Contract, Effect, Event, Spec};
use crate::naming;
use crate::render::{Doc, Namespace};

/// Code of a request or published event that does not carry the machine's key.
pub const KEY: &str = "specc.wiring-handler-key";
/// Code of a request that does not carry an event's payload field.
pub const PAYLOAD: &str = "specc.wiring-handler-payload";
/// Code of an event that names a contract that is not declared.
pub const CONTRACT_UNKNOWN: &str = "specc.wiring-event-contract-unknown";

/// The handlers backend.
pub struct HandlersBackend;

/// How one received event reaches a machine.
enum Arrival<'a> {
    Contract(&'a Contract),
    Published(&'a Behavior),
    Timer,
    Unstated,
}

/// The events machine `b` receives, in the order of the events section.
fn received<'a>(spec: &'a Spec, b: &Behavior) -> Vec<&'a Event> {
    let mut ids = BTreeSet::new();
    for t in &b.transitions {
        ids.insert(t.on.as_str());
        for e in &t.effects {
            if let Effect::Timer { event, .. } = e {
                ids.insert(event.as_str());
            }
        }
    }
    for s in &b.states {
        ids.extend(s.ignores.iter().map(String::as_str));
    }
    spec.events
        .iter()
        .filter(|e| ids.contains(e.id.as_str()))
        .collect()
}

/// The events machine `b` emits, in the order of the events section.
fn emitted<'a>(spec: &'a Spec, b: &Behavior) -> Vec<&'a Event> {
    let ids: BTreeSet<&str> = b
        .transitions
        .iter()
        .flat_map(|t| &t.effects)
        .filter_map(|e| match e {
            Effect::Emit { event, .. } => Some(event.as_str()),
            _ => None,
        })
        .collect();
    spec.events
        .iter()
        .filter(|e| ids.contains(e.id.as_str()))
        .collect()
}

fn timer_fired(b: &Behavior, event: &str) -> bool {
    b.transitions
        .iter()
        .flat_map(|t| &t.effects)
        .any(|e| matches!(e, Effect::Timer { event: fired, .. } if fired == event))
}

/// The machines that emit `event` to machine `b`, in declaration order.
fn publishers<'a>(spec: &'a Spec, b: &Behavior, event: &str) -> Vec<&'a Behavior> {
    spec.machines
        .iter()
        .filter(|m| {
            m.transitions.iter().flat_map(|t| &t.effects).any(|e| {
                matches!(e, Effect::Emit { event: emitted, to: Some(to), .. } if emitted == event && *to == b.id)
            })
        })
        .collect()
}

/// How each received event reaches machine `b`. One event can arrive in several ways.
fn arrivals<'a>(spec: &'a Spec, b: &'a Behavior) -> Vec<(&'a Event, Vec<Arrival<'a>>)> {
    received(spec, b)
        .into_iter()
        .map(|e| {
            let mut ways = Vec::new();
            if matches!(e.role.as_deref(), None | Some("request")) {
                if let Some(c) = e
                    .contract
                    .as_deref()
                    .and_then(|id| spec.contracts.iter().find(|c| c.id == id))
                {
                    ways.push(Arrival::Contract(c));
                }
            }
            ways.extend(
                publishers(spec, b, &e.id)
                    .into_iter()
                    .map(Arrival::Published),
            );
            if timer_fired(b, &e.id) {
                ways.push(Arrival::Timer);
            }
            if ways.is_empty() {
                ways.push(Arrival::Unstated);
            }
            (e, ways)
        })
        .collect()
}

/// The machine's key field: its name and its type name.
fn key_field(spec: &Spec, b: &Behavior) -> Option<(String, String)> {
    let (record, key) = b
        .storage
        .as_ref()
        .and_then(|s| Some((s.record.clone()?, s.key.clone()?)))
        .or_else(|| {
            b.instances
                .as_ref()
                .and_then(|i| Some((i.stored_in.clone()?, i.key.clone()?)))
        })?;
    let field = spec.record(&record)?.field(&key)?;
    Some((key, field.type_name.clone()))
}

fn snake(name: &str) -> String {
    naming::snake(name).unwrap_or_default()
}

fn pascal(name: &str) -> String {
    naming::pascal(name).unwrap_or_default()
}

impl Backend for HandlersBackend {
    fn module(&self) -> &'static str {
        "handlers"
    }

    fn wanted(&self, spec: &Spec) -> bool {
        !spec.machines.is_empty()
    }

    fn check(&self, spec: &Spec) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        for e in &spec.events {
            if let Some(c) = &e.contract {
                if !spec.contracts.iter().any(|k| k.id == *c) {
                    out.push(Diagnostic::new(
                        CONTRACT_UNKNOWN,
                        &e.id,
                        format!(
                            "Event \"{}\" is a message of \"{c}\", which is not a contract.",
                            e.id
                        ),
                    ));
                }
            }
        }
        let mut modules = Namespace::new("module handlers");
        for b in &spec.machines {
            if let Ok(name) = naming::snake(&b.id) {
                modules.claim(&mut out, &b.id, &format!("machine \"{}\"", b.id), &name);
            }
            // The machines backend refuses a machine without a key field.
            let Some((key, key_type)) = key_field(spec, b) else {
                continue;
            };
            let mut names = Namespace::new(format!("the handlers of machine \"{}\"", b.id));
            for (e, ways) in arrivals(spec, b) {
                for way in ways {
                    match way {
                        Arrival::Contract(c) => {
                            check_contract(&mut out, b, e, c, &key, &key_type);
                            let n = snake(&c.id);
                            let what = format!("the handler of contract \"{}\"", c.id);
                            names.claim(&mut out, &b.id, &what, &n);
                            names.claim(&mut out, &b.id, &what, &format!("{n}_event"));
                        }
                        Arrival::Published(m) => {
                            let carries = e.payload.iter().any(|f| {
                                f.name == key
                                    && !f.optional
                                    && f.type_name.as_deref() == Some(key_type.as_str())
                            });
                            if !carries {
                                out.push(Diagnostic::new(
                                    KEY,
                                    &b.id,
                                    format!(
                                        "Machine \"{}\" emits event \"{}\" to machine \"{}\", whose payload has no field \"{key}\" of type {key_type} to key the instance by.",
                                        m.id, e.id, b.id
                                    ),
                                ));
                            }
                        }
                        Arrival::Timer | Arrival::Unstated => {}
                    }
                }
            }
            let mut seen = BTreeSet::new();
            for (_, ways) in arrivals(spec, b) {
                for way in ways {
                    if let Arrival::Published(m) = way {
                        if seen.insert(m.id.clone()) {
                            let n = snake(&m.id);
                            let what = format!("the delivery from machine \"{}\"", m.id);
                            names.claim(&mut out, &b.id, &what, &format!("from_{n}"));
                            names.claim(&mut out, &b.id, &what, &format!("deliver_from_{n}"));
                        }
                    }
                }
            }
        }
        out
    }

    fn emit(&self, spec: &Spec) -> TokenStream {
        let modules: Vec<TokenStream> = spec
            .machines
            .iter()
            .map(|b| machine_module(spec, b))
            .collect();
        quote! {
            #![doc = " How incoming requests and events reach the implementation-level machines: for each machine, the glue that turns a contract's request or another machine's emitted event into the machine's event and runs it through `strata_runtime::Host`."]
            #![doc = ""]
            #![doc = " Events whose arrival the specification does not state get nothing, and are listed on their machine's module."]
            #(#modules)*
        }
    }
}

fn check_contract(
    out: &mut Vec<Diagnostic>,
    b: &Behavior,
    e: &Event,
    c: &Contract,
    key: &str,
    key_type: &str,
) {
    let key_input = c.inputs.iter().find(|i| i.name == key);
    if !key_input.is_some_and(|i| !i.optional && i.type_name.as_deref() == Some(key_type)) {
        out.push(Diagnostic::new(
            KEY,
            &b.id,
            format!(
                "Event \"{}\" of machine \"{}\" arrives through contract \"{}\", which has no required input \"{key}\" of type {key_type} to key the instance by.",
                e.id, b.id, c.id
            ),
        ));
    }
    for f in &e.payload {
        let fits = c
            .inputs
            .iter()
            .any(|i| i.name == f.name && i.optional == f.optional && i.type_name == f.type_name);
        if !fits {
            out.push(Diagnostic::new(
                PAYLOAD,
                &b.id,
                format!(
                    "Event \"{}\" of machine \"{}\" arrives through contract \"{}\", which has no input \"{}\" of the payload field's type{}.",
                    e.id,
                    b.id,
                    c.id,
                    f.name,
                    if f.optional { ", optional" } else { "" }
                ),
            ));
        }
    }
}

/// The payload of event `e` built from fields of `source` of the same names.
fn payload_from(machine: &TokenStream, e: &Event, source: &TokenStream) -> TokenStream {
    let variant = view::pascal_ident(&e.id);
    let payload = naming::ident(&format!("{}Payload", pascal(&e.id)));
    let fields: Vec<_> = e
        .payload
        .iter()
        .map(|f| view::snake_ident(&f.name))
        .collect();
    quote! {
        #machine::Event::#variant(#machine::#payload {
            #( #fields: #source.#fields.clone(), )*
        })
    }
}

fn machine_module(spec: &Spec, b: &Behavior) -> TokenStream {
    let module = view::snake_ident(&b.id);
    let machine = quote! { crate::machines::#module };
    let machine_ty = view::pascal_ident(&b.id);
    let key = key_field(spec, b).map(|(k, _)| k).unwrap_or_default();
    let key_ident = view::snake_ident(&key);

    let mut lines = Vec::new();
    let mut unstated = Vec::new();
    let mut items = Vec::new();
    let mut publishers_seen: Vec<&Behavior> = Vec::new();
    for (e, ways) in arrivals(spec, b) {
        for way in ways {
            match way {
                Arrival::Contract(c) => {
                    lines.push(format!("`{}`: through contract `{}`.", e.id, c.id));
                    let handler = view::snake_ident(&c.id);
                    let builder = naming::ident(&format!("{}_event", snake(&c.id)));
                    let request = view::request_path(&c.id);
                    let event = payload_from(&machine, e, &quote! { request });
                    let build_doc = format!(
                        " The instance key and the event `{}` a request of contract `{}` carries to machine `{}`.",
                        e.id, c.id, b.id
                    );
                    let handle_doc = format!(
                        " Steps machine `{}` with the event `{}` a request of contract `{}` carries, then dispatches its emits.",
                        b.id, e.id, c.id
                    );
                    items.push(quote! {
                        #[doc = #build_doc]
                        pub fn #builder(request: &#request) -> (#machine::Key, #machine::Event) {
                            (request.#key_ident.clone(), #event)
                        }

                        #[doc = #handle_doc]
                        pub async fn #handler<S, D>(
                            host: &strata_runtime::Host<#machine::#machine_ty, S, D>,
                            request: &#request,
                        ) -> Result<strata_runtime::Handled<#machine::#machine_ty>, strata_runtime::HostError>
                        where
                            S: strata_runtime::MachineStore,
                            D: strata_runtime::Dispatcher<#machine::Emitted>,
                        {
                            let (key, event) = #builder(request);
                            host.handle(key, event).await
                        }
                    });
                }
                Arrival::Published(m) => {
                    lines.push(format!("`{}`: published by machine `{}`.", e.id, m.id));
                    if !publishers_seen.iter().any(|p| p.id == m.id) {
                        publishers_seen.push(m);
                    }
                }
                Arrival::Timer => {
                    lines.push(format!("`{}`: fired by the machine's own timers.", e.id));
                }
                Arrival::Unstated => unstated.push(format!("`{}`", e.id)),
            }
        }
    }
    for m in publishers_seen {
        let from_module = view::snake_ident(&m.id);
        let from = quote! { crate::machines::#from_module };
        let receives: BTreeSet<&str> = received(spec, b).iter().map(|e| e.id.as_str()).collect();
        let published: Vec<&Event> = emitted(spec, m)
            .into_iter()
            .filter(|e| {
                receives.contains(e.id.as_str())
                    && publishers(spec, b, &e.id).iter().any(|p| p.id == m.id)
            })
            .collect();
        let all = published.len() == emitted(spec, m).len();
        let arms: Vec<TokenStream> = published
            .iter()
            .map(|e| {
                let variant = view::pascal_ident(&e.id);
                let event = payload_from(&machine, e, &quote! { payload });
                quote! {
                    #from::Emitted::#variant(payload) => Some((payload.#key_ident.clone(), #event)),
                }
            })
            .collect();
        let rest = if all {
            TokenStream::new()
        } else {
            quote! { _ => None, }
        };
        let from_fn = naming::ident(&format!("from_{}", snake(&m.id)));
        let deliver_fn = naming::ident(&format!("deliver_from_{}", snake(&m.id)));
        let from_doc = format!(
            " The instance key and event of machine `{}` that an event machine `{}` emitted to it carries, or `None` for an event `{}` does not publish to it.",
            b.id, m.id, m.id
        );
        let deliver_doc = format!(
            " Steps machine `{}` with an event machine `{}` emitted to it, then dispatches its emits; `Ok(None)` when the event is not one `{}` publishes to it. A dispatcher of machine `{}` calls it for the receiver `{}`.",
            b.id, m.id, m.id, m.id, b.id
        );
        let matched = quote! {
            match emitted {
                #(#arms)*
                #rest
            }
        };
        items.push(quote! {
            #[doc = #from_doc]
            pub fn #from_fn(emitted: &#from::Emitted) -> Option<(#machine::Key, #machine::Event)> {
                #matched
            }

            #[doc = #deliver_doc]
            pub async fn #deliver_fn<S, D>(
                host: &strata_runtime::Host<#machine::#machine_ty, S, D>,
                emitted: &#from::Emitted,
            ) -> Result<Option<strata_runtime::Handled<#machine::#machine_ty>>, strata_runtime::HostError>
            where
                S: strata_runtime::MachineStore,
                D: strata_runtime::Dispatcher<#machine::Emitted>,
            {
                match #from_fn(emitted) {
                    Some((key, event)) => host.handle(key, event).await.map(Some),
                    None => Ok(None),
                }
            }
        });
    }
    let mut doc = Doc::new(&format!(
        "How events reach machine `{}` ({}).",
        b.id, b.name
    ));
    doc.list("Arrivals:", &lines);
    if !unstated.is_empty() {
        doc.paragraph(&format!(
            "The specification does not say how these events arrive, so nothing is generated for them: {}.",
            unstated.join(", ")
        ));
    }
    let doc = doc.tokens();
    let allow = if items.is_empty() {
        TokenStream::new()
    } else {
        quote! { #![allow(clippy::clone_on_copy)] }
    };
    quote! {
        #doc
        pub mod #module {
            #allow
            #(#items)*
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load::load;

    fn spec(contract_inputs: &str) -> Spec {
        load(
            "s",
            &format!(
                r#"{{ "system": {{ "id": "s", "name": "S" }},
                "components": [ {{ "id": "svc", "name": "Service" }} ],
                "records": [ {{ "id": "rec", "name": "R", "owner": "svc", "key": ["id"], "schemaVersion": 1,
                    "fields": [ {{ "name": "id", "type": "string" }}, {{ "name": "state", "type": "string" }} ] }} ],
                "contracts": [ {{ "id": "poke", "name": "Poke", "realisation": "generated", "implementedBy": "svc",
                    "inputs": [ {contract_inputs} ] }} ],
                "events": [ {{ "id": "poked", "name": "Poked", "contract": "poke", "role": "request",
                                 "payload": [ {{ "name": "n", "type": "u32" }} ] }},
                            {{ "id": "nudged", "name": "Nudged" }} ],
                "behaviors": [ {{ "id": "m", "name": "M", "level": "implementation", "initial": "a",
                    "storage": {{ "record": "rec", "key": "id", "creates": true }},
                    "states": [ {{ "id": "a", "name": "A" }} ],
                    "transitions": [ {{ "id": "t", "from": "a", "to": "a", "on": "poked" }},
                                     {{ "id": "u", "from": "a", "to": "a", "on": "nudged" }} ] }} ] }}"#
            ),
        )
        .unwrap()
    }

    fn codes(s: &Spec) -> Vec<String> {
        HandlersBackend
            .check(s)
            .into_iter()
            .map(|d| d.code)
            .collect()
    }

    #[test]
    fn an_event_through_a_contract_gets_a_handler_and_an_unstated_one_is_listed() {
        let s = spec(r#"{ "name": "id", "type": "string" }, { "name": "n", "type": "u32" }"#);
        assert!(codes(&s).is_empty());
        let text = HandlersBackend.emit(&s).to_string();
        assert!(text.contains("pub fn poke_event"));
        assert!(text.contains("pub async fn poke"));
        assert!(text.contains("`nudged`"));
    }

    #[test]
    fn a_request_without_the_key_or_a_payload_field_is_refused() {
        let s = spec(r#"{ "name": "n", "type": "string" }"#);
        assert_eq!(codes(&s), [KEY, PAYLOAD]);
    }
}
