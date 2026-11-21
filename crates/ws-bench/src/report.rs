//! The report of one run. Everything under `determinism`, `ops`, `coherence` and `faults` is a
//! function of the seed (and the mode, where a layer's answers could differ); everything under
//! `timing` and the wall-clock fields of `store` and `wrapper` is a measurement and is not.

use crate::layer::WrapperStats;
use crate::modes::StoreCounts;
use crate::scenario::Refutation;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Serialize)]
pub struct Latency {
    pub count: u64,
    pub p50_ns: u64,
    pub p95_ns: u64,
    pub p99_ns: u64,
    pub max_ns: u64,
    pub mean_ns: u64,
}

impl Latency {
    pub fn of(samples: &mut [u64]) -> Latency {
        if samples.is_empty() {
            return Latency::default();
        }
        samples.sort_unstable();
        let at = |q: f64| {
            let rank = ((samples.len() as f64) * q).ceil() as usize;
            samples[rank.clamp(1, samples.len()) - 1]
        };
        Latency {
            count: samples.len() as u64,
            p50_ns: at(0.50),
            p95_ns: at(0.95),
            p99_ns: at(0.99),
            max_ns: *samples.last().unwrap_or(&0),
            mean_ns: samples.iter().sum::<u64>() / samples.len() as u64,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct KindCounts {
    pub count: u64,
    pub ok: u64,
    pub domain_errors: u64,
    pub store_errors: u64,
    /// The operation could not be built (the tenant offers none of its kind, or the type has no
    /// records left).
    pub skipped: u64,
    /// The kernel's refusals of this kind of operation, by the kind of error (a policy refusal
    /// is `Unauthorized`). Counted in `domain_errors` as well.
    pub refused: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Divergence {
    pub op: u64,
    pub kind: String,
    pub tenant: String,
    pub what: String,
    pub under_test: String,
    pub reference: String,
    pub generation: Option<u64>,
    pub after_fault: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Coherence {
    /// Answers compared with the reference's at one generation.
    pub checked: u64,
    /// Answers not compared because the generation moved between the two reads.
    pub unmatched: u64,
    pub divergences: u64,
    /// The probes read straight after a fault, and how many of them diverged.
    pub post_fault_checked: u64,
    pub post_fault_divergences: u64,
    /// Values of an erased subject looked for after the erasure, and how many were still there.
    pub erasure_probes_read: u64,
    pub plaintext_leaks: u64,
    /// The first few, in detail.
    pub first: Vec<Divergence>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct FaultRecord {
    pub kind: String,
    pub at_op: u64,
    pub outcome: String,
    /// The fault took effect, by its own check (a no-op fault is not).
    pub effective: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Timing {
    pub wall_ms: f64,
    pub timed_ms: f64,
    pub throughput_ops_per_s: f64,
    pub overall: Latency,
    pub by_kind: BTreeMap<String, Latency>,
    /// Setup (population) time, not part of the run.
    pub populate_ms: f64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct StoreSide {
    #[serde(flatten)]
    pub counts: StoreCounts,
    /// Schema decodes during the timed operations; `null` where the adapter keeps no counter.
    pub schema_decodes: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Wrapper {
    #[serde(flatten)]
    pub stats: WrapperStats,
    /// Hits over hits and misses; `null` for a placeholder.
    pub hit_rate_by_count: Option<f64>,
    /// Time the hits saved over that and the time the misses cost; `null` for a placeholder.
    pub hit_rate_by_cost: Option<f64>,
}

impl Wrapper {
    pub fn of(stats: WrapperStats) -> Wrapper {
        let (by_count, by_cost) = if stats.placeholder || stats.hits + stats.misses == 0 {
            (None, None)
        } else {
            let total_cost = stats.hit_saved_ns + stats.miss_cost_ns;
            (
                Some(stats.hits as f64 / (stats.hits + stats.misses) as f64),
                (total_cost > 0).then(|| stats.hit_saved_ns as f64 / total_cost as f64),
            )
        };
        Wrapper {
            stats,
            hit_rate_by_count: by_count,
            hit_rate_by_cost: by_cost,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Conflicts {
    /// Commits refused for a stale base and decided again, summed over the run.
    pub retries: u64,
    /// Contended updates that lost their race at the revision the client had seen.
    pub contended_conflicts: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Determinism {
    /// A hash of the operation sequence: client, kind, tenant, object and id of every operation.
    pub sequence_digest: String,
    /// A hash of every answer.
    pub answer_digest: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    pub scenario: String,
    pub store: String,
    pub mode: String,
    pub seed: u64,
    pub scale: u32,
    /// What the primary tenant is (the corpus scale, or the stand-in's size).
    pub scale_name: String,
    pub length: usize,
    pub clients: u32,
    pub tenants: usize,
    pub modules: Vec<String>,
    pub measures: Vec<String>,
    pub refutation: Option<Refutation>,
    pub determinism: Determinism,
    pub ops: BTreeMap<String, KindCounts>,
    pub coherence: Coherence,
    pub conflicts: Conflicts,
    pub faults: Vec<FaultRecord>,
    /// Faults that did not take effect: any is a failure of the run.
    pub faults_ineffective: u64,
    /// The kernel's refusals of operations, by the kind of error, over the whole run.
    pub refusals: BTreeMap<String, u64>,
    pub store_side: StoreSide,
    pub wrapper: Wrapper,
    pub timing: Timing,
    /// Object types the scenario named that the catalogue lacks, and what each ran as.
    pub object_aliases: BTreeMap<String, String>,
    /// Operations the primary tenant could not map onto its schema when it was opened.
    pub unmapped: Vec<String>,
}
