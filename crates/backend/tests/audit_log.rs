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

// Verifies: OBS-3129
#[test]
fn a_chain_cut_short_of_its_anchor_is_refused_naming_the_lost_line() {
    let (app, memory, site) = started();
    for (correlation, to) in [("t-1", "editor"), ("t-2", "viewer"), ("t-3", "editor")] {
        members(
            &app,
            &site,
            correlation,
            MembersRequest::AssignRole {
                principal: format!("alpha:{correlation}"),
                role: to.into(),
            },
        )
        .expect("assigned");
    }
    let anchor = app
        .anchor_audit_head(&site)
        .expect("anchored")
        .expect("the trail has a head");
    let owner = identity(&app, &site, "alpha:owner", "t-verify");
    let whole = audit_log::verify(&app, &owner, &json!({})).expect("verified");
    assert_eq!(whole["intact"], true, "{whole}");

    // The newest line is removed: every remaining line still chains, and only the anchor, which
    // the tenant's own storage does not hold, shows what was lost.
    assert!(memory.remove_audit_line(&site, anchor.seq as usize - 1));
    let cut = audit_log::verify(&app, &owner, &json!({})).expect("verified");
    assert_eq!(cut["intact"], false, "{cut}");
    assert_eq!(cut["code"], "AnchorMismatch", "{cut}");
    assert_eq!(cut["first_break"].as_u64(), Some(anchor.seq), "{cut}");

    // Anchoring again does not anchor over the cut.
    let held = app
        .anchor_audit_head(&site)
        .expect("anchored")
        .expect("held");
    assert_eq!(held, anchor);
}

// Verifies: OBS-3129
#[test]
fn a_restore_does_not_drop_audit_entries() {
    let (app, memory, site) = started();
    let snapshot = memory
        .create_snapshot(&site, "before", Timestamp(1_500))
        .expect("snapshot");
    for correlation in ["t-1", "t-2"] {
        members(
            &app,
            &site,
            correlation,
            MembersRequest::AssignRole {
                principal: format!("alpha:{correlation}"),
                role: "editor".into(),
            },
        )
        .expect("assigned");
    }
    let before = memory.audit_list(&site, None, 1_000).expect("trail").len();
    memory
        .restore_snapshot(&site, snapshot.id, None)
        .expect("restored");
    let after = memory.audit_list(&site, None, 1_000).expect("trail").len();
    assert_eq!(before, after, "a restore leaves the audit trail whole");
    let report = app.verify_audit_chain(&site).expect("chain");
    assert!(report.ok, "{report:?}");
}

/// A tenant whose type `event` is append-only, with one record `1`, ready to be changed.
fn with_append_only_type() -> (Shared, Arc<MemoryStore>, TenantId) {
    let (app, memory, site) = started();
    let owner = identity(&app, &site, "alpha:owner", "t-declare");
    let changes = vec![
        SchemaChange::DefineObject {
            id: strata_kernel::ObjectTypeId::new("event").expect("object"),
            label: "Event".into(),
        },
        SchemaChange::DefineField {
            object: strata_kernel::ObjectTypeId::new("event").expect("object"),
            field: strata_kernel::testkit::field_def("name", strata_kernel::FieldType::Str, true),
        },
        SchemaChange::SetAppendOnly {
            object: strata_kernel::ObjectTypeId::new("event").expect("object"),
        },
    ];
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
    .expect("the type is declared append-only");
    let created = ops::run_intent(
        &app,
        &identity(&app, &site, "alpha:owner", "t-create").context(&app),
        &Intent::CreateRecord {
            object: strata_kernel::ObjectTypeId::new("event").expect("object"),
            fields: std::collections::BTreeMap::from([(
                strata_kernel::FieldId::new("name").expect("field"),
                strata_kernel::Value::Str("first".into()),
            )]),
        },
        false,
    );
    created.expect("a record is appended");
    (app, memory, site)
}

// Verifies: OBS-3128
#[test]
fn an_update_a_delete_and_a_restore_of_an_append_only_record_are_refused_and_audited() {
    let (app, memory, site) = with_append_only_type();
    let event = || strata_kernel::ObjectTypeId::new("event").expect("object");
    let owner = || identity(&app, &site, "alpha:owner", "t-attempt");

    // The owner may neither update nor delete it.
    let updated = ops::run_intent(
        &app,
        &owner().context(&app),
        &Intent::UpdateRecord {
            object: event(),
            id: strata_kernel::RecordId(1),
            fields: std::collections::BTreeMap::from([(
                strata_kernel::FieldId::new("name").expect("field"),
                strata_kernel::Value::Str("rewritten".into()),
            )]),
            expected_version: None,
        },
        false,
    )
    .expect_err("an update is refused");
    assert_eq!(updated.0.kind.as_deref(), Some("AppendOnly"));
    let deleted = ops::run_intent(
        &app,
        &owner().context(&app),
        &Intent::DeleteRecord {
            object: event(),
            id: strata_kernel::RecordId(1),
            expected_version: None,
        },
        false,
    )
    .expect_err("a delete is refused");
    assert_eq!(deleted.0.kind.as_deref(), Some("AppendOnly"));

    // Each refusal is an audit entry that names the intent and the actor.
    let refused = entries(&app, &site, "refused.append_only");
    let intents: Vec<&str> = refused
        .iter()
        .map(|(_, data)| data["intent"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(intents, ["update_record", "delete_record"], "{refused:?}");
    assert_eq!(refused[0].1["actor"], "alpha:owner");

    // A restore to a state before the record is refused too, and changes nothing.
    let before = memory
        .create_snapshot(&site, "empty", Timestamp(1_500))
        .expect("snapshot");
    let _ = before;
    let snapshots = memory.snapshots(&site).expect("snapshots");
    assert_eq!(snapshots.len(), 1);
    ops::run_intent(
        &app,
        &owner().context(&app),
        &Intent::CreateRecord {
            object: event(),
            fields: std::collections::BTreeMap::from([(
                strata_kernel::FieldId::new("name").expect("field"),
                strata_kernel::Value::Str("second".into()),
            )]),
        },
        false,
    )
    .expect("a second record is appended");
    let restore = memory.restore_snapshot(&site, snapshots[0].id, None);
    assert!(
        matches!(&restore, Err(strata_storage_api::StoreError::Invalid(m)) if m.starts_with("AppendOnly")),
        "{restore:?}"
    );
}
