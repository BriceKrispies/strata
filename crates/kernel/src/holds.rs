//! Held operations (D-8400, D-8406, ADM-1601 to ADM-1604, ADM-1614).
//!
//! A hold rule is tenant metadata ([`TenantSchema::hold_rules`]): it names a target (every
//! invocation of a tenant operation, every grant or revocation of a role, every write of a
//! setting), an optional condition over the change, and the capability its approver needs. A
//! change that a rule holds is not applied when it is submitted. [`transition`] files it as a
//! held operation instead ([`TenantSchema::holds`], kept the way invitations are), and answers
//! `Output::Held`. A caller that holds a change for a reason of its own files one the same way with
//! the intent `FileHold`.
//!
//! The decision on a hold (`DecideHold`) is made here, in this order, and each refusal names its
//! error kind and leaves the hold as it was:
//!
//! 1. the hold exists;
//! 2. the decider is a signed-in person, not an automation or an agent's principal, and holds the
//!    approver capability of the hold (`ApproverPermission`);
//! 3. the decider is neither the requester nor the person the change acts for (`SelfAction`);
//! 4. the hold is still pending at the revision the decision read (`VersionConflict`, D-8406): two
//!    approvers that race read the same revision, and one commit raises it, so the other finds it
//!    moved, whether the store retried the decision on the newer state or the decision was
//!    committed against the revision it read;
//! 5. the hold has not expired, seven days after its filing (`HoldExpired`);
//! 6. a rejection records itself. An approval rereads the state the change read, which must hash as
//!    it did when the change was filed (`StaleHold`), and applies the frozen change as the
//!    requester, with the requester's own authority asked again at that moment (`StaleHold` when
//!    it is refused). The change and the record of the approval are one commit, so an approved
//!    change applies once.
//!
//! The kernel does not read the clock: the decisions compare `ctx.now` with the expiry.
//!
//! A change is never applied unless it is decided here. The decision of the change itself runs
//! through [`crate::transition::transition_pinned`] with the hold rules switched off, so a
//! change applied from a hold is decided by the same code, on the same state, as the direct
//! change it stands for.

use crate::canon::hash_of;
use crate::definitions::{check_expr, expr_nodes};
use crate::evolution::PinBook;
use crate::expr::{self, Env};
use crate::intent::{
    Check, CheckExt, Context, DeltaOp, DomainError, DomainErrorExt, ErrorKind, Intent, Output,
    SchemaChange, StateDelta, Transition,
};
use crate::limits;
use crate::rules::{holds_any, roles_of, Lookup};
use crate::schema::{Grant, TenantSchema};
use crate::state::{Principal, State, TenantState};
use crate::types::{
    CapabilityName, HeldChange, HeldOperation, HeldOperationId, HoldDecision, HoldRule, HoldRuleId,
    HoldState, HoldTarget, PrincipalId, RoleId, Timestamp, Value,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The capability that defines and removes hold rules (D-8400).
pub const HOLDS_DEFINE: &str = "holds-define";

/// How long a hold waits for its decision, in milliseconds: seven days (ADM-1603).
pub const HOLD_LIFETIME_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// How long a decided hold is kept, in milliseconds: ninety days. The audit log keeps what it
/// said; the tenant keeps the record to show an approver what was decided.
pub const DECIDED_RETENTION_MS: i64 = 90 * 24 * 60 * 60 * 1000;

/// The most holds a tenant keeps waiting for a decision.
pub const MAX_PENDING_HOLDS: usize = 1000;

/// The error kind for each way a decision on a hold is refused is the one the requirement names;
/// the rule of the failed check says which check.
fn refuse(
    evidence: &[Check],
    kind: ErrorKind,
    rule: &str,
    message: impl Into<String>,
) -> DomainError {
    let message = message.into();
    let mut checks = evidence.to_vec();
    checks.push(Check::fail(rule, message.clone()));
    DomainError::new(kind, message).with(checks)
}

// ---------------------------------------------------------------------------------------------
// The change a hold freezes
// ---------------------------------------------------------------------------------------------

/// The frozen form of an intent a hold can hold, or `None` for an intent no rule holds.
pub fn change_of(intent: &Intent) -> Option<HeldChange> {
    Some(match intent {
        Intent::InvokeOperation {
            operation,
            inputs,
            idempotency_key,
        } => HeldChange::InvokeOperation {
            operation: operation.clone(),
            inputs: inputs.clone(),
            idempotency_key: idempotency_key.clone(),
        },
        Intent::AssignRole { principal, role } => HeldChange::AssignRole {
            principal: principal.clone(),
            role: role.clone(),
        },
        Intent::RevokeRole { principal, role } => HeldChange::RevokeRole {
            principal: principal.clone(),
            role: role.clone(),
        },
        Intent::AssignTeamRole { team, role } => HeldChange::AssignTeamRole {
            team: team.clone(),
            role: role.clone(),
        },
        Intent::RevokeTeamRole { team, role } => HeldChange::RevokeTeamRole {
            team: team.clone(),
            role: role.clone(),
        },
        Intent::AddTeamMember { team, principal } => HeldChange::AddTeamMember {
            team: team.clone(),
            principal: principal.clone(),
        },
        Intent::RemoveTeamMember { team, principal } => HeldChange::RemoveTeamMember {
            team: team.clone(),
            principal: principal.clone(),
        },
        Intent::CreateInvitation {
            invitation,
            address,
            roles,
        } => HeldChange::CreateInvitation {
            invitation: invitation.clone(),
            address: address.clone(),
            roles: roles.clone(),
        },
        Intent::WriteSetting {
            setting,
            level,
            team,
            value,
            expected_revision,
        } => HeldChange::WriteSetting {
            setting: setting.clone(),
            level: *level,
            team: team.clone(),
            value: value.clone(),
            expected_revision: *expected_revision,
        },
        _ => return None,
    })
}

/// The intent a frozen change stands for.
pub fn intent_of(change: &HeldChange) -> Intent {
    match change {
        HeldChange::InvokeOperation {
            operation,
            inputs,
            idempotency_key,
        } => Intent::InvokeOperation {
            operation: operation.clone(),
            inputs: inputs.clone(),
            idempotency_key: idempotency_key.clone(),
        },
        HeldChange::AssignRole { principal, role } => Intent::AssignRole {
            principal: principal.clone(),
            role: role.clone(),
        },
        HeldChange::RevokeRole { principal, role } => Intent::RevokeRole {
            principal: principal.clone(),
            role: role.clone(),
        },
        HeldChange::AssignTeamRole { team, role } => Intent::AssignTeamRole {
            team: team.clone(),
            role: role.clone(),
        },
        HeldChange::RevokeTeamRole { team, role } => Intent::RevokeTeamRole {
            team: team.clone(),
            role: role.clone(),
        },
        HeldChange::AddTeamMember { team, principal } => Intent::AddTeamMember {
            team: team.clone(),
            principal: principal.clone(),
        },
        HeldChange::RemoveTeamMember { team, principal } => Intent::RemoveTeamMember {
            team: team.clone(),
            principal: principal.clone(),
        },
        HeldChange::CreateInvitation {
            invitation,
            address,
            roles,
        } => Intent::CreateInvitation {
            invitation: invitation.clone(),
            address: address.clone(),
            roles: roles.clone(),
        },
        HeldChange::WriteSetting {
            setting,
            level,
            team,
            value,
            expected_revision,
        } => Intent::WriteSetting {
            setting: setting.clone(),
            level: *level,
            team: team.clone(),
            value: value.clone(),
            expected_revision: *expected_revision,
        },
    }
}

fn text(value: &str) -> Value {
    Value::Str(value.to_string())
}

/// The roles a change grants or takes away, and what it reads as parameters of a rule's
/// condition. A change of a team's membership counts as a change of every role the team holds.
fn params_of(change: &HeldChange) -> BTreeMap<String, Value> {
    let mut params = BTreeMap::new();
    match change {
        HeldChange::InvokeOperation { inputs, .. } => params.extend(inputs.clone()),
        HeldChange::AssignRole { principal, role } | HeldChange::RevokeRole { principal, role } => {
            let action = matches!(change, HeldChange::AssignRole { .. });
            params.insert("principal".into(), text(principal.as_str()));
            params.insert("role".into(), text(role.as_str()));
            params.insert(
                "action".into(),
                text(if action { "assign" } else { "revoke" }),
            );
        }
        HeldChange::AssignTeamRole { team, role } | HeldChange::RevokeTeamRole { team, role } => {
            let action = matches!(change, HeldChange::AssignTeamRole { .. });
            params.insert("team".into(), text(team.as_str()));
            params.insert("role".into(), text(role.as_str()));
            params.insert(
                "action".into(),
                text(if action { "assign" } else { "revoke" }),
            );
        }
        HeldChange::AddTeamMember { team, principal }
        | HeldChange::RemoveTeamMember { team, principal } => {
            let action = matches!(change, HeldChange::AddTeamMember { .. });
            params.insert("team".into(), text(team.as_str()));
            params.insert("principal".into(), text(principal.as_str()));
            params.insert("action".into(), text(if action { "add" } else { "remove" }));
        }
        HeldChange::CreateInvitation {
            invitation,
            address,
            ..
        } => {
            params.insert("invitation".into(), text(invitation.as_str()));
            params.insert("address".into(), text(address));
            params.insert("action".into(), text("invite"));
        }
        HeldChange::WriteSetting {
            setting,
            level,
            team,
            value,
            ..
        } => {
            params.insert("setting".into(), text(setting.as_str()));
            params.insert("level".into(), text(level.as_str()));
            params.insert(
                "team".into(),
                team.as_ref().map_or(Value::Null, |t| text(t.as_str())),
            );
            params.insert("value".into(), value.clone().unwrap_or(Value::Null));
        }
    }
    params
}

/// The roles of the tenant a change confers or takes away: the one it names, the ones an
/// invitation offers, and the ones of the team a person joins or leaves. A rule whose target is
/// one of these roles holds the change.
fn roles_touched(tenant: &TenantState, change: &HeldChange) -> BTreeSet<RoleId> {
    match change {
        HeldChange::AssignRole { role, .. }
        | HeldChange::RevokeRole { role, .. }
        | HeldChange::AssignTeamRole { role, .. }
        | HeldChange::RevokeTeamRole { role, .. } => BTreeSet::from([role.clone()]),
        HeldChange::AddTeamMember { team, .. } | HeldChange::RemoveTeamMember { team, .. } => {
            tenant
                .schema
                .teams
                .as_ref()
                .and_then(|teams| teams.get(team))
                .map(|def| def.roles.clone())
                .unwrap_or_default()
        }
        HeldChange::CreateInvitation { roles, .. } => roles.clone(),
        HeldChange::InvokeOperation { .. } | HeldChange::WriteSetting { .. } => BTreeSet::new(),
    }
}

/// Whether the target of a rule names the change.
fn names(target: &HoldTarget, tenant: &TenantState, change: &HeldChange) -> bool {
    match (target, change) {
        (
            HoldTarget::Operation { operation },
            HeldChange::InvokeOperation { operation: of, .. },
        ) => operation == of,
        (HoldTarget::Setting { setting }, HeldChange::WriteSetting { setting: of, .. }) => {
            setting == of
        }
        (HoldTarget::Role { role }, change) => roles_touched(tenant, change).contains(role),
        _ => false,
    }
}

/// The rule that holds `change` for the principal of `ctx`, if one does: the first by rule id
/// whose target names the change and whose condition is not false. A condition that is null or
/// cannot be evaluated holds the change: a hold fails closed.
pub fn rule_holding<'a>(
    tenant: &'a TenantState,
    ctx: &Context,
    change: &HeldChange,
) -> Option<&'a HoldRule> {
    // The platform's own acts are not held: nobody could approve them.
    if matches!(ctx.principal, Principal::System) {
        return None;
    }
    let rules = tenant.schema.hold_rules.as_ref()?;
    let params = params_of(change);
    let roles = roles_of(tenant, &ctx.principal);
    rules.values().find(|rule| {
        names(&rule.target, tenant, change)
            && match &rule.when {
                None => true,
                Some(when) => {
                    let env = Env {
                        record: None,
                        old: None,
                        new: None,
                        params: &params,
                        now: ctx.now,
                        principal: &ctx.principal,
                        roles: &roles,
                        lookup: None,
                    };
                    !matches!(expr::eval(when, &env), Ok(Value::Bool(false)))
                }
            }
    })
}

// ---------------------------------------------------------------------------------------------
// The state a change read
// ---------------------------------------------------------------------------------------------

/// The state `change` reads, as the hash of the hold records it: what the roles of a person or a
/// team are, what the operation is and the version of the record it acts on, and nothing the
/// change itself checks (a setting write carries the revision it read in its own fields, and the
/// store refuses it when that has moved, which is a stale hold when it is applied).
fn witness(lookup: Lookup<'_>, change: &HeldChange) -> Result<serde_json::Value, DomainError> {
    let tenant = lookup.tenant;
    Ok(match change {
        HeldChange::AssignRole { principal, .. } | HeldChange::RevokeRole { principal, .. } => {
            json!({ "roles": tenant.members.get(principal) })
        }
        HeldChange::AssignTeamRole { team, .. }
        | HeldChange::RevokeTeamRole { team, .. }
        | HeldChange::AddTeamMember { team, .. }
        | HeldChange::RemoveTeamMember { team, .. } => {
            json!({ "team": tenant.schema.teams.as_ref().and_then(|t| t.get(team)) })
        }
        HeldChange::CreateInvitation { invitation, .. } => {
            json!({ "taken": tenant.schema.invitations.as_ref().is_some_and(|i| i.contains_key(invitation)) })
        }
        HeldChange::WriteSetting { .. } => json!({}),
        HeldChange::InvokeOperation {
            operation, inputs, ..
        } => {
            let op = tenant
                .schema
                .operations
                .as_ref()
                .and_then(|ops| ops.get(operation));
            let subject = match op.and_then(|op| op.subject.as_ref()) {
                Some(subject) => match inputs.get(&subject.input) {
                    Some(Value::Ref(id)) => lookup
                        .record(&subject.object, *id)?
                        .map(|record| record.version),
                    _ => None,
                },
                None => None,
            };
            json!({ "operation": op, "subject": subject })
        }
    })
}

/// The hash a hold records for `change`, with the state it reads.
fn input_hash(lookup: Lookup<'_>, change: &HeldChange) -> Result<String, DomainError> {
    Ok(hash_of(&json!({
        "change": change,
        "read": witness(lookup, change)?,
    })))
}

// ---------------------------------------------------------------------------------------------
// Filing
// ---------------------------------------------------------------------------------------------

/// The number a hold id `h-<n>` carries.
fn number_of(id: &HeldOperationId) -> u64 {
    id.as_str()
        .strip_prefix("h-")
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// A transition of the schema alone: one revision on, the schema replaced.
fn schema_transition(
    tenant: &TenantState,
    schema: TenantSchema,
    evidence: Vec<Check>,
    output: Output,
) -> Transition {
    Transition {
        delta: StateDelta {
            tenant: tenant.id.clone(),
            base_revision: Some(tenant.revision),
            new_revision: crate::types::StateRevision(tenant.revision.0 + 1),
            next_record: tenant.next_record,
            event_seq: tenant.event_seq,
            ops: vec![DeltaOp::PutSchema(Arc::new(schema))],
        },
        events: Vec::new(),
        effects: Vec::new(),
        evidence,
        output,
    }
}

/// File `change` as a held operation (D-8400, ADM-1601). The change must be one the requester
/// would be allowed to make now: it is decided as it would be directly, and refused as it would
/// be if it fails, so that nothing that could never apply is ever waiting. Nothing the change
/// does is kept from that decision.
pub(crate) fn file(
    state: &State,
    tenant: &TenantState,
    ctx: &Context,
    pins: &PinBook,
    change: HeldChange,
    approver: CapabilityName,
    rule: Option<HoldRuleId>,
    on_behalf_of: Option<PrincipalId>,
) -> Result<Transition, DomainError> {
    let mut evidence: Vec<Check> = Vec::new();
    let Principal::User(requester) = &ctx.principal else {
        return Err(refuse(
            &evidence,
            ErrorKind::InvalidIntent,
            "hold:requester_is_a_person",
            "only a signed-in person files a change for approval",
        ));
    };
    evidence.push(Check::pass(
        "hold:requester_is_a_person",
        requester.as_str(),
    ));
    if rule.is_none() {
        if let Some(problem) = approver_problem(&tenant.schema, approver.as_str()) {
            return Err(refuse(
                &evidence,
                ErrorKind::InvalidIntent,
                "hold:approver_is_declared",
                problem,
            ));
        }
    }
    let pending = tenant
        .schema
        .holds
        .iter()
        .flatten()
        .filter(|(_, hold)| hold.state == HoldState::Pending)
        .count();
    if pending >= MAX_PENDING_HOLDS {
        return Err(refuse(
            &evidence,
            ErrorKind::LimitExceeded,
            "hold:limit",
            format!("{pending} holds wait for a decision, the most a tenant keeps"),
        ));
    }
    // The change as the requester would make it: refused as it would be refused, unheld.
    let intent = intent_of(&change);
    let direct = crate::transition::transition_unheld(state, &intent, ctx, pins)?;
    evidence.extend(direct.evidence);
    let lookup = Lookup {
        tenant,
        coverage: &state.coverage,
    };
    let hash = input_hash(lookup, &change)?;
    let mut schema = TenantSchema::clone(&tenant.schema);
    let holds = schema.holds.get_or_insert_with(BTreeMap::new);
    // Decided holds past their retention go; the newest hold stays whatever it is, so that the
    // next number is never one that was used.
    let newest = holds.keys().map(number_of).max().unwrap_or(0);
    holds.retain(|id, hold| {
        number_of(id) == newest
            || hold.state == HoldState::Pending
            || hold
                .decided_at
                .is_none_or(|at| ctx.now.0.saturating_sub(at.0) < DECIDED_RETENTION_MS)
    });
    let id = HeldOperationId::new(format!("h-{}", newest + 1)).expect("h-<n> is a hold id");
    let expires_at = Timestamp(ctx.now.0.saturating_add(HOLD_LIFETIME_MS));
    evidence.push(Check::pass(
        "hold:filed",
        format!(
            "{id}: {} files {}, approver {approver}, expires at {}",
            requester.as_str(),
            intent_name(&change),
            expires_at.0
        ),
    ));
    holds.insert(
        id.clone(),
        HeldOperation {
            id: id.clone(),
            rule,
            approver: approver.clone(),
            change,
            requester: requester.clone(),
            on_behalf_of,
            filed_at: ctx.now,
            expires_at,
            input_hash: hash,
            revision: 1,
            state: HoldState::Pending,
            decided_by: None,
            decided_at: None,
            reason: None,
        },
    );
    Ok(schema_transition(
        tenant,
        schema,
        evidence,
        Output::Held {
            hold: id,
            approver,
            expires_at,
        },
    ))
}

/// Whether the held change is an invocation of a tenant operation that posts a ledger transaction.
fn holds_ledger_transaction(tenant: &TenantState, change: &HeldChange) -> bool {
    let HeldChange::InvokeOperation { operation, .. } = change else {
        return false;
    };
    tenant
        .schema
        .operations
        .as_ref()
        .and_then(|operations| operations.get(operation))
        .is_some_and(crate::definitions::posts_ledger_transaction)
}

fn intent_name(change: &HeldChange) -> &'static str {
    match change {
        HeldChange::InvokeOperation { .. } => "invoke-operation",
        HeldChange::AssignRole { .. } => "assign-role",
        HeldChange::RevokeRole { .. } => "revoke-role",
        HeldChange::AssignTeamRole { .. } => "assign-team-role",
        HeldChange::RevokeTeamRole { .. } => "revoke-team-role",
        HeldChange::AddTeamMember { .. } => "add-team-member",
        HeldChange::RemoveTeamMember { .. } => "remove-team-member",
        HeldChange::CreateInvitation { .. } => "create-invitation",
        HeldChange::WriteSetting { .. } => "write-setting",
    }
}

// ---------------------------------------------------------------------------------------------
// Deciding
// ---------------------------------------------------------------------------------------------

/// Decide `hold` (D-8400, D-8406, ADM-1602 to ADM-1604, ADM-1614).
#[allow(clippy::too_many_arguments)]
pub(crate) fn decide(
    state: &State,
    tenant: &TenantState,
    ctx: &Context,
    pins: &PinBook,
    hold: &HeldOperationId,
    decision: HoldDecision,
    read_revision: u64,
    reason: Option<&String>,
) -> Result<Transition, DomainError> {
    let mut evidence: Vec<Check> = Vec::new();
    let Some(held) = tenant.schema.holds.as_ref().and_then(|h| h.get(hold)) else {
        return Err(refuse(
            &evidence,
            ErrorKind::UnknownRecord,
            "hold:exists",
            format!("no held operation {hold}"),
        ));
    };
    // 2. A signed-in person, who is not an automation, holding the approver permission.
    let decider = match &ctx.principal {
        Principal::User(who) if crate::automation::automation_named(who).is_none() => who,
        _ => {
            return Err(refuse(
                &evidence,
                ErrorKind::ApproverPermission,
                "hold:decider_is_a_person",
                "only a signed-in person decides a held operation",
            ))
        }
    };
    evidence.push(Check::pass("hold:decider_is_a_person", decider.as_str()));
    let wanted = BTreeSet::from([Grant::Capability(held.approver.to_string())]);
    if !holds_any(tenant, &ctx.principal, &wanted) {
        return Err(refuse(
            &evidence,
            ErrorKind::ApproverPermission,
            "hold:approver_permission",
            format!("deciding {hold} needs the permission {}", held.approver),
        ));
    }
    evidence.push(Check::pass(
        "hold:approver_permission",
        held.approver.to_string(),
    ));
    // 3a. A held posting of a ledger transaction is not approved by its actor or by the person it
    // was made for (LDG-2907): the rule is the ledger's, stated on the header of the transaction,
    // so that no operation can skip it.
    if holds_ledger_transaction(tenant, &held.change) {
        if let Some(mut refusal) = crate::ledger_transaction::approval_refusal(
            &ctx.principal,
            &held.requester,
            held.on_behalf_of.as_ref(),
        ) {
            let mut checks = evidence.clone();
            checks.append(&mut refusal.evidence);
            refusal.evidence = checks;
            return Err(refusal);
        }
        evidence.push(Check::pass(
            "ledger:self-approval",
            "the approver is neither the actor of the held transaction nor the person it was made for",
        ));
    }
    // 3. Not the requester, and not the person the change acts for.
    if *decider == held.requester || held.on_behalf_of.as_ref() == Some(decider) {
        return Err(refuse(
            &evidence,
            ErrorKind::SelfAction,
            "hold:not_the_requester",
            format!("{decider} filed {hold} or is the person it acts for"),
        ));
    }
    evidence.push(Check::pass("hold:not_the_requester", decider.as_str()));
    // 4. Still pending at the revision the decision read.
    if held.state != HoldState::Pending || held.revision != read_revision {
        return Err(refuse(
            &evidence,
            ErrorKind::VersionConflict,
            "hold:revision",
            format!(
                "{hold} is {:?} at revision {}, not pending at revision {read_revision}",
                held.state, held.revision
            ),
        ));
    }
    evidence.push(Check::pass("hold:revision", read_revision.to_string()));
    // 5. Not expired.
    if ctx.now > held.expires_at {
        return Err(refuse(
            &evidence,
            ErrorKind::HoldExpired,
            "hold:expiry",
            format!("{hold} expired at {}", held.expires_at.0),
        ));
    }
    evidence.push(Check::pass("hold:expiry", held.expires_at.0.to_string()));

    let decided = |state: HoldState| HeldOperation {
        revision: held.revision + 1,
        state,
        decided_by: Some(decider.clone()),
        decided_at: Some(ctx.now),
        reason: reason.cloned(),
        ..held.clone()
    };
    let with_hold = |schema: &TenantSchema, after: HeldOperation| {
        let mut schema = TenantSchema::clone(schema);
        schema
            .holds
            .get_or_insert_with(BTreeMap::new)
            .insert(after.id.clone(), after);
        schema
    };

    // 6. A rejection records itself.
    if decision == HoldDecision::Reject {
        let after = decided(HoldState::Rejected);
        let revision = after.revision;
        let schema = with_hold(&tenant.schema, after);
        return Ok(schema_transition(
            tenant,
            schema,
            evidence,
            Output::HoldDecided {
                hold: hold.clone(),
                state: HoldState::Rejected,
                revision,
            },
        ));
    }

    // An approval: the state the change read is the state it will change.
    let lookup = Lookup {
        tenant,
        coverage: &state.coverage,
    };
    if input_hash(lookup, &held.change)? != held.input_hash {
        return Err(refuse(
            &evidence,
            ErrorKind::StaleHold,
            "hold:input_hash",
            format!("the state {hold} read has changed since it was filed"),
        ));
    }
    evidence.push(Check::pass("hold:input_hash", held.input_hash.clone()));
    // And applied as the requester, whose authority is asked again now.
    let mut as_requester = ctx.clone();
    as_requester.principal = Principal::User(held.requester.clone());
    as_requester.session = None;
    let intent = intent_of(&held.change);
    let mut applied =
        match crate::transition::transition_unheld(state, &intent, &as_requester, pins) {
            Ok(applied) => applied,
            Err(error) if error.kind == ErrorKind::InsufficientState => return Err(error),
            Err(error) => {
                let mut checks = evidence;
                checks.extend(error.evidence);
                checks.push(Check::fail(
                    "hold:requester_authority",
                    format!(
                        "the change no longer applies as {}: {}",
                        held.requester, error.message
                    ),
                ));
                return Err(DomainError::new(
                    ErrorKind::StaleHold,
                    format!("{hold} no longer applies: {}", error.message),
                )
                .with(checks));
            }
        };
    let after = decided(HoldState::Applied);
    let revision = after.revision;
    // The record of the approval joins the schema the change leaves, if the change left one.
    let at = applied
        .delta
        .ops
        .iter()
        .rposition(|op| matches!(op, DeltaOp::PutSchema(_)));
    match at {
        Some(at) => {
            if let DeltaOp::PutSchema(schema) = &applied.delta.ops[at] {
                let schema = with_hold(schema, after);
                applied.delta.ops[at] = DeltaOp::PutSchema(Arc::new(schema));
            }
        }
        None => applied
            .delta
            .ops
            .push(DeltaOp::PutSchema(Arc::new(with_hold(
                &tenant.schema,
                after,
            )))),
    }
    evidence.extend(applied.evidence);
    applied.evidence = evidence;
    applied.output = Output::HoldDecided {
        hold: hold.clone(),
        state: HoldState::Applied,
        revision,
    };
    Ok(applied)
}

/// What an intent that `decide` or `file` runs needs loaded, besides what the hold's own change
/// names: the records of the change it applies.
pub fn needs_of(schema: &TenantSchema, hold: &HeldOperationId) -> Option<Intent> {
    schema
        .holds
        .as_ref()
        .and_then(|holds| holds.get(hold))
        .map(|held| intent_of(&held.change))
}

// ---------------------------------------------------------------------------------------------
// Definition
// ---------------------------------------------------------------------------------------------

/// Whether a schema change defines or removes a hold rule.
pub fn is_hold_change(change: &SchemaChange) -> bool {
    matches!(
        change,
        SchemaChange::DefineHoldRule(_) | SchemaChange::RemoveHoldRule(_)
    )
}

/// Define (or replace) or remove a hold rule on a working copy of the schema, and describe it.
pub(crate) fn apply(schema: &mut TenantSchema, change: &SchemaChange) -> Result<String, String> {
    match change {
        SchemaChange::DefineHoldRule(rule) => {
            schema
                .hold_rules
                .get_or_insert_with(BTreeMap::new)
                .insert(rule.id.clone(), rule.clone());
            Ok(format!("define hold rule {}", rule.id))
        }
        SchemaChange::RemoveHoldRule(id) => {
            let rules = schema
                .hold_rules
                .as_mut()
                .ok_or_else(|| format!("no hold rule {id}"))?;
            rules
                .remove(id)
                .ok_or_else(|| format!("no hold rule {id}"))?;
            if rules.is_empty() {
                schema.hold_rules = None;
            }
            Ok(format!("remove hold rule {id}"))
        }
        _ => Err("not a change of a hold rule".to_string()),
    }
}

/// The reason a capability cannot be the approver of a hold: a capability of the product form
/// that nobody declared. A capability of the platform form is the platform's.
fn approver_problem(schema: &TenantSchema, approver: &str) -> Option<String> {
    if approver.trim().is_empty() {
        return Some("a hold names the capability its approver needs".to_string());
    }
    if crate::capabilities::is_product_form(approver)
        && !crate::capabilities::is_declared(schema, approver)
    {
        return Some(format!(
            "capability {approver} is neither a platform capability nor declared in the tenant"
        ));
    }
    None
}

/// The parameters a rule's condition may name, by the kind of its target.
fn condition_params(schema: &TenantSchema, target: &HoldTarget) -> BTreeSet<String> {
    let names: Vec<String> = match target {
        HoldTarget::Operation { operation } => schema
            .operations
            .as_ref()
            .and_then(|ops| ops.get(operation))
            .map(|op| op.inputs.iter().map(|input| input.name.clone()).collect())
            .unwrap_or_default(),
        HoldTarget::Role { .. } => [
            "principal",
            "team",
            "role",
            "action",
            "invitation",
            "address",
        ]
        .map(String::from)
        .to_vec(),
        HoldTarget::Setting { .. } => ["setting", "level", "team", "value"]
            .map(String::from)
            .to_vec(),
    };
    names.into_iter().collect()
}

/// The problems of the hold rules a schema declares (D-8400): the key is the id, the label is
/// there, the target exists, the condition checks as a truth value over the parameters of its
/// target, and the approver is a capability that exists.
pub fn problems(schema: &TenantSchema, problems: &mut Vec<String>) {
    let Some(rules) = &schema.hold_rules else {
        return;
    };
    if rules.len() > limits::MAX_HOLD_RULES {
        problems.push(format!(
            "limit: {} hold rules is more than the maximum of {}",
            rules.len(),
            limits::MAX_HOLD_RULES
        ));
    }
    let roles: BTreeSet<RoleId> = schema.roles.keys().cloned().collect();
    let fields = BTreeMap::new();
    for (key, rule) in rules {
        let at = format!("hold rule {key}");
        if *key != rule.id {
            problems.push(format!("{at}: the key is not the id {}", rule.id));
        }
        if rule.label.trim().is_empty() {
            problems.push(format!("{at}: a hold rule needs a label"));
        }
        let exists = match &rule.target {
            HoldTarget::Operation { operation } => schema
                .operations
                .as_ref()
                .is_some_and(|ops| ops.contains_key(operation)),
            HoldTarget::Role { role } => schema.roles.contains_key(role),
            HoldTarget::Setting { setting } => {
                crate::settings::definition(schema, setting).is_some()
            }
        };
        if !exists {
            problems.push(format!("{at}: its target {:?} does not exist", rule.target));
        }
        if let Some(problem) = approver_problem(schema, rule.approver.as_str()) {
            problems.push(format!("{at}: {problem}"));
        }
        if let Some(when) = &rule.when {
            let nodes = expr_nodes(when);
            if nodes > limits::MAX_CONDITION_NODES {
                problems.push(format!(
                    "{at}: its condition has {nodes} nodes, more than the maximum of {}",
                    limits::MAX_CONDITION_NODES
                ));
            }
            let params = condition_params(schema, &rule.target);
            let env = expr::TypeEnv {
                fields: &fields,
                params: &params,
                roles: &roles,
                objects: &schema.objects,
            };
            check_expr(problems, &at, "condition", when, &env, true);
        }
    }
}

/// The holds a principal may decide now: pending, not expired, whose approver capability the
/// principal holds, and that the principal neither filed nor is the person acted for. The list an
/// approvals screen reads.
pub fn pending_for<'a>(
    tenant: &'a TenantState,
    principal: &PrincipalId,
    now: Timestamp,
) -> Vec<&'a HeldOperation> {
    let who = Principal::User(principal.clone());
    tenant
        .schema
        .holds
        .iter()
        .flatten()
        .map(|(_, hold)| hold)
        .filter(|hold| hold.state == HoldState::Pending && now <= hold.expires_at)
        .filter(|hold| {
            holds_any(
                tenant,
                &who,
                &BTreeSet::from([Grant::Capability(hold.approver.to_string())]),
            ) && hold.requester != *principal
                && hold.on_behalf_of.as_ref() != Some(principal)
        })
        .collect()
}
