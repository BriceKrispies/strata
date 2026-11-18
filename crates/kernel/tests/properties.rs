//! Property tests over random operation sequences on a two-tenant world.

use proptest::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use strata_kernel::testkit::*;
use strata_kernel::*;

/// One randomly chosen operation. Small ranges on purpose, so that collisions, conflicts and
/// references to missing records are common.
#[derive(Clone, Debug)]
struct Op {
    tenant: usize,
    actor_tenant: usize,
    actor: usize,
    kind: OpKind,
}

#[derive(Clone, Debug)]
enum OpKind {
    Create { title: String, slug: u8, rank: Option<i64>, author: Option<u64> },
    Person { name: String },
    Update { id: u64, title: Option<String>, status: Option<&'static str>, rank: Option<i64> },
    Delete { object: &'static str, id: u64 },
    RequireRank { grandfather: bool },
    Revoke { who: usize },
}

fn op_strategy() -> impl Strategy<Value = Op> {
    let status = prop_oneof![Just("draft"), Just("published"), Just("archived")];
    let kind = prop_oneof![
        4 => ("[a-zA-Z ]{0,30}", 0u8..6, proptest::option::of(0i64..5), proptest::option::of(1u64..6))
            .prop_map(|(title, slug, rank, author)| OpKind::Create { title, slug, rank, author }),
        1 => "[a-z]{1,6}".prop_map(|name| OpKind::Person { name }),
        4 => (1u64..8, proptest::option::of("[a-z]{0,30}"), proptest::option::of(status), proptest::option::of(0i64..5))
            .prop_map(|(id, title, status, rank)| OpKind::Update { id, title, status, rank }),
        2 => (prop_oneof![Just("note"), Just("person")], 1u64..8).prop_map(|(object, id)| OpKind::Delete { object, id }),
        1 => any::<bool>().prop_map(|grandfather| OpKind::RequireRank { grandfather }),
        1 => (0usize..5).prop_map(|who| OpKind::Revoke { who }),
    ];
    // Most actors belong to the tenant they act on; some come from the other tenant.
    (0usize..2, 0usize..2, 0usize..6, kind, 0u8..10).prop_map(|(tenant, other, actor, kind, cross)| Op {
        tenant,
        actor_tenant: if cross == 0 { other } else { tenant },
        actor,
        kind,
    })
}

fn intent_of(op: &Op) -> Intent {
    match &op.kind {
        OpKind::Create { title, slug, rank, author } => {
            let mut fields = BTreeMap::from([(field("title"), text(title)), (field("slug"), text(&format!("s{slug}")))]);
            if let Some(rank) = rank {
                fields.insert(field("rank"), Value::Int(*rank));
            }
            if let Some(author) = author {
                fields.insert(field("author"), Value::Ref(RecordId(*author)));
            }
            Intent::CreateRecord { object: object("note"), fields }
        }
        OpKind::Person { name } => {
            Intent::CreateRecord { object: object("person"), fields: BTreeMap::from([(field("name"), text(name))]) }
        }
        OpKind::Update { id, title, status, rank } => {
            let mut fields = BTreeMap::new();
            if let Some(title) = title {
                fields.insert(field("title"), text(title));
            }
            if let Some(status) = status {
                fields.insert(field("status"), text(status));
            }
            if let Some(rank) = rank {
                fields.insert(field("rank"), Value::Int(*rank));
            }
            Intent::UpdateRecord { object: object("note"), id: RecordId(*id), fields, expected_version: None }
        }
        OpKind::Delete { object: name, id } => {
            Intent::DeleteRecord { object: object(name), id: RecordId(*id), expected_version: None }
        }
        OpKind::RequireRank { grandfather } => Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineField { object: object("note"), field: field_def("rank", FieldType::Int, true) }],
            expected_version: None,
            existing: if *grandfather { ExistingData::Grandfather } else { ExistingData::Reject },
        },
        OpKind::Revoke { who } => {
            let names = ["owner", "editor", "writer1", "writer2", "stranger"];
            let roles = ["owner", "editor", "writer", "writer", "writer"];
            Intent::RevokeRole {
                principal: principal_id(&format!("{}:{}", TENANTS[op.tenant], names[*who])),
                role: role(roles[*who]),
            }
        }
    }
}

fn context_of(op: &Op, now: i64) -> Context {
    let principal = principals(TENANTS[op.actor_tenant])[op.actor].clone();
    ctx(TENANTS[op.tenant], principal, now)
}

/// The partial snapshot a store would build from a list of needs.
fn partial(tenant: &Arc<TenantState>, wanted: &BTreeSet<Need>) -> State {
    let mut thin = TenantState::clone(tenant);
    thin.records.clear();
    for need in wanted {
        let matching: Vec<Arc<Record>> = match need {
            Need::Record { object, id } => tenant.record(object, *id).into_iter().cloned().collect(),
            Need::AllOf(object) => tenant.records_of(object).cloned().collect(),
            Need::ByField { object, field, value } => tenant
                .records_of(object)
                .filter(|r| match r.get(field) {
                    Value::List(items) => items.contains(value),
                    other => other == value,
                })
                .cloned()
                .collect(),
        };
        for record in matching {
            thin.records.entry(record.object.clone()).or_default().insert(record.id, record);
        }
    }
    State { tenant_id: tenant.id.clone(), tenant: Some(Arc::new(thin)), coverage: Coverage::Partial(wanted.clone()) }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, ..ProptestConfig::default() })]

    /// K2. The same state, intent and context always give the same result, and equal worlds
    /// have equal hashes.
    #[test]
    fn transitions_are_deterministic(ops in proptest::collection::vec(op_strategy(), 1..30)) {
        let mut world = notes_world();
        for (i, op) in ops.iter().enumerate() {
            let (intent, context) = (intent_of(op), context_of(op, 10_000 + i as i64));
            let first = world.step(&intent, &context);
            let second = world.step(&intent, &context);
            prop_assert_eq!(&first, &second);
            if let Ok((next, outcome)) = first {
                prop_assert_eq!(canon::hash_of(&outcome), canon::hash_of(&second.unwrap().1));
                world = next;
            }
        }
        // A world rebuilt from its canonical form is the same world.
        let rebuilt: World = serde_json::from_str(&canon::canonical_json(&world)).unwrap();
        prop_assert_eq!(rebuilt.hash(), world.hash());
    }

    /// K3. A step in one tenant leaves every other tenant exactly as it was, and a rejected
    /// step leaves everything as it was.
    #[test]
    fn a_step_never_touches_another_tenant(ops in proptest::collection::vec(op_strategy(), 1..30)) {
        let mut world = notes_world();
        for (i, op) in ops.iter().enumerate() {
            let other = tenant_id(TENANTS[1 - op.tenant]);
            let before = world.tenant_hash(&other);
            if let Ok((next, outcome)) = world.step(&intent_of(op), &context_of(op, 10_000 + i as i64)) {
                prop_assert_eq!(&outcome.delta.tenant, &tenant_id(TENANTS[op.tenant]));
                prop_assert!(outcome.events.iter().all(|e| e.tenant == outcome.delta.tenant));
                world = next;
            }
            prop_assert_eq!(world.tenant_hash(&other), before);
        }
    }

    /// K4. Anonymous visitors, strangers and members of another tenant never change anything.
    #[test]
    fn outsiders_cannot_mutate(ops in proptest::collection::vec(op_strategy(), 1..30)) {
        let mut world = notes_world();
        for (i, op) in ops.iter().enumerate() {
            let context = context_of(op, 10_000 + i as i64);
            let outsider = op.actor >= 4 || op.actor_tenant != op.tenant;
            match world.step(&intent_of(op), &context) {
                Ok(_) if outsider => prop_assert!(false, "an outsider's intent was accepted: {:?}", op),
                Ok((next, _)) => world = next,
                Err(error) if outsider => prop_assert!(
                    matches!(error.kind, ErrorKind::Unauthorized | ErrorKind::UnknownRecord | ErrorKind::UnknownObject),
                    "outsider got {:?}", error.kind
                ),
                Err(_) => {}
            }
        }
    }

    /// K4, reads. Every returned row is one the principal may see; the public never sees a
    /// draft; nobody sees another tenant's notes.
    #[test]
    fn reads_are_filtered_by_policy(ops in proptest::collection::vec(op_strategy(), 1..30), reader in 0usize..6, reader_tenant in 0usize..2) {
        let mut world = notes_world();
        for (i, op) in ops.iter().enumerate() {
            if let Ok((next, _)) = world.step(&intent_of(op), &context_of(op, 10_000 + i as i64)) {
                world = next;
            }
        }
        let request = ReadRequest::Named { query: QueryId::new("notes_by_rank").unwrap(), params: BTreeMap::new() };
        for tenant in TENANTS {
            let principal = principals(TENANTS[reader_tenant])[reader].clone();
            let context = ctx(tenant, principal.clone(), 99_000);
            let rows = world.read(&request, &context).unwrap().rows;
            let state = &world.tenants[&tenant_id(tenant)];
            // Roles may have been revoked along the way, so membership is read from the state.
            let holds_a_role = matches!(&principal, Principal::User(id) if state.members.contains_key(id));
            for row in &rows {
                prop_assert!(visible(state, &principal, row, context.now));
                prop_assert_eq!(state.record(&row.object, row.id), Some(row), "row belongs to this tenant");
                if !holds_a_role {
                    prop_assert_eq!(row.get(&field("status")), &text("published"));
                }
            }
        }
    }

    /// K7. On the partial snapshot planned by `needs`, a transition either agrees exactly with
    /// the complete snapshot or asks for more state. Loading what it asks for converges on the
    /// same answer.
    #[test]
    fn partial_snapshots_never_change_a_decision(ops in proptest::collection::vec(op_strategy(), 1..25)) {
        let mut world = notes_world();
        for (i, op) in ops.iter().enumerate() {
            let (intent, context) = (intent_of(op), context_of(op, 10_000 + i as i64));
            let tenant = world.tenants[&context.tenant].clone();
            let full = transition(&world.state(&context.tenant), &intent, &context);
            let mut wanted: BTreeSet<Need> = needs(Some(&tenant.schema), &intent).into_iter().collect();
            let mut rounds = 0;
            let thin = loop {
                match transition(&partial(&tenant, &wanted), &intent, &context) {
                    Err(DomainError { kind: ErrorKind::InsufficientState, missing: Some(need), .. }) => {
                        prop_assert!(wanted.insert(need), "asked twice for the same need");
                        rounds += 1;
                        prop_assert!(rounds < 20);
                    }
                    other => break other,
                }
            };
            match (&full, &thin) {
                (Ok(a), Ok(b)) => prop_assert_eq!(a, b),
                (Err(a), Err(b)) => prop_assert_eq!(a.kind, b.kind),
                _ => prop_assert!(false, "complete and partial snapshots disagree: {:?} vs {:?}", full.is_ok(), thin.is_ok()),
            }
            if let Ok((next, _)) = world.step(&intent, &context) {
                world = next;
            }
        }
    }

    /// K8. Revisions rise by one per accepted step, event sequence numbers are gapless and
    /// record ids are never reused.
    #[test]
    fn counters_are_monotonic(ops in proptest::collection::vec(op_strategy(), 1..30)) {
        let mut world = notes_world();
        let mut seen_ids: BTreeMap<(TenantId, ObjectTypeId), BTreeSet<RecordId>> = BTreeMap::new();
        for (i, op) in ops.iter().enumerate() {
            let context = context_of(op, 10_000 + i as i64);
            let before = world.tenants[&context.tenant].clone();
            if let Ok((next, outcome)) = world.step(&intent_of(op), &context) {
                let after = &next.tenants[&context.tenant];
                prop_assert_eq!(after.revision.0, before.revision.0 + 1);
                for (n, event) in outcome.events.iter().enumerate() {
                    prop_assert_eq!(event.seq, before.event_seq + 1 + n as u64);
                }
                prop_assert_eq!(after.event_seq, before.event_seq + outcome.events.len() as u64);
                if let Output::Record { object, id, version: 1 } = &outcome.output {
                    prop_assert!(seen_ids.entry((context.tenant.clone(), object.clone())).or_default().insert(*id));
                }
                world = next;
            }
        }
    }
}
