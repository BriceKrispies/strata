//! Invoking a tenant operation (WG-5100; D-6546 to D-6549, OPR-1540 to OPR-1554): who may
//! invoke one, what it may do, that it commits all of its steps or none, and each refusal.
//! The repeat of a key is the store's (crates/backend/tests/operations.rs).

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::operations::{posting_key, result_of};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_model_core::types::{LedgerAmount, LedgerUnit, LedgerUnitDecl};

// ---------------------------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------------------------

fn op_id(s: &str) -> TenantOperationId {
    TenantOperationId::new(s).expect("operation id")
}

fn param(s: &str) -> Expr {
    Expr::Param(s.to_string())
}

fn int(n: i64) -> Expr {
    Expr::Lit(Value::Int(n))
}

fn input(name: &str, ty: FieldType) -> TenantOperationInput {
    TenantOperationInput {
        name: name.into(),
        ty,
        required: true,
        label: None,
    }
}

fn note_ref() -> FieldType {
    FieldType::Ref {
        target: object("note"),
        on_delete: OnDelete::Restrict,
    }
}

/// Copy a published note (the subject) under a new slug, then rank the copy: the second step
/// reads the record the first one made as `param.copy`.
fn feature() -> TenantOperationDef {
    TenantOperationDef {
        id: op_id("feature"),
        label: "Feature".into(),
        inputs: vec![input("note", note_ref()), input("slug", FieldType::Str)],
        invokers: TenantOperationInvokers {
            signed_in: true,
            roles: BTreeSet::new(),
        },
        subject: Some(TenantOperationSubject {
            object: object("note"),
            input: "note".into(),
        }),
        conditions: vec![TenantOperationCondition {
            id: "published".into(),
            expr: eq(var(Scope::Record, "status"), lit("published")),
            error: "not-published".into(),
            message: "Only a published note is featured.".into(),
        }],
        steps: vec![
            TenantOperationStep::CreateRecord {
                id: "copy".into(),
                object: object("note"),
                fields: BTreeMap::from([
                    (field("title"), var(Scope::Record, "title")),
                    (field("slug"), param("slug")),
                ]),
            },
            TenantOperationStep::UpdateRecord {
                id: "rank".into(),
                object: object("note"),
                record: param("copy"),
                fields: BTreeMap::from([(field("rank"), int(7))]),
            },
        ],
    }
}

/// [`feature`] whose second step writes a title longer than the note's constraint allows.
fn feature_failing_late() -> TenantOperationDef {
    let mut op = feature();
    op.id = op_id("feature_late");
    if let TenantOperationStep::UpdateRecord { fields, .. } = &mut op.steps[1] {
        fields.insert(field("title"), lit("A title far longer than twenty-four"));
    }
    op
}

/// [`feature`] that only editors may invoke.
fn editors_feature() -> TenantOperationDef {
    let mut op = feature();
    op.id = op_id("editors_feature");
    op.invokers = TenantOperationInvokers {
        signed_in: false,
        roles: BTreeSet::from([role("editor")]),
    };
    op
}

/// Delete the note it names.
fn discard() -> TenantOperationDef {
    TenantOperationDef {
        id: op_id("discard"),
        label: "Discard".into(),
        inputs: vec![input("note", note_ref())],
        invokers: TenantOperationInvokers {
            signed_in: true,
            roles: BTreeSet::new(),
        },
        subject: Some(TenantOperationSubject {
            object: object("note"),
            input: "note".into(),
        }),
        conditions: vec![],
        steps: vec![TenantOperationStep::DeleteRecord {
            id: "gone".into(),
            object: object("note"),
            record: param("note"),
        }],
    }
}

/// Count a point for the invoker in the ledger.
fn count() -> TenantOperationDef {
    TenantOperationDef {
        id: op_id("count"),
        label: "Count".into(),
        inputs: vec![],
        invokers: TenantOperationInvokers {
            signed_in: true,
            roles: BTreeSet::new(),
        },
        subject: None,
        conditions: vec![],
        steps: vec![TenantOperationStep::PostLedger {
            id: "take".into(),
            owner: TenantOperationPostingOwner::Invoker,
            unit: LedgerUnit::new("points").unwrap(),
            scope: None,
            amount: int(1),
            reason: "counted".into(),
        }],
    }
}

fn run(
    world: &World,
    who: Principal,
    now: i64,
    intent: &Intent,
) -> Result<(World, Transition), DomainError> {
    world.step(intent, &ctx("alpha", who, now))
}

fn as_user(who: &str) -> Principal {
    user(&format!("alpha:{who}"))
}

fn must(world: &World, who: &str, now: i64, intent: Intent) -> World {
    run(world, as_user(who), now, &intent).expect("accepted").0
}

fn invoke(op: &str, inputs: &[(&str, Value)], key: &str) -> Intent {
    Intent::InvokeOperation {
        operation: op_id(op),
        inputs: inputs
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone()))
            .collect(),
        idempotency_key: key.to_string(),
    }
}

fn feature_note(note: u64, slug: &str) -> Intent {
    invoke(
        "feature",
        &[("note", Value::Ref(RecordId(note))), ("slug", text(slug))],
        "k-1",
    )
}

/// The notes world of alpha with the operations above, defined by the owner; a role `member`
/// that grants nothing, held by `alpha:stranger`; note 1 published by the editor and note 2 a
/// draft of writer1.
fn world() -> World {
    let mut world = must(
        &notes_world(),
        "owner",
        5_000,
        Intent::ChangeSchema {
            changes: vec![
                SchemaChange::DefineRole(RoleDef {
                    id: role("member"),
                    label: "Member".into(),
                }),
                SchemaChange::DefineOperation(feature()),
                SchemaChange::DefineOperation(feature_failing_late()),
                SchemaChange::DefineOperation(editors_feature()),
                SchemaChange::DefineOperation(discard()),
                SchemaChange::DeclareLedgerUnit(LedgerUnitDecl {
                    unit: LedgerUnit::new("points").unwrap(),
                    description: "Points".into(),
                    expiry: None,
                }),
                SchemaChange::DefineOperation(count()),
            ],
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    world = must(
        &world,
        "owner",
        5_001,
        Intent::AssignRole {
            principal: principal_id("alpha:stranger"),
            role: role("member"),
        },
    );
    world = must(
        &world,
        "editor",
        5_002,
        new_note("Field notes", "field-notes"),
    );
    world = must(
        &world,
        "editor",
        5_003,
        Intent::UpdateRecord {
            object: object("note"),
            id: RecordId(1),
            fields: BTreeMap::from([(field("status"), text("published"))]),
            expected_version: None,
        },
    );
    must(&world, "writer1", 5_004, new_note("Draft", "draft"))
}

fn tenant(world: &World) -> &TenantState {
    &world.tenants[&tenant_id("alpha")]
}

fn note(world: &World, id: u64) -> Option<&Record> {
    tenant(world)
        .record(&object("note"), RecordId(id))
        .map(|record| record.as_ref())
}

/// The refusal of `intent` as `who`, with the world left as it was.
fn refused(world: &World, who: Principal, intent: &Intent) -> DomainError {
    run(world, who, 9_000, intent).expect_err("refused")
}

fn failed_rules(error: &DomainError) -> Vec<&str> {
    error
        .evidence
        .iter()
        .filter(|check| !check.passed)
        .map(|check| check.rule.as_str())
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Acting with the operation's authority.
// ---------------------------------------------------------------------------------------------

// Verifies: OPR-1546
// Verifies: OPR-1548
#[test]
fn a_member_invokes_without_holding_the_powers_its_steps_use() {
    let world = world();
    // The member may not create a note directly.
    let direct = refused(&world, as_user("stranger"), &new_note("Mine", "mine"));
    assert_eq!(direct.kind, ErrorKind::Unauthorized);

    let (after, outcome) = run(
        &world,
        as_user("stranger"),
        9_000,
        &feature_note(1, "featured"),
    )
    .expect("the invocation is accepted");
    let copy = note(&after, 3).expect("the copy exists");
    assert_eq!(copy.created_by, Some(principal_id("alpha:stranger")));
    assert_eq!(copy.get(&field("title")), &text("Field notes"));
    assert_eq!(copy.get(&field("rank")), &Value::Int(7));
    assert!(outcome
        .evidence
        .iter()
        .any(|check| check.passed && check.rule == "operation:feature:power"));
    assert_eq!(
        outcome.output,
        Output::Operation {
            operation: op_id("feature"),
            created: vec![RecordId(3)],
            changed: vec![],
            deleted: vec![],
            entries: vec![],
        }
    );
}

// Verifies: OPR-1543
#[test]
fn a_later_step_reads_what_an_earlier_step_made() {
    let world = world();
    let (_, outcome) = run(
        &world,
        as_user("stranger"),
        9_000,
        &feature_note(1, "featured"),
    )
    .expect("accepted");
    // One delta, the creation and then the update of the copy, with an event each in step order.
    assert_eq!(outcome.delta.ops.len(), 2);
    let bodies: Vec<&EventBody> = outcome.events.iter().map(|event| &event.body).collect();
    assert!(matches!(bodies[0], EventBody::RecordCreated { record } if record.version == 1));
    assert!(matches!(
        bodies[1],
        EventBody::RecordUpdated { before, after, .. } if before.version == 1 && after.version == 2
    ));
    assert_eq!(outcome.events[0].seq + 1, outcome.events[1].seq);
}

#[test]
fn a_member_cannot_make_the_operation_do_more_than_its_definition() {
    let world = world();
    // No input outside the definition, so no field the definition does not write.
    let error = refused(
        &world,
        as_user("stranger"),
        &invoke(
            "feature",
            &[
                ("note", Value::Ref(RecordId(1))),
                ("slug", text("featured")),
                ("status", text("published")),
            ],
            "k-1",
        ),
    );
    assert_eq!(error.kind, ErrorKind::UnknownField);
    assert_eq!(failed_rules(&error), vec!["input:feature.status"]);

    // What it writes is what the definition writes, and the rest is the type's defaults.
    let (after, _) = run(
        &world,
        as_user("stranger"),
        9_000,
        &feature_note(1, "featured"),
    )
    .expect("accepted");
    let written: BTreeSet<&str> = note(&after, 3)
        .unwrap()
        .fields
        .keys()
        .map(|f| f.as_str())
        .collect();
    assert_eq!(written, BTreeSet::from(["rank", "slug", "status", "title"]));

    // An operation that names its invokers by role is refused to everyone else.
    let error = refused(
        &world,
        as_user("stranger"),
        &invoke(
            "editors_feature",
            &[("note", Value::Ref(RecordId(1))), ("slug", text("x"))],
            "k-1",
        ),
    );
    assert_eq!(error.kind, ErrorKind::Unauthorized);
    assert_eq!(
        failed_rules(&error),
        vec!["operation:editors_feature:invoker"]
    );
    run(
        &world,
        as_user("editor"),
        9_000,
        &invoke(
            "editors_feature",
            &[("note", Value::Ref(RecordId(1))), ("slug", text("x"))],
            "k-1",
        ),
    )
    .expect("an editor is named");
}

#[test]
fn a_delete_step_deletes_as_the_operation() {
    let world = world();
    let (after, outcome) = run(
        &world,
        as_user("stranger"),
        9_000,
        &invoke("discard", &[("note", Value::Ref(RecordId(1)))], "k-1"),
    )
    .expect("accepted");
    assert!(note(&after, 1).is_none());
    assert!(matches!(
        outcome.events[0].body,
        EventBody::RecordDeleted { .. }
    ));
    let result = result_of(&outcome.output, outcome.delta.new_revision, false).unwrap();
    assert_eq!(result.deleted, vec![RecordId(1)]);
    assert!(!result.replayed);
}

// ---------------------------------------------------------------------------------------------
// All or nothing.
// ---------------------------------------------------------------------------------------------

// Verifies: OPR-1541
// Verifies: OPR-1542
#[test]
fn a_refused_later_step_commits_nothing_of_the_earlier_ones() {
    let world = world();
    let error = refused(
        &world,
        as_user("stranger"),
        &invoke(
            "feature_late",
            &[
                ("note", Value::Ref(RecordId(1))),
                ("slug", text("featured")),
            ],
            "k-1",
        ),
    );
    assert_eq!(error.kind, ErrorKind::ConstraintViolated);
    assert!(
        error.message.starts_with("step rank: "),
        "{}",
        error.message
    );
    assert!(failed_rules(&error).contains(&"operation:feature_late:step:rank"));
    // The first step ran and was decided, then thrown away with the rest.
    assert!(error
        .evidence
        .iter()
        .any(|check| check.passed && check.rule == "workflow:note_status:initial"));
    // A refusal is a result, not a world: nothing of the copy exists.
    assert!(note(&world, 3).is_none());
}

// Verifies: OPR-1542
#[test]
fn a_false_condition_refuses_with_its_code_and_commits_nothing() {
    let world = world();
    // writer1 reads its draft, so the subject is found; the condition then refuses.
    let error = refused(&world, as_user("writer1"), &feature_note(2, "featured"));
    assert_eq!(error.kind, ErrorKind::ConstraintViolated);
    assert_eq!(
        error.message,
        "not-published: Only a published note is featured."
    );
    assert_eq!(
        failed_rules(&error),
        vec!["operation:feature:condition:published"]
    );
}

#[test]
fn a_refused_step_names_the_step_and_its_refusal() {
    let world = world();
    let world = run(
        &world,
        as_user("stranger"),
        9_000,
        &feature_note(1, "featured"),
    )
    .expect("accepted")
    .0;
    // The same slug again: the unique constraint of the type refuses the create step.
    let error = refused(&world, as_user("stranger"), &feature_note(1, "featured"));
    assert_eq!(error.kind, ErrorKind::UniqueViolated);
    assert!(
        error.message.starts_with("step copy: unique:note.slug"),
        "{}",
        error.message
    );
    assert!(failed_rules(&error).contains(&"operation:feature:step:copy"));
}

// ---------------------------------------------------------------------------------------------
// The other refusals.
// ---------------------------------------------------------------------------------------------

#[test]
fn an_operation_the_tenant_does_not_have_is_refused() {
    let error = refused(
        &world(),
        as_user("stranger"),
        &invoke("nothing", &[], "k-1"),
    );
    assert_eq!(error.kind, ErrorKind::InvalidIntent);
    assert_eq!(error.message, "no operation nothing");
    assert_eq!(failed_rules(&error), vec!["operation:exists"]);
}

// Verifies: OPR-1546
#[test]
fn only_a_signed_in_person_invokes() {
    let error = refused(&world(), Principal::Anonymous, &feature_note(1, "featured"));
    assert_eq!(error.kind, ErrorKind::Unauthorized);
    assert_eq!(failed_rules(&error), vec!["operation:feature:invoker"]);
}

#[test]
fn an_idempotency_key_is_one_to_128_visible_characters_and_the_contexts() {
    let world = world();
    let with_key = |key: &str| {
        invoke(
            "feature",
            &[
                ("note", Value::Ref(RecordId(1))),
                ("slug", text("featured")),
            ],
            key,
        )
    };
    let long = "k".repeat(129);
    for key in ["", "has space", long.as_str()] {
        let error = refused(&world, as_user("stranger"), &with_key(key));
        assert_eq!(error.kind, ErrorKind::InvalidIntent, "{key:?}");
        assert_eq!(
            failed_rules(&error),
            vec!["operation:feature:idempotency_key"]
        );
    }
    run(
        &world,
        as_user("stranger"),
        9_000,
        &with_key(&"k".repeat(128)),
    )
    .expect("128 is allowed");
    // The store records the result under the context's key: the two must agree.
    let mut context = ctx("alpha", as_user("stranger"), 9_000);
    context.idempotency_key = Some("k-2".into());
    let error = world.step(&with_key("k-1"), &context).expect_err("refused");
    assert_eq!(error.kind, ErrorKind::InvalidIntent);
    context.idempotency_key = Some("k-1".into());
    world
        .step(&with_key("k-1"), &context)
        .expect("the same key");
}

#[test]
fn inputs_are_declared_present_and_of_their_type() {
    let world = world();
    let error = refused(
        &world,
        as_user("stranger"),
        &invoke("feature", &[("note", Value::Ref(RecordId(1)))], "k-1"),
    );
    assert_eq!(error.kind, ErrorKind::MissingRequired);
    assert_eq!(failed_rules(&error), vec!["input:feature.slug"]);
    let error = refused(
        &world,
        as_user("stranger"),
        &invoke(
            "feature",
            &[("note", text("1")), ("slug", text("featured"))],
            "k-1",
        ),
    );
    assert_eq!(error.kind, ErrorKind::TypeMismatch);
    assert_eq!(failed_rules(&error), vec!["input:feature.note"]);
}

#[test]
fn a_subject_the_invoker_may_not_read_is_refused_as_one_that_does_not_exist() {
    let world = world();
    // The member reads published notes only: the draft is as absent as a note that never was.
    let hidden = refused(&world, as_user("stranger"), &feature_note(2, "featured"));
    let missing = refused(&world, as_user("stranger"), &feature_note(99, "featured"));
    assert_eq!(hidden.kind, ErrorKind::UnknownRecord);
    assert_eq!(missing.kind, ErrorKind::UnknownRecord);
    assert_eq!(hidden.message, "no note 2");
    assert_eq!(missing.message, "no note 99");
    assert_eq!(failed_rules(&hidden), failed_rules(&missing));
}

// Verifies: OPR-1545
#[test]
fn a_posting_step_posts_to_the_ledger_under_the_key_of_the_invocation() {
    let world = world();
    let (_, transition) = run(
        &world,
        as_user("stranger"),
        9_000,
        &invoke("count", &[], "k-1"),
    )
    .expect("accepted");
    let Output::Operation { entries, .. } = &transition.output else {
        panic!("{:?}", transition.output);
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].idempotency_key,
        posting_key(
            &op_id("count"),
            &principal_id("alpha:stranger"),
            "k-1",
            "take"
        )
    );
    assert_eq!(entries[0].amount, LedgerAmount(1));
    assert!(transition
        .events
        .iter()
        .any(|event| matches!(event.body, EventBody::LedgerEntryPosted { .. })));
}

// ---------------------------------------------------------------------------------------------
// Tenants and plans.
// ---------------------------------------------------------------------------------------------

#[test]
fn an_invocation_reaches_only_its_own_tenant() {
    let world = world();
    // Beta has no such operation: an operation of alpha does not exist there.
    let error = world
        .step(
            &feature_note(1, "featured"),
            &ctx("beta", user("beta:writer1"), 9_000),
        )
        .expect_err("refused");
    assert_eq!(failed_rules(&error), vec!["operation:exists"]);
    // A snapshot of alpha cannot decide for a context of beta.
    let error = transition(
        &world.state(&tenant_id("alpha")),
        &feature_note(1, "featured"),
        &ctx("beta", user("beta:writer1"), 9_000),
    )
    .expect_err("refused");
    assert_eq!(error.kind, ErrorKind::TenantMismatch);
}

#[test]
fn the_plan_names_what_the_invocation_reads_in_one_load() {
    let world = world();
    let intent = feature_note(1, "featured");
    let planned = needs(Some(&*tenant(&world).schema), &intent);
    assert!(planned.contains(&Need::Record {
        object: object("note"),
        id: RecordId(1),
    }));
    assert!(planned.contains(&Need::ByField {
        object: object("note"),
        field: field("slug"),
        value: text("featured"),
    }));
    let snapshot = |coverage: BTreeSet<Need>| State {
        tenant_id: tenant_id("alpha"),
        tenant: Some(world.tenants[&tenant_id("alpha")].clone()),
        coverage: Coverage::Partial(coverage),
    };
    let context = ctx("alpha", as_user("stranger"), 9_000);
    // With nothing loaded the decision asks for the subject.
    let error = transition(&snapshot(BTreeSet::new()), &intent, &context).expect_err("short");
    assert_eq!(error.kind, ErrorKind::InsufficientState);
    assert_eq!(
        error.missing,
        Some(Need::Record {
            object: object("note"),
            id: RecordId(1),
        })
    );
    // With the plan loaded it is decided.
    let mut loaded: BTreeSet<Need> = planned.into_iter().collect();
    loaded.extend(refine_needs(&snapshot(loaded.clone()), &intent));
    transition(&snapshot(loaded), &intent, &context).expect("the plan is enough");
}
