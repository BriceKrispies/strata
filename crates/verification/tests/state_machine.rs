//! K5 as a state-machine property: whatever sequence of intents is thrown at the kernel, every
//! state it accepts satisfies every invariant, as long as no schema change was grandfathered in.

use proptest::prelude::*;
use std::collections::BTreeMap;
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_verification::invariants::check_world;

#[derive(Clone, Debug)]
enum Op {
    Note {
        title: String,
        slug: u8,
        rank: Option<i64>,
        author: Option<u64>,
    },
    Person(String),
    Edit {
        id: u64,
        title: Option<String>,
        status: Option<&'static str>,
        author: Option<u64>,
        clear_rank: bool,
    },
    Delete {
        object: &'static str,
        id: u64,
    },
    RequireRank,
    TightenTitle(i64),
    Revoke(usize),
}

fn op() -> impl Strategy<Value = (usize, usize, Op)> {
    let status = prop_oneof![Just("draft"), Just("published"), Just("nonsense")];
    let op = prop_oneof![
        5 => ("[a-zA-Z ]{0,30}", 0u8..5, proptest::option::of(-2i64..5), proptest::option::of(1u64..8))
            .prop_map(|(title, slug, rank, author)| Op::Note { title, slug, rank, author }),
        2 => "[a-z]{0,5}".prop_map(Op::Person),
        5 => (1u64..10, proptest::option::of("[a-z]{0,30}"), proptest::option::of(status), proptest::option::of(1u64..8), any::<bool>())
            .prop_map(|(id, title, status, author, clear_rank)| Op::Edit { id, title, status, author, clear_rank }),
        3 => (prop_oneof![Just("note"), Just("person")], 1u64..10).prop_map(|(object, id)| Op::Delete { object, id }),
        1 => Just(Op::RequireRank),
        1 => (3i64..30).prop_map(Op::TightenTitle),
        1 => (0usize..4).prop_map(Op::Revoke),
    ];
    (0usize..2, 0usize..6, op)
}

fn intent(tenant: &str, op: &Op) -> Intent {
    match op {
        Op::Note {
            title,
            slug,
            rank,
            author,
        } => {
            let mut fields = BTreeMap::from([
                (field("title"), text(title)),
                (field("slug"), text(&format!("s{slug}"))),
            ]);
            if let Some(rank) = rank {
                fields.insert(field("rank"), Value::Int(*rank));
            }
            if let Some(author) = author {
                fields.insert(field("author"), Value::Ref(RecordId(*author)));
            }
            Intent::CreateRecord {
                object: object("note"),
                fields,
            }
        }
        Op::Person(name) => Intent::CreateRecord {
            object: object("person"),
            fields: BTreeMap::from([(field("name"), text(name))]),
        },
        Op::Edit {
            id,
            title,
            status,
            author,
            clear_rank,
        } => {
            let mut fields = BTreeMap::new();
            if let Some(title) = title {
                fields.insert(field("title"), text(title));
            }
            if let Some(status) = status {
                fields.insert(field("status"), text(status));
            }
            if let Some(author) = author {
                fields.insert(field("author"), Value::Ref(RecordId(*author)));
            }
            if *clear_rank {
                fields.insert(field("rank"), Value::Null);
            }
            Intent::UpdateRecord {
                object: object("note"),
                id: RecordId(*id),
                fields,
                expected_version: None,
            }
        }
        Op::Delete { object: name, id } => Intent::DeleteRecord {
            object: object(name),
            id: RecordId(*id),
            expected_version: None,
        },
        // Strict migrations only: a grandfathered one is allowed to leave non-conforming records.
        Op::RequireRank => Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineField {
                object: object("note"),
                field: field_def("rank", FieldType::Int, true),
            }],
            expected_version: None,
            existing: ExistingData::Reject,
        },
        Op::TightenTitle(limit) => Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineConstraint {
                object: object("note"),
                constraint: ConstraintDef {
                    id: ConstraintId::new("title_length").unwrap(),
                    expr: Expr::Cmp(
                        CmpOp::Le,
                        Box::new(Expr::Len(Box::new(var(Scope::New, "title")))),
                        Box::new(Expr::Lit(Value::Int(*limit))),
                    ),
                    message: "shorter".into(),
                },
            }],
            expected_version: None,
            existing: ExistingData::Reject,
        },
        Op::Revoke(who) => {
            let names = ["owner", "editor", "writer1", "writer2"];
            let roles = ["owner", "editor", "writer", "writer"];
            Intent::RevokeRole {
                principal: principal_id(&format!("{tenant}:{}", names[*who])),
                role: role(roles[*who]),
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    #[test]
    fn every_accepted_state_satisfies_every_invariant(ops in proptest::collection::vec(op(), 1..40)) {
        let mut world = notes_world();
        prop_assert!(check_world(&world).iter().all(|i| i.holds));
        for (step, (tenant, actor, op)) in ops.iter().enumerate() {
            let tenant = TENANTS[*tenant];
            // Owners and editors act most of the time, so that plenty is accepted.
            let principal = principals(tenant)[*actor].clone();
            let context = ctx(tenant, principal, 10_000 + step as i64);
            if let Ok((next, _)) = world.step(&intent(tenant, op), &context) {
                for invariant in check_world(&next) {
                    prop_assert!(invariant.holds, "after step {} ({:?}): {} broken: {:?}", step, op, invariant.id, invariant.violations);
                }
                world = next;
            }
        }
    }
}

#[test]
fn a_grandfathered_migration_is_reported_as_a_broken_invariant() {
    let world = notes_world();
    let (world, _) = world
        .step(
            &new_note("First", "first"),
            &ctx("alpha", user("alpha:writer1"), 5_000),
        )
        .unwrap();
    let grandfather = Intent::ChangeSchema {
        changes: vec![SchemaChange::DefineField {
            object: object("note"),
            field: field_def("rank", FieldType::Int, true),
        }],
        expected_version: None,
        existing: ExistingData::Grandfather,
    };
    let (world, _) = world
        .step(&grandfather, &ctx("alpha", user("alpha:owner"), 5_001))
        .unwrap();
    let broken: Vec<_> = check_world(&world)
        .into_iter()
        .filter(|i| !i.holds)
        .collect();
    assert_eq!(broken.len(), 1);
    assert_eq!(broken[0].id, "records_conform");
    assert_eq!(
        broken[0].violations,
        ["alpha: note/1: required:note.rank (a value is required)"]
    );
}
