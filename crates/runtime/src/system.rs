//! The system host: every machine of a deployment on one queue, delivered as Exegra delivers.
//!
//! [`System`] runs a set of generated machines ([`Member`]s) against one store, with the routing
//! table specc generates from the specification ([`Router`]). It follows Exegra's semantics of a
//! format-2 run (`docs/semantics.md`; normative `insertE`, `addressed`, `decideRoutes`,
//! `chooseAll`, `fireAll` and `step` in `formal/Exegra/Format2.lean`):
//!
//! - **One queue.** Every input from outside, every emit and every timer is an entry of one
//!   queue, ordered by `(due, seq)`: the time it is due, then the order it was created in. An
//!   input is due when it is submitted ([`System::submit`]) or at the time it is given
//!   ([`System::submit_at`]); an emit at the step's time plus its delay; a timer at the step's
//!   time plus its `after`. Setting a timer again removes its queued entry and queues a new one;
//!   cancelling removes it.
//! - **One step, one entry.** [`System::step`] takes the head of the queue once it is due and
//!   delivers it with `now` equal to its due time, never the clock's time, however late the host
//!   gets to it. A timer reaches only the instance that set it. Any other entry reaches, for every
//!   machine that has a transition on its event, the instance its route names (Exegra's route
//!   `by: key`: the payload field's value is the key) or, with no route, every instance of the
//!   machine. The receivers step in routing-table order, each on the configuration before the
//!   step (their records are distinct), and what they emit and schedule is queued in that order,
//!   effects in the order written. So the run is breadth first: an emit is delivered after every
//!   entry already queued for the same time.
//! - **Atomic.** One step is one transaction across every instance it reaches: the entry's
//!   removal, every record written, every entry queued and every outbox message. An ignored event
//!   writes nothing for its instance, not even the record.
//! - **A failed step commits nothing.** When any receiver fails (its step refuses, a record cannot
//!   be read or is missing for a machine that does not create records, an event or key does not
//!   decode), the transaction is rolled back, so no receiver's effects remain. Exegra ends the
//!   run there, as an error. A production host cannot stop, so this one takes the entry off the
//!   queue as a *dead letter* ([`QueueStore::dead_letters`]) with the error, reports
//!   `queue.dead_lettered` to telemetry at level error, and goes on with the next entry. A storage
//!   failure (unavailable, conflict) is not a failed step: the entry stays at the head and the
//!   step is retried.
//! - **Leaving the system.** Every emit is also written, in the step's transaction, to the outbox
//!   for its `to` (the receiver outside the machines that the specification names), due at the
//!   same time. [`System::drain_outbox`] hands due messages to the [`Dispatcher`] in due order:
//!   `at-least-once` ones retried with backoff until delivered, `at-most-once` ones dispatched
//!   once and removed after the dispatch.
//!
//! Where this differs from Exegra, and why:
//!
//! - A dead-lettered entry ends nothing (Exegra's run ends at a failed step); see above.
//! - "Every instance" of a machine is every record in its collection that holds one of its
//!   states: a record the machine cannot read is another machine's and is passed over. Exegra's
//!   instances are those its routes have created, plus the one instance of an unkeyed behaviour.
//! - A route to a key with no record steps a new instance only when the machine creates records
//!   (`storage.creates`); otherwise the step fails with [`HostError::RecordMissing`]. Exegra
//!   creates the instance in its initial state. A keyed behaviour's `max` is a model bound and
//!   is not enforced.
//! - Faults (dropped, delayed or duplicated entries, disabled transitions) are what Exegra
//!   injects; the host has none of its own.
//! - Exegra's `reply` addressing and links (`via`) are not supported; specc refuses them.

use crate::clock::{Clock, Time};
use crate::host::{decode, encode, DispatchReport, Dispatcher, Envelope, HostError, RetryPolicy};
use crate::machine::{Delivery, EffectRef, Isolation, Machine, Receiver, TimerCommand};
use crate::store::{DeadLetter, MachineStore, NewEntry, NewMessage, OutboxEntry, QueueEntry, Tx};
use crate::telemetry::{Level, NoTelemetry, Span, Telemetry};
use serde::Serialize;
use serde_json::Value;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

/// How the receivers of an event are found: one row of the routing table specc generates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Routing {
    /// The receiving machine's name.
    pub machine: &'static str,
    /// `Some(field)`: the one instance whose key is the value of this payload field (as the
    /// payload names it on the wire), Exegra's route `by: key`. `None`: every instance.
    pub key: Option<&'static str>,
}

impl Routing {
    /// Every instance of `machine`.
    pub const fn every(machine: &'static str) -> Self {
        Routing { machine, key: None }
    }

    /// The instance of `machine` keyed by the payload field `field`.
    pub const fn by_key(machine: &'static str, field: &'static str) -> Self {
        Routing {
            machine,
            key: Some(field),
        }
    }
}

/// The routing table: for each event id, the machines that react to it.
pub type RouteTable = &'static [(&'static str, &'static [Routing])];

/// What one instance did with a delivered entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivered {
    /// A transition fired.
    Fired,
    /// No transition fired; nothing was written.
    Ignored,
}

type DeliverFn = fn(&mut dyn Tx, &str, &Value, Time) -> Result<Delivered, HostError>;
type ViewFn = fn(&mut dyn Tx, &str) -> Result<Option<(Value, Value)>, HostError>;
type InitFn = fn(&mut dyn Tx, &str, &Value) -> Result<(), HostError>;
type KeyFn = fn(&Value) -> Result<String, HostError>;
type AcceptsFn = fn(&Value) -> Result<(), String>;

/// One machine as the system host drives it: events, keys and records as JSON, with the typed
/// work done by the machine's own code.
#[derive(Clone, Copy)]
pub struct Member {
    /// The machine's name.
    pub name: &'static str,
    /// The record its instances are stored in.
    pub record: &'static str,
    /// The isolation its transitions declare.
    pub isolation: Isolation,
    /// Whether an event for a key with no record creates the record.
    pub creates: bool,
    deliver: DeliverFn,
    view: ViewFn,
    init: InitFn,
    key: KeyFn,
    accepts: AcceptsFn,
}

impl std::fmt::Debug for Member {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Member")
            .field("name", &self.name)
            .field("record", &self.record)
            .finish()
    }
}

impl Member {
    /// The member for machine `M`. Its state and variables serialise, so the host can show an
    /// instance ([`System::instance`]).
    pub fn of<M: Machine>() -> Self
    where
        M::State: Serialize,
        M::Vars: Serialize,
    {
        Member {
            name: M::NAME,
            record: M::RECORD,
            isolation: M::ISOLATION,
            creates: M::CREATES,
            deliver: deliver::<M>,
            view: view::<M>,
            init: init::<M>,
            key: canonical_key::<M>,
            accepts: accepts::<M>,
        }
    }

    /// The storage key (JSON text) of the instance a key value names, in the machine's own
    /// encoding, so two spellings of one key are one instance.
    pub fn key_of(&self, value: &Value) -> Result<String, HostError> {
        (self.key)(value)
    }

    /// Whether the machine can read `event` (`{"event", "payload"}`) as one of its events.
    pub fn accepts(&self, event: &Value) -> Result<(), String> {
        (self.accepts)(event)
    }
}

fn codec(error: impl std::fmt::Display) -> HostError {
    HostError::Codec(error.to_string())
}

fn canonical_key<M: Machine>(value: &Value) -> Result<String, HostError> {
    let key: M::Key = serde_json::from_value(value.clone()).map_err(codec)?;
    encode(&key)
}

fn accepts<M: Machine>(event: &Value) -> Result<(), String> {
    serde_json::from_value::<M::Event>(event.clone())
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The state and variables of an instance as stored, or as a creating machine starts it.
fn current<M: Machine>(
    tx: &mut dyn Tx,
    key: &M::Key,
    key_text: &str,
) -> Result<Option<(M::State, M::Vars, Option<(M::Record, u64)>)>, HostError> {
    let Some(stored) = tx.load(M::RECORD, key_text)? else {
        if M::CREATES {
            let (state, vars) = M::initial(key);
            return Ok(Some((state, vars, None)));
        }
        return Ok(None);
    };
    let record: M::Record = decode(&stored.data)?;
    let (state, vars) = M::from_record(&record)?;
    Ok(Some((state, vars, Some((record, stored.version)))))
}

fn view<M: Machine>(tx: &mut dyn Tx, key_text: &str) -> Result<Option<(Value, Value)>, HostError>
where
    M::State: Serialize,
    M::Vars: Serialize,
{
    let key: M::Key = decode(key_text)?;
    let key_text = encode(&key)?;
    Ok(match current::<M>(tx, &key, &key_text)? {
        Some((state, vars, _)) => Some((
            serde_json::to_value(&state).map_err(codec)?,
            serde_json::to_value(&vars).map_err(codec)?,
        )),
        None => None,
    })
}

fn init<M: Machine>(tx: &mut dyn Tx, key_text: &str, base: &Value) -> Result<(), HostError> {
    let key: M::Key = decode(key_text)?;
    let key_text = encode(&key)?;
    let stored = tx.load(M::RECORD, &key_text)?;
    let previous: M::Record = match &stored {
        Some(s) => decode(&s.data)?,
        None => serde_json::from_value(base.clone()).map_err(codec)?,
    };
    let (state, vars) = M::initial(&key);
    let record = M::to_record(&key, &state, &vars, Some(&previous))?;
    tx.save(
        M::RECORD,
        &key_text,
        &encode(&record)?,
        stored.map(|s| s.version),
    )?;
    Ok(())
}

fn deliver<M: Machine>(
    tx: &mut dyn Tx,
    key_text: &str,
    event: &Value,
    now: Time,
) -> Result<Delivered, HostError> {
    let key: M::Key = decode(key_text)?;
    let key_text = encode(&key)?;
    let Some((state, vars, previous)) = current::<M>(tx, &key, &key_text)? else {
        return Err(HostError::RecordMissing {
            machine: M::NAME,
            key: key_text,
        });
    };
    let event: M::Event = serde_json::from_value(event.clone()).map_err(codec)?;
    let outcome = M::step(&state, &vars, &event, now)?;
    if outcome.ignored {
        return Ok(Delivered::Ignored);
    }
    let (previous, version) = previous.unzip();
    let record = M::to_record(&key, &outcome.state, &outcome.vars, previous.as_ref())?;
    tx.save(M::RECORD, &key_text, &encode(&record)?, version)?;
    for effect in outcome.effects() {
        match effect {
            EffectRef::Emit(i) => {
                let emit = &outcome.emits[i];
                let due = now.after(emit.after.unwrap_or(Duration::ZERO));
                let text = encode(&emit.event)?;
                tx.push(&NewEntry {
                    due,
                    event: text.clone(),
                    target: None,
                    timer: None,
                    source: M::NAME.to_string(),
                    source_key: key_text.clone(),
                })?;
                tx.enqueue(&NewMessage {
                    source: M::NAME.to_string(),
                    key: key_text.clone(),
                    receiver: emit.receiver.0.clone(),
                    event: text,
                    created_at: now,
                    delivery: emit.delivery,
                    not_before: due,
                    next_attempt_at: due,
                })?;
            }
            EffectRef::Timer(i) => match &outcome.timers[i] {
                TimerCommand::Set { name, after, event } => {
                    tx.drop_timer_entry(M::NAME, &key_text, name)?;
                    tx.push(&NewEntry {
                        due: now.after(*after),
                        event: encode(event)?,
                        target: Some((M::NAME.to_string(), key_text.clone())),
                        timer: Some(name.clone()),
                        source: M::NAME.to_string(),
                        source_key: key_text.clone(),
                    })?;
                }
                TimerCommand::Cancel { name } => {
                    tx.drop_timer_entry(M::NAME, &key_text, name)?;
                }
            },
        }
    }
    Ok(Delivered::Fired)
}

/// The machines of a deployment and the routing table between them, as specc generates them.
#[derive(Clone, Debug)]
pub struct Router {
    members: Vec<Member>,
    routes: RouteTable,
    isolation: Isolation,
}

impl Router {
    /// A router over `members`, routing by `routes`.
    pub fn new(members: Vec<Member>, routes: RouteTable) -> Self {
        let isolation = members
            .iter()
            .map(|m| m.isolation)
            .max()
            .unwrap_or(Isolation::Serializable);
        Router {
            members,
            routes,
            isolation,
        }
    }

    /// The members, in table order.
    pub fn members(&self) -> &[Member] {
        &self.members
    }

    /// The member named `name`.
    pub fn member(&self, name: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.name == name)
    }

    /// The machines that react to `event`, with how each finds its instance.
    pub fn routes(&self, event: &str) -> &'static [Routing] {
        self.routes
            .iter()
            .find(|(id, _)| *id == event)
            .map_or(&[], |(_, routes)| routes)
    }

    /// The strongest isolation any member declares: the isolation of every step, which may
    /// reach several of them.
    pub fn isolation(&self) -> Isolation {
        self.isolation
    }
}

/// One instance an entry reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reached {
    /// The machine.
    pub machine: &'static str,
    /// The instance's key (JSON text).
    pub key: String,
    /// What it did.
    pub delivered: Delivered,
}

/// What one [`System::step`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepReport {
    /// The entry delivered.
    pub entry: QueueEntry,
    /// The instances it reached, in the order they stepped; empty when it failed.
    pub reached: Vec<Reached>,
    /// When the step failed: why. The entry is a dead letter and nothing else was committed.
    pub failed: Option<String>,
}

/// What one [`System::tick`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SystemTick {
    /// Entries delivered and committed.
    pub steps: usize,
    /// Entries whose step failed, now dead letters.
    pub dead: usize,
    /// Outbox dispatches.
    pub dispatched: DispatchReport,
}

/// How many entries or outbox messages one tick handles at most.
const BATCH: usize = 256;

/// The source of an entry submitted from outside.
pub const INPUT: &str = "input";

/// The receivers of an entry: the member, the key, and whether a timer addressed it.
type Receivers<'r> = Vec<(&'r Member, String, bool)>;

fn receivers<'r>(
    router: &'r Router,
    tx: &mut dyn Tx,
    entry: &QueueEntry,
    event: &Value,
) -> Result<Receivers<'r>, HostError> {
    if let Some((machine, key)) = &entry.target {
        return Ok(router
            .member(machine)
            .map(|m| vec![(m, key.clone(), true)])
            .unwrap_or_default());
    }
    let id = event
        .get("event")
        .and_then(Value::as_str)
        .ok_or_else(|| codec("a queue entry without an event id"))?;
    let mut out = Vec::new();
    for routing in router.routes(id) {
        let Some(member) = router.member(routing.machine) else {
            continue;
        };
        match routing.key {
            Some(field) => {
                let value = event
                    .get("payload")
                    .and_then(|p| p.get(field))
                    .filter(|v| !v.is_null())
                    .ok_or_else(|| {
                        HostError::Step(crate::machine::StepError::Rejected(format!(
                            "the route of {id} to {} needs payload field {field}",
                            member.name
                        )))
                    })?;
                out.push((member, member.key_of(value)?, false));
            }
            None => {
                for key in tx.keys(member.record)? {
                    match (member.view)(tx, &key) {
                        Ok(Some(_)) => out.push((member, key, false)),
                        Ok(None) | Err(HostError::Record(_)) => {}
                        Err(error) => return Err(error),
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Deliver `entry` inside `tx`: take it off the queue and step every receiver.
fn step_entry(
    router: &Router,
    tx: &mut dyn Tx,
    entry: &QueueEntry,
) -> Result<Option<Vec<Reached>>, HostError> {
    if !tx.take_entry(entry.seq)? {
        // Taken since it was read.
        return Ok(None);
    }
    let event: Value = serde_json::from_str(&entry.event).map_err(codec)?;
    let mut reached = Vec::new();
    for (member, key, targeted) in receivers(router, tx, entry, &event)? {
        match (member.deliver)(tx, &key, &event, entry.due) {
            Ok(delivered) => reached.push(Reached {
                machine: member.name,
                key,
                delivered,
            }),
            // A timer of an instance that is gone reaches nothing.
            Err(HostError::RecordMissing { .. }) if targeted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(Some(reached))
}

/// Errors that fail a step for good, as opposed to storage trouble that a retry may clear.
fn fails_step(error: &HostError) -> bool {
    matches!(
        error,
        HostError::Step(_)
            | HostError::Record(_)
            | HostError::RecordMissing { .. }
            | HostError::Codec(_)
    )
}

/// Runs every machine of a deployment on one store, one queue and one dispatcher.
pub struct System<S: MachineStore, D: Dispatcher<Value>> {
    store: Arc<S>,
    router: Arc<Router>,
    dispatcher: Arc<D>,
    clock: Arc<dyn Clock>,
    telemetry: Arc<dyn Telemetry>,
    retry: RetryPolicy,
}

impl<S: MachineStore, D: Dispatcher<Value>> Clone for System<S, D> {
    fn clone(&self) -> Self {
        System {
            store: self.store.clone(),
            router: self.router.clone(),
            dispatcher: self.dispatcher.clone(),
            clock: self.clock.clone(),
            telemetry: self.telemetry.clone(),
            retry: self.retry,
        }
    }
}

impl<S: MachineStore, D: Dispatcher<Value>> System<S, D> {
    /// A system host. Fails when the store cannot run a member's declared isolation.
    pub fn new(
        store: Arc<S>,
        router: Router,
        dispatcher: Arc<D>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, HostError> {
        for member in router.members() {
            if !store.supports(member.isolation) {
                return Err(HostError::Unsupported {
                    machine: member.name,
                    isolation: member.isolation,
                });
            }
        }
        Ok(System {
            store,
            router: Arc::new(router),
            dispatcher,
            clock,
            telemetry: Arc::new(NoTelemetry),
            retry: RetryPolicy::default(),
        })
    }

    /// Report to `telemetry` instead of nowhere.
    pub fn with_telemetry(mut self, telemetry: Arc<dyn Telemetry>) -> Self {
        self.telemetry = telemetry;
        self
    }

    /// Retry failed `at-least-once` dispatches by `retry` instead of the default.
    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// The store.
    pub fn store(&self) -> &Arc<S> {
        &self.store
    }

    /// The router.
    pub fn router(&self) -> &Router {
        &self.router
    }

    async fn on_store<T, F>(&self, work: F) -> Result<T, HostError>
    where
        T: Send + 'static,
        F: FnOnce(&S, &Router) -> T + Send + 'static,
    {
        let (store, router) = (self.store.clone(), self.router.clone());
        tokio::task::spawn_blocking(move || work(&store, &router))
            .await
            .map_err(|e| HostError::Worker(e.to_string()))
    }

    /// Queue `event` (`{"event": <id>, "payload": {...}}`), an input from outside, due now.
    /// Returns its sequence number.
    pub async fn submit(&self, event: Value) -> Result<u64, HostError> {
        self.submit_at(event, self.clock.now()).await
    }

    /// Queue `event`, an input from outside, due at `at`. Returns its sequence number.
    pub async fn submit_at(&self, event: Value, at: Time) -> Result<u64, HostError> {
        if event.get("event").and_then(Value::as_str).is_none() {
            return Err(codec("an input needs a string \"event\""));
        }
        let text = event.to_string();
        self.on_store(move |store, router| {
            store.transact(router.isolation(), |tx| -> Result<u64, HostError> {
                Ok(tx.push(&NewEntry {
                    due: at,
                    event: text,
                    target: None,
                    timer: None,
                    source: INPUT.to_string(),
                    source_key: String::new(),
                })?)
            })
        })
        .await?
    }

    /// When the head of the queue is due, if anything is queued.
    pub async fn next_due(&self) -> Result<Option<Time>, HostError> {
        let head = self.on_store(|store, _| store.head()).await??;
        Ok(head.map(|e| e.due))
    }

    /// Up to `limit` queued entries, in delivery order.
    pub async fn queued(&self, limit: usize) -> Result<Vec<QueueEntry>, HostError> {
        Ok(self.on_store(move |store, _| store.queued(limit)).await??)
    }

    /// The dead letters, oldest first.
    pub async fn dead_letters(&self) -> Result<Vec<DeadLetter>, HostError> {
        Ok(self.on_store(|store, _| store.dead_letters()).await??)
    }

    /// The state and variables of the instance of `machine` with key `key` (JSON text), as
    /// JSON: from its record, or the initial ones of a creating machine with no record yet.
    /// `None` when a non-creating machine has no record for the key.
    pub async fn instance(
        &self,
        machine: &str,
        key: &str,
    ) -> Result<Option<(Value, Value)>, HostError> {
        let (machine, key) = (machine.to_string(), key.to_string());
        self.on_store(move |store, router| {
            let Some(member) = router.member(&machine).copied() else {
                return Err(codec(format!("no machine {machine}")));
            };
            store.transact(member.isolation, |tx| (member.view)(tx, &key))
        })
        .await?
    }

    /// The keys (JSON text) of every record `machine`'s instances are stored in, in creation
    /// order, whether or not they hold one of its instances.
    pub async fn keys(&self, machine: &str) -> Result<Vec<String>, HostError> {
        let machine = machine.to_string();
        self.on_store(move |store, router| {
            let Some(member) = router.member(&machine).copied() else {
                return Err(codec(format!("no machine {machine}")));
            };
            store.transact(member.isolation, |tx| -> Result<_, HostError> {
                Ok(tx.keys(member.record)?)
            })
        })
        .await?
    }

    /// Put the instance of `machine` with key `key` (JSON text) in its initial state, over its
    /// record or, when it has none, over `base`: the record as whoever creates it (such as a
    /// contract) would have written it. For setting up an instance a non-creating machine steps.
    pub async fn init(&self, machine: &str, key: &str, base: Value) -> Result<(), HostError> {
        let (machine, key) = (machine.to_string(), key.to_string());
        self.on_store(move |store, router| {
            let Some(member) = router.member(&machine).copied() else {
                return Err(codec(format!("no machine {machine}")));
            };
            store.transact(member.isolation, |tx| (member.init)(tx, &key, &base))
        })
        .await?
    }

    /// Deliver the head of the queue if it is due: one step. `None` when nothing is due.
    pub async fn step(&self) -> Result<Option<StepReport>, HostError> {
        let now = self.clock.now();
        let Some(entry) = self.on_store(|store, _| store.head()).await?? else {
            return Ok(None);
        };
        if entry.due > now {
            return Ok(None);
        }
        let span = Span::start(&self.telemetry, "system.step");
        let taken = entry.clone();
        let result = self
            .on_store(move |store, router| {
                store.transact(router.isolation(), |tx| step_entry(router, tx, &taken))
            })
            .await?;
        match result {
            Ok(Some(reached)) => {
                span.finish(true, &[("reached", (reached.len() as u64).into())]);
                Ok(Some(StepReport {
                    entry,
                    reached,
                    failed: None,
                }))
            }
            Ok(None) => {
                span.finish(true, &[("reached", 0u64.into())]);
                Ok(Some(StepReport {
                    entry,
                    reached: Vec::new(),
                    failed: None,
                }))
            }
            Err(error) if fails_step(&error) => {
                span.finish(false, &[]);
                let why = error.to_string();
                self.telemetry.event(
                    Level::Error,
                    "queue.dead_lettered",
                    &[
                        ("seq", entry.seq.into()),
                        ("source", entry.source.as_str().into()),
                        ("error", why.as_str().into()),
                    ],
                );
                let (dead, reason) = (entry.clone(), why.clone());
                self.on_store(move |store, router| {
                    store.transact(router.isolation(), |tx| -> Result<(), HostError> {
                        if tx.take_entry(dead.seq)? {
                            tx.dead_letter(&dead, &reason, now)?;
                        }
                        Ok(())
                    })
                })
                .await??;
                Ok(Some(StepReport {
                    entry,
                    reached: Vec::new(),
                    failed: Some(why),
                }))
            }
            Err(error) => {
                span.finish(false, &[]);
                Err(error)
            }
        }
    }

    /// Hand every due outbox message of every member to the dispatcher, in due order.
    pub async fn drain_outbox(&self) -> Result<DispatchReport, HostError> {
        let now = self.clock.now();
        let mut due: Vec<OutboxEntry> = self
            .on_store(
                move |store, router| -> Result<Vec<OutboxEntry>, HostError> {
                    let mut all = Vec::new();
                    for member in router.members() {
                        all.extend(store.due_messages(member.name, now, BATCH)?);
                    }
                    Ok(all)
                },
            )
            .await??;
        due.sort_by_key(|m| (m.not_before, m.id));
        due.truncate(BATCH);
        let mut report = DispatchReport::default();
        for entry in due {
            let event: Value = match serde_json::from_str(&entry.event) {
                Ok(event) => event,
                Err(error) => {
                    let (id, attempts, next) =
                        (entry.id, entry.attempts + 1, now.after(self.retry.max));
                    let why = error.to_string();
                    self.telemetry.event(
                        Level::Error,
                        "outbox.unreadable",
                        &[("id", id.into()), ("error", why.as_str().into())],
                    );
                    let _ = self
                        .on_store(move |store, _| store.defer(id, attempts, next, &why))
                        .await;
                    report.deferred += 1;
                    continue;
                }
            };
            let at_least_once = entry.delivery == Delivery::AtLeastOnce;
            let envelope = Envelope {
                id: at_least_once.then_some(entry.id),
                source: entry.source,
                key: entry.key,
                receiver: Receiver(entry.receiver),
                delivery: entry.delivery,
                event,
            };
            let (id, attempts) = (entry.id, entry.attempts);
            let result = self.dispatcher.dispatch(&envelope).await;
            match result {
                Ok(()) => {
                    report.delivered += 1;
                    self.forget(id).await;
                }
                Err(error) if at_least_once => {
                    let attempts = attempts + 1;
                    let next = self.clock.now().after(self.retry.delay(attempts));
                    let why = error.0.clone();
                    self.telemetry.event(
                        Level::Warn,
                        "outbox.deferred",
                        &[
                            ("source", envelope.source.as_str().into()),
                            ("receiver", envelope.receiver.as_str().into()),
                            ("id", id.into()),
                            ("attempts", attempts.into()),
                            ("error", why.as_str().into()),
                        ],
                    );
                    let _ = self
                        .on_store(move |store, _| store.defer(id, attempts, next, &why))
                        .await;
                    report.deferred += 1;
                }
                Err(error) => {
                    self.telemetry.event(
                        Level::Warn,
                        "emit.dropped",
                        &[
                            ("source", envelope.source.as_str().into()),
                            ("receiver", envelope.receiver.as_str().into()),
                            ("error", error.0.as_str().into()),
                        ],
                    );
                    report.dropped += 1;
                    // At most once: removed after its one dispatch.
                    self.forget(id).await;
                }
            }
        }
        Ok(report)
    }

    async fn forget(&self, id: u64) {
        let failed = match self
            .on_store(move |store, _| store.mark_delivered(id))
            .await
        {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(error.to_string()),
            Err(error) => Some(error.to_string()),
        };
        if let Some(why) = failed {
            self.telemetry.event(
                Level::Warn,
                "outbox.unremoved",
                &[("id", id.into()), ("error", why.as_str().into())],
            );
        }
    }

    /// Deliver due entries, up to a batch, then drain the outbox.
    pub async fn tick(&self) -> Result<SystemTick, HostError> {
        let mut report = SystemTick::default();
        for _ in 0..BATCH {
            match self.step().await? {
                None => break,
                Some(step) if step.failed.is_some() => report.dead += 1,
                Some(_) => report.steps += 1,
            }
        }
        report.dispatched = self.drain_outbox().await?;
        Ok(report)
    }

    /// Call [`System::tick`] every `period` until `shutdown` resolves. Failures are reported to
    /// telemetry and the loop goes on.
    pub async fn run(&self, period: Duration, shutdown: impl Future<Output = ()>) {
        let mut shutdown = std::pin::pin!(shutdown);
        loop {
            if let Err(error) = self.tick().await {
                self.telemetry.event(
                    Level::Error,
                    "system.tick_failed",
                    &[("error", error.to_string().as_str().into())],
                );
            }
            tokio::select! {
                _ = &mut shutdown => return,
                _ = tokio::time::sleep(period) => {}
            }
        }
    }
}
