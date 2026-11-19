//! `strata-backend`: the backend process. Configuration comes from the environment.
//!
//! | Variable | Default | Meaning |
//! |---|---|---|
//! | `STRATA_BACKEND_ADDR` | `127.0.0.1:4101` | Listen address. Must be loopback. |
//! | `STRATA_SERVICE_TOKEN` | none, required | Shared secret the BFF presents. |
//! | `STRATA_STORE` | `sqlite` | `sqlite` or `memory`. |
//! | `STRATA_DB` | `var/strata.db` | SQLite database file. |
//! | `STRATA_TENANTS` | `tenants` | Directory of tenant bundles to install. |
//! | `STRATA_DEV` | unset | `1` enables development logins and fixture endpoints. |
//! | `STRATA_LEAN_DIR` | `lean` | The Lean project, for verification runs. |
//! | `STRATA_SESSION_TTL_MS` | 12 hours | Absolute session lifetime. |
//! | `STRATA_SESSION_IDLE_MS` | 2 hours | Idle session lifetime. |
//! | `STRATA_SOCKETS_PER_SESSION` | 8 | Open sockets one session may hold. |
//! | `STRATA_SOCKETS_PER_SOURCE` | 32 | Open sockets one source address may hold. |
//! | `STRATA_DIAG_PER_SOCKET` | 50 | Browser log lines a development socket may write. |
//! | `STRATA_DIAG_PER_SOURCE` | 200 | Browser log lines one source may write per minute. |
//! | `STRATA_LOGINS_PER_SOURCE` | 30 | Sign-in attempts one source may make per minute. |
//! | `STRATA_ARGON2_MEMORY_MIB` | 64 | Memory for concurrent password verifications (19 MiB each, at least one). |
//! | `STRATA_ARGON2_WAIT_MS` | 3000 | How long a sign-in waits for a place to hash. |
//! | `STRATA_LAB_WORKERS` | 2 | Threads running Impact Lab projections. |
//! | `STRATA_LAB_QUEUE` | 4 | Projections that may wait for a thread. |
//! | `STRATA_LAB_RUNS_PER_TENANT` | 2 | Projections one tenant may have running or waiting. |
//! | `STRATA_VERIFY_INTERVAL_MS` | 60000 | Least time between two verification runs of a tenant. |
//! | `STRATA_HEADER_TIMEOUT_SECS` | 10 | Time a client has to send its request headers. |
//! | `STRATA_REQUEST_TIMEOUT_SECS` | 30 | Time a request may take before it is answered `408`. |
//! | `STRATA_MAX_CONNECTIONS` | 2048 | Open connections. |

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use strata_backend::app::Config;
use strata_backend::limits::Limits;
use strata_kernel::Timestamp;
use strata_protocol::log::now_ms;
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::Store;
use strata_storage_sqlite::SqliteStore;

use strata_runtime::server::{self, env_number as number, env_var as var};

/// A limit from the environment: a whole number, at least one.
fn limit(name: &str, default: i64) -> Result<i64, String> {
    let value = number(name, default)?;
    if value < 1 {
        return Err(format!("{name} must be at least 1"));
    }
    Ok(value)
}

fn limits() -> Result<Limits, String> {
    let defaults = Limits::default();
    Ok(Limits {
        sockets_per_session: limit(
            "STRATA_SOCKETS_PER_SESSION",
            defaults.sockets_per_session as i64,
        )? as usize,
        sockets_per_source: limit(
            "STRATA_SOCKETS_PER_SOURCE",
            defaults.sockets_per_source as i64,
        )? as usize,
        diag_per_socket: limit(
            "STRATA_DIAG_PER_SOCKET",
            i64::from(defaults.diag_per_socket),
        )? as u32,
        diag_per_source_per_minute: limit(
            "STRATA_DIAG_PER_SOURCE",
            defaults.diag_per_source_per_minute as i64,
        )? as usize,
        logins_per_source_per_minute: limit(
            "STRATA_LOGINS_PER_SOURCE",
            defaults.logins_per_source_per_minute as i64,
        )? as usize,
        argon2_memory_mib: limit(
            "STRATA_ARGON2_MEMORY_MIB",
            i64::from(defaults.argon2_memory_mib),
        )? as u32,
        argon2_wait_ms: limit("STRATA_ARGON2_WAIT_MS", defaults.argon2_wait_ms as i64)? as u64,
        lab_workers: limit("STRATA_LAB_WORKERS", defaults.lab_workers as i64)? as usize,
        lab_queue: limit("STRATA_LAB_QUEUE", defaults.lab_queue as i64)? as usize,
        lab_runs_per_tenant: limit(
            "STRATA_LAB_RUNS_PER_TENANT",
            defaults.lab_runs_per_tenant as i64,
        )? as usize,
        verify_interval_ms: limit("STRATA_VERIFY_INTERVAL_MS", defaults.verify_interval_ms)?,
    })
}

fn start() -> Result<(Config, Arc<dyn Store>), String> {
    let addr: SocketAddr = var("STRATA_BACKEND_ADDR", "127.0.0.1:4101")
        .parse()
        .map_err(|e| format!("STRATA_BACKEND_ADDR: {e}"))?;
    if !addr.ip().is_loopback() {
        return Err(format!("the backend only listens on loopback, not {addr}"));
    }
    let service_token = var("STRATA_SERVICE_TOKEN", "");
    if service_token.len() < 24 {
        return Err("STRATA_SERVICE_TOKEN must be set to at least 24 characters".into());
    }
    let store: Arc<dyn Store> = match var("STRATA_STORE", "sqlite").as_str() {
        "memory" => Arc::new(MemoryStore::new()),
        "sqlite" => {
            let path = PathBuf::from(var("STRATA_DB", "var/strata.db"));
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            Arc::new(SqliteStore::open(&path, Timestamp(now_ms())).map_err(|e| e.to_string())?)
        }
        other => return Err(format!("STRATA_STORE: unknown adapter {other:?}")),
    };
    let cfg = Config {
        addr,
        service_token,
        dev: var("STRATA_DEV", "") == "1",
        tenants_dir: PathBuf::from(var("STRATA_TENANTS", "tenants")),
        lean_dir: PathBuf::from(var("STRATA_LEAN_DIR", "lean")),
        session_ttl_ms: number("STRATA_SESSION_TTL_MS", 12 * 3_600_000)?,
        session_idle_ms: number("STRATA_SESSION_IDLE_MS", 2 * 3_600_000)?,
        limits: limits()?,
    };
    Ok((cfg, store))
}

#[tokio::main]
async fn main() {
    let result = async {
        let (cfg, store) = start()?;
        let listener = server::bind(cfg.addr).await?;
        strata_backend::run(cfg, store, listener, server::interrupted()).await
    }
    .await;
    server::exit_on_error("strata-backend", result);
}
