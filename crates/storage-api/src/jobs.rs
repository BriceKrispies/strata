//! The jobs of the automation worker, and the run, step and exception records it keeps
//! (component `automation-worker`, SCH-001 to SCH-015, D-6520 to D-6539).
//!
//! A job is one unit of due work of one tenant: starting a run of an automation, or one attempt
//! of one step of a run (D-526). It is kept in the tenant's own database and created, leased,
//! reported, retried, cancelled and dead-lettered through the [`crate::Store`] operations, whose
//! rules live here as pure functions so that every adapter applies the same ones:
//!
//! - **Creation is idempotent by key** (SCH-004, EML-005): a job is created once for its key,
//!   whether in the commit that causes it ([`crate::Commit::jobs`]) or alone; its id is derived
//!   from the tenant and the key ([`job_id`]), so a repeat names the same job.
//! - **A lease carries a fencing token and an expiry** (SCH-010, SCH-011): every lease raises the
//!   token, and a report is accepted only with the current token, before the lease expires
//!   ([`apply_report`]).
//! - **Exactly one outcome** (SCH-012): a job that is completed, failed or cancelled is never
//!   leased or reported again; only an owner's retry gives a failed job a new attempt.
//! - **Retries grow and are bounded** (SCH-013, SCH-014, D-6529): a failed attempt is due again
//!   after its first delay doubled for each earlier attempt, up to the maximum delay
//!   ([`retry_delay_ms`]); the error of every attempt is kept, and the attempt that spends the
//!   policy dead-letters the job with the reason `attempts-exhausted`.
//!
//! What a job does is the worker's: the store keeps [`JobWork`] as it was given.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use strata_kernel::triggers::{AutomationDecline, AutomationStart};
use strata_kernel::{
    canon, Effect, PrincipalId, SchemaVersion, StepName, TenantId, Timestamp, Transition,
    TriggerId, Value, WorkflowId,
};

/// The attempts of a policy when the definition names none (D-6529).
pub const DEFAULT_ATTEMPTS: u32 = 5;
/// The delay before the second attempt when the definition names none: ten seconds (D-6529).
pub const DEFAULT_FIRST_DELAY_MS: i64 = 10_000;
/// The longest delay between attempts when the definition names none: one hour (D-6529).
pub const DEFAULT_MAX_DELAY_MS: i64 = 3_600_000;
/// How long one attempt may hold its lease when the definition names no timeout: sixty seconds
/// (D-6529).
pub const DEFAULT_LEASE_MS: i64 = 60_000;
/// The longest a job key may be, in bytes.
pub const MAX_JOB_KEY_LEN: usize = 255;
/// The longest error text kept for one attempt, in bytes. Longer text is cut at a character
/// boundary.
pub const MAX_ERROR_LEN: usize = 512;

/// Where a job is (spec type `job-state`, without `unscheduled`, which this slice does not use).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JobState {
    /// Due at `due_at`, waiting for a worker.
    Waiting,
    /// Held by a worker until `due_at`, the expiry of its lease.
    Leased,
    Completed,
    /// Ended without success: its attempts are spent, or its failure was final.
    Failed,
    Cancelled,
}

impl JobState {
    /// The state as the specification writes it.
    pub fn as_str(&self) -> &'static str {
        match self {
            JobState::Waiting => "waiting",
            JobState::Leased => "leased",
            JobState::Completed => "completed",
            JobState::Failed => "failed",
            JobState::Cancelled => "cancelled",
        }
    }

    /// Whether the job has its one outcome (SCH-012).
    pub fn ended(&self) -> bool {
        matches!(
            self,
            JobState::Completed | JobState::Failed | JobState::Cancelled
        )
    }

    /// The state named by `text`, as [`JobState::as_str`] writes it.
    pub fn parse(text: &str) -> Option<JobState> {
        Some(match text {
            "waiting" => JobState::Waiting,
            "leased" => JobState::Leased,
            "completed" => JobState::Completed,
            "failed" => JobState::Failed,
            "cancelled" => JobState::Cancelled,
            _ => return None,
        })
    }
}

/// How the attempts of a job are retried and how long one may hold its lease (D-6529).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Attempts in all, at least one.
    pub attempts: u32,
    pub first_delay_ms: i64,
    pub max_delay_ms: i64,
    /// How long one attempt holds its lease: the timeout of the step.
    pub lease_ms: i64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            attempts: DEFAULT_ATTEMPTS,
            first_delay_ms: DEFAULT_FIRST_DELAY_MS,
            max_delay_ms: DEFAULT_MAX_DELAY_MS,
            lease_ms: DEFAULT_LEASE_MS,
        }
    }
}

/// The delay before the attempt after attempt `attempt` (counted from one) failed: the first
/// delay, doubled for each attempt after the first, never more than the maximum delay (SCH-013).
pub fn retry_delay_ms(policy: &RetryPolicy, attempt: u32) -> i64 {
    let first = policy.first_delay_ms.max(0);
    let max = policy.max_delay_ms.max(first);
    let doublings = attempt.saturating_sub(1).min(40);
    first.saturating_mul(1_i64 << doublings).min(max)
}

/// What a job does. The store keeps it as it was given and never reads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JobWork {
    /// Start a run of an automation for a committed event that one of its triggers matched
    /// (SCH-004, WFL-1521). The run's id is the job's id, so a repeated start starts nothing.
    StartRun {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trigger: Option<TriggerId>,
        automation: WorkflowId,
        /// The sequence number of the event that started the run.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_seq: Option<u64>,
        /// The hop count of that event (D-6531).
        hop: u64,
        /// The parameters the event binds (D-6523).
        #[serde(default)]
        params: BTreeMap<String, Value>,
        /// Who caused the event the trigger matched (`automation-start.caused-by`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caused_by: Option<PrincipalId>,
        /// The person the automation acts for: its definer (D-7204).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_behalf_of: Option<PrincipalId>,
        /// The schema version the trigger was decided under.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        definition_version: Option<SchemaVersion>,
        /// The run key the automation's expression gave (WFL-1522): one run per key.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_key: Option<String>,
    },
    /// Run one step of a run: one job per step, retried as attempts of the job (D-526).
    Step {
        run: String,
        automation: WorkflowId,
        step: StepName,
        /// The position of the step in its run, from one.
        sequence: u64,
    },
    /// One batch of the retention sweep of a tenant (PRV-357): the records of one type that are
    /// past their retention, deleted as the system retention principal. The job creates the next
    /// batch, or the next sweep a day on, in the report that ends it (`WorkerWrites::jobs`), so the
    /// sweep recurs without a schedule. The cursor is the type being swept and the position after
    /// which it continues.
    Retention {
        /// The number of this batch in the chain of the tenant, from zero.
        step: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        object: Option<strata_kernel::ObjectTypeId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<strata_kernel::query_plan::IndexPosition>,
    },
    /// One batch of the export of the data of a subject (CT-210, PRV-350): the records the cursor
    /// names are read, kept as a part of the export, and the next batch is created in the report;
    /// the last one assembles the document and ends the request.
    SubjectExport {
        /// The id of the export request.
        export: String,
        subject: PrincipalId,
        /// The number of this batch, from zero: the key of its part.
        seq: u64,
        cursor: strata_kernel::subject_export::ExportCursor,
    },
}

/// A job to create: its key, what it does, when it is due and how it is retried.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewJob {
    /// The job is created once for this key within its tenant.
    pub key: String,
    pub work: JobWork,
    pub due_at: Timestamp,
    #[serde(default)]
    pub policy: RetryPolicy,
}

/// One job of a tenant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    /// Derived from the tenant and the key ([`job_id`]): a UUID in its text form.
    pub id: String,
    pub key: String,
    pub work: JobWork,
    pub state: JobState,
    /// When waiting, when it is due; when leased, when the lease expires; when ended, when it was
    /// last due.
    pub due_at: Timestamp,
    pub created_at: Timestamp,
    /// Attempts started.
    pub attempt: u32,
    pub policy: RetryPolicy,
    /// The token of the latest lease; 0 before the first.
    pub lease_token: u64,
    /// The worker that holds the lease, while it is leased.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<String>,
    /// The error of each failed attempt, in order (SCH-014).
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Timestamp>,
    /// Why it was cancelled or failed for good.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The order the job was created in within its tenant, from one: the cursor of a listing.
    /// Set by the store.
    #[serde(default)]
    pub seq: u64,
}

/// A lease on a job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    /// The job as leased: state `Leased`, `due_at` the expiry.
    pub job: Job,
    /// The fencing token a report must present.
    pub token: u64,
    pub expires_at: Timestamp,
    /// The previous lease of this job expired on its last allowed attempt: the worker is to fail
    /// it for good with the records that go with that (SCH-1526), not to run it again.
    pub exhausted: bool,
}

/// The outcome of one attempt, as the worker reports it (spec type `job-outcome`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobReport {
    Succeeded,
    /// The attempt failed. With `retry`, it is retried while the policy allows; without, the job
    /// fails for good now.
    Failed {
        error: String,
        retry: bool,
    },
}

/// What became of a report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reported {
    /// The outcome was recorded; the job as it is now.
    Recorded(Job),
    /// The report was refused and nothing was written (SCH-011): the reason.
    Refused(String),
}

/// What a cancel or a retry did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobChange {
    NotFound,
    /// The job was not in a state the change applies to; it is returned as it is.
    Unchanged(Job),
    Changed(Job),
}

/// Where a run is (a subset of spec type `run-state`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunState {
    Running,
    /// Waiting for the time of a wait step (WFL-1537).
    Waiting,
    Completed,
    Failed,
    Cancelled,
}

impl RunState {
    pub fn as_str(&self) -> &'static str {
        match self {
            RunState::Running => "running",
            RunState::Waiting => "waiting",
            RunState::Completed => "completed",
            RunState::Failed => "failed",
            RunState::Cancelled => "cancelled",
        }
    }

    pub fn parse(text: &str) -> Option<RunState> {
        Some(match text {
            "running" => RunState::Running,
            "waiting" => RunState::Waiting,
            "completed" => RunState::Completed,
            "failed" => RunState::Failed,
            "cancelled" => RunState::Cancelled,
            _ => return None,
        })
    }

    pub fn ended(&self) -> bool {
        matches!(
            self,
            RunState::Completed | RunState::Failed | RunState::Cancelled
        )
    }
}

/// One run of an automation (rec-workflow-run, automation-run-summary, WFL-1530, WFL-1531).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    pub run: String,
    pub automation: WorkflowId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<TriggerId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_seq: Option<u64>,
    pub state: RunState,
    /// The step it is at, or ended at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<StepName>,
    /// Steps run so far.
    pub steps: u64,
    /// The hop count of the event that started it, unchanged by waits (SCH-1533).
    pub hop: u64,
    /// The automation principal its steps act as (WFL-1530).
    pub caused_by: PrincipalId,
    /// The definer whose authority bounds the run (WFL-1530); `None` while the schema does not
    /// record who defined an automation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_behalf_of: Option<PrincipalId>,
    /// The parameters of the starting event and the outputs of the completed steps (D-6523).
    #[serde(default)]
    pub params: BTreeMap<String, Value>,
    pub started_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The order the run was first written in within its tenant: the cursor of a listing. Set by
    /// the store.
    #[serde(default)]
    pub seq: u64,
}

/// Where one step of a run is (spec type `step-run-state`, without `compensated`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepState {
    Due,
    Leased,
    Waiting,
    Completed,
    Failed,
    Cancelled,
}

/// One step of a run (rec-workflow-step-run, automation-step-summary, WFL-1531).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepRecord {
    pub run: String,
    /// The position of the step in its run, from one.
    pub sequence: u64,
    pub step: StepName,
    pub state: StepState,
    pub attempts: u32,
    /// The job that runs it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
    /// The step key every effect of the step carries (SCH-1524, D-6528).
    pub idempotency_key: String,
    /// The typed result of the completed step (automation-step-result), as JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// The error of each failed attempt, in order.
    #[serde(default)]
    pub errors: Vec<String>,
    pub started_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Timestamp>,
}

/// The kind of an exception record (spec type `automation-exception-kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExceptionKind {
    Declined,
    Held,
    Parked,
    DeadLettered,
}

impl ExceptionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExceptionKind::Declined => "declined",
            ExceptionKind::Held => "held",
            ExceptionKind::Parked => "parked",
            ExceptionKind::DeadLettered => "dead-lettered",
        }
    }

    pub fn parse(text: &str) -> Option<ExceptionKind> {
        Some(match text {
            "declined" => ExceptionKind::Declined,
            "held" => ExceptionKind::Held,
            "parked" => ExceptionKind::Parked,
            "dead-lettered" => ExceptionKind::DeadLettered,
            _ => return None,
        })
    }

    /// Whether the owner settles it (parked and dead-lettered); a declined or held record is
    /// settled when it is written (D-6530).
    pub fn settled_by_owner(&self) -> bool {
        matches!(self, ExceptionKind::Parked | ExceptionKind::DeadLettered)
    }
}

/// Declined, held, parked or dead-lettered work (rec-automation-exception, SCH-1532).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExceptionRecord {
    pub id: String,
    pub kind: ExceptionKind,
    /// One of the values of spec type `automation-exception-reason`.
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<TriggerId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automation: Option<WorkflowId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<StepName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hop: Option<u64>,
    pub at: Timestamp,
    /// Ids and limits only, never a field value; at most 256 bytes.
    pub detail: String,
    pub settled: bool,
    /// The order the record was written in within its tenant: the cursor of a listing. Set by
    /// the store.
    #[serde(default)]
    pub seq: u64,
}

/// What a report writes beside the job's outcome, in the same transaction: the run and step
/// records it changes, exception records, and the jobs that follow (the next step, the resumption
/// of a wait). A job of `jobs` whose key exists already is not created again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerWrites {
    #[serde(default)]
    pub runs: Vec<RunRecord>,
    #[serde(default)]
    pub steps: Vec<StepRecord>,
    #[serde(default)]
    pub exceptions: Vec<ExceptionRecord>,
    #[serde(default)]
    pub jobs: Vec<NewJob>,
}

/// What settling an exception did (contract settle-automation-exception).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Settled {
    NotFound,
    /// Not settled: the record is declined or held, already settled, or its job cannot be given
    /// another attempt. The reason.
    Conflict(String),
    /// Settled; the job that was given a new attempt, when it was released.
    Done {
        job: Option<String>,
    },
}

/// The id of the job of `key` in `tenant`: the first 128 bits of a SHA-256 of both, written as a
/// UUID (version 8, RFC 9562), so that a repeat of a key names the same job.
pub fn job_id(tenant: &TenantId, key: &str) -> String {
    derived_uuid(&format!("strata/job/v1\0{tenant}\0{key}"))
}

/// A UUID in text form derived from `seed` (version 8, variant RFC 9562).
pub fn derived_uuid(seed: &str) -> String {
    let hex = canon::hash_bytes(seed.as_bytes());
    let mut digits: Vec<char> = hex.chars().take(32).collect();
    digits[12] = '8';
    let variant = digits[16].to_digit(16).unwrap_or(0);
    digits[16] = char::from_digit(0x8 | (variant & 0x3), 16).unwrap_or('8');
    let text: String = digits.into_iter().collect();
    format!(
        "{}-{}-{}-{}-{}",
        &text[0..8],
        &text[8..12],
        &text[12..16],
        &text[16..20],
        &text[20..32]
    )
}

/// Error text as kept for one attempt: at most [`MAX_ERROR_LEN`] bytes.
pub fn clip_error(error: &str) -> String {
    if error.len() <= MAX_ERROR_LEN {
        return error.to_string();
    }
    let mut end = MAX_ERROR_LEN;
    while !error.is_char_boundary(end) {
        end -= 1;
    }
    error[..end].to_string()
}

/// Why a job cannot be created, or `None` when it can: its key is 1 to [`MAX_JOB_KEY_LEN`]
/// bytes and its policy allows at least one attempt and a positive lease.
pub fn job_problem(job: &NewJob) -> Option<String> {
    if job.key.is_empty() || job.key.len() > MAX_JOB_KEY_LEN {
        return Some(format!(
            "a job key is 1 to {MAX_JOB_KEY_LEN} bytes, not {}",
            job.key.len()
        ));
    }
    if job.policy.attempts == 0 {
        return Some(format!(
            "job {}: a policy allows at least one attempt",
            job.key
        ));
    }
    if job.policy.lease_ms <= 0 {
        return Some(format!("job {}: a lease lasts a positive time", job.key));
    }
    None
}

/// The job a [`NewJob`] becomes in `tenant` at `now`.
pub fn new_job(tenant: &TenantId, job: &NewJob, now: Timestamp) -> Job {
    Job {
        id: job_id(tenant, &job.key),
        key: job.key.clone(),
        work: job.work.clone(),
        state: JobState::Waiting,
        due_at: job.due_at,
        created_at: now,
        attempt: 0,
        policy: job.policy,
        lease_token: 0,
        worker: None,
        errors: Vec::new(),
        ended_at: None,
        reason: None,
        seq: 0,
    }
}

/// Whether a job may be leased at `now`: waiting and due, or leased with its lease expired
/// (SCH-001, SCH-010).
pub fn leasable(job: &Job, now: Timestamp) -> bool {
    matches!(job.state, JobState::Waiting | JobState::Leased) && job.due_at <= now
}

/// Lease a leasable job to `worker` at `now`: a new attempt, the next token, the expiry of the
/// policy's lease. A lease that expired counts as a failed attempt (SCH-1526); when it was the
/// last attempt the lease is marked exhausted, and the worker fails the job for good.
pub fn apply_lease(job: &Job, worker: &str, now: Timestamp) -> Lease {
    let mut next = job.clone();
    let mut exhausted = false;
    if job.state == JobState::Leased {
        next.errors.push(format!(
            "attempt {}: the lease expired before a result was reported",
            job.attempt
        ));
        exhausted = job.attempt >= job.policy.attempts;
    }
    if !exhausted {
        next.attempt = job.attempt.saturating_add(1);
    }
    next.lease_token = job.lease_token.saturating_add(1);
    next.state = JobState::Leased;
    next.due_at = Timestamp(now.0.saturating_add(job.policy.lease_ms.max(1)));
    next.worker = Some(worker.to_string());
    Lease {
        token: next.lease_token,
        expires_at: next.due_at,
        job: next,
        exhausted,
    }
}

/// Whether reporting `report` for the job as leased ends it as failed: a failure without retry,
/// or one on the last attempt of its policy.
pub fn report_fails_job(job: &Job, report: &JobReport) -> bool {
    match report {
        JobReport::Succeeded => false,
        JobReport::Failed { retry, .. } => !retry || job.attempt >= job.policy.attempts,
    }
}

/// Record the outcome of an attempt, or refuse it: a token other than the current lease's, a job
/// that is not leased (cancelled, ended, released) and a lease that expired before `now` are
/// refused, and nothing changes (SCH-011, SCH-1526).
pub fn apply_report(
    job: &Job,
    token: u64,
    report: &JobReport,
    now: Timestamp,
) -> Result<Job, String> {
    if job.state != JobState::Leased {
        return Err(format!(
            "job {} is {}, not leased: the report is refused",
            job.id,
            job.state.as_str()
        ));
    }
    if token != job.lease_token {
        return Err(format!(
            "the lease token {token} is superseded by {} (SCH-011)",
            job.lease_token
        ));
    }
    if now > job.due_at {
        return Err(format!(
            "the lease expired at {} before the result was reported at {} (SCH-1526)",
            job.due_at.0, now.0
        ));
    }
    let mut next = job.clone();
    next.worker = None;
    match report {
        JobReport::Succeeded => {
            next.state = JobState::Completed;
            next.ended_at = Some(now);
        }
        JobReport::Failed { error, .. } => {
            next.errors
                .push(format!("attempt {}: {}", job.attempt, clip_error(error)));
            if report_fails_job(job, report) {
                next.state = JobState::Failed;
                next.ended_at = Some(now);
                next.reason = Some(if job.attempt >= job.policy.attempts {
                    "attempts-exhausted".to_string()
                } else {
                    clip_error(error)
                });
            } else {
                next.state = JobState::Waiting;
                next.due_at = Timestamp(
                    now.0
                        .saturating_add(retry_delay_ms(&job.policy, job.attempt)),
                );
            }
        }
    }
    Ok(next)
}

/// Cancel a job at `now` with `reason`: no later attempt happens (SCH-006). An ended job is left
/// as it is.
pub fn apply_cancel(job: &Job, reason: &str, now: Timestamp) -> JobChange {
    if job.state.ended() {
        return JobChange::Unchanged(job.clone());
    }
    let mut next = job.clone();
    next.state = JobState::Cancelled;
    next.ended_at = Some(now);
    next.worker = None;
    next.reason = Some(clip_error(reason));
    JobChange::Changed(next)
}

/// Give a failed job one more attempt, due at `now`, keeping the outcomes of the earlier ones
/// (AGT-418, SCH-1524: its work, and so its step key, is unchanged). Any other job is left as it
/// is.
pub fn apply_retry(job: &Job, now: Timestamp) -> JobChange {
    if job.state != JobState::Failed {
        return JobChange::Unchanged(job.clone());
    }
    let mut next = job.clone();
    next.state = JobState::Waiting;
    next.due_at = now;
    next.ended_at = None;
    next.reason = None;
    next.policy.attempts = job.attempt.saturating_add(1);
    JobChange::Changed(next)
}

/// Why an exception record cannot be settled, or `None` when it can (contract
/// settle-automation-exception): only a parked or dead-lettered record that is not settled yet,
/// and only one that names a job when it is released. Releasing a parked event offers the event
/// again, which this worker does not do: nothing parks events yet (D-519), so it is refused.
pub fn settle_problem(record: &ExceptionRecord, release: bool) -> Option<String> {
    if !record.kind.settled_by_owner() {
        return Some(format!(
            "a {} record is settled when it is written",
            record.kind.as_str()
        ));
    }
    if record.settled {
        return Some("the record is settled already".to_string());
    }
    if release && record.kind == ExceptionKind::Parked {
        return Some("releasing a parked event is not built".to_string());
    }
    if release && record.job.is_none() {
        return Some("the record names no job to release".to_string());
    }
    None
}

/// The jobs and exception records a decided transition asks for: the hand-off from the kernel's
/// trigger decision (SCH-004, D-6524, D-7200). This is the one place that reads it.
///
/// Each trigger that starts a run for an event of the transition is an
/// [`Effect::StartAutomation`]: it becomes one job, keyed by its run key when the automation has
/// one (one run per key, WFL-1522) and else by its trigger and event, due now. Each trigger the
/// kernel declined (the hop limit, a condition that could not be evaluated, a suspended or
/// removed automation) is an [`Effect::DeclineAutomation`]: it becomes one declined exception
/// record. [`crate::Commit::of`] writes both in the transaction of the transition.
pub fn trigger_intake(
    tenant: &TenantId,
    transition: &Transition,
    now: Timestamp,
) -> (Vec<NewJob>, Vec<ExceptionRecord>) {
    let mut jobs = Vec::new();
    let mut exceptions = Vec::new();
    for effect in &transition.effects {
        match effect {
            Effect::StartAutomation(start) => jobs.push(start_job(start, now)),
            Effect::DeclineAutomation(decline) => {
                exceptions.push(declined_record(tenant, decline, now))
            }
            Effect::Publish { .. } | Effect::RevokeSessions { .. } => {}
        }
    }
    (jobs, exceptions)
}

/// The job that starts the run a trigger asked for.
fn start_job(start: &AutomationStart, now: Timestamp) -> NewJob {
    let key = match &start.run_key {
        Some(run_key) => format!("start:{}:key:{run_key}", start.automation),
        None => format!(
            "start:{}:{}:{}",
            start.trigger, start.automation, start.event_seq
        ),
    };
    NewJob {
        key,
        work: JobWork::StartRun {
            trigger: Some(start.trigger.clone()),
            automation: start.automation.clone(),
            event_seq: Some(start.event_seq),
            hop: u64::from(start.hop),
            params: start.params.clone(),
            caused_by: Some(start.caused_by.clone()),
            on_behalf_of: start.on_behalf_of.clone(),
            definition_version: Some(start.definition_version),
            run_key: start.run_key.clone(),
        },
        due_at: now,
        policy: RetryPolicy::default(),
    }
}

/// The declined exception record of a trigger the kernel declined; the id is derived from the
/// tenant, the trigger, the event and the reason, so a decision made again writes it once.
fn declined_record(
    tenant: &TenantId,
    decline: &AutomationDecline,
    now: Timestamp,
) -> ExceptionRecord {
    let reason = decline.reason.as_str();
    let id = derived_uuid(&format!(
        "strata/automation-exception/v1\0{tenant}\0declined\0{}\0{}\0{reason}",
        decline.trigger, decline.event_seq
    ));
    let mut detail = decline.detail.clone();
    let cut = (0..=detail.len().min(256))
        .rev()
        .find(|i| detail.is_char_boundary(*i))
        .unwrap_or(0);
    detail.truncate(cut);
    ExceptionRecord {
        id,
        kind: ExceptionKind::Declined,
        reason: reason.to_string(),
        trigger: Some(decline.trigger.clone()),
        automation: Some(decline.automation.clone()),
        run: None,
        step: None,
        event_seq: Some(decline.event_seq),
        job: None,
        hop: Some(u64::from(decline.hop)),
        at: now,
        detail,
        settled: true,
        seq: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant() -> TenantId {
        TenantId::new("alpha").unwrap()
    }

    /// Verifies the hand-off of D-7200: a start becomes one job carrying every field of the
    /// trigger's effect, keyed by the run key; a decline becomes one settled declined record.
    #[test]
    fn the_trigger_effects_become_a_job_and_a_declined_record() {
        use strata_kernel::triggers::AutomationExceptionReason;
        use strata_kernel::{Output, SchemaVersion, StateDelta, StateRevision};
        let principal = |text: &str| PrincipalId::new(text).unwrap();
        let start = AutomationStart {
            trigger: TriggerId::new("on_publish").unwrap(),
            automation: WorkflowId::new("award").unwrap(),
            event_seq: 7,
            hop: 2,
            caused_by: principal("alpha:writer1"),
            on_behalf_of: Some(principal("alpha:owner")),
            definition_version: SchemaVersion(4),
            run_key: Some("abc".into()),
            params: BTreeMap::from([("seq".to_string(), Value::Int(7))]),
        };
        let decline = AutomationDecline {
            trigger: TriggerId::new("on_publish").unwrap(),
            automation: WorkflowId::new("award").unwrap(),
            event_seq: 8,
            hop: 9,
            reason: AutomationExceptionReason::HopLimit,
            detail: "hop 9 is past the hop limit of 8".into(),
        };
        let transition = Transition {
            delta: StateDelta {
                tenant: tenant(),
                base_revision: Some(StateRevision(1)),
                new_revision: StateRevision(2),
                next_record: 1,
                event_seq: 8,
                ops: Vec::new(),
            },
            events: Vec::new(),
            effects: vec![
                Effect::Publish { seq: 7 },
                Effect::StartAutomation(start),
                Effect::DeclineAutomation(decline),
            ],
            evidence: Vec::new(),
            output: Output::Member,
        };
        let (jobs, exceptions) = trigger_intake(&tenant(), &transition, Timestamp(50));
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].key, "start:award:key:abc");
        assert_eq!(jobs[0].due_at, Timestamp(50));
        let JobWork::StartRun {
            hop,
            caused_by,
            on_behalf_of,
            definition_version,
            run_key,
            params,
            ..
        } = &jobs[0].work
        else {
            panic!("{:?}", jobs[0].work);
        };
        assert_eq!(*hop, 2);
        assert_eq!(caused_by.as_ref(), Some(&principal("alpha:writer1")));
        assert_eq!(on_behalf_of.as_ref(), Some(&principal("alpha:owner")));
        assert_eq!(*definition_version, Some(SchemaVersion(4)));
        assert_eq!(run_key.as_deref(), Some("abc"));
        assert_eq!(params.get("seq"), Some(&Value::Int(7)));
        assert_eq!(exceptions.len(), 1);
        assert_eq!(exceptions[0].kind, ExceptionKind::Declined);
        assert_eq!(exceptions[0].reason, "hop-limit");
        assert_eq!(exceptions[0].hop, Some(9));
        assert!(exceptions[0].settled);
    }

    fn job(attempts: u32) -> Job {
        new_job(
            &tenant(),
            &NewJob {
                key: "k".into(),
                work: JobWork::Step {
                    run: "r".into(),
                    automation: WorkflowId::new("award").unwrap(),
                    step: StepName::new("first").unwrap(),
                    sequence: 1,
                },
                due_at: Timestamp(100),
                policy: RetryPolicy {
                    attempts,
                    first_delay_ms: 10,
                    max_delay_ms: 25,
                    lease_ms: 50,
                },
            },
            Timestamp(0),
        )
    }

    // Verifies: SCH-013
    #[test]
    fn the_delay_doubles_up_to_the_maximum() {
        let policy = job(5).policy;
        let delays: Vec<i64> = (1..=4).map(|n| retry_delay_ms(&policy, n)).collect();
        assert_eq!(delays, vec![10, 20, 25, 25]);
    }

    #[test]
    fn a_job_id_is_a_uuid_derived_from_tenant_and_key() {
        let id = job_id(&tenant(), "k");
        assert_eq!(id, job_id(&tenant(), "k"));
        assert_ne!(id, job_id(&TenantId::new("beta").unwrap(), "k"));
        assert_eq!(id.len(), 36);
        assert_eq!(id.as_bytes()[14], b'8');
    }

    // Verifies: SCH-011
    #[test]
    fn a_superseded_or_late_report_is_refused() {
        let first = apply_lease(&job(3), "w1", Timestamp(100));
        assert!(!first.exhausted);
        // The lease expires at 150 and another worker takes the job.
        let second = apply_lease(&first.job, "w2", Timestamp(151));
        assert_eq!(second.token, first.token + 1);
        assert!(apply_report(
            &second.job,
            first.token,
            &JobReport::Succeeded,
            Timestamp(152)
        )
        .is_err());
        assert!(apply_report(
            &second.job,
            second.token,
            &JobReport::Succeeded,
            Timestamp(152)
        )
        .is_ok());
        assert!(apply_report(
            &second.job,
            second.token,
            &JobReport::Succeeded,
            Timestamp(202)
        )
        .is_err());
    }

    // Verifies: SCH-014
    #[test]
    fn the_last_failed_attempt_fails_the_job_with_every_error() {
        let mut current = job(2);
        for at in [100, 200] {
            let lease = apply_lease(&current, "w", Timestamp(at));
            current = apply_report(
                &lease.job,
                lease.token,
                &JobReport::Failed {
                    error: format!("boom {at}"),
                    retry: true,
                },
                Timestamp(at + 1),
            )
            .unwrap();
        }
        assert_eq!(current.state, JobState::Failed);
        assert_eq!(current.reason.as_deref(), Some("attempts-exhausted"));
        assert_eq!(current.errors.len(), 2);
    }
}
