//! Property tests over random operation sequences. Every sequence starts from one of several
//! worlds (see `starting_worlds`: no tenant, one tenant, two tenants, seeded records, a
//! re-staffed and tightened tenant) and draws from all seven intents: create a tenant, change
//! the schema, assign a role, revoke a role, create, update and delete a record. Actors are
//! members, strangers, the public and the system, acting on their own tenant and on others.

use proptest::prelude::*;
use proptest::strategy::ValueTree;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use strata_kernel::testkit::*;
use strata_kernel::*;

/// Number of starting worlds `starting_worlds` returns.
const WORLDS: usize = 5;

/// One randomly chosen operation. Small ranges on purpose, so that collisions, conflicts and
/// references to missing records are common.
#[derive(Clone, Debug)]
struct Op {
    /// Index into `WORLD_TENANTS` of the tenant the operation addresses.
    tenant: usize,
    /// Index into `WORLD_TENANTS` of the tenant the actor belongs to.
    actor_tenant: usize,
    /// 0..=3 the members of the tenant, 4 a stranger, 5 an anonymous visitor, 6 the system.
    actor: usize,
    kind: OpKind,
}

#[derive(Clone, Debug)]
enum OpKind {
    CreateTenant {
        name: String,
        owner: usize,
    },
    Create {
        title: String,
        slug: u8,
        rank: Option<i64>,
        author: Option<u64>,
    },
    Person {
        name: String,
    },
    Update {
        id: u64,
        title: Option<String>,
        status: Option<&'static str>,
        rank: Option<i64>,
    },
    Delete {
        object: &'static str,
        id: u64,
    },
    ChangeSchema {
        change: SchemaOp,
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

#[derive(Clone, Copy, Debug)]
enum SchemaOp {
    RequireRank,
    TightenTitle(i64),
    /// A change that names an object that does not exist.
    Nonsense,
}

const MEMBERS: [&str; 5] = ["owner", "editor", "writer1", "writer2", "stranger"];
const ROLES: [&str; 5] = ["owner", "editor", "writer", "ghost", "public"];

fn op_strategy() -> impl Strategy<Value = Op> {
    let status = prop_oneof![Just("draft"), Just("published"), Just("archived")];
    let schema_op = prop_oneof![
        3 => Just(SchemaOp::RequireRank),
        3 => (3i64..30).prop_map(SchemaOp::TightenTitle),
        1 => Just(SchemaOp::Nonsense),
    ];
    let kind = prop_oneof![
        1 => ("[a-z]{0,4}", 0usize..2).prop_map(|(name, owner)| OpKind::CreateTenant { name, owner }),
        4 => ("[a-zA-Z ]{0,30}", 0u8..6, proptest::option::of(0i64..5), proptest::option::of(1u64..6))
            .prop_map(|(title, slug, rank, author)| OpKind::Create { title, slug, rank, author }),
        1 => "[a-z]{1,6}".prop_map(|name| OpKind::Person { name }),
        4 => (1u64..8, proptest::option::of("[a-z]{0,30}"), proptest::option::of(status), proptest::option::of(0i64..5))
            .prop_map(|(id, title, status, rank)| OpKind::Update { id, title, status, rank }),
        2 => (prop_oneof![Just("note"), Just("person")], 1u64..8).prop_map(|(object, id)| OpKind::Delete { object, id }),
        2 => (schema_op, any::<bool>()).prop_map(|(change, grandfather)| OpKind::ChangeSchema { change, grandfather }),
        2 => (0usize..5, 0usize..5).prop_map(|(who, role)| OpKind::Assign { who, role }),
        2 => (0usize..5, 0usize..5).prop_map(|(who, role)| OpKind::Revoke { who, role }),
    ];
    // Most actors belong to the tenant they act on; some come from another tenant, some are
    // strangers, the public or the system.
    (0usize..3, 0usize..3, 0usize..7, kind, 0u8..10).prop_map(
        |(tenant, other, actor, kind, cross)| Op {
            tenant,
            actor_tenant: if cross == 0 { other } else { tenant },
            actor,
            kind,
        },
    )
}

fn intent_of(op: &Op) -> Intent {
    match &op.kind {
        OpKind::CreateTenant { name, owner } => Intent::CreateTenant {
            name: name.clone(),
            owner: principal_id(&format!("{}:{}", WORLD_TENANTS[op.tenant], MEMBERS[*owner])),
        },
        OpKind::Create {
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
        OpKind::Person { name } => Intent::CreateRecord {
            object: object("person"),
            fields: BTreeMap::from([(field("name"), text(name))]),
        },
        OpKind::Update {
            id,
            title,
            status,
            rank,
        } => {
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
            Intent::UpdateRecord {
                object: object("note"),
                id: RecordId(*id),
                fields,
                expected_version: None,
            }
        }
        OpKind::Delete { object: name, id } => Intent::DeleteRecord {
            object: object(name),
            id: RecordId(*id),
            expected_version: None,
        },
        OpKind::ChangeSchema {
            change,
            grandfather,
        } => {
            let change = match change {
                SchemaOp::RequireRank => SchemaChange::DefineField {
                    object: object("note"),
                    field: field_def("rank", FieldType::Int, true),
                },
                SchemaOp::TightenTitle(limit) => SchemaChange::DefineConstraint {
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
                },
                SchemaOp::Nonsense => SchemaChange::RemoveField {
                    object: object("nothing"),
                    field: field("nothing"),
                },
            };
            Intent::ChangeSchema {
                changes: vec![change],
                expected_version: None,
                existing: if *grandfather {
                    ExistingData::Grandfather
                } else {
                    ExistingData::Reject
                },
            }
        }
        OpKind::Assign { who, role: r } => Intent::AssignRole {
            principal: principal_id(&format!("{}:{}", WORLD_TENANTS[op.tenant], MEMBERS[*who])),
            role: role(ROLES[*r]),
        },
        OpKind::Revoke { who, role: r } => Intent::RevokeRole {
            principal: principal_id(&format!("{}:{}", WORLD_TENANTS[op.tenant], MEMBERS[*who])),
            role: role(ROLES[*r]),
        },
    }
}

fn context_of(op: &Op, now: i64) -> Context {
    let principal = if op.actor == 6 {
        Principal::System
    } else {
        principals(WORLD_TENANTS[op.actor_tenant])[op.actor].clone()
    };
    ctx(WORLD_TENANTS[op.tenant], principal, now)
}

fn start(index: usize) -> World {
    starting_worlds().swap_remove(index).1
}

/// The partial snapshot a store would build from a list of needs.
fn partial(tenant: &Arc<TenantState>, wanted: &BTreeSet<Need>) -> State {
    let mut thin = TenantState::clone(tenant);
    thin.records.clear();
    for need in wanted {
        let matching: Vec<Arc<Record>> = match need {
            Need::Record { object, id } => {
                tenant.record(object, *id).into_iter().cloned().collect()
            }
            Need::AllOf(object) => tenant.records_of(object).cloned().collect(),
            Need::PageOf {
                object,
                after,
                limit,
            } => tenant
                .records_of(object)
                .filter(|r| after.is_none_or(|a| r.id > a))
                .take(*limit as usize)
                .cloned()
                .collect(),
            Need::ByField {
                object,
                field,
                value,
            } => tenant
                .records_of(object)
                .filter(|r| match r.get(field) {
                    Value::List(items) => items.contains(value),
                    other => other == value,
                })
                .cloned()
                .collect(),
            Need::ByFields { object, fields } => tenant
                .records_of(object)
                .filter(|r| fields.iter().all(|(field, value)| r.get(field) == value))
                .cloned()
                .collect(),
        };
        for record in matching {
            thin.records
                .entry(record.object.clone())
                .or_default()
                .insert(record.id, record);
        }
    }
    State {
        tenant_id: tenant.id.clone(),
        tenant: Some(Arc::new(thin)),
        coverage: Coverage::Partial(wanted.clone()),
    }
}

/// The roles a principal holds in a tenant, worked out here and not by the kernel.
fn roles_held(state: &TenantState, principal: &Principal) -> BTreeSet<RoleId> {
    match principal {
        Principal::System => BTreeSet::new(),
        Principal::Anonymous => BTreeSet::from([role("public")]),
        Principal::User(id) => {
            let mut roles = state.members.get(id).cloned().unwrap_or_default();
            roles.insert(role("public"));
            roles
        }
    }
}

/// Whether any policy of the tenant could authorise the intent for the principal, leaving the
/// conditions of the policies aside. Written from the definition of default deny: the owner
/// may do anything, everyone else needs a policy that names a role they hold, the action and
/// the resource.
fn some_policy_could_authorise(
    state: &TenantState,
    principal: &Principal,
    intent: &Intent,
) -> bool {
    let (action, resource) = match intent {
        Intent::CreateTenant { .. } => return *principal == Principal::System,
        Intent::CreateRecord { object, .. } => (Action::Create, Resource::Object(object.clone())),
        Intent::UpdateRecord { object, .. } => (Action::Update, Resource::Object(object.clone())),
        Intent::DeleteRecord { object, .. } => (Action::Delete, Resource::Object(object.clone())),
        Intent::ChangeSchema { .. } => (Action::Manage, Resource::Schema),
        Intent::AssignRole { .. }
        | Intent::RevokeRole { .. }
        | Intent::DefineTeam { .. }
        | Intent::AssignTeamRole { .. }
        | Intent::RevokeTeamRole { .. }
        | Intent::AddTeamMember { .. }
        | Intent::RemoveTeamMember { .. }
        | Intent::CreateInvitation { .. }
        | Intent::RevokeInvitation { .. }
        | Intent::EraseSubject { .. } => (Action::Manage, Resource::Members),
        // Redeeming needs no policy: any signed-in person may try.
        Intent::RedeemInvitation { .. } => return matches!(principal, Principal::User(_)),
    };
    let roles = roles_held(state, principal);
    roles.contains(&role("owner"))
        || state.schema.policies.values().any(|policy| {
            roles.contains(&policy.role)
                && policy.actions.contains(&action)
                && match (&policy.resource, &resource) {
                    (Resource::AnyObject, Resource::Object(_)) => true,
                    (a, b) => a == b,
                }
        })
}

/// Whether the notes policies let a principal with these roles read a record, written out from
/// the policies of `notes_schema`: owners and editors read everything, writers read notes,
/// everyone but the system reads people and published notes.
fn may_read(roles: &BTreeSet<RoleId>, record: &Record) -> bool {
    let has = |name: &str| roles.contains(&role(name));
    let is_note = record.object == object("note");
    has("owner")
        || has("editor")
        || (has("writer") && is_note)
        || (has("public")
            && (record.object == object("person")
                || (is_note && record.get(&field("status")) == &text("published"))))
}

proptest! {
    // A fixed, recorded seed: a run is the same run every time, and a failure reproduces.
    // Change `rng_seed` on purpose to explore other inputs.
    #![proptest_config(ProptestConfig {
        cases: 160,
        rng_seed: proptest::test_runner::RngSeed::Fixed(20260101),
        ..ProptestConfig::default()
    })]

    /// The same state, intent and context always give the same result, whether the state is
    /// the world's, a copy of it rebuilt from its canonical form, or a partial snapshot, and
    /// equal worlds have equal hashes.
    /// Verifies: KRN-001
    #[test]
    fn transitions_are_deterministic(
        from in 0..WORLDS,
        ops in proptest::collection::vec(op_strategy(), 1..30),
    ) {
        let mut world = start(from);
        for (i, op) in ops.iter().enumerate() {
            let (intent, context) = (intent_of(op), context_of(op, 10_000 + i as i64));
            let snapshot = world.state(&context.tenant);
            let first = transition(&snapshot, &intent, &context);
            prop_assert_eq!(&first, &transition(&snapshot, &intent, &context));
            // A snapshot rebuilt from its canonical form is an equal snapshot and decides alike.
            let rebuilt_snapshot: State =
                serde_json::from_str(&canon::canonical_json(&snapshot)).unwrap();
            prop_assert_eq!(canon::hash_of(&rebuilt_snapshot), canon::hash_of(&snapshot));
            prop_assert_eq!(&transition(&rebuilt_snapshot, &intent, &context), &first);
            // So does a partial snapshot, each time it is asked.
            if let Some(tenant) = &snapshot.tenant {
                let wanted: BTreeSet<Need> =
                    needs(Some(&tenant.schema), &intent).into_iter().collect();
                let thin = partial(tenant, &wanted);
                prop_assert_eq!(
                    &transition(&thin, &intent, &context),
                    &transition(&thin, &intent, &context)
                );
            }
            let (stepped, second) = (world.step(&intent, &context), world.step(&intent, &context));
            prop_assert_eq!(&stepped, &second);
            if let Ok((next, outcome)) = stepped {
                prop_assert_eq!(&Ok::<_, DomainError>(outcome.clone()), &first);
                prop_assert_eq!(canon::hash_of(&outcome), canon::hash_of(&second.unwrap().1));
                world = next;
            }
        }
        // A world rebuilt from its canonical form is the same world.
        let rebuilt: World = serde_json::from_str(&canon::canonical_json(&world)).unwrap();
        prop_assert_eq!(rebuilt.hash(), world.hash());
    }

    /// A step in one tenant leaves every other tenant exactly as it was, a rejected step leaves
    /// everything as it was, an accepted step's delta and events name only the tenant of its
    /// context, and a snapshot of another tenant is never accepted for it.
    /// Verifies: KRN-002
    #[test]
    fn a_step_never_touches_another_tenant(
        from in 0..WORLDS,
        ops in proptest::collection::vec(op_strategy(), 1..30),
    ) {
        let mut world = start(from);
        for (i, op) in ops.iter().enumerate() {
            let own = tenant_id(WORLD_TENANTS[op.tenant]);
            let others: Vec<TenantId> = WORLD_TENANTS
                .iter()
                .map(|name| tenant_id(name))
                .filter(|id| *id != own)
                .collect();
            let before: Vec<String> = others.iter().map(|id| world.tenant_hash(id)).collect();
            let (intent, context) = (intent_of(op), context_of(op, 10_000 + i as i64));
            for other in &others {
                let refused = transition(&world.state(other), &intent, &context);
                prop_assert!(
                    matches!(&refused, Err(e) if e.kind == ErrorKind::TenantMismatch),
                    "another tenant's snapshot was used: {:?}", refused
                );
            }
            if let Ok((next, outcome)) = world.step(&intent, &context) {
                prop_assert_eq!(&outcome.delta.tenant, &own);
                prop_assert!(outcome.events.iter().all(|e| e.tenant == own));
                world = next;
            }
            let after: Vec<String> = others.iter().map(|id| world.tenant_hash(id)).collect();
            prop_assert_eq!(after, before);
        }
    }

    /// An intent is accepted only if some policy of the tenant could authorise its principal,
    /// and one that no policy could authorise is rejected, naming the policy check that failed.
    /// The authorisation is worked out in the test from the tenant's policies and the roles in
    /// its member table at the time of the step, so a role revoked or assigned along the way
    /// counts.
    /// Verifies: KRN-003
    #[test]
    fn what_no_policy_authorises_is_rejected(
        from in 0..WORLDS,
        ops in proptest::collection::vec(op_strategy(), 1..30),
    ) {
        let mut world = start(from);
        for (i, op) in ops.iter().enumerate() {
            let (intent, context) = (intent_of(op), context_of(op, 10_000 + i as i64));
            let state = world.tenants.get(&context.tenant).cloned();
            let creates = matches!(intent, Intent::CreateTenant { .. });
            let authorisable = match &state {
                Some(state) => some_policy_could_authorise(state, &context.principal, &intent),
                None => creates && context.principal == Principal::System,
            };
            match world.step(&intent, &context) {
                Ok((next, _)) => {
                    prop_assert!(authorisable, "accepted although no policy authorises: {:?}", op);
                    world = next;
                }
                Err(error) if !authorisable => {
                    prop_assert!(
                        matches!(
                            error.kind,
                            ErrorKind::Unauthorized
                                | ErrorKind::UnknownRecord
                                | ErrorKind::UnknownObject
                                | ErrorKind::TenantExists
                                | ErrorKind::TenantNotFound
                        ),
                        "an unauthorised {:?} was refused as {:?}", op, error.kind
                    );
                    if error.kind == ErrorKind::Unauthorized {
                        prop_assert!(
                            error.evidence.iter().any(|c| !c.passed && c.rule.starts_with("policy:")),
                            "no failed policy check: {:?}", error.evidence
                        );
                    }
                    let manages = matches!(
                        intent,
                        Intent::ChangeSchema { .. } | Intent::AssignRole { .. } | Intent::RevokeRole { .. }
                    );
                    if state.is_some() && manages {
                        prop_assert_eq!(error.kind, ErrorKind::Unauthorized);
                    }
                }
                Err(error) => {
                    // Authorisable, yet refused: a policy condition or a validation can still say
                    // no, but never the owner, and never as `Unauthorized` without a failed
                    // policy check to show for it.
                    if error.kind == ErrorKind::Unauthorized {
                        if let Some(state) = &state {
                            prop_assert!(
                                !roles_held(state, &context.principal).contains(&role("owner")),
                                "the owner was refused as unauthorised: {:?}", op
                            );
                        }
                        prop_assert!(
                            error.evidence.iter().any(|c| !c.passed && c.rule.starts_with("policy:")),
                            "refused as unauthorised with no failed policy check: {:?}", error.evidence
                        );
                    }
                }
            }
        }
    }

    /// A read returns exactly the records the policies let the principal read: every row is
    /// one a role of the principal is granted, every such record of the tenant is there, and
    /// nothing comes from another tenant. The grant is worked out in the test from the roles
    /// in the member table.
    /// Verifies: KRN-004
    #[test]
    fn reads_are_filtered_by_policy(
        from in 0..WORLDS,
        ops in proptest::collection::vec(op_strategy(), 1..30),
        reader in 0usize..7,
        reader_tenant in 0usize..3,
    ) {
        let mut world = start(from);
        for (i, op) in ops.iter().enumerate() {
            if let Ok((next, _)) = world.step(&intent_of(op), &context_of(op, 10_000 + i as i64)) {
                world = next;
            }
        }
        let principal = if reader == 6 {
            Principal::System
        } else {
            principals(WORLD_TENANTS[reader_tenant])[reader].clone()
        };
        let by_rank = ReadRequest::Named { query: QueryId::new("notes_by_rank").unwrap(), params: BTreeMap::new(), page_size: None, cursor: None };
        let ids: Vec<RecordId> = (1..9).map(RecordId).collect();
        for tenant in WORLD_TENANTS {
            let context = ctx(tenant, principal.clone(), 99_000);
            let Some(state) = world.tenants.get(&tenant_id(tenant)) else {
                let error = world.read(&by_rank, &context).unwrap_err();
                prop_assert_eq!(error.kind, ErrorKind::TenantNotFound);
                continue;
            };
            let roles = roles_held(state, &principal);
            // A snapshot of this tenant answers no read that names another.
            let elsewhere = ctx(WORLD_TENANTS[(reader_tenant + 1) % 3], principal.clone(), 99_000);
            if elsewhere.tenant != context.tenant {
                let error = read(&world.state(&context.tenant), &by_rank, &elsewhere).unwrap_err();
                prop_assert_eq!(error.kind, ErrorKind::TenantMismatch);
            }

            let asked: Vec<(ObjectTypeId, ReadRequest)> = vec![
                (object("note"), by_rank.clone()),
                (object("note"), ReadRequest::Records { object: object("note"), ids: ids.clone() }),
                (object("person"), ReadRequest::Records { object: object("person"), ids: ids.clone() }),
            ];
            for (object_type, request) in asked {
                if !state.schema.objects.contains_key(&object_type) {
                    // A tenant created along the way has no notes model to read.
                    let error = world.read(&request, &context).unwrap_err();
                    prop_assert!(matches!(error.kind, ErrorKind::UnknownObject | ErrorKind::UnknownQuery));
                    continue;
                }
                let rows = world.read(&request, &context).unwrap().rows;
                let named = matches!(&request, ReadRequest::Named { .. });
                let expected: BTreeSet<RecordId> = state
                    .records_of(&object_type)
                    .filter(|r| may_read(&roles, r))
                    .filter(|r| named || ids.contains(&r.id))
                    .map(|r| r.id)
                    .collect();
                let got: BTreeSet<RecordId> = rows.iter().map(|r| r.id).collect();
                prop_assert_eq!(rows.len(), got.len(), "a row came twice");
                prop_assert_eq!(&got, &expected, "{:?} reading {:?} as {:?}", tenant, request, principal);
                for row in &rows {
                    prop_assert_eq!(state.record(&row.object, row.id), Some(row), "row belongs to this tenant");
                    prop_assert!(may_read(&roles, row));
                }
            }
        }
    }

    /// On a partial snapshot a transition returns what the complete snapshot returns, or asks
    /// for state the snapshot did not cover and names it. Partial snapshots here hold nothing,
    /// what `needs` plans, or that plus a random selection of records and whole object types.
    /// Loading what each refusal names converges on the answer of the complete snapshot.
    /// Verifies: KRN-005
    #[test]
    fn partial_snapshots_never_change_a_decision(
        from in 0..WORLDS,
        ops in proptest::collection::vec((op_strategy(), any::<u64>()), 1..25),
    ) {
        let mut world = start(from);
        for (i, (op, mask)) in ops.iter().enumerate() {
            let (intent, context) = (intent_of(op), context_of(op, 10_000 + i as i64));
            if let Some(tenant) = world.tenants.get(&context.tenant).cloned() {
                let full = transition(&world.state(&context.tenant), &intent, &context);
                let planned: BTreeSet<Need> = needs(Some(&tenant.schema), &intent).into_iter().collect();
                let mut selected = planned.clone();
                for (n, (object, id)) in tenant
                    .records
                    .iter()
                    .flat_map(|(object, records)| records.keys().map(move |id| (object.clone(), *id)))
                    .enumerate()
                {
                    if (mask >> (n % 60)) & 1 == 1 {
                        selected.insert(Need::Record { object, id });
                    }
                }
                if (mask >> 62) & 1 == 1 {
                    selected.insert(Need::AllOf(object("note")));
                }
                if (mask >> 63) & 1 == 1 {
                    selected.insert(Need::AllOf(object("person")));
                }
                for mut wanted in [BTreeSet::new(), planned, selected] {
                    let mut rounds = 0;
                    loop {
                        let answer = transition(&partial(&tenant, &wanted), &intent, &context);
                        match answer {
                            Err(DomainError { kind: ErrorKind::InsufficientState, missing: Some(need), .. }) => {
                                prop_assert!(
                                    !Coverage::Partial(wanted.clone()).covers(&need),
                                    "asked for {:?}, which the snapshot already covered", need
                                );
                                prop_assert!(wanted.insert(need));
                                rounds += 1;
                                prop_assert!(rounds < 64);
                            }
                            other => {
                                prop_assert_eq!(&other, &full);
                                break;
                            }
                        }
                    }
                }
            }
            if let Ok((next, _)) = world.step(&intent, &context) {
                world = next;
            }
        }
    }

    /// Revisions rise by exactly one per accepted step, from nothing to one for a new tenant;
    /// the delta says so and cannot be applied twice; event sequence numbers are gapless and
    /// record ids come from a counter that only rises, so none is reused.
    /// Verifies: KRN-006
    #[test]
    fn counters_are_monotonic(
        from in 0..WORLDS,
        ops in proptest::collection::vec(op_strategy(), 1..30),
    ) {
        let mut world = start(from);
        let mut seen_ids: BTreeMap<TenantId, BTreeSet<RecordId>> = BTreeMap::new();
        for (id, tenant) in &world.tenants {
            for records in tenant.records.values() {
                seen_ids.entry(id.clone()).or_default().extend(records.keys().copied());
            }
        }
        for (i, op) in ops.iter().enumerate() {
            let context = context_of(op, 10_000 + i as i64);
            let before = world.tenants.get(&context.tenant).cloned();
            let (revision, event_seq, next_record) = match &before {
                Some(tenant) => (tenant.revision.0, tenant.event_seq, tenant.next_record),
                None => (0, 0, 1),
            };
            if let Ok((next, outcome)) = world.step(&intent_of(op), &context) {
                let after = &next.tenants[&context.tenant];
                prop_assert_eq!(after.revision.0, revision + 1);
                prop_assert_eq!(outcome.delta.new_revision, after.revision);
                prop_assert_eq!(outcome.delta.base_revision, before.as_ref().map(|t| t.revision));
                prop_assert!(
                    apply_delta(Some(&**after), &outcome.delta).is_err(),
                    "the same delta applied twice"
                );
                for (n, event) in outcome.events.iter().enumerate() {
                    prop_assert_eq!(event.seq, event_seq + 1 + n as u64);
                    prop_assert_eq!(event.revision, after.revision);
                }
                prop_assert_eq!(after.event_seq, event_seq + outcome.events.len() as u64);
                if let Output::Record { id, version: 1, .. } = &outcome.output {
                    prop_assert_eq!(id.0, next_record);
                    prop_assert!(seen_ids.entry(context.tenant.clone()).or_default().insert(*id));
                    prop_assert_eq!(after.next_record, next_record + 1);
                } else {
                    prop_assert_eq!(after.next_record, next_record);
                }
                world = next;
            }
        }
    }

    /// A workflow field changes only along a declared transition, and only for someone the
    /// transition's guard lets through; a record starts in an initial state. (Over random
    /// input, beside the table of `behaviour.rs`: KRN-008.)
    #[test]
    fn a_workflow_field_changes_only_by_a_declared_transition(
        from in 0..WORLDS,
        ops in proptest::collection::vec(op_strategy(), 1..30),
    ) {
        let status = field("status");
        let mut world = start(from);
        for (i, op) in ops.iter().enumerate() {
            let context = context_of(op, 10_000 + i as i64);
            let roles = world
                .tenants
                .get(&context.tenant)
                .map(|tenant| roles_held(tenant, &context.principal))
                .unwrap_or_default();
            if let Ok((next, outcome)) = world.step(&intent_of(op), &context) {
                for event in &outcome.events {
                    match &event.body {
                        EventBody::RecordCreated { record } if record.object == object("note") => {
                            prop_assert_eq!(record.get(&status), &text("draft"));
                        }
                        EventBody::RecordUpdated { before, after } if before.object == object("note") => {
                            let (a, b) = (before.get(&status).as_str(), after.get(&status).as_str());
                            if a != b {
                                prop_assert!(
                                    matches!((a, b), (Some("draft"), Some("published")) | (Some("published"), Some("draft"))),
                                    "{:?} -> {:?} is not a declared transition", a, b
                                );
                                if b == Some("published") {
                                    prop_assert!(roles.contains(&role("editor")), "published without the guard's role");
                                }
                            }
                        }
                        _ => {}
                    }
                }
                world = next;
            }
        }
    }

    /// Every rejection, of whichever intent and from whichever world, lists at least one check
    /// that failed. (The same requirement as the example table in `behaviour.rs`, KRN-007, over
    /// random input.)
    #[test]
    fn every_rejection_lists_a_failed_check(
        from in 0..WORLDS,
        ops in proptest::collection::vec(op_strategy(), 1..30),
    ) {
        let mut world = start(from);
        for (i, op) in ops.iter().enumerate() {
            match world.step(&intent_of(op), &context_of(op, 10_000 + i as i64)) {
                Ok((next, _)) => world = next,
                Err(error) => prop_assert!(
                    error.evidence.iter().any(|c| !c.passed),
                    "{:?} was refused as {:?} with no failed check: {:?}", op, error.kind, error.evidence
                ),
            }
        }
    }
}

/// The property tests are only as good as the worlds and intents they draw from: the worlds
/// really differ, and between them the generated operations reach all seven intents and every
/// kind of actor.
#[test]
fn the_starting_worlds_differ_and_every_intent_is_generated() {
    let worlds = starting_worlds();
    assert_eq!(worlds.len(), WORLDS);
    let hashes: BTreeSet<String> = worlds.iter().map(|(_, w)| w.hash()).collect();
    assert_eq!(hashes.len(), WORLDS, "two starting worlds are the same");

    let mut runner = proptest::test_runner::TestRunner::deterministic();
    let mut names = BTreeSet::new();
    let mut actors = BTreeSet::new();
    for _ in 0..400 {
        let op = op_strategy().new_tree(&mut runner).unwrap().current();
        names.insert(intent_of(&op).name());
        actors.insert(op.actor);
    }
    assert_eq!(
        names,
        BTreeSet::from([
            "create_tenant",
            "change_schema",
            "assign_role",
            "revoke_role",
            "create_record",
            "update_record",
            "delete_record"
        ])
    );
    assert_eq!(actors.len(), 7);
}
