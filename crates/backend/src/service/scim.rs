//! Provisioning of people from an identity provider, by SCIM 2.0.
//!
//! The provider presents a *provisioning token* the realm's owner made for it. The token is
//! kept as a hash, belongs to one tenant, and opens this endpoint and nothing else (FED-259): no
//! other call looks for it, so it is no session and no client secret, and a token of another
//! tenant finds no row here.
//!
//! The operations are the cases of the provisioning table (FED-014): create, replace, patch,
//! delete and filter a user, and change a group's members. A person made here has an account no
//! password opens, and signs in by the provider. Deactivating or deleting a person ends their
//! sessions and every token issued in their name (FED-015). The one way this endpoint changes
//! roles is a group the realm mapped to a role, with the realm's own say (`set_group_role`): a
//! user resource carries no role, and a group the realm did not map is refused (FED-260).
//!
//! The backend gets the request as an envelope (`method`, `path` below `/scim/v2`, `query`,
//! `authorization`, `body`) and answers with the status and the body the gateway sends on.
//!
//! Implements: FED-014, FED-015, FED-259, FED-260

use super::{get, list, owner_context, put, require_owner, text};
use crate::app::{ApiError, App, LOG};
use crate::auth::{random_hex, token_hash, Caller, Identity};
use crate::signin::{normalise_login, start_rules, throttled};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use std::collections::BTreeSet;
use strata_kernel::{owner_role, public_role, Intent, PrincipalId, RoleId, TenantId};
use strata_protocol::api::ErrorCode;
use strata_storage_api::{Account, FederatedIdentity, ThrottleSettle, ThrottleVerdict};

/// Deactivated people, by principal.
pub const DISABLED: &str = "disabled";
const USER: &str = "scim-user";
const GROUP_ROLE: &str = "group-role";
const GROUP_MEMBERS: &str = "group-members";
const TOKEN: &str = "prov-token";
/// The name of the provider that the identities made here are tied under.
const PROVIDER: &str = "scim";

const SCHEMA_USER: &str = "urn:ietf:params:scim:schemas:core:2.0:User";
const SCHEMA_GROUP: &str = "urn:ietf:params:scim:schemas:core:2.0:Group";
const SCHEMA_LIST: &str = "urn:ietf:params:scim:api:messages:2.0:ListResponse";
const SCHEMA_ERROR: &str = "urn:ietf:params:scim:api:messages:2.0:Error";
const MAX_LISTED: usize = 200;
const NAME_CHARS: usize = 80;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProvisioningToken {
    id: String,
    created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ScimUser {
    principal: String,
    user_name: String,
    external_id: Option<String>,
    display_name: String,
    active: bool,
    created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct GroupRole {
    group: String,
    role: String,
}

/// A status and the body that goes with it.
type Answer = (u16, Json);

fn error(status: u16, detail: &str, scim_type: Option<&str>) -> Answer {
    let mut body =
        json!({ "schemas": [SCHEMA_ERROR], "status": status.to_string(), "detail": detail });
    if let Some(kind) = scim_type {
        body["scimType"] = json!(kind);
    }
    (status, body)
}

// ---------------------------------------------------------------------------------------------
// The provisioning token

/// Make a provisioning token for the realm. It is in the reply and nowhere else.
pub fn create_token(app: &App, identity: &Identity) -> Result<Json, ApiError> {
    require_owner(app, identity)?;
    let token = format!("scimt_{}", random_hex(32)?);
    let record = ProvisioningToken {
        id: format!("pt_{}", random_hex(6)?),
        created_at: app.now().0,
    };
    put(
        app,
        &identity.tenant,
        TOKEN,
        &token_hash(&token),
        &record,
        true,
    )?;
    app.audit(
        "provisioning_token.created",
        &identity.correlation,
        json!({ "tenant": identity.tenant.as_str(), "id": record.id }),
    );
    Ok(json!({ "id": record.id, "token": token }))
}

/// Withdraw a provisioning token: its next request is refused.
pub fn revoke_token(app: &App, identity: &Identity, body: &Json) -> Result<Json, ApiError> {
    require_owner(app, identity)?;
    let id = text(body, "id", 64)?;
    for (key, value) in app.store.doc_list(&identity.tenant, TOKEN)? {
        let record: ProvisioningToken = serde_json::from_str(&value)
            .map_err(|e| ApiError::hidden(ErrorCode::Internal, e.to_string()))?;
        if record.id == id {
            app.store.doc_remove(&identity.tenant, TOKEN, &key)?;
            app.audit(
                "provisioning_token.revoked",
                &identity.correlation,
                json!({ "tenant": identity.tenant.as_str(), "id": id }),
            );
            return Ok(json!({ "id": id, "revoked": true }));
        }
    }
    Err(ApiError::new(
        ErrorCode::NotFound,
        "No such provisioning token.",
    ))
}

// ---------------------------------------------------------------------------------------------
// The realm's group mapping

fn valid_group(group: &str) -> bool {
    !group.is_empty()
        && group.len() <= 64
        && group
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'))
}

/// The realm declares that a group of the provider carries a role. The owner role is not one a
/// provider can hand out (FED-260).
pub fn set_group_role(app: &App, identity: &Identity, body: &Json) -> Result<Json, ApiError> {
    require_owner(app, identity)?;
    let group = text(body, "group", 64)?;
    if !valid_group(group) {
        return Err(ApiError::new(
            ErrorCode::Invalid,
            "A group is letters, digits and . _ : - only.",
        ));
    }
    let role = text(body, "role", 64)?;
    let id = RoleId::new(role.to_string())
        .map_err(|_| ApiError::new(ErrorCode::Invalid, "That is not a role name."))?;
    let tenant = crate::ops::header(app, identity)?;
    if id == owner_role() || id == public_role() || !tenant.schema.roles.contains_key(&id) {
        return Err(ApiError::new(
            ErrorCode::Invalid,
            "A group can be mapped to a role of the site other than owner and public.",
        ));
    }
    put(
        app,
        &identity.tenant,
        GROUP_ROLE,
        group,
        &GroupRole {
            group: group.to_string(),
            role: role.to_string(),
        },
        false,
    )?;
    app.audit(
        "group_mapping.set",
        &identity.correlation,
        json!({ "tenant": identity.tenant.as_str(), "group": group, "role": role }),
    );
    Ok(json!({ "group": group, "role": role }))
}

/// The realm no longer maps a group: the roles it gave are taken back.
pub fn remove_group_role(app: &App, identity: &Identity, body: &Json) -> Result<Json, ApiError> {
    require_owner(app, identity)?;
    let group = text(body, "group", 64)?;
    let Some(mapping) = get::<GroupRole>(app, &identity.tenant, GROUP_ROLE, group)? else {
        return Err(ApiError::new(ErrorCode::NotFound, "No such mapping."));
    };
    let members = members_of(app, &identity.tenant, group)?;
    app.store.doc_remove(&identity.tenant, GROUP_ROLE, group)?;
    for member in &members {
        if let Ok(principal) = PrincipalId::new(member.clone()) {
            take_role(
                app,
                &identity.tenant,
                &identity.correlation,
                &principal,
                &mapping.role,
            )?;
        }
    }
    app.store
        .doc_remove(&identity.tenant, GROUP_MEMBERS, group)?;
    Ok(json!({ "group": group, "removed": true }))
}

pub fn group_roles(app: &App, identity: &Identity) -> Result<Json, ApiError> {
    require_owner(app, identity)?;
    let mappings: Vec<GroupRole> = list(app, &identity.tenant, GROUP_ROLE)?;
    Ok(
        json!({ "groups": mappings.iter().map(|m| json!({ "group": m.group, "role": m.role })).collect::<Vec<_>>() }),
    )
}

fn members_of(app: &App, tenant: &TenantId, group: &str) -> Result<BTreeSet<String>, ApiError> {
    Ok(get::<BTreeSet<String>>(app, tenant, GROUP_MEMBERS, group)?.unwrap_or_default())
}

/// Give a role, as the platform acting for the tenant's owner.
fn give_role(
    app: &App,
    tenant: &TenantId,
    correlation: &str,
    principal: &PrincipalId,
    role: &str,
) -> Result<(), ApiError> {
    let role = RoleId::new(role.to_string())
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, e.to_string()))?;
    let ctx = owner_context(app, tenant, correlation)?;
    crate::ops::run_intent(
        app,
        &ctx,
        &Intent::AssignRole {
            principal: principal.clone(),
            role,
        },
        false,
    )?;
    Ok(())
}

/// Take a role back, unless another group the realm mapped to it still holds the person.
fn take_role(
    app: &App,
    tenant: &TenantId,
    correlation: &str,
    principal: &PrincipalId,
    role: &str,
) -> Result<(), ApiError> {
    let mappings: Vec<GroupRole> = list(app, tenant, GROUP_ROLE)?;
    for mapping in mappings.iter().filter(|m| m.role == role) {
        if members_of(app, tenant, &mapping.group)?.contains(principal.as_str()) {
            return Ok(());
        }
    }
    let role = RoleId::new(role.to_string())
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, e.to_string()))?;
    let ctx = owner_context(app, tenant, correlation)?;
    crate::ops::run_intent(
        app,
        &ctx,
        &Intent::RevokeRole {
            principal: principal.clone(),
            role,
        },
        false,
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// People

fn render(user: &ScimUser) -> Json {
    let mut body = json!({
        "schemas": [SCHEMA_USER],
        "id": user.principal,
        "userName": user.user_name,
        "displayName": user.display_name,
        "active": user.active,
    });
    if let Some(external) = &user.external_id {
        body["externalId"] = json!(external);
    }
    if user.user_name.contains('@') {
        body["emails"] = json!([{ "value": user.user_name, "primary": true }]);
    }
    body
}

/// End every live session of a person (FED-015).
pub fn end_sessions(
    app: &App,
    tenant: &TenantId,
    principal: &PrincipalId,
    correlation: &str,
) -> Result<usize, ApiError> {
    let sessions: Vec<_> = app
        .store
        .live_sessions_of(tenant, Some(principal), app.now())?;
    let count = sessions.len();
    for session in sessions {
        crate::auth::revoke(app, session, correlation)?;
    }
    Ok(count)
}

fn user_of(app: &App, tenant: &TenantId, id: &str) -> Result<Option<ScimUser>, ApiError> {
    if PrincipalId::new(id.to_string()).is_err() {
        return Ok(None);
    }
    get(app, tenant, USER, id)
}

/// A flag as a provider writes it: a boolean, or the text of one.
fn flag(value: &Json) -> Option<bool> {
    match value {
        Json::Bool(b) => Some(*b),
        Json::String(s) if s.eq_ignore_ascii_case("true") => Some(true),
        Json::String(s) if s.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

fn clean_name(value: &Json) -> Option<String> {
    let name: String = value.as_str()?.trim().chars().take(NAME_CHARS).collect();
    (!name.is_empty()).then_some(name)
}

/// Deactivate or reactivate. Deactivation ends the person's sessions and, because the person is
/// no subject of a token any more, the tokens made in their name (FED-015).
fn set_active(
    app: &App,
    tenant: &TenantId,
    correlation: &str,
    user: &mut ScimUser,
    active: bool,
) -> Result<(), ApiError> {
    if user.active == active {
        return Ok(());
    }
    let principal = PrincipalId::new(user.principal.clone())
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, e.to_string()))?;
    if active {
        app.store.doc_remove(tenant, DISABLED, principal.as_str())?;
    } else {
        put(
            app,
            tenant,
            DISABLED,
            principal.as_str(),
            &json!({ "at": app.now().0 }),
            false,
        )?;
        end_sessions(app, tenant, &principal, correlation)?;
    }
    user.active = active;
    app.audit(
        if active {
            "scim.reactivated"
        } else {
            "scim.deactivated"
        },
        correlation,
        json!({ "tenant": tenant.as_str(), "principal": principal.as_str() }),
    );
    Ok(())
}

fn create_user(app: &App, caller: &Caller, body: &Json) -> Result<Answer, ApiError> {
    let Some(user_name) = body.get("userName").and_then(Json::as_str) else {
        return Ok(error(400, "userName is required.", Some("invalidValue")));
    };
    let login = normalise_login(user_name);
    if login.is_empty() || login.chars().count() > 254 || login.chars().any(char::is_control) {
        return Ok(error(400, "userName is not usable.", Some("invalidValue")));
    }
    let external_id = match body.get("externalId") {
        None | Some(Json::Null) => None,
        Some(Json::String(id)) if !id.is_empty() && id.chars().count() <= 255 => Some(id.clone()),
        Some(_) => {
            return Ok(error(
                400,
                "externalId is not usable.",
                Some("invalidValue"),
            ))
        }
    };
    let active = match body.get("active") {
        None | Some(Json::Null) => true,
        Some(value) => match flag(value) {
            Some(active) => active,
            None => return Ok(error(400, "active is a boolean.", Some("invalidValue"))),
        },
    };
    if app.store.account(&caller.site, &login)?.is_some() {
        return Ok(error(409, "That userName is in use.", Some("uniqueness")));
    }
    if let Some(external) = &external_id {
        if app
            .store
            .identity(&caller.site, PROVIDER, external)?
            .is_some()
        {
            return Ok(error(409, "That externalId is in use.", Some("uniqueness")));
        }
    }
    let principal = PrincipalId::new(format!("user:scim-{}", random_hex(12)?))
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, e.to_string()))?;
    let user = ScimUser {
        principal: principal.as_str().to_string(),
        user_name: user_name.trim().to_string(),
        external_id: external_id.clone(),
        display_name: body
            .get("displayName")
            .and_then(clean_name)
            .unwrap_or_else(|| user_name.trim().chars().take(NAME_CHARS).collect()),
        active: true,
        created_at: app.now().0,
    };
    put(app, &caller.site, USER, principal.as_str(), &user, true)?;
    app.store.put_account(&Account {
        tenant: caller.site.clone(),
        login,
        principal: principal.clone(),
        display_name: user.display_name.clone(),
        // Not a password hash: no password opens this login.
        password_hash: String::new(),
        verified_email: None,
    })?;
    if let Some(external) = external_id {
        app.store.put_identity(&FederatedIdentity {
            tenant: caller.site.clone(),
            provider: PROVIDER.to_string(),
            subject: external,
            principal: principal.clone(),
            created_at: app.now(),
        })?;
    }
    let mut user = user;
    if !active {
        set_active(app, &caller.site, &caller.correlation, &mut user, false)?;
    }
    put(app, &caller.site, USER, principal.as_str(), &user, false)?;
    app.audit(
        "scim.created",
        &caller.correlation,
        json!({ "tenant": caller.site.as_str(), "principal": principal.as_str() }),
    );
    Ok((201, render(&user)))
}

/// `userName eq "value"`, the one filter the table lists.
fn user_name_filter(filter: &str) -> Option<String> {
    let rest = filter.trim().get(..)?;
    let (attribute, rest) = rest.split_once(' ')?;
    let (operator, value) = rest.trim_start().split_once(' ')?;
    if !attribute.eq_ignore_ascii_case("username") || !operator.eq_ignore_ascii_case("eq") {
        return None;
    }
    let value = value.trim();
    Some(value.strip_prefix('"')?.strip_suffix('"')?.to_string())
}

fn list_users(app: &App, caller: &Caller, filter: Option<&str>) -> Result<Answer, ApiError> {
    let wanted = match filter {
        None => None,
        Some(filter) => match user_name_filter(filter) {
            Some(name) => Some(normalise_login(&name)),
            None => {
                return Ok(error(
                    400,
                    "Only `userName eq \"…\"` is supported.",
                    Some("invalidFilter"),
                ))
            }
        },
    };
    let users: Vec<ScimUser> = list(app, &caller.site, USER)?;
    let found: Vec<Json> = users
        .iter()
        .filter(|u| {
            wanted
                .as_ref()
                .is_none_or(|name| normalise_login(&u.user_name) == *name)
        })
        .take(MAX_LISTED)
        .map(render)
        .collect();
    Ok((
        200,
        json!({
            "schemas": [SCHEMA_LIST],
            "totalResults": found.len(),
            "startIndex": 1,
            "itemsPerPage": found.len(),
            "Resources": found,
        }),
    ))
}

fn replace_user(app: &App, caller: &Caller, id: &str, body: &Json) -> Result<Answer, ApiError> {
    let Some(mut user) = user_of(app, &caller.site, id)? else {
        return Ok(error(404, "No such user.", None));
    };
    if let Some(name) = body.get("userName").and_then(Json::as_str) {
        if normalise_login(name) != normalise_login(&user.user_name) {
            return Ok(error(
                400,
                "userName cannot be changed.",
                Some("mutability"),
            ));
        }
    }
    if let Some(name) = body.get("displayName").and_then(clean_name) {
        user.display_name = name;
    }
    if let Some(Json::String(external)) = body.get("externalId") {
        if user.external_id.as_deref() != Some(external.as_str()) {
            if external.is_empty() || external.chars().count() > 255 {
                return Ok(error(
                    400,
                    "externalId is not usable.",
                    Some("invalidValue"),
                ));
            }
            if app
                .store
                .identity(&caller.site, PROVIDER, external)?
                .is_some()
            {
                return Ok(error(409, "That externalId is in use.", Some("uniqueness")));
            }
            let principal = PrincipalId::new(user.principal.clone())
                .map_err(|e| ApiError::hidden(ErrorCode::Internal, e.to_string()))?;
            app.store.put_identity(&FederatedIdentity {
                tenant: caller.site.clone(),
                provider: PROVIDER.to_string(),
                subject: external.clone(),
                principal,
                created_at: app.now(),
            })?;
            user.external_id = Some(external.clone());
        }
    }
    if let Some(value) = body.get("active") {
        let Some(active) = flag(value) else {
            return Ok(error(400, "active is a boolean.", Some("invalidValue")));
        };
        set_active(app, &caller.site, &caller.correlation, &mut user, active)?;
    }
    save_user(app, &caller.site, &user)?;
    Ok((200, render(&user)))
}

/// Keep a changed person, and the name on their login.
fn save_user(app: &App, tenant: &TenantId, user: &ScimUser) -> Result<(), ApiError> {
    put(app, tenant, USER, &user.principal, user, false)?;
    let principal = PrincipalId::new(user.principal.clone())
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, e.to_string()))?;
    if let Some(mut account) = app.store.account_of(tenant, &principal)? {
        if account.display_name != user.display_name {
            account.display_name = user.display_name.clone();
            app.store.put_account(&account)?;
        }
    }
    Ok(())
}

fn patch_user(app: &App, caller: &Caller, id: &str, body: &Json) -> Result<Answer, ApiError> {
    let Some(mut user) = user_of(app, &caller.site, id)? else {
        return Ok(error(404, "No such user.", None));
    };
    let Some(operations) = body.get("Operations").and_then(Json::as_array) else {
        return Ok(error(400, "Operations is required.", Some("invalidSyntax")));
    };
    // The changes are worked out first and applied after, so a patch with one bad operation
    // changes nothing.
    let mut active = None;
    let mut display_name = None;
    for operation in operations {
        let op = operation
            .get("op")
            .and_then(Json::as_str)
            .map(str::to_lowercase);
        if !matches!(op.as_deref(), Some("replace" | "add")) {
            return Ok(error(
                400,
                "Only add and replace are supported.",
                Some("invalidSyntax"),
            ));
        }
        let value = operation.get("value");
        let pairs: Vec<(String, &Json)> =
            match (operation.get("path").and_then(Json::as_str), value) {
                (Some(path), Some(value)) => vec![(path.to_string(), value)],
                (None, Some(Json::Object(map))) => {
                    map.iter().map(|(k, v)| (k.clone(), v)).collect()
                }
                _ => {
                    return Ok(error(
                        400,
                        "An operation needs a value.",
                        Some("invalidSyntax"),
                    ))
                }
            };
        for (path, value) in pairs {
            match path.as_str() {
                "active" => match flag(value) {
                    Some(flag) => active = Some(flag),
                    None => return Ok(error(400, "active is a boolean.", Some("invalidValue"))),
                },
                "displayName" => match clean_name(value) {
                    Some(name) => display_name = Some(name),
                    None => {
                        return Ok(error(
                            400,
                            "displayName is not usable.",
                            Some("invalidValue"),
                        ))
                    }
                },
                _ => {
                    return Ok(error(
                        400,
                        "That attribute cannot be patched.",
                        Some("noTarget"),
                    ))
                }
            }
        }
    }
    if let Some(name) = display_name {
        user.display_name = name;
    }
    if let Some(active) = active {
        set_active(app, &caller.site, &caller.correlation, &mut user, active)?;
    }
    save_user(app, &caller.site, &user)?;
    Ok((200, render(&user)))
}

fn delete_user(app: &App, caller: &Caller, id: &str) -> Result<Answer, ApiError> {
    let Some(user) = user_of(app, &caller.site, id)? else {
        return Ok(error(404, "No such user.", None));
    };
    let principal = PrincipalId::new(user.principal.clone())
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, e.to_string()))?;
    // Deactivated first, so that nothing issued in their name is accepted from here on.
    put(
        app,
        &caller.site,
        DISABLED,
        principal.as_str(),
        &json!({ "at": app.now().0 }),
        false,
    )?;
    end_sessions(app, &caller.site, &principal, &caller.correlation)?;
    // Out of every group, which takes back the roles the groups gave.
    let mappings: Vec<GroupRole> = list(app, &caller.site, GROUP_ROLE)?;
    for mapping in &mappings {
        let mut members = members_of(app, &caller.site, &mapping.group)?;
        if members.remove(principal.as_str()) {
            put(
                app,
                &caller.site,
                GROUP_MEMBERS,
                &mapping.group,
                &members,
                false,
            )?;
            take_role(
                app,
                &caller.site,
                &caller.correlation,
                &principal,
                &mapping.role,
            )?;
        }
    }
    if let Some(account) = app.store.account_of(&caller.site, &principal)? {
        app.store.remove_account(&caller.site, &account.login)?;
    }
    app.store
        .doc_remove(&caller.site, USER, principal.as_str())?;
    app.audit(
        "scim.deleted",
        &caller.correlation,
        json!({ "tenant": caller.site.as_str(), "principal": principal.as_str() }),
    );
    Ok((204, Json::Null))
}

// ---------------------------------------------------------------------------------------------
// Groups

fn render_group(group: &str, members: &BTreeSet<String>) -> Json {
    json!({
        "schemas": [SCHEMA_GROUP],
        "id": group,
        "displayName": group,
        "members": members.iter().map(|m| json!({ "value": m })).collect::<Vec<_>>(),
    })
}

/// The members a group operation names: `[{"value": id}]`, or a list of identifiers.
fn named_members(value: &Json) -> Option<Vec<String>> {
    let items = match value {
        Json::Array(items) => items.clone(),
        Json::Object(map) => map.get("members").and_then(Json::as_array)?.clone(),
        _ => return None,
    };
    items
        .iter()
        .map(|item| match item {
            Json::String(id) => Some(id.clone()),
            other => other
                .get("value")
                .and_then(Json::as_str)
                .map(str::to_string),
        })
        .collect()
}

/// `members[value eq "id"]`.
fn member_path_target(path: &str) -> Option<String> {
    let inner = path.strip_prefix("members[")?.strip_suffix(']')?;
    user_name_filter(&inner.replacen("value", "userName", 1))
}

fn get_group(app: &App, caller: &Caller, group: &str) -> Result<Answer, ApiError> {
    if get::<GroupRole>(app, &caller.site, GROUP_ROLE, group)?.is_none() {
        return Ok(error(404, "No such group.", None));
    }
    Ok((
        200,
        render_group(group, &members_of(app, &caller.site, group)?),
    ))
}

/// Change the members of a group, and with them the roles the realm mapped it to (FED-260).
fn patch_group(app: &App, caller: &Caller, group: &str, body: &Json) -> Result<Answer, ApiError> {
    let Some(mapping) = get::<GroupRole>(app, &caller.site, GROUP_ROLE, group)? else {
        return Ok(error(
            400,
            "The realm declares no mapping for that group.",
            Some("invalidValue"),
        ));
    };
    let Some(operations) = body.get("Operations").and_then(Json::as_array) else {
        return Ok(error(400, "Operations is required.", Some("invalidSyntax")));
    };
    let before = members_of(app, &caller.site, group)?;
    let mut members = before.clone();
    for operation in operations {
        let op = operation
            .get("op")
            .and_then(Json::as_str)
            .map(str::to_lowercase)
            .unwrap_or_default();
        let path = operation.get("path").and_then(Json::as_str);
        let value = operation.get("value");
        match (op.as_str(), path) {
            ("remove", Some(path)) if member_path_target(path).is_some() => {
                members.remove(&member_path_target(path).unwrap_or_default());
            }
            ("remove", Some("members")) => match value.and_then(named_members) {
                Some(named) => {
                    for id in named {
                        members.remove(&id);
                    }
                }
                None => members.clear(),
            },
            ("add" | "replace", Some("members") | None) => {
                let Some(named) = value.and_then(named_members) else {
                    return Ok(error(400, "members is a list.", Some("invalidValue")));
                };
                if op == "replace" {
                    members.clear();
                }
                members.extend(named);
            }
            _ => {
                return Ok(error(
                    400,
                    "That operation is not supported.",
                    Some("invalidSyntax"),
                ))
            }
        }
    }
    for id in members.difference(&before) {
        if user_of(app, &caller.site, id)?.is_none() {
            return Ok(error(
                400,
                "A member is not a person of this realm.",
                Some("invalidValue"),
            ));
        }
    }
    let added: Vec<String> = members.difference(&before).cloned().collect();
    let removed: Vec<String> = before.difference(&members).cloned().collect();
    // The members are kept first: `take_role` looks at what the groups hold.
    put(app, &caller.site, GROUP_MEMBERS, group, &members, false)?;
    for id in &added {
        if let Ok(principal) = PrincipalId::new(id.clone()) {
            give_role(
                app,
                &caller.site,
                &caller.correlation,
                &principal,
                &mapping.role,
            )?;
        }
    }
    for id in &removed {
        if let Ok(principal) = PrincipalId::new(id.clone()) {
            take_role(
                app,
                &caller.site,
                &caller.correlation,
                &principal,
                &mapping.role,
            )?;
        }
    }
    app.audit(
        "scim.group_changed",
        &caller.correlation,
        json!({ "tenant": caller.site.as_str(), "group": group, "added": added.len(), "removed": removed.len() }),
    );
    Ok((200, render_group(group, &members)))
}

// ---------------------------------------------------------------------------------------------
// The endpoint

/// One SCIM request. The provisioning token is checked first and is the only thing this reads
/// it for (FED-259); requests are counted by source and realm, and a right token gives its slot
/// back, so a provider provisioning many people is not limited and a guesser is.
///
/// Implements: FED-014, FED-259
pub fn handle(app: &App, caller: &Caller, request: &Json) -> Result<Json, ApiError> {
    let limits = start_rules(app, "scim", &caller.source);
    if let ThrottleVerdict::Refused { .. } =
        app.store
            .throttle_reserve(&caller.site, &limits, app.now())?
    {
        return Err(throttled());
    }
    let presented = request
        .get("authorization")
        .and_then(Json::as_str)
        .and_then(|header| header.strip_prefix("Bearer "))
        .filter(|token| token.len() <= 256);
    let known = match presented {
        Some(token) => app
            .store
            .doc_get(&caller.site, TOKEN, &token_hash(token))?
            .is_some(),
        None => false,
    };
    if !known {
        LOG.warn(
            "scim.refused",
            &caller.correlation,
            json!({ "tenant": caller.site.as_str() }),
        );
        let (status, body) = error(401, "A provisioning token is required.", None);
        return Ok(json!({ "status": status, "body": body }));
    }
    for rule in &limits {
        app.store
            .throttle_settle(&caller.site, &rule.key, rule.max, ThrottleSettle::Refund)?;
    }

    let method = request
        .get("method")
        .and_then(Json::as_str)
        .unwrap_or("")
        .to_uppercase();
    let path = request.get("path").and_then(Json::as_str).unwrap_or("");
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    let body = request.get("body").cloned().unwrap_or(Json::Null);
    let filter = request.pointer("/query/filter").and_then(Json::as_str);
    let (status, body) = match (method.as_str(), segments.as_slice()) {
        ("POST", ["Users"]) => create_user(app, caller, &body)?,
        ("GET", ["Users"]) => list_users(app, caller, filter)?,
        ("GET", ["Users", id]) => match user_of(app, &caller.site, id)? {
            Some(user) => (200, render(&user)),
            None => error(404, "No such user.", None),
        },
        ("PUT", ["Users", id]) => replace_user(app, caller, id, &body)?,
        ("PATCH", ["Users", id]) => patch_user(app, caller, id, &body)?,
        ("DELETE", ["Users", id]) => delete_user(app, caller, id)?,
        ("GET", ["Groups", group]) => get_group(app, caller, group)?,
        ("PATCH", ["Groups", group]) => patch_group(app, caller, group, &body)?,
        _ => error(404, "No such resource.", None),
    };
    Ok(json!({ "status": status, "body": body }))
}
