//! Where the jobs a commit creates are due, once a plan's allowance of jobs is taken into account
//! (D-6533, D-508, SCH-302).
//!
//! The daily job quota of a plan is checked when a commit creates jobs. A job within it is
//! created due as usual; a job past it is created due at the start of the next quota day, counts
//! against that day, and writes a held exception record. The change itself always commits. A job
//! past the per-minute admission rate (D-700) is created due at the start of the next minute that
//! has room, so a burst is released at the rate and none is lost.
//!
//! A quota day is a UTC calendar day. This module holds the decision only: the counters it reads
//! come from whoever implements [`JobAdmission`], and the usage of a commit is recorded by that
//! implementation once the commit is written.

use crate::jobs::{derived_uuid, job_id, ExceptionKind, ExceptionRecord, NewJob};
use std::collections::BTreeMap;
use strata_kernel::{TenantId, Timestamp};

/// Milliseconds in a quota day.
pub const DAY_MS: i64 = 86_400_000;
/// Milliseconds in the window of the per-minute admission rate.
pub const MINUTE_MS: i64 = 60_000;
/// The reason of the exception record a held job writes (spec type `automation-exception-reason`).
pub const REASON_JOB_QUOTA: &str = "job-quota";
/// How many days or minutes ahead a job is looked for room. A tenant that fills every one of them
/// has its job placed at the last.
const MAX_PERIODS_AHEAD: i64 = 4_000;

/// The quota day and the admission minute a job was counted against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    /// The day number since the epoch (UTC).
    pub day: i64,
    /// The minute number since the epoch.
    pub minute: i64,
}

/// What a tenant may create, and what it has used.
pub struct JobLimits<'a> {
    /// Jobs per quota day; `None` is no limit.
    pub per_day: Option<u64>,
    /// Jobs released per minute; `None` is no limit.
    pub per_minute: Option<u64>,
    /// Jobs already counted against a quota day, by day number.
    pub used_in_day: &'a dyn Fn(i64) -> u64,
    /// Jobs already counted against a minute, by minute number.
    pub used_in_minute: &'a dyn Fn(i64) -> u64,
}

/// Decide when each of `jobs` is due, in order, and which are held. A job is changed only in its
/// `due_at`. Returns where each job was counted and the held exception record of each job moved
/// to a later quota day.
pub fn place_jobs(
    tenant: &TenantId,
    jobs: &mut [NewJob],
    now: Timestamp,
    limits: &JobLimits<'_>,
) -> (Vec<Placement>, Vec<ExceptionRecord>) {
    let today = now.0.div_euclid(DAY_MS);
    let mut day_added: BTreeMap<i64, u64> = BTreeMap::new();
    let mut minute_added: BTreeMap<i64, u64> = BTreeMap::new();
    let mut placements = Vec::with_capacity(jobs.len());
    let mut held = Vec::new();
    for job in jobs.iter_mut() {
        let mut day = today;
        if let Some(cap) = limits.per_day {
            let cap = cap.max(1);
            while day - today < MAX_PERIODS_AHEAD
                && (limits.used_in_day)(day) + day_added.get(&day).copied().unwrap_or(0) >= cap
            {
                day += 1;
            }
        }
        *day_added.entry(day).or_insert(0) += 1;
        if day > today {
            job.due_at = Timestamp(job.due_at.0.max(day * DAY_MS));
            held.push(held_record(tenant, job, day, now));
        }
        let mut minute = job.due_at.0.max(now.0).div_euclid(MINUTE_MS);
        if let Some(cap) = limits.per_minute {
            let cap = cap.max(1);
            let start = minute;
            while minute - start < MAX_PERIODS_AHEAD
                && (limits.used_in_minute)(minute) + minute_added.get(&minute).copied().unwrap_or(0)
                    >= cap
            {
                minute += 1;
            }
            if minute > start {
                job.due_at = Timestamp(job.due_at.0.max(minute * MINUTE_MS));
            }
        }
        *minute_added.entry(minute).or_insert(0) += 1;
        placements.push(Placement { day, minute });
    }
    (placements, held)
}

/// The held exception record of a job moved to the quota day `day`. Its id is derived from the
/// tenant and the job's key, so a commit decided again writes it once.
fn held_record(tenant: &TenantId, job: &NewJob, day: i64, now: Timestamp) -> ExceptionRecord {
    let mut detail = format!(
        "daily job quota reached; the job is due at the start of day {day} (UTC), {}",
        day * DAY_MS
    );
    detail.truncate(256);
    ExceptionRecord {
        id: derived_uuid(&format!(
            "strata/automation-exception/v1\0{tenant}\0held\0{}\0{REASON_JOB_QUOTA}",
            job.key
        )),
        kind: ExceptionKind::Held,
        reason: REASON_JOB_QUOTA.to_string(),
        trigger: None,
        automation: None,
        run: None,
        step: None,
        event_seq: None,
        job: Some(job_id(tenant, &job.key)),
        hop: None,
        at: now,
        detail,
        settled: true,
        seq: 0,
    }
}

/// What a deployment supplies to hold the jobs of a commit to a tenant's allowance. The
/// implementation reads the tenant's plan and counters; it decides nothing about the change.
pub trait JobAdmission: Send + Sync {
    /// Called with the jobs and exception records a commit is about to write: it may move the due
    /// time of a job and add held exception records. Returns where each job was counted. It
    /// counts nothing yet, since the commit may lose its race and be decided again.
    fn admit(
        &self,
        tenant: &TenantId,
        jobs: &mut Vec<NewJob>,
        exceptions: &mut Vec<ExceptionRecord>,
        now: Timestamp,
    ) -> Vec<Placement>;

    /// Called once the commit is written: the placements to count.
    fn committed(&self, tenant: &TenantId, placements: &[Placement]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{JobWork, RetryPolicy};

    fn tenant() -> TenantId {
        TenantId::new("alpha").unwrap()
    }

    fn job(key: &str, due: i64) -> NewJob {
        NewJob {
            key: key.to_string(),
            work: JobWork::StartRun {
                trigger: None,
                automation: strata_kernel::WorkflowId::new("w").unwrap(),
                event_seq: None,
                hop: 0,
                params: Default::default(),
                caused_by: None,
                on_behalf_of: None,
                definition_version: None,
                run_key: None,
            },
            due_at: Timestamp(due),
            policy: RetryPolicy::default(),
        }
    }

    /// Jobs within the daily quota keep their due time; those past it are due at the start of the
    /// next day and each writes one held record.
    #[test]
    fn jobs_past_the_daily_quota_are_held_to_the_next_day() {
        let now = Timestamp(10 * DAY_MS + 5_000);
        let mut jobs = vec![job("a", now.0), job("b", now.0), job("c", now.0)];
        let none = |_: i64| 0u64;
        let used_today = |day: i64| if day == 10 { 1 } else { 0 };
        let limits = JobLimits {
            per_day: Some(3),
            per_minute: None,
            used_in_day: &used_today,
            used_in_minute: &none,
        };
        let (placed, held) = place_jobs(&tenant(), &mut jobs, now, &limits);
        assert_eq!(jobs[0].due_at, now);
        assert_eq!(jobs[1].due_at, now);
        assert_eq!(jobs[2].due_at, Timestamp(11 * DAY_MS));
        assert_eq!(placed[2].day, 11);
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].kind, ExceptionKind::Held);
        assert_eq!(held[0].reason, REASON_JOB_QUOTA);
    }

    /// A burst past the per-minute rate is released at the rate, a minute at a time.
    #[test]
    fn a_burst_is_released_at_the_per_minute_rate() {
        let now = Timestamp(MINUTE_MS * 7);
        let mut jobs: Vec<NewJob> = (0..5).map(|n| job(&format!("j{n}"), now.0)).collect();
        let none = |_: i64| 0u64;
        let limits = JobLimits {
            per_day: None,
            per_minute: Some(2),
            used_in_day: &none,
            used_in_minute: &none,
        };
        let (placed, held) = place_jobs(&tenant(), &mut jobs, now, &limits);
        let minutes: Vec<i64> = placed.iter().map(|p| p.minute).collect();
        assert_eq!(minutes, vec![7, 7, 8, 8, 9]);
        assert_eq!(jobs[4].due_at, Timestamp(9 * MINUTE_MS));
        assert!(held.is_empty(), "pacing writes no held record");
    }
}
