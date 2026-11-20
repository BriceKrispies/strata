//! The fixtures of the recorded-data test: the steps whose journal, deltas, events and states are
//! stored under `tests/recorded/`. Shared by `tests/recorded_data.rs`, which only reads the
//! recorded files, and by `examples/record_data.rs`, which is the only thing that writes them.

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_storage_api::JournalEntry;
use strata_substrate::bundle::{install, Bundle};

/// Every step of a fixture: the journal entry the store writes, the delta and the events.
#[derive(Default)]
pub struct Recording {
    pub world: World,
    journal: Vec<JournalEntry>,
    deltas: Vec<StateDelta>,
    events: Vec<DomainEvent>,
}

impl Recording {
    fn step(&mut self, ctx: Context, intent: Intent) -> Result<Output, DomainError> {
        let (next, outcome) = self.world.step(&intent, &ctx)?;
        self.world = next;
        self.journal.push(JournalEntry {
            tenant: ctx.tenant.clone(),
            base_revision: outcome.delta.base_revision,
            revision: outcome.delta.new_revision,
            kernel_version: KERNEL_VERSION.to_string(),
            intent,
            context: ctx,
            delta_hash: canon::hash_of(&outcome.delta),
            events_hash: canon::hash_of(&outcome.events),
        });
        self.deltas.push(outcome.delta.clone());
        self.events.extend(outcome.events.iter().cloned());
        Ok(outcome.output)
    }

    fn accept(&mut self, ctx: Context, intent: Intent) {
        if let Err(e) = self.step(ctx, intent) {
            panic!("fixture intent rejected: {:?}: {}", e.kind, e.message);
        }
    }
}

/// The notes tenant founded step by step as `testkit::add_notes_tenant` does, then a note
/// created, updated and deleted, so the journal holds every kind of event.
pub fn notes() -> Recording {
    let mut rec = Recording::default();
    let owner = principal_id("alpha:owner");
    let mut now = 1_000;
    let mut at = |principal: Principal| {
        now += 1;
        ctx("alpha", principal, now)
    };
    rec.accept(
        at(Principal::System),
        Intent::CreateTenant {
            name: "alpha".to_string(),
            owner: owner.clone(),
        },
    );
    let owner = Principal::User(owner);
    rec.accept(
        at(owner.clone()),
        Intent::ChangeSchema {
            changes: notes_schema(),
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    for (name, role_name) in [
        ("editor", "editor"),
        ("writer1", "writer"),
        ("writer2", "writer"),
    ] {
        rec.accept(
            at(owner.clone()),
            Intent::AssignRole {
                principal: principal_id(&format!("alpha:{name}")),
                role: role(role_name),
            },
        );
    }
    rec.accept(at(owner.clone()), new_note("First note", "first-note"));
    let id = rec
        .world
        .tenants
        .values()
        .flat_map(|t| t.records.values().flat_map(|r| r.keys().copied()))
        .max()
        .expect("the note was created");
    rec.accept(
        at(owner.clone()),
        Intent::UpdateRecord {
            object: object("note"),
            id,
            fields: BTreeMap::from([(field("title"), text("First note, revised"))]),
            expected_version: Some(1),
        },
    );
    rec.accept(at(owner.clone()), new_note("Second note", "second-note"));
    rec.accept(
        at(owner),
        Intent::DeleteRecord {
            object: object("note"),
            id,
            expected_version: Some(2),
        },
    );
    rec
}

/// The publication template installed with a small set of records, as the template's own tests
/// install it.
pub fn publication() -> Recording {
    let mut doc: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../tenants/_templates/publication.json"
    ))
    .expect("the publication template is JSON");
    let extra = serde_json::json!({
        "id": "gazette",
        "name": "Gazette",
        "owner": "user:admin",
        "created_at": "2026-01-01T00:00:00Z",
        "members": [
            { "principal": "user:ed", "roles": ["editor"] },
            { "principal": "user:ann", "roles": ["author"] }
        ],
        "records": [
            { "object": "publication", "as": "user:admin", "at": "2026-01-02", "fields": { "key": "main", "name": "Gazette" } },
            { "object": "author", "key": "ann", "as": "user:ed", "at": "2026-01-02", "fields": { "name": "Ann", "slug": "ann", "principal": "user:ann" } },
            { "object": "category", "key": "craft", "as": "user:ed", "at": "2026-01-02", "fields": { "name": "Craft", "slug": "craft" } },
            {
                "object": "post", "key": "first", "as": "user:ann", "at": "2026-01-03T10:00:00Z",
                "fields": { "title": "First", "slug": "first", "body": "Hello *there*.", "author": "@ann", "category": "@craft" },
                "then": [
                    { "as": "user:ann", "at": "2026-01-04T08:00:00Z", "set": { "status": "published" } },
                    { "as": "user:ed", "at": "2026-01-04T09:00:00Z", "set": { "featured": true } }
                ]
            }
        ]
    });
    doc.as_object_mut()
        .expect("the template is an object")
        .extend(extra.as_object().expect("an object").clone());
    let bundle = Bundle::from_json(&doc.to_string()).expect("the bundle reads");
    let mut rec = Recording::default();
    install(&bundle, &mut |ctx, intent| rec.step(ctx, intent))
        .unwrap_or_else(|e| panic!("the publication template installs: {e}"));
    rec
}

pub fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/recorded")
}

/// One file: a value of `T` in the canonical form SQLite stores.
fn file<T: Serialize>(name: &str, value: &T) -> (String, String) {
    (name.to_string(), canon::canonical_json(value) + "\n")
}

/// Every file of a fixture: what each SQLite table holds for it.
pub fn files(prefix: &str, rec: &Recording) -> Vec<(String, String)> {
    let mut out = vec![
        file(&format!("{prefix}-world.json"), &rec.world),
        file(&format!("{prefix}-journal.json"), &rec.journal),
        file(&format!("{prefix}-deltas.json"), &rec.deltas),
        file(&format!("{prefix}-events.json"), &rec.events),
    ];
    for (id, tenant) in &rec.world.tenants {
        let at = format!("{prefix}-{id}");
        out.push(file(&format!("{at}-state.json"), &**tenant));
        out.push(file(&format!("{at}-schema.json"), &*tenant.schema));
        out.push(file(&format!("{at}-members.json"), &tenant.members));
        let records: Vec<&Record> = tenant
            .records
            .values()
            .flat_map(|r| r.values().map(|record| &**record))
            .collect();
        out.push(file(&format!("{at}-records.json"), &records));
    }
    out
}
