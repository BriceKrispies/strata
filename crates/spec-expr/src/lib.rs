//! The Exegra expression language as used by the Strata specification: a parser that follows
//! `src/core/expr.js`, a type checker against representation types, and a translation to Rust.
//!
//! Grammar, lowest precedence first:
//!
//! ```text
//! implies  :  a => b            (right associative)
//! or       :  a || b
//! and      :  a && b
//! equality :  a == b, a != b
//! compare  :  a < b, a <= b, a > b, a >= b
//! additive :  a + b, a - b
//! multiply :  a * b, a / b, a % b
//! unary    :  !a, -a
//! primary  :  number | 'string' | "string" | true | false | null | reference | ( expr )
//! reference:  ident ( '.' ident )*     identifiers may contain inner hyphens
//! ```
//!
//! The generated expressions use `?` and, for `f64` division, `return Err(..)`: they belong in a
//! function returning `Result<_, E>` where `E` is the scope's overflow error type. They are
//! generously parenthesised, so the generated module should allow `unused_parens`.

mod ast;
mod check;
mod error;
mod parse;

pub use ast::{BinOp, Expr, ExprKind, Lit, Span, UnOp};
pub use check::{to_rust, to_rust_as, type_of, Binding, Scope, Ty};
pub use error::{ParseError, TypeError, TypeErrorKind};
pub use parse::parse;
