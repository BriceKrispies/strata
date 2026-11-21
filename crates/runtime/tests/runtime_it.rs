//! The routine tests of strata-runtime, gathered into one test binary: each module below is one of
//! the package's test files, unchanged.

#[path = "delayed.rs"]
mod delayed;

#[path = "dependencies.rs"]
mod dependencies;

#[path = "lease.rs"]
mod lease;

#[path = "replay.rs"]
mod replay;

#[path = "serve_limits.rs"]
mod serve_limits;

#[path = "shutdown.rs"]
mod shutdown;

#[path = "system.rs"]
mod system;

#[path = "tenants.rs"]
mod tenants;
