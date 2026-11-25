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
//! - **A held operation** (D-8400): `hold.filed` when a hold rule or `file-hold` files a change
//!   (the change itself leaves no entry, since nothing changed), with `hold`, `requester`,
//!   `on_behalf_of`, `approver`, `rule`, `input_hash`, `expires_at` and `change` (the name of the
//!   intent it freezes); `hold.decided` when `decide-hold` approves or rejects one, with `hold`,
//!   `state`, `revision`, `requester` and `decided_by`, and, for an approval, the entry of the change
//!   it applied, written with the requester as `actor` and the approver as `approved_by`;
//!   `hold.refused` for a decision that was refused ([`refused`]), with the `kind` and the failed
//!   checks.
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
    Context, EventBody, HeldOperation, HeldOperationId, HoldState, Intent, Output, Principal,
    PrincipalId, RoleId, SchemaChange, SettingLevel, TenantSchemaExt, TenantState,
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
    if event_of(intent).is_none() && !matches!(intent, Intent::DecideHold { .. }) {
        return None;
    }
    header(app, ctx).map(|state| Prior { state })
}

/// Append the audit entry of a committed `intent`.
pub fn record(app: &App, ctx: &Context, intent: &Intent, prior: Option<Prior>, done: &Executed) {
    if done.replayed {
        return;
    }
    match &done.transition.output {
        Output::Held { hold, .. } => {
            return record_filed(app, ctx, intent, hold, done);
        }
        Output::HoldDecided { hold, .. } => {
            return record_decided(app, ctx, hold, prior, done);
        }
        _ => {}
    }
    let (Some(event), Some(prior)) = (event_of(intent), prior) else {
        return;
    };
    let Some(after) = header(app, ctx) else {
        return;
    };
    write_entry(app, ctx, event, intent, &prior, &after, done, None);
}

/// The entry of an accepted change of configuration or roles, with the principal of `ctx` as its
/// actor and `extra` members added.
#[allow(clippy::too_many_arguments)]
fn write_entry(
    app: &App,
    ctx: &Context,
    event: &str,
    intent: &Intent,
    prior: &Prior,
    after: &Arc<TenantState>,
    done: &Executed,
    extra: Option<Map<String, Json>>,
) {
    let previous = value_in(intent, ctx, &prior.state);
    let now = value_in(intent, ctx, after);
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
    if let Some(extra) = extra {
        fields.extend(extra);
    }
    app.audit(event, &ctx.correlation, Json::Object(fields));
}

/// The hold `id` as the tenant holds it after the commit.
fn held_in(state: &TenantState, id: &HeldOperationId) -> Option<HeldOperation> {
    state.schema.holds.as_ref()?.get(id).cloned()
}

/// `hold.filed`: a change a rule held, or that `file-hold` filed (D-8400, ADM-1601).
fn record_filed(app: &App, ctx: &Context, intent: &Intent, id: &HeldOperationId, done: &Executed) {
    let Some(after) = header(app, ctx) else {
        return;
    };
    let Some(held) = held_in(&after, id) else {
        return;
    };
    let mut fields = Map::new();
    fields.insert("tenant".into(), json!(ctx.tenant.as_str()));
    fields.insert("intent".into(), json!(intent.name()));
    fields.insert("revision".into(), json!(done.receipt.revision.0));
    fields.insert("hold".into(), json!(held.id.as_str()));
    fields.insert("requester".into(), json!(held.requester.as_str()));
    fields.insert(
        "on_behalf_of".into(),
        json!(held.on_behalf_of.as_ref().map(PrincipalId::as_str)),
    );
    fields.insert("approver".into(), json!(held.approver.to_string()));
    fields.insert(
        "rule".into(),
        json!(held.rule.as_ref().map(|rule| rule.as_str())),
    );
    fields.insert("input_hash".into(), json!(held.input_hash));
    fields.insert("expires_at".into(), json!(held.expires_at.0));
    fields.insert(
        "change".into(),
        json!(strata_kernel::holds::intent_of(&held.change).name()),
    );
    app.audit("hold.filed", &ctx.correlation, Json::Object(fields));
}

/// `hold.decided`, and for an approval the entry of the change it applied (D-8400, ADM-1604).
fn record_decided(
    app: &App,
    ctx: &Context,
    id: &HeldOperationId,
    prior: Option<Prior>,
    done: &Executed,
) {
    let Some(after) = header(app, ctx) else {
        return;
    };
    let Some(held) = held_in(&after, id) else {
        return;
    };
    let mut fields = Map::new();
    fields.insert("tenant".into(), json!(ctx.tenant.as_str()));
    fields.insert("intent".into(), json!("decide_hold"));
    fields.insert("revision".into(), json!(done.receipt.revision.0));
    fields.insert("hold".into(), json!(held.id.as_str()));
    fields.insert(
        "state".into(),
        json!(format!("{:?}", held.state).to_lowercase()),
    );
    fields.insert("hold_revision".into(), json!(held.revision));
    fields.insert("requester".into(), json!(held.requester.as_str()));
    fields.insert(
        "decided_by".into(),
        json!(held.decided_by.as_ref().map(PrincipalId::as_str)),
    );
    fields.insert("approver".into(), json!(held.approver.to_string()));
    fields.insert("input_hash".into(), json!(held.input_hash));
    if let Principal::User(decider) = &ctx.principal {
        fields.insert("actor".into(), json!(decider.as_str()));
    }
    app.audit("hold.decided", &ctx.correlation, Json::Object(fields));

    // The change an approval applied is a change like any other: it leaves its entry, made by the
    // requester and approved by the person who decided.
    if held.state != HoldState::Applied {
        return;
    }
    let Some(prior) = prior else {
        return;
    };
    let applied = strata_kernel::holds::intent_of(&held.change);
    let Some(event) = event_of(&applied) else {
        return;
    };
    let as_requester = Context {
        principal: Principal::User(held.requester.clone()),
        ..ctx.clone()
    };
    let extra = Map::from_iter([
        ("hold".to_string(), json!(held.id.as_str())),
        (
            "approved_by".to_string(),
            json!(held.decided_by.as_ref().map(PrincipalId::as_str)),
        ),
    ]);
    write_entry(
        app,
        &as_requester,
        event,
        &applied,
        &prior,
        &after,
        done,
        Some(extra),
    );
}

/// `hold.refused`: a decision on a held operation that was refused, with the kind of the refusal
/// and the checks that failed (ADM-1602 to ADM-1604, ADM-1614). Any other intent leaves nothing.
pub fn refused(app: &App, ctx: &Context, intent: &Intent, error: &crate::app::ApiError) {
    let Intent::DecideHold {
        hold,
        decision,
        read_revision,
        ..
    } = intent
    else {
        return;
    };
    let mut fields = Map::new();
    fields.insert("tenant".into(), json!(ctx.tenant.as_str()));
    fields.insert("intent".into(), json!("decide_hold"));
    fields.insert("hold".into(), json!(hold.as_str()));
    fields.insert(
        "decision".into(),
        json!(format!("{decision:?}").to_lowercase()),
    );
    fields.insert("read_revision".into(), json!(read_revision));
    fields.insert("kind".into(), json!(error.0.kind));
    fields.insert(
        "failed".into(),
        json!(error
            .0
            .failed
            .iter()
            .map(|check| check.rule.as_str())
            .collect::<Vec<_>>()),
    );
    match &ctx.principal {
        Principal::User(actor) => fields.insert("actor".into(), json!(actor.as_str())),
        Principal::System => fields.insert("actor_kind".into(), json!("system")),
        Principal::Anonymous => fields.insert("actor_kind".into(), json!("anonymous")),
    };
    app.audit("hold.refused", &ctx.correlation, Json::Object(fields));
}
