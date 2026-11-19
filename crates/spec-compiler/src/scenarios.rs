//! The scenario tests: `specc scenarios` turns the specification's scenarios that run only
//! implementation-level machines into Rust integration tests, one `#[test]` per scenario, in one
//! file (`scenarios.rs`) meant for the generated crate's `tests` directory.
//!
//! # Why a separate command
//!
//! The crate that `specc generate` writes is a library whose files are all derived from the
//! specification alone; the scenario tests also need Exegra's trace of every scenario, which
//! `generate` neither has nor should need. Like `specc e2e`, the output is its own command with
//! its own `check`, written into a directory the caller names (`<crate>/tests`), so a crate
//! without scenario tests, or a build without Exegra, is unaffected.
//!
//! # Where the expectations come from
//!
//! A scenario's claims are trace properties, which nothing in Rust can assert directly. What a
//! test can assert is what Exegra computed for the same steps: after every delivery, each
//! machine's state and variables, the events it emitted (with receiver and delivery), the timers
//! it set (name, delay and event) and the timers pending. specc runs the Exegra CLI `trace` for
//! each scenario at generation time and writes those values into the test as literals, so the
//! committed test needs neither Exegra nor Node. Without a trace source nothing is written and
//! the command is refused with [`NO_EXEGRA`]: a test with no expectations would pass whatever
//! the machines do.
//!
//! # What a test does
//!
//! The test builds each machine's initial state with `Machine::initial` and checks it against the
//! trace's initial configuration. It then replays the trace's deliveries in order: each is one
//! `Machine::step` with `now` equal to the delivery's time (Exegra's milliseconds), for exactly
//! the machines Exegra routed the event to: those of the step's `fired` and `ignored` lists. A
//! machine Exegra did not route the event to is not stepped, however many events it receives; a
//! machine Exegra routed an event to that its generated `Event` has no variant for (an event it
//! neither has a transition on nor ignores) is not stepped either, since the type cannot hold it,
//! and Exegra must have ignored it (a firing on such an event is a trace mismatch). A step, a
//! time or an event id the trace does not carry is a trace mismatch, never a default. A delivery
//! that Exegra made because a timer
//! fired is checked against the test's own timer table, which applies the `TimerCommand`s of the
//! steps exactly as the runtime host does (a set replaces a timer of the same name, a cancel
//! removes it): the timer must be pending, due at that time, with that event.
//!
//! # Faults
//!
//! Exegra applies the scenario's faults when it runs the trace, so the trace's deliveries are
//! already what the runtime would see: a `delay-event` shows up as the same event delivered
//! later, a `drop-event` as a step that delivers nothing (the test notes it in a comment), and an
//! `inject-event` as an extra delivery. Those three kinds are supported. A fault that changes the
//! model rather than the input (`enable-transition`, `disable-transition`, and kinds with no
//! counterpart in a generated machine) cannot be expressed against generated code and is refused
//! with [`FAULT_UNSUPPORTED`].
//!
//! # Linking tests to requirements
//!
//! A claim with `models: <requirement>` whose `evaluate.scenarios` includes the scenario is a
//! check over the model that restates that requirement; each such requirement's `ref` becomes one
//! `/// Verifies: <ref>` doc line on the test, which Concordance's Rust indexer reads.
//!
//! # Not supported
//!
//! Machines with `instances` (several instances of one behavior), and machines that do not
//! create their records (`initial` is only defined for ones that do), are refused.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

use serde_json::{json, Map, Value};

use crate::diagnostic::{dedup, Diagnostic, SPEC_ENTITY};
use crate::emit::{rustfmt, FileError, GeneratedFile, HEADER_MARKERS, RUSTFMT_MISSING};
use crate::{jsonc, naming};

/// Code of a text that is not JSONC.
pub const SYNTAX: &str = "specc.scenarios-spec-syntax";
/// Code of a value with the wrong shape.
pub const SHAPE: &str = "specc.scenarios-spec-shape";
/// Code of a run with no Exegra to take expectations from.
pub const NO_EXEGRA: &str = "specc.scenarios-no-exegra";
/// Code of an Exegra run that failed or printed something that is not a trace.
pub const EXEGRA_FAILED: &str = "specc.scenarios-exegra-failed";
/// Code of a scenario that has no time unit of milliseconds.
pub const TIME_UNIT: &str = "specc.scenarios-time-unit";
/// Code of a fault that cannot be expressed against generated machines.
pub const FAULT_UNSUPPORTED: &str = "specc.scenarios-fault-unsupported";
/// Code of a machine with several instances.
pub const INSTANCES: &str = "specc.scenarios-instances-unsupported";
/// Code of a machine that does not create its records.
pub const NOT_CREATED: &str = "specc.scenarios-machine-not-created";
/// Code of a trace that does not end quiescent.
pub const TRACE_OUTCOME: &str = "specc.scenarios-trace-outcome";
/// Code of a trace that does not fit the specification it was run for.
pub const TRACE_MISMATCH: &str = "specc.scenarios-trace-mismatch";
/// Code of a claim that models something that is not a requirement with a `ref`.
pub const VERIFIES: &str = "specc.scenarios-verifies-invalid";
/// Code of an emitted event with no delivery guarantee.
pub const DELIVERY_MISSING: &str = "specc.scenarios-delivery-missing";
/// Code of a name that cannot be a Rust identifier.
pub const NAME: &str = "specc.scenarios-name-invalid";
/// Code of a specification with no scenario that runs only implementation machines.
pub const NO_SCENARIOS: &str = "specc.scenarios-none";

/// Where the trace of a scenario comes from: its id to the `result` of Exegra's `trace`, or why
/// there is none.
pub type TraceSource<'a> = &'a dyn Fn(&str) -> Result<Value, String>;

/// The generated files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenariosOutput {
    /// `scenarios.rs`.
    pub files: Vec<GeneratedFile>,
}

/// The file the tests are written to.
pub const FILE: &str = "scenarios.rs";

/// A trace source that runs the Exegra CLI (`node <home>/cli/exegra.js trace <scenario>
/// --project <project> --specs <specs>`).
pub fn exegra_cli(
    home: PathBuf,
    project: String,
    specs: PathBuf,
) -> impl Fn(&str) -> Result<Value, String> {
    move |scenario| {
        let output = Command::new("node")
            .arg(home.join("cli").join("exegra.js"))
            .args(["trace", scenario, "--project", &project, "--specs"])
            .arg(&specs)
            .output()
            .map_err(|e| format!("node could not be run ({e})"))?;
        let envelope: Value = serde_json::from_slice(&output.stdout).map_err(|e| {
            format!(
                "the Exegra CLI did not print JSON ({e}): {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
        })?;
        if envelope["ok"] != json!(true) {
            return Err(format!("Exegra refused: {}", envelope["error"]["message"]));
        }
        Ok(envelope["result"].clone())
    }
}

/// Generates the scenario tests from a specification's text, or refuses with every diagnostic.
/// `crate_name` is the package the tests import the machines from; `traces` is where Exegra's
/// traces come from (`None` refuses with [`NO_EXEGRA`]).
pub fn generate(
    spec_path: &str,
    text: &str,
    crate_name: &str,
    traces: Option<TraceSource>,
) -> Result<ScenariosOutput, Vec<Diagnostic>> {
    let spec_path = spec_path.replace('\\', "/");
    let mut plan = load(text)?;
    plan.spec = crate::load::load(&spec_path, text).ok();
    if let Some(spec) = &plan.spec {
        plan.cases = crate::wire::type_cases(spec);
    }
    let mut d = Vec::new();
    if !crate::emit::is_crate_name(crate_name) {
        d.push(Diagnostic::new(
            crate::CRATE_NAME_INVALID,
            SPEC_ENTITY,
            format!("\"{crate_name}\" cannot be a package name."),
        ));
    }
    if plan.scenarios.is_empty() {
        d.push(Diagnostic::new(
            NO_SCENARIOS,
            SPEC_ENTITY,
            "no scenario runs only implementation-level machines, so there is nothing to generate.",
        ));
    }
    let mut names = crate::codetext::Claims::new(FILE, &[]);
    for sc in &plan.scenarios {
        if let Err(clash) = names.claim(&sc.id, &test_name(&sc.id)) {
            d.push(clash);
        }
        d.extend(check_scenario(&plan, sc));
    }
    if !d.is_empty() {
        return Err(dedup(d));
    }
    let Some(traces) = traces else {
        return Err(vec![Diagnostic::new(
            NO_EXEGRA,
            SPEC_ENTITY,
            "the expected states, variables and events of a scenario test are what Exegra computes, so specc needs Exegra to generate them: set EXEGRA_HOME to an Exegra checkout (one with cli/exegra.js).",
        )]);
    };
    let mut tests = Vec::new();
    for sc in &plan.scenarios {
        match traces(&sc.id) {
            Err(e) => d.push(Diagnostic::new(
                EXEGRA_FAILED,
                &sc.id,
                format!("no trace: {e}"),
            )),
            Ok(trace) => match test_of(&plan, sc, &trace) {
                Ok(code) => tests.push(code),
                Err(mut e) => d.append(&mut e),
            },
        }
    }
    if !d.is_empty() {
        return Err(dedup(d));
    }
    let body = file_text(&plan, crate_name, &tests);
    let text = format!(
        "{} from {}. Do not edit; change the specification and regenerate.\n{body}",
        HEADER_MARKERS[0],
        crate::codetext::line_text(&spec_path)
    );
    let contents = match rustfmt(&text) {
        Ok(formatted) => formatted,
        Err(FileError::RustfmtMissing(e)) => {
            return Err(vec![Diagnostic::new(
                RUSTFMT_MISSING,
                FILE,
                format!(
                    "rustfmt could not be started ({e}); specc does not write unformatted code."
                ),
            )])
        }
        Err(_) => {
            return Err(vec![Diagnostic::new(
                crate::emit::INTERNAL,
                FILE,
                "rustfmt refused the scenario tests; this is a defect in specc.",
            )])
        }
    };
    Ok(ScenariosOutput {
        files: vec![GeneratedFile {
            path: FILE.to_string(),
            contents,
        }],
    })
}

// ---- loading ------------------------------------------------------------------------------

/// One machine of a scenario.
struct MachineInfo {
    id: String,
    /// Ident of the module and type.
    module: String,
    ty: String,
    /// Events the machine receives: those it has a transition on and those a state ignores.
    events: BTreeSet<String>,
    /// Transition id to its effects.
    transitions: Vec<(String, Vec<Value>)>,
}

struct ScenarioPlan {
    id: String,
    name: String,
    machines: Vec<MachineInfo>,
    faults: Vec<String>,
    verifies: Vec<String>,
    /// Diagnostics found while reading it (names, claims).
    problems: Vec<Diagnostic>,
}

struct Plan {
    scenarios: Vec<ScenarioPlan>,
    /// Every event: id to (delivery, elapsed).
    events: Vec<(String, Option<String>, bool)>,
    /// Every fault: id to kind.
    fault_kinds: Vec<(String, String)>,
    /// Behaviors that run as several instances or that do not create their records.
    unsupported: Vec<(String, &'static str, &'static str)>,
    /// The typed specification, when it loads: payloads and variables are written in the wire
    /// form the machines serialise ([`crate::wire`]). Without it they are written as traced.
    spec: Option<crate::ir::Spec>,
    /// The wire case of each struct type a contract reaches, when `spec` loads.
    cases: std::collections::BTreeMap<String, crate::wire::WireCase>,
}

impl Plan {
    /// An event's payload in the wire form its machine serialises.
    fn wire_payload(&self, event: &str, payload: &Value) -> Value {
        match &self.spec {
            Some(spec) => crate::wire::WireValues::new(spec, &self.cases).payload(event, payload),
            None => payload.clone(),
        }
    }

    /// A machine's variables in the wire form it serialises.
    fn wire_vars(&self, machine: &str, vars: &Value) -> Value {
        match &self.spec {
            Some(spec) => crate::wire::WireValues::new(spec, &self.cases).vars(machine, vars),
            None => vars.clone(),
        }
    }
}

fn string<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    obj.get(key).and_then(Value::as_str)
}

fn items<'a>(top: &'a Map<String, Value>, key: &str) -> Vec<&'a Map<String, Value>> {
    top.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default()
}

fn load(text: &str) -> Result<Plan, Vec<Diagnostic>> {
    let value = jsonc::parse(text).map_err(|e| {
        vec![Diagnostic::new(
            SYNTAX,
            SPEC_ENTITY,
            format!("the specification is not valid JSONC: {e}"),
        )]
    })?;
    let Some(top) = value.as_object() else {
        return Err(vec![Diagnostic::new(
            SHAPE,
            SPEC_ENTITY,
            "the specification is not a JSON object",
        )]);
    };
    let events: Vec<(String, Option<String>, bool)> = items(top, "events")
        .iter()
        .filter_map(|e| {
            Some((
                string(e, "id")?.to_string(),
                string(e, "delivery").map(str::to_string),
                e.get("elapsed") == Some(&json!(true)),
            ))
        })
        .collect();
    let fault_kinds: Vec<(String, String)> = items(top, "faults")
        .iter()
        .filter_map(|f| Some((string(f, "id")?.to_string(), string(f, "kind")?.to_string())))
        .collect();
    let claims = items(top, "claims");
    let behaviors = items(top, "behaviors");

    let mut unsupported = Vec::new();
    let mut scenarios = Vec::new();
    for sc in items(top, "scenarios") {
        let Some(id) = string(sc, "id") else { continue };
        let ids: Vec<&str> = sc
            .get("behaviors")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let found: Vec<&Map<String, Value>> = ids
            .iter()
            .filter_map(|b| {
                behaviors
                    .iter()
                    .find(|x| string(x, "id") == Some(b))
                    .copied()
            })
            .collect();
        let all_impl = !ids.is_empty()
            && found.len() == ids.len()
            && found
                .iter()
                .all(|b| string(b, "level") == Some("implementation"));
        if !all_impl {
            continue;
        }
        let mut problems = Vec::new();
        let mut machines = Vec::new();
        for b in found {
            let bid = string(b, "id").unwrap_or_default();
            if b.contains_key("instances") {
                unsupported.push((
                    bid.to_string(),
                    INSTANCES,
                    "runs as several instances, which a scenario test does not drive",
                ));
            }
            let creates = b
                .get("storage")
                .and_then(|s| s.get("creates"))
                .and_then(Value::as_bool)
                == Some(true);
            if !creates {
                unsupported.push((
                    bid.to_string(),
                    NOT_CREATED,
                    "does not create its records (storage.creates), so it has no initial state to start a test from",
                ));
            }
            let (module, ty) = match (naming::snake(bid), naming::pascal(bid)) {
                (Ok(m), Ok(t)) => (m, t),
                (Err(e), _) | (_, Err(e)) => {
                    problems.push(Diagnostic::new(NAME, bid, e));
                    continue;
                }
            };
            let mut events_of = BTreeSet::new();
            let mut transitions = Vec::new();
            for t in items(b, "transitions") {
                if let Some(on) = string(t, "on") {
                    events_of.insert(on.to_string());
                }
                if let Some(tid) = string(t, "id") {
                    let effects = t
                        .get("effects")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    transitions.push((tid.to_string(), effects));
                }
            }
            for s in items(b, "states") {
                if let Some(ignores) = s.get("ignores").and_then(Value::as_array) {
                    events_of.extend(ignores.iter().filter_map(Value::as_str).map(str::to_string));
                }
            }
            machines.push(MachineInfo {
                id: bid.to_string(),
                module,
                ty,
                events: events_of,
                transitions,
            });
        }
        let faults = sc
            .get("faults")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let mut verifies = Vec::new();
        for claim in &claims {
            let in_scenario = claim
                .get("evaluate")
                .and_then(|e| e.get("scenarios"))
                .and_then(Value::as_array)
                .is_some_and(|s| s.iter().any(|v| v.as_str() == Some(id)));
            if !in_scenario {
                continue;
            }
            let models: Vec<&str> = match claim.get("models") {
                Some(Value::String(s)) => vec![s.as_str()],
                Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
                _ => continue,
            };
            let claim_id = string(claim, "id").unwrap_or_default();
            for m in models {
                match claims.iter().find(|c| string(c, "id") == Some(m)) {
                    Some(r) if string(r, "kind") == Some("requirement") => match string(r, "ref") {
                        Some(reference) => {
                            if !verifies.iter().any(|v| v == reference) {
                                verifies.push(reference.to_string());
                            }
                        }
                        None => problems.push(Diagnostic::new(
                            VERIFIES,
                            id,
                            format!("claim \"{claim_id}\" models requirement \"{m}\", which has no ref, so Concordance cannot link a test to it."),
                        )),
                    },
                    _ => problems.push(Diagnostic::new(
                        VERIFIES,
                        id,
                        format!("claim \"{claim_id}\" models \"{m}\", which is not a requirement."),
                    )),
                }
            }
        }
        scenarios.push(ScenarioPlan {
            id: id.to_string(),
            name: string(sc, "name").unwrap_or(id).to_string(),
            machines,
            faults,
            verifies,
            problems,
        });
    }
    Ok(Plan {
        scenarios,
        events,
        fault_kinds,
        unsupported,
        spec: None,
        cases: std::collections::BTreeMap::new(),
    })
}

/// The refusals that need no trace.
fn check_scenario(plan: &Plan, sc: &ScenarioPlan) -> Vec<Diagnostic> {
    let mut d = sc.problems.clone();
    for m in &sc.machines {
        for (bid, code, why) in &plan.unsupported {
            if *bid == m.id {
                d.push(Diagnostic::new(
                    code,
                    &sc.id,
                    format!("behavior \"{bid}\" {why}."),
                ));
            }
        }
    }
    for fault in &sc.faults {
        let kind = plan
            .fault_kinds
            .iter()
            .find(|(id, _)| id == fault)
            .map(|(_, k)| k.as_str());
        if !matches!(kind, Some("drop-event" | "delay-event" | "inject-event")) {
            d.push(Diagnostic::new(
                FAULT_UNSUPPORTED,
                &sc.id,
                format!(
                    "fault \"{fault}\" is of kind {}: a generated machine receives events and sends none back, so only drop-event, delay-event and inject-event can be replayed against it (enable-transition and disable-transition change the model, not the input).",
                    kind.unwrap_or("unknown")
                ),
            ));
        }
    }
    d
}

// ---- one scenario -------------------------------------------------------------------------

fn mismatch(sc: &ScenarioPlan, message: String) -> Vec<Diagnostic> {
    vec![Diagnostic::new(TRACE_MISMATCH, &sc.id, message)]
}

/// A `json!` call building `value`: strings are Rust literals, so no control character of the
/// value makes invalid Rust (see [`crate::codetext::rust::json`]).
fn lit(value: &Value) -> String {
    format!("json!({})", crate::codetext::rust::json(value))
}

fn str_lit(text: &str) -> String {
    crate::codetext::rust::string(text)
}

fn test_of(plan: &Plan, sc: &ScenarioPlan, trace: &Value) -> Result<String, Vec<Diagnostic>> {
    if trace["timeUnit"] != json!("ms") {
        return Err(vec![Diagnostic::new(
            TIME_UNIT,
            &sc.id,
            format!(
                "the scenario's time unit is {}, but generated machines read time as milliseconds: give the scenario \"timeUnit\": \"ms\".",
                trace["timeUnit"]
            ),
        )]);
    }
    if trace["outcome"] != json!("quiescent") {
        return Err(vec![Diagnostic::new(
            TRACE_OUTCOME,
            &sc.id,
            format!(
                "the trace ends {}, not quiescent, so there is no complete behaviour to expect.",
                trace["outcome"]
            ),
        )]);
    }
    let Some(steps) = trace["steps"].as_array() else {
        return Err(mismatch(sc, "the trace has no steps".to_string()));
    };
    let mut out =
        crate::codetext::rust::doc_comment("", &format!("Scenario `{}`: {}", sc.id, sc.name));
    if !sc.verifies.is_empty() {
        out.push_str("///\n");
    }
    for line in &sc.verifies {
        out.push_str(&crate::codetext::rust::doc_comment(
            "",
            &format!("Verifies: {line}"),
        ));
    }
    out.push_str(&format!("#[test]\nfn {}() {{\n", test_name(&sc.id)));
    for m in &sc.machines {
        let Some(initial) = trace["initial"]["machines"].get(&m.id) else {
            return Err(mismatch(
                sc,
                format!("the trace has no machine \"{}\"", m.id),
            ));
        };
        let Some(initial_state) = initial["state"].as_str() else {
            return Err(mismatch(
                sc,
                format!(
                    "the trace's initial configuration of \"{}\" has no state",
                    m.id
                ),
            ));
        };
        out.push_str(&format!(
            "    let mut {var} = Run::<{ty}>::new({key}, {state}, {vars});\n",
            var = var(m),
            ty = m.ty,
            key = str_lit(&m.id),
            state = str_lit(initial_state),
            vars = lit(&plan.wire_vars(&m.id, &initial["vars"])),
        ));
    }
    for (position, step) in steps.iter().enumerate() {
        let Some(index) = step["index"].as_u64() else {
            return Err(mismatch(
                sc,
                format!("step {position} of the trace has no index"),
            ));
        };
        let Some(time) = step["time"].as_i64() else {
            return Err(mismatch(
                sc,
                format!("step {index} of the trace has no time"),
            ));
        };
        let Some(event) = step["event"]["id"].as_str() else {
            return Err(mismatch(
                sc,
                format!("step {index} of the trace has no event id"),
            ));
        };
        let Some(kind) = step["kind"].as_str() else {
            return Err(mismatch(
                sc,
                format!("step {index} of the trace has no kind"),
            ));
        };
        out.push_str(&format!("\n    // Step {index}, t={time}: {event}"));
        match kind {
            "drop" | "delay" => {
                let kind = step["kind"].as_str().unwrap_or_default();
                let faults: Vec<String> = step["faults"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|f| f["fault"].as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let what = if kind == "drop" { "dropped" } else { "delayed" };
                out.push_str(&format!(
                    ", {what} by {}; nothing is delivered.\n",
                    faults.join(", ")
                ));
            }
            "deliver" => {
                out.push('\n');
                out.push_str(&deliveries(plan, sc, step, time, event)?);
            }
            other => {
                return Err(mismatch(
                    sc,
                    format!("step {index} is of kind \"{other}\", which a test cannot replay"),
                ))
            }
        }
    }
    out.push_str("}\n");
    Ok(out)
}

/// The calls that deliver one trace step to the machines that receive it.
fn deliveries(
    plan: &Plan,
    sc: &ScenarioPlan,
    step: &Value,
    time: i64,
    event: &str,
) -> Result<String, Vec<Diagnostic>> {
    let Some(index) = step["index"].as_u64() else {
        return Err(mismatch(sc, "a step of the trace has no index".to_string()));
    };
    let source = &step["event"]["source"];
    let timer = (source["kind"] == json!("timer")).then(|| source["timer"].as_str());
    let payload = if step["event"]["payload"].is_null() {
        json!({})
    } else {
        step["event"]["payload"].clone()
    };
    let input = json!({ "event": event, "payload": plan.wire_payload(event, &payload) });
    let mut out = String::new();
    for m in &sc.machines {
        // Exegra's routing decides who receives the event: the machines it fired a transition of
        // or ignored the event in. Every other machine is not delivered to.
        let fired = step["fired"]
            .as_array()
            .and_then(|f| f.iter().find(|f| f["behavior"].as_str() == Some(&m.id)));
        let ignored = step["ignored"]
            .as_array()
            .is_some_and(|i| i.iter().any(|i| i["behavior"].as_str() == Some(&m.id)));
        if fired.is_none() && !ignored {
            continue;
        }
        if !m.events.contains(event) {
            // The machine's `Event` has no variant for this event: it cannot be stepped, and
            // Exegra can only have ignored it.
            if fired.is_some() {
                return Err(mismatch(
                    sc,
                    format!("step {index}: the trace fired a transition of \"{}\" on \"{event}\", which it neither has a transition on nor ignores", m.id),
                ));
            }
            continue;
        }
        let context = format!("step {index} (t={time}, {event})");
        let pending: Vec<&str> = step["config"]["timers"]
            .as_object()
            .map(|t| {
                t.keys()
                    .filter_map(|k| k.strip_prefix(&format!("{}/", m.id)))
                    .collect()
            })
            .unwrap_or_default();
        let pending = format!(
            "&[{}]",
            pending
                .iter()
                .map(|p| str_lit(p))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let expect = match fired {
            None => format!("Expect::Ignored {{ pending: {pending} }}"),
            Some(fired) => {
                let Some(transition) = fired["transition"].as_str() else {
                    return Err(mismatch(
                        sc,
                        format!("step {index}: a firing of \"{}\" names no transition", m.id),
                    ));
                };
                let Some(fired_to) = fired["to"].as_str() else {
                    return Err(mismatch(
                        sc,
                        format!("step {index}: the firing of \"{transition}\" has no target state"),
                    ));
                };
                let Some((_, effects)) = m.transitions.iter().find(|(id, _)| id == transition)
                else {
                    return Err(mismatch(
                        sc,
                        format!(
                            "step {index}: no transition \"{transition}\" in \"{}\"",
                            m.id
                        ),
                    ));
                };
                let emitted = fired["emitted"].as_array().cloned().unwrap_or_default();
                let effects: Vec<&Value> =
                    effects.iter().filter(|e| e.get("emit").is_some()).collect();
                if effects.len() != emitted.len() {
                    return Err(mismatch(
                        sc,
                        format!(
                            "step {index}: transition \"{transition}\" has {} emit effects but the trace emitted {} events",
                            effects.len(),
                            emitted.len()
                        ),
                    ));
                }
                let mut emits = Vec::new();
                let mut timers = Vec::new();
                for (effect, e) in effects.iter().zip(&emitted) {
                    let Some(id) = e["event"].as_str() else {
                        return Err(mismatch(
                            sc,
                            format!("step {index}: transition \"{transition}\" emitted an event with no id"),
                        ));
                    };
                    if effect["emit"].as_str() != Some(id) {
                        return Err(mismatch(
                            sc,
                            format!("step {index}: transition \"{transition}\" emits {} where the trace emitted \"{id}\"", effect["emit"]),
                        ));
                    }
                    let value =
                        json!({ "event": id, "payload": plan.wire_payload(id, &e["payload"]) });
                    let Some(due) = e["time"].as_i64() else {
                        return Err(mismatch(
                            sc,
                            format!("step {index}: the emitted \"{id}\" of \"{transition}\" has no time"),
                        ));
                    };
                    if let Some(name) = effect["timer"].as_str() {
                        let after = due - time;
                        timers.push(format!("({}, {after}, {})", str_lit(name), lit(&value)));
                    } else {
                        let to = effect["to"].as_str().unwrap_or_default();
                        let delivery = plan
                            .events
                            .iter()
                            .find(|(eid, _, _)| eid == id)
                            .and_then(|(_, d, _)| d.as_deref());
                        let Some(delivery) = delivery else {
                            return Err(vec![Diagnostic::new(
                                DELIVERY_MISSING,
                                &sc.id,
                                format!("event \"{id}\" is emitted to \"{to}\" but declares no delivery."),
                            )]);
                        };
                        emits.push(format!(
                            "({}, {}, {})",
                            str_lit(to),
                            str_lit(delivery),
                            lit(&value)
                        ));
                    }
                }
                let after = &step["config"]["machines"][&m.id];
                format!(
                    "Expect::Fired {{ to: {to}, vars: {vars}, emits: vec![{emits}], timers: vec![{timers}], pending: {pending} }}",
                    to = str_lit(fired_to),
                    vars = lit(&plan.wire_vars(&m.id, &after["vars"])),
                    emits = emits.join(", "),
                    timers = timers.join(", "),
                )
            }
        };
        // A timer's event is checked against the timer table of the machine that set the timer.
        let timer_arg = match timer {
            Some(Some(name)) if source["behavior"].as_str() == Some(&m.id) => {
                format!("Some({})", str_lit(name))
            }
            _ => "None".to_string(),
        };
        out.push_str(&format!(
            "    {var}.deliver({ctx}, {time}, {timer_arg}, {input}, {expect});\n",
            var = var(m),
            ctx = str_lit(&context),
            input = lit(&input),
        ));
    }
    Ok(out)
}

fn var(m: &MachineInfo) -> String {
    format!("run_{}", m.module)
}

/// The name of the test function of a scenario. Ids that differ only in characters that are not
/// letters or digits (`a-b`, `a_b`, `a.b`) give one name: [`generate`] refuses them with
/// [`crate::codetext::IDENTIFIER_COLLISION`].
fn test_name(id: &str) -> String {
    crate::codetext::rust::fn_name("scenario", id)
}

// ---- the file -----------------------------------------------------------------------------

fn file_text(plan: &Plan, crate_name: &str, tests: &[String]) -> String {
    let crate_ident = crate_name.replace('-', "_");
    let mut used: Vec<&MachineInfo> = Vec::new();
    for sc in &plan.scenarios {
        for m in &sc.machines {
            if !used.iter().any(|u| u.id == m.id) {
                used.push(m);
            }
        }
    }
    let imports: String = used
        .iter()
        .map(|m| format!("use {crate_ident}::machines::{}::{};\n", m.module, m.ty))
        .collect();
    format!(
        "//! Scenario tests: each test drives the generated machines through one scenario of the\n\
         //! specification, with the expectations Exegra computed for it.\n\
         \n\
         use std::collections::BTreeMap;\n\
         \n\
         use serde::Serialize;\n\
         use serde_json::{{json, Value}};\n\
         use strata_runtime::{{Machine, Time, TimerCommand}};\n\
         \n\
         {imports}\n\
         {HELPERS}\n\
         {tests}",
        tests = tests.join("\n"),
    )
}

/// The test harness every generated file starts with.
const HELPERS: &str = r#"/// What Exegra computed for one delivery to one machine.
enum Expect {
    /// A transition fired.
    Fired {
        /// The state after.
        to: &'static str,
        /// The variables after.
        vars: Value,
        /// The emitted events: receiver, delivery and event.
        emits: Vec<(&'static str, &'static str, Value)>,
        /// The timers set: name, delay in milliseconds and event.
        timers: Vec<(&'static str, i64, Value)>,
        /// The timers pending after, by name.
        pending: &'static [&'static str],
    },
    /// No transition fired: nothing changes and nothing is asked of the runtime.
    Ignored {
        /// The timers pending after, by name.
        pending: &'static [&'static str],
    },
}

/// One machine instance, with the timers the runtime host would keep for it.
struct Run<M: Machine> {
    state: M::State,
    vars: M::Vars,
    /// Pending timers: name to the time they are due and the event they fire.
    timers: BTreeMap<String, (i64, Value)>,
}

/// A value with every number that is a whole number written as an integer, so that values
/// compare by value: an `f64` variable holding `1` equals `1.0`.
fn norm(value: Value) -> Value {
    match value {
        Value::Number(n) if n.is_f64() => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => Value::from(f as i64),
            _ => Value::Number(n),
        },
        Value::Array(items) => Value::Array(items.into_iter().map(norm).collect()),
        Value::Object(map) => Value::Object(map.into_iter().map(|(k, v)| (k, norm(v))).collect()),
        other => other,
    }
}

fn to_json<T: Serialize>(value: &T) -> Value {
    norm(serde_json::to_value(value).expect("serialisable"))
}

impl<M: Machine> Run<M>
where
    M::State: Serialize,
    M::Vars: Serialize,
{
    /// The machine in its initial state, checked against Exegra's initial configuration.
    fn new(key: &str, state: &str, vars: Value) -> Self {
        let key: M::Key = serde_json::from_value(Value::String(key.to_string()))
            .expect("the instance key is the behavior id");
        let (initial_state, initial_vars) = M::initial(&key);
        assert_eq!(to_json(&initial_state), Value::String(state.to_string()), "initial state");
        assert_eq!(to_json(&initial_vars), norm(vars), "initial variables");
        Run {
            state: initial_state,
            vars: initial_vars,
            timers: BTreeMap::new(),
        }
    }

    /// One step of the machine at `now`, then the checks. `timer` names the timer whose firing
    /// delivers `event`: it must be pending, due now, with this event.
    fn deliver(&mut self, at: &str, now: i64, timer: Option<&str>, event: Value, expect: Expect) {
        if let Some(name) = timer {
            let (due, scheduled) = self
                .timers
                .remove(name)
                .unwrap_or_else(|| panic!("{at}: timer {name} is not pending"));
            assert_eq!(due, now, "{at}: when timer {name} is due");
            assert_eq!(norm(scheduled), norm(event.clone()), "{at}: the event of timer {name}");
        }
        let event: M::Event = serde_json::from_value(event)
            .unwrap_or_else(|e| panic!("{at}: not an event of the machine: {e}"));
        let outcome = M::step(&self.state, &self.vars, &event, Time(now))
            .unwrap_or_else(|e| panic!("{at}: the step refused: {e}"));
        let mut set = Vec::new();
        for command in &outcome.timers {
            match command {
                TimerCommand::Set { name, after, event } => {
                    let after = i64::try_from(after.as_millis()).expect("a delay in range");
                    set.push((name.as_str(), after, to_json(event)));
                    self.timers.insert(name.clone(), (now + after, to_json(event)));
                }
                TimerCommand::Cancel { name } => {
                    self.timers.remove(name);
                }
            }
        }
        let pending: Vec<&str> = self.timers.keys().map(String::as_str).collect();
        match expect {
            Expect::Fired {
                to,
                vars,
                emits,
                timers,
                pending: expected,
            } => {
                assert_eq!(to_json(&outcome.state), Value::String(to.to_string()), "{at}: state");
                assert_eq!(to_json(&outcome.vars), norm(vars), "{at}: variables");
                let emitted: Vec<(&str, &str, Value)> = outcome
                    .emits
                    .iter()
                    .map(|e| (e.receiver.as_str(), e.delivery.name(), to_json(&e.event)))
                    .collect();
                let emits: Vec<(&str, &str, Value)> =
                    emits.into_iter().map(|(to, d, e)| (to, d, norm(e))).collect();
                let timers: Vec<(&str, i64, Value)> =
                    timers.into_iter().map(|(n, a, e)| (n, a, norm(e))).collect();
                assert_eq!(emitted, emits, "{at}: emitted events");
                assert_eq!(set, timers, "{at}: timers set");
                assert_eq!(pending, expected, "{at}: timers pending");
            }
            Expect::Ignored { pending: expected } => {
                assert_eq!(to_json(&outcome.state), to_json(&self.state), "{at}: ignored, but the state changed");
                assert_eq!(to_json(&outcome.vars), to_json(&self.vars), "{at}: ignored, but the variables changed");
                assert!(outcome.emits.is_empty(), "{at}: ignored, but emitted");
                assert!(set.is_empty(), "{at}: ignored, but set timers");
                assert_eq!(pending, expected, "{at}: timers pending");
            }
        }
        self.state = outcome.state;
        self.vars = outcome.vars;
    }
}
"#;
