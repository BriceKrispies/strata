//! Deterministic simulation of Strata, driving the production kernel.
//!
//! - [`engine`]: logical time, the schedule, seeded fault injection, traces, trace replay.
//! - [`workload`]: seeded generation of plausible operations from a tenant's own schema.
//! - [`project`]: counterfactual branching: baseline against a proposed schema change.
//! - [`journal`]: replay of the transitions production committed.
//!
//! The simulator is logical. It executes exactly the domain semantics the backend executes and
//! models nothing else: no wall-clock time, no real concurrency, no resource use.
//!
//! # Where resource models would go
//!
//! `engine::run_with` already separates *what happens* (the kernel's answer) from *when* (the
//! schedule's logical timestamps). A calibrated resource model would be a second pass over a
//! finished `Trace`: a function from each step (intent kind, state needs, delta size, events) to
//! a cost distribution measured on a real deployment, summed along the schedule. Because it would
//! consume traces and never feed back into the kernel's inputs, it cannot change any behavioural
//! result, and its outputs would carry the `Empirical` class. Nothing of the kind exists yet.

pub mod engine;
pub mod journal;
pub mod project;
pub mod workload;
