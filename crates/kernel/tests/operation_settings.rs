//! A write-setting step of a tenant operation (WG-5606). The settings route's own rule decides each
//! write, each write makes one setting-changed event, and the writes of an invocation commit with
//! its other steps or not at all.

use std::collections::BTreeSet;
use strata_kernel::testkit::*;
use strata_kernel::*;

fn op_id(s: &str) -> TenantOperationId {
    TenantOperationId::new(s).expect("operation id")
}

fn sid(s: &str) -> SettingId {
    SettingId::new(s).expect("setting id")
}

fn param(s: &str) -> Expr {
    Expr::Param(s.to_string())
}

fn int(n: i64) -> Expr {
    Expr::Lit(Value::Int(n))
}

fn input(name: &str, ty: FieldType) -> TenantOperationInput {
    TenantOperationInput {
        name: name.into(),
        ty,
        required: true,
        label: None,
    }
}

fn write(id: &str, setting: &str, level: OperationSettingLevel, value: Expr) -> TenantOperationStep {
    TenantOperationStep::WriteSetting {
        id: id.into(),
        setting: sid(setting),
        level,
        value,
    }
}

fn operation(
    id: &str,
    inputs: Vec<TenantOperationInput>,
    steps: Vec<TenantOperationStep>,
) -> TenantOperationDef {
    TenantOperationDef {
        id: op_id(id),
        label: id.into(),
        inputs,
        invokers: TenantOperationInvokers {
            signed_in: true,
            roles: BTreeSet::new(),
        },
        subject: None,
        conditions: Vec::new(),
        steps,
    }
}

/// A setting a person holds for themselves: one of two channels.
fn home_channel() -> SettingDef {
    SettingDef {
        id: sid("home_channel"),
        label: "Home channel".into(),
        r#type: SettingType::new("string").expect("setting type"),
        default: Value::Str("general".into()),
        levels: BTreeSet::from([SettingLevel::Person]),
        locked_by: None,
        area: None,
        description: None,
        allowed: Some(SettingAllowed::Closed {
            values: vec![Value::Str("general".into()), Value::Str("random".into())],
        }),
    }
}

/// A setting the tenant holds, a whole number of hours from 0 to 23.
fn digest_hour() -> SettingDef {
    SettingDef {
        id: sid("digest_hour"),
        label: "Digest hour".into(),
        r#type: SettingType::new("i64").expect("setting type"),
        default: Value::Int(9),
        levels: BTreeSet::from([SettingLevel::Tenant]),
        locked_by: None,
        area: None,
        description: None,
        allowed: Some(SettingAllowed::Range { min: 0, max: 23 }),
    }
}

fn step(
    world: &World,
    who: &str,
    now: i64,
    intent: Intent,
) -> Result<(World, Transition), DomainError> {
    world.step(&intent, &ctx("alpha", user(&format!("alpha:{who}")), now))
}

fn invoke(op: &str, inputs: &[(&str, Value)], key: &str) -> Intent {
    Intent::InvokeOperation {
        operation: op_id(op),
        inputs: inputs
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone()))
            .collect(),
        idempotency_key: key.to_string(),
    }
}

fn tenant(world: &World) -> &TenantState {
    &world.tenants[&tenant_id("alpha")]
}

fn value_of(world: &World, who: &str, setting: &str) -> Value {
    settings::resolve(tenant(world), &user(&format!("alpha:{who}")), &sid(setting))
        .expect("the setting is defined")
        .value
}

/// The subjects of the setting-changed events of an invocation, one per write.
fn setting_events(transition: &Transition) -> Vec<String> {
    transition
        .events
        .iter()
        .filter_map(|event| match &event.body {
            EventBody::SettingChanged { subject, .. } => Some(subject.clone()),
            _ => None,
        })
        .collect()
}

/// The notes world of alpha with the two settings and four operations that write them.
fn setting_world() -> World {
    let changes = vec![
        SchemaChange::DefineSetting(home_channel()),
        SchemaChange::DefineSetting(digest_hour()),
        SchemaChange::DefineOperation(operation(
            "choose_channel",
            vec![input("channel", FieldType::Str)],
            vec![write(
                "home",
                "home_channel",
                OperationSettingLevel::Person,
                param("channel"),
            )],
        )),
        SchemaChange::DefineOperation(operation(
            "set_digest",
            vec![input("hour", FieldType::Int)],
            vec![write(
                "digest",
                "digest_hour",
                OperationSettingLevel::Tenant,
                param("hour"),
            )],
        )),
        SchemaChange::DefineOperation(operation(
            "choose_twice",
            vec![input("first", FieldType::Str), input("second", FieldType::Str)],
            vec![
                write("a", "home_channel", OperationSettingLevel::Person, param("first")),
                write("b", "home_channel", OperationSettingLevel::Person, param("second")),
            ],
        )),
        SchemaChange::DefineOperation(operation(
            "set_and_choose",
            vec![input("channel", FieldType::Str)],
            vec![
                write(
                    "home",
                    "home_channel",
                    OperationSettingLevel::Person,
                    param("channel"),
                ),
                write("digest", "digest_hour", OperationSettingLevel::Tenant, int(5)),
            ],
        )),
    ];
    let intent = Intent::ChangeSchema {
        changes,
        expected_version: None,
        existing: ExistingData::Reject,
    };
    step(&notes_world(), "owner", 5_000, intent)
        .expect("the schema is accepted")
        .0
}

/// A person's write takes the invoker's own value, not another person's, and makes one event that
/// names the invoker.
#[test]
fn a_person_step_writes_only_the_invokers_own_value_and_makes_one_event() {
    let world = setting_world();
    let (after, transition) = step(
        &world,
        "writer1",
        6_000,
        invoke("choose_channel", &[("channel", Value::Str("random".into()))], "k-1"),
    )
    .expect("accepted");
    assert_eq!(
        value_of(&after, "writer1", "home_channel"),
        Value::Str("random".into())
    );
    assert_eq!(
        value_of(&after, "editor", "home_channel"),
        Value::Str("general".into()),
        "another person's value is not written"
    );
    assert_eq!(setting_events(&transition), vec!["alpha:writer1".to_string()]);
}

/// A value outside the closed list is refused with the rule the settings route applies.
#[test]
fn a_value_outside_the_allowed_list_is_refused_as_the_settings_route_refuses_it() {
    let world = setting_world();
    let err = step(
        &world,
        "writer1",
        6_000,
        invoke("choose_channel", &[("channel", Value::Str("blue".into()))], "k-1"),
    )
    .expect_err("refused");
    assert_eq!(err.kind, ErrorKind::InvalidIntent);
    assert!(
        err.evidence
            .iter()
            .any(|check| !check.passed && check.rule == "setting:value_is_allowed"),
        "{:?}",
        err.evidence
    );
    assert_eq!(
        value_of(&world, "writer1", "home_channel"),
        Value::Str("general".into()),
        "the refused write leaves the setting as it was"
    );
}

/// A range is enforced for a tenant write as the route enforces it.
#[test]
fn a_range_is_enforced_for_a_tenant_write_as_the_route_enforces_it() {
    let world = setting_world();
    let err = step(
        &world,
        "owner",
        6_000,
        invoke("set_digest", &[("hour", Value::Int(24))], "k-1"),
    )
    .expect_err("refused");
    assert_eq!(err.kind, ErrorKind::LimitExceeded);
    let (after, transition) = step(
        &world,
        "owner",
        6_000,
        invoke("set_digest", &[("hour", Value::Int(7))], "k-2"),
    )
    .expect("accepted");
    assert_eq!(value_of(&after, "writer1", "digest_hour"), Value::Int(7));
    assert_eq!(setting_events(&transition), vec!["alpha".to_string()]);
}

/// A tenant-level write needs the capability setting-write-tenant of the invoker, which the owner
/// holds and a member does not.
#[test]
fn a_tenant_step_needs_setting_write_tenant_of_its_invoker() {
    let world = setting_world();
    let err = step(
        &world,
        "writer1",
        6_000,
        invoke("set_digest", &[("hour", Value::Int(7))], "k-1"),
    )
    .expect_err("refused");
    assert_eq!(err.kind, ErrorKind::Unauthorized);
    assert!(
        err.evidence.iter().any(|check| !check.passed
            && check.rule == "permission:capability:setting-write-tenant"),
        "{:?}",
        err.evidence
    );
    step(
        &world,
        "owner",
        6_000,
        invoke("set_digest", &[("hour", Value::Int(7))], "k-2"),
    )
    .expect("the owner holds the capability");
}

/// Two writes of one setting in one invocation make two events, and the second reads the revision
/// the first left.
#[test]
fn each_write_of_an_invocation_makes_one_event_and_the_second_reads_the_first() {
    let world = setting_world();
    let (after, transition) = step(
        &world,
        "writer1",
        6_000,
        invoke(
            "choose_twice",
            &[
                ("first", Value::Str("random".into())),
                ("second", Value::Str("general".into())),
            ],
            "k-1",
        ),
    )
    .expect("accepted");
    assert_eq!(
        value_of(&after, "writer1", "home_channel"),
        Value::Str("general".into())
    );
    assert_eq!(
        settings::tenant_revision(tenant(&after), &sid("home_channel")),
        2,
        "the second write names the revision the first one left"
    );
    assert_eq!(
        setting_events(&transition),
        vec!["alpha:writer1".to_string(), "alpha:writer1".to_string()],
        "one event per write"
    );
}

/// The writes of an invocation commit with its other steps or not at all: a write the invoker may
/// not make refuses the whole invocation, and the same invocation by the owner commits both.
#[test]
fn the_writes_of_an_invocation_commit_together_or_not_at_all() {
    let world = setting_world();
    let err = step(
        &world,
        "writer1",
        6_000,
        invoke(
            "set_and_choose",
            &[("channel", Value::Str("random".into()))],
            "k-1",
        ),
    )
    .expect_err("refused");
    assert_eq!(err.kind, ErrorKind::Unauthorized);
    assert!(
        err.evidence
            .iter()
            .any(|check| !check.passed && check.rule.contains("step:digest")),
        "{:?}",
        err.evidence
    );
    let (after, transition) = step(
        &world,
        "owner",
        6_000,
        invoke(
            "set_and_choose",
            &[("channel", Value::Str("random".into()))],
            "k-2",
        ),
    )
    .expect("the owner's invocation commits both writes");
    assert_eq!(
        value_of(&after, "writer1", "home_channel"),
        Value::Str("random".into())
    );
    assert_eq!(value_of(&after, "writer1", "digest_hour"), Value::Int(5));
    assert_eq!(setting_events(&transition).len(), 2);
}

/// An operation that writes a setting the tenant does not define, or at a level the definition does
/// not allow, is refused when it is defined.
#[test]
fn an_operation_that_writes_an_undefined_setting_or_a_closed_level_is_refused_when_defined() {
    let world = setting_world();
    for (name, setting, level) in [
        ("write_ghost", "ghost_setting", OperationSettingLevel::Person),
        ("write_digest_mine", "digest_hour", OperationSettingLevel::Person),
    ] {
        let op = operation(
            name,
            vec![input("v", FieldType::Int)],
            vec![write("w", setting, level, param("v"))],
        );
        let intent = Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineOperation(op)],
            expected_version: None,
            existing: ExistingData::Reject,
        };
        assert!(
            step(&world, "owner", 6_100, intent).is_err(),
            "{name} is defined"
        );
    }
}
