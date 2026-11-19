//! The system host against Exegra's simulator, scenario by scenario.
//!
//! For every scenario of `specifications/spec.jsonc` that drives implementation-level machines,
//! this test asks Exegra for the trace (`node $EXEGRA_HOME/cli/exegra.js trace <scenario>`,
//! through a temporary project link to this checkout's specification) and runs the same scenario
//! on `strata_runtime::System` with the generated machines and routing table
//! (`strata_model::target_machines::dispatch`, which holds shipped and target machines), on the in-memory store, with a `TestClock`:
//!
//! - The host gets the scenario's machines only, as Exegra runs only the scenario's behaviours.
//! - A machine that does not create its records (`storage.creates` false) steps records someone
//!   else made. Its instances are set up first (`System::init`) in their initial state, over a
//!   record whose other fields hold placeholder values of their types: the one instance of an
//!   unkeyed machine under a placeholder key, and each key the scenario's inputs route to for a
//!   keyed one. That stands for the contract that creates the record, which Exegra leaves out.
//! - The external entries (the scenario's steps and injected events: Exegra's initial queue) are
//!   submitted at their times before the run, in Exegra's order, so they take the lowest sequence
//!   numbers as in Exegra.
//! - Then, for each step of the trace, the clock is set to the time of the next due entry, which
//!   must be the step's time, and the host takes one step. After it the test compares: the entry
//!   delivered (event and payload), the state and variables of every instance Exegra has, and the
//!   whole queue in order (event, payload and due time: the emits with their times and the timers
//!   with their deadlines). An instance the host has and Exegra has not yet created must still be
//!   as it was set up. A run Exegra ends quiescent must leave the host's queue empty.
//!
//! Exegra's time is read as the host's milliseconds, one base unit for one millisecond, from 0;
//! the generated machines read delays and `time` the same way.
//!
//! Faults are produced as the runtime produces them (WG-4903, Exegra's `docs/semantics.md`,
//! "Faults the runtime can produce"). Inside one system host every entry is delivered exactly
//! once, so a fault acts on the hop by which an entry reaches the host: an input, sent by the
//! outside or by another host's outbox through a dispatcher that submits it here. The test is
//! that dispatcher. An input Exegra drops or loses (`drop-event`, `lose-event`) is never
//! submitted; one it delays (`delay-event`, `retry-event`) is submitted when Exegra queues the
//! delayed copy, due when the copy is; one it duplicates (`duplicate-event`) is submitted again
//! when Exegra queues the copy, after the step that delivered the first one, as a sender's outbox
//! offers an at-least-once message again after a stop between dispatch and acknowledgement. An
//! injected event (`inject-event`) is one more input.
//!
//! Skipped, each with its reason in the output: scenarios with a fault the runtime cannot produce
//! (Exegra's `fault.unrealisable`, mirrored by `strata_spec_compiler::scenarios::unrealisable`),
//! a fault that changes a machine (`enable-transition`, `disable-transition`), or a fault on an
//! event a scenario machine emits or a timer fires (the host's own queue delivers those exactly
//! once; the fault needs machines on two hosts); scenarios with a behaviour that has no generated
//! Rust machine (a model behaviour, or a client machine generated as JavaScript), and scenarios
//! whose inputs the generated types cannot hold (a payload that does not decode as the machine's
//! event, or a key that does not decode as its key type). The last are gaps in the specification,
//! listed as such.
//!
//! The test fails when `EXEGRA_HOME` is not an Exegra checkout or `node` cannot run, when a trace
//! cannot be had, when any covered scenario diverges, and when no scenario is covered.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde_json::{json, Value};
// The routing table of the whole system, shipped machines and target machines together (the
// `target-models` feature of `strata-model`, which this crate's manifest enables).
use strata_model::target_machines::dispatch;
use strata_runtime::{
    DispatchError, Dispatcher, Envelope, Member, MemoryStore, Router, System, TenantId, TestClock,
    Time,
};
use strata_spec_compiler::ir::{Spec, TypeKind};
use strata_spec_compiler::resolve::{Builtin, TypeRef};
use strata_spec_compiler::{jsonc, load};

/// The project name of the temporary link.
const PROJECT: &str = "strata-under-test";

/// The fault kinds the host can be given, on its inputs: an injected event is one more input; a
/// dropped, lost, delayed, retried or duplicated one is what the dispatcher that submits it does.
const HOST_FAULTS: &[&str] = &[
    "inject-event",
    "drop-event",
    "lose-event",
    "delay-event",
    "retry-event",
    "duplicate-event",
];

fn workspace() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    std::path::absolute(dir.join("..").join(".."))
        .unwrap()
        .components()
        .collect()
}

fn spec_path() -> PathBuf {
    workspace().join("specifications").join("spec.jsonc")
}

/// `to` relative to the directory `from`, with forward slashes.
fn relative(from: &Path, to: &Path) -> String {
    let a: Vec<Component> = from.components().collect();
    let b: Vec<Component> = to.components().collect();
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    assert!(
        common > 0 && matches!(a[0], Component::Prefix(_) | Component::RootDir),
        "{} and {} share no root, so the project link cannot name the specification relatively",
        from.display(),
        to.display()
    );
    let mut parts: Vec<String> = vec!["..".to_string(); a.len() - common];
    parts.extend(
        b[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}

/// The Exegra CLI, or a failure saying how to get one.
struct Exegra {
    cli: PathBuf,
    specs: PathBuf,
}

impl Exegra {
    fn new() -> Self {
        let home = std::env::var_os("EXEGRA_HOME").unwrap_or_else(|| {
            panic!("set EXEGRA_HOME to an Exegra checkout (one with cli/exegra.js): this test compares the system host with Exegra's traces")
        });
        let cli = PathBuf::from(home).join("cli").join("exegra.js");
        assert!(
            cli.is_file(),
            "EXEGRA_HOME has no cli/exegra.js: {}",
            cli.display()
        );
        let version = Command::new("node").arg("--version").output();
        assert!(
            version.as_ref().is_ok_and(|o| o.status.success()),
            "node cannot be run: {version:?}"
        );
        let specs = std::path::absolute(PathBuf::from(env!("CARGO_TARGET_TMPDIR")))
            .unwrap()
            .join(format!("exegra-specs-{}", std::process::id()));
        let project = specs.join(PROJECT);
        std::fs::create_dir_all(&project).unwrap();
        let link = json!({ "spec": relative(&project, &spec_path()) });
        std::fs::write(project.join("project.jsonc"), link.to_string()).unwrap();
        Exegra { cli, specs }
    }

    fn trace(&self, scenario: &str) -> Value {
        let out = self.specs.join(format!("{scenario}.trace.json"));
        let output = Command::new("node")
            .arg(&self.cli)
            .args(["trace", scenario, "--project", PROJECT, "--specs"])
            .arg(&self.specs)
            .arg("--out")
            .arg(&out)
            .output()
            .unwrap_or_else(|e| panic!("node could not be run: {e}"));
        let envelope: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
            panic!(
                "exegra trace {scenario} did not print JSON ({e}): {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        assert_eq!(
            envelope["ok"],
            json!(true),
            "exegra trace {scenario} failed: {envelope}"
        );
        let text = std::fs::read_to_string(&out).unwrap();
        serde_json::from_str(&text).unwrap()
    }
}

impl Drop for Exegra {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.specs);
    }
}

/// Every dispatch to a receiver outside the machines succeeds; the test looks at the queue.
struct Outside;

impl Dispatcher<Value> for Outside {
    async fn dispatch(&self, _: &Envelope<Value>) -> Result<(), DispatchError> {
        Ok(())
    }
}

/// Two JSON values are the same, numbers compared by value (`1` and `1.0` are one number).
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

fn payload_of(v: &Value) -> Value {
    match v.get("payload") {
        Some(p) if !p.is_null() => p.clone(),
        _ => json!({}),
    }
}

fn time_of(v: &Value) -> i64 {
    v.as_i64()
        .or_else(|| v.as_f64().map(|f| f as i64))
        .unwrap_or_else(|| panic!("{v} is not a time"))
}

/// A value of a type, for the record fields a machine does not hold.
fn placeholder(spec: &Spec, t: &TypeRef) -> Value {
    match t {
        TypeRef::Builtin(b) => match b {
            Builtin::Bool => json!(false),
            Builtin::F64 => json!(0.0),
            Builtin::String => json!("x"),
            Builtin::Bytes => json!([]),
            Builtin::Uuid => json!("00000000-0000-0000-0000-000000000000"),
            Builtin::Json => Value::Null,
            _ => json!(0),
        },
        TypeRef::List(_) | TypeRef::Set(_) => json!([]),
        TypeRef::Option(_) => Value::Null,
        TypeRef::Shared(inner) => placeholder(spec, inner),
        TypeRef::Map(..) => json!({}),
        TypeRef::Declared(id) => match spec.type_decl(id).map(|d| &d.kind) {
            Some(TypeKind::Id { repr } | TypeKind::Scalar { repr }) => {
                placeholder(spec, &TypeRef::parse(repr.as_deref().unwrap_or("string")))
            }
            Some(TypeKind::Enum { values }) => json!(values.first().cloned().unwrap_or_default()),
            Some(TypeKind::Struct { fields }) => Value::Object(
                fields
                    .iter()
                    .filter(|f| !f.optional)
                    .map(|f| {
                        (
                            f.name.clone(),
                            placeholder(spec, &TypeRef::parse(&f.type_name)),
                        )
                    })
                    .collect(),
            ),
            // No record field holds a union yet; its wire form depends on its encoding.
            Some(TypeKind::Union { .. }) | None => Value::Null,
        },
    }
}

/// What a scenario needs of the generated code, or why it cannot run on the host.
struct Plan<'s> {
    id: String,
    members: Vec<Member>,
    behaviours: Vec<&'s strata_spec_compiler::ir::Behavior>,
}

fn plan<'s>(spec: &'s Spec, all: &Value, scenario: &Value) -> Result<Plan<'s>, String> {
    let id = scenario["id"].as_str().unwrap_or_default().to_string();
    let faults = all["faults"].as_array().cloned().unwrap_or_default();
    let top = all.as_object().cloned().unwrap_or_default();
    let raw = scenario.as_object().cloned().unwrap_or_default();
    // Events a scenario machine emits or a timer of one fires: the host's queue carries them.
    let mut internal: BTreeSet<String> = BTreeSet::new();
    for b in all["behaviors"].as_array().into_iter().flatten() {
        let ran = scenario["behaviors"]
            .as_array()
            .is_some_and(|bs| bs.iter().any(|x| *x == b["id"]));
        if !ran {
            continue;
        }
        for t in b["transitions"].as_array().into_iter().flatten() {
            for fx in t["effects"].as_array().into_iter().flatten() {
                if let Some(e) = fx["emit"].as_str() {
                    internal.insert(e.to_string());
                }
            }
        }
    }
    for f in scenario["faults"].as_array().into_iter().flatten() {
        let fid = f.as_str().unwrap_or_default();
        let declared = faults.iter().find(|x| x["id"] == fid);
        let kind = declared
            .and_then(|x| x["kind"].as_str())
            .unwrap_or("unknown");
        if !HOST_FAULTS.contains(&kind) {
            return Err(format!(
                "fault {fid} ({kind}) changes a machine, which the runtime cannot do"
            ));
        }
        let declared = declared.and_then(Value::as_object);
        if let Some(why) =
            declared.and_then(|d| strata_spec_compiler::scenarios::unrealisable(&top, &raw, d))
        {
            return Err(format!(
                "fault {fid} ({kind}) is one the runtime cannot produce: {why}"
            ));
        }
        let event = declared
            .and_then(|d| d.get("event"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if kind != "inject-event" && internal.contains(event) {
            return Err(format!(
                "fault {fid} ({kind}) acts on {event}, which a scenario machine emits: one system host delivers it exactly once, and the fault needs the machines on two hosts"
            ));
        }
    }
    let generated = dispatch::members();
    let mut members = Vec::new();
    let mut behaviours = Vec::new();
    for b in scenario["behaviors"].as_array().into_iter().flatten() {
        let b = b.as_str().unwrap_or_default();
        let (Some(member), Some(ir)) = (
            generated.iter().find(|m| m.name == b),
            spec.all_machines().into_iter().find(|m| m.id == b),
        ) else {
            return Err(format!(
                "behaviour {b} has no generated Rust machine (a model behaviour or a client machine)"
            ));
        };
        members.push(*member);
        behaviours.push(ir);
    }
    Ok(Plan {
        id,
        members,
        behaviours,
    })
}

/// The instance an Exegra machine key names: `<behaviour>` for an unkeyed machine, or
/// `<behaviour>#<key as JSON>`.
fn host_instance(
    router: &Router,
    singles: &BTreeMap<String, String>,
    exegra_key: &str,
) -> Result<(String, String), String> {
    if let Some((behaviour, key)) = exegra_key.split_once('#') {
        let member = router.member(behaviour).ok_or_else(|| {
            format!("Exegra has an instance of {behaviour}, not a scenario machine")
        })?;
        let value: Value = serde_json::from_str(key).map_err(|e| e.to_string())?;
        let key = member
            .key_of(&value)
            .map_err(|e| format!("key {key} of {behaviour}: {e}"))?;
        return Ok((behaviour.to_string(), key));
    }
    let key = singles
        .get(exegra_key)
        .ok_or_else(|| format!("Exegra has machine {exegra_key}, which the host does not run"))?;
    Ok((exegra_key.to_string(), key.clone()))
}

fn describe(entry: &(i64, String, Value)) -> String {
    format!("{}@{} {}", entry.1, entry.0, entry.2)
}

/// Run one scenario on the host against its trace. `Ok(n)`: `n` steps agreed. `Err` with
/// `skip: true` when the inputs cannot be held by the generated types.
async fn run(spec: &Spec, plan: &Plan<'_>, trace: &Value) -> Result<usize, (bool, String)> {
    let diverged = |m: String| (false, m);
    let router = Router::new(plan.members.clone(), dispatch::ROUTES);
    let inputs: Vec<&Value> = trace["initial"]["pending"]
        .as_array()
        .map(|p| p.iter().collect())
        .unwrap_or_default();

    // The inputs must be what the generated machines read.
    let mut keyed: Vec<(Member, Value)> = Vec::new();
    for input in &inputs {
        let event = json!({ "event": input["event"], "payload": payload_of(input) });
        let id = input["event"].as_str().unwrap_or_default();
        for routing in router.routes(id) {
            let Some(member) = router.member(routing.machine) else {
                continue;
            };
            if let Err(e) = member.accepts(&event) {
                return Err((
                    true,
                    format!(
                        "input {id} {} is not an event of {}: {e}",
                        payload_of(input),
                        member.name
                    ),
                ));
            }
            if let Some(field) = routing.key {
                let value = payload_of(input).get(field).cloned().unwrap_or(Value::Null);
                if let Err(e) = member.key_of(&value) {
                    return Err((
                        true,
                        format!(
                            "input {id} routes to {} by {field} = {value}, which is not its key type: {e}",
                            member.name
                        ),
                    ));
                }
                if !keyed
                    .iter()
                    .any(|(m, v)| m.name == member.name && *v == value)
                {
                    keyed.push((*member, value));
                }
            }
        }
    }

    let clock = TestClock::at(Time(0));
    let store = Arc::new(MemoryStore::new(TenantId::new("acme").unwrap()));
    let system = System::new(
        store,
        router.clone(),
        Arc::new(Outside),
        Arc::new(clock.clone()),
    )
    .map_err(|e| diverged(e.to_string()))?;

    // Set up the instances whose records someone else creates.
    let mut singles: BTreeMap<String, String> = BTreeMap::new();
    let mut seeded: BTreeMap<(String, String), (Value, Value)> = BTreeMap::new();
    for (member, b) in plan.members.iter().zip(&plan.behaviours) {
        let storage = b.storage.as_ref().expect("a generated machine has storage");
        let record = spec
            .record(member.record)
            .unwrap_or_else(|| panic!("no record {}", member.record));
        // The key fields: one, or several for a record with a composite key (a JSON array).
        let key_fields = storage
            .key
            .clone()
            .or_else(|| b.instances.as_ref().and_then(|i| i.key.clone()))
            .expect("a generated machine has a key");
        let key_types: Vec<TypeRef> = key_fields
            .iter()
            .map(|k| {
                record
                    .field(k)
                    .map(|f| TypeRef::parse(&f.type_name))
                    .expect("the key is a record field")
            })
            .collect();
        let keys: Vec<Value> = if b.instances.is_some() {
            keyed
                .iter()
                .filter(|(m, _)| m.name == member.name)
                .map(|(_, v)| v.clone())
                .collect()
        } else {
            let key = if let [one] = key_types.as_slice() {
                placeholder(spec, one)
            } else {
                Value::Array(key_types.iter().map(|t| placeholder(spec, t)).collect())
            };
            let text = member.key_of(&key).map_err(|e| diverged(e.to_string()))?;
            singles.insert(member.name.to_string(), text);
            vec![key]
        };
        if member.creates {
            continue;
        }
        for key in keys {
            let text = member.key_of(&key).map_err(|e| diverged(e.to_string()))?;
            let mut base = serde_json::Map::new();
            for f in record.fields.iter().filter(|f| !f.optional) {
                base.insert(
                    f.name.clone(),
                    placeholder(spec, &TypeRef::parse(&f.type_name)),
                );
            }
            if let [one] = key_fields.as_slice() {
                base.insert(one.clone(), key.clone());
            } else if let Value::Array(parts) = &key {
                for (field, part) in key_fields.iter().zip(parts) {
                    base.insert(field.clone(), part.clone());
                }
            }
            system
                .init(member.name, &text, Value::Object(base))
                .await
                .map_err(|e| match e {
                    // Exegra starts the instance in a state whose record the runtime refuses to
                    // hold: the specification's initial values break the record's constraints.
                    // That is a fidelity gap of the specification (WG-4903,
                    // machine.initial-unreachable), reported as such, never hidden by skipping
                    // the runtime's validation.
                    strata_runtime::HostError::Constraint(why) => (
                        true,
                        format!(
                            "the initial state of {} (instance {text}) cannot be held by its \
                             record: {why}",
                            member.name
                        ),
                    ),
                    e => diverged(format!(
                        "setting up instance {text} of {}: {e}",
                        member.name
                    )),
                })?;
        }
    }
    for member in &plan.members {
        for key in system
            .keys(member.name)
            .await
            .map_err(|e| diverged(e.to_string()))?
        {
            if let Some(view) = system
                .instance(member.name, &key)
                .await
                .map_err(|e| diverged(e.to_string()))?
            {
                seeded.insert((member.name.to_string(), key), view);
            }
        }
    }

    // Compare the instances of a configuration.
    let instances = |config: &Value| -> Result<BTreeMap<(String, String), (Value, Value)>, String> {
        let mut out = BTreeMap::new();
        for (key, m) in config["machines"].as_object().into_iter().flatten() {
            let at = host_instance(&router, &singles, key)?;
            out.insert(at, (m["state"].clone(), m["vars"].clone()));
        }
        Ok(out)
    };
    let check = |context: &str,
                 expected: BTreeMap<(String, String), (Value, Value)>,
                 actual: BTreeMap<(String, String), Option<(Value, Value)>>|
     -> Result<(), String> {
        for (at, (state, vars)) in &expected {
            match actual.get(at).cloned().flatten() {
                None => {
                    return Err(format!(
                        "{context}: Exegra has {} {} in {state}, the host has no instance",
                        at.0, at.1
                    ))
                }
                Some((s, v)) => {
                    if !same(&s, state) || !same(&v, vars) {
                        return Err(format!(
                            "{context}: {} {}: Exegra {state} {vars}, host {s} {v}",
                            at.0, at.1
                        ));
                    }
                }
            }
        }
        for (at, view) in &actual {
            let Some(view) = view else { continue };
            if expected.contains_key(at) {
                continue;
            }
            if seeded.get(at) != Some(view) {
                return Err(format!(
                    "{context}: the host has {} {} in {} {}, which Exegra does not have",
                    at.0, at.1, view.0, view.1
                ));
            }
        }
        Ok(())
    };
    // The host's instances: every readable record of a scenario machine, the unkeyed ones, and
    // every instance Exegra has (a creating machine's instance with no record is in its initial
    // state, as Exegra creates it).
    let (sys, members, single_keys) = (&system, &plan.members, &singles);
    let host_instances = |expected: Vec<(String, String)>| async move {
        let mut out: BTreeMap<(String, String), Option<(Value, Value)>> = BTreeMap::new();
        for (machine, key) in expected {
            let view = sys
                .instance(&machine, &key)
                .await
                .map_err(|e| e.to_string())?;
            out.insert((machine, key), view);
        }
        for member in members {
            let mut keys = sys.keys(member.name).await.map_err(|e| e.to_string())?;
            keys.extend(single_keys.get(member.name).cloned());
            for key in keys {
                let at = (member.name.to_string(), key);
                if out.contains_key(&at) {
                    continue;
                }
                let view = sys
                    .instance(member.name, &at.1)
                    .await
                    .or_else(|e| match e {
                        // Another machine's record in a shared collection.
                        strata_runtime::HostError::Record(_) => Ok(None),
                        other => Err(other.to_string()),
                    })?;
                out.insert(at, view);
            }
        }
        Ok::<_, String>(out)
    };

    let expected = instances(&trace["initial"]).map_err(diverged)?;
    let actual = host_instances(expected.keys().cloned().collect())
        .await
        .map_err(diverged)?;
    check("initially", expected, actual).map_err(diverged)?;

    let steps = trace["steps"].as_array().cloned().unwrap_or_default();
    // What Exegra does with each entry it takes, by sequence number: the kind of that step. The
    // dispatcher in front of the host (this test) submits an input only when Exegra delivers it,
    // or never takes it.
    let fate: BTreeMap<u64, String> = steps
        .iter()
        .filter_map(|s| Some((s["event"]["seq"].as_u64()?, s["kind"].as_str()?.to_string())))
        .collect();
    let reaches_host = |entry: &Value| {
        let taken = entry["seq"].as_u64().and_then(|s| fate.get(&s));
        taken.is_none_or(|k| k == "deliver")
    };

    // Exegra's initial queue: the external entries, in its order, but those Exegra drops or
    // delays before delivering them.
    for input in inputs.iter().filter(|i| reaches_host(i)) {
        let event = json!({ "event": input["event"], "payload": payload_of(input) });
        system
            .submit_at(event, Time(time_of(&input["time"])))
            .await
            .map_err(|e| diverged(e.to_string()))?;
    }

    // The copies of an input Exegra queued in a step: a delayed, retried or duplicated input,
    // which the dispatcher submits when Exegra queues it, due when it is.
    let copies = |before: &[Value], step: &Value| -> Vec<Value> {
        let head = &step["event"];
        step["config"]["pending"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| !before.iter().any(|b| b["seq"] == p["seq"]))
            .filter(|p| p["event"] == head["id"] && p["source"] == head["source"])
            .filter(|p| reaches_host(p))
            .cloned()
            .collect()
    };
    let mut before: Vec<Value> = inputs.iter().map(|i| (*i).clone()).collect();
    for step in &steps {
        let index = &step["index"];
        let time = time_of(&step["time"]);
        let id = step["event"]["id"].as_str().unwrap_or_default();
        let context = format!("step {index} (t={time}, {id})");
        let kind = step["kind"].as_str().unwrap_or_default();
        let sys = &system;
        let submit = |copies: Vec<Value>| async move {
            for copy in copies {
                let event = json!({ "event": copy["event"], "payload": payload_of(&copy) });
                sys.submit_at(event, Time(time_of(&copy["time"])))
                    .await
                    .map_err(|e| e.to_string())?;
            }
            Ok::<(), String>(())
        };
        if kind == "drop" || kind == "delay" {
            // Exegra's fault acted on an input on its way to the host: nothing reaches the host
            // now, and a delayed input is submitted for its later time.
            let source = step["event"]["source"]["kind"].as_str();
            if !matches!(source, Some("scenario" | "fault")) {
                return Err(diverged(format!(
                    "{context}: Exegra's {kind} acts on an entry the machines queued, which one host delivers exactly once"
                )));
            }
            if kind == "delay" {
                let delayed = copies(&before, step);
                if delayed.is_empty() && step["error"].is_null() {
                    return Err(diverged(format!(
                        "{context}: Exegra delayed the input, but its trace has no delayed copy"
                    )));
                }
                submit(delayed)
                    .await
                    .map_err(|e| diverged(format!("{context}: {e}")))?;
            }
            before = step["config"]["pending"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            continue;
        }
        if kind != "deliver" {
            return Err(diverged(format!(
                "{context}: Exegra's step is a {kind}, which the host does not make"
            )));
        }
        let due = system
            .next_due()
            .await
            .map_err(|e| diverged(e.to_string()))?
            .ok_or_else(|| diverged(format!("{context}: the host's queue is empty")))?;
        if due.0 != time {
            return Err(diverged(format!(
                "{context}: the host's next entry is due at {}",
                due.0
            )));
        }
        clock.set(due);
        let report = system
            .step()
            .await
            .map_err(|e| diverged(format!("{context}: {e}")))?
            .ok_or_else(|| diverged(format!("{context}: the host took no step")))?;
        let delivered: Value = serde_json::from_str(&report.entry.event).unwrap();
        if delivered["event"] != step["event"]["id"]
            || !same(&payload_of(&delivered), &payload_of(&step["event"]))
        {
            return Err(diverged(format!(
                "{context}: the host delivered {delivered}"
            )));
        }
        let exegra_failed = step.get("error").filter(|e| !e.is_null());
        match (&report.failed, exegra_failed) {
            (Some(why), None) => {
                return Err(diverged(format!(
                    "{context}: the host's step failed: {why}"
                )))
            }
            (None, Some(why)) => {
                return Err(diverged(format!(
                    "{context}: Exegra's step failed ({why}), the host's did not"
                )))
            }
            (Some(_), Some(_)) => return Ok(steps.len()),
            (None, None) => {}
        }
        let expected = instances(&step["config"]).map_err(diverged)?;
        let actual = host_instances(expected.keys().cloned().collect())
            .await
            .map_err(diverged)?;
        check(&context, expected, actual).map_err(diverged)?;
        // A duplicated input: the sender's outbox offers it again, after this delivery.
        let duplicated = step["faults"]
            .as_array()
            .is_some_and(|f| f.iter().any(|f| f["effect"] == json!("duplicated")));
        if duplicated {
            submit(copies(&before, step))
                .await
                .map_err(|e| diverged(format!("{context}: {e}")))?;
        }
        let expected: Vec<(i64, String, Value)> = step["config"]["pending"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| {
                (
                    time_of(&p["time"]),
                    p["event"].as_str().unwrap_or_default().to_string(),
                    payload_of(p),
                )
            })
            .collect();
        let queued: Vec<(i64, String, Value)> = system
            .queued(usize::MAX)
            .await
            .map_err(|e| diverged(e.to_string()))?
            .into_iter()
            .map(|e| {
                let v: Value = serde_json::from_str(&e.event).unwrap();
                (
                    e.due.0,
                    v["event"].as_str().unwrap_or_default().to_string(),
                    payload_of(&v),
                )
            })
            .collect();
        let agree = expected.len() == queued.len()
            && expected
                .iter()
                .zip(&queued)
                .all(|(a, b)| a.0 == b.0 && a.1 == b.1 && same(&a.2, &b.2));
        if !agree {
            return Err(diverged(format!(
                "{context}: queue\n  Exegra: [{}]\n  host:   [{}]",
                expected.iter().map(describe).collect::<Vec<_>>().join(", "),
                queued.iter().map(describe).collect::<Vec<_>>().join(", ")
            )));
        }
        before = step["config"]["pending"]
            .as_array()
            .cloned()
            .unwrap_or_default();
    }
    if trace["outcome"] == json!("quiescent") {
        if let Some(due) = system
            .next_due()
            .await
            .map_err(|e| diverged(e.to_string()))?
        {
            return Err(diverged(format!(
                "Exegra's run ends quiescent, the host still has an entry due at {}",
                due.0
            )));
        }
    }
    Ok(steps.len())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_system_host_runs_every_scenario_as_exegra_does() {
    let exegra = Exegra::new();
    let path = spec_path();
    let text = std::fs::read_to_string(&path).unwrap();
    let all = jsonc::parse(&text).unwrap();
    let spec = load::load("specifications/spec.jsonc", &text)
        .unwrap_or_else(|d| panic!("the specification does not load: {d:?}"));
    let machines: BTreeSet<&str> = spec
        .all_machines()
        .into_iter()
        .map(|m| m.id.as_str())
        .collect();
    let implementation: BTreeSet<&str> = all["behaviors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["level"] == json!("implementation"))
        .filter_map(|b| b["id"].as_str())
        .collect();

    let mut covered = Vec::new();
    let mut skipped = Vec::new();
    let mut unrepresentable = Vec::new();
    let mut diverged = Vec::new();
    for scenario in all["scenarios"].as_array().into_iter().flatten() {
        let drives = scenario["behaviors"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(|b| implementation.contains(b) || machines.contains(b));
        if !drives {
            continue;
        }
        let plan = match plan(&spec, &all, scenario) {
            Ok(plan) => plan,
            Err(why) => {
                skipped.push((scenario["id"].as_str().unwrap_or_default().to_string(), why));
                continue;
            }
        };
        let trace = exegra.trace(&plan.id);
        match run(&spec, &plan, &trace).await {
            Ok(steps) => covered.push((plan.id.clone(), steps)),
            Err((true, why)) => unrepresentable.push((plan.id.clone(), why)),
            Err((false, why)) => diverged.push((plan.id.clone(), why)),
        }
    }

    println!("covered ({}):", covered.len());
    for (id, steps) in &covered {
        println!("  {id}: {steps} steps agree");
    }
    println!("skipped ({}):", skipped.len());
    for (id, why) in &skipped {
        println!("  {id}: {why}");
    }
    println!(
        "skipped, inputs the generated types cannot hold ({}):",
        unrepresentable.len()
    );
    for (id, why) in &unrepresentable {
        println!("  {id}: {why}");
    }
    println!("diverged ({}):", diverged.len());
    for (id, why) in &diverged {
        println!("  {id}: {why}");
    }
    assert!(
        diverged.is_empty(),
        "{} scenario(s) run differently on the system host than in Exegra:\n{}",
        diverged.len(),
        diverged
            .iter()
            .map(|(id, why)| format!("- {id}: {why}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(!covered.is_empty(), "no scenario ran on the system host");
}
