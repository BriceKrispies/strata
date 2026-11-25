//! The golden fixtures: generating from `tests/fixtures/full/spec.jsonc` gives exactly the crate
//! `crates/spec-compiler-fixture`, and from `tests/fixtures/machines/spec.jsonc` exactly
//! `crates/spec-compiler-machines-fixture`, and from `tests/fixtures/wiring/spec.jsonc` exactly
//! `crates/spec-compiler-wiring-fixture`; the workspace compiles all three. Run with `SPECC_BLESS=1`
//! to rewrite those crates from the current compiler instead of comparing.

use std::fs;
use std::path::PathBuf;

use strata_spec_compiler::{generate, output, GeneratedFile, Options, RuntimeDependency};

/// The fixture's path as the headers name it: relative to the workspace root.
const FIXTURE_SPEC: &str = "crates/spec-compiler/tests/fixtures/full/spec.jsonc";
const FIXTURE_CRATE: &str = "strata-spec-compiler-fixture";
const MACHINES_SPEC: &str = "crates/spec-compiler/tests/fixtures/machines/spec.jsonc";
const MACHINES_CRATE: &str = "strata-spec-compiler-machines-fixture";
const WIRING_SPEC: &str = "crates/spec-compiler/tests/fixtures/wiring/spec.jsonc";
const WIRING_CRATE: &str = "strata-spec-compiler-wiring-fixture";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_text() -> String {
    fs::read_to_string(manifest_dir().join("tests/fixtures/full/spec.jsonc")).unwrap()
}

fn machines_text() -> String {
    fs::read_to_string(manifest_dir().join("tests/fixtures/machines/spec.jsonc")).unwrap()
}

fn wiring_text() -> String {
    fs::read_to_string(manifest_dir().join("tests/fixtures/wiring/spec.jsonc")).unwrap()
}

// The fixtures are generated once for every test in this binary; a test that needs a fresh
// generation calls `generated` itself.

fn fixture_files() -> &'static [GeneratedFile] {
    static FILES: std::sync::OnceLock<Vec<GeneratedFile>> = std::sync::OnceLock::new();
    FILES.get_or_init(|| generated(FIXTURE_SPEC, &fixture_text(), FIXTURE_CRATE))
}

fn machines_files() -> &'static [GeneratedFile] {
    static FILES: std::sync::OnceLock<Vec<GeneratedFile>> = std::sync::OnceLock::new();
    FILES.get_or_init(|| generated(MACHINES_SPEC, &machines_text(), MACHINES_CRATE))
}

fn wiring_files() -> &'static [GeneratedFile] {
    static FILES: std::sync::OnceLock<Vec<GeneratedFile>> = std::sync::OnceLock::new();
    FILES.get_or_init(|| generated(WIRING_SPEC, &wiring_text(), WIRING_CRATE))
}

fn generated(spec: &str, text: &str, crate_name: &str) -> Vec<GeneratedFile> {
    let options = Options {
        crate_name: crate_name.to_string(),
        runtime: RuntimeDependency::Workspace,
    };
    match generate(spec, text, &options) {
        Ok(files) => files,
        Err(diagnostics) => panic!(
            "the fixture was refused:\n{}",
            diagnostics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        ),
    }
}

#[test]
fn golden_output_matches_the_fixture_crate() {
    let dir = manifest_dir().join("../spec-compiler-fixture");
    let files = fixture_files();
    if std::env::var("SPECC_BLESS").as_deref() == Ok("1") {
        output::write(&dir, files).unwrap();
        return;
    }
    let differences = output::compare(&dir, files).unwrap();
    assert!(
        differences.is_empty(),
        "crates/spec-compiler-fixture is not what specc generates from the fixture: {differences:?}. \
         Run the test with SPECC_BLESS=1 to rewrite it, and review the change."
    );
}

#[test]
fn machines_golden_output_matches_its_fixture_crate() {
    let dir = manifest_dir().join("../spec-compiler-machines-fixture");
    let files = machines_files();
    if std::env::var("SPECC_BLESS").as_deref() == Ok("1") {
        output::write(&dir, files).unwrap();
        return;
    }
    let differences = output::compare(&dir, files).unwrap();
    assert!(
        differences.is_empty(),
        "crates/spec-compiler-machines-fixture is not what specc generates from the machines fixture: {differences:?}.          Run the test with SPECC_BLESS=1 to rewrite it, and review the change."
    );
}

#[test]
fn wiring_golden_output_matches_its_fixture_crate() {
    let dir = manifest_dir().join("../spec-compiler-wiring-fixture");
    let files = wiring_files();
    if std::env::var("SPECC_BLESS").as_deref() == Ok("1") {
        output::write(&dir, files).unwrap();
        return;
    }
    let differences = output::compare(&dir, files).unwrap();
    assert!(
        differences.is_empty(),
        "crates/spec-compiler-wiring-fixture is not what specc generates from the wiring fixture: {differences:?}.          Run the test with SPECC_BLESS=1 to rewrite it, and review the change."
    );
}

/// The compile-time guarantee of the `deps` module, shown on the wiring fixture, where `scheduler`
/// calls `reports` and `gateway` calls `scheduler`: the first function compiles; a client of a
/// component `scheduler` does not call has no accessor in the generated `SchedulerDeps`.
#[test]
fn a_component_reaches_only_the_clients_its_calls_declare() {
    use strata_spec_compiler_wiring_fixture::deps::SchedulerDeps;
    use strata_spec_compiler_wiring_fixture::routes::ReportsClient;

    fn reach(deps: &SchedulerDeps) -> &ReportsClient {
        deps.reports()
    }
    let _ = reach;

    let deps = wiring_files()
        .iter()
        .find(|f| f.path.ends_with("deps.rs"))
        .expect("the wiring fixture generates deps.rs");
    let start = deps.contents.find("pub struct SchedulerDeps").unwrap();
    let end = deps.contents.find("pub struct ReportsDeps").unwrap();
    let scheduler = &deps.contents[start..end];
    assert!(scheduler.contains("pub fn reports("));
    assert!(!scheduler.contains("gateway"), "{scheduler}");
}

const CORE_SPEC: &str = "crates/spec-compiler/tests/fixtures/core/spec.jsonc";
const CORE_CRATE: &str = "strata-spec-compiler-core-fixture";

fn core_text() -> String {
    fs::read_to_string(manifest_dir().join("tests/fixtures/core/spec.jsonc")).unwrap()
}

/// The pure crate of unions, maps and constraints: `crates/spec-compiler-core-fixture`, whose
/// own tests pin the JSON each form serialises to.
#[test]
fn core_golden_output_matches_its_fixture_crate() {
    let dir = manifest_dir().join("../spec-compiler-core-fixture");
    let files = match strata_spec_compiler::generate_core(CORE_SPEC, &core_text(), CORE_CRATE) {
        Ok(files) => files,
        Err(diagnostics) => panic!(
            "the core fixture was refused:\n{}",
            diagnostics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        ),
    };
    if std::env::var("SPECC_BLESS").as_deref() == Ok("1") {
        output::write(&dir, &files).unwrap();
        return;
    }
    let differences = output::compare(&dir, &files).unwrap();
    assert!(
        differences.is_empty(),
        "crates/spec-compiler-core-fixture is not what specc generates from the core fixture: {differences:?}. Run the test with SPECC_BLESS=1 to rewrite it, and review the change."
    );
}

const FUNCTIONS_SPEC: &str = "crates/spec-compiler/tests/fixtures/functions/spec.jsonc";
const FUNCTIONS_CRATE: &str = "strata-spec-compiler-functions-fixture";

/// The pure crate of the functions fixture (WG-4934): `crates/spec-compiler-functions-fixture`,
/// whose module `functions` holds the generated functions and, as its tests, their examples.
/// It lives here rather than in `tests/functions.rs`, which uses the generated crate and so
/// cannot compile before the crate is blessed.
#[test]
fn functions_golden_output_matches_its_fixture_crate() {
    let dir = manifest_dir().join("../spec-compiler-functions-fixture");
    let text =
        fs::read_to_string(manifest_dir().join("tests/fixtures/functions/spec.jsonc")).unwrap();
    let files = match strata_spec_compiler::generate_core(FUNCTIONS_SPEC, &text, FUNCTIONS_CRATE) {
        Ok(files) => files,
        Err(diagnostics) => panic!(
            "the functions fixture was refused:\n{}",
            diagnostics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        ),
    };
    let functions = file_text(&files, "src/functions.rs");
    for f in [
        "pub fn set_of(",
        "pub fn roles_of(",
        "pub fn policy_applies(",
        "pub fn may_attempt(",
        "pub fn authorize(",
        "pub fn check_version(",
        "pub fn next_version(",
        "fn check_version_example_1()",
        // WG-4966: timestamps, sets and tuple variants.
        "pub fn expires_at(",
        "now: crate::prelude::Timestamp,",
        "pub fn role_set(",
        "roles: &std::collections::BTreeSet<String>,",
        "types::Grant::Object(object, action) =>",
        "fn grants_of_example_1()",
        // WG-4966: shared and boxed values, f64, the string operations.
        "pub fn with_top(",
        "pub fn double_negation(",
        "pub fn num_text(",
        "pub fn millis(",
        "fn parse_time(",
        // WG-4966: checked conversions.
        "pub fn narrow(",
        "pub fn role_map(",
        "pub fn key_order(",
        // WG-5133: structural recursion is ordinary recursive Rust.
        "pub fn mentions_field(",
        // WG-5132: eval with a lookup, and durations.
        "pub fn follow_reference(",
        "fn eval_lookup_rule_language_rule_env(",
        "the lookup does not cover {field}",
        "pub fn later(",
    ] {
        assert!(functions.contains(f), "no {f} in src/functions.rs");
    }
    assert!(file_text(&files, "src/lib.rs").contains("pub mod functions;"));
    let manifest = file_text(&files, "Cargo.toml");
    for forbidden in ["strata-runtime", "uuid", "tokio", "[dev-dependencies]"] {
        assert!(!manifest.contains(forbidden), "{forbidden} in {manifest}");
    }
    if std::env::var("SPECC_BLESS").as_deref() == Ok("1") {
        output::write(&dir, &files).unwrap();
        return;
    }
    let differences = output::compare(&dir, &files).unwrap();
    assert!(
        differences.is_empty(),
        "crates/spec-compiler-functions-fixture is not what specc generates from the functions fixture: {differences:?}. Run the test with SPECC_BLESS=1 to rewrite it, and review the change."
    );
}

#[test]
fn generation_is_deterministic() {
    // A fresh generation against the one the other tests share.
    assert_eq!(
        generated(FIXTURE_SPEC, &fixture_text(), FIXTURE_CRATE),
        fixture_files()
    );
    assert_eq!(
        generated(MACHINES_SPEC, &machines_text(), MACHINES_CRATE),
        machines_files()
    );
    assert_eq!(
        generated(WIRING_SPEC, &wiring_text(), WIRING_CRATE),
        wiring_files()
    );
}

fn file_text(files: &[GeneratedFile], path: &str) -> String {
    files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("no {path}"))
        .contents
        .clone()
}

const DERIVED_SPEC: &str = "crates/spec-compiler/tests/fixtures/derived/spec.jsonc";
const DERIVED_CRATE: &str = "strata-spec-compiler-derived-fixture";

/// The model crate of the derived fixture (WG-4935): `crates/spec-compiler-derived-fixture`,
/// whose module `derived` implements the derived contracts from the functions and from the
/// load and commit contracts' trait. The facade `edit-counter` has types but no trait method and
/// no route. `tests/derived.rs` runs the generated loops; it uses the generated crate and so
/// cannot compile before the crate is blessed.
#[test]
fn derived_golden_output_matches_its_fixture_crate() {
    let dir = manifest_dir().join("../spec-compiler-derived-fixture");
    let text =
        fs::read_to_string(manifest_dir().join("tests/fixtures/derived/spec.jsonc")).unwrap();
    let files = generated(DERIVED_SPEC, &text, DERIVED_CRATE);
    let derived = file_text(&files, "src/derived.rs");
    for item in [
        "pub fn pick_larger(",
        "pub async fn read_counter<",
        "pub async fn copy_counter<",
        "pub async fn copy_counter_once<",
        "S0: contracts::Store + Sync",
        "crate::functions::decide_copy(",
        "break (loaded, decided)",
        "if rounds < 4 && !needs.contains(&need)",
        "if conflicts < 3 => conflicts += 1",
    ] {
        assert!(
            derived.contains(item),
            "no {item} in src/derived.rs:\n{derived}"
        );
    }
    // A bound of zero is no loop at all.
    let once = &derived[derived.find("pub async fn copy_counter_once<").unwrap()..];
    assert!(!once.contains("loop"), "{once}");
    assert!(file_text(&files, "src/lib.rs").contains("pub mod derived;"));
    let contracts = file_text(&files, "src/contracts.rs");
    assert!(contracts.contains("pub struct EditCounterRequest"));
    assert!(contracts.contains("realisation: Realisation::Facade,"));
    assert!(contracts.contains("realisation: Realisation::Derived,"));
    for method in ["fn load_counters(", "fn commit_bump(", "fn mutate_counter("] {
        assert!(contracts.contains(method), "no {method} in the Store trait");
    }
    assert!(
        !contracts.contains("fn edit_counter("),
        "the facade is in a trait"
    );
    let routes = file_text(&files, "src/routes.rs");
    assert!(
        !routes.contains("EditCounter"),
        "the facade is served:\n{routes}"
    );
    assert!(routes.contains("LoadCounters"));
    if std::env::var("SPECC_BLESS").as_deref() == Ok("1") {
        output::write(&dir, &files).unwrap();
        return;
    }
    let differences = output::compare(&dir, &files).unwrap();
    assert!(
        differences.is_empty(),
        "crates/spec-compiler-derived-fixture is not what specc generates from the derived fixture: {differences:?}. Run the test with SPECC_BLESS=1 to rewrite it, and review the change."
    );
}
