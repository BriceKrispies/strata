//! specc: compiles an Exegra specification (types, records, contracts and implementation-level
//! machines) into a Rust crate.
//!
//! The pipeline is fixed and deterministic:
//!
//! 1. [`load`]: JSONC text ([`jsonc`]) to the typed IR ([`ir`]).
//! 2. [`readiness`]: every backend checks that the specification states what it needs; any
//!    diagnostic refuses the whole generation.
//! 3. [`backend`]: each backend emits one module as tokens; [`emit`] assembles the crate
//!    (`Cargo.toml`, `lib.rs`, the prelude and the modules), prints it with prettyplease and
//!    formats it with the `rustfmt` on PATH (`--edition 2021`), so the generated crate passes
//!    `cargo fmt --check`. Without rustfmt the generation is refused (`specc.rustfmt-missing`)
//!    rather than written unformatted.
//! 4. [`output`]: the files are written to a directory, or a directory is compared with them.
//!
//! Product files layered on the specification ([`layers`]) are loaded and checked with it; they
//! are tenant metadata, so nothing is generated from them and the output is the platform's alone.
//!
//! Output depends only on the specification text, the path it is named by, the crate name and
//! the rustfmt version (the repository pins its toolchain in `rust-toolchain.toml`):
//! everything is emitted in declaration order, nothing iterates a hash map, and nothing records
//! a time.

pub mod backend;
pub mod bundle;
pub mod budget;
pub mod catalogue;
pub mod codetext;
pub mod constructs;
pub mod contract_rules;
pub mod deploy;
pub mod diagnostic;
pub mod e2e;
pub mod emit;
pub mod function;
pub mod ir;
pub mod jsonc;
pub mod layer_expr;
pub mod layer_values;
pub mod language;
pub mod layers;
pub mod load;
pub mod model_core;
pub mod naming;
pub mod output;
pub mod readiness;
pub mod render;
pub mod resolve;
pub mod rules;
pub mod scenarios;
pub mod web;
pub mod web_kernel;
pub mod web_screens;
pub mod wire;

pub use diagnostic::Diagnostic;
pub use emit::GeneratedFile;

/// Code of a crate name that cannot be a package name.
pub const CRATE_NAME_INVALID: &str = "specc.crate-name-invalid";

/// Options of a generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The package name of the generated crate.
    pub crate_name: String,
    /// How the generated crate depends on `strata-runtime`, which its machines implement and its
    /// contract routes and clients use. Only written into the manifest when the specification
    /// has an implementation-level machine or a contract a component implements.
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

/// The shared readiness check alone: loads the specification and runs every backend's check, as
/// [`generate`] does before it emits, and emits nothing. The commands that generate from the raw
/// specification (`web`, `e2e`, `catalogue`, `deploy`, `scenarios`) run it first, so a
/// specification the Rust backends would refuse is refused for them as well.
pub fn check_ready(spec_path: &str, text: &str) -> Result<(), Vec<Diagnostic>> {
    let spec = load::load(spec_path, text)?;
    let diagnostics = readiness::check(&spec);
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

/// The files of a generation split into two crates: the model crate (`options`), and the pure
/// crate `core` names ([`model_core`]), which the model crate depends on. Returns the model crate's
/// files, then the pure crate's, or refuses with every diagnostic.
pub fn generate_with_core(
    spec_path: &str,
    text: &str,
    options: &Options,
    core: &model_core::CoreDependency,
) -> Result<(Vec<GeneratedFile>, Vec<GeneratedFile>), Vec<Diagnostic>> {
    let spec = load::load(spec_path, text)?;
    let mut diagnostics = Vec::new();
    for name in [&options.crate_name, &core.crate_name] {
        if !emit::is_crate_name(name) {
            diagnostics.push(Diagnostic::new(
                CRATE_NAME_INVALID,
                diagnostic::SPEC_ENTITY,
                format!(
                    "\"{name}\" cannot be a package name: it must be an ASCII letter followed by letters, digits, - and _."
                ),
            ));
        }
    }
    if options.crate_name == core.crate_name {
        diagnostics.push(Diagnostic::new(
            CRATE_NAME_INVALID,
            diagnostic::SPEC_ENTITY,
            format!(
                "the model crate and the pure crate are both named \"{}\"; give them different names.",
                core.crate_name
            ),
        ));
    }
    diagnostics.extend(readiness::check(&spec));
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let model = emit::crate_files_with(&spec, options, Some(core))?;
    let pure = model_core::crate_files(&spec, &core.crate_name)?;
    Ok((model, pure))
}

/// The files of the pure crate alone ([`model_core`]), named `crate_name`: what [`generate_with_core`]
/// returns second. The golden fixture of unions and maps is generated this way.
pub fn generate_core(
    spec_path: &str,
    text: &str,
    crate_name: &str,
) -> Result<Vec<GeneratedFile>, Vec<Diagnostic>> {
    let spec = load::load(spec_path, text)?;
    let mut diagnostics = Vec::new();
    if !emit::is_crate_name(crate_name) {
        diagnostics.push(Diagnostic::new(
            CRATE_NAME_INVALID,
            diagnostic::SPEC_ENTITY,
            format!(
                "\"{crate_name}\" cannot be a package name: it must be an ASCII letter followed by letters, digits, - and _."
            ),
        ));
    }
    diagnostics.extend(readiness::check(&spec));
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    model_core::crate_files(&spec, crate_name)
}
