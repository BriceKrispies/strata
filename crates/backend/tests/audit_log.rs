//! The audit log of a tenant: every configuration and role change is an entry with the previous
//! value, the new value, the principal and the correlation identifier (OPS-329), and the route
//! `verify-audit-chain` (CT-128) reports a chain whose entry was altered or removed.
//!
//! The tenant is `alpha`, owned by `alpha:owner`, on the in-memory store. The changes go through
//! the same functions the routes call (`ops::members`, `ops::run_intent`); the verification goes
//! through `audit_log::verify`, which `POST /internal/audit/verify` serves.

use serde_json::{json, Value as Json};
use std::sync::Arc;
use strata_backend::app::{ApiError, App, Config, Shared};
use strata_backend::audit_log;
use strata_backend::auth::Identity;
use strata_backend::mail::DevSink;
use strata_backend::ops;
use strata_kernel::{
    Context, ExistingData, Intent, Permission, Principal, PrincipalId, RoleDef, RoleId,
    SchemaChange, SessionId, TenantId, Timestamp,
};
use strata_protocol::api::MembersRequest;
use strata_protocol::ws::SessionState;
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, SessionRecord, Store};

fn tenant(name: &str) -> TenantId {
    TenantId::new(name).expect("tenant id")
}

fn who(name: &str) -> PrincipalId {
    PrincipalId::new(name).expect("principal id")
}

fn role(name: &str) -> RoleId {
    RoleId::new(name).expect("role id")
}

fn app_on(store: Arc<MemoryStore>) -> Shared {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cfg = Config {
        addr: "127.0.0.1:0".parse().expect("address"),
        service_token: "test-service-token-0123456789abcdef".into(),
        dev: true,
        tenants_dir: root.join("tenants"),
        lean_dir: root.join("lean"),
        session_ttl_ms: 3_600_000,
        session_idle_ms: 3_600_000,
        throttle: Default::default(),
        passkey_challenge_ttl_ms: 300_000,
        link_ttl_ms: 900_000,
        mail: Arc::new(DevSink::memory()),
        federation: Arc::new(strata_backend::signin::oidc::Federation::none()),
        factors: Default::default(),
        limits: Default::default(),
    };
    App::new(cfg, store)
}

/// A signed-in identity of `principal` whose request carries `correlation`.
fn identity(app: &App, site: &TenantId, principal: &str, correlation: &str) -> Identity {
    let principal = who(principal);
    let now = app.now().0;
    Identity {
        tenant: site.clone(),
        principal: Principal::User(principal.clone()),
        session: Some(SessionRecord {
            id: SessionId::new(format!("s-{}-{}", site.as_str(), principal.as_str()))
                .expect("session id"),
            token_hash: "hash-of-the-session-token".into(),
            csrf_token: "csrf-token-value".into(),
            tenant: site.clone(),
            principal,
            display_name: "someone".into(),
            created_at: Timestamp(now),
            last_seen_at: Timestamp(now),
            expires_at: Timestamp(i64::MAX),
            revoked_at: None,
            authenticated_at: Timestamp(now),
            network: None,
        }),
        state: SessionState::Active,
        correlation: correlation.to_string(),
        verified_address: None,
        idempotency_key: None,
    }
}

/// A tenant `alpha` with the roles `editor` and `viewer`, whose owner is `alpha:owner`. The
/// roles are defined through the route's own commit path, so the change is an audit entry too.
fn started() -> (Shared, Arc<MemoryStore>, TenantId) {
    let memory = Arc::new(MemoryStore::new());
    let app = app_on(memory.clone());
    let site = tenant("alpha");
    execute(
        memory.as_ref(),
        &Intent::CreateTenant {
            name: "alpha".into(),
            owner: who("alpha:owner"),
        },
        &Context {
            tenant: site.clone(),
            principal: Principal::System,
            session: None,
            now: Timestamp(1_000),
            correlation: "found".into(),
            verified_address: None,
            idempotency_key: None,
            language: None,
            hop: None,
        },
    )
    .expect("the tenant is created");
    let owner = identity(&app, &site, "alpha:owner", "t-roles-defined");
    let changes = ["editor", "viewer"]
        .into_iter()
        .map(|name| {
            SchemaChange::DefineRole(RoleDef {
                id: role(name),
                label: name.to_string(),
            })
        })
        .collect();
    ops::run_intent(
        &app,
        &owner.context(&app),
        &Intent::ChangeSchema {
            changes,
            expected_version: None,
            existing: ExistingData::Reject,
        },
        false,
    )
    .expect("the roles are defined");
    (app, memory, site)
}

fn members(
    app: &App,
    site: &TenantId,
    correlation: &str,
    request: MembersRequest,
) -> Result<(), ApiError> {
    let owner = identity(app, site, "alpha:owner", correlation);
    ops::members(app, &owner, &request).map(|_| ())
}

/// The data of each entry of `event`, as the owner of the tenant reads it, oldest first, with the
/// correlation identifier of the entry.
fn entries(app: &App, site: &TenantId, event: &str) -> Vec<(String, Json)> {
    app.store
        .audit_list(site, Some("audit"), 1_000)
        .expect("the audit trail")
        .into_iter()
        .filter(|line| line.event == event)
        .map(|line| {
            (
                line.correlation,
                serde_json::from_str(&line.data).expect("the data is JSON"),
            )
        })
        .collect()
}

/// Give the role `editor` the capability `audit-read`, through the commit path of the routes.
fn grant_audit_read(app: &App, site: &TenantId, correlation: &str) {
    let owner = identity(app, site, "alpha:owner", correlation);
    ops::run_intent(
        app,
        &owner.context(app),
        &Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineRolePermissions {
                role: role("editor"),
                permissions: vec![Permission::Capability("audit-read".into())],
            }],
            expected_version: None,
            existing: ExistingData::Reject,
        },
        false,
    )
    .expect("the permission is granted");
}

// Verifies: OPS-329
// Verifies: OPS-3120
#[test]
fn a_configuration_change_is_an_entry_with_the_previous_and_the_new_value() {
    let (app, _memory, site) = started();
    // The definition of the roles made by `started` is the first schema change.
    let defined = entries(&app, &site, "schema.changed");
    assert_eq!(defined.len(), 1, "{defined:?}");
    assert_eq!(defined[0].0, "t-roles-defined");
    assert_eq!(defined[0].1["actor"], "alpha:owner");
    assert!(
        defined[0].1["changes"]
            .as_array()
            .is_some_and(|summary| !summary.is_empty()),
        "{}",
        defined[0].1
    );

    // Granting a role a permission is a change of configuration and of roles at once: the entry
    // holds the permissions of the role before and after, and the schema version.
    grant_audit_read(&app, &site, "t-permissions");
    let changed = entries(&app, &site, "schema.changed");
    assert_eq!(changed.len(), 2, "{changed:?}");
    let (correlation, data) = &changed[1];
    assert_eq!(correlation, "t-permissions");
    assert_eq!(data["actor"], "alpha:owner");
    assert_eq!(data["tenant"], "alpha");
    assert_eq!(
        data["after"]["version"].as_u64(),
        data["before"]["version"].as_u64().map(|v| v + 1),
        "{data}"
    );
    assert_eq!(data["before"]["role_permissions"]["editor"], json!([]));
    assert!(
        data["after"]["role_permissions"]["editor"]
            .to_string()
            .contains("audit-read"),
        "{data}"
    );

    // A schema change the kernel refuses is not an entry.
    let owner = identity(&app, &site, "alpha:owner", "t-refused");
    let refused = ops::run_intent(
        &app,
        &owner.context(&app),
        &Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineRolePermissions {
                role: role("no_such_role"),
                permissions: vec![Permission::Capability("audit-read".into())],
            }],
            expected_version: None,
            existing: ExistingData::Reject,
        },
        false,
    );
    assert!(refused.is_err());
    assert_eq!(entries(&app, &site, "schema.changed").len(), 2);
}

// Verifies: OBS-007
// Verifies: OBS-3121
#[test]
fn the_route_reports_a_removed_entry() {
    let (app, memory, site) = started();
    for (correlation, to) in [("t-1", "editor"), ("t-2", "viewer")] {
        members(
            &app,
            &site,
            correlation,
            MembersRequest::AssignRole {
                principal: "alpha:ada".into(),
                role: to.into(),
            },
        )
        .expect("assigned");
    }
    let owner = identity(&app, &site, "alpha:owner", "t-verify");
    assert_eq!(
        audit_log::verify(&app, &owner, &json!({})).expect("verified")["intact"],
        true
    );
    // The second line is removed from the middle of the trail.
    assert!(memory.remove_audit_line(&site, 1));
    let cut = audit_log::verify(&app, &owner, &json!({})).expect("verified");
    assert_eq!(cut["intact"], false, "{cut}");
    assert_eq!(cut["first_break"], 2, "{cut}");
}
