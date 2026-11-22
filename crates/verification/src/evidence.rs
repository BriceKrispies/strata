//! How much a claim is worth: the five classes every Strata finding is labelled with.
//!
//! There are no confidence scores. A finding says which kind of evidence stands behind it and
//! what was assumed, and that is all it says.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// Machine-checked in Lean for the abstract model, under the stated assumptions. Says
    /// nothing by itself about the Rust implementation.
    Proven,
    /// Observed by executing the real kernel on stated inputs. Exactly repeatable. True of
    /// those inputs and of nothing else.
    Reproduced,
    /// Derived by a sound over-approximation: everything that can happen is included, and
    /// some things that cannot may be.
    SoundApproximation,
    /// Counted over a generated sample. A description of the sample, not a prediction.
    Empirical,
    /// Not modelled. Stated so that silence is not mistaken for safety.
    Unknown,
}

impl Class {
    pub fn label(self) -> &'static str {
        match self {
            Class::Proven => "Proven for the abstract model",
            Class::Reproduced => "Reproduced by execution",
            Class::SoundApproximation => "Sound approximation",
            Class::Empirical => "Estimated from a sample",
            Class::Unknown => "Unknown or not modelled",
        }
    }

    pub fn meaning(self) -> &'static str {
        match self {
            Class::Proven => {
                "Machine-checked in Lean 4 for the abstract model in lean/Strata. It holds for the Rust kernel only as far as the correspondence tests and review support."
            }
            Class::Reproduced => {
                "The production kernel was run on exactly these inputs and this is what it did. Running it again gives the same trace hash."
            }
            Class::SoundApproximation => {
                "Computed without running the future: it lists everything that could be affected, and may list things that never will be."
            }
            Class::Empirical => {
                "Counted over operations generated from a seed. Real traffic will differ; no confidence is attached."
            }
            Class::Unknown => "Outside what the simulator models. No claim is made either way.",
        }
    }
}

/// What the logical simulator does not model, said once and attached to every projection.
pub fn unmodelled() -> Vec<&'static str> {
    vec![
        "Wall-clock time: CPU use, database latency, lock contention and network delay are not simulated.",
        "Concurrency: operations run one at a time in schedule order. Interleavings the schedule does not contain are not explored.",
        "The shell around the kernel: sessions, cookies, CSRF checks, the WebSocket relay and the browser are not part of a simulation.",
        "External effects: with faults on, publication runs through the backend's own publisher over an in-memory store; what a subscriber does with an event is not modelled.",
        "Future operations: only the operations in the schedule are executed. Operations nobody generated or recorded are not predicted.",
        "Parameterised queries are compared only for the argument values listed in the report.",
    ]
}
