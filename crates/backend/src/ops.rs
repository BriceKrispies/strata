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
    read, read_needs, visible, Check, Context, DomainError, ErrorKind, FieldId, Intent,
    InvitationId, Need, ObjectTypeId, Output, PageCursor, Principal, PrincipalId, ReadRequest,
    ReadResult, Record, RecordId, RoleId, State, TeamId, TenantId, TenantState, ViewSection,
};
use strata_protocol::api::{
    ErrorCode, MembersReply, MembersRequest, Mutation, MutationReply, ViewRequest, MAX_VIEW_LIMIT,
};
use strata_storage_api::{execute_admitting, ExecuteError, Executed};
use strata_substrate::plain::coerce_fields;
use strata_substrate::project::{
    aggregate_request_with, decode_cursor, encode_cursor, included_refs, match_route,
    resolved_settings, section_order, section_request_with, select_fields, with_page, SectionData,
    ViewData,
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
    // A write past the tenant's stored-bytes allowance is refused, naming the quota (BIL-151).
    crate::quota::check_write(app, ctx, intent)?;
    crate::files::attach::check_intent(app, ctx, intent)?;
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
    // The jobs the commit creates are held to the tenant's allowance, never refusing the change
    // (D-6533).
    let result = execute_admitting(
        app.store.as_ref(),
        intent,
        ctx,
        strata_runtime::server::work_deadline(),
        Some(&crate::quota::JobAdmitter { app }),
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
/// The caller's idempotency key, when it sent one, is in the identity and so in the context of
/// the intent: repeating the request with the same key returns the first reply and changes
/// nothing.
pub fn mutate(
    app: &App,
    identity: &Identity,
    mutation: &Mutation,
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
    if identity.idempotency_key.is_some() && identity.anonymous() {
        return Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in to use an idempotency key.",
        ));
    }
    let ctx = identity.context(app);
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

/// The fields a view declares: those of the object of each section's query and of the objects its
/// included references lead to.
fn declared_fields(
    schema: &strata_kernel::TenantSchema,
    def: &strata_kernel::ViewDef,
) -> BTreeSet<FieldId> {
    let mut declared = BTreeSet::new();
    for section in def.sections.values() {
        let Some(query) = schema.queries.get(&section.query) else {
            continue;
        };
        let Some(object) = schema.objects.get(&query.object) else {
            continue;
        };
        declared.extend(object.fields.keys().cloned());
        for field in &section.include {
            if let Some((target, _, _)) = object.fields.get(field).and_then(|f| f.ty.ref_target()) {
                if let Some(target) = schema.objects.get(target) {
                    declared.extend(target.fields.keys().cloned());
                }
            }
        }
    }
    declared
}

/// The field selection of a delivery request, checked against the fields the view declares
/// (CMS-163): at least one name, each a field of the view. A name the view does not declare is
/// refused, not ignored, so that a misspelt field is not mistaken for one that is empty.
fn field_selection(
    schema: &strata_kernel::TenantSchema,
    def: &strata_kernel::ViewDef,
    names: &[String],
) -> Result<BTreeSet<FieldId>, ApiError> {
    if names.is_empty() {
        return Err(ApiError::new(
            ErrorCode::Invalid,
            "A field selection names at least one field.",
        ));
    }
    let declared = declared_fields(schema, def);
    let mut selected = BTreeSet::new();
    for name in names {
        let field = named("fields", FieldId::new(name.as_str()))?;
        if !declared.contains(&field) {
            return Err(ApiError::new(
                ErrorCode::Invalid,
                format!("view {} has no field {name}", def.id.as_str()),
            ));
        }
        selected.insert(field);
    }
    Ok(selected)
}

/// The page of a delivery request: the size, and the section its cursor continues with that
/// cursor. A cursor is `<section>:<position>`, as [`view`] answered it in `next`.
fn page_of_request(
    def: &strata_kernel::ViewDef,
    request: &ViewRequest,
) -> Result<(Option<u32>, Option<(String, PageCursor)>), ApiError> {
    let limit = match request.limit {
        Some(0) => {
            return Err(ApiError::new(ErrorCode::Invalid, "A limit is at least 1."));
        }
        Some(n) if n > MAX_VIEW_LIMIT => {
            return Err(ApiError::new(
                ErrorCode::Invalid,
                format!("A limit is at most {MAX_VIEW_LIMIT}."),
            ));
        }
        other => other,
    };
    let Some(text) = request.cursor.as_deref() else {
        return Ok((limit, None));
    };
    let wrong = || {
        ApiError::new(
            ErrorCode::Invalid,
            "The cursor is not one this view issued.",
        )
    };
    let (section, position) = text.rsplit_once(':').ok_or_else(wrong)?;
    let listed = def.sections.get(section).is_some_and(|s| !s.single);
    let cursor = decode_cursor(position)
        .filter(|_| listed)
        .ok_or_else(wrong)?;
    Ok((limit, Some((section.to_string(), cursor))))
}

/// Answer one read against the snapshot, loading what the read needs when the snapshot does not
/// cover it and what it reports missing after: a section loads the window of its page, never
/// every record of its type.
#[allow(clippy::too_many_arguments)]
fn read_loaded(
    app: &App,
    tenant: &TenantId,
    schema: &strata_kernel::TenantSchema,
    state: &mut State,
    needs: &mut BTreeSet<Need>,
    request: &ReadRequest,
    ctx: &Context,
    anonymous: bool,
) -> Result<ReadResult, ApiError> {
    let reload = |state: &mut State, needs: &BTreeSet<Need>| -> Result<(), ApiError> {
        let wanted: Vec<Need> = needs.iter().cloned().collect();
        *state = app.store.load(tenant, &wanted)?;
        Ok(())
    };
    // The view's snapshot (`view_load`) normally covers the read already; only a read it does not
    // cover loads, and then everything this view has had to load so far.
    let wanted = read_needs(schema, request);
    if wanted.iter().any(|need| !state.coverage.covers(need)) {
        needs.extend(wanted);
        reload(state, needs)?;
    }
    // A policy that follows a reference names the record it lacks; it is loaded and the read made
    // again, a bounded number of times.
    for _ in 0..32 {
        let result = read(state, request, ctx);
        let missing = match &result {
            Err(error) if error.kind == ErrorKind::InsufficientState => error.missing.clone(),
            _ => None,
        };
        match missing {
            Some(need) if !needs.contains(&need) => {
                needs.insert(need);
                reload(state, needs)?;
            }
            _ => return result.map_err(|error| ApiError::domain(error, anonymous)),
        }
    }
    Err(ApiError::new(
        ErrorCode::Internal,
        "A read kept asking for more state than it was given.",
    ))
}

/// Answer one aggregate against the snapshot, as [`read_loaded`] answers a read: load what the
/// aggregate needs first and each record it reports missing after (D-5067).
#[allow(clippy::too_many_arguments)]
fn aggregate_loaded(
    app: &App,
    tenant: &TenantId,
    schema: &strata_kernel::TenantSchema,
    state: &mut State,
    needs: &mut BTreeSet<Need>,
    request: &strata_kernel::AggregateRequest,
    ctx: &Context,
    anonymous: bool,
) -> Result<strata_kernel::AggregateResult, ApiError> {
    let reload = |state: &mut State, needs: &BTreeSet<Need>| -> Result<(), ApiError> {
        let wanted: Vec<Need> = needs.iter().cloned().collect();
        *state = app.store.load(tenant, &wanted)?;
        Ok(())
    };
    let wanted = strata_kernel::aggregates::needs(schema, request);
    if wanted.iter().any(|need| !state.coverage.covers(need)) {
        needs.extend(wanted);
        reload(state, needs)?;
    }
    for _ in 0..32 {
        let result = strata_kernel::aggregates::run(state, request, ctx);
        let missing = match &result {
            Err(error) if error.kind == ErrorKind::InsufficientState => error.missing.clone(),
            _ => None,
        };
        match missing {
            Some(need) if !needs.contains(&need) => {
                needs.insert(need);
                reload(state, needs)?;
            }
            _ => return result.map_err(|error| ApiError::domain(error, anonymous)),
        }
    }
    Err(ApiError::new(
        ErrorCode::Internal,
        "An aggregate kept asking for more state than it was given.",
    ))
}

/// Evaluate the view whose route matches a path. Every section is read from one snapshot, and
/// every row and every included record passes through the kernel's read authorization.
///
/// A section loads and answers one page of its query (the request's `limit`, else the query's own
/// size), with the cursor of the page after it in `next`; a request `cursor` continues one section.
/// A field selection narrows each row and included record to the fields it names after the read,
/// so it can only take away from what the reader may see (CMS-163, CMS-164).
///
/// Implements: CT-093
pub fn view(app: &App, identity: &Identity, request: &ViewRequest) -> Result<ViewData, ApiError> {
    let tenant = header(app, identity)?;
    let schema = &tenant.schema;
    let (def, route) = match_route(schema, &request.path).ok_or_else(|| {
        ApiError::new(ErrorCode::NotFound, format!("nothing at {}", request.path))
    })?;
    let (limit, continued) = page_of_request(def, request)?;
    let selected = request
        .fields
        .as_ref()
        .map(|names| field_selection(schema, def, names))
        .transpose()?;

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
    // Each section loads only the page its query reads (WG-5104, KRN-2526). The snapshot is found
    // with the very requests the sections below make: the page and cursor of the request included.
    let page_of = |section: &ViewSection| -> (Option<u32>, Option<PageCursor>) {
        if section.single {
            return (None, None);
        }
        let cursor = continued
            .as_ref()
            .filter(|(name, _)| {
                def.sections
                    .get(name)
                    .is_some_and(|held| std::ptr::eq(held, section))
            })
            .map(|(_, cursor)| cursor.clone());
        (limit, cursor)
    };
    // The arguments that come from a setting, resolved for this reader (SET-1540).
    let settings = resolved_settings(&tenant, &identity.principal, def);
    let request_of = |section: &ViewSection, done: &BTreeMap<String, Option<Arc<Record>>>| {
        let (size, cursor) = page_of(section);
        with_page(
            section_request_with(section, &route, &request.request, done, &settings),
            size,
            cursor,
        )
    };
    let ctx = identity.context(app);
    let mut state = crate::view_load::load_view_state(app, &ctx, def, &depends_on, &request_of)?;
    let anonymous = identity.anonymous();
    // A restricted person's personal fields are not shown (PRV-352, subject_rights).
    let restrictions = crate::subject_rights::Rights::load_restrictions(app, &identity.tenant)?;
    let shown = |record: &Arc<Record>| {
        let record = &restrictions.hide(schema, record);
        let trimmed = without_others_authorship(&identity.principal, record);
        match &selected {
            Some(fields) => select_fields(&trimmed, fields),
            None => trimmed,
        }
    };

    let mut needs = BTreeSet::new();
    let mut done = BTreeMap::new();
    let mut sections = BTreeMap::new();
    let mut revision = tenant.revision;
    for name in section_order(def) {
        let section = &def.sections[name];
        // A section that names an aggregate delivers its result in place of rows (D-5067); the
        // kernel decides it for this reader like any other read of the same snapshot.
        if let Some(aggregate) = &section.aggregate {
            let ask = aggregate_request_with(
                section,
                aggregate,
                &route,
                &request.request,
                &done,
                &settings,
            );
            let result = aggregate_loaded(
                app,
                &identity.tenant,
                schema,
                &mut state,
                &mut needs,
                &ask,
                &ctx,
                anonymous,
            )?;
            revision = result.revision;
            sections.insert(
                name.clone(),
                SectionData {
                    single: false,
                    rows: Vec::new(),
                    included: BTreeMap::new(),
                    markdown: Vec::new(),
                    next: None,
                    aggregate: Some(result),
                },
            );
            continue;
        }
        let wanted = request_of(section, &done);
        let result = read_loaded(
            app,
            &identity.tenant,
            schema,
            &mut state,
            &mut needs,
            &wanted,
            &ctx,
            anonymous,
        )?;
        revision = result.revision;
        let object = &schema.queries[&section.query].object;
        let mut included = BTreeMap::new();
        for (field, (target, ids)) in included_refs(schema, object, section, &result.rows) {
            let found = read_loaded(
                app,
                &identity.tenant,
                schema,
                &mut state,
                &mut needs,
                &ReadRequest::Records {
                    object: target,
                    ids,
                },
                &ctx,
                anonymous,
            )?;
            included.insert(
                field,
                found
                    .rows
                    .iter()
                    .map(|record| (record.id, shown(record)))
                    .collect(),
            );
        }
        // Later sections may read this one's row, authorship and every field included; only what
        // is sent is trimmed and narrowed.
        if section.single {
            done.insert(name.clone(), result.rows.first().cloned());
        }
        sections.insert(
            name.clone(),
            SectionData {
                single: section.single,
                rows: result.rows.iter().map(&shown).collect(),
                included,
                markdown: section
                    .markdown
                    .iter()
                    .filter(|field| {
                        selected
                            .as_ref()
                            .is_none_or(|fields| fields.contains(*field))
                    })
                    .cloned()
                    .collect(),
                next: result
                    .next
                    .as_ref()
                    .filter(|_| !section.single)
                    .map(|cursor| format!("{name}:{}", encode_cursor(cursor))),
                aggregate: None,
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
        "machine_store": app.hub.outbox.get().map(|outbox| outbox.held()),
        "counters": app.stats.counters.lock().map(|c| json!(*c)).unwrap_or(Json::Null),
        "recent_events": app.stats.recent_events.list(),
        "recent_rejections": app.stats.recent_rejections.list(),
        "recent_runs": app.lab.recent(),
        "audit": app.audit.list(),
        "decisions": app.decisions.list(),
    }))
}
