//! Authorized reads. A read never changes state and never returns a record the principal's
//! policies do not grant.

use crate::expr::{self, Env};
use crate::intent::{
    Check, CheckExt, Context, DomainError, DomainErrorExt, ErrorKind, ReadRequest, ReadResult,
};
use crate::limits;
use crate::rules::{authorize, roles_of, Lookup, Subject};
use crate::schema::{Action, Resource, TenantSchema};
use crate::state::{Need, Principal, Record, State, TenantState};
use crate::types::{PageCursor, QueryDef, RecordId, RoleId, Timestamp, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// What a reader receives of a record: the record without the fields whose `read` condition
/// does not hold for them. A condition that cannot be evaluated hides the field. The system
/// reads everything. Policy conditions, query filters and constraints see the whole record,
/// because they run inside the kernel; only what leaves it is trimmed.
fn redact(
    lookup: Lookup<'_>,
    principal: &Principal,
    roles: &BTreeSet<RoleId>,
    now: Timestamp,
    record: &Arc<Record>,
) -> Arc<Record> {
    if matches!(principal, Principal::System) {
        return record.clone();
    }
    let Some(def) = lookup.tenant.schema.objects.get(&record.object) else {
        return record.clone();
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
    let hidden: Vec<_> = def
        .fields
        .values()
        .filter(|field| {
            field.read.as_ref().is_some_and(|rule| {
                record.fields.contains_key(&field.id) && !expr::holds(rule, &env).unwrap_or(false)
            })
        })
        .map(|field| field.id.clone())
        .collect();
    if hidden.is_empty() {
        return record.clone();
    }
    let mut shown = Record::clone(record);
    for field in &hidden {
        shown.fields.remove(field);
    }
    Arc::new(shown)
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

/// How a named query is served: a window of at most `MAX_PAGE_SIZE` records in id order after a
/// cursor, when the query has no filter and its order is none or the record id ascending, or
/// the sorted scan of the object type, paged by offset.
enum Plan {
    ById { after: Option<RecordId>, size: u32 },
    Scan { offset: usize, size: usize },
}

/// Whether a window of the object type can answer the query (KRN-1287): it has no filter, so a
/// window holds what the page needs, and its order is the order records are stored in.
fn is_windowed(def: &QueryDef) -> bool {
    def.filter.is_none()
        && match def.order.as_slice() {
            [] => true,
            [key] => key.field == "$id" && !key.descending,
            _ => false,
        }
}

/// The page size of a named query read (KRN-1285): the one the request names, else the one the
/// query declares, else the default.
fn page_size(def: &QueryDef, asked: Option<u32>) -> Result<u32, DomainError> {
    let size = asked.or(def.limit).unwrap_or(limits::DEFAULT_PAGE_SIZE);
    if size == 0 {
        return Err(DomainError::new(
            ErrorKind::InvalidIntent,
            "a page size is at least 1",
        ));
    }
    if size > limits::MAX_PAGE_SIZE {
        return Err(DomainError::new(
            ErrorKind::LimitExceeded,
            format!(
                "a page size of {size} is more than the maximum of {}",
                limits::MAX_PAGE_SIZE
            ),
        ));
    }
    Ok(size)
}

fn plan(
    def: &QueryDef,
    asked: Option<u32>,
    cursor: Option<&PageCursor>,
) -> Result<Plan, DomainError> {
    let size = page_size(def, asked)?;
    let wrong = || {
        DomainError::new(
            ErrorKind::InvalidIntent,
            "the cursor is not one this query issued",
        )
    };
    match (is_windowed(def), cursor) {
        (true, None) => Ok(Plan::ById { after: None, size }),
        (true, Some(PageCursor::After(after))) => Ok(Plan::ById {
            after: Some(*after),
            size,
        }),
        (false, None) => Ok(Plan::Scan {
            offset: 0,
            size: size as usize,
        }),
        (false, Some(PageCursor::Offset(offset))) => Ok(Plan::Scan {
            offset: *offset as usize,
            size: size as usize,
        }),
        _ => Err(wrong()),
    }
}

/// The state a read is expected to need. A named query needs a bounded window of its object
/// type, never the whole of it (KRN-1286); a request the kernel will refuse needs nothing.
pub fn read_needs(schema: &TenantSchema, request: &ReadRequest) -> Vec<Need> {
    match request {
        ReadRequest::Named {
            query,
            page_size,
            cursor,
            ..
        } => {
            let Some(def) = schema.queries.get(query) else {
                return Vec::new();
            };
            match plan(def, *page_size, cursor.as_ref()) {
                Ok(Plan::ById { after, .. }) => vec![Need::PageOf {
                    object: def.object.clone(),
                    after,
                    limit: limits::MAX_PAGE_SIZE,
                }],
                Ok(Plan::Scan { .. }) => vec![Need::PageOf {
                    object: def.object.clone(),
                    after: None,
                    limit: limits::MAX_SCAN_RECORDS + 1,
                }],
                Err(_) => Vec::new(),
            }
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

    let mut next = None;
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
                    if visible(tenant, &ctx.principal, record, ctx.now) {
                        rows.push(record.clone());
                    }
                }
            }
            rows
        }
        ReadRequest::Named {
            query,
            params,
            page_size,
            cursor,
        } => {
            let def = schema.queries.get(query).ok_or_else(|| {
                DomainError::new(ErrorKind::UnknownQuery, format!("no query {query}"))
            })?;
            if let Some(extra) = params.keys().find(|name| !def.params.contains(*name)) {
                return Err(DomainError::new(
                    ErrorKind::InvalidIntent,
                    format!("query {query} has no parameter {extra}"),
                ));
            }
            let plan = plan(def, *page_size, cursor.as_ref())?;
            let mut window_end = None;
            let window = match plan {
                Plan::ById { after, .. } => {
                    let window = lookup.page_of(&def.object, after, limits::MAX_PAGE_SIZE)?;
                    if window.len() == limits::MAX_PAGE_SIZE as usize {
                        window_end = window.last().map(|r| r.id);
                    }
                    window
                }
                Plan::Scan { .. } => {
                    let window = lookup.page_of(&def.object, None, limits::MAX_SCAN_RECORDS + 1)?;
                    if window.len() > limits::MAX_SCAN_RECORDS as usize {
                        return Err(DomainError::new(
                            ErrorKind::LimitExceeded,
                            format!(
                                "query {query} orders {} records, more than the scan bound of {}",
                                def.object,
                                limits::MAX_SCAN_RECORDS
                            ),
                        )
                        .with(vec![Check::fail(
                            format!("scan-bound:{query}"),
                            "the query is not served by a window of the object type",
                        )]));
                    }
                    window
                }
            };
            let roles = roles_of(tenant, &ctx.principal);
            let mut keyed = Vec::new();
            for record in window {
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
                        lookup: Some(lookup),
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
            let (offset, size) = match plan {
                Plan::ById { size, .. } => {
                    let size = size as usize;
                    next = match keyed.get(size.wrapping_sub(1)) {
                        Some((_, last)) if keyed.len() > size => Some(last.id),
                        _ => window_end,
                    }
                    .map(PageCursor::After);
                    (0, size)
                }
                Plan::Scan { offset, size } => {
                    if offset.saturating_add(size) < keyed.len() {
                        next = Some(PageCursor::Offset((offset + size) as u32));
                    }
                    (offset, size)
                }
            };
            keyed
                .into_iter()
                .skip(offset)
                .take(size)
                .map(|(_, record)| record)
                .collect()
        }
    };
    let roles = roles_of(tenant, &ctx.principal);
    let rows: Vec<Arc<Record>> = rows
        .iter()
        .map(|record| redact(lookup, &ctx.principal, &roles, ctx.now, record))
        .collect();
    Ok(ReadResult {
        rows,
        revision: tenant.revision,
        schema_version: schema.version,
        next,
    })
}
