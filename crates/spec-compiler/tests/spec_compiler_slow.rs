//! The slow tests of strata-spec-compiler, gathered into one test binary: each module below is one of
//! the package's test files, unchanged.

mod support;

#[path = "bundle.rs"]
mod bundle;

#[path = "conformance.rs"]
mod conformance;

#[path = "golden.rs"]
mod golden;

#[path = "layers.rs"]
mod layers;

#[path = "strata_differential.rs"]
mod strata_differential;
