//! Type checking against representation types and translation to Rust tokens.
//!
//! Exegra evaluates every number as an IEEE double. Rust has distinct integer widths, so the
//! checker accepts only what both languages agree on:
//!
//! * operands of arithmetic and comparison have the same type; there are no implicit conversions;
//! * an integer literal takes the type of the other operand and must fit it; a fractional literal
//!   is only an `f64`;
//! * integer `/` is refused: Exegra computes the exact quotient (`7 / 2` is `3.5`), Rust truncates;
//! * `Timestamp` and `Duration` compare but have no arithmetic (their Rust representation is not
//!   fixed by the language);
//! * `null` is an option value and is only compared with, or used as, an `option<T>`.
//!
//! Every emitted sub-expression is wrapped in parentheses.

use crate::ast::{BinOp, Expr, ExprKind, Lit, Span, UnOp};
use crate::error::{TypeError, TypeErrorKind};
use proc_macro2::{Delimiter, TokenStream, TokenTree};
use quote::quote;
use std::fmt;

/// A representation type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    Bool,
    U8,
    U16,
    U32,
    U64,
    I32,
    I64,
    F64,
    String,
    Bytes,
    Timestamp,
    Duration,
    Uuid,
    /// A declared `type` entity (id, scalar, enum or struct), by id.
    Named(String),
    List(Box<Ty>),
    Option(Box<Ty>),
}

impl Ty {
    fn int_info(&self) -> Option<(bool, u32, &'static str)> {
        Some(match self {
            Ty::U8 => (false, 8, "u8"),
            Ty::U16 => (false, 16, "u16"),
            Ty::U32 => (false, 32, "u32"),
            Ty::U64 => (false, 64, "u64"),
            Ty::I32 => (true, 32, "i32"),
            Ty::I64 => (true, 64, "i64"),
            _ => return None,
        })
    }

    pub fn is_integer(&self) -> bool {
        self.int_info().is_some()
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Bool => f.write_str("bool"),
            Ty::U8 => f.write_str("u8"),
            Ty::U16 => f.write_str("u16"),
            Ty::U32 => f.write_str("u32"),
            Ty::U64 => f.write_str("u64"),
            Ty::I32 => f.write_str("i32"),
            Ty::I64 => f.write_str("i64"),
            Ty::F64 => f.write_str("f64"),
            Ty::String => f.write_str("string"),
            Ty::Bytes => f.write_str("bytes"),
            Ty::Timestamp => f.write_str("timestamp"),
            Ty::Duration => f.write_str("duration"),
            Ty::Uuid => f.write_str("uuid"),
            Ty::Named(n) => f.write_str(n),
            Ty::List(t) => write!(f, "list<{t}>"),
            Ty::Option(t) => write!(f, "option<{t}>"),
        }
    }
}

/// What a reference resolves to: its type and a Rust expression that reads it. The expression is a
/// place or value of the Rust type for `ty`; it is wrapped in parentheses when used and is only
/// ever read (compared, copied, borrowed), never moved out of.
#[derive(Debug, Clone)]
pub struct Binding {
    pub ty: Ty,
    pub tokens: TokenStream,
}

pub trait Scope {
    /// Resolve a dotted reference such as `["token"]`, `["event","token"]`,
    /// `["lease","count","jl-done"]`.
    fn resolve(&self, path: &[&str]) -> Option<Binding>;
    /// Tokens for the error value returned when checked arithmetic overflows or a division by zero
    /// happens, e.g. `StepError::Overflow`.
    fn overflow_error(&self) -> TokenStream;
}

/// The type of an expression. A bare integer or fractional literal, or `null`, has none by itself.
pub fn type_of(expr: &Expr, scope: &dyn Scope) -> Result<Ty, TypeError> {
    let cx = Cx { scope };
    let out = cx.go(expr, None)?;
    cx.known(expr, out).map(|(t, _)| t)
}

/// Translate an expression whose type is determined by its references and typed literals.
pub fn to_rust(expr: &Expr, scope: &dyn Scope) -> Result<TokenStream, TypeError> {
    let cx = Cx { scope };
    let out = cx.go(expr, None)?;
    cx.known(expr, out).map(|(_, t)| t)
}

/// Translate an expression that must have type `expected`: literals take that type. Use this for
/// the value of an effect, whose target type is that of the variable.
pub fn to_rust_as(expr: &Expr, scope: &dyn Scope, expected: &Ty) -> Result<TokenStream, TypeError> {
    let cx = Cx { scope };
    let out = cx.go(expr, Some(expected))?;
    let (ty, tokens) = cx.known(expr, out)?;
    if &ty != expected {
        return Err(cx.err(
            expr,
            TypeErrorKind::Mismatch,
            format!("expected {expected}, found {ty}"),
        ));
    }
    Ok(tokens)
}

#[derive(Debug, Clone, PartialEq)]
enum Inf {
    Known(Ty),
    /// An integer literal expression still waiting for a type.
    Int,
    /// A fractional literal expression still waiting for a type.
    Float,
    Null,
}

struct Out {
    ty: Inf,
    tokens: TokenStream,
}

impl Out {
    fn known(ty: Ty, tokens: TokenStream) -> Self {
        Out {
            ty: Inf::Known(ty),
            tokens,
        }
    }

    fn pending(ty: Inf) -> Self {
        Out {
            ty,
            tokens: TokenStream::new(),
        }
    }
}

struct Cx<'a> {
    scope: &'a dyn Scope,
}

/// Strip one pair of parentheses from a token stream that is exactly one parenthesised group.
fn unparen(ts: &TokenStream) -> TokenStream {
    let mut it = ts.clone().into_iter();
    if let (Some(TokenTree::Group(g)), None) = (it.next(), it.next()) {
        if g.delimiter() == Delimiter::Parenthesis {
            return g.stream();
        }
    }
    ts.clone()
}

fn is_float(t: &Ty) -> bool {
    matches!(t, Ty::F64)
}

impl Cx<'_> {
    fn err(&self, e: &Expr, kind: TypeErrorKind, message: impl Into<String>) -> TypeError {
        self.err_at(e, e.span, kind, message)
    }

    fn err_at(
        &self,
        e: &Expr,
        span: Span,
        kind: TypeErrorKind,
        message: impl Into<String>,
    ) -> TypeError {
        TypeError {
            kind,
            message: message.into(),
            span,
            source: e.source.to_string(),
        }
    }

    fn known(&self, e: &Expr, out: Out) -> Result<(Ty, TokenStream), TypeError> {
        match out.ty {
            Inf::Known(t) => Ok((t, out.tokens)),
            Inf::Int | Inf::Float => Err(self.err(
                e,
                TypeErrorKind::LiteralNeedsType,
                "a number literal needs a type: compare it with, or combine it with, a typed value",
            )),
            Inf::Null => Err(self.err(
                e,
                TypeErrorKind::NullNeedsOption,
                "null needs an option type: compare it with an option value",
            )),
        }
    }

    fn require_bool(&self, e: &Expr, out: Out, op: &str) -> Result<TokenStream, TypeError> {
        match out.ty {
            Inf::Known(Ty::Bool) => Ok(out.tokens),
            Inf::Known(t) => Err(self.err(
                e,
                TypeErrorKind::NotBoolean,
                format!("operator \"{op}\" needs a boolean, got {t}"),
            )),
            Inf::Int | Inf::Float => Err(self.err(
                e,
                TypeErrorKind::NotBoolean,
                format!("operator \"{op}\" needs a boolean, got a number"),
            )),
            Inf::Null => Err(self.err(
                e,
                TypeErrorKind::NotBoolean,
                format!("operator \"{op}\" needs a boolean, got null"),
            )),
        }
    }

    /// Tokens of a number literal (`text`, negated when `neg`) at type `ty`.
    fn number(&self, e: &Expr, text: &str, neg: bool, ty: &Ty) -> Result<Out, TypeError> {
        let fractional = text.contains('.');
        let shown = if neg {
            format!("-{text}")
        } else {
            text.to_string()
        };
        if let Some((signed, bits, suffix)) = ty.int_info() {
            if fractional {
                return Err(self.err(
                    e,
                    TypeErrorKind::LiteralDoesNotFit,
                    format!("literal {shown} is not an integer and cannot be a {ty}"),
                ));
            }
            let fits = text.parse::<u128>().is_ok_and(|v| {
                if !signed {
                    !neg || v == 0
                } else if neg {
                    v <= 1u128 << (bits - 1)
                } else {
                    v < 1u128 << (bits - 1)
                }
            });
            if !fits {
                return Err(self.err(
                    e,
                    TypeErrorKind::LiteralDoesNotFit,
                    format!("literal {shown} does not fit in {ty}"),
                ));
            }
            let digits = text.trim_start_matches('0');
            let digits = if digits.is_empty() { "0" } else { digits };
            let sign = if neg && digits != "0" { "-" } else { "" };
            let lit = format!("({sign}{digits}{suffix})");
            return Ok(Out::known(ty.clone(), self.lex(e, &lit)?));
        }
        if is_float(ty) {
            let v: f64 = text.parse().unwrap_or(f64::INFINITY);
            if !v.is_finite() {
                return Err(self.err(
                    e,
                    TypeErrorKind::LiteralDoesNotFit,
                    format!("literal {shown} does not fit in f64"),
                ));
            }
            let sign = if neg { "-" } else { "" };
            let lit = format!("({sign}{v:?}f64)");
            return Ok(Out::known(ty.clone(), self.lex(e, &lit)?));
        }
        Err(self.err(
            e,
            TypeErrorKind::Mismatch,
            format!("a number cannot be used where {ty} is expected"),
        ))
    }

    fn lex(&self, e: &Expr, text: &str) -> Result<TokenStream, TypeError> {
        text.parse::<TokenStream>().map_err(|_| {
            self.err(
                e,
                TypeErrorKind::Unsupported,
                "literal cannot be written as Rust tokens",
            )
        })
    }

    fn go(&self, e: &Expr, hint: Option<&Ty>) -> Result<Out, TypeError> {
        match &e.kind {
            ExprKind::Lit(Lit::Bool(b)) => Ok(Out::known(Ty::Bool, quote!((#b)))),
            ExprKind::Lit(Lit::Str(s)) => Ok(Out::known(
                Ty::String,
                quote!((::std::string::String::from(#s))),
            )),
            ExprKind::Lit(Lit::Null) => match hint {
                Some(t @ Ty::Option(_)) => Ok(Out::known(
                    t.clone(),
                    quote!((::core::option::Option::None)),
                )),
                Some(t) => Err(self.err(
                    e,
                    TypeErrorKind::NullNeedsOption,
                    format!("null is only an option value, but {t} is expected"),
                )),
                None => Ok(Out::pending(Inf::Null)),
            },
            ExprKind::Lit(Lit::Num(text)) => self.literal(e, text, false, hint),
            ExprKind::Ref(path) => {
                let parts: Vec<&str> = path.iter().map(String::as_str).collect();
                match self.scope.resolve(&parts) {
                    Some(b) => {
                        let t = b.tokens;
                        Ok(Out::known(b.ty, quote!((#t))))
                    }
                    None => Err(self.err(
                        e,
                        TypeErrorKind::UnknownReference,
                        format!("Unknown reference \"{}\"", path.join(".")),
                    )),
                }
            }
            ExprKind::Un(UnOp::Not, a) => {
                let o = self.go(a, None)?;
                let t = self.require_bool(a, o, "!")?;
                Ok(Out::known(Ty::Bool, quote!((!#t))))
            }
            ExprKind::Un(UnOp::Neg, a) => {
                if let ExprKind::Lit(Lit::Num(text)) = &a.kind {
                    return self.literal(e, text, true, hint);
                }
                let o = self.go(a, hint)?;
                match o.ty {
                    Inf::Int | Inf::Float => Ok(o),
                    Inf::Null => Err(self.err(
                        a,
                        TypeErrorKind::BadOperand,
                        "operator \"-\" needs a number, got null",
                    )),
                    Inf::Known(t) => {
                        let v = o.tokens;
                        if let Some((signed, _, _)) = t.int_info() {
                            if !signed {
                                return Err(self.err(
                                    e,
                                    TypeErrorKind::BadOperand,
                                    format!("cannot negate {t}: it is unsigned"),
                                ));
                            }
                            let ov = self.scope.overflow_error();
                            Ok(Out::known(t, quote!((#v.checked_neg().ok_or(#ov)?))))
                        } else if is_float(&t) {
                            Ok(Out::known(t, quote!((-#v))))
                        } else {
                            Err(self.err(
                                a,
                                TypeErrorKind::BadOperand,
                                format!("operator \"-\" needs a number, got {t}"),
                            ))
                        }
                    }
                }
            }
            ExprKind::Bin(op, a, b) => self.binary(e, *op, a, b, hint),
        }
    }

    fn literal(
        &self,
        e: &Expr,
        text: &str,
        neg: bool,
        hint: Option<&Ty>,
    ) -> Result<Out, TypeError> {
        match hint {
            Some(t) => self.number(e, text, neg, t),
            None if text.contains('.') => Ok(Out::pending(Inf::Float)),
            None => Ok(Out::pending(Inf::Int)),
        }
    }

    /// Both operands at one common type, taking literals' types from the other side.
    /// `Err(inf)` is returned when neither side has a type.
    fn pair(
        &self,
        a: &Expr,
        b: &Expr,
        hint: Option<&Ty>,
    ) -> Result<Result<(Ty, Out, Out), Inf>, TypeError> {
        let mut l = self.go(a, hint)?;
        let mut r = self.go(b, hint)?;
        match (&l.ty, &r.ty) {
            (Inf::Known(x), Inf::Known(y)) => {
                if x != y {
                    return Err(self.err_at(
                        a,
                        Span {
                            start: a.span.start,
                            end: b.span.end,
                        },
                        TypeErrorKind::Mismatch,
                        format!("operands have different types: {x} and {y}"),
                    ));
                }
            }
            (Inf::Known(x), _) => {
                let x = x.clone();
                r = self.go(b, Some(&x))?;
            }
            (_, Inf::Known(y)) => {
                let y = y.clone();
                l = self.go(a, Some(&y))?;
            }
            (x, y) => {
                let float = *x == Inf::Float || *y == Inf::Float;
                let null = *x == Inf::Null || *y == Inf::Null;
                return Ok(Err(if null {
                    Inf::Null
                } else if float {
                    Inf::Float
                } else {
                    Inf::Int
                }));
            }
        }
        match (&l.ty, &r.ty) {
            (Inf::Known(x), Inf::Known(y)) if x == y => {
                let ty = x.clone();
                Ok(Ok((ty, l, r)))
            }
            (Inf::Known(x), Inf::Known(y)) => Err(self.err_at(
                a,
                Span {
                    start: a.span.start,
                    end: b.span.end,
                },
                TypeErrorKind::Mismatch,
                format!("operands have different types: {x} and {y}"),
            )),
            _ => {
                let bad = if matches!(l.ty, Inf::Known(_)) { b } else { a };
                Err(self.err(
                    bad,
                    TypeErrorKind::NullNeedsOption,
                    "null can only be used with an option value",
                ))
            }
        }
    }

    fn binary(
        &self,
        e: &Expr,
        op: BinOp,
        a: &Expr,
        b: &Expr,
        hint: Option<&Ty>,
    ) -> Result<Out, TypeError> {
        match op {
            BinOp::And | BinOp::Or | BinOp::Implies => {
                let l = self.go(a, None)?;
                let l = self.require_bool(a, l, op.symbol())?;
                let r = self.go(b, None)?;
                let r = self.require_bool(b, r, op.symbol())?;
                let t = match op {
                    BinOp::And => quote!((#l && #r)),
                    BinOp::Or => quote!((#l || #r)),
                    _ => quote!(((!#l) || #r)),
                };
                Ok(Out::known(Ty::Bool, t))
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let (ty, l, r) = match self.pair(a, b, None)? {
                    Ok(x) => x,
                    Err(Inf::Null) => {
                        return Err(self.err(
                            e,
                            TypeErrorKind::NullNeedsOption,
                            "null can only be compared with an option value",
                        ))
                    }
                    Err(_) => {
                        return Err(self.err(
                            e,
                            TypeErrorKind::LiteralNeedsType,
                            "both operands are literals: one of them needs a type",
                        ))
                    }
                };
                let ordering = !matches!(op, BinOp::Eq | BinOp::Ne);
                if ordering
                    && !(ty.is_integer()
                        || is_float(&ty)
                        || matches!(ty, Ty::Timestamp | Ty::Duration))
                {
                    return Err(self.err(
                        e,
                        TypeErrorKind::BadOperand,
                        format!("operator \"{}\" needs numbers, got {ty}", op.symbol()),
                    ));
                }
                let (l, r) = (l.tokens, r.tokens);
                let t = match op {
                    BinOp::Eq => quote!((#l == #r)),
                    BinOp::Ne => quote!((#l != #r)),
                    BinOp::Lt => quote!((#l < #r)),
                    BinOp::Le => quote!((#l <= #r)),
                    BinOp::Gt => quote!((#l > #r)),
                    _ => quote!((#l >= #r)),
                };
                Ok(Out::known(Ty::Bool, t))
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                let (ty, l, r) = match self.pair(a, b, hint)? {
                    Ok(x) => x,
                    Err(Inf::Null) => {
                        return Err(self.err(
                            e,
                            TypeErrorKind::BadOperand,
                            format!("operator \"{}\" needs numbers, got null", op.symbol()),
                        ))
                    }
                    Err(p) => return Ok(Out::pending(p)),
                };
                self.arithmetic(e, op, ty, l.tokens, r.tokens)
            }
        }
    }

    fn arithmetic(
        &self,
        e: &Expr,
        op: BinOp,
        ty: Ty,
        l: TokenStream,
        r: TokenStream,
    ) -> Result<Out, TypeError> {
        let sym = op.symbol();
        if ty == Ty::String {
            if op != BinOp::Add {
                return Err(self.err(
                    e,
                    TypeErrorKind::BadOperand,
                    format!("operator \"{sym}\" needs numbers, got string"),
                ));
            }
            return Ok(Out::known(
                ty,
                quote!(({
                    let mut __s = ::std::string::String::from(&*#l);
                    __s.push_str(&*#r);
                    __s
                })),
            ));
        }
        if matches!(ty, Ty::Timestamp | Ty::Duration) {
            return Err(self.err(
                e,
                TypeErrorKind::Unsupported,
                format!("arithmetic on {ty} is not supported: its Rust representation has no fixed arithmetic"),
            ));
        }
        let ov = self.scope.overflow_error();
        if ty.is_integer() {
            let arg = unparen(&r);
            let t = match op {
                BinOp::Add => quote!((#l.checked_add(#arg).ok_or(#ov)?)),
                BinOp::Sub => quote!((#l.checked_sub(#arg).ok_or(#ov)?)),
                BinOp::Mul => quote!((#l.checked_mul(#arg).ok_or(#ov)?)),
                BinOp::Rem => quote!((#l.checked_rem(#arg).ok_or(#ov)?)),
                _ => {
                    return Err(self.err(
                        e,
                        TypeErrorKind::Unsupported,
                        format!(
                            "integer division is not supported: Exegra's \"/\" on {ty} is the exact quotient (7 / 2 is 3.5) while Rust truncates"
                        ),
                    ))
                }
            };
            return Ok(Out::known(ty, t));
        }
        if is_float(&ty) {
            let t = match op {
                BinOp::Add => quote!((#l + #r)),
                BinOp::Sub => quote!((#l - #r)),
                BinOp::Mul => quote!((#l * #r)),
                BinOp::Div => quote!(({
                    let __l = #l;
                    let __r = #r;
                    if __r == 0.0f64 {
                        return ::core::result::Result::Err(#ov);
                    }
                    (__l / __r)
                })),
                _ => quote!(({
                    let __l = #l;
                    let __r = #r;
                    if __r == 0.0f64 {
                        return ::core::result::Result::Err(#ov);
                    }
                    (__l % __r)
                })),
            };
            return Ok(Out::known(ty, t));
        }
        Err(self.err(
            e,
            TypeErrorKind::BadOperand,
            format!("operator \"{sym}\" needs numbers, got {ty}"),
        ))
    }
}
