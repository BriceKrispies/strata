//! The in-memory machine store: the reference implementation of the storage traits.
//!
//! One mutex guards everything. A transaction holds it from start to end and works on a copy,
//! which replaces the original only when the work succeeds, so transactions are serializable and
//! a failed one leaves nothing behind. Copying the whole store per transaction is fine for tests
//! and simulation, which is what this store is for.

use crate::clock::Time;
use crate::machine::Isolation;
use crate::store::{
    DeadLetter, NewEntry, NewMessage, NewTimer, OutboxEntry, OutboxStore, QueueEntry, QueueStore,
    RecordStore, StorageError, StoredRecord, TimerEntry, TimerStore, Tx,
};
use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

#[derive(Clone, Debug, Default)]
struct Data {
    records: BTreeMap<(String, String), StoredRecord>,
    /// When each record was created, as a counter: the order of [`Tx::keys`].
    created: BTreeMap<(String, String), u64>,
    outbox: BTreeMap<u64, OutboxEntry>,
    timers: BTreeMap<u64, TimerEntry>,
    queue: BTreeMap<u64, QueueEntry>,
    dead: Vec<DeadLetter>,
    last_message: u64,
    last_timer: u64,
    last_record: u64,
    last_entry: u64,
}

fn in_queue_order(entries: impl Iterator<Item = QueueEntry>) -> Vec<QueueEntry> {
    let mut all: Vec<QueueEntry> = entries.collect();
    all.sort_by_key(|e| (e.due, e.seq));
    all
}

/// A machine store held in memory. Its contents live as long as the value.
#[derive(Debug, Default)]
pub struct MemoryStore {
    data: Mutex<Data>,
}

impl MemoryStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> Result<MutexGuard<'_, Data>, StorageError> {
        self.data
            .lock()
            .map_err(|_| StorageError::Unavailable("a writer panicked".into()))
    }
}

struct MemoryTx<'a> {
    data: &'a mut Data,
}

impl Tx for MemoryTx<'_> {
    fn load(&mut self, collection: &str, key: &str) -> Result<Option<StoredRecord>, StorageError> {
        Ok(self
            .data
            .records
            .get(&(collection.to_string(), key.to_string()))
            .cloned())
    }

    fn save(
        &mut self,
        collection: &str,
        key: &str,
        data: &str,
        expected: Option<u64>,
    ) -> Result<u64, StorageError> {
        let slot = (collection.to_string(), key.to_string());
        let found = self.data.records.get(&slot).map(|r| r.version);
        if found != expected {
            return Err(StorageError::Conflict(format!(
                "{collection} {key}: expected version {expected:?}, found {found:?}"
            )));
        }
        let version = found.unwrap_or(0) + 1;
        if found.is_none() {
            self.data.last_record += 1;
            self.data
                .created
                .insert(slot.clone(), self.data.last_record);
        }
        self.data.records.insert(
            slot,
            StoredRecord {
                data: data.to_string(),
                version,
            },
        );
        Ok(version)
    }

    fn enqueue(&mut self, message: &NewMessage) -> Result<u64, StorageError> {
        self.data.last_message += 1;
        let id = self.data.last_message;
        self.data.outbox.insert(
            id,
            OutboxEntry {
                id,
                source: message.source.clone(),
                key: message.key.clone(),
                receiver: message.receiver.clone(),
                event: message.event.clone(),
                created_at: message.created_at,
                delivery: message.delivery,
                not_before: message.not_before,
                attempts: 0,
                next_attempt_at: message.next_attempt_at,
                last_error: None,
            },
        );
        Ok(id)
    }

    fn set_timer(&mut self, timer: &NewTimer) -> Result<u64, StorageError> {
        self.cancel_timer(&timer.machine, &timer.key, &timer.name)?;
        self.data.last_timer += 1;
        let id = self.data.last_timer;
        self.data.timers.insert(
            id,
            TimerEntry {
                id,
                machine: timer.machine.clone(),
                key: timer.key.clone(),
                name: timer.name.clone(),
                fire_at: timer.fire_at,
                event: timer.event.clone(),
            },
        );
        Ok(id)
    }

    fn cancel_timer(&mut self, machine: &str, key: &str, name: &str) -> Result<bool, StorageError> {
        let before = self.data.timers.len();
        self.data
            .timers
            .retain(|_, t| !(t.machine == machine && t.key == key && t.name == name));
        Ok(self.data.timers.len() != before)
    }

    fn take_timer(&mut self, id: u64) -> Result<bool, StorageError> {
        Ok(self.data.timers.remove(&id).is_some())
    }

    fn keys(&mut self, collection: &str) -> Result<Vec<String>, StorageError> {
        let mut keys: Vec<(u64, String)> = self
            .data
            .created
            .iter()
            .filter(|((c, _), _)| c == collection)
            .map(|((_, k), n)| (*n, k.clone()))
            .collect();
        keys.sort();
        Ok(keys.into_iter().map(|(_, k)| k).collect())
    }

    fn push(&mut self, entry: &NewEntry) -> Result<u64, StorageError> {
        self.data.last_entry += 1;
        let seq = self.data.last_entry;
        self.data.queue.insert(
            seq,
            QueueEntry {
                seq,
                due: entry.due,
                event: entry.event.clone(),
                target: entry.target.clone(),
                timer: entry.timer.clone(),
                source: entry.source.clone(),
                source_key: entry.source_key.clone(),
            },
        );
        Ok(seq)
    }

    fn take_entry(&mut self, seq: u64) -> Result<bool, StorageError> {
        Ok(self.data.queue.remove(&seq).is_some())
    }

    fn drop_timer_entry(
        &mut self,
        machine: &str,
        key: &str,
        name: &str,
    ) -> Result<bool, StorageError> {
        let before = self.data.queue.len();
        self.data.queue.retain(|_, e| {
            !(e.timer.as_deref() == Some(name)
                && e.target
                    .as_ref()
                    .is_some_and(|(m, k)| m == machine && k == key))
        });
        Ok(self.data.queue.len() != before)
    }

    fn dead_letter(
        &mut self,
        entry: &QueueEntry,
        error: &str,
        at: Time,
    ) -> Result<(), StorageError> {
        self.data.dead.push(DeadLetter {
            entry: entry.clone(),
            error: error.to_string(),
            at,
        });
        Ok(())
    }
}

impl QueueStore for MemoryStore {
    fn head(&self) -> Result<Option<QueueEntry>, StorageError> {
        Ok(in_queue_order(self.lock()?.queue.values().cloned())
            .into_iter()
            .next())
    }

    fn queued(&self, limit: usize) -> Result<Vec<QueueEntry>, StorageError> {
        let mut all = in_queue_order(self.lock()?.queue.values().cloned());
        all.truncate(limit);
        Ok(all)
    }

    fn dead_letters(&self) -> Result<Vec<DeadLetter>, StorageError> {
        Ok(self.lock()?.dead.clone())
    }
}

impl RecordStore for MemoryStore {
    fn supports(&self, _: Isolation) -> bool {
        true
    }

    fn transact<T, E>(
        &self,
        _: Isolation,
        work: impl FnOnce(&mut dyn Tx) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<StorageError>,
    {
        let mut data = self.lock()?;
        let mut working = data.clone();
        let result = work(&mut MemoryTx { data: &mut working })?;
        *data = working;
        Ok(result)
    }
}

impl OutboxStore for MemoryStore {
    fn due_messages(
        &self,
        source: &str,
        now: Time,
        limit: usize,
    ) -> Result<Vec<OutboxEntry>, StorageError> {
        let mut due: Vec<OutboxEntry> = self
            .lock()?
            .outbox
            .values()
            .filter(|m| m.source == source && m.next_attempt_at <= now && m.not_before <= now)
            .cloned()
            .collect();
        due.sort_by_key(|m| (m.not_before, m.id));
        due.truncate(limit);
        Ok(due)
    }

    fn mark_delivered(&self, id: u64) -> Result<bool, StorageError> {
        Ok(self.lock()?.outbox.remove(&id).is_some())
    }

    fn defer(
        &self,
        id: u64,
        attempts: u32,
        next_attempt_at: Time,
        error: &str,
    ) -> Result<(), StorageError> {
        if let Some(entry) = self.lock()?.outbox.get_mut(&id) {
            entry.attempts = attempts;
            entry.next_attempt_at = next_attempt_at;
            entry.last_error = Some(error.to_string());
        }
        Ok(())
    }

    fn pending(&self) -> Result<u64, StorageError> {
        Ok(self.lock()?.outbox.len() as u64)
    }
}

impl TimerStore for MemoryStore {
    fn due_timers(
        &self,
        machine: &str,
        now: Time,
        limit: usize,
    ) -> Result<Vec<TimerEntry>, StorageError> {
        let data = self.lock()?;
        let mut due: Vec<TimerEntry> = data
            .timers
            .values()
            .filter(|t| t.machine == machine && t.fire_at <= now)
            .cloned()
            .collect();
        due.sort_by_key(|t| (t.fire_at, t.id));
        due.truncate(limit);
        Ok(due)
    }
}
