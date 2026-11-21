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
fn a_realisation_that_is_neither_shipped_nor_target_is_refused() {
    let mut spec = fixture("machines");
    behavior(&mut spec, "lease")["realisation"] = json!("someday");
    let refused = generate("spec.jsonc", &spec.to_string(), &options()).unwrap_err();
    assert!(refused
        .iter()
        .any(|d| d.entity == "lease" && d.message.contains("shipped or target")));
}
