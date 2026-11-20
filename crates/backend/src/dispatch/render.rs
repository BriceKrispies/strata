//! Rendering a message from its template (render-message, EML-016, k-450) and the header checks
//! (EML-1566, D-6572).
//!
//! The body of a template is structured blocks (D-7900, `strata_kernel::blocks`). The body is
//! rendered by `strata_kernel::blocks::render`, the one function the mail dispatcher and the
//! notice service share: each value the body places is a text node, each token is replaced by
//! its text, and the result is blocks that place neither. This module then writes the plain text
//! part from the rendered text and the HTML part from the rendered blocks, with every text
//! escaped there, so neither a value nor the template's own text can place markup in a message.
//!
//! - The values of a send arrive as text and are read as the type the body declares for each
//!   (`str`, `int`, `bool`, `time` as whole milliseconds); a value that does not read as its
//!   type, or one the body places and the send does not give, refuses the message.
//! - A token has no reader here yet (CTK-011, CTK-015: the expression is evaluated for a
//!   reader), so each token renders as its fallback text.
//! - The subject is one line of text, not blocks: `{{name}}` in it is replaced by the value
//!   `name` of the send, as text.

use serde_json::Value as Json;
use std::collections::BTreeMap;
use strata_kernel::blocks::{self, ValueType};
use strata_kernel::{MailTemplateDecl, Value};

/// A message rendered for one recipient: its subject and its two parts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rendered {
    pub subject: String,
    pub text: String,
    pub html: String,
}

/// `text` with every `{{name}}` that `values` names replaced by its value.
pub fn fill(text: &str, values: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        match after.find("}}") {
            Some(close) => {
                let name = after[..close].trim();
                match values.get(name) {
                    Some(value) => out.push_str(value),
                    None => out.push_str(&rest[open..open + 2 + close + 2]),
                }
                rest = &after[close + 2..];
            }
            None => {
                out.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Text escaped for an HTML element or attribute.
pub fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// The values a body places, read from the text of a send as the type the body declares for each.
pub fn typed_values(
    body: &str,
    values: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, Value>, String> {
    let mut typed_values = BTreeMap::new();
    for (name, declared) in &blocks::analyse(body).values {
        let text = values
            .get(name)
            .ok_or_else(|| format!("the value {name} was not supplied"))?;
        typed_values.insert(name.clone(), typed(name, *declared, text)?);
    }
    Ok(typed_values)
}

fn typed(name: &str, declared: ValueType, text: &str) -> Result<Value, String> {
    match declared {
        ValueType::Str => Ok(Value::Str(text.to_string())),
        ValueType::Int => text
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|_| format!("the value {name} is not a whole number")),
        ValueType::Bool => match text {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(format!("the value {name} is not true or false")),
        },
        ValueType::Time => text
            .parse::<i64>()
            .map(Value::Time)
            .map_err(|_| format!("the value {name} is not a time in milliseconds")),
    }
}

fn inlines_html(list: &[Json], out: &mut String) {
    for node in list {
        let text = node.get("text").and_then(Json::as_str).unwrap_or_default();
        let children = node
            .get("children")
            .and_then(Json::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        match node.get("type").and_then(Json::as_str) {
            Some("text") => out.push_str(&escape_html(text)),
            Some("code") => out.push_str(&format!("<code>{}</code>", escape_html(text))),
            Some("em") => {
                out.push_str("<em>");
                inlines_html(children, out);
                out.push_str("</em>");
            }
            Some("strong") => {
                out.push_str("<strong>");
                inlines_html(children, out);
                out.push_str("</strong>");
            }
            Some("link") => {
                let href = node.get("href").and_then(Json::as_str).unwrap_or_default();
                if blocks::allowed_href(href) {
                    out.push_str(&format!("<a href=\"{}\">", escape_html(href)));
                    inlines_html(children, out);
                    out.push_str("</a>");
                } else {
                    inlines_html(children, out);
                }
            }
            _ => {}
        }
    }
}

fn blocks_html(list: &[Json], out: &mut Vec<String>) {
    for block in list {
        let mut inner = String::new();
        let inlines = block
            .get("inlines")
            .and_then(Json::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        match block.get("type").and_then(Json::as_str) {
            Some("paragraph") => {
                inlines_html(inlines, &mut inner);
                out.push(format!("<p>{inner}</p>"));
            }
            Some("heading") => {
                let level = block
                    .get("level")
                    .and_then(Json::as_u64)
                    .filter(|l| (2..=4).contains(l))
                    .unwrap_or(2);
                inlines_html(inlines, &mut inner);
                out.push(format!("<h{level}>{inner}</h{level}>"));
            }
            Some("quote") => {
                let mut nested = Vec::new();
                blocks_html(
                    block
                        .get("blocks")
                        .and_then(Json::as_array)
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                    &mut nested,
                );
                out.push(format!("<blockquote>{}</blockquote>", nested.join("")));
            }
            Some("list") => {
                let tag = if block.get("ordered") == Some(&Json::Bool(true)) {
                    "ol"
                } else {
                    "ul"
                };
                let items: String = block
                    .get("items")
                    .and_then(Json::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .map(|item| {
                        let mut one = String::new();
                        inlines_html(
                            item.as_array().map(Vec::as_slice).unwrap_or_default(),
                            &mut one,
                        );
                        format!("<li>{one}</li>")
                    })
                    .collect();
                out.push(format!("<{tag}>{items}</{tag}>"));
            }
            Some("code") => {
                let text = block.get("text").and_then(Json::as_str).unwrap_or_default();
                out.push(format!("<pre><code>{}</code></pre>", escape_html(text)));
            }
            Some("figure") => {
                // A figure points at a file of the site; a message carries its description only.
                let alt = block.get("alt").and_then(Json::as_str).unwrap_or_default();
                out.push(format!("<p>{}</p>", escape_html(alt)));
            }
            Some("rule") => out.push("<hr>".to_string()),
            _ => {}
        }
    }
}

/// Render a template with the values of a send; `token` gives the text of a token, or `None` for
/// one that cannot be resolved. Why not, when the body is not blocks, a value it places is
/// missing or of another type, or a token has no text.
///
/// Implements: CT-220
pub fn render(
    decl: &MailTemplateDecl,
    values: &BTreeMap<String, String>,
    token: &dyn Fn(&str) -> Option<String>,
) -> Result<Rendered, String> {
    let body = decl.body.as_str();
    let typed_values = typed_values(body, values)?;
    let done = blocks::render(body, &typed_values, token).map_err(|e| e.to_string())?;
    let mut html = Vec::new();
    if let Json::Array(list) = &done.blocks {
        blocks_html(list, &mut html);
    }
    Ok(Rendered {
        subject: fill(&decl.subject, values),
        text: done.text,
        html: html.join("\n"),
    })
}

/// Add a closing paragraph with a link to both parts of a rendered message.
pub fn with_link(rendered: &mut Rendered, sentence: &str, url: &str) {
    if !rendered.text.is_empty() {
        rendered.text.push_str("\n\n");
    }
    rendered.text.push_str(&format!("{sentence} {url}"));
    if !rendered.html.is_empty() {
        rendered.html.push('\n');
    }
    rendered.html.push_str(&format!(
        "<p>{} <a href=\"{}\">{}</a></p>",
        escape_html(sentence),
        escape_html(url),
        escape_html(url)
    ));
}

/// Why a header field cannot be sent as it is, or `None`: a carriage return, a line feed or a NUL
/// byte would begin a new header (EML-1566). Refused, never removed (D-6572).
pub fn header_problem(value: &str) -> Option<&'static str> {
    if value.contains(['\r', '\n', '\0']) {
        Some("holds a line break or a NUL byte")
    } else {
        None
    }
}

/// Why `address` is not one address that can be sent to, or `None`: no line break or NUL byte,
/// one address only (one `@`, no space, comma, semicolon or angle bracket), at most 254 bytes
/// (RFC 5321), with text on both sides of the `@` (EML-1566).
pub fn address_problem(address: &str) -> Option<&'static str> {
    if let Some(problem) = header_problem(address) {
        return Some(problem);
    }
    if address.len() > 254 {
        return Some("is longer than 254 bytes");
    }
    if address
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, ',' | ';' | '<' | '>' | '"' | '(' | ')'))
    {
        return Some("is not one address");
    }
    match address.split_once('@') {
        Some((local, domain))
            if !local.is_empty() && !domain.is_empty() && !domain.contains('@') =>
        {
            None
        }
        _ => Some("is not one address"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_kernel::{StructuredBlocks, TemplateId};

    fn template(subject: &str, body: &str) -> MailTemplateDecl {
        MailTemplateDecl {
            template: TemplateId("welcome".into()),
            subject: subject.into(),
            body: StructuredBlocks(body.into()),
        }
    }

    // Verifies: EML-016
    #[test]
    fn blocks_render_as_paragraphs_and_markup_in_a_value_is_text() {
        let values = BTreeMap::from([("name".to_string(), "<b>Ada</b>".to_string())]);
        let rendered = render(
            &template(
                "Hello {{name}}",
                r#"[{"type":"paragraph","inlines":[{"type":"text","text":"Hi "},{"type":"value","name":"name","value_type":"str"}]},{"type":"paragraph","inlines":[{"type":"text","text":"Bye <i>"}]}]"#,
            ),
            &values,
            &|_| None,
        )
        .expect("rendered");
        assert_eq!(rendered.subject, "Hello <b>Ada</b>");
        assert_eq!(rendered.text, "Hi <b>Ada</b>\n\nBye <i>");
        assert_eq!(
            rendered.html,
            "<p>Hi &lt;b&gt;Ada&lt;/b&gt;</p>\n<p>Bye &lt;i&gt;</p>"
        );
    }

    // Verifies: EML-1566
    #[test]
    fn a_line_break_in_a_header_or_two_addresses_are_refused() {
        assert!(header_problem("Hello\r\nBcc: x@y.example").is_some());
        assert!(header_problem("Hello").is_none());
        assert!(address_problem("a@x.example").is_none());
        assert!(address_problem("a@x.example, b@y.example").is_some());
        assert!(address_problem("a@x.example\nBcc: b@y.example").is_some());
        assert!(address_problem("nobody").is_some());
    }
}
