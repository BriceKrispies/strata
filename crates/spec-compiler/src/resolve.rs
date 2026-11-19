//! Type names and what they resolve to.
//!
//! A type name resolves when it is a built-in representation type, the id of a declared `type`,
//! or `list<T>` / `option<T>` of a name that resolves (the shared generation-readiness format).

use std::collections::BTreeSet;

use crate::ir::{Spec, TypeKind};

/// The built-in representation types, in the format's order.
pub const BUILTIN_TYPES: &[&str] = &[
    "bool",
    "u8",
    "u16",
    "u32",
    "u64",
    "i32",
    "i64",
    "f64",
    "string",
    "bytes",
    "timestamp",
    "duration",
    "uuid",
    "json",
];

/// A built-in representation type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    /// `bool`
    Bool,
    /// `u8`
    U8,
    /// `u16`
    U16,
    /// `u32`
    U32,
    /// `u64`
    U64,
    /// `i32`
    I32,
    /// `i64`
    I64,
    /// `f64`
    F64,
    /// `string`
    String,
    /// `bytes`
    Bytes,
    /// `timestamp`: milliseconds since the Unix epoch, UTC, as an `i64`.
    Timestamp,
    /// `duration`: milliseconds, as a `u64`.
    Duration,
    /// `uuid`
    Uuid,
    /// `json`: any JSON value, as a `serde_json::Value`.
    Json,
}

impl Builtin {
    /// The built-in type with this name.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "bool" => Self::Bool,
            "u8" => Self::U8,
            "u16" => Self::U16,
            "u32" => Self::U32,
            "u64" => Self::U64,
            "i32" => Self::I32,
            "i64" => Self::I64,
            "f64" => Self::F64,
            "string" => Self::String,
            "bytes" => Self::Bytes,
            "timestamp" => Self::Timestamp,
            "duration" => Self::Duration,
            "uuid" => Self::Uuid,
            "json" => Self::Json,
            _ => return None,
        })
    }

    /// The name as the specification writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::F64 => "f64",
            Self::String => "string",
            Self::Bytes => "bytes",
            Self::Timestamp => "timestamp",
            Self::Duration => "duration",
            Self::Uuid => "uuid",
            Self::Json => "json",
        }
    }

    /// The range of an integer representation (timestamps and durations count as their
    /// underlying `i64` and `u64`).
    pub fn integer_range(self) -> Option<(i128, i128)> {
        Some(match self {
            Self::U8 => (0, i128::from(u8::MAX)),
            Self::U16 => (0, i128::from(u16::MAX)),
            Self::U32 => (0, i128::from(u32::MAX)),
            Self::U64 | Self::Duration => (0, i128::from(u64::MAX)),
            Self::I32 => (i128::from(i32::MIN), i128::from(i32::MAX)),
            Self::I64 | Self::Timestamp => (i128::from(i64::MIN), i128::from(i64::MAX)),
            _ => return None,
        })
    }

    /// True when the generated Rust type is `Copy`.
    pub fn is_copy(self) -> bool {
        !matches!(self, Self::String | Self::Bytes | Self::Json)
    }

    /// True when the generated Rust type is `Eq`, `Ord` and `Hash` (`serde_json::Value` is
    /// neither ordered nor hashable).
    pub fn is_total(self) -> bool {
        !matches!(self, Self::F64 | Self::Json)
    }
}

/// A parsed type name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeRef {
    /// A built-in representation type.
    Builtin(Builtin),
    /// Any other name: a declared type when it resolves.
    Declared(String),
    /// `list<T>`
    List(Box<TypeRef>),
    /// `option<T>`
    Option(Box<TypeRef>),
}

impl TypeRef {
    /// Parses a type name. Every text parses; whether it resolves is [`resolves`].
    pub fn parse(text: &str) -> Self {
        let text = text.trim();
        let wrapped = |prefix: &str| {
            text.strip_prefix(prefix)
                .and_then(|t| t.strip_suffix('>'))
                .filter(|inner| !inner.is_empty())
                .map(|inner| Box::new(TypeRef::parse(inner)))
        };
        if let Some(inner) = wrapped("list<") {
            return TypeRef::List(inner);
        }
        if let Some(inner) = wrapped("option<") {
            return TypeRef::Option(inner);
        }
        match Builtin::parse(text) {
            Some(b) => TypeRef::Builtin(b),
            None => TypeRef::Declared(text.to_string()),
        }
    }
}

/// True when the type name resolves in the specification.
pub fn resolves(spec: &Spec, name: &str) -> bool {
    ref_resolves(spec, &TypeRef::parse(name))
}

/// True when every declared name inside the type is a declared type.
pub fn ref_resolves(spec: &Spec, t: &TypeRef) -> bool {
    match t {
        TypeRef::Builtin(_) => true,
        TypeRef::Declared(id) => spec.type_decl(id).is_some(),
        TypeRef::List(inner) | TypeRef::Option(inner) => ref_resolves(spec, inner),
    }
}

/// How a diagnostic describes a type that does not resolve.
pub fn unresolved_text(name: Option<&str>) -> String {
    match name {
        None => "no type".to_string(),
        Some(n) => format!("type \"{n}\", which is neither built-in nor a declared type"),
    }
}

/// True when the generated Rust type can derive `Eq`, `Ord` and `Hash`: nothing in it is an
/// `f64`. A type that does not resolve counts as not total.
pub fn is_total(spec: &Spec, t: &TypeRef) -> bool {
    total(spec, t, &mut BTreeSet::new())
}

fn total(spec: &Spec, t: &TypeRef, visiting: &mut BTreeSet<String>) -> bool {
    match t {
        TypeRef::Builtin(b) => b.is_total(),
        TypeRef::List(inner) | TypeRef::Option(inner) => total(spec, inner, visiting),
        TypeRef::Declared(id) => {
            let Some(decl) = spec.type_decl(id) else {
                return false;
            };
            if !visiting.insert(id.clone()) {
                // A recursive occurrence is total exactly when the rest of the type is.
                return true;
            }
            let result = match &decl.kind {
                TypeKind::Id { repr } | TypeKind::Scalar { repr } => repr
                    .as_deref()
                    .and_then(Builtin::parse)
                    .is_some_and(Builtin::is_total),
                TypeKind::Enum { .. } => true,
                TypeKind::Struct { fields } => fields
                    .iter()
                    .all(|f| total(spec, &TypeRef::parse(&f.type_name), visiting)),
            };
            visiting.remove(id);
            result
        }
    }
}

/// True when the generated Rust type is `Copy`. Declared types never derive `Copy`.
pub fn is_copy(t: &TypeRef) -> bool {
    match t {
        TypeRef::Builtin(b) => b.is_copy(),
        TypeRef::Option(inner) => is_copy(inner),
        TypeRef::Declared(_) | TypeRef::List(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{StructField, System, TypeDecl};

    fn spec_with(types: Vec<TypeDecl>) -> Spec {
        Spec {
            path: "s.jsonc".into(),
            system: System {
                id: "s".into(),
                name: "S".into(),
                generation_target: None,
                wire_case: Default::default(),
                error_wire: Default::default(),
            },
            components: vec![],
            types,
            records: vec![],
            contracts: vec![],
            events: vec![],
            machines: vec![],
            memory_machines: vec![],
            target_machines: vec![],
            constructs: Default::default(),
            telemetry: Default::default(),
        }
    }

    fn decl(id: &str, kind: TypeKind) -> TypeDecl {
        TypeDecl {
            id: id.into(),
            name: id.into(),
            description: None,
            kind,
        }
    }

    fn field(name: &str, type_name: &str) -> StructField {
        StructField {
            name: name.into(),
            type_name: type_name.into(),
            optional: false,
        }
    }

    #[test]
    fn builtins_match_the_format_list() {
        for name in BUILTIN_TYPES {
            assert_eq!(Builtin::parse(name).map(Builtin::as_str), Some(*name));
        }
        assert_eq!(Builtin::parse("string[]"), None);
    }

    #[test]
    fn wrappers_parse_recursively() {
        assert_eq!(
            TypeRef::parse(" list<option<u8>> "),
            TypeRef::List(Box::new(TypeRef::Option(Box::new(TypeRef::Builtin(
                Builtin::U8
            )))))
        );
        assert_eq!(TypeRef::parse("list<>"), TypeRef::Declared("list<>".into()));
    }

    #[test]
    fn declared_names_resolve_only_when_declared() {
        let spec = spec_with(vec![decl(
            "tenant-id",
            TypeKind::Id {
                repr: Some("string".into()),
            },
        )]);
        assert!(resolves(&spec, "tenant-id"));
        assert!(resolves(&spec, "list<tenant-id>"));
        assert!(resolves(&spec, "option<list<uuid>>"));
        assert!(!resolves(&spec, "user-id"));
        assert!(!resolves(&spec, "list<user-id>"));
        assert!(!resolves(&spec, "Operation[]"));
    }

    #[test]
    fn totality_follows_f64_through_declarations() {
        let spec = spec_with(vec![
            decl(
                "score",
                TypeKind::Scalar {
                    repr: Some("f64".into()),
                },
            ),
            decl(
                "result",
                TypeKind::Struct {
                    fields: vec![field("s", "option<score>")],
                },
            ),
            decl(
                "tree",
                TypeKind::Struct {
                    fields: vec![field("children", "list<tree>"), field("n", "u32")],
                },
            ),
        ]);
        assert!(!is_total(&spec, &TypeRef::parse("score")));
        assert!(!is_total(&spec, &TypeRef::parse("list<result>")));
        assert!(is_total(&spec, &TypeRef::parse("tree")));
        assert!(is_total(&spec, &TypeRef::parse("timestamp")));
        assert!(!is_total(&spec, &TypeRef::parse("unknown")));
    }

    #[test]
    fn copy_types_are_scalars_and_their_options() {
        assert!(is_copy(&TypeRef::parse("option<u64>")));
        assert!(is_copy(&TypeRef::parse("uuid")));
        assert!(!is_copy(&TypeRef::parse("string")));
        assert!(!is_copy(&TypeRef::parse("list<u8>")));
        assert!(!is_copy(&TypeRef::parse("tenant-id")));
    }
}
