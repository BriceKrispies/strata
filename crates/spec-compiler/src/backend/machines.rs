//! Implementation-level machines (`behaviors` with `level: "implementation"`) to the `machines`
//! module.
//!
//! Each machine becomes one inline module holding its `State`, `Vars`, `Event` and `Emitted`
//! types, the payload structs of its events, its `Key`, and a unit struct implementing
//! `strata_runtime::Machine`. `step` follows Exegra's semantics (`docs/semantics.md`, normative
//! `scan`, `runEffects` and `fireOne` in `formal/Exegra/Format2.lean`):
//!
//! - among the transitions from the current state on the event, in declaration order, the first
//!   whose guard holds fires (an absent guard holds; `otherwise` is declarative and means the
//!   same); the guards read the variables as they were before the step;
//! - its effects run in the order written, each on the variables as updated by the effects
//!   before it, so a later `set` or payload sees an earlier `set`; the machine then moves to the
//!   transition's target state;
//! - an `emit` becomes an emit of the outcome, addressed to its `to` with its event's
//!   `delivery`; an `emit` with `after` (whole milliseconds) becomes a delayed emit of the
//!   outcome (`Outcome::emit_after`), dispatched to the receiver once that delay has passed; a `timer` sets a named timer firing its event at the same instance after
//!   `after` milliseconds, replacing one of the same name; a `cancel` removes it;
//! - an event with no enabled transition, whether the state lists it in `ignores`, no guard
//!   holds, or the state is terminal, is ignored (`Outcome::ignore`): the same state and
//!   variables, nothing emitted, nothing written (Exegra's `ignored` outcome). An evaluation that
//!   fails (checked arithmetic overflowing, a division by zero) is `StepError::Rejected` and
//!   nothing is committed.
//!
//! After the machines comes the `dispatch` module, the routing table of the system host
//! (`strata_runtime::System`): for every event that is not elapsed time, each machine with a
//! transition on it, reached at the instance a scenario route `by: key` names (the routes of all
//! scenarios together; two that disagree are refused) or, with no route, at every instance. That
//! is how Exegra addresses an entry (`addressed` and `decideRoutes` in `Format2.lean`); an emit's
//! `to` names a receiver outside the machines and does not route it.
//!
//! Realisation and durability. A behavior whose `realisation` is `target` (declared, or by
//! default because its `subject` component is `not-implemented`) models the enterprise target, not
//! code that ships, so it is not in `machines`: it is written to `target_machines`, which the
//! generated crate compiles only with its `target-models` feature (tests and differentials enable
//! it). `target_machines::dispatch` is the routing table of the whole system, shipped and target
//! machines together, as Exegra runs the specification; `machines::dispatch` holds the shipped
//! machines only. A machine with `storage.durability: "memory"` has no record, key or
//! transaction, so it implements no `Machine`: its module has an inherent pure `step` returning a
//! `strata_runtime::MemoryOutcome`, and the `Host` and `System` cannot run it (the routing tables
//! do not name it); whoever owns the instance keeps its state and carries out the effects.
//!
//! Guards and values are translated by `strata-spec-expr`: a variable reads `vars.<field>` in a
//! guard and the updated `next.<field>` in an effect, `event.<field>` reads the payload, and
//! `time` reads the step's `now` (milliseconds since the Unix epoch), which is what the simulator's
//! `time` counts only for a machine whose scenarios count milliseconds (`specc.time-unit`
//! otherwise); a bare-number delay is in the unit of the machine's scenarios
//! (`Behavior::time_units`, Exegra's `machineTimeUnit`) and is converted to whole milliseconds, or
//! refused when it is not a whole number of them. Timestamps, durations and
//! declared identifiers and scalars are read as their representation, so `time + 15` is integer
//! arithmetic in milliseconds, and written back through their constructors.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;

use proc_macro2::{Ident, Literal, TokenStream};
use quote::{format_ident, quote};
use serde_json::Value;
use strata_spec_expr::{self as expr, Binding, ExprKind, Ty};

use super::Backend;
use crate::diagnostic::Diagnostic;
use crate::ir::{Behavior, Effect, Event, Record, RecordField, Spec, Transition, TypeKind};
use crate::naming;
use crate::render::{self, Doc, Field, Namespace, Scope as TypeScope, Uses};
use crate::resolve::{self, Builtin, TypeRef};

/// Code of an expression reading another behavior's variables, an aggregate or a resource.
pub const CROSS_BEHAVIOUR_READ: &str = "specc.cross-behaviour-read";
/// Code of a guard or value that does not parse or type-check against the machine.
pub const EXPRESSION: &str = "specc.machine-expression";
/// Code of a resource effect (`use`, `acquire`, `release`).
pub const RESOURCE_EFFECT: &str = "specc.resource-effect";
/// Code of an emit over a link (`via`, `size`, `fanout`).
pub const EMIT_LINK: &str = "specc.emit-link";
/// Code of an emit whose delay (`after`) is not a whole number of milliseconds (a distribution,
/// an expression or a fraction).
pub const EMIT_AFTER: &str = "specc.emit-after";
/// Code of an emit without a `to` (a reply, or addressing by scenario routes).
pub const EMIT_UNADDRESSED: &str = "specc.emit-unaddressed";
/// Code of a timer whose `after` is not a whole number of milliseconds.
pub const TIMER_AFTER: &str = "specc.timer-after";
/// Code of a machine that reads `time` while its scenarios count time in another unit than
/// milliseconds (generated code's `time` is milliseconds since the Unix epoch).
pub const TIME_UNIT: &str = "specc.time-unit";
/// Code of any other effect the runtime cannot express.
pub const EFFECT_UNSUPPORTED: &str = "specc.effect-unsupported";
/// Code of counted instances (`instances.count`).
pub const INSTANCES_COUNTED: &str = "specc.instances-counted";
/// Code of a machine whose storage record or key is missing or unknown.
pub const STORAGE: &str = "specc.machine-storage";
/// Code of a machine key that is not the whole key of its record (Exegra's rule `storage.key-whole`).
pub const KEY_WHOLE: &str = "storage.key-whole";
/// Code of a `storage.fields` entry naming a variable the machine lacks or a field the record
/// lacks, or of a record field asked to hold two of the key, the state and the variables
/// (Exegra's rule of the same code).
pub const FIELD_UNKNOWN: &str = "ready.storage-field-unknown";
/// Code of a variable held in no field of the storage record.
pub const VARIABLE_UNMAPPED: &str = "ready.storage-variable-unmapped";
/// Code of a variable whose record field has a type its repr does not fit.
pub const FIELD_TYPE: &str = "ready.storage-field-type";
/// Code of a machine state the storage record cannot hold.
pub const STATE_FIELD: &str = "ready.storage-state-field";
/// Code of a required record field a creating machine does not fill.
pub const CREATES_UNFILLED: &str = "ready.storage-creates-unfilled";
/// Code of a reference to an event that is not declared.
pub const EVENT_UNKNOWN: &str = "specc.event-unknown";
/// Code of a reference to a state the machine does not declare.
pub const STATE_UNKNOWN: &str = "specc.state-unknown";
/// Code of a `set` of a variable the machine does not declare.
pub const VARIABLE_UNKNOWN: &str = "specc.variable-unknown";
/// Code of an initial or literal value that cannot be written as its type.
pub const LITERAL: &str = "specc.machine-literal";
/// Code of an emit or timer payload that does not match its event's payload fields.
pub const PAYLOAD_FIELD: &str = "specc.payload-field";
/// Code of an event that stands for elapsed time (`elapsed: true`) and is reachable from outside:
/// a scenario step injects it into an implementation machine, or a scenario routes it to one.
/// Only a timer the machine set may deliver it, and the generated hosts refuse it as an input.
pub const ELAPSED_EXTERNAL: &str = "event.elapsed-external";
/// Code of a machine that receives no event.
pub const NO_EVENTS: &str = "specc.machine-no-events";

/// The machines backend.
pub struct MachinesBackend;

impl Backend for MachinesBackend {
    fn module(&self) -> &'static str {
        "machines"
    }

    fn wanted(&self, spec: &Spec) -> bool {
        !spec.machines.is_empty() || !spec.memory_machines.is_empty()
    }

    fn check(&self, spec: &Spec) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut module = Namespace::new("module machines");
        module.claim(
            &mut out,
            crate::diagnostic::SPEC_ENTITY,
            "the routing table `dispatch`",
            "dispatch",
        );
        let shipped: Vec<&Behavior> = in_declaration_order(&spec.machines, &spec.memory_machines);
        check_machines(spec, &shipped, &mut module, &mut out);
        let record: Vec<&Behavior> = spec.machines.iter().collect();
        routing_rows(spec, &record, &mut out);
        elapsed_external(spec, &record, &mut out);
        out
    }

    fn emit(&self, spec: &Spec) -> TokenStream {
        let shipped: Vec<&Behavior> = in_declaration_order(&spec.machines, &spec.memory_machines);
        let modules = machine_modules(spec, &shipped);
        let record: Vec<&Behavior> = spec.machines.iter().collect();
        let members: Vec<(&Behavior, TokenStream)> =
            record.iter().map(|b| (*b, quote! { super })).collect();
        let dispatch = dispatch_module(spec, &record, &members);
        quote! {
            #![doc = " The shipped implementation-level machines of the specification: for each, its states, variables and events, and its `strata_runtime::Machine` (a machine held in memory has a pure `step` instead); and the routing table that delivers events between the stored ones (`dispatch`). Machines the specification marks as target are in `target_machines`, behind the `target-models` feature."]
            #![allow(clippy::bool_comparison, clippy::nonminimal_bool, clippy::cmp_owned)]
            #(#modules)*
            #dispatch
        }
    }
}

/// The target machines backend: the machines whose realisation is `target`, which the shipped
/// code does not run. Written to `target_machines`, a module the generated crate compiles only
/// with its `target-models` feature, so that tests and differentials still step them.
pub struct TargetMachinesBackend;

impl Backend for TargetMachinesBackend {
    fn module(&self) -> &'static str {
        super::TARGET_MODULE
    }

    fn wanted(&self, spec: &Spec) -> bool {
        !spec.target_machines.is_empty()
    }

    fn check(&self, spec: &Spec) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut module = Namespace::new("module target_machines");
        module.claim(
            &mut out,
            crate::diagnostic::SPEC_ENTITY,
            "the routing table `dispatch`",
            "dispatch",
        );
        let target: Vec<&Behavior> = spec.target_machines.iter().collect();
        check_machines(spec, &target, &mut module, &mut out);
        let record = full_system(spec);
        routing_rows(spec, &record, &mut out);
        let stored: Vec<&Behavior> = target.iter().copied().filter(|b| !b.memory).collect();
        elapsed_external(spec, &stored, &mut out);
        out
    }

    fn emit(&self, spec: &Spec) -> TokenStream {
        let target: Vec<&Behavior> = spec.target_machines.iter().collect();
        let modules = machine_modules(spec, &target);
        let record = full_system(spec);
        let members: Vec<(&Behavior, TokenStream)> = record
            .iter()
            .map(|b| {
                if b.target {
                    (*b, quote! { super })
                } else {
                    (*b, quote! { crate::machines })
                }
            })
            .collect();
        let dispatch = dispatch_module(spec, &record, &members);
        quote! {
            #![doc = " The machines the specification marks as target: they model the enterprise target, not code that ships, so they are compiled only with the `target-models` feature, which tests and differentials enable."]
            #![doc = ""]
            #![doc = " `dispatch` is the routing table of the whole system, shipped machines (from `machines`) and target machines together, as Exegra runs the specification."]
            #![allow(clippy::bool_comparison, clippy::nonminimal_bool, clippy::cmp_owned)]
            #(#modules)*
            #dispatch
        }
    }
}

/// Every stored machine of the specification, shipped and target, in declaration order: the
/// members of the whole-system routing table. Machines held in memory are not hosted.
fn full_system(spec: &Spec) -> Vec<&Behavior> {
    let mut all: Vec<&Behavior> = spec
        .machines
        .iter()
        .chain(spec.target_machines.iter().filter(|b| !b.memory))
        .collect();
    all.sort_by_key(|b| b.declared);
    all
}

/// The two lists merged in declaration order.
fn in_declaration_order<'a>(a: &'a [Behavior], b: &'a [Behavior]) -> Vec<&'a Behavior> {
    let mut all: Vec<&Behavior> = a.iter().chain(b.iter()).collect();
    all.sort_by_key(|m| m.declared);
    all
}

/// The registry macros of the generated `lib.rs`: `for_each_shipped_machine!(callback)` expands
/// to `callback! { machines::<module>::<Type>, ... }` over every stored shipped machine, and
/// `for_each_target_machine!(callback)` over every stored target machine. A caller that builds a
/// registry from them cannot miss a machine the specification declares. Memory machines are not
/// listed: they have no `strata_runtime::Machine` impl. The target macro is defined only with the
/// `target-models` feature, the feature that compiles `target_machines`.
pub fn registry_macros(spec: &Spec) -> TokenStream {
    let shipped = registry_paths(&spec.machines, "machines");
    let target = registry_paths(&spec.target_machines, "target_machines");
    let mut out = TokenStream::new();
    if !shipped.is_empty() {
        out.extend(quote! {
            #[doc = " Calls the macro `$callback` as `$callback! { machines::<module>::<Type>, ... }`, with every shipped stored implementation machine of the specification, in declaration order."]
            #[macro_export]
            macro_rules! for_each_shipped_machine {
                ($callback:ident) => {
                    $callback! {
                        #(#shipped,)*
                    }
                };
            }
        });
    }
    if !target.is_empty() {
        out.extend(quote! {
            #[cfg(feature = "target-models")]
            #[doc = " Calls the macro `$callback` as `$callback! { target_machines::<module>::<Type>, ... }`, with every target stored implementation machine of the specification, in declaration order. Expanding it needs the `target-models` feature."]
            #[macro_export]
            macro_rules! for_each_target_machine {
                ($callback:ident) => {
                    $callback! {
                        #(#target,)*
                    }
                };
            }
        });
    }
    out
}

/// The paths `<module>::<machine module>::<machine type>` of the stored machines in `machines`.
fn registry_paths(machines: &[Behavior], module: &str) -> Vec<TokenStream> {
    let root = naming::ident(module);
    machines
        .iter()
        .filter(|b| !b.memory)
        .map(|b| {
            let file = naming::ident(&naming::snake(&b.id).unwrap_or_default());
            let ty = naming::ident(&naming::pascal(&b.id).unwrap_or_default());
            quote! { #root::#file::#ty }
        })
        .collect()
}

/// The module-name and generation checks of the machines of one module.
fn check_machines(
    spec: &Spec,
    machines: &[&Behavior],
    module: &mut Namespace,
    out: &mut Vec<Diagnostic>,
) {
    for b in machines {
        let what = format!("machine \"{}\"", b.id);
        if let Some(name) = render::checked_name(out, &b.id, &what, naming::snake(&b.id)) {
            module.claim(out, &b.id, &what, &name);
        }
        let mut gen = Gen::new(spec, b);
        gen.machine();
        out.extend(gen.out);
    }
}

/// One inline module per machine.
fn machine_modules(spec: &Spec, machines: &[&Behavior]) -> Vec<TokenStream> {
    machines
        .iter()
        .map(|b| {
            let mut gen = Gen::new(spec, b);
            let body = gen.machine().unwrap_or_default();
            let name = naming::ident(&naming::snake(&b.id).unwrap_or_default());
            let doc = gen.module_doc().tokens();
            quote! {
                #doc
                pub mod #name {
                    #body
                }
            }
        })
        .collect()
}

/// Code of a scenario route the generated routing table cannot follow (any `by` but `key`).
pub const ROUTE_UNSUPPORTED: &str = "specc.route-unsupported";
/// Code of two scenario routes of one event to one machine that key it by different fields, or
/// a key route without a field or by a field the event does not carry.
pub const ROUTE_KEY: &str = "specc.route-key";

/// The machines that react to each event, with how each finds its instance: the routing table,
/// in event declaration order, machines in declaration order within an event. An event stands
/// for elapsed time only as a timer, which reaches its own instance and no route, so elapsed
/// events have no row.
fn routing_rows<'a>(
    spec: &'a Spec,
    machines: &[&'a Behavior],
    out: &mut Vec<Diagnostic>,
) -> Vec<(&'a Event, Vec<(&'a Behavior, Option<String>)>)> {
    let mut rows = Vec::new();
    for event in spec.events.iter().filter(|e| !e.elapsed) {
        let mut receivers = Vec::new();
        for b in machines.iter().copied() {
            if !b
                .transitions
                .iter()
                .any(|t| t.on == event.id && !t.fault_only)
            {
                continue;
            }
            let routes: Vec<_> = b.routes.iter().filter(|r| r.event == event.id).collect();
            let mut field: Option<String> = None;
            for r in &routes {
                if r.by != "key" {
                    out.push(Diagnostic::new(
                        ROUTE_UNSUPPORTED,
                        &r.scenario,
                        format!(
                            "Scenario \"{}\" routes \"{}\" to machine \"{}\" by \"{}\"; generated routing follows routes by key only.",
                            r.scenario, r.event, b.id, r.by
                        ),
                    ));
                    continue;
                }
                let Some(f) = &r.field else {
                    out.push(Diagnostic::new(
                        ROUTE_KEY,
                        &r.scenario,
                        format!(
                            "Scenario \"{}\" routes \"{}\" to machine \"{}\" by key without a \"field\".",
                            r.scenario, r.event, b.id
                        ),
                    ));
                    continue;
                };
                if !event.payload.iter().any(|p| p.name == *f) {
                    out.push(Diagnostic::new(
                        ROUTE_KEY,
                        &r.scenario,
                        format!(
                            "Scenario \"{}\" routes \"{}\" to machine \"{}\" by field \"{f}\", which is not a payload field of the event.",
                            r.scenario, r.event, b.id
                        ),
                    ));
                    continue;
                }
                match &field {
                    Some(other) if other != f => out.push(Diagnostic::new(
                        ROUTE_KEY,
                        &r.scenario,
                        format!(
                            "Scenario \"{}\" routes \"{}\" to machine \"{}\" by field \"{f}\", another scenario by \"{other}\"; one event reaches one machine by one key.",
                            r.scenario, r.event, b.id
                        ),
                    )),
                    _ => field = Some(f.clone()),
                }
            }
            let wire = field.map(|f| crate::wire::event_case(spec, event).name(&f));
            receivers.push((b, wire));
        }
        if !receivers.is_empty() {
            rows.push((event, receivers));
        }
    }
    rows
}

/// Refuse an elapsed event that a scenario injects into, or routes to, an implementation machine:
/// the generated hosts accept an elapsed event only from the timer that set it.
fn elapsed_external(spec: &Spec, machines: &[&Behavior], out: &mut Vec<Diagnostic>) {
    for b in machines.iter().copied() {
        let injected = b
            .injected
            .iter()
            .map(|i| (&i.event, &i.scenario, "injects"));
        let routed = b.routes.iter().map(|r| (&r.event, &r.scenario, "routes"));
        for (event, scenario, how) in injected.chain(routed) {
            if spec.event(event).is_some_and(|e| e.elapsed) {
                out.push(Diagnostic::new(
                    ELAPSED_EXTERNAL,
                    &b.id,
                    format!(
                        "Scenario \"{scenario}\" {how} \"{event}\", which stands for elapsed time (\"elapsed\": true), into machine \"{}\"; only a timer the machine sets may deliver it, so it cannot be an input from outside.",
                        b.id
                    ),
                ));
            }
        }
    }
}

/// The `dispatch` module: the members and routing table of the system host.
/// `paths` give each member and the path of the module that holds its type.
fn dispatch_module(
    spec: &Spec,
    machines: &[&Behavior],
    paths: &[(&Behavior, TokenStream)],
) -> TokenStream {
    let rows = routing_rows(spec, machines, &mut Vec::new());
    let members: Vec<TokenStream> = paths
        .iter()
        .map(|(b, path)| {
            let module = naming::ident(&naming::snake(&b.id).unwrap_or_default());
            let ty = naming::ident(&naming::pascal(&b.id).unwrap_or_default());
            quote! { strata_runtime::Member::of::<#path::#module::#ty>() }
        })
        .collect();
    let table: Vec<TokenStream> = rows
        .iter()
        .map(|(event, receivers)| {
            let id = &event.id;
            let routings: Vec<TokenStream> = receivers
                .iter()
                .map(|(b, field)| {
                    let name = &b.id;
                    match field {
                        Some(f) => quote! { strata_runtime::Routing::by_key(#name, #f) },
                        None => quote! { strata_runtime::Routing::every(#name) },
                    }
                })
                .collect();
            quote! { (#id, &[#(#routings),*]) }
        })
        .collect();
    quote! {
        /// How events travel between the machines, as Exegra delivers them: for every event,
        /// each machine with a transition on it and how the event finds its instance there
        /// (Exegra's route `by: key`, from the scenarios' `routes`, or every instance). An
        /// emit's `to` names its receiver outside the machines and does not route it.
        pub mod dispatch {
            /// For each event id, the machines that receive it.
            pub const ROUTES: strata_runtime::RouteTable = &[#(#table),*];

            /// Every machine, in declaration order.
            pub fn members() -> Vec<strata_runtime::Member> {
                vec![#(#members),*]
            }

            /// The router of the system host: every machine and the routing table.
            pub fn router() -> strata_runtime::Router {
                strata_runtime::Router::new(members(), ROUTES)
            }
        }
    }
}

/// How an expression sees a value of a type, and how a value is written back.
#[derive(Debug, Clone)]
struct View {
    /// The type expressions see.
    ty: Ty,
    /// How many `.0` steps reach that representation.
    unwrap: usize,
    /// The constructors that rebuild the stored type from the representation, outermost first.
    wraps: Vec<TokenStream>,
}

impl View {
    fn plain(ty: Ty) -> Self {
        View {
            ty,
            unwrap: 0,
            wraps: Vec::new(),
        }
    }

    fn read(&self, base: TokenStream) -> TokenStream {
        let mut t = base;
        for _ in 0..self.unwrap {
            let zero = Literal::usize_unsuffixed(0);
            t = quote! { #t.#zero };
        }
        t
    }

    fn wrap(&self, value: TokenStream) -> TokenStream {
        let mut t = value;
        for w in self.wraps.iter().rev() {
            t = quote! { #w(#t) };
        }
        t
    }
}

/// True when values of `ty` are `Copy` in the generated code.
fn ty_is_copy(ty: &Ty) -> bool {
    match ty {
        Ty::String | Ty::Bytes | Ty::Named(_) | Ty::List(_) => false,
        Ty::Option(inner) => ty_is_copy(inner),
        _ => true,
    }
}

fn builtin_ty(b: Builtin) -> Ty {
    match b {
        Builtin::Bool => Ty::Bool,
        Builtin::U8 => Ty::U8,
        Builtin::U16 => Ty::U16,
        Builtin::U32 => Ty::U32,
        Builtin::U64 => Ty::U64,
        Builtin::I32 => Ty::I32,
        Builtin::I64 => Ty::I64,
        Builtin::F64 => Ty::F64,
        Builtin::String => Ty::String,
        Builtin::Bytes => Ty::Bytes,
        Builtin::Timestamp => Ty::Timestamp,
        Builtin::Duration => Ty::Duration,
        Builtin::Uuid => Ty::Uuid,
        Builtin::Json => Ty::Json,
    }
}

/// The expression type of a type nested in an option or list, where nothing is unwrapped.
fn raw_ty(t: &TypeRef) -> Ty {
    match t {
        TypeRef::Builtin(b) => builtin_ty(*b),
        TypeRef::Declared(id) => Ty::Named(id.clone()),
        TypeRef::Option(inner) => Ty::Option(Box::new(raw_ty(inner))),
        TypeRef::List(inner) | TypeRef::Set(inner) => Ty::List(Box::new(raw_ty(inner))),
        // A map is opaque to expressions, like json: compared only with == and !=.
        TypeRef::Map(..) => Ty::Named(t.text()),
        // So is a shared value, whose Rust type is not its inner type.
        TypeRef::Shared(..) => Ty::Named(t.text()),
    }
}

/// How expressions see a value of type `t`. Assumes `t` resolves.
fn view(spec: &Spec, t: &TypeRef, uses: &mut Uses) -> View {
    match t {
        TypeRef::Builtin(Builtin::Timestamp) => {
            uses.prelude = true;
            View {
                ty: Ty::I64,
                unwrap: 1,
                wraps: vec![quote! { prelude::Timestamp }],
            }
        }
        TypeRef::Builtin(Builtin::Duration) => {
            uses.prelude = true;
            View {
                ty: Ty::U64,
                unwrap: 1,
                wraps: vec![quote! { prelude::DurationMs }],
            }
        }
        TypeRef::Builtin(b) => View::plain(builtin_ty(*b)),
        TypeRef::Declared(id) => {
            let repr = spec.type_decl(id).and_then(|d| match &d.kind {
                TypeKind::Id { repr } | TypeKind::Scalar { repr } => {
                    repr.as_deref().and_then(Builtin::parse)
                }
                _ => None,
            });
            match repr {
                Some(b) => {
                    let inner = view(spec, &TypeRef::Builtin(b), uses);
                    let name = render::rust_type(spec, t, TypeScope::Other, uses);
                    let mut wraps = vec![name];
                    wraps.extend(inner.wraps);
                    View {
                        ty: inner.ty,
                        unwrap: inner.unwrap + 1,
                        wraps,
                    }
                }
                None => View::plain(Ty::Named(id.clone())),
            }
        }
        TypeRef::Option(_)
        | TypeRef::List(_)
        | TypeRef::Set(_)
        | TypeRef::Map(..)
        | TypeRef::Shared(_) => View::plain(raw_ty(t)),
    }
}

/// A named value an expression can read: a variable or a payload field.
#[derive(Debug, Clone)]
struct Slot {
    /// The name in the specification.
    name: String,
    /// The Rust field.
    field: Ident,
    /// Its type name.
    type_name: String,
    /// True when the field is `Option<T>` because it may be left out (payload fields only).
    optional: bool,
    /// How expressions see it.
    view: View,
}

/// The scope guards and values are translated in. The flags record what the translation read,
/// so unused parameters and helpers are left out of the generated code.
struct MachineScope {
    vars: Vec<Slot>,
    /// `vars` in guards, `next` in the effects of a transition that sets variables.
    base: Ident,
    payload: Vec<Slot>,
    transition: String,
    used_time: Rc<Cell<bool>>,
    used_payload: Rc<Cell<bool>>,
    used_overflow: Rc<Cell<bool>>,
}

impl expr::Scope for MachineScope {
    fn resolve(&self, path: &[&str]) -> Option<Binding> {
        match path {
            ["time"] => {
                self.used_time.set(true);
                Some(Binding {
                    ty: Ty::I64,
                    tokens: quote! { now.0 },
                })
            }
            [name] => {
                let slot = self.vars.iter().find(|s| s.name == *name)?;
                let (base, field) = (&self.base, &slot.field);
                Some(Binding {
                    ty: slot.view.ty.clone(),
                    tokens: slot.view.read(quote! { #base.#field }),
                })
            }
            ["event", name] => {
                let slot = self.payload.iter().find(|s| s.name == *name)?;
                self.used_payload.set(true);
                let field = &slot.field;
                if slot.optional {
                    // An optional field is read as the option it is.
                    let ty = Ty::Option(Box::new(raw_ty(&TypeRef::parse(&slot.type_name))));
                    return Some(Binding {
                        ty,
                        tokens: quote! { payload.#field },
                    });
                }
                Some(Binding {
                    ty: slot.view.ty.clone(),
                    tokens: slot.view.read(quote! { payload.#field }),
                })
            }
            _ => None,
        }
    }

    fn overflow_error(&self) -> TokenStream {
        self.used_overflow.set(true);
        let t = &self.transition;
        quote! { overflow(#t) }
    }
}

/// The decoder that bounds a `u64` or `i64` payload field, and the attribute it needs beside it (a
/// field of type `option<T>` that is not marked optional still defaults to `None` when absent).
fn safe_decoder(t: &TypeRef, optional: bool) -> Option<(TokenStream, &'static str)> {
    let (inner, opt_type) = match t {
        TypeRef::Option(inner) => (inner.as_ref(), true),
        other => (other, false),
    };
    let unsigned = match inner {
        TypeRef::Builtin(Builtin::U64) => true,
        TypeRef::Builtin(Builtin::I64) => false,
        _ => return None,
    };
    let opt = optional || opt_type;
    let decoder = match (unsigned, opt) {
        (true, false) => "prelude::safe_u64",
        (false, false) => "prelude::safe_i64",
        (true, true) => "prelude::safe_opt_u64",
        (false, true) => "prelude::safe_opt_i64",
    };
    let attr = if opt_type && !optional {
        quote! { #[serde(default)] }
    } else {
        TokenStream::new()
    };
    Some((attr, decoder))
}

/// The machine's time unit, for a refusal: the unit, or the units its scenarios disagree on.
fn unit_text(b: &Behavior) -> String {
    match b.time_unit() {
        Ok(u) => format!("scenario time unit {u}"),
        Err(many) => format!(
            "its scenarios disagree on the time unit: {}",
            many.join(", ")
        ),
    }
}

/// Every dotted reference in an expression.
fn references(e: &expr::Expr, out: &mut Vec<Vec<String>>) {
    match &e.kind {
        ExprKind::Lit(_) => {}
        ExprKind::Ref(path) => out.push(path.clone()),
        ExprKind::Un(_, a) => references(a, out),
        ExprKind::Bin(_, a, b) => {
            references(a, out);
            references(b, out);
        }
    }
}

/// One event the machine receives or emits, with its Rust names.
#[derive(Clone)]
struct EventInfo<'a> {
    event: &'a Event,
    variant: Ident,
    payload_struct: Ident,
    fields: Vec<Slot>,
}

/// The generation of one machine: diagnostics while it goes, tokens when everything holds.
struct Gen<'a> {
    spec: &'a Spec,
    b: &'a Behavior,
    out: Vec<Diagnostic>,
    uses: Uses,
    names: Namespace,
    received: Vec<EventInfo<'a>>,
    emitted: Vec<EventInfo<'a>>,
    vars: Vec<Slot>,
    machine: Ident,
    used_time: Rc<Cell<bool>>,
    used_overflow: Rc<Cell<bool>>,
    has_emits: bool,
    has_timers: bool,
}

impl<'a> Gen<'a> {
    fn new(spec: &'a Spec, b: &'a Behavior) -> Self {
        let mut names = Namespace::new(format!("the module of machine \"{}\"", b.id));
        let mut ignored = Vec::new();
        for fixed in [
            "State",
            "Vars",
            "Event",
            "Emitted",
            "Key",
            "Delivery",
            "Duration",
            "Isolation",
            "Machine",
            "Outcome",
            "RecordError",
            "StepError",
            "Time",
        ] {
            names.claim(&mut ignored, &b.id, &format!("`{fixed}`"), fixed);
        }
        Gen {
            spec,
            b,
            out: Vec::new(),
            uses: Uses::default(),
            names,
            received: Vec::new(),
            emitted: Vec::new(),
            vars: Vec::new(),
            machine: format_ident!("Machine"),
            used_time: Rc::default(),
            used_overflow: Rc::default(),
            has_emits: false,
            has_timers: false,
        }
    }

    fn refuse(&mut self, code: &str, message: String) {
        self.out.push(Diagnostic::new(code, &self.b.id, message));
    }

    fn module_doc(&self) -> Doc {
        let b = self.b;
        let mut doc = Doc::new(&b.name);
        if let Some(d) = &b.description {
            doc.paragraph(d);
        }
        doc.paragraph(&format!(
            "Specification behavior `{}`, an implementation-level machine.",
            b.id
        ));
        doc
    }

    /// The module body, or `None` with diagnostics recorded.
    fn machine(&mut self) -> Option<TokenStream> {
        let b = self.b;
        let errors_before = self.out.len();

        // States.
        if b.states.is_empty() {
            self.refuse(
                STATE_UNKNOWN,
                format!("Machine \"{}\" has no states.", b.id),
            );
            return None;
        }
        let mut variants = Namespace::new(format!("the states of machine \"{}\"", b.id));
        let mut state_variants = Vec::new();
        for s in &b.states {
            let what = format!("state \"{}\"", s.id);
            let name = render::checked_name(&mut self.out, &b.id, &what, naming::pascal(&s.id));
            if let Some(n) = &name {
                variants.claim(&mut self.out, &b.id, &what, n);
            }
            state_variants.push(naming::ident(&name.unwrap_or_default()));
        }
        let what = format!("machine \"{}\"", b.id);
        if let Some(name) = render::checked_name(&mut self.out, &b.id, &what, naming::pascal(&b.id))
        {
            self.names.claim_type(&mut self.out, &b.id, &what, &name);
            self.machine = naming::ident(&name);
        }
        let state_of = |id: &str| b.states.iter().position(|s| s.id == id);
        let initial = match &b.initial {
            Some(i) if state_of(i).is_some() => state_of(i),
            Some(i) => {
                self.refuse(
                    STATE_UNKNOWN,
                    format!(
                        "The initial state \"{i}\" of machine \"{}\" is not one of its states.",
                        b.id
                    ),
                );
                None
            }
            None => {
                self.refuse(
                    STATE_UNKNOWN,
                    format!("Machine \"{}\" names no initial state.", b.id),
                );
                None
            }
        };

        // Isolation.
        let isolation = match b.isolation.as_deref() {
            // A memory machine has no transaction.
            _ if b.memory => Some(TokenStream::new()),
            Some("serializable") => Some(quote! { Isolation::Serializable }),
            Some("snapshot") => Some(quote! { Isolation::Snapshot }),
            Some("read-committed") => Some(quote! { Isolation::ReadCommitted }),
            Some(other) => {
                self.refuse(
                    "ready.transaction-missing",
                    format!("The transaction isolation \"{other}\" of machine \"{}\" is not serializable, snapshot or read-committed.", b.id),
                );
                None
            }
            None => {
                self.refuse(
                    "ready.transaction-missing",
                    format!("Machine \"{}\" declares no transaction: {{\"scope\": \"transition\", \"isolation\": ...}}.", b.id),
                );
                None
            }
        };

        // Variables.
        let mut fields = Namespace::new(format!("the variables of machine \"{}\"", b.id));
        for v in &b.variables {
            let what = format!("variable \"{}\"", v.name);
            let Some(repr) = &v.repr else {
                self.refuse(
                    "ready.repr-missing",
                    format!(
                        "Variable \"{}\" of machine \"{}\" has no repr.",
                        v.name, b.id
                    ),
                );
                continue;
            };
            if !resolve::resolves(self.spec, repr) {
                self.refuse(
                    "ready.repr-unresolved",
                    format!(
                        "Variable \"{}\" of machine \"{}\" has {}.",
                        v.name,
                        b.id,
                        resolve::unresolved_text(Some(repr))
                    ),
                );
                continue;
            }
            let Some(name) =
                render::checked_name(&mut self.out, &b.id, &what, naming::snake(&v.name))
            else {
                continue;
            };
            fields.claim(&mut self.out, &b.id, &what, &name);
            let t = TypeRef::parse(repr);
            let view = view(self.spec, &t, &mut self.uses);
            self.vars.push(Slot {
                name: v.name.clone(),
                field: naming::ident(&name),
                type_name: repr.clone(),
                optional: false,
                view,
            });
        }

        // Events: what the machine receives and emits, in the order of the events section.
        let mut received_ids = BTreeSet::new();
        let mut emitted_ids = BTreeSet::new();
        for t in &b.transitions {
            if t.fault_only {
                self.refuse(
                    "ready.fault-only",
                    format!("Transition \"{}\" of machine \"{}\" is faultOnly; production code has no faults to enable it.", t.id, b.id),
                );
            }
            received_ids.insert(t.on.as_str());
            for e in &t.effects {
                match e {
                    Effect::Emit { event, .. } => {
                        emitted_ids.insert(event.as_str());
                    }
                    Effect::Timer { event, .. } => {
                        received_ids.insert(event.as_str());
                    }
                    _ => {}
                }
            }
        }
        for s in &b.states {
            for e in &s.ignores {
                received_ids.insert(e.as_str());
            }
        }
        for id in received_ids.iter().chain(emitted_ids.iter()) {
            if self.spec.event(id).is_none() {
                self.refuse(
                    EVENT_UNKNOWN,
                    format!(
                        "Machine \"{}\" names event \"{id}\", which is not declared.",
                        b.id
                    ),
                );
            }
        }
        let mut event_variants = Namespace::new(format!("the events of machine \"{}\"", b.id));
        let mut emitted_variants =
            Namespace::new(format!("the emitted events of machine \"{}\"", b.id));
        let mut payload_structs = BTreeSet::new();
        let spec = self.spec;
        for e in &spec.events {
            let receives = received_ids.contains(e.id.as_str());
            let emits = emitted_ids.contains(e.id.as_str());
            if !receives && !emits {
                continue;
            }
            let Some(info) = self.event_info(e, &mut payload_structs) else {
                continue;
            };
            let what = format!("event \"{}\"", e.id);
            if emits {
                if !matches!(
                    e.delivery.as_deref(),
                    Some("at-most-once" | "at-least-once")
                ) {
                    self.refuse(
                        "ready.delivery-missing",
                        format!("Event \"{}\", which machine \"{}\" emits, declares no delivery (at-most-once or at-least-once).", e.id, b.id),
                    );
                }
                emitted_variants.claim(&mut self.out, &b.id, &what, &info.variant.to_string());
            }
            if receives {
                event_variants.claim(&mut self.out, &b.id, &what, &info.variant.to_string());
            }
            if receives && emits {
                self.received.push(info.clone());
                self.emitted.push(info);
            } else if receives {
                self.received.push(info);
            } else {
                self.emitted.push(info);
            }
        }
        if self.received.is_empty() && self.out.len() == errors_before {
            self.refuse(
                NO_EVENTS,
                format!(
                    "Machine \"{}\" receives no event, so it can never step.",
                    b.id
                ),
            );
        }

        // Storage.
        // A memory machine has no record: its state lives in memory, and nothing converts it.
        let storage: Option<Option<StorageCode>> = if b.memory {
            Some(None)
        } else {
            self.storage().map(Some)
        };

        // Transitions, grouped by (from, on) in the order the groups first appear per state.
        let mut arms = Vec::new();
        let mut covered = 0usize;
        for t in &b.transitions {
            for (state, what) in [(&t.from, "source"), (&t.to, "target")] {
                if state_of(state).is_none() {
                    self.refuse(
                        STATE_UNKNOWN,
                        format!("The {what} state \"{state}\" of transition \"{}\" is not a state of machine \"{}\".", t.id, b.id),
                    );
                }
            }
        }
        for (si, s) in b.states.iter().enumerate() {
            let mut seen: Vec<&str> = Vec::new();
            for t in &b.transitions {
                if t.from == s.id && !t.fault_only && !seen.contains(&t.on.as_str()) {
                    seen.push(&t.on);
                }
            }
            for on in seen {
                let group: Vec<&Transition> = b
                    .transitions
                    .iter()
                    .filter(|t| t.from == s.id && t.on == on && !t.fault_only)
                    .collect();
                if let Some(arm) = self.arm(&state_variants[si], on, &group, &state_variants) {
                    arms.push(arm);
                    covered += 1;
                }
            }
        }
        // `time` is the step's clock in milliseconds; the simulator's `time` counts the unit of
        // the machine's scenarios, so the two agree only in milliseconds.
        if self.used_time.get() && b.time_unit() != Ok("ms") {
            self.refuse(
                TIME_UNIT,
                format!(
                    "Machine \"{}\" reads time, but {}; generated code's time counts milliseconds, so its scenarios must use \"timeUnit\": \"ms\".",
                    b.id,
                    unit_text(b)
                ),
            );
        }
        if self.out.len() > errors_before {
            return None;
        }
        let (initial, isolation, storage) = (initial?, isolation?, storage?);
        Some(self.assemble(&state_variants, initial, isolation, storage, arms, covered))
    }

    fn event_info(
        &mut self,
        e: &'a Event,
        payload_structs: &mut BTreeSet<String>,
    ) -> Option<EventInfo<'a>> {
        let b = self.b;
        let what = format!("event \"{}\"", e.id);
        let variant = render::checked_name(&mut self.out, &b.id, &what, naming::pascal(&e.id))?;
        let payload_name = format!("{variant}Payload");
        if payload_structs.insert(payload_name.clone()) {
            self.names.claim_type(
                &mut self.out,
                &b.id,
                &format!("the payload of {what}"),
                &payload_name,
            );
        }
        let mut names = Namespace::new(format!("the payload of {what}"));
        let mut fields = Vec::new();
        for f in &e.payload {
            let fwhat = format!("payload field \"{}\" of {what}", f.name);
            let resolved = f
                .type_name
                .as_deref()
                .filter(|t| resolve::resolves(self.spec, t));
            let Some(type_name) = resolved else {
                self.refuse(
                    "ready.payload-type-unresolved",
                    format!(
                        "Payload field \"{}\" of event \"{}\", which machine \"{}\" uses, has {}.",
                        f.name,
                        e.id,
                        b.id,
                        resolve::unresolved_text(f.type_name.as_deref())
                    ),
                );
                continue;
            };
            let Some(name) =
                render::checked_name(&mut self.out, &b.id, &fwhat, naming::snake(&f.name))
            else {
                continue;
            };
            names.claim(&mut self.out, &b.id, &fwhat, &name);
            let view = view(self.spec, &TypeRef::parse(type_name), &mut self.uses);
            fields.push(Slot {
                name: f.name.clone(),
                field: naming::ident(&name),
                type_name: type_name.to_string(),
                optional: f.optional,
                view,
            });
        }
        Some(EventInfo {
            event: e,
            variant: naming::ident(&variant),
            payload_struct: naming::ident(&payload_name),
            fields,
        })
    }

    /// The storage record, its Rust type, the key's Rust type and the record conversions.
    fn storage(&mut self) -> Option<StorageCode> {
        let b = self.b;
        if let Some(i) = &b.instances {
            if i.counted {
                self.refuse(
                    INSTANCES_COUNTED,
                    format!("Machine \"{}\" has counted instances (instances.count); a stored machine is keyed by a record field.", b.id),
                );
                return None;
            }
        }
        let from_storage = b
            .storage
            .as_ref()
            .and_then(|s| Some((s.record.clone()?, s.key.clone()?)));
        let from_instances = b
            .instances
            .as_ref()
            .and_then(|i| Some((i.stored_in.clone()?, i.key.clone()?)));
        let (record_id, key) = match (from_storage, from_instances) {
            (Some(a), Some(b2)) if a != b2 => {
                self.refuse(
                    "ready.storage-mismatch",
                    format!(
                        "Machine \"{}\" is stored in record \"{}\" by \"{}\" according to storage, but in \"{}\" by \"{}\" according to instances.",
                        b.id, a.0, a.1.join(", "), b2.0, b2.1.join(", ")
                    ),
                );
                return None;
            }
            (Some(a), _) | (None, Some(a)) => a,
            (None, None) => {
                self.refuse(
                    STORAGE,
                    format!("Machine \"{}\" names no record holding it: storage {{record, key}} or instances {{key, storedIn}}.", b.id),
                );
                return None;
            }
        };
        let spec = self.spec;
        let Some(record) = spec.record(&record_id) else {
            self.refuse(
                STORAGE,
                format!(
                    "Machine \"{}\" is stored in \"{record_id}\", which is not a record.",
                    b.id
                ),
            );
            return None;
        };
        // Every key field is a field of the record, and together they are the record's whole key,
        // in its order: a machine keyed by part of the key would be one instance for every record
        // that agrees on that part (Exegra's rule `storage.key-whole`, WG-4907).
        let mut key_fields = Vec::new();
        for k in &key {
            let Some(f) = record.field(k) else {
                self.refuse(
                    STORAGE,
                    format!(
                        "The key \"{k}\" of machine \"{}\" is not a field of record \"{record_id}\".",
                        b.id
                    ),
                );
                return None;
            };
            key_fields.push(f);
        }
        let whole = record.key.clone().unwrap_or_default();
        if key != whole {
            self.refuse(
                KEY_WHOLE,
                format!(
                    "The key {key:?} of machine \"{}\" is not the whole key {whole:?} of record \"{record_id}\"; a machine is keyed by every key field of its record, in order, or records that agree on the named fields would share one instance.",
                    b.id
                ),
            );
            return None;
        }
        // The key's Rust type: the field's, or a tuple of the fields' for a composite key.
        let key_types: Vec<(TokenStream, bool)> = key_fields
            .iter()
            .map(|f| {
                let r = TypeRef::parse(&f.type_name);
                let mut ty = render::rust_type(spec, &r, TypeScope::Other, &mut self.uses);
                if f.optional {
                    ty = quote! { Option<#ty> };
                }
                (ty, resolve::is_copy(spec, &r) && !f.optional)
            })
            .collect();
        let key_ty = if let [(ty, _)] = key_types.as_slice() {
            ty.clone()
        } else {
            let tys = key_types.iter().map(|(t, _)| t);
            quote! { (#(#tys),*) }
        };

        let st = b.storage.clone();
        let explicit: Vec<(String, Option<String>)> =
            st.as_ref().map(|s| s.fields.clone()).unwrap_or_default();
        let state_explicit = st.as_ref().and_then(|s| s.state.clone());
        let state_values = st.as_ref().and_then(|s| s.state_values.clone());
        let creates = st.as_ref().is_some_and(|s| s.creates);

        // What each record field holds: the key, a variable, the state.
        let mut owned: Vec<(String, String)> = key
            .iter()
            .map(|k| (k.clone(), "the key".to_string()))
            .collect();
        for (v, f) in &explicit {
            if !b.variables.iter().any(|x| x.name == *v) {
                self.refuse(
                    FIELD_UNKNOWN,
                    format!(
                        "storage.fields of \"{}\" maps \"{v}\", which is not one of its variables.",
                        b.id
                    ),
                );
            } else if f.as_deref().is_none_or(|f| record.field(f).is_none()) {
                self.refuse(
                    FIELD_UNKNOWN,
                    format!(
                        "storage.fields of \"{}\" maps variable \"{v}\" to {}, which is not a field of record \"{record_id}\".",
                        b.id,
                        f.as_deref().map_or("a non-string".to_string(), |f| format!("\"{f}\""))
                    ),
                );
            }
        }
        // The variables and the record fields holding them.
        let mut held: Vec<(Slot, String)> = Vec::new();
        for v in &b.variables {
            let mapped = explicit.iter().find(|(name, _)| *name == v.name);
            let f = match mapped {
                Some((_, Some(f))) if record.field(f).is_some() => f.clone(),
                Some(_) => continue, // reported above
                None => v.name.clone(),
            };
            if record.field(&f).is_none() {
                self.refuse(
                    VARIABLE_UNMAPPED,
                    format!(
                        "Variable \"{}\" of \"{}\" is held in no field of record \"{record_id}\": the record has no field \"{}\" and storage.fields does not map the variable.",
                        v.name, b.id, v.name
                    ),
                );
                continue;
            }
            if let Some((_, taken)) = owned.iter().find(|(name, _)| *name == f) {
                let taken = taken.clone();
                self.refuse(
                    FIELD_UNKNOWN,
                    format!(
                        "Field \"{f}\" of record \"{record_id}\" already holds {taken}, so variable \"{}\" of \"{}\" cannot be held there too.",
                        v.name, b.id
                    ),
                );
                continue;
            }
            owned.push((f.clone(), format!("variable \"{}\"", v.name)));
            let Some(slot) = self.vars.iter().find(|s| s.name == v.name).cloned() else {
                continue; // its repr is refused already
            };
            let field = record.field(&f).expect("checked above");
            if fit(spec, &slot.type_name, field).is_none() {
                let (optional, base) = field_base(field);
                self.refuse(
                    FIELD_TYPE,
                    format!(
                        "Variable \"{}\" of \"{}\" has repr \"{}\", but field \"{f}\" of record \"{record_id}\" holds {}\"{base}\".",
                        v.name,
                        b.id,
                        slot.type_name,
                        if optional { "an optional " } else { "" },
                    ),
                );
                continue;
            }
            held.push((slot, f));
        }

        // The state: stored when the machine says where, or has more than one state, or the
        // record has a free "state" field.
        let state_field = state_explicit
            .clone()
            .unwrap_or_else(|| "state".to_string());
        let stores_state = state_explicit.is_some()
            || state_values.is_some()
            || b.states.len() > 1
            || (record.field("state").is_some() && !owned.iter().any(|(f, _)| f == "state"));
        let state_code = if stores_state {
            match state_storage(
                spec,
                b,
                record,
                &state_field,
                state_values.as_deref(),
                &owned,
                &mut self.uses,
            ) {
                Ok(code) => code,
                Err(message) => {
                    self.refuse(STATE_FIELD, message);
                    StateCode::Unstored
                }
            }
        } else {
            StateCode::Unstored
        };

        // A creating machine writes whole records; a non-creating one carries over the rest.
        if creates {
            for f in &record.fields {
                let holds_state = stores_state && f.name == state_field;
                if field_base(f).0 || owned.iter().any(|(o, _)| *o == f.name) || holds_state {
                    continue;
                }
                self.refuse(
                    CREATES_UNFILLED,
                    format!(
                        "\"{}\" creates records of \"{record_id}\" (storage.creates), but required field \"{}\" is neither its key, its state nor one of its variables, so a new record would leave it unset.",
                        b.id, f.name
                    ),
                );
            }
        }

        let mut to_fields = Vec::new();
        let mut uses_previous = false;
        let mut uses_state = false;
        for f in &record.fields {
            let field = naming::ident(&naming::snake(&f.name).unwrap_or_default());
            let field_copy = resolve::is_copy(spec, &TypeRef::parse(&f.type_name));
            let value = if let Some(at) = key.iter().position(|k| *k == f.name) {
                let copy = key_types[at].1;
                if key.len() == 1 {
                    if copy {
                        quote! { *key }
                    } else {
                        quote! { key.clone() }
                    }
                } else {
                    let at = proc_macro2::Literal::usize_unsuffixed(at);
                    if copy {
                        quote! { key.#at }
                    } else {
                        quote! { key.#at.clone() }
                    }
                }
            } else if f.name == state_field && !matches!(state_code, StateCode::Unstored) {
                uses_state = true;
                state_code.write()
            } else if let Some((slot, _)) = held.iter().find(|(_, hf)| *hf == f.name) {
                self.write_field(slot, f)
            } else {
                uses_previous = true;
                match (creates, field_copy) {
                    (false, true) => quote! { previous.#field },
                    (false, false) => quote! { previous.#field.clone() },
                    (true, true) => quote! { previous.and_then(|p| p.#field) },
                    (true, false) => quote! { previous.and_then(|p| p.#field.clone()) },
                }
            };
            to_fields.push(quote! { #field: #value });
        }
        let mut from_fields = Vec::new();
        for (slot, f) in &held {
            let field = record.field(f).expect("checked above");
            let read = self.read_field(slot, field);
            let vf = &slot.field;
            from_fields.push(quote! { #vf: #read });
        }
        let only = naming::ident(
            &b.states
                .first()
                .and_then(|s| naming::pascal(&s.id).ok())
                .unwrap_or_default(),
        );
        let read_state = state_code.read(
            &naming::ident(&naming::snake(&state_field).unwrap_or_default()),
            &b.id,
            &only,
        );
        Some(StorageCode {
            record: record.clone(),
            record_ty: naming::ident(&naming::pascal(&record.id).unwrap_or_default()),
            key_field: key.join("`, `"),
            key_ty,
            read_state,
            reads_record: !held.is_empty() || !matches!(state_code, StateCode::Unstored),
            creates,
            uses_state,
            uses_previous,
            to_fields,
            from_fields,
        })
    }

    /// The value of record field `f` holding the variable of `slot`, in `to_record`.
    fn write_field(&mut self, slot: &Slot, f: &RecordField) -> TokenStream {
        let vf = &slot.field;
        let value = if resolve::is_copy(self.spec, &TypeRef::parse(&slot.type_name)) {
            quote! { vars.#vf }
        } else {
            quote! { vars.#vf.clone() }
        };
        match fit(self.spec, &slot.type_name, f) {
            Some(Fit::Option) | None => value,
            Some(Fit::Same { optional }) => wrap_some(value, optional),
            Some(Fit::Declared { optional, base }) => {
                let ty = render::rust_type(
                    self.spec,
                    &TypeRef::Declared(base),
                    TypeScope::Other,
                    &mut self.uses,
                );
                wrap_some(quote! { #ty(#value) }, optional)
            }
        }
    }

    /// The value of the variable of `slot` read from record field `f`, in `from_record`. An
    /// unset optional field gives the variable its initial value.
    fn read_field(&mut self, slot: &Slot, f: &RecordField) -> TokenStream {
        let rf = naming::ident(&naming::snake(&f.name).unwrap_or_default());
        let copy = resolve::is_copy(self.spec, &TypeRef::parse(&slot.type_name));
        let (optional, declared) = match fit(self.spec, &slot.type_name, f) {
            Some(Fit::Option) | None => {
                return if copy {
                    quote! { record.#rf }
                } else {
                    quote! { record.#rf.clone() }
                };
            }
            Some(Fit::Same { optional }) => (optional, false),
            Some(Fit::Declared { optional, .. }) => (optional, true),
        };
        let inner = |base: TokenStream| {
            let base = if declared {
                quote! { #base.0 }
            } else {
                base
            };
            if copy {
                base
            } else {
                quote! { #base.clone() }
            }
        };
        if !optional {
            return inner(quote! { record.#rf });
        }
        let set = if declared || !copy {
            inner(quote! { v })
        } else {
            quote! { *v }
        };
        let initial = self
            .b
            .variables
            .iter()
            .find(|v| v.name == slot.name)
            .and_then(|v| v.initial.clone());
        // An initial value that cannot be written is refused when the variables are assembled.
        let unset = initial
            .and_then(|i| self.literal(&i, &TypeRef::parse(&slot.type_name)).ok())
            .unwrap_or_else(|| quote! { Default::default() });
        quote! {
            match &record.#rf {
                Some(v) => #set,
                None => #unset,
            }
        }
    }

    /// The scope of one transition: its guard reads `base` = `vars`, its effects `next` when
    /// it sets variables.
    fn scope(
        &self,
        payload: &[Slot],
        transition: &str,
        base: &str,
        used_payload: &Rc<Cell<bool>>,
    ) -> MachineScope {
        MachineScope {
            vars: self.vars.clone(),
            base: format_ident!("{}", base),
            payload: payload.to_vec(),
            transition: transition.to_string(),
            used_time: self.used_time.clone(),
            used_payload: used_payload.clone(),
            used_overflow: self.used_overflow.clone(),
        }
    }

    /// Parses an expression and refuses references the runtime cannot serve.
    fn parse(&mut self, src: &str, what: &str) -> Option<expr::Expr> {
        let b = self.b;
        let parsed = match expr::parse(src) {
            Ok(e) => e,
            Err(e) => {
                self.refuse(
                    EXPRESSION,
                    format!(
                        "{what} of machine \"{}\" (`{src}`) does not parse: {}.",
                        b.id, e.message
                    ),
                );
                return None;
            }
        };
        let mut refs = Vec::new();
        references(&parsed, &mut refs);
        let mut ok = true;
        for r in refs {
            if r.len() >= 2 && r[0] != "event" {
                self.refuse(
                    CROSS_BEHAVIOUR_READ,
                    format!(
                        "{what} of machine \"{}\" reads `{}` in `{src}`: another behavior's variable, an aggregate or a resource, which one machine's step cannot read.",
                        b.id,
                        r.join(".")
                    ),
                );
                ok = false;
            }
        }
        ok.then_some(parsed)
    }

    /// An expression value of a slot's type, ready to store: cloned when it is a non-`Copy`
    /// place, and rebuilt through the slot's constructors.
    fn value(
        &mut self,
        src: &str,
        target: &View,
        scope: &MachineScope,
        what: &str,
    ) -> Option<TokenStream> {
        let parsed = self.parse(src, what)?;
        match expr::to_rust_as(&parsed, scope, &target.ty) {
            Ok(tokens) => {
                let tokens = if matches!(parsed.kind, ExprKind::Ref(_)) && !ty_is_copy(&target.ty) {
                    quote! { #tokens.clone() }
                } else {
                    tokens
                };
                Some(target.wrap(tokens))
            }
            Err(e) => {
                self.refuse(
                    EXPRESSION,
                    format!(
                        "{what} of machine \"{}\" (`{src}`) cannot be generated: {}.",
                        self.b.id, e.message
                    ),
                );
                None
            }
        }
    }

    /// A literal JSON value (an initial value, or a non-string `to` or payload entry) as a
    /// value of type `t`.
    fn literal(&mut self, value: &Value, t: &TypeRef) -> Result<TokenStream, String> {
        let spec = self.spec;
        match (t, value) {
            (TypeRef::Option(_), Value::Null) => Ok(quote! { None }),
            (TypeRef::Option(inner), v) => {
                let inner = self.literal(v, inner)?;
                Ok(quote! { Some(#inner) })
            }
            (TypeRef::Set(inner), Value::Array(items)) => {
                let items = items
                    .iter()
                    .map(|i| self.literal(i, inner))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(quote! { [#(#items),*].into_iter().collect::<std::collections::BTreeSet<_>>() })
            }
            (TypeRef::List(inner), Value::Array(items)) => {
                if items.is_empty() {
                    return Ok(quote! { Vec::new() });
                }
                let items = items
                    .iter()
                    .map(|i| self.literal(i, inner))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(quote! { vec![#(#items),*] })
            }
            (TypeRef::Builtin(Builtin::Bool), Value::Bool(v)) => Ok(quote! { #v }),
            (TypeRef::Builtin(Builtin::String), Value::String(s)) => {
                Ok(quote! { String::from(#s) })
            }
            (TypeRef::Builtin(_), Value::Number(n)) => {
                let v = view(spec, t, &mut self.uses);
                let text = n.to_string();
                let parsed = expr::parse(&text).map_err(|e| e.message)?;
                let tokens = expr::to_rust_as(&parsed, &NoScope, &v.ty).map_err(|e| e.message)?;
                Ok(v.wrap(tokens))
            }
            (TypeRef::Declared(id), v) => {
                let decl = spec
                    .type_decl(id)
                    .ok_or_else(|| format!("\"{id}\" is not declared"))?;
                let name = render::rust_type(spec, t, TypeScope::Other, &mut self.uses);
                match (&decl.kind, v) {
                    (TypeKind::Id { repr } | TypeKind::Scalar { repr }, v) => {
                        let repr = repr
                            .as_deref()
                            .and_then(Builtin::parse)
                            .ok_or_else(|| format!("\"{id}\" has no built-in repr"))?;
                        let inner = self.literal(v, &TypeRef::Builtin(repr))?;
                        Ok(quote! { #name(#inner) })
                    }
                    (TypeKind::Enum { values }, Value::String(s)) if values.contains(s) => {
                        let variant = naming::ident(&naming::pascal(s)?);
                        Ok(quote! { #name::#variant })
                    }
                    _ => Err(format!("{v} is not a literal of type \"{id}\"")),
                }
            }
            (_, v) => Err(format!("{v} is not a literal of this type")),
        }
    }

    /// The payload struct of an emit or timer, built from its payload entries.
    fn payload(
        &mut self,
        info_fields: &[Slot],
        payload_struct: &Ident,
        entries: &[(String, Value)],
        scope: &MachineScope,
        what: &str,
    ) -> Option<TokenStream> {
        let b = self.b;
        let mut ok = true;
        for (k, _) in entries {
            if !info_fields.iter().any(|f| f.name == *k) {
                self.refuse(
                    PAYLOAD_FIELD,
                    format!("{what} of machine \"{}\" gives payload field \"{k}\", which its event does not declare (or whose type does not resolve).", b.id),
                );
                ok = false;
            }
        }
        let mut fields = Vec::new();
        for f in info_fields {
            let field = &f.field;
            let entry = entries.iter().find(|(k, _)| *k == f.name).map(|(_, v)| v);
            let fwhat = format!("{what}, payload field \"{}\",", f.name);
            let t = TypeRef::parse(&f.type_name);
            // A field that is optional, or whose type is `option<T>`, left out is none (as in
            // Exegra, where a missing payload field is absent).
            let omittable = f.optional || matches!(t, TypeRef::Option(_));
            let value = match entry {
                None if omittable => Some(quote! { None }),
                None => {
                    self.refuse(
                        PAYLOAD_FIELD,
                        format!("{what} of machine \"{}\" gives no value for payload field \"{}\", which is required.", b.id, f.name),
                    );
                    None
                }
                Some(Value::Null) if f.optional => Some(quote! { None }),
                Some(Value::String(src)) => {
                    let v = self.value(src, &f.view, scope, &fwhat);
                    if f.optional {
                        v.map(|v| quote! { Some(#v) })
                    } else {
                        v
                    }
                }
                Some(literal) => match self.literal(literal, &t) {
                    Ok(v) if f.optional => Some(quote! { Some(#v) }),
                    Ok(v) => Some(v),
                    Err(reason) => {
                        self.refuse(
                            LITERAL,
                            format!(
                                "{fwhat} of machine \"{}\" cannot be written: {reason}.",
                                b.id
                            ),
                        );
                        None
                    }
                },
            };
            match value {
                Some(v) => fields.push(quote! { #field: #v }),
                None => ok = false,
            }
        }
        ok.then(|| quote! { #payload_struct { #(#fields),* } })
    }

    /// The match arm of one `(state, event)` group.
    fn arm(
        &mut self,
        state: &Ident,
        on: &str,
        group: &[&'a Transition],
        states: &[Ident],
    ) -> Option<TokenStream> {
        let info = self.received.iter().find(|e| e.event.id == on)?;
        let (variant, payload_fields) = (info.variant.clone(), info.fields.clone());
        let used_payload = Rc::new(Cell::new(false));
        let mut branches: Vec<(Option<TokenStream>, TokenStream)> = Vec::new();
        let mut unconditional = false;
        for t in group {
            let guard = match &t.guard {
                Some(g) => {
                    let what = format!("The guard of transition \"{}\"", t.id);
                    let scope = self.scope(&payload_fields, &t.id, "vars", &used_payload);
                    let parsed = self.parse(g, &what);
                    let tokens =
                        parsed.and_then(|p| match expr::to_rust_as(&p, &scope, &Ty::Bool) {
                            Ok(t) => Some(t),
                            Err(e) => {
                                self.out.push(Diagnostic::new(
                                    EXPRESSION,
                                    &self.b.id,
                                    format!(
                                        "{what} of machine \"{}\" (`{g}`) cannot be generated: {}.",
                                        self.b.id, e.message
                                    ),
                                ));
                                None
                            }
                        });
                    Some(tokens?)
                }
                None => None,
            };
            let body = self.body(t, &payload_fields, &used_payload, states)?;
            let last = guard.is_none();
            branches.push((guard, body));
            if last {
                // Declaration order: nothing after an unguarded transition is ever considered.
                unconditional = true;
                break;
            }
        }
        let ignore = quote! { Ok(Outcome::ignore(*state, vars.clone())) };
        let mut chain = if unconditional {
            branches.pop().map(|(_, body)| body).unwrap_or(ignore)
        } else {
            ignore
        };
        while let Some((guard, body)) = branches.pop() {
            let guard = guard.unwrap_or_else(|| quote! { true });
            chain = quote! {
                if #guard {
                    #body
                } else {
                    #chain
                }
            };
        }
        let payload = if used_payload.get() {
            quote! { payload }
        } else {
            quote! { _ }
        };
        Some(quote! {
            (State::#state, Event::#variant(#payload)) => {
                #chain
            }
        })
    }

    /// The outcome of one transition: its effects in order, then its target state.
    fn body(
        &mut self,
        t: &'a Transition,
        payload_fields: &[Slot],
        used_payload: &Rc<Cell<bool>>,
        states: &[Ident],
    ) -> Option<TokenStream> {
        let b = self.b;
        let target = &states[b.states.iter().position(|s| s.id == t.to)?];
        let sets = t
            .effects
            .iter()
            .filter(|e| matches!(e, Effect::Set { .. }))
            .count();
        let base = if sets > 0 { "next" } else { "vars" };
        let mut statements = Vec::new();
        let mut chain = Vec::new();
        let mut ok = true;
        for (i, e) in t.effects.iter().enumerate() {
            let what = format!("Effect {} of transition \"{}\"", i + 1, t.id);
            let scope = self.scope(payload_fields, &t.id, base, used_payload);
            match e {
                Effect::Set { variable, to } => {
                    let Some(slot) = self.vars.iter().find(|v| v.name == *variable).cloned() else {
                        self.refuse(
                            VARIABLE_UNKNOWN,
                            format!("{what} of machine \"{}\" sets \"{variable}\", which is not one of its variables.", b.id),
                        );
                        ok = false;
                        continue;
                    };
                    let value = match to {
                        Value::String(src) => self.value(src, &slot.view, &scope, &what),
                        literal => match self.literal(literal, &TypeRef::parse(&slot.type_name)) {
                            Ok(v) => Some(v),
                            Err(reason) => {
                                self.refuse(
                                    LITERAL,
                                    format!("{what} of machine \"{}\" sets \"{variable}\" to a value that cannot be written: {reason}.", b.id),
                                );
                                None
                            }
                        },
                    };
                    let field = &slot.field;
                    match value {
                        Some(v) => statements.push(quote! { next.#field = #v; }),
                        None => ok = false,
                    }
                }
                Effect::Emit {
                    event,
                    to,
                    target,
                    after,
                    payload,
                    link,
                } => {
                    if !link.is_empty() {
                        self.refuse(
                            EMIT_LINK,
                            format!("{what} of machine \"{}\" emits \"{event}\" over a link ({}); the runtime delivers through its outbox and has no links.", b.id, link.join(", ")),
                        );
                        ok = false;
                    }
                    // A bare number is in the machine's time unit (its scenarios'), converted to
                    // milliseconds (Exegra docs/semantics.md, "Time").
                    let delay = match after {
                        None | Some(Value::Null) => Some(0),
                        Some(v) => b.delay_ms(v),
                    };
                    let Some(delay) = delay else {
                        self.refuse(
                            EMIT_AFTER,
                            format!("{what} of machine \"{}\" emits \"{event}\" after a delay that is not a whole number of milliseconds in its time unit ({}); generated code delays an emit by a fixed number of milliseconds only.", b.id, unit_text(b)),
                        );
                        ok = false;
                        continue;
                    };
                    let Some(receiver) = to.clone() else {
                        let how = match target.as_deref() {
                            Some("reply") => "replies to the sender".to_string(),
                            Some(other) => format!("is addressed by target \"{other}\""),
                            None => "is addressed by scenario routes".to_string(),
                        };
                        self.refuse(
                            EMIT_UNADDRESSED,
                            format!("{what} of machine \"{}\" emits \"{event}\", which {how}; generated code addresses an emit only by its \"to\" (a component or contract id).", b.id),
                        );
                        ok = false;
                        continue;
                    };
                    let Some(info) = self.emitted.iter().find(|e| e.event.id == *event) else {
                        ok = false;
                        continue;
                    };
                    let (variant, ps, fields) = (
                        info.variant.clone(),
                        info.payload_struct.clone(),
                        info.fields.clone(),
                    );
                    let delivery = match info.event.delivery.as_deref() {
                        Some("at-least-once") => quote! { Delivery::AtLeastOnce },
                        _ => quote! { Delivery::AtMostOnce },
                    };
                    let Some(p) = self.payload(&fields, &ps, payload, &scope, &what) else {
                        ok = false;
                        continue;
                    };
                    self.has_emits = true;
                    let value = quote! { Emitted::#variant(#p) };
                    // `next` moves into the outcome, so a transition that sets variables builds
                    // each payload where it stands among the effects, before later sets.
                    let value = if sets > 0 {
                        let local = format_ident!("emitted_{}", i + 1);
                        statements.push(quote! { let #local = #value; });
                        quote! { #local }
                    } else {
                        value
                    };
                    if delay > 0 {
                        self.has_timers = true;
                        let millis = Literal::u64_unsuffixed(delay);
                        chain.push(quote! {
                            .emit_after(#receiver, #delivery, #value, Duration::from_millis(#millis))
                        });
                    } else {
                        chain.push(quote! { .emit(#receiver, #delivery, #value) });
                    }
                }
                Effect::Timer {
                    name,
                    after,
                    event,
                    payload,
                } => {
                    let millis = after.as_ref().and_then(|v| b.delay_ms(v));
                    let Some(millis) = millis else {
                        self.refuse(
                            TIMER_AFTER,
                            format!("{what} of machine \"{}\" sets timer \"{name}\" with an \"after\" that is not a whole number of milliseconds in its time unit ({}).", b.id, unit_text(b)),
                        );
                        ok = false;
                        continue;
                    };
                    let Some(info) = self.received.iter().find(|e| e.event.id == *event) else {
                        ok = false;
                        continue;
                    };
                    let (variant, ps, fields) = (
                        info.variant.clone(),
                        info.payload_struct.clone(),
                        info.fields.clone(),
                    );
                    let Some(p) = self.payload(&fields, &ps, payload, &scope, &what) else {
                        ok = false;
                        continue;
                    };
                    self.has_timers = true;
                    let value = quote! { Event::#variant(#p) };
                    let value = if sets > 0 {
                        let local = format_ident!("timer_{}", i + 1);
                        statements.push(quote! { let #local = #value; });
                        quote! { #local }
                    } else {
                        value
                    };
                    let millis = Literal::u64_unsuffixed(millis);
                    chain
                        .push(quote! { .set_timer(#name, Duration::from_millis(#millis), #value) });
                }
                Effect::Cancel { name } => chain.push(quote! { .cancel_timer(#name) }),
                Effect::Other { kind } => {
                    let code = if matches!(kind.as_str(), "use" | "acquire" | "release") {
                        RESOURCE_EFFECT
                    } else {
                        EFFECT_UNSUPPORTED
                    };
                    self.refuse(
                        code,
                        format!("{what} of machine \"{}\" is a \"{kind}\" effect, which the runtime cannot express.", b.id),
                    );
                    ok = false;
                }
            }
        }
        if !ok {
            return None;
        }
        let outcome = if sets > 0 {
            quote! { Outcome::to(State::#target, next) }
        } else {
            quote! { Outcome::to(State::#target, vars.clone()) }
        };
        if sets > 0 {
            Some(quote! {
                let mut next = vars.clone();
                #(#statements)*
                Ok(#outcome #(#chain)*)
            })
        } else {
            Some(quote! {
                #(#statements)*
                Ok(#outcome #(#chain)*)
            })
        }
    }

    fn assemble(
        &mut self,
        states: &[Ident],
        initial: usize,
        isolation: TokenStream,
        storage: Option<StorageCode>,
        arms: Vec<TokenStream>,
        covered: usize,
    ) -> TokenStream {
        let b = self.b;
        let spec = self.spec;
        let machine = self.machine.clone();

        // State.
        let state_docs: Vec<String> = b
            .states
            .iter()
            .map(|s| {
                let terminal = if s.terminal { " Terminal." } else { "" };
                let ignores = if s.ignores.is_empty() {
                    String::new()
                } else {
                    format!(" Ignores {}.", quote_list(&s.ignores))
                };
                format!(" State `{}`: {}.{terminal}{ignores}", s.id, s.name)
            })
            .collect();
        let state_ids: Vec<&str> = b.states.iter().map(|s| s.id.as_str()).collect();
        let state_doc = format!(" The states of machine `{}`.", b.id);

        // Vars.
        let total_vars = self
            .vars
            .iter()
            .all(|v| resolve::is_total(spec, &TypeRef::parse(&v.type_name)));
        let var_fields: Vec<TokenStream> = self
            .vars
            .iter()
            .map(|v| {
                let ty = render::rust_type(
                    spec,
                    &TypeRef::parse(&v.type_name),
                    TypeScope::Other,
                    &mut self.uses,
                );
                Field {
                    wire_name: v.name.clone(),
                    rust_name: v.field.to_string(),
                    ty,
                    optional: false,
                    doc: Doc::new(&format!("Variable `{}`.", v.name)),
                    deserialize_with: None,
                }
                .tokens()
            })
            .collect();
        let vars_derive = render::derives(total_vars);
        let mut initial_values: Vec<TokenStream> = Vec::new();
        for v in &b.variables {
            let Some(slot) = self.vars.iter().find(|s| s.name == v.name).cloned() else {
                continue;
            };
            let item = {
                let t = TypeRef::parse(&slot.type_name);
                let value = match &v.initial {
                    Some(value) => self.literal(value, &t),
                    None if matches!(t, TypeRef::Option(_)) => Ok(quote! { None }),
                    None => Err("no initial value is given".to_string()),
                };
                let field = &slot.field;
                match value {
                    Ok(value) => Some(quote! { #field: #value }),
                    Err(reason) => {
                        self.out.push(Diagnostic::new(
                            LITERAL,
                            &b.id,
                            format!(
                                "The initial value of variable \"{}\" of machine \"{}\" cannot be written: {reason}.",
                                v.name, b.id
                            ),
                        ));
                        None
                    }
                }
            };
            initial_values.extend(item);
        }

        // Payload structs, once each, in the order the events are declared.
        let mut payload_items = Vec::new();
        let mut done = BTreeSet::new();
        let events: Vec<(&Event, Ident, Vec<Slot>)> = self
            .received
            .iter()
            .chain(self.emitted.iter())
            .map(|e| (e.event, e.payload_struct.clone(), e.fields.clone()))
            .collect();
        for (event, ps, fields) in &events {
            if !done.insert(ps.to_string()) {
                continue;
            }
            let total = fields
                .iter()
                .all(|f| resolve::is_total(spec, &TypeRef::parse(&f.type_name)));
            let derive = render::derives(total);
            let field_tokens: Vec<TokenStream> = fields
                .iter()
                .map(|f| {
                    let ty = render::rust_type(
                        spec,
                        &TypeRef::parse(&f.type_name),
                        TypeScope::Other,
                        &mut self.uses,
                    );
                    let tokens = Field {
                        wire_name: crate::wire::event_case(spec, event).name(&f.name),
                        rust_name: f.field.to_string(),
                        ty,
                        optional: f.optional,
                        doc: Doc::new(&format!("Field `{}`.", f.name)),
                        deserialize_with: None,
                    }
                    .tokens();
                    // A u64 or i64 beyond +-(2^53 - 1) is refused when the payload is decoded,
                    // as Exegra refuses it in a scenario (ready.number-bound).
                    match safe_decoder(&TypeRef::parse(&f.type_name), f.optional) {
                        Some((attr, decoder)) => {
                            self.uses.prelude = true;
                            quote! { #attr #[serde(deserialize_with = #decoder)] #tokens }
                        }
                        None => tokens,
                    }
                })
                .collect();
            let mut doc = Doc::new(&format!(
                "The payload of event `{}`: {}.",
                event.id, event.name
            ));
            if let Some(d) = &event.description {
                doc.paragraph(d);
            }
            let doc = doc.tokens();
            payload_items.push(quote! {
                #doc
                #derive
                pub struct #ps {
                    #(#field_tokens)*
                }
            });
        }

        let event_enum = self.event_enum(
            quote! { Event },
            &format!(
                " The events machine `{}` receives, including those its timers fire.",
                b.id
            ),
            &self.received,
        );
        let emitted_enum = self.event_enum(
            quote! { Emitted },
            &format!(" The events machine `{}` emits.", b.id),
            &self.emitted,
        );

        // Storage: a record-stored machine converts to and from its record; a memory machine has
        // no record, key or transaction.
        let storage_code = storage.as_ref().map(|storage| {
            let record_ty = &storage.record_ty;
            let record_id = &storage.record.id;
            let key_ty = &storage.key_ty;
            let key_doc = format!(
                " The instance key: field `{}` of record `{}`.",
                storage.key_field, record_id
            );
            let to_fields = &storage.to_fields;
            let from_fields = &storage.from_fields;
            let state_param = if storage.uses_state {
                quote! { state }
            } else {
                quote! { _state }
            };
            let previous_param = if storage.uses_previous {
                quote! { previous }
            } else {
                quote! { _previous }
            };
            let vars_param = if self.vars.is_empty() {
                quote! { _vars }
            } else {
                quote! { vars }
            };
            let record_param = if storage.reads_record {
                quote! { record }
            } else {
                quote! { _record }
            };
            let read_state = &storage.read_state;
            let creates = storage.creates;
            // `instances.max` of a keyed machine that creates its records: the runtime refuses an
            // event that would create one instance more (`Machine::MAX_INSTANCES`), as Exegra does.
            let max_instances = match b.instances.as_ref().and_then(|i| i.max) {
                Some(max) if creates => quote! { const MAX_INSTANCES: Option<u64> = Some(#max); },
                _ => quote! {},
            };
            let previous_required = if storage.uses_previous && !creates {
                let message = format!(
                    "machine `{}` does not create records of `{}`, and the key has none",
                    b.id, record_id
                );
                quote! {
                    let Some(previous) = previous else {
                        return Err(RecordError(#message.to_string()));
                    };
                }
            } else {
                TokenStream::new()
            };
            StorageTokens {
                key_doc,
                key_ty: quote! { #key_ty },
                record_id: record_id.clone(),
                record_ty: quote! { #record_ty },
                creates,
                record_param,
                read_state: read_state.clone(),
                from_fields: from_fields.clone(),
                state_param,
                vars_param,
                previous_param,
                previous_required,
                max_instances,
                to_fields: to_fields.clone(),
                machine_doc: format!(
                    " Machine `{}`, stored in record `{}` by `{}`.",
                    b.id, record_id, storage.key_field
                ),
            }
        });
        let initial_state = &states[initial];
        let name = &b.id;

        // Step.
        let total_pairs = b.states.len() * self.received.len();
        let fallback = if covered < total_pairs {
            quote! { _ => Ok(Outcome::ignore(*state, vars.clone())), }
        } else {
            TokenStream::new()
        };
        let now = if self.used_time.get() {
            quote! { now }
        } else {
            quote! { _now }
        };
        let overflow = if self.used_overflow.get() {
            quote! {
                /// The refusal of a step whose arithmetic overflows or divides by zero.
                fn overflow(transition: &str) -> StepError {
                    StepError::Rejected(format!(
                        "transition {transition} overflowed or divided by zero"
                    ))
                }
            }
        } else {
            TokenStream::new()
        };

        let mut runtime = Vec::new();
        if self.has_emits {
            runtime.push(quote! { Delivery });
        }
        if storage_code.is_some() {
            runtime.extend([
                quote! { Isolation },
                quote! { Machine },
                quote! { Outcome },
                quote! { RecordError },
                quote! { StepError },
                quote! { Time },
            ]);
        } else {
            // A memory machine steps to a `MemoryOutcome`, which the module names `Outcome`.
            runtime.extend([
                quote! { MemoryOutcome },
                quote! { StepError },
                quote! { Time },
            ]);
        }
        let duration = if self.has_timers {
            quote! { use std::time::Duration; }
        } else {
            TokenStream::new()
        };
        let mut crate_mods = Vec::new();
        if self.uses.prelude {
            crate_mods.push(quote! { prelude });
        }
        if storage_code.is_some() {
            crate_mods.push(quote! { records });
        }
        if self.uses.types {
            crate_mods.push(quote! { types });
        }
        let crate_use = match crate_mods.as_slice() {
            [] => TokenStream::new(),
            [only] => quote! { use crate::#only; },
            _ => quote! { use crate::{#(#crate_mods),*}; },
        };
        // Logging: the events it emits that the specification logs (telemetry.rs).
        let log_arms: Vec<(TokenStream, TokenStream)> = self
            .emitted
            .iter()
            .filter_map(|e| {
                let fields: Vec<(String, Ident)> = e
                    .fields
                    .iter()
                    .map(|f| (f.name.clone(), f.field.clone()))
                    .collect();
                super::telemetry::emitted_arm(self.spec, e.event, &e.variant, &fields)
            })
            .collect();
        let partial = log_arms.len() < self.emitted.len();
        let log_body = match log_arms.as_slice() {
            [] => TokenStream::new(),
            [(pattern, call)] if partial => quote! {
                if let #pattern = emitted {
                    #call;
                }
            },
            arms => {
                let rest = if partial {
                    quote! { _ => {} }
                } else {
                    TokenStream::new()
                };
                let (patterns, calls): (Vec<_>, Vec<_>) = arms.iter().cloned().unzip();
                quote! {
                    match emitted {
                        #( #patterns => #calls, )*
                        #rest
                    }
                }
            }
        };
        // In a `Machine` impl `log_emitted` is a trait method; a memory machine has it inherent.
        let log_vis = if storage_code.is_some() {
            TokenStream::new()
        } else {
            quote! { pub }
        };
        let log_emitted = if log_arms.is_empty() {
            TokenStream::new()
        } else {
            quote! {
                #log_vis fn log_emitted(emitted: &Emitted, telemetry: &dyn strata_runtime::Telemetry) {
                    #log_body
                }
            }
        };
        // Events that stand for elapsed time are fired only by a timer: the hosts refuse them
        // from outside, by this.
        let elapsed_variants: Vec<&Ident> = self
            .received
            .iter()
            .filter(|e| e.event.elapsed)
            .map(|e| &e.variant)
            .collect();
        let is_elapsed = if elapsed_variants.is_empty() {
            quote! { let _ = event; false }
        } else {
            quote! { matches!(event, #(Event::#elapsed_variants(_))|*) }
        };

        let machine_items = match &storage_code {
            Some(st) => {
                let StorageTokens {
                    key_doc,
                    key_ty,
                    record_id,
                    record_ty,
                    creates,
                    record_param,
                    read_state,
                    from_fields,
                    state_param,
                    vars_param,
                    previous_param,
                    previous_required,
                    max_instances,
                    to_fields,
                    machine_doc,
                } = st;
                quote! {
                    #[doc = #key_doc]
                    pub type Key = #key_ty;

                    #[doc = #machine_doc]
                    #[derive(Debug, Clone, Copy, Default)]
                    pub struct #machine;

                    #overflow

                    impl Machine for #machine {
                        const NAME: &'static str = #name;
                        const RECORD: &'static str = #record_id;
                        const ISOLATION: Isolation = #isolation;
                        const CREATES: bool = #creates;
                        #max_instances

                        type State = State;
                        type Vars = Vars;
                        type Event = Event;
                        type Emitted = Emitted;
                        type Key = Key;
                        type Record = records::#record_ty;

                        fn initial(_key: &Key) -> (State, Vars) {
                            (State::#initial_state, Vars { #(#initial_values),* })
                        }

                        fn from_record(#record_param: &records::#record_ty) -> Result<(State, Vars), RecordError> {
                            #read_state
                            Ok((state, Vars { #(#from_fields),* }))
                        }

                        fn to_record(
                            key: &Key,
                            #state_param: &State,
                            #vars_param: &Vars,
                            #previous_param: Option<&records::#record_ty>,
                        ) -> Result<records::#record_ty, RecordError> {
                            #previous_required
                            Ok(records::#record_ty { #(#to_fields),* })
                        }

                        fn is_elapsed(event: &Event) -> bool {
                            #is_elapsed
                        }

                        fn validate_record(record: &records::#record_ty) -> Result<(), String> {
                            record.validate().map_err(|violation| violation.to_string())
                        }

                        fn step(state: &State, vars: &Vars, event: &Event, #now: Time) -> Result<Outcome<Self>, StepError> {
                            match (state, event) {
                                #(#arms)*
                                #fallback
                            }
                        }

                        #log_emitted
                    }
                }
            }
            None => {
                let machine_doc = format!(
                    " Machine `{}`, held in memory: it has no record, key or transaction, so the `Host` and `System` cannot run it; its `step` is pure.",
                    b.id
                );
                quote! {
                    /// What a step of the machine yields.
                    pub type Outcome = MemoryOutcome<State, Vars, Event, Emitted>;

                    #[doc = #machine_doc]
                    #[derive(Debug, Clone, Copy, Default)]
                    pub struct #machine;

                    #overflow

                    impl #machine {
                        /// The behavior's id.
                        pub const NAME: &'static str = #name;

                        /// The state and variables of a new instance.
                        pub fn initial() -> (State, Vars) {
                            (State::#initial_state, Vars { #(#initial_values),* })
                        }

                        /// Whether `event` stands for elapsed time: only a timer the machine set fires it.
                        pub fn is_elapsed(event: &Event) -> bool {
                            #is_elapsed
                        }

                        /// Takes one transition at time `now`: the first enabled transition from `state` on `event`, in declaration order, or an ignore.
                        pub fn step(state: &State, vars: &Vars, event: &Event, #now: Time) -> Result<Outcome, StepError> {
                            match (state, event) {
                                #(#arms)*
                                #fallback
                            }
                        }

                        #log_emitted
                    }
                }
            }
        };

        quote! {
            #duration

            use serde::{Deserialize, Serialize};
            use strata_runtime::{#(#runtime),*};

            #crate_use

            #[doc = #state_doc]
            #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
            pub enum State {
                #(
                    #[doc = #state_docs]
                    #[serde(rename = #state_ids)]
                    #states,
                )*
            }

            impl State {
                /// The state's id in the specification.
                pub fn id(self) -> &'static str {
                    match self {
                        #( State::#states => #state_ids, )*
                    }
                }

                /// The state with this id in the specification.
                pub fn from_id(id: &str) -> Option<State> {
                    match id {
                        #( #state_ids => Some(State::#states), )*
                        _ => None,
                    }
                }
            }

            /// The variables of the machine.
            #vars_derive
            pub struct Vars {
                #(#var_fields)*
            }

            #(#payload_items)*

            #event_enum

            #emitted_enum

            #machine_items
        }
    }

    fn event_enum(&self, name: TokenStream, doc: &str, events: &[EventInfo<'_>]) -> TokenStream {
        let spec = self.spec;
        if events.is_empty() {
            return quote! {
                #[doc = #doc]
                #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
                pub enum #name {}
            };
        }
        let total = events.iter().all(|e| {
            e.fields
                .iter()
                .all(|f| resolve::is_total(spec, &TypeRef::parse(&f.type_name)))
        });
        let derive = render::derives(total);
        let variants = events.iter().map(|e| {
            let id = &e.event.id;
            let doc = format!(" Event `{}`: {}.", id, e.event.name);
            let (v, ps) = (&e.variant, &e.payload_struct);
            quote! {
                #[doc = #doc]
                #[serde(rename = #id)]
                #v(#ps),
            }
        });
        quote! {
            #[doc = #doc]
            #derive
            #[serde(tag = "event", content = "payload")]
            pub enum #name {
                #(#variants)*
            }
        }
    }
}

/// The tokens `assemble` writes for a record-stored machine's `Machine` impl.
struct StorageTokens {
    key_doc: String,
    key_ty: TokenStream,
    record_id: String,
    record_ty: TokenStream,
    creates: bool,
    record_param: TokenStream,
    read_state: TokenStream,
    from_fields: Vec<TokenStream>,
    state_param: TokenStream,
    vars_param: TokenStream,
    previous_param: TokenStream,
    previous_required: TokenStream,
    /// `const MAX_INSTANCES` for a keyed machine that creates its records and states `instances.max`.
    max_instances: TokenStream,
    to_fields: Vec<TokenStream>,
    machine_doc: String,
}

/// What `assemble` needs to know about the storage record.
struct StorageCode {
    record: Record,
    record_ty: Ident,
    key_field: String,
    key_ty: TokenStream,
    /// The statements of `from_record` that bind `state`.
    read_state: TokenStream,
    /// True when `from_record` reads the record at all.
    reads_record: bool,
    creates: bool,
    uses_state: bool,
    uses_previous: bool,
    to_fields: Vec<TokenStream>,
    from_fields: Vec<TokenStream>,
}

/// A record field's type split into whether it may be unset and the type it holds when set: a
/// field that is optional or whose type is `option<T>` is optional with base T.
fn field_base(f: &RecordField) -> (bool, String) {
    let t = f.type_name.trim();
    match t.strip_prefix("option<").and_then(|r| r.strip_suffix('>')) {
        Some(inner) => (true, inner.trim().to_string()),
        None => (f.optional, t.to_string()),
    }
}

/// How a variable is held in a record field.
enum Fit {
    /// A variable of repr `option<T>` in an optional field of type T: stored as it is.
    Option,
    /// A field of the variable's repr, optional or not.
    Same { optional: bool },
    /// A field of declared id or scalar type `base` whose repr is the variable's repr.
    Declared { optional: bool, base: String },
}

/// How a variable of representation `repr` fits record field `f`, or `None` when it does not
/// (Exegra's `reprFitsField`).
fn fit(spec: &Spec, repr: &str, f: &RecordField) -> Option<Fit> {
    let r = repr.trim();
    let (optional, base) = field_base(f);
    if let Some(inner) = r.strip_prefix("option<").and_then(|r| r.strip_suffix('>')) {
        return (optional && base == inner.trim()).then_some(Fit::Option);
    }
    if base == r {
        return Some(Fit::Same { optional });
    }
    let declared = spec.type_decl(&base).and_then(|d| match &d.kind {
        TypeKind::Id { repr } | TypeKind::Scalar { repr } => repr.as_deref().map(str::trim),
        _ => None,
    });
    (declared == Some(r)).then_some(Fit::Declared { optional, base })
}

fn wrap_some(value: TokenStream, optional: bool) -> TokenStream {
    if optional {
        quote! { Some(#value) }
    } else {
        value
    }
}

/// How the state is held in its record field.
enum StateCode {
    /// Not stored: the machine has one state.
    Unstored,
    /// A string field holding each state's id.
    Ids,
    /// A string field holding a value per state (`stateValues`).
    Strings(Vec<(Ident, String)>),
    /// An enum field: its Rust type, the variant per state, and whether the enum has values no
    /// state is stored as.
    Enum {
        ty: TokenStream,
        variants: Vec<(Ident, Ident)>,
        partial: bool,
    },
}

impl StateCode {
    /// The value of the state field in `to_record`.
    fn write(&self) -> TokenStream {
        match self {
            StateCode::Unstored => TokenStream::new(),
            StateCode::Ids => quote! { state.id().to_string() },
            StateCode::Strings(values) => {
                let (states, values): (Vec<_>, Vec<_>) = values.iter().cloned().unzip();
                quote! {
                    match state {
                        #( State::#states => #values, )*
                    }
                    .to_string()
                }
            }
            StateCode::Enum { ty, variants, .. } => {
                let (states, values): (Vec<_>, Vec<_>) = variants.iter().cloned().unzip();
                quote! {
                    match state {
                        #( State::#states => #ty::#values, )*
                    }
                }
            }
        }
    }

    /// The statements of `from_record` binding `state` from record field `field`.
    fn read(&self, field: &Ident, machine: &str, only: &Ident) -> TokenStream {
        let message = format!("{{:?}} is not a state of machine `{machine}`");
        let other_message = format!("{{other:?}} is not a state of machine `{machine}`");
        match self {
            StateCode::Unstored => quote! { let state = State::#only; },
            StateCode::Ids => quote! {
                let state = State::from_id(&record.#field)
                    .ok_or_else(|| RecordError(format!(#message, record.#field)))?;
            },
            StateCode::Strings(values) => {
                let (states, values): (Vec<_>, Vec<_>) = values.iter().cloned().unzip();
                quote! {
                    let state = match record.#field.as_str() {
                        #( #values => State::#states, )*
                        other => return Err(RecordError(format!(#other_message))),
                    };
                }
            }
            StateCode::Enum {
                ty,
                variants,
                partial,
            } => {
                let (states, values): (Vec<_>, Vec<_>) = variants.iter().cloned().unzip();
                let rest = if *partial {
                    quote! { other => return Err(RecordError(format!(#other_message))), }
                } else {
                    TokenStream::new()
                };
                quote! {
                    let state = match &record.#field {
                        #( #ty::#values => State::#states, )*
                        #rest
                    };
                }
            }
        }
    }
}

/// How the state of `b` is held in `field` of `record`, or why it cannot be (Exegra's
/// `stateFieldProblem`).
fn state_storage(
    spec: &Spec,
    b: &Behavior,
    record: &Record,
    field: &str,
    state_values: Option<&[(String, Option<String>)]>,
    owned: &[(String, String)],
    uses: &mut Uses,
) -> Result<StateCode, String> {
    let record_id = &record.id;
    let Some(f) = record.field(field) else {
        return Err(format!(
            "Record \"{record_id}\" has no field \"{field}\" to hold the state of \"{}\" (storage.state names the field; default \"state\").",
            b.id
        ));
    };
    if let Some((_, taken)) = owned.iter().find(|(o, _)| o == field) {
        return Err(format!(
            "Field \"{field}\" of record \"{record_id}\" holds {taken}, so it cannot also hold the state of \"{}\".",
            b.id
        ));
    }
    let (optional, base) = field_base(f);
    let enum_values = spec.type_decl(&base).and_then(|d| match &d.kind {
        TypeKind::Enum { values } => Some(values.clone()),
        _ => None,
    });
    if optional {
        return Err(format!(
            "Field \"{field}\" of record \"{record_id}\" is optional; the state of \"{}\" needs a required field.",
            b.id
        ));
    }
    if base != "string" && enum_values.is_none() {
        return Err(format!(
            "Field \"{field}\" of record \"{record_id}\" has type \"{base}\"; the state of \"{}\" is held in a string or an enum field.",
            b.id
        ));
    }
    if let Some(sv) = state_values {
        for (k, v) in sv {
            if !b.states.iter().any(|s| s.id == *k) {
                return Err(format!(
                    "storage.stateValues of \"{}\" names \"{k}\", which is not one of its states.",
                    b.id
                ));
            }
            if v.is_none() {
                return Err(format!(
                    "storage.stateValues of \"{}\" gives state \"{k}\" a value that is not a string.",
                    b.id
                ));
            }
        }
    }
    let stored: Vec<String> = b
        .states
        .iter()
        .map(|s| {
            state_values
                .and_then(|sv| sv.iter().find(|(k, _)| *k == s.id))
                .and_then(|(_, v)| v.clone())
                .unwrap_or_else(|| s.id.clone())
        })
        .collect();
    if let Some(dup) = stored
        .iter()
        .enumerate()
        .find(|(i, v)| stored[..*i].contains(v))
        .map(|(_, v)| v)
    {
        return Err(format!(
            "Two states of \"{}\" would be stored as \"{dup}\" in field \"{field}\", so the state could not be read back.",
            b.id
        ));
    }
    let states: Vec<Ident> = b
        .states
        .iter()
        .map(|s| naming::ident(&naming::pascal(&s.id).unwrap_or_default()))
        .collect();
    let Some(values) = enum_values else {
        if state_values.is_none() {
            return Ok(StateCode::Ids);
        }
        return Ok(StateCode::Strings(states.into_iter().zip(stored).collect()));
    };
    let missing: Vec<usize> = (0..stored.len())
        .filter(|i| !values.contains(&stored[*i]))
        .collect();
    if !missing.is_empty() {
        let list = |items: Vec<&String>| {
            items
                .iter()
                .map(|v| format!("\"{v}\""))
                .collect::<Vec<_>>()
                .join(", ")
        };
        return Err(format!(
            "Field \"{field}\" of record \"{record_id}\" is enum \"{base}\", whose values do not include {} (for state{} {} of \"{}\"); map the states through storage.stateValues.",
            list(missing.iter().map(|i| &stored[*i]).collect()),
            if missing.len() > 1 { "s" } else { "" },
            list(missing.iter().map(|i| &b.states[*i].id).collect()),
            b.id
        ));
    }
    let ty = render::rust_type(spec, &TypeRef::Declared(base), TypeScope::Other, uses);
    let variants = states
        .into_iter()
        .zip(
            stored
                .iter()
                .map(|v| naming::ident(&naming::pascal(v).unwrap_or_default())),
        )
        .collect();
    Ok(StateCode::Enum {
        ty,
        variants,
        partial: values.len() > stored.len(),
    })
}

/// A scope with nothing in it, for literals.
struct NoScope;

impl expr::Scope for NoScope {
    fn resolve(&self, _path: &[&str]) -> Option<Binding> {
        None
    }

    fn overflow_error(&self) -> TokenStream {
        quote! { unreachable!() }
    }
}

fn quote_list(items: &[String]) -> String {
    items
        .iter()
        .map(|i| format!("`{i}`"))
        .collect::<Vec<_>>()
        .join(", ")
}
