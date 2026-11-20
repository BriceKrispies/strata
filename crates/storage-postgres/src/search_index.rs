//! The search entries of a tenant schema (SRH-150 to SRH-152, D-6585), as the SQLite adapter's
//! `search_index.rs` keeps them: one row per token of each searchable field of each record,
//! written in the transaction that writes the record and removed with it (`ON DELETE CASCADE`).
//! The tokens and which fields are searchable are the kernel's
//! ([`strata_kernel::search::entries_of`]). A prefix of a token is a range of the `COLLATE "C"`
//! key order, from the token up to the token followed by the highest code point.

use crate::{all_of, corrupt_id, db, decode, get, select_strings};
use postgres::types::ToSql;
use postgres::Transaction;
use std::collections::BTreeSet;
use strata_kernel::search::{self, Candidate, Probe};
use strata_kernel::{FieldId, ObjectTypeId, Record, RecordId, TenantSchema};
use strata_storage_api::StoreResult;

/// Tenant step 18: the entries. A schema that had records before the step is marked, and
/// [`rebuild_if_marked`] writes every entry when the schema is next prepared.
pub(crate) const SEARCH_ENTRIES_SQL: &str = r#"
    CREATE TABLE search_entries (
        object TEXT COLLATE "C" NOT NULL,
        id     BIGINT NOT NULL,
        field  TEXT COLLATE "C" NOT NULL,
        token  TEXT COLLATE "C" NOT NULL,
        PRIMARY KEY (object, id, field, token),
        FOREIGN KEY (object, id) REFERENCES records(object, id) ON DELETE CASCADE
    );
    CREATE INDEX search_entries_token ON search_entries(object, field, token, id);
    CREATE TABLE search_entries_rebuild (pending BIGINT NOT NULL);
    INSERT INTO search_entries_rebuild (pending) VALUES (1);
    "#;

/// Replace the entries of a record with those its type declares in `schema`.
pub(crate) fn write_entries(
    t: &mut Transaction<'_>,
    schema: &TenantSchema,
    record: &Record,
) -> StoreResult<()> {
    let (object, id) = (record.object.as_str(), record.id.0 as i64);
    t.execute(
        "DELETE FROM search_entries WHERE object = $1 AND id = $2",
        &[&object, &id],
    )
    .map_err(db)?;
    let Some(def) = schema.objects.get(&record.object) else {
        return Ok(());
    };
    let entries = search::entries_of(def, record);
    if entries.is_empty() {
        return Ok(());
    }
    for (field, tokens) in entries {
        for token in tokens {
            t.execute(
                "INSERT INTO search_entries (object, id, field, token) VALUES ($1, $2, $3, $4)",
                &[&object, &id, &field.as_str(), &token],
            )
            .map_err(db)?;
        }
    }
    Ok(())
}

/// Write the entries of every record of one object type, after removing the old ones.
fn rebuild_object(
    t: &mut Transaction<'_>,
    schema: &TenantSchema,
    object: &ObjectTypeId,
) -> StoreResult<()> {
    t.execute(
        "DELETE FROM search_entries WHERE object = $1",
        &[&object.as_str()],
    )
    .map_err(db)?;
    let Some(def) = schema.objects.get(object) else {
        return Ok(());
    };
    if search::indexed_fields(def).is_empty() {
        return Ok(());
    }
    for text in all_of(t, object)? {
        let record: Record = decode(&text, "record")?;
        write_entries(t, schema, &record)?;
    }
    Ok(())
}

/// Rebuild the entries of every object type whose searchable fields differ between two schemas, in
/// the transaction that stores the new one.
pub(crate) fn reconcile(
    t: &mut Transaction<'_>,
    before: &TenantSchema,
    after: &TenantSchema,
) -> StoreResult<()> {
    let objects: BTreeSet<&ObjectTypeId> =
        before.objects.keys().chain(after.objects.keys()).collect();
    for object in objects {
        let held = before.objects.get(object).map(search::indexed_fields);
        let now = after.objects.get(object).map(search::indexed_fields);
        if held != now {
            rebuild_object(t, after, object)?;
        }
    }
    Ok(())
}

/// Delete every entry and write them again from the records and the stored schema.
pub(crate) fn rebuild_all(t: &mut Transaction<'_>) -> StoreResult<()> {
    t.execute("DELETE FROM search_entries", &[]).map_err(db)?;
    let stored = select_strings(t, "SELECT schema FROM tenant LIMIT 1", &[])?;
    let Some(text) = stored.into_iter().next() else {
        return Ok(());
    };
    let schema: TenantSchema = decode(&text, "tenant schema")?;
    for object in schema.objects.keys() {
        rebuild_object(t, &schema, object)?;
    }
    Ok(())
}

/// Rebuild the entries when the step that added them marked the schema.
pub(crate) fn rebuild_if_marked(t: &mut Transaction<'_>) -> StoreResult<()> {
    let row = t
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM search_entries_rebuild)",
            &[],
        )
        .map_err(db)?;
    let marked: bool = get(&row, 0)?;
    if !marked {
        return Ok(());
    }
    rebuild_all(t)?;
    t.execute("DELETE FROM search_entries_rebuild", &[])
        .map_err(db)?;
    Ok(())
}

/// Bind a value and name its parameter.
fn bind(owned: &mut Vec<Box<dyn ToSql + Sync>>, value: Box<dyn ToSql + Sync>) -> String {
    owned.push(value);
    format!("${}", owned.len())
}

/// The candidates of a probe: records of the object type above `after`, in ascending id, at most
/// `limit`, each with the probed fields in which every query token is the start of a token.
pub(crate) fn candidates(t: &mut Transaction<'_>, probe: &Probe) -> StoreResult<Vec<Candidate>> {
    if probe.fields.is_empty() || probe.tokens.is_empty() || probe.limit == 0 {
        return Ok(Vec::new());
    }
    let mut owned: Vec<Box<dyn ToSql + Sync>> = Vec::new();
    let object = bind(&mut owned, Box::new(probe.object.as_str().to_string()));
    let after_id: i64 = probe.after.map_or(-1, |id| id.0 as i64);
    let after = bind(&mut owned, Box::new(after_id));
    let fields: Vec<String> = probe
        .fields
        .iter()
        .map(|field| bind(&mut owned, Box::new(field.as_str().to_string())))
        .collect();
    let mut sql = format!(
        "SELECT DISTINCT e0.id, e0.field FROM search_entries e0
         WHERE e0.object = {object} AND e0.id > {after} AND e0.field IN ({})",
        fields.join(", ")
    );
    for (n, token) in probe.tokens.iter().enumerate() {
        let low = bind(&mut owned, Box::new(token.clone()));
        let high = bind(&mut owned, Box::new(format!("{token}\u{10FFFF}")));
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
    let args: Vec<&(dyn ToSql + Sync)> = owned.iter().map(|value| value.as_ref()).collect();
    let found = t.query(sql.as_str(), &args).map_err(db)?;
    let mut out: Vec<Candidate> = Vec::new();
    for row in &found {
        let id: i64 = get(row, 0)?;
        let field: String = get(row, 1)?;
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
    t: &mut Transaction<'_>,
    object: &ObjectTypeId,
    id: RecordId,
) -> StoreResult<Vec<String>> {
    let id: i64 = id.0 as i64;
    select_strings(
        t,
        "SELECT field || ':' || token FROM search_entries WHERE object = $1 AND id = $2
         ORDER BY field, token",
        &[&object.as_str(), &id],
    )
}
