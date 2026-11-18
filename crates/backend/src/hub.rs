//! In-process fan-out of committed events, and the outbox publisher that feeds it.
//!
//! Nothing reaches the hub except through [`publisher`], which reads the store's outbox. An
//! event is therefore only ever published after its transaction has committed, and an event
//! committed just before a crash is still in the outbox when the process starts again.
//! Delivery is at-least-once; subscribers skip sequence numbers they have already seen.

use crate::app::{Shared, LOG};
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use strata_kernel::{DomainEvent, EventBody, PrincipalId, SessionId, TenantId, TenantState};
use strata_protocol::ws::SessionState;
use tokio::sync::{broadcast, Notify};

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

pub struct Hub {
    pub events: broadcast::Sender<Arc<Published>>,
    pub runs: broadcast::Sender<RunMessage>,
    pub sessions: broadcast::Sender<(SessionId, SessionState)>,
    /// Wakes the publisher after a commit.
    pub kick: Notify,
    pub connections: Mutex<BTreeMap<String, Connection>>,
}

impl Hub {
    pub fn new() -> Self {
        Hub {
            events: broadcast::channel(1024).0,
            runs: broadcast::channel(256).0,
            sessions: broadcast::channel(64).0,
            kick: Notify::new(),
            connections: Mutex::new(BTreeMap::new()),
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

/// Move one batch of committed events from the outbox to the hub. Returns how many moved.
pub fn publish_pending(app: &crate::app::App) -> Result<usize, strata_storage_api::StoreError> {
    let pending = app.store.outbox(256)?;
    let Some(last) = pending.last().map(|entry| entry.position) else {
        return Ok(0);
    };
    let mut headers: BTreeMap<TenantId, Arc<TenantState>> = BTreeMap::new();
    for entry in &pending {
        let tenant = entry.event.tenant.clone();
        let header = match headers.get(&tenant) {
            Some(header) => header.clone(),
            None => {
                let Some(header) = app.store.load(&tenant, &[])?.tenant else {
                    continue;
                };
                headers.insert(tenant, header.clone());
                header
            }
        };
        let summary = describe(&entry.event);
        LOG.info("event.published", &entry.event.correlation, summary.clone());
        app.stats.recent_events.push(summary);
        // No receivers is fine: the event stays in the durable log for later replay.
        let _ = app.hub.events.send(Arc::new(Published {
            event: entry.event.clone(),
            tenant: header,
        }));
    }
    app.store.mark_published(last)?;
    Ok(pending.len())
}

/// Publish committed events for as long as the process lives.
pub async fn publisher(app: Shared) {
    loop {
        let drained = app.blocking(|app| {
            let mut total = 0;
            loop {
                match publish_pending(app) {
                    Ok(0) => return Ok(total),
                    Ok(n) => total += n,
                    Err(e) => return Err(e),
                }
            }
        });
        if let Ok(Err(error)) = drained.await {
            LOG.error("outbox.failed", "", json!({ "error": error.to_string() }));
        }
        // A commit wakes us at once; the timeout covers commits made by another process.
        let _ = tokio::time::timeout(Duration::from_millis(500), app.hub.kick.notified()).await;
    }
}
