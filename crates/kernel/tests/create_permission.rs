//! A create asks for the permission before it evaluates the defaults of the type (WG-5665). A
//! caller whom no grant or policy could allow to create is refused `unauthorized` whether or not a
//! default would fail, and whether or not the type exists, so the refusal tells it nothing about
//! the schema. A caller who could create still gets the defaults' evaluation error.

use strata_kernel::testkit::*;
use strata_kernel::*;

/// The largest integer the kernel holds exactly (2^53 - 1). One more than it overflows.
const LARGEST: i64 = 9_007_199_254_740_991;

/// The notes tenant of `testkit` with an object type `probe` whose field `count` defaults to
/// `LARGEST + 1`, an `integer-overflow` evaluation error. Only the owner may create on `probe`.
fn world_with_failing_default() -> World {
    let overflowing = Expr::Arith(
        ArithOp::Add,
        Box::new(Expr::Lit(Value::Int(LARGEST))),
        Box::new(Expr::Lit(Value::Int(1))),
    );
    let changes = vec![
        SchemaChange::DefineObject {
            id: object("probe"),
            label: "Probe".into(),
        },
        SchemaChange::DefineField {
            object: object("probe"),
            field: field_def("title", FieldType::Str, false),
        },
        SchemaChange::DefineField {
            object: object("probe"),
            field: FieldDef {
                default: Some(overflowing),
                ..field_def("count", FieldType::Int, false)
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
        .expect("the owner defines the probe type")
        .0
}

/// `who` (a principal of `alpha`) creates a record of `object_name` with a title.
fn attempt(world: &World, who: &str, object_name: &str, now: i64) -> Result<(), DomainError> {
    let intent = Intent::CreateRecord {
        object: object(object_name),
        fields: [(field("title"), text("hello"))].into_iter().collect(),
    };
    world
        .step(&intent, &ctx("alpha", user(&format!("alpha:{who}")), now))
        .map(|_| ())
}

#[test]
fn an_unauthorised_create_is_refused_before_a_failing_default_is_evaluated() {
    let world = world_with_failing_default();

    // The owner may create, so the failing default is reached and reported.
    let owner = attempt(&world, "owner", "probe", 9_100).expect_err("the default overflows");
    assert_eq!(owner.kind, ErrorKind::Evaluation, "{}", owner.message);

    // A stranger may not create on `probe`: `unauthorized`, not the default's evaluation error.
    let stranger = attempt(&world, "stranger", "probe", 9_101).expect_err("refused");
    assert_eq!(
        stranger.kind,
        ErrorKind::Unauthorized,
        "{}",
        stranger.message
    );

    // The same for a type that does not exist: the stranger learns nothing about the schema.
    let missing = attempt(&world, "stranger", "ghost", 9_102).expect_err("refused");
    assert_eq!(missing.kind, ErrorKind::Unauthorized, "{}", missing.message);
}
