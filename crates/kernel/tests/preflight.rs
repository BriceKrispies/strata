//! Preflight of a row action (WG-5466): what a row's affordance may say before the invocation
//! runs. `allowed` is never given where the decision refuses at stages 1 to 4 on the same state
//! and inputs; `unknown` is given for each case the design names, and `refused` carries the
//! condition's code.

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::operations::{
    invoke, preflight, reads_individual, Preflight, PreflightBudget, UnknownReason,
};
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

fn record_var(name: &str) -> Expr {
    var(Scope::Record, name)
}

fn condition(id: &str, expr: Expr, error: &str) -> TenantOperationCondition {
    TenantOperationCondition {
        id: id.into(),
        expr,
        error: error.into(),
        message: format!("{error}."),
    }
}

/// Delete the note the input names, for the given invokers and conditions.
fn discard_with(
    id: &str,
    signed_in: bool,
    roles: BTreeSet<RoleId>,
    conditions: Vec<TenantOperationCondition>,
) -> TenantOperationDef {
    TenantOperationDef {
        id: op_id(id),
        label: id.into(),
        inputs: vec![input("note", note_ref())],
        invokers: TenantOperationInvokers { signed_in, roles },
        subject: Some(TenantOperationSubject {
            object: object("note"),
            input: "note".into(),
            self_action: None,
        }),
        conditions,
        steps: vec![TenantOperationStep::DeleteRecord {
            id: "gone".into(),
            object: object("note"),
            record: param("note"),
        }],
        agent_policy: None,
    }
}

/// Delete a published note: refused for a draft with the code `not-published`.
fn discard_note() -> TenantOperationDef {
    discard_with(
        "discard_note",
        true,
        BTreeSet::new(),
        vec![condition(
            "published",
            eq(record_var("status"), lit("published")),
            "not-published",
        )],
    )
}

/// Only editors may invoke it.
fn editors_discard() -> TenantOperationDef {
    discard_with(
        "editors_discard",
        false,
        BTreeSet::from([role("editor")]),
        vec![],
    )
}

/// Refused once the note has a score, which the viewer may read only as an editor.
fn scored() -> TenantOperationDef {
    discard_with(
        "scored",
        true,
        BTreeSet::new(),
        vec![condition(
            "unscored",
            Expr::IsNull(Box::new(record_var("score"))),
            "already-scored",
        )],
    )
}

/// Only holders of the member role may invoke it: its condition reads the roles of the viewer.
fn member_only() -> TenantOperationDef {
    TenantOperationDef {
        id: op_id("member_only"),
        label: "Members only".into(),
        inputs: vec![],
        invokers: TenantOperationInvokers {
            signed_in: true,
            roles: BTreeSet::new(),
        },
        subject: None,
        conditions: vec![condition("member", Expr::HasRole(role("member")), "not-member")],
        steps: vec![TenantOperationStep::PostLedger {
            id: "take".into(),
            owner: TenantOperationPostingOwner::Invoker,
            unit: LedgerUnit::new("points").unwrap(),
            scope: None,
            amount: Expr::Lit(Value::Int(1)),
            reason: "counted".into(),
            reference: None,
        }],
        agent_policy: None,
    }
}

/// A seeded generator of operations over notes: the conditions, invokers and inputs vary, the
/// subject and its step do not. Each operation is `gen_<n>`.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() as usize) % n
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

fn generated_condition(rng: &mut Lcg, index: usize) -> TenantOperationCondition {
    let (expr, error) = match rng.below(5) {
        0 => (
            eq(record_var("status"), lit("published")),
            "not-published",
        ),
        1 => (
            Expr::Not(Box::new(eq(record_var("status"), lit("draft")))),
            "is-draft",
        ),
        2 => (
            Expr::IsNull(Box::new(record_var("score"))),
            "already-scored",
        ),
        3 => (
            Expr::Or(vec![
                Expr::HasRole(role("editor")),
                eq(record_var("status"), lit("draft")),
            ]),
            "editors-or-drafts",
        ),
        _ => (
            Expr::And(vec![
                eq(record_var("status"), lit("published")),
                Expr::IsNull(Box::new(record_var("score"))),
            ]),
            "published-unscored",
        ),
    };
    condition(&format!("c{index}"), expr, error)
}

fn generated_operation(rng: &mut Lcg, n: usize) -> TenantOperationDef {
    let signed_in = rng.chance(60);
    let roles = if signed_in {
        BTreeSet::new()
    } else {
        BTreeSet::from([role("editor")])
    };
    let count = 1 + rng.below(2);
    let conditions = (0..count)
        .map(|index| generated_condition(rng, index))
        .collect();
    let mut op = discard_with(&format!("gen_{n}"), signed_in, roles, conditions);
    op.label = format!("Generated {n}");
    op
}

/// The notes world of alpha with the operations above and the generated ones: note 1 published
/// by the editor and scored, note 2 a draft of writer1, and the stranger a member.
fn world() -> World {
    let mut world = must(
        &notes_world(),
        "owner",
        5_000,
        Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineRole(RoleDef {
                id: role("member"),
                label: "Member".into(),
            })],
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    world = must(
        &world,
        "owner",
        5_001,
        Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineField {
                object: object("note"),
                field: FieldDef {
                    read: Some(Expr::HasRole(role("editor"))),
                    ..field_def("score", FieldType::Int, false)
                },
            }],
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    let mut operations = vec![
        discard_note(),
        scored(),
        editors_discard(),
        member_only(),
    ];
    let mut rng = Lcg(0x5eed_5466);
    for n in 0..12 {
        operations.push(generated_operation(&mut rng, n));
    }
    world = must(
        &world,
        "owner",
        5_002,
        Intent::ChangeSchema {
            changes: std::iter::once(SchemaChange::DeclareLedgerUnit(LedgerUnitDecl {
                unit: LedgerUnit::new("points").unwrap(),
                description: "Points".into(),
                expiry: None,
            }))
            .chain(operations.into_iter().map(SchemaChange::DefineOperation))
            .collect(),
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    world = must(
        &world,
        "owner",
        5_003,
        Intent::AssignRole {
            principal: principal_id("alpha:stranger"),
            role: role("member"),
        },
    );
    world = must(
        &world,
        "editor",
        5_004,
        new_note("Field notes", "field-notes"),
    );
    world = must(
        &world,
        "editor",
        5_005,
        Intent::UpdateRecord {
            object: object("note"),
            id: RecordId(1),
            fields: BTreeMap::from([
                (field("status"), text("published")),
                (field("score"), Value::Int(7)),
            ]),
            expected_version: None,
        },
    );
    must(&world, "writer1", 5_006, new_note("Draft", "draft"))
}

fn as_user(who: &str) -> Principal {
    user(&format!("alpha:{who}"))
}

fn must(world: &World, who: &str, now: i64, intent: Intent) -> World {
    world
        .step(&intent, &ctx("alpha", as_user(who), now))
        .expect("accepted")
        .0
}

fn tenant(world: &World) -> &TenantState {
    &world.tenants[&tenant_id("alpha")]
}

fn row(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.clone()))
        .collect()
}

fn note_row(id: u64) -> BTreeMap<String, Value> {
    row(&[("note", Value::Ref(RecordId(id)))])
}

/// The answer of preflight for `who` on `op` with `inputs`, on a snapshot with `coverage`.
fn ask(
    world: &World,
    who: &str,
    op: &str,
    inputs: &BTreeMap<String, Value>,
    coverage: &Coverage,
    budget: &mut PreflightBudget,
) -> Preflight {
    preflight(
        tenant(world),
        coverage,
        &ctx("alpha", as_user(who), 9_000),
        &op_id(op),
        inputs,
        "k-1",
        budget,
    )
}

fn ample() -> PreflightBudget {
    PreflightBudget::new(10_000)
}

// ---------------------------------------------------------------------------------------------
// Each case the design names.
// ---------------------------------------------------------------------------------------------

#[test]
fn an_input_the_row_does_not_supply_is_unknown() {
    let world = world();
    let answer = ask(
        &world,
        "editor",
        "discard_note",
        &row(&[]),
        &Coverage::Complete,
        &mut ample(),
    );
    assert_eq!(
        answer,
        Preflight::Unknown(UnknownReason::InputNotSupplied("note".into()))
    );
}

#[test]
fn a_viewer_who_may_not_read_the_condition_record_gets_unknown() {
    let world = world();
    // The stranger may not read the score, so the condition on it is not known to them.
    assert_eq!(
        ask(
            &world,
            "stranger",
            "scored",
            &note_row(1),
            &Coverage::Complete,
            &mut ample(),
        ),
        Preflight::Unknown(UnknownReason::WithheldRead)
    );
    // An editor may read it, and the condition refuses with its own code.
    assert_eq!(
        ask(
            &world,
            "editor",
            "scored",
            &note_row(1),
            &Coverage::Complete,
            &mut ample(),
        ),
        Preflight::Refused {
            code: "already-scored".into(),
            detail: "already-scored: already-scored.".into(),
        }
    );
}

#[test]
fn a_snapshot_that_does_not_hold_the_subject_is_unknown() {
    let world = world();
    let answer = ask(
        &world,
        "editor",
        "discard_note",
        &note_row(1),
        &Coverage::Partial(BTreeSet::new()),
        &mut ample(),
    );
    assert_eq!(answer, Preflight::Unknown(UnknownReason::NotLoaded));
}

#[test]
fn past_the_session_budget_every_answer_is_unknown() {
    let world = world();
    let mut budget = PreflightBudget::new(1);
    assert_eq!(
        ask(
            &world,
            "editor",
            "discard_note",
            &note_row(1),
            &Coverage::Complete,
            &mut budget,
        ),
        Preflight::Allowed
    );
    assert_eq!(budget.remaining(), 0);
    assert_eq!(
        ask(
            &world,
            "editor",
            "discard_note",
            &note_row(1),
            &Coverage::Complete,
            &mut budget,
        ),
        Preflight::Unknown(UnknownReason::Budget)
    );
}

// ---------------------------------------------------------------------------------------------
// Refusals and allowances carry what the decision would say.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_condition_refusal_carries_the_condition_code() {
    let world = world();
    assert_eq!(
        ask(
            &world,
            "editor",
            "discard_note",
            &note_row(2),
            &Coverage::Complete,
            &mut ample(),
        ),
        Preflight::Refused {
            code: "not-published".into(),
            detail: "not-published: not-published.".into(),
        }
    );
}

#[test]
fn an_invoker_refusal_names_its_rule() {
    let world = world();
    let answer = ask(
        &world,
        "stranger",
        "editors_discard",
        &note_row(1),
        &Coverage::Complete,
        &mut ample(),
    );
    assert_eq!(
        answer,
        Preflight::Refused {
            code: "operation:editors_discard:invoker".into(),
            detail: "the invokers of operation editors_discard do not name alpha:stranger".into(),
        }
    );
}

#[test]
fn a_row_action_that_holds_is_allowed() {
    let world = world();
    assert_eq!(
        ask(
            &world,
            "editor",
            "discard_note",
            &note_row(1),
            &Coverage::Complete,
            &mut ample(),
        ),
        Preflight::Allowed
    );
}

#[test]
fn a_condition_on_the_principal_or_the_clock_makes_the_answer_individual() {
    assert!(reads_individual(&member_only()));
    assert!(!reads_individual(&discard_note()));
}

// ---------------------------------------------------------------------------------------------
// Differential: preflight never answers allowed where the decision refuses at stages 1 to 4.
// ---------------------------------------------------------------------------------------------

/// The refusal of a stage names no step: a step's refusal is named `operation:<id>:step:<step>`.
fn refused_by_a_stage(error: &DomainError) -> bool {
    error
        .evidence
        .last()
        .is_some_and(|check| !check.rule.contains(":step:"))
}

// Verifies: OPR-1542
#[test]
fn preflight_never_allows_what_the_decision_refuses_at_stages_one_to_four() {
    let world = world();
    let tenant = tenant(&world);
    let principals = ["owner", "editor", "writer1", "stranger"];
    let inputs: Vec<BTreeMap<String, Value>> = vec![
        row(&[]),
        note_row(1),
        note_row(2),
        note_row(999),
        row(&[("note", text("1"))]),
        row(&[("note", Value::Ref(RecordId(1))), ("extra", Value::Int(1))]),
    ];
    let coverages = [Coverage::Complete, Coverage::Partial(BTreeSet::new())];
    let mut operations: Vec<String> = vec![
        "discard_note".into(),
        "scored".into(),
        "editors_discard".into(),
        "member_only".into(),
    ];
    operations.extend((0..12).map(|n| format!("gen_{n}")));

    let (mut allowed, mut refused, mut unknown) = (0usize, 0usize, 0usize);
    for op in &operations {
        for who in principals {
            for input in &inputs {
                for coverage in &coverages {
                    let context = ctx("alpha", as_user(who), 9_000);
                    let answer = preflight(
                        tenant,
                        coverage,
                        &context,
                        &op_id(op),
                        input,
                        "k-1",
                        &mut ample(),
                    );
                    let decision = invoke(
                        tenant,
                        coverage,
                        &context,
                        &op_id(op),
                        input,
                        "k-1",
                    );
                    match answer {
                        Preflight::Allowed => {
                            allowed += 1;
                            if let Err(error) = &decision {
                                assert!(
                                    !refused_by_a_stage(error),
                                    "{op} by {who} allowed, but the decision refuses at a stage: {error:?}"
                                );
                            }
                        }
                        Preflight::Refused { .. } => {
                            refused += 1;
                            assert!(
                                decision.is_err(),
                                "{op} by {who} refused by preflight, but the decision accepts"
                            );
                        }
                        Preflight::Unknown(_) => unknown += 1,
                    }
                }
            }
        }
    }
    // The generated space reaches every answer, so the agreement above is not vacuous.
    assert!(allowed > 0, "no allowed answer in the generated space");
    assert!(refused > 0, "no refused answer in the generated space");
    assert!(unknown > 0, "no unknown answer in the generated space");
}
