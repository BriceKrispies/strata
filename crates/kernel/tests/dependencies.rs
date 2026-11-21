//! The dependency graph, and the class and impact of a schema change (`strata_kernel::dependencies`).
//!
//! The starting point is the notes tenant of `testkit` with `dossier` (`title`, `memo`),
//! `dossier_v3` (`title`) and `ticket` (`code`, `status`, and a single reference `a` to `dossier`).

use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::dependencies::{classify, impact, ChangeClass, DependencyGraph, DependentId};
use strata_kernel::testkit::*;
use strata_kernel::*;

fn tid() -> TenantId {
    tenant_id("alpha")
}

fn at() -> Context {
    ctx("alpha", user("alpha:owner"), 20_000)
}

fn schema_of(world: &World) -> TenantSchema {
    TenantSchema::clone(&world.tenants[&tid()].schema)
}

fn change(world: &World, changes: Vec<SchemaChange>) -> Result<World, DomainError> {
    world
        .step(
            &Intent::ChangeSchema {
                changes,
                expected_version: None,
                existing: ExistingData::Reject,
            },
            &at(),
        )
        .map(|(world, _)| world)
}

fn accepted(world: &World, changes: Vec<SchemaChange>) -> World {
    change(world, changes).expect("the change is accepted")
}

fn object_def(id: &str) -> SchemaChange {
    SchemaChange::DefineObject {
        id: object(id),
        label: id.to_string(),
    }
}

fn typed_field(of: &str, name: &str, ty: FieldType, required: bool) -> SchemaChange {
    SchemaChange::DefineField {
        object: object(of),
        field: field_def(name, ty, required),
    }
}

fn str_field(of: &str, name: &str) -> SchemaChange {
    typed_field(of, name, FieldType::Str, false)
}

fn reference(of: &str, name: &str, target: &str) -> SchemaChange {
    SchemaChange::DefineRelationship {
        object: object(of),
        field: field(name),
        label: name.to_string(),
        target: object(target),
        on_delete: OnDelete::Restrict,
        required: false,
    }
}

fn remove_field(of: &str, name: &str) -> SchemaChange {
    SchemaChange::RemoveField {
        object: object(of),
        field: field(name),
    }
}

fn query_def(id: &str, of: &str, filter: Option<Expr>) -> QueryDef {
    QueryDef {
        id: QueryId::new(id).unwrap(),
        object: object(of),
        filter,
        params: BTreeSet::new(),
        order: vec![],
        limit: None,
        aggregates: None,
        declared_fields: None,
    }
}

fn through_title() -> Expr {
    eq(
        Expr::Through(Scope::Record, "a".into(), "title".into()),
        lit("T"),
    )
}

fn here(name: &str) -> Expr {
    var(Scope::Record, name)
}

fn gate() -> SchemaChange {
    SchemaChange::DefinePolicy(PolicyDef {
        id: PolicyId::new("gate").unwrap(),
        role: role("writer"),
        resource: Resource::Object(object("ticket")),
        actions: BTreeSet::from([Action::Read]),
        condition: Some(through_title()),
    })
}

fn award() -> WorkflowId {
    WorkflowId::new("award").unwrap()
}

fn award_flow() -> AutomationDef {
    let done = StepName::new("done").unwrap();
    AutomationDef {
        id: award(),
        label: "Award".into(),
        run_as: RunAs {
            roles: BTreeSet::from([role("editor")]),
        },
        run_key: None,
        start: done.clone(),
        steps: BTreeMap::from([(
            done,
            AutomationStepDef {
                action: AutomationStep::CreateRecord {
                    object: object("ticket"),
                    fields: BTreeMap::from([(field("code"), lit("x"))]),
                },
                next: None,
                retry: None,
                timeout: None,
                compensate: None,
            },
        )]),
        defined_by: None,
        defined_at: None,
        suspended_for: None,
        state: None,
    }
}

fn on_ticket() -> TriggerDef {
    TriggerDef {
        id: TriggerId::new("on_ticket").unwrap(),
        on: TriggerEvent::RecordCreated {
            object: object("ticket"),
        },
        condition: Some(eq(here("code"), lit("x"))),
        automation: award(),
        paused: None,
        defined_by: None,
        defined_at: None,
    }
}

fn by_title_view() -> SchemaChange {
    SchemaChange::DefineView(ViewDef {
        id: ViewId::new("tickets").unwrap(),
        route: "/tickets".into(),
        title: "Tickets".into(),
        sections: BTreeMap::from([(
            "main".to_string(),
            ViewSection {
                query: QueryId::new("by_title").unwrap(),
                args: BTreeMap::new(),
                include: vec![field("a")],
                markdown: vec![],
                single: false,
                aggregate: None,
            },
        )]),
    })
}

/// The tenant with the objects, a policy and a query that read `dossier.title` through `a`, a
/// view over the query, a constraint, a composite unique constraint, an automation that writes
/// `ticket.code` and a trigger that reads it.
fn tenant() -> World {
    accepted(
        &notes_world(),
        vec![
            object_def("dossier"),
            str_field("dossier", "title"),
            str_field("dossier", "memo"),
            object_def("dossier_v3"),
            str_field("dossier_v3", "title"),
            object_def("ticket"),
            str_field("ticket", "code"),
            str_field("ticket", "status"),
            reference("ticket", "a", "dossier"),
            SchemaChange::DefineQuery(query_def("by_title", "ticket", Some(through_title()))),
            SchemaChange::DefineQuery(query_def(
                "open_tickets",
                "ticket",
                Some(eq(here("status"), lit("open"))),
            )),
            by_title_view(),
            gate(),
            SchemaChange::DefineConstraint {
                object: object("ticket"),
                constraint: ConstraintDef {
                    id: ConstraintId::new("code_present").unwrap(),
                    expr: Expr::Not(Box::new(Expr::IsNull(Box::new(here("code"))))),
                    message: "a code is needed".into(),
                },
            },
            SchemaChange::DefineUnique {
                object: object("ticket"),
                unique: UniqueDef {
                    id: ConstraintId::new("code_status").unwrap(),
                    fields: vec![field("code"), field("status")],
                    message: "already taken".into(),
                },
            },
            SchemaChange::DefineAutomation(award_flow()),
            SchemaChange::DefineTrigger(on_ticket()),
        ],
    )
}

fn cells(pairs: &[(&str, &str)]) -> BTreeSet<(ObjectTypeId, FieldId)> {
    pairs.iter().map(|(o, f)| (object(o), field(f))).collect()
}

fn objects(names: &[&str]) -> BTreeSet<ObjectTypeId> {
    names.iter().map(|n| object(n)).collect()
}

/// The read set of each kind of dependent is what its definition names, at the level of a field,
/// and nothing else: a read through a reference names the reference field and the field reached.
///
/// Verifies: INT-2910
#[test]
fn the_read_set_of_a_dependent_is_what_its_definition_names() {
    let schema = schema_of(&tenant());
    let graph = DependencyGraph::of(&schema);
    let reads = |id: DependentId| graph.reads.get(&id).expect("the dependent is in the graph");

    let query = reads(DependentId::Query(QueryId::new("by_title").unwrap()));
    assert_eq!(query.cells, cells(&[("ticket", "a"), ("dossier", "title")]));
    assert_eq!(query.objects, objects(&["ticket", "dossier"]));
    assert!(query.uses.is_empty());

    let policy = reads(DependentId::Policy(PolicyId::new("gate").unwrap()));
    assert_eq!(
        policy.cells,
        cells(&[("ticket", "a"), ("dossier", "title")])
    );

    let open = reads(DependentId::Query(QueryId::new("open_tickets").unwrap()));
    assert_eq!(open.cells, cells(&[("ticket", "status")]));
    assert_eq!(open.objects, objects(&["ticket"]));

    let view = reads(DependentId::View(ViewId::new("tickets").unwrap()));
    assert_eq!(view.cells, cells(&[("ticket", "a")]));
    assert_eq!(
        view.uses,
        BTreeSet::from([DependentId::Query(QueryId::new("by_title").unwrap())])
    );

    let constraint = reads(DependentId::Constraint(
        object("ticket"),
        ConstraintId::new("code_present").unwrap(),
    ));
    assert_eq!(constraint.cells, cells(&[("ticket", "code")]));

    let unique = reads(DependentId::Unique(
        object("ticket"),
        ConstraintId::new("code_status").unwrap(),
    ));
    assert_eq!(
        unique.cells,
        cells(&[("ticket", "code"), ("ticket", "status")])
    );

    let automation = reads(DependentId::Automation(award()));
    assert_eq!(automation.cells, cells(&[("ticket", "code")]));
    assert_eq!(automation.objects, objects(&["ticket"]));

    let trigger = reads(DependentId::Trigger(TriggerId::new("on_ticket").unwrap()));
    assert_eq!(trigger.cells, cells(&[("ticket", "code")]));
    assert_eq!(
        trigger.uses,
        BTreeSet::from([DependentId::Automation(award())])
    );

    let link = reads(DependentId::Reference(object("ticket"), field("a")));
    assert_eq!(link.objects, objects(&["dossier"]));
    assert!(link.cells.is_empty());

    assert_eq!(
        graph.readers_of(&(object("dossier"), field("memo"))),
        BTreeSet::new(),
        "nothing reads dossier.memo"
    );
}

/// The class of a change is computed from what the change does, not stated by its author.
///
/// Verifies: MIG-589
#[test]
fn every_change_form_has_a_class() {
    let world = accepted(
        &tenant(),
        vec![typed_field("dossier", "lang", FieldType::Language, false)],
    );
    let schema = schema_of(&world);
    let class = |c: SchemaChange| classify(&c, &schema);

    // Compatible: add a field, widen a type, replace a definition, change what no dependent reads.
    assert_eq!(
        class(str_field("dossier", "extra")),
        ChangeClass::Compatible
    );
    assert_eq!(
        class(typed_field("dossier", "lang", FieldType::Str, false)),
        ChangeClass::Compatible
    );
    assert_eq!(
        class(typed_field("ticket", "code", FieldType::Str, true)),
        ChangeClass::Compatible
    );
    assert_eq!(
        class(SchemaChange::DefineQuery(query_def(
            "by_title", "ticket", None
        ))),
        ChangeClass::Compatible
    );
    assert_eq!(class(object_def("fresh")), ChangeClass::Compatible);

    // Breaking: remove, narrow, retarget a reference, turn a single value into a list.
    assert_eq!(
        class(remove_field("dossier", "memo")),
        ChangeClass::Breaking
    );
    assert_eq!(
        class(SchemaChange::RemoveObject {
            id: object("dossier"),
            delete_records: false
        }),
        ChangeClass::Breaking
    );
    assert_eq!(
        class(typed_field("ticket", "code", FieldType::Int, false)),
        ChangeClass::Breaking
    );
    assert_eq!(
        class(typed_field("dossier", "lang", FieldType::Int, false)),
        ChangeClass::Breaking
    );
    assert_eq!(
        class(reference("ticket", "a", "dossier_v3")),
        ChangeClass::Breaking
    );
    assert_eq!(
        class(reference("ticket", "a", "dossier")),
        ChangeClass::Compatible
    );
    let list = FieldType::List(Box::new(FieldType::Ref {
        target: object("dossier"),
        on_delete: OnDelete::Restrict,
    }));
    assert_eq!(
        class(typed_field("ticket", "a", list, false)),
        ChangeClass::Breaking
    );
    assert_eq!(
        class(SchemaChange::RemoveQuery(QueryId::new("by_title").unwrap())),
        ChangeClass::Breaking
    );
}

/// The impact of a change is the dependents that read what it touches, directly or through
/// another dependent: a removal of the field a query filters through reaches the query and the
/// view over the query, and a removal nobody reads reaches nothing.
#[test]
fn the_impact_of_a_change_is_what_reads_it_directly_or_through_another_dependent() {
    let schema = schema_of(&tenant());
    let query = DependentId::Query(QueryId::new("by_title").unwrap());
    let policy = DependentId::Policy(PolicyId::new("gate").unwrap());
    let view = DependentId::View(ViewId::new("tickets").unwrap());

    assert_eq!(
        impact(&remove_field("dossier", "title"), &schema),
        BTreeSet::from([query.clone(), policy.clone(), view.clone()]),
        "the view reaches the field through the query, and also reads the reference `a`"
    );
    assert_eq!(
        impact(&remove_field("dossier", "memo"), &schema),
        BTreeSet::new()
    );
    assert_eq!(
        impact(
            &SchemaChange::RemoveQuery(QueryId::new("by_title").unwrap()),
            &schema
        ),
        BTreeSet::from([view]),
        "a view that uses the removed query is reached; the query itself is not in its impact"
    );
    let removed = impact(
        &SchemaChange::RemoveObject {
            id: object("dossier"),
            delete_records: false,
        },
        &schema,
    );
    assert!(removed.contains(&DependentId::Reference(object("ticket"), field("a"))));
    assert!(removed.contains(&query));
    assert!(removed.contains(&policy));
    assert!(!removed.contains(&DependentId::Query(QueryId::new("open_tickets").unwrap())));
}

/// A breaking change that passes the consistency check of the whole schema is still refused when a
/// dependent reads what it changes, naming each dependent. `ticket.a` is retargeted to an object
/// type that has a `title` too, so the query and the policy still type-check against it.
///
/// Verifies: MIG-592
#[test]
fn a_breaking_change_is_refused_naming_each_affected_dependent() {
    let world = tenant();
    let err = change(&world, vec![reference("ticket", "a", "dossier_v3")])
        .expect_err("the retarget is refused");
    assert_eq!(err.kind, ErrorKind::InvalidSchema);
    assert!(err.message.contains("by_title"), "{}", err.message);
    assert!(err.message.contains("gate"), "{}", err.message);
    let named: Vec<&str> = err
        .evidence
        .iter()
        .filter(|c| !c.passed && c.rule == "schema:impact")
        .map(|c| c.detail.as_str())
        .collect();
    assert!(
        named.iter().any(|d| d.contains("query by_title")),
        "{named:?}"
    );
    assert!(named.iter().any(|d| d.contains("policy gate")), "{named:?}");
}

/// A field removed and defined again by one change is another entity: the dependents that read
/// its name are refused, so none binds to the new field. The same holds for an object type.
///
/// Verifies: MIG-604
#[test]
fn a_dependent_does_not_bind_to_a_new_entity_that_takes_the_name_of_the_removed_one() {
    let world = tenant();
    let err = change(
        &world,
        vec![
            remove_field("dossier", "title"),
            str_field("dossier", "title"),
        ],
    )
    .expect_err("the field is refused");
    assert_eq!(err.kind, ErrorKind::InvalidSchema);
    assert!(err.message.contains("by_title"), "{}", err.message);

    let err = change(
        &world,
        vec![
            SchemaChange::RemoveObject {
                id: object("dossier"),
                delete_records: false,
            },
            object_def("dossier"),
            str_field("dossier", "title"),
        ],
    )
    .expect_err("the object type is refused");
    assert_eq!(err.kind, ErrorKind::InvalidSchema);
    assert!(err.message.contains("by_title"), "{}", err.message);
    assert!(err.message.contains("ticket.a"), "{}", err.message);
}

/// The refusal is not wider than the reads: a name that no dependent reads may be removed and
/// defined again, and a removal is fine when the dependent that read it goes in the same change.
#[test]
fn a_name_nothing_reads_may_be_removed_and_defined_again() {
    let world = accepted(
        &tenant(),
        vec![
            remove_field("dossier", "memo"),
            str_field("dossier", "memo"),
        ],
    );
    assert!(schema_of(&world).objects[&object("dossier")]
        .fields
        .contains_key(&field("memo")));

    let world = accepted(
        &world,
        vec![
            SchemaChange::RemoveView(ViewId::new("tickets").unwrap()),
            SchemaChange::RemoveQuery(QueryId::new("by_title").unwrap()),
            SchemaChange::RemovePolicy(PolicyId::new("gate").unwrap()),
            remove_field("dossier", "title"),
        ],
    );
    assert!(!schema_of(&world).objects[&object("dossier")]
        .fields
        .contains_key(&field("title")));
}

/// The evidence of an accepted change carries the class of each change and the dependents it
/// reaches.
#[test]
fn the_evidence_of_a_change_shows_its_class_and_impact() {
    let (_, outcome) = tenant()
        .step(
            &Intent::ChangeSchema {
                changes: vec![
                    str_field("dossier", "extra"),
                    remove_field("dossier", "memo"),
                ],
                expected_version: None,
                existing: ExistingData::Reject,
            },
            &at(),
        )
        .expect("the change is accepted");
    let lines: Vec<&str> = outcome
        .evidence
        .iter()
        .filter(|c| c.rule == "schema:class")
        .map(|c| c.detail.as_str())
        .collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].contains("define field dossier.extra") && lines[0].contains("compatible"));
    assert!(lines[1].contains("remove field dossier.memo") && lines[1].contains("breaking"));
}
