//! One WebSocket connection: subscriptions, replay, heartbeats, back-pressure.
//!
//! The connection is bound at upgrade to the identity the session resolved to. Every event it
//! delivers has been through `strata_substrate::subscribe::notice`, which asks the kernel
//! whether this principal may read the record. Outbound messages go through a bounded queue; a
//! client that cannot keep up is disconnected and resumes from the durable event log.

use crate::app::{App, Shared, LOG};
use crate::auth::{session_state, Identity};
use crate::hub::{Connection, Published};
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use strata_kernel::rules::may_attempt;
use strata_kernel::{Action, ObjectTypeId, Principal, Resource};
use strata_protocol::log::now_ms;
use strata_protocol::ws::{
    parse_client, ClientMessage, ServerMessage, SessionState, HEARTBEAT_MS, MAX_REPLAY,
    MAX_UNACKED, VERSION,
};
use strata_substrate::subscribe::{notice, Selector};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

const OUTBOUND_QUEUE: usize = 256;
const DIAG_PER_CONNECTION: u32 = 200;

struct Subscription {
    selector: Selector,
    /// Events up to this sequence have been handled for this subscription.
    cursor: u64,
}

struct Conn {
    app: Shared,
    id: String,
    identity: Identity,
    out: mpsc::Sender<ServerMessage>,
    subs: BTreeMap<String, Subscription>,
    delivered: u64,
    acknowledged: u64,
    diag_budget: u32,
}

/// The queue is full or the writer has gone: the connection is over.
struct Closed;

impl Conn {
    fn send(&self, message: ServerMessage) -> Result<(), Closed> {
        self.out.try_send(message).map_err(|_| Closed)
    }

    fn principal_name(&self) -> String {
        match &self.identity.principal {
            Principal::User(id) => id.as_str().to_string(),
            other => format!("{other:?}").to_lowercase(),
        }
    }

    fn register(&self) {
        if let Ok(mut connections) = self.app.hub.connections.lock() {
            let opened_at = connections
                .get(&self.id)
                .map_or_else(now_ms, |c| c.opened_at);
            connections.insert(
                self.id.clone(),
                Connection {
                    tenant: self.identity.tenant.as_str().to_string(),
                    principal: self.principal_name(),
                    session: self
                        .identity
                        .session
                        .as_ref()
                        .map(|s| s.id.as_str().to_string()),
                    opened_at,
                    subscriptions: self.subs.keys().cloned().collect(),
                    delivered: self.delivered,
                    acknowledged: self.acknowledged,
                },
            );
        }
    }

    fn deliver(&mut self, published: &Published) -> Result<(), Closed> {
        if published.event.tenant != self.identity.tenant {
            return Ok(());
        }
        let seq = published.event.seq;
        let mut outgoing = Vec::new();
        for (name, sub) in self.subs.iter_mut() {
            if seq <= sub.cursor {
                continue;
            }
            sub.cursor = seq;
            if let Some(found) = notice(
                &published.tenant,
                &self.identity.principal,
                &sub.selector,
                &published.event,
            ) {
                outgoing.push((name.clone(), found));
            }
        }
        // The correlation id may have been chosen by the client that made the change. It goes
        // back to that principal's own connections and to nobody else.
        let own =
            !self.identity.anonymous() && published.event.principal == self.identity.principal;
        let correlation = if own {
            published.event.correlation.clone()
        } else {
            String::new()
        };
        for (sub, found) in outgoing {
            self.delivered += 1;
            self.send(ServerMessage::Event {
                sub,
                n: self.delivered,
                seq,
                correlation: correlation.clone(),
                notice: found,
            })?;
        }
        if self.delivered - self.acknowledged > MAX_UNACKED {
            self.send(ServerMessage::Resync {
                reason: "too many unacknowledged events".into(),
                head: seq,
            })?;
            return Err(Closed);
        }
        Ok(())
    }

    async fn head(&self) -> u64 {
        let tenant = self.identity.tenant.clone();
        self.app
            .blocking(move |app| {
                app.store
                    .load(&tenant, &[])
                    .ok()
                    .and_then(|s| s.tenant)
                    .map_or(0, |t| t.event_seq)
            })
            .await
            .unwrap_or(0)
    }

    async fn subscribe(
        &mut self,
        req: String,
        name: String,
        objects: Option<Vec<String>>,
        after: Option<u64>,
    ) -> Result<(), Closed> {
        let selector = match objects.map(|names| {
            names
                .into_iter()
                .map(ObjectTypeId::new)
                .collect::<Result<_, _>>()
        }) {
            None => Selector { objects: None },
            Some(Ok(set)) => Selector { objects: Some(set) },
            Some(Err(e)) => {
                return self.send(ServerMessage::Error {
                    req: Some(req),
                    code: "invalid".into(),
                    message: e.to_string(),
                })
            }
        };
        if self.subs.len() >= 16 && !self.subs.contains_key(&name) {
            return self.send(ServerMessage::Error {
                req: Some(req),
                code: "invalid".into(),
                message: "too many subscriptions".into(),
            });
        }
        let tenant = self.identity.tenant.clone();
        let from = after;
        let loaded = self
            .app
            .blocking(move |app: &App| {
                let header = app.store.load(&tenant, &[])?.tenant;
                let missed = match (from, &header) {
                    (Some(after), Some(_)) => {
                        app.store.events_after(&tenant, after, MAX_REPLAY + 1)?
                    }
                    _ => Vec::new(),
                };
                Ok::<_, strata_storage_api::StoreError>((header, missed))
            })
            .await;
        let (Ok(Ok((Some(header), missed))),) = (loaded,) else {
            return self.send(ServerMessage::Error {
                req: Some(req),
                code: "unavailable".into(),
                message: "the site's state could not be read".into(),
            });
        };
        self.send(ServerMessage::Ok { req })?;
        let head = header.event_seq;
        let cursor = after.unwrap_or(head).min(head);
        self.subs
            .insert(name.clone(), Subscription { selector, cursor });
        if missed.len() > MAX_REPLAY {
            if let Some(sub) = self.subs.get_mut(&name) {
                sub.cursor = head;
            }
            self.send(ServerMessage::Resync {
                reason: "too far behind to replay".into(),
                head,
            })?;
        } else {
            for event in missed {
                self.deliver(&Published {
                    event,
                    tenant: header.clone(),
                })?;
            }
        }
        self.register();
        Ok(())
    }

    async fn handle(&mut self, message: ClientMessage) -> Result<(), Closed> {
        match message {
            ClientMessage::Subscribe {
                req,
                sub,
                objects,
                after,
            } => self.subscribe(req, sub, objects, after).await,
            ClientMessage::Unsubscribe { req, sub } => {
                self.subs.remove(&sub);
                self.register();
                self.send(ServerMessage::Ok { req })
            }
            ClientMessage::Ack { n } => {
                self.acknowledged = self.acknowledged.max(n.min(self.delivered));
                Ok(())
            }
            ClientMessage::Ping { nonce } => {
                let head = self.head().await;
                self.send(ServerMessage::Pong { nonce, head })
            }
            ClientMessage::Cancel { req, run } => {
                let identity = self.identity.clone();
                let may_manage_schema = self
                    .app
                    .blocking(move |app: &App| {
                        app.store
                            .load(&identity.tenant, &[])
                            .ok()
                            .and_then(|state| state.tenant)
                            .is_some_and(|tenant| {
                                may_attempt(
                                    &tenant,
                                    &identity.principal,
                                    Action::Manage,
                                    &Resource::Schema,
                                )
                            })
                    })
                    .await
                    .unwrap_or(false);
                let cancelled = self.app.lab.cancel(&run, &self.identity, may_manage_schema);
                if cancelled {
                    self.send(ServerMessage::Ok { req })
                } else {
                    self.send(ServerMessage::Error {
                        req: Some(req),
                        code: "not_found".into(),
                        message: "no such run".into(),
                    })
                }
            }
            ClientMessage::Diag {
                level,
                event,
                correlation,
                detail,
            } => {
                if self.app.cfg.dev && self.diag_budget > 0 {
                    self.diag_budget -= 1;
                    let line = json!({ "conn": self.id, "browser_level": level, "detail": detail });
                    strata_protocol::log::Logger { service: "browser" }.info(
                        &event,
                        &correlation,
                        line,
                    );
                }
                Ok(())
            }
        }
    }
}

/// Serve one upgraded connection until it ends.
pub async fn serve(app: Shared, identity: Identity, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (out, mut outbound) = mpsc::channel::<ServerMessage>(OUTBOUND_QUEUE);
    let writer = tokio::spawn(async move {
        while let Some(message) = outbound.recv().await {
            let Ok(text) = serde_json::to_string(&message) else {
                continue;
            };
            if sink.send(Message::Text(text)).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // Subscribe to the hub before anything is read from the store, so that nothing committed in
    // between is missed; per-subscription cursors drop what replay already covered.
    let mut events = app.hub.events.subscribe();
    let mut runs = app.hub.runs.subscribe();
    let mut sessions = app.hub.sessions.subscribe();

    let id = format!("c_{}", crate::auth::random_hex(6).unwrap_or_default());
    let mut conn = Conn {
        app: app.clone(),
        id: id.clone(),
        identity,
        out,
        subs: BTreeMap::new(),
        delivered: 0,
        acknowledged: 0,
        diag_budget: DIAG_PER_CONNECTION,
    };
    let correlation = conn.identity.correlation.clone();
    LOG.info(
        "ws.opened",
        &correlation,
        json!({ "conn": id, "tenant": conn.identity.tenant.as_str(), "principal": conn.principal_name() }),
    );
    conn.register();
    let head = conn.head().await;
    let state = if conn.identity.session.is_some() {
        SessionState::Active
    } else {
        conn.identity.state
    };
    let mut alive = conn
        .send(ServerMessage::Welcome {
            v: VERSION,
            conn: id.clone(),
            head,
            session: state,
            heartbeat_ms: HEARTBEAT_MS,
        })
        .is_ok();

    let mut last_heard = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    let mut reason = "client closed";
    while alive {
        let step: Result<(), Closed> = tokio::select! {
            frame = stream.next() => match frame {
                Some(Ok(Message::Text(text))) => {
                    last_heard = Instant::now();
                    match parse_client(&text) {
                        Ok(message) => conn.handle(message).await,
                        Err(problem) => conn.send(ServerMessage::Error { req: None, code: "invalid".into(), message: problem }),
                    }
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => Ok(()),
                Some(Ok(Message::Binary(_))) => {
                    conn.send(ServerMessage::Error { req: None, code: "invalid".into(), message: "text frames only".into() })
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => Err(Closed),
            },
            published = events.recv() => match published {
                Ok(published) => conn.deliver(&published),
                Err(RecvError::Lagged(_)) => {
                    reason = "fell behind the event stream";
                    let head = conn.head().await;
                    let _ = conn.send(ServerMessage::Resync { reason: reason.into(), head });
                    Err(Closed)
                }
                Err(RecvError::Closed) => Err(Closed),
            },
            run = runs.recv() => match run {
                Ok(message) => {
                    let mine = message.tenant == conn.identity.tenant
                        && matches!(&conn.identity.principal, Principal::User(id) if *id == message.principal);
                    if mine {
                        conn.send(ServerMessage::Run { run: message.run, kind: message.kind, payload: message.payload })
                    } else {
                        Ok(())
                    }
                }
                Err(_) => Ok(()),
            },
            ended = sessions.recv() => match ended {
                Ok((session, state)) if conn.identity.session.as_ref().is_some_and(|s| s.id == session) => {
                    reason = "session ended";
                    let _ = conn.send(ServerMessage::Session { state });
                    Err(Closed)
                }
                _ => Ok(()),
            },
            _ = tick.tick() => {
                if last_heard.elapsed() > Duration::from_millis(HEARTBEAT_MS * 3) {
                    reason = "heartbeat missed";
                    Err(Closed)
                } else if let Some(session) = conn.identity.session.clone() {
                    let current = app
                        .blocking(move |app: &App| {
                            app.store.session(&session.id).ok().flatten().map(|s| session_state(app, &s, app.now().0))
                        })
                        .await
                        .ok()
                        .flatten()
                        .unwrap_or(SessionState::Revoked);
                    if current == SessionState::Active {
                        conn.register();
                        Ok(())
                    } else {
                        reason = "session ended";
                        let _ = conn.send(ServerMessage::Session { state: current });
                        Err(Closed)
                    }
                } else {
                    conn.register();
                    Ok(())
                }
            }
        };
        alive = step.is_ok();
    }

    if let Ok(mut connections) = app.hub.connections.lock() {
        connections.remove(&id);
    }
    LOG.info(
        "ws.closed",
        &correlation,
        json!({ "conn": id, "reason": reason, "delivered": conn.delivered }),
    );
    // Dropping the connection drops the sender; the writer flushes what is queued and closes.
    drop(conn);
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
}
