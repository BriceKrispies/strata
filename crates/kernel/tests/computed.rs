//! Kernel-computed fields (D-8307; KRN-2924 to KRN-2926): the declarations the kernel refuses, the
//! recomputation in the commit of a write, the bound on it, and the clock refresh.

use std::collections::BTreeMap;
use std::sync::Arc;
use strata_kernel::testkit::*;
use strata_kernel::*;

/// A tenant with a `task` (a title, a due time and `overdue`, which reads the clock) and a
/// `ticket` that refers to a task and computes the title of the task it refers to.
fn model() -> World {
    let overdue = Expr::Cmp(
        CmpOp::Gt,
        Box::new(Expr::Now),
        Box::new(var(Scope::Record, "due")),
    );
    let task_title = Expr::Through(Scope::Record, "task".into(), "title".into());
    let changes = vec![
        SchemaChange::DefineObject {
            id: object("task"),
            label: "Task".into(),
        },
        SchemaChange::DefineField {
            object: object("task"),
            field: field_def("title", FieldType::Str, true),
        },
        SchemaChange::DefineField {
            object: object("task"),
            field: field_def("due", FieldType::Time, false),
        },
        SchemaChange::DefineField {
            object: object("task"),
            field: FieldDef {
                computed: Some(overdue),
                ..field_def("overdue", FieldType::Bool, false)
            },
        },
        SchemaChange::DefineObject {
            id: object("ticket"),
            label: "Ticket".into(),
        },
        SchemaChange::DefineField {
            object: object("ticket"),
            field: field_def(
                "task",
                FieldType::Ref {
                    target: object("task"),
                    on_delete: OnDelete::Restrict,
                },
                false,
            ),
        },
        SchemaChange::DefineField {
            object: object("ticket"),
            field: FieldDef {
                computed: Some(task_title),
                ..field_def("task_title", FieldType::Str, false)
            },
        },
    ];
    must(
        &notes_world(),
        "owner",
        9_000,
        Intent::ChangeSchema {
            changes,
            expected_version: None,
            existing: ExistingData::Reject,
        },
    )
}

fn step(
    world: &World,
    who: &str,
    now: i64,
    intent: Intent,
) -> Result<(World, Transition), DomainError> {
    world.step(&intent, &ctx("alpha", user(&format!("alpha:{who}")), now))
}

fn must(world: &World, who: &str, now: i64, intent: Intent) -> World {
    step(world, who, now, intent).expect("accepted").0
}

/// The record a create step made, with the world it made it in.
fn created(world: &World, now: i64, intent: Intent) -> (World, RecordId) {
    let (next, transition) = step(world, "owner", now, intent).expect("accepted");
    match transition.output {
        Output::Record { id, .. } => (next, id),
        other => panic!("a create answers with the record, not {other:?}"),
    }
}

fn task(title: &str, due: Option<i64>) -> Intent {
    let mut fields = BTreeMap::from([(field("title"), text(title))]);
    if let Some(due) = due {
        fields.insert(field("due"), Value::Time(due));
    }
    Intent::CreateRecord {
        object: object("task"),
        fields,
    }
}

fn ticket(task: RecordId) -> Intent {
    Intent::CreateRecord {
        object: object("ticket"),
        fields: BTreeMap::from([(field("task"), Value::Ref(task))]),
    }
}

fn retitle(task: RecordId, title: &str) -> Intent {
    Intent::UpdateRecord {
        object: object("task"),
        id: task,
        fields: BTreeMap::from([(field("title"), text(title))]),
        expected_version: None,
    }
}

fn record(world: &World, name: &str, id: RecordId) -> Arc<Record> {
    world
        .state(&tenant_id("alpha"))
        .tenant
        .as_ref()
        .expect("alpha has a tenant")
        .record(&object(name), id)
        .cloned()
        .expect("the record exists")
}

fn refresh(world: &World, now: i64, after: Option<RecordId>) -> Result<(World, Output), DomainError> {
    let intent = Intent::RefreshComputed {
        object: object("task"),
        after,
    };
    world
        .step(&intent, &ctx("alpha", Principal::System, now))
        .map(|(next, transition)| (next, transition.output))
}

#[test]
fn a_computed_field_with_a_default_or_a_writer_is_refused() {
    let changes = |field: FieldDef| {
        vec![SchemaChange::DefineField {
            object: object("task"),
            field,
        }]
    };
    let base = model();
    let with_default = FieldDef {
        computed: Some(Expr::Lit(Value::Bool(true))),
        default: Some(Expr::Lit(Value::Bool(false))),
        ..field_def("flag", FieldType::Bool, false)
    };
    let error = step(
        &base,
        "owner",
        9_001,
        Intent::ChangeSchema {
            changes: changes(with_default),
            expected_version: None,
            existing: ExistingData::Reject,
        },
    )
    .expect_err("a computed field with a default is refused");
    assert_eq!(error.kind, ErrorKind::InvalidSchema);
    assert!(error.message.contains("no default"), "{}", error.message);

    let unique = FieldDef {
        computed: Some(Expr::Lit(Value::Bool(true))),
        unique: true,
        ..field_def("flag", FieldType::Bool, false)
    };
    let error = step(
        &base,
        "owner",
        9_002,
        Intent::ChangeSchema {
            changes: changes(unique),
            expected_version: None,
            existing: ExistingData::Reject,
        },
    )
    .expect_err("a unique computed field is refused");
    assert!(error.message.contains("cannot be unique"), "{}", error.message);

    // A computed field is never written: a create that names one is refused.
    let error = step(
        &base,
        "owner",
        9_003,
        Intent::CreateRecord {
            object: object("task"),
            fields: BTreeMap::from([
                (field("title"), text("a")),
                (field("overdue"), Value::Bool(true)),
            ]),
        },
    )
    .expect_err("a write of a computed field is refused");
    assert_eq!(error.kind, ErrorKind::InvalidIntent);
    assert!(error.message.contains("computed by the kernel"), "{}", error.message);
}

#[test]
fn a_computed_field_reads_no_reader_and_no_other_computed_field() {
    let base = model();
    let reads_principal = FieldDef {
        computed: Some(Expr::Principal),
        ..field_def("who", FieldType::Str, false)
    };
    let error = step(
        &base,
        "owner",
        9_010,
        Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineField {
                object: object("task"),
                field: reads_principal,
            }],
            expected_version: None,
            existing: ExistingData::Reject,
        },
    )
    .expect_err("a computed field that reads the principal is refused");
    assert!(error.message.contains("principal or a role"), "{}", error.message);

    let reads_computed = FieldDef {
        computed: Some(var(Scope::Record, "overdue")),
        ..field_def("again", FieldType::Bool, false)
    };
    let error = step(
        &base,
        "owner",
        9_011,
        Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineField {
                object: object("task"),
                field: reads_computed,
            }],
            expected_version: None,
            existing: ExistingData::Reject,
        },
    )
    .expect_err("a computed field that reads a computed field is refused");
    assert!(error.message.contains("computed field overdue"), "{}", error.message);
}

#[test]
fn a_write_recomputes_the_referring_record_in_its_commit() {
    let world = model();
    let (world, first) = created(&world, 9_100, task("draft", None));
    let (world, referring) = created(&world, 9_101, ticket(first));
    assert_eq!(
        record(&world, "ticket", referring).get(&field("task_title")),
        &text("draft")
    );
    let before = record(&world, "ticket", referring).version;

    // The write to the referred record and the recomputation of its referrer are one commit.
    let (world, transition) = step(&world, "owner", 9_102, retitle(first, "final"))
        .expect("the title changes");
    assert_eq!(
        record(&world, "ticket", referring).get(&field("task_title")),
        &text("final")
    );
    assert_eq!(record(&world, "ticket", referring).version, before + 1);
    let referrer_events = transition
        .events
        .iter()
        .filter(|event| matches!(&event.body, EventBody::RecordUpdated { after, .. } if after.object == object("ticket")))
        .count();
    assert_eq!(referrer_events, 1, "the referrer's change is an event of the same commit");

    // A write to a field no computed field reads leaves the referrer alone.
    let (world, _) = step(
        &world,
        "owner",
        9_103,
        Intent::UpdateRecord {
            object: object("task"),
            id: first,
            fields: BTreeMap::from([(field("due"), Value::Time(9_000_000))]),
            expected_version: None,
        },
    )
    .expect("the due time changes");
    assert_eq!(record(&world, "ticket", referring).version, before + 1);
}

#[test]
fn a_write_that_would_recompute_too_many_referring_records_is_refused() {
    let mut world = model();
    let (next, target) = created(&world, 9_200, task("shared", None));
    world = next;
    for n in 0..=limits::MAX_COMPUTED_FANOUT {
        world = must(&world, "owner", 9_201 + n as i64, ticket(target));
    }
    let error = step(&world, "owner", 20_000, retitle(target, "renamed"))
        .expect_err("the recomputation is over the bound");
    assert_eq!(error.kind, ErrorKind::LimitExceeded);
    assert_eq!(
        record(&world, "task", target).get(&field("title")),
        &text("shared"),
        "a refused write changes nothing"
    );
}

#[test]
fn a_clock_step_refreshes_only_the_values_that_changed() {
    let world = model();
    let (world, due_later) = created(&world, 10_000, task("later", Some(11_000)));
    // A refresh that the clock does not move past the due time changes nothing.
    let (world, output) = refresh(&world, 10_500, None).expect("the refresh is decided");
    assert_eq!(
        output,
        Output::Refreshed {
            changed: 0,
            more: false,
            next_after: None
        }
    );
    assert_eq!(
        record(&world, "task", due_later).get(&field("overdue")),
        &Value::Bool(false)
    );

    // The clock steps past the due time: the value is recomputed once.
    let (world, output) = refresh(&world, 12_000, None).expect("the refresh is decided");
    assert_eq!(
        output,
        Output::Refreshed {
            changed: 1,
            more: false,
            next_after: None
        }
    );
    let stored = record(&world, "task", due_later);
    assert_eq!(stored.get(&field("overdue")), &Value::Bool(true));
    assert_eq!(stored.version, 2);
    let (_, output) = refresh(&world, 12_000, None).expect("the refresh is decided");
    assert_eq!(
        output,
        Output::Refreshed {
            changed: 0,
            more: false,
            next_after: None
        }
    );

    // Only the system refreshes, and only a type that reads the clock.
    let error = step(
        &world,
        "owner",
        12_001,
        Intent::RefreshComputed {
            object: object("task"),
            after: None,
        },
    )
    .expect_err("an owner may not refresh");
    assert_eq!(error.kind, ErrorKind::Unauthorized);
    let error = world
        .step(
            &Intent::RefreshComputed {
                object: object("ticket"),
                after: None,
            },
            &ctx("alpha", Principal::System, 12_002),
        )
        .expect_err("a type with no time-dependent field has nothing to refresh");
    assert_eq!(error.kind, ErrorKind::InvalidIntent);
}

#[test]
fn a_refresh_examines_one_batch_from_its_cursor() {
    let mut world = model();
    let mut ids = Vec::new();
    for n in 0..=erasure::BATCH {
        let (next, id) = created(&world, 10_000 + n as i64, task("t", Some(11_000)));
        world = next;
        ids.push(id);
    }
    let (world, output) = refresh(&world, 12_000, None).expect("the first batch is decided");
    let Output::Refreshed {
        changed,
        more,
        next_after,
    } = output
    else {
        panic!("a refresh answers with the count of changed values");
    };
    assert_eq!(changed as usize, erasure::BATCH);
    assert!(more, "a full batch says that more may follow");
    assert_eq!(next_after, Some(ids[erasure::BATCH - 1]));

    let (_, output) = refresh(&world, 12_000, next_after).expect("the second batch is decided");
    assert_eq!(
        output,
        Output::Refreshed {
            changed: 1,
            more: false,
            next_after: None
        }
    );
}

#[test]
fn a_field_defined_over_existing_records_is_derived_for_them() {
    let world = model();
    let (world, id) = created(&world, 9_300, task("undated", None));
    let defined = FieldDef {
        computed: Some(Expr::IsNull(Box::new(var(Scope::Record, "due")))),
        ..field_def("undated_flag", FieldType::Bool, false)
    };
    let world = must(
        &world,
        "owner",
        9_301,
        Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineField {
                object: object("task"),
                field: defined,
            }],
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    assert_eq!(
        record(&world, "task", id).get(&field("undated_flag")),
        &Value::Bool(true),
        "the change that defines the field derives it for the record that exists"
    );
}
