//! The declarations of an object type (retention, publication, language field, translation
//! group, a personal field), the removal of an object type, and the catalogue of a tenant schema
//! (templates, tokens, the ownership record of a product): what the kernel holds, what it refuses
//! when they are defined, and that a schema that never used one hashes as before. One rule per test.

use std::collections::BTreeMap;
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_model_core::types::{Expression, StructuredBlocks};

// ---------------------------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------------------------

fn step(
    world: &World,
    who: &str,
    now: i64,
    intent: Intent,
) -> Result<(World, Transition), DomainError> {
    world.step(&intent, &ctx("alpha", user(&format!("alpha:{who}")), now))
}

fn must(world: &World, who: &str, now: i64, intent: Intent) -> World {
    step(world, who, now, intent).expect("accepted").0
}

fn change(changes: Vec<SchemaChange>) -> Intent {
    Intent::ChangeSchema {
        changes,
        expected_version: None,
        existing: ExistingData::Reject,
    }
}

fn refused(world: &World, changes: Vec<SchemaChange>) -> DomainError {
    step(world, "owner", 9_000, change(changes)).expect_err("refused")
}

fn schema(world: &World) -> TenantSchema {
    TenantSchema::clone(&world.tenants[&tenant_id("alpha")].schema)
}

fn personal(id: &str) -> FieldDef {
    FieldDef {
        personal: Some(PersonalDeclaration {
            kind: PersonalKind::Name,
            subject: DataSubject::Creator,
            searchable: false,
        }),
        normalise: None,
        ..field_def(id, FieldType::Str, false)
    }
}

fn retention() -> RetentionRule {
    RetentionRule {
        days: 365,
        from: RetentionStart::Created,
    }
}

// ---------------------------------------------------------------------------------------------
// Retention and personal fields (D-6584).
// ---------------------------------------------------------------------------------------------

// Verifies: INT-2340
#[test]
fn the_subject_of_personal_data_is_a_field_of_the_type() {
    let mut field = personal("name");
    field.personal.as_mut().unwrap().subject = DataSubject::Field {
        field: strata_kernel::testkit::field("owner_of"),
    };
    let world = notes_world();
    let error = refused(
        &world,
        vec![
            SchemaChange::DefineObject {
                id: object("member"),
                label: "Member".into(),
            },
            SchemaChange::DefineField {
                object: object("member"),
                field,
            },
            SchemaChange::SetRetention {
                object: object("member"),
                retention: retention(),
            },
        ],
    );
    assert!(error.message.contains("unknown field owner_of"));
}

// ---------------------------------------------------------------------------------------------
// The declarations of an object type: set, clear, hash.
// ---------------------------------------------------------------------------------------------

// ---------------------------------------------------------------------------------------------
// Removing an object type (D-6581).
// ---------------------------------------------------------------------------------------------

fn scratch() -> Vec<SchemaChange> {
    vec![
        SchemaChange::DefineObject {
            id: object("scratch"),
            label: "Scratch".into(),
        },
        SchemaChange::DefineField {
            object: object("scratch"),
            field: field_def("name", FieldType::Str, false),
        },
    ]
}

fn scratch_world() -> World {
    let world = must(&notes_world(), "owner", 7_000, change(scratch()));
    must(
        &world,
        "owner",
        7_001,
        Intent::CreateRecord {
            object: object("scratch"),
            fields: BTreeMap::from([(field("name"), text("one"))]),
        },
    )
}

// Verifies: INT-2341
#[test]
fn an_object_type_with_records_is_not_removed_unless_the_change_deletes_them() {
    let world = scratch_world();
    let error = refused(
        &world,
        vec![SchemaChange::RemoveObject {
            id: object("scratch"),
            delete_records: false,
        }],
    );
    assert_eq!(error.kind, ErrorKind::MigrationRejected);
    assert!(
        error.message.contains("scratch has 1 record(s)"),
        "{}",
        error.message
    );
    // Nothing was changed.
    assert!(schema(&world).objects.contains_key(&object("scratch")));
}

// ---------------------------------------------------------------------------------------------
// Templates and tokens (EML-012, NTF-1570, CTK-001 to CTK-003).
// ---------------------------------------------------------------------------------------------

/// A body of one paragraph (the blocks of `strata_kernel::blocks`, D-7900).
fn paragraph_body(text: &str) -> StructuredBlocks {
    StructuredBlocks(format!(
        "[{{\"type\":\"paragraph\",\"inlines\":[{{\"type\":\"text\",\"text\":\"{text}\"}}]}}]"
    ))
}

fn mail(id: &str) -> MailTemplateDecl {
    MailTemplateDecl {
        template: TemplateId(id.to_string()),
        subject: "Welcome".into(),
        body: paragraph_body("Hello"),
    }
}

fn notice(id: &str) -> NoticeTemplateDecl {
    NoticeTemplateDecl {
        template: TemplateId(id.to_string()),
        title: "Welcome".into(),
        body: paragraph_body("Hello"),
    }
}

/// An edit of the templates and tokens of a schema, as one schema change.
enum Edit {
    MailTemplate(MailTemplateDecl),
    NoticeTemplate(NoticeTemplateDecl),
    Token(TokenDecl),
}

fn catalogue(edit: Edit) -> SchemaChange {
    match edit {
        Edit::MailTemplate(d) => SchemaChange::DefineMailTemplate(d),
        Edit::NoticeTemplate(d) => SchemaChange::DefineNoticeTemplate(d),
        Edit::Token(d) => SchemaChange::DefineToken(d),
    }
}

// Verifies: INT-2343
#[test]
fn a_template_id_is_used_by_one_kind_of_template_only() {
    let error = refused(
        &notes_world(),
        vec![
            catalogue(Edit::MailTemplate(mail("welcome"))),
            catalogue(Edit::NoticeTemplate(notice("welcome"))),
        ],
    );
    assert_eq!(error.kind, ErrorKind::InvalidSchema);
    assert_eq!(
        error.message,
        "template welcome: is both a mail template and a notice template"
    );
}

// Verifies: CTK-002
#[test]
fn a_token_whose_expression_does_not_parse_is_refused() {
    let error = refused(
        &notes_world(),
        vec![catalogue(Edit::Token(TokenDecl {
            name: "broken".into(),
            expression: Expression("record.title ==".into()),
            fallback: "".into(),
        }))],
    );
    assert_eq!(error.kind, ErrorKind::InvalidSchema);
    assert!(
        error.message.starts_with("token broken: expression:"),
        "{}",
        error.message
    );
}

// ---------------------------------------------------------------------------------------------
// The blocks of a template: the schema, the placement of a token and of a value, the render
// (EML-012, NTF-1570, CMS-250, CTK-003, CTK-004, CTK-005, CTK-015, D-7900).
// ---------------------------------------------------------------------------------------------

fn body(json: &str) -> StructuredBlocks {
    StructuredBlocks(json.to_string())
}

fn notice_with(body_json: &str) -> SchemaChange {
    catalogue(Edit::NoticeTemplate(NoticeTemplateDecl {
        body: body(body_json),
        ..notice("greeting")
    }))
}

// Verifies: CTK-003
#[test]
fn a_token_expression_cannot_name_another_token() {
    // The tenant rule language reads record fields and event parameters only, so a token that
    // names another, itself or a cycle of tokens does not parse.
    for expression in ["token.first_name", "token.other + token.first_name"] {
        let error = refused(
            &notes_world(),
            vec![catalogue(Edit::Token(TokenDecl {
                name: "first_name".into(),
                expression: Expression(expression.into()),
                fallback: "".into(),
            }))],
        );
        assert!(
            error.message.starts_with("token first_name: expression:"),
            "{}",
            error.message
        );
    }
}

fn bad(world: &World, json: &str) -> String {
    refused(world, vec![notice_with(json)]).message
}

// Verifies: EML-012
// Verifies: CMS-250
#[test]
fn a_body_that_is_not_blocks_of_the_schema_is_refused_with_where() {
    let world = notes_world();
    let cases = [
        ("{}", "the body is not a list of blocks"),
        (
            r#"[{"type":"paragraph","text":"Hello"}]"#,
            "the body at [0]: has the member \"text\", which its type does not",
        ),
        (
            r#"[{"type":"banner"}]"#,
            "the body at [0]: \"banner\" is not a block type",
        ),
        (
            r#"[{"type":"heading","level":1,"inlines":[]}]"#,
            "the level of a heading is 2, 3 or 4",
        ),
        (
            r#"[{"type":"paragraph","inlines":[{"type":"blink","text":"x"}]}]"#,
            "the body at [0].inlines[0]: \"blink\" is not an inline type",
        ),
        (
            r#"[{"type":"paragraph","inlines":[{"type":"link","href":"javascript:alert(1)","children":[]}]}]"#,
            "a link carries a web (http, https) or mail (mailto) address",
        ),
        (
            r#"[{"type":"paragraph","inlines":[{"type":"link","href":"//evil.test","children":[]}]}]"#,
            "a link carries a web (http, https) or mail (mailto) address",
        ),
        (
            r#"[{"type":"paragraph","inlines":[{"type":"link","href":"https://a.test/ b","children":[]}]}]"#,
            "a link carries a web (http, https) or mail (mailto) address",
        ),
        (
            r#"[{"type":"figure","src":"https://a.test/x.png","alt":"","caption":""}]"#,
            "src is not a path under /media/",
        ),
        (
            r#"[{"type":"paragraph","inlines":[{"type":"token","name":"First Name"}]}]"#,
            "the name of a token is lower-case letters",
        ),
        (
            r#"[{"type":"paragraph","inlines":[{"type":"value","name":"n","value_type":"float"}]}]"#,
            "value_type is str, int, bool or time",
        ),
        (
            r#"[{"type":"list","ordered":"yes","items":[]}]"#,
            "ordered is not true or false",
        ),
    ];
    for (json, want) in cases {
        let message = bad(&world, json);
        assert!(message.contains(want), "{json}: {message}");
    }
}

fn rendered(json: &str, values: &[(&str, Value)]) -> Result<blocks::Rendered, blocks::RenderError> {
    let values: BTreeMap<String, Value> = values
        .iter()
        .map(|(name, value)| (name.to_string(), value.clone()))
        .collect();
    blocks::render(json, &values, &|name| match name {
        "first_name" => Some("Ada".to_string()),
        _ => None,
    })
}

// Verifies: CTK-015
// Verifies: NTF-1570
#[test]
fn a_body_renders_with_each_token_and_value_as_a_text_node_and_as_plain_text() {
    let json = r##"[
        {"type":"heading","level":2,"inlines":[{"type":"text","text":"Hello "},{"type":"token","name":"first_name"}]},
        {"type":"paragraph","inlines":[
            {"type":"text","text":"You have "},
            {"type":"strong","children":[{"type":"value","name":"points","value_type":"int"}]},
            {"type":"text","text":" points since "},
            {"type":"value","name":"since","value_type":"time"},
            {"type":"text","text":" ("},
            {"type":"value","name":"vip","value_type":"bool"},
            {"type":"text","text":"). "},
            {"type":"link","href":"https://example.test/badges","children":[{"type":"text","text":"See them"}]}
        ]},
        {"type":"quote","blocks":[{"type":"paragraph","inlines":[{"type":"text","text":"Well done"}]}]},
        {"type":"list","ordered":false,"items":[[{"type":"text","text":"a"}],[{"type":"text","text":"b"}]]},
        {"type":"list","ordered":true,"items":[[{"type":"text","text":"a"}],[{"type":"text","text":"b"}]]},
        {"type":"rule"}
    ]"##;
    let out = rendered(
        json,
        &[
            ("points", Value::Int(500)),
            ("since", Value::Time(1_700_000_000_000)),
            ("vip", Value::Bool(true)),
            // A value the body does not place is ignored.
            ("unused", Value::Str("x".into())),
        ],
    )
    .expect("rendered");
    assert_eq!(
        out.text,
        "Hello Ada\n\nYou have 500 points since 2023-11-14T22:13:20Z (true). See them (https://example.test/badges)\n\n> Well done\n\n- a\n- b\n\n1. a\n2. b\n\n---"
    );
    // No token and no value is left in the blocks, and the blocks are blocks of the schema.
    let again = blocks::analyse(&out.blocks.to_string());
    assert!(again.problems.is_empty(), "{:?}", again.problems);
    assert!(again.tokens.is_empty() && again.values.is_empty());
    assert_eq!(
        out.blocks[0]["inlines"],
        serde_json::json!([
            {"type":"text","text":"Hello "},
            {"type":"text","text":"Ada"}
        ])
    );
    assert_eq!(
        out.blocks[1]["inlines"][1]["children"],
        serde_json::json!([{"type":"text","text":"500"}])
    );
    // Equal inputs give equal output (CTK-008).
    assert_eq!(
        rendered(
            json,
            &[
                ("points", Value::Int(500)),
                ("since", Value::Time(1_700_000_000_000)),
                ("vip", Value::Bool(true))
            ]
        )
        .unwrap(),
        out
    );
}

// ---------------------------------------------------------------------------------------------
// The ownership record of a product (D-6582).
// ---------------------------------------------------------------------------------------------

// ---------------------------------------------------------------------------------------------
// A change from outside a product to an entity it owns (D-6582, INT-1582).
// ---------------------------------------------------------------------------------------------

// ---------------------------------------------------------------------------------------------
// Removing a role.
// ---------------------------------------------------------------------------------------------
