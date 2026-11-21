//! The routine tests of strata-spec-expr, gathered into one test binary: each module below is one of
//! the package's test files, unchanged.

#[path = "grammar.rs"]
mod grammar;

#[path = "js_semantics.rs"]
mod js_semantics;

#[path = "js_translate.rs"]
mod js_translate;

#[path = "json.rs"]
mod json;

#[path = "spec_files.rs"]
mod spec_files;

#[path = "translate.rs"]
mod translate;

#[path = "typing.rs"]
mod typing;
