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

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use strata_backend::app::Config;
use strata_kernel::Timestamp;
use strata_protocol::log::now_ms;
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::Store;
use strata_storage_sqlite::SqliteStore;

fn var(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn number(name: &str, default: i64) -> Result<i64, String> {
    var(name, &default.to_string())
        .parse()
        .map_err(|_| format!("{name} must be a number"))
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
    };
    Ok((cfg, store))
}

#[tokio::main]
async fn main() {
    let result = async {
        let (cfg, store) = start()?;
        let listener = tokio::net::TcpListener::bind(cfg.addr)
            .await
            .map_err(|e| format!("{}: {e}", cfg.addr))?;
        let shutdown = async {
            let _ = tokio::signal::ctrl_c().await;
        };
        strata_backend::run(cfg, store, listener, shutdown).await
    }
    .await;
    if let Err(problem) = result {
        eprintln!("strata-backend: {problem}");
        std::process::exit(1);
    }
}
