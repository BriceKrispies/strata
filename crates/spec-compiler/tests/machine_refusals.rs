//! Refusals of the machines backend: an implementation-level machine that one generated `step`
//! cannot express, or that its storage record cannot hold, is refused with a diagnostic naming
//! the machine, never with a panic and never by inventing behaviour or storage.

use serde_json::{json, Value};
use strata_spec_compiler::backend::machines as m;
use strata_spec_compiler::{generate, Diagnostic, Options, RuntimeDependency};

fn options() -> Options {
    Options {
        crate_name: "generated".to_string(),
        runtime: RuntimeDependency::Path("../runtime".to_string()),
    }
}

/// A ready specification with one machine `m`: two states, a variable, a guarded transition
/// with a set, an emit and a timer, an `otherwise`, an ignored event and a cancel.
fn base() -> Value {
    json!({
        "exegra": "0.1",
        "system": { "id": "sys", "name": "System" },
        "components": [ { "id": "svc", "name": "Service" } ],
        "records": [
            { "id": "m-rec", "name": "M", "owner": "svc", "key": ["id"], "schemaVersion": 1,
              "fields": [
                  { "name": "id", "type": "string" },
                  { "name": "state", "type": "string" },
                  { "name": "count", "type": "u32" },
                  { "name": "seen", "type": "timestamp" }
              ] }
        ],
        "events": [
            { "id": "ev-go", "name": "Go", "payload": [ { "name": "n", "type": "u32" } ] },
            { "id": "ev-out", "name": "Out", "delivery": "at-least-once", "payload": [ { "name": "n", "type": "u32" } ] },
            { "id": "ev-tick", "name": "Tick", "elapsed": true }
        ],
        "behaviors": [
            { "id": "m", "name": "M", "subject": "svc", "initial": "a", "level": "implementation",
              "variables": [
                  { "name": "count", "type": "number", "initial": 0, "repr": "u32" },
                  { "name": "seen", "type": "number", "initial": 0, "repr": "timestamp" }
              ],
              "storage": { "record": "m-rec", "key": "id" },
              "transaction": { "scope": "transition", "isolation": "serializable" },
              "states": [ { "id": "a", "name": "A", "ignores": ["ev-tick"] }, { "id": "b", "name": "B" } ],
              "transitions": [
                  { "id": "t1", "from": "a", "to": "b", "on": "ev-go", "guard": "event.n > 0",
                    "effects": [
                        { "set": "count", "to": "count + event.n" },
                        { "set": "seen", "to": "time + 5" },
                        { "emit": "ev-out", "to": "svc", "payload": { "n": "count" } },
                        { "timer": "tk", "after": 5, "emit": "ev-tick" }
                    ] },
                  { "id": "t2", "from": "a", "to": "a", "on": "ev-go", "otherwise": true },
                  { "id": "t3", "from": "b", "to": "a", "on": "ev-tick", "otherwise": true,
                    "effects": [ { "cancel": "tk" } ] }
              ] }
        ]
    })
}

/// The base with `edit` applied.
fn with(edit: impl FnOnce(&mut Value)) -> String {
    let mut spec = base();
    edit(&mut spec);
    spec.to_string()
}

fn machine(spec: &mut Value) -> &mut Value {
    &mut spec["behaviors"][0]
}

fn effects(spec: &mut Value) -> &mut Vec<Value> {
    machine(spec)["transitions"][0]["effects"]
        .as_array_mut()
        .unwrap()
}

fn refuse(text: &str) -> Vec<Diagnostic> {
    match generate("spec.jsonc", text, &options()) {
        Ok(_) => panic!("the specification was not refused"),
        Err(diagnostics) => diagnostics,
    }
}

/// Refused with `code` on machine `m`, and the message contains `needle`.
fn assert_refused(text: &str, code: &str, needle: &str) {
    let diagnostics = refuse(text);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == code && d.entity == "m" && d.message.contains(needle)),
        "expected {code} on m mentioning {needle:?}, got:\n{}",
        diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn the_base_machine_generates() {
    let files = generate("spec.jsonc", &with(|_| {}), &options()).unwrap();
    let code = &files
        .iter()
        .find(|f| f.path == "src/machines.rs")
        .unwrap()
        .contents;
    assert!(code.contains("pub mod m {"));
    assert!(code.contains("impl Machine for M {"));
    let manifest = &files
        .iter()
        .find(|f| f.path == "Cargo.toml")
        .unwrap()
        .contents;
    assert!(manifest.contains("strata-runtime = { path = \"../runtime\" }"));
}

#[test]
fn reading_another_behavior_is_refused() {
    let text = with(|s| machine(s)["transitions"][0]["guard"] = json!("other.count > 0"));
    assert_refused(&text, m::CROSS_BEHAVIOUR_READ, "other.count");
    let text = with(|s| effects(s)[0]["to"] = json!("count + jobs.count.done"));
    assert_refused(&text, m::CROSS_BEHAVIOUR_READ, "jobs.count.done");
}

#[test]
fn bad_expressions_are_refused() {
    let text = with(|s| machine(s)["transitions"][0]["guard"] = json!("event.n >"));
    assert_refused(&text, m::EXPRESSION, "does not parse");
    let text = with(|s| machine(s)["transitions"][0]["guard"] = json!("event.n > 'x'"));
    assert_refused(&text, m::EXPRESSION, "cannot be generated");
    let text = with(|s| machine(s)["transitions"][0]["guard"] = json!("event.missing > 0"));
    assert_refused(&text, m::EXPRESSION, "event.missing");
}

#[test]
fn resource_effects_are_refused() {
    let text = with(|s| effects(s).push(json!({ "acquire": "pool", "units": 1, "then": "ev-go" })));
    assert_refused(&text, m::RESOURCE_EFFECT, "acquire");
}

#[test]
fn emits_the_runtime_cannot_deliver_are_refused() {
    let text = with(|s| effects(s)[2]["via"] = json!("net"));
    assert_refused(&text, m::EMIT_LINK, "via");
    let text = with(|s| effects(s)[2]["fanout"] = json!(true));
    assert_refused(&text, m::EMIT_LINK, "fanout");
    let text = with(|s| effects(s)[2]["after"] = json!({ "dist": "exponential", "mean": 5 }));
    assert_refused(&text, m::EMIT_AFTER, "ev-out");
    let text = with(|s| effects(s)[2]["after"] = json!(2.5));
    assert_refused(&text, m::EMIT_AFTER, "ev-out");
    // A whole number of milliseconds is a delayed emit.
    let files = generate(
        "spec.jsonc",
        &with(|s| effects(s)[2]["after"] = json!(5)),
        &options(),
    )
    .unwrap();
    let code = &files
        .iter()
        .find(|f| f.path == "src/machines.rs")
        .unwrap()
        .contents;
    assert!(code.contains("Duration::from_millis(5)"));
    assert!(code.contains(".emit_after("));
    let text = with(|s| {
        let e = effects(s)[2].as_object_mut().unwrap();
        e.remove("to");
        e.insert("target".into(), json!("reply"));
    });
    assert_refused(&text, m::EMIT_UNADDRESSED, "replies");
    let text = with(|s| {
        effects(s)[2].as_object_mut().unwrap().remove("to");
    });
    assert_refused(&text, m::EMIT_UNADDRESSED, "routes");
}

#[test]
fn timers_without_a_fixed_delay_are_refused() {
    let text = with(|s| effects(s)[3]["after"] = json!({ "kind": "uniform", "lo": 1, "hi": 9 }));
    assert_refused(&text, m::TIMER_AFTER, "tk");
    let text = with(|s| effects(s)[3]["after"] = json!(-1));
    assert_refused(&text, m::TIMER_AFTER, "tk");
}

#[test]
fn other_effects_are_refused() {
    let text = with(|s| {
        effects(s)
            .push(json!({ "set": "count", "to": { "sample": { "kind": "fixed", "value": 1 } } }))
    });
    assert_refused(&text, m::EFFECT_UNSUPPORTED, "sampled");
    let text = with(|s| effects(s).push(json!({ "log": "hello" })));
    assert_refused(&text, m::EFFECT_UNSUPPORTED, "log");
}

#[test]
fn counted_instances_are_refused() {
    let text = with(|s| machine(s)["instances"] = json!({ "count": 3 }));
    assert_refused(&text, m::INSTANCES_COUNTED, "count");
}

#[test]
fn storage_must_be_named_and_exist() {
    let text = with(|s| {
        machine(s).as_object_mut().unwrap().remove("storage");
    });
    assert_refused(&text, m::STORAGE, "names no record");
    let text = with(|s| machine(s)["storage"]["record"] = json!("nope"));
    assert_refused(&text, m::STORAGE, "nope");
    let text = with(|s| machine(s)["storage"]["key"] = json!("count2"));
    assert_refused(&text, m::STORAGE, "count2");
    let text = with(|s| machine(s)["instances"] = json!({ "key": "count", "storedIn": "m-rec" }));
    assert_refused(&text, "ready.storage-mismatch", "instances");
}

/// A machine is keyed by the whole key of its record (Exegra's rule `storage.key-whole`, WG-4907):
/// a key naming part of a composite key, or the fields in another order, is refused, and the list
/// of every key field in order is generated.
#[test]
fn the_machine_key_is_the_whole_record_key() {
    let composite = |key: Value| {
        with(|s| {
            s["records"][0]["fields"]
                .as_array_mut()
                .unwrap()
                .push(json!({ "name": "part", "type": "string" }));
            s["records"][0]["key"] = json!(["part", "id"]);
            machine(s)["storage"]["key"] = key;
        })
    };
    assert_refused(&composite(json!("id")), m::KEY_WHOLE, "whole key");
    assert_refused(&composite(json!(["id"])), m::KEY_WHOLE, "whole key");
    assert_refused(&composite(json!(["id", "part"])), m::KEY_WHOLE, "whole key");
    let files = generate("spec.jsonc", &composite(json!(["part", "id"])), &options())
        .unwrap_or_else(|d| panic!("refused: {d:?}"));
    let code = &files
        .iter()
        .find(|f| f.path == "src/machines.rs")
        .unwrap()
        .contents;
    assert!(code.contains("pub type Key = ("), "{code}");
}

#[test]
fn variables_need_a_same_named_field_of_their_type() {
    let text = with(|s| {
        machine(s)["variables"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "name": "extra", "type": "number", "initial": 0, "repr": "u32" }));
    });
    assert_refused(&text, m::VARIABLE_UNMAPPED, "extra");
    let text = with(|s| machine(s)["variables"][0]["repr"] = json!("u64"));
    assert_refused(&text, m::FIELD_TYPE, "count");
}

#[test]
fn storage_fields_must_name_a_variable_and_a_field() {
    let text = with(|s| machine(s)["storage"]["fields"] = json!({ "count": "nope" }));
    assert_refused(&text, m::FIELD_UNKNOWN, "nope");
    let text = with(|s| machine(s)["storage"]["fields"] = json!({ "nope": "count" }));
    assert_refused(&text, m::FIELD_UNKNOWN, "nope");
    let text = with(|s| machine(s)["storage"]["fields"] = json!({ "count": "id" }));
    assert_refused(&text, m::FIELD_UNKNOWN, "the key");
    // A mapped variable is held in the field it names.
    let text = with(|s| {
        s["records"][0]["fields"][2]["name"] = json!("total");
        machine(s)["storage"]["fields"] = json!({ "count": "total" });
    });
    assert!(generate("spec.jsonc", &text, &options()).is_ok());
}

#[test]
fn the_state_needs_a_string_field() {
    let text = with(|s| {
        s["records"][0]["fields"]
            .as_array_mut()
            .unwrap()
            .retain(|f| f["name"] != "state");
    });
    assert_refused(&text, m::STATE_FIELD, "no field \"state\"");
    let text = with(|s| s["records"][0]["fields"][1]["type"] = json!("u32"));
    assert_refused(&text, m::STATE_FIELD, "u32");
    // An enum field must cover every state, through stateValues when its values differ.
    let text = with(|s| {
        s["types"] =
            json!([{ "id": "phase", "name": "Phase", "kind": "enum", "values": ["x", "y"] }]);
        s["records"][0]["fields"][1]["type"] = json!("phase");
    });
    assert_refused(&text, m::STATE_FIELD, "map the states");
    let text = with(|s| {
        s["types"] =
            json!([{ "id": "phase", "name": "Phase", "kind": "enum", "values": ["x", "y"] }]);
        s["records"][0]["fields"][1]["type"] = json!("phase");
        machine(s)["storage"]["stateValues"] = json!({ "a": "x", "b": "y" });
    });
    assert!(generate("spec.jsonc", &text, &options()).is_ok());
}

#[test]
fn required_fields_a_creating_machine_does_not_own_are_refused() {
    let text = with(|s| {
        s["records"][0]["fields"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "name": "owner", "type": "string" }));
        machine(s)["storage"]["creates"] = json!(true);
    });
    assert_refused(&text, m::CREATES_UNFILLED, "owner");
    // A machine that does not create records carries them over from the existing record.
    let text = with(|s| {
        s["records"][0]["fields"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "name": "owner", "type": "string" }));
    });
    let files = generate("spec.jsonc", &text, &options()).unwrap();
    let code = &files
        .iter()
        .find(|f| f.path == "src/machines.rs")
        .unwrap()
        .contents;
    assert!(code.contains("const CREATES: bool = false;"));
    assert!(code.contains("owner: previous.owner.clone()"));
    // An optional one a creating machine carries over when there is a previous record.
    let text = with(|s| {
        s["records"][0]["fields"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "name": "owner", "type": "string", "optional": true }));
        machine(s)["storage"]["creates"] = json!(true);
    });
    assert!(generate("spec.jsonc", &text, &options()).is_ok());
}

#[test]
fn unknown_names_are_refused() {
    let text = with(|s| machine(s)["transitions"][1]["on"] = json!("ev-nope"));
    assert_refused(&text, m::EVENT_UNKNOWN, "ev-nope");
    let text = with(|s| machine(s)["transitions"][1]["to"] = json!("z"));
    assert_refused(&text, m::STATE_UNKNOWN, "\"z\"");
    let text = with(|s| machine(s)["initial"] = json!("z"));
    assert_refused(&text, m::STATE_UNKNOWN, "initial");
    let text = with(|s| effects(s)[0]["set"] = json!("nope"));
    assert_refused(&text, m::VARIABLE_UNKNOWN, "nope");
}

#[test]
fn literals_must_fit_their_type() {
    let text = with(|s| machine(s)["variables"][0]["initial"] = json!("many"));
    assert_refused(&text, m::LITERAL, "count");
    let text = with(|s| machine(s)["variables"][0]["initial"] = json!(-1));
    assert_refused(&text, m::LITERAL, "count");
    let text = with(|s| effects(s)[0]["to"] = json!(true));
    assert_refused(&text, m::LITERAL, "count");
}

/// The generated type of an id refuses to decode a value that breaks its pattern, so an initial
/// value that breaks it would be one the machine writes and cannot read back.
#[test]
fn an_initial_value_must_meet_the_constraints_of_its_type() {
    let constrained = |initial: Value| {
        with(|s| {
            s["types"] = json!([
                { "id": "who", "name": "Who", "kind": "id", "repr": "string",
                  "constraints": { "pattern": "[a-z]+", "maxLength": 4 } }
            ]);
            s["records"][0]["fields"]
                .as_array_mut()
                .unwrap()
                .push(json!({ "name": "owner", "type": "who" }));
            machine(s)["variables"].as_array_mut().unwrap().push(
                json!({ "name": "owner", "type": "string", "repr": "who", "initial": initial }),
            );
        })
    };
    assert_refused(&constrained(json!("")), m::LITERAL, "pattern");
    assert_refused(&constrained(json!("abcde")), m::LITERAL, "maxLength");
    assert!(generate("spec.jsonc", &constrained(json!("abc")), &options()).is_ok());
}

#[test]
fn payloads_must_match_their_event() {
    let text = with(|s| effects(s)[2]["payload"] = json!({ "n": "count", "m": "count" }));
    assert_refused(&text, m::PAYLOAD_FIELD, "\"m\"");
    let text = with(|s| effects(s)[2]["payload"] = json!({}));
    assert_refused(&text, m::PAYLOAD_FIELD, "required");
}

#[test]
fn exegra_readiness_gaps_are_refused_with_exegra_codes() {
    let text = with(|s| {
        machine(s)["variables"][0]
            .as_object_mut()
            .unwrap()
            .remove("repr");
    });
    assert_refused(&text, "ready.repr-missing", "count");
    let text = with(|s| machine(s)["variables"][0]["repr"] = json!("u33"));
    assert_refused(&text, "ready.repr-unresolved", "u33");
    let text = with(|s| {
        machine(s).as_object_mut().unwrap().remove("transaction");
    });
    assert_refused(&text, "ready.transaction-missing", "transaction");
    let text = with(|s| {
        s["events"][1].as_object_mut().unwrap().remove("delivery");
    });
    assert_refused(&text, "ready.delivery-missing", "ev-out");
    let text = with(|s| s["events"][0]["payload"][0]["type"] = json!("int"));
    assert_refused(&text, "ready.payload-type-unresolved", "int");
    let text = with(|s| machine(s)["transitions"][1]["faultOnly"] = json!(true));
    assert_refused(&text, "ready.fault-only", "t2");
}

#[test]
fn a_machine_that_receives_nothing_is_refused() {
    let text = with(|s| {
        let b = machine(s);
        b["transitions"] = json!([]);
        b["states"][0]["ignores"] = json!([]);
    });
    assert_refused(&text, m::NO_EVENTS, "receives no event");
}

#[test]
fn model_level_behaviors_are_not_generated() {
    let text = with(|s| {
        machine(s)["level"] = json!("model");
        machine(s)["transitions"][0]["guard"] = json!("other.count > 0");
    });
    let files = generate("spec.jsonc", &text, &options()).unwrap();
    assert!(!files.iter().any(|f| f.path == "src/machines.rs"));
}

#[test]
fn an_omitted_option_payload_field_is_none() {
    let mut spec = base();
    spec["events"][1]["payload"] = json!([
        { "name": "n", "type": "u32" },
        { "name": "note", "type": "option<string>" }
    ]);
    let text = serde_json::to_string(&spec).unwrap();
    let files = generate("spec.jsonc", &text, &options()).unwrap();
    let machines = &files
        .iter()
        .find(|f| f.path == "src/machines.rs")
        .unwrap()
        .contents;
    assert!(machines.contains("note: None"), "{machines}");
}

/// The base with a scenario that runs machine `m` and takes `steps`.
fn with_scenario(steps: Value) -> String {
    with(|s| {
        s["scenarios"] = json!([
            { "id": "run", "name": "Run", "description": "Runs the machine.",
              "behaviors": ["m"], "timeUnit": "ms", "steps": steps }
        ]);
    })
}

/// Negative fixture of `event.elapsed-external` (WG-4948): the base specification plus one
/// scenario step that injects the elapsed event `ev-tick` into machine `m` from outside. That is
/// the only thing missing from a specification that generates.
#[test]
fn an_elapsed_event_injected_from_outside_is_refused() {
    let ok = with_scenario(json!([ { "at": 0, "event": "ev-go", "payload": { "n": 1 } } ]));
    generate("spec.jsonc", &ok, &options()).expect("the control scenario generates");

    let text = with_scenario(json!([
        { "at": 0, "event": "ev-go", "payload": { "n": 1 } },
        { "at": 1, "event": "ev-tick" }
    ]));
    assert_refused(&text, m::ELAPSED_EXTERNAL, "ev-tick");
    assert_eq!(m::ELAPSED_EXTERNAL, "event.elapsed-external");
}

/// The generated machine says which of its events are elapsed time, and validates its record.
#[test]
fn the_generated_machine_marks_elapsed_events_and_validates_records() {
    let files = generate("spec.jsonc", &with(|_| {}), &options()).unwrap();
    let code = &files
        .iter()
        .find(|f| f.path == "src/machines.rs")
        .unwrap()
        .contents;
    assert!(code.contains("fn is_elapsed(event: &Event) -> bool"));
    assert!(code.contains("matches!(event, Event::EvTick(_))"), "{code}");
    assert!(code.contains("record.validate()"));
}
