//! The pair index of named queries and the statements of `Need::PairPage` (KRN-2920, KRN-2921,
//! D-7920).
//!
//! A query such as "posts by this author, newest first" (`author == p` ordered by `$created_at`
//! descending) is served from the pair index (author, `$created_at`): for each value of the field,
//! that value's records in the order of the attribute and then the record id. A file keeps such an
//! index for exactly the pairs some defined query's plan reads
//! ([`strata_kernel::query_plan::index_pairs`]), listed in `record_pairs`, so that a write costs
//! one row per pair the tenant's queries use and nothing for pairs nobody queries.
//!
//! `record_pair_index` holds one row per record and pair whose field holds a single value (not
//! `null`, not a list) with a canonical JSON key of at most [`INDEX_KEY_BYTES`] bytes. A value
//! with a longer key has no row, and no probe the index serves can equal it (a probe whose value
//! is longer is answered by a scan in the kernel). The row's order key is the attribute's value:
//! `kind` 0 for a missing or `null` value (with `ord` 0), 2 for an integer and 4 for a time (with
//! `ord` the number), so ordering by `(kind, ord, id)` is ordering by `Value` and then the record
//! id. A value of any other kind cannot be placed in that order: its row has `kind` and `ord`
//! NULL, and while such a row exists for a pair the pair's pages are answered by reading the
//! records of the type and applying the kernel's own definition,
//! [`strata_kernel::query_plan::pair_page`], so the record is still found in its place. That row
//! is the counter: it is written and removed with the record, in the same transaction.
//!
//! The rows of a pair are built when a schema change first defines a query that reads it, in the
//! commit of that change, from the records of the type ([`sync`]); they are dropped in the commit
//! of the change after which no query reads the pair. A file opened with queries defined before
//! this table existed is brought up to date the same way when it is opened ([`sync_stored`]).

use super::{all_of, db, decode, encode, select_strings};
use rusqlite::{params, Connection, Transaction};
use std::collections::BTreeSet;
use std::sync::Arc;
use strata_kernel::ext::*;
use strata_kernel::query_plan::{self, IndexPosition, INDEX_KEY_BYTES};
use strata_kernel::{FieldId, ObjectTypeId, Record, TenantSchema, Value};
use strata_storage_api::StoreResult;

/// The tenant migration that adds the pair index. It is the last step of `TENANT_MIGRATIONS`.
pub(crate) const PAIR_INDEX_SQL: &str = "
    CREATE TABLE record_pairs (
        object TEXT NOT NULL,
        field  TEXT NOT NULL,
        attr   TEXT NOT NULL,
        PRIMARY KEY (object, field, attr)
    );
    CREATE TABLE record_pair_index (
        object TEXT NOT NULL,
        field  TEXT NOT NULL,
        attr   TEXT NOT NULL,
        value  TEXT NOT NULL,
        kind   INTEGER,
        ord    INTEGER,
        id     INTEGER NOT NULL,
        PRIMARY KEY (object, field, attr, value, id)
    );
    CREATE INDEX record_pair_order
        ON record_pair_index(object, field, attr, value, kind, ord, id);
    CREATE INDEX record_pair_order_desc
        ON record_pair_index(object, field, attr, value, kind DESC, ord DESC, id);
    CREATE INDEX record_pair_rows ON record_pair_index(object, id);
    CREATE INDEX record_pair_unordered ON record_pair_index(object, field, attr)
        WHERE kind IS NULL;
    ";

/// The order key of an attribute's value: `(kind, ord)`, or `None` for a value the index does not
/// order.
fn order_key(value: &Value) -> Option<(i64, i64)> {
    match value {
        Value::Null => Some((0, 0)),
        Value::Int(n) => Some((2, *n)),
        Value::Time(n) => Some((4, *n)),
        _ => None,
    }
}

/// The rank of a value's kind in `Value`'s order (0 null, 1 bool, 2 int, 3 str, 4 time, 5 ref,
/// 6 list).
fn kind_rank(value: &Value) -> i64 {
    match value {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Int(_) => 2,
        Value::Str(_) => 3,
        Value::Time(_) => 4,
        Value::Ref(_) => 5,
        Value::List(_) => 6,
    }
}

/// The key of an equality value, when the index holds rows for it.
fn value_key(value: &Value) -> Option<String> {
    if matches!(value, Value::Null | Value::List(_)) {
        return None;
    }
    let key = encode(value);
    (key.len() <= INDEX_KEY_BYTES).then_some(key)
}

/// The pairs the file keeps for an object type, as (field, attribute).
fn pairs_of(tx: &Connection, object: &str) -> StoreResult<Vec<(String, String)>> {
    let mut statement = tx
        .prepare_cached(
            "SELECT field, attr FROM record_pairs WHERE object = ?1 ORDER BY field, attr",
        )
        .map_err(db)?;
    let rows = statement
        .query_map(params![object], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(db)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db)
}

/// Write the row of one record for one pair, when its field holds a value the index holds.
fn insert_row(tx: &Connection, record: &Record, field: &str, attr: &str) -> StoreResult<()> {
    let Ok(field_id) = FieldId::new(field) else {
        return Ok(());
    };
    let Some(key) = value_key(record.get(&field_id)) else {
        return Ok(());
    };
    let (kind, ord) = match order_key(&query_plan::order_value(record, attr)) {
        Some((kind, ord)) => (Some(kind), Some(ord)),
        None => (None, None),
    };
    tx.prepare_cached(
        "INSERT INTO record_pair_index (object, field, attr, value, kind, ord, id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .map_err(db)?
    .execute(params![
        record.object.as_str(),
        field,
        attr,
        key,
        kind,
        ord,
        record.id.0 as i64
    ])
    .map_err(db)?;
    Ok(())
}

/// Replace the pair rows of a record that was just written.
pub(crate) fn write_rows(tx: &Transaction<'_>, record: &Record) -> StoreResult<()> {
    let object = record.object.as_str();
    delete_rows(tx, &record.object, record.id.0)?;
    for (field, attr) in pairs_of(tx, object)? {
        insert_row(tx, record, &field, &attr)?;
    }
    Ok(())
}

/// Remove the pair rows of a record.
pub(crate) fn delete_rows(tx: &Connection, object: &ObjectTypeId, id: u64) -> StoreResult<()> {
    tx.execute(
        "DELETE FROM record_pair_index WHERE object = ?1 AND id = ?2",
        params![object.as_str(), id as i64],
    )
    .map_err(db)?;
    Ok(())
}

/// Keep exactly the pairs the queries of `schema` read (KRN-2921): drop the rows of a pair no
/// query reads any more, and build the rows of a new pair from the records of its type, in the
/// caller's transaction (the commit of the schema change).
pub(crate) fn sync(tx: &Transaction<'_>, schema: &TenantSchema) -> StoreResult<()> {
    let wanted: BTreeSet<(String, String, String)> = query_plan::index_pairs(schema)
        .into_iter()
        .map(|pair| {
            (
                pair.object.as_str().to_string(),
                pair.field.as_str().to_string(),
                pair.order,
            )
        })
        .collect();
    let held: BTreeSet<(String, String, String)> = {
        let mut statement = tx
            .prepare_cached("SELECT object, field, attr FROM record_pairs")
            .map_err(db)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(db)?;
        rows.collect::<Result<BTreeSet<_>, _>>().map_err(db)?
    };
    for (object, field, attr) in held.difference(&wanted) {
        tx.execute(
            "DELETE FROM record_pair_index WHERE object = ?1 AND field = ?2 AND attr = ?3",
            params![object, field, attr],
        )
        .map_err(db)?;
        tx.execute(
            "DELETE FROM record_pairs WHERE object = ?1 AND field = ?2 AND attr = ?3",
            params![object, field, attr],
        )
        .map_err(db)?;
    }
    for (object, field, attr) in wanted.difference(&held) {
        tx.execute(
            "INSERT INTO record_pairs (object, field, attr) VALUES (?1, ?2, ?3)",
            params![object, field, attr],
        )
        .map_err(db)?;
        let Ok(object_id) = ObjectTypeId::new(object.as_str()) else {
            continue;
        };
        for text in all_of(tx, &object_id)? {
            let record: Record = decode(&text, "record")?;
            insert_row(tx, &record, field, attr)?;
        }
    }
    Ok(())
}

/// Forget every pair and its rows, then keep those `schema` reads: for a restore, whose records
/// are written afterwards, each with its rows.
pub(crate) fn reset(tx: &Transaction<'_>, schema: &TenantSchema) -> StoreResult<()> {
    tx.execute("DELETE FROM record_pair_index", [])
        .map_err(db)?;
    tx.execute("DELETE FROM record_pairs", []).map_err(db)?;
    sync(tx, schema)
}

/// Bring the pairs of an opened file up to date with its stored schema.
pub(crate) fn sync_stored(tx: &Transaction<'_>) -> StoreResult<()> {
    let schemas = select_strings(tx, "SELECT schema FROM tenant", &[])?;
    for text in schemas {
        let schema: TenantSchema = decode(&text, "tenant schema")?;
        sync(tx, &schema)?;
    }
    Ok(())
}

/// The records of a pair page, as stored JSON, in the page's order.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pair_page(
    tx: &Connection,
    object: &ObjectTypeId,
    field: &FieldId,
    value: &Value,
    attr: &str,
    descending: bool,
    after: Option<&IndexPosition>,
    limit: u32,
) -> StoreResult<Vec<String>> {
    let Some(key) = value_key(value) else {
        // A null or a list equals no single value; a longer key has no row.
        return scanned_page(tx, object, field, value, attr, descending, after, limit);
    };
    let (object_name, field_name) = (object.as_str(), field.as_str());
    let kept: bool = tx
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM record_pairs
                            WHERE object = ?1 AND field = ?2 AND attr = ?3)
                AND NOT EXISTS (SELECT 1 FROM record_pair_index
                                WHERE object = ?1 AND field = ?2 AND attr = ?3
                                  AND kind IS NULL)",
            params![object_name, field_name, attr],
            |r| r.get(0),
        )
        .map_err(db)?;
    if !kept {
        // No query reads the pair, or a record's value of the attribute is not in its order.
        return scanned_page(tx, object, field, value, attr, descending, after, limit);
    }
    let mut rows = Vec::new();
    // Where the page starts: after the position's (kind, ord), once the rest of that key's own
    // records (by id) are taken.
    let mut beyond: Option<(i64, i64)> = None;
    if let Some(at) = after {
        match order_key(&at.value) {
            Some((kind, ord)) => {
                rows = select_strings(
                    tx,
                    "SELECT r.data FROM record_pair_index i
                     JOIN records r ON r.object = i.object AND r.id = i.id
                     WHERE i.object = ?1 AND i.field = ?2 AND i.attr = ?3 AND i.value = ?4
                       AND i.kind = ?5 AND i.ord = ?6 AND i.id > ?7
                     ORDER BY i.id LIMIT ?8",
                    &[
                        &object_name,
                        &field_name,
                        &attr,
                        &key,
                        &kind,
                        &ord,
                        &(at.id.0 as i64),
                        &(limit as i64),
                    ],
                )?;
                beyond = Some((kind, ord));
            }
            // A position of a kind the index does not order sits between kinds: the rows beyond
            // it are those of the kinds on its far side.
            None => {
                let rank = kind_rank(&at.value);
                beyond = Some(if descending {
                    (rank, i64::MIN)
                } else {
                    (rank, i64::MAX)
                });
            }
        }
    }
    let left = limit as usize - rows.len().min(limit as usize);
    if left == 0 {
        return Ok(rows);
    }
    let more = match (beyond, descending) {
        (None, false) => select_strings(
            tx,
            "SELECT r.data FROM record_pair_index i
             JOIN records r ON r.object = i.object AND r.id = i.id
             WHERE i.object = ?1 AND i.field = ?2 AND i.attr = ?3 AND i.value = ?4
               AND i.kind IS NOT NULL
             ORDER BY i.kind ASC, i.ord ASC, i.id ASC LIMIT ?5",
            &[&object_name, &field_name, &attr, &key, &(left as i64)],
        )?,
        (None, true) => select_strings(
            tx,
            "SELECT r.data FROM record_pair_index i
             JOIN records r ON r.object = i.object AND r.id = i.id
             WHERE i.object = ?1 AND i.field = ?2 AND i.attr = ?3 AND i.value = ?4
               AND i.kind IS NOT NULL
             ORDER BY i.kind DESC, i.ord DESC, i.id ASC LIMIT ?5",
            &[&object_name, &field_name, &attr, &key, &(left as i64)],
        )?,
        (Some((kind, ord)), false) => select_strings(
            tx,
            "SELECT r.data FROM record_pair_index i
             JOIN records r ON r.object = i.object AND r.id = i.id
             WHERE i.object = ?1 AND i.field = ?2 AND i.attr = ?3 AND i.value = ?4
               AND i.kind IS NOT NULL AND (i.kind, i.ord) > (?5, ?6)
             ORDER BY i.kind ASC, i.ord ASC, i.id ASC LIMIT ?7",
            &[
                &object_name,
                &field_name,
                &attr,
                &key,
                &kind,
                &ord,
                &(left as i64),
            ],
        )?,
        (Some((kind, ord)), true) => select_strings(
            tx,
            "SELECT r.data FROM record_pair_index i
             JOIN records r ON r.object = i.object AND r.id = i.id
             WHERE i.object = ?1 AND i.field = ?2 AND i.attr = ?3 AND i.value = ?4
               AND i.kind IS NOT NULL AND (i.kind, i.ord) < (?5, ?6)
             ORDER BY i.kind DESC, i.ord DESC, i.id ASC LIMIT ?7",
            &[
                &object_name,
                &field_name,
                &attr,
                &key,
                &kind,
                &ord,
                &(left as i64),
            ],
        )?,
    };
    rows.extend(more);
    Ok(rows)
}

/// Any other page: the kernel's definition over the records of the type.
#[allow(clippy::too_many_arguments)]
fn scanned_page(
    tx: &Connection,
    object: &ObjectTypeId,
    field: &FieldId,
    value: &Value,
    attr: &str,
    descending: bool,
    after: Option<&IndexPosition>,
    limit: u32,
) -> StoreResult<Vec<String>> {
    let mut records = Vec::new();
    let mut texts = std::collections::BTreeMap::new();
    for text in all_of(tx, object)? {
        let record: Record = decode(&text, "record")?;
        texts.insert(record.id, text);
        records.push(Arc::new(record));
    }
    Ok(
        query_plan::pair_page(records.iter(), field, value, attr, descending, after, limit)
            .into_iter()
            .filter_map(|record| texts.remove(&record.id))
            .collect(),
    )
}
