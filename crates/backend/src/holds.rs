//! Held operations at the backend (D-8400, D-8406, ADM-1601 to ADM-1604, ADM-1614).
//!
//! The kernel decides everything (`strata_kernel::holds`): which change a rule holds, who may decide
//! it, whether it is still pending at the revision the decision read, and what it applies. This
//! module is the shell around it:
//!
//! - [`notice_of`] turns the kernel's answer to a held change into the notice a reply carries;
//! - [`decide`] is the decision of a signed-in person on one hold, committed through
//!   [`crate::ops::run_intent`], so the store's serialisation is what makes two approvers that race
//!   into one decision and one `VersionConflict`;
//! - [`list`] is what an approvals screen reads: the holds the caller may decide, and the ones the
//!   caller filed.
//!
//! Every step is audited by [`crate::change_audit`], which sees each intent that goes through
//! [`crate::ops::run_intent`]: `hold.filed` when a change is filed (by a rule or by `file-hold`),
//! `hold.decided` when it is approved or rejected, with the entry of the change itself when it was
//! applied, as the requester and with the approver named, and `hold.refused` for each refused
//! decision.

use crate::app::{ApiError, App, Shared};
use crate::auth::Identity;
use crate::ops::run_intent;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json_};
use strata_kernel::holds::{intent_of, pending_for};
use strata_kernel::{
    Context, HeldOperation, HeldOperationId, HoldDecision, HoldState, Intent, Output, Principal,
};
use strata_protocol::api::{ErrorCode, HeldNotice};

/// The notice a reply carries when the kernel filed the change instead of applying it.
pub fn notice_of(output: &Output) -> Option<HeldNotice> {
    match output {
        Output::Held {
            hold,
            approver,
            expires_at,
        } => Some(HeldNotice {
            hold: hold.as_str().to_string(),
            approver: approver.to_string(),
            expires_at: expires_at.0,
        }),
        _ => None,
    }
}

/// A decision on a hold: the hold, `approve` or `reject`, the revision of the hold the decider
/// read, and an optional reason.
#[derive(Clone, Debug, Deserialize)]
pub struct DecideRequest {
    pub hold: String,
    pub decision: String,
    pub read_revision: u64,
    #[serde(default)]
    pub reason: Option<String>,
}

/// What a decision reports: the state the hold is now in and its new revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DecideReply {
    pub hold: String,
    pub state: String,
    pub revision: u64,
    pub event_seq: u64,
}

fn invalid(message: &str) -> ApiError {
    ApiError::new(ErrorCode::Invalid, message)
}

fn state_name(state: HoldState) -> &'static str {
    match state {
        HoldState::Pending => "pending",
        HoldState::Applied => "applied",
        HoldState::Rejected => "rejected",
    }
}

/// Decide a held operation as the signed-in caller (ADM-1602, ADM-1614).
pub fn decide(
    app: &App,
    identity: &Identity,
    request: &DecideRequest,
) -> Result<DecideReply, ApiError> {
    if identity.anonymous() {
        return Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in to decide a held operation.",
        ));
    }
    let hold = HeldOperationId::new(request.hold.as_str())
        .map_err(|_| invalid("hold is not the id of a held operation."))?;
    let decision = match request.decision.as_str() {
        "approve" => HoldDecision::Approve,
        "reject" => HoldDecision::Reject,
        _ => return Err(invalid("decision is approve or reject.")),
    };
    if request
        .reason
        .as_ref()
        .is_some_and(|r| r.chars().count() > 500)
    {
        return Err(invalid("reason is at most 500 characters."));
    }
    let intent = Intent::DecideHold {
        hold,
        decision,
        read_revision: request.read_revision,
        reason: request.reason.clone(),
    };
    let ctx: Context = identity.context(app);
    let done = run_intent(app, &ctx, &intent, false)?;
    match done.transition.output {
        Output::HoldDecided {
            hold,
            state,
            revision,
        } => Ok(DecideReply {
            hold: hold.as_str().to_string(),
            state: state_name(state).to_string(),
            revision,
            event_seq: done.transition.delta.event_seq,
        }),
        other => Err(ApiError::new(
            ErrorCode::Internal,
            format!("unexpected output {other:?}"),
        )),
    }
}

/// One hold as a screen reads it.
fn line(held: &HeldOperation) -> Json_ {
    json!({
        "hold": held.id.as_str(),
        "state": state_name(held.state),
        "revision": held.revision,
        "approver": held.approver.to_string(),
        "requester": held.requester.as_str(),
        "on_behalf_of": held.on_behalf_of.as_ref().map(|p| p.as_str()),
        "filed_at": held.filed_at.0,
        "expires_at": held.expires_at.0,
        "rule": held.rule.as_ref().map(|r| r.as_str()),
        "change": intent_name(&intent_of(&held.change)),
        "decided_by": held.decided_by.as_ref().map(|p| p.as_str()),
        "decided_at": held.decided_at.map(|t| t.0),
    })
}

fn intent_name(intent: &Intent) -> &'static str {
    use strata_kernel::ext::IntentExt;
    intent.name()
}

/// The holds the signed-in caller may decide now, and the ones the caller filed or is the person
/// acted for, newest first (D-8400). A caller who is neither an approver nor a requester sees none,
/// since a hold names the change and the person it acts for.
pub fn list(app: &App, identity: &Identity) -> Result<Json_, ApiError> {
    if identity.anonymous() {
        return Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in to read the held operations.",
        ));
    }
    let Principal::User(who) = &identity.principal else {
        return Ok(json!({ "decidable": [], "own": [] }));
    };
    let tenant = crate::ops::header(app, identity)?;
    let mut decidable: Vec<&HeldOperation> = pending_for(&tenant, who, app.now());
    decidable.sort_by(|a, b| b.filed_at.cmp(&a.filed_at).then(b.id.cmp(&a.id)));
    let mut own: Vec<&HeldOperation> = tenant
        .schema
        .holds
        .iter()
        .flatten()
        .map(|(_, held)| held)
        .filter(|held| held.requester == *who || held.on_behalf_of.as_ref() == Some(who))
        .collect();
    own.sort_by(|a, b| b.filed_at.cmp(&a.filed_at).then(b.id.cmp(&a.id)));
    Ok(json!({
        "decidable": decidable.into_iter().map(line).collect::<Vec<_>>(),
        "own": own.into_iter().map(line).collect::<Vec<_>>(),
    }))
}

/// `/internal/holds/decide`: decide a held operation as the identified caller.
pub async fn decide_route(
    State(app): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<DecideRequest>,
) -> Result<Json<DecideReply>, ApiError> {
    crate::http::as_caller(&app, &headers, move |app, identity| {
        decide(app, identity, &request)
    })
    .await
}

/// `/internal/holds/list`: the held operations the identified caller may decide or filed.
pub async fn list_route(
    State(app): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Json_>, ApiError> {
    crate::http::as_caller(&app, &headers, list).await
}
