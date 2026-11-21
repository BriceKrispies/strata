//! Deterministic simulation of Strata.
//!
//! - [`shell`]: the deployment's own code (the backend's request path, automation worker, outbox
//!   publisher and adapters, which the backend exposes as `strata_backend::simulated`) over the
//!   store contract's in-memory adapter, with logical time, seeded randomness and the failure
//!   modes of the specification; traces checked against the store's journal; recorded workloads.
//! - [`faults`]: the store layer that injects the faults a store shows in a deployment.
//! - [`engine`]: the kernel-level engine the Impact Lab branches on: logical time, the schedule,
//!   traces, trace replay.
//! - [`workload`]: seeded generation of plausible operations from a tenant's own schema.
//! - [`project`]: counterfactual branching: baseline against a proposed schema change.
//! - [`journal`]: replay of the transitions production committed.
//!
//! The simulator is logical: no wall-clock time, no real concurrency, no resource use. What the
//! shell path reports is what the deployment's code did; `README.md` is the audit of which
//! backend paths it runs.
//!
//! # Where resource models would go
//!
//! `shell::run_shell` and `engine::run_with` already separate *what happens* (the deployment's
//! and the kernel's answers) from *when* (the schedule's logical timestamps). A calibrated
//! resource model would be a second pass over a finished trace: a function from each step (intent kind, state needs, delta size, events) to
//! a cost distribution measured on a real deployment, summed along the schedule. Because it would
//! consume traces and never feed back into the kernel's inputs, it cannot change any behavioural
//! result, and its outputs would carry the `Empirical` class. Nothing of the kind exists yet.

pub mod engine;
pub mod faults;
pub mod journal;
pub mod project;
pub mod shell;
pub mod workload;
