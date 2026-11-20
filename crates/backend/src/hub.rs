//! In-process fan-out of committed events, and the publication that feeds it.
//!
//! Nothing reaches the hub except through [`publish_pending`], which reads the store's outbox
//! when the generated outbox machine (see [`crate::outbox`]) says to. An event is therefore only
//! ever published after its transaction has committed, and an event committed just before a
//! crash is still in the outbox when the process starts again. Delivery is at-least-once;
//! subscribers skip sequence numbers they have already seen.

use crate::outbox::Outbox;
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use strata_kernel::ext::*;
use strata_kernel::{
    DomainEvent, EventBody, PrincipalId, Record, SessionId, TenantId, TenantState, Value,
};
use strata_model::telemetry;
use strata_protocol::ws::SessionState;
use tokio::sync::broadcast;

/// A committed event together with the tenant's schema and members at publication time, which
/// is what deciding visibility needs.
pub struct Published {
    pub event: DomainEvent,
    pub tenant: Arc<TenantState>,
}

/// A message about a simulation or verification run, addressed to one principal.
#[derive(Clone, Debug)]
pub struct RunMessage {
    pub tenant: TenantId,
    pub principal: PrincipalId,
    pub run: String,
    pub kind: String,
    pub payload: Json,
}

/// What diagnostics shows about one WebSocket connection.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Connection {
    pub tenant: String,
    pub principal: String,
    pub session: Option<String>,
    pub opened_at: i64,
    pub subscriptions: Vec<String>,
    pub delivered: u64,
    pub acknowledged: u64,
}

/// A broadcast channel for each tenant. A subscriber hears only its own tenant's messages, so a
/// burst in one tenant overflows no other tenant's subscribers.
pub struct Fanout<T: Clone> {
    channels: Mutex<BTreeMap<TenantId, broadcast::Sender<T>>>,
    capacity: usize,
}

impl<T: Clone> Fanout<T> {
    pub fn new(capacity: usize) -> Self {
        Fanout {
            channels: Mutex::new(BTreeMap::new()),
            capacity,
        }
    }

    /// Listen to `tenant`'s messages.
    pub fn subscribe(&self, tenant: &TenantId) -> broadcast::Receiver<T> {
        let mut channels = self.channels.lock().unwrap_or_else(|e| e.into_inner());
        channels
            .entry(tenant.clone())
            .or_insert_with(|| broadcast::channel(self.capacity).0)
            .subscribe()
    }

    /// Send to `tenant`'s subscribers. With none, nothing is kept and the channel is dropped.
    pub fn send(&self, tenant: &TenantId, message: T) {
        let mut channels = self.channels.lock().unwrap_or_else(|e| e.into_inner());
        let Some(channel) = channels.get(tenant) else {
            return;
        };
        if channel.receiver_count() == 0 {
            channels.remove(tenant);
            return;
        }
        let _ = channel.send(message);
    }
}

pub struct Hub {
    pub events: Fanout<Arc<Published>>,
    pub runs: broadcast::Sender<RunMessage>,
    pub sessions: broadcast::Sender<(SessionId, SessionState)>,
    /// The outbox publisher, set once the backend has started.
    pub outbox: OnceLock<Outbox>,
    pub connections: Mutex<BTreeMap<String, Connection>>,
}

impl Hub {
    pub fn new() -> Self {
        Hub {
            events: Fanout::new(1024),
            runs: broadcast::channel(256).0,
            sessions: broadcast::channel(64).0,
            outbox: OnceLock::new(),
            connections: Mutex::new(BTreeMap::new()),
        }
    }
}

impl Hub {
    /// A commit was made: tell the outbox publisher.
    pub fn committed(&self) {
        if let Some(outbox) = self.outbox.get() {
            outbox.committed();
        }
    }
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

fn describe(event: &DomainEvent) -> Json {
    let (kind, object, id) = match &event.body {
        EventBody::TenantCreated { .. } => ("tenant_created", None, None),
        EventBody::SchemaChanged { .. } => ("schema_changed", None, None),
        EventBody::MemberChanged { .. } => ("member_changed", None, None),
        EventBody::RecordCreated { record } => (
            "record_created",
            Some(record.object.as_str()),
            Some(record.id.0),
        ),
        EventBody::RecordUpdated { after, .. } => (
            "record_updated",
            Some(after.object.as_str()),
            Some(after.id.0),
        ),
        EventBody::RecordDeleted { before } => (
            "record_deleted",
            Some(before.object.as_str()),
            Some(before.id.0),
        ),
    };
    json!({
        "tenant": event.tenant.as_str(),
        "seq": event.seq,
        "revision": event.revision.0,
        "at": event.at.0,
        "kind": kind,
        "object": object,
        "id": id,
        "correlation": event.correlation,
    })
}

/// The workflow transition an update took, when it moved a workflow field of the record's object
/// type from one state to another along a declared transition (`publish`, `unpublish`).
fn workflow_transition<'a>(
    tenant: &'a TenantState,
    before: &Record,
    after: &'a Record,
) -> Option<&'a str> {
    tenant
        .schema
        .workflows_of(&after.object)
        .find_map(|workflow| {
            let (Value::Str(from), Value::Str(to)) =
                (before.get(&workflow.field), after.get(&workflow.field))
            else {
                return None;
            };
            if from == to {
                return None;
            }
            workflow
                .transitions
                .iter()
                .find(|t| t.from == *from && t.to == *to)
                .map(|t| t.name.as_str())
        })
}

/// Write the log line of a published domain event through the generated telemetry functions
/// (`strata_model::telemetry`): the tenant and the allowlisted fields the specification states,
/// never record contents.
fn log_published(event: &DomainEvent, tenant: &TenantState) {
    let sink = &telemetry::BACKEND;
    let correlation = event.correlation.as_str();
    let (name, seq, revision) = (event.tenant.as_str(), event.seq, event.revision.0);
    match &event.body {
        EventBody::TenantCreated { name: title } => telemetry::tenant_created(
            sink,
            &telemetry::TenantCreated {
                correlation,
                tenant: name,
                seq,
                revision,
                name: title,
            },
        ),
        EventBody::SchemaChanged {
            version,
            summary,
            grandfathered,
        } => telemetry::schema_changed(
            sink,
            &telemetry::SchemaChanged {
                correlation,
                tenant: name,
                seq,
                revision,
                version: version.0,
                changes: summary.len() as u64,
                grandfathered: *grandfathered,
            },
        ),
        EventBody::MemberChanged { principal, .. } => telemetry::member_changed(
            sink,
            &telemetry::MemberChanged {
                correlation,
                tenant: name,
                seq,
                revision,
                principal: principal.as_str(),
            },
        ),
        EventBody::RecordCreated { record } => telemetry::record_created(
            sink,
            &telemetry::RecordCreated {
                correlation,
                tenant: name,
                seq,
                revision,
                object: record.object.as_str(),
                record: record.id.0,
                version: record.version,
            },
        ),
        EventBody::RecordUpdated { before, after } => telemetry::record_updated(
            sink,
            &telemetry::RecordUpdated {
                correlation,
                tenant: name,
                seq,
                revision,
                object: after.object.as_str(),
                record: after.id.0,
                version: after.version,
                transition: workflow_transition(tenant, before, after),
            },
        ),
        EventBody::RecordDeleted { before } => telemetry::record_deleted(
            sink,
            &telemetry::RecordDeleted {
                correlation,
                tenant: name,
                seq,
                revision,
                object: before.object.as_str(),
                record: before.id.0,
            },
        ),
    }
}

/// Move one batch of committed events from the outbox to the hub. Returns how many were sent.
///
/// Outbox positions are per tenant (each tenant may have a database of its own), so this is the
/// fan-in: the batch holds the waiting events of every tenant, each tenant's in its commit order,
/// and each tenant's outbox is marked published up to the last of its events that was sent. A
/// tenant whose state cannot be loaded is left alone, with its events still waiting; it does not
/// stop the others.
pub fn publish_pending(app: &crate::app::App) -> Result<usize, strata_storage_api::StoreError> {
    let pending = app.store.outbox(256)?;
    if pending.is_empty() {
        return Ok(0);
    }
    let mut last: BTreeMap<TenantId, u64> = BTreeMap::new();
    // A tenant maps to its header, or to `None` when it could not be loaded this sweep.
    let mut headers: BTreeMap<TenantId, Option<Arc<TenantState>>> = BTreeMap::new();
    let mut sent = 0;
    for entry in &pending {
        let tenant = entry.event.tenant.clone();
        let header = match headers.get(&tenant) {
            Some(header) => header.clone(),
            None => {
                let header = match app.store.load(&tenant, &[]) {
                    Ok(state) => state.tenant,
                    Err(error) => {
                        crate::app::LOG.warn(
                            "hub.publish_skipped",
                            "",
                            json!({ "tenant": tenant.as_str(), "error": error.to_string() }),
                        );
                        None
                    }
                };
                headers.insert(tenant.clone(), header.clone());
                header
            }
        };
        let Some(header) = header else {
            continue;
        };
        last.insert(tenant, entry.position);
        log_published(&entry.event, &header);
        app.stats.recent_events.push(describe(&entry.event));
        // No receivers is fine: the event stays in the durable log for later replay.
        app.hub.events.send(
            &entry.event.tenant,
            Arc::new(Published {
                event: entry.event.clone(),
                tenant: header,
            }),
        );
        sent += 1;
    }
    for (tenant, position) in &last {
        if let Err(error) = app.store.mark_published(tenant, *position) {
            crate::app::LOG.warn(
                "hub.mark_published_failed",
                "",
                json!({ "tenant": tenant.as_str(), "error": error.to_string() }),
            );
        }
    }
    Ok(sent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast::error::TryRecvError;

    #[test]
    fn a_burst_in_one_tenant_lags_nobody_in_another() {
        let fanout: Fanout<u32> = Fanout::new(1024);
        let (loud, quiet) = (
            TenantId::new("fieldnotes").unwrap(),
            TenantId::new("tidewater").unwrap(),
        );
        let mut loud_rx = fanout.subscribe(&loud);
        let mut quiet_rx = fanout.subscribe(&quiet);
        for n in 0..2000 {
            fanout.send(&loud, n);
        }
        fanout.send(&quiet, 7);
        assert_eq!(quiet_rx.try_recv().unwrap(), 7);
        assert!(matches!(quiet_rx.try_recv(), Err(TryRecvError::Empty)));
        // The loud tenant's own slow subscriber is the one that lags.
        assert!(matches!(loud_rx.try_recv(), Err(TryRecvError::Lagged(_))));
    }

    #[test]
    fn a_tenant_without_subscribers_keeps_no_channel() {
        let fanout: Fanout<u32> = Fanout::new(8);
        let tenant = TenantId::new("fieldnotes").unwrap();
        drop(fanout.subscribe(&tenant));
        fanout.send(&tenant, 1);
        assert!(fanout.channels.lock().unwrap().is_empty());
    }
}
