//! The one meaning of a validation on the server and in the page: its pattern, range and list of
//! values.
//!
//! The server matches a pattern as a Rust `regex` expression, anchored as `\A(?:pattern)\z`. The
//! page matches it as a JavaScript regular expression in Unicode mode, anchored as
//! `^(?:pattern)$`. The two dialects agree on a small subset and differ on the rest: what `\s`,
//! `\d`, `\w` and `.` cover, what an escape may name, how a class nests, what a flag does. This
//! module translates the subset on which the meaning can be made identical and refuses every
//! other pattern, so a pattern is never read two ways.
//!
//! The accepted syntax: literal characters, `.`, the escapes `\s \S \d \D \w \W \n \r \t \f \v \a
//! \A \z` and an escaped punctuation character, classes of characters, ranges and those escapes
//! (a negated `\W` only outside a class), groups `( )` and `(?: )`, alternation, the repetitions
//! `* + ? {n} {n,} {n,m}` with their lazy forms, and the anchors `^` and `$`.
//!
//! The cases that pin the agreement live in `tests/fixtures/validation-cases.json`, which a Rust
//! test (against the `regex` crate) and a JavaScript test (against `RegExp`) both read.

use serde_json::{Number, Value};

use crate::web::sq;

/// Rust `\w`: the Unicode word characters.
const WORD: &str = r"\p{Alphabetic}\p{M}\p{Nd}\p{Pc}\p{Join_Control}";

/// The JavaScript source (without the anchors) with the meaning the Rust pattern has, or the
/// reason it cannot be given the same meaning.
///
/// # Errors
/// The pattern uses syntax outside the subset the two engines share.
pub fn to_js(pattern: &str) -> Result<String, String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut in_class = false;
    let mut class_start = false;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        if in_class {
            let first = class_start;
            class_start = false;
            match c {
                ']' if first => return Err("a `]` first in a class".into()),
                ']' => {
                    in_class = false;
                    out.push(']');
                }
                '[' => return Err("a nested class or a POSIX class".into()),
                '&' | '-' | '~' if chars.get(i) == Some(&c) => {
                    return Err(format!("the class operator `{c}{c}`"));
                }
                '\\' => {
                    let e = *chars.get(i).ok_or("a trailing backslash")?;
                    i += 1;
                    match e {
                        's' => out.push_str(r"\p{White_Space}"),
                        'S' => out.push_str(r"\P{White_Space}"),
                        'd' => out.push_str(r"\p{Nd}"),
                        'D' => out.push_str(r"\P{Nd}"),
                        'w' => out.push_str(WORD),
                        'W' => return Err(r"`\W` inside a class".into()),
                        'n' | 'r' | 't' | 'f' | 'v' => {
                            out.push('\\');
                            out.push(e);
                        }
                        'a' => out.push_str(r"\x07"),
                        _ if e.is_ascii_punctuation() => {
                            if "^$\\.*+?()[]{}|/-".contains(e) {
                                out.push('\\');
                            }
                            out.push(e);
                        }
                        _ => return Err(format!("the escape `\\{e}`")),
                    }
                }
                '^' if !first => out.push_str(r"\^"),
                _ => out.push(c),
            }
            continue;
        }
        match c {
            '\\' => {
                let e = *chars.get(i).ok_or("a trailing backslash")?;
                i += 1;
                match e {
                    's' => out.push_str(r"\p{White_Space}"),
                    'S' => out.push_str(r"\P{White_Space}"),
                    'd' => out.push_str(r"\p{Nd}"),
                    'D' => out.push_str(r"\P{Nd}"),
                    'w' => {
                        out.push('[');
                        out.push_str(WORD);
                        out.push(']');
                    }
                    'W' => {
                        out.push_str("[^");
                        out.push_str(WORD);
                        out.push(']');
                    }
                    'A' => out.push('^'),
                    'z' => out.push('$'),
                    'n' | 'r' | 't' | 'f' | 'v' => {
                        out.push('\\');
                        out.push(e);
                    }
                    'a' => out.push_str(r"\x07"),
                    _ if e.is_ascii_punctuation() => {
                        if "^$\\.*+?()[]{}|/".contains(e) {
                            out.push('\\');
                        }
                        out.push(e);
                    }
                    _ => return Err(format!("the escape `\\{e}`")),
                }
            }
            '.' => out.push_str(r"[^\n]"),
            '[' => {
                in_class = true;
                class_start = true;
                out.push('[');
                if chars.get(i) == Some(&'^') {
                    i += 1;
                    out.push('^');
                }
            }
            '(' => {
                if chars.get(i) == Some(&'?') {
                    if chars.get(i + 1) == Some(&':') {
                        i += 2;
                        out.push_str("(?:");
                    } else {
                        return Err(
                            "a group that is not `(?:` (a flag, a name, a look-around)".into()
                        );
                    }
                } else {
                    out.push('(');
                }
            }
            '{' => {
                let close = chars[i..]
                    .iter()
                    .position(|&x| x == '}')
                    .ok_or("a `{` that opens no repetition")?;
                let body: String = chars[i..i + close].iter().collect();
                let (low, high) = body
                    .split_once(',')
                    .map_or((body.as_str(), None), |(l, h)| (l, Some(h)));
                let digits = |s: &str| !s.is_empty() && s.chars().all(|d| d.is_ascii_digit());
                if !digits(low) || high.is_some_and(|h| !h.is_empty() && !digits(h)) {
                    return Err("a `{` that opens no repetition".into());
                }
                out.push('{');
                out.push_str(&body);
                out.push('}');
                i += close + 1;
            }
            '}' => out.push_str(r"\}"),
            ']' => out.push_str(r"\]"),
            ')' | '|' | '*' | '+' | '?' | '^' | '$' => out.push(c),
            _ => out.push(c),
        }
    }
    if in_class {
        return Err("a class that is not closed".into());
    }
    Ok(out)
}

/// The helper the page's range and list checks read a value with, as the JavaScript source of
/// `forms.js`: the number a value states, or NaN when it states none. A text states a number only
/// when the whole of it has the form a Rust `f64` is read from (no hex, no `Infinity`, no blank).
pub const NUMERIC_JS: &str = r"/**
 * A value as the number it states, or NaN when it states none.
 * @param {unknown} value
 * @returns {number}
 */
function numeric(value) {
  if (typeof value === 'number') return value;
  if (typeof value !== 'string') return Number.NaN;
  const text = value.trim();
  return /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/.test(text) ? Number(text) : Number.NaN;
}
";

/// The JavaScript expression that is true when `value` (an expression for the text) does not
/// match the pattern as a whole, as the server matches it.
///
/// # Errors
/// The pattern cannot be given the same meaning in JavaScript; see [`to_js`].
pub fn pattern_fails(pattern: &str, value: &str) -> Result<String, String> {
    let js = to_js(pattern)?;
    Ok(format!(
        "!new RegExp({}, 'u').test(String({value}))",
        sq(&format!("^(?:{js})$"))
    ))
}

/// The expression that is true when `value` is outside the range: not a finite number, below
/// `min` or above `max`. `None` when the range has no bound.
#[must_use]
pub fn range_fails(value: &str, min: Option<&Number>, max: Option<&Number>) -> Option<String> {
    if min.is_none() && max.is_none() {
        return None;
    }
    let mut parts = vec![format!("!Number.isFinite(numeric({value}))")];
    if let Some(min) = min {
        parts.push(format!("numeric({value}) < {min}"));
    }
    if let Some(max) = max {
        parts.push(format!("numeric({value}) > {max}"));
    }
    Some(parts.join(" || "))
}

/// The expression that is true when `value` is none of `values`. Each listed value is compared as
/// its own kind: a number as the number the value states, a string or a flag as text.
#[must_use]
pub fn one_of_fails(value: &str, values: &[Value]) -> Option<String> {
    let items: Vec<String> = values
        .iter()
        .map(|x| match x {
            Value::Number(_) => format!("numeric({value}) === {x}"),
            Value::String(s) => format!("String({value}) === {}", sq(s)),
            other => format!("String({value}) === {}", sq(&other.to_string())),
        })
        .collect();
    (!items.is_empty()).then(|| format!("!({})", items.join(" || ")))
}

/// The pattern as the server matches it: against the whole value.
#[must_use]
pub fn anchored(pattern: &str) -> String {
    format!("\\A(?:{pattern})\\z")
}
