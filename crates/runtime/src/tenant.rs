//! The tenant a machine store belongs to.
//!
//! The runtime depends on no Strata crate, so it names a tenant with a type of its own: the same
//! rule as the kernel's tenant id (`[a-z][a-z0-9_]*`, at most 64 bytes), checked here. A host
//! that holds the kernel's id converts it by its text.

use std::fmt;

/// A tenant's name: a lower-case letter, then lower-case letters, digits and `_`, at most 64
/// bytes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TenantId(String);

/// A name that is not a tenant's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidTenant(pub String);

impl fmt::Display for InvalidTenant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} is not a tenant name ([a-z][a-z0-9_]*, at most 64 bytes)",
            self.0
        )
    }
}

impl std::error::Error for InvalidTenant {}

impl TenantId {
    /// The name, when it meets the rule.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidTenant> {
        let value = value.into();
        let mut chars = value.chars();
        let first = chars.next().is_some_and(|c| c.is_ascii_lowercase());
        let rest = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if first && rest && value.len() <= 64 {
            Ok(TenantId(value))
        } else {
            Err(InvalidTenant(value))
        }
    }

    /// The name as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::TenantId;

    #[test]
    fn the_name_rule_is_the_kernels() {
        for good in ["a", "acme", "outbox_machine", "t2"] {
            assert!(TenantId::new(good).is_ok(), "{good}");
        }
        for bad in ["", "Acme", "2t", "a-b", "_a", &"a".repeat(65)] {
            assert!(TenantId::new(bad).is_err(), "{bad}");
        }
    }
}
