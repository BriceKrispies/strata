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
//!   the slice and `BTreeMap` operations; `format` is `format!`, its `{}` Display, its `{:?}`
//!   the generated types' derived Debug (which Exegra's debug rule mirrors; refused for a value
//!   holding a timestamp, which Exegra writes as a bare number) and its `{:json}`
//!   `serde_json::to_string`.
//! - The string operations: `startsWith` and `endsWith` are `str::starts_with`/`ends_with`,
//!   `length` the characters of a string (`chars().count()`) or the items of a list or set,
//!   `trim` is `str::trim` (Rust's White_Space, which Exegra's trim removes), `chars` the
//!   characters as strings, and `isAsciiLowercase` and the other class tests, `snakeCase` and
//!   `parseTime` (substrate's `parse_time`) are `support` helpers written as Exegra defines them.
//! - `eval` and `check` of a language call its generated module ([`crate::language`]) through an
//!   adapter that implements the module's environment traits over the environment struct.
//!
//! # Timestamps, sets, tuple variants, shared and boxed values, f64
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
//! - `shared<T>` is `Arc<T>`, and `T` to Exegra. Fields read through it (the `Arc` derefs), a
//!   match on one matches `Arc::as_ref`, and it is borrowed as `&T` the same way. Where a plain
//!   `T` is expected the value is cloned out (`<T as Clone>::clone(&arc)`); where a shared one is
//!   expected a plain value is shared with `Arc::new` and a shared one cloned (`Arc::clone`).
//!   Lists, options and map values of them convert item by item. An example writes it as a `T`.
//! - A boxed field (`Box<T>`, a field holding its own recursive group) is read and bound as the
//!   place in the box (`(*x.f)`, `(**b)`), and built with `Box::new`. An optional one
//!   (`Option<Box<T>>`) is not compiled.
//! - `f64` is `f64`. A number with a fraction is one; an integer stands for one (`as f64`), as in
//!   Exegra, where both are numbers. It compares and computes as Exegra's numbers do (IEEE
//!   doubles), and division or remainder by zero, an evaluation error in Exegra, is a fault. It
//!   is not a sort key (it has no total order) nor a set item.
//!
//! # Conversions between scalars
//!
//! Exegra's values are numbers, strings and bools: it passes an id where its representation is
//! expected and back, an integer where another integer type is, a string where a constrained id
//! is, and it checks neither ranges (but of arithmetic and of example values) nor a type's
//! constraints inside a function. The generated types hold less, so specc converts:
//!
//! - an id to its representation is `.0`, and to another scalar its representation converted;
//! - an integer to a wider integer type is `From`; to a narrower one it is checked with
//!   `try_from`, and one that does not fit is a `Fault` (`op: "as"`), where Exegra goes on with
//!   a number beyond the type;
//! - a value to an unconstrained id wraps it; to a constrained one it is checked with the
//!   type's `new`, and one that breaks the constraints is a `Fault` (`op: "new"`), where Exegra
//!   goes on. A string literal is checked by specc instead: one that meets the constraints is
//!   built directly, one that does not is refused (the check would fail on every run);
//! - a map lookup (`get`, `remove`) by a string in a map of constrained ids builds the key
//!   without the check: it is never stored, and a key that breaks the constraints is one no map
//!   holds, so the lookup finds nothing, as in Exegra;
//! - lists, sets, options and maps convert item by item (keys too), and collect the first
//!   failure of a checked item.
//!
//! A function that does a conversion that can fail can fault, and says so in its type; specc
//! finds them by generating each function once before deciding the signatures.
//!
//! Literals, branches and keys are built in the type expected of them: a list literal, `if`,
//! `match`, `let` or `insert` where a `list<role-id>`, `set<role-id>` or `map<role-id, T>` is
//! expected builds its string literals as `RoleId`s, and branches of other integer types as the
//! expected one. `insert` of a key that is not the map's id (a string, another id) gives a map
//! of the representation, as Exegra's keys are. A number sort key is compared as an `i128`, which
//! holds every integer type, so keys of different integer types compare as Exegra's numbers do;
//! a string key (an id's too) as its string.
//!
//! # Ownership
//!
//! A parameter of a `Copy` type (bool, the integers, f64, timestamp, options of them) is taken by
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
//! A function that can fault (it does such arithmetic or a checked conversion, or calls a
//! function that can) says so in
//! its type: it returns `Result<R, Fault>` when it declares no errors, and
//! `Result<R, Failure<E>>` (`Failure::Error(E)` or `Failure::Fault(Fault)`) when it does. A
//! function that cannot fault keeps the plain `R` or `Result<R, E>`.
//!
//! # Not compiled (refused)
//!
//! `duration`, `bytes`, `uuid` and `json`; `print`, the unit value `()` and structural
//! recursion; dividing a timestamp; `{:?}` of a value holding a timestamp; an f64 sort key or
//! set item; `==` between a set and a list other than `[]`, or a shared and a plain value; an
//! example set not written in order or with an item twice; an optional boxed field; an
//! optional field of an option type; a string literal built as a constrained id whose
//! constraints it breaks; names that become the same Rust name, or the name of a function.

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
    // Conversions that can fail (an integer narrowed, a constrained id built from a value) are
    // found by generating each function once: those that use a fault can fault too.
    let names: BTreeSet<String> = reserved.keys().cloned().collect();
    let mut converting = Vec::new();
    for f in &spec.functions {
        let t = &typed[&f.id];
        let mut gen = rust::Gen::new(spec, &sigs, &sigs[&f.id], names.clone());
        if gen.function(TokenStream::new(), &t.lets, &t.body).is_ok() && gen.faults() {
            converting.push(f.id.clone());
        }
    }
    for id in converting {
        typed.get_mut(&id).expect("typed").arith = true;
    }
    // Which functions can fault: their own arithmetic or conversions, or a call of one that can.
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
        (None, true) => origin.push_str("; an integer overflow, a division by zero or a failed conversion is a `Fault`."),
        (Some(e), true) => origin.push_str(&format!(
            "; fails with `{e}` (`Failure::Error`), and an integer overflow, a division by zero or a failed conversion is `Failure::Fault`."
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
            /// evaluation error; or a value the generated types cannot hold where Exegra passes
            /// it on: an integer out of the range of the type it is converted to, a value that
            /// breaks the constraints of the id it is built as.
            #[derive(Debug, Clone, Copy, PartialEq, Eq)]
            pub struct Fault {
                /// The operator: `+`, `-`, `*`, `/` or `%`; `as` for an integer conversion,
                /// `new` for a constrained id built from a value.
                pub op: &'static str,
            }

            impl std::fmt::Display for Fault {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    match self.op {
                        "as" => write!(f, "an integer out of the range of the type it is converted to"),
                        "new" => write!(f, "a value that breaks the constraints of the id it is built as"),
                        op => write!(f, "integer overflow or division by zero in {op}"),
                    }
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
            clippy::unnecessary_fold,
            clippy::float_cmp
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
            r#"{ "id": "half", "params": [{ "name": "x", "type": "bytes" }], "returns": "bytes", "body": "x",
                 "examples": [{ "args": { "x": "AA==" }, "result": "AA==" }] }"#,
            "",
        );
        let d = refusals(&text);
        assert!(
            d.iter()
                .any(|d| d.code == FUNCTION_UNSUPPORTED && d.message.contains("bytes")),
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

    const REC: &str = r#"{"id": "rec", "name": "Rec", "kind": "struct", "fields": [{"name": "id", "type": "u64"}, {"name": "name", "type": "string"}]}, {"id": "store", "name": "Store", "kind": "struct", "fields": [{"name": "rows", "type": "map<u64, shared<rec>>"}, {"name": "top", "type": "shared<rec>"}]}"#;

    #[test]
    fn a_shared_value_is_read_through_its_arc_and_built_with_arc_new() {
        let out = module(&spec(
            r#"{"id": "top-name", "params": [{"name": "s", "type": "store"}], "returns": "string", "body": "s.top.name", "examples": [{"args": {"s": {"rows": {}, "top": {"id": 1, "name": "a"}}}, "result": "a"}]}, {"id": "top", "params": [{"name": "s", "type": "store"}], "returns": "rec", "body": "s.top", "examples": [{"args": {"s": {"rows": {}, "top": {"id": 1, "name": "a"}}}, "result": {"id": 1, "name": "a"}}]}, {"id": "row", "params": [{"name": "s", "type": "store"}, {"name": "id", "type": "u64"}], "returns": "option<rec>", "body": "get(s.rows, id)", "examples": [{"args": {"s": {"rows": {"2": {"id": 2, "name": "b"}}, "top": {"id": 1, "name": "a"}}, "id": 2}, "result": {"id": 2, "name": "b"}}, {"args": {"s": {"rows": {}, "top": {"id": 1, "name": "a"}}, "id": 2}, "result": null}]}, {"id": "with-top", "params": [{"name": "s", "type": "store"}, {"name": "r", "type": "rec"}], "returns": "store", "body": "store{rows: s.rows, top: r}", "examples": [{"args": {"s": {"rows": {}, "top": {"id": 1, "name": "a"}}, "r": {"id": 2, "name": "b"}}, "result": {"rows": {}, "top": {"id": 2, "name": "b"}}}]}"#,
            REC,
        ));
        assert!(out.contains("s . top . name . clone ()"), "{out}");
        assert!(
            out.contains("< types :: Rec as Clone > :: clone (& s . top)"),
            "{out}"
        );
        assert!(
            out.contains("map (| item_ | < types :: Rec as Clone > :: clone (& item_))"),
            "{out}"
        );
        assert!(
            out.contains("top : std :: sync :: Arc :: new (r . clone ())"),
            "{out}"
        );
        // An example writes a shared<T> as a T.
        assert!(
            out.contains("top : std :: sync :: Arc :: new (types :: Rec {"),
            "{out}"
        );
    }

    const FT: &str = r#"{"id": "ft", "name": "Field type", "kind": "union", "variants": [{"name": "str"}, {"name": "list", "newtype": true, "fields": [{"name": "value", "type": "ft"}]}]}"#;

    #[test]
    fn a_boxed_field_is_bound_as_the_value_in_its_box() {
        let out = module(&spec(
            r#"{"id": "list-of-str", "params": [{"name": "t", "type": "ft"}], "returns": "bool", "body": "match t { list(inner) => match inner { str => true, _ => false }, _ => false }", "examples": [{"args": {"t": {"list": "str"}}, "result": true}, {"args": {"t": {"list": {"list": "str"}}}, "result": false}, {"args": {"t": "str"}, "result": false}]}, {"id": "wrap", "params": [{"name": "t", "type": "ft"}], "returns": "ft", "body": "ft.list(t)", "examples": [{"args": {"t": "str"}, "result": {"list": "str"}}]}"#,
            FT,
        ));
        assert!(out.contains("types :: Ft :: List (inner) =>"), "{out}");
        assert!(
            out.contains("match & (** inner)") || out.contains("match & (* * inner)"),
            "{out}"
        );
        assert!(
            out.contains("types :: Ft :: List (Box :: new (t . clone ()))"),
            "{out}"
        );
    }

    #[test]
    fn the_string_operations_are_rust_s_own_and_the_support_helpers() {
        let out = module(&spec(
            r#"{"id": "text-checks", "params": [{"name": "s", "type": "string"}], "returns": "bool", "body": "startsWith(s, 'ab') && endsWith(s, 'z') && isAsciiLowercase(s)", "examples": [{"args": {"s": "abz"}, "result": true}, {"args": {"s": "abZ"}, "result": false}]}, {"id": "sizes", "params": [{"name": "s", "type": "string"}, {"name": "xs", "type": "list<string>"}], "returns": "u64", "body": "length(s) + length(xs)", "examples": [{"args": {"s": "héllo", "xs": ["a"]}, "result": 6}]}, {"id": "tidy", "params": [{"name": "s", "type": "string"}], "returns": "list<string>", "body": "chars(trim(s))", "examples": [{"args": {"s": " ab\n"}, "result": ["a", "b"]}]}, {"id": "snake", "params": [{"name": "s", "type": "string"}], "returns": "string", "body": "snakeCase(s)", "examples": [{"args": {"s": "PostAuthor"}, "result": "post_author"}]}, {"id": "millis", "params": [{"name": "s", "type": "string"}], "returns": "i64", "body": "match parseTime(s) { Ok(ms) => ms, Err(e) => 0 - 1 }", "examples": [{"args": {"s": "1970-01-02"}, "result": 86400000}, {"args": {"s": "1970-01-01T00:01Z"}, "result": 60000}, {"args": {"s": "x"}, "result": -1}]}"#,
            "",
        ));
        assert!(
            out.contains("s . as_str () . starts_with (\"ab\")"),
            "{out}"
        );
        assert!(out.contains("s . as_str () . ends_with (\"z\")"), "{out}");
        assert!(
            out.contains("support :: every_char (s . as_str () , char :: is_ascii_lowercase)"),
            "{out}"
        );
        assert!(out.contains(". chars () . count ()"), "{out}");
        assert!(out.contains("u64 :: try_from (xs . len ())"), "{out}");
        assert!(out.contains(". trim () . to_string ()"), "{out}");
        assert!(
            out.contains(". chars () . map (String :: from) . collect :: < Vec < _ >> ()"),
            "{out}"
        );
        assert!(
            out.contains("fn snake_case (name : & str) -> String"),
            "{out}"
        );
        assert!(
            out.contains("fn parse_time (text : & str) -> Result < i64 , String >"),
            "{out}"
        );
        assert!(out.contains("fn every_char"), "{out}");
    }

    const NUM: &str = r#"{"id": "num", "name": "Number", "kind": "union", "variants": [{"name": "pos-int", "newtype": true, "fields": [{"name": "value", "type": "u64"}]}, {"name": "float", "newtype": true, "fields": [{"name": "value", "type": "f64"}]}]}"#;

    #[test]
    fn an_f64_is_bound_formatted_compared_and_divided() {
        let out = module(&spec(
            r#"{"id": "num-text", "params": [{"name": "n", "type": "num"}], "returns": "string", "body": "match n { pos-int(u) => format('{}', u), float(x) => format('{} {:json} {:?}', x, x, n) }", "examples": [{"args": {"n": {"float": 2.5}}, "result": "2.5 2.5 Float(2.5)"}, {"args": {"n": {"float": 1}}, "result": "1 1.0 Float(1.0)"}, {"args": {"n": {"pos-int": 7}}, "result": "7"}]}, {"id": "half", "params": [{"name": "x", "type": "f64"}], "returns": "f64", "body": "x / 2", "examples": [{"args": {"x": 3}, "result": 1.5}]}, {"id": "bigger", "params": [{"name": "x", "type": "f64"}], "returns": "bool", "body": "x > 1.5", "examples": [{"args": {"x": 2}, "result": true}]}"#,
            NUM,
        ));
        assert!(out.contains("types :: Num :: Float (x) =>"), "{out}");
        assert!(
            out.contains("format ! (\"{x} {} {:?}\" , serde_json :: to_string (x) . unwrap_or_default () , n)"),
            "{out}"
        );
        assert!(out.contains("let divisor_ : f64 = 2.0 ;"), "{out}");
        assert!(out.contains("Result < f64 , Fault >"), "{out}");
        assert!(out.contains("x > 1.5"), "{out}");
        // Example values of f64 are f64 literals.
        assert!(out.contains("types :: Num :: Float (2.5)"), "{out}");
    }

    #[test]
    fn an_integer_widens_with_from() {
        let wide = r#"{ "id": "wide", "params": [{ "name": "x", "type": "u32" }], "returns": "u64", "body": "x",
                 "examples": [{ "args": { "x": 3 }, "result": 3 }] }"#;
        let out = module(&spec(wide, ""));
        assert!(out.contains("u64 :: from (x)"), "{out}");
        assert!(out.contains("pub fn wide (x : u32) -> u64"), "{out}");
    }

    const CONVERT: &str = r#"{"id": "role", "name": "Role", "kind": "id", "repr": "string", "constraints": {"pattern": "^[a-z][a-z0-9_]*$", "maxLength": 64}}, {"id": "rid", "name": "Record id", "kind": "id", "repr": "u64"}, {"id": "key", "name": "Key", "kind": "union", "variants": [{"name": "rec", "newtype": true, "fields": [{"name": "value", "type": "rid"}]}, {"name": "num", "newtype": true, "fields": [{"name": "value", "type": "i64"}]}]}"#;

    #[test]
    fn an_integer_narrows_with_try_from_and_the_function_can_fault() {
        let out = module(&spec(
            r#"{"id": "narrow", "params": [{"name": "x", "type": "i64"}], "returns": "u64", "body": "x", "examples": [{"args": {"x": 3}, "result": 3}]}, {"id": "twice", "params": [{"name": "x", "type": "i64"}], "returns": "u64", "body": "call narrow(x)", "examples": [{"args": {"x": 3}, "result": 3}]}, {"id": "rid-number", "params": [{"name": "r", "type": "rid"}], "returns": "i64", "body": "r", "examples": [{"args": {"r": 4}, "result": 4}]}"#,
            CONVERT,
        ));
        assert!(
            out.contains("u64 :: try_from (x) . map_err (| _ | Fault { op : \"as\" }) ?"),
            "{out}"
        );
        assert!(
            out.contains("pub fn narrow (x : i64) -> Result < u64 , Fault >"),
            "{out}"
        );
        // A caller of a function that can fault can fault too.
        assert!(out.contains("narrow (x) ?"), "{out}");
        assert!(
            out.contains("pub fn twice (x : i64) -> Result < u64 , Fault >"),
            "{out}"
        );
        // An id over u64 where an i64 is expected: its representation, narrowed.
        assert!(out.contains("i64 :: try_from (r . clone () . 0)"), "{out}");
    }

    #[test]
    fn a_constrained_id_from_a_value_is_checked_and_map_keys_convert() {
        let out = module(&spec(
            r#"{"id": "make-role", "params": [{"name": "s", "type": "string"}], "returns": "role", "body": "s", "examples": [{"args": {"s": "editor"}, "result": "editor"}]}, {"id": "role-names", "params": [{"name": "m", "type": "map<role, string>"}], "returns": "map<string, string>", "body": "m", "examples": [{"args": {"m": {"editor": "x"}}, "result": {"editor": "x"}}]}, {"id": "role-map", "params": [{"name": "m", "type": "map<string, string>"}], "returns": "map<role, string>", "body": "m", "examples": [{"args": {"m": {"editor": "x"}}, "result": {"editor": "x"}}]}, {"id": "has-role", "params": [{"name": "m", "type": "map<role, string>"}, {"name": "s", "type": "string"}], "returns": "bool", "body": "get(m, s) != null", "examples": [{"args": {"m": {"editor": "x"}, "s": "editor"}, "result": true}, {"args": {"m": {}, "s": "Not A Role"}, "result": false}]}, {"id": "scoped", "params": [{"name": "m", "type": "map<role, string>"}], "returns": "map<string, string>", "body": "insert(m, '$id', 'y')", "examples": [{"args": {"m": {"editor": "x"}}, "result": {"$id": "y", "editor": "x"}}]}, {"id": "builtin-roles", "params": [], "returns": "map<role, string>", "body": "insert(insert({}, 'owner', 'Owner'), 'public', 'Everyone')", "examples": [{"args": {}, "result": {"owner": "Owner", "public": "Everyone"}}]}"#,
            CONVERT,
        ));
        assert!(
            out.contains(
                "types :: Role :: new (s . clone ()) . map_err (| _ | Fault { op : \"new\" }) ?"
            ),
            "{out}"
        );
        assert!(
            out.contains("(| (key_ , item_) | (key_ . 0 , item_))"),
            "{out}"
        );
        assert!(
            out.contains("Ok :: < _ , Fault > ((types :: Role :: new (key_)"),
            "{out}"
        );
        assert!(
            out.contains(
                "collect :: < Result < std :: collections :: BTreeMap < _ , _ > , _ >> () ?"
            ),
            "{out}"
        );
        // A lookup builds the key without the check: it finds nothing, as Exegra's does.
        assert!(
            out.contains("m . get (& types :: Role (s . clone ())) . is_some ()"),
            "{out}"
        );
        assert!(
            out.contains("pub fn has_role (m : & std :: collections :: BTreeMap < types :: Role , String > , s : & String) -> bool"),
            "{out}"
        );
        // A key of the representation makes a map of the representation.
        assert!(
            out.contains("map_ . insert (\"$id\" . to_string () , \"y\" . to_string ())"),
            "{out}"
        );
        // Literal keys where ids are expected are built (and checked) as ids.
        assert!(
            out.contains("types :: Role (\"owner\" . to_string ())"),
            "{out}"
        );
        assert!(
            out.contains("pub fn builtin_roles () -> std :: collections :: BTreeMap"),
            "{out}"
        );
    }

    #[test]
    fn a_number_sort_key_of_mixed_integer_types_is_an_i128() {
        let out = module(&spec(
            r#"{"id": "key-order", "params": [{"name": "xs", "type": "list<key>"}], "returns": "list<key>", "body": "sortBy(x in xs, match x { rec(r) => r, num(i) => i })", "examples": [{"args": {"xs": [{"rec": 5}, {"num": -1}, {"num": 3}]}, "result": [{"num": -1}, {"num": 3}, {"rec": 5}]}]}"#,
            CONVERT,
        ));
        assert!(out.contains("i128 :: from (r . clone () . 0)"), "{out}");
        assert!(out.contains("i128 :: from (* i)"), "{out}");
    }

    #[test]
    fn a_let_bound_count_converts_where_a_u64_field_takes_it() {
        let text = spec(
            r#"{ "id": "count-into", "params": [{ "name": "xs", "type": "list<string>" }], "returns": "holder",
                 "let": [{ "name": "checks", "expr": "fold(c in xs, n = 0, n + 1)" }],
                 "body": "holder{n: checks}",
                 "examples": [{ "args": { "xs": ["a", "b"] }, "result": { "n": 2 } }] }"#,
            r#"{ "id": "holder", "name": "Holder", "kind": "struct", "fields": [{ "name": "n", "type": "u64" }] }"#,
        );
        let out = module(&text);
        // The let holds Exegra's untyped number as an i64; the u64 field converts it.
        assert!(out.contains("let checks : i64 ="), "{out}");
        assert!(
            out.contains("n : u64 :: try_from (checks) . map_err (| _ | Fault { op : \"as\" }) ?"),
            "{out}"
        );
        assert!(
            out.contains(
                "pub fn count_into (xs : & Vec < String >) -> Result < types :: Holder , Fault >"
            ),
            "{out}"
        );
    }

    #[test]
    fn print_is_refused_as_not_compiled() {
        let d = refusals(&spec(
            r#"{ "id": "p", "params": [{ "name": "s", "type": "string" }], "returns": "string", "body": "print(l, s)",
                 "examples": [{ "args": { "s": "a" }, "result": "a" }] }"#,
            "",
        ));
        assert!(
            d.iter()
                .any(|d| d.code == FUNCTION_UNSUPPORTED && d.message.contains("print")),
            "{d:?}"
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
