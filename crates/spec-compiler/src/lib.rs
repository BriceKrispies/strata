//! specc: compiles an Exegra specification (types, records, contracts and implementation-level
//! machines) into a Rust crate.
//!
//! The pipeline is fixed and deterministic:
//!
//! 1. [`load`]: JSONC text ([`jsonc`]) to the typed IR ([`ir`]).
//! 2. [`readiness`]: every backend checks that the specification states what it needs; any
//!    diagnostic refuses the whole generation.
//! 3. [`backend`]: each backend emits one module as tokens; [`emit`] assembles the crate
//!    (`Cargo.toml`, `lib.rs`, the prelude and the modules) and formats it with prettyplease.
//! 4. [`output`]: the files are written to a directory, or a directory is compared with them.
//!
//! Output depends only on the specification text, the path it is named by and the crate name:
//! everything is emitted in declaration order, nothing iterates a hash map, and nothing records
//! a time.

pub mod backend;
pub mod constructs;
pub mod diagnostic;
pub mod emit;
pub mod ir;
pub mod jsonc;
pub mod load;
pub mod naming;
pub mod output;
pub mod readiness;
pub mod render;
pub mod resolve;

pub use diagnostic::Diagnostic;
pub use emit::GeneratedFile;

/// Code of a crate name that cannot be a package name.
pub const CRATE_NAME_INVALID: &str = "specc.crate-name-invalid";

/// Options of a generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The package name of the generated crate.
    pub crate_name: String,
    /// How the generated crate depends on `strata-runtime`, which its machines implement. Only
    /// written into the manifest when the specification has an implementation-level machine.
    pub runtime: RuntimeDependency,
}

/// How the generated crate names its `strata-runtime` dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeDependency {
    /// `strata-runtime.workspace = true`: the generated crate is a member of the Strata
    /// workspace, as the golden fixture crates are.
    Workspace,
    /// `strata-runtime = { path = "..." }`, the path relative to the generated crate's directory,
    /// with forward slashes.
    Path(String),
}

/// Generates the crate's files from a specification's text, or refuses with every diagnostic.
/// `spec_path` is the path written into the headers (backslashes become forward slashes).
pub fn generate(
    spec_path: &str,
    text: &str,
    options: &Options,
) -> Result<Vec<GeneratedFile>, Vec<Diagnostic>> {
    let spec = load::load(spec_path, text)?;
    let mut diagnostics = Vec::new();
    if !emit::is_crate_name(&options.crate_name) {
        diagnostics.push(Diagnostic::new(
            CRATE_NAME_INVALID,
            diagnostic::SPEC_ENTITY,
            format!(
                "\"{}\" cannot be a package name: it must be an ASCII letter followed by letters, digits, - and _.",
                options.crate_name
            ),
        ));
    }
    diagnostics.extend(readiness::check(&spec));
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    emit::crate_files(&spec, options)
}
