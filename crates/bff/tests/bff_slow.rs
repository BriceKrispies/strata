//! The slow tests of strata-bff, gathered into one test binary: each module below is one of
//! the package's test files, unchanged. The conformance suites are binaries of their own, since
//! their generated code addresses `crate::harness`.

#[path = "process.rs"]
mod process;
