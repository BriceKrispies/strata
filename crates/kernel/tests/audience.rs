//! Audience classes (D-15, CPY-005, CPY-006, AGG-033, D-5065): which readers are indistinguishable
//! for a result, derived from the policies of a tenant, and what the object types of an aggregate
//! depend on (AGG-042).
//!
//! The tenant has a `note` object type with a title, a status, a rank and a score, the roles
//! `writer`, `editor` and `auditor`, and a query `stats` that declares aggregates over the notes.
//! Each test states which Read policies and field read rules the tenant has, because those are what
//! the classes are derived from.

use proptest::prelude::*;
use std::collections::BTreeSet;
use strata_kernel::audience::{class_of, AudienceClass, Reading};
use strata_kernel::testkit::*;
use strata_kernel::*;

const T: &str = "alpha";

fn boss(tenant: &str) -> Principal {
    user(&format!("{tenant}:owner"))
}

fn member(tenant: &str, i: usize) -> Principal {
    user(&format!("{tenant}:u{i}"))
}

fn aggregate(
    id: &str,
    kind: AggregateKind,
    field_name: Option<&str>,
    group: Option<&str>,
) -> AggregateDef {
    AggregateDef {
        id: AggregateId::new(id).unwrap(),
        kind,
        field: field_name.map(field),
        group_by: group.map(field),
        group_by_second: None,
        n: (kind == AggregateKind::Top).then_some(2),
        descending: true,
        scan_bound: 1_000,
    }
}

fn stats_query(declared: &[&str]) -> QueryDef {
    use AggregateKind::*;
    let mut aggregates = vec![
        aggregate("count_all", Count, None, None),
        aggregate("sum_rank", Sum, Some("rank"), None),
        aggregate("by_status", Count, None, Some("status")),
        aggregate("top_rank", Top, Some("rank"), None),
    ];
    // An aggregate may only name a declared field.
    if declared.contains(&"score") {
        aggregates.push(aggregate("max_score", Maximum, Some("score"), None));
    }
    QueryDef {
        id: QueryId::new("stats").unwrap(),
        object: object("note"),
        filter: None,
        params: BTreeSet::new(),
        order: vec![],
        limit: None,
        aggregates: Some(aggregates),
        declared_fields: Some(declared.iter().map(|f| field(f)).collect()),
        tree: None,
    }
}

fn plain_query() -> QueryDef {
    QueryDef {
        id: QueryId::new("plain").unwrap(),
        object: object("note"),
        filter: None,
        params: BTreeSet::new(),
        order: vec![],
        limit: None,
        aggregates: None,
        declared_fields: None,
        tree: None,
    }
}

fn policy(id: &str, role_name: &str, actions: &[Action], condition: Option<Expr>) -> SchemaChange {
    SchemaChange::DefinePolicy(PolicyDef {
        id: PolicyId::new(id).unwrap(),
        role: role(role_name),
        resource: Resource::Object(object("note")),
        actions: actions.iter().copied().collect(),
        condition,
    })
}

fn published() -> Expr {
    eq(var(Scope::Record, "status"), lit("published"))
}

fn own() -> Expr {
    eq(var(Scope::Record, "$created_by"), Expr::Principal)
}

/// What a tenant's Read policies and the read rule of the `score` field are.
#[derive(Clone, Copy, Debug)]
struct Shape {
    /// What the public reads: 0 nothing, 1 published notes.
    public_reads: u8,
    /// What writers read: 0 nothing, 1 every note, 2 their own notes.
    writer_reads: u8,
    /// Whether editors read every note.
    editor_reads: bool,
    /// What auditors read: 0 nothing, 1 every note, 2 published notes.
    auditor_reads: u8,
    /// Who may read `score`: 0 everyone, 1 editors, 2 auditors, 3 the creator of the note.
    score_rule: u8,
}

const TYPICAL: Shape = Shape {
    public_reads: 1,
    writer_reads: 1,
    editor_reads: true,
    auditor_reads: 0,
    score_rule: 1,
};

/// Which of `writer`, `editor` and `auditor` (bits 0, 1, 2) each of the four members holds.
type Roles = [u8; 4];

const NAMES: [&str; 3] = ["writer", "editor", "auditor"];

fn score_rule(rule: u8) -> Option<Expr> {
    let either = |role_name: &str| {
        Expr::Or(vec![
            Expr::HasRole(role(role_name)),
            Expr::HasRole(owner_role()),
        ])
    };
    match rule {
        0 => None,
        1 => Some(either("editor")),
        2 => Some(either("auditor")),
        _ => Some(Expr::Or(vec![Expr::HasRole(owner_role()), own()])),
    }
}

/// The schema changes of a tenant of this shape, with the aggregates of `stats` declaring
/// `declared`, and `extra` after them.
fn changes(shape: Shape, declared: &[&str], extra: Vec<SchemaChange>) -> Vec<SchemaChange> {
    let mut changes: Vec<SchemaChange> = NAMES
        .iter()
        .map(|name| {
            SchemaChange::DefineRole(RoleDef {
                id: role(name),
                label: name.to_string(),
            })
        })
        .collect();
    changes.extend([
        SchemaChange::DefineObject {
            id: object("note"),
            label: "Note".into(),
        },
        SchemaChange::DefineField {
            object: object("note"),
            field: field_def("title", FieldType::Str, true),
        },
        SchemaChange::DefineField {
            object: object("note"),
            field: FieldDef {
                default: Some(lit("draft")),
                ..field_def("status", FieldType::Str, true)
            },
        },
        SchemaChange::DefineField {
            object: object("note"),
            field: field_def("rank", FieldType::Int, false),
        },
        SchemaChange::DefineField {
            object: object("note"),
            field: FieldDef {
                read: score_rule(shape.score_rule),
                ..field_def("score", FieldType::Int, false)
            },
        },
        // Anyone may write notes; this test is about who may read them.
        policy("anyone_creates", "public", &[Action::Create], None),
    ]);
    if shape.public_reads == 1 {
        changes.push(policy(
            "public_reads",
            "public",
            &[Action::Read],
            Some(published()),
        ));
    }
    match shape.writer_reads {
        1 => changes.push(policy("writer_reads", "writer", &[Action::Read], None)),
        2 => changes.push(policy(
            "writer_reads",
            "writer",
            &[Action::Read],
            Some(own()),
        )),
        _ => {}
    }
    if shape.editor_reads {
        changes.push(policy("editor_reads", "editor", &[Action::Read], None));
    }
    match shape.auditor_reads {
        1 => changes.push(policy("auditor_reads", "auditor", &[Action::Read], None)),
        2 => changes.push(policy(
            "auditor_reads",
            "auditor",
            &[Action::Read],
            Some(published()),
        )),
        _ => {}
    }
    changes.push(SchemaChange::DefineQuery(stats_query(declared)));
    changes.push(SchemaChange::DefineQuery(plain_query()));
    changes.extend(extra);
    changes
}

const ALL_FIELDS: [&str; 3] = ["rank", "status", "score"];

/// A tenant of this shape whose four members hold the given roles.
fn tenant_of(
    tenant: &str,
    shape: Shape,
    held: Roles,
    declared: &[&str],
    extra: Vec<SchemaChange>,
) -> World {
    let mut now = 0;
    let mut step = |world: &World, who: Principal, intent: Intent| {
        now += 1;
        world
            .step(&intent, &ctx(tenant, who, now))
            .unwrap_or_else(|e| panic!("fixture refused: {}", e.describe()))
            .0
    };
    let mut world = step(
        &World::default(),
        Principal::System,
        Intent::CreateTenant {
            name: tenant.to_string(),
            owner: principal_id(&format!("{tenant}:owner")),
        },
    );
    world = step(
        &world,
        boss(tenant),
        Intent::ChangeSchema {
            changes: changes(shape, declared, extra),
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    for (i, mask) in held.iter().enumerate() {
        for (bit, name) in NAMES.iter().enumerate() {
            if mask & (1 << bit) != 0 {
                world = step(
                    &world,
                    boss(tenant),
                    Intent::AssignRole {
                        principal: principal_id(&format!("{tenant}:u{i}")),
                        role: role(name),
                    },
                );
            }
        }
    }
    world
}

fn typical(held: Roles) -> World {
    tenant_of(T, TYPICAL, held, &ALL_FIELDS, vec![])
}

fn tenant_state(world: &World, tenant: &str) -> State {
    world.state(&tenant_id(tenant))
}

fn class(world: &World, tenant: &str, aggregate_id: &str, who: &Principal) -> AudienceClass {
    let state = tenant_state(world, tenant);
    let query = QueryId::new("stats").unwrap();
    let aggregate = AggregateId::new(aggregate_id).unwrap();
    class_of(
        state.tenant.as_deref().expect("the tenant exists"),
        Reading::Aggregate {
            query: &query,
            aggregate: &aggregate,
        },
        who,
    )
}

fn roles_class(names: &[&str]) -> AudienceClass {
    AudienceClass::Roles(names.iter().map(|n| role(n)).collect())
}

// Verifies: AGG-033
// Verifies: AGG-2860
// Verifies: CPY-005
// Verifies: CPY-006
#[test]
fn two_readers_with_the_same_roles_share_a_class_and_a_reader_with_another_role_does_not() {
    // u0 and u1 are writers, u2 is an editor, u3 holds no role.
    let world = typical([0b001, 0b001, 0b010, 0]);
    let writer_a = class(&world, T, "count_all", &member(T, 0));
    let writer_b = class(&world, T, "count_all", &member(T, 1));
    let editor = class(&world, T, "count_all", &member(T, 2));
    let nobody = class(&world, T, "count_all", &member(T, 3));
    let public = class(&world, T, "count_all", &Principal::Anonymous);

    assert_eq!(writer_a, roles_class(&["public", "writer"]));
    assert_eq!(writer_a, writer_b);
    assert_ne!(writer_a, editor);
    assert_eq!(editor, roles_class(&["public", "editor"]));
    assert_ne!(writer_a, nobody);
    // A signed-in person with no role and the anonymous public are told the same.
    assert_eq!(nobody, roles_class(&["public"]));
    assert_eq!(nobody, public);
    assert_eq!(
        class(&world, T, "count_all", &boss(T)),
        roles_class(&["owner", "public"])
    );
    assert!(writer_a.is_shareable() && editor.is_shareable() && public.is_shareable());
    // The same roles give the same key, and other roles give another.
    assert_eq!(writer_a.key(), writer_b.key());
    assert_ne!(writer_a.key(), editor.key());
    assert_eq!(
        class(&world, T, "count_all", &Principal::System),
        AudienceClass::System
    );
}

// ---- the property: a result stored for a class is the result computed for any reader of it ----------

proptest! {
    // A fixed, recorded seed: a run is the same run every time, and a failure reproduces.
    #![proptest_config(ProptestConfig {
        cases: 48,
        rng_seed: proptest::test_runner::RngSeed::Fixed(20261121),
        ..ProptestConfig::default()
    })]

}
