//! Restrictions of processing (contracts `restrict-subject`, `lift-subject-restriction`, PRV-352)
//! and legal holds (`place-legal-hold`, `release-legal-hold`, `list-legal-holds`, PRV-355), and the
//! one route that serves them and the subject export (`/internal/subject-rights/<operation>`).
//!
//! Both are kept as documents of the tenant (`tbl-identity-docs`): a restriction under its
//! principal, a hold under its id. They are set and lifted by an owner with a fresh step-up, and
//! every change is an audit entry that names the tenant and the request, never the person.
//!
//! **What a restriction does, and where it is enforced.** PRV-352 says a restricted principal's
//! personal fields are not read until the restriction is lifted. The data stays; its use stops. In
//! this tree the restriction is enforced at:
//!
//! - the views (`ops::view`): every row and included record that a view returns has the personal
//!   fields whose subject is a restricted principal removed ([`Rights::hide`]);
//! - the retention sweep (`retention`): a record that concerns a restricted principal is not
//!   deleted when its period ends, so the restriction cannot be used to destroy evidence the person
//!   asked to have kept, and the record is deleted after the restriction is lifted;
//! - the erasure: it is not blocked (erasure is the stronger measure), reports the restriction in
//!   its receipt, and ends it with the erasure (`rec-restriction`: until lifted or erased);
//! - the export: it is not blocked (access is the person's own right) and names the restriction.
//!
//! Not enforced, because the point does not take a restriction yet: automations, segments, mail and
//! any export to a third party. Each is filed as a task; the receipt and the specification say so.
//!
//! **What a legal hold does.** A hold names a subject or a record type. A hold on a subject refuses
//! the erasure of that subject with `conflict`, naming the hold, before any record is changed. A
//! hold on a record type leaves its records unchanged by every erasure (the erasure deletes the
//! rest and the receipt lists each held record) and keeps them from the retention sweep. Released
//! holds stay as the record of the hold.
//!
//! Implements: CT-212, CT-215, CT-1705, CT-1706, CT-1707, CT-1708

use crate::app::{ApiError, App};
use crate::auth::{Caller, Identity};
use crate::ops;
use crate::signin::stepup;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use std::sync::Arc;
use strata_kernel::rules::roles_of;
use strata_kernel::subject_export::subjects_of;
use strata_kernel::{
    owner_role, ObjectTypeDef, ObjectTypeId, PrincipalId, Record, TenantId, TenantSchema,
    TenantSchemaExt,
};
use strata_protocol::api::ErrorCode;

/// The kind of identity document that holds the restrictions of a tenant, one per principal.
pub const RESTRICTION_KIND: &str = "restriction";
/// The kind of identity document that holds the legal holds of a tenant, one per hold id.
pub const HOLD_KIND: &str = "legal-hold";

/// The operations served at `/internal/subject-rights/<operation>`.
pub const OPERATIONS: [&str; 8] = [
    "export",
    "export-fetch",
    "restrict",
    "lift-restriction",
    "place-hold",
    "release-hold",
    "holds",
    "restrictions",
];

/// A restriction of the processing of one principal (`rec-restriction`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Restriction {
    pub principal: String,
    /// When it was placed, in milliseconds.
    pub since: i64,
    pub reason: String,
    /// The owner who placed it.
    pub set_by: String,
}

/// A legal hold (`rec-legal-hold`): on one subject or on one record type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hold {
    pub id: String,
    /// The principal the hold names, when it is a hold on a subject.
    #[serde(default)]
    pub subject: Option<String>,
    /// The record type the hold names, when it is a hold on a record type.
    #[serde(default)]
    pub object: Option<String>,
    pub reason: String,
    pub set_by: String,
    pub set_at: i64,
    /// When the hold was released; `None` while it holds.
    #[serde(default)]
    pub released_at: Option<i64>,
}

impl Hold {
    /// What the hold covers, in words.
    pub fn scope(&self) -> String {
        match (&self.subject, &self.object) {
            (Some(_), _) => "a subject".to_string(),
            (None, Some(object)) => format!("the record type {object}"),
            (None, None) => "nothing".to_string(),
        }
    }
}

/// The restrictions and the active holds of a tenant, read once for one decision.
#[derive(Clone, Debug, Default)]
pub struct Rights {
    restrictions: BTreeMap<String, Restriction>,
    holds: Vec<Hold>,
}

fn read_docs<T: serde::de::DeserializeOwned>(
    app: &App,
    tenant: &TenantId,
    kind: &str,
) -> Result<Vec<T>, ApiError> {
    Ok(app
        .store
        .doc_list(tenant, kind)?
        .into_iter()
        .filter_map(|(_, text)| serde_json::from_str::<T>(&text).ok())
        .collect())
}

impl Rights {
    /// The restrictions and the active holds of `tenant`.
    pub fn load(app: &App, tenant: &TenantId) -> Result<Rights, ApiError> {
        let mut rights = Rights::load_restrictions(app, tenant)?;
        rights.holds = read_docs::<Hold>(app, tenant, HOLD_KIND)?
            .into_iter()
            .filter(|hold| hold.released_at.is_none())
            .collect();
        rights.holds.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(rights)
    }

    /// Only the restrictions: what a read of records needs.
    pub fn load_restrictions(app: &App, tenant: &TenantId) -> Result<Rights, ApiError> {
        Ok(Rights {
            restrictions: read_docs::<Restriction>(app, tenant, RESTRICTION_KIND)?
                .into_iter()
                .map(|restriction| (restriction.principal.clone(), restriction))
                .collect(),
            holds: Vec::new(),
        })
    }

    pub fn restriction_of(&self, subject: &PrincipalId) -> Option<&Restriction> {
        self.restrictions.get(subject.as_str())
    }

    /// The active hold on the subject, if there is one.
    pub fn hold_on_subject(&self, subject: &PrincipalId) -> Option<&Hold> {
        self.holds
            .iter()
            .find(|hold| hold.subject.as_deref() == Some(subject.as_str()))
    }

    /// The record types an active hold names, in id order.
    pub fn held_objects(&self) -> Vec<ObjectTypeId> {
        let mut out: Vec<ObjectTypeId> = self
            .holds
            .iter()
            .filter_map(|hold| hold.object.as_deref())
            .filter_map(|name| ObjectTypeId::new(name).ok())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// The refusal of an erasure the subject's hold suspends: `conflict`, naming the hold.
    pub fn refuse_if_held(&self, subject: &PrincipalId) -> Result<(), ApiError> {
        match self.hold_on_subject(subject) {
            None => Ok(()),
            Some(hold) => {
                let mut error = ApiError::new(
                    ErrorCode::Conflict,
                    format!(
                        "Legal hold {} suspends the erasure of this person until it is released: {}",
                        hold.id, hold.reason
                    ),
                );
                error.0.kind = Some("legal_hold".to_string());
                Err(error)
            }
        }
    }

    /// Why a record may not be deleted by retention: a hold on its type or on a person it
    /// concerns, or a restriction of a person it concerns. `None` when nothing stops it.
    pub fn blocks_retention(&self, object: &ObjectTypeDef, record: &Record) -> Option<String> {
        if let Some(hold) = self
            .holds
            .iter()
            .find(|hold| hold.object.as_deref() == Some(object.id.as_str()))
        {
            return Some(format!("legal hold {}", hold.id));
        }
        if self.restrictions.is_empty() && self.holds.is_empty() {
            return None;
        }
        for subject in subjects_of(object, record) {
            if let Some(hold) = self.hold_on_subject(&subject) {
                return Some(format!("legal hold {}", hold.id));
            }
            if self.restriction_of(&subject).is_some() {
                return Some("a restriction of processing".to_string());
            }
        }
        None
    }

    /// The record as a read may show it: every personal field whose subject is a restricted
    /// principal removed (PRV-352). A record that concerns no restricted principal is returned as
    /// it is.
    pub fn hide(&self, schema: &TenantSchema, record: &Arc<Record>) -> Arc<Record> {
        if self.restrictions.is_empty() {
            return record.clone();
        }
        let Some(object) = schema.objects.get(&record.object) else {
            return record.clone();
        };
        let mut masked: Option<Record> = None;
        for (id, field) in &object.fields {
            let Some(personal) = &field.personal else {
                continue;
            };
            let concerned = match &personal.subject {
                strata_kernel::DataSubject::Creator => record
                    .created_by
                    .as_ref()
                    .is_some_and(|creator| self.restriction_of(creator).is_some()),
                strata_kernel::DataSubject::Field { field: link } => {
                    let mut who = std::collections::BTreeSet::new();
                    collect(record.fields.get(link), &mut who);
                    who.iter().any(|p| self.restriction_of(p).is_some())
                }
            };
            if concerned {
                masked
                    .get_or_insert_with(|| Record::clone(record))
                    .fields
                    .remove(id);
            }
        }
        match masked {
            Some(masked) => Arc::new(masked),
            None => record.clone(),
        }
    }

    /// A search hit as a read may show it (PRV-352, D-7741): the record with its restricted
    /// personal fields removed ([`Rights::hide`]) and the matched fields that are still in it. `None`
    /// when no matched field survives: the record was found only by a field the restriction hides,
    /// and returning it would tell the reader that field's value.
    pub fn hide_hit(
        &self,
        schema: &TenantSchema,
        record: &Arc<Record>,
        matched: &[strata_kernel::FieldId],
    ) -> Option<(Arc<Record>, Vec<strata_kernel::FieldId>)> {
        let shown = self.hide(schema, record);
        let kept: Vec<strata_kernel::FieldId> = matched
            .iter()
            .filter(|field| shown.fields.contains_key(*field))
            .cloned()
            .collect();
        (!kept.is_empty()).then_some((shown, kept))
    }

    /// What the receipt of an erasure says of restrictions and holds, for the subject.
    pub fn receipt_lines(&self, subject: &PrincipalId) -> Vec<String> {
        let mut lines = Vec::new();
        match self.restriction_of(subject) {
            Some(restriction) => lines.push(format!(
                "restriction of processing (PRV-352): the principal was restricted since {} ({}); the erasure was not blocked and the restriction ended with it",
                restriction.since, restriction.reason
            )),
            None => lines.push(
                "restriction of processing (PRV-352): none was in force on the principal"
                    .to_string(),
            ),
        }
        let held = self.held_objects();
        if held.is_empty() {
            lines.push("legal holds (PRV-355): none suspended any record".to_string());
        } else {
            lines.push(format!(
                "legal holds (PRV-355): records of the held record types {:?} were left unchanged; each is named in the records not erased",
                held.iter().map(|o| o.as_str()).collect::<Vec<_>>()
            ));
        }
        lines
    }
}

fn collect(
    value: Option<&strata_kernel::Value>,
    into: &mut std::collections::BTreeSet<PrincipalId>,
) {
    match value {
        Some(strata_kernel::Value::Str(text)) => {
            if let Ok(id) = PrincipalId::new(text.as_str()) {
                into.insert(id);
            }
        }
        Some(strata_kernel::Value::List(items)) => {
            for item in items {
                collect(Some(item), into);
            }
        }
        _ => {}
    }
}

fn not_found() -> ApiError {
    // The same answer whether the person never existed here or belongs to another tenant.
    ApiError::new(ErrorCode::NotFound, "No such person.")
}

fn invalid(message: &str) -> ApiError {
    ApiError::new(ErrorCode::Invalid, message)
}

/// The signed-in caller, or the refusal that says to sign in.
pub fn signed_in(identity: &Identity) -> Result<PrincipalId, ApiError> {
    match &identity.principal {
        strata_kernel::Principal::User(id) if !identity.anonymous() => Ok(id.clone()),
        _ => Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in to do this.",
        )),
    }
}

/// Whether the caller owns the tenant.
pub fn is_owner(app: &App, identity: &Identity) -> Result<bool, ApiError> {
    let tenant = ops::header(app, identity)?;
    Ok(roles_of(&tenant, &identity.principal).contains(&owner_role()))
}

fn require_owner(app: &App, identity: &Identity) -> Result<PrincipalId, ApiError> {
    let actor = signed_in(identity)?;
    if is_owner(app, identity)? {
        Ok(actor)
    } else {
        Err(ApiError::new(
            ErrorCode::Forbidden,
            "Only an owner of the tenant may do this.",
        ))
    }
}

/// A request that names a tenant other than the one it is made at is refused, and the refusal is
/// an audit entry (TEN-1140).
pub fn refuse_other_tenant(app: &App, identity: &Identity, body: &Json) -> Result<(), ApiError> {
    match body.get("tenant").and_then(Json::as_str) {
        Some(named) if named != identity.tenant.as_str() => {
            app.audit(
                "subject_rights.tenant_mismatch",
                &identity.correlation,
                json!({ "tenant": identity.tenant.as_str() }),
            );
            let mut error = ApiError::new(
                ErrorCode::Forbidden,
                "The request names another tenant than the one it is made at.",
            );
            error.0.kind = Some("tenant_mismatch".to_string());
            Err(error)
        }
        _ => Ok(()),
    }
}

/// The principal a request names, as the tenant knows it: a member, a place in a team, an account,
/// or an invitation issued or redeemed. Anyone else, or a malformed name, is not found.
pub fn known_principal(
    app: &App,
    identity: &Identity,
    body: &Json,
    field: &str,
) -> Result<PrincipalId, ApiError> {
    let subject = body
        .get(field)
        .and_then(Json::as_str)
        .and_then(|name| PrincipalId::new(name).ok())
        .ok_or_else(not_found)?;
    let tenant = ops::header(app, identity)?;
    let known = tenant.members.contains_key(&subject)
        || tenant
            .schema
            .all_teams()
            .any(|team| team.members.contains(&subject))
        || tenant.schema.all_invitations().any(|offer| {
            offer.created_by == subject || offer.redeemed_by.as_ref() == Some(&subject)
        })
        || app.store.account_of(&identity.tenant, &subject)?.is_some();
    if known {
        Ok(subject)
    } else {
        Err(not_found())
    }
}

fn reason_of(body: &Json) -> Result<String, ApiError> {
    let reason = body
        .get("reason")
        .and_then(Json::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if reason.is_empty() {
        return Err(invalid("Say why."));
    }
    if reason.len() > 500 {
        return Err(invalid("The reason is at most 500 characters."));
    }
    Ok(reason.to_string())
}

/// Place a restriction on one principal: `restrict-subject` (CT-212).
///
/// Implements: CT-212
pub fn restrict(
    app: &App,
    caller: &Caller,
    identity: &Identity,
    body: &Json,
) -> Result<Json, ApiError> {
    refuse_other_tenant(app, identity, body)?;
    let actor = require_owner(app, identity)?;
    stepup::require_fresh(app, caller, identity)?;
    let subject = known_principal(app, identity, body, "principal")?;
    let reason = reason_of(body)?;
    let now = app.now();
    let restriction = Restriction {
        principal: subject.as_str().to_string(),
        since: now.0,
        reason,
        set_by: actor.as_str().to_string(),
    };
    // A restriction that is in force already keeps its first time and reason: placing it again
    // changes nothing (the contract is idempotent).
    let existing = Rights::load_restrictions(app, &identity.tenant)?;
    let kept = existing
        .restriction_of(&subject)
        .cloned()
        .unwrap_or(restriction);
    let text = serde_json::to_string(&kept)
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, e.to_string()))?;
    app.store.doc_put(
        &identity.tenant,
        RESTRICTION_KIND,
        subject.as_str(),
        &text,
        false,
    )?;
    app.audit(
        "subject_rights.restricted",
        &identity.correlation,
        json!({ "tenant": identity.tenant.as_str(), "by": actor.as_str() }),
    );
    Ok(json!({ "restriction": kept }))
}

/// Lift a restriction: `lift-subject-restriction` (CT-1705).
///
/// Implements: CT-1705
pub fn lift(
    app: &App,
    caller: &Caller,
    identity: &Identity,
    body: &Json,
) -> Result<Json, ApiError> {
    refuse_other_tenant(app, identity, body)?;
    let actor = require_owner(app, identity)?;
    stepup::require_fresh(app, caller, identity)?;
    let subject = body
        .get("principal")
        .and_then(Json::as_str)
        .and_then(|name| PrincipalId::new(name).ok())
        .ok_or_else(not_found)?;
    let lifted = app
        .store
        .doc_remove(&identity.tenant, RESTRICTION_KIND, subject.as_str())?;
    if lifted {
        app.audit(
            "subject_rights.restriction_lifted",
            &identity.correlation,
            json!({ "tenant": identity.tenant.as_str(), "by": actor.as_str() }),
        );
    }
    Ok(json!({ "lifted": lifted }))
}

/// The restrictions of the tenant, readable by an owner.
pub fn restrictions(app: &App, identity: &Identity) -> Result<Json, ApiError> {
    require_owner(app, identity)?;
    let rights = Rights::load_restrictions(app, &identity.tenant)?;
    Ok(json!({ "restrictions": rights.restrictions.values().collect::<Vec<_>>() }))
}

/// Place a legal hold on a subject or a record type: `place-legal-hold` (CT-215).
///
/// Implements: CT-215
pub fn place_hold(
    app: &App,
    caller: &Caller,
    identity: &Identity,
    body: &Json,
) -> Result<Json, ApiError> {
    refuse_other_tenant(app, identity, body)?;
    let actor = require_owner(app, identity)?;
    stepup::require_fresh(app, caller, identity)?;
    let reason = reason_of(body)?;
    let subject = body.get("subject").and_then(Json::as_str);
    let object = body.get("object").and_then(Json::as_str);
    let (subject, object) = match (subject, object) {
        (Some(subject), None) => {
            let id = PrincipalId::new(subject).map_err(|_| invalid("Name the person."))?;
            (Some(id.as_str().to_string()), None)
        }
        (None, Some(object)) => {
            let id = ObjectTypeId::new(object).map_err(|_| invalid("Name the record type."))?;
            let tenant = ops::header(app, identity)?;
            if !tenant.schema.objects.contains_key(&id) {
                return Err(invalid("The tenant has no such record type."));
            }
            (None, Some(id.as_str().to_string()))
        }
        _ => {
            return Err(invalid(
                "A hold names either a subject or a record type, one of the two.",
            ))
        }
    };
    let now = app.now();
    // A UUID derived from the tenant, the request and the time: a repeat of the same request
    // names the same hold (the document is created once for its key).
    let id = strata_storage_api::jobs::derived_uuid(&format!(
        "strata/legal-hold/v1\0{}\0{}\0{}",
        identity.tenant, identity.correlation, now.0
    ));
    let hold = Hold {
        id: id.clone(),
        subject,
        object,
        reason,
        set_by: actor.as_str().to_string(),
        set_at: now.0,
        released_at: None,
    };
    let text = serde_json::to_string(&hold)
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, e.to_string()))?;
    app.store
        .doc_put(&identity.tenant, HOLD_KIND, &id, &text, true)?;
    app.audit(
        "subject_rights.hold_placed",
        &identity.correlation,
        json!({ "tenant": identity.tenant.as_str(), "hold": id, "by": actor.as_str() }),
    );
    Ok(json!({ "hold": hold }))
}

/// Release a legal hold: `release-legal-hold` (CT-1706). The hold stays as the record of it.
///
/// Implements: CT-1706
pub fn release_hold(
    app: &App,
    caller: &Caller,
    identity: &Identity,
    body: &Json,
) -> Result<Json, ApiError> {
    refuse_other_tenant(app, identity, body)?;
    let actor = require_owner(app, identity)?;
    stepup::require_fresh(app, caller, identity)?;
    let id = body
        .get("hold")
        .and_then(Json::as_str)
        .ok_or_else(|| invalid("Say which hold."))?;
    let text = app
        .store
        .doc_get(&identity.tenant, HOLD_KIND, id)?
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "No such hold."))?;
    let mut hold: Hold = serde_json::from_str(&text)
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, e.to_string()))?;
    if hold.released_at.is_none() {
        hold.released_at = Some(app.now().0);
        let text = serde_json::to_string(&hold)
            .map_err(|e| ApiError::hidden(ErrorCode::Internal, e.to_string()))?;
        app.store
            .doc_put(&identity.tenant, HOLD_KIND, id, &text, false)?;
        app.audit(
            "subject_rights.hold_released",
            &identity.correlation,
            json!({ "tenant": identity.tenant.as_str(), "hold": id, "by": actor.as_str() }),
        );
    }
    Ok(json!({ "hold": hold }))
}

/// The legal holds of the tenant, active and released, newest first: `list-legal-holds` (CT-1707).
///
/// Implements: CT-1707
pub fn list_holds(app: &App, identity: &Identity) -> Result<Json, ApiError> {
    require_owner(app, identity)?;
    let mut holds = read_docs::<Hold>(app, &identity.tenant, HOLD_KIND)?;
    holds.sort_by(|a, b| b.set_at.cmp(&a.set_at).then(a.id.cmp(&b.id)));
    Ok(json!({ "holds": holds }))
}

/// The invitations still pending that are addressed to the verified address of `subject` (WG-5117).
/// The kernel holds no address of a person, so the shell finds them, from the address the store
/// holds for the account, and hands the kernel their ids; the erasure revokes each and clears its
/// address. It must run before the account is removed.
pub fn addressed_invitations(
    app: &App,
    tenant: &TenantId,
    subject: &PrincipalId,
) -> Result<Vec<strata_kernel::InvitationId>, ApiError> {
    let Some(address) = app.store.verified_address(tenant, subject)? else {
        return Ok(Vec::new());
    };
    let address = strata_kernel::normalise_address(&address);
    let Some(state) = app.store.load(tenant, &[])?.tenant else {
        return Ok(Vec::new());
    };
    Ok(state
        .schema
        .all_invitations()
        .filter(|offer| {
            offer.redeemed_by.is_none() && offer.revoked_by.is_none() && offer.address == address
        })
        .map(|offer| offer.id.clone())
        .collect())
}

/// End the restriction of an erased principal (`rec-restriction`: until lifted or erased) and say
/// whether there was one.
pub fn end_with_erasure(
    app: &App,
    tenant: &TenantId,
    subject: &PrincipalId,
) -> Result<bool, ApiError> {
    Ok(app
        .store
        .doc_remove(tenant, RESTRICTION_KIND, subject.as_str())?)
}

/// `/internal/subject-rights/<operation>`.
///
/// Implements: CT-210, CT-212, CT-215, CT-1705, CT-1706, CT-1707, CT-1708
pub async fn route(
    axum::extract::State(app): axum::extract::State<crate::app::Shared>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(operation): axum::extract::Path<String>,
    axum::Json(body): axum::Json<Json>,
) -> Result<axum::Json<Json>, ApiError> {
    crate::http::as_caller_at(&app, &headers, move |app, caller, identity| {
        handle(app, caller, identity, &operation, &body)
    })
    .await
}

/// Serve one operation.
pub fn handle(
    app: &App,
    caller: &Caller,
    identity: &Identity,
    operation: &str,
    body: &Json,
) -> Result<Json, ApiError> {
    match operation {
        "export" => crate::subject_export::request(app, caller, identity, body),
        "export-fetch" => crate::subject_export::fetch(app, caller, identity, body),
        "restrict" => restrict(app, caller, identity, body),
        "lift-restriction" => lift(app, caller, identity, body),
        "place-hold" => place_hold(app, caller, identity, body),
        "release-hold" => release_hold(app, caller, identity, body),
        "holds" => list_holds(app, identity),
        "restrictions" => restrictions(app, identity),
        _ => Err(ApiError::new(ErrorCode::NotFound, "No such operation.")),
    }
}
