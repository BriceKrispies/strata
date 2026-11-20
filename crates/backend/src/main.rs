//! `strata-backend`: the backend process. Configuration comes from the environment.
//!
//! | Variable | Default | Meaning |
//! |---|---|---|
//! | `STRATA_BACKEND_ADDR` | `127.0.0.1:4101` | Listen address. Must be loopback. |
//! | `STRATA_SERVICE_TOKEN` | none, required | Shared secret the BFF presents. |
//! | `STRATA_STORE` | `sqlite` | `sqlite` or `memory`. |
//! | `STRATA_DB` | `var/strata.db` | SQLite control database (the tenant directory). |
//! | `STRATA_TENANT_FILES` | `<STRATA_DB stem>-tenants` beside it | Directory of the per-tenant SQLite files. |
//! | `STRATA_TENANTS` | `tenants` | Directory of tenant bundles to install. |
//! | `STRATA_DEV` | unset | `1` enables development logins and fixture endpoints. |
//! | `STRATA_LEAN_DIR` | `lean` | The Lean project, for verification runs. |
//! | `STRATA_SESSION_TTL_MS` | 12 hours | Absolute session lifetime. |
//! | `STRATA_SESSION_IDLE_MS` | 2 hours | Idle session lifetime. |
//! | `STRATA_PASSKEY_CHALLENGE_TTL_MS` | 5 minutes | How long a passkey challenge can be answered. |
//! | `STRATA_LINK_TTL_MS` | 15 minutes | How long an emailed sign-in link is accepted. |
//! | `STRATA_SMTP_URL` | unset | An SMTP server (`smtps://user:password@host:465`). Unset: no mail is sent. |
//! | `STRATA_MAIL_FROM` | none | The sender address, required with `STRATA_SMTP_URL`. |
//! | `STRATA_MAIL_DIR` | `var/mail` | Where the development sink writes `.eml` files (development only). |
//! | `STRATA_OIDC_CONFIG` | unset | A JSON file of each tenant's OpenID Connect providers. Unset: no tenant has one. |
//! | `STRATA_SECOND_FACTOR_REALMS` | unset | Tenants whose realm policy requires a second factor (IDN-018), comma-separated. |
//! | `STRATA_STEP_UP_AGE_MS` | 15 minutes | How recent an authentication must be for a sensitive operation (IDN-019). |
//! | `STRATA_RECOVERY_DELAY_MS` | 24 hours | How long account recovery waits before it can complete (IDN-265). |
//! | `STRATA_FACTOR_KEY` | the service token | The secret TOTP secrets are sealed under. |
//! | `STRATA_SAML_CONFIG` | unset | A JSON file of each tenant's SAML providers. Unset: no tenant has one. |
//! | `STRATA_SOCKETS_PER_SESSION` | 8 | Open sockets one session may hold. |
//! | `STRATA_SOCKETS_PER_SOURCE` | 32 | Open sockets one source address may hold. |
//! | `STRATA_DIAG_PER_SOCKET` | 50 | Browser log lines a development socket may write. |
//! | `STRATA_DIAG_PER_SOURCE` | 200 | Browser log lines one source may write per minute. |
//! | `STRATA_LOGINS_PER_SOURCE` | 30 | Sign-in attempts one source may make per minute. |
//! | `STRATA_ARGON2_MEMORY_MIB` | 64 | Memory for concurrent password verifications (19 MiB each, at least one). |
//! | `STRATA_ARGON2_WAIT_MS` | 3000 | How long a sign-in waits for a place to hash. |
//! | `STRATA_ARGON2_PER_TENANT` | 2 | Verifications one tenant may run at once, out of that pool. |
//! | `STRATA_BLOCKING_PER_TENANT` | 16 | Store calls one tenant may have running or waiting; over it, 429. |
//! | `STRATA_BLOCKING_QUEUE` | 256 | Store calls all tenants may have running or waiting; over it, 429. |
//! | `STRATA_LAB_WORKERS` | 2 | Threads running Impact Lab projections. |
//! | `STRATA_LAB_QUEUE` | 4 | Projections that may wait for a thread. |
//! | `STRATA_LAB_RUNS_PER_TENANT` | 2 | Projections one tenant may have running or waiting. |
//! | `STRATA_VERIFY_INTERVAL_MS` | 60000 | Least time between two verification runs of a tenant. |
//! | `STRATA_HEADER_TIMEOUT_SECS` | 10 | Time a client has to send its request headers. |
//! | `STRATA_REQUEST_TIMEOUT_SECS` | 30 | Time a request may take before it is answered `408`. |
//! | `STRATA_MAX_CONNECTIONS` | 2048 | Open connections. |
//! | `STRATA_MAX_CONNECTIONS_PER_SOURCE` | 512 | Open connections from one peer address. |

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use strata_backend::app::{Config, FactorPolicy, SealKey, LOG};
use strata_backend::limits::Limits;
use strata_backend::mail::{DevSink, Mailer, Smtp};
use strata_backend::signin::oidc::Federation;
use strata_kernel::{TenantId, Timestamp};
use strata_protocol::log::now_ms;
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::Store;
use strata_storage_sqlite::{default_tenants_dir, SqliteStore};

use strata_runtime::server::{self, env_number as number, env_var as var};

/// The mail transport. An SMTP server is used only when `STRATA_SMTP_URL` names one; otherwise
/// the development sink keeps the messages and nothing is sent. In development the sink also
/// writes each message to `var/mail/*.eml`; outside it, nothing is written to disk.
fn mailer(dev: bool) -> Result<Arc<dyn Mailer>, String> {
    let url = var("STRATA_SMTP_URL", "");
    if !url.is_empty() {
        let from = var("STRATA_MAIL_FROM", "");
        return Ok(Arc::new(Smtp::new(&url, &from)?));
    }
    if dev {
        let dir = PathBuf::from(var("STRATA_MAIL_DIR", "var/mail"));
        return Ok(Arc::new(DevSink::files(dir)));
    }
    LOG.warn(
        "mail.unconfigured",
        "",
        serde_json::json!({ "note": "STRATA_SMTP_URL is not set; mail is kept in memory and not sent" }),
    );
    Ok(Arc::new(DevSink::memory()))
}

/// The tenants' identity providers, from the platform configuration file when one is named.
fn federation() -> Result<Federation, String> {
    let path = var("STRATA_OIDC_CONFIG", "");
    let federation = if path.is_empty() {
        Federation::none()
    } else {
        Federation::from_file(&PathBuf::from(path))?
    };
    let saml = var("STRATA_SAML_CONFIG", "");
    if saml.is_empty() {
        return Ok(federation);
    }
    federation.with_saml_file(&PathBuf::from(saml))
}

/// What the realms ask of second factors. `STRATA_SECOND_FACTOR_REALMS` names the tenants whose
/// policy requires one, comma-separated. TOTP secrets are sealed under a key derived from
/// `STRATA_FACTOR_KEY` when it is set and from the service token otherwise.
fn factors(service_token: &str) -> Result<FactorPolicy, String> {
    let mut policy = FactorPolicy::default();
    for name in var("STRATA_SECOND_FACTOR_REALMS", "")
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        let tenant =
            TenantId::new(name).map_err(|e| format!("STRATA_SECOND_FACTOR_REALMS: {name}: {e}"))?;
        policy.realm_required.insert(tenant);
    }
    policy.step_up_age_ms = number("STRATA_STEP_UP_AGE_MS", policy.step_up_age_ms)?;
    policy.recovery_delay_ms = number("STRATA_RECOVERY_DELAY_MS", policy.recovery_delay_ms)?;
    let key = var("STRATA_FACTOR_KEY", "");
    policy.seal_key = SealKey::derive(if key.is_empty() { service_token } else { &key });
    Ok(policy)
}

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
        argon2_per_tenant: limit(
            "STRATA_ARGON2_PER_TENANT",
            defaults.argon2_per_tenant as i64,
        )? as usize,
        blocking_per_tenant: limit(
            "STRATA_BLOCKING_PER_TENANT",
            defaults.blocking_per_tenant as i64,
        )? as usize,
        blocking_queue: limit("STRATA_BLOCKING_QUEUE", defaults.blocking_queue as i64)? as usize,
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
            let tenant_files = match var("STRATA_TENANT_FILES", "").as_str() {
                "" => default_tenants_dir(&path),
                dir => PathBuf::from(dir),
            };
            Arc::new(
                SqliteStore::open_with(&path, &tenant_files, Timestamp(now_ms()))
                    .map_err(|e| e.to_string())?,
            )
        }
        other => return Err(format!("STRATA_STORE: unknown adapter {other:?}")),
    };
    let policy = factors(&service_token)?;
    let cfg = Config {
        addr,
        service_token,
        dev: var("STRATA_DEV", "") == "1",
        tenants_dir: PathBuf::from(var("STRATA_TENANTS", "tenants")),
        lean_dir: PathBuf::from(var("STRATA_LEAN_DIR", "lean")),
        session_ttl_ms: number(
            "STRATA_SESSION_TTL_MS",
            strata_backend::app::DEFAULT_SESSION_TTL_MS,
        )?,
        session_idle_ms: number(
            "STRATA_SESSION_IDLE_MS",
            strata_backend::app::DEFAULT_SESSION_IDLE_MS,
        )?,
        throttle: Default::default(),
        passkey_challenge_ttl_ms: number("STRATA_PASSKEY_CHALLENGE_TTL_MS", 300_000)?,
        link_ttl_ms: number("STRATA_LINK_TTL_MS", 15 * 60_000)?,
        mail: mailer(var("STRATA_DEV", "") == "1")?,
        federation: Arc::new(federation()?),
        factors: policy,
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
