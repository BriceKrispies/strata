//! Object types declared append-only (D-8422, OBS-3128).
//!
//! A type whose definition says `append-only` refuses every update and every delete of a record
//! of it, whoever asks and whatever path the change takes: a plain update or delete, a step of an
//! operation or an automation, the clearing of a reference when a record it refers to is deleted,
//! a rewrite of records by a schema change, the removal of the type with its records, and the
//! expiry of a retention period. The refusal is one guard on the delta of every decision
//! ([`guard`], called by `transition`), so no path is left to remember.
//!
//! The one exception is the erasure of a personal subject (PRV-1586): the records an erasure
//! rewrites are the records it is allowed to rewrite. A restore of a snapshot is guarded by the
//! stores ([`restore_refusal`]) because it does not pass through a decision.
//!
//! A declaration is made by [`SchemaChange::SetAppendOnly`] and is never cleared: a type that
//! could be turned back would be append-only only until somebody asked.

use crate::intent::{
    Check, CheckExt, DeltaOp, DomainError, DomainErrorExt, ErrorKind, Intent, Transition,
};
use crate::state::{Record, State};
use crate::types::{ObjectTypeId, TenantSchema};
use std::collections::BTreeSet;

/// The code a restore refused for lack of append-only records carries.
pub const CODE: &str = "AppendOnly";

/// Whether `object` is declared append-only in `schema`.
pub fn is_append_only(schema: &TenantSchema, object: &ObjectTypeId) -> bool {
    schema
        .objects
        .get(object)
        .is_some_and(|def| def.append_only == Some(true))
}

/// The object types of `schema` declared append-only.
pub fn declared(schema: &TenantSchema) -> BTreeSet<ObjectTypeId> {
    schema
        .objects
        .values()
        .filter(|def| def.append_only == Some(true))
        .map(|def| def.id.clone())
        .collect()
}

fn refusal(rule: &str, message: String) -> DomainError {
    DomainError::new(ErrorKind::AppendOnly, message.clone()).with(vec![Check::fail(rule, message)])
}

/// Refuse a decision whose delta updates or deletes a record of an append-only type.
///
/// The types are those the tenant declares before the decision and those its delta declares, so a
/// change of schema cannot rewrite the records of a type in the commit that declares it. A record
/// is created at version 1 and every later write raises the version, which is how an update is
/// told from a creation.
pub fn guard(state: &State, intent: &Intent, transition: &Transition) -> Result<(), DomainError> {
    let Some(tenant) = state.tenant.as_deref() else {
        return Ok(());
    };
    let mut types = declared(&tenant.schema);
    for op in &transition.delta.ops {
        if let DeltaOp::PutSchema(schema) = op {
            types.extend(declared(schema));
        }
    }
    if types.is_empty() {
        return Ok(());
    }
    let erasing = matches!(intent, Intent::EraseSubject { .. });
    for op in &transition.delta.ops {
        match op {
            DeltaOp::PutRecord(record) if record.version > 1 && types.contains(&record.object) => {
                if !erasing {
                    return Err(refusal(
                        "append_only:no_update",
                        format!(
                            "{} is append-only: record {} cannot be updated",
                            record.object, record.id
                        ),
                    ));
                }
            }
            DeltaOp::DeleteRecord { object, id } if types.contains(object) => {
                return Err(refusal(
                    "append_only:no_delete",
                    format!("{object} is append-only: record {id} cannot be deleted"),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Why a snapshot may not be restored over the records a tenant holds: a record of an append-only
/// type that the snapshot lacks, or holds at another version. `held` are the records of the
/// append-only types now, `snapshot` the records of the state that would be restored. The text
/// begins with [`CODE`].
pub fn restore_refusal<'a>(
    held: impl IntoIterator<Item = &'a Record>,
    snapshot: &crate::state::TenantState,
) -> Option<String> {
    for record in held {
        let there = snapshot
            .records
            .get(&record.object)
            .and_then(|records| records.get(&record.id));
        match there {
            None => {
                return Some(format!(
                    "{CODE}: the snapshot lacks record {} of {}, which is append-only and is never removed",
                    record.id, record.object
                ))
            }
            Some(old) if old.version != record.version => {
                return Some(format!(
                    "{CODE}: the snapshot holds record {} of {} at version {}, not {}, and an append-only record is never rewritten",
                    record.id, record.object, old.version, record.version
                ))
            }
            Some(_) => {}
        }
    }
    None
}
