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

pub mod files;
pub mod hosts;
pub mod limits;
pub mod relay;
pub mod routes;
pub mod signin;
pub mod token;
pub mod webhooks;

use serde_json::json;
use std::collections::BTreeMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
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
    /// Host name (without port) to tenant. A static override: the backend's list of hosts
    /// (see [`hosts`]) adds to it and never replaces an entry of it.
    pub sites: BTreeMap<String, TenantId>,
    /// Origins a browser request may come from, such as `http://127.0.0.1:4100`. The backend's
    /// list adds to these.
    pub origins: Vec<String>,
    /// How often the backend is asked for the hosts it serves.
    pub hosts: hosts::Refresh,
    /// The built frontend: `index.html`, `assets/`, `media/`.
    pub dist: PathBuf,
    pub dev: bool,
    /// The secret request tokens are keyed with. `None` draws a random one at start, which is for
    /// development only: such tokens die with the process and are worthless at another instance.
    pub token_secret: Option<String>,
    /// Proxies in front of this process whose `X-Forwarded-For` is believed (exact addresses).
    /// Empty when browsers connect directly. See [`limits::client_address`].
    pub trusted_proxies: Vec<IpAddr>,
    /// Sockets one session and one source address may hold.
    pub limits: limits::Limits,
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
    /// Per-session and per-source caps on open sockets.
    pub admission: limits::Admission,
    /// `index.html` as built, read once at start.
    pub index: Option<String>,
    /// Issues and checks the request tokens of browsers that have no session.
    pub tokens: token::Issuer,
    /// The hosts and origins learned from the backend, beside `cfg.sites` and `cfg.origins`.
    pub directory: hosts::Directory,
}

impl Bff {
    /// The tenant a host name serves: the static configuration first, then what the backend
    /// has said.
    pub fn site_of(&self, host: &str) -> Option<TenantId> {
        let host = host.to_lowercase();
        self.cfg
            .sites
            .get(&host)
            .cloned()
            .or_else(|| self.directory.site(&host))
    }

    /// Every origin browser requests may come from: configured and learned.
    pub fn origins(&self) -> Vec<String> {
        let mut origins = self.cfg.origins.clone();
        origins.extend(self.directory.origins());
        origins
    }
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
            "trusted_proxies": cfg.trusted_proxies.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "sites": cfg.sites.iter().map(|(host, tenant)| format!("{host}={tenant}")).collect::<Vec<_>>(),
            "frontend_built": index.is_some(),
        }),
    );
    let cfg_token_secret = cfg.token_secret.clone();
    let refresh = cfg.hosts.clone();
    let bff = Arc::new(Bff {
        admission: limits::Admission::new(&cfg.limits),
        cfg,
        http: reqwest::Client::builder()
            .build()
            .map_err(|e| e.to_string())?,
        sockets: AtomicI64::new(0),
        in_flight: AtomicI64::new(0),
        index,
        tokens: match &cfg_token_secret {
            Some(secret) => token::Issuer::from_secret(secret),
            None => token::Issuer::new()?,
        },
        directory: hosts::Directory::new(refresh),
    });
    // The hosts the backend already knows, before the first request; a backend that is not up yet
    // is asked again when a request names a host nobody knows.
    hosts::refresh_if_due(&bff, false).await;
    strata_runtime::server::serve(listener, routes::router(bff), shutdown).await
}
