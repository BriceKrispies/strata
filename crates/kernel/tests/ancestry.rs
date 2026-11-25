//! `ancestor_of` over a self-reference (WG-5510): the bounded walk, its limit and its cycle refusal,
//! and a subtree policy over a four-level tree.
//!
//! The walk is `ancestor_of(x, y, "comment", "parent")`: true when `x` is `y` or one of its ancestors,
//! following the single reference `parent` of the object type `comment`. It reads one record per hop and
//! takes at most `limits::MAX_ANCESTOR_HOPS` hops; a longer walk, and a walk that returns to a record it
//! visited, are errors, never a silent answer.
//!
//! The starting point is the notes tenant of `testkit`, with the object type `comment` added: `body`, and
//! the single reference `parent` to `comment`.

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_model_core::types::TreeDef;

const NOW: i64 = 20_000;

fn owner() -> Principal {
    user("alpha:owner")
}

fn step(world: &World, who: Principal, intent: Intent) -> Result<(World, Transition), DomainError> {
    world.step(&intent, &ctx("alpha", who, NOW))
}

fn define(world: &World, changes: Vec<SchemaChange>) -> World {
    step(
        world,
        owner(),
        Intent::ChangeSchema {
            changes,
            expected_version: None,
            existing: ExistingData::Reject,
        },
    )
    .expect("the schema change is accepted")
    .0
}

/// The notes tenant with the object type `comment`: `body`, and the reference `parent` to `comment`.
fn comment_world() -> World {
    define(
        &notes_world(),
        vec![
            SchemaChange::DefineObject {
                id: object("comment"),
                label: "Comment".into(),
            },
            SchemaChange::DefineField {
                object: object("comment"),
                field: field_def("body", FieldType::Str, false),
            },
            SchemaChange::DefineField {
                object: object("comment"),
                field: field_def("hidden", FieldType::Bool, true),
            },
            SchemaChange::DefineRelationship {
                object: object("comment"),
                field: field("parent"),
                label: "parent".into(),
                target: object("comment"),
                on_delete: OnDelete::Restrict,
                required: false,
            },
        ],
    )
}

/// Creates a comment under `parent` (or at the top) as the owner.
fn comment(world: &World, parent: Option<RecordId>) -> (World, RecordId) {
    comment_with(world, parent, false)
}

/// Creates a comment under `parent`, hidden or not.
fn comment_with(world: &World, parent: Option<RecordId>, hidden: bool) -> (World, RecordId) {
    let mut fields = BTreeMap::from([
        (field("body"), text("a comment")),
        (field("hidden"), Value::Bool(hidden)),
    ]);
    if let Some(parent) = parent {
        fields.insert(field("parent"), Value::Ref(parent));
    }
    let (world, outcome) = step(
        world,
        owner(),
        Intent::CreateRecord {
            object: object("comment"),
            fields,
        },
    )
    .expect("the comment is accepted");
    match outcome.output {
        Output::Record { id, .. } => (world, id),
        _ => panic!("a record was expected"),
    }
}

/// `ancestor_of(<root>, $id, "comment", "parent")`: the comments at or below `root`.
fn in_subtree_of(root: RecordId) -> Expr {
    Expr::AncestorOf(
        Box::new(Expr::Lit(Value::Ref(root))),
        Box::new(var(Scope::Record, "$id")),
        Box::new(lit("comment")),
        Box::new(lit("parent")),
    )
}

fn query(id: &str, filter: Option<Expr>) -> SchemaChange {
    SchemaChange::DefineQuery(QueryDef {
        id: QueryId::new(id).unwrap(),
        object: object("comment"),
        filter,
        params: BTreeSet::new(),
        order: vec![],
        limit: None,
        aggregates: None,
        declared_fields: None,
        tree: None,
    })
}

/// The ids of the rows of a named query, read as `who`.
fn ids(world: &World, who: Principal, query: &str) -> Result<BTreeSet<u64>, DomainError> {
    let request = ReadRequest::Named {
        query: QueryId::new(query).unwrap(),
        params: BTreeMap::new(),
        page_size: Some(200),
        cursor: None,
    };
    let result = read(&world.state(&tid()), &request, &ctx("alpha", who, NOW))?;
    Ok(result.rows.iter().map(|row| row.id.0).collect())
}

fn tid() -> TenantId {
    tenant_id("alpha")
}

/// A chain of `n` comments below a root, the deepest last: the root is `chain[0]`.
fn chain(world: &World, n: usize) -> (World, Vec<RecordId>) {
    let (mut world, root) = comment(world, None);
    let mut ids = vec![root];
    for _ in 1..n {
        let parent = *ids.last().unwrap();
        let (next, id) = comment(&world, Some(parent));
        world = next;
        ids.push(id);
    }
    (world, ids)
}

#[test]
fn a_record_is_its_own_ancestor_and_the_ancestors_of_its_chain_are_found() {
    let world = comment_world();
    let (world, nodes) = chain(&world, 3);
    let (world, outside) = comment(&world, None);
    let world = define(&world, vec![query("below_root", Some(in_subtree_of(nodes[0])))]);
    let found = ids(&world, owner(), "below_root").expect("the walk answers");
    let expected: BTreeSet<u64> = nodes.iter().map(|id| id.0).collect();
    assert_eq!(found, expected);
    assert!(!found.contains(&outside.0));
}

#[test]
fn a_walk_of_exactly_the_limit_answers_and_a_longer_one_is_refused() {
    let limit = limits::MAX_ANCESTOR_HOPS as usize;
    // The deepest record of a chain of limit + 1 comments is limit hops below the root.
    let world = comment_world();
    let (world, nodes) = chain(&world, limit + 1);
    let world = define(&world, vec![query("below_root", Some(in_subtree_of(nodes[0])))]);
    let found = ids(&world, owner(), "below_root").expect("a walk of the limit answers");
    assert_eq!(found.len(), limit + 1);

    // One more hop than the limit: the walk from the deepest record is refused, naming the limit.
    let world = comment_world();
    let (world, nodes) = chain(&world, limit + 2);
    let world = define(&world, vec![query("below_root", Some(in_subtree_of(nodes[0])))]);
    let refused = ids(&world, owner(), "below_root").expect_err("a longer walk is refused");
    assert!(
        refused.message.contains("longer than"),
        "the refusal names the limit: {}",
        refused.message
    );
}

#[test]
fn a_cycle_through_the_reference_is_refused_and_not_answered_as_false() {
    let world = comment_world();
    let (world, a) = comment(&world, None);
    let (world, b) = comment(&world, Some(a));
    // Close the loop: a's parent becomes b, so a and b reference each other.
    let (world, _) = step(
        &world,
        owner(),
        Intent::UpdateRecord {
            object: object("comment"),
            id: a,
            fields: BTreeMap::from([(field("parent"), Value::Ref(b))]),
            expected_version: None,
        },
    )
    .expect("the update is accepted: the schema does not refuse cycles");
    let (world, unrelated) = comment(&world, None);
    let world = define(&world, vec![query("below_unrelated", Some(in_subtree_of(unrelated)))]);
    let refused = ids(&world, owner(), "below_unrelated").expect_err("the cycle is refused");
    assert!(
        refused.message.contains("cycle"),
        "the refusal names the cycle: {}",
        refused.message
    );
}

#[test]
fn a_manager_reads_the_subtree_of_a_four_level_tree_and_nothing_above_it() {
    let world = comment_world();
    // root -> a -> b -> c, and a sibling d under root. The manager is a.
    let (world, root) = comment(&world, None);
    let (world, a) = comment(&world, Some(root));
    let (world, b) = comment(&world, Some(a));
    let (world, c) = comment(&world, Some(b));
    let (world, d) = comment(&world, Some(root));
    let manager = user("alpha:manager");
    let world = define(
        &world,
        vec![
            SchemaChange::DefineRole(RoleDef {
                id: role("manager"),
                label: "Manager".into(),
            }),
            SchemaChange::DefinePolicy(PolicyDef {
                id: PolicyId::new("manager_subtree").expect("policy id"),
                role: role("manager"),
                resource: Resource::Object(object("comment")),
                actions: BTreeSet::from([Action::Read]),
                condition: Some(Expr::AncestorOf(
                    Box::new(Expr::Lit(Value::Ref(a))),
                    Box::new(var(Scope::Record, "$id")),
                    Box::new(lit("comment")),
                    Box::new(lit("parent")),
                )),
            }),
            query("all_comments", None),
        ],
    );
    let (world, _) = step(
        &world,
        owner(),
        Intent::AssignRole {
            principal: principal_id("alpha:manager"),
            role: role("manager"),
        },
    )
    .expect("the role is assigned");
    let seen = ids(&world, manager, "all_comments").expect("the manager reads");
    let expected: BTreeSet<u64> = [a, b, c].iter().map(|id| id.0).collect();
    assert_eq!(seen, expected);
    assert!(!seen.contains(&root.0) && !seen.contains(&d.0));
}

/// A descendants query: the comments below the record the parameter `root` names, `max_depth` levels down.
fn descendants(id: &str, max_depth: u32) -> SchemaChange {
    SchemaChange::DefineQuery(QueryDef {
        id: QueryId::new(id).unwrap(),
        object: object("comment"),
        filter: None,
        params: BTreeSet::from(["root".to_string()]),
        order: vec![],
        limit: None,
        aggregates: None,
        declared_fields: None,
        tree: Some(TreeDef {
            via: "parent".to_string(),
            root: "root".to_string(),
            max_depth,
        }),
    })
}

/// The rows of a descendants query, as (id, level) pairs, read as `who` below `root`.
fn levels(
    world: &World,
    who: Principal,
    query: &str,
    root: RecordId,
) -> Result<Vec<(u64, u32)>, DomainError> {
    let request = ReadRequest::Named {
        query: QueryId::new(query).unwrap(),
        params: BTreeMap::from([("root".to_string(), Value::Ref(root))]),
        page_size: Some(200),
        cursor: None,
    };
    let result = read(&world.state(&tid()), &request, &ctx("alpha", who, NOW))?;
    let depths = result.depths.clone().expect("a descendants read gives depths");
    Ok(result
        .rows
        .iter()
        .map(|row| row.id.0)
        .zip(depths)
        .collect())
}

/// A manager of `a` reads the comments under it, and the condition also says a hidden comment is not
/// readable by the manager.
fn manager_of(a: RecordId) -> PolicyDef {
    PolicyDef {
        id: PolicyId::new("manager_subtree").expect("policy id"),
        role: role("manager"),
        resource: Resource::Object(object("comment")),
        actions: BTreeSet::from([Action::Read]),
        condition: Some(Expr::And(vec![
            Expr::AncestorOf(
                Box::new(Expr::Lit(Value::Ref(a))),
                Box::new(var(Scope::Record, "$id")),
                Box::new(lit("comment")),
                Box::new(lit("parent")),
            ),
            Expr::Not(Box::new(eq(var(Scope::Record, "hidden"), Expr::Lit(Value::Bool(true))))),
        ])),
    }
}

/// The comments under `root` and at which level, through the three shapes the tree read must answer:
/// the levels of a four-level tree, a refusal past the declared depth, and a cycle.
#[test]
fn a_descendants_query_gives_each_row_its_level_below_the_root() {
    let world = comment_world();
    // root -> a -> b -> c, and d under root.
    let (world, root) = comment(&world, None);
    let (world, a) = comment(&world, Some(root));
    let (world, b) = comment(&world, Some(a));
    let (world, c) = comment(&world, Some(b));
    let (world, d) = comment(&world, Some(root));
    let world = define(&world, vec![descendants("below", 3)]);
    let rows = levels(&world, owner(), "below", root).expect("the tree reads");
    let expected = vec![(a.0, 1), (d.0, 1), (b.0, 2), (c.0, 3)];
    assert_eq!(rows, expected, "level order, children in creation order");
}

#[test]
fn a_tree_deeper_than_the_declared_depth_is_refused_not_cut_short() {
    let world = comment_world();
    let (world, root) = comment(&world, None);
    let (world, a) = comment(&world, Some(root));
    let (world, b) = comment(&world, Some(a));
    let (world, _) = comment(&world, Some(b));
    let world = define(&world, vec![descendants("shallow", 2)]);
    let refused = levels(&world, owner(), "shallow", root).expect_err("the tree is deeper");
    assert!(
        refused.message.contains("deeper than 2 levels"),
        "the refusal names the depth: {}",
        refused.message
    );
}

#[test]
fn a_cycle_in_a_descendants_read_is_refused() {
    let world = comment_world();
    let (world, a) = comment(&world, None);
    let (world, b) = comment(&world, Some(a));
    let (world, _) = step(
        &world,
        owner(),
        Intent::UpdateRecord {
            object: object("comment"),
            id: a,
            fields: BTreeMap::from([(field("parent"), Value::Ref(b))]),
            expected_version: None,
        },
    )
    .expect("the update is accepted: the schema does not refuse cycles");
    let world = define(&world, vec![descendants("loop", 4)]);
    let refused = levels(&world, owner(), "loop", a).expect_err("the cycle is refused");
    assert!(
        refused.message.contains("cycle"),
        "the refusal names the cycle: {}",
        refused.message
    );
}

/// The manager's descendants read does not pass through a comment the manager may not read: with the
/// middle comment hidden, the comment below it is not returned either (WG-5510, the read rule).
#[test]
fn a_hidden_intermediate_comment_hides_what_is_below_it() {
    for (hide_middle, expected) in [(false, vec![(2, 1), (3, 2)]), (true, vec![])] {
        let world = comment_world();
        let (world, a) = comment(&world, None);
        let (world, b) = comment_with(&world, Some(a), hide_middle);
        let (world, c) = comment(&world, Some(b));
        let world = define(
            &world,
            vec![
                SchemaChange::DefineRole(RoleDef {
                    id: role("manager"),
                    label: "Manager".into(),
                }),
                SchemaChange::DefinePolicy(manager_of(a)),
                descendants("below", 3),
            ],
        );
        let (world, _) = step(
            &world,
            owner(),
            Intent::AssignRole {
                principal: principal_id("alpha:manager"),
                role: role("manager"),
            },
        )
        .expect("the role is assigned");
        let rows = levels(&world, user("alpha:manager"), "below", a).expect("the manager reads");
        let expected: Vec<(u64, u32)> = expected
            .into_iter()
            .map(|(which, level)| (if which == 2 { b.0 } else { c.0 }, level))
            .collect();
        assert_eq!(rows, expected, "hide_middle = {hide_middle}");
    }
}

/// The plan declares the reads a descendants query makes: the pair index of its parent field, ordered by
/// creation, which the storage keeps for the query.
#[test]
fn a_descendants_query_declares_the_pair_index_it_reads_each_level_through() {
    let world = define(&comment_world(), vec![descendants("below", 3)]);
    let schema = TenantSchema::clone(&world.tenants[&tid()].schema);
    let pairs = strata_kernel::query_plan::index_pairs(&schema);
    assert!(pairs.contains(&strata_kernel::query_plan::IndexPair {
        object: object("comment"),
        field: field("parent"),
        order: "$created_at".to_string(),
    }));
}
