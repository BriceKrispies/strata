//! The runtime is generic: it depends on no crate that holds platform semantics.
//!
//! A source check, like `crates/verification/tests/purity.rs`, without `cargo metadata`: it reads
//! the manifest's dependency sections and scans every source file for paths into the forbidden
//! crates. Generated and platform code may depend on the runtime, never the other way round.
