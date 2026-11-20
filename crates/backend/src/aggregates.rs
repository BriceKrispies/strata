//! Running one aggregate of a named query for a caller: load the window the kernel asks for, let
//! the kernel compute the result for the caller's principal, and report it. No rule about what a
//! reader may count is written here (D-5061): the kernel leaves out every record the reader may not
//! read, and refuses an aggregate that would examine more than its bound.
//!
//! # What is stored
//!
//! A result may be kept as a copy and served again (D-5064, D-5065, AGG-030 to AGG-034). The key of
//! a copy holds the tenant (first, CPY-003), the query, the aggregate, its arguments, the audience
//! class of the caller that the kernel derives from the tenant's policies
//! ([`strata_kernel::audience`]), the schema version and a hash of the schema (teams and the roles
//! of a team change the schema without a new version), and the change counter of each object type
//! the aggregate depends on ([`strata_kernel::aggregates::dependencies`], including the types that
//! the Read policies of the object type follow, AGG-042). The counters come from the store and
//! rise in the commit that writes or deletes a record of the type, so a commit makes a new key and
//! an answer computed before it is never found again: that is the whole of what stale means, and
//! the lifetime of [`COPY_LIFETIME_MS`] is only a backstop and a bound on memory.
//!
//! **Stored:** the result of an aggregate, for a caller whose class is shareable, that the kernel
//! computed without a refusal from a snapshot whose schema and members are those the key was made
//! from. A caller of the same class is served the copy, with the revision of the tenant now.
//!
//! **Not stored:** a result for an individual class, that is, whenever a condition that applies to
//! the caller (a policy of one of the caller's roles, a field read rule, the query filter) mentions
//! the principal or the clock, or the kernel cannot resolve the query; a refusal; a result whose
//! snapshot is not the one the key was made from. Nothing that is not an aggregate result is
//! stored, and no copy is a shared copy: it is served only to the callers of the class it was
//! stored for, behind the same route and authentication as any other answer (AGG-034).
//!
//! The copies are in the memory of this process and lost when it ends; a store that cannot be
//! read or written is a miss, never an error (CPY-011). Concurrent first requests may each compute
//! the result.
//!
//! Implements: CT-310, CT-311

use crate::app::{ApiError, App};
use crate::auth::Identity;
use crate::ops::header;
use serde_json::{json, Value as Json};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use strata_kernel::audience::{class_of, Reading};
use strata_kernel::{
    aggregates, canon, AggregateId, AggregateRequest, AggregateResult, ErrorKind, ObjectTypeId,
    QueryId, TenantId,
};
use strata_protocol::api::{AggregateRunRequest, ErrorCode};
use strata_substrate::plain::coerce_fields;
use strata_substrate::project::project_aggregate;

/// The most records a snapshot may be asked to add, one at a time, for records that a policy or a
/// read rule follows a reference into.
const MOST_ROUNDS: usize = 8;

/// How long a copy is kept at most, in milliseconds: the backstop of a key that is exact.
pub const COPY_LIFETIME_MS: i64 = 60_000;

/// The most copies held at once; past it the oldest go first.
pub const MOST_COPIES: usize = 2_048;

struct Held {
    tenant: TenantId,
    stored_at: i64,
    result: AggregateResult,
}

/// What the copies have done, for tests and diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CopyStats {
    /// Copies held now.
    pub held: usize,
    /// Requests answered from a copy.
    pub served: u64,
    /// Results put into the store.
    pub stored: u64,
}

/// The aggregate copies of one process.
#[derive(Default)]
pub struct Copies {
    held: Mutex<HashMap<String, Held>>,
    served: AtomicU64,
    stored: AtomicU64,
}

impl Copies {
    /// The copy under `key`, when it is younger than [`COPY_LIFETIME_MS`] at `now`.
    pub fn find(&self, key: &str, now: i64) -> Option<AggregateResult> {
        let held = self.held.lock().ok()?;
        let copy = held.get(key)?;
        if now.saturating_sub(copy.stored_at) > COPY_LIFETIME_MS {
            return None;
        }
        self.served.fetch_add(1, Ordering::Relaxed);
        Some(copy.result.clone())
    }

    /// Keep `result` under `key`.
    pub fn keep(&self, key: String, tenant: &TenantId, result: &AggregateResult, now: i64) {
        let Ok(mut held) = self.held.lock() else {
            return;
        };
        if held.len() >= MOST_COPIES {
            held.retain(|_, copy| now.saturating_sub(copy.stored_at) <= COPY_LIFETIME_MS);
        }
        while held.len() >= MOST_COPIES {
            let oldest = held
                .iter()
                .min_by_key(|(_, copy)| copy.stored_at)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(oldest) => held.remove(&oldest),
                None => break,
            };
        }
        held.insert(
            key,
            Held {
                tenant: tenant.clone(),
                stored_at: now,
                result: result.clone(),
            },
        );
        self.stored.fetch_add(1, Ordering::Relaxed);
    }

    /// Drop every copy of a tenant: a development reset gives the tenant a new history whose
    /// counters may repeat.
    pub fn forget(&self, tenant: &TenantId) {
        if let Ok(mut held) = self.held.lock() {
            held.retain(|_, copy| copy.tenant != *tenant);
        }
    }

    pub fn stats(&self) -> CopyStats {
        CopyStats {
            held: self.held.lock().map_or(0, |held| held.len()),
            served: self.served.load(Ordering::Relaxed),
            stored: self.stored.load(Ordering::Relaxed),
        }
    }
}

/// The key of a copy (AGG-030): the tenant first, then everything the answer is a function of.
/// `None` when the arguments cannot be written down, which makes the answer uncacheable.
pub fn copy_key(
    tenant: &TenantId,
    request: &AggregateRequest,
    class: &str,
    schema_version: u64,
    schema_hash: &str,
    revisions: &BTreeMap<ObjectTypeId, u64>,
) -> Option<String> {
    let rest = serde_json::to_string(&json!({
        "query": request.query,
        "aggregate": request.aggregate,
        "params": request.params,
        "filter": request.filter,
        "class": class,
        "schema_version": schema_version,
        "schema": schema_hash,
        "revisions": revisions,
    }))
    .ok()?;
    Some(format!("{tenant}|{rest}"))
}

/// A request that names a tenant other than the one it is made at is refused, and the refusal is an
/// audit entry (TEN-1140, contract aggregate-run-signed-in).
fn same_tenant(app: &App, identity: &Identity, named: &Option<String>) -> Result<(), ApiError> {
    match named {
        Some(tenant) if tenant != identity.tenant.as_str() => {
            app.audit(
                "aggregate.tenant_mismatch",
                &identity.correlation,
                json!({ "tenant": identity.tenant.as_str() }),
            );
            let mut error = ApiError::new(
                ErrorCode::Forbidden,
                "The request names another tenant than the one it is made at.",
            );
            error.0.kind = Some("tenant_mismatch".to_string());
            Err(error)
        }
        _ => Ok(()),
    }
}

/// Run an aggregate as the caller, answering the plain model of its result.
pub fn run(
    app: &App,
    identity: &Identity,
    request: &AggregateRunRequest,
) -> Result<Json, ApiError> {
    same_tenant(app, identity, &request.tenant)?;
    let tenant = header(app, identity)?;
    let missing =
        |what: &str, name: &str| ApiError::new(ErrorCode::NotFound, format!("no {what} {name}"));
    let query =
        QueryId::new(request.query.as_str()).map_err(|_| missing("query", &request.query))?;
    let aggregate = AggregateId::new(request.aggregate.as_str())
        .map_err(|_| missing("aggregate", &request.aggregate))?;
    let def = tenant
        .schema
        .queries
        .get(&query)
        .ok_or_else(|| missing("query", &request.query))?;
    let object = tenant
        .schema
        .objects
        .get(&def.object)
        .ok_or_else(|| missing("object type", def.object.as_str()))?;
    let filter = coerce_fields(object, &request.filter, &|_| None)
        .map_err(|problem| ApiError::new(ErrorCode::Invalid, problem))?;
    let ask = AggregateRequest {
        query,
        aggregate,
        params: Default::default(),
        filter,
    };

    let ctx = identity.context(app);

    // The key of a copy, when the audience class of the caller is one that shares. The change
    // counters are read before the snapshot is, so a copy is never stored under counters newer
    // than the records it was computed from.
    let class = class_of(
        &tenant,
        Reading::Aggregate {
            query: &ask.query,
            aggregate: &ask.aggregate,
        },
        &ctx.principal,
    );
    let key = match (
        class.key(),
        aggregates::dependencies(&tenant.schema, &ask.query, &ask.aggregate),
    ) {
        (Some(class), Some(dependencies)) => {
            let dependencies: Vec<ObjectTypeId> = dependencies.into_iter().collect();
            match app.store.object_revisions(&identity.tenant, &dependencies) {
                Ok(revisions) => copy_key(
                    &identity.tenant,
                    &ask,
                    &class,
                    tenant.schema.version.0,
                    &canon::hash_of(&*tenant.schema),
                    &revisions,
                ),
                Err(_) => None,
            }
        }
        _ => None,
    };
    let now = ctx.now.0;
    if let Some(key) = &key {
        if let Some(mut copy) = app.aggregate_copies.find(key, now) {
            // The aggregate's inputs have not changed since the copy was computed, so what it
            // says holds at the revision the tenant is at now.
            copy.revision = tenant.revision;
            return Ok(project_aggregate(&copy));
        }
    }

    let mut needs = aggregates::needs(&tenant.schema, &ask);
    for _ in 0..MOST_ROUNDS {
        let state = app.store.load(&identity.tenant, &needs)?;
        match aggregates::run(&state, &ask, &ctx) {
            Ok(result) => {
                // A copy is kept only when the snapshot is the one the key was made from: the
                // same schema and the same members, so the class and the dependencies hold.
                let same = state
                    .tenant
                    .as_ref()
                    .is_some_and(|t| *t.schema == *tenant.schema && t.members == tenant.members);
                if let (Some(key), true) = (key, same) {
                    app.aggregate_copies
                        .keep(key, &identity.tenant, &result, now);
                }
                return Ok(project_aggregate(&result));
            }
            Err(error) if error.kind == ErrorKind::InsufficientState => {
                match error.missing.clone() {
                    Some(more) if !needs.contains(&more) => needs.push(more),
                    _ => return Err(ApiError::domain(error, identity.anonymous())),
                }
            }
            Err(error) => return Err(ApiError::domain(error, identity.anonymous())),
        }
    }
    Err(ApiError::new(
        ErrorCode::Internal,
        "the records an aggregate follows could not be loaded",
    ))
}
