//! The expressions of the typed sections of a product file.
//!
//! A value of the platform type `expr` or `expression` (a policy condition, a trigger condition, an
//! operation's check, a field default or read rule, a segment predicate, an automation's expression)
//! is text in the authoring syntax of the tenant rule language (D-6546). [`crate::layer_values`]
//! collects each such text with its place; this module parses it and checks it statically against the
//! scope it is written in, and [`crate::layers`] reports what it finds. Exegra does the same
//! (`src/core/layer-expr.js` there), with the same rule codes:
//!
//! - `layer.expr-syntax`: the text does not parse: a character, a name or an operator the language
//!   does not have (a call such as `balance(x)` is an unknown name), a missing bracket;
//! - `layer.expr-unknown-field`: a record field that the object type of the scope does not have, or a
//!   field a reference hop cannot follow to;
//! - `layer.expr-unknown-role`: `has_role` names a role that neither the platform nor the product
//!   declares;
//! - `layer.expr-unknown-param`: `param.<name>` names a parameter the scope does not bind;
//! - `layer.expr-type`: an operand of a shape the operator does not accept, two shapes that cannot be
//!   compared, branches of different shapes, a hop through a field that is not a single reference.
//!
//! Every message names the product entity, the place of the text in it, the text and the position in
//! the text (a count of characters from 0).
//!
//! Which checker: specc's own, here. The kernel's checker is generated into `strata-model-core`
//! (`tenant_rule_language::check`, wrapped in `strata_kernel::expr`), but this crate must not depend on
//! `strata-model-core`, which specc generates (`Cargo.toml`: it depends on `strata-spec-expr` only);
//! and the checker `language.rs` holds is of the language entity (its operators, tokens and scopes),
//! not of a text of the language. So the grammar and the static check here follow the generated parser
//! and checker one to one (`tokenize`, `Parser`, `check` in `tenant_rule_language.rs`), which the same
//! entity pins, including their messages.
//!
//! The grammar, level by level (D-6543): `or` (n-ary), `and` (n-ary), `not`, one comparison or `in`
//! (non-associative; operands are sums), `+` and `-` (left), `*` (left), then a primary. A `-` before a
//! digit is the sign of an integer literal only where no operand ends before it (after a name that is
//! not `or`, `and`, `not` or `in`, an integer, a string, `)` or `]` it is the binary operator).
//!
//! Drift between this module and the language is a failing test, not a review item:
//! `tests/layer_expr_drift.rs` probes every operator the `tenant-rule-language` entity declares
//! against this parser and checker, and compares both with the generated parser and checker on a
//! corpus. [`print_text`], [`shape_name`] and [`call_names`] exist for it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde_json::{Map, Value};

use crate::diagnostic::Diagnostic;

/// A text that does not parse.
pub const EXPR_SYNTAX: &str = "layer.expr-syntax";
/// A record field the scope does not have.
pub const EXPR_UNKNOWN_FIELD: &str = "layer.expr-unknown-field";
/// A role no layer declares.
pub const EXPR_UNKNOWN_ROLE: &str = "layer.expr-unknown-role";
/// A parameter the scope does not bind.
pub const EXPR_UNKNOWN_PARAM: &str = "layer.expr-unknown-param";
/// A shape the operator does not accept.
pub const EXPR_TYPE: &str = "layer.expr-type";

/// The parameters every event binds (D-6523).
const EVENT_BASE_PARAMS: &[&str] = &["seq", "at", "hop"];

/// The parameters each event adds, and whether it binds a record (D-6523).
const EVENT_PARAMS: &[(&str, &[&str], bool)] = &[
    ("RecordCreated", &[], true),
    ("RecordUpdated", &["transition"], true),
    ("RecordDeleted", &[], true),
    ("SegmentEntered", &["person", "segment", "reason"], false),
    ("SegmentExited", &["person", "segment", "reason"], false),
    (
        "LedgerPosted",
        &[
            "owner",
            "unit",
            "amount",
            "balance",
            "balance_before",
            "entry_sequence",
            "reason",
        ],
        false,
    ),
    ("SettingChanged", &["setting", "level", "subject"], false),
    ("ScheduleDue", &["schedule", "due_at"], false),
    ("InboundWebhook", &["endpoint", "event_id", "body"], false),
];

/// The shape of an expression, as the kernel names them in messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Bool,
    Int,
    Str,
    Time,
    Ref,
    List,
    Any,
}

/// A field of an object type: its shape, and the object type a single reference points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldInfo {
    shape: Shape,
    target: Option<String>,
}

/// The fields of an object type, by name.
pub type FieldMap = BTreeMap<String, FieldInfo>;

/// What the expressions of a product file may name: its object types and its roles, and the roles of
/// the platform.
#[derive(Debug, Clone, Default)]
pub struct ProductContext {
    objects: BTreeMap<String, FieldMap>,
    roles: BTreeSet<String>,
}

/// The scope a text is written in. `None`: not known here, so any field or any param is accepted.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    fields: Option<FieldMap>,
    params: Option<BTreeSet<String>>,
}

/// A text of a typed value, and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprUse {
    /// Where in the item, such as `conditions.0.expr`.
    pub at: String,
    /// The text.
    pub text: String,
}

/// The fields of an object type value, with the shape of each.
pub fn field_map_of(object_type: &Value) -> FieldMap {
    let mut out = FieldMap::new();
    let Some(fields) = object_type.get("fields").and_then(Value::as_object) else {
        return out;
    };
    for (name, def) in fields {
        let ty = def.get("ty");
        let tag = match ty {
            Some(Value::String(tag)) => tag.as_str(),
            Some(Value::Object(map)) => map.keys().next().map_or("", String::as_str),
            _ => "",
        };
        let shape = match tag {
            "Str" | "File" | "Language" => Shape::Str,
            "Int" => Shape::Int,
            "Bool" => Shape::Bool,
            "Time" => Shape::Time,
            "Ref" => Shape::Ref,
            "List" => Shape::List,
            _ => Shape::Any,
        };
        let target = if tag == "Ref" {
            ty.and_then(|t| t.get("Ref"))
                .and_then(|r| r.get("target"))
                .and_then(Value::as_str)
                .map(str::to_string)
        } else {
            None
        };
        out.insert(name.clone(), FieldInfo { shape, target });
    }
    out
}

impl ProductContext {
    /// The context of one product file: `platform_ids` is every id the platform declares with its
    /// section (its roles are the ones with the section `roles`), `top` the sections of the file.
    pub fn of(platform_ids: &BTreeMap<String, String>, top: &Map<String, Value>) -> Self {
        let mut ctx = ProductContext::default();
        for (id, section) in platform_ids {
            if section == "roles" {
                ctx.roles.insert(id.clone());
            }
        }
        for item in top
            .get("objects")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(id) = item.get("id").and_then(Value::as_str) {
                ctx.objects.insert(id.to_string(), field_map_of(item));
            }
        }
        for item in top
            .get("roles")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(id) = item.get("id").and_then(Value::as_str) {
                ctx.roles.insert(id.to_string());
            }
        }
        ctx
    }

    /// The scope a text is written in, found from the item that holds it. Objects: the fields of the
    /// object type itself. Policies: the object type of the resource. Segments: the subject type.
    /// Triggers: the object of a record event (none for the others) and the params the event binds.
    /// Operations: the subject's object type and the inputs as params. Anything else (an automation, a
    /// token): the run's params are not known here, so any field and param is accepted.
    pub fn scope_of(&self, section: &str, item: &Value) -> Scope {
        let fields_of = |id: Option<&str>| id.and_then(|id| self.objects.get(id)).cloned();
        let no_params = Some(BTreeSet::new());
        match section {
            "objects" => Scope {
                fields: Some(fields_of(item.get("id").and_then(Value::as_str)).unwrap_or_default()),
                params: no_params,
            },
            "policies" => Scope {
                fields: fields_of(
                    item.get("resource")
                        .and_then(|r| r.get("Object"))
                        .and_then(Value::as_str),
                ),
                params: no_params,
            },
            "segments" => Scope {
                fields: fields_of(item.get("subject-type").and_then(Value::as_str)),
                params: no_params,
            },
            "triggers" => {
                let on = item.get("on");
                let event = match on {
                    Some(Value::String(event)) => Some(event.as_str()),
                    Some(Value::Object(map)) => map.keys().next().map(String::as_str),
                    _ => None,
                };
                let Some((_, adds, record)) = EVENT_PARAMS
                    .iter()
                    .find(|(name, _, _)| Some(*name) == event)
                else {
                    return Scope::default();
                };
                let params: BTreeSet<String> = EVENT_BASE_PARAMS
                    .iter()
                    .chain(adds.iter())
                    .map(|p| p.to_string())
                    .collect();
                let fields = if *record {
                    fields_of(
                        on.and_then(|o| o.get(event.unwrap_or("")))
                            .and_then(|o| o.get("object"))
                            .and_then(Value::as_str),
                    )
                } else {
                    Some(FieldMap::new())
                };
                Scope {
                    fields,
                    params: Some(params),
                }
            }
            "operations" => {
                let params: BTreeSet<String> = item
                    .get("inputs")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|i| i.get("name").and_then(Value::as_str))
                    .map(str::to_string)
                    // A later step reads the id of a record an earlier step created as a parameter
                    // named by that step (D-6548). The scope is the operation's, so every creating
                    // step's name is bound here; the kernel refuses a read before the step that
                    // creates it when the operation is defined.
                    .chain(
                        item.get("steps")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(|step| step.get("CreateRecord"))
                            .filter_map(|create| create.get("id").and_then(Value::as_str))
                            .map(str::to_string),
                    )
                    .collect();
                let fields = match item
                    .get("subject")
                    .and_then(|s| s.get("object"))
                    .and_then(Value::as_str)
                {
                    Some(object) => fields_of(Some(object)),
                    None => Some(FieldMap::new()),
                };
                Scope {
                    fields,
                    params: Some(params),
                }
            }
            // A query reads the fields of its object type and the parameters it declares; a workflow
            // reads the fields of its object type and no parameter (D-6700).
            "queries" => Scope {
                fields: fields_of(item.get("object").and_then(Value::as_str)),
                params: Some(
                    item.get("params")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect(),
                ),
            },
            "workflows" => Scope {
                fields: fields_of(item.get("object").and_then(Value::as_str)),
                params: no_params,
            },
            _ => Scope::default(),
        }
    }
}

/// The first problem of a text.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Problem {
    code: &'static str,
    pos: usize,
    message: String,
}

impl Problem {
    fn new(code: &'static str, pos: usize, message: impl Into<String>) -> Self {
        Self {
            code,
            pos,
            message: message.into(),
        }
    }
}

impl fmt::Display for Shape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Shape::Bool => "Bool",
            Shape::Int => "Int",
            Shape::Str => "Str",
            Shape::Time => "Time",
            Shape::Ref => "Ref",
            Shape::List => "List",
            Shape::Any => "Any",
        };
        f.write_str(name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Id(String),
    Int(String),
    Str(String),
    Sym(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Token {
    kind: Kind,
    pos: usize,
}

/// Whether a token ends an operand, so that a `-` after it is the binary operator rather than the sign
/// of a literal.
fn ends_operand(token: Option<&Token>) -> bool {
    const WORD_OPERATORS: [&str; 4] = ["or", "and", "not", "in"];
    match token.map(|t| &t.kind) {
        Some(Kind::Int(_) | Kind::Str(_)) => true,
        Some(Kind::Sym(s)) => *s == ")" || *s == "]",
        Some(Kind::Id(w)) => !WORD_OPERATORS.contains(&w.as_str()),
        None => false,
    }
}

fn tokenize(source: &str) -> Result<Vec<Token>, Problem> {
    const SYMBOLS: [&str; 15] = [
        "==", "!=", "<=", ">=", "<", ">", "+", "-", "*", "(", ")", "[", "]", ",", ".",
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
            tokens.push(Token {
                kind: Kind::Id(chars[start..i].iter().collect()),
                pos: start,
            });
        } else if c.is_ascii_digit()
            || (c == '-'
                && chars.get(i + 1).is_some_and(char::is_ascii_digit)
                && !ends_operand(tokens.last()))
        {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            if text.parse::<i64>().is_err() {
                return Err(Problem::new(
                    EXPR_SYNTAX,
                    start,
                    format!("number out of range: {text}"),
                ));
            }
            tokens.push(Token {
                kind: Kind::Int(text),
                pos: start,
            });
        } else if c == '\'' {
            let start = i;
            let mut text = String::new();
            i += 1;
            loop {
                match chars.get(i) {
                    None => {
                        return Err(Problem::new(EXPR_SYNTAX, start, "unterminated string"));
                    }
                    Some('\'') => break,
                    Some('\\') => {
                        i += 1;
                        match chars.get(i) {
                            Some('n') => text.push('\n'),
                            Some(c @ ('\'' | '\\')) => text.push(*c),
                            other => {
                                return Err(Problem::new(
                                    EXPR_SYNTAX,
                                    i - 1,
                                    format!("bad escape: {other:?}"),
                                ));
                            }
                        }
                    }
                    Some(c) => text.push(*c),
                }
                i += 1;
            }
            i += 1;
            tokens.push(Token {
                kind: Kind::Str(text),
                pos: start,
            });
        } else {
            let rest: String = chars[i..chars.len().min(i + 2)].iter().collect();
            let Some(symbol) = SYMBOLS.iter().find(|s| rest.starts_with(**s)) else {
                return Err(Problem::new(
                    EXPR_SYNTAX,
                    i,
                    format!("unexpected character {c:?}"),
                ));
            };
            tokens.push(Token {
                kind: Kind::Sym(symbol),
                pos: i,
            });
            i += symbol.chars().count();
        }
    }
    Ok(tokens)
}

/// The calls of the language and their argument counts.
const CALLS: &[(&str, usize)] = &[
    ("is_null", 1),
    ("len", 1),
    ("lower", 1),
    ("contains", 2),
    ("starts_with", 2),
    ("is_slug", 1),
    ("if", 3),
    ("ms_between", 2),
    ("days", 1),
    ("hours", 1),
    ("minutes", 1),
    ("concat", 2),
    ("text", 1),
];

/// The names of the calls this module parses (the `call` operators of the language it follows).
pub fn call_names() -> Vec<&'static str> {
    CALLS.iter().map(|(name, _)| *name).collect()
}

/// A literal, as the generated AST holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Lit {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Time(i64),
    Ref(u64),
    List(Vec<Lit>),
}

impl Lit {
    fn shape(&self) -> Shape {
        match self {
            Lit::Null => Shape::Any,
            Lit::Bool(_) => Shape::Bool,
            Lit::Int(_) => Shape::Int,
            Lit::Str(_) => Shape::Str,
            Lit::Time(_) => Shape::Time,
            Lit::Ref(_) => Shape::Ref,
            Lit::List(_) => Shape::List,
        }
    }
}

/// The authoring syntax, as the generated printer writes it.
impl fmt::Display for Lit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Lit::Null => f.write_str("null"),
            Lit::Bool(x) => write!(f, "{x}"),
            Lit::Int(x) => write!(f, "{x}"),
            Lit::Str(s) => {
                f.write_str("'")?;
                for c in s.chars() {
                    match c {
                        '\'' => f.write_str("\\'")?,
                        '\\' => f.write_str("\\\\")?,
                        '\n' => f.write_str("\\n")?,
                        c => write!(f, "{c}")?,
                    }
                }
                f.write_str("'")
            }
            Lit::Time(x) => write!(f, "time({x})"),
            Lit::Ref(x) => write!(f, "ref({x})"),
            Lit::List(items) => {
                f.write_str("[")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{item}")?;
                }
                f.write_str("]")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NodeKind {
    Lit(Lit),
    Var {
        scope: &'static str,
        name: String,
        name_pos: usize,
    },
    Through {
        scope: &'static str,
        field: String,
        field_pos: usize,
        name: String,
        name_pos: usize,
    },
    Param {
        name: String,
        name_pos: usize,
    },
    Now,
    Principal,
    HasRole {
        role: String,
        role_pos: usize,
    },
    Not(Box<Node>),
    /// `and` and `or`: every item is a boolean.
    Junction {
        word: &'static str,
        items: Vec<Node>,
    },
    Cmp {
        op: &'static str,
        left: Box<Node>,
        right: Box<Node>,
    },
    /// `+`, `-` or `*`.
    Arith {
        op: &'static str,
        left: Box<Node>,
        right: Box<Node>,
    },
    In(Box<Node>, Box<Node>),
    Call(String, Vec<Node>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Node {
    pos: usize,
    kind: NodeKind,
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
    end: usize,
}

impl Parser {
    fn here(&self) -> usize {
        self.tokens.get(self.at).map_or(self.end, |t| t.pos)
    }

    fn describe(token: Option<&Token>) -> String {
        match token.map(|t| &t.kind) {
            None => "the end of the text".to_string(),
            Some(Kind::Id(w)) => format!("\"{w}\""),
            Some(Kind::Int(n)) => format!("\"{n}\""),
            Some(Kind::Str(_)) => "a string".to_string(),
            Some(Kind::Sym(s)) => format!("\"{s}\""),
        }
    }

    fn word(&mut self, word: &str) -> bool {
        if matches!(self.tokens.get(self.at), Some(Token { kind: Kind::Id(w), .. }) if w == word) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn sym(&mut self, symbol: &str) -> bool {
        if matches!(self.tokens.get(self.at), Some(Token { kind: Kind::Sym(s), .. }) if *s == symbol)
        {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, symbol: &str) -> Result<(), Problem> {
        if self.sym(symbol) {
            Ok(())
        } else {
            Err(Problem::new(
                EXPR_SYNTAX,
                self.here(),
                format!(
                    "expected \"{symbol}\", found {}",
                    Self::describe(self.tokens.get(self.at))
                ),
            ))
        }
    }

    /// A name and its position.
    fn ident(&mut self) -> Result<(String, usize), Problem> {
        match self.tokens.get(self.at) {
            Some(Token {
                kind: Kind::Id(name),
                pos,
            }) => {
                let found = (name.clone(), *pos);
                self.at += 1;
                Ok(found)
            }
            other => Err(Problem::new(
                EXPR_SYNTAX,
                self.here(),
                format!("expected a name, found {}", Self::describe(other)),
            )),
        }
    }

    /// `(` an integer `)`, the integer as written.
    fn int_arg(&mut self) -> Result<String, Problem> {
        self.expect("(")?;
        let value = match self.tokens.get(self.at) {
            Some(Token {
                kind: Kind::Int(n), ..
            }) => n.clone(),
            other => {
                return Err(Problem::new(
                    EXPR_SYNTAX,
                    self.here(),
                    format!("expected a number, found {}", Self::describe(other)),
                ));
            }
        };
        self.at += 1;
        self.expect(")")?;
        Ok(value)
    }

    fn args(&mut self, n: usize) -> Result<Vec<Node>, Problem> {
        self.expect("(")?;
        let mut out = Vec::new();
        for i in 0..n {
            if i > 0 {
                self.expect(",")?;
            }
            out.push(self.level_0()?);
        }
        self.expect(")")?;
        Ok(out)
    }

    /// Precedence 1: `or`.
    fn level_0(&mut self) -> Result<Node, Problem> {
        let pos = self.here();
        let mut items = vec![self.level_1()?];
        while self.word("or") {
            items.push(self.level_1()?);
        }
        Ok(if items.len() == 1 {
            items.remove(0)
        } else {
            Node {
                pos,
                kind: NodeKind::Junction { word: "or", items },
            }
        })
    }

    /// Precedence 2: `and`.
    fn level_1(&mut self) -> Result<Node, Problem> {
        let pos = self.here();
        let mut items = vec![self.level_2()?];
        while self.word("and") {
            items.push(self.level_2()?);
        }
        Ok(if items.len() == 1 {
            items.remove(0)
        } else {
            Node {
                pos,
                kind: NodeKind::Junction { word: "and", items },
            }
        })
    }

    /// Precedence 3: `not`.
    fn level_2(&mut self) -> Result<Node, Problem> {
        let pos = self.here();
        if self.word("not") {
            let operand = self.level_2()?;
            return Ok(Node {
                pos,
                kind: NodeKind::Not(Box::new(operand)),
            });
        }
        self.level_3()
    }

    /// Precedence 4: a comparison or `in`.
    fn level_3(&mut self) -> Result<Node, Problem> {
        let pos = self.here();
        let left = self.level_4()?;
        for op in ["==", "!=", "<", "<=", ">", ">="] {
            if self.sym(op) {
                let right = self.level_4()?;
                return Ok(Node {
                    pos,
                    kind: NodeKind::Cmp {
                        op,
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                });
            }
        }
        if self.word("in") {
            let right = self.level_4()?;
            return Ok(Node {
                pos,
                kind: NodeKind::In(Box::new(left), Box::new(right)),
            });
        }
        Ok(left)
    }

    /// Precedence 5: `+` and `-`, left associative.
    fn level_4(&mut self) -> Result<Node, Problem> {
        let pos = self.here();
        let mut left = self.level_5()?;
        loop {
            let op = if self.sym("+") {
                "+"
            } else if self.sym("-") {
                "-"
            } else {
                return Ok(left);
            };
            let right = self.level_5()?;
            left = Node {
                pos,
                kind: NodeKind::Arith {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
            };
        }
    }

    /// Precedence 6: `*`, left associative.
    fn level_5(&mut self) -> Result<Node, Problem> {
        let pos = self.here();
        let mut left = self.primary()?;
        while self.sym("*") {
            let right = self.primary()?;
            left = Node {
                pos,
                kind: NodeKind::Arith {
                    op: "*",
                    left: Box::new(left),
                    right: Box::new(right),
                },
            };
        }
        Ok(left)
    }

    fn primary(&mut self) -> Result<Node, Problem> {
        let pos = self.here();
        let Some(token) = self.tokens.get(self.at).cloned() else {
            return Err(Problem::new(EXPR_SYNTAX, pos, "unexpected end of the text"));
        };
        self.at += 1;
        let lit = |value: Lit| -> Result<Node, Problem> {
            Ok(Node {
                pos,
                kind: NodeKind::Lit(value),
            })
        };
        match token.kind {
            Kind::Int(text) => lit(Lit::Int(int_value(&text, pos)?)),
            Kind::Str(text) => lit(Lit::Str(text)),
            Kind::Sym("(") => {
                let inner = self.level_0()?;
                self.expect(")")?;
                Ok(inner)
            }
            Kind::Sym("[") => {
                let mut items = Vec::new();
                while !self.sym("]") {
                    if !items.is_empty() {
                        self.expect(",")?;
                    }
                    let item = self.primary()?;
                    let at = item.pos;
                    match item.kind {
                        NodeKind::Lit(value) => items.push(value),
                        _ => {
                            return Err(Problem::new(
                                EXPR_SYNTAX,
                                at,
                                "a list holds literals only",
                            ));
                        }
                    }
                }
                lit(Lit::List(items))
            }
            Kind::Sym(other) => Err(Problem::new(
                EXPR_SYNTAX,
                pos,
                format!("unexpected \"{other}\""),
            )),
            Kind::Id(name) => match name.as_str() {
                "true" => lit(Lit::Bool(true)),
                "false" => lit(Lit::Bool(false)),
                "null" => lit(Lit::Null),
                "param" => {
                    self.expect(".")?;
                    let (name, name_pos) = self.ident()?;
                    Ok(Node {
                        pos,
                        kind: NodeKind::Param { name, name_pos },
                    })
                }
                "now" => Ok(Node {
                    pos,
                    kind: NodeKind::Now,
                }),
                "principal" => Ok(Node {
                    pos,
                    kind: NodeKind::Principal,
                }),
                "has_role" => {
                    self.expect("(")?;
                    let (role, role_pos) = self.ident()?;
                    self.expect(")")?;
                    Ok(Node {
                        pos,
                        kind: NodeKind::HasRole { role, role_pos },
                    })
                }
                "record" | "old" | "new" => {
                    let scope = match name.as_str() {
                        "record" => "record",
                        "old" => "old",
                        _ => "new",
                    };
                    self.expect(".")?;
                    let (field, field_pos) = self.ident()?;
                    if self.sym(".") {
                        let (name, name_pos) = self.ident()?;
                        Ok(Node {
                            pos,
                            kind: NodeKind::Through {
                                scope,
                                field,
                                field_pos,
                                name,
                                name_pos,
                            },
                        })
                    } else {
                        Ok(Node {
                            pos,
                            kind: NodeKind::Var {
                                scope,
                                name: field,
                                name_pos: field_pos,
                            },
                        })
                    }
                }
                "time" => {
                    let value = self.int_arg()?;
                    lit(Lit::Time(int_value(&value, pos)?))
                }
                "ref" => {
                    let value = self.int_arg()?;
                    if value.starts_with('-') {
                        return Err(Problem::new(
                            EXPR_SYNTAX,
                            pos,
                            "a record id is not negative",
                        ));
                    }
                    let id = value.parse::<u64>().map_err(|_| {
                        Problem::new(EXPR_SYNTAX, pos, "a record id is not negative")
                    })?;
                    lit(Lit::Ref(id))
                }
                call => match CALLS.iter().find(|(c, _)| *c == call) {
                    Some((_, count)) => {
                        let args = self.args(*count)?;
                        Ok(Node {
                            pos,
                            kind: NodeKind::Call(name.clone(), args),
                        })
                    }
                    None => Err(Problem::new(
                        EXPR_SYNTAX,
                        pos,
                        format!(
                            "unknown name \"{call}\": the tenant rule language has no such name or call (its calls are {}, and has_role, param, now, principal)",
                            CALLS.iter().map(|(c, _)| *c).collect::<Vec<_>>().join(", ")
                        ),
                    )),
                },
            },
        }
    }
}

/// The value of an integer token (the tokenizer has checked its range).
fn int_value(text: &str, pos: usize) -> Result<i64, Problem> {
    text.parse::<i64>()
        .map_err(|_| Problem::new(EXPR_SYNTAX, pos, format!("number out of range: {text}")))
}

fn parse(source: &str) -> Result<Node, Problem> {
    let mut parser = Parser {
        tokens: tokenize(source)?,
        at: 0,
        end: source.chars().count(),
    };
    let tree = parser.level_0()?;
    if parser.at < parser.tokens.len() {
        return Err(Problem::new(
            EXPR_SYNTAX,
            parser.tokens[parser.at].pos,
            format!(
                "unexpected {} after the expression",
                Parser::describe(parser.tokens.get(parser.at))
            ),
        ));
    }
    Ok(tree)
}

/// What a check reads: the scope and what the product declares.
struct Env<'a> {
    scope: &'a Scope,
    ctx: &'a ProductContext,
}

/// Whether `name` is a field identifier: a lower-case letter, then lower-case letters, digits and `_`,
/// at most 64 bytes.
fn is_field_id(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn system_attribute(name: &str) -> Option<Shape> {
    match name {
        "$id" => Some(Shape::Ref),
        "$version" => Some(Shape::Int),
        "$created_by" => Some(Shape::Str),
        "$created_at" | "$updated_at" => Some(Shape::Time),
        _ => None,
    }
}

fn expect_shape(shape: Shape, allowed: &[Shape], what: &str, pos: usize) -> Result<(), Problem> {
    if shape == Shape::Any || allowed.contains(&shape) {
        Ok(())
    } else {
        Err(Problem::new(
            EXPR_TYPE,
            pos,
            format!("{what} does not accept a {shape} operand"),
        ))
    }
}

fn field_id(name: &str, pos: usize) -> Result<(), Problem> {
    if is_field_id(name) {
        Ok(())
    } else {
        Err(Problem::new(
            EXPR_UNKNOWN_FIELD,
            pos,
            format!("invalid field identifier: {name:?}"),
        ))
    }
}

fn unknown_field(name: &str, fields: &FieldMap, pos: usize) -> Problem {
    let have = if fields.is_empty() {
        "no fields in this scope".to_string()
    } else {
        format!(
            "the fields {}",
            fields.keys().cloned().collect::<Vec<_>>().join(", ")
        )
    };
    Problem::new(
        EXPR_UNKNOWN_FIELD,
        pos,
        format!("unknown field {name}; the object type has {have}"),
    )
}

/// The shape of a name read from `fields` (`None`: any field is accepted).
fn attribute(fields: Option<&FieldMap>, name: &str, pos: usize) -> Result<Shape, Problem> {
    if let Some(shape) = system_attribute(name) {
        return Ok(shape);
    }
    field_id(name, pos)?;
    match fields {
        None => Ok(Shape::Any),
        Some(fields) => fields
            .get(name)
            .map(|f| f.shape)
            .ok_or_else(|| unknown_field(name, fields, pos)),
    }
}

fn shape_of(node: &Node, env: &Env<'_>) -> Result<Shape, Problem> {
    match &node.kind {
        NodeKind::Lit(value) => Ok(value.shape()),
        NodeKind::Var { name, name_pos, .. } => {
            attribute(env.scope.fields.as_ref(), name, *name_pos)
        }
        NodeKind::Through {
            field,
            field_pos,
            name,
            name_pos,
            ..
        } => {
            field_id(field, *field_pos)?;
            let target = match &env.scope.fields {
                None => None,
                Some(fields) => {
                    let info = fields
                        .get(field)
                        .ok_or_else(|| unknown_field(field, fields, *field_pos))?;
                    let Some(object) = info.target.as_ref().filter(|_| info.shape == Shape::Ref)
                    else {
                        return Err(Problem::new(
                            EXPR_TYPE,
                            *field_pos,
                            format!("{field} is not a single reference"),
                        ));
                    };
                    env.ctx.objects.get(object)
                }
            };
            attribute(target, name, *name_pos)
        }
        NodeKind::Param { name, name_pos } => {
            if let Some(params) = &env.scope.params {
                if !params.contains(name) {
                    let bound = if params.is_empty() {
                        "no parameters".to_string()
                    } else {
                        params.iter().cloned().collect::<Vec<_>>().join(", ")
                    };
                    return Err(Problem::new(
                        EXPR_UNKNOWN_PARAM,
                        *name_pos,
                        format!("undeclared parameter {name}; the scope binds {bound}"),
                    ));
                }
            }
            Ok(Shape::Any)
        }
        NodeKind::Now => Ok(Shape::Time),
        NodeKind::Principal => Ok(Shape::Str),
        NodeKind::HasRole { role, role_pos } => {
            if env.ctx.roles.contains(role) {
                Ok(Shape::Bool)
            } else {
                Err(Problem::new(
                    EXPR_UNKNOWN_ROLE,
                    *role_pos,
                    format!(
                        "unknown role {role}; neither the platform nor the product declares it"
                    ),
                ))
            }
        }
        NodeKind::Not(operand) => {
            expect_shape(shape_of(operand, env)?, &[Shape::Bool], "not", operand.pos)?;
            Ok(Shape::Bool)
        }
        NodeKind::Junction { items, .. } => {
            for item in items {
                expect_shape(shape_of(item, env)?, &[Shape::Bool], "and/or", item.pos)?;
            }
            Ok(Shape::Bool)
        }
        NodeKind::Cmp { op, left, right } => {
            let (l, r) = (shape_of(left, env)?, shape_of(right, env)?);
            if l != Shape::Any && r != Shape::Any && l != r {
                return Err(Problem::new(
                    EXPR_TYPE,
                    node.pos,
                    format!("cannot compare {l} with {r}"),
                ));
            }
            if *op != "==" && *op != "!=" {
                let orderable = [Shape::Int, Shape::Time, Shape::Str];
                expect_shape(l, &orderable, "ordering", left.pos)?;
                expect_shape(r, &orderable, "ordering", right.pos)?;
            }
            Ok(Shape::Bool)
        }
        NodeKind::In(left, right) => {
            shape_of(left, env)?;
            expect_shape(shape_of(right, env)?, &[Shape::List], "in", right.pos)?;
            Ok(Shape::Bool)
        }
        NodeKind::Arith { op, left, right } => {
            let first = shape_of(left, env)?;
            if *op == "*" {
                expect_shape(first, &[Shape::Int], op, left.pos)?;
                expect_shape(shape_of(right, env)?, &[Shape::Int], op, right.pos)?;
                Ok(Shape::Int)
            } else {
                expect_shape(first, &[Shape::Int, Shape::Time], op, left.pos)?;
                expect_shape(shape_of(right, env)?, &[Shape::Int], op, right.pos)?;
                Ok(first)
            }
        }
        NodeKind::Call(name, args) => call_shape(node.pos, name, args, env),
    }
}

fn call_shape(pos: usize, name: &str, args: &[Node], env: &Env<'_>) -> Result<Shape, Problem> {
    match name {
        "is_null" => {
            shape_of(&args[0], env)?;
            Ok(Shape::Bool)
        }
        "len" => {
            expect_shape(
                shape_of(&args[0], env)?,
                &[Shape::Str, Shape::List],
                "len",
                args[0].pos,
            )?;
            Ok(Shape::Int)
        }
        "lower" => {
            expect_shape(
                shape_of(&args[0], env)?,
                &[Shape::Str],
                "lower",
                args[0].pos,
            )?;
            Ok(Shape::Str)
        }
        "contains" => {
            let (first, second) = (shape_of(&args[0], env)?, shape_of(&args[1], env)?);
            expect_shape(first, &[Shape::Str, Shape::List], "contains", args[0].pos)?;
            if first == Shape::Str {
                expect_shape(second, &[Shape::Str], "contains", args[1].pos)?;
            }
            Ok(Shape::Bool)
        }
        "starts_with" => {
            expect_shape(
                shape_of(&args[0], env)?,
                &[Shape::Str],
                "starts_with",
                args[0].pos,
            )?;
            expect_shape(
                shape_of(&args[1], env)?,
                &[Shape::Str],
                "starts_with",
                args[1].pos,
            )?;
            Ok(Shape::Bool)
        }
        "is_slug" => {
            expect_shape(
                shape_of(&args[0], env)?,
                &[Shape::Str],
                "is_slug",
                args[0].pos,
            )?;
            Ok(Shape::Bool)
        }
        "ms_between" => {
            for arg in args {
                expect_shape(shape_of(arg, env)?, &[Shape::Time], "ms_between", arg.pos)?;
            }
            Ok(Shape::Int)
        }
        "days" | "hours" | "minutes" => {
            expect_shape(shape_of(&args[0], env)?, &[Shape::Int], name, args[0].pos)?;
            Ok(Shape::Int)
        }
        "concat" => {
            for arg in args {
                expect_shape(shape_of(arg, env)?, &[Shape::Str], "concat", arg.pos)?;
            }
            Ok(Shape::Str)
        }
        "text" => {
            expect_shape(
                shape_of(&args[0], env)?,
                &[Shape::Int, Shape::Time, Shape::Ref, Shape::Bool, Shape::Str],
                "text",
                args[0].pos,
            )?;
            Ok(Shape::Str)
        }
        _ => {
            expect_shape(shape_of(&args[0], env)?, &[Shape::Bool], "if", args[0].pos)?;
            let (then, otherwise) = (shape_of(&args[1], env)?, shape_of(&args[2], env)?);
            if then != Shape::Any && otherwise != Shape::Any && then != otherwise {
                return Err(Problem::new(
                    EXPR_TYPE,
                    pos,
                    format!("if branches differ: {then} and {otherwise}"),
                ));
            }
            Ok(if then == Shape::Any { otherwise } else { then })
        }
    }
}

/// The text as the generated printer (`Display` of the generated AST) prints what it parsed, or the
/// message of the syntax problem. Parentheses are explicit: `(a + (b * c))`.
pub fn print_text(source: &str) -> Result<String, String> {
    parse(source)
        .map(|tree| print(&tree))
        .map_err(|problem| problem.message)
}

/// The shape the static check gives a text (`Bool`, `Int`, ... as the generated checker names them),
/// or the code and message of the first problem.
pub fn shape_name(
    source: &str,
    scope: &Scope,
    ctx: &ProductContext,
) -> Result<String, (&'static str, String)> {
    let env = Env { scope, ctx };
    parse(source)
        .and_then(|tree| shape_of(&tree, &env))
        .map(|shape| shape.to_string())
        .map_err(|problem| (problem.code, problem.message))
}

fn print(node: &Node) -> String {
    let all = |nodes: &[Node]| nodes.iter().map(print).collect::<Vec<_>>();
    match &node.kind {
        NodeKind::Lit(value) => value.to_string(),
        NodeKind::Var { scope, name, .. } => format!("{scope}.{name}"),
        NodeKind::Through {
            scope, field, name, ..
        } => format!("{scope}.{field}.{name}"),
        NodeKind::Param { name, .. } => format!("param.{name}"),
        NodeKind::Now => "now".to_string(),
        NodeKind::Principal => "principal".to_string(),
        NodeKind::HasRole { role, .. } => format!("has_role({role})"),
        NodeKind::Not(operand) => format!("not({})", print(operand)),
        NodeKind::Junction { word, items } => {
            format!("({})", all(items).join(&format!(" {word} ")))
        }
        NodeKind::Cmp { op, left, right } | NodeKind::Arith { op, left, right } => {
            format!("({} {op} {})", print(left), print(right))
        }
        NodeKind::In(item, list) => format!("({} in {})", print(item), print(list)),
        NodeKind::Call(name, args) => format!("{name}({})", all(args).join(", ")),
    }
}

/// Parses and checks one text. Returns the first problem as a diagnostic, or `None` when the text
/// parses and checks. `entity` is the product entity that holds the text, `at` where in it.
pub fn check_text(
    file: &str,
    entity: &str,
    at: &str,
    text: &str,
    scope: &Scope,
    ctx: &ProductContext,
) -> Option<Diagnostic> {
    let env = Env { scope, ctx };
    let problem = parse(text).and_then(|tree| shape_of(&tree, &env)).err()?;
    let place = if at.is_empty() {
        "its value".to_string()
    } else {
        format!("\"{at}\"")
    };
    Some(Diagnostic::new(
        problem.code,
        entity,
        format!(
            "\"{entity}\" in {file}: the expression at {place}, `{text}`, is refused at position {}: {}. An expression in a product is text in the tenant rule language, checked against the object type, params and roles of the place it is written in; correct the text.",
            problem.pos, problem.message
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tier() -> Value {
        json!({ "id": "p__tier", "fields": {
            "name": { "ty": "Str" },
            "points": { "ty": "Int" },
            "owner": { "ty": { "Ref": { "target": "p__member", "on-delete": "Restrict" } } },
            "tags": { "ty": { "List": "Str" } } } })
    }

    fn context() -> ProductContext {
        let top = json!({
            "objects": [ tier(), { "id": "p__member", "fields": { "email": { "ty": "Str" } } } ],
            "roles": [ { "id": "p__boss" } ] });
        let platform: BTreeMap<String, String> =
            [("member".to_string(), "roles".to_string())].into();
        ProductContext::of(&platform, top.as_object().unwrap())
    }

    /// The code and position of the first problem of `text` in the scope of the tier.
    fn problem(text: &str) -> Option<(String, String)> {
        let ctx = context();
        let scope = ctx.scope_of("objects", &tier());
        check_text(
            "p.jsonc",
            "p__tier",
            "constraints.c.expr",
            text,
            &scope,
            &ctx,
        )
        .map(|d| (d.code, d.message))
    }

    fn code(text: &str) -> Option<String> {
        problem(text).map(|(code, _)| code)
    }

    #[test]
    fn texts_of_the_language_are_accepted() {
        for ok in [
            "record.name == 'gold' and record.points >= 10",
            "not record.points < 3 or true",
            "record.owner.email == 'a'",
            "record.$id == ref(3)",
            "now > time(5)",
            "principal == record.$created_by",
            "has_role(member) or has_role(p__boss)",
            "contains(record.tags, 'x')",
            "record.name in ['a', 'b']",
            "if(record.points > 1, 1, 2) == 1",
            "is_null(record.owner)",
            "starts_with(lower(record.name), 'g')",
            "len(record.tags) > 0",
            "record.points == null",
            "(true)",
        ] {
            assert_eq!(problem(ok), None, "{ok}");
        }
    }

    #[test]
    fn a_name_the_language_lacks_and_malformed_text_is_a_syntax_error_with_its_position() {
        assert_eq!(
            code("balance(record.points) > 1").as_deref(),
            Some(EXPR_SYNTAX)
        );
        assert!(problem("balance(record.points) > 1")
            .unwrap()
            .1
            .contains("position 0: unknown name \"balance\""));
        assert!(problem("record.points +")
            .unwrap()
            .1
            .contains("position 15"));
        assert!(problem("record.name == \"gold\"")
            .unwrap()
            .1
            .contains("position 15"));
        assert!(problem("record.name == 'gold")
            .unwrap()
            .1
            .contains("position 15"));
        assert!(problem("record.points >")
            .unwrap()
            .1
            .contains("position 15"));
        assert!(problem("(record.points > 1")
            .unwrap()
            .1
            .contains("position 18"));
        assert!(problem("record.points > 1 2")
            .unwrap()
            .1
            .contains("position 18"));
        assert!(problem("[record.points]")
            .unwrap()
            .1
            .contains("position 1:"));
        assert!(problem("len(record.name, 1)")
            .unwrap()
            .1
            .contains("position 15"));
        for text in ["record.points +", "(record.points > 1", "[record.points]"] {
            assert_eq!(code(text).as_deref(), Some(EXPR_SYNTAX), "{text}");
        }
    }

    #[test]
    fn fields_hops_params_and_roles_are_checked_against_the_scope() {
        let (c, m) = problem("record.colour == 1").unwrap();
        assert_eq!(c, EXPR_UNKNOWN_FIELD);
        assert!(
            m.contains(
                "unknown field colour; the object type has the fields name, owner, points, tags"
            ),
            "{m}"
        );
        assert_eq!(
            code("record.owner.phone == null").as_deref(),
            Some(EXPR_UNKNOWN_FIELD)
        );
        assert_eq!(
            code("record.nothing.email == null").as_deref(),
            Some(EXPR_UNKNOWN_FIELD)
        );
        assert_eq!(
            code("record.Name == null").as_deref(),
            Some(EXPR_UNKNOWN_FIELD)
        );
        assert_eq!(code("param.other > 1").as_deref(), Some(EXPR_UNKNOWN_PARAM));
        assert_eq!(code("has_role(ghost)").as_deref(), Some(EXPR_UNKNOWN_ROLE));
        assert_eq!(code("record.name.email == 1").as_deref(), Some(EXPR_TYPE));
        assert_eq!(code("record.tags.email == 1").as_deref(), Some(EXPR_TYPE));
    }

    #[test]
    fn shapes_the_operator_does_not_accept_are_type_errors() {
        for bad in [
            "record.name == 1",
            "record.name > 1",
            "len(record.points) > 0",
            "not record.points",
            "record.points and true",
            "lower(record.points) == 'a'",
            "record.points in record.points",
            "if(record.points, 1, 2) == 1",
            "if(true, 1, 'a') == 1",
            "contains(record.name, 1)",
            "starts_with(1, 'a')",
        ] {
            assert_eq!(code(bad).as_deref(), Some(EXPR_TYPE), "{bad}");
        }
        assert!(problem("record.name == record.points")
            .unwrap()
            .1
            .contains("cannot compare Str with Int"));
    }

    #[test]
    fn checked_arithmetic_time_arithmetic_durations_concat_and_text_are_accepted() {
        for ok in [
            "record.points + 1 == 3",
            "record.points - 1 > 0",
            "record.points * 2 + 1 == 3",
            "(record.points + 1) * 2 >= 4",
            "now + days(1) > now",
            "now - hours(2) < now",
            "now + minutes(30) <= time(5)",
            "ms_between(time(1), now) >= minutes(5)",
            "ms_between(now, now + days(90)) <= days(90)",
            "concat('a', text(record.points)) == 'a1'",
            "concat(lower(record.name), 'x') == 'x'",
            "len(concat(record.name, 'x')) > 1",
            "text(now) == 'x'",
            "text(record.owner) == 'x'",
            "text(record.$id) == 'x'",
            "text(true) == 'x'",
            "text('s') == 's'",
            "-1 + 2 == 1",
            "1 in [-1, 2]",
            "time(-5) < now",
            "days(-1) < 0",
        ] {
            assert_eq!(problem(ok), None, "{ok}");
        }
        // A parameter has no shape the check knows (Any): the result follows the other operand.
        for (text, shape) in [
            ("param.a + 1", "Any"),
            ("param.a * 2", "Int"),
            ("now + param.n", "Time"),
            ("text(param.x)", "Str"),
            ("ms_between(param.a, now)", "Int"),
        ] {
            assert_eq!(
                shape_name(text, &Scope::default(), &ProductContext::default()),
                Ok(shape.to_string()),
                "{text}"
            );
        }
    }

    #[test]
    fn a_minus_is_a_sign_only_where_no_operand_ends() {
        let printed = |text: &str| print_text(text).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(printed("record.a-1"), "(record.a - 1)");
        assert_eq!(printed("record.a -1"), "(record.a - 1)");
        assert_eq!(printed("record.a - -1"), "(record.a - -1)");
        assert_eq!(printed("1 -1"), "(1 - 1)");
        assert_eq!(printed("(1) -1"), "(1 - 1)");
        assert_eq!(printed("[1] in [1]"), "([1] in [1])");
        assert_eq!(printed("-1"), "-1");
        assert_eq!(printed("2*-3"), "(2 * -3)");
        assert_eq!(printed("time(-5)"), "time(-5)");
        assert_eq!(printed(r"'a\'b'"), r"'a\'b'");
    }

    #[test]
    fn a_minus_that_is_not_a_sign_and_has_no_left_operand_is_a_syntax_error() {
        for text in ["- 1", "--1", "not -1 == 1 -", "[1 -1]", "1 - - 1"] {
            let (found, _) =
                shape_name(text, &Scope::default(), &ProductContext::default()).expect_err(text);
            assert_eq!(found, EXPR_SYNTAX, "{text}");
        }
        // After `not` a `-1` is a literal, so `not` gets an integer.
        assert_eq!(code("not -1").as_deref(), Some(EXPR_TYPE));
        assert!(problem("[1 -1]").unwrap().1.contains("position 3"));
    }

    #[test]
    fn arithmetic_binds_tighter_than_comparison_and_multiplication_tighter_than_addition() {
        for (text, printed) in [
            ("1 + 2 * 3", "(1 + (2 * 3))"),
            ("1 * 2 + 3", "((1 * 2) + 3)"),
            ("1 - 2 - 3", "((1 - 2) - 3)"),
            ("1 * 2 * 3", "((1 * 2) * 3)"),
            ("(1 + 2) * 3", "((1 + 2) * 3)"),
            ("1 + 2 == 3", "((1 + 2) == 3)"),
            ("1 == 2 + 3", "(1 == (2 + 3))"),
            ("1 in 2 + 3", "(1 in (2 + 3))"),
            ("not 1 < 2", "not((1 < 2))"),
            (
                "1 + 2 == 3 and 4 < 5 * 6 or true",
                "((((1 + 2) == 3) and (4 < (5 * 6))) or true)",
            ),
        ] {
            assert_eq!(print_text(text), Ok(printed.to_string()), "{text}");
        }
        // A comparison is not associative.
        assert!(print_text("1 < 2 < 3")
            .unwrap_err()
            .contains("after the expression"));
        assert!(print_text("1 == 2 in [3]").is_err());
        assert!(print_text("1 + 2 +").is_err());
    }

    // Verifies: CMS-1545
    #[test]
    fn an_operand_the_new_operators_do_not_accept_is_a_type_error() {
        for (bad, message) in [
            ("now + now > now", "+ does not accept a Time operand"),
            ("'a' + 1 == 1", "+ does not accept a Str operand"),
            (
                "record.points + 'a' == 1",
                "+ does not accept a Str operand",
            ),
            ("true - 1 == 1", "- does not accept a Bool operand"),
            ("now - now > now", "- does not accept a Time operand"),
            (
                "record.points * now == 1",
                "* does not accept a Time operand",
            ),
            ("now * 2 == 1", "* does not accept a Time operand"),
            (
                "ms_between(1, now) > 0",
                "ms_between does not accept a Int operand",
            ),
            (
                "ms_between(now, 'a') > 0",
                "ms_between does not accept a Str operand",
            ),
            ("days('a') > 0", "days does not accept a Str operand"),
            ("hours(now) > 0", "hours does not accept a Time operand"),
            (
                "minutes(true) > 0",
                "minutes does not accept a Bool operand",
            ),
            (
                "concat('a', 1) == 'a'",
                "concat does not accept a Int operand",
            ),
            (
                "concat(1, 'a') == 'a'",
                "concat does not accept a Int operand",
            ),
            (
                "text(record.tags) == 'x'",
                "text does not accept a List operand",
            ),
            ("now + 1 > 5", "cannot compare Time with Int"),
            ("record.points + 1 == 'a'", "cannot compare Int with Str"),
        ] {
            let (found, text) = problem(bad).unwrap_or_else(|| panic!("{bad} was accepted"));
            assert_eq!(found, EXPR_TYPE, "{bad}");
            assert!(text.contains(message), "{bad}: {text}");
        }
    }

    #[test]
    fn the_scope_comes_from_the_value_that_holds_the_text() {
        let ctx = context();
        let names = |scope: &Scope| -> Vec<String> {
            scope
                .fields
                .as_ref()
                .map(|f| f.keys().cloned().collect())
                .unwrap_or_default()
        };
        assert_eq!(
            names(&ctx.scope_of("objects", &tier())),
            ["name", "owner", "points", "tags"]
        );
        assert_eq!(
            names(&ctx.scope_of(
                "policies",
                &json!({ "resource": { "Object": "p__member" } })
            )),
            ["email"]
        );
        assert!(ctx
            .scope_of("policies", &json!({ "resource": "AnyObject" }))
            .fields
            .is_none());
        assert_eq!(
            names(&ctx.scope_of("segments", &json!({ "subject-type": "p__member" }))),
            ["email"]
        );
        let created = ctx.scope_of(
            "triggers",
            &json!({ "on": { "RecordCreated": { "object": "p__tier" } } }),
        );
        assert!(created
            .fields
            .as_ref()
            .is_some_and(|f| f.contains_key("points")));
        assert_eq!(
            created.params.unwrap().into_iter().collect::<Vec<_>>(),
            ["at", "hop", "seq"]
        );
        let posted = ctx.scope_of(
            "triggers",
            &json!({ "on": { "LedgerPosted": { "unit": "p__u" } } }),
        );
        assert_eq!(posted.fields.as_ref().map(BTreeMap::len), Some(0));
        assert!(posted.params.unwrap().contains("balance_before"));
        let op = ctx.scope_of(
            "operations",
            &json!({ "inputs": [ { "name": "tier" } ], "subject": { "object": "p__tier", "input": "tier" } }),
        );
        assert!(op.fields.as_ref().is_some_and(|f| f.contains_key("points")));
        assert_eq!(op.params.unwrap().into_iter().collect::<Vec<_>>(), ["tier"]);
        assert_eq!(
            ctx.scope_of("operations", &json!({ "inputs": [] }))
                .fields
                .as_ref()
                .map(BTreeMap::len),
            Some(0)
        );
        let free = ctx.scope_of("automations", &json!({}));
        assert!(free.fields.is_none() && free.params.is_none());
    }
}
