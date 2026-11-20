//! Recurring schedules of the automation worker (rec-schedule, schedule-flow-impl, SCH-003,
//! SCH-005, SCH-1573, SCH-1574), and the retention of the records the worker keeps.
//!
//! A schedule is a row of its tenant's file: its rule, whether it is paused, the due time of its
//! next job and three counters that are the variables of the machine `schedule-flow-impl`:
//! `created` (jobs created), `completions` (jobs completed or replaced) and `created_at_pause`.
//! The store keeps the rows and applies the rules below in the transaction that writes them, so
//! that both adapters decide alike:
//!
//! - **Completing a job creates the next one in the same transaction** ([`apply_completion`]):
//!   a crash between the two cannot lose the recurrence (SCH-003), and a repeat of the same
//!   completion creates the same job once, because the job's key names the schedule and the
//!   occurrence ([`job_key`]).
//! - **A paused schedule creates no job** (SCH-005): a completion while paused counts and creates
//!   nothing; resuming creates the next job only when no job is open, that is when
//!   `created == completions`.
//! - What the occurrences are, and the time zone they are read in, is the worker's: the rule is
//!   expanded in `strata-backend` (`worker::recurrence`) and the store is given the due time.

use crate::jobs::{JobWork, NewJob, RetryPolicy};
use serde::{Deserialize, Serialize};
use strata_kernel::Timestamp;

/// How long an ended job is kept after its outcome: thirty days (rec-job, OPS-348 family).
pub const JOB_RETENTION_MS: i64 = 30 * DAY_MS;
/// How long a run is kept after it ends, and its steps with it: ninety days
/// (rec-workflow-run, rec-workflow-step-run).
pub const RUN_RETENTION_MS: i64 = 90 * DAY_MS;
/// How long an exception record is kept: ninety days from the time it was written. A parked or
/// dead-lettered one is kept until the tenant owner settles it, and only then counts (OPS-348).
pub const EXCEPTION_RETENTION_MS: i64 = 90 * DAY_MS;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// How long the records of the automation worker are kept past the end of their work, as
/// milliseconds ([`crate::Store::purge_automation`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutomationRetention {
    /// Ended jobs, from their outcome.
    pub jobs_ms: i64,
    /// Ended runs and their steps, from the end of the run.
    pub runs_ms: i64,
    /// Declined and held exception records, and settled parked and dead-lettered ones, from the
    /// time they were written.
    pub exceptions_ms: i64,
}

impl Default for AutomationRetention {
    fn default() -> Self {
        AutomationRetention {
            jobs_ms: JOB_RETENTION_MS,
            runs_ms: RUN_RETENTION_MS,
            exceptions_ms: EXCEPTION_RETENTION_MS,
        }
    }
}

/// What a purge of the worker's records removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AutomationPurged {
    pub jobs: u64,
    pub runs: u64,
    pub steps: u64,
    pub exceptions: u64,
}

impl AutomationPurged {
    pub fn total(&self) -> u64 {
        self.jobs + self.runs + self.steps + self.exceptions
    }
}

/// One schedule of a tenant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    /// A UUID in its text form (schedule-id).
    pub id: String,
    /// The recurrence, as the owner wrote it (SCH-1574).
    pub rule: String,
    /// The time the rule counts from: its DTSTART. Kept while the rule is unchanged.
    pub anchor: Timestamp,
    pub paused: bool,
    /// The due time of the job that is open, or that was last open while paused.
    pub next_due: Timestamp,
    /// Jobs created for the schedule.
    pub created: u64,
    /// Jobs of the schedule that are done: completed, or replaced by a change of the rule.
    pub completions: u64,
    /// The value of `created` when the schedule was paused.
    pub created_at_pause: u64,
    /// When it was last defined.
    pub defined_at: Timestamp,
}

impl Schedule {
    /// A job of the schedule is open: created and not yet completed.
    pub fn has_open_job(&self) -> bool {
        self.created > self.completions
    }
}

/// What `define-schedule` asks the store to hold: the rule, whether it is paused, and the due
/// time of its next occurrence, which the worker has computed from the rule (none when the rule
/// has no occurrence left).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleSet {
    pub id: String,
    pub rule: String,
    pub paused: bool,
    /// DTSTART of the rule when it is new or changed.
    pub anchor: Timestamp,
    /// The first occurrence after the definition, or none.
    pub due: Option<Timestamp>,
}

/// The completion of a schedule's job, as the worker reports it in [`crate::jobs::WorkerWrites`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleAdvance {
    pub schedule: String,
    /// The due time of the next occurrence, or none when the rule has no occurrence left.
    pub next_due: Option<Timestamp>,
}

/// The key of the `n`th job of `schedule`, counted from one: one job for each count, so a
/// job that is created again by a repeat is the same job, and a job that replaces a cancelled
/// one of the same time is a new one.
pub fn job_key(schedule: &str, n: u64) -> String {
    format!("schedule:{schedule}:{n}")
}

/// The `n`th job of `schedule`, for the occurrence due at `due`.
pub fn new_schedule_job(schedule: &str, n: u64, due: Timestamp) -> NewJob {
    NewJob {
        key: job_key(schedule, n),
        work: JobWork::ScheduleDue {
            schedule: schedule.to_string(),
            due,
        },
        due_at: due,
        policy: RetryPolicy::default(),
    }
}

/// What a definition does to the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetOutcome {
    /// The schedule as it is now.
    pub schedule: Schedule,
    /// The job to create, when a live schedule has none open.
    pub create: Option<NewJob>,
    /// The open job of the old rule is cancelled: it is for an occurrence the new rule does not
    /// have.
    pub replace_open: bool,
}

/// Define a schedule at `now`: create it, change its rule, pause it or resume it.
///
/// - A new schedule that is not paused has its first job; a new paused one has none (SCH-005).
/// - A changed rule replaces the open job: the old one is cancelled, counted as done, and a new
///   one is created at the due time of the new rule, unless the schedule is paused.
/// - Pausing keeps an open job and records `created_at_pause`; the job completes without a
///   successor.
/// - Resuming creates a job only when none is open (`created == completions`).
pub fn apply_set(existing: Option<&Schedule>, set: &ScheduleSet, now: Timestamp) -> SetOutcome {
    let mut next = match existing {
        Some(held) => held.clone(),
        None => Schedule {
            id: set.id.clone(),
            rule: set.rule.clone(),
            anchor: set.anchor,
            paused: set.paused,
            next_due: set.due.unwrap_or(set.anchor),
            created: 0,
            completions: 0,
            created_at_pause: 0,
            defined_at: now,
        },
    };
    let mut replace_open = false;
    if let Some(held) = existing {
        if held.rule != set.rule {
            next.rule = set.rule.clone();
            next.anchor = set.anchor;
            if held.has_open_job() {
                replace_open = true;
                next.completions = next.created;
            }
        }
        if set.paused && !held.paused {
            next.created_at_pause = next.created;
        }
        next.paused = set.paused;
        next.defined_at = now;
    }
    let mut create = None;
    if !next.paused && !next.has_open_job() {
        if let Some(due) = set.due {
            next.created += 1;
            next.next_due = due;
            create = Some(new_schedule_job(&next.id, next.created, due));
        }
    }
    if next.paused || create.is_none() {
        if let Some(due) = set.due {
            if !next.has_open_job() {
                next.next_due = due;
            }
        }
    }
    SetOutcome {
        schedule: next,
        create,
        replace_open,
    }
}

/// Count the completion of a job of `schedule` and decide the next one: none when the schedule is
/// paused (SCH-005) or its rule has no occurrence left, else the job of the next occurrence
/// (SCH-003). The caller writes the schedule and creates the job in the transaction that records
/// the completion.
pub fn apply_completion(
    schedule: &Schedule,
    advance: &ScheduleAdvance,
) -> (Schedule, Option<NewJob>) {
    let mut next = schedule.clone();
    next.completions += 1;
    let mut create = None;
    if !next.paused {
        if let Some(due) = advance.next_due {
            next.created += 1;
            next.next_due = due;
            create = Some(new_schedule_job(&next.id, next.created, due));
        }
    }
    (next, create)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(rule: &str, paused: bool, due: Option<i64>) -> ScheduleSet {
        ScheduleSet {
            id: "s-1".into(),
            rule: rule.into(),
            paused,
            anchor: Timestamp(0),
            due: due.map(Timestamp),
        }
    }

    // Verifies: SCH-005
    #[test]
    fn a_new_paused_schedule_creates_no_job_and_resuming_creates_one() {
        let made = apply_set(None, &set("FREQ=DAILY", true, Some(100)), Timestamp(1));
        assert!(made.create.is_none());
        assert_eq!((made.schedule.created, made.schedule.completions), (0, 0));
        let resumed = apply_set(
            Some(&made.schedule),
            &set("FREQ=DAILY", false, Some(200)),
            Timestamp(2),
        );
        assert_eq!(
            resumed.create.as_ref().map(|j| j.due_at),
            Some(Timestamp(200))
        );
        assert_eq!(resumed.schedule.created, 1);
    }

    // Verifies: SCH-003, SCH-005
    #[test]
    fn a_completion_creates_the_next_job_unless_the_schedule_is_paused() {
        let live = apply_set(None, &set("FREQ=DAILY", false, Some(100)), Timestamp(1)).schedule;
        let advance = ScheduleAdvance {
            schedule: "s-1".into(),
            next_due: Some(Timestamp(200)),
        };
        let (after, job) = apply_completion(&live, &advance);
        assert_eq!((after.created, after.completions), (2, 1));
        assert_eq!(job.map(|j| j.key), Some(job_key("s-1", 2)));
        let paused = apply_set(
            Some(&after),
            &set("FREQ=DAILY", true, Some(300)),
            Timestamp(2),
        );
        assert!(paused.create.is_none());
        let (done, none) = apply_completion(&paused.schedule, &advance);
        assert!(none.is_none());
        assert_eq!((done.created, done.completions), (2, 2));
        // Resumed with no job open: one is created.
        let resumed = apply_set(
            Some(&done),
            &set("FREQ=DAILY", false, Some(400)),
            Timestamp(3),
        );
        assert_eq!(resumed.create.map(|j| j.due_at), Some(Timestamp(400)));
    }

    #[test]
    fn resuming_with_a_job_open_creates_none_and_a_changed_rule_replaces_it() {
        let live = apply_set(None, &set("FREQ=DAILY", false, Some(100)), Timestamp(1)).schedule;
        let paused = apply_set(
            Some(&live),
            &set("FREQ=DAILY", true, Some(100)),
            Timestamp(2),
        );
        assert_eq!(paused.schedule.created_at_pause, 1);
        let resumed = apply_set(
            Some(&paused.schedule),
            &set("FREQ=DAILY", false, Some(100)),
            Timestamp(3),
        );
        assert!(resumed.create.is_none() && !resumed.replace_open);
        let changed = apply_set(
            Some(&resumed.schedule),
            &set("FREQ=HOURLY", false, Some(150)),
            Timestamp(4),
        );
        assert!(changed.replace_open);
        assert_eq!(changed.create.map(|j| j.due_at), Some(Timestamp(150)));
        assert_eq!(changed.schedule.created, 2);
        assert_eq!(changed.schedule.completions, 1);
    }
}
