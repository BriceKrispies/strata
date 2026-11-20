//! The automation worker (component `automation-worker`, WG-4992): it finds the tenants with due
//! jobs, takes leases on their jobs, runs them and reports each outcome.
//!
//! - **Finding work** reads the store's wake index only ([`strata_storage_api::Store::jobs_due`]):
//!   a tenant with nothing due is never opened (SCH-007), and the index holds a tenant id and a
//!   due time and nothing of a job (SCH-008).
//! - **Fairness**: each pass ([`WorkerConfig::tick`]) takes at most [`WorkerConfig::tenants`]
//!   due tenants, the longest due first, and runs at most [`WorkerConfig::budget`] jobs of each.
//!   So a due job of one tenant starts within one pass of the wake index naming its tenant,
//!   however many jobs another tenant has queued, while no more than that many tenants are due
//!   at once (SCH-009).
//! - **Bounded concurrency**: at most [`WorkerConfig::per_tenant`] passes run for one tenant and
//!   [`WorkerConfig::slots`] for all tenants at once, and each runs as blocking work admitted for
//!   its tenant ([`crate::app::App::blocking_for`]): a tenant whose requests already hold their
//!   share of the blocking pool is passed over until a place frees, so the worker never takes the
//!   places of a tenant's requests beyond the admission limits.
//! - **A clean stop**: once the stop signal is set no lease is taken; the jobs being run finish
//!   and report, and [`spawn`]'s task ends.
//!
//! What a job does is [`steps`]; the lease, run and report sequence is [`leases`]; the routes the
//! tenant owner lists runs, steps and exception records through are [`routes`].

pub mod approvals;
pub mod commands;
pub mod fan_out;
pub mod invoke;
pub mod leases;
pub mod recurrence;
pub mod routes;
pub mod saga;
pub mod schedules;
pub mod segment_work;
pub mod steps;

use crate::app::{App, Shared, LOG};
use crate::limits::Limits;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use strata_kernel::{TenantId, Timestamp};
use strata_runtime::limits::Cap;
use tokio::sync::{watch, Semaphore};

/// How the worker paces and bounds itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerConfig {
    /// How often the wake index is read.
    pub tick: Duration,
    /// The most due tenants one pass takes.
    pub tenants: usize,
    /// The most jobs of one tenant one pass runs.
    pub budget: usize,
    /// The most passes that run for one tenant at once.
    pub per_tenant: usize,
    /// The most passes that run at once, all tenants together.
    pub slots: usize,
}

impl WorkerConfig {
    /// The worker's bounds from the admission limits: a quarter of a tenant's blocking places
    /// and a quarter of the blocking queue, at least one each, so that requests keep the rest.
    pub fn from_limits(limits: &Limits) -> Self {
        WorkerConfig {
            tick: Duration::from_millis(250),
            tenants: 256,
            budget: 16,
            per_tenant: (limits.blocking_per_tenant / 4).max(1),
            slots: (limits.blocking_queue / 4).max(1),
        }
    }
}

/// What one pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pass {
    /// The tenants the wake index named.
    pub tenants: usize,
    /// The jobs that ran.
    pub jobs: usize,
}

/// One pass of the worker, synchronously, at the time `clock` gives: every due tenant the wake
/// index names (at most `config.tenants`), each with at most `config.budget` jobs, one tenant
/// after the other. The running worker makes the same pass with the tenants side by side.
pub fn tick(app: &App, worker: &str, config: &WorkerConfig, clock: &dyn Fn() -> Timestamp) -> Pass {
    let stop = AtomicBool::new(false);
    let due = match app.store.jobs_due(clock(), config.tenants) {
        Ok(due) => due,
        Err(error) => {
            LOG.error(
                "worker.due_failed",
                "",
                json!({ "error": error.to_string() }),
            );
            return Pass::default();
        }
    };
    let mut pass = Pass {
        tenants: due.len(),
        jobs: 0,
    };
    for tenant in &due {
        match leases::drain_tenant(app, tenant, worker, config.budget, &stop, clock) {
            Ok(ran) => pass.jobs += ran,
            Err(error) => LOG.error(
                "worker.tenant_failed",
                "",
                json!({ "tenant": tenant.as_str(), "error": error.to_string() }),
            ),
        }
    }
    pass
}

/// The worker of this process: its name, its bounds and its stop flag.
struct Worker {
    id: String,
    config: WorkerConfig,
    per_tenant: Cap,
    slots: Arc<Semaphore>,
    stop: Arc<AtomicBool>,
}

impl Worker {
    /// Start a pass for each due tenant that has room, each on the blocking pool as work of its
    /// tenant.
    async fn dispatch(&self, app: &Shared, tasks: &mut tokio::task::JoinSet<()>) {
        let tenants = self.config.tenants;
        let due: Vec<TenantId> = match app
            .blocking(move |app| app.store.jobs_due(app.now(), tenants))
            .await
        {
            Ok(Ok(due)) => due,
            Ok(Err(error)) => {
                LOG.error(
                    "worker.due_failed",
                    "",
                    json!({ "error": error.to_string() }),
                );
                return;
            }
            Err(error) => {
                LOG.error(
                    "worker.due_failed",
                    "",
                    json!({ "error": error.describe() }),
                );
                return;
            }
        };
        for tenant in due {
            if self.stop.load(Ordering::Relaxed) {
                return;
            }
            let Some(place) = self.per_tenant.acquire(tenant.as_str()) else {
                continue;
            };
            let Ok(slot) = self.slots.clone().try_acquire_owned() else {
                return;
            };
            let app = app.clone();
            let (id, budget, stop) = (self.id.clone(), self.config.budget, self.stop.clone());
            tasks.spawn(async move {
                let _held = (place, slot);
                let name = tenant.as_str().to_string();
                let ran = app
                    .blocking_for(&name, move |app| {
                        leases::drain_tenant(app, &tenant, &id, budget, &stop, &|| app.now())
                    })
                    .await;
                match ran {
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => LOG.error(
                        "worker.tenant_failed",
                        "",
                        json!({ "tenant": name, "error": error.to_string() }),
                    ),
                    // Refused by the admission limits: the tenant is busy; the next pass tries.
                    Err(error) => LOG.info(
                        "worker.tenant_deferred",
                        "",
                        json!({ "tenant": name, "why": error.describe() }),
                    ),
                }
            });
        }
    }
}

/// Run the worker until `stop` turns true (or its sender is dropped). After the stop no lease is
/// taken; the passes in progress finish their current job and report it, and then the task ends.
pub fn spawn(app: &Shared, mut stop: watch::Receiver<bool>) -> tokio::task::JoinHandle<()> {
    let config = WorkerConfig::from_limits(&app.cfg.limits);
    let worker = Worker {
        id: format!("backend-{}", std::process::id()),
        per_tenant: Cap::new(config.per_tenant),
        slots: Arc::new(Semaphore::new(config.slots)),
        stop: Arc::new(AtomicBool::new(false)),
        config,
    };
    let app = app.clone();
    tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        LOG.info(
            "worker.started",
            "",
            json!({ "worker": worker.id, "slots": worker.config.slots, "per_tenant": worker.config.per_tenant }),
        );
        loop {
            if *stop.borrow() {
                break;
            }
            while tasks.try_join_next().is_some() {}
            worker.dispatch(&app, &mut tasks).await;
            tokio::select! {
                _ = tokio::time::sleep(worker.config.tick) => {}
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() {
                        break;
                    }
                }
            }
        }
        worker.stop.store(true, Ordering::Relaxed);
        while tasks.join_next().await.is_some() {}
        LOG.info("worker.stopped", "", json!({ "worker": worker.id }));
    })
}
