//! The machine host: runs a [`Machine`]'s steps against a store, with an outbox and timers.
//!
//! For one incoming event the host
//!
//! 1. opens one transaction at the machine's declared isolation (or stronger),
//! 2. loads the instance's record by key, or, when there is none, starts from [`Machine::initial`]
//!    if the machine creates records ([`Machine::CREATES`]) and otherwise refuses the event with
//!    [`HostError::RecordMissing`],
//! 3. runs [`Machine::step`],
//! 4. saves the new record, applies the timer commands, and writes every `at-least-once` emit to
//!    the outbox, all in that same transaction,
//! 5. commits, and only then
//! 6. dispatches the emits in order: an `at-least-once` emit is removed from the outbox when its
//!    dispatch succeeds and deferred with backoff when it fails; an `at-most-once` emit is
//!    dispatched once and forgotten either way.
//!
//! If the step fails, the transaction is rolled back and nothing is saved, scheduled or emitted.
//! If the process stops between commit and dispatch, the `at-least-once` emits are still in the
//! outbox and [`Host::drain_outbox`] delivers them later; the `at-most-once` emits are lost.
//!
//! Timers fire through [`Host::fire_due_timers`]: each due timer is removed and its event is
//! stepped in one transaction, so a timer fires at most once per setting and its effects commit
//! together with its removal. [`Host::tick`] does both jobs and [`Host::run`] calls it in a loop.

use crate::clock::{Clock, Time};
use crate::machine::{
    Delivery, Isolation, Machine, Receiver, RecordError, StepError, TimerCommand,
};
use crate::store::{MachineStore, NewMessage, NewTimer, OutboxEntry, StorageError, TimerEntry, Tx};
use crate::telemetry::{Level, NoTelemetry, Span, Telemetry};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

/// An emitted event on its way to a receiver.
#[derive(Clone, Debug, PartialEq)]
pub struct Envelope<E> {
    /// The outbox id of an `at-least-once` event: the same on every redelivery, so a receiver
    /// can drop duplicates by it. `None` for `at-most-once` events.
    pub id: Option<u64>,
    /// The emitting machine's name.
    pub source: String,
    /// The emitting instance's key (JSON text).
    pub key: String,
    /// Where it goes.
    pub receiver: Receiver,
    /// Its delivery guarantee.
    pub delivery: Delivery,
    /// The event.
    pub event: E,
}

/// A dispatch that did not reach its receiver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchError(pub String);

impl fmt::Display for DispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DispatchError {}

/// Hands emitted events to their receivers. Written by hand per deployment: it maps a
/// [`Receiver`] id to a channel, a client call or another host.
///
/// Returning `Ok` means the receiver has the event. For an `at-least-once` envelope, an `Err` (or
/// a process stop before `Ok` is recorded) means it will be offered again, with the same id.
pub trait Dispatcher<E>: Send + Sync + 'static {
    /// Deliver one envelope.
    fn dispatch(
        &self,
        envelope: &Envelope<E>,
    ) -> impl Future<Output = Result<(), DispatchError>> + Send;
}

/// When to retry an `at-least-once` dispatch that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// The delay after the first failure. Also how long a freshly committed entry is held back
    /// from the outbox drain while the host dispatches it itself.
    pub first: Duration,
    /// Each further failure multiplies the delay by this.
    pub factor: u32,
    /// The delay never grows beyond this.
    pub max: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            first: Duration::from_secs(1),
            factor: 2,
            max: Duration::from_secs(300),
        }
    }
}

impl RetryPolicy {
    /// The delay after `attempts` failed attempts (1 for the first failure).
    pub fn delay(&self, attempts: u32) -> Duration {
        let mut delay = self.first;
        for _ in 1..attempts {
            delay = delay.saturating_mul(self.factor.max(1));
            if delay >= self.max {
                return self.max;
            }
        }
        delay.min(self.max)
    }
}

/// Why the host could not handle an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostError {
    /// The store failed; nothing was committed.
    Storage(StorageError),
    /// The step refused the event; nothing was committed.
    Step(StepError),
    /// The stored record does not hold an instance.
    Record(RecordError),
    /// The machine does not create records (`storage.creates` false) and the key has no record:
    /// the event is refused and nothing was stored, scheduled or emitted.
    RecordMissing {
        /// The machine.
        machine: &'static str,
        /// The key (JSON text).
        key: String,
    },
    /// A key, record or event could not be encoded or decoded.
    Codec(String),
    /// The store cannot run the machine's declared isolation.
    Unsupported {
        /// The machine.
        machine: &'static str,
        /// What it declares.
        isolation: Isolation,
    },
    /// The blocking worker running the store call failed.
    Worker(String),
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostError::Storage(e) => e.fmt(f),
            HostError::Step(e) => e.fmt(f),
            HostError::Record(e) => e.fmt(f),
            HostError::RecordMissing { machine, key } => {
                write!(
                    f,
                    "{machine} has no record for key {key} and does not create records"
                )
            }
            HostError::Codec(why) => write!(f, "encoding: {why}"),
            HostError::Unsupported { machine, isolation } => {
                write!(
                    f,
                    "the store cannot run {machine} at {isolation:?} isolation"
                )
            }
            HostError::Worker(why) => write!(f, "worker failed: {why}"),
        }
    }
}

impl std::error::Error for HostError {}

impl From<StorageError> for HostError {
    fn from(error: StorageError) -> Self {
        HostError::Storage(error)
    }
}

impl From<StepError> for HostError {
    fn from(error: StepError) -> Self {
        HostError::Step(error)
    }
}

impl From<RecordError> for HostError {
    fn from(error: RecordError) -> Self {
        HostError::Record(error)
    }
}

fn encode<T: Serialize>(value: &T) -> Result<String, HostError> {
    serde_json::to_string(value).map_err(|e| HostError::Codec(e.to_string()))
}

fn decode<T: DeserializeOwned>(text: &str) -> Result<T, HostError> {
    serde_json::from_str(text).map_err(|e| HostError::Codec(e.to_string()))
}

/// A committed transition whose emits have not been dispatched yet.
///
/// Dropping it without [`Host::dispatch`] is what a process stop between commit and dispatch
/// looks like: the `at-least-once` emits stay in the outbox, the `at-most-once` ones are gone.
pub struct Committed<M: Machine> {
    /// The instance's key.
    pub key: M::Key,
    /// Its state after the step.
    pub state: M::State,
    /// Its variables after the step.
    pub vars: M::Vars,
    /// The record version written.
    pub version: u64,
    /// The emits, in order, waiting for dispatch.
    pub emits: Vec<Envelope<M::Emitted>>,
}

/// What happened to a batch of dispatches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DispatchReport {
    /// Reached their receivers.
    pub delivered: usize,
    /// `at-least-once` dispatches that failed and stay in the outbox for a retry.
    pub deferred: usize,
    /// `at-most-once` dispatches that failed and are gone.
    pub dropped: usize,
}

impl DispatchReport {
    fn add(&mut self, other: DispatchReport) {
        self.delivered += other.delivered;
        self.deferred += other.deferred;
        self.dropped += other.dropped;
    }
}

/// The result of [`Host::handle`].
pub struct Handled<M: Machine> {
    /// The instance's state after the step.
    pub state: M::State,
    /// Its variables after the step.
    pub vars: M::Vars,
    /// The record version written.
    pub version: u64,
    /// What happened to the emits.
    pub dispatched: DispatchReport,
}

/// What one [`Host::tick`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TickReport {
    /// Timers that fired and committed a step.
    pub fired: usize,
    /// Timers that were due but discarded because their step failed.
    pub discarded: usize,
    /// Dispatches, from fired timers and from the outbox drain together.
    pub dispatched: DispatchReport,
}

/// How many outbox entries or timers one drain looks at.
const BATCH: usize = 256;

/// Runs one machine against one store and one dispatcher.
pub struct Host<M: Machine, S: MachineStore, D: Dispatcher<M::Emitted>> {
    store: Arc<S>,
    dispatcher: Arc<D>,
    clock: Arc<dyn Clock>,
    telemetry: Arc<dyn Telemetry>,
    retry: RetryPolicy,
    machine: PhantomData<fn() -> M>,
}

impl<M: Machine, S: MachineStore, D: Dispatcher<M::Emitted>> Clone for Host<M, S, D> {
    fn clone(&self) -> Self {
        Host {
            store: self.store.clone(),
            dispatcher: self.dispatcher.clone(),
            clock: self.clock.clone(),
            telemetry: self.telemetry.clone(),
            retry: self.retry,
            machine: PhantomData,
        }
    }
}

/// Step one instance inside an open transaction. `timer` is the id of the timer being fired,
/// which must still be set; `Ok(None)` means it was not and nothing happened.
fn transition<M: Machine>(
    tx: &mut dyn Tx,
    key: &M::Key,
    event: &M::Event,
    now: Time,
    hold: Duration,
    timer: Option<u64>,
) -> Result<Option<Committed<M>>, HostError> {
    if let Some(id) = timer {
        if !tx.take_timer(id)? {
            return Ok(None);
        }
    }
    let key_text = encode(key)?;
    let stored = tx.load(M::RECORD, &key_text)?;
    let previous: Option<M::Record> = stored.as_ref().map(|s| decode(&s.data)).transpose()?;
    let (state, vars) = match &previous {
        Some(record) => M::from_record(record)?,
        None if M::CREATES => M::initial(key),
        None => {
            return Err(HostError::RecordMissing {
                machine: M::NAME,
                key: key_text,
            })
        }
    };
    let outcome = M::step(&state, &vars, event, now)?;
    let record = M::to_record(key, &outcome.state, &outcome.vars, previous.as_ref())?;
    let version = tx.save(
        M::RECORD,
        &key_text,
        &encode(&record)?,
        stored.map(|s| s.version),
    )?;
    for command in &outcome.timers {
        match command {
            TimerCommand::Set { name, after, event } => {
                tx.set_timer(&NewTimer {
                    machine: M::NAME.to_string(),
                    key: key_text.clone(),
                    name: name.clone(),
                    fire_at: now.after(*after),
                    event: encode(event)?,
                })?;
            }
            TimerCommand::Cancel { name } => {
                tx.cancel_timer(M::NAME, &key_text, name)?;
            }
        }
    }
    let mut emits = Vec::with_capacity(outcome.emits.len());
    for emit in outcome.emits {
        let id = match emit.delivery {
            Delivery::AtLeastOnce => Some(tx.enqueue(&NewMessage {
                source: M::NAME.to_string(),
                key: key_text.clone(),
                receiver: emit.receiver.0.clone(),
                event: encode(&emit.event)?,
                created_at: now,
                next_attempt_at: now.after(hold),
            })?),
            Delivery::AtMostOnce => None,
        };
        emits.push(Envelope {
            id,
            source: M::NAME.to_string(),
            key: key_text.clone(),
            receiver: emit.receiver,
            delivery: emit.delivery,
            event: emit.event,
        });
    }
    Ok(Some(Committed {
        key: key.clone(),
        state: outcome.state,
        vars: outcome.vars,
        version,
        emits,
    }))
}

impl<M: Machine, S: MachineStore, D: Dispatcher<M::Emitted>> Host<M, S, D> {
    /// A host for `M`. Fails when the store cannot run the machine's declared isolation.
    pub fn new(
        store: Arc<S>,
        dispatcher: Arc<D>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, HostError> {
        if !store.supports(M::ISOLATION) {
            return Err(HostError::Unsupported {
                machine: M::NAME,
                isolation: M::ISOLATION,
            });
        }
        Ok(Host {
            store,
            dispatcher,
            clock,
            telemetry: Arc::new(NoTelemetry),
            retry: RetryPolicy::default(),
            machine: PhantomData,
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

    /// The store this host writes to.
    pub fn store(&self) -> &Arc<S> {
        &self.store
    }

    async fn on_store<T, F>(&self, work: F) -> Result<T, HostError>
    where
        T: Send + 'static,
        F: FnOnce(&S) -> T + Send + 'static,
    {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || work(&store))
            .await
            .map_err(|e| HostError::Worker(e.to_string()))
    }

    /// The instance of `key` as stored: state, variables and record version, or `None` when it
    /// has no record.
    pub async fn load(&self, key: &M::Key) -> Result<Option<(M::State, M::Vars, u64)>, HostError> {
        let key_text = encode(key)?;
        self.on_store(move |store| {
            store.transact(M::ISOLATION, |tx| -> Result<_, HostError> {
                let Some(stored) = tx.load(M::RECORD, &key_text)? else {
                    return Ok(None);
                };
                let record: M::Record = decode(&stored.data)?;
                let (state, vars) = M::from_record(&record)?;
                Ok(Some((state, vars, stored.version)))
            })
        })
        .await?
    }

    /// Step the instance of `key` with `event` and commit, without dispatching the emits.
    pub async fn commit(&self, key: M::Key, event: M::Event) -> Result<Committed<M>, HostError> {
        let span = Span::start(&self.telemetry, "machine.transition");
        let (now, hold) = (self.clock.now(), self.retry.first);
        let result = self
            .on_store(move |store| {
                store.transact(M::ISOLATION, |tx| {
                    transition::<M>(tx, &key, &event, now, hold, None)
                })
            })
            .await
            .and_then(|inner| inner)
            .map(|committed| committed.expect("a transition without a timer always commits"));
        span.finish(result.is_ok(), &[("machine", M::NAME.into())]);
        result
    }

    /// Dispatch a committed transition's emits, in order.
    pub async fn dispatch(&self, committed: Committed<M>) -> DispatchReport {
        let mut report = DispatchReport::default();
        for envelope in committed.emits {
            report.add(self.deliver(envelope, 0).await);
        }
        report
    }

    /// Step the instance of `key` with `event`, commit, then dispatch the emits.
    pub async fn handle(&self, key: M::Key, event: M::Event) -> Result<Handled<M>, HostError> {
        let committed = self.commit(key, event).await?;
        let (state, vars, version) = (
            committed.state.clone(),
            committed.vars.clone(),
            committed.version,
        );
        let dispatched = self.dispatch(committed).await;
        Ok(Handled {
            state,
            vars,
            version,
            dispatched,
        })
    }

    /// Dispatch one envelope that has failed `attempts` times so far, and record the result.
    async fn deliver(&self, envelope: Envelope<M::Emitted>, attempts: u32) -> DispatchReport {
        let result = self.dispatcher.dispatch(&envelope).await;
        let receiver = envelope.receiver.as_str();
        match (envelope.id, result) {
            (Some(id), Ok(())) => {
                let marked = match self.on_store(move |store| store.mark_delivered(id)).await {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(error.to_string()),
                    Err(error) => Some(error.to_string()),
                };
                if let Some(why) = marked {
                    // The receiver has it; it will be offered again, which it must tolerate.
                    self.telemetry.event(
                        Level::Warn,
                        "outbox.unmarked",
                        &[("id", id.into()), ("error", why.as_str().into())],
                    );
                }
                DispatchReport {
                    delivered: 1,
                    ..DispatchReport::default()
                }
            }
            (Some(id), Err(error)) => {
                let attempts = attempts + 1;
                let next = self.clock.now().after(self.retry.delay(attempts));
                let why = error.0.clone();
                self.telemetry.event(
                    Level::Warn,
                    "outbox.deferred",
                    &[
                        ("machine", M::NAME.into()),
                        ("receiver", receiver.into()),
                        ("id", id.into()),
                        ("attempts", attempts.into()),
                        ("error", why.as_str().into()),
                    ],
                );
                let _ = self
                    .on_store(move |store| store.defer(id, attempts, next, &why))
                    .await;
                DispatchReport {
                    deferred: 1,
                    ..DispatchReport::default()
                }
            }
            (None, Ok(())) => DispatchReport {
                delivered: 1,
                ..DispatchReport::default()
            },
            (None, Err(error)) => {
                self.telemetry.event(
                    Level::Warn,
                    "emit.dropped",
                    &[
                        ("machine", M::NAME.into()),
                        ("receiver", receiver.into()),
                        ("error", error.0.as_str().into()),
                    ],
                );
                DispatchReport {
                    dropped: 1,
                    ..DispatchReport::default()
                }
            }
        }
    }

    /// Dispatch every outbox entry of this machine that is due, oldest first.
    pub async fn drain_outbox(&self) -> Result<DispatchReport, HostError> {
        let now = self.clock.now();
        let due: Vec<OutboxEntry> = self
            .on_store(move |store| store.due_messages(M::NAME, now, BATCH))
            .await??;
        let mut report = DispatchReport::default();
        for entry in due {
            let event: M::Emitted = match decode(&entry.event) {
                Ok(event) => event,
                Err(error) => {
                    // Unreadable: keep it, but at the longest delay, and say so.
                    let (id, attempts, next) =
                        (entry.id, entry.attempts + 1, now.after(self.retry.max));
                    let why = error.to_string();
                    self.telemetry.event(
                        Level::Error,
                        "outbox.unreadable",
                        &[("id", id.into()), ("error", why.as_str().into())],
                    );
                    let _ = self
                        .on_store(move |store| store.defer(id, attempts, next, &why))
                        .await;
                    report.deferred += 1;
                    continue;
                }
            };
            let envelope = Envelope {
                id: Some(entry.id),
                source: entry.source,
                key: entry.key,
                receiver: Receiver(entry.receiver),
                delivery: Delivery::AtLeastOnce,
                event,
            };
            report.add(self.deliver(envelope, entry.attempts).await);
        }
        Ok(report)
    }

    /// Fire every timer of this machine that is due, earliest first, and dispatch what the
    /// fired steps emit. A timer whose step fails is removed without effect and counted as
    /// discarded.
    pub async fn fire_due_timers(&self) -> Result<TickReport, HostError> {
        let now = self.clock.now();
        let due: Vec<TimerEntry> = self
            .on_store(move |store| store.due_timers(M::NAME, now, BATCH))
            .await??;
        let mut report = TickReport::default();
        let hold = self.retry.first;
        for timer in due {
            let key: M::Key = decode(&timer.key)?;
            let event: M::Event = decode(&timer.event)?;
            let id = timer.id;
            let fired = self
                .on_store(move |store| {
                    store.transact(M::ISOLATION, |tx| {
                        transition::<M>(tx, &key, &event, now, hold, Some(id))
                    })
                })
                .await?;
            match fired {
                Ok(Some(committed)) => {
                    report.fired += 1;
                    report.dispatched.add(self.dispatch(committed).await);
                }
                Ok(None) => {}
                Err(
                    error @ (HostError::Step(_)
                    | HostError::Record(_)
                    | HostError::RecordMissing { .. }
                    | HostError::Codec(_)),
                ) => {
                    // The step can never succeed for this timer; firing it again would fail the
                    // same way, so it is removed on its own.
                    let why = error.to_string();
                    self.telemetry.event(
                        Level::Warn,
                        "timer.discarded",
                        &[
                            ("machine", M::NAME.into()),
                            ("timer", timer.name.as_str().into()),
                            ("error", why.as_str().into()),
                        ],
                    );
                    self.on_store(move |store| {
                        store.transact(M::ISOLATION, |tx| -> Result<bool, HostError> {
                            Ok(tx.take_timer(id)?)
                        })
                    })
                    .await??;
                    report.discarded += 1;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(report)
    }

    /// Fire due timers, then drain the outbox.
    pub async fn tick(&self) -> Result<TickReport, HostError> {
        let mut report = self.fire_due_timers().await?;
        report.dispatched.add(self.drain_outbox().await?);
        Ok(report)
    }

    /// Call [`Host::tick`] every `period` until `shutdown` resolves. Failures are reported to
    /// telemetry and the loop goes on.
    pub async fn run(&self, period: Duration, shutdown: impl Future<Output = ()>) {
        let mut shutdown = std::pin::pin!(shutdown);
        loop {
            if let Err(error) = self.tick().await {
                self.telemetry.event(
                    Level::Error,
                    "machine.tick_failed",
                    &[
                        ("machine", M::NAME.into()),
                        ("error", error.to_string().as_str().into()),
                    ],
                );
            }
            tokio::select! {
                _ = &mut shutdown => return,
                _ = tokio::time::sleep(period) => {}
            }
        }
    }
}
