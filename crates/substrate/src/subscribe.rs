//! Subscriptions as data, and the pure decision of what a subscriber is told about an event.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use strata_kernel::ext::*;
use strata_kernel::rules::Lookup;
use strata_kernel::segment_flow;
use strata_kernel::segments::{self as seg, Segment, SegmentDirection, SegmentEvent, SegmentId};
use strata_kernel::{
    owner_role, visible, DomainEvent, EventBody, ObjectTypeId, Principal, PrincipalId, RecordId,
    SettingId, SettingLevel, State, TeamId, TenantState,
};

/// The kinds of event the kernel emits today, named as the specification names the variants of
/// `event-body`. A selector may name these and no other kind until another kind is emitted.
pub const EVENT_KINDS: [&str; 8] = [
    "tenant-created",
    "schema-changed",
    "member-changed",
    "record-created",
    "record-updated",
    "record-deleted",
    "setting-changed",
    "automation-suspended",
];

/// The kind of an event: the name of its body's variant in the specification (`event-kind`).
pub fn kind_of(body: &EventBody) -> &'static str {
    match body {
        EventBody::TenantCreated { .. } => "tenant-created",
        EventBody::SchemaChanged { .. } => "schema-changed",
        EventBody::MemberChanged { .. } => "member-changed",
        EventBody::RecordCreated { .. } => "record-created",
        EventBody::RecordUpdated { .. } => "record-updated",
        EventBody::RecordDeleted { .. } => "record-deleted",
        EventBody::SettingChanged { .. } => "setting-changed",
        EventBody::AutomationSuspended { .. } => "automation-suspended",
        EventBody::LedgerEntryPosted { .. } => "ledger-entry-posted",
    }
}

/// What a subscriber wants to hear about (the specification's type `selector`, SCH-1575). Every
/// member is optional and an absent member matches everything; members combine as AND, the values
/// of one member as OR. A selector narrows what a subscriber is told and never widens it: what
/// remains is still decided by the kernel's read decision for that subscriber.
///
/// `units` is not a member here: no ledger event of a unit is emitted yet, and [`Selector::new`]
/// refuses a request that names one rather than match nothing silently.
///
/// `segments` selects the entry and exit events of segments (SEG-033, D-7280). They are not
/// events of the kernel's sequence, so a selector that names segments hears only those, which are
/// told by [`segment_notice`], and a selector that does not name them never hears one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selector {
    /// Limit record changes to these object types. `None` means every object type.
    #[serde(default)]
    pub objects: Option<BTreeSet<ObjectTypeId>>,
    /// Limit events to these kinds (see [`EVENT_KINDS`]). `None` means every kind.
    #[serde(default)]
    pub kinds: Option<BTreeSet<String>>,
    /// Limit to record updates that fired one of these named workflow transitions. `None` means
    /// every event. The kernel names in the event the transition each workflow fired (D-7203,
    /// SCH-2202), so two transitions between the same states are told apart by the one whose
    /// guard let the update through.
    #[serde(default)]
    pub transitions: Option<BTreeSet<String>>,
    /// Limit to the entry and exit events of these segments. `None` selects no segment event.
    #[serde(default)]
    pub segments: Option<BTreeSet<SegmentId>>,
}

impl Selector {
    /// The selector that matches every event.
    pub fn all() -> Self {
        Selector::default()
    }

    /// A selector from the members of a subscription request, or the reason it is refused: a
    /// name that is not valid, a kind the kernel does not emit, a segment id that is not one, or
    /// a member (`units`) whose events are not emitted yet.
    pub fn new(
        objects: Option<Vec<String>>,
        kinds: Option<Vec<String>>,
        segments: Option<Vec<String>>,
        units: Option<Vec<String>>,
        transitions: Option<Vec<String>>,
    ) -> Result<Selector, String> {
        if units.is_some() {
            return Err(
                "units cannot be selected: no ledger event of a unit is emitted yet".into(),
            );
        }
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
                    .collect::<Result<BTreeSet<_>, _>>()
            })
            .transpose()?;
        let objects = objects
            .map(|names| {
                names
                    .into_iter()
                    .map(ObjectTypeId::new)
                    .collect::<Result<BTreeSet<_>, _>>()
                    .map_err(|e| e.to_string())
            })
            .transpose()?;
        let kinds = kinds
            .map(|names| {
                names
                    .into_iter()
                    .map(|name| {
                        if EVENT_KINDS.contains(&name.as_str()) {
                            Ok(name)
                        } else {
                            Err(format!(
                                "the event kind {name:?} is not emitted; the kinds are {}",
                                EVENT_KINDS.join(", ")
                            ))
                        }
                    })
                    .collect::<Result<BTreeSet<_>, _>>()
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
                    .collect::<Result<BTreeSet<_>, _>>()
            })
            .transpose()?;
        Ok(Selector {
            objects,
            kinds,
            transitions,
            segments,
        })
    }

    /// Whether the selector lets this event through at all, before visibility is asked. Events
    /// that name no record (schema and membership changes) are not limited by `objects`.
    fn admits(&self, event: &DomainEvent) -> bool {
        // A selector that names segments selects their events and nothing of the kernel's.
        if self.segments.is_some() {
            return false;
        }
        if let Some(kinds) = &self.kinds {
            if !kinds.contains(kind_of(&event.body)) {
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
                        .and_then(|team| tenant.schema.team(&team).map(|t| t.members.contains(id)))
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
        // A posting names its owner, who is personal; no socket notice carries it until the
        // ledger has a subscription of its own (units of the selector are refused above).
        EventBody::LedgerEntryPosted { .. } => None,
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
            kinds: Some(kinds.iter().map(|k| k.to_string()).collect()),
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
            kinds: Some(["record-created".to_string()].into()),
            transitions: Some(["publish".to_string()].into()),
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
            kinds: Some(
                ["record-created", "record-updated"]
                    .map(str::to_string)
                    .into(),
            ),
            transitions: None,
            segments: None,
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
            Selector::new(None, None, None, None, None),
            Ok(Selector::all())
        );
        let accepted = Selector::new(
            names(&["note"]),
            names(&["record-updated"]),
            None,
            None,
            names(&["publish"]),
        )
        .unwrap();
        assert_eq!(accepted.objects, Some([object("note")].into()));
        assert_eq!(accepted.kinds, Some(["record-updated".to_string()].into()));
        assert_eq!(accepted.transitions, Some(["publish".to_string()].into()));

        let segments = Selector::new(
            None,
            None,
            names(&["0b1a6a7e-0000-4000-8000-000000000001"]),
            None,
            None,
        )
        .unwrap();
        assert_eq!(segments.segments.map(|set| set.len()), Some(1));
        assert!(Selector::new(None, None, names(&["not-a-uuid"]), None, None).is_err());
        let units = Selector::new(None, None, None, names(&["eur"]), None);
        assert!(units.unwrap_err().contains("units"), "units are refused");
        // Naming nothing is still naming the member: it would match nothing, so it is refused.
        assert!(Selector::new(None, None, Some(vec![]), None, None).is_err());
        assert!(Selector::new(None, None, None, Some(vec![]), None).is_err());
        let unknown = Selector::new(None, names(&["record-create"]), None, None, None);
        assert!(unknown.unwrap_err().contains("not emitted"));
        assert!(Selector::new(None, None, None, None, names(&[""])).is_err());
        assert!(Selector::new(names(&["Not An Object"]), None, None, None, None).is_err());
    }
}
