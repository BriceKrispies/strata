//! Authorized reads. A read never changes state and never returns a record the principal's
//! policies do not grant.

use crate::expr::{self, Env};
use crate::ids::Timestamp;
use crate::intent::{Check, Context, DomainError, ErrorKind, ReadRequest, ReadResult};
use crate::rules::{authorize, roles_of, Lookup, Subject};
use crate::schema::{Action, Resource, TenantSchema};
use crate::state::{Need, Principal, Record, State, TenantState};
use std::sync::Arc;

/// Whether a principal may read a record. The same decision `read` makes per row; the shell
/// uses it to decide who receives an event about the record.
pub fn visible(
    tenant: &TenantState,
    principal: &Principal,
    record: &Record,
    now: Timestamp,
) -> bool {
    authorize(
        tenant,
        &tenant.schema,
        principal,
        now,
        Action::Read,
        &Resource::Object(record.object.clone()),
        Subject::stored(record),
    )
    .0
}

/// The state a read is expected to need.
pub fn read_needs(schema: &TenantSchema, request: &ReadRequest) -> Vec<Need> {
    match request {
        ReadRequest::Named { query, .. } => schema
            .queries
            .get(query)
            .map(|def| vec![Need::AllOf(def.object.clone())])
            .unwrap_or_default(),
        ReadRequest::Records { object, ids } => ids
            .iter()
            .map(|id| Need::Record {
                object: object.clone(),
                id: *id,
            })
            .collect(),
    }
}

/// Answer a read request against a snapshot.
pub fn read(
    state: &State,
    request: &ReadRequest,
    ctx: &Context,
) -> Result<ReadResult, DomainError> {
    if state.tenant_id != ctx.tenant {
        return Err(DomainError::new(
            ErrorKind::TenantMismatch,
            "snapshot is of another tenant",
        ));
    }
    let tenant = state
        .tenant
        .as_deref()
        .filter(|tenant| tenant.id == ctx.tenant)
        .ok_or_else(|| DomainError::new(ErrorKind::TenantNotFound, "no such tenant"))?;
    let lookup = Lookup {
        tenant,
        coverage: &state.coverage,
    };
    let schema = &tenant.schema;

    let rows: Vec<Arc<Record>> = match request {
        ReadRequest::Records { object, ids } => {
            if !schema.objects.contains_key(object) {
                return Err(DomainError::new(
                    ErrorKind::UnknownObject,
                    format!("no object type {object}"),
                ));
            }
            let mut rows = Vec::new();
            for id in ids {
                if let Some(record) = lookup.record(object, *id)? {
                    if visible(tenant, &ctx.principal, record, ctx.now) {
                        rows.push(record.clone());
                    }
                }
            }
            rows
        }
        ReadRequest::Named { query, params } => {
            let def = schema.queries.get(query).ok_or_else(|| {
                DomainError::new(ErrorKind::UnknownQuery, format!("no query {query}"))
            })?;
            if let Some(extra) = params.keys().find(|name| !def.params.contains(*name)) {
                return Err(DomainError::new(
                    ErrorKind::InvalidIntent,
                    format!("query {query} has no parameter {extra}"),
                ));
            }
            let roles = roles_of(tenant, &ctx.principal);
            let mut keyed = Vec::new();
            for record in lookup.all_of(&def.object)? {
                if !visible(tenant, &ctx.principal, record, ctx.now) {
                    continue;
                }
                if let Some(filter) = &def.filter {
                    let env = Env {
                        record: Some(record),
                        old: Some(record),
                        new: Some(record),
                        params,
                        now: ctx.now,
                        principal: &ctx.principal,
                        roles: &roles,
                    };
                    let keep = expr::holds(filter, &env).map_err(|e| {
                        DomainError::new(ErrorKind::Evaluation, format!("query {query}: {e}")).with(
                            vec![Check::fail(format!("query:{query}"), filter.to_string())],
                        )
                    })?;
                    if !keep {
                        continue;
                    }
                }
                let mut key = Vec::with_capacity(def.order.len());
                for order in &def.order {
                    key.push(expr::attribute(Some(record), &order.field).map_err(|e| {
                        DomainError::new(ErrorKind::Evaluation, format!("query {query}: {e}"))
                    })?);
                }
                keyed.push((key, record.clone()));
            }
            keyed.sort_by(|(a, ra), (b, rb)| {
                for (i, order) in def.order.iter().enumerate() {
                    let ordering = a[i].cmp(&b[i]);
                    let ordering = if order.descending {
                        ordering.reverse()
                    } else {
                        ordering
                    };
                    if ordering.is_ne() {
                        return ordering;
                    }
                }
                ra.id.cmp(&rb.id)
            });
            let limit = def.limit.map_or(usize::MAX, |n| n as usize);
            keyed
                .into_iter()
                .take(limit)
                .map(|(_, record)| record)
                .collect()
        }
    };
    Ok(ReadResult {
        rows,
        revision: tenant.revision,
        schema_version: schema.version,
    })
}
