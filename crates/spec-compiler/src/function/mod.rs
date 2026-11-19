//! Functions (`functions`, kind `function`; Exegra WG-4933) compiled into Rust (WG-4934): every
//! function becomes a `pub fn` of the module `functions` of the pure crate (the model crate when
//! there is none), and every example a unit test of it.
//!
//! Exegra's evaluator (`src/core/function.js`) is the reference: specc parses bodies as
//! `function-syntax.js` does ([`syntax`]), types them as `typeFunction` does ([`typing`]), and
//! refuses what it cannot compile with `specc.function-unsupported` (or `specc.function-invalid`
//! for what Exegra itself refuses) instead of emitting code that means something else.
//!
//! # Translation
//!
//! - Parameters and returns have the generated types (`types::Name`, `Vec`, `Option`,
//!   `BTreeMap`). A function with `errors` returns `Result<R, E>` with its error union; `fail(e)`
//!   is `return Err(e)` and `try call f(..)` is `f(..)?`. A call of a function with errors
//!   without `try` is the `Result` value.
//! - `if`, `let`, `match` (on unions, options, results and enums), constructors and list
//!   literals are Rust's own. `==` compares structurally; `x == null` is `x.is_none()`,
//!   `xs == []` is `xs.is_empty()`, an option against a plain value compares
//!   `opt.as_ref() == Some(&v)`, an id or scalar against its representation compares `.0`.
//! - `map`, `filter`, `find`, `any`, `all`, `fold` and `sortBy` are iterator adapters over
//!   `iter()` (or `values()` / `keys()` of a map, without collecting them first). When the step
//!   can return early (a `?` or a `fail`), it cannot sit in a closure, and the operation is a
//!   `for` loop instead. `take`, `concat`, `keys`, `values`, `get`, `insert` and `remove` are
//!   the slice and `BTreeMap` operations; `format` is `format!`.
//! - `eval` and `check` of a language call its generated module ([`crate::language`]) through an
//!   adapter that implements the module's environment traits over the environment struct.
//!
//! # Timestamps, sets and tuple variants
//!
//! - `timestamp` is `prelude::Timestamp(i64)`, a `Copy` value taken by value. Exegra types it as
//!   a number, so it is compiled as an id over `i64` is: literals and `i64`s stand for it (built
//!   as `Timestamp(n)`), it stands for them (`.0`), it compares and sorts by its `i64`, and
//!   arithmetic on it is checked `i64` arithmetic wrapped back into `Timestamp`. An example writes
//!   it as its integer. Dividing a timestamp is refused, since Exegra's division of it can give a
//!   fraction.
//! - `set<T>` is a `BTreeSet<T>`. Exegra has no set operations: a set is a list to it, and every
//!   list operation takes one, iterating it in order (`iter()` of the `BTreeSet`); `get(s, i)`
//!   is its `i`-th item. What the operations give is a list (`Vec`), concatenation too. A list
//!   stands where a set is expected and back by collecting its items, converting each where the
//!   item types differ as a single value would be (a `set<role-id>` passed as a `list<string>`);
//!   a list literal there is built as a set, `[]` as an empty one. `s == []` is `is_empty()`.
//!   An example writes a set as an array in the order the generated type derives, each item
//!   once (anything else is refused), so that Exegra, which iterates the array as written,
//!   iterates it as the `BTreeSet` does.
//! - A tuple variant (`Variant(T1, T2, ..)`) is built and taken apart by its declared field
//!   names, as Exegra writes it: `T.V{a: x, b: y}` is `T::V(x, y)` and the pattern `V{b}` is
//!   `T::V(_, b)`. An example writes it as `{"V": {"a": .., "b": ..}}`.
//!
//! Literals and branches are built in the id or scalar expected of them: a list literal, `if`,
//! `match` or `let` where a `list<role-id>` or `set<role-id>` is expected builds its string
//! literals as `RoleId`s, and a string literal of a constrained id or scalar is built directly
//! when it meets the type's constraints, which specc checks on the literal.
//!
//! # Ownership
//!
//! A parameter of a `Copy` type (bool, the integers, timestamp, options of them) is taken by
//! value; every other parameter by reference (`&T`, including `&String` and `&Vec<T>`, so the
//! module allows `clippy::ptr_arg`). Lets own their values. Items bound by a list operation and
//! bindings of a
//! match on a borrowed value are references. A value is cloned only where it must be owned (a
//! constructor field, a list item, a returned value, a fold accumulator); comparisons, field
//! reads, map lookups and iteration borrow.
//!
//! # Integer overflow
//!
//! Integer arithmetic is checked in the type of the expression, as Exegra's typed evaluation and
//! WG-4902 have it: `checked_add`, `checked_sub`, `checked_mul`, `checked_div` and `checked_rem`
//! (so division by zero is a fault too), and `checked_neg`. Operands of different integer types
//! are computed in `i128` and converted back with `try_from`. Nothing panics and nothing wraps.
//! A function that can fault (it does such arithmetic, or calls a function that can) says so in
//! its type: it returns `Result<R, Fault>` when it declares no errors, and
//! `Result<R, Failure<E>>` (`Failure::Error(E)` or `Failure::Fault(Fault)`) when it does. A
//! function that cannot fault keeps the plain `R` or `Result<R, E>`.
//!
//! # Not compiled (refused)
//!
//! `f64`, `duration`, `bytes`, `uuid`, `json` and `shared<T>`; number literals with a fraction;
//! dividing a timestamp; `==` between a set and a list other than `[]`; an example set not
//! written in order or with an item twice; reading or binding a boxed field (a field whose type
//! holds its own type inline); an optional field of an option type; values of a constrained id
//! or scalar built from their representation (but a string literal that meets the constraints);
//! mixing two integer types where a value is passed (rather than computed or
//! compared); names that become the same Rust name, or the name of a function.

mod examples;
mod rust;
mod syntax;
mod typing;

use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::TokenStream;
use quote::quote;
use serde_json::Value;

use crate::diagnostic::Diagnostic;
use crate::ir::Spec;
use crate::load::{Loader, Obj};
use crate::naming;
use crate::render::Doc;
use typing::{BridgeUse, Checker, Refusal, Sig, Ty};

/// Code of a function Exegra would refuse too: a body that does not parse or type, recursion.
pub const FUNCTION_INVALID: &str = "specc.function-invalid";
/// Code of a function Exegra accepts that specc cannot compile.
pub const FUNCTION_UNSUPPORTED: &str = "specc.function-unsupported";
/// The module the functions are generated into.
pub const MODULE: &str = "functions";

/// A `function` entity.
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    /// The function id.
    pub id: String,
    /// The description, if any.
    pub description: Option<String>,
    /// The parameters, in order.
    pub params: Vec<Param>,
    /// The return type.
    pub returns: Option<String>,
    /// The id of the error union.
    pub errors: Option<String>,
    /// The lets, in order.
    pub lets: Vec<Let>,
    /// The body.
    pub body: Option<String>,
    /// The examples, in order.
    pub examples: Vec<Example>,
}

/// A parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    /// Its name.
    pub name: String,
    /// Its type text.
    pub type_name: String,
}

/// A let: a name bound before the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Let {
    /// Its name.
    pub name: String,
    /// Its expression.
    pub expr: String,
}

/// An example: arguments and what the function gives for them.
#[derive(Debug, Clone, PartialEq)]
pub struct Example {
    /// The description, if any.
    pub description: Option<String>,
    /// The arguments by parameter name; a parameter left out is null.
    pub args: serde_json::Map<String, Value>,
    /// The result or the error.
    pub outcome: Outcome,
}

/// An example fixture: a value of a type that examples refer to as `{"fixture": id}` or
/// `{"fixture": id, "with": {path: value}}` instead of writing it out (Exegra WG-4963).
#[derive(Debug, Clone, PartialEq)]
pub struct ExampleFixture {
    /// Its id.
    pub id: String,
    /// The description, if any.
    pub description: Option<String>,
    /// The type text of its value.
    pub type_name: String,
    /// The value, written as examples write values.
    pub value: Value,
}

/// What an example expects.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// `result`: the function gives this value.
    Result(Value),
    /// `error`: the function fails with this value of its error union.
    Error(Value),
}

impl Loader {
    /// The `functions` section.
    pub(crate) fn functions(&mut self, top: &Obj) -> Vec<Function> {
        self.section(top, "functions")
            .into_iter()
            .filter_map(|(id, obj)| self.function(&id, obj))
            .collect()
    }

    /// The `exampleFixtures` section.
    pub(crate) fn example_fixtures(&mut self, top: &Obj) -> Vec<ExampleFixture> {
        let mut out = Vec::new();
        for (id, obj) in self.section(top, "exampleFixtures") {
            let description = self.opt_str(obj, "description", &id);
            let type_name = self.req_str(obj, "type", &id);
            let Some(value) = obj.get("value").cloned() else {
                self.shape(&id, "\"value\" is required");
                continue;
            };
            if let Some(type_name) = type_name {
                out.push(ExampleFixture {
                    id,
                    description,
                    type_name,
                    value,
                });
            }
        }
        out
    }

    fn function(&mut self, id: &str, obj: &Obj) -> Option<Function> {
        let description = self.opt_str(obj, "description", id);
        let mut params = Vec::new();
        for p in self.obj_list(obj, "params", id) {
            if let (Some(name), Some(type_name)) =
                (self.req_str(p, "name", id), self.req_str(p, "type", id))
            {
                params.push(Param { name, type_name });
            }
        }
        let returns = self.opt_str(obj, "returns", id);
        let errors = self.opt_str(obj, "errors", id);
        let mut lets = Vec::new();
        for l in self.obj_list(obj, "let", id) {
            if let (Some(name), Some(expr)) =
                (self.req_str(l, "name", id), self.req_str(l, "expr", id))
            {
                lets.push(Let { name, expr });
            }
        }
        let body = self.opt_str(obj, "body", id);
        let mut examples = Vec::new();
        for ex in self.obj_list(obj, "examples", id) {
            let description = self.opt_str(ex, "description", id);
            let args = match ex.get("args") {
                Some(Value::Object(a)) => a.clone(),
                _ => {
                    self.shape(
                        id,
                        "every example needs \"args\", an object of the parameters",
                    );
                    continue;
                }
            };
            let outcome = match (ex.get("result"), ex.get("error")) {
                (Some(r), None) => Outcome::Result(r.clone()),
                (None, Some(e)) => Outcome::Error(e.clone()),
                _ => {
                    self.shape(
                        id,
                        "an example states exactly one of \"result\" and \"error\"",
                    );
                    continue;
                }
            };
            examples.push(Example {
                description,
                args,
                outcome,
            });
        }
        Some(Function {
            id: id.to_string(),
            description,
            params,
            returns,
            errors,
            lets,
            body,
            examples,
        })
    }
}

fn diag(f: &str, r: Refusal) -> Diagnostic {
    Diagnostic::new(r.code, f, format!("function {f}: {}.", r.message))
}

/// The readiness checks of the functions: every reason specc would not compile one.
pub fn check(spec: &Spec) -> Vec<Diagnostic> {
    match compile(spec) {
        Ok(_) => Vec::new(),
        Err(d) => d,
    }
}

/// True when the specification has functions, so the crate has a `functions` module.
pub fn wanted(spec: &Spec) -> bool {
    !spec.functions.is_empty()
}

/// The `functions` module. Assumes [`check`] passed.
pub fn emit(spec: &Spec) -> TokenStream {
    compile(spec).unwrap_or_default()
}

/// A function parsed and typed.
struct Typed {
    lets: Vec<(String, syntax::Expr)>,
    body: syntax::Expr,
    arith: bool,
    calls: Vec<String>,
}

fn signature(spec: &Spec, f: &Function) -> Result<Sig, Refusal> {
    let mut names: Vec<&str> = Vec::new();
    let mut params = Vec::new();
    for p in &f.params {
        if !syntax::is_bindable_name(&p.name) {
            return Err(typing::invalid(format!(
                "the parameter {:?} is not a plain name",
                p.name
            )));
        }
        if names.contains(&p.name.as_str()) {
            return Err(typing::invalid(format!(
                "the parameter {} is named twice",
                p.name
            )));
        }
        names.push(&p.name);
        let t = typing::ty_of_text(spec, &p.type_name).map_err(|r| Refusal {
            code: r.code,
            message: format!("parameter {}: {}", p.name, r.message),
        })?;
        params.push((p.name.clone(), t));
    }
    for l in &f.lets {
        if !syntax::is_bindable_name(&l.name) || names.contains(&l.name.as_str()) {
            return Err(typing::invalid(format!(
                "the let {:?} is not a plain name, or names a parameter or an earlier let",
                l.name
            )));
        }
        names.push(&l.name);
    }
    let Some(returns) = &f.returns else {
        return Err(typing::invalid("it states no return type"));
    };
    let returns = typing::ty_of_text(spec, returns).map_err(|r| Refusal {
        code: r.code,
        message: format!("the return type: {}", r.message),
    })?;
    let errors = match &f.errors {
        None => None,
        Some(e) => match typing::ty_of_text(spec, e) {
            Ok(Ty::Union(id)) => Some(id),
            _ => {
                return Err(typing::invalid(format!(
                    "its errors, {e}, is not a union type"
                )))
            }
        },
    };
    Ok(Sig {
        id: f.id.clone(),
        params,
        returns,
        errors,
        faultable: false,
    })
}

fn parse(f: &Function) -> Result<(Vec<(String, syntax::Expr)>, syntax::Expr), Refusal> {
    let mut lets = Vec::new();
    for l in &f.lets {
        let e = syntax::parse(&l.expr).map_err(|e| {
            typing::invalid(format!(
                "let {} does not parse: {} (at {})",
                l.name, e.message, e.pos
            ))
        })?;
        lets.push((l.name.clone(), e));
    }
    let Some(body) = &f.body else {
        return Err(typing::invalid("it has no body"));
    };
    let body = syntax::parse(body).map_err(|e| {
        typing::invalid(format!(
            "the body does not parse: {} (at {})",
            e.message, e.pos
        ))
    })?;
    Ok((lets, body))
}

/// The functions on a cycle of the call graph, each cycle by its first function.
fn cycles(calls: &BTreeMap<String, Vec<String>>, order: &[String]) -> Vec<String> {
    let reach = |from: &str| {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<String> = calls.get(from).cloned().unwrap_or_default();
        while let Some(n) = stack.pop() {
            if seen.insert(n.clone()) {
                stack.extend(calls.get(&n).cloned().unwrap_or_default());
            }
        }
        seen
    };
    let reached: BTreeMap<&str, BTreeSet<String>> =
        order.iter().map(|id| (id.as_str(), reach(id))).collect();
    let mut done = BTreeSet::new();
    let mut out = Vec::new();
    for id in order {
        if done.contains(id) || !reached[id.as_str()].contains(id) {
            continue;
        }
        for o in order {
            if reached[id.as_str()].contains(o) && reached[o.as_str()].contains(id) {
                done.insert(o.clone());
            }
        }
        out.push(id.clone());
    }
    out
}

/// The functions named, signed, parsed, checked for cycles and typed, with what each can fault.
struct Analysis {
    reserved: BTreeMap<String, String>,
    sigs: BTreeMap<String, Sig>,
    typed: BTreeMap<String, Typed>,
    bridges: Vec<BridgeUse>,
}

/// How a generated function is called, for code that calls it (the derived contracts).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    /// The parameters in order: the name, and true when it is passed by reference (`&T`).
    pub params: Vec<(String, bool)>,
    /// The id of its error union: it returns `Result<R, E>` (or `Result<R, Failure<E>>`).
    pub errors: Option<String>,
    /// It can fault: it returns `Result<R, Fault>` (or `Result<R, Failure<E>>` with errors).
    pub faultable: bool,
}

/// The call shape of every function, by id; `None` when the functions do not compile (their
/// own diagnostics say why).
pub fn shapes(spec: &Spec) -> Option<BTreeMap<String, Shape>> {
    let analysis = analyse(spec).ok()?;
    Some(
        analysis
            .sigs
            .into_iter()
            .map(|(id, sig)| {
                let shape = Shape {
                    params: sig
                        .params
                        .iter()
                        .map(|(name, t)| (name.clone(), rust::by_ref(t)))
                        .collect(),
                    errors: sig.errors.clone(),
                    faultable: sig.faultable,
                };
                (id, shape)
            })
            .collect(),
    )
}

fn analyse(spec: &Spec) -> Result<Analysis, Vec<Diagnostic>> {
    let mut diags = Vec::new();
    if spec
        .languages
        .iter()
        .any(|l| crate::language::module_name(l) == MODULE)
    {
        diags.push(Diagnostic::new(
            FUNCTION_UNSUPPORTED,
            diagnostic_entity(spec),
            format!("a language becomes the module `{MODULE}`, which holds the functions."),
        ));
    }
    // Names and signatures.
    let mut reserved: BTreeMap<String, String> = BTreeMap::new();
    let mut sigs: BTreeMap<String, Sig> = BTreeMap::new();
    let order: Vec<String> = spec.functions.iter().map(|f| f.id.clone()).collect();
    for f in &spec.functions {
        match naming::snake(&f.id) {
            Ok(rust) => {
                if let Some(other) = reserved.insert(rust.clone(), f.id.clone()) {
                    diags.push(Diagnostic::new(
                        crate::render::NAME_COLLISION,
                        &f.id,
                        format!(
                            "the functions {other} and {} both become the Rust function `{rust}`.",
                            f.id
                        ),
                    ));
                }
            }
            Err(reason) => diags.push(Diagnostic::new(
                crate::render::NAME_INVALID,
                &f.id,
                format!("the function id cannot become a Rust name: {reason}."),
            )),
        }
        match signature(spec, f) {
            Ok(sig) => {
                sigs.insert(f.id.clone(), sig);
            }
            Err(r) => diags.push(diag(&f.id, r)),
        }
    }
    if !diags.is_empty() {
        return Err(diags);
    }
    // Parse, check the call graph, type.
    let mut typed: BTreeMap<String, Typed> = BTreeMap::new();
    let mut calls: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in &spec.functions {
        match parse(f) {
            Ok((lets, body)) => {
                let mut called = Vec::new();
                for (_, e) in &lets {
                    syntax::called(e, &mut called);
                }
                syntax::called(&body, &mut called);
                calls.insert(f.id.clone(), called.clone());
                typed.insert(
                    f.id.clone(),
                    Typed {
                        lets,
                        body,
                        arith: false,
                        calls: called,
                    },
                );
            }
            Err(r) => diags.push(diag(&f.id, r)),
        }
    }
    for id in cycles(&calls, &order) {
        diags.push(diag(
            &id,
            typing::invalid("it calls itself, directly or through other functions; the call graph must be acyclic"),
        ));
    }
    if !diags.is_empty() {
        return Err(diags);
    }
    let mut bridges: Vec<BridgeUse> = Vec::new();
    for f in &spec.functions {
        let t = typed.get_mut(&f.id).expect("parsed");
        let mut checker = Checker::new(spec, &sigs, &sigs[&f.id]);
        match checker.function(&mut t.lets, &mut t.body) {
            Ok(()) => {
                t.arith = checker.arith;
                for b in checker.bridges {
                    if !bridges.contains(&b) {
                        bridges.push(b);
                    }
                }
            }
            Err(r) => diags.push(diag(&f.id, r)),
        }
    }
    if !diags.is_empty() {
        return Err(diags);
    }
    // Which functions can fault: their own arithmetic, or a call of one that can.
    loop {
        let mut changed = false;
        for id in &order {
            let t = &typed[id];
            let can = t.arith
                || t.calls
                    .iter()
                    .any(|c| sigs.get(c).is_some_and(|s| s.faultable));
            if can && !sigs[id].faultable {
                sigs.get_mut(id).expect("signed").faultable = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    Ok(Analysis {
        reserved,
        sigs,
        typed,
        bridges,
    })
}

fn compile(spec: &Spec) -> Result<TokenStream, Vec<Diagnostic>> {
    if !wanted(spec) {
        return Ok(TokenStream::new());
    }
    let Analysis {
        reserved,
        sigs,
        typed,
        bridges,
    } = analyse(spec)?;
    let mut diags = Vec::new();
    // Code.
    let reserved: BTreeSet<String> = reserved.into_keys().collect();
    let mut items = Vec::new();
    let mut tests = Vec::new();
    let mut used_fixtures = BTreeSet::new();
    let mut helpers: BTreeSet<&'static str> = BTreeSet::new();
    for f in &spec.functions {
        let t = &typed[&f.id];
        let sig = &sigs[&f.id];
        let mut gen = rust::Gen::new(spec, &sigs, sig, reserved.clone());
        match gen.function(function_doc(f, sig), &t.lets, &t.body) {
            Ok(item) => {
                items.push(item);
                helpers.extend(gen.helpers.iter().copied());
            }
            Err(r) => {
                diags.push(diag(&f.id, r));
                continue;
            }
        }
        match examples::tests(spec, f, sig, &mut used_fixtures) {
            Ok(mut ts) => tests.append(&mut ts),
            Err(r) => diags.push(diag(&f.id, r)),
        }
    }
    match examples::fixtures(spec, &used_fixtures) {
        Ok(mut fixtures) => {
            fixtures.append(&mut tests);
            tests = fixtures;
        }
        Err((id, r)) => diags.push(Diagnostic::new(
            r.code,
            &id,
            format!("example fixture {id}: {}.", r.message),
        )),
    }
    if !diags.is_empty() {
        return Err(diags);
    }
    Ok(assemble(spec, &sigs, items, tests, &helpers, &bridges))
}

fn diagnostic_entity(spec: &Spec) -> &str {
    spec.functions
        .first()
        .map(|f| f.id.as_str())
        .unwrap_or(crate::diagnostic::SPEC_ENTITY)
}

fn function_doc(f: &Function, sig: &Sig) -> TokenStream {
    let mut doc = Doc::new(f.description.as_deref().unwrap_or(""));
    if f.description.is_none() {
        doc.paragraph(&format!("Function `{}`.", f.id));
    }
    let mut origin = format!("Specification function `{}`", f.id);
    match (&sig.errors, sig.faultable) {
        (None, false) => origin.push('.'),
        (Some(e), false) => origin.push_str(&format!("; fails with `{e}`.")),
        (None, true) => origin.push_str("; an integer overflow or a division by zero is a `Fault`."),
        (Some(e), true) => origin.push_str(&format!(
            "; fails with `{e}` (`Failure::Error`), and an integer overflow or a division by zero is `Failure::Fault`."
        )),
    }
    doc.paragraph(&origin);
    doc.tokens()
}

fn assemble(
    spec: &Spec,
    sigs: &BTreeMap<String, Sig>,
    items: Vec<TokenStream>,
    tests: Vec<TokenStream>,
    helpers: &BTreeSet<&'static str>,
    bridges: &[BridgeUse],
) -> TokenStream {
    let summary = format!(
        " The functions of the specification of {}: pure functions of their arguments, with each example as a test.",
        spec.system.name
    );
    let overflow = " Integer arithmetic is checked: an overflow or a division by zero is a `Fault`, never a panic or a wrapped value. A function that can fault returns `Result<R, Fault>`, or `Result<R, Failure<E>>` when it also declares the errors `E`.";
    let faults = sigs.values().any(|s| s.faultable);
    let failures = sigs.values().any(|s| s.faultable && s.errors.is_some());
    let fault_items = if faults {
        let failure = if failures {
            quote! {
                /// A failure of a function that declares errors and can fault.
                #[derive(Debug, Clone, PartialEq)]
                pub enum Failure<E> {
                    /// One of the function's declared errors.
                    Error(E),
                    /// An integer overflow or a division by zero.
                    Fault(Fault),
                }
            }
        } else {
            TokenStream::new()
        };
        quote! {
            /// An integer overflow or a division by zero, which Exegra's evaluation reports as an
            /// evaluation error.
            #[derive(Debug, Clone, Copy, PartialEq, Eq)]
            pub struct Fault {
                /// The operator: `+`, `-`, `*`, `/` or `%`.
                pub op: &'static str,
            }

            impl std::fmt::Display for Fault {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    write!(f, "integer overflow or division by zero in {}", self.op)
                }
            }

            #failure
        }
    } else {
        TokenStream::new()
    };
    let mut support_items: Vec<TokenStream> = helpers.iter().map(|h| rust::helper(h)).collect();
    let mut records_done = BTreeSet::new();
    for b in bridges {
        let Some(lang) = spec.languages.iter().find(|l| l.id == b.language) else {
            continue;
        };
        let Some(bridge) = crate::language::bridge(spec, lang) else {
            continue;
        };
        if records_done.insert(b.language.clone()) {
            support_items.push(bridge.record_impl());
        }
        let name = rust::bridge_name(b.eval, &b.language, &b.env);
        let env = rust::type_path(&b.env);
        support_items.push(if b.eval {
            bridge.eval_fn(&name, &env)
        } else {
            bridge.check_fn(&name, &env)
        });
    }
    let support = if support_items.is_empty() {
        TokenStream::new()
    } else {
        let uses = if bridges.is_empty() {
            TokenStream::new()
        } else {
            quote! { use crate::types; }
        };
        quote! {
            /// What the functions share: list helpers and the bridges to the languages.
            mod support {
                #uses

                #(#support_items)*
            }
        }
    };
    let body = quote! { #(#items)* };
    let types_use = if body.to_string().contains("types ::") {
        quote! { use crate::types; }
    } else {
        TokenStream::new()
    };
    let tests = if tests.is_empty() {
        TokenStream::new()
    } else {
        quote! {
            /// The examples of the specification, one test each.
            #[cfg(test)]
            mod tests {
                use super::*;

                #(#tests)*
            }
        }
    };
    quote! {
        #![doc = #summary]
        #![doc = ""]
        #![doc = #overflow]
        #![allow(
            clippy::ptr_arg,
            clippy::bool_comparison,
            clippy::collapsible_else_if,
            clippy::if_same_then_else,
            clippy::match_like_matches_macro,
            clippy::needless_bool,
            clippy::nonminimal_bool,
            clippy::redundant_clone,
            clippy::single_match,
            clippy::unnecessary_fold
        )]

        #types_use

        #fault_items

        #body

        #support

        #tests
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(functions: &str, types: &str) -> String {
        format!(
            r#"{{ "system": {{ "id": "s", "name": "S", "generation": {{ "target": "rust" }} }},
                 "types": [{types}],
                 "functions": [{functions}] }}"#
        )
    }

    fn refusals(text: &str) -> Vec<Diagnostic> {
        let spec = crate::load::load("s.jsonc", text).expect("loads");
        check(&spec)
    }

    fn module(text: &str) -> String {
        let spec = crate::load::load("s.jsonc", text).expect("loads");
        let d = check(&spec);
        assert!(d.is_empty(), "{d:?}");
        emit(&spec).to_string()
    }

    #[test]
    fn arithmetic_is_checked_and_the_function_says_it_can_fault() {
        let text = spec(
            r#"{ "id": "next", "params": [{ "name": "n", "type": "u64" }], "returns": "u64", "body": "n + 1",
                 "examples": [{ "args": { "n": 1 }, "result": 2 }] }"#,
            "",
        );
        let out = module(&text);
        assert!(out.contains("checked_add"), "{out}");
        assert!(out.contains("Result < u64 , Fault >"), "{out}");
        assert!(out.contains("fn next_example_1"), "{out}");
    }

    #[test]
    fn a_function_without_arithmetic_keeps_its_plain_type() {
        let text = spec(
            r#"{ "id": "both", "params": [{ "name": "a", "type": "bool" }, { "name": "b", "type": "bool" }],
                 "returns": "bool", "body": "a && b",
                 "examples": [{ "args": { "a": true, "b": false }, "result": false }] }"#,
            "",
        );
        let out = module(&text);
        assert!(
            out.contains("pub fn both (a : bool , b : bool) -> bool"),
            "{out}"
        );
        assert!(out.contains("assert ! (! both (a , b))"), "{out}");
    }

    #[test]
    fn unsupported_types_and_recursion_are_refused() {
        let text = spec(
            r#"{ "id": "half", "params": [{ "name": "x", "type": "f64" }], "returns": "f64", "body": "x",
                 "examples": [{ "args": { "x": 1 }, "result": 1 }] }"#,
            "",
        );
        let d = refusals(&text);
        assert!(
            d.iter()
                .any(|d| d.code == FUNCTION_UNSUPPORTED && d.message.contains("f64")),
            "{d:?}"
        );
        let text = spec(
            r#"{ "id": "loop", "params": [{ "name": "x", "type": "bool" }], "returns": "bool", "body": "call loop(x)",
                 "examples": [{ "args": { "x": true }, "result": true }] }"#,
            "",
        );
        let d = refusals(&text);
        assert!(
            d.iter()
                .any(|d| d.code == FUNCTION_INVALID && d.message.contains("calls itself")),
            "{d:?}"
        );
    }

    #[test]
    fn a_body_that_does_not_type_is_refused() {
        let text = spec(
            r#"{ "id": "bad", "params": [{ "name": "x", "type": "bool" }], "returns": "bool", "body": "x + 1",
                 "examples": [{ "args": { "x": true }, "result": true }] }"#,
            "",
        );
        let d = refusals(&text);
        assert!(d.iter().any(|d| d.code == FUNCTION_INVALID), "{d:?}");
    }

    #[test]
    fn a_timestamp_is_the_prelude_timestamp_and_computes_as_its_i64() {
        let text = spec(
            r#"{ "id": "later", "params": [{ "name": "at", "type": "timestamp" }, { "name": "by", "type": "i64" }],
                 "returns": "timestamp", "body": "at + by",
                 "examples": [{ "args": { "at": 1000, "by": 5 }, "result": 1005 }] },
               { "id": "due", "params": [{ "name": "at", "type": "timestamp" }, { "name": "now", "type": "timestamp" }],
                 "returns": "bool", "body": "at <= now",
                 "examples": [{ "args": { "at": 1000, "now": 1000 }, "result": true }] },
               { "id": "millis", "params": [{ "name": "at", "type": "timestamp" }], "returns": "i64", "body": "at",
                 "examples": [{ "args": { "at": 7 }, "result": 7 }] }"#,
            "",
        );
        let out = module(&text);
        // Taken by value: Timestamp is Copy.
        assert!(
            out.contains("at : crate :: prelude :: Timestamp , by : i64"),
            "{out}"
        );
        assert!(
            out.contains("Result < crate :: prelude :: Timestamp , Fault >"),
            "{out}"
        );
        assert!(out.contains("at . 0 . checked_add (by)"), "{out}");
        assert!(out.contains("at . 0 <= now . 0"), "{out}");
        assert!(out.contains("-> i64 { at . 0 }"), "{out}");
        assert!(
            out.contains("crate :: prelude :: Timestamp (1000)"),
            "{out}"
        );
        assert!(!out.contains("types ::"), "{out}");
    }

    #[test]
    fn dividing_a_timestamp_is_refused() {
        let text = spec(
            r#"{ "id": "half", "params": [{ "name": "at", "type": "timestamp" }], "returns": "timestamp", "body": "at / 2",
                 "examples": [{ "args": { "at": 4 }, "result": 2 }] }"#,
            "",
        );
        let d = refusals(&text);
        assert!(
            d.iter().any(
                |d| d.code == FUNCTION_UNSUPPORTED && d.message.contains("divides a timestamp")
            ),
            "{d:?}"
        );
    }

    const ROLE: &str = r#"{ "id": "role-id", "name": "Role", "kind": "id", "repr": "string" }"#;

    #[test]
    fn a_set_is_a_btreeset_that_list_operations_take() {
        let text = spec(
            r#"{ "id": "holds", "params": [{ "name": "roles", "type": "set<string>" }, { "name": "r", "type": "string" }],
                 "returns": "bool", "body": "any(x in roles, x == r) && roles != []",
                 "examples": [{ "args": { "roles": ["a", "b"], "r": "b" }, "result": true }] },
               { "id": "distinct", "params": [{ "name": "items", "type": "list<string>" }], "returns": "set<string>",
                 "body": "fold(r in sortBy(r in items, r), acc = [], if any(a in acc, a == r) then acc else concat(acc, [r]))",
                 "examples": [{ "args": { "items": ["b", "a", "b"] }, "result": ["a", "b"] }] },
               { "id": "nobody", "params": [], "returns": "set<string>", "body": "[]",
                 "examples": [{ "args": {}, "result": [] }] },
               { "id": "one", "params": [], "returns": "set<string>", "body": "['x']",
                 "examples": [{ "args": {}, "result": ["x"] }] },
               { "id": "names", "params": [{ "name": "roles", "type": "set<role-id>" }], "returns": "list<string>",
                 "body": "roles", "examples": [{ "args": { "roles": ["a"] }, "result": ["a"] }] },
               { "id": "plus", "params": [{ "name": "roles", "type": "set<string>" }], "returns": "list<string>",
                 "body": "concat(roles, ['z'])", "examples": [{ "args": { "roles": ["a"] }, "result": ["a", "z"] }] }"#,
            ROLE,
        );
        let out = module(&text);
        assert!(
            out.contains("roles : & std :: collections :: BTreeSet < String >"),
            "{out}"
        );
        assert!(out.contains("roles . iter () . any"), "{out}");
        assert!(out.contains("! roles . is_empty ()"), "{out}");
        assert!(
            out.contains(". into_iter () . collect :: < std :: collections :: BTreeSet < _ > > ()"),
            "{out}"
        );
        assert!(
            out.contains("std :: collections :: BTreeSet :: < String > :: new ()"),
            "{out}"
        );
        assert!(
            out.contains("std :: collections :: BTreeSet :: from ([\"x\" . to_string ()])"),
            "{out}"
        );
        assert!(out.contains("map (| item_ | item_ . 0)"), "{out}");
        assert!(out.contains("support :: concat"), "{out}");
        // Examples write sets as BTreeSets.
        assert!(
            out.contains(
                "std :: collections :: BTreeSet :: from ([\"a\" . to_string () , \"b\" . to_string ()])"
            ),
            "{out}"
        );
    }

    #[test]
    fn an_example_set_out_of_order_or_with_a_repeated_item_is_refused() {
        for items in [r#"["b", "a"]"#, r#"["a", "a"]"#] {
            let text = spec(
                &format!(
                    r#"{{ "id": "size", "params": [{{ "name": "s", "type": "set<string>" }}], "returns": "bool",
                         "body": "s == []", "examples": [{{ "args": {{ "s": {items} }}, "result": false }}] }}"#
                ),
                "",
            );
            let d = refusals(&text);
            assert!(
                d.iter().any(|d| d.code == FUNCTION_UNSUPPORTED
                    && d.message.contains("does not have item 2 after item 1")),
                "{d:?}"
            );
        }
    }

    const CONSTRAINED_ROLE: &str = r#"{ "id": "role", "name": "Role", "kind": "id", "repr": "string",
        "constraints": { "pattern": "^[a-z]+$", "maxLength": 8 } }"#;

    #[test]
    fn a_literal_where_a_set_of_constrained_ids_is_expected_is_built_as_one_when_it_meets_them() {
        let body = |role: &str| {
            spec(
                &format!(
                    r#"{{ "id": "anyone", "params": [{{ "name": "p", "type": "bool" }}], "returns": "set<role>",
                         "body": "if p then ['{role}'] else []",
                         "examples": [{{ "args": {{ "p": true }}, "result": ["{role}"] }}, {{ "args": {{ "p": false }}, "result": [] }}] }}"#
                ),
                CONSTRAINED_ROLE,
            )
        };
        let out = module(&body("public"));
        assert!(
            out.contains("vec ! [types :: Role (\"public\" . to_string ())]"),
            "{out}"
        );
        assert!(out.contains("Vec :: < types :: Role > :: new ()"), "{out}");
        // A literal that breaks a constraint stays refused.
        let d = refusals(&body("Public"));
        assert!(
            d.iter()
                .any(|d| d.code == FUNCTION_UNSUPPORTED
                    && d.message.contains("constrained type role")),
            "{d:?}"
        );
    }

    const GRANT: &str = r#"{ "id": "grant", "name": "Grant", "kind": "union", "variants": [
        { "name": "capability", "newtype": true, "fields": [{ "name": "value", "type": "string" }] },
        { "name": "object", "tuple": true, "fields": [{ "name": "object", "type": "string" }, { "name": "action", "type": "string" }] } ] }"#;

    #[test]
    fn a_tuple_variant_is_built_and_taken_apart_by_position() {
        let text = spec(
            r#"{ "id": "object-grant", "params": [{ "name": "o", "type": "string" }, { "name": "a", "type": "string" }],
                 "returns": "grant", "body": "grant.object{action: a, object: o}",
                 "examples": [{ "args": { "o": "post", "a": "read" }, "result": { "object": { "object": "post", "action": "read" } } }] },
               { "id": "grant-action", "params": [{ "name": "g", "type": "grant" }], "returns": "string",
                 "body": "match g { capability(name) => name, object{action} => action }",
                 "examples": [{ "args": { "g": { "object": { "object": "post", "action": "read" } } }, "result": "read" },
                              { "args": { "g": { "capability": "export" } }, "result": "export" }] }"#,
            GRANT,
        );
        let out = module(&text);
        assert!(
            out.contains("types :: Grant :: Object (o . clone () , a . clone ())"),
            "{out}"
        );
        assert!(
            out.contains("types :: Grant :: Object (_ , action) =>"),
            "{out}"
        );
        assert!(
            out.contains(
                "types :: Grant :: Object (\"post\" . to_string () , \"read\" . to_string ())"
            ),
            "{out}"
        );
    }

    const TENANT: &str = r#"{ "id": "tenant", "name": "Tenant", "kind": "struct",
        "fields": [{ "name": "members", "type": "map<string, list<string>>" }] }"#;

    fn fixture_spec(fixtures: &str, args: &str) -> String {
        format!(
            r#"{{ "system": {{ "id": "s", "name": "S", "generation": {{ "target": "rust" }} }},
                 "types": [{TENANT}],
                 "exampleFixtures": [{fixtures}],
                 "functions": [{{ "id": "held", "params": [{{ "name": "t", "type": "tenant" }}], "returns": "bool",
                   "body": "any(k in keys(t.members), k == 'a')",
                   "examples": [{{ "args": {{ "t": {args} }}, "result": true }}] }}] }}"#
        )
    }

    const ALPHA: &str =
        r#"{ "id": "alpha-tenant", "type": "tenant", "value": { "members": { "a": ["owner"] } } }"#;

    #[test]
    fn a_fixture_is_written_once_and_an_example_calls_it() {
        let out = module(&fixture_spec(ALPHA, r#"{ "fixture": "alpha-tenant" }"#));
        assert!(
            out.contains("fn fixture_alpha_tenant () -> types :: Tenant"),
            "{out}"
        );
        assert!(
            out.contains("let t : types :: Tenant = fixture_alpha_tenant ()"),
            "{out}"
        );
    }

    #[test]
    fn an_override_is_an_assignment_after_the_call() {
        let out = module(&fixture_spec(
            ALPHA,
            r#"{ "fixture": "alpha-tenant", "with": { "members.b": ["writer"] } }"#,
        ));
        assert!(
            out.contains("let mut fixture = fixture_alpha_tenant ()"),
            "{out}"
        );
        assert!(out.contains("fixture . members . insert"), "{out}");
    }

    #[test]
    fn an_unused_fixture_is_not_written() {
        let other = r#"{ "id": "beta-tenant", "type": "tenant", "value": { "members": {} } }"#;
        let out = module(&fixture_spec(
            &format!("{ALPHA}, {other}"),
            r#"{ "fixture": "alpha-tenant" }"#,
        ));
        assert!(!out.contains("fixture_beta_tenant"), "{out}");
    }

    #[test]
    fn a_fixture_that_is_not_declared_or_of_another_type_is_refused() {
        let d = refusals(&fixture_spec(ALPHA, r#"{ "fixture": "gamma" }"#));
        assert!(
            d.iter()
                .any(|d| d.code == FUNCTION_INVALID && d.message.contains("gamma")),
            "{d:?}"
        );
        let text = fixture_spec(
            r#"{ "id": "n", "type": "string", "value": "x" }"#,
            r#"{ "fixture": "n" }"#,
        );
        let d = refusals(&text);
        assert!(
            d.iter()
                .any(|d| d.code == FUNCTION_INVALID && d.message.contains("stands here")),
            "{d:?}"
        );
    }
}
