//! The blocks of a mail template or a notice template (EML-012, NTF-1570, CTK-015, D-7900): the
//! schema its body has, how a block places a token or a value, and the one function that renders
//! a body for a reader.
//!
//! The body of a template is `structured-blocks`: JSON text holding a list of blocks. The blocks
//! are those that `strata_substrate::markdown` produces from constrained Markdown (`Block` and
//! `Inline` there), so a body written as Markdown and parsed is a valid body, with two more inline
//! nodes that only a template has:
//!
//! * `{"type":"token","name":"<token>"}` places a content token (CTK-015): the name of a token the
//!   tenant defines, replaced when the body is rendered by the text the token yields for the reader.
//! * `{"type":"value","name":"<value>","value_type":"str"|"int"|"bool"|"time"}` places a value the
//!   sender of the message supplies (the `values` of send-notice): its name and its type are
//!   declared where it is placed, and a name is placed with one type only.
//!
//! Every other node is a Markdown node: blocks `heading` (level 2 to 4), `paragraph`, `quote`,
//! `list`, `code`, `figure` and `rule`; inlines `text`, `em`, `strong`, `code` and `link`. A link
//! carries a web or mail target (CMS-250) or a path or fragment of the site. Quotes nest at most
//! [`MAX_QUOTE_DEPTH`] deep and inline elements at most [`MAX_INLINE_DEPTH`], the bounds of the
//! Markdown parser (a test in the substrate crate keeps them equal). A node with a member this
//! schema does not name is refused, so a body means one thing.
//!
//! [`analyse`] checks a body and reports the tokens and the typed values it places; the checks of
//! a schema (a token exists) are made by [`crate::templates`]. [`render`] is the pure function the
//! mail dispatcher and the notice service call: blocks, values and the resolution of each token
//! in, the rendered blocks and their plain text out.

use crate::limits;
use crate::types::Value;
use serde_json::{Map, Value as Json};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// The most quotes inside quotes, as the Markdown parser keeps them.
pub const MAX_QUOTE_DEPTH: usize = 8;
/// The most inline elements inside inline elements, as the Markdown parser keeps them.
pub const MAX_INLINE_DEPTH: usize = 8;
/// The most nodes (blocks and inlines) one body holds.
pub const MAX_NODES: usize = 5_000;
/// The most problems [`analyse`] reports; a body with more is refused all the same.
pub const MAX_PROBLEMS: usize = 20;

/// The type a template declares for a value it places (`template-value-type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ValueType {
    Str,
    Int,
    Bool,
    Time,
}

impl ValueType {
    /// The type a body writes as `str`, `int`, `bool` or `time`.
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "str" => Self::Str,
            "int" => Self::Int,
            "bool" => Self::Bool,
            "time" => Self::Time,
            _ => return None,
        })
    }

    /// The type as a body writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Str => "str",
            Self::Int => "int",
            Self::Bool => "bool",
            Self::Time => "time",
        }
    }

    fn holds(self, value: &Value) -> bool {
        matches!(
            (self, value),
            (Self::Str, Value::Str(_))
                | (Self::Int, Value::Int(_))
                | (Self::Bool, Value::Bool(_))
                | (Self::Time, Value::Time(_))
        )
    }
}

/// What [`analyse`] found in a body.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Analysis {
    /// Why the body is not a list of blocks of this schema; empty when it is.
    pub problems: Vec<String>,
    /// The names of the tokens the body places.
    pub tokens: BTreeSet<String>,
    /// The values the body places, each with its declared type.
    pub values: BTreeMap<String, ValueType>,
}

/// Whether `name` is the name of a token or of a value: lower-case letters, digits and
/// underscores, beginning with a letter, at most 64 bytes (the pattern of a token name).
pub fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 64
        && matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Whether a link target is one a body may carry (CMS-250): a web (`http`, `https`) or mail
/// (`mailto`) address, a path of the site or a fragment, with no space or control character.
pub fn allowed_href(href: &str) -> bool {
    let lower = href.to_ascii_lowercase();
    let scheme = (href.starts_with('/') && !href.starts_with("//"))
        || href.starts_with('#')
        || lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:");
    scheme && !href.chars().any(|c| c.is_control() || c.is_whitespace())
}

fn allowed_media_path(src: &str) -> bool {
    src.starts_with("/media/")
        && !src.contains("..")
        && src
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.'))
}

struct Walk {
    out: Analysis,
    nodes: usize,
    over: bool,
}

impl Walk {
    fn problem(&mut self, at: &str, text: impl fmt::Display) {
        if self.out.problems.len() < MAX_PROBLEMS {
            self.out.problems.push(format!("at {at}: {text}"));
        }
    }

    /// Count one node; false once the body holds more than [`MAX_NODES`].
    fn count(&mut self, at: &str) -> bool {
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            if !self.over {
                self.over = true;
                self.problem(at, format!("the body holds more than {MAX_NODES} nodes"));
            }
            return false;
        }
        true
    }

    /// The members of `map` outside `allowed` are refused.
    fn only(&mut self, map: &Map<String, Json>, allowed: &[&str], at: &str) {
        for key in map.keys() {
            if !allowed.contains(&key.as_str()) {
                self.problem(
                    at,
                    format!("has the member {key:?}, which its type does not"),
                );
            }
        }
    }

    fn string<'a>(&mut self, map: &'a Map<String, Json>, key: &str, at: &str) -> Option<&'a str> {
        match map.get(key) {
            Some(Json::String(text)) => Some(text),
            Some(_) => {
                self.problem(at, format!("{key} is not text"));
                None
            }
            None => {
                self.problem(at, format!("has no {key}"));
                None
            }
        }
    }

    fn list<'a>(
        &mut self,
        map: &'a Map<String, Json>,
        key: &str,
        at: &str,
    ) -> Option<&'a Vec<Json>> {
        match map.get(key) {
            Some(Json::Array(list)) => Some(list),
            Some(_) => {
                self.problem(at, format!("{key} is not a list"));
                None
            }
            None => {
                self.problem(at, format!("has no {key}"));
                None
            }
        }
    }

    fn blocks(&mut self, list: &[Json], at: &str, quote_depth: usize) {
        for (i, node) in list.iter().enumerate() {
            if !self.count(at) {
                return;
            }
            self.block(node, &format!("{at}[{i}]"), quote_depth);
        }
    }

    fn block(&mut self, node: &Json, at: &str, quote_depth: usize) {
        let Some(map) = node.as_object() else {
            return self.problem(at, "is not an object");
        };
        let Some(kind) = map.get("type").and_then(Json::as_str) else {
            return self.problem(at, "has no type");
        };
        match kind {
            "heading" => {
                self.only(map, &["type", "level", "inlines"], at);
                if !matches!(map.get("level").and_then(Json::as_u64), Some(2..=4)) {
                    self.problem(at, "the level of a heading is 2, 3 or 4");
                }
                if let Some(list) = self.list(map, "inlines", at) {
                    self.inlines(list, &format!("{at}.inlines"), 0);
                }
            }
            "paragraph" => {
                self.only(map, &["type", "inlines"], at);
                if let Some(list) = self.list(map, "inlines", at) {
                    self.inlines(list, &format!("{at}.inlines"), 0);
                }
            }
            "quote" => {
                self.only(map, &["type", "blocks"], at);
                if quote_depth >= MAX_QUOTE_DEPTH {
                    self.problem(at, format!("quotes nest more than {MAX_QUOTE_DEPTH} deep"));
                } else if let Some(list) = self.list(map, "blocks", at) {
                    self.blocks(list, &format!("{at}.blocks"), quote_depth + 1);
                }
            }
            "list" => {
                self.only(map, &["type", "ordered", "items"], at);
                if !matches!(map.get("ordered"), Some(Json::Bool(_))) {
                    self.problem(at, "ordered is not true or false");
                }
                if let Some(items) = self.list(map, "items", at) {
                    for (i, item) in items.iter().enumerate() {
                        let here = format!("{at}.items[{i}]");
                        match item {
                            Json::Array(list) => self.inlines(list, &here, 0),
                            _ => self.problem(&here, "an item is not a list of inlines"),
                        }
                    }
                }
            }
            "code" => {
                self.only(map, &["type", "lang", "text"], at);
                if let Some(lang) = self.string(map, "lang", at) {
                    if !lang.chars().all(|c| c.is_ascii_alphanumeric()) {
                        self.problem(at, "lang has a character that is not a letter or a digit");
                    }
                }
                self.string(map, "text", at);
            }
            "figure" => {
                self.only(map, &["type", "src", "alt", "caption"], at);
                if let Some(src) = self.string(map, "src", at) {
                    if !allowed_media_path(src) {
                        self.problem(at, "src is not a path under /media/");
                    }
                }
                self.string(map, "alt", at);
                self.string(map, "caption", at);
            }
            "rule" => self.only(map, &["type"], at),
            other => self.problem(at, format!("{other:?} is not a block type")),
        }
    }

    fn inlines(&mut self, list: &[Json], at: &str, depth: usize) {
        for (i, node) in list.iter().enumerate() {
            if !self.count(at) {
                return;
            }
            self.inline(node, &format!("{at}[{i}]"), depth);
        }
    }

    fn inline(&mut self, node: &Json, at: &str, depth: usize) {
        let Some(map) = node.as_object() else {
            return self.problem(at, "is not an object");
        };
        let Some(kind) = map.get("type").and_then(Json::as_str) else {
            return self.problem(at, "has no type");
        };
        match kind {
            "text" | "code" => {
                self.only(map, &["type", "text"], at);
                self.string(map, "text", at);
            }
            "em" | "strong" | "link" => {
                if kind == "link" {
                    self.only(map, &["type", "href", "children"], at);
                    if let Some(href) = self.string(map, "href", at) {
                        if !allowed_href(href) {
                            self.problem(
                                at,
                                "a link carries a web (http, https) or mail (mailto) address, a path of the site or a fragment",
                            );
                        }
                    }
                } else {
                    self.only(map, &["type", "children"], at);
                }
                if depth >= MAX_INLINE_DEPTH {
                    self.problem(
                        at,
                        format!("inline elements nest more than {MAX_INLINE_DEPTH} deep"),
                    );
                } else if let Some(list) = self.list(map, "children", at) {
                    self.inlines(list, &format!("{at}.children"), depth + 1);
                }
            }
            "token" => {
                self.only(map, &["type", "name"], at);
                if let Some(name) = self.string(map, "name", at) {
                    if is_name(name) {
                        self.out.tokens.insert(name.to_string());
                    } else {
                        self.problem(
                            at,
                            "the name of a token is lower-case letters, digits and underscores, beginning with a letter",
                        );
                    }
                }
            }
            "value" => {
                self.only(map, &["type", "name", "value_type"], at);
                let name = self.string(map, "name", at);
                let ty = self.string(map, "value_type", at);
                let ty = ty.and_then(|text| {
                    let parsed = ValueType::parse(text);
                    if parsed.is_none() {
                        self.problem(at, "value_type is str, int, bool or time");
                    }
                    parsed
                });
                if let Some(name) = name {
                    if !is_name(name) {
                        self.problem(
                            at,
                            "the name of a value is lower-case letters, digits and underscores, beginning with a letter",
                        );
                    } else if let Some(ty) = ty {
                        match self.out.values.insert(name.to_string(), ty) {
                            Some(first) if first != ty => self.problem(
                                at,
                                format!(
                                    "the value {name} is placed as {} here and as {} before",
                                    ty.as_str(),
                                    first.as_str()
                                ),
                            ),
                            _ => {}
                        }
                    }
                }
            }
            other => self.problem(at, format!("{other:?} is not an inline type")),
        }
    }
}

/// Check the body of a template: JSON text holding a list of blocks of the schema above. The
/// tokens it places and the values it places with their types are reported whatever else is
/// wrong with it.
pub fn analyse(body: &str) -> Analysis {
    let mut walk = Walk {
        out: Analysis::default(),
        nodes: 0,
        over: false,
    };
    if body.len() > limits::MAX_STRING_BYTES {
        walk.out
            .problems
            .push(format!("is longer than {} bytes", limits::MAX_STRING_BYTES));
        return walk.out;
    }
    match serde_json::from_str::<Json>(body) {
        Err(e) => walk.out.problems.push(format!("is not JSON text: {e}")),
        Ok(Json::Array(list)) => walk.blocks(&list, "", 0),
        Ok(_) => walk
            .out
            .problems
            .push("is not a list of blocks".to_string()),
    }
    walk.out
}

/// A body rendered for one reader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rendered {
    /// The blocks with each token and value replaced by a text node (CTK-015): a list of blocks
    /// of the schema above that places no token and no value.
    pub blocks: Json,
    /// The same blocks as plain text (the plain part of a message, a notice): blocks separated by
    /// one blank line, a quote prefixed with `> `, a list item with `- ` or its number, a link
    /// followed by its address in parentheses, a rule as `---`.
    pub text: String,
}

/// Why a body was not rendered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderError {
    /// The body is not a list of blocks of the schema; the problems of [`analyse`].
    Invalid(Vec<String>),
    /// A value the body places was not supplied.
    MissingValue(String),
    /// A value was supplied with another type than the body declares.
    WrongType { name: String, declared: ValueType },
    /// A token the body places was not resolved.
    UnresolvedToken(String),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(problems) => write!(f, "the body is not blocks: {}", problems.join("; ")),
            Self::MissingValue(name) => write!(f, "the value {name} was not supplied"),
            Self::WrongType { name, declared } => {
                write!(
                    f,
                    "the value {name} is not of its declared type {}",
                    declared.as_str()
                )
            }
            Self::UnresolvedToken(name) => write!(f, "the token {name} was not resolved"),
        }
    }
}

impl std::error::Error for RenderError {}

/// Render a body for a reader: pure and deterministic. `values` are the values the sender
/// supplies, each of the type the body declares; `token` gives the text of a token for the reader
/// (its fallback when the expression yields none, CTK-011, which is the caller's to decide) or
/// `None` for a token it cannot resolve. A time is written as RFC 3339 in UTC, an integer in
/// decimal, a boolean as `true` or `false`.
pub fn render(
    body: &str,
    values: &BTreeMap<String, Value>,
    token: &dyn Fn(&str) -> Option<String>,
) -> Result<Rendered, RenderError> {
    let analysis = analyse(body);
    if !analysis.problems.is_empty() {
        return Err(RenderError::Invalid(analysis.problems));
    }
    for (name, declared) in &analysis.values {
        match values.get(name) {
            None => return Err(RenderError::MissingValue(name.clone())),
            Some(value) if !declared.holds(value) => {
                return Err(RenderError::WrongType {
                    name: name.clone(),
                    declared: *declared,
                })
            }
            Some(_) => {}
        }
    }
    let Ok(Json::Array(list)) = serde_json::from_str::<Json>(body) else {
        return Err(RenderError::Invalid(vec!["is not a list of blocks".into()]));
    };
    let cx = Cx { values, token };
    let blocks = cx.blocks(&list)?;
    let text = blocks
        .iter()
        .map(block_text)
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok(Rendered {
        blocks: Json::Array(blocks),
        text,
    })
}

struct Cx<'a> {
    values: &'a BTreeMap<String, Value>,
    token: &'a dyn Fn(&str) -> Option<String>,
}

fn text_node(text: String) -> Json {
    let mut node = Map::new();
    node.insert("type".into(), Json::String("text".into()));
    node.insert("text".into(), Json::String(text));
    Json::Object(node)
}

impl Cx<'_> {
    fn blocks(&self, list: &[Json]) -> Result<Vec<Json>, RenderError> {
        list.iter().map(|block| self.block(block)).collect()
    }

    fn block(&self, node: &Json) -> Result<Json, RenderError> {
        let mut map = node.as_object().cloned().unwrap_or_default();
        if let Some(Json::Array(list)) = map.get("inlines") {
            let rendered = self.inlines(list)?;
            map.insert("inlines".into(), Json::Array(rendered));
        }
        if let Some(Json::Array(list)) = map.get("blocks") {
            let rendered = self.blocks(list)?;
            map.insert("blocks".into(), Json::Array(rendered));
        }
        if let Some(Json::Array(items)) = map.get("items") {
            let mut rendered = Vec::new();
            for item in items {
                let list = item.as_array().map(Vec::as_slice).unwrap_or_default();
                rendered.push(Json::Array(self.inlines(list)?));
            }
            map.insert("items".into(), Json::Array(rendered));
        }
        Ok(Json::Object(map))
    }

    fn inlines(&self, list: &[Json]) -> Result<Vec<Json>, RenderError> {
        list.iter().map(|node| self.inline(node)).collect()
    }

    fn inline(&self, node: &Json) -> Result<Json, RenderError> {
        let mut map = node.as_object().cloned().unwrap_or_default();
        let name = map
            .get("name")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string();
        let kind = map
            .get("type")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string();
        let name = name.as_str();
        Ok(match kind.as_str() {
            "token" => text_node(
                (self.token)(name).ok_or_else(|| RenderError::UnresolvedToken(name.to_string()))?,
            ),
            "value" => text_node(match self.values.get(name) {
                Some(Value::Str(text)) => text.clone(),
                Some(Value::Int(n)) => n.to_string(),
                Some(Value::Bool(b)) => b.to_string(),
                Some(Value::Time(ms)) => rfc3339(*ms),
                _ => return Err(RenderError::MissingValue(name.to_string())),
            }),
            "em" | "strong" | "link" => {
                if let Some(Json::Array(children)) = map.get("children") {
                    let rendered = self.inlines(children)?;
                    map.insert("children".into(), Json::Array(rendered));
                }
                Json::Object(map)
            }
            _ => node.clone(),
        })
    }
}

fn inline_text(list: &[Json]) -> String {
    list.iter()
        .map(|node| {
            let kind = node.get("type").and_then(Json::as_str);
            let own = node.get("text").and_then(Json::as_str).unwrap_or_default();
            let children = node
                .get("children")
                .and_then(Json::as_array)
                .map(|c| inline_text(c))
                .unwrap_or_default();
            match kind {
                Some("link") => {
                    let href = node.get("href").and_then(Json::as_str).unwrap_or_default();
                    if href == children || href.starts_with('#') {
                        children
                    } else {
                        format!("{children} ({href})")
                    }
                }
                Some("em" | "strong") => children,
                _ => own.to_string(),
            }
        })
        .collect()
}

fn block_text(block: &Json) -> String {
    let inlines = |key: &str| {
        block
            .get(key)
            .and_then(Json::as_array)
            .map(|list| inline_text(list))
            .unwrap_or_default()
    };
    let field = |key: &str| {
        block
            .get(key)
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string()
    };
    match block.get("type").and_then(Json::as_str) {
        Some("heading" | "paragraph") => inlines("inlines"),
        Some("quote") => block
            .get("blocks")
            .and_then(Json::as_array)
            .map(|inner| {
                inner
                    .iter()
                    .map(block_text)
                    .collect::<Vec<_>>()
                    .join("\n\n")
                    .lines()
                    .map(|line| {
                        if line.is_empty() {
                            ">".to_string()
                        } else {
                            format!("> {line}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default(),
        Some("list") => {
            let ordered = block.get("ordered") == Some(&Json::Bool(true));
            block
                .get("items")
                .and_then(Json::as_array)
                .map(|items| {
                    items
                        .iter()
                        .enumerate()
                        .map(|(i, item)| {
                            let text = item.as_array().map(|l| inline_text(l)).unwrap_or_default();
                            if ordered {
                                format!("{}. {text}", i + 1)
                            } else {
                                format!("- {text}")
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default()
        }
        Some("code") => field("text"),
        Some("figure") => {
            let (alt, caption) = (field("alt"), field("caption"));
            if caption.is_empty() {
                alt
            } else {
                format!("{alt}\n{caption}")
            }
        }
        Some("rule") => "---".to_string(),
        _ => String::new(),
    }
}

/// A time in milliseconds since the epoch as RFC 3339 in UTC, to the second.
fn rfc3339(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    // Days since 1970-01-01 to a civil date (the algorithm of H. Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 {
        shifted + 3
    } else {
        shifted - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}
