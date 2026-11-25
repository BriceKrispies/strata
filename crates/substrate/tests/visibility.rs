//! Who hears of a record's events (D-8402, D-8428): the audience of an event about a record is the
//! Read decision for that record, and a subscription that names a type the subscriber has no Read
//! grant on is refused.
//!
//! The scenario is the reader engagement of a content module. An engagement records who viewed or
//! reacted to a piece of content. It is a record, and its audience is stated as two Read policies:
//! the content editors read every engagement, and the content authors read the engagements of the
//! content they created, through the reference the engagement holds to its content.

use std::collections::BTreeMap;
use strata_kernel::testkit::*;
use strata_kernel::*;
use strata_substrate::subscribe::{admit, notice, notice_in, Notice, Selector, SelectorExt};

fn policy(id: &str, role_name: &str, object_name: &str, condition: Option<Expr>) -> SchemaChange {
    SchemaChange::DefinePolicy(PolicyDef {
        id: PolicyId::new(id).unwrap(),
        role: role(role_name),
        resource: Resource::Object(object(object_name)),
        actions: [Action::Read, Action::Create].into_iter().collect(),
        condition,
    })
}

/// The tenant: content by authors, one engagement per piece of content, and four people: an
/// editor, two authors (one for each piece of content) and a reader who holds neither role.
struct Scene {
    world: World,
    /// The events of the two engagements being recorded.
    first: Vec<DomainEvent>,
    second: Vec<DomainEvent>,
}

fn scene() -> Scene {
    let mut now = 10_000;
    let mut step = |world: &World, who: &str, intent: Intent| {
        now += 1;
        world
            .step(&intent, &ctx("alpha", user(&format!("alpha:{who}")), now))
            .unwrap_or_else(|e| panic!("fixture refused: {}", e.describe()))
    };
    let authors_own = eq(
        Expr::Through(Scope::Record, "content".into(), "$created_by".into()),
        Expr::Principal,
    );
    let (mut world, _) = step(
        &notes_world(),
        "owner",
        Intent::ChangeSchema {
            changes: vec![
                SchemaChange::DefineRole(RoleDef {
                    id: role("content_editor"),
                    label: "Content editor".into(),
                }),
                SchemaChange::DefineRole(RoleDef {
                    id: role("content_author"),
                    label: "Content author".into(),
                }),
                SchemaChange::DefineRole(RoleDef {
                    id: role("reader"),
                    label: "Reader".into(),
                }),
                SchemaChange::DefineObject {
                    id: object("content"),
                    label: "Content".into(),
                },
                SchemaChange::DefineField {
                    object: object("content"),
                    field: field_def("title", FieldType::Str, true),
                },
                SchemaChange::DefineObject {
                    id: object("engagement"),
                    label: "Engagement".into(),
                },
                SchemaChange::DefineRelationship {
                    object: object("engagement"),
                    field: field("content"),
                    label: "Content".into(),
                    target: object("content"),
                    on_delete: OnDelete::SetNull,
                    required: false,
                },
                SchemaChange::DefineField {
                    object: object("engagement"),
                    field: field_def("views", FieldType::Int, false),
                },
                policy("authors_write_content", "content_author", "content", None),
                policy("editors_read_content", "content_editor", "content", None),
                // The audience of an engagement: the content editors, and the author of the
                // content it refers to. No other role has a policy on the type.
                policy(
                    "editors_read_engagement",
                    "content_editor",
                    "engagement",
                    None,
                ),
                policy(
                    "authors_read_their_engagement",
                    "content_author",
                    "engagement",
                    Some(authors_own),
                ),
            ],
            expected_version: None,
            existing: ExistingData::Reject,
        },
    );
    for (who, held) in [
        ("editor", "content_editor"),
        ("writer1", "content_author"),
        ("writer2", "content_author"),
        ("stranger", "reader"),
    ] {
        world = step(
            &world,
            "owner",
            Intent::AssignRole {
                principal: principal_id(&format!("alpha:{who}")),
                role: role(held),
            },
        )
        .0;
    }
    let mut content = Vec::new();
    for (who, title) in [("writer1", "First"), ("writer2", "Second")] {
        let (next, done) = step(
            &world,
            who,
            Intent::CreateRecord {
                object: object("content"),
                fields: BTreeMap::from([(field("title"), text(title))]),
            },
        );
        world = next;
        let Output::Record { id, .. } = done.output else {
            panic!("a record was created");
        };
        content.push(id);
    }
    let mut engagements = Vec::new();
    for id in content {
        let (next, done) = step(
            &world,
            "owner",
            Intent::CreateRecord {
                object: object("engagement"),
                fields: BTreeMap::from([
                    (field("content"), Value::Ref(id)),
                    (field("views"), Value::Int(1)),
                ]),
            },
        );
        world = next;
        engagements.push(done.events);
    }
    let second = engagements.pop().unwrap();
    let first = engagements.pop().unwrap();
    Scene {
        world,
        first,
        second,
    }
}

/// What `who` is told of `events` by the delivery path: decided over a snapshot that holds the
/// records the audience of each event follows into, as the shell loads them.
fn heard(scene: &Scene, who: &str, events: &[DomainEvent]) -> Vec<Notice> {
    let state = scene.world.state(&tenant_id("alpha"));
    events
        .iter()
        .filter_map(|event| {
            notice_in(
                &state,
                &user(&format!("alpha:{who}")),
                &Selector::all(),
                event,
            )
        })
        .collect()
}

fn engagement_selector() -> Selector {
    Selector {
        objects: Some([object("engagement")].into()),
        ..Selector::all()
    }
}

// Verifies: ADM-1610
#[test]
fn an_event_reaches_only_the_principals_its_audience_allows() {
    let scene = scene();
    let upserts = |notices: Vec<Notice>| {
        notices
            .iter()
            .filter(
                |n| matches!(n, Notice::Upsert { object, .. } if object.as_str() == "engagement"),
            )
            .count()
    };
    // The audience of the engagement of the first content is its editors and its author. The
    // second author, a reader with no role on the type and the anonymous visitor hear nothing of
    // it, whatever they subscribed to.
    assert_eq!(upserts(heard(&scene, "editor", &scene.first)), 1);
    assert_eq!(upserts(heard(&scene, "writer1", &scene.first)), 1);
    for nobody in ["writer2", "stranger"] {
        assert_eq!(upserts(heard(&scene, nobody, &scene.first)), 0, "{nobody}");
    }
    let tenant = &scene.world.tenants[&tenant_id("alpha")];
    for event in &scene.first {
        assert_eq!(
            notice(tenant, &Principal::Anonymous, &Selector::all(), event),
            None
        );
    }
    // The delivery path loads the content the engagement refers to before it decides. Without it
    // the author's audience cannot be decided and nothing is told to the author, while the
    // editors, whose audience follows no reference, are still told.
    let tenant = &scene.world.tenants[&tenant_id("alpha")];
    let EventBody::RecordCreated { record } = &scene.first[0].body else {
        panic!("the engagement was created");
    };
    assert_eq!(audience_needs(&tenant.schema, record).len(), 1);
    let bare = |who: &str| {
        notice(
            tenant,
            &user(&format!("alpha:{who}")),
            &Selector::all(),
            &scene.first[0],
        )
    };
    assert_eq!(bare("writer1"), None);
    assert!(bare("editor").is_some());
    // And the notice names no field of the engagement: it carries the id and the version only.
    let told = heard(&scene, "editor", &scene.first);
    assert!(matches!(
        told.as_slice(),
        [Notice::Upsert { version: 1, .. }]
    ));
}

// Verifies: ADM-1632
#[test]
fn a_reader_engagement_event_reaches_only_its_author_and_the_content_editors() {
    let scene = scene();
    let tenant = &scene.world.tenants[&tenant_id("alpha")];
    let told = |who: &str, events: &[DomainEvent]| {
        heard(&scene, who, events)
            .iter()
            .any(|n| matches!(n, Notice::Upsert { object, .. } if object.as_str() == "engagement"))
    };

    // The author of the content and the content editors are told of its engagement, and the
    // author of other content is not.
    assert!(told("writer1", &scene.first));
    assert!(told("editor", &scene.first));
    assert!(!told("writer2", &scene.first));
    assert!(told("writer2", &scene.second));
    assert!(!told("writer1", &scene.second));
    assert!(told("editor", &scene.second));
    assert!(!told("stranger", &scene.first) && !told("stranger", &scene.second));

    // A subscription to the engagements by a member who holds neither role is refused, with the
    // refusal the requirement names; the two roles are admitted, and so is the owner.
    let selector = engagement_selector();
    let refused = admit(tenant, &user("alpha:stranger"), &selector)
        .expect_err("a reader has no audience for engagements");
    assert_eq!(refused.kind, ErrorKind::AudienceNotPermitted);
    for who in ["alpha:editor", "alpha:writer1", "alpha:owner"] {
        assert!(admit(tenant, &user(who), &selector).is_ok(), "{who}");
    }
    assert_eq!(
        admit(tenant, &Principal::Anonymous, &selector)
            .expect_err("the public has no audience for engagements")
            .kind,
        ErrorKind::AudienceNotPermitted
    );

    // A type the tenant does not have is refused the same way, so the refusal tells nothing about
    // which types exist; a selector that names no type is admitted and decided event by event.
    let unknown = Selector {
        objects: Some([object("nothing")].into()),
        ..Selector::all()
    };
    assert_eq!(
        admit(tenant, &user("alpha:stranger"), &unknown)
            .expect_err("no such type")
            .kind,
        ErrorKind::AudienceNotPermitted
    );
    assert!(admit(tenant, &user("alpha:stranger"), &Selector::all()).is_ok());
}
