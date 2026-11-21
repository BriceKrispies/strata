//! Audience classes (D-15, CPY-005, CPY-006, AGG-033, D-5065): which readers are indistinguishable
//! for a result, derived from the policies of a tenant, and what the object types of an aggregate
//! depend on (AGG-042).
//!
//! The tenant has a `note` object type with a title, a status, a rank and a score, the roles
//! `writer`, `editor` and `auditor`, and a query `stats` that declares aggregates over the notes.
//! Each test states which Read policies and field read rules the tenant has, because those are what
//! the classes are derived from.

use proptest::prelude::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use strata_kernel::audience::{audience_class, class_of, AudienceClass, Reading};
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

/// One note, created as `who`.
fn add_note(
    world: &World,
    tenant: &str,
    who: Principal,
    now: i64,
    rank: i64,
    shown: bool,
) -> World {
    let fields = BTreeMap::from([
        (field("title"), text("t")),
        (
            field("status"),
            text(if shown { "published" } else { "draft" }),
        ),
        (field("rank"), Value::Int(rank)),
        (field("score"), Value::Int(rank * 2)),
    ]);
    world
        .step(
            &Intent::CreateRecord {
                object: object("note"),
                fields,
            },
            &ctx(tenant, who, 1_000 + now),
        )
        .unwrap_or_else(|e| panic!("note refused: {}", e.describe()))
        .0
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

// Verifies: AGG-2860
// Verifies: AGG-2861
#[test]
fn a_policy_that_mentions_the_reader_gives_each_holder_of_its_role_a_class_of_their_own() {
    let shape = Shape {
        writer_reads: 2,
        ..TYPICAL
    };
    let world = tenant_of(T, shape, [0b001, 0b001, 0b010, 0], &ALL_FIELDS, vec![]);
    let a = class(&world, T, "count_all", &member(T, 0));
    let b = class(&world, T, "count_all", &member(T, 1));
    assert_eq!(a, AudienceClass::Individual(member(T, 0)));
    assert_eq!(b, AudienceClass::Individual(member(T, 1)));
    assert_ne!(a, b);
    assert!(!a.is_shareable());
    assert_eq!(a.key(), None, "an individual class is never stored under");
    // A reader who does not hold the role is not judged by its policy, and shares as before.
    assert_eq!(
        class(&world, T, "count_all", &member(T, 2)),
        roles_class(&["editor", "public"])
    );
    assert_eq!(
        class(&world, T, "count_all", &Principal::Anonymous),
        roles_class(&["public"])
    );
    // A writer's other aggregates are individual as well: the policy is about the note.
    assert_eq!(
        class(&world, T, "by_status", &member(T, 0)),
        AudienceClass::Individual(member(T, 0))
    );
}

#[test]
fn a_field_read_rule_that_mentions_the_reader_makes_every_reader_individual() {
    let shape = Shape {
        score_rule: 3,
        ..TYPICAL
    };
    let world = tenant_of(T, shape, [0b001, 0, 0, 0], &ALL_FIELDS, vec![]);
    for who in [member(T, 0), member(T, 3), Principal::Anonymous, boss(T)] {
        assert_eq!(
            class(&world, T, "max_score", &who),
            AudienceClass::Individual(who.clone())
        );
    }
    // The rule is on a field the query declares, so every aggregate of the query carries it.
    assert!(!class(&world, T, "count_all", &member(T, 3)).is_shareable());
    // A query that declares no such field, and an aggregate that names none, is not affected.
    let narrow = tenant_of(T, shape, [0b001, 0, 0, 0], &["rank", "status"], vec![]);
    assert_eq!(
        class(&narrow, T, "count_all", &member(T, 3)),
        roles_class(&["public"])
    );
}

// Verifies: AGG-033
#[test]
fn a_field_read_rule_that_differs_between_two_roles_splits_their_classes() {
    // The auditor role reads no note by any policy; only the read rule of `score` names it.
    // u0 holds it, u3 does not.
    let held = [0b100, 0, 0, 0];
    let declared_score = tenant_of(
        T,
        Shape {
            score_rule: 2,
            ..TYPICAL
        },
        held,
        &ALL_FIELDS,
        vec![],
    );
    let auditor = class(&declared_score, T, "count_all", &member(T, 0));
    let nobody = class(&declared_score, T, "count_all", &member(T, 3));
    assert_eq!(auditor, roles_class(&["auditor", "public"]));
    assert_eq!(nobody, roles_class(&["public"]));
    assert_ne!(auditor, nobody, "the rule gives them different answers");

    // Without the rule, or when the aggregate cannot carry the field, nothing tells them apart.
    let no_rule = tenant_of(
        T,
        Shape {
            score_rule: 0,
            ..TYPICAL
        },
        held,
        &ALL_FIELDS,
        vec![],
    );
    assert_eq!(
        class(&no_rule, T, "count_all", &member(T, 0)),
        class(&no_rule, T, "count_all", &member(T, 3))
    );
    let not_declared = tenant_of(
        T,
        Shape {
            score_rule: 2,
            ..TYPICAL
        },
        held,
        &["rank", "status"],
        vec![],
    );
    assert_eq!(
        class(&not_declared, T, "count_all", &member(T, 0)),
        class(&not_declared, T, "count_all", &member(T, 3))
    );
}

#[test]
fn what_the_kernel_cannot_resolve_is_individual() {
    let world = typical([0b001, 0, 0, 0]);
    let state = tenant_state(&world, T);
    let tenant = state.tenant.as_deref().unwrap();
    let nothing = QueryId::new("nothing").unwrap();
    let count = AggregateId::new("count_all").unwrap();
    let who = member(T, 0);
    let unknown_query = class_of(
        tenant,
        Reading::Aggregate {
            query: &nothing,
            aggregate: &count,
        },
        &who,
    );
    assert_eq!(unknown_query, AudienceClass::Individual(who.clone()));
    let stats = QueryId::new("stats").unwrap();
    let missing = AggregateId::new("missing").unwrap();
    let unknown_aggregate = class_of(
        tenant,
        Reading::Aggregate {
            query: &stats,
            aggregate: &missing,
        },
        &who,
    );
    assert_eq!(unknown_aggregate, AudienceClass::Individual(who.clone()));
    let view = ViewId::new("nothing").unwrap();
    assert_eq!(
        class_of(tenant, Reading::View(&view), &who),
        AudienceClass::Individual(who.clone())
    );
    assert_eq!(
        class_of(tenant, Reading::Query(&nothing), &who),
        AudienceClass::Individual(who)
    );
}

#[test]
fn a_named_query_and_a_view_have_the_class_of_everything_their_rows_carry() {
    let board = ViewDef {
        id: ViewId::new("board").unwrap(),
        route: "/board".into(),
        title: "Board".into(),
        sections: BTreeMap::from([(
            "rows".to_string(),
            ViewSection {
                query: QueryId::new("plain").unwrap(),
                args: BTreeMap::new(),
                include: vec![],
                markdown: vec![],
                single: false,
                aggregate: None,
            },
        )]),
    };
    let counts = ViewDef {
        id: ViewId::new("counts").unwrap(),
        route: "/counts".into(),
        title: "Counts".into(),
        sections: BTreeMap::from([(
            "total".to_string(),
            ViewSection {
                query: QueryId::new("stats").unwrap(),
                args: BTreeMap::new(),
                include: vec![],
                markdown: vec![],
                single: false,
                aggregate: Some(AggregateId::new("count_all").unwrap()),
            },
        )]),
    };
    // Auditors read `score` and nothing else of interest; the aggregate of `counts` declares only
    // fields without a rule, while the rows of `board` carry every field.
    let shape = Shape {
        score_rule: 2,
        ..TYPICAL
    };
    let world = tenant_of(
        T,
        shape,
        [0b100, 0, 0, 0],
        &["rank", "status"],
        vec![
            SchemaChange::DefineView(board),
            SchemaChange::DefineView(counts),
        ],
    );
    let state = tenant_state(&world, T);
    let tenant = state.tenant.as_deref().unwrap();
    let (auditor, nobody) = (member(T, 0), member(T, 3));
    let plain = QueryId::new("plain").unwrap();
    let board_id = ViewId::new("board").unwrap();
    let counts_id = ViewId::new("counts").unwrap();

    assert_eq!(
        class_of(tenant, Reading::Query(&plain), &auditor),
        roles_class(&["auditor", "public"])
    );
    assert_eq!(
        class_of(tenant, Reading::View(&board_id), &auditor),
        class_of(tenant, Reading::Query(&plain), &auditor)
    );
    assert_ne!(
        class_of(tenant, Reading::View(&board_id), &auditor),
        class_of(tenant, Reading::View(&board_id), &nobody)
    );
    assert_eq!(
        class_of(tenant, Reading::View(&counts_id), &auditor),
        class_of(tenant, Reading::View(&counts_id), &nobody),
        "the aggregate section carries no field the auditor role reads"
    );
}

// Verifies: AGG-042
#[test]
fn the_read_policies_of_the_object_type_are_followed_into_the_types_they_depend_on() {
    let stats = QueryId::new("stats").unwrap();
    let count = AggregateId::new("count_all").unwrap();
    let alone = typical([0, 0, 0, 0]);
    let schema = alone.tenants[&tenant_id(T)].schema.clone();
    assert_eq!(
        aggregates::dependencies(&schema, &stats, &count),
        Some(BTreeSet::from([object("note")])),
        "no policy follows a reference, so the object type stands alone"
    );
    assert_eq!(
        aggregates::dependencies(&schema, &QueryId::new("nothing").unwrap(), &count),
        None
    );

    // Writers read the notes of a person called Ada only: the policy follows the author reference.
    let follows = vec![
        SchemaChange::DefineObject {
            id: object("person"),
            label: "Person".into(),
        },
        SchemaChange::DefineField {
            object: object("person"),
            field: field_def("name", FieldType::Str, true),
        },
        SchemaChange::DefineRelationship {
            object: object("note"),
            field: field("author"),
            label: "Author".into(),
            target: object("person"),
            on_delete: OnDelete::SetNull,
            required: false,
        },
        policy(
            "writers_read_ada",
            "writer",
            &[Action::Read],
            Some(eq(
                Expr::Through(Scope::Record, "author".into(), "name".into()),
                lit("Ada"),
            )),
        ),
    ];
    let world = tenant_of(T, TYPICAL, [0b001, 0, 0, 0], &ALL_FIELDS, follows);
    let schema = world.tenants[&tenant_id(T)].schema.clone();
    let wanted = BTreeSet::from([object("note"), object("person")]);
    assert_eq!(
        aggregates::dependencies(&schema, &stats, &count),
        Some(wanted.clone())
    );
    // A result names the same set, so the key a copy is stored under and the result agree.
    let state = world.state(&tenant_id(T));
    let result = aggregates::run(
        &state,
        &AggregateRequest {
            query: stats,
            aggregate: count,
            params: BTreeMap::new(),
            filter: BTreeMap::new(),
        },
        &ctx(T, boss(T), 90_000),
    )
    .unwrap_or_else(|e| panic!("{}", e.describe()));
    assert_eq!(result.depends_on, wanted);
}

#[test]
fn the_class_of_a_reader_follows_the_policies_of_the_tenant_it_reads() {
    // The same person, the same role, two tenants: one reads her own notes, the other every note.
    let own_only = Shape {
        writer_reads: 2,
        ..TYPICAL
    };
    let alpha = tenant_of("alpha", own_only, [0b001, 0, 0, 0], &ALL_FIELDS, vec![]);
    let beta = tenant_of("beta", TYPICAL, [0b001, 0, 0, 0], &ALL_FIELDS, vec![]);
    let in_alpha = class(&alpha, "alpha", "count_all", &member("alpha", 0));
    let in_beta = class(&beta, "beta", "count_all", &member("beta", 0));
    assert_eq!(in_alpha, AudienceClass::Individual(member("alpha", 0)));
    assert_eq!(in_beta, roles_class(&["public", "writer"]));
    // A class says who may share a result inside one tenant; the tenant is the first part of every
    // key a result is stored under, and a snapshot of another tenant answers nothing.
    let state = beta.state(&tenant_id("beta"));
    let error = aggregates::run(
        &state,
        &AggregateRequest {
            query: QueryId::new("stats").unwrap(),
            aggregate: AggregateId::new("count_all").unwrap(),
            params: BTreeMap::new(),
            filter: BTreeMap::new(),
        },
        &ctx("alpha", boss("alpha"), 90_000),
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::TenantMismatch);
}

// ---- the property: a result stored for a class is the result computed for any reader of it ----------

fn shape_strategy() -> impl Strategy<Value = Shape> {
    (0u8..2, 0u8..3, any::<bool>(), 0u8..3, 0u8..4).prop_map(
        |(public_reads, writer_reads, editor_reads, auditor_reads, score_rule)| Shape {
            public_reads,
            writer_reads,
            editor_reads,
            auditor_reads,
            score_rule,
        },
    )
}

proptest! {
    // A fixed, recorded seed: a run is the same run every time, and a failure reproduces.
    #![proptest_config(ProptestConfig {
        cases: 48,
        rng_seed: proptest::test_runner::RngSeed::Fixed(20261121),
        ..ProptestConfig::default()
    })]

    /// For a random schema, random roles and random records, a result held for the class of one
    /// reader equals the result computed for any other reader who has that class, after every
    /// write; and a reader is individual exactly when a condition that applies to them mentions
    /// the reader.
    /// Verifies: AGG-032
    /// Verifies: AGG-033
    #[test]
    fn a_result_held_for_a_class_equals_the_result_computed_fresh_for_each_reader_of_it(
        shape in shape_strategy(),
        held in proptest::array::uniform4(0u8..8),
        notes in proptest::collection::vec((0usize..5, 0i64..9, any::<bool>()), 1..6),
    ) {
        let mut world = tenant_of(T, shape, held, &ALL_FIELDS, vec![]);
        let readers: Vec<Principal> = std::iter::once(boss(T))
            .chain((0..4).map(|i| member(T, i)))
            .chain([Principal::Anonymous])
            .collect();
        let query = QueryId::new("stats").unwrap();
        // What the store would hold: by class key, aggregate and revision of the tenant.
        let mut held_copies: HashMap<(String, String, u64), AggregateResult> = HashMap::new();
        for (step, (creator, rank, shown)) in notes.iter().enumerate() {
            let who = if *creator == 4 { boss(T) } else { member(T, *creator) };
            world = add_note(&world, T, who, step as i64, *rank, *shown);
            let state = tenant_state(&world, T);
            let tenant = state.tenant.as_deref().unwrap();
            for (index, reader) in readers.iter().enumerate() {
                let holds = |bit: u8| index >= 1 && index <= 4 && held[index - 1] & (1 << bit) != 0;
                let should_be_individual = shape.score_rule == 3 || (shape.writer_reads == 2 && holds(0));
                for aggregate_id in ["count_all", "sum_rank", "by_status", "max_score", "top_rank"] {
                    let aggregate = AggregateId::new(aggregate_id).unwrap();
                    let class = class_of(
                        tenant,
                        Reading::Aggregate { query: &query, aggregate: &aggregate },
                        reader,
                    );
                    prop_assert_eq!(!class.is_shareable(), should_be_individual, "{:?} as {:?}", shape, reader);
                    let request = AggregateRequest {
                        query: query.clone(),
                        aggregate: aggregate.clone(),
                        params: BTreeMap::new(),
                        filter: BTreeMap::new(),
                    };
                    let fresh = aggregates::run(&state, &request, &ctx(T, reader.clone(), 90_000)).unwrap();
                    if let Some(key) = class.key() {
                        let slot = (key, aggregate_id.to_string(), tenant.revision.0);
                        match held_copies.get(&slot) {
                            Some(copy) => prop_assert_eq!(copy, &fresh, "{:?} as {:?}", shape, reader),
                            None => { held_copies.insert(slot, fresh); }
                        }
                    }
                }
            }
        }
    }
}
