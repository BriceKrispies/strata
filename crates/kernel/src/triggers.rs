//! Running the triggers a tenant declares: the kernel's half (D-6524, D-7200 to D-7204).
//!
//! [`crate::definitions`] holds and checks triggers and automations; this module decides them.
//! Once the events of an accepted transition are final, [`after_decision`]:
//!
//! 1. when the transition changed members or the schema, checks every automation that has a
//!    definer against the state after the change, and suspends in the same transition each one
//!    whose run-as names a role its definer may no longer confer (WFL-1529, CT-798): a
//!    `PutSchema` with the automation marked `suspended-for` that role and an
//!    `AutomationSuspended` event;
//! 2. for each event of the transition, in sequence order, and each trigger of the tenant whose
//!    event matches it, in trigger id order, decides the trigger and appends one effect:
//!    `StartAutomation` ([`AutomationStart`]) for a run to start, `DeclineAutomation`
//!    ([`AutomationDecline`]) for a run that is declined for a reason the tenant owner sees
//!    (SCH-1532). A false condition, a paused trigger and an event about a record the automation
//!    principal may not read append nothing (D-6530, D-7201).
//!
//! The automation worker consumes the effects: it creates the job of the run, or writes the
//! declined exception record, in the commit of the transition (SCH-004), and it refuses a
//! repeated run key (D-6534, from the hash this module computes) and holds jobs past the quota
//! (D-6533), neither of which the kernel holds.
//!
//! A condition and a run key are evaluated as the automation principal
//! `automation:<workflow-id>`, which holds its run-as roles and no other (D-6525, D-7201). Its
//! definer may confer every one of them (WFL-1528, WFL-1529), so whatever it may read its definer
//! may read, which D-6524 asks of a condition.
//!
//! Bound per commit: one decision for each pair of an event of the transition and a trigger whose
//! event matches it, so at most [`crate::limits::MAX_TRIGGERS`] (one hundred) decisions per event;
//! each decision is one read decision for each record the event binds, one evaluation of the
//! condition and of the run key, and one record lookup and read decision per reference they
//! follow. The authority check after a change of members or of the schema is one conferral check
//! per run-as role of each automation that has a definer. The state after the change is built
//! once per commit, and only when a trigger matches or the commit changed members or the schema.

use crate::canon;
use crate::expr::{self, Env, Expr, Scope};
use crate::intent::{
    Check, CheckExt, Context, DeltaOp, DomainError, DomainEvent, Effect, EventBody, SchemaChange,
    Transition,
};
use crate::read::{redact, visible_in};
use crate::rules::{insufficient, may_confer, roles_of, Lookup};
use crate::schema::TenantSchema;
use crate::state::{Principal, Record, RecordExt, TenantState};
use crate::transition::apply_delta;
use crate::types::{
    AutomationDef, FieldId, FieldTypeExt, LedgerOwner, PrincipalId, RoleId, TriggerDef,
    TriggerEvent, Value, WorkflowId,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The hand-off to the automation worker (D-7200), and why a trigger declined.
pub use strata_model_core::types::{AutomationDecline, AutomationExceptionReason, AutomationStart};

/// The hop limit of D-700: a trigger starts a run for an event whose hop count is at most eight,
/// and declines with `hop-limit` past it (SCH-300, SCH-1532, D-7202).
pub const HOP_LIMIT: u32 = 8;

/// The automation principal and its reserved prefix live in [`crate::automation`], the one rule
/// the worker and the triggers share (D-6525, WFL-1527).
pub use crate::automation::{automation_principal, automation_roles, AUTOMATION_PREFIX};

/// The longest detail a declined exception carries (rec-automation-exception.detail).
const MAX_DETAIL: usize = 256;

/// The changes of a schema change with every definition of a trigger or an automation stamped
/// with who defined it and when, from the context and never from the input (D-7204, WFL-2201):
/// `defined-by` is the acting person (none for the system or an anonymous caller), `defined-at`
/// the time of the context. An automation defined again is no longer suspended: the change that
/// defines it checks that its definer may confer every run-as role before it is accepted
/// (WFL-1528), which is what lifts a suspension (D-6526).
pub fn stamp_definitions(changes: &[SchemaChange], ctx: &Context) -> Vec<SchemaChange> {
    let definer = match &ctx.principal {
        Principal::User(id) => Some(id.clone()),
        _ => None,
    };
    changes
        .iter()
        .map(|change| match change {
            SchemaChange::DefineTrigger(def) => SchemaChange::DefineTrigger(TriggerDef {
                defined_by: definer.clone(),
                defined_at: Some(ctx.now),
                ..def.clone()
            }),
            SchemaChange::DefineAutomation(def) => SchemaChange::DefineAutomation(AutomationDef {
                defined_by: definer.clone(),
                defined_at: Some(ctx.now),
                suspended_for: None,
                ..def.clone()
            }),
            other => other.clone(),
        })
        .collect()
}

/// An automation whose definer may no longer confer a role of its run-as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LostAuthority {
    pub automation: WorkflowId,
    pub definer: PrincipalId,
    /// The first role of the run-as, in id order, that the definer may not confer.
    pub role: RoleId,
    /// What the definer lacks, as the conferral check says it.
    pub detail: String,
}

/// The automations of `tenant` (a state after a change) whose definer may no longer confer a role
/// of their run-as under ADM-152, in id order (WFL-1529, CT-798). An automation that is already
/// suspended, or that has no definer, is not checked.
pub fn lost_authority(tenant: &TenantState) -> Vec<LostAuthority> {
    let mut out = Vec::new();
    for def in tenant.schema.automations.iter().flat_map(BTreeMap::values) {
        if def.suspended_for.is_some() {
            continue;
        }
        let Some(definer) = &def.defined_by else {
            continue;
        };
        let who = Principal::User(definer.clone());
        for role in &def.run_as.roles {
            let check = may_confer(tenant, &who, role);
            if !check.passed {
                out.push(LostAuthority {
                    automation: def.id.clone(),
                    definer: definer.clone(),
                    role: role.clone(),
                    detail: check.detail,
                });
                break;
            }
        }
    }
    out
}

/// The workflow transitions a record-updated event says it fired, in workflow id order.
fn fired(transitions: &Option<BTreeMap<WorkflowId, String>>) -> impl Iterator<Item = &String> {
    transitions.iter().flat_map(BTreeMap::values)
}

/// Whether a trigger's event matches a domain event (D-6522, D-7200): the kind and the object type
/// and, for record-updated naming a transition, a transition the update fired (D-7203). The other
/// trigger events come from engines outside the kernel and match no kernel event; a ledger-posted
/// trigger matches the entry posted to a ledger unit it names (D-6523).
pub fn matches(on: &TriggerEvent, body: &EventBody) -> bool {
    match (on, body) {
        (TriggerEvent::RecordCreated { object }, EventBody::RecordCreated { record }) => {
            record.object == *object
        }
        (
            TriggerEvent::RecordUpdated { object, transition },
            EventBody::RecordUpdated {
                after, transitions, ..
            },
        ) => {
            after.object == *object
                && transition
                    .as_ref()
                    .is_none_or(|name| fired(transitions).any(|t| t == name))
        }
        (TriggerEvent::RecordDeleted { object }, EventBody::RecordDeleted { before }) => {
            before.object == *object
        }
        (
            TriggerEvent::LedgerPosted { unit },
            EventBody::LedgerEntryPosted { unit: posted, .. },
        ) => unit == posted,
        _ => false,
    }
}

/// The parameters an event binds for a trigger (D-6523): `seq`, `at` and `hop`, and for
/// record-updated `transition`: the transition the trigger names, else the first the update
/// fired in workflow id order, else null.
pub fn event_params(event: &DomainEvent, hop: u32, on: &TriggerEvent) -> BTreeMap<String, Value> {
    let mut params = BTreeMap::from([
        (
            "seq".to_string(),
            Value::Int(i64::try_from(event.seq).unwrap_or(i64::MAX)),
        ),
        ("at".to_string(), Value::Time(event.at.0)),
        ("hop".to_string(), Value::Int(i64::from(hop))),
    ]);
    // ledger-posted binds the entry and the balance on both sides of it (D-6523); a pool has no
    // principal, so its owner is null.
    if let EventBody::LedgerEntryPosted {
        owner,
        unit,
        amount,
        balance,
        balance_before,
        entry_sequence,
        reason,
        ..
    } = &event.body
    {
        params.extend([
            (
                "owner".to_string(),
                match owner {
                    LedgerOwner::Principal { principal } => Value::Str(principal.to_string()),
                    LedgerOwner::Pool { .. } => Value::Null,
                },
            ),
            ("unit".to_string(), Value::Str(unit.to_string())),
            ("amount".to_string(), Value::Int(amount.0)),
            ("balance".to_string(), Value::Int(balance.0)),
            ("balance_before".to_string(), Value::Int(balance_before.0)),
            (
                "entry_sequence".to_string(),
                Value::Int(i64::try_from(*entry_sequence).unwrap_or(i64::MAX)),
            ),
            ("reason".to_string(), Value::Str(reason.clone())),
        ]);
    }
    if let EventBody::RecordUpdated { transitions, .. } = &event.body {
        let name = match on {
            TriggerEvent::RecordUpdated {
                transition: Some(name),
                ..
            } => Some(name.clone()),
            _ => fired(transitions).next().cloned(),
        };
        params.insert(
            "transition".to_string(),
            name.map_or(Value::Null, Value::Str),
        );
    }
    params
}

/// The record scopes an event binds (D-6523): record-created binds `record` and `new`,
/// record-updated `old` and `new`, record-deleted `old`.
struct Scopes<'a> {
    record: Option<&'a Record>,
    old: Option<&'a Record>,
    new: Option<&'a Record>,
}

/// The scopes of an event as the automation principal may observe them: `None` when it may not
/// read the record the event leaves (the record created, the record after an update, the record
/// deleted), so the trigger does not observe the event; the scope `old` of an update only when
/// it may read the record as it was (D-7201).
fn scopes<'a>(
    lookup: Lookup<'_>,
    principal: &Principal,
    now: crate::types::Timestamp,
    body: &'a EventBody,
) -> Result<Option<Scopes<'a>>, DomainError> {
    Ok(match body {
        EventBody::RecordCreated { record } => {
            let record: &'a Record = record;
            visible_in(lookup, principal, record, now)?.then_some(Scopes {
                record: Some(record),
                old: None,
                new: Some(record),
            })
        }
        EventBody::RecordUpdated { before, after, .. } => {
            let before: &'a Record = before;
            let after: &'a Record = after;
            if visible_in(lookup, principal, after, now)? {
                let old = visible_in(lookup, principal, before, now)?.then_some(before);
                Some(Scopes {
                    record: None,
                    old,
                    new: Some(after),
                })
            } else {
                None
            }
        }
        EventBody::RecordDeleted { before } => {
            let before: &'a Record = before;
            visible_in(lookup, principal, before, now)?.then_some(Scopes {
                record: None,
                old: Some(before),
                new: None,
            })
        }
        // A posting is about no record: it binds parameters only.
        EventBody::LedgerEntryPosted { .. } => Some(Scopes {
            record: None,
            old: None,
            new: None,
        }),
        _ => None,
    })
}

/// A field an expression reads: of a scope's record, or of the record a reference field of the
/// scope's record points at.
enum Reads<'e> {
    Field(Scope, &'e str),
    Through(Scope, &'e str, &'e str),
}

/// Every field an expression reads, in the order it is written.
fn reads<'e>(expr: &'e Expr, out: &mut Vec<Reads<'e>>) {
    match expr {
        Expr::Var(scope, name) => out.push(Reads::Field(*scope, name)),
        Expr::Through(scope, reference, field) => {
            out.push(Reads::Through(*scope, reference, field))
        }
        Expr::Lit(_) | Expr::Param(_) | Expr::Now | Expr::Principal | Expr::HasRole(_) => {}
        Expr::Not(a)
        | Expr::IsNull(a)
        | Expr::Len(a)
        | Expr::Lower(a)
        | Expr::IsSlug(a)
        | Expr::Duration(_, a)
        | Expr::Text(a) => reads(a, out),
        Expr::And(items) | Expr::Or(items) => {
            for item in items {
                reads(item, out);
            }
        }
        Expr::Cmp(_, a, b)
        | Expr::In(a, b)
        | Expr::Contains(a, b)
        | Expr::StartsWith(a, b)
        | Expr::Arith(_, a, b)
        | Expr::MsBetween(a, b)
        | Expr::Concat(a, b) => {
            reads(a, out);
            reads(b, out);
        }
        Expr::If(a, b, c) => {
            reads(a, out);
            reads(b, out);
            reads(c, out);
        }
    }
}

/// Whether the read rule of `field` hides it from the principal on `record` (D-6586): the record
/// holds the field and the record as the principal receives it does not.
fn hidden(
    lookup: Lookup<'_>,
    principal: &Principal,
    roles: &BTreeSet<RoleId>,
    now: crate::types::Timestamp,
    record: &Record,
    field: &str,
) -> Result<bool, DomainError> {
    let Ok(id) = FieldId::new(field) else {
        return Ok(false);
    };
    if !record.fields.contains_key(&id) {
        return Ok(false);
    }
    let shown = redact(lookup, principal, roles, now, &Arc::new(record.clone()))?;
    Ok(!shown.fields.contains_key(&id))
}

/// The first field an expression reads that the automation principal may not read, described by
/// ids only (D-7201): a field of a scope's record its read rule hides, a record a reference
/// leads to that the principal may not read, or a field of it that its read rule hides. `None`
/// when every field is readable. A record the snapshot does not cover is `InsufficientState`.
fn unreadable(
    lookup: Lookup<'_>,
    principal: &Principal,
    roles: &BTreeSet<RoleId>,
    now: crate::types::Timestamp,
    expr: &Expr,
    scopes: &Scopes<'_>,
) -> Result<Option<String>, DomainError> {
    let scope_record = |scope: &Scope| match scope {
        Scope::Record => scopes.record,
        Scope::Old => scopes.old,
        Scope::New => scopes.new,
    };
    let mut found = Vec::new();
    reads(expr, &mut found);
    for read in found {
        match read {
            Reads::Field(scope, field) => {
                let Some(record) = scope_record(&scope) else {
                    continue;
                };
                if hidden(lookup, principal, roles, now, record, field)? {
                    return Ok(Some(format!(
                        "reads {}.{field}, which the automation may not read",
                        record.object
                    )));
                }
            }
            Reads::Through(scope, reference, field) => {
                let Some(record) = scope_record(&scope) else {
                    continue;
                };
                let Ok(id) = FieldId::new(reference) else {
                    continue;
                };
                let Value::Ref(target) = record.get(&id) else {
                    continue;
                };
                let reference_def = lookup
                    .tenant
                    .schema
                    .objects
                    .get(&record.object)
                    .and_then(|object| object.fields.get(&id))
                    .and_then(|def| def.ty.ref_target());
                let Some((object, _, false)) = reference_def else {
                    continue;
                };
                let Some(other) = lookup.record(object, *target)? else {
                    continue;
                };
                if !visible_in(lookup, principal, other, now)? {
                    return Ok(Some(format!(
                        "follows {}.{reference} to {object} {target}, which the automation may not read",
                        record.object
                    )));
                }
                if hidden(lookup, principal, roles, now, other, field)? {
                    return Ok(Some(format!(
                        "reads {object}.{field}, which the automation may not read"
                    )));
                }
            }
        }
    }
    Ok(None)
}

/// A detail of at most [`MAX_DETAIL`] characters.
fn detail(text: String) -> String {
    if text.chars().count() <= MAX_DETAIL {
        text
    } else {
        text.chars().take(MAX_DETAIL).collect()
    }
}

/// Decide one trigger whose event matches `event`, in the state after the transition (`lookup`,
/// `schema`): the effect it appends, or `None` (D-7200). In order: a paused trigger appends
/// nothing; a missing automation declines with `automation-removed` and a suspended one with
/// `run-as-suspended`, and a draft with `automation-inactive`, whatever the condition; an event about a record the automation principal
/// may not read appends nothing; a condition that reads what the principal may not read, or that
/// cannot be evaluated, declines with `condition-failed`, and a false one appends nothing; past
/// the hop limit the trigger declines with `hop-limit`; a run key that is null or cannot be
/// evaluated declines with `condition-failed`; otherwise the run starts. A condition or run key
/// that needs a record the snapshot does not hold is `InsufficientState` naming it (SCH-2203).
fn decide(
    lookup: Lookup<'_>,
    schema: &TenantSchema,
    ctx: &Context,
    hop: u32,
    event: &DomainEvent,
    trigger: &TriggerDef,
) -> Result<Option<Effect>, DomainError> {
    if trigger.paused == Some(true) {
        return Ok(None);
    }
    let decline = |reason: AutomationExceptionReason, text: String| {
        Some(Effect::DeclineAutomation(AutomationDecline {
            trigger: trigger.id.clone(),
            automation: trigger.automation.clone(),
            event_seq: event.seq,
            hop,
            reason,
            detail: detail(text),
        }))
    };
    let Some(automation) = schema
        .automations
        .as_ref()
        .and_then(|all| all.get(&trigger.automation))
    else {
        return Ok(decline(
            AutomationExceptionReason::AutomationRemoved,
            format!("automation {} does not exist", trigger.automation),
        ));
    };
    if let Some(role) = &automation.suspended_for {
        return Ok(decline(
            AutomationExceptionReason::RunAsSuspended,
            format!(
                "automation {} is suspended: its definer may not confer role {role}",
                automation.id
            ),
        ));
    }
    if !crate::activation::is_active(automation) {
        return Ok(decline(
            AutomationExceptionReason::AutomationInactive,
            crate::activation::inactive_detail(automation),
        ));
    }
    let principal = Principal::User(automation_principal(&automation.id));
    let roles = roles_of(lookup.tenant, &principal);
    let Some(scopes) = scopes(lookup, &principal, ctx.now, &event.body)? else {
        return Ok(None);
    };
    let params = event_params(event, hop, &trigger.on);
    let env = Env {
        record: scopes.record,
        old: scopes.old,
        new: scopes.new,
        params: &params,
        now: ctx.now,
        principal: &principal,
        roles: &roles,
        lookup: Some(lookup),
    };
    if let Some(condition) = &trigger.condition {
        if let Some(why) = unreadable(lookup, &principal, &roles, ctx.now, condition, &scopes)? {
            return Ok(decline(
                AutomationExceptionReason::ConditionFailed,
                format!("the condition of trigger {} {why}", trigger.id),
            ));
        }
        match expr::holds(condition, &env) {
            Ok(true) => {}
            Ok(false) => return Ok(None),
            Err(_) => {
                if let Some(need) = expr::missing(condition, &env) {
                    return Err(insufficient(need));
                }
                return Ok(decline(
                    AutomationExceptionReason::ConditionFailed,
                    format!(
                        "the condition of trigger {} cannot be evaluated",
                        trigger.id
                    ),
                ));
            }
        }
    }
    if hop > HOP_LIMIT {
        return Ok(decline(
            AutomationExceptionReason::HopLimit,
            format!("hop {hop} is past the hop limit of {HOP_LIMIT}"),
        ));
    }
    let run_key = match &automation.run_key {
        None => None,
        Some(key) => {
            if let Some(why) = unreadable(lookup, &principal, &roles, ctx.now, key, &scopes)? {
                return Ok(decline(
                    AutomationExceptionReason::ConditionFailed,
                    format!("the run key of automation {} {why}", automation.id),
                ));
            }
            match expr::eval(key, &env) {
                Ok(Value::Null) => {
                    return Ok(decline(
                        AutomationExceptionReason::ConditionFailed,
                        format!("the run key of automation {} is null", automation.id),
                    ))
                }
                Ok(value) => Some(canon::hash_of(&value)),
                Err(_) => {
                    if let Some(need) = expr::missing(key, &env) {
                        return Err(insufficient(need));
                    }
                    return Ok(decline(
                        AutomationExceptionReason::ConditionFailed,
                        format!(
                            "the run key of automation {} cannot be evaluated",
                            automation.id
                        ),
                    ));
                }
            }
        }
    };
    Ok(Some(Effect::StartAutomation(AutomationStart {
        trigger: trigger.id.clone(),
        automation: automation.id.clone(),
        event_seq: event.seq,
        hop,
        caused_by: automation_principal(&automation.id),
        on_behalf_of: automation.defined_by.clone(),
        definition_version: schema.version,
        run_key,
        params,
    })))
}

/// Append events to an accepted transition, numbered after its last event, each with a `Publish`
/// effect placed after the `Publish` effects already there, as `Draft::finish` orders them.
fn append_events(result: &mut Transition, ctx: &Context, bodies: Vec<EventBody>) {
    for body in bodies {
        let seq = result.delta.event_seq + 1;
        result.events.push(DomainEvent {
            tenant: result.delta.tenant.clone(),
            seq,
            revision: result.delta.new_revision,
            at: ctx.now,
            principal: ctx.principal.clone(),
            correlation: ctx.correlation.clone(),
            body,
        });
        result.delta.event_seq = seq;
        let at = result
            .effects
            .iter()
            .take_while(|effect| matches!(effect, Effect::Publish { .. }))
            .count();
        result.effects.insert(at, Effect::Publish { seq });
    }
}

/// Suspend, in `after` and in the transition, every automation whose definer lost the authority
/// to confer a role of its run-as (D-7204, WFL-1529).
fn suspend_lost(after: &mut TenantState, ctx: &Context, result: &mut Transition) {
    let lost = lost_authority(after);
    if lost.is_empty() {
        return;
    }
    let mut schema = TenantSchema::clone(&after.schema);
    let mut bodies = Vec::new();
    for loss in lost {
        if let Some(def) = schema
            .automations
            .as_mut()
            .and_then(|all| all.get_mut(&loss.automation))
        {
            def.suspended_for = Some(loss.role.clone());
        }
        result.evidence.push(Check::pass(
            format!("automation:{}:suspended", loss.automation),
            detail(format!(
                "definer {} may no longer confer role {}: {}",
                loss.definer, loss.role, loss.detail
            )),
        ));
        bodies.push(EventBody::AutomationSuspended {
            automation: loss.automation,
            definer: loss.definer,
            role: loss.role,
        });
    }
    let schema = Arc::new(schema);
    result.delta.ops.push(DeltaOp::PutSchema(schema.clone()));
    after.schema = schema;
    append_events(result, ctx, bodies);
}

/// What the kernel adds to an accepted transition once its events are final (D-7200, D-7204):
/// the suspensions of automations whose definer lost authority, then the effect of each trigger
/// that matches an event of the transition. `lookup` is the snapshot the transition was decided
/// on; conditions are decided on the state after the transition, with the snapshot's coverage.
pub fn after_decision(
    lookup: Lookup<'_>,
    ctx: &Context,
    mut result: Transition,
) -> Result<Transition, DomainError> {
    let tenant = lookup.tenant;
    let authority_changed = result
        .delta
        .ops
        .iter()
        .any(|op| matches!(op, DeltaOp::PutSchema(_) | DeltaOp::PutMember { .. }));
    let triggered = tenant
        .schema
        .triggers
        .iter()
        .flat_map(BTreeMap::values)
        .any(|trigger| {
            trigger.paused != Some(true)
                && result
                    .events
                    .iter()
                    .any(|event| matches(&trigger.on, &event.body))
        });
    if !authority_changed && !triggered {
        return Ok(result);
    }
    let mut after = apply_delta(Some(tenant), &result.delta)?;
    if authority_changed {
        suspend_lost(&mut after, ctx, &mut result);
    }
    let schema = after.schema.clone();
    let Some(triggers) = schema.triggers.as_ref() else {
        return Ok(result);
    };
    let state = Lookup {
        tenant: &after,
        coverage: lookup.coverage,
    };
    let hop = ctx.hop.unwrap_or(0);
    let mut effects = Vec::new();
    for event in &result.events {
        if event.tenant != after.id {
            continue;
        }
        for trigger in triggers.values() {
            if !matches(&trigger.on, &event.body) {
                continue;
            }
            if let Some(effect) = decide(state, &schema, ctx, hop, event, trigger)? {
                effects.push(effect);
            }
        }
    }
    result.effects.extend(effects);
    Ok(result)
}
