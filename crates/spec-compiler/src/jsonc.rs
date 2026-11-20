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
    /// True when the text is valid JSON but one object names the same key twice.
    pub duplicate_key: bool,
}

/// The rule code of an object that names one key twice.
pub const DUPLICATE_KEY: &str = "specc.duplicate-key";

impl JsoncError {
    /// The rule code for this error: [`DUPLICATE_KEY`] for a repeated key, else `syntax`.
    #[must_use]
    pub fn code(&self, syntax: &'static str) -> &'static str {
        if self.duplicate_key {
            DUPLICATE_KEY
        } else {
            syntax
        }
    }
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
    let value = serde_json::from_str(&json).map_err(|e| JsoncError {
        line: e.line(),
        message: e.to_string(),
        duplicate_key: false,
    })?;
    // `serde_json` keeps the last of two equal keys without a word, so a second `"auth"` could
    // replace the first unseen. The text is valid JSON here, so the scan can be lenient.
    if let Some((line, key)) = find_duplicate_key(&json) {
        return Err(JsoncError {
            line,
            message: format!(
                "the object names the key \"{key}\" twice; keep one and merge the other into it"
            ),
            duplicate_key: true,
        });
    }
    Ok(value)
}

/// The line and decoded name of the first key that an object repeats, in a valid JSON text.
fn find_duplicate_key(json: &str) -> Option<(usize, String)> {
    let mut scan = KeyScan {
        chars: json.chars().collect(),
        at: 0,
        line: 1,
    };
    scan.value()
}

struct KeyScan {
    chars: Vec<char>,
    at: usize,
    line: usize,
}

impl KeyScan {
    fn skip_space(&mut self) {
        while let Some(&c) = self.chars.get(self.at) {
            if c == '\n' {
                self.line += 1;
            } else if !c.is_whitespace() {
                break;
            }
            self.at += 1;
        }
    }

    /// Reads a string literal at the cursor and returns its decoded text.
    fn string(&mut self) -> String {
        let start = self.at;
        self.at += 1;
        while let Some(&c) = self.chars.get(self.at) {
            self.at += 1;
            match c {
                '\\' => {
                    if self.chars.get(self.at) == Some(&'\n') {
                        self.line += 1;
                    }
                    self.at += 1;
                }
                '"' => break,
                '\n' => self.line += 1,
                _ => {}
            }
        }
        let end = self.at.min(self.chars.len());
        let literal: String = self.chars[start..end].iter().collect();
        serde_json::from_str::<String>(&literal).unwrap_or(literal)
    }

    fn value(&mut self) -> Option<(usize, String)> {
        self.skip_space();
        match self.chars.get(self.at).copied()? {
            '{' => {
                self.at += 1;
                let mut seen = std::collections::BTreeSet::new();
                loop {
                    self.skip_space();
                    match self.chars.get(self.at).copied()? {
                        '}' => {
                            self.at += 1;
                            return None;
                        }
                        ',' => self.at += 1,
                        '"' => {
                            let line = self.line;
                            let key = self.string();
                            self.skip_space();
                            // The colon.
                            self.at += 1;
                            if !seen.insert(key.clone()) {
                                return Some((line, key));
                            }
                            if let Some(found) = self.value() {
                                return Some(found);
                            }
                        }
                        _ => return None,
                    }
                }
            }
            '[' => {
                self.at += 1;
                loop {
                    self.skip_space();
                    match self.chars.get(self.at).copied()? {
                        ']' => {
                            self.at += 1;
                            return None;
                        }
                        ',' => self.at += 1,
                        _ => {
                            if let Some(found) = self.value() {
                                return Some(found);
                            }
                        }
                    }
                }
            }
            '"' => {
                self.string();
                None
            }
            _ => {
                // A number, `true`, `false` or `null`.
                while let Some(&c) = self.chars.get(self.at) {
                    if matches!(c, ',' | '}' | ']') || c.is_whitespace() {
                        break;
                    }
                    self.at += 1;
                }
                None
            }
        }
    }
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
                        duplicate_key: false,
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
                        duplicate_key: false,
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

    #[test]
    fn a_repeated_key_is_refused_with_its_line() {
        let err = parse(
            "{
  \"roles\": [],
  // c
  \"roles\": []
}",
        )
        .unwrap_err();
        assert!(err.duplicate_key);
        assert_eq!(err.line, 4);
        assert_eq!(err.code("x"), DUPLICATE_KEY);
    }

    #[test]
    fn a_repeated_key_is_found_in_a_nested_object_and_through_escapes() {
        let err = parse("{\"a\": [{\"b\": 1, \"\\u0062\": 2}]}").unwrap_err();
        assert!(err.duplicate_key);
        assert!(
            parse("{\"a\": {\"b\": 1}, \"c\": {\"b\": 2}, \"d\": [{\"b\": 1}, {\"b\": 2}]}")
                .is_ok()
        );
    }
}
