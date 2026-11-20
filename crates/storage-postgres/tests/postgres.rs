//! The PostgreSQL adapter passes the conformance suite against a real server.
//!
//! The server is named by `STRATA_PG_URL`, such as
//! `postgres://strata:strata@127.0.0.1:55432/strata`; the role must be allowed to create
//! databases. Without the variable every test here fails and says so: it never passes silently.
//!
//! Each store a test makes gets a database of its own, `strata_conf_<process>_<n>`, created for it,
//! so that no case sees another's data. The databases a previous run left are dropped before the
//! first one of this run is created.

use postgres::{Client, NoTls};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use strata_kernel::Timestamp;
use strata_storage_api::{conformance, Store};
use strata_storage_postgres::PostgresStore;

/// The variable that names the server.
const URL_VARIABLE: &str = "STRATA_PG_URL";

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Held while a database is created or dropped, so that this run's statements do not race.
static ADMIN: Mutex<()> = Mutex::new(());

/// The old databases are dropped once per run, before the first is created.
static CLEARED: OnceLock<()> = OnceLock::new();

/// The server's connection string, or a failure that names the variable.
fn server_url() -> String {
    match std::env::var(URL_VARIABLE) {
        Ok(url) if !url.trim().is_empty() => url,
        _ => panic!(
            "{URL_VARIABLE} is not set: these tests need a PostgreSQL server, named by a \
             connection string such as postgres://strata:strata@127.0.0.1:55432/strata"
        ),
    }
}

/// The connection string of `database` on the same server as `url`.
fn url_of_database(url: &str, database: &str) -> String {
    match url.find("://") {
        Some(scheme) => {
            let rest = &url[scheme + 3..];
            let (base, query) = match rest.find('?') {
                Some(q) => (&rest[..q], &rest[q..]),
                None => (rest, ""),
            };
            let host = match base.find('/') {
                Some(slash) => &base[..slash],
                None => base,
            };
            format!("{}{host}/{database}{query}", &url[..scheme + 3])
        }
        // A key=value string: a later key wins.
        None => format!("{url} dbname={database}"),
    }
}

fn admin(url: &str) -> Client {
    Client::connect(url, NoTls)
        .unwrap_or_else(|e| panic!("could not connect to the server named by {URL_VARIABLE}: {e}"))
}

/// Drop every `strata_conf_*` database a previous run left behind.
fn clear_old_databases(url: &str) {
    let mut client = admin(url);
    let names: Vec<String> = client
        .query(
            "SELECT datname::text FROM pg_database WHERE datname::text LIKE 'strata\\_conf\\_%'",
            &[],
        )
        .expect("the list of databases")
        .iter()
        .map(|row| row.get::<_, String>(0))
        .collect();
    for name in names {
        // The names are ours: lower-case letters, digits and `_`.
        if name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            if let Err(error) = client.batch_execute(&format!("DROP DATABASE IF EXISTS \"{name}\""))
            {
                eprintln!("could not drop {name}: {error}");
            }
        }
    }
}

/// A fresh database on the server, and a store connected to it.
fn fresh_store() -> PostgresStore {
    let url = server_url();
    let name = {
        let _turn = ADMIN
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        CLEARED.get_or_init(|| clear_old_databases(&url));
        let name = format!(
            "strata_conf_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        );
        admin(&url)
            .batch_execute(&format!("CREATE DATABASE \"{name}\""))
            .unwrap_or_else(|e| panic!("could not create the database {name}: {e}"));
        name
    };
    PostgresStore::connect(&url_of_database(&url, &name), Timestamp(1))
        .unwrap_or_else(|e| panic!("could not connect a store to {name}: {e}"))
}

/// Verifies: STO-002
///
/// Every row of the table in the specification has a check and every check has a row, and each
/// check runs on a database of its own.
#[test]
fn postgres_store_conforms() {
    let ran = conformance::run(&|| Box::new(fresh_store()) as Box<dyn Store>);
    let spec = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../specifications/spec.jsonc"
    ))
    .expect("the specification file");
    let (mut ran, mut rows) = (ran, conformance::spec_cases(&spec));
    assert!(rows.len() >= 13, "the table was not read: {rows:?}");
    ran.sort();
    rows.sort();
    assert_eq!(
        ran, rows,
        "the checks that ran are not the rows of the conformance table"
    );
}

/// The pair index of named queries follows writes and query definitions.
// Verifies: KRN-2921
#[test]
fn postgres_store_answers_pair_pages() {
    conformance::pair_pages_follow_writes_and_definitions(&fresh_store());
}

/// The identity service's documents are kept per tenant and kind.
#[test]
fn postgres_store_keeps_documents() {
    conformance::documents_are_scoped_to_their_tenant_and_kind(&fresh_store());
}

/// The jobs of the automation worker, its wake index in the control schema and its run, step and
/// exception records.
#[test]
fn postgres_store_keeps_jobs() {
    conformance::jobs_conform(&fresh_store());
}

/// Schedules, run keys, the retention of idempotency entries and the purge of the worker's
/// records.
#[test]
fn postgres_store_keeps_schedules_run_keys_retention_and_purges() {
    conformance::schedules_conform(&fresh_store());
    conformance::run_keys_conform(&fresh_store());
    conformance::idempotency_retention_is_a_property_of_the_entry(&fresh_store());
    conformance::automation_purge_conforms(&fresh_store());
}

/// The audit trail is kept per tenant.
#[test]
fn postgres_store_keeps_the_audit_trail() {
    conformance::audit_is_kept_per_tenant(&fresh_store());
}

/// The layout history the store reports is the control steps, as the SQLite adapter's is.
#[test]
fn postgres_store_reports_the_control_steps() {
    let store = fresh_store();
    let names: Vec<String> = store
        .migrations()
        .unwrap()
        .into_iter()
        .map(|m| m.name)
        .collect();
    assert_eq!(
        names,
        [
            "tenant directory and session directory",
            "directory state provisioning",
            "job wake index",
            "tenant plan",
            "platform level of settings",
        ]
    );
    assert!(store.health().unwrap().ok);
}

#[test]
fn a_database_url_is_rewritten_for_another_database() {
    assert_eq!(
        url_of_database("postgres://strata:strata@127.0.0.1:55432/strata", "x"),
        "postgres://strata:strata@127.0.0.1:55432/x"
    );
    assert_eq!(
        url_of_database("postgresql://u@h/db?sslmode=disable", "x"),
        "postgresql://u@h/x?sslmode=disable"
    );
    assert_eq!(url_of_database("postgres://u@h", "x"), "postgres://u@h/x");
}
