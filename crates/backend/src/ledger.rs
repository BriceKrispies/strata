//! The tenant ledger over HTTP: `POST /internal/ledger/<operation>`, behind the service token like
//! every internal route, reached from the BFF's `POST /api/ledger/<operation>`.
//!
//! Each operation is one contract. A posting and a declaration are kernel intents committed by
//! `execute_by` (so a lost race is decided again, D-5009); a read loads what the kernel's ledger
//! module names and lets it answer (D-5011). No ledger rule is written here.
//!
//! | operation           | contract                         |
//! |---------------------|----------------------------------|
//! | `post`              | post-ledger-entry (CT-281)       |
//! | `balance`           | read-ledger-balance (CT-282)     |
//! | `entries`           | list-ledger-entries (CT-283)     |
//! | `declare-unit`      | declare-ledger-unit (CT-284)     |
//! | `declare-pool`      | declare-ledger-pool (CT-285)     |
//! | `set-limit`         | set-ledger-limit (CT-286)        |
//! | `my-balance`        | read-my-ledger-balance (CT-807)  |
//! | `my-entries`        | list-my-ledger-entries (CT-808)  |
//! | `accounts`          | list-ledger-accounts (CT-809)    |
//! | `definitions`       | list-ledger-definitions (CT-810) |
//! | `remove-definition` | remove-ledger-definition (CT-811)|
//! | `verify`            | verify-ledger-balance (CT-287)   |
//! | `expire`            | expire-ledger-credits (CT-812)   |
//!
//! `verify` and `expire` are for operators and tests. The expiry of credits also runs as a job of
//! each tenant that declares a unit with an expiry ([`run_expiry_job`]): the job is created
//! through the job store when such a unit is declared, and each run creates the next one, so
//! nothing depends on a schedule.

use crate::app::{ApiError, App, Shared};
use crate::auth::Identity;
use crate::ops::run_intent;
use crate::worker::steps::Outcome;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value as Json_};
use strata_kernel::ext::DomainErrorExt;
use strata_kernel::ledger;
use strata_kernel::ledger_expiry;
use strata_kernel::{
    Context, DomainError, ErrorKind, ExistingData, Intent, LedgerAccountId, LedgerDefinitionRef,
    LedgerExpiry, LedgerLimit, LedgerNeed, LedgerOrder, LedgerOwner, LedgerPoolDecl,
    LedgerPoolName, LedgerPostingRequest, LedgerScope, LedgerUnit, LedgerUnitDecl, Need, Output,
    Principal, SchemaChange, State as Snapshot, TenantId, Timestamp,
};
use strata_protocol::api::ErrorCode;
use strata_storage_api::jobs::{JobReport, JobWork, Lease, NewJob, RetryPolicy, WorkerWrites};

/// The operations of the route. The BFF names the first eleven; `verify` and `expire` are for
/// operators and tests and are served on the internal route only.
pub const OPERATIONS: [&str; 13] = [
    "post",
    "balance",
    "entries",
    "declare-unit",
    "declare-pool",
    "set-limit",
    "my-balance",
    "my-entries",
    "accounts",
    "definitions",
    "remove-definition",
    "verify",
    "expire",
];

/// How long after one run of the expiry job the next is due (LDG-1546: credit expires no later
/// than one day after the end of its period, so an hour is well inside it).
pub const EXPIRY_INTERVAL_MS: i64 = 3_600_000;
/// The most batches of one unit one run of the expiry job decides; what is left waits for the
/// next run, so a tenant with a great many due accounts does not hold its worker.
const MAX_BATCHES_PER_RUN: usize = 20;
/// The entries of the journal of an account one page of a verification replays.
const VERIFY_PAGE: u32 = 500;
/// The key of the job that starts the chain of the expiry jobs of a tenant.
const EXPIRY_START_KEY: &str = "ledger-expiry:start";

/// The fields every operation may carry: the tenant, which must be the tenant of the request
/// when it is given (`tenant-mismatch`).
#[derive(Debug, Default, Deserialize)]
struct Common {
    #[serde(default)]
    tenant: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PostRequest {
    #[serde(default)]
    tenant: Option<String>,
    owner: LedgerOwner,
    unit: LedgerUnit,
    #[serde(default)]
    scope: Option<LedgerScope>,
    amount: i64,
    idempotency_key: String,
    reason: String,
    #[serde(default)]
    corrects: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountRequest {
    #[serde(default)]
    tenant: Option<String>,
    #[serde(default)]
    owner: Option<LedgerOwner>,
    unit: LedgerUnit,
    #[serde(default)]
    scope: Option<LedgerScope>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    order: Option<LedgerOrder>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UnitRequest {
    #[serde(default)]
    tenant: Option<String>,
    unit: LedgerUnit,
    description: String,
    #[serde(default)]
    expiry: Option<LedgerExpiry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifyRequest {
    #[serde(default)]
    tenant: Option<String>,
    account: LedgerAccountId,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpireRequest {
    #[serde(default)]
    tenant: Option<String>,
    unit: LedgerUnit,
    /// The start of the latest bucket to expire; by default the latest whose life has ended.
    #[serde(default)]
    bucket: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PoolRequest {
    #[serde(default)]
    tenant: Option<String>,
    pool: LedgerPoolName,
    description: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LimitRequest {
    #[serde(default)]
    tenant: Option<String>,
    limit: LedgerLimit,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoveRequest {
    #[serde(default)]
    tenant: Option<String>,
    definition: LedgerDefinitionRef,
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Invalid, message)
}

fn parse<T: for<'de> Deserialize<'de>>(body: Json_) -> Result<T, ApiError> {
    serde_json::from_value(body).map_err(|e| {
        invalid(format!(
            "The request does not name what this operation needs: {e}"
        ))
    })
}

/// A tenant input that names another tenant than the request's is refused (TEN-1140).
fn same_tenant(app: &App, identity: &Identity, named: &Option<String>) -> Result<(), ApiError> {
    match named {
        Some(tenant) if tenant != identity.tenant.as_str() => {
            app.audit(
                "ledger.tenant_mismatch",
                &identity.correlation,
                json!({ "tenant": identity.tenant.as_str() }),
            );
            Err(ApiError::new(
                ErrorCode::Forbidden,
                "The request names another tenant.",
            ))
        }
        _ => Ok(()),
    }
}

fn scope_of(scope: &Option<LedgerScope>) -> &str {
    scope.as_ref().map_or("", |s| s.as_str())
}

/// A cursor as the number it stands for; an absent cursor is the start.
fn cursor_number<T: std::str::FromStr>(cursor: &Option<String>) -> Result<Option<T>, ApiError> {
    cursor
        .as_deref()
        .map(|text| {
            text.parse::<T>()
                .map_err(|_| invalid("The cursor is not one this list gave."))
        })
        .transpose()
}

/// Answer a read with the kernel's ledger module: load what `needs` names, let `answer` decide,
/// and load once more for each need it says is missing.
fn read<T>(
    app: &App,
    identity: &Identity,
    needs: Vec<Need>,
    answer: impl Fn(&Snapshot, &Context) -> Result<T, DomainError>,
) -> Result<T, ApiError> {
    let ctx = identity.context(app);
    let mut wanted = needs;
    // A ranked listing under a ledger policy asks for a wider window each time its page is short.
    for _ in 0..16 {
        let state = app.store.load(&ctx.tenant, &wanted)?;
        match answer(&state, &ctx) {
            Err(DomainError {
                kind: ErrorKind::InsufficientState,
                missing: Some(need),
                ..
            }) if !wanted.contains(&need) => wanted.push(need),
            other => return other.map_err(|e| ApiError::domain(e, identity.anonymous())),
        }
    }
    Err(ApiError::hidden(
        ErrorCode::Internal,
        "a ledger read asked for more state than it was given",
    ))
}

/// One schema change of the ledger, decided and committed as the caller.
fn change(app: &App, identity: &Identity, change: SchemaChange) -> Result<Json_, ApiError> {
    let intent = Intent::ChangeSchema {
        changes: vec![change],
        expected_version: None,
        existing: ExistingData::Reject,
    };
    let done = run_intent(app, &identity.context(app), &intent, identity.anonymous())?;
    let version = match done.transition.output {
        Output::Schema { version } => version.0,
        other => {
            return Err(ApiError::hidden(
                ErrorCode::Internal,
                format!("unexpected output {other:?}"),
            ))
        }
    };
    Ok(json!({ "schema_version": version, "revision": done.receipt.revision.0 }))
}

/// Do one operation as the identified caller.
pub fn handle(app: &App, identity: &Identity, op: &str, body: Json_) -> Result<Json_, ApiError> {
    let tenant = identity.tenant.clone();
    match op {
        // Implements: CT-281
        "post" => {
            let r: PostRequest = parse(body)?;
            same_tenant(app, identity, &r.tenant)?;
            let intent = Intent::PostLedger {
                postings: vec![LedgerPostingRequest {
                    owner: r.owner,
                    unit: r.unit,
                    amount: strata_kernel::LedgerAmount(r.amount),
                    idempotency_key: r.idempotency_key,
                    reason: r.reason,
                    corrects: r.corrects,
                    scope: r.scope,
                }],
            };
            let done = run_intent(app, &identity.context(app), &intent, identity.anonymous())?;
            match done.transition.output {
                Output::LedgerPosted { mut results } if results.len() == 1 => {
                    Ok(json!({ "result": results.remove(0) }))
                }
                other => Err(ApiError::hidden(
                    ErrorCode::Internal,
                    format!("unexpected output {other:?}"),
                )),
            }
        }
        // Implements: CT-282, CT-807
        "balance" | "my-balance" => {
            let r: AccountRequest = parse(body)?;
            same_tenant(app, identity, &r.tenant)?;
            let own = op == "my-balance";
            let owner = owner_of(identity, app, own, r.owner)?;
            let scope = scope_of(&r.scope).to_string();
            let needs = ledger::balance_needs(&tenant, &owner, &r.unit, &scope);
            let balance = read(app, identity, needs, |state, ctx| {
                ledger::read_balance(state, &owner, &r.unit, &scope, own, ctx)
            })?;
            Ok(json!({ "balance": balance }))
        }
        // Implements: CT-283, CT-808
        "entries" | "my-entries" => {
            let r: AccountRequest = parse(body)?;
            same_tenant(app, identity, &r.tenant)?;
            let own = op == "my-entries";
            let owner = owner_of(identity, app, own, r.owner)?;
            let scope = scope_of(&r.scope).to_string();
            let after: Option<u64> = cursor_number(&r.cursor)?;
            let needs = ledger::entries_needs(&tenant, &owner, &r.unit, &scope, after, r.limit)
                .map_err(|e| ApiError::domain(e, identity.anonymous()))?;
            let (entries, next) = read(app, identity, needs, |state, ctx| {
                ledger::read_entries(state, &owner, &r.unit, &scope, after, r.limit, own, ctx)
            })?;
            Ok(json!({ "entries": entries, "next": next.map(|n| n.to_string()) }))
        }
        // Implements: CT-809
        "accounts" => {
            let r: AccountRequest = parse(body)?;
            same_tenant(app, identity, &r.tenant)?;
            if r.owner.is_some() {
                return Err(invalid("A ranked listing names no owner."));
            }
            let order = r
                .order
                .ok_or_else(|| invalid("A ranked listing names its order."))?;
            let scope = scope_of(&r.scope).to_string();
            let offset: u32 = cursor_number(&r.cursor)?.unwrap_or(0);
            // Refused before anything is read when the walk would pass its bound (LDG-1543).
            let needs = ledger::ranked_needs(&r.unit, &scope, order, offset, r.limit)
                .map_err(|e| ApiError::domain(e, identity.anonymous()))?;
            let (accounts, next) = read(app, identity, needs, |state, ctx| {
                ledger::read_ranked(state, &r.unit, &scope, order, offset, r.limit, ctx)
            })?;
            Ok(json!({ "accounts": accounts, "next": next.map(|n| n.to_string()) }))
        }
        // Implements: CT-810
        "definitions" => {
            let r: Common = parse(body)?;
            same_tenant(app, identity, &r.tenant)?;
            let definitions = read(app, identity, Vec::new(), ledger::read_definitions)?;
            Ok(json!({ "definitions": definitions }))
        }
        // Implements: CT-284
        "declare-unit" => {
            let r: UnitRequest = parse(body)?;
            same_tenant(app, identity, &r.tenant)?;
            let unit = r.unit.clone();
            let expires = r.expiry.is_some();
            let mut reply = change(
                app,
                identity,
                SchemaChange::DeclareLedgerUnit(LedgerUnitDecl {
                    unit: r.unit,
                    description: r.description,
                    expiry: r.expiry,
                }),
            )?;
            if expires {
                start_expiry_chain(app, &identity.tenant)?;
            }
            reply["unit"] = json!(unit);
            Ok(reply)
        }
        // Implements: CT-285
        "declare-pool" => {
            let r: PoolRequest = parse(body)?;
            same_tenant(app, identity, &r.tenant)?;
            let pool = r.pool.clone();
            let mut reply = change(
                app,
                identity,
                SchemaChange::DeclareLedgerPool(LedgerPoolDecl {
                    pool: r.pool,
                    description: r.description,
                }),
            )?;
            reply["pool"] = json!(pool);
            Ok(reply)
        }
        // Implements: CT-286
        "set-limit" => {
            let r: LimitRequest = parse(body)?;
            same_tenant(app, identity, &r.tenant)?;
            let limit = r.limit.clone();
            let mut reply = change(app, identity, SchemaChange::SetLedgerLimit(r.limit))?;
            reply["limit"] = json!(limit);
            Ok(reply)
        }
        // Implements: CT-811
        "remove-definition" => {
            let r: RemoveRequest = parse(body)?;
            same_tenant(app, identity, &r.tenant)?;
            change(
                app,
                identity,
                SchemaChange::RemoveLedgerDefinition(r.definition),
            )
        }
        // Implements: CT-287
        "verify" => {
            let r: VerifyRequest = parse(body)?;
            same_tenant(app, identity, &r.tenant)?;
            verify(app, identity, &r.account)
        }
        // Implements: CT-812
        "expire" => {
            let r: ExpireRequest = parse(body)?;
            same_tenant(app, identity, &r.tenant)?;
            let ctx = identity.context(app);
            let state = app.store.load(&tenant, &[])?;
            if let Some(found) = state.tenant.as_deref() {
                if !ledger_expiry::may_expire(found, &ctx.principal) {
                    return Err(ApiError::domain(
                        DomainError::new(
                            ErrorKind::Unauthorized,
                            "the expiry of credits needs ledger-configure",
                        ),
                        identity.anonymous(),
                    ));
                }
            }
            let schema_expiry = state
                .tenant
                .as_deref()
                .and_then(|t| ledger_expiry::expiry_of(&t.schema, &r.unit).cloned());
            let Some(expiry) = schema_expiry else {
                return Err(invalid("That unit declares no expiry."));
            };
            let cutoff = r
                .bucket
                .map(Timestamp)
                .unwrap_or_else(|| ledger_expiry::cutoff(&expiry, ctx.now));
            // Nothing due is nothing to commit.
            if first_due(app, &tenant, &r.unit, cutoff)?.is_none() {
                return Ok(json!({ "posted": 0, "next": null }));
            }
            let intent = Intent::ExpireLedger {
                unit: r.unit.clone(),
                cutoff,
            };
            let done = run_intent(app, &ctx, &intent, identity.anonymous())?;
            let posted = match done.transition.output {
                Output::LedgerPosted { results } => results.len(),
                other => {
                    return Err(ApiError::hidden(
                        ErrorCode::Internal,
                        format!("unexpected output {other:?}"),
                    ))
                }
            };
            let next = first_due(app, &tenant, &r.unit, cutoff)?;
            Ok(json!({ "posted": posted, "next": next }))
        }
        _ => Err(ApiError::new(ErrorCode::NotFound, "No such endpoint.")),
    }
}

/// Compare the stored balance of an account with the sum of its journal (CT-287, LDG-019,
/// LDG-020): `ok`, the `stored` balance, the `replayed` sum, the number of entries replayed and,
/// when the numbers of the entries have a gap, the first number that is missing.
fn verify(app: &App, identity: &Identity, account: &LedgerAccountId) -> Result<Json_, ApiError> {
    let ctx = identity.context(app);
    let wanted = [Need::Ledger(LedgerNeed::Account { account: *account })];
    let state = app.store.load(&ctx.tenant, &wanted)?;
    ledger::verify_access(&state, &ctx).map_err(|e| ApiError::domain(e, identity.anonymous()))?;
    let Some(header) = ledger::header_of(&state, account) else {
        return Err(ApiError::new(
            ErrorCode::NotFound,
            "No such ledger account.",
        ));
    };
    let mut replay = ledger::Replay::default();
    let mut after: Option<u64> = None;
    loop {
        let page = app.store.load(
            &ctx.tenant,
            &[Need::Ledger(LedgerNeed::Entries {
                account: *account,
                after,
                limit: VERIFY_PAGE,
            })],
        )?;
        let entries = ledger::entries_of(&page, account);
        replay.feed(&entries);
        after = entries.last().map(|e| e.sequence);
        if replay.gap.is_some() || entries.len() < VERIFY_PAGE as usize {
            break;
        }
    }
    Ok(json!({
        "ok": replay.agrees(header.balance.0, header.last_sequence),
        "stored": header.balance,
        "replayed": replay.sum,
        "entries": replay.last,
        "gap": replay.gap,
    }))
}

/// The id of the first account that still has credit due at `cutoff`, if one has.
fn first_due(
    app: &App,
    tenant: &TenantId,
    unit: &LedgerUnit,
    cutoff: Timestamp,
) -> Result<Option<LedgerAccountId>, ApiError> {
    let state = app.store.load(
        tenant,
        &[Need::Ledger(LedgerNeed::Expiring {
            unit: unit.clone(),
            cutoff,
            limit: 1,
        })],
    )?;
    Ok(state
        .tenant
        .as_deref()
        .and_then(|t| t.ledger.as_ref())
        .and_then(|l| {
            ledger_expiry::due_accounts(l.accounts.values(), unit, cutoff, 1)
                .first()
                .map(|a| a.account)
        }))
}

/// Create the job that starts the chain of expiry jobs of a tenant, once: the key is the same for
/// every declaration of a unit with an expiry, so the second creates nothing (SCH-004).
fn start_expiry_chain(app: &App, tenant: &TenantId) -> Result<(), ApiError> {
    let now = app.now();
    app.store.jobs_create(
        tenant,
        &[NewJob {
            key: EXPIRY_START_KEY.to_string(),
            work: JobWork::ExpireLedgerCredits {},
            due_at: Timestamp(now.0 + EXPIRY_INTERVAL_MS),
            policy: RetryPolicy::default(),
        }],
        now,
    )?;
    Ok(())
}

/// The job that follows `job`: due an interval after the job it follows was due (after the time it
/// was created, for the first of a chain), under a key that names its due time. It depends on the
/// job alone, so that a retry of the same job creates the same next job and the store creates it
/// once; a chain that fell behind catches up one run at a time.
fn next_expiry_job(job: &strata_storage_api::jobs::Job) -> NewJob {
    let due = job
        .key
        .strip_prefix("ledger-expiry:")
        .and_then(|n| n.parse::<i64>().ok())
        .unwrap_or(job.created_at.0)
        + EXPIRY_INTERVAL_MS;
    NewJob {
        key: format!("ledger-expiry:{due}"),
        work: JobWork::ExpireLedgerCredits {},
        due_at: Timestamp(due),
        policy: RetryPolicy::default(),
    }
}

/// Run one job of the expiry chain of a tenant at `now`: for each unit that declares an expiry,
/// decide batches of the kernel's `ExpireLedger` (at most [`MAX_BATCHES_PER_RUN`] of one hundred
/// accounts) while accounts with credit due remain, as the system. The next job is created in the
/// transaction that reports this one, so the chain goes on whether or not this run succeeded; a
/// run that fails is retried under the job's policy and posts nothing twice, because every entry
/// has the key `expiry:<bucket start>` (LDG-1546, LDG-1547).
pub fn run_expiry_job(app: &App, tenant: &TenantId, lease: &Lease, now: Timestamp) -> Outcome {
    let writes = WorkerWrites {
        jobs: vec![next_expiry_job(&lease.job)],
        ..WorkerWrites::default()
    };
    let report = match expire_due(app, tenant, now, &lease.job.id) {
        Ok(_) => JobReport::Succeeded,
        Err(error) => JobReport::Failed {
            error,
            retry: !lease.exhausted,
        },
    };
    Outcome { report, writes }
}

/// Expire what is due at `now` in every unit of the tenant that declares an expiry; the number of
/// entries posted.
fn expire_due(app: &App, tenant: &TenantId, now: Timestamp, job: &str) -> Result<usize, String> {
    let state = app.store.load(tenant, &[]).map_err(|e| e.to_string())?;
    let Some(found) = state.tenant else {
        return Ok(0);
    };
    let expiring: Vec<(LedgerUnit, LedgerExpiry)> = found
        .schema
        .ledger
        .iter()
        .flat_map(|declared| declared.units.values())
        .filter_map(|decl| decl.expiry.clone().map(|e| (decl.unit.clone(), e)))
        .collect();
    let mut posted = 0;
    for (unit, expiry) in expiring {
        let cutoff = ledger_expiry::cutoff(&expiry, now);
        for _ in 0..MAX_BATCHES_PER_RUN {
            let due = first_due(app, tenant, &unit, cutoff).map_err(|e| e.describe())?;
            if due.is_none() {
                break;
            }
            let ctx = Context {
                tenant: tenant.clone(),
                principal: Principal::System,
                session: None,
                now,
                correlation: format!("job:{job}"),
                verified_address: None,
                idempotency_key: None,
                language: None,
                hop: None,
            };
            let intent = Intent::ExpireLedger {
                unit: unit.clone(),
                cutoff,
            };
            let done = run_intent(app, &ctx, &intent, false).map_err(|e| e.describe())?;
            match done.transition.output {
                Output::LedgerPosted { results } => posted += results.len(),
                other => return Err(format!("unexpected output {other:?}")),
            }
        }
    }
    Ok(posted)
}

/// The owner a read names: the caller for a read of one's own account (which takes no owner),
/// else the owner the request gives.
fn owner_of(
    identity: &Identity,
    app: &App,
    own: bool,
    named: Option<LedgerOwner>,
) -> Result<LedgerOwner, ApiError> {
    if own {
        if named.is_some() {
            return Err(invalid("A read of one's own account names no owner."));
        }
        return ledger::own_owner(&identity.context(app))
            .map_err(|e| ApiError::domain(e, identity.anonymous()));
    }
    named.ok_or_else(|| invalid("The read names the owner of the account."))
}

/// `/internal/ledger/<operation>`.
///
/// Implements: CT-281, CT-282, CT-283, CT-284, CT-285, CT-286, CT-287, CT-807, CT-808, CT-809, CT-810, CT-811, CT-812
pub async fn route(
    State(app): State<Shared>,
    headers: HeaderMap,
    Path(op): Path<String>,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    crate::http::as_caller(&app, &headers, move |app, identity| {
        handle(app, identity, &op, body)
    })
    .await
}
