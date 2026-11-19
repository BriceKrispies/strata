//! Process bootstrap shared by every Strata service: environment configuration, binding,
//! serving an axum router until shutdown, running blocking work, and the process exit.

use axum::Router;
use std::future::Future;
use std::net::SocketAddr;
use tokio::net::TcpListener;

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

/// Resolves when the process is asked to stop (Ctrl-C).
pub async fn interrupted() {
    let _ = tokio::signal::ctrl_c().await;
}

/// Serve `router` on `listener` until `shutdown` resolves, then finish the requests in flight.
pub async fn serve(
    listener: TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), String> {
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|e| e.to_string())
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
