//! `strata-bff`: the browser-facing process. Configuration comes from the environment.
//!
//! | Variable | Default | Meaning |
//! |---|---|---|
//! | `STRATA_BFF_ADDR` | `127.0.0.1:4100` | Listen address. |
//! | `STRATA_BACKEND_URL` | `http://127.0.0.1:4101` | The backend's internal API. |
//! | `STRATA_SERVICE_TOKEN` | none, required | Shared secret presented to the backend. |
//! | `STRATA_SITES` | see below | `host=tenant` pairs: which tenant each host name serves. |
//! | `STRATA_ORIGINS` | derived | Comma-separated origins browser requests may come from. |
//! | `STRATA_WEB_DIST` | `web/dist` | The built frontend. |
//! | `STRATA_DEV` | unset | `1` exposes the development endpoints. |
//!
//! The default sites are `127.0.0.1=fieldnotes,localhost=fieldnotes,tidewater.localhost=tidewater`.
//! The default origins are every site host on the BFF's port and on the Vite port 5173.

use std::net::SocketAddr;
use std::path::PathBuf;
use strata_bff::Config;

fn var(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
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
    })
}

#[tokio::main]
async fn main() {
    let result = async {
        let cfg = config()?;
        let listener = tokio::net::TcpListener::bind(cfg.addr)
            .await
            .map_err(|e| format!("{}: {e}", cfg.addr))?;
        let shutdown = async {
            let _ = tokio::signal::ctrl_c().await;
        };
        strata_bff::run(cfg, listener, shutdown).await
    }
    .await;
    if let Err(problem) = result {
        eprintln!("strata-bff: {problem}");
        std::process::exit(1);
    }
}
