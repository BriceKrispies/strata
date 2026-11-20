//! Running one aggregate of a named query for a caller: load the window the kernel asks for,
//! let the kernel compute the result for the caller's principal, and report it. No rule about
//! what a reader may count is written here (D-5061): the kernel leaves out every record the
//! reader may not read, and refuses an aggregate that would examine more than its bound.
//!
//! The result is computed for the caller on every request. The kernel derives no audience class
//! from the policies of a tenant, so nothing is stored or shared between callers.
//!
//! Implements: CT-310

use crate::app::{ApiError, App};
use crate::auth::Identity;
use crate::ops::header;
use serde_json::Value as Json;
use strata_kernel::{aggregates, AggregateId, AggregateRequest, ErrorKind, QueryId};
use strata_protocol::api::{AggregateRunRequest, ErrorCode};
use strata_substrate::plain::coerce_fields;
use strata_substrate::project::project_aggregate;

/// The most records a snapshot may be asked to add, one at a time, for records that a policy or a
/// read rule follows a reference into.
const MOST_ROUNDS: usize = 8;

/// Run an aggregate as the caller, answering the plain model of its result.
pub fn run(
    app: &App,
    identity: &Identity,
    request: &AggregateRunRequest,
) -> Result<Json, ApiError> {
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
    let mut needs = aggregates::needs(&tenant.schema, &ask);
    for _ in 0..MOST_ROUNDS {
        let state = app.store.load(&identity.tenant, &needs)?;
        match aggregates::run(&state, &ask, &ctx) {
            Ok(result) => return Ok(project_aggregate(&result)),
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
