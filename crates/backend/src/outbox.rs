//! The outbox publisher, as the generated machine `arc-outbox-delivery-flow-impl`.
//!
//! The machine is the schedule; this module is the plumbing around it, and is meant as the
//! pattern for the other generated machines the backend will run:
//!
//! - **A System over a machine store.** [`strata_runtime::System`] runs the machine on
//!   [`StorageApiMachineStore`], so its record, queue and timers are committed through the
//!   persistence contract like any other state. The store is a private one: the machine's records
//!   must not become domain events of a tenant.
//! - **A commit hook.** [`Outbox::committed`] is called after a commit (and once at start-up, for
//!   events committed before a stop). It offers the machine `rt-committed`, which sets its sweep
//!   timer (500 ms), and then `arc-ob-publish-now`, the kick.
//! - **An outside dispatcher.** What the machine emits to `notice-queue` leaves the system through
//!   [`Publisher`], whose one action is [`crate::hub::publish_pending`]: the broadcast fan-out to
//!   WebSocket subscribers stays hand-written plumbing.
//! - **A clock.** The sweep is the machine's timer; [`System::run`] delivers it when it is due.
//!
//! An event reaches the hub only through the publisher, which reads the store's outbox, so only
//! after its commit. Without a kick the sweep publishes it 500 ms after the commit.
//!
//! The sweep never ends: the machine has no terminal state, and every return to idle arms the
//! timer again. The first commit hook call (the start-up one) starts it; from then on it publishes
//! what another writer committed to the store (another replica, a command line tool) within
//! 500 ms, with no commit hook of this process.

use crate::app::{App, Shared, LOG};
use crate::hub::publish_pending;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use strata_kernel::TenantId;
use strata_machine_store::StorageApiMachineStore;
use strata_model::machines::arc_outbox_delivery_flow_impl::ArcOutboxDeliveryFlowImpl;
use strata_runtime::telemetry::{Field, JsonLines, Level, Telemetry};
use strata_runtime::{
    DispatchError, Dispatcher, Envelope, HostError, Machine, Member, PurgeReport, Retention,
    RouteTable, Router, Routing, System, SystemClock,
};
use strata_storage_api::memory::MemoryStore;

/// The machine's name.
const MACHINE: &str = ArcOutboxDeliveryFlowImpl::NAME;
/// The key (JSON text) of the one instance: the backend's publisher.
const KEY: &str = "0";
/// The receiver the machine emits to when events are to be published.
const NOTICE_QUEUE: &str = "notice-queue";
/// How often the host looks for due timers and queued events.
const TICK: Duration = Duration::from_millis(50);

/// The events the backend offers the machine from outside.
const ROUTES: RouteTable = &[
    (
        "rt-committed",
        &[Routing::every(ArcOutboxDeliveryFlowImpl::NAME)],
    ),
    (
        "arc-ob-publish-now",
        &[Routing::every(ArcOutboxDeliveryFlowImpl::NAME)],
    ),
];

fn event(id: &str) -> Value {
    json!({ "event": id, "payload": {} })
}

/// The record the instance is set up over: what a commit leaves in `rec-event`, with nothing of
/// the event itself, since the machine reads none of it.
fn base_record() -> Value {
    json!({
        "seq": 0,
        "revision": 0,
        // The record's body is the kernel's event body; the machine never reads it, so any valid
        // one stands in, written by the type itself so its encoding is exactly what decodes.
        "body": serde_json::to_value(strata_kernel::EventBody::TenantCreated { name: String::new() })
            .unwrap_or(Value::Null),
        "actor": "system",
        "correlation": "00000000-0000-0000-0000-000000000000",
        "state": "arc-od-idle-i",
    })
}

/// The dispatcher of the machine's emits: publishes what is waiting in the store's outbox.
pub struct Publisher {
    app: Weak<App>,
    /// One publication at a time, so that two dispatches cannot move the same events twice.
    serial: Arc<Mutex<()>>,
}

impl Publisher {
    pub fn new(app: &Shared) -> Self {
        Publisher {
            app: Arc::downgrade(app),
            serial: Arc::new(Mutex::new(())),
        }
    }
}

impl Dispatcher<Value> for Publisher {
    async fn dispatch(&self, envelope: &Envelope<Value>) -> Result<(), DispatchError> {
        if envelope.receiver.as_str() != NOTICE_QUEUE {
            return Err(DispatchError(format!(
                "no receiver {}",
                envelope.receiver.as_str()
            )));
        }
        let Some(app) = self.app.upgrade() else {
            return Err(DispatchError("the backend has stopped".into()));
        };
        let serial = self.serial.clone();
        app.blocking(move |app| {
            let _turn = serial.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                match publish_pending(app) {
                    Ok(0) => return Ok(()),
                    Ok(_) => {}
                    Err(error) => return Err(DispatchError(error.to_string())),
                }
            }
        })
        .await
        .map_err(|error| DispatchError(error.describe()))?
    }
}

type OutboxSystem = System<StorageApiMachineStore, Publisher>;

/// The publisher's telemetry: every report goes to the log as JSON lines, and the errors are
/// counted so that health can say the publisher is failing.
struct Beat {
    lines: JsonLines,
    errors: AtomicU64,
    last_error_at: AtomicI64,
}

impl Telemetry for Beat {
    fn event(&self, level: Level, name: &str, fields: &[Field<'_>]) {
        if matches!(level, Level::Error) {
            self.errors.fetch_add(1, Ordering::Relaxed);
            self.last_error_at
                .store(strata_protocol::log::now_ms(), Ordering::Relaxed);
        }
        self.lines.event(level, name, fields);
    }

    fn metric(&self, name: &str, value: f64, fields: &[Field<'_>]) {
        self.lines.metric(name, value, fields);
    }

    fn span(&self, name: &str, elapsed: Duration, ok: bool, fields: &[Field<'_>]) {
        self.lines.span(name, elapsed, ok, fields);
    }
}

struct Inner {
    system: OutboxSystem,
    /// One commit hook at a time: arming, offering and delivering are a sequence.
    turn: tokio::sync::Mutex<()>,
    /// Whether a commit also kicks the publisher. Always on; a development request turns it off
    /// so that the sweep can be seen to do its work.
    kick: AtomicBool,
    beat: Arc<Beat>,
    /// The task that runs the host; finished means the publisher has stopped.
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// What the machine store held at the last purge, for the platform diagnostics.
    held: Mutex<Value>,
}

/// The outbox publisher: the machine on its host.
#[derive(Clone)]
pub struct Outbox {
    inner: Arc<Inner>,
}

impl Outbox {
    /// The machine on a System of its own, dispatching into `app`.
    pub fn new(app: &Shared) -> Result<Outbox, String> {
        let store = Arc::new(StorageApiMachineStore::new(
            Arc::new(MemoryStore::new()),
            TenantId::new("outbox_machine").map_err(|e| e.to_string())?,
            Arc::new(SystemClock),
        ));
        let router = Router::new(vec![Member::of::<ArcOutboxDeliveryFlowImpl>()], ROUTES);
        let beat = Arc::new(Beat {
            lines: JsonLines { service: "backend" },
            errors: AtomicU64::new(0),
            last_error_at: AtomicI64::new(0),
        });
        let system = System::new(
            store,
            router,
            Arc::new(Publisher::new(app)),
            Arc::new(SystemClock),
        )
        .map_err(|e| e.to_string())?
        .with_telemetry(beat.clone());
        Ok(Outbox {
            inner: Arc::new(Inner {
                system,
                turn: tokio::sync::Mutex::new(()),
                kick: AtomicBool::new(true),
                beat,
                task: Mutex::new(None),
                held: Mutex::new(Value::Null),
            }),
        })
    }

    /// Run the host for as long as the process lives: it delivers the sweep timer when it is due
    /// and hands what the machine emits to the dispatcher.
    pub fn run(&self) {
        let system = self.inner.system.clone();
        let task = tokio::spawn(async move { system.run(TICK, std::future::pending()).await });
        if let Ok(mut slot) = self.inner.task.lock() {
            *slot = Some(task);
        }
    }

    /// Whether the publisher's task is still running.
    pub fn running(&self) -> bool {
        self.inner
            .task
            .lock()
            .map(|slot| slot.as_ref().is_some_and(|task| !task.is_finished()))
            .unwrap_or(false)
    }

    /// The publisher's condition, for health: whether it runs, and how often its ticks failed and
    /// when last.
    pub fn status(&self) -> serde_json::Value {
        json!({
            "running": self.running(),
            "tick_failures": self.inner.beat.errors.load(Ordering::Relaxed),
            "last_failure_at": self.inner.beat.last_error_at.load(Ordering::Relaxed),
        })
    }

    /// The janitor's one call for the machine store: remove the records of finished instances and
    /// the dead letters that are past retention ([`Retention::default`]), and note what the store
    /// holds afterwards ([`Outbox::held`]). A failure is logged and the next pass tries again.
    pub async fn purge(&self) {
        let system = &self.inner.system;
        match system.purge(Retention::default()).await {
            Ok(report) if report == PurgeReport::default() => {}
            Ok(report) => LOG.info(
                "janitor.purged",
                "",
                json!({
                    "what": "machine_store",
                    "records": report.records,
                    "dead_letters": report.dead_letters,
                }),
            ),
            Err(error) => LOG.error(
                "janitor.purge_failed",
                "",
                json!({ "what": "machine_store", "error": error.to_string() }),
            ),
        }
        match system.diagnostics().await {
            Ok(held) => {
                if let Ok(mut slot) = self.inner.held.lock() {
                    *slot = json!(held);
                }
            }
            Err(error) => LOG.error(
                "janitor.diagnostics_failed",
                "",
                json!({ "what": "machine_store", "error": error.to_string() }),
            ),
        }
    }

    /// What the machine store held at the last purge: instance records, undelivered outbox
    /// messages and dead letters; `null` before the first purge.
    pub fn held(&self) -> Value {
        self.inner
            .held
            .lock()
            .map(|held| held.clone())
            .unwrap_or(Value::Null)
    }

    /// Whether commits kick the publisher.
    pub fn set_kick(&self, kick: bool) {
        self.inner.kick.store(kick, Ordering::Relaxed);
    }

    /// A commit was made: from any thread, without waiting. Does nothing outside a runtime.
    pub fn committed(&self) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let this = self.clone();
        runtime.spawn(async move { this.on_commit().await });
    }

    /// Offer the machine the commit and the kick, and deliver what follows.
    pub async fn on_commit(&self) {
        let _turn = self.inner.turn.lock().await;
        let kick = self.inner.kick.load(Ordering::Relaxed);
        if let Err(error) = self.offer(kick).await {
            LOG.error("outbox.failed", "", json!({ "error": error.to_string() }));
        }
    }

    async fn offer(&self, kick: bool) -> Result<(), HostError> {
        let system = &self.inner.system;
        // One instance serves every commit and nothing is terminal: it returns to idle after each
        // publication and its sweep keeps itself going, so it is set up only when none exists.
        if system.instance(MACHINE, KEY).await?.is_none() {
            system.init(MACHINE, KEY, base_record()).await?;
        }
        system.submit(event("rt-committed")).await?;
        if kick {
            system.submit(event("arc-ob-publish-now")).await?;
        }
        system.tick().await?;
        Ok(())
    }
}
