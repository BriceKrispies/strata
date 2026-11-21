//! Verification support for Strata.
//!
//! - [`evidence`]: the five evidence classes and the list of what is not modelled.
//! - [`invariants`]: state invariants, evaluated with the kernel's own validation.
//! - [`obligations`]: which claim is backed by which Lean theorem and which Rust test.
//! - [`lean`]: building the Lean project, auditing its axioms, and the correspondence fixtures.
//!
//! Nothing here is needed to serve a request. The backend calls into it only when someone
//! asks for a verification report or runs a simulation.

pub mod evidence;
pub mod invariants;
pub mod lean;
pub mod lean_diff;
pub mod obligations;
