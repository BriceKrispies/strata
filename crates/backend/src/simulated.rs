//! The backend as the simulator runs it (`strata_simulation::shell`).
//!
//! [`Backend`] starts a process the way [`crate::run`] does, minus the network listener and the
//! background loops: the same [`App`] over the store it is given, the same installation of the
//! configured tenants ([`crate::seed::seed`]), the same outbox publisher ([`Outbox`], the
//! generated machine on its runtime host) and the same first commit hook. Its [`Process`] then
//! offers the simulator exactly the entry points a deployed process runs on its own:
//!
//! - a request: [`crate::ops::run_intent`], the function every request handler calls once the
//!   caller is authenticated;
//! - a worker pass: [`crate::worker::tick`], the pass the running worker makes, with the jobs run
//!   by [`crate::worker::leases`] and [`crate::worker::steps`];
//! - the commit hook and the publisher's timers: [`Outbox::on_commit`] and [`Outbox::sweep`],
//!   which hand what the machine emits to [`crate::hub::publish_pending`];
//! - the outside services: a webhook's transport behind the real outbound gate
//!   ([`crate::outbound::gate::Gate`]), and the mail service behind [`Mailer`].
//!
//! What it replaces, and nothing else: the system clock (the simulator's logical clock, through
//! [`App::set_clock`] and [`Outbox::with_clock`]), the operating system's randomness (a seeded
//! source on the simulator's thread, through [`strata_runtime::entropy`]), the network (a
//! transport and a resolver whose answers the simulation decides) and the mail transport.
//! Neither is reachable from a deployed process: it never calls `set_clock`, never installs an
//! entropy source and builds its gate and mailer from its configuration.

use crate::app::{App, Config, Shared};
use crate::auth::Identity;
use crate::mail::{Mail, MailError, Mailer};
use crate::outbound::gate::TransportError;
use crate::outbound::gate::{Answer as WireAnswer, Exchange, Gate, Policy, Resolve, Transport};
use crate::outbox::Outbox;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use strata_kernel::{
    canon, Context, DomainEvent, Intent, Principal, PrincipalId, SessionId, TenantId, TenantState,
    Timestamp,
};
use strata_protocol::ws::SessionState;
use strata_runtime::{Clock, Time};
use strata_simulation::engine::summarise;
use strata_simulation::journal::{JournalReplay, Recorded};
use strata_simulation::shell::{
    seeded_bytes, Adapter, Adapters, Answer, Deploy, Environment, LogicalClock, Process, Verdict,
};
use strata_storage_api::SessionRecord;
use tokio::sync::broadcast::{error::TryRecvError, Receiver};

/// The backend deployment, configured with the directory of the tenant bundles a process installs
/// when it starts (none: it installs nothing).
pub struct Backend {
    pub tenants_dir: Option<PathBuf>,
}

impl Backend {
    pub fn new(tenants_dir: Option<PathBuf>) -> Self {
        Backend { tenants_dir }
    }
}

/// The logical clock as the runtime's clock.
struct Logical(LogicalClock);

impl Clock for Logical {
    fn now(&self) -> Time {
        Time(self.0.now().0)
    }
}

/// Resolves every name to one public address: the gate judges it as it judges any answer.
struct Resolver;

impl Resolve for Resolver {
    fn resolve(&self, _host: &str, _timeout: Duration) -> Result<Vec<IpAddr>, String> {
        Ok(vec![IpAddr::from([93, 184, 216, 34])])
    }
}

/// The network to a webhook's receiver: each exchange answers, fails or times out as the
/// simulation decides for its delivery.
struct Wire(Arc<dyn Adapters>);

/// A receiver's answer: 200 and a two-byte body.
struct Accepted {
    sent: bool,
}

impl WireAnswer for Accepted {
    fn status(&self) -> u16 {
        200
    }
    fn headers(&self) -> &[(String, String)] {
        &[]
    }
    fn content_length(&self) -> Option<u64> {
        Some(2)
    }
    fn next_chunk(&mut self, _deadline: Instant) -> Result<Option<Vec<u8>>, TransportError> {
        if self.sent {
            Ok(None)
        } else {
            self.sent = true;
            Ok(Some(b"ok".to_vec()))
        }
    }
}

impl Transport for Wire {
    fn open(&self, exchange: &Exchange<'_>) -> Result<Box<dyn WireAnswer>, TransportError> {
        let subject = exchange
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(crate::outbound::signature::HEADER_EVENT))
            .map_or_else(|| exchange.url.to_string(), |(_, value)| value.clone());
        match self.0.call(Adapter::Webhook, &subject) {
            Verdict::Answers => Ok(Box::new(Accepted { sent: false })),
            Verdict::Fails => Err(TransportError::Failed(
                "simulated: the receiver refused the connection".into(),
            )),
            Verdict::TimesOut => Err(TransportError::TimedOut),
        }
    }
}

/// The mail service: each message is taken, refused for now, or not answered in time, as the
/// simulation decides for its address.
struct Post(Arc<dyn Adapters>);

impl std::fmt::Debug for Post {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("simulated mail service")
    }
}

impl Mailer for Post {
    fn send(&self, mail: &Mail) -> Result<(), MailError> {
        match self.0.call(Adapter::Mail, &mail.to) {
            Verdict::Answers => Ok(()),
            Verdict::Fails => Err(MailError(
                "simulated: the mail service refused the message for now".into(),
            )),
            Verdict::TimesOut => Err(MailError(
                "simulated: the mail service did not answer in time".into(),
            )),
        }
    }
}

/// One running backend process.
struct Running {
    app: Shared,
    outbox: Outbox,
    runtime: tokio::runtime::Runtime,
    listeners: Mutex<Vec<Receiver<Arc<crate::hub::Published>>>>,
    /// The seeded randomness of the process, installed on the simulator's thread while the
    /// process runs. Dropped last.
    _entropy: strata_runtime::entropy::Installed,
}

impl Deploy for Backend {
    fn start(&self, env: &Environment) -> Result<Box<dyn Process>, String> {
        let entropy =
            strata_runtime::entropy::on_this_thread(Box::new(seeded_bytes(env.entropy_seed)));
        let cfg = Config {
            addr: "127.0.0.1:0"
                .parse()
                .map_err(|e: std::net::AddrParseError| e.to_string())?,
            service_token: "simulated-service-token-0123456789abcdef".into(),
            dev: false,
            tenants_dir: self.tenants_dir.clone().unwrap_or_default(),
            lean_dir: PathBuf::new(),
            session_ttl_ms: 3_600_000,
            session_idle_ms: 3_600_000,
            throttle: Default::default(),
            passkey_challenge_ttl_ms: 300_000,
            link_ttl_ms: 900_000,
            mail: Arc::new(Post(env.adapters.clone())),
            federation: Arc::new(crate::signin::oidc::Federation::none()),
            factors: Default::default(),
            limits: Default::default(),
        };
        let app = App::new(cfg, env.store.clone());
        app.set_clock(env.clock.reader());
        crate::outbound::install(
            &app,
            Gate::new(
                Arc::new(Resolver),
                Arc::new(Wire(env.adapters.clone())),
                Policy::default(),
            ),
        )?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let outbox = Outbox::with_clock(&app, Arc::new(Logical(env.clock.clone())))?;
        let listeners = env
            .listen
            .iter()
            .map(|tenant| app.hub.events.subscribe(tenant))
            .collect();
        // As `crate::run`: install the configured tenants, then the first commit hook, for what
        // was committed before this process started.
        if env.install && self.tenants_dir.is_some() {
            crate::seed::seed(&app)?;
        }
        runtime.block_on(outbox.on_commit());
        Ok(Box::new(Running {
            app,
            outbox,
            runtime,
            listeners: Mutex::new(listeners),
            _entropy: entropy,
        }))
    }
}

/// A signed-in member of a tenant, as a request handler sees one after authentication.
fn identity(tenant: &TenantId, principal: &PrincipalId) -> Result<Identity, String> {
    Ok(Identity {
        tenant: tenant.clone(),
        principal: Principal::User(principal.clone()),
        session: Some(SessionRecord {
            id: SessionId::new("s-simulated".to_string()).map_err(|e| e.to_string())?,
            token_hash: "simulated".into(),
            csrf_token: "simulated".into(),
            tenant: tenant.clone(),
            principal: principal.clone(),
            display_name: principal.to_string(),
            created_at: Timestamp(0),
            last_seen_at: Timestamp(0),
            expires_at: Timestamp(i64::MAX),
            revoked_at: None,
            authenticated_at: Timestamp(0),
            network: None,
        }),
        state: SessionState::Active,
        correlation: "simulated".into(),
        verified_address: None,
        idempotency_key: None,
        agent: None,
    })
}

impl Process for Running {
    fn request(&self, intent: &Intent, context: &Context) -> Answer {
        match crate::ops::run_intent(&self.app, context, intent, false) {
            Ok(done) => Answer::Committed {
                revision: done.receipt.revision.0,
                delta_hash: canon::hash_of(&done.transition.delta),
                events_hash: canon::hash_of(&done.transition.events),
                events: done.transition.events.iter().map(summarise).collect(),
                output: done.transition.output,
                conflicts: done.conflicts,
                replayed: done.replayed,
            },
            Err(error) => Answer::Refused {
                code: serde_json::to_value(error.0.code)
                    .ok()
                    .and_then(|code| code.as_str().map(str::to_string))
                    .unwrap_or_default(),
                kind: error.0.kind,
                message: error.0.message,
            },
        }
    }

    fn work(&self) -> usize {
        let config = crate::worker::WorkerConfig::from_limits(&self.app.cfg.limits);
        let app = &self.app;
        crate::worker::tick(app, "simulated-worker", &config, &|| app.now()).jobs
    }

    fn committed(&self) -> Result<(), String> {
        self.runtime.block_on(self.outbox.on_commit());
        Ok(())
    }

    fn sweep(&self) -> Result<(), String> {
        self.runtime
            .block_on(self.outbox.sweep())
            .map_err(|e| e.to_string())
    }

    fn heard(&self) -> Vec<DomainEvent> {
        let mut heard = Vec::new();
        let Ok(mut listeners) = self.listeners.lock() else {
            return heard;
        };
        for listener in listeners.iter_mut() {
            loop {
                match listener.try_recv() {
                    Ok(published) => heard.push(published.event.clone()),
                    Err(TryRecvError::Lagged(_)) => continue,
                    Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                }
            }
        }
        heard
    }

    fn state(&self, tenant: &TenantId) -> Result<Option<Arc<TenantState>>, String> {
        self.app
            .store
            .load_complete(tenant)
            .map(|state| state.tenant)
            .map_err(|e| e.to_string())
    }

    fn journal(&self, tenant: &TenantId) -> Result<Vec<Recorded>, String> {
        let mut entries = Vec::new();
        let mut after = 0;
        loop {
            let page = self
                .app
                .store
                .journal_after(tenant, after, 500)
                .map_err(|e| e.to_string())?;
            let Some(last) = page.last() else {
                break;
            };
            after = last.revision.0;
            entries.extend(page.into_iter().map(|e| Recorded {
                revision: e.revision,
                kernel_version: e.kernel_version,
                intent: e.intent,
                context: e.context,
                delta_hash: e.delta_hash,
                events_hash: e.events_hash,
            }));
        }
        Ok(entries)
    }

    fn replay_journal(&self, tenant: &TenantId) -> Result<JournalReplay, String> {
        crate::lab::replay_tenant(&self.app, tenant)
            .map(|(report, _)| report)
            .map_err(|e| e.describe())
    }

    fn operate(
        &self,
        operation: &str,
        tenant: &TenantId,
        principal: &PrincipalId,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let identity = identity(tenant, principal)?;
        match operation {
            "create-webhook-subscription" => {
                crate::outbound::deliveries::create_subscription(&self.app, &identity, body)
                    .map_err(|e| e.describe())
            }
            other => Err(format!("no simulated operation {other}")),
        }
    }
}
