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
//! Emitted expressions carry the binding strength of their top operator, and a parenthesis is
//! written only where Rust's precedence needs one, so the output reads like hand-written code.
//! The recursion is bounded by the parser's depth limit; the recursive functions keep their frames
//! small by leaving token construction to non-recursive helpers.

use crate::ast::{BinOp, Expr, ExprKind, Lit, Span, UnOp};
use crate::error::{TypeError, TypeErrorKind};
use proc_macro2::TokenStream;
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

/// What a reference resolves to: its type and a Rust expression that reads it. The expression must
/// be an atom for precedence purposes (a path, field access, call or index, as in `self.token`),
/// a place or value of the Rust type for `ty`; it is only ever read (compared, copied, borrowed),
/// never moved out of.
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

/// Translate an expression whose type is determined by its references and typed literals. The
/// result is a complete Rust expression; parenthesise it when using it as an operand.
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

// Binding strength of an emitted Rust expression, weakest first.
const OR: u8 = 10;
const AND: u8 = 20;
const CMP: u8 = 30;
const ADD: u8 = 40;
const MUL: u8 = 50;
const UNARY: u8 = 60;
/// Atoms and postfix chains (method calls, `?`, blocks, literals, paths).
const ATOM: u8 = 70;

/// `tokens`, parenthesised when its strength `prec` is below what the position needs.
fn at(tokens: TokenStream, prec: u8, min: u8) -> TokenStream {
    if prec < min {
        quote!((#tokens))
    } else {
        tokens
    }
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
    prec: u8,
}

impl Out {
    fn known(ty: Ty, tokens: TokenStream) -> Self {
        Self::known_at(ty, tokens, ATOM)
    }

    fn known_at(ty: Ty, tokens: TokenStream, prec: u8) -> Self {
        Out {
            ty: Inf::Known(ty),
            tokens,
            prec,
        }
    }

    fn pending(ty: Inf) -> Self {
        Out {
            ty,
            tokens: TokenStream::new(),
            prec: ATOM,
        }
    }

    /// The tokens, parenthesised if weaker than `min`.
    fn at(self, min: u8) -> TokenStream {
        at(self.tokens, self.prec, min)
    }
}

struct Cx<'a> {
    scope: &'a dyn Scope,
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

    fn require_bool(&self, e: &Expr, out: Out, op: &str) -> Result<Out, TypeError> {
        let got = match &out.ty {
            Inf::Known(Ty::Bool) => return Ok(out),
            Inf::Known(t) => t.to_string(),
            Inf::Int | Inf::Float => "a number".to_string(),
            Inf::Null => "null".to_string(),
        };
        Err(self.err(
            e,
            TypeErrorKind::NotBoolean,
            format!("operator \"{op}\" needs a boolean, got {got}"),
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

    /// A number literal (`text`, negated when `neg`) at type `ty`; the range of `ty` is enforced.
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
            let fits = text.parse::<u128>().is_ok_and(|v| match (signed, neg) {
                (false, false) => v < 1u128 << bits,
                (false, true) => v == 0,
                (true, false) => v < 1u128 << (bits - 1),
                (true, true) => v <= 1u128 << (bits - 1),
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
            let negative = neg && digits != "0";
            let sign = if negative { "-" } else { "" };
            let tokens = self.lex(e, &format!("{sign}{digits}{suffix}"))?;
            let prec = if negative { UNARY } else { ATOM };
            return Ok(Out::known_at(ty.clone(), tokens, prec));
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
            let tokens = self.lex(e, &format!("{sign}{v:?}f64"))?;
            let prec = if neg { UNARY } else { ATOM };
            return Ok(Out::known_at(ty.clone(), tokens, prec));
        }
        Err(self.err(
            e,
            TypeErrorKind::Mismatch,
            format!("a number cannot be used where {ty} is expected"),
        ))
    }

    fn go(&self, e: &Expr, hint: Option<&Ty>) -> Result<Out, TypeError> {
        match &e.kind {
            ExprKind::Lit(l) => self.lit(e, l, hint),
            ExprKind::Ref(path) => self.reference(e, path),
            ExprKind::Un(UnOp::Not, a) => self.not(a),
            ExprKind::Un(UnOp::Neg, a) => self.neg(e, a, hint),
            ExprKind::Bin(op, a, b) => self.binary(e, *op, a, b, hint),
        }
    }

    #[inline(never)]
    fn lit(&self, e: &Expr, l: &Lit, hint: Option<&Ty>) -> Result<Out, TypeError> {
        match l {
            Lit::Bool(b) => Ok(Out::known(Ty::Bool, quote!(#b))),
            Lit::Str(s) => Ok(Out::known(
                Ty::String,
                quote!(::std::string::String::from(#s)),
            )),
            Lit::Null => match hint {
                Some(t @ Ty::Option(_)) => {
                    Ok(Out::known(t.clone(), quote!(::core::option::Option::None)))
                }
                Some(t) => Err(self.err(
                    e,
                    TypeErrorKind::NullNeedsOption,
                    format!("null is only an option value, but {t} is expected"),
                )),
                None => Ok(Out::pending(Inf::Null)),
            },
            Lit::Num(text) => self.literal(e, text, false, hint),
        }
    }

    #[inline(never)]
    fn reference(&self, e: &Expr, path: &[String]) -> Result<Out, TypeError> {
        let parts: Vec<&str> = path.iter().map(String::as_str).collect();
        match self.scope.resolve(&parts) {
            Some(b) => Ok(Out::known(b.ty, b.tokens)),
            None => Err(self.err(
                e,
                TypeErrorKind::UnknownReference,
                format!("Unknown reference \"{}\"", path.join(".")),
            )),
        }
    }

    fn not(&self, a: &Expr) -> Result<Out, TypeError> {
        let o = self.go(a, None)?;
        let o = self.require_bool(a, o, "!")?;
        Ok(Self::negate_bool(o))
    }

    #[inline(never)]
    fn negate_bool(o: Out) -> Out {
        let t = o.at(UNARY);
        Out::known_at(Ty::Bool, quote!(!#t), UNARY)
    }

    fn neg(&self, e: &Expr, a: &Expr, hint: Option<&Ty>) -> Result<Out, TypeError> {
        if let ExprKind::Lit(Lit::Num(text)) = &a.kind {
            return self.literal(e, text, true, hint);
        }
        let o = self.go(a, hint)?;
        match &o.ty {
            Inf::Int | Inf::Float => Ok(o),
            Inf::Null => Err(self.err(
                a,
                TypeErrorKind::BadOperand,
                "operator \"-\" needs a number, got null",
            )),
            Inf::Known(t) => {
                let t = t.clone();
                if let Some((signed, _, _)) = t.int_info() {
                    if !signed {
                        return Err(self.err(
                            e,
                            TypeErrorKind::BadOperand,
                            format!("cannot negate {t}: it is unsigned"),
                        ));
                    }
                    Ok(self.checked_neg(t, o))
                } else if is_float(&t) {
                    Ok(Self::float_neg(t, o))
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

    #[inline(never)]
    fn checked_neg(&self, ty: Ty, o: Out) -> Out {
        let v = o.at(ATOM);
        let ov = self.scope.overflow_error();
        Out::known(ty, quote!(#v.checked_neg().ok_or(#ov)?))
    }

    #[inline(never)]
    fn float_neg(ty: Ty, o: Out) -> Out {
        let v = o.at(UNARY);
        Out::known_at(ty, quote!(-#v), UNARY)
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

    /// Both operands at one common type, taking literals' types from the other side. This is on
    /// the recursive path, so error construction lives in `settle` and its helpers.
    fn pair(&self, a: &Expr, b: &Expr, hint: Option<&Ty>) -> Result<Pair, TypeError> {
        let mut l = self.go(a, hint)?;
        let mut r = self.go(b, hint)?;
        if let Some(x) = Self::known_ty(&l).filter(|_| Self::known_ty(&r).is_none()) {
            r = self.go(b, Some(&x))?;
        } else if let Some(y) = Self::known_ty(&r).filter(|_| Self::known_ty(&l).is_none()) {
            l = self.go(a, Some(&y))?;
        }
        self.settle(a, b, l, r)
    }

    #[inline(never)]
    fn known_ty(o: &Out) -> Option<Ty> {
        match &o.ty {
            Inf::Known(t) => Some(t.clone()),
            _ => None,
        }
    }

    #[inline(never)]
    fn settle(&self, a: &Expr, b: &Expr, l: Out, r: Out) -> Result<Pair, TypeError> {
        match (&l.ty, &r.ty) {
            (Inf::Known(x), Inf::Known(y)) if x == y => {
                let ty = x.clone();
                Ok(Pair::Typed(ty, l, r))
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
            (Inf::Known(_), _) => Err(self.null_misuse(b)),
            (_, Inf::Known(_)) => Err(self.null_misuse(a)),
            (x, y) => {
                let float = *x == Inf::Float || *y == Inf::Float;
                let null = *x == Inf::Null || *y == Inf::Null;
                Ok(Pair::Untyped(if null {
                    Inf::Null
                } else if float {
                    Inf::Float
                } else {
                    Inf::Int
                }))
            }
        }
    }

    #[cold]
    #[inline(never)]
    fn null_misuse(&self, e: &Expr) -> TypeError {
        self.err(
            e,
            TypeErrorKind::NullNeedsOption,
            "null can only be used with an option value",
        )
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
            BinOp::And | BinOp::Or | BinOp::Implies => self.logical_expr(op, a, b),
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                match self.pair(a, b, None)? {
                    Pair::Typed(ty, mut l, mut r) => {
                        // A String compares with a `&str` directly, so a literal operand of an
                        // equality stays a plain literal instead of building a String to compare.
                        if matches!(op, BinOp::Eq | BinOp::Ne) && ty == Ty::String {
                            if let ExprKind::Lit(Lit::Str(s)) = &a.kind {
                                l = Out::known(Ty::String, quote!(#s));
                            }
                            if let ExprKind::Lit(Lit::Str(s)) = &b.kind {
                                r = Out::known(Ty::String, quote!(#s));
                            }
                        }
                        self.compare(e, op, ty, l, r)
                    }
                    Pair::Untyped(p) => Err(self.untyped_comparison(e, &p)),
                }
            }
            _ => match self.pair(a, b, hint)? {
                Pair::Typed(ty, l, r) => self.arithmetic(e, op, ty, l, r),
                Pair::Untyped(Inf::Null) => Err(self.null_arithmetic(e, op)),
                Pair::Untyped(p) => Ok(Out::pending(p)),
            },
        }
    }

    fn logical_expr(&self, op: BinOp, a: &Expr, b: &Expr) -> Result<Out, TypeError> {
        let l = self.go(a, None)?;
        let l = self.require_bool(a, l, op.symbol())?;
        let r = self.go(b, None)?;
        let r = self.require_bool(b, r, op.symbol())?;
        Ok(Self::logical(op, l, r))
    }

    #[cold]
    #[inline(never)]
    fn untyped_comparison(&self, e: &Expr, p: &Inf) -> TypeError {
        if *p == Inf::Null {
            self.err(
                e,
                TypeErrorKind::NullNeedsOption,
                "null can only be compared with an option value",
            )
        } else {
            self.err(
                e,
                TypeErrorKind::LiteralNeedsType,
                "both operands are literals: one of them needs a type",
            )
        }
    }

    #[cold]
    #[inline(never)]
    fn null_arithmetic(&self, e: &Expr, op: BinOp) -> TypeError {
        self.err(
            e,
            TypeErrorKind::BadOperand,
            format!("operator \"{}\" needs numbers, got null", op.symbol()),
        )
    }

    #[inline(never)]
    fn logical(op: BinOp, l: Out, r: Out) -> Out {
        match op {
            BinOp::And => {
                let (l, r) = (l.at(AND), r.at(AND));
                Out::known_at(Ty::Bool, quote!(#l && #r), AND)
            }
            BinOp::Or => {
                let (l, r) = (l.at(OR), r.at(OR));
                Out::known_at(Ty::Bool, quote!(#l || #r), OR)
            }
            _ => {
                let (l, r) = (l.at(UNARY), r.at(OR));
                Out::known_at(Ty::Bool, quote!(!#l || #r), OR)
            }
        }
    }

    #[inline(never)]
    fn compare(&self, e: &Expr, op: BinOp, ty: Ty, l: Out, r: Out) -> Result<Out, TypeError> {
        let ordering = !matches!(op, BinOp::Eq | BinOp::Ne);
        if ordering
            && !(ty.is_integer() || is_float(&ty) || matches!(ty, Ty::Timestamp | Ty::Duration))
        {
            return Err(self.err(
                e,
                TypeErrorKind::BadOperand,
                format!("operator \"{}\" needs numbers, got {ty}", op.symbol()),
            ));
        }
        // Comparison operators do not chain in Rust: a comparison operand is parenthesised.
        let (l, r) = (l.at(ADD), r.at(ADD));
        let t = match op {
            BinOp::Eq => quote!(#l == #r),
            BinOp::Ne => quote!(#l != #r),
            BinOp::Lt => quote!(#l < #r),
            BinOp::Le => quote!(#l <= #r),
            BinOp::Gt => quote!(#l > #r),
            _ => quote!(#l >= #r),
        };
        Ok(Out::known_at(Ty::Bool, t, CMP))
    }

    #[inline(never)]
    fn arithmetic(&self, e: &Expr, op: BinOp, ty: Ty, l: Out, r: Out) -> Result<Out, TypeError> {
        let sym = op.symbol();
        if ty == Ty::String {
            if op != BinOp::Add {
                return Err(self.err(
                    e,
                    TypeErrorKind::BadOperand,
                    format!("operator \"{sym}\" needs numbers, got string"),
                ));
            }
            let (l, r) = (l.at(UNARY), r.at(UNARY));
            return Ok(Out::known(
                ty,
                quote!({
                    let mut __s = ::std::string::String::from(&*#l);
                    __s.push_str(&*#r);
                    __s
                }),
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
            let (l, arg) = (l.at(ATOM), r.tokens);
            let t = match op {
                BinOp::Add => quote!(#l.checked_add(#arg).ok_or(#ov)?),
                BinOp::Sub => quote!(#l.checked_sub(#arg).ok_or(#ov)?),
                BinOp::Mul => quote!(#l.checked_mul(#arg).ok_or(#ov)?),
                BinOp::Rem => quote!(#l.checked_rem(#arg).ok_or(#ov)?),
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
            let out = match op {
                BinOp::Add => {
                    let (l, r) = (l.at(ADD), r.at(MUL));
                    Out::known_at(ty, quote!(#l + #r), ADD)
                }
                BinOp::Sub => {
                    let (l, r) = (l.at(ADD), r.at(MUL));
                    Out::known_at(ty, quote!(#l - #r), ADD)
                }
                BinOp::Mul => {
                    let (l, r) = (l.at(MUL), r.at(UNARY));
                    Out::known_at(ty, quote!(#l * #r), MUL)
                }
                BinOp::Div => {
                    let (l, r) = (l.tokens, r.tokens);
                    Out::known(
                        ty,
                        quote!({
                            let __l = #l;
                            let __r = #r;
                            if __r == 0.0f64 {
                                return ::core::result::Result::Err(#ov);
                            }
                            __l / __r
                        }),
                    )
                }
                _ => {
                    let (l, r) = (l.tokens, r.tokens);
                    Out::known(
                        ty,
                        quote!({
                            let __l = #l;
                            let __r = #r;
                            if __r == 0.0f64 {
                                return ::core::result::Result::Err(#ov);
                            }
                            __l % __r
                        }),
                    )
                }
            };
            return Ok(out);
        }
        Err(self.err(
            e,
            TypeErrorKind::BadOperand,
            format!("operator \"{sym}\" needs numbers, got {ty}"),
        ))
    }
}

/// Two operands settled at one type, or both still without one.
enum Pair {
    Typed(Ty, Out, Out),
    Untyped(Inf),
}
