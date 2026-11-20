//! Derived contracts (`realisation: "derived"`, Exegra WG-4935) to the `derived` module: one
//! generated implementation per derived contract, built from the specification's functions
//! ([`crate::function`]) and, for a read or a command, from the contracts it loads and commits
//! through, called through their component's trait (`contracts::<Component>`). Nothing here
//! names a particular specification's contracts.
//!
//! - **pure** `{function}`: `pub fn <contract>(request) -> Result<Response, Error>` calls the
//!   function with the request's inputs of the same names and builds the response from what it
//!   gives: the one output, or the fields of the struct it gives.
//! - **read** `{needs, load, decide}`: `pub async fn <contract>(<component>: &S, request)`
//!   computes the needs, calls the load contract (its inputs are the request's inputs of the
//!   same names and the needs), and calls decide with the request's inputs and the load
//!   contract's outputs of the same names.
//! - **command** `{needs, load, decide, commit, conflict, insufficient, needRounds, retries}`:
//!   the load-decide-commit loop of `crates/storage-api/src/lib.rs` `execute`. Load and decide;
//!   while decide fails with the variant `insufficient` carrying a need that is not yet asked for,
//!   and fewer than `needRounds` extra needs were added, add it and load again. Commit what
//!   decide gave (its inputs: the request's inputs and the load outputs of the same names, and
//!   decide's value); while the commit fails with `conflict` and fewer than `retries` commits were
//!   retried, start over from the needs. Each output is a commit output, or a field of decide's
//!   value, of the same name. A bound of zero emits no loop for it.
//!
//! Every other failure becomes the contract's error of the same words (`VersionConflict` is
//! `version-conflict`): a function's error variant (a unit variant, or a newtype carrying the
//! error's payload), a load or commit error, and `insufficient` and `conflict` once their bound
//! is spent. A function that can fault (checked integer arithmetic) needs `derivation.fault`,
//! the payload-free error a fault becomes. The functions are the pure crate's when there is one,
//! the crate's own `functions` module otherwise.
//!
//! What specc relies on is checked again here ([`DERIVED`] for what Exegra refuses too,
//! [`UNSUPPORTED`] for what Exegra accepts and specc cannot emit), so a specification that was
//! not validated cannot produce code that means something else.

use std::collections::BTreeMap;

use proc_macro2::{Ident, Literal, Span, TokenStream};
use quote::quote;

use super::Backend;
use crate::diagnostic::Diagnostic;
use crate::function::{self, Shape};
use crate::ir::{Contract, ContractField, Derivation, Realisation, Spec, StructField, TypeKind};
use crate::naming;
use crate::render::Doc;

/// Code of a derivation Exegra refuses too (derivation.*): it cannot mean an implementation.
pub const DERIVED: &str = "specc.derived-invalid";
/// Code of a derivation Exegra accepts that specc cannot emit.
pub const UNSUPPORTED: &str = "specc.derived-unsupported";
/// The module name.
pub const MODULE: &str = "derived";

/// Names the generated implementations bind; a component handle may not take one.
const LOCALS: &[&str] = &[
    "request",
    "needs",
    "loaded",
    "decision",
    "decided",
    "committed",
    "rounds",
    "conflicts",
    "value",
    "error",
    "need",
];

/// The derived-contracts backend.
pub struct DerivedBackend;

impl Backend for DerivedBackend {
    fn module(&self) -> &'static str {
        MODULE
    }

    fn wanted(&self, spec: &Spec) -> bool {
        spec.contracts.iter().any(is_derived)
    }

    fn check(&self, spec: &Spec) -> Vec<Diagnostic> {
        let derived: Vec<&Contract> = spec.contracts.iter().filter(|c| is_derived(c)).collect();
        if derived.is_empty() {
            return Vec::new();
        }
        // Functions that do not compile are reported by the function check.
        let Some(shapes) = function::shapes(spec) else {
            return Vec::new();
        };
        derived
            .into_iter()
            .filter_map(|c| plan(spec, &shapes, c).err())
            .map(|(code, message)| Diagnostic::new(code, &message.contract, message.text))
            .collect()
    }

    fn emit(&self, spec: &Spec) -> TokenStream {
        emit_with(spec, None)
    }
}

/// A refusal: the contract and the text.
struct Message {
    contract: String,
    text: String,
}

/// A derived contract of the shipped code: a target contract has no implementation there.
fn is_derived(c: &Contract) -> bool {
    !c.target && c.realisation == Some(Realisation::Derived)
}

/// The module, with the functions in the pure crate `core_crate` (its Rust name) when there is
/// one. Assumes the checks passed.
pub fn emit_with(spec: &Spec, core_crate: Option<&str>) -> TokenStream {
    let shapes = function::shapes(spec).unwrap_or_default();
    let functions = match core_crate {
        Some(krate) => {
            let krate = Ident::new(krate, Span::call_site());
            quote! { #krate::functions }
        }
        None => quote! { crate::functions },
    };
    let mut items = Vec::new();
    let mut uses_types = false;
    for c in spec.contracts.iter().filter(|c| is_derived(c)) {
        if let Ok(p) = plan(spec, &shapes, c) {
            uses_types |= p.uses_types();
            items.push(p.emit(&functions));
        }
    }
    let types_use = if uses_types {
        quote! { use crate::types; }
    } else {
        TokenStream::new()
    };
    quote! {
        #![doc = " The derived contracts of the specification: each one's implementation, generated from its derivation (functions, and the contracts it loads and commits through, called through their component's trait). A component's trait still has a method for a derived contract it implements; that method calls the function here."]
        #![allow(clippy::clone_on_copy, clippy::redundant_clone, clippy::redundant_field_names)]

        use crate::contracts;
        #types_use

        #(#items)*
    }
}

// ---------------------------------------------------------------------------------------------
// Planning: what each value is filled from, checked as Exegra checks it.

/// A value in scope: an input of the request, or an output of the load contract.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    Input(String),
    Loaded(String),
    Needs,
    Decided,
}

/// One argument of a function call.
struct Arg {
    source: Source,
    by_ref: bool,
}

/// How a failure maps onto the derived contract's error: the variant it comes from, and the
/// variant of the derived contract's error enum it becomes.
struct ErrMap {
    from: String,
    to: String,
    payload: bool,
}

/// A call of a function and what its failures become.
struct Call {
    function: String,
    args: Vec<Arg>,
    /// The error union and its variants' mapping, when the function declares errors.
    errors: Option<(String, Vec<ErrMap>)>,
    faultable: bool,
}

/// A call of another contract through its component's trait.
struct ContractCall {
    contract: String,
    handle: usize,
    /// Each input of the called contract: its name and what fills it (`None`: an optional input
    /// nothing fills, left out).
    fills: Vec<(String, Option<Source>)>,
    errors: Vec<ErrMap>,
}

/// Where an output of the derived contract comes from.
enum Out {
    /// The function's value itself (the contract's one output).
    Value,
    /// A field of the function's value.
    Field(String),
    /// An output of the commit contract.
    Committed(String),
}

enum Body {
    Pure(Call),
    Read {
        needs: Call,
        load: ContractCall,
        decide: Call,
    },
    Command {
        needs: Call,
        load: ContractCall,
        decide: Call,
        commit: ContractCall,
        conflict: (String, bool),
        insufficient: String,
        need_rounds: u64,
        retries: u64,
    },
}

struct Plan<'a> {
    contract: &'a Contract,
    /// The components whose traits are called: (component id, handle name).
    handles: Vec<(String, String)>,
    body: Body,
    outputs: Vec<(String, Out)>,
    fault: Option<String>,
}

type Refusal = (&'static str, Message);

fn refuse(code: &'static str, c: &Contract, text: String) -> Refusal {
    (
        code,
        Message {
            contract: c.id.clone(),
            text: format!("derived contract {}: {text}.", c.id),
        },
    )
}

/// A type text without white space.
fn norm(t: &str) -> String {
    t.chars().filter(|c| !c.is_whitespace()).collect()
}

/// The type of a contract field: `option<T>` when it is optional.
fn field_type(f: &ContractField) -> String {
    let t = norm(f.type_name.as_deref().unwrap_or_default());
    if f.optional {
        format!("option<{t}>")
    } else {
        t
    }
}

fn struct_field_type(f: &StructField) -> String {
    let t = norm(&f.type_name);
    if f.optional {
        format!("option<{t}>")
    } else {
        t
    }
}

/// The words of a name joined by `-`: how error variants and contract errors are matched.
fn words(name: &str) -> String {
    naming::words(name).unwrap_or_default().join("-")
}

fn pascal(name: &str) -> String {
    naming::pascal(name).unwrap_or_default()
}

/// The variants of a union as errors: (name, payload type), or a refusal for a variant with
/// named fields or a boxed field.
fn union_errors(spec: &Spec, union: &str) -> Result<Vec<(String, Option<String>)>, String> {
    let Some(decl) = spec.type_decl(union) else {
        return Err(format!("the error union {union} is not declared"));
    };
    let TypeKind::Union { variants, .. } = &decl.kind else {
        return Err(format!("the errors {union} are not a union"));
    };
    let mut out = Vec::new();
    for v in variants {
        match v.fields.as_slice() {
            [] => out.push((v.name.clone(), None)),
            [f] if v.newtype => {
                if super::types::field_is_boxed(spec, decl, f) {
                    return Err(format!(
                        "the variant {} of {union} holds its payload boxed, which a contract error does not",
                        v.name
                    ));
                }
                out.push((v.name.clone(), Some(struct_field_type(f))));
            }
            _ => {
                return Err(format!(
                    "the variant {} of {union} has named fields; a variant that becomes a contract error is a unit variant or a newtype of the error's payload type",
                    v.name
                ))
            }
        }
    }
    Ok(out)
}

/// Maps each `(name, payload)` failure onto the error of `c` with the same words and payload.
fn map_errors(
    c: &Contract,
    errs: &[(String, Option<String>)],
    what: &str,
) -> Result<Vec<ErrMap>, String> {
    let mut out = Vec::new();
    for (name, payload) in errs {
        let Some(target) = c.errors.iter().find(|e| words(&e.name) == words(name)) else {
            return Err(format!("{what} {name} is not an error of the contract"));
        };
        let want = target.type_name.as_deref().map(norm);
        if &want != payload {
            return Err(format!(
                "{what} {name} carries {}, but the contract's error {} carries {}",
                payload.as_deref().unwrap_or("nothing"),
                target.name,
                want.as_deref().unwrap_or("nothing")
            ));
        }
        out.push(ErrMap {
            from: pascal(name),
            to: pascal(&target.name),
            payload: payload.is_some(),
        });
    }
    Ok(out)
}

fn function_of<'s>(spec: &'s Spec, id: &str) -> Option<&'s function::Function> {
    spec.functions.iter().find(|f| f.id == id)
}

/// A call of function `id` with its parameters from `scope`, checked.
fn call(
    spec: &Spec,
    shapes: &BTreeMap<String, Shape>,
    c: &Contract,
    id: &str,
    role: &str,
    scope: &[(String, String, Source)],
) -> Result<(Call, String), Refusal> {
    let Some(f) = function_of(spec, id) else {
        return Err(refuse(
            DERIVED,
            c,
            format!("its {role} function {id} is not declared"),
        ));
    };
    let Some(shape) = shapes.get(id) else {
        return Err(refuse(
            DERIVED,
            c,
            format!("its {role} function {id} does not compile"),
        ));
    };
    let mut args = Vec::new();
    for (p, (_, by_ref)) in f.params.iter().zip(&shape.params) {
        let Some((_, t, source)) = scope.iter().find(|(n, _, _)| n == &p.name) else {
            return Err(refuse(
                DERIVED,
                c,
                format!(
                    "parameter {} of its {role} function {id} is not an input of the contract{}",
                    p.name,
                    if role == "decide" {
                        " nor an output of its load contract"
                    } else {
                        ""
                    }
                ),
            ));
        };
        if *t != norm(&p.type_name) {
            return Err(refuse(
                DERIVED,
                c,
                format!(
                    "parameter {} of its {role} function {id} is {}, but the value it is given is {t}",
                    p.name,
                    norm(&p.type_name)
                ),
            ));
        }
        args.push(Arg {
            source: source.clone(),
            by_ref: *by_ref,
        });
    }
    let errors = match &shape.errors {
        None => None,
        Some(union) => {
            let errs = union_errors(spec, union).map_err(|m| refuse(DERIVED, c, m))?;
            let mapped = map_errors(c, &errs, &format!("the error variant of {id}"))
                .map_err(|m| refuse(DERIVED, c, m))?;
            Some((union.clone(), mapped))
        }
    };
    let returns = norm(f.returns.as_deref().unwrap_or_default());
    Ok((
        Call {
            function: id.to_string(),
            args,
            errors,
            faultable: shape.faultable,
        },
        returns,
    ))
}

/// The struct fields of a declared struct type, or `None`.
fn struct_fields<'s>(
    spec: &'s Spec,
    t: &str,
) -> Option<(&'s crate::ir::TypeDecl, &'s [StructField])> {
    let decl = spec.type_decl(t)?;
    match &decl.kind {
        TypeKind::Struct { fields } => Some((decl, fields.as_slice())),
        _ => None,
    }
}

/// The outputs of `c` from a function's value of type `returns`: the one output, or the fields
/// of the struct it is.
fn outputs_from_value(
    spec: &Spec,
    c: &Contract,
    returns: &str,
    id: &str,
) -> Result<Vec<(String, Out)>, Refusal> {
    if let [only] = c.outputs.as_slice() {
        if field_type(only) == returns {
            return Ok(vec![(only.name.clone(), Out::Value)]);
        }
    }
    if let Some((decl, fields)) = struct_fields(spec, returns) {
        let fits = fields.len() == c.outputs.len()
            && c.outputs.iter().all(|o| {
                fields
                    .iter()
                    .any(|f| f.name == o.name && struct_field_type(f) == field_type(o))
            });
        if fits {
            if fields
                .iter()
                .any(|f| super::types::field_is_boxed(spec, decl, f))
            {
                return Err(refuse(
                    UNSUPPORTED,
                    c,
                    format!(
                        "{returns}, what {id} gives, holds a field boxed, which an output does not"
                    ),
                ));
            }
            return Ok(c
                .outputs
                .iter()
                .map(|o| (o.name.clone(), Out::Field(o.name.clone())))
                .collect());
        }
    }
    Err(refuse(
        DERIVED,
        c,
        format!(
            "{id} gives {returns}, which is neither the type of the contract's one output nor a struct whose fields are exactly its outputs"
        ),
    ))
}

/// The handle index of the component serving `called`, added to `handles` when new.
fn handle(
    spec: &Spec,
    c: &Contract,
    called: &Contract,
    handles: &mut Vec<(String, String)>,
) -> Result<usize, Refusal> {
    let Some(component) = called.served_by().filter(|id| spec.component(id).is_some()) else {
        return Err(refuse(
            UNSUPPORTED,
            c,
            format!(
                "contract {} is served by no declared component, so there is no trait to call it through",
                called.id
            ),
        ));
    };
    if let Some(i) = handles.iter().position(|(id, _)| id == component) {
        return Ok(i);
    }
    let name = naming::snake(component).unwrap_or_default();
    if LOCALS.contains(&name.as_str()) {
        return Err(refuse(
            UNSUPPORTED,
            c,
            format!("the component {component} would be the parameter `{name}`, a name the implementation binds itself"),
        ));
    }
    handles.push((component.to_string(), name));
    Ok(handles.len() - 1)
}

/// The contract a derivation calls, checked.
fn called<'s>(
    spec: &'s Spec,
    c: &Contract,
    id: Option<&str>,
    role: &str,
) -> Result<&'s Contract, Refusal> {
    let Some(id) = id else {
        return Err(refuse(DERIVED, c, format!("it names no {role} contract")));
    };
    let Some(called) = spec.contracts.iter().find(|o| o.id == id) else {
        return Err(refuse(
            DERIVED,
            c,
            format!("its {role} contract {id} is not declared"),
        ));
    };
    if called.id == c.id || called.realisation == Some(Realisation::Facade) {
        return Err(refuse(
            DERIVED,
            c,
            format!("its {role} contract {id} is itself or a facade, which nothing serves"),
        ));
    }
    Ok(called)
}

/// Fills the inputs of `called` by name from `scope`, else by type with `extra`, which fills
/// exactly one.
fn fills(
    c: &Contract,
    called: &Contract,
    scope: &[(String, String, Source)],
    extra: (&str, Source),
    role: &str,
) -> Result<Vec<(String, Option<Source>)>, Refusal> {
    let mut out = Vec::new();
    let mut taken = 0;
    for i in &called.inputs {
        let t = field_type(i);
        if let Some((_, _, s)) = scope.iter().find(|(n, st, _)| n == &i.name && *st == t) {
            out.push((i.name.clone(), Some(s.clone())));
        } else if t == extra.0 {
            taken += 1;
            out.push((i.name.clone(), Some(extra.1.clone())));
        } else if i.optional {
            out.push((i.name.clone(), None));
        } else {
            return Err(refuse(
                DERIVED,
                c,
                format!(
                    "input {} of its {role} contract {} is filled by nothing of that name and type",
                    i.name, called.id
                ),
            ));
        }
    }
    if taken != 1 {
        return Err(refuse(
            DERIVED,
            c,
            format!(
                "{taken} inputs of its {role} contract {} take {}; exactly one must",
                called.id, extra.0
            ),
        ));
    }
    Ok(out)
}

fn contract_errors(called: &Contract) -> Vec<(String, Option<String>)> {
    called
        .errors
        .iter()
        .map(|e| (e.name.clone(), e.type_name.as_deref().map(norm)))
        .collect()
}

fn bound(c: &Contract, n: Option<&serde_json::Number>, name: &str) -> Result<u64, Refusal> {
    n.and_then(serde_json::Number::as_u64).ok_or_else(|| {
        refuse(
            DERIVED,
            c,
            format!("{name} is not stated as a non-negative integer"),
        )
    })
}

fn plan<'a>(
    spec: &Spec,
    shapes: &BTreeMap<String, Shape>,
    c: &'a Contract,
) -> Result<Plan<'a>, Refusal> {
    let Some(d) = &c.derivation else {
        return Err(refuse(DERIVED, c, "it states no derivation".to_string()));
    };
    let inputs: Vec<(String, String, Source)> = c
        .inputs
        .iter()
        .map(|i| (i.name.clone(), field_type(i), Source::Input(i.name.clone())))
        .collect();
    let mut handles = Vec::new();
    let (body, outputs) = match d.kind.as_deref() {
        Some("pure") => {
            let id = required(c, &d.function, "function")?;
            let (f, returns) = call(spec, shapes, c, id, "pure", &inputs)?;
            let outputs = outputs_from_value(spec, c, &returns, id)?;
            (Body::Pure(f), outputs)
        }
        Some(kind @ ("read" | "command")) => {
            let needs_id = required(c, &d.needs, "needs")?;
            let decide_id = required(c, &d.decide, "decide")?;
            let (needs, needs_type) = call(spec, shapes, c, needs_id, "needs", &inputs)?;
            let Some(need) = needs_type
                .strip_prefix("list<")
                .and_then(|t| t.strip_suffix('>'))
                .map(str::to_string)
            else {
                return Err(refuse(
                    DERIVED,
                    c,
                    format!(
                        "its needs function {needs_id} gives {needs_type}, not a list of needs"
                    ),
                ));
            };
            let load_contract = called(spec, c, d.load.as_deref(), "load")?;
            let load_handle = handle(spec, c, load_contract, &mut handles)?;
            let load_fills = fills(
                c,
                load_contract,
                &inputs,
                (needs_type.as_str(), Source::Needs),
                "load",
            )?;
            let load_errors = map_errors(
                c,
                &contract_errors(load_contract),
                &format!("the error of its load contract {}", load_contract.id),
            )
            .map_err(|m| refuse(DERIVED, c, m))?;
            let mut scope = inputs.clone();
            for o in &load_contract.outputs {
                if inputs.iter().any(|(n, _, _)| n == &o.name) {
                    return Err(refuse(
                        DERIVED,
                        c,
                        format!(
                            "output {} of its load contract {} is named like an input",
                            o.name, load_contract.id
                        ),
                    ));
                }
                scope.push((
                    o.name.clone(),
                    field_type(o),
                    Source::Loaded(o.name.clone()),
                ));
            }
            let (decide, decided_type) = call(spec, shapes, c, decide_id, "decide", &scope)?;
            let load = ContractCall {
                contract: load_contract.id.clone(),
                handle: load_handle,
                fills: load_fills,
                errors: load_errors,
            };
            if kind == "read" {
                let outputs = outputs_from_value(spec, c, &decided_type, decide_id)?;
                (
                    Body::Read {
                        needs,
                        load,
                        decide,
                    },
                    outputs,
                )
            } else {
                command(
                    spec,
                    c,
                    d,
                    &mut handles,
                    (needs, need),
                    load,
                    (decide, decided_type),
                    &scope,
                )?
            }
        }
        other => {
            return Err(refuse(
                DERIVED,
                c,
                format!(
                    "its derivation has kind {}, not pure, read or command",
                    other.unwrap_or("(none)")
                ),
            ))
        }
    };
    let faultable = match &body {
        Body::Pure(f) => f.faultable,
        Body::Read { needs, decide, .. } | Body::Command { needs, decide, .. } => {
            needs.faultable || decide.faultable
        }
    };
    let fault = match &d.fault {
        None if faultable => {
            return Err(refuse(
                UNSUPPORTED,
                c,
                "a function it calls can fault (checked integer arithmetic), and its derivation names no fault error for that".to_string(),
            ))
        }
        None => None,
        Some(name) => match c.errors.iter().find(|e| &e.name == name) {
            Some(e) if e.type_name.is_none() => Some(pascal(&e.name)),
            _ => {
                return Err(refuse(
                    DERIVED,
                    c,
                    format!("its fault {name} is not an error of the contract without a payload"),
                ))
            }
        },
    };
    Ok(Plan {
        contract: c,
        handles,
        body,
        outputs,
        fault,
    })
}

fn required<'d>(c: &Contract, v: &'d Option<String>, name: &str) -> Result<&'d str, Refusal> {
    v.as_deref()
        .ok_or_else(|| refuse(DERIVED, c, format!("its derivation names no {name}")))
}

#[allow(clippy::too_many_arguments)]
fn command(
    spec: &Spec,
    c: &Contract,
    d: &Derivation,
    handles: &mut Vec<(String, String)>,
    (needs, need): (Call, String),
    load: ContractCall,
    (decide, decided_type): (Call, String),
    scope: &[(String, String, Source)],
) -> Result<(Body, Vec<(String, Out)>), Refusal> {
    let decide_id = decide.function.clone();
    let insufficient = required(c, &d.insufficient, "insufficient")?;
    let union = decide.errors.as_ref().map(|(u, _)| u.clone());
    let Some(union) = union else {
        return Err(refuse(
            DERIVED,
            c,
            format!("its insufficient variant {insufficient} needs errors on {decide_id}, which declares none"),
        ));
    };
    let errs = union_errors(spec, &union).map_err(|m| refuse(DERIVED, c, m))?;
    match errs.iter().find(|(n, _)| n == insufficient) {
        Some((_, Some(t))) if *t == need => {}
        _ => {
            return Err(refuse(
                DERIVED,
                c,
                format!("its insufficient variant {insufficient} is not a variant of {union} carrying one need, {need}"),
            ))
        }
    }
    let commit_contract = called(spec, c, d.commit.as_deref(), "commit")?;
    let commit_handle = handle(spec, c, commit_contract, handles)?;
    let commit_fills = fills(
        c,
        commit_contract,
        scope,
        (decided_type.as_str(), Source::Decided),
        "commit",
    )?;
    let conflict_name = required(c, &d.conflict, "conflict")?;
    let Some(conflict) = commit_contract
        .errors
        .iter()
        .find(|e| e.name == conflict_name)
    else {
        return Err(refuse(
            DERIVED,
            c,
            format!(
                "its conflict {conflict_name} is not an error of its commit contract {}",
                commit_contract.id
            ),
        ));
    };
    let commit_errors = map_errors(
        c,
        &contract_errors(commit_contract),
        &format!("the error of its commit contract {}", commit_contract.id),
    )
    .map_err(|m| refuse(DERIVED, c, m))?;
    let decided_fields = struct_fields(spec, &decided_type);
    let mut outputs = Vec::new();
    for o in &c.outputs {
        let t = field_type(o);
        if commit_contract
            .outputs
            .iter()
            .any(|co| co.name == o.name && field_type(co) == t)
        {
            outputs.push((o.name.clone(), Out::Committed(o.name.clone())));
            continue;
        }
        if let Some((decl, fields)) = decided_fields {
            if let Some(f) = fields
                .iter()
                .find(|f| f.name == o.name && struct_field_type(f) == t)
            {
                if super::types::field_is_boxed(spec, decl, f) {
                    return Err(refuse(
                        UNSUPPORTED,
                        c,
                        format!("output {} is a boxed field of {decided_type}", o.name),
                    ));
                }
                outputs.push((o.name.clone(), Out::Field(o.name.clone())));
                continue;
            }
        }
        if c.outputs.len() == 1 && t == decided_type {
            outputs.push((o.name.clone(), Out::Value));
            continue;
        }
        return Err(refuse(
            DERIVED,
            c,
            format!(
                "output {} is neither an output of its commit contract {} nor a field of {decided_type}, what {decide_id} gives",
                o.name, commit_contract.id
            ),
        ));
    }
    let need_rounds = bound(c, d.need_rounds.as_ref(), "needRounds")?;
    let retries = bound(c, d.retries.as_ref(), "retries")?;
    let commit = ContractCall {
        contract: commit_contract.id.clone(),
        handle: commit_handle,
        fills: commit_fills,
        errors: commit_errors,
    };
    Ok((
        Body::Command {
            needs,
            load,
            decide,
            commit,
            conflict: (pascal(&conflict.name), conflict.type_name.is_some()),
            insufficient: pascal(insufficient),
            need_rounds,
            retries,
        },
        outputs,
    ))
}

// ---------------------------------------------------------------------------------------------
// Emission.

fn ident(name: &str) -> Ident {
    naming::ident(name)
}

fn snake(name: &str) -> Ident {
    ident(&naming::snake(name).unwrap_or_default())
}

fn contract_type(id: &str, suffix: &str) -> TokenStream {
    let name = ident(&format!("{}{suffix}", pascal(id)));
    quote! { contracts::#name }
}

fn union_path(id: &str) -> TokenStream {
    let name = ident(&pascal(id));
    quote! { types::#name }
}

fn literal(n: u64) -> Literal {
    Literal::u64_unsuffixed(n)
}

impl Plan<'_> {
    fn uses_types(&self) -> bool {
        let calls: Vec<&Call> = match &self.body {
            Body::Pure(f) => vec![f],
            Body::Read { needs, decide, .. } | Body::Command { needs, decide, .. } => {
                vec![needs, decide]
            }
        };
        calls.iter().any(|f| f.errors.is_some())
    }

    fn error_type(&self) -> TokenStream {
        contract_type(&self.contract.id, "Error")
    }

    /// The mapping of a failure `error` of `f` onto the contract's error.
    fn map_call_error(&self, f: &Call, functions: &TokenStream) -> ErrorMap {
        let target = self.error_type();
        let map_union = |(union, maps): &(String, Vec<ErrMap>)| {
            let path = union_path(union);
            let arms = maps.iter().map(|m| {
                let (from, to) = (ident(&m.from), ident(&m.to));
                if m.payload {
                    quote! { #path::#from(payload) => #target::#to(payload), }
                } else {
                    quote! { #path::#from => #target::#to, }
                }
            });
            quote! { match error { #(#arms)* } }
        };
        let fault = self.fault.as_deref().map(|f| {
            let f = ident(f);
            quote! { #target::#f }
        });
        match (&f.errors, f.faultable) {
            (Some(e), false) => ErrorMap::Match(map_union(e)),
            (None, true) => ErrorMap::Const(fault.unwrap_or_default()),
            (Some(e), true) => {
                let inner = map_union(e);
                let fault = fault.unwrap_or_default();
                ErrorMap::Match(quote! {
                    match error {
                        #functions::Failure::Error(error) => #inner,
                        #functions::Failure::Fault(_) => #fault,
                    }
                })
            }
            (None, false) => ErrorMap::Never,
        }
    }

    /// The expression of a call of `f` (its `Result` when it can fail).
    fn call_expr(&self, f: &Call, functions: &TokenStream) -> TokenStream {
        let name = snake(&f.function);
        let args = f.args.iter().map(|a| {
            let value = match &a.source {
                Source::Input(n) => {
                    let n = snake(n);
                    quote! { request.#n }
                }
                Source::Loaded(n) => {
                    let n = snake(n);
                    quote! { loaded.#n }
                }
                // Exegra fills no function parameter from these.
                Source::Needs => quote! { needs },
                Source::Decided => quote! { decided },
            };
            if a.by_ref {
                quote! { &#value }
            } else {
                quote! { #value }
            }
        });
        quote! { #functions::#name(#(#args),*) }
    }

    /// `let <name> = <call>;` (`let mut` when `mutable`), returning the mapped error when the
    /// call fails.
    fn bind_call(
        &self,
        name: &Ident,
        f: &Call,
        functions: &TokenStream,
        mutable: bool,
    ) -> TokenStream {
        let call = self.call_expr(f, functions);
        let binding = if mutable {
            quote! { mut #name }
        } else {
            quote! { #name }
        };
        if f.errors.is_none() && !f.faultable {
            return quote! { let #binding = #call; };
        }
        let arm = err_arm(self.map_call_error(f, functions), false);
        quote! {
            let #binding = match #call {
                Ok(value) => value,
                #arm
            };
        }
    }

    /// The request of a contract call.
    fn request(&self, call: &ContractCall, clone_needs: bool, clone_decided: bool) -> TokenStream {
        let ty = contract_type(&call.contract, "Request");
        let fields = call.fills.iter().map(|(field, source)| {
            let f = snake(field);
            let value = match source {
                None => quote! { None },
                Some(Source::Input(n)) => {
                    let n = snake(n);
                    quote! { request.#n.clone() }
                }
                Some(Source::Loaded(n)) => {
                    let n = snake(n);
                    quote! { loaded.#n }
                }
                Some(Source::Needs) if clone_needs => quote! { needs.clone() },
                Some(Source::Needs) => quote! { needs },
                Some(Source::Decided) if clone_decided => quote! { decided.clone() },
                Some(Source::Decided) => quote! { decided },
            };
            quote! { #f: #value }
        });
        quote! { #ty { #(#fields),* } }
    }

    /// The mapping of a called contract's error `error` onto the derived contract's error.
    fn map_contract_error(&self, call: &ContractCall) -> ErrorMap {
        if call.errors.is_empty() {
            return ErrorMap::Never;
        }
        let from_ty = contract_type(&call.contract, "Error");
        let target = self.error_type();
        let arms = call.errors.iter().map(|m| {
            let (from, to) = (ident(&m.from), ident(&m.to));
            if m.payload {
                quote! { #from_ty::#from(payload) => #target::#to(payload), }
            } else {
                quote! { #from_ty::#from => #target::#to, }
            }
        });
        ErrorMap::Match(quote! { match error { #(#arms)* } })
    }

    /// `<handle>.<method>(<request>).await`.
    fn contract_call(&self, call: &ContractCall, request: TokenStream) -> TokenStream {
        let handle = ident(&self.handles[call.handle].1);
        let method = snake(&call.contract);
        quote! { #handle.#method(#request).await }
    }

    /// `let loaded = ...;` through the load contract.
    fn bind_load(&self, load: &ContractCall, clone_needs: bool, used: bool) -> TokenStream {
        let request = self.request(load, clone_needs, false);
        let call = self.contract_call(load, request);
        let arm = err_arm(self.map_contract_error(load), false);
        let name = if used {
            ident("loaded")
        } else {
            ident("_loaded")
        };
        quote! {
            let #name = match #call {
                Ok(loaded) => loaded,
                #arm
            };
        }
    }

    /// The response from the outputs' sources.
    fn response(&self, value: &Ident) -> TokenStream {
        let ty = contract_type(&self.contract.id, "Response");
        let fields = self.outputs.iter().map(|(name, out)| {
            let f = snake(name);
            match out {
                Out::Value => quote! { #f: #value },
                Out::Field(n) => {
                    let n = snake(n);
                    quote! { #f: #value.#n }
                }
                Out::Committed(n) => {
                    let n = snake(n);
                    quote! { #f: committed.#n }
                }
            }
        });
        quote! { #ty { #(#fields),* } }
    }

    fn doc(&self) -> TokenStream {
        let c = self.contract;
        let mut doc = Doc::new(&c.name);
        let reference = c
            .reference
            .as_deref()
            .map(|r| format!(" ({r})"))
            .unwrap_or_default();
        let how = match &self.body {
            Body::Pure(f) => format!("the function `{}`", f.function),
            Body::Read { needs, load, decide } => format!(
                "a read: the needs `{}` gives, loaded through `{}`, decided by `{}`",
                needs.function, load.contract, decide.function
            ),
            Body::Command {
                needs,
                load,
                decide,
                commit,
                need_rounds,
                retries,
                ..
            } => format!(
                "a command: the needs `{}` gives, loaded through `{}`, decided by `{}` (at most {need_rounds} more needs), committed through `{}` (at most {retries} retries after a conflict)",
                needs.function, load.contract, decide.function, commit.contract
            ),
        };
        doc.paragraph(&format!(
            "Contract `{}`{reference}, derived from {how}.",
            c.id
        ));
        doc.tokens()
    }

    fn uses_request(&self) -> bool {
        let in_call = |f: &Call| f.args.iter().any(|a| matches!(a.source, Source::Input(_)));
        let in_fills = |c: &ContractCall| {
            c.fills
                .iter()
                .any(|(_, s)| matches!(s, Some(Source::Input(_))))
        };
        match &self.body {
            Body::Pure(f) => in_call(f),
            Body::Read {
                needs,
                load,
                decide,
            } => in_call(needs) || in_fills(load) || in_call(decide),
            Body::Command {
                needs,
                load,
                decide,
                commit,
                ..
            } => in_call(needs) || in_fills(load) || in_call(decide) || in_fills(commit),
        }
    }

    fn emit(&self, functions: &TokenStream) -> TokenStream {
        let name = snake(&self.contract.id);
        let request_ty = contract_type(&self.contract.id, "Request");
        let response_ty = contract_type(&self.contract.id, "Response");
        let error_ty = self.error_type();
        let doc = self.doc();
        let request = if self.uses_request() {
            ident("request")
        } else {
            ident("_request")
        };
        let value = ident("value");
        let decided = ident("decided");
        match &self.body {
            Body::Pure(f) => {
                let bind = self.bind_call(&value, f, functions, false);
                let response = self.response(&value);
                quote! {
                    #doc
                    pub fn #name(#request: #request_ty) -> Result<#response_ty, #error_ty> {
                        #bind
                        Ok(#response)
                    }
                }
            }
            Body::Read {
                needs,
                load,
                decide,
            } => {
                let generics = self.generics();
                let params = self.handle_params();
                let needs_name = ident("needs");
                let bind_needs = self.bind_call(&needs_name, needs, functions, false);
                let loaded_used = decide
                    .args
                    .iter()
                    .any(|a| matches!(a.source, Source::Loaded(_)));
                let bind_load = self.bind_load(load, false, loaded_used);
                let bind_decide = self.bind_call(&value, decide, functions, false);
                let response = self.response(&value);
                quote! {
                    #doc
                    pub async fn #name<#generics>(#params #request: #request_ty) -> Result<#response_ty, #error_ty> {
                        #bind_needs
                        #bind_load
                        #bind_decide
                        Ok(#response)
                    }
                }
            }
            Body::Command {
                needs,
                load,
                decide,
                commit,
                conflict,
                insufficient,
                need_rounds,
                retries,
            } => {
                let generics = self.generics();
                let params = self.handle_params();
                let rounds = *need_rounds > 0;
                let needs_name = ident("needs");
                let bind_needs = self.bind_call(&needs_name, needs, functions, rounds);
                let loaded_used = decide
                    .args
                    .iter()
                    .any(|a| matches!(a.source, Source::Loaded(_)))
                    || commit
                        .fills
                        .iter()
                        .any(|(_, s)| matches!(s, Some(Source::Loaded(_))));
                let bind_load = self.bind_load(load, rounds, loaded_used);
                let loaded = if loaded_used {
                    ident("loaded")
                } else {
                    ident("_loaded")
                };
                let decide_call = self.call_expr(decide, functions);
                let decide_arm = err_arm(self.map_call_error(decide, functions), false);
                let decide_part = if rounds {
                    let union = union_path(
                        &decide
                            .errors
                            .as_ref()
                            .map(|(u, _)| u.clone())
                            .unwrap_or_default(),
                    );
                    let variant = ident(insufficient);
                    let pattern = if decide.faultable {
                        quote! { Err(#functions::Failure::Error(#union::#variant(need))) }
                    } else {
                        quote! { Err(#union::#variant(need)) }
                    };
                    let n = literal(*need_rounds);
                    quote! {
                        let mut rounds: u64 = 0;
                        let (#loaded, decided) = loop {
                            #bind_load
                            let decision = #decide_call;
                            match decision {
                                Ok(decided) => break (#loaded, decided),
                                #pattern if rounds < #n && !needs.contains(&need) => {
                                    needs.push(need);
                                    rounds += 1;
                                }
                                #decide_arm
                            }
                        };
                    }
                } else {
                    quote! {
                        #bind_load
                        let decided = match #decide_call {
                            Ok(decided) => decided,
                            #decide_arm
                        };
                    }
                };
                let decided_read = self
                    .outputs
                    .iter()
                    .any(|(_, o)| matches!(o, Out::Value | Out::Field(_)));
                let commit_request = self.request(commit, false, decided_read);
                let commit_call = self.contract_call(commit, commit_request);
                let commit_return = err_arm(self.map_contract_error(commit), false);
                let commit_tail = err_arm(self.map_contract_error(commit), true);
                let committed_used = self
                    .outputs
                    .iter()
                    .any(|(_, o)| matches!(o, Out::Committed(_)));
                let committed = if committed_used {
                    ident("committed")
                } else {
                    ident("_committed")
                };
                let response = self.response(&decided);
                let conflict_ty = contract_type(&commit.contract, "Error");
                let conflict_variant = ident(&conflict.0);
                let conflict_pattern = if conflict.1 {
                    quote! { Err(#conflict_ty::#conflict_variant(_)) }
                } else {
                    quote! { Err(#conflict_ty::#conflict_variant) }
                };
                let body = if *retries > 0 {
                    let r = literal(*retries);
                    quote! {
                        let mut conflicts: u64 = 0;
                        loop {
                            #bind_needs
                            #decide_part
                            match #commit_call {
                                Ok(#committed) => return Ok(#response),
                                #conflict_pattern if conflicts < #r => conflicts += 1,
                                #commit_return
                            }
                        }
                    }
                } else {
                    quote! {
                        #bind_needs
                        #decide_part
                        match #commit_call {
                            Ok(#committed) => Ok(#response),
                            #commit_tail
                        }
                    }
                };
                quote! {
                    #doc
                    pub async fn #name<#generics>(#params #request: #request_ty) -> Result<#response_ty, #error_ty> {
                        #body
                    }
                }
            }
        }
    }

    /// `S0: contracts::<Component> + Sync, ...`.
    fn generics(&self) -> TokenStream {
        let bounds = self.handles.iter().enumerate().map(|(i, (component, _))| {
            let g = ident(&format!("S{i}"));
            let tr = ident(&pascal(component));
            quote! { #g: contracts::#tr + Sync }
        });
        quote! { #(#bounds),* }
    }

    /// `<handle>: &S0, ...` with a trailing comma.
    fn handle_params(&self) -> TokenStream {
        let params = self.handles.iter().enumerate().map(|(i, (_, name))| {
            let g = ident(&format!("S{i}"));
            let n = ident(name);
            quote! { #n: &#g, }
        });
        quote! { #(#params)* }
    }
}

/// What a failure `error` becomes.
enum ErrorMap {
    /// An expression over `error`.
    Match(TokenStream),
    /// An expression that does not read it.
    Const(TokenStream),
    /// Nothing: the error type has no values.
    Never,
}

/// The arm for `Err(error)`: returning the mapped error, or (`tail`) giving it as the value.
fn err_arm(map: ErrorMap, tail: bool) -> TokenStream {
    let give = |m: TokenStream| {
        if tail {
            quote! { Err(#m) }
        } else {
            quote! { return Err(#m) }
        }
    };
    match map {
        ErrorMap::Match(m) => {
            let g = give(m);
            quote! { Err(error) => #g, }
        }
        ErrorMap::Const(m) => {
            let g = give(m);
            quote! { Err(_) => #g, }
        }
        ErrorMap::Never => quote! { Err(error) => match error {}, },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(text: &str) -> Result<String, Vec<Diagnostic>> {
        let spec = crate::load::load("s.jsonc", text).expect("loads");
        let d = DerivedBackend.check(&spec);
        if !d.is_empty() {
            return Err(d);
        }
        Ok(DerivedBackend.emit(&spec).to_string())
    }

    const BASE: &str = r#"{ "system": { "id": "s", "name": "S", "generation": { "target": "rust" } },
        "functions": [{ "id": "larger", "params": [{ "name": "a", "type": "u64" }, { "name": "b", "type": "u64" }],
            "returns": "u64", "body": "if a > b then a else b", "examples": [{ "args": { "a": 1, "b": 2 }, "result": 2 }] },
          { "id": "next", "params": [{ "name": "a", "type": "u64" }], "returns": "u64", "body": "a + 1",
            "examples": [{ "args": { "a": 1 }, "result": 2 }] }],
        "contracts": [CONTRACT] }"#;

    fn spec(contract: &str) -> String {
        BASE.replace("CONTRACT", contract)
    }

    #[test]
    fn a_pure_contract_calls_its_function_and_answers_its_one_output() {
        let out = module(&spec(
            r#"{ "id": "pick", "name": "Pick", "realisation": "derived", "derivation": { "kind": "pure", "function": "larger" },
                 "inputs": [{ "name": "a", "type": "u64" }, { "name": "b", "type": "u64" }],
                 "outputs": [{ "name": "larger", "type": "u64" }], "errors": [] }"#,
        ))
        .unwrap();
        assert!(
            out.contains("pub fn pick (request : contracts :: PickRequest)"),
            "{out}"
        );
        assert!(
            out.contains("crate :: functions :: larger (request . a , request . b)"),
            "{out}"
        );
        assert!(
            out.contains("contracts :: PickResponse { larger : value }"),
            "{out}"
        );
    }

    #[test]
    fn a_function_that_can_fault_needs_a_fault_error() {
        let contract = r#"{ "id": "bump", "name": "Bump", "realisation": "derived", "derivation": { "kind": "pure", "function": "next" FAULT },
                 "inputs": [{ "name": "a", "type": "u64" }], "outputs": [{ "name": "b", "type": "u64" }],
                 "errors": [{ "name": "overflow" }] }"#;
        let refused = module(&spec(&contract.replace("FAULT", ""))).unwrap_err();
        assert!(refused.iter().any(|d| d.code == UNSUPPORTED), "{refused:?}");
        let out = module(&spec(
            &contract.replace("FAULT", r#", "fault": "overflow""#),
        ))
        .unwrap();
        assert!(
            out.contains("Err (_) => return Err (contracts :: BumpError :: Overflow)"),
            "{out}"
        );
    }

    #[test]
    fn a_parameter_the_contract_does_not_give_is_refused() {
        let refused = module(&spec(
            r#"{ "id": "pick", "name": "Pick", "realisation": "derived", "derivation": { "kind": "pure", "function": "larger" },
                 "inputs": [{ "name": "a", "type": "u64" }], "outputs": [{ "name": "larger", "type": "u64" }], "errors": [] }"#,
        ))
        .unwrap_err();
        assert!(
            refused
                .iter()
                .any(|d| d.code == DERIVED && d.message.contains("parameter b")),
            "{refused:?}"
        );
    }
}
