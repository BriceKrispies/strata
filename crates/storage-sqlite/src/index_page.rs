//! The record index's keys and the statements of `Need::IndexPage` (WG-5104, KRN-2525).
//!
//! `record_index` holds one row per field value of a record (one per item of a list), keyed by
//! the value's canonical JSON when that is at most [`INDEX_KEY_BYTES`] long. Two columns added by
//! tenant migration 16 make it answer index pages:
//!
//! - `kind`: the variant of a single value in `Value`'s order (0 null, 1 bool, 2 int, 3 str,
//!   4 time, 5 ref), and NULL for an item of a list. An index page is over single values only, so
//!   a list's items never answer one.
//! - `ord`: the number of an integer or a time, NULL for every other value. Ordering by
//!   `(kind, ord, id)` is ordering by `Value` and then the record id, so a range of integers or of
//!   times is one index range (`record_index_ord`, and `record_index_ord_desc` for the descending
//!   order, whose ties are still in ascending id order).
//!
//! A sealed value (`strata_storage_api::sealed`) has a row for each blind hash it carries and no
//! kind, and none at all when its field is not searchable.
//!
//! An equality page is answered from the primary key `(object, field, value, id)` in id order,
//! for a value of any kind whose key fits; a range page of integers or of times from `ord`. Every
//! other page (a value without a key, a range of another kind or of mixed kinds) is answered by
//! reading the records of the type and applying the kernel's own definition,
//! [`strata_kernel::query_plan::index_page`]; the kernel never asks for one, and the answer is the
//! same either way. Strings are compared by their bytes (SQLite's `BINARY` collation, Rust's
//! `str` order); they are equality-only in the index.
//!
//! Migration 16 cannot compute the new columns in SQL, so it marks the file
//! (`record_index_rebuild`), and [`rebuild_if_marked`] rewrites every index row from the records,
//! in one transaction, when the file is opened. A crash before that commit leaves the mark, and the
//! next open rebuilds again.

use super::{all_of, db, decode, encode, select_strings};
use rusqlite::{params, Connection, Transaction};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::Arc;
use strata_kernel::query_plan::{self, IndexBound, IndexPosition, INDEX_KEY_BYTES};
use strata_kernel::{FieldId, ObjectTypeId, Record, Value};
use strata_storage_api::StoreResult;

/// The tenant migration that adds the keys (step 16 of `TENANT_MIGRATIONS`).
pub(crate) const ORDER_KEYS_SQL: &str = "
    ALTER TABLE record_index ADD COLUMN kind INTEGER;
    ALTER TABLE record_index ADD COLUMN ord INTEGER;
    CREATE INDEX record_index_ord ON record_index(object, field, kind, ord, id)
        WHERE ord IS NOT NULL;
    CREATE INDEX record_index_ord_desc ON record_index(object, field, kind, ord DESC, id)
        WHERE ord IS NOT NULL;
    CREATE TABLE record_index_rebuild (pending INTEGER NOT NULL);
    INSERT INTO record_index_rebuild (pending) VALUES (1);
    ";

/// One row of `record_index`.
pub(crate) struct IndexRow {
    pub field: String,
    pub key: String,
    pub kind: Option<i64>,
    pub ord: Option<i64>,
}

/// The variant of a single value in `Value`'s order; `None` for a list.
fn kind_of(value: &Value) -> Option<i64> {
    match value {
        Value::Null => Some(0),
        Value::Bool(_) => Some(1),
        Value::Int(_) => Some(2),
        Value::Str(_) => Some(3),
        Value::Time(_) => Some(4),
        Value::Ref(_) => Some(5),
        Value::List(_) => None,
    }
}

/// The number an integer or a time is ordered by.
fn ord_of(value: &Value) -> Option<i64> {
    match value {
        Value::Int(n) | Value::Time(n) => Some(*n),
        _ => None,
    }
}

/// The index rows of a record: one per single value and one per list item, each whose key fits.
pub(crate) fn index_rows(record: &Record) -> Vec<IndexRow> {
    let mut rows: BTreeMap<(String, String), (Option<i64>, Option<i64>)> = BTreeMap::new();
    for (field, value) in &record.fields {
        // A sealed value is indexed by its blind hashes only, and not at all when its field is
        // not searchable (D-6585, WG-5112); no index page ever answers one (no kind).
        if let Some(blinds) = strata_storage_api::sealed::index_values(value) {
            for blind in blinds {
                let key = encode(&blind);
                if key.len() <= INDEX_KEY_BYTES {
                    rows.entry((field.as_str().to_string(), key))
                        .or_insert((None, None));
                }
            }
            continue;
        }
        match value {
            Value::List(items) => {
                for item in items {
                    let key = encode(item);
                    if key.len() <= INDEX_KEY_BYTES {
                        rows.entry((field.as_str().to_string(), key))
                            .or_insert((None, None));
                    }
                }
            }
            single => {
                let key = encode(single);
                if key.len() <= INDEX_KEY_BYTES {
                    rows.insert(
                        (field.as_str().to_string(), key),
                        (kind_of(single), ord_of(single)),
                    );
                }
            }
        }
    }
    rows.into_iter()
        .map(|((field, key), (kind, ord))| IndexRow {
            field,
            key,
            kind,
            ord,
        })
        .collect()
}

/// Write the index rows of a record whose old rows are gone.
pub(crate) fn insert_rows(tx: &Transaction<'_>, record: &Record) -> StoreResult<()> {
    let mut insert = tx
        .prepare_cached(
            "INSERT INTO record_index (object, field, value, id, kind, ord)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .map_err(db)?;
    let (object, id) = (record.object.as_str(), record.id.0 as i64);
    for row in index_rows(record) {
        insert
            .execute(params![object, row.field, row.key, id, row.kind, row.ord])
            .map_err(db)?;
    }
    Ok(())
}

/// Rebuild every index row from the records when migration 16 marked the file.
pub(crate) fn rebuild_if_marked(tx: &Transaction<'_>) -> StoreResult<()> {
    let marked: bool = tx
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM record_index_rebuild)",
            [],
            |r| r.get(0),
        )
        .map_err(db)?;
    if !marked {
        return Ok(());
    }
    tx.execute("DELETE FROM record_index", []).map_err(db)?;
    let data = select_strings(tx, "SELECT data FROM records ORDER BY object, id", &[])?;
    for text in data {
        let record: Record = decode(&text, "record")?;
        insert_rows(tx, &record)?;
    }
    tx.execute("DELETE FROM record_index_rebuild", [])
        .map_err(db)?;
    Ok(())
}

/// The records of an index page, as stored JSON, in the page's order.
pub(crate) fn index_page(
    tx: &Connection,
    object: &ObjectTypeId,
    field: &FieldId,
    lower: &IndexBound,
    upper: &IndexBound,
    descending: bool,
    after: Option<&IndexPosition>,
    limit: u32,
) -> StoreResult<Vec<String>> {
    let equality = lower.value == upper.value && lower.inclusive && upper.inclusive;
    let single = !matches!(lower.value, Value::Null | Value::List(_));
    if equality && single {
        let key = encode(&lower.value);
        if key.len() <= INDEX_KEY_BYTES {
            return equal_page(
                tx,
                object,
                field,
                &lower.value,
                &key,
                descending,
                after,
                limit,
            );
        }
    }
    let numbered = match (&lower.value, &upper.value) {
        (Value::Int(lo), Value::Int(hi)) => Some((2, *lo, *hi)),
        (Value::Time(lo), Value::Time(hi)) => Some((4, *lo, *hi)),
        _ => None,
    };
    if let Some((kind, lo, hi)) = numbered {
        let lo = if lower.inclusive {
            Some(lo)
        } else {
            lo.checked_add(1)
        };
        let hi = if upper.inclusive {
            Some(hi)
        } else {
            hi.checked_sub(1)
        };
        let (Some(lo), Some(hi)) = (lo, hi) else {
            return Ok(Vec::new());
        };
        return range_page(tx, object, field, kind, lo, hi, descending, after, limit);
    }
    scanned_page(tx, object, field, lower, upper, descending, after, limit)
}

/// The rows of one value: those after `after` are all of them, the ones with a higher id, or
/// none, as the value compares with the position's in the page's order.
#[allow(clippy::too_many_arguments)]
fn equal_page(
    tx: &Connection,
    object: &ObjectTypeId,
    field: &FieldId,
    value: &Value,
    key: &str,
    descending: bool,
    after: Option<&IndexPosition>,
    limit: u32,
) -> StoreResult<Vec<String>> {
    let above = match after {
        None => -1_i64,
        Some(at) => {
            let ordering = value.cmp(&at.value);
            let ordering = if descending {
                ordering.reverse()
            } else {
                ordering
            };
            match ordering {
                Ordering::Greater => -1,
                Ordering::Equal => at.id.0 as i64,
                Ordering::Less => return Ok(Vec::new()),
            }
        }
    };
    select_strings(
        tx,
        "SELECT r.data FROM record_index i
         JOIN records r ON r.object = i.object AND r.id = i.id
         WHERE i.object = ?1 AND i.field = ?2 AND i.value = ?3 AND i.kind IS NOT NULL
           AND i.id > ?4
         ORDER BY i.id LIMIT ?5",
        &[
            &object.as_str(),
            &field.as_str(),
            &key,
            &above,
            &(limit as i64),
        ],
    )
}

/// The rows of integers (kind 2) or times (kind 4) from `lo` to `hi` inclusive, after `after`,
/// in the page's order: first the rest of the position's own value in id order, then the values
/// beyond it. Each part is one range of an index.
#[allow(clippy::too_many_arguments)]
fn range_page(
    tx: &Connection,
    object: &ObjectTypeId,
    field: &FieldId,
    kind: i64,
    lo: i64,
    hi: i64,
    descending: bool,
    after: Option<&IndexPosition>,
    limit: u32,
) -> StoreResult<Vec<String>> {
    let (mut lo, mut hi) = (lo, hi);
    let mut rows = Vec::new();
    if let Some(at) = after {
        match (kind_of(&at.value), ord_of(&at.value)) {
            (Some(k), Some(n)) if k == kind => {
                if (lo..=hi).contains(&n) {
                    rows = select_strings(
                        tx,
                        "SELECT r.data FROM record_index i
                         JOIN records r ON r.object = i.object AND r.id = i.id
                         WHERE i.object = ?1 AND i.field = ?2 AND i.kind = ?3 AND i.ord = ?4
                           AND i.id > ?5
                         ORDER BY i.id LIMIT ?6",
                        &[
                            &object.as_str(),
                            &field.as_str(),
                            &kind,
                            &n,
                            &(at.id.0 as i64),
                            &(limit as i64),
                        ],
                    )?;
                }
                // The values beyond the position's own.
                if descending {
                    match n.checked_sub(1) {
                        Some(m) => hi = hi.min(m),
                        None => return Ok(rows),
                    }
                } else {
                    match n.checked_add(1) {
                        Some(m) => lo = lo.max(m),
                        None => return Ok(rows),
                    }
                }
            }
            // A position of another kind comes before or after every value of this kind, as it
            // compares with any one of them.
            _ => {
                let sample = if kind == 2 {
                    Value::Int(lo)
                } else {
                    Value::Time(lo)
                };
                let passed = match at.value.cmp(&sample) {
                    Ordering::Less => descending,
                    _ => !descending,
                };
                if passed {
                    return Ok(Vec::new());
                }
            }
        }
    }
    let left = limit as usize - rows.len().min(limit as usize);
    if left == 0 || lo > hi {
        return Ok(rows);
    }
    let sql = if descending {
        "SELECT r.data FROM record_index i
         JOIN records r ON r.object = i.object AND r.id = i.id
         WHERE i.object = ?1 AND i.field = ?2 AND i.kind = ?3 AND i.ord >= ?4 AND i.ord <= ?5
         ORDER BY i.ord DESC, i.id ASC LIMIT ?6"
    } else {
        "SELECT r.data FROM record_index i
         JOIN records r ON r.object = i.object AND r.id = i.id
         WHERE i.object = ?1 AND i.field = ?2 AND i.kind = ?3 AND i.ord >= ?4 AND i.ord <= ?5
         ORDER BY i.ord ASC, i.id ASC LIMIT ?6"
    };
    rows.extend(select_strings(
        tx,
        sql,
        &[
            &object.as_str(),
            &field.as_str(),
            &kind,
            &lo,
            &hi,
            &(left as i64),
        ],
    )?);
    Ok(rows)
}

/// Any other page: the kernel's definition over the records of the type.
#[allow(clippy::too_many_arguments)]
fn scanned_page(
    tx: &Connection,
    object: &ObjectTypeId,
    field: &FieldId,
    lower: &IndexBound,
    upper: &IndexBound,
    descending: bool,
    after: Option<&IndexPosition>,
    limit: u32,
) -> StoreResult<Vec<String>> {
    let mut records = Vec::new();
    let mut texts = BTreeMap::new();
    for text in all_of(tx, object)? {
        let record: Record = decode(&text, "record")?;
        texts.insert(record.id, text);
        records.push(Arc::new(record));
    }
    Ok(query_plan::index_page(
        records.iter(),
        field,
        lower,
        upper,
        descending,
        after,
        limit,
    )
    .into_iter()
    .filter_map(|record| texts.remove(&record.id))
    .collect())
}
