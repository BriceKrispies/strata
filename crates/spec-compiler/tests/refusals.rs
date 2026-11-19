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

/// A base specification with a record, a contract with inputs, and the given construct sections.
fn with_constructs(sections: Value) -> String {
    let mut base: Value = serde_json::from_str(&spec(json!({
        "records": [ record(json!([
            { "name": "id", "type": "u32" }, { "name": "name", "type": "string" },
            { "name": "note", "type": "string", "optional": true }, { "name": "hash", "type": "bytes" } ]), json!({})) ],
        "contracts": [ contract(json!({ "policyDefault": "deny", "idempotent": false,
            "inputs": [ { "name": "slug", "type": "string" }, { "name": "n", "type": "u32" }, { "name": "tags", "type": "list<string>" } ],
            "outputs": [ { "name": "ok", "type": "bool" } ],
            "errors": [ { "name": "slow-down" } ] })) ]
    })))
    .unwrap();
    if let (Some(base), Some(extra)) = (base.as_object_mut(), sections.as_object()) {
        for (k, v) in extra {
            base.insert(k.clone(), v.clone());
        }
    }
    base.to_string()
}

#[test]
fn a_specification_with_every_construct_is_ready() {
    let text = with_constructs(json!({
        "policies": [ { "id": "p", "subject": { "principal": "member" }, "contract": "do-it", "effect": "allow", "condition": "request.n > 1" } ],
        "validations": [ { "id": "v", "contract": "do-it", "input": "slug", "rule": "required", "message": "m" } ],
        "retentions": [ { "id": "t", "record": "rec", "keepFor": "1 d", "then": "delete" } ],
        "quotas": [ { "id": "q", "scope": "tenant", "contract": "do-it", "limit": 1, "window": "1 s", "onExceed": "refuse", "error": "slow-down" } ],
        "retries": [ { "id": "y", "contract": "do-it", "attempts": 2, "backoff": { "kind": "fixed", "base": "1 s" } } ],
        "audits": [ { "id": "a", "contract": "do-it", "record": "rec", "immutability": "chained-hash", "chainField": "hash",
            "capture": [ { "field": "name", "from": "request.slug" }, { "field": "note", "from": "null" } ] } ]
    }));
    if let Err(diagnostics) = generate("spec.jsonc", &text, &options()) {
        panic!("refused: {diagnostics:?}");
    }
}

#[test]
fn constructs_that_cannot_be_generated_are_refused() {
    let text = with_constructs(json!({
        "policies": [
            { "id": "p-transition", "subject": { "principal": "member" }, "transition": "go", "effect": "allow" },
            { "id": "p-list", "subject": { "principal": "member" }, "contract": "do-it", "effect": "deny", "condition": "request.tags == null" },
            { "id": "p-contract", "subject": { "principal": "member" }, "contract": "nope", "effect": "allow" } ],
        "validations": [
            { "id": "v-unit", "contract": "do-it", "input": "slug", "rule": "maxLength", "maxLength": 3, "message": "m" },
            { "id": "v-type", "contract": "do-it", "input": "n", "rule": "pattern", "pattern": "a", "message": "m" },
            { "id": "v-target", "contract": "do-it", "rule": "required", "message": "m" } ],
        "retentions": [
            { "id": "t-clear", "record": "rec", "keepFor": "1 d", "then": "anonymise", "fields": ["id", "ghost"] },
            { "id": "t-twice", "record": "rec", "keepFor": "1 s", "then": "delete" } ],
        "quotas": [ { "id": "q", "scope": "tenant", "contract": "do-it", "limit": 1, "window": "1 s", "onExceed": "refuse" } ],
        "retries": [ { "id": "y", "contract": "do-it", "attempts": 2, "backoff": { "kind": "fixed", "base": "1 s", "cap": "2 s" } } ],
        "audits": [
            { "id": "a-event", "event": "sent", "record": "rec", "immutability": "append-only", "capture": [ { "field": "name", "from": "time" } ] },
            { "id": "a-unfilled", "contract": "do-it", "record": "rec", "immutability": "append-only", "capture": [ { "field": "name", "from": "request.slug" } ] } ]
    }));
    let found = codes(&text);
    for code in [
        "specc.policy-transition-unsupported",
        "specc.policy-expression",
        "specc.policy-contract-unknown",
        "ready.validation-unit-missing",
        "specc.validation-target-type",
        "specc.validation-target",
        "specc.retention-field-unknown",
        "specc.retention-duplicate",
        "specc.quota-error",
        "specc.retry-backoff",
        "specc.audit-target-unsupported",
        "ready.audit-field-unfilled",
    ] {
        assert!(
            found.iter().any(|c| c == code),
            "{code} missing from {found:?}"
        );
    }
}

#[test]
fn a_governed_contract_needs_a_policy_default() {
    let mut text: Value = serde_json::from_str(&with_constructs(json!({
        "policies": [ { "id": "p", "subject": { "principal": "member" }, "contract": "do-it", "effect": "allow" } ]
    })))
    .unwrap();
    text["contracts"][0]
        .as_object_mut()
        .unwrap()
        .remove("policyDefault");
    assert_refused_with(&text.to_string(), "ready.policy-default-missing", "do-it");
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

// Wire naming and json (WG-4881).

#[test]
fn a_struct_crossing_contracts_of_two_wire_cases_is_refused() {
    let text = spec(json!({
        "system": { "id": "sys", "name": "System", "generation": { "target": "rust", "wireCase": "snake_case" } },
        "types": [ { "id": "page", "name": "Page", "kind": "struct", "fields": [ { "name": "cover-alt", "type": "string" } ] } ],
        "contracts": [
            { "id": "get-page", "name": "Get page", "realisation": "generated", "outputs": [ { "name": "page", "type": "page" } ] },
            { "id": "get-page-camel", "name": "Get page in camel case", "realisation": "generated", "binding": { "case": "camelCase" },
              "outputs": [ { "name": "page", "type": "list<page>" } ] }
        ]
    }));
    assert_refused_with(&text, "specc.wire-case-conflict", "page");
}

#[test]
fn a_struct_whose_fields_read_the_same_in_both_cases_is_not_a_conflict() {
    let text = spec(json!({
        "system": { "id": "sys", "name": "System", "generation": { "target": "rust", "wireCase": "snake_case" } },
        "types": [ { "id": "point", "name": "Point", "kind": "struct", "fields": [ { "name": "x", "type": "f64" } ] } ],
        "contracts": [
            { "id": "a", "name": "A", "realisation": "generated", "outputs": [ { "name": "p", "type": "point" } ] },
            { "id": "b", "name": "B", "realisation": "generated", "binding": { "case": "camelCase" }, "outputs": [ { "name": "p", "type": "point" } ] }
        ]
    }));
    assert!(generate("spec.jsonc", &text, &options()).is_ok());
}

#[test]
fn a_wire_case_that_is_not_one_of_the_three_is_refused() {
    let text = spec(json!({
        "contracts": [ { "id": "a", "name": "A", "realisation": "generated", "binding": { "case": "PascalCase" } } ]
    }));
    assert_refused_with(&text, "specc.spec-shape", "a");
}

#[test]
fn an_id_represented_as_json_is_refused() {
    let text = spec(json!({
        "types": [ { "id": "doc-id", "name": "Document id", "kind": "id", "repr": "json" } ]
    }));
    assert_refused_with(&text, "type.repr-json", "doc-id");
}

/// A specification with one logged event: the base of the telemetry refusals, each of which
/// changes exactly one thing.
fn telemetry_spec(edit: impl Fn(&mut Value)) -> String {
    let mut sections = json!({
        "system": { "id": "sys", "name": "System",
            "telemetry": { "services": [ { "id": "backend", "log": "var/logs/backend.jsonl" } ] } },
        "types": [
            { "id": "tenant-id", "name": "Tenant", "kind": "id", "repr": "string" },
            { "id": "session-token", "name": "Session token", "kind": "scalar", "repr": "string", "sensitive": "token" },
            { "id": "login", "name": "Login", "kind": "struct", "fields": [ { "name": "password", "type": "string", "sensitive": "password" } ] }
        ],
        "events": [ {
            "id": "signed-in", "name": "Signed in", "domain": true,
            "payload": [
                { "name": "tenant", "type": "tenant-id" }, { "name": "session", "type": "string" },
                { "name": "attempts", "type": "u8" }, { "name": "at", "type": "timestamp" },
                { "name": "device", "type": "option<uuid>", "optional": true },
                { "name": "token", "type": "session-token" }, { "name": "secret", "type": "string", "sensitive": "secret" },
                { "name": "credentials", "type": "list<login>" }, { "name": "roles", "type": "list<string>" }
            ],
            "logging": { "logged": true, "service": "backend", "level": "info", "name": "session.started", "tenant": "tenant",
                "fields": [ { "field": "session" }, { "field": "attempts", "as": "tries" }, { "field": "at" }, { "field": "device" } ] }
        } ]
    });
    edit(&mut sections);
    spec(sections)
}

fn logged_fields(v: &mut Value) -> &mut Vec<Value> {
    v["events"][0]["logging"]["fields"].as_array_mut().unwrap()
}

#[test]
fn a_logged_event_generates_its_telemetry_function() {
    let files = generate("spec.jsonc", &telemetry_spec(|_| {}), &options()).unwrap();
    let module = &files
        .iter()
        .find(|f| f.path == "src/telemetry.rs")
        .expect("a telemetry module")
        .contents;
    assert!(module.contains("pub const BACKEND: JsonLines = JsonLines { service: \"backend\" };"));
    assert!(module.contains("pub struct SignedIn<'a>"));
    assert!(module.contains("pub fn signed_in(telemetry: &dyn Telemetry, line: &SignedIn<'_>)"));
    assert!(module.contains("fields.push((\"tries\", line.attempts.into()));"));
    assert!(module.contains("telemetry.event(Level::Info, \"session.started\", &fields);"));
    // Exactly the allowlist: nothing of the rest of the payload reaches the struct.
    for absent in ["pub token", "pub secret", "pub credentials", "pub roles"] {
        assert!(
            !module.contains(absent),
            "{absent} is in the telemetry module"
        );
    }
    let manifest = &files
        .iter()
        .find(|f| f.path == "Cargo.toml")
        .unwrap()
        .contents;
    assert!(manifest.contains("strata-runtime.workspace = true"));
}

#[test]
fn a_logged_field_that_is_not_in_the_payload_is_refused() {
    let text = telemetry_spec(|v| logged_fields(v).push(json!({ "field": "missing" })));
    assert_refused_with(&text, "specc.telemetry-field-unknown", "signed-in");
    let text = telemetry_spec(|v| v["events"][0]["logging"]["tenant"] = json!("site"));
    assert_refused_with(&text, "specc.telemetry-field-unknown", "signed-in");
}

#[test]
fn a_logged_field_of_a_sensitive_type_is_refused() {
    let text = telemetry_spec(|v| logged_fields(v).push(json!({ "field": "token" })));
    assert_eq!(codes(&text), ["specc.telemetry-field-sensitive"]);
}

#[test]
fn a_logged_field_marked_sensitive_is_refused() {
    let text = telemetry_spec(|v| logged_fields(v).push(json!({ "field": "secret" })));
    assert_eq!(codes(&text), ["specc.telemetry-field-sensitive"]);
}

#[test]
fn a_logged_list_of_a_struct_with_a_sensitive_field_is_refused() {
    let text = telemetry_spec(|v| logged_fields(v).push(json!({ "field": "credentials" })));
    assert_eq!(codes(&text), ["specc.telemetry-field-sensitive"]);
}

#[test]
fn a_logged_field_that_is_not_a_scalar_is_refused() {
    let text = telemetry_spec(|v| logged_fields(v).push(json!({ "field": "roles" })));
    assert_eq!(codes(&text), ["specc.telemetry-field-type"]);
}

#[test]
fn a_logged_event_of_an_undeclared_service_is_refused() {
    let text = telemetry_spec(|v| v["events"][0]["logging"]["service"] = json!("worker"));
    assert_eq!(codes(&text), ["specc.telemetry-service-unknown"]);
}

#[test]
fn a_logging_level_that_is_not_known_is_refused() {
    let text = telemetry_spec(|v| v["events"][0]["logging"]["level"] = json!("notice"));
    assert_eq!(codes(&text), ["specc.telemetry-level"]);
}
