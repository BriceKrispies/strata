//! Rewrite the committed Lean fixtures (`lean/fixtures/scenarios.txt` and `expected.txt`) after an
//! intended change to the generator or the model.
//!
//! The scenario file is the generated scenarios followed by the hand-written ones, which this
//! keeps as they are. The expected file is what the Lean model prints for the whole file: it is
//! never written from the kernel, so that the committed output is the model's.
//!
//! This is the only place the fixtures are written. The test
//! `committed_fixtures_match_the_generator_and_the_kernel` only reads them and fails when they are
//! stale; it never writes. Run: `cargo run -p strata-verification --example bless_fixtures`.

use std::path::PathBuf;
use strata_verification::lean::{
    hand_written, model_output, scenarios, FIXTURE_COUNT, FIXTURE_SEED,
};

fn main() {
    let lean_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("lean");
    let fixtures = lean_dir.join("fixtures");
    let current = std::fs::read_to_string(fixtures.join("scenarios.txt"))
        .unwrap_or_default()
        .replace("\r\n", "\n");
    let text = format!(
        "{}{}",
        scenarios(FIXTURE_SEED, FIXTURE_COUNT),
        hand_written(&current)
    );
    std::fs::write(fixtures.join("scenarios.txt"), &text).expect("write scenarios.txt");
    let expected = model_output(&lean_dir, "fixtures/scenarios.txt").expect("the Lean model runs");
    std::fs::write(fixtures.join("expected.txt"), expected).expect("write expected.txt");
    println!("rewrote {}", fixtures.display());
}
