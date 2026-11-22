//! What `serve_with` allows a connection and a request to cost: a header-read timeout, a request
//! timeout, a cap on open connections, and the peer's address on every request.

use axum::routing::get;
use axum::Router;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};
use strata_runtime::server::{serve_with, ServeLimits};

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

/// Verifies: GWY-1684
/// A response to a client that stops reading must not hold a connection place for ever: after the
/// write idle timeout the connection is dropped and the place is free for the next client.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_stops_reading_is_dropped_and_its_place_freed() {
    async fn big() -> Vec<u8> {
        // Far more than the kernel buffers of both ends hold, so the write must wait for the
        // client to read.
        vec![0u8; 96 * 1024 * 1024]
    }
    let router = Router::new()
        .route("/big", get(big))
        .route("/fast", get(|| async { "fast" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let limits = ServeLimits {
        write_idle_timeout: Duration::from_secs(1),
        max_connections: 1,
        ..ServeLimits::default()
    };
    tokio::spawn(serve_with(listener, router, std::future::pending(), limits));

    // The only place is taken by a client that asks for the big body and never reads it.
    let mut stalled = TcpStream::connect(addr).unwrap();
    stalled
        .write_all(format!("GET /big HTTP/1.1\r\n{GET}").as_bytes())
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let started = Instant::now();
    let served = tokio::task::spawn_blocking(move || {
        exchange(
            addr,
            &format!("GET /fast HTTP/1.1\r\n{GET}"),
            Duration::from_secs(10),
        )
    })
    .await
    .unwrap()
    .expect("the stalled connection's place was freed");
    assert!(served.starts_with("HTTP/1.1 200"), "{served}");
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "freed after {:?}",
        started.elapsed()
    );
    drop(stalled);
}
