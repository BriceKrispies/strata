//! An update is authorised against the record it will become, after the workflow's assignments,
//! not only against the record the caller wrote: a transition cannot set what the caller could not
//! have written directly.

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::testkit::*;
use strata_kernel::*;

fn ticket_schema() -> Vec<SchemaChange> {
    vec![
        SchemaChange::DefineRole(RoleDef {
            id: role("agent"),
            label: "Agent".into(),
        }),
        SchemaChange::DefineObject {
            id: object("ticket"),
            label: "Ticket".into(),
        },
        SchemaChange::DefineField {
            object: object("ticket"),
            field: field_def("title", FieldType::Str, true),
        },
        SchemaChange::DefineField {
            object: object("ticket"),
            field: FieldDef {
                default: Some(lit("open")),
                ..field_def("status", FieldType::Str, true)
            },
        },
        SchemaChange::DefineField {
            object: object("ticket"),
            field: FieldDef {
                default: Some(lit("standard")),
                ..field_def("tier", FieldType::Str, true)
            },
        },
        SchemaChange::DefineWorkflow(WorkflowDef {
            id: WorkflowId::new("ticket_flow").expect("id"),
            object: object("ticket"),
            field: field("status"),
            initial: BTreeSet::from(["open".to_string()]),
            transitions: vec![
                WorkflowTransition {
                    name: "escalate".into(),
                    from: "open".into(),
                    to: "escalated".into(),
                    guard: None,
                    set: BTreeMap::from([(field("tier"), lit("premium"))]),
                },
                WorkflowTransition {
                    name: "close".into(),
                    from: "open".into(),
                    to: "closed".into(),
                    guard: None,
                    set: BTreeMap::new(),
                },
            ],
        }),
        SchemaChange::DefinePolicy(PolicyDef {
            id: PolicyId::new("agents_read_and_open").expect("id"),
            role: role("agent"),
            resource: Resource::Object(object("ticket")),
            actions: BTreeSet::from([Action::Read, Action::Create]),
            condition: None,
        }),
        // An agent may update a ticket only while the ticket it becomes is a standard one.
        SchemaChange::DefinePolicy(PolicyDef {
            id: PolicyId::new("agents_update_standard").expect("id"),
            role: role("agent"),
            resource: Resource::Object(object("ticket")),
            actions: BTreeSet::from([Action::Update]),
            condition: Some(eq(var(Scope::New, "tier"), lit("standard"))),
        }),
    ]
}

fn patch(fields: &[(&str, Value)]) -> Intent {
    Intent::UpdateRecord {
        object: object("ticket"),
        id: RecordId(1),
        fields: fields.iter().map(|(k, v)| (field(k), v.clone())).collect(),
        expected_version: None,
    }
}

fn world_with_a_ticket() -> World {
    let world = notes_world();
    let step = |world: &World, who: &str, now: i64, intent: Intent| {
        world
            .step(&intent, &ctx("alpha", user(&format!("alpha:{who}")), now))
            .expect("fixture intent is accepted")
            .0
    };
    let world = step(
        &world,
        "owner",
        5_000,
        Intent::ChangeSchema {
            changes: ticket_schema(),
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    let world = step(
        &world,
        "owner",
        5_001,
        Intent::AssignRole {
            principal: principal_id("alpha:writer1"),
            role: role("agent"),
        },
    );
    step(
        &world,
        "writer1",
        5_002,
        Intent::CreateRecord {
            object: object("ticket"),
            fields: BTreeMap::from([(field("title"), text("Printer"))]),
        },
    )
}

#[test]
fn a_workflow_assignment_cannot_set_what_the_caller_could_not_write_directly() {
    let world = world_with_a_ticket();
    let agent = || ctx("alpha", user("alpha:writer1"), 6_000);

    // The same write made directly is refused: the policy asks for a standard ticket.
    let direct = world
        .step(&patch(&[("tier", text("premium"))]), &agent())
        .expect_err("a direct write of the tier is refused");
    assert_eq!(direct.kind, ErrorKind::Unauthorized);

    // Escalating writes nothing the policy forbids (the caller sets only the status, so the first
    // decision passes), but the step assigns the premium tier. The candidate the step makes is
    // decided again and refused.
    let escalated = world
        .step(&patch(&[("status", text("escalated"))]), &agent())
        .expect_err("the assignment reaches what the caller may not write");
    assert_eq!(escalated.kind, ErrorKind::Unauthorized);
    assert!(
        escalated.evidence.iter().any(|check| !check.passed),
        "the refusal names the failed check"
    );

    // A step that assigns nothing is decided once and accepted.
    let (after, _) = world
        .step(&patch(&[("status", text("closed"))]), &agent())
        .expect("a step that leaves the candidate as written is allowed");
    let ticket = after.tenants[&tenant_id("alpha")]
        .record(&object("ticket"), RecordId(1))
        .expect("the ticket exists");
    assert_eq!(ticket.get(&field("status")), &text("closed"));
    assert_eq!(ticket.get(&field("tier")), &text("standard"));
}
