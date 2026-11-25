//! Invoking a tenant operation (WG-5100; D-6546 to D-6549, OPR-1540 to OPR-1554): who may
//! invoke one, what it may do, that it commits all of its steps or none, and each refusal.
//! The repeat of a key is the store's (crates/backend/tests/operations.rs).

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::ledger;
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
            self_action: None,
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
        agent_policy: None,
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
            self_action: None,
        }),
        conditions: vec![],
        steps: vec![TenantOperationStep::DeleteRecord {
            id: "gone".into(),
            object: object("note"),
            record: param("note"),
        }],
        agent_policy: None,
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
            reference: None,
        }],
        agent_policy: None,
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
                    transfer: None,
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

// ---------------------------------------------------------------------------------------------
// Find and branch steps (WG-5596): a toggle, two toggles at once, and a find that reads only
// what the invoker may read.
// ---------------------------------------------------------------------------------------------

fn react_op() -> TenantOperationDef {
    TenantOperationDef {
        id: op_id("react"),
        label: "React, or take the reaction back".into(),
        inputs: vec![input("note", note_ref()), input("kind", FieldType::Str)],
        invokers: TenantOperationInvokers {
            signed_in: true,
            roles: BTreeSet::new(),
        },
        subject: None,
        conditions: Vec::new(),
        steps: vec![
            TenantOperationStep::FindRecord {
                id: "mine".into(),
                object: object("reaction"),
                predicate: Expr::And(vec![
                    eq(var(Scope::Record, "message"), param("note")),
                    eq(var(Scope::Record, "reactor"), Expr::Principal),
                    eq(var(Scope::Record, "kind"), param("kind")),
                ]),
                error: "toggle-ambiguous".into(),
            },
            TenantOperationStep::Branch {
                id: "toggle".into(),
                test: BranchTest::Found {
                    binding: "mine".into(),
                },
                matched: vec![TenantOperationStep::DeleteRecord {
                    id: "removed".into(),
                    object: object("reaction"),
                    record: param("mine"),
                }],
                unmatched: vec![TenantOperationStep::CreateRecord {
                    id: "added".into(),
                    object: object("reaction"),
                    fields: BTreeMap::from([
                        (field("message"), param("note")),
                        (field("reactor"), Expr::Principal),
                        (field("kind"), param("kind")),
                    ]),
                }],
            },
        ],
        agent_policy: None,
    }
}

/// Finds the notes with the title input and, when exactly one is visible to the invoker, raises its
/// rank.
fn bump_title_op() -> TenantOperationDef {
    TenantOperationDef {
        id: op_id("bump_title"),
        label: "Raise the rank of the note with a title".into(),
        inputs: vec![input("title", FieldType::Str)],
        invokers: TenantOperationInvokers {
            signed_in: true,
            roles: BTreeSet::new(),
        },
        subject: None,
        conditions: Vec::new(),
        steps: vec![
            TenantOperationStep::FindRecord {
                id: "same".into(),
                object: object("note"),
                predicate: eq(var(Scope::Record, "title"), param("title")),
                error: "title-ambiguous".into(),
            },
            TenantOperationStep::Branch {
                id: "bump".into(),
                test: BranchTest::Found {
                    binding: "same".into(),
                },
                matched: vec![TenantOperationStep::UpdateRecord {
                    id: "bumped".into(),
                    object: object("note"),
                    record: param("same"),
                    fields: BTreeMap::from([(field("rank"), int(9))]),
                }],
                unmatched: Vec::new(),
            },
        ],
        agent_policy: None,
    }
}

/// The notes world of alpha with a reaction object (one reaction per message, reactor and kind),
/// readable by the writers only as their own, and the two operations above.
fn reactions_world() -> World {
    let reaction = object("reaction");
    let own = eq(var(Scope::Record, "$created_by"), Expr::Principal);
    let changes = vec![
        SchemaChange::DefineObject {
            id: reaction.clone(),
            label: "Reaction".into(),
        },
        SchemaChange::DefineField {
            object: reaction.clone(),
            field: field_def(
                "message",
                FieldType::Ref {
                    target: object("note"),
                    on_delete: OnDelete::Restrict,
                },
                true,
            ),
        },
        SchemaChange::DefineField {
            object: reaction.clone(),
            field: field_def("reactor", FieldType::Str, true),
        },
        SchemaChange::DefineField {
            object: reaction.clone(),
            field: field_def("kind", FieldType::Str, true),
        },
        SchemaChange::DefineUnique {
            object: reaction.clone(),
            unique: UniqueDef {
                id: ConstraintId::new("one_of_a_kind").expect("constraint id"),
                fields: vec![field("message"), field("reactor"), field("kind")],
                message: "You have already reacted to this message that way.".into(),
            },
        },
        SchemaChange::DefinePolicy(PolicyDef {
            id: PolicyId::new("writers_read_own_reactions").expect("policy id"),
            role: role("writer"),
            resource: Resource::Object(reaction.clone()),
            actions: BTreeSet::from([Action::Read]),
            condition: Some(own),
        }),
        SchemaChange::DefineOperation(react_op()),
        SchemaChange::DefineOperation(bump_title_op()),
    ];
    must(
        &notes_world(),
        "owner",
        5_000,
        Intent::ChangeSchema {
            changes,
            expected_version: None,
            existing: ExistingData::Reject,
        },
    )
}

fn react(note: u64, kind: &str, key: &str) -> Intent {
    invoke(
        "react",
        &[("note", Value::Ref(RecordId(note))), ("kind", text(kind))],
        key,
    )
}

fn reactions(world: &World) -> Vec<&Record> {
    tenant(world)
        .records_of(&object("reaction"))
        .map(|record| record.as_ref())
        .collect()
}

/// The reaction of a toggle is one commit: created when none is there, removed when one is, each
/// of them one revision.
#[test]
fn a_toggle_removes_the_callers_reaction_when_present_and_creates_it_otherwise() {
    let base = must(
        &reactions_world(),
        "writer1",
        5_010,
        new_note("Field notes", "field-notes"),
    );
    let revision = tenant(&base).revision;

    let added = must(&base, "writer1", 5_011, react(1, "love", "k-add"));
    assert_eq!(
        reactions(&added).len(),
        1,
        "the first toggle creates one reaction"
    );
    assert_eq!(
        tenant(&added).revision,
        StateRevision(revision.0 + 1),
        "the toggle is one commit"
    );

    let removed = must(&added, "writer1", 5_012, react(1, "love", "k-remove"));
    assert!(
        reactions(&removed).is_empty(),
        "the second toggle removes the reaction it found"
    );
    assert_eq!(tenant(&removed).revision, StateRevision(revision.0 + 2));

    // Another kind is a reaction of its own.
    let other = must(&added, "writer1", 5_013, react(1, "laugh", "k-laugh"));
    assert_eq!(reactions(&other).len(), 2);
}

/// Two toggles by the same person decided on the same state: the first commit is accepted, the
/// second is based on a revision that is gone and fails with `VersionConflict`, which the caller
/// may retry. The retry decides on the new state and removes the reaction, so at most one remains
/// at any point. The unique constraint refuses a second create of the same triple as well.
#[test]
fn two_concurrent_toggles_by_one_person_leave_at_most_one_reaction() {
    let base = must(
        &reactions_world(),
        "writer1",
        5_020,
        new_note("Field notes", "field-notes"),
    );
    let ctx_first = ctx("alpha", as_user("writer1"), 5_021);
    let ctx_second = ctx("alpha", as_user("writer1"), 5_021);
    let first_intent = react(1, "love", "k-first");
    let second_intent = react(1, "love", "k-second");

    // Both decide on the same snapshot: neither sees a reaction.
    let (after_first, first) = base
        .step(&first_intent, &ctx_first)
        .expect("first accepted");
    let second = transition(
        &base.state(&tenant_id("alpha")),
        &second_intent,
        &ctx_second,
    )
    .expect("second decided on the stale snapshot");
    assert_eq!(reactions(&after_first).len(), 1);
    assert!(!first.delta.ops.is_empty());

    // The loser's commit is refused for its base revision: the caller gets a conflict to retry.
    let loser = apply_delta(
        after_first
            .tenants
            .get(&tenant_id("alpha"))
            .map(|t| t.as_ref()),
        &second.delta,
    )
    .expect_err("the stale delta is refused");
    assert_eq!(loser.kind, ErrorKind::VersionConflict);

    // Retried on the new state, the second toggle removes the first reaction: none is left twice.
    let (after_retry, _) = after_first
        .step(&second_intent, &ctx_second)
        .expect("retry accepted");
    assert!(reactions(&after_retry).is_empty());

    // A create of the same triple is refused by the unique constraint whatever the decision.
    let duplicate = Intent::CreateRecord {
        object: object("reaction"),
        fields: BTreeMap::from([
            (field("message"), Value::Ref(RecordId(1))),
            (field("reactor"), text("alpha:writer1")),
            (field("kind"), text("love")),
        ]),
    };
    let refused = after_first
        .step(&duplicate, &ctx("alpha", as_user("owner"), 5_022))
        .expect_err("a duplicate reaction is refused");
    assert_eq!(refused.kind, ErrorKind::UniqueViolated);
}

/// A find reads only the records the invoker may read. The writer's own note is found and raised;
/// the same title on a reaction it cannot read (another person's) is not a candidate, so the
/// toggle creates its own reaction without any refusal and leaves the other one alone.
#[test]
fn a_find_does_not_see_a_record_the_invoker_may_not_read() {
    let world = must(
        &reactions_world(),
        "writer2",
        5_030,
        new_note("Field notes", "field-notes"),
    );
    let world = must(&world, "writer2", 5_031, react(1, "love", "k-theirs"));
    let theirs = reactions(&world)[0].id;

    // writer1 cannot read writer2's reaction: the find finds none, and the toggle creates one.
    let world = must(&world, "writer1", 5_032, react(1, "love", "k-mine"));
    let all = reactions(&world);
    assert_eq!(all.len(), 2, "writer1 creates its own reaction");
    assert!(
        all.iter()
            .any(|r| r.id == theirs && r.created_by == Some(principal_id("alpha:writer2"))),
        "writer2's reaction is left as it was"
    );
}

/// Two notes with one title that the invoker may read are two matches: the find refuses with the
/// step's error code, and nothing is changed.
#[test]
fn two_readable_matches_refuse_with_the_steps_error_code() {
    let world = must(
        &reactions_world(),
        "writer2",
        5_040,
        new_note("Dup", "dup-one"),
    );
    let world = must(&world, "writer1", 5_041, new_note("Dup", "dup-two"));
    let rank_before = note(&world, 2).map(|n| n.version);
    let refused = refused(
        &world,
        as_user("writer1"),
        &invoke("bump_title", &[("title", text("Dup"))], "k-two"),
    );
    assert!(
        refused.message.contains("title-ambiguous"),
        "two readable matches refuse with the step's code: {}",
        refused.message
    );
    assert_eq!(note(&world, 2).map(|n| n.version), rank_before);
}

// ---------------------------------------------------------------------------------------------
// Reversals of ledger postings (WG-5598): a toggle that reverses what its add posted, and the
// refusals of a reversal that has nothing to reverse.

/// A toggle of the counted operation, which posts on add and reverses on remove.
fn counted(note: u64, kind: &str, key: &str) -> Intent {
    invoke(
        "react_counted",
        &[("note", Value::Ref(RecordId(note))), ("kind", text(kind))],
        key,
    )
}

fn given_unit() -> LedgerUnit {
    LedgerUnit::new("reactions_given").expect("unit")
}

fn received_unit() -> LedgerUnit {
    LedgerUnit::new("reactions_received").expect("unit")
}

/// The toggle of a reaction that posts what it gives and receives on add, and reverses both on
/// remove. The postings are keyed by the reaction they are for, so the remove finds them again.
fn counted_react_op() -> TenantOperationDef {
    let mut steps = react_op().steps;
    let Some(TenantOperationStep::Branch {
        matched, unmatched, ..
    }) = steps.last_mut()
    else {
        panic!("the toggle ends with a branch");
    };
    *matched = vec![
        TenantOperationStep::DeleteRecord {
            id: "removed".into(),
            object: object("reaction"),
            record: param("mine"),
        },
        TenantOperationStep::ReverseLedger {
            id: "given_back".into(),
            posting: "given".into(),
            reference: param("mine"),
            reason: "reaction-taken-back".into(),
        },
        TenantOperationStep::ReverseLedger {
            id: "received_back".into(),
            posting: "received".into(),
            reference: param("mine"),
            reason: "reaction-taken-back".into(),
        },
    ];
    *unmatched = vec![
        TenantOperationStep::CreateRecord {
            id: "added".into(),
            object: object("reaction"),
            fields: BTreeMap::from([
                (field("message"), param("note")),
                (field("reactor"), Expr::Principal),
                (field("kind"), param("kind")),
            ]),
        },
        TenantOperationStep::PostLedger {
            id: "given".into(),
            owner: TenantOperationPostingOwner::Invoker,
            unit: given_unit(),
            scope: None,
            amount: int(1),
            reason: "reaction-given".into(),
            reference: Some(param("added")),
        },
        TenantOperationStep::PostLedger {
            id: "received".into(),
            owner: TenantOperationPostingOwner::Principal {
                principal: lit("alpha:writer1"),
            },
            unit: received_unit(),
            scope: None,
            amount: int(1),
            reason: "reaction-received".into(),
            reference: Some(param("added")),
        },
    ];
    TenantOperationDef {
        id: op_id("react_counted"),
        steps,
        ..react_op()
    }
}

/// An operation with the given steps, invoked with a note as its input.
fn ledger_op(id: &str, steps: Vec<TenantOperationStep>) -> TenantOperationDef {
    TenantOperationDef {
        id: op_id(id),
        label: id.into(),
        inputs: vec![input("note", note_ref())],
        invokers: TenantOperationInvokers {
            signed_in: true,
            roles: BTreeSet::new(),
        },
        subject: None,
        conditions: Vec::new(),
        steps,
        agent_policy: None,
    }
}

/// A posting of one point under the note it is for, as the invoker.
fn given_posting() -> TenantOperationStep {
    TenantOperationStep::PostLedger {
        id: "given".into(),
        owner: TenantOperationPostingOwner::Invoker,
        unit: given_unit(),
        scope: None,
        amount: int(1),
        reason: "reaction-given".into(),
        reference: Some(param("note")),
    }
}

/// The reversal of the posting `given` made under the note.
fn given_back(id: &str) -> TenantOperationStep {
    TenantOperationStep::ReverseLedger {
        id: id.into(),
        posting: "given".into(),
        reference: param("note"),
        reason: "undo".into(),
    }
}

/// The notes world with the reactions, two ledger units, a cap of two additions a day on the
/// reactions given (which counts additions only), and the operations of the reversal tests.
fn reversal_world() -> World {
    let world = must(
        &reactions_world(),
        "owner",
        5_100,
        Intent::ChangeSchema {
            changes: vec![
                SchemaChange::DeclareLedgerUnit(LedgerUnitDecl {
                    unit: given_unit(),
                    description: "Reactions given".into(),
                    expiry: None,
                    transfer: None,
                }),
                SchemaChange::DeclareLedgerUnit(LedgerUnitDecl {
                    unit: received_unit(),
                    description: "Reactions received".into(),
                    expiry: None,
                    transfer: None,
                }),
                SchemaChange::SetLedgerLimit(LedgerLimit {
                    unit: given_unit(),
                    owner: None,
                    floor: None,
                    cap: None,
                    period_cap: Some(2),
                    period: Some(LedgerPeriod::Day),
                    scope: None,
                    additions_only: Some(true),
                }),
                SchemaChange::DefineOperation(counted_react_op()),
                SchemaChange::DefineOperation(ledger_op(
                    "post_then_undo",
                    vec![given_posting(), given_back("undo")],
                )),
                SchemaChange::DefineOperation(ledger_op(
                    "post_twice_undo",
                    vec![given_posting(), given_back("undo"), given_back("again")],
                )),
                SchemaChange::DefineOperation(ledger_op(
                    "undo_only",
                    vec![TenantOperationStep::Branch {
                        id: "gate".into(),
                        test: BranchTest::Condition {
                            expr: Expr::Lit(Value::Bool(false)),
                        },
                        matched: vec![given_posting()],
                        unmatched: vec![given_back("undo")],
                    }],
                )),
            ],
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    must(
        &world,
        "writer1",
        5_101,
        new_note("Second note", "second-note"),
    )
}

/// The balance of a unit of the account of a principal, zero before it has an entry.
fn unit_balance(world: &World, who: &str, unit: &LedgerUnit) -> i64 {
    let tenant = tenant(world);
    let owner = LedgerOwner::Principal {
        principal: principal_id(&format!("alpha:{who}")),
    };
    let id = ledger::account_id(&tenant.id, &owner, unit, "");
    tenant
        .ledger
        .as_ref()
        .and_then(|l| l.accounts.get(&ledger::account_key(&id)))
        .map_or(0, |account| account.balance.0)
}

/// The entries of the account of a unit of a principal, in order.
fn unit_entries(world: &World, who: &str, unit: &LedgerUnit) -> Vec<LedgerEntry> {
    let tenant = tenant(world);
    let owner = LedgerOwner::Principal {
        principal: principal_id(&format!("alpha:{who}")),
    };
    let id = ledger::account_id(&tenant.id, &owner, unit, "");
    tenant
        .ledger
        .as_ref()
        .and_then(|l| l.entries.get(&ledger::account_key(&id)).cloned())
        .unwrap_or_default()
}

/// Verifies: LDG-2900, LDG-2902
/// Add, remove, add: the toggle posts what it gives and receives on add and reverses both on remove.
/// The balances return to zero on remove, and the daily cap counts the two additions only, so a
/// third addition of the day is refused although the removal came first.
#[test]
fn a_toggle_that_reverses_its_postings_on_remove_leaves_balances_and_the_cap_count_of_additions() {
    let base = reversal_world();
    let added = must(&base, "writer1", 5_200, counted(1, "love", "k-add"));
    assert_eq!(unit_balance(&added, "writer1", &given_unit()), 1);
    assert_eq!(unit_balance(&added, "writer1", &received_unit()), 1);

    let removed = must(&added, "writer1", 5_201, counted(1, "love", "k-remove"));
    assert!(reactions(&removed).is_empty());
    assert_eq!(unit_balance(&removed, "writer1", &given_unit()), 0);
    assert_eq!(unit_balance(&removed, "writer1", &received_unit()), 0);
    let given = unit_entries(&removed, "writer1", &given_unit());
    assert_eq!(given.len(), 2, "the add and its reversal are both entries");
    assert_eq!(given[1].amount, LedgerAmount(-1));
    assert_eq!(given[1].corrects, Some(given[0].sequence));

    // The second add is the second addition of the day, still within the cap of two.
    let again = must(&removed, "writer1", 5_202, counted(1, "love", "k-again"));
    assert_eq!(unit_balance(&again, "writer1", &given_unit()), 1);
    let removed_again = must(
        &again,
        "writer1",
        5_203,
        counted(1, "love", "k-remove-again"),
    );

    // Removing freed no capacity, so a third addition of the day is over the cap.
    let error = run(
        &removed_again,
        as_user("writer1"),
        5_204,
        &counted(1, "love", "k-third"),
    )
    .expect_err("the third addition of the day is over the cap");
    assert!(
        error
            .evidence
            .iter()
            .any(|check| check.rule == "ledger:above-period-cap"),
        "the refusal names the cap: {}",
        error.message
    );
    assert!(reactions(&removed_again).is_empty());
    assert_eq!(unit_balance(&removed_again, "writer1", &given_unit()), 0);
}

/// Verifies: LDG-2900
/// A posting and its reversal in one operation: the balance returns to its value before the posting.
#[test]
fn a_reversal_in_the_same_operation_returns_the_balance_to_what_it_was() {
    let base = reversal_world();
    let done = must(
        &base,
        "writer1",
        5_300,
        invoke(
            "post_then_undo",
            &[("note", Value::Ref(RecordId(1)))],
            "k-undo",
        ),
    );
    assert_eq!(unit_balance(&done, "writer1", &given_unit()), 0);
    let entries = unit_entries(&done, "writer1", &given_unit());
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].amount, LedgerAmount(-1));
    assert_eq!(entries[1].corrects, Some(entries[0].sequence));
}

/// Verifies: LDG-2901
/// A reversal of a posting that was reversed already in the same change is refused, and nothing is
/// committed.
#[test]
fn a_posting_reversed_twice_is_refused_the_second_time() {
    let base = reversal_world();
    let error = refused(
        &base,
        as_user("writer1"),
        &invoke(
            "post_twice_undo",
            &[("note", Value::Ref(RecordId(1)))],
            "k-twice",
        ),
    );
    assert!(
        error
            .evidence
            .iter()
            .any(|check| check.rule == "ledger:already-reversed"),
        "the refusal names the reversal: {}",
        error.message
    );
    assert_eq!(unit_balance(&base, "writer1", &given_unit()), 0);
}

/// Verifies: LDG-2901
/// A reversal of a posting that was never made is refused.
#[test]
fn a_reversal_of_a_posting_that_was_never_made_is_refused() {
    let base = reversal_world();
    let error = refused(
        &base,
        as_user("writer1"),
        &invoke("undo_only", &[("note", Value::Ref(RecordId(1)))], "k-none"),
    );
    assert!(
        error
            .evidence
            .iter()
            .any(|check| check.rule == "ledger:no-such-posting"),
        "the refusal names the missing posting: {}",
        error.message
    );
    assert!(unit_entries(&base, "writer1", &given_unit()).is_empty());
}
