//! The storage the host needs: instance records, an outbox and timers.
//!
//! These traits are deliberately untyped: records, events and keys cross them as JSON text, and
//! the [`Host`](crate::Host) does the typed encoding. One store therefore holds the instances of
//! every machine, and an adapter is written once, not per machine.
//!
//! All three traits are synchronous, like the tenant store. The host calls them from a blocking
//! worker thread, never on the async executor.
//!
//! Every write the host makes for one transition goes through one [`Tx`] inside one
//! [`RecordStore::transact`] call, and is committed whole or not at all. The system host
//! ([`System`](crate::System)) likewise makes every write of one queue entry's step, across all
//! the instances it reaches, in one transaction.
//!
//! The isolation a transaction is asked for is a floor: every store here runs every transaction
//! serializable (one writer at a time), which satisfies each declared level.

use crate::clock::Time;
use crate::machine::{Delivery, Isolation};
use std::fmt;

/// Why a storage operation failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageError {
    /// The store could not be reached or the operation could not run. Retrying may help.
    Unavailable(String),
    /// What the store holds cannot be read back.
    Corrupt(String),
    /// A concurrent writer changed what this transaction read. Retrying the whole transaction
    /// may succeed.
    Conflict(String),
    /// The operation itself is wrong, such as a constraint the write breaks.
    Invalid(String),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageError::Unavailable(why) => write!(f, "storage unavailable: {why}"),
            StorageError::Corrupt(why) => write!(f, "storage corrupt: {why}"),
            StorageError::Conflict(why) => write!(f, "storage conflict: {why}"),
            StorageError::Invalid(why) => write!(f, "storage refused: {why}"),
        }
    }
}

impl std::error::Error for StorageError {}

/// A record as stored: its JSON text and a version that grows by one on every save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredRecord {
    /// The record's JSON text.
    pub data: String,
    /// 1 after the first save, then one more per save.
    pub version: u64,
}

/// An event to put in the outbox: an `at-least-once` emit, or a delayed emit of either delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewMessage {
    /// The emitting machine's name.
    pub source: String,
    /// The emitting instance's key (JSON text).
    pub key: String,
    /// The receiver's id.
    pub receiver: String,
    /// The event's JSON text.
    pub event: String,
    /// When the transition committed.
    pub created_at: Time,
    /// Its delivery guarantee: an `at-least-once` entry is retried until delivered, an
    /// `at-most-once` one is removed and dispatched once.
    pub delivery: Delivery,
    /// The earliest time it may be dispatched: the step's time plus the delay of a delayed emit,
    /// otherwise the step's time.
    pub not_before: Time,
    /// When the outbox may first hand it out for dispatch, never before `not_before`. For an
    /// immediate emit the host sets this a little after the commit, so that its own dispatch
    /// right after commit is not raced by the outbox drain.
    pub next_attempt_at: Time,
}

/// An entry of the outbox: an event not yet dispatched (an `at-least-once` one not yet
/// delivered).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboxEntry {
    /// Assigned by the store when the entry is written, unique and never reused. Receivers use
    /// it to recognise a redelivery.
    pub id: u64,
    /// The emitting machine's name.
    pub source: String,
    /// The emitting instance's key (JSON text).
    pub key: String,
    /// The receiver's id.
    pub receiver: String,
    /// The event's JSON text.
    pub event: String,
    /// When the transition committed.
    pub created_at: Time,
    /// Its delivery guarantee.
    pub delivery: Delivery,
    /// The earliest time it may be dispatched.
    pub not_before: Time,
    /// Dispatch attempts that failed so far.
    pub attempts: u32,
    /// When it is next due for dispatch.
    pub next_attempt_at: Time,
    /// Why the last attempt failed.
    pub last_error: Option<String>,
}

/// A timer to set. It replaces any timer of the same machine, key and name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewTimer {
    /// The machine's name.
    pub machine: String,
    /// The instance key (JSON text).
    pub key: String,
    /// The timer's name within the instance.
    pub name: String,
    /// When it fires.
    pub fire_at: Time,
    /// The JSON text of the event it delivers.
    pub event: String,
}

/// A set timer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimerEntry {
    /// Assigned by the store when the timer is set; setting it again gives a new id.
    pub id: u64,
    /// The machine's name.
    pub machine: String,
    /// The instance key (JSON text).
    pub key: String,
    /// The timer's name within the instance.
    pub name: String,
    /// When it fires.
    pub fire_at: Time,
    /// The JSON text of the event it delivers.
    pub event: String,
}

/// An entry to put on the system queue: an input, an emit or a timer, due at a time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewEntry {
    /// When it is delivered. Entries are delivered in `(due, seq)` order.
    pub due: Time,
    /// The event's JSON text: `{"event": <id>, "payload": {...}}`.
    pub event: String,
    /// The one instance it goes to, as `(machine, key JSON text)`: set for a timer, which only
    /// its own instance receives. `None`: the routes decide the receivers.
    pub target: Option<(String, String)>,
    /// For a timer, its name within the target instance; setting a timer of the same name
    /// replaces this entry.
    pub timer: Option<String>,
    /// Who put it there: a machine's name, or `input` for an event from outside.
    pub source: String,
    /// The emitting instance's key (JSON text); empty for an input.
    pub source_key: String,
}

/// An entry of the system queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueEntry {
    /// Assigned by the store when the entry is written: unique, never reused, and increasing,
    /// so entries due at the same time keep the order they were created in.
    pub seq: u64,
    /// When it is delivered.
    pub due: Time,
    /// The event's JSON text.
    pub event: String,
    /// The one instance it goes to, for a timer.
    pub target: Option<(String, String)>,
    /// The timer's name, for a timer.
    pub timer: Option<String>,
    /// Who put it there.
    pub source: String,
    /// The emitting instance's key (JSON text).
    pub source_key: String,
}

/// A queue entry whose delivery failed, kept with the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeadLetter {
    /// The entry as it was on the queue.
    pub entry: QueueEntry,
    /// Why its step failed.
    pub error: String,
    /// When it was set aside.
    pub at: Time,
}

/// The writes and reads of one transaction.
pub trait Tx {
    /// The record of `key` in `collection`, if there is one.
    fn load(&mut self, collection: &str, key: &str) -> Result<Option<StoredRecord>, StorageError>;

    /// Store `data` as the record of `key` in `collection`. `expected` is the version that was
    /// loaded (`None` when there was no record); a different stored version is a
    /// [`StorageError::Conflict`]. Returns the new version.
    fn save(
        &mut self,
        collection: &str,
        key: &str,
        data: &str,
        expected: Option<u64>,
    ) -> Result<u64, StorageError>;

    /// Put an event in the outbox. Returns its id.
    fn enqueue(&mut self, message: &NewMessage) -> Result<u64, StorageError>;

    /// Set a timer, replacing one of the same machine, key and name. Returns its id.
    fn set_timer(&mut self, timer: &NewTimer) -> Result<u64, StorageError>;

    /// Remove the timer `name` of an instance. Returns whether one was set.
    fn cancel_timer(&mut self, machine: &str, key: &str, name: &str) -> Result<bool, StorageError>;

    /// Remove the timer with this id, if it is still set. Returns whether it was: `false` means
    /// it was cancelled or replaced since it was listed, and must not fire.
    fn take_timer(&mut self, id: u64) -> Result<bool, StorageError>;

    /// The keys (JSON text) of every record in `collection`, in the order the records were
    /// created.
    fn keys(&mut self, collection: &str) -> Result<Vec<String>, StorageError>;

    /// Put an entry on the system queue. Returns its sequence number.
    fn push(&mut self, entry: &NewEntry) -> Result<u64, StorageError>;

    /// Remove the queue entry `seq`. Returns whether it was there.
    fn take_entry(&mut self, seq: u64) -> Result<bool, StorageError>;

    /// Remove the queued timer `name` of an instance (`target` `(machine, key)`). Returns
    /// whether one was queued.
    fn drop_timer_entry(
        &mut self,
        machine: &str,
        key: &str,
        name: &str,
    ) -> Result<bool, StorageError>;

    /// Keep `entry`, which has been taken off the queue, as a dead letter with `error`.
    fn dead_letter(
        &mut self,
        entry: &QueueEntry,
        error: &str,
        at: Time,
    ) -> Result<(), StorageError>;
}

/// Records, read and written in transactions.
pub trait RecordStore: Send + Sync + 'static {
    /// Whether this store can run a transaction at `isolation` or stronger.
    fn supports(&self, isolation: Isolation) -> bool;

    /// Run `work` in one transaction at `isolation` or stronger. Commit when it returns `Ok`;
    /// roll back, keeping none of its writes, when it returns `Err` or the commit fails.
    fn transact<T, E>(
        &self,
        isolation: Isolation,
        work: impl FnOnce(&mut dyn Tx) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<StorageError>;
}

/// The outbox, outside any transition's transaction.
pub trait OutboxStore: Send + Sync + 'static {
    /// Entries of `source` whose `next_attempt_at` and `not_before` are at or before `now`, in
    /// due order: by `not_before`, then by id.
    fn due_messages(
        &self,
        source: &str,
        now: Time,
        limit: usize,
    ) -> Result<Vec<OutboxEntry>, StorageError>;

    /// The entry was delivered: remove it. Removing an entry that is gone does nothing.
    fn mark_delivered(&self, id: u64) -> Result<(), StorageError>;

    /// A dispatch failed: record the attempt and when to try again.
    fn defer(
        &self,
        id: u64,
        attempts: u32,
        next_attempt_at: Time,
        error: &str,
    ) -> Result<(), StorageError>;

    /// How many entries are waiting, across all sources.
    fn pending(&self) -> Result<u64, StorageError>;
}

/// Timers, outside any transition's transaction.
pub trait TimerStore: Send + Sync + 'static {
    /// Timers of `machine` that fire at or before `now`, earliest first.
    fn due_timers(
        &self,
        machine: &str,
        now: Time,
        limit: usize,
    ) -> Result<Vec<TimerEntry>, StorageError>;
}

/// The system queue, outside any step's transaction.
pub trait QueueStore: Send + Sync + 'static {
    /// The entry first in `(due, seq)` order, whether or not it is due yet.
    fn head(&self) -> Result<Option<QueueEntry>, StorageError>;

    /// Up to `limit` entries in `(due, seq)` order.
    fn queued(&self, limit: usize) -> Result<Vec<QueueEntry>, StorageError>;

    /// The dead letters, oldest first.
    fn dead_letters(&self) -> Result<Vec<DeadLetter>, StorageError>;
}

/// Everything the hosts need from a store.
pub trait MachineStore: RecordStore + OutboxStore + TimerStore + QueueStore {}

impl<S: RecordStore + OutboxStore + TimerStore + QueueStore> MachineStore for S {}
