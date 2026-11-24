//! The staged job of a product upgrade over the cap (D-DS-16, WG-5472): the approved stages are
//! committed one at a time under the approving owner's recorded approval, which is bounded in the
//! number of stages and in time by the tenant's configuration; a stopped job is resumed only by an
//! owner with step-up; each stage re-checks the remaining plan against the approved diff; and a
//! job is abandoned by its owner (step-up), by a pre-authorisation for a security upgrade, or
//! automatically once its approval period has passed. Abandon removes what the stages created,
//! the staging role included, and restores the record at the version installed before the job.
//!
//! This module decides and orders; the backend supplies the commits, the stored job and the
//! notifications. The job is held in memory here and is serialised by its caller.
//!
//! Implements: D-DS-16, WG-5472.

use crate::product::{BundleError, BundleErrorKind, InstalledRecord, OwnedEntity};
use crate::stages::{staging_role, Stage, StagePlan};
use strata_kernel::*;

/// The approval an owner records for a staged job: who approved it, for how many stages and until
/// when. The tenant's configuration bounds both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approval {
    pub owner: String,
    pub stages_allowed: usize,
    pub valid_until: i64,
}

/// The tenant's configuration of staged jobs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StagingPolicy {
    /// The most stages one approval covers.
    pub max_stages: usize,
    /// How long an approval stands, in milliseconds from the approval.
    pub approval_period_ms: i64,
    /// Whether the owner has pre-authorised "abandon then apply" for security upgrades.
    pub abandon_security_upgrades: bool,
}

/// Where a job is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobState {
    /// Committing stages under an approval.
    Running,
    /// The approval ran out of stages or time before the plan ended; only a resume continues it.
    Stopped { since: i64 },
    /// The last stage committed; the product records the target version.
    Done,
    /// Abandoned: what the stages created is removed and the record restored.
    Abandoned { reason: String },
}

/// A staged job for one product.
#[derive(Clone, Debug, PartialEq)]
pub struct StagedJob {
    pub product: String,
    /// The record installed before the job: abandon restores it.
    pub installed: InstalledRecord,
    /// The approved plan. Its last stage records the target version.
    pub plan: StagePlan,
    /// Whether the upgrade is tagged security (pre-authorised abandon applies to it).
    pub security: bool,
    /// How many stages have committed.
    pub committed: usize,
    pub approval: Approval,
    pub state: JobState,
}

/// Who asks for something: the owner, with a session that has authenticated recently, or no one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Authority {
    pub principal: String,
    pub is_owner: bool,
    pub stepped_up: bool,
}

/// What a commit of one stage reports when it fails.
#[derive(Debug, PartialEq, Eq)]
pub enum StepFailure {
    /// Another change committed first: the stage is planned again once.
    SchemaConflict,
    /// The kernel refused the stage.
    Refused(String),
}

/// Why a staged step did not happen.
#[derive(Debug, PartialEq, Eq)]
pub enum StagedError {
    /// `product.base-moved`: the remaining plan no longer matches the approved diff.
    BaseMoved(String),
    /// `upgrade.in-progress`: the product has a staged job, which only its owner may abandon.
    InProgress(String),
    /// The caller may not resume or abandon the job (not an owner, or no step-up).
    NotAllowed(String),
    /// A stage the kernel refused.
    Refused(String),
    /// The job is not in the state the call needs.
    WrongState(String),
}

impl StagedError {
    /// The error as the bundle error the routes answer with, when there is one.
    pub fn bundle_error(&self) -> Option<BundleError> {
        match self {
            StagedError::BaseMoved(m) => Some(BundleError {
                kind: BundleErrorKind::BaseMoved,
                message: format!("product.base-moved: {m}"),
            }),
            StagedError::InProgress(m) => Some(BundleError {
                kind: BundleErrorKind::InProgress,
                message: format!("upgrade.in-progress: {m}"),
            }),
            _ => None,
        }
    }
}

/// What a finished call tells its caller to record and to notify.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub product: String,
    pub detail: String,
}

/// Starts a job for an approved plan. The plan's stages are the approved diff.
pub fn start(
    plan: StagePlan,
    installed: InstalledRecord,
    approval: Approval,
    security: bool,
) -> StagedJob {
    StagedJob {
        product: installed.product.clone(),
        installed,
        plan,
        security,
        committed: 0,
        approval,
        state: JobState::Running,
    }
}

/// Refuses a change to a product that has a staged job still open (`upgrade.in-progress`). An
/// abandon is not refused by this; it is the way out.
pub fn refuse_while_staging(job: Option<&StagedJob>) -> Result<(), StagedError> {
    match job {
        Some(job) if !matches!(job.state, JobState::Done | JobState::Abandoned { .. }) => {
            Err(StagedError::InProgress(format!(
                "the product {} is being upgraded in stages; the owner may abandon the upgrade",
                job.product
            )))
        }
        _ => Ok(()),
    }
}

/// Commits the approved stages in order, while the approval stands. `replan` plans the upgrade
/// again against the current tenant (`None` when it now fits one commit); `commit` runs one stage.
/// Each stage first checks that the remaining plan matches the approved one. A `SchemaConflict` is
/// planned again once; a second conflict, or a different plan, is `product.base-moved`.
pub fn run(
    job: &mut StagedJob,
    now: i64,
    replan: &mut dyn FnMut(&StagedJob) -> Result<Option<StagePlan>, BundleError>,
    commit: &mut dyn FnMut(&Stage) -> Result<(), StepFailure>,
) -> Result<(), StagedError> {
    if !matches!(job.state, JobState::Running) {
        return Err(StagedError::WrongState(format!(
            "the job of {} is not running",
            job.product
        )));
    }
    while job.committed < job.plan.stages.len() {
        if now > job.approval.valid_until {
            job.state = JobState::Stopped { since: now };
            return Ok(());
        }
        if job.committed >= job.approval.stages_allowed {
            job.state = JobState::Stopped { since: now };
            return Ok(());
        }
        check_remaining(job, replan)?;
        let stage = job.plan.stages[job.committed].clone();
        match commit(&stage) {
            Ok(()) => {}
            Err(StepFailure::SchemaConflict) => {
                check_remaining(job, replan)?;
                match commit(&stage) {
                    Ok(()) => {}
                    Err(StepFailure::SchemaConflict) => {
                        return Err(StagedError::BaseMoved(format!(
                            "the stage {} conflicted twice; the upgrade is planned again",
                            stage.version
                        )))
                    }
                    Err(StepFailure::Refused(m)) => return Err(StagedError::Refused(m)),
                }
            }
            Err(StepFailure::Refused(m)) => return Err(StagedError::Refused(m)),
        }
        job.committed += 1;
    }
    job.state = JobState::Done;
    Ok(())
}

/// The re-check of a stage: the plan made now must hold the same stages from the next one on as
/// the approved plan does (`product.base-moved` otherwise).
fn check_remaining(
    job: &StagedJob,
    replan: &mut dyn FnMut(&StagedJob) -> Result<Option<StagePlan>, BundleError>,
) -> Result<(), StagedError> {
    let fresh = replan(job).map_err(|e| StagedError::BaseMoved(e.message))?;
    let Some(fresh) = fresh else {
        return Err(StagedError::BaseMoved(
            "the upgrade fits one commit now, so it is not the approved plan".to_string(),
        ));
    };
    let approved = &job.plan.stages[job.committed..];
    let now = &fresh.stages[job.committed.min(fresh.stages.len())..];
    if approved.len() != now.len()
        || approved
            .iter()
            .zip(now)
            .any(|(a, b)| a.version != b.version || a.changes != b.changes)
    {
        return Err(StagedError::BaseMoved(format!(
            "the remaining stages of {} differ from the approved diff",
            job.product
        )));
    }
    Ok(())
}

/// Resumes a stopped job under a new approval. Only an owner with step-up may resume it.
pub fn resume(
    job: &mut StagedJob,
    authority: &Authority,
    approval: Approval,
    now: i64,
) -> Result<(), StagedError> {
    if !authority.is_owner || !authority.stepped_up {
        return Err(StagedError::NotAllowed(
            "resuming a staged upgrade needs an owner with step-up".to_string(),
        ));
    }
    if !matches!(job.state, JobState::Stopped { .. }) {
        return Err(StagedError::WrongState(format!(
            "the job of {} is not stopped",
            job.product
        )));
    }
    if approval.valid_until <= now {
        return Err(StagedError::NotAllowed(
            "the approval has already run out".to_string(),
        ));
    }
    job.approval = Approval {
        owner: authority.principal.clone(),
        ..approval
    };
    job.state = JobState::Running;
    Ok(())
}

/// Why an abandon happens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AbandonBy {
    /// The owner asked for it, with step-up (exempt from `upgrade.in-progress`).
    Owner(Authority),
    /// A security upgrade arrived while the job was stopped, under a pre-authorisation.
    PreAuthorised,
    /// The approval period passed while the job was stopped.
    Expired,
}

/// The schema changes that abandon a job, in commits of at most the cap. Each commit removes some
/// of what the stages created, in dependency order (operations, then queries, views, workflows and
/// templates, then object types and the staging role); the last commit restores the record at the
/// version installed before the job. Intermediate records are at `<target>-abandon.<n>`.
pub fn abandon_plan(
    job: &StagedJob,
    by: &AbandonBy,
) -> Result<Vec<Vec<SchemaChange>>, StagedError> {
    if let AbandonBy::Owner(authority) = by {
        if !authority.is_owner || !authority.stepped_up {
            return Err(StagedError::NotAllowed(
                "abandoning a staged upgrade needs an owner with step-up".to_string(),
            ));
        }
    }
    let target = job
        .plan
        .stages
        .last()
        .map(|s| s.version.clone())
        .unwrap_or_default();
    let staging = RoleId::new(staging_role(&job.product))
        .map_err(|e| StagedError::Refused(format!("the staging role name: {e:?}")))?;

    // What the committed stages created, in removal order, then the staging role.
    let mut created: Vec<OwnedEntity> = job
        .plan
        .stages
        .iter()
        .take(job.committed)
        .flat_map(|s| s.created.iter().cloned())
        .collect();
    created.sort_by_key(|e| removal_rank(&e.kind));
    let mut removals: Vec<(String, SchemaChange)> = Vec::new();
    for entity in &created {
        removals.push((
            format!("{}:{}", entity.kind, entity.id),
            removal_of(entity)?,
        ));
    }
    // The staging role exists once the first stage has committed; a job stopped before that has
    // created nothing to remove.
    let staging_defined = job.committed > 0;
    if staging_defined {
        removals.push((
            format!("role:{staging}"),
            SchemaChange::RemoveRole(staging.clone()),
        ));
    }

    // Every commit keeps the record owning what is still there: the installed entities, the
    // created ones, and the staging role, less what the commits so far have removed.
    let mut owned: Vec<OwnedEntity> = job.installed.owns.clone();
    owned.extend(created.iter().cloned());
    if staging_defined {
        owned.push(OwnedEntity {
            kind: "role".to_string(),
            id: staging.0.clone(),
        });
    }

    let per = strata_kernel::limits::MAX_SCHEMA_CHANGES - 1;
    let mut commits: Vec<Vec<SchemaChange>> = Vec::new();
    for (index, chunk) in removals.chunks(per).enumerate() {
        let gone: Vec<&str> = chunk.iter().map(|(key, _)| key.as_str()).collect();
        owned.retain(|o| !gone.contains(&format!("{}:{}", o.kind, o.id).as_str()));
        let record = InstalledRecord {
            product: job.product.clone(),
            version: format!("{target}-abandon.{}", index + 1),
            platform: job.installed.platform.clone(),
            owns: owned.clone(),
            schema_version: None,
        };
        let mut changes: Vec<SchemaChange> = chunk.iter().map(|(_, c)| c.clone()).collect();
        changes.push(SchemaChange::RecordProduct(
            record.to_kernel().map_err(StagedError::Refused)?,
        ));
        commits.push(changes);
    }
    // The last commit restores the record installed before the job.
    let restored = job.installed.to_kernel().map_err(StagedError::Refused)?;
    // Nothing committed means nothing is written: the record is already the installed one.
    if let Some(last) = commits.last_mut() {
        last.pop();
        last.push(SchemaChange::RecordProduct(restored));
    }
    Ok(commits)
}

fn removal_of(entity: &OwnedEntity) -> Result<SchemaChange, StagedError> {
    let bad = |e: String| StagedError::Refused(format!("{}: {e}", entity.id));
    Ok(match entity.kind.as_str() {
        "operation" => SchemaChange::RemoveOperation(
            TenantOperationId::new(entity.id.clone()).map_err(|e| bad(format!("{e:?}")))?,
        ),
        "query" => SchemaChange::RemoveQuery(
            QueryId::new(entity.id.clone()).map_err(|e| bad(format!("{e:?}")))?,
        ),
        "view" => SchemaChange::RemoveView(
            ViewId::new(entity.id.clone()).map_err(|e| bad(format!("{e:?}")))?,
        ),
        "workflow" => SchemaChange::RemoveWorkflow(
            WorkflowId::new(entity.id.clone()).map_err(|e| bad(format!("{e:?}")))?,
        ),
        "mail-template" => SchemaChange::RemoveMailTemplate(TemplateId(entity.id.clone())),
        "notice-template" => SchemaChange::RemoveNoticeTemplate(TemplateId(entity.id.clone())),
        "object" => SchemaChange::RemoveObject {
            id: ObjectTypeId::new(entity.id.clone()).map_err(|e| bad(format!("{e:?}")))?,
            delete_records: true,
        },
        other => return Err(bad(format!("a {other} is not a staged entity"))),
    })
}

/// Removal order: what refers to others first.
fn removal_rank(kind: &str) -> u8 {
    match kind {
        "operation" => 0,
        "view" => 1,
        "query" => 2,
        "workflow" => 3,
        "mail-template" | "notice-template" => 4,
        "object" => 5,
        _ => 6,
    }
}

/// Whether a job approved under `policy` is abandoned by a pre-authorisation for an arriving
/// upgrade: a stopped security job, when the owner pre-authorised it.
pub fn pre_authorised_abandon(
    job: &StagedJob,
    policy: &StagingPolicy,
    incoming_security: bool,
) -> bool {
    policy.abandon_security_upgrades
        && incoming_security
        && matches!(job.state, JobState::Stopped { .. })
}

/// Abandons a stopped job whose approval period has passed (`Expired`). Returns whether it did.
pub fn expire(job: &StagedJob, now: i64) -> bool {
    matches!(job.state, JobState::Stopped { .. }) && now > job.approval.valid_until
}

/// Marks a job abandoned once its commits have run, with what it recorded and whom to notify.
pub fn mark_abandoned(job: &mut StagedJob, by: &AbandonBy) -> Notice {
    let reason = match by {
        AbandonBy::Owner(a) => format!("abandoned by {}", a.principal),
        AbandonBy::PreAuthorised => {
            "abandoned for a security upgrade under the pre-authorisation".to_string()
        }
        AbandonBy::Expired => "abandoned automatically after the approval period".to_string(),
    };
    job.state = JobState::Abandoned {
        reason: reason.clone(),
    };
    Notice {
        product: job.product.clone(),
        detail: reason,
    }
}
