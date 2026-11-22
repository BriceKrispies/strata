//! The records of one data subject, read and not changed: the enumeration of the subject export
//! (CT-210, PRV-350, PRV-1586) and the question of whom a record concerns.
//!
//! The enumeration is the erasure's (`crate::erasure`): the same declarations (`declared`), the
//! same needs (`by-field` for the text fields that name the subject, pages by record id for the
//! types whose subject is the creator), the same test of whether a field belongs to the subject
//! (`is_subject_of`). The two read the same set, so an export can never lack a field an erasure
//! clears and an erasure can never clear a field an export did not hold.
//!
//! One call reads at most [`BATCH`] records and says where the next call continues, so a subject
//! with many records is exported in bounded steps (a job of the backend). The cursor has two
//! phases: first the records that name the subject through a text field, in object and record id
//! order; then, one object type at a time, pages of the types whose subject is the creator.
//! A record may come in both phases; the caller keeps one entry for each `(object, id)`.

use crate::erasure::{declared, is_subject_of, page_need, scan_from, BATCH};
use crate::intent::DomainError;
use crate::rules::Lookup;
use crate::state::{Need, Record, RecordExt};
use crate::types::{
    DataSubject, FieldId, ObjectTypeDef, ObjectTypeId, PrincipalId, RecordId, TenantSchema,
    Timestamp, Value, ValueExt,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Where an export continues.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportCursor {
    /// 1: records that name the subject through a text field. 2: records the subject created.
    /// 0 is read as 1, so a default cursor is the start.
    #[serde(default)]
    pub phase: u8,
    /// Phase 1: continue after this `(object, after)` pair. Phase 2: the object type being paged.
    #[serde(default)]
    pub object: Option<ObjectTypeId>,
    #[serde(default)]
    pub after: Option<RecordId>,
}

/// One record of the subject, with the personal fields that belong to the subject and no other.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportedRecord {
    pub object: ObjectTypeId,
    pub id: RecordId,
    pub version: u64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// The principal that created the record, only when the subject is that principal and a
    /// field is declared personal to the creator (erasure clears the link with those fields).
    pub created_by: Option<PrincipalId>,
    /// Each personal field whose subject is the principal, with its value. A field of another
    /// subject or without a personal declaration is not here.
    pub fields: BTreeMap<FieldId, Value>,
}

/// One step of an export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportPage {
    pub records: Vec<ExportedRecord>,
    /// Where the next call continues; `None` when the enumeration is complete.
    pub next: Option<ExportCursor>,
}

/// The state one call of [`export_page`] needs: the same needs the erasure has.
pub fn export_needs(
    schema: &TenantSchema,
    subject: &PrincipalId,
    cursor: &ExportCursor,
) -> Vec<Need> {
    let declared = declared(schema);
    let mut out = BTreeSet::new();
    if cursor.phase <= 1 {
        for d in &declared {
            for link in &d.links {
                out.insert(Need::ByField {
                    object: d.object.id.clone(),
                    field: link.clone(),
                    value: Value::Str(subject.as_str().to_string()),
                });
            }
        }
    } else if let Some(d) = scan_from(&declared, cursor.object.as_ref()) {
        let after = if cursor.object.as_ref() == Some(&d.object.id) {
            cursor.after
        } else {
            None
        };
        out.insert(page_need(&d.object.id, after));
    }
    out.into_iter().collect()
}

/// The personal fields of `record` that belong to `subject`, as the export holds them. `None` when
/// the record holds nothing of the subject.
fn exported(
    object: &ObjectTypeDef,
    record: &Record,
    subject: &PrincipalId,
) -> Option<ExportedRecord> {
    let mut fields = BTreeMap::new();
    let mut creator_link = false;
    for (id, field) in &object.fields {
        if !is_subject_of(field, record, subject) {
            continue;
        }
        if matches!(
            field.personal.as_ref().map(|p| &p.subject),
            Some(DataSubject::Creator)
        ) {
            creator_link = true;
        }
        let value = record.get(id);
        if !value.is_null() {
            fields.insert(id.clone(), value.clone());
        }
    }
    let created_by = if creator_link {
        record.created_by.clone()
    } else {
        None
    };
    if fields.is_empty() && created_by.is_none() {
        return None;
    }
    Some(ExportedRecord {
        object: object.id.clone(),
        id: record.id,
        version: record.version,
        created_at: record.created_at,
        updated_at: record.updated_at,
        created_by,
        fields,
    })
}

/// Read one step of the export of `subject`, from `cursor`. The snapshot must cover
/// [`export_needs`] of the same cursor.
pub fn export_page(
    lookup: Lookup<'_>,
    subject: &PrincipalId,
    cursor: &ExportCursor,
) -> Result<ExportPage, DomainError> {
    let schema: &TenantSchema = &lookup.tenant.schema;
    let declared = declared(schema);
    let mut records = Vec::new();
    if cursor.phase <= 1 {
        let value = Value::Str(subject.as_str().to_string());
        let mut found: BTreeMap<(ObjectTypeId, RecordId), ExportedRecord> = BTreeMap::new();
        for d in &declared {
            for link in &d.links {
                for record in lookup.by_field(&d.object.id, link, &value)? {
                    let key = (d.object.id.clone(), record.id);
                    let past = match &cursor.object {
                        Some(object) => {
                            key <= (object.clone(), cursor.after.unwrap_or(RecordId(0)))
                        }
                        None => false,
                    };
                    if past || found.contains_key(&key) {
                        continue;
                    }
                    if let Some(entry) = exported(d.object, record, subject) {
                        found.insert(key, entry);
                    }
                }
            }
        }
        let more = found.len() > BATCH;
        records.extend(found.into_values().take(BATCH));
        if more {
            let last = records.last().map(|r| (r.object.clone(), r.id));
            return Ok(ExportPage {
                records,
                next: last.map(|(object, id)| ExportCursor {
                    phase: 1,
                    object: Some(object),
                    after: Some(id),
                }),
            });
        }
        let next = scan_from(&declared, None).map(|_| ExportCursor {
            phase: 2,
            object: None,
            after: None,
        });
        return Ok(ExportPage { records, next });
    }

    let Some(d) = scan_from(&declared, cursor.object.as_ref()) else {
        return Ok(ExportPage {
            records,
            next: None,
        });
    };
    let after = if cursor.object.as_ref() == Some(&d.object.id) {
        cursor.after
    } else {
        None
    };
    let page = lookup.page_of(&d.object.id, after, BATCH as u32)?;
    for record in &page {
        if record.created_by.as_ref() == Some(subject) {
            if let Some(entry) = exported(d.object, record, subject) {
                records.push(entry);
            }
        }
    }
    let next = if page.len() == BATCH {
        Some(ExportCursor {
            phase: 2,
            object: Some(d.object.id.clone()),
            after: page.last().map(|record| record.id),
        })
    } else {
        declared
            .iter()
            .find(|other| other.creator && other.object.id > d.object.id)
            .map(|following| ExportCursor {
                phase: 2,
                object: Some(following.object.id.clone()),
                after: None,
            })
    };
    Ok(ExportPage { records, next })
}

/// Every principal some personal field of `record` names as its subject: the creator when a field
/// is declared personal to the creator, and each principal id held in a field that names the
/// subject of another. Retention and the restriction of processing use it to ask whether a
/// record concerns a restricted or held person.
pub fn subjects_of(object: &ObjectTypeDef, record: &Record) -> BTreeSet<PrincipalId> {
    let mut found = BTreeSet::new();
    for field in object.fields.values() {
        let Some(personal) = &field.personal else {
            continue;
        };
        match &personal.subject {
            DataSubject::Creator => {
                if let Some(creator) = &record.created_by {
                    found.insert(creator.clone());
                }
            }
            DataSubject::Field { field: link } => collect(record.get(link), &mut found),
        }
    }
    found
}

fn collect(value: &Value, into: &mut BTreeSet<PrincipalId>) {
    match value {
        Value::Str(text) => {
            if let Ok(id) = PrincipalId::new(text.as_str()) {
                into.insert(id);
            }
        }
        Value::List(items) => {
            for item in items {
                collect(item, into);
            }
        }
        _ => {}
    }
}
