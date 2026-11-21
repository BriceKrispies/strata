//! The pure crate (`model-core`): the part of the generated model a pure kernel can depend on.
//!
//! The kernel may not touch files, the network, the clock, randomness or the environment
//! (KRN-012), so it cannot depend on the model crate, which depends on `strata-runtime` (tokio,
//! axum, rusqlite). The pure crate holds what needs none of that:
//!
//! - `prelude`: the representation types (`Timestamp`, `DurationMs`, the safe-integer decoders);
//! - `types`: every declared type ([`types::is_core_type`]);
//! - one module per declared `language` ([`crate::language`]): its parser, printer, static
//!   checker and evaluator over the language's AST and value types;
//! - `functions` when the specification declares functions ([`crate::function`]): each a pure
//!   Rust function, each example a unit test.
//!
//! Its only dependencies are serde, serde_json and regex (sha2 is allowed for what later needs
//! it), and uuid with its `serde` feature only when a declared type reaches a `uuid`: no `v4`, so
//! nothing in it makes a random id. The model crate generated beside it depends on it by path,
//! re-exports its prelude's representation types and its types (`pub use <core>::types::*;`),
//! and declares only the types the pure crate does not hold, so every `types::Name` path of the
//! model crate stays valid and names the same type in both crates.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

use crate::backend::{prelude, types};
use crate::diagnostic::Diagnostic;
use crate::emit::{
    self, GeneratedFile, REGEX_VERSION, SERDE_JSON_VERSION, SERDE_VERSION, UUID_VERSION,
};
use crate::ir::Spec;
use crate::{function, language};

/// How the model crate names the pure crate beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreDependency {
    /// The pure crate's package name.
    pub crate_name: String,
    /// Its directory relative to the model crate's, with forward slashes.
    pub path: String,
}

/// Every file of the pure crate, sorted by path. Assumes the readiness checks passed.
pub fn crate_files(spec: &Spec, crate_name: &str) -> Result<Vec<GeneratedFile>, Vec<Diagnostic>> {
    crate_files_with(spec, crate_name, false)
}

/// As [`crate_files`]; with `tests_apart`, the examples of the functions are not a test module of
/// `functions` but integration test files `tests/function_examples_<n>.rs`
/// ([`function::emit_tests_apart`]), so the library compiles once, with no tests, and each test
/// file in a compiler process of its own.
pub fn crate_files_with(
    spec: &Spec,
    crate_name: &str,
    tests_apart: bool,
) -> Result<Vec<GeneratedFile>, Vec<Diagnostic>> {
    let mut files = vec![GeneratedFile {
        path: "Cargo.toml".to_string(),
        contents: cargo_toml(spec, crate_name),
    }];
    let languages: Vec<String> = spec.languages.iter().map(language::module_name).collect();
    let mut modules: Vec<(&str, TokenStream)> = vec![
        (prelude::MODULE, prelude::emit_core()),
        ("types", types::emit(spec, types::Selection::Core)),
    ];
    for (lang, module) in spec.languages.iter().zip(&languages) {
        modules.push((module.as_str(), language::emit(spec, lang)));
    }
    let mut names = languages.clone();
    let mut tests = Vec::new();
    if function::wanted(spec) {
        if tests_apart {
            let (module, test_files) = function::emit_tests_apart(spec, crate_name);
            modules.push((function::MODULE, module));
            tests = test_files;
        } else {
            modules.push((function::MODULE, function::emit(spec)));
        }
        names.push(function::MODULE.to_string());
    }
    modules.push(("lib", lib_rs(spec, &names)));
    let tests: Vec<(String, TokenStream)> = tests
        .into_iter()
        .map(|(stem, tokens)| (format!("tests/{stem}.rs"), tokens))
        .collect();
    files.extend(emit::rust_files_at(spec, Vec::new(), tests)?);
    emit::rust_files(spec, files, modules)
}

fn cargo_toml(spec: &Spec, crate_name: &str) -> String {
    // The serde feature only: no v4, so the pure crate cannot make a random id.
    let uuid = if types::core_reaches_uuid(spec) {
        format!(
            "uuid = {{ version = \"{UUID_VERSION}\", default-features = false, features = [\"serde\"] }}\n"
        )
    } else {
        String::new()
    };
    // `rc` lets serde read and write a `shared<T>` (an `Arc<T>`) as the `T` it holds.
    let serde_features = if types::core_uses_shared(spec) {
        r#"["derive", "rc"]"#
    } else {
        r#"["derive"]"#
    };
    format!(
        "{header}\n\
         [package]\n\
         name = \"{crate_name}\"\n\
         version = \"0.1.0\"\n\
         edition = \"2021\"\n\
         publish = false\n\
         \n\
         [dependencies]\n\
         regex = \"{REGEX_VERSION}\"\n\
         serde = {{ version = \"{SERDE_VERSION}\", features = {serde_features} }}\n\
         serde_json = \"{SERDE_JSON_VERSION}\"\n\
         {uuid}\n\
         [lints]\n\
         workspace = true\n",
        header = emit::toml_header(&spec.path),
    )
}

fn lib_rs(spec: &Spec, languages: &[String]) -> TokenStream {
    let summary = format!(
        " {}: the pure part of its specification's model, as Rust: the declared types, which reach no runtime type.",
        spec.system.name
    );
    let purity = " It touches no file, network, clock, randomness or environment, and depends only on serde, serde_json, regex and (for uuid ids) uuid without its v4 feature, so a pure kernel can depend on it.";
    let origin = format!(
        " Generated by specc from system `{}`. Change the specification and regenerate; do not edit.",
        spec.system.id
    );
    let modules: Vec<Ident> = ["prelude", "types"]
        .into_iter()
        .chain(languages.iter().map(String::as_str))
        .map(|m| Ident::new(m, Span::call_site()))
        .collect();
    quote! {
        #![doc = #summary]
        #![doc = ""]
        #![doc = #purity]
        #![doc = ""]
        #![doc = #origin]
        #![warn(missing_docs)]
        #![allow(clippy::enum_variant_names)]

        #( pub mod #modules; )*
    }
}
