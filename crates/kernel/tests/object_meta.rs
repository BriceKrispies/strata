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

fn blocks(text: &str) -> StructuredBlocks {
    StructuredBlocks(format!("[{{\"type\":\"paragraph\",\"text\":\"{text}\"}}]"))
}

fn mail(id: &str) -> MailTemplateDecl {
    MailTemplateDecl {
        template: TemplateId(id.to_string()),
        subject: "Welcome".into(),
        body: blocks("Hello"),
    }
}

fn notice(id: &str) -> NoticeTemplateDecl {
    NoticeTemplateDecl {
        template: TemplateId(id.to_string()),
        title: "Welcome".into(),
        body: blocks("Hello"),
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
