//! The working-set measurement harness (WG-5407).
//!
//! A *scenario* (`tests/scenarios/*.jsonc`) states a product workload over the enterprise corpus:
//! a mix of reads and intents with a Zipf skew, a population scale, a length, a few logical
//! clients and optional faults. [`runner::run`] plays it against a store chosen by [`stores`],
//! through the layer a [`modes::Mode`] names, from a seed, and returns a [`report::Report`]:
//! latency percentiles, store loads by need kind, schema decodes, bytes read, what the wrapper
//! reports of itself, conflicts and retries, and a coherence check that compares every read and
//! every decision with an unwrapped second handle over the same storage.
//!
//! What the harness needs from other work is behind small traits, each with a stand-in that
//! compiles today: [`corpus::CorpusLoader`] (WG-5398), [`generation::GenerationView`] (WG-5389)
//! and the layers of [`modes`] (WG-5390, WG-5391). Design: `docs/working-set.md`, "Experiments".

pub mod corpus;
pub mod decide;
mod delegate;
pub mod enterprise;
pub mod generation;
pub mod layer;
pub mod modes;
pub mod report;
pub mod rng;
pub mod runner;
pub mod scenario;
pub mod stores;
