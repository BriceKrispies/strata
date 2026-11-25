//! The routine tests of strata-backend, gathered into one test binary: each module below is one of
//! the package's test files, unchanged.

#[path = "agents.rs"]
mod agents;

#[path = "aggregates.rs"]
mod aggregates;

#[path = "erasure.rs"]
mod erasure;

#[path = "factors.rs"]
mod factors;

#[path = "holds.rs"]
mod holds;

#[path = "hosts_invite.rs"]
mod hosts_invite;

#[path = "lab_revision.rs"]
mod lab_revision;

#[path = "ledger.rs"]
mod ledger;

#[path = "oidc.rs"]
mod oidc;

#[path = "operations.rs"]
mod operations;

#[path = "products.rs"]
mod products;

#[path = "saml.rs"]
mod saml;

#[path = "schedules.rs"]
mod schedules;

#[path = "scim.rs"]
mod scim;

#[path = "segments.rs"]
mod segments;

#[path = "spec_agreement.rs"]
mod spec_agreement;

#[path = "tokens.rs"]
mod tokens;

#[path = "worker.rs"]
mod worker;
