//! What the editor is told of a record type (WG-5143): never a field the reader may not read, and
//! never a constraint or guard that reads one. One rule per test.

use strata_kernel::editor_rules::record_rules;
use strata_kernel::rules::Lookup;
use strata_kernel::testkit::*;
use strata_kernel::*;

/// The notes world with a `secret` field on notes that only an editor may read, and a constraint
/// over it.
fn world() -> World {
    let secret = FieldDef {
        read: Some(Expr::HasRole(role("editor"))),
        ..field_def("secret", FieldType::Str, false)
    };
    let length = Expr::Cmp(
        CmpOp::Le,
        Box::new(Expr::Len(Box::new(var(Scope::New, "secret")))),
        Box::new(Expr::Lit(Value::Int(10))),
    );
    let changes = vec![
        SchemaChange::DefineField {
            object: object("note"),
            field: secret,
        },
        SchemaChange::DefineConstraint {
            object: object("note"),
            constraint: ConstraintDef {
                id: ConstraintId::new("secret_length").expect("id"),
                expr: length,
                message: "A secret has at most 10 characters".into(),
            },
        },
    ];
    notes_world()
        .step(
            &Intent::ChangeSchema {
                changes,
                expected_version: None,
                existing: ExistingData::Reject,
            },
            &ctx("alpha", user("alpha:owner"), 5_000),
        )
        .expect("the owner defines the secret")
        .0
}

/// The metadata one member of alpha is sent for a note: the first stored one, or a new one.
fn rules_for(world: &World, who: &str, stored: bool) -> serde_json::Value {
    let state = world.state(&tenant_id("alpha"));
    let held = state.tenant.as_deref().expect("alpha exists");
    let lookup = Lookup {
        tenant: held,
        coverage: &state.coverage,
    };
    let note = object("note");
    let first = held
        .records
        .get(&note)
        .and_then(|records| records.values().next())
        .cloned();
    assert!(first.is_some(), "the fixture holds a note");
    record_rules(
        lookup,
        &user(&format!("alpha:{who}")),
        Timestamp(9_000),
        &held.schema.objects[&note],
        first.as_ref().filter(|_| stored),
    )
    .expect("the snapshot decides it")
}

fn ids(rules: &serde_json::Value, list: &str) -> Vec<String> {
    rules[list]
        .as_array()
        .expect("a list")
        .iter()
        .map(|item| item["id"].as_str().expect("an id").to_string())
        .collect()
}

#[test]
fn a_reader_who_may_not_read_a_field_is_told_nothing_of_it() {
    let world = world();
    for stored in [false, true] {
        let rules = rules_for(&world, "writer1", stored);
        assert!(!ids(&rules, "fields").contains(&"secret".to_string()));
        assert_eq!(ids(&rules, "constraints"), vec!["title_length"]);
        assert!(!rules.to_string().contains("secret"), "{rules}");
    }
}

#[test]
fn a_reader_who_may_read_the_field_is_told_its_shape_and_its_constraints() {
    let world = world();
    let rules = rules_for(&world, "editor", true);
    assert!(ids(&rules, "fields").contains(&"secret".to_string()));
    assert_eq!(
        ids(&rules, "constraints"),
        vec!["secret_length", "title_length"]
    );
    let secret = rules["fields"]
        .as_array()
        .and_then(|fields| fields.iter().find(|f| f["id"] == "secret"))
        .expect("described");
    assert_eq!(secret["type"], "str");
    assert_eq!(rules["limits"]["string_bytes"], 65_536);
    assert!(secret.get("read").is_none(), "the read rule is never sent");
}

#[test]
fn a_new_record_is_not_told_of_a_field_whose_read_rule_it_cannot_decide() {
    let world = world();
    let rules = rules_for(&world, "editor", false);
    assert!(!ids(&rules, "fields").contains(&"secret".to_string()));
    assert_eq!(ids(&rules, "constraints"), vec!["title_length"]);
}

#[test]
fn a_workflow_is_told_from_the_current_state_with_its_guard_as_text() {
    let world = world();
    let rules = rules_for(&world, "editor", false);
    let flow = &rules["workflows"][0];
    assert_eq!(flow["field"], "status");
    assert_eq!(flow["initial"], serde_json::json!(["draft"]));
    assert_eq!(flow["transitions"], serde_json::json!([]));
}

#[test]
fn the_server_refuses_a_draft_with_the_constraint_and_the_words_the_editor_was_told() {
    let world = world();
    let told = rules_for(&world, "editor", false);
    let constraint = told["constraints"]
        .as_array()
        .and_then(|list| list.iter().find(|c| c["id"] == "title_length"))
        .expect("the editor is told the constraint");
    let refused = world
        .step(
            &new_note(&"t".repeat(25), "too-long"),
            &ctx("alpha", user("alpha:editor"), 9_000),
        )
        .expect_err("a title of 25 characters is refused");
    let check = refused
        .evidence
        .iter()
        .find(|check| !check.passed && check.rule == "constraint:note.title_length")
        .expect("the refusal names the constraint the editor was told");
    // The page words its refusal the same way: `message [rule]` (web/src/kernel/record-rules.js).
    assert_eq!(
        check.detail,
        format!(
            "{} [{}]",
            constraint["message"].as_str().unwrap(),
            constraint["rule"].as_str().unwrap()
        )
    );
}
