//! Concrete behaviours of the transition function, one rule per test.

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::testkit::*;
use strata_kernel::*;

fn step(
    world: &World,
    tenant: &str,
    who: &str,
    now: i64,
    intent: Intent,
) -> Result<(World, Transition), DomainError> {
    world.step(&intent, &ctx(tenant, user(&format!("{tenant}:{who}")), now))
}

fn must(world: &World, tenant: &str, who: &str, now: i64, intent: Intent) -> World {
    step(world, tenant, who, now, intent).expect("accepted").0
}

fn patch(id: u64, fields: &[(&str, Value)]) -> Intent {
    Intent::UpdateRecord {
        object: object("note"),
        id: RecordId(id),
        fields: fields.iter().map(|(k, v)| (field(k), v.clone())).collect(),
        expected_version: None,
    }
}

fn note(world: &World, tenant: &str, id: u64) -> std::sync::Arc<Record> {
    world.tenants[&tenant_id(tenant)]
        .record(&object("note"), RecordId(id))
        .expect("note exists")
        .clone()
}

#[test]
fn create_applies_defaults_and_allocates_the_next_id() {
    let world = notes_world();
    let (world, outcome) = step(
        &world,
        "alpha",
        "writer1",
        5_000,
        new_note("First", "first"),
    )
    .unwrap();
    assert_eq!(
        outcome.output,
        Output::Record {
            object: object("note"),
            id: RecordId(1),
            version: 1
        }
    );
    let created = note(&world, "alpha", 1);
    assert_eq!(created.get(&field("status")), &text("draft"));
    assert_eq!(created.created_at, Timestamp(5_000));
    assert_eq!(created.created_by, Some(principal_id("alpha:writer1")));
    assert_eq!(outcome.events.len(), 1);
    assert_eq!(
        outcome.effects,
        vec![Effect::Publish {
            seq: outcome.events[0].seq
        }]
    );
    assert!(outcome
        .evidence
        .iter()
        .any(|c| c.rule == "policy:writers_read_notes" && c.passed));
}

#[test]
fn a_rejection_explains_itself() {
    let world = notes_world();
    let error = step(
        &world,
        "alpha",
        "writer1",
        5_000,
        new_note("A title that is far too long to fit", "long"),
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::ConstraintViolated);
    let failed: Vec<_> = error.evidence.iter().filter(|c| !c.passed).collect();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].rule, "constraint:note.title_length");
    assert!(failed[0].detail.contains("len(new.title) <= 24"));
}

/// Verifies: KRN-007
///
/// Every kind of rejection, not one example: each refused step must list at least one check
/// that failed. A rejection whose evidence holds only passed checks is a refusal that does not
/// say why.
#[test]
fn every_rejection_lists_at_least_one_failed_check() {
    let world = must(
        &notes_world(),
        "alpha",
        "writer1",
        5_000,
        new_note("First", "first"),
    );
    let rejections: Vec<(&str, &str, Intent)> = vec![
        (
            "constraint",
            "writer1",
            new_note("A title that is far too long to fit", "long"),
        ),
        (
            "missing required",
            "writer1",
            Intent::CreateRecord {
                object: object("note"),
                fields: BTreeMap::from([(field("slug"), text("x"))]),
            },
        ),
        (
            "not authorised",
            "writer2",
            patch(1, &[("title", text("Mine now"))]),
        ),
        (
            "workflow",
            "writer1",
            patch(1, &[("status", text("published"))]),
        ),
        (
            "unknown record",
            "writer1",
            patch(42, &[("title", text("x"))]),
        ),
        (
            "unknown role",
            "owner",
            Intent::RevokeRole {
                principal: principal_id("alpha:editor"),
                role: role("nobody"),
            },
        ),
        (
            "stale version",
            "writer1",
            Intent::UpdateRecord {
                object: object("note"),
                id: RecordId(1),
                fields: BTreeMap::from([(field("body"), text("two"))]),
                expected_version: Some(7),
            },
        ),
    ];
    for (name, who, intent) in rejections {
        let Err(error) = step(&world, "alpha", who, 5_001, intent) else {
            panic!("{name} was accepted");
        };
        assert!(
            error.evidence.iter().any(|c| !c.passed),
            "{name} was refused as {:?} with no failed check listed: {:?}",
            error.kind,
            error.evidence
        );
    }

    // Refusals that come before any rule of the tenant is consulted, and the ones that come
    // from a stale or partial snapshot, are refusals too.
    let create = Intent::CreateTenant {
        name: "Gamma".into(),
        owner: principal_id("gamma:owner"),
    };
    let blank = Intent::CreateTenant {
        name: " ".into(),
        owner: principal_id("gamma:owner"),
    };
    let tenant = world.tenants[&tenant_id("alpha")].clone();
    let mut thin = TenantState::clone(&tenant);
    thin.records.clear();
    let thin = State {
        tenant_id: tenant_id("alpha"),
        tenant: Some(std::sync::Arc::new(thin)),
        coverage: Coverage::Partial(BTreeSet::new()),
    };
    let owner_of_alpha = ctx("alpha", user("alpha:owner"), 5_001);
    let early: Vec<(&str, DomainError)> = vec![
        (
            "tenant exists",
            world
                .step(&create, &ctx("alpha", Principal::System, 5_001))
                .unwrap_err(),
        ),
        (
            "no such tenant",
            world
                .step(
                    &new_note("T", "t"),
                    &ctx("gamma", user("alpha:owner"), 5_001),
                )
                .unwrap_err(),
        ),
        (
            "tenant created by someone but the system",
            world
                .step(&create, &ctx("gamma", user("alpha:owner"), 5_001))
                .unwrap_err(),
        ),
        (
            "tenant without a name",
            world
                .step(&blank, &ctx("gamma", Principal::System, 5_001))
                .unwrap_err(),
        ),
        (
            "snapshot of another tenant",
            transition(
                &world.state(&tenant_id("alpha")),
                &new_note("T", "t"),
                &ctx("beta", user("beta:owner"), 5_001),
            )
            .unwrap_err(),
        ),
        (
            "partial snapshot that lacks state",
            transition(
                &thin,
                &new_note("Second", "first"),
                &ctx("alpha", user("alpha:writer1"), 5_001),
            )
            .unwrap_err(),
        ),
        (
            "unknown object type",
            world
                .step(
                    &Intent::CreateRecord {
                        object: object("nothing"),
                        fields: BTreeMap::new(),
                    },
                    &owner_of_alpha,
                )
                .unwrap_err(),
        ),
        (
            "the last owner",
            world
                .step(
                    &Intent::RevokeRole {
                        principal: principal_id("alpha:owner"),
                        role: role("owner"),
                    },
                    &owner_of_alpha,
                )
                .unwrap_err(),
        ),
        (
            "schema change that is not consistent",
            world
                .step(
                    &Intent::ChangeSchema {
                        changes: vec![SchemaChange::RemoveField {
                            object: object("note"),
                            field: field("nothing"),
                        }],
                        expected_version: None,
                        existing: ExistingData::Reject,
                    },
                    &owner_of_alpha,
                )
                .unwrap_err(),
        ),
        (
            "stale schema version",
            world
                .step(
                    &Intent::ChangeSchema {
                        changes: vec![SchemaChange::RemoveField {
                            object: object("note"),
                            field: field("body"),
                        }],
                        expected_version: Some(SchemaVersion(99)),
                        existing: ExistingData::Reject,
                    },
                    &owner_of_alpha,
                )
                .unwrap_err(),
        ),
    ];
    for (name, error) in early {
        assert!(
            error.evidence.iter().any(|c| !c.passed),
            "{name} was refused as {:?} with no failed check listed: {:?}",
            error.kind,
            error.evidence
        );
    }
}

#[test]
fn required_type_unknown_field_and_unique_are_each_rejected() {
    let world = must(
        &notes_world(),
        "alpha",
        "writer1",
        5_000,
        new_note("First", "first"),
    );
    let create = |fields: &[(&str, Value)]| Intent::CreateRecord {
        object: object("note"),
        fields: fields.iter().map(|(k, v)| (field(k), v.clone())).collect(),
    };
    let kind = |intent| {
        step(&world, "alpha", "writer1", 5_001, intent)
            .unwrap_err()
            .kind
    };
    assert_eq!(
        kind(create(&[("slug", text("x"))])),
        ErrorKind::MissingRequired
    );
    assert_eq!(
        kind(create(&[("title", Value::Int(3)), ("slug", text("x"))])),
        ErrorKind::TypeMismatch
    );
    assert_eq!(
        kind(create(&[
            ("title", text("T")),
            ("slug", text("x")),
            ("nope", text("y"))
        ])),
        ErrorKind::UnknownField
    );
    assert_eq!(kind(new_note("Second", "first")), ErrorKind::UniqueViolated);
    assert_eq!(
        kind(create(&[
            ("title", text("T")),
            ("slug", text("x")),
            ("author", Value::Ref(RecordId(99)))
        ])),
        ErrorKind::DanglingReference
    );
}

#[test]
fn writers_edit_only_their_own_notes() {
    let world = must(
        &notes_world(),
        "alpha",
        "writer1",
        5_000,
        new_note("First", "first"),
    );
    let other = step(
        &world,
        "alpha",
        "writer2",
        5_001,
        patch(1, &[("title", text("Mine now"))]),
    )
    .unwrap_err();
    assert_eq!(other.kind, ErrorKind::Unauthorized);
    assert!(other
        .evidence
        .iter()
        .any(|c| c.rule == "policy:writers_edit_own" && !c.passed));
    let world = must(
        &world,
        "alpha",
        "writer1",
        5_002,
        patch(1, &[("title", text("Still mine"))]),
    );
    assert_eq!(note(&world, "alpha", 1).version, 2);
}

#[test]
fn workflow_guards_the_status_field_and_sets_fields_when_it_fires() {
    let world = must(
        &notes_world(),
        "alpha",
        "writer1",
        5_000,
        new_note("First", "first"),
    );
    let refused = step(
        &world,
        "alpha",
        "writer1",
        5_001,
        patch(1, &[("status", text("published"))]),
    )
    .unwrap_err();
    assert_eq!(refused.kind, ErrorKind::WorkflowViolation);
    let skipped = step(
        &world,
        "alpha",
        "editor",
        5_001,
        patch(1, &[("status", text("archived"))]),
    )
    .unwrap_err();
    assert_eq!(skipped.kind, ErrorKind::WorkflowViolation);

    let world = must(
        &world,
        "alpha",
        "editor",
        6_000,
        patch(1, &[("status", text("published"))]),
    );
    assert_eq!(
        note(&world, "alpha", 1).get(&field("published_at")),
        &Value::Time(6_000)
    );
    let world = must(
        &world,
        "alpha",
        "editor",
        7_000,
        patch(1, &[("status", text("draft"))]),
    );
    assert_eq!(
        note(&world, "alpha", 1).get(&field("published_at")),
        &Value::Null
    );

    let started_published = Intent::CreateRecord {
        object: object("note"),
        fields: BTreeMap::from([
            (field("title"), text("T")),
            (field("slug"), text("t")),
            (field("status"), text("published")),
        ]),
    };
    assert_eq!(
        step(&world, "alpha", "editor", 7_001, started_published)
            .unwrap_err()
            .kind,
        ErrorKind::WorkflowViolation
    );
}

/// Verifies: KRN-008
///
/// From each state of the declared workflow, every status the workflow does not declare as a
/// transition from that state is refused, whoever asks; the declared move is accepted for an
/// editor. The rows cover both states and the undeclared targets, not one example.
#[test]
fn undeclared_status_changes_are_refused_from_every_state() {
    let world = must(
        &notes_world(),
        "alpha",
        "writer1",
        5_000,
        new_note("First", "first"),
    );
    // draft: the declared move is to published; archived and an unknown word are not declared.
    for target in ["archived", "nonsense"] {
        let refused = step(
            &world,
            "alpha",
            "editor",
            5_001,
            patch(1, &[("status", text(target))]),
        )
        .unwrap_err();
        assert_eq!(
            refused.kind,
            ErrorKind::WorkflowViolation,
            "draft -> {target}"
        );
    }
    let published = must(
        &world,
        "alpha",
        "editor",
        5_002,
        patch(1, &[("status", text("published"))]),
    );
    // published: the declared move is back to draft; archived and an unknown word are not declared.
    for target in ["archived", "nonsense"] {
        let refused = step(
            &published,
            "alpha",
            "editor",
            5_003,
            patch(1, &[("status", text(target))]),
        )
        .unwrap_err();
        assert_eq!(
            refused.kind,
            ErrorKind::WorkflowViolation,
            "published -> {target}"
        );
    }
    let back = must(
        &published,
        "alpha",
        "editor",
        5_004,
        patch(1, &[("status", text("draft"))]),
    );
    assert_eq!(
        note(&back, "alpha", 1).get(&field("status")),
        &text("draft")
    );

    // A declared transition is permitted only where its guard holds: the writer who owns the
    // note is refused the move the editor was allowed.
    let guarded = step(
        &world,
        "alpha",
        "writer1",
        5_005,
        patch(1, &[("status", text("published"))]),
    )
    .unwrap_err();
    assert_eq!(guarded.kind, ErrorKind::WorkflowViolation);
    assert!(guarded
        .evidence
        .iter()
        .any(|c| !c.passed && c.rule == "workflow:note_status"));
    // A record is created only in an initial state of the workflow.
    for start in ["published", "archived", "nonsense"] {
        let refused = step(
            &world,
            "alpha",
            "editor",
            5_006,
            Intent::CreateRecord {
                object: object("note"),
                fields: BTreeMap::from([
                    (field("title"), text("T")),
                    (field("slug"), text("t")),
                    (field("status"), text(start)),
                ]),
            },
        )
        .unwrap_err();
        assert_eq!(
            refused.kind,
            ErrorKind::WorkflowViolation,
            "created as {start}"
        );
    }
    // A change that leaves the workflow field alone is not a workflow matter, in any state.
    must(
        &world,
        "alpha",
        "writer1",
        5_007,
        patch(1, &[("body", text("one"))]),
    );
    must(
        &published,
        "alpha",
        "writer1",
        5_008,
        patch(1, &[("body", text("two"))]),
    );
}

#[test]
fn a_stale_expected_version_is_a_conflict() {
    let world = must(
        &notes_world(),
        "alpha",
        "writer1",
        5_000,
        new_note("First", "first"),
    );
    let world = must(
        &world,
        "alpha",
        "writer1",
        5_001,
        patch(1, &[("body", text("one"))]),
    );
    let stale = Intent::UpdateRecord {
        object: object("note"),
        id: RecordId(1),
        fields: BTreeMap::from([(field("body"), text("two"))]),
        expected_version: Some(1),
    };
    assert_eq!(
        step(&world, "alpha", "writer1", 5_002, stale)
            .unwrap_err()
            .kind,
        ErrorKind::VersionConflict
    );
}

#[test]
fn deleting_a_referenced_record_clears_set_null_references() {
    let world = notes_world();
    let person = Intent::CreateRecord {
        object: object("person"),
        fields: BTreeMap::from([(field("name"), text("Ada"))]),
    };
    let world = must(&world, "alpha", "editor", 5_000, person);
    let with_author = Intent::CreateRecord {
        object: object("note"),
        fields: BTreeMap::from([
            (field("title"), text("T")),
            (field("slug"), text("t")),
            (field("author"), Value::Ref(RecordId(1))),
        ]),
    };
    let world = must(&world, "alpha", "editor", 5_001, with_author);
    let delete = Intent::DeleteRecord {
        object: object("person"),
        id: RecordId(1),
        expected_version: None,
    };
    let (world, outcome) = step(&world, "alpha", "editor", 5_002, delete).unwrap();
    assert_eq!(note(&world, "alpha", 2).get(&field("author")), &Value::Null);
    assert_eq!(
        outcome.events.len(),
        2,
        "one update for the cleared note, one delete"
    );
    assert!(world.tenants[&tenant_id("alpha")]
        .record(&object("person"), RecordId(1))
        .is_none());
}

fn require_rank(mode: ExistingData) -> Intent {
    Intent::ChangeSchema {
        changes: vec![SchemaChange::DefineField {
            object: object("note"),
            field: field_def("rank", FieldType::Int, true),
        }],
        expected_version: None,
        existing: mode,
    }
}

#[test]
fn a_strict_migration_is_rejected_when_existing_records_would_not_conform() {
    let world = must(
        &notes_world(),
        "alpha",
        "writer1",
        5_000,
        new_note("First", "first"),
    );
    let world = must(
        &world,
        "alpha",
        "writer1",
        5_001,
        new_note("Second", "second"),
    );
    let error = step(
        &world,
        "alpha",
        "owner",
        5_002,
        require_rank(ExistingData::Reject),
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::MigrationRejected);
    for id in [1, 2] {
        let rule = format!("existing:note/{id}:required:note.rank");
        assert!(
            error.evidence.iter().any(|c| c.rule == rule && !c.passed),
            "{rule} is not listed among the refusal's checks"
        );
    }
}

#[test]
fn a_grandfathered_migration_is_accepted_and_bites_on_the_next_write() {
    let world = must(
        &notes_world(),
        "alpha",
        "writer1",
        5_000,
        new_note("First", "first"),
    );
    let world = must(
        &world,
        "alpha",
        "writer1",
        5_001,
        new_note("Second", "second"),
    );
    let (world, outcome) = step(
        &world,
        "alpha",
        "owner",
        5_002,
        require_rank(ExistingData::Grandfather),
    )
    .unwrap();
    assert!(matches!(
        outcome.events[0].body,
        EventBody::SchemaChanged {
            grandfathered: 2,
            ..
        }
    ));
    // The non-conforming records are listed in the change's own evidence, one check each.
    for id in [1, 2] {
        let rule = format!("existing:note/{id}:required:note.rank");
        assert!(
            outcome.evidence.iter().any(|c| c.rule == rule && !c.passed),
            "{rule} is not listed in the accepted change's evidence"
        );
    }
    let stuck = step(
        &world,
        "alpha",
        "writer1",
        5_002,
        patch(1, &[("body", text("edit"))]),
    )
    .unwrap_err();
    assert_eq!(stuck.kind, ErrorKind::MissingRequired);
    must(
        &world,
        "alpha",
        "writer1",
        5_003,
        patch(1, &[("body", text("edit")), ("rank", Value::Int(1))]),
    );
}

/// Note 1 has a short title and a rank, note 2 has a long title and no rank.
fn mixed_notes() -> World {
    let world = must(
        &notes_world(),
        "alpha",
        "writer1",
        5_000,
        new_note("Short", "first"),
    );
    let world = must(
        &world,
        "alpha",
        "writer1",
        5_001,
        patch(1, &[("rank", Value::Int(1))]),
    );
    must(
        &world,
        "alpha",
        "writer1",
        5_002,
        new_note("A longer title", "second"),
    )
}

/// One change of each of three kinds that existing records can break, the rule each breaks and
/// the records of `mixed_notes` that break it: a new required field (note 2 has no rank), a
/// tighter constraint (note 2 has a long title) and a changed field type (note 1's rank is a
/// number).
fn breaking_changes() -> Vec<(&'static str, SchemaChange, &'static str, Vec<u64>)> {
    let shorter = SchemaChange::DefineConstraint {
        object: object("note"),
        constraint: ConstraintDef {
            id: ConstraintId::new("title_length").unwrap(),
            expr: Expr::Cmp(
                CmpOp::Le,
                Box::new(Expr::Len(Box::new(var(Scope::New, "title")))),
                Box::new(Expr::Lit(Value::Int(5))),
            ),
            message: "short".into(),
        },
    };
    vec![
        (
            "new required field",
            SchemaChange::DefineField {
                object: object("note"),
                field: field_def("rank", FieldType::Int, true),
            },
            "required:note.rank",
            vec![2],
        ),
        (
            "tighter constraint",
            shorter,
            "constraint:note.title_length",
            vec![2],
        ),
        (
            "changed field type",
            SchemaChange::DefineField {
                object: object("note"),
                field: field_def("rank", FieldType::Str, false),
            },
            "type:note.rank",
            vec![1],
        ),
    ]
}

fn change_in(mode: ExistingData, change: SchemaChange) -> Intent {
    Intent::ChangeSchema {
        changes: vec![change],
        expected_version: None,
        existing: mode,
    }
}

/// The checks `existing:<object>/<id>:<rule>` of an outcome that failed.
fn listed_existing(evidence: &[Check]) -> BTreeSet<String> {
    evidence
        .iter()
        .filter(|c| !c.passed && c.rule.starts_with("existing:"))
        .map(|c| c.rule.clone())
        .collect()
}

/// Verifies: KRN-009
///
/// Strict mode refuses each kind of change while a record would not conform, listing exactly
/// the records that do not conform, and accepts the same change where every record conforms.
#[test]
fn a_strict_migration_is_refused_for_every_kind_of_change_that_a_record_breaks() {
    let world = mixed_notes();
    for (name, change, rule, broken) in breaking_changes() {
        let intent = change_in(ExistingData::Reject, change);
        let error = step(&world, "alpha", "owner", 5_003, intent.clone()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::MigrationRejected, "{name}");
        let expected: BTreeSet<String> = broken
            .iter()
            .map(|id| format!("existing:note/{id}:{rule}"))
            .collect();
        assert_eq!(listed_existing(&error.evidence), expected, "{name}");
        // Where every record conforms, the same change is accepted.
        must(&notes_world(), "alpha", "owner", 5_003, intent);
    }
}

/// Verifies: KRN-010
///
/// Grandfather mode accepts each kind of change that records break, listing exactly the records
/// that do not conform in the change's own evidence and leaving every record as it was.
#[test]
fn a_grandfathered_migration_is_accepted_for_every_kind_of_change_that_a_record_breaks() {
    let world = mixed_notes();
    for (name, change, rule, broken) in breaking_changes() {
        let intent = change_in(ExistingData::Grandfather, change);
        let (changed, outcome) = step(&world, "alpha", "owner", 5_003, intent)
            .unwrap_or_else(|e| panic!("{name} was refused: {}", e.describe()));
        let expected: BTreeSet<String> = broken
            .iter()
            .map(|id| format!("existing:note/{id}:{rule}"))
            .collect();
        assert_eq!(listed_existing(&outcome.evidence), expected, "{name}");
        let EventBody::SchemaChanged { grandfathered, .. } = &outcome.events[0].body else {
            panic!("{name}: no schema event");
        };
        assert_eq!(*grandfathered, broken.len() as u64, "{name}");
        assert_eq!(
            changed.tenants[&tenant_id("alpha")].records,
            world.tenants[&tenant_id("alpha")].records,
            "{name}"
        );
    }
}

#[test]
fn an_inconsistent_schema_change_is_rejected_with_the_reason() {
    let world = notes_world();
    let bad = Intent::ChangeSchema {
        changes: vec![SchemaChange::DefineConstraint {
            object: object("note"),
            constraint: ConstraintDef {
                id: ConstraintId::new("broken").unwrap(),
                expr: eq(var(Scope::New, "no_such_field"), lit("x")),
                message: "never".into(),
            },
        }],
        expected_version: None,
        existing: ExistingData::Reject,
    };
    let error = step(&world, "alpha", "owner", 5_000, bad).unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidSchema);
    assert!(error.message.contains("unknown field no_such_field"));
    let not_owner = step(
        &world,
        "alpha",
        "editor",
        5_000,
        require_rank(ExistingData::Reject),
    )
    .unwrap_err();
    assert_eq!(not_owner.kind, ErrorKind::Unauthorized);
}

#[test]
fn removing_a_field_strips_it_from_existing_records() {
    let world = must(
        &notes_world(),
        "alpha",
        "writer1",
        5_000,
        new_note("First", "first"),
    );
    let world = must(
        &world,
        "alpha",
        "writer1",
        5_001,
        patch(1, &[("body", text("text"))]),
    );
    let remove = Intent::ChangeSchema {
        changes: vec![SchemaChange::RemoveField {
            object: object("note"),
            field: field("body"),
        }],
        expected_version: None,
        existing: ExistingData::Reject,
    };
    let world = must(&world, "alpha", "owner", 5_002, remove);
    assert!(!note(&world, "alpha", 1).fields.contains_key(&field("body")));
}

#[test]
fn reads_return_only_what_the_principal_may_see_in_the_query_order() {
    let mut world = notes_world();
    for (i, slug) in ["a", "b", "c"].iter().enumerate() {
        world = must(
            &world,
            "alpha",
            "writer1",
            5_000 + i as i64,
            new_note(slug, slug),
        );
        world = must(
            &world,
            "alpha",
            "editor",
            5_100 + i as i64,
            patch(i as u64 + 1, &[("rank", Value::Int(i as i64))]),
        );
    }
    world = must(
        &world,
        "alpha",
        "editor",
        6_000,
        patch(2, &[("status", text("published"))]),
    );
    let by_rank = ReadRequest::Named {
        query: QueryId::new("notes_by_rank").unwrap(),
        params: BTreeMap::new(),
    };
    let ids = |principal| -> Vec<u64> {
        world
            .read(&by_rank, &ctx("alpha", principal, 7_000))
            .unwrap()
            .rows
            .iter()
            .map(|r| r.id.0)
            .collect()
    };
    assert_eq!(ids(user("alpha:writer2")), vec![3, 2, 1]);
    assert_eq!(ids(Principal::Anonymous), vec![2]);
    assert_eq!(
        ids(user("beta:owner")),
        vec![2],
        "another tenant's owner is a stranger here"
    );

    let direct = ReadRequest::Records {
        object: object("note"),
        ids: vec![RecordId(1), RecordId(2)],
    };
    let seen = world
        .read(&direct, &ctx("alpha", Principal::Anonymous, 7_000))
        .unwrap();
    assert_eq!(seen.rows.len(), 1);
}

#[test]
fn only_the_system_creates_tenants_and_a_tenant_is_created_once() {
    let world = notes_world();
    let create = Intent::CreateTenant {
        name: "Gamma".into(),
        owner: principal_id("gamma:owner"),
    };
    let refused = world
        .step(&create, &ctx("gamma", user("alpha:owner"), 1))
        .unwrap_err();
    assert_eq!(refused.kind, ErrorKind::Unauthorized);
    let twice = world
        .step(&create, &ctx("alpha", Principal::System, 1))
        .unwrap_err();
    assert_eq!(twice.kind, ErrorKind::TenantExists);
    let nowhere = world
        .step(&new_note("T", "t"), &ctx("gamma", user("alpha:owner"), 1))
        .unwrap_err();
    assert_eq!(nowhere.kind, ErrorKind::TenantNotFound);
}

#[test]
fn a_snapshot_of_one_tenant_cannot_serve_a_context_naming_another() {
    let world = notes_world();
    let alpha = world.state(&tenant_id("alpha"));
    let error = transition(
        &alpha,
        &new_note("T", "t"),
        &ctx("beta", user("beta:owner"), 1),
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::TenantMismatch);
    let read_error = read(
        &alpha,
        &ReadRequest::Records {
            object: object("note"),
            ids: vec![],
        },
        &ctx("beta", user("beta:owner"), 1),
    )
    .unwrap_err();
    assert_eq!(read_error.kind, ErrorKind::TenantMismatch);
}

#[test]
fn a_thin_snapshot_is_refused_with_the_missing_need() {
    let world = must(
        &notes_world(),
        "alpha",
        "writer1",
        5_000,
        new_note("First", "first"),
    );
    let tenant = world.tenants[&tenant_id("alpha")].clone();
    let mut thin = TenantState::clone(&tenant);
    thin.records.clear();
    let state = State {
        tenant_id: tenant_id("alpha"),
        tenant: Some(std::sync::Arc::new(thin)),
        coverage: Coverage::Partial(BTreeSet::new()),
    };
    // Without the uniqueness lookup this would wrongly accept a duplicate slug.
    let error = transition(
        &state,
        &new_note("Second", "first"),
        &ctx("alpha", user("alpha:writer1"), 5_001),
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::InsufficientState);
    assert_eq!(
        error.missing,
        Some(Need::ByField {
            object: object("note"),
            field: field("slug"),
            value: text("first")
        })
    );
}

#[test]
fn removing_the_last_role_asks_for_the_sessions_to_end_and_the_last_owner_stays() {
    let world = notes_world();
    let revoke = Intent::RevokeRole {
        principal: principal_id("alpha:writer1"),
        role: role("writer"),
    };
    let (_, outcome) = step(&world, "alpha", "owner", 5_000, revoke).unwrap();
    assert!(outcome.effects.contains(&Effect::RevokeSessions {
        principal: principal_id("alpha:writer1")
    }));
    let last_owner = Intent::RevokeRole {
        principal: principal_id("alpha:owner"),
        role: role("owner"),
    };
    assert_eq!(
        step(&world, "alpha", "owner", 5_001, last_owner)
            .unwrap_err()
            .kind,
        ErrorKind::InvalidIntent
    );
}

#[test]
fn expressions_use_three_valued_logic() {
    use strata_kernel::expr::{eval, Env};
    let params = BTreeMap::new();
    let roles = BTreeSet::new();
    let env = Env {
        record: None,
        old: None,
        new: None,
        params: &params,
        now: Timestamp(0),
        principal: &Principal::Anonymous,
        roles: &roles,
        lookup: None,
    };
    let null = Expr::Lit(Value::Null);
    let t = Expr::Lit(Value::Bool(true));
    let f = Expr::Lit(Value::Bool(false));
    assert_eq!(eval(&eq(null.clone(), lit("x")), &env), Ok(Value::Null));
    assert_eq!(
        eval(&Expr::And(vec![null.clone(), f.clone()]), &env),
        Ok(Value::Bool(false))
    );
    assert_eq!(
        eval(&Expr::And(vec![null.clone(), t.clone()]), &env),
        Ok(Value::Null)
    );
    assert_eq!(
        eval(&Expr::Or(vec![null.clone(), t]), &env),
        Ok(Value::Bool(true))
    );
    assert_eq!(
        eval(&Expr::Or(vec![null.clone(), f]), &env),
        Ok(Value::Null)
    );
    assert!(
        eval(&eq(lit("x"), Expr::Lit(Value::Int(1))), &env).is_err(),
        "a type error is an error, not false"
    );
    assert_eq!(
        eval(&Expr::IsSlug(Box::new(lit("a-good-slug"))), &env),
        Ok(Value::Bool(true))
    );
    assert_eq!(
        eval(&Expr::IsSlug(Box::new(lit("Bad Slug"))), &env),
        Ok(Value::Bool(false))
    );
}

/// A unique clash names the record it clashes with only to someone who may read that record.
/// Anyone else learns that the value is taken, and nothing about a hidden record's identifier.
#[test]
fn a_unique_clash_with_a_hidden_record_does_not_name_it() {
    // A `guest` may create notes but reads only what the public reads: published ones.
    let guest = Intent::ChangeSchema {
        changes: vec![
            SchemaChange::DefineRole(RoleDef {
                id: role("guest"),
                label: "Guest".into(),
            }),
            SchemaChange::DefinePolicy(PolicyDef {
                id: PolicyId::new("guests_create_notes").unwrap(),
                role: role("guest"),
                resource: Resource::Object(object("note")),
                actions: BTreeSet::from([Action::Create]),
                condition: None,
            }),
        ],
        expected_version: None,
        existing: ExistingData::Reject,
    };
    let world = must(&notes_world(), "alpha", "owner", 5_000, guest);
    let world = must(
        &world,
        "alpha",
        "owner",
        5_001,
        Intent::AssignRole {
            principal: principal_id("alpha:stranger"),
            role: role("guest"),
        },
    );
    // writer1's draft, which the guest cannot read.
    let world = must(
        &world,
        "alpha",
        "writer1",
        5_002,
        new_note("Hidden", "hidden"),
    );
    let hidden = 1;

    let refused = step(
        &world,
        "alpha",
        "stranger",
        5_003,
        new_note("Mine", "hidden"),
    )
    .expect_err("the slug is taken");
    assert_eq!(refused.kind, ErrorKind::UniqueViolated);
    let told = format!("{} {:?}", refused.message, refused.evidence);
    assert!(
        !told.contains(&format!("record {hidden}")),
        "the hidden record was named: {told}"
    );
    assert!(refused
        .evidence
        .iter()
        .any(|c| !c.passed && c.rule == "unique:note.slug"));

    // A writer, who reads every note, is told which record holds the value.
    let refused = step(
        &world,
        "alpha",
        "writer2",
        5_004,
        new_note("Mine", "hidden"),
    )
    .expect_err("the slug is taken");
    assert!(
        refused
            .message
            .contains(&format!("record {hidden} already has this value")),
        "{}",
        refused.message
    );
}

// Roles as sets of permissions, teams and invitations (ADM-150 to ADM-153).

fn object_permission(name: &str, verbs: &[ObjectAction]) -> Permission {
    Permission::Object {
        object: object(name),
        actions: verbs.iter().copied().collect(),
    }
}

fn capability(name: &str) -> Permission {
    Permission::Capability(name.to_string())
}

/// The notes tenant with four more roles. `writer` creates notes, `reviewer` reads them,
/// `publisher` holds one capability, and `moderator` manages members and holds a capability and
/// the right to read notes. The principal `alpha:stranger` is a moderator.
fn access_world() -> World {
    let role_def = |id: &str, label: &str| {
        SchemaChange::DefineRole(RoleDef {
            id: role(id),
            label: label.into(),
        })
    };
    let permissions =
        |id: &str, permissions: Vec<Permission>| SchemaChange::DefineRolePermissions {
            role: role(id),
            permissions,
        };
    let world = must(
        &notes_world(),
        "alpha",
        "owner",
        6_000,
        Intent::ChangeSchema {
            changes: vec![
                role_def("moderator", "Moderator"),
                role_def("publisher", "Publisher"),
                role_def("reviewer", "Reviewer"),
                permissions(
                    "writer",
                    vec![object_permission("note", &[ObjectAction::Create])],
                ),
                permissions(
                    "reviewer",
                    vec![object_permission("note", &[ObjectAction::Read])],
                ),
                permissions("publisher", vec![capability("content.publish")]),
                permissions(
                    "moderator",
                    vec![
                        capability("people.manage"),
                        object_permission("note", &[ObjectAction::Read]),
                    ],
                ),
                SchemaChange::DefinePolicy(PolicyDef {
                    id: PolicyId::new("moderators_manage_members").unwrap(),
                    role: role("moderator"),
                    resource: Resource::Members,
                    actions: BTreeSet::from([Action::Manage]),
                    condition: None,
                }),
            ],
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    must(
        &world,
        "alpha",
        "owner",
        6_001,
        Intent::AssignRole {
            principal: principal_id("alpha:stranger"),
            role: role("moderator"),
        },
    )
}

fn held(world: &World, who: &str) -> BTreeSet<Grant> {
    strata_kernel::rules::effective_permissions(
        &world.tenants[&tenant_id("alpha")],
        &user(&format!("alpha:{who}")),
    )
}

fn roles_held_by(world: &World, who: &str) -> BTreeSet<RoleId> {
    strata_kernel::rules::roles_of(
        &world.tenants[&tenant_id("alpha")],
        &user(&format!("alpha:{who}")),
    )
}

fn refused(result: Result<(World, Transition), DomainError>) -> DomainError {
    match result {
        Ok(_) => panic!("the intent was accepted"),
        Err(error) => error,
    }
}

fn team(id: &str) -> TeamId {
    TeamId::new(id).unwrap()
}

fn define_team(id: &str) -> Intent {
    Intent::DefineTeam {
        team: team(id),
        label: id.to_string(),
    }
}

fn team_gets(id: &str, role_name: &str) -> Intent {
    Intent::AssignTeamRole {
        team: team(id),
        role: role(role_name),
    }
}

fn joins(id: &str, who: &str) -> Intent {
    Intent::AddTeamMember {
        team: team(id),
        principal: principal_id(&format!("alpha:{who}")),
    }
}

/// Verifies: ADM-150
/// Verifies: ADM-151
///
/// A principal holds the roles of each team it belongs to, and so what the role permits, for as
/// long as it belongs and no longer.
#[test]
fn a_team_confers_its_roles_while_the_principal_is_a_member() {
    let world = access_world();
    let world = must(&world, "alpha", "owner", 7_000, define_team("desk"));
    let world = must(&world, "alpha", "owner", 7_001, team_gets("desk", "writer"));
    let create = || new_note("From the desk", "from-the-desk");

    let error = refused(step(&world, "alpha", "newbie", 7_002, create()));
    assert_eq!(error.kind, ErrorKind::Unauthorized);
    assert!(held(&world, "newbie").is_empty());

    let world = must(&world, "alpha", "owner", 7_003, joins("desk", "newbie"));
    assert!(roles_held_by(&world, "newbie").contains(&role("writer")));
    assert_eq!(
        held(&world, "newbie"),
        BTreeSet::from([Grant::Object(object("note"), ObjectAction::Create)])
    );
    step(&world, "alpha", "newbie", 7_004, create()).expect("the team's role lets it create");

    let world = must(
        &world,
        "alpha",
        "owner",
        7_005,
        Intent::RemoveTeamMember {
            team: team("desk"),
            principal: principal_id("alpha:newbie"),
        },
    );
    assert!(!roles_held_by(&world, "newbie").contains(&role("writer")));
    assert!(held(&world, "newbie").is_empty());
    let error = refused(step(&world, "alpha", "newbie", 7_006, create()));
    assert_eq!(error.kind, ErrorKind::Unauthorized);
}

/// Verifies: ADM-151
///
/// What a principal holds is exactly the permissions of the roles assigned to it and of the
/// roles of its teams, and nothing of another team's roles or of a role nobody gave it.
#[test]
fn a_principal_holds_the_union_of_its_roles_and_its_teams_roles_and_nothing_more() {
    let world = access_world();
    let world = must(&world, "alpha", "owner", 7_000, define_team("desk"));
    let world = must(
        &world,
        "alpha",
        "owner",
        7_001,
        team_gets("desk", "reviewer"),
    );
    let world = must(&world, "alpha", "owner", 7_002, define_team("press"));
    let world = must(
        &world,
        "alpha",
        "owner",
        7_003,
        team_gets("press", "publisher"),
    );
    let world = must(&world, "alpha", "owner", 7_004, joins("desk", "writer1"));

    let direct = Grant::Object(object("note"), ObjectAction::Create);
    let through_the_team = Grant::Object(object("note"), ObjectAction::Read);
    assert_eq!(
        held(&world, "writer1"),
        BTreeSet::from([direct.clone(), through_the_team.clone()])
    );
    assert!(!held(&world, "writer1").contains(&Grant::Capability("content.publish".into())));
    // Another holder of the direct role does not share the team's permission.
    assert_eq!(held(&world, "writer2"), BTreeSet::from([direct]));
    // A principal with no role holds nothing.
    assert!(held(&world, "newbie").is_empty());

    let world = must(&world, "alpha", "owner", 7_005, joins("press", "writer1"));
    assert!(held(&world, "writer1").contains(&Grant::Capability("content.publish".into())));
    assert!(held(&world, "writer1").contains(&through_the_team));
}

/// Verifies: ADM-150
///
/// A permission on an object type is a permission to act: a role that states one lets its holder
/// do it with no policy naming the role.
#[test]
fn an_object_permission_of_a_role_authorises_the_action() {
    let world = access_world();
    let world = must(&world, "alpha", "owner", 7_000, define_team("readers"));
    let world = must(
        &world,
        "alpha",
        "owner",
        7_001,
        team_gets("readers", "reviewer"),
    );
    let world = must(&world, "alpha", "owner", 7_002, joins("readers", "newbie"));
    let world = must(
        &world,
        "alpha",
        "writer1",
        7_003,
        new_note("Draft", "draft"),
    );
    let read = ReadRequest::Records {
        object: object("note"),
        ids: vec![RecordId(1)],
    };
    let rows_for = |who: &str| {
        world
            .read(&read, &ctx("alpha", user(&format!("alpha:{who}")), 7_004))
            .unwrap()
            .rows
    };
    // No policy lets a reviewer read a draft; the role's permission does.
    assert_eq!(rows_for("newbie").len(), 1);
    assert_eq!(rows_for("writer2").len(), 1, "writers read notes by policy");
    assert_eq!(
        rows_for("stranger").len(),
        1,
        "a moderator holds the permission too"
    );
    assert!(rows_for("nobody").is_empty());
}

/// Verifies: ADM-152
///
/// A principal that manages members still may not assign a role granting a permission it does
/// not hold, whether to a person, to a team, through a team's membership or in an invitation,
/// and may assign one whose permissions it holds.
#[test]
fn assigning_a_role_is_refused_without_one_of_its_permissions() {
    let world = access_world();
    // `alpha:stranger` is a moderator: it manages members, holds people.manage and note read.
    let world = must(&world, "alpha", "owner", 7_000, define_team("press"));
    let world = must(
        &world,
        "alpha",
        "owner",
        7_001,
        team_gets("press", "publisher"),
    );
    let world = must(&world, "alpha", "stranger", 7_002, define_team("desk"));

    let confer_failed = |error: &DomainError, role_name: &str| {
        assert_eq!(error.kind, ErrorKind::Unauthorized);
        let rule = format!("permission:confer:{role_name}");
        assert!(
            error.evidence.iter().any(|c| c.rule == rule && !c.passed),
            "no failed {rule} in {:?}",
            error.evidence
        );
    };

    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        7_003,
        Intent::AssignRole {
            principal: principal_id("alpha:writer2"),
            role: role("publisher"),
        },
    ));
    confer_failed(&error, "publisher");
    assert!(!roles_held_by(&world, "writer2").contains(&role("publisher")));

    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        7_004,
        team_gets("desk", "publisher"),
    ));
    confer_failed(&error, "publisher");

    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        7_005,
        joins("press", "writer2"),
    ));
    confer_failed(&error, "publisher");

    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        7_006,
        Intent::CreateInvitation {
            invitation: InvitationId::new("inv-over").unwrap(),
            address: "ines@example.org".into(),
            roles: BTreeSet::from([role("publisher")]),
        },
    ));
    confer_failed(&error, "publisher");

    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        7_007,
        Intent::AssignRole {
            principal: principal_id("alpha:writer2"),
            role: role("owner"),
        },
    ));
    confer_failed(&error, "owner");

    // A role granting a permission the assigner holds is accepted, to a person and to a team.
    let world = must(
        &world,
        "alpha",
        "stranger",
        7_008,
        Intent::AssignRole {
            principal: principal_id("alpha:writer2"),
            role: role("reviewer"),
        },
    );
    assert!(roles_held_by(&world, "writer2").contains(&role("reviewer")));
    let world = must(
        &world,
        "alpha",
        "stranger",
        7_009,
        team_gets("desk", "reviewer"),
    );
    let world = must(&world, "alpha", "stranger", 7_010, joins("desk", "writer1"));
    assert!(roles_held_by(&world, "writer1").contains(&role("reviewer")));

    // Someone who does not manage members is refused by policy, as before.
    let error = refused(step(
        &world,
        "alpha",
        "writer1",
        7_011,
        Intent::AssignRole {
            principal: principal_id("alpha:writer2"),
            role: role("reviewer"),
        },
    ));
    assert!(error
        .evidence
        .iter()
        .any(|c| !c.passed && c.rule.starts_with("policy:")));

    // The owner holds every permission and may assign anything.
    must(
        &world,
        "alpha",
        "owner",
        7_012,
        Intent::AssignRole {
            principal: principal_id("alpha:writer2"),
            role: role("publisher"),
        },
    );
}

/// A step as a person whose account has the verified address `address` (or none).
fn step_verified(
    world: &World,
    who: &str,
    now: i64,
    address: Option<&str>,
    intent: Intent,
) -> Result<(World, Transition), DomainError> {
    let mut context = ctx("alpha", user(&format!("alpha:{who}")), now);
    context.verified_address = address.map(str::to_string);
    world.step(&intent, &context)
}

/// Verifies: ADM-153
///
/// An invitation grants its roles to the one person who redeems it with the address it names:
/// not to a person whose verified address is another, not to one with no verified address, and
/// not a second time. The address is the one the shell verified for the acting person, in the
/// context; the intent names the invitation and nothing else.
#[test]
fn an_invitation_grants_its_roles_to_the_one_person_with_its_address() {
    let world = access_world();
    let world = must(
        &world,
        "alpha",
        "owner",
        7_000,
        Intent::CreateInvitation {
            invitation: InvitationId::new("inv-ines").unwrap(),
            address: "Ines@Example.org".into(),
            roles: BTreeSet::from([role("writer")]),
        },
    );
    let redeem = || Intent::RedeemInvitation {
        invitation: InvitationId::new("inv-ines").unwrap(),
    };
    // The intent has no field a caller could put an address in: this pattern names them all.
    let Intent::RedeemInvitation { invitation: _ } = redeem() else {
        unreachable!()
    };

    // Another person, whose verified address is another, gets nothing, and is not told which
    // address the invitation names.
    let error = refused(step_verified(
        &world,
        "ines",
        7_001,
        Some("other@example.org"),
        redeem(),
    ));
    assert_eq!(error.kind, ErrorKind::Unauthorized);
    assert!(error
        .evidence
        .iter()
        .any(|c| c.rule == "invitation:address_matches" && !c.passed));
    assert!(!roles_held_by(&world, "ines").contains(&role("writer")));
    assert!(!error.message.contains("ines@example.org"));
    assert!(error
        .evidence
        .iter()
        .all(|c| !c.detail.contains("ines@example.org")));

    // A person whose account has no verified address gets nothing either.
    let error = refused(step_verified(&world, "ines", 7_002, None, redeem()));
    assert_eq!(error.kind, ErrorKind::Unauthorized);
    assert!(error
        .evidence
        .iter()
        .any(|c| c.rule == "invitation:address_matches"
            && !c.passed
            && c.detail.contains("no verified address")));
    assert!(!roles_held_by(&world, "ines").contains(&role("writer")));

    // The person with the named address gets the roles.
    let (world, outcome) =
        step_verified(&world, "ines", 7_003, Some("ines@example.org"), redeem()).unwrap();
    assert_eq!(outcome.output, Output::Member);
    assert!(roles_held_by(&world, "ines").contains(&role("writer")));

    // A second redemption is refused, by the same person and by another with the address.
    for who in ["ines", "other"] {
        let error = refused(step_verified(
            &world,
            who,
            7_004,
            Some("ines@example.org"),
            redeem(),
        ));
        assert!(error
            .evidence
            .iter()
            .any(|c| c.rule == "invitation:redeemed_once" && !c.passed));
    }
    assert!(!roles_held_by(&world, "other").contains(&role("writer")));

    // An anonymous visitor redeems nothing, whatever address the context carries.
    let mut context = ctx("alpha", Principal::Anonymous, 7_005);
    context.verified_address = Some("ines@example.org".into());
    let error = refused(world.step(&redeem(), &context));
    assert_eq!(error.kind, ErrorKind::Unauthorized);
}

/// Verifies: ADM-152
///
/// A role may be conferred only by a principal that holds every permission and capability it
/// grants: one it lacks among several refuses the assignment, each kind of permission on its
/// own, and what the principal holds through its roles and its teams' roles together counts.
#[test]
fn assigning_a_role_is_refused_unless_the_assigner_holds_all_of_its_permissions() {
    let role_def = |id: &str| {
        SchemaChange::DefineRole(RoleDef {
            id: role(id),
            label: id.into(),
        })
    };
    let permissions =
        |id: &str, permissions: Vec<Permission>| SchemaChange::DefineRolePermissions {
            role: role(id),
            permissions,
        };
    // `alpha:stranger` is a moderator: it manages members, holds people.manage and reads notes.
    let world = must(
        &access_world(),
        "alpha",
        "owner",
        8_000,
        Intent::ChangeSchema {
            changes: vec![
                role_def("reader_editor"),
                role_def("reader_publisher"),
                role_def("manager_reader"),
                role_def("note_rw"),
                permissions(
                    "reader_editor",
                    vec![object_permission(
                        "note",
                        &[ObjectAction::Read, ObjectAction::Update],
                    )],
                ),
                permissions(
                    "reader_publisher",
                    vec![
                        object_permission("note", &[ObjectAction::Read]),
                        capability("content.publish"),
                    ],
                ),
                permissions(
                    "manager_reader",
                    vec![
                        object_permission("note", &[ObjectAction::Read]),
                        capability("people.manage"),
                    ],
                ),
                permissions(
                    "note_rw",
                    vec![object_permission(
                        "note",
                        &[ObjectAction::Read, ObjectAction::Create],
                    )],
                ),
            ],
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    let assign = |role_name: &str| Intent::AssignRole {
        principal: principal_id("alpha:writer2"),
        role: role(role_name),
    };
    let confer_failed = |error: &DomainError, role_name: &str, lacking: &str| {
        assert_eq!(error.kind, ErrorKind::Unauthorized);
        let rule = format!("permission:confer:{role_name}");
        let check = error
            .evidence
            .iter()
            .find(|c| c.rule == rule && !c.passed)
            .unwrap_or_else(|| panic!("no failed {rule} in {:?}", error.evidence));
        assert!(
            check.detail.contains(lacking),
            "{} does not name {lacking}",
            check.detail
        );
        assert!(
            check.detail.contains("lacks 1 of the 2"),
            "{}",
            check.detail
        );
    };

    // An object permission among two is missing: the one it holds does not make up for it.
    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        8_001,
        assign("reader_editor"),
    ));
    confer_failed(&error, "reader_editor", "note:update");
    assert!(!roles_held_by(&world, "writer2").contains(&role("reader_editor")));

    // A capability among two is missing, though the object permission with it is held.
    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        8_002,
        assign("reader_publisher"),
    ));
    confer_failed(&error, "reader_publisher", "capability content.publish");

    // The same holds to a team, in a team's membership and in an invitation.
    let world = must(&world, "alpha", "stranger", 8_003, define_team("desk"));
    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        8_004,
        team_gets("desk", "reader_publisher"),
    ));
    confer_failed(&error, "reader_publisher", "capability content.publish");
    let world = must(&world, "alpha", "owner", 8_005, define_team("crew"));
    let world = must(
        &world,
        "alpha",
        "owner",
        8_006,
        team_gets("crew", "reader_editor"),
    );
    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        8_007,
        joins("crew", "writer2"),
    ));
    confer_failed(&error, "reader_editor", "note:update");
    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        8_008,
        Intent::CreateInvitation {
            invitation: InvitationId::new("inv-partial").unwrap(),
            address: "ines@example.org".into(),
            roles: BTreeSet::from([role("reader_editor")]),
        },
    ));
    confer_failed(&error, "reader_editor", "note:update");

    // Holding every permission is enough: the object permission and the capability are held.
    let world = must(&world, "alpha", "stranger", 8_009, assign("manager_reader"));
    assert!(roles_held_by(&world, "writer2").contains(&role("manager_reader")));

    // What the assigner holds through several roles and teams counts together: it reads notes
    // as a moderator and creates them through a team that holds the writer role.
    let error = refused(step(&world, "alpha", "stranger", 8_010, assign("note_rw")));
    confer_failed(&error, "note_rw", "note:create");
    let world = must(&world, "alpha", "owner", 8_011, team_gets("crew", "writer"));
    let world = must(&world, "alpha", "owner", 8_012, joins("crew", "stranger"));
    let world = must(&world, "alpha", "stranger", 8_013, assign("note_rw"));
    assert!(roles_held_by(&world, "writer2").contains(&role("note_rw")));

    // The owner holds every permission and confers any role.
    must(&world, "alpha", "owner", 8_014, assign("reader_publisher"));
}

/// Verifies: ADM-151
///
/// A capability is held only through a role that names it, assigned to the principal or held by
/// a team it belongs to: this is what the generated authorizer fills a principal's tenant roles
/// and teams from.
#[test]
fn a_capability_is_held_only_through_a_role_that_names_it() {
    let capabilities_of = |world: &World, who: &str| {
        strata_kernel::rules::held_capabilities(
            &world.tenants[&tenant_id("alpha")],
            &user(&format!("alpha:{who}")),
        )
    };
    let names = |items: &[&str]| -> BTreeSet<String> {
        items.iter().map(|item| item.to_string()).collect()
    };
    let world = access_world();
    // The moderator's role names people.manage; its object permission is no capability.
    assert_eq!(
        capabilities_of(&world, "stranger"),
        names(&["people.manage"])
    );
    // A principal with no role holds none, and `reviewer`, with only an object permission,
    // gives none.
    assert_eq!(capabilities_of(&world, "writer1"), names(&[]));
    let world = must(
        &world,
        "alpha",
        "owner",
        8_100,
        Intent::AssignRole {
            principal: principal_id("alpha:writer1"),
            role: role("reviewer"),
        },
    );
    assert_eq!(capabilities_of(&world, "writer1"), names(&[]));
    // Through a team: the team's role names content.publish, and its members hold it only while
    // they belong.
    let world = must(&world, "alpha", "owner", 8_101, define_team("press"));
    let world = must(
        &world,
        "alpha",
        "owner",
        8_102,
        team_gets("press", "publisher"),
    );
    assert_eq!(capabilities_of(&world, "writer1"), names(&[]));
    let world = must(&world, "alpha", "owner", 8_103, joins("press", "writer1"));
    assert_eq!(
        capabilities_of(&world, "writer1"),
        names(&["content.publish"])
    );
    // Another principal, in no team, holds nothing of it.
    assert_eq!(capabilities_of(&world, "writer2"), names(&[]));
    let world = must(
        &world,
        "alpha",
        "owner",
        8_104,
        Intent::RemoveTeamMember {
            team: team("press"),
            principal: principal_id("alpha:writer1"),
        },
    );
    assert_eq!(capabilities_of(&world, "writer1"), names(&[]));
}

// Conferral through policies, revocation, owners through teams, invitation lifetime, and
// refusals that do not reveal what exists (ADM-1080 to ADM-1085, KRN-1080).

/// `access_world` with two more roles that hold no permission of their own but are empowered by
/// a policy: `admin2` manages the schema, `reader2` reads every object type.
fn policy_world() -> World {
    must(
        &access_world(),
        "alpha",
        "owner",
        7_500,
        Intent::ChangeSchema {
            changes: vec![
                SchemaChange::DefineRole(RoleDef {
                    id: role("admin2"),
                    label: "Admin two".into(),
                }),
                SchemaChange::DefineRole(RoleDef {
                    id: role("reader2"),
                    label: "Reader two".into(),
                }),
                SchemaChange::DefinePolicy(PolicyDef {
                    id: PolicyId::new("admin2_manage_schema").unwrap(),
                    role: role("admin2"),
                    resource: Resource::Schema,
                    actions: BTreeSet::from([Action::Manage]),
                    condition: None,
                }),
                SchemaChange::DefinePolicy(PolicyDef {
                    id: PolicyId::new("reader2_read_all").unwrap(),
                    role: role("reader2"),
                    resource: Resource::AnyObject,
                    actions: BTreeSet::from([Action::Read]),
                    condition: None,
                }),
            ],
            expected_version: None,
            existing: ExistingData::Reject,
        },
    )
}

fn assert_confer_refused(error: &DomainError, role_name: &str) {
    assert_eq!(error.kind, ErrorKind::Unauthorized);
    let rule = format!("permission:confer:{role_name}");
    assert!(
        error.evidence.iter().any(|c| c.rule == rule && !c.passed),
        "no failed {rule} in {:?}",
        error.evidence
    );
}

/// Verifies: ADM-1085
///
/// A role that holds no permission but is empowered by a policy is conferred only by a principal
/// that holds that power: not by assigning it, not by adding someone to a team that holds it,
/// not by giving it to a team, not in an invitation. The owner confers it.
#[test]
fn a_role_empowered_by_a_policy_is_not_conferred_without_that_power() {
    let world = policy_world();
    // `alpha:stranger` manages members; it does not manage the schema.
    let world = must(&world, "alpha", "owner", 7_501, define_team("ops"));
    let world = must(&world, "alpha", "owner", 7_502, team_gets("ops", "admin2"));
    let world = must(&world, "alpha", "stranger", 7_503, define_team("desk"));

    for role_name in ["admin2", "reader2"] {
        let error = refused(step(
            &world,
            "alpha",
            "stranger",
            7_504,
            Intent::AssignRole {
                principal: principal_id("alpha:writer2"),
                role: role(role_name),
            },
        ));
        assert_confer_refused(&error, role_name);
        assert!(!roles_held_by(&world, "writer2").contains(&role(role_name)));
    }

    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        7_505,
        joins("ops", "writer2"),
    ));
    assert_confer_refused(&error, "admin2");

    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        7_506,
        team_gets("desk", "admin2"),
    ));
    assert_confer_refused(&error, "admin2");

    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        7_507,
        Intent::CreateInvitation {
            invitation: InvitationId::new("inv-policy").unwrap(),
            address: "ines@example.org".into(),
            roles: BTreeSet::from([role("admin2")]),
        },
    ));
    assert_confer_refused(&error, "admin2");

    // The owner holds every power and confers the role.
    let world = must(
        &world,
        "alpha",
        "owner",
        7_508,
        Intent::AssignRole {
            principal: principal_id("alpha:writer2"),
            role: role("admin2"),
        },
    );
    assert!(roles_held_by(&world, "writer2").contains(&role("admin2")));
}

/// Verifies: ADM-1080
///
/// Taking a role away asks what giving it asks: a member manager who could not confer a role
/// cannot revoke it from a person, revoke it from a team or take a person out of a team that
/// holds it. The same manager revokes a role it could confer.
#[test]
fn revoking_a_role_needs_the_authority_to_confer_it() {
    let world = access_world();
    let world = must(
        &world,
        "alpha",
        "owner",
        7_600,
        Intent::AssignRole {
            principal: principal_id("alpha:writer2"),
            role: role("publisher"),
        },
    );
    let world = must(
        &world,
        "alpha",
        "owner",
        7_601,
        Intent::AssignRole {
            principal: principal_id("alpha:writer2"),
            role: role("reviewer"),
        },
    );
    let world = must(&world, "alpha", "owner", 7_602, define_team("press"));
    let world = must(
        &world,
        "alpha",
        "owner",
        7_603,
        team_gets("press", "publisher"),
    );
    let world = must(&world, "alpha", "owner", 7_604, joins("press", "writer1"));

    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        7_605,
        Intent::RevokeRole {
            principal: principal_id("alpha:writer2"),
            role: role("publisher"),
        },
    ));
    assert_confer_refused(&error, "publisher");
    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        7_606,
        Intent::RevokeTeamRole {
            team: team("press"),
            role: role("publisher"),
        },
    ));
    assert_confer_refused(&error, "publisher");
    let error = refused(step(
        &world,
        "alpha",
        "stranger",
        7_607,
        Intent::RemoveTeamMember {
            team: team("press"),
            principal: principal_id("alpha:writer1"),
        },
    ));
    assert_confer_refused(&error, "publisher");
    assert!(roles_held_by(&world, "writer2").contains(&role("publisher")));
    assert!(roles_held_by(&world, "writer1").contains(&role("publisher")));

    // A role the manager could confer, it may revoke.
    let world = must(
        &world,
        "alpha",
        "stranger",
        7_608,
        Intent::RevokeRole {
            principal: principal_id("alpha:writer2"),
            role: role("reviewer"),
        },
    );
    assert!(!roles_held_by(&world, "writer2").contains(&role("reviewer")));
    // The owner revokes the rest.
    must(
        &world,
        "alpha",
        "owner",
        7_609,
        Intent::RevokeTeamRole {
            team: team("press"),
            role: role("publisher"),
        },
    );
}

/// Verifies: ADM-1081
///
/// Owners are counted in effect, through teams as well as directly: no revocation, of the role
/// or of a team's role or of a team member, leaves the tenant with none.
#[test]
fn no_sequence_of_revocations_leaves_a_tenant_without_an_owner() {
    let world = notes_world();
    let world = must(&world, "alpha", "owner", 7_700, define_team("board"));
    let world = must(&world, "alpha", "owner", 7_701, team_gets("board", "owner"));
    let world = must(&world, "alpha", "owner", 7_702, joins("board", "writer1"));
    // The direct owner steps down: the board still holds an owner.
    let world = must(
        &world,
        "alpha",
        "owner",
        7_703,
        Intent::RevokeRole {
            principal: principal_id("alpha:owner"),
            role: role("owner"),
        },
    );
    assert!(roles_held_by(&world, "writer1").contains(&role("owner")));

    // The one owner left, through the team, cannot leave the team or take the role from it.
    let error = refused(step(
        &world,
        "alpha",
        "writer1",
        7_704,
        Intent::RemoveTeamMember {
            team: team("board"),
            principal: principal_id("alpha:writer1"),
        },
    ));
    assert_eq!(error.kind, ErrorKind::InvalidIntent);
    let error = refused(step(
        &world,
        "alpha",
        "writer1",
        7_705,
        Intent::RevokeTeamRole {
            team: team("board"),
            role: role("owner"),
        },
    ));
    assert_eq!(error.kind, ErrorKind::InvalidIntent);

    // With another owner in place, leaving is allowed.
    let world = must(
        &world,
        "alpha",
        "writer1",
        7_706,
        Intent::AssignRole {
            principal: principal_id("alpha:writer2"),
            role: role("owner"),
        },
    );
    let world = must(
        &world,
        "alpha",
        "writer1",
        7_707,
        Intent::RemoveTeamMember {
            team: team("board"),
            principal: principal_id("alpha:writer1"),
        },
    );
    assert!(!roles_held_by(&world, "writer1").contains(&role("owner")));
    assert!(roles_held_by(&world, "writer2").contains(&role("owner")));
}

fn invite(id: &str, address: &str, role_name: &str) -> Intent {
    Intent::CreateInvitation {
        invitation: InvitationId::new(id).unwrap(),
        address: address.into(),
        roles: BTreeSet::from([role(role_name)]),
    }
}

fn redeem_invitation(id: &str) -> Intent {
    Intent::RedeemInvitation {
        invitation: InvitationId::new(id).unwrap(),
    }
}

/// Verifies: ADM-1082
///
/// An invitation is redeemable until its expiry and not after.
#[test]
fn an_invitation_expires() {
    let world = must(
        &access_world(),
        "alpha",
        "owner",
        7_800,
        invite("inv-late", "ines@example.org", "writer"),
    );
    let fourteen_days = 14 * 24 * 60 * 60 * 1000;
    let error = refused(step_verified(
        &world,
        "ines",
        7_800 + fourteen_days,
        Some("ines@example.org"),
        redeem_invitation("inv-late"),
    ));
    assert_eq!(error.kind, ErrorKind::InvalidIntent);
    assert!(error
        .evidence
        .iter()
        .any(|c| c.rule == "invitation:not_expired" && !c.passed));
    assert!(!roles_held_by(&world, "ines").contains(&role("writer")));

    let (world, _) = step_verified(
        &world,
        "ines",
        7_800 + fourteen_days - 1,
        Some("ines@example.org"),
        redeem_invitation("inv-late"),
    )
    .unwrap();
    assert!(roles_held_by(&world, "ines").contains(&role("writer")));
}

/// Verifies: ADM-1083
///
/// An invitation is revoked by a principal who could confer its roles, once, and a revoked
/// invitation is never redeemable.
#[test]
fn an_invitation_can_be_revoked() {
    let world = must(
        &access_world(),
        "alpha",
        "owner",
        7_900,
        invite("inv-gone", "ines@example.org", "writer"),
    );
    let revoke = || Intent::RevokeInvitation {
        invitation: InvitationId::new("inv-gone").unwrap(),
    };
    // A member manager who could not confer `writer` cannot revoke it either.
    let error = refused(step(&world, "alpha", "stranger", 7_901, revoke()));
    assert_confer_refused(&error, "writer");
    // Someone who does not manage members is refused by policy.
    let error = refused(step(&world, "alpha", "writer1", 7_902, revoke()));
    assert_eq!(error.kind, ErrorKind::Unauthorized);

    let world = must(&world, "alpha", "owner", 7_903, revoke());
    let error = refused(step(&world, "alpha", "owner", 7_904, revoke()));
    assert_eq!(error.kind, ErrorKind::InvalidIntent);
    let error = refused(step_verified(
        &world,
        "ines",
        7_905,
        Some("ines@example.org"),
        redeem_invitation("inv-gone"),
    ));
    assert_eq!(error.kind, ErrorKind::InvalidIntent);
    assert!(error
        .evidence
        .iter()
        .any(|c| c.rule == "invitation:not_revoked" && !c.passed));
    assert!(!roles_held_by(&world, "ines").contains(&role("writer")));
}

/// Verifies: ADM-1084
///
/// Redeeming asks again whether the issuer may still issue the invitation: one issued by a
/// member manager who has since lost the role grants nothing.
#[test]
fn redeeming_asks_again_for_the_authority_of_the_issuer() {
    let world = must(
        &access_world(),
        "alpha",
        "stranger",
        8_000,
        invite("inv-moderated", "ines@example.org", "reviewer"),
    );
    // While the issuer still manages members the invitation is good (checked on a copy).
    step_verified(
        &world,
        "ines",
        8_001,
        Some("ines@example.org"),
        redeem_invitation("inv-moderated"),
    )
    .unwrap();
    let world = must(
        &world,
        "alpha",
        "owner",
        8_002,
        Intent::RevokeRole {
            principal: principal_id("alpha:stranger"),
            role: role("moderator"),
        },
    );
    let error = refused(step_verified(
        &world,
        "ines",
        8_003,
        Some("ines@example.org"),
        redeem_invitation("inv-moderated"),
    ));
    assert_eq!(error.kind, ErrorKind::Unauthorized);
    assert!(error
        .evidence
        .iter()
        .any(|c| c.rule == "invitation:issuer_authority" && !c.passed));
    assert!(!roles_held_by(&world, "ines").contains(&role("reviewer")));
}

/// Verifies: KRN-1080
///
/// A caller who may not act on a record or an object type is refused the same way whether the
/// record or the type exists or not, so a refusal tells nothing about what exists.
#[test]
fn an_unauthorised_caller_cannot_tell_a_missing_record_from_an_existing_one() {
    let world = must(
        &access_world(),
        "alpha",
        "writer1",
        8_100,
        new_note("First", "first"),
    );
    // `alpha:stranger` reads notes and may do nothing else with them.
    let delete = |object_name: &str, id: u64| Intent::DeleteRecord {
        object: object(object_name),
        id: RecordId(id),
        expected_version: None,
    };
    let update = |object_name: &str, id: u64| Intent::UpdateRecord {
        object: object(object_name),
        id: RecordId(id),
        fields: BTreeMap::from([(field("title"), text("x"))]),
        expected_version: None,
    };
    let create = |object_name: &str| Intent::CreateRecord {
        object: object(object_name),
        fields: BTreeMap::new(),
    };
    let pairs = [
        (update("note", 1), update("note", 999)),
        (delete("note", 1), delete("note", 999)),
        (update("note", 1), update("ghost", 1)),
        (delete("note", 1), delete("ghost", 1)),
        (create("note"), create("ghost")),
    ];
    // What a refusal says, with the name the caller supplied for the missing type put back to the
    // existing one: its message and the checks that failed.
    let said = |error: &DomainError| {
        (
            error.kind,
            error.message.replace("ghost", "note"),
            error
                .evidence
                .iter()
                .filter(|c| !c.passed)
                .map(|c| (c.rule.clone(), c.detail.replace("ghost", "note")))
                .collect::<Vec<_>>(),
        )
    };
    for (n, (present, absent)) in pairs.into_iter().enumerate() {
        let a = refused(step(&world, "alpha", "stranger", 8_101 + n as i64, present));
        let b = refused(step(&world, "alpha", "stranger", 8_101 + n as i64, absent));
        assert_eq!(a.kind, ErrorKind::Unauthorized, "pair {n}");
        assert_eq!(said(&a), said(&b), "pair {n}");
    }
    // One who may act on the type is told what is missing.
    let error = refused(step(&world, "alpha", "owner", 8_200, update("note", 999)));
    assert_eq!(error.kind, ErrorKind::UnknownRecord);
    let error = refused(step(&world, "alpha", "owner", 8_201, update("ghost", 1)));
    assert_eq!(error.kind, ErrorKind::UnknownObject);
}
