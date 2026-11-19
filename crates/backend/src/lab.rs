//! Impact Lab: projections, journal replay and verification runs.
//!
//! Everything here reads the store and never writes it. A projection loads a complete snapshot,
//! hands it to the simulator, and the simulator works on that copy with the same kernel the
//! backend uses. Live tenant state cannot be changed from this module: it calls no `commit`.

use crate::app::{ApiError, App, Shared, LOG};
use crate::auth::Identity;
use crate::hub::RunMessage;
use serde::Deserialize;
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use strata_kernel::{ExistingData, Principal, PrincipalId, TenantId, TenantState, World};
use strata_protocol::api::ErrorCode;
use strata_simulation::engine::{replay as replay_trace, Faults, Scenario, Scheduled};
use strata_simulation::journal::{replay_journal, Recorded};
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
}

struct Run {
    tenant: TenantId,
    principal: PrincipalId,
    status: &'static str,
    started_at: i64,
    summary: Json,
    result: Option<Json>,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct Lab {
    runs: Mutex<BTreeMap<String, Run>>,
    order: Mutex<Vec<String>>,
    verification: Mutex<Option<Json>>,
}

const KEPT_RUNS: usize = 12;

impl Lab {
    pub fn new() -> Self {
        Self::default()
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
            json!({ "id": id, "status": run.status, "summary": run.summary, "result": run.result }),
        )
    }

    fn insert(&self, id: &str, run: Run) {
        if let (Ok(mut runs), Ok(mut order)) = (self.runs.lock(), self.order.lock()) {
            runs.insert(id.to_string(), run);
            order.push(id.to_string());
            while order.len() > KEPT_RUNS {
                let oldest = order.remove(0);
                runs.remove(&oldest);
            }
        }
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
    let journal = app.store.journal_after(&identity.tenant, 0, 100_000)?.len();
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
        "verification": app.lab.verification.lock().ok().and_then(|v| v.clone()),
        "recent_runs": app.lab.recent().into_iter().filter(|r| r["tenant"] == identity.tenant.as_str()).collect::<Vec<_>>(),
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
        },
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

fn build_scenario(
    app: &App,
    identity: &Identity,
    request: &RunRequest,
    live: &Arc<TenantState>,
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
            // Every tenant is copied into the simulated world, so that the comparison can show
            // the others are untouched.
            let mut world = World::default();
            for summary in app.store.tenants()? {
                if let Some(state) = app.store.load_complete(&summary.id)?.tenant {
                    world.tenants.insert(summary.id, state);
                }
            }
            world.tenants.insert(identity.tenant.clone(), live.clone());
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
            let schedule: Vec<Scheduled> = app
                .store
                .journal_after(&identity.tenant, 0, 100_000)?
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
    let changes = proposal::parse(&live.schema, &request.change)
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
    let scenario = build_scenario(app, identity, &request, &live)?;
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
        "revision": live.revision.0,
        "schema_version": live.schema.version.0,
        "records": live.record_count(),
        "state_hash": strata_kernel::canon::hash_of(&*live),
    });

    let steps = scenario.schedule.len();
    let id = format!("run_{}", crate::auth::random_hex(6)?);
    let cancel = Arc::new(AtomicBool::new(false));
    let summary = json!({ "change": request.change, "source": request.source, "steps": scenario.schedule.len() });
    app.lab.insert(
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
    );
    LOG.info("lab.started", &identity.correlation, json!({ "run": id, "tenant": identity.tenant.as_str(), "source": request.source, "steps": scenario.schedule.len() }));

    let (shared, run_id, tenant, correlation) = (
        shared.clone(),
        id.clone(),
        identity.tenant.clone(),
        identity.correlation.clone(),
    );
    std::thread::spawn(move || {
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
                LOG.info("lab.cancelled", &correlation, json!({ "run": run_id }));
                send("cancelled", json!({}));
            }
        }
    });
    Ok(json!({ "run": id, "steps": steps, "apply_at": before_step }))
}

/// Replay the tenant's journal of committed transitions from nothing and compare with the live
/// state.
pub fn replay_live(app: &App, identity: &Identity) -> Result<Json, ApiError> {
    let live = complete(app, &identity.tenant)?;
    let entries: Vec<Recorded> = app
        .store
        .journal_after(&identity.tenant, 0, 100_000)?
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
    let report = replay_journal(&identity.tenant, &entries, Some(&live));
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
/// background. The report arrives over the WebSocket and is kept for the overview.
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
    std::thread::spawn(move || {
        let report = strata_verification::lean::verify(&lean_dir, now);
        LOG.info("lab.verified", &correlation, json!({ "all_checked": report.all_checked, "theorems": report.theorems.len(), "problem": report.problem }));
        let payload = json!(report);
        if let Ok(mut slot) = shared.lab.verification.lock() {
            *slot = Some(payload.clone());
        }
        let _ = shared.hub.runs.send(RunMessage {
            tenant,
            principal,
            run,
            kind: "verification".into(),
            payload,
        });
    });
    Ok(json!({ "run": id }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_kernel::{SessionId, Timestamp};
    use strata_protocol::ws::SessionState;
    use strata_storage_api::SessionRecord;

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
}
