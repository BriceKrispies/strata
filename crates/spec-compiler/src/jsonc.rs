//! JSON with comments and trailing commas, reduced to plain JSON.
//!
//! Comments (`// ...` to the end of the line, `/* ... */`) become spaces, keeping every newline,
//! so the line and column numbers `serde_json` reports still point into the original text. A
//! comma followed only by whitespace or comments and then `}` or `]` is dropped the same way.
//! Text inside strings is never touched.

use std::fmt;

/// A JSONC text that cannot be reduced to JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsoncError {
    /// The 1-based line where the problem starts.
    pub line: usize,
    /// What is wrong.
    pub message: String,
}

impl fmt::Display for JsoncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for JsoncError {}

/// Replaces comments and trailing commas with spaces. The result has the same length in
/// characters and the same line breaks as `text`.
pub fn strip(text: &str) -> Result<String, JsoncError> {
    let without_comments = strip_comments(text)?;
    Ok(strip_trailing_commas(&without_comments))
}

/// Parses JSONC into a JSON value.
pub fn parse(text: &str) -> Result<serde_json::Value, JsoncError> {
    let json = strip(text)?;
    serde_json::from_str(&json).map_err(|e| JsoncError {
        line: e.line(),
        message: e.to_string(),
    })
}

fn strip_comments(text: &str) -> Result<String, JsoncError> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut line = 1;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' => {
                let start_line = line;
                out.push(c);
                i += 1;
                let mut closed = false;
                while i < chars.len() {
                    let s = chars[i];
                    out.push(s);
                    i += 1;
                    match s {
                        '\\' => {
                            if let Some(&next) = chars.get(i) {
                                if next == '\n' {
                                    line += 1;
                                }
                                out.push(next);
                                i += 1;
                            }
                        }
                        '"' => {
                            closed = true;
                            break;
                        }
                        '\n' => line += 1,
                        _ => {}
                    }
                }
                if !closed {
                    return Err(JsoncError {
                        line: start_line,
                        message: "unterminated string".to_string(),
                    });
                }
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    out.push(' ');
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                let start_line = line;
                out.push_str("  ");
                i += 2;
                let mut closed = false;
                while i < chars.len() {
                    if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                        out.push_str("  ");
                        i += 2;
                        closed = true;
                        break;
                    }
                    if chars[i] == '\n' {
                        line += 1;
                        out.push('\n');
                    } else {
                        out.push(' ');
                    }
                    i += 1;
                }
                if !closed {
                    return Err(JsoncError {
                        line: start_line,
                        message: "unterminated block comment".to_string(),
                    });
                }
            }
            _ => {
                if c == '\n' {
                    line += 1;
                }
                out.push(c);
                i += 1;
            }
        }
    }
    Ok(out)
}

/// Drops commas that only whitespace separates from a closing bracket. Runs on text that has
/// no comments left, and skips strings.
fn strip_trailing_commas(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            out.push(c);
            i += 1;
            while i < chars.len() {
                let s = chars[i];
                out.push(s);
                i += 1;
                if s == '\\' {
                    if let Some(&next) = chars.get(i) {
                        out.push(next);
                        i += 1;
                    }
                } else if s == '"' {
                    break;
                }
            }
            continue;
        }
        if c == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if matches!(chars.get(j), Some('}' | ']')) {
                out.push(' ');
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_comments_become_spaces_and_keep_lines() {
        let text = "{\n  \"a\": 1 // one\n}";
        let out = strip(text).unwrap();
        assert_eq!(out, "{\n  \"a\": 1       \n}");
        assert_eq!(parse(text).unwrap(), serde_json::json!({"a": 1}));
    }

    #[test]
    fn block_comments_keep_newlines() {
        let text = "{/* a\nb */\"a\": 1}";
        let out = strip(text).unwrap();
        assert_eq!(out.lines().count(), 2);
        assert_eq!(parse(text).unwrap(), serde_json::json!({"a": 1}));
    }

    #[test]
    fn comment_markers_inside_strings_are_kept() {
        let text = r#"{"url": "http://x/*y*/", "s": "a // b"}"#;
        assert_eq!(strip(text).unwrap(), text);
    }

    #[test]
    fn escaped_quotes_do_not_end_a_string() {
        let text = r#"{"q": "say \"hi\" // not a comment", "n": 2,}"#;
        let value = parse(text).unwrap();
        assert_eq!(value["q"], "say \"hi\" // not a comment");
        assert_eq!(value["n"], 2);
    }

    #[test]
    fn trailing_commas_are_dropped() {
        let text = "{\"a\": [1, 2, ], \"b\": {\"c\": 3,\n}, }";
        assert_eq!(
            parse(text).unwrap(),
            serde_json::json!({"a": [1, 2], "b": {"c": 3}})
        );
    }

    #[test]
    fn trailing_comma_before_a_comment_is_dropped() {
        let text = "[1, // last\n]";
        assert_eq!(parse(text).unwrap(), serde_json::json!([1]));
    }

    #[test]
    fn comma_inside_a_string_before_a_bracket_is_kept() {
        let text = r#"["a,]", "b"]"#;
        assert_eq!(parse(text).unwrap(), serde_json::json!(["a,]", "b"]));
    }

    #[test]
    fn unterminated_block_comment_is_an_error() {
        let err = strip("{\n/* open").unwrap_err();
        assert_eq!(err.line, 2);
    }

    #[test]
    fn unterminated_string_is_an_error() {
        assert!(strip("{\"a\": \"open}").is_err());
    }

    #[test]
    fn syntax_errors_report_the_original_line() {
        let err = parse("{\n// c\n\"a\": }").unwrap_err();
        assert_eq!(err.line, 3);
    }
}
