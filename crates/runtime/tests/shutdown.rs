//! What a server tells long-lived work when it is asked to stop: the stop signal reaches a task
//! that outlives its request (an upgraded socket), and the server waits for that task to end.

use axum::extract::{Extension, State};
use axum::routing::get;
use axum::Router;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use strata_runtime::server::{serve_with, ServeLimits, Shutdown};

/// Verifies: GWY-1682
#[tokio::test(flavor = "multi_thread")]
async fn a_task_that_outlives_its_request_is_told_to_stop_and_waited_for() {
    #[derive(Clone)]
    struct Flags {
        started: Arc<AtomicBool>,
        closed: Arc<AtomicBool>,
    }
    async fn hold(
        State(flags): State<Flags>,
        Extension(mut stop): Extension<Shutdown>,
    ) -> &'static str {
        let held = stop.hold().expect("the server is serving");
        flags.started.store(true, Ordering::SeqCst);
        tokio::spawn(async move {
            stop.stopped().await;
            // Closing takes a moment, as sending a Close frame does.
            tokio::time::sleep(Duration::from_millis(300)).await;
            flags.closed.store(true, Ordering::SeqCst);
            drop(held);
        });
        "held"
    }
    let flags = Flags {
        started: Arc::new(AtomicBool::new(false)),
        closed: Arc::new(AtomicBool::new(false)),
    };
    let router = Router::new()
        .route("/hold", get(hold))
        .with_state(flags.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve_with(
        listener,
        router,
        async move {
            let _ = stop_rx.await;
        },
        ServeLimits::default(),
    ));

    tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .write_all(b"GET /hold HTTP/1.1\r\nConnection: close\r\nHost: test\r\n\r\n")
            .unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    })
    .await
    .unwrap();
    assert!(flags.started.load(Ordering::SeqCst));
    assert!(
        !flags.closed.load(Ordering::SeqCst),
        "nothing has been asked to stop yet"
    );

    let asked = Instant::now();
    stop_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("the server ended well before the drain deadline")
        .unwrap()
        .unwrap();
    assert!(
        flags.closed.load(Ordering::SeqCst),
        "the server returned while a held task was still open"
    );
    assert!(asked.elapsed() < Duration::from_secs(10));
}
