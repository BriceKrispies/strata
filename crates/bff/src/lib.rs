//! The Strata backend-for-frontend.
//!
//! The browser talks to this process and to nothing else. It turns a browser request into a
//! request to the backend's internal API, and decides nothing about the domain itself:
//!
//! - **Site.** The tenant is looked up from the request's host name in this process's own
//!   configuration. No request parameter, header or body field can name a tenant.
//! - **Session.** The session token lives in an `__Host-` prefixed, `Secure`, `HttpOnly`,
//!   `SameSite=Strict` cookie. This process forwards it; the backend holds the session.
//! - **CSRF.** A state-changing request, sign-in and sign-out included, must come from an
//!   allowed origin and carry the request token of its session in `x-csrf-token`. A browser
//!   that has no session yet is given a request token by this process with its session reply
//!   (see `token`), and presents that.
//! - **Views.** The backend returns kernel-typed, already authorized rows; this process projects
//!   them into the plain view model the browser renders.
//! - **WebSocket.** The origin and the request token (`?csrf=`) are checked at upgrade, frames
//!   from the browser are validated against protocol v1, and both directions go through bounded
//!   queues.

pub mod relay;
pub mod routes;
pub mod token;

use serde_json::json;
use std::collections::BTreeMap;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::AtomicI64;
use std::sync::Arc;
use strata_kernel::TenantId;
use strata_protocol::log::Logger;
use tokio::net::TcpListener;

pub const LOG: Logger = Logger { service: "bff" };
/// The session cookie. The `__Host-` prefix makes a browser accept it only with `Secure`,
/// `Path=/` and no `Domain`, and keeps another host from setting a cookie of this name.
pub const COOKIE: &str = "__Host-strata_sid";

#[derive(Clone, Debug)]
pub struct Config {
    pub addr: SocketAddr,
    /// Base URL of the backend's internal API, such as `http://127.0.0.1:4101`.
    pub backend: String,
    pub service_token: String,
    /// Host name (without port) to tenant.
    pub sites: BTreeMap<String, TenantId>,
    /// Origins a browser request may come from, such as `http://127.0.0.1:4100`.
    pub origins: Vec<String>,
    /// The built frontend: `index.html`, `assets/`, `media/`.
    pub dist: PathBuf,
    pub dev: bool,
}

impl Config {
    /// Parse `host=tenant,host=tenant`.
    pub fn parse_sites(text: &str) -> Result<BTreeMap<String, TenantId>, String> {
        text.split(',')
            .filter(|entry| !entry.trim().is_empty())
            .map(|entry| {
                let (host, tenant) = entry
                    .trim()
                    .split_once('=')
                    .ok_or_else(|| format!("bad site entry {entry:?}"))?;
                Ok((
                    host.to_lowercase(),
                    TenantId::new(tenant).map_err(|e| e.to_string())?,
                ))
            })
            .collect()
    }
}

pub struct Bff {
    pub cfg: Config,
    pub http: reqwest::Client,
    pub sockets: AtomicI64,
    pub in_flight: AtomicI64,
    /// `index.html` as built, read once at start.
    pub index: Option<String>,
    /// Issues and checks the request tokens of browsers that have no session.
    pub tokens: token::Issuer,
}

pub type Shared = Arc<Bff>;

/// Serve until `shutdown` resolves.
pub async fn run(
    cfg: Config,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), String> {
    let index = std::fs::read_to_string(cfg.dist.join("index.html")).ok();
    let address = listener.local_addr().map_err(|e| e.to_string())?;
    LOG.info(
        "bff.started",
        "",
        json!({
            "address": address.to_string(),
            "backend": cfg.backend,
            "sites": cfg.sites.iter().map(|(host, tenant)| format!("{host}={tenant}")).collect::<Vec<_>>(),
            "frontend_built": index.is_some(),
        }),
    );
    let bff = Arc::new(Bff {
        cfg,
        http: reqwest::Client::builder()
            .build()
            .map_err(|e| e.to_string())?,
        sockets: AtomicI64::new(0),
        in_flight: AtomicI64::new(0),
        index,
        tokens: token::Issuer::new()?,
    });
    strata_runtime::server::serve(listener, routes::router(bff), shutdown).await
}
