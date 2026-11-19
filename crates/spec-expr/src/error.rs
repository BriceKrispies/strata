//! Errors, each with the source text and a byte span, rendered as readable messages.

use crate::ast::Span;
use std::fmt;

fn render(f: &mut fmt::Formatter<'_>, message: &str, source: &str, span: Span) -> fmt::Result {
    let start = span.start.min(source.len());
    let end = span.end.clamp(start, source.len());
    let col = source.get(..start).map_or(0, |s| s.chars().count());
    let width = source
        .get(start..end)
        .map_or(1, |s| s.chars().count())
        .max(1);
    write!(
        f,
        "{message} (at byte {start})\n  | {source}\n  | {}{}",
        " ".repeat(col),
        "^".repeat(width)
    )
}

/// A syntax error. Messages are those of the Exegra parser; `pos` is a byte offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub pos: usize,
    pub source: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        render(
            f,
            &self.message,
            &self.source,
            Span {
                start: self.pos,
                end: self.pos + 1,
            },
        )
    }
}

impl std::error::Error for ParseError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeErrorKind {
    /// A reference the scope does not resolve.
    UnknownReference,
    /// Operand types differ, or an expression has a type other than the one required.
    Mismatch,
    /// A boolean is required (logical operators, `!`).
    NotBoolean,
    /// The operator does not apply to the operand type.
    BadOperand,
    /// An integer literal outside the range of its type, or a fractional literal for an integer.
    LiteralDoesNotFit,
    /// A literal (or literal-only expression) with no other operand to take a type from.
    LiteralNeedsType,
    /// `null` used where the type is not an option.
    NullNeedsOption,
    /// Legal in Exegra but not representable faithfully in Rust.
    Unsupported,
}

/// A type error with the expression it concerns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeError {
    pub kind: TypeErrorKind,
    pub message: String,
    pub span: Span,
    pub source: String,
}

impl fmt::Display for TypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        render(f, &self.message, &self.source, self.span)
    }
}

impl std::error::Error for TypeError {}
