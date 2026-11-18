//! What the backend and the BFF say to each other, and what the BFF says to the browser over
//! the WebSocket.
//!
//! - [`api`]: the backend's internal HTTP API. Loopback only, service token required.
//! - [`ws`]: WebSocket protocol v1, spoken browser to BFF and BFF to backend alike.
//! - [`log`]: one structured log line format for both services.
//! - [`tenants`]: reading tenant bundles from a directory.

pub mod api;
pub mod log;
pub mod tenants;
pub mod ws;
