//! `strata-bff`: the browser-facing process. Configuration comes from the environment.
//!
//! | Variable | Default | Meaning |
//! |---|---|---|
//! | `STRATA_BFF_ADDR` | `127.0.0.1:4100` | Listen address. |
//! | `STRATA_BACKEND_URL` | `http://127.0.0.1:4101` | The backend's internal API. |
//! | `STRATA_SERVICE_TOKEN` | none, required | Shared secret presented to the backend. |
//! | `STRATA_SITES` | see below; required unless `STRATA_DEV=1` | `host=tenant` pairs: which tenant each host name serves. |
//! | `STRATA_BFF_TOKEN_SECRET` | the service token outside development, random in development | Secret the request tokens of browsers without a session are keyed with; every instance of one deployment must share it. |
//! | `STRATA_BFF_REQUESTS_PER_MINUTE` | 600 | Page and view requests one source address may make per minute. |
//! | `STRATA_ORIGINS` | derived | Comma-separated origins browser requests may come from. Each is accepted only on the site its host name serves. |
//! | `STRATA_WEB_DIST` | `web/dist` | The built frontend. |
//! | `STRATA_DEV` | unset | `1` exposes the development endpoints. |
//! | `STRATA_TRUSTED_PROXIES` | none | Comma-separated addresses of reverse proxies in front of this process. Only their `X-Forwarded-For` is believed (the rightmost hop that is not itself a trusted proxy is the visitor); from any other peer it is ignored. |
//! | `STRATA_BFF_SOCKETS_PER_SESSION` | 8 | Open sockets one session may hold. |
//! | `STRATA_BFF_SOCKETS_PER_SOURCE` | 32 | Open sockets one source address may hold. |
//! | `STRATA_HEADER_TIMEOUT_SECS` | 10 | Time a client has to send its request headers. |
//! | `STRATA_REQUEST_TIMEOUT_SECS` | 30 | Time a request may take before it is answered `408`. |
//! | `STRATA_MAX_CONNECTIONS` | 2048 | Open connections. |
//! | `STRATA_WRITE_IDLE_TIMEOUT_SECS` | 30 | A connection whose client has not read for this long while this process has bytes to send is dropped. |
//! | `STRATA_HOSTS_MAX_AGE_SECS` | 60 | How old the list of tenant hosts learned from the backend may be before it is fetched again. |
//! | `STRATA_HOSTS_MISS_INTERVAL_SECS` | 5 | The least time between two calls to the backend for its hosts, however many unknown hosts ask. |
//!
//! `STRATA_SITES` and `STRATA_ORIGINS` are a static override: besides them this process learns the
//! hosts of tenants, and their origins, from the backend (`/internal/platform/hosts`), so a tenant
//! created while it runs needs no restart. An entry of the static configuration is never moved to
//! another tenant by what the backend says.
//!
//! The development sites (with `STRATA_DEV=1` and no `STRATA_SITES`) are
//! `127.0.0.1=fieldnotes,localhost=fieldnotes,tidewater.localhost=tidewater`. Outside development
//! an unset `STRATA_SITES` refuses to start: no demo tenant is served by default.
//! The default origins are every site host on the BFF's port and on the Vite port 5173.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use strata_bff::hosts::Refresh;
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

/// The text of `STRATA_SITES`. Only a development start may leave it unset, and then it serves the
/// demonstration sites; any other start refuses, naming the variable.
fn sites_text(dev: bool, given: &str) -> Result<String, String> {
    match (given, dev) {
        ("", true) => Ok(
            "127.0.0.1=fieldnotes,localhost=fieldnotes,tidewater.localhost=tidewater".to_string(),
        ),
        ("", false) => Err(
            "STRATA_SITES must be set to the host=tenant pairs this deployment serves (only a              development start, STRATA_DEV=1, has default sites)"
                .into(),
        ),
        (given, _) => Ok(given.to_string()),
    }
}

/// The secret for request tokens: the one configured, else (outside development) the shared
/// service token, so every instance derives the same key; random in development.
fn token_secret(dev: bool, service_token: &str) -> Option<String> {
    match var("STRATA_BFF_TOKEN_SECRET", "").as_str() {
        "" if dev => None,
        "" => Some(service_token.to_string()),
        given => Some(given.to_string()),
    }
}

fn config() -> Result<Config, String> {
    let addr: SocketAddr = var("STRATA_BFF_ADDR", "127.0.0.1:4100")
        .parse()
        .map_err(|e| format!("STRATA_BFF_ADDR: {e}"))?;
    let service_token = var("STRATA_SERVICE_TOKEN", "");
    if service_token.len() < 24 {
        return Err("STRATA_SERVICE_TOKEN must be set to at least 24 characters".into());
    }
    let dev = var("STRATA_DEV", "") == "1";
    let sites = Config::parse_sites(&sites_text(dev, &var("STRATA_SITES", ""))?)?;
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
        service_token: service_token.clone(),
        sites,
        origins,
        hosts: Refresh::new(
            limit("STRATA_HOSTS_MAX_AGE_SECS", 60)? as i64 * 1000,
            limit("STRATA_HOSTS_MISS_INTERVAL_SECS", 5)? as i64 * 1000,
        ),
        dist: PathBuf::from(var("STRATA_WEB_DIST", "web/dist")),
        dev,
        token_secret: token_secret(dev, &service_token),
        trusted_proxies: trusted_proxies(&var("STRATA_TRUSTED_PROXIES", ""))?,
        limits: Limits {
            sockets_per_session: limit("STRATA_BFF_SOCKETS_PER_SESSION", 8)?,
            sockets_per_source: limit("STRATA_BFF_SOCKETS_PER_SOURCE", 32)?,
            requests_per_minute: limit("STRATA_BFF_REQUESTS_PER_MINUTE", 600)?,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: GWY-1401
    #[test]
    fn only_a_development_start_has_default_sites() {
        assert!(sites_text(true, "").unwrap().contains("fieldnotes"));
        let refusal = sites_text(false, "").unwrap_err();
        assert!(refusal.contains("STRATA_SITES"), "{refusal}");
        assert_eq!(
            sites_text(false, "a.example=blog").unwrap(),
            "a.example=blog"
        );
    }
}
