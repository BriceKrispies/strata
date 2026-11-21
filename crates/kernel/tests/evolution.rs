//! The schema evolution situations: what the kernel does when a `ChangeSchema` edits a definition
//! that other definitions depend on.
//!
//! Each test asserts the wanted outcome of the design. A change that breaks a dependent is
//! refused and names it; a compatible change leaves every dependent as it was; disjoint changes
//! commute and overlapping ones are held by the revision guard. A situation the kernel cannot
//! produce today is ignored, with the gap it shows in its reason (`cargo test -- --ignored`
//! prints the gap table).
//!
//! The starting point is the notes tenant of `testkit`, with these object types added:
//! `dossier` (A: `title`, `memo`), `dossier_v2` (a second reference target: `kind`), and `ticket`
//! (B: `code`, `status`, and a single reference `a` to `dossier`). The `open_tickets` query reads
//! tickets by `status`. Each test adds what its situation needs.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use strata_kernel::testkit::*;
use strata_kernel::*;

const NOW: i64 = 20_000;

fn tid() -> TenantId {
    tenant_id("alpha")
}

fn owner() -> Principal {
    user("alpha:owner")
}

fn at() -> Context {
    ctx("alpha", owner(), NOW)
}

fn schema_of(world: &World) -> TenantSchema {
    TenantSchema::clone(&world.tenants[&tid()].schema)
}

fn version(world: &World) -> SchemaVersion {
    schema_of(world).version
}

fn change_with(
    world: &World,
    changes: Vec<SchemaChange>,
    expected: Option<SchemaVersion>,
    existing: ExistingData,
) -> Result<(World, Transition), DomainError> {
    world.step(
        &Intent::ChangeSchema {
            changes,
            expected_version: expected,
            existing,
        },
        &at(),
    )
}

fn change(world: &World, changes: Vec<SchemaChange>) -> Result<World, DomainError> {
    change_with(world, changes, None, ExistingData::Reject).map(|(world, _)| world)
}

fn accepted(world: &World, changes: Vec<SchemaChange>) -> World {
    change(world, changes).expect("the change is accepted")
}

fn refused(world: &World, changes: Vec<SchemaChange>) -> DomainError {
    change(world, changes).expect_err("the change is refused")
}

fn create(world: &World, of: &str, fields: Vec<(&str, Value)>) -> (World, RecordId) {
    let intent = Intent::CreateRecord {
        object: object(of),
        fields: fields
            .into_iter()
            .map(|(name, value)| (field(name), value))
            .collect(),
    };
    let (world, outcome) = world.step(&intent, &at()).expect("the record is accepted");
    match outcome.output {
        Output::Record { id, .. } => (world, id),
        _ => panic!("a record was expected"),
    }
}

fn stored(world: &World, of: &str, id: RecordId) -> Arc<Record> {
    world.tenants[&tid()].records[&object(of)][&id].clone()
}

/// The ids of the rows of a named query, as the owner reads them.
fn rows(world: &World, query: &str) -> Vec<u64> {
    let request = ReadRequest::Named {
        query: QueryId::new(query).unwrap(),
        params: BTreeMap::new(),
        page_size: None,
        cursor: None,
    };
    read(&world.state(&tid()), &request, &at())
        .expect("the query reads")
        .rows
        .iter()
        .map(|row| row.id.0)
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Definitions used by the situations.
// ---------------------------------------------------------------------------------------------

fn object_def(id: &str) -> SchemaChange {
    SchemaChange::DefineObject {
        id: object(id),
        label: id.to_string(),
    }
}

fn str_field(of: &str, name: &str) -> SchemaChange {
    SchemaChange::DefineField {
        object: object(of),
        field: field_def(name, FieldType::Str, false),
    }
}

fn typed_field(of: &str, name: &str, ty: FieldType, required: bool) -> SchemaChange {
    SchemaChange::DefineField {
        object: object(of),
        field: field_def(name, ty, required),
    }
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

fn query(id: &str, of: &str, filter: Option<Expr>) -> SchemaChange {
    SchemaChange::DefineQuery(QueryDef {
        id: QueryId::new(id).unwrap(),
        object: object(of),
        filter,
        params: BTreeSet::new(),
        order: vec![],
        limit: None,
        aggregates: None,
        declared_fields: None,
    })
}

/// A field of the record the expression is evaluated on.
fn here(name: &str) -> Expr {
    var(Scope::Record, name)
}

/// The field `target` of the record that the reference field `reference` points at.
fn through(reference: &str, target: &str) -> Expr {
    Expr::Through(Scope::Record, reference.to_string(), target.to_string())
}

fn is_text(expr: Expr, text: &str) -> Expr {
    eq(expr, lit(text))
}

fn gate() -> SchemaChange {
    SchemaChange::DefinePolicy(PolicyDef {
        id: PolicyId::new("gate").unwrap(),
        role: role("writer"),
        resource: Resource::Object(object("ticket")),
        actions: BTreeSet::from([Action::Read]),
        condition: Some(is_text(through("a", "title"), "T")),
    })
}

fn award() -> WorkflowId {
    WorkflowId::new("award").unwrap()
}

/// An automation of one step, which runs `action` and ends.
fn flow(action: AutomationStep) -> AutomationDef {
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
                action,
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

fn ticket_created_trigger(condition: Expr) -> TriggerDef {
    TriggerDef {
        id: TriggerId::new("on_ticket").unwrap(),
        on: TriggerEvent::RecordCreated {
            object: object("ticket"),
        },
        condition: Some(condition),
        automation: award(),
        paused: None,
        defined_by: None,
        defined_at: None,
    }
}

fn ticket_flow(initial: &[&str], transitions: Vec<WorkflowTransition>) -> SchemaChange {
    SchemaChange::DefineWorkflow(WorkflowDef {
        id: WorkflowId::new("ticket_flow").unwrap(),
        object: object("ticket"),
        field: field("status"),
        initial: initial.iter().map(|s| s.to_string()).collect(),
        transitions,
    })
}

fn step(name: &str, from: &str, to: &str, set: BTreeMap<FieldId, Expr>) -> WorkflowTransition {
    WorkflowTransition {
        name: name.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        guard: None,
        set,
    }
}

/// The notes tenant with `dossier`, `dossier_v2`, `ticket` and the `open_tickets` query.
fn catalogue() -> World {
    accepted(
        &notes_world(),
        vec![
            object_def("dossier"),
            str_field("dossier", "title"),
            str_field("dossier", "memo"),
            object_def("dossier_v2"),
            str_field("dossier_v2", "kind"),
            object_def("ticket"),
            str_field("ticket", "code"),
            str_field("ticket", "status"),
            reference("ticket", "a", "dossier"),
            query(
                "open_tickets",
                "ticket",
                Some(is_text(here("status"), "open")),
            ),
        ],
    )
}

// ---------------------------------------------------------------------------------------------
// The situations.
// ---------------------------------------------------------------------------------------------

/// Situation 1: a chain. `dossier` (A) is read by `ticket` (B) through the reference `a`, a
/// policy on `ticket` reads `dossier.title` through it, and `open_tickets` (C) filters `ticket`
/// on its own field. Adding a field to A.
///
/// Wanted: the change is accepted, the policy and the query stay, and the query's rows are the
/// same before and after.
///
/// Today: an added field is `DefineField` on `dossier` (`transition.rs` `apply_change`,
/// around line 1579, define-or-replace), which only touches `dossier` (and any reference target,
/// `touched_objects` at `transition.rs:422`). `schema.rs` `problems()` re-checks the policy and
/// the query, and both still type-check, so the change is accepted. The rows are not rewritten.
#[test]
fn situation_01_add_field_to_a_keeps_everything_valid() {
    let world = accepted(&catalogue(), vec![gate()]);
    let (world, d) = create(&world, "dossier", vec![("title", text("T"))]);
    let (world, _) = create(
        &world,
        "ticket",
        vec![
            ("code", text("c1")),
            ("status", text("open")),
            ("a", Value::Ref(d)),
        ],
    );
    let before = rows(&world, "open_tickets");
    assert_eq!(before.len(), 1);
    let world = accepted(&world, vec![str_field("dossier", "extra")]);
    assert_eq!(rows(&world, "open_tickets"), before);
    assert!(schema_of(&world)
        .policies
        .contains_key(&PolicyId::new("gate").unwrap()));
    assert!(schema_of(&world).objects[&object("dossier")]
        .fields
        .contains_key(&field("extra")));
}

/// Situation 2: a rename of `dossier.title` while the policy `gate` on `ticket` reads it through
/// `a`. There is no rename change; the nearest is removing `title` and defining `heading` in the
/// same change.
///
/// Wanted: either the policy is rewritten, or the change is refused naming `gate`. It must never
/// be accepted with the policy left reading a field that is gone.
///
/// Today: `problems()` checks each policy condition with `expr::check`, and `Through` looks up
/// `title` on `dossier` (`expr.rs` `TypeEnv::field_shape`, line 301; the generated checker at
/// `tenant_rule_language.rs` `Expr::Through`). It fails, and `plan_migration` returns
/// `InvalidSchema` with `policy gate: ...` (`schema.rs:401-440`). The refusal names the policy.
/// The policy is never rewritten, because the kernel has no rename.
#[test]
fn situation_02_rename_under_a_policy_is_refused_naming_the_policy() {
    let world = accepted(&catalogue(), vec![gate()]);
    let err = refused(
        &world,
        vec![
            remove_field("dossier", "title"),
            str_field("dossier", "heading"),
        ],
    );
    assert_eq!(err.kind, ErrorKind::InvalidSchema);
    assert!(err.message.contains("gate"), "{}", err.message);
}

/// Situation 3: removing `dossier.title` while a query on `ticket` filters through `a` into it.
///
/// Wanted: refused, naming the query.
///
/// Today: the query filter is checked against the final schema (`schema.rs:483-505`), and the
/// `Through` lookup fails on the missing field. The refusal reads `query by_dossier_title: ...`.
#[test]
fn situation_03_remove_a_field_a_query_filters_through_is_refused_naming_the_query() {
    let world = accepted(
        &catalogue(),
        vec![query(
            "by_dossier_title",
            "ticket",
            Some(is_text(through("a", "title"), "T")),
        )],
    );
    let err = refused(&world, vec![remove_field("dossier", "title")]);
    assert_eq!(err.kind, ErrorKind::InvalidSchema);
    assert!(err.message.contains("by_dossier_title"), "{}", err.message);
}

/// Situation 4: removing `dossier.memo`, which nothing reads.
///
/// Wanted: accepted; the record loses `memo` and keeps `title`. The kernel has no irreversibility
/// marker in its evidence, so the wanted "marked irreversible" part is not asserted.
///
/// Today: accepted. `plan_migration` strips fields that the final schema no longer has
/// (`transition.rs:1929`, `stripped`), so the record is rewritten without `memo`.
#[test]
fn situation_04_remove_a_field_nothing_depends_on_is_accepted() {
    let world = catalogue();
    let (world, d) = create(
        &world,
        "dossier",
        vec![("title", text("T")), ("memo", text("m"))],
    );
    let world = accepted(&world, vec![remove_field("dossier", "memo")]);
    let record = stored(&world, "dossier", d);
    assert!(!record.fields.contains_key(&field("memo")));
    assert_eq!(record.fields.get(&field("title")), Some(&text("T")));
}

/// Situation 5: a diamond. `ticket` (B) and `annex` (C) both reference `dossier` (A); `bundle`
/// (D) references `ticket` and `annex`; `bundles` (a query on D) reads through both. A change of
/// A that is compatible (an added field).
///
/// Wanted: accepted, and every dependent evaluates as before.
///
/// Today: accepted. Only `dossier` is touched; the query's `Through` lookups still resolve
/// (`expr.rs` `TypeEnv::target`), so its rows are unchanged.
#[test]
fn situation_05_diamond_compatible_change_keeps_every_dependent_valid() {
    let world = accepted(
        &catalogue(),
        vec![
            object_def("annex"),
            str_field("annex", "label"),
            reference("annex", "dossier_ref", "dossier"),
            object_def("bundle"),
            reference("bundle", "ticket_ref", "ticket"),
            reference("bundle", "annex_ref", "annex"),
            query(
                "bundles",
                "bundle",
                Some(Expr::And(vec![
                    is_text(through("ticket_ref", "code"), "c1"),
                    is_text(through("annex_ref", "label"), "L"),
                ])),
            ),
        ],
    );
    let (world, d) = create(&world, "dossier", vec![("title", text("T"))]);
    let (world, n) = create(
        &world,
        "annex",
        vec![("label", text("L")), ("dossier_ref", Value::Ref(d))],
    );
    let (world, t) = create(
        &world,
        "ticket",
        vec![
            ("code", text("c1")),
            ("status", text("open")),
            ("a", Value::Ref(d)),
        ],
    );
    let (world, b) = create(
        &world,
        "bundle",
        vec![("ticket_ref", Value::Ref(t)), ("annex_ref", Value::Ref(n))],
    );
    let before = rows(&world, "bundles");
    assert_eq!(before, vec![b.0]);
    let world = accepted(&world, vec![str_field("dossier", "extra")]);
    assert_eq!(rows(&world, "bundles"), before);
    assert_eq!(rows(&world, "open_tickets").len(), 1);
}

/// Situation 6: retarget `ticket.a` from `dossier` to `dossier_v2` while a ticket holds a
/// reference to a `dossier` record.
///
/// Wanted: refused (a breaking change: the reference cannot be fixed automatically), naming the
/// records and the field.
///
/// Today: `DefineRelationship` replaces the field in place (`transition.rs:1588`). `touched_objects`
/// adds `ticket` and `dossier_v2`, and `validate_record` finds the stored reference missing in
/// `dossier_v2` (`rules.rs:668-680`, `DanglingReference`). With `ExistingData::Reject` the change
/// is refused with `MigrationRejected` (`transition.rs` `change_schema`, `transition.rs:2053`). The
/// count in the message does not name the field, but the evidence does:
/// `existing:ticket/<id>:reference:ticket.a`.
#[test]
fn situation_06_retarget_a_reference_with_records_is_refused_naming_them() {
    let world = catalogue();
    let (world, d) = create(&world, "dossier", vec![("title", text("T"))]);
    let (world, t) = create(
        &world,
        "ticket",
        vec![
            ("code", text("c1")),
            ("status", text("open")),
            ("a", Value::Ref(d)),
        ],
    );
    let err = refused(&world, vec![reference("ticket", "a", "dossier_v2")]);
    assert_eq!(err.kind, ErrorKind::MigrationRejected);
    let named = err.evidence.iter().any(|c| {
        !c.passed && c.rule.starts_with("existing:ticket/") && c.rule.contains("reference:ticket.a")
    });
    assert!(
        named,
        "no evidence names the record {}: {:?}",
        t.0, err.evidence
    );
}

/// Situation 7: change the single reference `ticket.a` to a list of references, while the query
/// `tickets_by_title` filters through `a`.
///
/// Wanted: refused (breaking), naming the query.
///
/// Today: the query's `Through` needs a single reference (`TypeEnv::follow`, `expr.rs:314`,
/// `target` returns an error for a list). `problems()` refuses with `query tickets_by_title:
/// filter: ...` before any record is checked.
#[test]
fn situation_07_single_reference_to_list_is_refused_naming_the_query() {
    let world = accepted(
        &catalogue(),
        vec![query(
            "tickets_by_title",
            "ticket",
            Some(is_text(through("a", "title"), "T")),
        )],
    );
    let list = FieldType::List(Box::new(FieldType::Ref {
        target: object("dossier"),
        on_delete: OnDelete::Restrict,
    }));
    let err = refused(&world, vec![typed_field("ticket", "a", list, false)]);
    assert_eq!(err.kind, ErrorKind::InvalidSchema);
    assert!(err.message.contains("tickets_by_title"), "{}", err.message);
}

/// Situation 8: narrow `ticket.code` from text to a number while a ticket holds the text `abc`.
///
/// Wanted: refused with `ExistingData::Reject`, and with `Grandfather` accepted, the non-conforming
/// record listed.
///
/// Today: `validate_record` reports `type:ticket.code` (`rules.rs`, `TypeMismatch`).
/// `change_schema` pushes `existing:ticket/<id>:type:ticket.code` into the evidence and refuses
/// with `MigrationRejected` under Reject (`transition.rs:2053`). Under Grandfather the same
/// check is kept in the returned evidence and the change is accepted.
#[test]
fn situation_08_narrowing_a_type_is_refused_then_grandfathered_with_the_records_listed() {
    let world = catalogue();
    let (world, t) = create(
        &world,
        "ticket",
        vec![("code", text("abc")), ("status", text("open"))],
    );
    let narrow = typed_field("ticket", "code", FieldType::Int, false);
    let err = refused(&world, vec![narrow.clone()]);
    assert_eq!(err.kind, ErrorKind::MigrationRejected);

    let (after, outcome) = change_with(&world, vec![narrow], None, ExistingData::Grandfather)
        .expect("the grandfathered change is accepted");
    assert_eq!(
        schema_of(&after).objects[&object("ticket")].fields[&field("code")].ty,
        FieldType::Int
    );
    let listed = outcome.evidence.iter().any(|c| {
        !c.passed && c.rule.starts_with("existing:ticket/") && c.rule.contains("ticket.code")
    });
    assert!(listed, "the non-conforming record {} is not listed", t.0);
}

/// Situation 9: an ABA. The query `tickets_by_title` reads `dossier.title` through `a` as text.
/// The change removes `title` and defines a new `title` of another type (a number) in one go.
///
/// Wanted: refused naming the query. A dependent written against the old `title` must not bind
/// silently to the new one.
///
/// Today: the final schema is checked (`schema.rs:483-505`). The comparison `title == "T"` now
/// compares an `Int` with a `Str`, and `expr.rs`'s checker rejects it (`tenant_rule_language.rs`,
/// `Expr::Cmp` `Eq`: "cannot compare"). The refusal names the query.
#[test]
fn situation_09_aba_with_another_type_does_not_bind_silently() {
    let world = accepted(
        &catalogue(),
        vec![query(
            "tickets_by_title",
            "ticket",
            Some(is_text(through("a", "title"), "T")),
        )],
    );
    let err = refused(
        &world,
        vec![
            remove_field("dossier", "title"),
            typed_field("dossier", "title", FieldType::Int, false),
        ],
    );
    assert_eq!(err.kind, ErrorKind::InvalidSchema);
    assert!(err.message.contains("tickets_by_title"), "{}", err.message);
}

/// Situation 9b: the same ABA, but the new `title` has the same type as the old one.
///
/// Wanted: refused naming the query, because removing `title` breaks every dependent of it, even
/// when a field of that name comes back in the same change.
///
/// Today: refused by `change_schema` (`transition.rs`, `assess_changes`). The removal is a breaking
/// change (`dependencies::classify`), and its impact, read in the graph before and the graph after
/// the whole list, holds the query: it reads `dossier.title` in both, so it would bind to the new
/// field. The refusal is `InvalidSchema` and names `tickets_by_title`.
///
/// Verifies: MIG-604
#[test]
fn situation_09b_aba_with_the_same_type_must_not_rebind_silently() {
    let world = accepted(
        &catalogue(),
        vec![query(
            "tickets_by_title",
            "ticket",
            Some(is_text(through("a", "title"), "T")),
        )],
    );
    let err = refused(
        &world,
        vec![
            remove_field("dossier", "title"),
            str_field("dossier", "title"),
        ],
    );
    assert_eq!(err.kind, ErrorKind::InvalidSchema);
    assert!(err.message.contains("tickets_by_title"), "{}", err.message);
}

/// Situation 10: removing the object type `dossier` while `ticket.a` refers to it.
///
/// Wanted: refused, naming `ticket`.
///
/// Today: `object_meta::apply` removes the type (`object_meta.rs:112`), and `problems()` then finds
/// `ticket.a: refers to unknown object type dossier` (`schema.rs:294-296`). The refusal names
/// `ticket`.
#[test]
fn situation_10_delete_an_object_type_that_is_referenced_is_refused_naming_the_referrer() {
    let err = refused(
        &catalogue(),
        vec![SchemaChange::RemoveObject {
            id: object("dossier"),
            delete_records: false,
        }],
    );
    assert_eq!(err.kind, ErrorKind::InvalidSchema);
    assert!(err.message.contains("ticket"), "{}", err.message);
}

/// Situation 11: a reference cycle. `dossier.owner_ticket` refers to `ticket`, and `ticket.a`
/// refers to `dossier`. Removing `dossier.owner_ticket` breaks nothing.
///
/// Wanted: accepted; the records stay consistent, the `ticket` side keeps its reference, and the
/// `dossier` record loses only the removed field.
///
/// Today: accepted. `touched_objects` for a `RemoveField` is only `dossier` (`transition.rs:436`),
/// and the stripping in `plan_migration` removes `owner_ticket` from the stored `dossier` record.
#[test]
fn situation_11_reference_cycle_removing_one_side_stays_consistent() {
    let world = accepted(
        &catalogue(),
        vec![reference("dossier", "owner_ticket", "ticket")],
    );
    let (world, d) = create(&world, "dossier", vec![("title", text("T"))]);
    let (world, t) = create(
        &world,
        "ticket",
        vec![
            ("code", text("c1")),
            ("status", text("open")),
            ("a", Value::Ref(d)),
        ],
    );
    let (world, _) = {
        let intent = Intent::UpdateRecord {
            object: object("dossier"),
            id: d,
            fields: BTreeMap::from([(field("owner_ticket"), Value::Ref(t))]),
            expected_version: None,
        };
        let (world, outcome) = world.step(&intent, &at()).expect("the update is accepted");
        (world, outcome)
    };
    let world = accepted(&world, vec![remove_field("dossier", "owner_ticket")]);
    let dossier = stored(&world, "dossier", d);
    assert!(!dossier.fields.contains_key(&field("owner_ticket")));
    assert_eq!(dossier.fields.get(&field("title")), Some(&text("T")));
    assert_eq!(
        stored(&world, "ticket", t).fields.get(&field("a")),
        Some(&Value::Ref(d))
    );
}

/// Situation 12: a workflow transition writes `status = "closed"`, but `closed` is not a state of
/// the redefined workflow (its states are the initial states and both ends of its transitions).
///
/// Wanted: refused, naming the transition and the value.
///
/// Today: refused. `problems()` checks a transition whose `set` assigns the workflow's own field
/// a literal against the workflow's states (`schema.rs`, workflow validation), so the change
/// names the transition and the value.
#[test]
fn situation_12_a_transition_writing_a_removed_state_is_refused_naming_it() {
    let world = accepted(
        &catalogue(),
        vec![ticket_flow(
            &["open"],
            vec![step("close", "open", "closed", BTreeMap::new())],
        )],
    );
    let err = refused(
        &world,
        vec![ticket_flow(
            &["open"],
            vec![step(
                "close",
                "open",
                "done",
                BTreeMap::from([(field("status"), lit("closed"))]),
            )],
        )],
    );
    assert!(err.message.contains("closed"), "{}", err.message);
}

/// Situation 13: a trigger's condition reads `ticket.code`, and the field is removed.
///
/// Wanted: refused, naming the trigger.
///
/// Today: `trigger_problems` checks each condition against the object's fields
/// (`definitions.rs:426-490`). The refusal names `trigger on_ticket`.
#[test]
fn situation_13_remove_a_field_a_trigger_condition_reads_is_refused_naming_the_trigger() {
    let world = accepted(
        &catalogue(),
        vec![
            SchemaChange::DefineAutomation(flow(AutomationStep::End)),
            SchemaChange::DefineTrigger(ticket_created_trigger(is_text(here("code"), "x"))),
        ],
    );
    let err = refused(&world, vec![remove_field("ticket", "code")]);
    assert_eq!(err.kind, ErrorKind::InvalidSchema);
    assert!(err.message.contains("on_ticket"), "{}", err.message);
}

/// Situation 14: two authors built their changes against the same base version. The first
/// renames `dossier.title` (removes it and defines `heading`). The second adds a query over
/// `dossier.title` through `a`.
///
/// Wanted: the second is rejected by the revision guard. When it is decided again on the new
/// schema, it is refused (or rewritten), never accepted with a dangling name.
///
/// Today: `change_schema` compares `expected_version` with the current version and refuses with
/// `SchemaConflict` (`transition.rs:1983-1994`). Decided again on the new schema, the query's
/// `Through` finds no `title`, and the refusal names `by_title`.
#[test]
fn situation_14_a_stale_author_is_held_by_the_guard_and_refused_when_decided_again() {
    let base = catalogue();
    let v0 = version(&base);
    let rename = vec![
        remove_field("dossier", "title"),
        str_field("dossier", "heading"),
    ];
    let over_title = vec![query(
        "by_title",
        "ticket",
        Some(is_text(through("a", "title"), "T")),
    )];
    let (first, _) =
        change_with(&base, rename, Some(v0), ExistingData::Reject).expect("the first author lands");

    let stale = change_with(&first, over_title.clone(), Some(v0), ExistingData::Reject)
        .expect_err("the stale author is held");
    assert_eq!(stale.kind, ErrorKind::SchemaConflict);

    let v1 = version(&first);
    let decided = change_with(&first, over_title, Some(v1), ExistingData::Reject)
        .expect_err("the query over the removed name is refused");
    assert_eq!(decided.kind, ErrorKind::InvalidSchema);
    assert!(decided.message.contains("by_title"), "{}", decided.message);
}

/// Situation 15: two disjoint changes from the same base, one on `dossier` and one on `person`.
///
/// Wanted: applied in either order, the final schema is the same.
///
/// Today: each change touches its own object (`touched_objects`, `transition.rs:422`), and the
/// schema is a plain value compared with `PartialEq`. Serialised by the guard, both orders give
/// the same schema. A stale second author is also held by the guard (`SchemaConflict`).
#[test]
fn situation_15_disjoint_changes_commute_once_serialised() {
    let base = catalogue();
    let v0 = version(&base);
    let on_dossier = str_field("dossier", "extra");
    let on_person = str_field("person", "nickname");

    let (a1, _) = change_with(
        &base,
        vec![on_dossier.clone()],
        Some(v0),
        ExistingData::Reject,
    )
    .expect("first, then the other");
    let v1 = version(&a1);
    let (a2, _) = change_with(&a1, vec![on_person.clone()], Some(v1), ExistingData::Reject)
        .expect("the second lands after the first");

    let (b1, _) = change_with(
        &base,
        vec![on_person.clone()],
        Some(v0),
        ExistingData::Reject,
    )
    .expect("the other order");
    let w1 = version(&b1);
    let (b2, _) = change_with(
        &b1,
        vec![on_dossier.clone()],
        Some(w1),
        ExistingData::Reject,
    )
    .expect("the other order, second");

    assert_eq!(schema_of(&a2), schema_of(&b2));

    let stale = change_with(&a1, vec![on_person], Some(v0), ExistingData::Reject)
        .expect_err("the stale change is held");
    assert_eq!(stale.kind, ErrorKind::SchemaConflict);
}

/// Situation 16: one change carries 201 definitions, one more than the most one intent may hold.
///
/// Wanted: refused with `LimitExceeded`.
///
/// Today: refused before anything is applied (`transition.rs:1998-2006`,
/// `limits::MAX_SCHEMA_CHANGES` = 200 at `limits.rs:21`). This caps the count of changes in one
/// intent, not the dependents they touch (the item WG-5295 tracks).
#[test]
fn situation_16_more_changes_than_one_intent_may_carry_is_limit_exceeded() {
    let changes: Vec<SchemaChange> = (0..201)
        .map(|i| str_field("ticket", &format!("n{i}")))
        .collect();
    let err = refused(&catalogue(), changes);
    assert_eq!(err.kind, ErrorKind::LimitExceeded);
}

/// Situation 17: a change from outside a product to an entity it owns. The product `badges` owns
/// the object type `ticket`; a schema editor adds a field to it.
///
/// Wanted: refused, naming the product.
///
/// Today: `products::foreign_change` (`products.rs:333`) finds that `ticket` is owned and that the
/// change does not come through `badges`, and `change_schema` refuses with `InvalidSchema`
/// (`transition.rs:2010`, `foreign_change`). Covered at kernel level, so the test is run here.
#[test]
fn situation_17_a_product_owned_entity_changed_from_outside_is_refused() {
    let world = accepted(
        &catalogue(),
        vec![SchemaChange::RecordProduct(InstalledProduct {
            product: ProductId::new("badges").unwrap(),
            version: "1.0.0".into(),
            platform: None,
            owns: vec![EntityRef {
                kind: EntityKind::Object,
                id: "ticket".into(),
            }],
            schema_version: None,
        })],
    );
    let err = refused(&world, vec![str_field("ticket", "note")]);
    assert_eq!(err.kind, ErrorKind::InvalidSchema);
    assert!(err.message.contains("badges"), "{}", err.message);
}

/// Situation 18: `ticket.code` becomes required while a ticket has no code.
///
/// Wanted: refused with `Reject`; accepted with `Grandfather`, the record listed.
///
/// Today: `validate_record` reports `required:ticket.code` (`rules.rs`, `MissingRequired`), the same
/// path as situation 8, with the same results.
#[test]
fn situation_18_a_field_made_required_is_refused_then_grandfathered() {
    let world = catalogue();
    let (world, t) = create(&world, "ticket", vec![("status", text("open"))]);
    let required = typed_field("ticket", "code", FieldType::Str, true);
    let err = refused(&world, vec![required.clone()]);
    assert_eq!(err.kind, ErrorKind::MigrationRejected);

    let (_, outcome) = change_with(&world, vec![required], None, ExistingData::Grandfather)
        .expect("the grandfathered change is accepted");
    let listed = outcome.evidence.iter().any(|c| {
        !c.passed
            && c.rule.starts_with("existing:ticket/")
            && c.rule.contains("required:ticket.code")
    });
    assert!(listed, "the record {} without a code is not listed", t.0);
}

/// Situation 19: a workflow state that a record holds is removed from the workflow.
///
/// Wanted: refused, naming the workflow and the record.
///
/// Today: `DefineWorkflow` touches the workflow's object (`touched_objects`, `transition.rs:443`),
/// so every ticket is validated. `validate_record` reports `workflow_state:ticket_flow` for the
/// removed state (`rules.rs:723-736`, `WorkflowViolation`), and under Reject the change is refused
/// with `MigrationRejected`. The evidence names the record and the workflow.
#[test]
fn situation_19_a_workflow_state_a_record_holds_is_refused_naming_the_record() {
    let world = accepted(
        &catalogue(),
        vec![ticket_flow(
            &["open", "closed"],
            vec![step("close", "open", "closed", BTreeMap::new())],
        )],
    );
    let (world, t) = create(
        &world,
        "ticket",
        vec![("code", text("c1")), ("status", text("closed"))],
    );
    let err = refused(&world, vec![ticket_flow(&["open"], vec![])]);
    assert_eq!(err.kind, ErrorKind::MigrationRejected);
    let named = err.evidence.iter().any(|c| {
        !c.passed
            && c.rule.starts_with("existing:ticket/")
            && c.rule.contains("workflow_state:ticket_flow")
    });
    assert!(
        named,
        "no evidence names the record {}: {:?}",
        t.0, err.evidence
    );
}

/// Situation 20: a retarget that breaks a query. `ticket.a` moves to `dossier_v2`, which has no
/// `title`, while `by_title` filters through `a` into `title`.
///
/// Wanted: refused, naming the query.
///
/// Today: `problems()` checks the query against the final schema, and `Through` finds no `title`
/// on `dossier_v2` (`expr.rs` `TypeEnv::field_shape`). The refusal names `by_title`.
#[test]
fn situation_20_a_retarget_that_breaks_a_query_is_refused_naming_it() {
    let world = accepted(
        &catalogue(),
        vec![query(
            "by_title",
            "ticket",
            Some(is_text(through("a", "title"), "T")),
        )],
    );
    let err = refused(&world, vec![reference("ticket", "a", "dossier_v2")]);
    assert_eq!(err.kind, ErrorKind::InvalidSchema);
    assert!(err.message.contains("by_title"), "{}", err.message);
}

/// Situation 21: a constraint on `ticket` reads `code`, and `code` is removed.
///
/// Wanted: refused, naming the constraint.
///
/// Today: `problems()` checks each constraint expression (`schema.rs:328-336`) and names it
/// `ticket.code_present: ...`.
#[test]
fn situation_21_remove_a_field_a_constraint_reads_is_refused_naming_the_constraint() {
    let world = accepted(
        &catalogue(),
        vec![SchemaChange::DefineConstraint {
            object: object("ticket"),
            constraint: ConstraintDef {
                id: ConstraintId::new("code_present").unwrap(),
                expr: Expr::Not(Box::new(Expr::IsNull(Box::new(here("code"))))),
                message: "a code is needed".into(),
            },
        }],
    );
    let err = refused(&world, vec![remove_field("ticket", "code")]);
    assert_eq!(err.kind, ErrorKind::InvalidSchema);
    assert!(err.message.contains("code_present"), "{}", err.message);
}

/// Situation 22: a composite unique constraint on `ticket` names `code`, and `code` is removed.
///
/// Wanted: refused, naming the constraint.
///
/// Today: `definitions::unique_problems` reports `ticket.code_status: unknown field code`
/// (`definitions.rs:394-419`).
#[test]
fn situation_22_remove_a_field_a_unique_constraint_names_is_refused() {
    let world = accepted(
        &catalogue(),
        vec![SchemaChange::DefineUnique {
            object: object("ticket"),
            unique: UniqueDef {
                id: ConstraintId::new("code_status").unwrap(),
                fields: vec![field("code"), field("status")],
                message: "already taken".into(),
            },
        }],
    );
    let err = refused(&world, vec![remove_field("ticket", "code")]);
    assert_eq!(err.kind, ErrorKind::InvalidSchema);
    assert!(err.message.contains("code_status"), "{}", err.message);
}

/// Situation 23: an automation writes `ticket.code`, and `code` is removed.
///
/// Wanted: refused, naming the automation.
///
/// Today: `definitions::written_problems` checks every field an automation step writes, and names
/// the step (`definitions.rs:1017-1036`, `automation award.done: unknown field code`).
#[test]
fn situation_23_remove_a_field_an_automation_writes_is_refused_naming_it() {
    let world = accepted(
        &catalogue(),
        vec![SchemaChange::DefineAutomation(flow(
            AutomationStep::CreateRecord {
                object: object("ticket"),
                fields: BTreeMap::from([(field("code"), lit("x"))]),
            },
        ))],
    );
    let err = refused(&world, vec![remove_field("ticket", "code")]);
    assert_eq!(err.kind, ErrorKind::InvalidSchema);
    assert!(err.message.contains("award"), "{}", err.message);
}
