//! The one place where specification text becomes code text.
//!
//! Every name, description, requirement ref and string value of a specification that specc
//! writes into generated Rust or JavaScript goes through this module, so that no input can end a
//! comment early, start a new line of code, close a literal, or name a reserved word:
//!
//! - **Comments** ([`rust::line_comments`], [`rust::doc_comment`], [`js::line_comments`],
//!   [`js::comment_text`], [`js::doc_text`]): every line terminator of either language (`\n`,
//!   `\r`, U+0085, U+2028, U+2029, vertical tab, form feed) ends the line being written and
//!   starts a new comment line; other control characters and the bidirectional controls that
//!   rustc and editors flag are written as `<U+XXXX>`; a block comment's `*/` becomes `* /`;
//!   in a JSDoc block a `@` becomes `(at)`, so text cannot start a tag.
//! - **String literals** ([`rust::string`], [`rust::json`], [`js::string`], [`js::double_quoted`]):
//!   quotes, backslashes, control characters, line separators and bidirectional controls are
//!   escaped; a `*/` inside a JavaScript literal is written `*\/`, so a literal that ends up
//!   inside a JSDoc type cannot close the comment.
//! - **Identifiers** ([`rust::ident`], [`js::ident`], [`js::camel`]): characters that cannot
//!   appear in an identifier become `_`, a leading digit gets a `_` in front, and a reserved word
//!   is written as a raw identifier (Rust, where one exists) or with a trailing `_`.
//! - **Collisions** ([`Claims`]): two sources that map to one identifier, or a source that maps
//!   to a name the generated file already uses, are the diagnostic [`IDENTIFIER_COLLISION`].

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::diagnostic::Diagnostic;

/// Code of two specification names that become one generated identifier, or of a name that
/// becomes one the generated file already uses.
pub const IDENTIFIER_COLLISION: &str = "specc.identifier-collision";

/// True for a character that ends a line in Rust, JavaScript or an editor.
fn is_line_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// True for the bidirectional controls that can make code read differently from how it runs.
fn is_bidi(c: char) -> bool {
    matches!(
        c,
        '\u{61c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

/// The text with no line break, control character or bidirectional control in it: a break
/// becomes a space and anything else that is not safe becomes `<U+XXXX>`.
fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if is_line_break(c) || c == '\t' {
            out.push(' ');
        } else if c.is_control() || is_bidi(c) {
            out.push_str(&format!("<U+{:04X}>", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

/// The text on one line, safe in a `//`, `///` or `#` comment of any generated language: line
/// breaks become spaces, other control and bidirectional characters `<U+XXXX>`.
pub fn line_text(text: &str) -> String {
    one_line(text)
}

/// The lines of a text, split at every line terminator (`\r\n` is one) and made safe for a
/// comment. A text with no lines (empty) gives one empty line.
pub fn lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if is_line_break(c) {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push(one_line(&std::mem::take(&mut current)));
        } else {
            current.push(c);
        }
    }
    out.push(one_line(&current));
    out
}

/// One comment line per line of `text`, each `{indent}{marker} {line}\n` (`{marker}` alone for an
/// empty line).
fn comment_lines(indent: &str, marker: &str, text: &str, fix: fn(&str) -> String) -> String {
    let mut out = String::new();
    for line in lines(text) {
        let line = fix(&line);
        if line.trim().is_empty() {
            out.push_str(&format!("{indent}{marker}\n"));
        } else {
            out.push_str(&format!("{indent}{marker} {line}\n"));
        }
    }
    out
}

fn same(line: &str) -> String {
    line.to_string()
}

// ---------------------------------------------------------------------------------------------

/// Names claimed in one generated file: each identifier belongs to one specification name, and
/// the file's own names are reserved.
pub struct Claims {
    file: String,
    reserved: BTreeSet<String>,
    taken: BTreeMap<String, String>,
}

impl Claims {
    /// A claim register for the generated file `file`, whose own top-level names are `reserved`.
    pub fn new(file: &str, reserved: &[&str]) -> Self {
        Self {
            file: file.to_string(),
            reserved: reserved.iter().map(|r| (*r).to_string()).collect(),
            taken: BTreeMap::new(),
        }
    }

    /// Claims `ident` for the specification name `source`. Claiming one name twice for the same
    /// source is fine; a second source, or a name the file uses itself, is a diagnostic.
    pub fn claim(&mut self, source: &str, ident: &str) -> Result<(), Diagnostic> {
        if self.reserved.contains(ident) {
            return Err(Diagnostic::new(
                IDENTIFIER_COLLISION,
                source,
                format!(
                    "\"{source}\" becomes the identifier `{ident}` in {}, which the file uses itself; rename it.",
                    self.file
                ),
            ));
        }
        match self.taken.get(ident) {
            Some(other) if other != source => Err(Diagnostic::new(
                IDENTIFIER_COLLISION,
                source,
                format!(
                    "\"{source}\" and \"{other}\" both become the identifier `{ident}` in {}; rename one.",
                    self.file
                ),
            )),
            Some(_) => Ok(()),
            None => {
                self.taken.insert(ident.to_string(), source.to_string());
                Ok(())
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------

/// Text written into generated Rust.
pub mod rust {
    use proc_macro2::{Delimiter, Group, Literal, TokenStream, TokenTree};

    use super::*;

    /// Rust keywords that can be raw identifiers.
    const KEYWORDS: &[&str] = &[
        "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "do",
        "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl", "in",
        "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
        "return", "static", "struct", "trait", "true", "try", "type", "typeof", "unsafe",
        "unsized", "use", "virtual", "where", "while", "yield",
    ];

    /// Keywords that cannot be raw identifiers.
    const UNRAWABLE: &[&str] = &["self", "Self", "super", "crate", "_"];

    /// Line comments (`// `), one per line of `text`, each ended by `\n`. No input ends a
    /// comment line early or starts a line of code.
    pub fn line_comments(indent: &str, text: &str) -> String {
        comment_lines(indent, "//", text, same)
    }

    /// Doc comment lines (`/// `), one per line of `text`, each ended by `\n`.
    pub fn doc_comment(indent: &str, text: &str) -> String {
        comment_lines(indent, "///", text, same)
    }

    /// A Rust string literal for `text`.
    pub fn string(text: &str) -> String {
        let mut out = String::with_capacity(text.len() + 2);
        out.push('"');
        for c in text.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if c.is_control() || is_line_break(c) || is_bidi(c) => {
                    out.push_str(&format!("\\u{{{:x}}}", c as u32));
                }
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    /// The arguments of a `serde_json::json!` call that builds `value`: JSON as Rust tokens,
    /// strings as Rust literals (JSON's own escapes, such as `\u0001`, are not Rust's), and
    /// integers that do not fit `i32` with their type, since an unsuffixed literal is an `i32`.
    pub fn json(value: &Value) -> String {
        match value {
            Value::Null => "null".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    if i32::try_from(i).is_ok() {
                        i.to_string()
                    } else {
                        format!("{i}i64")
                    }
                } else if let Some(u) = n.as_u64() {
                    format!("{u}u64")
                } else {
                    n.to_string()
                }
            }
            Value::String(s) => string(s),
            Value::Array(items) => {
                format!("[{}]", items.iter().map(json).collect::<Vec<_>>().join(","))
            }
            Value::Object(map) => format!(
                "{{{}}}",
                map.iter()
                    .map(|(k, v)| format!("{}:{}", string(k), json(v)))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }

    /// An identifier made from `name`: characters that cannot be in one become `_`, a leading
    /// digit gets a `_` in front, a keyword is raw (`r#type`) and `self`, `Self`, `super`,
    /// `crate` and `_` get a trailing `_`.
    pub fn ident(name: &str) -> String {
        let mut out: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        if out.is_empty() {
            out.push_str("unnamed");
        }
        if out.starts_with(|c: char| c.is_ascii_digit()) {
            out.insert(0, '_');
        }
        if UNRAWABLE.contains(&out.as_str()) {
            out.push('_');
        } else if KEYWORDS.contains(&out.as_str()) {
            out.insert_str(0, "r#");
        }
        out
    }

    /// The text of a doc attribute made safe: a line feed stays, every other line break and
    /// tab becomes a space, and a control or bidirectional character becomes `<U+XXXX>`. prettyplease
    /// prints a doc string as a `///` comment as it is, so a bare `\r` or a bidirectional
    /// control in it would make rustc refuse the comment.
    fn doc_attribute_text(text: &str) -> String {
        text.split('\n')
            .map(one_line)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The tokens with every `doc = "..."` attribute value made safe ([`doc_attribute_text`]).
    /// Applied to a whole generated file, so that no doc string of any backend, whatever
    /// specification text it holds, can reach the printed comment unchecked.
    pub fn neutralise_docs(stream: TokenStream) -> TokenStream {
        stream
            .into_iter()
            .map(|tree| match tree {
                TokenTree::Group(group) => {
                    let inner = if group.delimiter() == Delimiter::Bracket {
                        doc_attribute(group.stream())
                    } else {
                        None
                    };
                    let inner = inner.unwrap_or_else(|| neutralise_docs(group.stream()));
                    let mut rebuilt = Group::new(group.delimiter(), inner);
                    rebuilt.set_span(group.span());
                    TokenTree::Group(rebuilt)
                }
                other => other,
            })
            .collect()
    }

    /// `doc = "text"` with the text made safe, or `None` for any other attribute body.
    fn doc_attribute(stream: TokenStream) -> Option<TokenStream> {
        let trees: Vec<TokenTree> = stream.into_iter().collect();
        match trees.as_slice() {
            [TokenTree::Ident(name), TokenTree::Punct(eq), TokenTree::Literal(value)]
                if name == "doc" && eq.as_char() == '=' =>
            {
                let parsed: syn::LitStr = syn::parse_str(&value.to_string()).ok()?;
                let mut safe = Literal::string(&doc_attribute_text(&parsed.value()));
                safe.set_span(value.span());
                Some(
                    [trees[0].clone(), trees[1].clone(), TokenTree::Literal(safe)]
                        .into_iter()
                        .collect(),
                )
            }
            _ => None,
        }
    }

    /// A function name `{prefix}_{id}` with every character of `id` that is not an ASCII letter
    /// or digit written as `_`. Two ids can give one name: claim it with [`Claims`].
    pub fn fn_name(prefix: &str, id: &str) -> String {
        let tail: String = id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        format!("{prefix}_{tail}")
    }
}

// ---------------------------------------------------------------------------------------------

/// Text written into generated JavaScript.
pub mod js {
    use super::*;

    /// Reserved words of JavaScript modules (strict mode), the literals, and the globals the
    /// generated modules call and so must not shadow.
    const RESERVED: &[&str] = &[
        "arguments",
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "debugger",
        "decodeURIComponent",
        "default",
        "delete",
        "do",
        "else",
        "enum",
        "encodeURIComponent",
        "eval",
        "export",
        "extends",
        "false",
        "finally",
        "for",
        "function",
        "globalThis",
        "if",
        "implements",
        "import",
        "in",
        "instanceof",
        "interface",
        "let",
        "new",
        "null",
        "package",
        "private",
        "protected",
        "public",
        "return",
        "static",
        "super",
        "switch",
        "this",
        "throw",
        "true",
        "try",
        "typeof",
        "undefined",
        "var",
        "void",
        "while",
        "with",
        "yield",
        "NaN",
        "Infinity",
    ];

    /// Text for a `//` comment or a JSDoc line: one line, no `*/`, no control characters.
    pub fn comment_text(text: &str) -> String {
        one_line(text).replace("*/", "* /")
    }

    /// As [`comment_text`], for a JSDoc block: a `@` becomes `(at)`, so text cannot start a tag.
    pub fn doc_text(text: &str) -> String {
        comment_text(text).replace('@', "(at)")
    }

    /// `//` comments, one per line of `text`, each ended by `\n`. No input ends a comment line
    /// early (U+2028, U+2029 and a bare `\r` included) or starts a line of code.
    pub fn line_comments(indent: &str, text: &str) -> String {
        comment_lines(indent, "//", text, comment_text)
    }

    /// A single-quoted JavaScript string literal for `text`.
    pub fn string(text: &str) -> String {
        let mut out = String::with_capacity(text.len() + 2);
        out.push('\'');
        let mut after_star = false;
        for c in text.chars() {
            match c {
                '\'' => out.push_str("\\'"),
                '\\' => out.push_str("\\\\"),
                '/' if after_star => out.push_str("\\/"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if c.is_control() || is_line_break(c) || is_bidi(c) => {
                    out.push_str(&format!("\\u{:04x}", c as u32));
                }
                c => out.push(c),
            }
            after_star = c == '*';
        }
        out.push('\'');
        out
    }

    /// A double-quoted JavaScript string literal for `text`.
    pub fn double_quoted(text: &str) -> String {
        let single = string(text);
        let inner = &single[1..single.len() - 1];
        let mut out = String::with_capacity(inner.len() + 2);
        out.push('"');
        let mut chars = inner.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some('\'') => out.push('\''),
                    Some(next) => {
                        out.push('\\');
                        out.push(next);
                    }
                    None => out.push('\\'),
                },
                '"' => out.push_str("\\\""),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    /// An identifier made from `name`: characters that cannot be in one become `_`, a leading
    /// digit gets a `_` in front and a reserved word a trailing `_`.
    pub fn ident(name: &str) -> String {
        let mut out: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '$' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        if out.is_empty() {
            out.push_str("unnamed");
        }
        if out.starts_with(|c: char| c.is_ascii_digit()) {
            out.insert(0, '_');
        }
        if RESERVED.contains(&out.as_str()) {
            out.push('_');
        }
        out
    }

    /// The `camelCase` JavaScript identifier of a specification id.
    pub fn camel(id: &str) -> String {
        let pascal = crate::naming::pascal(id).unwrap_or_else(|_| {
            id.chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect()
        });
        let mut chars = pascal.chars();
        let lowered = match chars.next() {
            Some(first) => format!("{}{}", first.to_ascii_lowercase(), chars.as_str()),
            None => String::new(),
        };
        ident(&lowered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOSTILE: &str = "a*/b\u{2028}c\u{2029}d\re\nf\u{0}g\u{202e}h@type 'q' \"d\" \\";

    #[test]
    fn js_comment_text_has_no_terminator() {
        let t = js::comment_text(HOSTILE);
        assert!(!t.contains("*/"));
        assert!(!t.chars().any(|c| is_line_break(c) || c.is_control()));
        assert!(!t.contains('\u{202e}'));
    }

    #[test]
    fn js_doc_text_has_no_tag() {
        assert_eq!(js::doc_text("x @type {any}"), "x (at)type {any}");
    }

    #[test]
    fn line_comments_split_at_every_terminator() {
        let out = js::line_comments("  ", "one\u{2028}two\r\nthree\rfour\n\nfive");
        assert_eq!(
            out,
            "  // one\n  // two\n  // three\n  // four\n  //\n  // five\n"
        );
        for line in out.lines() {
            assert!(line.trim_start().starts_with("//"));
        }
        assert_eq!(out.matches('\n').count(), 6);
    }

    #[test]
    fn doc_attributes_are_made_safe_in_a_whole_token_stream() {
        let tokens: proc_macro2::TokenStream = quote::quote! {
            #[doc = " a\rb\u{2028}c\u{202e}"]
            pub mod m {
                #![doc = " x\u{1}y"]
                #[doc = " keep\nline"]
                pub fn f() {}
            }
        };
        let out = rust::neutralise_docs(tokens).to_string();
        assert!(out.contains("\" a b c<U+202E>\""), "{out}");
        assert!(out.contains("\" x<U+0001>y\""), "{out}");
        assert!(out.contains("\" keep\\nline\""), "{out}");
    }

    #[test]
    fn rust_comments_cannot_inject_a_line() {
        let out = rust::doc_comment("", "a\r\nb\u{85}fn evil() {}");
        assert_eq!(out, "/// a\n/// b\n/// fn evil() {}\n");
        let out = rust::line_comments("", "x\ry");
        assert_eq!(out, "// x\n// y\n");
    }

    #[test]
    fn js_string_is_one_safe_literal() {
        let s = js::string("it's \"q\" \\ \n \u{2028} \u{0} */ x");
        assert_eq!(s, "'it\\'s \"q\" \\\\ \\n \\u2028 \\u0000 *\\/ x'");
        assert!(!s.contains("*/"));
        assert_eq!(js::double_quoted("a\"b'c"), "\"a\\\"b'c\"");
        assert_eq!(js::double_quoted("x*/y"), "\"x*\\/y\"");
    }

    #[test]
    fn rust_string_is_one_safe_literal() {
        assert_eq!(
            rust::string("a\"b\\c\nd\u{1}e\u{202e}"),
            "\"a\\\"b\\\\c\\nd\\u{1}e\\u{202e}\""
        );
    }

    #[test]
    fn json_uses_rust_escapes_and_sized_integers() {
        let v: Value =
            serde_json::from_str(r#"{"k\u0001":["a\u0001",1,3000000000,-5,1.5,null,true]}"#)
                .unwrap();
        assert_eq!(
            rust::json(&v),
            "{\"k\\u{1}\":[\"a\\u{1}\",1,3000000000i64,-5,1.5,null,true]}"
        );
    }

    #[test]
    fn identifiers_are_valid_and_never_reserved() {
        assert_eq!(js::ident("1st"), "_1st");
        assert_eq!(js::ident("class"), "class_");
        assert_eq!(js::ident("a-b"), "a_b");
        assert_eq!(js::ident(""), "unnamed");
        assert_eq!(js::camel("new"), "new_");
        assert_eq!(js::camel("2-phase"), "_2_phase");
        assert_eq!(js::ident("encodeURIComponent"), "encodeURIComponent_");
        assert_eq!(rust::ident("type"), "r#type");
        assert_eq!(rust::ident("self"), "self_");
        assert_eq!(rust::ident("_"), "__");
        assert_eq!(rust::ident("1x"), "_1x");
        assert_eq!(rust::ident("a-b"), "a_b");
        assert_eq!(rust::fn_name("scenario", "a.b"), "scenario_a_b");
    }

    #[test]
    fn claims_report_collisions_and_reserved_names() {
        let mut claims = Claims::new("contracts.js", &["request"]);
        assert!(claims.claim("a-b", "aB").is_ok());
        assert!(claims.claim("a-b", "aB").is_ok());
        let d = claims.claim("a_b", "aB").unwrap_err();
        assert_eq!(d.code, IDENTIFIER_COLLISION);
        assert_eq!(d.entity, "a_b");
        assert!(claims.claim("request", "request").is_err());
    }
}
