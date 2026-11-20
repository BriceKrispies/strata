//! The pair index of named queries and the statements of `Need::PairPage` (KRN-2920, KRN-2921,
//! D-7920), as the SQLite adapter's `pair_index.rs` keeps them: one row per record and pair (a
//! field and an order attribute some defined query's plan reads) whose field holds a single value
//! with a key of at most [`INDEX_KEY_BYTES`] bytes, ordered by `(kind, ord, id)`. A value of the
//! attribute the index cannot order has a row with `kind` NULL, and while such a row exists the
//! pair's pages are answered by the kernel's own definition over the records of the type.

use crate::{all_of, db, decode, encode, get, select_strings};
use postgres::Transaction;
use std::collections::BTreeSet;
use std::sync::Arc;
use strata_kernel::ext::*;
use strata_kernel::query_plan::{self, IndexPosition, INDEX_KEY_BYTES};
use strata_kernel::{FieldId, ObjectTypeId, Record, TenantSchema, Value};
use strata_storage_api::StoreResult;

/// Tenant step 17: the pair index.
pub(crate) const PAIR_INDEX_SQL: &str = r#"
    CREATE TABLE record_pairs (
        object TEXT COLLATE "C" NOT NULL,
        field  TEXT COLLATE "C" NOT NULL,
        attr   TEXT COLLATE "C" NOT NULL,
        PRIMARY KEY (object, field, attr)
    );
    CREATE TABLE record_pair_index (
        object TEXT COLLATE "C" NOT NULL,
        field  TEXT COLLATE "C" NOT NULL,
        attr   TEXT COLLATE "C" NOT NULL,
        value  TEXT COLLATE "C" NOT NULL,
        kind   BIGINT,
        ord    BIGINT,
        id     BIGINT NOT NULL,
        PRIMARY KEY (object, field, attr, value, id)
    );
    CREATE INDEX record_pair_order
        ON record_pair_index(object, field, attr, value, kind, ord, id);
    CREATE INDEX record_pair_order_desc
        ON record_pair_index(object, field, attr, value, kind DESC, ord DESC, id);
    CREATE INDEX record_pair_rows ON record_pair_index(object, id);
    CREATE INDEX record_pair_unordered ON record_pair_index(object, field, attr)
        WHERE kind IS NULL;
    "#;

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

/// The rank of a value's kind in `Value`'s order.
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

/// The pairs the schema keeps for an object type, as (field, attribute).
fn pairs_of(t: &mut Transaction<'_>, object: &str) -> StoreResult<Vec<(String, String)>> {
    let rows = t
        .query(
            "SELECT field, attr FROM record_pairs WHERE object = $1 ORDER BY field, attr",
            &[&object],
        )
        .map_err(db)?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let field: String = get(row, 0)?;
        let attr: String = get(row, 1)?;
        out.push((field, attr));
    }
    Ok(out)
}

/// Write the row of one record for one pair, when its field holds a value the index holds.
fn insert_row(
    t: &mut Transaction<'_>,
    record: &Record,
    field: &str,
    attr: &str,
) -> StoreResult<()> {
    let Ok(field_id) = FieldId::new(field) else {
        return Ok(());
    };
    let Some(key) = value_key(record.get(&field_id)) else {
        return Ok(());
    };
    let (kind, ord): (Option<i64>, Option<i64>) =
        match order_key(&query_plan::order_value(record, attr)) {
            Some((kind, ord)) => (Some(kind), Some(ord)),
            None => (None, None),
        };
    let id: i64 = record.id.0 as i64;
    t.execute(
        "INSERT INTO record_pair_index (object, field, attr, value, kind, ord, id)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
        &[
            &record.object.as_str(),
            &field,
            &attr,
            &key,
            &kind,
            &ord,
            &id,
        ],
    )
    .map_err(db)?;
    Ok(())
}

/// Replace the pair rows of a record that was just written.
pub(crate) fn write_rows(t: &mut Transaction<'_>, record: &Record) -> StoreResult<()> {
    let object = record.object.as_str();
    delete_rows(t, &record.object, record.id.0)?;
    for (field, attr) in pairs_of(t, object)? {
        insert_row(t, record, &field, &attr)?;
    }
    Ok(())
}

/// Remove the pair rows of a record.
pub(crate) fn delete_rows(t: &mut Transaction<'_>, object: &ObjectTypeId, id: u64) -> StoreResult<()> {
    let id: i64 = id as i64;
    t.execute(
        "DELETE FROM record_pair_index WHERE object = $1 AND id = $2",
        &[&object.as_str(), &id],
    )
    .map_err(db)?;
    Ok(())
}

/// Keep exactly the pairs the queries of `schema` read (KRN-2921): drop the rows of a pair no
/// query reads any more, and build the rows of a new pair from the records of its type, in the
/// caller's transaction.
pub(crate) fn sync(t: &mut Transaction<'_>, schema: &TenantSchema) -> StoreResult<()> {
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
    let mut held: BTreeSet<(String, String, String)> = BTreeSet::new();
    for row in &t
        .query("SELECT object, field, attr FROM record_pairs", &[])
        .map_err(db)?
    {
        let object: String = get(row, 0)?;
        let field: String = get(row, 1)?;
        let attr: String = get(row, 2)?;
        held.insert((object, field, attr));
    }
    for (object, field, attr) in held.difference(&wanted) {
        t.execute(
            "DELETE FROM record_pair_index WHERE object = $1 AND field = $2 AND attr = $3",
            &[object, field, attr],
        )
        .map_err(db)?;
        t.execute(
            "DELETE FROM record_pairs WHERE object = $1 AND field = $2 AND attr = $3",
            &[object, field, attr],
        )
        .map_err(db)?;
    }
    for (object, field, attr) in wanted.difference(&held) {
        t.execute(
            "INSERT INTO record_pairs (object, field, attr) VALUES ($1, $2, $3)",
            &[object, field, attr],
        )
        .map_err(db)?;
        let Ok(object_id) = ObjectTypeId::new(object.as_str()) else {
            continue;
        };
        for text in all_of(t, &object_id)? {
            let record: Record = decode(&text, "record")?;
            insert_row(t, &record, field, attr)?;
        }
    }
    Ok(())
}

/// Forget every pair and its rows, then keep those `schema` reads: for a restore, whose records
/// are written afterwards, each with its rows.
pub(crate) fn reset(t: &mut Transaction<'_>, schema: &TenantSchema) -> StoreResult<()> {
    t.execute("DELETE FROM record_pair_index", &[])
        .map_err(db)?;
    t.execute("DELETE FROM record_pairs", &[]).map_err(db)?;
    sync(t, schema)
}

/// Bring the pairs of a prepared schema up to date with its stored tenant schema.
pub(crate) fn sync_stored(t: &mut Transaction<'_>) -> StoreResult<()> {
    let schemas = select_strings(t, "SELECT schema FROM tenant", &[])?;
    for text in schemas {
        let schema: TenantSchema = decode(&text, "tenant schema")?;
        sync(t, &schema)?;
    }
    Ok(())
}

/// The records of a pair page, as stored JSON, in the page's order.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pair_page(
    t: &mut Transaction<'_>,
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
        return scanned_page(t, object, field, value, attr, descending, after, limit);
    };
    let (object_name, field_name) = (object.as_str(), field.as_str());
    let row = t
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM record_pairs
                            WHERE object = $1 AND field = $2 AND attr = $3)
                AND NOT EXISTS (SELECT 1 FROM record_pair_index
                                WHERE object = $1 AND field = $2 AND attr = $3
                                  AND kind IS NULL)",
            &[&object_name, &field_name, &attr],
        )
        .map_err(db)?;
    let kept: bool = get(&row, 0)?;
    if !kept {
        // No query reads the pair, or a record's value of the attribute is not in its order.
        return scanned_page(t, object, field, value, attr, descending, after, limit);
    }
    let mut rows = Vec::new();
    // Where the page starts: after the position's (kind, ord), once the rest of that key's own
    // records (by id) are taken.
    let mut beyond: Option<(i64, i64)> = None;
    if let Some(at) = after {
        match order_key(&at.value) {
            Some((kind, ord)) => {
                let id: i64 = at.id.0 as i64;
                let cap: i64 = i64::from(limit);
                rows = select_strings(
                    t,
                    "SELECT r.data FROM record_pair_index i
                     JOIN records r ON r.object = i.object AND r.id = i.id
                     WHERE i.object = $1 AND i.field = $2 AND i.attr = $3 AND i.value = $4
                       AND i.kind = $5 AND i.ord = $6 AND i.id > $7
                     ORDER BY i.id LIMIT $8",
                    &[
                        &object_name,
                        &field_name,
                        &attr,
                        &key,
                        &kind,
                        &ord,
                        &id,
                        &cap,
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
    let left: i64 = left as i64;
    let more = match (beyond, descending) {
        (None, false) => select_strings(
            t,
            "SELECT r.data FROM record_pair_index i
             JOIN records r ON r.object = i.object AND r.id = i.id
             WHERE i.object = $1 AND i.field = $2 AND i.attr = $3 AND i.value = $4
               AND i.kind IS NOT NULL
             ORDER BY i.kind ASC, i.ord ASC, i.id ASC LIMIT $5",
            &[&object_name, &field_name, &attr, &key, &left],
        )?,
        (None, true) => select_strings(
            t,
            "SELECT r.data FROM record_pair_index i
             JOIN records r ON r.object = i.object AND r.id = i.id
             WHERE i.object = $1 AND i.field = $2 AND i.attr = $3 AND i.value = $4
               AND i.kind IS NOT NULL
             ORDER BY i.kind DESC, i.ord DESC, i.id ASC LIMIT $5",
            &[&object_name, &field_name, &attr, &key, &left],
        )?,
        (Some((kind, ord)), false) => select_strings(
            t,
            "SELECT r.data FROM record_pair_index i
             JOIN records r ON r.object = i.object AND r.id = i.id
             WHERE i.object = $1 AND i.field = $2 AND i.attr = $3 AND i.value = $4
               AND i.kind IS NOT NULL AND (i.kind, i.ord) > ($5::bigint, $6::bigint)
             ORDER BY i.kind ASC, i.ord ASC, i.id ASC LIMIT $7",
            &[&object_name, &field_name, &attr, &key, &kind, &ord, &left],
        )?,
        (Some((kind, ord)), true) => select_strings(
            t,
            "SELECT r.data FROM record_pair_index i
             JOIN records r ON r.object = i.object AND r.id = i.id
             WHERE i.object = $1 AND i.field = $2 AND i.attr = $3 AND i.value = $4
               AND i.kind IS NOT NULL AND (i.kind, i.ord) < ($5::bigint, $6::bigint)
             ORDER BY i.kind DESC, i.ord DESC, i.id ASC LIMIT $7",
            &[&object_name, &field_name, &attr, &key, &kind, &ord, &left],
        )?,
    };
    rows.extend(more);
    Ok(rows)
}

/// Any other page: the kernel's definition over the records of the type.
#[allow(clippy::too_many_arguments)]
fn scanned_page(
    t: &mut Transaction<'_>,
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
    for text in all_of(t, object)? {
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
