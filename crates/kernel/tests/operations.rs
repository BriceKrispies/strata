//! Invoking a tenant operation (WG-5100; D-6546 to D-6549, OPR-1540 to OPR-1554): who may
//! invoke one, what it may do, that it commits all of its steps or none, and each refusal.
//! The repeat of a key is the store's (crates/backend/tests/operations.rs).

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::operations::posting_key;
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_model_core::types::{LedgerUnit, LedgerUnitDecl};

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

// ---------------------------------------------------------------------------------------------
// All or nothing.
// ---------------------------------------------------------------------------------------------

// ---------------------------------------------------------------------------------------------
// The other refusals.
// ---------------------------------------------------------------------------------------------

// The key is what makes a retried invocation post once across restarts, so its form is pinned.
// Verifies: OPR-1545
#[test]
fn a_posting_is_keyed_by_operation_invoker_key_and_step() {
    assert_eq!(
        posting_key(
            &op_id("redeem"),
            &principal_id("alpha:stranger"),
            "k-1",
            "take_stock"
        ),
        "op:redeem:alpha:stranger:k-1:take_stock"
    );
}

// ---------------------------------------------------------------------------------------------
// Tenants and plans.
// ---------------------------------------------------------------------------------------------
