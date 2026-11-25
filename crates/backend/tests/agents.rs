//! Agent governance through the backend (D-8401, D-8424, ADM-1605 to ADM-1608): the identity of an
//! agent's request carries the agent into the kernel's context, every refusal of an agent's call is
//! an audit entry naming the agent and the person it acted for, and a call whose policy is
//! approval is filed as a hold on behalf of that person. The kernel's half, the rules themselves,
//! is `crates/kernel/tests/agents.rs`.
//!
//! The tenant is `alpha`, owned by `alpha:owner`, on the in-memory store.

use serde_json::{json, Value as Json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use strata_backend::app::{App, Config, Shared};
use strata_backend::auth::Identity;
use strata_backend::holds::{decide, DecideRequest};
use strata_backend::mail::DevSink;
use strata_backend::service::tokens::{agent_call, Act, Claims, Verified};
use strata_backend::{operations, ops};
use strata_kernel::*;
use strata_protocol::api::{ErrorCode, OperationRequest};
use strata_protocol::ws::SessionState;
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, SessionRecord};

const AGENT: &str = "agent:bot";

fn who(name: &str) -> PrincipalId {
    PrincipalId::new(name).expect("principal id")
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
        agent: None,
    }
}

/// The identity of the agent `agent:bot` acting for `person` with a token of `scopes`.
fn as_agent(
    app: &App,
    site: &TenantId,
    person: &str,
    scopes: &[&str],
    correlation: &str,
) -> Identity {
    identity(app, site, person, correlation).acting_through(AgentCall {
        agent: who(AGENT),
        scopes: scopes.iter().map(|s| s.to_string()).collect(),
    })
}

fn object() -> ObjectTypeId {
    ObjectTypeId::new("note").expect("object")
}

fn note_operation(id: &str, policy: Option<AgentPolicy>) -> TenantOperationDef {
    TenantOperationDef {
        id: TenantOperationId::new(id).expect("operation"),
        label: "File a note".into(),
        inputs: vec![TenantOperationInput {
            name: "slug".into(),
            ty: FieldType::Str,
            required: true,
            label: None,
        }],
        invokers: TenantOperationInvokers {
            signed_in: true,
            roles: BTreeSet::new(),
        },
        subject: None,
        conditions: vec![],
        steps: vec![TenantOperationStep::CreateRecord {
            id: "made".into(),
            object: object(),
            fields: BTreeMap::from([(
                FieldId::new("slug").expect("field"),
                Expr::Param("slug".into()),
            )]),
        }],
        agent_policy: policy,
    }
}

fn role_with(id: &str, permissions: Vec<Permission>) -> Vec<SchemaChange> {
    vec![
        SchemaChange::DefineRole(RoleDef {
            id: RoleId::new(id).expect("role"),
            label: id.into(),
        }),
        SchemaChange::DefineRolePermissions {
            role: RoleId::new(id).expect("role"),
            permissions,
        },
    ]
}

/// `alpha` with a record type `note`; the roles `bot` (held by `agent:bot`: it creates notes and
/// writes tenant settings), `approver` (the capability agent-approve, held by `alpha:boss`) and
/// `admin` (a capability only an administrator holds); the operations `held_note` (approval) and
/// `denied_note` (deny); and the setting `page_size`.
fn started() -> (Shared, TenantId) {
    let memory = Arc::new(MemoryStore::new());
    let app = app_on(memory.clone());
    let site = TenantId::new("alpha").expect("tenant");
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
            agent: None,
        },
    )
    .expect("the tenant is created");
    let owner = identity(&app, &site, "alpha:owner", "t-setup");
    let mut changes = vec![
        SchemaChange::DefineObject {
            id: object(),
            label: "Note".into(),
        },
        SchemaChange::DefineField {
            object: object(),
            field: FieldDef {
                id: FieldId::new("slug").expect("field"),
                label: "slug".into(),
                ty: FieldType::Str,
                required: true,
                unique: false,
                default: None,
                read: None,
                personal: None,
                normalise: None,
                computed: None,
            },
        },
    ];
    changes.extend(role_with(
        "bot",
        vec![
            Permission::Object {
                object: object(),
                actions: BTreeSet::from([ObjectAction::Create]),
            },
            Permission::Capability("setting-write-tenant".into()),
        ],
    ));
    changes.extend(role_with(
        "approver",
        vec![Permission::Capability("agent-approve".into())],
    ));
    changes.extend(role_with(
        "admin",
        vec![Permission::Capability("holds-define".into())],
    ));
    changes.push(SchemaChange::DefineOperation(note_operation(
        "held_note",
        Some(AgentPolicy::Approval),
    )));
    changes.push(SchemaChange::DefineOperation(note_operation(
        "denied_note",
        Some(AgentPolicy::Deny),
    )));
    changes.push(SchemaChange::DefineSetting(SettingDef {
        id: SettingId::new("page_size").expect("setting"),
        label: "Page size".into(),
        r#type: SettingType::new("i64").expect("type"),
        default: Value::Int(20),
        levels: BTreeSet::from([SettingLevel::Tenant]),
        locked_by: None,
        area: None,
        description: None,
        allowed: None,
    }));
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
    .expect("the schema is defined");
    for (principal, role) in [(AGENT, "bot"), ("alpha:boss", "approver")] {
        ops::run_intent(
            &app,
            &owner.context(&app),
            &Intent::AssignRole {
                principal: who(principal),
                role: RoleId::new(role).expect("role"),
            },
            false,
        )
        .expect("the role is assigned");
    }
    (app, site)
}

fn entries(app: &App, site: &TenantId, event: &str) -> Vec<Json> {
    app.store
        .audit_list(site, Some("audit"), 1_000)
        .expect("the audit trail")
        .into_iter()
        .filter(|line| line.event == event)
        .map(|line| serde_json::from_str(&line.data).expect("the data is JSON"))
        .collect()
}

fn notes(app: &App, site: &TenantId) -> usize {
    app.store
        .load(site, &[Need::AllOf(object())])
        .expect("the tenant loads")
        .tenant
        .expect("the tenant exists")
        .records
        .get(&object())
        .map_or(0, BTreeMap::len)
}

fn create_note(slug: &str) -> Intent {
    Intent::CreateRecord {
        object: object(),
        fields: BTreeMap::from([(
            FieldId::new("slug").expect("field"),
            Value::Str(slug.into()),
        )]),
    }
}

fn request(operation: &str, slug: &str) -> OperationRequest {
    OperationRequest {
        operation: operation.into(),
        inputs: json!({ "slug": slug })
            .as_object()
            .expect("an object")
            .clone(),
        dry_run: false,
    }
}

/// The kind of a refusal, and its audit entry: the event, the actor, the agent.
fn assert_refused(
    app: &App,
    site: &TenantId,
    error: &strata_backend::app::ApiError,
    kind: &str,
    event: &str,
) {
    assert_eq!(error.0.code, ErrorCode::Forbidden, "{}", error.describe());
    assert_eq!(error.0.kind.as_deref(), Some(kind));
    let audited = entries(app, site, event);
    assert_eq!(audited.len(), 1, "{event}: {audited:?}");
    assert_eq!(audited[0]["kind"], kind);
    assert_eq!(audited[0]["agent"], AGENT);
    assert_eq!(audited[0]["actor"], "alpha:maker");
}

// Verifies: ADM-1605
#[test]
fn a_call_outside_the_ceiling_is_refused_and_audited_with_the_agent_and_the_person() {
    let (app, site) = started();
    // The token names a scope that does not reach creating a note.
    let agent = as_agent(&app, &site, "alpha:maker", &["read:note"], "t-scope");
    let error = ops::run_intent(&app, &agent.context(&app), &create_note("a"), false)
        .expect_err("outside the intersection");
    assert_refused(
        &app,
        &site,
        &error,
        "AgentCeilingDenies",
        "refused.agent_ceiling_denies",
    );
    assert_eq!(notes(&app, &site), 0);
}

// Verifies: ADM-1606
#[test]
fn a_denied_call_is_audited_and_an_approval_call_is_filed_for_the_person() {
    let (app, site) = started();
    let agent = as_agent(&app, &site, "alpha:maker", &["create:note"], "t-denied");
    let error = operations::invoke(&app, &agent, &request("denied_note", "a"), "k-denied")
        .expect_err("deny");
    assert_refused(&app, &site, &error, "AgentDenied", "refused.agent_denied");

    // Approval: the reply says it is held, nothing is created, and the filing is audited with the
    // agent and the person it acts for.
    let reply =
        operations::invoke(&app, &agent, &request("held_note", "b"), "k-held").expect("filed");
    let held = reply.held.expect("held");
    assert_eq!(held.approver, "agent-approve");
    assert_eq!(notes(&app, &site), 0);
    let filed = entries(&app, &site, "hold.filed");
    assert_eq!(filed.len(), 1, "{filed:?}");
    assert_eq!(filed[0]["requester"], "alpha:maker");
    assert_eq!(filed[0]["on_behalf_of"], "alpha:maker");
    assert_eq!(filed[0]["agent"], AGENT);

    // A dry run says it would be held and changes nothing.
    let mut dry = request("held_note", "c");
    dry.dry_run = true;
    let reply = operations::invoke(&app, &agent, &dry, "k-dry").expect("dry run");
    assert!(reply.held.is_some() && reply.dry_run);

    // A human approver applies it, as the person.
    decide(
        &app,
        &identity(&app, &site, "alpha:boss", "t-approve"),
        &DecideRequest {
            hold: held.hold.clone(),
            decision: "approve".into(),
            read_revision: 1,
            reason: None,
        },
    )
    .expect("approved");
    assert_eq!(notes(&app, &site), 1);
}

// Verifies: ADM-1607
#[test]
fn a_grant_of_an_administration_power_to_an_agent_is_refused_and_audited() {
    let (app, site) = started();
    let owner = identity(&app, &site, "alpha:owner", "t-grant");
    let error = ops::run_intent(
        &app,
        &owner.context(&app),
        &Intent::AssignRole {
            principal: who(AGENT),
            role: RoleId::new("admin").expect("role"),
        },
        false,
    )
    .expect_err("an administration power");
    assert_eq!(error.0.code, ErrorCode::Forbidden);
    assert_eq!(error.0.kind.as_deref(), Some("AgentAdminPower"));
    let audited = entries(&app, &site, "refused.agent_admin_power");
    assert_eq!(audited.len(), 1, "{audited:?}");
    assert_eq!(audited[0]["actor"], "alpha:owner");
    assert_eq!(audited[0]["intent"], "assign_role");
}

// Verifies: ADM-1608
#[test]
fn an_agents_settings_change_is_held_and_applies_when_a_human_decides_it() {
    let (app, site) = started();
    let agent = as_agent(&app, &site, "alpha:owner", &["update:*"], "t-setting");
    let write = Intent::WriteSetting {
        setting: SettingId::new("page_size").expect("setting"),
        level: SettingLevel::Tenant,
        team: None,
        value: Some(Value::Int(50)),
        expected_revision: 0,
    };
    let done = ops::run_intent(&app, &agent.context(&app), &write, false).expect("filed");
    let Output::Held { hold, .. } = &done.transition.output else {
        panic!("not held: {:?}", done.transition.output);
    };
    let value = |app: &App| {
        let state = app
            .store
            .load(&site, &[])
            .expect("loads")
            .tenant
            .expect("tenant");
        strata_kernel::settings::resolve(
            &state,
            &Principal::User(who("alpha:owner")),
            &SettingId::new("page_size").expect("setting"),
        )
        .expect("resolved")
        .value
    };
    assert_eq!(value(&app), Value::Int(20));
    let filed = entries(&app, &site, "hold.filed");
    assert_eq!(filed.len(), 1, "{filed:?}");
    assert_eq!(filed[0]["agent"], AGENT);
    assert_eq!(filed[0]["change"], "write_setting");

    decide(
        &app,
        &identity(&app, &site, "alpha:boss", "t-approve"),
        &DecideRequest {
            hold: hold.as_str().into(),
            decision: "approve".into(),
            read_revision: 1,
            reason: None,
        },
    )
    .expect("approved");
    assert_eq!(value(&app), Value::Int(50));
}

// Verifies: ADM-1605
#[test]
fn the_context_of_an_agent_comes_from_the_act_claim_and_the_scopes_of_its_token() {
    let claims = |act: Option<&str>| Claims {
        iss: "strata".into(),
        aud: "strata:alpha".into(),
        sub: "alpha:maker".into(),
        tid: "alpha".into(),
        azp: "bot".into(),
        scope: "create:note".into(),
        iat: 0,
        exp: 300,
        jti: "j".into(),
        gid: None,
        act: act.map(|sub| Act { sub: sub.into() }),
    };
    let verified = |act| Verified {
        claims: claims(act),
        scopes: vec!["create:note".into()],
    };
    let call = agent_call(&verified(Some("bot"))).expect("an agent");
    assert_eq!(call.agent, who(AGENT));
    assert_eq!(call.scopes, ["create:note"]);
    // A service account's token names no agent.
    assert!(agent_call(&verified(None)).is_none());
}
