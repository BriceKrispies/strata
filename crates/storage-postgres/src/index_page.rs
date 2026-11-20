//! The record index's keys and the statements of `Need::IndexPage` (WG-5104, KRN-2525), as the
//! SQLite adapter's `index_page.rs` keeps them.
//!
//! `record_index` holds one row per field value of a record (one per item of a list), keyed by
//! the value's canonical JSON when that is at most [`INDEX_KEY_BYTES`] long; `kind` is the variant
//! of a single value in `Value`'s order (NULL for a list item) and `ord` the number of an integer or
//! a time. An equality page is answered from the primary key in id order, a range page of integers
//! or of times from `ord`, and every other page by reading the records of the type and applying
//! the kernel's own definition, [`strata_kernel::query_plan::index_page`].

use crate::{all_of, db, decode, encode, select_strings};
use postgres::Transaction;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::Arc;
use strata_kernel::query_plan::{self, IndexBound, IndexPosition, INDEX_KEY_BYTES};
use strata_kernel::{FieldId, ObjectTypeId, Record, Value};
use strata_storage_api::StoreResult;

/// Tenant step 16: the order keys. A schema that had index rows before the step is marked, and
/// [`rebuild_if_marked`] rewrites every row from the records when the schema is next prepared.
pub(crate) const ORDER_KEYS_SQL: &str = "
    ALTER TABLE record_index ADD COLUMN kind BIGINT;
    ALTER TABLE record_index ADD COLUMN ord BIGINT;
    CREATE INDEX record_index_ord ON record_index(object, field, kind, ord, id)
        WHERE ord IS NOT NULL;
    CREATE INDEX record_index_ord_desc ON record_index(object, field, kind, ord DESC, id)
        WHERE ord IS NOT NULL;
    CREATE TABLE record_index_rebuild (pending BIGINT NOT NULL);
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
pub(crate) fn insert_rows(t: &mut Transaction<'_>, record: &Record) -> StoreResult<()> {
    let (object, id) = (record.object.as_str(), record.id.0 as i64);
    for row in index_rows(record) {
        t.execute(
            "INSERT INTO record_index (object, field, value, id, kind, ord)
             VALUES ($1, $2, $3, $4, $5, $6)",
            &[&object, &row.field, &row.key, &id, &row.kind, &row.ord],
        )
        .map_err(db)?;
    }
    Ok(())
}

/// Rebuild every index row from the records when step 16 marked the schema.
pub(crate) fn rebuild_if_marked(t: &mut Transaction<'_>) -> StoreResult<()> {
    let row = t
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM record_index_rebuild)",
            &[],
        )
        .map_err(db)?;
    let marked: bool = crate::get(&row, 0)?;
    if !marked {
        return Ok(());
    }
    t.execute("DELETE FROM record_index", &[]).map_err(db)?;
    let data = select_strings(t, "SELECT data FROM records ORDER BY object, id", &[])?;
    for text in data {
        let record: Record = decode(&text, "record")?;
        insert_rows(t, &record)?;
    }
    t.execute("DELETE FROM record_index_rebuild", &[])
        .map_err(db)?;
    Ok(())
}

/// The records of an index page, as stored JSON, in the page's order.
#[allow(clippy::too_many_arguments)]
pub(crate) fn index_page(
    t: &mut Transaction<'_>,
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
                t,
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
        return range_page(t, object, field, kind, lo, hi, descending, after, limit);
    }
    scanned_page(t, object, field, lower, upper, descending, after, limit)
}

/// The rows of one value: those after `after` are all of them, the ones with a higher id, or
/// none, as the value compares with the position's in the page's order.
#[allow(clippy::too_many_arguments)]
fn equal_page(
    t: &mut Transaction<'_>,
    object: &ObjectTypeId,
    field: &FieldId,
    value: &Value,
    key: &str,
    descending: bool,
    after: Option<&IndexPosition>,
    limit: u32,
) -> StoreResult<Vec<String>> {
    let above: i64 = match after {
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
    let limit: i64 = i64::from(limit);
    select_strings(
        t,
        "SELECT r.data FROM record_index i
         JOIN records r ON r.object = i.object AND r.id = i.id
         WHERE i.object = $1 AND i.field = $2 AND i.value = $3 AND i.kind IS NOT NULL
           AND i.id > $4
         ORDER BY i.id LIMIT $5",
        &[&object.as_str(), &field.as_str(), &key, &above, &limit],
    )
}

/// The rows of integers (kind 2) or times (kind 4) from `lo` to `hi` inclusive, after `after`,
/// in the page's order: first the rest of the position's own value in id order, then the values
/// beyond it.
#[allow(clippy::too_many_arguments)]
fn range_page(
    t: &mut Transaction<'_>,
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
                    let id: i64 = at.id.0 as i64;
                    let cap: i64 = i64::from(limit);
                    rows = select_strings(
                        t,
                        "SELECT r.data FROM record_index i
                         JOIN records r ON r.object = i.object AND r.id = i.id
                         WHERE i.object = $1 AND i.field = $2 AND i.kind = $3 AND i.ord = $4
                           AND i.id > $5
                         ORDER BY i.id LIMIT $6",
                        &[&object.as_str(), &field.as_str(), &kind, &n, &id, &cap],
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
         WHERE i.object = $1 AND i.field = $2 AND i.kind = $3 AND i.ord >= $4 AND i.ord <= $5
         ORDER BY i.ord DESC, i.id ASC LIMIT $6"
    } else {
        "SELECT r.data FROM record_index i
         JOIN records r ON r.object = i.object AND r.id = i.id
         WHERE i.object = $1 AND i.field = $2 AND i.kind = $3 AND i.ord >= $4 AND i.ord <= $5
         ORDER BY i.ord ASC, i.id ASC LIMIT $6"
    };
    let left: i64 = left as i64;
    rows.extend(select_strings(
        t,
        sql,
        &[&object.as_str(), &field.as_str(), &kind, &lo, &hi, &left],
    )?);
    Ok(rows)
}

/// Any other page: the kernel's definition over the records of the type.
#[allow(clippy::too_many_arguments)]
fn scanned_page(
    t: &mut Transaction<'_>,
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
    for text in all_of(t, object)? {
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
