//! Rewrite the committed Lean fixtures (`lean/fixtures/scenarios.txt` and `expected.txt`) from the
//! generator and the kernel, after an intended change to either.
//!
//! This is the only place the fixtures are written. The test
//! `committed_fixtures_match_the_generator_and_the_kernel` only reads them and fails when they are
//! stale; it never writes. Run: `cargo run -p strata-verification --example bless_fixtures`.

use std::path::PathBuf;
use strata_verification::lean::{kernel_output, scenarios, FIXTURE_COUNT, FIXTURE_SEED};

fn main() {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("lean")
        .join("fixtures");
    let generated = scenarios(FIXTURE_SEED, FIXTURE_COUNT);
    let expected = kernel_output(&generated);
    std::fs::write(fixtures.join("scenarios.txt"), &generated).expect("write scenarios.txt");
    std::fs::write(fixtures.join("expected.txt"), &expected).expect("write expected.txt");
    println!("rewrote {}", fixtures.display());
}
