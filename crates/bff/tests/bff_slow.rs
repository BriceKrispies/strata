//! The slow tests of strata-bff, gathered into one test binary: each module below is one of
//! the package's test files, unchanged.

#[path = "contract_conformance.rs"]
mod contract_conformance;

#[path = "contract_conformance_fixtures.rs"]
mod contract_conformance_fixtures;

#[path = "process.rs"]
mod process;
