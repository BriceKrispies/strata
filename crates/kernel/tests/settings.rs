//! Layered settings and the capabilities of products (D-5040 to D-5049, D-6590, D-6701): what the
//! kernel holds, what it refuses when a definition is made or an override is written, how a read
//! resolves, who may write each level and what the events say. One rule per test.

use std::collections::BTreeSet;
use strata_kernel::settings;
use strata_kernel::testkit::*;
use strata_kernel::*;

// ---------------------------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------------------------

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

fn change(changes: Vec<SchemaChange>) -> Intent {
    Intent::ChangeSchema {
        changes,
        expected_version: None,
        existing: ExistingData::Reject,
    }
}

fn sid(name: &str) -> SettingId {
    SettingId::new(name).expect("setting id")
}

fn team_id(name: &str) -> TeamId {
    TeamId::new(name).expect("team id")
}

fn text_value(s: &str) -> Value {
    Value::Str(s.to_string())
}

/// A setting that allows the tenant, team and person levels.
fn layered(id: &str, ty: &str, default: Value) -> SettingDef {
    SettingDef {
        id: sid(id),
        label: id.to_string(),
        r#type: SettingType::new(ty).expect("setting type"),
        default,
        levels: BTreeSet::from([
            SettingLevel::Tenant,
            SettingLevel::Team,
            SettingLevel::Person,
        ]),
        locked_by: None,
    }
}

fn page_size() -> SettingDef {
    layered("page_size", "i64", Value::Int(20))
}

fn define(world: &World, def: SettingDef) -> World {
    must(
        world,
        "owner",
        5_000,
        change(vec![SchemaChange::DefineSetting(def)]),
    )
}

/// The notes world with `page_size` defined.
fn with_page_size() -> World {
    define(&notes_world(), page_size())
}

#[allow(clippy::too_many_arguments)]
fn write(
    world: &World,
    who: &str,
    now: i64,
    setting: &str,
    level: SettingLevel,
    team: Option<&str>,
    value: Option<Value>,
    expected: u64,
) -> Result<(World, Transition), DomainError> {
    step(
        world,
        who,
        now,
        Intent::WriteSetting {
            setting: sid(setting),
            level,
            team: team.map(team_id),
            value,
            expected_revision: expected,
        },
    )
}

fn set_tenant(world: &World, setting: &str, value: Value, expected: u64) -> World {
    write(
        world,
        "owner",
        6_000,
        setting,
        SettingLevel::Tenant,
        None,
        Some(value),
        expected,
    )
    .expect("accepted")
    .0
}

fn set_team(world: &World, team: &str, setting: &str, value: Value, expected: u64) -> World {
    write(
        world,
        "owner",
        6_100,
        setting,
        SettingLevel::Team,
        Some(team),
        Some(value),
        expected,
    )
    .expect("accepted")
    .0
}

fn set_person(world: &World, who: &str, setting: &str, value: Value, expected: u64) -> World {
    write(
        world,
        who,
        6_200,
        setting,
        SettingLevel::Person,
        None,
        Some(value),
        expected,
    )
    .expect("accepted")
    .0
}

fn with_team(world: &World, team: &str, members: &[&str]) -> World {
    let mut world = must(
        world,
        "owner",
        4_000,
        Intent::DefineTeam {
            team: team_id(team),
            label: team.to_string(),
        },
    );
    for member in members {
        world = must(
            &world,
            "owner",
            4_001,
            Intent::AddTeamMember {
                team: team_id(team),
                principal: principal_id(&format!("alpha:{member}")),
            },
        );
    }
    world
}

fn tenant(world: &World) -> &TenantState {
    &world.tenants[&tenant_id("alpha")]
}

fn resolve_as(world: &World, who: Option<&str>, setting: &str) -> SettingResolution {
    let principal = match who {
        Some(name) => user(&format!("alpha:{name}")),
        None => Principal::Anonymous,
    };
    settings::resolve(tenant(world), &principal, &sid(setting)).expect("the setting is defined")
}

fn value_for(world: &World, who: &str, setting: &str) -> Value {
    resolve_as(world, Some(who), setting).value
}

fn refusal(result: Result<(World, Transition), DomainError>) -> DomainError {
    result.expect_err("refused")
}

fn schema_of(world: &World) -> &TenantSchema {
    &tenant(world).schema
}

// ---------------------------------------------------------------------------------------------
// Definitions (SET-001 to SET-005).
// ---------------------------------------------------------------------------------------------

// Verifies: SET-001
#[test]
fn a_definition_states_a_name_a_type_a_default_and_the_levels() {
    let world = with_page_size();
    let def = settings::definition(schema_of(&world), &sid("page_size")).expect("defined");
    assert_eq!(def.r#type.as_str(), "i64");
    assert_eq!(def.default, Value::Int(20));
    assert_eq!(def.levels.len(), 3);
    // Nobody has overridden it, so a read answers its default and says so (SET-012).
    let found = resolve_as(&world, Some("writer1"), "page_size");
    assert_eq!(found.value, Value::Int(20));
    assert_eq!(found.source, SettingSource::Default);
    assert_eq!(found.team, None);
}

// Verifies: SET-002
#[test]
fn a_default_that_is_not_a_value_of_the_type_is_refused() {
    let world = notes_world();
    let mut def = page_size();
    def.default = text_value("twenty");
    let refused = refusal(step(
        &world,
        "owner",
        5_000,
        change(vec![SchemaChange::DefineSetting(def)]),
    ));
    assert_eq!(refused.kind, ErrorKind::InvalidSchema);
    assert!(refused.message.contains("default"), "{}", refused.message);
}

#[test]
fn a_definition_with_no_level_a_bad_lock_or_an_unknown_type_is_refused() {
    let world = notes_world();
    let mut none = page_size();
    none.levels = BTreeSet::new();
    let mut team_lock = page_size();
    team_lock.locked_by = Some(SettingLevel::Team);
    let mut float = layered("ratio", "f64", Value::Int(1));
    float.default = Value::Int(1);
    let unknown = layered("colour", "colour", text_value("red"));
    for (def, reason) in [
        (none, "at least one level"),
        (team_lock, "platform or the tenant"),
        (float, "float"),
        (unknown, "not one the kernel holds"),
    ] {
        let refused = refusal(step(
            &world,
            "owner",
            5_000,
            change(vec![SchemaChange::DefineSetting(def)]),
        ));
        assert_eq!(refused.kind, ErrorKind::InvalidSchema);
        assert!(
            refused.message.contains(reason),
            "{reason}: {}",
            refused.message
        );
    }
}

#[test]
fn defining_a_setting_needs_the_capability_setting_define() {
    let world = notes_world();
    let refused = refusal(step(
        &world,
        "editor",
        5_000,
        change(vec![SchemaChange::DefineSetting(page_size())]),
    ));
    assert_eq!(refused.kind, ErrorKind::Unauthorized);
    // The owner holds every capability.
    define(&world, page_size());
}

// Verifies: SET-004
#[test]
fn the_type_of_a_definition_does_not_change_while_an_override_exists() {
    let world = with_page_size();
    let world = set_person(&world, "writer1", "page_size", Value::Int(5), 0);
    let retyped = layered("page_size", "string", text_value("20"));
    let refused = refusal(step(
        &world,
        "owner",
        7_000,
        change(vec![SchemaChange::DefineSetting(retyped.clone())]),
    ));
    assert_eq!(refused.kind, ErrorKind::InvalidSchema);
    assert!(refused.message.contains("SET-004"), "{}", refused.message);

    // Once the override is gone the type may change; the same type with another default always may.
    let held = settings::tenant_revision(tenant(&world), &sid("page_size"));
    let (cleared, _) = write(
        &world,
        "writer1",
        7_100,
        "page_size",
        SettingLevel::Person,
        None,
        None,
        held,
    )
    .expect("the override is removed");
    let retyped_world = define(&cleared, retyped);
    assert_eq!(
        value_for(&retyped_world, "writer1", "page_size"),
        text_value("20")
    );
    let mut other_default = page_size();
    other_default.default = Value::Int(50);
    define(&world, other_default);
}

// Verifies: SET-2302
#[test]
fn removing_a_definition_removes_its_overrides_and_keeps_the_revision_rising() {
    let world = with_page_size();
    let world = set_tenant(&world, "page_size", Value::Int(30), 0);
    let tenant_revision = settings::tenant_revision(tenant(&world), &sid("page_size"));
    let world = set_person(&world, "writer1", "page_size", Value::Int(5), 0);
    let before = settings::tenant_revision(tenant(&world), &sid("page_size"));
    assert!(before > tenant_revision);

    let removed = must(
        &world,
        "owner",
        8_000,
        change(vec![SchemaChange::RemoveSetting(sid("page_size"))]),
    );
    assert!(settings::definition(schema_of(&removed), &sid("page_size")).is_none());
    assert_eq!(
        settings::overrides_of(tenant(&removed), &sid("page_size")).count(),
        0
    );
    assert_eq!(
        settings::tenant_revision(tenant(&removed), &sid("page_size")),
        before + 1,
        "the revision rises with the removal and is not reused"
    );
    assert!(
        settings::resolve(tenant(&removed), &user("alpha:writer1"), &sid("page_size")).is_err()
    );
    // Removing what is not defined is refused.
    let refused = refusal(step(
        &removed,
        "owner",
        8_100,
        change(vec![SchemaChange::RemoveSetting(sid("page_size"))]),
    ));
    assert_eq!(refused.kind, ErrorKind::InvalidSchema);
}

// ---------------------------------------------------------------------------------------------
// Resolution (SET-010 to SET-014, SET-022 to SET-024).
// ---------------------------------------------------------------------------------------------

// Verifies: SET-010
// Verifies: SET-012
// Verifies: SET-022
#[test]
fn a_read_answers_the_most_specific_override_and_removing_it_falls_back() {
    let world = with_team(&with_page_size(), "desk", &["writer1"]);
    let world = set_tenant(&world, "page_size", Value::Int(30), 0);
    let found = resolve_as(&world, Some("writer1"), "page_size");
    assert_eq!(
        (found.value, found.source),
        (Value::Int(30), SettingSource::Tenant)
    );

    let team_rev = settings::tenant_revision(tenant(&world), &sid("page_size"));
    let world = set_team(&world, "desk", "page_size", Value::Int(40), 0);
    let found = resolve_as(&world, Some("writer1"), "page_size");
    assert_eq!(found.value, Value::Int(40));
    assert_eq!(found.source, SettingSource::Team);
    assert_eq!(found.team, Some(team_id("desk")));
    assert!(found.tenant_revision > team_rev);
    assert_eq!(found.platform_revision, 0);

    let world = set_person(&world, "writer1", "page_size", Value::Int(50), 0);
    let found = resolve_as(&world, Some("writer1"), "page_size");
    assert_eq!(
        (found.value, found.source),
        (Value::Int(50), SettingSource::Person)
    );

    // Another person of the tenant, outside the team, reads the tenant level.
    let other = resolve_as(&world, Some("writer2"), "page_size");
    assert_eq!(
        (other.value, other.source),
        (Value::Int(30), SettingSource::Tenant)
    );

    // Removing the person's override returns the team's value, then removing the team's returns
    // the tenant's, then removing the tenant's returns the default.
    let revision = settings::override_of(
        tenant(&world),
        &sid("page_size"),
        SettingLevel::Person,
        "alpha:writer1",
    )
    .expect("held")
    .revision;
    let (world, _) = write(
        &world,
        "writer1",
        9_000,
        "page_size",
        SettingLevel::Person,
        None,
        None,
        revision,
    )
    .expect("removed");
    assert_eq!(value_for(&world, "writer1", "page_size"), Value::Int(40));
    let revision = settings::override_of(
        tenant(&world),
        &sid("page_size"),
        SettingLevel::Team,
        "desk",
    )
    .expect("held")
    .revision;
    let (world, _) = write(
        &world,
        "owner",
        9_100,
        "page_size",
        SettingLevel::Team,
        Some("desk"),
        None,
        revision,
    )
    .expect("removed");
    assert_eq!(value_for(&world, "writer1", "page_size"), Value::Int(30));
    let revision = settings::override_of(
        tenant(&world),
        &sid("page_size"),
        SettingLevel::Tenant,
        "alpha",
    )
    .expect("held")
    .revision;
    let (world, _) = write(
        &world,
        "owner",
        9_200,
        "page_size",
        SettingLevel::Tenant,
        None,
        None,
        revision,
    )
    .expect("removed");
    let found = resolve_as(&world, Some("writer1"), "page_size");
    assert_eq!(
        (found.value, found.source),
        (Value::Int(20), SettingSource::Default)
    );
}

// Verifies: SET-011
#[test]
fn of_several_teams_the_team_whose_id_comes_first_applies_whatever_the_order_of_joining() {
    for order in [["bravo", "alpha_team"], ["alpha_team", "bravo"]] {
        let mut world = with_page_size();
        for team in order {
            world = with_team(&world, team, &["writer1"]);
        }
        // The override of the later team is written first, so neither the order of joining nor
        // the order of writing decides.
        world = set_team(&world, "bravo", "page_size", Value::Int(2), 0);
        let rev = settings::tenant_revision(tenant(&world), &sid("page_size"));
        world = set_team(&world, "alpha_team", "page_size", Value::Int(1), 0);
        assert!(settings::tenant_revision(tenant(&world), &sid("page_size")) > rev);
        let found = resolve_as(&world, Some("writer1"), "page_size");
        assert_eq!(found.value, Value::Int(1), "{order:?}");
        assert_eq!(found.team, Some(team_id("alpha_team")), "{order:?}");
    }
    // A team without an override does not shadow one with it: byte order applies among the teams
    // that hold an override.
    let mut world = with_page_size();
    world = with_team(&world, "aaa", &["writer1"]);
    world = with_team(&world, "zzz", &["writer1"]);
    world = set_team(&world, "zzz", "page_size", Value::Int(9), 0);
    let found = resolve_as(&world, Some("writer1"), "page_size");
    assert_eq!(found.team, Some(team_id("zzz")));
}

// Verifies: SET-013
#[test]
fn an_anonymous_reader_is_answered_from_the_tenant_level_and_the_default() {
    let world = with_team(&with_page_size(), "desk", &["writer1"]);
    let world = set_person(&world, "writer1", "page_size", Value::Int(5), 0);
    let anonymous = resolve_as(&world, None, "page_size");
    assert_eq!(
        (anonymous.value.clone(), anonymous.source),
        (Value::Int(20), SettingSource::Default)
    );
    let world = set_team(&world, "desk", "page_size", Value::Int(6), 0);
    let world = set_tenant(&world, "page_size", Value::Int(30), 0);
    let anonymous = resolve_as(&world, None, "page_size");
    assert_eq!(
        (anonymous.value, anonymous.source),
        (Value::Int(30), SettingSource::Tenant)
    );
    // The system has no person either.
    let system = settings::resolve(tenant(&world), &Principal::System, &sid("page_size")).unwrap();
    assert_eq!(system.source, SettingSource::Tenant);
}

// Verifies: SET-014
// Verifies: SET-023
// Verifies: SET-024
#[test]
fn a_lock_closes_the_levels_below_it_keeps_their_overrides_and_lifting_it_applies_them_again() {
    let world = with_team(&with_page_size(), "desk", &["writer1"]);
    let world = set_tenant(&world, "page_size", Value::Int(30), 0);
    let world = set_team(&world, "desk", "page_size", Value::Int(40), 0);
    let world = set_person(&world, "writer1", "page_size", Value::Int(50), 0);
    assert_eq!(value_for(&world, "writer1", "page_size"), Value::Int(50));

    // Locked by the tenant: the team and person levels are closed and ignored.
    let mut locked = page_size();
    locked.locked_by = Some(SettingLevel::Tenant);
    let world = define(&world, locked);
    let found = resolve_as(&world, Some("writer1"), "page_size");
    assert_eq!(
        (found.value, found.source),
        (Value::Int(30), SettingSource::Tenant)
    );
    // A write at a closed level is refused, and nothing is deleted.
    let refused = refusal(write(
        &world,
        "writer1",
        9_000,
        "page_size",
        SettingLevel::Person,
        None,
        Some(Value::Int(1)),
        1,
    ));
    assert_eq!(refused.kind, ErrorKind::InvalidIntent);
    assert!(refused.message.contains("lock"), "{}", refused.message);
    assert_eq!(
        settings::overrides_of(tenant(&world), &sid("page_size")).count(),
        3
    );

    // Locked by the platform: only the platform level is left, so the tenant's override is closed.
    let mut platform = page_size();
    platform.levels.insert(SettingLevel::Platform);
    platform.locked_by = Some(SettingLevel::Platform);
    let world = define(&world, platform);
    let found = resolve_as(&world, Some("writer1"), "page_size");
    assert_eq!(
        (found.value, found.source),
        (Value::Int(20), SettingSource::Default)
    );

    // Lifted: the overrides that were kept apply again.
    let world = define(&world, page_size());
    assert_eq!(value_for(&world, "writer1", "page_size"), Value::Int(50));
}

// Verifies: SET-020
#[test]
fn a_write_at_a_level_the_definition_does_not_allow_is_refused() {
    let mut tenant_only = layered("banner", "string", text_value("hello"));
    tenant_only.levels = BTreeSet::from([SettingLevel::Tenant]);
    let world = define(&notes_world(), tenant_only);
    let refused = refusal(write(
        &world,
        "writer1",
        9_000,
        "banner",
        SettingLevel::Person,
        None,
        Some(text_value("hi")),
        0,
    ));
    assert_eq!(refused.kind, ErrorKind::InvalidIntent);
    assert!(
        refused.message.contains("does not allow"),
        "{}",
        refused.message
    );
    let refused = refusal(write(
        &world,
        "owner",
        9_000,
        "banner",
        SettingLevel::Team,
        Some("desk"),
        Some(text_value("hi")),
        0,
    ));
    assert_eq!(refused.kind, ErrorKind::InvalidIntent);
    // And an override that is read ignores a level the definition does not allow.
    set_tenant(&world, "banner", text_value("tenant banner"), 0);
}

// Verifies: SET-021
#[test]
fn a_value_of_another_type_and_a_null_are_refused_and_the_other_types_are_accepted() {
    let world = define(
        &define(
            &define(
                &with_page_size(),
                layered("beta", "bool", Value::Bool(false)),
            ),
            layered("motto", "string", text_value("")),
        ),
        layered("since", "timestamp", Value::Time(0)),
    );
    for (setting, wrong) in [
        ("page_size", text_value("ten")),
        ("beta", Value::Int(1)),
        ("motto", Value::Bool(true)),
        ("since", Value::Int(5)),
        ("page_size", Value::Null),
    ] {
        let refused = refusal(write(
            &world,
            "writer1",
            9_000,
            setting,
            SettingLevel::Person,
            None,
            Some(wrong),
            0,
        ));
        assert_eq!(refused.kind, ErrorKind::TypeMismatch, "{setting}");
    }
    let world = set_person(&world, "writer1", "beta", Value::Bool(true), 0);
    let world = set_person(&world, "writer1", "motto", text_value("be kind"), 0);
    let world = set_person(&world, "writer1", "since", Value::Time(1_700), 0);
    assert_eq!(value_for(&world, "writer1", "beta"), Value::Bool(true));
    assert_eq!(value_for(&world, "writer1", "motto"), text_value("be kind"));
    assert_eq!(value_for(&world, "writer1", "since"), Value::Time(1_700));
    // A string past the stated maximum is refused as too large, not as another type.
    let huge = text_value(&"x".repeat(70_000));
    let refused = refusal(write(
        &world,
        "writer2",
        9_000,
        "motto",
        SettingLevel::Person,
        None,
        Some(huge),
        0,
    ));
    assert_eq!(refused.kind, ErrorKind::LimitExceeded);
}

#[test]
fn a_setting_that_is_not_defined_and_a_removal_of_nothing_are_refused() {
    let world = with_page_size();
    let refused = refusal(write(
        &world,
        "writer1",
        9_000,
        "nothing",
        SettingLevel::Person,
        None,
        Some(Value::Int(1)),
        0,
    ));
    assert_eq!(refused.kind, ErrorKind::UnknownObject);
    let refused = refusal(write(
        &world,
        "writer1",
        9_000,
        "page_size",
        SettingLevel::Person,
        None,
        None,
        0,
    ));
    assert_eq!(refused.kind, ErrorKind::InvalidIntent);
    assert!(settings::resolve(tenant(&world), &user("alpha:writer1"), &sid("nothing")).is_err());
    assert!(matches!(
        settings::resolve(tenant(&world), &user("alpha:writer1"), &sid("nothing")),
        Err(error) if error.kind == ErrorKind::UnknownObject
    ));
}

// ---------------------------------------------------------------------------------------------
// Who may write each level (SET-030 to SET-034).
// ---------------------------------------------------------------------------------------------

// Verifies: SET-030
#[test]
fn the_platform_level_is_not_written_through_the_tenant() {
    let mut def = page_size();
    def.levels.insert(SettingLevel::Platform);
    let world = define(&notes_world(), def);
    for who in ["owner", "writer1"] {
        let refused = refusal(write(
            &world,
            who,
            9_000,
            "page_size",
            SettingLevel::Platform,
            None,
            Some(Value::Int(1)),
            0,
        ));
        assert_eq!(refused.kind, ErrorKind::InvalidIntent, "{who}");
        assert!(
            refused.message.contains("control plane"),
            "{}",
            refused.message
        );
    }
    // The platform level contributes no value while the control plane holds none.
    assert_eq!(
        resolve_as(&world, Some("owner"), "page_size").source,
        SettingSource::Default
    );
}

// Verifies: SET-031
// Verifies: SET-032
#[test]
fn the_tenant_and_team_levels_need_their_capabilities_which_the_owner_holds() {
    let world = with_team(&with_page_size(), "desk", &["writer1"]);
    for (who, level, team) in [
        ("writer1", SettingLevel::Tenant, None),
        ("editor", SettingLevel::Tenant, None),
        ("stranger", SettingLevel::Tenant, None),
        ("writer1", SettingLevel::Team, Some("desk")),
        ("editor", SettingLevel::Team, Some("desk")),
    ] {
        let refused = refusal(write(
            &world,
            who,
            9_000,
            "page_size",
            level,
            team,
            Some(Value::Int(1)),
            0,
        ));
        assert_eq!(refused.kind, ErrorKind::Unauthorized, "{who} {level:?}");
        assert!(
            refused.evidence.iter().any(|check| !check.passed
                && check
                    .rule
                    .starts_with("permission:capability:setting-write-")),
            "{who} {level:?}: {:?}",
            refused.evidence
        );
    }
    // The owner holds both.
    set_tenant(&world, "page_size", Value::Int(1), 0);
    let world = set_team(&world, "desk", "page_size", Value::Int(2), 0);
    // A role that a schema change gives the capability may write that level and no other.
    let world = must(
        &world,
        "owner",
        9_500,
        change(vec![SchemaChange::DefineRolePermissions {
            role: role("editor"),
            permissions: vec![Permission::Capability("setting-write-team".into())],
        }]),
    );
    let revision = settings::tenant_revision(tenant(&world), &sid("page_size"));
    let (world, _) = write(
        &world,
        "editor",
        9_600,
        "page_size",
        SettingLevel::Team,
        Some("desk"),
        Some(Value::Int(3)),
        settings::override_of(
            tenant(&world),
            &sid("page_size"),
            SettingLevel::Team,
            "desk",
        )
        .expect("held")
        .revision,
    )
    .expect("the editor holds setting-write-team");
    assert!(settings::tenant_revision(tenant(&world), &sid("page_size")) > revision);
    let refused = refusal(write(
        &world,
        "editor",
        9_700,
        "page_size",
        SettingLevel::Tenant,
        None,
        Some(Value::Int(3)),
        0,
    ));
    assert_eq!(refused.kind, ErrorKind::Unauthorized);
    // A team that does not exist is refused.
    let refused = refusal(write(
        &world,
        "owner",
        9_800,
        "page_size",
        SettingLevel::Team,
        Some("ghosts"),
        Some(Value::Int(3)),
        0,
    ));
    assert_eq!(refused.kind, ErrorKind::InvalidIntent);
}

// Verifies: SET-033
// Verifies: SET-034
#[test]
fn a_person_writes_only_their_own_level_whatever_their_role_and_no_one_else_does() {
    let world = with_page_size();
    // Every signed-in person writes their own level, the stranger with no role and the owner too.
    let world = set_person(&world, "stranger", "page_size", Value::Int(7), 0);
    let world = set_person(&world, "owner", "page_size", Value::Int(8), 0);
    // The override is stored under the person that wrote it, and under nobody else.
    for (who, expected) in [("stranger", 7), ("owner", 8)] {
        let held = settings::override_of(
            tenant(&world),
            &sid("page_size"),
            SettingLevel::Person,
            &format!("alpha:{who}"),
        )
        .expect("held");
        assert_eq!(held.value, Value::Int(expected));
        assert_eq!(held.set_by, principal_id(&format!("alpha:{who}")));
    }
    // The owner's write did not touch the stranger's.
    assert_eq!(value_for(&world, "stranger", "page_size"), Value::Int(7));
    // The system and an anonymous caller write nobody's level.
    for principal in [Principal::System, Principal::Anonymous] {
        let refused = world
            .step(
                &Intent::WriteSetting {
                    setting: sid("page_size"),
                    level: SettingLevel::Person,
                    team: None,
                    value: Some(Value::Int(1)),
                    expected_revision: 0,
                },
                &ctx("alpha", principal.clone(), 9_000),
            )
            .expect_err("refused");
        assert_eq!(refused.kind, ErrorKind::Unauthorized, "{principal:?}");
    }
    // A team named at the person level is not a way to write another's override.
    let refused = refusal(write(
        &world,
        "writer1",
        9_000,
        "page_size",
        SettingLevel::Person,
        Some("desk"),
        Some(Value::Int(1)),
        0,
    ));
    assert_eq!(refused.kind, ErrorKind::InvalidIntent);
}

// ---------------------------------------------------------------------------------------------
// Revisions (SET-025, SET-051, D-5045).
// ---------------------------------------------------------------------------------------------

// Verifies: SET-025
// Verifies: SET-2303
#[test]
fn a_write_against_another_revision_is_refused_and_a_revision_is_not_reused() {
    let world = with_page_size();
    let defined = settings::tenant_revision(tenant(&world), &sid("page_size"));
    assert_eq!(defined, 1, "the definition is the first change");

    // A first write expects revision zero and makes the next revision of the setting.
    let (world, first) = write(
        &world,
        "writer1",
        9_000,
        "page_size",
        SettingLevel::Person,
        None,
        Some(Value::Int(5)),
        0,
    )
    .expect("accepted");
    assert_eq!(
        first.output,
        Output::Setting {
            setting: sid("page_size"),
            revision: 2
        }
    );
    // A stale writer, and one that guesses, are refused with a conflict that changes nothing.
    for expected in [0, 1, 3] {
        let refused = refusal(write(
            &world,
            "writer1",
            9_100,
            "page_size",
            SettingLevel::Person,
            None,
            Some(Value::Int(6)),
            expected,
        ));
        assert_eq!(refused.kind, ErrorKind::VersionConflict, "{expected}");
    }
    assert_eq!(value_for(&world, "writer1", "page_size"), Value::Int(5));
    // The current revision succeeds and raises it again.
    let (world, second) = write(
        &world,
        "writer1",
        9_200,
        "page_size",
        SettingLevel::Person,
        None,
        Some(Value::Int(6)),
        2,
    )
    .expect("accepted");
    assert_eq!(
        second.output,
        Output::Setting {
            setting: sid("page_size"),
            revision: 3
        }
    );
    // Removed and written again, the override has a later revision than a stale writer can hold.
    let (world, _) = write(
        &world,
        "writer1",
        9_300,
        "page_size",
        SettingLevel::Person,
        None,
        None,
        3,
    )
    .expect("removed");
    let (world, again) = write(
        &world,
        "writer1",
        9_400,
        "page_size",
        SettingLevel::Person,
        None,
        Some(Value::Int(9)),
        0,
    )
    .expect("accepted");
    assert_eq!(
        again.output,
        Output::Setting {
            setting: sid("page_size"),
            revision: 5
        }
    );
    for stale in [2, 3] {
        let refused = refusal(write(
            &world,
            "writer1",
            9_500,
            "page_size",
            SettingLevel::Person,
            None,
            Some(Value::Int(1)),
            stale,
        ));
        assert_eq!(refused.kind, ErrorKind::VersionConflict, "{stale}");
    }
    assert_eq!(
        settings::tenant_revision(tenant(&world), &sid("page_size")),
        5
    );
}

// ---------------------------------------------------------------------------------------------
// Reading another person's value (SET-1541, SET-1542).
// ---------------------------------------------------------------------------------------------

// Verifies: SET-1541
#[test]
fn a_person_cannot_read_the_value_of_another_person() {
    let world = with_page_size();
    let world = set_person(&world, "writer1", "page_size", Value::Int(5), 0);
    // Another person, an anonymous reader and the system read their own answer: the default.
    assert_eq!(value_for(&world, "writer2", "page_size"), Value::Int(20));
    assert_eq!(resolve_as(&world, None, "page_size").value, Value::Int(20));
    // Reading every setting answers the same for each reader, and none of them holds another's.
    let theirs = settings::resolve_all(tenant(&world), &user("alpha:writer2"));
    assert!(theirs.iter().all(|found| found.value != Value::Int(5)));
    let mine = settings::resolve_all(tenant(&world), &user("alpha:writer1"));
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].value, Value::Int(5));

    // The one way to ask for another person's value is the service call of the dispatcher, which
    // takes the person as an argument; the answer is the one the person would get.
    let for_recipient = settings::resolve_for(
        tenant(&world),
        &principal_id("alpha:writer1"),
        &sid("page_size"),
    )
    .expect("a recipient of the tenant");
    assert_eq!(
        for_recipient,
        settings::resolve(tenant(&world), &user("alpha:writer1"), &sid("page_size")).unwrap()
    );
    // A person who is not of the tenant is not found.
    let missing = settings::resolve_for(
        tenant(&world),
        &principal_id("alpha:nobody"),
        &sid("page_size"),
    )
    .expect_err("no such principal");
    assert_eq!(missing.kind, ErrorKind::UnknownObject);
    let undefined = settings::resolve_for(
        tenant(&world),
        &principal_id("alpha:writer1"),
        &sid("nothing"),
    )
    .expect_err("no such setting");
    assert_eq!(undefined.kind, ErrorKind::UnknownObject);
}

// ---------------------------------------------------------------------------------------------
// Events (SET-040, SET-1543, D-5048).
// ---------------------------------------------------------------------------------------------

// Verifies: SET-040
// Verifies: SET-1543
#[test]
fn the_event_of_a_change_names_the_setting_the_level_and_the_subject_and_carries_no_value() {
    let secret = "hunter2-a-personal-value";
    let world = define(&notes_world(), layered("motto", "string", text_value("")));
    let world = with_team(&world, "desk", &["writer1"]);

    let cases = [
        ("writer1", SettingLevel::Person, None, "alpha:writer1"),
        ("owner", SettingLevel::Tenant, None, "alpha"),
        ("owner", SettingLevel::Team, Some("desk"), "desk"),
    ];
    for (who, level, team, subject) in cases {
        let (next, outcome) = write(
            &world,
            who,
            9_000,
            "motto",
            level,
            team,
            Some(text_value(secret)),
            0,
        )
        .expect("accepted");
        assert_eq!(outcome.events.len(), 1);
        assert_eq!(
            outcome.events[0].body,
            EventBody::SettingChanged {
                setting: sid("motto"),
                level,
                subject: subject.to_string(),
            }
        );
        // The value is in the state and in the delta, and in no event, effect or evidence.
        let events = serde_json::to_string(&outcome.events).unwrap();
        let effects = serde_json::to_string(&outcome.effects).unwrap();
        let evidence = serde_json::to_string(&outcome.evidence).unwrap();
        for (what, text) in [
            ("events", events),
            ("effects", effects),
            ("evidence", evidence),
        ] {
            assert!(
                !text.contains(secret),
                "{what} of {level:?} carry the value: {text}"
            );
        }
        assert!(serde_json::to_string(&outcome.delta)
            .unwrap()
            .contains(secret));
        assert_eq!(
            settings::override_of(tenant(&next), &sid("motto"), level, subject)
                .expect("stored")
                .value,
            text_value(secret)
        );
    }
}

#[test]
fn a_refusal_names_the_setting_and_never_the_value() {
    let secret = "hunter2-a-personal-value";
    let world = with_page_size();
    let refused = refusal(write(
        &world,
        "writer1",
        9_000,
        "page_size",
        SettingLevel::Person,
        None,
        Some(text_value(secret)),
        0,
    ));
    assert_eq!(refused.kind, ErrorKind::TypeMismatch);
    let text = serde_json::to_string(&refused).unwrap();
    assert!(!text.contains(secret), "{text}");
}

#[test]
fn a_change_of_a_definition_is_an_event_at_the_tenant_level_and_raises_the_revision() {
    let world = notes_world();
    let (world, outcome) = step(
        &world,
        "owner",
        5_000,
        change(vec![SchemaChange::DefineSetting(page_size())]),
    )
    .expect("accepted");
    let kinds: Vec<&EventBody> = outcome.events.iter().map(|event| &event.body).collect();
    assert!(matches!(kinds[0], EventBody::SchemaChanged { .. }));
    assert_eq!(
        kinds[1],
        &EventBody::SettingChanged {
            setting: sid("page_size"),
            level: SettingLevel::Tenant,
            subject: "alpha".into(),
        }
    );
    assert_eq!(
        settings::tenant_revision(tenant(&world), &sid("page_size")),
        1
    );
    let (world, _) = step(
        &world,
        "owner",
        5_100,
        change(vec![SchemaChange::DefineSetting(page_size())]),
    )
    .expect("accepted");
    assert_eq!(
        settings::tenant_revision(tenant(&world), &sid("page_size")),
        2
    );
}

// ---------------------------------------------------------------------------------------------
// A setting nobody defined has no state at all.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_tenant_without_settings_holds_no_table_of_them_and_hashes_as_before() {
    let world = notes_world();
    assert!(schema_of(&world).settings.is_none());
    assert!(schema_of(&world).capabilities.is_none());
    assert!(tenant(&world).setting_values.is_none());
    // Defining and removing a setting leaves a schema equal to the one without.
    let before = canon::hash_of(schema_of(&world));
    let with = define(&world, page_size());
    let without = must(
        &with,
        "owner",
        5_100,
        change(vec![SchemaChange::RemoveSetting(sid("page_size"))]),
    );
    let mut restored = TenantSchema::clone(schema_of(&without));
    restored.version = schema_of(&world).version;
    assert_eq!(canon::hash_of(&restored), before);
}

// ---------------------------------------------------------------------------------------------
// What a definition names (WG-5101, SET-2304).
// ---------------------------------------------------------------------------------------------

fn tiny() -> AutomationDef {
    let done = StepName::new("done").expect("step name");
    AutomationDef {
        id: WorkflowId::new("tiny").expect("workflow id"),
        label: "Tiny".into(),
        run_as: RunAs {
            roles: Default::default(),
        },
        run_key: None,
        start: done.clone(),
        steps: std::collections::BTreeMap::from([(
            done,
            AutomationStepDef {
                action: AutomationStep::End,
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

fn on_setting(setting: &str) -> TriggerDef {
    TriggerDef {
        id: TriggerId::new("on_setting").expect("trigger id"),
        on: TriggerEvent::SettingChanged {
            setting: sid(setting),
        },
        condition: None,
        automation: WorkflowId::new("tiny").expect("workflow id"),
        paused: None,
        defined_by: None,
        defined_at: None,
    }
}

// Verifies: SET-2304
#[test]
fn a_trigger_on_a_setting_that_is_not_defined_is_refused_when_it_is_defined() {
    let world = must(
        &with_page_size(),
        "owner",
        5_100,
        change(vec![SchemaChange::DefineAutomation(tiny())]),
    );
    let refused = refusal(step(
        &world,
        "owner",
        5_200,
        change(vec![SchemaChange::DefineTrigger(on_setting("nothing"))]),
    ));
    assert_eq!(refused.kind, ErrorKind::InvalidSchema);
    assert!(
        refused.message.contains("unknown setting nothing"),
        "{}",
        refused.message
    );
    must(
        &world,
        "owner",
        5_200,
        change(vec![SchemaChange::DefineTrigger(on_setting("page_size"))]),
    );
}

// Verifies: SET-2302
#[test]
fn removing_a_setting_that_a_trigger_names_is_refused_while_it_names_it() {
    let world = must(
        &with_page_size(),
        "owner",
        5_100,
        change(vec![
            SchemaChange::DefineAutomation(tiny()),
            SchemaChange::DefineTrigger(on_setting("page_size")),
        ]),
    );
    let refused = refusal(step(
        &world,
        "owner",
        5_200,
        change(vec![SchemaChange::RemoveSetting(sid("page_size"))]),
    ));
    assert_eq!(refused.kind, ErrorKind::InvalidSchema);
    assert!(
        refused.message.contains("unknown setting page_size"),
        "{}",
        refused.message
    );
    // Removed together with the trigger, it goes.
    must(
        &world,
        "owner",
        5_300,
        change(vec![
            SchemaChange::RemoveTrigger(TriggerId::new("on_setting").unwrap()),
            SchemaChange::RemoveSetting(sid("page_size")),
        ]),
    );
}

// ---------------------------------------------------------------------------------------------
// The capabilities of products (D-6590, D-6701).
// ---------------------------------------------------------------------------------------------

fn promote() -> CapabilityDef {
    CapabilityDef {
        id: CapabilityName("member_tiers__promote".into()),
        name: "Promote members".into(),
        description: "Promote a member to a higher tier.".into(),
    }
}

fn product_role(id: &str) -> SchemaChange {
    SchemaChange::DefineRole(RoleDef {
        id: role(id),
        label: id.into(),
    })
}

fn grant(role_name: &str, capability: &str) -> SchemaChange {
    SchemaChange::DefineRolePermissions {
        role: role(role_name),
        permissions: vec![Permission::Capability(capability.into())],
    }
}

fn held(world: &World, who: &str) -> BTreeSet<String> {
    rules::held_capabilities(tenant(world), &user(&format!("alpha:{who}")))
}

#[test]
fn a_grant_of_a_capability_of_the_product_form_that_nobody_declared_is_refused() {
    let world = notes_world();
    let refused = refusal(step(
        &world,
        "owner",
        5_000,
        change(vec![
            product_role("member_tiers__editor"),
            grant("member_tiers__editor", "member_tiers__promote"),
        ]),
    ));
    assert_eq!(refused.kind, ErrorKind::InvalidSchema);
    assert!(
        refused
            .message
            .contains("neither a platform capability nor declared"),
        "{}",
        refused.message
    );
    // A tenant's own role is refused it as well.
    let refused = refusal(step(
        &world,
        "owner",
        5_000,
        change(vec![grant("editor", "member_tiers__promote")]),
    ));
    assert_eq!(refused.kind, ErrorKind::InvalidSchema);
}

// Verifies: SET-2300
#[test]
fn a_declared_capability_is_granted_by_a_role_of_its_product_and_by_a_role_of_the_tenant() {
    let world = must(
        &notes_world(),
        "owner",
        5_000,
        change(vec![
            SchemaChange::DefineCapability(promote()),
            product_role("member_tiers__editor"),
            grant("member_tiers__editor", "member_tiers__promote"),
            grant("editor", "member_tiers__promote"),
        ]),
    );
    assert!(schema_of(&world).capabilities.as_ref().is_some_and(
        |declared| declared.contains_key(&CapabilityName("member_tiers__promote".into()))
    ));
    // A person holds it through a role that grants it, and a person without the role does not.
    assert!(held(&world, "editor").contains("member_tiers__promote"));
    assert!(!held(&world, "writer1").contains("member_tiers__promote"));
    // A capability of the platform form needs no declaration.
    must(
        &world,
        "owner",
        5_100,
        change(vec![grant("writer", "content-read")]),
    );
}

// Verifies: SET-2300
#[test]
fn a_role_of_a_product_is_refused_a_capability_of_another_product() {
    let other = CapabilityDef {
        id: CapabilityName("badges__award".into()),
        name: "Award badges".into(),
        description: "Award a badge.".into(),
    };
    let world = notes_world();
    let refused = refusal(step(
        &world,
        "owner",
        5_000,
        change(vec![
            SchemaChange::DefineCapability(promote()),
            SchemaChange::DefineCapability(other),
            product_role("member_tiers__editor"),
            grant("member_tiers__editor", "badges__award"),
        ]),
    ));
    assert_eq!(refused.kind, ErrorKind::InvalidSchema);
    assert!(
        refused.message.contains("belongs to product badges"),
        "{}",
        refused.message
    );
}

#[test]
fn a_capability_is_not_removed_while_a_role_grants_it_and_a_malformed_id_is_refused() {
    let world = must(
        &notes_world(),
        "owner",
        5_000,
        change(vec![
            SchemaChange::DefineCapability(promote()),
            grant("editor", "member_tiers__promote"),
        ]),
    );
    let refused = refusal(step(
        &world,
        "owner",
        5_100,
        change(vec![SchemaChange::RemoveCapability(promote().id)]),
    ));
    assert_eq!(refused.kind, ErrorKind::InvalidSchema);
    // Revoke the grant and remove the capability in one change.
    let removed = must(
        &world,
        "owner",
        5_200,
        change(vec![
            SchemaChange::DefineRolePermissions {
                role: role("editor"),
                permissions: Vec::new(),
            },
            SchemaChange::RemoveCapability(promote().id),
        ]),
    );
    assert!(schema_of(&removed).capabilities.is_none());
    // Removing one that is not declared is refused.
    let refused = refusal(step(
        &removed,
        "owner",
        5_300,
        change(vec![SchemaChange::RemoveCapability(promote().id)]),
    ));
    assert_eq!(refused.kind, ErrorKind::InvalidSchema);
    // An id that does not begin with a product namespace is not a capability of a product.
    for bad in [
        "promote",
        "member_tiers_promote",
        "Member__promote",
        "__promote",
        "a__",
    ] {
        let def = CapabilityDef {
            id: CapabilityName(bad.into()),
            name: "Bad".into(),
            description: "Bad.".into(),
        };
        let refused = refusal(step(
            &notes_world(),
            "owner",
            5_000,
            change(vec![SchemaChange::DefineCapability(def)]),
        ));
        assert_eq!(refused.kind, ErrorKind::InvalidSchema, "{bad}");
    }
}

// ---------------------------------------------------------------------------------------------
// Declared types, and the arguments of a view that come from a setting (D-5046, D-7901, SET-001,
// SET-050 to SET-053, SET-1540).
// ---------------------------------------------------------------------------------------------

// Verifies: SET-001
// Verifies: SET-021
#[test]
fn a_setting_of_a_declared_enum_type_holds_only_a_value_of_that_enum() {
    let world = define(
        &notes_world(),
        layered("digest_mode", "notification-mode", text_value("on")),
    );
    let world = set_person(&world, "writer1", "digest_mode", text_value("digest"), 0);
    assert_eq!(
        value_for(&world, "writer1", "digest_mode"),
        text_value("digest")
    );
    assert_eq!(
        value_for(&world, "writer2", "digest_mode"),
        text_value("on")
    );
    let attempt = |value: Value| {
        refusal(write(
            &world,
            "writer2",
            6_300,
            "digest_mode",
            SettingLevel::Person,
            None,
            Some(value),
            0,
        ))
    };
    let refused = attempt(text_value("sometimes"));
    assert_eq!(refused.kind, ErrorKind::TypeMismatch);
    assert!(
        refused
            .message
            .contains("\"sometimes\" is not a value of the enum type notification-mode"),
        "{}",
        refused.message
    );
    let refused = attempt(Value::Int(1));
    assert_eq!(refused.kind, ErrorKind::TypeMismatch);
    assert!(
        refused.message.contains("which takes text"),
        "{}",
        refused.message
    );
    // The default is a value of the type too (SET-002).
    let bad_default = layered("digest_mode", "notification-mode", text_value("never"));
    let refused = refusal(step(
        &notes_world(),
        "owner",
        5_000,
        change(vec![SchemaChange::DefineSetting(bad_default)]),
    ));
    assert_eq!(refused.kind, ErrorKind::InvalidSchema);
    assert!(
        refused.message.contains("is not a value of the enum type"),
        "{}",
        refused.message
    );
}

// Verifies: SET-001
#[test]
fn a_setting_of_a_declared_id_or_scalar_type_holds_a_value_within_its_constraints() {
    let world = define(
        &notes_world(),
        layered("review_role", "role-id", text_value("editor")),
    );
    let world = define(&world, layered("home_page", "url", text_value("/")));
    // An id is checked by the pattern and the length of its type, and not for existing: removing
    // what it names later must not leave a value that is not of its type (SET-004).
    let world = set_tenant(&world, "review_role", text_value("reviewer"), 0);
    assert_eq!(
        value_for(&world, "writer1", "review_role"),
        text_value("reviewer")
    );
    let refused = refusal(write(
        &world,
        "owner",
        6_300,
        "review_role",
        SettingLevel::Tenant,
        None,
        Some(text_value("Not A Role")),
        settings::tenant_revision(tenant(&world), &sid("review_role")),
    ));
    assert_eq!(refused.kind, ErrorKind::TypeMismatch);
    assert!(
        refused
            .message
            .contains("is not a value of the id type role-id"),
        "{}",
        refused.message
    );
    // A scalar without constraints takes any text, within the limits of a string.
    let world = set_tenant(&world, "home_page", text_value("https://example.test"), 0);
    assert_eq!(
        value_for(&world, "writer1", "home_page"),
        text_value("https://example.test")
    );
    let refused = refusal(write(
        &world,
        "owner",
        6_400,
        "home_page",
        SettingLevel::Tenant,
        None,
        Some(Value::Str("x".repeat(limits::MAX_STRING_BYTES + 1))),
        settings::tenant_revision(tenant(&world), &sid("home_page")),
    ));
    assert_eq!(refused.kind, ErrorKind::LimitExceeded);
}

// Verifies: SET-001
#[test]
fn a_type_that_is_no_declared_type_is_refused_with_the_names_that_are() {
    let refused = refusal(step(
        &notes_world(),
        "owner",
        5_000,
        change(vec![SchemaChange::DefineSetting(layered(
            "colour",
            "colour-name",
            text_value("red"),
        ))]),
    ));
    assert_eq!(refused.kind, ErrorKind::InvalidSchema);
    assert!(
        refused.message.contains("notification-mode") && refused.message.contains("role-id"),
        "{}",
        refused.message
    );
    // A float stays out: the value algebra has none (D-7901).
    let refused = refusal(step(
        &notes_world(),
        "owner",
        5_000,
        change(vec![SchemaChange::DefineSetting(layered(
            "ratio",
            "f64",
            Value::Int(1),
        ))]),
    ));
    assert!(
        refused.message.contains("holds no float"),
        "{}",
        refused.message
    );
}

fn home_view(source: ArgSource) -> SchemaChange {
    SchemaChange::DefineView(ViewDef {
        id: ViewId::new("home").unwrap(),
        route: "/home".into(),
        title: "Home".into(),
        sections: std::collections::BTreeMap::from([(
            "main".to_string(),
            ViewSection {
                query: QueryId::new("note_by_slug").unwrap(),
                args: std::collections::BTreeMap::from([("slug".to_string(), source)]),
                include: vec![],
                markdown: vec![],
                single: true,
                aggregate: None,
            },
        )]),
    })
}

// Verifies: SET-1540
#[test]
fn a_view_argument_takes_a_setting_the_tenant_defines() {
    let refused = refusal(step(
        &notes_world(),
        "owner",
        5_000,
        change(vec![home_view(ArgSource::Setting(sid("home_slug")))]),
    ));
    assert_eq!(refused.kind, ErrorKind::InvalidSchema);
    assert!(
        refused
            .message
            .contains("home.main: unknown setting home_slug"),
        "{}",
        refused.message
    );
    // Defined in the same change, it is accepted.
    must(
        &notes_world(),
        "owner",
        5_000,
        change(vec![
            SchemaChange::DefineSetting(layered("home_slug", "string", text_value("first"))),
            home_view(ArgSource::Setting(sid("home_slug"))),
        ]),
    );
}

// Verifies: SET-050
// Verifies: SET-051
#[test]
fn the_key_of_a_copy_that_reads_a_setting_holds_its_revisions() {
    let world = define(
        &notes_world(),
        layered("home_slug", "string", text_value("first")),
    );
    let wanted = BTreeSet::from([sid("home_slug")]);
    let key = |world: &World, who: &str| -> Vec<String> {
        settings::copy_dependencies(tenant(world), &user(&format!("alpha:{who}")), &wanted)
            .iter()
            .map(|dependency| dependency.key_part())
            .collect()
    };
    let defined = key(&world, "writer1");
    assert_eq!(defined, vec!["home_slug@0.1".to_string()]);
    // A change at the tenant level raises the tenant revision, so a copy has a new key.
    let changed = set_tenant(&world, "home_slug", text_value("second"), 0);
    assert_eq!(key(&changed, "writer1"), vec!["home_slug@0.2".to_string()]);
    assert_ne!(key(&changed, "writer1"), defined);
    // A setting the tenant does not define adds nothing to the key.
    let none = settings::copy_dependencies(
        tenant(&world),
        &user("alpha:writer1"),
        &BTreeSet::from([sid("nothing")]),
    );
    assert!(none.is_empty());
}

// Verifies: SET-052
// Verifies: SET-053
#[test]
fn a_value_from_a_person_is_never_shared_and_one_from_a_team_has_the_class_of_the_team() {
    let world = define(
        &notes_world(),
        layered("home_slug", "string", text_value("first")),
    );
    let world = with_team(&world, "crew", &["writer1"]);
    let wanted = BTreeSet::from([sid("home_slug")]);
    let refinement = |world: &World, who: &str| {
        let found =
            settings::copy_dependencies(tenant(world), &user(&format!("alpha:{who}")), &wanted);
        (found[0].refinement.clone(), settings::shareable(&found))
    };
    // From the default: the class as the policies derive it, shared.
    assert_eq!(
        refinement(&world, "writer1"),
        (settings::Refinement::Shared, true)
    );
    // From the tenant: the same.
    let world = set_tenant(&world, "home_slug", text_value("tenant"), 0);
    assert_eq!(
        refinement(&world, "writer1"),
        (settings::Refinement::Shared, true)
    );
    // From a team: the class of that team, for its members only.
    let revision = settings::tenant_revision(tenant(&world), &sid("home_slug"));
    let world = set_team(&world, "crew", "home_slug", text_value("team"), 0);
    assert_eq!(
        refinement(&world, "writer1"),
        (settings::Refinement::Team(team_id("crew")), true)
    );
    assert_eq!(
        refinement(&world, "writer2"),
        (settings::Refinement::Shared, true)
    );
    assert!(settings::tenant_revision(tenant(&world), &sid("home_slug")) > revision);
    // From a person: the class of that person alone, and a copy that is not shared.
    let world = set_person(&world, "writer1", "home_slug", text_value("mine"), 0);
    assert_eq!(
        refinement(&world, "writer1"),
        (
            settings::Refinement::Person(principal_id("alpha:writer1")),
            false
        )
    );
    // Another person with no override of their own is not affected.
    assert_eq!(
        refinement(&world, "writer2"),
        (settings::Refinement::Shared, true)
    );
    // The classes of two people who each hold an override are not equal, so no copy is shared
    // between them.
    let world = set_person(&world, "writer2", "home_slug", text_value("theirs"), 0);
    assert_ne!(refinement(&world, "writer1"), refinement(&world, "writer2"));
}
