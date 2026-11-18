//! Constrained Markdown: the one rich-text format tenant content may use.
//!
//! The parser produces structured blocks, never HTML. Raw HTML in the source is plain text.
//! Links are kept only for `http:`, `https:`, `mailto:`, site-relative and fragment targets;
//! images only for site-relative `/media/` paths. The browser builds DOM nodes from the blocks
//! with safe DOM APIs, so tenant content cannot inject markup, scripts or styles.

use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Inline {
    Text { text: String },
    Em { children: Vec<Inline> },
    Strong { children: Vec<Inline> },
    Code { text: String },
    Link { href: String, children: Vec<Inline> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Heading {
        level: u8,
        inlines: Vec<Inline>,
    },
    Paragraph {
        inlines: Vec<Inline>,
    },
    Quote {
        blocks: Vec<Block>,
    },
    List {
        ordered: bool,
        items: Vec<Vec<Inline>>,
    },
    Code {
        lang: String,
        text: String,
    },
    Figure {
        src: String,
        alt: String,
        caption: String,
    },
    Rule,
}

fn safe_href(href: &str) -> bool {
    let lower = href.to_ascii_lowercase();
    (href.starts_with('/') && !href.starts_with("//"))
        || href.starts_with('#')
        || lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:")
}

fn push_text(out: &mut Vec<Inline>, text: &mut String) {
    if !text.is_empty() {
        out.push(Inline::Text {
            text: std::mem::take(text),
        });
    }
}

/// Find the closing delimiter, skipping escaped characters.
fn find_close(chars: &[char], from: usize, delimiter: &[char]) -> Option<usize> {
    let mut i = from;
    while i + delimiter.len() <= chars.len() {
        if chars[i] == '\\' {
            i += 2;
            continue;
        }
        if chars[i..].starts_with(delimiter) {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Parse inline markup: `code`, `**strong**`, `*em*`, `_em_`, `[text](href)` and `\` escapes.
pub fn parse_inlines(source: &str) -> Vec<Inline> {
    let chars: Vec<char> = source.chars().collect();
    inlines(&chars)
}

fn inlines(chars: &[char]) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let rest = &chars[i..];
        if c == '\\' && i + 1 < chars.len() {
            text.push(chars[i + 1]);
            i += 2;
        } else if c == '`' {
            match chars[i + 1..].iter().position(|c| *c == '`') {
                Some(len) => {
                    push_text(&mut out, &mut text);
                    out.push(Inline::Code {
                        text: chars[i + 1..i + 1 + len].iter().collect(),
                    });
                    i += len + 2;
                }
                None => {
                    text.push(c);
                    i += 1;
                }
            }
        } else if rest.starts_with(&['*', '*']) {
            match find_close(chars, i + 2, &['*', '*']) {
                Some(end) if end > i + 2 => {
                    push_text(&mut out, &mut text);
                    out.push(Inline::Strong {
                        children: inlines(&chars[i + 2..end]),
                    });
                    i = end + 2;
                }
                _ => {
                    text.push_str("**");
                    i += 2;
                }
            }
        } else if c == '*' || c == '_' {
            // An underscore inside a word is just an underscore.
            let inside_word = c == '_' && i > 0 && chars[i - 1].is_alphanumeric();
            match find_close(chars, i + 1, &[c]) {
                Some(end) if end > i + 1 && !inside_word && !chars[i + 1].is_whitespace() => {
                    push_text(&mut out, &mut text);
                    out.push(Inline::Em {
                        children: inlines(&chars[i + 1..end]),
                    });
                    i = end + 1;
                }
                _ => {
                    text.push(c);
                    i += 1;
                }
            }
        } else if c == '[' {
            let link = find_close(chars, i + 1, &[']', '(']).and_then(|mid| {
                let end = chars[mid + 2..].iter().position(|c| *c == ')')? + mid + 2;
                Some((mid, end))
            });
            match link {
                Some((mid, end)) => {
                    let href: String = chars[mid + 2..end].iter().collect();
                    let href = href.trim().to_string();
                    let children = inlines(&chars[i + 1..mid]);
                    push_text(&mut out, &mut text);
                    if safe_href(&href) {
                        out.push(Inline::Link { href, children });
                    } else {
                        out.extend(children);
                    }
                    i = end + 1;
                }
                None => {
                    text.push(c);
                    i += 1;
                }
            }
        } else {
            text.push(c);
            i += 1;
        }
    }
    push_text(&mut out, &mut text);
    out
}

fn ordered_item(line: &str) -> Option<&str> {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 3 {
        return None;
    }
    line[digits..].strip_prefix(". ")
}

fn bullet_item(line: &str) -> Option<&str> {
    line.strip_prefix("- ").or_else(|| line.strip_prefix("* "))
}

/// `![alt](/media/file "caption")` alone on a line.
fn figure(line: &str) -> Option<Block> {
    let rest = line.strip_prefix("![")?.strip_suffix(')')?;
    let (alt, target) = rest.split_once("](")?;
    let (src, caption) = match target.split_once(' ') {
        Some((src, caption)) => (src, caption.trim().trim_matches('"')),
        None => (target, ""),
    };
    let plain = src
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.'));
    if !src.starts_with("/media/") || !plain || src.contains("..") {
        return None;
    }
    Some(Block::Figure {
        src: src.to_string(),
        alt: alt.to_string(),
        caption: caption.to_string(),
    })
}

/// Parse constrained Markdown into blocks.
pub fn parse(source: &str) -> Vec<Block> {
    let normalised = source.replace("\r\n", "\n");
    let lines: Vec<&str> = normalised.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_end();
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            i += 1;
        } else if let Some(lang) = trimmed.strip_prefix("```") {
            let mut text = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                text.push(lines[i]);
                i += 1;
            }
            i += 1;
            let lang: String = lang
                .trim()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect();
            blocks.push(Block::Code {
                lang,
                text: text.join("\n"),
            });
        } else if trimmed == "---" || trimmed == "***" {
            blocks.push(Block::Rule);
            i += 1;
        } else if let Some(heading) = trimmed.strip_prefix('#') {
            let extra = heading.chars().take_while(|c| *c == '#').count();
            let text = heading[extra..].trim();
            // The page title is the only first-level heading, so content starts at level two.
            let level = (extra as u8 + 1).clamp(2, 4);
            blocks.push(Block::Heading {
                level,
                inlines: parse_inlines(text),
            });
            i += 1;
        } else if trimmed.starts_with('>') {
            let mut inner = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with('>') {
                let content = lines[i].trim_start()[1..]
                    .strip_prefix(' ')
                    .unwrap_or(&lines[i].trim_start()[1..]);
                inner.push(content.to_string());
                i += 1;
            }
            blocks.push(Block::Quote {
                blocks: parse(&inner.join("\n")),
            });
        } else if bullet_item(trimmed).is_some() || ordered_item(trimmed).is_some() {
            let ordered = bullet_item(trimmed).is_none();
            let mut items: Vec<String> = Vec::new();
            while i < lines.len() {
                let current = lines[i].trim_end();
                let start = current.trim_start();
                if let Some(item) = if ordered {
                    ordered_item(start)
                } else {
                    bullet_item(start)
                } {
                    items.push(item.to_string());
                } else if current.starts_with("  ")
                    && !current.trim().is_empty()
                    && !items.is_empty()
                {
                    let last = items.last_mut().expect("non-empty");
                    last.push(' ');
                    last.push_str(current.trim());
                } else {
                    break;
                }
                i += 1;
            }
            blocks.push(Block::List {
                ordered,
                items: items.iter().map(|item| parse_inlines(item)).collect(),
            });
        } else if let Some(block) = figure(trimmed) {
            blocks.push(block);
            i += 1;
        } else {
            let mut text = String::new();
            while i < lines.len() {
                let current = lines[i].trim();
                let starts_block = current.is_empty()
                    || current.starts_with('#')
                    || current.starts_with('>')
                    || current.starts_with("```")
                    || current == "---"
                    || bullet_item(current).is_some()
                    || ordered_item(current).is_some();
                if starts_block && !text.is_empty() {
                    break;
                }
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(current);
                i += 1;
            }
            blocks.push(Block::Paragraph {
                inlines: parse_inlines(&text),
            });
        }
    }
    blocks
}

/// Whitespace-separated runs that contain a letter or digit; stray punctuation is not a word.
fn words(text: &str) -> usize {
    text.split_whitespace()
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .count()
}

fn inline_words(inlines: &[Inline]) -> usize {
    inlines
        .iter()
        .map(|inline| match inline {
            Inline::Text { text } | Inline::Code { text } => words(text),
            Inline::Em { children }
            | Inline::Strong { children }
            | Inline::Link { children, .. } => inline_words(children),
        })
        .sum()
}

/// The number of words a reader has to get through.
pub fn word_count(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .map(|block| match block {
            Block::Heading { inlines, .. } | Block::Paragraph { inlines } => inline_words(inlines),
            Block::Quote { blocks } => word_count(blocks),
            Block::List { items, .. } => items.iter().map(|item| inline_words(item)).sum(),
            Block::Code { text, .. } => words(text),
            Block::Figure { .. } | Block::Rule => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Inline {
        Inline::Text {
            text: s.to_string(),
        }
    }

    #[test]
    fn parses_the_supported_blocks() {
        let blocks = parse("## Making\n\nFirst *line*\nsecond line.\n\n> Quoted **text**\n\n- one\n- two\n  continued\n\n1. first\n2. second\n\n```rust\nlet x = 1;\n```\n\n---\n\n![A kiln](/media/kiln.svg \"The kiln at dusk\")\n");
        assert_eq!(blocks.len(), 8);
        assert_eq!(
            blocks[0],
            Block::Heading {
                level: 2,
                inlines: vec![text("Making")]
            }
        );
        assert_eq!(
            blocks[1],
            Block::Paragraph {
                inlines: vec![
                    text("First "),
                    Inline::Em {
                        children: vec![text("line")]
                    },
                    text(" second line.")
                ]
            }
        );
        assert!(matches!(&blocks[2], Block::Quote { blocks } if blocks.len() == 1));
        assert_eq!(
            blocks[3],
            Block::List {
                ordered: false,
                items: vec![vec![text("one")], vec![text("two continued")]]
            }
        );
        assert!(matches!(&blocks[4], Block::List { ordered: true, items } if items.len() == 2));
        assert_eq!(
            blocks[5],
            Block::Code {
                lang: "rust".into(),
                text: "let x = 1;".into()
            }
        );
        assert_eq!(blocks[6], Block::Rule);
        assert!(
            matches!(&blocks[7], Block::Figure { caption, .. } if caption == "The kiln at dusk")
        );
    }

    #[test]
    fn figures_come_only_from_local_media() {
        assert_eq!(
            parse("![A kiln](/media/kiln.svg \"At dusk\")"),
            vec![Block::Figure {
                src: "/media/kiln.svg".into(),
                alt: "A kiln".into(),
                caption: "At dusk".into()
            }]
        );
        assert!(matches!(
            parse("![x](https://example.com/a.png)")[0],
            Block::Paragraph { .. }
        ));
        assert!(matches!(
            parse("![x](/media/../secret)")[0],
            Block::Paragraph { .. }
        ));
    }

    /// Verifies: CMS-004
    #[test]
    fn html_is_text_and_unsafe_links_lose_their_target() {
        let blocks = parse("<script>alert(1)</script> and [click](javascript:alert(1)) and [ok](https://example.com)");
        let Block::Paragraph { inlines } = &blocks[0] else {
            panic!()
        };
        assert_eq!(inlines[0], text("<script>alert(1)</script> and "));
        assert_eq!(inlines[1], text("click"));
        assert!(inlines
            .iter()
            .any(|i| matches!(i, Inline::Link { href, .. } if href == "https://example.com")));
        assert!(!serde_json::to_string(&blocks)
            .unwrap()
            .contains("javascript:"));
        // Other markup, inline and at the start of a line, is text as well.
        let other = parse("<img src=x onerror=alert(1)> then <b>bold</b>\n\n<div>block</div>");
        let json = serde_json::to_string(&other).unwrap();
        assert!(json.contains("<img src=x onerror=alert(1)>"), "{json}");
        assert!(json.contains("<b>bold</b>"), "{json}");
        assert!(json.contains("<div>block</div>"), "{json}");
    }

    #[test]
    fn underscores_inside_words_and_escapes_are_literal() {
        assert_eq!(
            parse_inlines("snake_case_name and 2 \\* 3"),
            vec![text("snake_case_name and 2 * 3")]
        );
        assert_eq!(
            parse_inlines("`a*b*`"),
            vec![Inline::Code {
                text: "a*b*".into()
            }]
        );
    }

    #[test]
    fn counts_words() {
        assert_eq!(word_count(&parse("One two **three**.\n\n- four five")), 5);
    }
}
