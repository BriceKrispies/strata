//! Invoking a tenant operation for a caller (contract invoke-tenant-operation, CT-805; WG-5100).
//!
//! The inputs arrive as plain JSON and are coerced by the operation's input types; the kernel
//! decides the invocation (`strata_kernel::operations`) and the store commits it in one
//! transaction, as every intent is committed. The caller's idempotency key is required: it is the
//! intent's key and the context's, so a repeat by the same person with the same key and the same
//! input answers with the first result and changes nothing (the store's idempotency, see
//! `strata_storage_api::execute_by`), and the same key with other input is refused with
//! idempotency-mismatch. A dry run is decided against the same snapshot and not committed.
//! No domain rule is written here.

use crate::app::{ApiError, App, Shared};
use crate::auth::Identity;
use crate::ops::{header, run_intent};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::Json;
use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::ext::*;
use strata_kernel::operations::{operation_of, result_of, valid_key, MAX_KEY_BYTES};
use strata_kernel::{
    canon, Check, Context, DomainError, ErrorKind, Intent, Need, TenantOperationId, Transition,
    Value,
};
use strata_protocol::api::{ErrorCode, OperationReply, OperationRequest, HEADER_IDEMPOTENCY};
use strata_storage_api::idempotency_scope;
use strata_substrate::plain::coerce;

/// How many rounds a dry run may spend loading state the decision found missing, as many as a
/// committed decision may (`strata_storage_api::execute_by`).
const MAX_NEED_ROUNDS: usize = 32;

/// `/internal/operations/invoke`: invoke a tenant operation as the identified caller, with the
/// key in the `idempotency-key` header. Mounted in the router; the service token is checked by
/// the gate in front of every internal route.
///
/// Implements: CT-805
pub async fn route(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<OperationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let key = headers
        .get(HEADER_IDEMPOTENCY)
        .and_then(|value| value.to_str().ok())
        .filter(|key| valid_key(key))
        .map(str::to_string)
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::Invalid,
                format!(
                    "An invocation needs an idempotency key of 1 to {MAX_KEY_BYTES} visible ASCII characters."
                ),
            )
        })?;
    crate::http::as_caller(&app, &headers, move |app, identity| {
        invoke(app, identity, &request, &key)
    })
    .await
}

/// A refusal of one input, naming it in the failed check so that a form can say which.
fn invalid_input(operation: &TenantOperationId, input: &str, problem: String) -> ApiError {
    let mut error = ApiError::new(ErrorCode::Invalid, problem.clone());
    error
        .0
        .failed
        .push(Check::fail(format!("input:{operation}.{input}"), problem));
    error
}

/// Invoke the operation `request` names, as `identity`, under `key`.
pub fn invoke(
    app: &App,
    identity: &Identity,
    request: &OperationRequest,
    key: &str,
) -> Result<OperationReply, ApiError> {
    // A key is scoped by principal, and only a signed-in person invokes (OPR-1546); nothing about
    // the tenant's operations is told to anyone else.
    if identity.anonymous() {
        return Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in to invoke an operation.",
        ));
    }
    let tenant = header(app, identity)?;
    let missing = || {
        ApiError::new(
            ErrorCode::NotFound,
            format!("no operation {}", request.operation),
        )
    };
    let operation = TenantOperationId::new(request.operation.as_str()).map_err(|_| missing())?;
    let def = operation_of(&tenant.schema, &operation).ok_or_else(missing)?;
    let mut inputs: BTreeMap<String, Value> = BTreeMap::new();
    for (name, json) in &request.inputs {
        let Some(input) = def.inputs.iter().find(|input| &input.name == name) else {
            return Err(invalid_input(
                &operation,
                name,
                format!("operation {operation} has no input {name}"),
            ));
        };
        let value = coerce(&input.ty, json, &|_| None)
            .map_err(|problem| invalid_input(&operation, name, format!("{name}: {problem}")))?;
        inputs.insert(name.clone(), value);
    }
    let intent = Intent::InvokeOperation {
        operation: operation.clone(),
        inputs,
        idempotency_key: key.to_string(),
    };
    let mut ctx = identity.context(app);
    ctx.idempotency_key = Some(key.to_string());
    let unexpected = || ApiError::hidden(ErrorCode::Internal, "an invocation without its result");

    if request.dry_run {
        let transition =
            decide(app, &ctx, &intent).map_err(|error| explain(app, &ctx, &intent, key, error))?;
        let result = result_of(&transition.output, transition.delta.new_revision, false)
            .ok_or_else(unexpected)?;
        return Ok(OperationReply {
            result,
            event_seq: transition.delta.event_seq,
            checks_passed: transition.evidence.len(),
            dry_run: true,
        });
    }
    let done = run_intent(app, &ctx, &intent, false)
        .map_err(|error| explain(app, &ctx, &intent, key, error))?;
    let result = result_of(
        &done.transition.output,
        done.receipt.revision,
        done.replayed,
    )
    .ok_or_else(unexpected)?;
    Ok(OperationReply {
        result,
        event_seq: done.transition.delta.event_seq,
        checks_passed: done.transition.evidence.len(),
        dry_run: false,
    })
}

/// The contract's answer for a refusal: an operation the tenant does not have is not-found, and a
/// key the caller used before with other input is idempotency-mismatch (OPR-1544). Every other
/// refusal stands as the kernel or the store gave it.
fn explain(app: &App, ctx: &Context, intent: &Intent, key: &str, mut error: ApiError) -> ApiError {
    if error
        .0
        .failed
        .iter()
        .any(|check| check.rule == "operation:exists")
    {
        error.0.code = ErrorCode::NotFound;
        return error;
    }
    if error.0.kind.as_deref() == Some("InvalidIntent") {
        let scope = idempotency_scope(&ctx.principal);
        if let Ok(Some(first)) = app
            .store
            .idempotent_result(&ctx.tenant, &scope, key, ctx.now)
        {
            if first.request_hash != canon::hash_of(intent) {
                let mut mismatch = ApiError::new(
                    ErrorCode::Conflict,
                    "This idempotency key was used for an invocation with other input.",
                );
                mismatch.0.kind = Some("IdempotencyMismatch".into());
                mismatch.0.failed.push(Check::fail(
                    "operation:idempotency_mismatch",
                    "the key was used with other input",
                ));
                return mismatch;
            }
        }
    }
    error
}

/// Decide an intent against what it needs, loading as `execute_by` loads, and commit nothing: a
/// dry run (OPR-1552).
fn decide(app: &App, ctx: &Context, intent: &Intent) -> Result<Transition, ApiError> {
    let store = app.store.as_ref();
    let head = store.load(&ctx.tenant, &[])?;
    let mut wanted: BTreeSet<Need> = strata_kernel::needs(
        head.tenant.as_deref().map(|tenant| tenant.schema.as_ref()),
        intent,
    )
    .into_iter()
    .collect();
    for _ in 0..MAX_NEED_ROUNDS {
        let list: Vec<Need> = wanted.iter().cloned().collect();
        let state = store.load(&ctx.tenant, &list)?;
        let more: Vec<Need> = strata_kernel::refine_needs(&state, intent)
            .into_iter()
            .filter(|need| !wanted.contains(need))
            .collect();
        if !more.is_empty() {
            wanted.extend(more);
            continue;
        }
        match strata_kernel::transition(&state, intent, ctx) {
            Err(DomainError {
                kind: ErrorKind::InsufficientState,
                missing: Some(need),
                ..
            }) if !wanted.contains(&need) => {
                wanted.insert(need);
            }
            Err(error) => return Err(ApiError::domain(error, false)),
            Ok(transition) => return Ok(transition),
        }
    }
    Err(ApiError::hidden(
        ErrorCode::Internal,
        "the decision asked for more state than one decision may load",
    ))
}
