//! The golden fixtures: generating from `tests/fixtures/full/spec.jsonc` gives exactly the crate
//! `crates/spec-compiler-fixture`, and from `tests/fixtures/machines/spec.jsonc` exactly
//! `crates/spec-compiler-machines-fixture`; the workspace compiles both. Run with `SPECC_BLESS=1`
//! to rewrite those crates from the current compiler instead of comparing.

use std::fs;
use std::path::{Path, PathBuf};

use strata_spec_compiler::emit::HEADER_MARKERS;
use strata_spec_compiler::{generate, output, GeneratedFile, Options, RuntimeDependency};

/// The fixture's path as the headers name it: relative to the workspace root.
const FIXTURE_SPEC: &str = "crates/spec-compiler/tests/fixtures/full/spec.jsonc";
const FIXTURE_CRATE: &str = "strata-spec-compiler-fixture";
const MACHINES_SPEC: &str = "crates/spec-compiler/tests/fixtures/machines/spec.jsonc";
const MACHINES_CRATE: &str = "strata-spec-compiler-machines-fixture";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_text() -> String {
    fs::read_to_string(manifest_dir().join("tests/fixtures/full/spec.jsonc")).unwrap()
}

fn fixture_files() -> Vec<GeneratedFile> {
    generated(FIXTURE_SPEC, &fixture_text(), FIXTURE_CRATE)
}

fn machines_files() -> Vec<GeneratedFile> {
    let text =
        fs::read_to_string(manifest_dir().join("tests/fixtures/machines/spec.jsonc")).unwrap();
    generated(MACHINES_SPEC, &text, MACHINES_CRATE)
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

/// A fresh, empty directory under the system temporary directory.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("specc-{name}-{}", std::process::id()));
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn golden_output_matches_the_fixture_crate() {
    let dir = manifest_dir().join("../spec-compiler-fixture");
    let files = fixture_files();
    if std::env::var("SPECC_BLESS").as_deref() == Ok("1") {
        output::write(&dir, &files).unwrap();
        return;
    }
    let differences = output::compare(&dir, &files).unwrap();
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
        output::write(&dir, &files).unwrap();
        return;
    }
    let differences = output::compare(&dir, &files).unwrap();
    assert!(
        differences.is_empty(),
        "crates/spec-compiler-machines-fixture is not what specc generates from the machines fixture: {differences:?}.          Run the test with SPECC_BLESS=1 to rewrite it, and review the change."
    );
}

#[test]
fn only_a_specification_with_machines_gets_a_machines_module_and_the_runtime() {
    let full = fixture_files();
    assert!(!full.iter().any(|f| f.path == "src/machines.rs"));
    let manifest = |files: &[GeneratedFile]| {
        files
            .iter()
            .find(|f| f.path == "Cargo.toml")
            .unwrap()
            .contents
            .clone()
    };
    assert!(!manifest(&full).contains("strata-runtime"));

    let machines = machines_files();
    let paths: Vec<&str> = machines.iter().map(|f| f.path.as_str()).collect();
    assert!(paths.contains(&"src/machines.rs"));
    assert!(manifest(&machines).contains(
        "strata-runtime.workspace = true
"
    ));

    let text =
        fs::read_to_string(manifest_dir().join("tests/fixtures/machines/spec.jsonc")).unwrap();
    let options = Options {
        crate_name: "out".to_string(),
        runtime: RuntimeDependency::Path("../../crates/runtime".to_string()),
    };
    let files = generate(MACHINES_SPEC, &text, &options).unwrap();
    assert!(manifest(&files).contains(
        "strata-runtime = { path = \"../../crates/runtime\" }
"
    ));
}

#[test]
fn machine_steps_follow_the_effect_order() {
    let machines = machines_files();
    let code = &machines
        .iter()
        .find(|f| f.path == "src/machines.rs")
        .unwrap()
        .contents;
    // The job's retry emits before it sets: the payload is built before the set, from the
    // variables as they were.
    let emitted = code.find("let emitted_1 = Emitted::JobStarted").unwrap();
    let set = code[emitted..].find("next.attempts = ").unwrap();
    assert!(set > 0);
    // Exegra's time is the step's now.
    assert!(code.contains("next.granted = prelude::Timestamp(now.0);"));
    // Ignored and unmatched events keep the state and variables.
    assert!(code.contains("_ => Ok(Outcome::to(*state, vars.clone())),"));
}

#[test]
fn the_crate_has_the_expected_files_each_with_a_header() {
    let files = fixture_files();
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "Cargo.toml",
            "src/contracts.rs",
            "src/lib.rs",
            "src/prelude.rs",
            "src/records.rs",
            "src/types.rs"
        ]
    );
    for file in &files {
        let first = file.contents.lines().next().unwrap();
        assert!(
            HEADER_MARKERS.iter().any(|m| first.starts_with(m)),
            "{} has no header",
            file.path
        );
        assert!(first.contains(&format!("from {FIXTURE_SPEC}.")));
        assert!(!file.contents.contains('\r'));
    }
}

#[test]
fn generation_is_deterministic() {
    assert_eq!(fixture_files(), fixture_files());
    assert_eq!(machines_files(), machines_files());
}

#[test]
fn the_hole_carries_its_implements_line() {
    let files = fixture_files();
    let contracts = &files
        .iter()
        .find(|f| f.path == "src/contracts.rs")
        .unwrap()
        .contents;
    assert!(contracts.contains(
        "/// Hole H-7: hand-written. The implementing item carries `/// Implements: H-7`."
    ));
}

#[test]
fn vacuous_bounds_are_not_checked() {
    let files = fixture_files();
    let records = &files
        .iter()
        .find(|f| f.path == "src/records.rs")
        .unwrap()
        .contents;
    // `quota` has min 0 on a u32: always true, so no comparison is emitted for it.
    assert!(!records.contains("value.0 < 0 "));
    assert!(records.contains("value.0 > 10"));
}

#[test]
fn check_reports_every_kind_of_difference_and_write_repairs_them() {
    let dir = scratch_dir("check");
    let files = fixture_files();
    output::write(&dir, &files).unwrap();
    assert!(output::compare(&dir, &files).unwrap().is_empty());

    fs::write(dir.join("src/types.rs"), "// edited by hand\n").unwrap();
    fs::remove_file(dir.join("src/records.rs")).unwrap();
    fs::write(
        dir.join("src/old.rs"),
        format!("{} from an older specification.\n", HEADER_MARKERS[0]),
    )
    .unwrap();
    fs::write(dir.join("src/hand.rs"), "// written by hand\n").unwrap();

    let differences = output::compare(&dir, &files).unwrap();
    assert_eq!(differences.differing, ["src/types.rs"]);
    assert_eq!(differences.missing, ["src/records.rs"]);
    assert_eq!(differences.extra, ["src/old.rs"]);

    let removed = output::write(&dir, &files).unwrap();
    assert_eq!(removed, ["src/old.rs"]);
    assert!(output::compare(&dir, &files).unwrap().is_empty());
    assert!(
        dir.join("src/hand.rs").exists(),
        "a file without the header is never touched"
    );

    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn crlf_line_endings_are_not_a_difference() {
    let dir = scratch_dir("crlf");
    let files = fixture_files();
    output::write(&dir, &files).unwrap();
    let lib = dir.join("src/lib.rs");
    let crlf = fs::read_to_string(&lib).unwrap().replace('\n', "\r\n");
    fs::write(&lib, crlf).unwrap();
    assert!(output::compare(&dir, &files).unwrap().is_empty());
    fs::remove_dir_all(&dir).unwrap();
}

fn specc(args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_specc"))
        .args(args)
        .output()
        .unwrap()
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[test]
fn the_command_generates_checks_and_refuses() {
    let dir = scratch_dir("cli");
    let out = dir.join("generated-crate");
    let spec = manifest_dir().join("tests/fixtures/full/spec.jsonc");
    let (spec_arg, out_arg) = (path_arg(&spec), path_arg(&out));

    let generated = specc(&["generate", "--spec", &spec_arg, "--out", &out_arg]);
    assert!(generated.status.success(), "{generated:?}");
    let manifest = fs::read_to_string(out.join("Cargo.toml")).unwrap();
    assert!(manifest.contains("name = \"generated-crate\""));

    let clean = specc(&["check", "--spec", &spec_arg, "--out", &out_arg]);
    assert_eq!(clean.status.code(), Some(0), "{clean:?}");

    fs::write(out.join("src/types.rs"), "// edited by hand\n").unwrap();
    let dirty = specc(&["check", "--spec", &spec_arg, "--out", &out_arg]);
    assert_eq!(dirty.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&dirty.stdout).contains("differs: src/types.rs"));

    let unready = dir.join("unready.jsonc");
    fs::write(
        &unready,
        r#"{ "system": { "id": "s", "name": "S" },
            "records": [ { "id": "r", "name": "R", "owner": "c", "key": ["a"], "fields": [ { "name": "a", "type": "nope" } ] } ] }"#,
    )
    .unwrap();
    let refused = specc(&["generate", "--spec", &path_arg(&unready), "--out", &out_arg]);
    assert_eq!(refused.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("error[ready.field-type-undeclared] r: "));
    assert!(stderr.contains("error[ready.schema-version-missing] r: "));

    let usage = specc(&["generate", "--spec", &spec_arg]);
    assert_eq!(usage.status.code(), Some(2));

    fs::remove_dir_all(&dir).unwrap();
}
