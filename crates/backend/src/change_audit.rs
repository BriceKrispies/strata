//! The audit entry of every configuration and role change, with the previous value, the new
//! value, the principal and the correlation identifier (OPS-329).
//!
//! `ops::run_intent_at` is the one place every change of a running tenant is committed. It calls
//! [`before`] ahead of the commit and [`record`] after it. An intent that is not a change of
//! configuration or of who holds which role leaves no entry and costs nothing here. A refused or
//! replayed intent leaves none either: the entry says what a commit changed.
//!
//! What is recorded, by intent:
//!
//! - **Roles of a person** (`assign-role`, `revoke-role`, `redeem-invitation`): the event
//!   `member.roles_changed`, with `principal` (the person whose roles changed), `previous` and
//!   `value` (their assigned roles before and after).
//! - **Team membership** (`add-team-member`, `remove-team-member`): `team.membership_changed`,
//!   with `principal` and `previous` and `value` as `{ team, member }`.
//! - **A team** (`define-team`, `assign-team-role`, `revoke-team-role`): `team.changed`, with
//!   `before` and `after` as `{ label, roles }`.
//! - **An invitation** (`create-invitation`, `revoke-invitation`): `invitation.changed`, with
//!   `before` and `after` as `{ roles, revoked, redeemed }`; the address is not recorded.
//! - **A setting** (`write-setting`): `setting.changed`, with `setting`, `level`, and the value at
//!   that level before and after as `before` and `after` (as `previous` and `value` with the
//!   `principal` at the person level, where the value is the person's own data).
//! - **The schema** (`change-schema`): `schema.changed`, with `before` and `after` as `{ version,
//!   role_permissions }`, the permissions of each role the change defines, and `changes`, the
//!   summary the kernel wrote.
//!
//! Every entry has `tenant`, `intent`, `revision` and the actor as `actor` (a person) or
//! `actor_kind` (`system`, `anonymous`); the correlation identifier is the entry's own. The
//! members that name a person, and `previous` and `value` with them, are sealed under that
//! person's key by the sealed store, like every audit line (PRV-359).
//!
//! The entry is written after the commit. A failure to write it is logged by [`App::audit`] and
//! does not undo the change; the commit's own event and journal line carry the same change.
//! Provisioning (`ops::run_intent_unaudited`) leaves none: a tenant that is being installed has
//! no earlier configuration to change.

use crate::app::App;
use serde_json::{json, Map, Value as Json};
use std::collections::BTreeSet;
use std::sync::Arc;
use strata_kernel::ext::IntentExt;
use strata_kernel::settings::override_of;
use strata_kernel::{
    Context, EventBody, Intent, Principal, PrincipalId, RoleId, SchemaChange, SettingLevel,
    TenantSchemaExt, TenantState,
};
use strata_storage_api::Executed;
use strata_substrate::plain::to_plain;

/// The tenant as it was before the commit.
pub struct Prior {
    state: Arc<TenantState>,
}

/// The name of the event an intent's entry has, when the intent changes configuration or roles.
fn event_of(intent: &Intent) -> Option<&'static str> {
    match intent {
        Intent::AssignRole { .. }
        | Intent::AssignScopedRole { .. }
        | Intent::RevokeRole { .. }
        | Intent::RedeemInvitation { .. } => Some("member.roles_changed"),
        Intent::AddTeamMember { .. } | Intent::RemoveTeamMember { .. } => {
            Some("team.membership_changed")
        }
        Intent::DefineTeam { .. }
        | Intent::AssignTeamRole { .. }
        | Intent::RevokeTeamRole { .. } => Some("team.changed"),
        Intent::CreateInvitation { .. } | Intent::RevokeInvitation { .. } => {
            Some("invitation.changed")
        }
        Intent::WriteSetting { .. } => Some("setting.changed"),
        Intent::ChangeSchema { .. } => Some("schema.changed"),
        _ => None,
    }
}

fn user_of(ctx: &Context) -> Option<PrincipalId> {
    match &ctx.principal {
        Principal::User(id) => Some(id.clone()),
        _ => None,
    }
}

/// The person a change is about, when it is about one: their roles or their own setting.
fn person_of(intent: &Intent, ctx: &Context) -> Option<PrincipalId> {
    match intent {
        Intent::AssignRole { principal, .. }
        | Intent::AssignScopedRole { principal, .. }
        | Intent::RevokeRole { principal, .. }
        | Intent::AddTeamMember { principal, .. }
        | Intent::RemoveTeamMember { principal, .. } => Some(principal.clone()),
        Intent::RedeemInvitation { .. }
        | Intent::WriteSetting {
            level: SettingLevel::Person,
            ..
        } => user_of(ctx),
        _ => None,
    }
}

fn names(roles: &BTreeSet<RoleId>) -> Vec<&str> {
    roles.iter().map(RoleId::as_str).collect()
}

fn header(app: &App, ctx: &Context) -> Option<Arc<TenantState>> {
    app.store.load(&ctx.tenant, &[]).ok()?.tenant
}

/// What `intent` changes, as it stands in `state`.
fn value_in(intent: &Intent, ctx: &Context, state: &TenantState) -> Json {
    match intent {
        Intent::AssignRole { principal, .. } | Intent::RevokeRole { principal, .. } => {
            json!(state.members.get(principal).map(names))
        }
        // A scoped assignment adds nothing to the roles of a person (D-8403): what the entry
        // records is their scoped roles, each with the object type it is narrowed to.
        Intent::AssignScopedRole { principal, .. } => {
            json!(strata_kernel::scoped::of(&state.schema, principal)
                .map(|held| json!({ "role": held.role.as_str(), "object": held.object.as_str() }))
                .collect::<Vec<_>>())
        }
        Intent::RedeemInvitation { .. } => {
            json!(user_of(ctx).and_then(|person| state.members.get(&person).map(names)))
        }
        Intent::AddTeamMember { team, principal }
        | Intent::RemoveTeamMember { team, principal } => {
            json!({
                "team": team.as_str(),
                "member": state
                    .schema
                    .team(team)
                    .is_some_and(|held| held.members.contains(principal)),
            })
        }
        Intent::DefineTeam { team, .. }
        | Intent::AssignTeamRole { team, .. }
        | Intent::RevokeTeamRole { team, .. } => state.schema.team(team).map_or(
            Json::Null,
            |held| json!({ "label": held.label, "roles": names(&held.roles) }),
        ),
        Intent::CreateInvitation { invitation, .. } | Intent::RevokeInvitation { invitation } => {
            state
                .schema
                .invitation(invitation)
                .map_or(Json::Null, |held| {
                    json!({
                        "roles": names(&held.roles),
                        "revoked": held.revoked_by.is_some(),
                        "redeemed": held.redeemed_by.is_some(),
                    })
                })
        }
        Intent::WriteSetting {
            setting,
            level,
            team,
            ..
        } => {
            let subject = match (level, team) {
                (SettingLevel::Team, Some(team)) => team.as_str().to_string(),
                (SettingLevel::Person, _) => user_of(ctx)
                    .map(|person| person.as_str().to_string())
                    .unwrap_or_default(),
                _ => state.id.as_str().to_string(),
            };
            override_of(state, setting, *level, &subject)
                .map_or(Json::Null, |held| to_plain(&held.value))
        }
        Intent::ChangeSchema { changes, .. } => {
            let mut permissions = Map::new();
            for change in changes {
                if let SchemaChange::DefineRolePermissions { role, .. } = change {
                    permissions.insert(
                        role.as_str().to_string(),
                        serde_json::to_value(state.schema.grants_of(role)).unwrap_or(Json::Null),
                    );
                }
            }
            json!({
                "version": state.schema.version.0,
                "role_permissions": permissions,
            })
        }
        _ => Json::Null,
    }
}

/// Note the tenant as it is before `intent` is committed. `None` for an intent that is not a
/// change of configuration or roles, or when the tenant cannot be read.
pub fn before(app: &App, ctx: &Context, intent: &Intent) -> Option<Prior> {
    event_of(intent)?;
    header(app, ctx).map(|state| Prior { state })
}

/// Append the audit entry of a committed `intent`.
pub fn record(app: &App, ctx: &Context, intent: &Intent, prior: Option<Prior>, done: &Executed) {
    let (Some(event), Some(prior)) = (event_of(intent), prior) else {
        return;
    };
    if done.replayed {
        return;
    }
    let Some(after) = header(app, ctx) else {
        return;
    };
    let previous = value_in(intent, ctx, &prior.state);
    let now = value_in(intent, ctx, &after);
    let mut fields = Map::new();
    fields.insert("tenant".into(), json!(ctx.tenant.as_str()));
    fields.insert("intent".into(), json!(intent.name()));
    fields.insert("revision".into(), json!(done.receipt.revision.0));
    match &ctx.principal {
        Principal::User(actor) => fields.insert("actor".into(), json!(actor.as_str())),
        Principal::System => fields.insert("actor_kind".into(), json!("system")),
        Principal::Anonymous => fields.insert("actor_kind".into(), json!("anonymous")),
    };
    match person_of(intent, ctx) {
        Some(person) => {
            fields.insert("principal".into(), json!(person.as_str()));
            fields.insert("previous".into(), previous);
            fields.insert("value".into(), now);
        }
        None => {
            fields.insert("before".into(), previous);
            fields.insert("after".into(), now);
        }
    }
    if let Intent::WriteSetting { setting, level, .. } = intent {
        fields.insert("setting".into(), json!(setting.as_str()));
        fields.insert("level".into(), json!(level.as_str()));
    }
    if let Intent::ChangeSchema { .. } = intent {
        let summary = done
            .transition
            .events
            .iter()
            .find_map(|event| match &event.body {
                EventBody::SchemaChanged { summary, .. } => Some(summary.clone()),
                _ => None,
            })
            .unwrap_or_default();
        fields.insert("changes".into(), json!(summary));
    }
    app.audit(event, &ctx.correlation, Json::Object(fields));
}
