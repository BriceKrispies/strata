//! Rendering a message from its template (render-message, EML-016, k-450) and the header checks
//! (EML-1566, D-6572).
//!
//! The specification states that the body of a template is structured blocks held as JSON text
//! and states no schema for a block and no syntax that places a token or a value in one
//! (WG-5177). Until it does, this module reads the blocks as paragraphs of text and nothing more:
//!
//! - a body that is a JSON array is a list of blocks; any other JSON value is one block;
//! - a block that is a string is a paragraph of that text; a block that is an object with a
//!   string member `text` is a paragraph of that text; any other block renders nothing;
//! - in the subject and in each paragraph, `{{name}}` is replaced by the value `name` of the
//!   send, as text; a name the send gives no value for is left as written.
//!
//! Every paragraph becomes a line of the plain text part and a `<p>` of the HTML part, with the
//! text escaped there, so neither a value nor the template's own text can place markup in a
//! message.

use std::collections::BTreeMap;
use strata_kernel::MailTemplateDecl;

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

/// The paragraphs of a body of structured blocks, as this module reads them.
pub fn paragraphs(body: &str) -> Vec<String> {
    let parsed: serde_json::Value = match serde_json::from_str(body) {
        Ok(value) => value,
        // A body that is not JSON text is refused when the template is defined; one that got
        // through renders as one paragraph of its text.
        Err(_) => return vec![body.to_string()],
    };
    let blocks = match parsed {
        serde_json::Value::Array(blocks) => blocks,
        other => vec![other],
    };
    blocks
        .into_iter()
        .filter_map(|block| match block {
            serde_json::Value::String(text) => Some(text),
            serde_json::Value::Object(fields) => match fields.get("text") {
                Some(serde_json::Value::String(text)) => Some(text.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect()
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

/// Render a template with the values of a send.
///
/// Implements: CT-220
pub fn render(decl: &MailTemplateDecl, values: &BTreeMap<String, String>) -> Rendered {
    let paragraphs: Vec<String> = paragraphs(decl.body.as_str())
        .iter()
        .map(|p| fill(p, values))
        .collect();
    Rendered {
        subject: fill(&decl.subject, values),
        text: paragraphs.join("\n\n"),
        html: paragraphs
            .iter()
            .map(|p| format!("<p>{}</p>", escape_html(p)))
            .collect::<Vec<_>>()
            .join("\n"),
    }
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
                r#"[{"type":"paragraph","text":"Hi {{name}}"},"Bye",{"type":"image"}]"#,
            ),
            &values,
        );
        assert_eq!(rendered.subject, "Hello <b>Ada</b>");
        assert_eq!(rendered.text, "Hi <b>Ada</b>\n\nBye");
        assert_eq!(
            rendered.html,
            "<p>Hi &lt;b&gt;Ada&lt;/b&gt;</p>\n<p>Bye</p>"
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
