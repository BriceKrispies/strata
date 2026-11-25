//! Subscriptions as data, and the pure decision of what a subscriber is told about an event.

use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use strata_kernel::rules::Lookup;
use strata_kernel::segment_flow;
use strata_kernel::segments::{self as seg, Segment, SegmentDirection, SegmentEvent, SegmentId};
use strata_kernel::{
    owner_role, visible, DomainEvent, EventBody, LedgerAccountId, LedgerScope, LedgerUnit,
    ObjectTypeId, Principal, PrincipalId, RecordId, SettingId, SettingLevel, State, TeamId,
    TenantState,
};
use strata_model_core::types::EventKind;

/// What a subscriber wants to hear about: the specification's type `selector` (SCH-1575), as
/// specc generates it. Every member is optional and an absent member matches everything; members
/// combine as AND, the values of one member as OR. A selector narrows what a subscriber is told
/// and never widens it: what remains is still decided by the kernel's read decision for that
/// subscriber. Build one from a request with [`SelectorExt::new`].
///
/// `segments` selects the entry and exit events of segments (SEG-033, D-7280). They are not
/// events of the kernel's sequence, so a selector that names segments hears only those, which are
/// told by [`segment_notice`], and a selector that does not name them never hears one. `units`
/// limits the ledger events to postings of those units.
pub use strata_model_core::types::Selector;

/// The names of the variants of the generated `event-body` union, as serde knows them, written the
/// way the specification writes an event kind (`RecordUpdated` is `record-updated`).
///
/// They are read from the generated type itself: deserializing a body under a name no variant has
/// makes serde list every variant it knows. A variant added to the specification therefore has a
/// kind here with nothing to remember.
pub fn emitted_kinds() -> Vec<String> {
    {
        let refusal = serde_json::from_value::<EventBody>(serde_json::json!({ "?": null }))
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        // "unknown variant `?`, expected one of `A`, `B`": the names are the odd pieces between
        // backticks, after the first, which is the one that was asked for.
        refusal
            .split('`')
            .skip(1)
            .step_by(2)
            .skip(1)
            .map(kebab)
            .collect()
    }
}

/// `RecordUpdated` as `record-updated`.
fn kebab(ident: &str) -> String {
    let mut out = String::with_capacity(ident.len() + 4);
    for (i, c) in ident.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The kind of an event: the name of its body's variant in the specification (`event-kind`).
///
/// The name is the variant's own, taken from the first thing its derived `Debug` writes, so no
/// match arm names it and a new variant has a kind at once.
pub fn kind_of(body: &EventBody) -> String {
    struct FirstWrite(String);
    impl std::fmt::Write for FirstWrite {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            self.0.push_str(s);
            // Stop: the rest of the body is its contents.
            Err(std::fmt::Error)
        }
    }
    let mut first = FirstWrite(String::new());
    let _ = write!(first, "{body:?}");
    kebab(&first.0)
}

/// Selectors from the members of a subscription request, and the one that matches every event.
/// An extension trait because the type is generated.
pub trait SelectorExt {
    /// The selector that matches every event.
    fn all() -> Selector;

    /// A selector from the members of a subscription request, or the reason it is refused: a name
    /// that is not valid, a kind the kernel does not emit, a segment id that is not one, or a
    /// member that names nothing.
    fn from_request(
        objects: Option<Vec<String>>,
        kinds: Option<Vec<String>>,
        segments: Option<Vec<String>>,
        units: Option<Vec<String>>,
        transitions: Option<Vec<String>>,
    ) -> Result<Selector, String>;

    /// Whether the selector lets this event through at all, before visibility is asked. Events
    /// that name no record (schema and membership changes) are not limited by `objects`.
    fn admits(&self, event: &DomainEvent) -> bool;
}

/// A member's values, in order and without repeats, so that equal requests give equal selectors.
fn tidy<T: Ord>(mut values: Vec<T>) -> Vec<T> {
    values.sort();
    values.dedup();
    values
}

impl SelectorExt for Selector {
    fn all() -> Selector {
        Selector {
            objects: None,
            kinds: None,
            segments: None,
            units: None,
            transitions: None,
        }
    }

    fn from_request(
        objects: Option<Vec<String>>,
        kinds: Option<Vec<String>>,
        segments: Option<Vec<String>>,
        units: Option<Vec<String>>,
        transitions: Option<Vec<String>>,
    ) -> Result<Selector, String> {
        let segments = segments
            .map(|names| {
                if names.is_empty() {
                    return Err("segments names no segment".to_string());
                }
                names
                    .into_iter()
                    .map(|name| {
                        serde_json::from_value::<SegmentId>(serde_json::Value::String(name.clone()))
                            .map_err(|_| format!("{name:?} is not a segment id"))
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(tidy)
            })
            .transpose()?;
        let units = units
            .map(|names| {
                if names.is_empty() {
                    return Err("units names no unit, so it would match nothing".to_string());
                }
                names
                    .into_iter()
                    .map(LedgerUnit::new)
                    .collect::<Result<Vec<_>, _>>()
                    .map(tidy)
                    .map_err(|e| e.to_string())
            })
            .transpose()?;
        let objects = objects
            .map(|names| {
                names
                    .into_iter()
                    .map(ObjectTypeId::new)
                    .collect::<Result<Vec<_>, _>>()
                    .map(tidy)
                    .map_err(|e| e.to_string())
            })
            .transpose()?;
        let kinds = kinds
            .map(|names| {
                names
                    .into_iter()
                    .map(|name| {
                        let kind = EventKind::new(name.as_str())
                            .map_err(|e| format!("{name:?} is not an event kind: {e}"))?;
                        if name.starts_with("app.") {
                            // The specification lets a tenant declare kinds under `app.`
                            // (declare-event-kind, emit-event; SCH-1576); the kernel has neither.
                            Err(format!(
                                "the event kind {name:?} is declared by the tenant, and the kernel \
                                 has no declare-event-kind or emit-event, so no tenant kind is \
                                 emitted"
                            ))
                        } else if emitted_kinds().contains(&name) {
                            Ok(kind)
                        } else {
                            Err(format!(
                                "the event kind {name:?} is not emitted; the kinds are {}",
                                emitted_kinds().join(", ")
                            ))
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(tidy)
            })
            .transpose()?;
        let transitions = transitions
            .map(|names| {
                names
                    .into_iter()
                    .map(|name| {
                        if name.is_empty() || name.chars().count() > 96 {
                            Err("a transition name is one to 96 characters".to_string())
                        } else {
                            Ok(name)
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(tidy)
            })
            .transpose()?;
        Ok(Selector {
            objects,
            kinds,
            segments,
            units,
            transitions,
        })
    }

    fn admits(&self, event: &DomainEvent) -> bool {
        // A selector that names segments selects their events and nothing of the kernel's.
        if self.segments.is_some() {
            return false;
        }
        if let Some(kinds) = &self.kinds {
            let kind = kind_of(&event.body);
            if !kinds.iter().any(|wanted| wanted.0 == kind) {
                return false;
            }
        }
        if let Some(wanted) = &self.transitions {
            let EventBody::RecordUpdated { transitions, .. } = &event.body else {
                return false;
            };
            let mut fired = transitions
                .iter()
                .flat_map(|by_workflow| by_workflow.values());
            if !fired.any(|name| wanted.contains(name)) {
                return false;
            }
        }
        if let Some(wanted) = &self.units {
            // Only a posting names a unit; a unit selector hears no other kind of event.
            let EventBody::LedgerEntryPosted { unit, .. } = &event.body else {
                return false;
            };
            if !wanted.contains(unit) {
                return false;
            }
        }
        true
    }
}

/// What a subscriber is told. It never carries record contents: the subscriber reads again
/// through the ordinary authorized path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Notice {
    /// A record the subscriber can read was created or changed.
    Upsert {
        object: ObjectTypeId,
        id: RecordId,
        version: u64,
    },
    /// A record the subscriber could read is gone, or is no longer readable by them.
    Remove { object: ObjectTypeId, id: RecordId },
    /// The tenant's schema changed.
    Schema { version: u64 },
    /// The subscriber's own roles changed.
    Membership,
    /// A setting changed at a level that reaches the subscriber (SET-041, SET-043): the subscriber
    /// reads it again through the ordinary path. The notice carries no value (SET-1543).
    Setting {
        setting: SettingId,
        level: SettingLevel,
    },
    /// An entry was posted to a ledger account the subscriber may read (D-6550, WG-5172). It names
    /// the account by its derived id, the unit, the scope and the number of the entry, and never
    /// the owner, the amount or the balance: the subscriber reads again through the ordinary path.
    Ledger {
        account: LedgerAccountId,
        unit: LedgerUnit,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<LedgerScope>,
        sequence: u64,
    },
    /// A person entered or left a segment whose members the subscriber may see, and the subscriber
    /// may read the person's subject record (SEG-033): the subscriber reads the members again
    /// through the ordinary path. `position` is the position of the event in the segment event
    /// log, which numbers its events apart from the kernel's.
    Segment {
        segment: SegmentId,
        person: PrincipalId,
        direction: SegmentDirection,
        position: u64,
    },
}

/// Decide what, if anything, a subscriber learns from an entry or exit event of a segment
/// (D-7280). `state` holds the tenant with the subject record of the person of the event, and
/// `segment` is the segment the event is of. The subscriber must have named the segment, hold
/// the right to read its members (`segments-read`, and every object its predicate reaches in
/// full, [`seg::may_read_members`]), and be able to read the subject record of the person
/// ([`seg::member_visible`]): a person the subscriber could not be shown in a list of members is
/// not announced either. An event about an erased person, whose record is gone, tells nobody.
pub fn segment_notice(
    state: &State,
    principal: &Principal,
    selector: &Selector,
    segment: &Segment,
    position: u64,
    logged: &SegmentEvent,
) -> Option<Notice> {
    let tenant = state.tenant.as_deref()?;
    let event = &logged.event;
    if logged.tenant != tenant.id || segment.id != event.segment {
        return None;
    }
    if !selector.segments.as_ref()?.contains(&event.segment) {
        return None;
    }
    if selector.kinds.is_some() || selector.transitions.is_some() {
        return None;
    }
    if let Some(objects) = &selector.objects {
        if !objects.contains(&segment.subject) {
            return None;
        }
    }
    seg::may_read_members(tenant, principal, segment).ok()?;
    let lookup = Lookup {
        tenant,
        coverage: &state.coverage,
    };
    let record = segment_flow::subject_of(&lookup, segment, &event.person).ok()??;
    seg::member_visible(lookup, principal, segment, record, logged.at)
        .ok()?
        .then(|| Notice::Segment {
            segment: event.segment,
            person: event.person.clone(),
            direction: event.direction,
            position,
        })
}

/// Decide what, if anything, a subscriber learns from an event.
///
/// `tenant` is the state after the event's transition (records are not needed, only schema and
/// members). Visibility is the kernel's read decision, evaluated on the record before and after
/// the change, so a draft that was never visible to a reader produces no notice at all.
pub fn notice(
    tenant: &TenantState,
    principal: &Principal,
    selector: &Selector,
    event: &DomainEvent,
) -> Option<Notice> {
    if event.tenant != tenant.id {
        return None;
    }
    if !selector.admits(event) {
        return None;
    }
    let wants = |object: &ObjectTypeId| {
        selector
            .objects
            .as_ref()
            .is_none_or(|set| set.contains(object))
    };
    let sees = |record| visible(tenant, principal, record, event.at);
    match &event.body {
        EventBody::TenantCreated { .. } => None,
        EventBody::SchemaChanged { version, .. } => Some(Notice::Schema { version: version.0 }),
        EventBody::MemberChanged {
            principal: changed, ..
        } => {
            let is_owner = matches!(principal, Principal::User(id)
                if tenant.members.get(id).is_some_and(|roles| roles.contains(&owner_role())));
            let is_subject = matches!(principal, Principal::User(id) if id == changed);
            (is_owner || is_subject).then_some(Notice::Membership)
        }
        EventBody::RecordCreated { record } => {
            (wants(&record.object) && sees(record)).then(|| Notice::Upsert {
                object: record.object.clone(),
                id: record.id,
                version: record.version,
            })
        }
        EventBody::RecordUpdated { before, after, .. } => {
            if !wants(&after.object) {
                None
            } else if sees(after) {
                Some(Notice::Upsert {
                    object: after.object.clone(),
                    id: after.id,
                    version: after.version,
                })
            } else if sees(before) {
                Some(Notice::Remove {
                    object: after.object.clone(),
                    id: after.id,
                })
            } else {
                None
            }
        }
        EventBody::RecordDeleted { before } => {
            (wants(&before.object) && sees(before)).then(|| Notice::Remove {
                object: before.object.clone(),
                id: before.id,
            })
        }
        EventBody::SettingChanged {
            setting,
            level,
            subject,
        } => {
            // A change at the tenant level reaches every open page; a change at the team level
            // those of the team's members; a change at the person level only those of that
            // person (SET-043).
            let reaches = match level {
                SettingLevel::Platform | SettingLevel::Tenant => true,
                SettingLevel::Team => match principal {
                    Principal::User(id) => TeamId::new(subject.as_str())
                        .ok()
                        .and_then(|team| {
                            use strata_kernel::ext::TenantSchemaExt;
                            tenant.schema.team(&team).map(|t| t.members.contains(id))
                        })
                        .unwrap_or(false),
                    _ => false,
                },
                SettingLevel::Person => {
                    matches!(principal, Principal::User(id) if id.as_str() == subject.as_str())
                }
            };
            reaches.then(|| Notice::Setting {
                setting: setting.clone(),
                level: *level,
            })
        }
        // A suspension changes a definition the tenant's owner manages, not a record anyone
        // reads; it is listed with the automation, not pushed to subscribers.
        EventBody::AutomationSuspended { .. } => None,
        // A posting names its owner, who is personal: the notice does not. Whether the subscriber
        // hears of it is the kernel's read decision for the account (`ledger::may_hear`).
        EventBody::LedgerEntryPosted {
            account,
            owner,
            unit,
            scope,
            entry_sequence,
            ..
        } => strata_kernel::ledger::may_hear(
            tenant,
            principal,
            event.at,
            owner,
            unit,
            scope.as_str(),
        )
        .then(|| Notice::Ledger {
            account: *account,
            unit: unit.clone(),
            scope: (!scope.as_str().is_empty()).then(|| scope.clone()),
            sequence: *entry_sequence,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_kernel::testkit::*;
    use strata_kernel::{Intent, World};

    fn published_then_withdrawn() -> (World, Vec<DomainEvent>) {
        let mut world = notes_world();
        let mut events = Vec::new();
        let mut run = |who: &str, now, intent| {
            let (next, outcome) = world.step(&intent, &ctx("alpha", user(who), now)).unwrap();
            world = next;
            events.extend(outcome.events);
        };
        let set_status = |status: &str| Intent::UpdateRecord {
            object: object("note"),
            id: strata_kernel::RecordId(1),
            fields: [(field("status"), text(status))].into(),
            expected_version: None,
        };
        run("alpha:writer1", 5_000, new_note("Draft", "draft"));
        run("alpha:editor", 5_001, set_status("published"));
        run("alpha:editor", 5_002, set_status("draft"));
        (world, events)
    }

    /// WG-5606: a person-level setting that a tenant operation writes is told to the person it is
    /// written for, so a view whose arguments read it is read again for them; no other person hears
    /// of it.
    #[test]
    fn a_setting_an_operation_writes_is_told_to_the_person_it_is_written_for() {
        use std::collections::BTreeSet;
        use strata_kernel::{
            ExistingData, Expr, FieldType, OperationSettingLevel, SchemaChange, SettingDef,
            SettingId, SettingType, TenantOperationDef, TenantOperationId, TenantOperationInput,
            TenantOperationInvokers, TenantOperationStep, Value,
        };
        let setting = SettingDef {
            id: SettingId::new("home_channel").expect("setting id"),
            label: "Home channel".into(),
            r#type: SettingType::new("string").expect("setting type"),
            default: Value::Str("general".into()),
            levels: BTreeSet::from([SettingLevel::Person]),
            locked_by: None,
            area: None,
            description: None,
            allowed: None,
        };
        let operation = TenantOperationDef {
            id: TenantOperationId::new("choose_channel").expect("operation id"),
            label: "Choose a channel".into(),
            inputs: vec![TenantOperationInput {
                name: "channel".into(),
                ty: FieldType::Str,
                required: true,
                label: None,
            }],
            invokers: TenantOperationInvokers {
                signed_in: true,
                roles: BTreeSet::new(),
            },
            subject: None,
            conditions: Vec::new(),
            steps: vec![TenantOperationStep::WriteSetting {
                id: "home".into(),
                setting: SettingId::new("home_channel").expect("setting id"),
                level: OperationSettingLevel::Person,
                value: Expr::Param("channel".into()),
            }],
        };
        let define = Intent::ChangeSchema {
            changes: vec![
                SchemaChange::DefineSetting(setting),
                SchemaChange::DefineOperation(operation),
            ],
            expected_version: None,
            existing: ExistingData::Reject,
        };
        let world = notes_world()
            .step(&define, &ctx("alpha", user("alpha:owner"), 5_000))
            .expect("the schema is accepted")
            .0;
        let invoke = Intent::InvokeOperation {
            operation: TenantOperationId::new("choose_channel").expect("operation id"),
            inputs: [("channel".to_string(), Value::Str("random".into()))].into(),
            idempotency_key: "k-1".into(),
        };
        let (world, outcome) = world
            .step(&invoke, &ctx("alpha", user("alpha:writer1"), 6_000))
            .expect("the invocation is accepted");
        let tenant = &world.tenants[&tenant_id("alpha")];
        let all = Selector::all();
        let for_writer: Vec<_> = outcome
            .events
            .iter()
            .map(|e| notice(tenant, &user("alpha:writer1"), &all, e))
            .collect();
        assert!(
            for_writer.iter().any(|n| matches!(
                n,
                Some(Notice::Setting { setting, level: SettingLevel::Person })
                    if setting.as_str() == "home_channel"
            )),
            "the person the value is written for hears of it: {for_writer:?}"
        );
        let for_editor: Vec<_> = outcome
            .events
            .iter()
            .map(|e| notice(tenant, &user("alpha:editor"), &all, e))
            .collect();
        assert!(
            for_editor
                .iter()
                .all(|n| !matches!(n, Some(Notice::Setting { .. }))),
            "another person hears nothing of it: {for_editor:?}"
        );
    }

    #[test]
    fn the_public_hears_nothing_about_a_draft_and_sees_a_withdrawal_as_a_removal() {
        let (world, events) = published_then_withdrawn();
        let tenant = &world.tenants[&tenant_id("alpha")];
        let all = Selector::all();
        let public: Vec<_> = events
            .iter()
            .map(|e| notice(tenant, &Principal::Anonymous, &all, e))
            .collect();
        assert_eq!(public[0], None, "creating a draft is invisible");
        assert!(matches!(public[1], Some(Notice::Upsert { version: 2, .. })));
        assert!(matches!(public[2], Some(Notice::Remove { .. })));
        let writer: Vec<_> = events
            .iter()
            .map(|e| notice(tenant, &user("alpha:writer2"), &all, e))
            .collect();
        assert!(writer
            .iter()
            .all(|n| matches!(n, Some(Notice::Upsert { .. }))));
    }

    #[test]
    fn selectors_filter_by_object_type_and_other_tenants_hear_nothing() {
        let (world, events) = published_then_withdrawn();
        let people_only = Selector {
            objects: Some([object("person")].into()),
            ..Selector::all()
        };
        let alpha = &world.tenants[&tenant_id("alpha")];
        assert!(events
            .iter()
            .all(|e| notice(alpha, &user("alpha:editor"), &people_only, e).is_none()));
        let beta = &world.tenants[&tenant_id("beta")];
        let all = Selector::all();
        assert!(events
            .iter()
            .all(|e| notice(beta, &user("beta:owner"), &all, e).is_none()));
    }

    fn only_kinds(kinds: &[&str]) -> Selector {
        Selector {
            kinds: Some(kinds.iter().map(|k| EventKind::new(*k).unwrap()).collect()),
            ..Selector::all()
        }
    }

    fn only_transitions(names: &[&str]) -> Selector {
        Selector {
            transitions: Some(names.iter().map(|n| n.to_string()).collect()),
            ..Selector::all()
        }
    }

    /// Verifies: SCH-1575
    #[test]
    fn a_kind_selector_passes_only_events_of_those_kinds() {
        let (world, events) = published_then_withdrawn();
        let tenant = &world.tenants[&tenant_id("alpha")];
        let writer = user("alpha:writer2");
        assert_eq!(
            kind_of(&events[0].body),
            "record-created",
            "the fixture's first event"
        );
        let told = |selector: &Selector| -> Vec<bool> {
            events
                .iter()
                .map(|e| notice(tenant, &writer, selector, e).is_some())
                .collect()
        };
        assert_eq!(told(&only_kinds(&["record-created"])), [true, false, false]);
        assert_eq!(told(&only_kinds(&["record-updated"])), [false, true, true]);
        assert_eq!(
            told(&only_kinds(&["record-created", "record-updated"])),
            [true, true, true],
            "the values of one member are alternatives"
        );
        assert_eq!(
            told(&only_kinds(&["record-deleted"])),
            [false, false, false]
        );
        assert_eq!(told(&Selector::all()), [true, true, true]);
    }

    /// Verifies: SCH-1575
    #[test]
    fn a_transition_selector_passes_only_updates_that_fired_those_transitions() {
        let (world, events) = published_then_withdrawn();
        let tenant = &world.tenants[&tenant_id("alpha")];
        let writer = user("alpha:writer2");
        let told = |who: &Principal, selector: &Selector| -> Vec<bool> {
            events
                .iter()
                .map(|e| notice(tenant, who, selector, e).is_some())
                .collect()
        };
        // The creation fired no transition; the second event fired publish, the third unpublish.
        assert_eq!(
            told(&writer, &only_transitions(&["publish"])),
            [false, true, false]
        );
        assert_eq!(
            told(&writer, &only_transitions(&["unpublish"])),
            [false, false, true]
        );
        assert_eq!(
            told(&writer, &only_transitions(&["publish", "unpublish"])),
            [false, true, true]
        );
        assert_eq!(
            told(&writer, &only_transitions(&["no-such-transition"])),
            [false, false, false]
        );
        // The members combine as AND: a creation is never an update that fired a transition.
        let both = Selector {
            kinds: Some(vec![EventKind::new("record-created").unwrap()]),
            transitions: Some(vec!["publish".to_string()]),
            ..Selector::all()
        };
        assert_eq!(told(&writer, &both), [false, false, false]);
    }

    /// Verifies: SCH-2202
    ///
    /// Two transitions between the same states with different guards: the selector matches the
    /// one whose guard let the update through, as the event names it, and not the other.
    #[test]
    fn a_transition_selector_tells_apart_two_transitions_between_the_same_states() {
        use strata_kernel::{
            ExistingData, Expr, SchemaChange, WorkflowDef, WorkflowId, WorkflowTransition,
        };
        let world = notes_world();
        let tenant = world.tenants[&tenant_id("alpha")].clone();
        let mut workflow: WorkflowDef =
            tenant.schema.workflows[&WorkflowId::new("note_status").unwrap()].clone();
        // `approve` comes first and only an owner passes its guard; an editor's publication
        // fires `publish`.
        let mut approve: WorkflowTransition = workflow.transitions[0].clone();
        approve.name = "approve".into();
        approve.guard = Some(Expr::HasRole(role("owner")));
        workflow.transitions.insert(0, approve);
        let mut world = world;
        let mut events = Vec::new();
        let mut run = |who: &str, now, intent| {
            let (next, outcome) = world.step(&intent, &ctx("alpha", user(who), now)).unwrap();
            world = next;
            events.extend(outcome.events);
        };
        run(
            "alpha:owner",
            5_000,
            Intent::ChangeSchema {
                changes: vec![SchemaChange::DefineWorkflow(workflow)],
                expected_version: None,
                existing: ExistingData::Reject,
            },
        );
        run("alpha:writer1", 5_001, new_note("Draft", "draft"));
        run(
            "alpha:editor",
            5_002,
            Intent::UpdateRecord {
                object: object("note"),
                id: strata_kernel::RecordId(1),
                fields: [(field("status"), text("published"))].into(),
                expected_version: None,
            },
        );
        let tenant = &world.tenants[&tenant_id("alpha")];
        let update = events.last().expect("the publication's event");
        let editor = user("alpha:editor");
        assert!(notice(tenant, &editor, &only_transitions(&["publish"]), update).is_some());
        assert_eq!(
            notice(tenant, &editor, &only_transitions(&["approve"]), update),
            None,
            "the transition whose guard did not hold did not fire"
        );
    }

    /// Verifies: SCH-1575
    ///
    /// A selector narrows and never widens: what the kernel's read decision hides from a
    /// subscriber stays hidden whatever the selector names.
    #[test]
    fn a_selector_never_widens_what_the_kernel_lets_a_subscriber_read() {
        let (world, events) = published_then_withdrawn();
        let tenant = &world.tenants[&tenant_id("alpha")];
        let everything = Selector {
            objects: Some([object("note")].into()),
            kinds: Some(vec![
                EventKind::new("record-created").unwrap(),
                EventKind::new("record-updated").unwrap(),
            ]),
            ..Selector::all()
        };
        // The public never reads the draft: no notice of its creation, and the withdrawal is a
        // removal, as without a selector.
        assert_eq!(
            notice(tenant, &Principal::Anonymous, &everything, &events[0]),
            None
        );
        assert!(matches!(
            notice(tenant, &Principal::Anonymous, &everything, &events[2]),
            Some(Notice::Remove { .. })
        ));
        // A transition selector for the creation's state change still shows the public nothing
        // of the draft, and shows a withdrawal only as the removal the kernel allows.
        let withdrawn = only_transitions(&["unpublish"]);
        assert_eq!(
            notice(tenant, &Principal::Anonymous, &withdrawn, &events[0]),
            None
        );
        assert_eq!(
            notice(tenant, &Principal::Anonymous, &withdrawn, &events[1]),
            None
        );
        assert!(matches!(
            notice(tenant, &Principal::Anonymous, &withdrawn, &events[2]),
            Some(Notice::Remove { .. })
        ));
        let published = only_transitions(&["publish"]);
        assert!(matches!(
            notice(tenant, &Principal::Anonymous, &published, &events[1]),
            Some(Notice::Upsert { version: 2, .. })
        ));
    }

    /// Verifies: SCH-1575
    #[test]
    fn a_request_is_refused_for_what_nothing_emits_and_for_names_that_are_not_valid() {
        let names = |list: &[&str]| Some(list.iter().map(|n| n.to_string()).collect::<Vec<_>>());
        assert_eq!(
            Selector::from_request(None, None, None, None, None),
            Ok(Selector::all())
        );
        let accepted = Selector::from_request(
            names(&["note"]),
            names(&["record-updated"]),
            None,
            None,
            names(&["publish"]),
        )
        .unwrap();
        assert_eq!(accepted.objects, Some([object("note")].into()));
        assert_eq!(
            accepted.kinds,
            Some(vec![EventKind::new("record-updated").unwrap()])
        );
        assert_eq!(accepted.transitions, Some(vec!["publish".to_string()]));

        let segments = Selector::from_request(
            None,
            None,
            names(&["0b1a6a7e-0000-4000-8000-000000000001"]),
            None,
            None,
        )
        .unwrap();
        assert_eq!(segments.segments.map(|set| set.len()), Some(1));
        assert!(Selector::from_request(None, None, names(&["not-a-uuid"]), None, None).is_err());
        let units = Selector::from_request(None, None, None, names(&["eur"]), None).unwrap();
        assert_eq!(units.units, Some(vec![LedgerUnit::new("eur").unwrap()]));
        assert!(Selector::from_request(None, None, None, names(&["Not A Unit"]), None).is_err());
        // Naming nothing is still naming the member: it would match nothing, so it is refused.
        assert!(Selector::from_request(None, None, Some(vec![]), None, None).is_err());
        assert!(Selector::from_request(None, None, None, Some(vec![]), None)
            .unwrap_err()
            .contains("names no unit"));
        let unknown = Selector::from_request(None, names(&["record-create"]), None, None, None);
        assert!(unknown.unwrap_err().contains("not emitted"));
        // A tenant-declared kind is refused for its own reason: the kernel emits none.
        let declared =
            Selector::from_request(None, names(&["app.survey-closed"]), None, None, None);
        assert!(declared
            .unwrap_err()
            .contains("no declare-event-kind or emit-event"));
        assert!(Selector::from_request(None, names(&["Not A Kind"]), None, None, None).is_err());
        // Every kind the kernel emits is accepted, the ledger's among them.
        assert!(
            Selector::from_request(None, names(&["ledger-entry-posted"]), None, None, None).is_ok()
        );
        assert!(Selector::from_request(None, None, None, None, names(&[""])).is_err());
        assert!(Selector::from_request(names(&["Not An Object"]), None, None, None, None).is_err());
    }

    /// Verifies: SCH-1576
    ///
    /// The kinds a selector may name are the generated `event-body`'s variants, written as event
    /// kinds: each is a valid `event-kind` and none claims the tenant's `app.` prefix, and the
    /// kind of a real event is one of them.
    #[test]
    fn every_variant_of_the_generated_event_body_has_a_kind_name() {
        let kinds = emitted_kinds();
        assert!(
            kinds.len() >= 9,
            "the variants were not read from the generated type: {kinds:?}"
        );
        for kind in &kinds {
            assert!(
                EventKind::new(kind.as_str()).is_ok() && !kind.starts_with("app."),
                "{kind:?} is not a platform event kind"
            );
        }
        let mut sorted = kinds.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), kinds.len(), "two variants share a kind name");
        for expected in [
            "tenant-created",
            "record-updated",
            "setting-changed",
            "automation-suspended",
            "ledger-entry-posted",
        ] {
            assert!(
                kinds.iter().any(|k| k == expected),
                "{expected} is not a kind"
            );
        }
        let (_, events) = published_then_withdrawn();
        for event in &events {
            let kind = kind_of(&event.body);
            assert!(kinds.contains(&kind), "{kind} is not among {kinds:?}");
        }
        let created = EventBody::TenantCreated {
            name: "Alpha".into(),
        };
        assert_eq!(kind_of(&created), "tenant-created");
    }

    /// Verifies: SCH-1575
    ///
    /// A posting is told to whoever may read the account (a holder of ledger-read, or the person
    /// who owns it) and to no one else, and a unit selector hears postings of its units only.
    #[test]
    fn a_posting_is_told_only_to_who_may_read_the_account() {
        use strata_kernel::ledger::account_id;
        use strata_kernel::{LedgerAmount, LedgerOwner, LedgerScope};
        let (world, events) = published_then_withdrawn();
        let tenant = &world.tenants[&tenant_id("alpha")];
        let owner = LedgerOwner::Principal {
            principal: principal_id("alpha:writer1"),
        };
        let unit = LedgerUnit::new("eur").unwrap();
        let posting = DomainEvent {
            body: EventBody::LedgerEntryPosted {
                account: account_id(&tenant.id, &owner, &unit, ""),
                owner,
                unit: unit.clone(),
                scope: LedgerScope(String::new()),
                amount: LedgerAmount(5),
                balance: LedgerAmount(5),
                balance_before: LedgerAmount(0),
                entry_sequence: 1,
                reason: "welcome".into(),
            },
            ..events[0].clone()
        };
        let all = Selector::all();
        let told = |who: &Principal, selector: &Selector| notice(tenant, who, selector, &posting);
        let expected = Notice::Ledger {
            account: account_id(
                &tenant.id,
                &LedgerOwner::Principal {
                    principal: principal_id("alpha:writer1"),
                },
                &unit,
                "",
            ),
            unit,
            scope: None,
            sequence: 1,
        };
        assert_eq!(told(&user("alpha:owner"), &all), Some(expected.clone()));
        assert_eq!(told(&user("alpha:writer1"), &all), Some(expected.clone()));
        assert_eq!(
            told(&user("alpha:writer2"), &all),
            None,
            "another person may not read the account"
        );
        assert_eq!(told(&Principal::Anonymous, &all), None);
        let units = |names: &[&str]| {
            Selector::from_request(
                None,
                None,
                None,
                Some(names.iter().map(|n| n.to_string()).collect()),
                None,
            )
            .unwrap()
        };
        assert_eq!(told(&user("alpha:owner"), &units(&["eur"])), Some(expected));
        assert_eq!(told(&user("alpha:owner"), &units(&["usd"])), None);
        // A unit selector hears no event that names no unit.
        assert_eq!(
            notice(tenant, &user("alpha:owner"), &units(&["eur"]), &events[0]),
            None
        );
    }

    /// A segment of the notes of a tenant (SEG-033), and the entry of `person` into it.
    fn segment_and_entry(person: &str) -> (Segment, SegmentEvent) {
        let mut bytes = [0u8; 16];
        bytes[15] = 1;
        bytes[6] = 0x40;
        bytes[8] = 0x80;
        let id = seg::segment_id_of(bytes);
        let segment = Segment {
            id,
            name: "Notes".to_string(),
            subject: object("note"),
            predicate: strata_kernel::Expr::Lit(strata_kernel::Value::Bool(true)),
            version: 1,
            time_dependent: false,
            indexable: false,
            refresh_seconds: None,
            defined_by: Some(principal_id("alpha:owner")),
            principal_field: field("slug"),
            visible_to_members: false,
            schema_version: strata_kernel::SchemaVersion(1),
            member_count: 0,
            refresh_skipped_at: None,
        };
        let logged = SegmentEvent {
            tenant: tenant_id("alpha"),
            at: strata_kernel::Timestamp(6_000),
            principal: Principal::System,
            correlation: "c-segment".to_string(),
            event: seg::SegmentMembershipEvent {
                segment: id,
                person: principal_id(person),
                direction: SegmentDirection::Entered,
                reason: seg::SegmentReason::Change,
                version: 1,
                revision: strata_kernel::StateRevision(0),
                hop: 0,
            },
        };
        (segment, logged)
    }

    // Verifies: SEG-033
    #[test]
    fn a_segment_selector_hears_the_entry_of_a_person_the_subscriber_may_read_and_nothing_else() {
        let mut world = notes_world();
        let mut run = |who: &str, now, intent| {
            let (next, _) = world.step(&intent, &ctx("alpha", user(who), now)).unwrap();
            world = next;
        };
        run("alpha:writer1", 5_000, new_note("Mine", "alpha:m1"));
        let tenant = world.tenants[&tenant_id("alpha")].clone();
        let state = State {
            tenant_id: tenant.id.clone(),
            tenant: Some(tenant),
            coverage: strata_kernel::Coverage::Complete,
        };
        let (segment, logged) = segment_and_entry("alpha:m1");
        let names =
            |list: Vec<String>| Selector::from_request(None, None, Some(list), None, None).unwrap();
        let selector = names(vec![segment.id.to_string()]);
        let hears = |who: &Principal, selector: &Selector, logged: &SegmentEvent| {
            segment_notice(&state, who, selector, &segment, 7, logged)
        };
        // A subscriber who reads the members and the person's record is told, by position.
        assert_eq!(
            hears(&user("alpha:owner"), &selector, &logged),
            Some(Notice::Segment {
                segment: segment.id,
                person: principal_id("alpha:m1"),
                direction: SegmentDirection::Entered,
                position: 7,
            })
        );
        // One who holds no right to the members, and one who is not signed in, are told nothing.
        assert_eq!(hears(&user("alpha:writer2"), &selector, &logged), None);
        assert_eq!(hears(&Principal::Anonymous, &selector, &logged), None);
        // A selector that does not name the segment never hears its events; one that names
        // another segment, or other kinds of event as well, does not either.
        assert_eq!(hears(&user("alpha:owner"), &Selector::all(), &logged), None);
        let mut other = [0u8; 16];
        other[15] = 2;
        other[6] = 0x40;
        other[8] = 0x80;
        let elsewhere = names(vec![seg::segment_id_of(other).to_string()]);
        assert_eq!(hears(&user("alpha:owner"), &elsewhere, &logged), None);
        let with_kinds = Selector::from_request(
            None,
            Some(vec!["record-updated".to_string()]),
            Some(vec![segment.id.to_string()]),
            None,
            None,
        )
        .unwrap();
        assert_eq!(hears(&user("alpha:owner"), &with_kinds, &logged), None);
        // The selector names the object type of the people it wants to hear of.
        let people = Selector::from_request(
            Some(vec!["person".to_string()]),
            None,
            Some(vec![segment.id.to_string()]),
            None,
            None,
        )
        .unwrap();
        assert_eq!(hears(&user("alpha:owner"), &people, &logged), None);
        // An erased person, whose record is gone, is announced to nobody.
        let (_, erased) = segment_and_entry("alpha:gone");
        assert_eq!(hears(&user("alpha:owner"), &selector, &erased), None);
        // An event of another tenant is never told.
        let mut foreign = logged.clone();
        foreign.tenant = tenant_id("beta");
        assert_eq!(hears(&user("alpha:owner"), &selector, &foreign), None);
    }

    // Verifies: SEG-033
    #[test]
    fn a_segment_selector_selects_no_event_of_the_kernels_sequence() {
        let (_, events) = published_then_withdrawn();
        let (segment, _) = segment_and_entry("alpha:m1");
        let selector =
            Selector::from_request(None, None, Some(vec![segment.id.to_string()]), None, None)
                .unwrap();
        assert!(events.iter().all(|event| !selector.admits(event)));
        assert!(events.iter().all(|event| Selector::all().admits(event)));
    }
}
