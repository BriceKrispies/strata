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

/// Verifies: KRN-007
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

/// Verifies: KRN-008
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

/// Verifies: KRN-009
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

/// Verifies: KRN-010
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
