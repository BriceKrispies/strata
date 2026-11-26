//! A delete clears the set-null references that point at the deleted record (D9). The record that
//! holds such a reference is rewritten, so its constraints must still hold on the rewritten record:
//! a delete that would leave one violated is refused with the constraint error, as a restrict
//! refuses a delete with `StillReferenced` (WG-5664).

use strata_kernel::testkit::*;
use strata_kernel::*;

/// The notes tenant of `testkit` with a `node` type. `next` refers to a node and is cleared when
/// that node is deleted. The constraint `flag_needs_next` says a node with a flag has a next.
fn world_with_nodes() -> World {
    let next_is_set = Expr::Not(Box::new(Expr::IsNull(Box::new(var(Scope::Record, "next")))));
    let flag_is_clear = Expr::IsNull(Box::new(var(Scope::Record, "flag")));
    let changes = vec![
        SchemaChange::DefineObject {
            id: object("node"),
            label: "Node".into(),
        },
        SchemaChange::DefineField {
            object: object("node"),
            field: field_def(
                "next",
                FieldType::Ref {
                    target: object("node"),
                    on_delete: OnDelete::SetNull,
                },
                false,
            ),
        },
        SchemaChange::DefineField {
            object: object("node"),
            field: field_def("flag", FieldType::Bool, false),
        },
        SchemaChange::DefineConstraint {
            object: object("node"),
            constraint: ConstraintDef {
                id: ConstraintId::new("flag_needs_next").expect("constraint id"),
                expr: Expr::Or(vec![flag_is_clear, next_is_set]),
                message: "a flagged node needs a next".into(),
            },
        },
    ];
    let intent = Intent::ChangeSchema {
        changes,
        expected_version: None,
        existing: ExistingData::Reject,
    };
    notes_world()
        .step(&intent, &ctx("alpha", user("alpha:owner"), 9_000))
        .expect("the owner defines the node type")
        .0
}

fn create(
    world: &World,
    fields: &[(&str, Value)],
    now: i64,
) -> Result<(World, RecordId), DomainError> {
    let intent = Intent::CreateRecord {
        object: object("node"),
        fields: fields
            .iter()
            .map(|(name, value)| (field(name), value.clone()))
            .collect(),
    };
    let (next, transition) = world.step(&intent, &ctx("alpha", user("alpha:owner"), now))?;
    match transition.output {
        Output::Record { id, .. } => Ok((next, id)),
        other => panic!("a create answers with the record, not {other:?}"),
    }
}

fn delete(world: &World, id: RecordId, now: i64) -> Result<World, DomainError> {
    let intent = Intent::DeleteRecord {
        object: object("node"),
        id,
        expected_version: None,
    };
    world
        .step(&intent, &ctx("alpha", user("alpha:owner"), now))
        .map(|(next, _)| next)
}

#[test]
fn a_delete_that_would_leave_a_referrer_breaking_its_constraint_is_refused() {
    let (world, a) = create(&world_with_nodes(), &[], 9_100).expect("a node with no flag");
    let (world, b) = create(
        &world,
        &[("flag", Value::Bool(true)), ("next", Value::Ref(a))],
        9_101,
    )
    .expect("a flagged node that refers to a node");

    // The delete of `a` would clear `b.next`, leaving `b` flagged with no next: refused.
    let refused = delete(&world, a, 9_102).expect_err("the delete is refused");
    assert_eq!(
        refused.kind,
        ErrorKind::ConstraintViolated,
        "{}",
        refused.message
    );

    // Refused, so nothing changed: `b` still refers to `a`, and deleting `b` first is accepted.
    let world = delete(&world, b, 9_103).expect("the referrer is deleted");
    delete(&world, a, 9_104).expect("nothing refers to a any more");
}

#[test]
fn a_delete_that_clears_only_an_unflagged_referrer_is_accepted() {
    let (world, a) = create(&world_with_nodes(), &[], 9_200).expect("a node with no flag");
    let (world, _b) = create(&world, &[("next", Value::Ref(a))], 9_201)
        .expect("an unflagged node that refers to a node");

    // `b` has no flag, so clearing its next breaks nothing: the delete goes through.
    delete(&world, a, 9_202).expect("accepted: the cleared referrer keeps its constraint");
}
