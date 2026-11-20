//! The search entries of a tenant file (SRH-150 to SRH-152, D-6585).
//!
//! `search_entries` holds one row per token of each searchable field of each record: `(object,
//! id, field, token)`. The tokens, and which fields of a record are searchable, are the kernel's
//! ([`strata_kernel::search::entries_of`]); this module only stores them and answers a probe with
//! them. SQLite's FTS5 is not used: the entries are a plain table, so the index works whatever the
//! bundled SQLite was built with, and a prefix of a token is a range of the table's key order.
//!
//! The entries are written in the transaction that writes the record ([`write_entries`], called
//! by the commit for every `PutRecord`) and are removed with it: the table references `records`
//! with `ON DELETE CASCADE`, so the commit that deletes a record, or erases it by rewriting it
//! without the field, leaves no entry of the old text. A commit that changes which fields of a type
//! are searchable rebuilds the entries of that type in the same transaction ([`reconcile`]).
//!
//! A file that has records when the table is added is marked (`search_entries_rebuild`), and
//! [`rebuild_if_marked`] writes every entry from the records, in one transaction, when the file
//! is opened; [`rebuild_all`] does the same on demand. The index is a function of the records and
//! the schema alone, so a rebuilt index answers every probe as the one it replaces.

use super::{all_of, corrupt_id, db, decode, select_strings};
use rusqlite::types::Value as Sql;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::collections::BTreeSet;
use strata_kernel::search::{self, Candidate, Probe};
use strata_kernel::{FieldId, ObjectTypeId, Record, RecordId, TenantSchema};
use strata_storage_api::StoreResult;

/// The tenant migration that adds the entries (the last step of `TENANT_MIGRATIONS`; the
/// coordinator numbers it).
pub(crate) const SEARCH_ENTRIES_SQL: &str = "
    CREATE TABLE search_entries (
        object TEXT NOT NULL,
        id     INTEGER NOT NULL,
        field  TEXT NOT NULL,
        token  TEXT NOT NULL,
        PRIMARY KEY (object, id, field, token),
        FOREIGN KEY (object, id) REFERENCES records(object, id) ON DELETE CASCADE
    ) WITHOUT ROWID;
    CREATE INDEX search_entries_token ON search_entries(object, field, token, id);
    CREATE TABLE search_entries_rebuild (pending INTEGER NOT NULL);
    INSERT INTO search_entries_rebuild (pending) VALUES (1);
    ";

/// Replace the entries of a record with those its type declares in `schema`.
pub(crate) fn write_entries(
    tx: &Transaction<'_>,
    schema: &TenantSchema,
    record: &Record,
) -> StoreResult<()> {
    let (object, id) = (record.object.as_str(), record.id.0 as i64);
    tx.execute(
        "DELETE FROM search_entries WHERE object = ?1 AND id = ?2",
        params![object, id],
    )
    .map_err(db)?;
    let Some(def) = schema.objects.get(&record.object) else {
        return Ok(());
    };
    let entries = search::entries_of(def, record);
    if entries.is_empty() {
        return Ok(());
    }
    let mut insert = tx
        .prepare_cached(
            "INSERT INTO search_entries (object, id, field, token) VALUES (?1, ?2, ?3, ?4)",
        )
        .map_err(db)?;
    for (field, tokens) in entries {
        for token in tokens {
            insert
                .execute(params![object, id, field.as_str(), token])
                .map_err(db)?;
        }
    }
    Ok(())
}

/// Write the entries of every record of one object type, after removing the old ones.
fn rebuild_object(
    tx: &Transaction<'_>,
    schema: &TenantSchema,
    object: &ObjectTypeId,
) -> StoreResult<()> {
    tx.execute(
        "DELETE FROM search_entries WHERE object = ?1",
        params![object.as_str()],
    )
    .map_err(db)?;
    let Some(def) = schema.objects.get(object) else {
        return Ok(());
    };
    if search::indexed_fields(def).is_empty() {
        return Ok(());
    }
    for text in all_of(tx, object)? {
        let record: Record = decode(&text, "record")?;
        write_entries(tx, schema, &record)?;
    }
    Ok(())
}

/// Rebuild the entries of every object type whose searchable fields differ between two schemas, in
/// the transaction that stores the new one.
pub(crate) fn reconcile(
    tx: &Transaction<'_>,
    before: &TenantSchema,
    after: &TenantSchema,
) -> StoreResult<()> {
    let objects: BTreeSet<&ObjectTypeId> =
        before.objects.keys().chain(after.objects.keys()).collect();
    for object in objects {
        let held = before.objects.get(object).map(search::indexed_fields);
        let now = after.objects.get(object).map(search::indexed_fields);
        if held != now {
            rebuild_object(tx, after, object)?;
        }
    }
    Ok(())
}

/// Delete every entry and write them again from the records and the schema stored in the file.
pub(crate) fn rebuild_all(tx: &Transaction<'_>) -> StoreResult<()> {
    tx.execute("DELETE FROM search_entries", []).map_err(db)?;
    let stored: Option<String> = tx
        .query_row("SELECT schema FROM tenant LIMIT 1", [], |r| r.get(0))
        .optional()
        .map_err(db)?;
    let Some(text) = stored else {
        return Ok(());
    };
    let schema: TenantSchema = decode(&text, "tenant schema")?;
    for object in schema.objects.keys() {
        rebuild_object(tx, &schema, object)?;
    }
    Ok(())
}

/// Rebuild the entries when the migration that added them marked the file.
pub(crate) fn rebuild_if_marked(tx: &Transaction<'_>) -> StoreResult<()> {
    let marked: bool = tx
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM search_entries_rebuild)",
            [],
            |r| r.get(0),
        )
        .map_err(db)?;
    if !marked {
        return Ok(());
    }
    rebuild_all(tx)?;
    tx.execute("DELETE FROM search_entries_rebuild", [])
        .map_err(db)?;
    Ok(())
}

/// Bind a value and name its parameter.
fn bind(owned: &mut Vec<Sql>, value: Sql) -> String {
    owned.push(value);
    format!("?{}", owned.len())
}

/// The candidates of a probe: records of the object type above `after`, in ascending id, at most
/// `limit`, each with the probed fields in which every query token is the start of a token. The
/// tokens that start with `t` are those from `t` up to `t` followed by the highest code point.
pub(crate) fn candidates(conn: &Connection, probe: &Probe) -> StoreResult<Vec<Candidate>> {
    if probe.fields.is_empty() || probe.tokens.is_empty() || probe.limit == 0 {
        return Ok(Vec::new());
    }
    let mut owned: Vec<Sql> = Vec::new();
    let object = bind(&mut owned, Sql::Text(probe.object.as_str().to_string()));
    let after = bind(
        &mut owned,
        Sql::Integer(probe.after.map_or(-1, |id| id.0 as i64)),
    );
    let fields: Vec<String> = probe
        .fields
        .iter()
        .map(|field| bind(&mut owned, Sql::Text(field.as_str().to_string())))
        .collect();
    let mut sql = format!(
        "SELECT DISTINCT e0.id, e0.field FROM search_entries e0
         WHERE e0.object = {object} AND e0.id > {after} AND e0.field IN ({})",
        fields.join(", ")
    );
    for (n, token) in probe.tokens.iter().enumerate() {
        let low = bind(&mut owned, Sql::Text(token.clone()));
        let high = bind(&mut owned, Sql::Text(format!("{token}\u{10FFFF}")));
        if n == 0 {
            sql.push_str(&format!(" AND e0.token >= {low} AND e0.token < {high}"));
        } else {
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM search_entries e
                   WHERE e.object = e0.object AND e.id = e0.id AND e.field = e0.field
                     AND e.token >= {low} AND e.token < {high})"
            ));
        }
    }
    // Each record has at most one row per probed field, so this many rows hold `limit` records.
    let rows = i64::from(probe.limit) * probe.fields.len() as i64;
    sql.push_str(&format!(" ORDER BY e0.id, e0.field LIMIT {rows}"));
    // The text varies with the probe, so it is prepared and dropped, not cached.
    let mut statement = conn.prepare(&sql).map_err(db)?;
    let found = statement
        .query_map(rusqlite::params_from_iter(owned.iter()), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(db)?;
    let mut out: Vec<Candidate> = Vec::new();
    for row in found {
        let (id, field) = row.map_err(db)?;
        let (id, field) = (
            RecordId(id as u64),
            FieldId::new(field).map_err(corrupt_id)?,
        );
        match out.last_mut() {
            Some(last) if last.id == id => last.fields.push(field),
            _ => {
                if out.len() as u32 >= probe.limit {
                    break;
                }
                out.push(Candidate {
                    id,
                    fields: vec![field],
                });
            }
        }
    }
    Ok(out)
}

/// The entries stored for a record, as `field:token` in order.
pub(crate) fn stored_entries(
    conn: &Connection,
    object: &ObjectTypeId,
    id: RecordId,
) -> StoreResult<Vec<String>> {
    select_strings(
        conn,
        "SELECT field || ':' || token FROM search_entries WHERE object = ?1 AND id = ?2
         ORDER BY field, token",
        &[&object.as_str(), &(id.0 as i64)],
    )
}
