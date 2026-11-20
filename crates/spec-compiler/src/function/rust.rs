//! Rust for function bodies.
//!
//! Every expression is emitted as a [`Code`]: its tokens, the precedence they bind at (so that
//! parentheses appear only where Rust needs them), and its [`Form`], which says how the value is
//! held:
//!
//! - `Temp`: an owned value nobody else holds (a literal, a call's result);
//! - `Place`: a place of type `T` (a let, a copied parameter, a field of a place);
//! - `Ref`: a variable of type `&T` (a parameter taken by reference, an item bound by a list
//!   operation, a binding of a match on a borrowed value);
//! - `OptRef`: an `Option<&T>` (what `get` and `find` give before anything needs to own it).
//!
//! A value is owned only where it must be (a constructor field, a list item, a returned value):
//! a place is cloned there, a copy type copied. Everywhere else places are borrowed.

use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::{Ident, Literal, TokenStream};
use quote::{format_ident, quote};

use super::syntax::{uses, Arm, BinOp, Expr, Kind, Op, Pattern, Res, UnOp};
use super::typing::{
    cases, class, format_parts, int_kind, is_number, json_writable, match_on, struct_fields,
    unshared, unsupported, variants, Class, Hole, MatchOn, Refusal, Sig, Ty, DURATION, TIMESTAMP,
};
use crate::ir::Spec;
use crate::naming;
use crate::resolve::Builtin;

/// How tightly an expression binds, lowest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Prec {
    /// `if`, `match`, a block, `return`.
    Block,
    Or,
    And,
    Cmp,
    Add,
    Prefix,
    /// Paths, literals, calls, method calls, fields, `?`, macros, parenthesised expressions.
    Postfix,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Form {
    Temp,
    Place,
    Ref,
    OptRef,
}

#[derive(Debug, Clone)]
pub(crate) struct Code {
    t: TokenStream,
    prec: Prec,
    form: Form,
    /// Contains `?` or `return`, so it cannot go into a closure.
    exits: bool,
    /// A bare identifier.
    ident: bool,
    /// A struct literal at its top level, which Rust refuses unparenthesised in a condition.
    brace: bool,
    /// An `if` expression, which an `else` takes without braces.
    is_if: bool,
}

impl Code {
    fn new(t: TokenStream, prec: Prec) -> Self {
        Code {
            t,
            prec,
            form: Form::Temp,
            exits: false,
            ident: false,
            brace: false,
            is_if: false,
        }
    }

    fn exits(mut self, exits: bool) -> Self {
        self.exits = self.exits || exits;
        self
    }

    fn form(mut self, form: Form) -> Self {
        self.form = form;
        self
    }

    /// The tokens, parenthesised unless they bind at least at `p`.
    fn at(&self, p: Prec) -> TokenStream {
        let t = &self.t;
        if self.prec >= p {
            t.clone()
        } else {
            quote! { (#t) }
        }
    }

    /// The tokens as a method receiver or field owner.
    fn recv(&self) -> TokenStream {
        self.at(Prec::Postfix)
    }

    /// The tokens as the condition of an `if` or the scrutinee of a `match`.
    fn cond(&self) -> TokenStream {
        let t = &self.t;
        if self.brace {
            quote! { (#t) }
        } else {
            t.clone()
        }
    }
}

/// A local name and how it is held.
#[derive(Debug, Clone)]
struct Binding {
    name: String,
    ident: Ident,
    ty: Ty,
    form: Form,
    /// How many `*` reach the value: 1 for a `Box<T>` bound by value, 2 for a `&Box<T>` (a
    /// boxed field bound by a match), 0 otherwise.
    deref: usize,
}

/// True when the generated Rust type is `Copy`.
pub(crate) fn is_copy(t: &Ty) -> bool {
    match t {
        Ty::Bool | Ty::Int(_) | Ty::Number | Ty::Float | Ty::Wide | Ty::Unit => true,
        // prelude::Timestamp and prelude::DurationMs are Copy.
        Ty::Alias(..) if t.is_prelude_number() => true,
        Ty::Option(of) => is_copy(of),
        _ => false,
    }
}

/// True when a parameter of this type is taken by reference.
pub(crate) fn by_ref(t: &Ty) -> bool {
    !is_copy(t)
}

/// The path of a declared type, or of the prelude's `Timestamp` for the built-in `timestamp`.
pub(crate) fn type_path(id: &str) -> TokenStream {
    if id == TIMESTAMP {
        return quote! { crate::prelude::Timestamp };
    }
    if id == DURATION {
        return quote! { crate::prelude::DurationMs };
    }
    let name = naming::ident(&naming::pascal(id).unwrap_or_default());
    quote! { types::#name }
}

fn variant_path(id: &str, variant: &str) -> TokenStream {
    let ty = type_path(id);
    let v = naming::ident(&naming::pascal(variant).unwrap_or_default());
    quote! { #ty::#v }
}

pub(crate) fn snake_ident(name: &str) -> Ident {
    naming::ident(&naming::snake(name).unwrap_or_default())
}

/// The Rust type of a concrete type.
pub(crate) fn rust_ty(t: &Ty) -> Result<TokenStream, Refusal> {
    Ok(match t {
        Ty::Bool => quote! { bool },
        Ty::Str => quote! { String },
        Ty::Int(b) => {
            let i = format_ident!("{}", b.as_str());
            quote! { #i }
        }
        Ty::Alias(id, _) | Ty::Enum(id) | Ty::Struct(id) | Ty::Union(id) => type_path(id),
        Ty::List(of) => {
            let of = rust_ty(of)?;
            quote! { Vec<#of> }
        }
        Ty::Set(of) => {
            let of = rust_ty(of)?;
            quote! { std::collections::BTreeSet<#of> }
        }
        Ty::Float => quote! { f64 },
        Ty::Uuid => quote! { uuid::Uuid },
        Ty::Wide => quote! { i128 },
        Ty::Unit => quote! { () },
        Ty::Shared(of) => {
            let of = rust_ty(of)?;
            quote! { std::sync::Arc<#of> }
        }
        Ty::Option(of) => {
            let of = rust_ty(of)?;
            quote! { Option<#of> }
        }
        Ty::Map(k, v) => {
            let (k, v) = (rust_ty(k)?, rust_ty(v)?);
            quote! { std::collections::BTreeMap<#k, #v> }
        }
        Ty::Result(o, e) => {
            let (o, e) = (rust_ty(o)?, rust_ty(e)?);
            quote! { Result<#o, #e> }
        }
        Ty::Number | Ty::Unknown | Ty::Never => {
            return Err(unsupported(
                "the type of an expression cannot be told from where it is used (an empty list, an empty map or null with nothing to give it a type)",
            ))
        }
    })
}

/// `t` with each `Number` and `Unknown` replaced by what `hint` has at that position.
pub(crate) fn concretize(t: &Ty, hint: Option<&Ty>) -> Ty {
    match (t, hint) {
        (Ty::Number, Some(h)) => match (int_kind(h), h) {
            (Some(b), _) => Ty::Int(b),
            (None, Ty::Float) => Ty::Float,
            (None, Ty::Wide) => Ty::Wide,
            (None, Ty::Option(inner)) => concretize(t, Some(inner)),
            _ => Ty::Int(Builtin::I64),
        },
        (Ty::Number, None) => Ty::Int(Builtin::I64),
        (Ty::Unknown | Ty::Never, Some(h)) => h.clone(),
        // A set and a list take each other's items; each keeps its own form.
        (Ty::List(a), Some(Ty::List(b) | Ty::Set(b))) => Ty::list(concretize(a, Some(b))),
        (Ty::Set(a), Some(Ty::List(b) | Ty::Set(b))) => Ty::set(concretize(a, Some(b))),
        // A shared value stays shared; what it holds takes the hint's (shared or not).
        (Ty::Shared(a), Some(h)) => Ty::shared(concretize(a, Some(unshared(h)))),
        (Ty::Shared(a), None) => Ty::shared(concretize(a, None)),
        (Ty::Option(a), Some(Ty::Option(b))) => Ty::option(concretize(a, Some(b))),
        (Ty::Map(k, v), Some(Ty::Map(hk, hv))) => {
            Ty::map(concretize(k, Some(hk)), concretize(v, Some(hv)))
        }
        (Ty::Result(o, e), Some(Ty::Result(ho, he))) => Ty::Result(
            Box::new(concretize(o, Some(ho))),
            Box::new(concretize(e, Some(he))),
        ),
        (_, Some(Ty::Option(inner))) if !matches!(t, Ty::Option(_)) => concretize(t, Some(inner)),
        (Ty::List(a), _) => Ty::list(concretize(a, None)),
        (Ty::Set(a), _) => Ty::set(concretize(a, None)),
        (Ty::Option(a), _) => Ty::option(concretize(a, None)),
        (Ty::Map(k, v), _) => Ty::map(concretize(k, None), concretize(v, None)),
        _ => t.clone(),
    }
}

/// The set a hint asks for, through options: a list literal written where it stands is built as
/// that set.
fn set_hint(hint: Option<&Ty>) -> Option<&Ty> {
    match hint {
        Some(Ty::Set(of)) => Some(of),
        Some(Ty::Option(inner)) => set_hint(Some(inner)),
        _ => None,
    }
}

/// `t` with each bool, string or integer replaced by the id or scalar over it that `hint` has at
/// that position, so that a literal is built as the id it stands for (`['public']` as a
/// `set<role-id>`) instead of being converted after.
fn refine(t: &Ty, hint: Option<&Ty>) -> Ty {
    match (t, hint) {
        // A scalar takes the scalar type expected of it (an id, its representation, another
        // integer type, the wide sort key), so each branch converts, rather than the whole.
        (_, Some(h))
            if t != h && scalar_class(t).is_some() && scalar_class(t) == scalar_class(h) =>
        {
            h.clone()
        }
        (Ty::List(a), Some(Ty::List(b) | Ty::Set(b))) => Ty::list(refine(a, Some(b))),
        (Ty::Set(a), Some(Ty::List(b) | Ty::Set(b))) => Ty::set(refine(a, Some(b))),
        (Ty::Option(a), Some(Ty::Option(b))) => Ty::option(refine(a, Some(b))),
        (Ty::Map(k, v), Some(Ty::Map(hk, hv))) => Ty::map(refine(k, Some(hk)), refine(v, Some(hv))),
        (_, Some(Ty::Option(b))) if !matches!(t, Ty::Option(_)) => refine(t, Some(b)),
        _ => t.clone(),
    }
}

/// The class of a scalar type (bool, string, number), for [`refine`]; `None` for any other.
fn scalar_class(t: &Ty) -> Option<Class> {
    match t {
        Ty::Wide => Some(Class::Num),
        Ty::Bool | Ty::Str | Ty::Int(_) | Ty::Number | Ty::Float | Ty::Alias(..) => class(t),
        _ => None,
    }
}

/// True for the expressions whose generated form follows what is expected of them: list
/// literals, `if`, `match`, `let` and `insert`, whose items, branches, keys and values are
/// built in the expected type.
fn follows_hint(e: &Expr) -> bool {
    match &e.kind {
        Kind::List(_) | Kind::If(..) | Kind::Match(..) | Kind::Let(..) => true,
        Kind::Op(op) => op.binder.is_none() && op.name == "insert",
        _ => false,
    }
}

/// The type `e` is generated in where `hint` is expected, when the hint shapes it ([`Gen::value`]
/// and [`Gen::borrow`]): its own, concretized, with literals and branches in the ids the hint
/// has ([`refine`]).
fn shaped(e: &Expr, hint: Option<&Ty>) -> Ty {
    let t = concretize(&e.ty(), hint);
    if follows_hint(e) {
        refine(&t, hint)
    } else {
        t
    }
}

/// The type `e` is held in where `hint` is expected: [`shaped`], and a set for a list literal
/// written where a set is expected ([`Gen::raw`] builds it as one).
fn held(e: &Expr, hint: Option<&Ty>) -> Ty {
    let t = shaped(e, hint);
    match (&e.kind, &t, set_hint(hint)) {
        (Kind::List(_), Ty::List(of), Some(_)) => Ty::Set(of.clone()),
        _ => t,
    }
}

/// True when values of the two types have the same Rust type (open types fit anything).
fn same(a: &Ty, b: &Ty) -> bool {
    match (a, b) {
        (Ty::Unknown | Ty::Never, _) | (_, Ty::Unknown | Ty::Never) => true,
        (Ty::Number, Ty::Int(_) | Ty::Float)
        | (Ty::Int(_) | Ty::Float, Ty::Number)
        | (Ty::Number, Ty::Number) => true,
        (Ty::List(x), Ty::List(y))
        | (Ty::Set(x), Ty::Set(y))
        | (Ty::Option(x), Ty::Option(y))
        | (Ty::Shared(x), Ty::Shared(y)) => same(x, y),
        (Ty::Map(k, v), Ty::Map(k2, v2)) | (Ty::Result(k, v), Ty::Result(k2, v2)) => {
            same(k, k2) && same(v, v2)
        }
        _ => a == b,
    }
}

/// A string literal written where a constrained id or scalar over a string is expected (or an
/// option of one), built directly when the literal meets the type's constraints: they are
/// checked here, on the literal, so the value needs no check where it is built. `None` for
/// anything else, which converts as usual (and a constrained type built from a value that is
/// not such a literal stays refused).
fn constrained_literal(spec: &Spec, e: &Expr, want: &Ty) -> Result<Option<Code>, Refusal> {
    let Kind::Str(s) = &e.kind else {
        return Ok(None);
    };
    let (inner, _) = match want {
        Ty::Option(of) => (of.as_ref(), true),
        other => (other, false),
    };
    if let Ty::Alias(id, repr) = inner {
        if **repr == Ty::Str && is_constrained(spec, id) {
            return match literal_meets(spec, s, want) {
                Some(c) => Ok(Some(c)),
                // Exegra does not check a type's constraints inside a function; the generated
                // construction would, and would fail on every run.
                None => Err(unsupported(format!(
                    "the string literal {s:?} is built as the constrained type {id} but breaks its constraints, so the generated check would fail on every run (Exegra does not check them inside functions)"
                ))),
            };
        }
    }
    Ok(None)
}

/// The literal as a value of the constrained type `want` (or an option of one), when it meets
/// the type's constraints.
fn literal_meets(spec: &Spec, s: &str, want: &Ty) -> Option<Code> {
    let (inner, some) = match want {
        Ty::Option(of) => (of.as_ref(), true),
        other => (other, false),
    };
    let Ty::Alias(id, repr) = inner else {
        return None;
    };
    if **repr != Ty::Str || !is_constrained(spec, id) {
        return None;
    }
    let c = &spec.type_decl(id)?.constraints;
    let n = u64::try_from(s.chars().count()).unwrap_or(u64::MAX);
    if c.min_length
        .as_ref()
        .and_then(serde_json::Number::as_u64)
        .is_some_and(|min| n < min)
        || c.max_length
            .as_ref()
            .and_then(serde_json::Number::as_u64)
            .is_some_and(|max| n > max)
    {
        return None;
    }
    if let Some(p) = &c.pattern {
        if !regex::Regex::new(p).ok()?.is_match(s) {
            return None;
        }
    }
    let p = type_path(id);
    let lit = Literal::string(s);
    let t = quote! { #p(#lit.to_string()) };
    Some(if some {
        Code::new(quote! { Some(#t) }, Prec::Postfix)
    } else {
        Code::new(t, Prec::Postfix)
    })
}

/// What in a type `{:?}` writes otherwise than Exegra: a timestamp, which the generated type
/// writes as `Timestamp(n)` and Exegra as `n`. Declared types are looked through once each.
fn debug_differs(spec: &Spec, t: &Ty, seen: &mut BTreeSet<String>) -> Option<String> {
    match t {
        _ if t.is_timestamp() => Some("a timestamp".to_string()),
        _ if t.is_prelude_number() => Some("a duration".to_string()),
        Ty::Uuid => Some("a uuid".to_string()),
        Ty::Alias(_, r) if matches!(**r, Ty::Uuid) => Some("a uuid".to_string()),
        Ty::List(of) | Ty::Set(of) | Ty::Option(of) | Ty::Shared(of) => {
            debug_differs(spec, of, seen)
        }
        Ty::Map(k, v) | Ty::Result(k, v) => {
            debug_differs(spec, k, seen).or_else(|| debug_differs(spec, v, seen))
        }
        Ty::Struct(id) | Ty::Union(id) => {
            if !seen.insert(id.clone()) {
                return None;
            }
            let fields = if matches!(t, Ty::Struct(_)) {
                struct_fields(spec, id)
            } else {
                variants(spec, id)
                    .into_iter()
                    .flat_map(|v| v.fields)
                    .collect()
            };
            fields.iter().find_map(|f| {
                let ft = f.declared(spec).ok()?;
                debug_differs(spec, &ft, seen).map(|w| format!("{w} (in {id}.{})", f.name))
            })
        }
        _ => None,
    }
}

fn is_constrained(spec: &Spec, id: &str) -> bool {
    spec.type_decl(id)
        .is_some_and(|d| !d.constraints.is_empty())
}

/// An integer literal of the kind, or `None` when the value does not fit it.
fn int_lit(v: i128, kind: Builtin, suffixed: bool) -> Option<Code> {
    let (lo, hi) = kind.integer_range()?;
    if v < lo || v > hi {
        return None;
    }
    let abs = v.unsigned_abs();
    let text = if suffixed {
        format!("{abs}{}", kind.as_str())
    } else {
        abs.to_string()
    };
    let lit: TokenStream = text.parse().ok()?;
    Some(if v < 0 {
        Code::new(quote! { -#lit }, Prec::Prefix)
    } else {
        Code::new(lit, Prec::Postfix)
    })
}

/// An f64 literal of a number as written (`1` is `1.0`).
fn float_lit(text: &str) -> Gened<Code> {
    let v: f64 = text
        .parse()
        .map_err(|_| unsupported(format!("the number {text} is not an f64")))?;
    if !v.is_finite() {
        return Err(unsupported(format!(
            "the number {text} is too large for an f64"
        )));
    }
    let lit = Literal::f64_unsuffixed(v);
    Ok(Code::new(quote! { #lit }, Prec::Postfix))
}

fn wide_lit(v: i128) -> TokenStream {
    let lit: TokenStream = format!("{}i128", v.unsigned_abs())
        .parse()
        .unwrap_or_default();
    if v < 0 {
        quote! { (-#lit) }
    } else {
        lit
    }
}

/// The value of an integer constant: a number literal, or one negated.
fn const_int(e: &Expr) -> Option<i128> {
    match &e.kind {
        Kind::Num(text) => text.parse::<i128>().ok(),
        Kind::Un(UnOp::Neg, a) => const_int(a).and_then(i128::checked_neg),
        _ => None,
    }
}

fn is_null(e: &Expr) -> bool {
    matches!(e.kind, Kind::Null)
}

fn is_empty_collection(e: &Expr) -> bool {
    matches!(&e.kind, Kind::List(items) if items.is_empty()) || matches!(e.kind, Kind::EmptyMap)
}

/// A format string piece: literal text, or a value.
enum Piece {
    Text(String),
    Arg(Code),
    /// A value written with `{:?}`.
    Debug(Code),
}

/// An integer operand of a comparison or arithmetic.
enum IntOpd {
    Const(i128),
    Code(Code, Builtin),
}

/// The generator of one function.
pub(crate) struct Gen<'a> {
    spec: &'a Spec,
    sigs: &'a BTreeMap<String, Sig>,
    me: &'a Sig,
    scope: Vec<Binding>,
    /// The Rust names of every function, which a local may not take.
    reserved: BTreeSet<String>,
    /// The `support` helpers used.
    pub helpers: BTreeSet<&'static str>,
    /// The next [`Gen::raw`] is of a value whose type [`held`] gives, so the hint shapes it.
    shape_next: bool,
    /// The code can fault: it does checked arithmetic or a checked conversion (an integer
    /// narrowed, a constrained id built from a value). Read by [`Gen::faults`].
    faulted: std::cell::Cell<bool>,
}

type Gened<T> = Result<T, Refusal>;

impl<'a> Gen<'a> {
    pub(crate) fn new(
        spec: &'a Spec,
        sigs: &'a BTreeMap<String, Sig>,
        me: &'a Sig,
        reserved: BTreeSet<String>,
    ) -> Self {
        Gen {
            spec,
            sigs,
            me,
            scope: Vec::new(),
            reserved,
            helpers: BTreeSet::new(),
            shape_next: false,
            faulted: std::cell::Cell::new(false),
        }
    }

    /// True when the generated code can fault ([`Gen::fault`] was used), so the function must
    /// say so in its type.
    pub(crate) fn faults(&self) -> bool {
        self.faulted.get()
    }

    fn bind(&mut self, name: &str, ty: Ty, form: Form) -> Gened<Ident> {
        let rust = naming::snake(name).map_err(|e| unsupported(format!("the name {name}: {e}")))?;
        if self.reserved.contains(&rust) {
            return Err(unsupported(format!(
                "the name {name} becomes the Rust name {rust}, which a function of the module has"
            )));
        }
        if let Some(other) = self
            .scope
            .iter()
            .find(|b| b.ident == rust && b.name != name)
        {
            return Err(unsupported(format!(
                "the names {} and {name} both become the Rust name {rust}",
                other.name
            )));
        }
        let ident = naming::ident(&rust);
        self.scope.push(Binding {
            name: name.to_string(),
            ident: ident.clone(),
            ty,
            form,
            deref: 0,
        });
        Ok(ident)
    }

    fn unbind(&mut self, n: usize) {
        let keep = self.scope.len().saturating_sub(n);
        self.scope.truncate(keep);
    }

    fn fault(&self, op: &str) -> TokenStream {
        self.faulted.set(true);
        if self.me.errors.is_some() {
            quote! { Failure::Fault(Fault { op: #op }) }
        } else {
            quote! { Fault { op: #op } }
        }
    }

    /// The error type a fault is returned as: `Fault`, or `Failure<E>` with errors.
    fn fault_type(&self) -> TokenStream {
        match &self.me.errors {
            Some(e) => {
                let e = type_path(e);
                quote! { Failure<#e> }
            }
            None => quote! { Fault },
        }
    }

    /// `items` with each item converted by `conv` (code over `item_`), collected as `target`;
    /// when the conversion can fail, the first failure is returned.
    fn map_items(&self, items: TokenStream, conv: &Code, target: TokenStream) -> Code {
        let it = &conv.t;
        if conv.exits {
            let err = self.fault_type();
            return Code::new(
                quote! {
                    #items
                        .map(|item_| Ok::<_, #err>(#it))
                        .collect::<Result<#target, _>>()?
                },
                Prec::Postfix,
            )
            .exits(true);
        }
        Code::new(
            quote! { #items.map(|item_| #it).collect::<#target>() },
            Prec::Postfix,
        )
    }

    /// The function: its signature, lets and body.
    pub(crate) fn function(
        &mut self,
        doc: TokenStream,
        lets: &[(String, Expr)],
        body: &Expr,
    ) -> Gened<TokenStream> {
        let name = snake_ident(&self.me.id);
        let used = |n: &str| lets.iter().any(|(_, e)| uses(e, n)) || uses(body, n);
        let mut params = Vec::new();
        for (pname, pt) in &self.me.params.clone() {
            let ty = rust_ty(pt)?;
            let ty = if by_ref(pt) {
                quote! { &#ty }
            } else {
                ty
            };
            let form = if by_ref(pt) { Form::Ref } else { Form::Place };
            let ident = self.bind(pname, pt.clone(), form)?;
            if used(pname) {
                params.push(quote! { #ident: #ty });
            } else {
                let unused = format_ident!("_{}", naming::snake(pname).unwrap_or_default());
                params.push(quote! { #unused: #ty });
            }
        }
        let mut stmts = Vec::new();
        for (i, (lname, e)) in lets.iter().enumerate() {
            let ty = concretize(&e.ty(), None);
            if !ty.is_concrete() {
                return Err(unsupported(format!(
                    "the type of let {lname} ({}) cannot be told",
                    e.ty().text()
                )));
            }
            let v = self.value(e, &ty)?;
            let later = lets[i + 1..].iter().any(|(_, l)| uses(l, lname)) || uses(body, lname);
            let t = &v.t;
            if later {
                // A let of a number literal states its type, which nothing else may fix.
                let annotation = if e.ty().is_concrete() {
                    TokenStream::new()
                } else {
                    let rt = rust_ty(&ty)?;
                    quote! { : #rt }
                };
                let ident = self.bind(lname, ty, Form::Place)?;
                stmts.push(quote! { let #ident #annotation = #t; });
            } else if v.exits {
                self.bind(lname, ty, Form::Place)?;
                stmts.push(quote! { let _ = #t; });
            } else {
                self.bind(lname, ty, Form::Place)?;
            }
        }
        let returns = self.me.returns.clone();
        let ret = rust_ty(&returns)?;
        let ret = match (&self.me.errors, self.me.faultable) {
            (None, false) => ret,
            (None, true) => quote! { Result<#ret, Fault> },
            (Some(e), false) => {
                let e = type_path(e);
                quote! { Result<#ret, #e> }
            }
            (Some(e), true) => {
                let e = type_path(e);
                quote! { Result<#ret, Failure<#e>> }
            }
        };
        let result = self.me.errors.is_some() || self.me.faultable;
        let tail = match &body.kind {
            // A body that fails, or passes on a call's result, is the result itself.
            Kind::Op(op) if result && op.name == "fail" => self.fail_value(op)?,
            Kind::Try(inner) if result => match &inner.kind {
                Kind::Call(f, args) if self.me.errors.is_some() => {
                    let call = self.call_tokens(f, args)?.t;
                    let callee = &self.sigs[f.as_str()];
                    if !callee.faultable && self.me.faultable {
                        quote! { #call.map_err(Failure::Error) }
                    } else {
                        call
                    }
                }
                _ => {
                    let v = self.value(body, &returns)?;
                    let t = &v.t;
                    quote! { Ok(#t) }
                }
            },
            _ => {
                let v = self.value(body, &returns)?;
                let t = &v.t;
                if result {
                    quote! { Ok(#t) }
                } else {
                    t.clone()
                }
            }
        };
        Ok(quote! {
            #doc
            pub fn #name(#(#params),*) -> #ret {
                #(#stmts)*
                #tail
            }
        })
    }

    // --- values, borrows and conversions -------------------------------------------------------

    /// An owned value of type `want`.
    fn value(&mut self, e: &Expr, want: &Ty) -> Gened<Code> {
        if let Some(c) = constrained_literal(self.spec, e, want)? {
            return Ok(c);
        }
        self.shape_next = true;
        let c = self.raw(e, Some(want))?;
        let from = self.held_of(e, Some(want));
        // A shared value where its plain type is expected: the value it holds, cloned from
        // the Arc without cloning the Arc first.
        if let Ty::Shared(inner) = &from {
            if !matches!(want, Ty::Shared(_)) && c.form != Form::OptRef {
                let v = self.unshare(Self::borrow_code(c), inner)?;
                return self.coerce(v, inner, want);
            }
        }
        let owned = self.own(c, &from);
        self.coerce(owned, &from, want)
    }

    /// The type `e` is held in where `hint` is expected ([`held`]), but a name (and its fields)
    /// as it is bound: a let or accumulator of a number (untyped in Exegra) is held in the
    /// integer type its binding was given, which the expected type need not be, so the value
    /// converts (an i64 count into a u64 field is `try_from`).
    fn held_of(&self, e: &Expr, hint: Option<&Ty>) -> Ty {
        self.bound_ty(e).unwrap_or_else(|| held(e, hint))
    }

    /// The type of a name as bound, then of its fields; `None` for any other expression.
    fn bound_ty(&self, e: &Expr) -> Option<Ty> {
        let Kind::Ref(path, Res::Var) = &e.kind else {
            return None;
        };
        let b = self.scope.iter().rev().find(|b| b.name == path[0])?;
        let mut ty = b.ty.clone();
        for f in &path[1..] {
            ty = self.field_ty(&ty, f).ok()?;
        }
        Some(ty)
    }

    /// The `T` a `&Arc<T>` holds, cloned.
    fn unshare(&self, r: Code, inner: &Ty) -> Gened<Code> {
        let ty = rust_ty(inner)?;
        let t = &r.t;
        Ok(Code::new(quote! { <#ty as Clone>::clone(#t) }, Prec::Postfix).exits(r.exits))
    }

    /// A `&T` of type `want`.
    fn borrow(&mut self, e: &Expr, want: &Ty) -> Gened<Code> {
        if let Some(c) = constrained_literal(self.spec, e, want)? {
            return Ok(Self::borrow_code(c));
        }
        self.shape_next = true;
        let c = self.raw(e, Some(want))?;
        let from = self.held_of(e, Some(want));
        if same(&from, want) && c.form != Form::OptRef {
            return Ok(Self::borrow_code(c));
        }
        // A shared value borrowed as its plain type: the `&T` in the Arc.
        if let Ty::Shared(inner) = &from {
            if same(inner, want) && c.form != Form::OptRef {
                let b = Self::borrow_code(c);
                let t = &b.t;
                return Ok(
                    Code::new(quote! { std::sync::Arc::as_ref(#t) }, Prec::Postfix).exits(b.exits),
                );
            }
        }
        let owned = self.own(c, &from);
        let v = self.coerce(owned, &from, want)?;
        Ok(Self::borrow_code(v))
    }

    fn borrow_code(c: Code) -> Code {
        if c.form == Form::Ref {
            return Code::new(c.t, c.prec).exits(c.exits);
        }
        let t = c.at(Prec::Prefix);
        Code::new(quote! { &#t }, Prec::Prefix).exits(c.exits)
    }

    /// The code as an owned value.
    fn own(&self, c: Code, ty: &Ty) -> Code {
        let exits = c.exits;
        match c.form {
            Form::Temp => c,
            Form::Place if is_copy(ty) => {
                let mut c = c;
                c.form = Form::Temp;
                c
            }
            Form::Place => {
                let r = c.recv();
                Code::new(quote! { #r.clone() }, Prec::Postfix).exits(exits)
            }
            Form::Ref if is_copy(ty) => {
                let r = c.at(Prec::Prefix);
                Code::new(quote! { *#r }, Prec::Prefix).exits(exits)
            }
            Form::Ref => {
                let r = c.recv();
                Code::new(quote! { #r.clone() }, Prec::Postfix).exits(exits)
            }
            Form::OptRef => {
                let r = c.recv();
                let inner = match ty {
                    Ty::Option(of) => of.as_ref(),
                    other => other,
                };
                if is_copy(inner) {
                    Code::new(quote! { #r.copied() }, Prec::Postfix).exits(exits)
                } else {
                    Code::new(quote! { #r.cloned() }, Prec::Postfix).exits(exits)
                }
            }
        }
    }

    /// An owned value of type `from` as one of type `to`.
    fn coerce(&self, c: Code, from: &Ty, to: &Ty) -> Gened<Code> {
        if same(from, to) {
            return Ok(c);
        }
        let exits = c.exits;
        match (from, to) {
            (f, Ty::Option(inner)) if !matches!(f, Ty::Option(_)) => {
                let v = self.coerce(c, f, inner)?;
                let t = &v.t;
                Ok(Code::new(quote! { Some(#t) }, Prec::Postfix).exits(exits))
            }
            (Ty::Alias(_, r), _) if same(r, to) => {
                let t = c.recv();
                Ok(Code::new(quote! { #t.0 }, Prec::Postfix).exits(exits))
            }
            // An id or scalar where another scalar is expected (another id, a wider or narrower
            // integer): its representation, converted as that would be.
            (Ty::Alias(_, r), _) if scalar_class(to).is_some() => {
                let t = c.recv();
                let v = Code::new(quote! { #t.0 }, Prec::Postfix).exits(exits);
                self.coerce(v, r, to)
            }
            // A shared value as the value it holds (Exegra reads shared<T> as T), and a value
            // shared where shared<T> is expected.
            (Ty::Shared(a), _) => {
                let t = c.at(Prec::Prefix);
                let v = self.unshare(Code::new(quote! { &#t }, Prec::Prefix).exits(exits), a)?;
                self.coerce(v, a, to)
            }
            (_, Ty::Shared(b)) => {
                let v = self.coerce(c, from, b)?;
                let t = &v.t;
                Ok(Code::new(quote! { std::sync::Arc::new(#t) }, Prec::Postfix).exits(exits))
            }
            (Ty::Option(a), Ty::Option(b)) => {
                let item = self.coerce(Code::new(quote! { item_ }, Prec::Postfix), a, b)?;
                let (t, it) = (c.recv(), &item.t);
                if item.exits {
                    let err = self.fault_type();
                    return Ok(Code::new(
                        quote! { #t.map(|item_| Ok::<_, #err>(#it)).transpose()? },
                        Prec::Postfix,
                    )
                    .exits(true));
                }
                Ok(Code::new(quote! { #t.map(|item_| #it) }, Prec::Postfix).exits(exits))
            }
            // A map whose keys or values are of other types: each entry converted, keys as
            // single values are (an id to its representation, a string to a constrained id,
            // checked), values too.
            (Ty::Map(k, a), Ty::Map(k2, b)) => {
                let key = self.coerce(Code::new(quote! { key_ }, Prec::Postfix), k, k2)?;
                let item = self.coerce(Code::new(quote! { item_ }, Prec::Postfix), a, b)?;
                let (t, kt, it) = (c.recv(), &key.t, &item.t);
                let target = quote! { std::collections::BTreeMap<_, _> };
                if key.exits || item.exits {
                    let err = self.fault_type();
                    return Ok(Code::new(
                        quote! {
                            #t.into_iter()
                                .map(|(key_, item_)| Ok::<_, #err>((#kt, #it)))
                                .collect::<Result<#target, _>>()?
                        },
                        Prec::Postfix,
                    )
                    .exits(true));
                }
                Ok(Code::new(
                    quote! {
                        #t.into_iter()
                            .map(|(key_, item_)| (#kt, #it))
                            .collect::<#target>()
                    },
                    Prec::Postfix,
                )
                .exits(exits))
            }
            // An integer where a wider integer type is expected (every value fits): `From`.
            (Ty::Int(a), Ty::Int(b))
                if a.integer_range()
                    .zip(b.integer_range())
                    .is_some_and(|((alo, ahi), (blo, bhi))| blo <= alo && ahi <= bhi) =>
            {
                let k = format_ident!("{}", b.as_str());
                let t = &c.t;
                Ok(Code::new(quote! { #k::from(#t) }, Prec::Postfix).exits(exits))
            }
            // An integer where a narrower integer type is expected. Exegra passes the number on
            // (it checks ranges only of arithmetic and of example values); the generated value
            // must fit, so one that does not is a fault.
            (Ty::Int(_), Ty::Int(b)) => {
                let k = format_ident!("{}", b.as_str());
                let fault = self.fault("as");
                let t = &c.t;
                Ok(Code::new(
                    quote! { #k::try_from(#t).map_err(|_| #fault)? },
                    Prec::Postfix,
                )
                .exits(true))
            }
            // Any integer as the wide sort key.
            (Ty::Int(_), Ty::Wide) => {
                let t = &c.t;
                Ok(Code::new(quote! { i128::from(#t) }, Prec::Postfix).exits(exits))
            }
            // An integer where an f64 is expected, as Exegra's numbers are.
            (Ty::Int(_), Ty::Float) => {
                let t = c.at(Prec::Prefix);
                Ok(Code::new(quote! { (#t as f64) }, Prec::Postfix).exits(exits))
            }
            // Exegra holds a set as a list: where one stands for the other, the items are
            // collected. A list that becomes a set is put in order and loses repeated items, as
            // a BTreeSet holds it. Items of another type (an id where its representation is
            // expected, say) are converted one by one, as a single value would be.
            (Ty::List(a) | Ty::Set(a), Ty::List(b) | Ty::Set(b)) => {
                let target = if matches!(to, Ty::Set(_)) {
                    quote! { std::collections::BTreeSet<_> }
                } else {
                    quote! { Vec<_> }
                };
                let t = c.recv();
                if same(a, b) {
                    return Ok(Code::new(
                        quote! { #t.into_iter().collect::<#target>() },
                        Prec::Postfix,
                    )
                    .exits(exits));
                }
                let item = self.coerce(Code::new(quote! { item_ }, Prec::Postfix), a, b)?;
                let v = self.map_items(quote! { #t.into_iter() }, &item, target);
                Ok(v.exits(exits))
            }
            (_, Ty::Alias(id, r)) if same(from, r) => {
                let p = type_path(id);
                let t = &c.t;
                if is_constrained(self.spec, id) {
                    // The generated type keeps its constraints: the value is checked. Exegra
                    // does not check them inside a function, so a value that breaks them is a
                    // fault here where Exegra goes on.
                    let fault = self.fault("new");
                    return Ok(Code::new(
                        quote! { #p::new(#t).map_err(|_| #fault)? },
                        Prec::Postfix,
                    )
                    .exits(true));
                }
                Ok(Code::new(quote! { #p(#t) }, Prec::Postfix).exits(exits))
            }
            // A scalar of another representation where an id is expected: converted to the
            // representation first (an i64 for a record id over u64, checked).
            (_, Ty::Alias(_, r))
                if scalar_class(from).is_some() && scalar_class(from) == class(r) =>
            {
                let v = self.coerce(c, from, r)?;
                self.coerce(v, r, to)
            }
            _ => Err(unsupported(format!(
                "a value of type {} stands where {} is expected; the generated types differ",
                from.text(),
                to.text()
            ))),
        }
    }

    // --- expressions -----------------------------------------------------------------------------

    /// `e` in its own type (with `hint` deciding literals and empty collections).
    fn raw(&mut self, e: &Expr, hint: Option<&Ty>) -> Gened<Code> {
        // Only a value or borrow lets the hint shape the expression, since only they convert
        // from the type [`held`] gives; every other caller reads the type it concretizes.
        let shape = std::mem::take(&mut self.shape_next);
        let eff = if shape {
            shaped(e, hint)
        } else {
            concretize(&e.ty(), hint)
        };
        match &e.kind {
            Kind::Num(text) if eff == Ty::Float || text.contains('.') => {
                if eff != Ty::Float {
                    return Err(unsupported(format!(
                        "the number {text} has a fraction where {} is expected",
                        eff.text()
                    )));
                }
                float_lit(text)
            }
            Kind::Num(text) => {
                let v: i128 = text
                    .parse()
                    .map_err(|_| unsupported(format!("the number {text} is too large")))?;
                if eff == Ty::Wide {
                    let lit = Literal::i128_suffixed(v);
                    return Ok(Code::new(quote! { #lit }, Prec::Postfix));
                }
                let kind = int_kind(&eff).unwrap_or(Builtin::I64);
                int_lit(v, kind, false).ok_or_else(|| {
                    unsupported(format!("the number {text} does not fit {}", kind.as_str()))
                })
            }
            Kind::Str(s) => {
                let lit = Literal::string(s);
                Ok(Code::new(quote! { #lit.to_string() }, Prec::Postfix))
            }
            Kind::Bool(b) => Ok(Code::new(quote! { #b }, Prec::Postfix)),
            Kind::Null => Ok(Code::new(quote! { None }, Prec::Postfix)),
            Kind::Unit => Ok(Code::new(quote! { () }, Prec::Postfix)),
            Kind::Ref(path, res) => self.reference(path, *res),
            Kind::Field(a, name) => {
                let c = self.raw(a, None)?;
                self.field(c, &concretize(&a.ty(), None), name)
            }
            Kind::Un(UnOp::Not, a) => {
                let v = self.value(a, &Ty::Bool)?;
                let t = v.at(Prec::Prefix);
                Ok(Code::new(quote! { !#t }, Prec::Prefix).exits(v.exits))
            }
            Kind::Un(UnOp::Neg, a) => self.negate(e, a, &eff),
            Kind::Bin(op, l, r) => self.binary(*op, l, r, &eff),
            Kind::If(c, x, y) => {
                let cond = self.value(c, &Ty::Bool)?;
                let a = self.value(x, &eff)?;
                let b = self.value(y, &eff)?;
                let (ct, at) = (cond.cond(), &a.t);
                let otherwise = if b.is_if {
                    b.t.clone()
                } else {
                    let bt = &b.t;
                    quote! { { #bt } }
                };
                let mut code = Code::new(quote! { if #ct { #at } else #otherwise }, Prec::Block)
                    .exits(cond.exits || a.exits || b.exits);
                code.is_if = true;
                Ok(code)
            }
            Kind::Let(name, v, body) => {
                let ty = concretize(&v.ty(), None);
                if !ty.is_concrete() {
                    return Err(unsupported(format!(
                        "the type of let {name} ({}) cannot be told",
                        v.ty().text()
                    )));
                }
                let vc = self.value(v, &ty)?;
                let vt = &vc.t;
                if uses(body, name) {
                    let annotation = if v.ty().is_concrete() {
                        TokenStream::new()
                    } else {
                        let rt = rust_ty(&ty)?;
                        quote! { : #rt }
                    };
                    let ident = self.bind(name, ty, Form::Place)?;
                    let bc = self.value(body, &eff);
                    self.unbind(1);
                    let bc = bc?;
                    let bt = &bc.t;
                    Ok(Code::new(
                        quote! { { let #ident #annotation = #vt; #bt } },
                        Prec::Block,
                    )
                    .exits(vc.exits || bc.exits))
                } else {
                    let bc = self.value(body, &eff)?;
                    let bt = &bc.t;
                    if vc.exits {
                        Ok(Code::new(quote! { { let _ = #vt; #bt } }, Prec::Block).exits(true))
                    } else {
                        Ok(bc)
                    }
                }
            }
            Kind::Match(s, arms) => self.matching(s, arms, &eff),
            Kind::List(items) => {
                let Ty::List(of) = &eff else {
                    return Err(unsupported("a list literal where no list is expected"));
                };
                // Where a set is expected the literal is built as one ([`held`]).
                let set = shape && set_hint(hint).is_some();
                if items.is_empty() {
                    let t = rust_ty(of)?;
                    return Ok(if set {
                        Code::new(
                            quote! { std::collections::BTreeSet::<#t>::new() },
                            Prec::Postfix,
                        )
                    } else {
                        Code::new(quote! { Vec::<#t>::new() }, Prec::Postfix)
                    });
                }
                let mut exits = false;
                let mut out = Vec::new();
                for item in items {
                    let v = self.value(item, of)?;
                    exits = exits || v.exits;
                    out.push(v.t);
                }
                Ok(if set {
                    Code::new(
                        quote! { std::collections::BTreeSet::from([#(#out),*]) },
                        Prec::Postfix,
                    )
                    .exits(exits)
                } else {
                    Code::new(quote! { vec![#(#out),*] }, Prec::Postfix).exits(exits)
                })
            }
            Kind::EmptyMap => {
                if let Ty::Map(k, v) = &eff {
                    if let (Ok(k), Ok(v)) = (rust_ty(k), rust_ty(v)) {
                        return Ok(Code::new(
                            quote! { std::collections::BTreeMap::<#k, #v>::new() },
                            Prec::Postfix,
                        ));
                    }
                }
                Ok(Code::new(
                    quote! { std::collections::BTreeMap::new() },
                    Prec::Postfix,
                ))
            }
            Kind::Ctor(path, fields) => self.construct(path, fields),
            Kind::Newtype(path, arg) => self.newtype(path, arg),
            Kind::Op(op) => self.operation(op, &eff),
            Kind::Call(f, args) => self.call(f, args),
            Kind::Try(a) => self.attempt(a),
        }
    }

    fn reference(&mut self, path: &[String], res: Res) -> Gened<Code> {
        match res {
            Res::Var => {
                let Some(b) = self.scope.iter().rev().find(|b| b.name == path[0]).cloned() else {
                    return Err(unsupported(format!("the name {} is not bound", path[0])));
                };
                let ident = &b.ident;
                let mut c = match b.deref {
                    0 => {
                        let mut c = Code::new(quote! { #ident }, Prec::Postfix).form(b.form);
                        c.ident = true;
                        c
                    }
                    // A boxed field bound by a match: the place in the box.
                    1 => Code::new(quote! { (*#ident) }, Prec::Postfix).form(Form::Place),
                    _ => Code::new(quote! { (**#ident) }, Prec::Postfix).form(Form::Place),
                };
                let mut ty = b.ty.clone();
                for f in &path[1..] {
                    let next = self.field(c, &ty, f)?;
                    ty = self.field_ty(&ty, f)?;
                    c = next;
                }
                Ok(c)
            }
            Res::Enum | Res::Unit => Ok(Code::new(variant_path(&path[0], &path[1]), Prec::Postfix)),
            Res::Language | Res::Unresolved => {
                Err(unsupported(format!("{} is not a value", path.join("."))))
            }
        }
    }

    fn field_ty(&self, owner: &Ty, name: &str) -> Gened<Ty> {
        let Ty::Struct(id) = unshared(owner) else {
            return Err(unsupported(format!("{} has no field {name}", owner.text())));
        };
        let f = struct_fields(self.spec, id)
            .into_iter()
            .find(|f| f.name == name)
            .ok_or_else(|| unsupported(format!("{id} has no field {name}")))?;
        f.read(self.spec)
    }

    /// `owner.name`: a field read through a shared owner (`Arc<T>` derefs to `T`), and a boxed
    /// field read as the place in its box (`(*owner.name)`).
    fn field(&mut self, c: Code, owner: &Ty, name: &str) -> Gened<Code> {
        let shared = matches!(owner, Ty::Shared(_));
        let Ty::Struct(id) = unshared(owner) else {
            return Err(unsupported(format!("{} has no field {name}", owner.text())));
        };
        let fields = struct_fields(self.spec, id);
        let f = fields
            .iter()
            .find(|f| f.name == name)
            .ok_or_else(|| unsupported(format!("{id} has no field {name}")))?;
        let read = f.read(self.spec)?;
        if f.boxed && matches!(read, Ty::Option(_)) {
            return Err(unsupported(format!(
                "the field {name} of {id} is an optional boxed field; reading it is not compiled yet"
            )));
        }
        let fid = snake_ident(name);
        let r = c.recv();
        // A field of an owned value is owned too, but not through an Arc or a Box, which it
        // cannot be moved out of.
        let form = if c.form == Form::Temp && !shared && !f.boxed {
            Form::Temp
        } else {
            Form::Place
        };
        let t = if f.boxed {
            quote! { (*#r.#fid) }
        } else {
            quote! { #r.#fid }
        };
        Ok(Code::new(t, Prec::Postfix).form(form).exits(c.exits))
    }

    fn negate(&mut self, e: &Expr, a: &Expr, eff: &Ty) -> Gened<Code> {
        if *eff == Ty::Float {
            let v = self.value(a, &Ty::Float)?;
            let t = v.at(Prec::Prefix);
            return Ok(Code::new(quote! { -#t }, Prec::Prefix).exits(v.exits));
        }
        let kind = int_kind(eff).unwrap_or(Builtin::I64);
        if let Some(v) = const_int(e) {
            return int_lit(v, kind, false).ok_or_else(|| {
                unsupported(format!("the number {v} does not fit {}", kind.as_str()))
            });
        }
        let inner = match eff {
            Ty::Alias(_, r) => (**r).clone(),
            other => other.clone(),
        };
        let v = self.value(a, &inner)?;
        let r = v.recv();
        let fault = self.fault("-");
        let code = quote! { #r.checked_neg().ok_or(#fault)? };
        Ok(self.wrap_alias(Code::new(code, Prec::Postfix).exits(true), eff))
    }

    fn wrap_alias(&self, c: Code, ty: &Ty) -> Code {
        match ty {
            Ty::Alias(id, _) => {
                let p = type_path(id);
                let t = &c.t;
                Code::new(quote! { #p(#t) }, Prec::Postfix).exits(c.exits)
            }
            _ => c,
        }
    }

    fn int_operand(&mut self, e: &Expr) -> Gened<IntOpd> {
        if let Some(v) = const_int(e) {
            return Ok(IntOpd::Const(v));
        }
        let ty = self
            .bound_ty(e)
            .unwrap_or_else(|| concretize(&e.ty(), None));
        let Some(kind) = int_kind(&ty) else {
            return Err(unsupported(format!("{} is not an integer type", ty.text())));
        };
        let c = self.raw(e, None)?;
        let c = self.own(c, &ty);
        let c = if let Ty::Alias(_, r) = &ty {
            self.coerce(c, &ty, r)?
        } else {
            c
        };
        Ok(IntOpd::Code(c, kind))
    }

    fn binary(&mut self, op: BinOp, l: &Expr, r: &Expr, eff: &Ty) -> Gened<Code> {
        match op {
            BinOp::And | BinOp::Or => {
                let (a, b) = (self.value(l, &Ty::Bool)?, self.value(r, &Ty::Bool)?);
                let (p, sym) = if op == BinOp::And {
                    (Prec::And, quote! { && })
                } else {
                    (Prec::Or, quote! { || })
                };
                let next = if op == BinOp::And {
                    Prec::Cmp
                } else {
                    Prec::And
                };
                let (at, bt) = (a.at(p), b.at(next));
                let mut c = Code::new(quote! { #at #sym #bt }, p).exits(a.exits || b.exits);
                c.brace = a.brace || b.brace;
                Ok(c)
            }
            BinOp::Implies => {
                let (a, b) = (self.value(l, &Ty::Bool)?, self.value(r, &Ty::Bool)?);
                let (at, bt) = (a.at(Prec::Prefix), b.at(Prec::And));
                let mut c = Code::new(quote! { !#at || #bt }, Prec::Or).exits(a.exits || b.exits);
                c.brace = a.brace || b.brace;
                Ok(c)
            }
            BinOp::Eq | BinOp::Ne => self.equality(l, r, op == BinOp::Ne),
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge if self.floats(l, r) => {
                self.float_compare(op, l, r)
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let (a, b) = (self.int_operand(l)?, self.int_operand(r)?);
                Ok(self.compare_ints(a, b, op))
            }
            BinOp::Add
                if class(&l.ty()) == Some(Class::Str) || class(&r.ty()) == Some(Class::Str) =>
            {
                let a = self.piece(l)?;
                let b = self.piece(r)?;
                Ok(self.format(vec![a, b]))
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                self.arith(op, l, r, eff)
            }
        }
    }

    fn compare_ints(&self, a: IntOpd, b: IntOpd, op: BinOp) -> Code {
        let sym: TokenStream = op.symbol().parse().unwrap_or_default();
        let t = match (a, b) {
            (IntOpd::Const(x), IntOpd::Const(y)) => {
                let v = match op {
                    BinOp::Eq => x == y,
                    BinOp::Ne => x != y,
                    BinOp::Lt => x < y,
                    BinOp::Le => x <= y,
                    BinOp::Gt => x > y,
                    _ => x >= y,
                };
                return Code::new(quote! { #v }, Prec::Postfix);
            }
            (IntOpd::Code(c, k), IntOpd::Const(y)) => {
                let ct = c.at(Prec::Add);
                match int_lit(y, k, false) {
                    Some(lit) => {
                        let lt = lit.at(Prec::Add);
                        return Code::new(quote! { #ct #sym #lt }, Prec::Cmp).exits(c.exits);
                    }
                    None => {
                        let w = wide_lit(y);
                        let ct = &c.t;
                        return Code::new(quote! { i128::from(#ct) #sym #w }, Prec::Cmp)
                            .exits(c.exits);
                    }
                }
            }
            (IntOpd::Const(x), IntOpd::Code(c, k)) => {
                let ct = c.at(Prec::Add);
                match int_lit(x, k, false) {
                    Some(lit) => {
                        let lt = lit.at(Prec::Add);
                        return Code::new(quote! { #lt #sym #ct }, Prec::Cmp).exits(c.exits);
                    }
                    None => {
                        let w = wide_lit(x);
                        let ct = &c.t;
                        return Code::new(quote! { #w #sym i128::from(#ct) }, Prec::Cmp)
                            .exits(c.exits);
                    }
                }
            }
            (IntOpd::Code(a, ka), IntOpd::Code(b, kb)) => {
                let exits = a.exits || b.exits;
                if ka == kb {
                    let (at, bt) = (a.at(Prec::Add), b.at(Prec::Add));
                    return Code::new(quote! { #at #sym #bt }, Prec::Cmp).exits(exits);
                }
                let (at, bt) = (&a.t, &b.t);
                (quote! { i128::from(#at) #sym i128::from(#bt) }, exits)
            }
        };
        Code::new(t.0, Prec::Cmp).exits(t.1)
    }

    /// True when a comparison or arithmetic is on f64s: either side is one (the other is then
    /// an f64 or an integer, which Exegra computes with as the same number).
    fn floats(&self, l: &Expr, r: &Expr) -> bool {
        concretize(&l.ty(), None) == Ty::Float || concretize(&r.ty(), None) == Ty::Float
    }

    /// Two numbers compared as f64s.
    fn float_compare(&mut self, op: BinOp, l: &Expr, r: &Expr) -> Gened<Code> {
        let (a, b) = (self.value(l, &Ty::Float)?, self.value(r, &Ty::Float)?);
        let sym: TokenStream = op.symbol().parse().unwrap_or_default();
        let (at, bt) = (a.at(Prec::Add), b.at(Prec::Add));
        Ok(Code::new(quote! { #at #sym #bt }, Prec::Cmp).exits(a.exits || b.exits))
    }

    /// f64 arithmetic, as Exegra computes with numbers: `+`, `-` and `*` as they are; `/` and
    /// `%` by zero are an evaluation error in Exegra, a fault here.
    fn float_arith(&mut self, op: BinOp, l: &Expr, r: &Expr) -> Gened<Code> {
        let (a, b) = (self.value(l, &Ty::Float)?, self.value(r, &Ty::Float)?);
        let exits = a.exits || b.exits;
        let (at, bt) = (a.at(Prec::Add), b.at(Prec::Prefix));
        Ok(match op {
            BinOp::Add => Code::new(quote! { #at + #bt }, Prec::Add).exits(exits),
            BinOp::Sub => Code::new(quote! { #at - #bt }, Prec::Add).exits(exits),
            BinOp::Mul => {
                let at = a.at(Prec::Prefix);
                Code::new(quote! { (#at * #bt) }, Prec::Postfix).exits(exits)
            }
            _ => {
                let fault = self.fault(op.symbol());
                let at = a.at(Prec::Prefix);
                let sym: TokenStream = op.symbol().parse().unwrap_or_default();
                let bt = &b.t;
                Code::new(
                    quote! {
                        {
                            let divisor_: f64 = #bt;
                            if divisor_ == 0.0 {
                                return Err(#fault);
                            }
                            #at #sym divisor_
                        }
                    },
                    Prec::Block,
                )
                .exits(true)
            }
        })
    }

    fn arith(&mut self, op: BinOp, l: &Expr, r: &Expr, eff: &Ty) -> Gened<Code> {
        if *eff == Ty::Float {
            return self.float_arith(op, l, r);
        }
        let kind = int_kind(eff).ok_or_else(|| {
            unsupported(format!(
                "arithmetic on {}, which is not an integer type",
                eff.text()
            ))
        })?;
        let method = format_ident!(
            "{}",
            match op {
                BinOp::Add => "checked_add",
                BinOp::Sub => "checked_sub",
                BinOp::Mul => "checked_mul",
                BinOp::Div => "checked_div",
                _ => "checked_rem",
            }
        );
        let fault = self.fault(op.symbol());
        let (a, b) = (self.int_operand(l)?, self.int_operand(r)?);
        let fits = |x: &IntOpd| match x {
            IntOpd::Const(v) => int_lit(*v, kind, false).is_some(),
            IntOpd::Code(_, k) => *k == kind,
        };
        let code = if fits(&a) && fits(&b) {
            let recv = match &a {
                IntOpd::Const(v) => int_lit(*v, kind, true)
                    .map(|c| c.recv())
                    .unwrap_or_default(),
                IntOpd::Code(c, _) => c.recv(),
            };
            let arg = match &b {
                IntOpd::Const(v) => int_lit(*v, kind, false).map(|c| c.t).unwrap_or_default(),
                IntOpd::Code(c, _) => c.t.clone(),
            };
            quote! { #recv.#method(#arg).ok_or(#fault)? }
        } else {
            let wide = |x: &IntOpd| match x {
                IntOpd::Const(v) => wide_lit(*v),
                IntOpd::Code(c, _) => {
                    let t = &c.t;
                    quote! { i128::from(#t) }
                }
            };
            let (wa, wb) = (wide(&a), wide(&b));
            let k = format_ident!("{}", kind.as_str());
            quote! { #wa.#method(#wb).and_then(|wide_| #k::try_from(wide_).ok()).ok_or(#fault)? }
        };
        Ok(self.wrap_alias(Code::new(code, Prec::Postfix).exits(true), eff))
    }

    fn equality(&mut self, l: &Expr, r: &Expr, ne: bool) -> Gened<Code> {
        let constant = |v: bool| Code::new(quote! { #v }, Prec::Postfix);
        if is_null(l) || is_null(r) {
            let other = if is_null(l) { r } else { l };
            if is_null(other) {
                return Ok(constant(!ne));
            }
            if !matches!(other.ty(), Ty::Option(_)) {
                // A value that is not an option is never null.
                return Ok(constant(ne));
            }
            let c = self.raw(other, None)?;
            let rt = c.recv();
            let test = if ne {
                quote! { is_some }
            } else {
                quote! { is_none }
            };
            return Ok(Code::new(quote! { #rt.#test() }, Prec::Postfix).exits(c.exits));
        }
        if is_empty_collection(l) || is_empty_collection(r) {
            let other = if is_empty_collection(l) { r } else { l };
            if is_empty_collection(other) {
                return Ok(constant(!ne));
            }
            let c = self.raw(other, None)?;
            let rt = c.recv();
            let t = if ne {
                quote! { !#rt.is_empty() }
            } else {
                quote! { #rt.is_empty() }
            };
            let prec = if ne { Prec::Prefix } else { Prec::Postfix };
            return Ok(Code::new(t, prec).exits(c.exits));
        }
        let (lt, rt) = (l.ty(), r.ty());
        let sym = if ne {
            quote! { != }
        } else {
            quote! { == }
        };
        let both_numbers = is_number(&lt) && is_number(&rt);
        if both_numbers
            && !matches!(lt, Ty::Option(_))
            && !matches!(rt, Ty::Option(_))
            && self.floats(l, r)
        {
            return self.float_compare(if ne { BinOp::Ne } else { BinOp::Eq }, l, r);
        }
        if both_numbers && !matches!(lt, Ty::Option(_)) && !matches!(rt, Ty::Option(_)) {
            let (a, b) = (self.int_operand(l)?, self.int_operand(r)?);
            return Ok(self.compare_ints(a, b, if ne { BinOp::Ne } else { BinOp::Eq }));
        }
        let lo = matches!(lt, Ty::Option(_));
        let ro = matches!(rt, Ty::Option(_));
        if lo != ro {
            let (o, p) = if lo { (l, r) } else { (r, l) };
            let oty = concretize(&o.ty(), Some(&Ty::option(p.ty())));
            let inner = match &oty {
                Ty::Option(of) => (**of).clone(),
                other => other.clone(),
            };
            let oc = self.raw(o, Some(&oty))?;
            let ot = if oc.form == Form::OptRef {
                oc.at(Prec::Add)
            } else {
                let r = oc.recv();
                quote! { #r.as_ref() }
            };
            let pb = self.borrow(p, &inner)?;
            let pt = &pb.t;
            return Ok(
                Code::new(quote! { #ot #sym Some(#pt) }, Prec::Cmp).exits(oc.exits || pb.exits)
            );
        }
        // Two values of one type; an id or scalar against its representation compares the
        // representation.
        let lf = concretize(&lt, Some(&rt));
        let rf = concretize(&rt, Some(&lt));
        let mut a = self.side(l, &lf, &rf)?;
        let mut b = self.side(r, &rf, &lf)?;
        let unwrap = |t: &Ty| match t {
            Ty::Alias(_, r) => (**r).clone(),
            other => other.clone(),
        };
        let (la, rb) = match (&lf, &rf) {
            (Ty::Alias(x, _), Ty::Alias(y, _)) if x == y => (lf.clone(), rf.clone()),
            (Ty::Alias(..), _) | (_, Ty::Alias(..)) => (unwrap(&lf), unwrap(&rf)),
            _ => (lf.clone(), rf.clone()),
        };
        if !same(&la, &rb) {
            return Err(unsupported(format!(
                "== compares {} with {}, whose generated types differ",
                lf.text(),
                rf.text()
            )));
        }
        // An Option<&T> against an Option<T>: borrow the other side too.
        if a.form == Form::OptRef && b.form != Form::OptRef {
            let r = b.recv();
            b = Code::new(quote! { #r.as_ref() }, Prec::Postfix).exits(b.exits);
        } else if b.form == Form::OptRef && a.form != Form::OptRef {
            let r = a.recv();
            a = Code::new(quote! { #r.as_ref() }, Prec::Postfix).exits(a.exits);
        }
        let (at, bt) = (Self::deref(&a), Self::deref(&b));
        let mut c = Code::new(quote! { #at #sym #bt }, Prec::Cmp).exits(a.exits || b.exits);
        c.brace = a.brace || b.brace;
        Ok(c)
    }

    /// One side of `==`: a string literal as `&str`, an id or scalar compared with its
    /// representation as `.0`, anything else as it is.
    fn side(&mut self, e: &Expr, own: &Ty, other: &Ty) -> Gened<Code> {
        if let Kind::Str(s) = &e.kind {
            let lit = Literal::string(s);
            return Ok(Code::new(quote! { #lit }, Prec::Postfix));
        }
        let c = self.raw(e, Some(other))?;
        match (own, other) {
            (Ty::Alias(x, _), Ty::Alias(y, _)) if x == y => Ok(c),
            (Ty::Alias(..), _) => {
                let r = c.recv();
                Ok(Code::new(quote! { #r.0 }, Prec::Postfix)
                    .form(if c.form == Form::Temp {
                        Form::Temp
                    } else {
                        Form::Place
                    })
                    .exits(c.exits))
            }
            _ => Ok(c),
        }
    }

    /// A side of a comparison at a level above `==`, dereferenced when it is a `&T`.
    fn deref(c: &Code) -> TokenStream {
        if c.form == Form::Ref {
            let t = c.at(Prec::Prefix);
            quote! { *#t }
        } else {
            c.at(Prec::Add)
        }
    }

    // --- format --------------------------------------------------------------------------------

    fn piece(&mut self, e: &Expr) -> Gened<Piece> {
        if let Kind::Str(s) = &e.kind {
            return Ok(Piece::Text(s.clone()));
        }
        if let Some(v) = const_int(e) {
            return Ok(Piece::Text(v.to_string()));
        }
        let ty = concretize(&e.ty(), None);
        let c = self.raw(e, None)?;
        let c = match &ty {
            Ty::Enum(_) => {
                let r = c.recv();
                Code::new(quote! { #r.as_str() }, Prec::Postfix).exits(c.exits)
            }
            Ty::Alias(..) => {
                let r = c.recv();
                Code::new(quote! { #r.0 }, Prec::Postfix).exits(c.exits)
            }
            Ty::Option(_)
            | Ty::List(_)
            | Ty::Set(_)
            | Ty::Map(..)
            | Ty::Struct(_)
            | Ty::Union(_) => {
                return Err(unsupported(format!(
                    "format writes {}, which is not a number, string, bool or enum value",
                    ty.text()
                )))
            }
            _ => c,
        };
        Ok(Piece::Arg(c))
    }

    /// A value written by `{:?}`: Rust's derived Debug of the generated type, which Exegra's
    /// debug rule mirrors, but where the generated type is a newtype Exegra writes as a bare
    /// number (a timestamp).
    fn debug_piece(&mut self, e: &Expr) -> Gened<Piece> {
        let ty = concretize(&e.ty(), None);
        if let Some(what) = debug_differs(self.spec, &ty, &mut BTreeSet::new()) {
            return Err(unsupported(format!(
                "{{:?}} writes {}, which holds {what}: Exegra writes it as its number, the generated type as a newtype",
                ty.text()
            )));
        }
        let c = self.raw(e, None)?;
        Ok(Piece::Debug(c))
    }

    /// A value written by `{:json}`: serde_json's text of it, as Exegra's JSON rule mirrors
    /// (strings, numbers, bools, ids and scalars over them, options and lists of these).
    fn json_piece(&mut self, e: &Expr) -> Gened<Piece> {
        let ty = concretize(&e.ty(), None);
        if !json_writable(&ty) {
            return Err(unsupported(format!("{{:json}} writes {}", ty.text())));
        }
        let c = self.raw(e, None)?;
        let b = Self::borrow_code(c);
        let t = &b.t;
        Ok(Piece::Arg(
            Code::new(
                quote! { serde_json::to_string(#t).unwrap_or_default() },
                Prec::Postfix,
            )
            .exits(b.exits),
        ))
    }

    fn format(&self, pieces: Vec<Piece>) -> Code {
        let mut text = String::new();
        let mut args = Vec::new();
        let mut exits = false;
        for p in pieces {
            match p {
                Piece::Text(s) => text.push_str(&s.replace('{', "{{").replace('}', "}}")),
                Piece::Debug(c) => {
                    exits = exits || c.exits;
                    text.push_str("{:?}");
                    args.push(c.t);
                }
                Piece::Arg(c) => {
                    exits = exits || c.exits;
                    let name = c.t.to_string();
                    if c.ident && !name.starts_with("r#") {
                        text.push('{');
                        text.push_str(&name);
                        text.push('}');
                    } else {
                        text.push_str("{}");
                        args.push(c.t);
                    }
                }
            }
        }
        let lit = Literal::string(&text);
        Code::new(quote! { format!(#lit #(, #args)*) }, Prec::Postfix).exits(exits)
    }

    // --- match -----------------------------------------------------------------------------------

    fn matching(&mut self, s: &Expr, arms: &[Arm], eff: &Ty) -> Gened<Code> {
        let st = concretize(&s.ty(), None);
        let Some(m) = match_on(&st) else {
            return Err(unsupported(format!("match on {}", st.text())));
        };
        let all = cases(self.spec, &m);
        let sc = self.raw(s, None)?;
        let (scrut, bind_form) = match sc.form {
            // A shared union is matched through its Arc.
            _ if matches!(st, Ty::Shared(_)) => {
                let b = Self::borrow_code(sc.clone());
                let t = &b.t;
                (quote! { std::sync::Arc::as_ref(#t) }, Form::Ref)
            }
            Form::Temp => (sc.cond(), Form::Place),
            Form::OptRef | Form::Ref => (sc.cond(), Form::Ref),
            Form::Place => {
                if is_copy(&st) || matches!(m, MatchOn::Enum(_)) {
                    (sc.cond(), Form::Place)
                } else {
                    let t = sc.at(Prec::Prefix);
                    (quote! { &#t }, Form::Ref)
                }
            }
        };
        let mut exits = sc.exits;
        let mut out = Vec::new();
        for arm in arms {
            let mut bound = 0;
            let pat = match &arm.pat {
                Pattern::Wild => quote! { _ },
                Pattern::Case { name, bind, fields } => {
                    let case = all
                        .iter()
                        .find(|c| &c.name == name)
                        .ok_or_else(|| unsupported(format!("no case {name}")))?;
                    // The binders, by field index.
                    let mut binds: Vec<Option<String>> = vec![None; case.fields.len()];
                    if let Some(b) = bind {
                        binds[0] = Some(b.clone());
                    }
                    for (field, b) in fields.iter().flatten() {
                        if let Some(i) = case.fields.iter().position(|f| &f.name == field) {
                            binds[i] = Some(b.clone());
                        }
                    }
                    let mut idents: Vec<TokenStream> = Vec::new();
                    for (i, b) in binds.iter().enumerate() {
                        match b {
                            Some(b) if b != "_" && uses(&arm.body, b) => {
                                let ty = case.fields[i].ty.clone()?;
                                let ident = self.bind(b, ty, bind_form)?;
                                if case.fields[i].boxed {
                                    // The binding is the Box (or a &Box); the value is the
                                    // place in it.
                                    if let Some(last) = self.scope.last_mut() {
                                        last.deref = if bind_form == Form::Ref { 2 } else { 1 };
                                        last.form = Form::Place;
                                    }
                                }
                                bound += 1;
                                idents.push(quote! { #ident });
                            }
                            _ => idents.push(quote! { _ }),
                        }
                    }
                    self.pattern(&m, case, &binds, &idents)?
                }
            };
            let body = self.value(&arm.body, eff);
            self.unbind(bound);
            let body = body?;
            exits = exits || body.exits;
            let bt = &body.t;
            out.push(quote! { #pat => #bt, });
        }
        Ok(Code::new(quote! { match #scrut { #(#out)* } }, Prec::Block).exits(exits))
    }

    fn pattern(
        &self,
        m: &MatchOn,
        case: &super::typing::Case,
        binds: &[Option<String>],
        idents: &[TokenStream],
    ) -> Gened<TokenStream> {
        let name = &case.name;
        Ok(match m {
            MatchOn::Option(_) | MatchOn::Result(..) => {
                let v = format_ident!("{}", name);
                if case.fields.is_empty() {
                    quote! { #v }
                } else {
                    let b = &idents[0];
                    quote! { #v(#b) }
                }
            }
            MatchOn::Enum(id) => variant_path(id, name),
            MatchOn::Union(id) => {
                let path = variant_path(id, name);
                let info = variants(self.spec, id)
                    .into_iter()
                    .find(|v| &v.name == name)
                    .ok_or_else(|| unsupported(format!("{id} has no variant {name}")))?;
                if info.fields.is_empty() {
                    path
                } else if info.newtype {
                    let b = &idents[0];
                    quote! { #path(#b) }
                } else if info.tuple {
                    // Positional: each field's binder in declaration order, `_` for the rest.
                    quote! { #path(#(#idents),*) }
                } else {
                    let mut parts = Vec::new();
                    let mut all = true;
                    for (i, f) in info.fields.iter().enumerate() {
                        let fid = snake_ident(&f.name);
                        if binds[i].is_none() {
                            all = false;
                            continue;
                        }
                        let b = &idents[i];
                        if b.to_string() == fid.to_string() {
                            parts.push(quote! { #fid });
                        } else {
                            parts.push(quote! { #fid: #b });
                        }
                    }
                    if all {
                        quote! { #path { #(#parts),* } }
                    } else if parts.is_empty() {
                        quote! { #path { .. } }
                    } else {
                        quote! { #path { #(#parts,)* .. } }
                    }
                }
            }
        })
    }

    // --- constructors ----------------------------------------------------------------------------

    /// The value of a field a constructor sets (`None` when it is left out), boxed where the
    /// type holds it boxed, and whether it is the bare name of the field.
    fn field_value(
        &mut self,
        f: &super::typing::FieldInfo,
        given: Option<&Expr>,
    ) -> Gened<(TokenStream, bool, bool)> {
        let Some(e) = given else {
            return Ok((quote! { None }, false, false));
        };
        let want = f.read(self.spec)?;
        let v = self.value(e, &want)?;
        let t = &v.t;
        let value = if f.boxed {
            if matches!(want, Ty::Option(_)) {
                let r = v.recv();
                quote! { #r.map(Box::new) }
            } else {
                quote! { Box::new(#t) }
            }
        } else {
            t.clone()
        };
        let fid = snake_ident(&f.name);
        let shorthand = !f.boxed && v.ident && v.t.to_string() == fid.to_string();
        Ok((value, v.exits, shorthand))
    }

    fn field_init(
        &mut self,
        f: &super::typing::FieldInfo,
        given: Option<&Expr>,
    ) -> Gened<(TokenStream, bool)> {
        let fid = snake_ident(&f.name);
        let (value, exits, shorthand) = self.field_value(f, given)?;
        if shorthand {
            return Ok((quote! { #fid }, exits));
        }
        Ok((quote! { #fid: #value }, exits))
    }

    fn construct(&mut self, path: &[String], fields: &[(String, Expr)]) -> Gened<Code> {
        let (p, declared, tuple) = if path.len() == 1 {
            (
                type_path(&path[0]),
                struct_fields(self.spec, &path[0]),
                false,
            )
        } else {
            let v = variants(self.spec, &path[0])
                .into_iter()
                .find(|v| v.name == path[1])
                .ok_or_else(|| unsupported(format!("{} has no variant {}", path[0], path[1])))?;
            (variant_path(&path[0], &path[1]), v.fields, v.tuple)
        };
        let mut inits = Vec::new();
        let mut exits = false;
        for f in &declared {
            let given = fields.iter().find(|(n, _)| n == &f.name).map(|(_, e)| e);
            let (init, x) = if tuple {
                // A tuple variant takes its fields by position, in declaration order.
                let (value, x, _) = self.field_value(f, given)?;
                (value, x)
            } else {
                self.field_init(f, given)?
            };
            exits = exits || x;
            inits.push(init);
        }
        if tuple {
            return Ok(Code::new(quote! { #p(#(#inits),*) }, Prec::Postfix).exits(exits));
        }
        let mut c = Code::new(quote! { #p { #(#inits),* } }, Prec::Postfix).exits(exits);
        c.brace = true;
        Ok(c)
    }

    fn newtype(&mut self, path: &[String], arg: &Expr) -> Gened<Code> {
        let v = variants(self.spec, &path[0])
            .into_iter()
            .find(|v| v.name == path[1])
            .ok_or_else(|| unsupported(format!("{} has no variant {}", path[0], path[1])))?;
        let p = variant_path(&path[0], &path[1]);
        let f = v
            .fields
            .first()
            .ok_or_else(|| unsupported(format!("{} has no field", path[1])))?;
        if v.newtype {
            let want = f.read(self.spec)?;
            let c = self.value(arg, &want)?;
            let t = &c.t;
            let t = if f.boxed {
                quote! { Box::new(#t) }
            } else {
                t.clone()
            };
            return Ok(Code::new(quote! { #p(#t) }, Prec::Postfix).exits(c.exits));
        }
        let (init, exits) = self.field_init(f, Some(arg))?;
        let mut c = Code::new(quote! { #p { #init } }, Prec::Postfix).exits(exits);
        c.brace = true;
        Ok(c)
    }

    // --- calls, try and fail ---------------------------------------------------------------------

    /// `f(args)` with each argument as the parameter takes it.
    fn call_tokens(&mut self, f: &str, args: &[Expr]) -> Gened<Code> {
        let callee = self
            .sigs
            .get(f)
            .ok_or_else(|| unsupported(format!("no function {f}")))?;
        let name = snake_ident(f);
        let mut out = Vec::new();
        let mut exits = false;
        for (a, (_, pt)) in args.iter().zip(&callee.params) {
            let c = if by_ref(pt) {
                self.borrow(a, pt)?
            } else {
                self.value(a, pt)?
            };
            exits = exits || c.exits;
            out.push(c.t);
        }
        Ok(Code::new(quote! { #name(#(#out),*) }, Prec::Postfix).exits(exits))
    }

    fn call(&mut self, f: &str, args: &[Expr]) -> Gened<Code> {
        let call = self.call_tokens(f, args)?;
        let callee = &self.sigs[f];
        let t = &call.t;
        Ok(match (&callee.errors, callee.faultable) {
            (_, false) => call,
            (None, true) => {
                if self.me.errors.is_some() {
                    Code::new(quote! { #t.map_err(Failure::Fault)? }, Prec::Postfix).exits(true)
                } else {
                    Code::new(quote! { #t? }, Prec::Postfix).exits(true)
                }
            }
            (Some(_), true) => {
                let lift = if self.me.errors.is_some() {
                    quote! { Failure::Fault(fault_) }
                } else {
                    quote! { fault_ }
                };
                Code::new(
                    quote! {
                        match #t {
                            Ok(value_) => Ok(value_),
                            Err(Failure::Error(error_)) => Err(error_),
                            Err(Failure::Fault(fault_)) => return Err(#lift),
                        }
                    },
                    Prec::Block,
                )
                .exits(true)
            }
        })
    }

    fn attempt(&mut self, a: &Expr) -> Gened<Code> {
        if let Kind::Call(f, args) = &a.kind {
            let call = self.call_tokens(f, args)?;
            let callee = &self.sigs[f.as_str()];
            let t = &call.t;
            let code = if !callee.faultable && self.me.faultable {
                quote! { #t.map_err(Failure::Error)? }
            } else {
                quote! { #t? }
            };
            return Ok(Code::new(code, Prec::Postfix).exits(true));
        }
        let ty = concretize(&a.ty(), None);
        let c = self.value(a, &ty)?;
        let r = c.recv();
        let code = if self.me.faultable {
            quote! { #r.map_err(Failure::Error)? }
        } else {
            quote! { #r? }
        };
        Ok(Code::new(code, Prec::Postfix).exits(true))
    }

    /// `Err(<error>)` of a `fail`.
    fn fail_value(&mut self, op: &Op) -> Gened<TokenStream> {
        let Some(errors) = self.me.errors.clone() else {
            return Err(unsupported("fail in a function without errors"));
        };
        let v = self.value(&op.args[0], &Ty::Union(errors))?;
        let t = &v.t;
        Ok(if self.me.faultable {
            quote! { Err(Failure::Error(#t)) }
        } else {
            quote! { Err(#t) }
        })
    }

    // --- operations ------------------------------------------------------------------------------

    /// An iterator over `&T` of the items of a list, or of a map's keys or values.
    fn iter_source(&mut self, src: &Expr) -> Gened<(Code, Ty)> {
        if let Kind::Op(op) = &src.kind {
            if op.binder.is_none() && (op.name == "values" || op.name == "keys") {
                let m = self.raw(&op.args[0], None)?;
                let mt = concretize(&op.args[0].ty(), None);
                let Ty::Map(k, v) = mt else {
                    return Err(unsupported(format!("{} of {}", op.name, mt.text())));
                };
                let r = m.recv();
                let (method, of) = if op.name == "values" {
                    (quote! { values }, *v)
                } else {
                    (quote! { keys }, *k)
                };
                return Ok((
                    Code::new(quote! { #r.#method() }, Prec::Postfix).exits(m.exits),
                    of,
                ));
            }
        }
        let st = concretize(&src.ty(), None);
        // A set iterates in its order, which is the order Exegra holds it in: its items are
        // sorted and distinct wherever it is read from (an example, a declared field).
        let of = match &st {
            Ty::List(of) | Ty::Set(of) => (**of).clone(),
            _ => return Err(unsupported(format!("a list operation over {}", st.text()))),
        };
        let c = self.raw(src, Some(&st))?;
        let c = if c.form == Form::OptRef {
            self.own(c, &st)
        } else {
            c
        };
        let r = c.recv();
        Ok((
            Code::new(quote! { #r.iter() }, Prec::Postfix).exits(c.exits),
            of,
        ))
    }

    fn operation(&mut self, op: &Op, eff: &Ty) -> Gened<Code> {
        if op.binder.is_some() {
            return self.binder_operation(op, eff);
        }
        match op.name.as_str() {
            "take" => {
                let Ty::List(of) = eff else {
                    return Err(unsupported("take of something that is not a list"));
                };
                let (it, _) = self.iter_source(&op.args[0])?;
                let count = match const_int(&op.args[1]) {
                    Some(v) => {
                        let n = Literal::usize_unsuffixed(
                            usize::try_from(v.max(0)).unwrap_or(usize::MAX),
                        );
                        Code::new(quote! { #n }, Prec::Postfix)
                    }
                    None => {
                        let IntOpd::Code(c, _) = self.int_operand(&op.args[1])? else {
                            return Err(unsupported("the count of take"));
                        };
                        self.helpers.insert("count");
                        let t = &c.t;
                        Code::new(quote! { support::count(i128::from(#t)) }, Prec::Postfix)
                            .exits(c.exits)
                    }
                };
                let (itt, ct) = (&it.t, &count.t);
                let copy = if is_copy(of) {
                    quote! { copied }
                } else {
                    quote! { cloned }
                };
                Ok(Code::new(
                    quote! { #itt.take(#ct).#copy().collect::<Vec<_>>() },
                    Prec::Postfix,
                )
                .exits(it.exits || count.exits))
            }
            "concat" => {
                let (a, b) = (&op.args[0], &op.args[1]);
                if is_empty_collection(a) {
                    return self.value(b, eff);
                }
                if is_empty_collection(b) {
                    return self.value(a, eff);
                }
                let Ty::List(of) = eff else {
                    return Err(unsupported("concat of something that is not a list"));
                };
                let sa = self.slice(a, eff, of)?;
                let sb = self.slice(b, eff, of)?;
                self.helpers.insert("concat");
                let (at, bt) = (&sa.t, &sb.t);
                Ok(
                    Code::new(quote! { support::concat(#at, #bt) }, Prec::Postfix)
                        .exits(sa.exits || sb.exits),
                )
            }
            "keys" | "values" => {
                let (it, of) = self.iter_source(&Expr {
                    kind: Kind::Op(op.clone()),
                    pos: 0,
                    ty: Some(eff.clone()),
                })?;
                let t = &it.t;
                let copy = if is_copy(&of) {
                    quote! { copied }
                } else {
                    quote! { cloned }
                };
                Ok(
                    Code::new(quote! { #t.#copy().collect::<Vec<_>>() }, Prec::Postfix)
                        .exits(it.exits),
                )
            }
            "get" => self.get(op),
            "insert" | "remove" => {
                let Ty::Map(k, v) = eff else {
                    return Err(unsupported(format!(
                        "{} on something that is not a map",
                        op.name
                    )));
                };
                let m = self.value(&op.args[0], eff)?;
                let mt = &m.t;
                if op.name == "insert" {
                    let kc = self.value(&op.args[1], k)?;
                    let vc = self.value(&op.args[2], v)?;
                    let (kt, vt) = (&kc.t, &vc.t);
                    Ok(Code::new(
                        quote! { { let mut map_ = #mt; map_.insert(#kt, #vt); map_ } },
                        Prec::Block,
                    )
                    .exits(m.exits || kc.exits || vc.exits))
                } else {
                    let kc = self.lookup_key(&op.args[1], k)?;
                    let kt = &kc.t;
                    Ok(Code::new(
                        quote! { { let mut map_ = #mt; map_.remove(#kt); map_ } },
                        Prec::Block,
                    )
                    .exits(m.exits || kc.exits))
                }
            }
            "format" => {
                let Kind::Str(text) = &op.args[0].kind else {
                    return Err(unsupported("format without a literal"));
                };
                let mut pieces = Vec::new();
                let mut arg = 1;
                for (part, hole) in format_parts(text) {
                    if !part.is_empty() {
                        pieces.push(Piece::Text(part));
                    }
                    let Some(hole) = hole else { continue };
                    let e = op
                        .args
                        .get(arg)
                        .ok_or_else(|| unsupported("format has more placeholders than values"))?;
                    arg += 1;
                    pieces.push(match hole {
                        Hole::Display => self.piece(e)?,
                        Hole::Debug => self.debug_piece(e)?,
                        Hole::Json => self.json_piece(e)?,
                    });
                }
                Ok(self.format(pieces))
            }
            "eval" | "check" => {
                let Kind::Ref(path, _) = &op.args[0].kind else {
                    return Err(unsupported("eval without a language"));
                };
                let lang = &path[0];
                let lang_decl = self
                    .spec
                    .languages
                    .iter()
                    .find(|l| &l.id == lang)
                    .ok_or_else(|| unsupported(format!("no language {lang}")))?;
                let bridge = crate::language::bridge(self.spec, lang_decl)
                    .ok_or_else(|| unsupported(format!("the language {lang} is refused")))?;
                let ast = Ty::Union(bridge.ast_id().to_string());
                let env_ty = concretize(&op.args[2].ty(), None);
                let Ty::Struct(env) = &env_ty else {
                    return Err(unsupported("an environment that is not a struct"));
                };
                let lookup = op.name == "eval" && op.args.len() == 4;
                let name = bridge_name(op.name == "eval", lookup, lang, env);
                let e = self.borrow(&op.args[1], &ast)?;
                let en = self.borrow(&op.args[2], &env_ty)?;
                let (et, ent) = (&e.t, &en.t);
                if lookup {
                    // The fourth argument: the lookup a reference is followed through.
                    let want = super::typing::lookup_ty(&Ty::Union(bridge.values_id().to_string()));
                    let l = self.borrow(&op.args[3], &want)?;
                    let lt = &l.t;
                    return Ok(
                        Code::new(quote! { support::#name(#et, #ent, #lt) }, Prec::Postfix)
                            .exits(e.exits || en.exits || l.exits),
                    );
                }
                Ok(
                    Code::new(quote! { support::#name(#et, #ent) }, Prec::Postfix)
                        .exits(e.exits || en.exits),
                )
            }
            "print" => {
                // The language module implements `Display` for its AST type with the printer
                // of the language's authoring syntax (`language::Plan::printer`).
                let Kind::Ref(path, _) = &op.args[0].kind else {
                    return Err(unsupported("print without a language"));
                };
                let lang = &path[0];
                let lang_decl = self
                    .spec
                    .languages
                    .iter()
                    .find(|l| &l.id == lang)
                    .ok_or_else(|| unsupported(format!("no language {lang}")))?;
                let bridge = crate::language::bridge(self.spec, lang_decl)
                    .ok_or_else(|| unsupported(format!("the language {lang} is refused")))?;
                let ast = Ty::Union(bridge.ast_id().to_string());
                let e = self.borrow(&op.args[1], &ast)?;
                let et = &e.t;
                Ok(Code::new(
                    quote! { std::string::ToString::to_string(#et) },
                    Prec::Postfix,
                )
                .exits(e.exits))
            }
            "fail" => {
                let v = self.fail_value(op)?;
                Ok(Code::new(quote! { return #v }, Prec::Block).exits(true))
            }
            "startsWith" | "endsWith" => {
                let (a, b) = (self.str_ref(&op.args[0])?, self.str_ref(&op.args[1])?);
                let method = if op.name == "startsWith" {
                    quote! { starts_with }
                } else {
                    quote! { ends_with }
                };
                let (at, bt) = (a.recv(), &b.t);
                Ok(Code::new(quote! { #at.#method(#bt) }, Prec::Postfix).exits(a.exits || b.exits))
            }
            "length" => {
                let ty = concretize(&op.args[0].ty(), None);
                if matches!(unshared(&ty), Ty::List(_) | Ty::Set(_)) {
                    let c = self.raw(&op.args[0], None)?;
                    let r = c.recv();
                    return Ok(Code::new(
                        quote! { u64::try_from(#r.len()).unwrap_or(u64::MAX) },
                        Prec::Postfix,
                    )
                    .exits(c.exits));
                }
                // A string's length in characters (Unicode scalar values), as Exegra counts.
                let c = self.str_ref(&op.args[0])?;
                let r = c.recv();
                Ok(Code::new(
                    quote! { u64::try_from(#r.chars().count()).unwrap_or(u64::MAX) },
                    Prec::Postfix,
                )
                .exits(c.exits))
            }
            // str::trim removes Rust's White_Space, which is the set Exegra's trim removes.
            "trim" => {
                let c = self.str_ref(&op.args[0])?;
                let r = c.recv();
                Ok(Code::new(quote! { #r.trim().to_string() }, Prec::Postfix).exits(c.exits))
            }
            "chars" => {
                let c = self.str_ref(&op.args[0])?;
                let r = c.recv();
                Ok(Code::new(
                    quote! { #r.chars().map(String::from).collect::<Vec<_>>() },
                    Prec::Postfix,
                )
                .exits(c.exits))
            }
            "snakeCase" | "parseTime" => {
                let c = self.str_ref(&op.args[0])?;
                let (helper, name) = if op.name == "snakeCase" {
                    ("snake_case", quote! { snake_case })
                } else {
                    ("parse_time", quote! { parse_time })
                };
                self.helpers.insert(helper);
                let t = &c.t;
                Ok(Code::new(quote! { support::#name(#t) }, Prec::Postfix).exits(c.exits))
            }
            "isAsciiLowercase"
            | "isAsciiUppercase"
            | "isAsciiAlphabetic"
            | "isAsciiDigit"
            | "isAsciiAlphanumeric" => {
                let test = match op.name.as_str() {
                    "isAsciiLowercase" => quote! { is_ascii_lowercase },
                    "isAsciiUppercase" => quote! { is_ascii_uppercase },
                    "isAsciiAlphabetic" => quote! { is_ascii_alphabetic },
                    "isAsciiDigit" => quote! { is_ascii_digit },
                    _ => quote! { is_ascii_alphanumeric },
                };
                let c = self.str_ref(&op.args[0])?;
                self.helpers.insert("every_char");
                let t = &c.t;
                Ok(Code::new(
                    quote! { support::every_char(#t, char::#test) },
                    Prec::Postfix,
                )
                .exits(c.exits))
            }
            other => Err(unsupported(format!("the operation {other}"))),
        }
    }

    /// The `&K` a map is looked up (or a key removed) by. A string (or an id of another type)
    /// looking up a map of constrained ids is built as that id without the check: it is never
    /// stored, and a value that breaks the constraints is a key no map holds, so the lookup
    /// finds nothing, as Exegra's does.
    fn lookup_key(&mut self, e: &Expr, k: &Ty) -> Gened<Code> {
        let et = concretize(&e.ty(), Some(k));
        if let Ty::Alias(id, repr) = k {
            if **repr == Ty::Str && is_constrained(self.spec, id) && !same(&et, k) {
                let v = self.value(e, &Ty::Str)?;
                let p = type_path(id);
                let t = &v.t;
                return Ok(Code::new(quote! { &#p(#t) }, Prec::Prefix).exits(v.exits));
            }
        }
        self.borrow(e, k)
    }

    /// A `&str` of a string, or of an id or scalar over one: a literal as itself.
    fn str_ref(&mut self, e: &Expr) -> Gened<Code> {
        if let Kind::Str(s) = &e.kind {
            let lit = Literal::string(s);
            return Ok(Code::new(quote! { #lit }, Prec::Postfix));
        }
        let ty = concretize(&e.ty(), None);
        let c = self.raw(e, None)?;
        let r = c.recv();
        let t = match &ty {
            Ty::Alias(..) => quote! { #r.0.as_str() },
            Ty::Str => quote! { #r.as_str() },
            other => {
                return Err(unsupported(format!(
                    "a string operation on {}, which is not a string",
                    other.text()
                )))
            }
        };
        Ok(Code::new(t, Prec::Postfix).exits(c.exits))
    }

    /// An argument of `concat`: a list literal as `&[..]`, anything else borrowed.
    fn slice(&mut self, e: &Expr, list: &Ty, of: &Ty) -> Gened<Code> {
        if let Kind::List(items) = &e.kind {
            let mut out = Vec::new();
            let mut exits = false;
            for item in items {
                let v = self.value(item, of)?;
                exits = exits || v.exits;
                out.push(v.t);
            }
            return Ok(Code::new(quote! { &[#(#out),*] }, Prec::Prefix).exits(exits));
        }
        self.borrow(e, list)
    }

    fn get(&mut self, op: &Op) -> Gened<Code> {
        let ct = concretize(&op.args[0].ty(), None);
        let c = self.raw(&op.args[0], None)?;
        let r = c.recv();
        match &ct {
            Ty::Map(k, _) => {
                let key = if let (Kind::Str(s), Ty::Str) = (&op.args[1].kind, k.as_ref()) {
                    let lit = Literal::string(s);
                    Code::new(quote! { #lit }, Prec::Postfix)
                } else {
                    self.lookup_key(&op.args[1], k)?
                };
                let kt = &key.t;
                Ok(Code::new(quote! { #r.get(#kt) }, Prec::Postfix)
                    .form(Form::OptRef)
                    .exits(c.exits || key.exits))
            }
            Ty::List(of) => {
                if let Some(v) = const_int(&op.args[1]) {
                    if v < 0 {
                        return Ok(Code::new(quote! { None }, Prec::Postfix));
                    }
                    let n = Literal::usize_unsuffixed(usize::try_from(v).unwrap_or(usize::MAX));
                    return Ok(Code::new(quote! { #r.get(#n) }, Prec::Postfix)
                        .form(Form::OptRef)
                        .exits(c.exits));
                }
                let IntOpd::Code(i, _) = self.int_operand(&op.args[1])? else {
                    return Err(unsupported("the index of get"));
                };
                let it = &i.t;
                let copy = if is_copy(of) {
                    quote! { copied }
                } else {
                    quote! { cloned }
                };
                Ok(Code::new(
                    quote! {
                        match usize::try_from(#it) {
                            Ok(index_) => #r.get(index_).#copy(),
                            Err(_) => None,
                        }
                    },
                    Prec::Block,
                )
                .exits(c.exits || i.exits))
            }
            // The item at an index of a set, in its order.
            Ty::Set(of) => {
                if let Some(v) = const_int(&op.args[1]) {
                    if v < 0 {
                        return Ok(Code::new(quote! { None }, Prec::Postfix));
                    }
                    let n = Literal::usize_unsuffixed(usize::try_from(v).unwrap_or(usize::MAX));
                    return Ok(Code::new(quote! { #r.iter().nth(#n) }, Prec::Postfix)
                        .form(Form::OptRef)
                        .exits(c.exits));
                }
                let IntOpd::Code(i, _) = self.int_operand(&op.args[1])? else {
                    return Err(unsupported("the index of get"));
                };
                let it = &i.t;
                let copy = if is_copy(of) {
                    quote! { copied }
                } else {
                    quote! { cloned }
                };
                Ok(Code::new(
                    quote! {
                        match usize::try_from(#it) {
                            Ok(index_) => #r.iter().nth(index_).#copy(),
                            Err(_) => None,
                        }
                    },
                    Prec::Block,
                )
                .exits(c.exits || i.exits))
            }
            other => Err(unsupported(format!("get on {}", other.text()))),
        }
    }

    fn binder_operation(&mut self, op: &Op, eff: &Ty) -> Gened<Code> {
        let (bname, src) = op.binder.as_ref().expect("a binder operation");
        let (it, of) = self.iter_source(src)?;
        let name = op.name.as_str();
        let used = uses(&op.args[0], bname);
        if name == "fold" {
            return self.fold(op, it, of, eff);
        }
        let x = if used {
            let i = self.bind(bname, of.clone(), Form::Ref)?;
            quote! { #i }
        } else {
            quote! { _ }
        };
        let body_ty = match name {
            "map" => match eff {
                Ty::List(t) => (**t).clone(),
                _ => return Err(unsupported("map that does not give a list")),
            },
            "sortBy" => concretize(&op.args[0].ty(), None),
            _ => Ty::Bool,
        };
        let body = if name == "sortBy" {
            self.sort_key(&op.args[0], &body_ty)
        } else {
            self.value(&op.args[0], &body_ty)
        };
        if used {
            self.unbind(1);
        }
        let body = body?;
        let itt = &it.t;
        let bt = &body.t;
        let copy = if is_copy(&of) {
            quote! { copied }
        } else {
            quote! { cloned }
        };
        let own_x = if !used {
            TokenStream::new()
        } else if is_copy(&of) {
            quote! { *#x }
        } else {
            quote! { #x.clone() }
        };
        let fallible = body.exits;
        let code = match (name, fallible) {
            ("map", false) => quote! { #itt.map(|#x| #bt).collect::<Vec<_>>() },
            ("map", true) => quote! {
                {
                    let mut out_ = Vec::new();
                    for #x in #itt {
                        out_.push(#bt);
                    }
                    out_
                }
            },
            ("filter", false) => {
                let pat = if used {
                    quote! { &#x }
                } else {
                    quote! { _ }
                };
                quote! { #itt.filter(|#pat| #bt).#copy().collect::<Vec<_>>() }
            }
            ("filter", true) => {
                let item = if used {
                    own_x.clone()
                } else {
                    quote! { item_.clone() }
                };
                let var = if used {
                    x.clone()
                } else {
                    quote! { item_ }
                };
                quote! {
                    {
                        let mut out_ = Vec::new();
                        for #var in #itt {
                            if #bt {
                                out_.push(#item);
                            }
                        }
                        out_
                    }
                }
            }
            ("find", false) => {
                let pat = if used {
                    quote! { &#x }
                } else {
                    quote! { _ }
                };
                return Ok(Code::new(quote! { #itt.find(|#pat| #bt) }, Prec::Postfix)
                    .form(Form::OptRef)
                    .exits(it.exits));
            }
            ("find", true) => {
                let item = if used {
                    own_x.clone()
                } else {
                    quote! { item_.clone() }
                };
                let var = if used {
                    x.clone()
                } else {
                    quote! { item_ }
                };
                quote! {
                    {
                        let mut found_ = None;
                        for #var in #itt {
                            if #bt {
                                found_ = Some(#item);
                                break;
                            }
                        }
                        found_
                    }
                }
            }
            ("any", false) => quote! { #itt.any(|#x| #bt) },
            ("all", false) => quote! { #itt.all(|#x| #bt) },
            ("any", true) => quote! {
                {
                    let mut any_ = false;
                    for #x in #itt {
                        if #bt {
                            any_ = true;
                            break;
                        }
                    }
                    any_
                }
            },
            ("all", true) => {
                let neg = body.at(Prec::Prefix);
                quote! {
                    {
                        let mut all_ = true;
                        for #x in #itt {
                            if !#neg {
                                all_ = false;
                                break;
                            }
                        }
                        all_
                    }
                }
            }
            ("sortBy", false) => {
                self.helpers.insert("sorted_by");
                quote! { support::sorted_by(#itt, |#x| #bt) }
            }
            ("sortBy", true) => {
                self.helpers.insert("sort_keyed");
                let item = if used {
                    own_x.clone()
                } else {
                    quote! { item_.clone() }
                };
                let var = if used {
                    x.clone()
                } else {
                    quote! { item_ }
                };
                quote! {
                    {
                        let mut keyed_ = Vec::new();
                        for #var in #itt {
                            keyed_.push((#bt, #item));
                        }
                        support::sort_keyed(keyed_)
                    }
                }
            }
            (other, _) => return Err(unsupported(format!("the operation {other}"))),
        };
        let prec = if fallible { Prec::Block } else { Prec::Postfix };
        Ok(Code::new(code, prec).exits(it.exits || fallible))
    }

    /// A sort key: numbers and strings as they are, an enum value by its name, an id or scalar
    /// by its representation.
    fn sort_key(&mut self, e: &Expr, ty: &Ty) -> Gened<Code> {
        if *ty == Ty::Float {
            return Err(unsupported(
                "sortBy with an f64 key: an f64 has no total order to sort by in Rust",
            ));
        }
        match ty {
            Ty::Enum(_) => {
                let c = self.raw(e, None)?;
                let r = c.recv();
                Ok(Code::new(quote! { #r.as_str() }, Prec::Postfix).exits(c.exits))
            }
            // A number key is compared as the number: an i128, which holds every integer type,
            // so branches of different integer types (a record id, an i64) compare as Exegra's
            // numbers do.
            _ if class(ty) == Some(Class::Num) => self.value(e, &Ty::Wide),
            // A string key, an id's included, is compared as its string.
            _ if class(ty) == Some(Class::Str) => self.value(e, &Ty::Str),
            _ => {
                let c = self.raw(e, None)?;
                Ok(self.own(c, ty))
            }
        }
    }

    fn fold(&mut self, op: &Op, it: Code, of: Ty, eff: &Ty) -> Gened<Code> {
        let (bname, _) = op.binder.as_ref().expect("fold binds a name");
        let (acc_name, init) = op.acc.as_ref().expect("fold has an accumulator");
        if !eff.is_concrete() {
            return Err(unsupported(format!(
                "the type of a fold ({}) cannot be told",
                eff.text()
            )));
        }
        let init = self.value(init, eff)?;
        let step = &op.args[0];
        let x_used = uses(step, bname) && bname != acc_name;
        let acc_used = uses(step, acc_name);
        let mut bound = 0;
        let acc = if acc_used {
            bound += 1;
            let i = self.bind(acc_name, eff.clone(), Form::Place)?;
            quote! { #i }
        } else {
            quote! { _ }
        };
        let x = if x_used {
            bound += 1;
            let i = self.bind(bname, of, Form::Ref)?;
            quote! { #i }
        } else {
            quote! { _ }
        };
        let body = self.value(step, eff);
        self.unbind(bound);
        let body = body?;
        let (itt, it_, bt) = (&it.t, &init.t, &body.t);
        if !body.exits {
            return Ok(
                Code::new(quote! { #itt.fold(#it_, |#acc, #x| #bt) }, Prec::Postfix)
                    .exits(it.exits || init.exits),
            );
        }
        let acc_var = if acc_used {
            acc
        } else {
            quote! { acc_ }
        };
        // The accumulator's type is written out: an integer literal start (`n = 0`) has no type
        // of its own, and `checked_add` on it would not resolve.
        let acc_ty = rust_ty(eff)?;
        Ok(Code::new(
            quote! {
                {
                    let mut #acc_var: #acc_ty = #it_;
                    for #x in #itt {
                        #acc_var = #bt;
                    }
                    #acc_var
                }
            },
            Prec::Block,
        )
        .exits(true))
    }
}

/// The name of the `support` function that evaluates or checks a language on an environment
/// struct; `eval_lookup_..` for an `eval` given a lookup.
pub(crate) fn bridge_name(eval: bool, lookup: bool, language: &str, env: &str) -> Ident {
    let what = match (eval, lookup) {
        (true, true) => "eval_lookup",
        (true, false) => "eval",
        (false, _) => "check",
    };
    format_ident!(
        "{what}_{}_{}",
        naming::snake(language).unwrap_or_default(),
        naming::snake(env).unwrap_or_default()
    )
}

/// The `support` helpers.
pub(crate) fn helper(name: &str) -> TokenStream {
    match name {
        "concat" => quote! {
            /// The items of `a`, then those of `b`.
            pub(super) fn concat<T: Clone>(a: &[T], b: &[T]) -> Vec<T> {
                let mut out = a.to_vec();
                out.extend_from_slice(b);
                out
            }
        },
        "snake_case" => quote! {
            /// Exegra's snakeCase (the kernel's snake_case of a PascalCase name): every ASCII
            /// capital becomes its lower case, after a `_` unless it is the first character.
            pub(super) fn snake_case(name: &str) -> String {
                let mut out = String::new();
                for (i, c) in name.chars().enumerate() {
                    if c.is_ascii_uppercase() {
                        if i > 0 {
                            out.push('_');
                        }
                        out.push(c.to_ascii_lowercase());
                    } else {
                        out.push(c);
                    }
                }
                out
            }
        },
        "every_char" => quote! {
            /// Whether a string has a character and every one passes the test (Exegra's
            /// isAscii* operations).
            pub(super) fn every_char(s: &str, test: fn(&char) -> bool) -> bool {
                !s.is_empty() && s.chars().all(|c| test(&c))
            }
        },
        "parse_time" => quote! {
            /// Exegra's parseTime, which is substrate's parse_time: `YYYY-MM-DD` or
            /// `YYYY-MM-DDTHH:MM[:SS]Z` (UTC) as milliseconds since the epoch. The date and time
            /// split at the first `T`; the year is four bytes; each number is read as
            /// `i64::from_str` reads it; month 1..=12, day 1..=31, hour 0..24, minute and
            /// second 0..60. It computes in i128, so a time beyond the i64 milliseconds is
            /// refused rather than wrapped.
            pub(super) fn parse_time(text: &str) -> Result<i64, String> {
                let bad = || format!("not a UTC timestamp: {text:?}");
                let number = |s: &str| s.parse::<i64>().map(i128::from).map_err(|_| bad());
                let (date, time) = match text.split_once('T') {
                    Some((date, time)) => (date, Some(time.strip_suffix('Z').ok_or_else(bad)?)),
                    None => (text, None),
                };
                let parts: Vec<&str> = date.split('-').collect();
                if parts.len() != 3 || parts[0].len() != 4 {
                    return Err(bad());
                }
                let (year, month, day) = (number(parts[0])?, number(parts[1])?, number(parts[2])?);
                if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
                    return Err(bad());
                }
                let y = if month <= 2 { year - 1 } else { year };
                let era = if y >= 0 { y } else { y - 399 } / 400;
                let yoe = y - era * 400;
                let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
                let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
                let mut seconds = (era * 146_097 + doe - 719_468) * 86_400;
                if let Some(time) = time {
                    let parts: Vec<&str> = time.split(':').collect();
                    if !(2..=3).contains(&parts.len()) {
                        return Err(bad());
                    }
                    let (hour, minute) = (number(parts[0])?, number(parts[1])?);
                    let second = if parts.len() == 3 { number(parts[2])? } else { 0 };
                    if !(0..24).contains(&hour)
                        || !(0..60).contains(&minute)
                        || !(0..60).contains(&second)
                    {
                        return Err(bad());
                    }
                    seconds += hour * 3_600 + minute * 60 + second;
                }
                i64::try_from(seconds * 1_000).map_err(|_| bad())
            }
        },
        "count" => quote! {
            /// How many items `take` keeps: none for a count below one.
            pub(super) fn count(n: i128) -> usize {
                if n <= 0 {
                    0
                } else {
                    usize::try_from(n).unwrap_or(usize::MAX)
                }
            }
        },
        "sorted_by" => quote! {
            /// The items ordered by a key, stably.
            pub(super) fn sorted_by<'a, T: Clone + 'a, K: Ord>(
                items: impl Iterator<Item = &'a T>,
                key: impl FnMut(&T) -> K,
            ) -> Vec<T> {
                let mut out: Vec<T> = items.cloned().collect();
                out.sort_by_key(key);
                out
            }
        },
        "sort_keyed" => quote! {
            /// The items of `(key, item)` pairs ordered by their keys, stably.
            pub(super) fn sort_keyed<K: Ord + Clone, T>(mut keyed: Vec<(K, T)>) -> Vec<T> {
                keyed.sort_by_key(|pair| pair.0.clone());
                keyed.into_iter().map(|(_, item)| item).collect()
            }
        },
        _ => TokenStream::new(),
    }
}
