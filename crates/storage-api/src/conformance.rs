//! The adapter conformance suite. Every `Store` implementation must pass [`run`].
//!
//! The suite drives the adapter with the kernel's notes fixture and compares what the adapter
//! holds with a `World` that received the same intents, so "the adapter stores what the kernel
//! decided" is checked by canonical hash rather than field by field.

use crate::{
    execute, execute_by, idempotency_scope, query, Account, AuditEntry, Commit, ExecuteError,
    FederatedIdentity, OutboxEntry, PasskeyCeremony, PasskeyChallenge, PasskeyRecord,
    PendingSignIn, PrincipalRemoval, RecoveryRequest, SessionRecord, SignInLink, Store, StoreError,
    TotpRecord, IDEMPOTENCY_RETENTION_MS,
};
use std::collections::BTreeMap;
use strata_kernel::testkit::*;
use strata_kernel::*;

/// A store and a world that have received exactly the same accepted intents.
struct Pair<'a> {
    store: &'a dyn Store,
    world: World,
    clock: i64,
}

impl<'a> Pair<'a> {
    /// A store loaded with the two-tenant notes fixture.
    fn new(store: &'a dyn Store) -> Self {
        let mut pair = Pair {
            store,
            world: World::default(),
            clock: 1_000,
        };
        for tenant in TENANTS {
            let owner = principal_id(&format!("{tenant}:owner"));
            pair.ok(
                tenant,
                Principal::System,
                Intent::CreateTenant {
                    name: tenant.to_string(),
                    owner: owner.clone(),
                },
            );
            pair.ok(
                tenant,
                Principal::User(owner.clone()),
                Intent::ChangeSchema {
                    changes: notes_schema(),
                    expected_version: None,
                    existing: ExistingData::Reject,
                },
            );
            for (name, role_name) in [("editor", "editor"), ("writer1", "writer")] {
                pair.ok(
                    tenant,
                    Principal::User(owner.clone()),
                    Intent::AssignRole {
                        principal: principal_id(&format!("{tenant}:{name}")),
                        role: role(role_name),
                    },
                );
            }
        }
        pair
    }

    fn run(
        &mut self,
        tenant: &str,
        principal: Principal,
        intent: Intent,
    ) -> Result<Transition, ExecuteError> {
        self.clock += 1;
        let context = ctx(tenant, principal, self.clock);
        let from_world = self.world.step(&intent, &context);
        let from_store = execute(self.store, &intent, &context);
        match (from_world, from_store) {
            (Ok((world, expected)), Ok(done)) => {
                assert_eq!(done.transition, expected, "store-backed and in-memory decisions differ");
                self.world = world;
                self.agree(tenant);
                Ok(done.transition)
            }
            (Err(expected), Err(ExecuteError::Domain(got))) => {
                assert_eq!(got.kind, expected.kind);
                self.agree(tenant);
                Err(ExecuteError::Domain(got))
            }
            (world, store) => panic!(
                "the store and the kernel disagree on {}: kernel accepted = {}, store result = {:?}",
                intent.name(),
                world.is_ok(),
                store.map(|d| d.receipt)
            ),
        }
    }

    fn ok(&mut self, tenant: &str, principal: Principal, intent: Intent) -> Transition {
        match self.run(tenant, principal, intent) {
            Ok(transition) => transition,
            Err(error) => panic!("fixture intent rejected: {error}"),
        }
    }

    fn note(&mut self, tenant: &str, title: &str, slug: &str) -> RecordId {
        match self
            .ok(
                tenant,
                user(&format!("{tenant}:writer1")),
                new_note(title, slug),
            )
            .output
        {
            Output::Record { id, .. } => id,
            other => panic!("unexpected output {other:?}"),
        }
    }

    /// The store's complete state of a tenant hashes the same as the world's.
    fn agree(&self, tenant: &str) {
        let id = tenant_id(tenant);
        let stored = self.store.load_complete(&id).expect("load_complete");
        assert_eq!(stored.coverage, Coverage::Complete);
        assert_eq!(
            canon::hash_of(&stored.tenant),
            self.world.tenant_hash(&id),
            "stored state of {tenant} differs from the kernel's"
        );
    }
}

fn session(id: &str, tenant: &str, principal: &str, expires: i64) -> SessionRecord {
    SessionRecord {
        id: SessionId::new(id).unwrap(),
        token_hash: format!("hash-of-{id}"),
        csrf_token: format!("csrf-{id}"),
        tenant: tenant_id(tenant),
        principal: principal_id(principal),
        display_name: principal.to_string(),
        created_at: Timestamp(10),
        last_seen_at: Timestamp(10),
        expires_at: Timestamp(expires),
        revoked_at: None,
        authenticated_at: Timestamp(10),
        network: None,
    }
}

fn absent_tenant_loads_as_absent(store: &dyn Store) {
    let state = store.load(&tenant_id("nowhere"), &[]).unwrap();
    assert!(state.tenant.is_none());
    assert!(store
        .load_complete(&tenant_id("nowhere"))
        .unwrap()
        .tenant
        .is_none());
    assert!(store.tenants().unwrap().is_empty());
    assert!(store
        .events_after(&tenant_id("nowhere"), 0, 10)
        .unwrap()
        .is_empty());
}

fn stored_state_equals_kernel_state(store: &dyn Store) {
    let mut pair = Pair::new(store);
    let first = pair.note("alpha", "First", "first");
    pair.note("alpha", "Second", "second");
    pair.note("beta", "Other", "first");
    let person = Intent::CreateRecord {
        object: object("person"),
        fields: BTreeMap::from([(field("name"), text("Ada"))]),
    };
    pair.ok("alpha", user("alpha:editor"), person);
    let patch = Intent::UpdateRecord {
        object: object("note"),
        id: first,
        fields: BTreeMap::from([
            (field("status"), text("published")),
            (field("author"), Value::Ref(RecordId(3))),
        ]),
        expected_version: Some(1),
    };
    pair.ok("alpha", user("alpha:editor"), patch);
    let delete = Intent::DeleteRecord {
        object: object("person"),
        id: RecordId(3),
        expected_version: None,
    };
    pair.ok("alpha", user("alpha:editor"), delete);
    assert!(pair
        .run("alpha", user("alpha:writer1"), new_note("Clash", "second"))
        .is_err());
    let summaries = store.tenants().unwrap();
    assert_eq!(summaries.len(), 2);
    assert_eq!(summaries[0].id, tenant_id("alpha"));
    assert_eq!(summaries[0].records, 2);
    assert_eq!(
        summaries[0].revision,
        pair.world.tenants[&tenant_id("alpha")].revision
    );
}

fn a_stale_commit_conflicts_and_changes_nothing(store: &dyn Store) {
    let mut pair = Pair::new(store);
    let alpha = tenant_id("alpha");
    let schema = store
        .load(&alpha, &[])
        .unwrap()
        .tenant
        .unwrap()
        .schema
        .clone();
    let stale_state = store
        .load(&alpha, &needs(Some(&schema), &new_note("Stale", "stale")))
        .unwrap();
    let context = ctx("alpha", user("alpha:writer1"), 9_000);
    let stale = transition(&stale_state, &new_note("Stale", "stale"), &context).unwrap();
    pair.note("alpha", "Winner", "winner");
    let events_before = store.events_after(&alpha, 0, 1_000).unwrap().len();
    let error = store
        .commit(&Commit::of(&new_note("Stale", "stale"), &context, &stale))
        .unwrap_err();
    assert!(
        matches!(error, StoreError::Conflict { .. }),
        "got {error:?}"
    );
    pair.agree("alpha");
    assert_eq!(
        store.events_after(&alpha, 0, 1_000).unwrap().len(),
        events_before
    );

    // Creating a tenant that already exists is the same conflict.
    let again = transition(
        &State::absent(alpha.clone()),
        &Intent::CreateTenant {
            name: "alpha".into(),
            owner: principal_id("alpha:owner"),
        },
        &ctx("alpha", Principal::System, 9_001),
    )
    .unwrap();
    let create = Intent::CreateTenant {
        name: "alpha".into(),
        owner: principal_id("alpha:owner"),
    };
    let error = store
        .commit(&Commit::of(
            &create,
            &ctx("alpha", Principal::System, 9_001),
            &again,
        ))
        .unwrap_err();
    assert!(matches!(error, StoreError::Conflict { .. }));
    pair.agree("alpha");
}

fn a_malformed_commit_is_refused_whole(store: &dyn Store) {
    let mut pair = Pair::new(store);
    let alpha = tenant_id("alpha");
    pair.note("alpha", "First", "first");
    let intent = new_note("Second", "second");
    let context = ctx("alpha", user("alpha:writer1"), 9_000);
    let schema = store
        .load(&alpha, &[])
        .unwrap()
        .tenant
        .unwrap()
        .schema
        .clone();
    let state = store.load(&alpha, &needs(Some(&schema), &intent)).unwrap();
    let good = transition(&state, &intent, &context).unwrap();
    let journal_before = store.journal_after(&alpha, 0, 1_000).unwrap().len();

    // An event sequence number that is already taken: the record write must not survive.
    let mut reused = Commit::of(&intent, &context, &good);
    reused.events[0].seq -= 1;
    assert!(matches!(store.commit(&reused), Err(StoreError::Invalid(_))));
    // An event addressed to another tenant.
    let mut foreign = Commit::of(&intent, &context, &good);
    foreign.events[0].tenant = tenant_id("beta");
    assert!(matches!(
        store.commit(&foreign),
        Err(StoreError::Invalid(_))
    ));

    pair.agree("alpha");
    pair.agree("beta");
    assert_eq!(
        store.journal_after(&alpha, 0, 1_000).unwrap().len(),
        journal_before
    );
    assert!(
        store
            .load(&alpha, &[Need::AllOf(object("note"))])
            .unwrap()
            .tenant
            .unwrap()
            .record_count()
            == 1
    );
    // The untouched commit still applies.
    store.commit(&Commit::of(&intent, &context, &good)).unwrap();
}

fn partial_loads_hold_what_the_needs_name(store: &dyn Store) {
    let mut pair = Pair::new(store);
    let first = pair.note("alpha", "First", "first");
    let second = pair.note("alpha", "Second", "second");
    pair.note("beta", "Beta note", "first");
    let alpha = tenant_id("alpha");
    let ids = |needs: &[Need]| -> Vec<RecordId> {
        let state = store.load(&alpha, needs).unwrap();
        assert_eq!(
            state.coverage,
            Coverage::Partial(needs.iter().cloned().collect())
        );
        let tenant = state.tenant.unwrap();
        assert_eq!(
            tenant.revision, pair.world.tenants[&alpha].revision,
            "header is always loaded"
        );
        tenant.records_of(&object("note")).map(|r| r.id).collect()
    };
    assert_eq!(ids(&[]), vec![]);
    assert_eq!(
        ids(&[Need::Record {
            object: object("note"),
            id: second
        }]),
        vec![second]
    );
    assert_eq!(
        ids(&[Need::Record {
            object: object("note"),
            id: RecordId(999)
        }]),
        vec![]
    );
    assert_eq!(ids(&[Need::AllOf(object("note"))]), vec![first, second]);
    // A page is the records after the cursor in id order, at most `limit` of them.
    assert_eq!(
        ids(&[Need::PageOf {
            object: object("note"),
            after: None,
            limit: 1
        }]),
        vec![first]
    );
    assert_eq!(
        ids(&[Need::PageOf {
            object: object("note"),
            after: Some(first),
            limit: 5
        }]),
        vec![second]
    );
    assert_eq!(
        ids(&[Need::PageOf {
            object: object("note"),
            after: Some(second),
            limit: 5
        }]),
        vec![]
    );
    assert_eq!(
        ids(&[Need::ByField {
            object: object("note"),
            field: field("slug"),
            value: text("first")
        }]),
        vec![first],
        "a field lookup returns this tenant's match only"
    );
    assert_eq!(
        ids(&[Need::ByField {
            object: object("note"),
            field: field("slug"),
            value: text("nope")
        }]),
        vec![]
    );
    // CMS-1557: the lookup of a composite unique constraint holds the records whose every named
    // field holds its value, and no other.
    let both = |title: &str, slug: &str| Need::ByFields {
        object: object("note"),
        fields: BTreeMap::from([(field("title"), text(title)), (field("slug"), text(slug))]),
    };
    assert_eq!(ids(&[both("First", "first")]), vec![first]);
    assert_eq!(ids(&[both("First", "second")]), vec![]);
}

fn list_fields_answer_field_lookups_by_containment(store: &dyn Store) {
    let mut pair = Pair::new(store);
    let add_tags = Intent::ChangeSchema {
        changes: vec![SchemaChange::DefineField {
            object: object("note"),
            field: field_def("tags", FieldType::List(Box::new(FieldType::Str)), false),
        }],
        expected_version: None,
        existing: ExistingData::Reject,
    };
    pair.ok("alpha", user("alpha:owner"), add_tags);
    let tagged = Intent::CreateRecord {
        object: object("note"),
        fields: BTreeMap::from([
            (field("title"), text("Tagged")),
            (field("slug"), text("tagged")),
            (field("tags"), Value::List(vec![text("wood"), text("glue")])),
        ]),
    };
    pair.ok("alpha", user("alpha:writer1"), tagged);
    pair.note("alpha", "Plain", "plain");
    let need = Need::ByField {
        object: object("note"),
        field: field("tags"),
        value: text("glue"),
    };
    let found = store
        .load(&tenant_id("alpha"), &[need])
        .unwrap()
        .tenant
        .unwrap();
    assert_eq!(found.record_count(), 1);
}

fn tenants_are_isolated(store: &dyn Store) {
    let mut pair = Pair::new(store);
    let alpha_note = pair.note("alpha", "Alpha only", "shared-slug");
    let beta_note = pair.note("beta", "Beta only", "shared-slug");
    assert_eq!(
        alpha_note, beta_note,
        "record ids are per tenant, so they collide on purpose"
    );
    let read = ReadRequest::Records {
        object: object("note"),
        ids: vec![alpha_note],
    };
    let as_beta_owner = query(store, &read, &ctx("beta", user("beta:owner"), 9_000)).unwrap();
    assert_eq!(
        as_beta_owner.rows[0].get(&field("title")),
        &text("Beta only")
    );
    // A member of alpha addressing beta is a stranger there and sees no draft.
    let as_alpha_owner = query(store, &read, &ctx("beta", user("alpha:owner"), 9_000)).unwrap();
    assert!(as_alpha_owner.rows.is_empty());
    let before = canon::hash_of(&store.load_complete(&tenant_id("beta")).unwrap().tenant);
    pair.note("alpha", "More", "more");
    assert_eq!(
        canon::hash_of(&store.load_complete(&tenant_id("beta")).unwrap().tenant),
        before
    );
    assert!(store
        .events_after(&tenant_id("beta"), 0, 1_000)
        .unwrap()
        .iter()
        .all(|e| e.tenant == tenant_id("beta")));
}

/// The outbox entries of one tenant, in the order the store returned them.
fn of_tenant(entries: &[OutboxEntry], tenant: &str) -> Vec<OutboxEntry> {
    entries
        .iter()
        .filter(|e| e.event.tenant.as_str() == tenant)
        .cloned()
        .collect()
}

/// Mark everything waiting as published, tenant by tenant.
fn publish_everything(store: &dyn Store) {
    let waiting = store.outbox(100_000).unwrap();
    let mut last: BTreeMap<TenantId, u64> = BTreeMap::new();
    for entry in &waiting {
        last.insert(entry.event.tenant.clone(), entry.position);
    }
    for (tenant, position) in last {
        store.mark_published(&tenant, position).unwrap();
    }
    assert!(store.outbox(10).unwrap().is_empty());
}

fn events_are_ordered_and_stay_in_the_outbox_until_published(store: &dyn Store) {
    let mut pair = Pair::new(store);
    let setup = store.outbox(1_000).unwrap();
    for tenant in TENANTS {
        let mine = of_tenant(&setup, tenant);
        assert!(!mine.is_empty());
        assert!(
            mine.windows(2)
                .all(|w| w[0].position < w[1].position && w[0].event.seq < w[1].event.seq),
            "each tenant's outbox follows its commit order"
        );
    }
    publish_everything(store);

    pair.note("alpha", "A", "a");
    pair.note("beta", "B", "b");
    pair.note("alpha", "C", "c");
    let pending = store.outbox(10).unwrap();
    assert_eq!(pending.len(), 3);
    let alpha_pending = of_tenant(&pending, "alpha");
    let beta_pending = of_tenant(&pending, "beta");
    assert_eq!(alpha_pending.len(), 2);
    assert_eq!(beta_pending.len(), 1);
    assert!(
        alpha_pending[0].position < alpha_pending[1].position
            && alpha_pending[0].event.seq < alpha_pending[1].event.seq,
        "the outbox follows commit order within a tenant"
    );
    let shared = store.outbox(2).unwrap();
    assert_eq!(shared.len(), 2);
    assert!(
        !of_tenant(&shared, "alpha").is_empty() && !of_tenant(&shared, "beta").is_empty(),
        "a small batch is shared between the tenants with events waiting"
    );
    assert_eq!(store.outbox_pending_of(&tenant_id("alpha")).unwrap(), 2);
    assert_eq!(store.outbox_pending_of(&tenant_id("beta")).unwrap(), 1);
    store
        .mark_published(&tenant_id("alpha"), alpha_pending[0].position)
        .unwrap();
    let left = store.outbox(10).unwrap();
    assert_eq!(of_tenant(&left, "alpha"), alpha_pending[1..].to_vec());
    assert_eq!(
        of_tenant(&left, "beta"),
        beta_pending,
        "marking one tenant's outbox leaves the other's waiting"
    );
    assert_eq!(store.health().unwrap().outbox_pending, 2);

    let alpha = tenant_id("alpha");
    let all = store.events_after(&alpha, 0, 1_000).unwrap();
    let seqs: Vec<u64> = all.iter().map(|e| e.seq).collect();
    assert_eq!(
        seqs,
        (1..=all.len() as u64).collect::<Vec<_>>(),
        "sequence numbers are gapless"
    );
    assert_eq!(
        store
            .events_after(&alpha, seqs[seqs.len() - 2], 1_000)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(store.events_after(&alpha, 0, 2).unwrap().len(), 2);
}

fn events_are_published_only_after_their_commit(store: &dyn Store) {
    let mut pair = Pair::new(store);
    publish_everything(store);
    let alpha = tenant_id("alpha");

    // A commit that loses the race, and one that is malformed, leave nothing in the outbox.
    let intent = new_note("Late", "late");
    let schema = store
        .load(&alpha, &[])
        .unwrap()
        .tenant
        .unwrap()
        .schema
        .clone();
    let stale_state = store.load(&alpha, &needs(Some(&schema), &intent)).unwrap();
    let context = ctx("alpha", user("alpha:writer1"), 9_000);
    let stale = transition(&stale_state, &intent, &context).unwrap();
    let mut malformed = Commit::of(&intent, &context, &stale);
    malformed.events[0].seq += 7;
    assert!(matches!(
        store.commit(&malformed),
        Err(StoreError::Invalid(_))
    ));
    assert!(
        store.outbox(10).unwrap().is_empty(),
        "a refused commit publishes nothing"
    );
    pair.note("alpha", "Winner", "winner");
    publish_everything(store);
    assert!(matches!(
        store.commit(&Commit::of(&intent, &context, &stale)),
        Err(StoreError::Conflict { .. })
    ));
    assert!(
        store.outbox(10).unwrap().is_empty(),
        "a conflicting commit publishes nothing"
    );

    // An accepted commit's events are waiting exactly when its state is visible, with the
    // positions its receipt names.
    pair.note("beta", "Beta", "beta");
    publish_everything(store);
    let state = store.load(&alpha, &[]).unwrap();
    let decided = transition(
        &store
            .load(&alpha, &needs(Some(&schema), &new_note("Next", "next")))
            .unwrap(),
        &new_note("Next", "next"),
        &ctx("alpha", user("alpha:writer1"), 9_100),
    )
    .unwrap();
    assert_eq!(
        decided.delta.base_revision,
        Some(state.tenant.unwrap().revision)
    );
    let receipt = store
        .commit(&Commit::of(
            &new_note("Next", "next"),
            &ctx("alpha", user("alpha:writer1"), 9_100),
            &decided,
        ))
        .unwrap();
    let waiting = store.outbox(10).unwrap();
    assert_eq!(waiting.len(), decided.events.len());
    assert!(waiting.iter().all(|e| e.event.tenant == alpha));
    assert_eq!(
        waiting.last().map(|e| e.position),
        receipt.last_position,
        "the receipt names the position of the commit's last event"
    );
    let stored = store.load(&alpha, &[]).unwrap().tenant.unwrap();
    assert_eq!(stored.revision, receipt.revision);
    assert!(
        waiting.iter().all(|e| e.event.revision <= stored.revision),
        "every waiting event belongs to a commit the store already shows"
    );
    assert_eq!(
        waiting.iter().map(|e| e.event.clone()).collect::<Vec<_>>(),
        store
            .events_after(&alpha, stored.event_seq - waiting.len() as u64, 100)
            .unwrap(),
        "the waiting events are the newest of the tenant's log"
    );
    // Marking another tenant's outbox does not publish them.
    store
        .mark_published(&tenant_id("beta"), u64::MAX >> 1)
        .unwrap();
    assert_eq!(store.outbox(10).unwrap(), waiting);
}

fn the_journal_replays_to_the_stored_state(store: &dyn Store) {
    let mut pair = Pair::new(store);
    let id = pair.note("alpha", "First", "first");
    let publish = Intent::UpdateRecord {
        object: object("note"),
        id,
        fields: BTreeMap::from([(field("status"), text("published"))]),
        expected_version: None,
    };
    pair.ok("alpha", user("alpha:editor"), publish);
    let alpha = tenant_id("alpha");
    let mut replayed = World::default();
    let mut after = 0;
    // The count agrees with the entries and is per tenant; an absent tenant has none.
    let counted = store.journal_count(&alpha).unwrap();
    assert_eq!(
        counted,
        store.journal_after(&alpha, 0, 1_000).unwrap().len() as u64
    );
    assert!(counted > 0);
    assert_eq!(store.journal_count(&tenant_id("nobody")).unwrap(), 0);
    loop {
        let page = store.journal_after(&alpha, after, 3).unwrap();
        if page.is_empty() {
            break;
        }
        for entry in page {
            assert_eq!(
                entry.revision.0,
                after + 1,
                "journal revisions are contiguous"
            );
            assert_eq!(entry.kernel_version, KERNEL_VERSION);
            let (next, outcome) = replayed
                .step(&entry.intent, &entry.context)
                .expect("journalled intent replays");
            assert_eq!(canon::hash_of(&outcome.delta), entry.delta_hash);
            assert_eq!(canon::hash_of(&outcome.events), entry.events_hash);
            replayed = next;
            after = entry.revision.0;
        }
    }
    assert_eq!(replayed.tenant_hash(&alpha), pair.world.tenant_hash(&alpha));
}

fn sessions_are_stored_revoked_and_purged(store: &dyn Store) {
    let mut pair = Pair::new(store);
    assert!(matches!(
        store.put_session(&session("s0", "nowhere", "x", 100)),
        Err(StoreError::NotFound(_))
    ));
    store
        .put_session(&session("s1", "alpha", "alpha:writer1", 5_000))
        .unwrap();
    store
        .put_session(&session("s2", "alpha", "alpha:editor", 5_000))
        .unwrap();
    store
        .put_session(&session("s3", "beta", "alpha:writer1", 50))
        .unwrap();
    assert_eq!(
        store
            .session_by_token("hash-of-s1")
            .unwrap()
            .unwrap()
            .id
            .as_str(),
        "s1"
    );
    assert!(
        store.session_by_token("s1").unwrap().is_none(),
        "lookup is by hash, never by id"
    );
    let mut clash = session("s9", "alpha", "alpha:editor", 5_000);
    clash.token_hash = "hash-of-s1".into();
    assert!(store.put_session(&clash).is_err());

    let mut seen = session("s1", "alpha", "alpha:writer1", 5_000);
    seen.last_seen_at = Timestamp(77);
    store.put_session(&seen).unwrap();
    assert_eq!(
        store
            .session(&SessionId::new("s1").unwrap())
            .unwrap()
            .unwrap()
            .last_seen_at,
        Timestamp(77)
    );

    // Losing the last role in alpha ends that principal's alpha sessions, in the same commit.
    let revoke = Intent::RevokeRole {
        principal: principal_id("alpha:writer1"),
        role: role("writer"),
    };
    pair.ok("alpha", user("alpha:owner"), revoke);
    assert!(store
        .session(&SessionId::new("s1").unwrap())
        .unwrap()
        .unwrap()
        .revoked_at
        .is_some());
    assert!(store
        .session(&SessionId::new("s2").unwrap())
        .unwrap()
        .unwrap()
        .revoked_at
        .is_none());
    assert!(
        store
            .session(&SessionId::new("s3").unwrap())
            .unwrap()
            .unwrap()
            .revoked_at
            .is_none(),
        "beta is untouched"
    );

    // A last-seen or step-up write made from a copy read before the revocation must not undo it.
    let s1 = SessionId::new("s1").unwrap();
    store.session_seen(&s1, Timestamp(88)).unwrap();
    store
        .session_authenticated(&s1, Timestamp(99), Some("net"))
        .unwrap();
    let after = store.session(&s1).unwrap().unwrap();
    assert!(
        after.revoked_at.is_some(),
        "a seen or step-up write keeps the revocation"
    );
    assert_eq!(after.last_seen_at, Timestamp(88));
    assert_eq!(after.authenticated_at, Timestamp(99));
    assert_eq!(after.network.as_deref(), Some("net"));
    store
        .session_seen(&SessionId::new("absent").unwrap(), Timestamp(1))
        .unwrap();

    let live: Vec<String> = store
        .live_sessions(Timestamp(100))
        .unwrap()
        .iter()
        .map(|s| s.id.to_string())
        .collect();
    assert_eq!(live, ["s2"], "s1 is revoked and s3 has expired");
    let of = |tenant: &str, principal: Option<&str>| -> Vec<String> {
        let principal = principal.map(principal_id);
        store
            .live_sessions_of(&tenant_id(tenant), principal.as_ref(), Timestamp(100))
            .unwrap()
            .iter()
            .map(|s| s.id.to_string())
            .collect()
    };
    assert_eq!(of("alpha", None), ["s2"]);
    assert_eq!(of("alpha", Some("alpha:editor")), ["s2"]);
    assert!(of("alpha", Some("alpha:writer1")).is_empty());
    assert!(of("beta", None).is_empty(), "s3 has expired");
    store.liveness().unwrap();
    assert_eq!(store.purge_sessions(Timestamp(100)).unwrap(), 1);
    assert!(store
        .session(&SessionId::new("s3").unwrap())
        .unwrap()
        .is_none());
}

/// The identity service's documents. Not a row of the conformance table: the table states the
/// storage guarantees, and this one is a mechanism of the identity service. Run by each adapter's
/// own test.
pub fn documents_are_scoped_to_their_tenant_and_kind(store: &dyn Store) {
    let _pair = Pair::new(store);
    let (alpha, beta) = (tenant_id("alpha"), tenant_id("beta"));
    assert!(store
        .doc_put(&alpha, "client", "c1", "{\"n\":1}", true)
        .unwrap());
    assert!(
        !store
            .doc_put(&alpha, "client", "c1", "{\"n\":2}", true)
            .unwrap(),
        "create-only keeps the first"
    );
    assert_eq!(
        store.doc_get(&alpha, "client", "c1").unwrap().as_deref(),
        Some("{\"n\":1}")
    );
    assert!(store
        .doc_put(&alpha, "client", "c1", "{\"n\":3}", false)
        .unwrap());
    assert_eq!(
        store.doc_get(&alpha, "client", "c1").unwrap().as_deref(),
        Some("{\"n\":3}")
    );
    assert_eq!(store.doc_get(&beta, "client", "c1").unwrap(), None);
    assert_eq!(store.doc_get(&alpha, "key", "c1").unwrap(), None);
    store.doc_put(&alpha, "client", "c0", "x", false).unwrap();
    let listed = store.doc_list(&alpha, "client").unwrap();
    assert_eq!(
        listed.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        ["c0", "c1"]
    );
    assert!(store.doc_list(&beta, "client").unwrap().is_empty());
    assert!(store.doc_remove(&alpha, "client", "c0").unwrap());
    assert!(!store.doc_remove(&alpha, "client", "c0").unwrap());
    assert!(store
        .doc_put(&tenant_id("nowhere"), "client", "c", "x", false)
        .is_err());
}

/// A request that repeats an idempotency key returns the first result and commits nothing; the
/// key is scoped to its tenant and principal, a key used with another intent is refused, a
/// request past its deadline commits nothing, and a record past its retention is forgotten.
fn idempotency_keys_return_the_first_result(store: &dyn Store) {
    let _pair = Pair::new(store);
    let (alpha, beta) = (tenant_id("alpha"), tenant_id("beta"));
    let records_of = |tenant: &TenantId| -> (usize, StateRevision) {
        let state = store.load_complete(tenant).expect("load_complete");
        let tenant = state.tenant.expect("tenant");
        (
            tenant.records.values().map(|by_id| by_id.len()).sum(),
            tenant.revision,
        )
    };
    let writer = user("alpha:writer1");
    let keyed = |tenant: &str, principal: Principal, now: i64, key: &str| {
        let mut context = ctx(tenant, principal, now);
        context.idempotency_key = Some(key.to_string());
        context
    };

    let (records_before, revision_before) = records_of(&alpha);
    let first = execute(
        store,
        &new_note("Once", "once"),
        &keyed("alpha", writer.clone(), 5_000, "k-1"),
    )
    .expect("the first request commits");
    assert!(!first.replayed);
    let (records_after, revision_after) = records_of(&alpha);
    assert_eq!(records_after, records_before + 1);
    assert_ne!(revision_after, revision_before);

    // The same request again, as a client does after a lost reply: the first result, no commit.
    let again = execute(
        store,
        &new_note("Once", "once"),
        &keyed("alpha", writer.clone(), 5_500, "k-1"),
    )
    .expect("the repeat is answered");
    assert!(again.replayed, "the repeat was committed again");
    assert_eq!(again.receipt, first.receipt);
    assert_eq!(again.transition.output, first.transition.output);
    assert_eq!(again.transition.evidence, first.transition.evidence);
    assert_eq!(
        again.transition.delta.event_seq,
        first.transition.delta.event_seq
    );
    assert_eq!(records_of(&alpha), (records_after, revision_after));

    // The same key with another intent is refused, and commits nothing.
    match execute(
        store,
        &new_note("Other", "other"),
        &keyed("alpha", writer.clone(), 5_600, "k-1"),
    ) {
        Err(ExecuteError::Domain(error)) => assert_eq!(error.kind, ErrorKind::IdempotencyMismatch),
        other => panic!("a key used with another intent was not refused: {other:?}"),
    }
    assert_eq!(records_of(&alpha), (records_after, revision_after));

    // The scope is the tenant and the principal: the key is unknown to another principal of the
    // tenant and to the same principal in another tenant.
    let scope = idempotency_scope(&writer);
    let now = Timestamp(5_700);
    assert!(store
        .idempotent_result(&alpha, &scope, "k-1", now)
        .unwrap()
        .is_some());
    assert!(store
        .idempotent_result(
            &alpha,
            &idempotency_scope(&user("alpha:editor")),
            "k-1",
            now
        )
        .unwrap()
        .is_none());
    assert!(store
        .idempotent_result(&beta, &scope, "k-1", now)
        .unwrap()
        .is_none());
    let in_beta = execute(
        store,
        &new_note("Once", "once"),
        &keyed("beta", user("beta:writer1"), 5_800, "k-1"),
    )
    .expect("the same key in another tenant is a new request");
    assert!(!in_beta.replayed);

    // A request whose deadline has passed commits nothing.
    let before = records_of(&alpha);
    match execute_by(
        store,
        &new_note("Late", "late"),
        &keyed("alpha", writer.clone(), 5_900, "k-late"),
        Some(std::time::Instant::now()),
    ) {
        Err(ExecuteError::Store(StoreError::Unavailable(_))) => {}
        other => panic!("a request past its deadline was not refused: {other:?}"),
    }
    assert_eq!(records_of(&alpha), before);
    assert!(store
        .idempotent_result(&alpha, &scope, "k-late", now)
        .unwrap()
        .is_none());

    // Retention: a record is returned for IDEMPOTENCY_RETENTION_MS after its commit and is
    // forgotten after that. The key is then a new request, which the next commit records again.
    let edge = Timestamp(5_000 + IDEMPOTENCY_RETENTION_MS);
    assert!(store
        .idempotent_result(&alpha, &scope, "k-1", edge)
        .unwrap()
        .is_some());
    let past = Timestamp(edge.0 + 1);
    assert!(store
        .idempotent_result(&alpha, &scope, "k-1", past)
        .unwrap()
        .is_none());
    let renewed = execute(
        store,
        &new_note("Twice", "twice"),
        &keyed("alpha", writer, past.0, "k-1"),
    )
    .expect("a key past its retention is a new request");
    assert!(!renewed.replayed);
}

/// The security audit trail. Not a row of the conformance table, like the documents: run by each
/// adapter's own test. Lines are kept per tenant in order, a kind can be asked for alone, the
/// newest lines are the ones a limit keeps, and an unknown tenant is refused.
pub fn audit_is_kept_per_tenant(store: &dyn Store) {
    let _pair = Pair::new(store);
    let (alpha, beta) = (tenant_id("alpha"), tenant_id("beta"));
    let line = |at: i64, kind: &str, event: &str| AuditEntry {
        at,
        kind: kind.into(),
        event: event.into(),
        correlation: format!("c-{at}"),
        data: "{}".into(),
    };
    store
        .audit_append(&alpha, &line(1, "decision", "auth.decision"))
        .unwrap();
    store
        .audit_append(&alpha, &line(2, "audit", "signing_key.compromised"))
        .unwrap();
    for at in 3..10 {
        store
            .audit_append(&alpha, &line(at, "decision", "auth.decision"))
            .unwrap();
    }
    let kept = store.audit_list(&alpha, Some("audit"), 100).unwrap();
    assert_eq!(
        kept.iter().map(|l| l.at).collect::<Vec<_>>(),
        [2],
        "a flood of decisions does not push an audit line out"
    );
    let newest = store.audit_list(&alpha, None, 3).unwrap();
    assert_eq!(
        newest.iter().map(|l| l.at).collect::<Vec<_>>(),
        [7, 8, 9],
        "the newest lines, oldest first"
    );
    assert!(store.audit_list(&beta, None, 100).unwrap().is_empty());
    assert!(store
        .audit_append(&tenant_id("nowhere"), &line(1, "audit", "x"))
        .is_err());
}

fn accounts_are_scoped_to_their_tenant(store: &dyn Store) {
    let _pair = Pair::new(store);
    let account = Account {
        tenant: tenant_id("alpha"),
        login: "ines".into(),
        principal: principal_id("alpha:editor"),
        display_name: "Ines".into(),
        password_hash: "$argon2id$placeholder".into(),
        verified_email: Some("ines@example.org".into()),
    };
    store.put_account(&account).unwrap();
    assert_eq!(
        store.account(&tenant_id("alpha"), "ines").unwrap(),
        Some(account.clone())
    );
    assert_eq!(store.account(&tenant_id("beta"), "ines").unwrap(), None);
    assert_eq!(store.account(&tenant_id("alpha"), "INES").unwrap(), None);
    // The verified address belongs to the account's principal in its own tenant.
    assert_eq!(
        store
            .verified_address(&tenant_id("alpha"), &principal_id("alpha:editor"))
            .unwrap(),
        Some("ines@example.org".to_string())
    );
    assert_eq!(
        store
            .verified_address(&tenant_id("beta"), &principal_id("alpha:editor"))
            .unwrap(),
        None
    );
    assert_eq!(
        store
            .verified_address(&tenant_id("alpha"), &principal_id("alpha:nobody"))
            .unwrap(),
        None
    );
    let changed = Account {
        display_name: "Inès".into(),
        ..account
    };
    store.put_account(&changed).unwrap();
    assert_eq!(
        store.account(&tenant_id("alpha"), "ines").unwrap(),
        Some(changed)
    );
    // Removing a login removes that one only, in its own tenant.
    let other = Account {
        tenant: tenant_id("beta"),
        login: "ines".into(),
        principal: principal_id("beta:editor"),
        verified_email: None,
        display_name: "Ines".into(),
        password_hash: "$argon2id$placeholder".into(),
    };
    store.put_account(&other).unwrap();
    assert!(store.remove_account(&tenant_id("alpha"), "ines").unwrap());
    assert!(!store.remove_account(&tenant_id("alpha"), "ines").unwrap());
    assert_eq!(store.account(&tenant_id("alpha"), "ines").unwrap(), None);
    assert_eq!(
        store.account(&tenant_id("beta"), "ines").unwrap(),
        Some(other)
    );
    an_account_is_created_once_and_updated_only_from_what_was_read(store);
    a_principal_is_removed_with_every_credential_it_held(store);
}

/// Part of the accounts check: removing a principal removes every login and credential row of
/// that principal in its tenant, and nothing of another person or another tenant.
fn a_principal_is_removed_with_every_credential_it_held(store: &dyn Store) {
    let (alpha, beta) = (tenant_id("alpha"), tenant_id("beta"));
    let (gone, kept) = (principal_id("alpha:editor"), principal_id("alpha:writer1"));
    let at = Timestamp(1);
    let later = Timestamp(1_000_000);
    // Every kind of row, held by `who` in `tenant`, named by `tag`.
    let hold = |tenant: &TenantId, who: &PrincipalId, tag: &str| {
        let name = |what: &str| format!("{what}-{tag}");
        store
            .put_account(&Account {
                tenant: tenant.clone(),
                login: name("login"),
                principal: who.clone(),
                display_name: "Person".into(),
                password_hash: "$argon2id$placeholder".into(),
                verified_email: None,
            })
            .unwrap();
        assert!(store
            .put_passkey(&PasskeyRecord {
                tenant: tenant.clone(),
                principal: who.clone(),
                id: name("passkey"),
                rp_id: "example.org".into(),
                label: "Key".into(),
                credential: "{}".into(),
                created_at: at,
                last_used_at: None,
            })
            .unwrap());
        store
            .put_passkey_challenge(&PasskeyChallenge {
                tenant: tenant.clone(),
                id_hash: name("challenge"),
                ceremony: PasskeyCeremony::SignIn,
                session: None,
                principal: Some(who.clone()),
                origin: "https://example.org".into(),
                state: "{}".into(),
                expires_at: later,
            })
            .unwrap();
        store
            .put_sign_in_link(&SignInLink {
                tenant: tenant.clone(),
                id_hash: name("link"),
                principal: who.clone(),
                token_hash: "t".into(),
                binding_hash: "b".into(),
                origin: "https://example.org".into(),
                expires_at: later,
            })
            .unwrap();
        assert!(store
            .put_identity(&FederatedIdentity {
                tenant: tenant.clone(),
                provider: "idp".into(),
                subject: name("subject"),
                principal: who.clone(),
                created_at: at,
            })
            .unwrap());
        assert!(store
            .put_totp(&TotpRecord {
                tenant: tenant.clone(),
                principal: who.clone(),
                secret: "sealed".into(),
                confirmed: true,
                last_step: -1,
                created_at: at,
            })
            .unwrap());
        store
            .replace_recovery_codes(tenant, who, &[name("code-1"), name("code-2")], at)
            .unwrap();
        store
            .put_pending(&PendingSignIn {
                tenant: tenant.clone(),
                id_hash: name("pending"),
                principal: who.clone(),
                method: "password".into(),
                factors: 1,
                attempts: 0,
                expires_at: later,
            })
            .unwrap();
        store
            .put_recovery(&RecoveryRequest {
                tenant: tenant.clone(),
                id_hash: name("recovery"),
                principal: who.clone(),
                login: name("login"),
                token_hash: "t".into(),
                cancel_hash: "c".into(),
                requested_at: at,
                completes_at: at,
                expires_at: later,
            })
            .unwrap();
    };
    hold(&alpha, &gone, "gone");
    hold(&alpha, &kept, "kept");
    // The same principal in another tenant is another person's rows.
    hold(&beta, &gone, "other-tenant");

    let removed = store.remove_principal(&alpha, &gone).unwrap();
    assert_eq!(
        removed,
        PrincipalRemoval {
            accounts: 1,
            passkeys: 1,
            passkey_challenges: 1,
            provider_flows: 0,
            identities: 1,
            totp_factors: 1,
            recovery_codes: 2,
            pending_sign_ins: 1,
            sign_in_links: 1,
            recovery_requests: 1,
        }
    );
    assert!(!removed.is_empty());

    // Nothing of the removed principal is left in alpha.
    let now = Timestamp(2);
    assert_eq!(store.account(&alpha, "login-gone").unwrap(), None);
    assert_eq!(store.account_of(&alpha, &gone).unwrap(), None);
    assert!(store.passkeys(&alpha, &gone).unwrap().is_empty());
    assert_eq!(store.passkey(&alpha, "passkey-gone").unwrap(), None);
    assert_eq!(
        store
            .take_passkey_challenge(&alpha, "challenge-gone", now)
            .unwrap(),
        None
    );
    assert_eq!(store.take_sign_in_link(&alpha, "link-gone").unwrap(), None);
    assert_eq!(store.identity(&alpha, "idp", "subject-gone").unwrap(), None);
    assert_eq!(store.totp(&alpha, &gone).unwrap(), None);
    assert_eq!(store.recovery_codes_left(&alpha, &gone).unwrap(), 0);
    assert_eq!(
        store.take_pending(&alpha, "pending-gone", now).unwrap(),
        None
    );
    assert_eq!(store.recovery(&alpha, "recovery-gone").unwrap(), None);

    // Another person of the tenant, and the same principal of another tenant, keep theirs.
    assert!(store.account(&alpha, "login-kept").unwrap().is_some());
    assert_eq!(store.passkeys(&alpha, &kept).unwrap().len(), 1);
    assert!(store
        .identity(&alpha, "idp", "subject-kept")
        .unwrap()
        .is_some());
    assert!(store.totp(&alpha, &kept).unwrap().is_some());
    assert_eq!(store.recovery_codes_left(&alpha, &kept).unwrap(), 2);
    assert!(store.recovery(&alpha, "recovery-kept").unwrap().is_some());
    assert!(store
        .account(&beta, "login-other-tenant")
        .unwrap()
        .is_some());
    assert_eq!(store.passkeys(&beta, &gone).unwrap().len(), 1);
    assert!(store
        .identity(&beta, "idp", "subject-other-tenant")
        .unwrap()
        .is_some());
    assert!(store.totp(&beta, &gone).unwrap().is_some());
    assert_eq!(store.recovery_codes_left(&beta, &gone).unwrap(), 2);
    assert!(store
        .take_pending(&beta, "pending-other-tenant", now)
        .unwrap()
        .is_some());

    // A second removal, and a principal or tenant with nothing held, remove nothing.
    assert!(store.remove_principal(&alpha, &gone).unwrap().is_empty());
    assert!(store
        .remove_principal(&alpha, &principal_id("alpha:nobody"))
        .unwrap()
        .is_empty());
    assert!(store
        .remove_principal(&tenant_id("absent"), &gone)
        .unwrap()
        .is_empty());
}

/// Part of the accounts check: a login is created once, and updated only from what was read.
fn an_account_is_created_once_and_updated_only_from_what_was_read(store: &dyn Store) {
    let alpha = tenant_id("alpha");
    let account = Account {
        tenant: alpha.clone(),
        login: "wren".into(),
        principal: principal_id("alpha:wren"),
        display_name: "Wren".into(),
        password_hash: String::new(),
        verified_email: None,
    };
    assert!(store.create_account(&account).unwrap());
    // A second creation of the login is refused and leaves the first account as it was.
    let rival = Account {
        principal: principal_id("alpha:rival"),
        display_name: "Rival".into(),
        ..account.clone()
    };
    assert!(!store.create_account(&rival).unwrap());
    assert_eq!(
        store.account(&alpha, "wren").unwrap(),
        Some(account.clone())
    );
    // An update applies to what was read, once; a second one from the same read is refused.
    let verified = Account {
        verified_email: Some("wren@example.org".into()),
        ..account.clone()
    };
    assert!(store.update_account(&account, &verified).unwrap());
    let renamed = Account {
        display_name: "Wren I".into(),
        ..account.clone()
    };
    assert!(!store.update_account(&account, &renamed).unwrap());
    assert_eq!(
        store.account(&alpha, "wren").unwrap(),
        Some(verified.clone())
    );
    // An update of a login that is not there changes nothing.
    let ghost = Account {
        login: "ghost".into(),
        ..account
    };
    assert!(!store.update_account(&ghost, &ghost).unwrap());
    assert_eq!(store.account(&alpha, "ghost").unwrap(), None);
}

fn snapshots_restore_to_the_same_hash(store: &dyn Store) {
    let mut pair = Pair::new(store);
    let alpha = tenant_id("alpha");
    pair.note("alpha", "Kept", "kept");
    let kept_world = pair.world.clone();
    let info = store
        .create_snapshot(&alpha, "before", Timestamp(42))
        .unwrap();
    assert_eq!(info.hash, kept_world.tenant_hash(&alpha));
    assert_eq!(info.revision, kept_world.tenants[&alpha].revision);
    pair.note("alpha", "Discarded", "discarded");
    pair.note("beta", "Unrelated", "unrelated");
    let beta_before = canon::hash_of(&store.load_complete(&tenant_id("beta")).unwrap().tenant);

    assert_eq!(
        canon::hash_of(&store.load_snapshot(&alpha, info.id).unwrap()),
        info.hash
    );
    assert_eq!(store.snapshots(&alpha).unwrap(), vec![info.clone()]);
    assert!(store.snapshots(&tenant_id("beta")).unwrap().is_empty());
    assert!(matches!(
        store.load_snapshot(&tenant_id("beta"), info.id),
        Err(StoreError::NotFound(_))
    ));

    let passed_seq = pair.world.tenants[&alpha].event_seq;
    store.restore_snapshot(&alpha, info.id).unwrap();
    // The restored state is the snapshot's, except that its event counter stays where it was.
    let mut restored = kept_world.tenants[&alpha].clone();
    std::sync::Arc::make_mut(&mut restored).event_seq = passed_seq;
    pair.world.tenants.insert(alpha.clone(), restored);
    pair.agree("alpha");
    assert_eq!(
        canon::hash_of(&store.load_complete(&tenant_id("beta")).unwrap().tenant),
        beta_before
    );
    assert_eq!(
        store
            .events_after(&alpha, 0, 1_000)
            .unwrap()
            .last()
            .unwrap()
            .seq,
        passed_seq,
        "no event sequence number is taken back"
    );
    // Life goes on from the restored state.
    pair.note("alpha", "After", "after");
}

/// The tenant ledger (LDG-009, LDG-019, LDG-022, LDG-027, LDG-028): postings are stored by the
/// commit that decided them and read back as the kernel holds them, whole and through each
/// ledger need; erasure clears the principal and keeps every amount and balance; a snapshot older
/// than an entry is not restored.
fn the_ledger_is_kept_with_its_commit_and_never_rewritten(store: &dyn Store) {
    let mut pair = Pair::new(store);
    let alpha = tenant_id("alpha");
    let owner = user("alpha:owner");
    let unit = LedgerUnit::new("points").unwrap();
    pair.ok(
        "alpha",
        owner.clone(),
        Intent::ChangeSchema {
            changes: vec![
                SchemaChange::DeclareLedgerUnit(LedgerUnitDecl {
                    unit: unit.clone(),
                    description: "Points".into(),
                    expiry: None,
                }),
                SchemaChange::SetLedgerLimit(LedgerLimit {
                    unit: unit.clone(),
                    owner: None,
                    floor: Some(LedgerAmount(0)),
                    cap: None,
                    period_cap: None,
                    period: None,
                    scope: None,
                }),
            ],
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    let info = store
        .create_snapshot(&alpha, "before the ledger", Timestamp(42))
        .unwrap();
    let writer = LedgerOwner::Principal {
        principal: principal_id("alpha:writer1"),
    };
    let posting = |amount: i64, key: &str| LedgerPostingRequest {
        owner: writer.clone(),
        unit: unit.clone(),
        amount: LedgerAmount(amount),
        idempotency_key: key.into(),
        reason: "conformance".into(),
        corrects: None,
        scope: None,
    };
    for (amount, key) in [(10, "a"), (5, "b"), (-3, "c")] {
        pair.ok(
            "alpha",
            owner.clone(),
            Intent::PostLedger {
                postings: vec![posting(amount, key)],
            },
        );
    }
    // Below the floor: refused by both, nothing stored.
    assert!(pair
        .run(
            "alpha",
            owner.clone(),
            Intent::PostLedger {
                postings: vec![posting(-100, "d")],
            },
        )
        .is_err());

    let account = ledger::account_id(&alpha, &writer, &unit, "");
    let key = ledger::account_key(&account);
    let partial = store
        .load(
            &alpha,
            &[
                Need::Ledger(LedgerNeed::Account { account }),
                Need::Ledger(LedgerNeed::EntryByKey {
                    account,
                    key: "b".into(),
                }),
            ],
        )
        .unwrap();
    let held = partial.tenant.as_ref().unwrap().ledger.as_ref().unwrap();
    assert_eq!(held.accounts[&key].balance, LedgerAmount(12));
    assert_eq!(held.accounts[&key].last_sequence, 3);
    let entries = &held.entries[&key];
    assert_eq!(entries.len(), 1, "only the entry of the key is loaded");
    assert_eq!(entries[0].sequence, 2);
    assert_eq!(entries[0].caused_by, Some(principal_id("alpha:owner")));

    let page = store
        .load(
            &alpha,
            &[Need::Ledger(LedgerNeed::Entries {
                account,
                after: Some(1),
                limit: 5,
            })],
        )
        .unwrap();
    let numbers: Vec<u64> = page
        .tenant
        .as_ref()
        .unwrap()
        .ledger
        .as_ref()
        .unwrap()
        .entries[&key]
        .iter()
        .map(|e| e.sequence)
        .collect();
    assert_eq!(numbers, [2, 3]);
    assert!(
        store
            .load(
                &tenant_id("beta"),
                &[Need::Ledger(LedgerNeed::Account { account })]
            )
            .unwrap()
            .tenant
            .unwrap()
            .ledger
            .is_none(),
        "another tenant holds nothing of the account"
    );

    // Erasure clears the principal from the entries it caused and keeps the rest (LDG-028).
    let cleared = store
        .anonymise_ledger_principal(&alpha, &principal_id("alpha:owner"))
        .unwrap();
    assert_eq!(cleared.entries, 3);
    assert_eq!(cleared.accounts, 0);
    let cleared = store
        .anonymise_ledger_principal(&alpha, &principal_id("alpha:writer1"))
        .unwrap();
    assert_eq!(cleared.accounts, 1);
    let whole = store.load_complete(&alpha).unwrap();
    let ledger_after = whole.tenant.as_ref().unwrap().ledger.as_ref().unwrap();
    assert_eq!(ledger_after.accounts[&key].owner, None);
    assert_eq!(ledger_after.accounts[&key].balance, LedgerAmount(12));
    let amounts: Vec<i64> = ledger_after.entries[&key]
        .iter()
        .map(|e| e.amount.0)
        .collect();
    assert_eq!(amounts, [10, 5, -3]);
    assert!(ledger_after.entries[&key]
        .iter()
        .all(|e| e.caused_by.is_none()));
    assert_eq!(
        ledger::replayed_balance(&ledger_after.entries[&key]),
        Ok(12),
        "the balance is the sum of the entries"
    );

    // Entries are never removed, so a snapshot older than one is not restored.
    assert!(matches!(
        store.restore_snapshot(&alpha, info.id),
        Err(StoreError::Invalid(_))
    ));
    assert_eq!(
        store
            .load_complete(&alpha)
            .unwrap()
            .tenant
            .unwrap()
            .ledger
            .clone(),
        whole.tenant.unwrap().ledger.clone()
    );
}

fn reset_removes_everything(store: &dyn Store) {
    let mut pair = Pair::new(store);
    pair.note("alpha", "Gone", "gone");
    pair.note("beta", "Kept", "kept");
    store
        .put_session(&session("s1", "alpha", "alpha:editor", 5_000))
        .unwrap();
    store
        .put_session(&session("s2", "beta", "beta:editor", 5_000))
        .unwrap();
    let account = |tenant: &str| Account {
        tenant: tenant_id(tenant),
        login: "editor".into(),
        principal: principal_id(&format!("{tenant}:editor")),
        display_name: "Editor".into(),
        verified_email: None,
        password_hash: "$argon2id$placeholder".into(),
    };
    store.put_account(&account("alpha")).unwrap();
    store.put_account(&account("beta")).unwrap();
    let beta = tenant_id("beta");
    let beta_before = (
        canon::hash_of(&store.load_complete(&beta).unwrap().tenant),
        store.events_after(&beta, 0, 1_000).unwrap(),
        store.journal_after(&beta, 0, 1_000).unwrap(),
    );

    // One tenant reset: it is gone with everything that was its own, the other is as it was.
    store.reset_tenant(&tenant_id("alpha")).unwrap();
    assert!(store
        .load(&tenant_id("alpha"), &[])
        .unwrap()
        .tenant
        .is_none());
    assert!(store
        .events_after(&tenant_id("alpha"), 0, 1_000)
        .unwrap()
        .is_empty());
    assert!(store
        .journal_after(&tenant_id("alpha"), 0, 1_000)
        .unwrap()
        .is_empty());
    assert!(store.session_by_token("hash-of-s1").unwrap().is_none());
    assert_eq!(store.account(&tenant_id("alpha"), "editor").unwrap(), None);
    assert!(store
        .outbox(1_000)
        .unwrap()
        .iter()
        .all(|entry| entry.event.tenant == beta));
    assert_eq!(
        (
            canon::hash_of(&store.load_complete(&beta).unwrap().tenant),
            store.events_after(&beta, 0, 1_000).unwrap(),
            store.journal_after(&beta, 0, 1_000).unwrap(),
        ),
        beta_before
    );
    assert!(store.session_by_token("hash-of-s2").unwrap().is_some());
    assert_eq!(
        store.account(&beta, "editor").unwrap(),
        Some(account("beta"))
    );

    store.reset().unwrap();
    assert!(store.tenants().unwrap().is_empty());
    assert!(store.outbox(10).unwrap().is_empty());
    assert!(store.session_by_token("hash-of-s1").unwrap().is_none());
    assert!(store
        .load(&tenant_id("alpha"), &[])
        .unwrap()
        .tenant
        .is_none());
    Pair::new(store);
}

/// The pair index of named queries (KRN-2920, KRN-2921): a store answers a pair page (the records
/// of one value of a field, in the order of another attribute and then the record id, after a
/// position) exactly as the kernel defines it ([`strata_kernel::query_plan::pair_page`]), before
/// any query reads the pair, once one is defined over existing records, after records are created,
/// updated in either field and deleted, and after the query is redefined and removed; a read of
/// the query through the store returns what the kernel returns from the complete state; and one
/// tenant's records never answer for another.
///
/// Not a row of the conformance table: the adapters' tests call it by name, as they call
/// [`documents_are_scoped_to_their_tenant_and_kind`].
pub fn pair_pages_follow_writes_and_definitions(store: &dyn Store) {
    use strata_kernel::query_plan::{order_value, pair_page, IndexPosition};
    let mut pair = Pair::new(store);
    let alpha = tenant_id("alpha");
    let writer = || user("alpha:writer1");
    let owner = || Principal::User(principal_id("alpha:owner"));
    let mut ids = Vec::new();
    let notes = [
        ("A", Some(3)),
        ("B", None),
        ("A", Some(1)),
        ("A", None),
        ("B", Some(2)),
        ("A", Some(3)),
    ];
    for (i, (title, rank)) in notes.iter().enumerate() {
        let id = pair.note("alpha", title, &format!("s{i}"));
        if let Some(rank) = rank {
            pair.ok(
                "alpha",
                writer(),
                Intent::UpdateRecord {
                    object: object("note"),
                    id,
                    fields: BTreeMap::from([(field("rank"), Value::Int(*rank))]),
                    expected_version: None,
                },
            );
        }
        ids.push(id);
    }
    pair.note("beta", "A", "s0");
    let by_title = |order: &str, descending: bool| QueryDef {
        id: QueryId::new("by_title").unwrap(),
        object: object("note"),
        filter: Some(eq(var(Scope::Record, "title"), Expr::Param("t".into()))),
        params: std::collections::BTreeSet::from(["t".to_string()]),
        order: vec![OrderKey {
            field: order.into(),
            descending,
        }],
        limit: None,
        aggregates: None,
        declared_fields: None,
    };
    let change = |pair: &mut Pair, changes: Vec<SchemaChange>| {
        pair.ok(
            "alpha",
            owner(),
            Intent::ChangeSchema {
                changes,
                expected_version: None,
                existing: ExistingData::Reject,
            },
        );
    };
    // Every pair page of the titles, from the start and after each record's position, as the
    // kernel defines it over the store's complete state.
    let agree = |stage: &str| {
        let all = store.load_complete(&alpha).unwrap().tenant.unwrap();
        let notes_of = || all.records_of(&object("note"));
        for title in ["A", "B", "C"] {
            for order in ["rank", "$created_at", "$updated_at"] {
                for descending in [false, true] {
                    let mut positions = vec![None];
                    positions.extend(
                        pair_page(
                            notes_of(),
                            &field("title"),
                            &text(title),
                            order,
                            descending,
                            None,
                            10,
                        )
                        .into_iter()
                        .map(|r| {
                            Some(IndexPosition {
                                value: order_value(r, order),
                                id: r.id,
                            })
                        }),
                    );
                    for after in positions {
                        for limit in [1, 2, 10] {
                            let mut expected: Vec<RecordId> = pair_page(
                                notes_of(),
                                &field("title"),
                                &text(title),
                                order,
                                descending,
                                after.as_ref(),
                                limit,
                            )
                            .into_iter()
                            .map(|r| r.id)
                            .collect();
                            expected.sort();
                            let need = Need::PairPage {
                                object: object("note"),
                                field: field("title"),
                                value: text(title),
                                order: order.to_string(),
                                descending,
                                after: after.clone(),
                                limit,
                            };
                            let loaded: Vec<RecordId> = store
                                .load(&alpha, &[need.clone()])
                                .unwrap()
                                .tenant
                                .unwrap()
                                .records_of(&object("note"))
                                .map(|r| r.id)
                                .collect();
                            assert_eq!(loaded, expected, "{stage}: {need:?}");
                        }
                    }
                }
            }
        }
    };
    let reads_agree = |stage: &str| {
        let complete = store.load_complete(&alpha).unwrap();
        for reader in [writer(), user("alpha:editor"), Principal::Anonymous] {
            let context = ctx("alpha", reader, 9_000);
            for title in ["A", "B"] {
                let request = ReadRequest::Named {
                    query: QueryId::new("by_title").unwrap(),
                    params: BTreeMap::from([("t".to_string(), text(title))]),
                    page_size: Some(2),
                    cursor: None,
                };
                assert_eq!(
                    query(store, &request, &context).unwrap(),
                    read(&complete, &request, &context).unwrap(),
                    "{stage}: by_title {title}"
                );
            }
        }
    };
    agree("before any query reads the pairs");
    // Defined over existing records.
    change(
        &mut pair,
        vec![SchemaChange::DefineQuery(by_title("rank", true))],
    );
    agree("defined");
    reads_agree("defined");
    // Created, and updated in either field.
    let more = pair.note("alpha", "B", "s9");
    pair.ok(
        "alpha",
        writer(),
        Intent::UpdateRecord {
            object: object("note"),
            id: ids[0],
            fields: BTreeMap::from([(field("title"), text("B"))]),
            expected_version: None,
        },
    );
    pair.ok(
        "alpha",
        writer(),
        Intent::UpdateRecord {
            object: object("note"),
            id: more,
            fields: BTreeMap::from([(field("rank"), Value::Int(7))]),
            expected_version: None,
        },
    );
    agree("updated");
    reads_agree("updated");
    pair.ok(
        "alpha",
        writer(),
        Intent::DeleteRecord {
            object: object("note"),
            id: ids[2],
            expected_version: None,
        },
    );
    agree("deleted");
    reads_agree("deleted");
    // Redefined with another order, then removed.
    change(
        &mut pair,
        vec![SchemaChange::DefineQuery(by_title("$created_at", false))],
    );
    agree("redefined");
    reads_agree("redefined");
    change(
        &mut pair,
        vec![SchemaChange::RemoveQuery(QueryId::new("by_title").unwrap())],
    );
    agree("removed");
    // The other tenant's note of the same title is never in alpha's pages.
    let need = Need::PairPage {
        object: object("note"),
        field: field("title"),
        value: text("A"),
        order: "$created_at".into(),
        descending: false,
        after: None,
        limit: 50,
    };
    let beta = store
        .load_complete(&tenant_id("beta"))
        .unwrap()
        .tenant
        .unwrap();
    let theirs: Vec<RecordId> = beta.records_of(&object("note")).map(|r| r.id).collect();
    let loaded = store.load(&alpha, &[need]).unwrap().tenant.unwrap();
    let all = store.load_complete(&alpha).unwrap().tenant.unwrap();
    for record in loaded.records_of(&object("note")) {
        assert_eq!(all.record(&object("note"), record.id), Some(record));
    }
    assert!(!theirs.is_empty());
}

/// The jobs of the automation worker (`crate::jobs`): created once per key, alone or with the
/// commit that causes them and never by one that is refused; offered no earlier than they are
/// due and only to their tenant; leased with a fencing token that refuses a superseded report;
/// ended in exactly one outcome; retried with a growing delay up to the attempts of their policy
/// and then failed with every error; cancelled for good; found through the wake index; and the
/// run, step and exception records written with them, listed and settled.
///
/// Not a row of the conformance table: the adapters' tests call it by name, as they call
/// [`documents_are_scoped_to_their_tenant_and_kind`].
// Verifies: SCH-001, SCH-004, SCH-006, SCH-010, SCH-011, SCH-012, SCH-013, SCH-014
pub fn jobs_conform(store: &dyn Store) {
    use crate::jobs::{
        ExceptionKind, ExceptionRecord, JobChange, JobReport, JobState, JobWork, NewJob, Reported,
        RetryPolicy, RunRecord, RunState, Settled, StepRecord, StepState, WorkerWrites,
    };
    Pair::new(store);
    let alpha = tenant_id("alpha");
    let beta = tenant_id("beta");
    let automation = WorkflowId::new("award").expect("id");
    let job = |key: &str, due: i64| NewJob {
        key: key.to_string(),
        work: JobWork::Step {
            run: "run-1".into(),
            automation: automation.clone(),
            step: StepName::new("first").expect("name"),
            sequence: 1,
        },
        due_at: Timestamp(due),
        policy: RetryPolicy {
            attempts: 2,
            first_delay_ms: 100,
            max_delay_ms: 1_000,
            lease_ms: 50,
        },
    };
    let none = WorkerWrites::default();
    let recorded = |reported: Reported| match reported {
        Reported::Recorded(job) => job,
        Reported::Refused(why) => panic!("the report was refused: {why}"),
    };

    // Created once for its key, in its own tenant only.
    let ids = store
        .jobs_create(
            &alpha,
            &[job("a", 5_000), job("a", 5_000)],
            Timestamp(4_000),
        )
        .unwrap();
    assert_eq!(ids[0], ids[1], "one key, one job");
    assert_eq!(store.jobs_list(&alpha, None, 10).unwrap().len(), 1);
    assert!(store.jobs_list(&beta, None, 10).unwrap().is_empty());
    assert!(matches!(
        store.jobs_create(&tenant_id("gamma"), &[job("a", 5_000)], Timestamp(4_000)),
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        store.jobs_create(&alpha, &[job("", 5_000)], Timestamp(4_000)),
        Err(StoreError::Invalid(_))
    ));

    // Offered no earlier than it is due (SCH-001), to its own tenant, and found by the wake
    // index then and not before.
    assert!(store
        .job_lease(&alpha, "w1", Timestamp(4_999))
        .unwrap()
        .is_none());
    assert!(store.jobs_due(Timestamp(4_999), 10).unwrap().is_empty());
    assert_eq!(
        store.jobs_due(Timestamp(5_000), 10).unwrap(),
        vec![alpha.clone()]
    );
    assert!(store
        .job_lease(&beta, "w1", Timestamp(5_000))
        .unwrap()
        .is_none());
    let first = store
        .job_lease(&alpha, "w1", Timestamp(5_000))
        .unwrap()
        .expect("the job is due");
    assert_eq!(first.job.id, ids[0]);
    assert_eq!(first.expires_at, Timestamp(5_050));
    assert!(!first.exhausted);
    assert!(
        store
            .job_lease(&alpha, "w2", Timestamp(5_049))
            .unwrap()
            .is_none(),
        "held while the lease lasts"
    );

    // The lease expires and another worker takes the job (SCH-010); the first worker's late
    // report is refused by the token and writes nothing (SCH-011).
    let second = store
        .job_lease(&alpha, "w2", Timestamp(5_051))
        .unwrap()
        .expect("the lease expired");
    assert!(second.token > first.token);
    let run = RunRecord {
        run: "run-1".into(),
        automation: automation.clone(),
        trigger: None,
        event_seq: Some(3),
        state: RunState::Running,
        step: Some(StepName::new("first").expect("name")),
        steps: 1,
        hop: 0,
        caused_by: principal_id("automation:award"),
        on_behalf_of: None,
        started_by: None,
        params: BTreeMap::new(),
        started_at: Timestamp(5_000),
        ended_at: None,
        error: None,
        seq: 0,
        definition_version: None,
        definition: None,
    };
    let step = StepRecord {
        run: "run-1".into(),
        sequence: 1,
        step: StepName::new("first").expect("name"),
        state: StepState::Completed,
        attempts: 2,
        job: Some(ids[0].clone()),
        idempotency_key: "k".into(),
        result: None,
        errors: Vec::new(),
        approval: None,
        started_at: Timestamp(5_000),
        ended_at: Some(Timestamp(5_060)),
    };
    let writes = WorkerWrites {
        runs: vec![run.clone()],
        steps: vec![step],
        exceptions: Vec::new(),
        jobs: vec![job("b", 6_000)],
        ..WorkerWrites::default()
    };
    assert!(matches!(
        store
            .job_report(
                &alpha,
                &ids[0],
                first.token,
                &JobReport::Succeeded,
                &writes,
                Timestamp(5_052)
            )
            .unwrap(),
        Reported::Refused(_)
    ));
    assert!(store.automation_run(&alpha, "run-1").unwrap().is_none());
    assert_eq!(store.jobs_list(&alpha, None, 10).unwrap().len(), 1);
    let done = recorded(
        store
            .job_report(
                &alpha,
                &ids[0],
                second.token,
                &JobReport::Succeeded,
                &writes,
                Timestamp(5_060),
            )
            .unwrap(),
    );
    assert_eq!(done.state, JobState::Completed);
    // Exactly one outcome (SCH-012): a second report is refused.
    assert!(matches!(
        store
            .job_report(
                &alpha,
                &ids[0],
                second.token,
                &JobReport::Succeeded,
                &none,
                Timestamp(5_061)
            )
            .unwrap(),
        Reported::Refused(_)
    ));
    assert_eq!(
        store
            .automation_run(&alpha, "run-1")
            .unwrap()
            .map(|r| r.state),
        Some(RunState::Running)
    );
    assert_eq!(
        store
            .automation_steps(&alpha, "run-1", None, 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .automation_runs(&alpha, Some(&automation), Some(RunState::Running), None, 10)
            .unwrap()
            .len(),
        1
    );
    assert!(store
        .automation_runs(&alpha, None, Some(RunState::Completed), None, 10)
        .unwrap()
        .is_empty());
    let listed = store.jobs_list(&alpha, None, 10).unwrap();
    assert_eq!(listed.len(), 2, "the job the report created");
    assert!(listed[0].seq > listed[1].seq, "newest first");
    assert_eq!(
        store
            .jobs_list(&alpha, Some(listed[0].seq), 10)
            .unwrap()
            .len(),
        1
    );

    // A failed attempt is retried after its delay (SCH-013); the last fails the job with every
    // error (SCH-014).
    let b = store
        .job_lease(&alpha, "w1", Timestamp(6_000))
        .unwrap()
        .expect("b is due");
    let failure = |error: &str| JobReport::Failed {
        error: error.to_string(),
        retry: true,
    };
    let waiting = recorded(
        store
            .job_report(
                &alpha,
                &b.job.id,
                b.token,
                &failure("one"),
                &none,
                Timestamp(6_001),
            )
            .unwrap(),
    );
    assert_eq!(waiting.state, JobState::Waiting);
    assert_eq!(waiting.due_at, Timestamp(6_101));
    assert!(store
        .job_lease(&alpha, "w1", Timestamp(6_100))
        .unwrap()
        .is_none());
    let b2 = store
        .job_lease(&alpha, "w1", Timestamp(6_101))
        .unwrap()
        .expect("retried");
    let failed = recorded(
        store
            .job_report(
                &alpha,
                &b.job.id,
                b2.token,
                &failure("two"),
                &none,
                Timestamp(6_102),
            )
            .unwrap(),
    );
    assert_eq!(failed.state, JobState::Failed);
    assert_eq!(failed.errors.len(), 2);
    assert_eq!(failed.reason.as_deref(), Some("attempts-exhausted"));
    assert!(store.jobs_due(Timestamp(9_000), 10).unwrap().is_empty());

    // A dead-lettered record is listed until it is settled; releasing it gives its job one more
    // attempt.
    let dead = ExceptionRecord {
        id: "x-1".into(),
        kind: ExceptionKind::DeadLettered,
        reason: "attempts-exhausted".into(),
        trigger: None,
        automation: Some(automation.clone()),
        run: Some("run-1".into()),
        step: None,
        event_seq: None,
        job: Some(b.job.id.clone()),
        hop: None,
        at: Timestamp(6_102),
        detail: String::new(),
        settled: false,
        seq: 0,
    };
    store
        .automation_write(
            &alpha,
            &WorkerWrites {
                exceptions: vec![dead.clone(), dead.clone()],
                ..WorkerWrites::default()
            },
            Timestamp(6_103),
        )
        .unwrap();
    let unsettled = store
        .automation_exceptions(&alpha, None, true, None, 10)
        .unwrap();
    assert_eq!(unsettled.len(), 1, "written once");
    assert_eq!(
        store
            .automation_exception_settle(&alpha, "x-1", true, Timestamp(7_000))
            .unwrap(),
        Settled::Done {
            job: Some(b.job.id.clone())
        }
    );
    assert!(matches!(
        store
            .automation_exception_settle(&alpha, "x-1", false, Timestamp(7_000))
            .unwrap(),
        Settled::Conflict(_)
    ));
    assert_eq!(
        store
            .automation_exception_settle(&alpha, "x-2", false, Timestamp(7_000))
            .unwrap(),
        Settled::NotFound
    );
    assert!(store
        .automation_exceptions(&alpha, None, true, None, 10)
        .unwrap()
        .is_empty());
    assert_eq!(
        store.job(&alpha, &b.job.id).unwrap().map(|j| j.state),
        Some(JobState::Waiting)
    );

    // Cancelling prevents every later attempt (SCH-006).
    assert!(matches!(
        store
            .job_cancel(&alpha, &b.job.id, "not wanted", Timestamp(7_001))
            .unwrap(),
        JobChange::Changed(_)
    ));
    assert!(matches!(
        store
            .job_retry(&alpha, &b.job.id, Timestamp(7_002))
            .unwrap(),
        JobChange::Unchanged(_)
    ));
    assert!(store
        .job_lease(&alpha, "w1", Timestamp(9_000))
        .unwrap()
        .is_none());
    assert!(store.jobs_due(Timestamp(9_000), 10).unwrap().is_empty());
    assert_eq!(
        store
            .job_cancel(&alpha, "no-such-job", "x", Timestamp(7_003))
            .unwrap(),
        JobChange::NotFound
    );

    // The jobs and exception records of a commit are written with it, and a commit that is
    // refused writes none (SCH-004).
    let state = store.load_complete(&alpha).unwrap();
    let intent = new_note("Jobbed", "jobbed");
    let context = ctx("alpha", user("alpha:writer1"), 10_000);
    let transition = strata_kernel::transition(&state, &intent, &context).expect("accepted");
    let mut commit = Commit::of(&intent, &context, &transition);
    commit.jobs.push(job("c", 10_000));
    commit.exceptions.push(ExceptionRecord {
        id: "x-3".into(),
        kind: ExceptionKind::Declined,
        reason: "hop-limit".into(),
        settled: true,
        ..dead.clone()
    });
    store.commit(&commit).unwrap();
    let mut stale = commit.clone();
    stale.jobs = vec![job("d", 10_000)];
    stale.exceptions[0].id = "x-4".into();
    assert!(matches!(
        store.commit(&stale),
        Err(StoreError::Conflict { .. })
    ));
    let keys: Vec<String> = store
        .jobs_list(&alpha, None, 10)
        .unwrap()
        .into_iter()
        .map(|j| j.key)
        .collect();
    assert!(keys.contains(&"c".to_string()), "{keys:?}");
    assert!(!keys.contains(&"d".to_string()), "{keys:?}");
    let declined = store
        .automation_exceptions(&alpha, Some(ExceptionKind::Declined), false, None, 10)
        .unwrap();
    assert_eq!(declined.len(), 1);
    assert_eq!(declined[0].id, "x-3");
    assert!(matches!(
        store
            .automation_exception_settle(&alpha, "x-3", false, Timestamp(10_001))
            .unwrap(),
        Settled::Conflict(_)
    ));
    assert_eq!(store.jobs_due(Timestamp(10_000), 10).unwrap(), vec![alpha]);
}

/// A check: the case of the conformance table it establishes (`specifications/spec.jsonc`,
/// section `cases-storage-conformance`), its name, and the check itself.
type Check = (&'static str, &'static str, fn(&dyn Store));

/// Every check, by case and name. `run` returns the cases it ran, and the adapters' tests compare
/// them with the rows of the specification's table, so a row without a check or a check without
/// a row fails.
pub const CHECKS: &[Check] = &[
    (
        "absent-tenant",
        "absent_tenant_loads_as_absent",
        absent_tenant_loads_as_absent,
    ),
    (
        "state-equals-kernel",
        "stored_state_equals_kernel_state",
        stored_state_equals_kernel_state,
    ),
    (
        "stale-commit",
        "a_stale_commit_conflicts_and_changes_nothing",
        a_stale_commit_conflicts_and_changes_nothing,
    ),
    (
        "malformed-commit",
        "a_malformed_commit_is_refused_whole",
        a_malformed_commit_is_refused_whole,
    ),
    (
        "partial-load",
        "partial_loads_hold_what_the_needs_name",
        partial_loads_hold_what_the_needs_name,
    ),
    (
        "list-containment",
        "list_fields_answer_field_lookups_by_containment",
        list_fields_answer_field_lookups_by_containment,
    ),
    (
        "tenant-isolation",
        "tenants_are_isolated",
        tenants_are_isolated,
    ),
    (
        "outbox-order",
        "events_are_ordered_and_stay_in_the_outbox_until_published",
        events_are_ordered_and_stay_in_the_outbox_until_published,
    ),
    (
        "publish-after-commit",
        "events_are_published_only_after_their_commit",
        events_are_published_only_after_their_commit,
    ),
    (
        "journal-replay",
        "the_journal_replays_to_the_stored_state",
        the_journal_replays_to_the_stored_state,
    ),
    (
        "sessions",
        "sessions_are_stored_revoked_and_purged",
        sessions_are_stored_revoked_and_purged,
    ),
    (
        "accounts",
        "accounts_are_scoped_to_their_tenant",
        accounts_are_scoped_to_their_tenant,
    ),
    (
        "snapshots",
        "snapshots_restore_to_the_same_hash",
        snapshots_restore_to_the_same_hash,
    ),
    (
        "idempotency",
        "idempotency_keys_return_the_first_result",
        idempotency_keys_return_the_first_result,
    ),
    (
        "reset",
        "reset_removes_everything",
        reset_removes_everything,
    ),
    (
        "ledger",
        "the_ledger_is_kept_with_its_commit_and_never_rewritten",
        the_ledger_is_kept_with_its_commit_and_never_rewritten,
    ),
];

/// Run every check against a fresh, empty store from `make`. Returns the cases it ran.
pub fn run(make: &dyn Fn() -> Box<dyn Store>) -> Vec<&'static str> {
    let mut ran = Vec::new();
    for (case, name, check) in CHECKS {
        eprintln!("conformance: {case} ({name})");
        let store = make();
        check(store.as_ref());
        ran.push(*case);
    }
    ran
}

/// The cases of the conformance table in the text of the specification file: the first column of
/// the table in the `cases-storage-conformance` section, in order.
pub fn spec_cases(spec: &str) -> Vec<String> {
    let line = spec
        .lines()
        .find(|line| {
            line.trim_start()
                .starts_with("{\"id\":\"cases-storage-conformance\"")
        })
        .expect("the specification has a cases-storage-conformance section");
    let section: serde_json::Value =
        serde_json::from_str(line.trim().trim_end_matches(',')).expect("the section is JSON");
    section["body"]
        .as_str()
        .expect("the section has a body")
        .lines()
        .filter(|row| row.starts_with('|'))
        .skip(2) // the header and its rule
        .filter_map(|row| row.trim_matches('|').split('|').next())
        .map(|cell| cell.trim().to_string())
        .collect()
}

/// Schedules (`crate::schedules`, SCH-003, SCH-005): a defined schedule has its first job; the
/// completion of a job creates the next one in its own transaction, once however often the
/// completion is repeated; a paused schedule creates no job and resuming it creates one only when
/// none is open; a changed rule replaces the open job; and a schedule belongs to its tenant.
///
/// Not a row of the conformance table: the adapters' tests call it by name, as they call
/// [`jobs_conform`].
// Verifies: SCH-003, SCH-005
pub fn schedules_conform(store: &dyn Store) {
    use crate::jobs::{JobReport, JobState, Reported, WorkerWrites};
    use crate::schedules::{job_key, ScheduleAdvance, ScheduleSet};
    Pair::new(store);
    let alpha = tenant_id("alpha");
    let beta = tenant_id("beta");
    let id = "0b1f6c3e-52a4-4c55-9a77-0d9a4c7e6b10";
    let set = |rule: &str, paused: bool, due: i64| ScheduleSet {
        id: id.to_string(),
        rule: rule.to_string(),
        paused,
        anchor: Timestamp(1_000),
        due: Some(Timestamp(due)),
    };
    let job_keys = |tenant: &TenantId| -> Vec<(String, JobState)> {
        let mut jobs = store.jobs_list(tenant, None, 50).unwrap();
        jobs.sort_by_key(|job| job.seq);
        jobs.into_iter().map(|job| (job.key, job.state)).collect()
    };
    // Lease the one job that is due at `due` and report it, with the next occurrence of the
    // schedule at `next`.
    let complete = |due: i64, next: Option<i64>| -> Reported {
        let lease = store
            .job_lease(&alpha, "w1", Timestamp(due))
            .unwrap()
            .expect("the job of the schedule is due");
        let writes = WorkerWrites {
            schedule_advance: Some(ScheduleAdvance {
                schedule: id.to_string(),
                next_due: next.map(Timestamp),
            }),
            ..WorkerWrites::default()
        };
        store
            .job_report(
                &alpha,
                &lease.job.id,
                lease.token,
                &JobReport::Succeeded,
                &writes,
                Timestamp(due + 1),
            )
            .unwrap()
    };

    // A new schedule has its first job, once: defining it again changes nothing.
    let made = store
        .schedule_set(&alpha, &set("FREQ=DAILY", false, 5_000), Timestamp(2_000))
        .unwrap();
    assert_eq!((made.created, made.completions, made.paused), (1, 0, false));
    store
        .schedule_set(&alpha, &set("FREQ=DAILY", false, 5_000), Timestamp(2_001))
        .unwrap();
    assert_eq!(
        job_keys(&alpha),
        vec![(job_key(id, 1), JobState::Waiting)],
        "one job"
    );
    assert!(store.schedule(&beta, id).unwrap().is_none());
    assert!(job_keys(&beta).is_empty());
    assert!(matches!(
        store.schedule_set(
            &tenant_id("gamma"),
            &set("FREQ=DAILY", false, 5_000),
            Timestamp(2_000)
        ),
        Err(StoreError::NotFound(_))
    ));

    // The completion creates the next job in its own transaction (SCH-003). A repeat of the
    // report is refused and creates nothing more.
    let lease = store
        .job_lease(&alpha, "w1", Timestamp(5_000))
        .unwrap()
        .expect("due");
    let writes = WorkerWrites {
        schedule_advance: Some(ScheduleAdvance {
            schedule: id.to_string(),
            next_due: Some(Timestamp(6_000)),
        }),
        ..WorkerWrites::default()
    };
    let report = |token: u64, now: i64| {
        store
            .job_report(
                &alpha,
                &lease.job.id,
                token,
                &JobReport::Succeeded,
                &writes,
                Timestamp(now),
            )
            .unwrap()
    };
    assert!(matches!(report(lease.token, 5_001), Reported::Recorded(_)));
    assert!(matches!(report(lease.token, 5_002), Reported::Refused(_)));
    assert_eq!(
        job_keys(&alpha),
        vec![
            (job_key(id, 1), JobState::Completed),
            (job_key(id, 2), JobState::Waiting)
        ]
    );
    let held = store.schedule(&alpha, id).unwrap().expect("the schedule");
    assert_eq!(
        (held.created, held.completions, held.next_due),
        (2, 1, Timestamp(6_000))
    );
    assert_eq!(store.schedules(&alpha).unwrap().len(), 1);

    // Paused: the open job completes and nothing follows (SCH-005).
    let paused = store
        .schedule_set(&alpha, &set("FREQ=DAILY", true, 6_000), Timestamp(6_000))
        .unwrap();
    assert!(paused.paused);
    assert_eq!(paused.created_at_pause, 2);
    assert_eq!(job_keys(&alpha).len(), 2, "pausing creates no job");
    assert!(matches!(
        complete(6_000, Some(7_000)),
        Reported::Recorded(_)
    ));
    assert_eq!(
        job_keys(&alpha).len(),
        2,
        "a paused schedule creates no job"
    );
    let after = store.schedule(&alpha, id).unwrap().expect("the schedule");
    assert_eq!((after.created, after.completions), (2, 2));

    // Resumed with no job open: the next job is created.
    store
        .schedule_set(&alpha, &set("FREQ=DAILY", false, 9_000), Timestamp(8_000))
        .unwrap();
    assert_eq!(
        job_keys(&alpha).last().cloned(),
        Some((job_key(id, 3), JobState::Waiting))
    );
    // Defined again with a job open: none is created.
    store
        .schedule_set(&alpha, &set("FREQ=DAILY", false, 9_000), Timestamp(8_001))
        .unwrap();
    assert_eq!(job_keys(&alpha).len(), 3);

    // A changed rule replaces the open job.
    store
        .schedule_set(&alpha, &set("FREQ=HOURLY", false, 9_500), Timestamp(8_002))
        .unwrap();
    assert_eq!(
        job_keys(&alpha)
            .iter()
            .rev()
            .take(2)
            .cloned()
            .collect::<Vec<_>>(),
        vec![
            (job_key(id, 4), JobState::Waiting),
            (job_key(id, 3), JobState::Cancelled)
        ]
    );
    let changed = store.schedule(&alpha, id).unwrap().expect("the schedule");
    assert_eq!(changed.rule, "FREQ=HOURLY");
    assert_eq!((changed.created, changed.completions), (4, 3));

    // The last occurrence of a rule: the completion creates no job.
    assert!(matches!(complete(9_500, None), Reported::Recorded(_)));
    assert_eq!(job_keys(&alpha).len(), 4);
}

/// The retention of an idempotency entry is a property of the entry (WG-5164, WG-5145): an entry
/// that states seven days is found for seven days and one that states nothing for twenty-four
/// hours, a key is a different key under another scope (another operation of the same person),
/// and the entry of an automation principal is kept for the span of a step's retries and the
/// retention of its run.
///
/// Not a row of the conformance table: the adapters' tests call it by name.
// Verifies: SCH-1524, SCH-2701, SCH-2702
pub fn idempotency_retention_is_a_property_of_the_entry(store: &dyn Store) {
    use crate::{
        idempotency_retention, idempotency_scope_for, AUTOMATION_STEP_RETENTION_MS,
        OPERATION_RETENTION_MS,
    };
    let _pair = Pair::new(store);
    let alpha = tenant_id("alpha");
    let writer = user("alpha:writer1");
    let scope = idempotency_scope(&writer);
    let day = 24 * 60 * 60 * 1_000_i64;
    let t0 = 10_000_i64;

    // The retention a request states: operations seven days, automation steps longer than the
    // longest retry of a step, anything else a day.
    let note = new_note("Stated", "stated");
    assert_eq!(
        idempotency_retention(&note, &ctx("alpha", writer.clone(), t0)),
        IDEMPOTENCY_RETENTION_MS
    );
    assert_eq!(
        idempotency_retention(&note, &ctx("alpha", user("automation:award"), t0)),
        AUTOMATION_STEP_RETENTION_MS
    );
    assert!(AUTOMATION_STEP_RETENTION_MS > 9 * day + 90 * day);
    assert_eq!(OPERATION_RETENTION_MS, 7 * day);
    // An invocation of an operation states seven days, and its key belongs to the operation.
    let invoke = |operation: &str| Intent::InvokeOperation {
        operation: TenantOperationId::new(operation).expect("id"),
        inputs: BTreeMap::new(),
        idempotency_key: "k".into(),
    };
    let context = ctx("alpha", writer.clone(), t0);
    assert_eq!(
        idempotency_retention(&invoke("one"), &context),
        OPERATION_RETENTION_MS
    );
    assert_ne!(
        idempotency_scope_for(&invoke("one"), &context),
        idempotency_scope_for(&invoke("two"), &context)
    );
    assert_eq!(idempotency_scope_for(&note, &context), scope);

    // One entry of the default retention, one of seven days, and two of seven days under other
    // scopes with one key, all recorded at about t0.
    let mut keyed = ctx("alpha", writer.clone(), t0);
    keyed.idempotency_key = Some("k-default".into());
    execute(store, &new_note("Default", "default"), &keyed).expect("the first request commits");
    let commit_with = |key: &str, scope: String, retention_ms: i64, slug: &str| {
        let state = store.load_complete(&alpha).unwrap();
        let intent = new_note(slug, slug);
        let mut context = ctx("alpha", writer.clone(), t0 + 1);
        context.idempotency_key = Some(key.to_string());
        let transition = strata_kernel::transition(&state, &intent, &context).expect("accepted");
        let mut commit = Commit::of(&intent, &context, &transition);
        let entry = commit.idempotency.as_mut().expect("an entry");
        entry.retention_ms = retention_ms;
        entry.principal = scope;
        store.commit(&commit).expect("the commit applies");
    };
    commit_with("k-week", scope.clone(), 7 * day, "week");
    commit_with("k-same", format!("{scope}|op:one"), 7 * day, "one");
    commit_with("k-same", format!("{scope}|op:two"), 7 * day, "two");

    let found = |scope: &str, key: &str, at: i64| {
        store
            .idempotent_result(&alpha, scope, key, Timestamp(at))
            .unwrap()
            .is_some()
    };
    // A day and an hour later the default entry is gone and the week's is not.
    assert!(found(&scope, "k-default", t0 + day));
    assert!(!found(&scope, "k-default", t0 + day + 1));
    assert!(found(&scope, "k-week", t0 + day + 3_600_000));
    assert!(found(&scope, "k-week", t0 + 6 * day));
    // A repeat of the request six days later answers the recorded result and changes nothing.
    let mut repeat = ctx("alpha", writer.clone(), t0 + 6 * day);
    repeat.idempotency_key = Some("k-week".into());
    let before = store
        .load_complete(&alpha)
        .unwrap()
        .tenant
        .map(|t| t.revision);
    let replayed = execute(store, &new_note("week", "week"), &repeat).expect("answered");
    assert!(replayed.replayed, "the repeat was committed again");
    assert_eq!(
        store
            .load_complete(&alpha)
            .unwrap()
            .tenant
            .map(|t| t.revision),
        before
    );
    // Seven days after its recording the entry is still found; one millisecond later it is not.
    assert!(found(&scope, "k-week", t0 + 1 + 7 * day));
    assert!(!found(&scope, "k-week", t0 + 2 + 7 * day));
    // The same key under two scopes is two entries, and a third scope has none.
    assert!(found(&format!("{scope}|op:one"), "k-same", t0 + 6 * day));
    assert!(found(&format!("{scope}|op:two"), "k-same", t0 + 6 * day));
    assert!(!found(&format!("{scope}|op:three"), "k-same", t0 + 6 * day));

    // A commit that names a scope recorded and not past its retention fails; past it, it applies.
    let state = store.load_complete(&alpha).unwrap();
    let again = new_note("Again", "again");
    let mut inside = ctx("alpha", writer.clone(), t0 + 3 * day);
    inside.idempotency_key = Some("k-week".into());
    let transition = strata_kernel::transition(&state, &again, &inside).expect("accepted");
    assert!(store
        .commit(&Commit::of(&again, &inside, &transition))
        .is_err());
    let mut late = ctx("alpha", writer, t0 + 8 * day);
    late.idempotency_key = Some("k-week".into());
    let transition = strata_kernel::transition(&state, &again, &late).expect("accepted");
    store
        .commit(&Commit::of(&again, &late, &transition))
        .expect("a key past its retention is new");
}

/// The run keys of automations (rec-automation-run-key, D-6534, D-6539): a key is held by the run
/// that claimed it first, whatever claims it later; the start jobs of one key that have not ended
/// are listed with their event sequence numbers; releasing an automation's keys frees them.
///
/// Not a row of the conformance table: the adapters' tests call it by name.
// Verifies: SCH-1536
pub fn run_keys_conform(store: &dyn Store) {
    use crate::jobs::{JobReport, JobWork, NewJob, RetryPolicy, RunKeyClaim, WorkerWrites};
    Pair::new(store);
    let alpha = tenant_id("alpha");
    let beta = tenant_id("beta");
    let award = WorkflowId::new("award").expect("id");
    let other = WorkflowId::new("other").expect("id");
    let start = |key: &str, seq: u64, run_key: &str| NewJob {
        key: key.to_string(),
        work: JobWork::StartRun {
            trigger: None,
            automation: award.clone(),
            event_seq: Some(seq),
            hop: 0,
            params: BTreeMap::new(),
            caused_by: None,
            on_behalf_of: None,
            definition_version: None,
            run_key: Some(run_key.to_string()),
        },
        due_at: Timestamp(5_000 + seq as i64),
        policy: RetryPolicy::default(),
    };
    let ids = store
        .jobs_create(
            &alpha,
            &[
                start("s1", 1, "h1"),
                start("s2", 2, "h1"),
                start("s3", 3, "h2"),
            ],
            Timestamp(4_000),
        )
        .unwrap();
    let open_of = |tenant: &TenantId, automation: &WorkflowId| -> Vec<String> {
        let mut open: Vec<(u64, String)> = store
            .open_run_key_starts(tenant, automation, "h1")
            .unwrap()
            .into_iter()
            .map(|job| match job.work {
                JobWork::StartRun { event_seq, .. } => (event_seq.unwrap_or(0), job.id),
                other => panic!("{other:?}"),
            })
            .collect();
        open.sort();
        open.into_iter().map(|(_, id)| id).collect()
    };
    assert_eq!(
        open_of(&alpha, &award),
        vec![ids[0].clone(), ids[1].clone()]
    );
    assert!(open_of(&alpha, &other).is_empty());
    assert!(open_of(&beta, &award).is_empty());

    // The first claim holds the key; a later claim of it, by another run, changes nothing.
    assert_eq!(store.run_key_holder(&alpha, &award, "h1").unwrap(), None);
    let claim = |run: &str, key: &str| WorkerWrites {
        run_keys: vec![RunKeyClaim {
            automation: award.clone(),
            key: key.to_string(),
            run: run.to_string(),
        }],
        ..WorkerWrites::default()
    };
    let lease = store
        .job_lease(&alpha, "w1", Timestamp(5_001))
        .unwrap()
        .expect("the first start is due");
    store
        .job_report(
            &alpha,
            &lease.job.id,
            lease.token,
            &JobReport::Succeeded,
            &claim("run-a", "h1"),
            Timestamp(5_002),
        )
        .unwrap();
    store
        .automation_write(&alpha, &claim("run-b", "h1"), Timestamp(5_003))
        .unwrap();
    assert_eq!(
        store
            .run_key_holder(&alpha, &award, "h1")
            .unwrap()
            .as_deref(),
        Some("run-a")
    );
    assert_eq!(store.run_key_holder(&alpha, &other, "h1").unwrap(), None);
    assert_eq!(store.run_key_holder(&beta, &award, "h1").unwrap(), None);
    // The ended start job is no longer open.
    assert_eq!(open_of(&alpha, &award), vec![ids[1].clone()]);

    // Removing the automation releases its keys.
    store
        .automation_write(
            &alpha,
            &WorkerWrites {
                run_keys_released: vec![award.clone()],
                ..WorkerWrites::default()
            },
            Timestamp(5_004),
        )
        .unwrap();
    assert_eq!(store.run_key_holder(&alpha, &award, "h1").unwrap(), None);
}

/// The purge of the worker's records (`Store::purge_automation`, OPS-348, PRV-357): a job that
/// ended thirty days ago goes and a younger one stays; a run that ended ninety days ago goes with
/// its steps and a younger one stays; a declined record goes after ninety days; a parked or
/// dead-lettered record that is not settled never goes, and a settled one goes after ninety days.
///
/// Not a row of the conformance table: the adapters' tests call it by name.
// Verifies: OPS-348, SCH-2703
pub fn automation_purge_conforms(store: &dyn Store) {
    use crate::jobs::{
        ExceptionKind, ExceptionRecord, JobReport, JobWork, NewJob, RetryPolicy, RunRecord,
        RunState, StepRecord, StepState, WorkerWrites,
    };
    use crate::schedules::AutomationRetention;
    Pair::new(store);
    let alpha = tenant_id("alpha");
    let beta = tenant_id("beta");
    let day = 24 * 60 * 60 * 1_000_i64;
    let automation = WorkflowId::new("award").expect("id");
    let retention = AutomationRetention::default();
    let job = |key: &str, due: i64| NewJob {
        key: key.to_string(),
        work: JobWork::Step {
            run: "run-old".into(),
            automation: automation.clone(),
            step: StepName::new("first").expect("name"),
            sequence: 1,
        },
        due_at: Timestamp(due),
        policy: RetryPolicy {
            attempts: 1,
            first_delay_ms: 1,
            max_delay_ms: 1,
            lease_ms: 50,
        },
    };
    // Two jobs that end: one at 5_010, one twenty days later.
    let ids = store
        .jobs_create(
            &alpha,
            &[job("old", 5_000), job("young", 5_000 + 20 * day)],
            Timestamp(4_000),
        )
        .unwrap();
    for (id, due) in [(&ids[0], 5_000), (&ids[1], 5_000 + 20 * day)] {
        let lease = store
            .job_lease(&alpha, "w1", Timestamp(due))
            .unwrap()
            .expect("due");
        assert_eq!(&lease.job.id, id);
        store
            .job_report(
                &alpha,
                id,
                lease.token,
                &JobReport::Succeeded,
                &WorkerWrites::default(),
                Timestamp(due + 10),
            )
            .unwrap();
    }
    // A job that has not ended is never purged.
    store
        .jobs_create(&alpha, &[job("open", 5_000 + 100 * day)], Timestamp(4_000))
        .unwrap();
    // The same in another tenant, to see that the purge reads every tenant.
    let beta_ids = store
        .jobs_create(&beta, &[job("old", 5_000)], Timestamp(4_000))
        .unwrap();
    let lease = store
        .job_lease(&beta, "w1", Timestamp(5_000))
        .unwrap()
        .expect("due");
    store
        .job_report(
            &beta,
            &beta_ids[0],
            lease.token,
            &JobReport::Succeeded,
            &WorkerWrites::default(),
            Timestamp(5_010),
        )
        .unwrap();

    let run = |id: &str, ended: Option<i64>| RunRecord {
        started_by: None,
        run: id.into(),
        automation: automation.clone(),
        trigger: None,
        event_seq: None,
        state: if ended.is_some() {
            RunState::Completed
        } else {
            RunState::Running
        },
        step: None,
        steps: 1,
        hop: 0,
        caused_by: principal_id("automation:award"),
        on_behalf_of: None,
        params: BTreeMap::new(),
        started_at: Timestamp(50),
        ended_at: ended.map(Timestamp),
        error: None,
        seq: 0,
        definition_version: None,
        definition: None,
    };
    let step = |run: &str| StepRecord {
        approval: None,
        run: run.into(),
        sequence: 1,
        step: StepName::new("first").expect("name"),
        state: StepState::Completed,
        attempts: 1,
        job: None,
        idempotency_key: "k".into(),
        result: None,
        errors: Vec::new(),
        started_at: Timestamp(50),
        ended_at: Some(Timestamp(60)),
    };
    let exception = |id: &str, kind: ExceptionKind, settled: bool, at: i64| ExceptionRecord {
        id: id.into(),
        kind,
        reason: "attempts-exhausted".into(),
        trigger: None,
        automation: Some(automation.clone()),
        run: None,
        step: None,
        event_seq: None,
        job: None,
        hop: None,
        at: Timestamp(at),
        detail: String::new(),
        settled,
        seq: 0,
    };
    store
        .automation_write(
            &alpha,
            &WorkerWrites {
                runs: vec![
                    run("run-old", Some(100)),
                    run("run-young", Some(80 * day)),
                    run("run-open", None),
                ],
                steps: vec![step("run-old"), step("run-young"), step("run-open")],
                exceptions: vec![
                    exception("declined-old", ExceptionKind::Declined, true, 50),
                    exception("declined-young", ExceptionKind::Declined, true, 85 * day),
                    exception("dead-open", ExceptionKind::DeadLettered, false, 50),
                    exception("parked-open", ExceptionKind::Parked, false, 50),
                    exception("dead-settled", ExceptionKind::DeadLettered, true, 60),
                ],
                ..WorkerWrites::default()
            },
            Timestamp(70),
        )
        .unwrap();
    let job_keys = |tenant: &TenantId| -> Vec<String> {
        let mut keys: Vec<String> = store
            .jobs_list(tenant, None, 50)
            .unwrap()
            .into_iter()
            .map(|job| job.key)
            .collect();
        keys.sort();
        keys
    };
    let exceptions_of = || -> Vec<String> {
        let mut ids: Vec<String> = store
            .automation_exceptions(&alpha, None, false, None, 50)
            .unwrap()
            .into_iter()
            .map(|record| record.id)
            .collect();
        ids.sort();
        ids
    };
    let runs_of = || -> Vec<String> {
        let mut runs: Vec<String> = store
            .automation_runs(&alpha, None, None, None, 50)
            .unwrap()
            .into_iter()
            .map(|run| run.run)
            .collect();
        runs.sort();
        runs
    };

    // Thirty days after the first job ended: it goes, in both tenants; the younger job and the
    // open one stay; the runs and exception records, kept ninety days, all stay.
    let at = Timestamp(5_010 + 30 * day);
    let purged = store.purge_automation(at, &retention).unwrap();
    assert_eq!((purged.jobs, purged.runs, purged.exceptions), (2, 0, 0));
    assert_eq!(job_keys(&alpha), vec!["open", "young"]);
    assert!(job_keys(&beta).is_empty(), "the purge reads every tenant");
    assert_eq!(runs_of().len(), 3);
    assert_eq!(exceptions_of().len(), 5);

    // Ninety days after the oldest records: the old run goes with its step, the young and open
    // runs stay; the old declined record and the settled dead-lettered one go; the unsettled
    // ones never go. The job that ended twenty days after the first is past thirty days too.
    let at = Timestamp(100 + 90 * day);
    let purged = store.purge_automation(at, &retention).unwrap();
    assert_eq!((purged.runs, purged.steps, purged.exceptions), (1, 1, 2));
    assert_eq!(runs_of(), vec!["run-open", "run-young"]);
    assert!(store
        .automation_steps(&alpha, "run-old", None, 10)
        .unwrap()
        .is_empty());
    assert_eq!(
        store
            .automation_steps(&alpha, "run-young", None, 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        exceptions_of(),
        vec!["dead-open", "declined-young", "parked-open"]
    );
    assert_eq!(job_keys(&alpha), vec!["open"]);

    // Far in the future: everything that ended or was settled goes, and nothing that is open.
    store
        .purge_automation(Timestamp(1_000 * day), &retention)
        .unwrap();
    assert_eq!(runs_of(), vec!["run-open"]);
    assert_eq!(exceptions_of(), vec!["dead-open", "parked-open"]);
    assert_eq!(job_keys(&alpha), vec!["open"]);
}
