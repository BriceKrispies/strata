//! `strata-bff`: the browser-facing process. Configuration comes from the environment.
//!
//! | Variable | Default | Meaning |
//! |---|---|---|
//! | `STRATA_BFF_ADDR` | `127.0.0.1:4100` | Listen address. |
//! | `STRATA_BACKEND_URL` | `http://127.0.0.1:4101` | The backend's internal API. |
//! | `STRATA_SERVICE_TOKEN` | none, required | Shared secret presented to the backend. |
//! | `STRATA_SITES` | see below | `host=tenant` pairs: which tenant each host name serves. |
//! | `STRATA_ORIGINS` | derived | Comma-separated origins browser requests may come from. Each is accepted only on the site its host name serves. |
//! | `STRATA_WEB_DIST` | `web/dist` | The built frontend. |
//! | `STRATA_DEV` | unset | `1` exposes the development endpoints. |
//! | `STRATA_TRUSTED_PROXIES` | none | Comma-separated addresses of reverse proxies in front of this process. Only their `X-Forwarded-For` is believed (the rightmost hop that is not itself a trusted proxy is the visitor); from any other peer it is ignored. |
//! | `STRATA_BFF_SOCKETS_PER_SESSION` | 8 | Open sockets one session may hold. |
//! | `STRATA_BFF_SOCKETS_PER_SOURCE` | 32 | Open sockets one source address may hold. |
//! | `STRATA_HEADER_TIMEOUT_SECS` | 10 | Time a client has to send its request headers. |
//! | `STRATA_REQUEST_TIMEOUT_SECS` | 30 | Time a request may take before it is answered `408`. |
//! | `STRATA_MAX_CONNECTIONS` | 2048 | Open connections. |
//!
//! The default sites are `127.0.0.1=fieldnotes,localhost=fieldnotes,tidewater.localhost=tidewater`.
//! The default origins are every site host on the BFF's port and on the Vite port 5173.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use strata_bff::limits::Limits;
use strata_bff::Config;

use strata_runtime::server::{self, env_var as var};

/// `STRATA_TRUSTED_PROXIES`: a comma-separated list of addresses.
fn trusted_proxies(text: &str) -> Result<Vec<IpAddr>, String> {
    text.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            entry
                .parse()
                .map_err(|_| format!("STRATA_TRUSTED_PROXIES: {entry:?} is not an IP address"))
        })
        .collect()
}

/// A limit from the environment: a whole number, at least one.
fn limit(name: &str, default: usize) -> Result<usize, String> {
    let value = server::env_number(name, default as i64)?;
    usize::try_from(value)
        .ok()
        .filter(|v| *v > 0)
        .ok_or_else(|| format!("{name} must be at least 1"))
}

fn config() -> Result<Config, String> {
    let addr: SocketAddr = var("STRATA_BFF_ADDR", "127.0.0.1:4100")
        .parse()
        .map_err(|e| format!("STRATA_BFF_ADDR: {e}"))?;
    let service_token = var("STRATA_SERVICE_TOKEN", "");
    if service_token.len() < 24 {
        return Err("STRATA_SERVICE_TOKEN must be set to at least 24 characters".into());
    }
    let sites = Config::parse_sites(&var(
        "STRATA_SITES",
        "127.0.0.1=fieldnotes,localhost=fieldnotes,tidewater.localhost=tidewater",
    ))?;
    let origins = match var("STRATA_ORIGINS", "").as_str() {
        "" => sites
            .keys()
            .flat_map(|host| {
                [
                    format!("http://{host}:{}", addr.port()),
                    format!("http://{host}:5173"),
                ]
            })
            .collect(),
        given => given
            .split(',')
            .map(|origin| origin.trim().to_string())
            .collect(),
    };
    Ok(Config {
        addr,
        backend: var("STRATA_BACKEND_URL", "http://127.0.0.1:4101")
            .trim_end_matches('/')
            .to_string(),
        service_token,
        sites,
        origins,
        dist: PathBuf::from(var("STRATA_WEB_DIST", "web/dist")),
        dev: var("STRATA_DEV", "") == "1",
        trusted_proxies: trusted_proxies(&var("STRATA_TRUSTED_PROXIES", ""))?,
        limits: Limits {
            sockets_per_session: limit("STRATA_BFF_SOCKETS_PER_SESSION", 8)?,
            sockets_per_source: limit("STRATA_BFF_SOCKETS_PER_SOURCE", 32)?,
        },
    })
}

#[tokio::main]
async fn main() {
    let result = async {
        let cfg = config()?;
        let listener = server::bind(cfg.addr).await?;
        strata_bff::run(cfg, listener, server::interrupted()).await
    }
    .await;
    server::exit_on_error("strata-bff", result);
}
