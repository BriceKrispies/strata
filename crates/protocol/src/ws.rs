//! WebSocket protocol v1.
//!
//! Text frames, one JSON object per frame, discriminated by `type`. The same messages travel
//! browser to BFF and BFF to backend; the BFF validates and relays.
//!
//! # Conversation
//!
//! 1. The server opens with `welcome`: protocol version, connection id, the tenant's current
//!    event sequence (`head`), the session state and the heartbeat interval.
//! 2. The client sends `subscribe` with a request id, a subscription id of its choosing and an
//!    optional `after` sequence. The server answers `ok` (or `error`), replays the visible
//!    events with a sequence above `after`, then delivers new ones as they are committed.
//! 3. Every `event` carries `n`, a counter that rises by exactly one per event on this
//!    connection, and `seq`, the tenant's event sequence. `seq` may skip (events the subscriber
//!    may not see are not delivered); `n` never does. A client that sees a gap in `n` has lost a
//!    message and must reconnect.
//! 4. The client acknowledges with `ack`, naming the highest `n` it has processed. A client that falls too far behind is sent `resync`
//!    and must read its data again; so is one that asks to resume from further back than the
//!    server will replay.
//! 5. The client sends `ping` at the heartbeat interval. The server answers `pong` with the
//!    current `head`, so an idle client can tell it has missed nothing. A connection that stays
//!    silent for three intervals is closed.
//! 6. After a reconnect the client subscribes again with `after` set to the last `seq` it
//!    processed. It either receives what it missed or a `resync`.
//!
//! The server sends `session` when the session behind the connection ends, then closes.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use strata_substrate::subscribe::Notice;

pub const VERSION: u32 = 1;

/// The largest frame either side accepts, in bytes.
pub const MAX_FRAME: usize = 16 * 1024;
/// The most events replayed for one `subscribe`. Beyond this the client is told to resync.
pub const MAX_REPLAY: usize = 500;
/// Unacknowledged events allowed on one connection before it is told to resync.
pub const MAX_UNACKED: u64 = 512;
/// How often the client pings.
pub const HEARTBEAT_MS: u64 = 10_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientMessage {
    Subscribe {
        req: String,
        sub: String,
        /// Limit record notices to these object types.
        #[serde(default)]
        objects: Option<Vec<String>>,
        /// Replay visible events with a sequence above this.
        #[serde(default)]
        after: Option<u64>,
    },
    Unsubscribe { req: String, sub: String },
    Ack { n: u64 },
    Ping { nonce: u64 },
    /// Stop a running simulation.
    Cancel { req: String, run: String },
    /// A diagnostic line from the browser, for the development log.
    Diag { level: String, event: String, correlation: String, detail: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Anonymous,
    Active,
    Expired,
    Revoked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Welcome { v: u32, conn: String, head: u64, session: SessionState, heartbeat_ms: u64 },
    Ok { req: String },
    Error { req: Option<String>, code: String, message: String },
    Event {
        sub: String,
        n: u64,
        seq: u64,
        /// The correlation id of the request that caused the event.
        correlation: String,
        notice: Notice,
    },
    /// The client can no longer be brought up to date event by event and must read again.
    Resync { reason: String, head: u64 },
    Pong { nonce: u64, head: u64 },
    Session { state: SessionState },
    /// Progress or the result of a simulation or verification run.
    Run { run: String, kind: String, payload: Json },
}

/// Parse a client frame, refusing oversized or malformed ones.
pub fn parse_client(frame: &str) -> Result<ClientMessage, String> {
    if frame.len() > MAX_FRAME {
        return Err(format!("frame of {} bytes exceeds {MAX_FRAME}", frame.len()));
    }
    let message: ClientMessage = serde_json::from_str(frame).map_err(|e| e.to_string())?;
    let short = |s: &str| !s.is_empty() && s.len() <= 64;
    let ok = match &message {
        ClientMessage::Subscribe { req, sub, objects, .. } => {
            short(req) && short(sub) && objects.as_ref().is_none_or(|o| o.len() <= 32)
        }
        ClientMessage::Unsubscribe { req, sub } => short(req) && short(sub),
        ClientMessage::Cancel { req, run } => short(req) && short(run),
        ClientMessage::Diag { level, event, correlation, detail } => {
            short(level) && short(event) && correlation.len() <= 64 && detail.len() <= 2_000
        }
        ClientMessage::Ack { .. } | ClientMessage::Ping { .. } => true,
    };
    if ok {
        Ok(message)
    } else {
        Err("a field is empty or too long".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_frames_are_validated() {
        assert_eq!(
            parse_client(r#"{"type":"subscribe","req":"r1","sub":"s1","after":7}"#),
            Ok(ClientMessage::Subscribe { req: "r1".into(), sub: "s1".into(), objects: None, after: Some(7) })
        );
        assert!(parse_client(r#"{"type":"subscribe","req":"","sub":"s1"}"#).is_err());
        assert!(parse_client(r#"{"type":"become_admin"}"#).is_err());
        assert!(parse_client(r#"{"type":"ping","nonce":1,"tenant":"other"}"#).is_err(), "unknown fields are refused");
        assert!(parse_client(&format!(r#"{{"type":"ping","nonce":1}}{}"#, " ".repeat(MAX_FRAME))).is_err());
    }

    #[test]
    fn server_frames_have_a_stable_shape() {
        let frame = serde_json::to_string(&ServerMessage::Pong { nonce: 3, head: 9 }).unwrap();
        assert_eq!(frame, r#"{"type":"pong","nonce":3,"head":9}"#);
    }
}
