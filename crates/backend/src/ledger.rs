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

use crate::app::{ApiError, App, Shared};
use crate::auth::Identity;
use crate::ops::run_intent;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value as Json_};
use strata_kernel::ledger;
use strata_kernel::{
    Context, DomainError, ErrorKind, ExistingData, Intent, LedgerDefinitionRef, LedgerLimit,
    LedgerOrder, LedgerOwner, LedgerPoolDecl, LedgerPoolName, LedgerPostingRequest, LedgerScope,
    LedgerUnit, LedgerUnitDecl, Need, Output, SchemaChange, State as Snapshot,
};
use strata_protocol::api::ErrorCode;

/// The operations of the route, as the BFF may name them.
pub const OPERATIONS: [&str; 11] = [
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
];

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
    expiry: Option<Json_>,
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
    for _ in 0..8 {
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
            if r.expiry.is_some() {
                return Err(invalid("The expiry of a unit is not offered yet."));
            }
            let unit = r.unit.clone();
            let mut reply = change(
                app,
                identity,
                SchemaChange::DeclareLedgerUnit(LedgerUnitDecl {
                    unit: r.unit,
                    description: r.description,
                }),
            )?;
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
        _ => Err(ApiError::new(ErrorCode::NotFound, "No such endpoint.")),
    }
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
/// Implements: CT-281, CT-282, CT-283, CT-284, CT-285, CT-286, CT-807, CT-808, CT-809, CT-810, CT-811
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
