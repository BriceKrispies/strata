//! The routine tests of strata-kernel, gathered into one test binary: each module below is one of
//! the package's test files, unchanged.

#[path = "aggregates.rs"]
mod aggregates;

#[path = "behaviour.rs"]
mod behaviour;

#[path = "definitions.rs"]
mod definitions;

#[path = "dependencies.rs"]
mod dependencies;

#[path = "erasure.rs"]
mod erasure;

#[path = "evolution.rs"]
mod evolution;

#[path = "generated_differential.rs"]
mod generated_differential;

#[path = "ledger.rs"]
mod ledger;

#[path = "limits.rs"]
mod limits;

#[path = "object_meta.rs"]
mod object_meta;

#[path = "operations.rs"]
mod operations;

#[path = "properties.rs"]
mod properties;

#[path = "query_plan.rs"]
mod query_plan;

#[path = "rule_arithmetic.rs"]
mod rule_arithmetic;

#[path = "segments.rs"]
mod segments;

#[path = "settings.rs"]
mod settings;

#[path = "triggers.rs"]
mod triggers;
