//! The fleet migration: every tenant file is brought up to this binary's layout in one pass, each
//! tenant has a status, and the run stops once failures pass the stated share of the fleet.

use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_storage_api::execute;
use strata_storage_sqlite::{migrate_in_order, tenant_migration_steps, SqliteStore, TenantStatus};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn tenant(name: &str) -> TenantId {
    TenantId::new(name).unwrap()
}

fn fresh_path(name: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "fleet-{name}-{}-{}.db",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    let _ = std::fs::remove_dir_all(strata_storage_sqlite::default_tenants_dir(&path));
    path
}

fn create(store: &SqliteStore, name: &str) {
    execute(
        store,
        &Intent::CreateTenant {
            name: name.into(),
            owner: principal_id(&format!("{name}:owner")),
        },
        &ctx(name, Principal::System, 10),
    )
    .unwrap();
}

#[test]
fn the_run_stops_when_failures_pass_the_stated_share_and_leaves_the_rest_untouched() {
    let names = ["a", "b", "c", "d", "e"].map(tenant).to_vec();
    let mut visited = Vec::new();
    // Two of five fail: 40 percent. A limit of 30 stops at the second failure.
    let report = migrate_in_order(names.clone(), 30, |t| {
        visited.push(t.as_str().to_string());
        if t.as_str() == "b" || t.as_str() == "c" {
            Err("a step failed".into())
        } else {
            Ok(())
        }
    });
    assert!(report.stopped);
    assert_eq!(visited, ["a", "b", "c"], "d and e were never opened");
    assert_eq!(report.tenants[0].1, TenantStatus::Ready);
    assert_eq!(
        report.tenants[2].1,
        TenantStatus::Failed("a step failed".into())
    );
    assert_eq!(report.tenants[4].1, TenantStatus::NotAttempted);
    assert_eq!(
        (report.ready(), report.failed(), report.not_attempted()),
        (1, 2, 2)
    );
    assert!(!report.complete());

    // The same failures under a limit of 40 do not stop the run.
    let report = migrate_in_order(names.clone(), 40, |t| {
        if t.as_str() == "b" || t.as_str() == "c" {
            Err("a step failed".into())
        } else {
            Ok(())
        }
    });
    assert!(!report.stopped);
    assert_eq!(
        (report.ready(), report.failed(), report.not_attempted()),
        (3, 2, 0)
    );

    // A limit of 0 stops at the first failure.
    let report = migrate_in_order(names, 0, |t| {
        if t.as_str() == "a" {
            Err("no".into())
        } else {
            Ok(())
        }
    });
    assert!(report.stopped);
    assert_eq!(report.not_attempted(), 4);
    assert!(report.render().contains("a: failed: no"));
}

#[test]
fn a_fleet_migration_reports_each_tenant_and_refuses_a_file_from_a_newer_version() {
    let path = fresh_path("fleet");
    let file = {
        let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
        for name in ["alpha", "bravo", "charlie"] {
            create(&store, name);
        }
        store.tenant_file(&tenant("bravo")).unwrap().unwrap()
    };
    let known = tenant_migration_steps().len() as i64;
    Connection::open(&file)
        .unwrap()
        .execute(
            "INSERT INTO tenant_migrations (version, name, applied_at) VALUES (?1, 'from the future', 1)",
            [known + 1],
        )
        .unwrap();

    let store = SqliteStore::open(&path, Timestamp(2)).unwrap();
    let report = store.migrate_fleet(100).unwrap();
    assert!(!report.stopped);
    assert_eq!(status_of(&report, "alpha"), TenantStatus::Ready);
    assert_eq!(status_of(&report, "charlie"), TenantStatus::Ready);
    match status_of(&report, "bravo") {
        TenantStatus::Failed(why) => assert!(why.contains("newer version"), "{why}"),
        other => panic!("a file of a newer version was opened: {other:?}"),
    }

    // Stopping at the first failure leaves the tenants after bravo (in id order) unvisited.
    let report = store.migrate_fleet(0).unwrap();
    assert!(report.stopped);
    assert_eq!(status_of(&report, "charlie"), TenantStatus::NotAttempted);
}

fn status_of(report: &strata_storage_sqlite::FleetReport, name: &str) -> TenantStatus {
    report
        .tenants
        .iter()
        .find(|(t, _)| t.as_str() == name)
        .unwrap()
        .1
        .clone()
}
