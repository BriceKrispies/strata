//! The declared types a setting may name (D-5046, D-7901): the id of a declared type of kind id,
//! scalar or enum, beside the types bool, i64, string and timestamp.
//!
//! The kernel holds no registry of the specification's types at run time, and a tenant or a
//! product declares none (a bundle has no section for one). So the registry is this table: the
//! platform's own declared types that a setting can sensibly hold, each checked by the type the
//! specification generates for it. A value of such a type is text ([`Value::Str`]) that decodes as
//! that type: for an enum one of its values, for an id or a scalar a text within the pattern and
//! the length its type states. Nothing is checked beyond the type: that the role, object type or
//! team an id names exists is not, because removing it later would leave a stored value that is not
//! of its type, and SET-004 promises that no stored value ever is.
//!
//! A type is named by its id in the specification (`notification-mode`, `role-id`). A name outside
//! the table is refused with the names the table holds.

use crate::types::{Value, ValueExt};
use serde::de::DeserializeOwned;
use strata_model_core::types::{
    AggregateKind, CapabilityName, DigestPeriod, DisplayName, Domain, EmailAddress, LedgerOrder,
    LedgerPeriod, LedgerPoolName, LedgerUnit, MessageCategoryId, MessageCategoryPurpose,
    NotificationChannel, NotificationMode, ObjectAction, ObjectTypeId, PersonalKind, PrincipalId,
    ProductId, RegistrationMode, RoleId, SendChannel, SettingId, TeamId, TemplateId, Url,
    WorkflowId,
};

/// The kind of a declared type (`type.kind` in the specification).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeKind {
    Enum,
    Id,
    Scalar,
}

impl TypeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enum => "enum",
            Self::Id => "id",
            Self::Scalar => "scalar",
        }
    }
}

/// One declared type a setting may name.
#[derive(Clone, Copy)]
pub struct DeclaredType {
    pub name: &'static str,
    pub kind: TypeKind,
    holds: fn(&str) -> bool,
}

impl DeclaredType {
    /// Whether the text is a value of the type.
    pub fn holds(&self, text: &str) -> bool {
        (self.holds)(text)
    }
}

/// Whether the text decodes as the generated type `T`, with the checks its decoding makes.
fn decodes<T: DeserializeOwned>(text: &str) -> bool {
    serde_json::from_value::<T>(serde_json::Value::String(text.to_string())).is_ok()
}

macro_rules! declared {
    ($kind:ident, $name:literal, $ty:ty) => {
        DeclaredType {
            name: $name,
            kind: TypeKind::$kind,
            holds: decodes::<$ty>,
        }
    };
}

/// The declared types, in name order.
pub const TYPES: &[DeclaredType] = &[
    declared!(Enum, "aggregate-kind", AggregateKind),
    declared!(Id, "capability-name", CapabilityName),
    declared!(Enum, "digest-period", DigestPeriod),
    declared!(Scalar, "display-name", DisplayName),
    declared!(Scalar, "domain", Domain),
    declared!(Scalar, "email-address", EmailAddress),
    declared!(Enum, "ledger-order", LedgerOrder),
    declared!(Enum, "ledger-period", LedgerPeriod),
    declared!(Id, "ledger-pool-name", LedgerPoolName),
    declared!(Id, "ledger-unit", LedgerUnit),
    declared!(Id, "message-category-id", MessageCategoryId),
    declared!(Enum, "message-category-purpose", MessageCategoryPurpose),
    declared!(Enum, "notification-channel", NotificationChannel),
    declared!(Enum, "notification-mode", NotificationMode),
    declared!(Enum, "object-action", ObjectAction),
    declared!(Id, "object-type-id", ObjectTypeId),
    declared!(Enum, "personal-kind", PersonalKind),
    declared!(Id, "principal-id", PrincipalId),
    declared!(Id, "product-id", ProductId),
    declared!(Enum, "registration-mode", RegistrationMode),
    declared!(Id, "role-id", RoleId),
    declared!(Enum, "send-channel", SendChannel),
    declared!(Id, "setting-id", SettingId),
    declared!(Id, "team-id", TeamId),
    declared!(Id, "template-id", TemplateId),
    declared!(Scalar, "url", Url),
    declared!(Id, "workflow-id", WorkflowId),
];

/// The declared type of a name, if the table holds it.
pub fn lookup(name: &str) -> Option<&'static DeclaredType> {
    TYPES.iter().find(|ty| ty.name == name)
}

/// The names the table holds, for a message.
pub fn names() -> String {
    TYPES
        .iter()
        .map(|ty| ty.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Why a value is not a value of the declared type `name`: `None` when it is, or when `name` is no
/// declared type (the caller refuses the name).
pub fn value_problem(ty: &DeclaredType, value: &Value) -> Option<String> {
    match value {
        Value::Str(text) if ty.holds(text) => None,
        Value::Str(text) => Some(format!(
            "{text:?} is not a value of the {} type {}",
            ty.kind.as_str(),
            ty.name
        )),
        other => Some(format!(
            "a {} is not a value of the {} type {}, which takes text",
            other.kind(),
            ty.kind.as_str(),
            ty.name
        )),
    }
}
