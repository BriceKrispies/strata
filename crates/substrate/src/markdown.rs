//! Constrained Markdown: the one rich-text format tenant content may use.
//!
//! The parser produces structured blocks, never HTML. Raw HTML in the source is plain text.
//! Links are kept only for `http:`, `https:`, `mailto:`, site-relative and fragment targets;
//! images only for site-relative `/media/` paths. The browser builds DOM nodes from the blocks
//! with safe DOM APIs, so tenant content cannot inject markup, scripts or styles.

use serde::Serialize;

/// The most bytes of one Markdown source that are read; the rest is dropped.
pub const MAX_SOURCE_BYTES: usize = 200_000;
/// The most quotes inside quotes. A deeper `>` run is kept as text of a paragraph.
pub const MAX_QUOTE_DEPTH: usize = 8;
/// The most inline elements inside inline elements (a link or emphasis in a link, and so on).
/// Markup nested deeper is kept as text.
pub const MAX_INLINE_DEPTH: usize = 8;

/// The source cut to [`MAX_SOURCE_BYTES`], at a character boundary.
fn bounded(source: &str) -> &str {
    if source.len() <= MAX_SOURCE_BYTES {
        return source;
    }
    let mut end = MAX_SOURCE_BYTES;
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    &source[..end]
}

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
    let chars: Vec<char> = bounded(source).chars().collect();
    inlines(&chars, 0)
}

fn inlines(chars: &[char], depth: usize) -> Vec<Inline> {
    if depth >= MAX_INLINE_DEPTH {
        return if chars.is_empty() {
            Vec::new()
        } else {
            vec![Inline::Text {
                text: chars.iter().collect(),
            }]
        };
    }
    // Once a `[` has found no complete link after it, no later `[` can: the scan is done once.
    let mut no_more_links = false;
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
                        children: inlines(&chars[i + 2..end], depth + 1),
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
                        children: inlines(&chars[i + 1..end], depth + 1),
                    });
                    i = end + 1;
                }
                _ => {
                    text.push(c);
                    i += 1;
                }
            }
        } else if c == '[' {
            let link = if no_more_links {
                None
            } else {
                find_close(chars, i + 1, &[']', '(']).and_then(|mid| {
                    let end = chars[mid + 2..].iter().position(|c| *c == ')')? + mid + 2;
                    Some((mid, end))
                })
            };
            if link.is_none() {
                no_more_links = true;
            }
            match link {
                Some((mid, end)) => {
                    let href: String = chars[mid + 2..end].iter().collect();
                    let href = href.trim().to_string();
                    let children = inlines(&chars[i + 1..mid], depth + 1);
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
///
/// At most [`MAX_SOURCE_BYTES`] of the source are read, quotes nest at most [`MAX_QUOTE_DEPTH`]
/// deep and inline markup at most [`MAX_INLINE_DEPTH`], so no source can exhaust the stack or
/// take more than linear time per level.
///
/// Implements: H-009
pub fn parse(source: &str) -> Vec<Block> {
    parse_at(bounded(source), 0)
}

fn parse_at(source: &str, quote_depth: usize) -> Vec<Block> {
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
            if quote_depth >= MAX_QUOTE_DEPTH {
                // Too deep to nest further: what is left is a paragraph of text.
                let text = inner.join(" ");
                blocks.push(Block::Paragraph {
                    inlines: parse_inlines(&text),
                });
            } else {
                blocks.push(Block::Quote {
                    blocks: parse_at(&inner.join("\n"), quote_depth + 1),
                });
            }
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

    /// The parser's half of CMS-004; the proof of the requirement itself is the gateway test
    /// `tenant_rich_text_reaches_the_browser_as_blocks_with_markup_as_text`.
    /// Verifies: H-009
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
        // Only the web and mail schemes survive, and the same source gives the same blocks.
        let links = parse("[m](mailto:a@example.com) [d](data:text/html,x) [v](vbscript:x) [f](file:///etc/passwd) [h](http://example.com)");
        let hrefs: Vec<String> = match &links[0] {
            Block::Paragraph { inlines } => inlines
                .iter()
                .filter_map(|i| match i {
                    Inline::Link { href, .. } => Some(href.clone()),
                    _ => None,
                })
                .collect(),
            _ => panic!(),
        };
        assert_eq!(hrefs, ["mailto:a@example.com", "http://example.com"]);
        assert_eq!(
            parse("A *b* [c](https://example.com)"),
            parse("A *b* [c](https://example.com)")
        );
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

    fn quote_depth(blocks: &[Block]) -> usize {
        blocks
            .iter()
            .map(|block| match block {
                Block::Quote { blocks } => 1 + quote_depth(blocks),
                _ => 0,
            })
            .max()
            .unwrap_or(0)
    }

    fn inline_depth(inlines: &[Inline]) -> usize {
        inlines
            .iter()
            .map(|inline| match inline {
                Inline::Em { children }
                | Inline::Strong { children }
                | Inline::Link { children, .. } => 1 + inline_depth(children),
                _ => 0,
            })
            .max()
            .unwrap_or(0)
    }

    /// Run on a thread with a small stack, so a recursion that follows the input depth overflows
    /// here instead of passing on the generous stack of the test runner.
    fn on_small_stack<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(work)
            .unwrap()
            .join()
            .unwrap()
    }

    /// Verifies: CMS-1020
    #[test]
    fn two_hundred_thousand_quote_markers_stop_at_the_depth_cap() {
        let blocks = on_small_stack(|| parse(&">".repeat(200_000)));
        assert_eq!(quote_depth(&blocks), MAX_QUOTE_DEPTH);
        let spaced = on_small_stack(|| parse(&"> ".repeat(100_000)));
        assert_eq!(quote_depth(&spaced), MAX_QUOTE_DEPTH);
        let lines = on_small_stack(|| parse(&">".repeat(10_000).replace('>', ">\n")));
        assert!(quote_depth(&lines) <= MAX_QUOTE_DEPTH);
    }

    /// Verifies: CMS-1020
    #[test]
    fn two_hundred_thousand_brackets_parse_in_bounded_time() {
        let started = std::time::Instant::now();
        let inlines = on_small_stack(|| parse_inlines(&"[".repeat(200_000)));
        assert_eq!(inline_depth(&inlines), 0);
        // Many openers before one complete link, and many unfinished links.
        let nested = on_small_stack(|| {
            parse_inlines(&format!("{}x](https://example.com)", "[".repeat(100_000)))
        });
        assert!(inline_depth(&nested) <= MAX_INLINE_DEPTH);
        on_small_stack(|| parse_inlines(&"[a](".repeat(50_000)));
        on_small_stack(|| parse_inlines(&"[a]".repeat(66_000)));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "took {:?}",
            started.elapsed()
        );
    }

    /// Verifies: CMS-1020
    #[test]
    fn a_document_at_the_maximum_depth_counts_and_serialises() {
        let source = format!(
            "{}{}x](/a)",
            "> ".repeat(MAX_QUOTE_DEPTH + 5),
            "[".repeat(MAX_INLINE_DEPTH + 5)
        );
        let (depth, words, json) = on_small_stack(move || {
            let blocks = parse(&source);
            (
                quote_depth(&blocks),
                word_count(&blocks),
                serde_json::to_string(&blocks).unwrap(),
            )
        });
        assert_eq!(depth, MAX_QUOTE_DEPTH);
        assert!(words >= 1);
        assert!(json.contains("\"quote\""));
    }

    /// Verifies: CMS-1020
    #[test]
    fn a_source_is_read_only_up_to_the_maximum_length() {
        let words = word_count(&parse(&"word ".repeat(100_000)));
        assert_eq!(words, MAX_SOURCE_BYTES / 5);
        // A cut inside a multi-byte character falls back to the character's start.
        let wide = "€".repeat(MAX_SOURCE_BYTES);
        assert_eq!(bounded(&wide).len(), 199_998);
        assert!(!parse_inlines(&wide).is_empty());
    }
}
