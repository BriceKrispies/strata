//! The routes of layered settings (D-5040 to D-5049): define a setting of the tenant, read the
//! settings for the caller, write an override at the tenant, team or person level, and resolve a
//! setting for a recipient on behalf of a platform service. Each is one kernel decision, as the
//! signed-in person; no rule of settings is written here (`strata_kernel::settings`).
//!
//! The platform level is written by the control plane (CT-303) and is not served here.
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
use strata_kernel::settings as kernel;
use strata_kernel::{
    ExistingData, Intent, Output, PrincipalId, SchemaChange, SchemaVersion, SettingDef, SettingId,
    SettingLevel, SettingResolution, TeamId, TenantId, Value,
};
use strata_protocol::api::{ErrorCode, HEADER_SESSION};

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
        let resolution = kernel::resolve(&tenant, &identity.principal, &setting)
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
        let settings: Vec<Json_> = kernel::resolve_all(&tenant, &identity.principal)
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
        kernel::resolve_for(&state, &person, &setting)
            .map(|resolution| resolution_json(&resolution))
            .map_err(|error| ApiError::domain(error, false))
    })
    .await?
    .map(Json)
}
