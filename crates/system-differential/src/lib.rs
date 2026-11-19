//! The system host's differential test against Exegra; the crate holds nothing but the test
//! (`tests/exegra.rs`).
//!
//! For every scenario of `specifications/spec.jsonc` that drives implementation-level machines,
//! the test runs the scenario's external entries through `strata_runtime::System`, with the
//! generated machines and routing table (`strata_model::machines::dispatch`) on a `TestClock`,
//! and compares every step with `exegra trace <scenario>`: the entry delivered and its time, every
//! instance's state and variables, and the whole pending queue (emits and timers with the times
//! they are due). It needs `EXEGRA_HOME` (an Exegra checkout) and `node`, and fails without them.
//!
//! ```text
//! EXEGRA_HOME=C:/dev/exegra cargo test -p strata-system-differential -- --nocapture
//! ```

#![forbid(unsafe_code)]
