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
//! | `STRATA_PASSKEY_CHALLENGE_TTL_MS` | 5 minutes | How long a passkey challenge can be answered. |
//! | `STRATA_LINK_TTL_MS` | 15 minutes | How long an emailed sign-in link is accepted. |
//! | `STRATA_SMTP_URL` | unset | An SMTP server (`smtps://user:password@host:465`). Unset: no mail is sent. |
//! | `STRATA_MAIL_FROM` | none | The sender address, required with `STRATA_SMTP_URL`. |
//! | `STRATA_MAIL_DIR` | `var/mail` | Where the development sink writes `.eml` files (development only). |
//! | `STRATA_OIDC_CONFIG` | unset | A JSON file of each tenant's OpenID Connect providers. Unset: no tenant has one. |
//! | `STRATA_SAML_CONFIG` | unset | A JSON file of each tenant's SAML providers. Unset: no tenant has one. |

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use strata_backend::app::{Config, LOG};
use strata_backend::mail::{DevSink, Mailer, Smtp};
use strata_backend::signin::oidc::Federation;
use strata_kernel::Timestamp;
use strata_protocol::log::now_ms;
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::Store;
use strata_storage_sqlite::SqliteStore;

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
        throttle: Default::default(),
        passkey_challenge_ttl_ms: number("STRATA_PASSKEY_CHALLENGE_TTL_MS", 300_000)?,
        link_ttl_ms: number("STRATA_LINK_TTL_MS", 15 * 60_000)?,
        mail: mailer(var("STRATA_DEV", "") == "1")?,
        federation: Arc::new(federation()?),
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
