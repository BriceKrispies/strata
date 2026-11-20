//! The export of the data of one principal (contract `export-subject-data`, CT-210; the fetch of
//! the result, `fetch-subject-export`, CT-1708; PRV-350, PRV-1586).
//!
//! **Delivery.** A request (`POST /internal/subject-rights/export`) starts a job and answers with
//! the id of the export. The result is fetched by id (`export-fetch`) once the job has ended, and
//! only by the subject themselves or by an owner of the tenant, each after a fresh step-up
//! (IDN-019); anyone else is refused. The result expires [`TTL_MS`] after the request; after that
//! the fetch answers not found and the documents are removed. Each request and each fetch is an
//! audit entry that names the tenant and the export, never the person.
//!
//! **Production.** The job (`JobWork::SubjectExport`) reads the records of the subject in bounded
//! batches through the very enumeration the erasure uses (`strata_kernel::subject_export`, which
//! shares `declared`, the needs and the test of whether a field belongs to the subject with
//! `strata_kernel::erasure`): at most `BATCH` records per job, each batch kept as a part, and the
//! next batch created by the report of the one before it. The last batch assembles the document
//! from the parts, adds what is not a record, writes the result and removes the parts.
//!
//! **The document** (`format` `strata-subject-export/1`): one JSON object whose members are, in
//! this order and all always present:
//!
//! - `format`, `tenant`, `subject`, `generated_at` (milliseconds);
//! - `restriction`: `null`, or the restriction in force (`since`, `reason`);
//! - `account`: the login, display name and verified address, or `null`; never a password hash;
//! - `credentials`: `passkeys` (id, label, `created_at`, `last_used_at`), `authenticator`
//!   (`confirmed`, `created_at`, or `null`) and `recovery_codes_left`; never a secret, a public
//!   key or a code;
//! - `sessions`: the live sessions (id and times), never a token or a request token;
//! - `membership`: `roles`, `teams` (id, roles), `invitations_issued` (id, roles, `redeemed`,
//!   `revoked`; never the address, which is another person's) and `invitations_redeemed` (id,
//!   roles, the address the person used);
//! - `records`: every record the erasure would change, in `(object, id)` order, each with `object`,
//!   `id`, `version`, `created_at`, `updated_at`, `created_by` (only when a field of the record is
//!   personal to its creator and the subject is that creator) and `fields`: exactly the personal
//!   fields whose subject is the principal, as plain JSON (timestamps in milliseconds, references
//!   as ids); no field of another subject;
//! - `ledger`: `accounts` the principal owns (account, unit, balance, `opened_at`) with their
//!   `entries`, and the `entries_caused` by the principal on any other account; a `caused_by` is
//!   shown only when it is the principal;
//! - `segments`: the memberships (segment, `since`);
//! - `settings`: the values set for the person (setting, value, `set_at`);
//! - `inbox`: always `null`, this tree holds no inbox;
//! - `not_included`: what is not in the export and why (the identities at identity providers have
//!   no listing by principal, the events and the journal are not records of the subject).
//!
//! The document is deterministic: the same state and the same request time give the same bytes;
//! every list is in a fixed order and every object is written with its members in a fixed order.
//!
//! Implements: CT-210, CT-1708, PRV-350, PRV-1586

use crate::app::{ApiError, App};
use crate::auth::{Caller, Identity};
use crate::signin::stepup;
use crate::subject_rights::{is_owner, known_principal, refuse_other_tenant, signed_in, Rights};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value as Json};
use std::collections::BTreeMap;
use strata_kernel::ledger::account_key;
use strata_kernel::rules::Lookup;
use strata_kernel::subject_export::{export_needs, export_page, ExportCursor, ExportedRecord};
use strata_kernel::{canon, PrincipalId, TenantId, TenantSchemaExt, Timestamp};
use strata_protocol::api::ErrorCode;
use strata_storage_api::jobs::{JobReport, JobWork, Lease, NewJob, RetryPolicy, WorkerWrites};
use strata_substrate::plain::to_plain;

/// The kind of identity document that holds the requests, by export id.
pub const REQUEST_KIND: &str = "subject-export-request";
/// The kind that holds the parts of an export while it is built, by `<export>/<batch>`.
pub const PART_KIND: &str = "subject-export-part";
/// The kind that holds the finished documents, by export id.
pub const RESULT_KIND: &str = "subject-export";
/// How long a result can be fetched: seven days, in milliseconds.
pub const TTL_MS: i64 = 7 * 86_400_000;
/// The name and version of the format of the document.
pub const FORMAT: &str = "strata-subject-export/1";

/// One request, kept under its id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportRequest {
    pub id: String,
    pub subject: String,
    pub requested_by: String,
    pub requested_at: i64,
    pub expires_at: i64,
    /// `queued` until the last batch has run, then `ready`, or `failed`.
    pub state: String,
    /// Batches run so far.
    pub batches: u64,
    /// Records in the document, once ready.
    pub records: u64,
    #[serde(default)]
    pub error: Option<String>,
}

fn internal(e: impl ToString) -> ApiError {
    ApiError::hidden(ErrorCode::Internal, e.to_string())
}

fn read_request(app: &App, tenant: &TenantId, id: &str) -> Result<Option<ExportRequest>, ApiError> {
    Ok(app
        .store
        .doc_get(tenant, REQUEST_KIND, id)?
        .and_then(|text| serde_json::from_str(&text).ok()))
}

fn write_request(app: &App, tenant: &TenantId, request: &ExportRequest) -> Result<(), ApiError> {
    let text = serde_json::to_string(request).map_err(internal)?;
    app.store
        .doc_put(tenant, REQUEST_KIND, &request.id, &text, false)?;
    Ok(())
}

/// Who may have the export of `subject`: the subject, or an owner, each with a fresh step-up.
fn authorise(
    app: &App,
    caller: &Caller,
    identity: &Identity,
    subject: &PrincipalId,
) -> Result<PrincipalId, ApiError> {
    let actor = signed_in(identity)?;
    if actor != *subject && !is_owner(app, identity)? {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "Only the person or an owner of the tenant may do this.",
        ));
    }
    stepup::require_fresh(app, caller, identity)?;
    Ok(actor)
}

/// Request the export: `export-subject-data` (CT-210). The body may name `principal`; without it
/// the caller exports their own data.
///
/// Implements: CT-210
pub fn request(
    app: &App,
    caller: &Caller,
    identity: &Identity,
    body: &Json,
) -> Result<Json, ApiError> {
    refuse_other_tenant(app, identity, body)?;
    let me = signed_in(identity)?;
    let subject = if body.get("principal").is_some() {
        // Another person's data is an owner's to ask for: the refusal comes before any answer
        // about whether that person exists.
        let named = body
            .get("principal")
            .and_then(Json::as_str)
            .and_then(|name| PrincipalId::new(name).ok());
        if named.as_ref() != Some(&me) && !is_owner(app, identity)? {
            return Err(ApiError::new(
                ErrorCode::Forbidden,
                "Only the person or an owner of the tenant may do this.",
            ));
        }
        if named.as_ref() == Some(&me) {
            me.clone()
        } else {
            known_principal(app, identity, body, "principal")?
        }
    } else {
        me.clone()
    };
    let actor = authorise(app, caller, identity, &subject)?;
    let tenant = &identity.tenant;
    let now = app.now();
    let id = format!(
        "exp-{}",
        &canon::hash_bytes(
            format!(
                "strata/subject-export/v1\0{tenant}\0{}\0{}",
                identity.correlation, now.0
            )
            .as_bytes()
        )[..24]
    );
    let record = ExportRequest {
        id: id.clone(),
        subject: subject.as_str().to_string(),
        requested_by: actor.as_str().to_string(),
        requested_at: now.0,
        expires_at: now.0 + TTL_MS,
        state: "queued".to_string(),
        batches: 0,
        records: 0,
        error: None,
    };
    write_request(app, tenant, &record)?;
    app.store.jobs_create(
        tenant,
        &[NewJob {
            key: format!("export:{id}:0"),
            work: JobWork::SubjectExport {
                export: id.clone(),
                subject,
                seq: 0,
                cursor: ExportCursor {
                    phase: 1,
                    object: None,
                    after: None,
                },
            },
            due_at: now,
            policy: RetryPolicy::default(),
        }],
        now,
    )?;
    app.audit(
        "subject_export.requested",
        &identity.correlation,
        json!({ "tenant": tenant.as_str(), "export": id }),
    );
    Ok(json!({ "export": id, "state": "queued", "expires_at": record.expires_at }))
}

/// Fetch an export: `fetch-subject-export` (CT-1708). While the job runs it answers with the
/// state; when ready, with the document.
///
/// Implements: CT-1708
pub fn fetch(
    app: &App,
    caller: &Caller,
    identity: &Identity,
    body: &Json,
) -> Result<Json, ApiError> {
    refuse_other_tenant(app, identity, body)?;
    signed_in(identity)?;
    let tenant = &identity.tenant;
    let id = body
        .get("export")
        .and_then(Json::as_str)
        .ok_or_else(|| ApiError::new(ErrorCode::Invalid, "Say which export."))?;
    let missing = || ApiError::new(ErrorCode::NotFound, "No such export, or it has expired.");
    let request = read_request(app, tenant, id)?.ok_or_else(missing)?;
    let subject = PrincipalId::new(request.subject.as_str()).map_err(internal)?;
    let actor = authorise(app, caller, identity, &subject)?;
    if app.now().0 >= request.expires_at {
        remove_documents(app, tenant, id)?;
        return Err(missing());
    }
    app.audit(
        "subject_export.fetched",
        &identity.correlation,
        // The person who fetches their own export is not named; an owner who fetches another's is.
        json!({
            "tenant": tenant.as_str(),
            "export": id,
            "by": (actor != subject).then(|| actor.as_str()),
        }),
    );
    if request.state != "ready" {
        return Ok(json!({
            "export": id,
            "state": request.state,
            "expires_at": request.expires_at,
            "error": request.error,
        }));
    }
    let text = app
        .store
        .doc_get(tenant, RESULT_KIND, id)?
        .ok_or_else(missing)?;
    let document: Json = serde_json::from_str(&text).map_err(internal)?;
    Ok(json!({
        "export": id,
        "state": "ready",
        "expires_at": request.expires_at,
        "document": document,
    }))
}

fn remove_documents(app: &App, tenant: &TenantId, id: &str) -> Result<(), ApiError> {
    app.store.doc_remove(tenant, REQUEST_KIND, id)?;
    app.store.doc_remove(tenant, RESULT_KIND, id)?;
    let prefix = format!("{id}/");
    for (key, _) in app.store.doc_list(tenant, PART_KIND)? {
        if key.starts_with(&prefix) {
            app.store.doc_remove(tenant, PART_KIND, &key)?;
        }
    }
    Ok(())
}

/// Remove every export of the tenant that has expired at `now`. Returns how many.
pub fn purge_expired(app: &App, tenant: &TenantId, now: Timestamp) -> Result<u64, ApiError> {
    let mut removed = 0;
    for (key, text) in app.store.doc_list(tenant, REQUEST_KIND)? {
        let expired = serde_json::from_str::<ExportRequest>(&text)
            .map(|request| now.0 >= request.expires_at)
            .unwrap_or(true);
        if expired {
            remove_documents(app, tenant, &key)?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// A record of the export as plain JSON, members in a fixed order.
fn record_json(record: &ExportedRecord) -> Json {
    let fields: Map<String, Json> = record
        .fields
        .iter()
        .map(|(id, value)| (id.as_str().to_string(), to_plain(value)))
        .collect();
    json!({
        "object": record.object.as_str(),
        "id": record.id.0,
        "version": record.version,
        "created_at": record.created_at.0,
        "updated_at": record.updated_at.0,
        "created_by": record.created_by.as_ref().map(|p| p.as_str()),
        "fields": fields,
    })
}

/// Run one batch of an export job.
pub fn run(
    app: &App,
    tenant: &TenantId,
    lease: &Lease,
    now: Timestamp,
) -> crate::worker::steps::Outcome {
    let JobWork::SubjectExport {
        export,
        subject,
        seq,
        cursor,
    } = &lease.job.work
    else {
        return crate::worker::steps::Outcome {
            report: JobReport::Succeeded,
            writes: WorkerWrites::default(),
        };
    };
    match batch(app, tenant, export, subject, *seq, cursor, now) {
        Ok(writes) => crate::worker::steps::Outcome {
            report: JobReport::Succeeded,
            writes,
        },
        Err(error) => {
            let last = lease.exhausted || lease.job.attempt >= lease.job.policy.attempts;
            if last {
                if let Ok(Some(mut request)) = read_request(app, tenant, export) {
                    request.state = "failed".to_string();
                    request.error = Some("the export could not be produced".to_string());
                    let _ = write_request(app, tenant, &request);
                }
            }
            crate::worker::steps::Outcome {
                report: JobReport::Failed {
                    error: strata_storage_api::jobs::clip_error(&error.describe()),
                    retry: !last,
                },
                writes: WorkerWrites::default(),
            }
        }
    }
}

fn batch(
    app: &App,
    tenant: &TenantId,
    export: &str,
    subject: &PrincipalId,
    seq: u64,
    cursor: &ExportCursor,
    now: Timestamp,
) -> Result<WorkerWrites, ApiError> {
    let Some(mut request) = read_request(app, tenant, export)? else {
        // Removed or expired meanwhile: nothing to produce.
        return Ok(WorkerWrites::default());
    };
    if request.state != "queued" || now.0 >= request.expires_at {
        return Ok(WorkerWrites::default());
    }
    let header = app.store.load(tenant, &[])?;
    let Some(state) = header.tenant else {
        return Err(ApiError::new(ErrorCode::NotFound, "No such tenant."));
    };
    let needs = export_needs(&state.schema, subject, cursor);
    let loaded = app.store.load(tenant, &needs)?;
    let Some(snapshot) = loaded.tenant.as_ref() else {
        return Err(ApiError::new(ErrorCode::NotFound, "No such tenant."));
    };
    let lookup = Lookup {
        tenant: snapshot,
        coverage: &loaded.coverage,
    };
    let page = export_page(lookup, subject, cursor)
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, format!("{e:?}")))?;
    let part: Vec<Json> = page.records.iter().map(record_json).collect();
    app.store.doc_put(
        tenant,
        PART_KIND,
        &format!("{export}/{seq:08}"),
        &Json::Array(part).to_string(),
        false,
    )?;
    request.batches = seq + 1;
    let mut writes = WorkerWrites::default();
    match page.next {
        Some(next) => {
            write_request(app, tenant, &request)?;
            writes.jobs.push(NewJob {
                key: format!("export:{export}:{}", seq + 1),
                work: JobWork::SubjectExport {
                    export: export.to_string(),
                    subject: subject.clone(),
                    seq: seq + 1,
                    cursor: next,
                },
                due_at: now,
                policy: RetryPolicy::default(),
            });
        }
        None => {
            let document = assemble(app, tenant, export, subject, now, &mut request)?;
            app.store
                .doc_put(tenant, RESULT_KIND, export, &document.to_string(), false)?;
            request.state = "ready".to_string();
            write_request(app, tenant, &request)?;
            let prefix = format!("{export}/");
            for (key, _) in app.store.doc_list(tenant, PART_KIND)? {
                if key.starts_with(&prefix) {
                    app.store.doc_remove(tenant, PART_KIND, &key)?;
                }
            }
        }
    }
    Ok(writes)
}

/// The document, from the parts and from everything else that holds the person.
fn assemble(
    app: &App,
    tenant: &TenantId,
    export: &str,
    subject: &PrincipalId,
    now: Timestamp,
    request: &mut ExportRequest,
) -> Result<Json, ApiError> {
    // The records: every part, one entry for each (object, id), in that order.
    let prefix = format!("{export}/");
    let mut records: BTreeMap<(String, u64), Json> = BTreeMap::new();
    for (key, text) in app.store.doc_list(tenant, PART_KIND)? {
        if !key.starts_with(&prefix) {
            continue;
        }
        let part: Vec<Json> = serde_json::from_str(&text).map_err(internal)?;
        for record in part {
            let object = record["object"].as_str().unwrap_or_default().to_string();
            let id = record["id"].as_u64().unwrap_or_default();
            records.insert((object, id), record);
        }
    }
    request.records = records.len() as u64;

    let complete = app.store.load_complete(tenant)?;
    let state = complete
        .tenant
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "No such tenant."))?;
    let rights = Rights::load_restrictions(app, tenant)?;

    let account = app.store.account_of(tenant, subject)?.map(|account| {
        json!({
            "login": account.login,
            "display_name": account.display_name,
            "verified_email": account.verified_email,
        })
    });
    let passkeys: Vec<Json> = app
        .store
        .passkeys(tenant, subject)?
        .iter()
        .map(|passkey| {
            json!({
                "id": passkey.id,
                "label": passkey.label,
                "created_at": passkey.created_at.0,
                "last_used_at": passkey.last_used_at.map(|t| t.0),
            })
        })
        .collect();
    let authenticator = app
        .store
        .totp(tenant, subject)?
        .map(|totp| json!({ "confirmed": totp.confirmed, "created_at": totp.created_at.0 }));
    let recovery_codes_left = app.store.recovery_codes_left(tenant, subject)?;
    let mut sessions: Vec<Json> = app
        .store
        .live_sessions_of(tenant, Some(subject), now)?
        .iter()
        .map(|session| {
            json!({
                "id": session.id.as_str(),
                "created_at": session.created_at.0,
                "last_seen_at": session.last_seen_at.0,
                "expires_at": session.expires_at.0,
                "authenticated_at": session.authenticated_at.0,
            })
        })
        .collect();
    sessions.sort_by_key(|s| s["id"].as_str().unwrap_or_default().to_string());

    // Membership.
    let roles: Vec<&str> = state
        .members
        .get(subject)
        .map(|roles| roles.iter().map(|r| r.as_str()).collect())
        .unwrap_or_default();
    let mut teams = Vec::new();
    for team in state.schema.all_teams() {
        if team.members.contains(subject) {
            teams.push(json!({
                "id": team.id.as_str(),
                "roles": team.roles.iter().map(|r| r.as_str()).collect::<Vec<_>>(),
            }));
        }
    }
    let mut issued = Vec::new();
    let mut redeemed = Vec::new();
    for offer in state.schema.all_invitations() {
        let offer_roles: Vec<&str> = offer.roles.iter().map(|r| r.as_str()).collect();
        if offer.created_by == *subject {
            issued.push(json!({
                "id": offer.id.as_str(),
                "roles": offer_roles,
                "redeemed": offer.redeemed_by.is_some(),
                "revoked": offer.revoked_by.is_some(),
            }));
        }
        if offer.redeemed_by.as_ref() == Some(subject) {
            redeemed.push(json!({
                "id": offer.id.as_str(),
                "roles": offer_roles,
                "address": offer.address,
            }));
        }
    }

    // The ledger: accounts the person owns with their entries, and entries the person caused.
    let mut accounts = Vec::new();
    let mut caused = Vec::new();
    if let Some(ledger) = &state.ledger {
        for (key, account) in &ledger.accounts {
            let owned = matches!(
                &account.owner,
                Some(strata_kernel::LedgerOwner::Principal { principal }) if principal == subject
            );
            let entries = ledger.entries.get(key).map(Vec::as_slice).unwrap_or(&[]);
            let shown: Vec<Json> = entries
                .iter()
                .filter(|e| owned || e.caused_by.as_ref() == Some(subject))
                .map(|e| {
                    json!({
                        "account": account_key(&e.account),
                        "sequence": e.sequence,
                        "amount": e.amount.0,
                        "reason": e.reason,
                        "at": e.at.0,
                        "caused_by": e.caused_by.as_ref().filter(|p| *p == subject).map(|p| p.as_str()),
                    })
                })
                .collect();
            if owned {
                accounts.push(json!({
                    "account": account_key(&account.account),
                    "unit": account.unit.as_str(),
                    "balance": account.balance.0,
                    "opened_at": account.opened_at.0,
                    "entries": shown,
                }));
            } else {
                caused.extend(shown);
            }
        }
    }

    // Segments and settings.
    let mut segments: Vec<Json> = app
        .store
        .segment_memberships_of(tenant, subject)?
        .iter()
        .map(|m| json!({ "segment": m.segment.to_string(), "since": m.since.0 }))
        .collect();
    segments.sort_by_key(|s| s["segment"].as_str().unwrap_or_default().to_string());
    let mut settings = Vec::new();
    if let Some(values) = state.setting_values.as_deref() {
        for held in values.overrides.values() {
            if held.level == strata_kernel::SettingLevel::Person && held.subject == subject.as_str()
            {
                settings.push(json!({
                    "setting": held.setting.to_string(),
                    "value": to_plain(&held.value),
                    "set_at": held.set_at.0,
                }));
            }
        }
    }

    let restriction = rights
        .restriction_of(subject)
        .map(|r| json!({ "since": r.since, "reason": r.reason }));
    Ok(json!({
        "format": FORMAT,
        "tenant": tenant.as_str(),
        "subject": subject.as_str(),
        "generated_at": now.0,
        "restriction": restriction,
        "account": account,
        "credentials": {
            "passkeys": passkeys,
            "authenticator": authenticator,
            "recovery_codes_left": recovery_codes_left,
        },
        "sessions": sessions,
        "membership": {
            "roles": roles,
            "teams": teams,
            "invitations_issued": issued,
            "invitations_redeemed": redeemed,
        },
        "records": records.into_values().collect::<Vec<_>>(),
        "ledger": { "accounts": accounts, "entries_caused": caused },
        "segments": segments,
        "settings": settings,
        "inbox": Json::Null,
        "not_included": [
            "identities at identity providers: the store lists them by provider and subject, not by principal",
            "events, journal, snapshots and the audit log: copies of what was written, not records of the subject (see the erasure receipt for what each holds)",
            "the content of files that file fields refer to",
        ],
    }))
}
