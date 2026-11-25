//! Differential random testing of the Rust kernel against the Lean model, at scale.
//!
//! Run: `cargo run -p strata-verification --example drt -- --seed <S> --count <N> [--batch <B>]
//! [--out <DIR>] [--generator full|fixture] [--require-coverage]`
//!
//! The Lean side is the model unless `STRATA_LEAN_KERNEL` is `impl` or `indexed`, which runs the
//! executable kernel (stage 1 or the indexed stage 2) against the Rust kernel instead.
//!
//! `--count` scenarios come from the generator (`full` by default: every intent and value form,
//! see `strata_verification::lean::Generator`), in batches of `--batch` (default 1000). Each batch
//! is one `lake exe strata_model` process. The exit status is 1 when any scenario disagrees; the
//! first disagreement is minimised and written to `<DIR>/h-drt-<seed>-<n>.txt`, ready to append to
//! `lean/fixtures/scenarios.txt`. `--out` defaults to `target/drt` at the workspace root.
//!
//! The run prints how often the model showed each outcome kind. A kind that never occurred is
//! reported as a coverage gap; `--require-coverage` makes a gap a failure too.

use std::path::PathBuf;
use std::time::Instant;
use strata_verification::drt;
use strata_verification::lean::Generator;

const USAGE: &str = "usage: drt --seed <S> --count <N> [--batch <B>] [--out <DIR>] \
[--generator full|fixture] [--require-coverage]";

struct Options {
    seed: u64,
    count: usize,
    batch: usize,
    out: PathBuf,
    generator: Generator,
    require_coverage: bool,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut seed: Option<u64> = None;
    let mut count: Option<usize> = None;
    let mut batch = 1000;
    let mut out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("drt");
    let mut generator = Generator::Full;
    let mut require_coverage = false;
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        if flag == "--require-coverage" {
            require_coverage = true;
            i += 1;
            continue;
        }
        let value = args
            .get(i + 1)
            .map(String::as_str)
            .ok_or_else(|| format!("{flag} needs a value"))?;
        match flag {
            "--generator" => {
                generator = match value {
                    "full" => Generator::Full,
                    "fixture" => Generator::Fixture,
                    other => return Err(format!("--generator is full or fixture, not {other}")),
                }
            }
            "--seed" => {
                seed = Some(
                    value
                        .parse()
                        .map_err(|_| format!("--seed is not a number: {value}"))?,
                )
            }
            "--count" => {
                count = Some(
                    value
                        .parse()
                        .map_err(|_| format!("--count is not a number: {value}"))?,
                )
            }
            "--batch" => {
                batch = value
                    .parse()
                    .map_err(|_| format!("--batch is not a number: {value}"))?
            }
            "--out" => out = PathBuf::from(value),
            other => return Err(format!("unknown argument {other}")),
        }
        i += 2;
    }
    Ok(Options {
        seed: seed.ok_or("--seed is required")?,
        count: count.ok_or("--count is required")?,
        batch,
        out,
        generator,
        require_coverage,
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse(&args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            std::process::exit(2);
        }
    };
    let lean_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("lean");
    let started = Instant::now();
    let result = drt::run(
        &lean_dir,
        &options.out,
        options.seed,
        options.count,
        options.batch,
        options.generator,
    );
    let elapsed = started.elapsed();
    let totals = match result {
        Ok(totals) => totals,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    };
    println!("scenarios: {}", totals.scenarios);
    println!("operations: {}", totals.operations);
    println!("batches: {}", totals.batches);
    println!("disagreements: {}", totals.disagreements);
    println!("elapsed: {:.1} s", elapsed.as_secs_f64());
    if let Some(found) = &totals.minimised {
        println!(
            "first disagreement: scenario {}, minimised from {} to {} operations (reproduced on its own: {})",
            found.scenario, found.operations_before, found.operations_after, found.reproduced
        );
        if let Some((line, lean, kernel)) = &found.first_difference {
            println!("  line {line}: lean `{lean}`, kernel `{kernel}`");
        }
        println!("  written to {}", found.path.display());
    }
    print_coverage(&totals);
    let uncovered = totals.uncovered();
    if !uncovered.is_empty() {
        println!(
            "COVERAGE GAP: {} outcome kind(s) never reached by the model in this run: {}",
            uncovered.len(),
            uncovered.join(", ")
        );
    }
    if totals.disagreements > 0 || (options.require_coverage && !uncovered.is_empty()) {
        std::process::exit(1);
    }
}

/// One row per required outcome kind, with its count, then any other kind seen (`unparsed`).
fn print_coverage(totals: &drt::Totals) {
    println!("coverage (outcomes the model printed):");
    let width = totals.required.iter().map(String::len).max().unwrap_or(0);
    for kind in &totals.required {
        let count = totals.coverage.get(kind).copied().unwrap_or(0);
        let flag = if count == 0 { "  <- MISSING" } else { "" };
        println!("  {kind:<width$}  {count:>10}{flag}");
    }
    for (kind, count) in &totals.coverage {
        if !totals.required.contains(kind) {
            println!("  {kind:<width$}  {count:>10}  <- not a required kind");
        }
    }
}
