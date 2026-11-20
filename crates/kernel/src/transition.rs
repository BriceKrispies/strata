//! `transition`: the one function that decides what an intent does to a tenant's state.

use crate::expr::{self, Env};
use crate::intent::{
    Check, CheckExt, Context, DeltaOp, DomainError, DomainErrorExt, DomainEvent, Effect, ErrorKind,
    EventBody, ExistingData, Intent, Output, SchemaChange, StateDelta, Transition,
};
use crate::rules::{
    assigned_roles, authorize, may_confer, roles_of, validate_record, Lookup, Subject,
};
use crate::schema::{
    normalise_address, Action, FieldDef, InvitationDef, ObjectTypeDef, Resource, TeamDef,
    TenantSchema, TenantSchemaExt,
};
use crate::state::{Need, Principal, Record, RecordExt, State, TenantState};
use crate::types::{
    owner_role, public_role, refs_in, FieldId, FieldType, FieldTypeExt, InvitationId, ObjectTypeId,
    OnDelete, PrincipalId, RecordId, RoleId, SchemaVersion, StateRevision, TeamId, Timestamp,
    Value, ValueExt,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Decide an intent against a snapshot.
///
/// Pure: the result depends only on the three arguments. An accepted intent yields the state
/// delta, the events, the requested effects and the checks that passed. A rejected intent
/// yields the error and the checks that led to it. Nothing is changed either way; the caller
/// commits the delta.
pub fn transition(
    state: &State,
    intent: &Intent,
    ctx: &Context,
) -> Result<Transition, DomainError> {
    if state.tenant_id != ctx.tenant {
        return Err(refuse(
            ErrorKind::TenantMismatch,
            "tenant:snapshot_matches_context",
            format!(
                "snapshot is of {}, context names {}",
                state.tenant_id, ctx.tenant
            ),
        ));
    }
    let tenant = match (intent, state.tenant.as_deref()) {
        (Intent::CreateTenant { name, owner }, None) => return create_tenant(ctx, name, owner),
        (Intent::CreateTenant { .. }, Some(_)) => {
            return Err(refuse(
                ErrorKind::TenantExists,
                "tenant:does_not_exist",
                "the tenant already exists",
            ))
        }
        (_, None) => {
            return Err(refuse(
                ErrorKind::TenantNotFound,
                "tenant:exists",
                "no such tenant",
            ));
        }
        (_, Some(tenant)) => tenant,
    };
    if tenant.id != ctx.tenant {
        return Err(refuse(
            ErrorKind::TenantMismatch,
            "tenant:snapshot_matches_context",
            "snapshot holds another tenant",
        ));
    }
    let lookup = Lookup {
        tenant,
        coverage: &state.coverage,
    };
    let mut draft = Draft::new(tenant, ctx, lookup);
    let output = match intent {
        Intent::CreateTenant { .. } => unreachable!("handled above"),
        Intent::ChangeSchema {
            changes,
            expected_version,
            existing,
        } => change_schema(&mut draft, lookup, changes, *expected_version, *existing)?,
        Intent::AssignRole { principal, role } => change_member(&mut draft, principal, role, true)?,
        Intent::RevokeRole { principal, role } => {
            change_member(&mut draft, principal, role, false)?
        }
        Intent::DefineTeam { team, label } => define_team(&mut draft, team, label)?,
        Intent::AssignTeamRole { team, role } => team_role(&mut draft, team, role, true)?,
        Intent::RevokeTeamRole { team, role } => team_role(&mut draft, team, role, false)?,
        Intent::AddTeamMember { team, principal } => {
            team_member(&mut draft, team, principal, true)?
        }
        Intent::RemoveTeamMember { team, principal } => {
            team_member(&mut draft, team, principal, false)?
        }
        Intent::CreateInvitation {
            invitation,
            address,
            roles,
        } => create_invitation(&mut draft, invitation, address, roles)?,
        Intent::RedeemInvitation { invitation } => redeem_invitation(&mut draft, invitation)?,
        Intent::CreateRecord { object, fields } => {
            create_record(&mut draft, lookup, object, fields)?
        }
        Intent::UpdateRecord {
            object,
            id,
            fields,
            expected_version,
        } => update_record(&mut draft, lookup, object, *id, fields, *expected_version)?,
        Intent::DeleteRecord {
            object,
            id,
            expected_version,
        } => delete_record(&mut draft, lookup, object, *id, *expected_version)?,
    };
    Ok(draft.finish(output))
}

/// A rejection made before any rule of the tenant is consulted. It still names the check that
/// failed, so that every refused transition says why.
fn refuse(kind: ErrorKind, rule: &str, message: impl Into<String>) -> DomainError {
    let message = message.into();
    DomainError::new(kind, message.clone()).with(vec![Check::fail(rule, message)])
}

/// The state an intent is expected to need. The shell loads these before calling `transition`.
///
/// This is a plan, not a proof: `transition` guards every lookup itself and answers
/// `InsufficientState` with the missing need if the plan fell short.
pub fn needs(schema: Option<&TenantSchema>, intent: &Intent) -> Vec<Need> {
    let Some(schema) = schema else {
        return Vec::new();
    };
    let mut out = BTreeSet::new();
    let field_needs =
        |out: &mut BTreeSet<Need>, object: &ObjectTypeId, fields: &BTreeMap<FieldId, Value>| {
            let Some(def) = schema.objects.get(object) else {
                return;
            };
            for (id, value) in fields {
                let Some(field) = def.fields.get(id) else {
                    continue;
                };
                if field.unique && !value.is_null() {
                    out.insert(Need::ByField {
                        object: object.clone(),
                        field: id.clone(),
                        value: value.clone(),
                    });
                }
                if let Some((target, _, _)) = field.ty.ref_target() {
                    for id in refs_in(value) {
                        out.insert(Need::Record {
                            object: target.clone(),
                            id,
                        });
                    }
                }
            }
        };
    match intent {
        Intent::CreateTenant { .. }
        | Intent::AssignRole { .. }
        | Intent::RevokeRole { .. }
        | Intent::DefineTeam { .. }
        | Intent::AssignTeamRole { .. }
        | Intent::RevokeTeamRole { .. }
        | Intent::AddTeamMember { .. }
        | Intent::RemoveTeamMember { .. }
        | Intent::CreateInvitation { .. }
        | Intent::RedeemInvitation { .. } => {}
        Intent::CreateRecord { object, fields } => field_needs(&mut out, object, fields),
        Intent::UpdateRecord {
            object, id, fields, ..
        } => {
            out.insert(Need::Record {
                object: object.clone(),
                id: *id,
            });
            field_needs(&mut out, object, fields);
        }
        Intent::DeleteRecord { object, id, .. } => {
            out.insert(Need::Record {
                object: object.clone(),
                id: *id,
            });
            for (from, field, _) in schema.referrers(object) {
                out.insert(Need::ByField {
                    object: from.id.clone(),
                    field: field.id.clone(),
                    value: Value::Ref(*id),
                });
            }
        }
        Intent::ChangeSchema { changes, .. } => {
            for object in touched_objects(changes) {
                out.insert(Need::AllOf(object));
            }
        }
    }
    out.into_iter().collect()
}

/// Object types whose records must be re-validated after these changes, plus the targets of
/// any reference fields they introduce.
fn touched_objects(changes: &[SchemaChange]) -> BTreeSet<ObjectTypeId> {
    let mut out = BTreeSet::new();
    for change in changes {
        match change {
            SchemaChange::DefineField { object, field } => {
                out.insert(object.clone());
                if let Some((target, _, _)) = field.ty.ref_target() {
                    out.insert(target.clone());
                }
            }
            SchemaChange::DefineRelationship { object, target, .. } => {
                out.insert(object.clone());
                out.insert(target.clone());
            }
            SchemaChange::RemoveField { object, .. }
            | SchemaChange::DefineConstraint { object, .. }
            | SchemaChange::RemoveConstraint { object, .. } => {
                out.insert(object.clone());
            }
            SchemaChange::DefineWorkflow(workflow) => {
                out.insert(workflow.object.clone());
            }
            _ => {}
        }
    }
    out
}

/// Accumulates the result of one transition.
struct Draft<'a> {
    tenant: &'a TenantState,
    ctx: &'a Context,
    lookup: Lookup<'a>,
    ops: Vec<DeltaOp>,
    events: Vec<EventBody>,
    effects: Vec<Effect>,
    evidence: Vec<Check>,
    next_record: u64,
}

impl<'a> Draft<'a> {
    fn new(tenant: &'a TenantState, ctx: &'a Context, lookup: Lookup<'a>) -> Self {
        Draft {
            tenant,
            ctx,
            lookup,
            ops: Vec::new(),
            events: Vec::new(),
            effects: Vec::new(),
            evidence: Vec::new(),
            next_record: tenant.next_record,
        }
    }

    /// A rejection always names at least one failed check. Refusals that come from a rule or a
    /// policy already carry it. The others (an unknown record, a stale version, an intent that
    /// makes no sense) get one here, named after the kind of refusal.
    fn reject(&self, kind: ErrorKind, message: impl Into<String>) -> DomainError {
        let message = message.into();
        let mut evidence = self.evidence.clone();
        if evidence.iter().all(|check| check.passed) {
            evidence.push(Check::fail(
                format!("intent:{}", snake_case(&format!("{kind:?}"))),
                message.clone(),
            ));
        }
        DomainError::new(kind, message).with(evidence)
    }

    fn require(
        &mut self,
        action: Action,
        resource: &Resource,
        subject: Subject<'_>,
    ) -> Result<(), DomainError> {
        let (allowed, checks) = authorize(
            self.tenant,
            &self.tenant.schema,
            &self.ctx.principal,
            self.ctx.now,
            action,
            resource,
            subject,
            Some(self.lookup),
        );
        self.evidence.extend(checks);
        if allowed {
            Ok(())
        } else {
            Err(self.reject(
                ErrorKind::Unauthorized,
                format!("{:?} may not {action:?} {resource:?}", self.ctx.principal),
            ))
        }
    }

    fn finish(self, output: Output) -> Transition {
        let revision = StateRevision(self.tenant.revision.0 + 1);
        let first_seq = self.tenant.event_seq + 1;
        let events: Vec<DomainEvent> = self
            .events
            .into_iter()
            .enumerate()
            .map(|(i, body)| DomainEvent {
                tenant: self.tenant.id.clone(),
                seq: first_seq + i as u64,
                revision,
                at: self.ctx.now,
                principal: self.ctx.principal.clone(),
                correlation: self.ctx.correlation.clone(),
                body,
            })
            .collect();
        let mut effects: Vec<Effect> = events
            .iter()
            .map(|event| Effect::Publish { seq: event.seq })
            .collect();
        effects.extend(self.effects);
        Transition {
            delta: StateDelta {
                tenant: self.tenant.id.clone(),
                base_revision: Some(self.tenant.revision),
                new_revision: revision,
                next_record: self.next_record,
                event_seq: self.tenant.event_seq + events.len() as u64,
                ops: self.ops,
            },
            events,
            effects,
            evidence: self.evidence,
            output,
        }
    }
}

fn create_tenant(
    ctx: &Context,
    name: &str,
    owner: &PrincipalId,
) -> Result<Transition, DomainError> {
    if ctx.principal != Principal::System {
        return Err(
            DomainError::new(ErrorKind::Unauthorized, "only the system creates tenants")
                .with(vec![Check::fail("policy:builtin_system", "CreateTenant")]),
        );
    }
    if name.trim().is_empty() {
        return Err(refuse(
            ErrorKind::InvalidIntent,
            "intent:tenant_has_name",
            "a tenant needs a name",
        ));
    }
    let revision = StateRevision(1);
    let event = DomainEvent {
        tenant: ctx.tenant.clone(),
        seq: 1,
        revision,
        at: ctx.now,
        principal: ctx.principal.clone(),
        correlation: ctx.correlation.clone(),
        body: EventBody::TenantCreated {
            name: name.to_string(),
        },
    };
    Ok(Transition {
        delta: StateDelta {
            tenant: ctx.tenant.clone(),
            base_revision: None,
            new_revision: revision,
            next_record: 1,
            event_seq: 1,
            ops: vec![
                DeltaOp::CreateTenant {
                    name: name.to_string(),
                },
                DeltaOp::PutSchema(Arc::new(TenantSchema::genesis())),
                DeltaOp::PutMember {
                    principal: owner.clone(),
                    roles: BTreeSet::from([owner_role()]),
                },
            ],
        },
        events: vec![event],
        effects: vec![Effect::Publish { seq: 1 }],
        evidence: vec![Check::pass("policy:builtin_system", "CreateTenant")],
        output: Output::Tenant,
    })
}

fn object_def<'a>(
    draft: &Draft<'a>,
    object: &ObjectTypeId,
) -> Result<&'a ObjectTypeDef, DomainError> {
    draft
        .tenant
        .schema
        .objects
        .get(object)
        .ok_or_else(|| draft.reject(ErrorKind::UnknownObject, format!("no object type {object}")))
}

fn check_version(
    draft: &Draft<'_>,
    record: &Record,
    expected: Option<u64>,
) -> Result<(), DomainError> {
    match expected {
        Some(version) if version != record.version => Err(draft.reject(
            ErrorKind::VersionConflict,
            format!(
                "record is at version {}, expected {version}",
                record.version
            ),
        )),
        _ => Ok(()),
    }
}

fn create_record(
    draft: &mut Draft<'_>,
    lookup: Lookup<'_>,
    object: &ObjectTypeId,
    fields: &BTreeMap<FieldId, Value>,
) -> Result<Output, DomainError> {
    let def = object_def(draft, object)?;
    let ctx = draft.ctx;
    let roles = roles_of(draft.tenant, &ctx.principal);
    let no_params = BTreeMap::new();

    let mut values: BTreeMap<FieldId, Value> = fields
        .iter()
        .filter(|(_, v)| !v.is_null())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for field in def.fields.values() {
        if fields.contains_key(&field.id) {
            continue;
        }
        if let Some(default) = &field.default {
            let env = Env {
                record: None,
                old: None,
                new: None,
                params: &no_params,
                now: ctx.now,
                principal: &ctx.principal,
                roles: &roles,
                lookup: Some(lookup),
            };
            let value = expr::eval(default, &env).map_err(|e| {
                draft.reject(
                    ErrorKind::Evaluation,
                    format!("default of {}: {e}", field.id),
                )
            })?;
            if !value.is_null() {
                draft.evidence.push(Check::pass(
                    format!("default:{object}.{}", field.id),
                    default.to_string(),
                ));
                values.insert(field.id.clone(), value);
            }
        }
    }

    let candidate = Record {
        id: RecordId(draft.next_record),
        object: object.clone(),
        version: 1,
        created_by: match &ctx.principal {
            Principal::User(id) => Some(id.clone()),
            _ => None,
        },
        created_at: ctx.now,
        updated_at: ctx.now,
        fields: values,
    };

    draft.require(
        Action::Create,
        &Resource::Object(object.clone()),
        Subject {
            old: None,
            new: Some(&candidate),
        },
    )?;

    for workflow in draft.tenant.schema.workflows_of(object) {
        let rule = format!("workflow:{}:initial", workflow.id);
        match candidate.get(&workflow.field) {
            Value::Str(state) if workflow.initial.contains(state) => {
                draft
                    .evidence
                    .push(Check::pass(rule, format!("starts in {state}")));
            }
            other => {
                draft.evidence.push(Check::fail(
                    rule,
                    format!("a new record must start in one of {:?}", workflow.initial),
                ));
                return Err(draft.reject(
                    ErrorKind::WorkflowViolation,
                    format!("{:?} is not an initial state of {}", other, workflow.id),
                ));
            }
        }
    }

    validate_record(
        lookup,
        &draft.tenant.schema,
        def,
        &candidate,
        None,
        &ctx.principal,
        ctx.now,
    )?
    .into_result(&mut draft.evidence)?;

    let record = Arc::new(candidate);
    draft.next_record += 1;
    draft.ops.push(DeltaOp::PutRecord(record.clone()));
    draft.events.push(EventBody::RecordCreated {
        record: record.clone(),
    });
    Ok(Output::Record {
        object: object.clone(),
        id: record.id,
        version: 1,
    })
}

fn stored<'a>(
    draft: &Draft<'_>,
    lookup: Lookup<'a>,
    object: &ObjectTypeId,
    id: RecordId,
) -> Result<&'a Arc<Record>, DomainError> {
    lookup
        .record(object, id)?
        .ok_or_else(|| draft.reject(ErrorKind::UnknownRecord, format!("no {object} {id}")))
}

fn update_record(
    draft: &mut Draft<'_>,
    lookup: Lookup<'_>,
    object: &ObjectTypeId,
    id: RecordId,
    fields: &BTreeMap<FieldId, Value>,
    expected_version: Option<u64>,
) -> Result<Output, DomainError> {
    let def = object_def(draft, object)?;
    let ctx = draft.ctx;
    let old = stored(draft, lookup, object, id)?;

    let mut candidate = Record::clone(old);
    for (field, value) in fields {
        if value.is_null() {
            candidate.fields.remove(field);
        } else {
            candidate.fields.insert(field.clone(), value.clone());
        }
    }
    candidate.version = old.version + 1;
    candidate.updated_at = ctx.now;

    // Authorize before revealing anything else about the record.
    draft.require(
        Action::Update,
        &Resource::Object(object.clone()),
        Subject {
            old: Some(old),
            new: Some(&candidate),
        },
    )?;
    check_version(draft, old, expected_version)?;

    let roles = roles_of(draft.tenant, &ctx.principal);
    let no_params = BTreeMap::new();
    for workflow in draft.tenant.schema.workflows_of(object) {
        let (from, to) = (
            old.get(&workflow.field),
            candidate.get(&workflow.field).clone(),
        );
        if from == &to {
            continue;
        }
        let rule = format!("workflow:{}", workflow.id);
        let env = Env {
            record: Some(old),
            old: Some(old),
            new: Some(&candidate),
            params: &no_params,
            now: ctx.now,
            principal: &ctx.principal,
            roles: &roles,
            lookup: Some(lookup),
        };
        let mut fired = None;
        let mut refused = Vec::new();
        for step in &workflow.transitions {
            if Some(step.from.as_str()) != from.as_str() || Some(step.to.as_str()) != to.as_str() {
                continue;
            }
            let open = match &step.guard {
                None => Ok(true),
                Some(guard) => expr::holds(guard, &env),
            };
            match open {
                Ok(true) => {
                    fired = Some(step);
                    break;
                }
                Ok(false) => refused.push(format!("{}: guard is not true", step.name)),
                Err(e) => refused.push(format!("{}: {e}", step.name)),
            }
        }
        let Some(step) = fired else {
            let detail = if refused.is_empty() {
                format!("no transition from {from:?} to {to:?}")
            } else {
                refused.join("; ")
            };
            draft.evidence.push(Check::fail(rule, detail.clone()));
            return Err(draft.reject(ErrorKind::WorkflowViolation, detail));
        };
        let mut assigned = Vec::new();
        for (field, value) in &step.set {
            let value = expr::eval(value, &env).map_err(|e| {
                draft.reject(
                    ErrorKind::Evaluation,
                    format!("{}: set {field}: {e}", step.name),
                )
            })?;
            assigned.push((field.clone(), value));
        }
        for (field, value) in assigned {
            if value.is_null() {
                candidate.fields.remove(&field);
            } else {
                candidate.fields.insert(field, value);
            }
        }
        draft.evidence.push(Check::pass(
            format!("{rule}:{}", step.name),
            format!("{} -> {}", step.from, step.to),
        ));
    }

    validate_record(
        lookup,
        &draft.tenant.schema,
        def,
        &candidate,
        Some(old),
        &ctx.principal,
        ctx.now,
    )?
    .into_result(&mut draft.evidence)?;

    let version = candidate.version;
    let after = Arc::new(candidate);
    draft.ops.push(DeltaOp::PutRecord(after.clone()));
    draft.events.push(EventBody::RecordUpdated {
        before: old.clone(),
        after,
    });
    Ok(Output::Record {
        object: object.clone(),
        id,
        version,
    })
}

fn delete_record(
    draft: &mut Draft<'_>,
    lookup: Lookup<'_>,
    object: &ObjectTypeId,
    id: RecordId,
    expected_version: Option<u64>,
) -> Result<Output, DomainError> {
    object_def(draft, object)?;
    let ctx = draft.ctx;
    let old = stored(draft, lookup, object, id)?;
    draft.require(
        Action::Delete,
        &Resource::Object(object.clone()),
        Subject::stored(old),
    )?;
    check_version(draft, old, expected_version)?;

    let target = Value::Ref(id);
    for (from, field, is_list) in draft.tenant.schema.referrers(object) {
        let (_, on_delete, _) = field
            .ty
            .ref_target()
            .expect("referrers are reference fields");
        for referrer in lookup.by_field(&from.id, &field.id, &target)? {
            if referrer.id == id && from.id == *object {
                continue;
            }
            let rule = format!("reference:{}.{}", from.id, field.id);
            let blocked = on_delete == OnDelete::Restrict || (field.required && !is_list);
            if blocked {
                draft.evidence.push(Check::fail(
                    rule,
                    format!("{} {} still refers to this record", from.id, referrer.id),
                ));
                return Err(draft.reject(
                    ErrorKind::StillReferenced,
                    format!("{} {} refers to {object} {id}", from.id, referrer.id),
                ));
            }
            let mut cleared = Record::clone(referrer);
            match cleared.fields.get(&field.id) {
                Some(Value::List(items)) => {
                    let kept: Vec<Value> =
                        items.iter().filter(|v| **v != target).cloned().collect();
                    cleared.fields.insert(field.id.clone(), Value::List(kept));
                }
                _ => {
                    cleared.fields.remove(&field.id);
                }
            }
            cleared.version += 1;
            cleared.updated_at = ctx.now;
            let after = Arc::new(cleared);
            draft.evidence.push(Check::pass(
                rule,
                format!("cleared on {} {}", from.id, referrer.id),
            ));
            draft.ops.push(DeltaOp::PutRecord(after.clone()));
            draft.events.push(EventBody::RecordUpdated {
                before: referrer.clone(),
                after,
            });
        }
    }

    draft.ops.push(DeltaOp::DeleteRecord {
        object: object.clone(),
        id,
    });
    draft.events.push(EventBody::RecordDeleted {
        before: old.clone(),
    });
    Ok(Output::Deleted {
        object: object.clone(),
        id,
    })
}

fn change_member(
    draft: &mut Draft<'_>,
    principal: &PrincipalId,
    role: &RoleId,
    grant: bool,
) -> Result<Output, DomainError> {
    draft.require(Action::Manage, &Resource::Members, Subject::default())?;
    if !draft.tenant.schema.roles.contains_key(role) {
        return Err(draft.reject(ErrorKind::UnknownRole, format!("no role {role}")));
    }
    if *role == public_role() {
        return Err(draft.reject(ErrorKind::InvalidIntent, "everyone already holds public"));
    }
    if grant {
        require_conferrable(draft, [role])?;
    }
    let mut roles = draft
        .tenant
        .members
        .get(principal)
        .cloned()
        .unwrap_or_default();
    if grant {
        roles.insert(role.clone());
    } else {
        roles.remove(role);
        let owners = draft
            .tenant
            .members
            .iter()
            .filter(|(p, r)| *p != principal && r.contains(&owner_role()))
            .count();
        if *role == owner_role() && owners == 0 {
            return Err(draft.reject(
                ErrorKind::InvalidIntent,
                "a tenant keeps at least one owner",
            ));
        }
    }
    let through_teams = assigned_roles(&BTreeMap::new(), &draft.tenant.schema, principal);
    if roles.is_empty() && through_teams.is_empty() {
        draft.effects.push(Effect::RevokeSessions {
            principal: principal.clone(),
        });
    }
    draft.ops.push(DeltaOp::PutMember {
        principal: principal.clone(),
        roles: roles.clone(),
    });
    draft.events.push(EventBody::MemberChanged {
        principal: principal.clone(),
        roles: roles.union(&through_teams).cloned().collect(),
    });
    Ok(Output::Member)
}

/// ADM-152: every role in `roles` must be one the acting principal may confer, which is to say
/// the principal holds every permission and capability the role grants. The first role it may
/// not confer refuses the intent.
fn require_conferrable<'r>(
    draft: &mut Draft<'_>,
    roles: impl IntoIterator<Item = &'r RoleId>,
) -> Result<(), DomainError> {
    for role in roles {
        let check = may_confer(draft.tenant, &draft.ctx.principal, role);
        let passed = check.passed;
        let detail = check.detail.clone();
        draft.evidence.push(check);
        if !passed {
            return Err(draft.reject(
                ErrorKind::Unauthorized,
                format!("may not confer role {role}: {detail}"),
            ));
        }
    }
    Ok(())
}

/// The id under which a team appears in events: a team is assigned roles like a principal.
fn team_principal(team: &TeamId) -> PrincipalId {
    PrincipalId::new(format!("team:{team}")).expect("a team id is a valid principal token")
}

fn team_of<'a>(draft: &Draft<'a>, team: &TeamId) -> Result<&'a TeamDef, DomainError> {
    let tenant: &'a TenantState = draft.tenant;
    tenant
        .schema
        .team(team)
        .ok_or_else(|| draft.reject(ErrorKind::InvalidIntent, format!("no team {team}")))
}

/// Record a changed team: the new schema, an event for the team and one for each member whose
/// roles it changes.
fn put_team(draft: &mut Draft<'_>, mut schema: TenantSchema, team: TeamDef) {
    draft.events.push(EventBody::MemberChanged {
        principal: team_principal(&team.id),
        roles: team.roles.clone(),
    });
    schema.put_team(team.clone());
    for member in &team.members {
        let roles = assigned_roles(&draft.tenant.members, &schema, member);
        draft.events.push(EventBody::MemberChanged {
            principal: member.clone(),
            roles,
        });
    }
    draft.ops.push(DeltaOp::PutSchema(Arc::new(schema)));
}

fn define_team(draft: &mut Draft<'_>, team: &TeamId, label: &str) -> Result<Output, DomainError> {
    draft.require(Action::Manage, &Resource::Members, Subject::default())?;
    if label.trim().is_empty() {
        return Err(draft.reject(ErrorKind::InvalidIntent, "a team needs a label"));
    }
    let schema = TenantSchema::clone(&draft.tenant.schema);
    let def = TeamDef {
        label: label.to_string(),
        ..schema.team(team).cloned().unwrap_or(TeamDef {
            id: team.clone(),
            label: String::new(),
            members: BTreeSet::new(),
            roles: BTreeSet::new(),
        })
    };
    put_team(draft, schema, def);
    Ok(Output::Member)
}

fn team_role(
    draft: &mut Draft<'_>,
    team: &TeamId,
    role: &RoleId,
    grant: bool,
) -> Result<Output, DomainError> {
    draft.require(Action::Manage, &Resource::Members, Subject::default())?;
    let mut def = team_of(draft, team)?.clone();
    if !draft.tenant.schema.roles.contains_key(role) {
        return Err(draft.reject(ErrorKind::UnknownRole, format!("no role {role}")));
    }
    if *role == public_role() {
        return Err(draft.reject(ErrorKind::InvalidIntent, "everyone already holds public"));
    }
    if grant {
        require_conferrable(draft, [role])?;
        def.roles.insert(role.clone());
    } else {
        def.roles.remove(role);
    }
    let schema = TenantSchema::clone(&draft.tenant.schema);
    let mut after = schema.clone();
    after.put_team(def.clone());
    let lost: Vec<PrincipalId> = if grant {
        Vec::new()
    } else {
        def.members
            .iter()
            .filter(|member| {
                draft
                    .tenant
                    .members
                    .get(*member)
                    .is_none_or(BTreeSet::is_empty)
                    && assigned_roles(&BTreeMap::new(), &after, member).is_empty()
            })
            .cloned()
            .collect()
    };
    for principal in lost {
        draft.effects.push(Effect::RevokeSessions { principal });
    }
    put_team(draft, schema, def);
    Ok(Output::Member)
}

fn team_member(
    draft: &mut Draft<'_>,
    team: &TeamId,
    principal: &PrincipalId,
    add: bool,
) -> Result<Output, DomainError> {
    draft.require(Action::Manage, &Resource::Members, Subject::default())?;
    let mut def = team_of(draft, team)?.clone();
    if add {
        require_conferrable(draft, def.roles.iter())?;
        def.members.insert(principal.clone());
    } else {
        def.members.remove(principal);
    }
    let schema = TenantSchema::clone(&draft.tenant.schema);
    // The member is no longer in `def.members` after a removal, so it gets its own event here.
    let mut after = schema.clone();
    after.put_team(def.clone());
    let roles = assigned_roles(&draft.tenant.members, &after, principal);
    if !add {
        draft.events.push(EventBody::MemberChanged {
            principal: principal.clone(),
            roles: roles.clone(),
        });
        if roles.is_empty() {
            draft.effects.push(Effect::RevokeSessions {
                principal: principal.clone(),
            });
        }
    }
    put_team(draft, schema, def);
    Ok(Output::Member)
}

fn create_invitation(
    draft: &mut Draft<'_>,
    invitation: &InvitationId,
    address: &str,
    roles: &BTreeSet<RoleId>,
) -> Result<Output, DomainError> {
    draft.require(Action::Manage, &Resource::Members, Subject::default())?;
    let ctx = draft.ctx;
    let Principal::User(creator) = &ctx.principal else {
        return Err(draft.reject(ErrorKind::InvalidIntent, "a person issues an invitation"));
    };
    let address = normalise_address(address);
    if !address.contains('@') {
        return Err(draft.reject(ErrorKind::InvalidIntent, "an invitation names an address"));
    }
    if roles.is_empty() {
        return Err(draft.reject(ErrorKind::InvalidIntent, "an invitation grants a role"));
    }
    if draft.tenant.schema.invitation(invitation).is_some() {
        return Err(draft.reject(
            ErrorKind::InvalidIntent,
            format!("invitation {invitation} already exists"),
        ));
    }
    for role in roles {
        if !draft.tenant.schema.roles.contains_key(role) {
            return Err(draft.reject(ErrorKind::UnknownRole, format!("no role {role}")));
        }
        if *role == public_role() {
            return Err(draft.reject(ErrorKind::InvalidIntent, "everyone already holds public"));
        }
    }
    require_conferrable(draft, roles.iter())?;
    let mut schema = TenantSchema::clone(&draft.tenant.schema);
    schema.put_invitation(InvitationDef {
        id: invitation.clone(),
        address,
        roles: roles.clone(),
        created_by: creator.clone(),
        redeemed_by: None,
    });
    draft.ops.push(DeltaOp::PutSchema(Arc::new(schema)));
    Ok(Output::Member)
}

/// ADM-153: the roles of an invitation go to the one person who redeems it with the address it
/// names, once. The address is the one the shell verified for the acting person
/// (`Context::verified_address`); the intent carries none. The refusal for a wrong address does
/// not say which address the invitation names.
fn redeem_invitation(
    draft: &mut Draft<'_>,
    invitation: &InvitationId,
) -> Result<Output, DomainError> {
    let ctx = draft.ctx;
    let Principal::User(redeemer) = &ctx.principal else {
        draft.evidence.push(Check::fail(
            "invitation:redeemer_is_person",
            "only a signed-in person redeems an invitation",
        ));
        return Err(draft.reject(
            ErrorKind::Unauthorized,
            "only a signed-in person redeems an invitation",
        ));
    };
    let tenant = draft.tenant;
    let Some(offer) = tenant.schema.invitation(invitation) else {
        return Err(draft.reject(
            ErrorKind::InvalidIntent,
            format!("no invitation {invitation}"),
        ));
    };
    let Some(verified_address) = ctx.verified_address.as_deref() else {
        draft.evidence.push(Check::fail(
            "invitation:address_matches",
            "the redeemer has no verified address",
        ));
        return Err(draft.reject(
            ErrorKind::Unauthorized,
            "the invitation needs a verified address",
        ));
    };
    if normalise_address(verified_address) != offer.address {
        draft.evidence.push(Check::fail(
            "invitation:address_matches",
            "the redeemer's verified address is not the address the invitation names",
        ));
        return Err(draft.reject(
            ErrorKind::Unauthorized,
            "the invitation names another address",
        ));
    }
    draft.evidence.push(Check::pass(
        "invitation:address_matches",
        "the verified address is the one the invitation names",
    ));
    if let Some(by) = &offer.redeemed_by {
        draft.evidence.push(Check::fail(
            "invitation:redeemed_once",
            format!("already redeemed by {by}"),
        ));
        return Err(draft.reject(
            ErrorKind::InvalidIntent,
            "the invitation was already redeemed",
        ));
    }
    draft.evidence.push(Check::pass(
        "invitation:redeemed_once",
        "not redeemed before",
    ));
    let mut roles = tenant.members.get(redeemer).cloned().unwrap_or_default();
    roles.extend(offer.roles.iter().cloned());
    let mut schema = TenantSchema::clone(&tenant.schema);
    if let Some(stored) = schema.invitation_mut(invitation) {
        stored.redeemed_by = Some(redeemer.clone());
    }
    let effective: BTreeSet<RoleId> = {
        let mut members = tenant.members.clone();
        members.insert(redeemer.clone(), roles.clone());
        assigned_roles(&members, &schema, redeemer)
    };
    draft.ops.push(DeltaOp::PutSchema(Arc::new(schema)));
    draft.ops.push(DeltaOp::PutMember {
        principal: redeemer.clone(),
        roles,
    });
    draft.events.push(EventBody::MemberChanged {
        principal: redeemer.clone(),
        roles: effective,
    });
    Ok(Output::Member)
}

/// Apply one schema change to a working copy and describe it.
fn apply_change(schema: &mut TenantSchema, change: &SchemaChange) -> Result<String, String> {
    fn object<'a>(
        schema: &'a mut TenantSchema,
        id: &ObjectTypeId,
    ) -> Result<&'a mut ObjectTypeDef, String> {
        schema
            .objects
            .get_mut(id)
            .ok_or_else(|| format!("no object type {id}"))
    }
    Ok(match change {
        SchemaChange::DefineObject { id, label } => {
            schema
                .objects
                .entry(id.clone())
                .and_modify(|o| o.label = label.clone())
                .or_insert_with(|| ObjectTypeDef {
                    id: id.clone(),
                    label: label.clone(),
                    fields: BTreeMap::new(),
                    constraints: BTreeMap::new(),
                });
            format!("define object {id}")
        }
        SchemaChange::DefineField { object: id, field } => {
            let previous = object(schema, id)?
                .fields
                .insert(field.id.clone(), field.clone());
            match previous {
                Some(_) => format!("redefine field {id}.{}", field.id),
                None => format!("define field {id}.{}", field.id),
            }
        }
        SchemaChange::DefineRelationship {
            object: id,
            field,
            label,
            target,
            on_delete,
            required,
        } => {
            let def = FieldDef {
                id: field.clone(),
                label: label.clone(),
                ty: FieldType::Ref {
                    target: target.clone(),
                    on_delete: *on_delete,
                },
                required: *required,
                unique: false,
                default: None,
                read: None,
            };
            object(schema, id)?.fields.insert(field.clone(), def);
            format!("define relationship {id}.{field} -> {target}")
        }
        SchemaChange::RemoveField { object: id, field } => {
            object(schema, id)?
                .fields
                .remove(field)
                .ok_or_else(|| format!("no field {id}.{field}"))?;
            format!("remove field {id}.{field}")
        }
        SchemaChange::DefineConstraint {
            object: id,
            constraint,
        } => {
            object(schema, id)?
                .constraints
                .insert(constraint.id.clone(), constraint.clone());
            format!("define constraint {id}.{}", constraint.id)
        }
        SchemaChange::RemoveConstraint {
            object: id,
            constraint,
        } => {
            object(schema, id)?
                .constraints
                .remove(constraint)
                .ok_or_else(|| format!("no constraint {id}.{constraint}"))?;
            format!("remove constraint {id}.{constraint}")
        }
        SchemaChange::DefineRole(role) => {
            if role.id == public_role() || role.id == owner_role() {
                return Err(format!("{} is built in", role.id));
            }
            schema.roles.insert(role.id.clone(), role.clone());
            format!("define role {}", role.id)
        }
        SchemaChange::DefineRolePermissions { role, permissions } => {
            if !schema.roles.contains_key(role) {
                return Err(format!("no role {role}"));
            }
            schema.set_role_permissions(role, permissions.clone());
            format!("define permissions of role {role}")
        }
        SchemaChange::DefinePolicy(policy) => {
            schema.policies.insert(policy.id.clone(), policy.clone());
            format!("define policy {}", policy.id)
        }
        SchemaChange::RemovePolicy(id) => {
            schema
                .policies
                .remove(id)
                .ok_or_else(|| format!("no policy {id}"))?;
            format!("remove policy {id}")
        }
        SchemaChange::DefineWorkflow(workflow) => {
            schema
                .workflows
                .insert(workflow.id.clone(), workflow.clone());
            format!("define workflow {}", workflow.id)
        }
        SchemaChange::RemoveWorkflow(id) => {
            schema
                .workflows
                .remove(id)
                .ok_or_else(|| format!("no workflow {id}"))?;
            format!("remove workflow {id}")
        }
        SchemaChange::DefineQuery(query) => {
            schema.queries.insert(query.id.clone(), query.clone());
            format!("define query {}", query.id)
        }
        SchemaChange::RemoveQuery(id) => {
            schema
                .queries
                .remove(id)
                .ok_or_else(|| format!("no query {id}"))?;
            format!("remove query {id}")
        }
        SchemaChange::DefineView(view) => {
            schema.views.insert(view.id.clone(), view.clone());
            format!("define view {}", view.id)
        }
        SchemaChange::RemoveView(id) => {
            schema
                .views
                .remove(id)
                .ok_or_else(|| format!("no view {id}"))?;
            format!("remove view {id}")
        }
    })
}

/// One existing record that does not conform to a proposed schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    pub object: ObjectTypeId,
    pub id: RecordId,
    pub check: Check,
    pub kind: ErrorKind,
}

/// The outcome of applying schema changes to a working copy and re-validating existing data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Migration {
    pub schema: TenantSchema,
    pub summary: Vec<String>,
    /// Records rewritten because a field they held was removed.
    pub rewritten: Vec<Arc<Record>>,
    pub violations: Vec<Violation>,
}

/// Work out what a list of schema changes would do, without deciding whether to accept it.
///
/// Returns `InvalidSchema` when the changes do not produce a consistent schema.
pub fn plan_migration(
    lookup: Lookup<'_>,
    changes: &[SchemaChange],
    now: Timestamp,
) -> Result<Migration, DomainError> {
    let current = &lookup.tenant.schema;
    let mut schema = TenantSchema::clone(current);
    let mut summary = Vec::new();
    let mut evidence = Vec::new();
    for change in changes {
        match apply_change(&mut schema, change) {
            Ok(line) => {
                evidence.push(Check::pass("schema:change", line.clone()));
                summary.push(line);
            }
            Err(problem) => {
                evidence.push(Check::fail("schema:change", problem.clone()));
                return Err(DomainError::new(ErrorKind::InvalidSchema, problem).with(evidence));
            }
        }
    }
    let problems = schema.problems();
    if !problems.is_empty() {
        let message = problems[0].clone();
        evidence.extend(
            problems
                .into_iter()
                .map(|p| Check::fail("schema:consistency", p)),
        );
        return Err(DomainError::new(ErrorKind::InvalidSchema, message).with(evidence));
    }
    schema.version = SchemaVersion(current.version.0 + 1);

    let mut rewritten = Vec::new();
    let mut violations = Vec::new();
    for object in touched_objects(changes) {
        let (Some(def), true) = (
            schema.objects.get(&object),
            current.objects.contains_key(&object),
        ) else {
            continue;
        };
        for record in lookup.all_of(&object)? {
            let stripped: BTreeMap<FieldId, Value> = record
                .fields
                .iter()
                .filter(|(field, _)| def.fields.contains_key(*field))
                .map(|(field, value)| (field.clone(), value.clone()))
                .collect();
            let candidate = if stripped.len() == record.fields.len() {
                record.clone()
            } else {
                let rewritten_record = Arc::new(Record {
                    fields: stripped,
                    ..Record::clone(record)
                });
                rewritten.push(rewritten_record.clone());
                rewritten_record
            };
            let outcome = validate_record(
                lookup,
                &schema,
                def,
                &candidate,
                None,
                &Principal::System,
                now,
            )?;
            violations.extend(outcome.failed.into_iter().map(|failure| Violation {
                object: object.clone(),
                id: record.id,
                check: failure.check,
                kind: failure.kind,
            }));
        }
    }
    Ok(Migration {
        schema,
        summary,
        rewritten,
        violations,
    })
}

fn change_schema(
    draft: &mut Draft<'_>,
    lookup: Lookup<'_>,
    changes: &[SchemaChange],
    expected_version: Option<SchemaVersion>,
    existing: ExistingData,
) -> Result<Output, DomainError> {
    draft.require(Action::Manage, &Resource::Schema, Subject::default())?;
    let current = draft.tenant.schema.version;
    if let Some(expected) = expected_version {
        if expected != current {
            return Err(draft.reject(
                ErrorKind::SchemaConflict,
                format!(
                    "schema is at version {}, expected {}",
                    current.0, expected.0
                ),
            ));
        }
    }
    if changes.is_empty() {
        return Err(draft.reject(ErrorKind::InvalidIntent, "no changes given"));
    }
    if changes.len() > crate::limits::MAX_SCHEMA_CHANGES {
        return Err(draft.reject(
            ErrorKind::LimitExceeded,
            format!(
                "{} changes in one schema change is more than the maximum of {}",
                changes.len(),
                crate::limits::MAX_SCHEMA_CHANGES
            ),
        ));
    }
    let migration = plan_migration(lookup, changes, draft.ctx.now).map_err(|mut e| {
        let mut evidence = draft.evidence.clone();
        evidence.append(&mut e.evidence);
        e.evidence = evidence;
        e
    })?;
    draft.evidence.extend(
        migration
            .summary
            .iter()
            .map(|line| Check::pass("schema:change", line.clone())),
    );

    let affected: BTreeSet<(ObjectTypeId, RecordId)> = migration
        .violations
        .iter()
        .map(|v| (v.object.clone(), v.id))
        .collect();
    for violation in &migration.violations {
        draft.evidence.push(Check::fail(
            format!(
                "existing:{}/{}:{}",
                violation.object, violation.id, violation.check.rule
            ),
            violation.check.detail.clone(),
        ));
    }
    if existing == ExistingData::Reject && !affected.is_empty() {
        return Err(draft.reject(
            ErrorKind::MigrationRejected,
            format!(
                "{} existing record(s) would not conform to the new schema",
                affected.len()
            ),
        ));
    }

    let version = migration.schema.version;
    draft
        .ops
        .push(DeltaOp::PutSchema(Arc::new(migration.schema)));
    draft
        .ops
        .extend(migration.rewritten.into_iter().map(DeltaOp::PutRecord));
    draft.events.push(EventBody::SchemaChanged {
        version,
        summary: migration.summary,
        grandfathered: affected.len() as u64,
    });
    Ok(Output::Schema { version })
}

/// Apply an accepted delta to the tenant state it was computed against.
///
/// Fails with `VersionConflict` if the state is not at the delta's base revision.
pub fn apply_delta(
    tenant: Option<&TenantState>,
    delta: &StateDelta,
) -> Result<TenantState, DomainError> {
    let conflict = |found: Option<StateRevision>| {
        DomainError::new(
            ErrorKind::VersionConflict,
            format!(
                "delta is based on {:?}, state is at {found:?}",
                delta.base_revision
            ),
        )
    };
    let mut next = match (tenant, delta.base_revision) {
        (None, None) => {
            let name = delta
                .ops
                .iter()
                .find_map(|op| match op {
                    DeltaOp::CreateTenant { name } => Some(name.clone()),
                    _ => None,
                })
                .ok_or_else(|| {
                    DomainError::new(ErrorKind::InvalidIntent, "delta creates no tenant")
                })?;
            TenantState {
                id: delta.tenant.clone(),
                name,
                revision: StateRevision(0),
                next_record: 1,
                event_seq: 0,
                schema: Arc::new(TenantSchema::genesis()),
                members: BTreeMap::new(),
                records: BTreeMap::new(),
            }
        }
        (Some(tenant), Some(base)) if tenant.revision == base && tenant.id == delta.tenant => {
            tenant.clone()
        }
        (found, _) => return Err(conflict(found.map(|t| t.revision))),
    };
    for op in &delta.ops {
        match op {
            DeltaOp::CreateTenant { .. } => {}
            DeltaOp::PutSchema(schema) => next.schema = schema.clone(),
            DeltaOp::PutMember { principal, roles } => {
                if roles.is_empty() {
                    next.members.remove(principal);
                } else {
                    next.members.insert(principal.clone(), roles.clone());
                }
            }
            DeltaOp::PutRecord(record) => {
                next.records
                    .entry(record.object.clone())
                    .or_default()
                    .insert(record.id, record.clone());
            }
            DeltaOp::DeleteRecord { object, id } => {
                if let Some(records) = next.records.get_mut(object) {
                    records.remove(id);
                    if records.is_empty() {
                        next.records.remove(object);
                    }
                }
            }
        }
    }
    next.revision = delta.new_revision;
    next.next_record = delta.next_record;
    next.event_seq = delta.event_seq;
    Ok(next)
}

/// `UnknownRecord` as `unknown_record`, for the rule name of a refusal.
fn snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (index, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}
