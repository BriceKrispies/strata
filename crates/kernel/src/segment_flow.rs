//! What the entry and exit events of a segment start, and what restoring a snapshot does to the
//! segments (SEG-014, SEG-031, SEG-032, D-7200, D-7202, D-7280). Everything here is pure.
//!
//! A segment event is not an event of the kernel's sequence (D-7280): it is a row of the segment
//! block's own ordered log. A trigger on `segment-entered` or `segment-exited` is matched against
//! it here, by the same rules as [`crate::triggers`] matches a record event:
//!
//! - the automation principal `automation:<workflow-id>` decides, holding its run-as roles only
//!   (D-6525, D-7201): an event about a person whose subject record it may not read starts
//!   nothing, and so does one about an erased person, whose record is gone;
//! - a paused trigger starts nothing; a removed or suspended automation declines; a condition that
//!   cannot be evaluated declines with `condition-failed`; a false one starts nothing (D-6530);
//! - the hop count of the event is the hop count of the change plus one (SEG-031); past
//!   [`HOP_LIMIT`] the trigger declines with `hop-limit` (SEG-032, D-7202);
//! - the parameters it binds are `seq` (the position in the log), `at`, `hop`, `person`,
//!   `segment` and `reason` (D-6523). No record scope is bound: a condition names `param.person`
//!   and does not read the person's record.

use crate::automation::automation_principal;
use crate::canon;
use crate::expr::{self, Env};
use crate::intent::{Context, DomainError, Effect};
use crate::read::visible_in;
use crate::rules::{insufficient, roles_of, Lookup};
use crate::segments::{
    locate, Batch, BatchChange, BatchScope, Locate, Segment, SegmentDirection, SegmentEvent,
    SegmentMembershipEvent, SegmentReason, SegmentWrite, Target,
};
use crate::state::{Need, Principal, Record};
use crate::triggers::HOP_LIMIT;
use crate::types::{PrincipalId, Timestamp, TriggerDef, TriggerEvent, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use strata_model_core::types::{AutomationDecline, AutomationExceptionReason, AutomationStart};

/// The longest detail a declined exception carries (rec-automation-exception.detail).
const MAX_DETAIL: usize = 256;

/// Whether a trigger's event matches an entry or exit event of a segment (SEG-014): the direction
/// and the segment.
pub fn matches(on: &TriggerEvent, event: &SegmentMembershipEvent) -> bool {
    match (on, event.direction) {
        (TriggerEvent::SegmentEntered { segment }, SegmentDirection::Entered)
        | (TriggerEvent::SegmentExited { segment }, SegmentDirection::Exited) => {
            *segment == event.segment
        }
        _ => false,
    }
}

/// The hop count of a logged event as the trigger decision counts it.
pub fn hop_of(event: &SegmentMembershipEvent) -> u32 {
    u32::try_from(event.hop).unwrap_or(u32::MAX)
}

/// The parameters an event binds for a trigger (D-6523): `seq`, `at`, `hop`, and for a segment
/// event `person`, `segment` and `reason`. `position` is the position of the event in the log.
pub fn event_params(position: u64, logged: &SegmentEvent) -> BTreeMap<String, Value> {
    BTreeMap::from([
        (
            "seq".to_string(),
            Value::Int(i64::try_from(position).unwrap_or(i64::MAX)),
        ),
        ("at".to_string(), Value::Time(logged.at.0)),
        (
            "hop".to_string(),
            Value::Int(i64::from(hop_of(&logged.event))),
        ),
        (
            "person".to_string(),
            Value::Str(logged.event.person.as_str().to_string()),
        ),
        (
            "segment".to_string(),
            Value::Str(logged.event.segment.to_string()),
        ),
        (
            "reason".to_string(),
            Value::Str(logged.event.reason.as_str().to_string()),
        ),
    ])
}

/// What a decision needs loaded to read the subject record of the person of an event.
pub fn subject_need(segment: &Segment, person: &PrincipalId) -> Need {
    Need::ByField {
        object: segment.subject.clone(),
        field: segment.principal_field.clone(),
        value: Value::Str(person.as_str().to_string()),
    }
}

/// The subject record of the person of an event, when the state holds it: `None` for a person
/// whose record is gone, as an erased person's is. A record the snapshot does not cover is
/// `InsufficientState`.
pub fn subject_of<'a>(
    lookup: &Lookup<'a>,
    segment: &Segment,
    person: &PrincipalId,
) -> Result<Option<&'a Arc<Record>>, DomainError> {
    locate(
        segment,
        lookup,
        &Target {
            segment: segment.id,
            person: person.clone(),
            locate: Locate::Principal,
        },
    )
}

/// A detail of at most [`MAX_DETAIL`] characters.
fn detail(text: String) -> String {
    if text.chars().count() <= MAX_DETAIL {
        text
    } else {
        text.chars().take(MAX_DETAIL).collect()
    }
}

/// Decide one trigger whose event matches the logged event at `position`, in the state `lookup`
/// holds (D-7200): the effect it appends, or `None`. `subject` is the subject record of the
/// person of the event ([`subject_of`]). The order is that of the decision for a record event
/// (see [`crate::triggers`]): paused; automation missing or suspended; no subject record, or one
/// the automation principal may not read; the condition; the hop limit; the run key. A record
/// the snapshot does not cover is `InsufficientState` naming it.
pub fn decide(
    lookup: Lookup<'_>,
    ctx: &Context,
    position: u64,
    logged: &SegmentEvent,
    subject: Option<&Record>,
    trigger: &TriggerDef,
) -> Result<Option<Effect>, DomainError> {
    if trigger.paused == Some(true) {
        return Ok(None);
    }
    let schema = &lookup.tenant.schema;
    let hop = hop_of(&logged.event);
    let decline = |reason: AutomationExceptionReason, text: String| {
        Some(Effect::DeclineAutomation(AutomationDecline {
            trigger: trigger.id.clone(),
            automation: trigger.automation.clone(),
            event_seq: position,
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
    let principal = Principal::User(automation_principal(&automation.id));
    let roles = roles_of(lookup.tenant, &principal);
    let Some(subject) = subject else {
        return Ok(None);
    };
    if !visible_in(lookup, &principal, subject, ctx.now)? {
        return Ok(None);
    }
    let params = event_params(position, logged);
    let env = Env {
        record: None,
        old: None,
        new: None,
        params: &params,
        now: ctx.now,
        principal: &principal,
        roles: &roles,
        lookup: Some(lookup),
    };
    if let Some(condition) = &trigger.condition {
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
        Some(key) => match expr::eval(key, &env) {
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
        },
    };
    Ok(Some(Effect::StartAutomation(AutomationStart {
        trigger: trigger.id.clone(),
        automation: automation.id.clone(),
        event_seq: position,
        hop,
        caused_by: automation_principal(&automation.id),
        on_behalf_of: automation.defined_by.clone(),
        definition_version: schema.version,
        run_key,
        params,
    })))
}

/// The segment write that follows the restore of a snapshot (WG-5167): the membership and wake
/// rows of every segment no longer rest on the records that were put back, so each segment is
/// treated as if its predicate had changed: its version is raised, which supersedes every pending
/// continuation (SEG-025), and a continuation over every person starts at the new version
/// (SEG-022), which re-evaluates each person against the restored records. The store clears the
/// membership and wake rows of the segments, which no event accompanies: a restore is not a
/// transition (D-7280). The event log is kept: it is history.
pub fn restore_write(catalog: &[Segment]) -> SegmentWrite {
    let mut write = SegmentWrite::default();
    for segment in catalog {
        let version = segment.version + 1;
        write.expect.insert(segment.id, Some(segment.version));
        write.put_segments.push(Segment {
            version,
            member_count: 0,
            ..segment.clone()
        });
        write.clear_batches.push(segment.id);
        write.batches.push(BatchChange::Put(Batch {
            segment: segment.id,
            scope: BatchScope::All,
            version,
            after: None,
            reason: SegmentReason::PredicateChange,
            hop: 0,
            at: Timestamp(0),
        }));
    }
    write
}
