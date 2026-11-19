//! Types of the body language, as Exegra types them (`typeFunction` in `src/core/function.js`),
//! restricted to what specc compiles: booleans, strings, the integer types, ids and scalars over
//! those, enums, structs, unions, lists, options, maps and results. `f64`, `timestamp`,
//! `duration`, `bytes`, `uuid`, `json` and sets are refused (`specc.function-unsupported`).
//!
//! Every node is annotated with its type, which the generator reads. A fold's step is typed a
//! second time with the accumulator at the fold's joined type, so `acc = []` gets its item type.

use std::collections::BTreeMap;

use super::syntax::{Arm, BinOp, Expr, Kind, Op, Pattern, Res, UnOp};
use super::{FUNCTION_INVALID, FUNCTION_UNSUPPORTED};
use crate::ir::{Spec, StructField, TypeDecl, TypeKind};
use crate::resolve::{Builtin, TypeRef};

/// A type of the body language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Ty {
    Bool,
    Str,
    /// An integer built-in: `u8`, `u16`, `u32`, `u64`, `i32` or `i64`.
    Int(Builtin),
    /// A number literal, which takes the integer type of where it is used.
    Number,
    /// An id or scalar type over a bool, string or integer representation.
    Alias(String, Box<Ty>),
    Enum(String),
    Struct(String),
    Union(String),
    List(Box<Ty>),
    Option(Box<Ty>),
    Map(Box<Ty>, Box<Ty>),
    Result(Box<Ty>, Box<Ty>),
    /// The type of `fail`, which fits anything.
    Never,
    /// The item of `[]` and `{}`, the inner type of `null`, which fits anything.
    Unknown,
}

impl Ty {
    pub(crate) fn list(of: Ty) -> Ty {
        Ty::List(Box::new(of))
    }

    pub(crate) fn option(of: Ty) -> Ty {
        Ty::Option(Box::new(of))
    }

    pub(crate) fn map(k: Ty, v: Ty) -> Ty {
        Ty::Map(Box::new(k), Box::new(v))
    }

    /// The type as the specification writes it.
    pub(crate) fn text(&self) -> String {
        match self {
            Ty::Bool => "bool".into(),
            Ty::Str => "string".into(),
            Ty::Int(b) => b.as_str().into(),
            Ty::Number => "a number".into(),
            Ty::Alias(id, _) | Ty::Enum(id) | Ty::Struct(id) | Ty::Union(id) => id.clone(),
            Ty::List(of) => format!("list<{}>", of.text()),
            Ty::Option(of) => format!("option<{}>", of.text()),
            Ty::Map(k, v) => format!("map<{}, {}>", k.text(), v.text()),
            Ty::Result(o, e) => format!("result<{}, {}>", o.text(), e.text()),
            Ty::Never => "never".into(),
            Ty::Unknown => "unknown".into(),
        }
    }

    /// True when nothing in the type is `Unknown` or `Number`.
    pub(crate) fn is_concrete(&self) -> bool {
        match self {
            Ty::Number | Ty::Unknown => false,
            Ty::List(of) | Ty::Option(of) => of.is_concrete(),
            Ty::Map(k, v) | Ty::Result(k, v) => k.is_concrete() && v.is_concrete(),
            Ty::Alias(_, r) => r.is_concrete(),
            _ => true,
        }
    }
}

/// The scalar class two types share: what Exegra compares and passes interchangeably.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    Bool,
    Str,
    Num,
}

pub(crate) fn class(t: &Ty) -> Option<Class> {
    match t {
        Ty::Bool => Some(Class::Bool),
        Ty::Str => Some(Class::Str),
        Ty::Int(_) | Ty::Number => Some(Class::Num),
        Ty::Alias(_, r) => class(r),
        _ => None,
    }
}

pub(crate) fn is_number(t: &Ty) -> bool {
    class(t) == Some(Class::Num)
}

fn is_open(t: &Ty) -> bool {
    matches!(t, Ty::Unknown | Ty::Never)
}

/// The integer built-in a number type is held in: its own, an alias's representation's.
pub(crate) fn int_kind(t: &Ty) -> Option<Builtin> {
    match t {
        Ty::Int(b) => Some(*b),
        Ty::Alias(_, r) => int_kind(r),
        _ => None,
    }
}

/// Whether a value of type `from` may stand where `to` is expected (Exegra's `assignable`).
pub(crate) fn assignable(from: &Ty, to: &Ty) -> bool {
    if is_open(from) || is_open(to) {
        return true;
    }
    if let Ty::Option(t) = to {
        return match from {
            Ty::Option(f) => assignable(f, t),
            _ => assignable(from, t),
        };
    }
    if matches!(from, Ty::Option(_)) {
        return false;
    }
    let (fc, tc) = (class(from), class(to));
    if fc.is_some() || tc.is_some() {
        return fc == tc;
    }
    match (from, to) {
        (Ty::List(a), Ty::List(b)) => assignable(a, b),
        (Ty::Map(ak, av), Ty::Map(bk, bv)) => assignable(ak, bk) && assignable(av, bv),
        (Ty::Result(ao, ae), Ty::Result(bo, be)) => assignable(ao, bo) && assignable(ae, be),
        (Ty::Enum(a), Ty::Enum(b))
        | (Ty::Struct(a), Ty::Struct(b))
        | (Ty::Union(a), Ty::Union(b)) => a == b,
        _ => false,
    }
}

fn comparable(a: &Ty, b: &Ty) -> bool {
    assignable(a, b) || assignable(b, a)
}

/// The type of two branches together (Exegra's `join`), or `None` when they do not fit.
pub(crate) fn join(a: &Ty, b: &Ty) -> Option<Ty> {
    if is_open(a) {
        return Some(if *b == Ty::Never && *a == Ty::Unknown {
            a.clone()
        } else {
            b.clone()
        });
    }
    if is_open(b) {
        return Some(a.clone());
    }
    if matches!(a, Ty::Option(_)) || matches!(b, Ty::Option(_)) {
        let ai = match a {
            Ty::Option(x) => x,
            _ => a,
        };
        let bi = match b {
            Ty::Option(x) => x,
            _ => b,
        };
        return join(ai, bi).map(Ty::option);
    }
    match (a, b) {
        (Ty::List(x), Ty::List(y)) => return join(x, y).map(Ty::list),
        (Ty::Map(ak, av), Ty::Map(bk, bv)) => {
            return Some(Ty::map(join(ak, bk)?, join(av, bv)?));
        }
        (Ty::Result(ao, ae), Ty::Result(bo, be)) => {
            return Some(Ty::Result(Box::new(join(ao, bo)?), Box::new(join(ae, be)?)));
        }
        _ => {}
    }
    if !assignable(a, b) || !assignable(b, a) {
        return None;
    }
    Some(if *a == Ty::Number {
        b.clone()
    } else {
        a.clone()
    })
}

/// Why specc refuses a function.
#[derive(Debug, Clone)]
pub(crate) struct Refusal {
    pub code: &'static str,
    pub message: String,
}

/// A finding Exegra also reports: the specification is wrong.
pub(crate) fn invalid(message: impl Into<String>) -> Refusal {
    Refusal {
        code: FUNCTION_INVALID,
        message: message.into(),
    }
}

/// A function Exegra accepts that specc cannot compile.
pub(crate) fn unsupported(message: impl Into<String>) -> Refusal {
    Refusal {
        code: FUNCTION_UNSUPPORTED,
        message: message.into(),
    }
}

/// The type of a type text: a type of the format or `result<T, E>`.
pub(crate) fn ty_of_text(spec: &Spec, text: &str) -> Result<Ty, Refusal> {
    let text = text.trim();
    if let Some(inner) = text
        .strip_prefix("result<")
        .and_then(|t| t.strip_suffix('>'))
    {
        let mut depth = 0i32;
        let mut split = None;
        for (i, c) in inner.char_indices() {
            match c {
                '<' => depth += 1,
                '>' => depth -= 1,
                ',' if depth == 0 && split.is_none() => split = Some(i),
                _ => {}
            }
        }
        let Some(i) = split else {
            return Err(invalid(format!("\"{text}\" is not result<T, E>")));
        };
        let ok = ty_of_text(spec, &inner[..i])?;
        let err = ty_of_text(spec, &inner[i + 1..])?;
        return Ok(Ty::Result(Box::new(ok), Box::new(err)));
    }
    ty_of(spec, &TypeRef::parse(text))
}

/// The type of a parsed type name.
pub(crate) fn ty_of(spec: &Spec, t: &TypeRef) -> Result<Ty, Refusal> {
    Ok(match t {
        TypeRef::Builtin(b) => builtin(*b)?,
        TypeRef::List(of) => Ty::list(ty_of(spec, of)?),
        TypeRef::Option(of) => Ty::option(ty_of(spec, of)?),
        TypeRef::Map(k, v) => Ty::map(ty_of(spec, k)?, ty_of(spec, v)?),
        TypeRef::Set(_) | TypeRef::Shared(_) => {
            return Err(unsupported(format!(
                "the type {} is not compiled in functions yet",
                t.text()
            )));
        }
        TypeRef::Declared(id) => {
            if id.starts_with("set<") || id.starts_with("result<") {
                return Err(unsupported(format!(
                    "the type {id} is not compiled in functions yet"
                )));
            }
            let Some(decl) = spec.type_decl(id) else {
                return Err(invalid(format!("the type \"{id}\" does not resolve")));
            };
            if !crate::backend::types::is_core_type(spec, id) {
                return Err(unsupported(format!(
                    "the type {id} reaches a uuid, so it is not in the pure crate functions are compiled into"
                )));
            }
            match &decl.kind {
                TypeKind::Id { repr } | TypeKind::Scalar { repr } => {
                    let b = repr.as_deref().and_then(Builtin::parse).ok_or_else(|| {
                        invalid(format!("the type {id} has no built-in representation"))
                    })?;
                    Ty::Alias(id.clone(), Box::new(builtin(b)?))
                }
                TypeKind::Enum { .. } => Ty::Enum(id.clone()),
                TypeKind::Struct { .. } => Ty::Struct(id.clone()),
                TypeKind::Union { variants, .. } => {
                    if let Some(v) = variants.iter().find(|v| v.tuple) {
                        return Err(unsupported(format!(
                            "variant {} of {id} is a tuple variant, which functions do not build or take apart yet",
                            v.name
                        )));
                    }
                    Ty::Union(id.clone())
                }
            }
        }
    })
}

fn builtin(b: Builtin) -> Result<Ty, Refusal> {
    Ok(match b {
        Builtin::Bool => Ty::Bool,
        Builtin::String => Ty::Str,
        Builtin::U8 | Builtin::U16 | Builtin::U32 | Builtin::U64 | Builtin::I32 | Builtin::I64 => {
            Ty::Int(b)
        }
        other => {
            return Err(unsupported(format!(
                "the built-in type {} is not compiled in functions yet (bool, string and the integer types are)",
                other.as_str()
            )))
        }
    })
}

/// A field of a struct or union variant, as functions read and write it.
#[derive(Debug, Clone)]
pub(crate) struct FieldInfo {
    pub name: String,
    pub type_name: String,
    pub optional: bool,
    /// Written `Box<T>` in the generated type: it holds its own recursive group inline.
    pub boxed: bool,
}

impl FieldInfo {
    fn new(spec: &Spec, owner: &TypeDecl, f: &StructField) -> Self {
        FieldInfo {
            name: f.name.clone(),
            type_name: f.type_name.clone(),
            optional: f.optional,
            boxed: crate::backend::types::field_is_boxed(spec, owner, f),
        }
    }

    /// The field's type as written.
    pub(crate) fn declared(&self, spec: &Spec) -> Result<Ty, Refusal> {
        ty_of(spec, &TypeRef::parse(&self.type_name))
    }

    /// The type a read gives: an optional field reads as `option<T>`.
    pub(crate) fn read(&self, spec: &Spec) -> Result<Ty, Refusal> {
        let t = self.declared(spec)?;
        if self.optional {
            if matches!(t, Ty::Option(_)) {
                return Err(unsupported(format!(
                    "the field {} is optional and of an option type, which the generated type holds as Option<Option<T>>",
                    self.name
                )));
            }
            return Ok(Ty::option(t));
        }
        Ok(t)
    }

    /// True when a constructor may leave the field out.
    pub(crate) fn may_be_absent(&self, spec: &Spec) -> bool {
        self.optional
            || matches!(TypeRef::parse(&self.type_name), TypeRef::Option(_))
            || self
                .declared(spec)
                .is_ok_and(|t| matches!(t, Ty::Option(_)))
    }
}

/// The fields of a struct type.
pub(crate) fn struct_fields(spec: &Spec, id: &str) -> Vec<FieldInfo> {
    match spec.type_decl(id) {
        Some(
            d @ TypeDecl {
                kind: TypeKind::Struct { fields },
                ..
            },
        ) => fields.iter().map(|f| FieldInfo::new(spec, d, f)).collect(),
        _ => Vec::new(),
    }
}

/// A variant of a union type.
#[derive(Debug, Clone)]
pub(crate) struct VariantInfo {
    pub name: String,
    pub newtype: bool,
    pub fields: Vec<FieldInfo>,
}

/// The variants of a union type.
pub(crate) fn variants(spec: &Spec, id: &str) -> Vec<VariantInfo> {
    match spec.type_decl(id) {
        Some(
            d @ TypeDecl {
                kind: TypeKind::Union { variants, .. },
                ..
            },
        ) => variants
            .iter()
            .map(|v| VariantInfo {
                name: v.name.clone(),
                newtype: v.newtype,
                fields: v
                    .fields
                    .iter()
                    .map(|f| FieldInfo::new(spec, d, f))
                    .collect(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The values of an enum type.
pub(crate) fn enum_values(spec: &Spec, id: &str) -> Vec<String> {
    match spec.type_decl(id).map(|d| &d.kind) {
        Some(TypeKind::Enum { values }) => values.clone(),
        _ => Vec::new(),
    }
}

/// What a match takes apart.
#[derive(Debug, Clone)]
pub(crate) enum MatchOn {
    Union(String),
    Option(Ty),
    Result(Ty, Ty),
    Enum(String),
}

/// A case of a match and its fields.
#[derive(Debug, Clone)]
pub(crate) struct Case {
    pub name: String,
    /// The fields, in order; for an option or result the one `value` or `error`.
    pub fields: Vec<CaseField>,
}

#[derive(Debug, Clone)]
pub(crate) struct CaseField {
    pub name: String,
    pub ty: Result<Ty, Refusal>,
    pub boxed: bool,
}

pub(crate) fn match_on(t: &Ty) -> Option<MatchOn> {
    Some(match t {
        Ty::Union(id) => MatchOn::Union(id.clone()),
        Ty::Option(of) => MatchOn::Option((**of).clone()),
        Ty::Result(o, e) => MatchOn::Result((**o).clone(), (**e).clone()),
        Ty::Enum(id) => MatchOn::Enum(id.clone()),
        _ => return None,
    })
}

pub(crate) fn cases(spec: &Spec, m: &MatchOn) -> Vec<Case> {
    let one = |name: &str, field: &str, ty: &Ty| Case {
        name: name.to_string(),
        fields: vec![CaseField {
            name: field.to_string(),
            ty: Ok(ty.clone()),
            boxed: false,
        }],
    };
    match m {
        MatchOn::Union(id) => variants(spec, id)
            .into_iter()
            .map(|v| Case {
                name: v.name,
                fields: v
                    .fields
                    .iter()
                    .map(|f| CaseField {
                        name: f.name.clone(),
                        ty: f.read(spec),
                        boxed: f.boxed,
                    })
                    .collect(),
            })
            .collect(),
        MatchOn::Option(of) => vec![
            Case {
                name: "None".into(),
                fields: Vec::new(),
            },
            one("Some", "value", of),
        ],
        MatchOn::Result(o, e) => vec![one("Ok", "value", o), one("Err", "error", e)],
        MatchOn::Enum(id) => enum_values(spec, id)
            .into_iter()
            .map(|v| Case {
                name: v,
                fields: Vec::new(),
            })
            .collect(),
    }
}

/// A function's signature.
#[derive(Debug, Clone)]
pub(crate) struct Sig {
    pub id: String,
    pub params: Vec<(String, Ty)>,
    pub returns: Ty,
    /// The id of its error union.
    pub errors: Option<String>,
    /// It can fail with an integer overflow or a division by zero: it does checked integer
    /// arithmetic or calls a function that can.
    pub faultable: bool,
}

/// An `eval` or `check` of a language on an environment struct.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct BridgeUse {
    pub language: String,
    pub env: String,
    pub eval: bool,
}

type Scope = Vec<(String, Ty)>;

fn lookup<'s>(scope: &'s Scope, name: &str) -> Option<&'s Ty> {
    scope.iter().rev().find(|(n, _)| n == name).map(|(_, t)| t)
}

fn bound(scope: &Scope, name: &str, t: Ty) -> Scope {
    let mut s = scope.clone();
    s.push((name.to_string(), t));
    s
}

/// Types one function's lets and body.
pub(crate) struct Checker<'a> {
    pub spec: &'a Spec,
    pub sigs: &'a BTreeMap<String, Sig>,
    pub me: &'a Sig,
    /// The body does checked integer arithmetic.
    pub arith: bool,
    /// The functions it calls.
    pub calls: Vec<String>,
    /// The languages it evaluates or checks, and on which environment struct.
    pub bridges: Vec<BridgeUse>,
}

impl<'a> Checker<'a> {
    pub(crate) fn new(spec: &'a Spec, sigs: &'a BTreeMap<String, Sig>, me: &'a Sig) -> Self {
        Checker {
            spec,
            sigs,
            me,
            arith: false,
            calls: Vec::new(),
            bridges: Vec::new(),
        }
    }

    /// Types the lets in order and then the body; the body must fit the return type.
    pub(crate) fn function(
        &mut self,
        lets: &mut [(String, Expr)],
        body: &mut Expr,
    ) -> Result<(), Refusal> {
        let mut scope: Scope = self.me.params.clone();
        for (name, e) in lets.iter_mut() {
            let t = self.walk(e, &scope)?;
            scope.push((name.clone(), t));
        }
        let t = self.walk(body, &scope)?;
        if !assignable(&t, &self.me.returns) {
            return Err(invalid(format!(
                "the body is {}, but the function returns {}",
                t.text(),
                self.me.returns.text()
            )));
        }
        Ok(())
    }

    fn need(&self, t: &Ty, want: &Ty, what: &str) -> Result<(), Refusal> {
        if assignable(t, want) {
            Ok(())
        } else {
            Err(invalid(format!(
                "{what} is {}, not {}",
                t.text(),
                want.text()
            )))
        }
    }

    fn need_list(&self, t: &Ty, what: &str) -> Result<Ty, Refusal> {
        match t {
            Ty::Unknown | Ty::Never => Ok(Ty::Unknown),
            Ty::List(of) => Ok((**of).clone()),
            _ => Err(invalid(format!("{what} is {}, not a list", t.text()))),
        }
    }

    fn need_map(&self, t: &Ty, what: &str) -> Result<(Ty, Ty), Refusal> {
        match t {
            Ty::Unknown | Ty::Never => Ok((Ty::Unknown, Ty::Unknown)),
            Ty::Map(k, v) => Ok(((**k).clone(), (**v).clone())),
            _ => Err(invalid(format!("{what} is {}, not a map", t.text()))),
        }
    }

    pub(crate) fn field_of(&self, t: &Ty, name: &str) -> Result<Ty, Refusal> {
        match t {
            Ty::Unknown | Ty::Never => Ok(Ty::Unknown),
            Ty::Struct(id) => {
                let fields = struct_fields(self.spec, id);
                let f = fields
                    .iter()
                    .find(|f| f.name == name)
                    .ok_or_else(|| invalid(format!("{id} has no field \"{name}\"")))?;
                if f.boxed {
                    return Err(unsupported(format!(
                        "the field {name} of {id} is boxed (its type holds {id} again); reading it is not compiled yet"
                    )));
                }
                f.read(self.spec)
            }
            _ => Err(invalid(format!(
                "{} has no fields; it cannot be read as .{name}",
                t.text()
            ))),
        }
    }

    fn type_decl(&self, id: &str) -> Option<&'a TypeDecl> {
        self.spec.type_decl(id)
    }

    fn variant_of(&self, path: &[String]) -> Result<(String, VariantInfo), Refusal> {
        let id = &path[0];
        match self.type_decl(id).map(|d| &d.kind) {
            None => Err(invalid(format!("\"{id}\" is not a declared type"))),
            Some(TypeKind::Union { .. }) => {
                ty_of(self.spec, &TypeRef::Declared(id.clone()))?;
                let v = variants(self.spec, id)
                    .into_iter()
                    .find(|v| v.name == path[1])
                    .ok_or_else(|| {
                        invalid(format!("union \"{id}\" has no variant \"{}\"", path[1]))
                    })?;
                Ok((id.clone(), v))
            }
            Some(_) => Err(invalid(format!(
                "\"{id}\" is not a union, so it has no variant {}",
                path[1]
            ))),
        }
    }

    fn ctor_fields(
        &mut self,
        declared: &[FieldInfo],
        given: &mut [(String, Expr)],
        scope: &Scope,
        what: &str,
    ) -> Result<(), Refusal> {
        let mut seen: Vec<String> = Vec::new();
        for (name, e) in given.iter_mut() {
            let f = declared
                .iter()
                .find(|f| &f.name == name)
                .ok_or_else(|| invalid(format!("{what} has no field \"{name}\"")))?;
            if seen.contains(name) {
                return Err(invalid(format!("{what} sets the field \"{name}\" twice")));
            }
            seen.push(name.clone());
            let t = self.walk(e, scope)?;
            let want = f.read(self.spec)?;
            self.need(&t, &want, &format!("field \"{name}\" of {what}"))?;
        }
        let missing: Vec<&str> = declared
            .iter()
            .filter(|f| !seen.contains(&f.name) && !f.may_be_absent(self.spec))
            .map(|f| f.name.as_str())
            .collect();
        if !missing.is_empty() {
            return Err(invalid(format!(
                "{what} does not set {}",
                missing.join(", ")
            )));
        }
        Ok(())
    }

    /// Types `e`, annotates it and returns its type.
    pub(crate) fn walk(&mut self, e: &mut Expr, scope: &Scope) -> Result<Ty, Refusal> {
        let t = self.type_of(e, scope)?;
        e.ty = Some(t.clone());
        Ok(t)
    }

    fn type_of(&mut self, e: &mut Expr, scope: &Scope) -> Result<Ty, Refusal> {
        let spec = self.spec;
        match &mut e.kind {
            Kind::Num(text) => {
                if text.contains('.') {
                    return Err(unsupported(format!(
                        "the number {text} has a fraction; functions compute with integers only"
                    )));
                }
                Ok(Ty::Number)
            }
            Kind::Str(_) => Ok(Ty::Str),
            Kind::Bool(_) => Ok(Ty::Bool),
            Kind::Null => Ok(Ty::option(Ty::Unknown)),
            Kind::Ref(path, res) => {
                let head = &path[0];
                if let Some(t) = lookup(scope, head) {
                    *res = Res::Var;
                    let mut t = t.clone();
                    for f in &path[1..] {
                        t = self.field_of(&t, f)?;
                    }
                    return Ok(t);
                }
                if path.len() == 2 {
                    match self.type_decl(head).map(|d| &d.kind) {
                        Some(TypeKind::Enum { values }) => {
                            if !values.contains(&path[1]) {
                                return Err(invalid(format!(
                                    "enum \"{head}\" has no value \"{}\"",
                                    path[1]
                                )));
                            }
                            *res = Res::Enum;
                            return ty_of(spec, &TypeRef::Declared(head.clone()));
                        }
                        Some(TypeKind::Union { .. }) => {
                            let (id, v) = self.variant_of(path)?;
                            if !v.fields.is_empty() {
                                return Err(invalid(format!(
                                    "variant {} of \"{id}\" has fields; write {id}.{}{{...}}",
                                    v.name, v.name
                                )));
                            }
                            *res = Res::Unit;
                            return Ok(Ty::Union(id));
                        }
                        _ => {}
                    }
                }
                Err(invalid(format!(
                    "\"{}\" names nothing: not a parameter, let or bound name",
                    path.join(".")
                )))
            }
            Kind::Field(a, name) => {
                let t = self.walk(a, scope)?;
                self.field_of(&t, name)
            }
            Kind::Un(op, a) => {
                let t = self.walk(a, scope)?;
                match op {
                    UnOp::Not => {
                        self.need(&t, &Ty::Bool, "the operand of \"!\"")?;
                        Ok(Ty::Bool)
                    }
                    UnOp::Neg => {
                        if !is_number(&t) && !is_open(&t) {
                            return Err(invalid(format!(
                                "the operand of \"-\" is {}, not a number",
                                t.text()
                            )));
                        }
                        if !matches!(a.kind, Kind::Num(_)) {
                            self.arith = true;
                        }
                        Ok(t)
                    }
                }
            }
            Kind::Bin(op, l, r) => {
                let a = self.walk(l, scope)?;
                let b = self.walk(r, scope)?;
                let op = *op;
                match op {
                    BinOp::And | BinOp::Or | BinOp::Implies => {
                        self.need(&a, &Ty::Bool, &format!("the left of \"{}\"", op.symbol()))?;
                        self.need(&b, &Ty::Bool, &format!("the right of \"{}\"", op.symbol()))?;
                        return Ok(Ty::Bool);
                    }
                    BinOp::Eq | BinOp::Ne => {
                        if !comparable(&a, &b) {
                            return Err(invalid(format!(
                                "\"{}\" compares {} with {}",
                                op.symbol(),
                                a.text(),
                                b.text()
                            )));
                        }
                        return Ok(Ty::Bool);
                    }
                    _ => {}
                }
                if op == BinOp::Add
                    && (class(&a) == Some(Class::Str) || class(&b) == Some(Class::Str))
                {
                    self.need(&a, &Ty::Str, "the left of \"+\"")?;
                    self.need(&b, &Ty::Str, "the right of \"+\"")?;
                    return Ok(Ty::Str);
                }
                for (t, side) in [(&a, "left"), (&b, "right")] {
                    if !is_number(t) && !is_open(t) {
                        return Err(invalid(format!(
                            "the {side} of \"{}\" is {}, not a number",
                            op.symbol(),
                            t.text()
                        )));
                    }
                }
                if matches!(op, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge) {
                    return Ok(Ty::Bool);
                }
                self.arith = true;
                Ok(if a != Ty::Number && is_number(&a) {
                    a
                } else {
                    b
                })
            }
            Kind::If(c, x, y) => {
                let ct = self.walk(c, scope)?;
                self.need(&ct, &Ty::Bool, "the condition of if")?;
                let a = self.walk(x, scope)?;
                let b = self.walk(y, scope)?;
                join(&a, &b).ok_or_else(|| {
                    invalid(format!(
                        "the branches of if are {} and {}",
                        a.text(),
                        b.text()
                    ))
                })
            }
            Kind::Let(name, v, body) => {
                let t = self.walk(v, scope)?;
                let s = bound(scope, name, t);
                self.walk(body, &s)
            }
            Kind::Match(s, arms) => self.type_match(s, arms, scope),
            Kind::List(items) => {
                let mut of = Ty::Unknown;
                for (i, item) in items.iter_mut().enumerate() {
                    let it = self.walk(item, scope)?;
                    of = join(&of, &it).ok_or_else(|| {
                        invalid(format!(
                            "item {} of the list is {}, the items before it {}",
                            i + 1,
                            it.text(),
                            of.text()
                        ))
                    })?;
                }
                Ok(Ty::list(of))
            }
            Kind::EmptyMap => Ok(Ty::map(Ty::Unknown, Ty::Unknown)),
            Kind::Ctor(path, fields) => {
                if path.len() == 1 {
                    let id = path[0].clone();
                    match self.type_decl(&id).map(|d| &d.kind) {
                        None => return Err(invalid(format!("\"{id}\" is not a declared type"))),
                        Some(TypeKind::Struct { .. }) => {}
                        Some(_) => return Err(invalid(format!("\"{id}\" is not a struct"))),
                    }
                    let t = ty_of(spec, &TypeRef::Declared(id.clone()))?;
                    let declared = struct_fields(spec, &id);
                    self.ctor_fields(&declared, fields, scope, &format!("struct \"{id}\""))?;
                    return Ok(t);
                }
                if path.len() != 2 {
                    return Err(invalid(format!(
                        "\"{}\" is not a type or a union variant",
                        path.join(".")
                    )));
                }
                let (id, v) = self.variant_of(path)?;
                if v.newtype {
                    return Err(invalid(format!(
                        "variant {} of \"{id}\" is a newtype; write {id}.{}(value)",
                        v.name, v.name
                    )));
                }
                self.ctor_fields(
                    &v.fields,
                    fields,
                    scope,
                    &format!("variant {} of \"{id}\"", v.name),
                )?;
                Ok(Ty::Union(id))
            }
            Kind::Newtype(path, arg) => {
                if path.len() != 2 {
                    return Err(invalid(format!(
                        "\"{}\" is not a union variant; a call of a function is written call f(...)",
                        path.join(".")
                    )));
                }
                let (id, v) = self.variant_of(path)?;
                if v.fields.len() != 1 {
                    return Err(invalid(format!(
                        "variant {} of \"{id}\" has {} fields; {id}.{}(value) builds a variant with one",
                        v.name,
                        v.fields.len(),
                        v.name
                    )));
                }
                let t = self.walk(arg, scope)?;
                let want = v.fields[0].read(spec)?;
                self.need(&t, &want, &format!("the value of {id}.{}", v.name))?;
                Ok(Ty::Union(id))
            }
            Kind::Call(f, args) => {
                let Some(callee) = self.sigs.get(f.as_str()) else {
                    return Err(invalid(format!(
                        "call names \"{f}\", which is not a declared function"
                    )));
                };
                if args.len() != callee.params.len() {
                    return Err(invalid(format!(
                        "\"{f}\" takes {} arguments; the call gives {}",
                        callee.params.len(),
                        args.len()
                    )));
                }
                for (a, (pname, pt)) in args.iter_mut().zip(&callee.params) {
                    let t = self.walk(a, scope)?;
                    self.need(&t, pt, &format!("argument \"{pname}\" of call {f}"))?;
                }
                self.calls.push(f.clone());
                Ok(match &callee.errors {
                    Some(err) => Ty::Result(
                        Box::new(callee.returns.clone()),
                        Box::new(Ty::Union(err.clone())),
                    ),
                    None => callee.returns.clone(),
                })
            }
            Kind::Try(a) => {
                let t = self.walk(a, scope)?;
                let Ty::Result(ok, err) = t else {
                    return Err(invalid(format!(
                        "try needs a result (a call of a function that declares errors); this is {}",
                        t.text()
                    )));
                };
                let Some(mine) = &self.me.errors else {
                    return Err(invalid(format!(
                        "try passes on an error, but function \"{}\" declares no errors",
                        self.me.id
                    )));
                };
                if *err != Ty::Union(mine.clone()) {
                    return Err(invalid(format!(
                        "try passes on {} errors, but function \"{}\" fails with {mine}",
                        err.text(),
                        self.me.id
                    )));
                }
                Ok(*ok)
            }
            Kind::Op(op) => self.type_op(op, scope),
        }
    }

    fn type_match(&mut self, s: &mut Expr, arms: &mut [Arm], scope: &Scope) -> Result<Ty, Refusal> {
        let st = self.walk(s, scope)?;
        let Some(m) = match_on(&st) else {
            return Err(invalid(format!(
                "match needs a union, an option, a result or an enum; this is {}",
                st.text()
            )));
        };
        let all = cases(self.spec, &m);
        let mut seen: Vec<String> = Vec::new();
        let mut wild = false;
        let mut out = Ty::Never;
        for arm in arms.iter_mut() {
            if wild {
                return Err(invalid(
                    "an arm of match follows \"_\", so it is never reached",
                ));
            }
            let mut s2 = scope.clone();
            match &arm.pat {
                Pattern::Wild => wild = true,
                Pattern::Case { name, bind, fields } => {
                    let case = all.iter().find(|c| &c.name == name).ok_or_else(|| {
                        invalid(format!("match on {} has no case \"{name}\"", st.text()))
                    })?;
                    if seen.contains(name) {
                        return Err(invalid(format!("match has two arms for \"{name}\"")));
                    }
                    seen.push(name.clone());
                    if let Some(b) = bind {
                        if case.fields.len() != 1 {
                            return Err(invalid(format!(
                                "\"{name}\" has {} fields; {name}(x) binds the one field of a variant that has one",
                                case.fields.len()
                            )));
                        }
                        if b != "_" {
                            let f = &case.fields[0];
                            if f.boxed {
                                return Err(unsupported(format!(
                                    "the field {} of {name} is boxed; binding it is not compiled yet",
                                    f.name
                                )));
                            }
                            s2.push((b.clone(), f.ty.clone()?));
                        }
                    }
                    for (field, b) in fields.iter().flatten() {
                        let f = case
                            .fields
                            .iter()
                            .find(|f| &f.name == field)
                            .ok_or_else(|| {
                                invalid(format!("\"{name}\" has no field \"{field}\""))
                            })?;
                        if b != "_" {
                            if f.boxed {
                                return Err(unsupported(format!(
                                    "the field {field} of {name} is boxed; binding it is not compiled yet"
                                )));
                            }
                            s2.push((b.clone(), f.ty.clone()?));
                        }
                    }
                }
            }
            let bt = self.walk(&mut arm.body, &s2)?;
            out = join(&out, &bt).ok_or_else(|| {
                invalid(format!(
                    "the arms of match are {} and {}",
                    out.text(),
                    bt.text()
                ))
            })?;
        }
        let missing: Vec<&str> = all
            .iter()
            .filter(|c| !seen.contains(&c.name))
            .map(|c| c.name.as_str())
            .collect();
        if !wild && !missing.is_empty() {
            return Err(invalid(format!(
                "match on {} does not cover {}",
                st.text(),
                missing.join(", ")
            )));
        }
        Ok(out)
    }

    fn type_op(&mut self, op: &mut Op, scope: &Scope) -> Result<Ty, Refusal> {
        let name = op.name.clone();
        let arity = |n: usize, op: &Op| -> Result<(), Refusal> {
            if op.args.len() == n {
                Ok(())
            } else {
                Err(invalid(format!(
                    "{} takes {n} arguments; it is given {}",
                    op.name,
                    op.args.len()
                )))
            }
        };
        if op.binder.is_some() {
            let st = {
                let (_, src) = op.binder.as_mut().expect("checked");
                self.walk(src, scope)?
            };
            let of = self.need_list(&st, &format!("the list of {name}"))?;
            arity(1, op)?;
            if name == "fold" {
                let Some((_, init)) = op.acc.as_mut() else {
                    return Err(invalid("fold has no accumulator"));
                };
                self.walk(init, scope)?;
                return self.type_fold(op, scope);
            }
            let bname = op.binder.as_ref().expect("checked").0.clone();
            let s = bound(scope, &bname, of.clone());
            let body = self.walk(&mut op.args[0], &s)?;
            return match name.as_str() {
                "map" => Ok(Ty::list(body)),
                "filter" => {
                    self.need(&body, &Ty::Bool, "the condition of filter")?;
                    Ok(Ty::list(of))
                }
                "find" => {
                    self.need(&body, &Ty::Bool, "the condition of find")?;
                    Ok(Ty::option(of))
                }
                "any" | "all" => {
                    self.need(&body, &Ty::Bool, &format!("the condition of {name}"))?;
                    Ok(Ty::Bool)
                }
                "sortBy" => {
                    let c = class(&body);
                    if c != Some(Class::Num)
                        && c != Some(Class::Str)
                        && body != Ty::Unknown
                        && !matches!(body, Ty::Enum(_))
                    {
                        return Err(invalid(format!(
                            "the key of sortBy is {}; it is a number or a string",
                            body.text()
                        )));
                    }
                    Ok(Ty::list(of))
                }
                _ => Err(invalid(format!("\"{name}\" is not an operation"))),
            };
        }
        let walk_args = |me: &mut Self, op: &mut Op| -> Result<Vec<Ty>, Refusal> {
            let mut out = Vec::new();
            for a in op.args.iter_mut() {
                out.push(me.walk(a, scope)?);
            }
            Ok(out)
        };
        match name.as_str() {
            "map" | "filter" | "find" | "any" | "all" | "fold" | "sortBy" => Err(invalid(format!(
                "{name} binds a name: {name}(x in list, ...)"
            ))),
            "take" => {
                arity(2, op)?;
                let t = walk_args(self, op)?;
                let of = self.need_list(&t[0], "the list of take")?;
                if !is_number(&t[1]) {
                    return Err(invalid(format!(
                        "the count of take is {}, not a number",
                        t[1].text()
                    )));
                }
                Ok(Ty::list(of))
            }
            "concat" => {
                arity(2, op)?;
                let t = walk_args(self, op)?;
                self.need_list(&t[0], "the first list of concat")?;
                self.need_list(&t[1], "the second list of concat")?;
                join(&t[0], &t[1]).ok_or_else(|| {
                    invalid(format!("concat joins {} and {}", t[0].text(), t[1].text()))
                })
            }
            "keys" => {
                arity(1, op)?;
                let t = walk_args(self, op)?;
                Ok(Ty::list(self.need_map(&t[0], "the argument of keys")?.0))
            }
            "values" => {
                arity(1, op)?;
                let t = walk_args(self, op)?;
                Ok(Ty::list(self.need_map(&t[0], "the argument of values")?.1))
            }
            "get" => {
                arity(2, op)?;
                let t = walk_args(self, op)?;
                if let Ty::List(of) = &t[0] {
                    if !is_number(&t[1]) {
                        return Err(invalid(format!(
                            "the index of get is {}, not a number",
                            t[1].text()
                        )));
                    }
                    return Ok(Ty::option((**of).clone()));
                }
                let (k, v) = self.need_map(&t[0], "the first argument of get")?;
                self.need(&t[1], &k, "the key of get")?;
                Ok(Ty::option(v))
            }
            "insert" => {
                arity(3, op)?;
                let t = walk_args(self, op)?;
                let (k, v) = self.need_map(&t[0], "the first argument of insert")?;
                let key = join(&k, &t[1]).ok_or_else(|| {
                    invalid(format!(
                        "the key of insert is {}, the map's keys {}",
                        t[1].text(),
                        k.text()
                    ))
                })?;
                let value = join(&v, &t[2]).ok_or_else(|| {
                    invalid(format!(
                        "the value of insert is {}, the map's values {}",
                        t[2].text(),
                        v.text()
                    ))
                })?;
                Ok(Ty::map(key, value))
            }
            "remove" => {
                arity(2, op)?;
                let t = walk_args(self, op)?;
                let (k, _) = self.need_map(&t[0], "the first argument of remove")?;
                self.need(&t[1], &k, "the key of remove")?;
                Ok(t[0].clone())
            }
            "format" => {
                let Some(Kind::Str(text)) = op.args.first().map(|a| &a.kind) else {
                    return Err(invalid(
                        "the first argument of format is a string literal, such as \"policy:{}\"",
                    ));
                };
                let holes = text.matches("{}").count();
                if holes != op.args.len() - 1 {
                    return Err(invalid(format!(
                        "format \"{text}\" has {holes} placeholders and {} values",
                        op.args.len() - 1
                    )));
                }
                let t = walk_args(self, op)?;
                for (i, a) in t.iter().enumerate().skip(1) {
                    if class(a).is_none() && !matches!(a, Ty::Enum(_) | Ty::Unknown) {
                        return Err(invalid(format!(
                            "value {i} of format is {}; format writes numbers, strings, bools and enum values",
                            a.text()
                        )));
                    }
                }
                Ok(Ty::Str)
            }
            "eval" | "check" => {
                arity(3, op)?;
                self.type_language(op, scope)
            }
            "fail" => {
                arity(1, op)?;
                let Some(mine) = self.me.errors.clone() else {
                    return Err(invalid(format!(
                        "fail is used, but function \"{}\" declares no errors",
                        self.me.id
                    )));
                };
                let t = walk_args(self, op)?;
                if t[0] != Ty::Union(mine.clone()) && !is_open(&t[0]) {
                    return Err(invalid(format!(
                        "fail is given {}, but function \"{}\" fails with {mine}",
                        t[0].text(),
                        self.me.id
                    )));
                }
                Ok(Ty::Never)
            }
            _ => Err(invalid(format!(
                "\"{name}\" is not an operation; a function is called as call {name}(...)"
            ))),
        }
    }

    fn type_fold(&mut self, op: &mut Op, scope: &Scope) -> Result<Ty, Refusal> {
        let (bname, st) = match &op.binder {
            Some((b, src)) => (b.clone(), src.ty()),
            None => return Err(invalid("fold binds no name")),
        };
        let of = self.need_list(&st, "the list of fold")?;
        let (acc_name, acc) = match &op.acc {
            Some((a, init)) => (a.clone(), init.ty()),
            None => return Err(invalid("fold has no accumulator")),
        };
        let s = bound(&bound(scope, &bname, of.clone()), &acc_name, acc.clone());
        let body = self.walk(&mut op.args[0], &s)?;
        let j = join(&acc, &body);
        let Some(j) = j.filter(|_| assignable(&body, &acc)) else {
            return Err(invalid(format!(
                "the step of fold is {}, its accumulator {}",
                body.text(),
                acc.text()
            )));
        };
        if j != acc {
            // Type the step again with the accumulator at the fold's type, so every node of
            // the step has the types the generated code holds.
            let s = bound(&bound(scope, &bname, of), &acc_name, j.clone());
            let again = self.walk(&mut op.args[0], &s)?;
            if !assignable(&again, &j) {
                return Err(invalid(format!(
                    "the step of fold is {}, its accumulator {}",
                    again.text(),
                    j.text()
                )));
            }
        }
        Ok(j)
    }

    fn type_language(&mut self, op: &mut Op, scope: &Scope) -> Result<Ty, Refusal> {
        let eval = op.name == "eval";
        let spec = self.spec;
        let lang_id = match &mut op.args[0].kind {
            Kind::Ref(path, res) if path.len() == 1 => {
                *res = Res::Language;
                path[0].clone()
            }
            _ => {
                return Err(invalid(format!(
                    "the first argument of {} is the id of a language",
                    op.name
                )))
            }
        };
        op.args[0].ty = Some(Ty::Unknown);
        let Some(lang) = spec.languages.iter().find(|l| l.id == lang_id) else {
            return Err(invalid(format!("\"{lang_id}\" is not a declared language")));
        };
        let Some(bridge) = crate::language::bridge(spec, lang) else {
            return Err(unsupported(format!(
                "the language {lang_id} is refused by specc, so its eval and check are not generated"
            )));
        };
        let ast = Ty::Union(bridge.ast_id().to_string());
        let t = self.walk(&mut op.args[1], scope)?;
        self.need(&t, &ast, &format!("the expression of {}", op.name))?;
        let env = self.walk(&mut op.args[2], scope)?;
        let Ty::Struct(env_id) = env else {
            return Err(invalid(format!(
                "the environment of {} is {}, not a struct",
                op.name,
                env.text()
            )));
        };
        let fields = struct_fields(spec, &env_id);
        let field = |name: &str| -> Result<Ty, Refusal> {
            let f = fields.iter().find(|f| f.name == name).ok_or_else(|| {
                invalid(format!("the environment {env_id} has no field \"{name}\""))
            })?;
            if f.boxed {
                return Err(unsupported(format!(
                    "the field {name} of {env_id} is boxed"
                )));
            }
            f.read(spec)
        };
        let exact = |name: &str, want: &Ty| -> Result<(), Refusal> {
            let got = field(name)?;
            if &got == want {
                Ok(())
            } else if assignable(&got, want) {
                Err(unsupported(format!(
                    "the field {name} of {env_id} is {}; specc reads it as exactly {}",
                    got.text(),
                    want.text()
                )))
            } else {
                Err(invalid(format!(
                    "the field {name} of {env_id} is {}, not {}",
                    got.text(),
                    want.text()
                )))
            }
        };
        if eval {
            let values = Ty::Union(bridge.values_id().to_string());
            for s in bridge.scopes() {
                let want = match s.kind.as_str() {
                    "record-scope" => Ty::option(Ty::map(Ty::Str, values.clone())),
                    "param" => Ty::map(Ty::Str, values.clone()),
                    "role-test" => Ty::list(Ty::Str),
                    _ => values.clone(),
                };
                exact(&s.name, &want)?;
            }
        } else {
            exact("fields", &Ty::map(Ty::Str, Ty::Str))?;
            exact("params", &Ty::list(Ty::Str))?;
            exact("roles", &Ty::list(Ty::Str))?;
        }
        let used = BridgeUse {
            language: lang_id,
            env: env_id,
            eval,
        };
        if !self.bridges.contains(&used) {
            self.bridges.push(used);
        }
        Ok(if eval {
            Ty::Result(
                Box::new(Ty::Union(bridge.values_id().to_string())),
                Box::new(Ty::Str),
            )
        } else {
            Ty::Result(Box::new(Ty::Str), Box::new(Ty::Str))
        })
    }
}
