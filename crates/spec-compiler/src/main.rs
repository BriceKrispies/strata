//! The `specc` command.
//!
//! ```text
//! specc generate     --spec <file> --out <dir> [--spec-hash <hash>] [--crate-name <name>] [--runtime <dir>] [--core-out <dir> [--core-crate-name <name>]]
//! specc check        --spec <file> --out <dir> [--spec-hash <hash>] [--crate-name <name>] [--runtime <dir>] [--core-out <dir> [--core-crate-name <name>]]
//! specc deploy       --spec <file> --out <dir> [--spec-hash <hash>]
//! specc check-deploy --spec <file> --out <dir> [--spec-hash <hash>]
//! specc e2e          --spec <file> --out <dir> [--spec-hash <hash>] [--origin <url>] [--expect-fail-prefix <id prefix>]
//! specc check-e2e    --spec <file> --out <dir> [--spec-hash <hash>] [--origin <url>] [--expect-fail-prefix <id prefix>]
//! specc scenarios       --spec <file> --out <dir> [--spec-hash <hash>] [--crate-name <name>] [--exegra-project <id>] [--exegra-specs <dir>]
//! specc check-scenarios --spec <file> --out <dir> [--spec-hash <hash>] [--crate-name <name>] [--exegra-project <id>] [--exegra-specs <dir>]
//! specc web          --spec <file> --out <dir> [--spec-hash <hash>]
//! specc check-web    --spec <file> --out <dir> [--spec-hash <hash>]
//! specc catalogue    --spec <file> --out <dir> [--spec-hash <hash>]
//! specc check-catalogue --spec <file> --out <dir> [--spec-hash <hash>]
//! specc conformance  --spec <file> --out <dir> [--spec-hash <hash>] [--expect-fail-prefix <id prefix>]
//! specc check-conformance --spec <file> --out <dir> [--spec-hash <hash>] [--expect-fail-prefix <id prefix>]
//! specc lint         --spec <file> [--gate <gate.json>]
//! specc bundle       --spec <file> --product <file> --out <dir>
//! ```
//!
//! Every command also takes `--product <file>`, any number of times: a product file layered on the
//! specification (`strata_spec_compiler::layers`). Each is loaded and checked against the
//! specification before anything else happens, and the command is refused with one `error[layer.*]`
//! line per problem. A product is tenant metadata, so nothing is generated from it: the files a
//! command writes or checks are exactly those it writes or checks without `--product`.
//!
//! `bundle` is the one command that is about a product: it takes exactly one `--product` and writes
//! `<product id>.bundle.json` into `<dir>`, the tenant bundle a tenant installs
//! (`strata_spec_compiler::bundle`). The product header carries the `version` and the `platform`
//! range of the bundle; a product without a version, a section a bundle has no place for and two
//! entities with one id are refused with `error[bundle.*]` lines.
//!
//! `--spec-hash <hash>` is the specification's semantic hash as `exegra validate` reports it
//! (`semanticHash`). It is written into the header of every generated file, so a file generated
//! from another version of the specification differs and `check` and `check-web` fail on it.
//!
//! `web` and `check-web` generate the machines of client components as JavaScript modules for
//! the browser (`strata_spec_compiler::web`), conventionally into `web/src/generated`; `generate`
//! leaves those machines out of the Rust crate. They also write, for each language the
//! specification declares, its JavaScript twin (`<language id>.js`, parser, printer, static check
//! and evaluator), so that a page evaluates a rule as the server does.
//!
//! `scenarios` writes `scenarios.rs` into `<dir>` (a generated crate's `tests` directory): one
//! test per scenario that runs only implementation machines, with the expectations Exegra
//! computes for it (see `strata_spec_compiler::scenarios`). It runs `node
//! $EXEGRA_HOME/cli/exegra.js trace`, and is refused (`specc.scenarios-no-exegra`) when
//! `EXEGRA_HOME` is not set. The crate name defaults to the name of `<dir>`'s parent; the Exegra
//! project to the name of the specification's directory and the specs directory to that
//! directory's parent.
//!
//! `e2e` and `check-e2e` write the Playwright journeys (`strata_spec_compiler::e2e`). `--origin`
//! is the origin the journeys' absolute addresses were written against, which the tests move to
//! `STRATA_E2E_BASE_URL` when that is set; journeys whose id starts with `--expect-fail-prefix`
//! are generated as tests that must fail (the fixtures that show each assertion can fail).
//!
//! `catalogue` and `check-catalogue` write `catalogue.md` (`strata_spec_compiler::catalogue`), the
//! compact capability catalogue of the platform, conventionally into `specifications`; the header
//! carries the semantic hash like every other generated file.
//!
//! `conformance` and `check-conformance` write `contracts.rs`
//! (`strata_spec_compiler::conformance`), the contract conformance suite: one Rust test per contract
//! that is shipped and names a `bffRoute`, run by `crates/bff/tests/contract_conformance.rs` against
//! the real BFF and backend, conventionally into `crates/bff/tests/conformance/generated`. Contracts
//! whose id starts with `--expect-fail-prefix` are generated as tests that must fail (the fixtures
//! of `crates/bff/tests/conformance/fixtures`). The command reads only the contracts, types, roles,
//! quotas and screens and refuses what it cannot build a request or a check from; it does not run
//! the readiness check of the Rust generation, so that its small fixture specification needs none of
//! what the backends ask for. The platform specification passes that check in `specc check` of the
//! same gate step.
//!
//! `lint` is the requirement wording lint (`strata_spec_compiler::lint`): `specc lint --spec <file>`
//! prints one `error[lint.<rule>] <entity>: <what to change> (<file>:<line>)` line per finding and
//! exits 1 when there is any finding. It reads `startedMilestones` from the `gate.json` beside the
//! specification or above it (or `--gate`), for `lint.no-check`.
//!
//! `deploy` and `check-deploy` do for the deployment files (a compose stack, Kubernetes
//! manifests and Prometheus alert rules; see `strata_spec_compiler::deploy`) what `generate`
//! and `check` do for the crate; their warnings go to standard error and do not change the exit
//! code. `generate` writes the crate into `<dir>` and removes generated files it no longer writes.
//! `check` exits 1 and lists the paths that differ when `<dir>` is not what `generate` would
//! leave. A specification that is not ready is refused with one `error[<code>] <entity>:
//! <message>` line per diagnostic and exit code 1. Usage errors exit with 2. The crate name
//! defaults to the last component of `<dir>`.
//!
//! `--core-out` splits the generation in two crates (`strata_spec_compiler::model_core`): the pure
//! crate, written into the `--core-out` directory and named `--core-crate-name` (by default that
//! directory's last component), holds the representation types and every declared type (uuid ids
//! through the uuid crate without its v4 feature); the crate in `--out` depends on it by relative
//! path and re-exports them. The examples of the specification's functions are integration test
//! files of the pure crate (`tests/function_examples_<n>.rs`), not a test module of its library.
//! `check` then compares both directories.
//!
//! A crate with implementation-level machines depends on `strata-runtime` by path: `--runtime`
//! names the runtime crate's directory, by default `crates/runtime` of the Strata checkout specc
//! was built from, and the manifest holds its path relative to `<dir>`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use strata_spec_compiler::emit::relative_path;
use strata_spec_compiler::layers::{self, ProductFile};
use strata_spec_compiler::{
    bundle, catalogue, conformance, deploy, e2e, generate, generate_with_core, lint, model_core,
    output, scenarios, web, GeneratedFile, Options, RuntimeDependency,
};

const USAGE: &str = "usage:
  specc generate     --spec <file> --out <dir> [--spec-hash <hash>] [--crate-name <name>] [--runtime <dir>] [--core-out <dir> [--core-crate-name <name>]]
  specc check        --spec <file> --out <dir> [--spec-hash <hash>] [--crate-name <name>] [--runtime <dir>] [--core-out <dir> [--core-crate-name <name>]]
  specc deploy       --spec <file> --out <dir> [--spec-hash <hash>] [--profile <file>]
  specc check-deploy --spec <file> --out <dir> [--spec-hash <hash>] [--profile <file>]
  specc e2e          --spec <file> --out <dir> [--spec-hash <hash>] [--origin <url>] [--expect-fail-prefix <id prefix>]
  specc check-e2e    --spec <file> --out <dir> [--spec-hash <hash>] [--origin <url>] [--expect-fail-prefix <id prefix>]
  specc scenarios       --spec <file> --out <dir> [--spec-hash <hash>] [--crate-name <name>] [--exegra-project <id>] [--exegra-specs <dir>]
  specc check-scenarios --spec <file> --out <dir> [--spec-hash <hash>] [--crate-name <name>] [--exegra-project <id>] [--exegra-specs <dir>]
  specc web          --spec <file> --out <dir> [--spec-hash <hash>]
  specc check-web    --spec <file> --out <dir> [--spec-hash <hash>]
  specc catalogue    --spec <file> --out <dir> [--spec-hash <hash>]
  specc check-catalogue --spec <file> --out <dir> [--spec-hash <hash>]
  specc conformance  --spec <file> --out <dir> [--spec-hash <hash>] [--expect-fail-prefix <id prefix>]
  specc check-conformance --spec <file> --out <dir> [--spec-hash <hash>] [--expect-fail-prefix <id prefix>]
  specc bundle       --spec <file> --product <file> --out <dir>
  specc lint         --spec <file> [--gate <gate.json>]
every command also takes --product <file>, repeatable: a product file layered on the specification (bundle takes exactly one)";

/// The runtime crate of the checkout specc was built from.
const DEFAULT_RUNTIME: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../runtime");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Generate,
    Check,
    Deploy,
    CheckDeploy,
    E2e,
    CheckE2e,
    Scenarios,
    CheckScenarios,
    Web,
    CheckWeb,
    Catalogue,
    CheckCatalogue,
    Conformance,
    CheckConformance,
    Bundle,
}

struct Args {
    command: Command,
    spec: String,
    out: PathBuf,
    crate_name: String,
    runtime: PathBuf,
    /// The specification's semantic hash, written into every header when given.
    spec_hash: Option<String>,
    exegra_project: Option<String>,
    exegra_specs: Option<PathBuf>,
    e2e: e2e::Options,
    conformance: conformance::Options,
    /// The pure crate's directory and package name, when the generation is split.
    core: Option<(PathBuf, String)>,
    /// The product files layered on the specification (`--product`, in order).
    products: Vec<String>,
    profile: Option<PathBuf>,
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let (command, rest) = match raw.split_first() {
        Some((c, rest)) if c == "generate" => (Command::Generate, rest),
        Some((c, rest)) if c == "check" => (Command::Check, rest),
        Some((c, rest)) if c == "deploy" => (Command::Deploy, rest),
        Some((c, rest)) if c == "check-deploy" => (Command::CheckDeploy, rest),
        Some((c, rest)) if c == "e2e" => (Command::E2e, rest),
        Some((c, rest)) if c == "check-e2e" => (Command::CheckE2e, rest),
        Some((c, rest)) if c == "scenarios" => (Command::Scenarios, rest),
        Some((c, rest)) if c == "check-scenarios" => (Command::CheckScenarios, rest),
        Some((c, rest)) if c == "web" => (Command::Web, rest),
        Some((c, rest)) if c == "check-web" => (Command::CheckWeb, rest),
        Some((c, rest)) if c == "catalogue" => (Command::Catalogue, rest),
        Some((c, rest)) if c == "check-catalogue" => (Command::CheckCatalogue, rest),
        Some((c, rest)) if c == "conformance" => (Command::Conformance, rest),
        Some((c, rest)) if c == "check-conformance" => (Command::CheckConformance, rest),
        Some((c, rest)) if c == "bundle" => (Command::Bundle, rest),
        Some((c, _)) => return Err(format!("unknown command \"{c}\"")),
        None => return Err("no command given".to_string()),
    };
    let mut spec = None;
    let mut out = None;
    let mut crate_name = None;
    let mut runtime = None;
    let mut spec_hash = None;
    let mut exegra_project = None;
    let mut exegra_specs = None;
    let mut origin = None;
    let mut expect_fail_prefix = None;
    let mut core_out = None;
    let mut core_crate_name = None;
    let mut products = Vec::new();
    let mut profile = None;
    let mut iter = rest.iter();
    while let Some(flag) = iter.next() {
        if flag == "--product" {
            let Some(value) = iter.next() else {
                return Err("--product needs a value".to_string());
            };
            products.push(value.clone());
            continue;
        }
        let slot = match flag.as_str() {
            "--spec" => &mut spec,
            "--out" => &mut out,
            "--crate-name" => &mut crate_name,
            "--runtime" => &mut runtime,
            "--spec-hash" => &mut spec_hash,
            "--exegra-project" => &mut exegra_project,
            "--exegra-specs" => &mut exegra_specs,
            "--origin" => &mut origin,
            "--expect-fail-prefix" => &mut expect_fail_prefix,
            "--core-out" => &mut core_out,
            "--core-crate-name" => &mut core_crate_name,
            "--profile" => &mut profile,
            other => return Err(format!("unknown argument \"{other}\"")),
        };
        let Some(value) = iter.next() else {
            return Err(format!("{flag} needs a value"));
        };
        if slot.replace(value.clone()).is_some() {
            return Err(format!("{flag} is given twice"));
        }
    }
    if let Some(hash) = &spec_hash {
        if hash.is_empty() || !hash.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(
                "--spec-hash must be letters and digits (Exegra's semanticHash)".to_string(),
            );
        }
    }
    let spec = spec.ok_or("--spec is required")?;
    let out = PathBuf::from(out.ok_or("--out is required")?);
    let deploys = matches!(
        command,
        Command::Deploy
            | Command::CheckDeploy
            | Command::E2e
            | Command::CheckE2e
            | Command::Web
            | Command::CheckWeb
            | Command::Catalogue
            | Command::CheckCatalogue
            | Command::Conformance
            | Command::CheckConformance
            | Command::Bundle
    );
    if deploys && crate_name.is_some() {
        return Err(
            "--crate-name does not apply to deploy, e2e, web, catalogue, conformance or bundle"
                .to_string(),
        );
    }
    let end_to_end = matches!(command, Command::E2e | Command::CheckE2e);
    let conforming = matches!(command, Command::Conformance | Command::CheckConformance);
    if !end_to_end && origin.is_some() {
        return Err("--origin applies only to e2e".to_string());
    }
    if !end_to_end && !conforming && expect_fail_prefix.is_some() {
        return Err("--expect-fail-prefix applies only to e2e and conformance".to_string());
    }
    if profile.is_some() && !matches!(command, Command::Deploy | Command::CheckDeploy) {
        return Err("--profile applies only to deploy and check-deploy".to_string());
    }
    let scenario = matches!(command, Command::Scenarios | Command::CheckScenarios);
    if !scenario && (exegra_project.is_some() || exegra_specs.is_some()) {
        return Err("--exegra-project and --exegra-specs apply only to scenarios".to_string());
    }
    let crate_command = matches!(command, Command::Generate | Command::Check);
    if !crate_command && (core_out.is_some() || core_crate_name.is_some()) {
        return Err(
            "--core-out and --core-crate-name apply only to generate and check".to_string(),
        );
    }
    if core_out.is_none() && core_crate_name.is_some() {
        return Err("--core-crate-name needs --core-out".to_string());
    }
    let core = match core_out {
        None => None,
        Some(dir) => {
            let dir = PathBuf::from(dir);
            let name = match core_crate_name {
                Some(name) => name,
                None => dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .ok_or(
                    "--core-out has no last component to name the crate; give --core-crate-name",
                )?,
            };
            Some((dir, name))
        }
    };
    let crate_name = match crate_name {
        _ if deploys => String::new(),
        Some(name) => name,
        None if scenario => absolute(&out)?
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or("--out has no parent to name the crate; give --crate-name")?,
        None => out
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or("--out has no last component to name the crate; give --crate-name")?,
    };
    Ok(Args {
        command,
        spec,
        out,
        crate_name,
        runtime: PathBuf::from(runtime.unwrap_or_else(|| DEFAULT_RUNTIME.to_string())),
        spec_hash,
        exegra_project,
        exegra_specs: exegra_specs.map(PathBuf::from),
        conformance: conformance::Options {
            expect_fail_prefix: if conforming {
                expect_fail_prefix.clone()
            } else {
                None
            },
        },
        e2e: e2e::Options {
            origin,
            expect_fail_prefix,
        },
        core,
        products,
        profile: profile.map(PathBuf::from),
    })
}

impl Args {
    /// What the headers of generated files name as their source: the specification's path, and
    /// its semantic hash when `--spec-hash` gave one.
    fn header_source(&self) -> String {
        match &self.spec_hash {
            Some(hash) => format!("{} (semantic hash {hash})", self.spec),
            None => self.spec.clone(),
        }
    }
}

/// specc walks specifications recursively (types, expressions, functions); a large specification
/// nests deeper than the main thread's default stack allows in a debug build, so the work runs on a
/// thread with a stack of its own.
const STACK_BYTES: usize = 256 * 1024 * 1024;

fn main() -> ExitCode {
    std::thread::Builder::new()
        .name("specc".into())
        .stack_size(STACK_BYTES)
        .spawn(run)
        .expect("specc: cannot start its worker thread")
        .join()
        .unwrap_or(ExitCode::FAILURE)
}

/// The shared readiness check, run before any command emits from the raw specification: the
/// refusal (exit code 1) when the Rust backends would refuse it, `None` when it is ready.
fn refuse_unready(args: &Args, text: &str) -> Option<ExitCode> {
    let Err(diagnostics) = strata_spec_compiler::check_ready(&args.header_source(), text) else {
        return None;
    };
    for d in &diagnostics {
        eprintln!("{d}");
    }
    eprintln!(
        "specc: refused {} with {} diagnostic(s); nothing was written",
        args.spec,
        diagnostics.len()
    );
    Some(ExitCode::from(1))
}

fn run() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.first().map(String::as_str) == Some("lint") {
        return run_lint(&raw[1..]);
    }
    let args = match parse_args(&raw) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("specc: {message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let text = match std::fs::read_to_string(&args.spec) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("specc: cannot read {}: {e}", args.spec);
            return ExitCode::from(1);
        }
    };
    if args.command == Command::Bundle {
        // The bundle checks its product itself: the header carries the version of the bundle.
        if let Some(code) = refuse_unready(&args, &text) {
            return code;
        }
        return run_bundle(&args, &text);
    }
    if let Err(code) = check_products(&args, &text) {
        return code;
    }
    if matches!(
        args.command,
        Command::Conformance | Command::CheckConformance
    ) {
        if let Some(code) = refuse_unready(&args, &text) {
            return code;
        }
        return run_conformance(&args, &text);
    }
    let raw_json_command = matches!(
        args.command,
        Command::Deploy
            | Command::CheckDeploy
            | Command::E2e
            | Command::CheckE2e
            | Command::Web
            | Command::CheckWeb
            | Command::Catalogue
            | Command::CheckCatalogue
            | Command::Scenarios
            | Command::CheckScenarios
    );
    if raw_json_command {
        // These commands read the specification's JSON directly; they run the readiness check
        // the Rust generation runs, so none of them emits from a specification it would refuse.
        if let Some(code) = refuse_unready(&args, &text) {
            return code;
        }
    }
    if matches!(args.command, Command::Deploy | Command::CheckDeploy) {
        return run_deploy(&args, &text);
    }
    if matches!(args.command, Command::E2e | Command::CheckE2e) {
        return run_e2e(&args, &text);
    }
    if matches!(args.command, Command::Web | Command::CheckWeb) {
        return run_web(&args, &text);
    }
    if matches!(args.command, Command::Catalogue | Command::CheckCatalogue) {
        return run_catalogue(&args, &text);
    }
    if matches!(args.command, Command::Scenarios | Command::CheckScenarios) {
        return run_scenarios(&args, &text);
    }
    let runtime = match runtime_path(&args.out, &args.runtime) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("specc: {message}");
            return ExitCode::from(1);
        }
    };
    let options = Options {
        crate_name: args.crate_name.clone(),
        runtime: RuntimeDependency::Path(runtime),
    };
    if let Some((core_out, core_name)) = &args.core {
        return run_split(&args, &text, &options, core_out, core_name);
    }
    let files = match generate(&args.header_source(), &text, &options) {
        Ok(files) => files,
        Err(diagnostics) => {
            for d in &diagnostics {
                eprintln!("{d}");
            }
            eprintln!(
                "specc: refused {} with {} diagnostic(s); nothing was written",
                args.spec,
                diagnostics.len()
            );
            return ExitCode::from(1);
        }
    };
    match args.command {
        Command::Generate => run_generate(&args.out, &files),
        Command::Check => run_check(&args.out, &files),
        // Handled before generation.
        Command::Deploy
        | Command::CheckDeploy
        | Command::E2e
        | Command::CheckE2e
        | Command::Scenarios
        | Command::CheckScenarios
        | Command::Web
        | Command::CheckWeb
        | Command::Catalogue
        | Command::CheckCatalogue
        | Command::Conformance
        | Command::CheckConformance
        | Command::Bundle => ExitCode::from(2),
    }
}

/// `lint`: the requirement wording lint (`strata_spec_compiler::lint`). Findings go to standard
/// output, one line each; the summary goes to standard error. Exit code 1 when there is any finding.
fn run_lint(rest: &[String]) -> ExitCode {
    let mut spec = None;
    let mut gate = None;
    let mut iter = rest.iter();
    while let Some(flag) = iter.next() {
        let slot = match flag.as_str() {
            "--spec" => &mut spec,
            "--gate" => &mut gate,
            other => {
                eprintln!(
                    "specc: unknown argument \"{other}\"
{USAGE}"
                );
                return ExitCode::from(2);
            }
        };
        let Some(value) = iter.next() else {
            eprintln!(
                "specc: {flag} needs a value
{USAGE}"
            );
            return ExitCode::from(2);
        };
        if slot.replace(value.clone()).is_some() {
            eprintln!(
                "specc: {flag} is given twice
{USAGE}"
            );
            return ExitCode::from(2);
        }
    }
    let Some(spec) = spec else {
        eprintln!(
            "specc: --spec is required
{USAGE}"
        );
        return ExitCode::from(2);
    };
    let text = match std::fs::read_to_string(&spec) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("specc: cannot read {spec}: {e}");
            return ExitCode::from(1);
        }
    };
    let started = match started_milestones(&spec, gate.as_deref()) {
        Ok(started) => started,
        Err(message) => {
            eprintln!("specc: {message}");
            return ExitCode::from(1);
        }
    };
    let findings = match lint::lint_started(&spec, &text, &started) {
        Ok(findings) => findings,
        Err(diagnostics) => {
            for d in &diagnostics {
                eprintln!("{d}");
            }
            return ExitCode::from(1);
        }
    };
    for finding in &findings {
        println!("{finding}");
    }
    eprintln!("specc: lint {spec}: {} finding(s)", findings.len());
    if findings.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// The `startedMilestones` of the gate configuration: the file `--gate` names, else the
/// `gate.json` beside the specification or in the directory above it. None found: no milestone is
/// started, and `lint.no-check` applies to nothing (said on standard error).
fn started_milestones(
    spec: &str,
    gate: Option<&str>,
) -> Result<std::collections::BTreeSet<u64>, String> {
    let beside = std::path::Path::new(spec)
        .parent()
        .map(std::path::Path::to_path_buf);
    let found = match gate {
        Some(path) => Some(std::path::PathBuf::from(path)),
        None => beside
            .iter()
            .flat_map(|dir| [dir.join("gate.json"), dir.join("../gate.json")])
            .find(|p| p.is_file()),
    };
    let Some(path) = found else {
        eprintln!("specc: no gate.json found beside {spec}; lint.no-check is not applied (--gate <file> names one)");
        return Ok(std::collections::BTreeSet::new());
    };
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let top = strata_spec_compiler::jsonc::parse(&text)
        .map_err(|e| format!("{} is not valid JSON: {e}", path.display()))?;
    let list = top
        .get("startedMilestones")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            format!(
                "{}: startedMilestones must be a list of milestone numbers",
                path.display()
            )
        })?;
    list.iter()
        .map(|m| {
            m.as_u64().ok_or_else(|| {
                format!(
                    "{}: startedMilestones holds {m}, not a milestone number",
                    path.display()
                )
            })
        })
        .collect()
}

/// `bundle`: the product file as a tenant bundle, `<product id>.bundle.json` in `--out`.
fn run_bundle(args: &Args, text: &str) -> ExitCode {
    let [path] = args.products.as_slice() else {
        eprintln!("specc: bundle takes exactly one --product <file>\n{USAGE}");
        return ExitCode::from(2);
    };
    let product = match std::fs::read_to_string(path) {
        Ok(product) => ProductFile {
            path: path.clone(),
            text: product,
            file: absolute(Path::new(path)).ok(),
        },
        Err(e) => {
            eprintln!("specc: cannot read the product file {path}: {e}");
            return ExitCode::from(1);
        }
    };
    let platform = absolute(Path::new(&args.spec)).ok();
    match bundle::generate(&args.spec, text, platform.as_deref(), &product) {
        Ok(output) => {
            for (section, count) in &output.omitted {
                eprintln!(
                    "specc: bundle {}: {count} item(s) of \"{section}\" state facts about the product, not tenant metadata, and are not in the bundle",
                    output.product
                );
            }
            run_generate(&args.out, &output.files)
        }
        Err(diagnostics) => {
            for d in &diagnostics {
                eprintln!("{d}");
            }
            eprintln!(
                "specc: refused {path} with {} diagnostic(s); nothing was written",
                diagnostics.len()
            );
            ExitCode::from(1)
        }
    }
}

/// Loads the product files of `--product` and checks them against the specification. Nothing is
/// generated from them, so the command goes on with the specification alone; a product that does
/// not pass refuses the whole command before anything is written.
fn check_products(args: &Args, text: &str) -> Result<(), ExitCode> {
    if args.products.is_empty() {
        return Ok(());
    }
    let mut files = Vec::new();
    for path in &args.products {
        match std::fs::read_to_string(path) {
            Ok(product) => files.push(ProductFile {
                path: path.clone(),
                text: product,
                file: absolute(Path::new(path)).ok(),
            }),
            Err(e) => {
                eprintln!("specc: cannot read the product file {path}: {e}");
                return Err(ExitCode::from(1));
            }
        }
    }
    let platform = absolute(Path::new(&args.spec)).ok();
    match layers::load_layered(&args.spec, text, platform.as_deref(), &files) {
        Ok(layered) => {
            for product in &layered.products {
                eprintln!(
                    "specc: product {} ({}) checked against {}: {} entities of tenant metadata, from which nothing is generated",
                    product.id,
                    product.path,
                    args.spec,
                    product.entities.len()
                );
            }
            Ok(())
        }
        Err(diagnostics) => {
            for d in &diagnostics {
                eprintln!("{d}");
            }
            eprintln!(
                "specc: refused {} with its product files, {} diagnostic(s); nothing was written",
                args.spec,
                diagnostics.len()
            );
            Err(ExitCode::from(1))
        }
    }
}

/// `generate` or `check` of a generation split into the model crate and the pure crate.
fn run_split(
    args: &Args,
    text: &str,
    options: &Options,
    core_out: &Path,
    core_name: &str,
) -> ExitCode {
    let path = match absolute(&args.out).and_then(|out| {
        let core = absolute(core_out)?;
        relative_path(&out, &core).ok_or_else(|| {
            format!(
                "{} and {} share no root, so the pure crate cannot be named by a relative path",
                out.display(),
                core.display()
            )
        })
    }) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("specc: {message}");
            return ExitCode::from(1);
        }
    };
    let core = model_core::CoreDependency {
        crate_name: core_name.to_string(),
        path,
    };
    let (model, pure) = match generate_with_core(&args.header_source(), text, options, &core) {
        Ok(files) => files,
        Err(diagnostics) => {
            for d in &diagnostics {
                eprintln!("{d}");
            }
            eprintln!(
                "specc: refused {} with {} diagnostic(s); nothing was written",
                args.spec,
                diagnostics.len()
            );
            return ExitCode::from(1);
        }
    };
    let run = |out: &Path, files: &[GeneratedFile]| match args.command {
        Command::Check => run_check(out, files),
        _ => run_generate(out, files),
    };
    let pure_result = run(core_out, &pure);
    let model_result = run(&args.out, &model);
    let failed: Vec<String> = [(core_out, pure_result), (args.out.as_path(), model_result)]
        .iter()
        .filter(|(_, result)| *result != ExitCode::SUCCESS)
        .map(|(dir, _)| dir.display().to_string())
        .collect();
    if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        // The last line names every crate that failed, for callers that read only that line.
        println!("specc: {} failed", failed.join(" and "));
        ExitCode::from(1)
    }
}

fn run_deploy(args: &Args, text: &str) -> ExitCode {
    // The profile is the file named by --profile, else `deploy.jsonc` next to the specification.
    let profile_path = args.profile.clone().or_else(|| {
        let next_to_spec = Path::new(&args.spec).with_file_name("deploy.jsonc");
        next_to_spec.is_file().then_some(next_to_spec)
    });
    let profile = match &profile_path {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(text) => Some(text),
            Err(e) => {
                eprintln!("specc: cannot read {}: {e}", path.display());
                return ExitCode::from(1);
            }
        },
        None => None,
    };
    let output =
        match deploy::generate_with_profile(&args.header_source(), text, profile.as_deref()) {
            Ok(output) => output,
            Err(diagnostics) => {
                for d in &diagnostics {
                    eprintln!("{d}");
                }
                eprintln!(
                    "specc: refused {} with {} diagnostic(s); nothing was written",
                    args.spec,
                    diagnostics.len()
                );
                return ExitCode::from(1);
            }
        };
    for warning in &output.warnings {
        eprintln!("{warning}");
    }
    match args.command {
        Command::CheckDeploy => run_check(&args.out, &output.files),
        _ => run_generate(&args.out, &output.files),
    }
}

fn run_web(args: &Args, text: &str) -> ExitCode {
    let files = match web::generate(&args.header_source(), text) {
        Ok(files) => files,
        Err(diagnostics) => {
            for d in &diagnostics {
                eprintln!("{d}");
            }
            eprintln!(
                "specc: refused {} with {} diagnostic(s); nothing was written",
                args.spec,
                diagnostics.len()
            );
            return ExitCode::from(1);
        }
    };
    match args.command {
        Command::CheckWeb => run_check(&args.out, &files),
        _ => run_generate(&args.out, &files),
    }
}

fn run_catalogue(args: &Args, text: &str) -> ExitCode {
    let output = match catalogue::generate(&args.header_source(), text) {
        Ok(output) => output,
        Err(diagnostics) => {
            for d in &diagnostics {
                eprintln!("{d}");
            }
            eprintln!(
                "specc: refused {} with {} diagnostic(s); nothing was written",
                args.spec,
                diagnostics.len()
            );
            return ExitCode::from(1);
        }
    };
    match args.command {
        Command::CheckCatalogue => run_check(&args.out, &output.files),
        _ => run_generate(&args.out, &output.files),
    }
}

/// `conformance` or `check-conformance`: the contract conformance suite, and on standard error what
/// it covers and the checks it cannot make, counted by check.
fn run_conformance(args: &Args, text: &str) -> ExitCode {
    let output = match conformance::generate(&args.header_source(), text, &args.conformance) {
        Ok(output) => output,
        Err(diagnostics) => {
            for d in &diagnostics {
                eprintln!("{d}");
            }
            eprintln!(
                "specc: refused {} with {} diagnostic(s); nothing was written",
                args.spec,
                diagnostics.len()
            );
            return ExitCode::from(1);
        }
    };
    let mut by_check: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for skip in &output.skipped {
        *by_check.entry(skip.check.as_str()).or_default() += 1;
    }
    eprintln!(
        "specc: conformance covers {} contract(s); {} check(s) not made{}",
        output.covered.len(),
        output.skipped.len(),
        if by_check.is_empty() {
            String::new()
        } else {
            format!(
                " ({})",
                by_check
                    .iter()
                    .map(|(check, n)| format!("{check} {n}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    );
    match args.command {
        Command::CheckConformance => run_check(&args.out, &output.files),
        _ => run_generate(&args.out, &output.files),
    }
}

fn run_e2e(args: &Args, text: &str) -> ExitCode {
    let output = match e2e::generate_with(&args.header_source(), text, &args.e2e) {
        Ok(output) => output,
        Err(diagnostics) => {
            for d in &diagnostics {
                eprintln!("{d}");
            }
            eprintln!(
                "specc: refused {} with {} diagnostic(s); nothing was written",
                args.spec,
                diagnostics.len()
            );
            return ExitCode::from(1);
        }
    };
    match args.command {
        Command::CheckE2e => run_check(&args.out, &output.files),
        _ => run_generate(&args.out, &output.files),
    }
}

fn run_scenarios(args: &Args, text: &str) -> ExitCode {
    let spec = absolute(Path::new(&args.spec)).unwrap_or_else(|_| PathBuf::from(&args.spec));
    let project = args.exegra_project.clone().or_else(|| {
        spec.parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
    });
    let specs = args
        .exegra_specs
        .clone()
        .or_else(|| spec.parent().and_then(Path::parent).map(Path::to_path_buf));
    let home = std::env::var_os("EXEGRA_HOME")
        .map(PathBuf::from)
        .filter(|h| h.join("cli").join("exegra.js").is_file());
    let source = match (home, project, specs) {
        (Some(home), Some(project), Some(specs)) => {
            Some(scenarios::exegra_cli(home, project, specs))
        }
        _ => None,
    };
    let result = scenarios::generate(
        &args.header_source(),
        text,
        &args.crate_name,
        source.as_ref().map(|s| s as scenarios::TraceSource),
    );
    let output = match result {
        Ok(output) => output,
        Err(diagnostics) => {
            for d in &diagnostics {
                eprintln!("{d}");
            }
            eprintln!(
                "specc: refused {} with {} diagnostic(s); nothing was written",
                args.spec,
                diagnostics.len()
            );
            return ExitCode::from(1);
        }
    };
    match args.command {
        Command::CheckScenarios => run_check(&args.out, &output.files),
        _ => run_generate(&args.out, &output.files),
    }
}

/// The runtime directory relative to the output directory, both made absolute and normalised
/// (the output directory need not exist yet).
fn runtime_path(out: &Path, runtime: &Path) -> Result<String, String> {
    let runtime = std::fs::canonicalize(runtime)
        .map_err(|e| format!("cannot find the runtime crate {}: {e}", runtime.display()))?;
    let out = absolute(out)?;
    // canonicalize gives verbatim paths on Windows (\\?\C:\...); compare like with like.
    let runtime = absolute(&strip_verbatim(&runtime))?;
    relative_path(&out, &runtime).ok_or_else(|| {
        format!(
            "{} and {} share no root, so the runtime cannot be named by a relative path; give --runtime on the same drive",
            out.display(),
            runtime.display()
        )
    })
}

/// `path` made absolute against the current directory, with `.` and `..` resolved by name.
fn absolute(path: &Path) -> Result<PathBuf, String> {
    let joined = std::env::current_dir()
        .map_err(|e| format!("cannot read the current directory: {e}"))?
        .join(path);
    let mut out = PathBuf::new();
    for part in joined.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

/// A Windows verbatim path (`\\?\C:\x`) as an ordinary one (`C:\x`); other paths unchanged.
fn strip_verbatim(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

/// Prints the path collisions among `files` and returns true when there are any: no command
/// writes or compares files that share a path.
fn refuse_path_collisions(files: &[GeneratedFile]) -> bool {
    let collisions = output::path_collisions(files);
    for d in &collisions {
        eprintln!("{d}");
    }
    if !collisions.is_empty() {
        eprintln!(
            "specc: refused with {} path diagnostic(s); nothing was written",
            collisions.len()
        );
    }
    !collisions.is_empty()
}

fn run_generate(out: &Path, files: &[GeneratedFile]) -> ExitCode {
    if refuse_path_collisions(files) {
        return ExitCode::from(1);
    }
    match output::write(out, files) {
        Ok(removed) => {
            for path in &removed {
                println!("removed {path}");
            }
            println!("wrote {} files to {}", files.len(), out.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("specc: cannot write {}: {e}", out.display());
            ExitCode::from(1)
        }
    }
}

fn run_check(out: &Path, files: &[GeneratedFile]) -> ExitCode {
    if refuse_path_collisions(files) {
        return ExitCode::from(1);
    }
    match output::compare(out, files) {
        Ok(differences) if differences.is_empty() => {
            println!("{} is up to date", out.display());
            ExitCode::SUCCESS
        }
        Ok(differences) => {
            for path in &differences.differing {
                println!("differs: {path}");
            }
            for path in &differences.missing {
                println!("missing: {path}");
            }
            for path in &differences.extra {
                println!("extra: {path}");
            }
            println!(
                "{} is not what specc generate would write; run specc generate",
                out.display()
            );
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("specc: cannot read {}: {e}", out.display());
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// Every command writes or compares through `run_generate` and `run_check`, so files that share
    /// a path are refused for every command, and nothing is written.
    #[test]
    fn files_that_share_a_path_are_refused_by_every_command_that_writes_or_checks() {
        let files: Vec<GeneratedFile> = ["forms.js", "FORMS.js"]
            .iter()
            .map(|path| GeneratedFile {
                path: (*path).to_string(),
                contents: String::new(),
            })
            .collect();
        let out = std::env::temp_dir().join(format!("specc-collision-{}", std::process::id()));
        assert!(refuse_path_collisions(&files));
        assert_eq!(
            format!("{:?}", run_generate(&out, &files)),
            format!("{:?}", ExitCode::from(1))
        );
        assert_eq!(
            format!("{:?}", run_check(&out, &files)),
            format!("{:?}", ExitCode::from(1))
        );
        assert!(!out.exists(), "nothing was written");
    }

    #[test]
    fn crate_name_defaults_to_the_out_directory() {
        let parsed = parse_args(&args(&[
            "generate",
            "--spec",
            "s.jsonc",
            "--out",
            "gen/my-crate",
        ]))
        .unwrap();
        assert_eq!(parsed.command, Command::Generate);
        assert_eq!(parsed.crate_name, "my-crate");
        assert_eq!(parsed.runtime, PathBuf::from(DEFAULT_RUNTIME));
        assert_eq!(parsed.core, None);
    }

    #[test]
    fn the_core_crate_is_named_after_its_directory() {
        let parsed = parse_args(&args(&[
            "generate",
            "--spec",
            "s.jsonc",
            "--out",
            "crates/model",
            "--core-out",
            "crates/model-core",
        ]))
        .unwrap();
        assert_eq!(
            parsed.core,
            Some((PathBuf::from("crates/model-core"), "model-core".to_string()))
        );
        assert!(parse_args(&args(&[
            "web",
            "--spec",
            "s",
            "--out",
            "o",
            "--core-out",
            "c"
        ]))
        .is_err());
        assert!(parse_args(&args(&[
            "generate",
            "--spec",
            "s",
            "--out",
            "o",
            "--core-crate-name",
            "c"
        ]))
        .is_err());
    }

    #[test]
    fn the_runtime_is_named_relative_to_the_output() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let out = workspace.join("target/specc-out/generated");
        let path = runtime_path(&out, Path::new(DEFAULT_RUNTIME)).unwrap();
        assert_eq!(path, "../../../crates/runtime");
    }

    #[test]
    fn product_files_are_collected_in_order() {
        let parsed = parse_args(&args(&[
            "web",
            "--spec",
            "s.jsonc",
            "--product",
            "a.jsonc",
            "--out",
            "o",
            "--product",
            "b.jsonc",
        ]))
        .unwrap();
        assert_eq!(parsed.products, vec!["a.jsonc", "b.jsonc"]);
        assert!(parse_args(&args(&["web", "--spec", "s", "--out", "o", "--product"])).is_err());
        assert!(parse_args(&args(&["web", "--spec", "s", "--out", "o"]))
            .unwrap()
            .products
            .is_empty());
    }

    #[test]
    fn bundle_names_a_product_and_takes_no_crate_name() {
        let parsed = parse_args(&args(&[
            "bundle",
            "--spec",
            "s.jsonc",
            "--product",
            "p.jsonc",
            "--out",
            "o",
        ]))
        .unwrap();
        assert_eq!(parsed.command, Command::Bundle);
        assert_eq!(parsed.products, vec!["p.jsonc"]);
        assert!(parse_args(&args(&[
            "bundle",
            "--spec",
            "s",
            "--out",
            "o",
            "--crate-name",
            "c"
        ]))
        .is_err());
    }

    #[test]
    fn conformance_takes_an_expect_fail_prefix_and_no_origin() {
        let parsed = parse_args(&args(&[
            "conformance",
            "--spec",
            "s.jsonc",
            "--out",
            "o",
            "--expect-fail-prefix",
            "violates-",
        ]))
        .unwrap();
        assert_eq!(parsed.command, Command::Conformance);
        assert_eq!(
            parsed.conformance.expect_fail_prefix.as_deref(),
            Some("violates-")
        );
        assert!(parse_args(&args(&[
            "check-conformance",
            "--spec",
            "s",
            "--out",
            "o",
            "--origin",
            "http://127.0.0.1:1"
        ]))
        .is_err());
        assert!(parse_args(&args(&[
            "web",
            "--spec",
            "s",
            "--out",
            "o",
            "--expect-fail-prefix",
            "v"
        ]))
        .is_err());
    }

    #[test]
    fn usage_errors_are_reported() {
        assert!(parse_args(&args(&[])).is_err());
        assert!(parse_args(&args(&["build"])).is_err());
        assert!(parse_args(&args(&["check", "--spec"])).is_err());
        assert!(parse_args(&args(&["check", "--out", "x"])).is_err());
        assert!(parse_args(&args(&[
            "check", "--spec", "a", "--spec", "b", "--out", "x"
        ]))
        .is_err());
    }
}
