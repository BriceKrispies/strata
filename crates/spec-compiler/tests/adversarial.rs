//! Adversarial specification text: names, descriptions and string literals that would end a
//! comment, close a literal, start a line of code or name a reserved word if they reached the
//! generated code unescaped (WG-4898).
//!
//! `tests/fixtures/adversarial/spec.jsonc` holds the hostile text. The tests show that:
//!
//! - the generated JavaScript parses (`node --check`) and holds no raw line separator, control
//!   character or bidirectional control, and the hostile text only ever appears in comments or
//!   string literals;
//! - the generated Rust parses (`syn`), with the same checks, and is the golden crate
//!   `crates/spec-compiler-adversarial-fixture` (a member of the workspace, so building the
//!   workspace compiles it);
//! - `scope.` inside a string literal is left alone (`telescope` stays `telescope`);
//! - reserved words (`class`, `new`, `default`) and ids that start a digit after a hyphen become
//!   valid identifiers;
//! - two ids that become one JavaScript name, or an id that becomes a name the module uses
//!   itself, are refused with `specc.identifier-collision`
//!   (`tests/fixtures/adversarial-collision/spec.jsonc`).
//!
//! `SPECC_BLESS=1` rewrites the golden directories.

mod support;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use strata_spec_compiler::codetext::IDENTIFIER_COLLISION;
use strata_spec_compiler::RuntimeDependency;
use strata_spec_compiler::{generate, output, web, Diagnostic, GeneratedFile, Options};

const SPEC: &str = "crates/spec-compiler/tests/fixtures/adversarial/spec.jsonc";
const COLLISION: &str = "crates/spec-compiler/tests/fixtures/adversarial-collision/spec.jsonc";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn text(name: &str) -> String {
    fs::read_to_string(manifest_dir().join("tests/fixtures").join(name)).unwrap()
}

fn lines_of(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn web_files() -> Vec<GeneratedFile> {
    match web::generate(SPEC, &text("adversarial/spec.jsonc")) {
        Ok(files) => files,
        Err(d) => panic!("the adversarial fixture was refused:\n{}", lines_of(&d)),
    }
}

fn rust_files() -> Vec<GeneratedFile> {
    let options = Options {
        crate_name: "strata-adversarial-fixture".to_string(),
        runtime: RuntimeDependency::Workspace,
    };
    match generate(SPEC, &text("adversarial/spec.jsonc"), &options) {
        Ok(files) => files,
        Err(d) => panic!("the adversarial fixture was refused:\n{}", lines_of(&d)),
    }
}

fn file<'a>(files: &'a [GeneratedFile], path: &str) -> &'a str {
    &files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("no generated file {path}"))
        .contents
}

/// A character that must never be in generated code as it is: a control character other than the
/// line feed, a line or paragraph separator, or a bidirectional control.
fn is_raw_hazard(c: char) -> bool {
    (c.is_control() && c != '\n')
        || matches!(
            c,
            '\u{2028}' | '\u{2029}' | '\u{61c}' | '\u{200e}' | '\u{200f}'
        )
        || ('\u{202a}'..='\u{202e}').contains(&c)
        || ('\u{2066}'..='\u{2069}').contains(&c)
}

fn assert_no_raw_hazards(files: &[GeneratedFile]) {
    for f in files {
        for (n, line) in f.contents.lines().enumerate() {
            if let Some(c) = line.chars().find(|c| is_raw_hazard(*c)) {
                panic!(
                    "{}:{}: raw U+{:04X} in generated code: {line}",
                    f.path,
                    n + 1,
                    c as u32
                );
            }
        }
        assert!(
            !f.contents.contains('\r'),
            "{}: a bare carriage return",
            f.path
        );
    }
}

/// True when a line is a comment line (`//`, a JSDoc line or a block's start or end).
fn is_comment_line(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//") || t.starts_with('*') || t.starts_with("/**") || t.starts_with("/*")
}

/// Every line that mentions `needle` is a comment line or has the needle inside a quoted string.
fn only_in_comments_or_strings(files: &[GeneratedFile], needle: &str) {
    for f in files {
        for (n, line) in f.contents.lines().enumerate() {
            if let Some(at) = line.find(needle) {
                let quotes = line[..at].matches('\'').count() + line[..at].matches('"').count();
                assert!(
                    is_comment_line(line) || quotes % 2 == 1,
                    "{}:{}: `{needle}` outside a comment or a string: {line}",
                    f.path,
                    n + 1
                );
            }
        }
    }
}

#[test]
fn generated_javascript_parses_and_holds_no_raw_hazard() {
    let files = web_files();
    assert_no_raw_hazards(&files);
    if !support::node_required("the node --check of the adversarial files") {
        return;
    }
    let dir = std::env::temp_dir().join(format!("specc-adversarial-{}", std::process::id()));
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    output::write(&dir, &files).unwrap();
    fs::write(dir.join("package.json"), "{ \"type\": \"module\" }\n").unwrap();
    for f in &files {
        let run = Command::new("node")
            .arg("--check")
            .arg(dir.join(&f.path))
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "node --check {}:\n{}",
            f.path,
            String::from_utf8_lossy(&run.stderr)
        );
    }
}

#[test]
fn hostile_text_is_only_ever_a_comment_or_a_string() {
    let files = web_files();
    for needle in ["injected()", "fake comment", "evil}", "fn injected"] {
        only_in_comments_or_strings(&files, needle);
    }
    // The machine's description is one comment line per line, whatever ended the line.
    let machine = file(&files, "login-form.js");
    assert!(
        machine.contains("// First line * / not the end\n"),
        "{machine}"
    );
    assert!(machine.contains("// second after U+2028\n"), "{machine}");
    assert!(machine.contains("// third after U+2029\n"), "{machine}");
    assert!(
        machine
            .contains("// fourth after a bare CR<U+0001> control <U+202E> bidi // fake comment\n"),
        "{machine}"
    );
    assert!(machine.contains("// CRLF line\n"), "{machine}");
    assert!(machine.contains("// /* open @type {evil}\n"), "{machine}");
    // The name's U+2028 ends a comment line too.
    assert!(
        machine.contains("// Machine `login-form`: Login * / form \n//  injected().\n"),
        "{machine}"
    );
    // A `*/` of a contract name cannot close its JSDoc block.
    let contracts = file(&files, "contracts.js");
    assert!(!contracts.contains("Class */"), "{contracts}");
    assert!(contracts.contains("Class * /"), "{contracts}");
    assert!(!contracts.contains("@type {evil}"), "{contracts}");
}

#[test]
fn reserved_words_and_digits_become_valid_names() {
    let files = web_files();
    let contracts = file(&files, "contracts.js");
    assert!(
        contracts.contains("export function class_(inputs)"),
        "{contracts}"
    );
    assert!(
        contracts.contains("export function step2(inputs)"),
        "{contracts}"
    );
    assert!(!contracts.contains("export function class("), "{contracts}");
    let forms = file(&files, "forms.js");
    assert!(forms.contains("const new_ = {"), "{forms}");
    assert!(!forms.contains("const new = {"), "{forms}");
    let actions = file(&files, "actions.js");
    assert!(actions.contains("const default_ = {"), "{actions}");
    // The ids themselves stay what they are in the tables.
    assert!(forms.contains("'new': new_,"), "{forms}");
    assert!(actions.contains("'default': default_,"), "{actions}");
}

#[test]
fn scope_inside_a_string_literal_is_not_rewritten() {
    let files = web_files();
    let forms = file(&files, "forms.js");
    assert!(forms.contains("telescope scope.principal.id"), "{forms}");
    assert!(!forms.contains("telecontext"), "{forms}");
    let actions = file(&files, "actions.js");
    assert!(
        actions.contains("telescope scope.principal.id"),
        "{actions}"
    );
    // The form's derivation reads only the form, so its context parameter is unused.
    assert!(
        forms.contains("derive(values, edited, _context)"),
        "{forms}"
    );
}

#[test]
fn generated_rust_parses_and_holds_no_raw_hazard() {
    let files = rust_files();
    assert_no_raw_hazards(&files);
    for f in files.iter().filter(|f| f.path.ends_with(".rs")) {
        if let Err(e) = syn::parse_file(&f.contents) {
            panic!("{} does not parse as Rust: {e}", f.path);
        }
    }
}

#[test]
fn golden_outputs_match_the_fixture_directories() {
    let web_dir = manifest_dir().join("tests/fixtures/adversarial/web");
    let crate_dir = manifest_dir().join("../spec-compiler-adversarial-fixture");
    let (web, rust) = (web_files(), rust_files());
    if std::env::var("SPECC_BLESS").as_deref() == Ok("1") {
        output::write(&web_dir, &web).unwrap();
        output::write(&crate_dir, &rust).unwrap();
        return;
    }
    for (dir, files) in [(&web_dir, &web), (&crate_dir, &rust)] {
        let differences = output::compare(dir, files).unwrap();
        assert!(
            differences.is_empty(),
            "{} is not what specc generates from the adversarial fixture: {differences:?}. \
             Run the test with SPECC_BLESS=1 to rewrite it, and review the change.",
            dir.display()
        );
    }
}

#[test]
fn colliding_and_reserved_ids_are_refused() {
    let Err(diagnostics) = web::generate(COLLISION, &text("adversarial-collision/spec.jsonc"))
    else {
        panic!("the colliding ids were not refused");
    };
    let refused = |entity: &str| {
        diagnostics
            .iter()
            .any(|d| d.code == IDENTIFIER_COLLISION && d.entity == entity)
    };
    // The second of two ids that make one name is the one refused; the first keeps the name.
    assert!(refused("tab_a"), "{}", lines_of(&diagnostics));
    assert!(refused("get_x"), "{}", lines_of(&diagnostics));
    // A form called `input` would shadow forms.js's own `input` function.
    assert!(refused("input"), "{}", lines_of(&diagnostics));
    assert!(!refused("tab-a"), "{}", lines_of(&diagnostics));
    assert!(!refused("get-x"), "{}", lines_of(&diagnostics));
    for d in diagnostics
        .iter()
        .filter(|d| d.code == IDENTIFIER_COLLISION)
    {
        assert!(d.message.contains("identifier"), "{d}");
    }
}
