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
use strata_kernel::dependencies::{self, ChangeClass, DependentId};
use strata_kernel::evolution::{self, PinBook, Rebase};
use strata_kernel::expr::{self, Env};
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
/// `a`. This is not a rename: it is removing `title` and defining `heading` in the
/// same change.
///
/// Wanted: either the policy is rewritten, or the change is refused naming `gate`. It must never
/// be accepted with the policy left reading a field that is gone.
///
/// Today: `problems()` checks each policy condition with `expr::check`, and `Through` looks up
/// `title` on `dossier` (`expr.rs` `TypeEnv::field_shape`, line 301; the generated checker at
/// `tenant_rule_language.rs` `Expr::Through`). It fails, and `plan_migration` returns
/// `InvalidSchema` with `policy gate: ...` (`schema.rs:401-440`). The refusal names the policy.
/// The policy is never rewritten, because a removal is not followed by a rewrite (a real rename is situation 2b).
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

// ---------------------------------------------------------------------------------------------
// Absorbing changes: adaptable changes, pins and projections, and rebase (D-8212..D-8217).
// ---------------------------------------------------------------------------------------------

fn rename(of: &str, from: &str, to: &str) -> SchemaChange {
    SchemaChange::RenameField {
        object: object(of),
        from: field(from),
        to: field(to),
    }
}

fn query_id(id: &str) -> DependentId {
    DependentId::Query(QueryId::new(id).unwrap())
}

/// Every rule of the evidence of a decision that starts with `prefix`, with its detail.
fn evidence_of(outcome: &Transition, prefix: &str) -> Vec<String> {
    outcome
        .evidence
        .iter()
        .filter(|check| check.rule.starts_with(prefix))
        .map(|check| format!("{}: {}", check.rule, check.detail))
        .collect()
}

/// Whether `expr` holds on a record, with no other record in reach.
fn holds_on(expr: &Expr, record: &Record) -> bool {
    let params = BTreeMap::new();
    let roles = BTreeSet::new();
    expr::holds(
        expr,
        &Env {
            record: Some(record),
            old: None,
            new: None,
            params: &params,
            now: Timestamp(NOW),
            principal: &Principal::System,
            roles: &roles,
            lookup: None,
        },
    )
    .expect("the expression evaluates")
}

/// A change of schema decided with a book of pins, and the world after it.
fn change_pinned(
    world: &World,
    changes: Vec<SchemaChange>,
    book: &PinBook,
) -> Result<(World, PinBook, Transition), DomainError> {
    let intent = Intent::ChangeSchema {
        changes,
        expected_version: None,
        existing: ExistingData::Reject,
    };
    let (outcome, next) = transition_pinned(&world.state(&tid()), &intent, &at(), book)?;
    let tenant = apply_delta(world.tenants.get(&tid()).map(Arc::as_ref), &outcome.delta)?;
    let mut tenants = world.tenants.clone();
    tenants.insert(tid(), Arc::new(tenant));
    Ok((World { tenants }, next, outcome))
}

/// Situation 2b: a real rename of `dossier.title` while the policy `gate` on `ticket` reads it
/// through `a`. Situation 2 stays as it is: removing a field and defining another is a removal.
///
/// Wanted: the change is adaptable and accepted; the policy is rewritten to read `heading`, the
/// values of the records are kept under the new name, the old name is refused afterwards, and the
/// query's rows are the same.
///
/// Verifies: MIG-560
#[test]
fn situation_02b_a_real_rename_is_absorbed_and_the_policy_rewritten() {
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
    let change = rename("dossier", "title", "heading");
    assert_eq!(
        dependencies::classify(&change, &schema_of(&world)),
        ChangeClass::Adaptable
    );
    let (world, outcome) = change_with(&world, vec![change], None, ExistingData::Reject)
        .expect("a rename is absorbed");
    let policy = &schema_of(&world).policies[&PolicyId::new("gate").unwrap()];
    assert_eq!(
        policy.condition,
        Some(is_text(through("a", "heading"), "T"))
    );
    let rewrites = evidence_of(&outcome, "schema:rewrite");
    assert!(
        rewrites.iter().any(|line| line.contains("policy gate")),
        "{rewrites:?}"
    );
    let record = stored(&world, "dossier", d);
    assert_eq!(record.fields.get(&field("heading")), Some(&text("T")));
    assert!(!record.fields.contains_key(&field("title")));
    assert_eq!(rows(&world, "open_tickets"), before);
    let old_name = world
        .step(
            &Intent::CreateRecord {
                object: object("dossier"),
                fields: BTreeMap::from([(field("title"), text("U"))]),
            },
            &at(),
        )
        .expect_err("the old name is refused after the rename");
    assert_ne!(old_name.kind, ErrorKind::InsufficientState);
}

/// Situation 24: a rename and a query over the new name in one change.
///
/// Wanted: accepted; the query reads the renamed field and finds the ticket whose dossier holds
/// the value the record had under the old name.
#[test]
fn situation_24_rename_then_a_query_using_the_new_name() {
    let (world, d) = create(&catalogue(), "dossier", vec![("title", text("T"))]);
    let (world, t) = create(
        &world,
        "ticket",
        vec![
            ("code", text("c1")),
            ("status", text("open")),
            ("a", Value::Ref(d)),
        ],
    );
    let world = accepted(
        &world,
        vec![
            rename("dossier", "title", "heading"),
            query(
                "by_heading",
                "ticket",
                Some(is_text(through("a", "heading"), "T")),
            ),
        ],
    );
    assert_eq!(rows(&world, "by_heading"), vec![t.0]);
}

/// Situation 25: a move of `ticket.status` to the `dossier` it references through `a`.
///
/// Wanted: the query that filtered on `status` reads it through `a` and finds the same ticket; the
/// value is on the dossier and gone from the ticket. A ticket that holds a value but points at no
/// dossier, and two tickets that point at one dossier with different values, are refused: the move
/// would lose a value.
#[test]
fn situation_25_move_a_field_along_a_reference() {
    let (world, d) = create(&catalogue(), "dossier", vec![("title", text("T"))]);
    let (with_ticket, t) = create(
        &world,
        "ticket",
        vec![
            ("code", text("c1")),
            ("status", text("open")),
            ("a", Value::Ref(d)),
        ],
    );
    let before = rows(&with_ticket, "open_tickets");
    assert_eq!(before, vec![t.0]);
    let move_status = SchemaChange::MoveField {
        object: object("ticket"),
        field: field("status"),
        via: field("a"),
        to_field: field("ticket_state"),
    };
    assert_eq!(
        dependencies::classify(&move_status, &schema_of(&with_ticket)),
        ChangeClass::Adaptable
    );
    let moved = accepted(&with_ticket, vec![move_status.clone()]);
    assert_eq!(rows(&moved, "open_tickets"), before);
    assert_eq!(
        stored(&moved, "dossier", d)
            .fields
            .get(&field("ticket_state")),
        Some(&text("open"))
    );
    assert!(!stored(&moved, "ticket", t)
        .fields
        .contains_key(&field("status")));
    assert_eq!(
        schema_of(&moved).queries[&QueryId::new("open_tickets").unwrap()].filter,
        Some(is_text(through("a", "ticket_state"), "open"))
    );

    let (orphan, _) = create(
        &with_ticket,
        "ticket",
        vec![("code", text("c2")), ("status", text("open"))],
    );
    let err = refused(&orphan, vec![move_status.clone()]);
    assert_eq!(err.kind, ErrorKind::MigrationRejected);
    let (conflict, _) = create(
        &with_ticket,
        "ticket",
        vec![
            ("code", text("c3")),
            ("status", text("closed")),
            ("a", Value::Ref(d)),
        ],
    );
    let err = refused(&conflict, vec![move_status]);
    assert_eq!(err.kind, ErrorKind::MigrationRejected);
}

/// Situation 26: ABA with a rename. A query reads `dossier.title` through `a`; the change renames
/// `title` to `heading` and defines a new `title` in the same change.
///
/// Wanted: accepted, and the query stays bound to the field it was written against, now named
/// `heading`; it does not bind to the new `title`.
///
/// Verifies: MIG-604
#[test]
fn situation_26_aba_with_a_rename_keeps_the_old_binding() {
    let world = accepted(
        &catalogue(),
        vec![query(
            "tickets_by_title",
            "ticket",
            Some(is_text(through("a", "title"), "T")),
        )],
    );
    let world = accepted(
        &world,
        vec![
            rename("dossier", "title", "heading"),
            str_field("dossier", "title"),
        ],
    );
    assert_eq!(
        schema_of(&world).queries[&QueryId::new("tickets_by_title").unwrap()].filter,
        Some(is_text(through("a", "heading"), "T"))
    );
}

/// Situation 27: a diamond. Two queries on `ticket` read `dossier.title` through the reference
/// `a`, and a view uses both and includes `a`. Renaming `a`.
///
/// Wanted: each dependent is rewritten once, and the view after the two queries it uses.
///
/// Verifies: MIG-608
#[test]
fn situation_27_diamond_rewrite_order() {
    let section = |query: &str| ViewSection {
        marks: None,
        joins: None,
        query: QueryId::new(query).unwrap(),
        args: BTreeMap::new(),
        // `include` names reference fields only, so the diamond meets at the reference `a`.
        include: vec![field("a")],
        markdown: vec![],
        single: false,
        aggregate: None,
    };
    let world = accepted(
        &catalogue(),
        vec![
            query(
                "first",
                "ticket",
                Some(is_text(through("a", "title"), "T1")),
            ),
            query(
                "second",
                "ticket",
                Some(is_text(through("a", "title"), "T2")),
            ),
            SchemaChange::DefineView(ViewDef {
                id: ViewId::new("board").unwrap(),
                route: "/board".into(),
                title: "Board".into(),
                sections: BTreeMap::from([
                    ("one".to_string(), section("first")),
                    ("two".to_string(), section("second")),
                ]),
            }),
        ],
    );
    let base = schema_of(&world);
    let change = vec![rename("ticket", "a", "holder")];
    let (after, outcome) =
        change_with(&world, change.clone(), None, ExistingData::Reject).expect("absorbed");
    let assessments = assess_changes(&base, &change, &schema_of(&after));
    assert_eq!(
        assessments[0].rewritten,
        vec![
            query_id("first"),
            query_id("second"),
            DependentId::View(ViewId::new("board").unwrap()),
        ]
    );
    let order: Vec<String> = evidence_of(&outcome, "schema:rewrite");
    assert_eq!(order.len(), 3, "{order:?}");
    assert!(order[2].contains("view board"), "{order:?}");
    let renamed = &schema_of(&after).views[&ViewId::new("board").unwrap()];
    assert_eq!(renamed.sections["one"].include, vec![field("holder")]);
}

/// A ticket grade as the retype situations use it: `A` is stored as `1`, and `1` or `one` reads
/// back as `A`.
fn grade_retype(window: Option<i64>) -> SchemaChange {
    SchemaChange::RetypeField {
        object: object("ticket"),
        field: field("grade"),
        ty: FieldType::Str,
        encode: Expr::If(
            Box::new(is_text(here("value"), "A")),
            Box::new(lit("1")),
            Box::new(here("value")),
        ),
        decode: Expr::If(
            Box::new(Expr::Or(vec![
                is_text(here("value"), "1"),
                is_text(here("value"), "one"),
            ])),
            Box::new(lit("A")),
            Box::new(here("value")),
        ),
        deprecation_ends: window.map(Timestamp),
    }
}

/// The catalogue with a `grade` on tickets, the query `top_grade` over it, and one graded ticket.
fn graded() -> (World, RecordId) {
    let world = accepted(
        &catalogue(),
        vec![
            str_field("ticket", "grade"),
            query("top_grade", "ticket", Some(is_text(here("grade"), "A"))),
        ],
    );
    create(
        &world,
        "ticket",
        vec![("code", text("c1")), ("grade", text("A"))],
    )
}

fn top_grade_filter(world: &World) -> Expr {
    schema_of(world).queries[&QueryId::new("top_grade").unwrap()]
        .filter
        .clone()
        .expect("a filter")
}

/// Situation 28: a retype that reaches a pinned dependent.
///
/// Wanted: with no pin the retype is breaking and refused naming the query. With the query pinned
/// it is accepted, the stored value is re-encoded and the structural version rises; the pinned
/// query reads the stored record through the projection and evaluates as it did before, while the
/// stored record read as it is would not satisfy it.
///
/// Verifies: MIG-596
#[test]
fn situation_28_a_pinned_dependent_survives_a_retype_through_a_projection() {
    let (world, t) = graded();
    let err = refused(&world, vec![grade_retype(None)]);
    assert_eq!(err.kind, ErrorKind::InvalidSchema);
    assert!(err.message.contains("top_grade"), "{}", err.message);

    let mut book = PinBook::default();
    book.pin(&query_id("top_grade"), &schema_of(&world));
    let (after, next, outcome) =
        change_pinned(&world, vec![grade_retype(None)], &book).expect("a pinned dependent is held");
    assert!(!evidence_of(&outcome, "schema:pin").is_empty());
    let record = stored(&after, "ticket", t);
    assert_eq!(record.fields.get(&field("grade")), Some(&text("1")));
    let cell = (object("ticket"), field("grade"));
    assert_eq!(next.version(&cell), 2);
    let filter = top_grade_filter(&after);
    assert!(!holds_on(&filter, &record));
    let projected =
        evolution::project_record(&next, &query_id("top_grade"), &record, Timestamp(NOW))
            .expect("the projection reads");
    assert!(holds_on(&filter, &projected));
}

/// Situation 29: the structural version moves only on a breaking change.
///
/// Wanted: an added field and a rename leave the book as it was; a retype nobody reads is
/// breaking, accepted, and raises the version of its cell by one.
///
/// Verifies: MIG-594
#[test]
fn situation_29_a_structural_version_moves_only_on_a_breaking_change() {
    let world = accepted(&catalogue(), vec![str_field("ticket", "grade")]);
    let book = PinBook::default();
    let (world, book, _) = change_pinned(
        &world,
        vec![
            str_field("ticket", "extra"),
            rename("ticket", "code", "ref"),
        ],
        &book,
    )
    .expect("compatible and adaptable changes");
    assert_eq!(book, PinBook::default());
    let (_, book, _) =
        change_pinned(&world, vec![grade_retype(None)], &book).expect("nobody reads grade");
    assert_eq!(book.version(&(object("ticket"), field("grade"))), 2);
    assert_eq!(book.version(&(object("ticket"), field("extra"))), 1);
}

/// Situation 30: a retype whose decode does not undo its encode has no projection.
///
/// Wanted: refused even though the dependent is pinned, saying so.
///
/// Verifies: MIG-597
#[test]
fn situation_30_a_retype_without_a_left_inverse_is_not_projected() {
    let (world, _) = graded();
    let mut book = PinBook::default();
    book.pin(&query_id("top_grade"), &schema_of(&world));
    let SchemaChange::RetypeField {
        object,
        field,
        ty,
        encode,
        ..
    } = grade_retype(None)
    else {
        unreachable!()
    };
    let identity = SchemaChange::RetypeField {
        object,
        field,
        ty,
        encode,
        decode: here("value"),
        deprecation_ends: None,
    };
    let err = change_pinned(&world, vec![identity], &book).expect_err("no left inverse");
    assert_eq!(err.kind, ErrorKind::InvalidSchema);
    assert!(err.message.contains("MIG-597"), "{}", err.message);
}

/// Situation 31: a removal across a pin waits for it.
///
/// Wanted: removing `ticket.grade` while `top_grade` is pinned to it is refused, saying that it
/// waits for the pin; once the query is defined again without it, the pin has ended and the
/// removal is accepted.
///
/// Verifies: MIG-599
#[test]
fn situation_31_a_removal_waits_for_the_pins_on_it() {
    let (world, _) = graded();
    let mut book = PinBook::default();
    book.pin(&query_id("top_grade"), &schema_of(&world));
    let err = change_pinned(&world, vec![remove_field("ticket", "grade")], &book)
        .expect_err("the pin holds the removal");
    assert!(err.message.contains("waits"), "{}", err.message);

    let (world, book, _) = change_pinned(
        &world,
        vec![query(
            "top_grade",
            "ticket",
            Some(is_text(here("code"), "c1")),
        )],
        &book,
    )
    .expect("the dependent migrates");
    assert!(!book.is_pinned(&query_id("top_grade")));
    change_pinned(&world, vec![remove_field("ticket", "grade")], &book)
        .expect("no pin remains, so the old shape goes");
}

/// Situation 32: a switch back to the earlier version after a migration.
///
/// Wanted: refused when a value written since cannot be read through the earlier version without
/// loss (`one` reads back as `A`, which is stored as `1`), accepted when every stored value
/// survives the round trip.
///
/// Verifies: MIG-607
#[test]
fn situation_32_a_switch_back_is_refused_when_it_would_lose_data() {
    let (world, t) = graded();
    let mut book = PinBook::default();
    book.pin(&query_id("top_grade"), &schema_of(&world));
    let (after, mut book, _) = change_pinned(&world, vec![grade_retype(None)], &book).unwrap();
    let cell = (object("ticket"), field("grade"));
    let dependent = query_id("top_grade");
    let kept = stored(&after, "ticket", t).fields[&field("grade")].clone();
    evolution::switch_back(&mut book, &dependent, &cell, 1, [&kept], Timestamp(NOW))
        .expect("every stored value survives");
    let lossy = text("one");
    let err = evolution::switch_back(
        &mut book,
        &dependent,
        &cell,
        1,
        [&kept, &lossy],
        Timestamp(NOW),
    )
    .expect_err("the value written since does not survive");
    assert!(err.contains("MIG-607"), "{err}");
}

/// Situation 33: a deprecation window that closes.
///
/// Wanted: the dependent reads through the projection inside the window; after it the pin is
/// reported and the read is refused.
///
/// Verifies: MIG-598
#[test]
fn situation_33_a_pin_held_past_its_window_is_reported_and_refused() {
    let (world, t) = graded();
    let mut book = PinBook::default();
    book.pin(&query_id("top_grade"), &schema_of(&world));
    let (after, book, _) =
        change_pinned(&world, vec![grade_retype(Some(NOW + 10))], &book).unwrap();
    let record = stored(&after, "ticket", t);
    let dependent = query_id("top_grade");
    assert!(evolution::expired_pins(&book, Timestamp(NOW)).is_empty());
    evolution::project_record(&book, &dependent, &record, Timestamp(NOW)).expect("inside");
    assert_eq!(
        evolution::expired_pins(&book, Timestamp(NOW + 10)),
        vec![(dependent.clone(), (object("ticket"), field("grade")))]
    );
    let err = evolution::project_record(&book, &dependent, &record, Timestamp(NOW + 10))
        .expect_err("after the window");
    assert!(err.contains("MIG-598"), "{err}");
}

/// Situation 34: a stale change whose footprint is disjoint from what was committed since its base.
///
/// Wanted: it commutes. Decided on the current schema it has the class it had at its base, and
/// the schema it leaves is the one the other order gives.
///
/// Verifies: MIG-602
#[test]
fn situation_34_a_stale_disjoint_change_is_decided_again_with_the_same_result() {
    let base = catalogue();
    let committed = vec![str_field("dossier", "extra")];
    let incoming = vec![str_field("person", "nickname")];
    let current = accepted(&base, committed.clone());
    let Rebase::Commutes { assessments } = evolution::rebase(
        &schema_of(&base),
        &committed,
        &schema_of(&current),
        &incoming,
    ) else {
        panic!("disjoint changes commute");
    };
    assert_eq!(assessments[0].class, ChangeClass::Compatible);
    let one_order = accepted(&current, incoming.clone());
    let other_order = accepted(&accepted(&base, incoming), committed);
    assert_eq!(schema_of(&one_order), schema_of(&other_order));
}

/// Situation 35: a stale change whose footprint overlaps what was committed since its base.
///
/// Wanted: it is classified again on the current schema. A rename of `ticket.status` made before a
/// query over `status` was committed now rewrites that query too; a query over `dossier.title`
/// made before `title` was renamed is refused on the current schema, naming the query.
///
/// Verifies: MIG-603
#[test]
fn situation_35_a_stale_overlapping_change_is_classified_again() {
    let base = catalogue();
    let by_status = vec![query(
        "by_status",
        "ticket",
        Some(is_text(here("status"), "closed")),
    )];
    let current = accepted(&base, by_status.clone());
    let incoming = vec![rename("ticket", "status", "state")];
    let Rebase::Reclassified {
        at_base,
        now,
        refused,
    } = evolution::rebase(
        &schema_of(&base),
        &by_status,
        &schema_of(&current),
        &incoming,
    )
    else {
        panic!("overlapping changes are classified again");
    };
    assert_eq!(refused, None);
    assert_eq!(at_base[0].rewritten, vec![query_id("open_tickets")]);
    assert_eq!(
        now[0].rewritten,
        vec![query_id("by_status"), query_id("open_tickets")]
    );

    let renamed = vec![rename("dossier", "title", "heading")];
    let current = accepted(&base, renamed.clone());
    let over_title = vec![query(
        "by_title",
        "ticket",
        Some(is_text(through("a", "title"), "T")),
    )];
    let Rebase::Reclassified { refused, .. } = evolution::rebase(
        &schema_of(&base),
        &renamed,
        &schema_of(&current),
        &over_title,
    ) else {
        panic!("a read of the renamed cell overlaps the rename");
    };
    let reason = refused.expect("refused on the current schema");
    assert!(reason.contains("by_title"), "{reason}");
}
