//! The Lean model and the Rust kernel agree on the committed scenarios, and the Lean proofs
//! check with no placeholder and no extra axiom.
//!
//! The scenario file and the expected output are committed, so the kernel side of the
//! comparison runs everywhere. The Lean side runs when `lake` is on the PATH and says so
//! loudly when it is not.

use std::path::PathBuf;
use std::process::Command;
use strata_verification::lean::{
    compare_outputs, kernel_output, model_output, parse_audit, scenarios, verify, FIXTURE_COUNT, FIXTURE_SEED,
};
use strata_verification::obligations;

fn lean_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../lean")
}

fn lake_available() -> bool {
    let found = Command::new("lake").arg("--version").output().is_ok_and(|o| o.status.success());
    if !found {
        eprintln!("SKIPPED: `lake` is not on the PATH, so the Lean side was not run. Install Lean 4 (elan) to run it.");
    }
    found
}

fn read(name: &str) -> String {
    std::fs::read_to_string(lean_dir().join("fixtures").join(name)).unwrap_or_default().replace("\r\n", "\n")
}

/// Set `STRATA_BLESS=1` to rewrite the committed files after an intended change.
#[test]
fn committed_fixtures_match_the_generator_and_the_kernel() {
    let generated = scenarios(FIXTURE_SEED, FIXTURE_COUNT);
    let expected = kernel_output(&generated);
    if std::env::var("STRATA_BLESS").as_deref() == Ok("1") {
        std::fs::write(lean_dir().join("fixtures/scenarios.txt"), &generated).unwrap();
        std::fs::write(lean_dir().join("fixtures/expected.txt"), &expected).unwrap();
    }
    assert_eq!(read("scenarios.txt"), generated, "lean/fixtures/scenarios.txt is stale; rerun with STRATA_BLESS=1");
    let committed = read("expected.txt");
    let difference = compare_outputs(&generated, &committed, &expected).first_difference;
    assert_eq!(difference, None, "the kernel no longer prints lean/fixtures/expected.txt");
}

#[test]
fn the_fixtures_exercise_every_outcome_the_model_has() {
    let generated = scenarios(FIXTURE_SEED, FIXTURE_COUNT);
    let output = kernel_output(&generated);
    for outcome in ["ok", "err unauthorized", "err not_found", "err invalid", "err migration_rejected", "err no_tenant"] {
        let count = output.lines().filter(|line| *line == outcome).count();
        assert!(count >= 5, "only {count} operations ended in `{outcome}`");
    }
    assert!(!output.contains("unmodelled") && !output.contains("unparsed"), "every kernel answer maps onto the model");
    assert!(output.lines().filter(|l| l.starts_with("record ")).count() > 50, "final states are not trivially empty");
    assert!(generated.lines().filter(|l| l.starts_with("op ")).count() > 900);
}

#[test]
fn the_lean_model_agrees_with_the_kernel_when_lean_is_installed() {
    if !lake_available() {
        return;
    }
    let generated = scenarios(FIXTURE_SEED, FIXTURE_COUNT);
    let lean = model_output(&lean_dir(), "fixtures/scenarios.txt").expect("the model runs");
    let result = compare_outputs(&generated, &lean, &kernel_output(&generated));
    assert!(result.agree, "model and kernel differ at line {:?}", result.first_difference);
    assert_eq!(result.scenarios, FIXTURE_COUNT);
}

#[test]
fn the_lean_proofs_check_with_only_standard_axioms_when_lean_is_installed() {
    if !lake_available() {
        return;
    }
    let report = verify(&lean_dir(), 0);
    assert_eq!(report.problem, None);
    assert!(report.build_ok && report.missing.is_empty(), "missing theorems: {:?}", report.missing);
    assert!(report.theorems.len() >= 9);
    for theorem in &report.theorems {
        assert!(theorem.clean, "{} depends on {:?}", theorem.theorem, theorem.axioms);
        assert!(!theorem.axioms.iter().any(|a| a == "sorryAx"));
    }
    assert!(report.all_checked);
    // The four required properties each name at least one checked theorem.
    for obligation in obligations::all() {
        assert!(obligation.theorems.iter().all(|t| report.theorems.iter().any(|c| &c.theorem == t)), "{}", obligation.id);
    }
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
            rest = rest[start..].find("-/").map_or("", |end| &rest[start + end + 2..]);
        }
        code.push_str(rest);
        let code: String = code.lines().map(|line| line.split("--").next().unwrap_or("")).collect::<Vec<_>>().join("\n");
        for forbidden in ["sorry", "admit", "axiom ", "native_decide", "unsafe "] {
            assert!(!code.contains(forbidden), "{file} contains `{forbidden}`");
        }
    }
}

#[test]
fn an_audit_line_with_a_placeholder_is_not_clean() {
    let audit = "'Strata.good' depends on axioms: [propext, Quot.sound]\n'Strata.bad' depends on axioms: [sorryAx]\n'Strata.pure' does not depend on any axioms\n";
    let parsed = parse_audit(audit);
    assert_eq!(parsed.iter().map(|c| c.clean).collect::<Vec<_>>(), [true, false, true]);
    assert_eq!(parsed[2].axioms, Vec::<String>::new());
}
