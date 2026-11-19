//! The generated functions (WG-4934) against the kernel decisions they state.
//!
//! `crates/spec-compiler-functions-fixture` is generated from `tests/fixtures/functions/spec.jsonc`
//! (the golden test is `functions_golden_output_matches_its_fixture_crate` in `tests/golden.rs`).
//! Its `authorize` and `check_version` are run here on the same inputs as the kernel's
//! `rules::authorize` and the version check of `transition` (through `World::step`, since
//! `check_version` itself is private), over the notes world of the kernel's testkit:
//!
//! - authorize: every principal of tenant alpha and the system, every action, five resources and
//!   five subjects (none, a stored note, a stored published note, a stored note of another
//!   writer, a new note); the decision and the rule and outcome of each check must agree. The
//!   kernel's check details are not stated by the function (see WG-4933), so they are not
//!   compared.
//! - check_version: a note at version 2 updated with no expected version and with expected
//!   versions 0 to 3 and 7; the kernel rejects with `VersionConflict` exactly when the generated
//!   function fails, with the same message.

use std::collections::BTreeMap;

use strata_kernel::expr as kexpr;
use strata_kernel::rules::{self, Subject as KSubject};
use strata_kernel::testkit::{
    ctx, field, new_note, notes_world, object, principals, tenant_id, text, user,
};
use strata_kernel::{
    Action as KAction, ErrorKind, Intent, Principal as KPrincipal, PrincipalId, Record as KRecord,
    RecordId, Resource as KResource, TenantState, Timestamp, Value as KValue, World,
};
use strata_spec_compiler_functions_fixture::functions;
use strata_spec_compiler_functions_fixture::rule_language;
use strata_spec_compiler_functions_fixture::types::{
    Action, DomainError, PolicyDef, Principal, Resource, Subject, Tenant, Unit, Value,
};

fn value(v: &KValue) -> Value {
    match v {
        KValue::Null => Value::Null,
        KValue::Bool(b) => Value::Bool(*b),
        KValue::Int(i) => Value::Int(*i),
        KValue::Str(s) => Value::Str(s.clone()),
        KValue::Time(t) => Value::Time(*t),
        KValue::Ref(id) => Value::Ref(id.0),
        KValue::List(items) => Value::List(items.iter().map(value).collect()),
    }
}

fn action(a: KAction) -> Action {
    match a {
        KAction::Read => Action::Read,
        KAction::Create => Action::Create,
        KAction::Update => Action::Update,
        KAction::Delete => Action::Delete,
        KAction::Manage => Action::Manage,
    }
}

fn resource(r: &KResource) -> Resource {
    match r {
        KResource::Object(o) => Resource::Object(o.as_str().to_string()),
        KResource::AnyObject => Resource::AnyObject,
        KResource::Schema => Resource::Schema,
        KResource::Members => Resource::Members,
    }
}

fn principal(p: &KPrincipal) -> Principal {
    match p {
        KPrincipal::System => Principal::System,
        KPrincipal::Anonymous => Principal::Anonymous,
        KPrincipal::User(id) => Principal::User(id.as_str().to_string()),
    }
}

/// A tenant as the functions read it: its members' roles and its schema's policies, with each
/// condition printed by the kernel and read back by the generated parser.
fn tenant(t: &TenantState) -> Tenant {
    let members = t
        .members
        .iter()
        .map(|(who, roles)| {
            (
                who.as_str().to_string(),
                roles.iter().map(|r| r.as_str().to_string()).collect(),
            )
        })
        .collect();
    let policies = t
        .schema
        .policies
        .iter()
        .map(|(id, p)| {
            let condition = p.condition.as_ref().map(|c| {
                rule_language::parse(&c.to_string())
                    .unwrap_or_else(|e| panic!("the kernel's {c} does not parse: {e}"))
            });
            (
                id.as_str().to_string(),
                PolicyDef {
                    id: p.id.as_str().to_string(),
                    role: p.role.as_str().to_string(),
                    resource: resource(&p.resource),
                    actions: p.actions.iter().map(|a| action(*a)).collect(),
                    condition,
                },
            )
        })
        .collect();
    Tenant { members, policies }
}

/// A record as a record scope reads it: its fields and its system attributes.
fn record(r: &KRecord) -> BTreeMap<String, Value> {
    let mut out: BTreeMap<String, Value> = r
        .fields
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), value(v)))
        .collect();
    for name in [
        "$id",
        "$version",
        "$created_by",
        "$created_at",
        "$updated_at",
    ] {
        let v = kexpr::attribute(Some(r), name).expect("a system attribute");
        out.insert(name.to_string(), value(&v));
    }
    out
}

fn world_with_a_note() -> (World, KRecord) {
    let world = notes_world()
        .step(
            &new_note("First", "first"),
            &ctx("alpha", user("alpha:writer1"), 5_000),
        )
        .expect("writer1 creates a note")
        .0;
    let note = world.tenants[&tenant_id("alpha")]
        .record(&object("note"), RecordId(1))
        .expect("note 1")
        .as_ref()
        .clone();
    (world, note)
}

#[test]
fn authorize_agrees_with_the_kernel() {
    let (world, note) = world_with_a_note();
    let alpha = world.tenants[&tenant_id("alpha")].clone();
    let generated_tenant = tenant(&alpha);
    let mut published = note.clone();
    published.fields.insert(field("status"), text("published"));
    let mut others = note.clone();
    others.created_by = Some(PrincipalId::new("alpha:writer2").unwrap());
    let records = [note.clone(), published, others];
    let mut subjects: Vec<(Option<&KRecord>, Option<&KRecord>)> = vec![(None, None)];
    subjects.extend(records.iter().map(|r| (Some(r), Some(r))));
    subjects.push((None, Some(&records[0])));
    let mut who = principals("alpha");
    who.push(KPrincipal::System);
    let actions = [
        KAction::Read,
        KAction::Create,
        KAction::Update,
        KAction::Delete,
        KAction::Manage,
    ];
    let resources = [
        KResource::Object(object("note")),
        KResource::Object(object("person")),
        KResource::AnyObject,
        KResource::Schema,
        KResource::Members,
    ];
    let mut failures = Vec::new();
    let mut compared = 0;
    for p in &who {
        for a in actions {
            for r in &resources {
                for (old, new) in &subjects {
                    let (allowed, checks) = rules::authorize(
                        &alpha,
                        &alpha.schema,
                        p,
                        Timestamp(5_000),
                        a,
                        r,
                        KSubject {
                            old: *old,
                            new: *new,
                        },
                        None,
                    );
                    let kernel: (bool, Vec<(String, bool)>) = (
                        allowed,
                        checks.into_iter().map(|c| (c.rule, c.passed)).collect(),
                    );
                    let subject = Subject {
                        old: old.map(record),
                        new: new.map(record),
                    };
                    let decision = functions::authorize(
                        &generated_tenant,
                        &principal(p),
                        5_000,
                        &action(a),
                        &resource(r),
                        &subject,
                    );
                    let generated: (bool, Vec<(String, bool)>) = (
                        decision.allowed,
                        decision
                            .checks
                            .into_iter()
                            .map(|c| (c.rule, c.passed))
                            .collect(),
                    );
                    if kernel != generated {
                        failures.push(format!(
                            "{p:?} {a:?} {r:?} (old: {}, new: {}):\n  kernel    {kernel:?}\n  generated {generated:?}",
                            old.is_some(),
                            new.is_some()
                        ));
                    }
                    compared += 1;
                }
            }
        }
    }
    assert!(compared > 500, "{compared}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn may_attempt_agrees_with_the_kernel() {
    let (world, _) = world_with_a_note();
    let alpha = world.tenants[&tenant_id("alpha")].clone();
    let generated_tenant = tenant(&alpha);
    let mut who = principals("alpha");
    who.push(KPrincipal::System);
    for p in &who {
        for a in [
            KAction::Read,
            KAction::Create,
            KAction::Update,
            KAction::Delete,
            KAction::Manage,
        ] {
            for r in [
                KResource::Object(object("note")),
                KResource::Object(object("person")),
                KResource::AnyObject,
                KResource::Schema,
                KResource::Members,
            ] {
                assert_eq!(
                    rules::may_attempt(&alpha, p, a, &r),
                    functions::may_attempt(
                        &generated_tenant,
                        &principal(p),
                        &action(a),
                        &resource(&r)
                    ),
                    "{p:?} {a:?} {r:?}"
                );
            }
        }
    }
}

#[test]
fn check_version_agrees_with_the_kernel() {
    let (world, _) = world_with_a_note();
    let patch = |expected: Option<u64>| Intent::UpdateRecord {
        object: object("note"),
        id: RecordId(1),
        fields: BTreeMap::from([(field("body"), text("one"))]),
        expected_version: expected,
    };
    let world = world
        .step(&patch(None), &ctx("alpha", user("alpha:writer1"), 5_001))
        .expect("writer1 edits its note")
        .0;
    let version = world.tenants[&tenant_id("alpha")]
        .record(&object("note"), RecordId(1))
        .expect("note 1")
        .version;
    assert_eq!(version, 2);
    for expected in [None, Some(0), Some(1), Some(2), Some(3), Some(7)] {
        let kernel = world.step(
            &patch(expected),
            &ctx("alpha", user("alpha:writer1"), 5_002),
        );
        let generated = functions::check_version(version, expected);
        match (&kernel, &generated) {
            (Err(e), Err(DomainError::VersionConflict { message })) => {
                assert_eq!(e.kind, ErrorKind::VersionConflict, "{expected:?}: {e:?}");
                assert_eq!(&e.message, message, "{expected:?}");
            }
            (Ok(_), Ok(Unit::Unit)) => {}
            (Err(e), Ok(_)) => assert_ne!(
                e.kind,
                ErrorKind::VersionConflict,
                "{expected:?}: the kernel saw a conflict the function did not"
            ),
            (Ok(_), Err(e)) => {
                panic!("{expected:?}: the function failed with {e:?}, the kernel accepted")
            }
        }
    }
    // next_version passes the conflict on, and computes the next version otherwise.
    assert_eq!(functions::next_version(version, None), Ok(3));
    assert!(matches!(
        functions::next_version(version, Some(1)),
        Err(functions::Failure::Error(
            DomainError::VersionConflict { .. }
        ))
    ));
    assert_eq!(
        functions::next_version(u64::MAX, None),
        Err(functions::Failure::Fault(functions::Fault { op: "+" }))
    );
}
