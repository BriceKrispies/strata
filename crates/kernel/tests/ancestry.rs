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
    let mut fields = BTreeMap::from([(field("body"), text("a comment"))]);
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
