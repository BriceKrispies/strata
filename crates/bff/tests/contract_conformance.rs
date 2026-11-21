//! The contract conformance suite (WG-5182): every contract that is shipped and names a
//! `bffRoute` is driven through the real BFF and the real backend over loopback, on a SQLite store,
//! by the tests `specc conformance` generates from the specification
//! (`conformance/generated/contracts.rs`) with the harness of `conformance/harness.rs`.
//!
//! The target is the development stack in-process: the backend seeds the tenants under `tenants/`
//! (fieldnotes at 127.0.0.1, tidewater at localhost), whose development accounts sign in with a
//! password. The persons the checks sign in as, and the roles each holds, are read from those files:
//! `accounts.dev.json` (login, password, principal) and `tenant.json` (the owner, the members'
//! roles, the teams' roles and members).
//!
//!   specc conformance --spec specifications/spec.jsonc --out crates/bff/tests/conformance/generated
//!   cargo test -p strata-bff --test contract_conformance -- --nocapture   (the skips are printed)

#[allow(dead_code)]
#[path = "conformance/harness.rs"]
mod harness;

#[allow(dead_code)]
#[path = "conformance/generated/contracts.rs"]
mod generated;

use harness::{Persona, Site, Target};
use serde_json::Value as Json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use strata_bff::Config;
use strata_storage_sqlite::SqliteStore;

const TOKEN: &str = "conformance-service-token-0123456789";

/// The sites of the development stack: the one checked, then the other tenant.
const SITES: [(&str, &str); 2] = [("fieldnotes", "127.0.0.1"), ("tidewater", "localhost")];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &Path) -> Json {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()))
}

/// The persons of a tenant who sign in with a password: the first login of each principal of
/// `accounts.dev.json`, with the roles `tenant.json` gives the principal (`owner` for the owner,
/// its member roles and the roles of its teams).
fn personas(site: usize, tenant: &str) -> Vec<Persona> {
    let dir = root().join("tenants").join(tenant);
    let accounts = read(&dir.join("accounts.dev.json"));
    let definition = read(&dir.join("tenant.json"));
    let mut out: Vec<(String, Persona)> = Vec::new();
    for account in accounts.as_array().into_iter().flatten() {
        let (Some(login), Some(password), Some(principal)) = (
            account["login"].as_str(),
            account["password"].as_str(),
            account["principal"].as_str(),
        ) else {
            continue;
        };
        if out.iter().any(|(held, _)| held == principal) {
            continue;
        }
        let mut roles: Vec<String> = Vec::new();
        if definition["owner"].as_str() == Some(principal) {
            roles.push("owner".into());
        }
        for member in definition["members"].as_array().into_iter().flatten() {
            if member["principal"].as_str() == Some(principal) {
                for role in member["roles"].as_array().into_iter().flatten() {
                    if let Some(role) = role.as_str() {
                        roles.push(role.to_string());
                    }
                }
            }
        }
        for team in definition["teams"].as_array().into_iter().flatten() {
            let belongs = team["members"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|m| m.as_str() == Some(principal));
            if belongs {
                for role in team["roles"].as_array().into_iter().flatten() {
                    if let Some(role) = role.as_str() {
                        roles.push(role.to_string());
                    }
                }
            }
        }
        roles.sort();
        roles.dedup();
        out.push((
            principal.to_string(),
            Persona {
                site,
                login: login.to_string(),
                password: password.to_string(),
                roles,
            },
        ));
    }
    out.into_iter().map(|(_, persona)| persona).collect()
}

/// A backend on a SQLite store and a BFF in front of it, in development mode, as `edge.rs` starts
/// them; ready when the BFF answers `/api/session`.
pub async fn start_target() -> Target {
    let root = root();
    let backend_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_addr = backend_listener.local_addr().unwrap();
    let backend_cfg = strata_backend::app::Config {
        addr: backend_addr,
        service_token: TOKEN.into(),
        dev: true,
        tenants_dir: root.join("tenants"),
        // A directory with no Lean project: verify-lab-proofs answers its call, and the build it
        // would start in the background finds nothing to build, so the suite never runs `lake`.
        lean_dir: PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("conformance-no-lean"),
        session_ttl_ms: 3_600_000,
        session_idle_ms: 3_600_000,
        passkey_challenge_ttl_ms: 300_000,
        link_ttl_ms: 900_000,
        mail: Arc::new(strata_backend::mail::DevSink::memory()),
        federation: Arc::new(strata_backend::signin::oidc::Federation::none()),
        factors: Default::default(),
        limits: Default::default(),
        throttle: Default::default(),
    };
    let store =
        SqliteStore::open_in_memory(strata_kernel::Timestamp(strata_protocol::log::now_ms()))
            .expect("an in-memory SQLite store");
    tokio::spawn(strata_backend::run(
        backend_cfg,
        Arc::new(store),
        backend_listener,
        std::future::pending(),
    ));

    let dist = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("conformance-dist-{}", std::process::id()));
    std::fs::create_dir_all(dist.join("assets")).unwrap();
    std::fs::write(
        dist.join("index.html"),
        "<!doctype html><html lang=\"en\"><head><title>Strata</title><!--strata:head--></head><body><div id=\"app\"></div><!--strata:bootstrap--></body></html>",
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let cfg = Config {
        addr: listener.local_addr().unwrap(),
        backend: format!("http://{backend_addr}"),
        service_token: TOKEN.into(),
        sites: Config::parse_sites(
            &SITES
                .iter()
                .map(|(tenant, host)| format!("{host}={tenant}"))
                .collect::<Vec<_>>()
                .join(","),
        )
        .unwrap(),
        origins: SITES
            .iter()
            .map(|(_, host)| format!("http://{host}:{port}"))
            .collect(),
        hosts: Default::default(),
        dist,
        dev: true,
        token_secret: None,
        trusted_proxies: Vec::new(),
        limits: Default::default(),
    };
    tokio::spawn(strata_bff::run(cfg, listener, std::future::pending()));
    let sites: Vec<Site> = SITES
        .iter()
        .map(|(tenant, host)| Site {
            tenant: (*tenant).to_string(),
            host: (*host).to_string(),
        })
        .collect();
    let personas = SITES
        .iter()
        .enumerate()
        .flat_map(|(i, (tenant, _))| personas(i, tenant))
        .collect();
    let target = Target::new(port, sites, personas);
    for _ in 0..400 {
        let ready = target
            .http
            .get(format!("http://127.0.0.1:{port}/api/session"))
            .send()
            .await
            .is_ok_and(|r| r.status().as_u16() == 200);
        if ready {
            return target;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the BFF and backend did not start");
}

/// Every contract the generated suite covers is shipped and has a route, and the suite has the
/// tables the harness reads.
#[test]
fn the_generated_suite_covers_contracts() {
    assert!(!generated::COVERED.is_empty());
    assert!(generated::SPEC.error_body.is_some());
    assert!(!generated::ROLES.is_empty());
}
