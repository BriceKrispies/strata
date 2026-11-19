//! What generated code implements for an implementation-level machine.
//!
//! The compiler turns each `behavior` with `level: "implementation"` into one type implementing
//! [`Machine`]: a pure `step` from `(state, vars, event)` to an [`Outcome`], plus the conversion
//! between the machine's instance and the record it is stored in (`instances.storedIn`). The
//! [`Host`](crate::Host) does everything around that call: loading, the transaction, persisting,
//! the outbox and the timers.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fmt::{self, Debug};
use std::time::Duration;

/// The isolation a machine's transition transaction runs at (`transaction.isolation`).
///
/// A store may run a transaction at a stronger level than declared, never a weaker one. The
/// levels are ordered from weakest to strongest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Isolation {
    /// Every read sees only committed data.
    ReadCommitted,
    /// Every read in the transaction sees one consistent snapshot; writes are checked for
    /// conflicts on the same rows.
    Snapshot,
    /// The transaction behaves as if it ran alone.
    Serializable,
}

/// The delivery guarantee of an emitted event (`event.delivery`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Delivery {
    /// Dispatched once after commit. Lost if the dispatch fails or the process stops first.
    AtMostOnce,
    /// Written to the outbox in the transition's transaction and dispatched after commit until
    /// a dispatch succeeds. May arrive more than once; receivers must be idempotent.
    AtLeastOnce,
}

impl Delivery {
    /// The specification's spelling: `at-most-once` or `at-least-once`.
    pub fn name(self) -> &'static str {
        match self {
            Delivery::AtMostOnce => "at-most-once",
            Delivery::AtLeastOnce => "at-least-once",
        }
    }
}

/// Who an emitted event is addressed to: a behavior, component or contract id from the
/// specification. The [`Dispatcher`](crate::Dispatcher) decides how to reach it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Receiver(pub String);

impl Receiver {
    /// A receiver named by `id`.
    pub fn new(id: impl Into<String>) -> Self {
        Receiver(id.into())
    }

    /// The receiver's id.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Receiver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One event a transition emits.
#[derive(Clone, Debug, PartialEq)]
pub struct Emit<E> {
    /// Where it goes.
    pub receiver: Receiver,
    /// How hard the runtime tries to get it there.
    pub delivery: Delivery,
    /// The event itself.
    pub event: E,
}

/// A change to the instance's timers, made by a transition.
///
/// Timers are named per instance. Setting a timer that is already set replaces it; cancelling a
/// timer that is not set does nothing. When a timer fires, its event is handed to the same
/// instance as an incoming event.
#[derive(Clone, Debug, PartialEq)]
pub enum TimerCommand<E> {
    /// Fire `event` at this instance `after` this long, measured from the transition.
    Set {
        /// The timer's name within the instance.
        name: String,
        /// How long from now.
        after: Duration,
        /// What the instance receives when the timer fires (an event with `elapsed: true`).
        event: E,
    },
    /// Forget the timer called `name`.
    Cancel {
        /// The timer's name within the instance.
        name: String,
    },
}

/// The result of one step: where the instance goes and what it asks the runtime to do.
pub struct Outcome<M: Machine + ?Sized> {
    /// The next state.
    pub state: M::State,
    /// The next variables.
    pub vars: M::Vars,
    /// Emitted events, in the order the transition's effects emit them.
    pub emits: Vec<Emit<M::Emitted>>,
    /// Timer changes, applied in order.
    pub timers: Vec<TimerCommand<M::Event>>,
}

impl<M: Machine + ?Sized> Outcome<M> {
    /// Go to `state` with `vars`, emitting nothing and leaving timers alone.
    pub fn to(state: M::State, vars: M::Vars) -> Self {
        Outcome {
            state,
            vars,
            emits: Vec::new(),
            timers: Vec::new(),
        }
    }

    /// Also emit `event` to `receiver` with `delivery`.
    pub fn emit(mut self, receiver: &str, delivery: Delivery, event: M::Emitted) -> Self {
        self.emits.push(Emit {
            receiver: Receiver::new(receiver),
            delivery,
            event,
        });
        self
    }

    /// Also set the timer `name` to fire `event` after `after`.
    pub fn set_timer(mut self, name: &str, after: Duration, event: M::Event) -> Self {
        self.timers.push(TimerCommand::Set {
            name: name.to_string(),
            after,
            event,
        });
        self
    }

    /// Also cancel the timer `name`.
    pub fn cancel_timer(mut self, name: &str) -> Self {
        self.timers.push(TimerCommand::Cancel {
            name: name.to_string(),
        });
        self
    }
}

impl<M: Machine + ?Sized> Debug for Outcome<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Outcome")
            .field("state", &self.state)
            .field("vars", &self.vars)
            .field("emits", &self.emits)
            .field("timers", &self.timers)
            .finish()
    }
}

/// Why a step produced no outcome. The host aborts the transaction: nothing is persisted,
/// emitted or scheduled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepError {
    /// No transition from `state` on `event` is enabled and the state does not ignore the event.
    NoTransition {
        /// The state the instance was in.
        state: String,
        /// The event it received.
        event: String,
    },
    /// The instance is in a terminal state and accepts no events.
    Terminal {
        /// The terminal state.
        state: String,
    },
    /// The step refused for another reason the generated code states.
    Rejected(String),
}

impl fmt::Display for StepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StepError::NoTransition { state, event } => {
                write!(f, "no transition from {state} on {event}")
            }
            StepError::Terminal { state } => write!(f, "{state} is terminal"),
            StepError::Rejected(why) => write!(f, "rejected: {why}"),
        }
    }
}

impl std::error::Error for StepError {}

/// A stored record that does not hold a valid instance of the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordError(pub String);

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "record does not hold an instance: {}", self.0)
    }
}

impl std::error::Error for RecordError {}

/// An implementation-level machine. Generated code implements this once per behavior.
///
/// Everything here is pure: no clock, no storage, no randomness. Elapsed time reaches the machine
/// only as events fired by timers it set.
pub trait Machine: Send + Sync + 'static {
    /// The behavior's id. Names the machine's timers and outbox entries in storage.
    const NAME: &'static str;
    /// The record its instances are stored in (`instances.storedIn`). Names the collection.
    const RECORD: &'static str;
    /// The isolation of the transition transaction (`transaction.isolation`).
    const ISOLATION: Isolation;

    /// The machine's control states.
    type State: Clone + Debug + Send + Sync + 'static;
    /// The machine's variables.
    type Vars: Clone + Debug + Send + Sync + 'static;
    /// Events the machine receives, including the elapsed-time events its timers fire.
    type Event: Serialize + DeserializeOwned + Clone + Debug + Send + Sync + 'static;
    /// Events the machine emits.
    type Emitted: Serialize + DeserializeOwned + Clone + Debug + Send + Sync + 'static;
    /// The instance key (`instances.key`). Its JSON text is the storage key, so two keys are the
    /// same instance exactly when they serialise identically.
    type Key: Serialize + DeserializeOwned + Clone + Debug + Send + Sync + 'static;
    /// The record type instances are stored in.
    type Record: Serialize + DeserializeOwned + Clone + Debug + Send + Sync + 'static;

    /// The state and variables of an instance that has no record yet.
    fn initial(key: &Self::Key) -> (Self::State, Self::Vars);

    /// Read the instance out of its record.
    fn from_record(record: &Self::Record) -> Result<(Self::State, Self::Vars), RecordError>;

    /// Write the instance into a record. `previous` is the record as loaded, when there was one,
    /// so fields the machine does not own are carried over.
    fn to_record(
        key: &Self::Key,
        state: &Self::State,
        vars: &Self::Vars,
        previous: Option<&Self::Record>,
    ) -> Self::Record;

    /// Take one transition: the first enabled transition from `state` on `event`, in declaration
    /// order, or an ignore (the same state and variables, nothing emitted).
    fn step(
        state: &Self::State,
        vars: &Self::Vars,
        event: &Self::Event,
    ) -> Result<Outcome<Self>, StepError>;
}
