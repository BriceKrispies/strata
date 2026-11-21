//! Word-level helpers of the requirement lint ([`crate::lint`]). Offsets are byte offsets into the
//! string given, which is never altered in length.

/// `s` with every backticked span (backticks included) replaced by spaces of equal length.
pub fn mask_code(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut inside = false;
    for c in s.chars() {
        if c == '`' {
            inside = !inside;
            out.push(' ');
        } else if inside {
            out.extend(std::iter::repeat_n(' ', c.len_utf8()));
        } else {
            out.push(c);
        }
    }
    out
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Offsets of whole-word, case-insensitive occurrences of `phrase` (which may contain spaces and
/// hyphens) in `s`.
pub fn find_word(s: &str, phrase: &str) -> Vec<usize> {
    let hay = s.to_lowercase();
    let needle = phrase.to_lowercase();
    // Lowercasing can change byte lengths for exotic characters; match nothing rather than
    // report a wrong offset.
    if hay.len() != s.len() || needle.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = hay[from..].find(&needle) {
        let at = from + i;
        let end = at + needle.len();
        let before = hay[..at].chars().next_back();
        let after = hay[end..].chars().next();
        if !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char) {
            out.push(at);
        }
        from = at + needle.len().max(1);
    }
    out
}

/// Lowercase alphanumeric tokens with their byte offsets.
pub fn tokens(s: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in s.char_indices() {
        if is_word_char(c) {
            start.get_or_insert(i);
        } else if let Some(st) = start.take() {
            out.push((st, s[st..i].to_lowercase()));
        }
    }
    if let Some(st) = start {
        out.push((st, s[st..].to_lowercase()));
    }
    out
}

/// Whether `s` has `no` followed by `ever` in the same sentence, with at most four words between
/// them (`no request ever`, `no user ever sees`). Returns the byte offset of `no`.
pub fn find_no_ever(s: &str) -> Option<usize> {
    for off in find_word(s, "no") {
        let sentence = s[off..].split(['.', ';', '!', '?']).next().unwrap_or("");
        let words: Vec<String> = tokens(sentence).into_iter().map(|(_, w)| w).collect();
        if words.iter().skip(1).take(5).any(|w| w == "ever") {
            return Some(off);
        }
    }
    None
}

const TIME_UNITS: &[&str] = &[
    "ns",
    "us",
    "\u{b5}s",
    "ms",
    "s",
    "sec",
    "secs",
    "second",
    "seconds",
    "min",
    "mins",
    "minute",
    "minutes",
    "hour",
    "hours",
    "day",
    "days",
    "nanosecond",
    "nanoseconds",
    "microsecond",
    "microseconds",
    "millisecond",
    "milliseconds",
];
/// Number words. `a` and `an` are not among them: "a second copy" is not a limit.
const NUMBER_WORDS: &[&str] = &[
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
    "twenty",
    "thirty",
    "forty",
    "fifty",
    "sixty",
    "seventy",
    "eighty",
    "ninety",
    "hundred",
    "thousand",
    "million",
    "billion",
];
/// The first performance bound in `s` (already code-masked): a bound on how the system performs that
/// has to be measured rather than shown at one input. That is a percentile (`99th percentile`,
/// `p99`), a time an operation takes (`within 5 seconds`, `within five milliseconds`) or a rate
/// (`100 requests per second`). An enforced limit (`refuses more than 200 ids`), a retention period
/// or an expiry time is not one: an example at the bound shows it exactly. Returns the byte offset
/// and the phrase.
pub fn find_performance_bound(s: &str) -> Option<(usize, String)> {
    let toks = tokens(s);
    let is_number = |w: &str| w.chars().all(|c| c.is_ascii_digit()) || NUMBER_WORDS.contains(&w);
    for (k, (off, w)) in toks.iter().enumerate() {
        if w == "percentile" {
            return Some((*off, w.clone()));
        }
        if w.len() >= 3 && w.starts_with('p') && w[1..].chars().all(|c| c.is_ascii_digit()) {
            return Some((*off, w.clone()));
        }
        if w == "within" {
            if let (Some((_, n)), Some((_, u))) = (toks.get(k + 1), toks.get(k + 2)) {
                if is_number(n) && TIME_UNITS.contains(&u.as_str()) {
                    return Some((*off, format!("within {n} {u}")));
                }
            }
        }
        if w == "per" {
            if let Some((_, u)) = toks.get(k + 1) {
                let rate_unit = matches!(u.as_str(), "second" | "minute" | "hour");
                let counted = toks[k.saturating_sub(3)..k]
                    .iter()
                    .any(|(_, n)| is_number(n));
                if rate_unit && counted {
                    return Some((*off, format!("per {u}")));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn performance_bounds_are_measured_quantities_not_enforced_limits() {
        let b = |s: &str| find_performance_bound(s).map(|l| l.1);
        assert_eq!(
            b("SHALL answer within 5 seconds"),
            Some("within 5 seconds".into())
        );
        assert_eq!(b("at the 99th percentile"), Some("percentile".into()));
        assert_eq!(
            b("serve 100 requests per second"),
            Some("per second".into())
        );
        assert_eq!(b("SHALL refuse a read that names more than 200 ids"), None);
        assert_eq!(b("keep an idempotency key for 24 hours"), None);
        assert_eq!(b("within the bounds that the platform defines"), None);
    }

    #[test]
    fn code_spans_are_masked_to_the_same_length() {
        let s = "a `x y` b";
        assert_eq!(mask_code(s).len(), s.len());
        assert!(find_word(&mask_code(s), "x").is_empty());
    }
}
