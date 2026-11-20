//! The Strata backend: the imperative shell around the kernel.
//!
//! It owns the clock, randomness, password hashing, the store and the sockets. It decides
//! nothing about the domain: every command is `load -> kernel -> commit`, every read is
//! `load -> kernel`, and every delivered event has been through the kernel's read decision.

pub mod app;
pub mod auth;
pub mod hosts;
pub mod http;
pub mod hub;
pub mod lab;
pub mod limits;
pub mod mail;
pub mod ops;
pub mod outbox;
pub mod seed;
pub mod service;
pub mod signin;
pub mod ws;

use app::{App, Config, Shared, LOG};
use serde_json::json;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use strata_runtime::{jobs, server};
use strata_storage_api::Store;
use tokio::net::TcpListener;

/// Seed the store, start the background tasks and serve until `shutdown` resolves.
pub async fn run(
    cfg: Config,
    store: Arc<dyn Store>,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), String> {
    let app: Shared = App::new(cfg, store);
    let created = app
        .blocking(seed::seed)
        .await
        .map_err(|e| e.describe())?
        .map_err(|e| format!("seeding failed: {e}"))?;
    let address = listener.local_addr().map_err(|e| e.to_string())?;
    LOG.info(
        "backend.started",
        "",
        json!({
            "address": address.to_string(),
            "dev": app.cfg.dev,
            "store": app.store.capabilities().adapter,
            "seeded": created,
        }),
    );

    // The publisher is the generated outbox machine. Events committed before a stop are still in
    // the store's outbox, so the first commit hook runs now.
    let outbox = outbox::Outbox::new(&app)?;
    let _ = app.hub.outbox.set(outbox.clone());
    outbox.run();
    outbox.committed();
    let janitor = app.clone();
    tokio::spawn(async move {
        jobs::every(Duration::from_secs(600), || async {
            let _ = janitor
                .blocking(|app| {
                    // Counters of failed sign-ins that have run out, and sessions that ended
                    // a day ago.
                    let _ = app.store.purge_throttle(app.now());
                    let _ = app.store.purge_passkey_challenges(app.now());
                    let _ = app.store.purge_sign_in_links(app.now());
                    let _ = app.store.purge_oidc_flows(app.now());
                    let _ = app.store.purge_factor_challenges(app.now());
                    let _ = app.store.purge_saml_requests(app.now());
                    let _ = app.store.purge_saml_assertions(app.now());
                    app.store
                        .purge_sessions(strata_kernel::Timestamp(app.now().0 - 86_400_000))
                })
                .await;
        })
        .await
    });

    server::serve(listener, http::router(app), shutdown).await
}
