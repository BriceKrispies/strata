//! The generated crate's `prelude` module: the representation types every backend names and the
//! descriptions later backends (storage, migrations, transport) read. It does not depend on the
//! specification, so it is the same in every generated crate.

use proc_macro2::TokenStream;
use quote::quote;

/// The module name.
pub const MODULE: &str = "prelude";

/// The names [`representation`] declares, which a model crate with a pure crate beside it
/// re-exports from there instead.
const REPRESENTATION_NAMES: &[&str] = &[
    "SAFE_INTEGER",
    "safe_u64",
    "safe_i64",
    "safe_opt_u64",
    "safe_opt_i64",
    "Timestamp",
    "DurationMs",
];

/// The module's source. With `core_crate` (the pure crate's Rust name), the representation types
/// are re-exported from that crate's prelude, so both crates name the same types.
pub fn emit_with(core_crate: Option<&str>) -> TokenStream {
    let representation = match core_crate {
        None => representation(),
        Some(krate) => {
            let krate = proc_macro2::Ident::new(krate, proc_macro2::Span::call_site());
            let names: Vec<proc_macro2::Ident> = REPRESENTATION_NAMES
                .iter()
                .map(|n| proc_macro2::Ident::new(n, proc_macro2::Span::call_site()))
                .collect();
            quote! {
                #[doc = " The representation types, from the pure crate."]
                pub use #krate::prelude::{#(#names),*};
            }
        }
    };
    let descriptions = descriptions();
    quote! {
        #![doc = " Shared representation types and descriptions used by the other generated modules."]

        use std::fmt;

        #representation

        #descriptions
    }
}

/// The module's source, declaring every type itself.
pub fn emit() -> TokenStream {
    emit_with(None)
}

/// The prelude of the pure crate: the representation types only.
pub fn emit_core() -> TokenStream {
    let representation = representation();
    quote! {
        #![doc = " Shared representation types used by the declared types."]

        #representation
    }
}

/// The representation types: safe-integer decoding, `Timestamp` and `DurationMs`.
fn representation() -> TokenStream {
    quote! {
        use serde::{Deserialize, Serialize};

        /// The largest integer a double holds exactly together with every smaller one, `2^53 - 1`:
        /// the bound of `u64` and `i64` values in the simulator, in Rust and in JavaScript (Exegra
        /// `docs/semantics.md`, "Numbers").
        pub const SAFE_INTEGER: i64 = 9_007_199_254_740_991;

        /// Decodes a `u64` of an event payload, refusing one beyond [`SAFE_INTEGER`].
        pub fn safe_u64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
            let v = u64::deserialize(d)?;
            if v <= SAFE_INTEGER as u64 {
                Ok(v)
            } else {
                Err(serde::de::Error::custom(format!("{v} is beyond 2^53 - 1")))
            }
        }

        /// Decodes an `i64` of an event payload, refusing one beyond +-[`SAFE_INTEGER`].
        pub fn safe_i64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
            let v = i64::deserialize(d)?;
            if (-SAFE_INTEGER..=SAFE_INTEGER).contains(&v) {
                Ok(v)
            } else {
                Err(serde::de::Error::custom(format!("{v} is beyond +-(2^53 - 1)")))
            }
        }

        /// As [`safe_u64`], for an optional field.
        pub fn safe_opt_u64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
            match Option::<u64>::deserialize(d)? {
                Some(v) if v > SAFE_INTEGER as u64 => {
                    Err(serde::de::Error::custom(format!("{v} is beyond 2^53 - 1")))
                }
                v => Ok(v),
            }
        }

        /// As [`safe_i64`], for an optional field.
        pub fn safe_opt_i64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
            match Option::<i64>::deserialize(d)? {
                Some(v) if !(-SAFE_INTEGER..=SAFE_INTEGER).contains(&v) => {
                    Err(serde::de::Error::custom(format!("{v} is beyond +-(2^53 - 1)")))
                }
                v => Ok(v),
            }
        }

        /// A point in time: milliseconds since the Unix epoch, UTC.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct Timestamp(pub i64);

        /// A length of time in milliseconds.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct DurationMs(pub u64);
    }
}

/// The descriptions later backends read: references, indexes, constraints, contract metadata.
fn descriptions() -> TokenStream {
    quote! {
        /// What deleting a referred record does to a record that refers to it.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum OnDelete {
            /// Deleting the referred record is refused while this record refers to it.
            Restrict,
            /// This record is deleted with the referred record.
            Cascade,
            /// The referring field is cleared.
            SetNull,
        }

        impl OnDelete {
            /// The value as the specification writes it.
            pub fn as_str(&self) -> &'static str {
                match self {
                    Self::Restrict => "restrict",
                    Self::Cascade => "cascade",
                    Self::SetNull => "set-null",
                }
            }
        }

        /// A record field that holds the key of another record.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct Reference {
            /// The referring field, by its specification name.
            pub field: &'static str,
            /// The id of the referred record, or `principal` for a principal id.
            pub record: &'static str,
            /// What deleting the referred record does.
            pub on_delete: OnDelete,
        }

        /// A named index of a record.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct Index {
            /// The index name.
            pub name: &'static str,
            /// The indexed fields, by their specification names, in order.
            pub fields: &'static [&'static str],
        }

        /// A kind of record constraint.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Constraint {
            /// The value is at least the stated minimum.
            Min,
            /// The value is at most the stated maximum.
            Max,
            /// The text has at most the stated number of characters.
            MaxLength,
            /// The text matches the stated pattern.
            Pattern,
        }

        impl Constraint {
            /// The constraint as the specification writes it.
            pub fn as_str(&self) -> &'static str {
                match self {
                    Self::Min => "min",
                    Self::Max => "max",
                    Self::MaxLength => "maxLength",
                    Self::Pattern => "pattern",
                }
            }
        }

        /// A record value that breaks one of its record's constraints.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct ConstraintViolation {
            /// The record id.
            pub record: &'static str,
            /// The field, by its specification name.
            pub field: &'static str,
            /// The constraint it breaks.
            pub constraint: Constraint,
        }

        impl fmt::Display for ConstraintViolation {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(
                    f,
                    "field {} of record {} breaks its {} constraint",
                    self.field,
                    self.record,
                    self.constraint.as_str()
                )
            }
        }

        impl std::error::Error for ConstraintViolation {}

        /// How a contract is realised.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Realisation {
            /// The code is generated from the contract.
            Generated,
            /// A hole: the code is written by hand against the contract's postconditions.
            Hand,
            /// The code is generated from the contract's derivation: functions, and the
            /// contracts it loads and commits through (module `derived`).
            Derived,
            /// No server implementation: a client lowers the contract onto the generic
            /// contract its route names. It has no trait method and no route.
            Facade,
        }

        /// What the specification states about a contract beyond its data.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct ContractMeta {
            /// The contract id.
            pub id: &'static str,
            /// The contract name.
            pub name: &'static str,
            /// The stable code, such as `CT-012`, or `H-012` for a hole.
            pub reference: Option<&'static str>,
            /// Whether the implementation is generated or hand-written.
            pub realisation: Realisation,
            /// The id of the implementing component.
            pub implemented_by: Option<&'static str>,
            /// Who may call it, in the specification's words.
            pub auth: Option<&'static str>,
            /// Whether repeating a call has the effect of making it once.
            pub idempotent: Option<bool>,
            /// How long a caller waits, in milliseconds.
            pub timeout_ms: Option<u64>,
        }
    }
}
