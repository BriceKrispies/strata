//! The shell path: a simulated deployment that runs the deployment's own code.
//!
//! A deployed backend takes a request through its request path (checks, the store contract's
//! `execute`, the commit, the commit hook), runs jobs with its automation worker (wake index,
//! lease, run, report), publishes committed events with its outbox publisher (the generated
//! outbox machine on its runtime host), and calls outside services through its adapters. This
//! module drives exactly that code, through the [`Deploy`] and [`Process`] traits that the
//! backend implements (`strata_backend::simulated`), over:
//!
//! - **the store contract's in-memory adapter**, behind [`FaultyStore`], which injects the
//!   faults a store shows in a deployment;
//! - **a logical clock** ([`LogicalClock`]) that every clock read of the process goes through
//!   (the backend's `App::now`, the outbox machine's timers and retries, the worker's leases);
//! - **seeded randomness** (`Environment::entropy_seed`), which the process installs as the
//!   source of every random value it makes for its domain;
//! - **adapters the simulation answers** ([`Adapters`]): a webhook's transport and the mail
//!   service succeed, fail or time out as the seed decides.
//!
//! The engine here decides only *when* things happen and what the environment does: when a
//! request arrives (once, twice, late, or again after the client was told the service is
//! unavailable), when the worker makes a pass, when the publisher's timer fires, when a process
//! stops and is started again over the same store. Every answer, job outcome, publication and
//! final state in a [`ShellTrace`] is what the backend's code produced. Nothing here imitates a
//! rule of the domain, of the store, of the worker or of the publisher.
//!
//! Recorded workloads ([`Recording`]) are the journal the store keeps: a run of the deployment is
//! recorded by reading its journal, and replaying the recording through a fresh deployment must
//! give the recorded outcomes ([`replay_recording`]).

use crate::engine::{decide, summarise, EventSummary, Rng, Scheduled};
use crate::faults::{FaultyStore, Injected, StoreFaults};
use crate::journal::{JournalReplay, Recorded};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use strata_kernel::ext::*;
use strata_kernel::{
    canon, Context, DomainEvent, Intent, Output, PrincipalId, TenantId, TenantState, Timestamp,
    World, KERNEL_VERSION,
};
use strata_storage_api::jobs::JobState;
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{Generation, SeededNonces, Store};
use strata_verification::evidence::Class;
use strata_verification::invariants::{check_world, InvariantResult};

/// How long after it was sent a delayed request arrives.
pub const DELAY_MS: i64 = 30_000;
/// How long a client waits before it sends a request again after being told the service is
/// unavailable, with the same idempotency key.
pub const RETRY_MS: i64 = 5_000;
/// The most times one request is sent (the first time included).
pub const MAX_DELIVERIES: u32 = 6;
/// How long a stalled worker stays stalled before it resumes and sends the report it held:
/// longer than the default lease (60 s), so the job has been offered again meanwhile.
pub const STALL_MS: i64 = 90_000;
/// How far past the last request the run goes on to let jobs, retries and publication finish.
pub const QUIESCE_MS: i64 = 4 * 3_600_000;
/// The publisher's sweep period, as the outbox machine arms it.
const SWEEP_MS: i64 = 500;
/// The most turns the quiescence phase takes.
const MAX_QUIESCE_TURNS: usize = 20_000;

// ---------------------------------------------------------------------------------------------
// The environment a process runs in
// ---------------------------------------------------------------------------------------------

/// Logical time, shared by every reader in a run. It only moves forward.
#[derive(Clone, Debug)]
pub struct LogicalClock(Arc<AtomicI64>);

impl LogicalClock {
    pub fn at(start: i64) -> Self {
        LogicalClock(Arc::new(AtomicI64::new(start)))
    }

    pub fn now(&self) -> Timestamp {
        Timestamp(self.0.load(Ordering::SeqCst))
    }

    /// Move to `to`, or stay where the clock is when `to` is earlier.
    pub fn advance_to(&self, to: i64) {
        self.0.fetch_max(to, Ordering::SeqCst);
    }

    /// The clock as a function, for the readers that take one.
    pub fn reader(&self) -> Arc<dyn Fn() -> Timestamp + Send + Sync> {
        let clock = self.clone();
        Arc::new(move || clock.now())
    }
}

/// An outside service a process calls through an adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Adapter {
    /// A webhook delivery: the outbound gate's transport.
    Webhook,
    /// The mail service.
    Mail,
}

/// What an outside service does with one call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Answers,
    Fails,
    TimesOut,
}

/// The outside services of a simulated process: each call's verdict.
pub trait Adapters: Send + Sync {
    /// The verdict on a call to `adapter` about `subject` (a delivery id, an address).
    fn call(&self, adapter: Adapter, subject: &str) -> Verdict;
}

/// One call an adapter made.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterCall {
    pub at: i64,
    pub adapter: Adapter,
    pub subject: String,
    /// The calls about the same subject before this one.
    pub attempt: u32,
    pub verdict: Verdict,
}

/// Adapters whose verdicts the seed decides, from the subject and the attempt only.
pub struct SeededAdapters {
    seed: u64,
    fails: u32,
    times_out: u32,
    clock: LogicalClock,
    calls: Mutex<Vec<AdapterCall>>,
}

impl SeededAdapters {
    pub fn new(seed: u64, fails: u32, times_out: u32, clock: LogicalClock) -> Self {
        SeededAdapters {
            seed,
            fails,
            times_out,
            clock,
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<AdapterCall> {
        self.calls
            .lock()
            .map(|calls| calls.clone())
            .unwrap_or_default()
    }
}

impl Adapters for SeededAdapters {
    fn call(&self, adapter: Adapter, subject: &str) -> Verdict {
        let Ok(mut calls) = self.calls.lock() else {
            return Verdict::Fails;
        };
        let attempt = calls
            .iter()
            .filter(|call| call.adapter == adapter && call.subject == subject)
            .count() as u32;
        let key = u64::from_str_radix(&canon::hash_bytes(subject.as_bytes())[..16], 16)
            .unwrap_or_default()
            ^ (adapter as u64);
        let verdict = if decide(self.seed, 6, key, u64::from(attempt), self.fails) {
            Verdict::Fails
        } else if decide(self.seed, 7, key, u64::from(attempt), self.times_out) {
            Verdict::TimesOut
        } else {
            Verdict::Answers
        };
        calls.push(AdapterCall {
            at: self.clock.now().0,
            adapter,
            subject: subject.to_string(),
            attempt,
            verdict,
        });
        verdict
    }
}

/// What a process is started over.
pub struct Environment {
    /// The store the process writes through, as its adapter.
    pub store: Arc<dyn Store>,
    pub clock: LogicalClock,
    pub adapters: Arc<dyn Adapters>,
    /// The seed of every random value the process makes.
    pub entropy_seed: u64,
    /// Whether the process installs the tenants of its configuration when it starts, as a
    /// deployed process does.
    pub install: bool,
    /// The tenants whose published events are listened to, from before the process publishes
    /// anything.
    pub listen: Vec<TenantId>,
}

/// A deployment that the simulator can start.
pub trait Deploy {
    /// Start a process over `env`: what a deployed process does when it starts (install the
    /// configured tenants, offer the publisher what was committed before), and nothing else.
    fn start(&self, env: &Environment) -> Result<Box<dyn Process>, String>;
}

/// The answer to a request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
pub enum Answer {
    Committed {
        revision: u64,
        delta_hash: String,
        events_hash: String,
        events: Vec<EventSummary>,
        output: Output,
        /// Commits that lost a race and were decided again.
        conflicts: usize,
        /// The first answer recorded under the request's idempotency key: nothing was committed.
        replayed: bool,
    },
    Refused {
        /// The failure class (`unavailable`, `conflict`, `invalid`, ...).
        code: String,
        /// The kernel's rejection kind, when the kernel rejected the request.
        kind: Option<String>,
        message: String,
    },
}

impl Answer {
    /// Whether the client was told the service is unavailable, and may send the request again.
    pub fn unavailable(&self) -> bool {
        matches!(self, Answer::Refused { code, .. } if code == "unavailable")
    }

    /// `committed`, `replayed` or `refused: <code>/<kind>`.
    pub fn verdict(&self) -> String {
        match self {
            Answer::Committed {
                replayed: false, ..
            } => "committed".into(),
            Answer::Committed { replayed: true, .. } => "replayed".into(),
            Answer::Refused { code, kind, .. } => {
                format!("refused: {code}/{}", kind.as_deref().unwrap_or("-"))
            }
        }
    }
}

/// One running process of the deployment.
pub trait Process {
    /// The request path after authentication: the same function a request handler calls.
    fn request(&self, intent: &Intent, context: &Context) -> Answer;
    /// One pass of the automation worker at the clock's time. Returns the jobs it ran.
    fn work(&self) -> usize;
    /// The commit hook: offer the publisher the commit and the kick, and deliver what follows.
    fn committed(&self) -> Result<(), String>;
    /// One turn of the publisher's host: the timers due at the clock's time.
    fn sweep(&self) -> Result<(), String>;
    /// The events the listened-to tenants' subscribers heard since the last call, in order.
    fn heard(&self) -> Vec<DomainEvent>;
    /// The tenant as the process reads it.
    fn state(&self, tenant: &TenantId) -> Result<Option<Arc<TenantState>>, String>;
    /// The tenant's journal, as the store holds it.
    fn journal(&self, tenant: &TenantId) -> Result<Vec<Recorded>, String>;
    /// The backend's own replay of the tenant's journal against its live state.
    fn replay_journal(&self, tenant: &TenantId) -> Result<JournalReplay, String>;
    /// A named operation of the deployment that is not an intent, as `principal` of `tenant`
    /// (such as creating a webhook subscription), with its JSON body.
    fn operate(
        &self,
        operation: &str,
        tenant: &TenantId,
        principal: &PrincipalId,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, String>;
}

// ---------------------------------------------------------------------------------------------
// Scenarios and traces
// ---------------------------------------------------------------------------------------------

/// Faults of the environment, in events per million.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Faults {
    pub store: StoreFaults,
    /// A request is delivered twice.
    pub request_repeated: u32,
    /// A request arrives [`DELAY_MS`] late, after requests sent after it.
    pub request_delayed: u32,
    /// A call to an outside service fails.
    pub adapter_fails: u32,
    /// A call to an outside service times out.
    pub adapter_times_out: u32,
    /// The process stops right after a request was answered, before the commit hook, and is
    /// started again over the same store.
    pub crash: u32,
}

/// A run of the deployment.
#[derive(Clone, Debug)]
pub struct ShellScenario {
    pub name: String,
    pub seed: u64,
    /// The logical time the process starts at.
    pub start: i64,
    /// Requests, each arriving at its context's time; ties keep their order here.
    pub schedule: Vec<Scheduled>,
    pub faults: Faults,
    /// The tenants whose publications, jobs and journals the run checks.
    pub listen: Vec<TenantId>,
    /// Give each request an idempotency key, as a client that retries does.
    pub keys: bool,
    /// The store faults while the first process starts (installs its tenants); `faults.store`
    /// applies from then on.
    pub start_faults: StoreFaults,
    /// Operations of the deployment that are not intents, each made at its time (before a
    /// request of the same time), with what they answered recorded in the trace.
    pub operations: Vec<Operation>,
}

/// A named operation of the deployment ([`Process::operate`]), made at `at`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operation {
    pub at: i64,
    pub operation: String,
    pub tenant: TenantId,
    pub principal: PrincipalId,
    pub body: serde_json::Value,
}

/// How a request reached the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arrival {
    First,
    /// The same message again, right after the first.
    Repeated,
    /// Late.
    Delayed,
    /// Sent again by the client after an unavailable answer.
    Retried,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellStep {
    pub index: usize,
    pub arrival: Arrival,
    /// How many times this request was sent before.
    pub delivery: u32,
    pub at: i64,
    pub label: String,
    pub intent: String,
    pub key: Option<String>,
    pub answer: Answer,
}

/// What an operation answered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationResult {
    pub at: i64,
    pub operation: String,
    pub answer: Result<serde_json::Value, String>,
}

/// A finished run: its trace, the listened-to tenants as the process read them at the end, and
/// each listened-to tenant's journal as a [`Recording`].
#[derive(Clone, Debug)]
pub struct ShellRun {
    pub trace: ShellTrace,
    pub world: World,
    pub recordings: BTreeMap<TenantId, Recording>,
    /// The generation each tenant holds at the end. The store draws its nonces from the
    /// scenario's seed, so two runs of one scenario end at the same generations.
    pub generations: BTreeMap<TenantId, Generation>,
}

/// Requests to send after an operation, made from what it answered (such as a definition that
/// names the subscription an operation created). Each is sent at its context's time.
pub type FollowUp<'a> =
    &'a dyn Fn(&Operation, &Result<serde_json::Value, String>) -> Vec<Scheduled>;

/// An event a subscriber heard.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heard {
    pub at: i64,
    pub event: EventSummary,
}

/// What became of a report a stalled worker sent when it resumed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LateReport {
    pub at: i64,
    pub stalled_at: i64,
    pub tenant: TenantId,
    pub job: String,
    pub token: u64,
    /// `recorded`, `refused: <why>` or `failed: <error>`.
    pub result: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellTrace {
    pub kernel_version: String,
    pub scenario: String,
    pub seed: u64,
    pub faults: Faults,
    /// What each operation answered, in the order they were made.
    pub operations: Vec<OperationResult>,
    pub steps: Vec<ShellStep>,
    pub late_reports: Vec<LateReport>,
    pub heard: Vec<Heard>,
    /// When the process was stopped and started again.
    pub restarts: Vec<i64>,
    /// Jobs the worker ran, over all passes.
    pub jobs_run: usize,
    pub injected: Vec<Injected>,
    pub adapter_calls: Vec<AdapterCall>,
    /// The listened-to tenants' jobs at the end, counted by state.
    pub jobs: BTreeMap<TenantId, BTreeMap<String, usize>>,
    /// Whether nothing was left to do (no event unpublished, no job due or leased, no report
    /// held) before [`QUIESCE_MS`] passed.
    pub quiesced: bool,
    pub ended_at: i64,
    pub final_hashes: BTreeMap<TenantId, String>,
    pub final_revisions: BTreeMap<TenantId, u64>,
    /// The backend's replay of each listened-to tenant's journal against its live state.
    pub journal: Vec<JournalReplay>,
    pub invariants: Vec<InvariantResult>,
}

impl ShellTrace {
    /// Canonical hash of the whole trace. Equal runs give equal hashes.
    pub fn hash(&self) -> String {
        canon::hash_of(self)
    }

    /// The invariants that do not hold.
    pub fn violations(&self) -> Vec<&InvariantResult> {
        self.invariants.iter().filter(|i| !i.holds).collect()
    }

    /// The last answer each scheduled request got, by its index.
    pub fn last_answers(&self) -> BTreeMap<usize, &Answer> {
        let mut last = BTreeMap::new();
        for step in &self.steps {
            last.insert(step.index, &step.answer);
        }
        last
    }
}

// ---------------------------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------------------------

/// The kinds of environment decision the engine makes (the store's start at 16).
const DELAYED: u64 = 3;
const REPEATED: u64 = 4;
const CRASH: u64 = 5;

#[derive(Clone, Copy, Debug)]
enum Item {
    Operation(usize),
    Request(Pending),
}

#[derive(Clone, Copy, Debug)]
struct Pending {
    at: i64,
    index: usize,
    delivery: u32,
    arrival: Arrival,
}

/// A run in progress: the store, the clock, the process and what has been observed.
struct Run<'a> {
    deploy: &'a dyn Deploy,
    scenario: &'a ShellScenario,
    clock: LogicalClock,
    store: Arc<FaultyStore>,
    adapters: Arc<SeededAdapters>,
    process: Option<Box<dyn Process>>,
    generation: u64,
    commits_seen: u64,
    heard: Vec<Heard>,
    late_reports: Vec<LateReport>,
    restarts: Vec<i64>,
    jobs_run: usize,
}

fn environment(
    store: &Arc<FaultyStore>,
    clock: &LogicalClock,
    adapters: &Arc<SeededAdapters>,
    scenario: &ShellScenario,
    generation: u64,
) -> Environment {
    Environment {
        store: store.clone(),
        clock: clock.clone(),
        adapters: adapters.clone(),
        entropy_seed: scenario.seed ^ generation.wrapping_mul(0x9e37_79b9_7f4a_7c15),
        install: true,
        listen: scenario.listen.clone(),
    }
}

impl Run<'_> {
    fn process(&self) -> &dyn Process {
        self.process.as_deref().expect("a process is running")
    }

    fn now(&self) -> i64 {
        self.clock.now().0
    }

    fn collect_heard(&mut self) {
        let at = self.now();
        for event in self.process().heard() {
            self.heard.push(Heard {
                at,
                event: summarise(&event),
            });
        }
    }

    /// The commit hook, when something was committed since it last ran: the backend offers the
    /// publisher every commit it makes, through the request path and the worker alike.
    fn hook(&mut self) -> Result<(), String> {
        let commits = self.store.commits();
        if commits != self.commits_seen {
            self.commits_seen = commits;
            self.process().committed()?;
        }
        Ok(())
    }

    /// What follows a request, or a turn of the clock: the commit hook, the publisher's due
    /// timers, a worker pass and the hook again for what the worker committed.
    fn settle(&mut self) -> Result<(), String> {
        self.deliver_late_reports();
        self.hook()?;
        self.process().sweep()?;
        self.jobs_run += self.process().work();
        self.hook()?;
        self.collect_heard();
        Ok(())
    }

    /// Reports that stalled workers held long enough are sent now, straight to the adapter, as
    /// the resumed worker would send them.
    fn deliver_late_reports(&mut self) {
        let now = self.clock.now();
        for held in self.store.take_held_reports(now, STALL_MS) {
            let result = match self.store.inner().job_report(
                &held.tenant,
                &held.job,
                held.token,
                &held.report,
                &held.writes,
                now,
            ) {
                Ok(strata_storage_api::jobs::Reported::Recorded(_)) => "recorded".to_string(),
                Ok(strata_storage_api::jobs::Reported::Refused(why)) => format!("refused: {why}"),
                Err(error) => format!("failed: {error}"),
            };
            self.late_reports.push(LateReport {
                at: now.0,
                stalled_at: held.at.0,
                tenant: held.tenant,
                job: held.job,
                token: held.token,
                result,
            });
        }
    }

    /// Stop the process and start a new one over the same store.
    fn restart(&mut self) -> Result<(), String> {
        self.collect_heard();
        self.restarts.push(self.now());
        self.generation += 1;
        let env = environment(
            &self.store,
            &self.clock,
            &self.adapters,
            self.scenario,
            self.generation,
        );
        // The old process is gone before the new one starts: nothing of it, its randomness
        // included, outlives it.
        self.process = None;
        self.process = Some(self.deploy.start(&env)?);
        self.commits_seen = self.store.commits();
        self.collect_heard();
        Ok(())
    }

    /// The listened-to tenants' work left: how many events are not yet published, and the
    /// earliest time a job is due or a lease expires.
    fn remaining(&self) -> (u64, Option<i64>) {
        let inner = self.store.inner();
        let mut unpublished = 0;
        let mut next: Option<i64> = None;
        for tenant in &self.scenario.listen {
            unpublished += inner.outbox_pending_of(tenant).unwrap_or(0);
            for job in inner.jobs_list(tenant, None, 1_000_000).unwrap_or_default() {
                if matches!(job.state, JobState::Waiting | JobState::Leased) {
                    next = Some(next.map_or(job.due_at.0, |n| n.min(job.due_at.0)));
                }
            }
        }
        (unpublished, next)
    }
}

/// Run a scenario through a deployment.
pub fn run_shell(deploy: &dyn Deploy, scenario: &ShellScenario) -> Result<ShellRun, String> {
    run_shell_with(deploy, scenario, &|_, _| Vec::new())
}

/// [`run_shell`], with the requests `follow` makes from each operation's answer sent too.
pub fn run_shell_with(
    deploy: &dyn Deploy,
    scenario: &ShellScenario,
    follow: FollowUp<'_>,
) -> Result<ShellRun, String> {
    let clock = LogicalClock::at(scenario.start);
    let store = Arc::new(FaultyStore::new(
        Arc::new(MemoryStore::with_nonces(SeededNonces::shared(
            scenario.seed,
        ))),
        scenario.seed,
        scenario.start_faults,
        clock.reader(),
    ));
    let adapters = Arc::new(SeededAdapters::new(
        scenario.seed,
        scenario.faults.adapter_fails,
        scenario.faults.adapter_times_out,
        clock.clone(),
    ));
    store.arm(true);
    let process = deploy.start(&environment(&store, &clock, &adapters, scenario, 0))?;
    store.set_faults(scenario.faults.store);
    let mut run = Run {
        deploy,
        scenario,
        commits_seen: store.commits(),
        clock,
        store,
        adapters,
        process: Some(process),
        generation: 0,
        heard: Vec::new(),
        late_reports: Vec::new(),
        restarts: Vec::new(),
        jobs_run: 0,
    };
    run.collect_heard();
    run.settle()?;

    let seed = scenario.seed;
    let faults = scenario.faults;
    let mut schedule: Vec<Scheduled> = scenario.schedule.clone();
    // Ordered by time, then by the order they were queued in: an operation before the requests
    // of its time.
    let mut queue: BTreeMap<(i64, u64), Item> = BTreeMap::new();
    let mut order = 0u64;
    for (index, operation) in scenario.operations.iter().enumerate() {
        queue.insert((operation.at, order), Item::Operation(index));
        order += 1;
    }
    for (index, scheduled) in schedule.iter().enumerate() {
        queue.insert(
            (scheduled.context.now.0, order),
            Item::Request(Pending {
                at: scheduled.context.now.0,
                index,
                delivery: 0,
                arrival: Arrival::First,
            }),
        );
        order += 1;
    }
    let mut steps = Vec::new();
    let mut operations = Vec::new();
    let mut last_at = scenario.start;
    while let Some(((at, _), item)) = queue.pop_first() {
        run.clock.advance_to(at);
        last_at = last_at.max(at);
        let pending = match item {
            Item::Operation(index) => {
                let operation = &scenario.operations[index];
                let answer = run.process().operate(
                    &operation.operation,
                    &operation.tenant,
                    &operation.principal,
                    &operation.body,
                );
                for scheduled in follow(operation, &answer) {
                    let index = schedule.len();
                    let at = scheduled.context.now.0;
                    schedule.push(scheduled);
                    queue.insert(
                        (at, order),
                        Item::Request(Pending {
                            at,
                            index,
                            delivery: 0,
                            arrival: Arrival::First,
                        }),
                    );
                    order += 1;
                }
                operations.push(OperationResult {
                    at: run.now(),
                    operation: operation.operation.clone(),
                    answer,
                });
                run.settle()?;
                continue;
            }
            Item::Request(pending) => pending,
        };
        let index = pending.index;
        let scheduled = schedule[index].clone();
        if pending.arrival == Arrival::First
            && decide(seed, DELAYED, index as u64, 0, faults.request_delayed)
        {
            let at = pending.at + DELAY_MS;
            queue.insert(
                (at, order),
                Item::Request(Pending {
                    at,
                    arrival: Arrival::Delayed,
                    ..pending
                }),
            );
            order += 1;
            continue;
        }
        let mut context = scheduled.context.clone();
        if scenario.keys && context.idempotency_key.is_none() {
            context.idempotency_key = Some(format!("sim-{seed}-{index}"));
        }
        let step = |arrival: Arrival, delivery: u32, answer: Answer, at: i64| ShellStep {
            index,
            arrival,
            delivery,
            at,
            label: scheduled.label.clone(),
            intent: scheduled.intent.name().to_string(),
            key: context.idempotency_key.clone(),
            answer,
        };
        let answer = run.process().request(&scheduled.intent, &context);
        let now = run.now();
        if answer.unavailable()
            && context.idempotency_key.is_some()
            && pending.delivery + 1 < MAX_DELIVERIES
        {
            let at = now + RETRY_MS;
            queue.insert(
                (at, order),
                Item::Request(Pending {
                    at,
                    index,
                    delivery: pending.delivery + 1,
                    arrival: Arrival::Retried,
                }),
            );
            order += 1;
        }
        steps.push(step(pending.arrival, pending.delivery, answer, now));
        if pending.arrival != Arrival::Repeated
            && decide(
                seed,
                REPEATED,
                index as u64,
                u64::from(pending.delivery),
                faults.request_repeated,
            )
        {
            let again = run.process().request(&scheduled.intent, &context);
            steps.push(step(Arrival::Repeated, pending.delivery + 1, again, now));
        }
        // A stop after the answer and before the commit hook: whatever was committed is in the
        // store's outbox and nothing of this process published it.
        if decide(
            seed,
            CRASH,
            index as u64,
            u64::from(pending.delivery),
            faults.crash,
        ) {
            run.restart()?;
        }
        run.settle()?;
    }

    // Let the jobs, the retries and the publication finish, jumping from one due time to the
    // next.
    let horizon = last_at.saturating_add(QUIESCE_MS);
    let mut quiesced = false;
    for _ in 0..MAX_QUIESCE_TURNS {
        run.settle()?;
        let (unpublished, next_job) = run.remaining();
        let held = run.store.holds_reports();
        if unpublished == 0 && next_job.is_none() && !held {
            quiesced = true;
            break;
        }
        let now = run.now();
        let mut next = i64::MAX;
        if unpublished > 0 || held {
            next = next.min(now + SWEEP_MS);
        }
        if let Some(due) = next_job {
            next = next.min(due);
        }
        let next = next.max(now + 1);
        if next > horizon {
            break;
        }
        run.clock.advance_to(next);
    }
    run.collect_heard();

    // What the process reads at the end, and its own checks of it.
    let mut world = World::default();
    let mut final_hashes = BTreeMap::new();
    let mut final_revisions = BTreeMap::new();
    let mut journal = Vec::new();
    let mut jobs = BTreeMap::new();
    for tenant in &scenario.listen {
        if let Some(state) = run.process().state(tenant)? {
            final_hashes.insert(tenant.clone(), canon::hash_of(&Some(state.as_ref())));
            final_revisions.insert(tenant.clone(), state.revision.0);
            world.tenants.insert(tenant.clone(), state);
            journal.push(run.process().replay_journal(tenant)?);
        }
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for job in run
            .store
            .inner()
            .jobs_list(tenant, None, 1_000_000)
            .map_err(|e| e.to_string())?
        {
            *counts.entry(job.state.as_str().to_string()).or_default() += 1;
        }
        jobs.insert(tenant.clone(), counts);
    }

    let mut invariants = check_world(&world);
    invariants.push(journal_invariant(&journal));
    invariants.push(publication_invariant(&scenario.listen, &world, &run.heard));
    invariants.push(keys_invariant(&scenario.listen, run.process())?);

    let trace = ShellTrace {
        kernel_version: KERNEL_VERSION.to_string(),
        scenario: scenario.name.clone(),
        seed,
        faults,
        operations,
        steps,
        late_reports: run.late_reports.clone(),
        heard: run.heard.clone(),
        restarts: run.restarts.clone(),
        jobs_run: run.jobs_run,
        injected: run.store.injected(),
        adapter_calls: run.adapters.calls(),
        jobs,
        quiesced,
        ended_at: run.now(),
        final_hashes,
        final_revisions,
        journal,
        invariants,
    };
    let mut recordings = BTreeMap::new();
    for tenant in world.tenants.keys() {
        recordings.insert(tenant.clone(), record(run.process(), tenant)?);
    }
    let mut generations = BTreeMap::new();
    for tenant in world.tenants.keys() {
        if let Some(held) = run
            .store
            .generation(tenant)
            .map_err(|e| format!("generation of {tenant}: {e}"))?
        {
            generations.insert(tenant.clone(), held);
        }
    }
    Ok(ShellRun {
        trace,
        world,
        recordings,
        generations,
    })
}

fn invariant(id: &str, statement: &str, violations: Vec<String>) -> InvariantResult {
    InvariantResult {
        id: id.into(),
        statement: statement.into(),
        holds: violations.is_empty(),
        violation_count: violations.len(),
        violations,
        class: Class::Reproduced,
    }
}

fn journal_invariant(journal: &[JournalReplay]) -> InvariantResult {
    let violations = journal
        .iter()
        .filter(|report| {
            !report.matches_live
                || report.first_mismatch.is_some()
                || report.agreed + report.redacted.len() != report.entries
        })
        .map(|report| {
            format!(
                "{}: {} of {} journal entries agree, first mismatch {:?}, matches live: {}",
                report.tenant,
                report.agreed,
                report.entries,
                report.first_mismatch,
                report.matches_live
            )
        })
        .collect();
    invariant(
        "journal_replays_to_live_state",
        "Every journalled transition replays through the kernel to its recorded hashes, and the replay ends in the live state",
        violations,
    )
}

/// Every event committed to a listened-to tenant is heard, first time in sequence order with no
/// gap; an event may be heard again (the publisher offers again what it could not mark), never
/// out of turn the first time.
fn publication_invariant(listen: &[TenantId], world: &World, heard: &[Heard]) -> InvariantResult {
    let mut violations = Vec::new();
    for tenant in listen {
        let committed = world.tenants.get(tenant).map_or(0, |t| t.event_seq);
        let mut first: Vec<u64> = Vec::new();
        let mut seen = BTreeSet::new();
        for item in heard.iter().filter(|h| h.event.tenant == *tenant) {
            if seen.insert(item.event.seq) {
                first.push(item.event.seq);
            }
        }
        let expected: Vec<u64> = (1..=committed).collect();
        if first != expected {
            let missing: Vec<u64> = expected
                .iter()
                .filter(|seq| !seen.contains(seq))
                .copied()
                .collect();
            let out_of_order = first.windows(2).any(|pair| pair[1] < pair[0]);
            violations.push(format!(
                "{tenant}: {} of {committed} committed events heard, missing {:?}{}",
                seen.len(),
                missing.iter().take(10).collect::<Vec<_>>(),
                if out_of_order {
                    ", some first heard out of order"
                } else {
                    ""
                }
            ));
        }
    }
    invariant(
        "committed_events_published_in_order",
        "Every committed event reaches the tenant's subscribers, the first time in sequence order and without a gap",
        violations,
    )
}

/// A request sent with an idempotency key is committed at most once, however often it arrived.
fn keys_invariant(listen: &[TenantId], process: &dyn Process) -> Result<InvariantResult, String> {
    let mut violations = Vec::new();
    for tenant in listen {
        let mut counts: BTreeMap<(String, String), usize> = BTreeMap::new();
        for entry in process.journal(tenant)? {
            if let Some(key) = &entry.context.idempotency_key {
                *counts
                    .entry((canon::canonical_json(&entry.context.principal), key.clone()))
                    .or_default() += 1;
            }
        }
        for ((principal, key), count) in counts {
            if count > 1 {
                violations.push(format!(
                    "{tenant}: key {key} of {principal} committed {count} times"
                ));
            }
        }
    }
    Ok(invariant(
        "keyed_requests_commit_once",
        "A request with an idempotency key is committed at most once, however many times it arrives",
        violations,
    ))
}

// ---------------------------------------------------------------------------------------------
// Recorded workloads
// ---------------------------------------------------------------------------------------------

/// A recorded workload: a tenant's journal as a deployment's store holds it, and the hash of the
/// tenant it left. The journal is the recording format: every committed transition with its
/// intent, its exact context and the hashes of what it produced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recording {
    pub tenant: TenantId,
    pub entries: Vec<Recorded>,
    pub final_hash: String,
}

/// Record a tenant of a running process.
pub fn record(process: &dyn Process, tenant: &TenantId) -> Result<Recording, String> {
    let state = process
        .state(tenant)?
        .ok_or_else(|| format!("no tenant {tenant}"))?;
    Ok(Recording {
        tenant: tenant.clone(),
        entries: process.journal(tenant)?,
        final_hash: canon::hash_of(&Some(state.as_ref())),
    })
}

/// The outcome of replaying a recording through a deployment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingReplay {
    pub entries: usize,
    /// Entries answered with the recorded revision, state delta and events.
    pub agreed: usize,
    /// The revision of the first entry that did not agree, and what was answered.
    pub first_mismatch: Option<(u64, String)>,
    pub replayed_hash: String,
    /// The tenant the replay left is the recorded one.
    pub matches: bool,
}

/// Send every recorded request, with its recorded context, through the request path of a fresh
/// process (one that installs nothing of its own) at its recorded time, and compare each answer
/// and the tenant it leaves with the recording.
pub fn replay_recording(
    deploy: &dyn Deploy,
    recording: &Recording,
    seed: u64,
) -> Result<RecordingReplay, String> {
    let start = recording
        .entries
        .first()
        .map_or(0, |entry| entry.context.now.0);
    let clock = LogicalClock::at(start);
    let store = Arc::new(FaultyStore::new(
        Arc::new(MemoryStore::with_nonces(SeededNonces::shared(seed))),
        seed,
        StoreFaults::default(),
        clock.reader(),
    ));
    let adapters = Arc::new(SeededAdapters::new(seed, 0, 0, clock.clone()));
    let process = deploy.start(&Environment {
        store: store.clone(),
        clock: clock.clone(),
        adapters,
        entropy_seed: seed,
        install: false,
        listen: Vec::new(),
    })?;
    let mut agreed = 0;
    let mut first_mismatch = None;
    for entry in &recording.entries {
        clock.advance_to(entry.context.now.0);
        let answer = process.request(&entry.intent, &entry.context);
        let same = matches!(
            &answer,
            Answer::Committed { revision, delta_hash, events_hash, replayed: false, .. }
                if *revision == entry.revision.0
                    && *delta_hash == entry.delta_hash
                    && *events_hash == entry.events_hash
        );
        if same {
            agreed += 1;
        } else if first_mismatch.is_none() {
            first_mismatch = Some((entry.revision.0, answer.verdict()));
        }
    }
    let replayed_hash = match process.state(&recording.tenant)? {
        Some(state) => canon::hash_of(&Some(state.as_ref())),
        None => canon::hash_of(&None::<&TenantState>),
    };
    Ok(RecordingReplay {
        entries: recording.entries.len(),
        agreed,
        first_mismatch,
        matches: replayed_hash == recording.final_hash,
        replayed_hash,
    })
}

/// A seeded source of bytes, for a process to install as its randomness.
pub fn seeded_bytes(seed: u64) -> impl FnMut(&mut [u8]) {
    let mut rng = Rng(seed);
    move |bytes: &mut [u8]| {
        for chunk in bytes.chunks_mut(8) {
            let word = rng.next_u64().to_le_bytes();
            chunk.copy_from_slice(&word[..chunk.len()]);
        }
    }
}
