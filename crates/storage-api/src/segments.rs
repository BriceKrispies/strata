//! Segments around the kernel's decisions (SEG-001 to SEG-038): what each decision loads, in a
//! bounded number of loads, and how its write is committed. Every rule is the kernel's
//! ([`strata_kernel::segments`]); this module only fetches what a decision names and commits what
//! it returns, as [`crate::execute`] does for intents.
//!
//! - [`decide_change`] runs inside [`crate::execute_by`], for every accepted transition: the
//!   memberships its change moves are decided before the commit and committed with it.
//! - [`define`], [`remove`], [`run_batch`], [`start_refresh`], [`skip_refresh`] and [`erase`] are
//!   segment writes of their own, committed with [`Store::commit_segments`].
//! - [`members`], [`count`], [`mine`], [`is_member`], [`test`] and [`preview`] are the reads.
//!
//! A batch is the continuation a decision leaves when a change reaches more persons than one
//! transaction evaluates, a predicate changed, or a refresh is due. The kernel writes it as a
//! `rec-segment-batch` row; the job worker runs the pending ones with [`run_batch`], each in a
//! transaction of its own (CT-297). Until that worker exists, [`run_pending`] runs them in order.

use crate::{Commit, ExecuteError, Receipt, Store, StoreError, StoreResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;
use strata_kernel::ext::*;
use strata_kernel::segments::{
    self as seg, Batch, BatchChange, BatchLoad, Definition, Membership, Occasion, Segment,
    SegmentCount, SegmentId, SegmentMember, SegmentPreview, SegmentReason, SegmentSummary,
    SegmentTestResult, SegmentWrite, Target,
};
use strata_kernel::{
    apply_delta, Check, Context, DeltaOp, DomainError, ErrorKind, Expr, FieldId, Need,
    ObjectTypeId, Principal, PrincipalId, Record, RecordId, State, StateRevision, TenantId,
    TenantSchema, TenantState, Transition,
};

/// A segment write that no transition carries, applied while the tenant is at `revision`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentCommit {
    pub tenant: TenantId,
    /// The revision the decision was made at; the store refuses the write as a conflict when the
    /// tenant is at another.
    pub revision: StateRevision,
    pub write: SegmentWrite,
}

/// How often a segment write is decided again after losing a race, and how many rounds a
/// decision may spend loading the records it names.
const MAX_RETRIES: usize = 8;
const MAX_ROUNDS: usize = 4;

/// What [`decide_change`] decided for a transition.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeDecision {
    pub write: SegmentWrite,
    /// Evaluations that failed and left a person as they were (SEG-030).
    pub failures: Vec<Check>,
}

fn domain(error: DomainError) -> ExecuteError {
    ExecuteError::Domain(error)
}

fn conflict(found: Option<StateRevision>, expected: Option<StateRevision>) -> ExecuteError {
    ExecuteError::Store(StoreError::Conflict { expected, found })
}

/// The schema a transition leaves.
fn schema_after(header: &TenantState, transition: &Transition) -> Arc<TenantSchema> {
    transition
        .delta
        .ops
        .iter()
        .rev()
        .find_map(|op| match op {
            DeltaOp::PutSchema(schema) => Some(schema.clone()),
            _ => None,
        })
        .unwrap_or_else(|| header.schema.clone())
}

/// Load what a decision over `targets` names until nothing is missing, each round building the
/// snapshot with `shape` (the state after a transition, or the stored state). At most
/// [`MAX_ROUNDS`] loads: a predicate follows one reference, so two suffice.
fn load_for(
    store: &dyn Store,
    tenant: &TenantId,
    catalog: &[Segment],
    targets: &[Target],
    first: Vec<Need>,
    shape: &dyn Fn(State) -> Result<State, ExecuteError>,
) -> Result<State, ExecuteError> {
    let mut wanted: BTreeSet<Need> = first.into_iter().collect();
    let mut rounds = 0;
    loop {
        let list: Vec<Need> = wanted.iter().cloned().collect();
        let state = shape(store.load(tenant, &list)?)?;
        let more: Vec<Need> = seg::needs_for(catalog, &state, targets)
            .into_iter()
            .filter(|need| !wanted.contains(need))
            .collect();
        if more.is_empty() || rounds >= MAX_ROUNDS {
            return Ok(state);
        }
        wanted.extend(more);
        rounds += 1;
    }
}

/// Decide the memberships an accepted transition moves, for its own commit (D-5022, CT-296).
///
/// The change names its persons ([`seg::plan_change`]): the subject records it changed, and the
/// referrers of a referenced record it changed, found through the record index at most
/// [`seg::BATCH`] + 1 at a time. The decision then loads the subject record of each person and
/// what its predicate reaches, on the stored state with the transition laid over it, and the
/// membership rows of exactly those persons. A tenant without segments costs one read of its
/// (empty) catalogue. A schema change that removes or retypes a field a segment names is refused
/// (SEG-006). The hop count of the change is `hop`; a request's is 0.
pub fn decide_change(
    store: &dyn Store,
    transition: &Transition,
    ctx: &Context,
    hop: u64,
) -> Result<ChangeDecision, ExecuteError> {
    let tenant = &ctx.tenant;
    let catalog = store.segment_catalog(tenant)?;
    if catalog.is_empty() {
        return Ok(ChangeDecision::default());
    }
    let header = store.load(tenant, &[])?;
    let Some(stored) = header.tenant.as_deref() else {
        return Ok(ChangeDecision::default());
    };
    let schema = schema_after(stored, transition);
    if !Arc::ptr_eq(&schema, &stored.schema) {
        let problems = seg::schema_change_problems(&catalog, &stored.schema, &schema);
        if !problems.is_empty() {
            let evidence = problems
                .iter()
                .map(|p| Check::fail("segment:schema-change", p.clone()))
                .collect();
            return Err(domain(
                DomainError::new(ErrorKind::InvalidSchema, problems.join("; ")).with(evidence),
            ));
        }
    }
    let plan = seg::plan_change(&catalog, &schema, &transition.events);
    if plan.direct.is_empty() && plan.referencing.is_empty() {
        return Ok(ChangeDecision::default());
    }
    let changed = seg::changes(&transition.events);
    let after = seg::after_map(&changed);
    let mut referenced = Vec::new();
    for item in &plan.referencing {
        let found = store.segment_referencing(
            tenant,
            &item.object,
            &item.field,
            item.record,
            None,
            seg::BATCH + 1,
        )?;
        referenced.push((item.clone(), found));
    }
    let (targets, batches) = seg::targets(&plan, &catalog, &referenced, &after, hop, ctx.now);
    let delta = &transition.delta;
    let shape = |loaded: State| -> Result<State, ExecuteError> {
        let next = apply_delta(loaded.tenant.as_deref(), delta).map_err(|_| {
            conflict(
                loaded.tenant.as_ref().map(|t| t.revision),
                delta.base_revision,
            )
        })?;
        let laid = State {
            tenant_id: loaded.tenant_id.clone(),
            tenant: Some(Arc::new(next)),
            coverage: loaded.coverage.clone(),
        };
        Ok(seg::overlay(&laid, &changed))
    };
    let state = load_for(store, tenant, &catalog, &targets, Vec::new(), &shape)?;
    let rows = store.segment_rows(tenant, &seg::pairs(&targets))?;
    let occasion = Occasion {
        reason: SegmentReason::Change,
        hop,
        revision: delta.new_revision,
        now: ctx.now,
    };
    let decided =
        seg::decide_write(&catalog, &state, &targets, &rows, &occasion, ctx).map_err(domain)?;
    let mut write = decided.write;
    for batch in batches {
        write
            .expect
            .entry(batch.segment)
            .or_insert(Some(batch.version));
        write.batches.push(BatchChange::Put(batch));
    }
    Ok(ChangeDecision {
        write,
        failures: decided.failures,
    })
}

/// Commit a segment write of its own, deciding it again after a lost race.
fn commit_own(
    store: &dyn Store,
    tenant: &TenantId,
    decide: &mut dyn FnMut(&TenantState, &[Segment]) -> Result<SegmentWrite, ExecuteError>,
) -> Result<(SegmentWrite, Receipt), ExecuteError> {
    let mut retries = 0;
    loop {
        let header = store.load(tenant, &[])?;
        let Some(stored) = header.tenant.as_deref() else {
            return Err(domain(DomainError::new(
                ErrorKind::TenantNotFound,
                format!("no tenant {tenant}"),
            )));
        };
        let catalog = store.segment_catalog(tenant)?;
        let write = decide(stored, &catalog)?;
        let commit = SegmentCommit {
            tenant: tenant.clone(),
            revision: stored.revision,
            write: write.clone(),
        };
        match store.commit_segments(&commit) {
            Ok(receipt) => return Ok((write, receipt)),
            Err(StoreError::Conflict { .. }) if retries < MAX_RETRIES => retries += 1,
            Err(error) => return Err(error.into()),
        }
    }
}

/// What a definition did (CT-290).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefineOutcome {
    pub segment: Segment,
    /// The batches that apply a changed predicate: the persons of the subject type divided by
    /// one hundred, rounded up (SEG-023); 0 when the predicate did not change.
    pub batches: u32,
}

/// Define or change a segment (CT-290).
pub fn define(
    store: &dyn Store,
    ctx: &Context,
    def: &Definition,
) -> Result<DefineOutcome, ExecuteError> {
    let mut decided: Option<seg::Defined> = None;
    commit_own(store, &ctx.tenant, &mut |tenant, catalog| {
        let defined = seg::define(tenant, catalog, def.clone(), ctx).map_err(domain)?;
        let write = defined.write.clone();
        decided = Some(defined);
        Ok(write)
    })?;
    let defined = decided.expect("a committed definition was decided");
    let batches = if defined.rebuild {
        let persons = store.segment_subject_count(&ctx.tenant, &defined.segment.subject)?;
        persons.div_ceil(seg::BATCH as u64) as u32
    } else {
        0
    };
    Ok(DefineOutcome {
        segment: defined.segment,
        batches,
    })
}

/// Remove a segment, its memberships, wake rows and batches, with no exit event (CT-291).
pub fn remove(store: &dyn Store, ctx: &Context, id: SegmentId) -> Result<(), ExecuteError> {
    commit_own(store, &ctx.tenant, &mut |tenant, catalog| {
        seg::remove(tenant, catalog, id, ctx).map_err(domain)
    })
    .map(|_| ())
}

/// What one batch did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchOutcome {
    /// The entry and exit events it wrote.
    pub written: usize,
    /// The batch that follows it, or `None` when it was the last.
    pub next: Option<Batch>,
}

/// Run one continuation of a segment at `ctx.now` (CT-297): at most [`seg::BATCH`] persons, in
/// one transaction; a batch of a superseded version writes nothing and is removed (SEG-025); a
/// repeated batch writes no second event, because membership already holds its result.
pub fn run_batch(
    store: &dyn Store,
    ctx: &Context,
    batch: &Batch,
) -> Result<BatchOutcome, ExecuteError> {
    let tenant = &ctx.tenant;
    let mut outcome = BatchOutcome {
        written: 0,
        next: None,
    };
    let mut retries = 0;
    loop {
        let header = store.load(tenant, &[])?;
        let Some(stored) = header.tenant.as_deref() else {
            return Ok(outcome);
        };
        let catalog = store.segment_catalog(tenant)?;
        // A batch of a superseded version writes nothing (SEG-025): the row of its scope already
        // belongs to the newer version, which runs its own batches.
        if !seg::batch_current(&catalog, batch) {
            return Ok(outcome);
        }
        let (write, next) = {
            let segment = catalog
                .iter()
                .find(|s| s.id == batch.segment)
                .expect("a current batch has its segment");
            let (records, due) = match seg::batch_load(segment, batch, ctx.now) {
                BatchLoad::Page {
                    object,
                    after,
                    limit,
                } => {
                    let state = store.load(
                        tenant,
                        &[Need::PageOf {
                            object: object.clone(),
                            after,
                            limit,
                        }],
                    )?;
                    let records: Vec<Arc<Record>> = state
                        .tenant
                        .as_deref()
                        .map(|t| t.records_of(&object).cloned().collect())
                        .unwrap_or_default();
                    (records, Vec::new())
                }
                BatchLoad::Referencing {
                    object,
                    field,
                    record,
                    after,
                    limit,
                } => (
                    store.segment_referencing(
                        tenant,
                        &object,
                        &field,
                        record,
                        after,
                        limit as usize,
                    )?,
                    Vec::new(),
                ),
                BatchLoad::Due { segment, at, limit } => (
                    Vec::new(),
                    store.segment_due(tenant, segment, at, limit as usize)?,
                ),
            };
            let (targets, follow) = seg::batch_targets(&catalog, batch, &records, &due, ctx.now);
            let first: Vec<Need> = records
                .iter()
                .map(|r| Need::Record {
                    object: r.object.clone(),
                    id: r.id,
                })
                .collect();
            let state = load_for(store, tenant, &catalog, &targets, first, &|s| Ok(s))?;
            let rows = store.segment_rows(tenant, &seg::pairs(&targets))?;
            let occasion = Occasion {
                reason: batch.reason,
                hop: batch.hop,
                revision: stored.revision,
                now: ctx.now,
            };
            let decided = seg::decide_write(&catalog, &state, &targets, &rows, &occasion, ctx)
                .map_err(domain)?;
            let mut write = decided.write;
            write
                .expect
                .entry(batch.segment)
                .or_insert(Some(batch.version));
            let next = match &follow {
                BatchChange::Put(next) => Some(next.clone()),
                BatchChange::Remove { .. } => None,
            };
            write.batches.push(follow);
            (write, next)
        };
        let written = write.events.len();
        let commit = SegmentCommit {
            tenant: tenant.clone(),
            revision: stored.revision,
            write,
        };
        match store.commit_segments(&commit) {
            Ok(_) => {
                outcome.written = written;
                outcome.next = next;
                return Ok(outcome);
            }
            Err(StoreError::Conflict { .. }) if retries < MAX_RETRIES => retries += 1,
            Err(error) => return Err(error.into()),
        }
    }
}

/// Run the pending continuations of a tenant, at most `max` batches, oldest scope first: what
/// the job worker does, one job per batch. Returns the events written.
pub fn run_pending(store: &dyn Store, ctx: &Context, max: usize) -> Result<usize, ExecuteError> {
    let mut written = 0;
    for _ in 0..max {
        let pending = store.segment_batches(&ctx.tenant)?;
        let Some(batch) = pending.into_iter().next() else {
            break;
        };
        written += run_batch(store, ctx, &batch)?.written;
    }
    Ok(written)
}

/// Start the refresh of a time-dependent segment at `ctx.now` (SEG-018, SEG-019, SEG-1545): the
/// continuation the schedule's job hands to [`run_batch`]. `None` for a segment that does not
/// read the time (SEG-020) or whose every person a continuation already evaluates.
pub fn start_refresh(
    store: &dyn Store,
    ctx: &Context,
    id: SegmentId,
) -> Result<Option<Batch>, ExecuteError> {
    let mut started = None;
    commit_own(store, &ctx.tenant, &mut |_, catalog| {
        let pending = store.segment_batches(&ctx.tenant)?;
        let mut write = SegmentWrite::default();
        started = catalog
            .iter()
            .find(|s| s.id == id)
            .and_then(|segment| seg::start_refresh(segment, &pending, ctx.now));
        if let Some(batch) = &started {
            write.expect.insert(batch.segment, Some(batch.version));
            write.batches.push(BatchChange::Put(batch.clone()));
        }
        Ok(write)
    })?;
    Ok(started)
}

/// Record a refresh skipped for want of job quota on its segment (SEG-1544).
pub fn skip_refresh(store: &dyn Store, ctx: &Context, id: SegmentId) -> Result<(), ExecuteError> {
    commit_own(store, &ctx.tenant, &mut |_, catalog| {
        Ok(catalog
            .iter()
            .find(|s| s.id == id)
            .map(|segment| seg::skip_refresh(segment, ctx.now))
            .unwrap_or_default())
    })
    .map(|_| ())
}

/// Remove an erased person from every segment, in one write: their memberships with one exit
/// event each naming them in `sealed` form, their wake rows, and a mark that keeps any later
/// decision from entering them (SEG-036 to SEG-038). The transaction that records the erasure
/// (PRV-353) calls this. Returns the exit events written.
pub fn erase(
    store: &dyn Store,
    ctx: &Context,
    person: &PrincipalId,
    sealed: &PrincipalId,
) -> Result<usize, ExecuteError> {
    let (write, _) = commit_own(store, &ctx.tenant, &mut |tenant, _| {
        let memberships = store.segment_memberships_of(&ctx.tenant, person)?;
        let wakes = store.segment_wakes_of(&ctx.tenant, person)?;
        let occasion = Occasion {
            reason: SegmentReason::Erasure,
            hop: 0,
            revision: tenant.revision,
            now: ctx.now,
        };
        let (mut write, events) = seg::erase(&memberships, &wakes, person, sealed, &occasion);
        write.events = seg::stamp(&events, ctx);
        Ok(write)
    })?;
    Ok(write.events.len())
}

// ---------------------------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------------------------

/// Answer a read from the store, loading what the kernel finds missing, as [`crate::query`] does.
fn read_with<T>(
    store: &dyn Store,
    tenant: &TenantId,
    first: Vec<Need>,
    answer: &dyn Fn(&State) -> Result<T, DomainError>,
) -> Result<T, ExecuteError> {
    let mut wanted: BTreeSet<Need> = first.into_iter().collect();
    let mut rounds = 0;
    loop {
        let list: Vec<Need> = wanted.iter().cloned().collect();
        let state = store.load(tenant, &list)?;
        match answer(&state) {
            Err(DomainError {
                kind: ErrorKind::InsufficientState,
                missing: Some(need),
                ..
            }) if rounds < 32 && !wanted.contains(&need) => {
                wanted.insert(need);
                rounds += 1;
            }
            other => return other.map_err(domain),
        }
    }
}

fn find(catalog: &[Segment], id: SegmentId) -> Result<Segment, ExecuteError> {
    catalog.iter().find(|s| s.id == id).cloned().ok_or_else(|| {
        domain(DomainError::new(
            ErrorKind::UnknownRecord,
            format!("no segment {id}"),
        ))
    })
}

fn by_principal(segment: &Segment, people: &[Membership]) -> Vec<Need> {
    people
        .iter()
        .map(|m| Need::ByField {
            object: segment.subject.clone(),
            field: segment.principal_field.clone(),
            value: strata_kernel::Value::Str(m.person.as_str().to_string()),
        })
        .collect()
}

/// One page of the members of a segment, as the caller may see them (CT-292, SEG-033).
pub fn members(
    store: &dyn Store,
    ctx: &Context,
    id: SegmentId,
    after: Option<&PrincipalId>,
    limit: Option<u32>,
) -> Result<(Vec<SegmentMember>, Option<PrincipalId>), ExecuteError> {
    let catalog = store.segment_catalog(&ctx.tenant)?;
    let segment = find(&catalog, id)?;
    let limit = limit
        .unwrap_or(seg::DEFAULT_MEMBERS_PAGE)
        .clamp(1, seg::MAX_MEMBERS_PAGE) as usize;
    let rows = store.segment_members(&ctx.tenant, id, after, limit + 1)?;
    let needs = by_principal(&segment, &rows);
    read_with(store, &ctx.tenant, needs, &|state| {
        seg::members_page(state, &ctx.principal, &segment, &rows, limit, ctx.now)
    })
}

/// Count the members of a segment the caller may read (CT-814, SEG-1541).
pub fn count(
    store: &dyn Store,
    ctx: &Context,
    id: SegmentId,
) -> Result<SegmentCount, ExecuteError> {
    let catalog = store.segment_catalog(&ctx.tenant)?;
    let segment = find(&catalog, id)?;
    let header = store.load(&ctx.tenant, &[])?;
    let tenant = header.tenant.as_deref().ok_or_else(|| {
        domain(DomainError::new(
            ErrorKind::TenantNotFound,
            "no such tenant",
        ))
    })?;
    seg::may_read_members(tenant, &ctx.principal, &segment).map_err(domain)?;
    if seg::count_plan(tenant, &ctx.principal, &segment) == seg::CountPlan::Kept {
        return seg::count(&header, &ctx.principal, &segment, None, ctx.now).map_err(domain);
    }
    let rows = store.segment_members(&ctx.tenant, id, None, seg::COUNT_SCAN_BOUND + 1)?;
    if rows.len() > seg::COUNT_SCAN_BOUND {
        return seg::count(&header, &ctx.principal, &segment, Some(&rows), ctx.now).map_err(domain);
    }
    let needs = by_principal(&segment, &rows);
    read_with(store, &ctx.tenant, needs, &|state| {
        seg::count(state, &ctx.principal, &segment, Some(&rows), ctx.now)
    })
}

/// The visible segments the caller belongs to (CT-813, SEG-1540).
pub fn mine(store: &dyn Store, ctx: &Context) -> Result<Vec<SegmentSummary>, ExecuteError> {
    let Principal::User(person) = &ctx.principal else {
        return seg::my_segments(&[], &ctx.principal, &[]).map_err(domain);
    };
    let catalog = store.segment_catalog(&ctx.tenant)?;
    let memberships = store.segment_memberships_of(&ctx.tenant, person)?;
    seg::my_segments(&catalog, &ctx.principal, &memberships).map_err(domain)
}

/// Whether a person is a member of a segment now: the membership row, as the commits that move
/// it keep it. For the platform's own use (a send to a segment, D-6561); a caller's read goes
/// through [`members`] or [`mine`], which apply the policies.
pub fn is_member(
    store: &dyn Store,
    tenant: &TenantId,
    id: SegmentId,
    person: &PrincipalId,
) -> StoreResult<bool> {
    let rows = store.segment_rows(tenant, &[(id, person.clone())])?;
    Ok(rows.memberships.contains_key(&(id, person.clone())))
}

/// Test a draft predicate against one person (CT-293, SEG-035).
pub fn test(
    store: &dyn Store,
    ctx: &Context,
    subject: &ObjectTypeId,
    principal_field: &FieldId,
    predicate: &Expr,
    person: &PrincipalId,
) -> Result<SegmentTestResult, ExecuteError> {
    let header = store.load(&ctx.tenant, &[])?;
    let tenant = header.tenant.as_deref().ok_or_else(|| {
        domain(DomainError::new(
            ErrorKind::TenantNotFound,
            "no such tenant",
        ))
    })?;
    let (draft, reads) =
        seg::draft(tenant, &ctx.principal, subject, principal_field, predicate).map_err(domain)?;
    let target = Target {
        segment: draft.id,
        person: person.clone(),
        locate: seg::Locate::Principal,
    };
    let state = load_for(
        store,
        &ctx.tenant,
        std::slice::from_ref(&draft),
        std::slice::from_ref(&target),
        Vec::new(),
        &|s| Ok(s),
    )?;
    seg::test_predicate(&state, &ctx.principal, &draft, &reads, person, ctx.now).map_err(domain)
}

/// Preview a draft predicate over the persons after a cursor (CT-815, SEG-1542).
pub fn preview(
    store: &dyn Store,
    ctx: &Context,
    subject: &ObjectTypeId,
    principal_field: &FieldId,
    predicate: &Expr,
    after: Option<RecordId>,
    limit: Option<u32>,
) -> Result<(SegmentPreview, Option<RecordId>), ExecuteError> {
    let header = store.load(&ctx.tenant, &[])?;
    let tenant = header.tenant.as_deref().ok_or_else(|| {
        domain(DomainError::new(
            ErrorKind::TenantNotFound,
            "no such tenant",
        ))
    })?;
    let (draft, _) =
        seg::draft(tenant, &ctx.principal, subject, principal_field, predicate).map_err(domain)?;
    let limit = limit.unwrap_or(seg::MAX_PREVIEW).clamp(1, seg::MAX_PREVIEW);
    let page = Need::PageOf {
        object: subject.clone(),
        after,
        limit: limit + 1,
    };
    let first = store.load(&ctx.tenant, std::slice::from_ref(&page))?;
    let records: Vec<Arc<Record>> = first
        .tenant
        .as_deref()
        .map(|t| t.records_of(subject).cloned().collect())
        .unwrap_or_default();
    let mut needs = vec![page];
    needs.extend(seg::record_needs(&draft, &first, &records));
    read_with(store, &ctx.tenant, needs, &|state| {
        seg::preview(state, &ctx.principal, &draft, &records, limit, ctx.now)
    })
}

/// Commit for an accepted transition with its segment write: [`Commit::of`] and the write, as
/// [`crate::execute_by`] commits them. For a caller that decided the transition itself.
pub fn commit_change(
    store: &dyn Store,
    intent: &strata_kernel::Intent,
    ctx: &Context,
    transition: &Transition,
) -> Result<Receipt, ExecuteError> {
    let decided = decide_change(store, transition, ctx, 0)?;
    Ok(store.commit_with_segments(&Commit::of(intent, ctx, transition), &decided.write)?)
}

/// The conflict a store answers when a segment write no longer rests on what it holds. A
/// conflict carries no revisions here (the tenant did not move); the caller decides again.
pub fn write_conflict() -> StoreError {
    StoreError::Conflict {
        expected: None,
        found: None,
    }
}
