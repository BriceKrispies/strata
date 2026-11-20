//! Impact Lab: projections, journal replay and verification runs.
//!
//! Everything here reads the store and never writes it. A projection loads a complete snapshot,
//! hands it to the simulator, and the simulator works on that copy with the same kernel the
//! backend uses. Live tenant state cannot be changed from this module: it calls no `commit`.

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
use strata_simulation::engine::{replay as replay_trace, Faults, Scenario, Scheduled};
use strata_simulation::journal::{replay_journal_with, JournalReplay, Recorded};
use strata_simulation::project::{project, Projection, Proposal};
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
) -> Json {
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
                "conflicts": step.conflicts,
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
            "faults": scenario.faults,
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
            "deliveries": projection.baseline.deliveries.len(),
            "dropped": projection.baseline.deliveries.iter().filter(|d| d.result == strata_simulation::engine::DeliveryResult::Dropped).count(),
        },
        "branch": {
            "trace_hash": projection.branch_hash,
            "final_hash": projection.branch.final_hash,
            "accepted": projection.branch.accepted(),
            "rejected": projection.branch.rejected(),
        },
        "trace_replay": { "steps": check.steps, "agreed": check.agreed, "final_hash_matches": check.final_hash_matches },
        "timeline": timeline,
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
    let faults = if request.faults {
        Faults {
            commit_conflict: 100_000,
            delivery_drop: 200_000,
        }
    } else {
        Faults::default()
    };
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
                faults,
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
                faults,
            })
        }
        other => Err(ApiError::new(
            ErrorCode::Invalid,
            format!("unknown schedule source {other:?}"),
        )),
    }
}

/// Start a projection in the background and return its id. Progress and the result are sent to
/// the caller's WebSocket connections; the result can also be fetched by id.
pub fn start(
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
                let result = present(&projection, &scenario, &request, snapshot);
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

/// Replay the tenant's journal of committed transitions from nothing and compare with the live
/// state.
pub fn replay_live(app: &App, identity: &Identity) -> Result<Json, ApiError> {
    let live = complete(app, &identity.tenant)?;
    let entries: Vec<Recorded> = journal_through(app, &identity.tenant, u64::MAX)?
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
    let report = replay_sealed(app, &identity.tenant, &entries, Some(&live));
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
}
