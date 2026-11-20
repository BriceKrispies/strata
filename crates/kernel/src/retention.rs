//! Retention: the end of a record's period, and the deletion of records past it (PRV-356, PRV-357,
//! D-6584).
//!
//! A record type may declare `retention`: a number of days counted from the creation of the
//! record, from its last change, or from a timestamp field of the record. This module decides two
//! things and does no I/O:
//!
//! - **Which records are past their period.** [`ended`] answers for one record. [`due_need`] and
//!   [`due_page`] find a batch of candidates the way the store can answer cheaply: a rule that
//!   counts from a timestamp field walks that field's index in the order of the timestamp
//!   (`Need::IndexPage`, the oldest first, up to the cutoff), so the candidates are exactly the
//!   records past the period, and nothing else is read. A rule that counts from the creation or
//!   the last change has no index of its own: its candidates are pages of the type in record id
//!   order (`Need::PageOf`), examined [`BATCH`] at a time through a cursor, and [`ended`] picks
//!   the ones past the period.
//! - **Deleting them.** `Intent::ExpireRecords` is decided by [`decide`] for the system retention
//!   principal alone. It re-checks every record it is given, so a caller that names a record whose
//!   period has not ended, or one that another record still refers to, deletes nothing of it: the
//!   record is listed as kept, with the reason. Each deletion is a `record-deleted` event whose
//!   `before` image has the personal fields removed, so the stream a retention deletes from
//!   carries no personal value of the record (as the events of an erasure carry none, PRV-2462).
//!
//! What the intent does not know: legal holds and restrictions of processing are the shell's
//! (`strata_backend::subject_rights`); it filters the candidates before it names them.

use crate::erasure::BATCH;
use crate::intent::{
    Check, CheckExt, Context, DeltaOp, DomainError, DomainErrorExt, ErrorKind, EventBody, Intent,
    Output,
};
use crate::query_plan::{index_page, IndexBound, IndexPosition};
use crate::rules::{insufficient, Lookup};
use crate::schema::TenantSchemaExt;
use crate::state::{CoverageExt, Need, Principal, Record, RecordExt, TenantStateExt};
use crate::types::{
    ObjectTypeDef, ObjectTypeId, RecordId, RetentionRule, RetentionStart, TenantSchema, Timestamp,
    Value,
};
use std::collections::BTreeSet;
use std::sync::Arc;

/// Milliseconds in a day.
const DAY_MS: i64 = 86_400_000;

/// The result of one decision.
pub struct Expiry {
    pub ops: Vec<DeltaOp>,
    pub events: Vec<EventBody>,
    pub evidence: Vec<Check>,
    pub output: Output,
}

/// The instant the retention period of `record` starts, in milliseconds, or `None` when it has
/// none (a rule that counts from a field the record does not hold).
pub fn start_of(rule: &RetentionRule, record: &Record) -> Option<i64> {
    match &rule.from {
        RetentionStart::Created => Some(record.created_at.0),
        RetentionStart::LastChanged => Some(record.updated_at.0),
        RetentionStart::Field { field } => match record.get(field) {
            Value::Time(at) => Some(*at),
            _ => None,
        },
    }
}

/// The latest start of a period that has ended at `now`: a record whose period starts at or before
/// it is past its retention.
pub fn cutoff(rule: &RetentionRule, now: Timestamp) -> i64 {
    now.0
        .saturating_sub((rule.days as i64).saturating_mul(DAY_MS))
}

/// Whether the retention of `record` has ended at `now`. A type without a rule never ends, and a
/// record without a start (a timestamp field it does not hold) is kept.
pub fn ended(object: &ObjectTypeDef, record: &Record, now: Timestamp) -> bool {
    let Some(rule) = &object.retention else {
        return false;
    };
    start_of(rule, record).is_some_and(|start| start <= cutoff(rule, now))
}

/// The need that finds the next batch of candidates of a type with a retention rule, after the
/// position of the previous batch. `None` for a type without a rule.
pub fn due_need(
    object: &ObjectTypeDef,
    now: Timestamp,
    after: Option<&IndexPosition>,
) -> Option<Need> {
    let rule = object.retention.as_ref()?;
    Some(match &rule.from {
        RetentionStart::Field { field } => Need::IndexPage {
            object: object.id.clone(),
            field: field.clone(),
            lower: IndexBound {
                value: Value::Time(i64::MIN),
                inclusive: true,
            },
            upper: IndexBound {
                value: Value::Time(cutoff(rule, now)),
                inclusive: true,
            },
            descending: false,
            after: after.cloned(),
            limit: BATCH as u32,
        },
        RetentionStart::Created | RetentionStart::LastChanged => Need::PageOf {
            object: object.id.clone(),
            after: after.map(|position| position.id),
            limit: BATCH as u32,
        },
    })
}

/// One batch of candidates.
pub struct DuePage {
    /// The records of the batch whose period has ended, in the order they were examined.
    pub due: Vec<Arc<Record>>,
    /// Where the batch ends: the position to continue after. `None` when nothing was examined.
    pub last: Option<IndexPosition>,
    /// Whether the batch was full, so that another may follow.
    pub full: bool,
}

/// The batch [`due_need`] asks for, from a snapshot that covers it.
pub fn due_page(
    lookup: Lookup<'_>,
    object: &ObjectTypeDef,
    now: Timestamp,
    after: Option<&IndexPosition>,
) -> Result<DuePage, DomainError> {
    let Some(need) = due_need(object, now, after) else {
        return Ok(DuePage {
            due: Vec::new(),
            last: None,
            full: false,
        });
    };
    if !lookup.coverage.covers(&need) {
        return Err(insufficient(need));
    }
    let examined: Vec<&Arc<Record>> = match &need {
        Need::IndexPage {
            field,
            lower,
            upper,
            descending,
            after,
            limit,
            ..
        } => index_page(
            lookup.tenant.records_of(&object.id),
            field,
            lower,
            upper,
            *descending,
            after.as_ref(),
            *limit,
        ),
        Need::PageOf { after, limit, .. } => lookup
            .tenant
            .records_of(&object.id)
            .filter(|record| after.is_none_or(|a| record.id > a))
            .take(*limit as usize)
            .collect(),
        _ => Vec::new(),
    };
    let last = examined.last().map(|record| IndexPosition {
        value: match &object.retention {
            Some(RetentionRule {
                from: RetentionStart::Field { field },
                ..
            }) => record.get(field).clone(),
            _ => Value::Null,
        },
        id: record.id,
    });
    Ok(DuePage {
        full: examined.len() >= BATCH,
        due: examined
            .into_iter()
            .filter(|record| ended(object, record, now))
            .cloned()
            .collect(),
        last,
    })
}

/// The state one decision of `intent` needs: each record named, and each record that refers to it.
pub fn needs(schema: &TenantSchema, intent: &Intent) -> Vec<Need> {
    let Intent::ExpireRecords { object, ids } = intent else {
        return Vec::new();
    };
    let mut out = BTreeSet::new();
    for id in ids {
        out.insert(Need::Record {
            object: object.clone(),
            id: *id,
        });
        for (from, field, _) in schema.referrers(object) {
            out.insert(Need::ByField {
                object: from.id.clone(),
                field: field.id.clone(),
                value: Value::Ref(*id),
            });
        }
    }
    out.into_iter().collect()
}

fn refuse(kind: ErrorKind, rule: &str, message: impl Into<String>) -> DomainError {
    let message = message.into();
    DomainError::new(kind, message.clone()).with(vec![Check::fail(rule, message)])
}

/// The record as an event carries it after a retention: every personal field removed, and the link
/// to the creator when a field is personal to the creator.
fn without_personal(object: &ObjectTypeDef, record: &Arc<Record>) -> Arc<Record> {
    let mut stripped = Record::clone(record);
    let mut creator = false;
    for (id, field) in &object.fields {
        if let Some(personal) = &field.personal {
            stripped.fields.remove(id);
            creator |= matches!(personal.subject, crate::types::DataSubject::Creator);
        }
    }
    if creator {
        stripped.created_by = None;
    }
    Arc::new(stripped)
}

/// Delete the records of `ids` of `object` whose retention has ended and that nothing refers to.
/// Only the system retention principal does it.
pub fn decide(
    lookup: Lookup<'_>,
    ctx: &Context,
    object: &ObjectTypeId,
    ids: &[RecordId],
) -> Result<Expiry, DomainError> {
    if ctx.principal != Principal::System {
        return Err(refuse(
            ErrorKind::Unauthorized,
            "retention:system_only",
            "only the system retention principal deletes records whose retention has ended",
        ));
    }
    let schema: &TenantSchema = &lookup.tenant.schema;
    let Some(def) = schema.objects.get(object) else {
        return Err(refuse(
            ErrorKind::UnknownObject,
            "retention:object_exists",
            format!("no object type {object}"),
        ));
    };
    if def.retention.is_none() {
        return Err(refuse(
            ErrorKind::InvalidIntent,
            "retention:type_has_a_rule",
            format!("{object} declares no retention"),
        ));
    }
    if ids.len() > BATCH {
        return Err(refuse(
            ErrorKind::InvalidIntent,
            "retention:batch",
            format!("at most {BATCH} records are deleted in one decision"),
        ));
    }
    let mut ops = Vec::new();
    let mut events = Vec::new();
    let mut deleted = Vec::new();
    let mut kept = Vec::new();
    let ids: BTreeSet<RecordId> = ids.iter().copied().collect();
    for id in ids {
        // A record that is gone was deleted by an earlier attempt: nothing to do.
        let Some(record) = lookup.record(object, id)? else {
            continue;
        };
        if !ended(def, record, ctx.now) {
            kept.push(format!("{object} {id}: its retention has not ended"));
            continue;
        }
        let mut referred_by = None;
        for (from, field, _) in schema.referrers(object) {
            for referrer in lookup.by_field(&from.id, &field.id, &Value::Ref(id))? {
                if !(referrer.id == id && from.id == *object) {
                    referred_by = Some(format!("{} {}", from.id, referrer.id));
                    break;
                }
            }
            if referred_by.is_some() {
                break;
            }
        }
        if let Some(by) = referred_by {
            kept.push(format!("{object} {id}: still referred to by {by}"));
            continue;
        }
        ops.push(DeltaOp::DeleteRecord {
            object: object.clone(),
            id,
        });
        events.push(EventBody::RecordDeleted {
            before: without_personal(def, record),
        });
        deleted.push(id);
    }
    let evidence = vec![
        Check::pass(
            "retention:system_only",
            "the system retention principal decides",
        ),
        Check::pass(
            "retention:period_ended",
            format!("{} of {object} deleted, {} kept", deleted.len(), kept.len()),
        ),
    ];
    Ok(Expiry {
        ops,
        events,
        evidence,
        output: Output::Expired {
            object: object.clone(),
            deleted,
            kept,
        },
    })
}
