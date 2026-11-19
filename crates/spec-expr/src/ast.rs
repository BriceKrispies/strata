//! The syntax tree of the Exegra expression language.

use std::rc::Rc;

/// A byte range of the source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

/// A literal. Numbers keep their source text so that integer and fractional literals stay distinct
/// and an integer literal can be checked against the width of the type it takes.
#[derive(Debug, Clone, PartialEq)]
pub enum Lit {
    Num(String),
    Str(String),
    Bool(bool),
    Null,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Not,
    Neg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Implies,
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

impl UnOp {
    pub fn symbol(self) -> &'static str {
        match self {
            UnOp::Not => "!",
            UnOp::Neg => "-",
        }
    }
}

impl BinOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Implies => "=>",
            BinOp::Or => "||",
            BinOp::And => "&&",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Lit(Lit),
    /// A dotted reference, one entry per name.
    Ref(Vec<String>),
    Un(UnOp, Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
}

/// A node with the source it came from, so that any error can point at it.
#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
    pub source: Rc<str>,
}

impl PartialEq for Expr {
    /// Structural equality: spans and source text are ignored.
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

impl Expr {
    /// The text this node was parsed from.
    pub fn text(&self) -> &str {
        &self.source[self.span.start..self.span.end]
    }

    /// A fully parenthesised prefix rendering, e.g. `(+ a (* b c))`. Parentheses of the source do
    /// not appear: the tree already carries the grouping.
    pub fn to_sexpr(&self) -> String {
        match &self.kind {
            ExprKind::Lit(Lit::Num(t)) => t.clone(),
            ExprKind::Lit(Lit::Str(s)) => format!("{s:?}"),
            ExprKind::Lit(Lit::Bool(b)) => b.to_string(),
            ExprKind::Lit(Lit::Null) => "null".to_string(),
            ExprKind::Ref(p) => p.join("."),
            ExprKind::Un(op, a) => format!("({} {})", op.symbol(), a.to_sexpr()),
            ExprKind::Bin(op, a, b) => {
                format!("({} {} {})", op.symbol(), a.to_sexpr(), b.to_sexpr())
            }
        }
    }
}
