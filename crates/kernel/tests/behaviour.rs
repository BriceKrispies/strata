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
            .unwrap_or_else(|e| panic!("{name} was refused: {e}"));
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

// Roles as sets of permissions, teams and invitations (ADM-150 to ADM-153).

fn object_permission(name: &str, verbs: &[Verb]) -> Permission {
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
                permissions("writer", vec![object_permission("note", &[Verb::Create])]),
                permissions("reviewer", vec![object_permission("note", &[Verb::Read])]),
                permissions("publisher", vec![capability("content.publish")]),
                permissions(
                    "moderator",
                    vec![
                        capability("people.manage"),
                        object_permission("note", &[Verb::Read]),
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
        BTreeSet::from([Grant::Object(object("note"), Verb::Create)])
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

    let direct = Grant::Object(object("note"), Verb::Create);
    let through_the_team = Grant::Object(object("note"), Verb::Read);
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
/// A principal that manages members still may not assign a role granting none of the
/// permissions it holds, whether to a person, to a team, through a team's membership or in an
/// invitation, and may assign one granting a permission it holds.
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

/// Verifies: ADM-153
///
/// An invitation grants its roles to the one person who redeems it with the address it names:
/// not to a person with another address, and not a second time.
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
    let redeem = |address: &str| Intent::RedeemInvitation {
        invitation: InvitationId::new("inv-ines").unwrap(),
        verified_address: address.into(),
    };

    // Another person, with another address, gets nothing.
    let error = refused(step(
        &world,
        "alpha",
        "ines",
        7_001,
        redeem("other@example.org"),
    ));
    assert_eq!(error.kind, ErrorKind::Unauthorized);
    assert!(error
        .evidence
        .iter()
        .any(|c| c.rule == "invitation:address_matches" && !c.passed));
    assert!(!roles_held_by(&world, "ines").contains(&role("writer")));
    assert!(!error.message.contains("ines@example.org"));

    // The person with the named address gets the roles.
    let (world, outcome) =
        step(&world, "alpha", "ines", 7_002, redeem("ines@example.org")).unwrap();
    assert_eq!(outcome.output, Output::Member);
    assert!(roles_held_by(&world, "ines").contains(&role("writer")));

    // A second redemption is refused, by the same person and by another with the address.
    for who in ["ines", "other"] {
        let error = refused(step(
            &world,
            "alpha",
            who,
            7_003,
            redeem("ines@example.org"),
        ));
        assert!(error
            .evidence
            .iter()
            .any(|c| c.rule == "invitation:redeemed_once" && !c.passed));
    }
    assert!(!roles_held_by(&world, "other").contains(&role("writer")));

    // An anonymous visitor redeems nothing.
    let error = refused(world.step(
        &redeem("ines@example.org"),
        &ctx("alpha", Principal::Anonymous, 7_004),
    ));
    assert_eq!(error.kind, ErrorKind::Unauthorized);
}
