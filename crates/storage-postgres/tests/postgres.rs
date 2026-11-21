//! The PostgreSQL adapter passes the conformance suite against a real server.
//!
//! The server is named by `STRATA_PG_URL`, such as
//! `postgres://strata:strata@127.0.0.1:55432/strata`; the role must be allowed to create
//! databases. The test is ignored by default and runs with `--ignored`; without the variable that
//! run fails and says so: it never passes silently.
//!
//! Each store a test makes gets a database of its own, `strata_conf_<process>_<n>`, created for it,
//! so that no case sees another's data. The databases a previous run left are dropped before the
//! first one of this run is created.

use postgres::{Client, NoTls};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use strata_kernel::testkit::*;
use strata_kernel::{ExistingData, Intent, Principal, Timestamp};
use strata_storage_api::{conformance, execute, SeededNonces, Store};
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
    let url = fresh_database();
    PostgresStore::connect(&url, Timestamp(1))
        .unwrap_or_else(|e| panic!("could not connect a store to {url}: {e}"))
}

/// A fresh database on the server: its connection string.
fn fresh_database() -> String {
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
    url_of_database(&url, &name)
}

/// Verifies: STO-002
///
/// Every row of the table in the specification has a check and every check has a row, and each
/// check runs on a database of its own.
#[test]
#[ignore = "needs PostgreSQL: set STRATA_PG_URL and run with --ignored"]
fn postgres_store_conforms() {
    let ran = conformance::run(&|| Box::new(fresh_store()) as Box<dyn Store>);
    let spec = strata_spec_parts::read_spec_text(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../specifications/spec.jsonc"
    )))
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

/// Verifies: STO-002
///
/// The tenant generation of the contract holds on PostgreSQL: it follows what a load returns for
/// every method of the trait, a commit is accepted only at the generation its decision was made
/// on, a load's generation is the generation of its rows under concurrent commits, and a seeded
/// store is reproducible.
#[test]
#[ignore = "needs PostgreSQL: set STRATA_PG_URL and run with --ignored"]
fn postgres_store_keeps_the_tenant_generation() {
    conformance::generation_follows_every_store_method(&|| {
        Box::new(fresh_store()) as Box<dyn Store>
    });
    conformance::a_commit_names_the_generation_it_was_decided_on(&fresh_store());
    conformance::interleaved_commits_and_loads_keep_each_revision_with_one_generation(
        &fresh_store(),
    );
    conformance::generations_are_reproducible_from_the_seed(&|seed| {
        Box::new(fresh_store().with_nonces(SeededNonces::shared(seed))) as Box<dyn Store>
    });
}

/// Verifies: STO-002
///
/// The generation is one small row: it is read from a tenant whose schema document cannot be read.
#[test]
#[ignore = "needs PostgreSQL: set STRATA_PG_URL and run with --ignored"]
fn the_generation_is_read_from_a_tenant_with_an_unreadable_schema() {
    let url = fresh_database();
    let store = PostgresStore::connect(&url, Timestamp(1)).expect("a store");
    let alpha = tenant_id("alpha");
    execute(
        &store,
        &Intent::CreateTenant {
            name: "alpha".into(),
            owner: principal_id("alpha:owner"),
        },
        &ctx("alpha", Principal::System, 10),
    )
    .unwrap();
    execute(
        &store,
        &Intent::ChangeSchema {
            changes: notes_schema(),
            expected_version: None,
            existing: ExistingData::Reject,
        },
        &ctx("alpha", Principal::User(principal_id("alpha:owner")), 11),
    )
    .unwrap();
    let held = store.generation(&alpha).unwrap().expect("alpha has one");
    let schema = store
        .tenant_schema_of(&alpha)
        .unwrap()
        .expect("the tenant has a schema");
    admin(&url)
        .batch_execute(&format!(
            "UPDATE \"{schema}\".tenant SET schema = 'not json'"
        ))
        .expect("the schema document is damaged");
    assert!(
        store.load(&alpha, &[]).is_err(),
        "the schema document is unreadable"
    );
    assert_eq!(store.generation(&alpha).unwrap(), Some(held));
}

/// Verifies: STO-002
///
/// Header residency over PostgreSQL (WG-5390): the conformance suite passes through a
/// `ResidentStore`, the schema is decoded once per tenant per generation, a commit that does and
/// one that does not touch the header each leave it current, and a commit from a second
/// connection to the same database makes the next load refresh.
#[test]
#[ignore = "needs PostgreSQL: set STRATA_PG_URL and run with --ignored"]
fn resident_store_over_postgres_keeps_the_header_current() {
    use std::sync::Arc;
    use strata_storage_api::ResidentStore;
    let resident =
        || Box::new(ResidentStore::header_only(Arc::new(fresh_store()))) as Box<dyn Store>;
    let ran = conformance::run(&resident);
    assert!(ran.len() >= 13, "the suite ran: {ran:?}");
    conformance::generation_follows_every_store_method(&resident);
    let fresh = || Arc::new(fresh_store()) as Arc<dyn Store>;
    conformance::resident_header_is_decoded_once_per_generation(&fresh);
    conformance::a_commit_leaves_the_resident_header_current(&fresh);

    let url = fresh_database();
    let connect = || {
        PostgresStore::connect(&url, Timestamp(1))
            .unwrap_or_else(|e| panic!("could not connect a store to {url}: {e}"))
    };
    let first = Arc::new(connect());
    let second = connect();
    conformance::a_foreign_commit_refreshes_the_resident_header(first, &second);
}
