//! The authoring syntax for kernel expressions, as text.
//!
//! ```text
//! len(new.title) <= 120 and new.status in ['draft', 'published']
//! record.status == 'published' or has_role(author)
//! ```
//!
//! `Expr`'s `Display` prints this syntax, so an expression shown in evidence can be pasted back
//! into a tenant definition or the Impact Lab editor.

use strata_kernel::{CmpOp, Expr, RecordId, RoleId, Scope, Value};

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Ident(String),
    Int(i64),
    Str(String),
    Sym(&'static str),
}

fn tokenize(source: &str) -> Result<Vec<Token>, String> {
    const SYMBOLS: [&str; 12] = [
        "==", "!=", "<=", ">=", "<", ">", "(", ")", "[", "]", ",", ".",
    ];
    let chars: Vec<char> = source.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_alphabetic() || c == '_' || c == '$' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '$')
            {
                i += 1;
            }
            tokens.push(Token::Ident(chars[start..i].iter().collect()));
        } else if c.is_ascii_digit()
            || (c == '-' && chars.get(i + 1).is_some_and(char::is_ascii_digit))
        {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            tokens.push(Token::Int(
                text.parse()
                    .map_err(|_| format!("number out of range: {text}"))?,
            ));
        } else if c == '\'' {
            let mut text = String::new();
            i += 1;
            loop {
                match chars.get(i) {
                    None => return Err("unterminated string".into()),
                    Some('\'') => break,
                    Some('\\') => {
                        i += 1;
                        match chars.get(i) {
                            Some('n') => text.push('\n'),
                            Some(c @ ('\'' | '\\')) => text.push(*c),
                            other => return Err(format!("bad escape: {other:?}")),
                        }
                    }
                    Some(c) => text.push(*c),
                }
                i += 1;
            }
            i += 1;
            tokens.push(Token::Str(text));
        } else {
            let rest: String = chars[i..chars.len().min(i + 2)].iter().collect();
            let symbol = SYMBOLS
                .iter()
                .find(|s| rest.starts_with(**s))
                .ok_or_else(|| format!("unexpected character {c:?}"))?;
            tokens.push(Token::Sym(symbol));
            i += symbol.len();
        }
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.at).cloned();
        self.at += 1;
        token
    }

    fn word(&mut self, word: &str) -> bool {
        if matches!(self.peek(), Some(Token::Ident(w)) if w == word) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn sym(&mut self, symbol: &str) -> bool {
        if matches!(self.peek(), Some(Token::Sym(s)) if *s == symbol) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, symbol: &str) -> Result<(), String> {
        if self.sym(symbol) {
            Ok(())
        } else {
            Err(format!("expected {symbol:?}, found {:?}", self.peek()))
        }
    }

    fn ident(&mut self) -> Result<String, String> {
        match self.next() {
            Some(Token::Ident(name)) => Ok(name),
            other => Err(format!("expected a name, found {other:?}")),
        }
    }

    fn or(&mut self) -> Result<Expr, String> {
        let mut items = vec![self.and()?];
        while self.word("or") {
            items.push(self.and()?);
        }
        Ok(if items.len() == 1 {
            items.remove(0)
        } else {
            Expr::Or(items)
        })
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut items = vec![self.not()?];
        while self.word("and") {
            items.push(self.not()?);
        }
        Ok(if items.len() == 1 {
            items.remove(0)
        } else {
            Expr::And(items)
        })
    }

    fn not(&mut self) -> Result<Expr, String> {
        if self.word("not") {
            Ok(Expr::Not(Box::new(self.not()?)))
        } else {
            self.comparison()
        }
    }

    fn comparison(&mut self) -> Result<Expr, String> {
        let left = self.primary()?;
        let ops = [
            ("==", CmpOp::Eq),
            ("!=", CmpOp::Ne),
            ("<=", CmpOp::Le),
            (">=", CmpOp::Ge),
            ("<", CmpOp::Lt),
            (">", CmpOp::Gt),
        ];
        for (symbol, op) in ops {
            if self.sym(symbol) {
                return Ok(Expr::Cmp(op, Box::new(left), Box::new(self.primary()?)));
            }
        }
        if self.word("in") {
            return Ok(Expr::In(Box::new(left), Box::new(self.primary()?)));
        }
        Ok(left)
    }

    fn args(&mut self, count: usize) -> Result<Vec<Expr>, String> {
        self.expect("(")?;
        let mut args = Vec::new();
        for i in 0..count {
            if i > 0 {
                self.expect(",")?;
            }
            args.push(self.or()?);
        }
        self.expect(")")?;
        Ok(args)
    }

    fn int_arg(&mut self) -> Result<i64, String> {
        self.expect("(")?;
        let value = match self.next() {
            Some(Token::Int(i)) => i,
            other => return Err(format!("expected a number, found {other:?}")),
        };
        self.expect(")")?;
        Ok(value)
    }

    fn primary(&mut self) -> Result<Expr, String> {
        let boxed = |mut args: Vec<Expr>| Box::new(args.remove(0));
        match self.next() {
            Some(Token::Int(i)) => Ok(Expr::Lit(Value::Int(i))),
            Some(Token::Str(s)) => Ok(Expr::Lit(Value::Str(s))),
            Some(Token::Sym("(")) => {
                let inner = self.or()?;
                self.expect(")")?;
                Ok(inner)
            }
            Some(Token::Sym("[")) => {
                let mut items = Vec::new();
                while !self.sym("]") {
                    if !items.is_empty() {
                        self.expect(",")?;
                    }
                    match self.primary()? {
                        Expr::Lit(value) => items.push(value),
                        other => return Err(format!("a list holds literals only, found {other}")),
                    }
                }
                Ok(Expr::Lit(Value::List(items)))
            }
            Some(Token::Ident(name)) => match name.as_str() {
                "true" => Ok(Expr::Lit(Value::Bool(true))),
                "false" => Ok(Expr::Lit(Value::Bool(false))),
                "null" => Ok(Expr::Lit(Value::Null)),
                "now" => Ok(Expr::Now),
                "principal" => Ok(Expr::Principal),
                "record" | "old" | "new" => {
                    self.expect(".")?;
                    let scope = match name.as_str() {
                        "record" => Scope::Record,
                        "old" => Scope::Old,
                        _ => Scope::New,
                    };
                    let field = self.ident()?;
                    if self.sym(".") {
                        Ok(Expr::Through(scope, field, self.ident()?))
                    } else {
                        Ok(Expr::Var(scope, field))
                    }
                }
                "param" => {
                    self.expect(".")?;
                    Ok(Expr::Param(self.ident()?))
                }
                "has_role" => {
                    self.expect("(")?;
                    let role = RoleId::new(self.ident()?).map_err(|e| e.to_string())?;
                    self.expect(")")?;
                    Ok(Expr::HasRole(role))
                }
                "time" => Ok(Expr::Lit(Value::Time(self.int_arg()?))),
                "ref" => {
                    let id = u64::try_from(self.int_arg()?)
                        .map_err(|_| "a record id is not negative")?;
                    Ok(Expr::Lit(Value::Ref(RecordId(id))))
                }
                "is_null" => Ok(Expr::IsNull(boxed(self.args(1)?))),
                "len" => Ok(Expr::Len(boxed(self.args(1)?))),
                "lower" => Ok(Expr::Lower(boxed(self.args(1)?))),
                "is_slug" => Ok(Expr::IsSlug(boxed(self.args(1)?))),
                "contains" | "starts_with" => {
                    let mut args = self.args(2)?;
                    let (second, first) = (Box::new(args.remove(1)), Box::new(args.remove(0)));
                    Ok(if name == "contains" {
                        Expr::Contains(first, second)
                    } else {
                        Expr::StartsWith(first, second)
                    })
                }
                "if" => {
                    let mut args = self.args(3)?;
                    let (c, b, a) = (args.remove(2), args.remove(1), args.remove(0));
                    Ok(Expr::If(Box::new(a), Box::new(b), Box::new(c)))
                }
                other => Err(format!("unknown name {other:?}")),
            },
            other => Err(format!("unexpected {other:?}")),
        }
    }
}

/// Parse an expression from its authoring syntax.
pub fn parse(source: &str) -> Result<Expr, String> {
    let mut parser = Parser {
        tokens: tokenize(source)?,
        at: 0,
    };
    let expr = parser.or().map_err(|e| format!("{e} in {source:?}"))?;
    match parser.peek() {
        None => Ok(expr),
        Some(extra) => Err(format!(
            "unexpected {extra:?} after the expression in {source:?}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_precedence_and_calls() {
        let expr =
            parse("len(new.title) <= 120 and not is_null(new.slug) or has_role(editor)").unwrap();
        assert_eq!(
            expr.to_string(),
            "(((len(new.title) <= 120) and not(is_null(new.slug))) or has_role(editor))"
        );
    }

    #[test]
    fn printed_expressions_parse_back_to_themselves() {
        for source in [
            "record.status == 'published'",
            "new.kind in ['a', 'b\\'c', 'd\\\\e']",
            "if(is_null(old.rank), 0, old.rank) < -3",
            "contains(lower(record.title), lower(param.q)) or starts_with(record.cover, '/media/')",
            "record.$created_by == principal and now >= record.published_at",
            "new.when == time(1700000000000) and new.who != ref(7) and new.flag == true and is_null(null)",
            "is_slug(new.slug)",
        ] {
            let parsed = parse(source).unwrap();
            assert_eq!(parse(&parsed.to_string()).unwrap(), parsed, "{source}");
        }
    }

    #[test]
    fn rejects_what_it_does_not_understand() {
        assert!(parse("record.title ==").is_err());
        assert!(parse("drop_table(record.x)").is_err());
        assert!(parse("'open").is_err());
        assert!(parse("record.a == 1 2").is_err());
        assert!(parse("[record.a]").is_err());
    }
}
