//! The declarations of an object type (retention, publication, language field, translation
//! group, a personal field), the removal of an object type, and the catalogue of a tenant schema
//! (templates, tokens, the ownership record of a product): what the kernel holds, what it refuses
//! when they are defined, and that a schema that never used one hashes as before. One rule per test.

use std::collections::{BTreeMap, BTreeSet};
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

/// The hash of the schema of a world, whatever its version.
fn hash(world: &World) -> String {
    canon::hash_of(&TenantSchema {
        version: SchemaVersion(0),
        ..schema(world)
    })
}

fn personal(id: &str) -> FieldDef {
    FieldDef {
        personal: Some(PersonalDeclaration {
            kind: PersonalKind::Name,
            subject: DataSubject::Creator,
            searchable: false,
        }),
        ..field_def(id, FieldType::Str, false)
    }
}

fn retention() -> RetentionRule {
    RetentionRule {
        days: 365,
        from: RetentionStart::Created,
    }
}

fn member() -> Vec<SchemaChange> {
    vec![
        SchemaChange::DefineObject {
            id: object("member"),
            label: "Member".into(),
        },
        SchemaChange::DefineField {
            object: object("member"),
            field: personal("name"),
        },
    ]
}

/// An object type with a status field and the workflow of a publication.
fn article() -> Vec<SchemaChange> {
    vec![
        SchemaChange::DefineObject {
            id: object("article"),
            label: "Article".into(),
        },
        SchemaChange::DefineField {
            object: object("article"),
            field: field_def("status", FieldType::Str, true),
        },
        SchemaChange::DefineField {
            object: object("article"),
            field: field_def("live_at", FieldType::Time, false),
        },
        SchemaChange::DefineWorkflow(WorkflowDef {
            id: WorkflowId::new("article_status").unwrap(),
            object: object("article"),
            field: field("status"),
            initial: BTreeSet::from(["draft".to_string()]),
            transitions: vec![
                WorkflowTransition {
                    name: "publish".into(),
                    from: "draft".into(),
                    to: "published".into(),
                    guard: None,
                    set: BTreeMap::new(),
                },
                WorkflowTransition {
                    name: "withdraw".into(),
                    from: "published".into(),
                    to: "withdrawn".into(),
                    guard: None,
                    set: BTreeMap::new(),
                },
            ],
        }),
    ]
}

fn publication() -> PublicationDef {
    PublicationDef {
        state_field: field("status"),
        draft: "draft".into(),
        published: "published".into(),
        withdrawn: "withdrawn".into(),
        in_review: None,
        approved: None,
        time_field: Some(field("live_at")),
        review_workflow: None,
        review_run_field: None,
    }
}

fn with(mut base: Vec<SchemaChange>, more: Vec<SchemaChange>) -> Vec<SchemaChange> {
    base.extend(more);
    base
}

// ---------------------------------------------------------------------------------------------
// Retention and personal fields (D-6584).
// ---------------------------------------------------------------------------------------------

// Verifies: INT-2340
#[test]
fn a_personal_field_needs_a_retention_in_the_same_change() {
    let world = notes_world();
    let error = refused(&world, member());
    assert_eq!(error.kind, ErrorKind::InvalidSchema);
    assert_eq!(
        error.message,
        "member: a type with a personal field must declare retention (D-6584)"
    );

    let world = must(
        &world,
        "owner",
        7_000,
        change(with(
            member(),
            vec![SchemaChange::SetRetention {
                object: object("member"),
                retention: retention(),
            }],
        )),
    );
    let schema = schema(&world);
    let member = &schema.objects[&object("member")];
    assert_eq!(member.retention, Some(retention()));
    // The declaration of the field survives the change and is stored with it.
    let declared = member.fields[&field("name")].personal.as_ref().unwrap();
    assert_eq!(declared.kind, PersonalKind::Name);
    assert_eq!(declared.subject, DataSubject::Creator);
}

#[test]
fn retention_cannot_be_cleared_while_a_field_is_personal() {
    let world = must(
        &notes_world(),
        "owner",
        7_000,
        change(with(
            member(),
            vec![SchemaChange::SetRetention {
                object: object("member"),
                retention: retention(),
            }],
        )),
    );
    let error = refused(
        &world,
        vec![SchemaChange::ClearRetention {
            object: object("member"),
        }],
    );
    assert_eq!(error.kind, ErrorKind::InvalidSchema);
    assert!(error.message.contains("must declare retention"));
}

#[test]
fn a_retention_counts_from_a_timestamp_field_and_lasts_a_day_at_least() {
    let world = notes_world();
    let set = |retention| {
        vec![SchemaChange::SetRetention {
            object: object("note"),
            retention,
        }]
    };
    let error = refused(
        &world,
        set(RetentionRule {
            days: 0,
            from: RetentionStart::LastChanged,
        }),
    );
    assert_eq!(error.message, "note: retention is at least one day");
    let error = refused(
        &world,
        set(RetentionRule {
            days: 30,
            from: RetentionStart::Field {
                field: field("title"),
            },
        }),
    );
    assert!(error.message.contains("which is not a timestamp"));
    let error = refused(
        &world,
        set(RetentionRule {
            days: 30,
            from: RetentionStart::Field {
                field: field("nothing"),
            },
        }),
    );
    assert!(error.message.contains("unknown field nothing"));
    must(
        &world,
        "owner",
        7_000,
        change(set(RetentionRule {
            days: 30,
            from: RetentionStart::Field {
                field: field("published_at"),
            },
        })),
    );
}

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

// Verifies: INT-2340
#[test]
fn a_declaration_changes_the_hash_of_the_schema_only_while_it_is_declared() {
    let world = notes_world();
    let before = hash(&world);
    let declared = must(
        &world,
        "owner",
        7_000,
        change(vec![SchemaChange::SetRetention {
            object: object("note"),
            retention: retention(),
        }]),
    );
    assert_ne!(hash(&declared), before);
    let cleared = must(
        &declared,
        "owner",
        7_001,
        change(vec![SchemaChange::ClearRetention {
            object: object("note"),
        }]),
    );
    assert_eq!(hash(&cleared), before);
    // A change of another kind does not touch the hash of a type that declares nothing.
    assert!(schema(&cleared).objects[&object("note")]
        .retention
        .is_none());
}

#[test]
fn clearing_what_is_not_declared_and_declaring_on_an_unknown_type_are_refused() {
    let world = notes_world();
    for (change, message) in [
        (
            SchemaChange::ClearRetention {
                object: object("note"),
            },
            "note declares no retention",
        ),
        (
            SchemaChange::ClearPublication {
                object: object("note"),
            },
            "note declares no publication",
        ),
        (
            SchemaChange::ClearLanguageField {
                object: object("note"),
            },
            "note declares no language field",
        ),
        (
            SchemaChange::ClearTranslationGroup {
                object: object("note"),
            },
            "note declares no translation group",
        ),
        (
            SchemaChange::SetRetention {
                object: object("ghost"),
                retention: retention(),
            },
            "no object type ghost",
        ),
    ] {
        let error = refused(&world, vec![change]);
        assert_eq!(error.kind, ErrorKind::InvalidSchema);
        assert_eq!(error.message, message);
    }
}

// Verifies: INT-2340
#[test]
fn a_language_field_has_the_language_type_and_a_translation_group_needs_one() {
    let world = notes_world();
    let error = refused(
        &world,
        vec![SchemaChange::SetLanguageField {
            object: object("note"),
            field: field("title"),
        }],
    );
    assert!(error.message.contains("does not have the language type"));
    let error = refused(
        &world,
        vec![
            SchemaChange::DefineField {
                object: object("note"),
                field: field_def("group", FieldType::Str, false),
            },
            SchemaChange::SetTranslationGroup {
                object: object("note"),
                field: field("group"),
            },
        ],
    );
    assert_eq!(
        error.message,
        "note: a translation group needs a language field"
    );
    let world = must(
        &world,
        "owner",
        7_000,
        change(vec![
            SchemaChange::DefineField {
                object: object("note"),
                field: field_def("lang", FieldType::Language, false),
            },
            SchemaChange::DefineField {
                object: object("note"),
                field: field_def("group", FieldType::Str, false),
            },
            SchemaChange::SetLanguageField {
                object: object("note"),
                field: field("lang"),
            },
            SchemaChange::SetTranslationGroup {
                object: object("note"),
                field: field("group"),
            },
        ]),
    );
    let note = &schema(&world).objects[&object("note")];
    assert_eq!(note.language_field, Some(field("lang")));
    assert_eq!(note.translation_group, Some(field("group")));
}

// Verifies: INT-2340
#[test]
fn a_publication_names_the_states_of_a_workflow_of_its_state_field() {
    let world = notes_world();
    // No workflow runs on the field.
    let error = refused(
        &world,
        with(
            vec![
                SchemaChange::DefineObject {
                    id: object("page"),
                    label: "Page".into(),
                },
                SchemaChange::DefineField {
                    object: object("page"),
                    field: field_def("status", FieldType::Str, false),
                },
            ],
            vec![SchemaChange::SetPublication {
                object: object("page"),
                publication: PublicationDef {
                    time_field: None,
                    ..publication()
                },
            }],
        ),
    );
    assert!(error
        .message
        .contains("no workflow of page runs on the state field status"));

    // A state the workflow does not have.
    let error = refused(
        &world,
        with(
            article(),
            vec![SchemaChange::SetPublication {
                object: object("article"),
                publication: PublicationDef {
                    withdrawn: "retired".into(),
                    ..publication()
                },
            }],
        ),
    );
    assert!(error
        .message
        .contains("retired is not a state of the workflow article_status"));

    // In review and approved come together.
    let error = refused(
        &world,
        with(
            article(),
            vec![SchemaChange::SetPublication {
                object: object("article"),
                publication: PublicationDef {
                    in_review: Some("draft".into()),
                    ..publication()
                },
            }],
        ),
    );
    assert!(error
        .message
        .contains("in-review and approved are declared together"));

    let world = must(
        &world,
        "owner",
        7_000,
        change(with(
            article(),
            vec![SchemaChange::SetPublication {
                object: object("article"),
                publication: publication(),
            }],
        )),
    );
    assert_eq!(
        schema(&world).objects[&object("article")].publication,
        Some(publication())
    );
}

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

// Verifies: INT-2341
#[test]
fn removing_an_object_type_with_delete_records_deletes_them_in_the_same_commit() {
    let world = scratch_world();
    let (next, transition) = step(
        &world,
        "owner",
        7_100,
        change(vec![SchemaChange::RemoveObject {
            id: object("scratch"),
            delete_records: true,
        }]),
    )
    .expect("accepted");
    assert!(!schema(&next).objects.contains_key(&object("scratch")));
    assert_eq!(
        next.tenants[&tenant_id("alpha")]
            .records_of(&object("scratch"))
            .count(),
        0
    );
    let deleted = transition
        .delta
        .ops
        .iter()
        .filter(|op| matches!(op, DeltaOp::DeleteRecord { .. }))
        .count();
    assert_eq!(deleted, 1);
}

#[test]
fn an_object_type_without_records_is_removed_and_one_that_is_still_named_is_not() {
    let world = must(&notes_world(), "owner", 7_000, change(scratch()));
    let gone = must(
        &world,
        "owner",
        7_001,
        change(vec![SchemaChange::RemoveObject {
            id: object("scratch"),
            delete_records: false,
        }]),
    );
    assert!(!schema(&gone).objects.contains_key(&object("scratch")));
    // `note` is named by policies, queries and a workflow of the schema.
    let error = refused(
        &world,
        vec![SchemaChange::RemoveObject {
            id: object("note"),
            delete_records: true,
        }],
    );
    assert_eq!(error.kind, ErrorKind::InvalidSchema);
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
    DefineMailTemplate(MailTemplateDecl),
    RemoveMailTemplate(TemplateId),
    DefineNoticeTemplate(NoticeTemplateDecl),
    RemoveNoticeTemplate(TemplateId),
    DefineToken(TokenDecl),
    RemoveToken(String),
}

fn catalogue(edit: Edit) -> SchemaChange {
    match edit {
        Edit::DefineMailTemplate(d) => SchemaChange::DefineMailTemplate(d),
        Edit::RemoveMailTemplate(id) => SchemaChange::RemoveMailTemplate(id),
        Edit::DefineNoticeTemplate(d) => SchemaChange::DefineNoticeTemplate(d),
        Edit::RemoveNoticeTemplate(id) => SchemaChange::RemoveNoticeTemplate(id),
        Edit::DefineToken(d) => SchemaChange::DefineToken(d),
        Edit::RemoveToken(name) => SchemaChange::RemoveToken(name),
    }
}

// Verifies: INT-2343
#[test]
fn templates_and_tokens_are_held_in_the_schema_and_listed() {
    let world = must(
        &notes_world(),
        "owner",
        7_000,
        change(vec![
            catalogue(Edit::DefineMailTemplate(mail("welcome"))),
            catalogue(Edit::DefineNoticeTemplate(notice("greeting"))),
            catalogue(Edit::DefineToken(TokenDecl {
                name: "first_name".into(),
                expression: Expression("record.title".into()),
                fallback: "friend".into(),
            })),
        ]),
    );
    let schema = schema(&world);
    let listed: Vec<(EntityKind, String)> = templates::list(&schema)
        .into_iter()
        .map(|t| (t.kind, t.id.to_string()))
        .collect();
    assert_eq!(
        listed,
        vec![
            (EntityKind::MailTemplate, "welcome".to_string()),
            (EntityKind::NoticeTemplate, "greeting".to_string())
        ]
    );
    assert_eq!(templates::tokens(&schema).len(), 1);
}

// Verifies: INT-2343
#[test]
fn a_template_id_is_used_by_one_kind_of_template_only() {
    let error = refused(
        &notes_world(),
        vec![
            catalogue(Edit::DefineMailTemplate(mail("welcome"))),
            catalogue(Edit::DefineNoticeTemplate(notice("welcome"))),
        ],
    );
    assert_eq!(error.kind, ErrorKind::InvalidSchema);
    assert_eq!(
        error.message,
        "template welcome: is both a mail template and a notice template"
    );
}

// Verifies: INT-2343
#[test]
fn a_template_has_a_one_line_subject_and_a_body_that_is_json() {
    let world = notes_world();
    let error = refused(
        &world,
        vec![catalogue(Edit::DefineMailTemplate(MailTemplateDecl {
            subject: "Hi\r\nBcc: someone@example.test".into(),
            ..mail("welcome")
        }))],
    );
    assert!(error.message.contains("the subject has a line break"));
    let error = refused(
        &world,
        vec![catalogue(Edit::DefineNoticeTemplate(NoticeTemplateDecl {
            body: StructuredBlocks("[{\"type\":".into()),
            ..notice("greeting")
        }))],
    );
    assert!(error.message.contains("the body is not JSON text"));
}

// Verifies: CTK-002
#[test]
fn a_token_whose_expression_does_not_parse_is_refused() {
    let error = refused(
        &notes_world(),
        vec![catalogue(Edit::DefineToken(TokenDecl {
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

#[test]
fn removing_the_templates_and_tokens_restores_the_hash_and_what_is_not_there_is_refused() {
    let world = notes_world();
    let before = hash(&world);
    let defined = must(
        &world,
        "owner",
        7_000,
        change(vec![
            catalogue(Edit::DefineMailTemplate(mail("welcome"))),
            catalogue(Edit::DefineNoticeTemplate(notice("greeting"))),
            catalogue(Edit::DefineToken(TokenDecl {
                name: "first_name".into(),
                expression: Expression("record.title".into()),
                fallback: "friend".into(),
            })),
        ]),
    );
    assert_ne!(hash(&defined), before);
    let removed = must(
        &defined,
        "owner",
        7_001,
        change(vec![
            catalogue(Edit::RemoveMailTemplate(TemplateId("welcome".into()))),
            catalogue(Edit::RemoveNoticeTemplate(TemplateId("greeting".into()))),
            catalogue(Edit::RemoveToken("first_name".into())),
        ]),
    );
    assert_eq!(hash(&removed), before);
    let removed = schema(&removed);
    assert!(
        removed.mail_templates.is_none()
            && removed.notice_templates.is_none()
            && removed.tokens.is_none()
    );
    let error = refused(
        &world,
        vec![catalogue(Edit::RemoveMailTemplate(TemplateId(
            "nothing".into(),
        )))],
    );
    assert_eq!(error.message, "no mail template nothing");
}

#[test]
fn an_automation_does_not_send_a_notice_template_as_mail() {
    let world = notes_world();
    let step = |action| AutomationStepDef {
        action,
        next: None,
        retry: None,
        timeout: None,
        compensate: None,
    };
    let automation = AutomationDef {
        id: WorkflowId::new("greet").unwrap(),
        label: "Greet".into(),
        run_as: RunAs {
            roles: BTreeSet::new(),
        },
        run_key: None,
        start: StepName::new("send").unwrap(),
        steps: BTreeMap::from([(
            StepName::new("send").unwrap(),
            step(AutomationStep::SendMail {
                template: TemplateId("greeting".into()),
                to: Expr::Param("owner".into()),
                category: None,
            }),
        )]),
        defined_by: None,
        defined_at: None,
        suspended_for: None,
    };
    let error = refused(
        &world,
        vec![
            catalogue(Edit::DefineNoticeTemplate(notice("greeting"))),
            SchemaChange::DefineAutomation(automation),
        ],
    );
    assert!(
        error
            .message
            .contains("sends the notice template greeting as mail"),
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
    catalogue(Edit::DefineNoticeTemplate(NoticeTemplateDecl {
        body: body(body_json),
        ..notice("greeting")
    }))
}

fn first_name() -> SchemaChange {
    catalogue(Edit::DefineToken(TokenDecl {
        name: "first_name".into(),
        expression: Expression("record.title".into()),
        fallback: "friend".into(),
    }))
}

// Verifies: CTK-005
#[test]
fn a_template_that_places_a_token_the_tenant_does_not_define_is_refused() {
    let placing = r#"[{"type":"paragraph","inlines":[{"type":"token","name":"first_name"}]}]"#;
    let error = refused(&notes_world(), vec![notice_with(placing)]);
    assert_eq!(error.kind, ErrorKind::InvalidSchema);
    assert_eq!(
        error.message,
        "notice template greeting: the body places the token first_name, which the tenant does not define"
    );
    // Defined in the same change or before, it is accepted.
    let world = must(
        &notes_world(),
        "owner",
        7_000,
        change(vec![first_name(), notice_with(placing)]),
    );
    // CTK-004: a token that a template places is not removed.
    let error = refused(&world, vec![SchemaChange::RemoveToken("first_name".into())]);
    assert!(
        error
            .message
            .contains("the body places the token first_name, which the tenant does not define"),
        "{}",
        error.message
    );
}

// Verifies: CTK-003
#[test]
fn a_token_expression_cannot_name_another_token() {
    // The tenant rule language reads record fields and event parameters only, so a token that
    // names another, itself or a cycle of tokens does not parse.
    for expression in ["token.first_name", "token.other + token.first_name"] {
        let error = refused(
            &notes_world(),
            vec![catalogue(Edit::DefineToken(TokenDecl {
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
#[test]
fn a_body_of_every_kind_of_block_is_accepted() {
    let every = r##"[
        {"type":"heading","level":2,"inlines":[{"type":"text","text":"Hello "},{"type":"token","name":"first_name"}]},
        {"type":"paragraph","inlines":[
            {"type":"strong","children":[{"type":"text","text":"Points: "}]},
            {"type":"value","name":"points","value_type":"int"},
            {"type":"em","children":[{"type":"code","text":"x"}]},
            {"type":"link","href":"https://example.test/a","children":[{"type":"text","text":"site"}]},
            {"type":"link","href":"mailto:a@example.test","children":[]},
            {"type":"link","href":"/badges","children":[]},
            {"type":"link","href":"#top","children":[]}
        ]},
        {"type":"quote","blocks":[{"type":"paragraph","inlines":[]}]},
        {"type":"list","ordered":true,"items":[[{"type":"text","text":"one"}],[]]},
        {"type":"code","lang":"rust","text":"let a = 1;"},
        {"type":"figure","src":"/media/badge.png","alt":"A badge","caption":"Silver"},
        {"type":"rule"}
    ]"##;
    must(
        &notes_world(),
        "owner",
        7_000,
        change(vec![first_name(), notice_with(every)]),
    );
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

// Verifies: EML-012
#[test]
fn a_value_is_placed_with_one_declared_type() {
    let message = bad(
        &notes_world(),
        r#"[{"type":"paragraph","inlines":[
            {"type":"value","name":"points","value_type":"int"},
            {"type":"value","name":"points","value_type":"str"}]}]"#,
    );
    assert!(
        message.contains("the value points is placed as str here and as int before"),
        "{message}"
    );
}

// Verifies: CMS-004
#[test]
fn quotes_and_inline_elements_nest_no_deeper_than_the_markdown_parser_keeps_them() {
    let world = notes_world();
    let quotes = |depth: usize| {
        let mut json = r#"{"type":"paragraph","inlines":[]}"#.to_string();
        for _ in 0..depth {
            json = format!(r#"{{"type":"quote","blocks":[{json}]}}"#);
        }
        format!("[{json}]")
    };
    let emphasis = |depth: usize| {
        let mut json = r#"{"type":"text","text":"x"}"#.to_string();
        for _ in 0..depth {
            json = format!(r#"{{"type":"em","children":[{json}]}}"#);
        }
        format!(r#"[{{"type":"paragraph","inlines":[{json}]}}]"#)
    };
    must(
        &world,
        "owner",
        7_000,
        change(vec![notice_with(&quotes(blocks::MAX_QUOTE_DEPTH))]),
    );
    assert!(
        bad(&world, &quotes(blocks::MAX_QUOTE_DEPTH + 1)).contains("quotes nest more than 8 deep")
    );
    must(
        &world,
        "owner",
        7_001,
        change(vec![notice_with(&emphasis(blocks::MAX_INLINE_DEPTH))]),
    );
    assert!(bad(&world, &emphasis(blocks::MAX_INLINE_DEPTH + 1))
        .contains("inline elements nest more than 8 deep"));
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

// Verifies: EML-012
#[test]
fn a_body_is_not_rendered_when_a_value_or_a_token_is_missing_or_of_another_type() {
    let json = r#"[{"type":"paragraph","inlines":[{"type":"value","name":"points","value_type":"int"},{"type":"token","name":"first_name"}]}]"#;
    assert_eq!(
        rendered(json, &[]),
        Err(blocks::RenderError::MissingValue("points".into()))
    );
    assert_eq!(
        rendered(json, &[("points", Value::Str("500".into()))]),
        Err(blocks::RenderError::WrongType {
            name: "points".into(),
            declared: blocks::ValueType::Int
        })
    );
    let unknown = r#"[{"type":"paragraph","inlines":[{"type":"token","name":"nobody"}]}]"#;
    assert_eq!(
        rendered(unknown, &[]),
        Err(blocks::RenderError::UnresolvedToken("nobody".into()))
    );
    assert!(matches!(
        rendered(r#"[{"type":"paragraph"}]"#, &[]),
        Err(blocks::RenderError::Invalid(_))
    ));
    // A body the kernel refused at definition is never rendered either.
    let message = rendered("not json", &[]).unwrap_err().to_string();
    assert!(
        message.starts_with("the body is not blocks: is not JSON text"),
        "{message}"
    );
}

// ---------------------------------------------------------------------------------------------
// The ownership record of a product (D-6582).
// ---------------------------------------------------------------------------------------------

fn owned(kind: EntityKind, id: &str) -> EntityRef {
    EntityRef {
        kind,
        id: id.to_string(),
    }
}

fn record(product: &str, owns: Vec<EntityRef>) -> SchemaChange {
    SchemaChange::RecordProduct(InstalledProduct {
        product: ProductId::new(product).unwrap(),
        version: "1.0.0".into(),
        platform: None,
        owns,
        schema_version: None,
    })
}

// Verifies: INT-2342
#[test]
fn the_record_of_a_product_is_committed_with_the_entities_it_names() {
    let world = notes_world();
    let before = hash(&world);
    let installed = must(
        &world,
        "owner",
        7_000,
        change(vec![
            SchemaChange::DefineObject {
                id: object("badges__award"),
                label: "Award".into(),
            },
            record("badges", vec![owned(EntityKind::Object, "badges__award")]),
        ]),
    );
    let schema = schema(&installed);
    let products = products::list(&schema);
    assert_eq!(products.len(), 1);
    assert_eq!(products[0].product.as_str(), "badges");
    assert_eq!(
        products::owner_of(&schema, EntityKind::Object, "badges__award").map(ProductId::as_str),
        Some("badges")
    );
    assert!(products::owner_of(&schema, EntityKind::Object, "note").is_none());

    // Removing the product removes its record and the entity in one change.
    let removed = must(
        &installed,
        "owner",
        7_001,
        change(vec![
            SchemaChange::RemoveObject {
                id: object("badges__award"),
                delete_records: false,
            },
            SchemaChange::RemoveProduct(ProductId::new("badges").unwrap()),
        ]),
    );
    assert_eq!(hash(&removed), before);
}

#[test]
fn a_record_that_names_an_entity_that_does_not_exist_or_has_two_owners_is_refused() {
    let world = notes_world();
    let error = refused(
        &world,
        vec![record("badges", vec![owned(EntityKind::Object, "ghost")])],
    );
    assert_eq!(error.kind, ErrorKind::InvalidSchema);
    assert!(error
        .message
        .contains("owns the Object ghost, which does not exist"));
    let error = refused(
        &world,
        vec![
            record("one", vec![owned(EntityKind::Object, "note")]),
            record("two", vec![owned(EntityKind::Object, "note")]),
        ],
    );
    assert!(
        error.message.contains("is owned by both one and two"),
        "{}",
        error.message
    );
}

#[test]
fn removing_the_record_of_a_product_that_is_not_installed_is_refused() {
    let error = refused(
        &notes_world(),
        vec![SchemaChange::RemoveProduct(
            ProductId::new("badges").unwrap(),
        )],
    );
    assert_eq!(error.message, "no product badges");
}

#[test]
fn a_schema_that_declares_nothing_new_keeps_the_hash_and_the_encoding_it_had() {
    let world = notes_world();
    let text = serde_json::to_string(&*world.tenants[&tenant_id("alpha")].schema).unwrap();
    for member in [
        "mail_templates",
        "notice_templates",
        "tokens",
        "products",
        "retention",
        "publication",
        "language_field",
    ] {
        assert!(!text.contains(member), "{member}");
    }
    let decoded: TenantSchema = serde_json::from_str(&text).unwrap();
    assert!(decoded.products.is_none() && decoded.tokens.is_none());
}

// ---------------------------------------------------------------------------------------------
// A change from outside a product to an entity it owns (D-6582, INT-1582).
// ---------------------------------------------------------------------------------------------

fn record_at(product: &str, version: &str, owns: Vec<EntityRef>) -> SchemaChange {
    SchemaChange::RecordProduct(InstalledProduct {
        product: ProductId::new(product).unwrap(),
        version: version.into(),
        platform: None,
        owns,
        schema_version: None,
    })
}

fn badges_entities() -> Vec<SchemaChange> {
    vec![
        SchemaChange::DefineObject {
            id: object("badges__award"),
            label: "Award".into(),
        },
        SchemaChange::DefineField {
            object: object("badges__award"),
            field: field_def("note", FieldType::Str, false),
        },
        SchemaChange::DefineRole(RoleDef {
            id: role("badges__awarder"),
            label: "Awarder".into(),
        }),
        catalogue(Edit::DefineToken(TokenDecl {
            name: "badges__first".into(),
            expression: Expression("record.title".into()),
            fallback: "friend".into(),
        })),
        catalogue(Edit::DefineMailTemplate(mail("badges__welcome"))),
    ]
}

fn badges_owns() -> Vec<EntityRef> {
    vec![
        owned(EntityKind::Object, "badges__award"),
        owned(EntityKind::Role, "badges__awarder"),
        owned(EntityKind::Token, "badges__first"),
        owned(EntityKind::MailTemplate, "badges__welcome"),
    ]
}

fn badges_world() -> World {
    let mut changes = badges_entities();
    changes.push(record_at("badges", "1.0.0", badges_owns()));
    must(&notes_world(), "owner", 7_000, change(changes))
}

fn add_extra_field() -> SchemaChange {
    SchemaChange::DefineField {
        object: object("badges__award"),
        field: field_def("extra", FieldType::Str, false),
    }
}

// Verifies: INT-1582
#[test]
fn a_schema_editor_may_not_change_an_entity_a_product_owns() {
    let world = badges_world();
    for who in ["owner", "editor"] {
        let error = step(&world, who, 8_000, change(vec![add_extra_field()])).expect_err("refused");
        assert_eq!(error.kind, ErrorKind::InvalidSchema);
        assert_eq!(
            error.message,
            "the object badges__award is owned by the product badges; it changes only through that product's upgrade or removal"
        );
    }
    // The role, the token and the template are as guarded as the record type.
    for (change_of, kind, id) in [
        (
            SchemaChange::DefineRolePermissions {
                role: role("badges__awarder"),
                permissions: Vec::new(),
            },
            "role",
            "badges__awarder",
        ),
        (
            SchemaChange::RemoveToken("badges__first".into()),
            "token",
            "badges__first",
        ),
        (
            catalogue(Edit::DefineMailTemplate(mail("badges__welcome"))),
            "mail-template",
            "badges__welcome",
        ),
    ] {
        let error = refused(&world, vec![change_of]);
        assert!(
            error
                .message
                .starts_with(&format!("the {kind} {id} is owned by the product badges")),
            "{}",
            error.message
        );
    }
    // What no product owns is not guarded.
    must(
        &world,
        "editor",
        8_001,
        change(vec![SchemaChange::DefineField {
            object: object("note"),
            field: field_def("extra", FieldType::Str, false),
        }]),
    );
}

// Verifies: INT-1582
#[test]
fn a_product_upgrade_changes_what_it_owns_and_nothing_else_does() {
    let world = badges_world();
    // The upgrade path: the record of the product at another version, in the same change.
    let upgraded = must(
        &world,
        "owner",
        8_000,
        change(vec![
            add_extra_field(),
            record_at("badges", "1.1.0", badges_owns()),
        ]),
    );
    let after = schema(&upgraded);
    assert_eq!(
        products::get(&after, &ProductId::new("badges").unwrap())
            .unwrap()
            .version,
        "1.1.0"
    );
    assert!(after.objects[&object("badges__award")]
        .fields
        .contains_key(&field("extra")));

    // A record at the version already installed is no upgrade.
    let error = refused(
        &world,
        vec![
            add_extra_field(),
            record_at("badges", "1.0.0", badges_owns()),
        ],
    );
    assert!(
        error.message.contains("is already installed"),
        "{}",
        error.message
    );

    // The upgrade of another product does not license the change.
    let both = must(
        &world,
        "owner",
        8_000,
        change(vec![record_at("other", "1.0.0", Vec::new())]),
    );
    let error = refused(
        &both,
        vec![add_extra_field(), record_at("other", "1.1.0", Vec::new())],
    );
    assert!(
        error
            .message
            .starts_with("the object badges__award is owned by the product badges"),
        "{}",
        error.message
    );
}

// Verifies: INT-1582
#[test]
fn a_schema_editor_cannot_forge_the_upgrade_of_a_product() {
    let world = badges_world();
    // The editor manages the schema but does not hold bundle-install, so a product record in its
    // change is refused before the entity is looked at.
    let error = step(
        &world,
        "editor",
        8_000,
        change(vec![
            add_extra_field(),
            record_at("badges", "9.9.9", badges_owns()),
        ]),
    )
    .expect_err("refused");
    assert_eq!(error.kind, ErrorKind::Unauthorized);
    assert_eq!(
        error.message,
        "the change needs the capability bundle-install"
    );
    let error = step(
        &world,
        "editor",
        8_001,
        change(vec![SchemaChange::RemoveProduct(
            ProductId::new("badges").unwrap(),
        )]),
    )
    .expect_err("refused");
    assert_eq!(error.kind, ErrorKind::Unauthorized);
}

// Verifies: INT-1582
#[test]
fn the_removal_of_a_product_removes_every_entity_it_owns() {
    let world = badges_world();
    let removed = must(
        &world,
        "owner",
        8_000,
        change(vec![
            SchemaChange::RemoveMailTemplate(TemplateId("badges__welcome".into())),
            SchemaChange::RemoveToken("badges__first".into()),
            SchemaChange::RemoveRole(role("badges__awarder")),
            SchemaChange::RemoveObject {
                id: object("badges__award"),
                delete_records: false,
            },
            SchemaChange::RemoveProduct(ProductId::new("badges").unwrap()),
        ]),
    );
    assert_eq!(hash(&removed), hash(&notes_world()));
}

// Verifies: INT-1582
#[test]
fn every_kind_of_entity_a_product_can_own_is_guarded() {
    let pool = || LedgerPoolName::new("badges__entity").unwrap();
    let unit = || LedgerUnit::new("badges__entity").unwrap();
    // One removal for each kind except the segment, which the schema does not hold.
    let by_kind: Vec<(EntityKind, String, SchemaChange)> = vec![
        (
            EntityKind::Object,
            "badges__entity".into(),
            SchemaChange::RemoveObject {
                id: object("badges__entity"),
                delete_records: false,
            },
        ),
        (
            EntityKind::Role,
            "badges__entity".into(),
            SchemaChange::RemoveRole(role("badges__entity")),
        ),
        (
            EntityKind::Capability,
            "badges__entity".into(),
            SchemaChange::RemoveCapability(CapabilityName("badges__entity".into())),
        ),
        (
            EntityKind::Policy,
            "badges__entity".into(),
            SchemaChange::RemovePolicy(PolicyId::new("badges__entity").unwrap()),
        ),
        (
            EntityKind::Workflow,
            "badges__entity".into(),
            SchemaChange::RemoveWorkflow(WorkflowId::new("badges__entity").unwrap()),
        ),
        (
            EntityKind::Query,
            "badges__entity".into(),
            SchemaChange::RemoveQuery(QueryId::new("badges__entity").unwrap()),
        ),
        (
            EntityKind::View,
            "badges__entity".into(),
            SchemaChange::RemoveView(ViewId::new("badges__entity").unwrap()),
        ),
        (
            EntityKind::Setting,
            "badges__entity".into(),
            SchemaChange::RemoveSetting(SettingId::new("badges__entity").unwrap()),
        ),
        (
            EntityKind::LedgerUnit,
            "badges__entity".into(),
            SchemaChange::RemoveLedgerDefinition(LedgerDefinitionRef::Unit { unit: unit() }),
        ),
        (
            EntityKind::LedgerPool,
            "badges__entity".into(),
            SchemaChange::RemoveLedgerDefinition(LedgerDefinitionRef::Pool { pool: pool() }),
        ),
        (
            EntityKind::LedgerLimit,
            products::ledger_limit_id(&unit(), &None, &None),
            SchemaChange::RemoveLedgerDefinition(LedgerDefinitionRef::Limit {
                unit: unit(),
                owner: None,
                scope: None,
            }),
        ),
        (
            EntityKind::MailTemplate,
            "badges__entity".into(),
            SchemaChange::RemoveMailTemplate(TemplateId("badges__entity".into())),
        ),
        (
            EntityKind::NoticeTemplate,
            "badges__entity".into(),
            SchemaChange::RemoveNoticeTemplate(TemplateId("badges__entity".into())),
        ),
        (
            EntityKind::Token,
            "badges__entity".into(),
            SchemaChange::RemoveToken("badges__entity".into()),
        ),
        (
            EntityKind::Trigger,
            "badges__entity".into(),
            SchemaChange::RemoveTrigger(TriggerId::new("badges__entity").unwrap()),
        ),
        (
            EntityKind::Automation,
            "badges__entity".into(),
            SchemaChange::RemoveAutomation(WorkflowId::new("badges__entity").unwrap()),
        ),
        (
            EntityKind::Operation,
            "badges__entity".into(),
            SchemaChange::RemoveOperation(TenantOperationId::new("badges__entity").unwrap()),
        ),
    ];
    let product = || ProductId::new("badges").unwrap();
    let schema = TenantSchema {
        products: Some(BTreeMap::from([(
            product(),
            InstalledProduct {
                product: product(),
                version: "1.0.0".into(),
                platform: None,
                owns: by_kind
                    .iter()
                    .map(|(kind, id, _)| owned(*kind, id))
                    .collect(),
                schema_version: None,
            },
        )])),
        ..schema(&notes_world())
    };
    // Every kind the specification lists but the segment is in the table.
    assert_eq!(by_kind.len(), 17);
    for (kind, id, change_of) in by_kind {
        assert_eq!(
            products::touched(&change_of),
            vec![owned(kind, &id)],
            "{kind:?}"
        );
        let message = products::foreign_change(&schema, std::slice::from_ref(&change_of))
            .unwrap_or_else(|| panic!("{kind:?} is not guarded"));
        assert_eq!(
            message,
            format!(
                "the {} {id} is owned by the product badges; it changes only through that product's upgrade or removal",
                kind.as_str()
            )
        );
        // The product's removal and its upgrade let the change through; another product's do not.
        assert!(
            products::foreign_change(
                &schema,
                &[change_of.clone(), SchemaChange::RemoveProduct(product())]
            )
            .is_none(),
            "{kind:?}"
        );
        assert!(
            products::foreign_change(
                &schema,
                &[change_of.clone(), record_at("badges", "2.0.0", Vec::new())]
            )
            .is_none(),
            "{kind:?}"
        );
        assert!(
            products::foreign_change(
                &schema,
                &[change_of, record_at("other", "2.0.0", Vec::new())]
            )
            .is_some(),
            "{kind:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Removing a role.
// ---------------------------------------------------------------------------------------------

// Verifies: INT-2341
#[test]
fn a_role_that_is_held_is_not_removed_and_one_that_is_not_takes_its_permissions_with_it() {
    let world = notes_world();
    let before = hash(&world);
    let defined = must(
        &world,
        "owner",
        7_000,
        change(vec![
            SchemaChange::DefineRole(RoleDef {
                id: role("reviewer"),
                label: "Reviewer".into(),
            }),
            SchemaChange::DefineRolePermissions {
                role: role("reviewer"),
                permissions: vec![Permission::Capability("content-read".into())],
            },
        ]),
    );
    let held = must(
        &defined,
        "owner",
        7_001,
        Intent::AssignRole {
            principal: principal_id("alpha:stranger"),
            role: role("reviewer"),
        },
    );
    let remove = || vec![SchemaChange::RemoveRole(role("reviewer"))];
    let error = refused(&held, remove());
    assert_eq!(error.kind, ErrorKind::InvalidSchema);
    assert!(error
        .message
        .contains("role reviewer is held by 1 principal(s)"));

    let revoked = must(
        &held,
        "owner",
        7_002,
        Intent::RevokeRole {
            principal: principal_id("alpha:stranger"),
            role: role("reviewer"),
        },
    );
    let removed = must(&revoked, "owner", 7_003, change(remove()));
    assert_eq!(hash(&removed), before);

    let error = refused(&world, vec![SchemaChange::RemoveRole(role("owner"))]);
    assert_eq!(error.message, "owner is built in");
    let error = refused(&world, vec![SchemaChange::RemoveRole(role("ghost"))]);
    assert_eq!(error.message, "no role ghost");
}
