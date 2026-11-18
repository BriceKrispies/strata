//! K5 as a state-machine property: whatever sequence of intents is thrown at the kernel, from
//! whichever starting world, every state it accepts satisfies every invariant, as long as no
//! schema change that left records non-conforming was grandfathered in. The sequences draw from
//! all seven intents: create a tenant, change the schema (strict and grandfathered), assign and
//! revoke a role, create, update and delete a record.

use proptest::prelude::*;
use std::collections::BTreeMap;
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_verification::invariants::check_world;

/// Number of starting worlds `starting_worlds` returns.
const WORLDS: usize = 5;

#[derive(Clone, Debug)]
enum Op {
    CreateTenant {
        name: String,
        owner: usize,
    },
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
    RequireRank {
        grandfather: bool,
    },
    TightenTitle {
        limit: i64,
        grandfather: bool,
    },
    Assign {
        who: usize,
        role: usize,
    },
    Revoke {
        who: usize,
        role: usize,
    },
}

const MEMBERS: [&str; 5] = ["owner", "editor", "writer1", "writer2", "stranger"];
const ROLES: [&str; 5] = ["owner", "editor", "writer", "ghost", "public"];

/// (tenant index, actor, operation). Actors 0..=3 are members, 4 a stranger, 5 an anonymous
/// visitor, 6 the system.
fn op() -> impl Strategy<Value = (usize, usize, Op)> {
    let status = prop_oneof![Just("draft"), Just("published"), Just("nonsense")];
    let op = prop_oneof![
        1 => ("[a-z]{0,4}", 0usize..2).prop_map(|(name, owner)| Op::CreateTenant { name, owner }),
        5 => ("[a-zA-Z ]{0,30}", 0u8..5, proptest::option::of(-2i64..5), proptest::option::of(1u64..8))
            .prop_map(|(title, slug, rank, author)| Op::Note { title, slug, rank, author }),
        2 => "[a-z]{0,5}".prop_map(Op::Person),
        5 => (1u64..10, proptest::option::of("[a-z]{0,30}"), proptest::option::of(status), proptest::option::of(1u64..8), any::<bool>())
            .prop_map(|(id, title, status, author, clear_rank)| Op::Edit { id, title, status, author, clear_rank }),
        3 => (prop_oneof![Just("note"), Just("person")], 1u64..10).prop_map(|(object, id)| Op::Delete { object, id }),
        1 => any::<bool>().prop_map(|grandfather| Op::RequireRank { grandfather }),
        1 => (3i64..30, any::<bool>()).prop_map(|(limit, grandfather)| Op::TightenTitle { limit, grandfather }),
        2 => (0usize..5, 0usize..5).prop_map(|(who, role)| Op::Assign { who, role }),
        2 => (0usize..5, 0usize..5).prop_map(|(who, role)| Op::Revoke { who, role }),
    ];
    // Owners and editors act most of the time, so that plenty is accepted.
    (0usize..3, 0usize..7, op)
}

fn intent(tenant: &str, op: &Op) -> Intent {
    let existing = |grandfather: bool| {
        if grandfather {
            ExistingData::Grandfather
        } else {
            ExistingData::Reject
        }
    };
    match op {
        Op::CreateTenant { name, owner } => Intent::CreateTenant {
            name: name.clone(),
            owner: principal_id(&format!("{tenant}:{}", MEMBERS[*owner])),
        },
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
        Op::RequireRank { grandfather } => Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineField {
                object: object("note"),
                field: field_def("rank", FieldType::Int, true),
            }],
            expected_version: None,
            existing: existing(*grandfather),
        },
        Op::TightenTitle { limit, grandfather } => Intent::ChangeSchema {
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
            existing: existing(*grandfather),
        },
        Op::Assign { who, role: r } => Intent::AssignRole {
            principal: principal_id(&format!("{tenant}:{}", MEMBERS[*who])),
            role: role(ROLES[*r]),
        },
        Op::Revoke { who, role: r } => Intent::RevokeRole {
            principal: principal_id(&format!("{tenant}:{}", MEMBERS[*who])),
            role: role(ROLES[*r]),
        },
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 160, ..ProptestConfig::default() })]

    /// Verifies: KRN-011
    #[test]
    fn every_accepted_state_satisfies_every_invariant(
        from in 0..WORLDS,
        ops in proptest::collection::vec(op(), 1..40),
    ) {
        let mut world = starting_worlds().swap_remove(from).1;
        prop_assert!(check_world(&world).iter().all(|i| i.holds));
        // Set once a grandfathered change has been accepted over records it leaves
        // non-conforming; the requirement exempts the states from then on.
        let mut grandfathered = false;
        for (step, (tenant, actor, op)) in ops.iter().enumerate() {
            let tenant = WORLD_TENANTS[*tenant];
            let principal = if *actor == 6 {
                Principal::System
            } else {
                principals(tenant)[*actor].clone()
            };
            let context = ctx(tenant, principal, 10_000 + step as i64);
            if let Ok((next, outcome)) = world.step(&intent(tenant, op), &context) {
                grandfathered |= outcome.events.iter().any(|event| {
                    matches!(event.body, EventBody::SchemaChanged { grandfathered: n, .. } if n > 0)
                });
                if !grandfathered {
                    for invariant in check_world(&next) {
                        prop_assert!(invariant.holds, "after step {} ({:?}): {} broken: {:?}", step, op, invariant.id, invariant.violations);
                    }
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
