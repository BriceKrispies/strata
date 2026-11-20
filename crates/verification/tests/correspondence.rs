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
    compare_outputs, kernel_output, model_output, parse_audit, scenarios, verify, FIXTURE_COUNT,
    FIXTURE_SEED,
};
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

/// This test never writes the committed files. After an intended change, rewrite them with
/// `cargo run -p strata-verification --example bless_fixtures` and review the diff.
#[test]
fn committed_fixtures_match_the_generator_and_the_kernel() {
    let generated = scenarios(FIXTURE_SEED, FIXTURE_COUNT);
    let expected = kernel_output(&generated);
    assert_eq!(
        read("scenarios.txt"),
        generated,
        "lean/fixtures/scenarios.txt is stale; run the bless_fixtures example"
    );
    let committed = read("expected.txt");
    let difference = compare_outputs(&generated, &committed, &expected).first_difference;
    assert_eq!(
        difference, None,
        "the kernel no longer prints lean/fixtures/expected.txt"
    );
}

#[test]
fn the_fixtures_exercise_every_outcome_the_model_has() {
    let generated = scenarios(FIXTURE_SEED, FIXTURE_COUNT);
    let output = kernel_output(&generated);
    for outcome in [
        "ok",
        "err unauthorized",
        "err not_found",
        "err invalid",
        "err migration_rejected",
        "err no_tenant",
    ] {
        let count = output.lines().filter(|line| *line == outcome).count();
        assert!(count >= 5, "only {count} operations ended in `{outcome}`");
    }
    assert!(
        !output.contains("unmodelled") && !output.contains("unparsed"),
        "every kernel answer maps onto the model"
    );
    assert!(
        output.lines().filter(|l| l.starts_with("record ")).count() > 50,
        "final states are not trivially empty"
    );
    assert!(generated.lines().filter(|l| l.starts_with("op ")).count() > 900);
}

/// Verifies: KRN-016
#[test]
fn the_lean_model_agrees_with_the_kernel() {
    let _lake = require_lake();
    let generated = scenarios(FIXTURE_SEED, FIXTURE_COUNT);
    // The scenarios the Lean program is given are the ones the kernel is given.
    assert_eq!(
        read("scenarios.txt"),
        generated,
        "lean/fixtures/scenarios.txt is not what the generator writes"
    );
    let lean = model_output(&lean_dir(), "fixtures/scenarios.txt").expect("the model runs");
    let result = compare_outputs(&generated, &lean, &kernel_output(&generated));
    assert!(
        result.agree,
        "model and kernel differ at line {:?}",
        result.first_difference
    );
    assert_eq!(result.scenarios, FIXTURE_COUNT);
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
fn the_citation_check_would_catch_a_dangling_name() {
    assert!(defines_function(
        "    fn real_test(
",
        "real_test"
    ));
    assert!(defines_function("async fn real_test() {}", "real_test"));
    assert!(!defines_function("fn real_test_two() {}", "real_test"));
    assert!(!defines_function("// real_test", "real_test"));
}

#[test]
fn the_lean_sources_contain_no_placeholder_proofs() {
    for file in ["Strata/Model.lean", "Strata/Proofs.lean"] {
        let source = std::fs::read_to_string(lean_dir().join(file)).unwrap();
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
        for forbidden in ["sorry", "admit", "axiom ", "native_decide", "unsafe "] {
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
