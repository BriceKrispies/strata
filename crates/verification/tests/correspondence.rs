//! The Lean model and the Rust kernel agree on the committed scenarios, and the Lean proofs
//! check with no placeholder and no extra axiom.
//!
//! The scenario file and the expected output are committed, so the kernel side of the
//! comparison runs everywhere. The Lean side needs `lake` on the PATH and fails when it is
//! missing: a skipped check must not report as passed.

use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use strata_verification::lean::{
    compare_outputs, hand_written, kernel_output, model_output, parse_audit, scenarios, verify,
    FIXTURE_COUNT, FIXTURE_SEED,
};
use strata_verification::lean_diff;
use strata_verification::obligations;

fn lean_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../lean")
}

/// Tests of this binary run on several threads, and two `lake` processes in one project directory
/// (`lake exe` builds, `lake build` builds) fight over its lock and its build files. Every test
/// that runs `lake` holds this lock for as long as it does; a failure in one does not hide in
/// the other, because a lock poisoned by a failed test is taken over, not treated as an error.
static LAKE: Mutex<()> = Mutex::new(());

/// The Lean side must run. A test that returns early when `lake` is missing reports ok without
/// checking anything, so a missing `lake` fails here instead. The returned guard keeps other
/// tests from running `lake` until it is dropped.
#[must_use = "hold the guard for as long as lake runs"]
fn require_lake() -> MutexGuard<'static, ()> {
    let guard = LAKE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let found = Command::new("lake")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    assert!(
        found,
        "`lake` is not on the PATH, so the Lean model and proofs were not checked. Install Lean 4 (elan)."
    );
    guard
}

fn read(name: &str) -> String {
    std::fs::read_to_string(lean_dir().join("fixtures").join(name))
        .unwrap_or_default()
        .replace("\r\n", "\n")
}

/// The committed scenario file is the generated scenarios followed by the hand-written ones
/// (scenarios named `h-...`, one or more for each decision of the model that the generator
/// does not aim at alone). The expected output is what the Lean model prints for the whole file.
/// This test never writes the committed files. After an intended change, rewrite them with
/// `cargo run -p strata-verification --example bless_fixtures` (the expected file is written
/// from the Lean model's output, never from the kernel's) and review the diff.
#[test]
fn committed_fixtures_match_the_generator_and_the_kernel() {
    let generated = scenarios(FIXTURE_SEED, FIXTURE_COUNT);
    let committed_scenarios = read("scenarios.txt");
    assert!(
        committed_scenarios.starts_with(&generated),
        "lean/fixtures/scenarios.txt does not begin with what the generator writes; run the bless_fixtures example"
    );
    assert_eq!(
        hand_written(&committed_scenarios),
        &committed_scenarios[generated.len()..],
        "everything after the generated scenarios must be hand-written scenarios named h-..."
    );
    let expected = kernel_output(&committed_scenarios);
    let committed = read("expected.txt");
    let difference = compare_outputs(&committed_scenarios, &committed, &expected).first_difference;
    assert_eq!(
        difference, None,
        "the kernel no longer prints lean/fixtures/expected.txt"
    );
}

/// Verifies: KRN-016
#[test]
fn the_lean_model_agrees_with_the_kernel() {
    let _lake = require_lake();
    let generated = scenarios(FIXTURE_SEED, FIXTURE_COUNT);
    let committed_scenarios = read("scenarios.txt");
    // The scenarios the Lean program is given are the ones the kernel is given.
    assert!(
        committed_scenarios.starts_with(&generated),
        "lean/fixtures/scenarios.txt does not begin with what the generator writes"
    );
    let lean = model_output(&lean_dir(), "fixtures/scenarios.txt").expect("the model runs");
    let result = compare_outputs(
        &committed_scenarios,
        &lean,
        &kernel_output(&committed_scenarios),
    );
    assert!(
        result.agree,
        "model and kernel differ at line {:?}",
        result.first_difference
    );
    assert_eq!(
        result.scenarios,
        FIXTURE_COUNT
            + hand_written(&committed_scenarios)
                .lines()
                .filter(|l| l.starts_with("scenario "))
                .count()
    );
}

#[test]
fn the_lean_proofs_check_with_only_standard_axioms() {
    let _lake = require_lake();
    let report = verify(&lean_dir(), 0);
    assert_eq!(report.problem, None);
    assert!(
        report.build_ok && report.missing.is_empty(),
        "missing theorems: {:?}",
        report.missing
    );
    assert!(report.theorems.len() >= 9);
    for theorem in &report.theorems {
        assert!(
            theorem.clean,
            "{} depends on {:?}",
            theorem.theorem, theorem.axioms
        );
        assert!(!theorem.axioms.iter().any(|a| a == "sorryAx"));
    }
    assert!(report.all_checked);
    // The four required properties each name at least one checked theorem.
    for obligation in obligations::all() {
        assert!(
            obligation
                .theorems
                .iter()
                .all(|t| report.theorems.iter().any(|c| &c.theorem == t)),
            "{}",
            obligation.id
        );
    }
}

/// The name of the package in `crates/<dir>/Cargo.toml`, for each crate directory.
fn crate_dirs() -> Vec<(String, PathBuf)> {
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&crates).unwrap().flatten() {
        let Ok(manifest) = std::fs::read_to_string(entry.path().join("Cargo.toml")) else {
            continue;
        };
        let name = manifest
            .lines()
            .skip_while(|l| l.trim() != "[package]")
            .find_map(|l| {
                let (key, value) = l.split_once('=')?;
                (key.trim() == "name").then(|| value.trim().trim_matches('"').to_string())
            });
        if let Some(name) = name {
            out.push((name, entry.path()));
        }
    }
    out
}

/// The Rust source files that can hold the tests of `module` in a crate: `tests/<module>.rs`,
/// `src/<module>.rs` and `src/<module>/mod.rs`.
fn module_files(dir: &std::path::Path, module: &str) -> Vec<PathBuf> {
    [
        dir.join("tests").join(format!("{module}.rs")),
        dir.join("src").join(format!("{module}.rs")),
        dir.join("src").join(module).join("mod.rs"),
    ]
    .into_iter()
    .filter(|p| p.is_file())
    .collect()
}

/// Whether `source` defines a function called `name`.
fn defines_function(source: &str, name: &str) -> bool {
    source.lines().any(|line| {
        let line = line.trim_start();
        line.split("fn ")
            .nth(1)
            .filter(|_| line.starts_with("fn ") || line.contains(" fn "))
            .is_some_and(|rest| {
                rest.strip_prefix(name)
                    .is_some_and(|after| after.starts_with(['(', '<']))
            })
    })
}

#[test]
fn every_cited_rust_test_exists() {
    let crates = crate_dirs();
    let mut missing = Vec::new();
    let mut checked = 0;
    for obligation in obligations::all() {
        for cited in &obligation.rust_tests {
            checked += 1;
            let parts: Vec<&str> = cited.split("::").collect();
            let [krate, module, name] = parts[..] else {
                missing.push(format!(
                    "{}: `{cited}` is not crate::module::function",
                    obligation.id
                ));
                continue;
            };
            let Some((_, dir)) = crates.iter().find(|(n, _)| n == krate) else {
                missing.push(format!("{}: `{cited}`: no crate {krate}", obligation.id));
                continue;
            };
            let found = module_files(dir, module).iter().any(|file| {
                defines_function(&std::fs::read_to_string(file).unwrap_or_default(), name)
            });
            if !found {
                missing.push(format!(
                    "{}: `{cited}`: no function {name} in {krate}'s {module} sources",
                    obligation.id
                ));
            }
        }
    }
    assert!(checked > 0, "the obligation table cites no Rust test");
    assert!(
        missing.is_empty(),
        "cited Rust tests that do not exist:
{}",
        missing.join(
            "
"
        )
    );
}

#[test]
fn the_lean_sources_contain_no_placeholder_proofs() {
    // Every file under lean/Strata (recursively), so a new slice is covered without editing this list.
    fn lean_files(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                lean_files(&path, out);
            } else if path.extension().is_some_and(|x| x == "lean") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    lean_files(&lean_dir().join("Strata"), &mut files);
    files.sort();
    assert!(files.len() >= 2, "lean/Strata holds {files:?}");
    for path in files {
        let file = path.display().to_string();
        let source = std::fs::read_to_string(&path).unwrap();
        // Strip comments, where the words may legitimately appear.
        let mut code = String::new();
        let mut rest = source.as_str();
        while let Some(start) = rest.find("/-") {
            code.push_str(&rest[..start]);
            rest = rest[start..]
                .find("-/")
                .map_or("", |end| &rest[start + end + 2..]);
        }
        code.push_str(rest);
        let code: String = code
            .lines()
            .map(|line| line.split("--").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        for forbidden in ["sorry", "native_decide"] {
            assert!(
                !words(&code).any(|(_, w)| w == forbidden),
                "{file} contains `{forbidden}`"
            );
        }
        // `admit` is not scanned for: it is also an ordinary name in the model (`def admit`, a
        // constructor `| admit`), and the tactic elaborates to sorryAx, which the axiom audit and
        // lake build's "declaration uses 'sorry'" both refuse.
        for forbidden in ["axiom ", "unsafe "] {
            assert!(!code.contains(forbidden), "{file} contains `{forbidden}`");
        }
    }
}

#[test]
fn an_audit_line_with_a_placeholder_is_not_clean() {
    let audit = "'Strata.good' depends on axioms: [propext, Quot.sound]\n'Strata.bad' depends on axioms: [sorryAx]\n'Strata.pure' does not depend on any axioms\n";
    let parsed = parse_audit(audit);
    assert_eq!(
        parsed.iter().map(|c| c.clean).collect::<Vec<_>>(),
        [true, false, true]
    );
    assert_eq!(parsed[2].axioms, Vec::<String>::new());
}

/// The default is sized to run in a few seconds of the Lean process; a larger run:
/// `STRATA_LEAN_DIFF_CASES=20000` (and `STRATA_LEAN_DIFF_SEED=<n>` to vary or reproduce).
const DEFAULT_DIFF_CASES: usize = 3000;
const DEFAULT_DIFF_SEED: u64 = 20_261_121;

fn env_number<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Verifies: KRN-016
#[test]
fn random_sequences_run_the_same_on_the_lean_model_and_the_kernel() {
    let _lake = require_lake();
    let cases = env_number("STRATA_LEAN_DIFF_CASES", DEFAULT_DIFF_CASES);
    let seed = env_number("STRATA_LEAN_DIFF_SEED", DEFAULT_DIFF_SEED);
    if let Err(report) = lean_diff::run(&lean_dir(), seed, cases) {
        panic!("{report}");
    }
}

/// A new intent in `Main.lean` must come with generator support, or it would run unexercised.
#[test]
fn the_generator_produces_every_intent_the_lean_model_executes() {
    let source = std::fs::read_to_string(lean_dir().join("Main.lean")).unwrap();
    let executed = lean_diff::lean_intents(&source).expect("Main.lean's intents can be read");
    let missing: Vec<&String> = executed
        .iter()
        .filter(|kind| !lean_diff::GENERATED_INTENTS.contains(&kind.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "Main.lean executes intents the differential generator never produces: {missing:?}; \
         add them to generate() and GENERATED_INTENTS in crates/verification/src/lean_diff.rs"
    );
    // The generator really does produce each kind it claims.
    let produced: String = (0..200)
        .map(|i| lean_diff::render("g", &[lean_diff::generate(lean_diff::case_seed(1, i))]))
        .collect();
    for kind in lean_diff::GENERATED_INTENTS {
        assert!(
            produced.lines().any(|l| l.split(' ').nth(3) == Some(kind)),
            "the generator never produces `{kind}`"
        );
    }
}

/// The identifiers of Lean source, each with the source before it (trailing spaces removed), so a
/// check can match whole words and look at what precedes one.
fn words(code: &str) -> impl Iterator<Item = (&str, &str)> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '\'';
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in code
        .char_indices()
        .chain(std::iter::once((code.len(), ' ')))
    {
        match (start, is_word(c)) {
            (None, true) => start = Some(i),
            (Some(s), false) => {
                out.push((code[..s].trim_end(), &code[s..i]));
                start = None;
            }
            _ => {}
        }
    }
    out.into_iter()
}

#[test]
fn the_placeholder_scan_matches_whole_words() {
    assert!(words(
        "theorem t : True := by
  sorry
"
    )
    .any(|(_, w)| w == "sorry"));
    assert!(!words("def sorryCount := 1").any(|(_, w)| w == "sorry"));
}
