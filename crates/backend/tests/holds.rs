//! Held operations through the backend (D-8400, D-8406, ADM-1601 to ADM-1604, ADM-1614): a change
//! a hold rule names is answered as held, every step of a hold is an audit entry, the decision
//! commits once when two approvers race, and each refused decision is audited with its kind. The
//! kernel's half, the rules themselves, is `crates/kernel/tests/holds.rs`.
//!
//! The tenant is `alpha`, owned by `alpha:owner`, on the in-memory store. The changes go through
//! the functions the routes call (`operations::invoke`, `holds::decide`, `ops::run_intent`).

use serde_json::{json, Value as Json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use strata_backend::app::{ApiError, App, Config, Shared};
use strata_backend::auth::Identity;
use strata_backend::holds::{decide, list, DecideRequest};
use strata_backend::mail::DevSink;
use strata_backend::{operations, ops};
use strata_kernel::*;
use strata_protocol::api::{ErrorCode, OperationRequest};
use strata_protocol::ws::SessionState;
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, SessionRecord};

const APPROVER: &str = "change-approve";

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
    }
}

fn note_field(id: &str) -> FieldDef {
    FieldDef {
        id: FieldId::new(id).expect("field"),
        label: id.to_string(),
        ty: FieldType::Str,
        required: true,
        unique: false,
        default: None,
        read: None,
        personal: None,
        normalise: None,
        computed: None,
    }
}

/// `alpha` with a record type `note`, the operation `file_note` (it creates a note with the slug
/// it is given, for any signed-in person), the rule `hold_file_note` that holds it for the
/// capability change-approve, the role `approver` that grants it, and two approvers.
fn started() -> (Shared, Arc<MemoryStore>, TenantId) {
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
        },
    )
    .expect("the tenant is created");
    let owner = identity(&app, &site, "alpha:owner", "t-setup");
    let object = ObjectTypeId::new("note").expect("object");
    let operation = TenantOperationId::new("file_note").expect("operation");
    let changes = vec![
        SchemaChange::DefineObject {
            id: object.clone(),
            label: "Note".into(),
        },
        SchemaChange::DefineField {
            object: object.clone(),
            field: note_field("slug"),
        },
        SchemaChange::DefineRole(RoleDef {
            id: RoleId::new("approver").expect("role"),
            label: "Approver".into(),
        }),
        SchemaChange::DefineRolePermissions {
            role: RoleId::new("approver").expect("role"),
            permissions: vec![Permission::Capability(APPROVER.into())],
        },
        SchemaChange::DefineOperation(TenantOperationDef {
            id: operation.clone(),
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
                object,
                fields: BTreeMap::from([(
                    FieldId::new("slug").expect("field"),
                    Expr::Param("slug".into()),
                )]),
            }],
        }),
        SchemaChange::DefineHoldRule(HoldRule {
            id: HoldRuleId::new("hold_file_note").expect("rule"),
            label: "File a note needs approval".into(),
            target: HoldTarget::Operation { operation },
            when: None,
            approver: CapabilityName(APPROVER.into()),
        }),
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
    .expect("the schema is defined");
    for boss in ["alpha:boss", "alpha:boss2"] {
        ops::run_intent(
            &app,
            &owner.context(&app),
            &Intent::AssignRole {
                principal: who(boss),
                role: RoleId::new("approver").expect("role"),
            },
            false,
        )
        .expect("the role is assigned");
    }
    (app, memory, site)
}

/// `alpha:maker` files a note through the operation, which the rule holds. Returns the hold.
fn file_a_note(app: &App, site: &TenantId, slug: &str, key: &str) -> String {
    let maker = identity(app, site, "alpha:maker", "t-file");
    let reply = operations::invoke(
        app,
        &maker,
        &OperationRequest {
            operation: "file_note".into(),
            inputs: slug_input(slug),
            dry_run: false,
        },
        key,
    )
    .expect("the invocation is answered");
    let held = reply.held.expect("the invocation is held");
    assert!(reply.result.created.is_empty(), "nothing was created");
    held.hold
}

fn slug_input(slug: &str) -> serde_json::Map<String, Json> {
    json!({ "slug": slug })
        .as_object()
        .expect("an object")
        .clone()
}

fn notes(app: &App, site: &TenantId) -> usize {
    let object = ObjectTypeId::new("note").expect("object");
    app.store
        .load(site, &[Need::AllOf(object.clone())])
        .expect("the tenant loads")
        .tenant
        .expect("the tenant exists")
        .records
        .get(&object)
        .map_or(0, BTreeMap::len)
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

fn approve(hold: &str, read_revision: u64) -> DecideRequest {
    DecideRequest {
        hold: hold.into(),
        decision: "approve".into(),
        read_revision,
        reason: None,
    }
}

fn kind_of(error: &ApiError) -> Option<&str> {
    error.0.kind.as_deref()
}

// Verifies: ADM-1601
// Verifies: ADM-1604
#[test]
fn filing_and_approving_a_change_are_audit_entries_and_the_change_applies_once() {
    let (app, _memory, site) = started();
    let hold = file_a_note(&app, &site, "first", "k-1");
    assert_eq!(notes(&app, &site), 0, "a held change changes no record");

    let filed = entries(&app, &site, "hold.filed");
    assert_eq!(filed.len(), 1, "{filed:?}");
    assert_eq!(filed[0]["hold"], hold);
    assert_eq!(filed[0]["requester"], "alpha:maker");
    assert_eq!(filed[0]["approver"], APPROVER);
    assert_eq!(filed[0]["rule"], "hold_file_note");
    assert_eq!(filed[0]["change"], "invoke_operation");
    assert_eq!(filed[0]["input_hash"].as_str().map(str::len), Some(64));
    assert!(filed[0]["expires_at"].as_i64().is_some());

    // The approver sees it, the requester sees it as their own, and nobody else sees it.
    let boss = identity(&app, &site, "alpha:boss", "t-list");
    let seen = list(&app, &boss).expect("the list");
    assert_eq!(seen["decidable"][0]["hold"], hold);
    assert_eq!(seen["decidable"][0]["revision"], 1);
    let maker = identity(&app, &site, "alpha:maker", "t-list-own");
    let own = list(&app, &maker).expect("the list");
    assert_eq!(own["decidable"], json!([]));
    assert_eq!(own["own"][0]["hold"], hold);
    let stranger = identity(&app, &site, "alpha:stranger", "t-list-none");
    let none = list(&app, &stranger).expect("the list");
    assert_eq!(none, json!({ "decidable": [], "own": [] }));

    let reply = decide(
        &app,
        &identity(&app, &site, "alpha:boss", "t-approve"),
        &approve(&hold, 1),
    )
    .expect("approved");
    assert_eq!((reply.state.as_str(), reply.revision), ("applied", 2));
    assert_eq!(notes(&app, &site), 1, "the approved change applied");

    let decided = entries(&app, &site, "hold.decided");
    assert_eq!(decided.len(), 1, "{decided:?}");
    assert_eq!(decided[0]["state"], "applied");
    assert_eq!(decided[0]["decided_by"], "alpha:boss");
    assert_eq!(decided[0]["requester"], "alpha:maker");

    // And once only: a second approval, by anyone, is a conflict, and the notes are still one.
    let again = decide(
        &app,
        &identity(&app, &site, "alpha:boss2", "t-approve-again"),
        &approve(&hold, 1),
    )
    .expect_err("the hold was decided");
    assert_eq!(again.0.code, ErrorCode::Conflict);
    assert_eq!(kind_of(&again), Some("VersionConflict"));
    assert_eq!(notes(&app, &site), 1);
}

// Verifies: ADM-1602
// Verifies: ADM-1603
#[test]
fn a_refused_decision_is_audited_with_the_kind_of_its_refusal() {
    let (app, _memory, site) = started();
    let hold = file_a_note(&app, &site, "first", "k-1");

    // A member that is no approver.
    let stranger = decide(
        &app,
        &identity(&app, &site, "alpha:stranger", "t-stranger"),
        &approve(&hold, 1),
    )
    .expect_err("not an approver");
    assert_eq!(stranger.0.code, ErrorCode::Forbidden);
    assert_eq!(kind_of(&stranger), Some("ApproverPermission"));

    // An approver who is the requester: boss files, boss decides.
    let boss = identity(&app, &site, "alpha:boss", "t-boss-files");
    let own = operations::invoke(
        &app,
        &boss,
        &OperationRequest {
            operation: "file_note".into(),
            inputs: slug_input("mine"),
            dry_run: false,
        },
        "k-boss",
    )
    .expect("filed")
    .held
    .expect("held")
    .hold;
    let itself = decide(
        &app,
        &identity(&app, &site, "alpha:boss", "t-boss-decides"),
        &approve(&own, 1),
    )
    .expect_err("their own request");
    assert_eq!(itself.0.code, ErrorCode::Forbidden);
    assert_eq!(kind_of(&itself), Some("SelfAction"));

    // A hold nobody decided within seven days: the decision is a conflict naming HoldExpired.
    let mut late = identity(&app, &site, "alpha:boss", "t-late").context(&app);
    late.now = Timestamp(late.now.0 + 8 * 24 * 60 * 60 * 1000);
    let expired = ops::run_intent(
        &app,
        &late,
        &Intent::DecideHold {
            hold: HeldOperationId::new(hold.as_str()).expect("hold id"),
            decision: HoldDecision::Approve,
            read_revision: 1,
            reason: None,
        },
        false,
    )
    .expect_err("expired");
    assert_eq!(expired.0.code, ErrorCode::Conflict);
    assert_eq!(kind_of(&expired), Some("HoldExpired"));

    // Each refusal left an entry; nothing changed.
    let refused = entries(&app, &site, "hold.refused");
    let kinds: Vec<&str> = refused
        .iter()
        .map(|entry| entry["kind"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(kinds, ["ApproverPermission", "SelfAction", "HoldExpired"]);
    assert_eq!(refused[0]["actor"], "alpha:stranger");
    assert_eq!(refused[0]["hold"], hold);
    assert_eq!(notes(&app, &site), 0);
    assert!(entries(&app, &site, "hold.decided").is_empty());
}

// Verifies: ADM-1614
#[test]
fn two_approvers_that_race_commit_one_decision() {
    let (app, _memory, site) = started();
    let hold = file_a_note(&app, &site, "first", "k-1");

    // Both read revision 1 and decide at the same moment, one to approve and one to reject.
    let results: Vec<Result<String, ApiError>> = std::thread::scope(|scope| {
        let handles: Vec<_> = [("alpha:boss", "approve"), ("alpha:boss2", "reject")]
            .into_iter()
            .map(|(who, decision)| {
                let (app, site, hold) = (&app, &site, hold.clone());
                scope.spawn(move || {
                    decide(
                        app,
                        &identity(app, site, who, &format!("t-race-{who}")),
                        &DecideRequest {
                            hold,
                            decision: decision.into(),
                            read_revision: 1,
                            reason: None,
                        },
                    )
                    .map(|reply| reply.state)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("the decision ran"))
            .collect()
    });
    let won: Vec<&String> = results.iter().filter_map(|r| r.as_ref().ok()).collect();
    let lost: Vec<&ApiError> = results.iter().filter_map(|r| r.as_ref().err()).collect();
    assert_eq!((won.len(), lost.len()), (1, 1), "{results:?}");
    assert_eq!(lost[0].0.code, ErrorCode::Conflict);
    assert_eq!(kind_of(lost[0]), Some("VersionConflict"));
    // The one decision that took effect is the one the record shows, and the change applied if and
    // only if it was the approval.
    let decided = entries(&app, &site, "hold.decided");
    assert_eq!(decided.len(), 1, "{decided:?}");
    assert_eq!(decided[0]["state"], won[0].as_str());
    assert_eq!(notes(&app, &site), usize::from(won[0] == "applied"));
    assert_eq!(entries(&app, &site, "hold.refused").len(), 1);
}
