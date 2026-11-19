//! Tokenizer and parser, following `src/core/expr.js` and `formal/Exegra/Expr.lean`.

use crate::ast::{BinOp, Expr, ExprKind, Lit, Span, UnOp};
use crate::error::ParseError;
use std::rc::Rc;

const OPS: [&str; 18] = [
    "=>", "||", "&&", "==", "!=", "<=", ">=", "<", ">", "+", "-", "*", "/", "%", "!", "(", ")", ".",
];

/// Binary operators, lowest precedence first.
const LEVELS: [&[&str]; 7] = [
    &["=>"],
    &["||"],
    &["&&"],
    &["==", "!="],
    &["<", "<=", ">", ">="],
    &["+", "-"],
    &["*", "/", "%"],
];

/// Nesting beyond this is refused (the JS parser would overflow its stack instead).
const MAX_DEPTH: usize = 200;

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Num(String),
    Str(String),
    Id(String),
    Op(&'static str),
    End,
}

#[derive(Debug, Clone)]
struct Tok {
    kind: Kind,
    pos: usize,
    end: usize,
}

impl Tok {
    /// The token as the JS parser prints it in `Unexpected "..."`.
    fn text(&self) -> String {
        match &self.kind {
            Kind::Num(t) => t
                .parse::<f64>()
                .map_or_else(|_| t.clone(), |v| v.to_string()),
            Kind::Str(s) | Kind::Id(s) => s.clone(),
            Kind::Op(o) => (*o).to_string(),
            Kind::End => "null".to_string(),
        }
    }
}

fn is_id_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_id_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn err(src: &str, message: impl Into<String>, pos: usize) -> ParseError {
    ParseError {
        message: message.into(),
        pos,
        source: src.to_string(),
    }
}

fn tokenize(src: &str) -> Result<Vec<Tok>, ParseError> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
            continue;
        }
        if c == b'"' || c == b'\'' {
            let Some(len) = src[i + 1..].find(c as char) else {
                return Err(err(src, "Unterminated string literal", i));
            };
            let end = i + 1 + len;
            out.push(Tok {
                kind: Kind::Str(src[i + 1..end].to_string()),
                pos: i,
                end: end + 1,
            });
            i = end + 1;
            continue;
        }
        if c.is_ascii_digit() {
            let mut j = i;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j + 1 < b.len() && b[j] == b'.' && b[j + 1].is_ascii_digit() {
                j += 1;
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
            }
            out.push(Tok {
                kind: Kind::Num(src[i..j].to_string()),
                pos: i,
                end: j,
            });
            i = j;
            continue;
        }
        if is_id_start(c) {
            let mut j = i + 1;
            loop {
                while j < b.len() && is_id_char(b[j]) {
                    j += 1;
                }
                if j + 1 < b.len() && b[j] == b'-' && is_id_start(b[j + 1]) {
                    j += 2;
                } else {
                    break;
                }
            }
            out.push(Tok {
                kind: Kind::Id(src[i..j].to_string()),
                pos: i,
                end: j,
            });
            i = j;
            continue;
        }
        let Some(op) = OPS.iter().find(|o| src[i..].starts_with(**o)) else {
            let ch = src[i..].chars().next().unwrap_or('?');
            return Err(err(src, format!("Unexpected character '{ch}'"), i));
        };
        out.push(Tok {
            kind: Kind::Op(op),
            pos: i,
            end: i + op.len(),
        });
        i += op.len();
    }
    out.push(Tok {
        kind: Kind::End,
        pos: src.len(),
        end: src.len(),
    });
    Ok(out)
}

struct Parser<'a> {
    src: &'a str,
    shared: Rc<str>,
    toks: Vec<Tok>,
    p: usize,
    depth: usize,
}

impl Parser<'_> {
    fn peek(&self) -> &Tok {
        &self.toks[self.p]
    }

    fn is_op(&self, o: &str) -> bool {
        matches!(&self.peek().kind, Kind::Op(x) if *x == o)
    }

    fn node(&self, kind: ExprKind, start: usize, end: usize) -> Expr {
        Expr {
            kind,
            span: Span { start, end },
            source: self.shared.clone(),
        }
    }

    fn enter(&mut self) -> Result<(), ParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(err(
                self.src,
                "Expression is nested too deeply",
                self.peek().pos,
            ));
        }
        Ok(())
    }

    fn binary(&mut self, level: usize) -> Result<Expr, ParseError> {
        if level >= LEVELS.len() {
            return self.unary();
        }
        let mut left = self.binary(level + 1)?;
        while let Kind::Op(o) = self.peek().kind {
            if !LEVELS[level].contains(&o) {
                break;
            }
            self.p += 1;
            let op = bin_op(o);
            if op == BinOp::Implies {
                self.enter()?;
                let right = self.binary(level)?;
                self.depth -= 1;
                let (s, e) = (left.span.start, right.span.end);
                return Ok(self.node(ExprKind::Bin(op, Box::new(left), Box::new(right)), s, e));
            }
            let right = self.binary(level + 1)?;
            let (s, e) = (left.span.start, right.span.end);
            left = self.node(ExprKind::Bin(op, Box::new(left), Box::new(right)), s, e);
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, ParseError> {
        let pos = self.peek().pos;
        if let Kind::Op(o @ ("!" | "-")) = self.peek().kind {
            self.p += 1;
            self.enter()?;
            let a = self.unary()?;
            self.depth -= 1;
            let op = if o == "!" { UnOp::Not } else { UnOp::Neg };
            let e = a.span.end;
            return Ok(self.node(ExprKind::Un(op, Box::new(a)), pos, e));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr, ParseError> {
        let tk = self.toks[self.p].clone();
        self.p += 1;
        match &tk.kind {
            Kind::Num(t) => Ok(self.node(ExprKind::Lit(Lit::Num(t.clone())), tk.pos, tk.end)),
            Kind::Str(s) => Ok(self.node(ExprKind::Lit(Lit::Str(s.clone())), tk.pos, tk.end)),
            Kind::Id(name) => {
                let lit = match name.as_str() {
                    "true" => Some(Lit::Bool(true)),
                    "false" => Some(Lit::Bool(false)),
                    "null" => Some(Lit::Null),
                    _ => None,
                };
                if let Some(l) = lit {
                    return Ok(self.node(ExprKind::Lit(l), tk.pos, tk.end));
                }
                let mut path = vec![name.clone()];
                let mut end = tk.end;
                while self.is_op(".") {
                    self.p += 1;
                    let next = self.toks[self.p].clone();
                    self.p += 1;
                    match next.kind {
                        Kind::Id(n) => {
                            path.push(n);
                            end = next.end;
                        }
                        _ => return Err(err(self.src, "Expected a name after \".\"", next.pos)),
                    }
                }
                Ok(self.node(ExprKind::Ref(path), tk.pos, end))
            }
            Kind::Op("(") => {
                self.enter()?;
                let inner = self.binary(0)?;
                self.depth -= 1;
                if !self.is_op(")") {
                    return Err(err(self.src, "Expected \")\"", self.peek().pos));
                }
                self.p += 1;
                Ok(inner)
            }
            Kind::End => Err(err(self.src, "Unexpected end of expression", tk.pos)),
            _ => Err(err(
                self.src,
                format!("Unexpected \"{}\"", tk.text()),
                tk.pos,
            )),
        }
    }
}

fn bin_op(o: &str) -> BinOp {
    match o {
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
        _ => BinOp::Rem,
    }
}

/// Parse an expression exactly as Exegra does.
pub fn parse(src: &str) -> Result<Expr, ParseError> {
    let toks = tokenize(src)?;
    let mut parser = Parser {
        src,
        shared: Rc::from(src),
        toks,
        p: 0,
        depth: 0,
    };
    let ast = parser.binary(0)?;
    let rest = parser.peek();
    if rest.kind != Kind::End {
        return Err(err(
            src,
            format!("Unexpected \"{}\"", rest.text()),
            rest.pos,
        ));
    }
    Ok(ast)
}
