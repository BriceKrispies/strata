//! The tenant ledger in a tenant file (D-5000 to D-5013): account headers and entries, written in
//! the transaction of the commit that decided them (LDG-009), and the answers to the ledger's
//! needs (`Need::Ledger`).
//!
//! Tables (tenant migration step [`MIGRATION`]):
//!
//! - `ledger_accounts`: one row per account, its header as canonical JSON in `data`, with the
//!   columns a load or a ranked listing selects on (unit, scope, balance, owner). Written by
//!   `PutAccount` only, which only the commit of a posting makes (LDG-021).
//! - `ledger_entries`: one row per entry, keyed by account and number, unique by account and
//!   idempotency key. Append-only (LDG-022): a trigger refuses every `DELETE`, and every `UPDATE`
//!   except the one erasure makes, which clears `caused_by` and nothing else (LDG-027,
//!   LDG-028). `caused_by` lives in its column only, so the JSON in `data` never names a person
//!   and is never rewritten.

use super::{db, decode, encode};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use strata_kernel::ledger::{account_key, Anonymised};
use strata_kernel::{
    LedgerAccount, LedgerDelta, LedgerEntry, LedgerNeed, LedgerOrder, LedgerOwner, PrincipalId,
    StateRevision, TenantState,
};
use strata_runtime::sqlite::Migration;
use strata_storage_api::{StoreError, StoreResult};

/// The tenant migration step that lays out the ledger. Its number is the next free one when it
/// was written; steps are only ever appended.
pub(crate) const MIGRATION: Migration = Migration {
    version: 14,
    name: "tenant ledger accounts and entries",
    sql: "
    CREATE TABLE ledger_accounts (
        account_id    TEXT PRIMARY KEY,
        owner_kind    INTEGER NOT NULL CHECK (owner_kind IN (0, 1, 2)),
        owner_name    TEXT,
        unit          TEXT NOT NULL,
        scope         TEXT NOT NULL,
        balance       INTEGER NOT NULL,
        last_sequence INTEGER NOT NULL,
        revision      INTEGER NOT NULL,
        data          TEXT NOT NULL
    );
    CREATE INDEX ledger_accounts_rank ON ledger_accounts(unit, scope, balance);
    CREATE INDEX ledger_accounts_owner ON ledger_accounts(owner_kind, owner_name);
    CREATE TABLE ledger_entries (
        account_id      TEXT NOT NULL REFERENCES ledger_accounts(account_id),
        sequence        INTEGER NOT NULL,
        idempotency_key TEXT NOT NULL,
        caused_by       TEXT,
        revision        INTEGER NOT NULL,
        data            TEXT NOT NULL,
        PRIMARY KEY (account_id, sequence),
        UNIQUE (account_id, idempotency_key)
    );
    CREATE INDEX ledger_entries_caused_by ON ledger_entries(caused_by);
    CREATE INDEX ledger_entries_revision ON ledger_entries(revision);
    CREATE TRIGGER ledger_entries_append_only_delete BEFORE DELETE ON ledger_entries
    BEGIN
        SELECT RAISE(ABORT, 'ledger entries are append-only');
    END;
    CREATE TRIGGER ledger_entries_append_only_update BEFORE UPDATE ON ledger_entries
    WHEN NOT (NEW.caused_by IS NULL
              AND NEW.account_id IS OLD.account_id
              AND NEW.sequence IS OLD.sequence
              AND NEW.idempotency_key IS OLD.idempotency_key
              AND NEW.revision IS OLD.revision
              AND NEW.data IS OLD.data)
    BEGIN
        SELECT RAISE(ABORT, 'ledger entries are append-only');
    END;
    ",
};

/// The owner of a header as the columns that order and select on it: kind 0 for an erased
/// owner, 1 for a principal, 2 for a pool, so that `ORDER BY owner_kind, owner_name` is the
/// order of `Option<LedgerOwner>` (LDG-1543).
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

fn select_accounts(
    tx: &Connection,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> StoreResult<Vec<String>> {
    let mut statement = tx.prepare_cached(sql).map_err(db)?;
    let rows = statement
        .query_map(args, |row| row.get::<_, String>(0))
        .map_err(db)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db)
}

fn select_entries(
    tx: &Connection,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> StoreResult<Vec<(String, Option<String>)>> {
    let mut statement = tx.prepare_cached(sql).map_err(db)?;
    let rows = statement
        .query_map(args, |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(db)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db)
}

/// Load what one ledger need names into `state`.
pub(crate) fn load(tx: &Connection, state: &mut TenantState, need: &LedgerNeed) -> StoreResult<()> {
    match need {
        LedgerNeed::Account { account } => {
            for data in select_accounts(
                tx,
                "SELECT data FROM ledger_accounts WHERE account_id = ?1",
                &[&account_key(account)],
            )? {
                put_account(state, &data)?;
            }
        }
        LedgerNeed::EntryByKey { account, key } => {
            for (data, caused_by) in select_entries(
                tx,
                "SELECT data, caused_by FROM ledger_entries WHERE account_id = ?1 AND idempotency_key = ?2",
                &[&account_key(account), key],
            )? {
                put_entry(state, &data, caused_by)?;
            }
        }
        LedgerNeed::Entries {
            account,
            after,
            limit,
        } => {
            for (data, caused_by) in select_entries(
                tx,
                "SELECT data, caused_by FROM ledger_entries WHERE account_id = ?1 AND sequence > ?2
                 ORDER BY sequence LIMIT ?3",
                &[
                    &account_key(account),
                    &after.map_or(0_i64, |a| a as i64),
                    &(*limit as i64),
                ],
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
            // Walked through the index on unit, scope and balance; equal balances in ascending
            // order of the owner, then of the account id (LDG-1543).
            let sql = match order {
                LedgerOrder::GreatestFirst => {
                    "SELECT data FROM ledger_accounts WHERE unit = ?1 AND scope = ?2
                     ORDER BY balance DESC, owner_kind, owner_name, account_id LIMIT ?3"
                }
                LedgerOrder::LeastFirst => {
                    "SELECT data FROM ledger_accounts WHERE unit = ?1 AND scope = ?2
                     ORDER BY balance ASC, owner_kind, owner_name, account_id LIMIT ?3"
                }
            };
            let walk = i64::from(*offset) + i64::from(*limit);
            for data in select_accounts(tx, sql, &[&unit.as_str(), &scope.as_str(), &walk])? {
                put_account(state, &data)?;
            }
        }
        LedgerNeed::UnitUsed { unit } => {
            for data in select_accounts(
                tx,
                "SELECT data FROM ledger_accounts WHERE unit = ?1 LIMIT 1",
                &[&unit.as_str()],
            )? {
                put_account(state, &data)?;
            }
        }
        LedgerNeed::PoolUsed { pool } => {
            for data in select_accounts(
                tx,
                "SELECT data FROM ledger_accounts WHERE owner_kind = 2 AND owner_name = ?1 LIMIT 1",
                &[&pool.as_str()],
            )? {
                put_account(state, &data)?;
            }
        }
    }
    Ok(())
}

/// Load every account and entry of the tenant into `state`, for a complete snapshot.
pub(crate) fn load_all(conn: &Connection, state: &mut TenantState) -> StoreResult<()> {
    for data in select_accounts(
        conn,
        "SELECT data FROM ledger_accounts ORDER BY account_id",
        &[],
    )? {
        put_account(state, &data)?;
    }
    for (data, caused_by) in select_entries(
        conn,
        "SELECT data, caused_by FROM ledger_entries ORDER BY account_id, sequence",
        &[],
    )? {
        put_entry(state, &data, caused_by)?;
    }
    Ok(())
}

/// Write one ledger change of a commit, in its transaction. A header replaces the stored one; an
/// entry is inserted, and one that repeats a number or a key of its account fails the commit,
/// which then changes nothing.
pub(crate) fn apply(
    tx: &Transaction<'_>,
    op: &LedgerDelta,
    revision: StateRevision,
) -> StoreResult<()> {
    match op {
        LedgerDelta::PutAccount(account) => {
            let (kind, name) = owner_columns(&account.owner);
            tx.execute(
                "INSERT INTO ledger_accounts
                     (account_id, owner_kind, owner_name, unit, scope, balance, last_sequence, revision, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(account_id) DO UPDATE SET
                     owner_kind = excluded.owner_kind, owner_name = excluded.owner_name,
                     balance = excluded.balance, last_sequence = excluded.last_sequence,
                     revision = excluded.revision, data = excluded.data",
                params![
                    account_key(&account.account),
                    kind,
                    name,
                    account.unit.as_str(),
                    account.scope.as_str(),
                    account.balance.0,
                    account.last_sequence as i64,
                    revision.0 as i64,
                    encode(account)
                ],
            )
            .map_err(db)?;
        }
        LedgerDelta::AppendEntry(entry) => {
            // The number continues the account's: the header of the same commit says so.
            let last: Option<i64> = tx
                .query_row(
                    "SELECT last_sequence FROM ledger_accounts WHERE account_id = ?1",
                    params![account_key(&entry.account)],
                    |row| row.get(0),
                )
                .optional()
                .map_err(db)?;
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
            tx.execute(
                "INSERT INTO ledger_entries (account_id, sequence, idempotency_key, caused_by, revision, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    account_key(&entry.account),
                    entry.sequence as i64,
                    entry.idempotency_key,
                    entry.caused_by.as_ref().map(|p| p.as_str().to_string()),
                    revision.0 as i64,
                    encode(&stored)
                ],
            )
            .map_err(db)?;
        }
    }
    Ok(())
}

/// Refuse to restore a snapshot older than an entry: entries are never removed (LDG-022).
pub(crate) fn check_restorable(tx: &Connection, revision: StateRevision) -> StoreResult<()> {
    let newer: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM ledger_entries WHERE revision > ?1",
            params![revision.0 as i64],
            |row| row.get(0),
        )
        .map_err(db)?;
    if newer > 0 {
        return Err(StoreError::Invalid(
            "the ledger holds entries posted after the snapshot, and entries are never removed"
                .into(),
        ));
    }
    Ok(())
}

/// Clear a principal from the ledger of a tenant file (LDG-027, LDG-028): the owner of its
/// accounts (columns and header) and `caused_by` of its entries. Amounts, keys, reasons, numbers,
/// times and balances stay.
pub(crate) fn anonymise(tx: &Transaction<'_>, principal: &PrincipalId) -> StoreResult<Anonymised> {
    let owned = select_accounts(
        tx,
        "SELECT data FROM ledger_accounts WHERE owner_kind = 1 AND owner_name = ?1",
        &[&principal.as_str()],
    )?;
    let mut done = Anonymised::default();
    for data in owned {
        let mut account: LedgerAccount = decode(&data, "ledger account")?;
        account.owner = None;
        tx.execute(
            "UPDATE ledger_accounts SET owner_kind = 0, owner_name = NULL, data = ?1 WHERE account_id = ?2",
            params![encode(&account), account_key(&account.account)],
        )
        .map_err(db)?;
        done.accounts += 1;
    }
    done.entries = tx
        .execute(
            "UPDATE ledger_entries SET caused_by = NULL WHERE caused_by = ?1",
            params![principal.as_str()],
        )
        .map_err(db)? as u64;
    Ok(done)
}

/// The ledger state a load built is none when it holds nothing.
pub(crate) fn tidy(state: &mut TenantState) {
    strata_kernel::ledger::tidy_state(&mut state.ledger);
}
