//! The golden fixtures: generating from `tests/fixtures/full/spec.jsonc` gives exactly the crate
//! `crates/spec-compiler-fixture`, and from `tests/fixtures/machines/spec.jsonc` exactly
//! `crates/spec-compiler-machines-fixture`, and from `tests/fixtures/wiring/spec.jsonc` exactly
//! `crates/spec-compiler-wiring-fixture`; the workspace compiles all three. Run with `SPECC_BLESS=1`
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
const WIRING_SPEC: &str = "crates/spec-compiler/tests/fixtures/wiring/spec.jsonc";
const WIRING_CRATE: &str = "strata-spec-compiler-wiring-fixture";

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

fn wiring_files() -> Vec<GeneratedFile> {
    let text = fs::read_to_string(manifest_dir().join("tests/fixtures/wiring/spec.jsonc")).unwrap();
    generated(WIRING_SPEC, &text, WIRING_CRATE)
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
fn wiring_golden_output_matches_its_fixture_crate() {
    let dir = manifest_dir().join("../spec-compiler-wiring-fixture");
    let files = wiring_files();
    if std::env::var("SPECC_BLESS").as_deref() == Ok("1") {
        output::write(&dir, &files).unwrap();
        return;
    }
    let differences = output::compare(&dir, &files).unwrap();
    assert!(
        differences.is_empty(),
        "crates/spec-compiler-wiring-fixture is not what specc generates from the wiring fixture: {differences:?}.          Run the test with SPECC_BLESS=1 to rewrite it, and review the change."
    );
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

/// A function specc cannot compile is refused with its code, and nothing is generated.
#[test]
fn a_function_specc_cannot_compile_is_refused() {
    let text =
        fs::read_to_string(manifest_dir().join("tests/fixtures/functions/spec.jsonc")).unwrap();
    let refused =
        |t: &str| match strata_spec_compiler::generate_core(FUNCTIONS_SPEC, t, FUNCTIONS_CRATE) {
            Ok(_) => Vec::new(),
            Err(d) => d.iter().map(ToString::to_string).collect::<Vec<_>>(),
        };
    assert_eq!(refused(&text), Vec::<String>::new());
    // A parameter of a type functions do not compute with.
    let float = text.replacen(
        "{\"name\":\"version\",\"type\":\"u64\"}",
        "{\"name\":\"version\",\"type\":\"f64\"}",
        1,
    );
    assert_ne!(float, text);
    assert!(
        refused(&float)
            .iter()
            .any(|d| d.contains("specc.function-unsupported") && d.contains("check-version")),
        "{:?}",
        refused(&float)
    );
    // A body that does not type.
    let mistyped = text.replacen("a == action", "a == resource", 1);
    assert_ne!(mistyped, text);
    assert!(
        refused(&mistyped)
            .iter()
            .any(|d| d.contains("specc.function-invalid") && d.contains("policy-applies")),
        "{:?}",
        refused(&mistyped)
    );
}

/// Split into two crates, the pure crate holds every declared type, uuid ids included (through the
/// uuid crate with its serde feature only, no v4), and the model crate depends on it and
/// re-exports them and its representation types.
#[test]
fn a_split_generation_puts_every_type_in_the_pure_crate() {
    let options = Options {
        crate_name: "fixture-model".to_string(),
        runtime: RuntimeDependency::Workspace,
    };
    let core = strata_spec_compiler::model_core::CoreDependency {
        crate_name: "fixture-model-core".to_string(),
        path: "../model-core".to_string(),
    };
    let (model, pure) =
        strata_spec_compiler::generate_with_core(CORE_SPEC, &core_text(), &options, &core).unwrap();
    let model_types = file_text(&model, "src/types.rs");
    let pure_types = file_text(&pure, "src/types.rs");
    assert!(model_types.contains("pub use fixture_model_core::types::*;"));
    assert!(!model_types.contains("pub struct AccountId"));
    assert!(!model_types.contains("pub struct Owner {"));
    assert!(!model_types.contains("pub enum Expr"));
    assert!(pure_types.contains("pub enum Expr"));
    assert!(pure_types.contains("pub struct AccountId(pub uuid::Uuid);"));
    assert!(pure_types.contains("pub struct Owner {"));
    assert!(file_text(&model, "src/prelude.rs").contains("pub use fixture_model_core::prelude::"));
    assert!(file_text(&model, "Cargo.toml")
        .contains("fixture-model-core = { path = \"../model-core\" }"));
    let pure_manifest = file_text(&pure, "Cargo.toml");
    for forbidden in ["strata-runtime", "tokio", "v4"] {
        assert!(
            !pure_manifest.contains(forbidden),
            "{forbidden} in {pure_manifest}"
        );
    }
    assert!(
        pure_manifest.contains("default-features = false, features = [\"serde\"]"),
        "uuid with the serde feature only: {pure_manifest}"
    );
    // The pure crate alone is the same pure crate.
    let alone =
        strata_spec_compiler::generate_core(CORE_SPEC, &core_text(), "fixture-model-core").unwrap();
    assert_eq!(alone, pure);
}

#[test]
fn only_a_specification_with_machines_gets_a_machines_module() {
    let full = fixture_files();
    assert!(!full.iter().any(|f| f.path == "src/machines.rs"));
    assert!(!full.iter().any(|f| f.path == "src/handlers.rs"));
    let manifest = |files: &[GeneratedFile]| {
        files
            .iter()
            .find(|f| f.path == "Cargo.toml")
            .unwrap()
            .contents
            .clone()
    };
    // The full fixture's components serve contracts, so it uses the runtime for its routes.
    assert!(manifest(&full).contains(
        "strata-runtime.workspace = true
"
    ));

    let machines = machines_files();
    let paths: Vec<&str> = machines.iter().map(|f| f.path.as_str()).collect();
    assert!(paths.contains(&"src/machines.rs"));
    assert!(paths.contains(&"src/handlers.rs"));
    // No component of the machines fixture implements a contract: no routes, no deps.
    assert!(!paths.contains(&"src/routes.rs"));
    assert!(!paths.contains(&"src/deps.rs"));
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
    // Ignored and unmatched events keep the state and variables, and are marked ignored so the
    // host writes nothing for them.
    assert!(code.contains("_ => Ok(Outcome::ignore(*state, vars.clone())),"));
}

#[test]
fn the_crate_has_the_expected_files_each_with_a_header() {
    let files = fixture_files();
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "Cargo.toml",
            "src/audits.rs",
            "src/contracts.rs",
            "src/deps.rs",
            "src/lib.rs",
            "src/policies.rs",
            "src/prelude.rs",
            "src/quotas.rs",
            "src/records.rs",
            "src/retentions.rs",
            "src/retries.rs",
            "src/routes.rs",
            "src/support.rs",
            "src/types.rs",
            "src/validations.rs"
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
    assert_eq!(wiring_files(), wiring_files());
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

fn file_text(files: &[GeneratedFile], path: &str) -> String {
    files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("no {path}"))
        .contents
        .clone()
}

#[test]
fn every_construct_reaches_the_generated_crate() {
    let files = fixture_files();
    let policies = file_text(&files, "src/policies.rs");
    assert!(policies.contains("pub fn authorize_create_tenant("));
    assert!(policies.contains("pub fn authorize_get_account("));
    assert!(policies.contains("record: Option<&crate::records::RecAccount>"));
    assert!(policies.contains("pub fn authorize_ping("));

    let validations = file_text(&files, "src/validations.rs");
    assert!(validations.contains("impl crate::contracts::CreateTenantRequest {"));
    assert!(validations.contains("pub fn validate_rules(&self)"));
    assert!(validations.contains("pub const VAL_SLUG_LENGTH: ValidationError"));
    assert!(validations.contains("static VAL_SLUG_PATTERN_PATTERN: LazyLock<Regex>"));

    let retentions = file_text(&files, "src/retentions.rs");
    assert!(retentions.contains("pub const RETENTIONS: &[Retention]"));
    assert!(retentions.contains("pub fn anonymise(&mut self)"));

    let quotas = file_text(&files, "src/quotas.rs");
    assert!(quotas.contains("pub const QUOTAS: &[Quota]"));
    assert!(quotas.contains("pub fn check(&self, counter: Option<Counter>, now: Timestamp)"));

    let retries = file_text(&files, "src/retries.rs");
    assert!(retries.contains("pub const RETRY_GET_ACCOUNT: RetryPolicy"));
    assert!(retries.contains("Target::Event(\"entry-posted\")"));

    let audits = file_text(&files, "src/audits.rs");
    assert!(audits.contains("pub fn audit_tenant_created("));
    assert!(audits.contains("pub fn chain_tenant_created("));
    assert!(audits.contains("pub fn verify_account_closing("));
    assert!(audits.contains("pub fn audit_account_read("));
    assert!(
        !audits.contains("chain_account_read"),
        "an append-only audit has no chain"
    );
    assert!(file_text(&files, "Cargo.toml").contains("sha2 = \"0.10.9\""));
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

#[test]
fn the_wiring_reaches_the_generated_crate() {
    let files = wiring_files();
    let routes = file_text(&files, "src/routes.rs");
    assert!(routes.contains("path: \"/api/jobs/submit\","));
    assert!(routes.contains("path: \"/scheduler/cancel-job\","));
    assert!(routes.contains("pub fn scheduler_routes<A, S, H>("));
    assert!(routes.contains("pub struct SchedulerClient<K: strata_runtime::Credentials = strata_runtime::NoCredentials>"));
    assert!(routes.contains("impl GatewayHooks for () {}"));
    assert!(routes.contains("fn append_job_submitted_audit("));

    let deps = file_text(&files, "src/deps.rs");
    assert!(deps.contains("pub fn scheduler(&self) -> &crate::routes::SchedulerClient<K>"));
    assert!(deps.contains("`worker`: implements no contract."));
    assert!(deps.contains("pub struct SchedulerDeps<K"));
    assert!(deps.contains("pub struct ReportsDeps;"));

    let handlers = file_text(&files, "src/handlers.rs");
    assert!(handlers.contains("pub async fn submit_job<S, D>("));
    assert!(handlers.contains("pub async fn deliver_from_job<S, D>("));
    assert!(handlers.contains("nothing is generated for them: `tracker-reset`."));
}

#[test]
fn a_machine_logs_the_emitted_events_the_specification_logs() {
    let files = machines_files();
    let telemetry = file_text(&files, "src/telemetry.rs");
    assert!(telemetry.contains("pub const WORKER: JsonLines = JsonLines { service: \"worker\" };"));
    assert!(telemetry.contains("fields.push((\"lease\", line.token.into()));"));
    let machines = file_text(&files, "src/machines.rs");
    // Only lease-granted is logged among what the lease machine emits.
    assert!(machines.contains("if let Emitted::LeaseGranted(payload) = emitted {"));
    assert!(machines.contains("holder: payload.holder.as_str(),"));
    assert_eq!(machines.matches("fn log_emitted(").count(), 1);
}
