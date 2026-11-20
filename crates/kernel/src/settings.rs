//! Typed settings in layers (D-5040 to D-5049, SET-001 to SET-053).
//!
//! A setting has a definition, which is tenant metadata held in [`TenantSchema::settings`] and
//! changed by the schema changes `define-setting` and `remove-setting`, and any number of
//! overrides, which are data held in [`TenantState::setting_values`] and changed by the intent
//! `write-setting`. A definition states a name, a type, a default and the levels that may
//! override it, and may be locked by the tenant, which leaves only the platform and tenant
//! levels. The levels from the least to the most specific are the platform, the tenant, a team
//! and a person.
//!
//! Resolution is a pure function of the definition, the overrides and the teams
//! ([`resolve`]): the person's own override, then that of the team of the person whose identifier
//! comes first in ascending byte order, then the tenant's, then the platform's, then the default.
//! A level the definition does not allow, or that a lock closes, is ignored; an override at such a
//! level is kept and applies again when the level is open again (SET-024).
//!
//! Every override carries the revision of the setting at the write that made it. The revision of
//! a setting rises by one at each change of its definition or of any override, never falls and is
//! never reused, so a write that names the revision it read is refused when anything changed the
//! override in between, including a removal and a new write (SET-025, SET-051).
//!
//! The event of a change names the setting, the level and the subject and carries no value
//! (SET-040, SET-1543): a person's value can be personal data and an event is kept and delivered.
//!
//! **The platform level.** The specification keeps the platform level in the control plane
//! (CT-303, `rec-setting-platform-value`), not in a tenant: a tenant's write at that level is
//! refused here ([`write_setting`]) and the operator's write is the control plane's. The kernel is
//! pure, so the control plane's values are handed in: [`resolve_with`], [`resolve_all_with`] and
//! [`resolve_for_with`] take them as [`PlatformLevels`] and read the platform's value between the
//! tenant's and the default; [`resolve`], [`resolve_all`] and [`resolve_for`] are the same with
//! none held. The platform's own definitions come with a release (SET-005) and none ships: a
//! platform value applies to every tenant whose definition of that name opens the platform level
//! and has a type the value is of.

use crate::intent::{
    Check, CheckExt, Context, DeltaOp, DomainError, DomainErrorExt, ErrorKind, EventBody, Output,
};
use crate::limits;
use crate::rules::holds_any;
use crate::schema::{Grant, TenantSchema, TenantSchemaExt};
use crate::state::TenantState;
use crate::types::{
    Principal, PrincipalId, SchemaChange, SettingDef, SettingId, SettingLevel, SettingOverride,
    SettingResolution, SettingSource, SettingValues, TeamId, Value, ValueExt,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Reading the resolved value of the settings for oneself (SET-010).
pub const SETTING_READ: &str = "setting-read";
/// Defining, changing and locking the settings of the tenant (SET-001).
pub const SETTING_DEFINE: &str = "setting-define";
/// Setting and removing the platform level (SET-030).
pub const SETTING_WRITE_PLATFORM: &str = "setting-write-platform";
/// Setting and removing the tenant level (SET-031).
pub const SETTING_WRITE_TENANT: &str = "setting-write-tenant";
/// Setting and removing the team level (SET-032).
pub const SETTING_WRITE_TEAM: &str = "setting-write-team";
/// Setting and removing one's own person level (SET-033, SET-034).
pub const SETTING_WRITE_PERSON: &str = "setting-write-person";
/// Resolving the setting of another principal, which only the platform's services hold (SET-1541).
pub const SETTING_RESOLVE_FOR: &str = "setting-resolve-for";

/// The names of the setting definitions the platform releases (SET-003, SET-005). The platform's
/// definitions come with a release and none ships yet, so no name is taken; a release that adds
/// one lists it here, and the tenants that already define the name are refused at their next
/// schema change.
pub const PLATFORM_SETTINGS: &[&str] = &[];

/// The platform level of one setting as the control plane holds it: the value, absent once
/// removed, and the revision, which rises at each change and is never reused (SET-051).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlatformLevel {
    pub value: Option<Value>,
    pub revision: u64,
}

/// The platform level of the settings that have one, by setting.
pub type PlatformLevels = BTreeMap<SettingId, PlatformLevel>;

/// Why a value may not be the platform level of a setting, or `None`: the platform's value is a
/// flag, a whole number or text, and within the limits of a value (SET-021). Whether it is of the
/// type of a tenant's definition is decided where the tenant's definition is read.
pub fn platform_value_problem(value: &Value) -> Option<String> {
    match value {
        Value::Bool(_) | Value::Int(_) | Value::Str(_) => limits::value_problem(value),
        other => Some(format!(
            "a {} is not a value of a setting: a flag, a whole number or text is",
            other.kind()
        )),
    }
}

/// The place of a level from the least to the most specific.
pub fn rank(level: SettingLevel) -> u8 {
    match level {
        SettingLevel::Platform => 0,
        SettingLevel::Tenant => 1,
        SettingLevel::Team => 2,
        SettingLevel::Person => 3,
    }
}

/// Whether the definition lets a level hold an override that is read: the definition allows the
/// level and no lock closes it. A lock of the platform leaves the platform level and a lock of the
/// tenant leaves the platform and tenant levels (D-5043).
pub fn is_open(def: &SettingDef, level: SettingLevel) -> bool {
    def.levels.contains(&level) && def.locked_by.is_none_or(|lock| rank(level) <= rank(lock))
}

/// The levels of a definition that are open (SET-014).
pub fn open_levels(def: &SettingDef) -> BTreeSet<SettingLevel> {
    def.levels
        .iter()
        .copied()
        .filter(|level| is_open(def, *level))
        .collect()
}

/// Why a value is not a value of a setting type, or `None` when it is (D-5046, SET-021). The types
/// are `bool`, `i64`, `string` and `timestamp`, and the id of a declared type of kind id, scalar or
/// enum ([`crate::declared_types`]), whose values are text that decodes as the type. `f64` is not
/// held: the kernel's value algebra has no float, and one could not be added without giving up
/// the equality, the order and the hash of every value (D-7901).
///
/// The kind of the refusal comes with the text: `TypeMismatch` for a value of another type or a
/// type that is not held, `LimitExceeded` for a value past the stated maximums.
pub fn type_problem(ty: &str, value: &Value) -> Option<(ErrorKind, String)> {
    let admitted = match (ty, value) {
        ("bool", Value::Bool(_))
        | ("i64", Value::Int(_))
        | ("string", Value::Str(_))
        | ("timestamp", Value::Time(_)) => true,
        ("bool" | "i64" | "string" | "timestamp", _) => false,
        _ => match crate::declared_types::lookup(ty) {
            Some(declared) => {
                if let Some(problem) = crate::declared_types::value_problem(declared, value) {
                    return Some((ErrorKind::TypeMismatch, problem));
                }
                true
            }
            None => return Some((ErrorKind::TypeMismatch, unknown_type(ty))),
        },
    };
    if !admitted {
        return Some((
            ErrorKind::TypeMismatch,
            format!("a {} is not a value of type {ty}", value.kind()),
        ));
    }
    limits::value_problem(value).map(|problem| (ErrorKind::LimitExceeded, problem))
}

fn unknown_type(ty: &str) -> String {
    if ty == "f64" {
        "type f64 has no value in the kernel's value algebra, which holds no float".to_string()
    } else {
        format!(
            "type {ty} is not one the kernel holds: bool, i64, string, timestamp, or a declared type ({})",
            crate::declared_types::names()
        )
    }
}

/// The key of an override in [`SettingValues::overrides`].
pub fn override_key(setting: &SettingId, level: SettingLevel, subject: &str) -> String {
    format!("{setting}/{}/{subject}", level.as_str())
}

/// The definition of a setting of the tenant.
pub fn definition<'a>(schema: &'a TenantSchema, setting: &SettingId) -> Option<&'a SettingDef> {
    schema.settings.as_ref().and_then(|defs| defs.get(setting))
}

/// One override, by setting, level and subject (the tenant, a team or a person).
pub fn override_of<'a>(
    tenant: &'a TenantState,
    setting: &SettingId,
    level: SettingLevel,
    subject: &str,
) -> Option<&'a SettingOverride> {
    tenant
        .setting_values
        .as_deref()?
        .overrides
        .get(&override_key(setting, level, subject))
}

/// The revision of a setting at the tenant (SET-051): zero until its first change.
pub fn tenant_revision(tenant: &TenantState, setting: &SettingId) -> u64 {
    tenant
        .setting_values
        .as_deref()
        .and_then(|values| values.revisions.get(setting))
        .copied()
        .unwrap_or(0)
}

/// Every override of a setting, in the order of their keys.
pub fn overrides_of<'a>(
    tenant: &'a TenantState,
    setting: &'a SettingId,
) -> impl Iterator<Item = &'a SettingOverride> {
    tenant
        .setting_values
        .iter()
        .flat_map(|values| values.overrides.values())
        .filter(move |held| held.setting == *setting)
}

/// The value of a setting for a principal and the level it came from (SET-010 to SET-014).
///
/// An anonymous principal and the system have no person and no team, so they are answered from the
/// tenant level and the default (SET-013). The platform level is absent: see the module notes.
pub fn resolve(
    tenant: &TenantState,
    principal: &Principal,
    setting: &SettingId,
) -> Result<SettingResolution, DomainError> {
    resolve_with(tenant, principal, setting, &PlatformLevels::new())
}

/// [`resolve`] with the platform level the control plane holds: the value of the platform is read
/// after the tenant's and before the default, when the definition opens the platform level and
/// the value is of its type, and `platform_revision` of the answer is its revision.
pub fn resolve_with(
    tenant: &TenantState,
    principal: &Principal,
    setting: &SettingId,
    platform: &PlatformLevels,
) -> Result<SettingResolution, DomainError> {
    let def = definition(&tenant.schema, setting).ok_or_else(|| unknown_setting(setting))?;
    Ok(resolve_definition(tenant, def, principal, platform))
}

/// [`resolve_with`] for a definition already found.
fn resolve_definition(
    tenant: &TenantState,
    def: &SettingDef,
    principal: &Principal,
    platform: &PlatformLevels,
) -> SettingResolution {
    let person = match principal {
        Principal::User(id) => Some(id),
        _ => None,
    };
    let mut found: Option<(SettingSource, Option<TeamId>, Value)> = None;
    if let Some(id) = person {
        if is_open(def, SettingLevel::Person) {
            if let Some(held) = override_of(tenant, &def.id, SettingLevel::Person, id.as_str()) {
                found = Some((SettingSource::Person, None, held.value.clone()));
            }
        }
        if found.is_none() && is_open(def, SettingLevel::Team) {
            // Teams are held by identifier, so this order is the ascending byte order of the
            // identifiers, whatever the order in which the person joined them or the overrides
            // were written (SET-011).
            for team in tenant.schema.all_teams() {
                if !team.members.contains(id) {
                    continue;
                }
                if let Some(held) =
                    override_of(tenant, &def.id, SettingLevel::Team, team.id.as_str())
                {
                    found = Some((
                        SettingSource::Team,
                        Some(team.id.clone()),
                        held.value.clone(),
                    ));
                    break;
                }
            }
        }
    }
    if found.is_none() && is_open(def, SettingLevel::Tenant) {
        if let Some(held) = override_of(tenant, &def.id, SettingLevel::Tenant, tenant.id.as_str()) {
            found = Some((SettingSource::Tenant, None, held.value.clone()));
        }
    }
    let platform_level = platform.get(&def.id);
    if found.is_none() && is_open(def, SettingLevel::Platform) {
        if let Some(value) = platform_level.and_then(|level| level.value.as_ref()) {
            // A whole number is a timestamp when the setting is of that type, as in a tenant's
            // own write; a value that is not of the type is not the platform's answer here.
            let value = match (def.r#type.as_str(), value) {
                ("timestamp", Value::Int(at)) => Value::Time(*at),
                _ => value.clone(),
            };
            if type_problem(def.r#type.as_str(), &value).is_none() {
                found = Some((SettingSource::Platform, None, value));
            }
        }
    }
    let (source, team, value) =
        found.unwrap_or_else(|| (SettingSource::Default, None, def.default.clone()));
    SettingResolution {
        setting: def.id.clone(),
        value,
        source,
        team,
        platform_revision: platform_level.map_or(0, |level| level.revision),
        tenant_revision: tenant_revision(tenant, &def.id),
    }
}

/// Every setting of the tenant resolved for a principal, in the order of the setting ids.
pub fn resolve_all(tenant: &TenantState, principal: &Principal) -> Vec<SettingResolution> {
    resolve_all_with(tenant, principal, &PlatformLevels::new())
}

/// [`resolve_all`] with the platform level the control plane holds (see [`resolve_with`]).
pub fn resolve_all_with(
    tenant: &TenantState,
    principal: &Principal,
    platform: &PlatformLevels,
) -> Vec<SettingResolution> {
    tenant
        .schema
        .settings
        .iter()
        .flat_map(|defs| defs.values())
        .map(|def| resolve_definition(tenant, def, principal, platform))
        .collect()
}

/// The setting of a recipient, for the platform component that renders a message for them
/// (CT-816, SET-1541): the answer the recipient would get from [`resolve`]. The call is the
/// platform's own and is gated by the capability `setting-resolve-for`, which only a service
/// holds; no tenant principal reaches it. The recipient must be a person of the tenant: a member,
/// or a person a team names.
pub fn resolve_for(
    tenant: &TenantState,
    person: &PrincipalId,
    setting: &SettingId,
) -> Result<SettingResolution, DomainError> {
    resolve_for_with(tenant, person, setting, &PlatformLevels::new())
}

/// [`resolve_for`] with the platform level the control plane holds (see [`resolve_with`]).
pub fn resolve_for_with(
    tenant: &TenantState,
    person: &PrincipalId,
    setting: &SettingId,
    platform: &PlatformLevels,
) -> Result<SettingResolution, DomainError> {
    let known = tenant.members.contains_key(person)
        || tenant
            .schema
            .all_teams()
            .any(|team| team.members.contains(person));
    if !known {
        let message = format!("the tenant has no principal {person}");
        return Err(DomainError::new(ErrorKind::UnknownObject, message.clone())
            .with(vec![Check::fail("setting:recipient_is_a_person", message)]));
    }
    resolve_with(tenant, &Principal::User(person.clone()), setting, platform)
}

/// Where the value of a setting comes from, as it refines the audience class of a read that
/// depends on it (D-5049, SET-053).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Refinement {
    /// The value does not come from a team or a person: the class is as the policies derive it.
    Shared,
    /// The value comes from the override of a team: the class of that team.
    Team(TeamId),
    /// The value comes from the override of the person: a class of that person alone, whose
    /// copies are never shared.
    Person(PrincipalId),
}

/// What a view copy that reads a setting depends on (SET-050, SET-053): the revisions of the
/// setting at the platform and at the tenant, which are in the key of the copy so that a change
/// makes a new key, and the refinement of the class of its reader.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CopyDependency {
    pub setting: SettingId,
    pub platform_revision: u64,
    pub tenant_revision: u64,
    pub refinement: Refinement,
}

impl CopyDependency {
    /// The part of a copy key this dependency makes: the setting and both revisions.
    pub fn key_part(&self) -> String {
        format!(
            "{}@{}.{}",
            self.setting, self.platform_revision, self.tenant_revision
        )
    }
}

/// The dependencies of a read that uses `settings` for a reader, in setting order (SET-050,
/// SET-053, D-5049). A setting the tenant does not define is left out: nothing was resolved. The
/// copy store of a view calls this with the settings the sections of the view name
/// (`strata_substrate::project::view_settings`).
pub fn copy_dependencies(
    tenant: &TenantState,
    principal: &Principal,
    settings: &BTreeSet<SettingId>,
) -> Vec<CopyDependency> {
    settings
        .iter()
        .filter_map(|setting| {
            let def = definition(&tenant.schema, setting)?;
            let found = resolve_definition(tenant, def, principal);
            let refinement = match (found.source, principal, found.team) {
                (SettingSource::Person, Principal::User(person), _) => {
                    Refinement::Person(person.clone())
                }
                (SettingSource::Team, _, Some(team)) => Refinement::Team(team),
                _ => Refinement::Shared,
            };
            Some(CopyDependency {
                setting: setting.clone(),
                platform_revision: found.platform_revision,
                tenant_revision: found.tenant_revision,
                refinement,
            })
        })
        .collect()
}

/// Whether a copy with these dependencies may be shared between readers: it may unless a value
/// comes from the override of a person (SET-052).
pub fn shareable(dependencies: &[CopyDependency]) -> bool {
    dependencies
        .iter()
        .all(|dependency| !matches!(dependency.refinement, Refinement::Person(_)))
}

/// The problem with a setting a view argument names, or `None` when the tenant defines it
/// (SET-1540).
pub fn view_argument_problem(schema: &TenantSchema, setting: &SettingId) -> Option<String> {
    crate::definitions::setting_reference_problem(schema, setting)
}

fn unknown_setting(setting: &SettingId) -> DomainError {
    let message = format!("no setting {setting}");
    DomainError::new(ErrorKind::UnknownObject, message.clone())
        .with(vec![Check::fail("setting:is_defined", message)])
}

/// The problems of the setting definitions of a schema (SET-001 to SET-003, D-5043, D-5046): at
/// most the stated number; the key of the table is the id; a definition allows at least one
/// level; a lock is by the platform or the tenant; the type is one the kernel holds, and the
/// default is a value of it; the name is not that of a platform definition.
pub fn definition_problems(schema: &TenantSchema, problems: &mut Vec<String>) {
    definition_problems_against(schema, PLATFORM_SETTINGS, problems);
}

/// [`definition_problems`] against the given names of the platform's definitions (SET-003), for a
/// release that ships some and for a test that names one.
pub fn definition_problems_against(
    schema: &TenantSchema,
    platform_names: &[&str],
    problems: &mut Vec<String>,
) {
    let Some(defs) = &schema.settings else {
        return;
    };
    if defs.len() > limits::MAX_SETTINGS {
        problems.push(format!(
            "limit: {} settings is more than the maximum of {}",
            defs.len(),
            limits::MAX_SETTINGS
        ));
    }
    for (key, def) in defs {
        let at = format!("setting {}", def.id);
        if key != &def.id {
            problems.push(format!("{at}: held under the key {key}"));
        }
        if def.levels.is_empty() {
            problems.push(format!("{at}: a setting allows at least one level"));
        }
        if matches!(
            def.locked_by,
            Some(SettingLevel::Team | SettingLevel::Person)
        ) {
            problems.push(format!("{at}: a lock is by the platform or the tenant"));
        }
        if def.label.trim().is_empty() {
            problems.push(format!("{at}: a setting needs a label"));
        }
        if platform_names.contains(&def.id.as_str()) {
            problems.push(format!(
                "{at}: the name is that of a platform setting (SET-003)"
            ));
        }
        if let Some((_, problem)) = type_problem(def.r#type.as_str(), &def.default) {
            problems.push(format!("{at}: default: {problem} (SET-002)"));
        }
    }
}

/// Whether a schema change edits a setting definition.
pub fn is_setting_change(change: &SchemaChange) -> bool {
    matches!(
        change,
        SchemaChange::DefineSetting(_) | SchemaChange::RemoveSetting(_)
    )
}

/// Define or replace a setting definition in a working copy of a schema.
pub fn apply_define(schema: &mut TenantSchema, def: &SettingDef) -> String {
    schema
        .settings
        .get_or_insert_with(Default::default)
        .insert(def.id.clone(), def.clone());
    format!("define setting {}", def.id)
}

/// Remove a setting definition from a working copy of a schema. Removing one that is not defined
/// is refused. A table left empty is dropped, so a schema that defines nothing holds no table.
pub fn apply_remove(schema: &mut TenantSchema, id: &SettingId) -> Result<String, String> {
    let mut defs = schema.settings.take().unwrap_or_default();
    let found = defs.remove(id).is_some();
    schema.settings = (!defs.is_empty()).then_some(defs);
    if found {
        Ok(format!("remove setting {id}"))
    } else {
        Err(format!("no setting {id}"))
    }
}

/// What a change of setting definitions does to the overrides and the events, beyond the schema.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SchemaEffects {
    pub ops: Vec<DeltaOp>,
    pub events: Vec<EventBody>,
}

/// The overrides, the revisions and the events that a list of schema changes brings with it, or why
/// the changes are refused: the type of a definition does not change while an override of the
/// setting exists (SET-004); removing a definition removes its overrides, because an override
/// without a definition has no type and no level; a change of a definition raises the revision of
/// the setting at the tenant and is an event that names the setting, the tenant level and the
/// tenant (SET-051, D-5048).
pub fn schema_change_effects(
    tenant: &TenantState,
    changes: &[SchemaChange],
) -> Result<SchemaEffects, String> {
    let mut effects = SchemaEffects::default();
    let mut revisions: BTreeMap<SettingId, u64> = BTreeMap::new();
    let mut types: BTreeMap<SettingId, Option<String>> = BTreeMap::new();
    let mut emptied: BTreeSet<SettingId> = BTreeSet::new();
    let subject = tenant.id.as_str().to_string();
    for change in changes {
        let id = match change {
            SchemaChange::DefineSetting(def) => &def.id,
            SchemaChange::RemoveSetting(id) => id,
            _ => continue,
        };
        let before = match types.get(id) {
            Some(known) => known.clone(),
            None => definition(&tenant.schema, id).map(|def| def.r#type.as_str().to_string()),
        };
        match change {
            SchemaChange::DefineSetting(def) => {
                let after = def.r#type.as_str();
                let held = !emptied.contains(id) && overrides_of(tenant, id).next().is_some();
                if held && before.as_deref().is_some_and(|ty| ty != after) {
                    return Err(format!(
                        "setting {id}: the type changes from {} to {after} while overrides of it exist (SET-004)",
                        before.unwrap_or_default()
                    ));
                }
                types.insert(id.clone(), Some(after.to_string()));
            }
            _ => {
                if emptied.insert(id.clone()) {
                    for held in overrides_of(tenant, id) {
                        effects.ops.push(DeltaOp::DeleteSettingOverride {
                            setting: held.setting.clone(),
                            level: held.level,
                            subject: held.subject.clone(),
                        });
                    }
                }
                types.insert(id.clone(), None);
            }
        }
        let next = revisions
            .get(id)
            .copied()
            .unwrap_or_else(|| tenant_revision(tenant, id))
            + 1;
        revisions.insert(id.clone(), next);
        effects.events.push(EventBody::SettingChanged {
            setting: id.clone(),
            level: SettingLevel::Tenant,
            subject: subject.clone(),
        });
    }
    for (setting, revision) in revisions {
        effects
            .ops
            .push(DeltaOp::PutSettingRevision { setting, revision });
    }
    Ok(effects)
}

/// What an accepted write of an override changes and says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingDecision {
    pub ops: Vec<DeltaOp>,
    pub events: Vec<EventBody>,
    pub evidence: Vec<Check>,
    pub output: Output,
}

/// A refusal that names the check that failed after the checks that passed.
fn refuse(passed: &[Check], kind: ErrorKind, rule: &str, message: String) -> DomainError {
    let mut evidence = passed.to_vec();
    evidence.push(Check::fail(rule, message.clone()));
    DomainError::new(kind, message).with(evidence)
}

/// Decide the write of one override (D-5044, D-5045, SET-020 to SET-034): set it to `value`, or
/// remove it when there is none. `expected_revision` is the revision the caller read the override
/// at, zero for an override that does not exist.
///
/// Only a signed-in person writes. The tenant level needs the capability `setting-write-tenant`
/// and the team level `setting-write-team`, which the owner holds. The person level is the
/// caller's own, whatever role the caller holds: it takes no subject, so no other person's value
/// can be named (SET-034); the capability `setting-write-person`, which every signed-in person
/// holds, is checked at the gateway with the session. The platform level is written by the control
/// plane and is refused here.
///
/// Refused with `Unauthorized` for a caller who may not, `UnknownObject` for a setting that is
/// not defined, `InvalidIntent` for a level the definition does not allow or that a lock closes,
/// for the platform level and for a team that does not exist, `TypeMismatch` for a value of
/// another type, `VersionConflict` for another revision, and `LimitExceeded` past the most
/// overrides a tenant holds.
pub fn write_setting(
    tenant: &TenantState,
    ctx: &Context,
    setting: &SettingId,
    level: SettingLevel,
    team: Option<&TeamId>,
    value: Option<&Value>,
    expected_revision: u64,
) -> Result<SettingDecision, DomainError> {
    let mut evidence: Vec<Check> = Vec::new();

    let Principal::User(caller) = &ctx.principal else {
        return Err(refuse(
            &evidence,
            ErrorKind::Unauthorized,
            "setting:caller_is_a_person",
            "only a signed-in person writes a setting".into(),
        ));
    };
    evidence.push(Check::pass("setting:caller_is_a_person", caller.as_str()));

    let capability = match level {
        SettingLevel::Platform => {
            return Err(refuse(
                &evidence,
                ErrorKind::InvalidIntent,
                "setting:platform_level_is_the_control_planes",
                "the platform level of a setting is written by the control plane".into(),
            ))
        }
        SettingLevel::Tenant => Some(SETTING_WRITE_TENANT),
        SettingLevel::Team => Some(SETTING_WRITE_TEAM),
        SettingLevel::Person => None,
    };
    if let Some(name) = capability {
        let rule = format!("permission:capability:{name}");
        let wanted = BTreeSet::from([Grant::Capability(name.to_string())]);
        if !holds_any(tenant, &ctx.principal, &wanted) {
            return Err(refuse(
                &evidence,
                ErrorKind::Unauthorized,
                &rule,
                format!(
                    "a write at the {} level needs the capability {name}",
                    level.as_str()
                ),
            ));
        }
        evidence.push(Check::pass(rule, format!("holds {name}")));
    }

    let Some(def) = definition(&tenant.schema, setting) else {
        return Err(refuse(
            &evidence,
            ErrorKind::UnknownObject,
            "setting:is_defined",
            format!("no setting {setting}"),
        ));
    };
    evidence.push(Check::pass("setting:is_defined", setting.as_str()));

    if !is_open(def, level) {
        let why = if def.levels.contains(&level) {
            "a lock of the definition closes it"
        } else {
            "the definition does not allow it"
        };
        return Err(refuse(
            &evidence,
            ErrorKind::InvalidIntent,
            "setting:level_is_open",
            format!(
                "setting {setting} takes no override at the {} level: {why}",
                level.as_str()
            ),
        ));
    }
    evidence.push(Check::pass("setting:level_is_open", level.as_str()));

    let subject = match (level, team) {
        (SettingLevel::Tenant, None) => tenant.id.as_str().to_string(),
        (SettingLevel::Person, None) => caller.as_str().to_string(),
        (SettingLevel::Team, Some(team)) => {
            if tenant.schema.team(team).is_none() {
                return Err(refuse(
                    &evidence,
                    ErrorKind::InvalidIntent,
                    "setting:team_exists",
                    format!("no team {team}"),
                ));
            }
            team.as_str().to_string()
        }
        (SettingLevel::Team, None) => {
            return Err(refuse(
                &evidence,
                ErrorKind::InvalidIntent,
                "setting:team_named",
                "a write at the team level names the team".into(),
            ))
        }
        _ => {
            return Err(refuse(
                &evidence,
                ErrorKind::InvalidIntent,
                "setting:team_only_at_team_level",
                format!("a write at the {} level names no team", level.as_str()),
            ))
        }
    };

    if let Some(value) = value {
        if value.is_null() {
            return Err(refuse(
                &evidence,
                ErrorKind::TypeMismatch,
                "setting:value_has_the_type",
                "a null is no value of a setting; remove the override instead".into(),
            ));
        }
        if let Some((kind, problem)) = type_problem(def.r#type.as_str(), value) {
            return Err(refuse(
                &evidence,
                kind,
                "setting:value_has_the_type",
                format!("setting {setting}: {problem}"),
            ));
        }
        evidence.push(Check::pass(
            "setting:value_has_the_type",
            def.r#type.as_str(),
        ));
    }

    let current = override_of(tenant, setting, level, &subject);
    let held = current.map_or(0, |held| held.revision);
    if expected_revision != held {
        return Err(refuse(
            &evidence,
            ErrorKind::VersionConflict,
            "setting:revision_is_current",
            format!(
                "the override of {setting} is at revision {held}, expected {expected_revision}"
            ),
        ));
    }
    evidence.push(Check::pass("setting:revision_is_current", held.to_string()));

    if value.is_none() && current.is_none() {
        return Err(refuse(
            &evidence,
            ErrorKind::InvalidIntent,
            "setting:override_exists",
            format!("there is no override of {setting} to remove"),
        ));
    }
    let held_overrides = tenant
        .setting_values
        .as_deref()
        .map_or(0, |values| values.overrides.len());
    if value.is_some() && current.is_none() && held_overrides >= limits::MAX_SETTING_OVERRIDES {
        return Err(refuse(
            &evidence,
            ErrorKind::LimitExceeded,
            "setting:overrides_within_the_maximum",
            format!(
                "{held_overrides} overrides is the maximum of {}",
                limits::MAX_SETTING_OVERRIDES
            ),
        ));
    }

    let revision = tenant_revision(tenant, setting) + 1;
    let mut ops = Vec::with_capacity(2);
    match value {
        Some(value) => ops.push(DeltaOp::PutSettingOverride(SettingOverride {
            setting: setting.clone(),
            level,
            subject: subject.clone(),
            value: value.clone(),
            revision,
            set_by: caller.clone(),
            set_at: ctx.now,
        })),
        None => ops.push(DeltaOp::DeleteSettingOverride {
            setting: setting.clone(),
            level,
            subject: subject.clone(),
        }),
    }
    ops.push(DeltaOp::PutSettingRevision {
        setting: setting.clone(),
        revision,
    });
    Ok(SettingDecision {
        ops,
        events: vec![EventBody::SettingChanged {
            setting: setting.clone(),
            level,
            subject,
        }],
        evidence,
        output: Output::Setting {
            setting: setting.clone(),
            revision,
        },
    })
}

/// Apply one of the delta operations on overrides to a tenant state. Any other operation is left
/// alone. A state with no override and no revision holds no table of them, so it hashes as a
/// state that never had a setting.
pub fn apply_op(next: &mut TenantState, op: &DeltaOp) {
    if !matches!(
        op,
        DeltaOp::PutSettingOverride(_)
            | DeltaOp::DeleteSettingOverride { .. }
            | DeltaOp::PutSettingRevision { .. }
    ) {
        return;
    }
    let slot = next.setting_values.get_or_insert_with(|| {
        Arc::new(SettingValues {
            overrides: BTreeMap::new(),
            revisions: BTreeMap::new(),
        })
    });
    let values = Arc::make_mut(slot);
    match op {
        DeltaOp::PutSettingOverride(held) => {
            values.overrides.insert(
                override_key(&held.setting, held.level, &held.subject),
                held.clone(),
            );
        }
        DeltaOp::DeleteSettingOverride {
            setting,
            level,
            subject,
        } => {
            values
                .overrides
                .remove(&override_key(setting, *level, subject));
        }
        DeltaOp::PutSettingRevision { setting, revision } => {
            values.revisions.insert(setting.clone(), *revision);
        }
        _ => {}
    }
    if values.overrides.is_empty() && values.revisions.is_empty() {
        next.setting_values = None;
    }
}
