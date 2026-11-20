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
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{watch, Semaphore};
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

/// What a server allows a connection and a request to cost.
///
/// | Environment variable | Default | Meaning |
/// |---|---|---|
/// | `STRATA_HEADER_TIMEOUT_SECS` | `10` | A client must finish its request headers within this time. |
/// | `STRATA_REQUEST_TIMEOUT_SECS` | `30` | A request, body included, is answered `408` after this time. |
/// | `STRATA_MAX_CONNECTIONS` | `2048` | Open connections; further ones wait in the kernel's backlog. |
/// | `STRATA_MAX_CONNECTIONS_PER_SOURCE` | `512` | Open connections from one peer address (an IPv6 /64 counts as one); further ones are closed at once. |
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServeLimits {
    pub header_read_timeout: Duration,
    pub request_timeout: Duration,
    pub max_connections: usize,
    pub max_connections_per_source: usize,
}

impl Default for ServeLimits {
    fn default() -> Self {
        ServeLimits {
            header_read_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
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
/// Upgraded connections (WebSockets) are not subject to the request timeout.
pub async fn serve_with(
    listener: TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
    limits: ServeLimits,
) -> Result<(), String> {
    let slots = Arc::new(Semaphore::new(limits.max_connections));
    let per_source = Cap::new(limits.max_connections_per_source);
    let (stop_tx, stop_rx) = watch::channel(false);
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
        let mut stop = stop_rx.clone();
        connections.spawn(async move {
            let _slot = permit;
            let _place = place;
            let service = hyper::service::service_fn(move |request: Request<Incoming>| {
                let router = router.clone();
                async move {
                    let (mut parts, body) = request.into_parts();
                    parts.extensions.insert(ConnectInfo(peer));
                    let request = Request::from_parts(parts, Body::new(body));
                    match tokio::time::timeout(limits.request_timeout, router.oneshot(request))
                        .await
                    {
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
            let connection = builder.serve_connection_with_upgrades(TokioIo::new(stream), service);
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
    // Connections finish what they are doing; any still open at the deadline are dropped.
    let drained = tokio::time::timeout(drain_deadline(), async {
        while connections.join_next().await.is_some() {}
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

/// Run `work` on a blocking worker thread, off the async executor, and wait for it. Storage
/// calls, which are synchronous, go through here.
pub async fn blocking<T, F>(work: F) -> Result<T, WorkerFailed>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| WorkerFailed(e.to_string()))
}
