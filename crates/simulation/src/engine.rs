//! The discrete-event engine. Single-threaded, logical time, no wall clock.
//!
//! It executes `strata_kernel::World::step`, the same composition of `transition` and
//! `apply_delta` that production commits. There is no second copy of any rule here: the engine
//! only decides *when* an intent runs and what the simulated environment does around it.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use strata_kernel::{
    canon, Check, Context, DomainEvent, Effect, ErrorKind, EventBody, Intent, Output, TenantId,
    World, KERNEL_VERSION,
};
use strata_verification::invariants::{check_world, InvariantResult};

/// SplitMix64. Small, fast, and entirely determined by its seed.
#[derive(Clone, Debug)]
pub struct Rng(pub u64);

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number below `bound` (which must not be zero).
    pub fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound
    }

    pub fn chance(&mut self, per_million: u32) -> bool {
        self.below(1_000_000) < u64::from(per_million)
    }
}

/// A decision of the simulated environment, as a pure function of the seed and of what the
/// decision is about. Because it does not depend on how many decisions came before, a branch
/// that runs one extra intent still sees the same faults at the same places as its baseline.
fn decide(seed: u64, kind: u64, subject: u64, attempt: u64, per_million: u32) -> bool {
    let mut rng = Rng(seed ^ kind.wrapping_mul(0xa24b_aed4_963e_e407) ^ subject.wrapping_mul(0x9fb2_1c65_1e98_df25) ^ attempt.wrapping_mul(0xd6e8_feb8_6659_fd93));
    rng.next_u64();
    rng.chance(per_million)
}

/// Faults the environment injects, in events per million.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Faults {
    /// A commit loses a race and the intent is decided again against fresh state.
    pub commit_conflict: u32,
    /// Delivery of an event to the subscriber fails and is retried later.
    pub delivery_drop: u32,
}

/// One intent on the schedule, with the exact context it runs in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scheduled {
    pub label: String,
    pub context: Context,
    pub intent: Intent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scenario {
    pub name: String,
    pub seed: u64,
    pub initial: World,
    /// Run in order of `context.now`; ties keep their order here.
    pub schedule: Vec<Scheduled>,
    pub faults: Faults,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventSummary {
    pub tenant: TenantId,
    pub seq: u64,
    pub kind: String,
    pub object: Option<String>,
    pub id: Option<u64>,
}

pub fn summarise(event: &DomainEvent) -> EventSummary {
    let (kind, object, id) = match &event.body {
        EventBody::TenantCreated { .. } => ("tenant_created", None, None),
        EventBody::SchemaChanged { .. } => ("schema_changed", None, None),
        EventBody::MemberChanged { .. } => ("member_changed", None, None),
        EventBody::RecordCreated { record } => ("record_created", Some(record.object.to_string()), Some(record.id.0)),
        EventBody::RecordUpdated { after, .. } => ("record_updated", Some(after.object.to_string()), Some(after.id.0)),
        EventBody::RecordDeleted { before } => ("record_deleted", Some(before.object.to_string()), Some(before.id.0)),
    };
    EventSummary { tenant: event.tenant.clone(), seq: event.seq, kind: kind.to_string(), object, id }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Outcome {
    Accepted {
        revision: u64,
        delta_hash: String,
        events: Vec<EventSummary>,
        effects: Vec<Effect>,
        output: Output,
        checks_passed: usize,
        /// Checks that failed without stopping the transition (grandfathered records).
        warnings: Vec<Check>,
    },
    Rejected { kind: ErrorKind, message: String, failed: Vec<Check> },
}

impl Outcome {
    pub fn accepted(&self) -> bool {
        matches!(self, Outcome::Accepted { .. })
    }

    /// `accepted`, or `rejected: <kind>`.
    pub fn verdict(&self) -> String {
        match self {
            Outcome::Accepted { .. } => "accepted".to_string(),
            Outcome::Rejected { kind, .. } => format!("rejected: {kind:?}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// Position in the scenario's schedule. An injected change has none.
    pub index: Option<usize>,
    pub at: i64,
    pub label: String,
    pub context: Context,
    pub intent: Intent,
    /// How many simulated commit conflicts preceded the decision recorded here.
    pub conflicts: u32,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryResult {
    Delivered,
    Dropped,
}

/// The simulated completion of a `Publish` effect.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    pub at: i64,
    pub tenant: TenantId,
    pub seq: u64,
    pub attempt: u32,
    pub result: DeliveryResult,
}

/// Everything needed to run an execution again and to check that it came out the same.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trace {
    pub kernel_version: String,
    pub scenario: String,
    pub seed: u64,
    pub faults: Faults,
    /// Canonical hash of the initial world. The world itself travels with the scenario.
    pub initial_hash: String,
    pub initial_schema_versions: BTreeMap<TenantId, u64>,
    pub steps: Vec<Step>,
    pub deliveries: Vec<Delivery>,
    pub final_hash: String,
    pub final_schema_versions: BTreeMap<TenantId, u64>,
    pub final_revisions: BTreeMap<TenantId, u64>,
    pub invariants: Vec<InvariantResult>,
}

impl Trace {
    /// Canonical hash of the whole trace. Equal inputs give equal hashes.
    pub fn hash(&self) -> String {
        canon::hash_of(self)
    }

    pub fn accepted(&self) -> usize {
        self.steps.iter().filter(|s| s.index.is_some() && s.outcome.accepted()).count()
    }

    pub fn rejected(&self) -> usize {
        self.steps.iter().filter(|s| s.index.is_some() && !s.outcome.accepted()).count()
    }
}

fn versions(world: &World) -> BTreeMap<TenantId, u64> {
    world.tenants.iter().map(|(id, tenant)| (id.clone(), tenant.schema.version.0)).collect()
}

/// An intent the caller inserts into a run without it being part of the scenario: the proposed
/// change of a counterfactual branch. It runs immediately before the scheduled step `before`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Injection {
    pub before: usize,
    pub scheduled: Scheduled,
}

/// What a run reports while it is going. Returning `false` stops it.
pub type Progress<'a> = &'a mut dyn FnMut(usize, usize) -> bool;

/// The run was stopped by its progress callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

/// Run a scenario. Returns the trace and the final world.
pub fn run(scenario: &Scenario) -> (Trace, World) {
    run_with(scenario, None, &mut |_, _| true).expect("an uncancellable run completes")
}

/// Run a scenario with an optional injected intent, reporting progress.
pub fn run_with(
    scenario: &Scenario,
    injection: Option<&Injection>,
    progress: Progress<'_>,
) -> Result<(Trace, World), Cancelled> {
    let mut world = scenario.initial.clone();
    let mut steps = Vec::new();
    let mut deliveries = Vec::new();
    // Per tenant: committed events waiting for delivery, oldest first, with attempts so far.
    let mut outbox: BTreeMap<TenantId, VecDeque<(u64, u32)>> = BTreeMap::new();
    let mut delivered: BTreeMap<TenantId, u64> = BTreeMap::new();

    let mut order: Vec<usize> = (0..scenario.schedule.len()).collect();
    order.sort_by_key(|i| (scenario.schedule[*i].context.now.0, *i));
    let total = order.len();

    // Deliver waiting events in order until one is dropped. A drop holds back everything
    // behind it, exactly as an unacknowledged outbox position does.
    let pump = |at: i64,
                final_pass: bool,
                outbox: &mut BTreeMap<TenantId, VecDeque<(u64, u32)>>,
                delivered: &mut BTreeMap<TenantId, u64>,
                deliveries: &mut Vec<Delivery>| {
        for (tenant, queue) in outbox.iter_mut() {
            while let Some((seq, attempt)) = queue.front().copied() {
                let subject = seq ^ (tenant.as_str().len() as u64) << 48;
                let dropped = attempt < 5 && decide(scenario.seed, 2, subject, u64::from(attempt), scenario.faults.delivery_drop);
                let result = if dropped { DeliveryResult::Dropped } else { DeliveryResult::Delivered };
                deliveries.push(Delivery { at, tenant: tenant.clone(), seq, attempt, result: result.clone() });
                if dropped {
                    queue.front_mut().expect("front exists").1 += 1;
                    if !final_pass {
                        break;
                    }
                } else {
                    queue.pop_front();
                    delivered.insert(tenant.clone(), seq);
                }
            }
        }
    };

    let mut execute = |index: Option<usize>, scheduled: &Scheduled, world: &mut World, steps: &mut Vec<Step>| {
        let subject = index.map_or(u64::MAX, |i| i as u64);
        let mut conflicts = 0u32;
        // A simulated lost race: the decision is discarded and made again. With one writer the
        // state has not changed, so the second decision equals the first; what the fault shows
        // is that a retry is harmless.
        while conflicts < 3 && index.is_some() && decide(scenario.seed, 1, subject, u64::from(conflicts), scenario.faults.commit_conflict) {
            let _ = world.step(&scheduled.intent, &scheduled.context);
            conflicts += 1;
        }
        let outcome = match world.step(&scheduled.intent, &scheduled.context) {
            Ok((next, transition)) => {
                *world = next;
                let queue = outbox.entry(scheduled.context.tenant.clone()).or_default();
                for effect in &transition.effects {
                    if let Effect::Publish { seq } = effect {
                        queue.push_back((*seq, 0));
                    }
                }
                Outcome::Accepted {
                    revision: transition.delta.new_revision.0,
                    delta_hash: canon::hash_of(&transition.delta),
                    events: transition.events.iter().map(summarise).collect(),
                    effects: transition.effects.clone(),
                    output: transition.output.clone(),
                    checks_passed: transition.evidence.iter().filter(|c| c.passed).count(),
                    warnings: transition.evidence.iter().filter(|c| !c.passed).cloned().collect(),
                }
            }
            Err(error) => Outcome::Rejected {
                kind: error.kind,
                message: error.message,
                failed: error.evidence.into_iter().filter(|c| !c.passed).collect(),
            },
        };
        steps.push(Step {
            index,
            at: scheduled.context.now.0,
            label: scheduled.label.clone(),
            context: scheduled.context.clone(),
            intent: scheduled.intent.clone(),
            conflicts,
            outcome,
        });
        pump(scheduled.context.now.0, false, &mut outbox, &mut delivered, &mut deliveries);
    };

    for (done, index) in order.iter().enumerate() {
        if !progress(done, total) {
            return Err(Cancelled);
        }
        if let Some(injection) = injection.filter(|injection| injection.before == *index) {
            execute(None, &injection.scheduled, &mut world, &mut steps);
        }
        execute(Some(*index), &scenario.schedule[*index], &mut world, &mut steps);
    }
    if let Some(injection) = injection.filter(|injection| injection.before >= total) {
        execute(None, &injection.scheduled, &mut world, &mut steps);
    }
    drop(execute);
    let end = steps.last().map_or(0, |step| step.at) + 1;
    pump(end, true, &mut outbox, &mut delivered, &mut deliveries);
    progress(total, total);

    // Delivery is part of what a run must get right: every committed event reaches the
    // subscriber exactly once, in order, whatever was dropped along the way.
    let mut invariants = check_world(&world);
    let mut delivery_problems = Vec::new();
    for (tenant, queue) in &outbox {
        let got: Vec<u64> = deliveries
            .iter()
            .filter(|d| d.tenant == *tenant && d.result == DeliveryResult::Delivered)
            .map(|d| d.seq)
            .collect();
        if !queue.is_empty() {
            delivery_problems.push(format!("{tenant}: {} event(s) never delivered", queue.len()));
        }
        if got.windows(2).any(|pair| pair[1] != pair[0] + 1) {
            delivery_problems.push(format!("{tenant}: delivered out of order or with a gap: {got:?}"));
        }
        let committed = world.tenants.get(tenant).map_or(0, |t| t.event_seq);
        if got.last().copied().unwrap_or(committed) != committed {
            delivery_problems.push(format!("{tenant}: last delivered {:?}, last committed {committed}", got.last()));
        }
    }
    invariants.push(InvariantResult {
        id: "events_delivered_once_in_order".into(),
        statement: "Every committed event is delivered to the subscriber exactly once, in sequence order".into(),
        holds: delivery_problems.is_empty(),
        violation_count: delivery_problems.len(),
        violations: delivery_problems,
        class: strata_verification::evidence::Class::Reproduced,
    });

    let trace = Trace {
        kernel_version: KERNEL_VERSION.to_string(),
        scenario: scenario.name.clone(),
        seed: scenario.seed,
        faults: scenario.faults,
        initial_hash: scenario.initial.hash(),
        initial_schema_versions: versions(&scenario.initial),
        steps,
        deliveries,
        final_hash: world.hash(),
        final_schema_versions: versions(&world),
        final_revisions: world.tenants.iter().map(|(id, t)| (id.clone(), t.revision.0)).collect(),
        invariants,
    };
    Ok((trace, world))
}

/// The result of checking a trace against the kernel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replay {
    pub steps: usize,
    pub agreed: usize,
    /// The first step whose replayed outcome differs from the recorded one.
    pub first_mismatch: Option<usize>,
    pub final_hash: String,
    pub final_hash_matches: bool,
    pub kernel_version_matches: bool,
}

/// Re-execute a trace's recorded intents and contexts directly against the kernel, with no
/// engine, no schedule and no faults, and compare every outcome with the recorded one.
///
/// A trace that passes is self-contained: its steps alone reproduce its final state.
pub fn replay(initial: &World, trace: &Trace) -> Replay {
    let mut world = initial.clone();
    let mut agreed = 0;
    let mut first_mismatch = None;
    for (position, step) in trace.steps.iter().enumerate() {
        let same = match (world.step(&step.intent, &step.context), &step.outcome) {
            (Ok((next, transition)), Outcome::Accepted { delta_hash, .. }) => {
                world = next;
                canon::hash_of(&transition.delta) == *delta_hash
            }
            (Err(error), Outcome::Rejected { kind, .. }) => error.kind == *kind,
            (Ok((next, _)), _) => {
                world = next;
                false
            }
            (Err(_), _) => false,
        };
        if same {
            agreed += 1;
        } else if first_mismatch.is_none() {
            first_mismatch = Some(position);
        }
    }
    let final_hash = world.hash();
    Replay {
        steps: trace.steps.len(),
        agreed,
        first_mismatch,
        final_hash_matches: final_hash == trace.final_hash && initial.hash() == trace.initial_hash,
        final_hash,
        kernel_version_matches: trace.kernel_version == KERNEL_VERSION,
    }
}
