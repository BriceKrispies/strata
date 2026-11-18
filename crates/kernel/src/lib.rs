//! The Strata domain kernel.
//!
//! Every domain decision is made here, by pure functions of explicit arguments:
//!
//! - [`transition`] decides an intent against a snapshot and returns a state delta, events,
//!   effect descriptions and evidence, or a rejection with evidence.
//! - [`read`] answers an authorized read.
//! - [`needs`] and [`read_needs`] say which state a decision is expected to require.
//! - [`apply_delta`] applies an accepted delta to a tenant state.
//!
//! The kernel performs no I/O, reads no clock, draws no randomness and holds no global state.
//! Time, identity and correlation arrive in [`Context`]. Record identifiers come from a
//! per-tenant counter carried in the state.

pub mod canon;
pub mod expr;
pub mod ids;
pub mod intent;
pub mod read;
pub mod rules;
pub mod schema;
pub mod state;
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;
pub mod transition;
pub mod value;
pub mod world;

pub use expr::{CmpOp, Expr, Scope};
pub use ids::*;
pub use intent::*;
pub use read::{read, read_needs, visible};
pub use schema::*;
pub use state::*;
pub use transition::{apply_delta, needs, plan_migration, transition, Migration, Violation};
pub use value::{FieldType, OnDelete, Value};
pub use world::World;

/// Identifies the transition semantics. Recorded in every trace and journal entry, so a replay
/// knows which kernel produced what it is replaying.
pub const KERNEL_VERSION: &str = "strata-kernel/0.1.0";
