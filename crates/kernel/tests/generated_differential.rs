//! The hand-written kernel (`strata_kernel::transition`, `read`, `visible`, `needs`,
//! `read_needs`) against the functions specc generates from the specification
//! (`strata_model_core::functions::kernel_transition`, `kernel_read`, `visible`, `needs`,
//! `read_needs`). The backend runs the hand-written kernel; the generated functions are what
//! contracts CT-001 to CT-005 are derived from. Nothing else compares the two, so this test
//! runs both on the same inputs and fails on any difference in outcome.
//!
//! Inputs: the five starting worlds of `strata_kernel::testkit` (the corpus `properties.rs` and
//! the Lean fixtures start from), driven by random operation sequences drawn from the same
//! operation generator `properties.rs` uses (copied here, because a test binary cannot import
//! another), with a fixed seed (`TestRunner::deterministic`). After the operations, reads and
//! visibility checks run over every tenant, principal and record.
//!
//! The kernel crate is the one place that already depends on both sides (the hand-written code
//! and `strata-model-core`), so the test lives here and adds no dependency.
//!
//! The two sides are given the same types: the kernel re-exports the generated types. The
//! adapters are small:
//!   - the generated functions take `texts` (`IntentTexts`), the printed forms of expressions the
//!     function language cannot print (WG-4957); `texts_for` derives them from the snapshot's
//!     schema and the intent, with `Expr`'s `Display`, which is what the hand-written kernel
//!     prints;
//!   - the generated functions fail with `Failure<Rejection>`; a `Rejection::Rejected` payload is
//!     the hand-written kernel's `DomainError`, and a `Failure::Fault` (integer overflow,
//!     division by zero) fails the test, because the hand-written kernel has no such outcome;
//!   - the generated `needs` takes `&Option<TenantSchema>`, the hand-written one
//!     `Option<&TenantSchema>`.
//!
//! Differences known by design, each listed with its reason. A difference that matches none of
//! them fails the test.
//!   1. `EVALUATION_FAILED_DETAIL`: for a policy condition that fails to evaluate, the
//!      hand-written kernel writes the check detail `<condition>: <evaluation error>`; the
//!      generated `policy-check-text` writes `<condition>: evaluation failed`, because
//!      `policy-check` does not carry the error (a gap of WG-4938). Compared as: the generated
//!      detail minus its suffix is a prefix of the hand-written detail.
//!   2. Not exercised, so not compared: a policy condition or query filter that follows a
//!      reference (`through`). The generated `authorize` has no lookup and fails such a
//!      condition (WG-4938, WG-4957); the hand-written one has a lookup. The corpus has no
//!      such condition.
//!   3. Not exercised: a query order key over a list-valued attribute. The generated order
//!      does not order a list element by element, as `Value`'s derived order does.
//!      The corpus has no list-valued field.
//!   4. Not exercised: the intents of teams and invitations, and schema changes other than
//!      `DefineField` and `DefineConstraint`: the operation generator of `properties.rs` draws
//!      only seven intents. `texts_for` therefore reads defaults and constraints only from those
//!      two kinds of change. The functions of those intents are mirrored all the same (the
//!      conferral of a role, the owner that remains, the reserved `team:` prefix, the rewrite of
//!      records by a schema change), each with examples in the specification.
//!   5. Not exercised, and not mirrored: size limits (`limits.rs`: the bytes of a record, the
//!      length of a string or a list, the encoded size of a value of a unique field). The
//!      generated `validate-record` has no such check, because the function language cannot
//!      measure the encoding of a value. The corpus writes titles of at most thirty characters.
//!   6. Not exercised, and not mirrored: field-level read conditions (`FieldDef::read`). The
//!      hand-written `read` trims a record to the fields whose condition holds for the reader;
//!      the generated `kernel-read` returns whole records. No field of the corpus has one.
//!   7. Not exercised, and not mirrored: deleting a record that refers to the deleted one in two
//!      fields. The hand-written `delete_record` clears both in one rewrite; the generated
//!      `delete-record` clears one field per rewrite, so the second rewrite starts from the
//!      stored record and keeps the first field. No object of the corpus has two reference
//!      fields.
//!   8. Not exercised, and not mirrored: the problems of a schema the generated `schema-problems`
//!      does not state: the counts of objects, policies and queries, a query limit outside 1 to
//!      200, a field read condition that is not a truth value, the permissions of roles, teams
//!      and invitations. The corpus changes the schema only by a field, a constraint or an
//!      unknown object type.
//!   9. Not exercised, and not mirrored: conditions that follow a reference are
//!      `InsufficientState` in the hand-written kernel when the snapshot lacks the record
//!      (difference 2 states the rest).
//!
//! Differences 5 to 8 are guarded: `assert_no_input_of_a_known_difference` runs on every
//! snapshot the test decides or reads, and fails when a snapshot has a field read condition or
//! an object with two reference fields to one object type, so that a corpus that grows into
//! them is told to mirror the rule or to count the difference here. The others are guarded by
//! the operation generator drawing nothing that reaches them.
//!
//! Mirrored in this pass, so no longer differences: authorisation before the lookup of an object
//! type or a record (`object-def`, `stored`), the conferral of a role counting policies
//! (`may-confer`, `holds-policy-power`), a revocation asking for the authority to confer, the
//! owner that remains counting teams (`owner-lost`), the second authorisation of an update after
//! workflow assignments, the reserved `team:` prefix (`reserved-principal`), the records a
//! schema change rewrites (version one higher, an event each), the limit of 200 changes in one
//! schema change, the permissions of a role granting before any policy (`permission:role`), the
//! plan of a schema change (`ref-needs`) and a query filter that cannot be evaluated on a record
//! leaving the record out.

use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use std::collections::BTreeMap;
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_model_core::functions::{self as spec, Failure};
use strata_model_core::types::{IntentTexts, Rejection};

/// How many random operation sequences run from each starting world, and how long each is.
const SEQUENCES: usize = 24;
const OPS_PER_SEQUENCE: usize = 20;

/// The reason of difference 1 above.
const EVALUATION_FAILED_DETAIL: &str = ": evaluation failed";

// ---------------------------------------------------------------------------------------
// The operation generator (a copy of `properties.rs`).
// ---------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Op {
    tenant: usize,
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
        1 => ("[a-z ]{0,4}", 0usize..2).prop_map(|(name, owner)| OpKind::CreateTenant { name, owner }),
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

fn principal_of(tenant: usize, actor: usize) -> Principal {
    if actor == 6 {
        Principal::System
    } else {
        principals(WORLD_TENANTS[tenant])[actor].clone()
    }
}

fn context_of(op: &Op, now: i64) -> Context {
    ctx(
        WORLD_TENANTS[op.tenant],
        principal_of(op.actor_tenant, op.actor),
        now,
    )
}

// ---------------------------------------------------------------------------------------
// The adapters.
// ---------------------------------------------------------------------------------------

/// The `texts` the generated functions take, derived as the caller (the shell) would: the
/// printed defaults, constraints, policy conditions and query filters of the snapshot's schema,
/// those a `ChangeSchema` intent defines, and whether a `CreateTenant` name is blank.
fn texts_for(state: &State, intent: &Intent) -> IntentTexts {
    let mut texts = IntentTexts {
        defaults: BTreeMap::new(),
        constraints: BTreeMap::new(),
        conditions: BTreeMap::new(),
        blank_name: false,
    };
    if let Some(tenant) = &state.tenant {
        for (object_id, def) in &tenant.schema.objects {
            for (field_id, field) in &def.fields {
                if let Some(default) = &field.default {
                    texts
                        .defaults
                        .entry(object_id.clone())
                        .or_default()
                        .insert(field_id.clone(), default.to_string());
                }
            }
            for (constraint_id, constraint) in &def.constraints {
                texts
                    .constraints
                    .entry(object_id.clone())
                    .or_default()
                    .insert(constraint_id.clone(), constraint.expr.to_string());
            }
        }
        for (policy_id, policy) in &tenant.schema.policies {
            if let Some(condition) = &policy.condition {
                texts
                    .conditions
                    .insert(format!("policy:{}", policy_id.0), condition.to_string());
            }
        }
        for (query_id, query) in &tenant.schema.queries {
            if let Some(filter) = &query.filter {
                texts
                    .conditions
                    .insert(format!("query:{}", query_id.0), filter.to_string());
            }
        }
    }
    match intent {
        Intent::ChangeSchema { changes, .. } => {
            for change in changes {
                match change {
                    SchemaChange::DefineField { object, field } => {
                        if let Some(default) = &field.default {
                            texts
                                .defaults
                                .entry(object.clone())
                                .or_default()
                                .insert(field.id.clone(), default.to_string());
                        }
                    }
                    SchemaChange::DefineConstraint { object, constraint } => {
                        texts
                            .constraints
                            .entry(object.clone())
                            .or_default()
                            .insert(constraint.id.clone(), constraint.expr.to_string());
                    }
                    _ => {}
                }
            }
        }
        Intent::CreateTenant { name, .. } => texts.blank_name = name.trim().is_empty(),
        _ => {}
    }
    texts
}

/// A generated function's failure as the hand-written kernel's error type. A fault has no
/// counterpart there, so it is reported as a difference by the caller.
fn domain_error(failure: Failure<Rejection>) -> Result<DomainError, String> {
    match failure {
        Failure::Error(Rejection::Rejected(error)) => Ok(error),
        Failure::Fault(fault) => Err(format!("fault: {fault}")),
    }
}

/// Difference 1: make the generated detail of a failed policy condition comparable with the
/// hand-written one when, and only when, the only difference is the evaluation error text.
/// Returns how many details it matched.
fn reconcile_evaluation_failed(hand: &[Check], generated: &mut [Check]) -> usize {
    let mut matched = 0;
    for (h, g) in hand.iter().zip(generated.iter_mut()) {
        if h.rule == g.rule
            && h.rule.starts_with("policy:")
            && !h.passed
            && !g.passed
            && g.detail.ends_with(EVALUATION_FAILED_DETAIL)
        {
            let condition = &g.detail[..g.detail.len() - EVALUATION_FAILED_DETAIL.len()];
            if h.detail.starts_with(&format!("{condition}: ")) {
                g.detail = h.detail.clone();
                matched += 1;
            }
        }
    }
    matched
}

/// Differences 5 to 8 of the header: fail when a snapshot holds an input of a rule the generated
/// functions do not mirror, so the corpus cannot start to depend on one silently.
fn assert_no_input_of_a_known_difference(state: &State) {
    let Some(tenant) = &state.tenant else {
        return;
    };
    for (object_id, def) in &tenant.schema.objects {
        let mut targets = Vec::new();
        for (field_id, field) in &def.fields {
            assert!(
                field.read.is_none(),
                "{object_id}.{field_id} has a read condition; the generated kernel-read does not trim fields (difference 6)"
            );
            if let Some((target, _, _)) = field.ty.ref_target() {
                targets.push(target);
            }
        }
        let distinct: std::collections::BTreeSet<_> = targets.iter().collect();
        assert_eq!(
            targets.len(),
            distinct.len(),
            "{object_id} has two reference fields to one object type; the generated delete-record clears one per rewrite (difference 7)"
        );
    }
}

#[derive(Default)]
struct Tally {
    compared: usize,
    accepted: usize,
    rejected: usize,
    reads: usize,
    visibility: usize,
    needs: usize,
    evaluation_failed_details: usize,
}

fn compare_transition(state: &State, intent: &Intent, context: &Context, tally: &mut Tally) {
    let label = format!(
        "{} as {:?} in {} (snapshot of {})",
        intent.name(),
        context.principal,
        context.tenant,
        state.tenant_id
    );
    assert_no_input_of_a_known_difference(state);
    let hand = transition(state, intent, context);
    let texts = texts_for(state, intent);
    let mut generated = match spec::kernel_transition(state, intent, context, &texts) {
        Ok(result) => Ok(result),
        Err(failure) => Err(domain_error(failure).unwrap_or_else(|fault| {
            panic!(
                "{label}: the generated kernel failed with {fault}; the hand kernel gave {hand:?}"
            )
        })),
    };
    match (&hand, &mut generated) {
        (Ok(h), Ok(g)) => {
            tally.evaluation_failed_details +=
                reconcile_evaluation_failed(&h.evidence, &mut g.evidence);
            tally.accepted += 1;
        }
        (Err(h), Err(g)) => {
            tally.evaluation_failed_details +=
                reconcile_evaluation_failed(&h.evidence, &mut g.evidence);
            tally.rejected += 1;
        }
        _ => {}
    }
    assert_eq!(
        hand, generated,
        "kernel and generated kernel-transition differ on {label}"
    );
    tally.compared += 1;

    // The state each side plans to load before deciding.
    let schema = state.tenant.as_ref().map(|t| (*t.schema).clone());
    assert_eq!(
        needs(schema.as_ref(), intent),
        spec::needs(&schema, intent),
        "needs differ on {label}"
    );
    tally.needs += 1;
}

fn compare_read(state: &State, request: &ReadRequest, context: &Context, tally: &mut Tally) {
    let label = format!(
        "{request:?} as {:?} in {} (snapshot of {})",
        context.principal, context.tenant, state.tenant_id
    );
    assert_no_input_of_a_known_difference(state);
    let hand = read(state, request, context);
    // Only the query filters are printed by a read; the intent is none.
    let texts = texts_for(
        state,
        &Intent::AssignRole {
            principal: principal_id("x"),
            role: role("public"),
        },
    );
    let mut generated = match spec::kernel_read(state, request, context, &texts) {
        Ok(result) => Ok(result),
        Err(failure) => Err(domain_error(failure).unwrap_or_else(|fault| {
            panic!("{label}: the generated read failed with {fault}; the hand kernel gave {hand:?}")
        })),
    };
    if let (Err(h), Err(g)) = (&hand, &mut generated) {
        tally.evaluation_failed_details +=
            reconcile_evaluation_failed(&h.evidence, &mut g.evidence);
    }
    assert_eq!(
        hand, generated,
        "kernel and generated kernel-read differ on {label}"
    );
    tally.reads += 1;

    if let Some(tenant) = &state.tenant {
        assert_eq!(
            read_needs(&tenant.schema, request),
            spec::read_needs(&tenant.schema, request),
            "read-needs differ on {label}"
        );
        tally.needs += 1;
    }
}

/// Every read the corpus asks of a snapshot, as every kind of principal of every tenant.
fn compare_reads_of(world: &World, now: i64, tally: &mut Tally) {
    let ids: Vec<RecordId> = (1..9).map(RecordId).collect();
    let requests = [
        ReadRequest::Named {
            query: QueryId::new("notes_by_rank").unwrap(),
            params: BTreeMap::new(),
            page_size: None,
            cursor: None,
        },
        ReadRequest::Named {
            query: QueryId::new("no_such_query").unwrap(),
            params: BTreeMap::new(),
            page_size: None,
            cursor: None,
        },
        ReadRequest::Records {
            object: object("note"),
            ids: ids.clone(),
        },
        ReadRequest::Records {
            object: object("person"),
            ids,
        },
        ReadRequest::Records {
            object: object("nothing"),
            ids: vec![RecordId(1)],
        },
    ];
    for tenant in WORLD_TENANTS {
        for actor_tenant in 0..WORLD_TENANTS.len() {
            for actor in 0..7 {
                let context = ctx(tenant, principal_of(actor_tenant, actor), now);
                let state = world.state(&tenant_id(tenant));
                for request in &requests {
                    compare_read(&state, request, &context, tally);
                }
                // A snapshot of another tenant than the context names.
                let elsewhere = world.state(&tenant_id(WORLD_TENANTS[(actor_tenant + 1) % 3]));
                compare_read(&elsewhere, &requests[0], &context, tally);
            }
        }
    }
}

/// `visible` on every stored record, for every principal.
fn compare_visibility(world: &World, now: i64, tally: &mut Tally) {
    for tenant in world.tenants.values() {
        for actor_tenant in 0..WORLD_TENANTS.len() {
            for actor in 0..7 {
                let principal = principal_of(actor_tenant, actor);
                for record in tenant.records.values().flat_map(|by_id| by_id.values()) {
                    let hand = visible(tenant, &principal, record, Timestamp(now));
                    let generated = spec::visible(tenant, &principal, record, Timestamp(now))
                        .unwrap_or_else(|fault| {
                            panic!(
                                "visible of {} {} for {principal:?} faulted: {fault}",
                                record.object, record.id.0
                            )
                        });
                    assert_eq!(
                        hand, generated,
                        "visible differs for {principal:?} on {} {} of {}",
                        record.object, record.id.0, tenant.id
                    );
                    tally.visibility += 1;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// The tests.
// ---------------------------------------------------------------------------------------

/// The hand-written kernel and the generated one decide, read and plan alike over the corpus.
#[test]
fn the_hand_written_kernel_agrees_with_the_generated_functions() {
    let mut runner = TestRunner::deterministic();
    let mut tally = Tally::default();
    let strategy = op_strategy();
    for (index, (_, start)) in starting_worlds().into_iter().enumerate() {
        for sequence in 0..SEQUENCES {
            let mut world = start.clone();
            for step in 0..OPS_PER_SEQUENCE {
                let op = strategy.new_tree(&mut runner).unwrap().current();
                let (intent, context) = (
                    intent_of(&op),
                    context_of(&op, 10_000 + (sequence * OPS_PER_SEQUENCE + step) as i64),
                );
                let state = world.state(&context.tenant);
                compare_transition(&state, &intent, &context, &mut tally);
                // A snapshot of another tenant than the context names.
                let elsewhere = world.state(&tenant_id(WORLD_TENANTS[(op.tenant + 1) % 3]));
                compare_transition(&elsewhere, &intent, &context, &mut tally);
                if let Ok((next, _)) = world.step(&intent, &context) {
                    world = next;
                }
            }
            if sequence % 4 == 0 {
                let now = 99_000 + (index * SEQUENCES + sequence) as i64;
                compare_reads_of(&world, now, &mut tally);
                compare_visibility(&world, now, &mut tally);
            }
        }
    }
    // The corpus reaches both outcomes, so agreement is not agreement on one answer.
    assert!(
        tally.accepted > 200,
        "only {} accepted decisions",
        tally.accepted
    );
    assert!(
        tally.rejected > 200,
        "only {} rejected decisions",
        tally.rejected
    );
    assert!(tally.reads > 1000, "only {} reads", tally.reads);
    assert!(
        tally.visibility > 500,
        "only {} visibility checks",
        tally.visibility
    );
    eprintln!(
        "compared {} decisions ({} accepted, {} rejected), {} reads, {} visibility checks, {} plans; \
         {} policy-condition details matched under difference 1",
        tally.compared,
        tally.accepted,
        tally.rejected,
        tally.reads,
        tally.visibility,
        tally.needs,
        tally.evaluation_failed_details
    );
}

/// The comparison can fail: the same inputs with a hand-written answer that has been altered are
/// not equal to the generated one. Guards against `assert_eq!` on two values that are always
/// equal by construction.
#[test]
fn the_comparison_notices_a_difference() {
    let world = notes_world();
    let context = ctx("alpha", user("alpha:owner"), 10_000);
    let intent = new_note("A title", "a-slug");
    let state = world.state(&context.tenant);
    let hand = transition(&state, &intent, &context).expect("the owner may create a note");
    let texts = texts_for(&state, &intent);
    let mut other = match spec::kernel_transition(&state, &intent, &context, &texts) {
        Ok(result) => result,
        Err(_) => panic!("the generated kernel refused what the hand kernel accepted"),
    };
    assert_eq!(hand, other);
    other.evidence.pop();
    assert_ne!(hand, other, "dropping a check must make the results differ");
}
