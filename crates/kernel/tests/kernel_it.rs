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

#[path = "field_read.rs"]
mod field_read;

#[path = "evolution.rs"]
mod evolution;

#[path = "generated_differential.rs"]
mod generated_differential;

#[path = "holds.rs"]
mod holds;

#[path = "ledger.rs"]
mod ledger;

#[path = "limits.rs"]
mod limits;

#[path = "object_meta.rs"]
mod object_meta;

#[path = "operation_settings.rs"]
mod operation_settings;
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

#[path = "text_rules.rs"]
mod text_rules;

#[path = "triggers.rs"]
mod triggers;

#[path = "ancestry.rs"]
mod ancestry;

#[path = "computed.rs"]
mod computed;

#[path = "role_widening.rs"]
mod role_widening;

#[path = "role_duties.rs"]
mod role_duties;
#[path = "visibility.rs"]
mod visibility;
