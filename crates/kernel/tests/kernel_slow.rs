//! The slow tests of strata-kernel, gathered into one test binary: each module below is one of
//! the package's test files, unchanged.

#[path = "approvals.rs"]
mod approvals;

#[path = "audience.rs"]
mod audience;

#[path = "ledger_policies.rs"]
mod ledger_policies;
