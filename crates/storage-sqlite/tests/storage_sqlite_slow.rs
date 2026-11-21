//! The slow tests of strata-storage-sqlite, gathered into one test binary: each module below is one of
//! the package's test files, unchanged.

#[path = "durability.rs"]
mod durability;

#[path = "fleet.rs"]
mod fleet;

#[path = "ledger_limits.rs"]
mod ledger_limits;

#[path = "query_plan_scale.rs"]
mod query_plan_scale;

#[path = "sealed_index.rs"]
mod sealed_index;

#[path = "search.rs"]
mod search;
