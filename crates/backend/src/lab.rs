//! Impact Lab: projections, journal replay and verification runs.
//!
//! Everything here reads the store and never writes it. A projection loads a complete snapshot,
//! hands it to the simulator, and the simulator works on that copy with the same kernel the
//! backend uses. Live tenant state cannot be changed from this module: it calls no `commit`.
//!
//! A projection that asks for faults also runs its schedule through the deployment's own code
//! ([`crate::simulated::Backend`], driven by `strata_simulation::shell`): a fresh in-memory store
//! made into a copy of the tenant by sending it the tenant's recorded history, then the schedule
//! through the request path, the worker and the publisher, with the store faults a deployment
//! meets. What the Lab reports of faults is read from that run's trace and from nothing else.

use crate::app::{ApiError, App, Shared, LOG};
use crate::auth::Identity;
use crate::hub::RunMessage;
use crate::limits::Limits;
use serde::Deserialize;
use serde_json::{json, Value as Json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use strata_kernel::ext::*;
use strata_kernel::{ExistingData, Principal, PrincipalId, TenantId, TenantState, World};
use strata_protocol::api::ErrorCode;
use strata_simulation::engine::{replay as replay_trace, Scenario, Scheduled};
use strata_simulation::faults::StoreFaults;
use strata_simulation::journal::{replay_journal_with, JournalReplay, Recorded};
use strata_simulation::project::{project, Projection, Proposal};
use strata_simulation::shell::{
    run_shell_after, Answer, Faults as ShellFaults, ShellRun, ShellScenario,
};
use strata_simulation::workload::generate;
use strata_substrate::proposal;
use strata_verification::evidence::Class;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunRequest {
    /// The proposed change, in the proposal syntax.
    pub change: String,
    /// `reject` or `grandfather`.
    pub existing: String,
    /// `generated` or `journal`.
    pub source: String,
    #[serde(default)]
    pub steps: Option<usize>,
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default)]
    pub faults: bool,
    /// Apply the change before this step of the schedule.
    #[serde(default)]
    pub apply_at: Option<usize>,
    /// The revision of the tenant the projection is taken at (AGT-422). The projection starts
    /// from the tenant's recorded history up to and including this revision: the live state when
    /// it is the current revision, otherwise the journal replayed to it. Absent means the
    /// current revision.
    #[serde(default)]
    pub revision: Option<u64>,
    /// Refuse with a conflict that names the current revision when the tenant is no longer at
    /// `revision`, instead of projecting against the past. The Lab page sets it, so that a
    /// projection shown next to the current site is never one of an older site.
    #[serde(default)]
    pub require_current: bool,
}

/// What a result says about how long it is kept.
const RETENTION: &str = "Results are held in this backend process only. They are lost when it restarts, and a site keeps its last 12 runs.";

/// Journal entries are read in pages of this many.
const JOURNAL_PAGE: usize = 5_000;

struct Run {
    tenant: TenantId,
    principal: PrincipalId,
    status: &'static str,
    started_at: i64,
    summary: Json,
    result: Option<Json>,
    cancel: Arc<AtomicBool>,
}

type Job = Box<dyn FnOnce() + Send + 'static>;

/// A fixed number of worker threads and a bounded queue in front of them. Work that finds the
/// queue full is refused, not started on a thread of its own.
pub struct Pool {
    queue: SyncSender<Job>,
}

impl Pool {
    pub fn new(name: &str, workers: usize, queue: usize) -> Self {
        let (sender, receiver) = sync_channel::<Job>(queue);
        let receiver = Arc::new(Mutex::new(receiver));
        for n in 0..workers.max(1) {
            let receiver = receiver.clone();
            let _ = std::thread::Builder::new()
                .name(format!("{name}-{n}"))
                .spawn(move || loop {
                    let job = match receiver.lock() {
                        Ok(receiver) => receiver.recv(),
                        Err(_) => return,
                    };
                    match job {
                        // A job that panics must not take its worker with it.
                        Ok(job) => {
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                        }
                        // The pool was dropped: no more work will come.
                        Err(_) => return,
                    }
                });
        }
        Pool { queue: sender }
    }

    /// Queue `job`, or refuse it when every worker is busy and the queue is full.
    pub fn submit(&self, job: impl FnOnce() + Send + 'static) -> Result<(), ApiError> {
        self.queue
            .try_send(Box::new(job))
            .map_err(|error| match error {
                TrySendError::Full(_) | TrySendError::Disconnected(_) => ApiError::new(
                    ErrorCode::Throttled,
                    "The Impact Lab is busy. Try again in a moment.",
                ),
            })
    }
}

/// Verification runs: which tenants have one going and when each last started one.
#[derive(Default)]
struct Verifying {
    running: BTreeSet<String>,
    last_started: BTreeMap<String, i64>,
}

pub struct Lab {
    runs: Mutex<BTreeMap<String, Run>>,
    order: Mutex<Vec<String>>,
    /// The last verification report of each tenant that asked for one.
    verification: Mutex<BTreeMap<String, Json>>,
    verifying: Mutex<Verifying>,
    /// How many finished runs of each tenant were dropped to keep to `KEPT_RUNS`, since this
    /// process started.
    evicted: Mutex<BTreeMap<String, u64>>,
    /// Projections.
    pool: Pool,
    /// Lean builds: one at a time, whichever tenant asked.
    verify_pool: Pool,
    runs_per_tenant: usize,
    verify_interval_ms: i64,
    /// The keys callers gave to start a projection, by tenant, person and key (D-6595).
    keys: Mutex<BTreeMap<KeyScope, KeyedStart>>,
}

/// The tenant, the person and the key a start was made under.
type KeyScope = (String, String, String);

/// How long a key is honoured: 24 hours (D-6595).
const KEY_HELD_MS: i64 = 86_400_000;

/// What a key stands for.
enum KeyedStart {
    /// The start is being made; the request is held as the text the repeat is compared with.
    Starting(String),
    /// The start was made at `at` and answered `answer`.
    Started {
        request: String,
        answer: Json,
        at: i64,
    },
}

fn key_mismatch() -> ApiError {
    let mut error = ApiError::new(
        ErrorCode::Invalid,
        "This idempotency key was used for a projection with other input.",
    );
    error.0.kind = Some("IdempotencyMismatch".into());
    error
}

const KEPT_RUNS: usize = 12;

impl Default for Lab {
    fn default() -> Self {
        Self::new()
    }
}

impl Lab {
    pub fn new() -> Self {
        Self::with_limits(&Limits::default())
    }

    pub fn with_limits(limits: &Limits) -> Self {
        Lab {
            runs: Mutex::default(),
            order: Mutex::default(),
            verification: Mutex::default(),
            verifying: Mutex::default(),
            evicted: Mutex::default(),
            pool: Pool::new("lab", limits.lab_workers, limits.lab_queue),
            verify_pool: Pool::new("lab-verify", 1, 2),
            runs_per_tenant: limits.lab_runs_per_tenant,
            verify_interval_ms: limits.verify_interval_ms,
            keys: Mutex::default(),
        }
    }

    /// Take the key for a start. `Ok(Some(answer))` is a repeat of a start already made with the
    /// same request: its first answer. The same key with another request is refused as an
    /// idempotency mismatch, and a repeat that arrives while the first is still being made is a
    /// conflict. `Ok(None)` means the caller makes the start and then calls [`Lab::settle_key`].
    fn claim_key(
        &self,
        scope: &KeyScope,
        request: &str,
        now: i64,
    ) -> Result<Option<Json>, ApiError> {
        let mut keys = self.keys.lock().map_err(|_| {
            ApiError::hidden(
                ErrorCode::Internal,
                "the table of idempotency keys is poisoned",
            )
        })?;
        keys.retain(|_, held| match held {
            KeyedStart::Started { at, .. } => now - *at < KEY_HELD_MS,
            KeyedStart::Starting(_) => true,
        });
        match keys.get(scope) {
            Some(KeyedStart::Started {
                request: first,
                answer,
                ..
            }) if first == request => Ok(Some(answer.clone())),
            Some(KeyedStart::Starting(first)) if first == request => Err(ApiError::new(
                ErrorCode::Conflict,
                "A projection with this idempotency key is being started. Try again.",
            )),
            Some(_) => Err(key_mismatch()),
            None => {
                keys.insert(scope.clone(), KeyedStart::Starting(request.to_string()));
                Ok(None)
            }
        }
    }

    /// Record how a start made under a key ended: an answer is kept for a repeat, and a refusal
    /// gives the key back, since nothing was started.
    fn settle_key(&self, scope: &KeyScope, request: &str, answer: Option<&Json>, now: i64) {
        if let Ok(mut keys) = self.keys.lock() {
            match answer {
                Some(answer) => {
                    keys.insert(
                        scope.clone(),
                        KeyedStart::Started {
                            request: request.to_string(),
                            answer: answer.clone(),
                            at: now,
                        },
                    );
                }
                None => {
                    keys.remove(scope);
                }
            }
        }
    }

    /// The tenant's last verification report.
    pub fn verification_of(&self, tenant: &TenantId) -> Option<Json> {
        self.verification
            .lock()
            .ok()
            .and_then(|reports| reports.get(tenant.as_str()).cloned())
    }

    fn remove(&self, id: &str) {
        if let (Ok(mut runs), Ok(mut order)) = (self.runs.lock(), self.order.lock()) {
            runs.remove(id);
            order.retain(|kept| kept != id);
        }
    }

    /// Recent runs for diagnostics: what was asked and how it ended, not the full results.
    pub fn recent(&self) -> Vec<Json> {
        let (Ok(runs), Ok(order)) = (self.runs.lock(), self.order.lock()) else {
            return Vec::new();
        };
        order
            .iter()
            .filter_map(|id| runs.get(id).map(|run| (id, run)))
            .map(|(id, run)| json!({ "id": id, "tenant": run.tenant.as_str(), "started_by": run.principal.as_str(), "status": run.status, "started_at": run.started_at, "summary": run.summary }))
            .collect()
    }

    /// Cancel a running run. Only the person who started it, or someone who may change the
    /// tenant's schema (`may_manage_schema`), may cancel it; for anyone else the run is not there.
    pub fn cancel(&self, id: &str, identity: &Identity, may_manage_schema: bool) -> bool {
        let Ok(runs) = self.runs.lock() else {
            return false;
        };
        let starter = |run: &Run| matches!(&identity.principal, Principal::User(who) if *who == run.principal);
        match runs.get(id) {
            Some(run)
                if run.tenant == identity.tenant
                    && run.status == "running"
                    && !identity.anonymous()
                    && (starter(run) || may_manage_schema) =>
            {
                run.cancel.store(true, Ordering::Relaxed);
                true
            }
            _ => false,
        }
    }

    pub fn result(&self, id: &str, identity: &Identity) -> Option<Json> {
        let runs = self.runs.lock().ok()?;
        let run = runs.get(id).filter(|run| run.tenant == identity.tenant)?;
        Some(
            json!({ "id": id, "status": run.status, "summary": run.summary, "result": run.result, "retention": RETENTION }),
        )
    }

    /// How many of the tenant's runs this process has dropped to keep to `KEPT_RUNS`.
    pub fn evicted_of(&self, tenant: &TenantId) -> u64 {
        self.evicted
            .lock()
            .ok()
            .and_then(|evicted| evicted.get(tenant.as_str()).copied())
            .unwrap_or(0)
    }

    #[cfg(test)]
    fn insert(&self, id: &str, run: Run) {
        let _ = self.insert_within(id, run, usize::MAX);
    }

    /// Record a new run unless its tenant already has `most` running (or waiting for a worker).
    fn insert_within(&self, id: &str, run: Run, most: usize) -> Result<(), ApiError> {
        if let (Ok(mut runs), Ok(mut order)) = (self.runs.lock(), self.order.lock()) {
            let active = runs
                .values()
                .filter(|other| other.tenant == run.tenant && other.status == "running")
                .count();
            if active >= most {
                return Err(ApiError::new(
                    ErrorCode::Throttled,
                    "This site already has as many projections running as it may. Wait for one to finish or cancel it.",
                ));
            }
            runs.insert(id.to_string(), run);
            order.push(id.to_string());
            // Each tenant keeps its own last KEPT_RUNS runs; a run still running is never evicted,
            // and one tenant's runs never push out another's.
            let tenant = runs[id].tenant.clone();
            loop {
                let kept = order
                    .iter()
                    .filter(|kept| runs.get(*kept).is_some_and(|r| r.tenant == tenant))
                    .count();
                if kept <= KEPT_RUNS {
                    break;
                }
                let Some(at) = order.iter().position(|kept| {
                    runs.get(kept)
                        .is_some_and(|r| r.tenant == tenant && r.status != "running")
                }) else {
                    break;
                };
                let oldest = order.remove(at);
                runs.remove(&oldest);
                if let Ok(mut evicted) = self.evicted.lock() {
                    *evicted.entry(tenant.as_str().to_string()).or_insert(0) += 1;
                }
            }
        }
        Ok(())
    }

    fn finish(&self, id: &str, status: &'static str, summary: Json, result: Option<Json>) {
        if let Ok(mut runs) = self.runs.lock() {
            if let Some(run) = runs.get_mut(id) {
                run.status = status;
                run.summary = summary;
                run.result = result;
            }
        }
    }
}

fn principal_of(identity: &Identity) -> Result<PrincipalId, ApiError> {
    match &identity.principal {
        Principal::User(id) => Ok(id.clone()),
        _ => Err(ApiError::new(
            ErrorCode::Unauthenticated,
            "Sign in to use the Impact Lab.",
        )),
    }
}

/// Replay a tenant's journal as the sealed store wrote it: an entry agrees when it hashes as
/// recorded plainly or in the sealed form, and one that still holds a value under a destroyed
/// key is reported as redacted (D-350, PRV-361).
fn replay_sealed(
    app: &App,
    tenant: &TenantId,
    entries: &[Recorded],
    live: Option<&TenantState>,
) -> JournalReplay {
    replay_journal_with(tenant, entries, live, &|before, transition| {
        app.sealed
            .journal_hashes(tenant, before, &transition.delta, &transition.events)
            .ok()
            .flatten()
    })
}

/// Every journal entry of the tenant up to and including `revision`, oldest first, read in pages.
fn journal_through(
    app: &App,
    tenant: &TenantId,
    revision: u64,
) -> Result<Vec<strata_storage_api::JournalEntry>, ApiError> {
    let mut entries = Vec::new();
    let mut after = 0;
    loop {
        let page = app.store.journal_after(tenant, after, JOURNAL_PAGE)?;
        let Some(last) = page.last() else {
            break;
        };
        after = last.revision.0;
        entries.extend(
            page.into_iter()
                .filter(|entry| entry.revision.0 <= revision),
        );
        if after >= revision {
            break;
        }
    }
    Ok(entries)
}

/// The tenant as a projection starts: its state at the revision the caller named, and the
/// revision the tenant is at now.
struct Basis {
    state: Arc<TenantState>,
    revision: u64,
    current: u64,
}

/// Decide the state a projection starts from (AGT-422). The revision named is the current one:
/// the live state. An earlier one: the journal replayed to it, which must reproduce every
/// recorded delta. A later one does not exist. With `require_current`, any revision but the
/// current one is a conflict that names the current one.
fn basis(
    app: &App,
    identity: &Identity,
    request: &RunRequest,
    live: &Arc<TenantState>,
) -> Result<Basis, ApiError> {
    let current = live.revision.0;
    let at = request.revision.unwrap_or(current);
    if request.require_current && at != current {
        let mut error = ApiError::new(
            ErrorCode::Conflict,
            format!("The site is at revision {current}, not {at}: it changed since this projection was asked for. Project again."),
        );
        error.0.kind = Some("StaleRevision".to_string());
        return Err(error);
    }
    if at > current {
        return Err(ApiError::new(
            ErrorCode::Invalid,
            format!("The site has no revision {at} yet; it is at revision {current}."),
        ));
    }
    if at == current {
        return Ok(Basis {
            state: live.clone(),
            revision: at,
            current,
        });
    }
    let entries: Vec<Recorded> = journal_through(app, &identity.tenant, at)?
        .into_iter()
        .map(|e| Recorded {
            revision: e.revision,
            kernel_version: e.kernel_version,
            intent: e.intent,
            context: e.context,
            delta_hash: e.delta_hash,
            events_hash: e.events_hash,
        })
        .collect();
    if entries.last().map(|entry| entry.revision.0) != Some(at) {
        return Err(ApiError::new(
            ErrorCode::NotFound,
            format!("The recorded history has no revision {at}."),
        ));
    }
    let report = replay_sealed(app, &identity.tenant, &entries, None);
    if let Some((revision, _)) = report.redacted.first() {
        return Err(ApiError::new(
            ErrorCode::Unavailable,
            format!(
                "The recorded history up to revision {at} holds values of an erased person from revision {revision} on, so it cannot be decided again."
            ),
        ));
    }
    if report.agreed != report.entries {
        return Err(ApiError::new(
            ErrorCode::Unavailable,
            format!(
                "The recorded history up to revision {at} does not replay under this kernel{}.",
                report
                    .first_mismatch
                    .map(|(revision, problem)| format!(" (revision {revision}: {problem})"))
                    .unwrap_or_default()
            ),
        ));
    }
    let state = report
        .replayed
        .tenants
        .get(&identity.tenant)
        .cloned()
        .ok_or_else(|| {
            ApiError::new(ErrorCode::NotFound, format!("no site {}", identity.tenant))
        })?;
    Ok(Basis {
        state,
        revision: at,
        current,
    })
}

fn complete(app: &App, tenant: &TenantId) -> Result<Arc<TenantState>, ApiError> {
    app.store
        .load_complete(tenant)?
        .tenant
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, format!("no site {tenant}")))
}

const PRESETS: [(&str, &str, &str, &str); 4] = [
    (
        "require_excerpt",
        "Require an excerpt on every article",
        "The excerpt is optional today; lists fall back to the opening paragraph. Find out what breaks if it becomes mandatory.",
        "require post.excerpt",
    ),
    (
        "short_titles",
        "Limit titles to 24 characters",
        "Tighten the title rule and see which existing articles and which future edits it would refuse.",
        "constraint post.title_length: len(new.title) >= 1 and len(new.title) <= 24 :: A title has between 1 and 24 characters",
    ),
    (
        "require_cover",
        "Require a cover image",
        "Every article would need one of the publication's drawings, and therefore a description of it.",
        "require post.cover",
    ),
    (
        "drop_tags",
        "Remove the tags field",
        "Delete the field and its data from every article, and see whether anything still depends on it.",
        "remove field post.tags",
    ),
];

/// What the Lab shows before anything is run: the tenant's metadata as data, the snapshots, the
/// presets, the evidence classes and the last verification report.
pub fn overview(app: &App, identity: &Identity) -> Result<Json, ApiError> {
    let tenant = complete(app, &identity.tenant)?;
    let schema = &tenant.schema;
    let objects: Vec<Json> = schema
        .objects
        .values()
        .map(|object| {
            json!({
                "id": object.id.as_str(),
                "label": object.label,
                "records": tenant.records.get(&object.id).map_or(0, BTreeMap::len),
                "fields": object.fields.values().map(|f| json!({
                    "id": f.id.as_str(),
                    "label": f.label,
                    "type": f.ty.name(),
                    "required": f.required,
                    "unique": f.unique,
                    "default": f.default.as_ref().map(|d| d.to_string()),
                    "read": f.read.as_ref().map(|r| r.to_string()),
                })).collect::<Vec<_>>(),
                "constraints": object.constraints.values().map(|c| json!({
                    "id": c.id.as_str(), "rule": c.expr.to_string(), "message": c.message,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let workflows: Vec<Json> = schema
        .workflows
        .values()
        .map(|w| {
            json!({
                "id": w.id.as_str(),
                "object": w.object.as_str(),
                "field": w.field.as_str(),
                "initial": w.initial,
                "transitions": w.transitions.iter().map(|t| json!({
                    "name": t.name, "from": t.from, "to": t.to,
                    "guard": t.guard.as_ref().map(|g| g.to_string()),
                    "sets": t.set.iter().map(|(field, value)| format!("{field} = {value}")).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let policies: Vec<Json> = schema
        .policies
        .values()
        .map(|p| {
            json!({
                "id": p.id.as_str(),
                "role": p.role.as_str(),
                "resource": match &p.resource {
                    strata_kernel::Resource::Object(o) => o.as_str().to_string(),
                    other => format!("{other:?}").to_lowercase(),
                },
                "actions": p.actions.iter().map(|a| format!("{a:?}").to_lowercase()).collect::<Vec<_>>(),
                "when": p.condition.as_ref().map(|c| c.to_string()),
            })
        })
        .collect();
    let queries: Vec<Json> = schema
        .queries
        .values()
        .map(|q| json!({ "id": q.id.as_str(), "object": q.object.as_str(), "where": q.filter.as_ref().map(|f| f.to_string()), "params": q.params, "limit": q.limit }))
        .collect();
    let journal = app.store.journal_count(&identity.tenant)?;
    let classes: Vec<Json> = [
        Class::Proven,
        Class::Reproduced,
        Class::SoundApproximation,
        Class::Empirical,
        Class::Unknown,
    ]
    .iter()
    .map(|c| json!({ "id": c, "label": c.label(), "meaning": c.meaning() }))
    .collect();
    Ok(json!({
        "tenant": {
            "id": tenant.id.as_str(),
            "name": tenant.name,
            "revision": tenant.revision.0,
            "schema_version": schema.version.0,
            "event_seq": tenant.event_seq,
            "records": tenant.record_count(),
            "state_hash": strata_kernel::canon::hash_of(&*tenant),
            "journal_entries": journal,
        },
        "kernel_version": strata_kernel::KERNEL_VERSION,
        "objects": objects,
        "workflows": workflows,
        "policies": policies,
        "roles": schema.roles.values().map(|r| json!({ "id": r.id.as_str(), "label": r.label })).collect::<Vec<_>>(),
        "queries": queries,
        "views": schema.views.values().map(|v| json!({ "id": v.id.as_str(), "route": v.route })).collect::<Vec<_>>(),
        "members": tenant.members.iter().map(|(p, roles)| json!({ "principal": p.as_str(), "roles": roles })).collect::<Vec<_>>(),
        "snapshots": app.store.snapshots(&identity.tenant)?,
        "presets": PRESETS.iter().map(|(id, title, about, change)| json!({ "id": id, "title": title, "about": about, "change": change })).collect::<Vec<_>>(),
        "classes": classes,
        "verification": app.lab.verification_of(&identity.tenant),
        "recent_runs": app.lab.recent().into_iter().filter(|r| r["tenant"] == identity.tenant.as_str()).collect::<Vec<_>>(),
        "runs_kept": {
            "most_per_site": KEPT_RUNS,
            "dropped_since_start": app.lab.evicted_of(&identity.tenant),
            "note": RETENTION,
        },
    }))
}

/// The projection as the Lab shows it: the comparison in full, the traces as a timeline.
fn present(
    projection: &Projection,
    scenario: &Scenario,
    request: &RunRequest,
    snapshot: Json,
    faulted: Option<&Faulted>,
) -> Json {
    let per_request = faulted.map(|faulted| faulted.per_request());
    let branch: BTreeMap<usize, &strata_simulation::engine::Step> = projection
        .branch
        .steps
        .iter()
        .filter_map(|s| s.index.map(|i| (i, s)))
        .collect();
    let who = |principal: &Principal| match principal {
        Principal::User(id) => id.as_str().to_string(),
        other => format!("{other:?}").to_lowercase(),
    };
    let timeline: Vec<Json> = projection
        .baseline
        .steps
        .iter()
        .filter_map(|step| step.index.map(|i| (i, step)))
        .map(|(index, step)| {
            let other = branch.get(&index);
            let (a, b) = (
                step.outcome.verdict(),
                other.map_or_else(String::new, |s| s.outcome.verdict()),
            );
            json!({
                "step": index,
                "at": step.at,
                "label": step.label,
                "principal": who(&step.context.principal),
                "baseline": a,
                "branch": b,
                "diverged": projection.comparison.outcome_changes.iter().any(|c| c.step == index),
                "faulted": per_request.as_ref().and_then(|each| each.get(&index)),
            })
        })
        .collect();
    let check = replay_trace(&scenario.initial, &projection.baseline);
    json!({
        "request": {
            "change": request.change,
            "existing": request.existing,
            "source": request.source,
            "steps": scenario.schedule.len(),
            "seed": scenario.seed,
            "faults": request.faults,
            "apply_at": projection.proposal.before_step,
            "revision": request.revision,
            "require_current": request.require_current,
        },
        "revision": snapshot["revision"].clone(),
        "current_revision": snapshot["current_revision"].clone(),
        "retention": RETENTION,
        "snapshot": snapshot,
        "kernel_version": projection.baseline.kernel_version,
        "change_outcome": projection.change_outcome,
        "affected_existing": projection.affected_existing,
        "baseline": {
            "trace_hash": projection.baseline_hash,
            "final_hash": projection.baseline.final_hash,
            "accepted": projection.baseline.accepted(),
            "rejected": projection.baseline.rejected(),
        },
        "branch": {
            "trace_hash": projection.branch_hash,
            "final_hash": projection.branch.final_hash,
            "accepted": projection.branch.accepted(),
            "rejected": projection.branch.rejected(),
        },
        "trace_replay": { "steps": check.steps, "agreed": check.agreed, "final_hash_matches": check.final_hash_matches },
        "timeline": timeline,
        "faults": faulted.map(Faulted::report),
        "comparison": projection.comparison,
        "counterexample": projection.counterexample,
        "findings": projection.findings,
        "unmodelled": projection.unmodelled,
    })
}

/// The simulated world of a generated run: the requesting tenant alone.
fn world_of(tenant: &TenantId, state: &Arc<TenantState>) -> World {
    let mut world = World::default();
    world.tenants.insert(tenant.clone(), state.clone());
    world
}

fn build_scenario(
    app: &App,
    identity: &Identity,
    request: &RunRequest,
    basis: &Basis,
) -> Result<Scenario, ApiError> {
    let seed = request.seed.unwrap_or(1);
    match request.source.as_str() {
        "generated" => {
            // The simulated world holds the requesting tenant alone: a run never reads, and the
            // process never copies into memory for it, another tenant's records.
            let world = world_of(&identity.tenant, &basis.state);
            let steps = request.steps.unwrap_or(60).clamp(5, 300);
            let schedule = generate(&world, &identity.tenant, seed, steps, app.now().0, 60_000);
            Ok(Scenario {
                name: format!("generated-{seed}"),
                seed,
                initial: world,
                schedule,
            })
        }
        "journal" => {
            let schedule: Vec<Scheduled> = journal_through(app, &identity.tenant, basis.revision)?
                .into_iter()
                .map(|entry| Scheduled {
                    label: format!(
                        "{} (revision {})",
                        entry.intent.name().replace('_', " "),
                        entry.revision.0
                    ),
                    context: entry.context,
                    intent: entry.intent,
                })
                .collect();
            Ok(Scenario {
                name: "journal".into(),
                seed,
                initial: World::default(),
                schedule,
            })
        }
        other => Err(ApiError::new(
            ErrorCode::Invalid,
            format!("unknown schedule source {other:?}"),
        )),
    }
}

/// The faults a projection that asks for them meets, as a deployment meets them: a commit that
/// loses a race to another writer, a commit whose reply is lost after the write is durable, a
/// process that stops right after it answered, and an outbox that cannot be read or whose
/// acknowledgement is lost. In events per million of each kind of call; the schedule, the seed and
/// these rates decide where each fault falls.
fn lab_faults() -> ShellFaults {
    ShellFaults {
        store: StoreFaults {
            commit_conflict: 100_000,
            commit_reply_lost: 100_000,
            publish_unavailable: 100_000,
            publish_ack_lost: 100_000,
            ..StoreFaults::default()
        },
        crash: 50_000,
        ..ShellFaults::default()
    }
}

/// What a faulted projection runs on besides the schedule: the tenant's recorded history, which
/// makes the fresh process's store a copy of the tenant, and the last event sequence number that
/// history holds (the publications of the schedule are the ones after it).
struct ShellInput {
    history: Vec<Recorded>,
    base_event_seq: u64,
}

fn shell_input(
    app: &App,
    identity: &Identity,
    request: &RunRequest,
    basis: &Basis,
) -> Result<Option<ShellInput>, ApiError> {
    if !request.faults {
        return Ok(None);
    }
    if request.source == "journal" {
        // The schedule is the journal itself, from nothing: there is no history before it.
        return Ok(Some(ShellInput {
            history: Vec::new(),
            base_event_seq: 0,
        }));
    }
    let history = journal_through(app, &identity.tenant, basis.revision)?
        .into_iter()
        .map(|e| Recorded {
            revision: e.revision,
            kernel_version: e.kernel_version,
            intent: e.intent,
            context: e.context,
            delta_hash: e.delta_hash,
            events_hash: e.events_hash,
        })
        .collect();
    Ok(Some(ShellInput {
        history,
        base_event_seq: basis.state.event_seq,
    }))
}

/// A projection's schedule as the deployment ran it with faults: the shell run, whose trace is
/// the only source of what the Lab reports about faults.
struct Faulted {
    tenant: TenantId,
    base_event_seq: u64,
    run: ShellRun,
}

/// Run `scenario`'s schedule through the deployment on a copy of the tenant, with
/// [`lab_faults`] and a client that retries with idempotency keys.
fn run_faulted(
    tenant: &TenantId,
    scenario: &Scenario,
    input: &ShellInput,
) -> Result<Faulted, String> {
    let first = input
        .history
        .iter()
        .map(|entry| entry.context.now.0)
        .chain(scenario.schedule.iter().map(|step| step.context.now.0))
        .min()
        .unwrap_or(0);
    let shell = ShellScenario {
        name: scenario.name.clone(),
        seed: scenario.seed,
        start: first,
        schedule: scenario.schedule.clone(),
        faults: lab_faults(),
        listen: vec![tenant.clone()],
        keys: true,
        start_faults: StoreFaults::default(),
        operations: Vec::new(),
    };
    let run = run_shell_after(
        &crate::simulated::Backend::new(None),
        &shell,
        &input.history,
        &|_, _| Vec::new(),
    )?;
    Ok(Faulted {
        tenant: tenant.clone(),
        base_event_seq: input.base_event_seq,
        run,
    })
}

impl Faulted {
    /// What happened to each scheduled request: how often it was sent, the commit races its
    /// commits lost and retried, and the answer the client finally had.
    fn per_request(&self) -> BTreeMap<usize, Json> {
        let trace = &self.run.trace;
        let last = trace.last_answers();
        let mut each: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
        for step in &trace.steps {
            let entry = each.entry(step.index).or_default();
            entry.0 += 1;
            if let Answer::Committed { conflicts, .. } = &step.answer {
                entry.1 += conflicts;
            }
        }
        each.into_iter()
            .map(|(index, (sent, conflicts))| {
                (
                    index,
                    json!({
                        "sent": sent,
                        "conflicts_retried": conflicts,
                        "answer": last.get(&index).map(|answer| answer.verdict()),
                    }),
                )
            })
            .collect()
    }

    /// The report of the run, counted from its trace.
    fn report(&self) -> Json {
        let trace = &self.run.trace;
        let mut injected: BTreeMap<String, usize> = BTreeMap::new();
        for fault in &trace.injected {
            let name = serde_json::to_value(fault.fault)
                .ok()
                .and_then(|name| name.as_str().map(str::to_string))
                .unwrap_or_default();
            *injected.entry(name).or_default() += 1;
        }
        let (mut conflicts_retried, mut unavailable, mut answered_from_key) =
            (0usize, 0usize, 0usize);
        for step in &trace.steps {
            match &step.answer {
                Answer::Committed {
                    conflicts,
                    replayed,
                    ..
                } => {
                    conflicts_retried += conflicts;
                    answered_from_key += usize::from(*replayed);
                }
                answer => unavailable += usize::from(answer.unavailable()),
            }
        }
        let heard: Vec<u64> = trace
            .heard
            .iter()
            .filter(|h| h.event.tenant == self.tenant && h.event.seq > self.base_event_seq)
            .map(|h| h.event.seq)
            .collect();
        let distinct: BTreeSet<u64> = heard.iter().copied().collect();
        let committed = self
            .run
            .world
            .tenants
            .get(&self.tenant)
            .map_or(0, |state| state.event_seq)
            .saturating_sub(self.base_event_seq);
        json!({
            "trace_hash": trace.hash(),
            "requests": trace.last_answers().len(),
            "sent": trace.steps.len(),
            "injected": injected,
            "process_stops": trace.restarts.len(),
            "conflicts_retried": conflicts_retried,
            "unavailable_answers": unavailable,
            "answered_from_key": answered_from_key,
            "publications": {
                "committed": committed,
                "offered": heard.len(),
                "heard": distinct.len(),
                "offered_again": heard.len() - distinct.len(),
            },
            "quiesced": trace.quiesced,
            "invariants": trace.invariants.iter().map(|i| json!({ "id": i.id, "statement": i.statement, "holds": i.holds })).collect::<Vec<_>>(),
        })
    }
}

/// Start a projection in the background and return its id. Progress and the result are sent to
/// the caller's WebSocket connections; the result can also be fetched by id.
///
/// A request that carries an idempotency key starts one projection for that key, tenant and person
/// within 24 hours: a repeat with the same request answers the first answer, and the same key
/// with another request is refused as an idempotency mismatch (D-6595, ARC-1599). The keys, like
/// the runs, are held in this backend process.
pub fn start(
    shared: &Shared,
    app: &App,
    identity: &Identity,
    request: RunRequest,
) -> Result<Json, ApiError> {
    let Some(key) = identity.idempotency_key.clone() else {
        return start_once(shared, app, identity, request);
    };
    let principal = principal_of(identity)?;
    let scope: KeyScope = (
        identity.tenant.as_str().to_string(),
        principal.as_str().to_string(),
        key,
    );
    let held = format!("{request:?}");
    let now = app.now().0;
    if let Some(first) = app.lab.claim_key(&scope, &held, now)? {
        return Ok(first);
    }
    let started = start_once(shared, app, identity, request);
    app.lab
        .settle_key(&scope, &held, started.as_ref().ok(), now);
    started
}

fn start_once(
    shared: &Shared,
    app: &App,
    identity: &Identity,
    request: RunRequest,
) -> Result<Json, ApiError> {
    let principal = principal_of(identity)?;
    let live = complete(app, &identity.tenant)?;
    let basis = basis(app, identity, &request, &live)?;
    let state = basis.state.clone();
    let changes = proposal::parse(&state.schema, &request.change)
        .map_err(|problem| ApiError::new(ErrorCode::Invalid, problem))?;
    let existing = match request.existing.as_str() {
        "reject" => ExistingData::Reject,
        "grandfather" => ExistingData::Grandfather,
        other => {
            return Err(ApiError::new(
                ErrorCode::Invalid,
                format!("unknown handling of existing records {other:?}"),
            ))
        }
    };
    let scenario = build_scenario(app, identity, &request, &basis)?;
    let shell = shell_input(app, identity, &request, &basis)?;
    // A recorded journal starts from nothing, so the change can only apply once the tenant and
    // its schema exist: after the first two entries.
    let earliest = if request.source == "journal" { 2 } else { 0 };
    let before_step = request
        .apply_at
        .unwrap_or(earliest)
        .clamp(earliest, scenario.schedule.len());
    let proposal = Proposal {
        tenant: identity.tenant.clone(),
        principal: Principal::User(principal.clone()),
        changes,
        existing,
        before_step,
    };
    let snapshot = json!({
        "revision": basis.revision,
        "current_revision": basis.current,
        "schema_version": state.schema.version.0,
        "records": state.record_count(),
        "state_hash": strata_kernel::canon::hash_of(&*state),
    });

    let steps = scenario.schedule.len();
    let id = format!("run_{}", crate::auth::random_hex(6)?);
    let cancel = Arc::new(AtomicBool::new(false));
    let summary = json!({ "change": request.change, "source": request.source, "steps": scenario.schedule.len() });
    app.lab.insert_within(
        &id,
        Run {
            tenant: identity.tenant.clone(),
            principal: principal.clone(),
            status: "running",
            started_at: app.now().0,
            summary: summary.clone(),
            result: None,
            cancel: cancel.clone(),
        },
        app.lab.runs_per_tenant,
    )?;
    LOG.info("lab.started", &identity.correlation, json!({ "run": id, "tenant": identity.tenant.as_str(), "source": request.source, "steps": scenario.schedule.len() }));

    let (shared, run_id, tenant, correlation) = (
        shared.clone(),
        id.clone(),
        identity.tenant.clone(),
        identity.correlation.clone(),
    );
    let job = move || {
        // A job that panics leaves its run failed, not running for ever.
        struct Unfinished {
            shared: Shared,
            run: String,
            tenant: TenantId,
            principal: PrincipalId,
            summary: Json,
            armed: bool,
        }
        impl Drop for Unfinished {
            fn drop(&mut self) {
                if !self.armed {
                    return;
                }
                self.shared
                    .lab
                    .finish(&self.run, "failed", self.summary.clone(), None);
                let _ = self.shared.hub.runs.send(RunMessage {
                    tenant: self.tenant.clone(),
                    principal: self.principal.clone(),
                    run: self.run.clone(),
                    kind: "failed".to_string(),
                    payload: json!({}),
                });
            }
        }
        let mut unfinished = Unfinished {
            shared: shared.clone(),
            run: run_id.clone(),
            tenant: tenant.clone(),
            principal: principal.clone(),
            summary: summary.clone(),
            armed: true,
        };
        let send = |kind: &str, payload: Json| {
            let _ = shared.hub.runs.send(RunMessage {
                tenant: tenant.clone(),
                principal: principal.clone(),
                run: run_id.clone(),
                kind: kind.to_string(),
                payload,
            });
        };
        let mut last = (9u8, 0usize);
        let outcome = project(&scenario, &proposal, &mut |phase, done, total| {
            // One message per twentieth of a phase is plenty for a progress bar.
            let slot = (done * 20).checked_div(total).unwrap_or(0);
            if (phase, slot) != last {
                last = (phase, slot);
                let name = ["baseline", "branch", "counterexample"][phase as usize % 3];
                send(
                    "progress",
                    json!({ "phase": name, "done": done, "total": total }),
                );
            }
            !cancel.load(Ordering::Relaxed)
        });
        match outcome {
            Ok(projection) => {
                let faulted = match shell
                    .as_ref()
                    .map(|input| run_faulted(&tenant, &scenario, input))
                    .transpose()
                {
                    Ok(faulted) => faulted,
                    Err(error) => {
                        // The projection stands, but the faults it asked about were not shown.
                        let summary = json!({ "change": request.change, "source": request.source, "error": error });
                        shared.lab.finish(&run_id, "failed", summary, None);
                        unfinished.armed = false;
                        LOG.warn(
                            "lab.faults_failed",
                            &correlation,
                            json!({ "run": run_id, "error": error }),
                        );
                        send("failed", json!({ "error": error }));
                        return;
                    }
                };
                let result = present(&projection, &scenario, &request, snapshot, faulted.as_ref());
                let summary = json!({
                    "change": request.change,
                    "source": request.source,
                    "steps": scenario.schedule.len(),
                    "diverged": projection.comparison.outcome_changes.len(),
                    "baseline_hash": projection.baseline_hash,
                    "branch_hash": projection.branch_hash,
                });
                shared
                    .lab
                    .finish(&run_id, "finished", summary.clone(), Some(result));
                unfinished.armed = false;
                LOG.info(
                    "lab.finished",
                    &correlation,
                    json!({ "run": run_id, "summary": summary }),
                );
                send("finished", summary);
            }
            Err(_) => {
                shared
                    .lab
                    .finish(&run_id, "cancelled", summary.clone(), None);
                unfinished.armed = false;
                LOG.info("lab.cancelled", &correlation, json!({ "run": run_id }));
                send("cancelled", json!({}));
            }
        }
    };
    if let Err(busy) = app.lab.pool.submit(job) {
        app.lab.remove(&id);
        return Err(busy);
    }
    Ok(
        json!({ "run": id, "steps": steps, "apply_at": before_step, "revision": basis.revision, "current_revision": basis.current }),
    )
}

/// The tenant's whole journal, as the store holds it, replayed from nothing as the sealed store
/// wrote it and compared with the live state: the report, and the live state it was compared
/// with. [`replay_live`] answers with it; a simulation of the backend checks its runs with it.
pub fn replay_tenant(
    app: &App,
    tenant: &TenantId,
) -> Result<(JournalReplay, Arc<TenantState>), ApiError> {
    let live = complete(app, tenant)?;
    let entries: Vec<Recorded> = journal_through(app, tenant, u64::MAX)?
        .into_iter()
        .map(|e| Recorded {
            revision: e.revision,
            kernel_version: e.kernel_version,
            intent: e.intent,
            context: e.context,
            delta_hash: e.delta_hash,
            events_hash: e.events_hash,
        })
        .collect();
    let report = replay_sealed(app, tenant, &entries, Some(&live));
    Ok((report, live))
}

/// Replay the tenant's journal of committed transitions from nothing and compare with the live
/// state.
pub fn replay_live(app: &App, identity: &Identity) -> Result<Json, ApiError> {
    let (report, live) = replay_tenant(app, &identity.tenant)?;
    LOG.info(
        "lab.replayed",
        &identity.correlation,
        json!({ "tenant": identity.tenant.as_str(), "entries": report.entries, "agreed": report.agreed, "matches_live": report.matches_live }),
    );
    Ok(json!({
        "report": report,
        "live_revision": live.revision.0,
        "class": Class::Reproduced,
        "establishes": "Every committed transition, run again through the kernel with its recorded context, produces the recorded state delta and events, and the result is the live state.",
        "does_not_establish": [
            "Anything about requests that were rejected: they are not journalled.",
            "The order in which concurrent requests arrived, beyond the order in which they committed.",
            "External effects: sessions, event delivery and what browsers did are not re-enacted.",
        ],
    }))
}

pub use replay_live as replay;

/// Build the Lean project, audit the proofs and run the correspondence fixtures, in the
/// background. The report arrives over the WebSocket and is kept, per tenant, for the overview.
///
/// A build is heavy, so it is limited: one at a time on the whole backend (on its own worker),
/// one at a time and at most one per `verify_interval_ms` for each tenant.
pub fn verify(shared: &Shared, app: &App, identity: &Identity) -> Result<Json, ApiError> {
    let principal = principal_of(identity)?;
    let id = format!("verify_{}", crate::auth::random_hex(4)?);
    let (shared, run, tenant, lean_dir, now) = (
        shared.clone(),
        id.clone(),
        identity.tenant.clone(),
        app.cfg.lean_dir.clone(),
        app.now().0,
    );
    let correlation = identity.correlation.clone();
    let key = tenant.as_str().to_string();
    {
        let mut verifying =
            app.lab.verifying.lock().map_err(|_| {
                ApiError::hidden(ErrorCode::Internal, "verification state poisoned")
            })?;
        if verifying.running.contains(&key) {
            return Err(ApiError::new(
                ErrorCode::Throttled,
                "A verification of this site is already running.",
            ));
        }
        if verifying
            .last_started
            .get(&key)
            .is_some_and(|last| now - last < app.lab.verify_interval_ms)
        {
            return Err(ApiError::new(
                ErrorCode::Throttled,
                "This site was verified a moment ago. Wait a minute and try again.",
            ));
        }
        verifying.running.insert(key.clone());
        verifying.last_started.insert(key.clone(), now);
    }
    let job = move || {
        // Frees the tenant's place however the build ends, a panic included.
        struct Done(Shared, String);
        impl Drop for Done {
            fn drop(&mut self) {
                if let Ok(mut verifying) = self.0.lab.verifying.lock() {
                    verifying.running.remove(&self.1);
                }
            }
        }
        let _done = Done(shared.clone(), key.clone());
        let report = strata_verification::lean::verify(&lean_dir, now);
        LOG.info("lab.verified", &correlation, json!({ "all_checked": report.all_checked, "theorems": report.theorems.len(), "problem": report.problem }));
        let payload = json!(report);
        if let Ok(mut reports) = shared.lab.verification.lock() {
            reports.insert(key.clone(), payload.clone());
        }
        let _ = shared.hub.runs.send(RunMessage {
            tenant,
            principal,
            run,
            kind: "verification".into(),
            payload,
        });
    };
    let key = identity.tenant.as_str().to_string();
    if let Err(busy) = app.lab.verify_pool.submit(job) {
        if let Ok(mut verifying) = app.lab.verifying.lock() {
            verifying.running.remove(&key);
            verifying.last_started.remove(&key);
        }
        return Err(busy);
    }
    Ok(json!({ "run": id }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_kernel::{SessionId, Timestamp};
    use strata_protocol::ws::SessionState;
    use strata_storage_api::SessionRecord;

    /// A projection is built from the requesting tenant's state alone: its world holds that one
    /// tenant, and the schedule generated over it addresses that tenant and no other.
    #[test]
    fn a_generated_run_reads_only_the_requesting_tenant() {
        let alpha = TenantId::new("alpha").unwrap();
        let state = strata_kernel::testkit::notes_world().state(&alpha);
        let tenant = state.tenant.clone().expect("alpha exists");
        let world = world_of(&alpha, &tenant);
        assert_eq!(world.tenants.keys().collect::<Vec<_>>(), vec![&alpha]);
        let schedule = generate(&world, &alpha, 7, 30, 1_000, 60_000);
        assert!(!schedule.is_empty());
        assert!(schedule.iter().all(|step| step.context.tenant == alpha));
    }

    fn identity(tenant: &str, principal: Option<&str>) -> Identity {
        let tenant = TenantId::new(tenant).unwrap();
        let session = principal.map(|p| SessionRecord {
            id: SessionId::new(format!("s_{}", p.replace(':', "_"))).unwrap(),
            token_hash: String::new(),
            csrf_token: String::new(),
            tenant: tenant.clone(),
            principal: PrincipalId::new(p).unwrap(),
            display_name: p.to_string(),
            created_at: Timestamp(0),
            last_seen_at: Timestamp(0),
            expires_at: Timestamp(i64::MAX),
            revoked_at: None,
            authenticated_at: Timestamp(0),
            network: None,
        });
        Identity {
            tenant,
            principal: principal.map_or(Principal::Anonymous, |p| {
                Principal::User(PrincipalId::new(p).unwrap())
            }),
            session,
            state: SessionState::Active,
            correlation: "t".into(),
            verified_address: None,
            idempotency_key: None,
        }
    }

    fn running(lab: &Lab, id: &str, tenant: &str, starter: &str) -> Arc<AtomicBool> {
        let cancel = Arc::new(AtomicBool::new(false));
        lab.insert(
            id,
            Run {
                tenant: TenantId::new(tenant).unwrap(),
                principal: PrincipalId::new(starter).unwrap(),
                status: "running",
                started_at: 0,
                summary: Json::Null,
                result: None,
                cancel: cancel.clone(),
            },
        );
        cancel
    }

    /// Work beyond the workers and the queue is refused, not given a thread of its own, and
    /// queued work runs once a worker is free.
    #[test]
    fn a_pool_runs_a_bounded_amount_of_work() {
        use std::sync::mpsc::channel;
        let pool = Pool::new("test", 1, 1);
        let (release, held) = channel::<()>();
        let (started, running) = channel::<()>();
        pool.submit(move || {
            let _ = started.send(());
            let _ = held.recv();
        })
        .expect("the worker takes it");
        running
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the worker started");
        let (done, finished) = channel::<()>();
        pool.submit(move || {
            let _ = done.send(());
        })
        .expect("the queue holds one");
        let refused = pool.submit(|| {}).expect_err("worker busy, queue full");
        assert_eq!(refused.0.code, ErrorCode::Throttled);
        release.send(()).unwrap();
        finished
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the queued job ran once the worker was free");
    }

    /// A tenant has only so many runs going at once; another tenant is not affected.
    #[test]
    fn a_tenant_may_have_only_so_many_runs_going() {
        let lab = Lab::new();
        let run = |tenant: &str| Run {
            tenant: TenantId::new(tenant).unwrap(),
            principal: PrincipalId::new("user:admin").unwrap(),
            status: "running",
            started_at: 0,
            summary: Json::Null,
            result: None,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        assert!(lab.insert_within("run_1", run("fieldnotes"), 2).is_ok());
        assert!(lab.insert_within("run_2", run("fieldnotes"), 2).is_ok());
        let refused = lab
            .insert_within("run_3", run("fieldnotes"), 2)
            .unwrap_err();
        assert_eq!(refused.0.code, ErrorCode::Throttled);
        assert!(lab.insert_within("run_4", run("tidewater"), 2).is_ok());
        lab.finish("run_1", "finished", Json::Null, None);
        assert!(lab.insert_within("run_5", run("fieldnotes"), 2).is_ok());
    }

    /// A run is cancelled by the person who started it or by someone who may change the schema
    /// of its tenant. Anyone else, a visitor with no session included, finds no such run, and
    /// the run goes on.
    #[test]
    fn only_the_starter_or_a_schema_manager_cancels_a_run() {
        let lab = Lab::new();
        let flag = running(&lab, "run_a", "fieldnotes", "user:admin");
        assert!(!lab.cancel("run_a", &identity("fieldnotes", None), false));
        assert!(!lab.cancel("run_a", &identity("fieldnotes", Some("user:mirela")), false));
        assert!(!lab.cancel("run_a", &identity("tidewater", Some("user:admin")), true));
        assert!(!flag.load(Ordering::Relaxed), "nobody else stopped it");
        assert!(lab.cancel("run_a", &identity("fieldnotes", Some("user:admin")), false));
        assert!(flag.load(Ordering::Relaxed));

        let flag = running(&lab, "run_b", "fieldnotes", "user:admin");
        assert!(lab.cancel("run_b", &identity("fieldnotes", Some("user:ines")), true));
        assert!(flag.load(Ordering::Relaxed));
    }

    /// The kept-runs ring is per tenant: one tenant's runs never evict another's, and a run that
    /// is still running is never evicted.
    #[test]
    fn the_kept_runs_ring_is_per_tenant_and_keeps_running_runs() {
        let lab = Lab::new();
        let run = |tenant: &str, status: &'static str| Run {
            tenant: TenantId::new(tenant).unwrap(),
            principal: PrincipalId::new("user:admin").unwrap(),
            status,
            started_at: 0,
            summary: Json::Null,
            result: None,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        lab.insert_within("quiet_running", run("tidewater", "running"), 2)
            .unwrap();
        lab.insert_within("quiet_done", run("tidewater", "finished"), 2)
            .unwrap();
        lab.insert_within("loud_running", run("fieldnotes", "running"), 2)
            .unwrap();
        for n in 0..(KEPT_RUNS * 3) {
            lab.insert_within(&format!("loud_{n}"), run("fieldnotes", "finished"), 2)
                .unwrap();
        }
        let runs = lab.runs.lock().unwrap();
        assert!(runs.contains_key("quiet_running"));
        assert!(runs.contains_key("quiet_done"));
        assert!(runs.contains_key("loud_running"), "a running run is kept");
        let loud = runs
            .values()
            .filter(|r| r.tenant.as_str() == "fieldnotes")
            .count();
        assert!(loud <= KEPT_RUNS + 1);
    }

    /// A projection with faults on reports what the shell trace holds. The expectations follow
    /// from what the trace means, not from a run: a commit whose reply is lost is answered
    /// "unavailable" and, the request carrying a key, sent again and answered from its key (one
    /// of each per lost reply, here, where no worker commits); a commit race retried is one that
    /// the store refused, so there are no more of them than refusals; every committed event is
    /// published, so the events heard are the events the committed requests produced, and what was
    /// offered beyond that was offered again.
    #[test]
    fn what_a_faulted_projection_reports_is_what_its_shell_trace_holds() {
        use strata_kernel::testkit::{add_notes_tenant, ctx, notes_schema, principal_id, role};
        use strata_kernel::{Intent, KERNEL_VERSION};
        use strata_simulation::shell::Arrival;

        let alpha = TenantId::new("alpha").unwrap();
        let owner = principal_id("alpha:owner");
        let mut intents = vec![
            (
                Principal::System,
                Intent::CreateTenant {
                    name: "alpha".into(),
                    owner: owner.clone(),
                },
            ),
            (
                Principal::User(owner.clone()),
                Intent::ChangeSchema {
                    changes: notes_schema(),
                    expected_version: None,
                    existing: ExistingData::Reject,
                },
            ),
        ];
        for (name, member_role) in [
            ("editor", "editor"),
            ("writer1", "writer"),
            ("writer2", "writer"),
        ] {
            intents.push((
                Principal::User(owner.clone()),
                Intent::AssignRole {
                    principal: principal_id(&format!("alpha:{name}")),
                    role: role(member_role),
                },
            ));
        }
        // The recorded history of the tenant, as `add_notes_tenant` makes it.
        let history: Vec<Recorded> = intents
            .into_iter()
            .enumerate()
            .map(|(n, (principal, intent))| Recorded {
                revision: strata_kernel::StateRevision(n as u64 + 1),
                kernel_version: KERNEL_VERSION.to_string(),
                intent,
                context: ctx("alpha", principal, 1_001 + n as i64),
                delta_hash: String::new(),
                events_hash: String::new(),
            })
            .collect();
        let world = add_notes_tenant(World::default(), "alpha", 1_000);
        let base_event_seq = world.tenants[&alpha].event_seq;
        let schedule = generate(&world, &alpha, 3, 200, 1_000_000, 60_000);
        let scenario = Scenario {
            name: "generated-3".into(),
            seed: 3,
            initial: world,
            schedule,
        };
        let input = ShellInput {
            history,
            base_event_seq,
        };
        let faulted = run_faulted(&alpha, &scenario, &input).expect("the history replays");
        let report = faulted.report();
        let trace = &faulted.run.trace;

        let count = |fault: &str| report["injected"][fault].as_u64().unwrap_or(0);
        // The faults were really injected, and the schedule was really sent.
        assert!(count("commit_reply_lost") > 0 && count("commit_conflict") > 0);
        assert_eq!(report["requests"], scenario.schedule.len());
        let retried = trace
            .steps
            .iter()
            .filter(|step| step.arrival == Arrival::Retried)
            .count();
        assert_eq!(report["sent"], scenario.schedule.len() + retried);
        // A lost reply: unavailable, then answered from its key.
        assert_eq!(report["unavailable_answers"], count("commit_reply_lost"));
        assert_eq!(report["answered_from_key"], count("commit_reply_lost"));
        assert!(report["conflicts_retried"].as_u64().unwrap() <= count("commit_conflict"));
        // Every committed event is published, in order, and what is offered beyond is offered again.
        // A commit whose reply was lost is answered unavailable first and then, on the retry, from
        // its key (replayed): its events are counted once, from the replayed answer.
        let produced: usize = trace
            .steps
            .iter()
            .filter_map(|step| match &step.answer {
                Answer::Committed { events, .. } => Some(events.len()),
                _ => None,
            })
            .sum();
        let published = &report["publications"];
        // A commit whose reply was lost is answered from its key on the retry with its receipt and
        // no events, so the answers show fewer events than the tenant committed: every lost reply
        // hides at least one. Every committed event is heard.
        let committed = published["committed"].as_u64().unwrap();
        assert!(committed >= produced as u64);
        assert!(count("commit_reply_lost") == 0 || committed > produced as u64);
        assert_eq!(published["heard"], published["committed"]);
        assert_eq!(
            published["offered"].as_u64().unwrap() - published["heard"].as_u64().unwrap(),
            published["offered_again"].as_u64().unwrap()
        );
        assert_eq!(report["quiesced"], true);
        assert!(report["invariants"]
            .as_array()
            .unwrap()
            .iter()
            .all(|invariant| invariant["holds"] == true));
        // The per-request view adds up to the same trace.
        let each = faulted.per_request();
        assert_eq!(each.len(), scenario.schedule.len());
        let sent: u64 = each.values().map(|e| e["sent"].as_u64().unwrap()).sum();
        assert_eq!(sent as usize, trace.steps.len());
    }
}
