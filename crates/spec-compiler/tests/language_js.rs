//! The tenant rule language as JavaScript (WG-5089): `specc web` writes, for each language the
//! specification declares, a module with its parser, printer, static check and evaluator, beside
//! the Rust one `specc generate` writes, so that a page can say a rule would be refused without
//! a round trip.
//!
//! - The golden: the module generated from the language fixture
//!   (`tests/fixtures/language/spec.jsonc`) is `tests/fixtures/language/golden/rule-language.js`
//!   (`SPECC_BLESS=1` rewrites it).
//! - The shared case table (`crates/spec-compiler-language-fixture/tests/kernel-cases.json`,
//!   which the generated Rust runs in `kernel_cases.rs` and Exegra runs on its own evaluator) is
//!   run through the generated JavaScript under `node`: the same parse, print, evaluation values
//!   and error texts, including `integer-overflow` at both ends of the range and
//!   `string-too-long`. The platform's own language (`tenant-rule-language`, whose AST is written
//!   with tuple variants and PascalCase tags) runs the same table through its sources.
//! - What the module cannot express is refused.
//!
//! The runs need `node` (see `support`).

mod support;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use strata_spec_compiler::{web, Diagnostic, GeneratedFile};

const LANGUAGE_SPEC: &str = "crates/spec-compiler/tests/fixtures/language/spec.jsonc";
const MODULE: &str = "rule-language.js";
const PLATFORM_MODULE: &str = "tenant-rule-language.js";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn language_text() -> String {
    fs::read_to_string(manifest_dir().join("tests/fixtures/language/spec.jsonc")).unwrap()
}

fn platform_text() -> String {
    fs::read_to_string(manifest_dir().join("../../specifications/spec.jsonc")).unwrap()
}

fn lines(d: &[Diagnostic]) -> String {
    d.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn generated(spec: &str, text: &str) -> Vec<GeneratedFile> {
    web::generate(spec, text).unwrap_or_else(|d| panic!("refused:\n{}", lines(&d)))
}

fn module<'a>(files: &'a [GeneratedFile], path: &str) -> &'a str {
    &files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("no generated file {path}"))
        .contents
}

/// Runs the shared case table on a generated module under node; the failures it prints.
fn run_table(name: &str, contents: &str, parsed_only: bool) -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("specc-language-js-{name}-{}", std::process::id()));
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("package.json"), "{ \"type\": \"module\" }\n").unwrap();
    let file = dir.join("language.js");
    fs::write(&file, contents).unwrap();
    let mut command = Command::new("node");
    command
        .arg(manifest_dir().join("tests/language-kernel-cases.mjs"))
        .arg(&file)
        .arg(manifest_dir().join("../spec-compiler-language-fixture/tests/kernel-cases.json"));
    if parsed_only {
        command.arg("--parsed");
    }
    let run = command.output().unwrap();
    let _ = fs::remove_dir_all(&dir);
    if run.status.success() {
        Ok(String::from_utf8_lossy(&run.stdout).into_owned())
    } else {
        Err(format!(
            "{}\n{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        ))
    }
}

#[test]
fn the_language_fixture_gives_the_golden_javascript_module() {
    let files = generated(LANGUAGE_SPEC, &language_text());
    let golden = manifest_dir().join("tests/fixtures/language/golden/rule-language.js");
    let made = module(&files, MODULE);
    if std::env::var("SPECC_BLESS").as_deref() == Ok("1") {
        fs::write(&golden, made).unwrap();
        return;
    }
    let expected = fs::read_to_string(&golden).unwrap();
    assert!(
        expected.replace('\r', "") == made.replace('\r', ""),
        "the JavaScript module of the language fixture is not tests/fixtures/language/golden/rule-language.js. Run the test with SPECC_BLESS=1 to rewrite it, and review the change."
    );
}

#[test]
fn a_language_gets_one_module_beside_the_machines_and_the_kernel() {
    let files = generated(LANGUAGE_SPEC, &language_text());
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, [MODULE]);
    let platform = generated("specifications/spec.jsonc", &platform_text());
    assert!(platform.iter().any(|f| f.path == PLATFORM_MODULE));
    assert!(platform.iter().any(|f| f.path == "screens.js"));
}

// Verifies: CMS-1546, CMS-2360, CMS-2361
#[test]
fn the_generated_javascript_gives_every_result_of_the_shared_table() {
    if !support::node_required("the run of the generated rule language on the shared case table") {
        return;
    }
    let files = generated(LANGUAGE_SPEC, &language_text());
    match run_table("fixture", module(&files, MODULE), false) {
        Ok(report) => assert!(report.contains("eval cases"), "{report}"),
        Err(report) => panic!("the generated JavaScript departs from the shared table:\n{report}"),
    }
}

// Verifies: CMS-1546, CMS-2360, CMS-2361
#[test]
fn the_platform_language_in_javascript_gives_every_result_of_the_shared_table() {
    if !support::node_required("the run of the platform rule language on the shared case table") {
        return;
    }
    let files = generated("specifications/spec.jsonc", &platform_text());
    match run_table("platform", module(&files, PLATFORM_MODULE), true) {
        Ok(report) => assert!(report.contains("eval cases"), "{report}"),
        Err(report) => {
            panic!("the platform language in JavaScript departs from the shared table:\n{report}")
        }
    }
}

#[test]
fn the_platform_module_reads_the_tuple_variants_and_pascal_case_tags_the_server_writes() {
    let files = generated("specifications/spec.jsonc", &platform_text());
    let js = module(&files, PLATFORM_MODULE);
    // `has-role` is written HasRole; `cmp` is a tuple variant of an operator, left and right.
    assert!(js.contains("case 'HasRole': {"), "{js}");
    assert!(js.contains("const [v0, v1, v2] = p;"), "{js}");
    assert!(js.contains("{ Cmp: ['Eq', left, this.level_"), "{js}");
    // A role is refused as the kernel's role id refuses one: by length, then by pattern.
    assert!(
        js.contains("a value of type role-id breaks its maxLength constraint"),
        "{js}"
    );
    assert!(
        js.contains("a value of type role-id breaks its pattern constraint"),
        "{js}"
    );
}

#[test]
fn a_language_that_the_javascript_module_cannot_express_is_refused() {
    // A union the module reads that is not externally tagged.
    let adjacent = language_text().replace(
        "{ \"id\": \"expr\", \"name\": \"Expression\", \"kind\": \"union\", \"variants\": [",
        "{ \"id\": \"expr\", \"name\": \"Expression\", \"kind\": \"union\", \"encoding\": { \"tag\": \"adjacent\", \"tagField\": \"t\", \"contentField\": \"c\" }, \"variants\": [",
    );
    assert_ne!(adjacent, language_text());
    let refused = web::generate(LANGUAGE_SPEC, &adjacent).unwrap_err();
    assert!(
        refused
            .iter()
            .any(|d| d.to_string().contains("specc.language-js-unsupported")
                && d.to_string().contains("not externally tagged")),
        "{}",
        lines(&refused)
    );
    // An id that is the name of another generated module.
    let taken = language_text().replacen("\"id\": \"rule-language\"", "\"id\": \"routes\"", 1);
    assert_ne!(taken, language_text());
    let refused = web::generate(LANGUAGE_SPEC, &taken).unwrap_err();
    assert!(
        refused
            .iter()
            .any(|d| d.to_string().contains("specc.language-js-name")),
        "{}",
        lines(&refused)
    );
    // An id that cannot name a file.
    let odd = language_text().replacen("\"id\": \"rule-language\"", "\"id\": \"a b\"", 1);
    let refused = web::generate(LANGUAGE_SPEC, &odd).unwrap_err();
    assert!(
        refused
            .iter()
            .any(|d| d.to_string().contains("specc.language-js-name")),
        "{}",
        lines(&refused)
    );
}
