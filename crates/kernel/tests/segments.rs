//! Segments in the kernel: definitions and their refusals, membership decided with the change
//! that moves it, the bound on one transaction and its continuation, time, erasure, the reads,
//! tenancy, and membership against a fresh evaluation after random changes.
//!
//! The harness runs every intent the way `strata_storage_api::execute_by` does: the kernel
//! decides the transition, the segment pipeline (`plan_change`, `targets`, `needs_for`,
//! `decide_write`) decides the memberships the change moves, and both are applied together.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use strata_kernel::segments::{
    self as seg, Batch, BatchLoad, BatchScope, Definition, Occasion, Segment, SegmentDirection,
    SegmentId, SegmentMembershipEvent, SegmentReason, SegmentTables, SegmentWrite,
};
use strata_kernel::testkit::{ctx, field, field_def, object, principal_id, role, tenant_id, text};
use strata_kernel::*;

fn parse(text: &str) -> Expr {
    expr::parse(text).expect("the predicate parses")
}

fn owner(tenant: &str) -> Principal {
    Principal::User(principal_id(&format!("{tenant}:owner")))
}

fn person(tenant: &str, name: &str) -> PrincipalId {
    principal_id(&format!("{tenant}:{name}"))
}

fn id(n: u8) -> SegmentId {
    let mut bytes = [0u8; 16];
    bytes[15] = n;
    bytes[6] = 0x40;
    bytes[8] = 0x80;
    seg::segment_id_of(bytes)
}

fn permissions(role_name: &str, permissions: Vec<Permission>) -> SchemaChange {
    SchemaChange::DefineRolePermissions {
        role: role(role_name),
        permissions,
    }
}

fn read_of(object_name: &str) -> Permission {
    Permission::Object {
        object: object(object_name),
        actions: BTreeSet::from([ObjectAction::Read]),
    }
}

/// `plan` { level } and `member` { principal (required, unique), tier, score, joined, plan, salary
/// (read by the owner only) }; roles: `analyst` (defines and reads segments, reads members but not
/// plans), `marketer` (defines segments, reads gold members only), `viewer` (reads segments,
/// members and plans), `clerk` (reads segments and members, not plans).
fn schema() -> Vec<SchemaChange> {
    let mut changes = Vec::new();
    for name in ["analyst", "marketer", "viewer", "clerk"] {
        changes.push(SchemaChange::DefineRole(RoleDef {
            id: role(name),
            label: name.to_string(),
        }));
    }
    changes.extend([
        SchemaChange::DefineObject {
            id: object("plan"),
            label: "Plan".into(),
        },
        SchemaChange::DefineField {
            object: object("plan"),
            field: field_def("level", FieldType::Str, true),
        },
        SchemaChange::DefineObject {
            id: object("member"),
            label: "Member".into(),
        },
        SchemaChange::DefineField {
            object: object("member"),
            field: FieldDef {
                unique: true,
                ..field_def("principal", FieldType::Str, true)
            },
        },
        SchemaChange::DefineField {
            object: object("member"),
            field: field_def("tier", FieldType::Str, false),
        },
        SchemaChange::DefineField {
            object: object("member"),
            field: field_def("score", FieldType::Int, false),
        },
        SchemaChange::DefineField {
            object: object("member"),
            field: field_def("joined", FieldType::Time, false),
        },
        SchemaChange::DefineField {
            object: object("member"),
            field: FieldDef {
                read: Some(Expr::HasRole(owner_role())),
                ..field_def("salary", FieldType::Int, false)
            },
        },
        SchemaChange::DefineRelationship {
            object: object("member"),
            field: field("plan"),
            label: "Plan".into(),
            target: object("plan"),
            on_delete: OnDelete::SetNull,
            required: false,
        },
        permissions(
            "analyst",
            vec![
                Permission::Capability(seg::SEGMENTS_DEFINE.into()),
                Permission::Capability(seg::SEGMENTS_READ.into()),
                read_of("member"),
            ],
        ),
        permissions(
            "marketer",
            vec![Permission::Capability(seg::SEGMENTS_DEFINE.into())],
        ),
        permissions(
            "viewer",
            vec![
                Permission::Capability(seg::SEGMENTS_READ.into()),
                read_of("member"),
                read_of("plan"),
            ],
        ),
        permissions(
            "clerk",
            vec![
                Permission::Capability(seg::SEGMENTS_READ.into()),
                read_of("member"),
            ],
        ),
        SchemaChange::DefinePolicy(PolicyDef {
            id: PolicyId::new("marketers_read_gold").expect("policy id"),
            role: role("marketer"),
            resource: Resource::Object(object("member")),
            actions: BTreeSet::from([Action::Read]),
            condition: Some(parse("record.tier == 'gold'")),
        }),
    ]);
    changes
}

/// A world and the segment block of each tenant, stepped as the shell steps them.
struct Harness {
    world: World,
    tables: BTreeMap<TenantId, SegmentTables>,
    now: i64,
}

impl Harness {
    fn new(tenants: &[&str]) -> Self {
        let mut h = Harness {
            world: World::default(),
            tables: BTreeMap::new(),
            now: 10_000,
        };
        for tenant in tenants {
            h.must(
                tenant,
                Principal::System,
                Intent::CreateTenant {
                    name: tenant.to_string(),
                    owner: person(tenant, "owner"),
                },
            );
            h.must(
                tenant,
                owner(tenant),
                Intent::ChangeSchema {
                    changes: schema(),
                    expected_version: None,
                    existing: ExistingData::Reject,
                },
            );
            for name in ["analyst", "marketer", "viewer", "clerk"] {
                h.must(
                    tenant,
                    owner(tenant),
                    Intent::AssignRole {
                        principal: person(tenant, name),
                        role: role(name),
                    },
                );
            }
        }
        h
    }

    fn ctx(&mut self, tenant: &str, who: Principal) -> Context {
        self.now += 1;
        ctx(tenant, who, self.now)
    }

    fn tables(&mut self, tenant: &str) -> &mut SegmentTables {
        self.tables.entry(tenant_id(tenant)).or_default()
    }

    fn state(&self, tenant: &str) -> State {
        self.world.state(&tenant_id(tenant))
    }

    fn tenant(&self, tenant: &str) -> Arc<TenantState> {
        self.world.tenants[&tenant_id(tenant)].clone()
    }

    /// Run an intent and the segment decision of its change, and apply both: the shell's
    /// `execute_by` with a complete snapshot.
    fn step(
        &mut self,
        tenant: &str,
        who: Principal,
        intent: Intent,
    ) -> Result<(Vec<SegmentMembershipEvent>, SegmentWrite), DomainError> {
        let ctx = self.ctx(tenant, who);
        let before = self.state(tenant);
        let outcome = transition(&before, &intent, &ctx)?;
        let after_tenant = apply_delta(before.tenant.as_deref(), &outcome.delta)?;
        let after = State::complete(Arc::new(after_tenant.clone()));
        let catalog = self.tables(tenant).catalog();
        let schema = after_tenant.schema.clone();
        if let Some(stored) = before.tenant.as_deref() {
            let problems = seg::schema_change_problems(&catalog, &stored.schema, &schema);
            if !problems.is_empty() {
                return Err(DomainError::new(
                    ErrorKind::InvalidSchema,
                    problems.join("; "),
                ));
            }
        }
        let plan = seg::plan_change(&catalog, &schema, &outcome.events);
        let changed = seg::changes(&outcome.events);
        // The record index answers from the stored state, in id order, one more than a batch.
        let referenced: Vec<_> = plan
            .referencing
            .iter()
            .map(|item| {
                let found: Vec<Arc<Record>> = before
                    .tenant
                    .as_deref()
                    .map(|t| {
                        t.records_of(&item.object)
                            .filter(|r| r.get(&item.field) == &Value::Ref(item.record))
                            .take(seg::BATCH + 1)
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                (item.clone(), found)
            })
            .collect();
        let (targets, batches) = seg::targets(
            &plan,
            &catalog,
            &referenced,
            &seg::after_map(&changed),
            0,
            ctx.now,
        );
        assert!(
            seg::needs_for(&catalog, &after, &targets).is_empty(),
            "a complete snapshot covers every need"
        );
        let rows = self.tables(tenant).rows(&seg::pairs(&targets));
        let occasion = Occasion {
            reason: SegmentReason::Change,
            hop: 0,
            revision: outcome.delta.new_revision,
            now: ctx.now,
        };
        let decided = seg::decide_write(&catalog, &after, &targets, &rows, &occasion, &ctx)?;
        let mut write = decided.write;
        for batch in batches {
            write.batches.push(seg::BatchChange::Put(batch));
        }
        self.tables(tenant)
            .apply(&write)
            .expect("the write rests on the tables");
        self.world
            .tenants
            .insert(tenant_id(tenant), Arc::new(after_tenant));
        Ok((decided.events, write))
    }

    fn must(
        &mut self,
        tenant: &str,
        who: Principal,
        intent: Intent,
    ) -> Vec<SegmentMembershipEvent> {
        self.step(tenant, who, intent)
            .expect("the intent is accepted")
            .0
    }

    fn define(
        &mut self,
        tenant: &str,
        who: Principal,
        def: Definition,
    ) -> Result<seg::Defined, DomainError> {
        let ctx = self.ctx(tenant, who);
        let state = self.tenant(tenant);
        let catalog = self.tables(tenant).catalog();
        let defined = seg::define(&state, &catalog, def, &ctx)?;
        self.tables(tenant)
            .apply(&defined.write)
            .expect("the definition rests on the tables");
        Ok(defined)
    }

    /// Run one pending batch of a tenant, as the job worker does; its events.
    fn run_batch(&mut self, tenant: &str, batch: &Batch) -> Vec<SegmentMembershipEvent> {
        let ctx = self.ctx(tenant, Principal::System);
        let state = self.state(tenant);
        let catalog = self.tables(tenant).catalog();
        let mut write = SegmentWrite::default();
        let mut events = Vec::new();
        // A batch of a superseded version writes nothing (SEG-025): the row of its scope already
        // belongs to the newer version.
        if seg::batch_current(&catalog, batch) {
            let segment = catalog
                .iter()
                .find(|s| s.id == batch.segment)
                .unwrap()
                .clone();
            let tenant_state = self.tenant(tenant);
            let (records, due): (Vec<Arc<Record>>, Vec<seg::Wake>) =
                match seg::batch_load(&segment, batch, ctx.now) {
                    BatchLoad::Page {
                        object,
                        after,
                        limit,
                    } => (
                        tenant_state
                            .records_of(&object)
                            .filter(|r| after.is_none_or(|a| r.id > a))
                            .take(limit as usize)
                            .cloned()
                            .collect(),
                        Vec::new(),
                    ),
                    BatchLoad::Referencing {
                        object,
                        field,
                        record,
                        after,
                        limit,
                    } => (
                        tenant_state
                            .records_of(&object)
                            .filter(|r| r.get(&field) == &Value::Ref(record))
                            .filter(|r| after.is_none_or(|a| r.id > a))
                            .take(limit as usize)
                            .cloned()
                            .collect(),
                        Vec::new(),
                    ),
                    BatchLoad::Due { segment, at, limit } => (
                        Vec::new(),
                        self.tables(tenant).due(segment, at, limit as usize),
                    ),
                };
            let (targets, follow) = seg::batch_targets(&catalog, batch, &records, &due, ctx.now);
            assert!(
                targets.len() <= seg::BATCH,
                "a batch evaluates at most one hundred persons"
            );
            let rows = self.tables(tenant).rows(&seg::pairs(&targets));
            let occasion = Occasion {
                reason: batch.reason,
                hop: batch.hop,
                revision: tenant_state.revision,
                now: ctx.now,
            };
            let decided =
                seg::decide_write(&catalog, &state, &targets, &rows, &occasion, &ctx).unwrap();
            events = decided.events;
            write = decided.write;
            write.batches.push(follow);
        }
        self.tables(tenant)
            .apply(&write)
            .expect("the batch rests on the tables");
        events
    }

    /// Run every pending batch of a tenant until none is left; the events of each batch.
    fn run_all(&mut self, tenant: &str) -> Vec<Vec<SegmentMembershipEvent>> {
        let mut runs = Vec::new();
        for _ in 0..1_000 {
            let Some(batch) = self.tables(tenant).batches.values().next().cloned() else {
                return runs;
            };
            runs.push(self.run_batch(tenant, &batch));
        }
        panic!("the batches do not end");
    }

    fn members(&mut self, tenant: &str, segment: SegmentId) -> BTreeSet<PrincipalId> {
        self.tables(tenant)
            .memberships
            .keys()
            .filter(|(s, _)| *s == segment)
            .map(|(_, p)| p.clone())
            .collect()
    }

    fn create_member(&mut self, tenant: &str, name: &str, fields: &[(&str, Value)]) -> RecordId {
        let mut values: BTreeMap<FieldId, Value> = fields
            .iter()
            .map(|(name, value)| (field(name), value.clone()))
            .collect();
        values.insert(field("principal"), text(person(tenant, name).as_str()));
        let ctx_owner = owner(tenant);
        let (_, _) = self
            .step(
                tenant,
                ctx_owner,
                Intent::CreateRecord {
                    object: object("member"),
                    fields: values,
                },
            )
            .expect("the member is created");
        let tenant_state = self.tenant(tenant);
        tenant_state
            .records_of(&object("member"))
            .find(|r| r.get(&field("principal")) == &text(person(tenant, name).as_str()))
            .map(|r| r.id)
            .expect("the member exists")
    }

    fn create_plan(&mut self, tenant: &str, level: &str) -> RecordId {
        self.step(
            tenant,
            owner(tenant),
            Intent::CreateRecord {
                object: object("plan"),
                fields: BTreeMap::from([(field("level"), text(level))]),
            },
        )
        .expect("the plan is created");
        self.tenant(tenant)
            .records_of(&object("plan"))
            .map(|r| r.id)
            .max()
            .expect("the plan exists")
    }

    fn update(
        &mut self,
        tenant: &str,
        object_name: &str,
        id: RecordId,
        fields: &[(&str, Value)],
    ) -> Vec<SegmentMembershipEvent> {
        self.must(
            tenant,
            owner(tenant),
            Intent::UpdateRecord {
                object: object(object_name),
                id,
                fields: fields
                    .iter()
                    .map(|(name, value)| (field(name), value.clone()))
                    .collect(),
                expected_version: None,
            },
        )
    }
}

fn definition(n: u8, name: &str, predicate: &str) -> Definition {
    Definition {
        id: id(n),
        name: name.to_string(),
        subject: object("member"),
        predicate: parse(predicate),
        refresh_seconds: None,
        principal_field: field("principal"),
        visible_to_members: false,
    }
}

fn directions(events: &[SegmentMembershipEvent]) -> Vec<(String, SegmentDirection)> {
    events
        .iter()
        .map(|e| (e.person.as_str().to_string(), e.direction))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Membership at commit
// ---------------------------------------------------------------------------------------------

// Verifies: SEG-008
#[test]
fn a_field_change_enters_and_leaves_in_the_commit_of_the_change() {
    let mut h = Harness::new(&["alpha"]);
    h.define(
        "alpha",
        owner("alpha"),
        definition(1, "Gold", "record.tier == 'gold'"),
    )
    .unwrap();
    let ada = h.create_member("alpha", "ada", &[("tier", text("silver"))]);
    assert!(h.members("alpha", id(1)).is_empty());

    let entered = h.update("alpha", "member", ada, &[("tier", text("gold"))]);
    assert_eq!(
        directions(&entered),
        [("alpha:ada".into(), SegmentDirection::Entered)]
    );
    let event = &entered[0];
    assert_eq!(event.segment, id(1));
    assert_eq!(event.reason, SegmentReason::Change);
    assert_eq!(event.version, 1);
    assert_eq!(
        event.hop, 1,
        "the hop count of the change plus one (SEG-031)"
    );
    assert_eq!(event.revision, h.tenant("alpha").revision);
    assert_eq!(
        h.members("alpha", id(1)),
        BTreeSet::from([person("alpha", "ada")])
    );

    let exited = h.update("alpha", "member", ada, &[("tier", text("silver"))]);
    assert_eq!(
        directions(&exited),
        [("alpha:ada".into(), SegmentDirection::Exited)]
    );
    assert!(h.members("alpha", id(1)).is_empty());
    assert_eq!(h.tables("alpha").segments[&id(1)].member_count, 0);
}

// Verifies: SEG-010
#[test]
fn a_change_that_leaves_membership_as_it_was_writes_no_event() {
    let mut h = Harness::new(&["alpha"]);
    h.define(
        "alpha",
        owner("alpha"),
        definition(1, "Gold", "record.tier == 'gold'"),
    )
    .unwrap();
    let ada = h.create_member("alpha", "ada", &[("tier", text("gold"))]);
    let bob = h.create_member("alpha", "bob", &[("tier", text("silver"))]);
    // A field the predicate does not read: the change names no one for the segment.
    let (events, write) = h
        .step(
            "alpha",
            owner("alpha"),
            Intent::UpdateRecord {
                object: object("member"),
                id: ada,
                fields: BTreeMap::from([(field("score"), Value::Int(7))]),
                expected_version: None,
            },
        )
        .unwrap();
    assert!(events.is_empty());
    assert!(write.seen.is_empty(), "nobody was evaluated");
    // A field it reads, without crossing the predicate either way.
    assert!(h
        .update("alpha", "member", bob, &[("tier", text("bronze"))])
        .is_empty());
    assert!(h
        .update("alpha", "member", ada, &[("tier", text("gold"))])
        .is_empty());
    assert_eq!(
        h.members("alpha", id(1)),
        BTreeSet::from([person("alpha", "ada")])
    );
}

// Verifies: SEG-023
#[test]
fn a_changed_predicate_is_applied_in_as_many_batches_as_hundreds_of_persons() {
    let mut h = Harness::new(&["alpha"]);
    for n in 0..250 {
        let tier = if n % 2 == 0 { "gold" } else { "silver" };
        h.create_member("alpha", &format!("p{n:03}"), &[("tier", text(tier))]);
    }
    let defined = h
        .define(
            "alpha",
            owner("alpha"),
            definition(1, "Gold", "record.tier == 'gold'"),
        )
        .unwrap();
    assert!(defined.rebuild);
    let runs = h.run_all("alpha");
    assert_eq!(runs.len(), 3, "250 persons, three batches");
    assert_eq!(h.members("alpha", id(1)).len(), 125);
    assert!(runs
        .iter()
        .flatten()
        .all(|e| e.reason == SegmentReason::PredicateChange));

    // A new version: the old members who no longer hold leave, and the version rises.
    let changed = h
        .define(
            "alpha",
            owner("alpha"),
            definition(1, "Gold", "record.tier == 'silver'"),
        )
        .unwrap();
    assert_eq!(changed.segment.version, 2);
    // A batch of the superseded version writes nothing (SEG-025).
    let stale = Batch {
        segment: id(1),
        scope: BatchScope::All,
        version: 1,
        after: None,
        reason: SegmentReason::PredicateChange,
        hop: 0,
        at: Timestamp(h.now),
    };
    assert!(h.run_batch("alpha", &stale).is_empty());
    h.run_all("alpha");
    assert_eq!(h.members("alpha", id(1)).len(), 125);
    assert!(h
        .tables("alpha")
        .memberships
        .values()
        .all(|m| m.version == 2));
    // A change of name keeps the version and starts no batch.
    let mut renamed = definition(1, "Silver", "record.tier == 'silver'");
    renamed.visible_to_members = true;
    let renamed = h.define("alpha", owner("alpha"), renamed).unwrap();
    assert_eq!(renamed.segment.version, 2);
    assert!(!renamed.rebuild);
}

// ---------------------------------------------------------------------------------------------
// Definitions
// ---------------------------------------------------------------------------------------------

// Verifies: SEG-029
#[test]
fn the_bounds_of_a_tenant_are_refused_past_them() {
    let mut h = Harness::new(&["alpha"]);
    for n in 0..seg::MAX_SEGMENTS_PER_OBJECT as u8 {
        h.define(
            "alpha",
            owner("alpha"),
            definition(n + 1, &format!("s{n}"), "record.score > 1"),
        )
        .unwrap();
    }
    let past = h
        .define(
            "alpha",
            owner("alpha"),
            definition(200, "one too many", "record.score > 2"),
        )
        .unwrap_err();
    assert_eq!(past.kind, ErrorKind::LimitExceeded);
    // Replacing one of them is not one more.
    h.define(
        "alpha",
        owner("alpha"),
        definition(1, "s0", "record.score > 3"),
    )
    .unwrap();

    // One hundred segments in all. They read plans, so the bound of twenty-five segments reading
    // members is not what refuses the next one; they are put straight into the tables, as a
    // plan has no principal field.
    let mut tables = SegmentTables::default();
    let state = h.tenant("alpha");
    for n in 0..seg::MAX_SEGMENTS as u8 {
        let segment = Segment {
            id: id(n + 1),
            name: format!("p{n}"),
            subject: object("plan"),
            predicate: parse("record.level == 'pro'"),
            version: 1,
            time_dependent: false,
            indexable: false,
            refresh_seconds: None,
            defined_by: Some(person("alpha", "owner")),
            principal_field: field("level"),
            visible_to_members: false,
            schema_version: state.schema.version,
            member_count: 0,
            refresh_skipped_at: None,
        };
        tables.segments.insert(segment.id, segment);
    }
    let ctx = ctx("alpha", owner("alpha"), h.now);
    let full = seg::define(
        &state,
        &tables.catalog(),
        definition(201, "extra", "record.score > 1"),
        &ctx,
    )
    .unwrap_err();
    assert_eq!(full.kind, ErrorKind::LimitExceeded);
    assert!(full.message.contains("100 segments"), "{}", full.message);
}

// Verifies: SEG-017
#[test]
fn time_predicates_declare_a_refresh_interval_within_its_bounds() {
    let mut h = Harness::new(&["alpha"]);
    let recent = "record.joined > now - days(7)";
    let missing = h
        .define("alpha", owner("alpha"), definition(1, "Recent", recent))
        .unwrap_err();
    assert_eq!(missing.kind, ErrorKind::InvalidIntent);
    for seconds in [59, 86_401] {
        let mut def = definition(1, "Recent", recent);
        def.refresh_seconds = Some(seconds);
        assert!(
            h.define("alpha", owner("alpha"), def).is_err(),
            "{seconds} s"
        );
    }
    let mut def = definition(1, "Recent", recent);
    def.refresh_seconds = Some(60);
    let defined = h.define("alpha", owner("alpha"), def).unwrap();
    assert!(defined.segment.time_dependent && defined.segment.indexable);
    // A predicate that does not read the time declares none.
    let mut def = definition(2, "Gold", "record.tier == 'gold'");
    def.refresh_seconds = Some(60);
    assert!(h.define("alpha", owner("alpha"), def).is_err());
}

// Verifies: SEG-1546
#[test]
fn a_predicate_that_is_not_indexable_refreshes_every_fifteen_minutes_at_most() {
    let mut h = Harness::new(&["alpha"]);
    let opaque = "ms_between(record.joined, now) > 1000";
    let mut def = definition(1, "Opaque", opaque);
    def.refresh_seconds = Some(600);
    assert!(h.define("alpha", owner("alpha"), def).is_err());
    let mut def = definition(1, "Opaque", opaque);
    def.refresh_seconds = Some(900);
    let defined = h.define("alpha", owner("alpha"), def).unwrap();
    assert!(defined.segment.time_dependent && !defined.segment.indexable);
}

// Verifies: SEG-1547
#[test]
fn a_sixth_segment_that_is_not_indexable_is_refused() {
    let mut h = Harness::new(&["alpha"]);
    for n in 1..=5u8 {
        let mut def = definition(n, &format!("o{n}"), "ms_between(record.joined, now) > 1000");
        def.refresh_seconds = Some(900);
        h.define("alpha", owner("alpha"), def).unwrap();
    }
    let mut def = definition(6, "o6", "ms_between(record.joined, now) > 1000");
    def.refresh_seconds = Some(900);
    assert_eq!(
        h.define("alpha", owner("alpha"), def).unwrap_err().kind,
        ErrorKind::LimitExceeded
    );
}

// Verifies: SEG-1548
#[test]
fn the_principal_field_is_a_required_unique_string() {
    let mut h = Harness::new(&["alpha"]);
    let mut def = definition(1, "Gold", "record.tier == 'gold'");
    def.principal_field = field("tier");
    assert_eq!(
        h.define("alpha", owner("alpha"), def).unwrap_err().kind,
        ErrorKind::InvalidIntent
    );
}

// Verifies: SEG-004
#[test]
fn a_predicate_outside_the_context_of_a_person_is_refused() {
    let mut h = Harness::new(&["alpha"]);
    let gold = || Box::new(Expr::Lit(text("gold")));
    let outside = [
        // old and new are the scopes of a change, not of a person
        Expr::Cmp(
            CmpOp::Eq,
            Box::new(Expr::Var(Scope::New, "tier".into())),
            gold(),
        ),
        // a segment predicate has no parameters
        Expr::Cmp(CmpOp::Eq, Box::new(Expr::Param("tier".into())), gold()),
        // a field the subject type does not have
        Expr::Cmp(
            CmpOp::Eq,
            Box::new(Expr::Var(Scope::Record, "nothing".into())),
            Box::new(Expr::Lit(Value::Int(1))),
        ),
        // not a truth value
        Expr::Var(Scope::Record, "tier".into()),
    ];
    for predicate in outside {
        let mut def = definition(1, "Bad", "record.tier == 'gold'");
        def.predicate = predicate.clone();
        assert!(
            h.define("alpha", owner("alpha"), def).is_err(),
            "{predicate:?}"
        );
    }
    // Two hundred nodes at most.
    let long = (0..120)
        .map(|n| format!("record.score == {n}"))
        .collect::<Vec<_>>()
        .join(" or ");
    assert!(h
        .define("alpha", owner("alpha"), definition(1, "Long", &long))
        .is_err());
}

// Verifies: SEG-007
#[test]
fn only_a_holder_of_segments_define_defines_or_removes() {
    let mut h = Harness::new(&["alpha"]);
    let viewer = Principal::User(person("alpha", "viewer"));
    assert_eq!(
        h.define(
            "alpha",
            viewer.clone(),
            definition(1, "Gold", "record.tier == 'gold'")
        )
        .unwrap_err()
        .kind,
        ErrorKind::Unauthorized
    );
    h.define(
        "alpha",
        owner("alpha"),
        definition(1, "Gold", "record.tier == 'gold'"),
    )
    .unwrap();
    let state = h.tenant("alpha");
    let catalog = h.tables("alpha").catalog();
    let refused = seg::remove(&state, &catalog, id(1), &ctx("alpha", viewer, h.now));
    assert_eq!(refused.unwrap_err().kind, ErrorKind::Unauthorized);
}

// Verifies: SEG-026
#[test]
fn removing_a_segment_removes_its_rows_and_writes_no_event() {
    let mut h = Harness::new(&["alpha"]);
    h.define(
        "alpha",
        owner("alpha"),
        definition(1, "Gold", "record.tier == 'gold'"),
    )
    .unwrap();
    h.create_member("alpha", "ada", &[("tier", text("gold"))]);
    let state = h.tenant("alpha");
    let catalog = h.tables("alpha").catalog();
    let write = seg::remove(
        &state,
        &catalog,
        id(1),
        &ctx("alpha", owner("alpha"), h.now),
    )
    .unwrap();
    assert!(write.events.is_empty());
    h.tables("alpha").apply(&write).unwrap();
    assert!(h.tables("alpha").memberships.is_empty());
    assert!(h.tables("alpha").segments.is_empty());
}

// Verifies: SEG-006
#[test]
fn a_schema_change_that_removes_a_named_field_is_refused() {
    let mut h = Harness::new(&["alpha"]);
    h.define(
        "alpha",
        owner("alpha"),
        definition(1, "Gold", "record.tier == 'gold'"),
    )
    .unwrap();
    let refused = h.step(
        "alpha",
        owner("alpha"),
        Intent::ChangeSchema {
            changes: vec![SchemaChange::RemoveField {
                object: object("member"),
                field: field("tier"),
            }],
            expected_version: None,
            existing: ExistingData::Grandfather,
        },
    );
    assert_eq!(refused.unwrap_err().kind, ErrorKind::InvalidSchema);
    // A field no segment names may go.
    h.step(
        "alpha",
        owner("alpha"),
        Intent::ChangeSchema {
            changes: vec![SchemaChange::RemoveField {
                object: object("member"),
                field: field("score"),
            }],
            expected_version: None,
            existing: ExistingData::Grandfather,
        },
    )
    .unwrap();
}

// ---------------------------------------------------------------------------------------------
// Time, erasure, reads, tenancy
// ---------------------------------------------------------------------------------------------

// Verifies: SEG-020
#[test]
fn a_predicate_that_does_not_read_the_time_is_never_refreshed() {
    let mut h = Harness::new(&["alpha"]);
    let defined = h
        .define(
            "alpha",
            owner("alpha"),
            definition(1, "Gold", "record.tier == 'gold'"),
        )
        .unwrap();
    assert!(seg::start_refresh(&defined.segment, &[], Timestamp(h.now)).is_none());
}

// Verifies: SEG-1544
#[test]
fn a_skipped_refresh_is_recorded_on_the_segment() {
    let mut h = Harness::new(&["alpha"]);
    let mut def = definition(1, "Opaque", "ms_between(record.joined, now) > 1000");
    def.refresh_seconds = Some(900);
    let defined = h.define("alpha", owner("alpha"), def).unwrap();
    let at = Timestamp(h.now);
    h.tables("alpha")
        .apply(&seg::skip_refresh(&defined.segment, at))
        .unwrap();
    assert_eq!(
        h.tables("alpha").segments[&id(1)].refresh_skipped_at,
        Some(at)
    );
}

// Verifies: SEG-1540
#[test]
fn a_member_lists_only_the_visible_segments_they_belong_to() {
    let mut h = Harness::new(&["alpha"]);
    let mut shown = definition(1, "Gold", "record.tier == 'gold'");
    shown.visible_to_members = true;
    h.define("alpha", owner("alpha"), shown).unwrap();
    h.define(
        "alpha",
        owner("alpha"),
        definition(2, "Hidden gold", "record.tier == 'gold'"),
    )
    .unwrap();
    h.create_member("alpha", "ada", &[("tier", text("gold"))]);
    let ada = Principal::User(person("alpha", "ada"));
    let catalog = h.tables("alpha").catalog();
    let memberships = h.tables("alpha").memberships_of(&person("alpha", "ada"));
    let mine = seg::my_segments(&catalog, &ada, &memberships).unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].segment, id(1));
    assert!(seg::my_segments(&catalog, &Principal::Anonymous, &memberships).is_err());
}

// Verifies: SEG-1542
#[test]
fn a_preview_counts_the_matches_among_the_persons_it_examines() {
    let mut h = Harness::new(&["alpha"]);
    for n in 0..10 {
        let tier = if n < 4 { "gold" } else { "silver" };
        h.create_member("alpha", &format!("p{n}"), &[("tier", text(tier))]);
    }
    let tenant = h.tenant("alpha");
    let (draft, _) = seg::draft(
        &tenant,
        &owner("alpha"),
        &object("member"),
        &field("principal"),
        &parse("record.tier == 'gold'"),
    )
    .unwrap();
    let records: Vec<Arc<Record>> = tenant.records_of(&object("member")).cloned().collect();
    let (first, next) = seg::preview(
        &h.state("alpha"),
        &owner("alpha"),
        &draft,
        &records,
        6,
        Timestamp(h.now),
    )
    .unwrap();
    assert_eq!((first.matched, first.examined), (4, 6));
    assert!(next.is_some());
    let (all, next) = seg::preview(
        &h.state("alpha"),
        &owner("alpha"),
        &draft,
        &records,
        1_000,
        Timestamp(h.now),
    )
    .unwrap();
    assert_eq!((all.matched, all.examined), (4, 10));
    assert!(next.is_none());
}

// ---------------------------------------------------------------------------------------------
// Incremental membership equals a fresh evaluation
// ---------------------------------------------------------------------------------------------

/// A fixed-seed generator, so the run is the same every time.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, below: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % below
    }
}

// Verifies: SEG-011
#[test]
fn incremental_membership_equals_a_fresh_evaluation_after_random_changes() {
    let mut h = Harness::new(&["alpha"]);
    let mut rng = Lcg(0x5e9_2280);
    let plans: Vec<RecordId> = (0..4)
        .map(|n| h.create_plan("alpha", if n % 2 == 0 { "pro" } else { "basic" }))
        .collect();
    h.define(
        "alpha",
        owner("alpha"),
        definition(1, "Gold", "record.tier == 'gold'"),
    )
    .unwrap();
    h.define(
        "alpha",
        owner("alpha"),
        definition(2, "Pro", "record.plan.level == 'pro'"),
    )
    .unwrap();
    h.define(
        "alpha",
        owner("alpha"),
        definition(
            3,
            "Engaged",
            "record.score > 50 and record.plan.level != 'basic'",
        ),
    )
    .unwrap();
    let tiers = ["gold", "silver", "bronze"];
    let levels = ["pro", "basic", "team"];
    let mut people: Vec<(String, Option<RecordId>)> = Vec::new();
    for step in 0..400u32 {
        match rng.next(10) {
            0..=2 => {
                let name = format!("p{step}");
                let mut fields = vec![
                    ("tier", text(tiers[rng.next(3) as usize])),
                    ("score", Value::Int(rng.next(100) as i64)),
                ];
                if rng.next(4) != 0 {
                    fields.push(("plan", Value::Ref(plans[rng.next(4) as usize])));
                }
                let id = h.create_member("alpha", &name, &fields);
                people.push((name, Some(id)));
            }
            3..=5 if !people.is_empty() => {
                let index = rng.next(people.len() as u64) as usize;
                if let (_, Some(id)) = people[index].clone() {
                    let value = match rng.next(3) {
                        0 => ("tier", text(tiers[rng.next(3) as usize])),
                        1 => ("score", Value::Int(rng.next(100) as i64)),
                        _ => ("plan", Value::Ref(plans[rng.next(4) as usize])),
                    };
                    h.update("alpha", "member", id, &[value]);
                }
            }
            6 if !people.is_empty() => {
                let index = rng.next(people.len() as u64) as usize;
                if let (_, Some(id)) = people[index].clone() {
                    h.must(
                        "alpha",
                        owner("alpha"),
                        Intent::DeleteRecord {
                            object: object("member"),
                            id,
                            expected_version: None,
                        },
                    );
                    people[index].1 = None;
                }
            }
            _ => {
                let plan = plans[rng.next(4) as usize];
                h.update(
                    "alpha",
                    "plan",
                    plan,
                    &[("level", text(levels[rng.next(3) as usize]))],
                );
            }
        }
        // Continuations a change left run before the comparison, as the worker would.
        h.run_all("alpha");
        let state = h.state("alpha");
        let now = Timestamp(h.now);
        for segment in h.tables("alpha").catalog() {
            let expected = seg::evaluate_all(&segment, &state, now, &BTreeSet::new());
            assert_eq!(
                h.members("alpha", segment.id),
                expected,
                "segment {} after step {step}",
                segment.name
            );
        }
    }
}
