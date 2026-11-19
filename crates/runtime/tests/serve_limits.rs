//! What `serve_with` allows a connection and a request to cost: a header-read timeout, a request
//! timeout, a cap on open connections, and the peer's address on every request.

use axum::extract::ConnectInfo;
use axum::routing::get;
use axum::Router;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};
use strata_runtime::server::{serve_with, ServeLimits};

async fn start(limits: ServeLimits) -> SocketAddr {
    async fn slow() -> &'static str {
        tokio::time::sleep(Duration::from_secs(10)).await;
        "late"
    }
    async fn who(ConnectInfo(peer): ConnectInfo<SocketAddr>) -> String {
        peer.ip().to_string()
    }
    let router = Router::new()
        .route("/slow", get(slow))
        .route("/who", get(who))
        .route("/fast", get(|| async { "fast" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve_with(listener, router, std::future::pending(), limits));
    addr
}

fn limits(header: u64, request: u64, connections: usize) -> ServeLimits {
    ServeLimits {
        header_read_timeout: Duration::from_secs(header),
        request_timeout: Duration::from_secs(request),
        max_connections: connections,
    }
}

/// Send `request` and read the whole reply (the request asks for the connection to close).
fn exchange(addr: SocketAddr, request: &str, patience: Duration) -> std::io::Result<String> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(patience))?;
    stream.write_all(request.as_bytes())?;
    let mut reply = String::new();
    let mut buffer = [0u8; 1024];
    loop {
        match stream.read(&mut buffer)? {
            0 => return Ok(reply),
            n => reply.push_str(&String::from_utf8_lossy(&buffer[..n])),
        }
    }
}

const GET: &str = "Connection: close\r\nHost: test\r\n\r\n";

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_never_finishes_its_headers_is_dropped() {
    let addr = start(limits(1, 30, 16)).await;
    let started = Instant::now();
    let outcome = tokio::task::spawn_blocking(move || {
        // The request line and one header, and then silence.
        exchange(
            addr,
            "GET /fast HTTP/1.1\r\nHost: test\r\n",
            Duration::from_secs(8),
        )
    })
    .await
    .unwrap();
    let elapsed = started.elapsed();
    // The server closes the connection (an empty read, or a reset) well before the client's own
    // patience runs out; a timed-out read would mean the server waited.
    if let Err(error) = &outcome {
        assert!(
            !matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ),
            "the server held a half-sent request open: {error}"
        );
    }
    assert!(elapsed < Duration::from_secs(6), "closed after {elapsed:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_that_takes_too_long_is_answered_408() {
    let addr = start(limits(10, 1, 16)).await;
    let started = Instant::now();
    let reply = tokio::task::spawn_blocking(move || {
        exchange(
            addr,
            &format!("GET /slow HTTP/1.1\r\n{GET}"),
            Duration::from_secs(8),
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert!(reply.starts_with("HTTP/1.1 408"), "{reply}");
    assert!(started.elapsed() < Duration::from_secs(6));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_carries_the_address_of_its_peer() {
    let addr = start(ServeLimits::default()).await;
    let reply = tokio::task::spawn_blocking(move || {
        exchange(
            addr,
            &format!("GET /who HTTP/1.1\r\n{GET}"),
            Duration::from_secs(5),
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    assert!(reply.ends_with("127.0.0.1"), "{reply}");
}

#[tokio::test(flavor = "multi_thread")]
async fn connections_beyond_the_cap_wait_until_one_closes() {
    let addr = start(limits(30, 30, 1)).await;
    // The first connection takes the only place and sits idle.
    let first = TcpStream::connect(addr).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let waiting = tokio::task::spawn_blocking(move || {
        exchange(
            addr,
            &format!("GET /fast HTTP/1.1\r\n{GET}"),
            Duration::from_millis(800),
        )
    })
    .await
    .unwrap();
    let error = waiting.expect_err("no place was free, so nothing was served");
    assert!(
        matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ),
        "{error}"
    );
    drop(first);
    let served = tokio::task::spawn_blocking(move || {
        exchange(
            addr,
            &format!("GET /fast HTTP/1.1\r\n{GET}"),
            Duration::from_secs(5),
        )
    })
    .await
    .unwrap()
    .expect("a place was freed");
    assert!(served.starts_with("HTTP/1.1 200"), "{served}");
}
