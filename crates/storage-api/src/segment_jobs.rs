//! The jobs of segments (WG-5165, WG-5152; SEG-1543 to SEG-1546, D-6535, D-7280): what a segment
//! write asks the job worker to do next, and the keys that make asking twice harmless.
//!
//! - A **batch job** ([`SegmentWork::Batch`]) runs the next continuation of a segment: the one row
//!   of `rec-segment-batch` of its segment and scope, in a transaction of its own (CT-297). A
//!   write that leaves a continuation creates the job in its own transaction ([`of_write`]): the
//!   job is in the commit that leaves the continuation, so a continuation is never left with
//!   nothing to run it. The job names the row it was made for (segment, scope, version and the
//!   position reached); the row decides what runs, so a job whose row has moved on or is gone
//!   runs nothing twice.
//! - A **refresh job** ([`SegmentWork::Refresh`]) is one tick of the recurring schedule of a
//!   time-dependent segment. Its due times are multiples of the refresh interval, so the chain
//!   that a definition starts and the one that a restore starts are the same chain: a job is
//!   created once for its key, and the key names the segment and the due time.
//! - An **events job** ([`SegmentWork::Events`]) reads the segment event log from the cursor of
//!   the tenant (D-7280): see [`crate::segment_pump`].
//! - A **fan-out job** ([`SegmentWork::FanOut`]) is one batch of members of a fan-out step.
//!
//! Every key is a function of what the job is for and never of when it was asked for, except the
//! salt of a batch job, which is the correlation of the decision that left the continuation: a
//! continuation that is left again by another decision is another job.

use crate::jobs::{JobWork, NewJob, RetryPolicy};
use serde::{Deserialize, Serialize};
use strata_kernel::segments::{Batch, BatchChange, Segment, SegmentId, SegmentWrite};
use strata_kernel::{canon, PrincipalId, RecordId, StepName, Timestamp, WorkflowId};

/// One quota day, in milliseconds: the daily job quota of a plan counts the jobs created from
/// 00:00 UTC to 24:00 UTC (D-6533).
pub const DAY_MS: i64 = 86_400_000;

/// What a segment job does. The store keeps it as it was given and never reads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SegmentWork {
    /// Run the continuation of `segment` in `scope` (the key of
    /// [`strata_kernel::segments::BatchScope`]) of `version`, which has reached `after`.
    Batch {
        segment: SegmentId,
        scope: String,
        version: u64,
        #[serde(default)]
        after: Option<RecordId>,
    },
    /// Refresh `segment` at its due time and ask for the next refresh one interval later.
    Refresh { segment: SegmentId, due: Timestamp },
    /// Hand the new entries of the segment event log to the triggers (D-7280).
    Events,
    /// One batch of the members of the segment of a fan-out step: the run and step it belongs to,
    /// the last member of the batch before (the cursor), the batches and the members done so far.
    FanOut {
        run: String,
        automation: WorkflowId,
        step: StepName,
        sequence: u64,
        #[serde(default)]
        after: Option<PrincipalId>,
        batch: u32,
        members: u64,
    },
}

/// The first 16 hex digits of a SHA-256 of `text`: a salt short enough for a job key.
fn short(text: &str) -> String {
    canon::hash_bytes(text.as_bytes())
        .chars()
        .take(16)
        .collect()
}

/// The key of the batch job of a continuation as one decision left it (`salt` is the
/// correlation of the decision).
pub fn batch_key(batch: &Batch, salt: &str) -> String {
    format!(
        "segb:{}:{}:{}:{}:{}",
        batch.segment,
        batch.scope.key(),
        batch.version,
        batch.after.map_or(0, |after| after.0),
        short(salt)
    )
}

/// The job that runs a continuation, due at `due`.
pub fn batch_job(batch: &Batch, salt: &str, due: Timestamp) -> NewJob {
    NewJob {
        key: batch_key(batch, salt),
        work: JobWork::Segment(SegmentWork::Batch {
            segment: batch.segment,
            scope: batch.scope.key(),
            version: batch.version,
            after: batch.after,
        }),
        due_at: due,
        policy: RetryPolicy::default(),
    }
}

/// The refresh job of `segment` due at `due`: one job for each segment and due time.
pub fn refresh_job(segment: SegmentId, due: Timestamp) -> NewJob {
    NewJob {
        key: format!("segr:{segment}:{}", due.0),
        work: JobWork::Segment(SegmentWork::Refresh { segment, due }),
        due_at: due,
        policy: RetryPolicy::default(),
    }
}

/// The job that hands the event log on, named by `tag`: one job for each tag.
pub fn events_job(tag: &str, due: Timestamp) -> NewJob {
    NewJob {
        key: format!("sege:{}", short(tag)),
        work: JobWork::Segment(SegmentWork::Events),
        due_at: due,
        policy: RetryPolicy::default(),
    }
}

/// The refresh interval of a segment in milliseconds, when it refreshes on a schedule: a
/// time-dependent segment that declares one (SEG-017).
pub fn interval_ms(segment: &Segment) -> Option<i64> {
    match (segment.time_dependent, segment.refresh_seconds) {
        (true, Some(seconds)) if seconds > 0 => Some(i64::from(seconds) * 1_000),
        _ => None,
    }
}

/// The first due time of the schedule of `segment` after `now`: the next multiple of its refresh
/// interval. `None` for a segment that does not refresh.
pub fn next_refresh(segment: &Segment, now: Timestamp) -> Option<Timestamp> {
    interval_ms(segment).map(|interval| Timestamp((now.0.div_euclid(interval) + 1) * interval))
}

/// The jobs a segment write asks for, as it is committed at `now` under `salt` (the correlation
/// of the decision, or a tag that names the write):
///
/// - a batch job for each continuation it leaves ([`BatchChange::Put`]);
/// - the first refresh of each time-dependent segment it defines, due at the next multiple of the
///   refresh interval; asking again for a schedule that runs creates nothing;
/// - an events job when it writes entry or exit events.
///
/// The jobs are created in the transaction of the write ([`crate::Commit::jobs`],
/// [`crate::segments::SegmentCommit::work`]), due now.
pub fn of_write(write: &SegmentWrite, salt: &str, now: Timestamp) -> Vec<NewJob> {
    let mut jobs = Vec::new();
    for change in &write.batches {
        if let BatchChange::Put(batch) = change {
            jobs.push(batch_job(batch, salt, now));
        }
    }
    for segment in &write.put_segments {
        if let Some(due) = next_refresh(segment, now) {
            jobs.push(refresh_job(segment.id, due));
        }
    }
    if !write.events.is_empty() {
        jobs.push(events_job(salt, now));
    }
    jobs
}

/// The start of the quota day that holds `now`.
pub fn day_start(now: Timestamp) -> Timestamp {
    Timestamp(now.0.div_euclid(DAY_MS) * DAY_MS)
}

/// The start of the quota day after the one that holds `now`: when a job past the quota is due
/// (D-6533).
pub fn next_day(now: Timestamp) -> Timestamp {
    Timestamp(day_start(now).0 + DAY_MS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_kernel::segments::{BatchScope, SegmentReason};

    fn segment_id() -> SegmentId {
        serde_json::from_value(serde_json::json!("6a0f3c1e-2280-4000-8000-000000000001")).unwrap()
    }

    #[test]
    fn a_job_key_names_what_the_job_is_for() {
        let batch = Batch {
            segment: segment_id(),
            scope: BatchScope::All,
            version: 2,
            after: Some(RecordId(100)),
            reason: SegmentReason::PredicateChange,
            hop: 0,
            at: Timestamp(5),
        };
        assert_eq!(batch_key(&batch, "req-1"), batch_key(&batch, "req-1"));
        assert_ne!(batch_key(&batch, "req-1"), batch_key(&batch, "req-2"));
        let later = Batch {
            after: Some(RecordId(200)),
            ..batch.clone()
        };
        assert_ne!(batch_key(&batch, "req-1"), batch_key(&later, "req-1"));
        assert!(batch_key(&batch, "req-1").len() <= crate::jobs::MAX_JOB_KEY_LEN);
    }

    #[test]
    fn the_quota_day_starts_at_midnight_utc() {
        let noon = Timestamp(3 * DAY_MS + DAY_MS / 2);
        assert_eq!(day_start(noon), Timestamp(3 * DAY_MS));
        assert_eq!(next_day(noon), Timestamp(4 * DAY_MS));
        assert_eq!(next_day(Timestamp(3 * DAY_MS)), Timestamp(4 * DAY_MS));
    }
}
