//! Identifier newtypes. Each kind of identifier is its own type so they cannot be mixed.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Why a string was refused as an identifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdError {
    pub kind: &'static str,
    pub value: String,
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid {} identifier: {:?}", self.kind, self.value)
    }
}

fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && s.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 96
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '.'))
}

macro_rules! string_id {
    ($(#[$doc:meta])* $name:ident, $kind:literal, $check:ident) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, IdError> {
                let value = value.into();
                if $check(&value) {
                    Ok(Self(value))
                } else {
                    Err(IdError { kind: $kind, value })
                }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdError;
            fn try_from(value: String) -> Result<Self, IdError> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> String {
                id.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_id!(
    /// A tenant. Lower-case name: `[a-z][a-z0-9_]*`.
    TenantId, "tenant", is_name
);
string_id!(
    /// An object type within a tenant, such as `post`.
    ObjectTypeId, "object type", is_name
);
string_id!(
    /// A field of an object type.
    FieldId, "field", is_name
);
string_id!(RoleId, "role", is_name);
string_id!(PolicyId, "policy", is_name);
string_id!(ConstraintId, "constraint", is_name);
string_id!(WorkflowId, "workflow", is_name);
string_id!(QueryId, "query", is_name);
string_id!(ViewId, "view", is_name);
string_id!(
    /// An authenticated identity. Assigned by the shell, opaque to the kernel.
    PrincipalId, "principal", is_token
);
string_id!(
    /// The public identifier of a session. Never the secret token.
    SessionId, "session", is_token
);

/// A record. Allocated from the tenant's counter, so creation is deterministic.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct RecordId(pub u64);

impl fmt::Display for RecordId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Version of a tenant's schema. Rises by one per accepted schema change.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct SchemaVersion(pub u64);

/// Revision of a tenant's logical state. Rises by one per accepted mutating transition.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct StateRevision(pub u64);

/// Milliseconds since the Unix epoch. Always supplied by the caller.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Timestamp(pub i64);

/// The built-in role every principal holds, including anonymous ones.
pub fn public_role() -> RoleId {
    RoleId("public".to_string())
}

/// The built-in role that may do anything inside its tenant.
pub fn owner_role() -> RoleId {
    RoleId("owner".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_validated_on_construction_and_on_deserialisation() {
        assert!(ObjectTypeId::new("post").is_ok());
        assert!(ObjectTypeId::new("Post").is_err());
        assert!(ObjectTypeId::new("").is_err());
        assert!(ObjectTypeId::new("a-b").is_err());
        assert!(serde_json::from_str::<FieldId>("\"1x\"").is_err());
        assert_eq!(serde_json::from_str::<FieldId>("\"title\"").unwrap().as_str(), "title");
        assert!(PrincipalId::new("user:ines").is_ok());
        assert!(PrincipalId::new("has space").is_err());
    }
}
