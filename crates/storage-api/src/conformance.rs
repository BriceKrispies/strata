//! The adapter conformance suite. Every `Store` implementation must pass [`run`].
//!
//! The suite drives the adapter with the kernel's notes fixture and compares what the adapter
//! holds with a `World` that received the same intents, so "the adapter stores what the kernel
//! decided" is checked by canonical hash rather than field by field.

use crate::{
    execute, execute_by, idempotency_scope, query, Account, AuditEntry, Commit, ExecuteError,
    OutboxEntry, SessionRecord, Store, StoreError, IDEMPOTENCY_RETENTION_MS,
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
        Err(ExecuteError::Domain(error)) => assert_eq!(error.kind, ErrorKind::InvalidIntent),
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
