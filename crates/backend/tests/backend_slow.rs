//! The slow tests of strata-backend, gathered into one test binary: each module below is one of
//! the package's test files, unchanged.

#[path = "aggregate_copies.rs"]
mod aggregate_copies;

#[path = "approvals.rs"]
mod approvals;

#[path = "audit_log.rs"]
mod audit_log;

#[path = "badge_milestones.rs"]
mod badge_milestones;

#[path = "dispatch.rs"]
mod dispatch;

#[path = "files.rs"]
mod files;

#[path = "inbox.rs"]
mod inbox;

#[path = "keycloak.rs"]
mod keycloak;

#[path = "ledger_more.rs"]
mod ledger_more;

#[path = "outbound.rs"]
mod outbound;

#[path = "quota.rs"]
mod quota;

#[path = "sealing.rs"]
mod sealing;

#[path = "search.rs"]
mod search;

#[path = "segment_jobs.rs"]
mod segment_jobs;

#[path = "service.rs"]
mod service;

#[path = "startup.rs"]
mod startup;

#[path = "subject_rights.rs"]
mod subject_rights;
