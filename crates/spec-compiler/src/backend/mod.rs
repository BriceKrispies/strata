//! Backends: each turns one part of the specification into one module of the generated crate.
//!
//! A backend first checks that the specification states everything it needs ([`Backend::check`]);
//! specc runs every check before emitting anything, and refuses with all their diagnostics at
//! once. Emission ([`Backend::emit`]) may then assume the checks passed. Adding a backend (the
//! state-machine backend, for instance) is one implementation of [`Backend`] and one entry in
//! [`BACKENDS`]; the crate assembly in `emit.rs` adds its module to `lib.rs` and writes its file.

use proc_macro2::TokenStream;

use crate::diagnostic::Diagnostic;
use crate::ir::Spec;

pub mod audits;
pub mod contracts;
pub mod policies;
pub mod prelude;
pub mod quotas;
pub mod records;
pub mod retentions;
pub mod retries;
pub mod support;
pub mod types;
pub mod validations;
pub mod view;

/// One part of the specification compiled to one module of the generated crate.
pub trait Backend {
    /// The module name; the module is written to `src/<module>.rs`.
    fn module(&self) -> &'static str;

    /// Every diagnostic that would stop this backend from emitting correct code, including
    /// name problems in its module.
    fn check(&self, spec: &Spec) -> Vec<Diagnostic>;

    /// The module's source, starting with its inner doc attributes. Called only when every
    /// backend's check returned nothing.
    fn emit(&self, spec: &Spec) -> TokenStream;
}

/// The backends, in the order their modules appear in `lib.rs`.
pub const BACKENDS: &[&dyn Backend] = &[
    &types::TypesBackend,
    &records::RecordsBackend,
    &contracts::ContractsBackend,
    &support::SupportBackend,
    &policies::PoliciesBackend,
    &validations::ValidationsBackend,
    &retentions::RetentionsBackend,
    &quotas::QuotasBackend,
    &retries::RetriesBackend,
    &audits::AuditsBackend,
];
