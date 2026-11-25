//! `transition`: the one function that decides what an intent does to a tenant's state.

use crate::definitions::{operation_powers, uniques, AUTOMATIONS_DEFINE};
use crate::dependencies::{self, Assessment, ChangeClass, DependencyGraph};
use crate::erasure::BATCH;
use crate::evolution::{self, PinBook};
use crate::expr::{self, Env, Expr};
use crate::intent::{
    Check, CheckExt, Context, DeltaOp, DomainError, DomainErrorExt, DomainEvent, Effect, ErrorKind,
    EventBody, ExistingData, Intent, Output, SchemaChange, StateDelta, Transition,
};
use crate::rewrite;
use crate::rules::{
    assigned_roles, authorize, authorize_needing, holds_any, holds_power, insufficient, may_confer,
    missing_error, missing_grants, roles_of, validate_record, Lookup, Subject,
};
use crate::schema::{
    normalise_address, Action, FieldDef, Grant, InvitationDef, ObjectTypeDef, Resource, TeamDef,
    TenantSchema, TenantSchemaExt,
};
use crate::state::{Need, Principal, Record, RecordExt, State, TenantState, TenantStateExt};
use crate::types::{
    owner_role, public_role, refs_in, ConstraintId, FieldId, FieldType, FieldTypeExt, InvitationId,
    ObjectTypeId, OnDelete, PrincipalId, RecordId, RoleId, SchemaVersion, ScopedRole,
    StateRevision, TeamId, TenantOperationDef, Timestamp, UniqueDef, Value, ValueExt, ViewId,
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
    transition_pinned(state, intent, ctx, &PinBook::default()).map(|(transition, _)| transition)
}

/// [`transition`] with the book of structural versions and pins of the tenant
/// ([`crate::evolution`]): a breaking change that reaches only dependents pinned at the version
/// in force, and that has a left inverse, is accepted. Returns the book after the decision, which
/// the shell stores with the transition; it is the book given, unchanged, for every intent but a
/// change of schema.
pub fn transition_pinned(
    state: &State,
    intent: &Intent,
    ctx: &Context,
    pins: &PinBook,
) -> Result<(Transition, PinBook), DomainError> {
    decide_intent(state, intent, ctx, pins, true)
}

/// [`transition`] of a change as it is made once a hold has let it through: the hold rules are not
/// asked, so the change a hold applies is decided by the same code as the direct change, once
/// (D-8400). Only [`crate::holds`] calls it, for the dry run of a filing and for an approval.
pub(crate) fn transition_unheld(
    state: &State,
    intent: &Intent,
    ctx: &Context,
    pins: &PinBook,
) -> Result<Transition, DomainError> {
    decide_intent(state, intent, ctx, pins, false).map(|(transition, _)| transition)
}

fn decide_intent(
    state: &State,
    intent: &Intent,
    ctx: &Context,
    pins: &PinBook,
    holding: bool,
) -> Result<(Transition, PinBook), DomainError> {
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
    refuse_reserved_principals(intent, ctx)?;
    let tenant = match (intent, state.tenant.as_deref()) {
        (Intent::CreateTenant { name, owner }, None) => {
            return create_tenant(ctx, name, owner).map(|t| (t, pins.clone()))
        }
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
    // A change that a hold rule names is filed, not applied (D-8400, ADM-1601).
    if holding {
        if let Some(change) = crate::holds::change_of(intent) {
            if let Some(rule) = crate::holds::rule_holding(tenant, ctx, &change) {
                let (rule_id, approver) = (rule.id.clone(), rule.approver.clone());
                return crate::holds::file(
                    state,
                    tenant,
                    ctx,
                    pins,
                    change,
                    approver,
                    Some(rule_id),
                    None,
                )
                .map(|t| (t, pins.clone()));
            }
        }
    }
    let lookup = Lookup {
        tenant,
        coverage: &state.coverage,
    };
    let mut draft = Draft::new(tenant, ctx, lookup);
    let mut pins_out = pins.clone();
    let output = match intent {
        Intent::CreateTenant { .. } => unreachable!("handled above"),
        Intent::ChangeSchema {
            changes,
            expected_version,
            existing,
        } => change_schema(
            &mut draft,
            lookup,
            changes,
            *expected_version,
            *existing,
            &mut pins_out,
        )?,
        Intent::AssignRole { principal, role } => change_member(&mut draft, principal, role, true)?,
        Intent::AssignScopedRole {
            principal,
            role,
            object,
            scope,
        } => assign_scoped_role(&mut draft, principal, role, object, scope)?,
        Intent::RevokeScopedRole {
            principal,
            role,
            object,
        } => revoke_scoped_role(&mut draft, principal, role, object)?,
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
        Intent::RevokeInvitation { invitation } => revoke_invitation(&mut draft, invitation)?,
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
        Intent::EraseSubject {
            subject,
            resume_object,
            resume_after,
            held_objects,
            addressed_invitations,
        } => erase_subject(
            &mut draft,
            lookup,
            subject,
            resume_object.as_ref(),
            *resume_after,
            held_objects,
            addressed_invitations,
        )?,
        Intent::ExpireRecords { object, ids } => {
            let done = crate::retention::decide(lookup, draft.ctx, object, ids)?;
            draft.ops.extend(done.ops);
            draft.events.extend(done.events);
            draft.evidence.extend(done.evidence);
            done.output
        }
        Intent::RefreshComputed { object, after } => {
            let done = crate::computed::decide_refresh(
                lookup,
                &draft.tenant.schema,
                draft.ctx,
                object,
                *after,
            )?;
            draft.ops.extend(done.recomputed.ops);
            draft.events.extend(done.recomputed.events);
            draft.evidence.extend(done.recomputed.evidence);
            done.output
        }
        Intent::WriteSetting {
            setting,
            level,
            team,
            value,
            expected_revision,
        } => {
            let decision = crate::settings::write_setting(
                tenant,
                ctx,
                setting,
                *level,
                team.as_ref(),
                value.as_ref(),
                *expected_revision,
            )?;
            draft.ops.extend(decision.ops);
            draft.events.extend(decision.events);
            draft.evidence.extend(decision.evidence);
            decision.output
        }
        Intent::InvokeOperation {
            operation,
            inputs,
            idempotency_key,
        } => {
            // The invocation's events reach the triggers like those of any other decision.
            return crate::operations::invoke(
                tenant,
                &state.coverage,
                ctx,
                operation,
                inputs,
                idempotency_key,
            )
            .and_then(|result| crate::triggers::after_decision(lookup, ctx, result))
            .map(|t| (t, pins.clone()));
        }
        Intent::FileHold {
            change,
            approver,
            on_behalf_of,
        } => {
            return crate::holds::file(
                state,
                tenant,
                ctx,
                pins,
                change.clone(),
                approver.clone(),
                None,
                on_behalf_of.clone(),
            )
            .map(|t| (t, pins.clone()));
        }
        Intent::DecideHold {
            hold,
            decision,
            read_revision,
            reason,
        } => {
            return crate::holds::decide(
                state,
                tenant,
                ctx,
                pins,
                hold,
                *decision,
                *read_revision,
                reason.as_ref(),
            )
            .map(|t| (t, pins.clone()));
        }
        Intent::PostLedger { postings } => {
            let decided = crate::ledger::decide_intent(state, postings, ctx)?;
            draft.ops.extend(decided.ops);
            draft.events.extend(decided.events);
            draft.evidence.extend(decided.evidence);
            decided.output
        }
        Intent::PostLedgerTransaction { transaction } => {
            let decided = crate::ledger_transaction::decide(state, transaction, ctx)?;
            draft.ops.extend(decided.ops);
            draft.events.extend(decided.events);
            draft.evidence.extend(decided.evidence);
            decided.output
        }
        Intent::ExpireLedger { unit, cutoff } => {
            let decided = crate::ledger_expiry::decide(state, unit, *cutoff, ctx)?;
            draft.ops.extend(decided.ops);
            draft.events.extend(decided.events);
            draft.evidence.extend(decided.evidence);
            decided.output
        }
    };
    crate::triggers::after_decision(lookup, ctx, draft.finish(output)).map(|t| (t, pins_out))
}

/// The prefix of the principal that stands for a team in events. A person may not hold an id
/// that starts with it, or a change to a team would reach that person's sessions and notices.
const TEAM_PREFIX: &str = "team:";

/// Refuse a person whose id starts with the team prefix, as the caller, as the owner of a new
/// tenant, or as a member being assigned a role or added to a team.
fn refuse_reserved_principals(intent: &Intent, ctx: &Context) -> Result<(), DomainError> {
    let named: Option<&PrincipalId> = match intent {
        Intent::CreateTenant { owner, .. } => Some(owner),
        Intent::AssignRole { principal, .. }
        | Intent::AssignScopedRole { principal, .. }
        | Intent::AddTeamMember { principal, .. } => Some(principal),
        _ => None,
    };
    let caller = match &ctx.principal {
        Principal::User(id) => Some(id),
        _ => None,
    };
    for id in named.into_iter().chain(caller) {
        if id.as_str().starts_with(TEAM_PREFIX) {
            return Err(refuse(
                ErrorKind::InvalidIntent,
                "principal:id_is_not_reserved",
                format!("the principal id {id} starts with the reserved prefix {TEAM_PREFIX}"),
            ));
        }
    }
    if let Some(problem) =
        crate::automation::reserved_problem(named, &ctx.principal, ctx.session.is_some())
    {
        return Err(refuse(
            ErrorKind::InvalidIntent,
            "principal:id_is_not_reserved",
            problem,
        ));
    }
    Ok(())
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
    match intent {
        Intent::CreateTenant { .. }
        | Intent::AssignRole { .. }
        | Intent::AssignScopedRole { .. }
        | Intent::RevokeScopedRole { .. }
        | Intent::RevokeRole { .. }
        | Intent::DefineTeam { .. }
        | Intent::AssignTeamRole { .. }
        | Intent::RevokeTeamRole { .. }
        | Intent::AddTeamMember { .. }
        | Intent::RemoveTeamMember { .. }
        | Intent::CreateInvitation { .. }
        | Intent::RedeemInvitation { .. }
        | Intent::RevokeInvitation { .. }
        | Intent::WriteSetting { .. }
        // The accounts a posting needs are named by `refine_needs`, which knows the tenant.
        | Intent::PostLedger { .. }
        | Intent::PostLedgerTransaction { .. } => {}
        Intent::ExpireLedger { unit, cutoff } => {
            out.extend(crate::ledger_expiry::needs(unit, *cutoff));
        }
        Intent::CreateRecord { object, fields } => field_needs(schema, &mut out, object, fields),
        Intent::UpdateRecord {
            object, id, fields, ..
        } => {
            out.insert(Need::Record {
                object: object.clone(),
                id: *id,
            });
            field_needs(schema, &mut out, object, fields);
            for dependent in crate::computed::dependents(schema, object) {
                out.insert(Need::ByField {
                    object: dependent.object,
                    field: dependent.reference,
                    value: Value::Ref(*id),
                });
            }
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
        Intent::EraseSubject { .. } => out.extend(crate::erasure::needs(schema, intent)),
        Intent::ExpireRecords { .. } => out.extend(crate::retention::needs(schema, intent)),
        Intent::RefreshComputed { object, after } => {
            out.insert(Need::PageOf {
                object: object.clone(),
                after: *after,
                limit: BATCH as u32,
            });
        }
        Intent::ChangeSchema { changes, .. } => {
            // Re-validation looks up every reference of every record of a touched type, in the
            // schema as it will be. Load each such type whole: one need per object type, however
            // many records or references there are.
            let mut changed = TenantSchema::clone(schema);
            for change in changes {
                let _ = apply_change(&mut changed, change);
            }
            for object in touched_objects(schema, changes) {
                for def in [schema.objects.get(&object), changed.objects.get(&object)]
                    .into_iter()
                    .flatten()
                {
                    for field in def.fields.values() {
                        if let Some((target, _, _)) = field.ty.ref_target() {
                            out.insert(Need::AllOf(target.clone()));
                        }
                    }
                }
                out.insert(Need::AllOf(object));
            }
            out.extend(crate::ledger::schema_change_needs(changes));
            crate::object_meta::removal_needs(changes, &mut out);
        }
        Intent::InvokeOperation {
            operation, inputs, ..
        } => crate::operations::needs(schema, operation, inputs, &mut out),
        // What a hold needs is what its change needs: the dry run of a filing and the application
        // of an approval decide that change.
        Intent::FileHold { change, .. } => {
            out.extend(needs(Some(schema), &crate::holds::intent_of(change)));
        }
        Intent::DecideHold { hold, .. } => {
            if let Some(held) = crate::holds::needs_of(schema, hold) {
                out.extend(needs(Some(schema), &held));
            }
        }
    }
    out.into_iter().collect()
}

/// The unique-value and reference lookups validating `fields` of an `object` record makes.
pub(crate) fn field_needs(
    schema: &TenantSchema,
    out: &mut BTreeSet<Need>,
    object: &ObjectTypeId,
    fields: &BTreeMap<FieldId, Value>,
) {
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
    // A composite unique constraint whose every field has a value: one lookup of the records of
    // the type with those values (D-6558, CMS-1557).
    for unique in uniques(def) {
        let wanted: Option<BTreeMap<FieldId, Value>> = unique
            .fields
            .iter()
            .map(|field| {
                fields
                    .get(field)
                    .filter(|value| !value.is_null())
                    .map(|value| (field.clone(), value.clone()))
            })
            .collect();
        if let Some(wanted) = wanted {
            out.insert(Need::ByFields {
                object: object.clone(),
                fields: wanted,
            });
        }
    }
}

/// The state an intent needs that only the state already loaded can say. Validating an update
/// looks up every reference and unique value of the whole candidate, the stored fields as well as
/// the supplied ones, and the stored record is not known until it is loaded. Given a snapshot that
/// holds it, this names all of those lookups at once, so the shell reloads once instead of once
/// per missing reference. Empty when the snapshot holds nothing more to say.
pub fn refine_needs(state: &State, intent: &Intent) -> Vec<Need> {
    let Some(tenant) = state.tenant.as_deref() else {
        return Vec::new();
    };
    // A hold plans what its change plans.
    if let Intent::FileHold { change, .. } = intent {
        return refine_needs(state, &crate::holds::intent_of(change));
    }
    if let Intent::DecideHold { hold, .. } = intent {
        return crate::holds::needs_of(&tenant.schema, hold)
            .map(|held| refine_needs(state, &held))
            .unwrap_or_default();
    }
    if let Intent::InvokeOperation {
        operation, inputs, ..
    } = intent
    {
        return crate::operations::refine_needs(tenant, operation, inputs);
    }
    if let Intent::PostLedger { postings } = intent {
        return crate::ledger::intent_needs(&tenant.id, postings);
    }
    if let Intent::PostLedgerTransaction { transaction } = intent {
        return crate::ledger_transaction::needs(&tenant.id, transaction);
    }
    if let Intent::ExpireLedger { unit, cutoff } = intent {
        return crate::ledger_expiry::needs(unit, *cutoff);
    }
    let Intent::UpdateRecord {
        object, id, fields, ..
    } = intent
    else {
        return Vec::new();
    };
    let Some(old) = tenant.record(object, *id) else {
        return Vec::new();
    };
    let mut merged = old.fields.clone();
    for (field, value) in fields {
        if value.is_null() {
            merged.remove(field);
        } else {
            merged.insert(field.clone(), value.clone());
        }
    }
    let mut out = BTreeSet::new();
    field_needs(&tenant.schema, &mut out, object, &merged);
    out.into_iter().collect()
}

/// Object types whose records must be re-validated after these changes, plus the targets of
/// any reference fields they introduce.
fn touched_objects(schema: &TenantSchema, changes: &[SchemaChange]) -> BTreeSet<ObjectTypeId> {
    let mut out = BTreeSet::new();
    for change in changes {
        match change {
            SchemaChange::RenameField { object, .. } | SchemaChange::RetypeField { object, .. } => {
                out.insert(object.clone());
            }
            SchemaChange::MoveField { object, via, .. } => {
                out.insert(object.clone());
                let target = schema
                    .objects
                    .get(object)
                    .and_then(|o| o.fields.get(via))
                    .and_then(|f| f.ty.ref_target().map(|(to, _, _)| to.clone()));
                out.extend(target);
            }
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
            | SchemaChange::RemoveConstraint { object, .. }
            | SchemaChange::DefineUnique { object, .. }
            | SchemaChange::RemoveUnique { object, .. } => {
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
        let (allowed, checks, needed) = authorize_needing(
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
        } else if let Some(need) = needed {
            // A condition followed a reference the snapshot does not hold: ask for it rather
            // than deny on a record that was merely not loaded.
            Err(insufficient(need))
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

/// The definition of `object`, for an intent that does `action` on it. A caller whom no policy
/// could allow that action on the type gets the refusal for lack of permission whether or not
/// the type exists, so a refusal never tells anyone which types exist; only a caller who could
/// act on a type learns that it is missing.
fn object_def<'a>(
    draft: &mut Draft<'a>,
    object: &ObjectTypeId,
    action: Action,
) -> Result<&'a ObjectTypeDef, DomainError> {
    let tenant: &'a TenantState = draft.tenant;
    match tenant.schema.objects.get(object) {
        Some(def) => Ok(def),
        None => {
            // A view is a read-only source (D-8404, ADM-1612): a write that names one is refused
            // as such for every caller, the owner included. The ids of views are published in
            // the routes of the tenant, so naming one tells nothing the caller could not read.
            if ViewId::new(object.as_str())
                .is_ok_and(|view| tenant.schema.views.contains_key(&view))
            {
                return Err(draft.reject(
                    ErrorKind::ViewReadOnly,
                    format!("{object} is a view, which is read-only and cannot be written"),
                ));
            }
            draft.require(
                action,
                &Resource::Object(object.clone()),
                Subject::default(),
            )?;
            Err(draft.reject(ErrorKind::UnknownObject, format!("no object type {object}")))
        }
    }
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
    let def = object_def(draft, object, Action::Create)?;
    if let Some(problem) = crate::computed::written(def, fields) {
        return Err(draft.reject(ErrorKind::InvalidIntent, problem));
    }
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

    crate::normalise::normalise_fields(def, &mut values);
    let mut candidate = Record {
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
    let computed =
        crate::computed::values(lookup, def, &candidate, &ctx.principal, &roles, ctx.now)?;
    crate::computed::store(&mut candidate, computed);

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

/// The stored record, for an intent that does `action` on it. When there is none, the caller is
/// asked for the permission first, with no record to test a condition against: one who could not
/// act on it gets the refusal for lack of permission, the same as for a record that exists, and
/// only one who could learns that it is missing.
fn stored<'a>(
    draft: &mut Draft<'_>,
    lookup: Lookup<'a>,
    object: &ObjectTypeId,
    id: RecordId,
    action: Action,
) -> Result<&'a Arc<Record>, DomainError> {
    match lookup.record(object, id)? {
        Some(record) => Ok(record),
        None => {
            draft.require(
                action,
                &Resource::Object(object.clone()),
                Subject::default(),
            )?;
            Err(draft.reject(ErrorKind::UnknownRecord, format!("no {object} {id}")))
        }
    }
}

fn update_record(
    draft: &mut Draft<'_>,
    lookup: Lookup<'_>,
    object: &ObjectTypeId,
    id: RecordId,
    fields: &BTreeMap<FieldId, Value>,
    expected_version: Option<u64>,
) -> Result<Output, DomainError> {
    let def = object_def(draft, object, Action::Update)?;
    let ctx = draft.ctx;
    let old = stored(draft, lookup, object, id, Action::Update)?;

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
    let roles = roles_of(draft.tenant, &ctx.principal);
    let computed =
        crate::computed::values(lookup, def, &candidate, &ctx.principal, &roles, ctx.now)?;
    crate::computed::store(&mut candidate, computed);

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

    let no_params = BTreeMap::new();
    let before_workflows = candidate.fields.clone();
    // D-7203: the transition each workflow fired, named in the event.
    let mut fired_transitions = BTreeMap::new();
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
            if let (Err(_), Some(guard)) = (&open, &step.guard) {
                if let Some(missing) = missing_error(guard, &env) {
                    return Err(missing);
                }
            }
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
                missing_error(value, &env).unwrap_or_else(|| {
                    draft.reject(
                        ErrorKind::Evaluation,
                        format!("{}: set {field}: {e}", step.name),
                    )
                })
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
        fired_transitions.insert(workflow.id.clone(), step.name.clone());
    }

    let computed =
        crate::computed::values(lookup, def, &candidate, &ctx.principal, &roles, ctx.now)?;
    crate::computed::store(&mut candidate, computed);
    if candidate.fields != before_workflows {
        // Workflow assignments changed the candidate after the first decision: the update must
        // also be allowed as the record it now is, so a step cannot write what the actor could
        // not write directly. An assignment that leaves the candidate as it was changes nothing
        // to decide again.
        draft.require(
            Action::Update,
            &Resource::Object(object.clone()),
            Subject {
                old: Some(old),
                new: Some(&candidate),
            },
        )?;
    }

    crate::normalise::normalise_fields(def, &mut candidate.fields);
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
    // The records that refer to this one through a computed field, recomputed in this commit.
    let referring = crate::computed::refer(
        &draft.tenant.schema,
        lookup,
        object,
        old,
        &after,
        &ctx.principal,
        &roles,
        ctx.now,
    )?;
    draft.evidence.extend(referring.evidence);
    draft.ops.extend(referring.ops);
    draft.events.extend(referring.events);
    draft.ops.push(DeltaOp::PutRecord(after.clone()));
    draft.events.push(EventBody::RecordUpdated {
        before: old.clone(),
        after,
        transitions: (!fired_transitions.is_empty()).then_some(fired_transitions),
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
    object_def(draft, object, Action::Delete)?;
    let ctx = draft.ctx;
    let old = stored(draft, lookup, object, id, Action::Delete)?;
    draft.require(
        Action::Delete,
        &Resource::Object(object.clone()),
        Subject::stored(old),
    )?;
    check_version(draft, old, expected_version)?;

    let target = Value::Ref(id);
    // Every field of one referrer that points here is cleared in the same rewrite, so a record
    // that refers twice is written once with both fields cleared.
    let mut cleared: BTreeMap<(ObjectTypeId, RecordId), (Arc<Record>, Record)> = BTreeMap::new();
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
            let (_, rewrite) = cleared
                .entry((from.id.clone(), referrer.id))
                .or_insert_with(|| (referrer.clone(), Record::clone(referrer)));
            match rewrite.fields.get(&field.id) {
                Some(Value::List(items)) => {
                    let kept: Vec<Value> =
                        items.iter().filter(|v| **v != target).cloned().collect();
                    rewrite.fields.insert(field.id.clone(), Value::List(kept));
                }
                _ => {
                    rewrite.fields.remove(&field.id);
                }
            }
            draft.evidence.push(Check::pass(
                rule,
                format!("cleared on {} {}", from.id, referrer.id),
            ));
        }
    }
    for (before, mut rewrite) in cleared.into_values() {
        rewrite.version += 1;
        rewrite.updated_at = ctx.now;
        let after = Arc::new(rewrite);
        draft.ops.push(DeltaOp::PutRecord(after.clone()));
        draft.events.push(EventBody::RecordUpdated {
            before,
            after,
            transitions: None,
        });
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

/// Erase one data subject, one batch of it (the module [`crate::erasure`]).
fn erase_subject(
    draft: &mut Draft<'_>,
    lookup: Lookup<'_>,
    subject: &PrincipalId,
    resume_object: Option<&ObjectTypeId>,
    resume_after: Option<RecordId>,
    held_objects: &[ObjectTypeId],
    addressed_invitations: &[InvitationId],
) -> Result<Output, DomainError> {
    let done = crate::erasure::decide(
        lookup,
        draft.ctx,
        subject,
        resume_object,
        resume_after,
        held_objects,
        addressed_invitations,
    )?;
    draft.ops.extend(done.ops);
    draft.events.extend(done.events);
    draft.effects.extend(done.effects);
    draft.evidence.extend(done.evidence);
    Ok(done.output)
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
    if grant && crate::products::is_staging_role(&role.0) {
        return Err(draft.reject(
            ErrorKind::InvalidIntent,
            format!(
                "the role {role} is reserved for a product's staged upgrade and is not assigned"
            ),
        ));
    }
    // ADM-152, ADM-1080: taking a role away asks as much of the actor as handing it out.
    require_conferrable(draft, [role])?;
    let mut roles = draft
        .tenant
        .members
        .get(principal)
        .cloned()
        .unwrap_or_default();
    let mut members = draft.tenant.members.clone();
    // D-8403: taking a role away takes the scoped assignments of it too.
    let mut schema_after: Option<TenantSchema> = None;
    if grant {
        roles.insert(role.clone());
        members.insert(principal.clone(), roles.clone());
        require_duties_apart(draft, &members, &draft.tenant.schema)?;
    } else {
        roles.remove(role);
        members.insert(principal.clone(), roles.clone());
        let (kept, changed) = crate::scoped::without(&draft.tenant.schema, principal, role);
        if changed {
            let mut schema = TenantSchema::clone(&draft.tenant.schema);
            schema.scoped_roles = kept;
            schema_after = Some(schema);
        }
        require_an_owner_remains(draft, &members, &draft.tenant.schema)?;
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
    if let Some(schema) = schema_after {
        draft.ops.push(DeltaOp::PutSchema(Arc::new(schema)));
    }
    draft.events.push(EventBody::MemberChanged {
        principal: principal.clone(),
        roles: roles.union(&through_teams).cloned().collect(),
    });
    Ok(Output::Member)
}

/// D-8403, ADM-1611: give a principal a role narrowed by a predicate over the records of one
/// object type. Asked as much of the acting principal as a direct assignment of the role
/// (ADM-152): it must hold every permission and capability the role grants, so a scope never
/// lets someone hand out a role they could not hand out whole. The owner role and `public` are
/// not narrowed. The predicate is checked against the fields of the object type, and a field it
/// does not declare is `ScopeUnknownField`. The assignment replaces one of the same role to the
/// same principal on the same object type, and adds nothing to the roles the principal holds
/// (see [`crate::scoped`]), so it is not a way around the owner count or the widening check.
fn assign_scoped_role(
    draft: &mut Draft<'_>,
    principal: &PrincipalId,
    role: &RoleId,
    object: &ObjectTypeId,
    scope: &Expr,
) -> Result<Output, DomainError> {
    draft.require(Action::Manage, &Resource::Members, Subject::default())?;
    if !draft.tenant.schema.roles.contains_key(role) {
        return Err(draft.reject(ErrorKind::UnknownRole, format!("no role {role}")));
    }
    if *role == public_role() || *role == owner_role() {
        return Err(draft.reject(
            ErrorKind::InvalidIntent,
            format!("the role {role} is not narrowed by a scope"),
        ));
    }
    if crate::products::is_staging_role(&role.0) {
        return Err(draft.reject(
            ErrorKind::InvalidIntent,
            format!("the role {role} is reserved for a staged upgrade and is not assigned"),
        ));
    }
    if let Some((kind, message)) =
        crate::scoped::predicate_problem(&draft.tenant.schema, object, scope)
    {
        draft
            .evidence
            .push(Check::fail("scope:predicate", message.clone()));
        return Err(draft.reject(kind, message));
    }
    require_conferrable(draft, [role])?;
    let mut schema = TenantSchema::clone(&draft.tenant.schema);
    let held = schema.scoped_roles.get_or_insert_with(Vec::new);
    held.retain(|a| !(a.principal == *principal && a.role == *role && a.object == *object));
    held.push(ScopedRole {
        principal: principal.clone(),
        role: role.clone(),
        object: object.clone(),
        scope: scope.clone(),
    });
    if held.len() > crate::scoped::MAX_SCOPED_ROLES {
        return Err(draft.reject(
            ErrorKind::LimitExceeded,
            format!(
                "more than {} scoped role assignments",
                crate::scoped::MAX_SCOPED_ROLES
            ),
        ));
    }
    require_duties_apart(draft, &draft.tenant.members, &schema)?;
    draft.ops.push(DeltaOp::PutSchema(Arc::new(schema)));
    Ok(Output::Member)
}

/// ADM-1643: take away the one scoped assignment of `role` that `principal` holds on `object`.
/// Asked as much of the acting principal as taking the role away (ADM-1080). The role a person
/// holds directly, and the same role scoped to another object type, are untouched, and an
/// assignment that does not exist is `InvalidIntent`, so a refusal never hides a typing mistake.
fn revoke_scoped_role(
    draft: &mut Draft<'_>,
    principal: &PrincipalId,
    role: &RoleId,
    object: &ObjectTypeId,
) -> Result<Output, DomainError> {
    draft.require(Action::Manage, &Resource::Members, Subject::default())?;
    if !draft.tenant.schema.roles.contains_key(role) {
        return Err(draft.reject(ErrorKind::UnknownRole, format!("no role {role}")));
    }
    require_conferrable(draft, [role])?;
    let Some(schema) = crate::scoped::without_one(&draft.tenant.schema, principal, role, object)
    else {
        return Err(draft.reject(
            ErrorKind::InvalidIntent,
            format!("{principal} holds no scoped role {role} on {object}"),
        ));
    };
    draft.ops.push(DeltaOp::PutSchema(Arc::new(schema)));
    Ok(Output::Member)
}

/// ADM-1630: a change must not give a principal two duties the platform keeps apart
/// ([`crate::duties`]). `members` and the schema are the state after the change; a pair the
/// tenant held before it is not made worse by it. The refusal is `ConflictingRoles`.
fn require_duties_apart(
    draft: &Draft<'_>,
    members: &BTreeMap<PrincipalId, BTreeSet<RoleId>>,
    schema: &TenantSchema,
) -> Result<(), DomainError> {
    match crate::duties::made_by(
        (&draft.tenant.members, &draft.tenant.schema),
        (members, schema),
    ) {
        Some(message) => Err(draft.reject(ErrorKind::ConflictingRoles, message)),
        None => Ok(()),
    }
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

/// The principals who hold `owner` in effect, directly or through a team.
fn effective_owners(
    members: &BTreeMap<PrincipalId, BTreeSet<RoleId>>,
    schema: &TenantSchema,
) -> BTreeSet<PrincipalId> {
    let mut candidates: BTreeSet<&PrincipalId> = members.keys().collect();
    for team in schema.all_teams() {
        candidates.extend(team.members.iter());
    }
    candidates
        .into_iter()
        .filter(|p| assigned_roles(members, schema, p).contains(&owner_role()))
        .cloned()
        .collect()
}

/// ADM-1081: a change that takes a role away from someone, a team or a team's membership must
/// leave the tenant an owner. `members` and `schema` are the state after the change; counting
/// goes through teams as well as direct assignments. A tenant that has no owner before the
/// change is not made worse by it, so it is not held to this.
fn require_an_owner_remains(
    draft: &Draft<'_>,
    members: &BTreeMap<PrincipalId, BTreeSet<RoleId>>,
    schema: &TenantSchema,
) -> Result<(), DomainError> {
    let before = effective_owners(&draft.tenant.members, &draft.tenant.schema).len();
    if before > 0 && effective_owners(members, schema).is_empty() {
        return Err(draft.reject(ErrorKind::LastOwner, "a tenant keeps at least one owner"));
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
    require_conferrable(draft, [role])?;
    if grant {
        def.roles.insert(role.clone());
    } else {
        def.roles.remove(role);
    }
    let schema = TenantSchema::clone(&draft.tenant.schema);
    let mut after = schema.clone();
    after.put_team(def.clone());
    if grant {
        require_duties_apart(draft, &draft.tenant.members, &after)?;
    } else {
        require_an_owner_remains(draft, &draft.tenant.members, &after)?;
    }
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
    require_conferrable(draft, def.roles.iter())?;
    if add {
        def.members.insert(principal.clone());
    } else {
        def.members.remove(principal);
    }
    let schema = TenantSchema::clone(&draft.tenant.schema);
    // The member is no longer in `def.members` after a removal, so it gets its own event here.
    let mut after = schema.clone();
    after.put_team(def.clone());
    if add {
        require_duties_apart(draft, &draft.tenant.members, &after)?;
    } else {
        require_an_owner_remains(draft, &draft.tenant.members, &after)?;
    }
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
        expires_at: Some(Timestamp(ctx.now.0.saturating_add(INVITATION_LIFETIME_MS))),
        revoked_by: None,
    });
    draft.ops.push(DeltaOp::PutSchema(Arc::new(schema)));
    Ok(Output::Member)
}

/// How long an invitation stays redeemable after it is issued, in milliseconds: fourteen days
/// (ADM-1082).
const INVITATION_LIFETIME_MS: i64 = 14 * 24 * 60 * 60 * 1000;

/// ADM-1083: withdraw an invitation that was not redeemed. Authorised as Manage on
/// Members, and the acting principal must be one who could confer every role the invitation
/// offers; the invitation stays in the schema, marked with who revoked it, and is never
/// redeemable again.
fn revoke_invitation(
    draft: &mut Draft<'_>,
    invitation: &InvitationId,
) -> Result<Output, DomainError> {
    draft.require(Action::Manage, &Resource::Members, Subject::default())?;
    let (ctx, tenant) = (draft.ctx, draft.tenant);
    let Principal::User(actor) = &ctx.principal else {
        return Err(draft.reject(ErrorKind::InvalidIntent, "a person revokes an invitation"));
    };
    let Some(offer) = tenant.schema.invitation(invitation) else {
        return Err(draft.reject(
            ErrorKind::InvalidIntent,
            format!("no invitation {invitation}"),
        ));
    };
    if offer.redeemed_by.is_some() {
        return Err(draft.reject(
            ErrorKind::InvalidIntent,
            "the invitation was already redeemed",
        ));
    }
    if offer.revoked_by.is_some() {
        return Err(draft.reject(
            ErrorKind::InvalidIntent,
            "the invitation was already revoked",
        ));
    }
    require_conferrable(draft, offer.roles.iter())?;
    let mut schema = TenantSchema::clone(&tenant.schema);
    if let Some(stored) = schema.invitation_mut(invitation) {
        stored.revoked_by = Some(actor.clone());
    }
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
    if let Some(by) = &offer.revoked_by {
        draft.evidence.push(Check::fail(
            "invitation:not_revoked",
            format!("revoked by {by}"),
        ));
        return Err(draft.reject(ErrorKind::InvalidIntent, "the invitation was revoked"));
    }
    draft
        .evidence
        .push(Check::pass("invitation:not_revoked", "not revoked"));
    // An invitation that records no expiry (one issued before expiry existed) does not expire
    // by date and so is never redeemable: it must be issued again.
    match offer.expires_at {
        Some(at) if ctx.now < at => draft
            .evidence
            .push(Check::pass("invitation:not_expired", "not expired")),
        _ => {
            draft.evidence.push(Check::fail(
                "invitation:not_expired",
                "the invitation has expired",
            ));
            return Err(draft.reject(ErrorKind::InvalidIntent, "the invitation has expired"));
        }
    }
    // ADM-1084: the issuer's authority is what the invitation stands on, so it is asked again
    // now: the issuer must still be allowed to manage members and to confer every role offered.
    let issuer = Principal::User(offer.created_by.clone());
    let (may_manage, _) = authorize(
        tenant,
        &tenant.schema,
        &issuer,
        ctx.now,
        Action::Manage,
        &Resource::Members,
        Subject::default(),
        None,
    );
    if !may_manage {
        draft.evidence.push(Check::fail(
            "invitation:issuer_authority",
            "the issuer may no longer manage members",
        ));
        return Err(draft.reject(
            ErrorKind::Unauthorized,
            "the invitation's issuer no longer holds the authority to issue it",
        ));
    }
    for role in &offer.roles {
        let check = may_confer(tenant, &issuer, role);
        if !check.passed {
            draft.evidence.push(check);
            draft.evidence.push(Check::fail(
                "invitation:issuer_authority",
                format!("the issuer may no longer confer {role}"),
            ));
            return Err(draft.reject(
                ErrorKind::Unauthorized,
                "the invitation's issuer no longer holds the authority to issue it",
            ));
        }
    }
    draft.evidence.push(Check::pass(
        "invitation:issuer_authority",
        "the issuer still holds the authority to issue it",
    ));
    let mut roles = tenant.members.get(redeemer).cloned().unwrap_or_default();
    roles.extend(offer.roles.iter().cloned());
    let mut schema = TenantSchema::clone(&tenant.schema);
    if let Some(stored) = schema.invitation_mut(invitation) {
        stored.redeemed_by = Some(redeemer.clone());
    }
    // ADM-1624, ADM-1630: redeeming gives roles, so it can end up with two duties kept apart in
    // one person, and it is checked for the owner too (it never takes one away).
    let mut after = tenant.members.clone();
    after.insert(redeemer.clone(), roles.clone());
    require_duties_apart(draft, &after, &schema)?;
    require_an_owner_remains(draft, &after, &schema)?;
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
pub(crate) fn apply_change(
    schema: &mut TenantSchema,
    change: &SchemaChange,
) -> Result<String, String> {
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
                    retention: None,
                    publication: None,
                    language_field: None,
                    translation_group: None,
                    unique: None,
                    searchable: None,
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
                personal: None,
                normalise: None,
                computed: None,
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
        SchemaChange::RenameField {
            object: id,
            from,
            to,
        } => {
            // The dependents that read the old name are rewritten first, against the schema as it
            // stands, then the field takes its new name (MIG-560, MIG-608).
            rewrite::rewrite_schema(change, schema)?;
            let def = object(schema, id)?;
            let mut field = def
                .fields
                .remove(from)
                .ok_or_else(|| format!("no field {id}.{from}"))?;
            field.id = to.clone();
            def.fields.insert(to.clone(), field);
            format!("rename field {id}.{from} to {to}")
        }
        SchemaChange::MoveField {
            object: id,
            field,
            via,
            to_field,
        } => {
            rewrite::rewrite_schema(change, schema)?;
            let target = schema
                .objects
                .get(id)
                .and_then(|o| o.fields.get(via))
                .and_then(|f| f.ty.ref_target().map(|(to, _, _)| to.clone()))
                .ok_or_else(|| format!("{id}.{via} is not a reference"))?;
            let moved = object(schema, id)?
                .fields
                .remove(field)
                .ok_or_else(|| format!("no field {id}.{field}"))?;
            object(schema, &target)?
                .fields
                .insert(to_field.clone(), rewrite::moved_def(&moved, to_field));
            format!("move field {id}.{field} to {target}.{to_field} along {via}")
        }
        SchemaChange::RetypeField {
            object: id,
            field,
            ty,
            ..
        } => {
            let def = object(schema, id)?
                .fields
                .get_mut(field)
                .ok_or_else(|| format!("no field {id}.{field}"))?;
            def.ty = ty.clone();
            format!("retype field {id}.{field}")
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
        SchemaChange::DefineTrigger(trigger) => {
            schema
                .triggers
                .get_or_insert_with(BTreeMap::new)
                .insert(trigger.id.clone(), trigger.clone());
            format!("define trigger {}", trigger.id)
        }
        SchemaChange::RemoveTrigger(id) => {
            remove_from(&mut schema.triggers, id).ok_or_else(|| format!("no trigger {id}"))?;
            format!("remove trigger {id}")
        }
        SchemaChange::DefineAutomation(automation) => {
            schema
                .automations
                .get_or_insert_with(BTreeMap::new)
                .insert(automation.id.clone(), automation.clone());
            format!("define automation {}", automation.id)
        }
        SchemaChange::RemoveAutomation(id) => {
            let defined = schema
                .automations
                .as_ref()
                .is_some_and(|automations| automations.contains_key(id));
            if !defined {
                return Err(format!("no automation {id}"));
            }
            // D-6539: an automation cannot be removed while a trigger names it.
            if let Some(trigger) = schema
                .triggers
                .iter()
                .flat_map(BTreeMap::values)
                .find(|trigger| &trigger.automation == id)
            {
                return Err(format!(
                    "automation {id} is named by trigger {}",
                    trigger.id
                ));
            }
            remove_from(&mut schema.automations, id);
            format!("remove automation {id}")
        }
        SchemaChange::DefineOperation(op) => {
            schema
                .operations
                .get_or_insert_with(BTreeMap::new)
                .insert(op.id.clone(), op.clone());
            format!("define operation {}", op.id)
        }
        SchemaChange::RemoveOperation(id) => {
            remove_from(&mut schema.operations, id).ok_or_else(|| format!("no operation {id}"))?;
            format!("remove operation {id}")
        }
        SchemaChange::DefineUnique { object: id, unique } => {
            object(schema, id)?
                .unique
                .get_or_insert_with(BTreeMap::new)
                .insert(unique.id.clone(), unique.clone());
            format!("define unique constraint {id}.{}", unique.id)
        }
        SchemaChange::RemoveUnique { object: id, unique } => {
            remove_from(&mut object(schema, id)?.unique, unique)
                .ok_or_else(|| format!("no unique constraint {id}.{unique}"))?;
            format!("remove unique constraint {id}.{unique}")
        }
        SchemaChange::DefineSetting(def) => crate::settings::apply_define(schema, def),
        SchemaChange::RemoveSetting(id) => crate::settings::apply_remove(schema, id)?,
        SchemaChange::DefineCapability(def) => crate::capabilities::apply_define(schema, def),
        SchemaChange::RemoveCapability(id) => crate::capabilities::apply_remove(schema, id)?,
        SchemaChange::DeclareLedgerUnit(_)
        | SchemaChange::DeclareLedgerPool(_)
        | SchemaChange::SetLedgerLimit(_)
        | SchemaChange::RemoveLedgerDefinition(_) => crate::ledger::apply_change(schema, change)?,
        SchemaChange::SetRetention { .. }
        | SchemaChange::ClearRetention { .. }
        | SchemaChange::SetPublication { .. }
        | SchemaChange::ClearPublication { .. }
        | SchemaChange::SetLanguageField { .. }
        | SchemaChange::ClearLanguageField { .. }
        | SchemaChange::SetTranslationGroup { .. }
        | SchemaChange::ClearTranslationGroup { .. }
        | SchemaChange::RemoveObject { .. }
        | SchemaChange::RemoveRole(_) => crate::object_meta::apply(schema, change)?,
        SchemaChange::SetSearchable { .. } | SchemaChange::ClearSearchable { .. } => {
            crate::search::apply(schema, change)?
        }
        SchemaChange::DefineMailTemplate(_)
        | SchemaChange::RemoveMailTemplate(_)
        | SchemaChange::DefineNoticeTemplate(_)
        | SchemaChange::RemoveNoticeTemplate(_)
        | SchemaChange::DefineToken(_)
        | SchemaChange::RemoveToken(_) => crate::templates::apply(schema, change)?,
        SchemaChange::DefineHoldRule(_) | SchemaChange::RemoveHoldRule(_) => {
            crate::holds::apply(schema, change)?
        }
        SchemaChange::DefineSeparation(_) | SchemaChange::RemoveSeparation(_) => {
            crate::duties::apply(schema, change)?
        }
        SchemaChange::RecordProduct(_) | SchemaChange::RemoveProduct(_) => {
            crate::products::apply(schema, change)?
        }
        SchemaChange::DefineUiElement { .. } | SchemaChange::RemoveUiElement(_) => {
            crate::ui::apply(schema, change)?
        }
    })
}

/// Remove `key` from an optional table; a table left empty is no table at all, so a schema
/// without such definitions reads and hashes as one that never had them.
fn remove_from<K: Ord, V>(table: &mut Option<BTreeMap<K, V>>, key: &K) -> Option<V> {
    let map = table.as_mut()?;
    let removed = map.remove(key);
    if map.is_empty() {
        *table = None;
    }
    removed
}

/// Two records of `object`, in id order, with equal values in every field of `unique`, none of
/// them null (D-6558, CMS-1556): the first such pair, or `None`.
fn duplicate_pair(
    lookup: Lookup<'_>,
    object: &ObjectTypeId,
    unique: &UniqueDef,
) -> Result<Option<(RecordId, RecordId)>, DomainError> {
    let mut seen: BTreeMap<Vec<&Value>, RecordId> = BTreeMap::new();
    for record in lookup.all_of(object)? {
        let values: Vec<&Value> = unique.fields.iter().map(|f| record.get(f)).collect();
        if values.iter().any(|value| value.is_null()) {
            continue;
        }
        if let Some(first) = seen.get(&values) {
            return Ok(Some((*first, record.id)));
        }
        seen.insert(values, record.id);
    }
    Ok(None)
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
    /// Records rewritten because a field they held was removed, as (before, after). The rewrite
    /// carries the next version and the time of the change.
    pub rewritten: Vec<(Arc<Record>, Arc<Record>)>,
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

    // CMS-1556: a composite unique constraint is not declared over records that already hold a
    // duplicate, whatever the change does with other records that do not conform. Checked once
    // per constraint, as the working copy holds it, for object types that already existed.
    let mut declared: BTreeSet<(ObjectTypeId, ConstraintId)> = BTreeSet::new();
    for change in changes {
        let SchemaChange::DefineUnique { object, unique } = change else {
            continue;
        };
        if !current.objects.contains_key(object)
            || !declared.insert((object.clone(), unique.id.clone()))
        {
            continue;
        }
        let Some(def) = schema
            .objects
            .get(object)
            .and_then(|o| o.unique.as_ref())
            .and_then(|u| u.get(&unique.id))
        else {
            continue;
        };
        if let Some((first, second)) = duplicate_pair(lookup, object, def)? {
            let rule = format!("unique:{object}.{}", def.id);
            let fields: Vec<&str> = def.fields.iter().map(|f| f.as_str()).collect();
            let detail = format!(
                "records {first} and {second} have the same values in {}",
                fields.join(", ")
            );
            evidence.push(Check::fail(rule.clone(), detail.clone()));
            return Err(
                DomainError::new(ErrorKind::UniqueViolated, format!("{rule}: {detail}"))
                    .with(evidence),
            );
        }
    }

    let mut rewritten = Vec::new();
    let mut violations = Vec::new();
    let moved = rewrite::plan_moves(lookup, changes, current)?;
    for object in touched_objects(current, changes) {
        let (Some(def), true) = (
            schema.objects.get(&object),
            current.objects.contains_key(&object),
        ) else {
            continue;
        };
        for record in lookup.all_of(&object)? {
            // A rename carries the value under the new name, a move brings the value to the
            // record it moves to, a retype re-encodes it (MIG-560); then what the object type no
            // longer declares is dropped.
            let mut carried = rewrite::carry_fields(changes, &object, &record.fields, now)
                .map_err(|e| DomainError::new(ErrorKind::MigrationRejected, e))?;
            if let Some(arrived) = moved.get(&(object.clone(), record.id)) {
                carried.extend(arrived.clone());
            }
            let mut stripped: BTreeMap<FieldId, Value> = carried
                .into_iter()
                .filter(|(field, _)| def.fields.contains_key(field))
                .collect();
            crate::normalise::normalise_fields(def, &mut stripped);
            let was_rewritten = stripped != record.fields;
            let candidate = if stripped == record.fields {
                record.clone()
            } else {
                let rewritten_record = Arc::new(Record {
                    fields: stripped,
                    version: record.version + 1,
                    updated_at: now,
                    ..Record::clone(record)
                });
                rewritten.push((record.clone(), rewritten_record.clone()));
                rewritten_record
            };
            // A computed field of the new schema is derived for the record as it now stands: a
            // record that already had a value gets it when the field is defined (D-8307).
            let computed = crate::computed::values(
                lookup,
                def,
                &candidate,
                &Principal::System,
                &BTreeSet::new(),
                now,
            )?;
            let candidate = if computed
                .iter()
                .all(|(field, value)| candidate.get(field) == value)
            {
                candidate
            } else {
                let mut recomputed = Record::clone(&candidate);
                crate::computed::store(&mut recomputed, computed);
                recomputed.version = record.version + 1;
                recomputed.updated_at = now;
                let recomputed = Arc::new(recomputed);
                if was_rewritten {
                    if let Some(last) = rewritten.last_mut() {
                        last.1 = recomputed.clone();
                    }
                } else {
                    rewritten.push((record.clone(), recomputed.clone()));
                }
                recomputed
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

/// The class and the impact of each change of a list, as the list is applied to `base` and
/// leaves `after`. A change is classified against the schema as it stands when it is applied; its
/// impact is read in the graph of `base` and the graph of `after` (see
/// [`dependencies::impact_between`]). Stops at a change that cannot be applied.
pub fn assess_changes(
    base: &TenantSchema,
    changes: &[SchemaChange],
    after: &TenantSchema,
) -> Vec<Assessment> {
    let mut working = base.clone();
    let mut steps = Vec::new();
    for change in changes {
        let class = dependencies::classify(change, &working);
        let footprint = dependencies::footprint(change, &working);
        // An adaptable change reaches the dependents that read what it moves, and rewrites each
        // of them: its impact is read in the graph of the schema it is applied to, and the
        // dependents it rewrites are listed in the order they are rewritten (MIG-608).
        let (reached, rewritten) = if class == ChangeClass::Adaptable {
            let graph = DependencyGraph::of(&working);
            let mut probe = working.clone();
            let rewritten = rewrite::rewrite_schema(change, &mut probe).unwrap_or_default();
            (
                Some(dependencies::impact_between(&footprint, &graph, &graph)),
                rewritten,
            )
        } else {
            (None, Vec::new())
        };
        let Ok(description) = apply_change(&mut working, change) else {
            break;
        };
        steps.push((description, class, footprint, reached, rewritten));
    }
    let before = DependencyGraph::of(base);
    let after = DependencyGraph::of(after);
    steps
        .into_iter()
        .map(
            |(description, class, footprint, reached, rewritten)| Assessment {
                description,
                class,
                impact: reached
                    .unwrap_or_else(|| dependencies::impact_between(&footprint, &before, &after)),
                footprint,
                rewritten,
            },
        )
        .collect()
}

fn change_schema(
    draft: &mut Draft<'_>,
    lookup: Lookup<'_>,
    changes: &[SchemaChange],
    expected_version: Option<SchemaVersion>,
    existing: ExistingData,
    pins: &mut PinBook,
) -> Result<Output, DomainError> {
    draft.require(Action::Manage, &Resource::Schema, Subject::default())?;
    // D-7204: who defined a trigger or an automation, and when, comes from the context.
    let stamped = crate::triggers::stamp_definitions(changes, draft.ctx);
    let changes = stamped.as_slice();
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
    require_definition_authority(draft, changes)?;
    require_owner_role_kept(draft, changes)?;
    // D-6582, INT-1582: an entity a product owns changes only through that product.
    if let Some(message) = crate::products::foreign_change(&draft.tenant.schema, changes) {
        return Err(draft.reject(ErrorKind::InvalidSchema, message));
    }
    crate::ledger::check_removals(lookup, changes, &mut draft.evidence)?;
    // D-8215, MIG-599: the old shape of a pinned entity is removed only when no pin on it remains.
    // A breaking change that is not a retype has no left inverse (MIG-557), so a dependent pinned
    // to what it removes keeps it waiting. Said here, before the consistency check below finds
    // the dependent reading what is gone.
    if !pins.pins.is_empty() {
        let mut working = TenantSchema::clone(&draft.tenant.schema);
        for change in changes {
            let breaking = dependencies::classify(change, &working) == ChangeClass::Breaking;
            if breaking && !matches!(change, SchemaChange::RetypeField { .. }) {
                let held: Vec<String> = dependencies::impact(change, &working)
                    .into_iter()
                    .filter(|dependent| pins.is_pinned(dependent))
                    .map(|dependent| dependent.to_string())
                    .collect();
                if !held.is_empty() {
                    let message = format!(
                        "{} has no left inverse (MIG-557), so it is not projected and waits until the pins of {} end (MIG-599)",
                        apply_change(&mut working.clone(), change).unwrap_or_default(),
                        held.join(", ")
                    );
                    draft
                        .evidence
                        .push(Check::fail("schema:pin", message.clone()));
                    return Err(draft.reject(ErrorKind::InvalidSchema, message));
                }
            }
            if apply_change(&mut working, change).is_err() {
                break;
            }
        }
    }
    let migration = plan_migration(lookup, changes, draft.ctx.now).map_err(|mut e| {
        let mut evidence = draft.evidence.clone();
        evidence.append(&mut e.evidence);
        e.evidence = evidence;
        e
    })?;
    // D-8212: the class of each change and the dependents it reaches are computed from the
    // dependency graph, shown in the evidence, and a breaking change that reaches a dependent is
    // refused naming it (MIG-592). The same check refuses a name that is removed and defined again
    // while a dependent reads it, which would otherwise bind the dependent to another entity
    // (D-8217, MIG-604).
    let assessments = assess_changes(&draft.tenant.schema, changes, &migration.schema);
    for assessment in &assessments {
        let reached: Vec<String> = assessment.impact.iter().map(|d| d.to_string()).collect();
        let detail = if reached.is_empty() {
            format!("{}: {}", assessment.description, assessment.class.name())
        } else {
            format!(
                "{}: {}, reaches {}",
                assessment.description,
                assessment.class.name(),
                reached.join(", ")
            )
        };
        draft.evidence.push(Check::pass("schema:class", detail));
    }
    // D-8212, MIG-608: what an adaptable change rewrote, each dependent once, after those it uses.
    for assessment in &assessments {
        for dependent in &assessment.rewritten {
            draft.evidence.push(Check::pass(
                "schema:rewrite",
                format!("{}: {dependent} rewritten", assessment.description),
            ));
        }
    }
    let mut refusals = Vec::new();
    for (change, assessment) in changes.iter().zip(&assessments) {
        if assessment.class != ChangeClass::Breaking || assessment.impact.is_empty() {
            continue;
        }
        // D-8214: a dependent pinned to the version in force reads a breaking change through a
        // projection, where the change has a left inverse on the data (MIG-596, MIG-597).
        match evolution::decide_breaking(change, assessment, pins, draft.ctx.now) {
            evolution::Verdict::Pinned => {
                for dependent in &assessment.impact {
                    draft.evidence.push(Check::pass(
                        "schema:pin",
                        format!(
                            "{}: {dependent} is pinned and reads through the projection",
                            assessment.description
                        ),
                    ));
                }
                continue;
            }
            evolution::Verdict::Refused(message) if !pins.pins.is_empty() => {
                for dependent in &assessment.impact {
                    draft.evidence.push(Check::fail(
                        "schema:impact",
                        format!(
                            "{}: {dependent} is still bound to what the change removes or replaces",
                            assessment.description
                        ),
                    ));
                }
                refusals.push(message);
                continue;
            }
            evolution::Verdict::Refused(_) => {}
        }
        let reached: Vec<String> = assessment.impact.iter().map(|d| d.to_string()).collect();
        for dependent in &reached {
            draft.evidence.push(Check::fail(
                "schema:impact",
                format!(
                    "{}: {dependent} is still bound to what the change removes or replaces",
                    assessment.description
                ),
            ));
        }
        refusals.push(format!(
            "{} is a breaking change that reaches {}",
            assessment.description,
            reached.join(", ")
        ));
    }
    if !refusals.is_empty() {
        return Err(draft.reject(ErrorKind::InvalidSchema, refusals.join("; ")));
    }
    // MIG-597: a retype that reaches a pinned dependent needs a decode that undoes its encode on
    // every value stored.
    for (change, assessment) in changes.iter().zip(&assessments) {
        let SchemaChange::RetypeField {
            object,
            field,
            encode,
            decode,
            ..
        } = change
        else {
            continue;
        };
        if assessment.class != ChangeClass::Breaking || assessment.impact.is_empty() {
            continue;
        }
        let stored: Vec<Value> = lookup
            .all_of(object)?
            .iter()
            .map(|record| record.get(field).clone())
            .collect();
        if let Err(problem) =
            evolution::check_left_inverse(encode, decode, stored.iter(), draft.ctx.now)
        {
            draft
                .evidence
                .push(Check::fail("schema:projection", problem.clone()));
            return Err(draft.reject(
                ErrorKind::InvalidSchema,
                format!(
                    "{}: no projection: {problem} (MIG-597)",
                    assessment.description
                ),
            ));
        }
        draft.evidence.push(Check::pass(
            "schema:projection",
            format!(
                "{}: decode undoes encode on the stored values",
                assessment.description
            ),
        ));
    }
    *pins = evolution::advance(pins, changes, &assessments);
    // The overrides and revisions that a change of setting definitions brings with it, or the
    // refusal of a type that changes under overrides (SET-004).
    let settings = crate::settings::schema_change_effects(draft.tenant, changes)
        .map_err(|problem| draft.reject(ErrorKind::InvalidSchema, problem))?;
    draft.evidence.extend(
        migration
            .summary
            .iter()
            .map(|line| Check::pass("schema:change", line.clone())),
    );
    // KRN-2524: which queries are scans, and why.
    draft.evidence.extend(crate::query_plan::scan_warnings(
        &draft.tenant.schema,
        &migration.schema,
        changes,
    ));

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

    // The records of the object types the changes remove (D-6581): refused unless the change says
    // they are deleted, and then deleted in this commit.
    let removed = match crate::object_meta::deleted_records(lookup, changes)? {
        Ok(records) => records,
        Err(message) => return Err(draft.reject(ErrorKind::MigrationRejected, message)),
    };
    if let Some(message) = crate::object_meta::held_roles(lookup.tenant, changes) {
        return Err(draft.reject(ErrorKind::InvalidSchema, message));
    }

    // ADM-1624, ADM-1630: what the schema now says each role grants must neither leave the tenant
    // without an owner nor give a principal two duties the platform keeps apart.
    require_an_owner_remains(draft, &draft.tenant.members, &migration.schema)?;
    require_duties_apart(draft, &draft.tenant.members, &migration.schema)?;

    let version = migration.schema.version;
    draft
        .ops
        .push(DeltaOp::PutSchema(Arc::new(migration.schema)));
    draft.events.push(EventBody::SchemaChanged {
        version,
        summary: migration.summary,
        grandfathered: affected.len() as u64,
    });
    draft.ops.extend(settings.ops);
    draft.events.extend(settings.events);
    // The limits of single accounts are written to the ledger's storage, not to the schema.
    draft.ops.extend(crate::ledger::limit_ops(changes));
    for (before, after) in migration.rewritten {
        draft.ops.push(DeltaOp::PutRecord(after.clone()));
        draft.events.push(EventBody::RecordUpdated {
            before,
            after,
            transitions: None,
        });
    }
    for before in removed {
        draft.ops.push(DeltaOp::DeleteRecord {
            object: before.object.clone(),
            id: before.id,
        });
        draft.events.push(EventBody::RecordDeleted { before });
    }
    Ok(Output::Schema { version })
}

/// ADM-1624: a change of role definitions does not remove or redefine the built-in `owner` role
/// while the tenant has an owner, because every owner would be demoted with it. A product's
/// install, upgrade and staged upgrade reach the schema through this same change, so the rule
/// holds for them too. The refusal is `LastOwner`.
fn require_owner_role_kept(draft: &Draft<'_>, changes: &[SchemaChange]) -> Result<(), DomainError> {
    let touches_owner = changes.iter().any(|change| match change {
        SchemaChange::RemoveRole(role) => *role == owner_role(),
        SchemaChange::DefineRole(def) => def.id == owner_role(),
        _ => false,
    });
    if touches_owner && !effective_owners(&draft.tenant.members, &draft.tenant.schema).is_empty() {
        return Err(draft.reject(
            ErrorKind::LastOwner,
            "a tenant keeps at least one owner: the owner role is not redefined or removed",
        ));
    }
    Ok(())
}

/// The authority a change of definitions asks of the principal making it, beyond Manage on
/// Schema: the capability automations-define for any change of a trigger or an automation
/// (WFL-1520); for an automation, every role of its run-as is one the principal may confer
/// (ADM-152, WFL-1528); for a tenant operation, every power its steps use (D-6547, OPR-1547).
/// Checked in the order of the changes, the capability first.
fn require_definition_authority(
    draft: &mut Draft<'_>,
    changes: &[SchemaChange],
) -> Result<(), DomainError> {
    let automation = changes.iter().any(|change| {
        matches!(
            change,
            SchemaChange::DefineTrigger(_)
                | SchemaChange::RemoveTrigger(_)
                | SchemaChange::DefineAutomation(_)
                | SchemaChange::RemoveAutomation(_)
        )
    });
    if automation {
        require_capability(draft, AUTOMATIONS_DEFINE)?;
    }
    if changes.iter().any(crate::settings::is_setting_change) {
        require_capability(draft, crate::settings::SETTING_DEFINE)?;
    }
    if changes.iter().any(crate::holds::is_hold_change) {
        require_capability(draft, crate::holds::HOLDS_DEFINE)?;
    }
    if changes.iter().any(crate::duties::is_separation_change) {
        require_capability(draft, crate::duties::SEPARATIONS_DEFINE)?;
    }
    if changes.iter().any(crate::ledger::is_ledger_change) {
        require_capability(draft, crate::ledger::LEDGER_CONFIGURE)?;
    }
    // A product record is the proof that a change is that product's install, upgrade or removal.
    if changes.iter().any(crate::products::is_product_change) {
        require_capability(draft, crate::products::BUNDLE_INSTALL)?;
    }
    for change in changes {
        match change {
            SchemaChange::DefineAutomation(def) => {
                require_conferrable(draft, def.run_as.roles.iter())?
            }
            SchemaChange::DefineOperation(op) => require_operation_powers(draft, op)?,
            _ => {}
        }
    }
    for change in changes {
        require_powers_held(draft, change)?;
    }
    Ok(())
}

/// ADM-1085, ADM-152: a change that adds a power to a role (a permission or capability the role
/// grants, or a power one of its policies gives it) is refused unless the acting principal holds
/// every power the change adds. The owner holds them all. Each power the principal lacks is named
/// in the refusal. A change that adds nothing to a role, and a change that only removes a power,
/// is not refused here.
///
/// The check runs against the schema before the change, so the powers a change adds are the ones
/// the role did not hold before it. It covers every path that changes a role's powers through
/// `ChangeSchema`, which includes a product's install, upgrade and staged upgrade. Team roles and
/// operation invokers keep their own checks (ADM-152, D-6547).
fn require_powers_held(draft: &mut Draft<'_>, change: &SchemaChange) -> Result<(), DomainError> {
    let added = crate::rules::powers_added(&draft.tenant.schema, change);
    let Some((role, _)) = added.first() else {
        return Ok(());
    };
    let role = role.clone();
    let (tenant, ctx) = (draft.tenant, draft.ctx);
    let lacking: Vec<String> = added
        .iter()
        .filter(|(_, power)| !crate::rules::holds_role_power(tenant, &ctx.principal, power))
        .map(|(_, power)| crate::rules::describe_role_power(power))
        .collect();
    let rule = format!("permission:widen:{role}");
    if lacking.is_empty() {
        draft.evidence.push(Check::pass(
            rule,
            format!("holds every power the change adds to role {role}"),
        ));
        Ok(())
    } else {
        let detail = lacking.join(", ");
        draft.evidence.push(Check::fail(rule, detail.clone()));
        Err(draft.reject(
            ErrorKind::Unauthorized,
            format!("may not widen role {role}: lacks {detail}"),
        ))
    }
}

/// The acting principal holds the capability `name`; the owner holds every capability.
fn require_capability(draft: &mut Draft<'_>, name: &str) -> Result<(), DomainError> {
    let rule = format!("permission:capability:{name}");
    let wanted = BTreeSet::from([Grant::Capability(name.to_string())]);
    if holds_any(draft.tenant, &draft.ctx.principal, &wanted) {
        draft
            .evidence
            .push(Check::pass(rule, format!("holds {name}")));
        Ok(())
    } else {
        draft
            .evidence
            .push(Check::fail(rule, format!("does not hold {name}")));
        Err(draft.reject(
            ErrorKind::Unauthorized,
            format!("the change needs the capability {name}"),
        ))
    }
}

/// D-6547, OPR-1547: a tenant operation grants its invokers no more than its definer holds, so
/// the definer must hold, unconditionally, each action on an object type its record steps use
/// and the capability of its posting steps.
fn require_operation_powers(
    draft: &mut Draft<'_>,
    op: &TenantOperationDef,
) -> Result<(), DomainError> {
    let (actions, capabilities) = operation_powers(op);
    let (tenant, ctx) = (draft.tenant, draft.ctx);
    let principal = &ctx.principal;
    let mut lacking: Vec<String> = actions
        .iter()
        .filter(|(action, resource)| !holds_power(tenant, principal, *action, resource))
        .map(|(action, resource)| format!("{action:?} on {resource:?}"))
        .collect();
    let wanted: BTreeSet<Grant> = capabilities
        .iter()
        .map(|name| Grant::Capability(name.clone()))
        .collect();
    for grant in missing_grants(tenant, principal, &wanted) {
        if let Grant::Capability(name) = grant {
            lacking.push(format!("capability {name}"));
        }
    }
    let rule = format!("operation:{}:definer_powers", op.id);
    if lacking.is_empty() {
        draft.evidence.push(Check::pass(
            rule,
            "the definer holds every power the steps use",
        ));
        Ok(())
    } else {
        let detail = lacking.join(", ");
        draft.evidence.push(Check::fail(rule, detail.clone()));
        Err(draft.reject(
            ErrorKind::Unauthorized,
            format!("may not define operation {}: lacks {detail}", op.id),
        ))
    }
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
                setting_values: None,
                ledger: None,
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
            DeltaOp::PutSettingOverride(_)
            | DeltaOp::DeleteSettingOverride { .. }
            | DeltaOp::PutSettingRevision { .. } => crate::settings::apply_op(&mut next, op),
            DeltaOp::Ledger(change) => crate::ledger::apply(&mut next, change),
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
