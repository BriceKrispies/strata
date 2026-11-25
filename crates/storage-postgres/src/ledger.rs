//! The tenant ledger in a tenant schema (D-5000 to D-5013), as the SQLite adapter keeps it in a
//! tenant file: account headers and entries written in the transaction of the commit that decided
//! them (LDG-009), and the answers to the ledger's needs (`Need::Ledger`).
//!
//! `ledger_entries` is append-only (LDG-022): a trigger refuses every `DELETE`, and every `UPDATE`
//! except the one erasure makes, which clears `caused_by` and nothing else (LDG-027, LDG-028). The
//! trigger raises an integrity-constraint error, which the adapter reports as `Invalid`, as SQLite's
//! `RAISE(ABORT)` is reported.

use crate::migrations::Migration;
use crate::{db, decode, encode, get};
use postgres::types::ToSql;
use postgres::Transaction;
use strata_kernel::ledger::{account_key, Anonymised};
use strata_kernel::{
    LedgerAccount, LedgerDelta, LedgerEntry, LedgerLimit, LedgerNeed, LedgerOrder, LedgerOwner,
    PrincipalId, StateRevision, TenantState,
};
use strata_storage_api::{StoreError, StoreResult};

/// Tenant step 14: the ledger's accounts and entries.
pub(crate) const MIGRATION: Migration = Migration {
    version: 14,
    name: "tenant ledger accounts and entries",
    sql: r#"
    CREATE TABLE ledger_accounts (
        account_id    TEXT COLLATE "C" PRIMARY KEY,
        owner_kind    BIGINT NOT NULL CHECK (owner_kind IN (0, 1, 2)),
        owner_name    TEXT COLLATE "C",
        unit          TEXT COLLATE "C" NOT NULL,
        scope         TEXT COLLATE "C" NOT NULL,
        balance       BIGINT NOT NULL,
        last_sequence BIGINT NOT NULL,
        revision      BIGINT NOT NULL,
        data          TEXT NOT NULL
    );
    CREATE INDEX ledger_accounts_rank ON ledger_accounts(unit, scope, balance);
    CREATE INDEX ledger_accounts_owner ON ledger_accounts(owner_kind, owner_name);
    CREATE TABLE ledger_entries (
        account_id      TEXT COLLATE "C" NOT NULL REFERENCES ledger_accounts(account_id),
        sequence        BIGINT NOT NULL,
        idempotency_key TEXT COLLATE "C" NOT NULL,
        caused_by       TEXT COLLATE "C",
        revision        BIGINT NOT NULL,
        data            TEXT NOT NULL,
        PRIMARY KEY (account_id, sequence),
        UNIQUE (account_id, idempotency_key)
    );
    CREATE INDEX ledger_entries_caused_by ON ledger_entries(caused_by);
    CREATE INDEX ledger_entries_revision ON ledger_entries(revision);
    CREATE FUNCTION ledger_entries_append_only() RETURNS trigger LANGUAGE plpgsql AS $$
    BEGIN
        IF TG_OP = 'DELETE' THEN
            RAISE EXCEPTION 'ledger entries are append-only'
                USING ERRCODE = 'integrity_constraint_violation';
        END IF;
        IF NOT (NEW.caused_by IS NULL
                AND NEW.account_id IS NOT DISTINCT FROM OLD.account_id
                AND NEW.sequence IS NOT DISTINCT FROM OLD.sequence
                AND NEW.idempotency_key IS NOT DISTINCT FROM OLD.idempotency_key
                AND NEW.revision IS NOT DISTINCT FROM OLD.revision
                AND NEW.data IS NOT DISTINCT FROM OLD.data) THEN
            RAISE EXCEPTION 'ledger entries are append-only'
                USING ERRCODE = 'integrity_constraint_violation';
        END IF;
        RETURN NEW;
    END
    $$;
    CREATE TRIGGER ledger_entries_append_only_delete BEFORE DELETE ON ledger_entries
        FOR EACH ROW EXECUTE FUNCTION ledger_entries_append_only();
    CREATE TRIGGER ledger_entries_append_only_update BEFORE UPDATE ON ledger_entries
        FOR EACH ROW EXECUTE FUNCTION ledger_entries_append_only();
    "#,
};

/// Tenant step 19: the limits of single accounts move out of the tenant schema into
/// `ledger_limits` (WG-5172). The rows are made from the stored schema (canonical JSON, read as
/// `jsonb`), and the schema is rewritten without them; a limit of a unit (no owner) stays in it.
pub(crate) const ACCOUNT_LIMITS_MIGRATION: Migration = Migration {
    version: 19,
    name: "tenant ledger account limits out of the schema",
    sql: r#"
    CREATE TABLE ledger_limits (
        unit       TEXT COLLATE "C" NOT NULL,
        owner_kind BIGINT NOT NULL CHECK (owner_kind IN (1, 2)),
        owner_name TEXT COLLATE "C" NOT NULL,
        scope      TEXT COLLATE "C" NOT NULL,
        revision   BIGINT NOT NULL,
        data       TEXT NOT NULL,
        PRIMARY KEY (unit, owner_kind, owner_name, scope)
    );
    CREATE INDEX ledger_limits_owner ON ledger_limits(owner_kind, owner_name);
    INSERT INTO ledger_limits (unit, owner_kind, owner_name, scope, revision, data)
    SELECT l.value ->> 'unit',
           CASE WHEN (l.value -> 'owner') ? 'principal' THEN 1 ELSE 2 END,
           COALESCE(l.value #>> '{owner,principal,principal}',
                    l.value #>> '{owner,pool,pool}'),
           COALESCE(l.value ->> 'scope', ''),
           0,
           l.value::text
      FROM tenant,
           jsonb_array_elements(
               CASE WHEN jsonb_typeof((tenant.schema::jsonb) #> '{ledger,limits}') = 'array'
                    THEN (tenant.schema::jsonb) #> '{ledger,limits}'
                    ELSE '[]'::jsonb END) AS l(value)
     WHERE l.value ? 'owner'
    ON CONFLICT (unit, owner_kind, owner_name, scope)
        DO UPDATE SET revision = EXCLUDED.revision, data = EXCLUDED.data;
    UPDATE tenant
       SET schema = jsonb_set(schema::jsonb, '{ledger,limits}',
             COALESCE((SELECT jsonb_agg(l.value)
                         FROM jsonb_array_elements((tenant.schema::jsonb) #> '{ledger,limits}') AS l(value)
                        WHERE NOT (l.value ? 'owner')), '[]'::jsonb))::text
     WHERE jsonb_typeof((schema::jsonb) #> '{ledger,limits}') = 'array';
    "#,
};

/// Tenant step 20: the accounts indexed by the start of their oldest bucket of credit (WG-5169).
pub(crate) const EXPIRY_MIGRATION: Migration = Migration {
    version: 20,
    name: "tenant ledger index of the oldest credit bucket",
    sql: "
    ALTER TABLE ledger_accounts ADD COLUMN oldest_bucket BIGINT;
    CREATE INDEX ledger_accounts_expiry ON ledger_accounts(unit, oldest_bucket, account_id)
        WHERE oldest_bucket IS NOT NULL;
    ",
};

/// The owner of a header as the columns that order and select on it: kind 0 for an erased
/// owner, 1 for a principal, 2 for a pool (LDG-1543).
fn owner_columns(owner: &Option<LedgerOwner>) -> (i64, Option<String>) {
    match owner {
        None => (0, None),
        Some(LedgerOwner::Principal { principal }) => (1, Some(principal.as_str().to_string())),
        Some(LedgerOwner::Pool { pool }) => (2, Some(pool.as_str().to_string())),
    }
}

fn put_account(state: &mut TenantState, data: &str) -> StoreResult<()> {
    let account: LedgerAccount = decode(data, "ledger account")?;
    state
        .ledger
        .get_or_insert_with(strata_kernel::ledger::empty_state)
        .accounts
        .insert(account_key(&account.account), account);
    Ok(())
}

fn put_entry(state: &mut TenantState, data: &str, caused_by: Option<String>) -> StoreResult<()> {
    let mut entry: LedgerEntry = decode(data, "ledger entry")?;
    entry.caused_by = match caused_by {
        Some(id) => Some(
            PrincipalId::new(id).map_err(|e| StoreError::Corrupt(format!("ledger entry: {e}")))?,
        ),
        None => None,
    };
    let entries = state
        .ledger
        .get_or_insert_with(strata_kernel::ledger::empty_state)
        .entries
        .entry(account_key(&entry.account))
        .or_default();
    if !entries.iter().any(|e| e.sequence == entry.sequence) {
        entries.push(entry);
        entries.sort_by_key(|e| e.sequence);
    }
    Ok(())
}

fn put_limit(state: &mut TenantState, data: &str) -> StoreResult<()> {
    let limit: LedgerLimit = decode(data, "ledger limit")?;
    let limits = state
        .ledger
        .get_or_insert_with(strata_kernel::ledger::empty_state)
        .limits
        .get_or_insert_with(Vec::new);
    limits.push(limit);
    limits.sort();
    Ok(())
}

/// The key columns of the limit of one account.
fn limit_columns(owner: &LedgerOwner, scope: &str) -> (i64, String, String) {
    let (kind, name) = owner_columns(&Some(owner.clone()));
    (kind, name.unwrap_or_default(), scope.to_string())
}

fn select_texts(
    t: &mut Transaction<'_>,
    sql: &str,
    args: &[&(dyn ToSql + Sync)],
) -> StoreResult<Vec<String>> {
    crate::select_strings(t, sql, args)
}

fn select_entries(
    t: &mut Transaction<'_>,
    sql: &str,
    args: &[&(dyn ToSql + Sync)],
) -> StoreResult<Vec<(String, Option<String>)>> {
    let rows = t.query(sql, args).map_err(db)?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let data: String = get(row, 0)?;
        let caused_by: Option<String> = get(row, 1)?;
        out.push((data, caused_by));
    }
    Ok(out)
}

/// Load what one ledger need names into `state`.
pub(crate) fn load(
    t: &mut Transaction<'_>,
    state: &mut TenantState,
    need: &LedgerNeed,
) -> StoreResult<()> {
    match need {
        LedgerNeed::Account { account } => {
            let key = account_key(account);
            for data in select_texts(
                t,
                "SELECT data FROM ledger_accounts WHERE account_id = $1",
                &[&key],
            )? {
                put_account(state, &data)?;
            }
        }
        LedgerNeed::EntryByKey { account, key } => {
            let account = account_key(account);
            for (data, caused_by) in select_entries(
                t,
                "SELECT data, caused_by FROM ledger_entries WHERE account_id = $1 AND idempotency_key = $2",
                &[&account, key],
            )? {
                put_entry(state, &data, caused_by)?;
            }
        }
        LedgerNeed::Entries {
            account,
            after,
            limit,
        } => {
            let account = account_key(account);
            let after: i64 = after.map_or(0_i64, |a| a as i64);
            let limit: i64 = *limit as i64;
            for (data, caused_by) in select_entries(
                t,
                "SELECT data, caused_by FROM ledger_entries WHERE account_id = $1 AND sequence > $2
                 ORDER BY sequence LIMIT $3",
                &[&account, &after, &limit],
            )? {
                put_entry(state, &data, caused_by)?;
            }
        }
        LedgerNeed::Ranked {
            unit,
            scope,
            order,
            offset,
            limit,
        } => {
            // Equal balances in ascending order of the owner, then of the account id (LDG-1543).
            let sql = match order {
                LedgerOrder::GreatestFirst => {
                    "SELECT data FROM ledger_accounts WHERE unit = $1 AND scope = $2
                     ORDER BY balance DESC, owner_kind, owner_name NULLS FIRST, account_id LIMIT $3"
                }
                LedgerOrder::LeastFirst => {
                    "SELECT data FROM ledger_accounts WHERE unit = $1 AND scope = $2
                     ORDER BY balance ASC, owner_kind, owner_name NULLS FIRST, account_id LIMIT $3"
                }
            };
            let walk: i64 = i64::from(*offset) + i64::from(*limit);
            for data in select_texts(t, sql, &[&unit.as_str(), &scope.as_str(), &walk])? {
                put_account(state, &data)?;
            }
        }
        LedgerNeed::UnitUsed { unit } => {
            for data in select_texts(
                t,
                "SELECT data FROM ledger_accounts WHERE unit = $1 LIMIT 1",
                &[&unit.as_str()],
            )? {
                put_account(state, &data)?;
            }
        }
        LedgerNeed::PoolUsed { pool } => {
            for data in select_texts(
                t,
                "SELECT data FROM ledger_accounts WHERE owner_kind = 2 AND owner_name = $1 LIMIT 1",
                &[&pool.as_str()],
            )? {
                put_account(state, &data)?;
            }
        }
        LedgerNeed::Limit { owner, unit, scope } => {
            let (kind, name, scope) = limit_columns(owner, scope.as_str());
            for data in select_texts(
                t,
                "SELECT data FROM ledger_limits
                 WHERE unit = $1 AND owner_kind = $2 AND owner_name = $3 AND scope = $4",
                &[&unit.as_str(), &kind, &name, &scope],
            )? {
                put_limit(state, &data)?;
            }
        }
        LedgerNeed::Limits => {
            for data in select_texts(
                t,
                "SELECT data FROM ledger_limits ORDER BY unit, owner_kind, owner_name, scope",
                &[],
            )? {
                put_limit(state, &data)?;
            }
        }
        LedgerNeed::Expiring {
            unit,
            cutoff,
            limit,
        } => {
            let cutoff: i64 = *cutoff;
            let limit: i64 = i64::from(*limit);
            for data in select_texts(
                t,
                "SELECT data FROM ledger_accounts
                 WHERE unit = $1 AND oldest_bucket IS NOT NULL AND oldest_bucket <= $2
                 ORDER BY oldest_bucket, account_id LIMIT $3",
                &[&unit.as_str(), &cutoff, &limit],
            )? {
                put_account(state, &data)?;
            }
        }
    }
    Ok(())
}

/// Load every account, entry and limit of the tenant into `state`, for a complete snapshot.
pub(crate) fn load_all(t: &mut Transaction<'_>, state: &mut TenantState) -> StoreResult<()> {
    for data in select_texts(
        t,
        "SELECT data FROM ledger_accounts ORDER BY account_id",
        &[],
    )? {
        put_account(state, &data)?;
    }
    for (data, caused_by) in select_entries(
        t,
        "SELECT data, caused_by FROM ledger_entries ORDER BY account_id, sequence",
        &[],
    )? {
        put_entry(state, &data, caused_by)?;
    }
    for data in select_texts(
        t,
        "SELECT data FROM ledger_limits ORDER BY unit, owner_kind, owner_name, scope",
        &[],
    )? {
        put_limit(state, &data)?;
    }
    Ok(())
}

/// Write one ledger change of a commit, in its transaction. A header replaces the stored one; an
/// entry is inserted, and one that repeats a number or a key of its account fails the commit,
/// which then changes nothing.
pub(crate) fn apply(
    t: &mut Transaction<'_>,
    op: &LedgerDelta,
    revision: StateRevision,
) -> StoreResult<()> {
    let revision: i64 = revision.0 as i64;
    match op {
        LedgerDelta::PutAccount(account) => {
            let (kind, name) = owner_columns(&account.owner);
            let key = account_key(&account.account);
            let balance: i64 = account.balance.0;
            let last: i64 = account.last_sequence as i64;
            let oldest: Option<i64> =
                strata_kernel::ledger_expiry::oldest_bucket(account).map(|start| start.0);
            t.execute(
                "INSERT INTO ledger_accounts
                     (account_id, owner_kind, owner_name, unit, scope, balance, last_sequence, revision, data,
                      oldest_bucket)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                 ON CONFLICT (account_id) DO UPDATE SET
                     owner_kind = EXCLUDED.owner_kind, owner_name = EXCLUDED.owner_name,
                     balance = EXCLUDED.balance, last_sequence = EXCLUDED.last_sequence,
                     revision = EXCLUDED.revision, data = EXCLUDED.data,
                     oldest_bucket = EXCLUDED.oldest_bucket",
                &[
                    &key,
                    &kind,
                    &name,
                    &account.unit.as_str(),
                    &account.scope.as_str(),
                    &balance,
                    &last,
                    &revision,
                    &encode(account),
                    &oldest,
                ],
            )
            .map_err(db)?;
        }
        LedgerDelta::PutLimit(limit) => {
            let owner = limit.owner.as_ref().ok_or_else(|| {
                StoreError::Invalid("a limit of a unit is not an account limit".into())
            })?;
            let (kind, name, scope) =
                limit_columns(owner, limit.scope.as_ref().map_or("", |s| s.as_str()));
            t.execute(
                "INSERT INTO ledger_limits (unit, owner_kind, owner_name, scope, revision, data)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (unit, owner_kind, owner_name, scope) DO UPDATE SET
                     revision = EXCLUDED.revision, data = EXCLUDED.data",
                &[
                    &limit.unit.as_str(),
                    &kind,
                    &name,
                    &scope,
                    &revision,
                    &encode(limit),
                ],
            )
            .map_err(db)?;
        }
        LedgerDelta::RemoveLimit { owner, unit, scope } => {
            let (kind, name, scope) = limit_columns(owner, scope.as_str());
            t.execute(
                "DELETE FROM ledger_limits
                 WHERE unit = $1 AND owner_kind = $2 AND owner_name = $3 AND scope = $4",
                &[&unit.as_str(), &kind, &name, &scope],
            )
            .map_err(db)?;
        }
        LedgerDelta::AppendEntry(entry) => {
            // The number continues the account's: the header of the same commit says so.
            let key = account_key(&entry.account);
            let last: Option<i64> = match t
                .query_opt(
                    "SELECT last_sequence FROM ledger_accounts WHERE account_id = $1",
                    &[&key],
                )
                .map_err(db)?
            {
                Some(row) => Some(get(&row, 0)?),
                None => None,
            };
            if last != Some(entry.sequence as i64) {
                return Err(StoreError::Invalid(format!(
                    "ledger entry {} does not continue its account",
                    entry.sequence
                )));
            }
            let stored = LedgerEntry {
                caused_by: None,
                ..entry.clone()
            };
            let sequence: i64 = entry.sequence as i64;
            let caused_by: Option<String> =
                entry.caused_by.as_ref().map(|p| p.as_str().to_string());
            t.execute(
                "INSERT INTO ledger_entries (account_id, sequence, idempotency_key, caused_by, revision, data)
                 VALUES ($1, $2, $3, $4, $5, $6)",
                &[
                    &key,
                    &sequence,
                    &entry.idempotency_key,
                    &caused_by,
                    &revision,
                    &encode(&stored),
                ],
            )
            .map_err(db)?;
        }
    }
    Ok(())
}

/// Refuse to restore a snapshot older than an entry: entries are never removed (LDG-022,
/// LDG-2911), and a snapshot whose journal head is older than the last anchor of the audit log
/// (LDG-2924). The audit log is not rolled back by a restore, so the anchors stand.
pub(crate) fn check_restorable(
    t: &mut Transaction<'_>,
    snapshot: &strata_kernel::TenantState,
) -> StoreResult<()> {
    let revision: i64 = snapshot.revision.0 as i64;
    let row = t
        .query_one(
            "SELECT COUNT(*) FROM ledger_entries WHERE revision > $1",
            &[&revision],
        )
        .map_err(db)?;
    let newer: i64 = get(&row, 0)?;
    if newer > 0 {
        return Err(StoreError::Invalid(format!(
            "{}: the ledger holds entries posted after the snapshot, and entries are never removed",
            strata_kernel::ledger_chain::RESTORE_DROPS_ENTRIES
        )));
    }
    let mut heads = std::collections::BTreeMap::new();
    for data in select_texts(t, "SELECT data FROM ledger_accounts", &[])? {
        let account: LedgerAccount = decode(&data, "ledger account")?;
        heads.insert(
            account_key(&account.account),
            (account.last_sequence, account.head_hash),
        );
    }
    let rows = t
        .query(
            "SELECT at, kind, event, correlation, data FROM audit_log WHERE event LIKE 'ledger.%' ORDER BY seq",
            &[],
        )
        .map_err(db)?;
    let mut lines = Vec::new();
    for row in &rows {
        lines.push(strata_storage_api::AuditEntry {
            at: get(row, 0)?,
            kind: get(row, 1)?,
            event: get(row, 2)?,
            correlation: get(row, 3)?,
            data: get(row, 4)?,
        });
    }
    strata_storage_api::ledger_audit::check_restore(&heads, &lines, snapshot.ledger.as_ref())
}

/// Clear a principal from the ledger of a tenant schema (LDG-027, LDG-028): the owner of its
/// accounts (columns and header) and `caused_by` of its entries. Amounts, keys, reasons, numbers,
/// times and balances stay.
pub(crate) fn anonymise(
    t: &mut Transaction<'_>,
    principal: &PrincipalId,
) -> StoreResult<Anonymised> {
    let owned = select_texts(
        t,
        "SELECT data FROM ledger_accounts WHERE owner_kind = 1 AND owner_name = $1",
        &[&principal.as_str()],
    )?;
    let mut done = Anonymised::default();
    for data in owned {
        let mut account: LedgerAccount = decode(&data, "ledger account")?;
        account.owner = None;
        t.execute(
            "UPDATE ledger_accounts SET owner_kind = 0, owner_name = NULL, data = $1 WHERE account_id = $2",
            &[&encode(&account), &account_key(&account.account)],
        )
        .map_err(db)?;
        done.accounts += 1;
    }
    done.entries = t
        .execute(
            "UPDATE ledger_entries SET caused_by = NULL WHERE caused_by = $1",
            &[&principal.as_str()],
        )
        .map_err(db)?;
    // The limits of the accounts the principal owned name it too.
    t.execute(
        "DELETE FROM ledger_limits WHERE owner_kind = 1 AND owner_name = $1",
        &[&principal.as_str()],
    )
    .map_err(db)?;
    Ok(done)
}

/// The ledger state a load built is none when it holds nothing.
pub(crate) fn tidy(state: &mut TenantState) {
    strata_kernel::ledger::tidy_state(&mut state.ledger);
}
