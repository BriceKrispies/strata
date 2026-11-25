//! The retention sweep: the scheduled job that deletes the records whose retention has ended
//! (PRV-357, PRV-356, D-6584, WG-5116).
//!
//! **A recurring job per tenant, without a schedule.** The sweep is a chain of jobs in the tenant's
//! own job store (`JobWork::Retention`). Each job does one batch for one record type and, in the
//! report that ends it, creates the next job of the chain (`WorkerWrites::jobs`, one transaction
//! with the report): the next batch of the same type when this one was full, the next type when it
//! was not, and, after the last type, the first type again a day on ([`INTERVAL_MS`]). So the sweep
//! recurs by itself. [`ensure_scheduled`] starts a chain for a tenant that has none; the janitor
//! calls it for every tenant, so a new tenant, or a chain that failed for good, is picked up.
//!
//! **Finding the records.** A type whose rule counts from a timestamp field walks the index of that
//! field, oldest first, up to the cutoff, so a batch holds only records past their period. A type
//! whose rule counts from the creation or the last change has no index of its own: its batches are
//! pages in record id order examined through the cursor, and a batch of at most
//! `strata_kernel::erasure::BATCH` records is examined at a time (`strata_kernel::retention`).
//!
//! **Deleting them.** The records of a batch that nothing stops are deleted by the kernel through
//! `Intent::ExpireRecords`, as the system retention principal, in one commit with one
//! `record-deleted` event each (before image without personal fields). What stops a record: a legal
//! hold on its type or on a person it concerns, a restriction of processing of a person it
//! concerns (`subject_rights::Rights::blocks_retention`); the record stays until the hold is
//! released or the restriction lifted. The kernel also keeps a record that another record still
//! refers to. Another tenant's job store and records are never read.
//!
//! The same chain removes the exports that have expired (`subject_export::purge_expired`) at the
//! end of each sweep.
//!
//! Implements: PRV-357

use crate::app::{ApiError, App, LOG};
use crate::ops;
use crate::subject_rights::Rights;
use crate::worker::steps::Outcome;
use serde_json::json;
use strata_kernel::retention::{due_need, due_page};
use strata_kernel::rules::Lookup;
use strata_kernel::{
    Context, Intent, ObjectTypeDef, ObjectTypeId, Output, Principal, TenantId, Timestamp,
};
use strata_protocol::api::ErrorCode;
use strata_storage_api::jobs::{JobReport, JobWork, Lease, NewJob, RetryPolicy, WorkerWrites};

/// The time between two sweeps of a tenant: a day, in milliseconds.
pub const INTERVAL_MS: i64 = 86_400_000;

/// How many jobs of a tenant [`ensure_scheduled`] reads to find the chain.
const LOOK_BACK: usize = 1_000;

/// Start the chain of a tenant that has none, due `now`. Returns whether it created a job.
pub fn ensure_scheduled(app: &App, tenant: &TenantId, now: Timestamp) -> Result<bool, ApiError> {
    let jobs = app.store.jobs_list(tenant, None, LOOK_BACK)?;
    let mut open = false;
    let mut latest: Option<u64> = None;
    for job in &jobs {
        if let JobWork::Retention { step, .. } = &job.work {
            open |= !job.state.ended();
            latest = Some(latest.map_or(*step, |seen| seen.max(*step)));
        }
    }
    if open {
        return Ok(false);
    }
    let step = latest.map_or(0, |seen| seen + 1);
    let made = app.store.jobs_create(
        tenant,
        &[NewJob {
            key: format!("retention:{step}"),
            work: JobWork::Retention {
                step,
                object: None,
                after: None,
            },
            due_at: now,
            policy: RetryPolicy::default(),
        }],
        now,
    )?;
    Ok(!made.is_empty())
}

/// [`ensure_scheduled`] for every tenant; a tenant that fails is logged and the others go on.
pub fn ensure_all(app: &App) {
    let now = app.now();
    let tenants = match app.store.tenants() {
        Ok(tenants) => tenants,
        Err(error) => {
            LOG.error(
                "retention.tenants_failed",
                "",
                json!({ "error": error.to_string() }),
            );
            return;
        }
    };
    for summary in tenants {
        if let Err(error) = ensure_scheduled(app, &summary.id, now) {
            LOG.error(
                "retention.schedule_failed",
                "",
                json!({ "tenant": summary.id.as_str(), "error": error.describe() }),
            );
        }
    }
}

/// What one batch did, and where the chain goes next.
struct Done {
    object: Option<ObjectTypeId>,
    after: Option<strata_kernel::query_plan::IndexPosition>,
    due_at: Timestamp,
    deleted: usize,
    kept: usize,
    held: usize,
}

/// Run one job of the chain.
pub fn run(app: &App, tenant: &TenantId, lease: &Lease, now: Timestamp) -> Outcome {
    let JobWork::Retention {
        step,
        object,
        after,
    } = &lease.job.work
    else {
        return Outcome {
            report: JobReport::Succeeded,
            writes: WorkerWrites::default(),
        };
    };
    match batch(app, tenant, *step, object.as_ref(), after.as_ref(), now) {
        Ok(done) => {
            if done.deleted + done.kept + done.held > 0 {
                app.audit(
                    "retention.swept",
                    &format!("retention:{step}"),
                    json!({
                        "tenant": tenant.as_str(),
                        "deleted": done.deleted,
                        "kept": done.kept,
                        "held": done.held,
                    }),
                );
            }
            let mut writes = WorkerWrites::default();
            writes.jobs.push(NewJob {
                key: format!("retention:{}", step + 1),
                work: JobWork::Retention {
                    step: step + 1,
                    object: done.object,
                    after: done.after,
                },
                due_at: done.due_at,
                policy: RetryPolicy::default(),
            });
            Outcome {
                report: JobReport::Succeeded,
                writes,
            }
        }
        Err(error) => Outcome {
            report: JobReport::Failed {
                error: strata_storage_api::jobs::clip_error(&error.describe()),
                retry: !lease.exhausted,
            },
            writes: WorkerWrites::default(),
        },
    }
}

/// The types of the tenant that declare a retention rule, in id order.
fn swept(schema: &strata_kernel::TenantSchema) -> Vec<&ObjectTypeDef> {
    schema
        .objects
        .values()
        .filter(|object| object.retention.is_some())
        .collect()
}

fn batch(
    app: &App,
    tenant: &TenantId,
    step: u64,
    object: Option<&ObjectTypeId>,
    after: Option<&strata_kernel::query_plan::IndexPosition>,
    now: Timestamp,
) -> Result<Done, ApiError> {
    let header = app.store.load(tenant, &[])?;
    let Some(state) = header.tenant else {
        return Err(ApiError::new(ErrorCode::NotFound, "No such tenant."));
    };
    let types = swept(&state.schema);
    // The next sweep, a day on, when this one is over.
    let finished = |deleted, kept, held| {
        let _ = crate::subject_export::purge_expired(app, tenant, now);
        Done {
            object: None,
            after: None,
            due_at: Timestamp(now.0 + INTERVAL_MS),
            deleted,
            kept,
            held,
        }
    };
    let position = match object {
        None => Some(0),
        Some(wanted) => types.iter().position(|t| t.id == *wanted),
    };
    // A type that lost its rule since the last batch ends the sweep; the next one starts over.
    let Some(index) = position.filter(|i| *i < types.len()) else {
        return Ok(finished(0, 0, 0));
    };
    let def = types[index];
    let after = if object.is_some() { after } else { None };
    let Some(need) = due_need(def, now, after) else {
        return Ok(finished(0, 0, 0));
    };
    let loaded = app.store.load(tenant, &[need])?;
    let Some(snapshot) = loaded.tenant.as_ref() else {
        return Err(ApiError::new(ErrorCode::NotFound, "No such tenant."));
    };
    let lookup = Lookup {
        tenant: snapshot,
        coverage: &loaded.coverage,
    };
    let page = due_page(lookup, def, now, after)
        .map_err(|e| ApiError::hidden(ErrorCode::Internal, format!("{e:?}")))?;

    let rights = Rights::load(app, tenant)?;
    let mut ids = Vec::new();
    let mut held = 0;
    for record in &page.due {
        if rights.blocks_retention(def, record).is_some() {
            held += 1;
        } else {
            ids.push(record.id);
        }
    }
    let (mut deleted, mut kept) = (0, 0);
    if !ids.is_empty() {
        let ctx = Context {
            tenant: tenant.clone(),
            principal: Principal::System,
            session: None,
            now,
            correlation: format!("retention:{step}"),
            verified_address: None,
            idempotency_key: None,
            language: None,
            hop: None,
            agent: None,
        };
        let intent = Intent::ExpireRecords {
            object: def.id.clone(),
            ids,
        };
        let done = ops::run_intent(app, &ctx, &intent, false)?;
        match done.transition.output {
            Output::Expired {
                deleted: gone,
                kept: stayed,
                ..
            } => {
                deleted = gone.len();
                kept = stayed.len();
            }
            other => {
                return Err(ApiError::hidden(
                    ErrorCode::Internal,
                    format!("unexpected output of a retention sweep: {other:?}"),
                ))
            }
        }
    }

    if page.full {
        return Ok(Done {
            object: Some(def.id.clone()),
            after: page.last,
            due_at: now,
            deleted,
            kept,
            held,
        });
    }
    match types.get(index + 1) {
        Some(next) => Ok(Done {
            object: Some(next.id.clone()),
            after: None,
            due_at: now,
            deleted,
            kept,
            held,
        }),
        None => Ok(finished(deleted, kept, held)),
    }
}
