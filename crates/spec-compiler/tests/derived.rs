//! The generated derived contracts (WG-4935) run against a store of named counters.
//!
//! `crates/spec-compiler-derived-fixture` is generated from `tests/fixtures/derived/spec.jsonc`
//! (the golden test is `derived_golden_output_matches_its_fixture_crate` in `tests/golden.rs`).
//! `Memory` implements the component trait `contracts::Store` the derived loops call through:
//! `load_counters` answers the counters its needs name that exist, and `commit_bump` refuses with
//! a conflict as often as the test asks before committing (and whenever the revision moved).
//!
//! - pure: `pick_larger` is the function `larger`.
//! - read: `read_counter` loads what `read-needs` names and decides with `read-value`.
//! - command, need rounds: `copy_counter` loads the source only; `decide-copy` asks for the target
//!   (`Missing`), which is added and loaded, then the copy commits. A need asked for twice (the
//!   counter does not exist) is the contract's `missing`.
//! - command, conflict retries: commits that lose their race start over from the needs, at most
//!   `retries` (3) times; then the contract's `conflict`.
//! - bound exhaustion: `copy_counter_once` has `needRounds` 0 and `retries` 0, so the first
//!   `Missing` and the first conflict are its errors.
//! - facade: `edit-counter` is in no trait (`Memory` implements exactly the three served
//!   contracts) and on no route; its metadata says so.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Mutex;

use strata_spec_compiler_derived_fixture::contracts::{self, meta};
use strata_spec_compiler_derived_fixture::derived;
use strata_spec_compiler_derived_fixture::prelude::Realisation;
use strata_spec_compiler_derived_fixture::types::{Bump, CounterNeed};

#[derive(Default)]
struct Memory {
    counters: Mutex<BTreeMap<String, u64>>,
    revision: Mutex<u64>,
    /// How many commits to refuse with a conflict before one may go through.
    conflicts: Mutex<u32>,
    /// The names each load asked for, in order.
    loads: Mutex<Vec<Vec<String>>>,
    /// How many commits were attempted.
    commits: Mutex<u32>,
}

impl Memory {
    fn with(counters: &[(&str, u64)]) -> Self {
        let memory = Memory::default();
        *memory.counters.lock().unwrap() = counters
            .iter()
            .map(|(name, value)| (name.to_string(), *value))
            .collect();
        memory
    }

    fn conflicting(self, times: u32) -> Self {
        *self.conflicts.lock().unwrap() = times;
        self
    }

    fn loads(&self) -> Vec<Vec<String>> {
        self.loads.lock().unwrap().clone()
    }

    fn commits(&self) -> u32 {
        *self.commits.lock().unwrap()
    }

    fn counter(&self, name: &str) -> Option<u64> {
        self.counters.lock().unwrap().get(name).copied()
    }
}

impl contracts::Store for Memory {
    fn load_counters(
        &self,
        request: contracts::LoadCountersRequest,
    ) -> impl Future<Output = Result<contracts::LoadCountersResponse, contracts::LoadCountersError>> + Send
    {
        let names: Vec<String> = request.needs.iter().map(|n| n.name.clone()).collect();
        self.loads.lock().unwrap().push(names.clone());
        let counters = {
            let all = self.counters.lock().unwrap();
            names
                .iter()
                .filter_map(|n| all.get(n).map(|v| (n.clone(), *v)))
                .collect()
        };
        let revision = *self.revision.lock().unwrap();
        std::future::ready(Ok(contracts::LoadCountersResponse { counters, revision }))
    }

    fn commit_bump(
        &self,
        request: contracts::CommitBumpRequest,
    ) -> impl Future<Output = Result<contracts::CommitBumpResponse, contracts::CommitBumpError>> + Send
    {
        *self.commits.lock().unwrap() += 1;
        let out = {
            let mut conflicts = self.conflicts.lock().unwrap();
            let mut revision = self.revision.lock().unwrap();
            if *conflicts > 0 {
                *conflicts -= 1;
                // Another commit got there first.
                *revision += 1;
                Err(contracts::CommitBumpError::Conflict)
            } else if request.revision != *revision {
                Err(contracts::CommitBumpError::Conflict)
            } else {
                let Bump { name, value } = request.bump;
                self.counters.lock().unwrap().insert(name, value);
                *revision += 1;
                Ok(contracts::CommitBumpResponse {
                    revision: *revision,
                })
            }
        };
        std::future::ready(out)
    }

    fn mutate_counter(
        &self,
        _request: contracts::MutateCounterRequest,
    ) -> impl Future<Output = Result<contracts::MutateCounterResponse, contracts::MutateCounterError>>
           + Send {
        std::future::ready(Err(contracts::MutateCounterError::Unavailable))
    }
}

fn names(loads: &[&[&str]]) -> Vec<Vec<String>> {
    loads
        .iter()
        .map(|l| l.iter().map(|n| n.to_string()).collect())
        .collect()
}

fn need(name: &str) -> CounterNeed {
    CounterNeed {
        name: name.to_string(),
    }
}

fn copy(source: &str, target: &str) -> contracts::CopyCounterRequest {
    contracts::CopyCounterRequest {
        source: source.to_string(),
        target: target.to_string(),
    }
}

#[test]
fn a_pure_contract_is_its_function() {
    for (a, b, larger) in [(3, 5, 5), (7, 2, 7), (4, 4, 4)] {
        assert_eq!(
            derived::pick_larger(contracts::PickLargerRequest { a, b }),
            Ok(contracts::PickLargerResponse { larger })
        );
    }
}

#[tokio::test]
async fn a_read_loads_its_needs_then_decides() {
    let store = Memory::with(&[("a", 3), ("b", 1)]);
    let read = |name: &str| contracts::ReadCounterRequest {
        name: name.to_string(),
    };
    assert_eq!(
        derived::read_counter(&store, read("a")).await,
        Ok(contracts::ReadCounterResponse { value: 3 })
    );
    assert_eq!(
        derived::read_counter(&store, read("z")).await,
        Err(contracts::ReadCounterError::Missing(need("z")))
    );
    assert_eq!(store.loads(), names(&[&["a"], &["z"]]));
    assert_eq!(store.commits(), 0);
}

#[tokio::test]
async fn a_command_that_needs_more_state_asks_for_it_and_gets_it() {
    let store = Memory::with(&[("a", 3), ("b", 1)]);
    assert_eq!(
        derived::copy_counter(&store, copy("a", "b")).await,
        Ok(contracts::CopyCounterResponse {
            revision: 1,
            value: 3
        })
    );
    // The source alone first; decide asked for the target, which the second load added.
    assert_eq!(store.loads(), names(&[&["a"], &["a", "b"]]));
    assert_eq!(store.commits(), 1);
    assert_eq!(store.counter("b"), Some(3));
}

#[tokio::test]
async fn a_need_asked_for_twice_is_the_contracts_error() {
    let store = Memory::with(&[("a", 3)]);
    assert_eq!(
        derived::copy_counter(&store, copy("a", "z")).await,
        Err(contracts::CopyCounterError::Missing(need("z")))
    );
    // z was added once; when it is still missing after it was loaded, the decision is final.
    assert_eq!(store.loads(), names(&[&["a"], &["a", "z"]]));
    assert_eq!(store.commits(), 0);
}

#[tokio::test]
async fn a_command_that_loses_a_race_is_decided_again() {
    let store = Memory::with(&[("a", 3), ("b", 1)]).conflicting(2);
    assert_eq!(
        derived::copy_counter(&store, copy("a", "b")).await,
        Ok(contracts::CopyCounterResponse {
            revision: 3,
            value: 3
        })
    );
    // Each retry starts over from the needs: three decisions of two loads each.
    assert_eq!(
        store.loads(),
        names(&[
            &["a"],
            &["a", "b"],
            &["a"],
            &["a", "b"],
            &["a"],
            &["a", "b"]
        ])
    );
    assert_eq!(store.commits(), 3);
    assert_eq!(store.counter("b"), Some(3));
}

#[tokio::test]
async fn conflict_retries_run_out() {
    let store = Memory::with(&[("a", 3), ("b", 1)]).conflicting(10);
    assert_eq!(
        derived::copy_counter(&store, copy("a", "b")).await,
        Err(contracts::CopyCounterError::Conflict)
    );
    // The first commit and three retries.
    assert_eq!(store.commits(), 4);
    assert_eq!(store.counter("b"), Some(1));
}

#[tokio::test]
async fn need_rounds_run_out() {
    let store = Memory::with(&[("a", 3), ("b", 1)]);
    // needRounds 0: the target decide asks for is never loaded.
    assert_eq!(
        derived::copy_counter_once(&store, copy("a", "b")).await,
        Err(contracts::CopyCounterOnceError::Missing(need("b")))
    );
    assert_eq!(store.loads(), names(&[&["a"]]));
    assert_eq!(store.commits(), 0);
    // retries 0: the first conflict is the contract's error. Both counters loaded first.
    let store = Memory::with(&[("a", 3), ("b", 1)]).conflicting(1);
    assert_eq!(
        derived::copy_counter_once(&store, copy("b", "b")).await,
        Err(contracts::CopyCounterOnceError::Conflict)
    );
    assert_eq!(store.commits(), 1);
}

#[test]
fn a_facade_has_no_implementation() {
    // Memory implements exactly the three served contracts: a facade in the Store trait would
    // leave it unimplemented, and this file would not compile.
    assert_eq!(meta::EDIT_COUNTER.realisation, Realisation::Facade);
    assert_eq!(meta::EDIT_COUNTER.implemented_by, None);
    assert_eq!(meta::COPY_COUNTER.realisation, Realisation::Derived);
    assert_eq!(meta::LOAD_COUNTERS.implemented_by, Some("store"));
}
