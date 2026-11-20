//! Process bootstrap shared by every Strata service: environment configuration, binding,
//! serving an axum router until shutdown, running blocking work, and the process exit.

use crate::limits::{source_key, Cap};
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use axum::Router;
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder;
use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch, Semaphore};
use tokio::time::Sleep;
use tower::ServiceExt;

/// The environment variable `name`, or `default` when it is unset or empty.
pub fn env_var(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// The environment variable `name` as a whole number, or `default` when it is unset or empty.
pub fn env_number(name: &str, default: i64) -> Result<i64, String> {
    env_var(name, &default.to_string())
        .parse()
        .map_err(|_| format!("{name} must be a number"))
}

/// Bind a listener on `addr`. The error names the address.
pub async fn bind(addr: SocketAddr) -> Result<TcpListener, String> {
    TcpListener::bind(addr)
        .await
        .map_err(|e| format!("{addr}: {e}"))
}

/// Resolves when the process is asked to stop: Ctrl-C, or on Unix SIGTERM, which is what an
/// orchestrator sends.
pub async fn interrupted() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

/// How long, after a stop is asked for, open connections may take to finish before they are
/// dropped (`STRATA_DRAIN_TIMEOUT_SECS`, default 30).
pub fn drain_deadline() -> Duration {
    let secs = env_number("STRATA_DRAIN_TIMEOUT_SECS", 30)
        .ok()
        .and_then(|v| u64::try_from(v).ok())
        .filter(|v| *v > 0)
        .unwrap_or(30);
    Duration::from_secs(secs)
}

/// The server's stop signal, for work that outlives a request: an upgraded WebSocket runs outside
/// the connection set the server drains, so it has to be told.
///
/// [`serve_with`] puts one in the extensions of every request (`Option<Extension<Shutdown>>` in a
/// handler; it is absent when the router is not served by `serve_with`). A socket task calls
/// [`Shutdown::hold`] before it is spawned, keeps the [`Held`] it gets for as long as it runs, and
/// waits on [`Shutdown::stopped`] beside its other work: when that resolves it sends its Close
/// frame and ends. The server waits for every [`Held`] to be dropped, up to the drain deadline,
/// before it returns.
#[derive(Clone, Debug)]
pub struct Shutdown {
    stop: watch::Receiver<bool>,
    open: mpsc::WeakSender<()>,
}

/// A place in the server's drain: while one exists the server has not finished stopping.
#[derive(Debug)]
pub struct Held(#[allow(dead_code)] mpsc::Sender<()>);

impl Shutdown {
    /// Resolves once the server has been asked to stop (at once when it already has been).
    pub async fn stopped(&mut self) {
        let _ = self.stop.wait_for(|stopping| *stopping).await;
    }

    /// Whether the server has been asked to stop.
    pub fn is_stopping(&self) -> bool {
        *self.stop.borrow()
    }

    /// Register a long-lived task with the drain. `None` when the server has finished draining:
    /// the task must not start.
    pub fn hold(&self) -> Option<Held> {
        self.open.upgrade().map(Held)
    }
}

/// A byte stream whose writes may not stall for ever: when the peer stops reading and a write
/// cannot make progress for `idle`, the write fails with `TimedOut`, which ends the connection
/// and frees what it held. Reads pass through, so a quiet connection that is merely waiting for
/// the client's next request is not affected (the header-read timeout covers that).
struct IdleWrite<T> {
    inner: T,
    idle: Duration,
    deadline: Pin<Box<Sleep>>,
    waiting: bool,
}

impl<T> IdleWrite<T> {
    fn new(inner: T, idle: Duration) -> Self {
        IdleWrite {
            inner,
            idle,
            deadline: Box::pin(tokio::time::sleep(idle)),
            waiting: false,
        }
    }

    /// A write that could not proceed: start the clock if it is not running, and fail the write
    /// when the clock has run out.
    fn stalled<R>(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Result<R>> {
        if !self.waiting {
            self.waiting = true;
            self.deadline
                .as_mut()
                .reset(tokio::time::Instant::now() + self.idle);
        }
        match self.deadline.as_mut().poll(cx) {
            Poll::Ready(()) => {
                self.waiting = false;
                Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "the peer did not read for too long",
                )))
            }
            Poll::Pending => Poll::Pending,
        }
    }

    /// A write that made progress (or failed): the clock stops.
    fn progressed<R>(&mut self, ready: Poll<R>) -> Poll<R> {
        if ready.is_ready() {
            self.waiting = false;
        }
        ready
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for IdleWrite<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for IdleWrite<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match Pin::new(&mut self.inner).poll_write(cx, buf) {
            Poll::Pending => self.stalled(cx),
            ready => self.progressed(ready),
        }
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        match Pin::new(&mut self.inner).poll_write_vectored(cx, bufs) {
            Poll::Pending => self.stalled(cx),
            ready => self.progressed(ready),
        }
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match Pin::new(&mut self.inner).poll_flush(cx) {
            Poll::Pending => self.stalled(cx),
            ready => self.progressed(ready),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match Pin::new(&mut self.inner).poll_shutdown(cx) {
            Poll::Pending => self.stalled(cx),
            ready => self.progressed(ready),
        }
    }
}

/// What a server allows a connection and a request to cost.
///
/// | Environment variable | Default | Meaning |
/// |---|---|---|
/// | `STRATA_HEADER_TIMEOUT_SECS` | `10` | A client must finish its request headers within this time. |
/// | `STRATA_REQUEST_TIMEOUT_SECS` | `30` | A request, body included, is answered `408` after this time. |
/// | `STRATA_MAX_CONNECTIONS` | `2048` | Open connections; further ones wait in the kernel's backlog. |
/// | `STRATA_MAX_CONNECTIONS_PER_SOURCE` | `512` | Open connections from one peer address (an IPv6 /64 counts as one); further ones are closed at once. |
/// | `STRATA_WRITE_IDLE_TIMEOUT_SECS` | `30` | A connection whose client has not read for this long while the server has bytes to send is dropped, and its place freed. |
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServeLimits {
    pub header_read_timeout: Duration,
    pub request_timeout: Duration,
    pub write_idle_timeout: Duration,
    pub max_connections: usize,
    pub max_connections_per_source: usize,
}

impl Default for ServeLimits {
    fn default() -> Self {
        ServeLimits {
            header_read_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            write_idle_timeout: Duration::from_secs(30),
            max_connections: 2048,
            max_connections_per_source: 512,
        }
    }
}

impl ServeLimits {
    /// The defaults, overridden by the environment variables above.
    pub fn from_env() -> Result<Self, String> {
        let defaults = ServeLimits::default();
        let positive = |name: &str, default: u64| -> Result<u64, String> {
            let value = env_number(name, default as i64)?;
            u64::try_from(value)
                .ok()
                .filter(|v| *v > 0)
                .ok_or_else(|| format!("{name} must be a positive number"))
        };
        Ok(ServeLimits {
            header_read_timeout: Duration::from_secs(positive(
                "STRATA_HEADER_TIMEOUT_SECS",
                defaults.header_read_timeout.as_secs(),
            )?),
            request_timeout: Duration::from_secs(positive(
                "STRATA_REQUEST_TIMEOUT_SECS",
                defaults.request_timeout.as_secs(),
            )?),
            write_idle_timeout: Duration::from_secs(positive(
                "STRATA_WRITE_IDLE_TIMEOUT_SECS",
                defaults.write_idle_timeout.as_secs(),
            )?),
            max_connections: positive("STRATA_MAX_CONNECTIONS", defaults.max_connections as u64)?
                as usize,
            max_connections_per_source: positive(
                "STRATA_MAX_CONNECTIONS_PER_SOURCE",
                defaults.max_connections_per_source as u64,
            )? as usize,
        })
    }
}

/// Serve `router` on `listener` until `shutdown` resolves, then finish the requests in flight.
/// Limits come from the environment ([`ServeLimits::from_env`]).
pub async fn serve(
    listener: TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), String> {
    serve_with(listener, router, shutdown, ServeLimits::from_env()?).await
}

/// [`serve`] with explicit limits.
///
/// Every request carries the peer's address as `ConnectInfo<SocketAddr>`. A client that does not
/// finish its headers within `header_read_timeout` is dropped, a request not answered within
/// `request_timeout` gets `408`, at most `max_connections` connections are open at once, and at
/// most `max_connections_per_source` of them come from one peer address.
/// Upgraded connections (WebSockets) are not subject to the request timeout. A connection whose
/// client stops reading is dropped after `write_idle_timeout`, upgraded ones included.
///
/// Every request also carries a [`Shutdown`]. On stop the server finishes its requests, then waits
/// (within [`drain_deadline`]) for every [`Held`] a socket task took, so that sockets can send a
/// Close frame before the process ends.
pub async fn serve_with(
    listener: TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
    limits: ServeLimits,
) -> Result<(), String> {
    let slots = Arc::new(Semaphore::new(limits.max_connections));
    let per_source = Cap::new(limits.max_connections_per_source);
    let (stop_tx, stop_rx) = watch::channel(false);
    let (open_tx, mut open_rx) = mpsc::channel::<()>(1);
    let shutdown_handle = Shutdown {
        stop: stop_rx.clone(),
        open: open_tx.downgrade(),
    };
    tokio::spawn(async move {
        shutdown.await;
        let _ = stop_tx.send(true);
    });
    let mut stopping = stop_rx.clone();
    let mut connections = tokio::task::JoinSet::new();
    loop {
        while connections.try_join_next().is_some() {}
        let permit = tokio::select! {
            permit = slots.clone().acquire_owned() => match permit {
                Ok(permit) => permit,
                Err(_) => break,
            },
            _ = stopping.changed() => break,
        };
        let (stream, peer) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            },
            _ = stopping.changed() => break,
        };
        let Some(place) = per_source.acquire(&source_key(&peer.ip().to_string())) else {
            // This address already holds as many connections as it may: close this one.
            continue;
        };
        let _ = stream.set_nodelay(true);
        let router = router.clone();
        let shutdown_handle = shutdown_handle.clone();
        let mut stop = stop_rx.clone();
        connections.spawn(async move {
            let _slot = permit;
            let _place = place;
            let service = hyper::service::service_fn(move |request: Request<Incoming>| {
                let router = router.clone();
                let shutdown_handle = shutdown_handle.clone();
                async move {
                    let (mut parts, body) = request.into_parts();
                    parts.extensions.insert(ConnectInfo(peer));
                    parts.extensions.insert(shutdown_handle);
                    let request = Request::from_parts(parts, Body::new(body));
                    let deadline = Instant::now() + limits.request_timeout;
                    let answered = REQUEST_DEADLINE.scope(deadline, router.oneshot(request));
                    match tokio::time::timeout(limits.request_timeout, answered).await {
                        Ok(Ok(response)) => Ok::<_, Infallible>(response),
                        Ok(Err(never)) => match never {},
                        Err(_) => {
                            Ok((StatusCode::REQUEST_TIMEOUT, "request timed out").into_response())
                        }
                    }
                }
            });
            let mut builder = Builder::new(TokioExecutor::new());
            builder
                .http1()
                .timer(TokioTimer::new())
                .header_read_timeout(limits.header_read_timeout);
            let connection = builder.serve_connection_with_upgrades(
                TokioIo::new(IdleWrite::new(stream, limits.write_idle_timeout)),
                service,
            );
            tokio::pin!(connection);
            let mut draining = false;
            loop {
                tokio::select! {
                    _ = connection.as_mut() => break,
                    _ = stop.changed(), if !draining => {
                        draining = true;
                        connection.as_mut().graceful_shutdown();
                    }
                }
            }
        });
    }
    drop(listener);
    // Connections finish what they are doing, and upgraded sockets (which the stop signal has
    // told to close) end; whatever is still open at the deadline is dropped.
    let drained = tokio::time::timeout(drain_deadline(), async {
        while connections.join_next().await.is_some() {}
        // No new socket may register now; wait for those that did.
        drop(open_tx);
        while open_rx.recv().await.is_some() {}
    })
    .await;
    if drained.is_err() {
        connections.abort_all();
    }
    Ok(())
}

/// End the process: normally on `Ok`, and on `Err` with the problem on standard error, prefixed
/// by `program`, and exit status 1.
pub fn exit_on_error(program: &str, result: Result<(), String>) {
    if let Err(problem) = result {
        eprintln!("{program}: {problem}");
        std::process::exit(1);
    }
}

/// A blocking worker thread failed (it panicked or was cancelled).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerFailed(pub String);

impl std::fmt::Display for WorkerFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WorkerFailed {}

tokio::task_local! {
    /// When the request being served is answered `408`.
    static REQUEST_DEADLINE: Instant;
}

thread_local! {
    static WORK_DEADLINE: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// The deadline of the request whose [`blocking`] work is running on this thread: when the
/// request is answered `408`. Work that would change the store checks it before the change and
/// does nothing once it has passed (a timeout cancels nothing by itself, since the worker thread
/// cannot be stopped). `None` outside a request.
pub fn work_deadline() -> Option<Instant> {
    WORK_DEADLINE.with(std::cell::Cell::get)
}

/// Run `work` on a blocking worker thread, off the async executor, and wait for it. Storage
/// calls, which are synchronous, go through here. The request's deadline is available to `work`
/// as [`work_deadline`].
pub async fn blocking<T, F>(work: F) -> Result<T, WorkerFailed>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let deadline = REQUEST_DEADLINE.try_with(|at| *at).ok();
    tokio::task::spawn_blocking(move || {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                WORK_DEADLINE.with(|cell| cell.set(None));
            }
        }
        WORK_DEADLINE.with(|cell| cell.set(deadline));
        let _reset = Reset;
        work()
    })
    .await
    .map_err(|e| WorkerFailed(e.to_string()))
}
