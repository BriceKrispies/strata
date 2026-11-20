//! The limits of single accounts in a tenant file (WG-5172): a limit set now is a row of
//! `ledger_limits` and not part of the schema, a file made when the schema held them is upgraded
//! with the same limits in the table and out of the schema, and a posting loads the one limit it
//! needs and no other.

use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_storage_api::{execute, Store};
use strata_storage_sqlite::{default_tenants_dir, SqliteStore};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn fresh_path(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let path = dir.join(format!(
        "limits-{name}-{}-{}.db",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    let _ = std::fs::remove_dir_all(default_tenants_dir(&path));
    path
}

fn points() -> LedgerUnit {
    LedgerUnit::new("points").unwrap()
}

fn writer() -> LedgerOwner {
    LedgerOwner::Principal {
        principal: principal_id("alpha:writer1"),
    }
}

fn limit(owner: Option<LedgerOwner>, scope: Option<&str>, cap: i64) -> LedgerLimit {
    LedgerLimit {
        unit: points(),
        owner,
        floor: None,
        cap: Some(LedgerAmount(cap)),
        period_cap: None,
        period: None,
        scope: scope.map(|s| LedgerScope(s.to_string())),
    }
}

fn count(file: &Path, sql: &str) -> i64 {
    let conn = Connection::open(file).unwrap();
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// A tenant with the notes schema, the unit `points`, the limit of the unit and the limits of
/// two accounts of writer1 (the empty scope and `s1`).
fn tenant_with_limits(store: &SqliteStore) {
    let owner = principal_id("alpha:owner");
    execute(
        store,
        &Intent::CreateTenant {
            name: "alpha".into(),
            owner: owner.clone(),
        },
        &ctx("alpha", Principal::System, 10),
    )
    .unwrap();
    let mut changes = notes_schema();
    changes.push(SchemaChange::DeclareLedgerUnit(LedgerUnitDecl {
        unit: points(),
        description: "Points".into(),
        expiry: None,
    }));
    changes.push(SchemaChange::SetLedgerLimit(limit(None, None, 1000)));
    changes.push(SchemaChange::SetLedgerLimit(limit(
        Some(writer()),
        None,
        10,
    )));
    changes.push(SchemaChange::SetLedgerLimit(limit(
        Some(writer()),
        Some("s1"),
        20,
    )));
    execute(
        store,
        &Intent::ChangeSchema {
            changes,
            expected_version: None,
            existing: ExistingData::Reject,
        },
        &ctx("alpha", Principal::User(owner), 11),
    )
    .unwrap();
}

fn limit_need(scope: &str) -> Need {
    Need::Ledger(LedgerNeed::Limit {
        owner: writer(),
        unit: points(),
        scope: LedgerScope(scope.to_string()),
    })
}

/// Verifies: LDG-1550
#[test]
fn a_limit_of_an_account_is_a_row_and_the_schema_carries_only_the_limit_of_the_unit() {
    let path = fresh_path("rows");
    let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
    tenant_with_limits(&store);
    let alpha = tenant_id("alpha");
    let file = store.tenant_file(&alpha).unwrap().unwrap();
    assert_eq!(count(&file, "SELECT COUNT(*) FROM ledger_limits"), 2);
    assert_eq!(
        count(
            &file,
            "SELECT json_array_length(json_extract(schema, '$.ledger.limits')) FROM tenant"
        ),
        1
    );
    // A schema load names no account limit, and a need loads exactly the one it names.
    let header = store.load(&alpha, &[]).unwrap().tenant.unwrap();
    assert_eq!(header.schema.ledger.as_ref().unwrap().limits.len(), 1);
    assert!(header.ledger.is_none());
    let one = store
        .load(&alpha, &[limit_need("s1")])
        .unwrap()
        .tenant
        .unwrap();
    assert_eq!(
        one.ledger.as_ref().unwrap().limits,
        Some(vec![limit(Some(writer()), Some("s1"), 20)])
    );
    // Removing one removes its row.
    execute(
        &store,
        &Intent::ChangeSchema {
            changes: vec![SchemaChange::RemoveLedgerDefinition(
                LedgerDefinitionRef::Limit {
                    unit: points(),
                    owner: Some(writer()),
                    scope: Some(LedgerScope("s1".into())),
                },
            )],
            expected_version: None,
            existing: ExistingData::Reject,
        },
        &ctx("alpha", Principal::User(principal_id("alpha:owner")), 12),
    )
    .unwrap();
    assert_eq!(count(&file, "SELECT COUNT(*) FROM ledger_limits"), 1);
}

/// Verifies: LDG-1550
///
/// A file made when the schema held the limits of accounts (the layout before the table) is
/// brought to the current one: the limits are in the table, the schema holds only the limit of the
/// unit, and the state the store loads is the one it was before.
#[test]
fn a_file_whose_schema_held_account_limits_is_upgraded_with_them_in_the_table() {
    let path = fresh_path("upgrade");
    let alpha = tenant_id("alpha");
    let (file, before) = {
        let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
        tenant_with_limits(&store);
        let file = store.tenant_file(&alpha).unwrap().unwrap();
        let before = canon::hash_of(&store.load_complete(&alpha).unwrap().tenant);
        (file, before)
    };
    // Put the file back as the old layout left it: the limits of accounts inside the schema, no
    // table for them and no index of buckets, and the two steps not yet recorded.
    {
        let conn = Connection::open(&file).unwrap();
        conn.execute_batch(
            "UPDATE tenant SET schema = json_set(schema, '$.ledger.limits',
                 json((SELECT json_group_array(json(x)) FROM (
                         SELECT value AS x FROM json_each(tenant.schema, '$.ledger.limits')
                         UNION ALL
                         SELECT data AS x FROM ledger_limits))));
             DROP TABLE ledger_limits;
             DROP INDEX ledger_accounts_expiry;
             ALTER TABLE ledger_accounts DROP COLUMN oldest_bucket;
             DELETE FROM tenant_migrations WHERE name IN (
                 'tenant ledger account limits out of the schema',
                 'tenant ledger index of the oldest credit bucket');",
        )
        .unwrap();
        let in_schema: i64 = conn
            .query_row(
                "SELECT json_array_length(json_extract(schema, '$.ledger.limits')) FROM tenant",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(in_schema, 3, "the old layout held all three in the schema");
    }
    let store = SqliteStore::open(&path, Timestamp(2)).unwrap();
    let header = store.load(&alpha, &[]).unwrap().tenant.unwrap();
    assert_eq!(
        header.schema.ledger.as_ref().unwrap().limits,
        vec![limit(None, None, 1000)],
        "the schema keeps the limit of the unit"
    );
    assert!(header.ledger.is_none(), "and carries no account limit");
    assert_eq!(count(&file, "SELECT COUNT(*) FROM ledger_limits"), 2);
    let both = store
        .load(&alpha, &[limit_need(""), limit_need("s1")])
        .unwrap()
        .tenant
        .unwrap();
    assert_eq!(
        both.ledger.as_ref().unwrap().limits.as_ref().unwrap().len(),
        2
    );
    assert_eq!(
        canon::hash_of(&store.load_complete(&alpha).unwrap().tenant),
        before,
        "the state the store loads is what it was"
    );
}
