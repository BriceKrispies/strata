//! Authorized reads. A read never changes state and never returns a record the principal's
//! policies do not grant.

use crate::expr::{self, Env};
use crate::intent::{Context, DomainError, DomainErrorExt, ErrorKind, ReadRequest, ReadResult};
use crate::limits;
use crate::rules::{
    authorize, authorize_needing, insufficient, missing_error, roles_of, Lookup, Subject,
};
use crate::schema::{Action, Resource, TenantSchema};
use crate::state::{Need, Principal, Record, State, TenantState};
use crate::types::{RoleId, Timestamp, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// What a reader receives of a record: the record without the fields whose `read` condition
/// does not hold for them. A condition that cannot be evaluated hides the field, unless it
/// failed because the snapshot lacks a record it follows: that is `InsufficientState`, so the
/// shell loads the record and asks again. The system reads everything. Policy conditions, query filters and constraints see the whole record,
/// because they run inside the kernel; only what leaves it is trimmed.
pub(crate) fn redact(
    lookup: Lookup<'_>,
    principal: &Principal,
    roles: &BTreeSet<RoleId>,
    now: Timestamp,
    record: &Arc<Record>,
) -> Result<Arc<Record>, DomainError> {
    if matches!(principal, Principal::System) {
        return Ok(record.clone());
    }
    let Some(def) = lookup.tenant.schema.objects.get(&record.object) else {
        return Ok(record.clone());
    };
    let params: BTreeMap<String, Value> = BTreeMap::new();
    let env = Env {
        record: Some(record),
        old: Some(record),
        new: Some(record),
        params: &params,
        now,
        principal,
        roles,
        lookup: Some(lookup),
    };
    let mut hidden = Vec::new();
    for field in def.fields.values() {
        let Some(rule) = field.read.as_ref() else {
            continue;
        };
        if !record.fields.contains_key(&field.id) {
            continue;
        }
        let shown = expr::holds(rule, &env);
        if shown.is_err() {
            if let Some(missing) = missing_error(rule, &env) {
                return Err(missing);
            }
        }
        if !shown.unwrap_or(false) {
            hidden.push(field.id.clone());
        }
    }
    if hidden.is_empty() {
        return Ok(record.clone());
    }
    let mut shown = Record::clone(record);
    for field in &hidden {
        shown.fields.remove(field);
    }
    Ok(Arc::new(shown))
}

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
        None,
    )
    .0
}

/// [`visible`] with the snapshot's lookup, so a Read policy that follows a reference is decided
/// on the record it reaches. A policy that needs a record the snapshot does not hold is
/// `InsufficientState` naming it, not a denial.
pub fn visible_in(
    lookup: Lookup<'_>,
    principal: &Principal,
    record: &Record,
    now: Timestamp,
) -> Result<bool, DomainError> {
    let (allowed, _, needed) = authorize_needing(
        lookup.tenant,
        &lookup.tenant.schema,
        principal,
        now,
        Action::Read,
        &Resource::Object(record.object.clone()),
        Subject::stored(record),
        Some(lookup),
    );
    if !allowed {
        if let Some(need) = needed {
            return Err(insufficient(need));
        }
    }
    Ok(allowed)
}

/// The state a read is expected to need. A named query needs a bounded window of its object
/// type, never the whole of it (KRN-1286); a request the kernel will refuse needs nothing.
pub fn read_needs(schema: &TenantSchema, request: &ReadRequest) -> Vec<Need> {
    match request {
        ReadRequest::Named {
            query,
            params,
            page_size,
            cursor,
        } => {
            let Some(def) = schema.queries.get(query) else {
                return Vec::new();
            };
            crate::query_plan::named_needs(schema, def, params, *page_size, cursor.as_ref())
        }
        ReadRequest::Records { object, ids } if ids.len() <= limits::MAX_READ_IDS => ids
            .iter()
            .map(|id| Need::Record {
                object: object.clone(),
                id: *id,
            })
            .collect(),
        ReadRequest::Records { .. } => Vec::new(),
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

    let next = None;
    let rows: Vec<Arc<Record>> = match request {
        ReadRequest::Records { object, ids } => {
            if ids.len() > limits::MAX_READ_IDS {
                return Err(DomainError::new(
                    ErrorKind::LimitExceeded,
                    format!(
                        "{} ids in one read is more than the maximum of {}",
                        ids.len(),
                        limits::MAX_READ_IDS
                    ),
                ));
            }
            if !schema.objects.contains_key(object) {
                return Err(DomainError::new(
                    ErrorKind::UnknownObject,
                    format!("no object type {object}"),
                ));
            }
            let mut rows = Vec::new();
            for id in ids {
                if let Some(record) = lookup.record(object, *id)? {
                    if visible_in(lookup, &ctx.principal, record, ctx.now)? {
                        rows.push(record.clone());
                    }
                }
            }
            rows
        }
        // Named queries are planned, read and redacted in `query_plan` (KRN-2520).
        ReadRequest::Named {
            query,
            params,
            page_size,
            cursor,
        } => {
            return crate::query_plan::read_named(
                lookup,
                query,
                params,
                *page_size,
                cursor.as_ref(),
                ctx,
            );
        }
    };
    let roles = roles_of(tenant, &ctx.principal);
    let rows: Vec<Arc<Record>> = rows
        .iter()
        .map(|record| redact(lookup, &ctx.principal, &roles, ctx.now, record))
        .collect::<Result<_, _>>()?;
    Ok(ReadResult {
        rows,
        revision: tenant.revision,
        schema_version: schema.version,
        next,
        depths: None,
    })
}
