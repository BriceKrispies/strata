//! The ledger's fourth wave through the backend: the expiry of credits (the job of each tenant and
//! its operator route), the verification of a balance against its journal, the limits of single
//! accounts held outside the schema, and what no other tenant can reach. Driven synchronously
//! through `worker::tick` and `ledger::handle`, on the in-memory store, with the kernel's notes
//! fixture.

use serde_json::{json, Value as Json};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use strata_backend::app::{ApiError, App, Config, Shared};
use strata_backend::auth::Identity;
use strata_backend::ledger::{self, EXPIRY_INTERVAL_MS};
use strata_backend::mail::DevSink;
use strata_backend::worker::{self, leases, steps, WorkerConfig};
use strata_kernel::ledger as kernel_ledger;
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_protocol::api::ErrorCode;
use strata_protocol::ws::SessionState;
use strata_storage_api::jobs::{JobState, JobWork, NewJob, RetryPolicy};
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{execute, Commit, SessionRecord, Store};

/// A Thursday, on the hour: 2025-11-20T12:00:00Z.
const T0: i64 = 1_763_640_000_000;
const MINUTE: i64 = 60_000;
const HOUR: i64 = 3_600_000;

fn app() -> Shared {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cfg = Config {
        addr: "127.0.0.1:0".parse().unwrap(),
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
    App::new(cfg, Arc::new(MemoryStore::new()))
}

fn points() -> LedgerUnit {
    LedgerUnit::new("points").unwrap()
}

fn person(tenant: &str, who: &str) -> LedgerOwner {
    LedgerOwner::Principal {
        principal: principal_id(&format!("{tenant}:{who}")),
    }
}

fn hourly() -> LedgerExpiry {
    LedgerExpiry {
        period: LedgerPeriod::Hour,
        periods: 1,
    }
}

/// A tenant with the notes schema and the unit `points` declared with the given expiry.
fn install(app: &App, tenant: &str, expiry: Option<LedgerExpiry>) {
    let store = app.store.as_ref();
    let owner = principal_id(&format!("{tenant}:owner"));
    let mut now = 100;
    let mut run = |principal: Principal, intent: Intent| {
        now += 1;
        execute(store, &intent, &ctx(tenant, principal, now)).expect("fixture intent");
    };
    run(
        Principal::System,
        Intent::CreateTenant {
            name: tenant.to_string(),
            owner: owner.clone(),
        },
    );
    let mut changes = notes_schema();
    changes.push(SchemaChange::DeclareLedgerUnit(LedgerUnitDecl {
        unit: points(),
        description: "Points".into(),
        expiry,
    }));
    run(
        Principal::User(owner.clone()),
        Intent::ChangeSchema {
            changes,
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    for who in ["writer1"] {
        run(
            Principal::User(owner.clone()),
            Intent::AssignRole {
                principal: principal_id(&format!("{tenant}:{who}")),
                role: role("writer"),
            },
        );
    }
}

fn post(app: &App, tenant: &str, owner: LedgerOwner, scope: &str, amount: i64, key: &str, at: i64) {
    execute(
        app.store.as_ref(),
        &Intent::PostLedger {
            postings: vec![LedgerPostingRequest {
                owner,
                unit: points(),
                amount: LedgerAmount(amount),
                idempotency_key: key.into(),
                reason: "test".into(),
                corrects: None,
                scope: (!scope.is_empty()).then(|| LedgerScope(scope.to_string())),
            }],
        },
        &ctx(
            tenant,
            Principal::User(principal_id(&format!("{tenant}:owner"))),
            at,
        ),
    )
    .unwrap_or_else(|e| panic!("posting {key} failed: {e}"));
}

fn account(tenant: &str, owner: &LedgerOwner) -> LedgerAccountId {
    kernel_ledger::account_id(&tenant_id(tenant), owner, &points(), "")
}

/// The header and the journal of an account of `points` in the empty scope.
fn state_of(
    app: &App,
    tenant: &str,
    owner: &LedgerOwner,
) -> (Option<LedgerAccount>, Vec<LedgerEntry>) {
    let state = app.store.load_complete(&tenant_id(tenant)).unwrap();
    let id = account(tenant, owner);
    let key = kernel_ledger::account_key(&id);
    let ledger = state.tenant.and_then(|t| t.ledger.clone());
    (
        ledger.as_ref().and_then(|l| l.accounts.get(&key).cloned()),
        ledger
            .as_ref()
            .and_then(|l| l.entries.get(&key).cloned())
            .unwrap_or_default(),
    )
}

fn at(t: i64) -> impl Fn() -> Timestamp {
    move || Timestamp(t)
}

fn config() -> WorkerConfig {
    WorkerConfig::from_limits(&Default::default())
}

fn signed_in(tenant: &str, who: &str) -> Identity {
    let principal = principal_id(&format!("{tenant}:{who}"));
    Identity {
        tenant: tenant_id(tenant),
        principal: Principal::User(principal.clone()),
        session: Some(SessionRecord {
            id: SessionId::new(format!("s-{who}")).unwrap(),
            token_hash: format!("hash-{who}"),
            csrf_token: "csrf".into(),
            tenant: tenant_id(tenant),
            principal,
            display_name: who.to_string(),
            created_at: Timestamp(0),
            last_seen_at: Timestamp(0),
            expires_at: Timestamp(i64::MAX),
            revoked_at: None,
            authenticated_at: Timestamp(0),
            network: None,
        }),
        state: SessionState::Active,
        correlation: "t-ledger-more".into(),
        verified_address: None,
        idempotency_key: None,
    }
}

fn expect_code(result: Result<Json, ApiError>, code: ErrorCode) {
    match result {
        Err(error) => assert_eq!(format!("{:?}", error.0.code), format!("{code:?}")),
        Ok(body) => panic!("expected {code:?}, got {body}"),
    }
}

// ------------------------------------------------------------------------------------------------
// The expiry job

/// Verifies: LDG-1546, LDG-1547
///
/// The job of a tenant expires what is due once: an attempt that ran and did not report (the lease
/// expired) is run again by another worker and posts nothing a second time, and the job that
/// follows is created when the run is reported.
#[test]
fn the_expiry_job_posts_each_due_credit_once_even_when_it_is_run_again() {
    let app = app();
    install(&app, "alpha", Some(hourly()));
    let alpha = tenant_id("alpha");
    let writer = person("alpha", "writer1");
    // 10 in the hour of noon, 5 in the hour after it.
    post(&app, "alpha", writer.clone(), "", 10, "a", T0 + MINUTE);
    post(
        &app,
        "alpha",
        writer.clone(),
        "",
        5,
        "b",
        T0 + HOUR + MINUTE,
    );
    let now = T0 + HOUR + 2 * MINUTE;
    app.store
        .jobs_create(
            &alpha,
            &[NewJob {
                key: "ledger-expiry:start".into(),
                work: JobWork::ExpireLedgerCredits {},
                due_at: Timestamp(now),
                policy: RetryPolicy::default(),
            }],
            Timestamp(now),
        )
        .unwrap();
    // One worker runs the job and stops before it reports.
    let first = app
        .store
        .job_lease(&alpha, "w1", Timestamp(now))
        .unwrap()
        .expect("the job is due");
    let _lost = steps::run_job(&app, &alpha, &first, Timestamp(now));
    let (header, journal) = state_of(&app, "alpha", &writer);
    assert_eq!(journal.len(), 3, "the credit of the noon hour expired");
    assert_eq!(header.unwrap().balance, LedgerAmount(5));
    // Its lease expires; another worker takes the job and runs it again.
    let later = first.expires_at.0 + 1;
    let second = app
        .store
        .job_lease(&alpha, "w2", Timestamp(later))
        .unwrap()
        .expect("the lease expired, so the job is offered again");
    leases::run_leased(&app, &alpha, &second, &at(later)).unwrap();
    let (_, journal) = state_of(&app, "alpha", &writer);
    assert_eq!(journal.len(), 3, "nothing was posted twice");
    let expiries: Vec<_> = journal
        .iter()
        .filter(|e| e.idempotency_key.starts_with("expiry:"))
        .collect();
    assert_eq!(expiries.len(), 1);
    assert_eq!(
        (
            expiries[0].amount.0,
            expiries[0].reason.as_str(),
            expiries[0].caused_by.clone()
        ),
        (-10, "expiry", None)
    );
    // The run created the next job, due an interval later.
    let jobs = app.store.jobs_list(&alpha, None, 10).unwrap();
    let next = jobs
        .iter()
        .find(|j| j.key.starts_with("ledger-expiry:") && j.key != "ledger-expiry:start")
        .expect("the job that follows");
    assert_eq!(next.state, JobState::Waiting);
    assert_eq!(next.due_at.0, now + EXPIRY_INTERVAL_MS);
    // An interval on, the credit of the second hour is due; the pass after it posts nothing more.
    let then = now + EXPIRY_INTERVAL_MS;
    let pass = worker::tick(&app, "w1", &config(), &at(then));
    assert_eq!(pass.jobs, 1);
    let (header, journal) = state_of(&app, "alpha", &writer);
    assert_eq!(journal.len(), 4);
    assert_eq!(header.unwrap().balance, LedgerAmount(0));
    let pass = worker::tick(&app, "w1", &config(), &at(then + MINUTE));
    assert_eq!(pass.jobs, 0, "the next job is not due yet");
    let (_, journal) = state_of(&app, "alpha", &writer);
    assert_eq!(journal.len(), 4);
}

/// Verifies: LDG-1546
#[test]
fn declaring_a_unit_with_an_expiry_starts_the_chain_of_jobs_once() {
    let app = app();
    install(&app, "alpha", None);
    let alpha = tenant_id("alpha");
    let owner = signed_in("alpha", "owner");
    let declare = |unit: &str| {
        ledger::handle(
            &app,
            &owner,
            "declare-unit",
            json!({ "unit": unit, "description": unit, "expiry": { "period": "day", "periods": 30 } }),
        )
        .unwrap()
    };
    declare("stamps");
    declare("badges");
    let started: Vec<_> = app
        .store
        .jobs_list(&alpha, None, 10)
        .unwrap()
        .into_iter()
        .filter(|j| matches!(j.work, JobWork::ExpireLedgerCredits {}))
        .collect();
    assert_eq!(
        started.len(),
        1,
        "a second declaration starts no second chain"
    );
    assert_eq!(started[0].key, "ledger-expiry:start");
    // A unit that does not expire starts none, and an expiry out of range is refused.
    expect_code(
        ledger::handle(
            &app,
            &owner,
            "declare-unit",
            json!({ "unit": "tokens", "description": "t", "expiry": { "period": "day", "periods": 99 } }),
        ),
        ErrorCode::Invalid,
    );
}

/// Verifies: LDG-1547
#[test]
fn the_expiry_route_handles_a_hundred_accounts_a_call_and_only_for_those_who_may() {
    let app = app();
    install(&app, "alpha", Some(hourly()));
    let writer = person("alpha", "writer1");
    let base = app.now().0;
    for n in 0..130 {
        post(
            &app,
            "alpha",
            writer.clone(),
            &format!("s{n}"),
            3,
            &format!("k{n}"),
            base,
        );
    }
    let owner = signed_in("alpha", "owner");
    // Nothing is due yet: the bucket of this hour has not ended its life.
    let none = ledger::handle(&app, &owner, "expire", json!({ "unit": "points" })).unwrap();
    assert_eq!(none["posted"], 0);
    // Three hours on, it is.
    app.clock_skew_ms.fetch_add(3 * HOUR, Ordering::Relaxed);
    let writer_identity = signed_in("alpha", "writer1");
    expect_code(
        ledger::handle(
            &app,
            &writer_identity,
            "expire",
            json!({ "unit": "points" }),
        ),
        ErrorCode::Forbidden,
    );
    let first = ledger::handle(&app, &owner, "expire", json!({ "unit": "points" })).unwrap();
    assert_eq!(first["posted"], 100);
    assert!(first["next"].is_string(), "{first}");
    let second = ledger::handle(&app, &owner, "expire", json!({ "unit": "points" })).unwrap();
    assert_eq!(second["posted"], 30);
    assert!(second["next"].is_null(), "{second}");
    let third = ledger::handle(&app, &owner, "expire", json!({ "unit": "points" })).unwrap();
    assert_eq!(third["posted"], 0);
}

// ------------------------------------------------------------------------------------------------
// Verification

/// Verifies: LDG-019, LDG-020
///
/// The route replays the journal of an account against its header: it agrees for a ledger that was
/// written as the kernel writes it, and does not for a header that was changed by hand.
#[test]
fn verify_agrees_with_a_sound_ledger_and_detects_a_tampered_header() {
    let app = app();
    install(&app, "alpha", None);
    let alpha = tenant_id("alpha");
    let writer = person("alpha", "writer1");
    post(&app, "alpha", writer.clone(), "", 12, "a", T0);
    post(&app, "alpha", writer.clone(), "", -5, "b", T0 + 1);
    let id = account("alpha", &writer);
    let owner = signed_in("alpha", "owner");
    let sound = ledger::handle(&app, &owner, "verify", json!({ "account": id })).unwrap();
    assert_eq!(sound["ok"], true, "{sound}");
    assert_eq!(
        (sound["stored"].clone(), sound["replayed"].clone()),
        (json!(7), json!(7))
    );
    assert_eq!(sound["entries"], 2);
    assert!(sound["gap"].is_null());

    // A third posting whose header says one more than its entries add up to.
    let state = app.store.load_complete(&alpha).unwrap();
    let intent = Intent::PostLedger {
        postings: vec![LedgerPostingRequest {
            owner: writer.clone(),
            unit: points(),
            amount: LedgerAmount(3),
            idempotency_key: "c".into(),
            reason: "test".into(),
            corrects: None,
            scope: None,
        }],
    };
    let context = ctx(
        "alpha",
        Principal::User(principal_id("alpha:owner")),
        T0 + 2,
    );
    let mut decided = transition(&state, &intent, &context).unwrap();
    for op in decided.delta.ops.iter_mut() {
        if let DeltaOp::Ledger(LedgerDelta::PutAccount(header)) = op {
            header.balance = LedgerAmount(header.balance.0 + 1);
        }
    }
    app.store
        .commit(&Commit::of(&intent, &context, &decided))
        .unwrap();
    let broken = ledger::handle(&app, &owner, "verify", json!({ "account": id })).unwrap();
    assert_eq!(broken["ok"], false, "{broken}");
    assert_eq!(
        (broken["stored"].clone(), broken["replayed"].clone()),
        (json!(11), json!(10))
    );
    // Only a holder of ledger-read may verify, and an account that is not there is not found.
    expect_code(
        ledger::handle(
            &app,
            &signed_in("alpha", "writer1"),
            "verify",
            json!({ "account": id }),
        ),
        ErrorCode::Forbidden,
    );
    let missing = account("alpha", &person("alpha", "nobody"));
    expect_code(
        ledger::handle(&app, &owner, "verify", json!({ "account": missing })),
        ErrorCode::NotFound,
    );
}

// ------------------------------------------------------------------------------------------------
// Limits outside the schema

/// Verifies: LDG-011, LDG-1548, LDG-1549, LDG-1550
#[test]
fn an_account_limit_is_set_listed_and_removed_and_the_schema_does_not_carry_it() {
    let app = app();
    install(&app, "alpha", None);
    let alpha = tenant_id("alpha");
    let owner = signed_in("alpha", "owner");
    let writer = json!({ "principal": { "principal": "alpha:writer1" } });
    ledger::handle(
        &app,
        &owner,
        "set-limit",
        json!({ "limit": { "unit": "points", "owner": writer, "cap": 50 } }),
    )
    .unwrap();
    // The unit's own limit stays in the schema.
    ledger::handle(
        &app,
        &owner,
        "set-limit",
        json!({ "limit": { "unit": "points", "floor": 0 } }),
    )
    .unwrap();
    let state = app.store.load(&alpha, &[]).unwrap();
    let tenant = state.tenant.unwrap();
    assert_eq!(
        tenant.schema.ledger.as_ref().unwrap().limits.len(),
        1,
        "the schema holds the limit of the unit and not the one of the account"
    );
    assert!(
        tenant.ledger.is_none(),
        "a schema load carries no account limit"
    );
    // Both are listed.
    let listed = ledger::handle(&app, &owner, "definitions", json!({})).unwrap();
    assert_eq!(listed["definitions"]["limits"].as_array().unwrap().len(), 2);
    // The limit binds its account and no other.
    let posting = |who: &str, amount: i64, key: &str| {
        ledger::handle(
            &app,
            &owner,
            "post",
            json!({
                "owner": { "principal": { "principal": who } },
                "unit": "points", "amount": amount, "idempotency_key": key, "reason": "test"
            }),
        )
    };
    expect_code(posting("alpha:writer1", 51, "a"), ErrorCode::Invalid);
    posting("alpha:writer1", 50, "b").unwrap();
    posting("alpha:owner", 51, "c").unwrap();
    // Removing it frees the account, and removing it again is refused.
    let remove = json!({ "definition": { "limit": { "unit": "points", "owner": writer } } });
    ledger::handle(&app, &owner, "remove-definition", remove.clone()).unwrap();
    expect_code(
        ledger::handle(&app, &owner, "remove-definition", remove),
        ErrorCode::Invalid,
    );
    posting("alpha:writer1", 10, "d").unwrap();
    let listed = ledger::handle(&app, &owner, "definitions", json!({})).unwrap();
    assert_eq!(listed["definitions"]["limits"].as_array().unwrap().len(), 1);
}

// ------------------------------------------------------------------------------------------------
// Tenants

/// Verifies: TEN-1140
#[test]
fn another_tenant_reaches_nothing_of_a_tenants_ledger_and_its_job_touches_nothing_of_theirs() {
    let app = app();
    install(&app, "alpha", Some(hourly()));
    install(&app, "beta", Some(hourly()));
    let alpha_writer = person("alpha", "writer1");
    let beta_writer = person("beta", "writer1");
    post(
        &app,
        "alpha",
        alpha_writer.clone(),
        "",
        10,
        "a",
        T0 + MINUTE,
    );
    post(&app, "beta", beta_writer.clone(), "", 20, "b", T0 + MINUTE);
    // The owner of beta cannot verify an account of alpha, or name alpha in a request.
    let alpha_account = account("alpha", &alpha_writer);
    let beta_owner = signed_in("beta", "owner");
    expect_code(
        ledger::handle(
            &app,
            &beta_owner,
            "verify",
            json!({ "account": alpha_account }),
        ),
        ErrorCode::NotFound,
    );
    expect_code(
        ledger::handle(
            &app,
            &beta_owner,
            "verify",
            json!({ "tenant": "alpha", "account": account("beta", &beta_writer) }),
        ),
        ErrorCode::Forbidden,
    );
    expect_code(
        ledger::handle(
            &app,
            &beta_owner,
            "expire",
            json!({ "tenant": "alpha", "unit": "points" }),
        ),
        ErrorCode::Forbidden,
    );
    // The expiry job of alpha expires alpha's credit and leaves beta's alone.
    app.store
        .jobs_create(
            &tenant_id("alpha"),
            &[NewJob {
                key: "ledger-expiry:start".into(),
                work: JobWork::ExpireLedgerCredits {},
                due_at: Timestamp(T0),
                policy: RetryPolicy::default(),
            }],
            Timestamp(T0),
        )
        .unwrap();
    let pass = worker::tick(&app, "w1", &config(), &at(T0 + 3 * HOUR));
    assert_eq!(pass.tenants, 1, "only alpha has a job");
    assert_eq!(
        state_of(&app, "alpha", &alpha_writer).0.unwrap().balance,
        LedgerAmount(0)
    );
    let (beta_header, beta_journal) = state_of(&app, "beta", &beta_writer);
    assert_eq!(beta_header.unwrap().balance, LedgerAmount(20));
    assert_eq!(beta_journal.len(), 1);
}
