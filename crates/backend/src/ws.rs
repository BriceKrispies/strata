//! One WebSocket connection: subscriptions, replay, heartbeats, back-pressure.
//!
//! The connection is bound at upgrade to the identity the session resolved to. Every event it
//! delivers has been through `strata_substrate::subscribe::notice`, which asks the kernel
//! whether this principal may read the record. Outbound messages go through a bounded queue; a
//! client that cannot keep up is disconnected and resumes from the durable event log.

use crate::app::{App, Shared, LOG};
use crate::auth::{session_state, Identity};
use crate::hub::{Connection, Published, PublishedSegment};
use crate::limits::SocketPlaces;
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use strata_kernel::rules::may_attempt;
use strata_kernel::{Action, EventBody, Principal, Resource};
use strata_protocol::log::now_ms;
use strata_protocol::ws::{
    parse_client, ClientMessage, ServerMessage, SessionState, HEARTBEAT_MS, MAX_REPLAY,
    MAX_UNACKED, VERSION,
};
use strata_substrate::subscribe::{admit, notice_in, segment_notice, Selector, SelectorExt};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

const OUTBOUND_QUEUE: usize = 256;
/// Room that must be free in the outbound queue before replay hands over the next event: one
/// event is at most one message per subscription (16) and a resync.
const REPLAY_ROOM: usize = 20;
/// How long replay waits for a client that does not read before the connection is closed.
const REPLAY_PATIENCE: Duration = Duration::from_secs(10);
/// A ping is answered from the head last read, unless that read is older than this.
const HEAD_TTL: Duration = Duration::from_secs(2);
/// Messages one connection may send: a burst, and the rate it refills at.
const MESSAGE_BURST: f64 = 40.0;
const MESSAGES_PER_SECOND: f64 = 20.0;
/// Messages refused for rate before the connection is closed.
const MESSAGE_STRIKES: u32 = 100;
/// The store is asked whether the session still holds this often. A session that is revoked or
/// ends on request is announced on the hub and closes its sockets at once; a published change of
/// the person's roles makes the next tick ask at once (whoever committed it); this catches the
/// idle timeout, which no event announces. Expiry by time needs no store read: see the tick.
const SESSION_RECHECK: Duration = Duration::from_secs(30);

/// A token bucket: a connection may send `MESSAGE_BURST` messages at once and
/// `MESSAGES_PER_SECOND` on average.
struct Bucket {
    tokens: f64,
    at: Instant,
}

impl Bucket {
    fn new(now: Instant) -> Self {
        Bucket {
            tokens: MESSAGE_BURST,
            at: now,
        }
    }

    /// Take one token at `now`; false when none is left.
    fn take(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.at).as_secs_f64();
        self.at = now;
        self.tokens = (self.tokens + elapsed * MESSAGES_PER_SECOND).min(MESSAGE_BURST);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Wait until `out` has `room` free places, the client has gone, or `patience` has run out.
async fn wait_for_room<T>(
    out: &mpsc::Sender<T>,
    room: usize,
    patience: Duration,
) -> Result<(), Closed> {
    let wait = async {
        while out.capacity() < room {
            if out.is_closed() {
                return Err(Closed);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok(())
    };
    tokio::time::timeout(patience, wait)
        .await
        .map_err(|_| Closed)?
}

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
    /// The visitor's address, for the per-source log budget.
    source: Option<String>,
    bucket: Bucket,
    strikes: u32,
    /// The newest event sequence this connection has seen or read, and when it last read it.
    known_head: u64,
    head_at: Instant,
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
        self.known_head = self.known_head.max(seq);
        let mut outgoing = Vec::new();
        for (name, sub) in self.subs.iter_mut() {
            if seq <= sub.cursor {
                continue;
            }
            sub.cursor = seq;
            if let Some(found) = notice_in(
                &published.state,
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

    /// Tell the subscriptions that select segments of an entry or exit event of one, when the
    /// subscriber may see its members and the person (D-7280). A segment event is live only: it
    /// carries no sequence number of the kernel's, so no cursor and no replay apply.
    fn deliver_segment(&mut self, published: &PublishedSegment) -> Result<(), Closed> {
        if published.event.tenant != self.identity.tenant {
            return Ok(());
        }
        let Some(segment) = published
            .catalog
            .iter()
            .find(|segment| segment.id == published.event.event.segment)
        else {
            return Ok(());
        };
        let mut outgoing = Vec::new();
        for (name, sub) in &self.subs {
            if let Some(found) = segment_notice(
                &published.state,
                &self.identity.principal,
                &sub.selector,
                segment,
                published.position,
                &published.event,
            ) {
                outgoing.push((name.clone(), found));
            }
        }
        for (sub, found) in outgoing {
            self.delivered += 1;
            self.send(ServerMessage::Event {
                sub,
                n: self.delivered,
                seq: 0,
                correlation: String::new(),
                notice: found,
            })?;
        }
        if self.delivered - self.acknowledged > MAX_UNACKED {
            self.send(ServerMessage::Resync {
                reason: "too many unacknowledged events".into(),
                head: self.known_head,
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

    /// Read the head from the store and remember it.
    async fn refresh_head(&mut self) -> u64 {
        let head = self.head().await;
        self.known_head = self.known_head.max(head);
        self.head_at = Instant::now();
        head
    }

    /// The head for a ping: at most `HEAD_TTL` old, so a ping costs no store read of its own
    /// except once in that time.
    async fn cached_head(&mut self) -> u64 {
        if self.head_at.elapsed() >= HEAD_TTL {
            self.refresh_head().await;
        }
        self.known_head
    }

    async fn subscribe(
        &mut self,
        req: String,
        name: String,
        selector: Result<Selector, String>,
        after: Option<u64>,
    ) -> Result<(), Closed> {
        // A selector that names what nothing emits yet, or an invalid name, is refused here: it
        // would otherwise match nothing and say so to no one.
        let selector = match selector {
            Ok(selector) => selector,
            Err(message) => {
                return self.send(ServerMessage::Error {
                    req: Some(req),
                    code: "invalid".into(),
                    message,
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
                let header = Arc::new(app.store.load(&tenant, &[])?);
                let missed = match (from, &header.tenant) {
                    (Some(after), Some(_)) => app
                        .store
                        .events_after(&tenant, after, MAX_REPLAY + 1)?
                        .into_iter()
                        .map(|event| Published {
                            state: crate::hub::published_state(app.store.as_ref(), &header, &event),
                            event,
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                Ok::<_, strata_storage_api::StoreError>((header, missed))
            })
            .await;
        let (Ok(Ok((header, missed))),) = (loaded,) else {
            return self.send(ServerMessage::Error {
                req: Some(req),
                code: "unavailable".into(),
                message: "the site's state could not be read".into(),
            });
        };
        let Some(tenant_state) = header.tenant.clone() else {
            return self.send(ServerMessage::Error {
                req: Some(req),
                code: "unavailable".into(),
                message: "the site's state could not be read".into(),
            });
        };
        // A subscription to events the subscriber has no audience for is refused and audited
        // (D-8402, ADM-1632); it never reaches the delivery path.
        if let Err(refusal) = admit(&tenant_state, &self.identity.principal, &selector) {
            self.app.audit(
                "subscription.refused",
                &self.id,
                json!({
                    "tenant": self.identity.tenant.as_str(),
                    "principal": self.principal_name(),
                    "subscription": name,
                    "kind": format!("{:?}", refusal.kind),
                }),
            );
            return self.send(ServerMessage::Error {
                req: Some(req),
                code: "audience_not_permitted".into(),
                message: refusal.message,
            });
        }
        self.send(ServerMessage::Ok { req })?;
        let head = tenant_state.event_seq;
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
            for published in missed {
                // Replay never overflows the queue: it waits for the writer to make room.
                wait_for_room(&self.out, REPLAY_ROOM, REPLAY_PATIENCE).await?;
                self.deliver(&published)?;
            }
        }
        self.register();
        Ok(())
    }

    async fn handle(&mut self, message: ClientMessage) -> Result<(), Closed> {
        if !self.bucket.take(Instant::now()) {
            self.strikes += 1;
            if self.strikes >= MESSAGE_STRIKES {
                return Err(Closed);
            }
            return self.send(ServerMessage::Error {
                req: None,
                code: "throttled".into(),
                message: "too many messages; slow down".into(),
            });
        }
        match message {
            ClientMessage::Subscribe {
                req,
                sub,
                objects,
                kinds,
                segments,
                units,
                transitions,
                after,
            } => {
                let selector = Selector::from_request(objects, kinds, segments, units, transitions);
                self.subscribe(req, sub, selector, after).await
            }
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
                let head = self.cached_head().await;
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
                if self.app.cfg.dev
                    && self.diag_budget > 0
                    && self
                        .app
                        .admission
                        .diag_allowed(self.source.as_deref(), now_ms())
                {
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
///
/// `places` are the socket's places under the per-session and per-source caps; they are held
/// until the connection ends. When `stop` is raised the socket is closed (its writer sends the
/// Close frame) before the server's drain deadline.
pub async fn serve(
    app: Shared,
    identity: Identity,
    places: SocketPlaces,
    socket: WebSocket,
    stop: Option<strata_runtime::server::Shutdown>,
) {
    let stopped = async move {
        match stop {
            Some(mut stop) => stop.stopped().await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(stopped);
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
    let mut events = app.hub.events.subscribe(&identity.tenant);
    let mut segment_events = app.hub.segments.subscribe(&identity.tenant);
    let mut runs = app.hub.runs.subscribe();
    let mut notices = app.hub.notices.subscribe(&identity.tenant);
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
        diag_budget: app.cfg.limits.diag_per_socket,
        source: places.source.clone(),
        bucket: Bucket::new(Instant::now()),
        strikes: 0,
        known_head: 0,
        head_at: Instant::now(),
    };
    let correlation = conn.identity.correlation.clone();
    LOG.info(
        "ws.opened",
        &correlation,
        json!({ "conn": id, "tenant": conn.identity.tenant.as_str(), "principal": conn.principal_name() }),
    );
    conn.register();
    let head = conn.refresh_head().await;
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
    let mut last_checked = Instant::now();
    // A published change of this person's roles: whoever committed it (this process, another
    // replica, a command line tool), their sessions may have ended with it, so the next tick
    // asks the store instead of waiting for the recheck.
    // A session that ended between sign-in and the subscription above was announced to nobody,
    // so a socket with a session asks at its first tick.
    let mut recheck_soon = conn.identity.session.is_some();
    let mut reason = "client closed";
    while alive {
        let step: Result<(), Closed> = tokio::select! {
            _ = &mut stopped => {
                reason = "server stopping";
                Err(Closed)
            },
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
                Ok(published) => {
                    if let EventBody::MemberChanged { principal, .. } = &published.event.body {
                        recheck_soon |= published.event.tenant == conn.identity.tenant
                            && matches!(&conn.identity.principal, Principal::User(id) if id == principal);
                    }
                    conn.deliver(&published)
                }
                Err(RecvError::Lagged(_)) => {
                    reason = "fell behind the event stream";
                    let head = conn.head().await;
                    let _ = conn.send(ServerMessage::Resync { reason: reason.into(), head });
                    Err(Closed)
                }
                Err(RecvError::Closed) => Err(Closed),
            },
            published = segment_events.recv() => match published {
                Ok(published) => conn.deliver_segment(&published),
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
            pushed = notices.recv() => match pushed {
                // Only the recipient's own connections hear of a notice (NTF-1569).
                Ok(push) if crate::inbox::visible_to(&push, &conn.identity) => {
                    conn.send(ServerMessage::Inbox {
                        kind: push.kind.to_string(),
                        notice: push.id.clone(),
                        topic: push.topic.clone(),
                        category: push.category.clone(),
                        at: push.at,
                        unread: push.unread,
                    })
                }
                Ok(_) => Ok(()),
                // Notices were missed: the client reads its inbox again, the socket stays.
                Err(RecvError::Lagged(_)) => {
                    let head = conn.known_head;
                    conn.send(ServerMessage::Resync { reason: "inbox notices were missed".into(), head })
                }
                Err(RecvError::Closed) => Err(Closed),
            },
            ended = sessions.recv() => match ended {
                Ok((session, state)) if conn.identity.session.as_ref().is_some_and(|s| s.id == session) => {
                    reason = "session ended";
                    let _ = conn.send(ServerMessage::Session { state });
                    Err(Closed)
                }
                // The notice channel overflowed: this session's notice may be among those lost.
                Err(RecvError::Lagged(_)) => {
                    recheck_soon = true;
                    Ok(())
                }
                _ => Ok(()),
            },
            _ = tick.tick() => {
                if last_heard.elapsed() > Duration::from_millis(HEARTBEAT_MS * 3) {
                    reason = "heartbeat missed";
                    Err(Closed)
                } else if conn
                    .identity
                    .session
                    .as_ref()
                    .is_some_and(|s| now_ms() >= s.expires_at.0)
                {
                    reason = "session ended";
                    let _ = conn.send(ServerMessage::Session { state: SessionState::Expired });
                    Err(Closed)
                } else if let Some(session) = conn
                    .identity
                    .session
                    .clone()
                    .filter(|_| recheck_soon || last_checked.elapsed() >= SESSION_RECHECK)
                {
                    last_checked = Instant::now();
                    recheck_soon = false;
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
    drop(places);
    LOG.info(
        "ws.closed",
        &correlation,
        json!({ "conn": id, "reason": reason, "delivered": conn.delivered }),
    );
    // Dropping the connection drops the sender; the writer flushes what is queued and closes.
    drop(conn);
    let mut writer = writer;
    if tokio::time::timeout(Duration::from_secs(2), &mut writer)
        .await
        .is_err()
    {
        // A client that stalls must not keep the writer task alive.
        writer.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connection_may_burst_then_is_held_to_the_rate() {
        let start = Instant::now();
        let mut bucket = Bucket::new(start);
        let allowed = (0..200).filter(|_| bucket.take(start)).count();
        assert_eq!(allowed, MESSAGE_BURST as usize);
        // A second later the bucket has refilled by the rate, no more.
        let later = start + Duration::from_secs(1);
        let allowed = (0..200).filter(|_| bucket.take(later)).count();
        assert_eq!(allowed, MESSAGES_PER_SECOND as usize);
    }

    #[tokio::test]
    async fn replay_waits_for_the_queue_instead_of_overflowing_it() {
        let (out, mut queue) = mpsc::channel::<u32>(OUTBOUND_QUEUE);
        // A client that has not read leaves the queue full.
        for n in 0..OUTBOUND_QUEUE as u32 {
            out.try_send(n).unwrap();
        }
        let reader = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            for _ in 0..100 {
                queue.recv().await.unwrap();
            }
            queue
        });
        assert!(wait_for_room(&out, REPLAY_ROOM, Duration::from_secs(5))
            .await
            .is_ok());
        assert!(out.capacity() >= REPLAY_ROOM);
        let _queue = reader.await.unwrap();
    }

    #[tokio::test]
    async fn replay_gives_up_on_a_client_that_never_reads() {
        let (out, _queue) = mpsc::channel::<u32>(OUTBOUND_QUEUE);
        for n in 0..OUTBOUND_QUEUE as u32 {
            out.try_send(n).unwrap();
        }
        assert!(wait_for_room(&out, REPLAY_ROOM, Duration::from_millis(50))
            .await
            .is_err());
    }
}
