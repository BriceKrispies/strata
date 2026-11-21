//! What an operation asks of its caller, decided before the request's body is read.
//!
//! The handlers take their input typed, so a body that lacks a member is refused by the extractor
//! before the handler runs, and a handler that checks a capability checks it after reading its
//! input. A caller who may not make the call at all would then learn that its input is wrong
//! rather than that it is refused. The contracts name what a caller must hold (`auth.requires`),
//! and an operation that cannot be made is refused for that before anything about its input is
//! looked at: `unauthenticated` for a caller with no session, `forbidden` for one who lacks a
//! capability.
//!
//! The table holds the operations whose capability is one the tenant's roles grant by name, which
//! is the capability the operation's own check holds the caller to; the check made here is the
//! same one, made earlier.

use crate::app::{ApiError, Shared};
use crate::auth::{self, Identity};
use crate::ops;
use axum::http::HeaderMap;
use std::collections::BTreeSet;
use strata_kernel::rules::holds_any;
use strata_kernel::Grant;
use strata_protocol::api::ErrorCode;

/// An operation of the backend and the capabilities its contract requires (every one).
struct Rule {
    path: &'static str,
    requires: &'static [&'static str],
}

const fn rule(path: &'static str, requires: &'static [&'static str]) -> Rule {
    Rule { path, requires }
}

/// The operations, by the path the gateway calls them at.
const RULES: &[Rule] = &[
    // Segments (CT-290 to CT-293, CT-813 to CT-815).
    rule("/internal/segments/define", &["segments-define"]),
    rule("/internal/segments/remove", &["segments-define"]),
    rule("/internal/segments/test", &["segments-define"]),
    rule("/internal/segments/preview", &["segments-define"]),
    rule("/internal/segments/members", &["segments-read"]),
    rule("/internal/segments/count", &["segments-read"]),
    rule("/internal/segments/mine", &["segments-read-own"]),
    // The ledger.
    rule("/internal/ledger/post", &["ledger-post"]),
    rule("/internal/ledger/balance", &["ledger-read"]),
    rule("/internal/ledger/entries", &["ledger-read"]),
    rule("/internal/ledger/accounts", &["ledger-read"]),
    rule("/internal/ledger/definitions", &["ledger-read"]),
    rule("/internal/ledger/my-balance", &["ledger-read-own"]),
    rule("/internal/ledger/my-entries", &["ledger-read-own"]),
    rule("/internal/ledger/declare-unit", &["ledger-configure"]),
    rule("/internal/ledger/declare-pool", &["ledger-configure"]),
    rule("/internal/ledger/set-limit", &["ledger-configure"]),
    rule("/internal/ledger/remove-definition", &["ledger-configure"]),
    // Settings.
    rule("/internal/settings/save-definition", &["setting-define"]),
    rule("/internal/settings/set-tenant", &["setting-write-tenant"]),
    rule("/internal/settings/set-team", &["setting-write-team"]),
    // Automation commands and exceptions.
    rule("/internal/automation/activate", &["workflow-activate"]),
    rule("/internal/automation/start", &["jobs-run"]),
    rule("/internal/automation/approve", &["workflow-approve"]),
    rule(
        "/internal/automation/exceptions/settle",
        &["automations-define"],
    ),
    // Webhooks.
    rule("/internal/webhooks/endpoints/create", &["webhooks-manage"]),
    rule("/internal/webhooks/endpoints/rotate", &["webhooks-manage"]),
    rule(
        "/internal/webhooks/subscriptions/create",
        &["webhooks-manage"],
    ),
    rule(
        "/internal/webhooks/subscriptions/rotate",
        &["webhooks-manage"],
    ),
    // Bundles.
    rule("/internal/products/install", &["bundle-install"]),
    rule("/internal/products/upgrade", &["bundle-install"]),
    rule("/internal/products/remove", &["bundle-install"]),
];

/// The capabilities the operation at `path` requires, when it has an entry.
fn requires(path: &str) -> Option<&'static [&'static str]> {
    RULES
        .iter()
        .find(|rule| rule.path == path)
        .map(|rule| rule.requires)
}

/// Refuse a call of an operation of the table whose caller has no session or lacks a capability
/// the contract requires. Anything else, and any operation not in the table, goes on.
pub async fn admit(app: &Shared, path: &str, headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(required) = requires(path) else {
        return Ok(());
    };
    let caller = auth::caller(headers)?;
    let correlation = caller.correlation.clone();
    let tenant = caller.site.as_str().to_string();
    app.blocking_for(&tenant, move |app| {
        let identity = auth::identify(app, &caller)?;
        holds_all(app, &identity, required)
    })
    .await?
    .map_err(|error| error.with_correlation(&correlation))
}

/// The caller is signed in and holds every capability of `required`; the owner holds all of them.
fn holds_all(
    app: &crate::app::App,
    identity: &Identity,
    required: &[&str],
) -> Result<(), ApiError> {
    if identity.anonymous() {
        return Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in to do this.",
        ));
    }
    let tenant = ops::header(app, identity)?;
    for name in required {
        let wanted = BTreeSet::from([Grant::Capability((*name).to_string())]);
        if !holds_any(&tenant, &identity.principal, &wanted) {
            return Err(ApiError::new(
                ErrorCode::Forbidden,
                format!("This needs the permission {name}."),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table names an operation once, with at least one capability.
    #[test]
    fn every_operation_of_the_table_is_named_once_and_asks_for_something() {
        let mut seen = BTreeSet::new();
        for rule in RULES {
            assert!(!rule.requires.is_empty(), "{} asks for nothing", rule.path);
            assert!(seen.insert(rule.path), "{} is listed twice", rule.path);
        }
    }
}
