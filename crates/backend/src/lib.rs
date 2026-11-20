//! The Strata backend: the imperative shell around the kernel.
//!
//! It owns the clock, randomness, password hashing, the store and the sockets. It decides
//! nothing about the domain: every command is `load -> kernel -> commit`, every read is
//! `load -> kernel`, and every delivered event has been through the kernel's read decision.

pub mod aggregates;
pub mod app;
pub mod auth;
#[cfg(feature = "development")]
pub mod dev;
pub mod erasure;
pub mod hosts;
pub mod http;
pub mod hub;
pub mod lab;
pub mod ledger;
pub mod limits;
pub mod mail;
pub mod operations;
pub mod ops;
pub mod outbound;
pub mod outbox;
pub mod product_segments;
pub mod products;
pub mod provision;
pub mod quota;
pub mod sealing;
pub mod search;
pub mod seed;
pub mod segments;
pub mod service;
pub mod settings;
pub mod signin;
pub mod view_load;
pub mod worker;
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
    // The listener is bound but is not served until seeding is done: a connection made before
    // that waits in the socket's backlog and gets no answer, health included. So nothing is
    // answered by a store that is still being filled, and a tenant that failed to install is
    // already logged and skipped by the time the first request is read.
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
            match janitor.blocking(purge).await {
                Ok(()) => {}
                Err(error) => LOG.error("janitor.failed", "", json!({ "error": error.describe() })),
            }
            if let Some(outbox) = janitor.hub.outbox.get() {
                outbox.purge().await;
            }
        })
        .await
    });

    // The automation worker stops taking leases when the stop is asked for, and the process waits
    // (within the drain deadline) for the jobs it is running to finish and report.
    let (stop_worker, stopping) = tokio::sync::watch::channel(false);
    let automation = worker::spawn(&app, stopping);
    let served = server::serve(listener, http::router(app), async move {
        shutdown.await;
        let _ = stop_worker.send(true);
    })
    .await;
    let _ = tokio::time::timeout(server::drain_deadline(), automation).await;
    served
}

/// One pass of the janitor: counters of failed sign-ins that have run out, challenges, links and
/// flows that lapsed, and sessions that ended a day ago. A failed purge is logged, with its name,
/// and the pass goes on with the others, so that one locked table does not stop the rest.
fn purge(app: &App) {
    let now = app.now();
    let results = [
        ("throttle", app.store.purge_throttle(now)),
        (
            "passkey_challenges",
            app.store.purge_passkey_challenges(now),
        ),
        ("sign_in_links", app.store.purge_sign_in_links(now)),
        ("quota_counters", quota::purge(app, now)),
        ("oidc_flows", app.store.purge_oidc_flows(now)),
        ("factor_challenges", app.store.purge_factor_challenges(now)),
        ("saml_requests", app.store.purge_saml_requests(now)),
        ("saml_assertions", app.store.purge_saml_assertions(now)),
        // Ended jobs after thirty days, ended runs and their steps and declined or settled
        // exception records after ninety; never an unsettled parked or dead-lettered record
        // (OPS-348, D-7703): one call.
        (
            "automation_records",
            app.store
                .purge_automation(
                    now,
                    &strata_storage_api::schedules::AutomationRetention::default(),
                )
                .map(|purged| purged.total()),
        ),
        (
            "sessions",
            app.store
                .purge_sessions(strata_kernel::Timestamp(now.0 - 86_400_000)),
        ),
    ];
    for (what, result) in results {
        match result {
            Ok(0) => {}
            Ok(removed) => LOG.info(
                "janitor.purged",
                "",
                json!({ "what": what, "removed": removed }),
            ),
            Err(error) => LOG.error(
                "janitor.purge_failed",
                "",
                json!({ "what": what, "error": error.to_string() }),
            ),
        }
    }
}
