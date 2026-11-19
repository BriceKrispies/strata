//! Refusals: a specification that does not state what the backends need is refused with
//! diagnostics, every one at once, and never with a panic.

use serde_json::{json, Value};
use strata_spec_compiler::{generate, Diagnostic, Options, RuntimeDependency};

fn options() -> Options {
    Options {
        crate_name: "generated".to_string(),
        runtime: RuntimeDependency::Workspace,
    }
}

/// A ready specification with the given sections merged in.
fn spec(sections: Value) -> String {
    let mut base = json!({
        "exegra": "0.1",
        "system": { "id": "sys", "name": "System" },
        "components": [ { "id": "svc", "name": "Service" } ],
        "types": [],
        "records": [],
        "contracts": []
    });
    if let (Some(base), Some(extra)) = (base.as_object_mut(), sections.as_object()) {
        for (k, v) in extra {
            base.insert(k.clone(), v.clone());
        }
    }
    base.to_string()
}

fn refuse(text: &str) -> Vec<Diagnostic> {
    match generate("spec.jsonc", text, &options()) {
        Ok(_) => panic!("the specification was not refused"),
        Err(diagnostics) => diagnostics,
    }
}

fn codes(text: &str) -> Vec<String> {
    refuse(text).into_iter().map(|d| d.code).collect()
}

fn assert_refused_with(text: &str, code: &str, entity: &str) {
    let diagnostics = refuse(text);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == code && d.entity == entity),
        "expected {code} on {entity}, got:\n{}",
        diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

fn record(fields: Value, extra: Value) -> Value {
    let mut r = json!({ "id": "rec", "name": "Rec", "owner": "svc", "key": ["id"], "schemaVersion": 1,
        "fields": fields });
    if let (Some(r), Some(extra)) = (r.as_object_mut(), extra.as_object()) {
        for (k, v) in extra {
            r.insert(k.clone(), v.clone());
        }
    }
    r
}

fn contract(extra: Value) -> Value {
    let mut c = json!({ "id": "do-it", "name": "Do it", "realisation": "generated" });
    if let (Some(c), Some(extra)) = (c.as_object_mut(), extra.as_object()) {
        for (k, v) in extra {
            c.insert(k.clone(), v.clone());
        }
    }
    c
}

#[test]
fn the_base_specification_is_ready() {
    assert!(generate("spec.jsonc", &spec(json!({})), &options()).is_ok());
}

#[test]
fn syntax_and_shape_errors_are_refused() {
    assert_eq!(codes("{ \"system\": "), ["specc.spec-syntax"]);
    assert_eq!(codes("[]"), ["specc.spec-shape"]);
    assert_refused_with(
        &spec(json!({ "types": [ { "id": "t", "name": "T", "kind": "union" } ] })),
        "specc.spec-shape",
        "t",
    );
}

#[test]
fn diagnostics_print_in_the_refusal_format() {
    let text = spec(
        json!({ "records": [ record(json!([ { "name": "id", "type": "nope" } ]), json!({})) ] }),
    );
    let line = refuse(&text)[0].to_string();
    assert_eq!(
        line,
        "error[ready.field-type-undeclared] rec: Field \"id\" of record \"rec\" has type \"nope\", which is neither built-in nor a declared type."
    );
}

#[test]
fn type_rules_are_checked() {
    let types = json!([
        { "id": "string", "name": "S", "kind": "id", "repr": "string" },
        { "id": "no-repr", "name": "N", "kind": "scalar" },
        { "id": "bad-repr", "name": "B", "kind": "id", "repr": "list<u8>" },
        { "id": "no-values", "name": "V", "kind": "enum", "values": [] },
        { "id": "no-fields", "name": "F", "kind": "struct", "fields": [] },
        { "id": "bad-field", "name": "G", "kind": "struct", "fields": [ { "name": "x", "type": "nope" } ] },
        { "id": "loop", "name": "L", "kind": "struct", "fields": [ { "name": "next", "type": "option<loop>" } ] },
        { "id": "ok-tree", "name": "T", "kind": "struct", "fields": [ { "name": "kids", "type": "list<ok-tree>" } ] }
    ]);
    let text = spec(json!({ "types": types }));
    assert_refused_with(&text, "type.shadows-builtin", "string");
    assert_refused_with(&text, "type.repr-missing", "no-repr");
    assert_refused_with(&text, "type.repr-not-builtin", "bad-repr");
    assert_refused_with(&text, "type.values-missing", "no-values");
    assert_refused_with(&text, "type.fields-missing", "no-fields");
    assert_refused_with(&text, "type.field-unresolved", "bad-field");
    assert_refused_with(&text, "specc.type-recursive", "loop");
    assert!(refuse(&text).iter().all(|d| d.entity != "ok-tree"));
}

#[test]
fn names_that_cannot_become_rust_names_are_refused() {
    let types = json!([
        { "id": "level", "name": "Level", "kind": "enum", "values": ["1st", "second"] },
        { "id": "a-b", "name": "A", "kind": "id", "repr": "u32" },
        { "id": "a_b", "name": "A", "kind": "id", "repr": "u32" },
        { "id": "option", "name": "O", "kind": "id", "repr": "u32" },
        { "id": "dup", "name": "D", "kind": "enum", "values": ["in-progress", "in_progress"] }
    ]);
    let text = spec(json!({ "types": types }));
    assert_refused_with(&text, "specc.name-invalid", "level");
    assert_refused_with(&text, "specc.name-collision", "a_b");
    assert_refused_with(&text, "specc.name-reserved", "option");
    assert_refused_with(&text, "specc.name-collision", "dup");
}

#[test]
fn record_key_and_reference_rules_are_checked() {
    let records = json!([
        { "id": "no-version", "name": "N", "owner": "svc", "key": ["id"], "fields": [ { "name": "id", "type": "u32" } ] },
        record(json!([ { "name": "id", "type": "u32" } ]), json!({ "id": "bad-version", "schemaVersion": 0 })),
        record(json!([ { "name": "id", "type": "u32" } ]), json!({ "id": "no-key", "key": null })),
        record(json!([ { "name": "id", "type": "u32" } ]), json!({ "id": "bad-key", "key": ["missing"] })),
        record(json!([
            { "name": "id", "type": "u32" },
            { "name": "a", "type": "u32", "refersTo": "no-version" },
            { "name": "b", "type": "u32", "onDelete": "cascade" },
            { "name": "c", "type": "u32", "refersTo": "no-version", "onDelete": "set-null" },
            { "name": "d", "type": "u32", "refersTo": "ghost", "onDelete": "restrict" },
            { "name": "id", "type": "u32" }
        ]), json!({ "id": "refs", "unique": [["zzz"]], "indexes": [ { "name": "ix", "fields": ["yyy"] } ] }))
    ]);
    let text = spec(json!({ "records": records }));
    assert_refused_with(&text, "ready.schema-version-missing", "no-version");
    assert_refused_with(&text, "record.schema-version", "bad-version");
    assert_refused_with(&text, "specc.record-key-missing", "no-key");
    assert_refused_with(&text, "record.key-field", "bad-key");
    assert_refused_with(&text, "ready.on-delete-missing", "refs");
    assert_refused_with(&text, "record.on-delete-without-refers-to", "refs");
    assert_refused_with(&text, "record.set-null-required", "refs");
    assert_refused_with(&text, "specc.refers-to-unknown", "refs");
    assert_refused_with(&text, "record.field-duplicate", "refs");
    assert_refused_with(&text, "record.unique-field", "refs");
    assert_refused_with(&text, "record.index-field", "refs");
}

#[test]
fn constraints_are_checked_against_the_field_type() {
    let records = json!([record(
        json!([
            { "name": "id", "type": "u32" },
            { "name": "a", "type": "u32", "constraints": { "pattern": "^a$" } },
            { "name": "b", "type": "string", "constraints": { "min": 1 } },
            { "name": "c", "type": "list<string>", "constraints": { "maxLength": 3 } },
            { "name": "d", "type": "u32", "constraints": { "min": 1.5 } },
            { "name": "e", "type": "u8", "constraints": { "min": 300 } },
            { "name": "f", "type": "i32", "constraints": { "min": 5, "max": 1 } },
            { "name": "g", "type": "string", "constraints": { "pattern": "(" } },
            { "name": "h", "type": "string", "constraints": { "maxLength": -1 } }
        ]),
        json!({})
    )]);
    let diagnostics = refuse(&spec(json!({ "records": records })));
    let count = |code: &str| diagnostics.iter().filter(|d| d.code == code).count();
    assert_eq!(count("specc.constraint-type"), 3, "{diagnostics:?}");
    assert_eq!(count("specc.constraint-value"), 4, "{diagnostics:?}");
    assert_eq!(count("specc.pattern-invalid"), 1, "{diagnostics:?}");
}

#[test]
fn contract_rules_are_checked() {
    let contracts = json!([
        contract(
            json!({ "id": "untyped", "inputs": [ { "name": "x" } ], "outputs": [ { "name": "y", "type": "nope" } ],
            "errors": [ { "name": "e", "type": "nope" } ] })
        ),
        contract(json!({ "id": "unrealised", "realisation": null })),
        contract(json!({ "id": "bad-hole", "realisation": "hand", "ref": "CT-1" })),
        contract(json!({ "id": "orphan", "implementedBy": "nobody", "timeoutMs": 0 }))
    ]);
    let text = spec(json!({ "contracts": contracts }));
    assert_refused_with(&text, "ready.contract-input-type-undeclared", "untyped");
    assert_refused_with(&text, "ready.contract-output-type-undeclared", "untyped");
    assert_refused_with(&text, "ready.contract-error-type-undeclared", "untyped");
    assert_refused_with(&text, "ready.contract-realisation-missing", "unrealised");
    assert_refused_with(&text, "ready.hole-ref-invalid", "bad-hole");
    assert_refused_with(&text, "ready.hole-implemented-by-missing", "bad-hole");
    assert_refused_with(&text, "ready.hole-postcondition-missing", "bad-hole");
    assert_refused_with(&text, "specc.component-unknown", "orphan");
    assert_refused_with(&text, "specc.timeout-invalid", "orphan");
}

#[test]
fn a_well_formed_hole_is_accepted() {
    let contracts = json!([contract(
        json!({ "realisation": "hand", "ref": "H-12", "implementedBy": "svc",
        "postconditions": ["It is done."] })
    )]);
    assert!(generate(
        "spec.jsonc",
        &spec(json!({ "contracts": contracts })),
        &options()
    )
    .is_ok());
}

#[test]
fn generation_target_and_crate_name_are_checked() {
    let text =
        spec(json!({ "system": { "id": "sys", "name": "S", "generation": { "target": "go" } } }));
    assert_refused_with(&text, "specc.generation-target", "sys");
    let bad = Options {
        crate_name: "not a name".to_string(),
        runtime: RuntimeDependency::Workspace,
    };
    let diagnostics = generate("spec.jsonc", &spec(json!({})), &bad).unwrap_err();
    assert_eq!(diagnostics[0].code, "specc.crate-name-invalid");
}

#[test]
fn every_problem_is_reported_at_once() {
    let text = spec(json!({
        "types": [ { "id": "t", "name": "T", "kind": "enum", "values": [] } ],
        "records": [ record(json!([ { "name": "id", "type": "nope" } ]), json!({ "schemaVersion": null })) ],
        "contracts": [ contract(json!({ "realisation": null })) ]
    }));
    let found = codes(&text);
    for code in [
        "type.values-missing",
        "ready.field-type-undeclared",
        "ready.schema-version-missing",
        "ready.contract-realisation-missing",
    ] {
        assert!(
            found.iter().any(|c| c == code),
            "{code} missing from {found:?}"
        );
    }
}

#[test]
fn hostile_values_are_refused_without_panicking() {
    let texts = [
        spec(json!({ "types": [ { "id": "t", "name": 3, "kind": "enum", "values": [1, 2] } ] })),
        spec(json!({ "records": [ { "id": "r", "name": "R", "fields": "none" } ] })),
        spec(
            json!({ "records": [ record(json!([ { "name": "id", "type": "u64", "constraints": { "min": 1e300, "max": -1e300 } } ]), json!({})) ] }),
        ),
        spec(
            json!({ "contracts": [ { "id": "c", "name": "C", "inputs": [ 1 ], "errors": { } } ] }),
        ),
        spec(
            json!({ "types": [ { "id": "\u{e9}t\u{e9}", "name": "E", "kind": "id", "repr": "u8" } ] }),
        ),
        "{ /* unterminated".to_string(),
    ];
    for text in texts {
        assert!(generate("spec.jsonc", &text, &options()).is_err());
    }
}

/// The Strata specification is either generated from or refused with diagnostics; which one
/// depends on how far its generation readiness has come, so only the absence of a panic and the
/// diagnostic format are asserted.
#[test]
fn the_strata_specification_is_handled() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../specifications/spec.jsonc"
    );
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let options = Options {
        crate_name: "strata-spec".to_string(),
        runtime: RuntimeDependency::Workspace,
    };
    if let Err(diagnostics) = generate("specifications/spec.jsonc", &text, &options) {
        assert!(!diagnostics.is_empty());
        for d in diagnostics {
            assert!(d.to_string().starts_with("error["));
        }
    }
}
