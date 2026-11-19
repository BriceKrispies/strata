//! Type checking and translation to JavaScript, for the browser's frontend kernel.
//!
//! The typing rules are those of the Rust target (`check.rs`) so that one specification compiles
//! to both targets or to neither: same operand rules, same refusal of integer `/` and of
//! timestamp/duration arithmetic, same refusal of literals that do not fit.
//!
//! Semantics follow `src/core/expr.js` of Exegra: `==`/`!=` are `===`/`!==`, `&&`, `||` and `=>`
//! short-circuit, `+` on strings concatenates, `null` is an option's `None`. A `json` value is
//! compared structurally (as `serde_json::Value` is in the Rust target), with an inline deep
//! equality, and is otherwise refused as in the Rust target. Operand types are
//! established statically, so the dynamic type checks Exegra performs at evaluation cannot fail
//! in well-typed output and are not emitted.
//!
//! Integers are JavaScript numbers (doubles). Every integer `+ - * %` and integer negation checks
//! its result against the range of the integer type and evaluates the scope's overflow
//! expression when it is outside, never wrapping and never losing precision. For `u64` and `i64`
//! the range is the safe-integer range, `+-(2^53 - 1)`, since a double cannot represent more;
//! integer literals of those types beyond it are refused (`LiteralDoesNotFit`). `f64` division and
//! remainder by zero evaluate the overflow expression, as the Rust target returns its error.
//!
//! The generated code is self-contained: checks are immediately invoked arrow functions with
//! `$`-prefixed parameters, and there is no runtime module to import. The only host contract is
//! `JsScope::overflow_error_js`, an expression that throws when evaluated, e.g.
//! `__overflow()` where `function __overflow() { throw new StepError('overflow'); }`.

use crate::ast::{BinOp, Expr, ExprKind, Lit, Span, UnOp};
use crate::check::Ty;
use crate::error::{TypeError, TypeErrorKind};

/// What a reference resolves to: its type and a JavaScript expression that reads it. The
/// expression must be an atom for precedence purposes (identifier, member access, call, index).
#[derive(Debug, Clone)]
pub struct JsBinding {
    pub ty: Ty,
    pub js: String,
}

pub trait JsScope {
    /// Resolve a dotted reference such as `["token"]` or `["lease","count","jl-done"]`.
    fn resolve_js(&self, path: &[&str]) -> Option<JsBinding>;
    /// A JavaScript expression that throws the step's overflow error when evaluated (checked
    /// arithmetic out of range, division or remainder by zero).
    fn overflow_error_js(&self) -> String;
}

/// A complete JavaScript expression for `expr`, whose type is determined by its references and
/// typed literals. Parenthesise it when using it as an operand.
pub fn to_js(expr: &Expr, scope: &dyn JsScope) -> Result<String, TypeError> {
    let cx = Cx { scope };
    let out = cx.go(expr, None)?;
    cx.known(expr, out).map(|(_, t)| t)
}

/// As [`to_js`], for an expression that must have type `expected`: literals take that type.
pub fn to_js_as(expr: &Expr, scope: &dyn JsScope, expected: &Ty) -> Result<String, TypeError> {
    let cx = Cx { scope };
    let out = cx.go(expr, Some(expected))?;
    let (ty, js) = cx.known(expr, out)?;
    if &ty != expected {
        return Err(cx.err(
            expr,
            TypeErrorKind::Mismatch,
            format!("expected {expected}, found {ty}"),
        ));
    }
    Ok(js)
}

const OR: u8 = 10;
const AND: u8 = 20;
const CMP: u8 = 30;
const ADD: u8 = 40;
const MUL: u8 = 50;
const UNARY: u8 = 60;
const ATOM: u8 = 70;

#[derive(Debug, Clone, PartialEq)]
enum Inf {
    Known(Ty),
    Int,
    Float,
    Null,
}

struct Out {
    ty: Inf,
    js: String,
    prec: u8,
}

impl Out {
    fn known_at(ty: Ty, js: String, prec: u8) -> Self {
        Out {
            ty: Inf::Known(ty),
            js,
            prec,
        }
    }

    fn known(ty: Ty, js: String) -> Self {
        Self::known_at(ty, js, ATOM)
    }

    fn pending(ty: Inf) -> Self {
        Out {
            ty,
            js: String::new(),
            prec: ATOM,
        }
    }

    fn at(self, min: u8) -> String {
        if self.prec < min {
            format!("({})", self.js)
        } else {
            self.js
        }
    }

    /// As `at`, and parenthesised when it starts with `-` (so `-` + operand never forms `--`).
    fn after_minus(self, min: u8) -> String {
        let s = self.at(min);
        if s.starts_with('-') {
            format!("({s})")
        } else {
            s
        }
    }
}

enum Pair {
    Typed(Ty, Out, Out),
    Untyped(Inf),
}

/// Structural equality of two JSON values, as `serde_json::Value`'s `==`: objects by their keys
/// and members in any order, arrays element by element, everything else by `===`.
const JSON_EQ: &str = "(($a, $b) => { const $eq = (x, y) => x === y || (typeof x === 'object' && typeof y === 'object' && x !== null && y !== null && Array.isArray(x) === Array.isArray(y) && Object.keys(x).length === Object.keys(y).length && Object.keys(x).every((k) => Object.prototype.hasOwnProperty.call(y, k) && $eq(x[k], y[k]))); return $eq($a, $b); })";

fn is_float(t: &Ty) -> bool {
    matches!(t, Ty::F64)
}

/// The largest integer a double holds exactly in every step, `2^53 - 1`.
const SAFE: i128 = (1 << 53) - 1;

/// Inclusive range of an integer type as checked by the JavaScript target.
fn range(signed: bool, bits: u32) -> (i128, i128) {
    if bits >= 64 {
        (if signed { -SAFE } else { 0 }, SAFE)
    } else if signed {
        (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
    } else {
        (0, (1i128 << bits) - 1)
    }
}

/// A JavaScript string literal for `s`.
fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' || c == '\u{2028}' || c == '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

struct Cx<'a> {
    scope: &'a dyn JsScope,
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

    fn known(&self, e: &Expr, out: Out) -> Result<(Ty, String), TypeError> {
        match out.ty {
            Inf::Known(t) => Ok((t, out.js)),
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

    fn number(&self, e: &Expr, text: &str, neg: bool, ty: &Ty) -> Result<Out, TypeError> {
        let fractional = text.contains('.');
        let shown = if neg {
            format!("-{text}")
        } else {
            text.to_string()
        };
        if let Some((signed, bits, _)) = ty.int_info() {
            if fractional {
                return Err(self.err(
                    e,
                    TypeErrorKind::LiteralDoesNotFit,
                    format!("literal {shown} is not an integer and cannot be a {ty}"),
                ));
            }
            let (lo, hi) = range(signed, bits);
            let fits = text.parse::<i128>().is_ok_and(|v| {
                let v = if neg { -v } else { v };
                v >= lo && v <= hi
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
            let js = if negative {
                format!("-{digits}")
            } else {
                digits.to_string()
            };
            let prec = if negative { UNARY } else { ATOM };
            return Ok(Out::known_at(ty.clone(), js, prec));
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
            let js = if neg { format!("-{v}") } else { format!("{v}") };
            let prec = if neg { UNARY } else { ATOM };
            return Ok(Out::known_at(ty.clone(), js, prec));
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
            Lit::Bool(b) => Ok(Out::known(Ty::Bool, b.to_string())),
            Lit::Str(s) => Ok(Out::known(Ty::String, js_string(s))),
            Lit::Null => match hint {
                Some(t @ Ty::Option(_)) => Ok(Out::known(t.clone(), "null".to_string())),
                Some(Ty::Json) => Ok(Out::known(Ty::Json, "null".to_string())),
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
        match self.scope.resolve_js(&parts) {
            Some(b) => Ok(Out::known(b.ty, b.js)),
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
        Ok(Out::known_at(Ty::Bool, format!("!{}", o.at(UNARY)), UNARY))
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
                if let Some((signed, bits, _)) = t.int_info() {
                    if !signed {
                        return Err(self.err(
                            e,
                            TypeErrorKind::BadOperand,
                            format!("cannot negate {t}: it is unsigned"),
                        ));
                    }
                    let js = self.checked(signed, bits, format!("0 - {}", o.at(MUL)));
                    Ok(Out::known(t, js))
                } else if is_float(&t) {
                    Ok(Out::known_at(
                        t,
                        format!("-{}", o.after_minus(UNARY)),
                        UNARY,
                    ))
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

    /// `expr` evaluated once and returned if within the range of the integer type, else the
    /// overflow expression.
    fn checked(&self, signed: bool, bits: u32, expr: String) -> String {
        let (lo, hi) = range(signed, bits);
        let ov = self.scope.overflow_error_js();
        format!("(($v) => $v >= {lo} && $v <= {hi} ? $v : {ov})({expr})")
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
                    Pair::Typed(ty, l, r) => self.compare(e, op, ty, l, r),
                    Pair::Untyped(p) => Err(self.untyped_comparison(e, &p)),
                }
            }
            _ => match self.pair(a, b, hint)? {
                Pair::Typed(ty, l, r) => self.arithmetic(e, op, ty, l, r),
                Pair::Untyped(Inf::Null) => Err(self.err(
                    e,
                    TypeErrorKind::BadOperand,
                    format!("operator \"{}\" needs numbers, got null", op.symbol()),
                )),
                Pair::Untyped(p) => Ok(Out::pending(p)),
            },
        }
    }

    fn logical_expr(&self, op: BinOp, a: &Expr, b: &Expr) -> Result<Out, TypeError> {
        let l = self.go(a, None)?;
        let l = self.require_bool(a, l, op.symbol())?;
        let r = self.go(b, None)?;
        let r = self.require_bool(b, r, op.symbol())?;
        Ok(match op {
            BinOp::And => Out::known_at(Ty::Bool, format!("{} && {}", l.at(AND), r.at(AND)), AND),
            BinOp::Or => Out::known_at(Ty::Bool, format!("{} || {}", l.at(OR), r.at(OR)), OR),
            _ => Out::known_at(Ty::Bool, format!("!{} || {}", l.at(UNARY), r.at(OR)), OR),
        })
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
        if ty == Ty::Json {
            let not = if op == BinOp::Ne { "!" } else { "" };
            return Ok(Out::known_at(
                Ty::Bool,
                format!("{not}{JSON_EQ}({}, {})", l.js, r.js),
                if op == BinOp::Ne { UNARY } else { ATOM },
            ));
        }
        let sym = match op {
            BinOp::Eq => "===",
            BinOp::Ne => "!==",
            other => other.symbol(),
        };
        // Comparison operands are parenthesised unless they bind tighter than `+`'s result.
        let (l, r) = (l.at(ADD), r.at(ADD));
        Ok(Out::known_at(Ty::Bool, format!("{l} {sym} {r}"), CMP))
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
            return Ok(Out::known_at(
                ty,
                format!("{} + {}", l.at(ADD), r.at(MUL)),
                ADD,
            ));
        }
        if matches!(ty, Ty::Timestamp | Ty::Duration) {
            return Err(self.err(
                e,
                TypeErrorKind::Unsupported,
                format!("arithmetic on {ty} is not supported: its representation has no fixed arithmetic"),
            ));
        }
        let ov = self.scope.overflow_error_js();
        if let Some((signed, bits, _)) = ty.int_info() {
            let js = match op {
                BinOp::Add | BinOp::Sub | BinOp::Mul => {
                    let (lp, rp) = match op {
                        BinOp::Mul => (MUL, UNARY),
                        _ => (ADD, MUL),
                    };
                    self.checked(signed, bits, format!("{} {sym} {}", l.at(lp), r.at(rp)))
                }
                BinOp::Rem => {
                    let (lo, _) = range(signed, bits);
                    let bad = if signed {
                        format!("$r === 0 || ($r === -1 && $l === {lo})")
                    } else {
                        "$r === 0".to_string()
                    };
                    format!(
                        "(($l, $r) => {bad} ? {ov} : $l % $r)({}, {})",
                        l.js, r.js
                    )
                }
                _ => {
                    return Err(self.err(
                        e,
                        TypeErrorKind::Unsupported,
                        format!(
                            "integer division is not supported: Exegra's \"/\" on {ty} is the exact quotient (7 / 2 is 3.5) while integer division truncates"
                        ),
                    ))
                }
            };
            return Ok(Out::known(ty, js));
        }
        if is_float(&ty) {
            let out = match op {
                BinOp::Add => Out::known_at(ty, format!("{} + {}", l.at(ADD), r.at(MUL)), ADD),
                BinOp::Sub => Out::known_at(ty, format!("{} - {}", l.at(ADD), r.at(MUL)), ADD),
                BinOp::Mul => Out::known_at(ty, format!("{} * {}", l.at(MUL), r.at(UNARY)), MUL),
                _ => {
                    let f = if op == BinOp::Div { "/" } else { "%" };
                    Out::known(
                        ty,
                        format!(
                            "(($l, $r) => $r === 0 ? {ov} : $l {f} $r)({}, {})",
                            l.js, r.js
                        ),
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
