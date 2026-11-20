//! Connections and transactions.
//!
//! A [`Pool`] is a small set of idle `postgres::Client`s of one server and database, behind a
//! mutex. A caller takes a client out, uses it, and gives it back; the mutex is held only to take
//! and to give back, never while a statement runs, so two callers never wait on each other's
//! connection, and a caller that needs a second connection (the control database while it has a
//! tenant's open) opens one instead of deadlocking on its own.
//!
//! A [`Scope`] is one schema of one pool. Its transactions set `search_path` to that schema
//! (`SET LOCAL`, so it ends with the transaction), and the statements name their tables
//! unqualified, exactly as the SQLite adapter names the tables of a tenant file. A schema name is
//! the only text ever formatted into SQL, and only once [`valid_schema`] has accepted it.

use crate::db;
use postgres::{Client, IsolationLevel, NoTls, Transaction};
use std::sync::{Arc, Mutex};
use strata_storage_api::{StoreError, StoreResult};

/// How many idle connections a pool keeps. More may be open at once; the rest are closed when
/// they are given back.
const MAX_IDLE: usize = 4;

/// Idle connections to one database.
pub(crate) struct Pool {
    url: String,
    idle: Mutex<Vec<Client>>,
}

impl Pool {
    pub(crate) fn new(url: &str) -> Self {
        Pool {
            url: url.to_string(),
            idle: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    fn take(&self) -> StoreResult<Client> {
        loop {
            let found = self
                .idle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pop();
            match found {
                Some(client) if !client.is_closed() => return Ok(client),
                Some(_) => continue,
                None => break,
            }
        }
        Client::connect(&self.url, NoTls).map_err(db)
    }

    fn give(&self, client: Client) {
        if client.is_closed() {
            return;
        }
        let mut idle = self
            .idle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if idle.len() < MAX_IDLE {
            idle.push(client);
        }
    }

    /// Run `work` on a connection of the pool. An open transaction `work` left behind is rolled
    /// back when it is dropped, before the connection is used again.
    pub(crate) fn with<T>(
        &self,
        work: impl FnOnce(&mut Client) -> StoreResult<T>,
    ) -> StoreResult<T> {
        let mut client = self.take()?;
        let result = work(&mut client);
        self.give(client);
        result
    }
}

/// Whether `name` may be used as a schema name: 1 to 63 bytes of `[a-z0-9_]`, not starting with a
/// digit. Such a name needs no escaping inside double quotes, and PostgreSQL does not truncate it.
pub(crate) fn valid_schema(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && !name.as_bytes()[0].is_ascii_digit()
}

/// A schema name as a quoted identifier, or a refusal of a name [`valid_schema`] does not accept.
pub(crate) fn quoted(name: &str) -> StoreResult<String> {
    if valid_schema(name) {
        Ok(format!("\"{name}\""))
    } else {
        Err(StoreError::Invalid(format!(
            "{name:?} is not a schema name this adapter uses"
        )))
    }
}

/// One schema of one database.
#[derive(Clone)]
pub(crate) struct Scope {
    pub(crate) pool: Arc<Pool>,
    pub(crate) schema: String,
}

impl Scope {
    fn enter(&self, t: &mut Transaction<'_>) -> StoreResult<()> {
        t.batch_execute(&format!("SET LOCAL search_path TO {}", quoted(&self.schema)?))
            .map_err(db)
    }

    /// The key of the transaction-level advisory lock that serialises the writers of the schema.
    fn lock_key(&self) -> String {
        format!("strata:{}", self.schema)
    }

    /// Run `work` in a READ COMMITTED transaction that holds the schema's writer lock (a
    /// transaction-level advisory lock, released at commit or rollback), and commit it when
    /// `work` returns `Ok`. One writer at a time per schema, as SQLite's `BEGIN IMMEDIATE` gives
    /// one writer at a time per file; readers are not blocked.
    pub(crate) fn write<T>(
        &self,
        work: impl FnOnce(&mut Transaction<'_>) -> StoreResult<T>,
    ) -> StoreResult<T> {
        let key = self.lock_key();
        self.pool.with(|client| {
            let mut t = client.transaction().map_err(db)?;
            t.execute(
                "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
                &[&key],
            )
            .map_err(db)?;
            self.enter(&mut t)?;
            let out = work(&mut t)?;
            t.commit().map_err(db)?;
            Ok(out)
        })
    }

    /// Run `work` in a REPEATABLE READ, READ ONLY transaction: every statement of it sees the
    /// same snapshot, so a load sees whole commits only and never a mixture of two.
    pub(crate) fn read<T>(
        &self,
        work: impl FnOnce(&mut Transaction<'_>) -> StoreResult<T>,
    ) -> StoreResult<T> {
        self.pool.with(|client| {
            let mut t = client
                .build_transaction()
                .isolation_level(IsolationLevel::RepeatableRead)
                .read_only(true)
                .start()
                .map_err(db)?;
            self.enter(&mut t)?;
            let out = work(&mut t)?;
            t.commit().map_err(db)?;
            Ok(out)
        })
    }
}
