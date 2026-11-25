//! Differential random testing of the Rust kernel against the Lean model, at scale.
//!
//! Run: `cargo run -p strata-verification --example drt -- --seed <S> --count <N> [--batch <B>] [--out <DIR>]`
//!
//! `--count` scenarios come from the fixture generator (about 17 operations each), in batches of
//! `--batch` (default 1000). Each batch is one `lake exe strata_model` process. The exit status is 1
//! when any scenario disagrees; the first disagreement is minimised and written to
//! `<DIR>/h-drt-<seed>-<n>.txt`, ready to append to `lean/fixtures/scenarios.txt`. `--out` defaults
//! to `target/drt` at the workspace root.

use std::path::PathBuf;
use std::time::Instant;
use strata_verification::drt;

const USAGE: &str = "usage: drt --seed <S> --count <N> [--batch <B>] [--out <DIR>]";

struct Options {
    seed: u64,
    count: usize,
    batch: usize,
    out: PathBuf,
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
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        let value = args
            .get(i + 1)
            .map(String::as_str)
            .ok_or_else(|| format!("{flag} needs a value"))?;
        match flag {
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
    if totals.disagreements > 0 {
        std::process::exit(1);
    }
}
