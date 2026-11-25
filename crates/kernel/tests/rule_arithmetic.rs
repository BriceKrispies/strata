//! Checked arithmetic, time arithmetic and text in the tenant rule language (D-6540 to D-6543,
//! WG-5064) at the three places the kernel evaluates a rule: a policy condition, a workflow guard
//! (and the assignment of a transition) and a query filter. An overflow is the evaluation error
//! `integer-overflow`: the policy fails with it as evidence, the guard refuses the transition with
//! it, the filter leaves the record out. Nothing panics, wraps or saturates.

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::expr::EvalError;
use strata_kernel::testkit::*;
use strata_kernel::*;

const DAY: i64 = 86_400_000;
const HOUR: i64 = 3_600_000;

fn rule(source: &str) -> Expr {
    expr::parse(source).unwrap_or_else(|e| panic!("{source}: {e}"))
}

fn define_policy(
    id: &str,
    role_name: &str,
    actions: &[Action],
    condition: Option<&str>,
) -> SchemaChange {
    SchemaChange::DefinePolicy(PolicyDef {
        id: PolicyId::new(id).expect("policy id"),
        role: role(role_name),
        resource: Resource::Object(object("offer")),
        actions: actions.iter().copied().collect(),
        condition: condition.map(rule),
    })
}

fn offer_field(field: FieldDef) -> SchemaChange {
    SchemaChange::DefineField {
        object: object("offer"),
        field,
    }
}

/// The schema of time-boxed offers: a window, a quota, a stage workflow whose guards and
/// assignment compute with times, policies whose conditions do, and queries whose filters do.
fn offer_schema() -> Vec<SchemaChange> {
    let all = [Action::Read, Action::Create, Action::Update, Action::Delete];
    vec![
        SchemaChange::DefineObject {
            id: object("offer"),
            label: "Offer".into(),
        },
        offer_field(field_def("title", FieldType::Str, true)),
        offer_field(field_def("starts_at", FieldType::Time, true)),
        offer_field(field_def("ends_at", FieldType::Time, true)),
        offer_field(field_def("quota", FieldType::Int, false)),
        offer_field(field_def("expires_at", FieldType::Time, false)),
        offer_field(FieldDef {
            default: Some(lit("open")),
            ..field_def("stage", FieldType::Str, true)
        }),
        SchemaChange::DefineWorkflow(WorkflowDef {
            id: WorkflowId::new("offer_stage").expect("id"),
            object: object("offer"),
            field: field("stage"),
            initial: BTreeSet::from(["open".to_string()]),
            transitions: vec![
                WorkflowTransition {
                    name: "close".into(),
                    from: "open".into(),
                    to: "closed".into(),
                    guard: Some(rule("ms_between(new.starts_at, now) >= days(1)")),
                    set: BTreeMap::from([(field("expires_at"), rule("now + days(30)"))]),
                },
                WorkflowTransition {
                    name: "archive".into(),
                    from: "closed".into(),
                    to: "archived".into(),
                    guard: Some(rule("new.quota * 4503599627370496 > 0")),
                    set: BTreeMap::new(),
                },
            ],
        }),
        define_policy(
            "writers_read_offers",
            "writer",
            &[Action::Read, Action::Create],
            None,
        ),
        define_policy(
            "writers_edit_while_running",
            "writer",
            &[Action::Update],
            Some("now < record.ends_at - minutes(5) and record.quota * 2 + 1 > 3"),
        ),
        define_policy(
            "writers_edit_big",
            "writer",
            &[Action::Update],
            Some("record.quota * 4503599627370496 > 0"),
        ),
        define_policy("editors_manage_offers", "editor", &all, None),
        SchemaChange::DefineQuery(QueryDef {
            id: QueryId::new("running_offers").expect("id"),
            object: object("offer"),
            filter: Some(rule(
                "record.starts_at <= param.at and param.at < record.ends_at + hours(1)",
            )),
            params: BTreeSet::from(["at".to_string()]),
            order: vec![],
            limit: None,
            aggregates: None,
            declared_fields: None,
            tree: None,
        }),
        SchemaChange::DefineQuery(QueryDef {
            id: QueryId::new("big_offers").expect("id"),
            object: object("offer"),
            filter: Some(rule("record.quota * 1000000000000 > 5000000000000")),
            params: BTreeSet::new(),
            order: vec![OrderKey {
                field: "quota".into(),
                descending: true,
            }],
            limit: None,
            aggregates: None,
            declared_fields: None,
            tree: None,
        }),
    ]
}

fn step(
    world: &World,
    who: &str,
    now: i64,
    intent: Intent,
) -> Result<(World, Transition), DomainError> {
    world.step(&intent, &ctx("alpha", user(&format!("alpha:{who}")), now))
}

fn offers_world() -> World {
    step(
        &notes_world(),
        "owner",
        2_000,
        Intent::ChangeSchema {
            changes: offer_schema(),
            expected_version: None,
            existing: ExistingData::Reject,
        },
    )
    .expect("the offer schema is accepted")
    .0
}

fn create_offer(world: &World, now: i64, starts: i64, ends: i64, quota: i64) -> (World, RecordId) {
    let (world, outcome) = step(
        world,
        "writer1",
        now,
        Intent::CreateRecord {
            object: object("offer"),
            fields: BTreeMap::from([
                (field("title"), text("Offer")),
                (field("starts_at"), Value::Time(starts)),
                (field("ends_at"), Value::Time(ends)),
                (field("quota"), Value::Int(quota)),
            ]),
        },
    )
    .expect("the offer is created");
    let Output::Record { id, .. } = outcome.output else {
        panic!("a create gives the record: {:?}", outcome.output);
    };
    (world, id)
}

fn update(id: RecordId, fields: &[(&str, Value)]) -> Intent {
    Intent::UpdateRecord {
        object: object("offer"),
        id,
        fields: fields.iter().map(|(k, v)| (field(k), v.clone())).collect(),
        expected_version: None,
    }
}

fn offer(world: &World, id: RecordId) -> std::sync::Arc<Record> {
    world.tenants[&tenant_id("alpha")]
        .record(&object("offer"), id)
        .expect("offer exists")
        .clone()
}

// Verifies: CMS-1541
#[test]
fn a_workflow_guard_measures_time_and_an_overflowing_guard_refuses_the_transition() {
    let (world, id) = create_offer(&offers_world(), 5_000, 5_000, 10 * DAY, 2);
    // Less than a day after the start the close is refused.
    let early = step(
        &world,
        "editor",
        5_000 + HOUR,
        update(id, &[("stage", text("closed"))]),
    )
    .unwrap_err();
    assert_eq!(early.kind, ErrorKind::WorkflowViolation);
    assert!(
        early.message.contains("guard is not true"),
        "{}",
        early.message
    );
    // A day after the start it fires and sets expires_at thirty days later.
    let closed_at = 5_000 + DAY;
    let world = step(
        &world,
        "editor",
        closed_at,
        update(id, &[("stage", text("closed"))]),
    )
    .expect("the close fires")
    .0;
    assert_eq!(
        offer(&world, id).get(&field("expires_at")),
        &Value::Time(closed_at + 30 * DAY)
    );
    // 2 * 2^52 is 2^53, past the range: the guard is an evaluation error, the transition is
    // refused with it, and the record is unchanged.
    let overflow = step(
        &world,
        "editor",
        closed_at + 1,
        update(id, &[("stage", text("archived"))]),
    )
    .unwrap_err();
    assert_eq!(overflow.kind, ErrorKind::WorkflowViolation);
    assert!(
        overflow.message.contains("archive: integer-overflow"),
        "{}",
        overflow.message
    );
    assert_eq!(offer(&world, id).get(&field("stage")), &text("closed"));
}

// Verifies: CMS-1540
#[test]
fn a_query_filter_computes_and_leaves_out_a_record_it_overflows_on() {
    let world = offers_world();
    let (world, small) = create_offer(&world, 5_000, 0, DAY, 6);
    let (world, edge) = create_offer(&world, 5_001, 0, DAY, 9007);
    let (world, over) = create_offer(&world, 5_002, 0, DAY, 9008);
    let read = |query: &str, params: BTreeMap<String, Value>, now: i64| -> Vec<RecordId> {
        world
            .read(
                &ReadRequest::Named {
                    query: QueryId::new(query).unwrap(),
                    params,
                    page_size: None,
                    cursor: None,
                },
                &ctx("alpha", user("alpha:editor"), now),
            )
            .expect("the query is answered")
            .rows
            .iter()
            .map(|r| r.id)
            .collect()
    };
    // 9007 * 10^12 is within 2^53-1; 9008 * 10^12 is not, so that record is left out.
    assert_eq!(
        read("big_offers", BTreeMap::new(), 6_000),
        vec![edge, small]
    );
    // The window with an hour of grace after the end.
    let at = |t: i64| BTreeMap::from([("at".to_string(), Value::Time(t))]);
    assert_eq!(read("running_offers", at(DAY + HOUR - 1), 6_000).len(), 3);
    assert!(read("running_offers", at(DAY + HOUR), 6_000).is_empty());

    // The filter on the left-out record is the evaluation error, not a panic or a false.
    let record = offer(&world, over);
    let params = BTreeMap::new();
    let roles = BTreeSet::new();
    let env = expr::Env {
        record: Some(&*record),
        old: None,
        new: None,
        params: &params,
        now: Timestamp(6_000),
        principal: &Principal::Anonymous,
        roles: &roles,
        lookup: None,
    };
    assert_eq!(
        expr::eval(&rule("record.quota * 1000000000000"), &env),
        Err(EvalError("integer-overflow".to_string()))
    );
    assert_eq!(
        expr::eval(&rule("concat(text(record.quota), ':') "), &env),
        Ok(text("9008:"))
    );
}

// Verifies: CMS-1545
#[test]
fn a_rule_of_the_wrong_shapes_is_refused_when_it_is_defined() {
    for (condition, wanted) in [
        ("record.title + 1 > 0", "+ does not accept a Str operand"),
        (
            "1 + record.starts_at > 0",
            "+ does not accept a Time operand",
        ),
        (
            "record.starts_at * 2 > 0",
            "* does not accept a Time operand",
        ),
        (
            "text(record.tags) == 'x'",
            "text does not accept a List operand",
        ),
    ] {
        let mut changes = offer_schema();
        changes.push(offer_field(field_def(
            "tags",
            FieldType::List(Box::new(FieldType::Str)),
            false,
        )));
        changes.push(define_policy(
            "writers_bad",
            "writer",
            &[Action::Update],
            Some(condition),
        ));
        let refused = step(
            &notes_world(),
            "owner",
            2_000,
            Intent::ChangeSchema {
                changes,
                expected_version: None,
                existing: ExistingData::Reject,
            },
        )
        .unwrap_err();
        assert!(
            refused.message.contains(wanted),
            "{condition}: {}",
            refused.message
        );
    }
}
