//! Differential test: the generated lease machine steps exactly as Exegra runs the same
//! specification.
//!
//! The Exegra CLI runs scenario `lease-run` of `tests/fixtures/machines/spec.jsonc`
//! (`node $EXEGRA_HOME/cli/exegra.js trace lease-run --project machines --specs tests/fixtures`),
//! and the generated `Lease::step` of `crates/spec-compiler-machines-fixture` is driven with the
//! same events at the same times. After every step the state, the variables, the emitted events
//! (with the receiver and delivery the specification gives them, and the delay of a delayed emit:
//! Exegra schedules it `after` the step, the generated outcome carries it as `Emit::after`), the
//! timers set and the timers pending must agree with the trace. Exegra's time is read as
//! milliseconds.
//!
//! Fails when Exegra or `node` is missing (see `support`); skipped, with a loud banner, only when
//! `SPECC_SKIP_EXEGRA_TESTS=1` says so.

mod support;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

use serde_json::{json, Value};
use strata_runtime::{Delivery, Machine, Time, TimerCommand};
use strata_spec_compiler::jsonc;
use strata_spec_compiler_machines_fixture::machines::lease::{Event, Lease, State, Vars};

const BEHAVIOR: &str = "lease";
const SCENARIO: &str = "lease-run";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The trace of the scenario, or `None` only when `SPECC_SKIP_EXEGRA_TESTS=1` opted out (a banner
/// is printed). A missing Exegra or `node` fails the test.
fn exegra_trace() -> Option<Value> {
    let cli = support::exegra_cli("the comparison of the generated lease with Exegra")?;
    if !support::node_required("the comparison of the generated lease with Exegra") {
        return None;
    }
    let specs = manifest_dir().join("tests").join("fixtures");
    let output = Command::new("node")
        .arg(&cli)
        .args(["trace", SCENARIO, "--project", "machines", "--specs"])
        .arg(&specs)
        .output()
        .unwrap_or_else(|e| panic!("node could not be run ({e})"));
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "the Exegra CLI did not print JSON ({e}):\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert_eq!(
        envelope["ok"],
        json!(true),
        "Exegra refused the fixture: {envelope:#}"
    );
    Some(envelope["result"].clone())
}

/// The fixture specification, parsed.
fn fixture_spec() -> Value {
    let text =
        std::fs::read_to_string(manifest_dir().join("tests/fixtures/machines/spec.jsonc")).unwrap();
    jsonc::parse(&text).unwrap()
}

/// The receivers (`to`) of the emit effects of a transition of the lease, in effect order.
fn receivers(spec: &Value, transition: &str) -> Vec<(String, String)> {
    let lease = spec["behaviors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["id"] == BEHAVIOR)
        .unwrap();
    let t = lease["transitions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == transition)
        .unwrap_or_else(|| panic!("no transition {transition}"));
    t["effects"]
        .as_array()
        .map(|effects| {
            effects
                .iter()
                .filter(|e| e.get("emit").is_some() && e.get("timer").is_none())
                .map(|e| {
                    (
                        e["emit"].as_str().unwrap().to_string(),
                        e["to"].as_str().unwrap().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The delivery an event declares in the specification.
fn delivery(spec: &Value, event: &str) -> Delivery {
    let e = spec["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == event)
        .unwrap();
    match e["delivery"].as_str() {
        Some("at-least-once") => Delivery::AtLeastOnce,
        Some("at-most-once") => Delivery::AtMostOnce,
        other => panic!("event {event} has delivery {other:?}"),
    }
}

/// For each effect of the transition that makes Exegra list an emitted event (an `emit` or a
/// `timer`, in order), whether it is a timer. The kind is the effect's, not the event's.
fn producing_effects_are_timers(spec: &Value, transition: &str) -> Vec<bool> {
    let lease = spec["behaviors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["id"] == BEHAVIOR)
        .unwrap();
    let t = lease["transitions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == transition)
        .unwrap_or_else(|| panic!("no transition {transition}"));
    t["effects"]
        .as_array()
        .map(|effects| {
            effects
                .iter()
                .filter(|e| e.get("timer").is_some() || e.get("emit").is_some())
                .map(|e| e.get("timer").is_some())
                .collect()
        })
        .unwrap_or_default()
}

/// The lease's machine in a configuration of the trace.
fn machine_of(config: &Value) -> (String, Value) {
    let m = &config["machines"][BEHAVIOR];
    assert!(m.is_object(), "no machine {BEHAVIOR} in {config:#}");
    (m["state"].as_str().unwrap().to_string(), m["vars"].clone())
}

/// The names of the lease's timers pending in a configuration (timer ids are `<key>/<name>`).
fn pending_timers(config: &Value) -> BTreeSet<String> {
    let prefix = format!("{BEHAVIOR}/");
    config["timers"]
        .as_object()
        .map(|timers| {
            timers
                .keys()
                .filter_map(|id| id.strip_prefix(&prefix).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn assert_machine(context: &str, state: State, vars: &Vars, config: &Value) {
    let (expected_state, expected_vars) = machine_of(config);
    assert_eq!(state.id(), expected_state, "{context}: state");
    assert_eq!(
        support::norm(serde_json::to_value(vars).unwrap()),
        support::norm(expected_vars),
        "{context}: variables"
    );
}

#[test]
fn the_generated_lease_steps_as_exegra_runs_it() {
    let Some(trace) = exegra_trace() else {
        return;
    };
    let spec = fixture_spec();
    assert_eq!(trace["scenarioId"], json!(SCENARIO));
    assert_eq!(
        trace["outcome"],
        json!("quiescent"),
        "the scenario must end: {}",
        trace["outcome"]
    );

    let key = "the-lease".to_string();
    let (mut state, mut vars) = Lease::initial(&key);
    assert_machine("initially", state, &vars, &trace["initial"]);

    let mut timers: BTreeSet<String> = BTreeSet::new();
    let mut fired_steps = 0;
    let mut delayed_emits = 0;
    for step in trace["steps"].as_array().unwrap() {
        let index = &step["index"];
        let time = step["time"].as_i64().unwrap();
        let event_id = step["event"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("step {index} of the trace has no event id"))
            .to_string();
        let context = format!("step {index} (t={time}, {event_id})");
        let fired = step["fired"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["behavior"] == BEHAVIOR);

        // A timer of this instance that fires is no longer pending.
        if let Some(id) = step["event"]["timer"].as_str() {
            if let Some(name) = id.strip_prefix(&format!("{BEHAVIOR}/")) {
                timers.remove(name);
            }
        }

        let event: Event = match serde_json::from_value(
            json!({ "event": event_id, "payload": step["event"]["payload"] }),
        ) {
            Ok(event) => event,
            Err(_) => {
                // An event the lease does not receive, such as one it emitted to a receiver
                // outside the scenario.
                assert!(
                    fired.is_none(),
                    "{context}: the lease fired on an event it does not receive"
                );
                continue;
            }
        };

        let outcome = Lease::step(&state, &vars, &event, Time(time))
            .unwrap_or_else(|e| panic!("{context}: the generated step refused: {e}"));

        let Some(fired) = fired else {
            // Ignored: nothing changes and nothing is asked of the runtime.
            assert_eq!(
                outcome.state, state,
                "{context}: ignored, but the state changed"
            );
            assert_eq!(
                outcome.vars, vars,
                "{context}: ignored, but the variables changed"
            );
            assert!(outcome.emits.is_empty(), "{context}: ignored, but emitted");
            assert!(
                outcome.timers.is_empty(),
                "{context}: ignored, but set timers"
            );
            assert_machine(&context, state, &vars, &step["config"]);
            continue;
        };
        fired_steps += 1;

        assert_eq!(
            outcome.state.id(),
            fired["to"].as_str().unwrap(),
            "{context}: target state"
        );
        assert_machine(&context, outcome.state, &outcome.vars, &step["config"]);

        // Emits: the same events and payloads, in order, each with the same delay (0 for an
        // immediate emit), to the effect's `to` with its event's delivery.
        let transition = fired["transition"].as_str().unwrap();
        let mut exegra_emits = Vec::new();
        let mut exegra_timers = Vec::new();
        let kinds = producing_effects_are_timers(&spec, transition);
        let emitted = fired["emitted"].as_array().unwrap();
        assert_eq!(
            kinds.len(),
            emitted.len(),
            "{context}: the emitted entries and the emit and timer effects of {transition}"
        );
        for (e, is_timer) in emitted.iter().zip(&kinds) {
            let id = e["event"].as_str().unwrap();
            let value = json!({ "event": id, "payload": e["payload"] });
            let after = e["time"].as_i64().unwrap() - time;
            if *is_timer {
                exegra_timers.push((value, after));
            } else {
                exegra_emits.push((value, after));
            }
        }
        let emits: Vec<(Value, i64)> = outcome
            .emits
            .iter()
            .map(|e| {
                let after = e.after.map_or(0, |d| i64::try_from(d.as_millis()).unwrap());
                (serde_json::to_value(&e.event).unwrap(), after)
            })
            .collect();
        assert_eq!(emits, exegra_emits, "{context}: emitted events and delays");
        delayed_emits += emits.iter().filter(|(_, after)| *after > 0).count();
        let expected_receivers = receivers(&spec, transition);
        assert_eq!(
            outcome.emits.len(),
            expected_receivers.len(),
            "{context}: emits of {transition}"
        );
        for (emit, (event, to)) in outcome.emits.iter().zip(&expected_receivers) {
            assert_eq!(emit.receiver.as_str(), to, "{context}: receiver of {event}");
            assert_eq!(
                emit.delivery,
                delivery(&spec, event),
                "{context}: delivery of {event}"
            );
        }

        // Timers: the same events, payloads and delays; then the same timers pending.
        let mut set = Vec::new();
        for command in &outcome.timers {
            match command {
                TimerCommand::Set { name, after, event } => {
                    let after = i64::try_from(after.as_millis()).unwrap();
                    set.push((serde_json::to_value(event).unwrap(), after));
                    timers.insert(name.clone());
                }
                TimerCommand::Cancel { name } => {
                    timers.remove(name);
                }
            }
        }
        assert_eq!(set, exegra_timers, "{context}: timers set");
        assert_eq!(
            timers,
            pending_timers(&step["config"]),
            "{context}: timers pending"
        );

        state = outcome.state;
        vars = outcome.vars;
    }
    assert_machine("finally", state, &vars, &trace["final"]);
    assert!(
        fired_steps >= 8,
        "the scenario should exercise the lease; it fired {fired_steps} times"
    );
    assert!(
        delayed_emits >= 1,
        "the scenario should exercise a delayed emit of the lease"
    );
}
