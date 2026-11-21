//! `ws-bench`: run one working-set scenario against a store and write its JSON report.
//!
//! ```text
//! ws-bench <scenario.jsonc> [--store memory|sqlite[:path]] [--mode none|header|full|stale]
//!          [--loader enterprise|stub] [--scale small|medium|large] [--reuse <sqlite path>]
//!          [--seed N] [--out report.json] [--coherence off|reads|full]
//!          [--max-divergences N] [--count-bytes] [--fail-on-divergence]
//! ```
//!
//! The population is the enterprise corpus (`strata-corpus`) at `--scale`; `--loader stub` uses the
//! small notes tenant of the kernel testkit instead. `--reuse <path>` opens a copy of a SQLite
//! store populated earlier (`strata-corpus build --scale ... --store sqlite:<path>`) and does not
//! rebuild it.

use std::process::ExitCode;
use strata_corpus::Scale;
use strata_ws_bench::corpus::{CorpusLoader, NotesLoader};
use strata_ws_bench::enterprise::EnterpriseLoader;
use strata_ws_bench::generation::StoreGeneration;
use strata_ws_bench::modes::Mode;
use strata_ws_bench::runner::{run, CoherenceLevel, Config};
use strata_ws_bench::scenario::Scenario;
use strata_ws_bench::stores::StoreSpec;

const USAGE: &str = "usage: ws-bench <scenario.jsonc> [--store memory|sqlite[:path]] \
[--mode none|header|full|stale] [--seed N] [--out report.json] [--coherence off|reads|full] \
[--max-divergences N] [--count-bytes] [--fail-on-divergence]";

/// The loader the harness populates tenants with.
fn loader(stub: bool, scale: Scale) -> Box<dyn CorpusLoader> {
    if stub {
        Box::new(NotesLoader)
    } else {
        Box::new(EnterpriseLoader { scale })
    }
}

fn main() -> ExitCode {
    match real_main() {
        Ok(code) => code,
        Err(message) => {
            eprintln!("ws-bench: {message}");
            ExitCode::from(2)
        }
    }
}

fn real_main() -> Result<ExitCode, String> {
    let mut args = std::env::args().skip(1);
    let mut scenario_path = None;
    let mut store = StoreSpec::Memory;
    let mut mode = Mode::None;
    let mut seed = 1u64;
    let mut out = None;
    let mut coherence = CoherenceLevel::Full;
    let mut max_divergences = 5usize;
    let mut count_bytes = false;
    let mut fail_on_divergence = false;
    let mut stub = false;
    let mut scale = Scale::Small;
    let mut reuse = false;
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--store" => store = StoreSpec::parse(&value("--store")?)?,
            "--mode" => mode = Mode::parse(&value("--mode")?)?,
            "--loader" => {
                stub = match value("--loader")?.as_str() {
                    "stub" => true,
                    "enterprise" => false,
                    other => return Err(format!("--loader {other:?}: enterprise or stub")),
                }
            }
            "--scale" => scale = value("--scale")?.parse::<Scale>()?,
            "--reuse" => {
                store = StoreSpec::SqliteReuse(value("--reuse")?.into());
                reuse = true;
            }
            "--seed" => {
                seed = value("--seed")?
                    .parse()
                    .map_err(|_| "--seed is a number".to_string())?
            }
            "--out" => out = Some(value("--out")?),
            "--coherence" => {
                coherence = match value("--coherence")?.as_str() {
                    "off" => CoherenceLevel::Off,
                    "reads" => CoherenceLevel::Reads,
                    "full" => CoherenceLevel::Full,
                    other => return Err(format!("--coherence {other:?}: off, reads or full")),
                }
            }
            "--max-divergences" => {
                max_divergences = value("--max-divergences")?
                    .parse()
                    .map_err(|_| "--max-divergences is a number".to_string())?
            }
            "--count-bytes" => count_bytes = true,
            "--fail-on-divergence" => fail_on_divergence = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown option {other}\n{USAGE}"))
            }
            other => {
                if scenario_path.replace(other.to_string()).is_some() {
                    return Err(format!("one scenario at a time\n{USAGE}"));
                }
            }
        }
    }
    let path = scenario_path.ok_or_else(|| format!("a scenario file is required\n{USAGE}"))?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    let scenario = Scenario::parse(&text).map_err(|e| format!("{path}: {e}"))?;
    let mut config = Config::new(store, mode, seed);
    config.coherence = coherence;
    config.max_divergences = max_divergences;
    config.count_bytes = count_bytes;
    config.reuse = reuse;
    let report = run(
        &scenario,
        &config,
        loader(stub, scale).as_ref(),
        &StoreGeneration,
    )?;
    let json = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    match &out {
        Some(file) => std::fs::write(file, &json).map_err(|e| format!("{file}: {e}"))?,
        None => println!("{json}"),
    }
    eprintln!(
        "{} on {} ({}): {} ops, p50 {} us, p99 {} us, {} loads, coherence {} checked, {} divergences",
        report.scenario,
        report.store,
        report.mode,
        report.length,
        report.timing.overall.p50_ns / 1_000,
        report.timing.overall.p99_ns / 1_000,
        report.store_side.counts.loads,
        report.coherence.checked,
        report.coherence.divergences,
    );
    if report.faults_ineffective > 0 {
        eprintln!(
            "ws-bench: {} fault(s) did not take effect: {}",
            report.faults_ineffective,
            report
                .faults
                .iter()
                .filter(|f| !f.effective)
                .map(|f| format!("{} at op {}: {}", f.kind, f.at_op, f.outcome))
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    if fail_on_divergence && (report.coherence.divergences > 0 || report.faults_ineffective > 0) {
        return Ok(ExitCode::from(1));
    }
    Ok(ExitCode::SUCCESS)
}
