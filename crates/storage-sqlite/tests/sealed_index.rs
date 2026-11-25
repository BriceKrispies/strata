//! What the record index holds of a sealed personal field (D-6585, PRV-1588, WG-5112): a field
//! that is not searchable has no row at all, a searchable one a row for the blind hash its token
//! carries and none for its plain value, and the erasure of the subject takes the row away with
//! the record's rewrite. The tokens are made by hand here: sealing itself is the sealed store's
//! (`strata_storage_api::sealed`), which this adapter stores as it is given.

use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_storage_api::sealed::{blind_value, format_token};
use strata_storage_api::{execute, Store};
use strata_storage_sqlite::{default_tenants_dir, SqliteStore};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn fresh_path() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let path = dir.join(format!(
        "sealed-index-{}-{}.db",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    let _ = std::fs::remove_dir_all(default_tenants_dir(&path));
    path
}

fn personal(id: &str, searchable: bool) -> FieldDef {
    FieldDef {
        personal: Some(PersonalDeclaration {
            kind: PersonalKind::EmailAddress,
            subject: DataSubject::Field {
                field: field("person"),
            },
            searchable,
        }),
        normalise: None,
        ..field_def(id, FieldType::Str, false)
    }
}

fn owner() -> Principal {
    user("alpha:owner")
}

fn setup(store: &SqliteStore) {
    execute(
        store,
        &Intent::CreateTenant {
            name: "alpha".into(),
            owner: principal_id("alpha:owner"),
        },
        &ctx("alpha", Principal::System, 10),
    )
    .unwrap();
    let profile = object("profile");
    let mut changes = vec![SchemaChange::DefineObject {
        id: profile.clone(),
        label: "Profile".into(),
    }];
    for def in [
        field_def("person", FieldType::Str, false),
        personal("email", true),
        personal("bio", false),
        field_def("plan", FieldType::Str, false),
    ] {
        changes.push(SchemaChange::DefineField {
            object: profile.clone(),
            field: def,
        });
    }
    changes.push(SchemaChange::SetRetention {
        object: profile,
        retention: RetentionRule {
            days: 30,
            from: RetentionStart::Created,
        },
    });
    execute(
        store,
        &Intent::ChangeSchema {
            changes,
            expected_version: None,
            existing: ExistingData::Reject,
        },
        &ctx("alpha", owner(), 11),
    )
    .unwrap();
    let fields = [
        ("person", text("alpha:ada")),
        (
            "email",
            Value::Str(format_token("k1", "b10c", "cGF5bG9hZC1lbWFpbA")),
        ),
        ("bio", Value::Str(format_token("k1", "", "cGF5bG9hZC1iaW8"))),
        ("plan", text("free")),
    ]
    .into_iter()
    .map(|(name, value)| (field(name), value))
    .collect();
    execute(
        store,
        &Intent::CreateRecord {
            object: object("profile"),
            fields,
        },
        &ctx("alpha", owner(), 12),
    )
    .unwrap();
}

fn rows(file: &std::path::Path, field: &str) -> Vec<String> {
    let conn = Connection::open(file).unwrap();
    let mut statement = conn
        .prepare("SELECT value FROM record_index WHERE object = 'profile' AND field = ?1")
        .unwrap();
    let found = statement
        .query_map([field], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    found
}

// Verifies: PRV-1588
// Verifies: PRV-2723
#[test]
fn a_personal_field_that_is_not_searchable_has_no_index_row() {
    let path = fresh_path();
    let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
    setup(&store);
    let file = store.tenant_file(&tenant_id("alpha")).unwrap().unwrap();
    assert!(rows(&file, "bio").is_empty());
    let email = rows(&file, "email");
    assert_eq!(
        email,
        vec![canon::canonical_json(&blind_value("b10c"))],
        "a searchable sealed field is indexed by its blind hash only"
    );
    assert_eq!(
        rows(&file, "plan"),
        vec![canon::canonical_json(&text("free"))]
    );
    // The blind hash finds the record.
    let state = store
        .load(
            &tenant_id("alpha"),
            &[Need::ByField {
                object: object("profile"),
                field: field("email"),
                value: blind_value("b10c"),
            }],
        )
        .unwrap();
    assert_eq!(
        state.tenant.unwrap().records_of(&object("profile")).count(),
        1
    );
}

/// The history of the schema keeps no team and no invitation: they name people (WG-5111).
// Verifies: PRV-2726
#[test]
fn the_schema_history_keeps_no_teams_or_invitations() {
    let path = fresh_path();
    let store = SqliteStore::open(&path, Timestamp(1)).unwrap();
    setup(&store);
    execute(
        &store,
        &Intent::CreateInvitation {
            invitation: InvitationId::new("inv-1").unwrap(),
            address: "someone@example.org".into(),
            roles: [role("owner")].into_iter().collect(),
        },
        &ctx("alpha", owner(), 30),
    )
    .unwrap();
    let file = store.tenant_file(&tenant_id("alpha")).unwrap().unwrap();
    let conn = Connection::open(&file).unwrap();
    let mut statement = conn.prepare("SELECT schema FROM schema_history").unwrap();
    let history = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(!history.is_empty());
    for schema in &history {
        assert!(!schema.contains("someone@example.org"), "{schema}");
        assert!(!schema.contains("alpha:owner"), "{schema}");
    }
    // The current schema still has the invitation.
    let current = store
        .load(&tenant_id("alpha"), &[])
        .unwrap()
        .tenant
        .unwrap();
    assert!(canon::canonical_json(&*current.schema).contains("someone@example.org"));
}
