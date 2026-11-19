//! The body language of functions, parsed as Exegra parses it (`src/core/function-syntax.js`):
//! Exegra's guard language extended with field access, `if`, `let`, `match`, constructors, list
//! literals, operations, calls of other functions, `fail` and `try`.
//!
//! ```text
//! expr     : implies
//! implies  : or ( '=>' implies )?                       right associative
//! or       : and ( '||' and )*
//! and      : equality ( '&&' equality )*
//! equality : compare ( ( '==' | '!=' ) compare )*
//! compare  : additive ( ( '<' | '<=' | '>' | '>=' ) additive )*
//! additive : multiply ( ( '+' | '-' ) multiply )*
//! multiply : unary ( ( '*' | '/' | '%' ) unary )*
//! unary    : '!' unary | '-' unary | 'try' unary | postfix
//! postfix  : primary ( '.' name )*
//! primary  : number | string | 'true' | 'false' | 'null' | '(' expr ')'
//!          | '[' ( expr ( ',' expr )* )? ']' | '{' '}'
//!          | 'if' expr 'then' expr 'else' expr | 'let' name '=' expr 'in' expr
//!          | 'match' expr '{' arm ( ',' arm )* ','? '}'
//!          | 'call' name '(' ( expr ( ',' expr )* )? ')'
//!          | name '(' args ')'                           an operation
//!          | path '{' ( init ( ',' init )* ','? )? '}'   struct T{..} or variant T.V{..}
//!          | path '(' expr ')'                           newtype variant T.V(e)
//!          | path
//! ```

use super::typing::Ty;

/// The words the body language keeps for itself.
pub(crate) const KEYWORDS: &[&str] = &[
    "let", "in", "if", "then", "else", "match", "call", "try", "true", "false", "null",
];

/// The operations whose first argument binds a name over a list.
pub(crate) const BINDER_OPS: &[&str] = &["map", "filter", "find", "any", "all", "sortBy", "fold"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnOp {
    Not,
    Neg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BinOp {
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

impl BinOp {
    pub(crate) fn symbol(self) -> &'static str {
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

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "=>" => BinOp::Implies,
            "||" => BinOp::Or,
            "&&" => BinOp::And,
            "==" => BinOp::Eq,
            "!=" => BinOp::Ne,
            "<" => BinOp::Lt,
            "<=" => BinOp::Le,
            ">" => BinOp::Gt,
            ">=" => BinOp::Ge,
            "+" => BinOp::Add,
            "-" => BinOp::Sub,
            "*" => BinOp::Mul,
            "/" => BinOp::Div,
            "%" => BinOp::Rem,
            _ => return None,
        })
    }
}

/// What a reference path names, once typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Res {
    /// Not yet resolved.
    Unresolved,
    /// A parameter, let or bound name, then its fields.
    Var,
    /// An enum value `T.V`.
    Enum,
    /// A unit variant `T.V` of a union.
    Unit,
    /// The id of a language (the first argument of `eval` and `check`).
    Language,
}

/// A pattern of a match arm.
#[derive(Debug, Clone)]
pub(crate) enum Pattern {
    /// `_`
    Wild,
    /// `V`, `V(x)` or `V{f, g: y}`.
    Case {
        name: String,
        bind: Option<String>,
        fields: Option<Vec<(String, String)>>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct Arm {
    pub pat: Pattern,
    pub body: Expr,
}

/// An operation `name(args)`, with its binder `x in xs` and fold accumulator `acc = init`.
#[derive(Debug, Clone)]
pub(crate) struct Op {
    pub name: String,
    pub binder: Option<(String, Box<Expr>)>,
    pub acc: Option<(String, Box<Expr>)>,
    pub args: Vec<Expr>,
}

#[derive(Debug, Clone)]
pub(crate) enum Kind {
    /// A number as written.
    Num(String),
    Str(String),
    Bool(bool),
    Null,
    Ref(Vec<String>, Res),
    Un(UnOp, Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Field(Box<Expr>, String),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    Let(String, Box<Expr>, Box<Expr>),
    Match(Box<Expr>, Vec<Arm>),
    List(Vec<Expr>),
    EmptyMap,
    Ctor(Vec<String>, Vec<(String, Expr)>),
    Newtype(Vec<String>, Box<Expr>),
    Op(Op),
    Call(String, Vec<Expr>),
    Try(Box<Expr>),
}

/// An expression with its position and, once typed, its type.
#[derive(Debug, Clone)]
pub(crate) struct Expr {
    pub kind: Kind,
    pub pos: usize,
    pub ty: Option<Ty>,
}

impl Expr {
    fn new(kind: Kind, pos: usize) -> Self {
        Expr {
            kind,
            pos,
            ty: None,
        }
    }

    /// The type the checker gave this node; `Unknown` before checking.
    pub(crate) fn ty(&self) -> Ty {
        self.ty.clone().unwrap_or(Ty::Unknown)
    }
}

/// A syntax error and where it is.
#[derive(Debug, Clone)]
pub(crate) struct SyntaxError {
    pub message: String,
    pub pos: usize,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Str(String),
    Num(String),
    Id(String),
    Op(&'static str),
    End,
}

const OPS: &[&str] = &[
    "=>", "||", "&&", "==", "!=", "<=", ">=", "<", ">", "+", "-", "*", "/", "%", "!", "(", ")",
    ".", "{", "}", "[", "]", ",", ":", "=",
];

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The identifier at `i`: `[A-Za-z_][A-Za-z0-9_]*`, with inner hyphens each followed by
/// `[A-Za-z_]`.
fn ident_at(chars: &[(usize, char)], i: usize) -> Option<usize> {
    if !chars.get(i).is_some_and(|(_, c)| is_ident_start(*c)) {
        return None;
    }
    let mut j = i + 1;
    loop {
        while chars.get(j).is_some_and(|(_, c)| is_ident_char(*c)) {
            j += 1;
        }
        if chars.get(j).is_some_and(|(_, c)| *c == '-')
            && chars.get(j + 1).is_some_and(|(_, c)| is_ident_start(*c))
        {
            j += 2;
            continue;
        }
        return Some(j);
    }
}

fn tokenize(src: &str) -> Result<Vec<(Tok, usize)>, SyntaxError> {
    let chars: Vec<(usize, char)> = src.char_indices().collect();
    let at = |i: usize| chars.get(i).map_or(src.len(), |(p, _)| *p);
    let text = |i: usize, j: usize| src[at(i)..at(j)].to_string();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (pos, c) = chars[i];
        if matches!(c, ' ' | '\t' | '\n' | '\r') {
            i += 1;
            continue;
        }
        if c == '"' || c == '\'' {
            let Some(end) = (i + 1..chars.len()).find(|k| chars[*k].1 == c) else {
                return Err(SyntaxError {
                    message: "Unterminated string literal".to_string(),
                    pos,
                });
            };
            out.push((Tok::Str(text(i + 1, end)), pos));
            i = end + 1;
            continue;
        }
        if c.is_ascii_digit() {
            let mut j = i;
            while chars.get(j).is_some_and(|(_, d)| d.is_ascii_digit()) {
                j += 1;
            }
            if chars.get(j).is_some_and(|(_, d)| *d == '.')
                && chars.get(j + 1).is_some_and(|(_, d)| d.is_ascii_digit())
            {
                j += 1;
                while chars.get(j).is_some_and(|(_, d)| d.is_ascii_digit()) {
                    j += 1;
                }
            }
            out.push((Tok::Num(text(i, j)), pos));
            i = j;
            continue;
        }
        if let Some(j) = ident_at(&chars, i) {
            out.push((Tok::Id(text(i, j)), pos));
            i = j;
            continue;
        }
        let rest = &src[pos..];
        let Some(op) = OPS.iter().find(|o| rest.starts_with(**o)) else {
            return Err(SyntaxError {
                message: format!("Unexpected character '{c}'"),
                pos,
            });
        };
        out.push((Tok::Op(op), pos));
        i += op.chars().count();
    }
    out.push((Tok::End, src.len()));
    Ok(out)
}

/// A name a parameter, let or binder may take.
pub(crate) fn is_bindable_name(s: &str) -> bool {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    ident_at(&chars, 0) == Some(chars.len()) && !KEYWORDS.contains(&s) && s != "_"
}

/// Parses a body or let expression.
pub(crate) fn parse(src: &str) -> Result<Expr, SyntaxError> {
    let toks = tokenize(src)?;
    let mut p = Parser {
        toks,
        p: 0,
        no_struct: false,
    };
    let e = p.expr()?;
    if p.peek() != &Tok::End {
        return Err(p.error(format!("Unexpected {}", p.show())));
    }
    Ok(e)
}

struct Parser {
    toks: Vec<(Tok, usize)>,
    p: usize,
    no_struct: bool,
}

const BINARY: &[&[&str]] = &[
    &["=>"],
    &["||"],
    &["&&"],
    &["==", "!="],
    &["<", "<=", ">", ">="],
    &["+", "-"],
    &["*", "/", "%"],
];

type Parsed<T> = Result<T, SyntaxError>;

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.p].0
    }

    fn pos(&self) -> usize {
        self.toks[self.p].1
    }

    fn show(&self) -> String {
        match self.peek() {
            Tok::End => "the end of the expression".to_string(),
            Tok::Str(s) | Tok::Num(s) | Tok::Id(s) => format!("\"{s}\""),
            Tok::Op(o) => format!("\"{o}\""),
        }
    }

    fn error(&self, message: String) -> SyntaxError {
        SyntaxError {
            message,
            pos: self.pos(),
        }
    }

    fn is_op(&self, v: &str) -> bool {
        matches!(self.peek(), Tok::Op(o) if *o == v)
    }

    fn is_word(&self, v: &str) -> bool {
        matches!(self.peek(), Tok::Id(w) if w == v)
    }

    fn expect_op(&mut self, v: &str) -> Parsed<()> {
        if !self.is_op(v) {
            return Err(self.error(format!("Expected \"{v}\", found {}", self.show())));
        }
        self.p += 1;
        Ok(())
    }

    fn expect_word(&mut self, v: &str) -> Parsed<()> {
        if !self.is_word(v) {
            return Err(self.error(format!("Expected \"{v}\", found {}", self.show())));
        }
        self.p += 1;
        Ok(())
    }

    fn name(&mut self, what: &str) -> Parsed<String> {
        match self.peek().clone() {
            Tok::Id(w) if !KEYWORDS.contains(&w.as_str()) => {
                self.p += 1;
                Ok(w)
            }
            _ => Err(self.error(format!("Expected {what}, found {}", self.show()))),
        }
    }

    fn bind_name(&mut self, what: &str) -> Parsed<String> {
        let pos = self.pos();
        let n = self.name(what)?;
        if n == "_" {
            return Err(SyntaxError {
                message: format!("{what} cannot be \"_\""),
                pos,
            });
        }
        Ok(n)
    }

    /// A sub-expression in which a struct constructor is allowed again.
    fn nested<T>(&mut self, f: impl FnOnce(&mut Self) -> Parsed<T>) -> Parsed<T> {
        let saved = self.no_struct;
        self.no_struct = false;
        let r = f(self);
        self.no_struct = saved;
        r
    }

    fn expr(&mut self) -> Parsed<Expr> {
        self.binary(0)
    }

    fn binary(&mut self, level: usize) -> Parsed<Expr> {
        if level >= BINARY.len() {
            return self.unary();
        }
        let mut left = self.binary(level + 1)?;
        loop {
            let op = match self.peek() {
                Tok::Op(o) if BINARY[level].contains(o) => *o,
                _ => break,
            };
            let pos = self.pos();
            self.p += 1;
            let bin = BinOp::parse(op).expect("a binary operator of the table");
            if bin == BinOp::Implies {
                let right = self.binary(level)?;
                return Ok(Expr::new(
                    Kind::Bin(bin, Box::new(left), Box::new(right)),
                    pos,
                ));
            }
            let right = self.binary(level + 1)?;
            left = Expr::new(Kind::Bin(bin, Box::new(left), Box::new(right)), pos);
        }
        Ok(left)
    }

    fn unary(&mut self) -> Parsed<Expr> {
        let pos = self.pos();
        if self.is_op("!") || self.is_op("-") {
            let op = if self.is_op("!") {
                UnOp::Not
            } else {
                UnOp::Neg
            };
            self.p += 1;
            let a = self.unary()?;
            return Ok(Expr::new(Kind::Un(op, Box::new(a)), pos));
        }
        if self.is_word("try") {
            self.p += 1;
            let a = self.unary()?;
            return Ok(Expr::new(Kind::Try(Box::new(a)), pos));
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Parsed<Expr> {
        let mut e = self.primary()?;
        while self.is_op(".") {
            self.p += 1;
            let pos = self.pos();
            let name = self.name("a field name after \".\"")?;
            e = Expr::new(Kind::Field(Box::new(e), name), pos);
        }
        Ok(e)
    }

    fn pattern(&mut self) -> Parsed<Pattern> {
        let n = self.name("a pattern")?;
        if n == "_" {
            return Ok(Pattern::Wild);
        }
        if self.is_op("(") {
            self.p += 1;
            let bind = self.name("a name or _")?;
            self.expect_op(")")?;
            return Ok(Pattern::Case {
                name: n,
                bind: Some(bind),
                fields: None,
            });
        }
        if self.is_op("{") {
            self.p += 1;
            let mut fields = Vec::new();
            while !self.is_op("}") {
                let field = self.name("a field name")?;
                let mut bind = field.clone();
                if self.is_op(":") {
                    self.p += 1;
                    bind = self.name("a name or _")?;
                }
                fields.push((field, bind));
                if !self.is_op(",") {
                    break;
                }
                self.p += 1;
            }
            self.expect_op("}")?;
            return Ok(Pattern::Case {
                name: n,
                bind: None,
                fields: Some(fields),
            });
        }
        Ok(Pattern::Case {
            name: n,
            bind: None,
            fields: None,
        })
    }

    fn list_of_exprs(&mut self, close: &str) -> Parsed<Vec<Expr>> {
        let mut items = Vec::new();
        if !self.is_op(close) {
            items.push(self.expr()?);
            while self.is_op(",") {
                self.p += 1;
                items.push(self.expr()?);
            }
        }
        self.expect_op(close)?;
        Ok(items)
    }

    fn builtin(&mut self, name: String, pos: usize) -> Parsed<Expr> {
        self.expect_op("(")?;
        self.nested(|p| {
            let mut op = Op {
                name: name.clone(),
                binder: None,
                acc: None,
                args: Vec::new(),
            };
            if BINDER_OPS.contains(&name.as_str()) {
                let bname = p.bind_name("the name a list operation binds")?;
                p.expect_word("in")?;
                let src = p.expr()?;
                op.binder = Some((bname, Box::new(src)));
                p.expect_op(",")?;
                if name == "fold" {
                    let acc = p.bind_name("the accumulator name of fold")?;
                    p.expect_op("=")?;
                    let init = p.expr()?;
                    op.acc = Some((acc, Box::new(init)));
                    p.expect_op(",")?;
                }
                op.args.push(p.expr()?);
                p.expect_op(")")?;
            } else {
                op.args = p.list_of_exprs(")")?;
            }
            Ok(Expr::new(Kind::Op(op), pos))
        })
    }

    fn primary(&mut self) -> Parsed<Expr> {
        let pos = self.pos();
        match self.peek().clone() {
            Tok::Num(n) => {
                self.p += 1;
                return Ok(Expr::new(Kind::Num(n), pos));
            }
            Tok::Str(s) => {
                self.p += 1;
                return Ok(Expr::new(Kind::Str(s), pos));
            }
            Tok::Op("(") => {
                self.p += 1;
                let inner = self.nested(|p| p.expr())?;
                self.expect_op(")")?;
                return Ok(inner);
            }
            Tok::Op("[") => {
                self.p += 1;
                let items = self.nested(|p| p.list_of_exprs("]"))?;
                return Ok(Expr::new(Kind::List(items), pos));
            }
            Tok::Op("{") => {
                self.p += 1;
                self.expect_op("}")?;
                return Ok(Expr::new(Kind::EmptyMap, pos));
            }
            Tok::Id(_) => {}
            Tok::End => return Err(self.error("Unexpected end of expression".to_string())),
            Tok::Op(o) => return Err(self.error(format!("Unexpected \"{o}\""))),
        }
        let Tok::Id(w) = self.peek().clone() else {
            unreachable!("checked above")
        };
        match w.as_str() {
            "true" | "false" => {
                self.p += 1;
                return Ok(Expr::new(Kind::Bool(w == "true"), pos));
            }
            "null" => {
                self.p += 1;
                return Ok(Expr::new(Kind::Null, pos));
            }
            "if" => {
                self.p += 1;
                let c = self.nested(|p| p.expr())?;
                self.expect_word("then")?;
                let a = self.nested(|p| p.expr())?;
                self.expect_word("else")?;
                let b = self.expr()?;
                return Ok(Expr::new(
                    Kind::If(Box::new(c), Box::new(a), Box::new(b)),
                    pos,
                ));
            }
            "let" => {
                self.p += 1;
                let n = self.bind_name("the name a let binds")?;
                self.expect_op("=")?;
                let value = self.nested(|p| p.expr())?;
                self.expect_word("in")?;
                let body = self.expr()?;
                return Ok(Expr::new(
                    Kind::Let(n, Box::new(value), Box::new(body)),
                    pos,
                ));
            }
            "match" => {
                self.p += 1;
                let saved = self.no_struct;
                self.no_struct = true;
                let e = self.expr();
                self.no_struct = saved;
                let e = e?;
                self.expect_op("{")?;
                let arms = self.nested(|p| {
                    let mut arms = Vec::new();
                    while !p.is_op("}") {
                        let pat = p.pattern()?;
                        p.expect_op("=>")?;
                        let body = p.expr()?;
                        arms.push(Arm { pat, body });
                        if !p.is_op(",") {
                            break;
                        }
                        p.p += 1;
                    }
                    p.expect_op("}")?;
                    Ok(arms)
                })?;
                if arms.is_empty() {
                    return Err(SyntaxError {
                        message: "A match needs at least one arm".to_string(),
                        pos,
                    });
                }
                return Ok(Expr::new(Kind::Match(Box::new(e), arms), pos));
            }
            "call" => {
                self.p += 1;
                let f = self.name("the id of the function to call")?;
                self.expect_op("(")?;
                let args = self.nested(|p| p.list_of_exprs(")"))?;
                return Ok(Expr::new(Kind::Call(f, args), pos));
            }
            _ => {}
        }
        if KEYWORDS.contains(&w.as_str()) {
            return Err(self.error(format!("Unexpected \"{w}\"")));
        }
        self.p += 1;
        if self.is_op("(") {
            return self.builtin(w, pos);
        }
        let mut path = vec![w];
        while self.is_op(".") {
            self.p += 1;
            path.push(self.name("a name after \".\"")?);
        }
        if self.is_op("(") {
            self.p += 1;
            let arg = self.nested(|p| p.expr())?;
            self.expect_op(")")?;
            return Ok(Expr::new(Kind::Newtype(path, Box::new(arg)), pos));
        }
        if self.is_op("{") && !self.no_struct {
            self.p += 1;
            let fields = self.nested(|p| {
                let mut fields = Vec::new();
                while !p.is_op("}") {
                    let fpos = p.pos();
                    let f = p.name("a field name")?;
                    if p.is_op(":") {
                        p.p += 1;
                        let e = p.expr()?;
                        fields.push((f, e));
                    } else {
                        let e = Expr::new(Kind::Ref(vec![f.clone()], Res::Unresolved), fpos);
                        fields.push((f, e));
                    }
                    if !p.is_op(",") {
                        break;
                    }
                    p.p += 1;
                }
                p.expect_op("}")?;
                Ok(fields)
            })?;
            return Ok(Expr::new(Kind::Ctor(path, fields), pos));
        }
        Ok(Expr::new(Kind::Ref(path, Res::Unresolved), pos))
    }
}

/// The ids of the functions an expression calls, in order of appearance.
pub(crate) fn called(e: &Expr, out: &mut Vec<String>) {
    each_child(e, &mut |c| called(c, out));
    if let Kind::Call(f, _) = &e.kind {
        out.push(f.clone());
    }
}

/// Calls `f` on each direct sub-expression.
pub(crate) fn each_child(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    match &e.kind {
        Kind::Num(_)
        | Kind::Str(_)
        | Kind::Bool(_)
        | Kind::Null
        | Kind::Ref(..)
        | Kind::EmptyMap => {}
        Kind::Un(_, a) | Kind::Field(a, _) | Kind::Newtype(_, a) | Kind::Try(a) => f(a),
        Kind::Bin(_, a, b) | Kind::Let(_, a, b) => {
            f(a);
            f(b);
        }
        Kind::If(a, b, c) => {
            f(a);
            f(b);
            f(c);
        }
        Kind::Match(s, arms) => {
            f(s);
            for arm in arms {
                f(&arm.body);
            }
        }
        Kind::List(items) | Kind::Call(_, items) => items.iter().for_each(f),
        Kind::Ctor(_, fields) => fields.iter().for_each(|(_, e)| f(e)),
        Kind::Op(op) => {
            if let Some((_, src)) = &op.binder {
                f(src);
            }
            if let Some((_, init)) = &op.acc {
                f(init);
            }
            op.args.iter().for_each(f);
        }
    }
}

/// True when `name` occurs free in `e`: read as a name somewhere it is not bound again.
pub(crate) fn uses(e: &Expr, name: &str) -> bool {
    match &e.kind {
        Kind::Ref(path, _) => path[0] == name,
        Kind::Let(n, v, body) => uses(v, name) || (n != name && uses(body, name)),
        Kind::Match(s, arms) => {
            uses(s, name)
                || arms.iter().any(|arm| {
                    let shadowed = match &arm.pat {
                        Pattern::Wild => false,
                        Pattern::Case { bind, fields, .. } => {
                            bind.as_deref() == Some(name)
                                || fields.iter().flatten().any(|(_, b)| b == name)
                        }
                    };
                    !shadowed && uses(&arm.body, name)
                })
        }
        Kind::Op(op) => {
            let src = op.binder.as_ref().is_some_and(|(_, s)| uses(s, name));
            let init = op.acc.as_ref().is_some_and(|(_, i)| uses(i, name));
            let bound = op.binder.as_ref().is_some_and(|(b, _)| b == name)
                || op.acc.as_ref().is_some_and(|(a, _)| a == name);
            src || init || (!bound && op.args.iter().any(|a| uses(a, name)))
        }
        _ => {
            let mut found = false;
            each_child(e, &mut |c| found = found || uses(c, name));
            found
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_take_inner_hyphens_and_minus_stays_an_operator() {
        let e = parse("call may-attempt(a, b) - 1").unwrap();
        let Kind::Bin(BinOp::Sub, l, _) = &e.kind else {
            panic!("{e:?}")
        };
        assert!(matches!(&l.kind, Kind::Call(f, args) if f == "may-attempt" && args.len() == 2));
    }

    #[test]
    fn implies_is_right_associative_and_if_reaches_right() {
        let e = parse("a => b => c").unwrap();
        let Kind::Bin(BinOp::Implies, _, r) = &e.kind else {
            panic!()
        };
        assert!(matches!(r.kind, Kind::Bin(BinOp::Implies, ..)));
        let e = parse("if a then b else c + 1").unwrap();
        let Kind::If(_, _, otherwise) = &e.kind else {
            panic!()
        };
        assert!(matches!(otherwise.kind, Kind::Bin(BinOp::Add, ..)));
    }

    #[test]
    fn a_match_scrutinee_takes_no_struct_constructor() {
        let e = parse("match x { Some(v) => s{a: v}, None => s{a: 1} }").unwrap();
        let Kind::Match(s, arms) = &e.kind else {
            panic!()
        };
        assert!(matches!(&s.kind, Kind::Ref(p, _) if p == &["x"]));
        assert!(matches!(arms[0].body.kind, Kind::Ctor(..)));
    }

    #[test]
    fn binders_and_fold_accumulators_parse() {
        let e = parse("fold(r in xs, acc = [], concat(acc, [r]))").unwrap();
        let Kind::Op(op) = &e.kind else { panic!() };
        assert_eq!(op.binder.as_ref().unwrap().0, "r");
        assert_eq!(op.acc.as_ref().unwrap().0, "acc");
        assert_eq!(op.args.len(), 1);
        assert!(parse("map(_ in xs, 1)").is_err());
    }

    #[test]
    fn free_names_respect_shadowing() {
        let e = parse("let x = y in map(x in x, x)").unwrap();
        assert!(uses(&e, "y"));
        assert!(!uses(&e, "x"));
        let e = parse("match a { Some(e) => e, None => e }").unwrap();
        assert!(uses(&e, "e"));
    }
}
