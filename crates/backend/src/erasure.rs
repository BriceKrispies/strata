//! Erasure of one data subject across a tenant: the contract `erase-subject` (CT-213) and the
//! reading of its receipts (`list-erasure-receipts`).
//!
//! The kernel erases the records and the memberships of the subject, a batch of at most
//! [`strata_kernel::erasure::BATCH`] records at a time (the module `strata_kernel::erasure`).
//! This module repeats the kernel's intent until it says nothing more is to do, then removes what
//! holds the person outside the records, and writes a receipt that says what was reached and what
//! was not.
//!
//! What is reached, by table (the `erasure` text of each spec record decides):
//!
//! - records and their index rows (`tbl-records`): by the kernel, in the same commit as the
//!   record; members, teams and invitations with them (the tenant row and the schema);
//! - every login and credential of the principal (`tbl-accounts`, `tbl-passkeys`,
//!   `tbl-passkey-challenges`, `tbl-federated-identities`, `tbl-totp-factors`,
//!   `tbl-recovery-codes`, `tbl-pending-sign-ins`, `tbl-sign-in-links`, `tbl-recovery-requests`,
//!   the sign-in flows started to link the account): `Store::remove_principal`, in one step;
//! - sessions (`tbl-sessions`, `tbl-session-directory`): revoked with the commit that removes the
//!   roles; the rows are purged by the janitor a day after they end, which is their erasure point;
//! - identity documents (`tbl-identity-docs`): the SCIM user document, the deactivation flag and
//!   the place in each provisioned group are removed, and delegations to agents are revoked and
//!   lose their purpose text;
//! - the ledger (`Store::anonymise_ledger_principal`): the principal is cleared from the accounts
//!   and entries, every amount and balance kept;
//! - segments (`strata_storage_api::segments::erase`): memberships end with a sealed exit event,
//!   wake rows go, and the person is marked erased;
//! - the subject key (`crate::subject_keys`, D-350): destroyed last, which makes every value
//!   sealed under it unreadable in every copy that holds it: the records and their index rows,
//!   the events and the outbox, the intents in the journal, the snapshots, and the members of
//!   audit lines that name the person. Nothing of those copies is rewritten, so the journal's
//!   hashes and the audit chain verify as before (PRV-359, PRV-360). The destruction is an audit
//!   line naming the tenant and the key ids, and no key material (PRV-362).
//!
//! What is not reached is named in the receipt, each with the task that closes it: values written
//! before sealing was in force for the tenant (they are not re-sealed in place), values whose
//! subject the store cannot resolve, intents and job parameters other than record field values,
//! backup images and exports, the schema in snapshots, and the rest listed in [`not_reached`].
//! The audit log is never rewritten (PRV-359).
//!
//! Implements: CT-213, PRV-353, PRV-1588

use crate::app::{ApiError, App};
use crate::auth::{Caller, Identity};
use crate::ops;
use crate::service::scim;
use crate::signin::stepup;
use crate::subject_rights::{addressed_invitations, end_with_erasure, Rights};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use std::collections::BTreeSet;
use strata_kernel::erasure::{ErasedRecord, BATCH};
use strata_kernel::rules::roles_of;
use strata_kernel::{
    owner_role, Context, Intent, InvitationId, ObjectTypeId, Output, PrincipalId, RecordId, TeamId,
};
use strata_protocol::api::ErrorCode;
use strata_storage_api::PrincipalRemoval;

/// The kind of identity document that holds the receipts of a tenant.
pub const RECEIPT_KIND: &str = "erasure-receipt";

/// The most batches one request runs. A request that needs more stops with a conflict; repeating
/// it carries on, because erasure is idempotent.
const MAX_BATCHES: u32 = 10_000;

/// The receipt of one erasure request (PRV-353): the specification type `erasure-receipt`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    /// The correlation id of the request.
    pub request: String,
    pub principal: String,
    /// When the erasure ended, in milliseconds.
    pub at: i64,
    /// The ids of the records changed.
    pub records: Vec<u64>,
    /// Each record changed, with the fields cleared (the names of fields, never their values).
    pub changed: Vec<ErasedRecord>,
    /// Each copy reached, and the erasure point it reached.
    pub reached: Vec<String>,
    /// Each copy not reached, why, and the task that closes it.
    pub not_reached: Vec<String>,
    /// True only when no copy is left unreached. Today it is always false.
    pub complete: bool,
    /// How many commits the kernel's batches took.
    pub batches: u32,
    /// The most records one batch changes.
    pub batch_bound: u64,
}

/// What one decision of the kernel's erasure says.
struct Batch {
    changed: Vec<ErasedRecord>,
    member_removed: bool,
    teams: Vec<TeamId>,
    invitations: Vec<InvitationId>,
    not_erased: Vec<String>,
    more: bool,
    next: (Option<ObjectTypeId>, Option<RecordId>),
}

impl Batch {
    fn of(output: Output) -> Result<Batch, ApiError> {
        match output {
            Output::Erased {
                changed,
                member_removed,
                teams,
                invitations,
                not_erased,
                more,
                next_object,
                next_after,
            } => Ok(Batch {
                changed,
                member_removed,
                teams,
                invitations,
                not_erased,
                more,
                next: (next_object, next_after),
            }),
            other => Err(ApiError::hidden(
                ErrorCode::Internal,
                format!("unexpected output of an erasure: {other:?}"),
            )),
        }
    }

    /// Whether the batch changes the tenant.
    fn has_effect(&self) -> bool {
        !self.changed.is_empty()
            || self.member_removed
            || !self.teams.is_empty()
            || !self.invitations.is_empty()
    }
}

fn not_found() -> ApiError {
    // The same answer whether the person never existed here or belongs to another tenant
    // (PRV-1588).
    ApiError::new(ErrorCode::NotFound, "No such person.")
}

/// The kernel's decision of one batch, made and not committed.
fn plan(app: &App, ctx: &Context, intent: &Intent) -> Result<Batch, ApiError> {
    let header = app.store.load(&ctx.tenant, &[])?;
    let wanted = strata_kernel::needs(header.tenant.as_deref().map(|t| t.schema.as_ref()), intent);
    let state = app.store.load(&ctx.tenant, &wanted)?;
    let transition = strata_kernel::transition(&state, intent, ctx)
        .map_err(|error| ApiError::domain(error, false))?;
    Batch::of(transition.output)
}

/// The receipts of a tenant, oldest first, as stored.
fn stored_receipts(
    app: &App,
    identity_tenant: &strata_kernel::TenantId,
) -> Result<Vec<Json>, ApiError> {
    let mut found: Vec<(String, Json)> = Vec::new();
    for (key, text) in app.store.doc_list(identity_tenant, RECEIPT_KIND)? {
        if let Ok(value) = serde_json::from_str::<Json>(&text) {
            found.push((key, value));
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(found.into_iter().map(|(_, value)| value).collect())
}

/// Whether the tenant holds anything of the principal that the erasure reaches. A principal that
/// was erased before is still known, through its receipt, so that repeating a request is not an
/// error.
fn is_known(
    app: &App,
    ctx: &Context,
    subject: &PrincipalId,
    first: &Batch,
) -> Result<bool, ApiError> {
    let header = app
        .store
        .load(&ctx.tenant, &[])?
        .tenant
        .ok_or_else(not_found)?;
    if header.members.contains_key(subject)
        || header
            .schema
            .teams
            .iter()
            .flatten()
            .any(|(_, team)| team.members.contains(subject))
        || header
            .schema
            .invitations
            .iter()
            .flatten()
            .any(|(_, offer)| {
                offer.created_by == *subject || offer.redeemed_by.as_ref() == Some(subject)
            })
        || first.has_effect()
        || app.store.account_of(&ctx.tenant, subject)?.is_some()
        || !app
            .store
            .live_sessions_of(&ctx.tenant, Some(subject), ctx.now)?
            .is_empty()
    {
        return Ok(true);
    }
    Ok(stored_receipts(app, &ctx.tenant)?
        .iter()
        .any(|receipt| receipt["principal"] == subject.as_str()))
}

/// The documents of the identity service that hold the person, removed: what each removal reached.
#[derive(Default)]
struct Documents {
    user_document: bool,
    deactivation_flag: bool,
    group_places: u64,
    delegations: u64,
}

fn erase_documents(app: &App, ctx: &Context, subject: &PrincipalId) -> Result<Documents, ApiError> {
    let tenant = &ctx.tenant;
    let key = subject.as_str();
    let mut done = Documents {
        user_document: app.store.doc_remove(tenant, scim::USER, key)?,
        deactivation_flag: app.store.doc_remove(tenant, scim::DISABLED, key)?,
        ..Documents::default()
    };
    for (group, text) in app.store.doc_list(tenant, scim::GROUP_MEMBERS)? {
        let Ok(mut members) = serde_json::from_str::<Vec<String>>(&text) else {
            continue;
        };
        let before = members.len();
        members.retain(|member| member != key);
        if members.len() != before {
            let kept = serde_json::to_string(&members)
                .map_err(|e| ApiError::hidden(ErrorCode::Internal, e.to_string()))?;
            app.store
                .doc_put(tenant, scim::GROUP_MEMBERS, &group, &kept, false)?;
            done.group_places += 1;
        }
    }
    for (id, text) in app
        .store
        .doc_list(tenant, crate::service::tokens::GRANT_KIND)?
    {
        let Ok(mut grant) = serde_json::from_str::<Json>(&text) else {
            continue;
        };
        if grant["principal"] != key {
            continue;
        }
        if grant["revoked_at"].is_null() {
            grant["revoked_at"] = json!(ctx.now.0);
        }
        grant["purpose"] = json!("");
        app.store.doc_put(
            tenant,
            crate::service::tokens::GRANT_KIND,
            &id,
            &grant.to_string(),
            false,
        )?;
        done.delegations += 1;
    }
    Ok(done)
}

/// The form a segment exit event names an erased person in: `sealed:` and the start of a hash of
/// the tenant and the person, so the event says that someone left without saying who (SEG-038).
fn sealed_principal(
    tenant: &strata_kernel::TenantId,
    subject: &PrincipalId,
) -> Result<PrincipalId, ApiError> {
    let hash = strata_kernel::canon::hash_bytes(
        format!("strata/sealed-principal/v1\0{tenant}\0{subject}").as_bytes(),
    );
    PrincipalId::new(format!("sealed:{}", &hash[..16]))
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, e.to_string()))
}

/// The copies the erasure does not reach, each with why and the task that closes it.
/// `sealing_since` is when the tenant's first value was sealed, `None` when none ever was.
fn not_reached(unbuilt: Vec<String>, sealing_since: Option<i64>) -> Vec<String> {
    let since = match sealing_since {
        Some(at) => format!("sealing has been in force for this tenant since {at} (milliseconds)"),
        None => "nothing of this tenant was ever sealed".to_string(),
    };
    let mut out = vec![
        format!("values written before sealing (events and outbox tbl-events, journal tbl-journal, snapshots tbl-snapshots): {since}; a value written before then, or before its field was declared personal, stays plain in the events, journal entries and snapshots written then, because stored history is not re-sealed in place (D-7724); the live records were cleared above"),
        "personal fields whose subject the store cannot resolve (a subject field that refers to the record of a person, a record with no creator): stored plain, so no key reaches them; closes with WG-5116".to_string(),
        "journal intents other than create-record and update-record (the inputs of invoke-operation, the address of create-invitation), automation job parameters and run records: not sealed; closes with WG-5245".to_string(),
        "snapshots (tbl-snapshots): their records are sealed, but each keeps the members, teams and invitations of the tenant as they were, and a restore brings those back until the restore re-applies this erasure from the audit log (D-6206, D-7727); closes with WG-5246".to_string(),
        "backup images of the tenant file (copy-tenant-backup): an image taken before the key destruction holds the subject key wrapped under the tenant key, so it opens the sealed fields until it expires at the end of its retention of 35 days (D-351); a restore re-applies the destruction from the audit log (D-6206); exports (copy-tenant-export) are not built and will hold sealed fields and wrapped keys only (D-7727); closes with WG-5246 and WG-5248".to_string(),
        "audit log (tbl-audit-log): lines that name the person only in a member other than principal, actor, subject, by and owner, or that hold a login or an address with no principal named, are not sealed; the log is never rewritten (PRV-359)".to_string(),
        "idempotency keys (tbl-idempotency): a row keeps the hash of the plain request and its result for 24 hours, and is removed by the first commit of the tenant after that".to_string(),
        "sign-in throttle counters (tbl-login-throttle): keyed by a hash of the login, removed by the sweep after their window".to_string(),
        "files of personal file fields: the field value was cleared, no stored bytes were removed; closes with WG-5118".to_string(),
    ];
    out.extend(unbuilt);
    out
}

/// Erase one data subject from a tenant, as the owner the context names, and return the receipt.
///
/// Idempotent: a repeat changes nothing that was erased and says what it found. The principal
/// must be known to the tenant, or have been erased by an earlier request.
///
/// Implements: CT-213, PRV-353, PRV-1588
pub fn erase_subject(app: &App, ctx: &Context, subject: &PrincipalId) -> Result<Receipt, ApiError> {
    let tenant = &ctx.tenant;
    // Legal holds and the invitations addressed to the person (subject_rights): read before any
    // change, so a held person is refused before a record is touched.
    let rights = Rights::load(app, tenant)?;
    let held_objects = rights.held_objects();
    let addressed = addressed_invitations(app, tenant, subject)?;
    let first_intent = Intent::EraseSubject {
        subject: subject.clone(),
        resume_object: None,
        resume_after: None,
        held_objects: held_objects.clone(),
        addressed_invitations: addressed.clone(),
    };
    let first = plan(app, ctx, &first_intent)?;
    if !is_known(app, ctx, subject, &first)? {
        return Err(not_found());
    }
    let _files = crate::files::erasure::erase_subject_files(app, ctx, subject)?;
    rights.refuse_if_held(subject)?;
    let sessions_before = app
        .store
        .live_sessions_of(tenant, Some(subject), ctx.now)?
        .len();

    let mut changed: Vec<ErasedRecord> = Vec::new();
    let mut not_erased: BTreeSet<String> = BTreeSet::new();
    let mut teams: BTreeSet<String> = BTreeSet::new();
    let mut invitations: BTreeSet<String> = BTreeSet::new();
    let mut member_removed = false;
    let mut resume: (Option<ObjectTypeId>, Option<RecordId>) = (None, None);
    let mut pending = Some(first);
    let mut commits = 0u32;
    let mut rounds = 0u32;
    loop {
        rounds += 1;
        if rounds > MAX_BATCHES {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                "There is more to erase than one request does. Repeat the request.",
            ));
        }
        let intent = Intent::EraseSubject {
            subject: subject.clone(),
            resume_object: resume.0.clone(),
            resume_after: resume.1,
            held_objects: held_objects.clone(),
            addressed_invitations: addressed.clone(),
        };
        let planned = match pending.take() {
            Some(first) => first,
            None => plan(app, ctx, &intent)?,
        };
        let done = if planned.has_effect() {
            let committed = ops::run_intent(app, ctx, &intent, false)?;
            commits += 1;
            Batch::of(committed.transition.output)?
        } else {
            planned
        };
        not_erased.extend(done.not_erased);
        member_removed |= done.member_removed;
        teams.extend(done.teams.iter().map(|t| t.as_str().to_string()));
        invitations.extend(done.invitations.iter().map(|i| i.as_str().to_string()));
        changed.extend(done.changed);
        if !done.more {
            break;
        }
        resume = done.next;
    }

    // What holds the person outside the records: every login and credential in one store step,
    // then whatever session was opened meanwhile, then the documents of the identity service.
    let removed: PrincipalRemoval = app.store.remove_principal(tenant, subject)?;
    scim::end_sessions(app, tenant, subject, &ctx.correlation)?;
    let documents = erase_documents(app, ctx, subject)?;
    // The ledger keeps every amount, key and balance, and loses the person (LDG-027, LDG-028).
    let ledger = app.store.anonymise_ledger_principal(tenant, subject)?;
    // The segments: every membership ends with an exit event that names the person sealed, the
    // wake rows go, and the person is marked so that no later decision enters them (SEG-036 to
    // SEG-038).
    let sealed = sealed_principal(tenant, subject)?;
    let segment_exits =
        strata_storage_api::segments::erase(app.store.as_ref(), ctx, subject, &sealed)
            .map_err(|error| ApiError::execute(error, false))?;
    // Last, the subject key: every value sealed under it, in every copy, is unreadable from here
    // on (D-350, PRV-358). Nothing is rewritten, so the journal's hashes and the audit chain
    // verify as before (PRV-359, PRV-360).
    let sealing_since = app.keys.sealing_since(app.store.as_ref(), tenant)?;
    let destroyed = app
        .keys
        .destroy(app.store.as_ref(), tenant, subject, ctx.now)?;
    if !destroyed.is_empty() {
        // The destruction is evidenced with the tenant and the key ids, never the key (PRV-362).
        app.audit(
            "erasure.subject_key_destroyed",
            &ctx.correlation,
            json!({
                "tenant": tenant.as_str(),
                "keys": destroyed,
                "destroyed_at": ctx.now.0,
            }),
        );
    }

    let mut fields: BTreeSet<String> = BTreeSet::new();
    for record in &changed {
        for field in &record.fields {
            fields.insert(format!("{}.{}", record.object, field));
        }
    }
    let mut reached = vec![
        format!(
            "records and their index rows (tbl-records): {} changed, the fields cleared are {:?}; erasure point: the commit of each batch ({commits} commits, at most {BATCH} records each)",
            changed.len(),
            fields,
        ),
        "events of the erasure: the events of the changed records carry no personal value (before images have the erased fields removed); erasure point: their commit".to_string(),
        if destroyed.is_empty() {
            "subject keys (rec-subject-key, kept in tbl-identity-docs): the person had none, so nothing of theirs was sealed".to_string()
        } else {
            format!(
                "subject keys (rec-subject-key, kept in tbl-identity-docs): {} destroyed ({}); every value sealed under them is unreadable from this point in every copy the platform reads: the records and their index rows (the blind hashes of searchable fields went with the record rewrite above), the events and the outbox, the field values of the intents in the journal, the snapshots, and the members of the audit lines that name the person; the journal's hashes and the audit chain cover the sealed form and still verify; erasure point: the destruction",
                destroyed.len(),
                destroyed.join(", "),
            )
        },
        "schema history (schema_history in the tenant file): it keeps no teams and no invitations (D-7726), so no older version names the person".to_string(),
        format!(
            "membership (tenant members, teams, invitations): roles removed {member_removed}, {} teams left, {} invitations revoked or cleared; erasure point: the commit of the first batch",
            teams.len(),
            invitations.len()
        ),
        format!(
            "logins and credentials (tbl-accounts, tbl-passkeys, tbl-passkey-challenges, tbl-federated-identities, tbl-totp-factors, tbl-recovery-codes, tbl-pending-sign-ins, tbl-sign-in-links, tbl-recovery-requests, sign-in flows): {} logins, {} passkeys, {} passkey challenges, {} identities, {} authenticator factors, {} recovery codes, {} waiting sign-ins, {} sign-in links, {} recoveries, {} provider flows removed by Store::remove_principal",
            removed.accounts,
            removed.passkeys,
            removed.passkey_challenges,
            removed.identities,
            removed.totp_factors,
            removed.recovery_codes,
            removed.pending_sign_ins,
            removed.sign_in_links,
            removed.recovery_requests,
            removed.provider_flows,
        ),
        format!(
            "sessions (tbl-sessions, tbl-session-directory): {sessions_before} live sessions ended; the rows are purged a day after they end (rec-session)"
        ),
        format!(
            "identity documents (tbl-identity-docs): SCIM user document removed {}, deactivation flag removed {}, {} group places removed, {} delegations revoked and cleared of their purpose",
            documents.user_document,
            documents.deactivation_flag,
            documents.group_places,
            documents.delegations,
        ),
        format!(
            "ledger (tbl-ledger-accounts, tbl-ledger-entries): the principal cleared from {} accounts it owns and {} entries it caused, every amount, key, reason, number, time and balance kept; erasure point: Store::anonymise_ledger_principal",
            ledger.accounts, ledger.entries,
        ),
        format!(
            "segments (tbl-segment-memberships, tbl-segment-wakes, tbl-segment-erased): {segment_exits} memberships ended with an exit event naming the person sealed, the wake rows removed, and the person marked so that no later decision enters them; erasure point: the segment write"
        ),
    ];
    // Restrictions and legal holds (PRV-352, PRV-355): what was in force, and the restriction ends
    // with the erasure.
    reached.extend(rights.receipt_lines(subject));
    end_with_erasure(app, tenant, subject)?;
    let receipt = Receipt {
        request: ctx.correlation.clone(),
        principal: subject.as_str().to_string(),
        at: ctx.now.0,
        records: changed.iter().map(|record| record.id.0).collect(),
        changed,
        reached,
        not_reached: not_reached(not_erased.into_iter().collect(), sealing_since),
        complete: false,
        batches: commits,
        batch_bound: BATCH as u64,
    };
    let text = serde_json::to_string(&receipt)
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, e.to_string()))?;
    app.store.doc_put(
        tenant,
        RECEIPT_KIND,
        &format!("{:020}-{}", ctx.now.0.max(0), ctx.correlation),
        &text,
        false,
    )?;
    // The audit entry names the request and counts, never the person: the log is never rewritten.
    app.audit(
        "erasure.completed",
        &ctx.correlation,
        json!({
            "tenant": tenant.as_str(),
            "records": receipt.records.len(),
            "credentials": removed.passkeys + removed.identities + removed.totp_factors
                + removed.recovery_codes,
            "accounts": removed.accounts,
        }),
    );
    Ok(receipt)
}

fn require_owner(app: &App, identity: &Identity) -> Result<(), ApiError> {
    if identity.anonymous() {
        return Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in to do this.",
        ));
    }
    let tenant = ops::header(app, identity)?;
    if roles_of(&tenant, &identity.principal).contains(&owner_role()) {
        Ok(())
    } else {
        Err(ApiError::new(
            ErrorCode::Forbidden,
            "Only an owner of the tenant may do this.",
        ))
    }
}

/// A request that names a tenant other than the one it is made at is refused, and the refusal is
/// an audit entry (TEN-1140).
fn refuse_other_tenant(app: &App, identity: &Identity, body: &Json) -> Result<(), ApiError> {
    match body.get("tenant").and_then(Json::as_str) {
        Some(named) if named != identity.tenant.as_str() => {
            app.audit(
                "erasure.tenant_mismatch",
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

/// `POST /internal/erasure/erase`: erase one principal, as the signed-in owner, behind step-up.
///
/// Implements: CT-213
pub fn erase(
    app: &App,
    caller: &Caller,
    identity: &Identity,
    body: &Json,
) -> Result<Json, ApiError> {
    if identity.anonymous() {
        return Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in to do this.",
        ));
    }
    refuse_other_tenant(app, identity, body)?;
    require_owner(app, identity)?;
    stepup::require_fresh(app, caller, identity)?;
    let subject = body
        .get("principal")
        .and_then(Json::as_str)
        .and_then(|name| PrincipalId::new(name).ok())
        .ok_or_else(not_found)?;
    let receipt = erase_subject(app, &identity.context(app), &subject)?;
    Ok(json!({ "receipt": receipt }))
}

/// `POST /internal/erasure/receipts`: the receipts of the tenant, newest first, optionally of one
/// principal. Readable by an owner only.
///
/// Implements: CT-1495
pub fn receipts(app: &App, identity: &Identity, body: &Json) -> Result<Json, ApiError> {
    require_owner(app, identity)?;
    let only = body.get("principal").and_then(Json::as_str);
    let mut found = stored_receipts(app, &identity.tenant)?;
    found.retain(|receipt| only.is_none_or(|name| receipt["principal"] == name));
    found.reverse();
    found.truncate(100);
    Ok(json!({ "receipts": found }))
}
