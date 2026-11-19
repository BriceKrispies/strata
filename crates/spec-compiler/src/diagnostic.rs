//! Refusal diagnostics: why specc will not generate code from a specification.

use std::fmt;

/// The entity name used for problems that belong to the whole specification or to the
/// command, not to one entity.
pub const SPEC_ENTITY: &str = "<spec>";

/// One reason to refuse, printed as `error[<code>] <entity id>: <message>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Diagnostic {
    /// A stable code such as `ready.field-type-undeclared` or `specc.name-collision`.
    pub code: String,
    /// The id of the entity concerned, or [`SPEC_ENTITY`].
    pub entity: String,
    /// What is wrong, in a sentence.
    pub message: String,
}

impl Diagnostic {
    /// A diagnostic with the given code, entity id and message.
    pub fn new(code: &str, entity: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            entity: entity.to_string(),
            message: message.into(),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error[{}] {}: {}", self.code, self.entity, self.message)
    }
}

/// Removes repeated diagnostics, keeping the first of each in its place.
pub fn dedup(diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let mut seen = std::collections::BTreeSet::new();
    diagnostics
        .into_iter()
        .filter(|d| seen.insert(d.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_in_the_refusal_format() {
        let d = Diagnostic::new("ready.schema-version-missing", "rec-a", "No version.");
        assert_eq!(
            d.to_string(),
            "error[ready.schema-version-missing] rec-a: No version."
        );
    }

    #[test]
    fn dedup_keeps_order_and_first_occurrence() {
        let a = Diagnostic::new("x", "a", "m");
        let b = Diagnostic::new("y", "b", "m");
        let out = dedup(vec![b.clone(), a.clone(), b.clone()]);
        assert_eq!(out, vec![b, a]);
    }
}
