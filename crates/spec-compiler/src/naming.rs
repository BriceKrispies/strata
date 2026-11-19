//! Rust names for specification names.
//!
//! Entity ids are kebab-case (`tenant-id`); field and value names may be kebab-case, snake_case,
//! camelCase or words with spaces. A name is split into lower-case words at every character
//! that is not an ASCII letter or digit and at every change from lower to upper case; the words
//! are then joined as PascalCase for types and variants, snake_case for fields, functions and
//! modules, and SCREAMING_SNAKE_CASE for constants. Rust keywords are written as raw identifiers
//! (`r#type`); the four that cannot be raw (`self`, `Self`, `super`, `crate`) get a trailing `_`.

use proc_macro2::{Ident, Span};

/// Keywords that a raw identifier can stand for.
const RAW_KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "do", "dyn",
    "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl", "in", "let",
    "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref", "return",
    "static", "struct", "trait", "true", "try", "type", "typeof", "unsafe", "unsized", "use",
    "virtual", "where", "while", "yield",
];

/// Keywords that cannot be raw identifiers; a trailing `_` is added instead.
const UNRAWABLE: &[&str] = &["self", "Self", "super", "crate"];

/// Type names the generated modules use unqualified (the standard prelude, the derives and the
/// imports of the generated files). A declared type, record or contract that maps to one of
/// them would shadow it, so it is refused.
pub const RESERVED_TYPE_NAMES: &[&str] = &[
    "Box",
    "Clone",
    "Copy",
    "Debug",
    "Default",
    "Deserialize",
    "Eq",
    "Err",
    "Hash",
    "LazyLock",
    "None",
    "Ok",
    "Option",
    "Ord",
    "PartialEq",
    "PartialOrd",
    "Regex",
    "Result",
    "Send",
    "Serialize",
    "Sized",
    "Some",
    "String",
    "Sync",
    "Vec",
];

/// Splits a name into lower-case ASCII words. `Err` names the first character that is neither
/// an ASCII letter, an ASCII digit, nor an ASCII separator.
pub fn words(name: &str) -> Result<Vec<String>, String> {
    if let Some(c) = name.chars().find(|c| !c.is_ascii()) {
        return Err(format!(
            "\"{name}\" contains the character '{c}', which is not ASCII"
        ));
    }
    let mut out = Vec::new();
    for run in name.split(|c: char| !c.is_ascii_alphanumeric()) {
        let chars: Vec<char> = run.chars().collect();
        let mut word = String::new();
        for (i, &c) in chars.iter().enumerate() {
            if i > 0 {
                let prev = chars[i - 1];
                let next = chars.get(i + 1).copied();
                let lower_to_upper =
                    (prev.is_ascii_lowercase() || prev.is_ascii_digit()) && c.is_ascii_uppercase();
                let acronym_end = prev.is_ascii_uppercase()
                    && c.is_ascii_uppercase()
                    && next.is_some_and(|n| n.is_ascii_lowercase());
                if (lower_to_upper || acronym_end) && !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
            }
            word.push(c.to_ascii_lowercase());
        }
        if !word.is_empty() {
            out.push(word);
        }
    }
    if out.is_empty() {
        return Err(format!("\"{name}\" has no letters or digits"));
    }
    if out[0].starts_with(|c: char| c.is_ascii_digit()) {
        return Err(format!(
            "\"{name}\" starts with a digit, so it cannot name a Rust item"
        ));
    }
    Ok(out)
}

/// The PascalCase name of a type, variant or trait.
pub fn pascal(name: &str) -> Result<String, String> {
    let joined: String = words(name)?
        .iter()
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect();
    Ok(escape_unrawable(joined))
}

/// The snake_case name of a field, function or module.
pub fn snake(name: &str) -> Result<String, String> {
    Ok(escape_unrawable(words(name)?.join("_")))
}

/// The SCREAMING_SNAKE_CASE name of a constant or static.
pub fn upper_snake(name: &str) -> Result<String, String> {
    Ok(words(name)?.join("_").to_ascii_uppercase())
}

fn escape_unrawable(name: String) -> String {
    if UNRAWABLE.contains(&name.as_str()) {
        name + "_"
    } else {
        name
    }
}

/// True when `name` is a Rust keyword written as a raw identifier.
pub fn is_raw_keyword(name: &str) -> bool {
    RAW_KEYWORDS.contains(&name)
}

/// True when `name` is a plain identifier: an ASCII letter or `_` followed by letters, digits
/// and `_`, and not `_` alone.
fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    name != "_" && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The identifier for a name produced by [`pascal`], [`snake`] or [`upper_snake`], raw when it
/// is a keyword. Names that were not checked first fall back to `invalid_name` instead of
/// panicking; the readiness checks refuse every name that would need the fallback.
pub fn ident(name: &str) -> Ident {
    if UNRAWABLE.contains(&name) || !is_identifier(name) {
        return Ident::new("invalid_name", Span::call_site());
    }
    if is_raw_keyword(name) {
        Ident::new_raw(name, Span::call_site())
    } else {
        Ident::new(name, Span::call_site())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kebab_ids_map_to_each_case() {
        assert_eq!(pascal("tenant-id").unwrap(), "TenantId");
        assert_eq!(snake("tenant-id").unwrap(), "tenant_id");
        assert_eq!(upper_snake("tenant-id").unwrap(), "TENANT_ID");
    }

    #[test]
    fn camel_case_and_acronyms_split_into_words() {
        assert_eq!(snake("tenantId").unwrap(), "tenant_id");
        assert_eq!(pascal("HTTPRequest").unwrap(), "HttpRequest");
        assert_eq!(snake("parseURL").unwrap(), "parse_url");
    }

    #[test]
    fn digits_stay_with_their_word() {
        assert_eq!(pascal("v2-api").unwrap(), "V2Api");
        assert_eq!(snake("tls-1-3").unwrap(), "tls_1_3");
    }

    #[test]
    fn underscores_and_spaces_separate_words() {
        assert_eq!(pascal("set_null").unwrap(), "SetNull");
        assert_eq!(pascal("read committed").unwrap(), "ReadCommitted");
        assert_eq!(pascal("not-found / gone").unwrap(), "NotFoundGone");
    }

    #[test]
    fn leading_digit_is_refused() {
        assert!(pascal("1st").is_err());
        assert!(snake("2-phase").is_err());
    }

    #[test]
    fn empty_and_non_ascii_names_are_refused() {
        assert!(snake("").is_err());
        assert!(snake("--").is_err());
        assert!(snake("caf\u{e9}").is_err());
    }

    #[test]
    fn keywords_become_raw_identifiers() {
        assert_eq!(snake("type").unwrap(), "type");
        assert_eq!(ident("type").to_string(), "r#type");
        assert_eq!(ident("tenant_id").to_string(), "tenant_id");
    }

    #[test]
    fn unrawable_keywords_get_a_trailing_underscore() {
        assert_eq!(snake("self").unwrap(), "self_");
        assert_eq!(pascal("self").unwrap(), "Self_");
        assert_eq!(snake("crate").unwrap(), "crate_");
        assert_eq!(ident("self_").to_string(), "self_");
    }

    #[test]
    fn unchecked_names_fall_back_instead_of_panicking() {
        assert_eq!(ident("1x").to_string(), "invalid_name");
        assert_eq!(ident("a-b").to_string(), "invalid_name");
        assert_eq!(ident("self").to_string(), "invalid_name");
    }
}
