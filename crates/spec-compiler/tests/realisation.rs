//! Target and memory: what specc generates for a behavior whose `realisation` is `target`, a
//! contract whose `deployment` is `target`, and a machine whose `storage.durability` is `memory`.
//!
//! - A target machine is written to `src/target_machines.rs`, which `lib.rs` compiles only with
//!   the crate's `target-models` feature; the shipped `src/machines.rs` does not hold it, and its
//!   `dispatch` does not route to it.
//! - A target contract has no route, no handler and no trait method.
//! - A memory machine has a pure `step` and no record conversion, key or transaction.
//! - A specification with none of them generates what it did before: no `target_machines.rs`, no
//!   `[features]`.
//!
//! Both defaults are from the component: a behavior's `subject` and a contract's `implementedBy`
//! with `implementation.status: "not-implemented"` make them target unless they say `shipped`.

use serde_json::{json, Value};
use strata_spec_compiler::{generate, jsonc, GeneratedFile, Options, RuntimeDependency};

fn options() -> Options {
    Options {
        crate_name: "generated".to_string(),
        runtime: RuntimeDependency::Path("../runtime".to_string()),
    }
}

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/{name}/spec.jsonc",
        env!("CARGO_MANIFEST_DIR")
    );
    jsonc::parse(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn files(spec: &Value) -> Vec<GeneratedFile> {
    match generate("spec.jsonc", &spec.to_string(), &options()) {
        Ok(files) => files,
        Err(d) => panic!(
            "the specification was refused:\n{}",
            d.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        ),
    }
}

fn file<'a>(files: &'a [GeneratedFile], path: &str) -> Option<&'a str> {
    files
        .iter()
        .find(|f| f.path == path)
        .map(|f| f.contents.as_str())
}

fn behavior<'a>(spec: &'a mut Value, id: &str) -> &'a mut Value {
    spec["behaviors"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|b| b["id"] == id)
        .unwrap()
}

fn contract<'a>(spec: &'a mut Value, id: &str) -> &'a mut Value {
    spec["contracts"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|c| c["id"] == id)
        .unwrap()
}

fn component<'a>(spec: &'a mut Value, id: &str) -> &'a mut Value {
    spec["components"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|c| c["id"] == id)
        .unwrap()
}

#[test]
fn a_specification_without_target_generates_no_target_module() {
    let out = files(&fixture("machines"));
    assert!(file(&out, "src/target_machines.rs").is_none());
    let lib = file(&out, "src/lib.rs").unwrap();
    assert!(!lib.contains("target_machines") && !lib.contains("target-models"));
    assert!(!file(&out, "Cargo.toml").unwrap().contains("[features]"));
    assert!(file(&out, "src/machines.rs")
        .unwrap()
        .contains("pub mod lease {"));
}

#[test]
fn a_declared_target_machine_leaves_the_shipped_module_and_enters_the_feature() {
    let mut spec = fixture("machines");
    behavior(&mut spec, "lease")["realisation"] = json!("target");
    let out = files(&spec);

    let shipped = file(&out, "src/machines.rs").unwrap();
    assert!(
        !shipped.contains("pub mod lease {"),
        "lease is still shipped"
    );
    assert!(shipped.contains("pub mod job {"));
    assert!(shipped.contains("pub mod hold {"));

    let target = file(&out, "src/target_machines.rs").unwrap();
    assert!(target.contains("pub mod lease {"));
    assert!(!target.contains("pub mod job {"));

    let lib = file(&out, "src/lib.rs").unwrap();
    assert!(lib.contains("#[cfg(feature = \"target-models\")]"));
    assert!(lib.contains("pub mod target_machines;"));
    let manifest = file(&out, "Cargo.toml").unwrap();
    assert!(manifest.contains("[features]") && manifest.contains("target-models = []"));
}

#[test]
fn the_target_dispatch_routes_the_whole_system_and_the_shipped_one_only_the_shipped() {
    let mut spec = fixture("machines");
    behavior(&mut spec, "lease")["realisation"] = json!("target");
    let out = files(&spec);
    let shipped = file(&out, "src/machines.rs").unwrap();
    let target = file(&out, "src/target_machines.rs").unwrap();
    // Every machine of the system is a member of the whole-system table, shipped ones through
    // `crate::machines`, target ones through `super`.
    assert!(target.contains("crate::machines::job::Job"));
    assert!(target.contains("super::lease::Lease"));
    // The shipped table names no target machine.
    assert!(!shipped.contains("lease::Lease"));
    assert!(!shipped.contains("Routing::every(\"lease\")"));
    assert!(!shipped.contains("Routing::by_key(\"lease\""));
}

#[test]
fn a_not_implemented_subject_makes_its_machines_target_unless_they_say_shipped() {
    let mut spec = fixture("machines");
    component(&mut spec, "lease-service")["implementation"] =
        json!({ "status": "not-implemented" });
    behavior(&mut spec, "job")["realisation"] = json!("shipped");
    let out = files(&spec);
    let shipped = file(&out, "src/machines.rs").unwrap();
    assert!(shipped.contains("pub mod job {"));
    assert!(!shipped.contains("pub mod lease {"));
    assert!(!shipped.contains("pub mod hold {"));
    let target = file(&out, "src/target_machines.rs").unwrap();
    assert!(target.contains("pub mod lease {") && target.contains("pub mod hold {"));
    assert!(!target.contains("pub mod job {"));
}

#[test]
fn a_declared_shipped_machine_of_an_implemented_component_stays_shipped() {
    let mut spec = fixture("machines");
    component(&mut spec, "lease-service")["implementation"] = json!({ "status": "implemented" });
    behavior(&mut spec, "lease")["realisation"] = json!("shipped");
    let out = files(&spec);
    assert!(file(&out, "src/target_machines.rs").is_none());
    assert!(file(&out, "src/machines.rs")
        .unwrap()
        .contains("pub mod lease {"));
}

#[test]
fn a_realisation_that_is_neither_shipped_nor_target_is_refused() {
    let mut spec = fixture("machines");
    behavior(&mut spec, "lease")["realisation"] = json!("someday");
    let refused = generate("spec.jsonc", &spec.to_string(), &options()).unwrap_err();
    assert!(refused
        .iter()
        .any(|d| d.entity == "lease" && d.message.contains("shipped or target")));
}

#[test]
fn a_target_contract_has_no_route_no_handler_and_no_trait_method() {
    let mut spec = fixture("wiring");
    let shipped = files(&spec);
    let routes = file(&shipped, "src/routes.rs").unwrap();
    assert!(routes.contains("/api/jobs/submit"));
    assert!(file(&shipped, "src/contracts.rs")
        .unwrap()
        .contains("fn submit_job("));
    assert!(file(&shipped, "src/handlers.rs")
        .unwrap()
        .contains("submit_job"));

    contract(&mut spec, "submit-job")["deployment"] = json!("target");
    let out = files(&spec);
    assert!(!file(&out, "src/routes.rs")
        .unwrap()
        .contains("/api/jobs/submit"));
    let contracts = file(&out, "src/contracts.rs").unwrap();
    // The request, response and error types and the metadata stay: the specification still
    // describes the contract. The trait the component implements does not have it.
    assert!(contracts.contains("pub struct SubmitJobRequest"));
    assert!(contracts.contains("pub const SUBMIT_JOB: ContractMeta"));
    assert!(!contracts.contains("fn submit_job("));
    assert!(!file(&out, "src/handlers.rs")
        .unwrap()
        .contains("pub async fn submit_job"));
    // The other contracts of the component are still served.
    assert!(file(&out, "src/routes.rs").unwrap().contains("cancel"));
}

#[test]
fn a_not_implemented_implementer_makes_its_contracts_target_unless_they_say_shipped() {
    let mut spec = fixture("wiring");
    component(&mut spec, "scheduler")["implementation"] = json!({ "status": "not-implemented" });
    contract(&mut spec, "cancel-job")["deployment"] = json!("shipped");
    let out = files(&spec);
    let routes = file(&out, "src/routes.rs").unwrap();
    assert!(!routes.contains("/api/jobs/submit"));
    let contracts = file(&out, "src/contracts.rs").unwrap();
    assert!(!contracts.contains("fn submit_job("));
    assert!(contracts.contains("fn cancel_job("));
}

/// A machine held in memory: no record, key, fields, state, state values, creates, storedIn or
/// transaction, only the states, variables and transitions.
fn memory_machine() -> Value {
    json!({
        "id": "ticker", "name": "Ticker", "subject": "lease-service", "initial": "idle",
        "level": "implementation",
        "storage": { "durability": "memory" },
        "variables": [ { "name": "ticks", "type": "number", "initial": 0, "repr": "u32" } ],
        "states": [ { "id": "idle", "name": "Idle" }, { "id": "busy", "name": "Busy" } ],
        "transitions": [
            { "id": "t-start", "from": "idle", "to": "busy", "on": "ticker-start",
              "effects": [ { "set": "ticks", "to": "ticks + 1" } ] },
            { "id": "t-stop", "from": "busy", "to": "idle", "on": "ticker-start", "otherwise": true }
        ]
    })
}

fn with_memory_machine() -> Value {
    let mut spec = fixture("machines");
    spec["events"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": "ticker-start", "name": "Start", "payload": [] }));
    spec["behaviors"]
        .as_array_mut()
        .unwrap()
        .push(memory_machine());
    spec
}

#[test]
fn a_memory_machine_has_a_pure_step_and_no_record_conversion_key_or_transaction() {
    let out = files(&with_memory_machine());
    let machines = file(&out, "src/machines.rs").unwrap();
    let start = machines
        .find("pub mod ticker {")
        .expect("the memory machine is generated");
    let end = machines[start..]
        .find("\npub mod ")
        .map_or(machines.len(), |i| start + i);
    let module = &machines[start..end];
    assert!(module.contains("pub fn step("));
    assert!(module.contains("MemoryOutcome"));
    assert!(!module.contains("to_record"));
    assert!(!module.contains("from_record"));
    assert!(!module.contains("pub type Key"));
    assert!(!module.contains("Isolation"));
    assert!(!module.contains("impl Machine for"));
    // The host and the system cannot run it, so the routing table does not name it.
    let dispatch = &machines[machines.find("pub mod dispatch").unwrap()..];
    assert!(!dispatch.contains("ticker"));
}

#[test]
fn a_target_memory_machine_is_in_the_feature_module_and_not_in_the_dispatch() {
    let mut spec = with_memory_machine();
    behavior(&mut spec, "ticker")["realisation"] = json!("target");
    let out = files(&spec);
    assert!(!file(&out, "src/machines.rs")
        .unwrap()
        .contains("pub mod ticker {"));
    let target = file(&out, "src/target_machines.rs").unwrap();
    assert!(target.contains("pub mod ticker {"));
    assert!(!target.contains("ticker::Ticker>"));
}
