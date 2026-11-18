//! The typed value algebra. Records hold these, expressions evaluate to these.

use crate::ids::{ObjectTypeId, RecordId};
use serde::{Deserialize, Serialize};

/// A value. `Null` is the absence of a value; a field is nullable unless it is required.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    /// Milliseconds since the Unix epoch.
    Time(i64),
    Ref(RecordId),
    List(Vec<Value>),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The name of this value's shape, for messages.
    pub fn kind(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Str(_) => "string",
            Value::Time(_) => "timestamp",
            Value::Ref(_) => "reference",
            Value::List(_) => "list",
        }
    }
}

/// What happens to a referencing record when the record it points at is deleted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum OnDelete {
    /// The delete is rejected while a reference exists.
    Restrict,
    /// The referencing field is cleared. Rejected if that field is required.
    SetNull,
}

/// The type of a field.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum FieldType {
    Str,
    Int,
    Bool,
    Time,
    /// A relationship to a record of `target`.
    Ref { target: ObjectTypeId, on_delete: OnDelete },
    /// A list whose elements are non-null values of the inner type.
    List(Box<FieldType>),
}

impl FieldType {
    /// Whether a non-null value has this type. Reference targets are checked against state
    /// separately, because that needs the records.
    pub fn admits(&self, value: &Value) -> bool {
        match (self, value) {
            (FieldType::Str, Value::Str(_))
            | (FieldType::Int, Value::Int(_))
            | (FieldType::Bool, Value::Bool(_))
            | (FieldType::Time, Value::Time(_))
            | (FieldType::Ref { .. }, Value::Ref(_)) => true,
            (FieldType::List(inner), Value::List(items)) => {
                items.iter().all(|item| inner.admits(item))
            }
            _ => false,
        }
    }

    pub fn name(&self) -> String {
        match self {
            FieldType::Str => "string".into(),
            FieldType::Int => "int".into(),
            FieldType::Bool => "bool".into(),
            FieldType::Time => "timestamp".into(),
            FieldType::Ref { target, .. } => format!("reference<{target}>"),
            FieldType::List(inner) => format!("list<{}>", inner.name()),
        }
    }

    /// The object type this field refers to, directly or as list elements.
    pub fn ref_target(&self) -> Option<(&ObjectTypeId, OnDelete, bool)> {
        match self {
            FieldType::Ref { target, on_delete } => Some((target, *on_delete, false)),
            FieldType::List(inner) => match inner.as_ref() {
                FieldType::Ref { target, on_delete } => Some((target, *on_delete, true)),
                _ => None,
            },
            _ => None,
        }
    }
}

/// The record ids a value refers to.
pub fn refs_in(value: &Value) -> Vec<RecordId> {
    match value {
        Value::Ref(id) => vec![*id],
        Value::List(items) => items.iter().flat_map(refs_in).collect(),
        _ => Vec::new(),
    }
}
