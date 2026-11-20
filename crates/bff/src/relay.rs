//! The WebSocket relay between a browser and the backend.
//!
//! One upstream connection per browser connection, opened with the service token, the site and
//! the browser's session token. Frames from the browser are parsed against protocol v1 before
//! they are forwarded; anything else is answered with an `error` and dropped. Both directions
//! pass through bounded queues, and a side that cannot keep up ends the connection, after which
//! the browser reconnects and resumes from the sequence it last processed.

use crate::limits::Places;
use crate::routes::Visit;
use crate::{Shared, LOG};
use axum::extract::ws::{CloseFrame, Message as BrowserMessage, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::sync::atomic::Ordering;
use strata_protocol::api::{
    HEADER_CORRELATION, HEADER_SERVICE, HEADER_SESSION, HEADER_SITE, HEADER_SOURCE,
};
use strata_protocol::ws::{parse_client, ServerMessage};
use strata_runtime::server::Shutdown;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as UpstreamMessage;

const QUEUE: usize = 128;

/// Relay one browser socket until it ends. `places` are its places under the per-session and
/// per-source caps, held until the end. When `stop` is raised the browser is sent a Close frame
/// (1001, going away) and the socket ends.
pub async fn relay(
    bff: Shared,
    visit: Visit,
    places: Places,
    browser: WebSocket,
    stop: Option<Shutdown>,
) {
    bff.sockets.fetch_add(1, Ordering::Relaxed);
    let reason = pump(&bff, &visit, browser, stop).await;
    bff.sockets.fetch_sub(1, Ordering::Relaxed);
    drop(places);
    LOG.info(
        "ws.relay_closed",
        &visit.correlation,
        json!({ "reason": reason }),
    );
}

async fn pump(
    bff: &Shared,
    visit: &Visit,
    browser: WebSocket,
    stop: Option<Shutdown>,
) -> &'static str {
    // A socket that is never told to stop waits on a future that never resolves.
    let stopped = async move {
        match stop {
            Some(mut stop) => stop.stopped().await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(stopped);
    let (mut to_browser, mut from_browser) = browser.split();

    let url = format!("{}/internal/ws", bff.cfg.backend.replacen("http", "ws", 1));
    let Ok(mut request) = url.into_client_request() else {
        return "bad backend address";
    };
    let headers = request.headers_mut();
    let mut put = |name: &'static str, value: &str| {
        if let Ok(value) = value.parse() {
            headers.insert(name, value);
        }
    };
    put(HEADER_SERVICE, &bff.cfg.service_token);
    put(HEADER_SITE, visit.site.as_str());
    put(HEADER_CORRELATION, &visit.correlation);
    if let Some(token) = &visit.token {
        put(HEADER_SESSION, token);
    }
    if let Some(source) = &visit.source {
        put(HEADER_SOURCE, source);
    }
    let upstream = match tokio_tungstenite::connect_async(request).await {
        Ok((upstream, _)) => upstream,
        Err(error) => {
            LOG.error(
                "ws.upstream_failed",
                &visit.correlation,
                json!({ "error": error.to_string() }),
            );
            let _ = to_browser
                .send(BrowserMessage::Close(Some(CloseFrame {
                    code: 1013,
                    reason: "backend unavailable".into(),
                })))
                .await;
            return "backend unavailable";
        }
    };
    let (mut to_upstream, mut from_upstream) = upstream.split();
    LOG.info(
        "ws.relay_opened",
        &visit.correlation,
        json!({ "site": visit.site.as_str() }),
    );

    // Everything bound for the browser goes through one bounded queue and one writer.
    let (outbound, mut queue) = mpsc::channel::<String>(QUEUE);
    let writer = tokio::spawn(async move {
        while let Some(frame) = queue.recv().await {
            if to_browser.send(BrowserMessage::Text(frame)).await.is_err() {
                return;
            }
        }
        let _ = to_browser
            .send(BrowserMessage::Close(Some(CloseFrame {
                code: 1001,
                reason: "closing".into(),
            })))
            .await;
    });

    let reason = loop {
        tokio::select! {
            _ = &mut stopped => break "server stopping",
            frame = from_browser.next() => match frame {
                Some(Ok(BrowserMessage::Text(text))) => match parse_client(&text) {
                    Ok(_) => {
                        if to_upstream.send(UpstreamMessage::Text(text)).await.is_err() {
                            break "backend closed";
                        }
                    }
                    Err(problem) => {
                        let reply = ServerMessage::Error { req: None, code: "invalid".into(), message: problem };
                        if outbound.try_send(serde_json::to_string(&reply).unwrap_or_default()).is_err() {
                            break "browser too slow";
                        }
                    }
                },
                Some(Ok(BrowserMessage::Ping(_) | BrowserMessage::Pong(_))) => {}
                Some(Ok(BrowserMessage::Binary(_))) => break "binary frame from browser",
                Some(Ok(BrowserMessage::Close(_))) | Some(Err(_)) | None => break "browser closed",
            },
            frame = from_upstream.next() => match frame {
                Some(Ok(UpstreamMessage::Text(text))) => {
                    if outbound.try_send(text).is_err() {
                        break "browser too slow";
                    }
                }
                Some(Ok(UpstreamMessage::Ping(_) | UpstreamMessage::Pong(_))) => {}
                Some(Ok(_)) | Some(Err(_)) | None => break "backend closed",
            },
        }
    };
    let _ = to_upstream.close().await;
    drop(outbound);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), writer).await;
    reason
}
