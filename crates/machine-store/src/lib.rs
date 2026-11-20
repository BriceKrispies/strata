//! The runtime's machine store on the persistence contract.
//!
//! [`StorageApiMachineStore`] implements the runtime's `MachineStore` (records, outbox and
//! timers) on a [`strata_storage_api::Store`], so a `Host` and the machines generated for it read
//! and write the same store the services use, memory or SQLite, with no change to the generated
//! code. It lives outside `strata-runtime` because the runtime depends on no Strata crate.
//!
//! # Mapping
//!
//! Everything lives in one tenant of the store, named when the adapter is built.
//!
//! - **A runtime transaction is one `Store::commit`.** The transaction reads through
//!   `Store::load` and buffers its writes; when the work returns `Ok` the buffer becomes one
//!   state delta with its events and journal entry. When the work returns `Err` nothing is
//!   committed. A transaction that wrote nothing commits nothing.
//! - **A machine record** is a store record of the object type named after its collection
//!   (`rec-published-page` becomes `rec_published_page`), with two fields: `key`, the instance key's JSON text,
//!   and `data`, the record's JSON text. It is found by a `ByField` need on `key`. The store
//!   assigns the numeric record id; the runtime's key maps to it through the `key` field.
//! - **The record version** is the store record's own `version`: 1 on the first save, one more
//!   per save. A save whose expected version differs from the loaded one is a
//!   `StorageError::Conflict`, as in the other stores.
//! - **Events.** Each machine record created, updated or deleted by a commit also appears as a
//!   `RecordCreated`, `RecordUpdated` or `RecordDeleted` domain event, so it takes its place in
//!   the tenant's event log and the store's outbox for publishing. Outbox and timer rows are
//!   internal and emit no events.
//! - **The runtime outbox** is the object type `runtime_outbox` in the same tenant, one record
//!   per message, so an entry is written in the same commit as the record it follows and is
//!   visible only after that commit. The store's own outbox (a cumulative log of domain events
//!   with a published mark) has no per-entry attempts, not-before time or backoff, so it cannot
//!   carry the runtime's messages; the runtime's dispatch bookkeeping is kept in these records.
//!   The entry id is the store record id: unique, never reused.
//! - **Timers** are the object type `runtime_timer`, one record per `(machine, key, name)`,
//!   written in the same commit as the transition that set them. Setting a timer again replaces
//!   the record with a new one, so the id changes.
//!
//! # Concurrency
//!
//! The store's guard is the tenant's revision, not a record's version: any other commit to the
//! tenant between this transaction's first read and its commit is a `StorageError::Conflict`,
//! including one to an unrelated record. Within one adapter writers are also serialised by a
//! mutex, so conflicts arise only from another writer on the same tenant (another process or
//! adapter). Every read in a transaction must see the revision the first one saw, so a
//! transaction is serializable.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use strata_kernel::ext::*;
use strata_kernel::{
    canon, Context, DeltaOp, DomainEvent, EventBody, FieldId, Intent, Need, ObjectTypeId,
    Principal, PrincipalId, Record, RecordId, StateDelta, StateRevision, TenantId, TenantSchema,
    Timestamp, Value, KERNEL_VERSION,
};
use strata_runtime::store::{
    DeadLetter, NewEntry, NewMessage, NewTimer, OutboxEntry, OutboxStore, QueueEntry, QueueStore,
    RecordStore, StoredRecord, TimerEntry, TimerStore, Tx,
};
use strata_runtime::{Clock, Delivery, Isolation, StorageError, Time};
use strata_storage_api::{Commit, JournalEntry, Store, StoreError};

/// The object type of the runtime's outbox messages.
pub const OUTBOX: &str = "runtime_outbox";
/// The object type of the runtime's timers.
pub const TIMERS: &str = "runtime_timer";
/// The object type of the system queue's entries.
pub const QUEUE: &str = "runtime_queue";
/// The object type of the system queue's dead letters.
pub const DEAD_LETTERS: &str = "runtime_dead_letter";

/// The adapter's own object types.
const INTERNAL: [&str; 4] = [OUTBOX, TIMERS, QUEUE, DEAD_LETTERS];

fn storage(error: StoreError) -> StorageError {
    match error {
        StoreError::Conflict { expected, found } => StorageError::Conflict(format!(
            "tenant revision changed: based on {expected:?}, store is at {found:?}"
        )),
        StoreError::NotFound(what) => StorageError::Invalid(format!("not found: {what}")),
        StoreError::Invalid(why) => StorageError::Invalid(why),
        StoreError::Unavailable(why) => StorageError::Unavailable(why),
        StoreError::Corrupt(why) => StorageError::Corrupt(why),
    }
}

fn field(name: &str) -> FieldId {
    FieldId::new(name).expect("the adapter's field names are valid identifiers")
}

fn object_type(name: &str) -> Result<ObjectTypeId, StorageError> {
    ObjectTypeId::new(name).map_err(|e| StorageError::Invalid(e.to_string()))
}

/// The object type of a machine collection: `-` becomes `_`, and the adapter's own names are
/// refused.
fn collection_object(collection: &str) -> Result<ObjectTypeId, StorageError> {
    let name = collection.replace('-', "_");
    if INTERNAL.contains(&name.as_str()) {
        return Err(StorageError::Invalid(format!(
            "collection {collection} collides with the runtime's own records"
        )));
    }
    object_type(&name)
}

fn text(record: &Record, name: &str) -> Result<String, StorageError> {
    match record.get(&field(name)) {
        Value::Str(s) => Ok(s.clone()),
        other => Err(StorageError::Corrupt(format!(
            "{} {}: field {name} holds {}, not a string",
            record.object,
            record.id,
            other.kind()
        ))),
    }
}

fn int(record: &Record, name: &str) -> Result<i64, StorageError> {
    match record.get(&field(name)) {
        Value::Int(n) => Ok(*n),
        other => Err(StorageError::Corrupt(format!(
            "{} {}: field {name} holds {}, not an int",
            record.object,
            record.id,
            other.kind()
        ))),
    }
}

fn strings(pairs: &[(&str, &str)]) -> BTreeMap<FieldId, Value> {
    pairs
        .iter()
        .map(|(name, value)| (field(name), Value::Str((*value).to_string())))
        .collect()
}

fn outbox_entry(record: &Record) -> Result<OutboxEntry, StorageError> {
    Ok(OutboxEntry {
        id: record.id.0,
        source: text(record, "source")?,
        key: text(record, "key")?,
        receiver: text(record, "receiver")?,
        event: text(record, "event")?,
        created_at: Time(int(record, "created_at")?),
        delivery: match text(record, "delivery")?.as_str() {
            "at-most-once" => Delivery::AtMostOnce,
            _ => Delivery::AtLeastOnce,
        },
        not_before: Time(int(record, "not_before")?),
        attempts: u32::try_from(int(record, "attempts")?)
            .map_err(|_| StorageError::Corrupt("outbox attempts out of range".into()))?,
        next_attempt_at: Time(int(record, "next_attempt_at")?),
        last_error: match record.get(&field("last_error")) {
            Value::Str(s) => Some(s.clone()),
            _ => None,
        },
    })
}

fn timer_entry(record: &Record) -> Result<TimerEntry, StorageError> {
    Ok(TimerEntry {
        id: record.id.0,
        machine: text(record, "machine")?,
        key: text(record, "key")?,
        name: text(record, "name")?,
        fire_at: Time(int(record, "fire_at")?),
        event: text(record, "event")?,
    })
}

fn optional_text(record: &Record, name: &str) -> Option<String> {
    match record.get(&field(name)) {
        Value::Str(s) => Some(s.clone()),
        _ => None,
    }
}

fn queue_entry(record: &Record) -> Result<QueueEntry, StorageError> {
    let machine = optional_text(record, "target_machine");
    let key = optional_text(record, "target_key");
    Ok(QueueEntry {
        seq: record.id.0,
        due: Time(int(record, "due")?),
        event: text(record, "event")?,
        target: machine.zip(key),
        timer: optional_text(record, "timer"),
        source: text(record, "source")?,
        source_key: text(record, "source_key")?,
    })
}

/// The fields of a queue entry (without its sequence number, which is the record id).
fn queue_fields(
    due: Time,
    event: &str,
    target: Option<&(String, String)>,
    timer: Option<&str>,
    source: &str,
    source_key: &str,
) -> BTreeMap<FieldId, Value> {
    let mut fields = strings(&[
        ("event", event),
        ("source", source),
        ("source_key", source_key),
    ]);
    fields.insert(field("due"), Value::Int(due.0));
    if let Some((machine, key)) = target {
        fields.insert(field("target_machine"), Value::Str(machine.clone()));
        fields.insert(field("target_key"), Value::Str(key.clone()));
        if let Some(name) = timer {
            fields.insert(field("timer"), Value::Str(name.to_string()));
            fields.insert(field("slot"), Value::Str(timer_slot(machine, key, name)));
        }
    }
    fields
}

fn context(tenant: &TenantId, now: Time) -> Context {
    Context {
        tenant: tenant.clone(),
        principal: Principal::System,
        session: None,
        now: Timestamp(now.0),
        correlation: "runtime".into(),
        verified_address: None,
        idempotency_key: None,
        language: None,
    }
}

/// The identity of a timer within the store: its machine, instance key and name.
fn timer_slot(machine: &str, key: &str, name: &str) -> String {
    serde_json::to_string(&[machine, key, name]).unwrap_or_default()
}

/// The tenant's counters as the transaction first saw them.
#[derive(Clone, Copy, Debug)]
struct Header {
    revision: StateRevision,
    event_seq: u64,
}

/// A record as it was when the transaction began and as it will be after it.
struct Slot {
    before: Option<Arc<Record>>,
    after: Option<Arc<Record>>,
}

/// One runtime transaction: reads from the store, buffers writes, and commits them as one
/// [`Commit`] in [`ApiTx::finish`].
struct ApiTx<'a> {
    store: &'a dyn Store,
    tenant: &'a TenantId,
    now: Time,
    header: Header,
    next_record: u64,
    pending: BTreeMap<(ObjectTypeId, RecordId), Slot>,
}

impl<'a> ApiTx<'a> {
    /// Open a transaction, creating the tenant first if the store does not have it.
    fn begin(store: &'a dyn Store, tenant: &'a TenantId, now: Time) -> Result<Self, StorageError> {
        for _ in 0..2 {
            let state = store.load(tenant, &[]).map_err(storage)?;
            if let Some(found) = state.tenant {
                return Ok(ApiTx {
                    store,
                    tenant,
                    now,
                    header: Header {
                        revision: found.revision,
                        event_seq: found.event_seq,
                    },
                    next_record: found.next_record,
                    pending: BTreeMap::new(),
                });
            }
            Self::create_tenant(store, tenant, now)?;
        }
        Err(StorageError::Unavailable(
            "the tenant could not be created".into(),
        ))
    }

    fn create_tenant(store: &dyn Store, tenant: &TenantId, now: Time) -> Result<(), StorageError> {
        let delta = StateDelta {
            tenant: tenant.clone(),
            base_revision: None,
            new_revision: StateRevision(1),
            next_record: 1,
            event_seq: 1,
            ops: vec![
                DeltaOp::CreateTenant {
                    name: tenant.to_string(),
                },
                DeltaOp::PutSchema(Arc::new(TenantSchema::genesis())),
            ],
        };
        let events = vec![DomainEvent {
            tenant: tenant.clone(),
            seq: 1,
            revision: StateRevision(1),
            at: Timestamp(now.0),
            principal: Principal::System,
            correlation: "runtime".into(),
            body: EventBody::TenantCreated {
                name: tenant.to_string(),
            },
        }];
        let owner = PrincipalId::new("runtime").expect("a valid principal id");
        let commit = Commit {
            journal: JournalEntry {
                tenant: tenant.clone(),
                base_revision: None,
                revision: StateRevision(1),
                kernel_version: KERNEL_VERSION.to_string(),
                intent: Intent::CreateTenant {
                    name: tenant.to_string(),
                    owner,
                },
                context: context(tenant, now),
                delta_hash: canon::hash_of(&delta),
                events_hash: canon::hash_of(&events),
            },
            delta,
            events,
            revoke_sessions: Vec::new(),
            idempotency: None,
        };
        match store.commit(&commit) {
            // Another writer created it first: the caller loads it again.
            Ok(_) | Err(StoreError::Conflict { .. }) => Ok(()),
            Err(error) => Err(storage(error)),
        }
    }

    fn alloc(&mut self) -> RecordId {
        let id = RecordId(self.next_record);
        self.next_record += 1;
        id
    }

    /// The records answering `need`, read at the revision this transaction began at.
    fn read(&mut self, need: Need) -> Result<Vec<Arc<Record>>, StorageError> {
        let state = self.store.load(self.tenant, &[need]).map_err(storage)?;
        let Some(found) = state.tenant else {
            return Err(StorageError::Conflict("the tenant disappeared".into()));
        };
        if found.revision != self.header.revision {
            return Err(StorageError::Conflict(format!(
                "tenant revision moved from {:?} to {:?} during the transaction",
                self.header.revision, found.revision
            )));
        }
        Ok(found
            .records
            .values()
            .flat_map(|by_id| by_id.values().cloned())
            .collect())
    }

    /// The current record of `object` whose string field `name` is `value`, as this transaction
    /// sees it.
    fn find(
        &mut self,
        object: &ObjectTypeId,
        name: &str,
        value: &str,
    ) -> Result<Option<Arc<Record>>, StorageError> {
        let wanted = Value::Str(value.to_string());
        let key = field(name);
        let mut deleted = false;
        for ((kind, _), slot) in &self.pending {
            if kind != object {
                continue;
            }
            if let Some(after) = slot.after.as_ref().filter(|r| r.get(&key) == &wanted) {
                return Ok(Some(after.clone()));
            }
            if slot.after.is_none() && slot.before.as_ref().is_some_and(|r| r.get(&key) == &wanted)
            {
                deleted = true;
            }
        }
        if deleted {
            // The stored record was removed in this transaction and nothing replaced it.
            return Ok(None);
        }
        Ok(self
            .read(Need::ByField {
                object: object.clone(),
                field: key,
                value: wanted,
            })?
            .into_iter()
            .next())
    }

    /// Every current record of `object` as this transaction sees it, by id.
    fn all(&mut self, object: &ObjectTypeId) -> Result<Vec<Arc<Record>>, StorageError> {
        let mut by_id: BTreeMap<RecordId, Arc<Record>> = self
            .read(Need::AllOf(object.clone()))?
            .into_iter()
            .map(|r| (r.id, r))
            .collect();
        for ((kind, id), slot) in &self.pending {
            if kind != object {
                continue;
            }
            match &slot.after {
                Some(after) => {
                    by_id.insert(*id, after.clone());
                }
                None => {
                    by_id.remove(id);
                }
            }
        }
        Ok(by_id.into_values().collect())
    }

    fn get(
        &mut self,
        object: &ObjectTypeId,
        id: RecordId,
    ) -> Result<Option<Arc<Record>>, StorageError> {
        if let Some(slot) = self.pending.get(&(object.clone(), id)) {
            return Ok(slot.after.clone());
        }
        Ok(self
            .read(Need::Record {
                object: object.clone(),
                id,
            })?
            .into_iter()
            .next())
    }

    fn put(&mut self, before: Option<Arc<Record>>, after: Arc<Record>) {
        let slot = (after.object.clone(), after.id);
        match self.pending.get_mut(&slot) {
            Some(existing) => existing.after = Some(after),
            None => {
                self.pending.insert(
                    slot,
                    Slot {
                        before,
                        after: Some(after),
                    },
                );
            }
        }
    }

    fn remove(&mut self, record: &Arc<Record>) {
        let slot = (record.object.clone(), record.id);
        match self.pending.get_mut(&slot) {
            Some(existing) => existing.after = None,
            None => {
                self.pending.insert(
                    slot,
                    Slot {
                        before: Some(record.clone()),
                        after: None,
                    },
                );
            }
        }
    }

    fn record(
        &self,
        object: ObjectTypeId,
        id: RecordId,
        version: u64,
        created_at: Timestamp,
        fields: BTreeMap<FieldId, Value>,
    ) -> Arc<Record> {
        Arc::new(Record {
            id,
            object,
            version,
            created_by: None,
            created_at,
            updated_at: Timestamp(self.now.0),
            fields,
        })
    }

    /// Remove an outbox entry. Returns whether it was there.
    fn remove_outbox(&mut self, id: u64) -> Result<bool, StorageError> {
        let Some(found) = self.get(&object_type(OUTBOX)?, RecordId(id))? else {
            return Ok(false);
        };
        self.remove(&found);
        Ok(true)
    }

    /// Record a failed dispatch on an outbox entry. An entry that is gone is left alone.
    fn defer_outbox(
        &mut self,
        id: u64,
        attempts: u32,
        next_attempt_at: Time,
        error: &str,
    ) -> Result<(), StorageError> {
        let Some(found) = self.get(&object_type(OUTBOX)?, RecordId(id))? else {
            return Ok(());
        };
        let mut fields = found.fields.clone();
        fields.insert(field("attempts"), Value::Int(i64::from(attempts)));
        fields.insert(field("next_attempt_at"), Value::Int(next_attempt_at.0));
        fields.insert(field("last_error"), Value::Str(error.to_string()));
        let after = self.record(
            found.object.clone(),
            found.id,
            found.version + 1,
            found.created_at,
            fields,
        );
        self.put(Some(found), after);
        Ok(())
    }

    /// Commit the buffered writes as one store commit. Nothing buffered commits nothing.
    fn finish(self) -> Result<(), StorageError> {
        let internal = INTERNAL
            .iter()
            .map(|name| object_type(name))
            .collect::<Result<Vec<_>, _>>()?;
        let revision = StateRevision(self.header.revision.0 + 1);
        let mut ops = Vec::new();
        let mut events: Vec<DomainEvent> = Vec::new();
        // The journal holds one intent: the first change to a machine record, else the first
        // change of any kind.
        let mut record_intent: Option<Intent> = None;
        let mut first_intent: Option<Intent> = None;
        for slot in self.pending.values() {
            let (body, intent, object) = match (&slot.before, &slot.after) {
                (None, None) => continue,
                (None, Some(after)) => {
                    ops.push(DeltaOp::PutRecord(after.clone()));
                    (
                        EventBody::RecordCreated {
                            record: after.clone(),
                        },
                        Intent::CreateRecord {
                            object: after.object.clone(),
                            fields: after.fields.clone(),
                        },
                        after.object.clone(),
                    )
                }
                (Some(before), Some(after)) => {
                    ops.push(DeltaOp::PutRecord(after.clone()));
                    (
                        EventBody::RecordUpdated {
                            before: before.clone(),
                            after: after.clone(),
                        },
                        Intent::UpdateRecord {
                            object: after.object.clone(),
                            id: after.id,
                            fields: after.fields.clone(),
                            expected_version: Some(before.version),
                        },
                        after.object.clone(),
                    )
                }
                (Some(before), None) => {
                    ops.push(DeltaOp::DeleteRecord {
                        object: before.object.clone(),
                        id: before.id,
                    });
                    (
                        EventBody::RecordDeleted {
                            before: before.clone(),
                        },
                        Intent::DeleteRecord {
                            object: before.object.clone(),
                            id: before.id,
                            expected_version: Some(before.version),
                        },
                        before.object.clone(),
                    )
                }
            };
            first_intent.get_or_insert(intent.clone());
            if internal.contains(&object) {
                continue;
            }
            record_intent.get_or_insert(intent);
            events.push(DomainEvent {
                tenant: self.tenant.clone(),
                seq: self.header.event_seq + events.len() as u64 + 1,
                revision,
                at: Timestamp(self.now.0),
                principal: Principal::System,
                correlation: "runtime".into(),
                body,
            });
        }
        let Some(intent) = record_intent.or(first_intent) else {
            return Ok(());
        };
        let delta = StateDelta {
            tenant: self.tenant.clone(),
            base_revision: Some(self.header.revision),
            new_revision: revision,
            next_record: self.next_record,
            event_seq: self.header.event_seq + events.len() as u64,
            ops,
        };
        let commit = Commit {
            journal: JournalEntry {
                tenant: self.tenant.clone(),
                base_revision: delta.base_revision,
                revision,
                kernel_version: KERNEL_VERSION.to_string(),
                intent,
                context: context(self.tenant, self.now),
                delta_hash: canon::hash_of(&delta),
                events_hash: canon::hash_of(&events),
            },
            delta,
            events,
            revoke_sessions: Vec::new(),
            idempotency: None,
        };
        self.store.commit(&commit).map_err(storage)?;
        Ok(())
    }
}

impl Tx for ApiTx<'_> {
    fn load(&mut self, collection: &str, key: &str) -> Result<Option<StoredRecord>, StorageError> {
        let object = collection_object(collection)?;
        let Some(found) = self.find(&object, "key", key)? else {
            return Ok(None);
        };
        Ok(Some(StoredRecord {
            data: text(&found, "data")?,
            version: found.version,
        }))
    }

    fn save(
        &mut self,
        collection: &str,
        key: &str,
        data: &str,
        expected: Option<u64>,
    ) -> Result<u64, StorageError> {
        let object = collection_object(collection)?;
        let existing = self.find(&object, "key", key)?;
        let found = existing.as_ref().map(|r| r.version);
        if found != expected {
            return Err(StorageError::Conflict(format!(
                "{collection} {key}: expected version {expected:?}, found {found:?}"
            )));
        }
        let version = found.unwrap_or(0) + 1;
        let id = match &existing {
            Some(record) => record.id,
            None => self.alloc(),
        };
        let created_at = existing
            .as_ref()
            .map_or(Timestamp(self.now.0), |r| r.created_at);
        let after = self.record(
            object,
            id,
            version,
            created_at,
            strings(&[("key", key), ("data", data)]),
        );
        self.put(existing, after);
        Ok(version)
    }

    fn enqueue(&mut self, message: &NewMessage) -> Result<u64, StorageError> {
        let id = self.alloc();
        let mut fields = strings(&[
            ("source", &message.source),
            ("key", &message.key),
            ("receiver", &message.receiver),
            ("event", &message.event),
            ("delivery", message.delivery.name()),
        ]);
        fields.insert(field("created_at"), Value::Int(message.created_at.0));
        fields.insert(field("not_before"), Value::Int(message.not_before.0));
        fields.insert(
            field("next_attempt_at"),
            Value::Int(message.next_attempt_at.0),
        );
        fields.insert(field("attempts"), Value::Int(0));
        let record = self.record(object_type(OUTBOX)?, id, 1, Timestamp(self.now.0), fields);
        self.put(None, record);
        Ok(id.0)
    }

    fn set_timer(&mut self, timer: &NewTimer) -> Result<u64, StorageError> {
        let slot = timer_slot(&timer.machine, &timer.key, &timer.name);
        let object = object_type(TIMERS)?;
        if let Some(old) = self.find(&object, "slot", &slot)? {
            self.remove(&old);
        }
        let id = self.alloc();
        let mut fields = strings(&[
            ("slot", &slot),
            ("machine", &timer.machine),
            ("key", &timer.key),
            ("name", &timer.name),
            ("event", &timer.event),
        ]);
        fields.insert(field("fire_at"), Value::Int(timer.fire_at.0));
        let record = self.record(object, id, 1, Timestamp(self.now.0), fields);
        self.put(None, record);
        Ok(id.0)
    }

    fn cancel_timer(&mut self, machine: &str, key: &str, name: &str) -> Result<bool, StorageError> {
        let slot = timer_slot(machine, key, name);
        match self.find(&object_type(TIMERS)?, "slot", &slot)? {
            Some(found) => {
                self.remove(&found);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn take_timer(&mut self, id: u64) -> Result<bool, StorageError> {
        match self.get(&object_type(TIMERS)?, RecordId(id))? {
            Some(found) => {
                self.remove(&found);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn keys(&mut self, collection: &str) -> Result<Vec<String>, StorageError> {
        let object = collection_object(collection)?;
        self.all(&object)?.iter().map(|r| text(r, "key")).collect()
    }

    fn push(&mut self, entry: &NewEntry) -> Result<u64, StorageError> {
        let id = self.alloc();
        let fields = queue_fields(
            entry.due,
            &entry.event,
            entry.target.as_ref(),
            entry.timer.as_deref(),
            &entry.source,
            &entry.source_key,
        );
        let record = self.record(object_type(QUEUE)?, id, 1, Timestamp(self.now.0), fields);
        self.put(None, record);
        Ok(id.0)
    }

    fn take_entry(&mut self, seq: u64) -> Result<bool, StorageError> {
        match self.get(&object_type(QUEUE)?, RecordId(seq))? {
            Some(found) => {
                self.remove(&found);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn drop_timer_entry(
        &mut self,
        machine: &str,
        key: &str,
        name: &str,
    ) -> Result<bool, StorageError> {
        let slot = timer_slot(machine, key, name);
        match self.find(&object_type(QUEUE)?, "slot", &slot)? {
            Some(found) => {
                self.remove(&found);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn dead_letter(
        &mut self,
        entry: &QueueEntry,
        error: &str,
        at: Time,
    ) -> Result<(), StorageError> {
        let id = self.alloc();
        let mut fields = queue_fields(
            entry.due,
            &entry.event,
            entry.target.as_ref(),
            entry.timer.as_deref(),
            &entry.source,
            &entry.source_key,
        );
        fields.remove(&field("slot"));
        fields.insert(field("seq"), Value::Int(entry.seq as i64));
        fields.insert(field("error"), Value::Str(error.to_string()));
        fields.insert(field("at"), Value::Int(at.0));
        let record = self.record(
            object_type(DEAD_LETTERS)?,
            id,
            1,
            Timestamp(self.now.0),
            fields,
        );
        self.put(None, record);
        Ok(())
    }
}

/// The runtime's machine store on a [`Store`].
pub struct StorageApiMachineStore {
    store: Arc<dyn Store>,
    tenant: TenantId,
    /// The same tenant under the runtime's own name type.
    owner: strata_runtime::TenantId,
    clock: Arc<dyn Clock>,
    gate: Mutex<()>,
}

impl StorageApiMachineStore {
    /// A machine store keeping its records, outbox and timers in `tenant` of `store`. The tenant
    /// is created on first use. `clock` stamps what the store records (creation and update
    /// times, event times); the host's own clock still decides what is due.
    pub fn new(store: Arc<dyn Store>, tenant: TenantId, clock: Arc<dyn Clock>) -> Self {
        // The kernel's tenant id and the runtime's tenant name have one rule, so a kernel id is
        // always a runtime name.
        let owner = strata_runtime::TenantId::new(tenant.as_str())
            .expect("a kernel tenant id is a runtime tenant name");
        StorageApiMachineStore {
            store,
            tenant,
            owner,
            clock,
            gate: Mutex::new(()),
        }
    }

    /// The store underneath.
    pub fn store(&self) -> &Arc<dyn Store> {
        &self.store
    }

    /// The tenant the machine data lives in.
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    fn write<T, E>(&self, work: impl FnOnce(&mut ApiTx<'_>) -> Result<T, E>) -> Result<T, E>
    where
        E: From<StorageError>,
    {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| StorageError::Unavailable("a writer panicked".into()))?;
        let mut tx = ApiTx::begin(&*self.store, &self.tenant, self.clock.now())?;
        let result = work(&mut tx)?;
        tx.finish()?;
        Ok(result)
    }

    fn records(&self, need: Need) -> Result<Vec<Arc<Record>>, StorageError> {
        let state = self.store.load(&self.tenant, &[need]).map_err(storage)?;
        Ok(state
            .tenant
            .map(|found| {
                found
                    .records
                    .values()
                    .flat_map(|by_id| by_id.values().cloned())
                    .collect()
            })
            .unwrap_or_default())
    }
}

impl std::fmt::Debug for StorageApiMachineStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageApiMachineStore")
            .field("tenant", &self.tenant)
            .finish()
    }
}

impl RecordStore for StorageApiMachineStore {
    fn tenant(&self) -> &strata_runtime::TenantId {
        &self.owner
    }

    /// Every commit is guarded by the tenant's revision and every read in a transaction must see
    /// that revision, so each transaction is serializable.
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
        self.write(|tx| work(tx))
    }
}

impl OutboxStore for StorageApiMachineStore {
    fn due_messages(
        &self,
        source: &str,
        now: Time,
        limit: usize,
    ) -> Result<Vec<OutboxEntry>, StorageError> {
        let mut due = Vec::new();
        for record in self.records(Need::ByField {
            object: object_type(OUTBOX)?,
            field: field("source"),
            value: Value::Str(source.to_string()),
        })? {
            let entry = outbox_entry(&record)?;
            if entry.next_attempt_at <= now && entry.not_before <= now {
                due.push(entry);
            }
        }
        due.sort_by_key(|entry| (entry.not_before, entry.id));
        due.truncate(limit);
        Ok(due)
    }

    fn mark_delivered(&self, id: u64) -> Result<bool, StorageError> {
        self.write(|tx| tx.remove_outbox(id))
    }

    fn defer(
        &self,
        id: u64,
        attempts: u32,
        next_attempt_at: Time,
        error: &str,
    ) -> Result<(), StorageError> {
        self.write(|tx| tx.defer_outbox(id, attempts, next_attempt_at, error))
    }

    fn pending(&self) -> Result<u64, StorageError> {
        Ok(self.records(Need::AllOf(object_type(OUTBOX)?))?.len() as u64)
    }
}

impl QueueStore for StorageApiMachineStore {
    fn head(&self) -> Result<Option<QueueEntry>, StorageError> {
        Ok(self.queued(1)?.into_iter().next())
    }

    fn queued(&self, limit: usize) -> Result<Vec<QueueEntry>, StorageError> {
        let mut entries = self
            .records(Need::AllOf(object_type(QUEUE)?))?
            .iter()
            .map(|r| queue_entry(r))
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|e| (e.due, e.seq));
        entries.truncate(limit);
        Ok(entries)
    }

    fn dead_letters(&self) -> Result<Vec<DeadLetter>, StorageError> {
        let mut records = self.records(Need::AllOf(object_type(DEAD_LETTERS)?))?;
        records.sort_by_key(|r| r.id);
        records
            .iter()
            .map(|r| {
                let mut entry = queue_entry(r)?;
                entry.seq = u64::try_from(int(r, "seq")?)
                    .map_err(|_| StorageError::Corrupt("dead letter seq out of range".into()))?;
                Ok(DeadLetter {
                    entry,
                    error: text(r, "error")?,
                    at: Time(int(r, "at")?),
                })
            })
            .collect()
    }
}

impl TimerStore for StorageApiMachineStore {
    fn due_timers(
        &self,
        machine: &str,
        now: Time,
        limit: usize,
    ) -> Result<Vec<TimerEntry>, StorageError> {
        let mut due = Vec::new();
        for record in self.records(Need::ByField {
            object: object_type(TIMERS)?,
            field: field("machine"),
            value: Value::Str(machine.to_string()),
        })? {
            let entry = timer_entry(&record)?;
            if entry.fire_at <= now {
                due.push(entry);
            }
        }
        due.sort_by_key(|t| (t.fire_at, t.id));
        due.truncate(limit);
        Ok(due)
    }
}
