//! The routes of layered settings (D-5040 to D-5049): define a setting of the tenant, read the
//! settings for the caller, write an override at the tenant, team or person level, and resolve a
//! setting for a recipient on behalf of a platform service. Each is one kernel decision, as the
//! signed-in person; no rule of settings is written here (`strata_kernel::settings`).
//!
//! The platform level is the control plane's (CT-303): the operator writes it through
//! [`set_platform`], the control database keeps it, and every resolution here reads it from
//! [`PlatformCache`], which is at most [`PLATFORM_CACHE_MAX_AGE`] old because every tenant's
//! reads pass through it. A tenant's own write at that level is refused by the kernel.
//!
//! A value is written to no log: the handlers log nothing of it, an error names the setting and
//! never the value, and the event of a change carries none (SET-1542, SET-1543).

use crate::app::{ApiError, App, Shared};
use crate::auth::Identity;
use crate::http::as_caller;
use crate::ops::{header, run_intent};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde_json::{json, Value as Json_};
use std::fmt::Display;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use strata_kernel::settings::{self as kernel, PlatformLevel, PlatformLevels};
use strata_kernel::{
    ExistingData, Intent, Output, PrincipalId, SchemaChange, SchemaVersion, SettingDef, SettingId,
    SettingLevel, SettingResolution, TeamId, TenantId, Value,
};
use strata_protocol::api::{ErrorCode, HEADER_CORRELATION, HEADER_SESSION};
use strata_storage_api::{PlatformWrite, Store, StoreResult};

/// How old the platform level a resolution reads may be. Every tenant's every read of a setting
/// passes through it, so it is read from the control database once in this long, not once per
/// read; a write through this process drops it at once, and a write by another process is seen
/// within this time.
pub const PLATFORM_CACHE_MAX_AGE: Duration = Duration::from_secs(5);

/// The platform level of the settings, kept for at most a bounded age.
pub struct PlatformCache {
    max_age: Duration,
    held: Mutex<Option<(Instant, Arc<PlatformLevels>)>>,
}

impl Default for PlatformCache {
    fn default() -> Self {
        PlatformCache::new(PLATFORM_CACHE_MAX_AGE)
    }
}

impl PlatformCache {
    pub fn new(max_age: Duration) -> Self {
        PlatformCache {
            max_age,
            held: Mutex::new(None),
        }
    }

    /// The platform level as of at most the cache's age ago.
    pub fn levels(&self, store: &dyn Store) -> StoreResult<Arc<PlatformLevels>> {
        self.levels_at(store, Instant::now())
    }

    /// [`levels`](Self::levels) at a given instant, so a test can move time.
    pub fn levels_at(&self, store: &dyn Store, now: Instant) -> StoreResult<Arc<PlatformLevels>> {
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((read_at, levels)) = held.as_ref() {
            if now.saturating_duration_since(*read_at) < self.max_age {
                return Ok(levels.clone());
            }
        }
        let levels: PlatformLevels = store
            .platform_settings()?
            .into_iter()
            .map(|row| {
                (
                    row.setting,
                    PlatformLevel {
                        value: row.value,
                        revision: row.revision,
                    },
                )
            })
            .collect();
        let levels = Arc::new(levels);
        *held = Some((now, levels.clone()));
        Ok(levels)
    }

    /// Forget what is held, so the next read is from the store.
    pub fn invalidate(&self) {
        *self.held.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Invalid, message)
}

/// Parse a name the caller sent into a kernel identifier, or refuse the request as invalid.
fn named<T, E: Display>(what: &str, parsed: Result<T, E>) -> Result<T, ApiError> {
    parsed.map_err(|problem| invalid(format!("{what}: {problem}")))
}

/// A signed-in person: the contracts of settings are not open to a visitor.
fn signed_in(identity: &Identity) -> Result<(), ApiError> {
    if identity.anonymous() {
        Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in to do this.",
        ))
    } else {
        Ok(())
    }
}

/// A text member of a request body.
fn text_of<'a>(body: &'a Json_, name: &str) -> Result<&'a str, ApiError> {
    body.get(name)
        .and_then(Json_::as_str)
        .ok_or_else(|| invalid(format!("The request needs {name}.")))
}

/// The revision a write names it was read at, zero for an override that does not exist.
fn expected_of(body: &Json_, name: &str) -> Result<u64, ApiError> {
    body.get(name)
        .and_then(Json_::as_u64)
        .ok_or_else(|| invalid(format!("The request needs {name}, a whole number.")))
}

fn setting_of(body: &Json_) -> Result<SettingId, ApiError> {
    named("setting", SettingId::new(text_of(body, "setting")?))
}

/// A value of a request as the kernel's value, by its shape. A whole number is a timestamp when
/// the setting is of that type. The kernel decides whether it is a value of the setting's type.
fn value_of(ty: Option<&str>, json: &Json_) -> Result<Value, ApiError> {
    match json {
        Json_::Bool(flag) => Ok(Value::Bool(*flag)),
        Json_::Number(number) => {
            let whole = number
                .as_i64()
                .ok_or_else(|| invalid("A number of a setting is a whole number."))?;
            Ok(if ty == Some("timestamp") {
                Value::Time(whole)
            } else {
                Value::Int(whole)
            })
        }
        Json_::String(text) => Ok(Value::Str(text.clone())),
        _ => Err(invalid("A value is a flag, a whole number or text.")),
    }
}

/// A kernel value as JSON.
fn json_of(value: &Value) -> Json_ {
    match value {
        Value::Null => Json_::Null,
        Value::Bool(flag) => json!(flag),
        Value::Int(number) | Value::Time(number) => json!(number),
        Value::Str(text) => json!(text),
        Value::Ref(id) => json!(id.0),
        Value::List(items) => Json_::Array(items.iter().map(json_of).collect()),
    }
}

/// The answer of a read of a setting (`setting-resolution`).
fn resolution_json(resolution: &SettingResolution) -> Json_ {
    json!({
        "setting": resolution.setting.as_str(),
        "value": json_of(&resolution.value),
        "source": resolution.source.as_str(),
        "team": resolution.team.as_ref().map(TeamId::as_str),
        "platform_revision": resolution.platform_revision,
        "tenant_revision": resolution.tenant_revision,
    })
}

/// Define or change a setting of the tenant (CT-300), as a schema change that raises the schema
/// version. The body is `{ "definition": <setting-def>, "expected_version": <schema version> }`.
///
/// Implements: CT-300
pub async fn save_definition(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        signed_in(identity)?;
        let definition: SettingDef =
            serde_json::from_value(body.get("definition").cloned().unwrap_or(Json_::Null))
                .map_err(|problem| invalid(format!("definition: {problem}")))?;
        let expected = SchemaVersion(expected_of(&body, "expected_version")?);
        let intent = Intent::ChangeSchema {
            changes: vec![SchemaChange::DefineSetting(definition)],
            expected_version: Some(expected),
            existing: ExistingData::Reject,
        };
        let done = run_intent(app, &identity.context(app), &intent, identity.anonymous())?;
        match done.transition.output {
            Output::Schema { version } => Ok(json!({ "version": version.0 })),
            other => Err(ApiError::new(
                ErrorCode::Internal,
                format!("unexpected output {other:?}"),
            )),
        }
    })
    .await
}

/// Read one setting for the caller (CT-301): `{ "setting": <id> }`.
///
/// Implements: CT-301
pub async fn resolve(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        signed_in(identity)?;
        let setting = setting_of(&body)?;
        let tenant = header(app, identity)?;
        let platform = app.platform_settings.levels(app.store.as_ref())?;
        let resolution = kernel::resolve_with(&tenant, &identity.principal, &setting, &platform)
            .map_err(|error| ApiError::domain(error, false))?;
        Ok(resolution_json(&resolution))
    })
    .await
}

/// List the settings for the caller (CT-302): every setting of the tenant, resolved.
///
/// Implements: CT-302
pub async fn resolve_all(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        signed_in(identity)?;
        let tenant = header(app, identity)?;
        let platform = app.platform_settings.levels(app.store.as_ref())?;
        let settings: Vec<Json_> =
            kernel::resolve_all_with(&tenant, &identity.principal, &platform)
                .iter()
                .map(resolution_json)
                .collect();
        Ok(json!({ "settings": settings }))
    })
    .await
}

/// Write or remove an override at `level` as the caller. The body is `{ "setting": <id>,
/// "value": <json, absent to remove>, "expected_version": <revision read, 0 for none> }`, with
/// `"team"` at the team level.
fn write(
    app: &App,
    identity: &Identity,
    body: &Json_,
    level: SettingLevel,
) -> Result<Json_, ApiError> {
    signed_in(identity)?;
    let setting = setting_of(body)?;
    let team = match level {
        SettingLevel::Team => Some(named("team", TeamId::new(text_of(body, "team")?))?),
        _ => None,
    };
    let value = match body.get("value") {
        None | Some(Json_::Null) => None,
        Some(json) => {
            let tenant = header(app, identity)?;
            let ty = kernel::definition(&tenant.schema, &setting)
                .map(|def| def.r#type.as_str().to_string());
            Some(value_of(ty.as_deref(), json)?)
        }
    };
    let intent = Intent::WriteSetting {
        setting,
        level,
        team,
        value,
        expected_revision: expected_of(body, "expected_version")?,
    };
    let done = run_intent(app, &identity.context(app), &intent, identity.anonymous())?;
    match done.transition.output {
        Output::Setting { revision, .. } => Ok(json!({
            "revision": revision,
            "event_seq": done.transition.delta.event_seq,
        })),
        other => Err(ApiError::new(
            ErrorCode::Internal,
            format!("unexpected output {other:?}"),
        )),
    }
}

/// Set or remove the tenant level of a setting (CT-304).
///
/// Implements: CT-304
pub async fn set_tenant(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        write(app, identity, &body, SettingLevel::Tenant)
    })
    .await
}

/// Set or remove the team level of a setting (CT-305).
///
/// Implements: CT-305
pub async fn set_team(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        write(app, identity, &body, SettingLevel::Team)
    })
    .await
}

/// Set or remove the person level of a setting for the caller (CT-306). The body names no person:
/// the override is the caller's own, whatever role the caller holds (SET-034).
///
/// Implements: CT-306
pub async fn set_person(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    as_caller(&app, &headers, move |app, identity| {
        write(app, identity, &body, SettingLevel::Person)
    })
    .await
}

/// Resolve a setting for a recipient (CT-816), for the mail dispatcher and the notification
/// service: `{ "tenant": <id>, "principal": <id>, "setting": <id> }`. It is a call of a platform
/// service, which holds `setting-resolve-for` through the service credential and carries no
/// session. A request that carries a session is a tenant principal asking, possibly relayed by the
/// BFF, and is refused: no tenant principal resolves the setting of another person (SET-1541).
/// The value is returned to the caller and written to no log, copy or event (SET-1542).
///
/// Implements: CT-816
pub async fn resolve_for(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    if headers.contains_key(HEADER_SESSION) {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "The setting of a recipient is resolved by a platform service, not for a session.",
        ));
    }
    let tenant = named("tenant", TenantId::new(text_of(&body, "tenant")?))?;
    let person = named("principal", PrincipalId::new(text_of(&body, "principal")?))?;
    let setting = setting_of(&body)?;
    let site = tenant.as_str().to_string();
    app.blocking_for(&site, move |app| -> Result<Json_, ApiError> {
        let state = app
            .store
            .load(&tenant, &[])?
            .tenant
            .ok_or_else(|| ApiError::new(ErrorCode::NotFound, format!("no site {tenant}")))?;
        let platform = app.platform_settings.levels(app.store.as_ref())?;
        kernel::resolve_for_with(&state, &person, &setting, &platform)
            .map(|resolution| resolution_json(&resolution))
            .map_err(|error| ApiError::domain(error, false))
    })
    .await?
    .map(Json)
}

/// Set or remove the platform level of a setting (CT-303): `{ "setting": <id>, "value": <flag,
/// whole number or text; absent to remove>, "expected_version": <revision read, 0 for none> }`.
///
/// The backend has no operator identity yet, so the route is guarded exactly as the other
/// `/internal/platform/*` routes are: by the service credential every internal route needs, and by
/// nothing else, so it is for the operator's tools on the backend's own network and the BFF has no
/// route to it. A request that carries a session is refused besides, as a platform service's call
/// is (see [`resolve_for`]): no tenant principal writes the platform level. The write is an audit
/// entry of the platform (`platform.setting_set`, in the ring and the log, naming no tenant) that
/// holds the previous and the new value (SET-040, SET-042), which are the operator's own and are
/// no person's data.
///
/// Refused as `conflict` when the level is not at `expected_version`, naming the current one
/// (SET-025). The platform ships no definitions of its own (SET-005), so a name is not checked
/// against any: the value applies to every tenant whose definition of that name opens the
/// platform level and has a type the value is of.
///
/// Implements: CT-303
pub async fn set_platform(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Json_>,
) -> Result<Json<Json_>, ApiError> {
    if headers.contains_key(HEADER_SESSION) {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "The platform level is written by a platform operator, not for a session.",
        ));
    }
    let correlation = headers
        .get(HEADER_CORRELATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let setting = setting_of(&body)?;
    let value = match body.get("value") {
        None | Some(Json_::Null) => None,
        Some(json) => {
            let value = value_of(None, json)?;
            if let Some(problem) = kernel::platform_value_problem(&value) {
                return Err(invalid(format!("value: {problem}")));
            }
            Some(value)
        }
    };
    let expected = expected_of(&body, "expected_version")?;
    let written = {
        let setting = setting.clone();
        let value = value.clone();
        app.blocking(move |app| {
            app.store
                .platform_setting_write(&setting, value.as_ref(), expected, app.now())
        })
        .await??
    };
    match written {
        PlatformWrite::Conflict { current } => Err(ApiError::new(
            ErrorCode::Conflict,
            format!(
                "The platform level of {setting} was changed since revision {expected}; it is now at revision {current}."
            ),
        )),
        PlatformWrite::Written { now, previous } => {
            app.platform_settings.invalidate();
            app.audit(
                "platform.setting_set",
                &correlation,
                json!({
                    "setting": setting.as_str(),
                    "level": "platform",
                    "previous": previous.as_ref().map(json_of),
                    "value": now.value.as_ref().map(json_of),
                    "revision": now.revision,
                }),
            );
            Ok(Json(json!({ "revision": now.revision })))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_kernel::Timestamp;
    use strata_storage_api::memory::MemoryStore;

    fn id(name: &str) -> SettingId {
        SettingId::new(name).unwrap()
    }

    #[test]
    fn the_platform_level_is_read_from_the_store_at_most_once_in_its_age_and_dropped_on_demand() {
        let store = MemoryStore::new();
        let cache = PlatformCache::new(Duration::from_secs(5));
        let start = Instant::now();
        assert!(cache.levels_at(&store, start).unwrap().is_empty());

        store
            .platform_setting_write(&id("page_size"), Some(&Value::Int(50)), 0, Timestamp(1))
            .unwrap();
        // Within the age the write is not seen; past it, it is.
        let soon = start + Duration::from_secs(4);
        assert!(cache.levels_at(&store, soon).unwrap().is_empty());
        let later = start + Duration::from_secs(5);
        let levels = cache.levels_at(&store, later).unwrap();
        assert_eq!(levels[&id("page_size")].revision, 1);
        assert_eq!(levels[&id("page_size")].value, Some(Value::Int(50)));

        // A write made through this process drops it at once.
        store
            .platform_setting_write(&id("page_size"), None, 1, Timestamp(2))
            .unwrap();
        assert_eq!(
            cache.levels_at(&store, later).unwrap()[&id("page_size")].revision,
            1
        );
        cache.invalidate();
        let levels = cache.levels_at(&store, later).unwrap();
        assert_eq!(levels[&id("page_size")].revision, 2);
        assert_eq!(levels[&id("page_size")].value, None);
    }

    #[test]
    fn a_write_at_a_stale_revision_changes_nothing_and_names_the_current_one() {
        let store = MemoryStore::new();
        let name = id("page_size");
        let first = store
            .platform_setting_write(&name, Some(&Value::Int(1)), 0, Timestamp(1))
            .unwrap();
        assert!(matches!(first, PlatformWrite::Written { .. }));
        let stale = store
            .platform_setting_write(&name, Some(&Value::Int(2)), 0, Timestamp(2))
            .unwrap();
        assert_eq!(stale, PlatformWrite::Conflict { current: 1 });
        assert_eq!(
            store.platform_settings().unwrap()[0].value,
            Some(Value::Int(1))
        );
    }
}
