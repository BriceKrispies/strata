//! The routine tests of strata-spec-compiler-language-fixture, gathered into one test binary: each module below is one of
//! the package's test files, unchanged.

#[path = "dependencies.rs"]
mod dependencies;

#[path = "evaluation.rs"]
mod evaluation;

#[path = "kernel_cases.rs"]
mod kernel_cases;

#[path = "syntax.rs"]
mod syntax;
