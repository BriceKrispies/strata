//! The operations the backend performs for a caller. Each one loads state through the storage
//! contract, lets the kernel decide, and reports. No domain rule is written here.

use crate::app::{ApiError, App, LOG};
use crate::auth::Identity;
use serde_json::{json, Value as Json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use strata_kernel::ext::*;
use strata_kernel::{
    read, visible, Check, Context, DomainError, ErrorKind, Intent, InvitationId, Need,
    ObjectTypeId, Output, Principal, PrincipalId, ReadRequest, Record, RecordId, RoleId, TeamId,
    TenantId, TenantState,
};
use strata_protocol::api::{
    ErrorCode, MembersReply, MembersRequest, Mutation, MutationReply, ViewRequest,
};
use strata_storage_api::{execute_by, ExecuteError, Executed};
use strata_substrate::plain::coerce_fields;
use strata_substrate::project::{
    included_refs, match_route, section_order, section_request, SectionData, ViewData,
};

/// The tenant's schema, members and counters, without records.
pub fn header(app: &App, identity: &Identity) -> Result<Arc<TenantState>, ApiError> {
    app.store
        .load(&identity.tenant, &[])?
        .tenant
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, format!("no site {}", identity.tenant)))
}

/// Decide and commit one intent, with logging and counters around it.
pub fn run_intent(
    app: &App,
    ctx: &Context,
    intent: &Intent,
    anonymous: bool,
) -> Result<Executed, ApiError> {
    // A change of someone's roles ends their sessions in the commit itself. The ones that are
    // live now are noted first, to be announced and to have their sockets closed.
    let affected = match intent {
        Intent::AssignRole { principal, .. }
        | Intent::RevokeRole { principal, .. }
        | Intent::EraseSubject {
            subject: principal, ..
        } => app
            .store
            .live_sessions_of(&ctx.tenant, Some(principal), ctx.now)?,
        _ => Vec::new(),
    };
    app.stats.in_flight.fetch_add(1, Ordering::Relaxed);
    // The request's deadline: a change that was not committed by the time the caller was told
    // "timed out" is not committed afterwards; a retry with the same key learns the outcome.
    let result = execute_by(
        app.store.as_ref(),
        intent,
        ctx,
        strata_runtime::server::work_deadline(),
    );
    app.stats.in_flight.fetch_sub(1, Ordering::Relaxed);
    match &result {
        Ok(done) => {
            app.stats
                .record(ctx.tenant.as_str(), intent.name(), None, done.conflicts);
            LOG.info(
                "kernel.accepted",
                &ctx.correlation,
                json!({
                    "tenant": ctx.tenant.as_str(),
                    "intent": intent.name(),
                    "revision": done.receipt.revision.0,
                    "events": done.transition.events.len(),
                    "checks": done.transition.evidence.len(),
                    "conflicts_retried": done.conflicts,
                }),
            );
            app.hub.committed();
            crate::auth::announce_ended(app, affected, &ctx.correlation);
        }
        Err(ExecuteError::Domain(error)) => {
            app.stats
                .record(ctx.tenant.as_str(), intent.name(), Some(error.kind), 0);
            let failed: Vec<&str> = error
                .evidence
                .iter()
                .filter(|c| !c.passed)
                .map(|c| c.rule.as_str())
                .collect();
            let line = json!({
                "at": ctx.now.0,
                "tenant": ctx.tenant.as_str(),
                "intent": intent.name(),
                "kind": format!("{:?}", error.kind),
                "failed": failed,
                "correlation": ctx.correlation,
            });
            LOG.info("kernel.rejected", &ctx.correlation, line.clone());
            app.stats.recent_rejections.push(line);
        }
        Err(ExecuteError::Store(error)) => {
            LOG.error(
                "store.failed",
                &ctx.correlation,
                json!({ "intent": intent.name(), "error": error.to_string() }),
            );
        }
    }
    result.map_err(|error| ApiError::execute(error, anonymous))
}

fn object_id(name: &str) -> Result<ObjectTypeId, ApiError> {
    ObjectTypeId::new(name)
        .map_err(|_| ApiError::new(ErrorCode::NotFound, format!("no object type {name}")))
}

/// Apply a record mutation given as plain JSON.
///
/// `idempotency_key` is the caller's key for this change, when it sent one: repeating the request
/// with the same key returns the first reply and changes nothing.
pub fn mutate(
    app: &App,
    identity: &Identity,
    mutation: &Mutation,
    idempotency_key: Option<&str>,
) -> Result<MutationReply, ApiError> {
    let tenant = header(app, identity)?;
    let fields_of = |object: &ObjectTypeId, fields: &serde_json::Map<String, Json>| {
        let def = tenant.schema.objects.get(object).ok_or_else(|| {
            ApiError::new(ErrorCode::NotFound, format!("no object type {object}"))
        })?;
        coerce_fields(def, fields, &|_| None).map_err(|problem| {
            let mut error = ApiError::new(ErrorCode::Invalid, problem.clone());
            let field = problem.split(':').next().unwrap_or("").trim().to_string();
            error.0.failed.push(strata_kernel::Check::fail(
                format!("input:{object}.{field}"),
                problem,
            ));
            error
        })
    };
    let intent = match mutation {
        Mutation::Create { object, fields } => {
            let object = object_id(object)?;
            Intent::CreateRecord {
                fields: fields_of(&object, fields)?,
                object,
            }
        }
        Mutation::Update {
            object,
            id,
            fields,
            expected_version,
        } => {
            let object = object_id(object)?;
            Intent::UpdateRecord {
                fields: fields_of(&object, fields)?,
                object,
                id: RecordId(*id),
                expected_version: *expected_version,
            }
        }
        Mutation::Delete {
            object,
            id,
            expected_version,
        } => Intent::DeleteRecord {
            object: object_id(object)?,
            id: RecordId(*id),
            expected_version: *expected_version,
        },
    };
    // A key is scoped by principal. Every anonymous caller is the same principal, so one of them
    // could read another's first result under a key they both chose: a key needs a signed-in
    // caller.
    if idempotency_key.is_some() && identity.anonymous() {
        return Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in to use an idempotency key.",
        ));
    }
    let mut ctx = identity.context(app);
    ctx.idempotency_key = idempotency_key.map(str::to_string);
    let done = match run_intent(app, &ctx, &intent, identity.anonymous()) {
        Err(error) if error.0.kind.as_deref() == Some("Unauthorized") => {
            return Err(hide_unreadable(app, identity, &intent).unwrap_or(error))
        }
        other => other?,
    };
    let (object, id, version) = match &done.transition.output {
        Output::Record {
            object,
            id,
            version,
        } => (object, id, Some(*version)),
        Output::Deleted { object, id } => (object, id, None),
        other => {
            return Err(ApiError::new(
                ErrorCode::Internal,
                format!("unexpected output {other:?}"),
            ))
        }
    };
    Ok(MutationReply {
        object: object.as_str().to_string(),
        id: id.0,
        version,
        revision: done.receipt.revision.0,
        event_seq: done.transition.delta.event_seq,
        checks_passed: done.transition.evidence.len(),
        fields: match mutation {
            Mutation::Create { fields, .. } | Mutation::Update { fields, .. } => {
                Some(fields.clone())
            }
            Mutation::Delete { .. } => None,
        },
    })
}

/// A refused change to a record the caller cannot even read answers exactly as a change to a
/// record that does not exist: "not allowed" would tell them it is there. Returns the reply to
/// send instead, or `None` when the record is readable and the refusal may stand.
fn hide_unreadable(app: &App, identity: &Identity, intent: &Intent) -> Option<ApiError> {
    let (Intent::UpdateRecord { object, id, .. } | Intent::DeleteRecord { object, id, .. }) =
        intent
    else {
        return None;
    };
    let need = Need::Record {
        object: object.clone(),
        id: *id,
    };
    let readable = app
        .store
        .load(&identity.tenant, &[need])
        .ok()
        .and_then(|state| state.tenant)
        .and_then(|tenant| {
            tenant
                .record(object, *id)
                .map(|record| visible(&tenant, &identity.principal, record, app.now()))
        })
        .unwrap_or(false);
    if readable {
        return None;
    }
    // The kernel's own reply for a record that is not there.
    let message = format!("no {object} {id}");
    let missing = DomainError::new(ErrorKind::UnknownRecord, message.clone())
        .with(vec![Check::fail("intent:unknown_record", message)]);
    Some(ApiError::domain(missing, identity.anonymous()))
}

/// Who created a record is told only to its creator. Principal ids are login identities, so
/// anyone else would learn the sign-in name of every author.
fn without_others_authorship(principal: &Principal, record: &Arc<Record>) -> Arc<Record> {
    let own = matches!(principal, Principal::User(id) if record.created_by.as_ref() == Some(id));
    if own || record.created_by.is_none() {
        return record.clone();
    }
    let mut shown = Record::clone(record);
    shown.created_by = None;
    Arc::new(shown)
}

/// Parse a name the browser sent into a kernel identifier, or refuse the request as invalid.
fn named<T, E: std::fmt::Display>(what: &str, parsed: Result<T, E>) -> Result<T, ApiError> {
    parsed.map_err(|problem| ApiError::new(ErrorCode::Invalid, format!("{what}: {problem}")))
}

/// Change who holds which role: assign or revoke a role, add or remove a team member, invite and
/// redeem, and revoke an invitation (ADM-150 to ADM-153, ADM-1083). Each is one kernel intent,
/// decided by the kernel as the signed-in person. The address an invitation is redeemed with is
/// never read from the request: it is the verified address of the signed-in account, which
/// `Identity::context` puts in the context.
///
/// Implements: ADM-1083
pub fn members(
    app: &App,
    identity: &Identity,
    request: &MembersRequest,
) -> Result<MembersReply, ApiError> {
    let intent = match request {
        MembersRequest::AssignRole { principal, role } => Intent::AssignRole {
            principal: named("principal", PrincipalId::new(principal.as_str()))?,
            role: named("role", RoleId::new(role.as_str()))?,
        },
        MembersRequest::RevokeRole { principal, role } => Intent::RevokeRole {
            principal: named("principal", PrincipalId::new(principal.as_str()))?,
            role: named("role", RoleId::new(role.as_str()))?,
        },
        MembersRequest::AddTeamMember { team, principal } => Intent::AddTeamMember {
            team: named("team", TeamId::new(team.as_str()))?,
            principal: named("principal", PrincipalId::new(principal.as_str()))?,
        },
        MembersRequest::RemoveTeamMember { team, principal } => Intent::RemoveTeamMember {
            team: named("team", TeamId::new(team.as_str()))?,
            principal: named("principal", PrincipalId::new(principal.as_str()))?,
        },
        MembersRequest::Invite {
            invitation,
            address,
            roles,
        } => Intent::CreateInvitation {
            invitation: named("invitation", InvitationId::new(invitation.as_str()))?,
            address: address.clone(),
            roles: roles
                .iter()
                .map(|role| named("role", RoleId::new(role.as_str())))
                .collect::<Result<BTreeSet<_>, _>>()?,
        },
        MembersRequest::Redeem { invitation } => Intent::RedeemInvitation {
            invitation: named("invitation", InvitationId::new(invitation.as_str()))?,
        },
        MembersRequest::RevokeInvitation { invitation } => Intent::RevokeInvitation {
            invitation: named("invitation", InvitationId::new(invitation.as_str()))?,
        },
    };
    let ctx = identity.context(app);
    let done = run_intent(app, &ctx, &intent, identity.anonymous())?;
    Ok(MembersReply {
        revision: done.receipt.revision.0,
        event_seq: done.transition.delta.event_seq,
        checks_passed: done.transition.evidence.len(),
    })
}

/// Evaluate the view whose route matches a path. Every section is read from one snapshot, and
/// every row and every included record passes through the kernel's read authorization.
pub fn view(app: &App, identity: &Identity, request: &ViewRequest) -> Result<ViewData, ApiError> {
    let tenant = header(app, identity)?;
    let schema = &tenant.schema;
    let (def, route) = match_route(schema, &request.path).ok_or_else(|| {
        ApiError::new(ErrorCode::NotFound, format!("nothing at {}", request.path))
    })?;

    let mut depends_on = BTreeSet::new();
    for section in def.sections.values() {
        let Some(query) = schema.queries.get(&section.query) else {
            continue;
        };
        depends_on.insert(query.object.clone());
        if let Some(object) = schema.objects.get(&query.object) {
            for field in &section.include {
                if let Some((target, _, _)) =
                    object.fields.get(field).and_then(|f| f.ty.ref_target())
                {
                    depends_on.insert(target.clone());
                }
            }
        }
    }
    let needs: Vec<Need> = depends_on.iter().cloned().map(Need::AllOf).collect();
    let state = app.store.load(&identity.tenant, &needs)?;
    let ctx = identity.context(app);
    let anonymous = identity.anonymous();
    let fail = |error| ApiError::domain(error, anonymous);

    let mut done = BTreeMap::new();
    let mut sections = BTreeMap::new();
    let mut revision = tenant.revision;
    for name in section_order(def) {
        let section = &def.sections[name];
        let result = read(
            &state,
            &section_request(section, &route, &request.request, &done),
            &ctx,
        )
        .map_err(fail)?;
        revision = result.revision;
        let object = &schema.queries[&section.query].object;
        let mut included = BTreeMap::new();
        for (field, (target, ids)) in included_refs(schema, object, section, &result.rows) {
            let found = read(
                &state,
                &ReadRequest::Records {
                    object: target,
                    ids,
                },
                &ctx,
            )
            .map_err(fail)?;
            included.insert(
                field,
                found
                    .rows
                    .iter()
                    .map(|record| {
                        (
                            record.id,
                            without_others_authorship(&identity.principal, record),
                        )
                    })
                    .collect(),
            );
        }
        // Later sections may read this one's row, authorship included; only what is sent is
        // trimmed.
        if section.single {
            done.insert(name.clone(), result.rows.first().cloned());
        }
        sections.insert(
            name.clone(),
            SectionData {
                single: section.single,
                rows: result
                    .rows
                    .iter()
                    .map(|record| without_others_authorship(&identity.principal, record))
                    .collect(),
                included,
                markdown: section.markdown.clone(),
            },
        );
    }
    Ok(ViewData {
        view: def.id.clone(),
        title: def.title.clone(),
        params: route,
        sections,
        depends_on,
        revision,
        schema_version: schema.version,
    })
}

fn session_line(s: &strata_storage_api::SessionRecord) -> Json {
    json!({
        "id": s.id.as_str(),
        "tenant": s.tenant.as_str(),
        "principal": s.principal.as_str(),
        "created_at": s.created_at.0,
        "last_seen_at": s.last_seen_at.0,
        "expires_at": s.expires_at.0,
    })
}

/// What a tenant's own diagnostics report: its sessions, connections, counters, events,
/// rejections and runs, and the store's general condition. Nothing of any other tenant.
/// Machine-readable, no secrets, no record contents.
pub fn diagnostics(app: &App, tenant: &TenantId) -> Result<Json, ApiError> {
    let now = app.now();
    let name = tenant.as_str();
    let sessions: Vec<Json> = app
        .store
        .live_sessions_of(tenant, None, now)?
        .iter()
        .map(session_line)
        .collect();
    let connections = app
        .hub
        .connections
        .lock()
        .map(|c| {
            json!(c
                .iter()
                .filter(|(_, connection)| connection.tenant == name)
                .collect::<BTreeMap<_, _>>())
        })
        .unwrap_or(Json::Null);
    // Liveness and this tenant's own backlog only: the integrity check of every tenant file is
    // the operator's (platform diagnostics), and it names other tenants.
    app.store.liveness()?;
    let outbox_pending = app.store.outbox_pending_of(tenant)?;
    let outbox_faults: Vec<_> = app
        .store
        .outbox_faults()
        .into_iter()
        .filter(|fault| fault.tenant == *tenant)
        .collect();
    let counters = app.stats.of(name);
    let mine = |line: &Json| line["tenant"] == name;
    Ok(json!({
        "service": "backend",
        "kernel_version": strata_kernel::KERNEL_VERSION,
        "started_at": app.started_at,
        "now": now.0,
        "store": {
            "capabilities": app.store.capabilities(),
            "health": { "ok": true, "outbox_pending": outbox_pending, "outbox_faults": outbox_faults },
            "migrations": app.store.migrations()?,
        },
        "tenants": app.store.tenant_summary(tenant)?.into_iter().collect::<Vec<_>>(),
        "sessions": sessions,
        "connections": connections,
        "transitions": counters.transitions,
        "rejections_by_kind": counters.rejections,
        "conflicts_retried": counters.conflicts_retried,
        "recent_events": app.stats.recent_events.list().into_iter().filter(mine).collect::<Vec<_>>(),
        "recent_rejections": app.stats.recent_rejections.list().into_iter().filter(mine).collect::<Vec<_>>(),
        "recent_runs": app.lab.recent().into_iter().filter(mine).collect::<Vec<_>>(),
    }))
}

/// Everything the platform diagnostics report, every tenant included, with the audit entries.
/// Served only on the backend's internal route, and only in development: no operator identity
/// exists to hold it to outside it. The BFF never relays it to a browser.
pub fn platform_diagnostics(app: &App) -> Result<Json, ApiError> {
    let now = app.now();
    // One tenant at a time, so a tenant that cannot be read hides nobody else's sessions.
    let mut sessions: Vec<Json> = Vec::new();
    for summary in app.store.tenants()? {
        if let Ok(found) = app.store.live_sessions_of(&summary.id, None, now) {
            sessions.extend(found.iter().map(session_line));
        }
    }
    let connections = app
        .hub
        .connections
        .lock()
        .map(|c| json!(*c))
        .unwrap_or(Json::Null);
    Ok(json!({
        "service": "backend",
        "kernel_version": strata_kernel::KERNEL_VERSION,
        "started_at": app.started_at,
        "now": now.0,
        "store": {
            "capabilities": app.store.capabilities(),
            "health": app.store.health()?,
            "outbox_faults": app.store.outbox_faults(),
            "migrations": app.store.migrations()?,
        },
        "tenants": app.store.tenants()?,
        "sessions": sessions,
        "connections": connections,
        "pending_operations": app.stats.in_flight.load(Ordering::Relaxed),
        "counters": app.stats.counters.lock().map(|c| json!(*c)).unwrap_or(Json::Null),
        "recent_events": app.stats.recent_events.list(),
        "recent_rejections": app.stats.recent_rejections.list(),
        "recent_runs": app.lab.recent(),
        "audit": app.audit.list(),
        "decisions": app.decisions.list(),
    }))
}
