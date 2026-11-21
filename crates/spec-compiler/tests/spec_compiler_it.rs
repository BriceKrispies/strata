//! The routine tests of strata-spec-compiler, gathered into one test binary: each module below is one of
//! the package's test files, unchanged.

#[path = "adversarial.rs"]
mod adversarial;

#[path = "audit_refusals.rs"]
mod audit_refusals;

#[path = "catalogue.rs"]
mod catalogue;

#[path = "deploy.rs"]
mod deploy;

#[path = "derived.rs"]
mod derived;

#[path = "differential.rs"]
mod differential;

#[path = "e2e.rs"]
mod e2e;

#[path = "functions.rs"]
mod functions;

#[path = "kernel_types.rs"]
mod kernel_types;

#[path = "language.rs"]
mod language;

#[path = "language_js.rs"]
mod language_js;

#[path = "layer_expr_drift.rs"]
mod layer_expr_drift;

#[path = "machine_refusals.rs"]
mod machine_refusals;

#[path = "realisation.rs"]
mod realisation;

#[path = "scenarios.rs"]
mod scenarios;

#[path = "web.rs"]
mod web;

#[path = "web_rules.rs"]
mod web_rules;

#[path = "wiring.rs"]
mod wiring;
