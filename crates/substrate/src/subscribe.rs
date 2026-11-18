//! Subscriptions as data, and the pure decision of what a subscriber is told about an event.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use strata_kernel::{
    owner_role, visible, DomainEvent, EventBody, ObjectTypeId, Principal, RecordId, TenantState,
};

/// What a subscriber wants to hear about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selector {
    /// Limit record changes to these object types. `None` means every object type.
    pub objects: Option<BTreeSet<ObjectTypeId>>,
}

/// What a subscriber is told. It never carries record contents: the subscriber reads again
/// through the ordinary authorized path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Notice {
    /// A record the subscriber can read was created or changed.
    Upsert { object: ObjectTypeId, id: RecordId, version: u64 },
    /// A record the subscriber could read is gone, or is no longer readable by them.
    Remove { object: ObjectTypeId, id: RecordId },
    /// The tenant's schema changed.
    Schema { version: u64 },
    /// The subscriber's own roles changed.
    Membership,
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
    let wants = |object: &ObjectTypeId| selector.objects.as_ref().is_none_or(|set| set.contains(object));
    let sees = |record| visible(tenant, principal, record, event.at);
    match &event.body {
        EventBody::TenantCreated { .. } => None,
        EventBody::SchemaChanged { version, .. } => Some(Notice::Schema { version: version.0 }),
        EventBody::MemberChanged { principal: changed, .. } => {
            let is_owner = matches!(principal, Principal::User(id)
                if tenant.members.get(id).is_some_and(|roles| roles.contains(&owner_role())));
            let is_subject = matches!(principal, Principal::User(id) if id == changed);
            (is_owner || is_subject).then_some(Notice::Membership)
        }
        EventBody::RecordCreated { record } => (wants(&record.object) && sees(record)).then(|| Notice::Upsert {
            object: record.object.clone(),
            id: record.id,
            version: record.version,
        }),
        EventBody::RecordUpdated { before, after } => {
            if !wants(&after.object) {
                None
            } else if sees(after) {
                Some(Notice::Upsert { object: after.object.clone(), id: after.id, version: after.version })
            } else if sees(before) {
                Some(Notice::Remove { object: after.object.clone(), id: after.id })
            } else {
                None
            }
        }
        EventBody::RecordDeleted { before } => (wants(&before.object) && sees(before))
            .then(|| Notice::Remove { object: before.object.clone(), id: before.id }),
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
        let all = Selector { objects: None };
        let public: Vec<_> = events.iter().map(|e| notice(tenant, &Principal::Anonymous, &all, e)).collect();
        assert_eq!(public[0], None, "creating a draft is invisible");
        assert!(matches!(public[1], Some(Notice::Upsert { version: 2, .. })));
        assert!(matches!(public[2], Some(Notice::Remove { .. })));
        let writer: Vec<_> = events.iter().map(|e| notice(tenant, &user("alpha:writer2"), &all, e)).collect();
        assert!(writer.iter().all(|n| matches!(n, Some(Notice::Upsert { .. }))));
    }

    #[test]
    fn selectors_filter_by_object_type_and_other_tenants_hear_nothing() {
        let (world, events) = published_then_withdrawn();
        let people_only = Selector { objects: Some([object("person")].into()) };
        let alpha = &world.tenants[&tenant_id("alpha")];
        assert!(events.iter().all(|e| notice(alpha, &user("alpha:editor"), &people_only, e).is_none()));
        let beta = &world.tenants[&tenant_id("beta")];
        let all = Selector { objects: None };
        assert!(events.iter().all(|e| notice(beta, &user("beta:owner"), &all, e).is_none()));
    }
}
